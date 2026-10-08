//! Everything claudio knows about Claude Code itself.
//!
//! Sub-modules:
//!
//! - [`hooks`] — hook event constants, settings builder, per-spawn token
//!   generator, and the short-lived relay process.
//! - [`projects`] — locating project directories, listing sessions, and
//!   reading transcript metadata.
//! - [`state`] — pure state machine that maps hook events onto
//!   [`proto::SessionEvent`]s.

#[cfg_attr(not(test), allow(dead_code))]
pub mod hooks;

#[cfg_attr(not(test), allow(dead_code))]
pub mod projects;

#[cfg_attr(not(test), allow(dead_code))]
pub mod state;
