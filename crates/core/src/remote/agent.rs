//! `claudio --slave`'s side of SSH agent forwarding: when the connection's
//! `SSH_AUTH_SOCK` answers an identities request, the bridge links it as
//! `<runtime>/agents/<pid>` for the daemon's relay (docs/manager-design.md,
//! "Agent forwarding"), and removes the link when it exits. A failure is
//! one stderr line and never fails the bridge: stdout is the protocol
//! channel and stays clean.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

use crate::paths;

/// How long the identities request may take, connect included.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
/// The longest identities answer taken; only its length and type are read.
const MAX_ANSWER: u32 = 256 * 1024;
/// `SSH_AGENTC_REQUEST_IDENTITIES` (11), framed: length 1, then the type.
const REQUEST_IDENTITIES: [u8; 5] = [0, 0, 0, 1, 11];
/// `SSH_AGENT_IDENTITIES_ANSWER`.
const IDENTITIES_ANSWER: u8 = 12;

/// This bridge's link under `agents/`; removed (best effort) when dropped,
/// on every return path of the bridge.
#[derive(Debug)]
pub struct Link(PathBuf);

impl Link {
    /// Where the link is.
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Link `sock` (the bridge's `SSH_AUTH_SOCK`) as `links/<pid>` when it is
/// an absolute path to a Unix socket that answers an identities request
/// within `timeout`. `None` when it is unset; otherwise any failure is one
/// line on stderr and `None`. ssh asks for forwarding even when its own
/// side has no agent, and sshd then makes a socket that answers nothing:
/// the probe keeps that one out.
pub async fn register(
    sock: Option<OsString>,
    links: &Path,
    pid: u32,
    timeout: Duration,
) -> Option<Link> {
    let sock = PathBuf::from(sock.filter(|s| !s.is_empty())?);
    let linked = match probe(&sock, timeout).await {
        Ok(()) => link(&sock, links, pid),
        Err(e) => Err(e),
    };
    linked
        .inspect_err(|e| eprintln!("claudio --slave: ssh agent not linked: {e}"))
        .ok()
}

/// Ask the agent at `sock` for its identities; `Ok` when it answers with
/// an identities answer within `timeout`.
async fn probe(sock: &Path, timeout: Duration) -> io::Result<()> {
    use std::os::unix::fs::FileTypeExt;
    if !sock.is_absolute() {
        return Err(io::Error::other("SSH_AUTH_SOCK is not an absolute path"));
    }
    if !std::fs::metadata(sock)?.file_type().is_socket() {
        return Err(io::Error::other("SSH_AUTH_SOCK is not a socket"));
    }
    let ask = async {
        let mut agent = UnixStream::connect(sock).await?;
        agent.write_all(&REQUEST_IDENTITIES).await?;
        let mut head = [0u8; 5];
        agent.read_exact(&mut head).await?;
        answer_ok(head).map_err(io::Error::other)
    };
    match tokio::time::timeout(timeout, ask).await {
        Ok(answer) => answer,
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "the agent did not answer",
        )),
    }
}

/// Whether `head`, an answer's 4-byte length and its type byte, starts an
/// identities answer of at most [`MAX_ANSWER`] bytes.
fn answer_ok(head: [u8; 5]) -> Result<(), String> {
    let [a, b, c, d, kind] = head;
    let len = u32::from_be_bytes([a, b, c, d]);
    if len == 0 || len > MAX_ANSWER {
        return Err(format!("answer length {len} out of range"));
    }
    if kind != IDENTITIES_ANSWER {
        return Err(format!("answer type {kind}, not an identities answer"));
    }
    Ok(())
}

/// `links/<pid>` → `sock`: a symlink made under a temporary name, then
/// renamed into place, so a stale link of a reused pid is replaced
/// atomically. `links` is made private (0700) first.
fn link(sock: &Path, links: &Path, pid: u32) -> io::Result<Link> {
    paths::ensure_private_dir(links)?;
    let tmp = links.join(format!(".{pid}.tmp"));
    let _ = std::fs::remove_file(&tmp);
    std::os::unix::fs::symlink(sock, &tmp)?;
    let path = links.join(pid.to_string());
    if let Err(e) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(Link(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::UnixListener;

    /// What a fake agent does with a request.
    #[derive(Clone, Copy)]
    enum Agent {
        /// Answers with these bytes.
        Says(&'static [u8]),
        /// Closes the connection.
        Hangs,
        /// Reads, then never answers.
        Silent,
    }

    /// A fake agent at `<dir>/agent.sock`, serving every connection.
    fn fake_agent(dir: &Path, agent: Agent) -> PathBuf {
        let path = dir.join("agent.sock");
        let listener = UnixListener::bind(&path).unwrap();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut req = [0u8; 5];
                    if s.read_exact(&mut req).await.is_err() {
                        return;
                    }
                    match agent {
                        Agent::Says(bytes) => {
                            let _ = s.write_all(bytes).await;
                        }
                        Agent::Hangs => {}
                        Agent::Silent => tokio::time::sleep(Duration::from_secs(30)).await,
                    }
                });
            }
        });
        path
    }

    fn dir() -> PathBuf {
        let dir = crate::paths::socket_test_dir("cl-ag");
        crate::paths::ensure_private_dir(&dir).unwrap();
        dir
    }

    const SHORT: Duration = Duration::from_millis(300);

    #[tokio::test]
    async fn an_answering_agent_is_linked_until_the_link_drops() {
        let dir = dir();
        let sock = fake_agent(&dir, Agent::Says(&[0, 0, 0, 5, 12, 0, 0, 0, 0]));
        let links = dir.join("agents");
        let link = register(Some(sock.clone().into()), &links, 4242, SHORT)
            .await
            .expect("linked");
        assert_eq!(link.path(), links.join("4242"));
        assert_eq!(std::fs::read_link(link.path()).unwrap(), sock);
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&links).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        drop(link);
        assert!(std::fs::symlink_metadata(links.join("4242")).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn a_reused_pid_replaces_the_stale_link() {
        let dir = dir();
        let sock = fake_agent(&dir, Agent::Says(&[0, 0, 0, 5, 12, 0, 0, 0, 0]));
        let links = dir.join("agents");
        crate::paths::ensure_private_dir(&links).unwrap();
        std::os::unix::fs::symlink("/nonexistent/agent", links.join("7")).unwrap();
        let link = register(Some(sock.clone().into()), &links, 7, SHORT).await;
        assert_eq!(std::fs::read_link(links.join("7")).unwrap(), sock);
        assert!(!links.join(".7.tmp").exists());
        drop(link);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn agents_that_do_not_answer_12_are_not_linked() {
        let cases: [(&str, Agent); 4] = [
            ("eof", Agent::Hangs),
            ("wrong type", Agent::Says(&[0, 0, 0, 1, 5])),
            ("too long", Agent::Says(&[0, 4, 0, 1, 12])),
            ("silent", Agent::Silent),
        ];
        for (what, agent) in cases {
            let dir = dir();
            let sock = fake_agent(&dir, agent);
            let links = dir.join("agents");
            let start = std::time::Instant::now();
            let link = register(Some(sock.into()), &links, 1, SHORT).await;
            assert!(link.is_none(), "{what}");
            assert!(
                start.elapsed() < SHORT * 3,
                "{what}: bounded by the timeout"
            );
            assert!(!links.join("1").exists(), "{what}");
            std::fs::remove_dir_all(&dir).unwrap();
        }
    }

    #[tokio::test]
    async fn no_usable_sock_links_nothing() {
        let dir = dir();
        let links = dir.join("agents");
        let file = dir.join("not-a-socket");
        std::fs::write(&file, b"").unwrap();
        for sock in [
            None,
            Some(OsString::new()),
            Some("relative/agent".into()),
            Some(file.into()),
            Some(dir.join("missing").into()),
        ] {
            assert!(register(sock, &links, 1, SHORT).await.is_none());
        }
        assert!(!links.exists(), "agents/ is made only for a working agent");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_answer_head_is_checked_by_length_and_type() {
        assert!(answer_ok([0, 0, 0, 5, 12]).is_ok());
        assert!(answer_ok([0, 4, 0, 0, 12]).is_ok(), "256 KiB is the limit");
        assert!(answer_ok([0, 4, 0, 1, 12]).is_err());
        assert!(answer_ok([0, 0, 0, 0, 12]).is_err());
        assert!(answer_ok([0, 0, 0, 1, 5]).is_err());
    }
}
