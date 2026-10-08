//! `claudio --slave`: the remote end of an SSH session transport.
//!
//! This command is invoked by the local client as:
//! ```text
//! ssh HOST '$HOME/.local/bin/claudio --slave'
//! ```
//! It ensures the remote daemon is running, then bridges the ssh connection's
//! stdio ↔ the daemon's Unix socket. **stdout must stay clean** — it is the
//! binary protocol channel — so all logging goes to stderr (and optionally to
//! a log file).
//!
//! Daemon startup strategy:
//! - If `systemd-run` is available and `$XDG_RUNTIME_DIR` is set, we start
//!   the daemon as a transient user service so it survives ssh logout and
//!   logind's `KillUserProcesses`.
//! - Otherwise we fall back to double-fork + setsid.

use std::io;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use tokio::io::{copy_bidirectional, AsyncRead, AsyncWrite};
use tokio::net::UnixStream;

use crate::paths;
use crate::proto::PROTO;

/// How long we wait for the daemon socket to appear after launch.
const DAEMON_WAIT: Duration = Duration::from_secs(10);
/// Poll interval while waiting.
const POLL: Duration = Duration::from_millis(50);

/// Run the slave bridge. Errors are written to stderr; stdout stays clean.
pub fn run() -> std::process::ExitCode {
    // Set up stderr logging (no tracing macros – those write to stderr too,
    // but we want a simple prefix so log lines can be grepped).
    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("claudio --slave: runtime: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    match rt.block_on(async_run()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("claudio --slave: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn async_run() -> io::Result<()> {
    // Ensure the remote daemon is running.
    let socket = paths::daemon_socket();
    ensure_daemon_detached(&socket).await?;

    // Connect to the daemon socket.
    let mut daemon = UnixStream::connect(&socket).await.map_err(|e| {
        io::Error::new(e.kind(), format!("cannot connect to daemon socket {}: {e}", socket.display()))
    })?;

    // Bridge stdin → daemon, daemon → stdout.
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let mut stdio = StdioPair { rd: stdin, wr: stdout };

    match copy_bidirectional(&mut stdio, &mut daemon).await {
        Ok(_) => Ok(()),
        // EOF on either side is a clean disconnect.
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe
            || e.kind() == io::ErrorKind::UnexpectedEof => Ok(()),
        Err(e) => Err(e),
    }
}

/// Start the daemon if not already running, using the most persistent method
/// available.
async fn ensure_daemon_detached(socket: &std::path::Path) -> io::Result<()> {
    // Already running?
    if std::os::unix::net::UnixStream::connect(socket).is_ok() {
        return Ok(());
    }

    let exe = std::env::current_exe()?;
    let dir = paths::runtime_dir();
    paths::ensure_private_dir(&dir)?;
    let log_path = dir.join("daemon.log");
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;

    if try_systemd_run(&exe, log.try_clone()?).is_err() {
        spawn_setsid(&exe, log)?;
    }

    // Poll until the socket appears.
    let deadline = Instant::now() + DAEMON_WAIT;
    loop {
        tokio::time::sleep(POLL).await;
        if std::os::unix::net::UnixStream::connect(socket).is_ok() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("daemon did not start within {}s; see {}", DAEMON_WAIT.as_secs(), log_path.display()),
            ));
        }
    }
}

/// Start the daemon via `systemd-run --user` (preferred: survives logout).
fn try_systemd_run(exe: &PathBuf, log: std::fs::File) -> io::Result<()> {
    // Only try if systemd-run is on PATH and XDG_RUNTIME_DIR is set.
    if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "no XDG_RUNTIME_DIR"));
    }
    if !systemd_run_available() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "systemd-run not found"));
    }

    let unit = format!("claudio-daemon-v{PROTO}");
    let exe_str = exe.to_string_lossy();
    std::process::Command::new("systemd-run")
        .args([
            "--user",
            "--quiet",
            "--collect",
            &format!("--unit={unit}"),
            &exe_str,
            "--daemon",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log))
        .spawn()?;
    Ok(())
}

/// Fall back to double-fork + setsid.
fn spawn_setsid(exe: &PathBuf, log: std::fs::File) -> io::Result<()> {
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log));
    // SAFETY: setsid is async-signal-safe and touches no parent state.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    cmd.spawn().map(|mut c| {
        std::thread::spawn(move || { c.wait().ok(); });
    })
}

fn systemd_run_available() -> bool {
    std::process::Command::new("systemd-run")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// A thin wrapper that presents stdin + stdout as a single `AsyncRead + AsyncWrite`.
struct StdioPair<R, W> {
    rd: R,
    wr: W,
}

impl<R: AsyncRead + Unpin, W: Unpin> AsyncRead for StdioPair<R, W> {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut self.get_mut().rd).poll_read(cx, buf)
    }
}

impl<R: Unpin, W: AsyncWrite + Unpin> AsyncWrite for StdioPair<R, W> {
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
