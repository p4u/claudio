//! Proxy integration: profiles, HTTP client and session env builder.
//!
//! Entry points for the CLI commands are in `cmd.rs`; the proxy module itself
//! is a pure library that neither reads from stdin nor writes to stdout.

pub mod api;
pub mod cmd;
pub mod env;
pub mod profile;
pub mod resolve;

pub use resolve::ProxyChoice;
