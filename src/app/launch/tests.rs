//! Tests of the launch composer with `TestAddon`s and a fake daemon behind
//! a real `Client`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::*;
use crate::addon::test_support::{Script, TestAddon};
use crate::addon::{Addon, Checklist, ChecklistAnswer, FlagDecl, LaunchArg};
use crate::app::addons::test_support::{fake_host, next_request, reply};
use crate::app::addons::{RuntimeOptions, RuntimeRx};
use crate::proto::{
    LensKind, LensSpec, RespawnSpec, SpawnSpec, MAX_LENSES, MAX_PLUGIN_INSTRUCTIONS,
};

/// What the fake daemon was sent.
type Sent = Arc<Mutex<Vec<Msg>>>;

/// A runtime for `addons` (no config file).
fn runtime(addons: Vec<Arc<dyn Addon>>) -> (AddonRuntime, RuntimeRx) {
    let addons = Arc::new(Addons::new(addons).unwrap());
    let opts = RuntimeOptions {
        config_path: "/nonexistent/claudio/config.toml".into(),
        watch_config: false,
    };
    AddonRuntime::start(addons, opts)
}

/// A client whose daemon records every launch and answers `Spawned`.
async fn daemon() -> (Client, Sent) {
    let sent: Sent = Arc::default();
    let seen = Arc::clone(&sent);
    let client = fake_host(&[], move |mut s| async move {
        while let Some((req, msg)) = next_request(&mut s).await {
            let id = match &msg {
                Msg::Spawn(spec) => spec.id,
                Msg::Respawn(spec) => spec.id,
                // The client's own requests (a keepalive) are not launches.
                _ => {
                    reply(&mut s, req, Msg::Pong).await;
                    continue;
                }
            };
            seen.lock().unwrap().push(msg);
            reply(&mut s, req, Msg::Spawned { id, pid: None }).await;
        }
    })
    .await;
    (client, sent)
}

fn sid() -> SessionId {
    SessionId::from_u128(7)
}

fn spawn_base() -> Msg {
    Msg::Spawn(SpawnSpec {
        id: sid(),
        cwd: "/w".into(),
        args: vec!["--resume".into(), "c1".into()],
        rows: 24,
        cols: 80,
        ..Default::default()
    })
}

fn respawn_base() -> Msg {
    Msg::Respawn(RespawnSpec {
        id: sid(),
        rows: 24,
        cols: 80,
        ..Default::default()
    })
}

fn lens(name: &str, instruction: &str) -> LensSpec {
    LensSpec {
        name: name.into(),
        kind: LensKind::Tag,
        instruction: instruction.into(),
    }
}

fn core() -> SessionExtensions {
    SessionExtensions {
        lenses: vec![lens("latex", "write <latex>")],
        ..Default::default()
    }
}

fn spec(op: LaunchOp, select: LaunchSelect) -> LaunchSpec {
    let base = match op {
        LaunchOp::Respawn => respawn_base(),
        _ => spawn_base(),
    };
    LaunchSpec {
        op,
        sid: sid(),
        host: "local".into(),
        cwd: "/w".into(),
        select,
        frontend: Frontend::Tui,
        base,
        core_lenses: core(),
    }
}

fn env_part(env: &[(&str, &str)]) -> LaunchPart {
    LaunchPart {
        env: env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        ..Default::default()
    }
}

fn choice(addon: &str, option: &str) -> LaunchChoice {
    LaunchChoice {
        addon: addon.into(),
        option: option.into(),
        flags: Vec::new(),
    }
}

/// An addon routing every launch as Required(`route`/`work`).
fn required(script: Script) -> Arc<TestAddon> {
    Arc::new(
        TestAddon::new("route")
            .routing(Ok(Routing::Required(choice("route", "work"))))
            .prepare(script),
    )
}

fn spawned(msg: &Msg) -> &SpawnSpec {
    match msg {
        Msg::Spawn(spec) => spec,
        other => panic!("not a spawn: {other:?}"),
    }
}

/// Whether the daemon got anything, after a moment for a request in flight.
async fn nothing_sent(sent: &Sent) -> bool {
    tokio::time::sleep(Duration::from_millis(50)).await;
    sent.lock().unwrap().is_empty()
}

#[test]
fn compose_layers_env_later_addon_wins_no_key_twice() {
    let parts = vec![
        ("a", env_part(&[("K", "1"), ("A", "x"), ("K", "1b")])),
        ("b", env_part(&[("K", "2"), ("B", "y")])),
    ];
    let (msg, notices) = compose(spawn_base(), core(), parts).unwrap();
    let spec = spawned(&msg);
    let pairs: Vec<(&str, &str)> = spec
        .env
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    assert_eq!(pairs, [("K", "2"), ("A", "x"), ("B", "y")]);
    assert!(notices.is_empty(), "{notices:?}");
}

#[test]
fn compose_unions_env_remove_and_always_sends_some() {
    let (msg, _) = compose(spawn_base(), core(), Vec::new()).unwrap();
    assert_eq!(spawned(&msg).env_remove, Some(Vec::new()));
    let part = |names: &[&str]| LaunchPart {
        env_remove: names.iter().map(|n| n.to_string()).collect(),
        ..Default::default()
    };
    let parts = vec![("a", part(&["Z", "A"])), ("b", part(&["A", "M"]))];
    let (msg, _) = compose(respawn_base(), core(), parts).unwrap();
    let Msg::Respawn(spec) = msg else {
        panic!("not a respawn")
    };
    let want: Vec<String> = ["A", "M", "Z"].map(String::from).to_vec();
    assert_eq!(spec.env_remove, Some(want));
}

#[test]
fn compose_marks_optional_args_and_respawn_drops_args() {
    let part = |value: &str| LaunchPart {
        env: vec![("T".into(), "t".into())],
        args: vec![
            LaunchArg::Plain("--verbose".into()),
            LaunchArg::Optional {
                flag: "--mcp-config".into(),
                value: value.into(),
            },
        ],
        ..Default::default()
    };
    let parts = vec![("a", part("{1}")), ("b", part("{2}"))];
    let (msg, _) = compose(spawn_base(), core(), parts.clone()).unwrap();
    let spec = spawned(&msg);
    let args: Vec<&str> = spec.args.iter().map(String::as_str).collect();
    assert_eq!(
        args,
        [
            "--resume",
            "c1",
            "--verbose",
            "--mcp-config",
            "{1}",
            "--verbose",
            "--mcp-config",
            "{2}"
        ]
    );
    assert_eq!(spec.optional, ["--mcp-config"]);
    // A respawn: env and extensions, no args (the journal has them).
    let (msg, _) = compose(respawn_base(), core(), parts).unwrap();
    let Msg::Respawn(spec) = msg else {
        panic!("not a respawn")
    };
    assert_eq!(spec.env, [("T".to_owned(), "t".to_owned())]);
    assert_eq!(spec.extensions.unwrap().names(), ["latex"]);
}

#[test]
fn compose_merges_lenses_first_wins_within_caps_and_cuts_instructions_at_8158() {
    assert_eq!(MAX_PLUGIN_INSTRUCTIONS, 8158);
    let a = LaunchPart {
        lenses: vec![
            lens("latex", "a's latex"),
            lens("Bad", "x"),
            lens("huge", &"h".repeat(3000)),
            lens("diagram", "draw"),
        ],
        instructions: "i".repeat(5000),
        ..Default::default()
    };
    let b = LaunchPart {
        lenses: (0..MAX_LENSES)
            .map(|i| lens(&format!("l{i}"), "y"))
            .collect(),
        instructions: "j".repeat(4000),
        ..Default::default()
    };
    let c = LaunchPart {
        instructions: "  short  ".into(),
        ..Default::default()
    };
    let parts = vec![("a", a), ("b", b), ("c", c)];
    let (msg, notices) = compose(spawn_base(), core(), parts).unwrap();
    let ext = spawned(&msg).extensions.clone().unwrap();
    assert_eq!(ext.lenses.len(), MAX_LENSES);
    assert_eq!(ext.lenses[0], lens("latex", "write <latex>"), "first wins");
    assert_eq!(ext.names()[1], "diagram");
    assert_eq!(ext.names()[MAX_LENSES - 1], "l13");
    assert_eq!(ext.instructions, "i".repeat(5000), "b and c cut");
    assert_eq!(
        notices,
        [
            "a: lens Bad left out: bad name",
            "a: lens huge left out: instruction over 2048 bytes",
            "b: lens l14 left out: more than 16 lenses",
            "b: lens l15 left out: more than 16 lenses",
            "instructions over 8158 bytes: b, c left out",
        ]
    );
    // Within the cap, they are joined by a blank line.
    let small = |t: &str| LaunchPart {
        instructions: t.into(),
        ..Default::default()
    };
    let (msg, _) = compose(
        spawn_base(),
        core(),
        vec![("a", small("A")), ("b", small("B"))],
    )
    .unwrap();
    assert_eq!(
        spawned(&msg).extensions.as_ref().unwrap().instructions,
        "A\n\nB"
    );
}

#[test]
fn compose_refuses_a_frame_over_the_cap() {
    let big = "v".repeat(crate::proto::MAX_FRAME);
    let parts = vec![("a", env_part(&[("BIG", &big)]))];
    let err = compose(spawn_base(), core(), parts).unwrap_err();
    assert_eq!(err, "launch too large");
    assert!(!err.contains("vvv"));
}

#[tokio::test]
async fn a_required_addon_failure_fails_the_launch_and_sends_nothing() {
    let route = required(Script::Fail("proxy down".into()));
    let extra = Arc::new(TestAddon::new("extra").prepare(Script::Ok(env_part(&[("E", "e")]))));
    let (rt, _rx) = runtime(vec![route.clone(), extra.clone()]);
    let (client, sent) = daemon().await;
    let step = run(&rt, spec(LaunchOp::New, LaunchSelect::default()), client).await;
    assert!(
        matches!(&step, Step::Failed(why) if why == "route: proxy down"),
        "{step:?}"
    );
    assert!(nothing_sent(&sent).await);
    rt.shutdown();
}

#[tokio::test(start_paused = true)]
async fn a_required_addon_timeout_fails_the_launch() {
    let slow = Script::Delay(Duration::from_secs(60), Box::new(Script::Ok(env_part(&[]))));
    let (rt, _rx) = runtime(vec![required(slow)]);
    let (client, sent) = daemon().await;
    let budget = Duration::from_millis(200);
    let launch = spec(LaunchOp::New, LaunchSelect::default());
    let step = run_within(&rt, launch, client, budget).await;
    assert!(
        matches!(&step, Step::Failed(why) if why == "route: timed out"),
        "{step:?}"
    );
    assert!(nothing_sent(&sent).await);
    rt.shutdown();
}

#[tokio::test]
async fn an_optional_addon_failure_is_a_notice_and_the_launch_goes_on() {
    let route = required(Script::Ok(env_part(&[("ANTHROPIC_AUTH_TOKEN", "sekret")])));
    let extra = Arc::new(TestAddon::new("extra").prepare(Script::Fail("no host".into())));
    let off = Arc::new(TestAddon::new("off").routing(Ok(Routing::Off)));
    let (rt, _rx) = runtime(vec![route.clone(), extra.clone(), off.clone()]);
    let (client, sent) = daemon().await;
    let step = run(&rt, spec(LaunchOp::New, LaunchSelect::default()), client).await;
    let Step::Sent {
        choice: got,
        notices,
        reply,
    } = step
    else {
        panic!("not sent: {step:?}")
    };
    assert_eq!(got, Some(choice("route", "work")));
    assert_eq!(notices, ["extra: no host"]);
    assert!(matches!(reply, Ok(Msg::Spawned { id, .. }) if id == sid()));
    let sent = sent.lock().unwrap();
    assert_eq!(spawned(&sent[0]).env[0].0, "ANTHROPIC_AUTH_TOKEN");
    assert!(
        !off.calls().iter().any(|c| c.starts_with("prepare")),
        "Off sits out"
    );
    rt.shutdown();
}

#[tokio::test]
async fn two_required_addons_fail_the_launch() {
    let one = required(Script::Ok(LaunchPart::default()));
    let two = Arc::new(
        TestAddon::new("other")
            .routing(Ok(Routing::Required(choice("other", "x"))))
            .prepare(Script::Ok(LaunchPart::default())),
    );
    let (rt, _rx) = runtime(vec![one, two.clone()]);
    let (client, sent) = daemon().await;
    let step = run(&rt, spec(LaunchOp::New, LaunchSelect::default()), client).await;
    let want = "two addons route this launch: route, other";
    assert!(
        matches!(&step, Step::Failed(why) if why == want),
        "{step:?}"
    );
    assert!(!two.calls().iter().any(|c| c.starts_with("prepare")));
    assert!(nothing_sent(&sent).await);
    rt.shutdown();
}

#[tokio::test]
async fn an_unknown_persisted_choice_fails_the_launch() {
    let route = required(Script::Ok(LaunchPart::default()));
    let (rt, _rx) = runtime(vec![route]);
    let (client, sent) = daemon().await;
    let gone = LaunchSelect::of(Some(&choice("gone", "work")));
    let step = run(&rt, spec(LaunchOp::Recover, gone), client).await;
    let want = "launch option of `gone` is not available in this build";
    assert!(
        matches!(&step, Step::Failed(why) if why == want),
        "{step:?}"
    );
    // An addon that does not take its own option never becomes direct.
    let lazy = Arc::new(TestAddon::new("lazy").routing(Ok(Routing::Optional)));
    let (rt2, _rx2) = runtime(vec![lazy]);
    let mine = LaunchSelect::of(Some(&choice("lazy", "w")));
    assert_eq!(
        resolve(&rt2, &mine).unwrap_err(),
        "lazy: launch option w was not taken"
    );
    // An addon's Err fails it too.
    let picky = Arc::new(TestAddon::new("picky").routing(Err("no profile x".into())));
    let (rt3, _rx3) = runtime(vec![picky]);
    assert_eq!(
        resolve(&rt3, &LaunchSelect::default()).unwrap_err(),
        "picky: no profile x"
    );
    assert!(nothing_sent(&sent).await);
    for rt in [rt, rt2, rt3] {
        rt.shutdown();
    }
}

/// An addon that needs its checklist reviewed, `before` the answer.
fn reviewing(before: Duration) -> Arc<TestAddon> {
    let part = env_part(&[("R", "r")]);
    let script = Script::Delay(before, Box::new(Script::Review(part)));
    let checklist = Checklist {
        id: "setup".into(),
        revision: "1".into(),
        title: "Set up".into(),
        intro: String::new(),
        items: Vec::new(),
    };
    Arc::new(
        TestAddon::new("route")
            .routing(Ok(Routing::Required(choice("route", "work"))))
            .prepare(script)
            .checklist(checklist),
    )
}

#[tokio::test(start_paused = true)]
async fn needs_review_parks_with_the_remaining_budget_and_resume_continues() {
    let route = reviewing(Duration::from_secs(5));
    let (rt, _rx) = runtime(vec![route.clone()]);
    let (client, sent) = daemon().await;
    let step = run(
        &rt,
        spec(LaunchOp::New, LaunchSelect::default()),
        client.clone(),
    )
    .await;
    let Step::Review {
        addon,
        checklist,
        revision,
        parked,
    } = step
    else {
        panic!("not parked: {step:?}")
    };
    assert_eq!(
        (addon, checklist.as_str(), revision.as_str()),
        ("route", "setup", "1")
    );
    assert_eq!(parked.remaining(), BUDGET - Duration::from_secs(5));
    assert!(!format!("{parked:?}").contains("\"r\""), "no env in Debug");
    // The member takes 30 s, longer than the whole budget: the clock is stopped.
    tokio::time::sleep(Duration::from_secs(30)).await;
    assert!(sent.lock().unwrap().is_empty());
    let save = ReviewAnswer::Save(ChecklistAnswer::Save {
        revision: "1".into(),
        off: Vec::new(),
    });
    let step = resume(&rt, parked, save, client).await;
    assert!(matches!(&step, Step::Sent { reply: Ok(_), .. }), "{step:?}");
    assert!(route.saved());
    assert_eq!(
        spawned(&sent.lock().unwrap()[0]).env,
        [("R".into(), "r".into())]
    );
    rt.shutdown();
}

/// The items changed while the checklist was open: the save of the old
/// revision is refused, and the launch parks again on the new one (the
/// checklist reopens); the launch is never failed or sent for it.
#[tokio::test]
async fn a_stale_save_reopens_the_review_with_the_new_revision() {
    let route = reviewing(Duration::ZERO);
    let (rt, _rx) = runtime(vec![route.clone()]);
    let (client, sent) = daemon().await;
    let launch = spec(LaunchOp::New, LaunchSelect::default());
    let Step::Review { parked, .. } = run(&rt, launch, client.clone()).await else {
        panic!("not parked")
    };
    route.set_checklist(Checklist {
        id: "setup".into(),
        revision: "2".into(),
        title: "Set up".into(),
        intro: String::new(),
        items: Vec::new(),
    });
    let old = |revision: &str| {
        ReviewAnswer::Save(ChecklistAnswer::Save {
            revision: revision.into(),
            off: Vec::new(),
        })
    };
    let step = resume(&rt, parked, old("1"), client.clone()).await;
    let Step::Review {
        checklist,
        revision,
        parked,
        ..
    } = step
    else {
        panic!("not parked again: {step:?}")
    };
    assert_eq!((checklist.as_str(), revision.as_str()), ("setup", "2"));
    assert!(!route.saved());
    assert!(nothing_sent(&sent).await);
    let step = resume(&rt, parked, old("2"), client).await;
    let Step::Sent { notices, reply, .. } = step else {
        panic!("not sent: {step:?}")
    };
    assert!(reply.is_ok() && route.saved());
    assert_eq!(notices, ["route: not saved: stale revision"]);
    rt.shutdown();
}

#[tokio::test]
async fn skip_defers_only_for_that_launch() {
    let route = reviewing(Duration::ZERO);
    let (rt, _rx) = runtime(vec![route.clone()]);
    let (client, sent) = daemon().await;
    let launch = || spec(LaunchOp::New, LaunchSelect::default());
    let Step::Review { parked, .. } = run(&rt, launch(), client.clone()).await else {
        panic!("not parked")
    };
    let step = resume(&rt, parked, ReviewAnswer::Skip, client.clone()).await;
    assert!(matches!(&step, Step::Sent { reply: Ok(_), .. }), "{step:?}");
    assert!(!route.saved());
    assert!(!route
        .calls()
        .iter()
        .any(|c| c.starts_with("save_checklist")));
    assert_eq!(sent.lock().unwrap().len(), 1);
    // The next launch asks again.
    let step = run(&rt, launch(), client).await;
    assert!(matches!(step, Step::Review { .. }), "{step:?}");
    rt.shutdown();
}

#[tokio::test]
async fn cancel_fails() {
    let route = reviewing(Duration::ZERO);
    let (rt, _rx) = runtime(vec![route.clone()]);
    let (client, sent) = daemon().await;
    let launch = spec(LaunchOp::Resume, LaunchSelect::default());
    let Step::Review { parked, .. } = run(&rt, launch, client.clone()).await else {
        panic!("not parked")
    };
    let step = resume(&rt, parked, ReviewAnswer::Cancel, client).await;
    assert!(
        matches!(&step, Step::Failed(why) if why == "cancelled"),
        "{step:?}"
    );
    assert!(!route
        .calls()
        .iter()
        .any(|c| c.starts_with("save_checklist")));
    assert!(nothing_sent(&sent).await);
    rt.shutdown();
}

#[tokio::test]
async fn respawn_never_parks() {
    for op in [LaunchOp::Respawn, LaunchOp::Recover] {
        let route = reviewing(Duration::ZERO);
        let (rt, _rx) = runtime(vec![route.clone()]);
        let (client, sent) = daemon().await;
        let step = run(&rt, spec(op, LaunchSelect::default()), client).await;
        assert!(
            matches!(&step, Step::Sent { reply: Ok(_), .. }),
            "{op:?}: {step:?}"
        );
        assert!(!route.saved());
        assert_eq!(sent.lock().unwrap().len(), 1);
        rt.shutdown();
    }
}

static ROUTE_FLAGS: &[FlagDecl] = &[
    FlagDecl {
        name: "--route",
        value: Some("NAME"),
        help: "route through NAME",
        label: None,
    },
    FlagDecl {
        name: "--no-route",
        value: None,
        help: "direct",
        label: None,
    },
    FlagDecl {
        name: "--first",
        value: None,
        help: "a switch",
        label: Some("First"),
    },
];

static OTHER_FLAGS: &[FlagDecl] = &[FlagDecl {
    name: "--other",
    value: Some("NAME"),
    help: "another route",
    label: None,
}];

#[test]
fn split_flags_consumes_leading_addon_flags_in_any_order() {
    let addons = Addons::new(vec![
        Arc::new(TestAddon::new("route").launch_flags(ROUTE_FLAGS)) as Arc<dyn Addon>,
        Arc::new(TestAddon::new("other").launch_flags(OTHER_FLAGS)),
    ])
    .unwrap();
    let args = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let pick = |option: &str| Pick::Option {
        addon: "route".into(),
        option: option.into(),
    };

    let a = args(&["--first", "--route", "work", "-c"]);
    let (select, rest) = split_flags(&addons, &a).unwrap();
    assert_eq!(select.pick, pick("work"));
    assert_eq!(select.flags, ["--first"]);
    assert_eq!(rest, &args(&["-c"])[..]);

    let a = args(&["--no-route", "--first"]);
    let (select, rest) = split_flags(&addons, &a).unwrap();
    assert_eq!(select.pick, Pick::Direct);
    assert_eq!(select.flags, ["--first"]);
    assert!(rest.is_empty());

    // None given: the default, and every argument stays.
    let a = args(&["--resume", "abc", "--first"]);
    let (select, rest) = split_flags(&addons, &a).unwrap();
    assert_eq!(select, LaunchSelect::default());
    assert_eq!(rest, &a[..]);

    // A value flag needs its value.
    assert!(split_flags(&addons, &args(&["--route"])).is_err());
    assert!(split_flags(&addons, &args(&["--first", "--route"])).is_err());
    // An addon's own usage error, and two routes.
    assert!(split_flags(&addons, &args(&["--route", ""])).is_err());
    let two = args(&["--route", "a", "--other", "b"]);
    assert_eq!(
        split_flags(&addons, &two).unwrap_err(),
        "launch flags pick two different routes"
    );
}

/// A launch right after the runtime starts (a `--plain` session, the web's
/// first spawn) resolves once the addons' start is done: the tasks an addon
/// spawned for `Started` read its state off the executor, and `resolve`
/// reads it without I/O.
#[tokio::test]
async fn a_launch_at_the_start_resolves_after_the_addons_start() {
    let addon = Arc::new(
        TestAddon::new("route")
            .loads_at_start(Duration::from_millis(300))
            .routing(Ok(Routing::Optional)),
    );
    let (rt, _rx) = runtime(vec![addon.clone() as Arc<dyn Addon>]);
    let (client, sent) = daemon().await;
    let step = run(&rt, spec(LaunchOp::New, LaunchSelect::default()), client).await;
    assert!(matches!(step, Step::Sent { .. }), "{step:?}");
    assert_eq!(sent.lock().unwrap().len(), 1);
    let calls = addon.calls();
    let started = calls.iter().position(|c| c == "event Started");
    let resolved = calls.iter().position(|c| c.starts_with("resolve"));
    assert!(started.is_some() && started < resolved, "{calls:?}");
    // Started once: a later launch does not wait again.
    let (client, _) = daemon().await;
    let begun = std::time::Instant::now();
    let step = run(&rt, spec(LaunchOp::New, LaunchSelect::default()), client).await;
    assert!(matches!(step, Step::Sent { .. }), "{step:?}");
    assert!(begun.elapsed() < Duration::from_millis(250));
    rt.shutdown();
}
