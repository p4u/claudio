//! Daemon tests for what a spawn carries beside its args (docs/daemon-protocol.md,
//! "Spawn env" and "Optional args"): the caps on `env_remove`, and the env a
//! session's process gets.

use super::*;

fn refused(reply: Msg) -> String {
    match reply {
        Msg::Error { message } => message,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bad_env_remove_is_refused() {
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
