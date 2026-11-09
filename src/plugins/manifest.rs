//! A plugin manifest: the TOML file a proxy plugin's `bundle` points at. It
//! says what a member's sessions get from the plugin: marketplaces and the
//! plugins to install from them, MCP servers (with the secret env each
//! needs), subagents, instructions for every session's context, and env.
//!
//! A manifest is untrusted input. [`Manifest::parse`] refuses anything
//! outside the grammar below, with a reason that names ids and fields,
//! never a value:
//!
//! - `format = 1`, `name` (`[a-z0-9-]{1,40}`), `version` (≤ 64 printable
//!   ASCII), `description` (text ≤ 200), `instructions` (text ≤ 8 KiB);
//! - `[[marketplace]]` ≤ 16: `name`, `source` (`github:<owner>/<repo>`,
//!   each `[A-Za-z0-9_.-]{1,64}`, or `https://…` ≤ 512 printable ASCII);
//! - `[[plugin]]` ≤ 64: `name`, `marketplace` (one of the manifest's);
//! - `[[mcp]]` ≤ 32: `name`, `type` (`http`, `sse` or `stdio`), `url` and
//!   `headers` (≤ 16, RFC 7230 token names ≤ 64, printable ASCII values ≤
//!   4 KiB) and `auth = "oauth"` for http/sse, `command` (≤ 256 printable
//!   ASCII), `args` (≤ 32 × text ≤ 1 KiB) and `env` (≤ 16) for stdio, and
//!   `secrets` (≤ 16 env names);
//! - `[[agent]]` ≤ 32: `name`, `description` (text ≤ 500), `model`
//!   (`[A-Za-z0-9._-]{1,64}`), `tools` (≤ 32 × `[A-Za-z0-9_:*()-]{1,64}`),
//!   `prompt` (text ≤ 16 KiB);
//! - `[env]` ≤ 64 keys.
//!
//! "Text" is UTF-8 without C0 controls but `\n` and `\t`. Names are
//! `[a-z0-9-]{1,40}`; env keys follow [`key_ok`]. Unknown keys, a name given
//! twice in a section, and an MCP server named like an agent are refused.
//! `${NAME}` in a server's `url`, `headers`, `args` and `env` is left for
//! Claude Code to expand from the session env: `NAME` follows [`key_ok`] and
//! is one of the server's `secrets` or a key of `[env]`; any other `${` is
//! refused. The renderers run on every item at parse time: an agents JSON
//! over 32 KiB or an MCP JSON over 64 KiB refuses the manifest.
//!
//! Every item has an id the member's choices and the hosts' reports use:
//! `plugin:<name>@<marketplace>`, `mcp:<name>`, `agent:<name>`,
//! `instructions` and `env`. A marketplace has none: it is added when one
//! of its plugins is.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::{json, Map, Value};

pub use super::names::{key_ok, name_ok};

/// A manifest's size cap, in bytes.
pub const MAX_BYTES: usize = 256 << 10;
/// The instructions' cap, in bytes.
pub const MAX_INSTRUCTIONS: usize = 8 << 10;
/// The rendered `--agents` JSON's cap, in bytes.
pub const MAX_AGENTS_JSON: usize = 32 << 10;
/// The rendered `--mcp-config` JSON's cap, in bytes.
pub const MAX_MCP_JSON: usize = 64 << 10;
/// An item id's cap, in bytes.
pub const MAX_ID: usize = 96;

const MAX_DESCRIPTION: usize = 200;
const MAX_MARKETPLACES: usize = 16;
const MAX_PLUGINS: usize = 64;
const MAX_SERVERS: usize = 32;
const MAX_AGENTS: usize = 32;
const MAX_ENV: usize = 64;
const MAX_VALUE: usize = 4096;
const MAX_URL: usize = 512;
const MAX_HEADERS: usize = 16;
const MAX_SECRETS: usize = 16;
const MAX_COMMAND: usize = 256;
const MAX_ARGS: usize = 32;
const MAX_ARG: usize = 1024;
const MAX_SERVER_ENV: usize = 16;
const MAX_AGENT_DESCRIPTION: usize = 500;
const MAX_PROMPT: usize = 16 << 10;
const MAX_TOOLS: usize = 32;

/// A parsed, checked manifest.
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    pub description: String,
    pub instructions: String,
    pub marketplaces: Vec<Marketplace>,
    pub plugins: Vec<PluginRef>,
    pub servers: Vec<Server>,
    pub agents: Vec<Agent>,
    /// `[env]`, sorted by key.
    pub env: Vec<(String, String)>,
}

/// A marketplace: the name it declares for itself (the key Claude Code
/// keeps it under) and where it is.
#[derive(Debug, Clone, PartialEq)]
pub struct Marketplace {
    pub name: String,
    /// As written: `github:<owner>/<repo>` or an https URL.
    pub source: String,
}

impl Marketplace {
    /// The source as `claude plugin marketplace add` takes it:
    /// `<owner>/<repo>` for a GitHub one, the URL otherwise.
    pub fn cli_source(&self) -> &str {
        self.source.strip_prefix("github:").unwrap_or(&self.source)
    }
}

/// A plugin to install from one of the manifest's marketplaces.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginRef {
    pub name: String,
    pub marketplace: String,
}

impl PluginRef {
    /// `<name>@<marketplace>`: Claude Code's id.
    pub fn spec(&self) -> String {
        format!("{}@{}", self.name, self.marketplace)
    }

    /// `plugin:<name>@<marketplace>`.
    pub fn id(&self) -> String {
        format!("plugin:{}", self.spec())
    }
}

/// An MCP server every session gets.
#[derive(Debug, Clone, PartialEq)]
pub struct Server {
    pub name: String,
    pub transport: Transport,
    /// The env names (never values) the server needs from the proxy.
    pub secrets: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Transport {
    Http {
        url: String,
        headers: Vec<(String, String)>,
        /// `auth = "oauth"`: the member signs in with `/mcp`.
        oauth: bool,
    },
    Sse {
        url: String,
        headers: Vec<(String, String)>,
        oauth: bool,
    },
    Stdio {
        command: String,
        args: Vec<String>,
        env: Vec<(String, String)>,
    },
}

impl Server {
    /// `mcp:<name>`.
    pub fn id(&self) -> String {
        format!("mcp:{}", self.name)
    }

    /// `http`, `sse` or `stdio`.
    pub fn kind(&self) -> &'static str {
        match self.transport {
            Transport::Http { .. } => "http",
            Transport::Sse { .. } => "sse",
            Transport::Stdio { .. } => "stdio",
        }
    }

    /// The server as Claude Code's MCP config has it.
    fn config(&self) -> Value {
        let map = |pairs: &[(String, String)]| {
            pairs
                .iter()
                .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                .collect::<Map<String, Value>>()
        };
        match &self.transport {
            Transport::Http { url, headers, .. } | Transport::Sse { url, headers, .. } => {
                let mut v = json!({ "type": self.kind(), "url": url });
                if !headers.is_empty() {
                    v["headers"] = Value::Object(map(headers));
                }
                v
            }
            Transport::Stdio { command, args, env } => {
                let mut v = json!({ "type": "stdio", "command": command, "args": args });
                if !env.is_empty() {
                    v["env"] = Value::Object(map(env));
                }
                v
            }
        }
    }
}

/// A subagent every session gets (`--agents`).
#[derive(Debug, Clone, PartialEq)]
pub struct Agent {
    pub name: String,
    pub description: String,
    pub model: Option<String>,
    pub tools: Option<Vec<String>>,
    pub prompt: String,
}

impl Agent {
    /// `agent:<name>`.
    pub fn id(&self) -> String {
        format!("agent:{}", self.name)
    }
}

/// Where an item is listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Plugins,
    Servers,
    Agents,
    Instructions,
    Env,
}

impl Section {
    pub fn heading(self) -> &'static str {
        match self {
            Section::Plugins => "Plugins",
            Section::Servers => "MCP servers",
            Section::Agents => "Agents",
            Section::Instructions => "Instructions",
            Section::Env => "Env",
        }
    }
}

/// One thing the member may turn off: its id, section and title.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub id: String,
    pub section: Section,
    pub title: String,
}

impl Manifest {
    /// Parse and check `src` (see the module doc). The reason names ids
    /// and fields, never a value.
    pub fn parse(src: &str) -> Result<Manifest, String> {
        if src.len() > MAX_BYTES {
            return Err("over 256 KiB".to_owned());
        }
        // `format` first: a newer format may have keys this one lacks.
        let table: toml::Table = toml::from_str(src).map_err(|e| toml_reason(src, &e))?;
        match table.get("format") {
            Some(toml::Value::Integer(1)) => {}
            Some(_) => return Err("format is not 1".to_owned()),
            None => return Err("missing key format".to_owned()),
        }
        let raw: Raw = toml::from_str(src).map_err(|e| toml_reason(src, &e))?;
        let manifest = raw.check()?;
        let all_agents = agents_json(&manifest.agents).unwrap_or_default();
        if all_agents.len() > MAX_AGENTS_JSON {
            return Err("agents JSON over 32 KiB".to_owned());
        }
        let all_servers = mcp_json(&manifest.servers).unwrap_or_default();
        if all_servers.len() > MAX_MCP_JSON {
            return Err("MCP JSON over 64 KiB".to_owned());
        }
        Ok(manifest)
    }

    /// [`Manifest::parse`], and the manifest must be plugin `name`'s.
    pub fn parse_named(src: &str, name: &str) -> Result<Manifest, String> {
        let manifest = Manifest::parse(src)?;
        if manifest.name != name {
            return Err(format!("the manifest is {}'s", manifest.name));
        }
        Ok(manifest)
    }

    /// The marketplace called `name`.
    pub fn marketplace(&self, name: &str) -> Option<&Marketplace> {
        self.marketplaces.iter().find(|m| m.name == name)
    }

    /// Every item, by section: plugins, servers, agents, then the
    /// instructions and the env when there are any.
    pub fn items(&self) -> Vec<Item> {
        let mut items = Vec::new();
        for p in &self.plugins {
            items.push(Item {
                id: p.id(),
                section: Section::Plugins,
                title: p.spec(),
            });
        }
        for s in &self.servers {
            let title = match &s.transport {
                Transport::Http { url, .. } | Transport::Sse { url, .. } => {
                    format!("{} ({} {url})", s.name, s.kind())
                }
                Transport::Stdio { command, args, .. } => {
                    let mut t = format!("{} (stdio: {command}", s.name);
                    for a in args {
                        t.push(' ');
                        t.push_str(a);
                    }
                    t.push(')');
                    t.chars().take(80).collect()
                }
            };
            items.push(Item {
                id: s.id(),
                section: Section::Servers,
                title,
            });
        }
        for a in &self.agents {
            items.push(Item {
                id: a.id(),
                section: Section::Agents,
                title: format!("{}: {}", a.name, a.description),
            });
        }
        if !self.instructions.trim().is_empty() {
            items.push(Item {
                id: "instructions".to_owned(),
                section: Section::Instructions,
                title: "instructions".to_owned(),
            });
        }
        if !self.env.is_empty() {
            items.push(Item {
                id: "env".to_owned(),
                section: Section::Env,
                title: format!("env ({} keys)", self.env.len()),
            });
        }
        items
    }

    /// Every item's id, in [`Manifest::items`] order.
    pub fn ids(&self) -> Vec<String> {
        self.items().into_iter().map(|i| i.id).collect()
    }
}

/// `{"mcpServers": {...}}` for `servers`, as `--mcp-config` takes it, or
/// `None` when there are none. Placeholders stay as written.
pub fn mcp_json<'a>(servers: impl IntoIterator<Item = &'a Server>) -> Option<String> {
    let map: Map<String, Value> = servers
        .into_iter()
        .map(|s| (s.name.clone(), s.config()))
        .collect();
    (!map.is_empty()).then(|| json!({ "mcpServers": map }).to_string())
}

/// `{"<name>": {"description", "prompt", "model"?, "tools"?}}` for
/// `agents`, as `--agents` takes it, or `None` when there are none.
pub fn agents_json<'a>(agents: impl IntoIterator<Item = &'a Agent>) -> Option<String> {
    let map: Map<String, Value> = agents
        .into_iter()
        .map(|a| {
            let mut v = json!({ "description": a.description, "prompt": a.prompt });
            if let Some(model) = &a.model {
                v["model"] = json!(model);
            }
            if let Some(tools) = &a.tools {
                v["tools"] = json!(tools);
            }
            (a.name.clone(), v)
        })
        .collect();
    (!map.is_empty()).then(|| Value::Object(map).to_string())
}

/// Whether `id` is an item id of the grammar (≤ [`MAX_ID`] bytes):
/// `plugin:<name>@<name>`, `mcp:<name>`, `agent:<name>`, `instructions`
/// or `env`.
pub fn id_ok(id: &str) -> bool {
    if id.len() > MAX_ID {
        return false;
    }
    if id == "instructions" || id == "env" {
        return true;
    }
    if let Some(rest) = id.strip_prefix("plugin:") {
        return rest
            .split_once('@')
            .is_some_and(|(p, m)| name_ok(p) && name_ok(m));
    }
    let name = id
        .strip_prefix("mcp:")
        .or_else(|| id.strip_prefix("agent:"));
    name.is_some_and(name_ok)
}

// ── Parsing ───────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    // Checked on the table before this is read.
    #[allow(dead_code)]
    format: i64,
    name: String,
    version: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    instructions: String,
    #[serde(default)]
    marketplace: Vec<RawMarketplace>,
    #[serde(default)]
    plugin: Vec<RawPlugin>,
    #[serde(default)]
    mcp: Vec<RawServer>,
    #[serde(default)]
    agent: Vec<RawAgent>,
    #[serde(default)]
    env: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMarketplace {
    name: String,
    source: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPlugin {
    name: String,
    marketplace: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawServer {
    name: String,
    #[serde(rename = "type")]
    kind: String,
    url: Option<String>,
    headers: Option<BTreeMap<String, String>>,
    #[serde(default)]
    secrets: Vec<String>,
    auth: Option<String>,
    command: Option<String>,
    args: Option<Vec<String>>,
    env: Option<BTreeMap<String, String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAgent {
    name: String,
    description: String,
    model: Option<String>,
    tools: Option<Vec<String>>,
    prompt: String,
}

/// Why `src` did not deserialize: an unknown or missing key by name, else
/// a generic reason; with the line, never the text.
fn toml_reason(src: &str, e: &toml::de::Error) -> String {
    let message = e.message();
    let key = |prefix: &str| {
        let rest = message.strip_prefix(prefix)?;
        let key = rest.split('`').next()?;
        let shown = (1..=64).contains(&key.len())
            && key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
        Some(if shown { key } else { "?" })
    };
    let what = if let Some(k) = key("unknown field `") {
        format!("unknown key {k}")
    } else if let Some(k) = key("missing field `") {
        format!("missing key {k}")
    } else {
        "not valid TOML, or a value of the wrong type".to_owned()
    };
    match e.span() {
        Some(span) => {
            let line = src[..span.start.min(src.len())].matches('\n').count() + 1;
            format!("{what} (line {line})")
        }
        None => what,
    }
}

/// UTF-8 without C0 controls but `\n` and `\t`.
fn is_text(s: &str) -> bool {
    !s.bytes().any(|b| b < 0x20 && b != b'\n' && b != b'\t')
}

/// Printable ASCII only.
fn is_printable(s: &str) -> bool {
    s.bytes().all(|b| (0x20..0x7f).contains(&b))
}

/// `what` holds text of at most `max` bytes.
fn check_text(what: &str, s: &str, max: usize) -> Result<(), String> {
    if s.len() > max {
        return Err(format!("{what} over {max} bytes"));
    }
    if !is_text(s) {
        return Err(format!("{what}: control character"));
    }
    Ok(())
}

/// `[<extra> alphanumeric]{1,max}`.
fn word_ok(s: &str, max: usize, extra: &[u8]) -> bool {
    (1..=max).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || extra.contains(&b))
}

/// An RFC 7230 token of at most 64 bytes: a header name.
fn header_name_ok(s: &str) -> bool {
    word_ok(s, 64, b"!#$%&'*+-.^_`|~")
}

/// `github:<owner>/<repo>` or an https URL of at most 512 printable bytes.
fn source_ok(s: &str) -> bool {
    if let Some(rest) = s.strip_prefix("github:") {
        // Neither part may read as a flag or a path to the CLI.
        let part = |p: &str| word_ok(p, 64, b"_.-") && !p.starts_with(['-', '.']);
        return rest
            .split_once('/')
            .is_some_and(|(o, r)| part(o) && part(r));
    }
    s.len() > "https://".len() && s.len() <= MAX_URL && s.starts_with("https://") && is_printable(s)
}

/// The names of the `${NAME}` placeholders in `s`, or `None` when a `${`
/// is not one.
fn placeholders(s: &str) -> Option<Vec<&str>> {
    let mut names = Vec::new();
    let mut rest = s;
    while let Some(at) = rest.find("${") {
        let after = &rest[at + 2..];
        let end = after.find('}')?;
        let name = &after[..end];
        key_ok(name).ok()?;
        names.push(name);
        rest = &after[end + 1..];
    }
    Some(names)
}

/// Every `${NAME}` of `values` (field `what` of server `id`) is declared:
/// one of `secrets` or a key of `env`.
fn check_placeholders<'a>(
    id: &str,
    what: &str,
    values: impl IntoIterator<Item = &'a String>,
    secrets: &[String],
    env: &BTreeMap<String, String>,
) -> Result<(), String> {
    for value in values {
        let names = placeholders(value).ok_or_else(|| format!("{id}: {what}: bad placeholder"))?;
        for name in names {
            if !secrets.iter().any(|s| s == name) && !env.contains_key(name) {
                return Err(format!(
                    "{id}: {what}: ${{{name}}} is not in secrets or [env]"
                ));
            }
        }
    }
    Ok(())
}

/// The pairs of `map`, each key by `key_ok` and each value text of at most
/// `max` bytes; at most `count` of them.
fn check_env(
    what: &str,
    map: &BTreeMap<String, String>,
    count: usize,
    max: usize,
) -> Result<Vec<(String, String)>, String> {
    if map.len() > count {
        return Err(format!("{what}: more than {count} keys"));
    }
    for (key, value) in map {
        key_ok(key).map_err(|why| format!("{what}: {why}"))?;
        check_text(&format!("{what} {key}"), value, max)?;
    }
    Ok(map.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
}

impl Raw {
    fn check(self) -> Result<Manifest, String> {
        if !name_ok(&self.name) {
            return Err("bad name".to_owned());
        }
        if !(1..=64).contains(&self.version.len()) || !is_printable(&self.version) {
            return Err("bad version".to_owned());
        }
        check_text("description", &self.description, MAX_DESCRIPTION)?;
        check_text("instructions", &self.instructions, MAX_INSTRUCTIONS)?;
        let env = check_env("env", &self.env, MAX_ENV, MAX_VALUE)?;

        if self.marketplace.len() > MAX_MARKETPLACES {
            return Err(format!("more than {MAX_MARKETPLACES} marketplaces"));
        }
        let mut marketplaces: Vec<Marketplace> = Vec::new();
        for (i, m) in self.marketplace.into_iter().enumerate() {
            if !name_ok(&m.name) {
                return Err(format!("marketplace #{}: bad name", i + 1));
            }
            if marketplaces.iter().any(|o| o.name == m.name) {
                return Err(format!("marketplace {}: given twice", m.name));
            }
            if !source_ok(&m.source) {
                return Err(format!("marketplace {}: bad source", m.name));
            }
            marketplaces.push(Marketplace {
                name: m.name,
                source: m.source,
            });
        }

        if self.plugin.len() > MAX_PLUGINS {
            return Err(format!("more than {MAX_PLUGINS} plugins"));
        }
        let mut plugins: Vec<PluginRef> = Vec::new();
        for (i, p) in self.plugin.into_iter().enumerate() {
            if !name_ok(&p.name) || !name_ok(&p.marketplace) {
                return Err(format!("plugin #{}: bad name", i + 1));
            }
            let p = PluginRef {
                name: p.name,
                marketplace: p.marketplace,
            };
            if plugins.iter().any(|o| o.name == p.name) {
                return Err(format!("plugin {}: given twice", p.name));
            }
            if !marketplaces.iter().any(|m| m.name == p.marketplace) {
                return Err(format!(
                    "{}: marketplace {} not declared",
                    p.id(),
                    p.marketplace
                ));
            }
            plugins.push(p);
        }

        if self.agent.len() > MAX_AGENTS {
            return Err(format!("more than {MAX_AGENTS} agents"));
        }
        let mut agents: Vec<Agent> = Vec::new();
        for (i, a) in self.agent.into_iter().enumerate() {
            if !name_ok(&a.name) {
                return Err(format!("agent #{}: bad name", i + 1));
            }
            let id = format!("agent:{}", a.name);
            if agents.iter().any(|o| o.name == a.name) {
                return Err(format!("{id}: given twice"));
            }
            if a.description.trim().is_empty() {
                return Err(format!("{id}: description is empty"));
            }
            check_text(
                &format!("{id}: description"),
                &a.description,
                MAX_AGENT_DESCRIPTION,
            )?;
            if a.prompt.trim().is_empty() {
                return Err(format!("{id}: prompt is empty"));
            }
            check_text(&format!("{id}: prompt"), &a.prompt, MAX_PROMPT)?;
            if a.model.as_ref().is_some_and(|m| !word_ok(m, 64, b"._-")) {
                return Err(format!("{id}: bad model"));
            }
            if let Some(tools) = &a.tools {
                if tools.len() > MAX_TOOLS {
                    return Err(format!("{id}: more than {MAX_TOOLS} tools"));
                }
                if tools.iter().any(|t| !word_ok(t, 64, b"_:*()-")) {
                    return Err(format!("{id}: bad tool"));
                }
            }
            agents.push(Agent {
                name: a.name,
                description: a.description,
                model: a.model,
                tools: a.tools,
                prompt: a.prompt,
            });
        }

        if self.mcp.len() > MAX_SERVERS {
            return Err(format!("more than {MAX_SERVERS} MCP servers"));
        }
        let mut servers: Vec<Server> = Vec::new();
        for (i, s) in self.mcp.into_iter().enumerate() {
            if !name_ok(&s.name) {
                return Err(format!("mcp #{}: bad name", i + 1));
            }
            let id = format!("mcp:{}", s.name);
            if servers.iter().any(|o| o.name == s.name) {
                return Err(format!("{id}: given twice"));
            }
            if agents.iter().any(|a| a.name == s.name) {
                return Err(format!("{id}: an agent has this name"));
            }
            servers.push(s.check(&id, &self.env)?);
        }

        Ok(Manifest {
            name: self.name,
            version: self.version,
            description: self.description,
            instructions: self.instructions,
            marketplaces,
            plugins,
            servers,
            agents,
            env,
        })
    }
}

impl RawServer {
    /// The server `id`, with `env` the manifest's `[env]`.
    fn check(self, id: &str, env: &BTreeMap<String, String>) -> Result<Server, String> {
        if self.secrets.len() > MAX_SECRETS {
            return Err(format!("{id}: more than {MAX_SECRETS} secrets"));
        }
        for secret in &self.secrets {
            key_ok(secret).map_err(|why| format!("{id}: secrets: {why}"))?;
        }
        let transport = match self.kind.as_str() {
            "http" | "sse" => {
                if self.command.is_some() || self.args.is_some() || self.env.is_some() {
                    return Err(format!(
                        "{id}: command, args or env on an {} server",
                        self.kind
                    ));
                }
                let url = self.url.ok_or_else(|| format!("{id}: missing key url"))?;
                if url.len() > MAX_URL || !url.starts_with("https://") || !is_printable(&url) {
                    return Err(format!("{id}: url is not https"));
                }
                let headers = self.headers.unwrap_or_default();
                if headers.len() > MAX_HEADERS {
                    return Err(format!("{id}: more than {MAX_HEADERS} headers"));
                }
                for (name, value) in &headers {
                    if !header_name_ok(name) {
                        return Err(format!("{id}: bad header name"));
                    }
                    if value.len() > MAX_VALUE || !is_printable(value) {
                        return Err(format!("{id}: header {name}: bad value"));
                    }
                }
                let oauth = match self.auth.as_deref() {
                    None => false,
                    Some("oauth") => true,
                    Some(_) => return Err(format!("{id}: auth is not oauth")),
                };
                if oauth && (!headers.is_empty() || !self.secrets.is_empty()) {
                    return Err(format!("{id}: auth = oauth takes no headers or secrets"));
                }
                check_placeholders(id, "url", [&url], &self.secrets, env)?;
                check_placeholders(id, "headers", headers.values(), &self.secrets, env)?;
                let headers = headers.into_iter().collect();
                if self.kind == "http" {
                    Transport::Http {
                        url,
                        headers,
                        oauth,
                    }
                } else {
                    Transport::Sse {
                        url,
                        headers,
                        oauth,
                    }
                }
            }
            "stdio" => {
                if self.url.is_some() || self.headers.is_some() || self.auth.is_some() {
                    return Err(format!("{id}: url, headers or auth on a stdio server"));
                }
                let command = self
                    .command
                    .ok_or_else(|| format!("{id}: missing key command"))?;
                if command.is_empty() || command.len() > MAX_COMMAND || !is_printable(&command) {
                    return Err(format!("{id}: bad command"));
                }
                if command.contains("${") {
                    return Err(format!("{id}: command: bad placeholder"));
                }
                let args = self.args.unwrap_or_default();
                if args.len() > MAX_ARGS {
                    return Err(format!("{id}: more than {MAX_ARGS} args"));
                }
                for arg in &args {
                    check_text(&format!("{id}: arg"), arg, MAX_ARG)?;
                }
                let server_env = self.env.unwrap_or_default();
                let pairs = check_env(
                    &format!("{id}: env"),
                    &server_env,
                    MAX_SERVER_ENV,
                    MAX_VALUE,
                )?;
                check_placeholders(id, "args", &args, &self.secrets, env)?;
                check_placeholders(id, "env", server_env.values(), &self.secrets, env)?;
                Transport::Stdio {
                    command,
                    args,
                    env: pairs,
                }
            }
            _ => return Err(format!("{id}: type is not http, sse or stdio")),
        };
        Ok(Server {
            name: self.name,
            transport,
            secrets: self.secrets,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A manifest with one of everything.
    const GOOD: &str = r#"
format = 1
name = "team"
version = "1.0.0"
description = "Team setup"
instructions = """
Search the knowledge base first.
"""

[[marketplace]]
name = "vocdoni"
source = "github:vocdoni/skills"

[[marketplace]]
name = "other"
source = "https://example.org/market.git"

[[plugin]]
name = "agentic-dev"
marketplace = "vocdoni"

[[mcp]]
name = "kb"
type = "http"
url = "https://kb.example.org/mcp"
headers = { Authorization = "Bearer ${KB_TOKEN}" }
secrets = ["KB_TOKEN"]

[[mcp]]
name = "notes"
type = "sse"
url = "https://notes.example.org/sse"
auth = "oauth"

[[mcp]]
name = "local"
type = "stdio"
command = "npx"
args = ["-y", "server", "--region=${REGION}"]
env = { API_KEY = "${LOCAL_KEY}" }
secrets = ["LOCAL_KEY"]

[[agent]]
name = "reviewer"
description = "Reviews code"
model = "opus"
tools = ["Read", "Grep", "mcp__kb__*"]
prompt = "You review code."

[env]
REGION = "eu"
"#;

    fn good() -> Manifest {
        Manifest::parse(GOOD).unwrap()
    }

    /// Why `GOOD` with `from` replaced by `to` is refused.
    fn refused(from: &str, to: &str) -> String {
        assert!(GOOD.contains(from), "{from:?} is not in GOOD");
        Manifest::parse(&GOOD.replacen(from, to, 1)).unwrap_err()
    }

    /// Why `GOOD` with `extra` appended is refused.
    fn refused_with(extra: &str) -> String {
        Manifest::parse(&format!("{GOOD}\n{extra}")).unwrap_err()
    }

    #[test]
    fn a_good_manifest_parses_with_every_item() {
        let m = good();
        assert_eq!((m.name.as_str(), m.version.as_str()), ("team", "1.0.0"));
        assert_eq!(m.marketplaces[0].cli_source(), "vocdoni/skills");
        assert_eq!(
            m.marketplaces[1].cli_source(),
            "https://example.org/market.git"
        );
        assert_eq!(m.plugins[0].spec(), "agentic-dev@vocdoni");
        assert_eq!(m.env, [("REGION".to_owned(), "eu".to_owned())]);
        assert_eq!(
            m.ids(),
            [
                "plugin:agentic-dev@vocdoni",
                "mcp:kb",
                "mcp:notes",
                "mcp:local",
                "agent:reviewer",
                "instructions",
                "env",
            ]
        );
        let titles: Vec<String> = m.items().into_iter().map(|i| i.title).collect();
        assert_eq!(
            titles,
            [
                "agentic-dev@vocdoni",
                "kb (http https://kb.example.org/mcp)",
                "notes (sse https://notes.example.org/sse)",
                "local (stdio: npx -y server --region=${REGION})",
                "reviewer: Reviews code",
                "instructions",
                "env (1 keys)",
            ]
        );
        assert!(m.ids().iter().all(|id| id_ok(id)));
    }

    #[test]
    fn a_stdio_title_shows_its_command_and_args_cut_at_80_chars() {
        let src = GOOD.replace(
            "args = [\"-y\", \"server\", \"--region=${REGION}\"]",
            "args = [\"-y\", \"server\", \"--a-very-long-argument-that-goes-on-and-on-and-on-and-on-and-on\"]",
        );
        let m = Manifest::parse(&src).unwrap();
        let title = m
            .items()
            .into_iter()
            .find(|i| i.id == "mcp:local")
            .unwrap()
            .title;
        assert_eq!(title.chars().count(), 80);
        assert!(title.starts_with("local (stdio: npx -y server --a-very-long"));
    }

    #[test]
    fn the_teams_draft_manifest_parses() {
        let src = include_str!("testdata/vocdoni-internal.toml");
        let m = Manifest::parse_named(src, "vocdoni-internal").unwrap();
        assert_eq!(
            (m.plugins.len(), m.servers.len(), m.agents.len()),
            (13, 3, 3)
        );
        assert!(agents_json(&m.agents).unwrap().len() <= MAX_AGENTS_JSON);
        let err = Manifest::parse_named(src, "other").unwrap_err();
        assert_eq!(err, "the manifest is vocdoni-internal's");
    }

    #[test]
    fn the_mcp_json_has_each_type_as_claude_code_expects() {
        let m = good();
        let v: Value = serde_json::from_str(&mcp_json(&m.servers).unwrap()).unwrap();
        assert_eq!(
            v,
            json!({"mcpServers": {
                "kb": {"type": "http", "url": "https://kb.example.org/mcp",
                       "headers": {"Authorization": "Bearer ${KB_TOKEN}"}},
                "notes": {"type": "sse", "url": "https://notes.example.org/sse"},
                "local": {"type": "stdio", "command": "npx",
                          "args": ["-y", "server", "--region=${REGION}"],
                          "env": {"API_KEY": "${LOCAL_KEY}"}},
            }})
        );
        assert_eq!(mcp_json(&[]), None);
        let only_kb = mcp_json(m.servers.iter().filter(|s| s.name == "kb")).unwrap();
        assert!(!only_kb.contains("notes"), "{only_kb}");
    }

    #[test]
    fn the_agents_json_has_the_optional_fields_only_when_given() {
        let mut m = good();
        let v: Value = serde_json::from_str(&agents_json(&m.agents).unwrap()).unwrap();
        assert_eq!(
            v,
            json!({"reviewer": {"description": "Reviews code", "prompt": "You review code.",
                                "model": "opus", "tools": ["Read", "Grep", "mcp__kb__*"]}})
        );
        m.agents[0].model = None;
        m.agents[0].tools = None;
        let v: Value = serde_json::from_str(&agents_json(&m.agents).unwrap()).unwrap();
        assert_eq!(
            v,
            json!({"reviewer": {"description": "Reviews code", "prompt": "You review code."}})
        );
        assert_eq!(agents_json(&[]), None);
    }

    #[test]
    fn format_and_toml_errors_name_no_value() {
        assert_eq!(refused("format = 1", "format = 2"), "format is not 1");
        assert_eq!(refused("format = 1", "format = \"1\""), "format is not 1");
        assert_eq!(refused("format = 1\n", ""), "missing key format");
        // A newer format's keys do not matter: the format is refused first.
        let newer = GOOD.replacen("format = 1", "format = 2\nfuture = true", 1);
        assert_eq!(Manifest::parse(&newer).unwrap_err(), "format is not 1");
        let err = refused("version = \"1.0.0\"", "version = \"1.0.0");
        assert!(err.starts_with("not valid TOML"), "{err}");
        assert!(err.ends_with("(line 4)"), "{err}");
        assert!(!err.contains("1.0.0"), "{err}");
        let err = refused("model = \"opus\"", "model = 5");
        assert!(
            err.starts_with("not valid TOML, or a value of the wrong type"),
            "{err}"
        );
        assert_eq!(
            Manifest::parse(&"#".repeat(MAX_BYTES + 1)).unwrap_err(),
            "over 256 KiB"
        );
    }

    #[test]
    fn unknown_and_missing_keys_are_refused_by_name() {
        let err = refused(
            "version = \"1.0.0\"",
            "version = \"1.0.0\"\nhomepage = \"x\"",
        );
        assert!(err.starts_with("unknown key homepage"), "{err}");
        let err = refused("model = \"opus\"", "model = \"opus\"\ncolor = \"red\"");
        assert!(err.starts_with("unknown key color"), "{err}");
        let err = refused("[env]", "[extra]\nx = 1\n[env]");
        assert!(err.starts_with("unknown key extra"), "{err}");
        let err = refused("name = \"team\"\n", "");
        assert!(err.starts_with("missing key name"), "{err}");
        let err = refused("prompt = \"You review code.\"\n", "");
        assert!(err.starts_with("missing key prompt"), "{err}");
    }

    #[test]
    fn top_level_caps_and_charsets() {
        assert_eq!(refused("name = \"team\"", "name = \"Team\""), "bad name");
        assert_eq!(
            refused("version = \"1.0.0\"", "version = \"\""),
            "bad version"
        );
        let long = format!("version = \"{}\"", "1".repeat(65));
        assert_eq!(refused("version = \"1.0.0\"", &long), "bad version");
        let long = format!("description = \"{}\"", "d".repeat(201));
        assert_eq!(
            refused("description = \"Team setup\"", &long),
            "description over 200 bytes"
        );
        assert_eq!(
            refused(
                "description = \"Team setup\"",
                "description = \"a\\u0007b\""
            ),
            "description: control character"
        );
        assert_eq!(
            refused(
                "Search the knowledge base first.",
                &"i".repeat(MAX_INSTRUCTIONS)
            ),
            "instructions over 8192 bytes"
        );
    }

    #[test]
    fn env_keys_follow_the_key_rule_and_values_are_text() {
        assert_eq!(
            refused("REGION = \"eu\"", "LD_PRELOAD = \"x\""),
            "env: env key LD_PRELOAD not allowed"
        );
        assert_eq!(
            refused("REGION = \"eu\"", "region = \"x\""),
            "env: bad env key"
        );
        let long = format!("REGION = \"{}\"", "v".repeat(MAX_VALUE + 1));
        assert_eq!(
            refused("REGION = \"eu\"", &long),
            "env REGION over 4096 bytes"
        );
        let many: String = (0..65).map(|i| format!("K{i} = \"v\"\n")).collect();
        assert_eq!(refused("REGION = \"eu\"", &many), "env: more than 64 keys");
    }

    #[test]
    fn marketplace_sources_and_names() {
        for (source, ok) in [
            ("github:vocdoni/skills", true),
            ("github:anthropics/claude-plugins-official", true),
            ("https://example.org/m.git", true),
            ("github:vocdoni", false),
            ("github:a/b/c", false),
            ("github:-x/skills", false),
            ("github:../skills", false),
            ("github:vocdoni/..", false),
            ("github:voc doni/skills", false),
            ("http://example.org/m.git", false),
            ("file:///etc", false),
            ("/home/me/market", false),
            ("git@github.com:a/b.git", false),
            ("https://", false),
        ] {
            let src = GOOD.replacen("github:vocdoni/skills", source, 1);
            match Manifest::parse(&src) {
                Ok(_) => assert!(ok, "{source} accepted"),
                Err(e) => {
                    assert!(!ok, "{source}: {e}");
                    assert_eq!(e, "marketplace vocdoni: bad source");
                }
            }
        }
        let long = format!("source = \"https://e.org/{}\"", "a".repeat(500));
        assert_eq!(
            refused("source = \"https://example.org/market.git\"", &long),
            "marketplace other: bad source"
        );
        assert_eq!(
            refused("name = \"other\"", "name = \"vocdoni\""),
            "marketplace vocdoni: given twice"
        );
        assert_eq!(
            refused("name = \"other\"", "name = \"Other\""),
            "marketplace #2: bad name"
        );
        let many: String = (0..17)
            .map(|i| format!("[[marketplace]]\nname = \"m{i}\"\nsource = \"github:a/b\"\n"))
            .collect();
        assert_eq!(refused_with(&many), "more than 16 marketplaces");
    }

    #[test]
    fn plugins_need_a_declared_marketplace_and_unique_names() {
        assert_eq!(
            refused("marketplace = \"vocdoni\"", "marketplace = \"nope\""),
            "plugin:agentic-dev@nope: marketplace nope not declared"
        );
        assert_eq!(
            refused_with("[[plugin]]\nname = \"agentic-dev\"\nmarketplace = \"other\""),
            "plugin agentic-dev: given twice"
        );
        assert_eq!(
            refused("name = \"agentic-dev\"", "name = \"Agentic\""),
            "plugin #1: bad name"
        );
        let many: String = (0..64)
            .map(|i| format!("[[plugin]]\nname = \"p{i}\"\nmarketplace = \"vocdoni\"\n"))
            .collect();
        assert_eq!(refused_with(&many), "more than 64 plugins");
    }

    #[test]
    fn remote_servers_are_https_with_token_headers() {
        let kb_url = "url = \"https://kb.example.org/mcp\"";
        assert_eq!(
            refused(kb_url, "url = \"http://kb.example.org/mcp\""),
            "mcp:kb: url is not https"
        );
        let long = format!("url = \"https://e.org/{}\"", "a".repeat(500));
        assert_eq!(refused(kb_url, &long), "mcp:kb: url is not https");
        assert_eq!(
            refused("{ Authorization =", "{ \"Bad Name\" ="),
            "mcp:kb: bad header name"
        );
        assert_eq!(
            refused("\"Bearer ${KB_TOKEN}\"", "\"Bearer\\n${KB_TOKEN}\""),
            "mcp:kb: header Authorization: bad value"
        );
        assert_eq!(
            refused("type = \"http\"", "type = \"ws\""),
            "mcp:kb: type is not http, sse or stdio"
        );
        assert_eq!(
            refused(kb_url, &format!("{kb_url}\ncommand = \"x\"")),
            "mcp:kb: command, args or env on an http server"
        );
        assert_eq!(refused(kb_url, ""), "mcp:kb: missing key url");
        let many: String = (0..17).map(|i| format!("H{i} = \"v\", ")).collect();
        assert_eq!(
            refused("{ Authorization =", &format!("{{ {many}Authorization =")),
            "mcp:kb: more than 16 headers"
        );
    }

    #[test]
    fn oauth_servers_take_no_headers_or_secrets() {
        let oauth = "auth = \"oauth\"";
        assert_eq!(
            refused(oauth, "auth = \"basic\""),
            "mcp:notes: auth is not oauth"
        );
        assert_eq!(
            refused(oauth, &format!("{oauth}\nheaders = {{ X = \"y\" }}")),
            "mcp:notes: auth = oauth takes no headers or secrets"
        );
        assert_eq!(
            refused(oauth, &format!("{oauth}\nsecrets = [\"NOTES_KEY\"]")),
            "mcp:notes: auth = oauth takes no headers or secrets"
        );
    }

    #[test]
    fn stdio_servers_have_a_command_and_text_args() {
        assert_eq!(
            refused(
                "command = \"npx\"",
                "command = \"npx\"\nurl = \"https://e.org\""
            ),
            "mcp:local: url, headers or auth on a stdio server"
        );
        assert_eq!(
            refused("command = \"npx\"\n", ""),
            "mcp:local: missing key command"
        );
        assert_eq!(
            refused("command = \"npx\"", "command = \"\""),
            "mcp:local: bad command"
        );
        assert_eq!(
            refused("command = \"npx\"", "command = \"${LOCAL_KEY}\""),
            "mcp:local: command: bad placeholder"
        );
        assert_eq!(
            refused("\"-y\",", "\"a\\u0000\","),
            "mcp:local: arg: control character"
        );
        let long = format!("\"{}\",", "a".repeat(MAX_ARG + 1));
        assert_eq!(refused("\"-y\",", &long), "mcp:local: arg over 1024 bytes");
        let many: String = (0..32).map(|i| format!("\"a{i}\", ")).collect();
        assert_eq!(
            refused("args = [", &format!("args = [{many}")),
            "mcp:local: more than 32 args"
        );
        assert_eq!(
            refused("API_KEY =", "HOME ="),
            "mcp:local: env: env key HOME not allowed"
        );
    }

    #[test]
    fn placeholders_must_be_declared_secrets_or_env() {
        assert_eq!(
            refused("secrets = [\"KB_TOKEN\"]", "secrets = []"),
            "mcp:kb: headers: ${KB_TOKEN} is not in secrets or [env]"
        );
        assert_eq!(
            refused("REGION = \"eu\"", "OTHER = \"eu\""),
            "mcp:local: args: ${REGION} is not in secrets or [env]"
        );
        for bad in [
            "${kb_token}",
            "${KB_TOKEN",
            "${KB_TOKEN:-x}",
            "${HOME}",
            "${}",
        ] {
            let to = format!("\"Bearer {bad}\"");
            assert_eq!(
                refused("\"Bearer ${KB_TOKEN}\"", &to),
                "mcp:kb: headers: bad placeholder",
                "{bad}"
            );
        }
        assert_eq!(
            refused("/mcp\"", "/${KB_TOKEN}/${X}\""),
            "mcp:kb: url: ${X} is not in secrets or [env]"
        );
        assert_eq!(
            refused("${LOCAL_KEY}\" }", "${NOPE}\" }"),
            "mcp:local: env: ${NOPE} is not in secrets or [env]"
        );
        assert_eq!(
            refused("secrets = [\"KB_TOKEN\"]", "secrets = [\"PATH\"]"),
            "mcp:kb: secrets: env key PATH not allowed"
        );
    }

    #[test]
    fn servers_and_agents_have_unique_names_and_caps() {
        assert_eq!(
            refused("name = \"notes\"", "name = \"kb\""),
            "mcp:kb: given twice"
        );
        assert_eq!(
            refused("name = \"notes\"", "name = \"reviewer\""),
            "mcp:reviewer: an agent has this name"
        );
        assert_eq!(
            refused_with("[[agent]]\nname = \"reviewer\"\ndescription = \"d\"\nprompt = \"p\""),
            "agent:reviewer: given twice"
        );
        let many: String = (0..30)
            .map(|i| format!("[[mcp]]\nname = \"s{i}\"\ntype = \"sse\"\nurl = \"https://e.org\"\n"))
            .collect();
        assert_eq!(refused_with(&many), "more than 32 MCP servers");
        let many: String = (0..32)
            .map(|i| format!("[[agent]]\nname = \"a{i}\"\ndescription = \"d\"\nprompt = \"p\"\n"))
            .collect();
        assert_eq!(refused_with(&many), "more than 32 agents");
        let secrets: Vec<String> = (0..17).map(|i| format!("\"S{i}\"")).collect();
        assert_eq!(
            refused(
                "secrets = [\"KB_TOKEN\"]",
                &format!("secrets = [\"KB_TOKEN\", {}]", secrets.join(", "))
            ),
            "mcp:kb: more than 16 secrets"
        );
    }

    #[test]
    fn agents_have_text_and_tool_and_model_charsets() {
        assert_eq!(
            refused("model = \"opus\"", "model = \"op us\""),
            "agent:reviewer: bad model"
        );
        assert_eq!(
            refused("\"Grep\",", "\"Gr ep\","),
            "agent:reviewer: bad tool"
        );
        let tools: Vec<String> = (0..33).map(|i| format!("\"T{i}\"")).collect();
        assert_eq!(
            refused(
                "tools = [\"Read\", \"Grep\", \"mcp__kb__*\"]",
                &format!("tools = [{}]", tools.join(", "))
            ),
            "agent:reviewer: more than 32 tools"
        );
        assert_eq!(
            refused("prompt = \"You review code.\"", "prompt = \"  \""),
            "agent:reviewer: prompt is empty"
        );
        assert_eq!(
            refused("description = \"Reviews code\"", "description = \"\""),
            "agent:reviewer: description is empty"
        );
        let long = format!("prompt = \"{}\"", "p".repeat(MAX_PROMPT + 1));
        assert_eq!(
            refused("prompt = \"You review code.\"", &long),
            "agent:reviewer: prompt over 16384 bytes"
        );
        let long = format!("description = \"{}\"", "d".repeat(501));
        assert_eq!(
            refused("description = \"Reviews code\"", &long),
            "agent:reviewer: description over 500 bytes"
        );
    }

    #[test]
    fn the_rendered_caps_are_the_rule() {
        // Three prompts at their own cap: each fits, the JSON does not.
        let agents: String = (0..3)
            .map(|i| {
                format!(
                    "[[agent]]\nname = \"big{i}\"\ndescription = \"d\"\nprompt = \"{}\"\n",
                    "p".repeat(MAX_PROMPT - 100)
                )
            })
            .collect();
        assert_eq!(refused_with(&agents), "agents JSON over 32 KiB");
        // Sixteen 4 KiB headers on two servers: over 64 KiB rendered.
        let headers: Vec<String> = (0..16)
            .map(|i| format!("H{i} = \"{}\"", "v".repeat(MAX_VALUE - 10)))
            .collect();
        let servers: String = (0..2)
            .map(|i| {
                format!(
                    "[[mcp]]\nname = \"big{i}\"\ntype = \"http\"\nurl = \"https://e.org\"\nheaders = {{ {} }}\n",
                    headers.join(", ")
                )
            })
            .collect();
        assert_eq!(refused_with(&servers), "MCP JSON over 64 KiB");
    }

    #[test]
    fn ids_follow_the_grammar() {
        for id in [
            "plugin:a@b",
            "mcp:kb",
            "agent:reviewer",
            "instructions",
            "env",
        ] {
            assert!(id_ok(id), "{id}");
        }
        let long = format!("plugin:{}@{}", "a".repeat(40), "b".repeat(50));
        for id in [
            "plugin:a",
            "plugin:A@b",
            "mcp:",
            "agent:x y",
            "skill:x",
            "",
            "ENV",
            long.as_str(),
        ] {
            assert!(!id_ok(id), "{id}");
        }
    }

    #[test]
    fn an_empty_manifest_has_no_items() {
        let m = Manifest::parse("format = 1\nname = \"x\"\nversion = \"1\"").unwrap();
        assert!(m.items().is_empty());
        assert_eq!((mcp_json(&m.servers), agents_json(&m.agents)), (None, None));
    }
}
