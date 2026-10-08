//! Terminal plumbing shared by every PTY host (print mode, the daemon, the TUI).

pub mod probe;

#[cfg_attr(not(test), allow(dead_code))]
pub mod keys;
#[cfg_attr(not(test), allow(dead_code))]
pub mod screen;
