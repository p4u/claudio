//! Tests of the addon runtime with `TestAddon` and a fake daemon behind a
//! real `Client` (over `tokio::io::duplex`).

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use super::test_support::{fake_host, next_request, reply};
use super::*;
use crate::addon::test_support::TestAddon;
use crate::addon::{Addon, AddonError, Level};
use crate::proto::{Msg, SessionKind};

const WAIT: Duration = Duration::from_secs(5);

/// A scratch dir with `config.toml` holding `text`.
fn scratch(text: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("claudio-addons-rt-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    std::fs::write(&path, text).unwrap();
    (dir, path)
}

/// A runtime for `addons` with `limits`, its config at `path`.
fn runtime(
    addons: Vec<Arc<dyn Addon>>,
    path: &std::path::Path,
    limits: Limits,
) -> (AddonRuntime, RuntimeRx) {
    let addons = Arc::new(Addons::new(addons).unwrap());
    let opts = RuntimeOptions {
        config_path: path.to_path_buf(),
        watch_config: false,
    };
    AddonRuntime::start_with(addons, opts, limits)
}

/// The events `addon` has seen so far.
fn events(addon: &TestAddon) -> Vec<String> {
    addon
        .calls()
        .into_iter()
        .filter_map(|c| c.strip_prefix("event ").map(str::to_owned))
        .collect()
}

/// Wait until `addon` has seen `n` events (ticks excluded) and return them.
async fn seen(addon: &TestAddon, n: usize) -> Vec<String> {
    tokio::time::timeout(WAIT, async {
        loop {
            let got: Vec<String> = events(addon).into_iter().filter(|e| e != "Tick").collect();
            if got.len() >= n {
                return got;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("events in time")
}

fn snap(sid: u128, conversation: Option<&str>) -> SessionSnap {
    SessionSnap {
        sid: uuid::Uuid::from_u128(sid),
        host: "local".into(),
        kind: SessionKind::Claude,
        cwd: "/w".into(),
        label: "w".into(),
        conversation: conversation.map(str::to_owned),
        launch: None,
        focused: false,
    }
}

#[tokio::test]
async fn events_arrive_in_order_after_started() {
    let (dir, path) = scratch("");
    let echo = Arc::new(TestAddon::new("echo"));
    let (rt, _rx) = runtime(vec![echo.clone()], &path, Limits::default());
    let client = fake_host(&["addon:echo"], |_s| async {}).await;
    rt.host_up("local", client);
    rt.sessions_reset(vec![snap(1, None)]);
    rt.session_changed(snap(1, Some("c1")));
    rt.set_focused(&[uuid::Uuid::from_u128(1)]);
    rt.set_focused(&[uuid::Uuid::from_u128(1)]); // no change, no event
    rt.claude_installed("local");
    rt.session_removed(uuid::Uuid::from_u128(1));
    rt.host_down("local");
    rt.host_down("local"); // already down
    let sid = uuid::Uuid::from_u128(1);
    let got = seen(&echo, 8).await;
    assert_eq!(
        got,
        [
            "Started".to_owned(),
            "HostUp { host: \"local\", features: [\"addon:echo\"] }".to_owned(),
            format!("SessionAdded {{ sid: {sid}, conversation: None }}"),
            format!("SessionChanged {{ sid: {sid}, conversation: Some(\"c1\") }}"),
            format!("SessionChanged {{ sid: {sid}, conversation: Some(\"c1\") }}"),
            "ClaudeInstalled { host: \"local\" }".to_owned(),
            format!("SessionRemoved {{ sid: {sid} }}"),
            "HostDown { host: \"local\" }".to_owned(),
        ]
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(seen(&echo, 8).await.len(), 8, "nothing more");
    rt.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn ticks_follow_started() {
    let (dir, path) = scratch("");
    let echo = Arc::new(TestAddon::new("echo"));
    let limits = Limits {
        tick: Duration::from_millis(20),
        ..Limits::default()
    };
    let (rt, _rx) = runtime(vec![echo.clone()], &path, limits);
    tokio::time::timeout(WAIT, async {
        while events(&echo).iter().filter(|e| *e == "Tick").count() < 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(events(&echo)[0], "Started");
    rt.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_request_needs_the_hosts_addon_feature() {
    let (dir, path) = scratch("");
    let echo = Arc::new(TestAddon::new("echo"));
    let (rt, _rx) = runtime(vec![echo.clone()], &path, Limits::default());
    let ctx = rt.ctx("echo");
    // No such host yet.
    let err = ctx.request("local", "echo", json!(1)).await.unwrap_err();
    assert_eq!(err, AddonError::NotConnected("local".into()));
    // A daemon without `addon:echo`: never asked.
    let old = fake_host(&["lens"], |mut s| async move {
        assert!(
            next_request(&mut s).await.is_none(),
            "an old daemon got a request"
        );
    })
    .await;
    rt.host_up("old", old);
    assert!(!ctx.host_has("old"));
    let err = ctx.request("old", "echo", json!(1)).await.unwrap_err();
    assert_eq!(err, AddonError::Missing("old".into()));
    // A daemon with it gets the op and its reply comes back.
    let new = fake_host(&["addon:echo"], |mut s| async move {
        let (req, msg) = next_request(&mut s).await.unwrap();
        let Msg::AddonOp { addon, op, body } = msg else {
            panic!("not an addon op");
        };
        assert_eq!((addon.as_str(), op.as_str()), ("echo", "ping"));
        reply(&mut s, req, Msg::AddonReply { body }).await;
    })
    .await;
    rt.host_up("new", new);
    assert!(ctx.host_has("new"));
    assert_eq!(
        ctx.request("new", "ping", json!({"a": 1})).await,
        Ok(json!({"a": 1}))
    );
    let hosts = ctx.hosts();
    assert_eq!(hosts.len(), 2);
    assert!(hosts.iter().all(|h| h.connected && !h.claude));
    // Gone: not connected any more.
    rt.host_down("new");
    assert!(!ctx.host_has("new"));
    let err = ctx.request("new", "ping", json!(1)).await.unwrap_err();
    assert_eq!(err, AddonError::NotConnected("new".into()));
    rt.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn request_bodies_are_capped_both_ways() {
    let (dir, path) = scratch("");
    let echo = Arc::new(TestAddon::new("echo"));
    let (rt, _rx) = runtime(vec![echo.clone()], &path, Limits::default());
    let asked = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&asked);
    let client = fake_host(&["addon:echo"], move |mut s| async move {
        while let Some((req, _)) = next_request(&mut s).await {
            counter.fetch_add(1, Ordering::SeqCst);
            let big = Value::String("x".repeat(600 << 10));
            reply(&mut s, req, Msg::AddonReply { body: big }).await;
        }
    })
    .await;
    rt.host_up("local", client);
    let ctx = rt.ctx("echo");
    let big = Value::String("x".repeat(600 << 10));
    assert_eq!(
        ctx.request("local", "x", big).await,
        Err(AddonError::TooLarge)
    );
    assert_eq!(
        asked.load(Ordering::SeqCst),
        0,
        "an oversized body is never sent"
    );
    assert_eq!(
        ctx.request("local", "x", json!(1)).await,
        Err(AddonError::TooLarge)
    );
    assert_eq!(asked.load(Ordering::SeqCst), 1);
    rt.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn request_maps_daemon_errors_and_times_out() {
    let (dir, path) = scratch("");
    let echo = Arc::new(TestAddon::new("echo"));
    let limits = Limits {
        request_timeout: Duration::from_millis(200),
        ..Limits::default()
    };
    let (rt, _rx) = runtime(vec![echo.clone()], &path, limits);
    let client = fake_host(&["addon:echo"], |mut s| async move {
        let (req, _) = next_request(&mut s).await.unwrap();
        let error = Msg::Error {
            message: "addon echo busy".into(),
        };
        reply(&mut s, req, error).await;
        // The second is never answered.
        let _ = next_request(&mut s).await;
        tokio::time::sleep(Duration::from_secs(30)).await;
    })
    .await;
    rt.host_up("local", client);
    let ctx = rt.ctx("echo");
    let err = ctx.request("local", "x", json!(1)).await.unwrap_err();
    assert_eq!(err, AddonError::Daemon("addon echo busy".into()));
    let err = ctx.request("local", "x", json!(1)).await.unwrap_err();
    assert_eq!(err, AddonError::Timeout);
    rt.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Sets its flag when dropped: a task's future was dropped (ended,
/// timed out or aborted).
struct Dropped(Arc<AtomicUsize>);

impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

async fn until(what: &str, f: impl Fn() -> bool) {
    tokio::time::timeout(WAIT, async {
        while !f() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{what}"));
}

#[tokio::test]
async fn spawned_tasks_are_bounded_timed_out_and_aborted_at_shutdown() {
    let (dir, path) = scratch("");
    let echo = Arc::new(TestAddon::new("echo"));
    let limits = Limits {
        tasks: 2,
        task_timeout: Duration::from_millis(300),
        ..Limits::default()
    };
    let (rt, _rx) = runtime(vec![echo.clone()], &path, limits);
    let ctx = rt.ctx("echo");
    let (started, dropped) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let forever = || {
        let (started, guard) = (Arc::clone(&started), Dropped(Arc::clone(&dropped)));
        Box::pin(async move {
            let _guard = guard;
            started.fetch_add(1, Ordering::SeqCst);
            std::future::pending::<()>().await;
        })
    };
    for _ in 0..3 {
        ctx.spawn(forever());
    }
    until("two tasks start", || started.load(Ordering::SeqCst) == 2).await;
    // The third was dropped unstarted (its guard went with it).
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    // Both time out.
    until("the tasks time out", || dropped.load(Ordering::SeqCst) == 3).await;
    // Room again; then shutdown aborts what runs, and nothing starts after.
    ctx.spawn(forever());
    until("a new task starts", || started.load(Ordering::SeqCst) == 3).await;
    rt.shutdown();
    rt.shutdown(); // idempotent
    until("shutdown aborts it", || dropped.load(Ordering::SeqCst) == 4).await;
    ctx.spawn(forever());
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(started.load(Ordering::SeqCst), 3);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn config_is_limited_to_declared_sections() {
    let text = "[team]\nurl = \"https://e.org\"\n[other]\nx = 1\n[extensions]\nteam = false\n";
    let (dir, path) = scratch(text);
    let echo = Arc::new(TestAddon::new("echo").sections(&["team"]));
    let (rt, _rx) = runtime(vec![echo.clone()], &path, Limits::default());
    seen(&echo, 1).await; // Started: the config is read
    let ctx = rt.ctx("echo");
    assert_eq!(ctx.config("team")["url"].as_str(), Some("https://e.org"));
    assert!(ctx.config("other").is_empty());
    assert!(ctx.config("extensions").is_empty());
    assert_eq!(ctx.extension_on("team"), Some(false));
    assert_eq!(ctx.extension_on("nope"), None);
    // A change on disk is seen after `config_changed`.
    std::fs::write(&path, "[team]\nurl = \"https://f.org\"\n").unwrap();
    rt.config_changed();
    assert_eq!(seen(&echo, 2).await[1], "ConfigChanged");
    assert_eq!(ctx.config("team")["url"].as_str(), Some("https://f.org"));
    rt.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn edit_config_refuses_undeclared_sections() {
    let (dir, path) = scratch("# mine\n[team]\nurl = \"a\"\n");
    let echo = Arc::new(TestAddon::new("echo").sections(&["team"]));
    let (rt, _rx) = runtime(vec![echo.clone()], &path, Limits::default());
    seen(&echo, 1).await;
    let ctx = rt.ctx("echo");
    let err = ctx
        .edit_config(
            "ui",
            Box::new(|t| {
                t.insert("notify".into(), toml::Value::Boolean(false));
            }),
        )
        .await
        .unwrap_err();
    assert_eq!(err, "[ui] is not echo's");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "# mine\n[team]\nurl = \"a\"\n"
    );
    ctx.edit_config(
        "team",
        Box::new(|t| {
            t.insert("url".into(), toml::Value::String("b".into()));
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "# mine\n[team]\nurl = \"b\"\n"
    );
    assert_eq!(ctx.config("team")["url"].as_str(), Some("b"));
    assert_eq!(seen(&echo, 2).await[1], "ConfigChanged");
    rt.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

async fn next(rx: &mut RuntimeRx) -> RuntimeOut {
    tokio::time::timeout(WAIT, rx.recv())
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn changed_is_coalesced() {
    let (dir, path) = scratch("");
    let a = Arc::new(TestAddon::new("a"));
    let b = Arc::new(TestAddon::new("b"));
    let (rt, mut rx) = runtime(vec![a, b], &path, Limits::default());
    let (ca, cb) = (rt.ctx("a"), rt.ctx("b"));
    ca.changed();
    cb.changed();
    ca.changed();
    assert_eq!(next(&mut rx).await, RuntimeOut::Changed { addon: "a" });
    assert_eq!(next(&mut rx).await, RuntimeOut::Changed { addon: "b" });
    let nothing = tokio::time::timeout(Duration::from_millis(50), rx.recv()).await;
    assert!(nothing.is_err(), "one Changed per addon until taken");
    ca.changed();
    assert_eq!(next(&mut rx).await, RuntimeOut::Changed { addon: "a" });
    // Once stopped, the receiver ends.
    rt.shutdown();
    assert_eq!(rx.recv().await, None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn notices_are_cleaned() {
    let (dir, path) = scratch("");
    let echo = Arc::new(TestAddon::new("echo"));
    let (rt, mut rx) = runtime(vec![echo], &path, Limits::default());
    let ctx = rt.ctx("echo");
    ctx.notify("a\x1b[31mb\x07\nc\u{9b}d".into());
    let RuntimeOut::Notice { addon, notice } = next(&mut rx).await else {
        panic!("not a notice");
    };
    assert_eq!(addon, "echo");
    assert_eq!(notice.level, Level::Info);
    assert!(
        !notice.text.chars().any(|c| c.is_control()),
        "{:?}",
        notice.text
    );
    assert!(notice.text.contains('a') && notice.text.contains('d'));
    ctx.offer_checklist(format!("setup\x07{}", "x".repeat(200)));
    let RuntimeOut::Offer { checklist, .. } = next(&mut rx).await else {
        panic!("not an offer");
    };
    assert!(checklist.starts_with("setup") && checklist.len() <= 64);
    rt.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_full_queue_drops_the_oldest_notice() {
    let (dir, path) = scratch("");
    let echo = Arc::new(TestAddon::new("echo"));
    let (rt, mut rx) = runtime(vec![echo], &path, Limits::default());
    let ctx = rt.ctx("echo");
    for i in 0..300 {
        ctx.notify(format!("n{i}"));
    }
    let RuntimeOut::Notice { notice, .. } = next(&mut rx).await else {
        panic!("not a notice");
    };
    assert_eq!(notice.text, "n44");
    rt.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn dropping_the_last_handle_stops_the_runtime() {
    let (dir, path) = scratch("");
    let echo = Arc::new(TestAddon::new("echo"));
    let (rt, mut rx) = runtime(vec![echo], &path, Limits::default());
    let ctx = rt.ctx("echo");
    let dropped = Arc::new(AtomicUsize::new(0));
    let guard = Dropped(Arc::clone(&dropped));
    ctx.spawn(Box::pin(async move {
        let _guard = guard;
        std::future::pending::<()>().await;
    }));
    drop(rt);
    until("the task is aborted", || {
        dropped.load(Ordering::SeqCst) == 1
    })
    .await;
    assert_eq!(rx.recv().await, None);
    // The context outlives the runtime harmlessly.
    ctx.notify("late".into());
    assert!(ctx.sessions().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Config reads started so far (the first is the one before `Started`).
fn reads(rt: &AddonRuntime) -> usize {
    rt.inner.state().reload.reads
}

/// The `ConfigChanged` events `addon` has seen.
fn config_changes(addon: &TestAddon) -> usize {
    events(addon)
        .iter()
        .filter(|e| *e == "ConfigChanged")
        .count()
}

/// A runtime whose config reads pause `pause` before their result is
/// applied, with its `Started` seen.
async fn slow_reads(text: &str, pause: u64) -> (PathBuf, PathBuf, Arc<TestAddon>, AddonRuntime) {
    let (dir, path) = scratch(text);
    let echo = Arc::new(TestAddon::new("echo").sections(&["team"]));
    let limits = Limits {
        reload_pause: Duration::from_millis(pause),
        ..Limits::default()
    };
    let (rt, _rx) = runtime(vec![echo.clone()], &path, limits);
    seen(&echo, 1).await;
    (dir, path, echo, rt)
}

/// Ten notifications while a read runs make one more read, not ten
/// (single flight, coalesced; A10, R16).
#[tokio::test]
async fn config_reloads_are_single_flight_and_coalesced() {
    let (dir, path, echo, rt) = slow_reads("[team]\nurl = \"a\"\n", 100).await;
    std::fs::write(&path, "[team]\nurl = \"b\"\n").unwrap();
    for _ in 0..10 {
        rt.config_changed();
    }
    until("two re-reads applied", || config_changes(&echo) == 2).await;
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(reads(&rt), 3, "the first read, then one, then one more");
    assert_eq!(config_changes(&echo), 2);
    assert_eq!(rt.ctx("echo").config("team")["url"].as_str(), Some("b"));
    // Idle again: the next notification reads again.
    rt.config_changed();
    until("a third re-read", || config_changes(&echo) == 3).await;
    assert_eq!(reads(&rt), 4);
    rt.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A read that started before an addon's edit finishes after it: its
/// older file is dropped (generation check), never put back over the
/// edit, and one more read follows.
#[tokio::test]
async fn an_older_read_never_overwrites_a_newer_edit() {
    let (dir, _path, echo, rt) = slow_reads("[team]\nurl = \"a\"\n", 300).await;
    let ctx = rt.ctx("echo");
    rt.config_changed();
    until("the read started", || reads(&rt) == 2).await;
    tokio::time::sleep(Duration::from_millis(50)).await; // it has read "a"
    ctx.edit_config(
        "team",
        Box::new(|t| {
            t.insert("url".into(), toml::Value::String("b".into()));
        }),
    )
    .await
    .unwrap();
    // Through the stale read's end and the next read's.
    let deadline = tokio::time::Instant::now() + Duration::from_millis(900);
    while tokio::time::Instant::now() < deadline {
        assert_eq!(ctx.config("team")["url"].as_str(), Some("b"), "stale");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(reads(&rt), 3, "one more read after the edit");
    assert_eq!(config_changes(&echo), 2, "the edit, then the fresh read");
    rt.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A config over the 1 MiB cap is not read: the last good table stays and
/// no `ConfigChanged` goes out (C2).
#[tokio::test]
async fn an_oversized_config_keeps_the_last_good_one() {
    let (dir, path, echo, rt) = slow_reads("[team]\nurl = \"a\"\n", 0).await;
    let mut big = String::from("[team]\nurl = \"big\"\n");
    while big.len() as u64 <= reload::CONFIG_MAX {
        big.push_str("# padding padding padding padding padding padding padding\n");
    }
    std::fs::write(&path, big).unwrap();
    rt.config_changed();
    until("the read ran", || {
        reads(&rt) == 2 && !rt.inner.state().reload.is_running()
    })
    .await;
    assert_eq!(rt.ctx("echo").config("team")["url"].as_str(), Some("a"));
    assert_eq!(config_changes(&echo), 0);
    rt.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Shutdown during a read: its result is never applied and later
/// notifications read nothing.
#[tokio::test]
async fn shutdown_drops_a_reload_in_flight() {
    let (dir, path, echo, rt) = slow_reads("[team]\nurl = \"a\"\n", 200).await;
    std::fs::write(&path, "[team]\nurl = \"b\"\n").unwrap();
    rt.config_changed();
    until("the read started", || reads(&rt) == 2).await;
    rt.shutdown();
    rt.config_changed();
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(reads(&rt), 2);
    assert_eq!(config_changes(&echo), 0);
    assert_eq!(rt.ctx("echo").config("team")["url"].as_str(), Some("a"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn rows_are_cleaned_and_a_toggle_is_written_before_the_addon_hears_it() {
    let (dir, path) = scratch("# mine\n");
    let row = crate::addon::ExtensionRow {
        name: "kb".into(),
        description: "evil\u{1b}[31m red".into(),
        default_on: true,
        refused: None,
        details: vec!["one\ntwo".into()],
        setup: Some("kb".into()),
    };
    let a = Arc::new(TestAddon::new("a").rows(vec![row]));
    let (rt, mut rx) = runtime(vec![a.clone()], &path, Limits::default());
    let rows = rt.extension_rows();
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].0, rows[0].1.name.as_str()), ("a", "kb"));
    assert!(!rows[0].1.description.contains('\u{1b}'), "{rows:?}");
    assert!(!rows[0].1.details[0].contains('\n'), "{rows:?}");

    assert_eq!(rt.toggle_row("a", "kb".into(), false).await, Ok(None));
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text, "# mine\n\n[extensions]\nkb = false\n");
    assert!(a
        .calls()
        .contains(&"row_action kb Toggled(false)".to_owned()));
    // The core's write is re-read: the addon and the front-end hear of it.
    loop {
        if next(&mut rx).await == RuntimeOut::ConfigChanged {
            break;
        }
    }
    assert_eq!(rt.ctx("a").extension_on("kb"), Some(false));

    assert_eq!(rt.refresh_rows().await, [Ok(None)]);
    assert!(a.calls().contains(&"row_action kb Refresh".to_owned()));
    assert!(rt.ctx("a").core_extensions().is_empty());
    rt.set_core_extensions(vec!["latex".into()]);
    assert_eq!(rt.ctx("a").core_extensions(), ["latex"]);
    let missing = rt.row_action("nope", "kb".into(), crate::addon::RowAction::Refresh);
    assert_eq!(missing.await, Err("no addon nope".to_owned()));
    rt.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}
