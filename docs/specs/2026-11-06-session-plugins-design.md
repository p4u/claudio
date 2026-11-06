# Session plugins and browser login: design

Status: draft   Class: architectural
Owner decisions so far (2026-11-06):
- Plugin content lives in a git repo, released as a zip; the proxy serves the pinned bundle reference and the secret env (proxy spec: `claude-proxy/docs/specs/2026-11-06-claudio-plugins-design.md`).
- `claudio proxy login` uses a device-style flow (one-time code, browser sign-in at the proxy, polling).
- Fetched plugin env is cached on disk, mode 0600, so sessions get it when the proxy is unreachable at start.
- The first plugin is `vocdoni-internal`; a `vocdoni-public` may follow.

This is sub-project 2 of 2 and depends on the proxy API from sub-project 1.

## Problem

A new member needs an admin-issued proxy token, then installs the team's
skills, plugins and MCP servers by hand and collects each server's key
separately. claudio already injects a proxy profile's env into every claude
session (`src/proxy/env.rs:53 session_env`, sent as `SpawnSpec.env`,
`src/proto.rs:365`) and already passes per-session settings on argv
(`src/daemon/session.rs:319 command()`), but knows nothing about plugins.
Claude Code can load a plugin for one session only, uninstalled, with
`--plugin-dir <dir|zip>` (claude `--help`), and its MCP configs expand
`${VAR}` from the environment.

## Goals

1. `claudio proxy login` signs a member in through the browser and stores
   their per-user token; the token-paste path stays as a fallback.
2. Every claude session claudio opens (manager, `--plain`, remote over SSH)
   loads each enabled proxy plugin's bundle and gets its env, unless the
   member already installed that plugin in Claude Code.
3. Plugins appear in the extensions screen (`Alt+i`) with their version and
   state, and can be turned off there, saved to `config.toml`.
4. A bundle is fetched once per version per host, hash-checked, and reused.

## Non-goals

- Building bundles, or any plugin content. The content repo's CI does that.
- Installing anything into `~/.claude`.
- Per-member secrets.
- Plugins for sessions not started by claudio (passthrough `claudio <args>`
  execs claude unchanged and stays so).

## Current state

- Login: `src/proxy/cmd.rs:58 cmd_login` reads a token from the tty or
  stdin (`:202`, `:277 read_token_no_echo`), verifies it against
  `GET /v1/claudio` (`src/proxy/api.rs:286 check_root`) and stores a
  `Profile{url, token, first_party}` (`src/proxy/profile.rs:30`) in the
  `[proxy]` section of `~/.config/claudio/config.toml`, mode 0600 (`:145
  save_to`, `paths::write_atomic`). `CLAUDIO_PROXY_URL=<token>@host` gives
  an ephemeral profile (`:228 from_env`).
- Spawn: `src/tui/sessions.rs:401 send_spawn` builds `SpawnSpec{args, env:
  vec![], extensions}` and `send_with_proxy` (`:452`) fills `env` from
  `proxy_env_for` (cached `/v1/claudio/config`, or `Effect::SpawnWithProxy`
  when cold). The respawn path (`:549-570`) does the same. The daemon's
  `command()` (`src/daemon/session.rs:319`) adds `--settings <hooks>`,
  `--allow-dangerously-skip-permissions` only when this host's `claude
  --help` lists it (`:331-338`), then `spec.args` verbatim. `apply_env`
  (`:356`) applies `env_diff` (`src/proxy/env.rs:135`): session markers
  removed, `ANTHROPIC_API_KEY` dropped when a token is set.
- The journal (`src/daemon/journal.rs`) stores args, never env; a session
  re-spawned at daemon start gets its args and no proxy env.
- Extensions screen: `src/tui/extensions_view.rs` lists the registry's
  entries; a toggle emits `Effect::SetExtension` and `Effect::SaveExtension`
  (`config::save_extension`, a line edit of `[extensions]`).
- Protocol: `src/proto.rs` `Msg` ops; an old daemon answers an unknown op
  with `Error "unsupported op…"`, which `App::older_daemon_notice` turns
  into "run `claudio daemon restart`".
- HTTP: `reqwest` is already a dependency (`src/proxy/api.rs:279
  build_client`).

## Design

### 1. Browser login

`claudio proxy login [--name N] [--token]`. Without `--token` and when
`POST <url>/v1/claudio/login/start` answers 200, the device flow runs:

```
$ claudio proxy login
Open this page and approve the sign-in:
  https://claude.vocdoni.net/claudio/login?code=K7PQ-3WXM
Your code: K7PQ-3WXM
Waiting… (Ctrl+C to cancel)
Signed in as pau@vocdoni.org. Profile "vocdoni" saved as default.
```

- The URL is opened with `xdg-open` / `open` when `$DISPLAY`/`$WAYLAND_DISPLAY`
  (Linux) or macOS, stdout is a tty, and `CLAUDIO_NO_BROWSER` is unset; it
  is always printed.
- Polling: `POST /v1/claudio/login/poll {device_code}` every `interval_s`
  (3 s; a 429 `slow_down` doubles it) until 200, or until `expires_in_s`
  (then "login expired, run again"). 403 → "sign-in denied". 404 on
  `login/start` → "this proxy has no browser login; paste a token with
  --token" and the paste flow runs.
- On 200 the token is verified as today (`check_root`) and the profile is
  stored with a new optional field `email` (shown by `claudio proxy
  status`). The profile name defaults to the host's first label
  (`claude.vocdoni.net` → `claude`) as today.
- `--token` forces the paste flow unchanged.

### 2. Plugin list and cache

New module `src/proxy/plugins.rs`:

```rust
pub struct Plugin { pub name: String, pub version: String,
    pub bundle_url: String, pub bundle_sha256: String,
    pub installed_id: String, pub env: Vec<(String, String)> }
pub struct PluginList { pub profile: String, pub fetched_at: u64,
    pub plugins: Vec<Plugin> }
pub fn fetch(profile: &Profile) -> Result<PluginList, ApiError>   // GET /v1/claudio/plugins
pub fn load_cache() -> Option<PluginList>                          // ~/.config/claudio/plugins.json
pub fn save_cache(&PluginList)                                     // write_atomic, 0600
pub fn admit(p: &Plugin) -> Result<(), String>                     // validation below
```

- Fetched by `tui/mod.rs` on start for the default profile (an `Effect`,
  never in `App`), after every successful login, every 15 min, and on
  `Alt+i` → `r`. A 404 or an error keeps the cache (if its `profile`
  matches) and sets the plugin state to "proxy unreachable, using cached
  list from <age>". A 404 from an old proxy with no cache means no plugins.
- `admit` rejects: a name not `[a-z0-9-]{1,40}`, a non-`https` URL, a hash
  that is not 64 hex, an env key not `[A-Z_][A-Z0-9_]{0,63}` or starting
  with `ANTHROPIC_`, `CLAUDIO_`, or equal to `PATH`, `HOME`, `SHELL`,
  `TERM`, `CLAUDE_CONFIG_DIR`; a value over 4 KiB or with control
  characters; more than 64 keys; more than 32 plugins. A rejected plugin is
  listed as "refused: <reason>" and never used.
- `plugins.json` holds secrets: created and rewritten 0600 through
  `write_atomic` inside the 0700 config dir; `claudio proxy logout` deletes
  it. `CLAUDE.md`'s rule becomes: the journal and `state.json` never hold
  secrets; `config.toml` and `plugins.json` (0600) do.

### 3. Extensions screen

Each admitted plugin is a registry entry of a new kind `Plugin`
(`src/tui/extensions/mod.rs`): name, description `"<version> · <profile>"`,
no lenses or actions, enabled by default, toggle saved as
`[extensions] <name> = false` through the existing `Effect::SaveExtension`.
The details pane shows the state: `ready`, `downloading`, `installed in
Claude Code (bundle skipped)`, `refused: …`, `download failed: …`, `proxy
unreachable, cached <age>`, `daemon too old`. Names clash with built-ins
and externals under the existing rules (the registry refuses a duplicate).

### 4. Bundle on each host

New op and event in `src/proto.rs`:

```rust
Msg::Bundle { name: String, url: String, sha256: String, installed_id: String }
Reply::Bundle { name: String, path: Option<String>, installed: bool }
```

The daemon (`src/daemon/bundle.rs`), from a spawned task:

1. If `installed_id` is non-empty and this host's
   `~/.claude/plugins/installed_plugins.json` (version 2: `plugins` keyed
   by `<plugin>@<marketplace>`) has that key, reply `installed: true, path:
   None` (no download).
2. Else, if `<cache>/plugins/<name>/<sha256>.zip` exists, reply with it.
3. Else download `url` with `reqwest` (30 s connect, 120 s total, 64 MiB
   cap, no redirects off `https`), stream to a temp file, verify SHA-256,
   rename into place (`paths::write_atomic` semantics), delete other
   versions of `<name>`, reply with the path. `<cache>` is
   `$XDG_CACHE_HOME/claudio` or `~/.cache/claudio`, dir 0700.
4. Any failure replies `Error "bundle <name>: <reason>"`.

One download per `(name, sha256)` at a time; a second request joins it.
The TUI sends `Bundle` for every enabled plugin to each connected host on
connect and whenever the plugin list changes, and remembers the reply per
host. An old daemon's `Error "unsupported op"` sets the state `daemon too
old` and the session is spawned without the bundle (env still applies).

### 5. Spawn

`send_spawn` and the respawn path, for claude sessions only:

- `args` gets `--plugin-dir <path>` per enabled plugin whose host reply had
  a path. The daemon drops `--plugin-dir` pairs from `spec.args` when its
  `claude --help` does not list `--plugin-dir` (same probe as
  `ALLOW_SKIP_PERMISSIONS`), so an old claude still starts.
- `env` gets the plugin env after the proxy env; on a duplicate key the
  proxy env wins. Over SSH the env travels in `Spawn.env` as today; the
  path is the remote host's.
- `--plain` uses the same path (`src/tui/plain.rs`). A journal re-spawn at
  daemon start keeps the `--plugin-dir` args (not secret) and, as today,
  no env; `Alt+e` restores both.

### Alternatives considered

| Option | Why not |
|---|---|
| `--plugin-url` (claude fetches the zip itself) | No hash check, a download per session, and the remote host's claude would need the URL to be reachable without claudio knowing whether it was. |
| A `plugins` field on `SpawnSpec` | An old daemon would ignore it and start a session silently without the bundle; a separate op fails safe (CLAUDE.md). |
| Writing MCP servers into `~/.claude.json` | Installs state into the member's own config; conflicts with their setup. |
| Memory-only env | Owner chose the disk cache for offline starts. |

## Compatibility and rollout

- Protocol: one new op and reply, backward compatible (`PROTO` unchanged;
  unknown op → `Error`, unknown reply variant → `Unknown`).
- Config: `[proxy.profiles.<n>] email` optional; `[extensions] <plugin> =
  false` uses the existing section. `plugins.json` is new; absent means no
  cache.
- Proxy: requires sub-project 1 for login and plugins; against an older
  proxy both degrade to today's behaviour with a notice.
- Rollout: proxy deployed and `vocdoni-internal` registered first; then a
  claudio release; members run `claudio proxy login` once.
- Rollback: the previous claudio ignores `plugins.json` and the `email`
  field; the daemon's cache dir can be deleted.

## Failure modes and security

- Bundle zips are untrusted input to Claude Code, not to claudio: claudio
  never unpacks them; the hash pins the exact file the admin registered.
- Secrets: plugin env goes only through `Spawn.env` over the uid-checked
  socket (and the SSH bridge, as proxy tokens do today), never argv, journal
  or `state.json`; `Debug` for specs already redacts env values. Logs name
  keys only.
- A plugin cannot override the proxy's env or `ANTHROPIC_*` / `CLAUDIO_*`
  (admitted keys), so it cannot redirect model traffic or claudio's own
  hooks.
- Download limits: 64 MiB, 120 s, https only, one in flight per bundle;
  cache pruned to the current version; a failed download leaves no partial
  file.
- The daemon never blocks its actor or accept loop on a download (spawned
  task; reply through the client's queue, as other slow requests).
- Login: the device code lives only in the login process's memory; Ctrl+C
  abandons it. The token is stored only in `config.toml` (0600).
- A member who installed the plugin in Claude Code gets no duplicate
  skills (`installed: true` skips the bundle; env still applies).

## Acceptance criteria

- [ ] AC1: unit tests for `plugins::admit` (every rejection), cache load/save (mode 0600, profile mismatch ignored), and the login state machine against a fake HTTP server (pending → 200; slow_down; expired; denied; 404 → paste fallback).
- [ ] AC2: daemon test (`src/daemon/tests.rs`): `Bundle` against a local https-less fake is rejected; against a fake installed list replies `installed`; a cached zip is reused; a wrong hash is an `Error` and leaves no file.
- [ ] AC3: manager test (`tests/manager.rs`, fake claude): a spawned session's argv has `--plugin-dir <cache path>` and its env has the plugin key; a plugin toggled off in `Alt+i` is absent from the next spawn and `config.toml` has `<name> = false`; a fake claude whose `--help` lacks `--plugin-dir` starts without it.
- [ ] AC4: `make test-unit` and `make test-manager` green; CI (Linux + macOS) green.
- [ ] AC5: manual, with the dev proxy: `claudio proxy login` through the browser, `Alt+i` shows `vocdoni-internal` ready, a new session lists the bundle's skills, and the same on `z6` over SSH.

## Constraints

- `app.rs` stays pure: every fetch, download request and file write is an `Effect` driven by `tui/mod.rs`.
- Never add a field to an existing op; new behaviour is a new op that fails safe on old daemons.
- Secrets never in argv, journal, `state.json`, logs or notices.
- Keys stay on `Alt`; the `Alt+i` screen gains no new bindings except `r` (refresh), checked for collisions.
- `cli.rs` forwards claude's flags unchanged; `--plugin-dir` is added by claudio's spawn path only, never parsed from the user's args.
- Conventional commits, no AI attribution or co-author trailers. Keep Linux and macOS compiling.

## Open questions

- None.
