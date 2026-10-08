//! The TUI client's persistent state (`~/.config/claudio/state.json`) and the
//! recovery merge against the daemon's view of the world.
//!
//! state.json remembers what only the client knows: tab order, user-given
//! names, the last active tab and recently used directories. The daemon
//! journal is authoritative for which sessions exist; [`merge`] reconciles
//! the two on every (re)connect.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::paths;
use crate::proto::{SessionId, SessionInfo, SessionState};

/// Most recently used directories kept for the new-session wizard.
pub const MAX_RECENT_DIRS: usize = 20;

/// Everything persisted in state.json.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ClientState {
    #[serde(default)]
    pub sessions: Vec<SavedSession>,
    #[serde(default)]
    pub active: Option<SessionId>,
    /// Most recently used first.
    #[serde(default)]
    pub recent_dirs: Vec<String>,
}

/// One session as the client remembers it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedSession {
    pub id: SessionId,
    #[serde(default)]
    pub name: Option<String>,
    pub cwd: String,
    #[serde(default = "local")]
    pub host: String,
    #[serde(default)]
    pub claude_session_id: Option<String>,
    #[serde(default)]
    pub created_at: u64,
}

fn local() -> String {
    "local".into()
}

impl ClientState {
    /// Read state.json. A missing or unreadable file yields the empty state:
    /// the daemon journal is authoritative, so nothing important is lost.
    pub fn load(path: &Path) -> ClientState {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Atomically write state.json.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let json = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        paths::write_atomic(path, &json)
    }
}

/// Move `dir` to the front of a most-recently-used list, capped at
/// [`MAX_RECENT_DIRS`].
pub fn push_recent(recent: &mut Vec<String>, dir: &str) {
    recent.retain(|d| d != dir);
    recent.insert(0, dir.to_owned());
    recent.truncate(MAX_RECENT_DIRS);
}

/// A session after reconciling state.json with the daemon.
#[derive(Debug, Clone, PartialEq)]
pub struct Recovered {
    /// The record to keep (and persist).
    pub saved: SavedSession,
    pub state: SessionState,
    pub title: Option<String>,
    /// `Some(args)` when the daemon knows the session but has no process for
    /// it: re-spawn it with these claude arguments.
    pub respawn: Option<Vec<String>>,
}

/// Reconcile the saved client state with the daemon's `ListSessions` reply.
///
/// - Order and names come from state.json; daemon sessions it doesn't know
///   are appended in the daemon's order.
/// - state.json entries the daemon doesn't know are dropped: the daemon
///   journal is authoritative.
/// - Dormant sessions (`pid == None`) are marked for re-spawn, resuming the
///   newest known claude conversation (the daemon's id beats state.json's).
pub fn merge(saved: &ClientState, live: &[SessionInfo]) -> Vec<Recovered> {
    let known = saved.sessions.iter().filter_map(|s| {
        let info = live.iter().find(|l| l.id == s.id)?;
        Some(recover(Some(s), info))
    });
    let unknown = live
        .iter()
        .filter(|l| !saved.sessions.iter().any(|s| s.id == l.id))
        .map(|info| recover(None, info));
    known.chain(unknown).collect()
}

fn recover(saved: Option<&SavedSession>, info: &SessionInfo) -> Recovered {
    let claude_session_id = info
        .claude_session_id
        .clone()
        .or_else(|| saved.and_then(|s| s.claude_session_id.clone()));
    let respawn = info.pid.is_none().then(|| match &claude_session_id {
        Some(csid) => vec!["--resume".to_owned(), csid.clone()],
        None => Vec::new(),
    });
    Recovered {
        saved: SavedSession {
            id: info.id,
            // A saved entry's name wins even when cleared (None): the daemon
            // may still carry the name from an older spawn.
            name: match saved {
                Some(s) => s.name.clone(),
                None => info.name.clone(),
            },
            cwd: info.cwd.clone(),
            host: saved.map_or_else(local, |s| s.host.clone()),
            claude_session_id,
            created_at: info.created_at,
        },
        state: info.state,
        title: info.title.clone(),
        respawn,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn saved(id: Uuid, name: Option<&str>, csid: Option<&str>) -> SavedSession {
        SavedSession {
            id,
            name: name.map(Into::into),
            cwd: "/old".into(),
            host: "local".into(),
            claude_session_id: csid.map(Into::into),
            created_at: 1,
        }
    }

    fn info(id: Uuid, pid: Option<u32>, csid: Option<&str>) -> SessionInfo {
        SessionInfo {
            id,
            cwd: "/srv".into(),
            name: Some("daemon-name".into()),
            state: SessionState::Idle,
            claude_session_id: csid.map(Into::into),
            title: Some("title".into()),
            pid,
            created_at: 100,
        }
    }

    #[test]
    fn json_round_trip() {
        let a = Uuid::new_v4();
        let state = ClientState {
            sessions: vec![saved(a, Some("api"), Some("c1"))],
            active: Some(a),
            recent_dirs: vec!["/srv".into(), "/tmp".into()],
        };
        let dir = std::env::temp_dir().join(format!("claudio-state-test-{}", Uuid::new_v4()));
        let path = dir.join("state.json");
        state.save(&path).unwrap();
        assert_eq!(ClientState::load(&path), state);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn load_tolerates_missing_and_partial_files() {
        let missing = std::env::temp_dir().join(format!("claudio-none-{}.json", Uuid::new_v4()));
        assert_eq!(ClientState::load(&missing), ClientState::default());
        let partial: ClientState = serde_json::from_str(
            r#"{"sessions":[{"id":"00000000-0000-0000-0000-000000000000","cwd":"/w"}]}"#,
        )
        .unwrap();
        assert_eq!(partial.sessions[0].host, "local");
        assert!(partial.recent_dirs.is_empty());
    }

    #[test]
    fn recent_dirs_move_to_front_and_cap() {
        let mut recent = Vec::new();
        for i in 0..25 {
            push_recent(&mut recent, &format!("/d{i}"));
        }
        assert_eq!(recent.len(), MAX_RECENT_DIRS);
        assert_eq!(recent[0], "/d24");
        push_recent(&mut recent, "/d20");
        assert_eq!(recent[0], "/d20");
        assert_eq!(recent.iter().filter(|d| *d == "/d20").count(), 1);
    }

    #[test]
    fn merge_keeps_saved_order_and_names_appends_unknown_drops_stale() {
        let (a, b, c, stale) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let state = ClientState {
            sessions: vec![saved(b, Some("bee"), None), saved(stale, None, None), saved(a, None, None)],
            active: Some(a),
            recent_dirs: vec![],
        };
        let live = vec![info(a, Some(1), None), info(c, Some(2), None), info(b, Some(3), None)];
        let merged = merge(&state, &live);
        let ids: Vec<Uuid> = merged.iter().map(|r| r.saved.id).collect();
        assert_eq!(ids, vec![b, a, c]);
        assert_eq!(merged[0].saved.name.as_deref(), Some("bee"));
        // A cleared name in state.json stays cleared.
        assert_eq!(merged[1].saved.name, None);
        // Unknown to state.json: the daemon's name is all we have.
        assert_eq!(merged[2].saved.name.as_deref(), Some("daemon-name"));
        // The daemon's cwd and creation time are authoritative.
        assert_eq!(merged[0].saved.cwd, "/srv");
        assert!(merged.iter().all(|r| r.respawn.is_none()));
    }

    #[test]
    fn dormant_sessions_respawn_with_newest_resume_id() {
        let (a, b, c) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let state = ClientState {
            sessions: vec![
                saved(a, None, Some("old-a")),
                saved(b, None, Some("only-saved")),
                saved(c, None, None),
            ],
            ..Default::default()
        };
        let live = vec![info(a, None, Some("new-a")), info(b, None, None), info(c, None, None)];
        let merged = merge(&state, &live);
        assert_eq!(merged[0].respawn, Some(vec!["--resume".into(), "new-a".into()]));
        assert_eq!(merged[0].saved.claude_session_id.as_deref(), Some("new-a"));
        assert_eq!(merged[1].respawn, Some(vec!["--resume".into(), "only-saved".into()]));
        assert_eq!(merged[2].respawn, Some(vec![]));
    }
}
