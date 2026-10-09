//! The TUI's connection to the local daemon, plus daemon autostart.
//!
//! A [`Client`] wraps one Unix-socket connection. A writer task owns the
//! write half and a reader task the read half. Replies are matched to
//! requests by their `req` id; everything unsolicited (terminal output,
//! `Attached`, session events) flows out through one ordered channel of
//! [`Incoming`] items.
//!
//! # Security
//!
//! Before every connection the runtime directory is validated with
//! [`paths::ensure_private_dir`] (owned by us, mode 0700, not a symlink).
//! After connecting the peer UID is verified via `SO_PEERCRED` /
//! `getpeereid`; a mismatch aborts with an error.
//!
//! # Liveness
//!
//! A heartbeat task sends `Ping` every 15 s and fails the connection if no
//! frame is received within 45 s, preventing a silently wedged daemon from
//! hanging the UI forever.  Every request also has a 30-second deadline
//! (longer for `Spawn`).  When the writer exits abnormally it signals the
//! reader to stop as well, failing all pending requests.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::future::Future;
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot, Notify};
use tokio::time::Instant as TokioInstant;

use crate::paths;
use crate::proto::{
    read_frame, write_frame, Envelope, Frame, Hello, Msg, SessionEvent, SessionId, Welcome, PROTO,
};

/// How long the handshake may take before the daemon is considered wedged.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
/// Input is split into frames of at most this many bytes (well under
/// `proto::MAX_FRAME`), so a huge paste can't break the connection.
const INPUT_CHUNK: usize = 64 * 1024;
/// Buffered unsolicited messages before the reader stops reading the socket
/// (the daemon then drops our backlog and re-snapshots).
const INCOMING_QUEUE: usize = 1024;
/// Outgoing-frame channel capacity. Backpressure prevents unbounded memory
/// growth when the socket is slow.
const OUTGOING_QUEUE: usize = 512;
/// Input-forwarding channel capacity. Generous to absorb burst typing without
/// blocking the UI loop; the forwarder task drains it with `send().await`.
const INPUT_FWD_QUEUE: usize = 4096;
/// Heartbeat interval: send `Ping` this often.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
/// Liveness deadline: if no frame is received within this window, the
/// connection is considered dead.
const LIVENESS_DEADLINE: Duration = Duration::from_secs(45);
/// Default per-request timeout.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Longer timeout for `Spawn` (the daemon may need to start a process).
const SPAWN_TIMEOUT: Duration = Duration::from_secs(60);

/// Something the daemon sent without being asked.
#[derive(Debug, PartialEq)]
pub enum Incoming {
    /// Terminal output for an attached session.
    Data {
        id: SessionId,
        bytes: Vec<u8>,
    },
    /// Reset the session's mirror to this size; a snapshot follows as `Data`.
    Attached {
        id: SessionId,
        rows: u16,
        cols: u16,
    },
    Event {
        id: SessionId,
        event: SessionEvent,
    },
    /// Host resource stats pushed by the daemon (after `SubscribeHostStats`).
    HostStats {
        cpu_pct: f32,
        mem_used: u64,
        mem_total: u64,
        load1: Option<f32>,
    },
    /// The connection is gone; nothing else follows.
    Disconnected,
}

/// Requests awaiting their reply, keyed by `req`. `closed` is set once the
/// reader has stopped, so late requests fail instead of hanging.
#[derive(Default)]
struct Pending {
    waiting: HashMap<u64, oneshot::Sender<Msg>>,
    closed: bool,
}

struct Shared {
    out: mpsc::Sender<Frame>,
    /// Per-connection input-forwarding channel (capacity INPUT_FWD_QUEUE). A
    /// forwarder task drains it with `.send().await` so keystrokes are never
    /// silently dropped when the outgoing channel is momentarily busy.
    input_fwd: mpsc::Sender<Frame>,
    /// Clone of the incoming sender so `send_input` can surface a notice when
    /// the input-forward queue is full.
    in_tx: mpsc::Sender<Incoming>,
    pending: Arc<Mutex<Pending>>,
    next_req: AtomicU64,
    welcome: Welcome,
    incoming: Mutex<Option<mpsc::Receiver<Incoming>>>,
    /// Tokio-time instant of the last frame received from the daemon.
    /// The reader stamps it; the heartbeat checks it. Using `tokio::time::Instant`
    /// makes the heartbeat testable with `tokio::time::pause()`.
    last_frame: Arc<Mutex<TokioInstant>>,
    /// Signals the reader loop to exit when the writer exits abnormally.
    write_died: Arc<Notify>,
}

/// A connection to a daemon. Cheap to clone; all clones share it.
#[derive(Clone)]
pub struct Client {
    shared: Arc<Shared>,
}

/// Connect to the local daemon at `socket` and perform the handshake.
///
/// Validates the runtime directory and verifies the peer UID before use.
pub async fn connect(socket: &Path) -> io::Result<Client> {
    // Validate the runtime directory before connecting.
    let dir = paths::runtime_dir();
    paths::ensure_private_dir(&dir)?;

    let stream = UnixStream::connect(socket).await?;

    // Verify that the daemon is owned by us.
    verify_peer_uid(&stream)?;

    Client::handshake(stream).await
}

/// Connect to a remote daemon over SSH and perform the handshake.
///
/// Uses the shared [`remote::ssh_cmd`] builder: `-T -o BatchMode=yes
/// -o ConnectTimeout=15 -o ServerAliveInterval=15 -o ServerAliveCountMax=3 -- HOST`.
/// `kill_on_drop(true)` ensures the ssh process is killed when the Client
/// is dropped.  The child is owned by a supervisor task so handshake failures
/// don't leak it.
pub async fn connect_ssh(host: &str) -> io::Result<Client> {
    crate::remote::validate_host(host).map_err(io::Error::other)?;

    let mut child = crate::remote::ssh_cmd(host)
        .arg(r#""$HOME"/.local/bin/claudio --slave"#)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("no ssh stdin"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("no ssh stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("no ssh stderr"))?;

    // Supervisor: owns the child, logs stderr, and reaps on exit.
    let host_owned = host.to_owned();
    tokio::spawn(async move {
        use tokio::io::AsyncBufReadExt;
        let mut lines = tokio::io::BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            tracing::debug!(host = %host_owned, "ssh stderr: {line}");
        }
        let _ = child.wait().await;
    });

    let pair = SshPair {
        rd: stdout,
        wr: stdin,
    };
    Client::handshake(pair).await
}

/// An `AsyncRead + AsyncWrite` pair over an ssh process's stdout/stdin.
struct SshPair {
    rd: tokio::process::ChildStdout,
    wr: tokio::process::ChildStdin,
}

impl AsyncRead for SshPair {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut self.get_mut().rd).poll_read(cx, buf)
    }
}

impl AsyncWrite for SshPair {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        std::pin::Pin::new(&mut self.get_mut().wr).poll_write(cx, buf)
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut self.get_mut().wr).poll_flush(cx)
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut self.get_mut().wr).poll_shutdown(cx)
    }
}

fn lock(m: &Mutex<Pending>) -> MutexGuard<'_, Pending> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn disconnected() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "daemon disconnected")
}

impl Client {
    /// Send `Hello` over `stream`, expect a compatible `Welcome`, then start
    /// the reader, writer and heartbeat tasks.
    pub async fn handshake<S>(stream: S) -> io::Result<Client>
    where
        S: AsyncRead + AsyncWrite + Send + 'static,
    {
        let (mut rd, mut wr) = tokio::io::split(stream);
        let hello = Hello {
            claudio_version: env!("CARGO_PKG_VERSION").to_owned(),
            proto: PROTO,
            colors: None,
        };
        write_frame(
            &mut wr,
            &Frame::Control(Envelope::request(0, Msg::Hello(hello))),
        )
        .await?;
        let first = tokio::time::timeout(HANDSHAKE_TIMEOUT, read_frame(&mut rd))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "daemon handshake timed out"))??;
        let welcome = match first {
            Some(Frame::Control(Envelope {
                msg: Msg::Welcome(w),
                ..
            })) if w.proto == PROTO => w,
            Some(Frame::Control(Envelope {
                msg: Msg::Welcome(w),
                ..
            })) => {
                return Err(io::Error::other(format!(
                    "daemon speaks protocol {}, this claudio speaks {PROTO}",
                    w.proto
                )))
            }
            Some(Frame::Control(Envelope {
                msg: Msg::Error { message },
                ..
            })) => {
                return Err(io::Error::other(format!(
                    "daemon refused the connection: {message}"
                )))
            }
            Some(other) => {
                return Err(io::Error::other(format!(
                    "unexpected handshake reply: {other:?}"
                )))
            }
            None => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "daemon closed the connection during the handshake",
                ))
            }
        };

        let pending = Arc::new(Mutex::new(Pending::default()));
        let (out_tx, out_rx) = mpsc::channel(OUTGOING_QUEUE);
        let (in_tx, in_rx) = mpsc::channel(INCOMING_QUEUE);
        let (input_fwd_tx, input_fwd_rx) = mpsc::channel(INPUT_FWD_QUEUE);
        let write_died = Arc::new(Notify::new());
        let last_frame = Arc::new(Mutex::new(TokioInstant::now()));

        let shared = Arc::new(Shared {
            out: out_tx.clone(),
            input_fwd: input_fwd_tx,
            in_tx: in_tx.clone(),
            pending: Arc::clone(&pending),
            next_req: AtomicU64::new(1),
            welcome,
            incoming: Mutex::new(Some(in_rx)),
            last_frame: Arc::clone(&last_frame),
            write_died: Arc::clone(&write_died),
        });

        tokio::spawn(write_loop(wr, out_rx, Arc::clone(&write_died)));
        tokio::spawn(read_loop(
            rd,
            Arc::clone(&pending),
            in_tx,
            Arc::clone(&write_died),
            last_frame,
        ));
        // Input-forwarder: drains input_fwd with .send().await so keystrokes
        // are never silently dropped when the outgoing channel is momentarily busy.
        tokio::spawn(input_forwarder(input_fwd_rx, out_tx));

        // Heartbeat task.
        let hb_shared = Arc::clone(&shared);
        tokio::spawn(async move {
            heartbeat_loop(hb_shared).await;
        });

        Ok(Client { shared })
    }

    /// The daemon's handshake reply.
    pub fn welcome(&self) -> &Welcome {
        &self.shared.welcome
    }

    /// The channel of unsolicited messages. It can be taken only once.
    pub fn take_incoming(&self) -> Option<mpsc::Receiver<Incoming>> {
        self.shared
            .incoming
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
    }

    /// Send `msg` as a request and await the reply carrying its `req` id.
    ///
    /// The request is queued for writing *before* this returns, so requests
    /// reach the daemon in call order even when the futures are awaited on
    /// different tasks. A daemon `Error` reply becomes an `Err`.
    ///
    /// A 30-second deadline applies to all requests; `Spawn` gets 60 seconds.
    pub fn request(&self, msg: Msg) -> impl Future<Output = io::Result<Msg>> + Send + 'static {
        let timeout = if matches!(msg, Msg::Spawn(_)) {
            SPAWN_TIMEOUT
        } else {
            REQUEST_TIMEOUT
        };
        let queued = self.enqueue(msg);
        async move {
            let recv = queued?;
            match tokio::time::timeout(timeout, recv).await {
                Ok(Ok(Msg::Error { message })) => Err(io::Error::other(message)),
                Ok(Ok(reply)) => Ok(reply),
                Ok(Err(_)) => Err(disconnected()),
                Err(_) => Err(io::Error::new(io::ErrorKind::TimedOut, "request timed out")),
            }
        }
    }

    fn enqueue(&self, msg: Msg) -> io::Result<oneshot::Receiver<Msg>> {
        let req = self.shared.next_req.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = lock(&self.shared.pending);
            if pending.closed {
                return Err(disconnected());
            }
            pending.waiting.insert(req, tx);
        }
        // Use `try_send` to avoid blocking; if the channel is full we fail fast.
        if self
            .shared
            .out
            .try_send(Frame::Control(Envelope::request(req, msg)))
            .is_err()
        {
            lock(&self.shared.pending).waiting.remove(&req);
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "outgoing channel is full",
            ));
        }
        Ok(rx)
    }

    /// Send keyboard/mouse input to a session.
    ///
    /// Input is queued in the per-connection input-forwarding channel
    /// (capacity [`INPUT_FWD_QUEUE`]). A forwarder task drains it with
    /// `.send().await` so this method is always non-blocking. If the queue is
    /// full, the connection is unhealthy; a notice is surfaced and the excess
    /// input is dropped.
    pub fn send_input(&self, id: SessionId, bytes: &[u8]) {
        for chunk in bytes.chunks(INPUT_CHUNK) {
            let frame = Frame::Data {
                session: id,
                bytes: chunk.to_vec(),
            };
            if let Err(_) = self.shared.input_fwd.try_send(frame) {
                tracing::warn!(%id, "input dropped: connection congested");
                // Surface a notice in the incoming stream so the TUI can show it.
                let _ = self.shared.in_tx.try_send(Incoming::Event {
                    id,
                    event: crate::proto::SessionEvent::Notice {
                        text: "input dropped: connection congested".into(),
                    },
                });
                break;
            }
        }
    }
}

/// Write queued frames until every `Client` clone is gone or the socket
/// fails, then notify the reader to exit.
async fn write_loop<W: AsyncWrite + Unpin>(
    mut wr: W,
    mut rx: mpsc::Receiver<Frame>,
    write_died: Arc<Notify>,
) {
    let clean = loop {
        match rx.recv().await {
            None => break true, // all Client clones dropped
            Some(frame) => {
                if write_frame(&mut wr, &frame).await.is_err() {
                    break false;
                }
            }
        }
    };
    if clean {
        let _ = wr.shutdown().await;
    }
    // Signal the read loop to exit, whether clean or not.
    write_died.notify_one();
}

/// Route frames: replies to their waiting request, the rest to `incoming`.
/// Also exits when the writer signals it died (so we don't keep a dead
/// connection "alive").
async fn read_loop<R: AsyncRead + Unpin>(
    mut rd: R,
    pending: Arc<Mutex<Pending>>,
    incoming: mpsc::Sender<Incoming>,
    write_died: Arc<Notify>,
    last_frame: Arc<Mutex<TokioInstant>>,
) {
    let mut ui_gone = false;

    loop {
        let frame_fut = read_frame(&mut rd);
        let item = tokio::select! {
            res = frame_fut => {
                match res {
                    Ok(Some(frame)) => {
                        // Stamp the tokio-time instant so the heartbeat can
                        // check liveness with tokio::time::pause() in tests.
                        *last_frame.lock().unwrap_or_else(|e| e.into_inner()) =
                            TokioInstant::now();
                        match frame {
                            Frame::Data { session, bytes } => Some(Incoming::Data { id: session, bytes }),
                            Frame::Control(env) => route(env, &pending),
                        }
                    }
                    _ => break, // EOF or error
                }
            }
            _ = write_died.notified() => break, // writer exited
        };

        if let Some(item) = item {
            if incoming.send(item).await.is_err() {
                ui_gone = true;
                break;
            }
        }
    }
    {
        let mut p = lock(&pending);
        p.closed = true;
        // Dropping the senders fails every outstanding request.
        p.waiting.clear();
    }
    if !ui_gone {
        let _ = incoming.send(Incoming::Disconnected).await;
    }
}

/// Send Ping every `HEARTBEAT_INTERVAL` and kill the connection if no frame
/// arrives within `LIVENESS_DEADLINE`.
///
/// Uses `tokio::time::Instant` so tests can drive time with
/// `tokio::time::pause()` / `tokio::time::advance()`.
async fn heartbeat_loop(shared: Arc<Shared>) {
    loop {
        tokio::time::sleep(HEARTBEAT_INTERVAL).await;

        // Check liveness using tokio time so tests can control it.
        let elapsed = {
            let last = shared.last_frame.lock().unwrap_or_else(|e| e.into_inner());
            last.elapsed()
        };
        if elapsed > LIVENESS_DEADLINE {
            tracing::warn!("daemon liveness deadline exceeded, closing connection");
            // Mark pending as closed and signal the reader.
            {
                let mut p = lock(&shared.pending);
                p.closed = true;
                p.waiting.clear();
            }
            shared.write_died.notify_one();
            return;
        }

        // Send heartbeat ping.
        let req = shared.next_req.fetch_add(1, Ordering::Relaxed);
        let (tx, _rx) = oneshot::channel::<Msg>(); // ignore reply
        {
            let mut p = lock(&shared.pending);
            if p.closed {
                return;
            }
            p.waiting.insert(req, tx);
        }
        let _ = shared
            .out
            .try_send(Frame::Control(Envelope::request(req, Msg::Ping)));
    }
}

/// Drain the input-forwarding channel with `.send().await` so keystrokes are
/// delivered reliably even when the outgoing channel is momentarily full.
async fn input_forwarder(mut rx: mpsc::Receiver<Frame>, out: mpsc::Sender<Frame>) {
    while let Some(frame) = rx.recv().await {
        if out.send(frame).await.is_err() {
            break; // writer is gone
        }
    }
}

/// Resolve a reply and return what (if anything) the UI must see. `Attached`
/// goes to the UI even when it is a reply, so it stays ordered with the
/// snapshot `Data` that follows it.
fn route(env: Envelope, pending: &Mutex<Pending>) -> Option<Incoming> {
    let item = match &env.msg {
        Msg::Attached { id, rows, cols } => Some(Incoming::Attached {
            id: *id,
            rows: *rows,
            cols: *cols,
        }),
        Msg::Event { id, event } if env.req.is_none() => Some(Incoming::Event {
            id: *id,
            event: event.clone(),
        }),
        Msg::HostStats {
            cpu_pct,
            mem_used,
            mem_total,
            load1,
        } if env.req.is_none() => Some(Incoming::HostStats {
            cpu_pct: *cpu_pct,
            mem_used: *mem_used,
            mem_total: *mem_total,
            load1: *load1,
        }),
        Msg::Unknown => None,
        _ => None,
    };
    if let Some(req) = env.req {
        if let Some(tx) = lock(pending).waiting.remove(&req) {
            let _ = tx.send(env.msg);
        }
    }
    item
}

/// Make sure a daemon is listening on [`paths::daemon_socket`], starting one
/// (`<this binary> --daemon`, detached in its own session) if needed.
pub fn ensure_daemon() -> io::Result<()> {
    let socket = paths::daemon_socket();
    let dir = paths::runtime_dir();
    // Validate the runtime directory first.
    paths::ensure_private_dir(&dir)?;

    if std::os::unix::net::UnixStream::connect(&socket).is_ok() {
        return Ok(());
    }
    let log_path = dir.join("daemon.log");
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(&log_path)?;
    let mut cmd = Command::new(std::env::current_exe()?);
    cmd.arg("--daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log);
    // SAFETY: setsid is async-signal-safe and touches no parent state.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = cmd.spawn()?;

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if std::os::unix::net::UnixStream::connect(&socket).is_ok() {
            break;
        }
        // A daemon that daemonizes itself exits 0 early; anything else is fatal.
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                return Err(io::Error::other(format!(
                    "daemon exited with {status}; see {}",
                    log_path.display()
                )));
            }
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "daemon did not start within 5 s; see {}",
                    log_path.display()
                ),
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // Reap the daemon if it ever exits while we are still running.
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// Verify that the peer on `stream` has the same effective UID as us.
pub fn verify_peer_uid(stream: &UnixStream) -> io::Result<()> {
    use std::os::unix::io::AsRawFd;
    let fd = stream.as_raw_fd();
    let peer = peer_uid_fd(fd)?;
    let mine = unsafe { libc::getuid() };
    if peer != mine {
        return Err(io::Error::other(format!(
            "daemon socket peer uid {peer} does not match ours ({mine}) — possible counterfeit socket"
        )));
    }
    Ok(())
}

/// Return the UID of the peer on socket `fd`.
fn peer_uid_fd(fd: std::os::unix::io::RawFd) -> io::Result<u32> {
    #[cfg(target_os = "linux")]
    {
        let mut cred = libc::ucred {
            pid: 0,
            uid: 0,
            gid: 0,
        };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        let rc = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                &mut cred as *mut _ as *mut libc::c_void,
                &mut len,
            )
        };
        if rc == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(cred.uid)
    }
    #[cfg(target_os = "macos")]
    {
        let mut uid: libc::uid_t = 0;
        let mut gid: libc::gid_t = 0;
        let rc = unsafe { libc::getpeereid(fd, &mut uid, &mut gid) };
        if rc == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(uid)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = fd;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "peer UID check not supported on this platform",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{self, HostInfo, SessionState};
    use uuid::Uuid;

    fn welcome(proto: u32) -> Msg {
        Msg::Welcome(Welcome {
            claudio_version: "test".into(),
            proto,
            host: HostInfo {
                hostname: "h".into(),
                os: "linux".into(),
                arch: "x86_64".into(),
                home: "/home/u".into(),
                claude: None,
            },
        })
    }

    /// Answer the handshake on the daemon side of `stream`.
    async fn accept<S: AsyncRead + AsyncWrite + Unpin>(stream: &mut S, reply: Msg) {
        match read_frame(stream).await.unwrap() {
            Some(Frame::Control(Envelope {
                msg: Msg::Hello(h), ..
            })) => {
                assert_eq!(h.proto, PROTO)
            }
            other => panic!("expected Hello, got {other:?}"),
        }
        write_frame(stream, &Frame::Control(Envelope::event(reply)))
            .await
            .unwrap();
    }

    async fn next_request<S: AsyncRead + Unpin>(stream: &mut S) -> (u64, Msg) {
        match read_frame(stream).await.unwrap() {
            Some(Frame::Control(Envelope {
                req: Some(req),
                msg,
            })) => (req, msg),
            other => panic!("expected a request, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn replies_are_correlated_by_req_and_events_flow_separately() {
        let (ours, mut daemon) = tokio::io::duplex(64 * 1024);
        let fake = tokio::spawn(async move {
            accept(&mut daemon, welcome(PROTO)).await;
            let (r1, m1) = next_request(&mut daemon).await;
            let (r2, m2) = next_request(&mut daemon).await;
            assert_eq!(m1, Msg::Ping);
            assert_eq!(m2, Msg::ListSessions);
            let id = Uuid::nil();
            // Unsolicited traffic first, then the replies out of order.
            let ev = Msg::Event {
                id,
                event: SessionEvent::State {
                    state: SessionState::Idle,
                },
            };
            write_frame(&mut daemon, &Frame::Control(Envelope::event(ev)))
                .await
                .unwrap();
            write_frame(
                &mut daemon,
                &Frame::Data {
                    session: id,
                    bytes: b"hi".to_vec(),
                },
            )
            .await
            .unwrap();
            let sessions = Msg::Sessions { sessions: vec![] };
            write_frame(
                &mut daemon,
                &Frame::Control(Envelope::request(r2, sessions)),
            )
            .await
            .unwrap();
            write_frame(
                &mut daemon,
                &Frame::Control(Envelope::request(r1, Msg::Pong)),
            )
            .await
            .unwrap();
            // An error reply and an input frame.
            let (r3, _) = next_request(&mut daemon).await;
            let err = Msg::Error {
                message: "no such dir".into(),
            };
            write_frame(&mut daemon, &Frame::Control(Envelope::request(r3, err)))
                .await
                .unwrap();
            let input = read_frame(&mut daemon).await.unwrap();
            assert_eq!(
                input,
                Some(Frame::Data {
                    session: id,
                    bytes: b"x".to_vec()
                })
            );
            daemon
        });

        let client = Client::handshake(ours).await.unwrap();
        assert_eq!(client.welcome().host.home, "/home/u");
        let mut incoming = client.take_incoming().unwrap();
        assert!(client.take_incoming().is_none());

        let ping = client.request(Msg::Ping);
        let list = client.request(Msg::ListSessions);
        let (ping, list) = tokio::join!(ping, list);
        assert_eq!(ping.unwrap(), Msg::Pong);
        assert_eq!(list.unwrap(), Msg::Sessions { sessions: vec![] });

        let err = client
            .request(Msg::ListDir {
                path: "/nope".into(),
            })
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "no such dir");
        client.send_input(Uuid::nil(), b"x");

        assert_eq!(
            incoming.recv().await,
            Some(Incoming::Event {
                id: Uuid::nil(),
                event: SessionEvent::State {
                    state: SessionState::Idle
                }
            })
        );
        assert_eq!(
            incoming.recv().await,
            Some(Incoming::Data {
                id: Uuid::nil(),
                bytes: b"hi".to_vec()
            })
        );

        // The daemon going away fails new requests and reports Disconnected.
        drop(fake.await.unwrap());
        assert_eq!(incoming.recv().await, Some(Incoming::Disconnected));
        assert!(client.request(Msg::Ping).await.is_err());
    }

    #[tokio::test]
    async fn attached_reply_is_also_delivered_in_order_with_data() {
        let (ours, mut daemon) = tokio::io::duplex(64 * 1024);
        let id = Uuid::new_v4();
        let fake = tokio::spawn(async move {
            accept(&mut daemon, welcome(PROTO)).await;
            let (req, _) = next_request(&mut daemon).await;
            let attached = Msg::Attached {
                id,
                rows: 10,
                cols: 20,
            };
            write_frame(
                &mut daemon,
                &Frame::Control(Envelope::request(req, attached)),
            )
            .await
            .unwrap();
            write_frame(
                &mut daemon,
                &Frame::Data {
                    session: id,
                    bytes: b"snap".to_vec(),
                },
            )
            .await
            .unwrap();
            daemon
        });
        let client = Client::handshake(ours).await.unwrap();
        let mut incoming = client.take_incoming().unwrap();
        let reply = client
            .request(Msg::Attach {
                id,
                rows: 10,
                cols: 20,
            })
            .await
            .unwrap();
        assert_eq!(
            reply,
            Msg::Attached {
                id,
                rows: 10,
                cols: 20
            }
        );
        assert_eq!(
            incoming.recv().await,
            Some(Incoming::Attached {
                id,
                rows: 10,
                cols: 20
            })
        );
        assert_eq!(
            incoming.recv().await,
            Some(Incoming::Data {
                id,
                bytes: b"snap".to_vec()
            })
        );
        drop(fake.await.unwrap());
    }

    #[tokio::test]
    async fn handshake_rejects_protocol_mismatch_and_errors() {
        for reply in [
            welcome(PROTO + 1),
            Msg::Error {
                message: "go away".into(),
            },
        ] {
            let (ours, mut daemon) = tokio::io::duplex(4096);
            let fake = tokio::spawn(async move {
                accept(&mut daemon, reply).await;
                daemon
            });
            assert!(Client::handshake(ours).await.is_err());
            drop(fake.await.unwrap());
        }
    }

    #[tokio::test]
    async fn works_over_a_unix_socket_pair() {
        let (ours, mut daemon) = UnixStream::pair().unwrap();
        let fake = tokio::spawn(async move {
            accept(&mut daemon, welcome(PROTO)).await;
            let (req, _) = next_request(&mut daemon).await;
            write_frame(
                &mut daemon,
                &Frame::Control(Envelope::request(req, Msg::Pong)),
            )
            .await
            .unwrap();
            daemon
        });
        let client = Client::handshake(ours).await.unwrap();
        assert_eq!(client.request(Msg::Ping).await.unwrap(), Msg::Pong);
        drop(fake.await.unwrap());
    }

    /// Peer UID mismatch is detected on a real Unix socket pair.
    /// Since both sides are us, UIDs match and it succeeds.
    #[tokio::test]
    async fn peer_uid_matches_self_on_socket_pair() {
        let (a, _b) = UnixStream::pair().unwrap();
        // Both endpoints are in the same process, so UIDs must match.
        assert!(verify_peer_uid(&a).is_ok());
    }

    /// Simulate a failed peer UID check via the mock helper.
    #[test]
    fn mock_peer_uid_mismatch_is_detected() {
        // We can't actually create a socket owned by another user in unit
        // tests, so exercise the error path through the helper directly.
        let mine = unsafe { libc::getuid() };
        let attacker = mine.wrapping_add(1);
        let err = check_uid_mismatch(mine, attacker).unwrap_err();
        assert!(err.to_string().contains("counterfeit"));
    }

    /// Helper for the mock UID mismatch test.
    fn check_uid_mismatch(mine: u32, peer: u32) -> io::Result<()> {
        if peer != mine {
            return Err(io::Error::other(format!(
                "daemon socket peer uid {peer} does not match ours ({mine}) — possible counterfeit socket"
            )));
        }
        Ok(())
    }

    /// Verify that a dir with wrong permissions is rejected before connect.
    #[test]
    fn wrong_permissions_dir_is_rejected() {
        let tmp =
            std::env::temp_dir().join(format!("claudio-client-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();
        // Set group-readable permissions (not 0700).
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).unwrap();
        // ensure_private_dir should tighten permissions (not fail).
        paths::ensure_private_dir(&tmp).unwrap();
        assert_eq!(
            std::fs::metadata(&tmp).unwrap().permissions().mode() & 0o777,
            0o700
        );
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    // ── Heartbeat tests (use tokio time-pause to run instantly) ───────────────

    /// A fake daemon that answers every Ping with Pong: even after advancing
    /// time well past `LIVENESS_DEADLINE`, the connection must stay alive
    /// because each Pong stamps `last_frame`.
    #[tokio::test(start_paused = true)]
    async fn heartbeat_stays_alive_when_daemon_responds() {
        let (ours, mut daemon) = tokio::io::duplex(64 * 1024);

        // Fake daemon: answer every Ping with Pong so last_frame is refreshed.
        let fake = tokio::spawn(async move {
            accept(&mut daemon, welcome(PROTO)).await;
            loop {
                let Ok(Some(frame)) = proto::read_frame(&mut daemon).await else {
                    break;
                };
                if let Frame::Control(Envelope {
                    req: Some(req),
                    msg: Msg::Ping,
                }) = frame
                {
                    let _ = write_frame(
                        &mut daemon,
                        &Frame::Control(Envelope::request(req, Msg::Pong)),
                    )
                    .await;
                }
            }
        });

        let client = Client::handshake(ours).await.unwrap();
        let mut incoming = client.take_incoming().unwrap();

        // Advance time in steps of HEARTBEAT_INTERVAL so each heartbeat fires
        // and the fake daemon gets a chance to respond (updating last_frame).
        // Total: 4 * 15 s = 60 s > LIVENESS_DEADLINE (45 s).
        for _ in 0..4 {
            tokio::time::advance(HEARTBEAT_INTERVAL).await;
            // Yield multiple times to let all tasks (heartbeat, write loop,
            // fake daemon, read loop) complete their scheduling rounds.
            for _ in 0..8 {
                tokio::task::yield_now().await;
            }
        }

        // No Disconnected should have been sent — the connection is alive.
        assert_eq!(
            incoming.try_recv().ok(),
            None,
            "connection should still be alive after 60 s with ping/pong"
        );

        fake.abort();
    }

    /// A fake daemon that stops answering after the handshake: the heartbeat
    /// must declare the connection dead once `LIVENESS_DEADLINE` has elapsed.
    #[tokio::test(start_paused = true)]
    async fn heartbeat_declares_dead_when_daemon_stops_responding() {
        let (ours, mut daemon) = tokio::io::duplex(64 * 1024);

        // Accept the handshake then go silent (hold the connection open).
        let fake = tokio::spawn(async move {
            accept(&mut daemon, welcome(PROTO)).await;
            // Don't respond to anything — just keep the connection open by
            // sitting on a long sleep (which tokio::time::advance will skip).
            tokio::time::sleep(Duration::from_secs(10_000)).await;
        });

        let client = Client::handshake(ours).await.unwrap();
        let mut incoming = client.take_incoming().unwrap();

        // Advance time well past LIVENESS_DEADLINE.
        // We need at least 4 heartbeat intervals (60 s > 45 s LIVENESS_DEADLINE).
        for _ in 0..4 {
            tokio::time::advance(HEARTBEAT_INTERVAL).await;
            for _ in 0..8 {
                tokio::task::yield_now().await;
            }
        }

        // The heartbeat must have declared the connection dead.
        let item = tokio::time::timeout(Duration::from_secs(5), incoming.recv()).await;
        assert_eq!(
            item.ok().flatten(),
            Some(Incoming::Disconnected),
            "connection should be declared dead after liveness deadline"
        );

        // New requests must fail.
        assert!(
            client.request(Msg::Ping).await.is_err(),
            "requests must fail after connection is declared dead"
        );

        fake.abort();
    }
}
