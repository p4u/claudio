//! The new-session wizard's route row: direct (`none`) and every Launch
//! addon's option (the proxy profiles), cycled with Left/Right. Pure.

use crate::addon::{LaunchOption, LaunchSelect, Pick};

/// The route row of the wizard.
#[derive(Debug, Clone, Default)]
pub struct Routes {
    /// What the row shows: `none` (direct) at index 0, then each option.
    pub labels: Vec<String>,
    /// The pick behind each label.
    picks: Vec<Pick>,
    /// The options' group ("Proxy").
    pub group: String,
    /// The switches every pick keeps (the run's).
    flags: Vec<String>,
    /// The selected label.
    pub selected: usize,
}

impl Routes {
    /// `options`, `select`'s pick selected (else direct), its switches kept.
    pub fn new(options: &[(&'static str, LaunchOption)], select: &LaunchSelect) -> Routes {
        let mut routes = Routes {
            flags: select.flags.clone(),
            ..Routes::default()
        };
        routes.set(options, &select.pick);
        routes
    }

    /// Fresher options, `pick` selected (else direct).
    pub fn set(&mut self, options: &[(&'static str, LaunchOption)], pick: &Pick) {
        (self.labels, self.picks) = (vec!["none".to_owned()], vec![Pick::Direct]);
        for (addon, o) in options {
            self.labels.push(o.label.clone());
            let (addon, option) = (addon.to_string(), o.id.clone());
            self.picks.push(Pick::Option { addon, option });
        }
        self.group = options
            .first()
            .map_or_else(String::new, |(_, o)| o.group.clone());
        self.selected = self.picks.iter().position(|p| p == pick).unwrap_or(0);
    }

    /// The selected label.
    pub fn label(&self) -> &str {
        self.labels
            .get(self.selected)
            .map_or("none", String::as_str)
    }

    /// Whether there is anything to choose (an option besides direct).
    pub fn cycles(&self) -> bool {
        self.labels.len() > 1
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
            pick: pick.unwrap_or(Pick::Direct),
            flags: self.flags.clone(),
        }
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
}
