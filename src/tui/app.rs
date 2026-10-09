//! Manager state and update logic.
//!
//! [`App`] holds the session views, the open modal and the bits of
//! persistent state. It never does I/O: inputs arrive through the `on_*`
//! methods and everything it wants done (daemon requests, PTY input, saving
//! state.json) is queued as [`Effect`]s that the event loop in `mod.rs`
//! drains with [`App::take_effects`].

use std::collections::HashMap;
use std::io;
use std::time::Instant;

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use uuid::Uuid;

use crate::client::Incoming;
use crate::proxy::api::{ConfigResponse, PoolHealthResponse, StatsResponse};
use crate::proto::{Msg, SessionEvent, SessionId, SessionInfo, SessionState, SpawnSpec};
use crate::term::keys::{encode_focus, encode_key, encode_mouse, encode_paste};
use crate::term::screen::Screen;

use super::keymap::{self, Action};
use super::state::{self, ClientState, SavedSession};
use super::ui;
use super::wizard::{self, Outcome, Wizard};

/// Rows taken by the tab bar and the status bar.
const CHROME_ROWS: u16 = 2;
/// How long a notice stays in the status bar, in ticks.
const NOTICE_TICKS: u32 = 24;
/// `RecentProjects` limit for the wizard.
const PROJECTS_LIMIT: u32 = 50;

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
}

/// What a request's reply is for.
#[derive(Debug, Clone, PartialEq)]
pub enum ReplyTo {
    /// Only errors matter; they are shown as a notice prefixed with this.
    Ack(&'static str),
    Spawned(SessionId),
    DirEntries,
    ClaudeSessions(String),
    Projects,
    /// Wizard directory listing for a remote host (host, path).
    RemoteDirEntries,
    /// Wizard claude sessions for a remote host.
    RemoteClaudeSessions(String),
    /// Wizard recent projects for a remote host.
    RemoteProjects,
}

/// The client-side view of one session.
pub struct SessionView {
    pub id: SessionId,
    pub name: Option<String>,
    pub cwd: String,
    pub host: String,
    pub state: SessionState,
    pub title: Option<String>,
    pub claude_session_id: Option<String>,
    pub created_at: u64,
    /// Mirror of the daemon's screen; fed only while attached.
    pub mirror: Screen,
    pub attached: bool,
    /// Proxy profile name (None = no proxy). Only the name, never the token.
    pub proxy: Option<String>,
}

impl SessionView {
    /// The tab label: the user's name, else claude's title, else the cwd's
    /// basename.
    pub fn label(&self) -> String {
        self.name
            .clone()
            .or_else(|| self.title.clone())
            .unwrap_or_else(|| self.cwd.rsplit('/').find(|s| !s.is_empty()).unwrap_or("/").to_owned())
    }

    fn saved(&self) -> SavedSession {
        SavedSession {
            id: self.id,
            name: self.name.clone(),
            cwd: self.cwd.clone(),
            host: self.host.clone(),
            claude_session_id: self.claude_session_id.clone(),
            created_at: self.created_at,
            proxy: self.proxy.clone(),
        }
    }
}

/// A transient status-bar message.
pub struct Notice {
    pub text: String,
    ticks_left: u32,
}

/// Live proxy data shown in the status bar for the active session.
#[derive(Debug, Clone, Default)]
pub struct ProxyStatus {
    pub pool: Option<PoolHealthResponse>,
    pub stats: Option<StatsResponse>,
    /// When the stats were last fetched.
    pub fetched_at: Option<Instant>,
}

/// A popup that captures the keyboard.
pub enum Modal {
    Rename { id: SessionId, input: String },
    /// Confirm killing a session.
    Close { id: SessionId },
    Wizard(Wizard),
    /// Proxy stats popup.
    ProxyStats { profile_name: String },
    /// Overview / "mission control": all sessions at a glance.
    Overview { selected: usize },
    /// Help popup: all key bindings.
    Help,
}

/// The manager's state.
pub struct App {
    pub sessions: Vec<SessionView>,
    pub active: Option<usize>,
    pub modal: Option<Modal>,
    pub recent_dirs: Vec<String>,
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
    effects: Vec<Effect>,
    /// Cached proxy config (env vars) per profile name, with fetch time.
    pub proxy_config: HashMap<String, (ConfigResponse, Instant)>,
    /// Live proxy stats per profile name.
    pub proxy_status: HashMap<String, ProxyStatus>,
    /// Ordered list of available proxy profile names (empty = no proxy configured).
    pub proxy_profiles: Vec<String>,
    /// The default proxy profile from config.
    pub proxy_default: Option<String>,
    /// Last state for which a desktop notification was sent per session.
    /// Used to debounce: only one notification per session per state change.
    pub notified: HashMap<SessionId, crate::proto::SessionState>,
    /// Whether to emit desktop notifications (from config.toml [ui] notify).
    pub notify_enabled: bool,
    /// Sessions that need a notification emitted after the next draw.
    /// Populated by `check_notifications`; drained by the event loop.
    pub pending_notifs: Vec<String>,
}

/// Current Unix time in seconds.
pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl App {
    /// Create an App with default settings (notify enabled). Tests and the
    /// daemon-status command use this.
    #[allow(dead_code)]
    pub fn new(width: u16, height: u16, home: String, recent_dirs: Vec<String>) -> App {
        App::new_with_config(width, height, home, recent_dirs, true)
    }

    pub fn new_with_config(
        width: u16,
        height: u16,
        home: String,
        recent_dirs: Vec<String>,
        notify_enabled: bool,
    ) -> App {
        // Load proxy profiles once at startup.
        let (proxy_profiles, proxy_default) = load_proxy_profiles();
        App {
            sessions: Vec::new(),
            active: None,
            modal: None,
            recent_dirs,
            projects: Vec::new(),
            width,
            height,
            tick: 0,
            now: unix_now(),
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
            notified: HashMap::new(),
            notify_enabled,
            pending_notifs: Vec::new(),
        }
    }

    /// Drain the queued effects, in order.
    pub fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }

    // ── Proxy ─────────────────────────────────────────────────────────────────

    /// Called when a `FetchProxyConfig` effect completes.
    pub fn on_proxy_config(&mut self, profile_name: String, cfg: ConfigResponse) {
        self.proxy_config.insert(profile_name, (cfg, Instant::now()));
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
            self.effects.push(Effect::FetchProxyConfig { profile_name: name.to_owned() });
        }
    }

    /// Schedule a proxy stats fetch for the named profile.
    pub fn schedule_proxy_stats(&mut self, name: &str) {
        self.effects.push(Effect::FetchProxyStats { profile_name: name.to_owned() });
    }

    /// The proxy status for the active session's profile, if any.
    pub fn active_proxy_status(&self) -> Option<(&str, &ProxyStatus)> {
        let name = self.active_view()?.proxy.as_deref()?;
        let status = self.proxy_status.get(name)?;
        Some((name, status))
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

    /// The pane size as `(rows, cols)`.
    pub fn pane_size(&self) -> (u16, u16) {
        (self.height.saturating_sub(CHROME_ROWS).max(1), self.width.max(1))
    }

    pub fn active_view(&self) -> Option<&SessionView> {
        self.active.and_then(|i| self.sessions.get(i))
    }

    /// What state.json should contain now.
    pub fn to_state(&self) -> ClientState {
        ClientState {
            sessions: self.sessions.iter().map(SessionView::saved).collect(),
            active: self.active_view().map(|v| v.id),
            recent_dirs: self.recent_dirs.clone(),
        }
    }

    /// Show a transient notice in the status bar.
    pub fn notify(&mut self, text: impl Into<String>) {
        self.notice = Some(Notice { text: text.into(), ticks_left: NOTICE_TICKS });
        self.redraw = true;
    }

    // ── Recovery ─────────────────────────────────────────────────────────────

    /// Rebuild the session list from `saved` and the daemon's `live`
    /// sessions: re-spawn dormant ones, attach the saved active session, and
    /// open the wizard when there is nothing to show.
    pub fn recover(&mut self, saved: &ClientState, live: &[SessionInfo]) {
        let (rows, cols) = self.pane_size();
        self.connected = true;
        self.sessions.clear();
        self.active = None;
        for r in state::merge(saved, live) {
            if let Some(args) = r.respawn {
                // Re-send proxy env so the respawned claude still routes
                // through the proxy (fix: was Vec::new(), losing the token).
                let env = self.proxy_env_for(r.saved.proxy.as_deref());
                let spec = SpawnSpec {
                    id: r.saved.id,
                    cwd: r.saved.cwd.clone(),
                    name: r.saved.name.clone(),
                    args,
                    env,
                    rows,
                    cols,
                };
                let host = r.saved.host.clone();
                self.effects.push(Effect::Request {
                    host,
                    msg: Msg::Spawn(spec),
                    to: ReplyTo::Spawned(r.saved.id),
                });
            }
            self.sessions.push(SessionView {
                id: r.saved.id,
                name: r.saved.name,
                cwd: r.saved.cwd,
                host: r.saved.host,
                state: r.state,
                title: r.title,
                claude_session_id: r.saved.claude_session_id,
                created_at: r.saved.created_at,
                mirror: Screen::new(rows, cols),
                attached: false,
                proxy: r.saved.proxy,
            });
        }
        let restore = saved.active.and_then(|id| self.index_of(id));
        match restore.or((!self.sessions.is_empty()).then_some(0)) {
            Some(i) => self.activate(i),
            None if self.modal.is_none() => self.open_wizard(),
            None => {}
        }
        self.save();
    }

    /// Re-run recovery after a reconnect, from the current in-memory state.
    pub fn on_reconnected(&mut self, live: &[SessionInfo], home: String) {
        self.home = home;
        self.notice = None;
        let saved = self.to_state();
        self.recover(&saved, live);
    }

    /// The daemon connection dropped.
    pub fn on_disconnected(&mut self) {
        self.connected = false;
        for v in &mut self.sessions {
            v.attached = false;
        }
        self.redraw = true;
    }

    // ── Session management ───────────────────────────────────────────────────

    fn index_of(&self, id: SessionId) -> Option<usize> {
        self.sessions.iter().position(|v| v.id == id)
    }

    fn save(&mut self) {
        if !matches!(self.effects.last(), Some(Effect::Save)) {
            self.effects.push(Effect::Save);
        }
    }

    /// Send a request to `host`'s daemon.
    fn request(&mut self, host: &str, msg: Msg, to: ReplyTo) {
        self.effects.push(Effect::Request { host: host.to_owned(), msg, to });
    }

    /// Send a request to the local daemon.
    fn request_local(&mut self, msg: Msg, to: ReplyTo) {
        self.request("local", msg, to);
    }

    /// The host of the active session (or "local" when none is active).
    fn active_host(&self) -> String {
        self.active_view().map(|v| v.host.clone()).unwrap_or_else(|| "local".to_owned())
    }

    /// The host the wizard is currently targeting (or "local").
    fn wizard_host(&self) -> String {
        match &self.modal {
            Some(Modal::Wizard(w)) => w.host.clone(),
            _ => "local".to_owned(),
        }
    }

    /// Make session `i` the active one: detach the old, attach the new.
    fn activate(&mut self, i: usize) {
        if i >= self.sessions.len() {
            return;
        }
        if self.active == Some(i) && self.sessions[i].attached {
            return;
        }
        if let Some(old) = self.active.filter(|&old| old != i) {
            if let Some(v) = self.sessions.get_mut(old).filter(|v| v.attached) {
                v.attached = false;
                let id = v.id;
                let host = v.host.clone();
                self.request(&host, Msg::Detach { id }, ReplyTo::Ack("detach"));
            }
        }
        self.active = Some(i);
        let (rows, cols) = self.pane_size();
        let view = &mut self.sessions[i];
        view.attached = true;
        // Blank until the daemon's `Attached` + snapshot arrive.
        view.mirror = Screen::new(rows, cols);
        let id = view.id;
        let host = view.host.clone();
        self.request(&host, Msg::Attach { id, rows, cols }, ReplyTo::Ack("attach"));
        self.redraw = true;
        self.save();
    }

    /// Forget session `i` and activate a neighbour.
    fn remove(&mut self, i: usize) {
        if i >= self.sessions.len() {
            return;
        }
        self.sessions.remove(i);
        match self.active {
            Some(a) if a == i => {
                self.active = None;
                if !self.sessions.is_empty() {
                    self.activate(i.min(self.sessions.len() - 1));
                }
            }
            Some(a) if a > i => self.active = Some(a - 1),
            _ => {}
        }
        if self.sessions.is_empty() && self.modal.is_none() {
            self.open_wizard();
        }
        self.redraw = true;
        self.save();
    }

    /// Spawn claude in `cwd` on `host` (optionally resuming) and switch to it.
    fn spawn(&mut self, host: String, cwd: String, resume: Option<String>, proxy: Option<String>) {
        let (rows, cols) = self.pane_size();
        let id = Uuid::new_v4();
        let args = resume.map(|r| vec!["--resume".to_owned(), r]).unwrap_or_default();
        // Build proxy env if a profile is selected.
        let env = self.proxy_env_for(proxy.as_deref());
        let spec = SpawnSpec { id, cwd: cwd.clone(), name: None, args, env, rows, cols };
        self.request(&host, Msg::Spawn(spec), ReplyTo::Spawned(id));
        self.sessions.push(SessionView {
            id,
            name: None,
            cwd: cwd.clone(),
            host: host.clone(),
            state: SessionState::Starting,
            title: None,
            claude_session_id: None,
            created_at: self.now,
            mirror: Screen::new(rows, cols),
            attached: false,
            proxy: proxy.clone(),
        });
        state::push_recent(&mut self.recent_dirs, &cwd);
        self.activate(self.sessions.len() - 1);
    }

    /// Build the `SpawnSpec.env` for a proxy profile name (or empty when none).
    fn proxy_env_for(&self, proxy_name: Option<&str>) -> Vec<(String, String)> {
        let name = match proxy_name {
            Some(n) => n,
            None => return Vec::new(),
        };
        // Resolve the profile: env var overrides if name is "env".
        let profile = if name == "env" {
            crate::proxy::profile::from_env().map(|(_, p)| p)
        } else {
            crate::proxy::profile::load().ok()
                .and_then(|sec| sec.profiles.get(name).cloned())
        };
        match profile {
            Some(p) => {
                let cfg = self.proxy_config_cached(name);
                crate::proxy::env::session_env(&p, cfg)
            }
            None => Vec::new(),
        }
    }

    fn open_wizard(&mut self) {
        let active_cwd = self.active_view().map(|v| v.cwd.clone());
        let active_proxy = self.active_view().and_then(|v| v.proxy.clone());
        let active_host = self.active_host();
        let seeds = wizard::assemble(active_cwd.as_deref(), &self.recent_dirs, &self.projects);
        let host_candidates = crate::remote::hosts::candidates();
        let proxy_default = active_proxy.as_deref().or(self.proxy_default.as_deref());
        self.modal = Some(Modal::Wizard(Wizard::new(
            seeds,
            self.home.clone(),
            &active_host,
            &host_candidates,
            &self.proxy_profiles.clone(),
            proxy_default,
        )));
        self.request_local(Msg::RecentProjects { limit: PROJECTS_LIMIT }, ReplyTo::Projects);
        self.redraw = true;
    }

    /// Act on what the wizard decided.
    fn wizard_outcome(&mut self, outcome: Outcome) {
        let wizard_host = self.wizard_host();
        match outcome {
            Outcome::None => {}
            Outcome::Cancel => self.modal = None,
            Outcome::ConnectHost(host) => {
                if host == "local" {
                    // Local: advance immediately.
                    if let Some(Modal::Wizard(w)) = &mut self.modal {
                        w.on_host_connected("local", &self.home);
                    }
                    // Refresh recent projects for local.
                    self.request_local(
                        Msg::RecentProjects { limit: PROJECTS_LIMIT },
                        ReplyTo::Projects,
                    );
                } else {
                    // Remote: kick off bootstrap + connect_ssh.
                    self.effects.push(Effect::Connect(host));
                }
            }
            Outcome::ListDir(path) => {
                let reply_to = if wizard_host == "local" {
                    ReplyTo::DirEntries
                } else {
                    ReplyTo::RemoteDirEntries
                };
                self.request(&wizard_host, Msg::ListDir { path }, reply_to)
            }
            Outcome::ChooseDir(cwd) => {
                let reply_to = if wizard_host == "local" {
                    ReplyTo::ClaudeSessions(cwd.clone())
                } else {
                    ReplyTo::RemoteClaudeSessions(cwd.clone())
                };
                self.request(&wizard_host, Msg::ListClaudeSessions { cwd }, reply_to)
            }
            Outcome::Spawn { cwd, resume, proxy } => {
                self.modal = None;
                // If proxy was selected, ensure config is pre-fetched.
                if let Some(ref pname) = proxy {
                    self.maybe_fetch_proxy_config(pname);
                }
                self.spawn(wizard_host, cwd, resume, proxy);
            }
        }
        self.redraw = true;
    }

    /// Called by the event loop when a remote host connection succeeds.
    pub fn on_host_connected(&mut self, host: &str, home: &str) {
        if let Some(Modal::Wizard(w)) = &mut self.modal {
            w.on_host_connected(host, home);
            // Fetch recent projects from the remote daemon to seed the directory step.
            let h = host.to_owned();
            self.effects.push(Effect::Request {
                host: h,
                msg: Msg::RecentProjects { limit: PROJECTS_LIMIT },
                to: ReplyTo::RemoteProjects,
            });
        }
        self.redraw = true;
    }

    /// Called by the event loop when a remote host connection fails.
    pub fn on_host_error(&mut self, host: &str, error: &str) {
        // Clear the "connecting" state from the wizard.
        if let Some(Modal::Wizard(w)) = &mut self.modal {
            if let Some(hs) = &mut w.host_step {
                hs.connecting = None;
            }
        }
        self.notify(format!("cannot connect to {host}: {error}"));
    }

    fn wizard_mut(&mut self) -> Option<&mut Wizard> {
        match &mut self.modal {
            Some(Modal::Wizard(w)) => Some(w),
            _ => None,
        }
    }

    // ── Terminal input ───────────────────────────────────────────────────────

    pub fn on_terminal(&mut self, ev: Event) {
        match ev {
            Event::Key(key) => self.on_key(key),
            Event::Paste(text) => self.on_paste(&text),
            Event::Mouse(m) => self.on_mouse(m),
            Event::FocusGained => self.on_focus(true),
            Event::FocusLost => self.on_focus(false),
            Event::Resize(w, h) => self.on_resize(w, h),
        }
    }

    fn on_key(&mut self, key: KeyEvent) {
        let action = keymap::lookup(&key);
        // Quitting always works, even from a dialog: sessions keep running.
        if action == Some(Action::Quit) {
            self.on_action(Action::Quit);
            return;
        }
        if self.modal.is_some() {
            self.modal_key(key);
            return;
        }
        if let Some(action) = action {
            self.on_action(action);
            return;
        }
        if let Some(v) = self.active_view().filter(|v| v.attached) {
            let bytes = encode_key(&key, &v.mirror.modes());
            if !bytes.is_empty() {
                self.effects.push(Effect::Input(v.id, bytes));
            }
        }
    }

    fn on_action(&mut self, action: Action) {
        let n = self.sessions.len();
        match action {
            Action::PrevSession | Action::NextSession if n > 0 => {
                let cur = self.active.unwrap_or(0);
                let next = if action == Action::NextSession { (cur + 1) % n } else { (cur + n - 1) % n };
                self.activate(next);
            }
            Action::NextAttention if n > 0 => {
                let cur = self.active.unwrap_or(0);
                let found = (1..n).map(|k| (cur + k) % n).find(|&i| self.sessions[i].state.wants_attention());
                match found {
                    Some(i) => self.activate(i),
                    None => self.notify("no session needs attention"),
                }
            }
            Action::NewSession => self.open_wizard(),
            Action::Rename => {
                if let Some(v) = self.active_view() {
                    self.modal = Some(Modal::Rename { id: v.id, input: v.label() });
                }
            }
            Action::Close => {
                if let Some(v) = self.active_view() {
                    self.modal = Some(Modal::Close { id: v.id });
                }
            }
            Action::Quit => {
                self.save();
                self.quit = true;
            }
            Action::ProxyStats => self.open_proxy_stats(),
            Action::Overview => self.open_overview(),
            Action::Help => self.open_help(),
            _ => {}
        }
        self.redraw = true;
    }

    /// Open the overview popup, selecting the active session.
    fn open_overview(&mut self) {
        let selected = self.active.unwrap_or(0);
        self.modal = Some(Modal::Overview { selected });
        self.redraw = true;
    }

    /// Open the help popup.
    fn open_help(&mut self) {
        self.modal = Some(Modal::Help);
        self.redraw = true;
    }

    /// Check for background sessions that entered a notification-worthy state
    /// since the last check, and populate `pending_notifs` with their labels.
    ///
    /// The caller (event loop) emits the OS notifications after each draw,
    /// outside the ratatui buffer, to avoid corrupting the terminal state.
    pub fn check_notifications(&mut self) {
        if !self.notify_enabled {
            return;
        }
        for (i, v) in self.sessions.iter().enumerate() {
            if Some(i) == self.active {
                // Never notify for the session the user is currently watching.
                continue;
            }
            if !v.state.wants_attention() {
                // Clear debounce state so a later attention state triggers again.
                self.notified.remove(&v.id);
                continue;
            }
            // Only notify once per (session, state) combination.
            if self.notified.get(&v.id) == Some(&v.state) {
                continue;
            }
            self.notified.insert(v.id, v.state);
            self.pending_notifs.push(v.label());
        }
    }

    fn modal_key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        self.redraw = true;
        let Some(modal) = &mut self.modal else { return };
        match modal {
            Modal::Wizard(w) => {
                let outcome = w.on_key(&key);
                self.wizard_outcome(outcome);
            }
            Modal::Rename { id, input } => match key.code {
                KeyCode::Esc => self.modal = None,
                KeyCode::Enter => {
                    let (id, name) = (*id, input.trim().to_owned());
                    self.modal = None;
                    if let Some(i) = self.index_of(id) {
                        self.sessions[i].name = (!name.is_empty()).then_some(name);
                        self.save();
                    }
                }
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Char('u') if key.modifiers == KeyModifiers::CONTROL => input.clear(),
                KeyCode::Char(c) if key.modifiers.difference(KeyModifiers::SHIFT).is_empty() => input.push(c),
                _ => {}
            },
            Modal::Close { id } => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    let id = *id;
                    self.modal = None;
                    if let Some(i) = self.index_of(id) {
                        // Persist the kill intent before sending `Kill`, so
                        // recovery never resurrects a closed session.
                        let host = self.sessions[i].host.clone();
                        self.remove(i);
                        self.save();
                        self.request(&host, Msg::Kill { id }, ReplyTo::Ack("kill"));
                    }
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => self.modal = None,
                _ => {}
            },
            Modal::ProxyStats { .. } => {
                if matches!(key.code, KeyCode::Esc | KeyCode::Char('q')) {
                    self.modal = None;
                }
            }
            Modal::Overview { selected } => match key.code {
                KeyCode::Esc => self.modal = None,
                KeyCode::Up if *selected > 0 => *selected -= 1,
                KeyCode::Down => {
                    let max = self.sessions.len().saturating_sub(1);
                    if *selected < max {
                        *selected += 1;
                    }
                }
                KeyCode::Enter => {
                    if let Modal::Overview { selected } = self.modal.take().unwrap() {
                        self.activate(selected);
                    }
                }
                _ => {}
            },
            Modal::Help => {
                // Any key closes the help.
                self.modal = None;
            }
        }
    }

    fn on_paste(&mut self, text: &str) {
        match &mut self.modal {
            Some(Modal::Wizard(w)) => {
                let outcome = w.on_paste(text);
                self.wizard_outcome(outcome);
            }
            Some(Modal::Rename { input, .. }) => {
                input.push_str(text.lines().next().unwrap_or(""));
                self.redraw = true;
            }
            Some(Modal::Close { .. })
            | Some(Modal::ProxyStats { .. })
            | Some(Modal::Overview { .. })
            | Some(Modal::Help) => {}
            None => {
                if let Some(v) = self.active_view().filter(|v| v.attached) {
                    let bytes = encode_paste(text, &v.mirror.modes());
                    self.effects.push(Effect::Input(v.id, bytes));
                }
            }
        }
    }

    fn on_mouse(&mut self, m: MouseEvent) {
        if self.modal.is_some() {
            return;
        }
        if m.row == 0 {
            if m.kind == MouseEventKind::Down(MouseButton::Left) {
                let titles = ui::tab_titles(&self.sessions, self.active, self.width, self.now);
                if let Some(i) = ui::tab_at(&titles, m.column) {
                    self.activate(i);
                }
            }
            return;
        }
        let (rows, cols) = self.pane_size();
        if let Some(v) = self.active_view().filter(|v| v.attached) {
            if let Some(bytes) = encode_mouse(&m, (0, 1), (cols, rows), &v.mirror.modes()) {
                self.effects.push(Effect::Input(v.id, bytes));
            }
        }
    }

    fn on_focus(&mut self, gained: bool) {
        if let Some(v) = self.active_view().filter(|v| v.attached) {
            if let Some(bytes) = encode_focus(gained, &v.mirror.modes()) {
                self.effects.push(Effect::Input(v.id, bytes));
            }
        }
    }

    fn on_resize(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
        self.redraw = true;
        let (rows, cols) = self.pane_size();
        if let Some(i) = self.active.filter(|&i| self.sessions[i].attached) {
            let v = &mut self.sessions[i];
            // Resize locally right away; the child repaints after SIGWINCH.
            v.mirror.resize(rows, cols);
            let id = v.id;
            let host = v.host.clone();
            self.request(&host, Msg::Resize { id, rows, cols }, ReplyTo::Ack("resize"));
        }
    }

    // ── Daemon input ─────────────────────────────────────────────────────────

    pub fn on_incoming(&mut self, inc: Incoming) {
        match inc {
            Incoming::Data { id, bytes } => {
                if let Some(i) = self.index_of(id).filter(|&i| self.sessions[i].attached) {
                    self.sessions[i].mirror.feed(&bytes);
                    if self.active == Some(i) {
                        self.redraw = true;
                    }
                }
            }
            Incoming::Attached { id, rows, cols } => {
                if let Some(i) = self.index_of(id).filter(|&i| self.sessions[i].attached) {
                    self.sessions[i].mirror = Screen::new(rows, cols);
                    self.redraw = true;
                }
            }
            Incoming::Event { id, event } => self.on_event(id, event),
            Incoming::Disconnected => self.on_disconnected(),
        }
    }

    fn on_event(&mut self, id: SessionId, event: SessionEvent) {
        self.redraw = true;
        if let SessionEvent::Created { info } = &event {
            if self.index_of(id).is_none() {
                let (rows, cols) = self.pane_size();
                self.sessions.push(SessionView {
                    id,
                    name: info.name.clone(),
                    cwd: info.cwd.clone(),
                    host: "local".to_owned(),
                    state: info.state,
                    title: info.title.clone(),
                    claude_session_id: info.claude_session_id.clone(),
                    created_at: info.created_at,
                    mirror: Screen::new(rows, cols),
                    attached: false,
                    proxy: None,
                });
                self.save();
                return;
            }
        }
        let Some(i) = self.index_of(id) else { return };
        let v = &mut self.sessions[i];
        match event {
            SessionEvent::Created { info } => {
                v.cwd = info.cwd;
                v.state = info.state;
                v.title = info.title.or(v.title.take());
                v.created_at = info.created_at;
                if info.claude_session_id.is_some() {
                    v.claude_session_id = info.claude_session_id;
                }
                self.save();
            }
            SessionEvent::Removed => self.remove(i),
            SessionEvent::State { state } => v.state = state,
            SessionEvent::ClaudeSession { claude_session_id } => {
                v.claude_session_id = Some(claude_session_id);
                self.save();
            }
            SessionEvent::Title { title } => v.title = Some(title),
            SessionEvent::Exited { .. } => v.state = SessionState::Exited,
        }
    }

    /// A reply (or failure) for a request made through [`Effect::Request`].
    pub fn on_reply(&mut self, to: ReplyTo, reply: io::Result<Msg>) {
        self.redraw = true;
        match (to, reply) {
            (ReplyTo::Projects, Ok(Msg::Projects { dirs })) => {
                self.projects = dirs.into_iter().map(|d| d.path).collect();
                let projects = self.projects.clone();
                if let Some(w) = self.wizard_mut() {
                    w.add_seeds(&projects);
                }
            }
            (ReplyTo::DirEntries, Ok(Msg::DirEntries { path, entries })) => {
                if let Some(w) = self.wizard_mut() {
                    w.set_dir_entries(&path, &entries);
                }
            }
            (ReplyTo::ClaudeSessions(cwd), reply) => {
                let sessions = match reply {
                    Ok(Msg::ClaudeSessions { sessions, .. }) => sessions,
                    Ok(_) => Vec::new(),
                    Err(e) => {
                        self.notify(format!("could not list claude sessions: {e}"));
                        Vec::new()
                    }
                };
                if let Some(w) = self.wizard_mut() {
                    let outcome = w.set_claude_sessions(&cwd, sessions);
                    self.wizard_outcome(outcome);
                }
            }
            (ReplyTo::Spawned(id), Err(e)) => {
                if let Some(i) = self.index_of(id) {
                    self.sessions[i].state = SessionState::Exited;
                }
                self.notify(format!("could not start claude: {e}"));
            }
            // Remote variants route to the same wizard handlers.
            (ReplyTo::RemoteProjects, Ok(Msg::Projects { dirs })) => {
                self.projects = dirs.into_iter().map(|d| d.path).collect();
                let projects = self.projects.clone();
                if let Some(w) = self.wizard_mut() {
                    w.add_seeds(&projects);
                }
            }
            (ReplyTo::RemoteDirEntries, Ok(Msg::DirEntries { path, entries })) => {
                if let Some(w) = self.wizard_mut() {
                    w.set_dir_entries(&path, &entries);
                }
            }
            (ReplyTo::RemoteClaudeSessions(cwd), reply) => {
                let sessions = match reply {
                    Ok(Msg::ClaudeSessions { sessions, .. }) => sessions,
                    Ok(_) => Vec::new(),
                    Err(e) => {
                        self.notify(format!("could not list remote claude sessions: {e}"));
                        Vec::new()
                    }
                };
                if let Some(w) = self.wizard_mut() {
                    let outcome = w.set_claude_sessions(&cwd, sessions);
                    self.wizard_outcome(outcome);
                }
            }
            // Typing a path that doesn't exist (yet) is not an error.
            (ReplyTo::DirEntries | ReplyTo::RemoteDirEntries, Err(_)) => {}
            (to, Err(e)) if self.connected => {
                let what = match to {
                    ReplyTo::Ack(what) => what,
                    ReplyTo::Projects | ReplyTo::RemoteProjects => "recent projects",
                    _ => "request",
                };
                self.notify(format!("{what} failed: {e}"));
            }
            _ => {}
        }
    }

    /// Advance animations, the clock and notice timeouts.
    pub fn on_tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
        self.now = unix_now();
        if let Some(n) = &mut self.notice {
            n.ticks_left = n.ticks_left.saturating_sub(1);
            if n.ticks_left == 0 {
                self.notice = None;
            }
        }
        self.redraw = true;
    }
}

/// Load proxy profiles from config.toml and CLAUDIO_PROXY_URL. Returns
/// `(profile_names, default_name)`. Never panics; returns empty on any error.
fn load_proxy_profiles() -> (Vec<String>, Option<String>) {
    // Check env var first — when set it overrides any saved default.
    if let Some((name, _)) = crate::proxy::profile::from_env() {
        // Return just the "env" profile, which is always the default.
        return (vec![name.to_owned()], Some(name.to_owned()));
    }
    match crate::proxy::profile::load() {
        Ok(sec) => {
            let names: Vec<String> = sec.profiles.keys().cloned().collect();
            (names, sec.default)
        }
        Err(_) => (Vec::new(), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        app.recover(&ClientState::default(), live);
        app.take_effects();
        app
    }

    fn requests(effects: &[Effect]) -> Vec<&Msg> {
        effects.iter().filter_map(|e| if let Effect::Request { msg: m, .. } = e { Some(m) } else { None }).collect()
    }

    #[test]
    fn recovery_respawns_dormant_sessions_and_attaches_the_saved_active() {
        let live = [info(Some(1), None), info(None, Some("c2"))];
        let saved = ClientState { active: Some(live[1].id), ..Default::default() };
        let mut app = App::new(100, 30, "/home/u".into(), vec![]);
        app.recover(&saved, &live);
        let effects = app.take_effects();
        let reqs = requests(&effects);
        let Msg::Spawn(spec) = reqs[0] else { panic!("expected Spawn, got {reqs:?}") };
        assert_eq!((spec.id, spec.rows), (live[1].id, 28));
        assert_eq!(spec.args, ["--resume", "c2"]);
        assert_eq!(*reqs[1], Msg::Attach { id: live[1].id, rows: 28, cols: 100 });
        assert!(matches!(effects.last(), Some(Effect::Save)));
        assert_eq!(app.active, Some(1));
        assert!(app.modal.is_none());
    }


    #[test]
    fn no_sessions_opens_the_wizard() {
        let mut app = App::new(100, 30, "/home/u".into(), vec![]);
        app.recover(&ClientState::default(), &[]);
        assert!(matches!(app.modal, Some(Modal::Wizard(_))));
        assert!(requests(&app.take_effects()).contains(&&Msg::RecentProjects { limit: PROJECTS_LIMIT }));
    }

    #[test]
    fn switching_detaches_the_old_session_then_attaches_the_new() {
        let live = [info(Some(1), None), info(Some(2), None)];
        let mut app = app_with(&live);
        app.on_terminal(key(KeyCode::Left, KeyModifiers::ALT));
        let effects = app.take_effects();
        assert_eq!(
            requests(&effects),
            vec![&Msg::Detach { id: live[0].id }, &Msg::Attach { id: live[1].id, rows: 28, cols: 100 }]
        );
        assert_eq!(app.active, Some(1), "wraps around");
        assert!(!app.sessions[0].attached && app.sessions[1].attached);
        // Attached resets the mirror, but only for the attached session.
        app.on_incoming(Incoming::Attached { id: live[1].id, rows: 10, cols: 40 });
        assert_eq!(app.sessions[1].mirror.size(), (10, 40));
        app.on_incoming(Incoming::Attached { id: live[0].id, rows: 5, cols: 5 });
        assert_eq!(app.sessions[0].mirror.size(), (28, 100));
    }

    #[test]
    fn keys_go_to_the_active_session_unless_a_modal_is_open() {
        let live = [info(Some(1), None)];
        let mut app = app_with(&live);
        app.on_terminal(plain(KeyCode::Char('h')));
        assert!(matches!(&app.take_effects()[..], [Effect::Input(id, b)] if *id == live[0].id && b == b"h"));
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
        assert!(matches!(app.take_effects()[..], [Effect::Save]));
        app.on_terminal(alt('r'));
        app.on_terminal(key(KeyCode::Char('u'), KeyModifiers::CONTROL));
        app.on_terminal(plain(KeyCode::Enter));
        assert_eq!(app.sessions[0].name, None);
        assert_eq!(app.to_state().sessions[0].name, None);
    }

    #[test]
    fn close_persists_intent_before_kill_and_moves_to_a_neighbour() {
        let live = [info(Some(1), None), info(Some(2), None), info(Some(3), None)];
        let mut app = app_with(&live);
        app.on_terminal(alt('x'));
        app.on_terminal(plain(KeyCode::Char('n')));
        assert!(app.modal.is_none() && app.sessions.len() == 3, "n cancels");
        app.on_terminal(alt('x'));
        app.on_terminal(plain(KeyCode::Char('y')));
        let effects = app.take_effects();
        let save = effects.iter().position(|e| matches!(e, Effect::Save)).unwrap();
        let kill = effects
            .iter()
            .position(|e| matches!(e, Effect::Request { msg: Msg::Kill { id }, .. } if *id == live[0].id))
            .unwrap();
        assert!(save < kill);
        assert_eq!(app.sessions.len(), 2);
        assert_eq!(app.active_view().map(|v| v.id), Some(live[1].id));
        assert!(!app.to_state().sessions.iter().any(|s| s.id == live[0].id));
    }

    #[test]
    fn attention_cycles_over_sessions_that_want_it() {
        let mut live = [info(Some(1), None), info(Some(2), None), info(Some(3), None)];
        live[2].state = SessionState::NeedsApproval;
        let mut app = app_with(&live);
        app.on_terminal(alt('a'));
        assert_eq!(app.active, Some(2));
        app.on_terminal(alt('a'));
        assert_eq!(app.active, Some(2), "nothing else wants attention");
        let event = SessionEvent::State { state: SessionState::NeedsInput };
        app.on_incoming(Incoming::Event { id: live[0].id, event });
        app.on_terminal(alt('a'));
        assert_eq!(app.active, Some(0));
    }

    #[test]
    fn wizard_spawns_into_the_chosen_dir_and_records_it() {
        let mut app = app_with(&[]);
        // Step 0: paste the target directory, then press Enter to confirm "local" host.
        app.on_terminal(Event::Paste("/w".into()));
        app.on_terminal(plain(KeyCode::Enter)); // selects local → advances to directory step
        app.take_effects(); // consume ListDir + RecentProjects requests
        // Step 1: press Enter again to confirm the pre-filled directory "/w".
        app.on_terminal(plain(KeyCode::Enter));
        let effects = app.take_effects();
        assert!(requests(&effects).contains(&&Msg::ListClaudeSessions { cwd: "/w".into() }));
        let reply = Msg::ClaudeSessions { cwd: "/w".into(), sessions: vec![] };
        app.on_reply(ReplyTo::ClaudeSessions("/w".into()), Ok(reply));
        let effects = app.take_effects();
        let reqs = requests(&effects);
        let Msg::Spawn(spec) = reqs[0] else { panic!("expected Spawn, got {reqs:?}") };
        assert_eq!((spec.cwd.as_str(), spec.args.len()), ("/w", 0));
        assert_eq!(*reqs[1], Msg::Attach { id: spec.id, rows: 28, cols: 100 });
        assert!(app.modal.is_none());
        assert_eq!(app.recent_dirs, vec!["/w".to_owned()]);
        assert_eq!(app.sessions[0].label(), "w");
    }

    #[test]
    fn events_update_and_add_sessions() {
        let live = [info(Some(1), None)];
        let mut app = app_with(&live);
        let id = live[0].id;
        app.on_incoming(Incoming::Event { id, event: SessionEvent::Title { title: "Refactor".into() } });
        let event = SessionEvent::ClaudeSession { claude_session_id: "c9".into() };
        app.on_incoming(Incoming::Event { id, event });
        assert_eq!(app.sessions[0].label(), "Refactor");
        assert_eq!(app.to_state().sessions[0].claude_session_id.as_deref(), Some("c9"));
        let other = info(Some(5), None);
        app.on_incoming(Incoming::Event { id: other.id, event: SessionEvent::Created { info: other.clone() } });
        assert_eq!(app.sessions.len(), 2);
        app.on_incoming(Incoming::Event { id: other.id, event: SessionEvent::Removed });
        assert_eq!(app.sessions.len(), 1);
        assert_eq!(app.active, Some(0));
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
        assert!(matches!(app.modal, Some(Modal::Overview { selected: 1 })));
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
        let ev = SessionEvent::State { state: SessionState::NeedsInput };
        app.on_incoming(Incoming::Event { id: live[1].id, event: ev });
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
        assert!(app.pending_notifs.is_empty(), "active session must not notify");
    }

    #[test]
    fn notifications_disabled_when_flag_is_off() {
        let mut live = [info(Some(1), None), info(Some(2), None)];
        live[1].state = SessionState::NeedsApproval;
        let mut app = App::new_with_config(100, 30, "/home/u".into(), vec![], false);
        app.recover(&ClientState::default(), &live);
        app.take_effects();
        app.check_notifications();
        assert!(app.pending_notifs.is_empty(), "notifications disabled");
    }
}
