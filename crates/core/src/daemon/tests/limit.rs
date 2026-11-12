//! Daemon tests for the usage-limit auto-switch (`session/limit.rs`): a
//! `TestAddon` answers `on_usage_limit`, the test sends the hooks a claude
//! would (`send_hook`), and a fake claude with a raw terminal logs each read
//! of its input as one line of `input.log`, so two PTY writes stay two lines.

use std::sync::OnceLock;

use serde_json::json;

use super::*;
use crate::addon::test_support::{LimitScript, TestAddon};
use crate::addon::{Addon, LimitOutcome};

/// claude's message for the hits the tests send.
const LIMIT: &str = "You've hit your 5-hour limit · resets 4pm (UTC)";

/// The notice of a switch that was not continued.
const NOT_CONTINUING: &str = "usage limit: switch to sub-b scheduled; not continuing automatically";

/// A fake claude with a raw terminal: it saves its `--settings` (the hook
/// token) and prints the banner like the shared fake, then appends each read
/// of its stdin to `input.log` in its cwd as one line (`od -c`'s characters,
/// spaces dropped: `continue`, `\r`).
fn recording_claude() -> &'static Path {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let dir = private_temp_dir("claudio-fake-claude-limit");
        let bin = dir.join("claude");
        let script = format!(
            "#!/bin/sh\n\
             if [ \"$1\" = \"--version\" ]; then echo '9.9.9 (Claude Code)'; exit 0; fi\n\
             if [ \"$1\" = \"--help\" ]; then exit 0; fi\n\
             stty raw -echo\n\
             printf '%s' \"$2\" > settings.json\n\
             echo {BANNER}\n\
             while :; do\n\
             chunk=$(dd bs=256 count=1 2>/dev/null | od -An -c | tr -d ' \\n')\n\
             [ -n \"$chunk\" ] || exit 0\n\
             printf '%s\\n' \"$chunk\" >> input.log\n\
             done\n"
        );
        write_executable(&bin, &script);
        bin
    })
}

fn switched() -> LimitScript {
    LimitScript::Answer(LimitOutcome::Switched {
        from: "sub-a".into(),
        to: "sub-b".into(),
    })
}

/// A daemon running the recording claude, `addons` (in this order) and an
/// attempt budget of `budget`.
fn start_with(addons: &[&Arc<TestAddon>], budget: Duration) -> TestDaemon {
    let dir = TestDaemon::new_dir();
    let list = addons
        .iter()
        .map(|a| Arc::clone(a) as Arc<dyn Addon>)
        .collect();
    let config = Config {
        claude: recording_claude().to_path_buf(),
        limit_attempt_timeout: budget,
        ..TestDaemon::config(&dir)
    };
    TestDaemon::start_with_addons(dir, config, Addons::new(list).unwrap())
}

/// A daemon whose one addon, `limit`, follows `script`.
fn start(script: LimitScript) -> (TestDaemon, Arc<TestAddon>) {
    let addon = Arc::new(TestAddon::new("limit").on_limit(script));
    (start_with(&[&addon], Duration::from_secs(20)), addon)
}

/// Spawn a session, attach `c` to it, and tell the daemon its conversation
/// (`conv-1`); its id and hook token.
async fn session(d: &TestDaemon, c: &mut Client) -> (SessionId, String) {
    let id = Uuid::new_v4();
    c.spawn(d.spec(id)).await;
    let token = d.token().await;
    c.attach_until(id, BANNER).await;
    let start = json!({"session_id": "conv-1", "source": "startup"});
    send_hook(&d.config.socket, &token, "SessionStart", start).await;
    (id, token)
}

/// A usage-limit `StopFailure`, as claude sends it.
async fn hit(d: &TestDaemon, token: &str) {
    let payload = json!({"hook_event_name": "StopFailure", "error": "rate_limit",
                         "last_assistant_message": LIMIT});
    send_hook(&d.config.socket, token, "StopFailure", payload).await;
}

/// The reads the fake claude logged so far.
fn input(d: &TestDaemon) -> Vec<String> {
    let log = std::fs::read_to_string(d.work().join("input.log")).unwrap_or_default();
    log.lines().map(str::to_owned).collect()
}

/// The reads, once there are `n`.
async fn input_until(d: &TestDaemon, n: usize) -> Vec<String> {
    within(async {
        loop {
            let got = input(d);
            if got.len() >= n {
                return got;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
}

/// The next notice of session `id`.
async fn notice(c: &mut Client, id: SessionId) -> String {
    c.event(id, |e| match e {
        SessionEvent::Notice { text } => Some(text.clone()),
        _ => None,
    })
    .await
}

/// The usage-limit calls `addon` recorded.
fn asked(addon: &TestAddon) -> Vec<String> {
    let calls = addon.calls();
    calls
        .into_iter()
        .filter(|c| c.starts_with("usage_limit"))
        .collect()
}

/// Until `addon` has been asked `n` times.
async fn asked_until(addon: &TestAddon, n: usize) {
    within(async {
        while asked(addon).len() < n {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
}

/// AC2 Switched, with two subscribed clients (one write, whatever the
/// clients): `continue` and then a separate `\r`, once, and the notice. The
/// addons are asked in registry order until one answers.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn switched_types_continue_then_a_separate_enter_once() {
    let quiet =
        Arc::new(TestAddon::new("quiet").on_limit(LimitScript::Answer(LimitOutcome::NotMine)));
    let proxy = Arc::new(TestAddon::new("proxy").on_limit(switched()));
    let d = start_with(&[&quiet, &proxy], Duration::from_secs(20));
    let mut a = d.client().await;
    let (id, token) = session(&d, &mut a).await;
    let mut b = d.client().await;
    b.attach_until(id, BANNER).await;
    hit(&d, &token).await;
    let want = "usage limit: switched sub-a → sub-b, continuing (1/3)";
    assert_eq!(notice(&mut a, id).await, want);
    assert_eq!(notice(&mut b, id).await, want);
    assert_eq!(input_until(&d, 2).await, ["continue", "\\r"]);
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(input(&d), ["continue", "\\r"], "typed once");
    let call = format!("usage_limit conv-1 {LIMIT} SECRET_ENV=<set>");
    assert_eq!(asked(&quiet), [call.as_str()]);
    assert_eq!(asked(&proxy), [call.as_str()]);
}

/// AC2 Repeated: three continues in a row, the give-up on the fourth hit; a
/// turn that ends normally (`Stop`) starts the count over.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn four_hits_without_a_stop_continue_three_times_then_give_up() {
    let (d, addon) = start(switched());
    let mut c = d.client().await;
    let (id, token) = session(&d, &mut c).await;
    for n in 1..=3 {
        hit(&d, &token).await;
        let want = format!("usage limit: switched sub-a → sub-b, continuing ({n}/3)");
        assert_eq!(notice(&mut c, id).await, want);
    }
    hit(&d, &token).await;
    let give_up = "usage limit: 3 automatic switches in a row; continue by hand";
    assert_eq!(notice(&mut c, id).await, give_up);
    assert_eq!(asked(&addon).len(), 3, "the fourth hit asks no addon");
    assert_eq!(input_until(&d, 6).await.len(), 6);

    send_hook(&d.config.socket, &token, "Stop", json!({})).await;
    hit(&d, &token).await;
    let want = "usage limit: switched sub-a → sub-b, continuing (1/3)";
    assert_eq!(notice(&mut c, id).await, want);
}

/// AC2 Failed: a notice with the addon's reason, nothing typed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_switch_is_a_notice_and_types_nothing() {
    let why = "no other eligible credential can serve this session";
    let (d, _) = start(LimitScript::Answer(LimitOutcome::Failed(why.into())));
    let mut c = d.client().await;
    let (id, token) = session(&d, &mut c).await;
    hit(&d, &token).await;
    let want = format!("usage limit: could not switch subscription: {why}");
    assert_eq!(notice(&mut c, id).await, want);
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(input(&d).is_empty(), "{:?}", input(&d));
}

/// AC2 NotMine: nothing typed and no notice.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn not_mine_types_nothing_and_says_nothing() {
    let (d, addon) = start(LimitScript::Answer(LimitOutcome::NotMine));
    let mut c = d.client().await;
    let (id, token) = session(&d, &mut c).await;
    hit(&d, &token).await;
    asked_until(&addon, 1).await;
    let any_notice = c.event(id, |e| {
        matches!(e, SessionEvent::Notice { .. }).then_some(())
    });
    let quiet = tokio::time::timeout(Duration::from_millis(1500), any_notice).await;
    assert!(quiet.is_err(), "a notice for a session no addon claims");
    assert!(input(&d).is_empty(), "{:?}", input(&d));
}

/// AC2 Client input: typed during the failing turn or between the hit and
/// the settle, nothing is typed and the "scheduled" notice says why; a
/// submitted prompt (`UserPromptSubmit`) clears it. A focus report is not
/// typing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_input_blocks_continue_until_a_prompt_is_submitted() {
    let (d, addon) = start(switched());
    let mut c = d.client().await;
    let (id, token) = session(&d, &mut c).await;
    let typed = |text: &str| Frame::Data {
        session: id,
        bytes: text.as_bytes().to_vec(),
    };
    let continued = |n: u8| format!("usage limit: switched sub-a → sub-b, continuing ({n}/3)");
    // A front-end's focus report blocks nothing.
    c.send(typed("\x1b[O")).await;
    input_until(&d, 1).await;
    hit(&d, &token).await;
    assert_eq!(notice(&mut c, id).await, continued(1));
    assert_eq!(input_until(&d, 3).await[1..], ["continue", "\\r"]);
    // Typed during the turn, before the hit.
    c.send(typed("x")).await;
    assert_eq!(input_until(&d, 4).await[3], "x");
    hit(&d, &token).await;
    assert_eq!(notice(&mut c, id).await, NOT_CONTINUING);
    // The prompt was submitted: the next hit continues.
    send_hook(&d.config.socket, &token, "UserPromptSubmit", json!({})).await;
    hit(&d, &token).await;
    assert_eq!(notice(&mut c, id).await, continued(2));
    assert_eq!(input_until(&d, 6).await[4..], ["continue", "\\r"]);
    // Typed between the hit and the settle.
    hit(&d, &token).await;
    asked_until(&addon, 4).await;
    c.send(typed("y")).await;
    assert_eq!(notice(&mut c, id).await, NOT_CONTINUING);
    assert_eq!(input_until(&d, 7).await[4..], ["continue", "\\r", "y"]);
}

/// AC2: a hit while an attempt is in flight starts no second attempt (and,
/// being newer, keeps the first from typing).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_hit_while_an_attempt_is_in_flight_starts_none() {
    let slow = LimitScript::Delay(Duration::from_millis(800), Box::new(switched()));
    let (d, addon) = start(slow);
    let mut c = d.client().await;
    let (id, token) = session(&d, &mut c).await;
    hit(&d, &token).await;
    hit(&d, &token).await;
    assert_eq!(notice(&mut c, id).await, NOT_CONTINUING);
    assert_eq!(asked(&addon).len(), 1);
    assert!(input(&d).is_empty(), "{:?}", input(&d));
}

/// AC2 Timeout: past `limit_attempt_timeout`, `Failed("timed out")`; the
/// abandoned answer never arrives, and the next hit can attempt again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_attempt_past_its_budget_fails_and_the_next_hit_tries_again() {
    let never = LimitScript::Delay(Duration::from_secs(30), Box::new(switched()));
    let addon = Arc::new(TestAddon::new("limit").on_limit(never));
    let d = start_with(&[&addon], Duration::from_millis(300));
    let mut c = d.client().await;
    let (id, token) = session(&d, &mut c).await;
    let started = std::time::Instant::now();
    hit(&d, &token).await;
    let want = "usage limit: could not switch subscription: timed out";
    assert_eq!(notice(&mut c, id).await, want);
    assert!(started.elapsed() < Duration::from_secs(5));
    addon.set_limit(switched());
    hit(&d, &token).await;
    let want = "usage limit: switched sub-a → sub-b, continuing (1/3)";
    assert_eq!(notice(&mut c, id).await, want);
    assert_eq!(input_until(&d, 2).await, ["continue", "\\r"]);
}

/// AC2 Panic: an addon future that panics is `Failed("addon panicked")`,
/// and the next hit can attempt again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_panicking_addon_fails_and_the_next_hit_tries_again() {
    let (d, addon) = start(LimitScript::Panic);
    let mut c = d.client().await;
    let (id, token) = session(&d, &mut c).await;
    hit(&d, &token).await;
    let want = "usage limit: could not switch subscription: addon panicked";
    assert_eq!(notice(&mut c, id).await, want);
    addon.set_limit(switched());
    hit(&d, &token).await;
    let want = "usage limit: switched sub-a → sub-b, continuing (1/3)";
    assert_eq!(notice(&mut c, id).await, want);
}
