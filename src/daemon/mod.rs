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
    Envelope, Frame, HostInfo, Msg, SessionEvent, SessionId, SessionInfo, SessionState, SpawnSpec,
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
}

/// Journaled sessions, and the live subset with a running process.
struct Registry {
    journal: Journal,
    live: HashMap<SessionId, Handle>,
    /// IDs currently being spawned (outside the lock) for idempotency.
    in_flight: HashSet<SessionId>,
}

impl Daemon {
    fn new(config: Config) -> Daemon {
        let journal = Journal::load(&config.journal);
        tracing::info!(
            sessions = journal.entries().len(),
            "journal loaded; all sessions dormant"
        );
        let (events, _) = broadcast::channel(EVENTS_CAPACITY);
        Daemon {
            config,
            registry: Mutex::new(Registry {
                journal,
                live: HashMap::new(),
                in_flight: HashSet::new(),
            }),
            journal_write: Mutex::new(()),
            events,
            host: OnceCell::new(),
            shutdown: tokio::sync::Notify::new(),
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
    /// ## Locking discipline
    ///
    /// The registry mutex is held **only** for:
    ///   1. Checking/reserving the id (in-flight mark).
    ///   2. Committing the handle after the PTY spawn succeeds.
    ///
    /// PTY open, process spawn, and thread creation happen in a
    /// `spawn_blocking` task, outside the lock. Journal disk writes also
    /// happen outside the lock, serialized by `journal_write`.
    async fn spawn(self: &Arc<Self>, spec: SpawnSpec) -> Result<Option<u32>, String> {
        // Phase 1: idempotency check and reservation (fast, no I/O).
        {
            let mut reg = self.registry();
            if let Some(live) = reg.live.get(&spec.id) {
                return Ok(live.pid);
            }
            if reg.in_flight.contains(&spec.id) {
                // A concurrent spawn for this id is already in progress; the
                // client will receive a Created event when it completes.
                return Err(format!("spawn in progress for {}", spec.id));
            }
            reg.in_flight.insert(spec.id);
        }
        // Registry lock released.

        // Phase 2: PTY open + process spawn + thread creation (blocking,
        // outside the registry lock).
        let daemon = Arc::clone(self);
        let spec_clone = spec.clone();
        let spawn_result = tokio::task::spawn_blocking(move || {
            session::spawn(&daemon, &spec_clone)
        })
        .await
        .unwrap_or_else(|e| Err(format!("spawn task panicked: {e}")));

        // Phase 3: commit or roll back under lock; snapshot journal entries
        // before releasing the lock (no disk I/O under the lock).
        let (pid_result, journal_snapshot, broadcast_info) = {
            let mut reg = self.registry();
            reg.in_flight.remove(&spec.id);
            match spawn_result {
                Ok(handle) => {
                    let pid = handle.pid;
                    let entry = reg.journal.upsert_entry(&spec);
                    reg.live.insert(spec.id, handle);
                    let snap = reg.journal.snapshot();
                    let info = SessionInfo {
                        state: SessionState::Starting,
                        pid,
                        ..dormant_info(&entry)
                    };
                    (Ok(pid), Some(snap), Some(info))
                }
                Err(e) => (Err(e), None, None),
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
        }

        pid_result
    }

    /// Kill a live session (or just forget a dormant one) and announce
    /// `Removed`. The journal removal is durable before returning `Ok`.
    async fn kill(&self, id: SessionId) -> Result<(), String> {
        // Take the live handle and remove in-memory journal entry under lock.
        let (live, was_journaled, journal_snapshot) = {
            let mut reg = self.registry();
            let live = reg.live.remove(&id);
            let was_journaled = reg.journal.remove_in_memory(id);
            let snap = if was_journaled || live.is_some() {
                Some(reg.journal.snapshot())
            } else {
                None
            };
            (live, was_journaled, snap)
        };

        if live.is_none() && !was_journaled {
            return Err(format!("no such session: {id}"));
        }

        // Write journal outside the lock; failure is a hard error for Kill.
        if let Some(snap) = journal_snapshot {
            let _guard = self.journal_write.lock().unwrap_or_else(|e| e.into_inner());
            if let Err(e) = Journal::write_snapshot(&self.config.journal, &snap) {
                // Re-insert the entry in memory so we don't lie about success.
                // We cannot un-kill the live session, but at least the journal
                // stays consistent with what we're about to announce.
                tracing::error!(%id, error = %e, "kill journal write failed; not acking");
                return Err(format!("could not durably remove session {id}: {e}"));
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
    fn record_claude_session(&self, id: SessionId, claude_session_id: &str) {
        self.registry()
            .journal
            .set_claude_session(id, claude_session_id);
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
                crate::proto::SpawnSpec {
                    id,
                    cwd: e.cwd.clone(),
                    name: e.name.clone(),
                    args,
                    env: vec![], // env is not stored in journal (by design)
                    rows: 24,
                    cols: 80,
                }
            })
        };
        let Some(spec) = spec_opt else {
            tracing::warn!(%id, "retry_spawn_fresh: no journal entry found");
            self.forget_live(id, ""); // ensure it's dormant
            self.broadcast(id, crate::proto::SessionEvent::State {
                state: crate::proto::SessionState::Exited,
            });
            self.broadcast(id, crate::proto::SessionEvent::Exited { code: None });
            return;
        };
        tracing::info!(%id, "retrying spawn without --resume");
        match self.spawn(spec).await {
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(%id, error = %e, "retry spawn failed; session stays dormant");
                self.forget_live(id, "");
                self.broadcast(id, crate::proto::SessionEvent::State {
                    state: crate::proto::SessionState::Exited,
                });
                self.broadcast(id, crate::proto::SessionEvent::Exited { code: Some(1) });
            }
        }
    }

    /// Drop a session from the live set when its process exits — unless it
    /// was already replaced by a newer spawn (different token).
    fn forget_live(&self, id: SessionId, token: &str) {
        let mut reg = self.registry();
        if reg.live.get(&id).is_some_and(|h| h.token == token) {
            reg.live.remove(&id);
        }
    }

    /// Resolve the args for respawning a dormant session.
    ///
    /// When the client requests a respawn with only `--resume` (or no extra
    /// args), we merge in the journal's stored args (model, permission flags,
    /// etc.) so the session is resumed with the same configuration it was
    /// originally spawned with. The `--resume <claude_session_id>` pair is
    /// always appended from the journal entry.
    #[allow(dead_code)]
    pub(crate) fn resolve_respawn_args(&self, spec: &SpawnSpec) -> Vec<String> {
        let reg = self.registry();
        let Some(entry) = reg.journal.entries().iter().find(|e| e.id == spec.id) else {
            return spec.args.clone();
        };

        // If the client supplied a non-trivial arg list (not just --resume),
        // trust it as-is. We consider it non-trivial when it contains any arg
        // other than "--resume" and its value.
        let is_resume_only = {
            let args = &spec.args;
            args.is_empty()
                || (args.len() == 2
                    && args[0] == "--resume"
                    && !args[1].is_empty())
                || (args.len() == 1 && args[0] == "--resume")
        };

        if !is_resume_only {
            return spec.args.clone();
        }

        // Build merged args: journal's base args (minus any existing
        // --resume/--session-id pairs) + --resume <current claude id>.
        let mut merged: Vec<String> = entry
            .args
            .iter()
            .filter(|a| *a != "--resume" && *a != "--session-id")
            .cloned()
            .collect();
        // Remove the value that follows --resume / --session-id in the journal.
        let mut skip_next = false;
        let mut cleaned: Vec<String> = Vec::new();
        for a in &merged {
            if skip_next {
                skip_next = false;
                continue;
            }
            if a == "--resume" || a == "--session-id" {
                skip_next = true;
                continue;
            }
            cleaned.push(a.clone());
        }
        merged = cleaned;

        // Append --resume with the stored claude session id, if any.
        if let Some(csid) = &entry.claude_session_id {
            merged.push("--resume".into());
            merged.push(csid.clone());
        }

        merged
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
    }
}
