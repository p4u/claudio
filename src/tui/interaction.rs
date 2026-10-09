//! Modal popup types (S1 split from app.rs).

use crate::proto::SessionId;

use super::wizard::Wizard;

/// A popup that captures the keyboard.
pub enum Modal {
    Rename { id: SessionId, input: String },
    /// Confirm killing a session.
    Close { id: SessionId },
    Wizard(Wizard),
    /// Proxy stats popup.
    ProxyStats { profile_name: String },
    /// Overview / "mission control": all sessions at a glance.
    Overview { selected: usize },
    /// Help popup: all key bindings.
    Help,
}
