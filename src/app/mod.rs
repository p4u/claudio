//! The layer both front-ends (the console TUI and the web UI) share: client
//! state, daemon connections, the extension registry and snippets. Nothing
//! here draws or reads input; it imports neither ratatui, crossterm nor axum.

// P3: the runtime lands before all its callers (docs/plans/2026-11-09-core-addons-p3-plan.md);
// removed in P3.20, with the addon API's own allow.
#[allow(dead_code)]
pub(crate) mod addons;
pub(crate) mod browser;
pub(crate) mod config_edit;
pub(crate) mod connections;
pub(crate) mod extensions;
pub(crate) mod label;
// P3: the composer lands before its callers (P3.5b/c) and its review half
// before the checklists (P3.11); removed in P3.20, with the addon API's own allow.
#[allow(dead_code)]
pub(crate) mod launch;
pub(crate) mod snippets;
pub(crate) mod state;

#[cfg(test)]
pub(crate) mod test_support;
