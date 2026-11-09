//! `claudio web` with the Vocdoni addon (features: vocdoni, web): proxy and
//! plugin secrets kept out of the browser, the addon's panels, checklists and
//! launch options, against a fake claude-proxy. Run single-threaded:
//! `cargo test --test vocdoni_web -- --test-threads=1`.

#[path = "../../../crates/tui/tests/common/mod.rs"]
mod common;
#[path = "../../web/tests/web_harness/mod.rs"]
mod web_harness;

use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::{fs, thread};

use common::{ManagerHarness, DAEMON_WAIT, WAIT};
use serde_json::{json, Value};
use web_harness::*;

// ── Spawn composition: proxy env, plugins, extensions, snippets ──────────────

const PLUGIN: &str = "vocdoni-test";

const PROXY_TOKEN: &str = "tok-SECRET-proxy-7f3a9c";

const PLUGIN_SECRET: &str = "plugin-SECRET-value-51d2e8";

const MANIFEST: &str = r#"format = 1
name = "vocdoni-test"
version = "1.0.0"
description = "Test setup"
instructions = "Manifest instructions for the test."

[[mcp]]
name = "kb"
type = "http"
url = "https://kb.example.org/mcp"
headers = { Authorization = "Bearer ${VOCDONI_TEST_TOKEN}" }
secrets = ["VOCDONI_TEST_TOKEN"]

[[agent]]
name = "reviewer"
description = "Reviews"
prompt = "Review the change."
"#;

/// A fake claude-proxy on loopback: `/v1/claudio/config`, `/plugins`
/// (PLUGIN, whose env holds PLUGIN_SECRET), the manifest, `/me/stats`
/// (any period) and `/pool/health`; anything else 404. Its `host:port`.
fn fake_proxy() -> String {
    use sha2::Digest;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind fake proxy");
    let addr = listener.local_addr().unwrap().to_string();
    let sha = format!("{:x}", sha2::Sha256::digest(MANIFEST.as_bytes()));
    let plugins = json!({"plugins": [{
        "name": PLUGIN, "version": "1.0.0",
        "bundle": {"url": format!("http://{addr}/{PLUGIN}.toml"), "sha256": sha},
        "env": {"VOCDONI_TEST_TOKEN": PLUGIN_SECRET}}]})
    .to_string();
    let stats = json!({"version": 1, "user_name": "tester", "period": "24h",
        "totals": {"requests": 12, "errors": 1, "input_tokens": 3400, "output_tokens": 5600, "cache_read": 0, "cache_creation": 0},
        "by_model": [{"model": "claude-opus-5-5", "requests": 12, "errors": 1, "input_tokens": 3400, "output_tokens": 5600, "cache_read": 0, "cache_creation": 0}],
        "limit": null})
    .to_string();
    let config = json!({"version": 1, "env": {}}).to_string();
    let pool =
        json!({"version": 1, "providers": [{"name": "anthropic", "status": "ok"}]}).to_string();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap_or(0) == 1 {
                head.push(byte[0]);
            }
            let line = String::from_utf8_lossy(&head).into_owned();
            let path = line
                .split(' ')
                .nth(1)
                .unwrap_or("")
                .split('?')
                .next()
                .unwrap_or("");
            let (status, reply) = match path {
                "/v1/claudio/config" => ("200 OK", config.as_str()),
                "/v1/claudio/plugins" => ("200 OK", plugins.as_str()),
                "/v1/claudio/me/stats" => ("200 OK", stats.as_str()),
                "/v1/claudio/pool/health" => ("200 OK", pool.as_str()),
                p if p == format!("/{PLUGIN}.toml") => ("200 OK", MANIFEST),
                _ => ("404 Not Found", "{}"),
            };
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                reply.len()
            );
        }
    });
    addr
}

/// A fake claude that records the env vars these tests assert on to
/// `env_file` (never the whole env, never to the terminal;
/// [`common::record_env_sh`]), lists `--agents`/`--mcp-config` in `--help`,
/// then prints the banner and echoes lines.
fn write_env_fake_claude(h: &ManagerHarness, env_file: &std::path::Path) {
    let record = common::record_env_sh(
        &env_file.display().to_string(),
        &[
            "ANTHROPIC_AUTH_TOKEN",
            "ANTHROPIC_API_KEY",
            "VOCDONI_TEST_TOKEN",
        ],
        &["CLAUDECODE"],
    );
    let script = format!(
        r#"#!/bin/sh
[ "$1" = "--version" ] && echo 'claude 0.0.0-fake' && exit 0
if [ "$1" = "--help" ]; then printf '  --agents <json>  Agents\n  --mcp-config <configs...>  MCP\n'; exit 0; fi
{record}printf '%s' "$2" > .fake-settings.json
echo "FAKE_CLAUDE_BANNER pid=$$"
while IFS= read -r line; do printf 'line:%s\n' "$line"; done
"#
    );
    fs::write(&h.fake_claude, script).unwrap();
    fs::set_permissions(&h.fake_claude, fs::Permissions::from_mode(0o755)).unwrap();
}

/// Poll `op` until `pick` accepts its data.
async fn until_req(tab: &mut Tab, op: &str, args: Value, pick: impl Fn(&Value) -> bool) -> Value {
    let deadline = std::time::Instant::now() + DAEMON_WAIT;
    loop {
        if let Ok(data) = tab.req(op, args.clone()).await {
            if pick(&data) {
                return data;
            }
        }
        assert!(std::time::Instant::now() < deadline, "{op} never satisfied");
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

/// The env the fake claude of the next launch dumps to `env_file`
/// (removed first by the caller).
async fn launched_env(env_file: &std::path::Path) -> String {
    let deadline = std::time::Instant::now() + WAIT;
    loop {
        if let Ok(env) = fs::read_to_string(env_file) {
            return env;
        }
        assert!(std::time::Instant::now() < deadline, "claude never started");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// `claudio web` with the fake proxy as CLAUDIO_PROXY_URL (profile `env`).
fn web_with_proxy(h: ManagerHarness, proxy: &str) -> Web {
    let url = format!("{PROXY_TOKEN}@{proxy}");
    Web::start_with(h, &[], |cmd| {
        cmd.env("CLAUDIO_PROXY_URL", url);
        cmd.env("CLAUDIO_TEST_LOOPBACK_HTTP", "1");
    })
}

/// AC3: the proxy token and a plugin's secret reach claude's env and never
/// the browser (no WS frame, no HTTP body) nor the server's log.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_spawn_keeps_proxy_and_plugin_secrets_out_of_the_browser() {
    let h = ManagerHarness::new();
    let env_file = h.root.join("claude-env.txt");
    write_env_fake_claude(&h, &env_file);
    let config = h.config_home.join("claudio/config.toml");
    let mut text = fs::read_to_string(&config).unwrap();
    text.push_str(&format!(
        "\n[plugins.{PLUGIN}]\ndisabled = []\nreviewed = \"1.0.0\"\n"
    ));
    fs::write(&config, text).unwrap();
    let proxy = fake_proxy();
    let dir = h.dirs[0].clone();
    let web = Web::start_with(h, &[], |cmd| {
        cmd.env("CLAUDIO_PROXY_URL", format!("{PROXY_TOKEN}@{proxy}"));
        cmd.env("CLAUDIO_TEST_LOOPBACK_HTTP", "1");
    });
    let cookie = web.login();
    let mut tab = Tab::open(&web, &cookie).await;
    assert!(tab.snapshot.get("profiles").is_none(), "version 1's");
    tab.connected("local").await;
    let list = tab.req("addons.list", json!({})).await.unwrap();
    assert_eq!(vocdoni_of(&list)["launch_options"][0]["id"], "env");
    host_ready(&mut tab).await;
    let reply = tab
        .req(
            "session.spawn",
            json!({"host": "local", "cwd": dir, "rows": 24, "cols": 80, "launch": {"pick": "default"}}),
        )
        .await
        .expect("spawn");
    let sid = reply["sid"].as_str().unwrap().to_owned();
    let deadline = std::time::Instant::now() + WAIT;
    let env = loop {
        if let Ok(env) = fs::read_to_string(&env_file) {
            break env;
        }
        assert!(std::time::Instant::now() < deadline, "claude never started");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };
    assert!(
        env.contains(&format!("ANTHROPIC_AUTH_TOKEN={PROXY_TOKEN}")),
        "the proxy token reaches claude"
    );
    assert!(
        env.contains(&format!("VOCDONI_TEST_TOKEN={PLUGIN_SECRET}")),
        "the plugin secret reaches claude"
    );

    // Everything a browser can see about this session and its plugins.
    tab.req(
        "session.attach",
        json!({"sid": sid, "rows": 24, "cols": 80}),
    )
    .await
    .unwrap();
    tab.output_until(&sid, "FAKE_CLAUDE_BANNER").await;
    for (op, args) in [
        ("hosts.list", json!({})),
        ("addons.list", json!({})),
        ("addons.rows", json!({})),
        (
            "addons.checklist",
            json!({"addon": "vocdoni", "checklist": PLUGIN}),
        ),
        ("extensions.list", json!({})),
        (
            "addons.panel",
            json!({"addon": "vocdoni", "panel": "proxy", "window": "24h"}),
        ),
        (
            "addons.panel",
            json!({"addon": "vocdoni", "panel": "proxy", "sid": sid, "page": 4}),
        ),
    ] {
        tab.req(op, args)
            .await
            .unwrap_or_else(|e| panic!("{op}: {e}"));
    }
    let second = Tab::open(&web, &cookie).await; // a snapshot with the session in it
    for frame in tab.seen.iter().chain(second.seen.iter()) {
        let text = String::from_utf8_lossy(frame);
        assert!(
            !text.contains(PROXY_TOKEN),
            "the proxy token in a frame: {text}"
        );
        assert!(
            !text.contains(PLUGIN_SECRET),
            "the plugin secret in a frame: {text}"
        );
    }
    for path in ["/", "/index.html"] {
        let body = web.get(path, &[("Accept-Encoding", "identity")]).body;
        assert!(!body.contains(PROXY_TOKEN) && !body.contains(PLUGIN_SECRET));
    }
    let log = fs::read_to_string(web.h().root.join("web.log")).unwrap();
    assert!(
        !log.contains(PROXY_TOKEN) && !log.contains(PLUGIN_SECRET),
        "a secret in the server log"
    );
}

/// A web spawn goes through the launch composer whatever its route: with
/// the proxy, the inherited `ANTHROPIC_API_KEY` is removed (`env_remove`)
/// and the proxy token set; direct, the key is kept. Never direct for the
/// default profile.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_spawn_uses_the_composer_env_remove() {
    let h = ManagerHarness::new();
    let env_file = h.root.join("claude-env.txt");
    write_env_fake_claude(&h, &env_file);
    let dirs = h.dirs.clone();
    let proxy = fake_proxy();
    let web = web_with_proxy(h, &proxy);
    let cookie = web.login();
    let mut tab = Tab::open(&web, &cookie).await;
    tab.connected("local").await;
    let spawn = |dir: &std::path::PathBuf, pick: &str| {
        json!({"host": "local", "cwd": dir, "rows": 24, "cols": 80,
        "launch": {"pick": pick}, "defer": [{"addon": "vocdoni", "id": PLUGIN}]})
    };
    tab.req("session.spawn", spawn(&dirs[0], "default"))
        .await
        .expect("spawn through the proxy");
    let env = launched_env(&env_file).await;
    assert!(env.contains(&format!("ANTHROPIC_AUTH_TOKEN={PROXY_TOKEN}")));
    assert!(
        !env.contains("ANTHROPIC_API_KEY="),
        "the inherited API key reached a proxied session"
    );
    fs::remove_file(&env_file).unwrap();
    tab.req("session.spawn", spawn(&dirs[1], "direct"))
        .await
        .expect("spawn direct");
    let env = launched_env(&env_file).await;
    assert!(
        env.contains("ANTHROPIC_API_KEY=test-key-not-real"),
        "{}",
        env.len()
    );
    assert!(!env.contains("ANTHROPIC_AUTH_TOKEN="));
    // First-party without a proxy is refused, never started.
    let mut bad = spawn(&dirs[1], "direct");
    bad["launch"]["flags"] = json!(["first-party"]);
    let err = tab.req("session.spawn", bad).await.unwrap_err();
    assert_eq!(err, "bad request: launch: flags need an addon's option");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_plugin_without_choices_is_left_out_until_saved() {
    let h = ManagerHarness::new();
    let env_file = h.root.join("claude-env.txt");
    write_env_fake_claude(&h, &env_file);
    let dirs = h.dirs.clone();
    let proxy = fake_proxy();
    let web = web_with_proxy(h, &proxy);
    let cookie = web.login();
    let mut tab = Tab::open(&web, &cookie).await;
    tab.connected("local").await;
    let checklist = plugin_checklist(&mut tab).await;
    assert_eq!(checklist["id"], PLUGIN);
    // No choices saved: the launch asks for the review and starts nothing.
    let reply = tab
        .req("session.spawn", v2_spawn(&dirs[0], &[]))
        .await
        .expect("spawn");
    assert_eq!(reply["review"]["id"], PLUGIN, "{reply}");
    assert!(!env_file.exists(), "nothing started before the review");
    // Deferred (the page's Skip): it starts, without the plugin.
    tab.req("session.spawn", v2_spawn(&dirs[0], &[PLUGIN]))
        .await
        .expect("spawn deferring the plugin");
    let env = launched_env(&env_file).await;
    assert!(env.contains(&format!("ANTHROPIC_AUTH_TOKEN={PROXY_TOKEN}")));
    assert!(!env.contains(PLUGIN_SECRET), "no choices: left out");

    let save = json!({"addon": "vocdoni", "checklist": PLUGIN,
        "revision": checklist["revision"], "answer": {"off": ["agent:reviewer"]}});
    tab.req("addons.checklist.save", save).await.expect("save");
    let config = fs::read_to_string(web.h().config_home.join("claudio/config.toml")).unwrap();
    assert!(config.contains(&format!("[plugins.{PLUGIN}]")), "{config}");
    assert!(
        config.contains("disabled = [\"agent:reviewer\"]"),
        "{config}"
    );
    host_ready(&mut tab).await;
    let items = plugin_checklist(&mut tab).await;
    let reviewer = items["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == "agent:reviewer")
        .cloned()
        .unwrap();
    assert_eq!(reviewer["on"], false);

    fs::remove_file(&env_file).unwrap();
    tab.req("session.spawn", v2_spawn(&dirs[1], &[]))
        .await
        .expect("spawn with the plugin");
    let env = launched_env(&env_file).await;
    assert!(
        env.contains(&format!("VOCDONI_TEST_TOKEN={PLUGIN_SECRET}")),
        "saved: the plugin's secret is in"
    );

    fs::remove_file(&env_file).unwrap();
    tab.req("session.spawn", v2_spawn(&dirs[0], &[PLUGIN]))
        .await
        .expect("spawn deferring the plugin");
    let env = launched_env(&env_file).await;
    assert!(env.contains(&format!("ANTHROPIC_AUTH_TOKEN={PROXY_TOKEN}")));
    assert!(!env.contains(PLUGIN_SECRET), "defer leaves it out");
}

/// The plugin's extension row once the local host answered it ready.
async fn host_ready(tab: &mut Tab) {
    until_req(tab, "addons.rows", json!({}), |d| {
        d["rows"].as_array().is_some_and(|rows| {
            rows.iter().any(|r| {
                r["name"] == PLUGIN
                    && r["details"].as_array().is_some_and(|ds| {
                        ds.iter()
                            .any(|l| l.as_str().is_some_and(|l| l.starts_with("local: ready")))
                    })
            })
        })
    })
    .await;
}

// ── Protocol v2: the addons.* ops (beside v1 until P3.16) ───────────────────

/// The Vocdoni addon of `list` (`addons.list`).
fn vocdoni_of(list: &Value) -> Value {
    list["addons"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == "vocdoni")
        .cloned()
        .expect("the vocdoni addon")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_addons_list_has_vocdoni_panels_and_launch_options() {
    let proxy = fake_proxy();
    let web = web_with_proxy(ManagerHarness::new(), &proxy);
    let cookie = web.login();
    let mut tab = Tab::open(&web, &cookie).await;
    let list = tab.req("addons.list", json!({})).await.expect("list");
    let vocdoni = vocdoni_of(&list);
    assert_eq!(
        vocdoni["panels"],
        json!([{"id": "proxy", "title": "Proxy stats", "windows": ["24h", "7d", "30d"], "key": "alt+s"}])
    );
    assert_eq!(
        vocdoni["launch_options"],
        json!([{"id": "env", "label": "env", "group": "Proxy", "default": true, "detail": null}])
    );
    assert_eq!(
        vocdoni["launch_flags"],
        json!([{"name": "first-party", "label": "First-party session"}])
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_addons_panel_renders_the_fixture_and_never_leaks_the_token() {
    let proxy = fake_proxy();
    let web = web_with_proxy(ManagerHarness::new(), &proxy);
    let cookie = web.login();
    let mut tab = Tab::open(&web, &cookie).await;
    let args = json!({"addon": "vocdoni", "panel": "proxy", "window": "7d"});
    let panel = until_req(&mut tab, "addons.panel", args, |p| {
        p.to_string().contains("tester · profile env")
    })
    .await;
    assert_eq!(panel["title"], "Proxy stats: env");
    let titles: Vec<&str> = panel["pages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| p["title"].as_str())
        .collect();
    assert_eq!(titles[0], "Overview");
    let text = panel.to_string();
    assert!(
        text.contains("out 6k") && text.contains("opus-5-5"),
        "{text}"
    );
    for frame in &tab.seen {
        let frame = String::from_utf8_lossy(frame);
        assert!(!frame.contains(PROXY_TOKEN), "the proxy token in a frame");
        assert!(
            !frame.contains(PLUGIN_SECRET),
            "the plugin secret in a frame"
        );
    }
    let log = fs::read_to_string(web.h().root.join("web.log")).unwrap();
    assert!(!log.contains(PROXY_TOKEN), "the proxy token in the log");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_addons_ops_refuse_unknown_addons_and_bad_args() {
    let web = Web::start();
    let cookie = web.login();
    let mut tab = Tab::open(&web, &cookie).await;
    let sid = "6f1c0000-0000-4000-8000-000000000000";
    for (op, args, want) in [
        (
            "addons.panel",
            json!({"addon": "nope", "panel": "proxy"}),
            "bad request: addon: no such addon",
        ),
        (
            "addons.panel",
            json!({"addon": "vocdoni", "panel": "Proxy!"}),
            "bad request: panel: not an id",
        ),
        (
            "addons.panel",
            json!({"addon": "vocdoni", "panel": "nope"}),
            "bad request: panel: no such panel",
        ),
        (
            "addons.panel",
            json!({"addon": "vocdoni", "panel": "proxy", "window": "1h"}),
            "bad request: window: not one of the panel's",
        ),
        (
            "addons.panel",
            json!({"addon": "vocdoni", "panel": "proxy", "sid": sid}),
            "no such session",
        ),
        (
            "addons.checklist",
            json!({"addon": "vocdoni", "checklist": "nope"}),
            "no such checklist",
        ),
        (
            "addons.checklist",
            json!({"addon": "nobody", "checklist": "x"}),
            "bad request: addon: no such addon",
        ),
        ("addons.nope", json!({}), "unknown op"),
    ] {
        assert_eq!(
            tab.req(op, args.clone()).await,
            Err(want.to_owned()),
            "{op} {args}"
        );
    }
    for (op, args) in [
        ("addons.list", json!({"x": 1})),
        ("addons.rows", json!({"x": 1})),
        (
            "addons.panel",
            json!({"addon": "vocdoni", "panel": "proxy", "profile": "work"}),
        ),
        ("addons.panel", json!({"addon": "vocdoni"})),
        (
            "addons.checklist",
            json!({"addon": "vocdoni", "checklist": "kb", "extra": true}),
        ),
    ] {
        let err = tab.req(op, args.clone()).await.unwrap_err();
        assert!(err.starts_with("bad request"), "{op} {args}: {err}");
    }
    // No proxy, so no plugin rows: only the web addon's (P3.18).
    let rows = tab.req("addons.rows", json!({})).await.unwrap();
    let rows: Vec<(&str, &str)> = rows["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["addon"].as_str().unwrap(), r["name"].as_str().unwrap()))
        .collect();
    assert_eq!(rows, [("web", "latex"), ("web", "diagram")]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_session_dto_has_launch_and_segments() {
    let h = ManagerHarness::new();
    let env_file = h.root.join("claude-env.txt");
    write_env_fake_claude(&h, &env_file);
    let dirs = h.dirs.clone();
    let proxy = fake_proxy();
    let web = web_with_proxy(h, &proxy);
    let cookie = web.login();
    let mut tab = Tab::open(&web, &cookie).await;
    tab.connected("local").await;
    let spawn = |dir: &std::path::PathBuf, pick: &str| {
        json!({"host": "local", "cwd": dir, "rows": 24, "cols": 80, "launch": {"pick": pick},
            "defer": [{"addon": "vocdoni", "id": PLUGIN}]})
    };
    let proxied = tab
        .req("session.spawn", spawn(&dirs[0], "default"))
        .await
        .expect("spawn");
    let sid = proxied["sid"].as_str().unwrap().to_owned();
    let ev = tab
        .event("session", |v| {
            v["session"]["id"] == sid.as_str() && v["session"]["segments"] != json!([])
        })
        .await;
    let session = &ev["session"];
    assert_eq!(
        session["launch"],
        json!({"known": true, "choice": {"addon": "vocdoni", "option": "env", "flags": []}})
    );
    assert_eq!(
        session["segments"][0],
        json!({"addon": "vocdoni", "id": "proxy", "text": "⇄ proxy:env", "tone": "accent", "priority": 10})
    );
    let direct = tab
        .req("session.spawn", spawn(&dirs[1], "direct"))
        .await
        .expect("spawn direct");
    let sid = direct["sid"].as_str().unwrap().to_owned();
    let ev = tab
        .event("session", |v| {
            v["session"]["id"] == sid.as_str() && v["session"]["launch"]["known"] == true
        })
        .await;
    assert_eq!(ev["session"]["launch"]["choice"], Value::Null);
    assert_eq!(ev["session"]["segments"], json!([]));
    for v1 in ["proxy", "proxy_known", "first_party"] {
        assert!(ev["session"].get(v1).is_none(), "{v1} is version 1's");
    }
}

/// A v2 spawn in `dir` (the addons' default route), with `defer`.
fn v2_spawn(dir: &std::path::Path, defer: &[&str]) -> Value {
    let defer: Vec<Value> = defer
        .iter()
        .map(|id| json!({"addon": "vocdoni", "id": id}))
        .collect();
    json!({"host": "local", "cwd": dir, "rows": 24, "cols": 80,
        "launch": {"pick": "default"}, "defer": defer})
}

/// The plugin's checklist, once its manifest is here.
async fn plugin_checklist(tab: &mut Tab) -> Value {
    let args = json!({"addon": "vocdoni", "checklist": PLUGIN});
    until_req(tab, "addons.checklist", args, |_| true).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_spawn_with_launch_needs_review_then_defer_spawns_without_the_plugin() {
    let h = ManagerHarness::new();
    let env_file = h.root.join("claude-env.txt");
    write_env_fake_claude(&h, &env_file);
    let dir = h.dirs[0].clone();
    let proxy = fake_proxy();
    let web = web_with_proxy(h, &proxy);
    let cookie = web.login();
    let mut tab = Tab::open(&web, &cookie).await;
    tab.connected("local").await;
    let checklist = plugin_checklist(&mut tab).await;
    let reply = tab.req("session.spawn", v2_spawn(&dir, &[])).await.unwrap();
    assert_eq!(
        reply,
        json!({"review": {"addon": "vocdoni", "id": PLUGIN, "revision": checklist["revision"]}})
    );
    assert!(!env_file.exists(), "nothing started before the review");
    let reply = tab
        .req("session.spawn", v2_spawn(&dir, &[PLUGIN]))
        .await
        .expect("spawn deferring the plugin");
    assert!(reply["sid"].is_string(), "{reply}");
    let env = launched_env(&env_file).await;
    assert!(env.contains(&format!("ANTHROPIC_AUTH_TOKEN={PROXY_TOKEN}")));
    assert!(!env.contains(PLUGIN_SECRET), "deferred: left out");
    // Flags without an option in effect.
    let mut alone = v2_spawn(&dir, &[]);
    alone["launch"] = json!({"pick": "direct", "flags": ["first-party"]});
    assert_eq!(
        tab.req("session.spawn", alone).await,
        Err("bad request: launch: flags need an addon's option".to_owned())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_checklist_save_then_spawn_gets_the_plugin() {
    let h = ManagerHarness::new();
    let env_file = h.root.join("claude-env.txt");
    write_env_fake_claude(&h, &env_file);
    let dir = h.dirs[0].clone();
    let proxy = fake_proxy();
    let web = web_with_proxy(h, &proxy);
    let cookie = web.login();
    let mut tab = Tab::open(&web, &cookie).await;
    tab.connected("local").await;
    let checklist = plugin_checklist(&mut tab).await;
    assert!(checklist["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["id"] == "agent:reviewer" && i["on"] == true));
    let save = json!({"addon": "vocdoni", "checklist": PLUGIN, "revision": checklist["revision"],
        "answer": {"off": ["agent:reviewer"]}});
    tab.req("addons.checklist.save", save).await.expect("save");
    let config = fs::read_to_string(web.h().config_home.join("claudio/config.toml")).unwrap();
    assert!(config.contains(&format!("[plugins.{PLUGIN}]")), "{config}");
    assert!(
        config.contains("disabled = [\"agent:reviewer\"]"),
        "{config}"
    );
    let reply = tab
        .req("session.spawn", v2_spawn(&dir, &[]))
        .await
        .expect("spawn");
    assert!(reply["sid"].is_string(), "saved: no review: {reply}");
    let env = launched_env(&env_file).await;
    assert!(
        env.contains(&format!("VOCDONI_TEST_TOKEN={PLUGIN_SECRET}")),
        "saved: the plugin's secret is in"
    );
    // Its row: toggled off through the core, then on.
    let rows = tab.req("addons.rows", json!({})).await.unwrap();
    let row = &rows["rows"][0];
    assert_eq!(
        (row["addon"].as_str(), row["name"].as_str()),
        (Some("vocdoni"), Some(PLUGIN))
    );
    assert_eq!(
        (row["on"].as_bool(), row["setup"].as_str()),
        (Some(true), Some(PLUGIN))
    );
    let toggle =
        |on: bool| json!({"addon": "vocdoni", "name": PLUGIN, "action": "toggle", "on": on});
    tab.req("addons.row_action", toggle(false))
        .await
        .expect("off");
    let config = fs::read_to_string(web.h().config_home.join("claudio/config.toml")).unwrap();
    assert!(config.contains(&format!("{PLUGIN} = false")), "{config}");
    let rows = tab.req("addons.rows", json!({})).await.unwrap();
    assert_eq!(rows["rows"][0]["on"], false);
    tab.req("addons.row_action", toggle(true))
        .await
        .expect("on");
    let refresh = json!({"addon": "vocdoni", "name": PLUGIN, "action": "refresh"});
    tab.req("addons.row_action", refresh)
        .await
        .expect("refresh");
    for (args, want) in [
        (
            json!({"addon": "vocdoni", "name": PLUGIN, "action": "toggle"}),
            "bad request: on: a toggle needs it",
        ),
        (
            json!({"addon": "vocdoni", "name": "diagram", "action": "refresh"}),
            "bad request: name: no such row",
        ),
        (
            json!({"addon": "vocdoni", "name": PLUGIN, "action": "drop"}),
            "bad request: action: toggle or refresh",
        ),
    ] {
        assert_eq!(
            tab.req("addons.row_action", args).await,
            Err(want.to_owned())
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_checklist_save_refuses_a_stale_revision() {
    let proxy = fake_proxy();
    let web = web_with_proxy(ManagerHarness::new(), &proxy);
    let cookie = web.login();
    let mut tab = Tab::open(&web, &cookie).await;
    tab.connected("local").await;
    let checklist = plugin_checklist(&mut tab).await;
    let save = |revision: &Value, answer: Value| json!({"addon": "vocdoni", "checklist": PLUGIN, "revision": revision, "answer": answer});
    let old = json!("0".repeat(64));
    assert_eq!(
        tab.req("addons.checklist.save", save(&old, json!({"off": []})))
            .await,
        Err("stale".to_owned())
    );
    let now = &checklist["revision"];
    for (answer, want) in [
        (
            json!({"off": ["plugin:nope@x"]}),
            "bad request: answer: no item plugin:nope@x",
        ),
        (
            json!("later"),
            r#"bad request: answer: {"off": [ids]} or "defer""#,
        ),
    ] {
        assert_eq!(
            tab.req("addons.checklist.save", save(now, answer)).await,
            Err(want.to_owned())
        );
    }
    let err = tab
        .req(
            "addons.checklist.save",
            save(now, json!({"off": [], "on": []})),
        )
        .await
        .unwrap_err();
    assert!(err.starts_with("bad request"), "{err}");
    tab.req("addons.checklist.save", save(now, json!("defer")))
        .await
        .expect("defer");
    let config = fs::read_to_string(web.h().config_home.join("claudio/config.toml")).unwrap();
    assert!(
        !config.contains("[plugins."),
        "deferred: nothing saved: {config}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_addons_action_switch_uses_server_state() {
    let h = ManagerHarness::new();
    let env_file = h.root.join("claude-env.txt");
    write_env_fake_claude(&h, &env_file);
    let dirs = h.dirs.clone();
    let proxy = fake_proxy();
    let web = web_with_proxy(h, &proxy);
    let cookie = web.login();
    let mut tab = Tab::open(&web, &cookie).await;
    tab.connected("local").await;
    let proxied = tab
        .req("session.spawn", v2_spawn(&dirs[0], &[PLUGIN]))
        .await
        .expect("spawn");
    let proxied = proxied["sid"].as_str().unwrap().to_owned();
    let mut direct = v2_spawn(&dirs[1], &[]);
    direct["launch"] = json!({"pick": "direct"});
    let direct = tab
        .req("session.spawn", direct)
        .await
        .expect("spawn direct");
    let direct = direct["sid"].as_str().unwrap().to_owned();
    let switch =
        |sid: &str| json!({"addon": "vocdoni", "panel": "proxy", "action": "switch", "sid": sid});
    // The profile comes from the server's record of the session: this one
    // is proxied (it only lacks a conversation yet), that one is direct.
    let deadline = std::time::Instant::now() + WAIT;
    let err = loop {
        let err = tab
            .req("addons.action", switch(&proxied))
            .await
            .unwrap_err();
        if err != "vocdoni: no such session" || std::time::Instant::now() > deadline {
            break err;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    };
    assert_eq!(err, "vocdoni: this session has no conversation yet");
    let deadline = std::time::Instant::now() + WAIT;
    let err = loop {
        let err = tab.req("addons.action", switch(&direct)).await.unwrap_err();
        if err != "vocdoni: no such session" || std::time::Instant::now() > deadline {
            break err;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    };
    assert_eq!(err, "vocdoni: this session runs direct");
    let mut forged = switch(&direct);
    forged["profile"] = json!("env");
    let err = tab.req("addons.action", forged).await.unwrap_err();
    assert!(err.starts_with("bad request"), "{err}");
    let unknown = switch("6f1c0000-0000-4000-8000-000000000000");
    assert_eq!(
        tab.req("addons.action", unknown).await,
        Err("no such session".to_owned())
    );
    let mut bad = switch(&proxied);
    bad["action"] = json!("drop");
    assert_eq!(
        tab.req("addons.action", bad).await,
        Err("vocdoni: no such action".to_owned())
    );
    // A respawn names its launch the v2 way.
    let again = json!({"sid": direct, "fresh": true, "rows": 24, "cols": 80,
        "launch": {"pick": "direct"}});
    let reply = tab.req("session.respawn", again).await.expect("respawn");
    assert_eq!(reply["sid"], direct.as_str());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_proxy_stats_render_the_fixture() {
    let proxy = fake_proxy();
    let web = web_with_proxy(ManagerHarness::new(), &proxy);
    let cookie = web.login();
    let mut tab = Tab::open(&web, &cookie).await;
    let args = json!({"addon": "vocdoni", "panel": "proxy", "window": "7d"});
    let panel = until_req(&mut tab, "addons.panel", args, |p| {
        p.to_string().contains("tester · profile env")
    })
    .await;
    let text = panel.to_string();
    assert!(text.contains("in the last 7d"), "{text}");
    assert!(
        text.contains("out 6k") && text.contains("opus-5-5"),
        "{text}"
    );
    assert!(text.contains("anthropic ok"), "the pool: {text}");
    let bad = json!({"addon": "vocdoni", "panel": "proxy", "window": "1h"});
    let err = tab.req("addons.panel", bad).await.unwrap_err();
    assert!(err.starts_with("bad request: window"), "{err}");
}

/// The version-1 ops are gone (docs/web-protocol.md v2): unknown.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_v1_proxy_and_plugin_ops_are_unknown() {
    let web = Web::start();
    let cookie = web.login();
    let mut tab = Tab::open(&web, &cookie).await;
    assert!(tab.snapshot.get("profiles").is_none());
    for (op, args) in [
        ("proxy.stats", json!({"profile": "env", "window": "24h"})),
        ("proxy.credential", json!({"sid": uuid::Uuid::nil()})),
        ("proxy.switch", json!({"sid": uuid::Uuid::nil()})),
        ("plugins.list", json!({})),
        ("plugins.items", json!({"name": PLUGIN})),
        ("plugins.save", json!({"name": PLUGIN, "disabled": []})),
        ("plugins.status", json!({})),
    ] {
        assert_eq!(
            tab.req(op, args).await,
            Err("unknown op".to_owned()),
            "{op}"
        );
    }
    let dir = web.h().dirs[0].clone();
    for (key, v1) in [
        ("proxy", json!({"kind": "direct"})),
        ("first_party", json!(true)),
        ("skip_plugins", json!([PLUGIN])),
    ] {
        let mut args = json!({"host": "local", "cwd": dir, "rows": 24, "cols": 80});
        args[key] = v1;
        let err = tab.req("session.spawn", args).await.unwrap_err();
        assert!(err.starts_with("bad request"), "{key}: {err}");
    }
}
