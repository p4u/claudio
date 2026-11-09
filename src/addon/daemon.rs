//! Daemon ops: the code an addon runs inside each host's daemon
//! (docs/addons.md, "Daemon ops"; docs/daemon-protocol.md).

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

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
