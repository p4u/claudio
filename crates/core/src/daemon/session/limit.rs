//! The usage-limit auto-switch (docs/manager-design.md, "Usage limits"): when a turn ends
//! on a usage limit, the daemon asks its addons to move the conversation to another
//! subscription and, when one did, types `continue`, then Enter, into the session.
//!
//! [`Guard`] decides and is pure: it takes hooks, client input and its own [`Step`]s, and
//! returns [`Effect`]s. The actor runs them: notices, PTY writes, timers (tasks that send it
//! a `Cmd::Limit` later, never a sleep on the actor) and the attempt, a task of its own under
//! `Config::limit_attempt_timeout`. Timers and attempts hold the actor's channel weakly and
//! report to it only, so a respawned session's new actor never hears them. Names no addon.

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::mpsc::WeakSender;

use super::{Actor, Cmd};
use crate::addon::LimitOutcome;
use crate::claude::hooks;
use crate::daemon::{addon_ops, Daemon};
use crate::proto::{SessionEvent, SessionId, SessionState};

/// Automatic continues in a row before the session is left to its user.
const MAX_CONTINUES: u8 = 3;
/// From an accepted switch to typing `continue`.
const SETTLE: Duration = Duration::from_secs(1);
/// From `continue` to Enter: Enter too early drops the just-typed text (the api driver's rule).
const DWELL: Duration = Duration::from_millis(150);
/// claude's message as an addon gets it, in bytes.
const MESSAGE_BYTES: usize = 256;
/// What the daemon types, in two writes.
const CONTINUE: &[u8] = b"continue";
const ENTER: &[u8] = b"\r";

/// The guard's own messages to its actor (`Cmd::Limit`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::daemon) enum Step {
    /// The attempt for hit `seq` ended with `outcome` (strings already cleaned).
    Done { seq: u64, outcome: LimitOutcome },
    /// The settle after hit `seq`'s switch is over: type `continue`.
    Settled { seq: u64, from: String, to: String },
    /// The dwell after `continue` is over: press Enter.
    Enter { seq: u64, from: String, to: String },
}

/// What the actor does for the guard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Effect {
    /// Broadcast this session notice.
    Notice(String),
    /// Ask the addons about hit `seq`, off the actor.
    Attempt {
        seq: u64,
        conversation: String,
        message: String,
    },
    /// Write these bytes to the session's PTY.
    Write(&'static [u8]),
    /// Send the actor `Cmd::Limit(step)` after this long.
    Later(Duration, Step),
}

/// The auto-switch state of one live session. A respawn starts a new one at zero.
#[derive(Debug, Default)]
pub(super) struct Guard {
    /// Raised on every usage-limit hit: a step for an older hit types nothing.
    seq: u64,
    /// Automatic continues since the last turn that ended normally (`Stop`).
    continues: u8,
    /// An attempt is in flight.
    pending: bool,
    /// A client typed since its last submitted prompt (`UserPromptSubmit`).
    typed: bool,
}

impl Guard {
    /// A hook, after the tracker's share; `conversation` is claude's session id, if known.
    pub(super) fn hook(
        &mut self,
        event: &str,
        payload: &Value,
        conversation: Option<&str>,
    ) -> Vec<Effect> {
        match event {
            "Stop" => self.continues = 0,
            "UserPromptSubmit" => self.typed = false,
            "StopFailure" if hooks::is_usage_limit(payload) => {
                return self.hit(payload, conversation)
            }
            _ => {}
        }
        Vec::new()
    }

    /// A client's bytes reached the session (never the daemon's own writes).
    pub(super) fn typed(&mut self) {
        self.typed = true;
    }

    /// One of the guard's own steps; `state` is the session's state now.
    pub(super) fn step(&mut self, step: Step, state: SessionState) -> Vec<Effect> {
        match step {
            Step::Done { seq, outcome } => {
                self.pending = false;
                match outcome {
                    LimitOutcome::NotMine => Vec::new(),
                    LimitOutcome::Failed(why) => vec![Effect::Notice(format!(
                        "usage limit: could not switch subscription: {why}"
                    ))],
                    LimitOutcome::Switched { from, to } => {
                        vec![Effect::Later(SETTLE, Step::Settled { seq, from, to })]
                    }
                }
            }
            Step::Settled { seq, from, to } if self.may_type(seq, state) => vec![
                Effect::Write(CONTINUE),
                Effect::Later(DWELL, Step::Enter { seq, from, to }),
            ],
            Step::Enter { seq, from, to } if self.may_type(seq, state) => {
                self.continues = self.continues.saturating_add(1);
                let n = self.continues;
                vec![
                    Effect::Write(ENTER),
                    Effect::Notice(format!(
                        "usage limit: switched {from} → {to}, continuing ({n}/{MAX_CONTINUES})"
                    )),
                ]
            }
            Step::Settled { to, .. } | Step::Enter { to, .. } => vec![Effect::Notice(format!(
                "usage limit: switch to {to} scheduled; not continuing automatically"
            ))],
        }
    }

    fn hit(&mut self, payload: &Value, conversation: Option<&str>) -> Vec<Effect> {
        self.seq += 1;
        if self.pending {
            // The attempt in flight reports for this hit too.
            return Vec::new();
        }
        if self.continues >= MAX_CONTINUES {
            return vec![Effect::Notice(format!(
                "usage limit: {MAX_CONTINUES} automatic switches in a row; continue by hand"
            ))];
        }
        let Some(conversation) = conversation else {
            return Vec::new();
        };
        self.pending = true;
        let message = payload["last_assistant_message"].as_str().unwrap_or("");
        vec![Effect::Attempt {
            seq: self.seq,
            conversation: conversation.to_owned(),
            message: crate::addon::line(message.trim(), MESSAGE_BYTES),
        }]
    }

    /// Whether the daemon may type for hit `seq`: no newer hit, the session still sits in
    /// the error that hit left, and no client typed since its last submitted prompt. The
    /// actor handles the step, so its process has not been reaped.
    fn may_type(&self, seq: u64, state: SessionState) -> bool {
        seq == self.seq && state == SessionState::Error && !self.typed
    }
}

/// One actor's guard and what it acts with.
pub(super) struct AutoSwitch {
    guard: Guard,
    /// The actor's own channel, weak: a timer or attempt never keeps it open.
    tx: WeakSender<Cmd>,
    /// Debug builds only ([`test_trigger`]): the next `Stop` is a usage limit that
    /// switches without asking an addon.
    test: bool,
}

impl AutoSwitch {
    /// A guard at zero for the actor behind `tx`.
    pub(super) fn new(tx: WeakSender<Cmd>) -> AutoSwitch {
        AutoSwitch {
            guard: Guard::default(),
            tx,
            test: test_trigger(),
        }
    }
}

impl Actor {
    /// A hook: the tracker's share ([`Actor::on_hook`]), then the guard's.
    pub(super) fn hook(&mut self, event: String, payload: Value) {
        let test = self.limit.test && event == "Stop";
        let (event, payload) = match test {
            true => {
                self.limit.test = false;
                test_hit()
            }
            false => (event, payload),
        };
        self.on_hook(&event, &payload);
        let conversation = self.tracker.claude_session_id.clone();
        let mut effects = self
            .limit
            .guard
            .hook(&event, &payload, conversation.as_deref());
        if test {
            effects = effects.into_iter().map(switched_here).collect();
        }
        self.limit_effects(effects);
    }

    /// A client's bytes: written; typing turns the next automatic `continue` off until
    /// the client submits a prompt.
    pub(super) fn client_input(&mut self, bytes: Vec<u8>) {
        if is_typing(&bytes) {
            self.limit.guard.typed();
        }
        self.write(bytes);
    }

    /// One of the guard's own steps.
    pub(super) fn limit_step(&mut self, step: Step) {
        let effects = self.limit.guard.step(step, self.tracker.state);
        self.limit_effects(effects);
    }

    fn limit_effects(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::Notice(text) => self
                    .daemon
                    .broadcast(self.id, SessionEvent::Notice { text }),
                Effect::Write(bytes) => self.write(bytes.to_vec()),
                Effect::Later(after, step) => later(self.limit.tx.clone(), after, step),
                Effect::Attempt {
                    seq,
                    conversation,
                    message,
                } => {
                    let daemon = Arc::clone(&self.daemon);
                    let tx = self.limit.tx.clone();
                    attempt(daemon, self.id, seq, conversation, message, tx);
                }
            }
        }
    }
}

/// Whether a client's bytes may put text in claude's input box: anything but focus
/// reports (`ESC [ I`, `ESC [ O`) and SGR mouse reports (`ESC [ < … M` or `m`), which a
/// front-end sends on its own when its window loses focus or the user scrolls.
fn is_typing(mut bytes: &[u8]) -> bool {
    loop {
        bytes = match bytes {
            [] => return false,
            [0x1b, b'[', b'I' | b'O', rest @ ..] => rest,
            [0x1b, b'[', b'<', rest @ ..] => {
                let params = rest
                    .iter()
                    .take_while(|b| b.is_ascii_digit() || **b == b';')
                    .count();
                match rest.get(params..) {
                    Some([b'M' | b'm', rest @ ..]) => rest,
                    _ => return true,
                }
            }
            _ => return true,
        }
    }
}

/// Send the actor `step` after `after`, if it is still there.
fn later(tx: WeakSender<Cmd>, after: Duration, step: Step) {
    tokio::spawn(async move {
        tokio::time::sleep(after).await;
        if let Some(tx) = tx.upgrade() {
            let _ = tx.send(Cmd::Limit(step)).await;
        }
    });
}

/// Ask the addons about hit `seq` of session `id` ([`addon_ops::usage_limit`]) on a task
/// of its own, within `Config::limit_attempt_timeout`, and report one `Step::Done` to the
/// actor if it is still there. A timeout aborts the addons' task; a panic in it is
/// `Failed("addon panicked")`, so the guard is never left pending.
fn attempt(
    daemon: Arc<Daemon>,
    id: SessionId,
    seq: u64,
    conversation: String,
    message: String,
    tx: WeakSender<Cmd>,
) {
    tokio::spawn(async move {
        let env = daemon.spawn_env(id);
        let budget = daemon.config.limit_attempt_timeout;
        let ask = addon_ops::usage_limit(Arc::clone(&daemon), conversation, message, env);
        let mut task = tokio::spawn(ask);
        let outcome = match tokio::time::timeout(budget, &mut task).await {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(_)) => LimitOutcome::Failed("addon panicked".to_owned()),
            Err(_) => {
                task.abort();
                LimitOutcome::Failed("timed out".to_owned())
            }
        };
        tracing::info!(%id, seq, ?outcome, "usage limit attempt");
        if let Some(tx) = tx.upgrade() {
            let _ = tx.send(Cmd::Limit(Step::Done { seq, outcome })).await;
        }
    });
}

impl Daemon {
    /// Session `id`'s last spawn env, copied out of the registry (empty for none).
    fn spawn_env(&self, id: SessionId) -> Vec<(String, String)> {
        let reg = self.registry();
        reg.session_env
            .get(&id)
            .map(|kept| kept.env.clone())
            .unwrap_or_default()
    }
}

/// `CLAUDIO_TEST_LIMIT_SWITCH=1` in a debug build's daemon env: each session's first
/// `Stop` becomes a usage limit whose attempt switches without asking an addon, so a
/// real-claude test can drive the type-dwell-Enter path. A release build has no such
/// trigger, and `make static` checks that the name is not in the binary.
#[cfg(debug_assertions)]
fn test_trigger() -> bool {
    std::env::var_os("CLAUDIO_TEST_LIMIT_SWITCH").is_some_and(|v| v == "1")
}

#[cfg(not(debug_assertions))]
fn test_trigger() -> bool {
    false
}

/// The usage-limit `StopFailure` the test trigger puts in place of a `Stop`.
fn test_hit() -> (String, Value) {
    let payload = serde_json::json!({
        "hook_event_name": "StopFailure",
        "error": "rate_limit",
        "last_assistant_message": "You've hit your test limit",
    });
    ("StopFailure".to_owned(), payload)
}

/// The test trigger's attempt: switched here, without asking an addon.
fn switched_here(effect: Effect) -> Effect {
    match effect {
        Effect::Attempt { seq, .. } => {
            let outcome = LimitOutcome::Switched {
                from: "test".to_owned(),
                to: "test".to_owned(),
            };
            Effect::Later(Duration::ZERO, Step::Done { seq, outcome })
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const LIMIT: &str = "You've hit your 5-hour limit · resets 4pm (UTC)";

    fn limit() -> Value {
        json!({"error": "rate_limit", "last_assistant_message": format!(" {LIMIT}\n")})
    }

    fn switched(seq: u64) -> Step {
        let outcome = LimitOutcome::Switched {
            from: "a".into(),
            to: "b".into(),
        };
        Step::Done { seq, outcome }
    }

    fn settled(seq: u64) -> Step {
        Step::Settled {
            seq,
            from: "a".into(),
            to: "b".into(),
        }
    }

    fn enter(seq: u64) -> Step {
        Step::Enter {
            seq,
            from: "a".into(),
            to: "b".into(),
        }
    }

    fn notice(text: &str) -> Vec<Effect> {
        vec![Effect::Notice(text.to_owned())]
    }

    const NOT_CONTINUING: &str = "usage limit: switch to b scheduled; not continuing automatically";

    /// One hit through to Enter; the continue notice it ends with.
    fn continue_once(g: &mut Guard) -> Vec<Effect> {
        let seq = match &g.hook("StopFailure", &limit(), Some("conv"))[..] {
            [Effect::Attempt { seq, .. }] => *seq,
            other => panic!("expected an attempt, got {other:?}"),
        };
        assert_eq!(
            g.step(switched(seq), SessionState::Error),
            [Effect::Later(SETTLE, settled(seq))]
        );
        assert_eq!(
            g.step(settled(seq), SessionState::Error),
            [Effect::Write(b"continue"), Effect::Later(DWELL, enter(seq))]
        );
        g.step(enter(seq), SessionState::Error)
    }

    #[test]
    fn a_hit_asks_once_with_the_conversation_and_the_trimmed_message() {
        let mut g = Guard::default();
        let attempt = Effect::Attempt {
            seq: 1,
            conversation: "conv".into(),
            message: LIMIT.into(),
        };
        assert_eq!(g.hook("StopFailure", &limit(), Some("conv")), [attempt]);
        // A second hit while the attempt is in flight asks nothing and says nothing.
        assert!(g.hook("StopFailure", &limit(), Some("conv")).is_empty());
        // Other failures, and a hit before claude named its conversation, do nothing.
        let mut g = Guard::default();
        let overloaded = json!({"error": "overloaded", "last_assistant_message": LIMIT});
        assert!(g.hook("StopFailure", &overloaded, Some("conv")).is_empty());
        assert!(g.hook("StopFailure", &limit(), None).is_empty());
        assert!(!g.pending);
    }

    #[test]
    fn switched_types_continue_then_enter_and_counts() {
        let mut g = Guard::default();
        assert_eq!(
            continue_once(&mut g),
            [
                Effect::Write(b"\r"),
                Effect::Notice("usage limit: switched a → b, continuing (1/3)".into())
            ]
        );
    }

    #[test]
    fn three_in_a_row_then_the_give_up_and_a_stop_resets() {
        let mut g = Guard::default();
        for n in 1..=3 {
            let want = format!("usage limit: switched a → b, continuing ({n}/3)");
            assert_eq!(continue_once(&mut g)[1], Effect::Notice(want));
        }
        let give_up = notice("usage limit: 3 automatic switches in a row; continue by hand");
        assert_eq!(g.hook("StopFailure", &limit(), Some("conv")), give_up);
        assert_eq!(g.hook("StopFailure", &limit(), Some("conv")), give_up);
        assert!(g.hook("Stop", &json!({}), Some("conv")).is_empty());
        let want = "usage limit: switched a → b, continuing (1/3)";
        assert_eq!(continue_once(&mut g)[1], Effect::Notice(want.into()));
    }

    #[test]
    fn failed_and_not_mine_type_nothing() {
        let mut g = Guard::default();
        g.hook("StopFailure", &limit(), Some("conv"));
        let failed = Step::Done {
            seq: 1,
            outcome: LimitOutcome::Failed("no other eligible credential".into()),
        };
        assert_eq!(
            g.step(failed, SessionState::Error),
            notice("usage limit: could not switch subscription: no other eligible credential")
        );
        // Not pending any more: the next hit asks again.
        assert!(matches!(
            g.hook("StopFailure", &limit(), Some("conv"))[..],
            [Effect::Attempt { seq: 2, .. }]
        ));
        let not_mine = Step::Done {
            seq: 2,
            outcome: LimitOutcome::NotMine,
        };
        assert!(g.step(not_mine, SessionState::Error).is_empty());
        assert!(!g.pending);
    }

    #[test]
    fn client_input_blocks_continue_until_a_prompt_is_submitted() {
        // Typed during the failing turn: the hit does not clear it.
        let mut g = Guard::default();
        g.typed();
        g.hook("StopFailure", &limit(), Some("conv"));
        g.step(switched(1), SessionState::Error);
        assert_eq!(
            g.step(settled(1), SessionState::Error),
            notice(NOT_CONTINUING)
        );
        // Typed between the hit and the settle.
        let mut g = Guard::default();
        g.hook("StopFailure", &limit(), Some("conv"));
        g.step(switched(1), SessionState::Error);
        g.typed();
        assert_eq!(
            g.step(settled(1), SessionState::Error),
            notice(NOT_CONTINUING)
        );
        // Typed between `continue` and Enter: no Enter.
        let mut g = Guard::default();
        g.hook("StopFailure", &limit(), Some("conv"));
        g.step(switched(1), SessionState::Error);
        g.step(settled(1), SessionState::Error);
        g.typed();
        assert_eq!(
            g.step(enter(1), SessionState::Error),
            notice(NOT_CONTINUING)
        );
        assert_eq!(g.continues, 0);
        // A submitted prompt clears it.
        let mut g = Guard::default();
        g.typed();
        g.hook("UserPromptSubmit", &json!({}), Some("conv"));
        assert_eq!(continue_once(&mut g)[0], Effect::Write(b"\r"));
    }

    #[test]
    fn a_newer_hit_or_a_state_change_types_nothing() {
        let mut g = Guard::default();
        g.hook("StopFailure", &limit(), Some("conv"));
        g.hook("StopFailure", &limit(), Some("conv"));
        g.step(switched(1), SessionState::Error);
        assert_eq!(
            g.step(settled(1), SessionState::Error),
            notice(NOT_CONTINUING)
        );
        let mut g = Guard::default();
        g.hook("StopFailure", &limit(), Some("conv"));
        g.step(switched(1), SessionState::Error);
        let working = SessionState::Working;
        assert_eq!(g.step(settled(1), working), notice(NOT_CONTINUING));
    }

    #[test]
    fn focus_and_mouse_reports_are_not_typing() {
        for quiet in [
            &b""[..],
            b"\x1b[I",
            b"\x1b[O\x1b[I",
            b"\x1b[<64;10;5M",
            b"\x1b[<0;1;1m\x1b[O",
        ] {
            assert!(!is_typing(quiet), "{quiet:?}");
        }
        for typing in [
            &b"x"[..],
            b"\r",
            b"\x1b[A",
            b"\x1b[I x",
            b"\x1b[<64;10",
            b"\x1b[<64;10;5X",
            b"\x1b[200~hi\x1b[201~",
        ] {
            assert!(is_typing(typing), "{typing:?}");
        }
    }

    #[test]
    fn the_test_trigger_is_a_usage_limit_answered_here() {
        let (event, payload) = test_hit();
        assert_eq!(event, "StopFailure");
        assert!(hooks::is_usage_limit(&payload));
        let attempt = Effect::Attempt {
            seq: 4,
            conversation: "c".into(),
            message: "m".into(),
        };
        let outcome = LimitOutcome::Switched {
            from: "test".into(),
            to: "test".into(),
        };
        let done = Step::Done { seq: 4, outcome };
        assert_eq!(switched_here(attempt), Effect::Later(Duration::ZERO, done));
        let other = Effect::Notice("n".into());
        assert_eq!(switched_here(other.clone()), other);
    }
}
