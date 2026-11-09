//! The Vocdoni addon: the claude-proxy client and the team plugins.

pub mod daemon;
pub mod plugins;
pub mod proxy;
// P3.8a: the state lands before its runner (P3.8b), which removes this.
#[allow(dead_code)]
pub mod state;
#[cfg(test)]
pub(crate) mod test_support;

use crate::addon::{Addon, DaemonOps};

/// The addon's name: its config sections' owner and its daemon ops' prefix.
pub const NAME: &str = "vocdoni";

/// The Vocdoni addon.
pub struct Vocdoni {
    daemon: daemon::VocdoniDaemon,
}

impl Vocdoni {
    /// The addon as this build registers it.
    pub fn new() -> Vocdoni {
        Vocdoni::with_daemon(daemon::VocdoniDaemon::new())
    }

    /// The addon with `daemon` as its daemon side (tests give it their dirs).
    pub fn with_daemon(daemon: daemon::VocdoniDaemon) -> Vocdoni {
        Vocdoni { daemon }
    }
}

impl Addon for Vocdoni {
    fn name(&self) -> &'static str {
        NAME
    }

    fn config_sections(&self) -> &'static [&'static str] {
        &["proxy", "plugins"]
    }

    fn daemon(&self) -> Option<&dyn DaemonOps> {
        Some(&self.daemon)
    }
}
