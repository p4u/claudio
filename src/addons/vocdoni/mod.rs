//! The Vocdoni addon: the claude-proxy client and the team plugins.

pub mod bridge;
pub mod daemon;
pub mod launch;
pub mod panel;
pub mod plugins;
pub mod profiles;
pub mod proxy;
mod rows;
pub mod runner;
pub mod state;
#[cfg(test)]
pub(crate) mod test_support;
pub mod ui;
pub mod v1;

use std::sync::Arc;

use crate::addon::{Addon, AddonCtx, AddonEvent, DaemonOps, Launch, Ui};

/// The addon's name: its config sections' owner and its daemon ops' prefix.
pub const NAME: &str = "vocdoni";

/// The Vocdoni addon.
pub struct Vocdoni {
    daemon: daemon::VocdoniDaemon,
    /// The client side: the plugin state and the proxy config cache.
    runner: Arc<runner::Runner>,
}

impl Vocdoni {
    /// The addon as this build registers it.
    pub fn new() -> Vocdoni {
        Vocdoni::with_daemon(daemon::VocdoniDaemon::new())
    }

    /// The addon with `runner` as its client side (tests give it their files).
    #[cfg(test)]
    pub fn with_runner(runner: Arc<runner::Runner>) -> Vocdoni {
        Vocdoni {
            daemon: daemon::VocdoniDaemon::new(),
            runner,
        }
    }

    /// The addon with `daemon` as its daemon side (tests give it their dirs).
    pub fn with_daemon(daemon: daemon::VocdoniDaemon) -> Vocdoni {
        Vocdoni {
            daemon,
            runner: Arc::default(),
        }
    }
}

impl Addon for Vocdoni {
    fn name(&self) -> &'static str {
        NAME
    }

    fn config_sections(&self) -> &'static [&'static str] {
        &["proxy", "plugins"]
    }

    fn launch(&self) -> Option<&dyn Launch> {
        Some(self)
    }

    fn ui(&self) -> Option<&dyn Ui> {
        Some(self)
    }

    fn daemon(&self) -> Option<&dyn DaemonOps> {
        Some(&self.daemon)
    }

    fn on_event(&self, event: &AddonEvent, ctx: &Arc<dyn AddonCtx>) {
        self.runner.on_event(event, ctx);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
