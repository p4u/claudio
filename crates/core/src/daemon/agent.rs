//! The daemon's SSH agent relay. It listens on `agent-v{PROTO}.sock` beside
//! the daemon socket (0600, peer uid checked like the daemon socket), and
//! sessions get that path as `SSH_AUTH_SOCK` once a bridge has linked an
//! agent here (`agents/` exists). Each connection goes to the newest live
//! link a bridge made (`remote::agent`), else to the daemon's own inherited
//! agent, so a session outlives the connection that forwarded its agent
//! (docs/manager-design.md, "Agent forwarding"). The traffic is copied,
//! never parsed, logged or stored.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime};

use portable_pty::CommandBuilder;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Semaphore;
use tokio::task::{JoinHandle, JoinSet};

use super::Daemon;
use crate::paths;

/// Relayed connections at once; one more is closed when accepted.
const MAX_RELAYS: usize = 32;
/// How long one agent may take to accept a connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(1);
/// The most `agents/` entries looked at, and links tried, per connection.
const MAX_ENTRIES: usize = 1024;
const MAX_LINKS: usize = 64;

/// The relay as the daemon's [`super::Config`] holds it: the daemon's own
/// inherited `SSH_AUTH_SOCK` (tried after the links, never pruned) and,
/// once the relay listens, its path. Clones share the listening state.
#[derive(Debug, Clone, Default)]
pub struct AgentRelay {
    /// `SSH_AUTH_SOCK` as the daemon inherited it, when absolute.
    pub fallback: Option<PathBuf>,
    listening: Arc<OnceLock<PathBuf>>,
}

impl AgentRelay {
    /// The relay of a daemon that inherited `sock` as its `SSH_AUTH_SOCK`.
    pub fn inherited(sock: Option<OsString>) -> AgentRelay {
        AgentRelay {
            fallback: sock.map(PathBuf::from).filter(|p| p.is_absolute()),
            listening: Arc::default(),
        }
    }

    /// Point a session at the relay (`SSH_AUTH_SOCK`) when it listens and a
    /// bridge has linked an agent here at least once: `agents/` exists, so
    /// a host no bridge reaches (the local one) keeps its own agent. Runs
    /// before the spawn's env is applied: an `env_remove` naming
    /// `SSH_AUTH_SOCK` takes it away again, and a value in the env wins.
    pub(super) fn apply(&self, cmd: &mut CommandBuilder) {
        let Some(relay) = self.listening.get() else {
            return;
        };
        let links = paths::agent_links_beside(relay);
        if std::fs::symlink_metadata(links).is_ok_and(|m| m.is_dir()) {
            cmd.env("SSH_AUTH_SOCK", relay);
        }
    }
}

/// The relay's accept loop; dropping it stops listening and every relayed
/// connection.
pub(super) struct Serving(JoinHandle<()>);

impl Drop for Serving {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Bind the relay beside the daemon socket and serve it until the returned
/// [`Serving`] is dropped. A bind failure is logged once and leaves the
/// relay off (and sessions' env as it was); the daemon still serves.
pub(super) fn start(daemon: &Arc<Daemon>) -> Option<Serving> {
    let config = &daemon.config;
    let path = paths::agent_relay_beside(&config.socket);
    let listener = super::server::bind(&path)
        .inspect_err(|e| tracing::warn!(error = %e, "agent relay off: cannot bind"))
        .ok()?;
    let fallback = config.agent.fallback.clone().filter(|f| *f != path);
    let links = paths::agent_links_beside(&path);
    let _ = config.agent.listening.set(path.clone());
    let accept = serve(listener, path, links, fallback);
    Some(Serving(tokio::spawn(accept)))
}

/// Accept forever: our own uid only, at most [`MAX_RELAYS`] at once.
async fn serve(listener: UnixListener, own: PathBuf, links: PathBuf, fallback: Option<PathBuf>) {
    let slots = Arc::new(Semaphore::new(MAX_RELAYS));
    let mut relays = JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let Ok((client, _)) = accepted else {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                };
                if !own_peer(&client) {
                    continue;
                }
                let Ok(slot) = Arc::clone(&slots).try_acquire_owned() else {
                    tracing::debug!("agent relay: {MAX_RELAYS} connections already; closed one");
                    continue;
                };
                let (own, links, fallback) = (own.clone(), links.clone(), fallback.clone());
                relays.spawn(async move {
                    relay(client, &own, &links, fallback.as_deref()).await;
                    drop(slot);
                });
            }
            Some(_) = relays.join_next(), if !relays.is_empty() => {}
        }
    }
}

/// Whether the peer runs as our uid (the daemon socket's guard).
fn own_peer(client: &UnixStream) -> bool {
    match client.peer_cred() {
        Ok(cred) if cred.uid() == paths::uid() => true,
        Ok(cred) => {
            tracing::warn!(uid = cred.uid(), "agent relay: refused another user");
            false
        }
        Err(e) => {
            tracing::warn!(error = %e, "agent relay: no peer credentials");
            false
        }
    }
}

/// Copy bytes between `client` and the first live agent until either side
/// closes; no live agent closes `client`. `own` is the relay's socket.
async fn relay(mut client: UnixStream, own: &Path, links: &Path, fallback: Option<&Path>) {
    let Some(mut agent) = connect_agent(own, links, fallback).await else {
        tracing::debug!("agent relay: no live agent");
        return;
    };
    let _ = tokio::io::copy_bidirectional(&mut client, &mut agent).await;
}

/// The links newest first, then `fallback`: the first that accepts within
/// [`CONNECT_TIMEOUT`]. A link whose target refuses, is gone or is not ours
/// to use is removed (the link is ours even when the target is not); a
/// timeout or another error keeps it. `fallback` is never removed. A link to
/// `own`, the relay's socket (a bridge that forwarded it), would loop: it is
/// skipped and kept.
async fn connect_agent(own: &Path, links: &Path, fallback: Option<&Path>) -> Option<UnixStream> {
    let (dir, own) = (links.to_owned(), own.to_owned());
    let ordered = tokio::task::spawn_blocking(move || {
        let mut ordered = newest_first(&dir);
        ordered.retain(|link| !std::fs::read_link(link).is_ok_and(|to| to == own));
        ordered
    })
    .await
    .unwrap_or_default();
    for link in ordered {
        match tokio::time::timeout(CONNECT_TIMEOUT, UnixStream::connect(&link)).await {
            Ok(Ok(agent)) => return Some(agent),
            Ok(Err(e)) if dead(&e) => {
                let _ = tokio::fs::remove_file(&link).await;
            }
            _ => {}
        }
    }
    let connect = UnixStream::connect(fallback?);
    tokio::time::timeout(CONNECT_TIMEOUT, connect)
        .await
        .ok()?
        .ok()
}

/// A link whose target will not come back: refused, gone, or forbidden.
fn dead(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::ConnectionRefused
            | io::ErrorKind::NotFound
            | io::ErrorKind::PermissionDenied
    )
}

/// The symlinks in `dir`, newest `lstat` mtime first, ties by file name
/// descending; at most [`MAX_LINKS`] of the first [`MAX_ENTRIES`] entries.
/// Dot-names are a bridge's half-made link (`.<pid>.tmp`): left out.
fn newest_first(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut links: Vec<(SystemTime, OsString, PathBuf)> = entries
        .take(MAX_ENTRIES)
        .filter_map(Result::ok)
        .filter(|e| !e.file_name().as_encoded_bytes().starts_with(b"."))
        .filter_map(|e| {
            let meta = e.metadata().ok().filter(|m| m.file_type().is_symlink())?;
            Some((meta.modified().ok()?, e.file_name(), e.path()))
        })
        .collect();
    links.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    links
        .into_iter()
        .take(MAX_LINKS)
        .map(|(_, _, p)| p)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_are_newest_first_ties_by_name_and_only_symlinks() {
        let dir = crate::paths::socket_test_dir("cl-ar");
        crate::paths::ensure_private_dir(&dir).unwrap();
        for name in ["100", "300", "200"] {
            std::os::unix::fs::symlink("/nowhere", dir.join(name)).unwrap();
        }
        std::fs::write(dir.join("999"), b"not a link").unwrap();
        // A bridge killed between its symlink and its rename leaves this behind.
        std::os::unix::fs::symlink("/nowhere", dir.join(".400.tmp")).unwrap();
        let names: Vec<String> = newest_first(&dir)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        // Made within one tick or not, "300" was made after "100": newest first
        // or, on a tie, the larger name first, never the plain file nor a
        // half-made link.
        assert_eq!(names.len(), 3, "{names:?}");
        assert!(!names.contains(&"999".to_owned()));
        let pos = |n: &str| names.iter().position(|x| x == n).unwrap();
        assert!(pos("300") < pos("100"), "{names:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_relay_that_does_not_listen_changes_no_env() {
        let mut cmd = CommandBuilder::new("claude");
        cmd.env_remove("SSH_AUTH_SOCK");
        AgentRelay::inherited(Some("relative".into())).apply(&mut cmd);
        assert_eq!(cmd.get_env("SSH_AUTH_SOCK"), None);
        assert_eq!(
            AgentRelay::inherited(Some("relative".into())).fallback,
            None
        );
        let abs = AgentRelay::inherited(Some("/tmp/a.sock".into()));
        assert_eq!(abs.fallback.as_deref(), Some(Path::new("/tmp/a.sock")));
    }

    #[tokio::test]
    async fn a_link_to_the_relay_itself_is_skipped_never_pruned() {
        let dir = crate::paths::socket_test_dir("cl-ar");
        let links = dir.join("agents");
        crate::paths::ensure_private_dir(&links).unwrap();
        let (own, a) = (dir.join("relay.sock"), dir.join("a.sock"));
        let agent = UnixListener::bind(&a).unwrap();
        std::os::unix::fs::symlink(&a, links.join("100")).unwrap();
        std::os::unix::fs::symlink(&own, links.join("200")).unwrap();
        // The newer link is the relay itself, listening (it would loop), then
        // closed (it would be pruned): the older live link answers both times.
        let mut relay = Some(UnixListener::bind(&own).unwrap());
        for _ in 0..2 {
            let _conn = connect_agent(&own, &links, None).await.unwrap();
            let got = tokio::time::timeout(Duration::from_secs(5), agent.accept()).await;
            assert!(got.is_ok_and(|r| r.is_ok()), "the older live link answers");
            assert!(std::fs::symlink_metadata(links.join("200")).is_ok(), "kept");
            drop(relay.take());
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
