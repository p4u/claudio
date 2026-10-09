//! Everything claudio knows about Claude Code itself.
//!
//! Sub-modules:
//!
//! - [`hooks`] — hook event constants, settings builder, per-spawn token
//!   generator, and the short-lived relay process.
//! - [`projects`] — locating project directories, listing sessions, and
//!   reading transcript metadata.
//! - [`update`] — version parsing, release-channel lookup and the daily
//!   check behind "keep claude up to date".
//! - [`state`] — pure state machine that maps hook events onto
//!   [`proto::SessionEvent`]s.

pub mod hooks;
pub mod projects;
pub mod state;
pub mod update;
