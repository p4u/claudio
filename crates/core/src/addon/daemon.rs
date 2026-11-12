//! Daemon ops: the code an addon runs inside each host's daemon
//! (docs/addons.md, "Daemon ops"; docs/daemon-protocol.md).

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use super::BoxFuture;

/// An addon's daemon side: answers `Msg::AddonOp`.
pub trait DaemonOps: Send + Sync {
    /// Answer op `op` (off the daemon's request loop, ≤ 8 in flight per
    /// addon, 150 s).
    fn handle<'a>(
        &'a self,
        op: &'a str,
        body: Value,
        ctx: Arc<dyn DaemonCtx>,
    ) -> BoxFuture<'a, Result<Value, String>>;
    /// claude was just installed on this host (today's `Manifests::forget_failures`).
    fn on_claude_installed(&self, ctx: &Arc<dyn DaemonCtx>) {
        let _ = ctx;
    }
    /// What the daemon should keep running for this addon: argv after the claudio binary,
    /// from the addon's declared config sections. `Ok(None)`: nothing. `Err(msg)`: the
    /// sections are invalid; `msg` names the key. Pure, no I/O.
    fn service(&self, config: &dyn Fn(&str) -> toml::Table) -> Result<Option<Vec<String>>, String> {
        let _ = config;
        Ok(None)
    }
    /// A session of this daemon ended a turn on a usage limit (docs/addons.md, "Usage
    /// limits"). Asked in registry order until one answers other than `NotMine`, off the
    /// session's actor, within `daemon::Config::limit_attempt_timeout` (20 s) for the round.
    fn on_usage_limit<'a>(
        &'a self,
        hit: UsageLimit<'a>,
        ctx: Arc<dyn DaemonCtx>,
    ) -> BoxFuture<'a, LimitOutcome> {
        let _ = (hit, ctx);
        Box::pin(async { LimitOutcome::NotMine })
    }
}

/// A usage-limit hit, as `DaemonOps::on_usage_limit` gets it. No `Debug`: `env` reads the
/// session's spawn env, secrets included (C1).
pub struct UsageLimit<'a> {
    /// claude's session id (the conversation).
    pub conversation: &'a str,
    /// claude's message, sanitised, one line, at most 256 bytes.
    pub message: &'a str,
    /// The session's spawn env, by name. Never log a value.
    pub env: &'a (dyn Fn(&str) -> Option<String> + Sync),
}

/// What an addon did about a usage-limit hit. The core cleans every string (one safe line,
/// at most 120 bytes) before a notice shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LimitOutcome {
    /// Not this addon's session, or the addon is switched off here.
    NotMine,
    /// The conversation's next message goes to another subscription.
    Switched {
        /// The subscription it leaves.
        from: String,
        /// The subscription it moves to.
        to: String,
    },
    /// The addon tried; why it did not work.
    Failed(String),
}

/// An addon service in this daemon (docs/addons.md, "Daemon ops").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceState {
    /// Nothing runs: `service()` answered `Ok(None)`, or nothing asked yet.
    Off,
    /// Started; its `ready` line has not come yet.
    Starting,
    /// It said `ready`.
    Running {
        /// The child's pid, also its process group.
        pid: u32,
        /// When it said `ready`, in Unix seconds.
        since: u64,
        /// What it was started with, after the claudio binary.
        argv: Vec<String>,
    },
    /// Its start failed, it exited unasked, or `service()` refused the sections.
    Failed {
        /// Its exit code, when it exited with one.
        exit: Option<i32>,
        /// The signal that ended it, when one did.
        signal: Option<i32>,
        /// The last ≤ 4 KiB it wrote to stderr since this start (sanitised),
        /// or why it did not start.
        tail: String,
        /// When it is started again (`None`: at the next reload or daemon start).
        retry_in: Option<Duration>,
    },
}

/// What the daemon offers an addon's op.
pub trait DaemonCtx: Send + Sync {
    /// The claude this daemon runs.
    fn claude_bin(&self) -> PathBuf;
    /// The daemon's login-shell PATH (`daemon::Config::path`).
    fn path(&self) -> Option<OsString>;
    /// The flags this claude knows (one `--help`, cached; probed off the runtime).
    fn claude_flags(&self) -> BoxFuture<'static, ClaudeFlags>;
    /// `<cache>/<addon>` (D7); the addon creates it 0700.
    fn cache_dir(&self) -> PathBuf;
    /// Held while claude update or a `claude plugin …` runs: one writer of `~/.claude`.
    fn updating(&self) -> Arc<tokio::sync::Mutex<()>>;
    /// Re-ask `service()` and converge: stop, start, or restart when the argv changed (or
    /// `force`). A service that is `Failed` is started now and its backoff reset, even with
    /// the same argv. Resolves once the child is ready, has failed its first attempt, or is
    /// off (≤ 25 s: a 3 s stop and a 20 s start). A second reload for the same addon waits for
    /// the first; reloads that queue behind it run as one.
    fn service_reload(&self, force: bool) -> BoxFuture<'static, ServiceState>;
    /// This addon's service as of now.
    fn service_state(&self) -> ServiceState;
    /// A declared section of this host's `config.toml` as it was when this context was
    /// made (each op and each `on_usage_limit` reads the file afresh, off the runtime);
    /// empty for an undeclared section, or a file that is missing, over 1 MiB or not TOML.
    fn config(&self, section: &str) -> toml::Table {
        let _ = section;
        toml::Table::new()
    }
}

/// Lets claude offer bypass-permissions mode; any mention in `--help` counts.
const ALLOW_SKIP_PERMISSIONS: &str = "--allow-dangerously-skip-permissions";

/// The optional flags a claude's `--help` lists (D6).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClaudeFlags {
    /// Whether `--help` mentions `--allow-dangerously-skip-permissions`.
    pub skip_permissions: bool,
    /// Every `--flag` that starts a line of `--help`.
    pub listed: BTreeSet<String>,
}

impl ClaudeFlags {
    /// Every `--flag` that starts a line of `help` (today's `host::lists`
    /// rule: the flag ends at a space, `=`, `,` or the line's end).
    pub fn from_help(help: &str) -> ClaudeFlags {
        let listed = help
            .lines()
            .filter_map(|line| {
                let line = line.trim_start();
                if !line.starts_with("--") {
                    return None;
                }
                let end = line.find([' ', '=', ',']).unwrap_or(line.len());
                Some(line[..end].to_string())
            })
            .collect();
        ClaudeFlags {
            skip_permissions: help.contains(ALLOW_SKIP_PERMISSIONS),
            listed,
        }
    }

    /// Whether this claude lists `flag`.
    pub fn knows(&self, flag: &str) -> bool {
        self.listed.contains(flag)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_flags_from_help_lists_line_leading_flags() {
        let help = "Usage: claude [options]\n\
                    \x20 --plugin-dir <paths...>  Load plugins\n\
                    \x20 --agents=<json>          Agents\n\
                    \x20 --mcp-config, -m         MCP\n\
                    \x20 -d, --debug              Debug (not line-leading)\n\
                    \x20 --allow-dangerously-skip-permissions\n\
                    \x20 --foo[=bar]              Odd\n\
                    see --verbose for more\n";
        let flags = ClaudeFlags::from_help(help);
        assert!(flags.skip_permissions);
        for known in ["--plugin-dir", "--agents", "--mcp-config"] {
            assert!(flags.knows(known), "{known}");
        }
        for unknown in ["--debug", "--verbose", "--foo", "--plugin", "-d"] {
            assert!(!flags.knows(unknown), "{unknown}");
        }
        let none = ClaudeFlags::from_help("claude 2.0\n");
        assert_eq!(none, ClaudeFlags::default());
    }
}
