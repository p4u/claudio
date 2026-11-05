//! The leader key (Alt+z) and what extension actions show: the leader
//! popup, the one-line input an action may ask for, the list picker, the
//! document (a reader view of one markdown text) and pasting into the
//! active session. Pure, like `app.rs`: external actions run in the
//! extension host through [`Effect::RunAction`], their results come back
//! through [`App::on_action_result`].

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::app::{App, Effect};
use super::extensions::protocol::SessionContext;
use super::extensions::{ActionResult, ListItem, Source, MANAGE_KEY, READER_KEY};
use super::interaction::Modal;
use super::reader::ReaderView;
use crate::claude::latex::sanitize;
use crate::proto::{Answer, SessionId, SessionKind};

/// The longest input an action gets, in bytes.
pub const INPUT_MAX: usize = 512;
/// The most text a paste writes, in bytes.
pub const PASTE_MAX: usize = 64 * 1024;
/// How far PageUp/PageDown move in the picker.
const PAGE: usize = 10;

/// One line of the leader popup.
#[derive(Debug, Clone, PartialEq)]
pub struct LeaderEntry {
    pub key: char,
    pub title: String,
    /// The extension's name (`None`: the reader, which is not one).
    pub extension: Option<String>,
}

/// The one-line input an action asks for before it runs.
#[derive(Debug, Clone, PartialEq)]
pub struct Input {
    pub extension: String,
    pub action: String,
    pub prompt: String,
    pub text: String,
    /// The cursor, as a byte offset into `text` (on a char boundary).
    pub cursor: usize,
}

/// What a key did to an [`Input`].
enum Typed {
    Editing,
    Done,
    Cancelled,
}

impl Input {
    fn on_key(&mut self, key: &KeyEvent) -> Typed {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => return Typed::Cancelled,
            KeyCode::Enter => return Typed::Done,
            KeyCode::Char('u') if ctrl => {
                self.text.clear();
                self.cursor = 0;
            }
            KeyCode::Char(c) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => {
                if self.text.len() + c.len_utf8() <= INPUT_MAX && !c.is_control() {
                    self.text.insert(self.cursor, c);
                    self.cursor += c.len_utf8();
                }
            }
            KeyCode::Backspace => {
                if let Some(c) = self.text[..self.cursor].chars().next_back() {
                    self.cursor -= c.len_utf8();
                    self.text.remove(self.cursor);
                }
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
            _ => {}
        }
        Typed::Editing
    }
}

/// A list to pick from, with an always-on filter.
#[derive(Debug, Clone, PartialEq)]
pub struct Picker {
    pub title: String,
    pub items: Vec<ListItem>,
    pub filter: String,
    /// An index into [`Picker::shown`].
    pub selected: usize,
}

/// What a key did to a [`Picker`].
enum Picked {
    Browsing,
    Chose(ActionResult),
    Closed,
}

impl Picker {
    pub fn new(title: String, items: Vec<ListItem>) -> Picker {
        Picker {
            title,
            items,
            filter: String::new(),
            selected: 0,
        }
    }

    /// The items the filter keeps (a case-insensitive substring of the
    /// label or the detail), by index.
    pub fn shown(&self) -> Vec<usize> {
        let filter = self.filter.to_lowercase();
        (0..self.items.len())
            .filter(|&i| {
                let item = &self.items[i];
                item.label.to_lowercase().contains(&filter)
                    || item
                        .detail
                        .as_ref()
                        .is_some_and(|d| d.to_lowercase().contains(&filter))
            })
            .collect()
    }

    fn on_key(&mut self, key: &KeyEvent) -> Picked {
        let shown = self.shown();
        let last = shown.len().saturating_sub(1);
        match key.code {
            KeyCode::Esc => return Picked::Closed,
            KeyCode::Enter => {
                return match shown.get(self.selected) {
                    Some(&i) => Picked::Chose(self.items[i].result.clone()),
                    None => Picked::Browsing,
                }
            }
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down => self.selected = (self.selected + 1).min(last),
            KeyCode::PageUp => self.selected = self.selected.saturating_sub(PAGE),
            KeyCode::PageDown => self.selected = (self.selected + PAGE).min(last),
            KeyCode::Home => self.selected = 0,
            KeyCode::End => self.selected = last,
            KeyCode::Backspace => {
                self.filter.pop();
                self.selected = 0;
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                if self.filter.len() < INPUT_MAX {
                    self.filter.push(c);
                }
                self.selected = 0;
            }
            _ => {}
        }
        Picked::Browsing
    }

    /// The mouse wheel: down (`down`) or up, one item.
    pub fn on_wheel(&mut self, down: bool) {
        let last = self.shown().len().saturating_sub(1);
        self.selected = if down {
            (self.selected + 1).min(last)
        } else {
            self.selected.saturating_sub(1)
        };
    }
}

impl App {
    /// Alt+z: the leader popup.
    pub(super) fn open_leader(&mut self) {
        self.modal = Some(Modal::Leader);
        self.redraw = true;
    }

    /// The leader's lines: the reader, then each enabled extension's
    /// actions (search is not offered in plain mode), then the extensions
    /// screen.
    pub fn leader_entries(&self) -> Vec<LeaderEntry> {
        let mut entries = vec![LeaderEntry {
            key: READER_KEY,
            title: "reader view".to_owned(),
            extension: None,
        }];
        let plain = self.mode == super::app::Mode::Plain;
        for (ext, action) in self.extensions.actions() {
            if plain && ext.name == "search" {
                continue;
            }
            entries.push(LeaderEntry {
                key: action.key,
                title: action.title.clone(),
                extension: Some(ext.name.clone()),
            });
        }
        entries.push(LeaderEntry {
            key: MANAGE_KEY,
            title: "manage extensions".to_owned(),
            extension: None,
        });
        entries
    }

    /// A key in the leader popup: an action's key runs it; anything else
    /// closes the popup. Nothing reaches the session.
    pub(super) fn leader_key(&mut self, key: &KeyEvent) {
        self.modal = None;
        let KeyCode::Char(c) = key.code else { return };
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return;
        }
        if !self.leader_entries().iter().any(|e| e.key == c) {
            return;
        }
        if c == READER_KEY {
            if self.active_view().is_none() {
                self.notify("no active session");
                return;
            }
            return self.open_reader();
        }
        if c == MANAGE_KEY {
            return self.open_extensions_menu();
        }
        let found = self
            .extensions
            .actions()
            .into_iter()
            .find(|(_, a)| a.key == c);
        let Some((ext, action)) = found else { return };
        let (ext, action) = (ext.name.clone(), action.clone());
        match action.input {
            Some(prompt) => {
                self.modal = Some(Modal::ActionInput(Box::new(Input {
                    extension: ext,
                    action: action.id,
                    prompt,
                    text: String::new(),
                    cursor: 0,
                })))
            }
            None => self.start_action(&ext, &action.id, None),
        }
    }

    /// A key in an action's input line.
    pub(super) fn action_input_key(&mut self, key: &KeyEvent) {
        let Some(Modal::ActionInput(input)) = &mut self.modal else {
            return;
        };
        match input.on_key(key) {
            Typed::Editing => {}
            Typed::Cancelled => self.modal = None,
            Typed::Done => {
                let (ext, action, text) = (
                    input.extension.clone(),
                    input.action.clone(),
                    input.text.clone(),
                );
                self.modal = None;
                self.start_action(&ext, &action, Some(text));
            }
        }
    }

    /// A key in the list picker: Enter applies the chosen item's result.
    pub(super) fn picker_key(&mut self, key: &KeyEvent) {
        let Some(Modal::Picker(picker)) = &mut self.modal else {
            return;
        };
        match picker.on_key(key) {
            Picked::Browsing => {}
            Picked::Closed => self.modal = None,
            Picked::Chose(result) => {
                self.modal = None;
                self.apply_result(result);
            }
        }
    }

    /// Run action `action` of extension `ext` with `input`: a built-in's
    /// here, an external one's in the extension host (its result comes
    /// back through [`App::on_action_result`]).
    pub(super) fn start_action(&mut self, ext: &str, action: &str, input: Option<String>) {
        let external = self
            .extensions
            .enabled()
            .any(|e| e.name == ext && matches!(e.source, Source::External(_)));
        if !external {
            return self.builtin_action(ext, action, input);
        }
        let session = self.active_view().map(|v| SessionContext {
            host: v.host.clone(),
            cwd: v.cwd.clone(),
            name: v.name.clone(),
            conversation: v.claude_session_id.clone(),
        });
        self.modal_gen += 1;
        let gen = self.modal_gen;
        self.pending_action = Some(gen);
        self.emit(Effect::RunAction {
            gen,
            extension: ext.to_owned(),
            action: action.to_owned(),
            input,
            session,
        });
    }

    /// The result of the external action started under `gen` (or the
    /// notice saying why there is none). Dropped when another action has
    /// started since, or a modal is open.
    pub fn on_action_result(&mut self, gen: u64, result: Result<ActionResult, String>) {
        if self.pending_action != Some(gen) {
            return;
        }
        self.pending_action = None;
        if self.modal.is_some() {
            return;
        }
        match result {
            Ok(result) => self.apply_result(result),
            Err(notice) => self.notify(notice),
        }
        self.redraw = true;
    }

    /// Show or do what an action gave.
    pub(super) fn apply_result(&mut self, result: ActionResult) {
        match result {
            ActionResult::Notice { text } => self.notify(text),
            ActionResult::Paste { text } => self.paste(&text),
            ActionResult::List { title, items } => {
                self.modal = Some(Modal::Picker(Box::new(Picker::new(title, items))));
            }
            ActionResult::Document { title, markdown } => self.open_document(title, markdown),
            ActionResult::Resume {
                host,
                cwd,
                conversation,
            } => self.resume_conversation(host, cwd, conversation),
        }
        self.redraw = true;
    }

    /// `markdown` in a reader view of its own, rendered on the worker like
    /// an answer.
    pub(super) fn open_document(&mut self, title: String, markdown: String) {
        self.modal_gen += 1;
        let gen = self.modal_gen;
        let id = SessionId::nil();
        let view = ReaderView::document(id, title.clone(), gen);
        self.modal = Some(Modal::Reader(Box::new(view)));
        let answers = vec![Answer {
            prompt: title,
            text: markdown,
        }];
        self.emit(Effect::RenderAnswers { gen, id, answers });
    }

    /// Paste `text` into the active session, as bracketed paste and never
    /// followed by Enter: only into an attached claude session whose prompt
    /// has bracketed paste on (so a newline in the text cannot submit it).
    pub(super) fn paste(&mut self, text: &str) {
        let text = sanitize(text);
        if text.len() > PASTE_MAX {
            return self.notify("result too large");
        }
        let Some(v) = self.active_view() else {
            return self.notify("no active session");
        };
        if v.kind != SessionKind::Claude {
            return self.notify("paste goes to claude sessions only");
        }
        let modes = v.mirror.modes();
        if !v.attached || !modes.bracketed_paste {
            return self.notify("paste needs claude's prompt (bracketed paste is off)");
        }
        let bytes = crate::term::keys::encode_paste(&text, &modes);
        let id = v.id;
        self.emit(Effect::Input(id, bytes));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::extensions::{manifest::Program, ActionDecl, Extension, Registry};
    use crate::tui::test_support::{alt, app_with, info, press};

    fn key(c: char) -> crossterm::event::Event {
        press(KeyCode::Char(c))
    }

    /// An app with an external extension `fake` whose action `k` runs `go`
    /// (asking for input when `input`).
    fn app_with_fake(input: Option<&str>) -> App {
        let mut app = app_with(&[info(Some(1), Some("conv"))]);
        let fake = Extension {
            name: "fake".into(),
            description: String::new(),
            enabled: true,
            lenses: vec![],
            actions: vec![ActionDecl {
                key: 'k',
                title: "do it".into(),
                id: "go".into(),
                input: input.map(str::to_owned),
            }],
            source: Source::External(Program {
                dir: "/x".into(),
                command: vec!["p".into()],
            }),
        };
        app.extensions = Registry::new(&Default::default(), true, vec![fake], &mut Vec::new());
        app.activate(0);
        app.take_effects();
        app
    }

    fn runs(app: &mut App) -> Vec<(u64, String, Option<String>, Option<String>)> {
        app.take_effects()
            .into_iter()
            .filter_map(|e| match e {
                Effect::RunAction {
                    gen,
                    action,
                    input,
                    session,
                    ..
                } => Some((gen, action, input, session.and_then(|s| s.conversation))),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_leader_lists_the_reader_and_every_action() {
        let app = app_with_fake(None);
        let entries: Vec<(char, Option<String>)> = app
            .leader_entries()
            .into_iter()
            .map(|e| (e.key, e.extension))
            .collect();
        let ext = |n: &str| Some(n.to_owned());
        assert_eq!(
            entries,
            [
                ('u', None),
                ('s', ext("search")),
                ('p', ext("snippets")),
                ('P', ext("snippets")),
                ('k', ext("fake")),
                ('e', None),
            ]
        );
        let last = app.leader_entries().pop().unwrap();
        assert_eq!(last.title, "manage extensions");
    }

    #[test]
    fn alt_z_e_opens_the_extensions_screen() {
        let mut app = app_with_fake(None);
        app.on_terminal(alt('z'));
        app.on_terminal(key('e'));
        assert!(matches!(app.modal, Some(Modal::ExtensionsMenu(_))));
        // Even with no session.
        app.modal = None;
        app.sessions.clear();
        app.active = None;
        app.on_terminal(alt('z'));
        app.on_terminal(key('e'));
        assert!(matches!(app.modal, Some(Modal::ExtensionsMenu(_))));
        assert!(runs(&mut app).is_empty());
    }

    #[test]
    fn alt_z_then_a_key_runs_its_action_and_other_keys_just_close() {
        let mut app = app_with_fake(None);
        app.on_terminal(alt('z'));
        assert!(matches!(app.modal, Some(Modal::Leader)));
        // Esc and an unknown key close it; nothing reaches the session.
        app.on_terminal(press(KeyCode::Esc));
        assert!(app.modal.is_none());
        app.on_terminal(alt('z'));
        app.on_terminal(key('q'));
        assert!(app.modal.is_none());
        assert!(!app
            .take_effects()
            .iter()
            .any(|e| matches!(e, Effect::Input(..))));
        // Alt+z again closes it too.
        app.on_terminal(alt('z'));
        app.on_terminal(alt('z'));
        assert!(app.modal.is_none());
        app.on_terminal(alt('z'));
        app.on_terminal(key('k'));
        let ran = runs(&mut app);
        assert_eq!(ran.len(), 1);
        assert_eq!((ran[0].1.as_str(), ran[0].2.as_deref()), ("go", None));
        assert_eq!(ran[0].3.as_deref(), Some("conv"), "the session goes along");
        // Alt+z u is the reader.
        app.on_terminal(alt('z'));
        app.on_terminal(key('u'));
        assert!(matches!(app.modal, Some(Modal::Reader(_))));
    }

    #[test]
    fn an_action_with_input_asks_for_a_line_first() {
        let mut app = app_with_fake(Some("label"));
        app.on_terminal(alt('z'));
        app.on_terminal(key('k'));
        assert!(matches!(app.modal, Some(Modal::ActionInput(_))));
        for c in "héllo".chars() {
            app.on_terminal(key(c));
        }
        app.on_terminal(press(KeyCode::Left));
        app.on_terminal(press(KeyCode::Backspace));
        app.on_terminal(press(KeyCode::Home));
        app.on_terminal(key('>'));
        app.on_terminal(press(KeyCode::End));
        app.on_terminal(key('!'));
        assert!(runs(&mut app).is_empty(), "nothing runs while typing");
        app.on_terminal(press(KeyCode::Enter));
        let ran = runs(&mut app);
        assert_eq!(ran[0].2.as_deref(), Some(">hélo!"));
        // Ctrl+U clears; Esc cancels.
        app.on_terminal(alt('z'));
        app.on_terminal(key('k'));
        app.on_terminal(key('x'));
        let ctrl_u = KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL);
        app.on_terminal(crossterm::event::Event::Key(ctrl_u));
        let Some(Modal::ActionInput(input)) = &app.modal else {
            panic!("no input")
        };
        assert!(input.text.is_empty());
        app.on_terminal(press(KeyCode::Esc));
        assert!(app.modal.is_none() && runs(&mut app).is_empty());
    }

    #[test]
    fn results_show_or_paste_and_stale_ones_are_dropped() {
        let mut app = app_with_fake(None);
        app.on_terminal(alt('z'));
        app.on_terminal(key('k'));
        let first = runs(&mut app)[0].0;
        app.on_terminal(alt('z'));
        app.on_terminal(key('k'));
        let second = runs(&mut app)[0].0;
        // The first answer comes late: dropped.
        app.on_action_result(
            first,
            Ok(ActionResult::Notice {
                text: "late".into(),
            }),
        );
        assert!(app.notice.is_none());
        app.on_action_result(second, Err("extension fake timed out".into()));
        assert_eq!(
            app.notice.as_ref().unwrap().text,
            "extension fake timed out"
        );

        // A list: filter, move, Enter applies the item's result.
        app.on_terminal(alt('z'));
        app.on_terminal(key('k'));
        let gen = runs(&mut app)[0].0;
        let item = |label: &str, text: &str| ListItem {
            label: label.into(),
            detail: Some(format!("about {label}")),
            result: ActionResult::Notice { text: text.into() },
        };
        let items = vec![item("alpha", "A"), item("beta", "B"), item("gamma", "G")];
        let list = ActionResult::List {
            title: "Pick".into(),
            items,
        };
        app.on_action_result(gen, Ok(list));
        let Some(Modal::Picker(picker)) = &app.modal else {
            panic!("no picker")
        };
        assert_eq!(picker.shown(), [0, 1, 2]);
        app.on_terminal(key('A'));
        let Some(Modal::Picker(picker)) = &app.modal else {
            panic!("no picker")
        };
        assert_eq!(picker.shown(), [0, 1, 2], "every label or detail has an a");
        app.on_terminal(press(KeyCode::Backspace));
        for c in "mm".chars() {
            app.on_terminal(key(c));
        }
        app.on_terminal(press(KeyCode::Enter));
        assert!(app.modal.is_none());
        assert_eq!(app.notice.as_ref().unwrap().text, "G");

        // A document opens a reader view on the worker.
        app.on_terminal(alt('z'));
        app.on_terminal(key('k'));
        let gen = runs(&mut app)[0].0;
        let doc = ActionResult::Document {
            title: "Doc".into(),
            markdown: "# hi".into(),
        };
        app.on_action_result(gen, Ok(doc));
        assert!(matches!(app.modal, Some(Modal::Reader(_))));
        assert!(app.take_effects().iter().any(
            |e| matches!(e, Effect::RenderAnswers { answers, .. } if answers[0].text == "# hi")
        ));
    }

    #[test]
    fn a_paste_needs_an_attached_claude_with_bracketed_paste() {
        let mut app = app_with_fake(None);
        app.sessions[0].attached = true;
        app.paste("hi\nthere\u{1b}[201~");
        assert_eq!(
            app.notice.as_ref().unwrap().text,
            "paste needs claude's prompt (bracketed paste is off)"
        );
        assert!(app.take_effects().is_empty());
        app.sessions[0].mirror.feed(b"\x1b[?2004h");
        app.paste("hi\nthere\u{1b}[201~");
        let inputs: Vec<Vec<u8>> = app
            .take_effects()
            .into_iter()
            .filter_map(|e| match e {
                Effect::Input(_, bytes) => Some(bytes),
                _ => None,
            })
            .collect();
        assert_eq!(inputs, [b"\x1b[200~hi\nthere[201~\x1b[201~".to_vec()]);
        app.sessions[0].kind = SessionKind::Shell;
        app.paste("x");
        assert_eq!(
            app.notice.as_ref().unwrap().text,
            "paste goes to claude sessions only"
        );
        app.sessions.clear();
        app.active = None;
        app.paste("x");
        assert_eq!(app.notice.as_ref().unwrap().text, "no active session");
    }

    #[test]
    fn plain_mode_offers_no_search() {
        use crate::tui::app::{AppConfig, Mode};
        let app = App::new(AppConfig {
            mode: Mode::Plain,
            ..crate::tui::test_support::config()
        });
        let keys: Vec<char> = app.leader_entries().iter().map(|e| e.key).collect();
        assert_eq!(keys, ['u', 'p', 'P', 'e']);
    }
}
