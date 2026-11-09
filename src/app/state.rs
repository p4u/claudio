//! The TUI client's persistent state (`~/.config/claudio/state.json`) and the
//! recovery merge against the daemon's view of the world.
//!
//! state.json remembers what only the client knows: tab order, user-given
//! names, the last active tab and recently used directories. The daemon
//! journal is authoritative for which sessions exist; [`merge`] and
//! [`merge_for_host`] reconcile the two on every (re)connect.

use std::collections::HashMap;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::addon::{Addons, LaunchChoice, LegacyChoice};
use crate::paths;
use crate::proto::{SessionId, SessionInfo, SessionKind, SessionState};

/// Most recently used directories kept for the new-session wizard.
pub const MAX_RECENT_DIRS: usize = 20;

/// A session id and host that the user explicitly closed.
/// Persisted before the Kill is sent so recovery never resurrects it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KillTombstone {
    pub host: String,
    pub id: SessionId,
}

/// Everything persisted in state.json.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ClientState {
    #[serde(default)]
    pub sessions: Vec<SavedSession>,
    #[serde(default)]
    pub active: Option<SessionId>,
    /// Most recently used directories, keyed by host ("local" for the local
    /// machine). Older state.json files stored a flat `Vec<String>`; those are
    /// transparently promoted to `{"local": [...]}` on load via a custom
    /// deserialiser so no data is ever lost.
    #[serde(default, deserialize_with = "deser_recent_dirs")]
    pub recent_dirs: HashMap<String, Vec<String>>,
    /// Sessions the user explicitly closed; Kill is retried until acknowledged.
    #[serde(default)]
    pub killed: Vec<KillTombstone>,
    /// Per host, the claude version the user chose to skip updating to
    /// ("skip this version" in the update prompt).
    #[serde(default)]
    pub claude_skipped: HashMap<String, String>,
}

/// Deserialise `recent_dirs` from either the new `{host: [dirs]}` map or the
/// legacy flat `[dirs]` list. The legacy list is promoted to `{"local": [dirs]}`.
fn deser_recent_dirs<'de, D>(d: D) -> Result<HashMap<String, Vec<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{MapAccess, SeqAccess, Visitor};

    struct Vis;

    impl<'de> Visitor<'de> for Vis {
        type Value = HashMap<String, Vec<String>>;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "a map of host→dirs or a legacy list of dirs")
        }

        fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
            let mut out = HashMap::new();
            while let Some((k, v)) = map.next_entry::<String, Vec<String>>()? {
                out.insert(k, v);
            }
            Ok(out)
        }

        fn visit_seq<S: SeqAccess<'de>>(self, mut seq: S) -> Result<Self::Value, S::Error> {
            let mut dirs = Vec::new();
            while let Some(d) = seq.next_element::<String>()? {
                dirs.push(d);
            }
            let mut out = HashMap::new();
            if !dirs.is_empty() {
                out.insert("local".to_owned(), dirs);
            }
            Ok(out)
        }
    }

    d.deserialize_any(Vis)
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
    /// What it was launched with (`None` with no `proxy`: direct). Read
    /// first; a state.json without it is read from `proxy`/`first_party`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch: Option<LaunchChoice>,
    /// compat(v0.10): the pre-addon routing, still written (derived from
    /// `launch` through `Launch::legacy`) so a rolled-back build keeps the
    /// session's routing: the proxy profile's name, never the token.
    #[serde(default)]
    pub proxy: Option<String>,
    /// compat(v0.10): launched first-party (`--first-party`). Omitted when
    /// false, so a state.json without it reads as gateway, as before.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub first_party: bool,
    /// Known only from its daemon (another front-end started it): its
    /// routing is unknown here, so a restart or recovery refuses rather
    /// than start it direct (R19). Omitted when false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub route_unknown: bool,
    /// A terminal tab never resumes and never gets proxy env.
    #[serde(default)]
    pub kind: SessionKind,
}

fn local() -> String {
    "local".into()
}

/// Why a persisted launch is refused (AC8: never direct instead).
pub const NOT_AVAILABLE: &str = "this session's launch option is not available in this build";

/// A session's route as state.json and web-state.json persist it: `launch`
/// and, for one release, the pre-addon fields beside it (compat(v0.10)).
/// Kept whole when this build cannot use it, so a save writes it back
/// unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SavedRoute {
    /// The launch choice, when the file has one.
    pub launch: Option<LaunchChoice>,
    /// `proxy` and `first_party` as the file has them.
    pub legacy: LegacyChoice,
}

impl SavedRoute {
    /// The route written for `choice` (`None`: direct): the choice and the
    /// pre-addon fields its addon derives (compat(v0.10)).
    pub fn of(addons: &Addons, choice: Option<&LaunchChoice>) -> SavedRoute {
        SavedRoute {
            launch: choice.cloned(),
            legacy: crate::app::launch::legacy(addons, choice),
        }
    }

    /// What the session is launched with: `launch` wins when this build
    /// has its addon; without `launch`, the pre-addon fields as an addon
    /// adopts them (no `proxy`: direct). `Err`: a choice this build cannot
    /// make, which fails the launch and is never direct (F4, AC8).
    pub fn resolve(&self, addons: &Addons) -> Result<Option<LaunchChoice>, String> {
        match &self.launch {
            Some(c) if addons.get(&c.addon).and_then(|a| a.launch()).is_some() => {
                Ok(Some(c.clone()))
            }
            Some(_) => Err(NOT_AVAILABLE.to_owned()),
            None => crate::app::launch::adopt_legacy(addons, &self.legacy)
                .map_err(|_| NOT_AVAILABLE.to_owned()),
        }
    }
}

impl SavedSession {
    /// Its persisted route.
    pub fn route(&self) -> SavedRoute {
        SavedRoute {
            launch: self.launch.clone(),
            legacy: LegacyChoice {
                proxy: self.proxy.clone(),
                first_party: self.first_party,
            },
        }
    }

    /// Its launch as this build makes it ([`SavedRoute::resolve`]); `Err`
    /// for one started elsewhere (R19) or a choice this build cannot make:
    /// never direct (AC8).
    pub fn launch_of(&self, addons: &Addons) -> Result<Option<LaunchChoice>, String> {
        if self.route_unknown {
            return Err("started elsewhere".to_owned());
        }
        self.route().resolve(addons)
    }

    /// Persist `route`.
    pub fn set_route(&mut self, route: SavedRoute) {
        self.launch = route.launch;
        self.proxy = route.legacy.proxy;
        self.first_party = route.legacy.first_party;
    }
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

    /// Whether `id` is in the kill tombstone list.
    pub fn is_killed(&self, id: SessionId) -> bool {
        self.killed.iter().any(|t| t.id == id)
    }
}

/// Move `dir` to the front of `host`'s MRU list in `recent`, capped at
/// [`MAX_RECENT_DIRS`] per host.
pub fn push_recent(recent: &mut HashMap<String, Vec<String>>, host: &str, dir: &str) {
    let dirs = recent.entry(host.to_owned()).or_default();
    dirs.retain(|d| d != dir);
    dirs.insert(0, dir.to_owned());
    dirs.truncate(MAX_RECENT_DIRS);
}

/// Return the MRU directory list for `host` (empty slice when unknown).
pub fn recent_for_host<'a>(recent: &'a HashMap<String, Vec<String>>, host: &str) -> &'a [String] {
    recent.get(host).map(Vec::as_slice).unwrap_or(&[])
}

/// A session after reconciling state.json with the daemon.
#[derive(Debug, Clone, PartialEq)]
pub struct Recovered {
    /// The record to keep (and persist).
    pub saved: SavedSession,
    pub state: SessionState,
    pub title: Option<String>,
    /// `Some(args)` when the daemon knows the session but has no process for
    /// it: re-spawn it with these claude arguments (none for a terminal).
    pub respawn: Option<Vec<String>>,
}

/// Like [`merge_for_host`] but assigns the given `host` to live sessions that aren't
/// in the saved list (instead of always defaulting to "local").
///
/// Used when reconciling a specific remote host's session list.
pub fn merge_for_host(host: &str, saved: &ClientState, live: &[SessionInfo]) -> Vec<Recovered> {
    // Skip tombstoned sessions, and those that are not the manager's
    // (`--plain`'s: never shown, never respawned).
    let live_non_killed: Vec<&SessionInfo> = live
        .iter()
        .filter(|l| !l.ephemeral && !saved.is_killed(l.id))
        .collect();

    let known = saved.sessions.iter().filter_map(|s| {
        // Skip tombstoned.
        if saved.is_killed(s.id) {
            return None;
        }
        let info = live_non_killed.iter().find(|l| l.id == s.id)?;
        Some(recover(Some(s), info))
    });
    let unknown = live_non_killed
        .iter()
        .filter(|l| !saved.sessions.iter().any(|s| s.id == l.id))
        .map(|info| recover_with_host(None, info, host));
    known.chain(unknown).collect()
}

fn recover(saved: Option<&SavedSession>, info: &SessionInfo) -> Recovered {
    let host = saved.map_or_else(local, |s| s.host.clone());
    recover_with_host(saved, info, &host)
}

fn recover_with_host(saved: Option<&SavedSession>, info: &SessionInfo, host: &str) -> Recovered {
    let claude_session_id = info
        .claude_session_id
        .clone()
        .or_else(|| saved.and_then(|s| s.claude_session_id.clone()));
    let respawn = info
        .pid
        .is_none()
        .then(|| match (info.kind, &claude_session_id) {
            (SessionKind::Claude, Some(csid)) => vec!["--resume".to_owned(), csid.clone()],
            _ => Vec::new(),
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
            host: saved
                .map(|s| s.host.clone())
                .unwrap_or_else(|| host.to_owned()),
            claude_session_id,
            created_at: info.created_at,
            launch: saved.and_then(|s| s.launch.clone()),
            proxy: saved.and_then(|s| s.proxy.clone()),
            first_party: saved.is_some_and(|s| s.first_party),
            // A session state.json does not know was started elsewhere.
            route_unknown: info.kind == SessionKind::Claude
                && saved.is_none_or(|s| s.route_unknown),
            // The daemon's journal is authoritative.
            kind: info.kind,
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

    #[test]
    fn first_party_is_saved_only_when_set() {
        let mut s = saved(Uuid::new_v4(), None, None);
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("first_party"), "{json}");

        s.first_party = true;
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"first_party\":true"), "{json}");
        let back: SavedSession = serde_json::from_str(&json).unwrap();
        assert!(back.first_party);

        // A state.json written before the field reads as gateway.
        let old = json.replace(",\"first_party\":true", "");
        let back: SavedSession = serde_json::from_str(&old).unwrap();
        assert!(!back.first_party);
    }

    fn saved(id: Uuid, name: Option<&str>, csid: Option<&str>) -> SavedSession {
        SavedSession {
            id,
            name: name.map(Into::into),
            cwd: "/old".into(),
            host: "local".into(),
            claude_session_id: csid.map(Into::into),
            created_at: 1,
            launch: None,
            proxy: None,
            first_party: false,
            route_unknown: false,
            kind: SessionKind::Claude,
        }
    }

    fn info(id: Uuid, pid: Option<u32>, csid: Option<&str>) -> SessionInfo {
        SessionInfo {
            id,
            cwd: "/srv".into(),
            name: Some("daemon-name".into()),
            title: Some("title".into()),
            created_at: 100,
            ..crate::app::test_support::info(pid, csid)
        }
    }

    fn mk_recent(host: &str, dirs: &[&str]) -> HashMap<String, Vec<String>> {
        let mut m = HashMap::new();
        m.insert(
            host.to_owned(),
            dirs.iter().map(|s| s.to_string()).collect(),
        );
        m
    }

    #[test]
    fn json_round_trip() {
        let a = Uuid::new_v4();
        let state = ClientState {
            sessions: vec![saved(a, Some("api"), Some("c1"))],
            active: Some(a),
            recent_dirs: mk_recent("local", &["/srv", "/tmp"]),
            killed: vec![],
            claude_skipped: HashMap::from([("devbox".to_owned(), "2.1.296".to_owned())]),
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
        assert!(partial.killed.is_empty());
        assert!(partial.claude_skipped.is_empty());
    }

    #[test]
    fn legacy_flat_recent_dirs_migrated_to_local() {
        // Old state.json stores recent_dirs as a flat array.
        let json = r#"{"recent_dirs":["/srv","/tmp"]}"#;
        let state: ClientState = serde_json::from_str(json).unwrap();
        let local_dirs = state
            .recent_dirs
            .get("local")
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        assert_eq!(local_dirs, &["/srv", "/tmp"][..]);
    }

    #[test]
    fn recent_dirs_move_to_front_and_cap() {
        let mut recent = HashMap::new();
        for i in 0..25 {
            push_recent(&mut recent, "local", &format!("/d{i}"));
        }
        let dirs = recent.get("local").unwrap();
        assert_eq!(dirs.len(), MAX_RECENT_DIRS);
        assert_eq!(dirs[0], "/d24");
        push_recent(&mut recent, "local", "/d20");
        let dirs = recent.get("local").unwrap();
        assert_eq!(dirs[0], "/d20");
        assert_eq!(dirs.iter().filter(|d| **d == "/d20").count(), 1);
    }

    #[test]
    fn recent_dirs_per_host() {
        let mut recent = HashMap::new();
        push_recent(&mut recent, "local", "/local/dir");
        push_recent(&mut recent, "myhost", "/remote/dir");
        assert_eq!(
            recent_for_host(&recent, "local"),
            &["/local/dir".to_owned()]
        );
        assert_eq!(
            recent_for_host(&recent, "myhost"),
            &["/remote/dir".to_owned()]
        );
        // A host with no entries returns an empty slice.
        assert!(recent_for_host(&recent, "unknown").is_empty());
    }

    #[test]
    fn merge_keeps_saved_order_and_names_appends_unknown_drops_stale() {
        let (a, b, c, stale) = (
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
        );
        let state = ClientState {
            sessions: vec![
                saved(b, Some("bee"), None),
                saved(stale, None, None),
                saved(a, None, None),
            ],
            active: Some(a),
            recent_dirs: HashMap::new(),
            killed: vec![],
            ..Default::default()
        };
        let live = vec![
            info(a, Some(1), None),
            info(c, Some(2), None),
            info(b, Some(3), None),
        ];
        let merged = merge_for_host("local", &state, &live);
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
        let live = vec![
            info(a, None, Some("new-a")),
            info(b, None, None),
            info(c, None, None),
        ];
        let merged = merge_for_host("local", &state, &live);
        assert_eq!(
            merged[0].respawn,
            Some(vec!["--resume".into(), "new-a".into()])
        );
        assert_eq!(merged[0].saved.claude_session_id.as_deref(), Some("new-a"));
        assert_eq!(
            merged[1].respawn,
            Some(vec!["--resume".into(), "only-saved".into()])
        );
        assert_eq!(merged[2].respawn, Some(vec![]));
    }

    #[test]
    fn dormant_terminals_respawn_without_resume_and_keep_their_kind() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let state = ClientState {
            sessions: vec![saved(a, None, Some("stale"))],
            ..Default::default()
        };
        let mut shell = info(a, None, Some("stale"));
        shell.kind = SessionKind::Shell;
        let mut unknown = info(b, None, None);
        unknown.kind = SessionKind::Shell;
        let merged = merge_for_host("local", &state, &[shell, unknown]);
        // The daemon's kind wins over the (older) saved record, and a
        // terminal never resumes a conversation.
        for r in &merged {
            assert_eq!(r.saved.kind, SessionKind::Shell);
            assert_eq!(r.respawn, Some(vec![]));
        }
    }

    #[test]
    fn saved_sessions_without_kind_are_claude() {
        let old: SavedSession =
            serde_json::from_str(r#"{"id":"00000000-0000-0000-0000-000000000000","cwd":"/w"}"#)
                .unwrap();
        assert_eq!(old.kind, SessionKind::Claude);
    }

    #[test]
    fn tombstoned_sessions_are_suppressed_in_merge() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let state = ClientState {
            sessions: vec![saved(a, None, None), saved(b, None, None)],
            killed: vec![KillTombstone {
                host: "local".into(),
                id: a,
            }],
            ..Default::default()
        };
        let live = vec![info(a, Some(1), None), info(b, Some(2), None)];
        let merged = merge_for_host("local", &state, &live);
        // Session `a` is tombstoned: must not appear.
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].saved.id, b);
    }

    #[test]
    fn merge_for_host_assigns_correct_host_to_unknown_sessions() {
        let id = Uuid::new_v4();
        let state = ClientState::default();
        let live = vec![info(id, Some(1), None)];
        let merged = merge_for_host("myserver.example.com", &state, &live);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].saved.host, "myserver.example.com");
    }
}

/// P3.19: `launch` beside the pre-addon fields, and the previous release's
/// state.json (AC8, B3).
#[cfg(test)]
mod launch_tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use super::*;
    use crate::addon::test_support::TestAddon;
    use crate::addon::Addon;

    /// A build whose addon `team` adopts the pre-addon routing.
    fn adopting() -> Addons {
        let team: Arc<dyn Addon> = Arc::new(TestAddon::new("team").adopts_legacy());
        Addons::new(vec![team]).unwrap()
    }

    fn fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/state-v0.8.0.json")
    }

    fn sid(n: u128) -> SessionId {
        format!("0c8a1f52-6d0e-4b8a-9b51-2f0d5b1c{n:04}")
            .parse()
            .unwrap()
    }

    /// Session `n`'s launch in `state` (`Err` for one it lacks).
    fn launch(
        state: &ClientState,
        addons: &Addons,
        n: u128,
    ) -> Result<Option<LaunchChoice>, String> {
        let saved = state.sessions.iter().find(|s| s.id == sid(n));
        saved
            .ok_or_else(|| "no such session".to_owned())?
            .launch_of(addons)
    }

    fn team(option: &str, flags: &[&str]) -> LaunchChoice {
        LaunchChoice {
            addon: "team".into(),
            option: option.into(),
            flags: flags.iter().map(|f| f.to_string()).collect(),
        }
    }

    #[test]
    fn v080_state_json_loads_with_its_proxy_choices() {
        let state = ClientState::load(&fixture());
        assert_eq!(state.sessions.len(), 4);
        assert_eq!(state.active, Some(sid(2)));
        let addons = adopting();
        assert_eq!(launch(&state, &addons, 1), Ok(None), "direct");
        assert_eq!(launch(&state, &addons, 2), Ok(Some(team("work", &[]))));
        assert_eq!(
            launch(&state, &addons, 3),
            Ok(Some(team("lab", &["--first-party"])))
        );
        assert_eq!(launch(&state, &addons, 4), Ok(None), "a terminal");
        assert!(launch(&state, &addons, 9).is_err(), "unknown");
    }

    #[test]
    fn saving_writes_launch_and_the_legacy_fields() {
        /// state.json's session as v0.8.0 reads it (its struct, kept here).
        #[derive(Deserialize)]
        struct V080Session {
            #[serde(default)]
            proxy: Option<String>,
            #[serde(default)]
            first_party: bool,
        }
        let addons = adopting();
        let mut state = ClientState::load(&fixture());
        let choice = team("lab", &["--first-party"]);
        state.sessions[0].set_route(SavedRoute::of(&addons, Some(&choice)));
        let json = serde_json::to_value(&state).unwrap();
        let entry = &json["sessions"][0];
        assert_eq!(
            entry["launch"],
            serde_json::json!({"addon": "team", "option": "lab", "flags": ["--first-party"]})
        );
        let old: V080Session = serde_json::from_value(entry.clone()).unwrap();
        assert_eq!(old.proxy.as_deref(), Some("lab"));
        assert!(old.first_party);
        // `launch` wins over the legacy fields once both are there.
        let back: ClientState = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(launch(&back, &addons, 1), Ok(Some(choice)));
        // A direct session writes neither.
        assert!(json["sessions"][3].get("launch").is_none());
        assert_eq!(json["sessions"][3]["proxy"], serde_json::Value::Null);
        assert_eq!(SavedRoute::of(&addons, None), SavedRoute::default());
    }

    #[test]
    fn an_unknown_legacy_profile_is_kept_and_fails_at_launch() {
        // The adopting addon takes any profile name; its launch then
        // resolves the option (and fails it), never direct (AC8).
        let addons = adopting();
        let mut state = ClientState::load(&fixture());
        state.sessions[1].proxy = Some("gone".into());
        let choice = launch(&state, &addons, 2).unwrap();
        assert_eq!(choice, Some(team("gone", &[])));
        assert_ne!(
            crate::addon::LaunchSelect::of(choice.as_ref()).pick,
            crate::addon::Pick::Direct
        );
    }

    #[test]
    fn a_build_without_vocdoni_keeps_the_choice_it_cannot_adopt() {
        let none = Addons::empty();
        let state = ClientState::load(&fixture());
        assert_eq!(launch(&state, &none, 1), Ok(None));
        assert_eq!(launch(&state, &none, 2), Err(NOT_AVAILABLE.to_owned()));
        assert_eq!(launch(&state, &none, 3), Err(NOT_AVAILABLE.to_owned()));
        // A choice whose addon this build lacks: refused, never direct.
        let mut later = state.sessions[1].clone();
        later.set_route(SavedRoute::of(&adopting(), Some(&team("work", &[]))));
        assert!(later.route().resolve(&none).is_err());
        // The raw fields survive a load and a save.
        let route = state.sessions[2].route();
        let mut copy = state.sessions[2].clone();
        copy.set_route(route.clone());
        assert_eq!(copy, state.sessions[2]);
        assert_eq!(route.legacy.proxy.as_deref(), Some("lab"));
    }
}
