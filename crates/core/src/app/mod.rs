//! The layer both front-ends (the console TUI and the web UI) share: client
//! state, daemon connections, the extension registry and snippets. Nothing
//! here draws or reads input; it imports neither ratatui, crossterm nor axum.

// P3: the runtime lands before all its callers (docs/plans/2026-11-09-core-addons-p3-plan.md);
// removed in P3.20, with the addon API's own allow.
#[allow(dead_code)]
pub mod addons;
pub mod browser;
pub mod config_edit;
pub mod connections;
pub mod extensions;
pub mod label;
// P3: the composer lands before its callers (P3.5b/c) and its review half
// before the checklists (P3.11); removed in P3.20, with the addon API's own allow.
#[allow(dead_code)]
pub mod launch;
pub mod snippets;
pub mod state;

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
