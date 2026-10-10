//! Keeping a host's daemon at least as new as the claudio binary on it.
//!
//! A daemon outlives the binary it was started from. An upgrade (a local
//! install, or the SSH bootstrap uploading a newer `~/.local/bin/claudio`)
//! leaves the old daemon running old code, without whatever the new client
//! expects (host stats, terminals, the git viewer…). So a binary about to
//! talk to its host's daemon checks it first, with [`ensure_current`]:
//!
//! - the local TUI, before it connects ([`crate::client::ensure_local_daemon`]);
//! - the remote bridge (`claudio --slave`), before it relays. The decision is
//!   made on the remote host, against the bridge's own binary: the one the
//!   bootstrap just installed, whatever the remote's platform.
//!
//! An outdated daemon is replaced only when no one would notice ([`busy`]):
//! it is asked to shut down, the current binary starts a new one, and the
//! clients' usual reconnect recovery brings the sessions back from its
//! journal (claude with `--resume`; terminals as fresh shells). Otherwise the
//! restart is *deferred*: the old daemon keeps serving, the connection is
//! flagged ([`crate::client::Client::daemon_outdated`]; over SSH the bridge
//! says so on stderr with [`DEFERRED`]), and the TUI tries again every couple
//! of minutes while that host's sessions are idle, and on every reconnect.
//! Silent either way.
//!
//! "Outdated" is "older", not just "different" ([`compare`]): two builds
//! (an installed release and a dev build, or two machines bootstrapping one
//! host) would otherwise replace each other's daemon forever. Daemons from
//! before this check report no build and are always outdated.

use std::fs::{File, TryLockError};
use std::future::Future;
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::client;
use crate::proto::{Msg, SessionInfo, SessionKind, SessionState, Welcome};
use crate::remote::probe::Probe;
use crate::upgrade::SemVer;

/// What the bridge prints on stderr when it left an outdated daemon running.
pub const DEFERRED: &str = "daemon-update-deferred";

/// How long an old daemon may take to exit after `Shutdown`.
const EXIT_WAIT: Duration = Duration::from_secs(10);

/// How a daemon's binary relates to ours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    /// The same build.
    Current,
    /// A different build, but not an older one: leave it be.
    Newer,
    /// Older than ours (or too old to say): replace it.
    Outdated,
}

/// The outcome of [`ensure_current`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonCheck {
    /// The daemon runs this build, or a newer one.
    UpToDate,
    /// It was outdated and has been replaced.
    Restarted,
    /// It is outdated, but busy: it was left running.
    Deferred,
}

/// Compare the daemon that sent `daemon` with this binary, `own`. A build is
/// older when its version is lower or, for one version, its file is older.
pub fn compare(daemon: &Welcome, own: &Probe) -> Freshness {
    match &daemon.build {
        Some(build) if *build == own.build => return Freshness::Current,
        Some(_) => {}
        None => return Freshness::Outdated,
    }
    let theirs = (
        SemVer::parse(&daemon.claudio_version),
        daemon.build_time.unwrap_or(0),
    );
    let ours = (SemVer::parse(&own.version), own.mtime.unwrap_or(0));
    if theirs < ours {
        Freshness::Outdated
    } else {
        Freshness::Newer
    }
}

/// Whether a daemon restart now would interrupt someone: a claude session
/// mid-turn or at a permission prompt, or any `--plain` session (never
/// journaled, so it would not come back). Idle claude sessions resume where
/// they were; terminals don't count, they come back as fresh shells.
pub fn busy(sessions: &[SessionInfo]) -> bool {
    sessions.iter().any(|s| {
        (s.ephemeral && s.state != SessionState::Exited)
            || (s.kind == SessionKind::Claude
                && matches!(s.state, SessionState::Working | SessionState::NeedsApproval))
    })
}

/// Make sure the daemon listening on `socket` (its startup lock at `lock`)
/// runs `own` or a newer build: an outdated one is shut down and `start`
/// launches ours, unless it is [`busy`]. The daemon must be running.
pub async fn ensure_current<F, Fut>(
    socket: &Path,
    lock: &Path,
    own: &Probe,
    start: F,
) -> io::Result<DaemonCheck>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = io::Result<()>>,
{
    let daemon = client::connect(socket).await?;
    if compare(daemon.welcome(), own) != Freshness::Outdated {
        return Ok(DaemonCheck::UpToDate);
    }
    let sessions = match daemon.request(Msg::ListSessions).await? {
        Msg::Sessions { sessions } => sessions,
        other => return Err(io::Error::other(format!("unexpected reply: {other:?}"))),
    };
    if busy(&sessions) {
        tracing::info!("the daemon is outdated but busy; leaving it running");
        return Ok(DaemonCheck::Deferred);
    }
    tracing::info!(
        old = %daemon.welcome().claudio_version,
        sessions = sessions.len(),
        "replacing the outdated daemon"
    );
    let asked = daemon.request(Msg::Shutdown).await;
    drop(daemon);
    if let Err(e) = asked {
        // A daemon older than `Shutdown` refuses it as an unknown op.
        tracing::info!(error = %e, "Shutdown refused; signalling the daemon");
        terminate_lock_holder(lock)?;
    }
    let deadline = Instant::now() + EXIT_WAIT;
    while !lock_free(lock) {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "the outdated daemon did not exit",
            ));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    start().await?;
    Ok(DaemonCheck::Restarted)
}

/// SIGTERM the daemon that wrote its PID into `lock`. It is live and ours:
/// it listens on our private socket and still holds the lock. On Linux it
/// must also be a `--daemon` process.
fn terminate_lock_holder(lock: &Path) -> io::Result<()> {
    let pid: libc::pid_t = std::fs::read_to_string(lock)?
        .trim()
        .parse()
        .ok()
        .filter(|pid| *pid > 1)
        .ok_or_else(|| io::Error::other("no daemon PID in the lock file"))?;
    #[cfg(target_os = "linux")]
    {
        let cmdline = std::fs::read(format!("/proc/{pid}/cmdline"))?;
        if !cmdline.split(|b| *b == 0).any(|arg| arg == b"--daemon") {
            return Err(io::Error::other(format!(
                "PID {pid} is not a claudio daemon"
            )));
        }
    }
    // SAFETY: kill(2) has no memory-safety preconditions.
    if unsafe { libc::kill(pid, libc::SIGTERM) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Whether no daemon holds the startup lock at `lock` (none runs, or the
/// last one has exited). An unreadable lock counts as free: starting a
/// daemon is then the way to find out.
pub fn lock_free(lock: &Path) -> bool {
    let Ok(file) = File::open(lock) else {
        return true;
    };
    // Taken and, with `file`, dropped again at once.
    !matches!(file.try_lock(), Err(TryLockError::WouldBlock))
}

/// Before starting a daemon: wait (up to `timeout`) while one holds the lock
/// at `lock` without listening on `socket` yet, i.e. is exiting or still
/// starting. A new one would find the lock taken and quit. Returns whether
/// a daemon is listening.
pub fn wait_for_lock(socket: &Path, lock: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if std::os::unix::net::UnixStream::connect(socket).is_ok() {
            return true;
        }
        if lock_free(lock) || Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::HostInfo;

    fn own() -> Probe {
        Probe {
            version: "0.3.0".into(),
            proto: 1,
            os: "linux".into(),
            arch: "x86_64".into(),
            build: "ours".into(),
            mtime: Some(1_000),
        }
    }

    fn welcome(version: &str, build: Option<&str>, build_time: Option<u64>) -> Welcome {
        Welcome {
            claudio_version: version.into(),
            proto: 1,
            host: HostInfo {
                hostname: "h".into(),
                os: "linux".into(),
                arch: "x86_64".into(),
                home: "/h".into(),
                claude: None,
            },
            build: build.map(Into::into),
            build_time,
        }
    }

    #[test]
    fn the_same_build_is_current_whatever_its_time() {
        let w = welcome("0.3.0", Some("ours"), Some(5));
        assert_eq!(compare(&w, &own()), Freshness::Current);
    }

    #[test]
    fn a_daemon_without_a_build_is_outdated() {
        // Daemons from before the check, whatever version they claim.
        let w = welcome("9.9.9", None, None);
        assert_eq!(compare(&w, &own()), Freshness::Outdated);
    }

    #[test]
    fn older_is_a_lower_version_or_an_older_file() {
        let o = own();
        let at = |v, t| compare(&welcome(v, Some("theirs"), Some(t)), &o);
        assert_eq!(at("0.2.9", 5_000), Freshness::Outdated);
        assert_eq!(at("0.3.0", 999), Freshness::Outdated);
        assert_eq!(at("0.3.0", 1_000), Freshness::Newer);
        assert_eq!(at("0.3.0", 2_000), Freshness::Newer);
        assert_eq!(at("0.4.0", 1), Freshness::Newer);
    }

    #[test]
    fn two_builds_never_both_think_the_other_outdated() {
        // Whichever one runs the daemon, the other leaves it alone, so two
        // binaries cannot replace each other's daemon back and forth.
        let a = own();
        let b = Probe {
            build: "other".into(),
            mtime: Some(2_000),
            ..own()
        };
        let runs = |p: &Probe| welcome(&p.version, Some(&p.build), p.mtime);
        assert_eq!(compare(&runs(&a), &b), Freshness::Outdated);
        assert_eq!(compare(&runs(&b), &a), Freshness::Newer);
    }

    fn session(kind: SessionKind, state: SessionState, ephemeral: bool) -> SessionInfo {
        SessionInfo {
            id: uuid::Uuid::new_v4(),
            cwd: "/w".into(),
            name: None,
            state,
            claude_session_id: None,
            title: None,
            pid: Some(1),
            created_at: 1,
            branch: None,
            model: None,
            context_tokens: None,
            kind,
            ephemeral,
        }
    }

    #[test]
    fn busy_means_a_claude_turn_or_prompt_or_a_plain_session() {
        use SessionKind::{Claude, Shell};
        use SessionState::*;
        let one = |kind, state, ephemeral| busy(&[session(kind, state, ephemeral)]);
        assert!(one(Claude, Working, false));
        assert!(one(Claude, NeedsApproval, false));
        for state in [Starting, NeedsInput, Idle, Error, Exited, Unknown] {
            assert!(!one(Claude, state, false), "{state:?}");
        }
        // A terminal comes back as a fresh shell, whatever it was doing.
        assert!(!one(Shell, Working, false));
        assert!(!one(Shell, Idle, false));
        // A live `--plain` session would be lost; a gone one would not.
        assert!(one(Claude, Idle, true));
        assert!(!one(Claude, Exited, true));
        assert!(!busy(&[]));
        assert!(busy(&[
            session(Shell, Idle, false),
            session(Claude, Working, false)
        ]));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn only_a_daemon_is_signalled() {
        let dir = std::env::temp_dir().join(format!("claudio-term-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let lock = dir.join("d.lock");
        // This test process is no `--daemon`: refused, and still alive.
        std::fs::write(&lock, format!("{}\n", std::process::id())).unwrap();
        assert!(terminate_lock_holder(&lock).is_err());
        for junk in ["", "1", "-5", "nope"] {
            std::fs::write(&lock, junk).unwrap();
            assert!(terminate_lock_holder(&lock).is_err(), "{junk:?}");
        }
        // A daemon is.
        let mut daemon = std::process::Command::new("sh")
            .args(["-c", "sleep 5; :", "sh", "--daemon"])
            .spawn()
            .unwrap();
        std::fs::write(&lock, format!("{}\n", daemon.id())).unwrap();
        // Until it has exec'd, its command line reads empty.
        let cmdline = format!("/proc/{}/cmdline", daemon.id());
        while std::fs::read(&cmdline).unwrap().is_empty() {
            std::thread::sleep(Duration::from_millis(10));
        }
        terminate_lock_holder(&lock).unwrap();
        let status = daemon.wait().unwrap();
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(libc::SIGTERM));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn lock_free_tells_a_held_lock() {
        let dir = std::env::temp_dir().join(format!("claudio-fresh-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let lock = dir.join("d.lock");
        assert!(lock_free(&lock), "a missing lock is free");
        let held = File::create(&lock).unwrap();
        held.try_lock().unwrap();
        assert!(!lock_free(&lock));
        let socket = dir.join("d.sock");
        let started = Instant::now();
        assert!(!wait_for_lock(&socket, &lock, Duration::from_millis(200)));
        assert!(started.elapsed() >= Duration::from_millis(200));
        drop(held);
        // A process another test forks meanwhile holds a copy of the lock's
        // descriptor until it execs: the lock frees a moment later.
        let deadline = Instant::now() + Duration::from_secs(2);
        while !lock_free(&lock) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(lock_free(&lock));
        assert!(!wait_for_lock(&socket, &lock, Duration::from_secs(5)));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
