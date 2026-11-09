//! Daemon tests for what a spawn carries beside its args (docs/daemon-protocol.md,
//! "Spawn env" and "Optional args"): the caps on `env_remove` and
//! `optional`, the journaled optional flags, and the env a session's
//! process gets.

use super::*;
use crate::addon::test_support::TestAddon;
use crate::addon::Addon;

fn refused(reply: Msg) -> String {
    match reply {
        Msg::Error { message } => message,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bad_env_remove_or_optional_is_refused() {
    let d = TestDaemon::start().await;
    let mut c = d.client().await;
    let id = Uuid::new_v4();
    let spec = SpawnSpec {
        env_remove: Some(vec!["OK".into(), "NOT A NAME".into()]),
        ..d.spec(id)
    };
    let why = refused(c.call(Msg::Spawn(spec)).await);
    assert_eq!(why, "bad env_remove: name 2 is not an env var name");
    let spec = SpawnSpec {
        env_remove: Some(vec!["A".into(); 65]),
        ..d.spec(id)
    };
    let why = refused(c.call(Msg::Spawn(spec)).await);
    assert_eq!(why, "bad env_remove: more than 64 names");
    let spec = SpawnSpec {
        optional: vec!["--agents".into(), "agents".into()],
        ..d.spec(id)
    };
    let why = refused(c.call(Msg::Spawn(spec)).await);
    assert_eq!(why, "bad optional: flag 2 is not --[a-z0-9][a-z0-9-]*");
    let spec = SpawnSpec {
        optional: vec!["--a".into(); 17],
        ..d.spec(id)
    };
    let why = refused(c.call(Msg::Spawn(spec)).await);
    assert_eq!(why, "bad optional: more than 16 flags");
    assert!(c.sessions().await.is_empty(), "nothing was started");

    // A good spawn, then a respawn with a bad list: refused before the
    // process is stopped.
    let pid = c.spawn(d.spec(id)).await;
    let respawn = proto::RespawnSpec {
        env_remove: Some(vec!["1BAD".into()]),
        ..respawn_spec(id, false)
    };
    let why = refused(c.call(Msg::Respawn(respawn)).await);
    assert_eq!(why, "bad env_remove: name 1 is not an env var name");
    let sessions = c.sessions().await;
    assert_eq!(sessions[0].pid, pid, "the session still runs");
}

/// The optional flags are journaled with the args, and a respawn (whose
/// request names none) drops them again on a claude that lacks them (the
/// fake's `--help` lists nothing), while other flags pass.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn optional_is_journaled_and_applied_on_respawn() {
    let d = TestDaemon::start_logging_argv().await;
    let mut c = d.client().await;
    let id = Uuid::new_v4();
    let spec = SpawnSpec {
        args: vec!["--x-test".into(), "v".into(), "--y-test".into()],
        optional: vec!["--x-test".into()],
        ..d.spec(id)
    };
    let old_pid = c.spawn(spec).await.unwrap();
    c.attach_until(id, BANNER).await;
    let entry = &d.journal()["sessions"][0];
    assert_eq!(
        entry["args"],
        serde_json::json!(["--x-test", "v", "--y-test"])
    );
    assert_eq!(entry["optional"], serde_json::json!(["--x-test"]));

    let (pid, _) = c.respawn(id, false).await;
    assert_ne!(pid, Some(old_pid));
    d.runs_reach(2).await;
    let runs = d.runs();
    assert_eq!(runs[0], ["--y-test", "-n", "test"]);
    assert_eq!(runs[1], ["--y-test", "-n", "test"]);
    assert_eq!(
        d.journal()["sessions"][0]["optional"],
        serde_json::json!(["--x-test"])
    );
}

/// A fake claude that fails at once on `--resume` (a deleted conversation)
/// and otherwise saves its env to `run.env` and its args to `run.argv` in
/// its cwd, then behaves like the normal fake.
fn fake_claude_recording() -> &'static Path {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("claudio-fake-claude-rr-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("claude");
        let script = format!(
            "#!/bin/sh\n\
             if [ \"$1\" = \"--version\" ]; then echo '9.9.9 (Claude Code)'; exit 0; fi\n\
             if [ \"$1\" = \"--help\" ]; then exit 0; fi\n\
             for arg in \"$@\"; do\n\
               if [ \"$arg\" = \"--resume\" ]; then exit 1; fi\n\
             done\n\
             env > run.env\n\
             printf '%s\\n' \"$@\" > run.argv\n\
             echo {BANNER}\n\
             exec cat\n"
        );
        crate::daemon::host::write_executable(&bin, &script);
        bin
    })
}

/// The env and the args the recording fake last ran with, once it has.
async fn recorded(d: &TestDaemon) -> (String, String) {
    let argv_file = d.work().join("run.argv");
    let argv = within(async {
        loop {
            match std::fs::read_to_string(&argv_file) {
                Ok(argv) if argv.ends_with('\n') => return argv,
                _ => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        }
    })
    .await;
    (
        std::fs::read_to_string(d.work().join("run.env")).unwrap(),
        argv,
    )
}

/// The fresh retry after a failed `--resume` starts with the env and
/// `env_remove` of the spawn it replaces (kept in the daemon's memory,
/// never journaled), and with its optional flags dropped again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resume_retry_keeps_the_session_env() {
    // HOME stands in for an inherited variable the spawn removes: a test
    // must not change this process's env, which the daemon's sessions
    // inherit.
    assert!(std::env::var_os("HOME").is_some(), "the test needs a HOME");
    let claude = fake_claude_recording().to_path_buf();
    let d = TestDaemon::start_with_claude(TestDaemon::new_dir(), claude).await;
    let mut c = d.client().await;
    let id = Uuid::new_v4();
    let spec = SpawnSpec {
        args: vec![
            "--resume".into(),
            "gone".into(),
            "--x-test".into(),
            "v".into(),
        ],
        env: vec![("ANTHROPIC_AUTH_TOKEN".into(), "spawn-token".into())],
        env_remove: Some(vec!["ANTHROPIC_API_KEY".into(), "HOME".into()]),
        optional: vec!["--x-test".into()],
        ..d.spec(id)
    };
    c.spawn(spec).await;
    let (env, argv) = recorded(&d).await;
    let set = |key: &str| env.lines().any(|l| l.starts_with(&format!("{key}=")));
    // Values are never printed (C1): the messages name the keys only.
    let token = "ANTHROPIC_AUTH_TOKEN=spawn-token";
    assert!(
        env.lines().any(|l| l == token),
        "the retry lost the spawn's env"
    );
    assert!(!set("ANTHROPIC_API_KEY"), "an API key reached the retry");
    assert!(!set("HOME"), "the retry lost the spawn's env_remove");
    let args: Vec<&str> = argv.lines().skip(2).collect();
    assert_eq!(args, ["-n", "test"], "fresh, and the optional flag dropped");
    let entry = &d.journal()["sessions"][0];
    assert!(entry.get("env").is_none() && entry.get("env_remove").is_none());
}

/// `CLAUDIO_TEST_OLD_DAEMON=1` (debug builds) makes a daemon play the
/// previous release for mixed-version tests: no `addon:` feature, `AddonOp`
/// refused as unknown, `env_remove` taken as absent (the compatibility
/// scrub) and `optional` ignored (only the legacy three are dropped).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_old_daemon_switch_hides_addons_and_scrubs_as_before() {
    assert!(super::super::admit::old_peer(Some("1".as_ref())));
    assert!(!super::super::admit::old_peer(Some("0".as_ref())));
    assert!(!super::super::admit::old_peer(None));

    let dir = TestDaemon::new_dir();
    let echo: Arc<dyn Addon> = Arc::new(TestAddon::new("echo").with_daemon());
    let config = Config {
        claude: fake_claude_recording().to_path_buf(),
        old_peer: true,
        ..TestDaemon::config(&dir)
    };
    let d = TestDaemon::start_with_addons(dir, config, Addons::new(vec![echo]).unwrap());
    let mut c = Client::raw(&d.config.socket).await;
    let Msg::Welcome(welcome) = c.call(Msg::Hello(hello(PROTO))).await else {
        panic!("no welcome");
    };
    assert!(welcome.features.iter().any(|f| f == "lens"));
    assert!(!welcome.features.iter().any(|f| f.starts_with("addon:")));
    let op = Msg::AddonOp {
        addon: "echo".into(),
        op: "echo".into(),
        body: serde_json::Value::Null,
    };
    let why = refused(c.call(op).await);
    assert_eq!(why, "unsupported op: addon_op");

    // A new client's spawn: env_remove and optional are ignored, so the
    // compatibility scrub applies (HOME stays) and the generic flag is
    // passed; a bad list is not even looked at.
    let id = Uuid::new_v4();
    let spec = SpawnSpec {
        args: vec![
            "--x-test".into(),
            "v".into(),
            "--agents".into(),
            "{}".into(),
        ],
        env: vec![("ANTHROPIC_AUTH_TOKEN".into(), "new-client".into())],
        env_remove: Some(vec!["HOME".into(), "NOT A NAME".into()]),
        optional: vec!["--x-test".into()],
        ..d.spec(id)
    };
    c.spawn(spec).await;
    let (env, argv) = recorded(&d).await;
    let set = |key: &str| env.lines().any(|l| l.starts_with(&format!("{key}=")));
    assert!(set("HOME"), "env_remove was applied");
    assert!(!set("ANTHROPIC_API_KEY") && !set("CLAUDECODE"));
    let args: Vec<&str> = argv.lines().skip(2).collect();
    assert_eq!(args, ["--x-test", "v", "-n", "test"]);
    assert!(d.journal()["sessions"][0].get("optional").is_none());
}
