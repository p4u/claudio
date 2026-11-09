//! The addons this build registers (docs/addons.md). The core never names
//! them; `main` hands `all()` to `addon::Addons::new`.

pub mod vocdoni;

use std::sync::Arc;

use crate::addon::Addon;

/// The addons this build registers, in registry order.
pub fn all() -> Vec<Arc<dyn Addon>> {
    vec![Arc::new(vocdoni::Vocdoni::new())]
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
