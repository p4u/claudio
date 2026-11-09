//! The Vocdoni addon: the claude-proxy client and the team plugins.

pub mod daemon;
pub mod plugins;
pub mod proxy;
#[cfg(test)]
pub(crate) mod test_support;

use crate::addon::Addon;

/// The addon's name: its config sections' owner and its daemon ops' prefix.
pub const NAME: &str = "vocdoni";

/// The Vocdoni addon.
pub struct Vocdoni;

impl Addon for Vocdoni {
    fn name(&self) -> &'static str {
        NAME
    }

    fn config_sections(&self) -> &'static [&'static str] {
        &["proxy", "plugins"]
    }
}
