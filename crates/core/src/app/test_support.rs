//! Fixtures shared by the unit tests of the shared layer (and re-exported by
//! the TUI's own).

use uuid::Uuid;

use crate::proto::{SessionInfo, SessionKind, SessionState};

/// An idle claude session in `/srv/app`, as the local daemon lists it:
/// running as `pid` (`None`: dormant), in conversation `csid`.
pub fn info(pid: Option<u32>, csid: Option<&str>) -> SessionInfo {
    SessionInfo {
        id: Uuid::new_v4(),
        cwd: "/srv/app".into(),
        name: None,
        state: SessionState::Idle,
        claude_session_id: csid.map(Into::into),
        title: None,
        pid,
        created_at: 1,
        branch: None,
        model: None,
        context_tokens: None,
        kind: SessionKind::Claude,
        ephemeral: false,
        attention: None,
    }
}

/// A terminal tab in `/srv/app`.
pub fn shell_info(pid: Option<u32>) -> SessionInfo {
    SessionInfo {
        kind: SessionKind::Shell,
        ..info(pid, None)
    }
}
