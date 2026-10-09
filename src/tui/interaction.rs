//! Modal popup types and terminal input / wizard routing for [`App`].
//!
//! The `impl App` block here handles all keyboard, paste, mouse, focus and
//! resize events, as well as wizard state transitions and host connection
//! callbacks. It can access `App::effects` because that field is `pub(super)`.

use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use std::io;

use crate::client::Incoming;
use crate::proto::{Msg, SessionEvent, SessionId, SessionKind, SessionState};
use crate::term::keys::{encode_focus, encode_key, encode_mouse, encode_paste};
use crate::term::screen::Screen;

use super::app::{App, Effect, Mode, ReplyTo};
use super::confirm::ConfirmPrompt;
use super::git_view::GitView;
use super::keymap::Action;
use super::sessions::SessionView;
use super::state::KillTombstone;
use super::stats_view::StatsView;
use super::ui;
use super::wizard::{Outcome, Wizard};

// ── Modal ─────────────────────────────────────────────────────────────────────

/// A popup that captures the keyboard.
pub enum Modal {
    Rename {
        id: SessionId,
        input: String,
    },
    /// Confirm killing a session.
    Close {
        id: SessionId,
    },
    Wizard(Wizard),
    /// Proxy stats popup (see `stats_view.rs`).
    ProxyStats(StatsView),
    /// Overview / "mission control": all sessions at a glance.
    Overview {
        selected: usize,
        /// Live filter string; empty = show all.
        filter: String,
    },
    /// Help popup: all key bindings.
    Help,
    /// A yes / no / skip question (see [`super::confirm`]).
    Confirm(ConfirmPrompt),
    /// The commit-history viewer (full pane).
    Git(Box<GitView>),
}

impl Modal {
    /// Read-only views give way to a manager action: its key closes the view
    /// and runs the action. Text-input modals swallow it instead.
    pub fn yields_to_actions(&self) -> bool {
        matches!(
            self,
            Modal::Git(_) | Modal::Overview { .. } | Modal::Help | Modal::ProxyStats { .. }
        )
    }

    /// Whether `action` is the one that opens this modal; pressing its key
    /// again closes the modal instead of reopening it.
    fn opened_by(&self, action: Action) -> bool {
        matches!(
            (self, action),
            (Modal::Git(_), Action::GitLog)
                | (Modal::Overview { .. }, Action::Overview)
                | (Modal::Help, Action::Help)
                | (Modal::ProxyStats { .. }, Action::ProxyStats)
        )
    }

    /// Whether the modal takes mouse wheel and clicks.
    fn wants_mouse(&self) -> bool {
        matches!(self, Modal::Git(_))
    }
}

// ── Terminal input (impl App) ─────────────────────────────────────────────────

impl App {
    pub fn on_terminal(&mut self, ev: crossterm::event::Event) {
        match ev {
            crossterm::event::Event::Key(key) => self.on_key(key),
            crossterm::event::Event::Paste(text) => self.on_paste(&text),
            crossterm::event::Event::Mouse(m) => self.on_mouse(m),
            crossterm::event::Event::FocusGained => self.on_focus(true),
            crossterm::event::Event::FocusLost => self.on_focus(false),
            crossterm::event::Event::Resize(w, h) => self.on_resize(w, h),
        }
    }

    fn on_key(&mut self, key: KeyEvent) {
        let action = self.keymap.lookup(&key);
        if action == Some(Action::Quit) {
            self.on_action(Action::Quit);
            return;
        }
        if let Some(modal) = &self.modal {
            match action.filter(|_| modal.yields_to_actions()) {
                Some(action) => {
                    let toggled_off = modal.opened_by(action);
                    self.modal = None;
                    if toggled_off {
                        self.redraw = true;
                    } else {
                        self.on_action(action);
                    }
                }
                None => self.modal_key(key),
            }
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
                let next = if action == Action::NextSession {
                    (cur + 1) % n
                } else {
                    (cur + n - 1) % n
                };
                self.activate(next);
            }
            Action::NextAttention if n > 0 => {
                let cur = self.active.unwrap_or(0);
                let found = (1..n)
                    .map(|k| (cur + k) % n)
                    .find(|&i| self.sessions[i].state.wants_attention());
                match found {
                    Some(i) => self.activate(i),
                    None => self.notify("no session needs attention"),
                }
            }
            Action::GotoSession(digit) => {
                // Tabs are numbered from 1; 0 is tab 10.
                let tab = if digit == 0 { 10 } else { usize::from(digit) };
                if tab <= n {
                    self.activate(tab - 1);
                } else {
                    self.notify(format!("no session {tab}"));
                }
            }
            Action::NewSession => self.open_wizard(),
            Action::Terminal => self.open_terminal(),
            Action::Reset => self.ask_reset(),
            Action::Rename => {
                if let Some(v) = self.active_view() {
                    self.modal = Some(Modal::Rename {
                        id: v.id,
                        input: v.label(),
                    });
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
            Action::GitLog => self.open_git_log(),
            _ => {}
        }
        self.redraw = true;
    }

    fn open_overview(&mut self) {
        let selected = self.active.unwrap_or(0);
        self.modal = Some(Modal::Overview { selected, filter: String::new() });
        self.redraw = true;
    }

    fn open_help(&mut self) {
        self.modal = Some(Modal::Help);
        self.redraw = true;
    }

    pub fn modal_key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        self.redraw = true;
        let rows = self.pane_size().0 as usize;
        let Some(modal) = &mut self.modal else { return };
        match modal {
            Modal::Wizard(w) => {
                let outcome = if self.keymap.lookup_wizard(&key) == Some(Action::ToggleHidden) {
                    w.toggle_hidden()
                } else {
                    w.on_key(&key)
                };
                self.wizard_outcome(outcome);
            }
            Modal::Rename { id, input } => match key.code {
                KeyCode::Esc => self.modal = None,
                KeyCode::Enter => {
                    let (id, name) = (*id, input.trim().to_owned());
                    let name = (!name.is_empty()).then_some(name);
                    self.modal = None;
                    if let Some(i) = self.index_of(id) {
                        let host = self.sessions[i].host.clone();
                        self.sessions[i].name = name.clone();
                        // Persist to daemon journal so the name survives reconnects.
                        self.request(
                            &host,
                            Msg::Rename { id, name },
                            ReplyTo::Ack("rename"),
                        );
                        self.save();
                    }
                }
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Char('u') if key.modifiers == KeyModifiers::CONTROL => input.clear(),
                KeyCode::Char(c) if key.modifiers.difference(KeyModifiers::SHIFT).is_empty() => {
                    input.push(c)
                }
                _ => {}
            },
            Modal::Close { id } => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    let id = *id;
                    self.modal = None;
                    if let Some(i) = self.index_of(id) {
                        let host = self.sessions[i].host.clone();
                        // The tombstone goes in before the Kill is sent.
                        self.killed.push(KillTombstone {
                            host: host.clone(),
                            id,
                        });
                        // Remove from session list (also queues Save via remove()).
                        self.remove(i);
                        // Save includes the tombstone since to_state() includes killed.
                        self.save();
                        // Kill is queued AFTER Save in the effects list, so the
                        // tombstone is durably persisted before Kill reaches the daemon.
                        self.request(&host, Msg::Kill { id }, ReplyTo::Kill(id));
                    }
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => self.modal = None,
                _ => {}
            },
            Modal::ProxyStats(_) => self.stats_key(key),
            Modal::Overview { selected, filter } => {
                let plain = key.modifiers.difference(KeyModifiers::SHIFT).is_empty();
                match key.code {
                    KeyCode::Esc => {
                        if filter.is_empty() {
                            self.modal = None;
                        } else {
                            *filter = String::new();
                            *selected = 0;
                        }
                    }
                    KeyCode::Backspace => {
                        filter.pop();
                        *selected = 0;
                    }
                    KeyCode::Char('u') if key.modifiers == KeyModifiers::CONTROL => {
                        filter.clear();
                        *selected = 0;
                    }
                    KeyCode::Up if *selected > 0 => *selected -= 1,
                    KeyCode::Down => {
                        // Max is the filtered session count.
                        let f = filter.clone();
                        let matches = self.sessions.iter()
                            .filter(|v| overview_matches(v, &f))
                            .count();
                        if *selected + 1 < matches {
                            *selected += 1;
                        }
                    }
                    KeyCode::Enter => {
                        let f = filter.clone();
                        let sel = *selected;
                        // Find the actual index in all sessions.
                        let real_idx = self.sessions.iter().enumerate()
                            .filter(|(_, v)| overview_matches(v, &f))
                            .nth(sel)
                            .map(|(i, _)| i);
                        self.modal = None;
                        if let Some(i) = real_idx {
                            self.activate(i);
                        }
                    }
                    KeyCode::Char(c) if plain => {
                        filter.push(c);
                        *selected = 0;
                    }
                    _ => {}
                }
            }
            Modal::Help => self.modal = None,
            Modal::Confirm(_) => self.confirm_key(key),
            Modal::Git(view) => {
                let outcome = view.on_key(&key, rows);
                self.git_outcome(outcome);
            }
        }
    }

    /// A mouse event while a modal that wants the mouse is open.
    fn modal_mouse(&mut self, m: MouseEvent) {
        let rows = self.pane_size().0 as usize;
        let Some(Modal::Git(view)) = &mut self.modal else { return };
        // Pane rows start below the tab bar; the status bar is not the pane.
        let above = self.mode.rows_above() as usize;
        let Some(row) = (m.row as usize).checked_sub(above).filter(|&r| r < rows) else {
            return;
        };
        let outcome = view.on_mouse(m.kind, row, rows);
        self.git_outcome(outcome);
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
            Some(Modal::Git(view)) => {
                view.on_paste(text);
                self.redraw = true;
            }
            Some(Modal::Close { .. })
            | Some(Modal::ProxyStats(_))
            | Some(Modal::Overview { .. })
            | Some(Modal::Help)
            | Some(Modal::Confirm(_)) => {}
            None => {
                if let Some(v) = self.active_view().filter(|v| v.attached) {
                    let bytes = encode_paste(text, &v.mirror.modes());
                    self.effects.push(Effect::Input(v.id, bytes));
                }
            }
        }
    }

    fn on_mouse(&mut self, m: MouseEvent) {
        if let Some(modal) = &self.modal {
            if modal.wants_mouse() {
                self.modal_mouse(m);
            }
            return;
        }
        if self.mode == Mode::Manager && m.row == 0 {
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
            let origin = (0, self.mode.rows_above());
            if let Some(bytes) = encode_mouse(&m, origin, (cols, rows), &v.mirror.modes()) {
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
            v.mirror.resize(rows, cols);
            let id = v.id;
            let host = v.host.clone();
            self.request(
                &host,
                Msg::Resize { id, rows, cols },
                ReplyTo::Ack("resize"),
            );
        }
    }

    // ── Wizard routing ────────────────────────────────────────────────────────

    pub(super) fn wizard_outcome(&mut self, outcome: Outcome) {
        let wizard_host = self.wizard_host();
        let wizard_gen = self.wizard_generation();
        match outcome {
            Outcome::None => {}
            Outcome::Cancel => self.modal = None,
            Outcome::ConnectHost(host) => {
                if host == "local" {
                    // Compute seeds before mutably borrowing self.modal.
                    let host_recent =
                        super::state::recent_for_host(&self.recent_dirs, "local").to_vec();
                    let active_cwd_local: Option<String> = self.active_view().and_then(|v| {
                        if v.host == "local" {
                            Some(v.cwd.clone())
                        } else {
                            None
                        }
                    });
                    let new_seeds = super::wizard::assemble(
                        active_cwd_local.as_deref(),
                        &host_recent,
                        &[],
                    );
                    let local_home = self.home.clone();
                    let list_home = match &mut self.modal {
                        Some(Modal::Wizard(w)) => {
                            w.on_host_connected("local", &local_home, new_seeds);
                            w.browse_home()
                        }
                        _ => Outcome::None,
                    };
                    self.wizard_outcome(list_home);
                    self.request_projects("local");
                } else {
                    self.effects.push(Effect::Connect(host));
                }
            }
            Outcome::ListDir(path) => {
                self.request(&wizard_host, Msg::ListDir { path }, ReplyTo::DirEntries)
            }
            Outcome::ChooseDir(cwd) => {
                // If ChooseDir comes from the first screen's LOCAL section,
                // the wizard is still in host_step. Transition to directory
                // step (local host) before recording pending.
                let in_host_step = matches!(&self.modal,
                    Some(Modal::Wizard(w)) if w.host_step.is_some());
                if in_host_step {
                    // Compute seeds before mutable borrow.
                    let host_recent =
                        super::state::recent_for_host(&self.recent_dirs, "local").to_vec();
                    let active_cwd_local = self.active_view()
                        .filter(|v| v.host == "local")
                        .map(|v| v.cwd.clone());
                    let seeds = super::wizard::assemble(
                        active_cwd_local.as_deref(),
                        &host_recent,
                        &[],
                    );
                    let local_home = self.home.clone();
                    let pending_dir = cwd.clone();
                    if let Some(Modal::Wizard(w)) = &mut self.modal {
                        w.on_host_connected("local", &local_home, seeds);
                        w.pending = Some(pending_dir);
                    }
                    self.request_projects("local");
                }
                let to = ReplyTo::ClaudeSessions {
                    host: wizard_host.clone(),
                    cwd: cwd.clone(),
                    gen: wizard_gen,
                };
                self.request(&wizard_host, Msg::ListClaudeSessions { cwd }, to)
            }
            Outcome::Spawn { cwd, resume, proxy } => {
                self.modal = None;
                if let Some(ref pname) = proxy {
                    self.maybe_fetch_proxy_config(pname);
                }
                self.spawn(wizard_host, cwd, resume, proxy);
            }
        }
        self.redraw = true;
    }

    pub fn on_host_connected(&mut self, host: &str, home: &str) {
        // Compute seeds before mutably borrowing self.modal.
        let host_recent = super::state::recent_for_host(&self.recent_dirs, host).to_vec();
        let active_cwd_for_host: Option<String> = self.active_view().and_then(|v| {
            if v.host == host {
                Some(v.cwd.clone())
            } else {
                None
            }
        });
        let new_seeds =
            super::wizard::assemble(active_cwd_for_host.as_deref(), &host_recent, &[]);

        if let Some(Modal::Wizard(w)) = &mut self.modal {
            w.on_host_connected(host, home, new_seeds);
            let list_home = w.browse_home();
            self.request_projects(host);
            self.wizard_outcome(list_home);
        }
        self.redraw = true;
    }

    pub fn on_host_error(&mut self, host: &str, error: &str) {
        if let Some(Modal::Wizard(w)) = &mut self.modal {
            if let Some(hs) = &mut w.host_step {
                hs.connecting = None;
            }
        }
        self.notify(format!("cannot connect to {host}: {error}"));
    }

    pub(super) fn wizard_mut(&mut self) -> Option<&mut Wizard> {
        match &mut self.modal {
            Some(Modal::Wizard(w)) => Some(w),
            _ => None,
        }
    }
}

// ── Daemon events (impl App) ──────────────────────────────────────────────────

impl App {
    /// Handle an incoming event from the daemon, tagged with the originating host.
    pub fn on_incoming_from(&mut self, host: &str, inc: Incoming) {
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
            Incoming::Event { id, event } => self.on_event(host, id, event),
            Incoming::HostStats { .. } => {
                // Handled in the event loop before on_incoming_from is called.
            }
            Incoming::Disconnected => {
                if host == "local" {
                    self.on_disconnected_local();
                } else {
                    self.on_disconnected_remote(host);
                }
            }
        }
    }

    fn on_event(&mut self, host: &str, id: SessionId, event: SessionEvent) {
        self.redraw = true;
        if self.mode == Mode::Plain && self.plain_event(id, &event) {
            return;
        }
        if let SessionEvent::Created { info } = &event {
            if self.index_of(id).is_none() {
                let view = SessionView::from_info(host, info, self.pane_size());
                self.sessions.push(view);
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
            // Our own closes are gone already; this is a clean exit or
            // another client's kill.
            SessionEvent::Removed => {
                let label = v.label();
                self.remove(i);
                self.notify(format!("{label}: session ended"));
            }
            SessionEvent::State { state } => v.state = state,
            SessionEvent::ClaudeSession { claude_session_id } => {
                v.claude_session_id = Some(claude_session_id);
                self.save();
                self.refresh_session_cred(super::proxy_state::SESSION_CRED_MIN_GAP_SECS);
            }
            // A fresh reset: recovery must not resume the old conversation
            // from what we saved.
            SessionEvent::ClaudeSessionCleared => {
                v.claude_session_id = None;
                self.save();
            }
            SessionEvent::Title { title } => v.title = Some(title),
            SessionEvent::Exited { .. } => v.state = SessionState::Exited,
            SessionEvent::Notice { text } => self.notify(text),
            SessionEvent::Renamed { name } => {
                v.name = name;
                self.save();
            }
            SessionEvent::Meta {
                branch,
                model,
                context_tokens,
            } => {
                if branch.is_some() {
                    v.branch = branch;
                }
                if model.is_some() {
                    v.model = model;
                }
                if context_tokens.is_some() {
                    v.context_tokens = context_tokens;
                }
            }
            SessionEvent::Unknown => {}
        }
    }

    /// A reply (or failure) for a request made through [`Effect::Request`].
    pub fn on_reply(&mut self, to: ReplyTo, reply: io::Result<Msg>) {
        self.redraw = true;
        match (to, reply) {
            // Delivered (a session that is already gone answers "no such
            // session"): the tombstone can go.
            (ReplyTo::Kill(id), Ok(_)) => {
                self.killed.retain(|t| t.id != id);
                self.save();
            }
            // The tombstone stays, so the next recovery sends the Kill again.
            (ReplyTo::Kill(_), Err(e)) => self.notify(format!("kill failed (will retry): {e}")),
            (ReplyTo::Projects { host, gen }, Ok(Msg::Projects { dirs })) => {
                let paths: Vec<String> = dirs.iter().map(|d| d.path.clone()).collect();
                if host == "local" {
                    // Seeds the next wizard at once.
                    self.projects = paths.clone();
                }
                if gen != self.wizard_generation() {
                    return;
                }
                if let Some(w) = self.wizard_mut() {
                    // Ignored unless the wizard is still on `host`.
                    w.add_seeds_for_host(&host, &paths);
                    w.add_project_meta(&dirs);
                }
            }
            (ReplyTo::DirEntries, Ok(Msg::DirEntries { path, entries, .. })) => {
                if let Some(w) = self.wizard_mut() {
                    w.set_dir_entries(&path, &entries);
                }
            }
            (ReplyTo::ClaudeSessions { host, cwd, gen }, reply) => {
                if gen != self.wizard_generation() {
                    return;
                }
                match reply {
                    Ok(Msg::ClaudeSessions { sessions, .. }) => {
                        if let Some(w) = self.wizard_mut() {
                            let outcome = w.set_claude_sessions(&cwd, sessions);
                            self.wizard_outcome(outcome);
                        }
                    }
                    Ok(_) => {}
                    // An error is not "no sessions": say so and let the user
                    // retry.
                    Err(e) => {
                        let place = if host == "local" { "" } else { "remote " };
                        self.notify(format!("could not list {place}claude sessions: {e}"));
                        if let Some(w) = self.wizard_mut() {
                            w.set_claude_sessions_error(&cwd);
                        }
                    }
                }
            }
            (ReplyTo::Git { gen, seq }, reply) => self.on_git_reply(gen, seq, reply),
            (ReplyTo::Respawned(id), reply) => self.on_respawned(id, reply),
            (ReplyTo::SpawnedDeferred(id), Ok(_)) => {
                // Re-attach the active view: its first Attach raced the Spawn.
                if let Some(i) = self.index_of(id).filter(|&i| self.active == Some(i)) {
                    self.sessions[i].attached = false;
                    self.activate(i);
                }
            }
            (ReplyTo::Spawned(_) | ReplyTo::SpawnedDeferred(_), Err(e))
                if self.mode == Mode::Plain =>
            {
                self.plain_failed(format!("could not start claude: {e}"));
            }
            (ReplyTo::Spawned(id) | ReplyTo::SpawnedDeferred(id), Err(e)) => {
                let Some(i) = self.index_of(id) else { return };
                let (kind, host) = (self.sessions[i].kind, self.sessions[i].host.clone());
                match (kind, App::older_daemon_notice(&host, &e)) {
                    // An old daemon cannot run terminals: no tab to keep.
                    (SessionKind::Shell, Some(notice)) => {
                        self.remove(i);
                        self.notify(notice);
                    }
                    (SessionKind::Shell, None) => {
                        self.sessions[i].state = SessionState::Exited;
                        self.notify(format!("could not start terminal: {e}"));
                    }
                    (SessionKind::Claude, _) => {
                        self.sessions[i].state = SessionState::Exited;
                        self.notify(format!("could not start claude: {e}"));
                    }
                }
            }
            (ReplyTo::ClaudeUpdate(host), reply) => self.on_claude_updated(&host, reply),
            // Typing a path that doesn't exist (yet) is not an error.
            (ReplyTo::DirEntries, Err(_)) => {}
            (to, Err(e)) if self.connected => {
                let what = match to {
                    ReplyTo::Ack(what) => what,
                    ReplyTo::Projects { .. } => "recent projects",
                    _ => "request",
                };
                self.notify(format!("{what} failed: {e}"));
            }
            _ => {}
        }
    }
}

// ── Overview filter helper ─────────────────────────────────────────────────────

/// Whether a session matches the overview filter string (case-insensitive
/// substring match against label, cwd, host, or state name).
pub fn overview_matches(v: &SessionView, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    let f = filter.to_lowercase();
    let label = v.label().to_lowercase();
    let cwd = v.cwd.to_lowercase();
    let host = v.host.to_lowercase();
    let state = v.state.name();
    label.contains(&f) || cwd.contains(&f) || host.contains(&f) || state.contains(&f)
}
