//! Fixtures shared by the TUI's unit tests.

use std::collections::HashMap;

use super::app::{App, AppConfig, Mode};
use super::keymap::Keymap;

/// A manager on a 100×30 terminal with home `/home/u`, the default keys, and
/// nothing from the machine it runs on: no proxy profiles, no SSH hosts, no
/// saved state, no local claude.
pub fn config() -> AppConfig {
    AppConfig {
        mode: Mode::Manager,
        size: (100, 30),
        home: "/home/u".to_owned(),
        keymap: Keymap::default(),
        notify: true,
        proxy_override: Default::default(),
        proxy_profiles: Vec::new(),
        proxy_default: None,
        claude: Default::default(),
        local_claude: None,
        recent_dirs: HashMap::new(),
        claude_skipped: HashMap::new(),
        ssh_hosts: Vec::new(),
    }
}

/// An app made from [`config`].
pub fn app() -> App {
    App::new(config())
}
