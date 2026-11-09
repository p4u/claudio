//! The addons this build registers (docs/addons.md). The core never names
//! them; `main` hands `all()` to `addon::Addons::new`. Each is a cargo
//! feature (`api`, `web`, `vocdoni`, all default); without any, the core alone.

#[cfg(feature = "api")]
pub use claudio_api as api;

#[cfg(feature = "vocdoni")]
// P4.6 slot: becomes `pub use claudio_vocdoni as vocdoni;`
pub mod vocdoni;

#[cfg(feature = "web")]
// P4.5 slot: becomes `pub use claudio_web as web;`
pub mod web;

use std::sync::Arc;

use crate::addon::Addon;

/// The addons this build registers, in registry order. The API addon
/// parses every remaining argv (`cli::parse`), so it is last.
pub fn all() -> Vec<Arc<dyn Addon>> {
    vec![
        #[cfg(feature = "vocdoni")]
        Arc::new(vocdoni::Vocdoni::new()),
        #[cfg(feature = "web")]
        Arc::new(web::WebAddon),
        #[cfg(feature = "api")]
        Arc::new(api::ApiAddon),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addon::Addons;

    #[test]
    fn addons_all_registers() {
        assert!(Addons::new(all()).is_ok());
    }
}
