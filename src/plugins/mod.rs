//! Proxy plugins as manifests: what a member's sessions get from a plugin
//! the proxy serves. [`manifest`] is the one parser, shared by the client
//! (which shows the items and keeps the member's choices) and the daemon
//! (which applies them on its host).

pub mod choices;
pub mod manifest;
pub mod state;
