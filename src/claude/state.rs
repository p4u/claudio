//! Pure state machine that turns Claude Code hook events into
//! [`proto::SessionEvent`]s.
//!
//! The tracker holds the current [`SessionState`], the active claude
//! conversation id and what the session waits for ([`Attention`]). Every
//! call to [`Tracker::apply`] returns the events to broadcast: real state or
//! id changes, and one `Attention` for every hook that asks for the user,
//! whether or not the state changed (a second prompt in a row is news).

use serde_json::Value;

use crate::proto::{Attention, SessionEvent, SessionState};
use crate::text::sanitize;

/// The most an attention's `kind` takes, in bytes.
pub const MAX_KIND: usize = 64;
/// The most an attention's `message` takes, in bytes.
pub const MAX_MESSAGE: usize = 512;
/// The most an attention's `detail` takes, in bytes.
pub const MAX_DETAIL: usize = 1024;
/// The most a tool name takes, in bytes.
pub const MAX_TOOL: usize = 64;
/// The kind of the attention a `SessionStart` (startup, resume, clear) sets.
pub const SESSION_START: &str = "session_start";

/// Notification types that wait for an approval or a dialog's answer.
const APPROVAL: [&str; 4] = [
    "permission_prompt",
    "worker_permission_prompt",
    "elicitation_dialog",
    "elicitation_url_dialog",
];
/// Notification types that wait for the next message.
const INPUT: [&str; 2] = ["idle_prompt", "agent_needs_input"];

/// Per-session hook event tracker.
///
/// Start a fresh `Tracker` when a session is spawned. Feed each incoming hook
/// payload to [`apply`](Self::apply); it returns the `SessionEvent`s to
/// broadcast.
#[derive(Debug, Clone)]
pub struct Tracker {
    /// Current derived state.
    pub state: SessionState,
    /// The claude conversation id reported by the most recent `SessionStart`.
    pub claude_session_id: Option<String>,
    /// What the session waits for; `None` unless a hook asked and no turn
    /// event came since.
    pub attention: Option<Attention>,
    /// The last `seq` given out (0: none yet).
    seq: u64,
    /// The most recent `PreToolUse` of this turn: tool name and summary.
    last_tool: Option<(String, String)>,
}

impl Tracker {
    /// Construct a tracker in the `Starting` state (no hook has fired yet).
    pub fn new() -> Self {
        Self {
            state: SessionState::Starting,
            claude_session_id: None,
            attention: None,
            seq: 0,
            last_tool: None,
        }
    }

    /// Apply one hook event and return the list of protocol events to broadcast.
    ///
    /// Redundant state transitions are suppressed; an asking hook always
    /// yields an `Attention`. Unknown events and unknown notification types
    /// are ignored.
    pub fn apply(&mut self, event: &str, payload: &Value) -> Vec<SessionEvent> {
        match event {
            "SessionStart" => self.handle_session_start(payload),
            "PreToolUse" => self.handle_pre_tool(payload),
            // Only the asking tools are subscribed: the user answered.
            "PostToolUse" => self.end_wait(SessionState::Working),
            "UserPromptSubmit" => self.end_wait(SessionState::Working),
            "Notification" => self.handle_notification(payload),
            "Stop" => self.end_wait(SessionState::Idle),
            "StopFailure" => self.end_wait(SessionState::Error),
            // `SessionEnd`: process exit is reported separately.
            _ => vec![],
        }
    }

    // ── private helpers ───────────────────────────────────────────────────────

    /// A turn boundary: nothing is asked any more, and the next prompt's
    /// tool comes from a new `PreToolUse`.
    fn end_wait(&mut self, next: SessionState) -> Vec<SessionEvent> {
        self.last_tool = None;
        self.attention = None;
        self.transition(next)
    }

    fn handle_pre_tool(&mut self, payload: &Value) -> Vec<SessionEvent> {
        let tool = payload.get("tool_name").and_then(Value::as_str);
        let input = payload.get("tool_input");
        let (next, kind, message, detail) = match tool {
            Some("AskUserQuestion") => {
                let first = input
                    .and_then(|i| i.get("questions"))
                    .and_then(|q| q.get(0));
                let message = first
                    .and_then(|q| q.get("question"))
                    .or_else(|| input.and_then(|i| i.get("question")))
                    .and_then(Value::as_str)
                    .filter(|m| !m.trim().is_empty())
                    .unwrap_or("Claude asks a question");
                let labels: Vec<&str> = first
                    .and_then(|q| q.get("options"))
                    .and_then(Value::as_array)
                    .map(|o| {
                        o.iter()
                            .filter_map(|o| o.get("label").and_then(Value::as_str))
                            .filter(|l| !l.trim().is_empty())
                            .collect()
                    })
                    .unwrap_or_default();
                let detail = (!labels.is_empty()).then(|| labels.join(" \u{b7} "));
                (
                    SessionState::NeedsInput,
                    "ask_user_question",
                    message,
                    detail,
                )
            }
            Some("ExitPlanMode") => (
                SessionState::NeedsApproval,
                "plan_approval",
                "Claude asks to approve its plan",
                None,
            ),
            _ => {
                self.last_tool = tool_summary(payload);
                self.attention = None;
                return self.transition(SessionState::Working);
            }
        };
        self.last_tool = None;
        let mut events = self.transition(next);
        self.seq += 1;
        let attention = Attention {
            seq: self.seq,
            kind: kind.into(),
            message: one_line(message, MAX_MESSAGE),
            tool: tool.map(|t| one_line(t, MAX_TOOL)),
            detail: detail
                .map(|d| one_line(&d, MAX_DETAIL))
                .filter(|d| !d.is_empty()),
        };
        self.attention = Some(attention.clone());
        events.push(SessionEvent::Attention { attention });
        events
    }

    fn handle_session_start(&mut self, payload: &Value) -> Vec<SessionEvent> {
        let mut events = Vec::new();

        // Journal the new claude session id if it changed.
        if let Some(new_id) = payload.get("session_id").and_then(Value::as_str) {
            if self.claude_session_id.as_deref() != Some(new_id) {
                self.claude_session_id = Some(new_id.to_owned());
                events.push(SessionEvent::ClaudeSession {
                    claude_session_id: new_id.to_owned(),
                });
            }
        }

        // On startup/resume/clear the session is at its prompt — NeedsInput.
        let source = payload.get("source").and_then(Value::as_str).unwrap_or("");
        if matches!(source, "startup" | "resume" | "clear") {
            self.last_tool = None;
            events.extend(self.transition(SessionState::NeedsInput));
            events.push(self.attend(SESSION_START, "", false));
        }

        events
    }

    fn handle_notification(&mut self, payload: &Value) -> Vec<SessionEvent> {
        let ntype = payload
            .get("notification_type")
            .and_then(Value::as_str)
            .unwrap_or("");
        let (next, approval) = if APPROVAL.contains(&ntype) {
            (SessionState::NeedsApproval, true)
        } else if INPUT.contains(&ntype) {
            (SessionState::NeedsInput, false)
        } else {
            // All other notification types change nothing.
            return vec![];
        };
        let message = payload.get("message").and_then(Value::as_str).unwrap_or("");
        let mut events = self.transition(next);
        events.push(self.attend(ntype, message, approval));
        events
    }

    /// Record and return a new attention. The tool of this turn is named for
    /// the approval kinds only.
    fn attend(&mut self, kind: &str, message: &str, with_tool: bool) -> SessionEvent {
        self.seq += 1;
        let (tool, detail) = match (&self.last_tool, with_tool) {
            (Some((tool, detail)), true) => (
                Some(tool.clone()),
                Some(detail.clone()).filter(|d| !d.is_empty()),
            ),
            _ => (None, None),
        };
        let attention = Attention {
            seq: self.seq,
            kind: one_line(kind, MAX_KIND),
            message: one_line(message, MAX_MESSAGE),
            tool,
            detail,
        };
        self.attention = Some(attention.clone());
        SessionEvent::Attention { attention }
    }

    /// Transition to `next`, returning a `State` event only if the state
    /// actually changed.
    fn transition(&mut self, next: SessionState) -> Vec<SessionEvent> {
        if self.state == next {
            return vec![];
        }
        self.state = next;
        vec![SessionEvent::State { state: next }]
    }
}

impl Default for Tracker {
    fn default() -> Self {
        Self::new()
    }
}

/// A `PreToolUse` payload's tool name and one-line summary: the command
/// (Bash), the file (Edit, Write, Read), the URL (WebFetch), the pattern
/// (Grep, Glob), else the input as compact JSON.
fn tool_summary(payload: &Value) -> Option<(String, String)> {
    let name = payload.get("tool_name")?.as_str()?;
    let input = payload.get("tool_input").filter(|i| !i.is_null());
    let field = |key: &str| input.and_then(|i| i.get(key)).and_then(Value::as_str);
    let picked = match name {
        "Bash" => field("command"),
        "Edit" | "Write" | "Read" => field("file_path"),
        "WebFetch" => field("url"),
        "Grep" | "Glob" => field("pattern"),
        _ => None,
    };
    let summary = match picked {
        Some(text) => one_line(text, MAX_DETAIL),
        None => input
            .map(|i| one_line(&i.to_string(), MAX_DETAIL))
            .unwrap_or_default(),
    };
    Some((one_line(name, MAX_TOOL), summary))
}

/// `text` sanitized, on one line (newlines and tabs become spaces), cut to
/// at most `max` bytes on a char boundary.
fn one_line(text: &str, max: usize) -> String {
    // Bound the work on a huge field before sanitizing it.
    let text = &text[..char_floor(text, max.saturating_mul(4))];
    let flat: String = text
        .chars()
        .map(|c| {
            if matches!(c, '\n' | '\r' | '\t') {
                ' '
            } else {
                c
            }
        })
        .collect();
    let mut out = sanitize(&flat);
    out.truncate(char_floor(&out, max));
    out
}

/// The largest char boundary of `s` at or below `max`.
fn char_floor(s: &str, max: usize) -> usize {
    if s.len() <= max {
        return s.len();
    }
    let mut cut = max;
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    cut
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn state(t: &Tracker) -> SessionState {
        t.state
    }

    // ── permission → working → stop flow ─────────────────────────────────────

    // ── /clear changes the session id ────────────────────────────────────────

    #[test]
    fn clear_changes_session_id() {
        let mut t = Tracker::new();

        // First session start (startup)
        let evs = t.apply(
            "SessionStart",
            &json!({"session_id": "aaaa", "source": "startup"}),
        );
        assert!(evs.iter().any(|e| matches!(
            e,
            SessionEvent::ClaudeSession { claude_session_id } if claude_session_id == "aaaa"
        )));
        assert!(evs.iter().any(|e| matches!(
            e,
            SessionEvent::State {
                state: SessionState::NeedsInput
            }
        )));
        assert_eq!(t.claude_session_id.as_deref(), Some("aaaa"));

        // /clear → new session id (clear source)
        let evs = t.apply(
            "SessionStart",
            &json!({"session_id": "bbbb", "source": "clear"}),
        );
        assert!(evs.iter().any(|e| matches!(
            e,
            SessionEvent::ClaudeSession { claude_session_id } if claude_session_id == "bbbb"
        )));
        assert_eq!(t.claude_session_id.as_deref(), Some("bbbb"));

        // Duplicate SessionStart with same id must not emit ClaudeSession again
        let evs = t.apply(
            "SessionStart",
            &json!({"session_id": "bbbb", "source": "resume"}),
        );
        assert!(!evs
            .iter()
            .any(|e| matches!(e, SessionEvent::ClaudeSession { .. })));
    }

    // ── blocked Stop followed by more work ───────────────────────────────────

    #[test]
    fn stop_followed_by_more_work() {
        let mut t = Tracker::new();

        // Working
        t.apply("UserPromptSubmit", &json!({"prompt": "go"}));
        assert_eq!(state(&t), SessionState::Working);

        // Stop (provisional Idle — stop_hook_active may run more)
        let evs = t.apply("Stop", &json!({"stop_hook_active": true}));
        assert_eq!(
            evs,
            vec![SessionEvent::State {
                state: SessionState::Idle
            }]
        );
        assert_eq!(state(&t), SessionState::Idle);

        // A later PreToolUse means the stop hook continued the turn → Working
        let evs = t.apply("PreToolUse", &json!({"tool_name": "Bash"}));
        assert_eq!(
            evs,
            vec![SessionEvent::State {
                state: SessionState::Working
            }]
        );
        assert_eq!(state(&t), SessionState::Working);
    }

    // ── unknown events and notification types ────────────────────────────────

    #[test]
    fn unknown_event_is_ignored() {
        let mut t = Tracker::new();
        let evs = t.apply("UnknownHookEvent", &json!({}));
        assert!(evs.is_empty());
        assert_eq!(state(&t), SessionState::Starting);
    }

    #[test]
    fn unknown_notification_type_is_ignored() {
        let mut t = Tracker::new();
        t.apply("UserPromptSubmit", &json!({}));
        let evs = t.apply(
            "Notification",
            &json!({"notification_type": "push_notification", "message": "foo"}),
        );
        assert!(evs.is_empty());
        assert_eq!(state(&t), SessionState::Working);
    }

    #[test]
    fn session_end_does_not_change_state() {
        let mut t = Tracker::new();
        t.apply("Stop", &json!({}));
        let evs = t.apply("SessionEnd", &json!({}));
        assert!(evs.is_empty());
        assert_eq!(state(&t), SessionState::Idle);
    }

    #[test]
    fn redundant_transition_emits_no_event() {
        let mut t = Tracker::new();
        t.apply("UserPromptSubmit", &json!({}));
        // Second UserPromptSubmit while already Working
        let evs = t.apply("UserPromptSubmit", &json!({}));
        assert!(evs.is_empty());
    }

    fn attention(evs: &[SessionEvent]) -> Option<&Attention> {
        evs.iter().find_map(|e| match e {
            SessionEvent::Attention { attention } => Some(attention),
            _ => None,
        })
    }

    fn state_event(state: SessionState) -> SessionEvent {
        SessionEvent::State { state }
    }

    #[test]
    fn permission_working_stop_flow() {
        let mut t = Tracker::new();
        assert_eq!(state(&t), SessionState::Starting);
        let evs = t.apply("UserPromptSubmit", &json!({"prompt": "do something"}));
        assert_eq!(evs, vec![state_event(SessionState::Working)]);
        let evs = t.apply(
            "Notification",
            &json!({"notification_type": "permission_prompt", "message": "Allow bash?"}),
        );
        assert_eq!(evs.len(), 2);
        assert_eq!(evs[0], state_event(SessionState::NeedsApproval));
        let a = attention(&evs).expect("an attention");
        assert_eq!(
            (a.seq, a.kind.as_str(), a.message.as_str()),
            (1, "permission_prompt", "Allow bash?")
        );
        let evs = t.apply("PreToolUse", &json!({"tool_name": "Bash"}));
        assert_eq!(evs, vec![state_event(SessionState::Working)]);
        assert_eq!(t.attention, None);
        let evs = t.apply("Stop", &json!({"stop_hook_active": false}));
        assert_eq!(evs, vec![state_event(SessionState::Idle)]);
        assert_eq!(state(&t), SessionState::Idle);
    }

    #[test]
    fn worker_permission_prompt_needs_approval() {
        let mut t = Tracker::new();
        t.apply("UserPromptSubmit", &json!({}));
        let evs = t.apply(
            "Notification",
            &json!({"notification_type": "worker_permission_prompt"}),
        );
        assert_eq!(evs[0], state_event(SessionState::NeedsApproval));
        assert_eq!(attention(&evs).unwrap().kind, "worker_permission_prompt");
    }

    #[test]
    fn elicitation_url_dialog_needs_approval() {
        let mut t = Tracker::new();
        let evs = t.apply(
            "Notification",
            &json!({"notification_type": "elicitation_url_dialog"}),
        );
        assert_eq!(evs[0], state_event(SessionState::NeedsApproval));
        let a = attention(&evs).unwrap();
        assert_eq!(
            (a.kind.as_str(), a.tool.as_deref()),
            ("elicitation_url_dialog", None)
        );
    }

    /// AC5: the tool call before the prompt names what is asked.
    #[test]
    fn permission_after_pretooluse_names_the_tool_and_its_command() {
        let mut t = Tracker::new();
        t.apply("UserPromptSubmit", &json!({"prompt": "test it"}));
        t.apply(
            "PreToolUse",
            &json!({"tool_name": "Bash", "tool_input": {"command": "cargo test --all", "description": "run"}}),
        );
        let evs = t.apply(
            "Notification",
            &json!({"notification_type": "permission_prompt", "message": "Claude needs your permission to use Bash"}),
        );
        let expected = Attention {
            seq: 1,
            kind: "permission_prompt".into(),
            message: "Claude needs your permission to use Bash".into(),
            tool: Some("Bash".into()),
            detail: Some("cargo test --all".into()),
        };
        assert_eq!(
            evs,
            vec![
                state_event(SessionState::NeedsApproval),
                SessionEvent::Attention {
                    attention: expected.clone()
                },
            ]
        );
        assert_eq!(t.attention, Some(expected));
    }

    /// AC5: an attention without a state change is still sent.
    #[test]
    fn idle_prompt_after_session_start_emits_an_attention_without_a_state() {
        let mut t = Tracker::new();
        let evs = t.apply(
            "SessionStart",
            &json!({"session_id": "c1", "source": "startup"}),
        );
        assert_eq!(evs.len(), 3, "{evs:?}");
        assert!(evs.contains(&state_event(SessionState::NeedsInput)));
        let start = attention(&evs).unwrap();
        assert_eq!(
            (start.seq, start.kind.as_str(), start.tool.as_deref()),
            (1, SESSION_START, None)
        );
        let evs = t.apply(
            "Notification",
            &json!({"notification_type": "idle_prompt", "message": "Claude is waiting for your input"}),
        );
        assert_eq!(
            evs,
            vec![SessionEvent::Attention {
                attention: Attention {
                    seq: 2,
                    kind: "idle_prompt".into(),
                    message: "Claude is waiting for your input".into(),
                    tool: None,
                    detail: None,
                }
            }]
        );
    }

    /// AC5: two prompts in a row, two attentions with a rising seq.
    #[test]
    fn permission_then_elicitation_emits_two_attentions_with_rising_seq() {
        let mut t = Tracker::new();
        t.apply(
            "PreToolUse",
            &json!({"tool_name": "Bash", "tool_input": {"command": "ls"}}),
        );
        let first = t.apply(
            "Notification",
            &json!({"notification_type": "permission_prompt", "message": "a"}),
        );
        let second = t.apply(
            "Notification",
            &json!({"notification_type": "elicitation_dialog", "message": "pick one"}),
        );
        assert_eq!(attention(&first).unwrap().seq, 1);
        assert_eq!(
            second.len(),
            1,
            "no State: the session still needs approval"
        );
        let a = attention(&second).unwrap();
        assert_eq!(
            (a.seq, a.kind.as_str(), a.message.as_str()),
            (2, "elicitation_dialog", "pick one")
        );
        assert_eq!(t.attention.as_ref().map(|a| a.seq), Some(2));
    }

    #[test]
    fn tool_summaries_pick_the_useful_field() {
        let cases = [
            (
                json!({"tool_name": "Edit", "tool_input": {"file_path": "/w/src/a.rs", "old_string": "x"}}),
                "Edit",
                "/w/src/a.rs",
            ),
            (
                json!({"tool_name": "Write", "tool_input": {"file_path": "/w/b.md", "content": "y"}}),
                "Write",
                "/w/b.md",
            ),
            (
                json!({"tool_name": "Read", "tool_input": {"file_path": "/w/c"}}),
                "Read",
                "/w/c",
            ),
            (
                json!({"tool_name": "WebFetch", "tool_input": {"url": "https://example.org", "prompt": "p"}}),
                "WebFetch",
                "https://example.org",
            ),
            (
                json!({"tool_name": "Grep", "tool_input": {"pattern": "fn main"}}),
                "Grep",
                "fn main",
            ),
            (
                json!({"tool_name": "Glob", "tool_input": {"pattern": "**/*.rs"}}),
                "Glob",
                "**/*.rs",
            ),
            (
                json!({"tool_name": "mcp__kb__search", "tool_input": {"q": "x"}}),
                "mcp__kb__search",
                r#"{"q":"x"}"#,
            ),
        ];
        for (pre, tool, detail) in cases {
            let mut t = Tracker::new();
            t.apply("PreToolUse", &pre);
            let evs = t.apply(
                "Notification",
                &json!({"notification_type": "permission_prompt"}),
            );
            let a = attention(&evs).unwrap();
            assert_eq!(
                (a.tool.as_deref(), a.detail.as_deref()),
                (Some(tool), Some(detail)),
                "{pre}"
            );
        }
    }

    #[test]
    fn attention_text_is_sanitized_one_line_and_capped() {
        let mut t = Tracker::new();
        let long = "é".repeat(3000); // 6000 bytes
        t.apply(
            "PreToolUse",
            &json!({"tool_name": "Bash", "tool_input": {"command": format!("echo \x1b[31mred\u{202e}\n{long}")}}),
        );
        let evs = t.apply(
            "Notification",
            &json!({"notification_type": "permission_prompt", "message": format!("line1\nline2\x07{long}")}),
        );
        let a = attention(&evs).unwrap();
        let detail = a.detail.as_deref().unwrap();
        for text in [a.message.as_str(), detail] {
            assert!(
                !text.chars().any(|c| c.is_control() || c == '\u{202e}'),
                "{text:?}"
            );
        }
        assert!(
            a.message.len() <= MAX_MESSAGE && a.message.starts_with("line1 line2é"),
            "{}",
            a.message
        );
        assert!(
            detail.len() <= MAX_DETAIL && detail.starts_with("echo [31mred é"),
            "{detail}"
        );
    }

    #[test]
    fn turn_events_clear_the_attention() {
        for event in ["PreToolUse", "UserPromptSubmit", "Stop", "StopFailure"] {
            let mut t = Tracker::new();
            t.apply(
                "Notification",
                &json!({"notification_type": "permission_prompt"}),
            );
            assert!(t.attention.is_some());
            t.apply(event, &json!({"tool_name": "Bash"}));
            assert_eq!(t.attention, None, "{event}");
        }
    }

    #[test]
    fn unknown_notifications_leave_the_attention_and_seq_alone() {
        let mut t = Tracker::new();
        t.apply("Notification", &json!({"notification_type": "idle_prompt"}));
        let evs = t.apply(
            "Notification",
            &json!({"notification_type": "push_notification", "message": "foo"}),
        );
        assert!(evs.is_empty());
        assert_eq!(t.attention.as_ref().map(|a| a.seq), Some(1));
        let evs = t.apply("Notification", &json!({"notification_type": "idle_prompt"}));
        assert_eq!(attention(&evs).unwrap().seq, 2);
    }

    #[test]
    fn ask_user_question_needs_input_with_the_options() {
        let mut t = Tracker::new();
        t.apply("UserPromptSubmit", &json!({}));
        let evs = t.apply(
            "PreToolUse",
            &json!({"tool_name": "AskUserQuestion", "tool_input": {"questions": [
                {"question": "Which db?\nreally", "options": [{"label": "Postgres"}, {"label": "SQLite", "description": "x"}]},
                {"question": "second", "options": [{"label": "no"}]}]}}),
        );
        assert_eq!(evs[0], state_event(SessionState::NeedsInput));
        let a = attention(&evs).unwrap();
        assert_eq!(a.kind, "ask_user_question");
        assert_eq!(a.message, "Which db? really");
        assert_eq!(a.tool.as_deref(), Some("AskUserQuestion"));
        assert_eq!(a.detail.as_deref(), Some("Postgres \u{b7} SQLite"));
        assert_eq!(a.seq, 1);
        assert_eq!(t.attention.as_ref(), Some(a));
        // The answer arrives as a PostToolUse: back to work, nothing asked.
        let evs = t.apply("PostToolUse", &json!({"tool_name": "AskUserQuestion"}));
        assert_eq!(evs, vec![state_event(SessionState::Working)]);
        assert_eq!(t.attention, None);
    }

    #[test]
    fn plan_approval_needs_approval() {
        let mut t = Tracker::new();
        t.apply("UserPromptSubmit", &json!({}));
        let evs = t.apply(
            "PreToolUse",
            &json!({"tool_name": "ExitPlanMode", "tool_input": {"plan": "do x"}}),
        );
        assert_eq!(evs[0], state_event(SessionState::NeedsApproval));
        let a = attention(&evs).unwrap();
        assert_eq!(a.kind, "plan_approval");
        assert_eq!(a.message, "Claude asks to approve its plan");
        assert_eq!(
            (a.tool.as_deref(), a.detail.as_deref()),
            (Some("ExitPlanMode"), None)
        );
        let evs = t.apply("PostToolUse", &json!({"tool_name": "ExitPlanMode"}));
        assert_eq!(evs, vec![state_event(SessionState::Working)]);
        assert_eq!(t.attention, None);
    }

    #[test]
    fn ask_user_question_falls_back_on_malformed_input() {
        let cases = [
            (
                json!({"tool_name": "AskUserQuestion"}),
                "Claude asks a question",
                None,
            ),
            (
                json!({"tool_name": "AskUserQuestion", "tool_input": {"questions": []}}),
                "Claude asks a question",
                None,
            ),
            (
                json!({"tool_name": "AskUserQuestion", "tool_input": {"question": "flat?"}}),
                "flat?",
                None,
            ),
            (
                json!({"tool_name": "AskUserQuestion", "tool_input": {"questions": [{"question": 5, "options": [{"label": 1}, "x", {"label": "ok"}]}]}}),
                "Claude asks a question",
                Some("ok"),
            ),
            (
                json!({"tool_name": "AskUserQuestion", "tool_input": {"questions": "bad"}}),
                "Claude asks a question",
                None,
            ),
        ];
        for (pre, message, detail) in cases {
            let mut t = Tracker::new();
            let evs = t.apply("PreToolUse", &pre);
            let a = attention(&evs).unwrap_or_else(|| panic!("{pre}"));
            assert_eq!(
                (a.message.as_str(), a.detail.as_deref()),
                (message, detail),
                "{pre}"
            );
        }
    }

    #[test]
    fn ordinary_pre_tool_use_is_unchanged() {
        let mut t = Tracker::new();
        let evs = t.apply(
            "PreToolUse",
            &json!({"tool_name": "Bash", "tool_input": {"command": "ls"}}),
        );
        assert_eq!(evs, vec![state_event(SessionState::Working)]);
        assert_eq!(t.attention, None);
    }
}
