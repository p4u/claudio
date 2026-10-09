//! `UpdateClaude`: run `claude update` (or the official installer when claude
//! is missing) on this host, then re-probe `claude --version`.
//!
//! The command runs as a task of its own, never inline in a client loop (the
//! reply is queued like `Attach`'s), under a deadline and with its output
//! bounded to the last few KiB. Running sessions are never touched: they keep
//! the binary they started with, new ones pick up the new one.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::claude::SESSION_MARKERS;
use super::Daemon;
use crate::proto::Msg;

/// How long the update may run.
const TIMEOUT: Duration = Duration::from_secs(300);

/// Output kept, counted from the end.
const TAIL_BYTES: usize = 4096;

/// Anthropic's official installer. Only ever run after an explicit `y`.
const INSTALLER: &str = "{ curl -fsSL https://claude.ai/install.sh | bash; } 2>&1";

/// Run the update (or the install) and answer `ClaudeUpdated`. Only one runs
/// at a time per daemon.
pub async fn run(daemon: &Daemon, install: bool) -> Msg {
    let Ok(_running) = daemon.updating.try_lock() else {
        return Msg::Error {
            message: "a claude update is already running on this host".into(),
        };
    };
    let (ok, tail) = match execute(&daemon.config.claude_bin(), install).await {
        Ok(done) => done,
        Err(e) => (false, e),
    };
    Msg::ClaudeUpdated {
        version: daemon.refresh_host().await.claude.map(|c| c.version),
        ok,
        tail,
    }
}

/// `(succeeded, output tail)`, or a description of why it could not run.
async fn execute(claude: &Path, install: bool) -> Result<(bool, String), String> {
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
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("could not start the update: {e}"))?;
    let mut out = child.stdout.take().ok_or("no output pipe")?;

    // Dropping the future on timeout kills the child (`kill_on_drop`).
    let finished = tokio::time::timeout(TIMEOUT, async {
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
        Err(_) => Err(format!("timed out after {} s", TIMEOUT.as_secs())),
    }
}

/// Output as text with control characters (ANSI escapes included) dropped.
fn printable(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
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
    fn output_is_stripped_of_control_characters() {
        assert_eq!(
            printable(b"\x1b[1mUpdated\x1b[0m\r\nto 2.1.296\n"),
            "[1mUpdated[0m\nto 2.1.296"
        );
        assert_eq!(printable(b"  \n"), "");
    }
}
