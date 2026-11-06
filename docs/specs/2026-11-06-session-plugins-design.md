# Session plugins and browser login: design

Status: approved (2026-11-06)   Class: architectural
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
2. Every claude session claudio opens with a proxy profile (manager,
   `--plain`, remote over SSH) loads each enabled plugin's bundle of that
   profile and gets its env, unless the member already installed that
   plugin in Claude Code.
3. Plugins appear in the extensions screen (`Alt+i`) with their version and
   state, and can be turned off there, saved to `config.toml`.
4. A bundle is fetched once per version per host, hash-checked, and reused.

## Non-goals

- Building bundles, or any plugin content. The content repo's CI does that.
- Installing anything into `~/.claude`.
- Per-member secrets.
- Plugins for sessions not started by claudio (passthrough `claudio <args>`
  execs claude unchanged and stays so), or for sessions with no proxy
  profile (`ProxyChoice::Direct`).

## Current state

- Login: `src/proxy/cmd.rs:58 cmd_login` takes a positional URL and
  `--name`, `--unverified` (`:66-74`), reads a token from the tty or stdin
  (`:202`, `:277 read_token_no_echo`), verifies it against `GET /v1/claudio`
  (`src/proxy/api.rs:286 check_root`, async) and stores a `Profile{url,
  token, first_party}` (`src/proxy/profile.rs:31`, no `deny_unknown_fields`)
  in the `[proxy]` section of `~/.config/claudio/config.toml`, mode 0600
  (`:145 save_to`, `paths::write_atomic`; the config dir itself is created
  with default mode, `src/paths.rs:129-131`). `CLAUDIO_PROXY_URL=<token>@host`
  gives an ephemeral profile (`:228 from_env`).
- Spawn: `src/tui/sessions.rs:401 send_spawn(proxy: Option<&str>, …)`
  builds `SpawnSpec{args, env: vec![], extensions}`; `send_with_proxy`
  (`:452`) fills `env` from `proxy_env_for` when the `/v1/claudio/config`
  cache is warm, else emits `Effect::SpawnWithProxy` and `tui/mod.rs`
  (`:1475-1501`) fetches with a 5 s timeout and calls `msg.with_env(env)`
  (`src/proto.rs:535`). The respawn path (`reset_session`, `:543-581`) does
  the same; `RespawnSpec` has no `args` (`proto.rs:390-407`): cwd, name and
  claude arguments come from the daemon's journal. The daemon's `command()`
  (`src/daemon/session.rs:319`) adds `--settings <hooks>`,
  `--allow-dangerously-skip-permissions` only when this host's `claude
  --help` lists it (`:331-338`, cached as `skip_permissions_supported`,
  `src/daemon/mod.rs:443-460`), then `spec.args` verbatim. `apply_env`
  (`:356-371`) applies `env_diff` (`src/proxy/env.rs:135`) and sets keys in
  order, so a later duplicate wins.
- The journal (`src/daemon/journal.rs:4-6`, `:42`) stores args verbatim,
  never env; a session re-spawned at daemon start gets its args and no env.
- Extensions: `Registry::new` runs once in `App::new` (`src/tui/app.rs:432`)
  from the built-ins and the externals `manifest::admit` de-duplicated
  (`src/tui/extensions/manifest.rs:334-377`); it has no insert or remove.
  `[extensions]` names it does not know only produce a notice
  (`src/tui/extensions/mod.rs:188-194`). The screen
  (`src/tui/extensions_view.rs`) uses plain keys (`:49-58`); a toggle emits
  `Effect::SetExtension` and `Effect::SaveExtension` (`config::save_extension`,
  a line edit of `[extensions]`, `src/config.rs:429-467`).
- Protocol: replies are `Msg` variants of one tagged enum with
  `#[serde(other)] Unknown` (`src/proto.rs:298`); an old daemon answers an
  unknown op with `Error "unsupported op…"` (`src/daemon/server.rs:562`),
  which `App::older_daemon_notice` (`src/tui/app.rs:812-815`) turns into
  "run `claudio daemon restart`". Slow requests reply from spawned tasks
  through the client's queue (`server.rs:496-513`).
- `reqwest` (rustls) and `sha2` are dependencies (`Cargo.toml:52-53`).
- Test fakes: every fake claude in `tests/manager.rs` answers `--help` with
  nothing (`:1744`, `:2155`); the argv-dumping fake filters only
  `--settings` and the skip flag (`:1747`).

## Design

### 1. Browser login

`claudio proxy login [URL] [--name N] [--unverified] [--token]`. `URL` is
required when no profile exists, else it defaults to the default profile's
URL. Without `--token`, when `POST <url>/v1/claudio/login/start` answers
200, the device flow runs:

```
$ claudio proxy login
Open this page, sign in, and type your code:
  https://claude.vocdoni.net/claudio/login
Your code: K7PQ-3WXM
Waiting… (Ctrl+C to cancel)
Signed in as pau@vocdoni.org. Profile "claude" saved as default.
```

- The URL is opened with `xdg-open` / `open` when `$DISPLAY` or
  `$WAYLAND_DISPLAY` is set (Linux) or on macOS, stdout is a tty, and
  `CLAUDIO_NO_BROWSER` is unset; it is always printed.
- Polling: `POST /v1/claudio/login/poll {device_code}` every `interval_s`
  (3 s; a 429 `slow_down` doubles it) until 200, or until `expires_in_s`
  (then "login expired, run again"; a 404 `unknown_code` or a 410 on
  `poll` ends the same way). 403 → "sign-in denied". 429 `too_many_pending`
  on `login/start` → "too many pending logins from this address, try again
  in a minute". 404 on `login/start` → "this proxy has no browser login;
  paste a token" and the paste flow runs. The URL carries no code: the
  member types it on the page (the proxy spec's phishing mitigation).
- On 200 the token is verified as today (`check_root`, unless
  `--unverified`) and the profile is stored with a new optional field
  `email` (shown by `claudio proxy status`). The profile name defaults as
  today.
- `--token` forces the paste flow unchanged.
- The device code lives only in the login process's memory; Ctrl+C
  abandons it.

### 2. Plugin list and cache

New module `src/proxy/plugins.rs`:

```rust
pub struct Plugin { pub name: String, pub version: String,
    pub bundle_url: String, pub bundle_sha256: String,
    pub installed_id: String, pub env: Vec<(String, String)> }
pub struct PluginList { pub profile: String, pub fetched_at: u64,
    pub plugins: Vec<Plugin> }
pub async fn fetch(profile: &Profile) -> Result<PluginList, ApiError>  // GET /v1/claudio/plugins
pub fn load_cache(profile: &str) -> Option<PluginList>                  // ~/.config/claudio/plugins.json
pub fn save_cache(&PluginList)                                           // one entry per profile
pub fn remove_cache(profile: &str)
pub fn admit(p: &Plugin) -> Result<(), String>                           // validation below
```

- The list is fetched for the profile a session is spawned with (the
  default profile, `--proxy <name>`, or the `env` profile); a session with
  no proxy gets no plugins. `plugins.json` is keyed by profile name, one
  entry per profile. `tui/mod.rs` fetches on start for the profiles in
  use (an `Effect`, never in `App`), after every successful login, every
  15 min, and on `Alt+i` → `r`.
- A 401 or 403 from `/v1/claudio/plugins` deletes that profile's cache
  entry and sets the state "signed out: run claudio proxy login". Any
  other error, or a 404 from an old proxy, keeps the cache entry and sets
  the state "proxy unreachable, cached <age>"; a 404 with no cache means
  no plugins.
- `admit` rejects: a name not `[a-z0-9-]{1,40}`; a non-`https` URL; a hash
  that is not 64 lowercase hex; an env key not `[A-Z_][A-Z0-9_]{0,63}`, or
  with a prefix in `ANTHROPIC_`, `CLAUDIO_`, `CLAUDE_`, `NODE_`, `LD_`,
  `DYLD_`, `GIT_`, `SSL_`, `XDG_`, `SSH_`, or equal to `PATH`, `HOME`,
  `SHELL`, `TERM`, `USER`, `LOGNAME`, `TMPDIR`, `BASH_ENV`, `ENV`,
  `PYTHONPATH`, `PERL5OPT`, `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`,
  `NO_PROXY` (and their lowercase forms); a value over 4 KiB, containing
  NUL, or any C0 control other than `\n` and `\t`; more than 64 keys;
  more than 32 plugins. A rejected plugin is listed as "refused: <reason>"
  (naming the key only, never a value) and never used.
- `plugins.json` holds secrets: `save_cache` calls
  `paths::ensure_private_dir(config_dir())` then `write_atomic` (0600).
  `claudio proxy logout <profile>` removes that profile's entry.
  `CLAUDE.md`'s rule becomes: the journal and `state.json` never hold
  secrets; `config.toml` and `plugins.json` (0600) do.
- `Plugin` and `PluginList` implement `Debug` with values redacted; the
  fetch event logs plugin names and key counts only.

### 3. Extensions screen

`Registry::set_plugins(list: &[Plugin], enabled: &BTreeMap<String, bool>)`
replaces the registry's `Source::Plugin` entries whenever a list arrives: an
admitted plugin whose name matches a built-in or external is refused ("name
taken") and listed as refused; the `[extensions]` map from config is kept in
`App` so a plugin that appears later gets its saved `false`. Each entry:
name, description `"<version> · <profile>"`, no lenses or actions, enabled
by default, toggle saved as `[extensions] <name> = false` through the
existing `Effect::SaveExtension`. A toggle applies to new sessions only,
and the details pane says so. The details pane lists one line per connected
host: `<host>: ready | downloading | installed in Claude Code (bundle
skipped) | download failed: … | daemon too old`, and one line for the list:
`proxy unreachable, cached <age>`, `signed out`, or `refused: …`. The key
`r` (refresh) is added to the screen's plain keys.

### 4. Bundle on each host

New op and reply in `src/proto.rs` (both `Msg` variants):

```rust
Msg::Bundle { name: String, url: String, sha256: String, installed_id: String }
Msg::Bundled { name: String, path: Option<String>, installed: bool }
```

The daemon (`src/daemon/bundle.rs`), from a spawned task, replying through
the client's queue:

0. Checks the op: `name` matches `[a-z0-9-]{1,40}`, `sha256` is 64
   lowercase hex, `url` is `https://`; otherwise `Error "bundle <name>:
   bad request"`. The cache dir `<cache>/plugins` is created with
   `paths::ensure_private_dir` (0700). At most 32 zips are kept; a 33rd
   name is refused.
1. If `installed_id` is non-empty and this host's
   `$CLAUDE_CONFIG_DIR/plugins/installed_plugins.json` (when the daemon's
   login env sets `CLAUDE_CONFIG_DIR`, else `~/.claude/plugins/…`; version
   2: `plugins` keyed by `<plugin>@<marketplace>`) has that key, reply
   `installed: true, path: None`. A missing or unparsable file counts as
   not installed.
2. Else, if `<cache>/plugins/<name>.zip` exists and its sidecar
   `<cache>/plugins/<name>.sha256` equals `sha256`, reply with the path.
3. Else download `url` with `reqwest` (30 s connect, 120 s total, 64 MiB
   cap, redirects only to `https`), stream to a temp file in the cache dir,
   verify SHA-256, write the sidecar, then rename the zip into place over
   any previous version. The path is stable per plugin, so journaled args
   stay valid across versions; a session that already started keeps the
   zip it opened. `<cache>` is `$XDG_CACHE_HOME/claudio` or
   `~/.cache/claudio`.
4. Any failure replies `Error "bundle <name>: <reason>"` and leaves no
   partial file.

One download per `(name, sha256)` at a time; a second request joins it.
The TUI sends `Bundle` for every enabled plugin to each connected host on
connect and whenever the plugin list changes, and `App` keeps the reply per
`(host, plugin)`. An old daemon's `Error "unsupported op"` sets the state
`daemon too old` and the session is spawned without the bundle (env still
applies).

### 5. Spawn

`send_spawn` and `reset_session`, for claude sessions with a proxy profile:

- `args` gets `--plugin-dir <path>` per enabled plugin whose host reply had
  a path. The daemon drops `--plugin-dir` pairs from `spec.args` when its
  `claude --help` does not list `--plugin-dir` (same probe as
  `ALLOW_SKIP_PERMISSIONS`, cached in `Daemon` like
  `skip_permissions_supported`), so an old claude still starts.
- `App` puts the admitted plugin env (every enabled plugin, in registry
  order) into `SpawnSpec.env` before `send_with_proxy`; `proxy_env_for`
  and the cold path in `tui/mod.rs` then append the proxy env and remove
  any plugin key the proxy env also sets (`env::merge(plugin, proxy)`:
  proxy wins, no duplicate key reaches `apply_env`). Over SSH the env
  travels in `Spawn.env` as today; the path is the remote host's.
- A spawn whose host has not yet answered `Bundle` for an enabled plugin
  is deferred like a cold proxy config (`Effect::SpawnWithProxy`,
  `ReplyTo::deferred`): `tui/mod.rs` waits for the pending replies up to
  20 s (the status line shows "fetching plugin <name>…"), then spawns with
  whatever answered; a plugin still pending or failed is left out and a
  notice says so. `--plain` (`src/tui/plain.rs:56`) goes through the same
  deferral, so its one session gets the bundle when the download is quick
  and starts anyway when it is not.
- A journal re-spawn at daemon start keeps the `--plugin-dir` args (not
  secret; the path is stable per plugin) and, as today, no env. `Alt+e`
  keeps the journaled args and sends the current env (`RespawnSpec` has no
  args); a plugin turned off or added applies to new sessions only.

### Alternatives considered

| Option | Why not |
|---|---|
| `--plugin-url` (claude fetches the zip itself) | No hash check, a download per session, and the remote host's claude would need the URL reachable without claudio knowing. |
| A `plugins` field on `SpawnSpec` | An old daemon would ignore it and start a session silently without the bundle; a separate op fails safe (CLAUDE.md). |
| Per-version zip paths | Journaled args would point at pruned files after a version bump; `RespawnSpec` carries no args to fix them. |
| Writing MCP servers into `~/.claude.json` | Installs state into the member's own config; conflicts with their setup. |
| Memory-only env | Owner chose the disk cache for offline starts. |

## Compatibility and rollout

- Protocol: one new op and one new reply, backward compatible (`PROTO`
  unchanged; unknown op → `Error`; an unknown reply op → `Msg::Unknown`).
- Config: `[proxy.profiles.<n>] email` optional (`Profile` has no
  `deny_unknown_fields`, safe on rollback); `[extensions] <plugin> = false`
  uses the existing section and is a harmless unknown name to an older
  claudio. `plugins.json` is new; absent means no cache.
- Proxy: requires sub-project 1 for login and plugins; against an older
  proxy both degrade to today's behaviour with a notice.
- Rollout: proxy deployed and `vocdoni-internal` registered first; then a
  claudio release; members run `claudio proxy login` once.
- Rollback: the previous claudio ignores `plugins.json` and the `email`
  field; the daemon's cache dir can be deleted.

## Failure modes and security

- Bundle zips are untrusted input to Claude Code, not to claudio: claudio
  never unpacks them; the hash pins the exact file the admin registered.
  Code execution by a hostile bundle is accepted: the admin's hash pins
  what runs.
- Secrets: plugin env goes only through `Spawn.env` over the uid-checked
  socket (and the SSH bridge, as proxy tokens do today), never argv,
  journal or `state.json`; `SpawnSpec`/`RespawnSpec` `Debug` already redact
  env values and the new types do the same. Logs and notices name keys
  only.
- A plugin cannot set the proxy's own keys, Claude Code's `CLAUDE_*` and
  `ANTHROPIC_*` switches, the process-level proxy and TLS variables, or
  loader and interpreter hooks (admitted keys).
- The daemon validates every `Bundle` op itself (`name` is a path
  component) and never writes outside its 0700 cache dir.
- Download limits: 64 MiB, 120 s, https only, one in flight per bundle;
  the cache holds one zip per plugin name; a failed download leaves no
  partial file.
- The daemon never blocks its actor or accept loop on a download (spawned
  task; reply through the client's queue, as other slow requests).
- A revoked token (401/403) drops the profile's cached secrets at the next
  fetch.
- A member who installed the plugin in Claude Code gets no duplicate
  skills (`installed: true` skips the bundle; env still applies).

## Acceptance criteria

- [ ] AC1: unit tests for `plugins::admit` (every rejection), cache load/save/remove (mode 0600, per-profile entries), `env::merge`, and the login state machine against a fake HTTP server (pending → 200; slow_down; expired; denied; 404 → paste fallback).
- [ ] AC2: daemon test (`src/daemon/tests.rs`): `Bundle` with a bad name, hash or non-https URL is an `Error`; against a fake installed list replies `installed`; a cached zip with a matching sidecar is reused; a wrong hash is an `Error` and leaves no file.
- [ ] AC3: manager test (`tests/manager.rs`): a fake claude whose `--help` prints a line containing `--plugin-dir`, a fake proxy serving one plugin (`GET /v1/claudio/plugins`) and a pre-seeded `<cache>/plugins/<name>.zip` + `.sha256` (a `bundle_url` the daemon never reaches): the spawned session's argv has `--plugin-dir <cache path>` and its env has the plugin key; toggled off in `Alt+i`, the next spawn has neither and `config.toml` has `<name> = false`; with the default fake (`--help` empty) the session starts with no `--plugin-dir`.
- [ ] AC4: `make test-unit` and `make test-manager` green; CI (Linux + macOS) green.
- [ ] AC5: manual, with the dev proxy: `claudio proxy login` through the browser, `Alt+i` shows `vocdoni-internal` ready, a new session lists the bundle's skills, and the same on `z6` over SSH.
- [ ] AC6: `tests/remote.rs` (gated by `CLAUDIO_SSH_TEST_HOST`): `Msg::Bundle` to the remote daemon with a pre-seeded cache replies `Bundled { path: Some(<remote path>) }` and a spawn there has that path in argv.

## Constraints

- `app.rs` stays pure: every fetch, download request and file write is an `Effect` driven by `tui/mod.rs`.
- Never add a field to an existing op; new behaviour is a new op that fails safe on old daemons.
- Secrets never in argv, journal, `state.json`, logs or notices.
- Keys stay on `Alt`; the `Alt+i` screen gains no new bindings except `r` (refresh), checked for collisions.
- `cli.rs` forwards claude's flags unchanged; `--plugin-dir` is added by claudio's spawn path only, never parsed from the user's args.
- Conventional commits, no AI attribution or co-author trailers. Keep Linux and macOS compiling.

## Open questions

- None.
