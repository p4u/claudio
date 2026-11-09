//! The layer both front-ends (the console TUI and the web UI) share: client
//! state, daemon connections, the extension registry and snippets. Nothing
//! here draws or reads input; it imports neither ratatui, crossterm nor axum.

pub(crate) mod connections;
pub(crate) mod extensions;
pub(crate) mod snippets;
pub(crate) mod state;

#[cfg(test)]
pub(crate) mod test_support;
