//! `claudio --daemon`: the per-host session daemon.
//!
//! One daemon per host owns the PTYs running `claude`, so closing the TUI or
//! dropping ssh never kills a session. TUI clients and hook relays talk to it
//! over a private Unix socket using [`crate::proto`].
//!
//! - [`server`] — socket, handshake, per-client connection loop, requests.
//! - [`session`] — one actor task per live session (PTY, screen, hooks).
//! - [`journal`] — the durable session list (dormant sessions survive restarts).
//! - [`host`] — host facts for `Welcome`, directory listing.
//!
//! This module holds the shared [`Daemon`] state: the registry of journaled
//! and live sessions, and the all-clients event broadcast.

pub mod ctl;
mod host;
mod journal;
mod server;
mod session;
mod stats;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use tokio::sync::{broadcast, OnceCell};
use tracing_subscriber::EnvFilter;

use crate::paths;
use crate::proto::{
    Envelope, Frame, HostInfo, Msg, SessionEvent, SessionId, SessionInfo, SessionKind, SessionState,
    SpawnSpec,
};
use journal::{Entry, Journal};
use session::{Cmd, Handle};

/// Capacity of the all-clients event broadcast. A client that falls this far
/// behind misses events (it is told via a log line; the next `ListSessions`
/// resyncs it).
const EVENTS_CAPACITY: usize = 1024;

/// How long `ListSessions` waits for a busy session actor's status.
const STATUS_TIMEOUT: Duration = Duration::from_secs(2);

/// Where the daemon lives and what it runs. Injectable so tests can run a
/// daemon in a temp dir against a fake `claude`.
#[derive(Debug, Clone)]
pub struct Config {
    /// The control socket (also handed to hooks).
    pub socket: PathBuf,
    /// The startup lock file; held for the daemon's lifetime.
    pub lock: PathBuf,
    /// The session journal.
    pub journal: PathBuf,
    /// The claude binary (`$CLAUDIO_CLAUDE_PATH`, else `claude` from `PATH`).
    pub claude: PathBuf,
    /// This binary, used as the hook relay command.
    pub claudio: PathBuf,
}

impl Config {
    /// The standard locations from [`paths`].
    pub fn from_env() -> io::Result<Config> {
        let claude = std::env::var_os("CLAUDIO_CLAUDE_PATH")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(resolve_claude_path);
        Ok(Config {
            socket: paths::daemon_socket(),
            lock: paths::daemon_lock(),
            journal: paths::daemon_journal(),
            claude,
            claudio: std::env::current_exe()?,
        })
    }
}

/// Resolve the `claude` binary path.
///
/// Non-interactive SSH shells often have a minimal PATH that omits
/// `~/.local/bin`, `~/.npm-global/bin` and similar. We try PATH first
/// then fall back through well-known locations so remote sessions still
/// find `claude` without the user touching their shell profile.
pub fn resolve_claude_path() -> PathBuf {
    // 1. PATH lookup.
    if let Some(p) = which_claude() {
        return p;
    }
    // 2. Well-known fallback locations, checked in priority order.
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"));
    let candidates = [
        home.join(".local/bin/claude"),
        home.join(".claude/local/claude"),
        PathBuf::from("/usr/local/bin/claude"),
        PathBuf::from("/opt/homebrew/bin/claude"),
        home.join(".npm-global/bin/claude"),
    ];
    for p in &candidates {
        if p.is_file() {
            return p.clone();
        }
    }
    // 3. Give up — return the bare name so later PATH lookups at spawn time
    // still have a chance.
    PathBuf::from("claude")
}

/// Search `$PATH` for `claude`.
fn which_claude() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join("claude"))
        .find(|p| p.is_file())
}

#[cfg(test)]
mod claude_path_tests {
    use super::*;

    #[test]
    fn resolve_returns_a_path() {
        // Smoke test: the function must not panic, and it must return
        // something (even if that something is the bare "claude" name).
        let p = resolve_claude_path();
        assert!(!p.as_os_str().is_empty());
    }

    #[test]
    fn which_claude_finds_existing_binary() {
        // If `sh` is on PATH, we can confirm which_claude works for it.
        let path = std::env::var_os("PATH").unwrap_or_default();
        let sh_exists = std::env::split_paths(&path).any(|d| d.join("sh").is_file());
        if sh_exists {
            // Not testing for claude specifically (may not exist in CI),
            // just that the function runs without panicking.
            let _ = which_claude();
        }
    }
}

/// Run the daemon until it is killed. Exits 0 straight away if another daemon
/// already holds the lock.
pub fn run() -> std::process::ExitCode {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "cannot locate the claudio binary");
            return std::process::ExitCode::FAILURE;
        }
    };
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            tracing::error!(error = %e, "failed to start async runtime");
            return std::process::ExitCode::FAILURE;
        }
    };
    runtime.block_on(async move {
        match server::start(config) {
            Ok(Some(listening)) => {
                listening.serve().await;
                std::process::ExitCode::SUCCESS
            }
            Ok(None) => {
                tracing::info!("another daemon is already running");
                std::process::ExitCode::SUCCESS
            }
            Err(e) => {
                tracing::error!(error = %e, "daemon failed to start");
                std::process::ExitCode::FAILURE
            }
        }
    })
}

/// State shared by every connection and session actor.
pub(crate) struct Daemon {
    config: Config,
    registry: Mutex<Registry>,
    /// Serializes journal disk writes; **never** held while `registry` is held.
    journal_write: Mutex<()>,
    /// Frames for every connected client (session events).
    events: broadcast::Sender<Frame>,
    /// Computed once, on first use (it runs `claude --version`).
    host: OnceCell<HostInfo>,
    /// Notified when the daemon should shut down cleanly.
    pub(crate) shutdown: tokio::sync::Notify,
    /// Latest host resource snapshot, updated every ~2 s by the stats sampler.
    pub(crate) stats_rx: tokio::sync::watch::Receiver<stats::HostSnapshot>,
}

/// Journaled sessions, and the live subset with a running process.
struct Registry {
    journal: Journal,
    live: HashMap<SessionId, Handle>,
    /// IDs currently being spawned (outside the lock) for idempotency.
    in_flight: HashSet<SessionId>,
    /// IDs that received a Kill while their spawn was still in-flight.
    /// When the spawn commits, it checks this set and kills the actor.
    to_kill: HashSet<SessionId>,
}

impl Daemon {
    fn new(config: Config) -> Daemon {
        let journal = Journal::load(&config.journal);
        tracing::info!(
            sessions = journal.entries().len(),
            "journal loaded; all sessions dormant"
        );
        let (events, _) = broadcast::channel(EVENTS_CAPACITY);
        let (stats_tx, stats_rx) = tokio::sync::watch::channel(stats::HostSnapshot::default());
        tokio::spawn(stats::sample_loop(stats_tx));
        Daemon {
            config,
            registry: Mutex::new(Registry {
                journal,
                live: HashMap::new(),
                in_flight: HashSet::new(),
                to_kill: HashSet::new(),
            }),
            journal_write: Mutex::new(()),
            events,
            host: OnceCell::new(),
            shutdown: tokio::sync::Notify::new(),
            stats_rx,
        }
    }

    /// The registry lock is only ever held for short, synchronous sections
    /// (never across an `.await`, never while doing I/O), so a poisoned lock
    /// still holds valid data.
    fn registry(&self) -> MutexGuard<'_, Registry> {
        self.registry.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Host facts for `Welcome`, probed once.
    async fn host(&self) -> HostInfo {
        self.host
            .get_or_init(|| async {
                let claude = self.config.claude.clone();
                tokio::task::spawn_blocking(move || host::probe(&claude))
                    .await
                    .unwrap_or_else(|_| host::basic())
            })
            .await
            .clone()
    }

    /// Send a session event to every connected client.
    fn broadcast(&self, id: SessionId, event: SessionEvent) {
        // No receivers just means no client is connected.
        let _ = self
            .events
            .send(Frame::Control(Envelope::event(Msg::Event { id, event })));
    }

    /// The command channel of a live session.
    fn session(&self, id: SessionId) -> Option<tokio::sync::mpsc::Sender<Cmd>> {
        self.registry().live.get(&id).map(|h| h.tx.clone())
    }

    /// Start `spec` unless it is already live or in-flight (idempotent by id).
    ///
    /// `requested` is what the caller asked for; the journal wins when it knows
    /// the id, so an old client re-spawning a dormant terminal with a plain
    /// `Spawn` still gets a shell. A shell never carries claude args or proxy
    /// env.
    ///
    /// ## Locking discipline
    ///
    /// The registry mutex is held **only** for:
    ///   1. Checking/reserving the id (in-flight mark).
    ///   2. Committing the handle after the PTY spawn succeeds.
    ///
    /// PTY open, process spawn, and thread creation happen in a
    /// `spawn_blocking` task, outside the lock. Journal disk writes also
    /// happen outside the lock, serialized by `journal_write`.
    async fn spawn(
        self: &Arc<Self>,
        mut spec: SpawnSpec,
        requested: SessionKind,
    ) -> io::Result<Option<u32>> {
        // Phase 1: idempotency check and reservation (fast, no I/O).
        let kind = {
            let mut reg = self.registry();
            if let Some(live) = reg.live.get(&spec.id) {
                return Ok(live.pid);
            }
            if reg.in_flight.contains(&spec.id) {
                // A concurrent spawn for this id is already in progress; the
                // client will receive a Created event when it completes.
                return Err(io::Error::other(format!(
                    "spawn in progress for {}",
                    spec.id
                )));
            }
            reg.in_flight.insert(spec.id);
            let journaled = reg.journal.entries().iter().find(|e| e.id == spec.id);
            journaled.map_or(requested, |e| e.kind)
        };
        // Registry lock released.
        if kind == SessionKind::Shell {
            spec.args.clear();
            spec.env.clear();
        }

        // Phase 2: PTY open + process spawn + thread creation (blocking,
        // outside the registry lock).
        let daemon = Arc::clone(self);
        let spec_clone = spec.clone();
        let spawn_result =
            tokio::task::spawn_blocking(move || session::spawn(&daemon, &spec_clone, kind))
                .await
                .unwrap_or_else(|e| Err(io::Error::other(format!("spawn task panicked: {e}"))));

        // Phase 3: commit or roll back under lock; snapshot journal entries
        // before releasing the lock (no disk I/O under the lock).
        // Also check whether this id was killed while in-flight.
        //
        // §Race fix: `committed_tx` is extracted here and sent AFTER the
        // `Created` broadcast below so the actor's on_exit (if it fires
        // immediately because the child already exited) always runs AFTER the
        // `Created` event is visible to clients — preserving protocol ordering.
        let (pid_result, journal_snapshot, broadcast_info, kill_on_commit, committed_tx) = {
            let mut reg = self.registry();
            reg.in_flight.remove(&spec.id);
            let was_killed = reg.to_kill.remove(&spec.id);
            match spawn_result {
                Ok((handle, committed_tx)) => {
                    let pid = handle.pid;
                    let entry = reg.journal.upsert_entry(&spec, kind);
                    reg.live.insert(spec.id, handle);
                    let snap = reg.journal.snapshot();
                    // A shell has no hooks to say otherwise: it is idle.
                    let state = match kind {
                        SessionKind::Claude => SessionState::Starting,
                        SessionKind::Shell => SessionState::Idle,
                    };
                    let info = SessionInfo {
                        state,
                        pid,
                        ..dormant_info(&entry)
                    };
                    (Ok(pid), Some(snap), Some(info), was_killed, Some(committed_tx))
                }
                Err(e) => (Err(e), None, None, false, None),
            }
        };
        // Registry lock released.

        // Phase 4: journal disk write outside the registry lock.
        if let Some(snap) = journal_snapshot {
            let _guard = self.journal_write.lock().unwrap_or_else(|e| e.into_inner());
            if let Err(e) = Journal::write_snapshot(&self.config.journal, &snap) {
                tracing::error!(error = %e, "could not write journal after spawn");
            }
        }

        // Broadcast Created after the lock is released.
        if let Some(info) = broadcast_info {
            tracing::info!(
                id = %spec.id, cwd = %spec.cwd, pid = ?info.pid,
                args = spec.args.len(), "session spawned"
            );
            self.broadcast(spec.id, SessionEvent::Created { info });
            // Branch for the tab's status line, for terminals as well as claude
            // (which only learns it on its first hook).
            let (daemon, id, cwd) = (Arc::clone(self), spec.id, host::expand_tilde(&spec.cwd));
            tokio::task::spawn_blocking(move || {
                if let Some(branch) = session::read_git_branch(&cwd) {
                    let meta = SessionEvent::Meta {
                        branch: Some(branch),
                        model: None,
                        context_tokens: None,
                    };
                    daemon.broadcast(id, meta);
                }
            });
        }

        // §Race fix: signal the actor that the handle is committed and Created
        // has been broadcast. The actor awaits this before running on_exit so
        // that `forget_live` and `retry_spawn_fresh` never run before Phase 3
        // inserts the handle, and `Exited` is never broadcast before `Created`.
        if let Some(tx) = committed_tx {
            let _ = tx.send(());
        }

        // If a Kill arrived while we were in-flight, execute it now.
        if kill_on_commit {
            tracing::info!(id = %spec.id, "executing deferred kill (killed while in-flight)");
            // Ignore errors (e.g. journal already clean).
            let _ = self.kill(spec.id).await;
        }

        pid_result
    }

    /// Kill a live session (or just forget a dormant one) and announce
    /// `Removed`. The journal removal is durable before returning `Ok`.
    ///
    /// If the id is in-flight (spawn not yet committed), the kill is deferred:
    /// it is marked in `to_kill` and executed when the spawn commits.
    ///
    /// If the journal write fails, the live handle and the in-memory journal
    /// entry are re-inserted so the daemon stays consistent and the kill is
    /// not acknowledged.
    async fn kill(&self, id: SessionId) -> io::Result<()> {
        // Take the live handle and save the journal entry, then remove both
        // from the in-memory registry, before doing any disk I/O.
        let (live, saved_entry, was_known, journal_snapshot) = {
            let mut reg = self.registry();
            // If the spawn is in-flight, defer the kill.
            if reg.in_flight.contains(&id) && reg.live.get(&id).is_none() {
                tracing::info!(%id, "kill deferred: spawn still in-flight");
                reg.to_kill.insert(id);
                // Announce Removed optimistically; the deferred kill will
                // finalise it. The client should not try to send input to
                // a session it just killed.
                drop(reg);
                self.broadcast(id, SessionEvent::Removed);
                return Ok(());
            }
            let live = reg.live.remove(&id);
            // Save a copy of the journal entry so we can re-insert on failure.
            let saved_entry = reg
                .journal
                .entries()
                .iter()
                .find(|e| e.id == id)
                .cloned();
            let was_journaled = reg.journal.remove_in_memory(id);
            let was_known = was_journaled || live.is_some();
            let snap = if was_known {
                Some(reg.journal.snapshot())
            } else {
                None
            };
            (live, saved_entry, was_known, snap)
        };

        if !was_known {
            return Err(io::Error::other(format!("no such session: {id}")));
        }

        // Write journal outside the lock; failure is a hard error for Kill.
        if let Some(snap) = journal_snapshot {
            let _guard = self.journal_write.lock().unwrap_or_else(|e| e.into_inner());
            if let Err(e) = Journal::write_snapshot(&self.config.journal, &snap) {
                // Re-insert the live handle and journal entry so the daemon
                // stays consistent with the on-disk state and does not lie
                // about success.
                tracing::error!(%id, error = %e, "kill journal write failed; not acking");
                let mut reg = self.registry();
                if let Some(handle) = live {
                    reg.live.insert(id, handle);
                }
                if let Some(entry) = saved_entry {
                    reg.journal.reinsert(entry);
                }
                return Err(io::Error::other(format!(
                    "could not durably remove session {id}: {e}"
                )));
            }
        }

        // Send kill command to the actor (if live).
        if let Some(handle) = live {
            let _ = handle.tx.send(Cmd::Kill).await;
        }

        tracing::info!(%id, "session removed");
        self.broadcast(id, SessionEvent::Removed);
        Ok(())
    }

    /// Update a session's human-readable name in the journal and broadcast the
    /// change so all connected clients (and any future `ListSessions` response)
    /// reflect it. Returns `false` when `id` is not in the journal.
    async fn rename(&self, id: SessionId, name: Option<String>) -> bool {
        let snap = {
            let mut reg = self.registry();
            if !reg.journal.rename_entry(id, name.clone()) {
                return false;
            }
            reg.journal.snapshot()
        };
        let _guard = self.journal_write.lock().unwrap_or_else(|e| e.into_inner());
        if let Err(e) = Journal::write_snapshot(&self.config.journal, &snap) {
            tracing::error!(%id, error = %e, "could not persist rename");
            // Non-fatal: in-memory is updated, client sees Ok; disk will be
            // repaired on next unrelated snapshot write.
        }
        self.broadcast(id, SessionEvent::Renamed { name });
        true
    }

    /// Every journaled session; live ones report their pid, state and title.
    async fn sessions(&self) -> Vec<SessionInfo> {
        let rows: Vec<(Entry, Option<(tokio::sync::mpsc::Sender<Cmd>, Option<u32>)>)> = {
            let reg = self.registry();
            reg.journal
                .entries()
                .iter()
                .map(|e| {
                    (
                        e.clone(),
                        reg.live.get(&e.id).map(|h| (h.tx.clone(), h.pid)),
                    )
                })
                .collect()
        };
        let mut out = Vec::with_capacity(rows.len());
        for (entry, live) in rows {
            let mut info = dormant_info(&entry);
            if let Some((tx, pid)) = live {
                if let Some(status) = session::status(&tx, STATUS_TIMEOUT).await {
                    info.state = status.state;
                    info.title = status.title;
                    info.pid = pid;
                }
            }
            out.push(info);
        }
        out
    }

    /// Route a hook to the session whose *current* spawn token matches.
    /// Unknown tokens (stale generations, forgeries) are dropped silently.
    async fn hook(&self, token: &str, event: String, payload: serde_json::Value) {
        let target = self
            .registry()
            .live
            .iter()
            .find(|(_, h)| h.token == token)
            .map(|(id, h)| (*id, h.tx.clone()));
        match target {
            Some((id, tx)) => {
                tracing::debug!(%id, %event, "hook");
                let _ = tx.send(Cmd::Hook { event, payload }).await;
            }
            None => tracing::debug!(%event, "hook with unknown token dropped"),
        }
    }

    /// Journal a session's new claude conversation id (durably, right away).
    ///
    /// Follows the split-lock pattern: mutate in-memory under the registry
    /// lock, snapshot, release the lock, then write to disk under the
    /// journal_write mutex. This avoids holding the registry lock during
    /// fsync'd I/O.
    fn record_claude_session(&self, id: SessionId, claude_session_id: &str) {
        // Phase 1: mutate in-memory and snapshot under the registry lock.
        let snapshot = {
            let mut reg = self.registry();
            let updated = reg.journal.update_claude_session(id, claude_session_id);
            if updated {
                Some(reg.journal.snapshot())
            } else {
                None
            }
        };
        // Phase 2: write to disk outside the registry lock.
        if let Some(snap) = snapshot {
            let _guard = self.journal_write.lock().unwrap_or_else(|e| e.into_inner());
            if let Err(e) = Journal::write_snapshot(&self.config.journal, &snap) {
                tracing::error!(path = %self.config.journal.display(), error = %e,
                    "could not write journal after claude session id update");
            }
        }
    }

    /// Retry spawning a session without `--resume` after a quick-exit failure.
    ///
    /// Called by the session actor (§2.3). Looks up the journal entry, strips
    /// `--resume`/`--session-id` args, and re-spawns with a fresh conversation.
    pub(crate) async fn retry_spawn_fresh(self: &Arc<Self>, id: SessionId) {
        // Build a stripped spec from the journal entry.
        let spec_opt = {
            let reg = self.registry();
            reg.journal.entries().iter().find(|e| e.id == id).map(|e| {
                let mut args: Vec<String> = Vec::new();
                let mut skip_next = false;
                for a in &e.args {
                    if skip_next {
                        skip_next = false;
                        continue;
                    }
                    if a == "--resume" || a == "--session-id" {
                        skip_next = true;
                        continue;
                    }
                    args.push(a.clone());
                }
                let spec = crate::proto::SpawnSpec {
                    id,
                    cwd: e.cwd.clone(),
                    name: e.name.clone(),
                    args,
                    env: vec![], // env is not stored in journal (by design)
                    rows: 24,
                    cols: 80,
                };
                (spec, e.kind)
            })
        };
        let Some((spec, kind)) = spec_opt else {
            tracing::warn!(%id, "retry_spawn_fresh: no journal entry found");
            self.forget_live(id, ""); // ensure it's dormant
            self.broadcast(
                id,
                crate::proto::SessionEvent::State {
                    state: crate::proto::SessionState::Exited,
                },
            );
            self.broadcast(id, crate::proto::SessionEvent::Exited { code: None });
            return;
        };
        tracing::info!(%id, "retrying spawn without --resume");
        match self.spawn(spec, kind).await {
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(%id, error = %e, "retry spawn failed; session stays dormant");
                self.forget_live(id, "");
                self.broadcast(
                    id,
                    crate::proto::SessionEvent::State {
                        state: crate::proto::SessionState::Exited,
                    },
                );
                self.broadcast(id, crate::proto::SessionEvent::Exited { code: Some(1) });
            }
        }
    }

    /// Drop a session from the live set when its process exits — unless it
    /// was already replaced by a newer spawn (different token). Returns
    /// whether it was dropped.
    fn forget_live(&self, id: SessionId, token: &str) -> bool {
        let mut reg = self.registry();
        let ours = reg.live.get(&id).is_some_and(|h| h.token == token);
        if ours {
            reg.live.remove(&id);
        }
        ours
    }

}

/// A journal entry as seen while it has no process.
fn dormant_info(entry: &Entry) -> SessionInfo {
    SessionInfo {
        id: entry.id,
        cwd: entry.cwd.clone(),
        name: entry.name.clone(),
        state: SessionState::Exited,
        claude_session_id: entry.claude_session_id.clone(),
        title: None,
        pid: None,
        created_at: entry.created_at,
        branch: None,
        model: None,
        context_tokens: None,
        kind: entry.kind,
    }
}
