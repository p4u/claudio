//! The new-session wizard's route row: direct (`none`) and every Launch
//! addon's option (the proxy profiles), cycled with Left/Right. Pure.
//!
//! A route the options do not list is kept, never turned into direct
//! (R18): a chosen option they lack is shown unavailable and Enter refuses
//! it; `Pick::Default` without an option marked default stays the default,
//! for the launch composer to resolve (an addon's configured default the
//! options lack fails there).

use crate::addon::{line, LaunchOption, LaunchSelect, Pick};

/// The most bytes of an unavailable option's id shown.
const ID_BYTES: usize = 64;

/// The route row of the wizard.
#[derive(Debug, Clone, Default)]
pub struct Routes {
    /// What the row shows: a kept route the options lack first, then `none`
    /// (direct), then each option.
    pub labels: Vec<String>,
    /// The pick behind each label.
    picks: Vec<Pick>,
    /// Whether `picks[0]` is a kept route the options lack.
    kept: bool,
    /// The options' group ("Proxy").
    pub group: String,
    /// The switches every pick keeps (the run's).
    flags: Vec<String>,
    /// The selected label.
    pub selected: usize,
}

impl Routes {
    /// `options`, `select`'s pick selected (kept when they lack it), its switches kept.
    pub fn new(options: &[(&'static str, LaunchOption)], select: &LaunchSelect) -> Routes {
        let mut routes = Routes {
            flags: select.flags.clone(),
            ..Routes::default()
        };
        routes.set(options, &select.pick);
        routes
    }

    /// Fresher options, `pick` selected: the option marked default for
    /// `Pick::Default`, and a pick they lack kept as its own entry.
    pub fn set(&mut self, options: &[(&'static str, LaunchOption)], pick: &Pick) {
        let marked = options.iter().find(|(_, o)| o.default);
        let pick = match (pick, marked) {
            (Pick::Default, Some((addon, o))) => Pick::Option {
                addon: addon.to_string(),
                option: o.id.clone(),
            },
            (pick, _) => pick.clone(),
        };
        (self.labels, self.picks) = (vec!["none".to_owned()], vec![Pick::Direct]);
        for (addon, o) in options {
            self.labels.push(o.label.clone());
            let (addon, option) = (addon.to_string(), o.id.clone());
            self.picks.push(Pick::Option { addon, option });
        }
        self.kept = !self.picks.contains(&pick);
        if self.kept {
            self.labels.insert(0, kept_label(&pick));
            self.picks.insert(0, pick.clone());
        }
        self.group = options
            .first()
            .map_or_else(String::new, |(_, o)| o.group.clone());
        self.selected = self.picks.iter().position(|p| *p == pick).unwrap_or(0);
    }

    /// The selected label.
    pub fn label(&self) -> &str {
        self.labels
            .get(self.selected)
            .map_or("none", String::as_str)
    }

    /// Whether there is anything to choose: an option, listed or kept.
    pub fn cycles(&self) -> bool {
        self.picks.iter().any(|p| matches!(p, Pick::Option { .. }))
    }

    /// The previous route (the last one after the first).
    pub fn left(&mut self) {
        let len = self.labels.len().max(1);
        self.selected = (self.selected + len - 1) % len;
    }

    /// The next route (the first one after the last).
    pub fn right(&mut self) {
        self.selected = (self.selected + 1) % self.labels.len().max(1);
    }

    /// The selected route, with the run's switches.
    pub fn select(&self) -> LaunchSelect {
        let pick = self.picks.get(self.selected).cloned();
        LaunchSelect {
            pick: pick.unwrap_or(Pick::Default),
            flags: self.flags.clone(),
        }
    }

    /// Why the selected route cannot launch: an option the options lack.
    pub fn unavailable(&self) -> Option<String> {
        match self.picks.get(self.selected) {
            Some(Pick::Option { option, .. }) if self.kept && self.selected == 0 => Some(format!(
                "{} is not available: pick another route (←/→)",
                line(option, ID_BYTES)
            )),
            _ => None,
        }
    }
}

/// How a kept route the options lack is shown.
fn kept_label(pick: &Pick) -> String {
    match pick {
        Pick::Option { option, .. } => format!("{} (unavailable)", line(option, ID_BYTES)),
        Pick::Default => "default".to_owned(),
        Pick::Direct => "none".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn option(id: &str, default: bool) -> (&'static str, LaunchOption) {
        let o = LaunchOption {
            id: id.into(),
            label: id.into(),
            group: "Proxy".into(),
            default,
            detail: None,
        };
        ("a", o)
    }

    #[test]
    fn routes_cycle_and_select_with_the_runs_switches() {
        let options = [option("work", true), option("lab", false)];
        let select = LaunchSelect {
            pick: Pick::Option {
                addon: "a".into(),
                option: "lab".into(),
            },
            flags: vec!["first-party".into()],
        };
        let mut r = Routes::new(&options, &select);
        assert_eq!(r.labels, ["none", "work", "lab"]);
        assert_eq!(r.label(), "lab");
        assert_eq!((r.selected, r.group.as_str()), (2, "Proxy"));
        assert_eq!(r.select(), select);
        r.right();
        assert_eq!(r.select().pick, Pick::Direct);
        assert_eq!(r.select().flags, ["first-party"]);
        r.left();
        r.left();
        assert_eq!(r.selected, 1);
        // Fresher options keep a pick still there, else fall back to direct.
        r.set(&options[1..], &Pick::Direct);
        assert_eq!((r.labels.len(), r.selected), (2, 0));
        assert!(!Routes::new(&[], &LaunchSelect::default()).cycles());
    }

    /// The App around the wizard: what an open wizard's Enter launches.
    mod app {
        use crate::addon::{LaunchOption, LaunchSelect, Pick};
        use crate::proto::Msg;
        use crate::tui::app::{App, Effect};
        use crate::tui::test_support;
        use crate::tui::wizard::Outcome;

        fn option(id: &str, default: bool) -> (&'static str, LaunchOption) {
            let o = LaunchOption {
                id: id.into(),
                label: id.into(),
                group: "Proxy".into(),
                default,
                detail: None,
            };
            ("vocdoni", o)
        }

        fn profile(option: &str) -> Pick {
            Pick::Option {
                addon: "vocdoni".into(),
                option: option.into(),
            }
        }

        /// A manager with no session (its wizard closed), `options` and the run's `pick`.
        fn manager(options: Vec<(&'static str, LaunchOption)>, pick: Pick) -> App {
            let mut app = test_support::app_with(&[]);
            app.modal = None;
            app.launch_options = options;
            app.launch_default = LaunchSelect {
                pick,
                flags: Vec::new(),
            };
            app.open_wizard();
            app.take_effects();
            app
        }

        /// The open wizard's route: its label and what Enter would launch.
        fn route(app: &mut App) -> (String, Pick) {
            let w = app.wizard_mut().expect("wizard open");
            (w.route.label().to_owned(), w.route.select().pick)
        }

        /// Enter on the wizard: the launches it queued (never a direct
        /// `Spawn` request: a claude session goes through the composer).
        fn enter(app: &mut App) -> Vec<LaunchSelect> {
            let select = app.wizard_mut().expect("wizard open").route.select();
            let outcome = Outcome::Spawn {
                cwd: "/srv/app".into(),
                resume: None,
                select,
            };
            app.wizard_outcome(outcome);
            let effects = app.take_effects();
            assert!(
                !effects.iter().any(|e| matches!(
                    e,
                    Effect::Request {
                        msg: Msg::Spawn(_),
                        ..
                    }
                )),
                "{effects:?}"
            );
            test_support::launches(&effects)
                .into_iter()
                .map(|c| c.select.clone())
                .collect()
        }

        fn notice(app: &App) -> String {
            app.notice
                .as_ref()
                .map(|n| n.text.clone())
                .unwrap_or_default()
        }

        /// R18: `--proxy gone` with no active session is shown as
        /// unavailable and refused, never the `none` route; the member's
        /// explicit `none` then starts direct.
        #[test]
        fn a_missing_explicit_profile_is_refused_never_direct() {
            let mut app = manager(vec![option("work", false)], profile("gone"));
            assert_eq!(
                route(&mut app),
                ("gone (unavailable)".into(), profile("gone"))
            );
            assert!(enter(&mut app).is_empty(), "nothing launched");
            assert!(app.wizard_mut().is_some(), "the wizard stays open");
            assert!(notice(&app).contains("gone"), "{}", notice(&app));
            // Without any option at all, the same.
            let mut app = manager(Vec::new(), profile("gone"));
            assert_eq!(route(&mut app).1, profile("gone"));
            assert!(enter(&mut app).is_empty());
            // The member picks direct explicitly.
            let mut app = manager(vec![option("work", false)], profile("gone"));
            app.wizard_mut().unwrap().route.right();
            assert_eq!(route(&mut app), ("none".into(), Pick::Direct));
            let launched = enter(&mut app);
            assert_eq!(launched.len(), 1);
            assert_eq!(launched[0].pick, Pick::Direct);
            assert!(app.wizard_mut().is_none());
        }

        /// R18: with no option marked default (a configured default that
        /// is not among the options, or none read), the launch keeps
        /// `Pick::Default` for the composer to resolve, never direct.
        #[test]
        fn a_default_absent_from_the_options_is_kept_never_direct() {
            for options in [vec![option("work", false)], Vec::new()] {
                let mut app = manager(options, Pick::Default);
                assert_eq!(route(&mut app).1, Pick::Default);
                let launched = enter(&mut app);
                assert_eq!(launched.len(), 1);
                assert_eq!(launched[0].pick, Pick::Default);
            }
            // A default among the options is that option.
            let mut app = manager(
                vec![option("lab", false), option("work", true)],
                Pick::Default,
            );
            assert_eq!(route(&mut app), ("work".into(), profile("work")));
        }

        /// R18: options that lose the selected one while the wizard is open
        /// keep it, unavailable and refused; when it comes back it resolves.
        #[test]
        fn options_that_disappear_keep_the_selection_unavailable() {
            let both = vec![option("work", false), option("lab", false)];
            let mut app = manager(both.clone(), profile("lab"));
            assert_eq!(route(&mut app), ("lab".into(), profile("lab")));
            app.on_launch_options(vec![option("work", false)]);
            assert_eq!(
                route(&mut app),
                ("lab (unavailable)".into(), profile("lab"))
            );
            assert!(enter(&mut app).is_empty(), "nothing launched");
            // An empty list (profiles unreadable) keeps it too.
            app.on_launch_options(Vec::new());
            assert_eq!(route(&mut app).1, profile("lab"));
            assert!(enter(&mut app).is_empty());
            app.on_launch_options(both);
            assert_eq!(route(&mut app), ("lab".into(), profile("lab")));
            let launched = enter(&mut app);
            assert_eq!(launched.len(), 1);
            assert_eq!(launched[0].pick, profile("lab"));
        }
    }
}
