# Plugin manifests: design

Status: draft   Class: architectural
Owner decisions so far (2026-11-06):
- A proxy plugin is a TOML manifest, not a zip: it declares marketplaces, plugins, MCP servers (with the secret each needs), subagents, instructions and env; claudio installs and configures all of it. Installing plugins into the member's `~/.claude` is wanted.
- A member who already holds a proxy user token needs no browser login; the token alone grants the manifest and its secrets. The browser login stays for members without one.
- The proxy side (merged, live on claude.vocdoni.net) stays as is: it serves the manifest's URL + SHA-256 and the secret env.
- The first manifest is `vocdoni-internal` in `vocdoni/skills`; it pulls in every plugin of the vocdoni marketplace plus firecrawl, context7 and remember from the official one, and the MCP servers vocdoni-kb, jina and hackmd.

Supersedes the zip bundle of `docs/specs/2026-11-06-session-plugins-design.md`
(merged to `main` at `ed65dec`, never released). The proxy spec
`claude-proxy/docs/specs/2026-11-06-claudio-plugins-design.md` is unchanged except
that `bundle` now points at a manifest and `installed_id` is unused.

## Problem

The merged design hands Claude Code one zip per plugin with `--plugin-dir`. A zip
is always exactly one plugin (verified: a zip of two plugin folders loads as one
empty plugin), so a team bundle would have to merge every team plugin into one,
renaming every skill, and it could carry no MCP server secrets, no marketplace
plugins such as context7, and no subagents outside a plugin. The owner wants one
text file that says what a member needs and a claudio that installs it.

## Goals

1. A proxy plugin is a manifest file; claudio fetches it per host (hash-checked,
   as the zip was), parses it, and applies it: marketplaces registered, plugins
   installed in the member's user scope, MCP servers and subagents given to every
   session, instructions in every session's context, env merged.
2. What is already there is left alone: an installed plugin, a marketplace
   already added, an MCP server or agent the member defined with the same name.
3. The extensions screen shows, per host, what the manifest did and what it
   skipped and why; turning the plugin off stops applying it to new sessions.
4. A manifest is untrusted input with caps; a bad one is refused with a reason
   and never half-applied into a session.

## Non-goals

- Uninstalling or updating plugins (Claude Code's own updater does updates; a
  member removes a plugin by hand and turns the manifest off to keep it away).
- Project or local scope installs; per-member secrets; writing MCP servers or
  agents into the member's `~/.claude.json` or `~/.claude/agents`.
- Changing the proxy.

## Current state

- `src/proto.rs:215 Msg::Bundle { name, url, sha256, installed_id }` and `:289
  Msg::Bundled { name, path, installed }` (unreleased); the daemon's
  `src/daemon/bundle.rs` validates the op, checks `installed_plugins.json`,
  keeps `<cache>/plugins/<name>.zip` + `.sha256`, downloads with caps, one
  download per name, replies from a spawned task through the client's queue
  (`src/daemon/server.rs`). `Daemon::claude_flags` (`src/daemon/mod.rs`) probes
  `claude --help` once for `--plugin-dir` and `command()`
  (`src/daemon/session.rs`) drops the pairs when absent.
- `src/tui/plugins.rs`: `BundleState { Pending, Ready(path), Installed,
  Failed(String), TooOld }` per `(host, plugin)`, `request_bundles` (re-asks
  `Failed`), `on_plugins` (401/403 → signed out), the Alt+i rows and details.
  `src/tui/sessions.rs plugin_bundles` turns `Ready` into `--plugin-dir <path>`
  and merges the plugin env (`proxy::env::merge`, proxy wins); `src/tui/mod.rs
  wait_bundles` defers a spawn up to 20 s.
- `src/proxy/plugins.rs`: `Plugin { name, version, bundle_url, bundle_sha256,
  installed_id, env }`, `admit` (names, https, hash, env key denylist, caps),
  `plugins.json` cache (0600, per profile).
- Session context: `src/claude/hooks.rs:66` adds `'<claudio>' __context file
  '<path>'` as a SessionStart hook when `SpawnSpec.extensions` has instructions;
  `src/daemon/session.rs write_context` writes `<runtime>/context/<id>.txt`
  from `spec.extensions.context()` (16 KiB cap in `hooks.rs:45`).
- Claude Code (2.1.x) surfaces: `claude plugin marketplace add <source> --scope
  user` (source: a GitHub `owner/repo`, a URL or a path), `claude plugin install
  <name>@<marketplace> --scope user` (a marketplace-declared install command
  needs `-y` or `--accept-command <sha256>`), `~/.claude/plugins/
  known_marketplaces.json` (keys: marketplace names), `installed_plugins.json`
  (`plugins` keyed by `<name>@<marketplace>`), `--mcp-config <file>` (JSON
  `{"mcpServers": {...}}`, `${VAR}` expanded from the environment),
  `--agents <json>` (object keyed by agent name: `description`, `prompt`,
  optional `model`, `tools`), `~/.claude.json` `mcpServers` (the member's own),
  `~/.claude/agents/<name>.md`.

## Design

### 1. The manifest

A TOML document, ≤ 256 KiB, served as the plugin's `bundle` on the proxy. Format:

```toml
format = 1                      # required; a higher number is refused
name = "vocdoni-internal"       # must equal the proxy's plugin name
version = "1.0.0"               # ≤ 64 printable ASCII, shown
description = "…"               # ≤ 200 chars, shown
instructions = """…"""          # ≤ 16 KiB; added to every session's context

[[marketplace]]                 # ≤ 16
name = "vocdoni"                # [a-z0-9-]{1,40}
source = "github:vocdoni/skills"   # github:<owner>/<repo> or https:// URL

[[plugin]]                      # ≤ 64
name = "agentic-dev"            # [a-z0-9-]{1,40}
marketplace = "vocdoni"         # a [[marketplace]] above, or a marketplace Claude Code has built in

[[mcp]]                         # ≤ 32
name = "vocdoni-kb"             # [a-z0-9-]{1,40}
type = "http"                   # http | sse | stdio
url = "https://kb-gw.vocdoni.net/mcp"          # https only (http/sse)
headers = { Authorization = "Bearer ${VOCDONI_KB_TOKEN}" }   # ≤ 16, values ≤ 4 KiB
secrets = ["VOCDONI_KB_TOKEN"]  # env names (admit's key rule) the server needs
auth = "oauth"                  # optional: claude runs the member's own OAuth; no secret
command = "npx"                 # stdio only; args = [...] ≤ 32; env = {...} ≤ 16

[[agent]]                       # ≤ 32
name = "reviewer"               # [a-z0-9-]{1,40}
description = "…"               # ≤ 500
model = "opus"                  # optional
tools = ["Read", "Grep"]        # optional, ≤ 32 names
prompt = """…"""                # ≤ 32 KiB; all agents together ≤ 64 KiB

[env]                           # non-secret env for sessions; same key rule and denylist as the proxy env
```

`${NAME}` in `headers`, `env` and `args` is left for Claude Code to expand; a
name used there must be listed in `secrets` or in the manifest's `[env]`.
Anything unknown (a key, a table) is refused: the format is versioned, not
extensible. Duplicate names within a section are refused.

### 2. Fetch and cache (daemon, as today)

`Msg::Bundle` and `Msg::Bundled` are renamed `Msg::Manifest` and `Msg::Applied`
(both unreleased):

```rust
Msg::Manifest { name: String, url: String, sha256: String, secrets: Vec<String> }
Msg::Applied  { name: String, result: Result<Applied, String> }   // serde: {"ok": {...}} | {"err": "…"}
pub struct Applied {
    pub version: String,
    pub mcp_config: Option<String>,   // path on this host, or None when no server applies
    pub agents: Option<String>,       // the --agents JSON (≤ 64 KiB), or None
    pub instructions: String,         // the manifest's text, sanitized
    pub env: Vec<(String, String)>,   // the manifest's [env]
    pub installed: Vec<String>,       // "<plugin>@<marketplace>" installed by this apply
    pub present: Vec<String>,         // already there: plugins, marketplaces, servers, agents (ids)
    pub skipped: Vec<(String, String)>, // (id, reason): "missing secret X", "yours", "install failed: …"
}
```

`secrets` are the names (never values) of the env the client holds for this
plugin, so the daemon knows which servers can work. The file is cached as
`<cache>/plugins/<name>.toml` + `.sha256` with the existing download code
(same caps, https, single flight, no partial file).

### 3. Apply (daemon, `src/daemon/manifest.rs`)

After the file is current, the daemon parses it (caps above; a refused manifest
is `Err("manifest: <reason>")` and nothing runs) and applies, one apply per
plugin name at a time, from the spawned task:

1. Marketplaces: for each not in `known_marketplaces.json`, run `claude plugin
   marketplace add <source> --scope user` (the daemon's claude, `Config::path`,
   own process group, 120 s, output capped 64 KiB, stdin null). Failure →
   `skipped: (marketplace, "add failed: <first line>")` and every plugin of it.
2. Plugins: for each `<name>@<marketplace>` not in `installed_plugins.json`,
   run `claude plugin install <name>@<marketplace> --scope user` (same limits,
   never `-y`: a marketplace-declared install command is not accepted by
   claudio; the install fails and the row says "needs manual install").
   Success → `installed`; failure → `skipped`. Installs run at most once per
   `(manifest sha, plugin)` per daemon life; a later `Manifest` request with
   the same sha re-reads the installed list only.
3. MCP servers: a server whose name is in the member's `~/.claude.json`
   `mcpServers` → `skipped: (name, "yours")`; one whose `secrets` are not all in
   the request's `secrets` → `skipped: (name, "missing secret X")`; the rest are
   written as `{"mcpServers": {...}}` to `<runtime>/plugins/<name>.mcp.json`
   (0600, `write_atomic`; it holds `${VAR}` placeholders, never values).
4. Agents: one whose name has `~/.claude/agents/<name>.md` → `skipped: (name,
   "yours")`; the rest become the `--agents` JSON.
5. `instructions` through `claude::latex::sanitize`; `[env]` through `admit`'s
   key rule.

The daemon reads `~/.claude.json` and the plugins files of the uid it runs as,
1 MiB cap, unparsable = empty. Reads of `known_marketplaces.json`,
`installed_plugins.json` and `~/.claude.json` never log their content.

### 4. Client

- `src/proxy/plugins.rs`: `Plugin` keeps its fields; `installed_id` is accepted
  and ignored. `admit` unchanged.
- `src/tui/plugins.rs`: `BundleState` becomes `ApplyState { Pending,
  Ready(Applied), Failed(String), TooOld }`. `request_bundles` →
  `request_manifests` (same cadence: connect, list change, `r`, retry of
  `Failed`), sending the plugin's env key names as `secrets`. The details pane
  shows per host: `ready · <n> installed, <m> present, <k> skipped` and one
  line per skipped id with its reason; `Failed` and `TooOld` as today.
- Spawn (`src/tui/sessions.rs`): for each enabled plugin whose host state is
  `Ready`: `--mcp-config <mcp_config>` when `Some`, `--agents <json>` when
  `Some`, the manifest `[env]` merged before the proxy env (proxy wins), and
  `instructions` appended to `SpawnSpec.extensions`' instructions (the existing
  context file; the 16 KiB cap applies to the total, a longer total is cut at
  the cap with a notice). The deferral (`wait_bundles`, 20 s) and the old-daemon
  fail-safe stay. `--plugin-dir` is no longer added; the `--help` probe and the
  drop stay for `--mcp-config`/`--agents` instead (an old claude without them
  starts without the plugin, with the state `claude too old`).
- `--plain` and `reset_session` go through the same path.
- A plugin toggled off: nothing applied to new sessions; already-installed
  plugins remain (the details say "installed plugins stay until you remove
  them").

### Alternatives considered

| Option | Why not |
|---|---|
| Keep the zip and merge every team plugin into one | Renames every skill; no marketplace plugins, no MCP secrets, no standalone agents. |
| Apply the manifest on the client and `claude mcp add` into `~/.claude.json` | Writes secrets into the member's config in plaintext; the daemon is the host that has claude. |
| `-y` on plugin install | Would run a marketplace-declared command without a person seeing it. |
| Keep `Bundle`/`Bundled` names | Unreleased; a rename keeps the wire honest. |

## Compatibility and rollout

- Wire: `Bundle`/`Bundled` were never released; `Manifest`/`Applied` replace
  them. An old daemon answers `Error "unsupported op"` → `TooOld`, as before.
- Proxy: unchanged; `bundle.url` points at a `.toml`; `installed_id` left empty.
- Cache: `<cache>/plugins/<name>.toml`; a stale `.zip` from a dev build is
  removed by the daemon at start.
- Rollout: register `vocdoni-internal` (manifest URL + sha + env) on the proxy,
  release claudio v0.7.0, members run `claudio proxy login --token` (or the
  browser flow).
- Rollback: the previous claudio ignores the manifest (no Bundle op in a
  release); plugins installed by claudio stay installed.

## Failure modes and security

- The manifest is admin-pinned by hash; what it installs runs as the member.
  Accepted, as a bundle's hooks were: the admin owns the content repo.
- No `-y`: no marketplace-declared command runs without a person.
- Secrets: values exist only in the proxy env on the client, in `plugins.json`
  (0600) and in `Spawn.env`; the mcp JSON on the host holds placeholders; the
  `--agents` JSON and `--mcp-config` path on argv carry no secret; logs name
  ids and reasons only.
- Installs are bounded (120 s, process group, capped output) and never block
  the actor or accept loop; a hung `claude plugin install` is killed as a group.
- A refused manifest applies nothing; a partially failed apply (one install
  failed) still yields a `Ready` with that id in `skipped`.
- `~/.claude.json` and the plugins files are read with size caps; a symlink or
  a non-regular file counts as empty.
- Per-host apply concurrency: one at a time per plugin name; two hosts are
  independent.

## Acceptance criteria

- [ ] AC1: unit tests for the manifest parser: every cap and charset, unknown keys refused, duplicate names refused, `format` other than 1 refused, placeholders must be declared.
- [ ] AC2: daemon tests with a fake `claude` that records its argv and writes fake `known_marketplaces.json`/`installed_plugins.json`: a marketplace is added once, an installed plugin is not reinstalled, an install failure lands in `skipped` with the reason, a server with a missing secret is skipped and absent from the mcp JSON, a server the member has is skipped "yours", the mcp JSON holds placeholders only, no `-y` on argv.
- [ ] AC3: manager test: with a fake proxy serving a manifest (one marketplace, one plugin, one http server with a secret, one agent, instructions), the spawned session's argv has `--mcp-config <runtime path>` and `--agents` with the agent, its env has the secret, the session context file has the instructions, and the fake claude's recorded install argv matches; toggled off, the next spawn has none; the Alt+i details list installed/present/skipped.
- [ ] AC4: `make test-unit`, `make test-manager` green; CI green.
- [ ] AC5: `tests/remote.rs` t9 reworked for `Manifest`/`Applied` on `z6` (release build).
- [ ] AC6: manual: with `vocdoni-internal` registered on claude.vocdoni.net, `claudio proxy login --token` with a member token, Alt+i shows the plugin ready, a new session has the vocdoni-kb server and the team skills.

## Constraints

- The spec's previous constraints hold: `app.rs` pure; no field added to an existing released op; secrets never in argv, journal, `state.json`, logs or notices; keys on `Alt`; `cli.rs` forwards claude's flags unchanged; conventional commits, no AI attribution; Linux and macOS.
- Never `-y`/`--accept-command` on `claude plugin install`.
- The daemon never writes into `~/.claude` itself; only `claude plugin …` does.

## Open questions

- None.
