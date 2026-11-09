//! `web-state.json`: what only the web knows — each session's proxy profile
//! (name only), the board order, the pane layout and the remote hosts it
//! was connected to. Written with `paths::write_atomic` (0600). The TUI's
//! `state.json` is read (for proxies it knows) and never written.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::addon::{LaunchChoice, LegacyChoice};
use crate::app::state::{ClientState, SavedRoute, SavedSession};
use crate::paths;
use crate::proto::SessionId;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct WebState {
    #[serde(default)]
    pub sessions: BTreeMap<SessionId, SessionMeta>,
    #[serde(default)]
    pub order: Vec<SessionId>,
    #[serde(default)]
    pub layout: Layout,
    /// Remote hosts the web was connected to: reconnected at start.
    #[serde(default)]
    pub hosts: Vec<String>,
}

/// What the web recorded of a session it started: never a token.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct SessionMeta {
    pub host: String,
    /// What it was launched with (`None` with no `proxy`: direct); read
    /// first, as state.json's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch: Option<LaunchChoice>,
    /// compat(v0.10): the proxy profile's name, still written for a
    /// rolled-back build.
    #[serde(default)]
    pub proxy: Option<String>,
    /// compat(v0.10): first-party, still written.
    #[serde(default)]
    pub first_party: bool,
}

impl SessionMeta {
    /// The record of a session on `host` launched as `route` says.
    pub(crate) fn new(host: &str, route: SavedRoute) -> SessionMeta {
        SessionMeta {
            host: host.to_owned(),
            launch: route.launch,
            proxy: route.legacy.proxy,
            first_party: route.legacy.first_party,
        }
    }

    /// Its persisted route.
    pub(crate) fn route(&self) -> SavedRoute {
        SavedRoute {
            launch: self.launch.clone(),
            legacy: LegacyChoice {
                proxy: self.proxy.clone(),
                first_party: self.first_party,
            },
        }
    }
}

/// The browser's panes (docs/web-protocol.md §4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Layout {
    pub grid: u8,
    pub panes: Vec<Option<SessionId>>,
    pub focus: u8,
}

impl Default for Layout {
    fn default() -> Self {
        Layout {
            grid: 1,
            panes: vec![None],
            focus: 0,
        }
    }
}

impl Layout {
    /// `grid` is 1, 2 or 4, with exactly that many panes, and `focus` names
    /// one of them.
    pub(crate) fn valid(&self) -> bool {
        matches!(self.grid, 1 | 2 | 4)
            && self.panes.len() == usize::from(self.grid)
            && self.focus < self.grid
    }
}

impl WebState {
    /// Forget the records of `host`'s sessions that `listed` (the daemon's
    /// list) does not name: they ended while no web was watching. Whether
    /// any went.
    pub(crate) fn forget_unlisted(
        &mut self,
        host: &str,
        listed: impl Fn(&SessionId) -> bool,
    ) -> bool {
        let before = self.sessions.len();
        self.sessions.retain(|sid, m| m.host != host || listed(sid));
        self.sessions.len() != before
    }
}

pub(crate) fn path() -> PathBuf {
    paths::config_dir().join("web-state.json")
}

/// Read `path`. A missing, unreadable or invalid file is the default state;
/// an invalid layout is the default layout.
pub(crate) fn load(path: &Path) -> WebState {
    let mut state: WebState = std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    if !state.layout.valid() {
        state.layout = Layout::default();
    }
    state
}

pub(crate) fn save(path: &Path, s: &WebState) -> io::Result<()> {
    let json = serde_json::to_vec_pretty(s).map_err(io::Error::other)?;
    paths::write_atomic(path, &json)
}

/// What is known of `sid`'s launch: the web's own record first, then the
/// TUI's state.json (read only; an entry the TUI marks as started
/// elsewhere knows nothing, R19). `None`: nothing is known.
pub(crate) fn known_launch(
    web: &WebState,
    tui: &ClientState,
    sid: SessionId,
) -> Option<SavedRoute> {
    if let Some(meta) = web.sessions.get(&sid) {
        return Some(meta.route());
    }
    tui.sessions
        .iter()
        .find(|s| s.id == sid && !s.route_unknown)
        .map(SavedSession::route)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::SavedSession;
    use std::os::unix::fs::PermissionsExt;
    use uuid::Uuid;

    fn dir(tag: &str) -> PathBuf {
        let d = crate::paths::socket_test_dir(tag);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn records_of_sessions_a_host_no_longer_lists_are_forgotten() {
        let meta = |host: &str| SessionMeta::new(host, SavedRoute::default());
        let (kept, gone, other) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let mut s = WebState::default();
        s.sessions.insert(kept, meta("local"));
        s.sessions.insert(gone, meta("local"));
        s.sessions.insert(other, meta("z6"));
        assert!(s.forget_unlisted("local", |sid| *sid == kept));
        assert_eq!(
            s.sessions.keys().copied().collect::<Vec<_>>().len(),
            2,
            "{s:?}"
        );
        assert!(s.sessions.contains_key(&kept) && s.sessions.contains_key(&other));
        assert!(
            !s.forget_unlisted("local", |sid| *sid == kept),
            "nothing left to drop"
        );
    }

    #[test]
    fn web_state_round_trips_and_is_private() {
        let d = dir("web-state");
        let p = d.join("web-state.json");
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let mut s = WebState::default();
        s.sessions.insert(
            a,
            SessionMeta {
                host: "local".into(),
                launch: None,
                proxy: Some("work".into()),
                first_party: true,
            },
        );
        s.order = vec![b, a];
        s.layout = Layout {
            grid: 2,
            panes: vec![Some(a), None],
            focus: 1,
        };
        s.hosts = vec!["z6".into()];
        save(&p, &s).unwrap();
        assert_eq!(load(&p), s);
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn invalid_layouts_fall_back_to_the_default() {
        let bad = [
            Layout {
                grid: 3,
                panes: vec![None, None, None],
                focus: 0,
            },
            Layout {
                grid: 2,
                panes: vec![None],
                focus: 0,
            },
            Layout {
                grid: 4,
                panes: vec![None; 4],
                focus: 4,
            },
        ];
        for layout in &bad {
            assert!(!layout.valid(), "{layout:?}");
        }
        assert!(Layout::default().valid());
        let d = dir("web-layout");
        let p = d.join("web-state.json");
        let s = WebState {
            order: vec![Uuid::new_v4()],
            layout: bad[0].clone(),
            ..WebState::default()
        };
        save(&p, &s).unwrap();
        let loaded = load(&p);
        assert_eq!(loaded.layout, Layout::default());
        assert_eq!(loaded.order, s.order, "the rest is kept");
        std::fs::write(&p, b"not json").unwrap();
        assert_eq!(load(&p), WebState::default());
        assert_eq!(load(&d.join("missing.json")), WebState::default());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn known_launch_prefers_web_state_then_tui_state() {
        let (a, b, c) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let mut web = WebState::default();
        web.sessions
            .insert(a, SessionMeta::new("local", SavedRoute::default()));
        let saved = |id, proxy: &str| SavedSession {
            id,
            name: None,
            cwd: "/".into(),
            host: "local".into(),
            claude_session_id: None,
            created_at: 0,
            launch: None,
            proxy: Some(proxy.into()),
            first_party: true,
            route_unknown: false,
            kind: Default::default(),
        };
        let tui = ClientState {
            sessions: vec![saved(a, "tui-a"), saved(b, "tui-b")],
            ..ClientState::default()
        };
        assert_eq!(known_launch(&web, &tui, a), Some(SavedRoute::default()));
        let legacy = LegacyChoice {
            proxy: Some("tui-b".into()),
            first_party: true,
        };
        assert_eq!(known_launch(&web, &tui, b).map(|r| r.legacy), Some(legacy));
        assert_eq!(known_launch(&web, &tui, c), None);
    }

    /// R19: a session the TUI only knows from its daemon (another client
    /// started it) is unknown here too, never direct.
    #[test]
    fn a_tui_entry_started_elsewhere_is_not_known() {
        let sid = Uuid::new_v4();
        let entry = serde_json::json!({
            "id": sid, "cwd": "/", "host": "local", "route_unknown": true,
        });
        let tui: ClientState =
            serde_json::from_value(serde_json::json!({ "sessions": [entry] })).unwrap();
        assert_eq!(known_launch(&WebState::default(), &tui, sid), None);
    }

    #[test]
    fn the_web_never_writes_state_json() {
        let d = dir("web-nostate");
        save(&d.join("web-state.json"), &WebState::default()).unwrap();
        assert!(d.join("web-state.json").exists());
        assert!(!d.join("state.json").exists());
        let names: Vec<String> = std::fs::read_dir(&d)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["web-state.json"]);
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// P3.19: the previous release's web-state.json (AC8, B3).
#[cfg(test)]
mod launch_tests {
    use std::sync::Arc;

    use super::*;
    use crate::addon::test_support::TestAddon;
    use crate::addon::{Addon, Addons};

    fn sid(n: u128) -> SessionId {
        format!("0c8a1f52-6d0e-4b8a-9b51-2f0d5b1c{n:04}")
            .parse()
            .unwrap()
    }

    #[test]
    fn v080_web_state_loads_with_its_proxy() {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/web-state-v0.8.0.json");
        let web = load(&path);
        assert_eq!(web.sessions.len(), 3);
        assert_eq!(web.hosts, ["devbox"]);
        assert_eq!(web.layout.grid, 2);
        let team: Arc<dyn Addon> = Arc::new(TestAddon::new("team").adopts_legacy());
        let addons = Addons::new(vec![team]).unwrap();
        let tui = ClientState::default();
        let launch = |n| known_launch(&web, &tui, sid(n)).map(|r| r.resolve(&addons));
        let choice = |option: &str, flags: &[&str]| LaunchChoice {
            addon: "team".into(),
            option: option.into(),
            flags: flags.iter().map(|f| f.to_string()).collect(),
        };
        assert_eq!(launch(11), Some(Ok(None)), "direct");
        assert_eq!(launch(12), Some(Ok(Some(choice("work", &[])))));
        assert_eq!(
            launch(13),
            Some(Ok(Some(choice("lab", &["--first-party"]))))
        );
        // Without an addon that adopts it: kept, and refused (never direct).
        let none = Addons::empty();
        let route = known_launch(&web, &tui, sid(12)).unwrap();
        assert!(route.resolve(&none).is_err());
        // Written back with `launch` and the fields v0.8.0 reads.
        let meta = SessionMeta::new(
            "local",
            SavedRoute::of(&addons, Some(&choice("lab", &["--first-party"]))),
        );
        let json = serde_json::to_value(&meta).unwrap();
        assert_eq!(json["launch"]["option"], "lab");
        assert_eq!(
            (json["proxy"].as_str(), json["first_party"].as_bool()),
            (Some("lab"), Some(true))
        );
        assert_eq!(SessionMeta::new("local", route.clone()).route(), route);
    }
}
