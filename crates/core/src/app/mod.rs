//! The layer both front-ends (the console TUI and the web UI) share: client
//! state, daemon connections, the extension registry and snippets. Nothing
//! here draws or reads input; it imports neither ratatui, crossterm nor axum.

pub mod addons;
pub mod browser;
pub mod config_edit;
pub mod connections;
pub mod extensions;
pub mod label;
pub mod launch;
pub mod snippets;
pub mod state;

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
