//! Remote host support for the session manager.
//!
//! - [`probe`] — `claudio __probe`: prints a JSON fingerprint of this binary.
//! - [`bridge`] — `claudio --slave`: the remote-side stdio↔socket bridge.
//! - [`hosts`] — SSH host alias discovery and MRU persistence.
//! - [`bootstrap`] — install or update the claudio binary on a remote host.
//! - [`diag`] — diagnostic / test-harness subcommands (`__bootstrap`, etc.).

pub mod bootstrap;
pub mod bridge;
pub mod diag;
pub mod hosts;
pub mod probe;
