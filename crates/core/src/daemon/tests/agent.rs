//! Daemon tests of the SSH agent relay (docs/manager-design.md, "Agent
//! forwarding"): which linked agent a connection reaches, pruning, the
//! cap, the socket's mode, and the env sessions and terminals get. The
//! agents are fake sockets in the daemon's temp dir; the test daemon has
//! no inherited agent, so the owner's `SSH_AUTH_SOCK` is never used.

use std::os::unix::fs::{symlink, PermissionsExt};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};

use super::*;
use crate::paths;

/// A fake agent at `path`: it greets every connection with `tag`, then
/// echoes. Dropping it closes its listener (the socket file stays, so a
/// connect is refused).
struct FakeAgent(tokio::task::JoinHandle<()>);

impl FakeAgent {
    fn start(path: &Path, tag: &'static [u8]) -> FakeAgent {
        let listener = UnixListener::bind(path).unwrap();
        FakeAgent(tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let (mut rd, mut wr) = s.split();
                    if wr.write_all(tag).await.is_ok() {
                        let _ = tokio::io::copy(&mut rd, &mut wr).await;
                    }
                });
            }
        }))
    }
}

impl Drop for FakeAgent {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// The relay's socket, once the daemon listens on it, and its `agents/`.
async fn relay_of(d: &TestDaemon) -> (PathBuf, PathBuf) {
    let relay = paths::agent_relay_beside(&d.config.socket);
    within(async {
        while !relay.exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    (relay, paths::agent_links_beside(&d.config.socket))
}

/// What a new relay connection is greeted with: the agent's tag, or ""
/// when the relay closes it.
async fn greeting(relay: &Path) -> String {
    let mut s = UnixStream::connect(relay).await.unwrap();
    let mut buf = [0u8; 1];
    let n = within(s.read(&mut buf)).await.unwrap_or(0);
    String::from_utf8_lossy(&buf[..n]).into_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_reaches_the_newest_live_agent_and_prunes_dead_links() {
    let d = TestDaemon::start().await;
    let (relay, links) = relay_of(&d).await;
    let mode = std::fs::metadata(&relay).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    paths::ensure_private_dir(&links).unwrap();
    let a = FakeAgent::start(&d.dir.join("a.sock"), b"A");
    symlink(d.dir.join("a.sock"), links.join("100")).unwrap();
    let b = FakeAgent::start(&d.dir.join("b.sock"), b"B");
    symlink(d.dir.join("b.sock"), links.join("200")).unwrap();

    assert_eq!(greeting(&relay).await, "B", "the newer link wins");
    drop(b);
    assert_eq!(greeting(&relay).await, "A");
    assert!(
        std::fs::symlink_metadata(links.join("200")).is_err(),
        "B's link is gone"
    );
    drop(a);
    assert_eq!(greeting(&relay).await, "", "no live agent: closed");
    assert!(std::fs::symlink_metadata(links.join("100")).is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_takes_32_connections_and_closes_the_33rd() {
    let d = TestDaemon::start().await;
    let (relay, links) = relay_of(&d).await;
    paths::ensure_private_dir(&links).unwrap();
    let _c = FakeAgent::start(&d.dir.join("c.sock"), b"C");
    symlink(d.dir.join("c.sock"), links.join("1")).unwrap();
    let mut held = Vec::new();
    for _ in 0..32 {
        let mut s = UnixStream::connect(&relay).await.unwrap();
        let mut tag = [0u8; 1];
        within(s.read_exact(&mut tag)).await.unwrap();
        assert_eq!(&tag, b"C");
        held.push(s);
    }
    assert_eq!(greeting(&relay).await, "", "the 33rd is closed");
    // The held ones still relay both ways.
    held[0].write_all(b"x").await.unwrap();
    let mut echo = [0u8; 1];
    within(held[0].read_exact(&mut echo)).await.unwrap();
    assert_eq!(&echo, b"x");
    // One closed frees its slot.
    held.pop();
    within(async {
        while greeting(&relay).await != "C" {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
}

/// A fake claude that writes to `agent.txt` in its cwd which agent it got:
/// `relay` (its `SSH_AUTH_SOCK` is the spec's `EXPECT_AGENT`), `explicit`,
/// `unset` or `other` (never the value).
fn fake_claude_agent() -> &'static Path {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let dir = private_temp_dir("claudio-fake-claude-agent");
        let bin = dir.join("claude");
        let script = format!(
            "#!/bin/sh\n\
             if [ \"$1\" = \"--version\" ]; then echo '9.9.9 (Claude Code)'; exit 0; fi\n\
             if [ \"$1\" = \"--help\" ]; then exit 0; fi\n\
             case \"$SSH_AUTH_SOCK\" in\n\
               \"$EXPECT_AGENT\") seen=relay ;;\n\
               /explicit/agent) seen=explicit ;;\n\
               '') seen=unset ;;\n\
               *) seen=other ;;\n\
             esac\n\
             echo \"$seen\" > agent.tmp && mv agent.tmp agent.txt\n\
             echo {BANNER}\n\
             exec cat\n"
        );
        crate::daemon::host::write_executable(&bin, &script);
        bin
    })
}

/// Spawn a claude session with `edit` applied to its spec; what agent it got.
async fn claude_agent(
    d: &TestDaemon,
    c: &mut Client,
    relay: &Path,
    edit: impl FnOnce(&mut SpawnSpec),
) -> String {
    let seen = d.work().join("agent.txt");
    let _ = std::fs::remove_file(&seen);
    let mut spec = d.spec(Uuid::new_v4());
    spec.env
        .push(("EXPECT_AGENT".into(), relay.to_string_lossy().into_owned()));
    edit(&mut spec);
    c.spawn(spec).await;
    within(async {
        loop {
            match std::fs::read_to_string(&seen) {
                Ok(text) if text.ends_with('\n') => return text.trim().to_owned(),
                _ => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        }
    })
    .await
}

/// Whether a new terminal's `SSH_AUTH_SOCK` is the relay.
async fn shell_has_relay(d: &TestDaemon, c: &mut Client, relay: &Path) -> bool {
    let id = Uuid::new_v4();
    c.spawn_shell(d.shell_spec(id)).await;
    c.attach(id, 24, 80).await;
    // The answer is computed, so the echoed command line never matches it.
    let test = format!(
        "[ \"$SSH_AUTH_SOCK\" = '{}' ] && echo relay-$((40+2)) || echo other-$((40+2))",
        relay.display()
    );
    c.type_line(id, &test).await;
    c.output_until(id, "-42").await.contains("relay-42")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sessions_get_the_relay_once_a_bridge_linked_an_agent() {
    let dir = TestDaemon::new_dir();
    let d = TestDaemon::start_with_claude(dir, fake_claude_agent().to_path_buf()).await;
    let (relay, links) = relay_of(&d).await;
    let mut c = d.client().await;
    // No bridge ever linked an agent here: the daemon's own value.
    let own = match std::env::var_os("SSH_AUTH_SOCK") {
        Some(v) if !v.is_empty() => "other",
        _ => "unset",
    };
    assert_eq!(claude_agent(&d, &mut c, &relay, |_| {}).await, own);
    assert!(!shell_has_relay(&d, &mut c, &relay).await);

    // A bridge linked a live agent: claude sessions and terminals get the relay.
    paths::ensure_private_dir(&links).unwrap();
    let _a = FakeAgent::start(&d.dir.join("a.sock"), b"A");
    symlink(d.dir.join("a.sock"), links.join("100")).unwrap();
    assert_eq!(claude_agent(&d, &mut c, &relay, |_| {}).await, "relay");
    assert!(shell_has_relay(&d, &mut c, &relay).await);

    // A spawn that scrubs the agent gets none; an explicit value wins.
    let scrub = |s: &mut SpawnSpec| s.env_remove = Some(vec!["SSH_AUTH_SOCK".into()]);
    assert_eq!(claude_agent(&d, &mut c, &relay, scrub).await, "unset");
    let explicit = |s: &mut SpawnSpec| {
        s.env
            .push(("SSH_AUTH_SOCK".into(), "/explicit/agent".into()))
    };
    assert_eq!(claude_agent(&d, &mut c, &relay, explicit).await, "explicit");
}
