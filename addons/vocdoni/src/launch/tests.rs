//! The Vocdoni launch's tests: resolve on given profiles, prepare against a
//! recording context and a profile file, and one launch through the
//! composer to a fake daemon.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Mutex;

use super::*;
use crate::plugins::choices::PluginChoices;
use crate::plugins::manifest::Manifest;
use crate::plugins::state::{ManifestState, PluginsFetch};
use crate::proxy::env::{DEFAULT_OPUS, DEFAULT_SONNET};
use crate::proxy::plugins::{sample, PluginList};
use crate::test_support::{lock, TestCtx, TEAM};
use claudio_core::addon::{Addon, Addons, LaunchSelect};
use claudio_core::app::addons::test_support::{fake_host, next_request, reply};
use claudio_core::app::addons::{AddonRuntime, RuntimeOptions};
use claudio_core::app::launch::{run_within, LaunchSpec, Step};
use claudio_core::proto::{Msg, SessionExtensions, SessionId, SpawnSpec};

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
        error: None,
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
        frontend: claudio_core::addon::Frontend::Tui,
        deferred: Vec::new(),
    }
}

fn env_get<'a>(env: &'a [(String, String)], key: &str) -> Option<&'a str> {
    env.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

/// `kb` is listed for `work`, its manifest is here and its choices saved.
fn kb_ready(runner: &Runner) {
    kb_loaded(runner, true);
}

/// The addon's start as `load` leaves it: `kb` listed for `work`, its
/// manifest here, its choices saved or not, all under one lock.
fn kb_loaded(runner: &Runner, saved: bool) {
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
    if saved {
        state
            .plugins
            .choices
            .insert("kb".into(), PluginChoices::default());
    }
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
    let (s, rest) = claudio_core::app::launch::split_flags(&addons, &args).unwrap();
    assert_eq!(s.pick, pick("work").pick);
    assert_eq!(s.flags, ["first-party"]);
    assert_eq!(rest, ["-c"]);
    let bare: Vec<String> = vec!["--proxy".into()];
    assert!(claudio_core::app::launch::split_flags(&addons, &bare).is_err());
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

/// F3: `resolve` and `options` never read config.toml (they run on the
/// executor): before the addon's start read the profiles, a launch that
/// needs one fails (never direct) and the wizard has no rows.
#[test]
fn resolve_and_options_never_read_the_profiles() {
    let path = profiles_file();
    let runner = runner(path.clone());
    let vocdoni = Vocdoni::with_runner(Arc::clone(&runner));
    let ctx = TestCtx::default();
    let err = Launch::resolve(&vocdoni, &LaunchSelect::default(), &ctx).unwrap_err();
    assert!(err.contains("not read yet"), "{err}");
    assert!(Launch::resolve(&vocdoni, &pick("work"), &ctx).is_err());
    assert!(Launch::options(&vocdoni, &ctx).is_empty());
    assert_eq!(
        Launch::resolve(&vocdoni, &LaunchSelect::of(None), &ctx),
        Ok(Routing::Off)
    );
    // Read (off the executor, at the start): they answer from it.
    runner.read_profiles();
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
    assert_eq!(
        Launch::resolve(&vocdoni, &LaunchSelect::default(), &ctx),
        Ok(Routing::Required(choice("work".into(), false)))
    );
    assert_eq!(Launch::options(&vocdoni, &ctx).len(), 2);
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
        frontend: claudio_core::addon::Frontend::Tui,
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
        .map(|LaunchArg::Optional { flag, .. }| flag.as_str())
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
    // One skipped for this launch is left out whole, its secret too.
    let runner = self::runner(profiles_file());
    kb_ready(&runner);
    let ctx: Arc<dyn AddonCtx> = Arc::new(TestCtx::default());
    let mut req = request(LaunchOp::New, choice("work".into(), false));
    req.deferred = vec!["kb".into()];
    let part = prepare(runner, req, ctx).await.unwrap();
    assert!(part.args.is_empty() && env_get(&part.env, "KB_TOKEN").is_none());
}

/// `kb` as [`kb_ready`] has it, but never set up.
fn kb_unsaved() -> Arc<Runner> {
    let runner = runner(profiles_file());
    kb_ready(&runner);
    runner.state().plugins.choices.clear();
    runner
}

#[tokio::test]
async fn prepare_needs_review_for_an_unsaved_plugin() {
    let runner = kb_unsaved();
    let test = Arc::new(TestCtx::default());
    let ctx: Arc<dyn AddonCtx> = test.clone();
    let req = request(LaunchOp::New, choice("work".into(), false));
    let out = prepare(Arc::clone(&runner), req.clone(), ctx.clone()).await;
    assert_eq!(
        out,
        Err(LaunchError::NeedsReview {
            checklist: "kb".into(),
            revision: "a".repeat(64),
        })
    );
    assert!(lock(&test.requests).is_empty(), "nothing asked before it");
    // A resume reviews too; a respawn or a recovery never does.
    let resume = LaunchRequest {
        op: LaunchOp::Resume,
        ..req.clone()
    };
    let out = prepare(Arc::clone(&runner), resume, ctx.clone()).await;
    assert!(matches!(out, Err(LaunchError::NeedsReview { .. })));
    for op in [LaunchOp::Respawn, LaunchOp::Recover] {
        let again = LaunchRequest { op, ..req.clone() };
        let part = prepare(Arc::clone(&runner), again, ctx.clone()).await;
        assert!(part.is_ok(), "{op:?}");
    }
    // A host without the addon gets no plugins: nothing to review.
    test.old.store(true, Ordering::SeqCst);
    assert!(prepare(Arc::clone(&runner), req.clone(), ctx).await.is_ok());
    // Its manifest failed: left out, said.
    let runner = kb_unsaved();
    let failed = ManifestState::Failed("bad hash".into());
    runner.state().plugins.manifests.get_mut("kb").unwrap().1 = failed;
    let ctx: Arc<dyn AddonCtx> = Arc::new(TestCtx::default());
    let part = prepare(runner, req, ctx).await.unwrap();
    assert_eq!(part.notices, ["plugin kb left out: manifest: bad hash"]);
    assert!(env_get(&part.env, "KB_TOKEN").is_none());
}

#[tokio::test(start_paused = true)]
async fn prepare_waits_for_an_unsaved_plugins_manifest() {
    let runner = kb_unsaved();
    runner.activate();
    let sha = "a".repeat(64);
    runner.state().plugins.manifests.get_mut("kb").unwrap().1 = ManifestState::Fetching;
    let later = Arc::clone(&runner);
    let arrives = sha.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(3)).await;
        let m = Manifest::parse(&TEAM.replace("\"team\"", "\"kb\"")).unwrap();
        later.state().on_manifest("kb", &arrives, Ok(m));
    });
    let ctx: Arc<dyn AddonCtx> = Arc::new(TestCtx::default());
    let req = request(LaunchOp::New, choice("work".into(), false));
    let out = prepare(Arc::clone(&runner), req.clone(), ctx.clone()).await;
    assert!(matches!(out, Err(LaunchError::NeedsReview { revision, .. }) if revision == sha));
    // Never here: left out once the wait is over, said.
    runner.state().plugins.manifests.get_mut("kb").unwrap().1 = ManifestState::Fetching;
    let started = Instant::now();
    let part = prepare(runner, req, ctx).await.unwrap();
    assert!(started.elapsed() >= MANIFEST_WAIT);
    assert_eq!(
        part.notices,
        ["plugin kb left out: its manifest is not here yet"]
    );
}

#[tokio::test]
async fn deferred_skips_review_for_that_launch_only() {
    let runner = kb_unsaved();
    let ctx: Arc<dyn AddonCtx> = Arc::new(TestCtx::default());
    let mut req = request(LaunchOp::New, choice("work".into(), false));
    req.deferred = vec!["kb".into()];
    let part = prepare(Arc::clone(&runner), req.clone(), ctx.clone())
        .await
        .unwrap();
    assert!(part.args.is_empty() && env_get(&part.env, "KB_TOKEN").is_none());
    assert!(part.notices.is_empty(), "{:?}", part.notices);
    // The next launch, which skips nothing, asks again: even after the
    // member put the first-sight offer off for the run.
    runner.state().defer("kb");
    req.deferred.clear();
    let out = prepare(runner, req, ctx).await;
    assert!(
        matches!(out, Err(LaunchError::NeedsReview { .. })),
        "{out:?}"
    );
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

/// F2: a launch whose `prepare` runs before the runtime's `Started` (a
/// `--plain` start, the web's first spawn) still waits for the addon's
/// start, so it asks for the plugin's review instead of starting without
/// it; a set-up plugin then comes with the session.
#[tokio::test]
async fn a_launch_before_the_runtimes_start_waits_for_it() {
    let path = profiles_file();
    for saved in [false, true] {
        let runner = runner(path.clone());
        assert!(!runner.is_active(), "no event delivered yet");
        let later = Arc::clone(&runner);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            later.activate();
            kb_loaded(&later, saved);
        });
        let vocdoni = Vocdoni::with_runner(runner);
        let ctx: Arc<dyn AddonCtx> = Arc::new(TestCtx::default());
        let req = request(LaunchOp::New, choice("work".into(), false));
        let out = Launch::prepare(&vocdoni, req, ctx).await;
        if saved {
            let part = out.unwrap();
            assert_eq!(part.args.len(), 2, "{part:?}");
        } else {
            assert_eq!(
                out,
                Err(LaunchError::NeedsReview {
                    checklist: "kb".into(),
                    revision: "a".repeat(64),
                })
            );
        }
    }
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
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

/// A config.toml of `text` in its own private directory (removed by the caller).
fn config_file(text: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("claudio-launch-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    std::fs::write(&path, text).unwrap();
    path
}

/// A launch of `select` through the composer, the profiles read from
/// `profiles`, to a fake daemon: where it got to, and every `Spawn` sent.
async fn compose_launch(
    profiles: PathBuf,
    select: LaunchSelect,
    frontend: claudio_core::addon::Frontend,
) -> (Step, Vec<Msg>) {
    let vocdoni = Vocdoni::with_runner(runner(profiles));
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
    let sid = SessionId::from_u128(11);
    let spec = LaunchSpec {
        op: LaunchOp::New,
        sid,
        host: "local".into(),
        cwd: "/w".into(),
        select,
        frontend,
        base: Msg::Spawn(SpawnSpec {
            id: sid,
            cwd: "/w".into(),
            ..Default::default()
        }),
        core_lenses: SessionExtensions::default(),
    };
    let step = run_within(&rt, spec, client, Duration::from_secs(25)).await;
    rt.shutdown();
    let sent = sent.lock().unwrap().clone();
    (step, sent)
}

/// R18: a configured default the profiles lack (what the wizard keeps as
/// `Pick::Default`) fails the launch; nothing is sent, never direct.
#[tokio::test]
async fn a_default_the_profiles_lack_fails_the_launch() {
    let path = config_file(
        "[proxy]\ndefault = \"gone\"\n[proxy.profiles.work]\nurl = \"http://127.0.0.1:1\"\ntoken = \"t\"\n",
    );
    let (step, sent) = compose_launch(
        path.clone(),
        LaunchSelect::default(),
        claudio_core::addon::Frontend::Tui,
    )
    .await;
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
    let Step::Failed(why) = step else {
        panic!("not failed: {step:?}")
    };
    assert!(why.contains("'gone' not found"), "{why}");
    assert!(sent.is_empty(), "{sent:?}");
}

/// R20: profiles that cannot be read (an unreadable config.toml, or one
/// that does not parse) fail a default or a chosen profile's launch with
/// the load error, nothing sent; an explicit direct launch still goes.
#[tokio::test]
async fn unreadable_or_malformed_profiles_fail_the_launch_never_direct() {
    use std::os::unix::fs::PermissionsExt;
    let unreadable = config_file(
        "[proxy]\ndefault = \"work\"\n[proxy.profiles.work]\nurl = \"http://127.0.0.1:1\"\ntoken = \"t\"\n",
    );
    std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o000)).unwrap();
    let malformed = config_file("[proxy\ndefault = \"work\"\n");
    let mistyped = config_file("[proxy]\ndefault = 5\n");
    for (path, why) in [
        (&unreadable, "Permission denied"),
        (&malformed, "config.toml"),
        (&mistyped, "config.toml"),
    ] {
        for select in [LaunchSelect::default(), pick("work")] {
            for frontend in [
                claudio_core::addon::Frontend::Tui,
                claudio_core::addon::Frontend::Plain,
            ] {
                let (step, sent) = compose_launch(path.clone(), select.clone(), frontend).await;
                let Step::Failed(e) = step else {
                    panic!("{path:?} {select:?}: not failed: {step:?}")
                };
                assert!(e.contains("cannot read the proxy profiles"), "{e}");
                assert!(e.contains(why), "{e}");
                assert!(sent.is_empty(), "{sent:?}");
            }
        }
        let (step, sent) = compose_launch(
            path.clone(),
            LaunchSelect::of(None),
            claudio_core::addon::Frontend::Plain,
        )
        .await;
        assert!(matches!(step, Step::Sent { choice: None, .. }), "{step:?}");
        assert_eq!(sent.len(), 1, "direct still starts");
    }
    for path in [unreadable, malformed, mistyped] {
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}

/// R20: a load error is not "no profiles": resolve fails the default and a
/// chosen profile with it; direct and other addons' options go on.
#[test]
fn a_profile_load_error_fails_resolve_unless_direct() {
    let broken = || Profiles {
        error: Some("cannot read the proxy profiles: config.toml: line 2".into()),
        ..Profiles::default()
    };
    for select in [LaunchSelect::default(), pick("work")] {
        let err = resolve(&select, broken).unwrap_err();
        assert!(err.starts_with("cannot read the proxy profiles"), "{err}");
    }
    assert_eq!(resolve(&LaunchSelect::of(None), broken), Ok(Routing::Off));
    let other = LaunchSelect {
        pick: Pick::Option {
            addon: "other".into(),
            option: "x".into(),
        },
        flags: Vec::new(),
    };
    assert_eq!(resolve(&other, broken), Ok(Routing::Off));
}

/// P3.19 (AC8): the previous release's state.json and web-state.json
/// routings are adopted as this addon's choices, and written back.
#[test]
fn v080_fixtures_adopt_their_proxy_profiles() {
    use claudio_core::addon::{Launch, LaunchChoice, LegacyChoice};
    let dir =
        std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures"));
    let read = |name: &str| -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(dir.join(name)).unwrap()).unwrap()
    };
    let legacy = |v: &serde_json::Value| LegacyChoice {
        proxy: v["proxy"].as_str().map(str::to_owned),
        first_party: v["first_party"].as_bool().unwrap_or(false),
    };
    let state = read("state-v0.8.0.json");
    let web = read("web-state-v0.8.0.json");
    let mut entries: Vec<LegacyChoice> = state["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(legacy)
        .collect();
    entries.extend(web["sessions"].as_object().unwrap().values().map(legacy));
    let vocdoni = super::super::Vocdoni::new();
    let adopted: Vec<Option<LaunchChoice>> =
        entries.iter().map(|l| vocdoni.adopt_legacy(l)).collect();
    let choice = |option: &str, first_party: bool| {
        Some(LaunchChoice {
            addon: "vocdoni".into(),
            option: option.into(),
            flags: if first_party {
                vec![super::FIRST_PARTY_FLAG.into()]
            } else {
                Vec::new()
            },
        })
    };
    let want = [
        None,
        choice("work", false),
        choice("lab", true),
        None,
        None,
        choice("work", false),
        choice("lab", true),
    ];
    assert_eq!(adopted, want);
    for (entry, adopted) in entries.iter().zip(&adopted) {
        if let Some(choice) = adopted {
            assert_eq!(vocdoni.legacy(choice).as_ref(), Some(entry));
        }
    }
}
