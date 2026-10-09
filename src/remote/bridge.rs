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
//! The bridge exits when **either** direction closes, not both.  This means
//! that when the local client closes its ssh connection (or the daemon socket
//! closes), the bridge exits promptly and does not wait for the other side.
//!
//! Daemon startup strategy:
//! - If `systemd-run` is available and `$XDG_RUNTIME_DIR` is set, we start
//!   the daemon as a transient user service so it survives ssh logout and
//!   logind's `KillUserProcesses`.  We **wait** for the launcher to exit
//!   (it is quick) and fall back to double-fork + setsid on failure.
//! - Otherwise we fall back to double-fork + setsid.

use std::io;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use tokio::net::UnixStream;

use crate::paths;
use crate::proto::PROTO;

/// How long we wait for the daemon socket to appear after launch.
const DAEMON_WAIT: Duration = Duration::from_secs(10);
/// Poll interval while waiting.
const POLL: Duration = Duration::from_millis(50);

/// Run the slave bridge. Errors are written to stderr; stdout stays clean.
pub fn run() -> std::process::ExitCode {
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
    // Validate the runtime directory before connecting.
    let dir = paths::runtime_dir();
    paths::ensure_private_dir(&dir)?;

    // Ensure the remote daemon is running.
    let socket = paths::daemon_socket();
    ensure_daemon_detached(&socket).await?;

    // Connect to the daemon socket.
    let daemon = UnixStream::connect(&socket).await.map_err(|e| {
        io::Error::new(e.kind(), format!("cannot connect to daemon socket {}: {e}", socket.display()))
    })?;

    // Verify that the daemon is owned by us (peer UID check).
    verify_peer_uid(&daemon)?;

    // Bridge stdin → daemon, daemon → stdout.
    // Use select! so that closing either direction exits the bridge promptly
    // (copy_bidirectional would wait for both directions to close).
    let (mut daemon_rd, mut daemon_wr) = daemon.into_split();
    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();

    let t_in = tokio::spawn(async move {
        tokio::io::copy(&mut stdin, &mut daemon_wr).await
    });
    let t_out = tokio::spawn(async move {
        tokio::io::copy(&mut daemon_rd, &mut stdout).await
    });

    // Exit as soon as either direction is done.
    let result = tokio::select! {
        r = t_in  => r.unwrap_or_else(|e| Err(io::Error::other(e))),
        r = t_out => r.unwrap_or_else(|e| Err(io::Error::other(e))),
    };

    match result {
        Ok(_) => Ok(()),
        Err(e)
            if e.kind() == io::ErrorKind::BrokenPipe
                || e.kind() == io::ErrorKind::UnexpectedEof =>
        {
            Ok(())
        }
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
    let log = std::fs::OpenOptions::new().create(true).append(true).open(&log_path)?;

    // Try systemd-run; fall back to setsid on failure.
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
                format!(
                    "daemon did not start within {}s; see {}",
                    DAEMON_WAIT.as_secs(),
                    log_path.display()
                ),
            ));
        }
    }
}

/// Start the daemon via `systemd-run --user` (preferred: survives logout).
///
/// Waits for the launcher to exit and returns `Err` if it fails (so the
/// caller can fall back to `spawn_setsid`).
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
    let mut child = std::process::Command::new("systemd-run")
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

    // Wait for the launcher to exit and check its status.
    let status = child.wait()?;
    if !status.success() {
        return Err(io::Error::other(format!(
            "systemd-run exited with {status}"
        )));
    }
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
        std::thread::spawn(move || {
            c.wait().ok();
        });
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

/// Verify that the peer on `stream` has the same effective UID as us.
/// Returns `Err` if the peer is a different user.
fn verify_peer_uid(stream: &UnixStream) -> io::Result<()> {
    use std::os::unix::io::AsRawFd;
    let fd = stream.as_raw_fd();
    let peer = peer_uid(fd)?;
    let mine = unsafe { libc::getuid() };
    if peer != mine {
        return Err(io::Error::other(format!(
            "daemon socket is owned by uid {peer}, expected {mine} — possible impersonation"
        )));
    }
    Ok(())
}

/// Return the effective UID of the process on the other end of `fd`.
fn peer_uid(fd: std::os::unix::io::RawFd) -> io::Result<u32> {
    #[cfg(target_os = "linux")]
    {
        let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
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
