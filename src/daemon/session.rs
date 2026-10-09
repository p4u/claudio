//! One live session: `claude` under a PTY, driven by a single actor task.
//!
//! The actor alone owns the PTY master, the child killer, the emulated
//! [`Screen`], the [`ProbeResponder`], the hook [`Tracker`] and the list of
//! subscribed clients — no locks. Three helper threads do the blocking work:
//!
//! - **reader**: PTY output → actor (`blocking_send`, so a busy actor slows
//!   the child down rather than buffering without bound);
//! - **writer**: actor → PTY input (keyboard input and probe replies), so a
//!   child that stops reading stdin can never stall the async runtime;
//! - **waiter**: reaps the child and reports its exit code.
//!
//! Output fans out to subscribers with `try_send`. A subscriber whose queue is
//! full is marked *lagging* and skipped; once its queue drains it gets a fresh
//! `Attached` + snapshot instead of the bytes it missed. A slow client
//! therefore never blocks the PTY.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::mpsc as std_mpsc;
use std::sync::Arc;
use std::time::Duration;

use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde_json::Value;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, oneshot};

use super::{host, Daemon};
use crate::claude::hooks;
use crate::claude::state::Tracker;
use crate::proto::{Envelope, Frame, Msg, SessionEvent, SessionId, SessionState, SpawnSpec};
use crate::term::probe::ProbeResponder;
use crate::term::screen::Screen;

/// Identifies one client connection.
pub type ClientId = u64;

/// Largest `D` frame payload the daemon sends.
const CHUNK: usize = 64 * 1024;

/// A lagging subscriber is resynced once this many queue slots are free.
const RESYNC_ROOM: usize = 64;

/// Timer-based lag recovery: check for resyncs this often even without new output.
const LAG_CHECK_INTERVAL: Duration = Duration::from_millis(250);

/// Queue depths: actor commands, PTY output chunks, PTY input writes.
const CMD_QUEUE: usize = 256;
const OUTPUT_QUEUE: usize = 64;
const INPUT_QUEUE: usize = 256;

/// How long after child exit to drain remaining PTY output before giving up.
const EXIT_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

/// If claude exits with no SessionStart within this window, consider it a
/// failed `--resume` (conversation gone) and retry once without `--resume`.
const RESUME_RETRY_WINDOW: Duration = Duration::from_secs(3);

/// After `Kill`'s SIGHUP, how long the child gets before SIGKILL.
const KILL_GRACE: Duration = Duration::from_secs(3);

/// Default size when a spec carries a zero dimension.
const DEFAULT_SIZE: (u16, u16) = (24, 80);

/// Messages to a session actor.
pub enum Cmd {
    /// Keyboard/mouse bytes for the PTY.
    Input(Vec<u8>),
    /// Subscribe `queue`: resize, reply `Attached` (echoing `req`), send the
    /// snapshot — all in one step so no output is lost or doubled.
    Attach {
        client: ClientId,
        queue: mpsc::Sender<Frame>,
        req: Option<u64>,
        rows: u16,
        cols: u16,
    },
    Detach {
        client: ClientId,
    },
    /// Resize, then resync every subscriber except `client`.
    Resize {
        client: ClientId,
        rows: u16,
        cols: u16,
    },
    Hook {
        event: String,
        payload: Value,
    },
    Status(oneshot::Sender<Status>),
    /// Kill the child and stop. The registry has already forgotten it.
    Kill,
}

/// What `ListSessions` needs from a live actor.
pub struct Status {
    pub state: SessionState,
    pub title: Option<String>,
}

/// The registry's handle on a live session.
pub struct Handle {
    pub tx: mpsc::Sender<Cmd>,
    /// This spawn's hook token; hooks carrying any other token are ignored.
    pub token: String,
    pub pid: Option<u32>,
}

/// Ask a live actor for its status; `None` if it is gone or too busy.
pub async fn status(tx: &mpsc::Sender<Cmd>, wait: Duration) -> Option<Status> {
    let (reply, rx) = oneshot::channel();
    tx.send(Cmd::Status(reply)).await.ok()?;
    tokio::time::timeout(wait, rx).await.ok()?.ok()
}

/// Start claude for `spec` under a new PTY and spawn its actor.
/// Called from `spawn_blocking` — must not use tokio primitives.
pub fn spawn(daemon: &Arc<Daemon>, spec: &SpawnSpec) -> io::Result<Handle> {
    let cwd = host::expand_tilde(&spec.cwd);
    if !cwd.is_dir() {
        return Err(io::Error::other(format!(
            "working directory {} does not exist",
            cwd.display()
        )));
    }
    let token = hooks::new_token();
    let cmd = command(&daemon.config, spec, &cwd, &token);
    let (rows, cols) = size_or_default(spec.rows, spec.cols);

    let pair = native_pty_system()
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| io::Error::other(format!("could not open a pty: {e}")))?;
    let child = pair.slave.spawn_command(cmd).map_err(|e| {
        io::Error::other(format!(
            "could not start {}: {e}",
            daemon.config.claude.display()
        ))
    })?;
    drop(pair.slave);

    let pid = child.process_id();
    let mut killer = child.clone_killer();
    let io = match start_io(pair.master.as_ref(), child) {
        Ok(io) => io,
        Err(e) => {
            let _ = killer.kill();
            return Err(io::Error::other(format!(
                "could not start session i/o: {e}"
            )));
        }
    };

    // Detect whether this spawn uses --resume (for retry logic, §2.3).
    let spawned_with_resume = spec.args.iter().any(|a| a == "--resume");
    let spawn_time = std::time::Instant::now();

    let (tx, cmds) = mpsc::channel(CMD_QUEUE);
    let actor = Actor {
        id: spec.id,
        daemon: Arc::clone(daemon),
        token: token.clone(),
        master: pair.master,
        input: io.input,
        killer,
        pid,
        screen: Screen::new(rows, cols),
        probe: ProbeResponder::new(rows, cols),
        tracker: Tracker::new(),
        title: None,
        subs: HashMap::new(),
        spawned_with_resume,
        spawn_time,
    };
    tokio::spawn(actor.run(cmds, io.output, io.exit));
    Ok(Handle { tx, token, pid })
}

/// `<claude> --settings <hooks> [args…] [-n <name>]`, in `cwd`, with the
/// session env applied.
fn command(cfg: &super::Config, spec: &SpawnSpec, cwd: &Path, token: &str) -> CommandBuilder {
    let settings = hooks::settings(&cfg.claudio, &cfg.socket, token);
    let mut cmd = CommandBuilder::new(&cfg.claude);
    cmd.arg("--settings");
    cmd.arg(settings.to_string());
    for a in &spec.args {
        cmd.arg(a);
    }
    if let Some(name) = &spec.name {
        cmd.arg("-n");
        cmd.arg(name);
    }
    cmd.cwd(cwd);
    apply_env(&mut cmd, &spec.env);
    cmd
}

/// Start from the daemon's env (the builder's default), scrub what would leak
/// from a daemon started inside claude, set the terminal type, then apply the
/// spec's env. A proxy token replaces any API key.
fn apply_env(cmd: &mut CommandBuilder, env: &[(String, String)]) {
    for marker in CLAUDE_SESSION_MARKERS {
        cmd.env_remove(marker);
    }
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    for (k, v) in env {
        cmd.env(k, v);
    }
    if env.iter().any(|(k, _)| k == "ANTHROPIC_AUTH_TOKEN") {
        cmd.env_remove("ANTHROPIC_API_KEY");
    }
}

/// claude animates its terminal title with a leading status glyph
/// (`✳ Claude Code`, `◐ Fix the parser`); strip it so the title only changes
/// — and is only broadcast — when the actual text does.
fn clean_title(title: &str) -> &str {
    title
        .trim_start_matches(|c: char| !c.is_alphanumeric())
        .trim()
}

/// Variables a running claude sets for its own children. A daemon started
/// from inside a claude session inherits them, and a session spawned with them
/// believes it is a child: it turns off transcript saving (breaking `--resume`)
/// and talks to the parent's messaging socket. User settings such as
/// `CLAUDE_CODE_USE_GATEWAY` are deliberately kept.
const CLAUDE_SESSION_MARKERS: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_CODE_SSE_PORT",
    "CLAUDE_SESSION_ID",
    "CLAUDE_PID",
    "CLAUDE_EFFORT",
];

fn size_or_default(rows: u16, cols: u16) -> (u16, u16) {
    if rows == 0 || cols == 0 {
        DEFAULT_SIZE
    } else {
        (rows, cols)
    }
}

/// Channels to the session's helper threads.
struct Io {
    input: std_mpsc::SyncSender<Vec<u8>>,
    output: mpsc::Receiver<Vec<u8>>,
    exit: oneshot::Receiver<Option<i32>>,
}

/// Spawn the reader, writer and waiter threads.
fn start_io(
    master: &(dyn MasterPty + Send),
    mut child: Box<dyn portable_pty::Child + Send + Sync>,
) -> std::io::Result<Io> {
    let reader = master.try_clone_reader().map_err(std::io::Error::other)?;
    let writer = master.take_writer().map_err(std::io::Error::other)?;
    let (out_tx, output) = mpsc::channel(OUTPUT_QUEUE);
    let (input, in_rx) = std_mpsc::sync_channel(INPUT_QUEUE);
    let (exit_tx, exit) = oneshot::channel();

    thread("claudio-pty-read", move || read_pty(reader, out_tx))?;
    thread("claudio-pty-write", move || write_pty(writer, in_rx))?;
    thread("claudio-pty-wait", move || {
        let code = child.wait().ok().map(|s| s.exit_code() as i32);
        let _ = exit_tx.send(code);
    })?;
    Ok(Io {
        input,
        output,
        exit,
    })
}

fn thread(name: &str, f: impl FnOnce() + Send + 'static) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(f)
        .map(drop)
}

fn read_pty(mut reader: Box<dyn Read + Send>, out: mpsc::Sender<Vec<u8>>) {
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if out.blocking_send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            // EIO once the child side closes.
            Err(_) => break,
        }
    }
}

fn write_pty(mut writer: Box<dyn Write + Send>, input: std_mpsc::Receiver<Vec<u8>>) {
    for bytes in input {
        if writer
            .write_all(&bytes)
            .and_then(|_| writer.flush())
            .is_err()
        {
            break;
        }
    }
}

/// A client subscribed to this session's output.
struct Subscriber {
    queue: mpsc::Sender<Frame>,
    /// Its queue overflowed; output is withheld until a resync.
    lagging: bool,
    /// The `Attach` request id, until an `Attached` carrying it is queued.
    pending_req: Option<u64>,
}

impl Subscriber {
    /// Queue `Attached` + the snapshot. Returns `false` if the client is gone.
    fn sync(&mut self, id: SessionId, (rows, cols): (u16, u16), snapshot: &[u8]) -> bool {
        let attached = Envelope {
            req: self.pending_req,
            msg: Msg::Attached { id, rows, cols },
        };
        self.lagging = false;
        match self.queue.try_send(Frame::Control(attached)) {
            Ok(()) => self.pending_req = None,
            Err(TrySendError::Full(_)) => {
                self.lagging = true;
                return true;
            }
            Err(TrySendError::Closed(_)) => return false,
        }
        self.offer(id, snapshot)
    }

    /// Queue `bytes` as `D` frames, marking the subscriber lagging if its
    /// queue fills. Returns `false` if the client is gone.
    fn offer(&mut self, id: SessionId, bytes: &[u8]) -> bool {
        for chunk in bytes.chunks(CHUNK) {
            let frame = Frame::Data {
                session: id,
                bytes: chunk.to_vec(),
            };
            match self.queue.try_send(frame) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => {
                    self.lagging = true;
                    return true;
                }
                Err(TrySendError::Closed(_)) => return false,
            }
        }
        true
    }

    /// A lagging subscriber is ready for a resync.
    fn can_resync(&self) -> bool {
        self.lagging && self.queue.capacity() > RESYNC_ROOM
    }
}

struct Actor {
    id: SessionId,
    daemon: Arc<Daemon>,
    token: String,
    master: Box<dyn MasterPty + Send>,
    input: std_mpsc::SyncSender<Vec<u8>>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    pid: Option<u32>,
    screen: Screen,
    probe: ProbeResponder,
    tracker: Tracker,
    title: Option<String>,
    subs: HashMap<ClientId, Subscriber>,
    /// Whether this spawn included `--resume` (for retry logic on quick exit).
    spawned_with_resume: bool,
    /// When the child was spawned, for measuring quick-exit window.
    spawn_time: std::time::Instant,
}

impl Actor {
    async fn run(
        mut self,
        mut cmds: mpsc::Receiver<Cmd>,
        mut output: mpsc::Receiver<Vec<u8>>,
        mut exit: oneshot::Receiver<Option<i32>>,
    ) {
        // Timer-based lag recovery: resync even when the child is silent.
        let mut lag_check = tokio::time::interval(LAG_CHECK_INTERVAL);
        lag_check.tick().await; // consume the immediate first tick

        loop {
            tokio::select! {
                // Commands first: keystrokes and attaches must not queue
                // behind a long burst of output.
                biased;
                cmd = cmds.recv() => match cmd {
                    Some(Cmd::Kill) | None => {
                        drop(output);
                        self.kill(exit).await;
                        return;
                    }
                    Some(cmd) => self.on_cmd(cmd),
                },
                Some(bytes) = output.recv() => self.on_output(&bytes),
                code = &mut exit => {
                    let exit_code = code.ok().flatten();
                    // Drain until the reader thread closes the channel (PTY EOF)
                    // so final output is never lost. A deadline prevents blocking
                    // forever if another process holds the PTY slave open.
                    let drain_deadline =
                        tokio::time::Instant::now() + EXIT_DRAIN_TIMEOUT;
                    loop {
                        match tokio::time::timeout_at(drain_deadline, output.recv()).await {
                            Ok(Some(bytes)) => self.on_output(&bytes),
                            Ok(None) | Err(_) => break,
                        }
                    }
                    self.on_exit(exit_code);
                    return;
                }
                _ = lag_check.tick() => self.resync_lagging(),
            }
        }
    }

    /// Timer-driven resync for lagging subscribers.
    ///
    /// Called on every lag-check tick regardless of new PTY output, so a
    /// subscriber that overflowed during a burst gets resynced even if the
    /// child subsequently goes silent (e.g. waiting for input).
    fn resync_lagging(&mut self) {
        if !self.subs.values().any(Subscriber::can_resync) {
            return;
        }
        let id = self.id;
        let size = self.screen.size();
        let snapshot = self.screen.snapshot();
        self.subs.retain(|_, sub| {
            if sub.can_resync() {
                sub.sync(id, size, &snapshot)
            } else {
                !sub.queue.is_closed()
            }
        });
    }

    fn on_cmd(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Input(bytes) => self.write(bytes),
            Cmd::Attach {
                client,
                queue,
                req,
                rows,
                cols,
            } => self.attach(client, queue, req, rows, cols),
            Cmd::Detach { client } => {
                self.subs.remove(&client);
            }
            Cmd::Resize { client, rows, cols } => self.resize(client, rows, cols),
            Cmd::Hook { event, payload } => self.on_hook(&event, &payload),
            Cmd::Status(reply) => {
                let _ = reply.send(Status {
                    state: self.tracker.state,
                    title: self.title.clone(),
                });
            }
            // Handled by `run`.
            Cmd::Kill => {}
        }
    }

    /// PTY output: answer probes, update the screen, then fan out.
    fn on_output(&mut self, bytes: &[u8]) {
        self.probe.feed(bytes);
        let replies = self.probe.take_responses();
        if !replies.is_empty() {
            self.write(replies);
        }
        self.screen.feed(bytes);
        self.update_title();
        self.fan_out(bytes);
    }

    fn update_title(&mut self) {
        let title = self.screen.title().map(|t| clean_title(&t).to_owned());
        if title != self.title {
            self.title = title.clone();
            let title = title.unwrap_or_default();
            self.daemon
                .broadcast(self.id, SessionEvent::Title { title });
        }
    }

    /// Send output to every subscriber; resync lagging ones that caught up.
    /// The screen already includes `bytes`, so a resync replaces them.
    fn fan_out(&mut self, bytes: &[u8]) {
        let (id, size) = (self.id, self.screen.size());
        let snapshot = self
            .subs
            .values()
            .any(Subscriber::can_resync)
            .then(|| self.screen.snapshot());
        self.subs.retain(|_, sub| match (&snapshot, sub.lagging) {
            (Some(snap), true) if sub.can_resync() => sub.sync(id, size, snap),
            (_, true) => !sub.queue.is_closed(),
            (_, false) => sub.offer(id, bytes),
        });
    }

    fn attach(
        &mut self,
        client: ClientId,
        queue: mpsc::Sender<Frame>,
        req: Option<u64>,
        rows: u16,
        cols: u16,
    ) {
        // Re-attaching replaces the old subscription.
        self.subs.remove(&client);
        self.resize(client, rows, cols);
        let mut sub = Subscriber {
            queue,
            lagging: false,
            pending_req: req,
        };
        if sub.sync(self.id, self.screen.size(), &self.screen.snapshot()) {
            self.subs.insert(client, sub);
        }
    }

    /// Resize the PTY, screen and probe responder, then resync every
    /// subscriber except `client` (their mirrors are now the wrong size).
    fn resize(&mut self, client: ClientId, rows: u16, cols: u16) {
        if rows == 0 || cols == 0 || self.screen.size() == (rows, cols) {
            return;
        }
        let size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        if let Err(e) = self.master.resize(size) {
            tracing::warn!(id = %self.id, error = %e, "pty resize failed");
        }
        self.screen.resize(rows, cols);
        self.probe.resize(rows, cols);

        let (id, snapshot) = (self.id, self.screen.snapshot());
        self.subs
            .retain(|c, sub| *c == client || sub.sync(id, (rows, cols), &snapshot));
    }

    fn on_hook(&mut self, event: &str, payload: &Value) {
        for ev in self.tracker.apply(event, payload) {
            if let SessionEvent::ClaudeSession { claude_session_id } = &ev {
                self.daemon
                    .record_claude_session(self.id, claude_session_id);
            }
            self.daemon.broadcast(self.id, ev);
        }
    }

    fn write(&mut self, bytes: Vec<u8>) {
        if let Err(std_mpsc::TrySendError::Full(_)) = self.input.try_send(bytes) {
            tracing::warn!(id = %self.id, "pty input queue full; input dropped");
        }
    }

    /// The child exited: the session goes dormant (it stays journaled).
    ///
    /// **Resume-failure retry (design §2.3):** if this spawn used `--resume`
    /// and the child exited within [`RESUME_RETRY_WINDOW`] with a non-zero
    /// exit code before any `SessionStart` hook fired, we broadcast a notice
    /// and re-spawn without `--resume` (the conversation may have been deleted).
    /// The retry happens at most once per spawn.
    fn on_exit(&mut self, code: Option<i32>) {
        tracing::info!(id = %self.id, ?code, "session exited");

        // Check resume-retry condition (§2.3).
        let quick_exit = self.spawn_time.elapsed() < RESUME_RETRY_WINDOW;
        let no_session_start = self.tracker.state == crate::proto::SessionState::Starting;
        let nonzero_exit = code.map(|c| c != 0).unwrap_or(true);

        if self.spawned_with_resume && quick_exit && no_session_start && nonzero_exit {
            tracing::info!(id = %self.id, "resume failed (conversation gone?); will retry fresh");
            // Forget the stale live entry before retrying, so retry_spawn_fresh
            // does not find a stale entry and return the old pid without spawning.
            self.daemon.forget_live(self.id, &self.token);
            // Emit a notice (not Title) so the TUI shows it in the status bar.
            self.daemon.broadcast(
                self.id,
                SessionEvent::Notice {
                    text: "conversation not found; started fresh".to_owned(),
                },
            );
            // Attempt a fresh spawn (no --resume) via the daemon.
            // This is best-effort; we proceed to dormant on failure.
            let id = self.id;
            let daemon = Arc::clone(&self.daemon);
            tokio::spawn(async move {
                daemon.retry_spawn_fresh(id).await;
            });
            return;
        }

        self.daemon.forget_live(self.id, &self.token);
        self.daemon.broadcast(
            self.id,
            SessionEvent::State {
                state: SessionState::Exited,
            },
        );
        self.daemon
            .broadcast(self.id, SessionEvent::Exited { code });
    }

    /// SIGHUP the child; SIGKILL it if it is still around after a grace
    /// period. The waiter has not reaped it yet, so the pid is still ours.
    async fn kill(mut self, exit: oneshot::Receiver<Option<i32>>) {
        let _ = self.killer.kill();
        if tokio::time::timeout(KILL_GRACE, exit).await.is_err() {
            if let Some(pid) = self.pid.and_then(|p| i32::try_from(p).ok()) {
                // SAFETY: kill(2) has no memory-safety preconditions.
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn title_glyphs_are_stripped() {
        assert_eq!(clean_title("◐ Claude Code"), "Claude Code");
        assert_eq!(clean_title("✳ Fix the parser"), "Fix the parser");
        assert_eq!(clean_title("plain"), "plain");
        assert_eq!(clean_title("◑ "), "");
    }

    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn full_queue_lags_then_resyncs_with_pending_req() {
        let id = uuid::Uuid::new_v4();
        let (queue, mut rx) = mpsc::channel(RESYNC_ROOM + 8);
        let mut sub = Subscriber {
            queue,
            lagging: false,
            pending_req: Some(7),
        };

        // Fill the queue: the next output marks it lagging (not gone).
        for _ in 0..RESYNC_ROOM + 8 {
            assert!(sub.offer(id, b"x"));
        }
        assert!(!sub.lagging);
        assert!(sub.offer(id, b"y"));
        assert!(sub.lagging);
        assert!(!sub.can_resync());

        // Once it drains, a resync sends Attached (with the pending req) and
        // the snapshot, and clears the flag.
        while rx.try_recv().is_ok() {}
        assert!(sub.can_resync());
        assert!(sub.sync(id, (24, 80), b"SNAP"));
        assert!(!sub.lagging);
        let attached = Envelope {
            req: Some(7),
            msg: Msg::Attached {
                id,
                rows: 24,
                cols: 80,
            },
        };
        assert_eq!(rx.try_recv().unwrap(), Frame::Control(attached));
        assert_eq!(
            rx.try_recv().unwrap(),
            Frame::Data {
                session: id,
                bytes: b"SNAP".to_vec()
            }
        );
        assert_eq!(sub.pending_req, None);

        // A vanished client is reported so the actor can drop it.
        drop(rx);
        assert!(!sub.offer(id, b"z"));
    }

    #[test]
    fn output_is_chunked() {
        let id = uuid::Uuid::new_v4();
        let (queue, mut rx) = mpsc::channel(8);
        let mut sub = Subscriber {
            queue,
            lagging: false,
            pending_req: None,
        };
        assert!(sub.offer(id, &vec![b'a'; CHUNK + 1]));
        let sizes: Vec<usize> = std::iter::from_fn(|| rx.try_recv().ok())
            .map(|f| match f {
                Frame::Data { bytes, .. } => bytes.len(),
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        assert_eq!(sizes, [CHUNK, 1]);
    }

    #[test]
    fn env_is_scrubbed_and_token_replaces_api_key() {
        let mut cmd = CommandBuilder::new("claude");
        cmd.env("CLAUDECODE", "1");
        cmd.env("CLAUDE_CODE_ENTRYPOINT", "cli");
        cmd.env("CLAUDE_CODE_CHILD_SESSION", "1");
        cmd.env("CLAUDE_CODE_USE_GATEWAY", "1");
        cmd.env("ANTHROPIC_API_KEY", "old");
        apply_env(&mut cmd, &[("ANTHROPIC_AUTH_TOKEN".into(), "tok".into())]);
        assert_eq!(cmd.get_env("CLAUDECODE"), None);
        assert_eq!(cmd.get_env("CLAUDE_CODE_ENTRYPOINT"), None);
        assert_eq!(cmd.get_env("CLAUDE_CODE_CHILD_SESSION"), None);
        // User settings survive; only per-session markers are scrubbed.
        assert_eq!(cmd.get_env("CLAUDE_CODE_USE_GATEWAY").and_then(|v| v.to_str()), Some("1"));
        assert_eq!(cmd.get_env("ANTHROPIC_API_KEY"), None);
        assert_eq!(cmd.get_env("ANTHROPIC_AUTH_TOKEN"), Some(OsStr::new("tok")));
        assert_eq!(cmd.get_env("TERM"), Some(OsStr::new("xterm-256color")));
        assert_eq!(cmd.get_env("COLORTERM"), Some(OsStr::new("truecolor")));
    }

    #[test]
    fn api_key_kept_without_auth_token() {
        let mut cmd = CommandBuilder::new("claude");
        cmd.env("ANTHROPIC_API_KEY", "key");
        apply_env(&mut cmd, &[]);
        assert_eq!(cmd.get_env("ANTHROPIC_API_KEY"), Some(OsStr::new("key")));
    }

    #[test]
    fn command_line_shape() {
        let cfg = super::super::Config {
            socket: "/run/d.sock".into(),
            lock: "/run/d.lock".into(),
            journal: "/j.json".into(),
            claude: "/bin/claude".into(),
            claudio: "/bin/claudio".into(),
        };
        let spec = SpawnSpec {
            id: uuid::Uuid::nil(),
            cwd: "/".into(),
            name: Some("work".into()),
            args: vec!["--resume".into(), "abc".into()],
            env: vec![],
            rows: 24,
            cols: 80,
        };
        let cmd = command(&cfg, &spec, Path::new("/"), "tok123");
        let argv: Vec<String> = cmd
            .get_argv()
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(argv[0], "/bin/claude");
        assert_eq!(argv[1], "--settings");
        let settings: Value = serde_json::from_str(&argv[2]).unwrap();
        let hook = settings["hooks"]["Stop"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        assert!(hook.contains("tok123") && hook.contains("/run/d.sock"));
        assert_eq!(&argv[3..], ["--resume", "abc", "-n", "work"]);
    }
}
