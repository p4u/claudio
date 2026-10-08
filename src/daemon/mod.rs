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

mod host;
mod journal;
mod server;
mod session;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
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
            .unwrap_or_else(|| PathBuf::from("claude"));
        Ok(Config {
            socket: paths::daemon_socket(),
            lock: paths::daemon_lock(),
            journal: paths::daemon_journal(),
            claude,
            claudio: std::env::current_exe()?,
        })
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
    /// Frames for every connected client (session events).
    events: broadcast::Sender<Frame>,
    /// Computed once, on first use (it runs `claude --version`).
    host: OnceCell<HostInfo>,
}

/// Journaled sessions, and the live subset with a running process.
struct Registry {
    journal: Journal,
    live: HashMap<SessionId, Handle>,
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
            }),
            events,
            host: OnceCell::new(),
        }
    }

    /// The registry lock is only ever held for short, synchronous sections
    /// (never across an `.await`), so a poisoned lock still holds valid data.
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

    /// Start `spec` unless it is already live (idempotent by id). Returns the
    /// pid; a fresh spawn is journaled and announced with `Created`.
    fn spawn(self: &Arc<Self>, spec: SpawnSpec) -> Result<Option<u32>, String> {
        let mut reg = self.registry();
        if let Some(live) = reg.live.get(&spec.id) {
            return Ok(live.pid);
        }
        let handle = session::spawn(self, &spec)?;
        let pid = handle.pid;
        reg.live.insert(spec.id, handle);
        let entry = reg.journal.record_spawn(&spec);
        drop(reg);

        tracing::info!(id = %spec.id, cwd = %spec.cwd, ?pid, args = spec.args.len(), "session spawned");
        let info = SessionInfo {
            state: SessionState::Starting,
            pid,
            ..dormant_info(&entry)
        };
        self.broadcast(spec.id, SessionEvent::Created { info });
        Ok(pid)
    }

    /// Kill a live session (or just forget a dormant one) and announce
    /// `Removed`.
    async fn kill(&self, id: SessionId) -> Result<(), String> {
        let (live, journaled) = {
            let mut reg = self.registry();
            (reg.live.remove(&id), reg.journal.remove(id))
        };
        if live.is_none() && !journaled {
            return Err(format!("no such session: {id}"));
        }
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

    /// Drop a session from the live set when its process exits — unless it
    /// was already replaced by a newer spawn (different token).
    fn forget_live(&self, id: SessionId, token: &str) {
        let mut reg = self.registry();
        if reg.live.get(&id).is_some_and(|h| h.token == token) {
            reg.live.remove(&id);
        }
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
