//! `WebAddon`: the web UI as an addon. Its `Commands` claim `claudio web …`.

use std::process::ExitCode;
use std::sync::Arc;

use crate::addon::{Addon, Addons, Commands};

/// The web addon.
pub struct WebAddon;

impl Addon for WebAddon {
    fn name(&self) -> &'static str {
        "web"
    }

    fn commands(&self) -> Option<&dyn Commands> {
        Some(self)
    }
}

impl Commands for WebAddon {
    fn dispatch(&self, argv: &[String], addons: &Arc<Addons>) -> Option<ExitCode> {
        (argv.get(1).map(String::as_str) == Some("web"))
            .then(|| super::run(&argv[2..], Arc::clone(addons)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_claims_its_word_only() {
        let addons = Arc::new(Addons::empty());
        let argv = |w: &[&str]| -> Vec<String> {
            std::iter::once("claudio")
                .chain(w.iter().copied())
                .map(String::from)
                .collect()
        };
        assert!(WebAddon.dispatch(&argv(&[]), &addons).is_none());
        assert!(WebAddon.dispatch(&argv(&["webx"]), &addons).is_none());
        assert!(WebAddon
            .dispatch(&argv(&["--plain", "web"]), &addons)
            .is_none());
        // `claudio web --help` prints the usage and runs nothing.
        assert_eq!(
            WebAddon.dispatch(&argv(&["web", "--help"]), &addons),
            Some(ExitCode::SUCCESS)
        );
    }
}
