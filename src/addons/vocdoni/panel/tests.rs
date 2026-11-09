//! The proxy panel's tests: each page's blocks from fixture stats (the
//! numbers of the stats popup's tests), the panel without a profile, and
//! the switch action against a loopback proxy stub.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use super::*;
use crate::addon::{Block, LaunchChoice};
use crate::addons::vocdoni::proxy::api::{
    CredentialInfo, CredentialUtilization, LimitInfo, ModelEntry, ModelsResponse,
    PoolHealthResponse, ProviderHealth, StatsModel, StatsResponse, StatsTotals,
};
use crate::addons::vocdoni::state::stats::{ProxyFetch, ProxyStatus};
use crate::addons::vocdoni::test_support::{lock, TestCtx};
use crate::proto::{SessionId, SessionKind};

fn model(name: &str, req: i64, err: i64, inp: i64, out: i64, cr: i64) -> StatsModel {
    StatsModel {
        model: name.into(),
        requests: req,
        errors: err,
        input_tokens: inp,
        output_tokens: out,
        cache_read: cr,
        cache_creation: cr / 20,
    }
}

fn stats(scale: i64) -> StatsResponse {
    let models = vec![
        model(
            "claude-opus-5-5[1m]",
            812 * scale,
            scale,
            8_100_000 * scale,
            301_000 * scale,
            60_200_000 * scale,
        ),
        model(
            "claude-sonnet-5-5",
            300 * scale,
            0,
            2_400_000 * scale,
            120_000 * scale,
            31_000_000 * scale,
        ),
        model(
            "claude-haiku-5-5",
            122 * scale,
            2 * scale,
            400_000 * scale,
            35_000 * scale,
            7_500_000 * scale,
        ),
    ];
    let mut totals = StatsTotals::default();
    for m in &models {
        totals.requests += m.requests;
        totals.errors += m.errors;
        totals.input_tokens += m.input_tokens;
        totals.output_tokens += m.output_tokens;
        totals.cache_read += m.cache_read;
        totals.cache_creation += m.cache_creation;
    }
    StatsResponse {
        version: 1,
        user_name: "pau".into(),
        period: "24h".into(),
        totals,
        by_model: models,
        limit: Some(LimitInfo {
            output_tokens: 5_000_000,
            window_seconds: 18_000,
            used_output_tokens: 900_000,
            used_pct: 0.18,
            blocked: false,
            blocked_until: None,
        }),
    }
}

fn provider(name: &str, status: &str) -> ProviderHealth {
    ProviderHealth {
        name: name.into(),
        status: status.into(),
    }
}

fn status() -> ProxyStatus {
    let fetch = ProxyFetch {
        stats: vec![
            (Window::H24, Ok(stats(1))),
            (Window::D7, Ok(stats(5))),
            (Window::D30, Ok(stats(18))),
        ],
        pool: Some(PoolHealthResponse {
            version: 1,
            providers: vec![
                provider("anthropic", "ok"),
                provider("google", "busy"),
                provider("glm", "unavailable"),
            ],
        }),
        models: Some(ModelsResponse {
            version: 1,
            refreshed_at: Some("2026-10-09T10:00:00Z".into()),
            data: vec![ModelEntry {
                id: "claude-opus-5-5[1m]".into(),
                display_name: "Claude Opus 5.5".into(),
                provider: "anthropic".into(),
                family: "opus".into(),
                max_input_tokens: 1_000_000,
                recommended_default: true,
            }],
        }),
    };
    let mut s = ProxyStatus::default();
    s.apply(fetch, 100);
    s
}

fn cred() -> SessionCredential {
    SessionCredential {
        credential: CredentialInfo {
            id: "cred_1".into(),
            label: "work-max".into(),
            provider: "anthropic".into(),
            plan: "max".into(),
        },
        utilization: Some(CredentialUtilization {
            five_hour_pct: Some(37.5),
            seven_day_pct: Some(12.0),
            captured_at: None,
        }),
        ..SessionCredential::default()
    }
}

fn row(tab: usize, label: &str, on_profile: bool, focused: bool) -> Row {
    Row {
        tab,
        label: label.into(),
        host: "local".into(),
        focused,
        on_profile,
        cred: on_profile.then(cred),
    }
}

/// Three sessions, two on `myproxy`, the first focused and asked about.
fn view() -> View {
    View {
        profile: "myproxy".into(),
        window: Window::H24,
        borrowed: false,
        can_switch: true,
        now: 7_201,
        session: Some(row(1, "claudio", true, true)),
        sessions: vec![
            row(1, "claudio", true, true),
            row(2, "direct", false, false),
            row(3, "docs", true, false),
        ],
    }
}

/// Every string of `blocks`, one per line.
fn flat(blocks: &[Block]) -> String {
    let mut out = Vec::new();
    for b in blocks {
        match b {
            Block::Text { text, .. } => out.push(text.clone()),
            Block::KeyValues { items } => {
                out.extend(items.iter().map(|(k, v)| format!("{k}: {v}")))
            }
            Block::Table { header, rows } => {
                out.push(header.join(" | "));
                out.extend(rows.iter().map(|r| r.join(" | ")));
            }
            Block::Gauge {
                label,
                used,
                limit,
                resets,
            } => out.push(format!("{label} {used}/{limit} {resets:?}")),
            Block::Bars { label, series } => {
                out.push(label.clone());
                out.extend(series.iter().map(|(k, v)| format!("{k} {v}")));
            }
        }
    }
    out.join("\n")
}

#[test]
fn each_page_has_the_popups_numbers() {
    let (v, s) = (view(), status());
    let expectations: [&[&str]; 5] = [
        &[
            "Account: pau · profile myproxy",
            "1,234 requests in the last 24h · 3 errors (0.2%)",
            "Cache hit: ",
            "Limit: output tokens per 5h 900000/5000000",
            "opus-5-5[1m] | ",
            "This session: claudio",
            "Credential: work-max (max) · 5h 37%",
            "press n to move this session",
        ],
        &[
            "Model | Req | Err",
            "opus-5-5[1m] | 812 | 1",
            "Total | 1,234",
        ],
        &[
            "Requests / day   ▲ +40% vs 7d avg",
            "24h 1234",
            "7d 881",
            "30d 740",
        ],
        &[
            "anthropic | ok",
            "google | busy",
            "Your limit",
            "per 5h",
            "Model catalogue (refreshed 2026-10-09 10:00 UTC)",
            "★ | opus-5-5[1m] | opus",
        ],
        &[
            "2 of 3 sessions use proxy profile myproxy",
            "▶ | 1 | claudio | local | work-max (max) · 5h 37%",
            " | 3 | docs | local",
        ],
    ];
    for (i, expect) in expectations.iter().enumerate() {
        let text = flat(&pages::page(i, &v, Some(&s)));
        for e in *expect {
            assert!(text.contains(e), "page {i} lacks {e:?}:\n{text}");
        }
    }
    // The Sessions page lists only sessions on the profile.
    assert!(!flat(&pages::page(4, &v, Some(&s))).contains("direct"));
    // A borrowed profile says so; an error stays above stale numbers.
    let mut borrowed = view();
    borrowed.borrowed = true;
    let mut failed = status();
    failed.error = Some("rate limited".into());
    let text = flat(&pages::page(0, &borrowed, Some(&failed)));
    assert!(text.starts_with("! rate limited"), "{text}");
    assert!(text.contains("this session runs direct") && text.contains("1,234 requests"));
    // Nothing fetched yet.
    let text = flat(&pages::page(1, &view(), None));
    assert_eq!(text, "No 24h data yet: press r to fetch.");
}

/// A runner whose profiles are `text`'s `[proxy]`.
fn runner_with(text: &str) -> Arc<Runner> {
    let dir = std::env::temp_dir().join(format!("claudio-panel-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path: PathBuf = dir.join("config.toml");
    std::fs::write(&path, text).unwrap();
    Arc::new(Runner::with_files(dir.join("plugins.json"), path))
}

fn request(sid: Option<SessionId>) -> PanelRequest {
    PanelRequest {
        id: DECL.id.into(),
        page: 0,
        window: None,
        sid,
    }
}

#[tokio::test]
async fn no_profile_says_no_proxy_configured() {
    let ctx: Arc<dyn AddonCtx> = Arc::new(TestCtx::default());
    let p = panel(runner_with(""), request(None), ctx).await;
    assert_eq!(p.title, "Proxy stats");
    let text = flat(&p.pages[0].blocks);
    assert!(text.starts_with("No proxy configured."), "{text}");
    assert!(text.contains("claudio proxy login"), "{text}");
    assert!(p.actions.is_empty());
}

/// A loopback proxy whose switch route records the conversation and the
/// token it was asked with, and answers 202.
async fn switch_stub() -> (String, Arc<Mutex<Vec<(String, String)>>>) {
    let seen: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    let record = Arc::clone(&seen);
    let router = axum::Router::new().route(
        "/v1/claudio/session/switch",
        axum::routing::post(
            move |headers: axum::http::HeaderMap, body: axum::Json<serde_json::Value>| {
                let record = Arc::clone(&record);
                async move {
                    let auth = headers["authorization"].to_str().unwrap_or("").to_owned();
                    let id = body["id"].as_str().unwrap_or("").to_owned();
                    lock(&record).push((id, auth));
                    (
                        axum::http::StatusCode::ACCEPTED,
                        r#"{"from":{"label":"work-max"},"to":{"label":"work-pro"}}"#,
                    )
                }
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (format!("http://127.0.0.1:{port}"), seen)
}

fn snap(sid: SessionId, profile: Option<&str>, conversation: Option<&str>) -> SessionSnap {
    SessionSnap {
        sid,
        host: "local".into(),
        kind: SessionKind::Claude,
        cwd: "/w".into(),
        label: "claudio".into(),
        conversation: conversation.map(str::to_owned),
        launch: profile.map(|p| LaunchChoice {
            addon: NAME.into(),
            option: p.into(),
            flags: Vec::new(),
        }),
        focused: true,
    }
}

#[tokio::test]
async fn switch_uses_the_sessions_own_profile_and_conversation() {
    let (url, seen) = switch_stub().await;
    let runner = runner_with(&format!(
        "[proxy]\ndefault = \"other\"\n[proxy.profiles.other]\nurl = \"http://127.0.0.1:1\"\n\
         token = \"other-token\"\n[proxy.profiles.lab]\nurl = \"{url}\"\ntoken = \"lab-token\"\n"
    ));
    let sid = SessionId::from_u128(4);
    let test = Arc::new(TestCtx::default());
    lock(&test.sessions).push(snap(sid, Some("lab"), Some("conv-1")));
    let ctx: Arc<dyn AddonCtx> = test.clone();
    let req = ActionRequest {
        panel: "proxy".into(),
        action: SWITCH.into(),
        sid: Some(sid),
    };
    let notice = action(Arc::clone(&runner), req, ctx).await.unwrap();
    assert_eq!(
        notice.text,
        "switch scheduled: work-max → work-pro (on the next message)"
    );
    assert_eq!(
        *lock(&seen),
        [("conv-1".to_owned(), "Bearer lab-token".to_owned())]
    );
}

#[tokio::test]
async fn the_switch_action_needs_a_conversation() {
    let runner = runner_with("");
    let sid = SessionId::from_u128(4);
    let test = Arc::new(TestCtx::default());
    lock(&test.sessions).push(snap(sid, Some("lab"), None));
    lock(&test.sessions).push(snap(SessionId::from_u128(5), None, Some("c")));
    let ctx: Arc<dyn AddonCtx> = test.clone();
    let ask = |sid: Option<SessionId>, action: &str| ActionRequest {
        panel: "proxy".into(),
        action: action.into(),
        sid,
    };
    let out = action(Arc::clone(&runner), ask(Some(sid), SWITCH), ctx.clone()).await;
    assert_eq!(out.unwrap_err(), "this session has no conversation yet");
    let direct = Some(SessionId::from_u128(5));
    let out = action(Arc::clone(&runner), ask(direct, SWITCH), ctx.clone()).await;
    assert_eq!(out.unwrap_err(), "this session runs direct");
    let out = action(Arc::clone(&runner), ask(None, SWITCH), ctx.clone()).await;
    assert_eq!(out.unwrap_err(), "no session to move");
    let out = action(runner, ask(Some(sid), "delete"), ctx).await;
    assert_eq!(out.unwrap_err(), "no such action");
    // Without a conversation the panel offers no switch.
    let mut v = view();
    v.can_switch = false;
    assert!(!flat(&pages::page(0, &v, Some(&status()))).contains("press n"));
}
