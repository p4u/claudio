//! The runner's tests: its tasks run against a recording context and a
//! loopback proxy stub.

use std::sync::atomic::Ordering;
use std::time::Duration;

use super::*;
use crate::addon::Addons;
use crate::addons::vocdoni::bridge::Bridge;
use crate::addons::vocdoni::plugins::choices::PluginChoices;
use crate::addons::vocdoni::plugins::state::ApplyState;
use crate::addons::vocdoni::proxy::plugins::sample;
use crate::addons::vocdoni::test_support::{lock, TestCtx, TEAM};

/// Wait until `f` holds (5 s at most).
async fn until(what: &str, mut f: impl FnMut() -> bool) {
    let wait = async {
        while !f() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(5), wait)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
}

/// A runner, active, with `local` connected (the event gives it `ctx`).
fn running(ctx: &Arc<TestCtx>) -> Arc<Runner> {
    let runner = Arc::new(Runner::with_cache("/nonexistent/plugins.json".into()));
    runner.activate();
    let ctx: Arc<dyn AddonCtx> = ctx.clone();
    let up = AddonEvent::HostUp {
        host: "local".into(),
        features: vec!["addon:vocdoni".into()],
    };
    runner.on_event(&up, &ctx);
    runner
}

/// `kb` is listed and its manifest is here; no choices yet.
fn listed_kb(runner: &Arc<Runner>) {
    let list = PluginList {
        profile: "work".into(),
        fetched_at: 1,
        plugins: vec![sample("kb")],
    };
    let manifest = Manifest::parse(&TEAM.replace("\"team\"", "\"kb\"")).unwrap();
    let mut state = runner.state();
    state.on_list("work", PluginsFetch::Listed(list));
    state.on_manifest("kb", &"a".repeat(64), Ok(manifest));
}

#[tokio::test]
async fn runner_asks_a_host_once_choices_are_saved() {
    let ctx = Arc::new(TestCtx::default());
    let runner = running(&ctx);
    listed_kb(&runner);
    // An event before activation is ignored; a tick asks nothing unsaved.
    runner.on_event(&AddonEvent::Tick, &(ctx.clone() as Arc<dyn AddonCtx>));
    assert!(lock(&ctx.requests).is_empty(), "nothing before the setup");

    let choices = PluginChoices {
        disabled: vec!["mcp:kb".into()],
        ..PluginChoices::default()
    };
    runner.transition(|s| s.save_choices("kb", choices.clone()));
    until("the answer", || {
        matches!(
            runner.state().apply_state("local", "kb"),
            Some(ApplyState::Ready(_))
        )
    })
    .await;
    let requests = lock(&ctx.requests).clone();
    assert_eq!(requests.len(), 1, "{requests:?}");
    let (host, op, body) = &requests[0];
    assert_eq!((host.as_str(), op.as_str()), ("local", "apply"));
    assert_eq!(body["disabled"], serde_json::json!(["mcp:kb"]));
    assert_eq!(body["secrets"], serde_json::json!(["KB_TOKEN"]));
    // Saved through the addon's own section, then said.
    until("the save", || !lock(&ctx.notices).is_empty()).await;
    let saved = choices::from_table(&ctx.config("plugins"));
    assert_eq!(saved.get("kb"), Some(&choices));
    assert_eq!(*lock(&ctx.notices), ["plugin kb set up (saved)"]);
    assert!(ctx.changed.load(Ordering::SeqCst) > 0);
    // Answered: the next tick asks no one.
    runner.on_event(&AddonEvent::Tick, &(ctx.clone() as Arc<dyn AddonCtx>));
    assert_eq!(lock(&ctx.requests).len(), 1);
}

#[tokio::test]
async fn an_inactive_runner_ignores_the_runtimes_events() {
    let ctx: Arc<dyn AddonCtx> = Arc::new(TestCtx::default());
    let runner = Arc::new(Runner::with_cache("/nonexistent/plugins.json".into()));
    let up = AddonEvent::HostUp {
        host: "local".into(),
        features: vec!["addon:vocdoni".into()],
    };
    runner.on_event(&up, &ctx);
    assert!(runner.state().plugins.hosts.is_empty());
}

/// A loopback proxy whose `/v1/claudio/plugins` answers `status` (with a
/// one-plugin list on 200).
async fn proxy_answering(status: u16) -> String {
    let router = axum::Router::new().route(
        "/v1/claudio/plugins",
        axum::routing::get(move || async move {
            let code = axum::http::StatusCode::from_u16(status).unwrap();
            (code, r#"{"plugins":[]}"#)
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://127.0.0.1:{port}")
}

#[tokio::test]
async fn runner_drops_the_cache_on_401() {
    let dir = std::env::temp_dir().join(format!("claudio-runner-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("plugins.json");
    let cached = PluginList {
        profile: "work".into(),
        fetched_at: 1,
        plugins: vec![sample("kb")],
    };
    cache::save_cache_to(&path, &cached).unwrap();
    let profile = |url: String| Profile {
        url,
        token: "t".into(),
        first_party: false,
        email: None,
    };
    // Any other failure leaves the cached list.
    let p = profile(proxy_answering(500).await);
    let (fetch, notice) = fetch_list_with("work", &p, &path).await;
    assert_eq!((fetch, notice), (PluginsFetch::Unreachable, None));
    assert!(cache::load_cache_from(&path, "work").is_some());
    // A 401: the profile's entry (its secrets) goes.
    let p = profile(proxy_answering(401).await);
    let (fetch, notice) = fetch_list_with("work", &p, &path).await;
    assert_eq!((fetch, notice), (PluginsFetch::SignedOut, None));
    assert!(cache::load_cache_from(&path, "work").is_none());
    // A list is saved.
    let p = profile(proxy_answering(200).await);
    let (fetch, _) = fetch_list_with("work", &p, &path).await;
    assert!(matches!(fetch, PluginsFetch::Listed(_)), "{fetch:?}");
    assert!(cache::load_cache_from(&path, "work").is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

/// R21: a list over its caps is refused before it is cached: the cached
/// list stays as it was.
#[tokio::test]
async fn an_over_count_list_is_never_cached() {
    let dir = std::env::temp_dir().join(format!("claudio-runner-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("plugins.json");
    let cached = PluginList {
        profile: "work".into(),
        fetched_at: 1,
        plugins: vec![sample("kb")],
    };
    cache::save_cache_to(&path, &cached).unwrap();
    let many: Vec<serde_json::Value> = (0..33)
        .map(|i| serde_json::json!({"name": format!("p{i}"), "version": "1"}))
        .collect();
    let body = serde_json::json!({ "plugins": many }).to_string();
    let p = Profile {
        url: crate::addons::vocdoni::test_support::serve_chunked(body).await,
        token: "t".into(),
        first_party: false,
        email: None,
    };
    let (fetch, notice) = fetch_list_with("work", &p, &path).await;
    assert_eq!((fetch, notice), (PluginsFetch::Unreachable, None));
    assert_eq!(cache::load_cache_from(&path, "work"), Some(cached));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_bridge_finds_the_registered_vocdoni() {
    let addons = Addons::new(crate::addons::all()).unwrap();
    assert!(Bridge::of(&addons).is_some());
    assert!(Bridge::of(&Addons::empty()).is_none());
}
