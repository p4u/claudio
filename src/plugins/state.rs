//! Proxy plugins on the client: each profile's list (from the proxy or its
//! cache in `plugins.json`), each plugin's manifest (fetched here, kept in
//! memory), what each connected host answered for it (`Applied`), and what
//! a spawn gets from them ([`compose`]).
//!
//! Pure, like `app.rs`: fetching a list or a manifest, writing the cache
//! and asking a host for a manifest are `Effect`s that `tui/mod.rs`
//! runs. The registry lists the plugins (`Source::Plugin`) so the
//! extensions screen shows and toggles them; turning one off is saved as
//! `[extensions] <name> = false` and applies to new sessions only. A host
//! is asked on connect, when the list's hash changes, on `r`, once claude
//! is installed there, every 30 s while it still installs, and when a
//! failed install's retry is due.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::config::PluginChoices;
use crate::plugins::manifest::Manifest;
use crate::proto::merge_env_pairs;
use crate::proto::{retry_secs, Applied, Msg, MAX_PLUGIN_INSTRUCTIONS};
use crate::proxy::plugins::PluginList;

/// `--agents`, `--mcp-config` (the daemon drops them for a claude without).
const AGENTS: &str = "--agents";
const MCP_CONFIG: &str = "--mcp-config";
/// The reason the daemon gives a plugin whose install is under way.
pub(crate) const INSTALLING: &str = "installing";

/// How often every profile's list is fetched again.
pub const REFRESH_SECS: u64 = 15 * 60;

/// How a profile's list was last fetched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListState {
    /// Read from `plugins.json` at start; the first fetch is under way.
    Cached,
    /// From the proxy, this run.
    Fresh,
    /// From the cache; the proxy did not answer (or is too old).
    Unreachable,
}

/// What a fetch of a profile's list gave (`tui/mod.rs` has already saved
/// or dropped the cache entry to match).
#[derive(Debug, Clone, PartialEq)]
pub enum PluginsFetch {
    Listed(PluginList),
    /// 401 or 403: the token is no longer valid; its cached list is gone.
    SignedOut,
    /// Any other failure, or a proxy without plugins (404): the cached list
    /// (if any) stays.
    Unreachable,
}

/// A plugin's manifest on this client: fetched by the client itself (at
/// the list's hash), kept in memory only.
#[derive(Debug, Clone, PartialEq)]
pub enum ManifestState {
    Fetching,
    Ready(Box<Manifest>),
    /// Refused or unreachable: nothing is asked of any host for it.
    Failed(String),
}

/// What a host answered for one plugin's manifest.
#[derive(Debug, Clone, PartialEq)]
pub enum ApplyState {
    /// Asked; no answer yet.
    Pending,
    /// What the host made of it (kept while it is asked again).
    Ready(Box<Applied>),
    Failed(String),
    /// The daemon does not know `Manifest`.
    TooOld,
}

/// One host's state for one plugin's manifest.
#[derive(Debug, Clone, PartialEq)]
pub struct Apply {
    /// The manifest hash asked for.
    pub sha: String,
    pub state: ApplyState,
    /// When it was last asked (Unix seconds).
    pub asked: u64,
}

/// The client's plugin state.
#[derive(Debug, Default)]
pub struct Plugins {
    /// `[extensions]` of config.toml, kept so a plugin that appears later
    /// gets its saved choice (and a toggle updates it).
    pub config: BTreeMap<String, bool>,
    /// `[plugins.<name>]` of config.toml: the member's manifest choices.
    pub choices: BTreeMap<String, PluginChoices>,
    /// Per profile: its list (`None` once signed out) and how it was fetched.
    pub lists: BTreeMap<String, (PluginList, ListState)>,
    /// The hosts whose daemon takes `Manifest` requests now.
    pub hosts: BTreeSet<String>,
    /// Per plugin: the manifest hash fetched, and the manifest.
    pub manifests: HashMap<String, (String, ManifestState)>,
    /// Per `(host, plugin)`: what that host answered.
    pub applies: HashMap<(String, String), Apply>,
    /// Plugins whose first-sight setup the member put off (Esc) in this
    /// run: nothing is asked of a host for them until the setup is saved.
    pub deferred: BTreeSet<String>,
    /// `(plugin, hash)` whose new items were named in a notice already.
    pub noticed: HashSet<(String, String)>,
}

/// What one enabled plugin brings to a spawn on a host.
#[derive(Clone, PartialEq)]
pub struct PluginPart {
    pub name: String,
    /// The host's `Applied`, once it answered.
    pub applied: Option<Applied>,
    /// The plugin's proxy env: its secrets.
    pub secrets: Vec<(String, String)>,
}

/// The plugins' share of a spawn ([`compose`]).
#[derive(Clone, Default, PartialEq)]
pub struct Composed {
    /// `--agents <json>` then `--mcp-config <json>`, per plugin.
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub instructions: String,
    pub notices: Vec<String>,
}

/// Env keys only: the values are secrets.
fn keys(env: &[(String, String)]) -> Vec<&str> {
    env.iter().map(|(k, _)| k.as_str()).collect()
}

impl std::fmt::Debug for PluginPart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginPart")
            .field("name", &self.name)
            .field("applied", &self.applied.as_ref().map(|a| &a.name))
            .field("secrets", &keys(&self.secrets))
            .finish()
    }
}

impl std::fmt::Debug for Composed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Composed")
            .field("args", &self.args)
            .field("env", &keys(&self.env))
            .field("instructions", &self.instructions.len())
            .field("notices", &self.notices)
            .finish()
    }
}

/// At most this many plugins go on one spawn (their flags are ≤ 96 KiB
/// each; a frame is 1 MiB).
pub const MAX_SPAWN_PLUGINS: usize = 8;

/// While a host still installs a manifest's plugins it is asked again this
/// often (seconds); while it waits to retry one, when that is due (and no
/// sooner than this).
pub const REASK_EVERY: u64 = 30;

/// The plugins' share of a spawn, from `parts` in registry order: for each
/// plugin the host answered for (at most [`MAX_SPAWN_PLUGINS`]), its
/// `--agents` then its `--mcp-config`, its `[env]` and its instructions;
/// every plugin's secrets. Env layers: the manifests' `[env]`, then the
/// secrets, a later layer replacing an earlier one's key and the first
/// plugin winning within a layer. The instructions are joined by a blank
/// line, cut at the last whole one that fits 8 KiB.
pub fn compose(parts: &[PluginPart]) -> Composed {
    let mut c = Composed::default();
    let mut manifest_env: Vec<(String, String)> = Vec::new();
    let mut instructions: Vec<&str> = Vec::new();
    let mut size = 0;
    let mut cut: Vec<&str> = Vec::new();
    let mut ready = 0;
    for part in parts {
        let Some(a) = &part.applied else { continue };
        if ready == MAX_SPAWN_PLUGINS {
            c.notices
                .push(format!("plugin {} left out: too many plugins", part.name));
            continue;
        }
        ready += 1;
        if let Some(agents) = &a.agents {
            c.args.push(AGENTS.to_owned());
            c.args.push(agents.clone());
        }
        if let Some(mcp) = &a.mcp_config {
            c.args.push(MCP_CONFIG.to_owned());
            c.args.push(mcp.clone());
        }
        manifest_env.extend(a.env.iter().cloned());
        let text = a.instructions.trim();
        if text.is_empty() {
            continue;
        }
        let sep = if instructions.is_empty() { 0 } else { 2 };
        if !cut.is_empty() || size + sep + text.len() > MAX_PLUGIN_INSTRUCTIONS {
            cut.push(&part.name);
            continue;
        }
        size += sep + text.len();
        instructions.push(text);
    }
    if !cut.is_empty() {
        c.notices.push(format!(
            "plugin instructions over 8 KiB: {} left out",
            cut.join(", ")
        ));
    }
    let secrets: Vec<(String, String)> = parts
        .iter()
        .flat_map(|p| p.secrets.iter().cloned())
        .collect();
    c.env = merge_env_pairs(&manifest_env, merge_env_pairs(&secrets, Vec::new()));
    c.instructions = instructions.join("\n\n");
    c
}

/// `msg` (a `Spawn` or `Respawn`) with the plugins' share: their flags
/// after its args (a `Spawn`'s; a `Respawn` keeps the journaled ones),
/// their env and their instructions.
pub fn with_plugins(msg: Msg, c: Composed) -> Msg {
    msg.with_plugin_args(&c.args)
        .with_instructions(c.instructions)
        .with_env(c.env)
}

/// How long after its last ask a host that answered `a` is asked again, if
/// it is: [`REASK_EVERY`] while a plugin installs, else a second after the
/// soonest retry (no sooner than [`REASK_EVERY`]).
pub(crate) fn reask_after(a: &Applied) -> Option<u64> {
    a.skipped
        .iter()
        .filter_map(|(_, why)| match why.as_str() {
            INSTALLING => Some(0),
            why => retry_secs(why).map(|secs| secs + 1),
        })
        .min()
        .map(|secs| secs.max(REASK_EVERY))
}

#[cfg(test)]
/// What a host answers for `name`'s manifest.
pub(crate) fn applied(name: &str) -> Applied {
    Applied {
        name: name.into(),
        version: "1.4.0".into(),
        mcp_config: Some(format!(r#"{{"mcpServers":{{"{name}":{{}}}}}}"#)),
        agents: Some(format!(r#"{{"{name}-agent":{{}}}}"#)),
        instructions: format!("Use {name}."),
        env: vec![("KB_REGION".into(), name.into())],
        ..Applied::default()
    }
}

#[cfg(test)]
pub(crate) fn part(name: &str, ready: bool, secrets: &[(&str, &str)]) -> PluginPart {
    PluginPart {
        name: name.into(),
        applied: ready.then(|| applied(name)),
        secrets: secrets
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compose_orders_flags_layers_env_and_joins_instructions() {
        let parts = [
            part("one", true, &[("ONE_TOKEN", "1"), ("KB_REGION", "secret")]),
            part("two", false, &[("TWO_TOKEN", "2"), ("ONE_TOKEN", "x")]),
            part("three", true, &[]),
        ];
        let c = compose(&parts);
        assert_eq!(
            c.args,
            [
                "--agents",
                r#"{"one-agent":{}}"#,
                "--mcp-config",
                r#"{"mcpServers":{"one":{}}}"#,
                "--agents",
                r#"{"three-agent":{}}"#,
                "--mcp-config",
                r#"{"mcpServers":{"three":{}}}"#,
            ]
        );
        let pair = |k: &str, v: &str| (k.to_owned(), v.to_owned());
        // [env] first (the first plugin's key wins), then the secrets.
        assert_eq!(
            c.env,
            [
                pair("ONE_TOKEN", "1"),
                pair("KB_REGION", "secret"),
                pair("TWO_TOKEN", "2"),
            ]
        );
        assert_eq!(c.instructions, "Use one.\n\nUse three.");
        assert!(c.notices.is_empty());
        // A plugin without agents or servers brings no flag.
        let mut bare = part("bare", true, &[]);
        let a = bare.applied.as_mut().unwrap();
        (a.agents, a.mcp_config) = (None, None);
        assert!(compose(&[bare]).args.is_empty());
    }

    #[test]
    fn debug_output_never_shows_env_or_secret_values() {
        let parts = [part("one", true, &[("ONE_TOKEN", "hunter2-secret")])];
        let c = compose(&parts);
        let shown = format!("{parts:?} {c:?}");
        assert!(!shown.contains("hunter2-secret"), "{shown}");
        assert!(shown.contains("ONE_TOKEN"), "{shown}");
    }

    #[test]
    fn compose_takes_eight_plugins_and_cuts_instructions_at_8_kib() {
        let parts: Vec<PluginPart> = (0..10).map(|i| part(&format!("p{i}"), true, &[])).collect();
        let c = compose(&parts);
        assert_eq!(c.args.len(), 8 * 4);
        assert_eq!(
            c.notices,
            [
                "plugin p8 left out: too many plugins",
                "plugin p9 left out: too many plugins",
            ]
        );
        let big = |name: &str, size: usize| {
            let mut p = part(name, true, &[]);
            p.applied.as_mut().unwrap().instructions = "i".repeat(size);
            p
        };
        let fits = MAX_PLUGIN_INSTRUCTIONS - 5000 - 2;
        let parts = [big("a", 5000), big("b", fits), big("c", 10)];
        let c = compose(&parts);
        assert_eq!(c.instructions.len(), MAX_PLUGIN_INSTRUCTIONS);
        assert_eq!(c.notices, ["plugin instructions over 8 KiB: c left out"]);
        let parts = [big("a", 5000), big("b", 4000), big("c", 10)];
        let c = compose(&parts);
        assert_eq!(c.instructions.len(), 5000);
        assert_eq!(c.notices, ["plugin instructions over 8 KiB: b, c left out"]);
    }
}
