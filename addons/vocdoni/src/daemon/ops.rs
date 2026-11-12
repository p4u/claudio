//! The Vocdoni addon's `DaemonOps`: the `apply` op over [`Manifests`], and
//! the helpers its installs need from a daemon.

use std::sync::{Arc, Once};
use std::time::Duration;

use serde_json::Value;
use tokio::process::Child;

use super::{handle, remove_stale_zips, ApplyRequest, Manifests, OP_APPLY};
use crate::plugins::manifest::name_ok;
use claudio_core::addon::{BoxFuture, ClaudeFlags, DaemonCtx, DaemonOps, LimitOutcome, UsageLimit};

/// The plugin flags a plugin's sessions use.
const AGENTS: &str = "--agents";
const MCP_CONFIG: &str = "--mcp-config";

/// How long a timed-out `claude plugin …` group gets after SIGTERM.
const KILL_GRACE: Duration = Duration::from_secs(2);

/// The Vocdoni addon in a daemon: answers `vocdoni/apply` (docs/addons.md,
/// "The Vocdoni addon") and keeps what the plugin installs did for the
/// daemon's life.
pub struct VocdoniDaemon {
    manifests: Arc<Manifests>,
    /// compat(v0.10): the plugin cache of the releases before addons,
    /// `<cache>/plugins`, swept of a pre-manifest build's zips once, at the
    /// first op.
    legacy: std::path::PathBuf,
    swept: Once,
}

impl Default for VocdoniDaemon {
    fn default() -> Self {
        VocdoniDaemon::new()
    }
}

impl VocdoniDaemon {
    /// Manifests in `DaemonCtx::cache_dir`, Claude Code's files under this
    /// daemon's `CLAUDE_CONFIG_DIR`, else `~/.claude`.
    pub fn new() -> VocdoniDaemon {
        VocdoniDaemon {
            manifests: Arc::new(Manifests::new()),
            legacy: claudio_core::paths::cache_dir().join("plugins"),
            swept: Once::new(),
        }
    }

    /// Manifests in `cache` (its sibling `plugins` the legacy cache),
    /// Claude Code's files in `claude_config`, and http accepted on
    /// loopback (tests).
    #[cfg(test)]
    pub(crate) fn with_dirs(
        cache: std::path::PathBuf,
        claude_config: std::path::PathBuf,
    ) -> VocdoniDaemon {
        let daemon = VocdoniDaemon::https_only(cache.clone(), claude_config.clone());
        daemon.manifests.set_dirs(cache, claude_config, true);
        daemon
    }

    /// As [`VocdoniDaemon::with_dirs`] but https only, as a real daemon.
    #[cfg(test)]
    pub(crate) fn https_only(
        cache: std::path::PathBuf,
        claude_config: std::path::PathBuf,
    ) -> VocdoniDaemon {
        let daemon = VocdoniDaemon {
            legacy: cache.with_file_name("plugins"),
            ..VocdoniDaemon::new()
        };
        daemon.manifests.set_dirs(cache, claude_config, false);
        daemon
    }

    /// What the installs did (tests watch and move its clock).
    #[cfg(test)]
    pub(crate) fn manifests(&self) -> &Arc<Manifests> {
        &self.manifests
    }
}

impl DaemonOps for VocdoniDaemon {
    fn handle<'a>(
        &'a self,
        op: &'a str,
        body: Value,
        ctx: Arc<dyn DaemonCtx>,
    ) -> BoxFuture<'a, Result<Value, String>> {
        Box::pin(async move {
            if op != OP_APPLY {
                return Err(format!("vocdoni: no op {op}"));
            }
            self.swept.call_once(|| remove_stale_zips(&self.legacy));
            let name = body.get("name").and_then(Value::as_str).unwrap_or("");
            let bad = bad_request(name);
            let req: ApplyRequest = serde_json::from_value(body).map_err(|_| bad)?;
            let applied = handle(&self.manifests, ctx, req).await?;
            serde_json::to_value(applied).map_err(|e| format!("vocdoni: {e}"))
        })
    }

    /// Claude was just installed: the installs that failed without it are
    /// tried at the next request. Manual installs stay.
    fn on_claude_installed(&self, _ctx: &Arc<dyn DaemonCtx>) {
        self.manifests.forget_failures();
    }

    /// A proxy session hit a usage limit: move its conversation to the
    /// proxy's next subscription ([`crate::proxy::limit`]).
    fn on_usage_limit<'a>(
        &'a self,
        hit: UsageLimit<'a>,
        ctx: Arc<dyn DaemonCtx>,
    ) -> BoxFuture<'a, LimitOutcome> {
        Box::pin(crate::proxy::limit::on_usage_limit(hit, ctx))
    }
}

/// The refusal of a request that is not one: names the plugin only when
/// the name is valid.
pub(super) fn bad_request(name: &str) -> String {
    format!("manifest {}: bad request", safe_name(name))
}

/// A name for an error about a refused request: shown only when valid.
fn safe_name(name: &str) -> &str {
    if name_ok(name) {
        name
    } else {
        "?"
    }
}

/// The plugin flags this claude lacks (`--mcp-config`, `--agents`): what a
/// plugin's sessions here go without.
pub(super) fn plugin_flags_dropped(flags: &ClaudeFlags) -> Vec<String> {
    [MCP_CONFIG, AGENTS]
        .into_iter()
        .filter(|flag| !flags.knows(flag))
        .map(str::to_owned)
        .collect()
}

/// Stop a `claude plugin …` run started with `process_group(0)`: SIGTERM
/// to its group, a grace, then SIGKILL to what is left of it.
pub(super) async fn kill_group(child: &mut Child) {
    // The leader's pid is the group id; `None` once it has been reaped,
    // and then its pid may already be reused.
    let Some(pgid) = child.id().and_then(|pid| i32::try_from(pid).ok()) else {
        return;
    };
    // SAFETY: kill(2) has no memory-safety preconditions.
    unsafe { libc::kill(-pgid, libc::SIGTERM) };
    let _ = tokio::time::timeout(KILL_GRACE, child.wait()).await;
    // A group id is not reused while any member is alive, so this reaches
    // only its descendants.
    // SAFETY: as above.
    unsafe { libc::kill(-pgid, libc::SIGKILL) };
    let _ = child.wait().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Moved from `daemon::host`'s flag tests with the function.
    #[test]
    fn plugin_flags_dropped_names_the_flags_claude_lacks() {
        assert_eq!(
            plugin_flags_dropped(&ClaudeFlags::from_help("")),
            ["--mcp-config", "--agents"]
        );
        let current = ClaudeFlags::from_help(
            "  --agents <json-or-file>  Agents\n  --mcp-config <configs...>  MCP\n               --bare  Skips --plugin-dir, --strict-mcp-config",
        );
        assert!(plugin_flags_dropped(&current).is_empty());
        let one = ClaudeFlags::from_help("  --agents <json>\n");
        assert_eq!(plugin_flags_dropped(&one), ["--mcp-config"]);
    }
}
