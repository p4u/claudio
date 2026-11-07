//! `UpdateClaude`: run `claude update` (or the official installer when claude
//! is missing) on this host, then re-probe `claude --version`.
//!
//! The installer needs `curl` and `bash` on the daemon's `PATH` (Linux and
//! macOS have both as a rule); without one, or with `CLAUDIO_CLAUDE_PATH`
//! naming a path (the installer does not write there), or with a claude
//! already there that did not answer `--version`, the reply says why and
//! nothing runs. After an install, the plugin installs that failed are
//! tried again at once (`Manifests::forget_failures`).
//!
//! The command runs as a task of its own, never inline in a client loop (the
//! reply is queued like `Attach`'s), under a deadline and with its output
//! bounded to the last few KiB. Running sessions are never touched: they keep
//! the binary they started with, new ones pick up the new one.

use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};

use super::Daemon;
use crate::claude::SESSION_MARKERS;
use crate::proto::{Msg, UPDATE_BUSY};
use crate::term::strip_escapes;

/// How long the update may run.
const TIMEOUT: Duration = Duration::from_secs(300);

/// After a timeout's SIGTERM, how long the update gets before SIGKILL.
const KILL_GRACE: Duration = Duration::from_secs(2);

/// Output kept, counted from the end.
const TAIL_BYTES: usize = 4096;

/// Anthropic's official installer, run where claude is missing (as
/// `[claude] install` says: at once, after a `y`, or never).
const INSTALLER: &str = "{ curl -fsSL https://claude.ai/install.sh | bash; } 2>&1";

/// Run the update (or the install) and answer `ClaudeUpdated`. Only one runs
/// at a time per daemon, and never beside a plugin install (which waits
/// for the same lock).
pub async fn run(daemon: &Daemon, install: bool) -> Msg {
    if install {
        let path = daemon
            .config
            .path
            .clone()
            .or_else(|| std::env::var_os("PATH"));
        if let Some(why) = cannot_install(&daemon.config.claude, path.as_deref()) {
            return Msg::ClaudeUpdated {
                version: None,
                ok: false,
                tail: why,
            };
        }
        // `Welcome` says "no claude" also when one is there but `--version`
        // failed or timed out: never install over it.
        let bin = daemon.config.claude_bin();
        if bin.is_file() {
            return Msg::ClaudeUpdated {
                version: None,
                ok: false,
                tail: format!(
                    "claude is at {} but did not answer --version: not installing over it",
                    bin.display()
                ),
            };
        }
    }
    let Ok(_running) = daemon.updating.try_lock() else {
        return Msg::Error {
            message: UPDATE_BUSY.into(),
        };
    };
    let claude = daemon.config.claude_bin();
    let path = daemon.config.path.as_deref();
    let (ok, tail) = match execute(&claude, install, path, TIMEOUT).await {
        Ok(done) => done,
        Err(e) => (false, e),
    };
    let version = daemon.refresh_host().await.claude.map(|c| c.version);
    if install && ok && version.is_some() {
        // Plugin installs that failed for want of claude go at once.
        daemon.manifests.forget_failures();
    }
    Msg::ClaudeUpdated { version, ok, tail }
}

/// Why the installer cannot run on this host: `claude` (the daemon's
/// setting) is a path, so `CLAUDIO_CLAUDE_PATH` pins one the installer
/// does not write; or `curl` or `bash` is not on `path`.
fn cannot_install(claude: &Path, path: Option<&OsStr>) -> Option<String> {
    if claude.components().count() > 1 {
        return Some(format!(
            "CLAUDIO_CLAUDE_PATH is {}, which the installer does not write: install claude there",
            claude.display()
        ));
    }
    ["curl", "bash"]
        .into_iter()
        .find(|tool| !on_path(tool, path))
        .map(|tool| format!("claude installer needs {tool}"))
}

/// Whether an executable `name` is in one of `path`'s directories.
fn on_path(name: &str, path: Option<&OsStr>) -> bool {
    path.into_iter()
        .flat_map(std::env::split_paths)
        .filter(|dir| dir.is_absolute())
        .filter_map(|dir| std::fs::metadata(dir.join(name)).ok())
        .any(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// `(succeeded, output tail)`, or a description of why it could not run.
///
/// The command runs in a process group of its own: the installer is a
/// pipeline (`curl | bash`) whose `bash` starts more processes, and killing
/// only `sh` on timeout would leave those running, still installing.
///
/// `path` (the daemon's login-shell `PATH`) is the update's `PATH`: an npm
/// install updates itself with the user's `npm`.
async fn execute(
    claude: &Path,
    install: bool,
    path: Option<&OsStr>,
    timeout: Duration,
) -> Result<(bool, String), String> {
    let mut cmd = Command::new("sh");
    cmd.arg("-c");
    if install {
        cmd.arg(INSTALLER);
    } else {
        // `$0` is the claude path, so it needs no quoting; stderr joins stdout.
        cmd.arg("exec \"$0\" update 2>&1").arg(claude);
    }
    for marker in SESSION_MARKERS {
        cmd.env_remove(marker);
    }
    if let Some(path) = path {
        cmd.env("PATH", path);
    }
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("could not start the update: {e}"))?;
    let mut out = child.stdout.take().ok_or("no output pipe")?;

    let finished = tokio::time::timeout(timeout, async {
        let mut tail = Vec::new();
        let mut buf = [0u8; 1024];
        loop {
            match out.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    tail.extend_from_slice(&buf[..n]);
                    let excess = tail.len().saturating_sub(TAIL_BYTES);
                    tail.drain(..excess);
                }
            }
        }
        (child.wait().await.is_ok_and(|s| s.success()), tail)
    })
    .await;
    match finished {
        Ok((ok, tail)) => Ok((ok, printable(&tail))),
        Err(_) => {
            kill_group(&mut child).await;
            Err(format!("timed out after {} s", timeout.as_secs()))
        }
    }
}

/// Stop the update's whole process group: SIGTERM, a grace period for the
/// leader to exit, then SIGKILL for whatever is left. Reaps the leader.
pub(super) async fn kill_group(child: &mut Child) {
    // The leader's pid is the group id (`process_group(0)`); `None` once
    // it has been reaped, and then its pid may already be reused.
    let Some(pgid) = child.id().and_then(|pid| i32::try_from(pid).ok()) else {
        return;
    };
    // SAFETY: kill(2) has no memory-safety preconditions.
    unsafe { libc::kill(-pgid, libc::SIGTERM) };
    let _ = tokio::time::timeout(KILL_GRACE, child.wait()).await;
    // The leader may have exited while its descendants did not. A group id
    // is not reused while any member is alive, so this reaches only them.
    // SAFETY: as above.
    unsafe { libc::kill(-pgid, libc::SIGKILL) };
    let _ = child.wait().await;
}

/// Output as text: escape sequences and other control characters dropped.
fn printable(bytes: &[u8]) -> String {
    strip_escapes(bytes)
        .chars()
        .filter(|&c| c == '\n' || !c.is_control())
        .collect::<String>()
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_installer_needs_curl_and_bash_and_no_pinned_claude() {
        let dir = std::env::temp_dir().join(format!("claudio-tools-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let tool = |name: &str, mode: u32| {
            std::fs::write(dir.join(name), "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(dir.join(name), std::fs::Permissions::from_mode(mode))
                .unwrap();
        };
        let claude = Path::new("claude");
        let path = std::env::join_paths([Path::new("relative"), &dir]).unwrap();
        tool("bash", 0o755);
        assert_eq!(
            cannot_install(claude, Some(&path)).as_deref(),
            Some("claude installer needs curl")
        );
        tool("curl", 0o644);
        assert_eq!(
            cannot_install(claude, Some(&path)).as_deref(),
            Some("claude installer needs curl"),
            "not executable"
        );
        tool("curl", 0o755);
        assert_eq!(cannot_install(claude, Some(&path)), None);
        std::fs::remove_file(dir.join("bash")).unwrap();
        assert_eq!(
            cannot_install(claude, Some(&path)).as_deref(),
            Some("claude installer needs bash")
        );
        assert_eq!(
            cannot_install(claude, None).as_deref(),
            Some("claude installer needs curl")
        );
        let pinned = cannot_install(Path::new("/opt/claude/bin/claude"), Some(&path));
        assert_eq!(
            pinned.as_deref(),
            Some("CLAUDIO_CLAUDE_PATH is /opt/claude/bin/claude, which the installer does not write: install claude there")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn output_is_stripped_of_escapes_and_control_characters() {
        assert_eq!(
            printable(b"\x1b[1mUpdated\x1b[0m\r\nto 2.1.296\x1b]0;title\x07\n"),
            "Updated\nto 2.1.296"
        );
        assert_eq!(printable(b"  \n"), "");
    }

    /// On timeout the whole process group goes, not just `sh`: a descendant
    /// that would write a file later never gets to.
    #[tokio::test]
    async fn timeout_kills_the_updaters_descendants() {
        let dir = std::env::temp_dir().join(format!("claudio-update-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let marker = dir.join("late");
        let claude = dir.join("claude");
        let script = format!(
            "#!/bin/sh\n\
             ( sleep 1; touch '{}' ) &\n\
             echo updating\n\
             sleep 30\n",
            marker.display()
        );
        std::fs::write(&claude, script).unwrap();
        std::fs::set_permissions(&claude, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();

        let started = std::time::Instant::now();
        let result = execute(&claude, false, None, Duration::from_millis(300)).await;
        assert!(matches!(&result, Err(e) if e.contains("timed out")), "{result:?}");
        assert!(started.elapsed() < Duration::from_secs(5), "the leader was reaped promptly");

        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(!marker.exists(), "a descendant outlived the timeout");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
