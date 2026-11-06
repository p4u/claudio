# Plugin manifests: design

Status: approved (2026-11-06)   Class: architectural
Owner decisions so far (2026-11-06):
- A proxy plugin is a TOML manifest, not a zip: it declares marketplaces, plugins, MCP servers (with the secret each needs), subagents, instructions and env; claudio installs and configures all of it. Installing plugins into the member's `~/.claude` is wanted.
- A member who already holds a proxy user token needs no browser login; the token alone grants the manifest and its secrets. The browser login stays for members without one.
- The proxy side (merged, live on claude.vocdoni.net) stays as is: it serves the manifest's URL + SHA-256 and the secret env.
- The first manifest is `vocdoni-internal` in `vocdoni/skills`: every plugin of the vocdoni marketplace plus firecrawl, context7 and remember from the official one; the MCP servers vocdoni-kb, jina and hackmd; the reviewer, glm-worker and qwen-worker agents.
- Per-item opt-out: on a manifest's first apply the member is shown every item and chooses what to disable; the choice is editable later and persisted; new items of a newer version default on with a notice.

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
text file that says what a member needs, a claudio that installs it, and a member
who can leave parts of it out.

## Goals

1. A proxy plugin is a manifest file; claudio fetches it (hash-checked), parses
   it, shows its items, and applies the chosen ones per host: marketplaces
   registered, plugins installed in the member's user scope, MCP servers and
   subagents given to every session, instructions in every session's context,
   env merged.
2. What is already there is left alone: an installed plugin, a marketplace
   already added, an MCP server or agent the member defined with the same name.
3. The member chooses, item by item, what to apply: a setup screen on first
   sight of a manifest, the same screen later from the extensions screen; the
   choice is saved in `config.toml`.
4. The extensions screen shows, per host, what was installed, what was already
   there, what is still installing and what was skipped and why.
5. A manifest is untrusted input with caps; a bad one is refused with a reason
   and nothing of it reaches a session.

## Non-goals

- Uninstalling or updating plugins (Claude Code's updater does updates; a member
  removes a plugin with `claude plugin uninstall`; disabling an item only stops
  applying it).
- Project or local scope installs; per-member secrets; writing MCP servers or
  agents into the member's `~/.claude.json` or `~/.claude/agents`.
- Changing the proxy.

## Current state

- `src/proto.rs:215 Msg::Bundle { name, url, sha256, installed_id }` and `:289
  Msg::Bundled { name, path, installed }` are on `main` at `d143cdb` ("release
  v0.7.0", tagged locally, never pushed; no v0.7.0 release exists on GitHub);
  this branch forks from `ed65dec`, the commit before that release commit.
  `src/daemon/bundle.rs` validates the op, checks `installed_plugins.json` under
  `$CLAUDE_CONFIG_DIR` else `~/.claude` (`:56-60`), keeps `<cache>/plugins/
  <name>.zip` + `.sha256`, downloads with a 64 MiB cap (`:26`, `:235-247`), one
  download per name (`:36-40`, `:80-84`), replies from a spawned task
  (`src/daemon/server.rs`). `Daemon::claude_flags` (`src/daemon/mod.rs:458`)
  probes `claude --help` once; `command()` (`src/daemon/session.rs:330-367`)
  drops `--plugin-dir` pairs and appends only `-n <name>` after the args.
  `Daemon::updating` is the lock `claude update` takes.
- Journal: `src/daemon/journal.rs:40-42` stores `args` verbatim and re-spawns
  them at daemon start; `RespawnSpec` has no args (`src/tui/sessions.rs:588`).
- Session extensions: `SessionExtensions { lenses: Vec<LensSpec> }`
  (`src/proto.rs:435-447`), each lens a named tag/fence with an instruction;
  `MAX_LENSES 16`, `MAX_INSTRUCTION 2048`, `MAX_INSTRUCTIONS 8192` (`:463-467`),
  `validate` (`:470-493`) refused by the daemon on Spawn/Respawn
  (`src/daemon/mod.rs:513`, `:884`). `src/claude/hooks.rs:66` adds `'<claudio>'
  __context file '<path>'` as a SessionStart hook when there are instructions;
  `write_context` (`src/daemon/session.rs`) writes `spec.extensions.context()`;
  the hook reads at most 16 KiB (`hooks.rs:46`).
- `src/tui/plugins.rs`: `BundleState { Pending, Ready(path), Installed,
  Failed(String), TooOld }`, `request_bundles` (re-asks `Failed` only,
  `:166-171`), `on_plugins` (401/403 → signed out); `src/tui/sessions.rs
  plugin_bundles` → `--plugin-dir`, `proxy::env::merge` (`src/proxy/env.rs:112`,
  later key wins, as `apply_env` `src/daemon/session.rs:371-374`);
  `src/tui/mod.rs:1588 BUNDLE_WAIT = 20 s`, `wait_bundles` (`:1594-1640`).
- `src/proxy/plugins.rs`: `Plugin { name, version, bundle_url, bundle_sha256,
  installed_id, env }`, `admit`, private `key_ok` (`:168-185`; `admit` `:191-226`), `plugins.json`
  cache. `config::save_extension` (`src/config.rs:429-467`) is the minimal line
  edit of `[extensions]`.
- Freshness: `src/freshness.rs:76-85` orders daemons by SemVer, then mtime.
- Claude Code (2.1.x), verified: `claude plugin marketplace add <source> --scope
  user --json` (source `owner/repo`, URL or path); `claude plugin install
  <name>@<marketplace> --scope user --json` (a marketplace-declared install
  command needs `-y`/`--accept-command`, and `-y` is "required when stdin or
  stdout is not a TTY" for those; `--json` reports `shownCommand.sha256` then);
  `<claude_config>/plugins/known_marketplaces.json` keyed by the name the
  marketplace declares in its own `.claude-plugin/marketplace.json`;
  `installed_plugins.json` `{"version":2,"plugins":{"<name>@<marketplace>":
  [{scope,…}]}}`; `<claude_config>/.claude.json` has `mcpServers` (277 KiB on
  the owner's machine); `~/.claude/agents/<name>.md`; `--mcp-config
  <configs...>` is variadic and takes JSON files or strings, and an inline
  string has `${VAR}` expanded from the environment (verified); `--agents
  <json>` (object keyed by name: `description`, `prompt`, optional `model`,
  `tools`).

## Design

### 1. The manifest

A TOML document, ≤ 256 KiB, served as the plugin's `bundle` on the proxy.
Unknown keys or tables, a `format` other than 1, and duplicate names within a
section are refused. Names below are `[a-z0-9-]{1,40}`; "text" is UTF-8 with
no C0 control but `\n`/`\t`; header names are RFC 7230 token characters ≤ 64.

```toml
format = 1                      # required
name = "vocdoni-internal"       # must equal the proxy's plugin name
version = "1.0.0"               # ≤ 64 printable ASCII, shown (SemVer not required)
description = "…"               # text ≤ 200
instructions = """…"""          # text ≤ 8 KiB; added to every session's context

[[marketplace]]                 # ≤ 16
name = "vocdoni"                # must equal the name the marketplace declares in its own
                                # .claude-plugin/marketplace.json (the key Claude Code uses)
source = "github:vocdoni/skills"   # github:<owner>/<repo>, each [A-Za-z0-9_.-]{1,64}
                                # (passed to the CLI as <owner>/<repo>), or https://… ≤ 512
                                # printable ASCII (passed verbatim); nothing else

[[plugin]]                      # ≤ 64
name = "agentic-dev"
marketplace = "vocdoni"         # a [[marketplace]] of this manifest; there is no built-in set:
                                # the official one is declared like any other
                                # (name = "claude-plugins-official", source = "github:anthropics/claude-plugins-official")

[[mcp]]                         # ≤ 32; a name may not repeat an [[agent]] name
name = "vocdoni-kb"
type = "http"                   # http | sse | stdio
url = "https://kb-gw.vocdoni.net/mcp"   # http/sse: https only, ≤ 512 printable ASCII
headers = { Authorization = "Bearer ${VOCDONI_KB_TOKEN}" }   # ≤ 16, values printable ASCII ≤ 4 KiB
secrets = ["VOCDONI_KB_TOKEN"]  # env names (key rule below) the server needs; ≤ 16
auth = "oauth"                  # optional, http/sse: documentation only; rendered with type and url
                                # and no headers; secrets must be empty; the member signs in with /mcp
command = "npx"                 # stdio only, ≤ 256 printable ASCII; args = [...] ≤ 32 × 1 KiB text;
                                # env = {...} ≤ 16, key rule, values text ≤ 4 KiB

[[agent]]                       # ≤ 32
name = "reviewer"
description = "…"               # text ≤ 500
model = "opus"                  # optional, [A-Za-z0-9._-]{1,64}
tools = ["Read", "Grep"]        # optional, ≤ 32 × [A-Za-z0-9_:*()-]{1,64}
prompt = """…"""                # text ≤ 16 KiB; the rendered agents JSON ≤ 32 KiB

[env]                           # ≤ 64 keys; key rule; values text ≤ 4 KiB
```

Key rule: `plugins::key_ok` (made `pub`): `[A-Z_][A-Z0-9_]{0,63}` outside the
denylist (`ANTHROPIC_*`, `CLAUDIO_*`, `CLAUDE_*`, `NODE_*`, `LD_*`, `DYLD_*`,
`GIT_*`, `SSL_*`, `XDG_*`, `SSH_*`, `PATH`, `HOME`, `SHELL`, `TERM`, `USER`,
`LOGNAME`, `TMPDIR`, `BASH_ENV`, `ENV`, `PYTHONPATH`, `PERL5OPT`, the proxy
variables). `${NAME}` in `headers`, `env`, `args` and `url` is left for Claude
Code to expand; a name used there must be listed in that server's `secrets` or
in `[env]`. A placeholder is exactly `${NAME}` with `NAME` by the key rule; any
other `${` is refused. The renderers run on the whole manifest at parse time
(every item, no `disabled`): an agents JSON over 32 KiB or an mcp JSON over
64 KiB refuses the manifest, so the per-item caps are guidance and the rendered
caps are the rule.

Item ids, used everywhere a member chooses or a host reports:
`plugin:<name>@<marketplace>`, `mcp:<name>`, `agent:<name>`, `instructions`,
`env`. A marketplace has no id: it is added when at least one of its plugins is
enabled.

The parser is `src/plugins/manifest.rs` (pure; `Manifest::parse(&str) ->
Result<Manifest, String>`, reasons name ids and fields, never values), shared
by the client and the daemon.

### 2. Client: fetch, choose, request

- `src/proxy/plugins.rs`: `Plugin` keeps its fields; `installed_id` is accepted
  and ignored. `admit` unchanged.
- The client fetches each admitted plugin's manifest itself
  (`Effect::FetchManifest { name, url, sha256 }`, 256 KiB cap, https, hash
  checked, kept in memory only; refetched when the list's hash changes and on
  `r`). A refused or unreachable manifest is the row's state
  `manifest: <reason>`; nothing is requested of any host for it.
- Choices: `config.toml` gains `[plugins.<name>]` with `disabled = ["mcp:jina",
  …]` (item ids), `reviewed = "<version>"` and `seen = [<every id of the
  reviewed version>]`, written by `config::save_plugin_choices` (the same
  minimal table edit as `save_extension`: replace the table's lines, or insert
  the table at the end). `seen` keeps the ids of `disabled` that the manifest
  no longer has, so a removed item that comes back stays disabled.
- Setup screen (`src/tui/plugin_setup.rs`, a pure view like `confirm.rs`,
  drawn by `ui/plugin_setup.rs`): every item by section with its title (a
  plugin's `name@marketplace`, a server's `name` + type + url, an agent's
  `name` + description, "instructions", "env (<n> keys)"), each with a
  checkbox. Keys inside the modal: Up/Down (or j/k), Space toggles, `a` all
  on, `n` all off, Enter saves and closes, Esc as below. It opens when a manifest arrives and
  `config.toml` has no `[plugins.<name>]` table (first sight), and from the
  extensions screen with Enter on the plugin's row (Space still toggles the
  plugin; on every other row Space and Enter toggle as before; the key hint
  and the module doc say so). A manifest with ids not in `seen` gives a
  notice "`<name> <version>` adds <ids>; Alt+i, Enter to review"; new items
  default on. Saving sets `reviewed` and `seen` and sends a new `Manifest`
  request to every host. Esc from the extensions screen closes without
  saving. Esc on first sight defers: nothing is written, nothing is sent to
  any host for that manifest, the plugin's row says `setup pending (Enter)`,
  and the screen opens again at the next claudio start (not on every refetch
  within the run). Only Enter writes the table and starts the applies. The
  screen is a modal that swallows every key (not in `yields_to_actions`, like
  `Confirm`). Setup screens go through the confirm queue, one at a time, in
  arrival order, so two manifests, or a manifest and an update prompt, are
  shown one after the other; a manifest's apply starts only once its own
  screen has closed with Enter. A manifest that arrives for a plugin whose
  screen is open or queued replaces that screen's items, keeping the toggles
  of ids still present.
- Request: `Msg::Manifest { name, url, sha256, secrets: Vec<String>, disabled:
  Vec<String> }` where `secrets` are the names (never values) of the plugin's
  proxy env and `disabled` the saved ids. Sent to each connected host on
  connect, when the list's hash changes, when choices are saved, on `r`, and
  every 30 s while a reply still lists an `installing` id (10 min at most per
  manifest hash). Nothing is sent before the first-sight screen closes.

### 3. Daemon: fetch, apply, reply (`src/daemon/manifest.rs`)

The op is checked as `Bundle` was (name, 64-hex hash, https), the file cached
as `<cache>/plugins/<name>.toml` + `.sha256` by the existing download code with
the cap as a parameter (256 KiB here). `secrets`: ≤ 64 names, each by
`key_ok`; `disabled`: ≤ 256 ids, each an item id of §1's grammar (≤ 96
bytes); otherwise `Error "manifest <name>: bad request"`. A stale
`<name>.zip` from a dev build is removed at daemon start.

Reply, sent as soon as the manifest is parsed and the static parts rendered,
before any subprocess runs:

```rust
Msg::Applied {
    name: String, version: String,
    mcp_config: Option<String>,   // the {"mcpServers": {...}} JSON string (≤ 64 KiB), placeholders only
    agents: Option<String>,       // the --agents JSON (≤ 32 KiB)
    instructions: String,         // sanitized, ≤ 8 KiB
    env: Vec<(String, String)>,   // the manifest's [env]
    installed: Vec<String>,       // ids installed by this daemon (this hash)
    present: Vec<String>,         // ids already there: plugins, servers, agents
    skipped: Vec<(String, String)>, // (id, reason)
    dropped: Vec<String>,         // "--mcp-config" / "--agents" when this host's claude lacks them (the cached probe)
}                                 // every list #[serde(default)]
```

A refused manifest or a failed download is `Msg::Error { message: "manifest
<name>: <reason>" }`, which the client maps to `Failed` as today.

Per item, honouring `disabled` (a disabled id is neither applied nor listed):

1. Plugins, decided in this order per id: (a) the id is a key of
   `installed_plugins.json` and this daemon installed it under this manifest
   hash → `installed`; (b) a key of the file otherwise → `present` (any
   scope: a second install beside a project-scope one is Claude Code's to
   refuse; the member sees `present` and the CLI command in the details);
   (c) remembered `needs manual install` (daemon's life) or a failure less
   than 10 minutes old → `skipped: (id, <that reason>)`; (d) else `skipped:
   (id, "installing")` and queued, unless already queued or running. Before
   an install, if its marketplace `name` is not a key of
   `known_marketplaces.json`, the marketplace is added first.
2. MCP servers: a name present in `<claude_config>/.claude.json` `mcpServers`
   → `skipped: (id, "yours")`; `secrets` not all in the request's `secrets` →
   `skipped: (id, "missing secret X")`; else rendered into `mcp_config` as
   Claude Code expects: `{"type":"http","url":…,"headers":{…}}`,
   `{"type":"sse",…}`, `{"type":"stdio","command":…,"args":[…],"env":{…}}`.
3. Agents: `<claude_config>/agents/<name>.md` exists → `skipped: (id,
   "yours")`; else `{"<name>": {"description", "prompt", "model"?, "tools"?}}`.
4. `instructions` through `claude::latex::sanitize`; `[env]` through
   `key_ok` and the value rules.

Files read: `<claude_config>/.claude.json`, `plugins/known_marketplaces.json`,
`plugins/installed_plugins.json`, `agents/` (`$CLAUDE_CONFIG_DIR` else
`~/.claude`), each a regular file of this uid, cap 16 MiB, content never
logged. An unreadable or unparsable `.claude.json` skips every server with
`(id, "cannot read .claude.json")`; an unreadable plugins file counts as "not
installed"/"not added" (the CLI then says so).

Installs run after the reply, in one spawned task per manifest hash. Each
`claude plugin …` subprocess (not the series) runs while holding
`Daemon::updating` (`tokio::sync::Mutex<()>`, taken with `lock().await`), so
two manifests, or a manifest and `claude update`, never write `~/.claude` at
once; `claude update` keeps its `try_lock` and its refusal reads "a claude
update or plugin install is already running on this host". Each subprocess: `claude plugin
marketplace add <source> --scope user --json` or `claude plugin install
<name>@<marketplace> --scope user --json`, the daemon's claude on
`Config::path`, stdin `/dev/null`, stdout and stderr piped and capped at 64
KiB each, env: the daemon's with the session markers scrubbed,
`GIT_TERMINAL_PROMPT=0`, `GIT_ASKPASS=/bin/true`; own process group, 120 s,
killed as a group on timeout. The outcome is the last stdout line parsed as
JSON: a `shownCommand` → `needs manual install: claude plugin install <id>`;
a non-zero exit or an `error` field → that message (first 200 chars); after a
marketplace add the daemon re-reads `known_marketplaces.json` and, when `name`
is still absent, the marketplace and its plugins are `skipped: (…, "added
under another name")`. Never `-y`, never `--accept-command`.

### 4. Spawn

`src/tui/sessions.rs` (`send_spawn`, `reset_session`, `--plain`): for each
enabled plugin whose state on the host is `Ready`:

- args: appended after every other argument, per plugin `--agents <json>`
  then `--mcp-config <json>` (so the variadic `--mcp-config` is followed by
  another flag, the next plugin's `--agents`, or the daemon's `-n <name>`,
  never by a positional); a unit test on `command()` asserts the order. At
  most 8 plugins' flags go on one spawn (≤ 768 KiB of args); further `Ready`
  plugins are left out with the notice `plugin <name> left out: too many
  plugins`.
- env: layers, a later layer replacing an earlier one (`env::merge` twice):
  `[env]` of every `Ready` plugin, then every plugin's proxy env (secrets),
  then the profile env. Within a layer the registry order holds and the first
  plugin's key wins (as `plugin_env` does today).
- instructions: `SessionExtensions.instructions: String` (new,
  `#[serde(default)]`, counted apart from the lenses: ≤ 8 KiB after
  `sanitize`, checked in `validate`, appended by `context()` after the lenses
  with a blank line, journaled with the rest) holds the concatenation of the
  `Applied.instructions` of every enabled plugin whose state is `Ready` on
  that host, blank line between; over 8 KiB it is cut at the last whole
  instruction that fits, with a notice. Lenses (≤ 8 KiB) and instructions
  (≤ 8 KiB) always fit the hook's 16 KiB read.
- The daemon's `command()` drops an `--agents <v>` or `--mcp-config <v>` pair
  when its `claude --help` lacks the flag (the one cached probe, like
  `--plugin-dir`, which is no longer added); it removes exactly one value. A
  journal re-spawn keeps the flags (no secret in them); `reset_session` and
  Alt+e keep the journaled flags and send the current env and the current
  `SessionExtensions` (lenses and instructions), which the journal takes over.
- Deferral: `wait_bundles` waits (20 s) only for a host that has not answered
  `Manifest` at all; an `Applied` with `installing` ids is `Ready` (sessions
  start with the servers, agents and instructions while plugins install).
- `--plain` with a proxy profile that has admitted plugins without a
  `[plugins.<name>]` table defers `start_plain` until every such manifest has
  arrived or failed, at most `BUNDLE_WAIT` (20 s), shows the setup screens
  (queued, as above), and spawns when the last closes; Esc there defers like
  the manager (the session starts without that plugin). Without a proxy
  profile, with every table present, or with no plugin list at all yet (no
  cache, nothing fetched), it spawns at once as today.

### 5. Extensions screen

Each admitted plugin stays a `Source::Plugin` row; its details show the
version and description, one line per host (`<host>: ready · <i> installed,
<p> present, <s> installing, <k> skipped`, then one line per skipped id with
its reason; `failed: …`; `daemon too old`; `claude too old: <flags>` from `Applied.dropped`, the
state staying `Ready` since the env and the instructions still apply), the
manifest state (`manifest: …`),
`<n> items disabled (Enter to change)`, and `installed plugins stay until you
remove them: claude plugin uninstall <id>`. Enter opens the setup screen.
Turning the plugin off: nothing applied to new sessions; a running install
is not stopped and nothing is uninstalled; the last reply is kept so turning
it on again needs no new apply.

### Alternatives considered

| Option | Why not |
|---|---|
| Keep the zip and merge every team plugin into one | Renames every skill; no marketplace plugins, no MCP secrets, no standalone agents. |
| Apply the manifest on the client and `claude mcp add` into `~/.claude.json` | Writes secrets into the member's config in plaintext; the daemon is the host that has claude. |
| `-y` on plugin install | Would run a marketplace-declared command without a person seeing it. |
| An mcp JSON file under the runtime dir | Dies at reboot while the journal keeps its path; the inline string holds only placeholders and works on every host. |
| The daemon parses the manifest and the client shows the setup from the reply | The member would choose after the first apply; the client must know the items before anything is installed. |

## Compatibility and rollout

- Wire: `Bundle`/`Bundled` exist only on the unpushed local `main`;
  `Manifest`/`Applied` replace them. An old daemon answers `Error "unsupported
  op"` → `TooOld`. `SessionExtensions.instructions` is `#[serde(default)]`: an
  old daemon ignores it (no instructions, nothing worse), an old client sends
  none.
- Base and release: the branch forks from `ed65dec`; the local release commit
  `d143cdb` and tag `v0.7.0` are dropped (owner OK), and v0.7.0 is cut from
  this work. No host ran a `0.7.0` build (`z6` ran a `0.6.1` dev build of
  `ed65dec`); `freshness::compare` replaces it by mtime.
- Proxy: unchanged; `bundle.url` points at a `.toml`; `installed_id` empty.
- Config: `[plugins.<name>]` is new; an older claudio ignores it.
- Rollout: register `vocdoni-internal` (manifest URL pinned to a commit of
  `vocdoni/skills`, its sha, the env) on the proxy; release claudio v0.7.0;
  members run `claudio proxy login --token` (or the browser flow) and review
  the setup screen once.
- Rollback: the previous claudio ignores everything new; plugins installed by
  claudio stay installed.

## Failure modes and security

- The manifest is admin-pinned by hash; what it installs and the stdio
  commands it declares run as the member. Accepted: the admin owns the content
  repo, and the member sees every item before the first apply.
- No `-y`: no marketplace-declared command runs without a person.
- Secrets: values exist only in the proxy env on the client, in `plugins.json`
  (0600) and in `Spawn.env`; the mcp JSON on argv holds placeholders; agent
  prompts are team content, not secrets: they are visible in `ps` and stored
  in the journal with the session's args (rendered JSON ≤ 32 KiB).
- Installs are bounded (120 s, process group, capped output, one at a time
  per daemon) and never block the actor or accept loop; a claude session
  running during an install may or may not see the plugin before its next
  start (accepted).
- A refused manifest applies nothing; a partial apply reports each id's
  outcome; a failed install is retried after 10 minutes, a manual one never
  without a person.
- Config files are read with size caps; a symlink or a non-regular file counts
  as unreadable (servers skipped with a reason, plugins treated as absent).
- Two hosts are independent; on one host, downloads are one per name and
  `~/.claude` writes one at a time.

## Acceptance criteria

- [ ] AC1: unit tests for `Manifest::parse`: every cap and charset, unknown keys, `format` ≠ 1, duplicates, undeclared placeholders, `auth = "oauth"` with headers or secrets refused, source forms; and for the renderers (mcp JSON per type, agents JSON, ids).
- [ ] AC2: daemon tests with a fake `claude` that records argv and answers `--json` (success, error, `shownCommand`) and fake `known_marketplaces.json`/`installed_plugins.json`/`.claude.json`/`agents/`: a marketplace is added once, an installed plugin is `present`, a failing install is `skipped` with the reason and retried after the 10 min window (clock injected), `shownCommand` is `needs manual install` for the daemon's life, a server with a missing secret and one the member has are skipped with their reasons, the reply comes before the install runs (`installing`), a disabled id is neither applied nor listed, no `-y` on argv, the install lock is shared with `claude update` (an `UpdateClaude` during an install is refused with that message; one between two installs runs), an unreadable `.claude.json` skips servers with the reason.
- [ ] AC3: unit tests for the setup view (keys, first sight, new-items notice), `save_plugin_choices` (table replaced or inserted, other lines untouched), `SessionExtensions` validation and `context()` with instructions, `command()` argv order and the pair drops.
- [ ] AC4: manager test: a fake proxy serving a manifest (one marketplace, one plugin, one http server with a secret, one agent, instructions): the setup screen appears first; after Enter the spawned session's argv has `--agents` and `--mcp-config` (placeholders only, env has the secret), the context file has the instructions, the fake claude recorded the install; a server disabled in the setup screen is absent; toggled off, the next spawn has none; the details list installed/present/skipped.
- [ ] AC5: `make test-unit`, `make test-manager` green; CI green.
- [ ] AC6: `tests/remote.rs` t9 reworked for `Manifest`/`Applied` on `z6` (release build).
- [ ] AC7: manual: with `vocdoni-internal` registered on claude.vocdoni.net, `claudio proxy login --token` with a member token, the setup screen lists its 13 plugins, 3 servers and 3 agents, a new session has the vocdoni-kb server, the team skills and the reviewer agent.

## Constraints

- `app.rs` pure; no field added to a released op (`SessionExtensions.instructions` is `#[serde(default)]` and ignorable); secrets never in argv, journal, `state.json`, logs or notices; keys on `Alt` outside modals; `cli.rs` forwards claude's flags unchanged; conventional commits, no AI attribution; Linux and macOS.
- Never `-y`/`--accept-command` on `claude plugin install`; the daemon never writes into `~/.claude` itself.
- One parser (`src/plugins/manifest.rs`) for client and daemon; reasons name ids and fields, never values.

## Open questions

- None.
