//! The extension registry: what claudio renders (lenses) and what the
//! leader key (Alt+z) offers (actions), from built-in extensions and from
//! external ones (`~/.config/claudio/extensions/<name>/`, see
//! [`manifest`]).
//!
//! Pure: the manifests are read by `tui/mod.rs` at start, external programs
//! run in the extension host it drives, and built-in renderers on the
//! render worker. Everything else in the TUI asks the registry: which tag
//! lenses the pane and the reader render, which fence lenses the reader
//! draws, the context a spawn carries ([`Registry::session_extensions`]) and
//! the leader's actions.

pub mod host;
pub mod manifest;
pub mod protocol;

use std::collections::BTreeMap;

use crate::proto::{LensKind, LensSpec, SessionExtensions, MAX_INSTRUCTIONS, MAX_LENSES};
use crate::proxy::plugins::PluginList;

/// The diagram extension's instruction (spec §7).
pub const DIAGRAM_CONTEXT: &str = "This terminal draws Mermaid diagrams. When a diagram \
helps (architecture, data or control flow, state machines, message sequences), write it as \
a fenced code block with the language `mermaid`, using `flowchart TD`/`flowchart LR` or \
`sequenceDiagram`. Keep labels short (under 30 characters) and diagrams small (under 20 \
nodes). Do not draw box diagrams with characters yourself.";

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
    /// A proxy plugin of `profile`: a bundle claude loads, and env. No
    /// lenses or actions. `refused` says why it is not used, if it is not.
    Plugin {
        profile: String,
        refused: Option<String>,
    },
}

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
    pub fn is_external(&self) -> bool {
        matches!(self.source, Source::External(_))
    }

    pub fn is_plugin(&self) -> bool {
        matches!(self.source, Source::Plugin { .. })
    }
}

/// The built-in extensions, in registry order, all enabled.
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
    let lens = |name: &str, kind, instruction: &str| Lens {
        name: name.to_owned(),
        kind,
        instruction: instruction.to_owned(),
    };
    let mut latex = builtin("latex", "math typeset from <latex> tags");
    latex.lenses = vec![lens(
        "latex",
        LensKind::Tag,
        crate::claude::hooks::LATEX_CONTEXT,
    )];
    let mut diagram = builtin("diagram", "Mermaid diagrams drawn in the reader");
    diagram.lenses = vec![lens("mermaid", LensKind::Fence, DIAGRAM_CONTEXT)];
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
    vec![latex, diagram, search, snippets]
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
    /// (`enabled`) applied; `[claude] latex = false` (`latex`) turns latex
    /// off too. A name in `enabled` that matches nothing gets a notice.
    pub fn new(
        enabled: &BTreeMap<String, bool>,
        latex: bool,
        external: Vec<Extension>,
        notices: &mut Vec<String>,
    ) -> Registry {
        let mut extensions = builtins();
        extensions.extend(external);
        for ext in &mut extensions {
            if let Some(on) = enabled.get(&ext.name) {
                ext.enabled = *on;
            }
            if ext.name == "latex" && !latex {
                ext.enabled = false;
            }
        }
        for name in enabled.keys() {
            if !extensions.iter().any(|e| &e.name == name) {
                notices.push(format!(
                    "config.toml [extensions]: no extension named '{name}'"
                ));
            }
        }
        Registry { extensions }
    }

    /// Replace the proxy plugins with those of `lists`, after the built-ins
    /// and the externals, with `[extensions]` (`enabled`) applied. A plugin
    /// [`PluginList::checked`] refuses, or whose name another extension
    /// (or an earlier list) has, is listed as refused and off.
    pub fn set_plugins(&mut self, lists: &[&PluginList], enabled: &BTreeMap<String, bool>) {
        self.extensions.retain(|e| !e.is_plugin());
        for list in lists {
            for (p, why) in list.checked() {
                let taken = self.extensions.iter().any(|e| e.name == p.name);
                let refused = why.or_else(|| taken.then(|| "name taken".to_owned()));
                let on = refused.is_none() && enabled.get(&p.name).copied().unwrap_or(true);
                self.extensions.push(Extension {
                    name: p.name.clone(),
                    description: format!("{} · {}", p.version, list.profile),
                    enabled: on,
                    lenses: Vec::new(),
                    actions: Vec::new(),
                    source: Source::Plugin {
                        profile: list.profile.clone(),
                        refused,
                    },
                });
            }
        }
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

    /// The enabled lenses of `kind`, with their extension, in order.
    pub fn lenses(&self, kind: LensKind) -> Vec<(&Extension, &Lens)> {
        self.enabled()
            .flat_map(|e| e.lenses.iter().map(move |l| (e, l)))
            .filter(|(_, l)| l.kind == kind)
            .collect()
    }

    /// The names of the enabled tag lenses.
    pub fn tag_lenses(&self) -> Vec<&str> {
        let lenses = self.lenses(LensKind::Tag);
        lenses.into_iter().map(|(_, l)| l.name.as_str()).collect()
    }

    /// The names of the enabled fence lenses.
    pub fn fence_lenses(&self) -> Vec<&str> {
        let lenses = self.lenses(LensKind::Fence);
        lenses.into_iter().map(|(_, l)| l.name.as_str()).collect()
    }

    /// The external extension that owns enabled lens `name`, if one does.
    pub fn external_lens(&self, name: &str) -> Option<&Extension> {
        self.enabled()
            .find(|e| e.lenses.iter().any(|l| l.name == name))
            .filter(|e| e.is_external())
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
            let size = crate::claude::latex::sanitize(&lens.instruction).len();
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
        for i in 0..MAX_LENSES {
            r.extensions
                .push(external(&format!("x{i}"), &format!("t{i}"), 'a'));
        }
        // 2 built-in lenses + MAX_LENSES: the last two do not fit.
        let last = format!("t{}", MAX_LENSES - 1);
        let before = format!("t{}", MAX_LENSES - 2);
        assert_eq!(r.dropped_lenses(), [before, last]);
        assert_eq!(r.session_extensions().lenses.len(), MAX_LENSES);
    }

    #[test]
    fn the_builtins_give_latex_and_mermaid_and_the_leader_actions() {
        let r = Registry::default();
        assert_eq!(r.tag_lenses(), ["latex"]);
        assert_eq!(r.fence_lenses(), ["mermaid"]);
        let ext = r.session_extensions();
        assert_eq!(names(&ext), ["latex", "mermaid"]);
        assert_eq!(
            ext.lenses[0].instruction,
            crate::claude::hooks::LATEX_CONTEXT
        );
        assert_eq!(ext.lenses[1].kind, LensKind::Fence);
        assert_eq!(ext.validate(), Ok(()));
        let keys: Vec<char> = r.actions().iter().map(|(_, a)| a.key).collect();
        assert_eq!(keys, ['s', 'p', 'P']);
        assert!(r.external_lens("latex").is_none());
    }

    #[test]
    fn config_turns_extensions_off_and_names_unknown_ones() {
        let mut notices = Vec::new();
        let enabled = BTreeMap::from([
            ("diagram".to_owned(), false),
            ("snippets".to_owned(), false),
            ("nope".to_owned(), false),
        ]);
        let r = Registry::new(&enabled, true, vec![], &mut notices);
        assert_eq!(names(&r.session_extensions()), ["latex"]);
        assert!(r.fence_lenses().is_empty());
        let keys: Vec<char> = r.actions().iter().map(|(_, a)| a.key).collect();
        assert_eq!(keys, ['s']);
        assert_eq!(
            notices,
            ["config.toml [extensions]: no extension named 'nope'"]
        );
        // `[claude] latex = false` is the latex extension off.
        let r = Registry::new(&BTreeMap::new(), false, vec![], &mut notices);
        assert!(r.tag_lenses().is_empty());
        assert_eq!(names(&r.session_extensions()), ["mermaid"]);
    }

    #[test]
    fn external_extensions_follow_the_builtins() {
        let mut notices = Vec::new();
        let ext = vec![external("keys", "keys", 'k')];
        let r = Registry::new(&BTreeMap::new(), true, ext, &mut notices);
        assert_eq!(r.tag_lenses(), ["latex", "keys"]);
        assert_eq!(names(&r.session_extensions()), ["latex", "mermaid", "keys"]);
        assert_eq!(
            r.external_lens("keys").map(|e| e.name.as_str()),
            Some("keys")
        );
        let keys: Vec<char> = r.actions().iter().map(|(_, a)| a.key).collect();
        assert_eq!(keys, ['s', 'p', 'P', 'k']);
        let off = BTreeMap::from([("keys".to_owned(), false)]);
        let r = Registry::new(
            &off,
            true,
            vec![external("keys", "keys", 'k')],
            &mut notices,
        );
        assert!(r.external_lens("keys").is_none());
        assert!(notices.is_empty());
    }

    #[test]
    fn an_extension_turned_off_and_on_again_takes_its_lenses_and_actions_along() {
        let ext = vec![external("keys", "keys", 'k')];
        let mut r = Registry::new(&BTreeMap::new(), true, ext, &mut Vec::new());
        assert!(r.set_enabled("keys", false));
        assert!(r.set_enabled("diagram", false));
        assert_eq!(r.tag_lenses(), ["latex"]);
        assert!(r.fence_lenses().is_empty());
        assert!(r.external_lens("keys").is_none());
        assert_eq!(names(&r.session_extensions()), ["latex"]);
        let keys: Vec<char> = r.actions().iter().map(|(_, a)| a.key).collect();
        assert_eq!(keys, ['s', 'p', 'P']);
        // Still listed, off.
        assert_eq!(r.extensions.len(), 5);
        assert!(r.set_enabled("keys", true));
        assert!(r.set_enabled("diagram", true));
        assert_eq!(names(&r.session_extensions()), ["latex", "mermaid", "keys"]);
        let keys: Vec<char> = r.actions().iter().map(|(_, a)| a.key).collect();
        assert_eq!(keys, ['s', 'p', 'P', 'k']);
        assert!(r.external_lens("keys").is_some());
        assert!(!r.set_enabled("nope", true));
    }

    #[test]
    fn plugins_follow_the_others_and_a_taken_name_is_refused() {
        use crate::proxy::plugins::sample;
        let ext = vec![external("keys", "keys", 'k')];
        let mut r = Registry::new(&BTreeMap::new(), true, ext, &mut Vec::new());
        let work = PluginList {
            profile: "work".into(),
            fetched_at: 1,
            plugins: vec![sample("kb"), sample("keys"), sample("latex"), sample("off")],
        };
        let mut bad = sample("evil");
        bad.bundle_url = "http://e.org/x.zip".into();
        let home = PluginList {
            profile: "home".into(),
            fetched_at: 1,
            plugins: vec![sample("kb"), bad],
        };
        let enabled = BTreeMap::from([("off".to_owned(), false)]);
        r.set_plugins(&[&work, &home], &enabled);
        let plugins: Vec<(&str, bool, Option<&str>)> = r
            .extensions
            .iter()
            .filter_map(|e| match &e.source {
                Source::Plugin { refused, .. } => {
                    Some((e.name.as_str(), e.enabled, refused.as_deref()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            plugins,
            [
                ("kb", true, None),
                ("keys", false, Some("name taken")),
                ("latex", false, Some("name taken")),
                ("off", false, None),
                ("kb", false, Some("name taken")),
                ("evil", false, Some("bundle URL is not https")),
            ]
        );
        assert_eq!(r.extensions[5].description, "1.4.0 · work");
        // Plugins bring no lenses or actions; a new list replaces the old.
        assert_eq!(names(&r.session_extensions()), ["latex", "mermaid", "keys"]);
        r.set_plugins(&[], &enabled);
        assert_eq!(r.extensions.len(), 5);
    }

    #[test]
    fn the_session_context_stays_within_the_caps() {
        let mut ext: Vec<Extension> = (0..16)
            .map(|i| external(&format!("e{i}"), &format!("t{i}"), 'k'))
            .collect();
        ext[0].lenses[0].instruction = "i".repeat(2048);
        ext[1].lenses[0].instruction = "i".repeat(2048);
        ext[2].lenses[0].instruction = "i".repeat(2048);
        let r = Registry::new(&BTreeMap::new(), true, ext, &mut Vec::new());
        let ctx = r.session_extensions();
        assert_eq!(ctx.validate(), Ok(()));
        assert_eq!(ctx.lenses.len(), 16);
        assert_eq!(&names(&ctx)[..3], ["latex", "mermaid", "t0"]);
    }
}
