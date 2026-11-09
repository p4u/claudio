//! The extensions screen (Alt+i, or `e` in the leader popup): one row per
//! extension (built-ins first, then the external ones that loaded, then the
//! proxy plugins, on or off) with its kind, keys and lenses; below the list,
//! the selected one's description, actions and lenses (for a plugin, its
//! manifest and its state on each host). Space turns it on or off, as
//! Enter does on every row but a plugin's, where Enter opens its setup
//! screen (`plugin_setup.rs`); `r` fetches the plugin lists again.
//!
//! Pure, like `app.rs`: a toggle changes the registry at once and returns
//! [`Effect::SaveExtension`] (the event loop writes `config.toml`) and, for
//! an external extension, [`Effect::SetExtension`] (the extension host
//! refuses or stops its program). Drawn by `ui/extensions.rs`.

use crossterm::event::{KeyCode, KeyEvent};

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
                    Source::Plugin { .. } => "plugin",
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
    /// line per action (`Alt+z <key>  <title>`), a line per lens with its
    /// kind, and for latex turned off by `[claude] latex = false`, that.
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
        if ext.name == "latex" && !self.claude_policy.latex {
            lines.push("also off by [claude] latex = false".to_owned());
        }
        if let Source::Plugin { profile, refused } = &ext.source {
            let details = self.plugin_details(&ext.name, profile, refused.as_deref());
            lines.extend(details.iter().map(|l| clean(l)));
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
                Some(ext) if ext.is_plugin() => self.open_plugin_setup(index, back),
                _ => self.toggle_extension(index),
            },
            Outcome::Refresh => self.refresh_plugins(),
        }
        self.redraw = true;
    }

    /// Turn extension `index` on or off: the registry at once (the leader,
    /// the lenses, later spawns), the extension host for an external one,
    /// and `config.toml`.
    pub(super) fn toggle_extension(&mut self, index: usize) {
        let Some(ext) = self.extensions.extensions.get(index) else {
            return;
        };
        let (name, on, external) = (ext.name.clone(), !ext.enabled, ext.is_external());
        if let Source::Plugin { refused, .. } = &ext.source {
            if let Some(why) = refused {
                self.notify(format!("plugin {} is refused: {why}", clean(&name)));
                return;
            }
            // Kept for a list that comes later; on: the hosts get its
            // manifest (unless they answered for it already).
            self.plugins.config.insert(name.clone(), on);
            self.extensions.extensions[index].enabled = on;
            if on {
                // Its setup if it was never set up, else its manifest.
                let sha = self.plugins.manifests.get(&name).map(|(s, _)| s.clone());
                if let Some(sha) = sha {
                    self.on_manifest_ready(&name, &sha);
                }
                self.request_all();
            }
            self.emit(Effect::SaveExtension { name, on });
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
    use crate::proto::Msg;
    use crate::tui::app::ReplyTo;
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
        app.extensions = Registry::new(&Default::default(), true, vec![fake], &mut Vec::new());
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
                "[x] latex     built-in  —          <latex>",
                "[x] diagram   built-in  —          ```mermaid",
                "[x] search    built-in  Alt+z s    —",
                "[x] snippets  built-in  Alt+z p/P  —",
                "[x] fake      external  Alt+z k    <keys>",
            ]
        );
        assert_eq!(
            app.extension_details(3),
            [
                "saved prompts",
                "Alt+z p  insert snippet",
                "Alt+z P  save last prompt as snippet",
            ]
        );
        assert_eq!(
            app.extension_details(1),
            [
                "Mermaid diagrams drawn in the web UI",
                "```mermaid  fence lens (the web UI)",
            ]
        );
        assert_eq!(
            app.extension_details(4),
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
        assert_eq!(selected(&app), 4);
        app.on_terminal(press(KeyCode::Down));
        assert_eq!(selected(&app), 4);
        app.on_terminal(press(KeyCode::Home));
        assert_eq!(selected(&app), 0);
        app.on_terminal(press(KeyCode::PageDown));
        assert_eq!(selected(&app), 4);
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
        assert!(
            !lenses(&app).contains(&"mermaid".to_owned()),
            "diagram is off"
        );
        assert!(row_lines(&app.extension_rows())[1].starts_with("[ ] diagram"));
        assert_eq!(
            toggles(&mut app),
            [("save", "diagram".to_owned(), false)],
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
        assert_eq!(app.extensions.session_extensions().lenses.len(), 1);
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
    fn latex_turned_off_in_claude_config_says_so() {
        let mut app = app_with_fake("");
        assert!(!app
            .extension_details(0)
            .iter()
            .any(|l| l.contains("[claude] latex")));
        app.claude_policy.latex = false;
        let details = app.extension_details(0);
        assert_eq!(
            details.last().map(String::as_str),
            Some("also off by [claude] latex = false")
        );
    }

    #[test]
    fn an_extension_disabled_after_failures_says_so_in_its_row() {
        let mut app = app_with_fake("");
        app.failed_extensions = vec!["fake".into()];
        let lines = row_lines(&app.extension_rows());
        assert_eq!(
            lines[4],
            "[x] fake      external  Alt+z k    <keys>  (disabled after failures)"
        );
        assert!(!lines[3].contains("failures"));
    }

    #[test]
    fn a_hostile_description_and_title_are_cleaned() {
        let mut app = app_with_fake("evil\u{1b}[31m red\nnext\u{202e}line\u{7}");
        app.extensions.extensions[4].actions[0].title = "do\u{1b}]52;c;eA==\u{7} it".into();
        let details = app.extension_details(4);
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

    /// [`app_with_fake`] with proxy profile `work` serving plugins `kb`
    /// and `latex` (a taken name), and `local` connected.
    fn app_with_plugins() -> App {
        use crate::addons::vocdoni::plugins::state::PluginsFetch;
        use crate::addons::vocdoni::proxy::plugins::{sample, PluginList};
        let mut app = app_with_fake("");
        app.on_plugin_host("local");
        let list = PluginList {
            profile: "work".into(),
            fetched_at: 1,
            plugins: vec![sample("kb"), sample("latex")],
        };
        app.on_plugins("work", PluginsFetch::Listed(list));
        app.take_effects();
        app
    }

    #[test]
    fn plugins_are_listed_with_their_state_on_each_host() {
        let mut app = app_with_plugins();
        let lines = row_lines(&app.extension_rows());
        assert_eq!(lines[5], "[x] kb        plugin    —          —");
        assert_eq!(lines[6], "[ ] latex     plugin    —          —");
        let details = |app: &App, i| app.extension_details(i);
        assert_eq!(
            details(&app, 5),
            [
                "1.4.0 · work",
                "turning it on or off applies to new sessions",
                "manifest: fetching",
            ]
        );
        let src = "format = 1\nname = \"kb\"\nversion = \"1.4.0\"\ndescription = \"KB\"";
        let manifest = crate::addons::vocdoni::plugins::manifest::Manifest::parse(src).unwrap();
        let sha = "a".repeat(64);
        app.plugins.choices.insert("kb".into(), Default::default());
        app.on_manifest("kb", &sha, Ok(manifest));
        assert_eq!(
            details(&app, 5)[2..],
            [
                "KB",
                "0 items disabled (Enter to change)",
                "local: applying"
            ]
        );
        let ready = Msg::Applied(crate::proto::Applied {
            name: "kb".into(),
            version: "1.4.0".into(),
            ..Default::default()
        });
        let to = ReplyTo::Manifest {
            host: "local".into(),
            name: "kb".into(),
            sha256: sha,
        };
        app.on_reply(to.clone(), Ok(ready));
        assert_eq!(
            details(&app, 5)[4],
            "local: ready · 0 installed, 0 present, 0 installing, 0 skipped"
        );
        app.on_reply(
            to,
            Err(std::io::Error::other(
                "manifest kb: download failed: HTTP 404",
            )),
        );
        assert_eq!(
            details(&app, 5)[4],
            "local: failed: download failed: HTTP 404"
        );
        assert!(
            app.notice.is_none(),
            "a manifest error is shown here, not as a notice"
        );
        assert_eq!(
            details(&app, 6),
            [
                "1.4.0 · work",
                "turning it on or off applies to new sessions",
                "refused: name taken",
            ]
        );
    }

    #[test]
    fn a_plugin_toggle_is_saved_and_kept_for_later_lists() {
        let mut app = app_with_plugins();
        app.on_terminal(alt('i'));
        for _ in 0..5 {
            app.on_terminal(press(KeyCode::Down));
        }
        app.on_terminal(press(KeyCode::Char(' ')));
        assert_eq!(toggles(&mut app), [("save", "kb".to_owned(), false)]);
        assert!(app.plugin_parts("local", "work").0.is_empty());
        assert_eq!(app.plugins.config.get("kb"), Some(&false));
        // Back on: saved, and the hosts are asked for its manifest again
        // only when they have no answer at its hash (here: no manifest).
        app.on_terminal(press(KeyCode::Char(' ')));
        assert_eq!(toggles(&mut app), [("save", "kb".to_owned(), true)]);
        // A refused one does not toggle.
        app.on_terminal(press(KeyCode::Down));
        app.on_terminal(press(KeyCode::Char(' ')));
        assert!(app.take_effects().is_empty());
        let notice = app.notice.as_ref().unwrap().text.clone();
        assert_eq!(notice, "plugin latex is refused: name taken");
    }

    #[test]
    fn r_fetches_the_plugin_lists_again() {
        let mut app = app_with_plugins();
        app.proxy_default = Some("home".into());
        app.on_terminal(alt('i'));
        app.on_terminal(press(KeyCode::Char('r')));
        let fetched: Vec<String> = app
            .take_effects()
            .into_iter()
            .filter_map(|e| match e {
                Effect::FetchPlugins { profile } => Some(profile),
                _ => None,
            })
            .collect();
        assert_eq!(fetched, ["home", "work"]);
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
            lines[2].starts_with("[x] search    built-in  —"),
            "{lines:?}"
        );
    }
}
