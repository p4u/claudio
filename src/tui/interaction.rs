//! Modal popup types and terminal input / wizard routing for [`App`].
//!
//! The `impl App` block here handles all keyboard, paste, mouse, focus and
//! resize events, as well as wizard state transitions and host connection
//! callbacks. It can access `App::effects` because that field is `pub(super)`.

use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use crate::proto::Msg;
use crate::term::keys::{encode_focus, encode_key, encode_mouse, encode_paste};

use super::app::{App, Effect, ReplyTo, PROJECTS_LIMIT};
use super::keymap::Action;
use super::state::KillTombstone;
use super::ui;
use super::wizard::{Outcome, Wizard};

// ── Modal ─────────────────────────────────────────────────────────────────────

use crate::proto::SessionId;

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
    /// Proxy stats popup.
    ProxyStats {
        profile_name: String,
    },
    /// Overview / "mission control": all sessions at a glance.
    Overview {
        selected: usize,
    },
    /// Help popup: all key bindings.
    Help,
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
            Action::NewSession => self.open_wizard(),
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
            _ => {}
        }
        self.redraw = true;
    }

    fn open_overview(&mut self) {
        let selected = self.active.unwrap_or(0);
        self.modal = Some(Modal::Overview { selected });
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
                        // M2: Add tombstone to killed list BEFORE sending Kill.
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
            Modal::Help => self.modal = None,
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
                    if let Some(Modal::Wizard(w)) = &mut self.modal {
                        w.on_host_connected("local", &self.home);
                    }
                    self.request_local(
                        Msg::RecentProjects {
                            limit: PROJECTS_LIMIT,
                        },
                        ReplyTo::Projects,
                    );
                } else {
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
                    ReplyTo::ClaudeSessions(cwd.clone(), wizard_gen)
                } else {
                    ReplyTo::RemoteClaudeSessions(cwd.clone(), wizard_gen)
                };
                self.request(&wizard_host, Msg::ListClaudeSessions { cwd }, reply_to)
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
        if let Some(Modal::Wizard(w)) = &mut self.modal {
            w.on_host_connected(host, home);
            let h = host.to_owned();
            self.effects.push(Effect::Request {
                host: h,
                msg: Msg::RecentProjects {
                    limit: PROJECTS_LIMIT,
                },
                to: ReplyTo::RemoteProjects,
            });
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
