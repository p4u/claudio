//! The TUI's connection to the local daemon, plus daemon autostart.
//!
//! A [`Client`] wraps one Unix-socket connection. A writer task owns the
//! write half and a reader task the read half. Replies are matched to
//! requests by their `req` id; everything unsolicited (terminal output,
//! `Attached`, session events) flows out through one ordered channel of
//! [`Incoming`] items.

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
use tokio::process::Command as TokioCommand;
use tokio::sync::{mpsc, oneshot};

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

/// Something the daemon sent without being asked.
#[derive(Debug, PartialEq)]
pub enum Incoming {
    /// Terminal output for an attached session.
    Data { id: SessionId, bytes: Vec<u8> },
    /// Reset the session's mirror to this size; a snapshot follows as `Data`.
    Attached { id: SessionId, rows: u16, cols: u16 },
    Event { id: SessionId, event: SessionEvent },
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
    out: mpsc::UnboundedSender<Frame>,
    pending: Arc<Mutex<Pending>>,
    next_req: AtomicU64,
    welcome: Welcome,
    incoming: Mutex<Option<mpsc::Receiver<Incoming>>>,
}

/// A connection to a daemon. Cheap to clone; all clones share it.
#[derive(Clone)]
pub struct Client {
    shared: Arc<Shared>,
}

/// Connect to the local daemon at `socket` and perform the handshake.
pub async fn connect(socket: &Path) -> io::Result<Client> {
    let stream = UnixStream::connect(socket).await?;
    Client::handshake(stream).await
}

/// Connect to a remote daemon over SSH and perform the handshake.
///
/// Spawns `ssh -T -o BatchMode=yes -o ServerAliveInterval=15
/// -o ServerAliveCountMax=3 HOST '$HOME/.local/bin/claudio --slave'` with
/// stdin/stdout piped. The child is killed when the [`Client`] is dropped
/// (via the writer-task's implicit `Arc` drop).
pub async fn connect_ssh(host: &str) -> io::Result<Client> {
    let mut child = TokioCommand::new("ssh")
        .args([
            "-T",
            "-o", "BatchMode=yes",
            "-o", "ServerAliveInterval=15",
            "-o", "ServerAliveCountMax=3",
            host,
            "$HOME/.local/bin/claudio --slave",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let stdin = child.stdin.take().ok_or_else(|| io::Error::other("no ssh stdin"))?;
    let stdout = child.stdout.take().ok_or_else(|| io::Error::other("no ssh stdout"))?;

    // Capture stderr into a small buffer for diagnostics and reap the child.
    let host_owned = host.to_owned();
    tokio::spawn(async move {
        use tokio::io::AsyncBufReadExt;
        let stderr = child.stderr.take().expect("stderr was piped");
        let mut lines = tokio::io::BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            tracing::debug!(host = %host_owned, "ssh stderr: {line}");
        }
        let _ = child.wait().await;
    });

    let pair = SshPair { rd: stdout, wr: stdin };
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
    /// the reader and writer tasks.
    pub async fn handshake<S>(stream: S) -> io::Result<Client>
    where
        S: AsyncRead + AsyncWrite + Send + 'static,
    {
        let (mut rd, mut wr) = tokio::io::split(stream);
        let hello = Hello { claudio_version: env!("CARGO_PKG_VERSION").to_owned(), proto: PROTO, colors: None };
        write_frame(&mut wr, &Frame::Control(Envelope::request(0, Msg::Hello(hello)))).await?;
        let first = tokio::time::timeout(HANDSHAKE_TIMEOUT, read_frame(&mut rd))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "daemon handshake timed out"))??;
        let welcome = match first {
            Some(Frame::Control(Envelope { msg: Msg::Welcome(w), .. })) if w.proto == PROTO => w,
            Some(Frame::Control(Envelope { msg: Msg::Welcome(w), .. })) => {
                return Err(io::Error::other(format!(
                    "daemon speaks protocol {}, this claudio speaks {PROTO}",
                    w.proto
                )))
            }
            Some(Frame::Control(Envelope { msg: Msg::Error { message }, .. })) => {
                return Err(io::Error::other(format!("daemon refused the connection: {message}")))
            }
            Some(other) => return Err(io::Error::other(format!("unexpected handshake reply: {other:?}"))),
            None => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "daemon closed the connection during the handshake",
                ))
            }
        };

        let pending = Arc::new(Mutex::new(Pending::default()));
        let (out_tx, out_rx) = mpsc::unbounded_channel();
        let (in_tx, in_rx) = mpsc::channel(INCOMING_QUEUE);
        tokio::spawn(write_loop(wr, out_rx));
        tokio::spawn(read_loop(rd, Arc::clone(&pending), in_tx));
        Ok(Client {
            shared: Arc::new(Shared {
                out: out_tx,
                pending,
                next_req: AtomicU64::new(1),
                welcome,
                incoming: Mutex::new(Some(in_rx)),
            }),
        })
    }

    /// The daemon's handshake reply.
    pub fn welcome(&self) -> &Welcome {
        &self.shared.welcome
    }

    /// The channel of unsolicited messages. It can be taken only once.
    pub fn take_incoming(&self) -> Option<mpsc::Receiver<Incoming>> {
        self.shared.incoming.lock().unwrap_or_else(|p| p.into_inner()).take()
    }

    /// Send `msg` as a request and await the reply carrying its `req` id.
    ///
    /// The request is queued for writing *before* this returns, so requests
    /// reach the daemon in call order even when the futures are awaited on
    /// different tasks. A daemon `Error` reply becomes an `Err`.
    pub fn request(&self, msg: Msg) -> impl Future<Output = io::Result<Msg>> + Send + 'static {
        let queued = self.enqueue(msg);
        async move {
            match queued?.await.map_err(|_| disconnected())? {
                Msg::Error { message } => Err(io::Error::other(message)),
                reply => Ok(reply),
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
        if self.shared.out.send(Frame::Control(Envelope::request(req, msg))).is_err() {
            lock(&self.shared.pending).waiting.remove(&req);
            return Err(disconnected());
        }
        Ok(rx)
    }

    /// Send keyboard/mouse input to a session (fire and forget).
    pub fn send_input(&self, id: SessionId, bytes: &[u8]) {
        for chunk in bytes.chunks(INPUT_CHUNK) {
            let _ = self.shared.out.send(Frame::Data { session: id, bytes: chunk.to_vec() });
        }
    }
}

/// Write queued frames until every `Client` clone is gone or the socket
/// fails, then shut the write side so the daemon sees EOF.
async fn write_loop<W: AsyncWrite + Unpin>(mut wr: W, mut rx: mpsc::UnboundedReceiver<Frame>) {
    while let Some(frame) = rx.recv().await {
        if write_frame(&mut wr, &frame).await.is_err() {
            return;
        }
    }
    let _ = wr.shutdown().await;
}

/// Route frames: replies to their waiting request, the rest to `incoming`.
async fn read_loop<R: AsyncRead + Unpin>(
    mut rd: R,
    pending: Arc<Mutex<Pending>>,
    incoming: mpsc::Sender<Incoming>,
) {
    let mut ui_gone = false;
    while let Ok(Some(frame)) = read_frame(&mut rd).await {
        let item = match frame {
            Frame::Data { session, bytes } => Some(Incoming::Data { id: session, bytes }),
            Frame::Control(env) => route(env, &pending),
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

/// Resolve a reply and return what (if anything) the UI must see. `Attached`
/// goes to the UI even when it is a reply, so it stays ordered with the
/// snapshot `Data` that follows it.
fn route(env: Envelope, pending: &Mutex<Pending>) -> Option<Incoming> {
    let item = match &env.msg {
        Msg::Attached { id, rows, cols } => Some(Incoming::Attached { id: *id, rows: *rows, cols: *cols }),
        Msg::Event { id, event } if env.req.is_none() => Some(Incoming::Event { id: *id, event: event.clone() }),
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
    if std::os::unix::net::UnixStream::connect(&socket).is_ok() {
        return Ok(());
    }
    let dir = paths::runtime_dir();
    paths::ensure_private_dir(&dir)?;
    let log_path = dir.join("daemon.log");
    let log = OpenOptions::new().create(true).append(true).mode(0o600).open(&log_path)?;
    let mut cmd = Command::new(std::env::current_exe()?);
    cmd.arg("--daemon").stdin(Stdio::null()).stdout(Stdio::null()).stderr(log);
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
                format!("daemon did not start within 5 s; see {}", log_path.display()),
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // Reap the daemon if it ever exits while we are still running.
    std::thread::spawn(move || child.wait());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{HostInfo, SessionState};
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
            Some(Frame::Control(Envelope { msg: Msg::Hello(h), .. })) => assert_eq!(h.proto, PROTO),
            other => panic!("expected Hello, got {other:?}"),
        }
        write_frame(stream, &Frame::Control(Envelope::event(reply))).await.unwrap();
    }

    async fn next_request<S: AsyncRead + Unpin>(stream: &mut S) -> (u64, Msg) {
        match read_frame(stream).await.unwrap() {
            Some(Frame::Control(Envelope { req: Some(req), msg })) => (req, msg),
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
            let ev = Msg::Event { id, event: SessionEvent::State { state: SessionState::Idle } };
            write_frame(&mut daemon, &Frame::Control(Envelope::event(ev))).await.unwrap();
            write_frame(&mut daemon, &Frame::Data { session: id, bytes: b"hi".to_vec() }).await.unwrap();
            let sessions = Msg::Sessions { sessions: vec![] };
            write_frame(&mut daemon, &Frame::Control(Envelope::request(r2, sessions))).await.unwrap();
            write_frame(&mut daemon, &Frame::Control(Envelope::request(r1, Msg::Pong))).await.unwrap();
            // An error reply and an input frame.
            let (r3, _) = next_request(&mut daemon).await;
            let err = Msg::Error { message: "no such dir".into() };
            write_frame(&mut daemon, &Frame::Control(Envelope::request(r3, err))).await.unwrap();
            let input = read_frame(&mut daemon).await.unwrap();
            assert_eq!(input, Some(Frame::Data { session: id, bytes: b"x".to_vec() }));
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

        let err = client.request(Msg::ListDir { path: "/nope".into() }).await.unwrap_err();
        assert_eq!(err.to_string(), "no such dir");
        client.send_input(Uuid::nil(), b"x");

        assert_eq!(
            incoming.recv().await,
            Some(Incoming::Event { id: Uuid::nil(), event: SessionEvent::State { state: SessionState::Idle } })
        );
        assert_eq!(incoming.recv().await, Some(Incoming::Data { id: Uuid::nil(), bytes: b"hi".to_vec() }));

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
            let attached = Msg::Attached { id, rows: 10, cols: 20 };
            write_frame(&mut daemon, &Frame::Control(Envelope::request(req, attached))).await.unwrap();
            write_frame(&mut daemon, &Frame::Data { session: id, bytes: b"snap".to_vec() }).await.unwrap();
            daemon
        });
        let client = Client::handshake(ours).await.unwrap();
        let mut incoming = client.take_incoming().unwrap();
        let reply = client.request(Msg::Attach { id, rows: 10, cols: 20 }).await.unwrap();
        assert_eq!(reply, Msg::Attached { id, rows: 10, cols: 20 });
        assert_eq!(incoming.recv().await, Some(Incoming::Attached { id, rows: 10, cols: 20 }));
        assert_eq!(incoming.recv().await, Some(Incoming::Data { id, bytes: b"snap".to_vec() }));
        drop(fake.await.unwrap());
    }

    #[tokio::test]
    async fn handshake_rejects_protocol_mismatch_and_errors() {
        for reply in [welcome(PROTO + 1), Msg::Error { message: "go away".into() }] {
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
            write_frame(&mut daemon, &Frame::Control(Envelope::request(req, Msg::Pong))).await.unwrap();
            daemon
        });
        let client = Client::handshake(ours).await.unwrap();
        assert_eq!(client.request(Msg::Ping).await.unwrap(), Msg::Pong);
        drop(fake.await.unwrap());
    }
}
