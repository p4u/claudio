//! Daemon tests for the freshness check ([`crate::freshness`]): an outdated
//! daemon is replaced when idle, its sessions coming back from the journal,
//! and left running when busy. "Outdated" is simulated with an injected
//! `Config::build`; the replacement is a second in-process daemon on the same
//! socket, lock and journal.

use std::sync::atomic::{AtomicBool, Ordering};

use super::*;
use crate::freshness::{ensure_current, DaemonCheck};
use crate::remote::probe::Probe;

/// This test binary, as the newer build.
fn own() -> Probe {
    Probe::own().expect("the test binary is readable").clone()
}

/// A build of the same version from an older file.
fn old_build() -> Probe {
    Probe {
        build: "0".repeat(64),
        mtime: Some(0),
        ..own()
    }
}

/// A daemon in a new dir that runs `build`, with claude being `claude`.
fn daemon_running(build: Probe, claude: &Path) -> TestDaemon {
    let dir = TestDaemon::new_dir();
    let config = Config {
        build: Some(build),
        claude: claude.to_path_buf(),
        ..TestDaemon::config(&dir)
    };
    TestDaemon::start_with_config(dir, config)
}

/// A fake claude that prints the arguments after its `--settings <json>` on
/// a line of their own (short, so the screen snapshot never wraps it).
fn fake_claude_args() -> &'static Path {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("claudio-fake-claude-args-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("claude");
        let script = format!(
            "#!/bin/sh\n\
             if [ \"$1\" = \"--version\" ]; then echo '9.9.9 (Claude Code)'; exit 0; fi\n\
             if [ \"$1\" = \"--help\" ]; then exit 0; fi\n\
             printf '%s' \"$2\" > settings.json\n\
             shift 2\n\
             echo {BANNER}\n\
             echo \"ARGS[$*]\"\n\
             exec cat\n"
        );
        std::fs::write(&bin, script).unwrap();
        std::fs::set_permissions(&bin, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        bin
    })
}

/// The `Welcome` of the daemon at `socket`.
async fn welcome(socket: &Path) -> proto::Welcome {
    let mut c = Client::raw(socket).await;
    match c.call(Msg::Hello(hello(PROTO))).await {
        Msg::Welcome(w) => w,
        other => panic!("expected welcome, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn welcome_reports_the_daemons_build() {
    let build = Probe {
        build: "b1".into(),
        mtime: Some(42),
        ..own()
    };
    let d = daemon_running(build, fake_claude());
    let w = welcome(&d.config.socket).await;
    assert_eq!((w.build.as_deref(), w.build_time), (Some("b1"), Some(42)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_outdated_idle_daemon_is_replaced_and_its_sessions_come_back() {
    let old = daemon_running(old_build(), fake_claude_args());
    let mut c = old.client().await;

    // A claude session that has reported its conversation, and a terminal.
    let (claude, shell) = (Uuid::new_v4(), Uuid::new_v4());
    c.spawn(old.spec(claude)).await;
    let token = old.token().await;
    let payload = serde_json::json!({"session_id": "conv-7", "source": "startup"});
    send_hook(&old.config.socket, &token, "SessionStart", payload).await;
    c.event(claude, |e| {
        matches!(e, SessionEvent::ClaudeSession { .. }).then_some(())
    })
    .await;
    c.spawn_shell(old.shell_spec(shell)).await;

    // The current binary starts its daemon on the same paths, once the old
    // one has let go of the lock.
    let config = Config {
        build: Some(own()),
        ..old.config.clone()
    };
    let start = || async move {
        let listening = server::start(config)?.expect("the old daemon released the lock");
        tokio::spawn(listening.serve());
        Ok(())
    };
    let check = ensure_current(&old.config.socket, &old.config.lock, &own(), start).await;
    assert_eq!(check.unwrap(), DaemonCheck::Restarted);

    // The new daemon runs our build, and has both sessions, dormant.
    let w = welcome(&old.config.socket).await;
    assert_eq!(w.build.as_deref(), Some(own().build.as_str()));
    let mut n = Client::connect(&old.config.socket).await;
    let mut sessions = n.sessions().await;
    sessions.sort_by_key(|s| s.id != claude);
    let dormant: Vec<_> = sessions
        .iter()
        .map(|s| (s.id, s.kind, s.pid, s.claude_session_id.as_deref()))
        .collect();
    assert_eq!(
        dormant,
        [
            (claude, proto::SessionKind::Claude, None, Some("conv-7")),
            (shell, proto::SessionKind::Shell, None, None),
        ]
    );

    // Recovery, as the TUI does it: claude resumes its conversation…
    let resume = SpawnSpec {
        args: vec!["--resume".into(), "conv-7".into()],
        ..old.spec(claude)
    };
    assert!(n.spawn(resume).await.is_some());
    n.attach_until(claude, "ARGS[--resume conv-7 -n test]")
        .await;
    // …and the terminal is a fresh shell.
    assert!(n.spawn_shell(old.shell_spec(shell)).await.is_some());
    n.attach(shell, 24, 80).await;
    n.type_line(shell, "echo fresh-$((40 + 2))").await;
    n.output_until(shell, "fresh-42").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_outdated_busy_daemon_is_left_running() {
    let old = daemon_running(old_build(), fake_claude());
    let mut c = old.client().await;
    let id = Uuid::new_v4();
    c.spawn(old.spec(id)).await;
    let token = old.token().await;
    let prompt = serde_json::json!({"prompt": "x"});
    send_hook(&old.config.socket, &token, "UserPromptSubmit", prompt).await;
    c.event(id, |e| {
        matches!(
            e,
            SessionEvent::State {
                state: SessionState::Working
            }
        )
        .then_some(())
    })
    .await;

    let started = AtomicBool::new(false);
    let start = || async {
        started.store(true, Ordering::Relaxed);
        Ok(())
    };
    let check = ensure_current(&old.config.socket, &old.config.lock, &own(), start).await;
    assert_eq!(check.unwrap(), DaemonCheck::Deferred);
    assert!(!started.load(Ordering::Relaxed));

    // Still the old daemon, its session still running.
    let w = welcome(&old.config.socket).await;
    assert_eq!(w.build, Some("0".repeat(64)));
    let s = &c.sessions().await[0];
    assert_eq!((s.state, s.pid.is_some()), (SessionState::Working, true));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_current_or_newer_daemon_is_left_alone() {
    let newer = Probe {
        build: "f".repeat(64),
        mtime: Some(u64::MAX),
        ..own()
    };
    for build in [own(), newer] {
        let d = daemon_running(build, fake_claude());
        let start = || async { panic!("no restart") };
        let check = ensure_current(&d.config.socket, &d.config.lock, &own(), start).await;
        assert_eq!(check.unwrap(), DaemonCheck::UpToDate);
        welcome(&d.config.socket).await;
    }
}
