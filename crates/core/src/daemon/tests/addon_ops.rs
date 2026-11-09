//! Daemon tests for addon ops ([`crate::daemon::addon_ops`]): the features a
//! daemon lists, `AddonOp` replies from a task, and every refusal, with a
//! `TestAddon` named `echo` (ops `echo`, `sleep`, `big`, `fail`).

use serde_json::{json, Value};

use super::*;
use crate::addon::test_support::TestAddon;
use crate::addon::Addon;
use crate::daemon::addon_ops;

/// A daemon with `echo` (daemon ops) and `quiet` (none), ops under `timeout`.
fn start_with_echo(timeout: Duration) -> (TestDaemon, Arc<TestAddon>) {
    let dir = TestDaemon::new_dir();
    let echo = Arc::new(TestAddon::new("echo").with_daemon());
    let quiet = Arc::new(TestAddon::new("quiet").segments(Vec::new()));
    let addons = Addons::new(vec![Arc::clone(&echo) as Arc<dyn Addon>, quiet]).unwrap();
    let config = Config {
        addon_timeout: timeout,
        ..TestDaemon::config(&dir)
    };
    (TestDaemon::start_with_addons(dir, config, addons), echo)
}

fn op(addon: &str, op: &str, body: Value) -> Msg {
    Msg::AddonOp {
        addon: addon.into(),
        op: op.into(),
        body,
    }
}

fn error(message: &str) -> Msg {
    Msg::Error {
        message: message.into(),
    }
}

async fn features(d: &TestDaemon) -> Vec<String> {
    let mut c = Client::raw(&d.config.socket).await;
    match c.call(Msg::Hello(hello(PROTO))).await {
        Msg::Welcome(w) => w.features,
        other => panic!("expected welcome, got {other:?}"),
    }
}

/// The replies to `reqs`, in arrival order.
async fn replies(c: &mut Client, reqs: &[u64]) -> Vec<(u64, Msg)> {
    let mut out = Vec::new();
    while out.len() < reqs.len() {
        let got = c
            .until(|f| match f {
                Frame::Control(Envelope { req: Some(r), msg }) if reqs.contains(r) => {
                    Some((*r, msg.clone()))
                }
                _ => None,
            })
            .await;
        out.push(got);
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn welcome_lists_addon_features_only_for_daemon_addons() {
    let plain = TestDaemon::start().await;
    let listed = features(&plain).await;
    assert!(listed.iter().any(|f| f == "lens"), "{listed:?}");
    assert!(
        !listed.iter().any(|f| f.starts_with("addon:")),
        "{listed:?}"
    );

    let (d, _) = start_with_echo(addon_ops::TIMEOUT);
    let listed = features(&d).await;
    assert!(listed.iter().any(|f| f == "addon:echo"), "{listed:?}");
    assert!(!listed.iter().any(|f| f == "addon:quiet"), "{listed:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn addon_op_replies_from_a_task_beside_other_requests() {
    let (d, echo) = start_with_echo(addon_ops::TIMEOUT);
    let mut c = d.client().await;
    let slow = c.request(op("echo", "sleep", json!({"secs": 2}))).await;
    let ping = c.request(Msg::Ping).await;
    let got = replies(&mut c, &[slow, ping]).await;
    assert_eq!(
        got,
        [
            (ping, Msg::Pong),
            (
                slow,
                Msg::AddonReply {
                    body: json!({"slept": 2.0})
                }
            ),
        ]
    );
    let body = json!({"a": [1, "two", null]});
    assert_eq!(
        c.call(op("echo", "echo", body.clone())).await,
        Msg::AddonReply { body }
    );
    assert_eq!(
        c.call(op("echo", "fail", Value::Null)).await,
        error("echo failed")
    );
    assert_eq!(echo.calls(), ["daemon sleep", "daemon echo", "daemon fail"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn addon_op_refuses_unknown_addons_big_bodies_and_a_ninth_in_flight() {
    let (d, echo) = start_with_echo(addon_ops::TIMEOUT);
    let mut c = d.client().await;
    assert_eq!(
        c.call(op("nope", "echo", Value::Null)).await,
        error("no such addon nope")
    );
    // An addon without daemon ops is no daemon addon.
    assert_eq!(
        c.call(op("quiet", "echo", Value::Null)).await,
        error("no such addon quiet")
    );
    let big = Value::String("x".repeat(600 * 1024));
    assert_eq!(
        c.call(op("echo", "echo", big)).await,
        error("body over 512 KiB")
    );
    assert!(echo.calls().is_empty(), "{:?}", echo.calls());

    let mut reqs = Vec::new();
    for _ in 0..9 {
        reqs.push(c.request(op("echo", "sleep", json!({"secs": 3}))).await);
    }
    let got = replies(&mut c, &reqs).await;
    let busy = got.iter().filter(|(_, m)| *m == error("addon echo busy"));
    assert_eq!(busy.count(), 1, "{got:?}");
    let slept = got
        .iter()
        .filter(|(_, m)| matches!(m, Msg::AddonReply { .. }));
    assert_eq!(slept.count(), 8, "{got:?}");
    // The slots are free again.
    assert_eq!(
        c.call(op("echo", "echo", json!(1))).await,
        Msg::AddonReply { body: json!(1) }
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn addon_op_reply_over_the_cap_is_an_error() {
    let (d, _) = start_with_echo(addon_ops::TIMEOUT);
    let mut c = d.client().await;
    assert_eq!(
        c.call(op("echo", "big", Value::Null)).await,
        error("body over 512 KiB")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn addon_op_times_out() {
    let (d, _) = start_with_echo(Duration::from_secs(1));
    let mut c = d.client().await;
    let started = std::time::Instant::now();
    assert_eq!(
        c.call(op("echo", "sleep", json!({"secs": 200}))).await,
        error("addon echo timed out")
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    // The timed-out op gave its slot back.
    assert_eq!(
        c.call(op("echo", "echo", json!("again"))).await,
        Msg::AddonReply {
            body: json!("again")
        }
    );
}

/// A newer client's op this daemon does not know arrives as `Unknown` and
/// is refused, the path an older daemon takes for `AddonOp`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_op_from_a_newer_client_is_refused() {
    let d = TestDaemon::start().await;
    let mut c = d.client().await;
    let raw = br#"{"req":1,"op":"from_the_future","x":1}"#;
    c.send(Frame::Control(serde_json::from_slice(raw).unwrap()))
        .await;
    let reply = c
        .until(|f| match f {
            Frame::Control(env) if env.req == Some(1) => Some(env.msg.clone()),
            _ => None,
        })
        .await;
    assert_eq!(reply, error("unsupported op: unknown"));
}
