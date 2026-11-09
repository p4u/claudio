//! The addon API (docs/addons.md): what an addon implements, what the core
//! offers it, the data it hands the front-ends, and the registry.
// P3: the API lands before its consumers (docs/plans/2026-11-09-core-addons-p3-plan.md);
// removed in P3.20.
#![allow(dead_code)]

mod clean;
mod daemon;
mod launch;
mod registry;
#[cfg(test)]
pub(crate) mod test_support;
mod ui;

pub use clean::*;
pub use daemon::*;
pub use launch::*;
pub use registry::*;
pub use ui::*;

use std::{future::Future, pin::Pin, process::ExitCode, sync::Arc};

use serde_json::Value;

use crate::proto::{SessionId, SessionKind};

/// A boxed, sendable future: what the API's async methods return.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A config edit: gets the addon's section, changes it.
pub type ConfigEdit = Box<dyn FnOnce(&mut toml::Table) + Send>;

/// Whether `name` is a valid addon or panel name: `[a-z][a-z0-9-]{0,31}`.
pub fn name_ok(name: &str) -> bool {
    let mut c = name.chars();
    name.len() <= 32
        && c.next().is_some_and(|c| c.is_ascii_lowercase())
        && c.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The `Welcome.features` entry of a daemon-side addon.
pub fn feature(addon: &str) -> String {
    format!("addon:{addon}")
}

/// An addon: a name and the capabilities it has (docs/addons.md).
pub trait Addon: Send + Sync + 'static {
    /// `[a-z][a-z0-9-]{0,31}`: its registry key, CLI word and daemon feature.
    fn name(&self) -> &'static str;
    /// The top-level `config.toml` tables it owns; a core table or one
    /// another addon claims is refused at registration.
    fn config_sections(&self) -> &'static [&'static str] {
        &[]
    }
    /// Its CLI words, if it has any.
    fn commands(&self) -> Option<&dyn Commands> {
        None
    }
    /// Its share of launches, if it has one.
    fn launch(&self) -> Option<&dyn Launch> {
        None
    }
    /// Its segments, panels, checklists and extension rows, if it has any.
    fn ui(&self) -> Option<&dyn Ui> {
        None
    }
    /// Its daemon ops, if it runs code in the daemon.
    fn daemon(&self) -> Option<&dyn DaemonOps> {
        None
    }
    /// A front-end event (D1). Must not block: spawn work with `ctx.spawn`.
    fn on_event(&self, event: &AddonEvent, ctx: &Arc<dyn AddonCtx>) {
        let _ = (event, ctx);
    }
    /// The addon as `Any`, written `{ self }` (no default body can turn a
    /// `?Sized` `Self` into `Any`). Transitional: only the Vocdoni bridge
    /// downcasts, until P3.13 removes both (docs/addons.md).
    fn as_any(&self) -> &dyn std::any::Any;
}

/// An addon's command-line words.
pub trait Commands: Send + Sync {
    /// Checked before anything else (print's hook relays).
    fn is_relay(&self, argv: &[String]) -> bool {
        let _ = argv;
        false
    }
    /// After the core's own words, in registry order; `None`: not mine.
    fn dispatch(&self, argv: &[String], addons: &Arc<Addons>) -> Option<ExitCode>;
    /// Appended to `claudio --help`.
    fn help(&self) -> &'static str {
        ""
    }
}

/// What the core offers an addon on a front-end (the TUI or the web hub).
/// Each addon gets its own (D2).
pub trait AddonCtx: Send + Sync {
    /// The addon this context belongs to.
    fn addon(&self) -> &'static str;
    /// A task on the front-end's runtime: ≤ 64 live per addon, 300 s each,
    /// aborted at shutdown.
    fn spawn(&self, task: BoxFuture<'static, ()>);
    /// The one shared HTTP client (cheap clone).
    fn http(&self) -> reqwest::Client;
    /// `AddonOp { addon: self.addon(), op, body }` to `host`'s daemon;
    /// `Missing` without `addon:<name>` in its features; bodies ≤ 512 KiB
    /// both ways; 170 s.
    fn request(
        &self,
        host: &str,
        op: &str,
        body: Value,
    ) -> BoxFuture<'static, Result<Value, AddonError>>;
    /// Whether `host` is connected and its daemon lists `addon:<self>`.
    fn host_has(&self, host: &str) -> bool;
    /// The sessions the front-end knows.
    fn sessions(&self) -> Vec<SessionSnap>;
    /// The hosts the front-end knows.
    fn hosts(&self) -> Vec<HostSnap>;
    /// A declared section of config.toml (empty for an undeclared one).
    fn config(&self, section: &str) -> toml::Table;
    /// Serialised, atomic, minimal edit of a declared section (`app::config_edit`).
    fn edit_config(
        &self,
        section: &str,
        edit: ConfigEdit,
    ) -> BoxFuture<'static, Result<(), String>>;
    /// `[extensions] <name>` as config.toml has it (D10).
    fn extension_on(&self, name: &str) -> Option<bool>;
    /// A notice in the front-end.
    fn notify(&self, text: String);
    /// Segments, rows, options or an open panel changed: front-ends re-read them.
    fn changed(&self);
    /// Ask front-ends to show checklist `id` when idle (D8).
    fn offer_checklist(&self, id: String);
}

/// A session as an addon sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionSnap {
    /// The session's id.
    pub sid: SessionId,
    /// The host it runs on.
    pub host: String,
    /// Claude or a terminal.
    pub kind: SessionKind,
    /// Its working directory.
    pub cwd: String,
    /// Its label in the tab bar.
    pub label: String,
    /// The claude conversation id, once known.
    pub conversation: Option<String>,
    /// What it was launched with (`None`: direct).
    pub launch: Option<LaunchChoice>,
    /// The TUI's active session, or one a browser tab has attached.
    pub focused: bool,
}

/// A host as an addon sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct HostSnap {
    /// `local` or an ssh alias.
    pub name: String,
    /// Whether its daemon is connected.
    pub connected: bool,
    /// Its daemon's `Welcome.features`.
    pub features: Vec<String>,
    /// Whether claude is installed there.
    pub claude: bool,
}

/// What `Addon::on_event` receives (docs/addons.md, "Events").
#[derive(Debug, Clone, PartialEq)]
pub enum AddonEvent {
    /// First, once the snapshots are filled.
    Started,
    /// Every second.
    Tick,
    /// A host's daemon connected.
    HostUp { host: String, features: Vec<String> },
    /// A host's daemon went away.
    HostDown { host: String },
    /// A session appeared.
    SessionAdded {
        sid: SessionId,
        conversation: Option<String>,
    },
    /// A session went away.
    SessionRemoved { sid: SessionId },
    /// The conversation id changed (`/clear`), or focus moved.
    SessionChanged {
        sid: SessionId,
        conversation: Option<String>,
    },
    /// claude was just installed on `host`.
    ClaudeInstalled { host: String },
    /// config.toml changed on disk.
    ConfigChanged,
}

/// Why `AddonCtx::request` failed.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum AddonError {
    /// The host is not connected.
    #[error("{0}: not connected")]
    NotConnected(String),
    /// The host's daemon does not list this addon's feature.
    #[error("daemon on {0} is older: run `claudio daemon restart`")]
    Missing(String),
    /// A body over 512 KiB encoded.
    #[error("message over 512 KiB")]
    TooLarge,
    /// No answer within the request timeout.
    #[error("timed out")]
    Timeout,
    /// The daemon answered with an error.
    #[error("{0}")]
    Daemon(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_ok_accepts_the_documented_charset_only() {
        for good in ["a", "vocdoni", "web", "a-1", "x9", &"a".repeat(32)] {
            assert!(name_ok(good), "{good}");
        }
        for bad in [
            "",
            "1a",
            "-a",
            "A",
            "aB",
            "a_b",
            "a b",
            "a.b",
            "é",
            &"a".repeat(33),
        ] {
            assert!(!name_ok(bad), "{bad}");
        }
        assert_eq!(feature("echo"), "addon:echo");
    }
}
