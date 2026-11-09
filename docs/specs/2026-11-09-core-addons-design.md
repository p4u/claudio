# Core and addons: design

Status: audited (spec audit + second-opinion review folded in, 2026-11-09)   Class: architectural
Owner decisions so far:
- 2026-11-09: claudio becomes a small generic core (a wrapper of the `claude` CLI that captures
  its screen, runs sessions locally and over SSH, installs and updates claude) plus addons for
  everything else: the API server, the web UI, the proxy. By default it works on a plain host
  with a normal `claude` login.
- 2026-11-09: everything Vocdoni-specific (the claude-proxy client, the vocdoni-internal team
  plugins, the proxy host) leaves the core and becomes one addon that needs only the member's
  proxy token to work.
- 2026-11-09: code acceptance rules come first (`ACCEPTANCE.md`), and every later change is
  checked against them.
- 2026-11-09: LaTeX and any other screen-modifying extension affect only the web UI. The console
  TUI stays simple and is part of the core.
- 2026-11-09: the owner delegated every design decision ("don't ask me anything"). Pushes, tags
  and releases still wait for the owner.

## Problem

claudio grew one feature at a time into an 81k-line single crate (`src/`, 136 Rust files) where
every feature reaches into every other one:

- **The proxy is woven through the core.** `tui` names `crate::proxy` 52 times, `web` 18 times.
  `proto` (`Msg::merge_env`, proto.rs:655), `config` (`save_plugin_choices`, config.rs:597),
  the daemon (`apply_env` → `proxy::env::env_diff`, session.rs:381; `daemon/manifest.rs`
  validators) and `tui::fmt` (fmt.rs:6-9) all import it. The `App` has 10 proxy/plugin fields
  (app.rs:311-393), 8 proxy/plugin `Effect`s (app.rs:110-211), 6 `HostEvent`s and the largest
  `run_effect` arm (`SpawnWithProxy`, tui/mod.rs:1550-1637, 88 lines).
- **The web UI is built on the TUI's internals.** `src/web` imports 12 `crate::tui::` items
  (state, connections, plugins, plugin_setup, extensions, snippets, stats_view, sessions).
  `web/plugins.rs` is "the web's copy of the TUI's plugin state machine" (its header), 1019
  lines. Spawn composition, the proxy config cache, snippets, config writes and the
  session-event apply exist twice.
- **Closed sets everywhere.** A feature edits `Msg`, `SessionEvent`, `Effect`, `ReplyTo`,
  `HostEvent`, `Modal`, `Action`, `run_effect` (349 lines), `event_loop` (348 lines) and the
  daemon's `request` (171 lines).
- **The TUI rewrites claude's screen.** The LaTeX typesetter (`tui/typeset`, `tui/latex.rs`,
  2.9k lines), the Mermaid renderer (`tui/diagram`, 2.7k lines) and tag lenses overlay the pane
  and the reader. The owner wants that only in the web UI.
- **Module cycles from misplaced helpers.** `proto → claude → proto` (`claude::latex::sanitize`),
  `freshness ↔ client`, `plugins ↔ proxy`; and `upgrade → remote` for release constants.
- **No quality floor.** `cargo fmt --check` reports 399 hunks; clippy shows about 49 warnings in
  the binary; 9 functions exceed 150 lines; 6 files exceed 1900 lines.

## Goals

1. **G1 Rules.** `ACCEPTANCE.md` defines the acceptance rules; `make check` enforces the
   mechanical ones, and CI runs it.
2. **G2 A generic core.** `claudio-core` + `claudio-tui` build and pass their tests alone,
   contain nothing Vocdoni, and run the manager, passthrough, daemon, SSH and claude install with
   a plain `claude` login and no config.
3. **G3 Addons through one API.** The API server (with `-p`), the web UI and the Vocdoni addon
   are separate crates that extend claudio only through `claudio_core::addon`, documented public
   core services and the daemon protocol.
4. **G4 One implementation per behaviour.** No domain logic exists twice across TUI and web:
   spawn composition, plugin state, proxy caches, snippets and config writes each live once.
5. **G5 A plain console TUI.** The TUI draws claude's screen untouched; formulas, diagrams and
   lenses render only in the web UI.
6. **G6 Vocdoni out of the box.** With the Vocdoni addon built in, `claudio vocdoni login` (a
   token, or the browser sign-in) is the only step a member takes; sessions then get the proxy,
   the team plugins, their MCP servers, agents and instructions, as today.
7. **G7 No user-visible regression** apart from what G5 removes. Existing `config.toml`,
   `state.json`, journal, `web-state.json` and `plugins.json` keep working, and a previous-release
   peer (client or daemon) keeps working with a new one for one release.

## Non-goals

- Runtime-loaded plugins (dynamic libraries, WASM). Addons are crates compiled into the binary
  and selected with cargo features; they switch on at runtime by configuration.
- New features. This work moves, merges and deletes code; behaviour changes only where G5 and G6
  say so.
- Changing the external extension protocol v1 (`docs/extensions.md`) beyond moving it.
- Separate release flavours. The release binary keeps every addon (the `default` features); a
  core-only download is a later decision.
- A universal application framework. The addon API has exactly the capabilities today's API,
  web and Vocdoni features need, and grows only when a real consumer needs more.

## Current state

The essentials (full file:line maps are in the delivery workspace):

- **Modes** are chosen in `src/main.rs:38-309`: `__probe`, `--slave`, diag commands, the print
  hook relays (`<Event> <port>` at :106-121, which must be recognised first, and the legacy
  `__hook <Event>` at :144), the manager hook `__hook <Event> <sock> <token>` (:129),
  `__context` (:136), `--daemon`, `upgrade`, `proxy …` (:161), `daemon …`, `sessions`, `web`
  (:176), `--plain` (:182), bare (:188), `--proxy`/`--no-proxy`/`--first-party` (:191), then
  `cli::parse`: help, `--api` (:238), passthrough (:246), `-p` (:276).
- **print, api, cli, msglog** form a closed cluster that depends only on `term` and
  `paths::unix_now`. API sessions run with tools disabled (`api/pool.rs:126-153`) and must stay
  outside session launch composition.
- **The daemon** never depends on `tui`, `web` or `client`. Its feature ops share the `request`
  match (`daemon/server.rs:417-587`).
- **Spawn composition**: `send_spawn` (tui/sessions.rs:402) → `send_with_proxy` (:465) →
  `compose` (tui/plugins.rs:175) → `Msg::merge_env`; the web's copy is `web/spawn.rs:166-343`.
  Respawn refreshes env and instructions but not the journaled args (proto.rs:641-684). The
  daemon applies `env_diff` (proxy/env.rs:151) for every `Spawn` and `Respawn`: always
  `claude::SESSION_MARKERS`, plus `ANTHROPIC_API_KEY` (and for first-party
  `CLAUDE_CODE_USE_GATEWAY`) when the env sets `ANTHROPIC_AUTH_TOKEN`.
- **Routing failures today**: a chosen profile that no longer exists fails the launch; an
  unreachable `/v1/claudio/config` only falls back to built-in model defaults
  (proxy/resolve.rs:111-130, tui/mod.rs:1566-1615). Neither starts a direct session.
- **Plugin review**: a first-seen manifest opens the setup screen; a host is asked `Manifest`
  only after the member saved the choices (tui/plugins.rs:538-541); `--plain` waits while the
  screen is open (tui/plugin_setup.rs:344-387); the web distinguishes save, skip-for-this-launch
  and cancel (web/app/ui/plugin-setup.js:98-113).
- **Mixed versions exist**: freshness leaves a busy older daemon running (freshness.rs:113-124)
  and a failed check is logged and ignored (client.rs:785-790); a newer daemon is never
  replaced by an older client.
- **Lenses**: the daemon extracts tag/fence items only for the lenses the spawning client lists
  in `SpawnSpec.extensions.lenses` (proto.rs:506-514, session.rs:604-760); the web never
  configures extraction after spawn (web/hub.rs:484-500, :1643).
- **Web protocol** v1 (`ws.rs:128`); the page reloads once when the server's `v` differs
  (`web/app/proto.js:53-56`).

## Design

### Crates (final layout, P4)

```
Cargo.toml            workspace, plus the `claudio` binary package (src/main.rs: wiring only)
crates/core           claudio-core    proto, paths, config, claude/, term/, client, remote/,
                                      freshness, release, daemon/, addon/ (the API), app/ (the
                                      shared layer)
crates/tui            claudio-tui     the console manager and --plain
addons/api            claudio-api     -p print emulation and the --api server
addons/web            claudio-web     `claudio web`: server, page (web/), build.rs
addons/vocdoni        claudio-vocdoni the claude-proxy client and the team plugins
```

Dependencies (A2): `claudio` → {tui, api, web, vocdoni}; tui → core; every addon → core only.
An addon may take `claudio-tui` or another addon as a **dev-dependency** for integration tests,
never as a dependency. The binary's features `api`, `web`, `vocdoni` (all `default`) select the
linked addons; `src/main.rs` builds one immutable `Addons` registry (`Arc<Addons>`) and passes it
to every entry point (`tui::run(addons)`, `tui::plain(addons, …)`, `daemon::run(addons)`, and
each command).

**The shared app layer** (`claudio_core::app`) holds what both front-ends need and today lives
in `tui`: `ClientState`/`SavedSession` (state.json), `Connections`, the extension registry,
host, manifest and protocol v1, snippets, `sanitize_label`, `open_browser`, the launch composer
and the addon runtime that drives addon tasks. It has no ratatui, crossterm or axum. Front-ends
keep their own navigation and interaction state; domain transitions live here once (A5).

### The addon API (`claudio_core::addon`)

An addon is a value implementing `Addon`. It exposes optional **capabilities**; an addon
implements only those it has.

```rust
pub trait Addon: Send + Sync + 'static {
    /// `[a-z][a-z0-9-]{0,31}`: its CLI word and registry key.
    fn name(&self) -> &'static str;
    /// Top-level `config.toml` tables it owns (`["proxy", "plugins"]` for Vocdoni). Two
    /// addons claiming one table, or a core table, is refused at registration.
    fn config_sections(&self) -> &'static [&'static str] { &[] }
    fn commands(&self) -> Option<&dyn Commands> { None }
    fn launch(&self) -> Option<&dyn Launch> { None }
    fn ui(&self) -> Option<&dyn Ui> { None }
    fn daemon(&self) -> Option<&dyn DaemonOps> { None }
}
```

- **`Commands`**: `is_relay(argv) -> bool` (checked before anything else, for print's hook
  relays), `dispatch(argv, &Arc<Addons>) -> Option<ExitCode>` (after the core's own commands,
  in registry order, before passthrough), `help() -> &str`. The core's words (`__probe`,
  `--slave`, `__hook`, `__context`, `--daemon`, `upgrade`, `daemon`, `sessions`, `--plain`) are
  reserved. The API addon decides with the existing `cli::parse`, not by scanning argv.
- **`Launch`**: `flags()` (e.g. `--proxy NAME`, `--no-proxy`, `--first-party`, parsed by the
  core on `claudio …` and `claudio --plain …`), `options(ctx) -> Vec<LaunchOption>` (the
  wizard's rows: `{ id, label, group, default, detail }`), `resolve(choice, ctx) -> Routing`
  and `prepare(req, ctx) -> BoxFuture<Result<LaunchPart, LaunchError>>` (below).
- **`Ui`**: `segments(session, ctx) -> Vec<Segment>`, `panels() -> &[PanelDecl]`,
  `panel(req, ctx) -> BoxFuture<Panel>`, `action(req, ctx) -> BoxFuture<Result<Notice,
  String>>`, `checklist(id, ctx) -> Option<Checklist>`, `save_checklist(id, answer, ctx)`,
  `extension_rows(ctx) -> Vec<ExtensionRow>` and `row_action(row, action, ctx)` (toggle,
  open setup, refresh).
- **`DaemonOps`**: `handle(op, body, DaemonCtx) -> BoxFuture<Result<Value, String>>`, run inside
  a host's daemon.

**Execution model.** An addon is a pure state machine (A6: no I/O, unit-tested) wrapped by its
capability impls, which are the runner. `AddonCtx` (implemented once in `app/`, used by the TUI
runner and the web hub) offers:
`spawn(fut)` (tasks on the front-end's runtime, aborted when the registry drops), `http()` (one
shared `reqwest::Client`), `request(host, op, body) -> BoxFuture<Result<Value, Error>>` (an
`AddonOp`, sent only when that host's `Welcome.features` lists `addon:<name>`), `host_has(addon)`,
`sessions()` and `hosts()` (snapshots), `config(section) -> toml::Table`, `edit_config(section,
f)` (atomic, smallest key set, other text and comments preserved; serialised), `notify(text)`
and `changed()` (the front-end re-reads segments, rows and open panels).
`on_event(AddonEvent)` delivers, after an initial snapshot: `Tick` (1 s), `HostUp { host,
features }`, `HostDown`, `SessionAdded/Removed/Changed { sid, conversation }` (a new
conversation id is how `/clear` is seen), `ClaudeInstalled { host }` and `ConfigChanged`.
`segments()` and `extension_rows()` read a snapshot under a `std::sync::Mutex` never held
across an await or a host callback, and return without I/O. Every addon task has a bound,
cancellation, a stale-result check (request generation) and ends at shutdown.

**Data, not widgets.** `Segment { id, text, tone, priority }`; `Panel { title, pages: [Page {
title, blocks: [Block] }], actions: [PanelAction { id, label, key, confirm: Option<String> }],
updated }` where `Block` is `Text`, `KeyValues`, `Table`, `Gauge { label, used, limit, resets }`
or `Bars { label, series }`; `PanelRequest { id, page, window, sid }` carries the view-local
state (the TUI's popup and each browser tab keep their own); `Checklist { id, revision, title,
intro, items: [{ id, label, detail, on }] }`; `Tone` is `Ok | Warn | Hot | Muted | Accent`.
Blocks are plain text: the web renders them as text nodes (never markup), the TUI as `Span`s.
The core sanitises every string (terminal controls, `sanitize`) and caps them (segment 64 chars,
panel ≤ 64 KiB encoded, checklist ≤ 256 items) before a front-end sees them. The TUI renders
them with one generic panel view (replacing `stats_view` + `ui/stats.rs`) and one checklist view
(replacing `plugin_setup`); the page with one panel and one checklist component (replacing
`stats.js` and `plugin-setup.js`), following DESIGN.md. `PanelDecl.key` binds a panel to a key
(default Alt+s for the Vocdoni panel; `[keys] proxy_stats` still overrides it), checked for
collisions by `keymap.rs`.

**The TUI stays pure (A6).** `App` gains generic variants only: `Effect::Addon(AddonCall)`,
`Effect::Launch(LaunchRequest)`, `HostEvent::Addon(AddonResult)`, `HostEvent::Launched { id,
notices }`, `Modal::Panel`, `Modal::Checklist`. **A `LaunchPart` never enters `App`**: the runner
composes the launch and sends `Spawn`/`Respawn` itself; `AddonResult` carries `Panel`,
`Segment`, `Checklist`, `Notice` and errors only.

### Launches

1. **Resolve routing first.** From the flags, the wizard's choice or the persisted session
   choice, the core asks each `Launch::resolve`, giving each addon `Routing::Required` (its
   option is in effect, explicitly *or* as its configured default, e.g. a default proxy
   profile), `Routing::Optional` or `Routing::Off`. An unknown persisted choice (an addon or
   profile that no longer exists) fails the launch with a message, as the web does today; it
   never becomes a direct session.
2. **Prepare.** `LaunchRequest { op: New | Resume | Respawn | Recover, sid, host, cwd, choice,
   frontend }` goes to each addon's `prepare`. Network waits are bounded by 25 s total (today's
   5 s config fetch + 20 s manifest wait); a `Required` addon's error or timeout fails the
   launch; an `Optional` one's becomes a notice and the launch goes on without it. Within a
   required addon, the addon itself separates what is required (the proxy env) from what is an
   enhancement (a plugin whose host has not answered): today's "unreachable config → built-in
   model defaults" stays inside the Vocdoni addon.
3. **Review.** `prepare` may return `Err(LaunchError::NeedsReview { checklist, revision })`. The
   core shows that checklist (queued, like a confirm) and stops the 25 s clock while it is open.
   Enter calls `save_checklist` and re-runs that addon's `prepare`; "skip" re-runs it with the
   item set deferred for this launch only; Esc cancels the launch. A revision change while open
   reopens it. `--plain`, the TUI and the web wizard use this one path; `App::plain_pending` and
   the web's plugin-gate code are deleted.
4. **Compose.** Each addon returns `LaunchPart { env, env_remove, args: [LaunchArg], lenses:
   [LensSpec], instructions, notices }` (its `Debug` redacts env values). `compose` (moved from
   `tui/plugins.rs`; the one implementation for TUI, web and `--plain`) merges them: env in
   registry order, a later addon's key wins, no key twice (today's layering `[env]` → secrets →
   proxy env is inside the Vocdoni part); `args` appended in order, `LaunchArg::Optional { flag,
   value }` dropped by the daemon when the host's claude lacks `flag` (today's `ClaudeFlags`
   rule made generic); `lenses` merged by name (first wins) into `SessionExtensions.lenses`;
   instructions joined by a blank line and cut at 8158 bytes; the whole `Spawn` frame checked
   against the frame cap before sending. A `Respawn` gets env, env_remove, lenses and
   instructions but not args (the journal keeps those), as today. A terminal launch skips the
   addons.

**Persistence.** The chosen option persists per session as `launch: Option<LaunchChoice>`
(`{ addon, option, flags }`) in `state.json` and `web-state.json`. The old `proxy`/`first_party`
fields are read into it (B3) and, for one release, also still written (derived from `launch`
when its addon is `vocdoni`), so reverting to the previous build keeps every session's proxy.
The daemon journal is unchanged (it never held env or routing).

### Daemon ops for addons

`Msg::AddonOp { addon, op, body: Value }` → `Msg::AddonReply { body: Value }` (or `Error`), new
ops (B1). The daemon finds the addon in its registry and runs `DaemonOps::handle` on a task (one
"reply later" helper, which the existing slow ops also adopt). At most 8 in flight per addon;
finer keys are the addon's (the Vocdoni addon keeps one install task per manifest hash and the
shared `updating` lock, as today). `DaemonCtx` offers `claude_bin()`, `path()`,
`claude_flags()`, `cache_dir(addon)`, the `updating` lock and an `on_claude_installed` hook (today's
`Manifests::forget_failures`). Bodies ≤ 512 KiB encoded, checked by `proto` on both sides.
`Welcome.features` lists `addon:<name>` per daemon-side addon; a client sends an `AddonOp` only to
a host that lists it. A core-only daemon lists none.

### Mixed versions (one-release window)

| Peers | What happens |
|---|---|
| new client → new daemon | `AddonOp`, `env_remove: Some(..)` |
| new client → old daemon (busy, deferred, failed check) | no `addon:vocdoni` in features → no `AddonOp`; the launch gets the proxy env without the plugins' share and one notice ("daemon too old, plugins after `claudio daemon restart`"). The old daemon applies its own `env_diff` (a superset of what the addon sends in `env_remove`). |
| old client → new daemon | `Spawn`/`Respawn` without `env_remove` (`None`) → the daemon's **compatibility scrub** applies today's `env_diff` rule. Its `Manifest` op is refused (`unsupported op`) → the old client's existing "daemon too old" path, session without plugins. |

`env_remove` is `Option<Vec<String>>` on both `SpawnSpec` and `RespawnSpec`
(`#[serde(default)]`): `None` means "an old client, scrub as before", `Some(list)` means "remove
exactly the markers plus this". Manager tests cover both directions, including an inherited
`ANTHROPIC_API_KEY`. The compatibility scrub and the legacy field writes are removed one release
later (dated in the plan).

### The Vocdoni addon (`claudio-vocdoni`)

Everything in `src/proxy/`, `src/plugins/`, `daemon/manifest.rs`, `tui/plugins.rs`,
`tui/plugin_setup.rs`, `tui/proxy_state.rs`, `tui/stats_view.rs`, `tui/ui/stats.rs`,
`web/proxy.rs` and `web/plugins.rs` becomes this addon, with one plugin state machine and one
proxy cache.

- **Commands**: `claudio vocdoni login [--token] | status | use NAME | logout`. `login` without a
  URL signs in to `https://claude.vocdoni.net` (the addon's constant); `--token` pastes one.
  `claudio proxy …` stays as an alias with the same subcommands.
- **Launch**: options are the profiles (`[proxy.profiles.*]`, unchanged) with first-party per
  profile; flags `--proxy NAME`, `--no-proxy`, `--first-party`; `CLAUDIO_PROXY_URL` keeps
  working. A default profile resolves `Required`. `prepare` returns the profile env, the team
  plugins' share (`--agents`/`--mcp-config` as optional args, secrets as env, instructions) and
  `env_remove` (`ANTHROPIC_API_KEY`, and `CLAUDE_CODE_USE_GATEWAY` for first-party).
- **Ui**: segments (route, credential, 5h and 7d windows), the panel "Proxy" (today's stats
  pages; "switch subscription" as a `PanelAction` with a confirm), one checklist per manifest,
  extension rows for its plugins. Credential lookups follow `SessionChanged { conversation }`;
  stale replies are dropped by generation, as `app.rs:562-643` does today. Actions resolve the
  session's profile and conversation from the addon's own state, never from browser input.
- **Daemon**: `AddonOp vocdoni/apply` is today's `Manifest` (download, verify, install with
  `claude plugin … --json`, back-off, the `Applied` answer), unchanged in behaviour; re-asked
  on `HostUp` and `ClaudeInstalled`.
- **Config**: `[proxy]`, `[plugins.<name>]` and `plugins.json` stay where and as they are.

### The API addon (`claudio-api`)

`print/`, `api/`, `cli.rs`, `msglog.rs` with `Commands` only: `is_relay` for the print relays,
`dispatch` for `-p`/`--print` and `--api` (decided by `cli::parse`, so `--` keeps its meaning),
`help` for `HELP_APPENDIX`. Its PTY sessions stay outside launch composition (tools disabled).

### The web addon (`claudio-web`)

`src/web/` and `web/` with `build.rs`, depending on `claudio-core` only, `Commands` claiming
`claudio web …`. It reaches other addons only through the registry it is handed.
**Web protocol v2** (`docs/web-protocol.md` first, B2; already-open tabs reload through the
existing version handshake): `addons.list` → `[{ name, panels, launch_options }]`;
`addons.panel { addon, id, page?, window?, sid? }` → `Panel`; `addons.action { addon, id,
action, sid? }` → `Notice`; `addons.checklist { addon, id }` → `Checklist`;
`addons.checklist.save { addon, id, revision, answer }`; `addons.rows` / `addons.row_action`;
event `addons.changed { addon }`. `proxy.*` and `plugins.*` ops are removed. The Session DTO
drops `proxy`, `proxy_known`, `first_party` and gains `launch` and `segments`. The wizard renders
one select per `LaunchOption.group`; a `PanelAction.confirm` goes through `modal.js`.

Formulas, diagrams and lenses are web-only (G5): the page keeps its KaTeX and Mermaid rendering,
fed by the daemon's lens events. The web addon's `Launch::prepare` contributes the `latex` and
`mermaid` **lenses** and their instructions when `[extensions] latex`/`diagram` are on (defaults
as today; `[claude] latex` stays an alias of `[extensions] latex`), for sessions started from
any front-end, so the daemon extracts them. The TUI then shows claude's raw `<latex>…</latex>`
text; its extensions screen keeps the `latex`/`diagram` rows (the web addon's `extension_rows`)
so a console-only user can turn the tags off. A core-only build asks claude for no tags.

### The plain console TUI (G5)

Removed from the TUI: `typeset/`, `latex.rs`, `formulas.rs`, `diagram/`, the pane overlay, the
lens and formula state on `SessionView`, `lens_hosts`, the render worker's formula and
external-lens paths, `Effect::RenderLatex`/`RenderExternal`, and `__context latex` (pre-0.6
daemons). The reader (Alt+u) stays: it shows answers as markdown, with formulas and diagram
fences as their source text. Leader actions (search, snippets, external actions) and the
extensions screen stay. `rust-latex-parser` leaves the dependency tree. The daemon keeps lens
extraction (generic transcript capture, used by the web UI); `latex_event`/`lens_items_event`
merge into one routine.

### Untangling

`claude::latex::sanitize` → `claudio_core::text`; `paths` stops importing `proto` (the socket
file names take `PROTO` from a `version` module); `proxy::plugins::{name_ok, key_ok}` → the
Vocdoni addon; `upgrade::SemVer`, the release constants and arch helpers → `release`;
`proxy::login::{browser_wanted, open_browser}` → `app`; `proxy::{api::parse_rfc3339,
cmd::fmt_tokens}` → `text`; `freshness ↔ client` broken by passing a connect function into
freshness; `config::save_plugin_choices` → the generic `edit_config`.

### Alternatives considered

| Option | Why not |
|---|---|
| Separate executables (`claudio-web`, `claudio-api`) on `PATH`, git-style | The remote bootstrap uploads one binary; install, `upgrade` and freshness assume one file. The Vocdoni addon must run inside the TUI, the web server and every daemon, so it would need a process protocol for env, segments and panels anyway. |
| Dynamic plugins (`dlopen`, WASM) | No stable Rust ABI; a static musl binary; a large attack surface for no user need. |
| One crate with boundaries checked by a script | The compiler is the cheapest boundary checker. Used only until P4. |
| Addon UI as ratatui/Preact code inside each addon | Each addon would depend on both front-ends, and every addon screen would exist twice. Data plus generic renderers keeps A2 and A5. |
| One big `Addon` trait | Command-only addons (api, web) would implement meaningless UI methods. Capabilities keep each addon to what it does. |
| Extend extension protocol v1 | It is a stdio program protocol; in-process addons need typed async calls and daemon ops. v1 stays for external extensions. |

## Compatibility and rollout

- **Wire**: the mixed-version table above; the compatibility scrub and refused `Manifest` op for
  one release.
- **Persisted**: `config.toml` sections unchanged; `state.json`/`web-state.json` read old fields
  into `launch` and keep writing them for one release; the journal and `plugins.json` unchanged.
  Fixtures of the previous release's files load (tests).
- **CLI**: every command and flag keeps working (`claudio proxy …`, `--proxy`, `--no-proxy`,
  `--first-party`, `-p`, `--api`, `web`, `--plain`); new `claudio vocdoni …`.
- **Web**: protocol v2; old tabs reload once.
- **Tests** split by crate at P4: `crates/tui/tests/manager.rs` (core behaviour, fake claude,
  core-only build), `addons/vocdoni/tests/` (today's 6 proxy/plugin manager tests and 8 web
  proxy/plugin tests, with the TUI harness and `claudio-web` as dev-dependencies),
  `addons/web/tests/web.rs`, `addons/api/tests/integration.rs`. CI runs the feature matrix:
  core-only, each addon alone, and all.
- **Rollout**: each phase lands on `refactor/core-addons` with all suites green, then merges to
  local `main`. Push, tag and release wait for the owner. Rollback: revert the merge (the legacy
  field writes keep a reverted build's sessions intact).

### Phases (each its own plan, audited before execution)

| Phase | Delivers | Exit check |
|---|---|---|
| P0 Rules | `scripts/check_rules.py`, size/unwrap baseline, `make check`, CI job; one `cargo fmt` commit; clippy to zero | `make check` green |
| P1 Plain TUI | G5 (TUI side; lenses still requested by the TUI's registry until P3 moves them to the web addon) | all suites green; no `typeset`/`diagram`/`latex` module in the TUI |
| P2 Untangle | helpers moved, cycles broken, `app` layer extracted; `web` imports no `tui::` item | `grep -r 'crate::tui' src/web` empty |
| P3 Addon API by migration | `addon/` and the launch composer inside the single crate, built by moving the real Vocdoni slices behind it in order: routing + launch + respawn → review + daemon install → segments + panel + actions → rows; then api and web as command addons; web protocol v2 | the proxy and plugin tests pass unchanged against the API; `src/tui` and `src/web` name no `proxy`/`plugins` module |
| P4 Workspace | crates core, tui, api, web, vocdoni by moving modules; features; feature-matrix CI; A1/A2 checks active | `make check` green on every feature combination |
| P5 Docs | `CLAUDE.md` map, README (generic first, addons after), `docs/addons.md` (docs also move with each phase, F1) | review |

## Failure modes and security

- **Secrets** (C1): addon secrets live only in their private files (`config.toml`,
  `plugins.json`, 0600), in `LaunchPart.env` (redacted `Debug`, never in `App`, never
  serialised except into `SpawnSpec.env`) and in the daemon's spawn env. `AddonOp` bodies,
  `Panel`, `Segment`, `Checklist` and DTOs never carry a secret value (the `apply` body carries
  secret names, as `Manifest` does).
- **Untrusted data**: proxy responses enter only the Vocdoni addon, which keeps today's `admit`
  caps; the core then sanitises and caps every string an addon hands a front-end (above).
- **A slow or failing addon**: launch waits bounded as above; panel futures time out at 10 s
  with an error page; `AddonOp` handlers run off the daemon's request loop, ≤ 8 per addon.
- **Core-only build**: no `addon:*` features; nothing addon-related is sent or shown.
- **Partial moves**: every commit builds and passes unit tests; every phase passes every suite
  before it merges.

## Acceptance criteria

- [ ] AC1 (G1): `make check` runs fmt, clippy `-D warnings`, `scripts/check_rules.py` (A1, A2, A8,
  D2, D4), the core-only build and `cargo test --workspace --lib`; CI runs it on Linux and macOS.
- [ ] AC2 (G2): `cargo test -p claudio-core -p claudio-tui` passes; `cargo build
  --no-default-features` succeeds; the TUI crate's manager tests pass on that build.
- [ ] AC3 (G2, A1): the rules script's A1 check over `crates/` reports nothing.
- [ ] AC4 (G3, A2): the rules script's dependency check (`cargo metadata`) reports nothing.
- [ ] AC5 (G4): `web/plugins.rs`, `web/proxy.rs`, `tui/plugins.rs`, `tui/plugin_setup.rs`,
  `tui/stats_view.rs`, `tui/ui/stats.rs` no longer exist; `compose` has one definition.
- [ ] AC6 (G5): the TUI crate has no `typeset`, `diagram` or `latex` module; a manager test shows
  a `<latex>…</latex>` answer drawn as claude printed it; a test starts a session from the TUI
  with the web addon registered and finds `latex` in `SpawnSpec.extensions.lenses`; the web e2e
  formula tests pass.
- [ ] AC7 (G6): with the fake proxy, `claudio vocdoni login --token` then a new session gets the
  proxy env, the plugin args and the instructions (today's plugin manager tests, moved).
- [ ] AC8 (G7): every existing suite passes, tests moved but not weakened; previous-release
  fixtures of `state.json` and `web-state.json` load with their proxy; the mixed-version tests
  (both directions, inherited `ANTHROPIC_API_KEY`) pass; a default-profile launch whose proxy
  config fetch fails still starts through the proxy and never direct.
- [ ] AC9: the size baseline shrinks; `run_effect`, `event_loop` and the daemon `request` are
  under 80 lines or listed in the baseline no larger than today.

## Constraints

- Follow `ACCEPTANCE.md`; cite rule numbers in reviews.
- Every commit builds and passes unit tests; each phase passes every suite before it merges.
- Moves are moves: `git mv` where a file moves whole; behaviour changes and moves never share a
  commit; a move commit may rename baseline entries (same counts) and nothing else.
- Protocol docs first (B2).
- C1 and C5 hold throughout.
- No push, tag or release without the owner. Never `--amend`; `git add <exact paths>`.
- New test scratch under `~/.cache/claudio/`; never touch the owner's daemon or `claudio web`.

## Open questions

None blocking. Decisions the owner may revisit: the addon is named `vocdoni`; the release binary
keeps all addons; the TUI reader stays without formula rendering; formula tags stay on by
default (raw in the console, rendered in the web).
