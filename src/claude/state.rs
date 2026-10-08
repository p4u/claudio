//! Pure state machine that turns Claude Code hook events into
//! [`proto::SessionEvent`]s.
//!
//! The tracker holds the current [`SessionState`] and the active claude
//! conversation id. Every call to [`Tracker::apply`] returns only the events
//! that represent real changes; callers broadcast those to subscribers.

use serde_json::Value;

use crate::proto::{SessionEvent, SessionState};

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
}

impl Tracker {
    /// Construct a tracker in the `Starting` state (no hook has fired yet).
    pub fn new() -> Self {
        Self { state: SessionState::Starting, claude_session_id: None }
    }

    /// Apply one hook event and return the list of protocol events to broadcast.
    ///
    /// Only real state or id changes produce events; redundant transitions are
    /// suppressed. Unknown events and unknown notification types are silently
    /// ignored.
    pub fn apply(&mut self, event: &str, payload: &Value) -> Vec<SessionEvent> {
        match event {
            "SessionStart" => self.handle_session_start(payload),
            "UserPromptSubmit" | "PreToolUse" => self.transition(SessionState::Working),
            "Notification" => self.handle_notification(payload),
            "Stop" => self.transition(SessionState::Idle),
            "StopFailure" => self.transition(SessionState::Error),
            "SessionEnd" => vec![], // process exit is reported separately
            _ => vec![],
        }
    }

    // ── private helpers ───────────────────────────────────────────────────────

    fn handle_session_start(&mut self, payload: &Value) -> Vec<SessionEvent> {
        let mut events = Vec::new();

        // Journal the new claude session id if it changed.
        if let Some(new_id) = payload.get("session_id").and_then(|v| v.as_str()) {
            let changed = self.claude_session_id.as_deref() != Some(new_id);
            if changed {
                self.claude_session_id = Some(new_id.to_owned());
                events.push(SessionEvent::ClaudeSession {
                    claude_session_id: new_id.to_owned(),
                });
            }
        }

        // On startup/resume/clear the session is at its prompt — NeedsInput.
        let source = payload.get("source").and_then(|v| v.as_str()).unwrap_or("");
        if matches!(source, "startup" | "resume" | "clear") {
            events.extend(self.transition(SessionState::NeedsInput));
        }

        events
    }

    fn handle_notification(&mut self, payload: &Value) -> Vec<SessionEvent> {
        let ntype = payload.get("notification_type").and_then(|v| v.as_str()).unwrap_or("");
        match ntype {
            "permission_prompt" | "worker_permission_prompt" | "elicitation_dialog"
            | "elicitation_url_dialog" => self.transition(SessionState::NeedsApproval),
            "idle_prompt" | "agent_needs_input" => self.transition(SessionState::NeedsInput),
            // All other notification types produce no state change.
            _ => vec![],
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn state(t: &Tracker) -> SessionState {
        t.state
    }

    // ── permission → working → stop flow ─────────────────────────────────────

    #[test]
    fn permission_working_stop_flow() {
        let mut t = Tracker::new();
        assert_eq!(state(&t), SessionState::Starting);

        // User submits a prompt → Working
        let evs = t.apply("UserPromptSubmit", &json!({"prompt": "do something"}));
        assert_eq!(evs, vec![SessionEvent::State { state: SessionState::Working }]);
        assert_eq!(state(&t), SessionState::Working);

        // A permission prompt fires → NeedsApproval
        let evs = t.apply(
            "Notification",
            &json!({"notification_type": "permission_prompt", "message": "Allow bash?"}),
        );
        assert_eq!(evs, vec![SessionEvent::State { state: SessionState::NeedsApproval }]);
        assert_eq!(state(&t), SessionState::NeedsApproval);

        // More work resumes → Working
        let evs = t.apply("PreToolUse", &json!({"tool_name": "Bash"}));
        assert_eq!(evs, vec![SessionEvent::State { state: SessionState::Working }]);

        // Stop → Idle
        let evs = t.apply("Stop", &json!({"stop_hook_active": false}));
        assert_eq!(evs, vec![SessionEvent::State { state: SessionState::Idle }]);
        assert_eq!(state(&t), SessionState::Idle);
    }

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
        assert!(evs.iter().any(|e| matches!(e, SessionEvent::State { state: SessionState::NeedsInput })));
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
        assert!(!evs.iter().any(|e| matches!(e, SessionEvent::ClaudeSession { .. })));
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
        assert_eq!(evs, vec![SessionEvent::State { state: SessionState::Idle }]);
        assert_eq!(state(&t), SessionState::Idle);

        // A later PreToolUse means the stop hook continued the turn → Working
        let evs = t.apply("PreToolUse", &json!({"tool_name": "Bash"}));
        assert_eq!(evs, vec![SessionEvent::State { state: SessionState::Working }]);
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

    #[test]
    fn worker_permission_prompt_needs_approval() {
        let mut t = Tracker::new();
        t.apply("UserPromptSubmit", &json!({}));
        let evs = t.apply(
            "Notification",
            &json!({"notification_type": "worker_permission_prompt"}),
        );
        assert_eq!(evs, vec![SessionEvent::State { state: SessionState::NeedsApproval }]);
    }

    #[test]
    fn elicitation_url_dialog_needs_approval() {
        let mut t = Tracker::new();
        let evs = t.apply(
            "Notification",
            &json!({"notification_type": "elicitation_url_dialog"}),
        );
        assert_eq!(evs, vec![SessionEvent::State { state: SessionState::NeedsApproval }]);
    }
}
