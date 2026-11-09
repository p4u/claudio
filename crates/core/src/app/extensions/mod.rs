//! The extension registry: the lenses a session's spawn asks for (the web
//! UI renders their tags; the console TUI shows claude's text as written)
//! and what the leader key (Alt+z) offers (actions), from built-in
//! extensions and from external ones
//! (`~/.config/claudio/extensions/<name>/`, see [`manifest`]).
//!
//! Pure: the manifests are read by `tui/mod.rs` at start, and external
//! programs run in the extension host it drives. Everything else asks the
//! registry: the context a spawn carries
//! ([`Registry::session_extensions`]) and the leader's actions.

pub mod host;
pub mod manifest;
pub mod protocol;

use std::collections::BTreeMap;

use crate::addon::ExtensionRow;
use crate::proto::{LensKind, LensSpec, SessionExtensions, MAX_INSTRUCTIONS, MAX_LENSES};

/// One lens of an extension.
#[derive(Debug, Clone, PartialEq)]
pub struct Lens {
    /// The tag, or the fence's info word.
    pub name: String,
    pub kind: LensKind,
    /// What claude is told about it (may be empty).
    pub instruction: String,
}

/// One action under the leader key.
#[derive(Debug, Clone, PartialEq)]
pub struct ActionDecl {
    pub key: char,
    pub title: String,
    /// The name passed back to the extension.
    pub id: String,
    /// Ask for a line of input first, with this prompt.
    pub input: Option<String>,
}

/// What an action gives (built-in and external actions share it).
#[derive(Debug, Clone, PartialEq)]
pub enum ActionResult {
    /// Markdown shown in a reader-style modal, its lenses rendered.
    Document { title: String, markdown: String },
    /// A status-line notice.
    Notice { text: String },
    /// Text pasted into the active claude session (never followed by Enter).
    Paste { text: String },
    /// A list to pick from; each item carries its own result (not a list).
    List { title: String, items: Vec<ListItem> },
    /// Built-ins only (search; never from an external program): show
    /// `conversation` on `host`, resuming it in `cwd` if no tab has it.
    Resume {
        host: String,
        cwd: String,
        conversation: String,
    },
}

/// One entry of [`ActionResult::List`].
#[derive(Debug, Clone, PartialEq)]
pub struct ListItem {
    pub label: String,
    pub detail: Option<String>,
    pub result: ActionResult,
}

/// Where an extension's code is.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    /// Rust code in claudio.
    BuiltIn,
    /// A program the extension host runs.
    External(manifest::Program),
    /// A row of addon `addon` (docs/addons.md, "Extension rows"): no lenses
    /// or actions here. `refused` says why it is not used, if it is not;
    /// `details` are the addon's lines about it; `setup` the checklist
    /// Enter opens.
    Addon {
        addon: &'static str,
        refused: Option<String>,
        details: Vec<String>,
        setup: Option<String>,
    },
}

/// One row of the extension registry: its name, whether it is on, the
/// lenses and actions it declares, and where its code is.
#[derive(Debug, Clone, PartialEq)]
pub struct Extension {
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub lenses: Vec<Lens>,
    pub actions: Vec<ActionDecl>,
    pub source: Source,
}

impl Extension {
    /// Whether a program run by the extension host implements it.
    pub fn is_external(&self) -> bool {
        matches!(self.source, Source::External(_))
    }

    /// Whether it is an addon's row (no lenses or actions of its own).
    pub fn is_addon(&self) -> bool {
        matches!(self.source, Source::Addon { .. })
    }
}

/// The built-in extensions, in registry order, all enabled. The `latex`
/// and `diagram` lenses are the web addon's (its `Launch` and rows).
pub fn builtins() -> Vec<Extension> {
    let builtin = |name: &str, description: &str| Extension {
        name: name.to_owned(),
        description: description.to_owned(),
        enabled: true,
        lenses: Vec::new(),
        actions: Vec::new(),
        source: Source::BuiltIn,
    };
    let action = |key, title: &str, id: &str, input: Option<&str>| ActionDecl {
        key,
        title: title.to_owned(),
        id: id.to_owned(),
        input: input.map(str::to_owned),
    };
    let mut search = builtin("search", "search claude conversations");
    search.actions = vec![action(
        's',
        "search conversations",
        "search",
        Some("search conversations"),
    )];
    let mut snippets = builtin("snippets", "saved prompts");
    snippets.actions = vec![
        action('p', "insert snippet", "insert", None),
        action('P', "save last prompt as snippet", "save", None),
    ];
    vec![search, snippets]
}

/// `[extensions]` as the core's own extensions (the built-ins and
/// `external`) see it. Any other name may be an addon's row, which comes
/// later from the addon runtime and applies it itself
/// ([`Registry::set_addon_rows`]), so it is no unknown name here.
pub fn core_choices(
    enabled: &BTreeMap<String, bool>,
    external: &[Extension],
) -> BTreeMap<String, bool> {
    let builtins = builtins();
    let held = |name: &str| {
        builtins.iter().any(|e| e.name == name) || external.iter().any(|e| e.name == name)
    };
    enabled
        .iter()
        .filter(|(name, _)| held(name))
        .map(|(name, on)| (name.clone(), *on))
        .collect()
}

/// The reader's leader key: always offered, not an extension's.
pub const READER_KEY: char = 'u';

/// The leader key of the extensions screen (also Alt+i): always offered,
/// not an extension's.
pub const MANAGE_KEY: char = 'e';

/// The extensions this client runs with.
#[derive(Debug, Clone, PartialEq)]
pub struct Registry {
    pub extensions: Vec<Extension>,
}

impl Default for Registry {
    /// The built-ins, all on.
    fn default() -> Self {
        Registry {
            extensions: builtins(),
        }
    }
}

impl Registry {
    /// The built-ins, then `external` (already validated against each
    /// other and the built-ins by [`manifest::admit`]), with `[extensions]`
    /// (`enabled`) applied. A name that matches none of them may be an
    /// addon's row: [`Registry::set_addon_rows`] names the unknown ones.
    pub fn new(enabled: &BTreeMap<String, bool>, external: Vec<Extension>) -> Registry {
        let mut extensions = builtins();
        extensions.extend(external);
        for ext in &mut extensions {
            if let Some(on) = enabled.get(&ext.name) {
                ext.enabled = *on;
            }
        }
        Registry { extensions }
    }

    /// Replace the addons' rows with `rows`, after the built-ins and the
    /// externals, in order. A row whose name is not a valid `[extensions]`
    /// key, or is held by another extension or an earlier row, is listed
    /// refused; a row is on when `enabled` (`[extensions]`) says so, else
    /// when it is on by default, and never while refused. Returns a notice
    /// for each name in `enabled` that matches no extension, row included
    /// (a front-end shows them once the addons have started).
    pub fn set_addon_rows(
        &mut self,
        rows: &[(&'static str, ExtensionRow)],
        enabled: &BTreeMap<String, bool>,
    ) -> Vec<String> {
        self.extensions.retain(|e| !e.is_addon());
        for (addon, row) in rows {
            let taken = self.extensions.iter().any(|e| e.name == row.name);
            let refused = if !crate::addon::name_ok(&row.name) {
                Some("not a valid extension name".to_owned())
            } else if taken {
                Some("name taken".to_owned())
            } else {
                row.refused.clone()
            };
            let on = enabled.get(&row.name).copied().unwrap_or(row.default_on);
            self.extensions.push(Extension {
                name: row.name.clone(),
                description: row.description.clone(),
                enabled: refused.is_none() && on,
                lenses: Vec::new(),
                actions: Vec::new(),
                source: Source::Addon {
                    addon,
                    refused,
                    details: row.details.clone(),
                    setup: row.setup.clone(),
                },
            });
        }
        enabled
            .keys()
            .filter(|name| !self.extensions.iter().any(|e| &e.name == *name))
            .map(|name| format!("config.toml [extensions]: no extension named '{name}'"))
            .collect()
    }

    /// Turn extension `name` on or off; whether there is one.
    pub fn set_enabled(&mut self, name: &str, on: bool) -> bool {
        match self.extensions.iter_mut().find(|e| e.name == name) {
            Some(ext) => {
                ext.enabled = on;
                true
            }
            None => false,
        }
    }

    /// The enabled extensions, in order.
    pub fn enabled(&self) -> impl Iterator<Item = &Extension> {
        self.extensions.iter().filter(|e| e.enabled)
    }

    /// The session context a claude spawn carries: the enabled lenses in
    /// registry order, within the protocol's caps (a lens that would break
    /// them is left out, so the daemon never refuses the spawn).
    pub fn session_extensions(&self) -> SessionExtensions {
        self.fit().0
    }

    /// The names of the enabled lenses [`Registry::session_extensions`]
    /// leaves out because of the caps.
    pub fn dropped_lenses(&self) -> Vec<String> {
        self.fit().1
    }

    /// The enabled lenses that fit the caps, and the names of those that
    /// do not.
    fn fit(&self) -> (SessionExtensions, Vec<String>) {
        let mut lenses = Vec::new();
        let mut dropped = Vec::new();
        let mut total = 0;
        for lens in self.enabled().flat_map(|e| e.lenses.iter()) {
            let size = crate::text::sanitize(&lens.instruction).len();
            if lenses.len() == MAX_LENSES || total + size > MAX_INSTRUCTIONS {
                dropped.push(lens.name.clone());
                continue;
            }
            total += size;
            lenses.push(LensSpec {
                name: lens.name.clone(),
                kind: lens.kind,
                instruction: lens.instruction.clone(),
            });
        }
        let ext = SessionExtensions {
            lenses,
            ..Default::default()
        };
        (ext, dropped)
    }

    /// The leader's actions: each enabled extension's, in order.
    pub fn actions(&self) -> Vec<(&Extension, &ActionDecl)> {
        self.enabled()
            .flat_map(|e| e.actions.iter().map(move |a| (e, a)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn external(name: &str, tag: &str, key: char) -> Extension {
        Extension {
            name: name.into(),
            description: String::new(),
            enabled: true,
            lenses: vec![Lens {
                name: tag.into(),
                kind: LensKind::Tag,
                instruction: format!("write <{tag}>"),
            }],
            actions: vec![ActionDecl {
                key,
                title: "t".into(),
                id: "a".into(),
                input: None,
            }],
            source: Source::External(manifest::Program {
                dir: "/x".into(),
                command: vec!["prog".into()],
            }),
        }
    }

    fn names(ext: &SessionExtensions) -> Vec<&str> {
        ext.lenses.iter().map(|l| l.name.as_str()).collect()
    }

    #[test]
    fn lenses_left_out_by_the_caps_are_named() {
        assert!(Registry::default().dropped_lenses().is_empty());
        let mut r = Registry::default();
        for i in 0..MAX_LENSES + 2 {
            r.extensions
                .push(external(&format!("x{i}"), &format!("t{i}"), 'a'));
        }
        // MAX_LENSES + 2 lenses: the last two do not fit.
        let last = format!("t{}", MAX_LENSES + 1);
        let before = format!("t{}", MAX_LENSES);
        assert_eq!(r.dropped_lenses(), [before, last]);
        assert_eq!(r.session_extensions().lenses.len(), MAX_LENSES);
    }

    #[test]
    fn the_builtins_are_the_leader_actions_and_ask_for_no_lens() {
        // latex and diagram are the web addon's (its lenses and rows).
        let r = Registry::default();
        let ext = r.session_extensions();
        assert!(ext.lenses.is_empty());
        assert_eq!(ext.validate(), Ok(()));
        let names: Vec<&str> = r.extensions.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["search", "snippets"]);
        let keys: Vec<char> = r.actions().iter().map(|(_, a)| a.key).collect();
        assert_eq!(keys, ['s', 'p', 'P']);
        assert!(!r.extensions.iter().any(Extension::is_external));
    }

    #[test]
    fn config_turns_extensions_off_and_names_unknown_ones() {
        let enabled = BTreeMap::from([
            ("snippets".to_owned(), false),
            ("kb".to_owned(), false),
            ("nope".to_owned(), false),
        ]);
        let mut r = Registry::new(&enabled, vec![]);
        let keys: Vec<char> = r.actions().iter().map(|(_, a)| a.key).collect();
        assert_eq!(keys, ['s']);
        // Once the rows are in: a built-in's or a row's name is known.
        let notices = r.set_addon_rows(&[row("kb", None)], &enabled);
        assert_eq!(
            notices,
            ["config.toml [extensions]: no extension named 'nope'"]
        );
        assert!(!r.extensions[2].enabled, "kb is off");
    }

    #[test]
    fn core_choices_leave_the_addons_rows_names_to_the_rows() {
        let enabled = BTreeMap::from([
            ("snippets".to_owned(), false),
            ("keys".to_owned(), false),
            ("latex".to_owned(), false),
            ("kb".to_owned(), true),
        ]);
        let external = [external("keys", "keys", 'k')];
        let choices = core_choices(&enabled, &external);
        let held: Vec<&str> = choices.keys().map(String::as_str).collect();
        assert_eq!(held, ["keys", "snippets"]);
        let r = Registry::new(&choices, external.to_vec());
        assert!(r.session_extensions().lenses.is_empty(), "keys is off");
    }

    #[test]
    fn external_extensions_follow_the_builtins() {
        let ext = vec![external("keys", "keys", 'k')];
        let r = Registry::new(&BTreeMap::new(), ext);
        assert_eq!(names(&r.session_extensions()), ["keys"]);
        assert!(r.extensions[2].is_external() && r.extensions[2].name == "keys");
        let keys: Vec<char> = r.actions().iter().map(|(_, a)| a.key).collect();
        assert_eq!(keys, ['s', 'p', 'P', 'k']);
        let off = BTreeMap::from([("keys".to_owned(), false)]);
        let r = Registry::new(&off, vec![external("keys", "keys", 'k')]);
        assert!(r.session_extensions().lenses.is_empty());
    }

    #[test]
    fn an_extension_turned_off_and_on_again_takes_its_lenses_and_actions_along() {
        let ext = vec![external("keys", "keys", 'k')];
        let mut r = Registry::new(&BTreeMap::new(), ext);
        assert!(r.set_enabled("keys", false));
        assert!(r.session_extensions().lenses.is_empty());
        let keys: Vec<char> = r.actions().iter().map(|(_, a)| a.key).collect();
        assert_eq!(keys, ['s', 'p', 'P']);
        // Still listed, off.
        assert_eq!(r.extensions.len(), 3);
        assert!(r.set_enabled("keys", true));
        assert_eq!(names(&r.session_extensions()), ["keys"]);
        let keys: Vec<char> = r.actions().iter().map(|(_, a)| a.key).collect();
        assert_eq!(keys, ['s', 'p', 'P', 'k']);
        assert!(!r.set_enabled("nope", true));
    }

    fn row(name: &str, refused: Option<&str>) -> (&'static str, ExtensionRow) {
        let row = ExtensionRow {
            name: name.into(),
            description: format!("1.4.0 · {name}"),
            default_on: true,
            refused: refused.map(str::to_owned),
            details: vec!["details".into()],
            setup: Some(name.into()),
        };
        ("team", row)
    }

    #[test]
    fn addon_rows_follow_the_others_and_a_taken_name_is_refused() {
        let ext = vec![external("keys", "keys", 'k')];
        let mut r = Registry::new(&BTreeMap::new(), ext);
        let rows = [
            row("kb", None),
            row("keys", None),
            row("search", None),
            row("off", None),
            row("kb", None),
            row("evil", Some("bundle URL is not https")),
            row("Bad Name", None),
        ];
        let enabled = BTreeMap::from([("off".to_owned(), false)]);
        r.set_addon_rows(&rows, &enabled);
        let listed: Vec<(&str, bool, Option<&str>)> = r
            .extensions
            .iter()
            .filter_map(|e| match &e.source {
                Source::Addon { refused, .. } => {
                    Some((e.name.as_str(), e.enabled, refused.as_deref()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            listed,
            [
                ("kb", true, None),
                ("keys", false, Some("name taken")),
                ("search", false, Some("name taken")),
                ("off", false, None),
                ("kb", false, Some("name taken")),
                ("evil", false, Some("bundle URL is not https")),
                ("Bad Name", false, Some("not a valid extension name")),
            ]
        );
        assert_eq!(r.extensions[3].description, "1.4.0 · kb");
        let Source::Addon { addon, setup, .. } = &r.extensions[3].source else {
            panic!("{:?}", r.extensions[3]);
        };
        assert_eq!((*addon, setup.as_deref()), ("team", Some("kb")));
        // Rows bring no lenses or actions; new rows replace the old.
        assert_eq!(names(&r.session_extensions()), ["keys"]);
        r.set_addon_rows(&[], &enabled);
        assert_eq!(r.extensions.len(), 3);
        // A row off by default stays off until `[extensions]` says on.
        let mut quiet = row("quiet", None);
        quiet.1.default_on = false;
        r.set_addon_rows(std::slice::from_ref(&quiet), &BTreeMap::new());
        assert!(!r.extensions[3].enabled);
        let on = BTreeMap::from([("quiet".to_owned(), true)]);
        r.set_addon_rows(&[quiet], &on);
        assert!(r.extensions[3].enabled);
    }

    #[test]
    fn the_session_context_stays_within_the_caps() {
        let mut ext: Vec<Extension> = (0..16)
            .map(|i| external(&format!("e{i}"), &format!("t{i}"), 'k'))
            .collect();
        ext[0].lenses[0].instruction = "i".repeat(2048);
        ext[1].lenses[0].instruction = "i".repeat(2048);
        ext[2].lenses[0].instruction = "i".repeat(2048);
        let r = Registry::new(&BTreeMap::new(), ext);
        let ctx = r.session_extensions();
        assert_eq!(ctx.validate(), Ok(()));
        assert_eq!(ctx.lenses.len(), 16);
        assert_eq!(&names(&ctx)[..3], ["t0", "t1", "t2"]);
    }
}
