//! The custom env menu (Alt+,): the env vars every claude session gets
//! (Global, `config.toml [env]`) and the focused claude session's own
//! (`session-env.json`), applied at the session's next start or restart.
//! Values are shown and edited in the escaped form of
//! `claude::env::escape` (`\n`, `\t`, `\\`).
//!
//! Pure, like `app.rs`: every read and write is an [`Effect::Env`] the event
//! loop runs off the UI thread (`env_edge.rs`); what it found comes back
//! through [`App::on_env`], and the menu shows it. A session's entry is
//! forgotten when its tab goes and pruned when its host lists its sessions.
//! Drawn by `ui/env.rs`.

use std::fmt;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::app::{App, Effect};
use super::interaction::Modal;
use claudio_core::app::env::{Listed, Scope};
use claudio_core::claude::env::{
    catalogue, check_name, check_value, escape, shown, unescape, KnownVar, Value, MAX_VALUE,
};
use claudio_core::proto::{SessionId, SessionKind};

/// The longest text an input holds, in bytes: a value's escaped form.
const INPUT_MAX: usize = 2 * MAX_VALUE;
/// How far PageUp/PageDown move.
const PAGE: usize = 10;
/// The value prompt's reminder of the escapes.
pub const ESCAPES: &str = "\\n newline, \\t tab, \\\\ backslash";

/// A read or a write of the custom env, for the event loop (`env_edge`).
#[derive(Debug, Clone, PartialEq)]
pub enum EnvCall {
    /// Read both scopes for the menu opened under `gen` (the session scope
    /// of `session`, when it has one).
    Read { gen: u64, session: Option<Scope> },
    /// Set `name` to `value` (`None`: unset) in `scope`, then read again.
    Write {
        gen: u64,
        scope: Scope,
        name: String,
        value: Option<Value>,
        session: Option<Scope>,
    },
    /// Forget the entry of a session whose tab went.
    Forget { host: String, sid: SessionId },
    /// Forget `host`'s entries whose session is not in `live`.
    Prune { host: String, live: Vec<SessionId> },
}

/// What a read or a write found, for the menu opened under `gen`.
#[derive(Debug, Clone, PartialEq)]
pub struct EnvDone {
    pub gen: u64,
    pub global: Listed,
    pub session: Option<Listed>,
    /// Why a write failed (a key at most, never a value).
    pub failed: Option<String>,
}

/// The menu's two tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Global,
    Session,
}

/// One line of input. Its `Debug` hides the text (a value is a secret).
#[derive(Clone, PartialEq, Default)]
pub struct LineInput {
    pub text: String,
    /// A byte offset into `text`, on a char boundary.
    pub cursor: usize,
}

impl fmt::Debug for LineInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "LineInput(<{} bytes>)", self.text.len())
    }
}

impl LineInput {
    fn with(text: String) -> LineInput {
        let cursor = text.len();
        LineInput { text, cursor }
    }

    /// Insert `text` at the cursor, up to [`INPUT_MAX`] bytes; control
    /// characters are left out.
    fn insert(&mut self, text: &str) {
        for c in text.chars().filter(|c| !c.is_control()) {
            if self.text.len() + c.len_utf8() > INPUT_MAX {
                break;
            }
            self.text.insert(self.cursor, c);
            self.cursor += c.len_utf8();
        }
    }

    /// An editing key; `false` when it is not one.
    fn on_key(&mut self, key: &KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('u') if ctrl => *self = LineInput::default(),
            KeyCode::Char(c) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => {
                self.insert(&c.to_string())
            }
            KeyCode::Backspace => {
                if let Some(c) = self.text[..self.cursor].chars().next_back() {
                    self.cursor -= c.len_utf8();
                    self.text.remove(self.cursor);
                }
            }
            KeyCode::Delete if self.cursor < self.text.len() => {
                self.text.remove(self.cursor);
            }
            KeyCode::Left => {
                if let Some(c) = self.text[..self.cursor].chars().next_back() {
                    self.cursor -= c.len_utf8();
                }
            }
            KeyCode::Right => {
                if let Some(c) = self.text[self.cursor..].chars().next() {
                    self.cursor += c.len_utf8();
                }
            }
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.text.len(),
            _ => return false,
        }
        true
    }
}

/// What the menu is doing.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// The rows of the tab.
    List,
    /// Typing a new variable's name; `pick` is the highlighted catalogue
    /// match (`None`: the typed text).
    Name {
        input: LineInput,
        pick: Option<usize>,
    },
    /// Typing `name`'s value, escaped.
    Value { name: String, input: LineInput },
    /// Asking whether to delete `name`.
    Delete(String),
}

/// The menu's state.
#[derive(Debug, Clone, PartialEq)]
pub struct EnvView {
    /// The generation it was opened under: results of another are dropped.
    pub gen: u64,
    pub tab: Tab,
    /// The focused claude session's scope and label, or why the session
    /// tab is off.
    pub session: Result<(Scope, String), &'static str>,
    /// The global rows, once read.
    pub global: Option<Vec<(String, Value)>>,
    /// The session's rows, once read.
    pub own: Option<Vec<(String, Value)>>,
    /// An index into the tab's rows.
    pub selected: usize,
    pub step: Step,
    /// The last refusal or failed write, until the next key (a key at most,
    /// never a value).
    pub error: Option<String>,
}

/// What a key asks of `App`.
enum Outcome {
    Stay,
    Close,
    Write {
        scope: Scope,
        name: String,
        value: Option<Value>,
    },
}

impl EnvView {
    fn new(gen: u64, session: Result<(Scope, String), &'static str>) -> EnvView {
        EnvView {
            gen,
            tab: Tab::Global,
            session,
            global: None,
            own: None,
            selected: 0,
            step: Step::List,
            error: None,
        }
    }

    /// The session tab's scope, when there is one.
    pub fn session_scope(&self) -> Option<Scope> {
        self.session.as_ref().ok().map(|(scope, _)| scope.clone())
    }

    /// The open tab's scope.
    fn scope(&self) -> Option<Scope> {
        match self.tab {
            Tab::Global => Some(Scope::Global),
            Tab::Session => self.session_scope(),
        }
    }

    /// The open tab's rows (none until read).
    pub fn rows(&self) -> &[(String, Value)] {
        let rows = match self.tab {
            Tab::Global => &self.global,
            Tab::Session => &self.own,
        };
        rows.as_deref().unwrap_or(&[])
    }

    /// The name of the selected row.
    pub fn selected_name(&self) -> Option<&str> {
        self.rows().get(self.selected).map(|(k, _)| k.as_str())
    }

    /// The catalogue entries whose name holds `typed` (any case), those
    /// starting with it first.
    pub fn matches(typed: &str) -> Vec<&'static KnownVar> {
        let typed = typed.trim().to_ascii_uppercase();
        let (mut first, mut then) = (Vec::new(), Vec::new());
        for var in catalogue() {
            let name = var.name.to_ascii_uppercase();
            if name.starts_with(&typed) {
                first.push(var);
            } else if name.contains(&typed) {
                then.push(var);
            }
        }
        first.extend(then);
        first
    }

    fn on_key(&mut self, key: &KeyEvent) -> Outcome {
        self.error = None;
        match std::mem::replace(&mut self.step, Step::List) {
            Step::List => self.list_key(key),
            Step::Name { input, pick } => self.name_key(key, input, pick),
            Step::Value { name, input } => self.value_key(key, name, input),
            Step::Delete(name) => self.delete_key(key, name),
        }
    }

    fn list_key(&mut self, key: &KeyEvent) -> Outcome {
        let last = self.rows().len().saturating_sub(1);
        match key.code {
            KeyCode::Esc => return Outcome::Close,
            KeyCode::Tab | KeyCode::BackTab => match (&self.session, self.tab) {
                (Ok(_), Tab::Global) => self.tab = Tab::Session,
                (_, Tab::Session) => self.tab = Tab::Global,
                (Err(why), Tab::Global) => self.error = Some(format!("no session tab: {why}")),
            },
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down => self.selected = (self.selected + 1).min(last),
            KeyCode::PageUp => self.selected = self.selected.saturating_sub(PAGE),
            KeyCode::PageDown => self.selected = (self.selected + PAGE).min(last),
            KeyCode::Home => self.selected = 0,
            KeyCode::End => self.selected = last,
            KeyCode::Char('a') => {
                self.step = Step::Name {
                    input: LineInput::default(),
                    pick: None,
                };
            }
            KeyCode::Enter => {
                if let Some((name, value)) = self.rows().get(self.selected).cloned() {
                    let input = LineInput::with(escape(value.as_str()));
                    self.step = Step::Value { name, input };
                }
            }
            KeyCode::Char('d') | KeyCode::Delete => {
                if let Some(name) = self.selected_name().map(str::to_owned) {
                    self.step = Step::Delete(name);
                }
            }
            _ => {}
        }
        self.selected = self.selected.min(self.rows().len().saturating_sub(1));
        Outcome::Stay
    }

    fn name_key(&mut self, key: &KeyEvent, mut input: LineInput, pick: Option<usize>) -> Outcome {
        let count = EnvView::matches(&input.text).len();
        let pick = match key.code {
            KeyCode::Esc => return Outcome::Stay,
            KeyCode::Down if count > 0 => Some(pick.map_or(0, |p| (p + 1).min(count - 1))),
            KeyCode::Up => pick.and_then(|p| p.checked_sub(1)),
            KeyCode::Enter => {
                let name = match pick.and_then(|p| EnvView::matches(&input.text).get(p).copied()) {
                    Some(var) => var.name.to_owned(),
                    None => input.text.trim().to_owned(),
                };
                if let Err(why) = check_name(&name) {
                    self.error = Some(shown(&name, &why));
                    self.step = Step::Name { input, pick };
                    return Outcome::Stay;
                }
                let old = self.rows().iter().find(|(k, _)| *k == name);
                let text = old.map(|(_, v)| escape(v.as_str())).unwrap_or_default();
                let input = LineInput::with(text);
                self.step = Step::Value { name, input };
                return Outcome::Stay;
            }
            _ if input.on_key(key) => None,
            _ => pick,
        };
        self.step = Step::Name { input, pick };
        Outcome::Stay
    }

    fn value_key(&mut self, key: &KeyEvent, name: String, mut input: LineInput) -> Outcome {
        match key.code {
            KeyCode::Esc => return Outcome::Stay,
            KeyCode::Enter => {
                let checked = unescape(&input.text)
                    .map_err(|why| format!("{name}: {why}"))
                    .and_then(|v| check_value(&v).map(|()| v).map_err(|e| shown(&name, &e)));
                match (checked, self.scope()) {
                    (Ok(value), Some(scope)) => {
                        return Outcome::Write {
                            scope,
                            name,
                            value: Some(Value::new(value)),
                        }
                    }
                    (Ok(_), None) => {}
                    (Err(why), _) => self.error = Some(why),
                }
            }
            _ => {
                input.on_key(key);
            }
        }
        self.step = Step::Value { name, input };
        Outcome::Stay
    }

    fn delete_key(&mut self, key: &KeyEvent, name: String) -> Outcome {
        match (key.code, self.scope()) {
            (KeyCode::Char('y') | KeyCode::Enter, Some(scope)) => Outcome::Write {
                scope,
                name,
                value: None,
            },
            (KeyCode::Char('n') | KeyCode::Esc, _) => Outcome::Stay,
            _ => {
                self.step = Step::Delete(name);
                Outcome::Stay
            }
        }
    }

    /// A paste: the name's first line, or the value's text escaped (`\r\n`
    /// and a lone `\r`, which most terminals paste for a line break, as a
    /// newline).
    pub fn on_paste(&mut self, text: &str) {
        match &mut self.step {
            Step::Name { input, pick } => {
                input.insert(text.lines().next().unwrap_or("").trim());
                *pick = None;
            }
            Step::Value { input, .. } => {
                let text = text.replace("\r\n", "\n").replace('\r', "\n");
                input.insert(&escape(&text));
            }
            Step::List | Step::Delete(_) => {}
        }
    }
}

impl App {
    /// Alt+,: the custom env menu, on the global tab; the session tab is
    /// the focused claude session's. Its rows come with [`App::on_env`].
    pub(super) fn open_environment(&mut self) {
        self.modal_gen += 1;
        let gen = self.modal_gen;
        let session = match self.active_view() {
            // Not started yet: its daemon does not list it, so a prune
            // would drop what is set now.
            Some(v) if v.kind == SessionKind::Claude && self.launching.contains(&v.id) => {
                Err("the session is still starting")
            }
            Some(v) if v.kind == SessionKind::Claude => {
                let scope = Scope::Session {
                    host: v.host.clone(),
                    sid: v.id,
                };
                Ok((scope, v.label()))
            }
            Some(_) => Err("a terminal tab runs no claude"),
            None => Err("no session is open"),
        };
        let view = EnvView::new(gen, session);
        let read = EnvCall::Read {
            gen,
            session: view.session_scope(),
        };
        self.modal = Some(Modal::Environment(Box::new(view)));
        self.emit(Effect::Env(read));
        self.redraw = true;
    }

    /// A key in the custom env menu.
    pub(super) fn env_key(&mut self, key: &KeyEvent) {
        let Some(Modal::Environment(view)) = &mut self.modal else {
            return;
        };
        match view.on_key(key) {
            Outcome::Stay => {}
            Outcome::Close => self.modal = None,
            Outcome::Write { scope, name, value } => {
                let (gen, session) = (view.gen, view.session_scope());
                let call = EnvCall::Write {
                    gen,
                    scope,
                    name,
                    value,
                    session,
                };
                self.emit(Effect::Env(call));
            }
        }
        self.redraw = true;
    }

    /// What a read or a write of the custom env found: the menu opened
    /// under `done.gen` shows it; a failed write and the reads' notices
    /// (key names only) become notices.
    pub fn on_env(&mut self, done: EnvDone) {
        if let Some(why) = &done.failed {
            self.notify(why.clone());
        }
        let Some(Modal::Environment(view)) = &mut self.modal else {
            return;
        };
        if view.gen != done.gen {
            return;
        }
        let mut notices = done.global.notices;
        view.global = Some(done.global.vars);
        if let Some(own) = done.session {
            notices.extend(own.notices);
            view.own = Some(own.vars);
        }
        view.error = done.failed.or(view.error.take());
        view.selected = view.selected.min(view.rows().len().saturating_sub(1));
        for notice in notices {
            self.notify(notice);
        }
        self.redraw = true;
    }

    /// The tab of session `sid` on `host` went: its custom env goes too.
    pub(super) fn forget_env(&mut self, host: &str, sid: SessionId) {
        let host = host.to_owned();
        self.emit(Effect::Env(EnvCall::Forget { host, sid }));
    }

    /// `host` listed its sessions (`live`): the custom env of its others
    /// goes.
    pub(super) fn prune_env(&mut self, host: &str, live: Vec<SessionId>) {
        let host = host.to_owned();
        self.emit(Effect::Env(EnvCall::Prune { host, live }));
    }

    /// The menu's footer: when its changes apply.
    pub fn env_footer(&self) -> String {
        let restart = self.keymap.key_for(super::keymap::Action::Reset);
        let restart = restart.unwrap_or_else(|| "Alt+e".to_owned());
        format!("applies on next start or restart ({restart}); a running claude keeps its env")
    }
}

#[cfg(test)]
mod tests;
