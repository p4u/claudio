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
//! - [`update`] — `claude update` (or the installer), on request.
//! - [`git`] — the read-only git queries behind the TUI's git viewer.
//! - [`login_path`] — the user's login-shell `PATH`, for claude and sessions.
//! - [`manifest`] — proxy plugin manifests: what a plugin's sessions get
//!   here, and the plugin installs it needs.
//! - [`service`] — the addons' long-running services, supervised.
//!
//! This module holds the shared [`Daemon`] state: the registry of journaled
//! and live sessions, and the all-clients event broadcast.

mod addon_ops;
mod admit;
mod agent;
mod command;
pub mod ctl;
mod git;
mod git_status;
mod host;
mod journal;
mod lifecycle;
mod login_path;
mod server;
mod service;
mod session;
mod stats;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
#[cfg(test)]
mod tests;
mod update;

use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use tokio::sync::broadcast;
use tracing_subscriber::EnvFilter;

use crate::paths;
use crate::proto::{
    Envelope, Frame, HostInfo, Msg, SessionEvent, SessionId, SessionInfo, SessionKind,
    SessionState, SpawnSpec,
};
use crate::remote::probe::Probe;
use journal::{Entry, Journal};
pub use service::ServiceTiming;
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
    /// The SSH agent relay beside it ([`agent`]): its inherited fallback.
    pub agent: agent::AgentRelay,
    /// The startup lock file; held for the daemon's lifetime.
    pub lock: PathBuf,
    /// The session journal.
    pub journal: PathBuf,
    /// The claude binary: `$CLAUDIO_CLAUDE_PATH` as given, else the bare
    /// name `claude`, looked up afresh on each use ([`Config::claude_bin`]).
    pub claude: PathBuf,
    /// This binary, used as the hook relay command.
    pub claudio: PathBuf,
    /// `[claude] allow_skip_permissions` from this host's `config.toml`:
    /// launch claude with `--allow-dangerously-skip-permissions`.
    pub skip_permissions: bool,
    /// `[claude] latex` from this host's `config.toml`: every claude session
    /// gets the `__context latex` SessionStart hook.
    pub latex: bool,
    /// The binary this daemon runs, as of its start (reported in `Welcome`
    /// so clients can tell an outdated daemon; see [`crate::freshness`]).
    /// `None` when it could not be read.
    pub build: Option<Probe>,
    /// The `PATH` claude is looked up in and every session runs with: the
    /// user's login-shell `PATH` merged with the inherited one
    /// ([`login_path`]). `None` keeps the daemon's own `PATH`.
    pub path: Option<OsString>,
    /// How long one addon op may run ([`addon_ops::TIMEOUT`]).
    pub addon_timeout: Duration,
    /// The budget of one usage-limit attempt, every addon's answer included (20 s).
    pub limit_attempt_timeout: Duration,
    /// Play the previous release (mixed-version tests only; docs/daemon-protocol.md,
    /// "Mixed versions"): no `addon:` features, `AddonOp` refused as unknown,
    /// every `env_remove` taken as absent and `optional` ignored.
    pub old_peer: bool,
    /// The `config.toml` the addons' services are read from, at start and at
    /// every reload ([`service`]); `None`: no addon has a service.
    pub config_file: Option<PathBuf>,
    /// How the addons' services are started, stopped and restarted.
    pub service_timing: ServiceTiming,
}

impl Config {
    /// The standard locations from [`paths`]. Asks the login shell for its
    /// `PATH` (≤ [`login_path::TIMEOUT`]).
    pub fn from_env() -> io::Result<Config> {
        let inherited = std::env::var_os("PATH");
        let path = login_path::resolve(
            &login_path::login_shell(),
            inherited.as_deref(),
            login_path::TIMEOUT,
        );
        let claude = crate::config::load().claude;
        Ok(Config {
            socket: paths::daemon_socket(),
            agent: agent::AgentRelay::inherited(std::env::var_os("SSH_AUTH_SOCK")),
            lock: paths::daemon_lock(),
            journal: paths::daemon_journal(),
            claude: claude_setting(std::env::var_os("CLAUDIO_CLAUDE_PATH")),
            claudio: std::env::current_exe()?,
            skip_permissions: claude.allow_skip_permissions,
            latex: claude.latex,
            // Hashed now: an upgrade may replace the file while we run.
            build: Probe::current()
                .inspect_err(|e| tracing::warn!(error = %e, "cannot identify this binary"))
                .ok(),
            path,
            addon_timeout: addon_ops::TIMEOUT,
            limit_attempt_timeout: Duration::from_secs(20),
            old_peer: admit::old_peer(std::env::var_os("CLAUDIO_TEST_OLD_DAEMON").as_deref()),
            config_file: Some(paths::config_file()),
            service_timing: ServiceTiming::default(),
        })
    }

    /// The claude binary to run now. A bare name is resolved again each time
    /// ([`resolve_claude_path`]): an update or the installer may have put a
    /// claude somewhere new since startup.
    pub fn claude_bin(&self) -> PathBuf {
        if self.claude.components().count() > 1 {
            return self.claude.clone();
        }
        let path = self.path.clone().or_else(|| std::env::var_os("PATH"));
        resolve_claude_path(self.claude.as_os_str(), path.as_deref(), &host::home())
    }
}

/// `$CLAUDIO_CLAUDE_PATH` when set (it always wins), else the bare `claude`.
fn claude_setting(env: Option<OsString>) -> PathBuf {
    env.filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("claude"))
}

/// Find the bare command `name` (normally `claude`): in `path` (the login
/// shell's, see [`Config::path`]), then in well-known install locations —
/// the user's own before system-wide ones — in case the login shell could not
/// be read. Falls back to `name` itself, for a lookup at spawn time.
pub fn resolve_claude_path(name: &OsStr, path: Option<&OsStr>, home: &Path) -> PathBuf {
    let in_path = path.into_iter().flat_map(std::env::split_paths);
    let fallbacks = [
        home.join(".local/bin"),
        home.join(".claude/local"),
        home.join(".npm-packages/bin"),
        home.join(".npm-global/bin"),
        home.join(".bun/bin"),
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/bin"),
    ];
    in_path
        .chain(fallbacks)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from(name))
}

/// Run the daemon until it is killed. Exits 0 straight away if another daemon
/// already holds the lock.
pub fn run(addons: Arc<crate::addon::Addons>) -> std::process::ExitCode {
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
    let (code, lock) = runtime.block_on(async move {
        match server::start(config, addons) {
            Ok(Some(listening)) => (
                std::process::ExitCode::SUCCESS,
                Some(serve_until_shutdown(listening).await),
            ),
            Ok(None) => {
                tracing::info!("another daemon is already running");
                (std::process::ExitCode::SUCCESS, None)
            }
            Err(e) => {
                tracing::error!(error = %e, "daemon failed to start");
                (std::process::ExitCode::FAILURE, None)
            }
        }
    });
    // Asked to shut down: stop every task, then exit, which closes the
    // sessions' terminals. The lock goes last, as a successor waits for it
    // and must not run beside us. Bounded: a stuck blocking task (a slow
    // `claude --version`) must not hold a restart up.
    runtime.shutdown_timeout(SHUTDOWN_GRACE);
    drop(lock);
    code
}

/// How long a shutting-down daemon waits for its blocking tasks.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(1);

/// Start the addons' services, serve until asked to shut down, then stop
/// every service (in parallel) before handing back the startup lock: a
/// successor's services never run beside ours.
async fn serve_until_shutdown(listening: server::Listening) -> std::fs::File {
    let daemon = listening.daemon();
    service::Services::start_all(&daemon);
    let relay = agent::start(&daemon);
    let lock = listening.serve().await;
    drop(relay);
    let grace = daemon.config.service_timing.stop;
    daemon.services.stop_all(grace).await;
    lock
}

/// State shared by every connection and session actor.
pub struct Daemon {
    config: Config,
    registry: Mutex<Registry>,
    /// Serializes journal disk writes; **never** held while `registry` is held.
    journal_write: Mutex<()>,
    /// Frames for every connected client (session events).
    events: broadcast::Sender<Frame>,
    /// Probed on first use (it runs `claude --version`), and again after a
    /// claude update.
    host: tokio::sync::Mutex<Option<HostInfo>>,
    /// Held while `claude update` or one `claude plugin …` runs: one at a
    /// time, so `~/.claude` has one writer.
    pub updating: Arc<tokio::sync::Mutex<()>>,
    /// Which optional flags this claude knows (`--allow-dangerously-skip-
    /// permissions`, `--plugin-dir`, `--agents`, `--mcp-config`; it runs
    /// `claude --help`). Probed on first use, and again after a claude
    /// update or install, like `host`.
    claude_flags: Mutex<Option<crate::addon::ClaudeFlags>>,
    /// Notified when the daemon should shut down cleanly.
    pub(crate) shutdown: tokio::sync::Notify,
    /// Latest host resource snapshot, updated every ~2 s by the stats sampler.
    pub(crate) stats_rx: tokio::sync::watch::Receiver<stats::HostSnapshot>,
    /// `Search`: one at a time, and the transcripts it reads.
    pub(crate) search: server::SearchSlot,
    /// The addons this binary runs, and each daemon-side one's op slots.
    addons: Arc<crate::addon::Addons>,
    addon_slots: HashMap<&'static str, Arc<tokio::sync::Semaphore>>,
    /// The addons' services ([`service`]).
    pub(crate) services: service::Services,
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
    /// Each journaled session's last spawn env, for a retry; dropped with
    /// its journal entry.
    session_env: HashMap<SessionId, admit::SessionEnv>,
}

impl Daemon {
    fn new(config: Config, addons: Arc<crate::addon::Addons>) -> Daemon {
        let journal = Journal::load(&config.journal);
        tracing::info!(
            sessions = journal.entries().len(),
            "journal loaded; all sessions dormant"
        );
        // Context files of a previous daemon's sessions: none is ours yet.
        session::clear_contexts(&config.socket);
        let (events, _) = broadcast::channel(EVENTS_CAPACITY);
        let (stats_tx, stats_rx) = tokio::sync::watch::channel(stats::HostSnapshot::default());
        tokio::spawn(stats::sample_loop(stats_tx));
        let services = service::Services::new(&addons, config.service_timing);
        Daemon {
            config,
            registry: Mutex::new(Registry {
                journal,
                live: HashMap::new(),
                in_flight: HashSet::new(),
                to_kill: HashSet::new(),
                session_env: HashMap::new(),
            }),
            journal_write: Mutex::new(()),
            events,
            host: tokio::sync::Mutex::new(None),
            updating: Arc::default(),
            claude_flags: Mutex::new(None),
            shutdown: tokio::sync::Notify::new(),
            stats_rx,
            search: server::SearchSlot::new(),
            addon_slots: addon_ops::slots(&addons),
            services,
            addons,
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
        let mut slot = self.host.lock().await;
        match &*slot {
            Some(host) => host.clone(),
            None => slot.insert(self.probe_host().await).clone(),
        }
    }

    /// Probe the host again (claude just changed on disk) and remember it.
    /// The flag check is redone on the next spawn: a claude that was missing
    /// or old before may know the flag now.
    async fn refresh_host(&self) -> HostInfo {
        *self.claude_flags.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let host = self.probe_host().await;
        *self.host.lock().await = Some(host.clone());
        host
    }

    async fn probe_host(&self) -> HostInfo {
        let claude = self.config.claude_bin();
        let path = self.config.path.clone();
        let host = tokio::task::spawn_blocking(move || host::probe(&claude, path.as_deref()))
            .await
            .unwrap_or_else(|_| host::basic());
        match &host.claude {
            Some(c) => tracing::info!(path = %c.path, version = %c.version, "claude"),
            None => tracing::warn!(
                looked_for = %self.config.claude_bin().display(),
                "claude not found (or `claude --version` failed)"
            ),
        }
        host
    }

    /// Whether to launch claude with `--allow-dangerously-skip-permissions`:
    /// enabled in the config and supported by this claude. Blocking the first
    /// time (it runs `claude --help`); call it off the async runtime.
    pub(crate) fn skip_permissions(&self) -> bool {
        self.config.skip_permissions && self.claude_flags().skip_permissions
    }

    /// The optional flags this claude knows: probed once (`claude --help`),
    /// until the next claude update. Blocking the first time.
    pub(crate) fn claude_flags(&self) -> crate::addon::ClaudeFlags {
        // Held while probing, so concurrent spawns wait for one probe.
        let mut flags = self.claude_flags.lock().unwrap_or_else(|e| e.into_inner());
        if flags.is_none() {
            *flags = host::claude_flags(&self.config.claude_bin(), self.config.path.as_deref());
        }
        flags.clone().unwrap_or_default()
    }

    /// Send a session event to every connected client.
    fn broadcast(&self, id: SessionId, event: SessionEvent) {
        // No receivers just means no client is connected.
        let _ = self
            .events
            .send(Frame::Control(Envelope::event(Msg::Event { id, event })));
    }

    /// Where session `id`'s conversation is: its directory, and its claude
    /// session id (`None` until claude reports one, and for a terminal).
    /// `None` for a session the daemon does not know.
    pub(crate) fn conversation(&self, id: SessionId) -> Option<(String, Option<String>)> {
        let reg = self.registry();
        let entry = reg.journal.entries().iter().find(|e| e.id == id)?;
        let claude = entry.kind == SessionKind::Claude;
        let sid = entry.claude_session_id.clone().filter(|_| claude);
        Some((entry.cwd.clone(), sid))
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
        spec: SpawnSpec,
        requested: SessionKind,
    ) -> io::Result<Option<u32>> {
        self.check_spawn(&spec)?;
        // Phase 1: idempotency check and reservation (fast, no I/O).
        let kind = {
            let mut reg = self.registry();
            if let Some(live) = reg.live.get(&spec.id) {
                return Ok(live.pid);
            }
            if reg.in_flight.contains(&spec.id) {
                // A concurrent spawn for this id is already in progress; the
                // client will receive a Created event when it completes.
                return Err(in_progress(spec.id));
            }
            reg.in_flight.insert(spec.id);
            let journaled = reg.journal.entries().iter().find(|e| e.id == spec.id);
            journaled.map_or(requested, |e| e.kind)
        };
        // Registry lock released.
        self.spawn_reserved(spec, kind).await
    }

    /// Phases 2–4 of [`Daemon::spawn`], for an id the caller has already
    /// reserved in `in_flight`. Releases the reservation, and carries out a
    /// Kill that arrived meanwhile.
    async fn spawn_reserved(
        self: &Arc<Self>,
        mut spec: SpawnSpec,
        kind: SessionKind,
    ) -> io::Result<Option<u32>> {
        self.prepare(&mut spec, kind);

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
                    let entry = reg.commit_entry(&spec, kind);
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
                    (
                        Ok(pid),
                        Some(snap),
                        Some(info),
                        was_killed,
                        Some(committed_tx),
                    )
                }
                // A respawn's id is journaled already: a Kill that came
                // meanwhile still has to remove it, spawned or not.
                Err(e) => (Err(e), None, None, was_killed, None),
            }
        };
        // Registry lock released.

        // Phase 4: journal disk write outside the registry lock.
        if let Some(snap) = journal_snapshot {
            if let Err(e) = self.write_journal(&snap) {
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
            // Git status for the tab's status line, for terminals as well as
            // claude (which only learns it on its first hook).
            let (daemon, id, cwd) = (Arc::clone(self), spec.id, host::expand_tilde(&spec.cwd));
            tokio::spawn(async move {
                daemon.broadcast(id, session::git_meta(&cwd).await);
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
    /// If the id is in-flight (a spawn or respawn not yet committed), the kill
    /// is deferred: it is marked in `to_kill` and executed when that ends.
    ///
    /// If the journal write fails, the live handle and the in-memory journal
    /// entry are re-inserted so the daemon stays consistent and the kill is
    /// not acknowledged.
    async fn kill(&self, id: SessionId) -> io::Result<()> {
        // Take the live handle and save the journal entry, then remove both
        // from the in-memory registry, before doing any disk I/O.
        let (live, (saved_entry, saved_env), was_known, journal_snapshot) = {
            let mut reg = self.registry();
            // If the spawn is in-flight, defer the kill.
            if reg.in_flight.contains(&id) && !reg.live.contains_key(&id) {
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
            let saved_entry = reg.journal.entries().iter().find(|e| e.id == id).cloned();
            let saved_env = reg.session_env.remove(&id);
            let was_journaled = reg.journal.remove_in_memory(id);
            let was_known = was_journaled || live.is_some();
            let snap = if was_known {
                Some(reg.journal.snapshot())
            } else {
                None
            };
            (live, (saved_entry, saved_env), was_known, snap)
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
                if let Some(env) = saved_env {
                    reg.session_env.insert(id, env);
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

        session::remove_context(&self.config.socket, id);
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
        // A journal entry and, when live, its command channel and pid.
        type Row = (Entry, Option<(tokio::sync::mpsc::Sender<Cmd>, Option<u32>)>);
        let rows: Vec<Row> = {
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
                    info.attention = status.attention;
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

    /// Persist a journal snapshot taken under the registry lock. Called
    /// without that lock held; `journal_write` orders the writes.
    fn write_journal(&self, snap: &[Entry]) -> io::Result<()> {
        let _guard = self.journal_write.lock().unwrap_or_else(|e| e.into_inner());
        Journal::write_snapshot(&self.config.journal, snap)
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

/// The refusal for a spawn or respawn of an id that is mid-(re)spawn; the
/// client hears `Created` when that one completes.
fn in_progress(id: SessionId) -> io::Error {
    io::Error::other(format!("spawn in progress for {id}"))
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
        ephemeral: entry.ephemeral,
        attention: None,
    }
}

#[cfg(test)]
mod claude_path_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("claudio-resolve-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// An executable `claude` in `dir`.
    fn install(dir: &Path) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let bin = dir.join("claude");
        std::fs::write(&bin, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        bin
    }

    fn join(dirs: &[&Path]) -> OsString {
        std::env::join_paths(dirs).unwrap()
    }

    const ABSENT: &str = "claude-not-installed-anywhere";

    #[test]
    fn the_login_path_comes_first() {
        let root = scratch();
        let home = root.join("home");
        let mine = install(&home.join(".npm-packages/bin"));
        let stale = install(&root.join("usr/local/bin"));
        let (mine_dir, stale_dir) = (mine.parent().unwrap(), stale.parent().unwrap());
        let path = join(&[mine_dir, stale_dir]);
        assert_eq!(
            resolve_claude_path(OsStr::new("claude"), Some(&path), &home),
            mine
        );
        // Whichever comes first in the PATH wins.
        let path = join(&[stale_dir, mine_dir]);
        assert_eq!(
            resolve_claude_path(OsStr::new("claude"), Some(&path), &home),
            stale
        );
    }

    #[test]
    fn well_known_locations_when_the_path_lacks_claude() {
        let root = scratch();
        let home = root.join("home");
        let empty = root.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        let npm = install(&home.join(".npm-packages/bin"));
        assert_eq!(
            resolve_claude_path(OsStr::new("claude"), Some(&join(&[&empty])), &home),
            npm
        );
        // The native installer's location ranks above npm's.
        let native = install(&home.join(".local/bin"));
        assert_eq!(
            resolve_claude_path(OsStr::new("claude"), None, &home),
            native
        );
    }

    #[test]
    fn missing_everywhere_is_the_bare_name() {
        let root = scratch();
        let got = resolve_claude_path(OsStr::new(ABSENT), Some(&join(&[&root])), &root);
        assert_eq!(got, PathBuf::from(ABSENT));
    }

    #[test]
    fn relative_path_entries_are_skipped() {
        let root = scratch();
        let path = OsString::from(".:bin");
        let got = resolve_claude_path(OsStr::new(ABSENT), Some(&path), &root);
        assert_eq!(got, PathBuf::from(ABSENT));
    }

    fn config(claude: PathBuf, path: Option<OsString>) -> Config {
        Config {
            socket: "/run/d.sock".into(),
            agent: Default::default(),
            lock: "/run/d.lock".into(),
            journal: "/j.json".into(),
            claude,
            claudio: "/bin/claudio".into(),
            skip_permissions: false,
            latex: false,
            build: None,
            path,
            addon_timeout: std::time::Duration::from_secs(150),
            limit_attempt_timeout: std::time::Duration::from_secs(20),
            old_peer: false,
            config_file: None,
            service_timing: Default::default(),
        }
    }

    #[test]
    fn claudio_claude_path_wins() {
        let root = scratch();
        let mine = install(&root.join("login"));
        let pinned = install(&root.join("pinned"));
        let path = Some(join(&[mine.parent().unwrap()]));
        let setting = claude_setting(Some(pinned.clone().into()));
        assert_eq!(config(setting, path.clone()).claude_bin(), pinned);
        // Unset or empty: the bare name, looked up in the login PATH.
        assert_eq!(claude_setting(None), PathBuf::from("claude"));
        assert_eq!(claude_setting(Some("".into())), PathBuf::from("claude"));
        assert_eq!(config(claude_setting(None), path).claude_bin(), mine);
    }

    #[test]
    fn claude_bin_sees_a_claude_installed_later() {
        let root = scratch();
        let dir = root.join("login");
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = config(PathBuf::from(ABSENT), Some(join(&[&dir])));
        assert_eq!(cfg.claude_bin(), PathBuf::from(ABSENT));
        std::fs::write(dir.join(ABSENT), "").unwrap();
        assert_eq!(cfg.claude_bin(), dir.join(ABSENT));
    }
}
