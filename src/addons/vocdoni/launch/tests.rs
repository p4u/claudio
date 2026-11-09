//! The Vocdoni launch's tests: resolve on given profiles, prepare against a
//! recording context and a profile file, and one launch through the
//! composer to a fake daemon.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Mutex;

use super::*;
use crate::addon::{Addon, Addons, LaunchSelect};
use crate::addons::vocdoni::plugins::choices::PluginChoices;
use crate::addons::vocdoni::plugins::manifest::Manifest;
use crate::addons::vocdoni::plugins::state::PluginsFetch;
use crate::addons::vocdoni::proxy::env::{DEFAULT_OPUS, DEFAULT_SONNET};
use crate::addons::vocdoni::proxy::plugins::{sample, PluginList};
use crate::addons::vocdoni::test_support::{lock, TestCtx, TEAM};
use crate::app::addons::test_support::{fake_host, next_request, reply};
use crate::app::addons::{AddonRuntime, RuntimeOptions};
use crate::app::launch::{run_within, LaunchSpec, Step};
use crate::proto::{Msg, SessionExtensions, SessionId, SpawnSpec};

/// A config.toml with profiles `work` (the default, on an unbound port)
/// and `lab` (first-party), in its own private directory.
fn profiles_file() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("claudio-launch-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    let text = r#"
[proxy]
default = "work"
[proxy.profiles.work]
url = "http://127.0.0.1:1"
token = "work-token-value"
[proxy.profiles.lab]
url = "http://127.0.0.1:1"
token = "lab-token-value"
first_party = true
"#;
    std::fs::write(&path, text).unwrap();
    path
}

fn runner(profiles: PathBuf) -> Arc<Runner> {
    Arc::new(Runner::with_files(
        "/nonexistent/plugins.json".into(),
        profiles,
    ))
}

fn names(list: &[&str], default: Option<&str>) -> Profiles {
    Profiles {
        names: list.iter().map(|s| s.to_string()).collect(),
        default: default.map(str::to_owned),
        first_party: Default::default(),
    }
}

fn pick(option: &str) -> LaunchSelect {
    LaunchSelect {
        pick: Pick::Option {
            addon: NAME.into(),
            option: option.into(),
        },
        flags: Vec::new(),
    }
}

fn request(op: LaunchOp, choice: LaunchChoice) -> LaunchRequest {
    LaunchRequest {
        op,
        sid: SessionId::from_u128(1),
        host: "local".into(),
        cwd: "/w".into(),
        choice: Some(choice.clone()),
        routing: Routing::Required(choice),
        frontend: crate::addon::Frontend::Tui,
        deferred: Vec::new(),
    }
}

fn env_get<'a>(env: &'a [(String, String)], key: &str) -> Option<&'a str> {
    env.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

/// `kb` is listed for `work`, its manifest is here and its choices saved.
fn kb_ready(runner: &Runner) {
    let list = PluginList {
        profile: "work".into(),
        fetched_at: 1,
        plugins: vec![sample("kb")],
    };
    let manifest = Manifest::parse(&TEAM.replace("\"team\"", "\"kb\"")).unwrap();
    let mut state = runner.state();
    state.loaded = true;
    state.on_list("work", PluginsFetch::Listed(list));
    state.on_manifest("kb", &"a".repeat(64), Ok(manifest));
    state
        .plugins
        .choices
        .insert("kb".into(), PluginChoices::default());
    // Connected: asked (that task is the runner's; no context runs it here).
    state.host_up("local", true);
}

fn flag(name: &str, value: Option<&str>) -> FlagArg {
    FlagArg {
        name: name.into(),
        value: value.map(str::to_owned),
    }
}

#[test]
fn default_profile_is_required() {
    let p = || names(&["work", "lab"], Some("work"));
    let required = |profile: &str, flags: Vec<String>| {
        Ok(Routing::Required(LaunchChoice {
            addon: NAME.into(),
            option: profile.into(),
            flags,
        }))
    };
    assert_eq!(
        resolve(&LaunchSelect::default(), p),
        required("work", vec![])
    );
    let fp = LaunchSelect {
        flags: vec![FIRST_PARTY_FLAG.into()],
        ..LaunchSelect::default()
    };
    assert_eq!(
        resolve(&fp, p),
        required("work", vec!["first-party".into()])
    );
    assert_eq!(resolve(&pick("lab"), p), required("lab", vec![]));
    // No default: the addon sits it out; direct and other addons' options too.
    let none = || names(&["work"], None);
    assert_eq!(resolve(&LaunchSelect::default(), none), Ok(Routing::Off));
    assert_eq!(resolve(&LaunchSelect::of(None), p), Ok(Routing::Off));
    let other = LaunchSelect {
        pick: Pick::Option {
            addon: "other".into(),
            option: "work".into(),
        },
        flags: Vec::new(),
    };
    assert_eq!(resolve(&other, p), Ok(Routing::Off));
    // Direct never reads the profiles.
    let never = || -> Profiles { panic!("read for a direct launch") };
    assert_eq!(resolve(&LaunchSelect::of(None), never), Ok(Routing::Off));
    // The options: one per profile, the default and first-party marked.
    let mut listed = names(&["lab", "work"], Some("work"));
    listed.first_party.insert("lab".into());
    let opts = options(&listed);
    assert_eq!(opts.len(), 2);
    assert_eq!((opts[0].id.as_str(), opts[0].default), ("lab", false));
    assert_eq!(opts[0].detail.as_deref(), Some("first-party"));
    assert_eq!((opts[1].id.as_str(), opts[1].default), ("work", true));
    assert!(opts.iter().all(|o| o.group == "Proxy"));
}

#[test]
fn flags_select_a_route_and_the_switch() {
    let s = select(&[
        flag(FIRST_PARTY, None),
        flag(PROXY, Some("work")),
        flag(FIRST_PARTY, None),
    ])
    .unwrap();
    assert_eq!(s.pick, pick("work").pick);
    assert_eq!(s.flags, ["first-party"]);
    let s = select(&[flag(PROXY, Some("work")), flag(NO_PROXY, None)]).unwrap();
    assert_eq!(s.pick, Pick::Direct);
    assert_eq!(select(&[]).unwrap(), LaunchSelect::default());
    assert_eq!(
        select(&[flag(PROXY, None)]).unwrap_err(),
        "--proxy requires a profile name"
    );
    // Through the core's parser: leading flags in any order, then the rest.
    let addons = Addons::new(vec![Arc::new(Vocdoni::new()) as Arc<dyn Addon>]).unwrap();
    let args: Vec<String> = ["--first-party", "--proxy", "work", "-c"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let (s, rest) = crate::app::launch::split_flags(&addons, &args).unwrap();
    assert_eq!(s.pick, pick("work").pick);
    assert_eq!(s.flags, ["first-party"]);
    assert_eq!(rest, ["-c"]);
    let bare: Vec<String> = vec!["--proxy".into()];
    assert!(crate::app::launch::split_flags(&addons, &bare).is_err());
}

#[test]
fn legacy_fields_are_adopted_and_written_back() {
    let v = Vocdoni::new();
    let legacy = LegacyChoice {
        proxy: Some("work".into()),
        first_party: true,
    };
    let choice = v.adopt_legacy(&legacy).unwrap();
    assert_eq!(
        choice,
        LaunchChoice {
            addon: NAME.into(),
            option: "work".into(),
            flags: vec!["first-party".into()],
        }
    );
    assert_eq!(v.legacy(&choice), Some(legacy));
    assert_eq!(v.adopt_legacy(&LegacyChoice::default()), None);
    let theirs = LaunchChoice {
        addon: "other".into(),
        ..choice
    };
    assert_eq!(v.legacy(&theirs), None);
}

#[tokio::test]
async fn unknown_profile_fails_never_direct() {
    let p = || names(&["work"], Some("work"));
    let err = resolve(&pick("gone"), p).unwrap_err();
    assert_eq!(err, "proxy profile 'gone' not found (was it deleted?)");
    let err = resolve(&pick("env"), p).unwrap_err();
    assert!(err.contains("CLAUDIO_PROXY_URL"), "{err}");
    // A profile deleted since resolve: prepare fails, no part.
    let runner = runner(profiles_file());
    let ctx: Arc<dyn AddonCtx> = Arc::new(TestCtx::default());
    let gone = LaunchChoice {
        addon: NAME.into(),
        option: "gone".into(),
        flags: Vec::new(),
    };
    let out = prepare(runner, request(LaunchOp::New, gone), ctx).await;
    assert_eq!(
        out,
        Err(LaunchError::Failed(
            "proxy profile 'gone' not found (was it deleted?)".into()
        ))
    );
}

#[tokio::test]
async fn unreachable_config_keeps_the_proxy_with_builtin_models() {
    let ctx: Arc<dyn AddonCtx> = Arc::new(TestCtx::default());
    let part = prepare(
        runner(profiles_file()),
        request(LaunchOp::New, choice("work".into(), false)),
        ctx,
    )
    .await
    .unwrap();
    assert_eq!(
        env_get(&part.env, "ANTHROPIC_BASE_URL"),
        Some("http://127.0.0.1:1")
    );
    assert_eq!(
        env_get(&part.env, "ANTHROPIC_AUTH_TOKEN"),
        Some("work-token-value")
    );
    assert_eq!(
        env_get(&part.env, "ANTHROPIC_DEFAULT_OPUS_MODEL"),
        Some(DEFAULT_OPUS)
    );
    assert_eq!(env_get(&part.env, "CLAUDE_CODE_USE_GATEWAY"), Some("1"));
    assert_eq!(part.env_remove, ["ANTHROPIC_API_KEY"]);
    assert!(part.args.is_empty() && part.notices.is_empty());
    // The secrets stay out of `Debug`.
    assert!(!format!("{part:?}").contains("work-token-value"));
}

/// The default profile's launch through the composer, its proxy config
/// unreachable: sent through the proxy, with the built-in models, never
/// direct.
#[tokio::test]
async fn a_default_profile_launch_with_an_unreachable_config_is_never_direct() {
    let vocdoni = Vocdoni::with_runner(runner(profiles_file()));
    let addons = Arc::new(Addons::new(vec![Arc::new(vocdoni) as Arc<dyn Addon>]).unwrap());
    let opts = RuntimeOptions {
        config_path: "/nonexistent/claudio/config.toml".into(),
        watch_config: false,
    };
    let (rt, _rx) = AddonRuntime::start(addons, opts);
    let sent: Arc<Mutex<Vec<Msg>>> = Arc::default();
    let seen = Arc::clone(&sent);
    let client = fake_host(&["addon:vocdoni"], move |mut s| async move {
        while let Some((req, msg)) = next_request(&mut s).await {
            if let Msg::Spawn(spec) = &msg {
                let id = spec.id;
                seen.lock().unwrap().push(msg);
                reply(&mut s, req, Msg::Spawned { id, pid: None }).await;
            }
        }
    })
    .await;
    rt.host_up("local", client.clone());
    let sid = SessionId::from_u128(9);
    let spec = LaunchSpec {
        op: LaunchOp::New,
        sid,
        host: "local".into(),
        cwd: "/w".into(),
        select: LaunchSelect::default(),
        frontend: crate::addon::Frontend::Tui,
        base: Msg::Spawn(SpawnSpec {
            id: sid,
            cwd: "/w".into(),
            ..Default::default()
        }),
        core_lenses: SessionExtensions::default(),
    };
    let step = run_within(&rt, spec, client, Duration::from_secs(25)).await;
    let Step::Sent { choice, reply, .. } = step else {
        panic!("not sent: {step:?}")
    };
    assert!(reply.is_ok());
    assert_eq!(choice.map(|c| c.option), Some("work".to_owned()));
    let sent = sent.lock().unwrap().clone();
    let [Msg::Spawn(spec)] = &sent[..] else {
        panic!("{sent:?}")
    };
    assert_eq!(
        env_get(&spec.env, "ANTHROPIC_BASE_URL"),
        Some("http://127.0.0.1:1")
    );
    assert_eq!(
        env_get(&spec.env, "ANTHROPIC_DEFAULT_SONNET_MODEL"),
        Some(DEFAULT_SONNET)
    );
    assert_eq!(spec.env_remove, Some(vec!["ANTHROPIC_API_KEY".to_owned()]));
    rt.shutdown();
}

#[tokio::test]
async fn first_party_removes_the_gateway_var_and_skips_gateway_env() {
    // By the switch on a gateway profile, and by the profile itself.
    for (profile, flag) in [("work", true), ("lab", false)] {
        let ctx: Arc<dyn AddonCtx> = Arc::new(TestCtx::default());
        let req = request(LaunchOp::New, choice(profile.into(), flag));
        let part = prepare(runner(profiles_file()), req, ctx).await.unwrap();
        assert_eq!(env_get(&part.env, "CLAUDE_CODE_USE_GATEWAY"), None);
        assert_eq!(
            env_get(&part.env, "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"),
            None
        );
        assert_eq!(env_get(&part.env, "DISABLE_AUTOUPDATER"), Some("1"));
        assert_eq!(
            part.env_remove,
            ["ANTHROPIC_API_KEY", "CLAUDE_CODE_USE_GATEWAY"]
        );
    }
}

#[tokio::test]
async fn plugin_share_becomes_optional_args_secrets_and_instructions() {
    let runner = runner(profiles_file());
    kb_ready(&runner);
    let test = Arc::new(TestCtx::default());
    let ctx: Arc<dyn AddonCtx> = test.clone();
    let req = request(LaunchOp::New, choice("work".into(), false));
    let part = prepare(Arc::clone(&runner), req, ctx).await.unwrap();
    // The host was asked, by secret names only.
    let requests = lock(&test.requests).clone();
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert_eq!(requests[0].1, "apply");
    assert_eq!(requests[0].2["secrets"], serde_json::json!(["KB_TOKEN"]));
    assert!(!requests[0].2.to_string().contains("s3cret"));
    let flags: Vec<&str> = part
        .args
        .iter()
        .map(|a| match a {
            LaunchArg::Optional { flag, .. } => flag.as_str(),
            LaunchArg::Plain(p) => p.as_str(),
        })
        .collect();
    assert_eq!(flags, ["--agents", "--mcp-config"]);
    // The plugin's [env] and secret first, the proxy's after; no key twice.
    assert_eq!(env_get(&part.env, "KB_REGION"), Some("kb"));
    assert_eq!(env_get(&part.env, "KB_TOKEN"), Some("s3cret"));
    let kb = part.env.iter().position(|(k, _)| k == "KB_TOKEN").unwrap();
    let base = part.env.iter().position(|(k, _)| k == "ANTHROPIC_BASE_URL");
    assert!(base.is_some_and(|b| b > kb));
    assert_eq!(part.instructions, "Use kb.");
    assert!(part.notices.is_empty(), "{:?}", part.notices);
    // The answer went to the addon's state as well.
    assert!(runner.state().apply_state("local", "kb").is_some());
    // An unsaved plugin is left out (the review comes later).
    let runner = self::runner(profiles_file());
    kb_ready(&runner);
    runner.state().plugins.choices.clear();
    let ctx: Arc<dyn AddonCtx> = Arc::new(TestCtx::default());
    let req = request(LaunchOp::New, choice("work".into(), false));
    let part = prepare(runner, req, ctx).await.unwrap();
    assert!(part.args.is_empty() && env_get(&part.env, "KB_TOKEN").is_some());
}

#[tokio::test]
async fn an_old_daemon_gets_the_proxy_env_without_plugins_and_a_notice() {
    let runner = runner(profiles_file());
    kb_ready(&runner);
    let test = Arc::new(TestCtx::default());
    test.old.store(true, Ordering::SeqCst);
    let ctx: Arc<dyn AddonCtx> = test.clone();
    let req = request(LaunchOp::New, choice("work".into(), false));
    let part = prepare(runner, req, ctx).await.unwrap();
    assert!(
        lock(&test.requests).is_empty(),
        "an old daemon is not asked"
    );
    assert!(part.args.is_empty() && part.instructions.is_empty());
    assert_eq!(env_get(&part.env, "KB_TOKEN"), None, "no plugin secret");
    assert_eq!(
        env_get(&part.env, "ANTHROPIC_AUTH_TOKEN"),
        Some("work-token-value")
    );
    assert_eq!(
        part.notices,
        ["daemon on local is older: plugins after `claudio daemon restart`"]
    );
}

/// A new session at a front-end's start waits for the addon to read its
/// lists and choices, then gets the plugins.
#[tokio::test]
async fn a_new_session_waits_for_the_addons_start() {
    let runner = runner(profiles_file());
    kb_ready(&runner);
    runner.state().loaded = false;
    runner.activate();
    let later = Arc::clone(&runner);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        later.state().loaded = true;
    });
    let ctx: Arc<dyn AddonCtx> = Arc::new(TestCtx::default());
    let req = request(LaunchOp::New, choice("work".into(), false));
    let part = prepare(runner, req, ctx).await.unwrap();
    assert_eq!(part.args.len(), 2, "{part:?}");
}

#[tokio::test]
async fn respawn_waits_for_no_manifest() {
    let runner = runner(profiles_file());
    kb_ready(&runner);
    // Not loaded and not answered: a new session would wait; a respawn
    // and a recovery take what is answered now.
    runner.state().loaded = false;
    runner.activate();
    for op in [LaunchOp::Respawn, LaunchOp::Recover] {
        let test = Arc::new(TestCtx::default());
        let ctx: Arc<dyn AddonCtx> = test.clone();
        let started = Instant::now();
        let part = prepare(
            Arc::clone(&runner),
            request(op, choice("work".into(), false)),
            ctx,
        )
        .await
        .unwrap();
        assert!(started.elapsed() < Duration::from_secs(10), "{op:?} waited");
        assert!(lock(&test.requests).is_empty(), "{op:?} asked");
        assert!(part.args.is_empty() && part.instructions.is_empty());
        // The plugin's secret still goes (its host answers later).
        assert_eq!(env_get(&part.env, "KB_TOKEN"), Some("s3cret"));
    }
}
