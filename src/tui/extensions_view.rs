//! The extensions screen (Alt+i, or `e` in the leader popup): one row per
//! extension (built-ins first, then the external ones that loaded, then the
//! addons' rows, on or off) with its kind, keys and lenses; below the list,
//! the selected one's description, actions and lenses (for an addon's row,
//! the addon's lines about it). Space turns it on or off, as Enter does on
//! every row but an addon's with a setup, where Enter opens that checklist
//! (`checklist.rs`); `r` has every addon refresh its rows.
//!
//! Pure, like `app.rs`: a toggle changes the registry at once and returns
//! [`Effect::SaveExtension`] (the event loop writes `config.toml`) and, for
//! an external extension, [`Effect::SetExtension`] (the extension host
//! refuses or stops its program); an addon's row is
//! [`AddonCall::ToggleRow`] (written, then the addon told). Drawn by
//! `ui/extensions.rs`.

use crossterm::event::{KeyCode, KeyEvent};

use super::addon_calls::AddonCall;
use super::app::{App, Effect, Mode};
use super::interaction::Modal;
use super::keymap::Action;
use crate::app::extensions::{ActionDecl, Extension, Source};
use crate::proto::LensKind;
use crate::text::sanitize;

/// The screen's title.
pub const TITLE: &str = "Extensions (Space toggle · Enter plugin setup · r refresh · Esc close)";
/// The screen's last line.
pub const FOOTER: &str =
    "changes apply now; claude's instructions change for new sessions (Alt+e restarts one)";
/// How far PageUp/PageDown move.
const PAGE: usize = 10;
/// What a row or a detail without a value shows.
const NONE: &str = "—";

/// The screen's state: which row is selected (an index into the
/// registry's extensions).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExtensionsView {
    pub selected: usize,
}

/// What a key did on the screen.
enum Outcome {
    Stay,
    Toggle(usize),
    /// Enter: a plugin's setup screen, else a toggle.
    Enter(usize),
    /// `r`: fetch the plugin lists again.
    Refresh,
    Close,
}

impl ExtensionsView {
    /// A key, with `count` rows listed.
    fn on_key(&mut self, key: &KeyEvent, count: usize) -> Outcome {
        let last = count.saturating_sub(1);
        match key.code {
            KeyCode::Esc => return Outcome::Close,
            KeyCode::Char(' ') if count > 0 => return Outcome::Toggle(self.selected.min(last)),
            KeyCode::Enter if count > 0 => return Outcome::Enter(self.selected.min(last)),
            KeyCode::Char('r') => return Outcome::Refresh,
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down => self.selected = (self.selected + 1).min(last),
            KeyCode::PageUp => self.selected = self.selected.saturating_sub(PAGE),
            KeyCode::PageDown => self.selected = (self.selected + PAGE).min(last),
            KeyCode::Home => self.selected = 0,
            KeyCode::End => self.selected = last,
            _ => {}
        }
        Outcome::Stay
    }

    /// The mouse wheel: down (`down`) or up, one row, with `count` rows.
    pub fn on_wheel(&mut self, down: bool, count: usize) {
        let last = count.saturating_sub(1);
        self.selected = if down {
            (self.selected + 1).min(last)
        } else {
            self.selected.saturating_sub(1)
        };
    }
}

/// One row of the list.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub name: String,
    pub enabled: bool,
    /// `built-in`, `external` or `plugin`.
    pub kind: &'static str,
    /// Its actions' keys (`Alt+z p/P`), or `—`.
    pub shortcuts: String,
    /// Its lenses (`<latex> · ```mermaid`), or `—`.
    pub lenses: String,
    /// The extension host disabled it after failures.
    pub failed: bool,
}

/// `text` from a manifest made safe to draw on one line: no control or
/// format characters, no newlines or tabs.
fn clean(text: &str) -> String {
    sanitize(text).replace(['\n', '\t', '\r'], " ")
}

/// How a lens shows: `<tag>` or ```` ```fence ````.
fn lens_label(name: &str, kind: LensKind) -> String {
    match kind {
        LensKind::Fence => format!("```{}", clean(name)),
        _ => format!("<{}>", clean(name)),
    }
}

/// The rows as aligned lines: `[x] name  kind  keys  lenses`.
pub fn row_lines(rows: &[Row]) -> Vec<String> {
    let name_width = rows.iter().map(|r| r.name.chars().count()).max();
    let keys_width = rows.iter().map(|r| r.shortcuts.chars().count()).max();
    let (name_width, keys_width) = (name_width.unwrap_or(0), keys_width.unwrap_or(0));
    rows.iter()
        .map(|r| {
            let mark = if r.enabled { 'x' } else { ' ' };
            let kind = r.kind;
            let failed = if r.failed {
                "  (disabled after failures)"
            } else {
                ""
            };
            let line = format!(
                "[{mark}] {:name_width$}  {kind:8}  {:keys_width$}  {}{failed}",
                r.name, r.shortcuts, r.lenses
            );
            line.trim_end().to_owned()
        })
        .collect()
}

impl App {
    /// Alt+i (or `e` in the leader popup): the extensions screen.
    pub(super) fn open_extensions_menu(&mut self) {
        self.modal = Some(Modal::ExtensionsMenu(ExtensionsView::default()));
        self.redraw = true;
    }

    /// The leader key as shown (`Alt+z`).
    fn leader_key_str(&self) -> String {
        self.keymap
            .key_for(Action::Extensions)
            .unwrap_or_else(|| "Alt+z".to_owned())
    }

    /// The actions of `ext` the leader offers here (search is not offered
    /// in plain mode).
    fn offered_actions<'a>(&self, ext: &'a Extension) -> &'a [ActionDecl] {
        if self.mode == Mode::Plain && ext.name == "search" {
            return &[];
        }
        &ext.actions
    }

    /// The screen's rows, in registry order.
    pub fn extension_rows(&self) -> Vec<Row> {
        let leader = self.leader_key_str();
        self.extensions
            .extensions
            .iter()
            .map(|ext| {
                let keys: Vec<String> = self
                    .offered_actions(ext)
                    .iter()
                    .map(|a| a.key.to_string())
                    .collect();
                let shortcuts = if keys.is_empty() {
                    NONE.to_owned()
                } else {
                    format!("{leader} {}", keys.join("/"))
                };
                let lenses: Vec<String> = ext
                    .lenses
                    .iter()
                    .map(|l| lens_label(&l.name, l.kind))
                    .collect();
                let lenses = if lenses.is_empty() {
                    NONE.to_owned()
                } else {
                    lenses.join(" · ")
                };
                let kind = match ext.source {
                    Source::BuiltIn => "built-in",
                    Source::External(_) => "external",
                    // The addon that lists it (`web`, a plugin's addon).
                    Source::Addon { addon, .. } => addon,
                };
                Row {
                    name: clean(&ext.name),
                    enabled: ext.enabled,
                    kind,
                    shortcuts,
                    lenses,
                    failed: self.failed_extensions.contains(&ext.name),
                }
            })
            .collect()
    }

    /// What the screen says about extension `index`: its description, a
    /// line per action (`Alt+z <key>  <title>`) and a line per lens with
    /// its kind.
    pub fn extension_details(&self, index: usize) -> Vec<String> {
        let Some(ext) = self.extensions.extensions.get(index) else {
            return Vec::new();
        };
        let leader = self.leader_key_str();
        let description = clean(&ext.description);
        let mut lines = vec![if description.trim().is_empty() {
            "(no description)".to_owned()
        } else {
            description
        }];
        for action in self.offered_actions(ext) {
            let title = clean(&action.title);
            lines.push(format!("{leader} {}  {title}", action.key));
        }
        for lens in &ext.lenses {
            let kind = match lens.kind {
                LensKind::Fence => "fence lens (the web UI)",
                _ => "tag lens (the web UI)",
            };
            lines.push(format!("{}  {kind}", lens_label(&lens.name, lens.kind)));
        }
        if let Source::Addon {
            refused, details, ..
        } = &ext.source
        {
            // Refused by the core (a name taken): the addon's lines do not
            // say so, the screen does.
            match refused {
                Some(why) if !details.iter().any(|l| l.contains(why.as_str())) => {
                    lines.push(format!("refused: {}", clean(why)))
                }
                _ => lines.extend(details.iter().map(|l| clean(l))),
            }
        }
        lines
    }

    /// A key on the extensions screen.
    pub(super) fn extensions_menu_key(&mut self, key: &KeyEvent) {
        let count = self.extensions.extensions.len();
        let Some(Modal::ExtensionsMenu(view)) = &mut self.modal else {
            return;
        };
        let back = view.clone();
        match view.on_key(key, count) {
            Outcome::Stay => {}
            Outcome::Close => self.modal = None,
            Outcome::Toggle(index) => self.toggle_extension(index),
            Outcome::Enter(index) => match self.extensions.extensions.get(index) {
                Some(Extension {
                    source: Source::Addon { setup: Some(_), .. },
                    ..
                }) => self.open_plugin_setup(index, back),
                _ => self.toggle_extension(index),
            },
            Outcome::Refresh => self.refresh_rows(),
        }
        self.redraw = true;
    }

    /// `r` on the extensions screen: every addon refreshes its rows.
    pub(super) fn refresh_rows(&mut self) {
        self.emit(Effect::Addon(AddonCall::RefreshRows));
        self.notify("refreshing plugins…");
    }

    /// Turn extension `index` on or off: the registry at once (the leader,
    /// the lenses, later spawns), the extension host for an external one,
    /// and `config.toml`.
    pub(super) fn toggle_extension(&mut self, index: usize) {
        let Some(ext) = self.extensions.extensions.get(index) else {
            return;
        };
        let (name, on, external) = (ext.name.clone(), !ext.enabled, ext.is_external());
        if let Source::Addon { addon, refused, .. } = &ext.source {
            if let Some(why) = refused {
                self.notify(format!(
                    "extension {} is refused: {}",
                    clean(&name),
                    clean(why)
                ));
                return;
            }
            // Written to `[extensions]` first, then the addon is told: it
            // keeps it for rows that come later.
            let addon = *addon;
            self.extensions.extensions[index].enabled = on;
            self.extension_config.insert(name.clone(), on);
            self.emit(Effect::Addon(AddonCall::ToggleRow { addon, name, on }));
            self.redraw = true;
            return;
        }
        self.extensions.set_enabled(&name, on);
        if external {
            self.emit(Effect::SetExtension {
                name: name.clone(),
                on,
            });
        }
        self.emit(Effect::SaveExtension { name, on });
        self.redraw = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::extensions::{manifest::Program, Lens, Registry};
    use crate::tui::test_support::{alt, app_with, info, press};

    /// An app with one session and an external extension `fake` (a tag
    /// lens `keys`, an action `k`) described as `description`.
    fn app_with_fake(description: &str) -> App {
        let mut app = app_with(&[info(Some(1), Some("conv"))]);
        let fake = Extension {
            name: "fake".into(),
            description: description.into(),
            enabled: true,
            lenses: vec![Lens {
                name: "keys".into(),
                kind: LensKind::Tag,
                instruction: String::new(),
            }],
            actions: vec![ActionDecl {
                key: 'k',
                title: "do it".into(),
                id: "go".into(),
                input: None,
            }],
            source: Source::External(Program {
                dir: "/x".into(),
                command: vec!["p".into()],
            }),
        };
        app.extensions = Registry::new(&Default::default(), vec![fake]);
        app.activate(0);
        app.take_effects();
        app
    }

    /// The effects queued, as (`host` | `save` | `other`, name, on).
    fn toggles(app: &mut App) -> Vec<(&'static str, String, bool)> {
        app.take_effects()
            .into_iter()
            .map(|e| match e {
                Effect::SetExtension { name, on } => ("host", name, on),
                Effect::SaveExtension { name, on } => ("save", name, on),
                Effect::Addon(AddonCall::ToggleRow { addon, name, on }) => (addon, name, on),
                other => ("other", format!("{other:?}"), false),
            })
            .collect()
    }

    fn selected(app: &App) -> usize {
        match &app.modal {
            Some(Modal::ExtensionsMenu(view)) => view.selected,
            _ => panic!("not the extensions screen"),
        }
    }

    #[test]
    fn alt_i_opens_the_screen_and_alt_i_or_esc_closes_it() {
        let mut app = app_with_fake("");
        app.on_terminal(alt('i'));
        assert_eq!(selected(&app), 0);
        app.on_terminal(alt('i'));
        assert!(app.modal.is_none());
        app.on_terminal(alt('i'));
        app.on_terminal(press(KeyCode::Esc));
        assert!(app.modal.is_none());
        // Nothing reached the session.
        assert!(!app
            .take_effects()
            .iter()
            .any(|e| matches!(e, Effect::Input(..))));
    }

    #[test]
    fn rows_list_the_builtins_then_the_external_ones_with_keys_and_lenses() {
        let app = app_with_fake("");
        assert_eq!(
            row_lines(&app.extension_rows()),
            [
                "[x] search    built-in  Alt+z s    —",
                "[x] snippets  built-in  Alt+z p/P  —",
                "[x] fake      external  Alt+z k    <keys>",
            ]
        );
        assert_eq!(
            app.extension_details(1),
            [
                "saved prompts",
                "Alt+z p  insert snippet",
                "Alt+z P  save last prompt as snippet",
            ]
        );
        assert_eq!(
            app.extension_details(2),
            [
                "(no description)",
                "Alt+z k  do it",
                "<keys>  tag lens (the web UI)",
            ]
        );
    }

    #[test]
    fn keys_and_the_wheel_move_the_selection_within_the_list() {
        let mut app = app_with_fake("");
        app.on_terminal(alt('i'));
        app.on_terminal(press(KeyCode::Up));
        assert_eq!(selected(&app), 0);
        app.on_terminal(press(KeyCode::Down));
        app.on_terminal(press(KeyCode::Down));
        assert_eq!(selected(&app), 2);
        app.on_terminal(press(KeyCode::End));
        assert_eq!(selected(&app), 2);
        app.on_terminal(press(KeyCode::Down));
        assert_eq!(selected(&app), 2);
        app.on_terminal(press(KeyCode::Home));
        assert_eq!(selected(&app), 0);
        app.on_terminal(press(KeyCode::PageDown));
        assert_eq!(selected(&app), 2);
        app.on_terminal(press(KeyCode::PageUp));
        assert_eq!(selected(&app), 0);
        let wheel = |kind| {
            crossterm::event::Event::Mouse(crossterm::event::MouseEvent {
                kind,
                column: 10,
                row: 10,
                modifiers: crossterm::event::KeyModifiers::NONE,
            })
        };
        use crossterm::event::MouseEventKind::{ScrollDown, ScrollUp};
        app.on_terminal(wheel(ScrollDown));
        app.on_terminal(wheel(ScrollDown));
        assert_eq!(selected(&app), 2);
        app.on_terminal(wheel(ScrollUp));
        assert_eq!(selected(&app), 1);
        // Other keys do nothing; the screen stays.
        app.on_terminal(press(KeyCode::Char('q')));
        assert_eq!(selected(&app), 1);
    }

    #[test]
    fn a_toggle_changes_the_registry_and_asks_to_save_it() {
        let mut app = app_with_fake("");
        app.on_terminal(alt('i'));
        app.on_terminal(press(KeyCode::Down));
        app.on_terminal(press(KeyCode::Char(' ')));
        let lenses = |app: &App| -> Vec<String> {
            let ext = app.extensions.session_extensions();
            ext.lenses.into_iter().map(|l| l.name).collect()
        };
        assert!(!app.leader_entries().iter().any(|e| e.key == 'p'));
        assert!(row_lines(&app.extension_rows())[1].starts_with("[ ] snippets"));
        assert_eq!(
            toggles(&mut app),
            [("save", "snippets".to_owned(), false)],
            "a built-in needs no host"
        );
        // An external one: the host follows, and it leaves the leader.
        app.on_terminal(press(KeyCode::End));
        app.on_terminal(press(KeyCode::Enter));
        assert_eq!(
            toggles(&mut app),
            [
                ("host", "fake".to_owned(), false),
                ("save", "fake".to_owned(), false)
            ]
        );
        assert!(!lenses(&app).contains(&"keys".to_owned()));
        assert!(!app.leader_entries().iter().any(|e| e.key == 'k'));
        assert!(app.extensions.session_extensions().lenses.is_empty());
        // Again: back on.
        app.on_terminal(press(KeyCode::Char(' ')));
        assert!(lenses(&app).contains(&"keys".to_owned()));
        assert!(app.leader_entries().iter().any(|e| e.key == 'k'));
        assert_eq!(
            toggles(&mut app),
            [
                ("host", "fake".to_owned(), true),
                ("save", "fake".to_owned(), true)
            ]
        );
        assert!(matches!(app.modal, Some(Modal::ExtensionsMenu(_))));
    }

    #[test]
    fn an_extension_disabled_after_failures_says_so_in_its_row() {
        let mut app = app_with_fake("");
        app.failed_extensions = vec!["fake".into()];
        let lines = row_lines(&app.extension_rows());
        assert_eq!(
            lines[2],
            "[x] fake      external  Alt+z k    <keys>  (disabled after failures)"
        );
        assert!(!lines[1].contains("failures"));
    }

    #[test]
    fn a_hostile_description_and_title_are_cleaned() {
        let mut app = app_with_fake("evil\u{1b}[31m red\nnext\u{202e}line\u{7}");
        app.extensions.extensions[2].actions[0].title = "do\u{1b}]52;c;eA==\u{7} it".into();
        let details = app.extension_details(2);
        for line in &details {
            assert!(
                !line
                    .chars()
                    .any(|c| c.is_control() || matches!(c, '\u{202a}'..='\u{202e}')),
                "{line:?}"
            );
        }
        assert!(details[0].starts_with("evil"), "{details:?}");
        assert!(details[0].contains("next"), "on one line: {details:?}");
    }

    /// [`app_with_fake`] with addon `team`'s rows `kb` and `search` (a
    /// name the core holds, refused by the registry).
    fn app_with_rows(details: &[&str]) -> App {
        use crate::tui::test_support::rows;
        let mut app = app_with_fake("");
        rows(&mut app, "team", &[("kb", None), ("search", None)], details);
        app.take_effects();
        app
    }

    /// A misspelt `[extensions]` key is named once the addons have started
    /// (their rows are known then); a name the rows hold is not.
    #[test]
    fn an_extensions_key_that_names_nothing_is_said_once_the_addons_started() {
        use crate::tui::addon_calls::AddonResult;
        let mut app = app_with_fake("");
        app.extension_config = [("kb".to_owned(), false), ("latx".to_owned(), false)].into();
        let kb = crate::addon::ExtensionRow {
            name: "kb".into(),
            description: String::new(),
            default_on: true,
            refused: None,
            details: Vec::new(),
            setup: None,
        };
        // A row change before the start says nothing.
        app.on_addon(AddonResult::Rows(Vec::new()));
        assert!(app.notice.is_none());
        app.on_addon(AddonResult::Started(vec![("team", kb)]));
        assert_eq!(
            app.notice.as_ref().map(|n| n.text.as_str()),
            Some("config.toml [extensions]: no extension named 'latx'")
        );
        assert!(app
            .extensions
            .extensions
            .iter()
            .any(|e| e.name == "kb" && !e.enabled));
    }

    #[test]
    fn addon_rows_are_listed_with_the_addons_details() {
        let details = [
            "turning it on or off applies to new sessions",
            "local: applying",
        ];
        let app = app_with_rows(&details);
        let lines = row_lines(&app.extension_rows());
        assert_eq!(lines[3], "[x] kb        team      —          —");
        assert_eq!(lines[4], "[ ] search    team      —          —");
        assert_eq!(
            app.extension_details(3),
            [
                "1.4.0 · work",
                "turning it on or off applies to new sessions",
                "local: applying",
            ]
        );
        // Refused by the core: the addon's lines do not apply.
        assert_eq!(
            app.extension_details(4),
            ["1.4.0 · work", "refused: name taken"]
        );
        // The addon's own refusal is in its lines.
        let mut app = app_with_fake("");
        let own = ["refused: bundle URL is not https"];
        crate::tui::test_support::rows(
            &mut app,
            "team",
            &[("evil", Some("bundle URL is not https"))],
            &own,
        );
        assert_eq!(app.extension_details(3), ["1.4.0 · work", own[0]]);
    }

    #[test]
    fn a_row_toggle_is_kept_for_later_rows_and_told_to_the_addon() {
        use crate::tui::test_support::rows;
        let mut app = app_with_rows(&[]);
        app.on_terminal(alt('i'));
        for _ in 0..3 {
            app.on_terminal(press(KeyCode::Down));
        }
        app.on_terminal(press(KeyCode::Char(' ')));
        // One call: the runner writes `[extensions]`, then tells the addon.
        assert_eq!(toggles(&mut app), [("team", "kb".to_owned(), false)]);
        assert_eq!(app.extension_config.get("kb"), Some(&false));
        // Kept for rows that come later.
        rows(&mut app, "team", &[("kb", None), ("search", None)], &[]);
        assert!(!app
            .extensions
            .extensions
            .iter()
            .any(|e| e.name == "kb" && e.enabled));
        app.on_terminal(press(KeyCode::Char(' ')));
        assert_eq!(toggles(&mut app), [("team", "kb".to_owned(), true)]);
        // A refused one does not toggle.
        app.on_terminal(press(KeyCode::Down));
        app.on_terminal(press(KeyCode::Char(' ')));
        assert!(app.take_effects().is_empty());
        let notice = app.notice.as_ref().unwrap().text.clone();
        assert_eq!(notice, "extension search is refused: name taken");
    }

    #[test]
    fn r_refreshes_the_addons_rows() {
        let mut app = app_with_rows(&[]);
        app.on_terminal(alt('i'));
        app.on_terminal(press(KeyCode::Char('r')));
        let calls: Vec<String> = app
            .take_effects()
            .into_iter()
            .map(|e| format!("{e:?}"))
            .collect();
        assert_eq!(calls, ["Addon(RefreshRows)"]);
        assert_eq!(app.notice.as_ref().unwrap().text, "refreshing plugins…");
        assert!(matches!(app.modal, Some(Modal::ExtensionsMenu(_))));
    }

    #[test]
    fn plain_mode_shows_no_search_key() {
        use crate::tui::app::AppConfig;
        let app = App::new(AppConfig {
            mode: Mode::Plain,
            ..crate::tui::test_support::config()
        });
        let lines = row_lines(&app.extension_rows());
        assert!(
            lines[0].starts_with("[x] search    built-in  —"),
            "{lines:?}"
        );
    }
}
