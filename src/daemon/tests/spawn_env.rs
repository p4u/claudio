//! Daemon tests for what a spawn carries beside its args (docs/daemon-protocol.md,
//! "Spawn env" and "Optional args"): the caps on `env_remove` and
//! `optional`, the journaled optional flags, and the env a session's
//! process gets.

use super::*;

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
