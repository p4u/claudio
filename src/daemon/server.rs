//! The daemon's Unix socket: startup, handshake, and the per-client loop.
//!
//! Every connection must come from our own uid and send its first frame within
//! 5 s. A `Hook` frame (from `claudio __hook`) is routed and the connection
//! closed; a `Hello` with our protocol version starts a client session.
//!
//! Each client gets a bounded outgoing queue drained by a writer task. Replies
//! are queued with `send().await`; session output is queued by the session
//! actors with `try_send` (see [`super::session`]). Session events reach every
//! client through the daemon's broadcast channel, forwarded by the client
//! loop. Before a reply is queued, pending events are flushed first, so a
//! client sees e.g. `Created` before its own `Spawned`.

use std::collections::HashSet;
use std::fs::{File, OpenOptions, Permissions, TryLockError};
use std::io::{self, Seek, SeekFrom, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::broadcast::error::{RecvError, TryRecvError};
use tokio::sync::{broadcast, mpsc};

use super::session::{ClientId, Cmd};
use super::{host, Config, Daemon};
use crate::claude::projects;
use crate::paths;
use crate::proto::{
    self, Envelope, Frame, Msg, ProjectDir, SessionId, SessionKind, SpawnSpec, Welcome, MAX_FRAME,
    PROTO,
};

/// Deadline for a connection's first frame.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// Outgoing frames buffered per client.
const CLIENT_QUEUE: usize = 512;

/// Incoming frames buffered between the socket reader and the client loop.
const INBOUND_QUEUE: usize = 64;

/// How often the serve loop checks that the socket path still exists and
/// re-binds if a tmp cleaner has removed it.
const SOCKET_HEALTH_INTERVAL: Duration = Duration::from_secs(30);

/// A bound, locked daemon ready to serve.
pub struct Listening {
    daemon: Arc<Daemon>,
    listener: UnixListener,
    /// Held (locked) for the daemon's lifetime.
    _lock: File,
}

/// Take the startup lock, bind the socket (mode 0600) and load the journal.
/// `Ok(None)` means another daemon holds the lock. Must run inside a tokio
/// runtime.
pub fn start(config: Config) -> io::Result<Option<Listening>> {
    for dir in [config.socket.parent(), config.lock.parent()]
        .into_iter()
        .flatten()
    {
        paths::ensure_private_dir(dir)?;
    }
    let mut lock = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&config.lock)?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return Ok(None),
        Err(TryLockError::Error(e)) => return Err(e),
    }
    // Write our PID into the lock file so `claudio daemon stop` can find this
    // daemon as a fallback when IPC is unavailable.
    lock.set_len(0)?;
    lock.seek(SeekFrom::Start(0))?;
    writeln!(lock, "{}", std::process::id())?;
    lock.flush()?;
    let socket_path = config.socket.clone();
    let listener = bind(&socket_path)?;
    tracing::info!(socket = %config.socket.display(), "daemon listening");

    let daemon = Arc::new(Daemon::new(config));
    // Warm the host probe (`claude --version`) so the first Welcome is quick.
    let warm = Arc::clone(&daemon);
    tokio::spawn(async move {
        warm.host().await;
    });
    Ok(Some(Listening {
        daemon,
        listener,
        _lock: lock,
    }))
}

/// Bind `path`, replacing a stale socket left by a dead daemon (we hold the
/// lock, so no live daemon owns it).
fn bind(path: &Path) -> io::Result<UnixListener> {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, Permissions::from_mode(0o600))?;
    Ok(listener)
}

impl Listening {
    /// Accept connections forever, re-binding the socket if it disappears.
    /// Exits when the daemon's shutdown notification fires.
    pub async fn serve(mut self) {
        let mut next: ClientId = 0;
        let socket_path = self.daemon.config.socket.clone();
        let mut health_tick = tokio::time::interval(SOCKET_HEALTH_INTERVAL);
        health_tick.tick().await; // consume the immediate first tick

        loop {
            tokio::select! {
                biased;
                _ = self.daemon.shutdown.notified() => {
                    tracing::info!("daemon shutdown requested; exiting");
                    break;
                }
                _ = health_tick.tick() => {
                    self.maybe_rebind(&socket_path);
                }
                accept = self.listener.accept() => match accept {
                    Ok((stream, _)) => {
                        next += 1;
                        tokio::spawn(connection(Arc::clone(&self.daemon), stream, next));
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "accept failed");
                        // On accept error the socket may have been removed;
                        // try to rebind immediately before backing off.
                        self.maybe_rebind(&socket_path);
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                },
            }
        }
    }

    /// Check whether the socket path still exists and re-bind if it has been
    /// removed (e.g. by a tmp cleaner). We hold the lock so no other daemon
    /// will race us.
    fn maybe_rebind(&mut self, path: &Path) {
        // If the socket file is gone, re-create it.
        match path.symlink_metadata() {
            Ok(_) => {} // still there
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                tracing::warn!(path = %path.display(), "socket path disappeared; re-binding");
                match bind(path) {
                    Ok(listener) => {
                        self.listener = listener;
                        tracing::info!(path = %path.display(), "socket re-bound");
                    }
                    Err(e) => tracing::error!(error = %e, "could not re-bind socket"),
                }
            }
            Err(e) => tracing::warn!(error = %e, "could not stat socket path"),
        }
    }
}

/// Authenticate, read the first frame, and dispatch on it.
async fn connection(daemon: Arc<Daemon>, mut stream: UnixStream, client: ClientId) {
    match stream.peer_cred() {
        Ok(cred) if cred.uid() == paths::uid() => {}
        Ok(cred) => {
            tracing::warn!(uid = cred.uid(), "rejected connection from another user");
            return;
        }
        Err(e) => {
            tracing::warn!(error = %e, "could not read peer credentials");
            return;
        }
    }
    let first = match tokio::time::timeout(HANDSHAKE_TIMEOUT, proto::read_frame(&mut stream)).await
    {
        Ok(Ok(Some(Frame::Control(env)))) => env,
        Ok(Ok(_)) => return,
        Ok(Err(e)) => {
            tracing::debug!(error = %e, "bad first frame");
            return;
        }
        Err(_) => {
            tracing::debug!("handshake timed out");
            return;
        }
    };
    match first.msg {
        Msg::Hook {
            token,
            event,
            payload,
        } => daemon.hook(&token, event, payload).await,
        Msg::Hello(hello) if hello.proto == PROTO => {
            tracing::debug!(client, version = %hello.claudio_version, "client connected");
            client_loop(daemon, stream, client, first.req).await;
            tracing::debug!(client, "client disconnected");
        }
        Msg::Hello(hello) => {
            let message = format!(
                "protocol mismatch: daemon speaks {PROTO}, client {}",
                hello.proto
            );
            let _ = refuse(&mut stream, first.req, message).await;
        }
        _ => {
            let _ = refuse(&mut stream, first.req, "expected hello".into()).await;
        }
    }
}

async fn refuse(stream: &mut UnixStream, req: Option<u64>, message: String) -> io::Result<()> {
    let reply = Envelope {
        req,
        msg: Msg::Error { message },
    };
    proto::write_frame(stream, &Frame::Control(reply)).await
}

/// Serve one client until it disconnects, then detach it everywhere.
async fn client_loop(
    daemon: Arc<Daemon>,
    stream: UnixStream,
    id: ClientId,
    hello_req: Option<u64>,
) {
    let (rd, wr) = stream.into_split();
    let (queue, queue_rx) = mpsc::channel(CLIENT_QUEUE);
    let (inbound_tx, mut inbound) = mpsc::channel(INBOUND_QUEUE);
    let writer = tokio::spawn(write_loop(wr, queue_rx));
    let reader = tokio::spawn(read_loop(rd, inbound_tx));

    // Subscribe before Welcome so no event after it is missed.
    let events = daemon.events.subscribe();
    let mut client = Client {
        id,
        daemon,
        queue,
        events,
        attached: HashSet::new(),
    };
    let welcome = Welcome {
        claudio_version: env!("CARGO_PKG_VERSION").to_owned(),
        proto: PROTO,
        host: client.daemon.host().await,
    };
    // Queued directly (no event flush): Welcome is always the first frame.
    let welcome = Envelope {
        req: hello_req,
        msg: Msg::Welcome(welcome),
    };
    let _ = client.queue.send(Frame::Control(welcome)).await;

    loop {
        tokio::select! {
            item = inbound.recv() => match item {
                Some(Inbound::Frame(frame)) => client.handle(frame).await,
                Some(Inbound::Undecodable { req, message }) => {
                    client.reply(req, Msg::Error { message }).await
                }
                None => break,
            },
            event = client.events.recv() => match event {
                Ok(frame) => client.forward(frame).await,
                Err(RecvError::Lagged(n)) => tracing::warn!(client = id, missed = n, "client missed events"),
                Err(RecvError::Closed) => break,
            },
        }
    }

    client.detach_all().await;
    reader.abort();
    writer.abort();
}

/// What the socket reader hands the client loop.
enum Inbound {
    Frame(Frame),
    /// A well-framed control message we could not decode (e.g. an op from a
    /// newer client). Answered with `Error`; the connection stays up.
    Undecodable {
        req: Option<u64>,
        message: String,
    },
}

async fn read_loop(mut rd: OwnedReadHalf, inbound: mpsc::Sender<Inbound>) {
    loop {
        let item = match read_body(&mut rd).await {
            Ok(Some(body)) => match Frame::decode(&body) {
                Ok(frame) => Inbound::Frame(frame),
                Err(e) => match undecodable(&body, e) {
                    Some(item) => item,
                    None => continue,
                },
            },
            Ok(None) => break,
            Err(e) => {
                tracing::debug!(error = %e, "client read failed");
                break;
            }
        };
        if inbound.send(item).await.is_err() {
            break;
        }
    }
}

/// Read one frame body (tag + payload). Unlike [`proto::read_frame`], a body
/// that fails to decode leaves the stream in sync, so it can be answered.
async fn read_body(rd: &mut OwnedReadHalf) -> io::Result<Option<Vec<u8>>> {
    let mut len = [0u8; 4];
    match rd.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_be_bytes(len) as usize;
    if len == 0 || len > proto::MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame length {len} out of range"),
        ));
    }
    let mut body = vec![0u8; len];
    rd.read_exact(&mut body).await?;
    Ok(Some(body))
}

/// A control frame that failed to decode becomes an `Error` reply (echoing
/// its `req` if it has one); a malformed data frame is dropped.
fn undecodable(body: &[u8], error: io::Error) -> Option<Inbound> {
    let json = body.strip_prefix(b"J")?;
    let req = serde_json::from_slice::<serde_json::Value>(json)
        .ok()
        .and_then(|v| v.get("req").and_then(serde_json::Value::as_u64));
    Some(Inbound::Undecodable {
        req,
        message: format!("unsupported request: {error}"),
    })
}

async fn write_loop(mut wr: OwnedWriteHalf, mut queue: mpsc::Receiver<Frame>) {
    while let Some(frame) = queue.recv().await {
        if let Err(e) = proto::write_frame(&mut wr, &frame).await {
            tracing::debug!(error = %e, "client write failed");
            break;
        }
    }
}

/// One connected TUI client.
struct Client {
    id: ClientId,
    daemon: Arc<Daemon>,
    /// Outgoing frames (drained by the writer task).
    queue: mpsc::Sender<Frame>,
    events: broadcast::Receiver<Frame>,
    /// Sessions this client is subscribed to, for cleanup on disconnect.
    attached: HashSet<SessionId>,
}

impl Client {
    async fn handle(&mut self, frame: Frame) {
        match frame {
            Frame::Data { session, bytes } => {
                if let Some(tx) = self.daemon.session(session) {
                    let _ = tx.send(Cmd::Input(bytes)).await;
                }
            }
            Frame::Control(env) => self.request(env.req, env.msg).await,
        }
    }

    async fn request(&mut self, req: Option<u64>, msg: Msg) {
        let reply = match msg {
            Msg::Ping => Msg::Pong,
            Msg::Shutdown => {
                // Authenticated (uid already checked at connection time).
                tracing::info!("Shutdown requested by client {}", self.id);
                self.daemon.shutdown.notify_one();
                Msg::Ok
            }
            Msg::ListSessions => Msg::Sessions {
                sessions: self.daemon.sessions().await,
            },
            Msg::Spawn(spec) => self.spawn(spec, SessionKind::Claude).await,
            Msg::SpawnShell(shell) => {
                let spec = SpawnSpec {
                    id: shell.id,
                    cwd: shell.cwd,
                    name: shell.name,
                    args: Vec::new(),
                    env: Vec::new(),
                    rows: shell.rows,
                    cols: shell.cols,
                };
                self.spawn(spec, SessionKind::Shell).await
            }
            Msg::Attach { id, rows, cols } => match self.attach(req, id, rows, cols).await {
                // The session actor replies `Attached` itself.
                Ok(()) => return,
                Err(message) => Msg::Error { message },
            },
            Msg::Detach { id } => {
                self.attached.remove(&id);
                self.to_session(id, Cmd::Detach { client: self.id }).await
            }
            Msg::Resize { id, rows, cols } => {
                self.to_session(
                    id,
                    Cmd::Resize {
                        client: self.id,
                        rows,
                        cols,
                    },
                )
                .await
            }
            Msg::Kill { id } => {
                self.attached.remove(&id);
                match self.daemon.kill(id).await {
                    Ok(()) => Msg::Ok,
                    Err(e) => Msg::Error {
                        message: e.to_string(),
                    },
                }
            }
            Msg::Rename { id, name } => {
                if self.daemon.rename(id, name).await {
                    Msg::Ok
                } else {
                    Msg::Error {
                        message: format!("no such session: {id}"),
                    }
                }
            }
            Msg::ListDir { path } => list_dir(path).await,
            Msg::ListClaudeSessions { cwd } => list_claude_sessions(cwd).await,
            Msg::RecentProjects { limit } => recent_projects(limit).await,
            Msg::SubscribeHostStats => {
                // Spawn a task that watches the stats channel and pushes
                // HostStats events to this client whenever the value changes.
                let mut rx = self.daemon.stats_rx.clone();
                let queue = self.queue.clone();
                tokio::spawn(async move {
                    loop {
                        if rx.changed().await.is_err() {
                            break;
                        }
                        let snap = *rx.borrow_and_update();
                        let msg = crate::proto::Msg::HostStats {
                            cpu_pct: snap.cpu_pct,
                            mem_used: snap.mem_used,
                            mem_total: snap.mem_total,
                            load1: None,
                        };
                        if queue
                            .send(Frame::Control(crate::proto::Envelope::event(msg)))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                });
                Msg::Ok
            }
            other => Msg::Error {
                message: format!("unsupported op: {}", op_name(&other)),
            },
        };
        self.reply(req, reply).await;
    }

    async fn spawn(&self, spec: SpawnSpec, kind: SessionKind) -> Msg {
        let id = spec.id;
        match self.daemon.spawn(spec, kind).await {
            Ok(pid) => Msg::Spawned { id, pid },
            Err(e) => Msg::Error {
                message: e.to_string(),
            },
        }
    }

    async fn attach(
        &mut self,
        req: Option<u64>,
        id: SessionId,
        rows: u16,
        cols: u16,
    ) -> Result<(), String> {
        let tx = self.daemon.session(id).ok_or_else(|| not_running(id))?;
        let cmd = Cmd::Attach {
            client: self.id,
            queue: self.queue.clone(),
            req,
            rows,
            cols,
        };
        tx.send(cmd).await.map_err(|_| not_running(id))?;
        self.attached.insert(id);
        Ok(())
    }

    /// Send `cmd` to a live session; the reply is `Ok` or `Error`.
    async fn to_session(&self, id: SessionId, cmd: Cmd) -> Msg {
        match self.daemon.session(id) {
            Some(tx) if tx.send(cmd).await.is_ok() => Msg::Ok,
            _ => Msg::Error {
                message: not_running(id),
            },
        }
    }

    /// Queue a reply, after any events that happened before it.
    async fn reply(&mut self, req: Option<u64>, msg: Msg) {
        self.flush_events().await;
        let _ = self.queue.send(Frame::Control(Envelope { req, msg })).await;
    }

    async fn flush_events(&mut self) {
        loop {
            match self.events.try_recv() {
                Ok(frame) => self.forward(frame).await,
                Err(TryRecvError::Lagged(n)) => {
                    tracing::warn!(client = self.id, missed = n, "client missed events")
                }
                Err(TryRecvError::Empty | TryRecvError::Closed) => break,
            }
        }
    }

    /// Forward a broadcast event. This task is the client's own, so waiting
    /// on its queue delays only this client (the broadcast buffers meanwhile).
    async fn forward(&self, frame: Frame) {
        let _ = self.queue.send(frame).await;
    }

    async fn detach_all(&mut self) {
        for id in std::mem::take(&mut self.attached) {
            if let Some(tx) = self.daemon.session(id) {
                let _ = tx.send(Cmd::Detach { client: self.id }).await;
            }
        }
    }
}

fn not_running(id: SessionId) -> String {
    format!("session {id} is not running")
}

/// Approximate encoded size of a `DirEntry` (name + bool + JSON overhead).
fn dir_entry_encoded_size(name: &str) -> usize {
    // JSON: {"name":"<name>","dir":false} ≈ 20 + name.len() bytes.
    name.len() + 24
}

async fn list_dir(path: String) -> Msg {
    let listed = tokio::task::spawn_blocking(move || {
        host::list_dir(&path).map_err(|e| format!("cannot list {path}: {e}"))
    })
    .await;
    match listed {
        Ok(Ok((path, entries))) => {
            // Cap entries so the reply fits within MAX_FRAME.
            // Reserve ~256 bytes for the JSON envelope.
            let budget = MAX_FRAME.saturating_sub(256);
            let mut used = 0usize;
            let mut truncated = false;
            let mut capped: Vec<_> = Vec::new();
            for entry in entries {
                let sz = dir_entry_encoded_size(&entry.name);
                if used + sz > budget {
                    truncated = true;
                    break;
                }
                used += sz;
                capped.push(entry);
            }
            Msg::DirEntries {
                path,
                entries: capped,
                truncated,
            }
        }
        Ok(Err(message)) => Msg::Error { message },
        Err(e) => Msg::Error {
            message: format!("list_dir failed: {e}"),
        },
    }
}

async fn list_claude_sessions(cwd: String) -> Msg {
    let dir = host::expand_tilde(&cwd);
    match tokio::task::spawn_blocking(move || projects::list_sessions(&dir)).await {
        Ok(sessions) => Msg::ClaudeSessions { cwd, sessions },
        Err(e) => Msg::Error {
            message: format!("list_claude_sessions failed: {e}"),
        },
    }
}

async fn recent_projects(limit: u32) -> Msg {
    let limit = usize::try_from(limit).unwrap_or(usize::MAX);
    match tokio::task::spawn_blocking(move || projects::recent_project_dirs(limit)).await {
        Ok(dirs) => Msg::Projects {
            dirs: dirs
                .into_iter()
                .map(|(path, modified)| {
                    let path_str = path.to_string_lossy().into_owned();
                    let git = host::detect_git_branch(&path);
                    let hidden = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.starts_with('.'))
                        .unwrap_or(false);
                    let symlink = path
                        .symlink_metadata()
                        .map(|m| m.file_type().is_symlink())
                        .unwrap_or(false);
                    ProjectDir {
                        path: path_str,
                        modified,
                        git,
                        hidden,
                        symlink,
                    }
                })
                .collect(),
        },
        Err(e) => Msg::Error {
            message: format!("recent_projects failed: {e}"),
        },
    }
}

/// The wire `op` of a message, for error text.
fn op_name(msg: &Msg) -> String {
    serde_json::to_value(msg)
        .ok()
        .and_then(|v| v.get("op").and_then(|op| op.as_str()).map(str::to_owned))
        .unwrap_or_else(|| "unknown".into())
}
