//! Desktop notification debouncing (S1 split from app.rs).

use std::collections::HashMap;

use crate::proto::{SessionId, SessionState};

use super::sessions::SessionView;

/// A transient status-bar message.
pub struct Notice {
    pub text: String,
    pub ticks_left: u32,
}

/// Check for background sessions that entered a notification-worthy state
/// since the last check, and populate `pending_notifs` with their labels.
///
/// `active_idx` is the current active session index (never notified for it).
/// `notified` is the debounce map (mutated in place).
pub fn check_notifications(
    sessions: &[SessionView],
    active_idx: Option<usize>,
    notified: &mut HashMap<SessionId, SessionState>,
    pending_notifs: &mut Vec<String>,
) {
    for (i, v) in sessions.iter().enumerate() {
        if Some(i) == active_idx {
            // Never notify for the session the user is currently watching.
            continue;
        }
        if !v.state.wants_attention() {
            // Clear debounce state so a later attention state triggers again.
            notified.remove(&v.id);
            continue;
        }
        // Only notify once per (session, state) combination.
        if notified.get(&v.id) == Some(&v.state) {
            continue;
        }
        notified.insert(v.id, v.state);
        pending_notifs.push(v.label());
    }
}
