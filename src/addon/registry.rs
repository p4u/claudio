//! The registry: the immutable set of addons this binary runs with.

use std::collections::HashSet;
use std::process::ExitCode;
use std::sync::Arc;

use super::{feature, name_ok, Addon, FlagDecl, PanelDecl};

/// The `config.toml` tables the core owns; no addon may claim them.
pub const CORE_SECTIONS: &[&str] = &["ui", "keys", "update", "claude", "extensions"];

/// The core's command words; never an addon's name.
pub const RESERVED_WORDS: &[&str] = &[
    "__probe",
    "--slave",
    "__hook",
    "__context",
    "--daemon",
    "upgrade",
    "daemon",
    "sessions",
    "--plain",
];

/// The core's flags; never an addon's launch flag.
pub const CORE_FLAGS: &[&str] = &[
    "--plain", "--daemon", "--slave", "-p", "--print", "--api", "-h", "--help",
];

/// The immutable set of addons this binary runs with.
pub struct Addons {
    list: Vec<Arc<dyn Addon>>,
}

/// `--[a-z][a-z0-9-]*`.
fn flag_ok(flag: &str) -> bool {
    let Some(rest) = flag.strip_prefix("--") else {
        return false;
    };
    let mut c = rest.chars();
    c.next().is_some_and(|c| c.is_ascii_lowercase())
        && c.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Every name, section, flag and panel claim of `list`, checked together.
#[derive(Default)]
struct Claims {
    names: HashSet<&'static str>,
    sections: HashSet<&'static str>,
    flags: HashSet<&'static str>,
    panel_keys: HashSet<&'static str>,
}

impl Claims {
    fn add(&mut self, addon: &dyn Addon) -> Result<(), String> {
        let name = addon.name();
        if !name_ok(name) {
            return Err(format!("addon name {name:?} is not [a-z][a-z0-9-]{{0,31}}"));
        }
        if RESERVED_WORDS.contains(&name) {
            return Err(format!("addon name {name:?} is a reserved word"));
        }
        if !self.names.insert(name) {
            return Err(format!("addon {name:?} is registered twice"));
        }
        for &section in addon.config_sections() {
            if CORE_SECTIONS.contains(&section) {
                return Err(format!("addon {name}: [{section}] is a core section"));
            }
            if !self.sections.insert(section) {
                return Err(format!("addon {name}: [{section}] is claimed twice"));
            }
        }
        for flag in addon.launch().map_or(&[][..], |l| l.flags()) {
            self.flag(name, flag)?;
        }
        let panels = addon.ui().map_or(&[][..], |u| u.panels());
        let mut ids = HashSet::new();
        for panel in panels {
            if !name_ok(panel.id) || !ids.insert(panel.id) {
                return Err(format!(
                    "addon {name}: panel id {:?} is bad or repeated",
                    panel.id
                ));
            }
            if let Some(key) = panel.key {
                if !self.panel_keys.insert(key.action) {
                    return Err(format!(
                        "addon {name}: key action {:?} is bound twice",
                        key.action
                    ));
                }
            }
        }
        Ok(())
    }

    fn flag(&mut self, name: &str, flag: &FlagDecl) -> Result<(), String> {
        if !flag_ok(flag.name) {
            return Err(format!(
                "addon {name}: flag {:?} is not --[a-z][a-z0-9-]*",
                flag.name
            ));
        }
        if CORE_FLAGS.contains(&flag.name) {
            return Err(format!("addon {name}: {} is a core flag", flag.name));
        }
        if !self.flags.insert(flag.name) {
            return Err(format!("addon {name}: {} is declared twice", flag.name));
        }
        Ok(())
    }
}

impl Addons {
    /// Refuses: a bad or repeated name or a reserved word; a core or twice-claimed config
    /// section; a launch flag not `--[a-z][a-z0-9-]*`, repeated, or a core flag; a bad or
    /// repeated panel id, or a panel key action named twice.
    pub fn new(list: Vec<Arc<dyn Addon>>) -> Result<Addons, String> {
        let mut claims = Claims::default();
        for addon in &list {
            claims.add(addon.as_ref())?;
        }
        Ok(Addons { list })
    }

    /// No addons: a core-only claudio.
    pub fn empty() -> Addons {
        Addons { list: Vec::new() }
    }

    /// The addons, in registry order.
    pub fn iter(&self) -> impl Iterator<Item = &Arc<dyn Addon>> {
        self.list.iter()
    }

    /// The addon named `name`.
    pub fn get(&self, name: &str) -> Option<&Arc<dyn Addon>> {
        self.list.iter().find(|a| a.name() == name)
    }

    /// The first addon whose `is_relay` takes `argv` runs it (a relay it
    /// then declines to dispatch fails rather than reaching passthrough).
    pub fn relay(self: &Arc<Self>, argv: &[String]) -> Option<ExitCode> {
        let commands = self
            .list
            .iter()
            .filter_map(|a| a.commands())
            .find(|c| c.is_relay(argv))?;
        Some(commands.dispatch(argv, self).unwrap_or(ExitCode::FAILURE))
    }

    /// The first addon, in registry order, whose `dispatch` takes `argv`.
    pub fn dispatch(self: &Arc<Self>, argv: &[String]) -> Option<ExitCode> {
        self.list
            .iter()
            .filter_map(|a| a.commands())
            .find_map(|c| c.dispatch(argv, self))
    }

    /// The addons' `--help` appendices, in registry order.
    pub fn help(&self) -> String {
        let parts: Vec<&str> = self
            .list
            .iter()
            .filter_map(|a| a.commands())
            .map(|c| c.help())
            .filter(|h| !h.is_empty())
            .collect();
        parts.join("\n")
    }

    /// `addon:<name>` for each addon with `daemon()`.
    pub fn daemon_features(&self) -> Vec<String> {
        self.list
            .iter()
            .filter(|a| a.daemon().is_some())
            .map(|a| feature(a.name()))
            .collect()
    }

    /// `(addon, panel)` for every panel, in registry order.
    pub fn panels(&self) -> Vec<(&'static str, PanelDecl)> {
        self.list
            .iter()
            .flat_map(|a| {
                let panels = a.ui().map_or(&[][..], |u| u.panels());
                panels.iter().map(move |p| (a.name(), *p))
            })
            .collect()
    }

    /// `(addon, flag)` for every launch flag.
    pub fn launch_flags(&self) -> Vec<(&'static str, FlagDecl)> {
        self.list
            .iter()
            .flat_map(|a| {
                let flags = a.launch().map_or(&[][..], |l| l.flags());
                flags.iter().map(move |f| (a.name(), *f))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addon::test_support::TestAddon;
    use crate::addon::PanelKey;

    fn reg(list: Vec<TestAddon>) -> Result<Addons, String> {
        Addons::new(
            list.into_iter()
                .map(|a| Arc::new(a) as Arc<dyn Addon>)
                .collect(),
        )
    }

    fn switch(name: &'static str) -> FlagDecl {
        FlagDecl {
            name,
            value: None,
            help: "",
            label: None,
        }
    }

    fn panel(id: &'static str, action: Option<&'static str>) -> PanelDecl {
        PanelDecl {
            id,
            title: "P",
            key: action.map(|action| PanelKey {
                action,
                default: "alt+s",
            }),
            windows: &[],
        }
    }

    #[test]
    fn registry_refuses_duplicates_reserved_words_core_sections_and_shared_sections() {
        assert!(reg(vec![TestAddon::new("a"), TestAddon::new("b")]).is_ok());
        let twice = reg(vec![TestAddon::new("a"), TestAddon::new("a")]);
        assert!(twice.err().unwrap().contains("twice"));
        for bad in ["A", "1a", "", "a_b"] {
            assert!(reg(vec![TestAddon::new(bad)]).is_err(), "{bad}");
        }
        for word in ["upgrade", "daemon", "sessions"] {
            let e = reg(vec![TestAddon::new(word)]).err().unwrap();
            assert!(e.contains("reserved"), "{e}");
        }
        for core in CORE_SECTIONS {
            let one = TestAddon::new("a").sections(Box::leak(Box::new([*core])));
            let e = reg(vec![one]).err().unwrap();
            assert!(e.contains("core section"), "{e}");
        }
        let shared = reg(vec![
            TestAddon::new("a").sections(&["proxy"]),
            TestAddon::new("b").sections(&["plugins", "proxy"]),
        ]);
        assert!(shared.err().unwrap().contains("[proxy] is claimed twice"));
        let own_twice = reg(vec![TestAddon::new("a").sections(&["x", "x"])]);
        assert!(own_twice.is_err());
        assert!(reg(vec![
            TestAddon::new("a").sections(&["proxy"]),
            TestAddon::new("b").sections(&["plugins"]),
        ])
        .is_ok());
    }

    #[test]
    fn registry_refuses_bad_or_repeated_flags_and_panel_keys() {
        let flags = |list: &'static [FlagDecl]| TestAddon::new("a").launch_flags(list);
        assert!(reg(vec![flags(Box::leak(Box::new([
            switch("--proxy"),
            switch("--x-1")
        ])))])
        .is_ok());
        for bad in ["proxy", "-x", "--", "--X", "--1a", "--a_b", "--a b"] {
            let list: &'static [FlagDecl] = Box::leak(Box::new([switch(bad)]));
            assert!(reg(vec![flags(list)]).is_err(), "{bad}");
        }
        for core in [
            "--plain", "--daemon", "--slave", "--print", "--api", "--help",
        ] {
            let list: &'static [FlagDecl] = Box::leak(Box::new([switch(core)]));
            let e = reg(vec![flags(list)]).err().unwrap();
            assert!(e.contains("core flag"), "{e}");
        }
        let repeated = reg(vec![
            TestAddon::new("a").launch_flags(Box::leak(Box::new([switch("--x")]))),
            TestAddon::new("b").launch_flags(Box::leak(Box::new([switch("--x")]))),
        ]);
        assert!(repeated.err().unwrap().contains("declared twice"));

        let panels = |name, list: &'static [PanelDecl]| TestAddon::new(name).panels(list);
        let ok = reg(vec![panels(
            "a",
            Box::leak(Box::new([panel("stats", Some("s"))])),
        )]);
        assert!(ok.is_ok());
        let bad_id = reg(vec![panels(
            "a",
            Box::leak(Box::new([panel("Stats", None)])),
        )]);
        assert!(bad_id.is_err());
        let same_id = reg(vec![panels(
            "a",
            Box::leak(Box::new([panel("p", None), panel("p", None)])),
        )]);
        assert!(same_id.is_err());
        // The same id in two addons is fine; the same key action is not.
        assert!(reg(vec![
            panels("a", Box::leak(Box::new([panel("p", Some("s"))]))),
            panels("b", Box::leak(Box::new([panel("p", Some("t"))]))),
        ])
        .is_ok());
        let same_key = reg(vec![
            panels("a", Box::leak(Box::new([panel("p", Some("s"))]))),
            panels("b", Box::leak(Box::new([panel("q", Some("s"))]))),
        ]);
        assert!(same_key.err().unwrap().contains("bound twice"));
    }

    #[test]
    fn registry_lists_daemon_features_only_for_daemon_addons() {
        assert!(Addons::empty().daemon_features().is_empty());
        let addons = reg(vec![
            TestAddon::new("plain"),
            TestAddon::new("echo").with_daemon(),
            TestAddon::new("other").with_daemon(),
        ])
        .unwrap();
        assert_eq!(addons.daemon_features(), ["addon:echo", "addon:other"]);
        assert!(addons.get("echo").is_some() && addons.get("nope").is_none());
        assert_eq!(addons.iter().count(), 3);
        let flagged = reg(vec![
            TestAddon::new("a").launch_flags(Box::leak(Box::new([switch("--one")]))),
            TestAddon::new("b").launch_flags(Box::leak(Box::new([switch("--two")]))),
        ])
        .unwrap();
        let names: Vec<_> = flagged
            .launch_flags()
            .into_iter()
            .map(|(a, f)| (a, f.name))
            .collect();
        assert_eq!(names, [("a", "--one"), ("b", "--two")]);
    }

    #[test]
    fn relay_runs_before_dispatch_and_dispatch_follows_registry_order() {
        let first = Arc::new(
            TestAddon::new("first")
                .command("go", 3)
                .help_text("first help"),
        );
        let second = Arc::new(
            TestAddon::new("second")
                .command("go", 4)
                .command("own", 5)
                .relay("Hook", 6)
                .help_text("second help"),
        );
        let addons = Arc::new(
            Addons::new(vec![
                Arc::clone(&first) as Arc<dyn Addon>,
                Arc::clone(&second) as Arc<dyn Addon>,
            ])
            .unwrap(),
        );
        let argv = |words: &[&str]| -> Vec<String> {
            std::iter::once("claudio")
                .chain(words.iter().copied())
                .map(String::from)
                .collect()
        };
        // A relay is answered by the addon that claims it, before dispatch.
        assert_eq!(addons.relay(&argv(&["Hook", "1"])), Some(ExitCode::from(6)));
        assert_eq!(addons.relay(&argv(&["go"])), None);
        // Dispatch: the first in registry order wins.
        assert_eq!(addons.dispatch(&argv(&["go"])), Some(ExitCode::from(3)));
        assert_eq!(addons.dispatch(&argv(&["own"])), Some(ExitCode::from(5)));
        assert_eq!(addons.dispatch(&argv(&["nobody"])), None);
        assert_eq!(
            first.calls(),
            [
                "is_relay",
                "is_relay",
                "dispatch go",
                "dispatch own",
                "dispatch nobody"
            ]
        );
        assert_eq!(
            second.calls(),
            [
                "is_relay",
                "dispatch Hook",
                "is_relay",
                "dispatch own",
                "dispatch nobody"
            ]
        );
        assert_eq!(addons.help(), "first help\nsecond help");
        assert_eq!(Addons::empty().help(), "");
    }
}
