//! Manager state and update logic.
//!
//! [`App`] holds the session views, the open modal and the bits of
//! persistent state. It never does I/O: inputs arrive through the `on_*`
//! methods and everything it wants done (daemon requests, PTY input, saving
//! state.json) is queued as [`Effect`]s that the event loop in `mod.rs`
//! drains with [`App::take_effects`].
//!
//! Sub-modules hold the individual types (S1 split):
//! - `sessions`      – SessionView, sanitize_label
//! - `proxy_state`   – ProxyStatus, load_proxy_profiles, proxy_env_for
//! - `notifications` – Notice, check_notifications
//! - `interaction`   – Modal

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Instant;

use crate::proto::{Msg, SessionId, SessionState};
use crate::proxy::api::{ConfigResponse, PoolHealthResponse, StatsResponse};

use super::confirm::ConfirmPrompt;
use super::keymap::Keymap;
use super::state::{ClientState, KillTombstone};

// Re-export split types so ui.rs and mod.rs can still import from `app`.
pub use super::interaction::Modal;
pub use super::notifications::Notice;
pub use super::proxy_state::ProxyStatus;
pub use super::sessions::SessionView;

/// How the manager should apply a proxy for new sessions.
///
/// Resolved once at startup (from `--proxy`/`--no-proxy` flags or config)
/// and used as the pre-selected proxy in the new-session wizard.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum ProxyChoice {
    /// Use the profile named in `config.toml [proxy] default` (the default).
    #[default]
    Default,
    /// Explicitly no proxy, even when a default is configured (--no-proxy).
    Direct,
    /// Use a specific named profile (--proxy <name>).
    Profile(String),
}

/// Rows taken by the tab bar and the two-line status bar.
pub(super) const CHROME_ROWS: u16 = 3;

/// A single host-stats sample stored in the ring buffer.
#[derive(Debug, Clone, Copy)]
pub struct HostStatsSample {
    pub cpu_pct: f32,
    pub mem_used: u64,
    pub mem_total: u64,
}
/// How long a notice stays in the status bar, in ticks.
const NOTICE_TICKS: u32 = 24;
/// `RecentProjects` limit for the wizard.
pub(super) const PROJECTS_LIMIT: u32 = 50;

/// Something for the event loop to do.
#[derive(Debug)]
pub enum Effect {
    /// Send a request to this host's daemon; the reply goes to [`App::on_reply`].
    Request { host: String, msg: Msg, to: ReplyTo },
    /// Forward input bytes to a session's PTY.
    Input(SessionId, Vec<u8>),
    /// Bootstrap and connect to a remote host (bootstrap + connect_ssh).
    /// When done, the event loop calls [`App::on_host_connected`] or
    /// [`App::on_host_error`].
    Connect(String),
    /// Write state.json.
    Save,
    /// Fetch proxy config (env) for a profile name. Result goes to
    /// [`App::on_proxy_config`].
    FetchProxyConfig { profile_name: String },
    /// Fetch proxy stats for the active session's profile. Result goes to
    /// [`App::on_proxy_stats`].
    FetchProxyStats { profile_name: String },
    /// Spawn a session with proxy config: the effect runner fetches the proxy
    /// config (with a 5 s timeout), builds `SpawnSpec.env`, then sends the
    /// Spawn request. This avoids silently using fallback model defaults when
    /// the proxy-config cache is cold on the first spawn.
    SpawnWithProxy {
        host: String,
        /// Partially-built spec; `env` is empty and will be filled by the runner.
        spec: crate::proto::SpawnSpec,
        proxy_name: String,
        to: ReplyTo,
    },
}

/// What a request's reply is for.
#[derive(Debug, Clone, PartialEq)]
pub enum ReplyTo {
    /// Only errors matter; they are shown as a notice prefixed with this.
    Ack(&'static str),
    Spawned(SessionId),
    /// A spawn sent after an async proxy-config fetch: any `Attach` issued
    /// meanwhile reached the daemon first and failed, so attach on success.
    SpawnedDeferred(SessionId),
    /// Kill acknowledged; the `SessionId` lets us clear the tombstone.
    Kill(SessionId),
    DirEntries,
    /// Wizard request tagged with the wizard's generation (S7).
    ClaudeSessions(String, u64),
    Projects,
    /// Wizard directory listing for a remote host (host, path).
    RemoteDirEntries,
    /// Wizard claude sessions for a remote host (S7: generation-tagged).
    RemoteClaudeSessions(String, u64),
    /// Wizard recent projects for a remote host (host, wizard generation).
    /// Both must match the current wizard or the reply is discarded.
    RemoteProjects(String, u64),
    /// `UpdateClaude` on this host; the outcome becomes a notice.
    ClaudeUpdate(String),
}

/// The manager's state.
pub struct App {
    pub sessions: Vec<SessionView>,
    pub active: Option<usize>,
    pub modal: Option<Modal>,
    /// Most recently used directories per host ("local" for the local machine).
    pub recent_dirs: HashMap<String, Vec<String>>,
    /// The last `RecentProjects` reply, to seed the wizard.
    pub projects: Vec<String>,
    /// Terminal size.
    pub width: u16,
    pub height: u16,
    /// Animation counter, advanced every tick.
    pub tick: usize,
    /// Unix seconds, refreshed every tick.
    pub now: u64,
    pub notice: Option<Notice>,
    pub connected: bool,
    /// The daemon host's home directory.
    pub home: String,
    /// Set when the user asked to leave.
    pub quit: bool,
    /// Set when the screen needs repainting.
    pub redraw: bool,
    /// Effect queue; drained by `take_effects`. `pub(super)` so sibling
    /// modules (`sessions`, `interaction`) can push effects via `impl App`.
    pub(super) effects: Vec<Effect>,
    /// Cached proxy config (env vars) per profile name, with fetch time.
    pub proxy_config: HashMap<String, (ConfigResponse, Instant)>,
    /// Live proxy stats per profile name.
    pub proxy_status: HashMap<String, ProxyStatus>,
    /// Ordered list of available proxy profile names (empty = no proxy configured).
    pub proxy_profiles: Vec<String>,
    /// The default proxy profile from config.
    pub proxy_default: Option<String>,
    /// Startup proxy override: Direct suppresses the default; Profile forces
    /// a specific profile regardless of per-session selection in the wizard.
    pub proxy_override: ProxyChoice,
    /// Last state for which a desktop notification was sent per session.
    /// Used to debounce: only one notification per session per state change.
    pub notified: HashMap<SessionId, SessionState>,
    /// Whether to emit desktop notifications (from config.toml [ui] notify).
    pub notify_enabled: bool,
    /// Sessions that need a notification emitted after the next draw.
    /// Populated by `check_notifications`; drained by the event loop.
    pub pending_notifs: Vec<String>,
    /// Kill tombstones: sessions the user explicitly closed. Persisted before
    /// Kill is sent, and cleared only when the daemon acknowledges.
    pub killed: Vec<KillTombstone>,
    /// The effective keymap (defaults + config.toml overrides).
    /// Owned here so rendering always reflects the current bindings.
    pub keymap: Keymap,
    /// Set by the background upgrade-check task when a newer release is found.
    /// Rendered as a dim `↑ vX.Y.Z` indicator at the right of the status bar.
    pub upgrade_notice: Option<String>,
    /// Ring buffer of host stats samples, keyed by host name.
    /// Capacity capped at 30 samples per host (~1 min at 2-s interval).
    pub host_stats: HashMap<String, std::collections::VecDeque<HostStatsSample>>,
    /// `[claude]` from config.toml: the update policies.
    pub claude_policy: crate::config::ClaudeSection,
    /// This machine's `claude --version`; remote hosts are compared with it.
    pub local_claude: Option<String>,
    /// Per host, the claude version the user chose to skip (persisted).
    pub claude_skipped: HashMap<String, String>,
    /// Hosts whose claude was already checked in this run.
    pub(super) claude_checked: HashSet<String>,
    /// Prompts waiting for the open modal to close.
    pub(super) confirms: VecDeque<ConfirmPrompt>,
}

impl App {
    /// Create an App with default settings (notify enabled, default keymap).
    /// Used by unit tests; kept pub for future daemon-status command.
    #[cfg(test)]
    pub fn new(width: u16, height: u16, home: String, recent_dirs: Vec<String>) -> App {
        use std::collections::HashMap;
        let mut rd = HashMap::new();
        if !recent_dirs.is_empty() {
            rd.insert("local".to_owned(), recent_dirs);
        }
        App::new_with_config(width, height, home, rd, true, Keymap::default())
    }

    #[cfg(test)]
    pub fn new_with_config(
        width: u16,
        height: u16,
        home: String,
        recent_dirs: HashMap<String, Vec<String>>,
        notify_enabled: bool,
        keymap: Keymap,
    ) -> App {
        App::new_with_proxy(width, height, home, recent_dirs, notify_enabled, keymap, ProxyChoice::Default)
    }

    pub fn new_with_proxy(
        width: u16,
        height: u16,
        home: String,
        recent_dirs: HashMap<String, Vec<String>>,
        notify_enabled: bool,
        keymap: Keymap,
        proxy_override: ProxyChoice,
    ) -> App {
        let (proxy_profiles, proxy_default) = super::proxy_state::load_proxy_profiles();
        App {
            sessions: Vec::new(),
            active: None,
            modal: None,
            recent_dirs,
            projects: Vec::new(),
            width,
            height,
            tick: 0,
            now: crate::paths::unix_now(),
            notice: None,
            connected: true,
            home,
            quit: false,
            redraw: true,
            effects: Vec::new(),
            proxy_config: HashMap::new(),
            proxy_status: HashMap::new(),
            proxy_profiles,
            proxy_default,
            proxy_override,
            notified: HashMap::new(),
            notify_enabled,
            pending_notifs: Vec::new(),
            killed: Vec::new(),
            keymap,
            upgrade_notice: None,
            host_stats: HashMap::new(),
            claude_policy: Default::default(),
            local_claude: None,
            claude_skipped: HashMap::new(),
            claude_checked: HashSet::new(),
            confirms: VecDeque::new(),
        }
    }

    /// Drain the queued effects, in order.
    pub fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }

    // ── Proxy ─────────────────────────────────────────────────────────────────

    /// Called when a `FetchProxyConfig` effect completes.
    pub fn on_proxy_config(&mut self, profile_name: String, cfg: ConfigResponse) {
        self.proxy_config
            .insert(profile_name, (cfg, Instant::now()));
        self.redraw = true;
    }

    /// Called when a `FetchProxyStats` effect completes.
    pub fn on_proxy_stats(
        &mut self,
        profile_name: String,
        stats: Option<StatsResponse>,
        pool: Option<PoolHealthResponse>,
    ) {
        let entry = self.proxy_status.entry(profile_name).or_default();
        if let Some(s) = stats {
            entry.stats = Some(s);
        }
        if let Some(p) = pool {
            entry.pool = Some(p);
        }
        entry.fetched_at = Some(Instant::now());
        self.redraw = true;
    }

    /// Proxy config for a profile, if cached and fresh (5 min).
    pub fn proxy_config_cached(&self, name: &str) -> Option<&ConfigResponse> {
        self.proxy_config.get(name).and_then(|(cfg, when)| {
            if when.elapsed().as_secs() < 300 {
                Some(cfg)
            } else {
                None
            }
        })
    }

    /// Schedule a proxy config fetch if the cache is stale.
    pub fn maybe_fetch_proxy_config(&mut self, name: &str) {
        if self.proxy_config_cached(name).is_none() {
            self.effects.push(Effect::FetchProxyConfig {
                profile_name: name.to_owned(),
            });
        }
    }

    /// Schedule a proxy stats fetch for the named profile.
    pub fn schedule_proxy_stats(&mut self, name: &str) {
        self.effects.push(Effect::FetchProxyStats {
            profile_name: name.to_owned(),
        });
    }

    /// Open the stats popup for the active session's proxy profile.
    pub fn open_proxy_stats(&mut self) {
        if let Some(name) = self.active_view().and_then(|v| v.proxy.as_deref()) {
            let n = name.to_owned();
            self.schedule_proxy_stats(&n);
            self.modal = Some(Modal::ProxyStats { profile_name: n });
            self.redraw = true;
        } else {
            self.notify("No proxy configured for this session (Alt+n → proxy toggle)");
        }
    }

    /// Build the `SpawnSpec.env` for a proxy profile name (M4 fix: fallible).
    ///
    /// Returns `Err(msg)` if a profile is selected but cannot be resolved.
    pub(super) fn proxy_env_for(
        &self,
        proxy_name: Option<&str>,
    ) -> Result<Vec<(String, String)>, String> {
        super::proxy_state::proxy_env_for(
            proxy_name,
            proxy_name.and_then(|n| self.proxy_config_cached(n)),
        )
    }

    // ── Geometry ──────────────────────────────────────────────────────────────

    /// The pane size as `(rows, cols)`.
    pub fn pane_size(&self) -> (u16, u16) {
        (
            self.height.saturating_sub(CHROME_ROWS).max(1),
            self.width.max(1),
        )
    }

    pub fn active_view(&self) -> Option<&SessionView> {
        self.active.and_then(|i| self.sessions.get(i))
    }

    // ── Persistence ───────────────────────────────────────────────────────────

    /// What state.json should contain now.
    pub fn to_state(&self) -> ClientState {
        ClientState {
            sessions: self.sessions.iter().map(SessionView::saved).collect(),
            active: self.active_view().map(|v| v.id),
            recent_dirs: self.recent_dirs.clone(),
            killed: self.killed.clone(),
            claude_skipped: self.claude_skipped.clone(),
        }
    }

    /// Show a transient notice in the status bar.
    pub fn notify(&mut self, text: impl Into<String>) {
        self.notice = Some(Notice {
            text: text.into(),
            ticks_left: NOTICE_TICKS,
        });
        self.redraw = true;
    }

    // ── Notifications ─────────────────────────────────────────────────────────

    /// Check for background sessions that entered a notification-worthy state
    /// since the last check, and populate `pending_notifs` with their labels.
    ///
    /// The caller (event loop) emits the OS notifications after each draw,
    /// outside the ratatui buffer, to avoid corrupting the terminal state.
    pub fn check_notifications(&mut self) {
        if !self.notify_enabled {
            return;
        }
        super::notifications::check_notifications(
            &self.sessions,
            self.active,
            &mut self.notified,
            &mut self.pending_notifs,
        );
    }

    /// Record a host-stats sample in the ring buffer.
    pub fn on_host_stats(&mut self, host: String, cpu_pct: f32, mem_used: u64, mem_total: u64) {
        let ring = self.host_stats.entry(host).or_default();
        ring.push_back(HostStatsSample { cpu_pct, mem_used, mem_total });
        while ring.len() > 30 {
            ring.pop_front();
        }
        self.redraw = true;
    }

    /// Advance animations, the clock and notice timeouts.
    pub fn on_tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
        self.now = crate::paths::unix_now();
        self.confirm_tick();
        if let Some(n) = &mut self.notice {
            n.ticks_left = n.ticks_left.saturating_sub(1);
            if n.ticks_left == 0 {
                self.notice = None;
            }
        }
        self.redraw = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::Incoming;
    use crate::proto::{SessionEvent, SessionInfo};
    use crate::term::screen::Screen;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use uuid::Uuid;

    fn info(pid: Option<u32>, csid: Option<&str>) -> SessionInfo {
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
        }
    }

    fn key(code: KeyCode, mods: KeyModifiers) -> Event {
        Event::Key(KeyEvent::new(code, mods))
    }

    fn alt(c: char) -> Event {
        key(KeyCode::Char(c), KeyModifiers::ALT)
    }

    fn plain(code: KeyCode) -> Event {
        key(code, KeyModifiers::NONE)
    }

    fn app_with(live: &[SessionInfo]) -> App {
        let mut app = App::new(100, 30, "/home/u".into(), vec![]);
        // Isolate tests from any real proxy config on disk.
        app.proxy_default = None;
        app.proxy_profiles = Vec::new();
        app.recover(&ClientState::default(), live);
        app.take_effects();
        app
    }

    fn requests(effects: &[Effect]) -> Vec<&Msg> {
        effects
            .iter()
            .filter_map(|e| {
                if let Effect::Request { msg: m, .. } = e {
                    Some(m)
                } else {
                    None
                }
            })
            .collect()
    }

    #[test]
    fn recovery_respawns_dormant_sessions_and_attaches_the_saved_active() {
        let live = [info(Some(1), None), info(None, Some("c2"))];
        let saved = ClientState {
            active: Some(live[1].id),
            ..Default::default()
        };
        let mut app = App::new(100, 30, "/home/u".into(), vec![]);
        app.recover(&saved, &live);
        let effects = app.take_effects();
        let reqs = requests(&effects);
        let Msg::Spawn(spec) = reqs[0] else {
            panic!("expected Spawn, got {reqs:?}")
        };
        assert_eq!((spec.id, spec.rows), (live[1].id, 27));
        assert_eq!(spec.args, ["--resume", "c2"]);
        assert_eq!(
            *reqs[1],
            Msg::Attach {
                id: live[1].id,
                rows: 27,
                cols: 100
            }
        );
        assert!(matches!(effects.last(), Some(Effect::Save)));
        assert_eq!(app.active, Some(1));
        assert!(app.modal.is_none());
    }

    #[test]
    fn no_sessions_opens_the_wizard() {
        let mut app = App::new(100, 30, "/home/u".into(), vec![]);
        app.recover(&ClientState::default(), &[]);
        assert!(matches!(app.modal, Some(Modal::Wizard(_))));
        assert!(
            requests(&app.take_effects()).contains(&&Msg::RecentProjects {
                limit: PROJECTS_LIMIT
            })
        );
    }

    #[test]
    fn switching_detaches_the_old_session_then_attaches_the_new() {
        let live = [info(Some(1), None), info(Some(2), None)];
        let mut app = app_with(&live);
        app.on_terminal(key(KeyCode::Left, KeyModifiers::ALT));
        let effects = app.take_effects();
        assert_eq!(
            requests(&effects),
            vec![
                &Msg::Detach { id: live[0].id },
                &Msg::Attach {
                    id: live[1].id,
                    rows: 27,
                    cols: 100
                }
            ]
        );
        assert_eq!(app.active, Some(1), "wraps around");
        assert!(!app.sessions[0].attached && app.sessions[1].attached);
        // Attached resets the mirror, but only for the attached session.
        app.on_incoming_from(
            "local",
            Incoming::Attached {
                id: live[1].id,
                rows: 10,
                cols: 40,
            },
        );
        assert_eq!(app.sessions[1].mirror.size(), (10, 40));
        app.on_incoming_from(
            "local",
            Incoming::Attached {
                id: live[0].id,
                rows: 5,
                cols: 5,
            },
        );
        assert_eq!(app.sessions[0].mirror.size(), (27, 100));
    }

    #[test]
    fn keys_go_to_the_active_session_unless_a_modal_is_open() {
        let live = [info(Some(1), None)];
        let mut app = app_with(&live);
        app.on_terminal(plain(KeyCode::Char('h')));
        assert!(
            matches!(&app.take_effects()[..], [Effect::Input(id, b)] if *id == live[0].id && b == b"h")
        );
        app.on_terminal(alt('r'));
        app.on_terminal(plain(KeyCode::Char('h')));
        assert!(app.take_effects().is_empty());
        assert!(matches!(&app.modal, Some(Modal::Rename { input, .. }) if input == "apph"));
    }

    #[test]
    fn rename_saves_and_empty_clears() {
        let mut app = app_with(&[info(Some(1), None)]);
        app.on_terminal(alt('r'));
        app.on_terminal(key(KeyCode::Char('u'), KeyModifiers::CONTROL));
        app.on_terminal(Event::Paste("api".into()));
        app.on_terminal(plain(KeyCode::Enter));
        assert_eq!(app.sessions[0].name.as_deref(), Some("api"));
        // Rename sends the change to the daemon (durable journal) and saves state.
        let effects = app.take_effects();
        assert!(
            effects.iter().any(|e| matches!(e, Effect::Save)),
            "expected Save in effects"
        );
        assert!(
            effects.iter().any(|e| matches!(e, Effect::Request { msg: Msg::Rename { .. }, .. })),
            "expected Rename request in effects"
        );
        app.on_terminal(alt('r'));
        app.on_terminal(key(KeyCode::Char('u'), KeyModifiers::CONTROL));
        app.on_terminal(plain(KeyCode::Enter));
        assert_eq!(app.sessions[0].name, None);
        assert_eq!(app.to_state().sessions[0].name, None);
    }

    #[test]
    fn close_persists_intent_before_kill_and_moves_to_a_neighbour() {
        let live = [
            info(Some(1), None),
            info(Some(2), None),
            info(Some(3), None),
        ];
        let mut app = app_with(&live);
        app.on_terminal(alt('x'));
        app.on_terminal(plain(KeyCode::Char('n')));
        assert!(app.modal.is_none() && app.sessions.len() == 3, "n cancels");
        app.on_terminal(alt('x'));
        app.on_terminal(plain(KeyCode::Char('y')));
        let effects = app.take_effects();
        let save = effects
            .iter()
            .position(|e| matches!(e, Effect::Save))
            .unwrap();
        let kill = effects
            .iter()
            .position(
                |e| matches!(e, Effect::Request { msg: Msg::Kill { id }, .. } if *id == live[0].id),
            )
            .unwrap();
        assert!(save < kill, "Save must come before Kill");
        assert_eq!(app.sessions.len(), 2);
        assert_eq!(app.active_view().map(|v| v.id), Some(live[1].id));
        assert!(!app.to_state().sessions.iter().any(|s| s.id == live[0].id));
    }

    #[test]
    fn tombstone_persisted_in_state_before_kill() {
        let live = [info(Some(1), None)];
        let mut app = app_with(&live);
        let id = live[0].id;
        app.on_terminal(alt('x'));
        app.on_terminal(plain(KeyCode::Char('y')));
        // The tombstone must be in to_state() before Kill is sent.
        let effects = app.take_effects();
        let save_pos = effects
            .iter()
            .position(|e| matches!(e, Effect::Save))
            .unwrap();
        let kill_pos = effects
            .iter()
            .position(|e| matches!(e, Effect::Request { msg: Msg::Kill { id: k }, .. } if *k == id))
            .unwrap();
        assert!(save_pos < kill_pos, "tombstone must be saved before Kill");
        assert!(
            app.killed.iter().any(|t| t.id == id),
            "tombstone must be in app.killed"
        );
    }

    #[test]
    fn tombstone_cleared_on_kill_ok_reply() {
        let live = [info(Some(1), None)];
        let mut app = app_with(&live);
        let id = live[0].id;
        app.on_terminal(alt('x'));
        app.on_terminal(plain(KeyCode::Char('y')));
        app.take_effects();
        assert!(!app.killed.is_empty());
        // Simulate a successful Kill reply.
        app.on_reply(ReplyTo::Kill(id), Ok(Msg::Pong));
        assert!(
            app.killed.is_empty(),
            "tombstone must be cleared after Kill OK"
        );
    }

    #[test]
    fn tombstone_cleared_on_no_such_session_reply() {
        let live = [info(Some(1), None)];
        let mut app = app_with(&live);
        let id = live[0].id;
        app.on_terminal(alt('x'));
        app.on_terminal(plain(KeyCode::Char('y')));
        app.take_effects();
        // Simulate "no such session" error.
        app.on_reply(
            ReplyTo::Kill(id),
            Ok(Msg::Error {
                message: "no such session".into(),
            }),
        );
        assert!(
            app.killed.is_empty(),
            "tombstone cleared on 'no such session'"
        );
    }

    #[test]
    fn dropped_kill_is_resent_on_recovery() {
        // Simulate: session was killed, tombstone saved, but Kill never delivered.
        let id = Uuid::new_v4();
        let mut saved = ClientState::default();
        saved.killed.push(KillTombstone {
            host: "local".into(),
            id,
        });
        // The daemon still lists the session as live.
        let live = [SessionInfo {
            id,
            cwd: "/w".into(),
            name: None,
            state: SessionState::Idle,
            claude_session_id: None,
            title: None,
            pid: Some(42),
            created_at: 1,
            branch: None,
            model: None,
            context_tokens: None,
        }];
        let mut app = App::new(100, 30, "/home/u".into(), vec![]);
        app.recover(&saved, &live);
        let effects = app.take_effects();
        // Kill must be re-sent.
        let kill_re_sent = effects
            .iter()
            .any(|e| matches!(e, Effect::Request { msg: Msg::Kill { id: k }, .. } if *k == id));
        assert!(kill_re_sent, "dropped Kill must be re-sent on recovery");
        // The tombstoned session must NOT appear in the session list.
        assert!(
            !app.sessions.iter().any(|v| v.id == id),
            "tombstoned session must not appear"
        );
    }

    #[test]
    fn recover_host_does_not_wipe_other_hosts_sessions() {
        // Set up two sessions: one local, one remote.
        let local_id = Uuid::new_v4();
        let remote_id = Uuid::new_v4();

        let local_live = vec![SessionInfo {
            id: local_id,
            cwd: "/local".into(),
            name: None,
            state: SessionState::Idle,
            claude_session_id: None,
            title: None,
            pid: Some(1),
            created_at: 1,
            branch: None,
            model: None,
            context_tokens: None,
        }];
        let mut app = App::new(100, 30, "/home/u".into(), vec![]);
        app.recover(&ClientState::default(), &local_live);
        app.take_effects();
        assert_eq!(app.sessions.len(), 1);

        // Manually add a fake "remote" session (simulating a previous recover_host call).
        app.sessions.push(SessionView {
            id: remote_id,
            name: None,
            cwd: "/remote".into(),
            host: "myserver".into(),
            state: SessionState::Idle,
            title: None,
            claude_session_id: None,
            created_at: 1,
            mirror: crate::term::screen::Screen::new(28, 100),
            attached: false,
            proxy: None,
            branch: None,
            model: None,
            context_tokens: None,
        });
        assert_eq!(app.sessions.len(), 2);

        // Now simulate a second local reconnect: recover_host("local", ...) must
        // NOT remove the "myserver" session.
        app.recover_host("local", &local_live);
        app.take_effects();

        // The remote session must still be there.
        assert!(
            app.sessions.iter().any(|v| v.id == remote_id),
            "remote session must survive local recover_host"
        );
        assert!(
            app.sessions.iter().any(|v| v.id == local_id),
            "local session must still be there"
        );
    }

    #[test]
    fn attention_cycles_over_sessions_that_want_it() {
        let mut live = [
            info(Some(1), None),
            info(Some(2), None),
            info(Some(3), None),
        ];
        live[2].state = SessionState::NeedsApproval;
        let mut app = app_with(&live);
        app.on_terminal(alt('a'));
        assert_eq!(app.active, Some(2));
        app.on_terminal(alt('a'));
        assert_eq!(app.active, Some(2), "nothing else wants attention");
        let event = SessionEvent::State {
            state: SessionState::NeedsInput,
        };
        app.on_incoming_from(
            "local",
            Incoming::Event {
                id: live[0].id,
                event,
            },
        );
        app.on_terminal(alt('a'));
        assert_eq!(app.active, Some(0));
    }

    #[test]
    fn wizard_spawns_into_the_chosen_dir_and_records_it() {
        let mut app = app_with(&[]);
        // Step 0: press Enter to select "local" host (empty host filter → picks local).
        app.on_terminal(plain(KeyCode::Enter));
        app.take_effects(); // consume RecentProjects request
                            // Step 1: type "/w" into the directory input, then press Enter.
        for c in "/w".chars() {
            app.on_terminal(key(KeyCode::Char(c), KeyModifiers::NONE));
        }
        app.on_terminal(plain(KeyCode::Enter));
        let effects = app.take_effects();
        assert!(requests(&effects)
            .iter()
            .any(|m| **m == Msg::ListClaudeSessions { cwd: "/w".into() }));
        let gen = app.wizard_generation();
        let reply = Msg::ClaudeSessions {
            cwd: "/w".into(),
            sessions: vec![],
        };
        app.on_reply(ReplyTo::ClaudeSessions("/w".into(), gen), Ok(reply));
        let effects = app.take_effects();
        let reqs = requests(&effects);
        let Msg::Spawn(spec) = reqs[0] else {
            panic!("expected Spawn, got {reqs:?}")
        };
        assert_eq!((spec.cwd.as_str(), spec.args.len()), ("/w", 0));
        assert_eq!(
            *reqs[1],
            Msg::Attach {
                id: spec.id,
                rows: 27,
                cols: 100
            }
        );
        assert!(app.modal.is_none());
        assert_eq!(
            app.recent_dirs.get("local").map(Vec::as_slice).unwrap_or(&[]),
            &["/w".to_owned()][..]
        );
        assert_eq!(app.sessions[0].label(), "w");
    }

    #[test]
    fn events_update_and_add_sessions() {
        let live = [info(Some(1), None)];
        let mut app = app_with(&live);
        let id = live[0].id;
        app.on_incoming_from(
            "local",
            Incoming::Event {
                id,
                event: SessionEvent::Title {
                    title: "Refactor".into(),
                },
            },
        );
        let event = SessionEvent::ClaudeSession {
            claude_session_id: "c9".into(),
        };
        app.on_incoming_from("local", Incoming::Event { id, event });
        assert_eq!(app.sessions[0].label(), "Refactor");
        assert_eq!(
            app.to_state().sessions[0].claude_session_id.as_deref(),
            Some("c9")
        );
        let other = info(Some(5), None);
        let other_id = other.id;
        app.on_incoming_from(
            "local",
            Incoming::Event {
                id: other_id,
                event: SessionEvent::Created {
                    info: other.clone(),
                },
            },
        );
        assert_eq!(app.sessions.len(), 2);
        // M1 fix: new unknown session must get host "local" (from on_incoming_from).
        assert_eq!(app.sessions[1].host, "local");
        app.on_incoming_from(
            "local",
            Incoming::Event {
                id: other_id,
                event: SessionEvent::Removed,
            },
        );
        assert_eq!(app.sessions.len(), 1);
        assert_eq!(app.active, Some(0));
    }

    #[test]
    fn on_incoming_from_remote_tags_created_sessions_correctly() {
        let mut app = App::new(100, 30, "/home/u".into(), vec![]);
        app.recover(&ClientState::default(), &[]);
        // Close wizard.
        app.modal = None;
        app.take_effects();

        let new_session = info(Some(99), None);
        let new_id = new_session.id;
        app.on_incoming_from(
            "myserver",
            Incoming::Event {
                id: new_id,
                event: SessionEvent::Created { info: new_session },
            },
        );
        let sv = app.sessions.iter().find(|v| v.id == new_id).unwrap();
        assert_eq!(
            sv.host, "myserver",
            "remote session must have host=myserver"
        );
    }

    #[test]
    fn local_disconnect_does_not_affect_remote_sessions() {
        let local_id = Uuid::new_v4();
        let remote_id = Uuid::new_v4();
        let mut app = App::new(100, 30, "/home/u".into(), vec![]);
        app.sessions.push(SessionView {
            id: local_id,
            name: None,
            cwd: "/l".into(),
            host: "local".into(),
            state: SessionState::Idle,
            title: None,
            claude_session_id: None,
            created_at: 1,
            mirror: Screen::new(28, 100),
            attached: true,
            proxy: None,
            branch: None,
            model: None,
            context_tokens: None,
        });
        app.sessions.push(SessionView {
            id: remote_id,
            name: None,
            cwd: "/r".into(),
            host: "myserver".into(),
            state: SessionState::Idle,
            title: None,
            claude_session_id: None,
            created_at: 1,
            mirror: Screen::new(28, 100),
            attached: true,
            proxy: None,
            branch: None,
            model: None,
            context_tokens: None,
        });
        app.on_incoming_from("local", Incoming::Disconnected);
        // Local session detached.
        assert!(
            !app.sessions
                .iter()
                .find(|v| v.id == local_id)
                .unwrap()
                .attached
        );
        // Remote session NOT affected.
        assert!(
            app.sessions
                .iter()
                .find(|v| v.id == remote_id)
                .unwrap()
                .attached
        );
    }

    // ── Overview ──────────────────────────────────────────────────────────────

    #[test]
    fn overview_opens_on_alt_g_and_closes_on_esc() {
        let live = [info(Some(1), None), info(Some(2), None)];
        let mut app = app_with(&live);
        app.on_terminal(alt('g'));
        assert!(matches!(app.modal, Some(Modal::Overview { .. })));
        app.modal_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.modal.is_none());
    }

    #[test]
    fn overview_enter_activates_selected_session() {
        let live = [info(Some(1), None), info(Some(2), None)];
        let mut app = app_with(&live);
        assert_eq!(app.active, Some(0));
        app.on_terminal(alt('g'));
        // Navigate down to index 1.
        app.modal_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert!(matches!(app.modal, Some(Modal::Overview { selected: 1, .. })));
        // Press Enter — should activate session 1.
        app.modal_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.modal.is_none());
        assert_eq!(app.active, Some(1));
    }

    // ── Help ──────────────────────────────────────────────────────────────────

    #[test]
    fn help_opens_on_alt_h_and_any_key_closes() {
        let live = [info(Some(1), None)];
        let mut app = app_with(&live);
        app.on_terminal(alt('h'));
        assert!(matches!(app.modal, Some(Modal::Help)));
        // Any key closes help.
        app.modal_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.modal.is_none());
    }

    // ── Notifications ─────────────────────────────────────────────────────────

    #[test]
    fn notification_emitted_once_per_state_change() {
        let mut live = [info(Some(1), None), info(Some(2), None)];
        live[1].state = SessionState::NeedsApproval;
        let mut app = app_with(&live);
        // Session 1 (index 1) wants attention; session 0 is active.
        assert_eq!(app.active, Some(0));

        // First check: should produce one notification.
        app.check_notifications();
        assert_eq!(app.pending_notifs.len(), 1);
        let _ = app.pending_notifs.drain(..);

        // Second check without state change: no new notification.
        app.check_notifications();
        assert!(app.pending_notifs.is_empty(), "debounced: no second notif");

        // State change: new notification.
        let ev = SessionEvent::State {
            state: SessionState::NeedsInput,
        };
        app.on_incoming_from(
            "local",
            Incoming::Event {
                id: live[1].id,
                event: ev,
            },
        );
        app.check_notifications();
        assert_eq!(app.pending_notifs.len(), 1, "new state → new notification");
    }

    #[test]
    fn active_session_never_notifies() {
        let mut live = [info(Some(1), None)];
        live[0].state = SessionState::NeedsApproval;
        let mut app = app_with(&live);
        // Session 0 is both active and wants attention.
        assert_eq!(app.active, Some(0));
        app.check_notifications();
        assert!(
            app.pending_notifs.is_empty(),
            "active session must not notify"
        );
    }

    #[test]
    fn notifications_disabled_when_flag_is_off() {
        let mut live = [info(Some(1), None), info(Some(2), None)];
        live[1].state = SessionState::NeedsApproval;
        let mut app =
            App::new_with_config(100, 30, "/home/u".into(), HashMap::new(), false, Keymap::default());
        app.recover(&ClientState::default(), &live);
        app.take_effects();
        app.check_notifications();
        assert!(app.pending_notifs.is_empty(), "notifications disabled");
    }

    // ── Wizard generation / S7 ────────────────────────────────────────────────

    #[test]
    fn stale_wizard_reply_is_discarded() {
        let mut app = app_with(&[]);
        let gen = app.wizard_generation();
        // Simulate wizard being cancelled.
        app.modal = None;
        // Now a reply with the old gen arrives; it must be silently dropped.
        let reply = Msg::ClaudeSessions {
            cwd: "/w".into(),
            sessions: vec![],
        };
        app.on_reply(ReplyTo::ClaudeSessions("/w".into(), gen), Ok(reply));
        // No modal opened (wizard was closed).
        assert!(app.modal.is_none());
    }
}
