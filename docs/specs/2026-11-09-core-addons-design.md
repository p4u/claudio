# Core and addons: design

Status: draft   Class: architectural (several sub-projects)
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

claudio grew one feature at a time into an 81k-line single crate (`src/`, 154 files) where every
feature reaches into every other one:

- **The proxy is woven through the core.** `tui` names `crate::proxy` 54 times, `web` 18 times.
  `proto` (`Msg::merge_env`, proto.rs:655), `config` (`save_plugin_choices`, config.rs:597),
  the daemon (`apply_env` → `proxy::env::env_diff`, session.rs:381; `daemon/manifest.rs`
  validators) and `tui::fmt` (fmt.rs:6-9) all import it. The `App` has 10 proxy/plugin fields
  (app.rs:311-393), 8 proxy/plugin `Effect`s (app.rs:110-211), 6 `HostEvent`s and the largest
  `run_effect` arm (`SpawnWithProxy`, tui/mod.rs:1550-1637, 88 lines).
- **The web UI is built on the TUI's internals.** `src/web` imports 12 `crate::tui::` items
  (state, connections, plugins, plugin_setup, extensions, snippets, stats_view, sessions).
  `web/plugins.rs` is "the TUI's plugin state machine re-expressed" (its own words, lines 1-2),
  1019 lines duplicating `tui/plugins.rs`. Spawn composition, the proxy config cache, snippets,
  config writes and the session-event apply exist twice.
- **Closed sets everywhere.** A feature edits `Msg`, `SessionEvent`, `Effect`, `ReplyTo`,
  `HostEvent`, `Modal`, `Action`, `run_effect` (349 lines), `event_loop` (347 lines) and the
  daemon's `request` (171 lines).
- **The TUI rewrites claude's screen.** The LaTeX typesetter (`tui/typeset`, `tui/latex.rs`,
  2.9k lines), the Mermaid renderer (`tui/diagram`, 2.7k lines) and tag lenses overlay the pane
  and the reader. The owner now wants that only in the web UI.
- **Module cycles from misplaced helpers.** `proto ↔ claude` (`claude::latex::sanitize`),
  `freshness ↔ client`, `upgrade ↔ remote` (release constants), `plugins ↔ proxy`.
- **No quality floor.** `cargo fmt --check` reports 399 hunks; clippy shows about 49 warnings in
  the binary; 9 functions exceed 150 lines; 7 files exceed 1900 lines.

## Goals

1. **G1 Rules.** `ACCEPTANCE.md` defines the acceptance rules; `make check` enforces the
   mechanical ones, and CI runs it.
2. **G2 A generic core.** `claudio-core` + `claudio-tui` build and pass their tests alone, name
   nothing Vocdoni, and run the manager, passthrough, daemon, SSH and claude install with a plain
   `claude` login and no config.
3. **G3 Addons through one API.** The API server (with `-p`), the web UI and the Vocdoni addon
   are separate crates that extend claudio only through `claudio_core::addon` and the daemon
   protocol.
4. **G4 One implementation per behaviour.** No logic exists twice across TUI and web: spawn
   composition, plugin state, proxy caches, snippets and config writes each live once.
5. **G5 A plain console TUI.** The TUI draws claude's screen untouched; formulas, diagrams and
   lenses render only in the web UI.
6. **G6 Vocdoni out of the box.** With the Vocdoni addon built in, `claudio vocdoni login` (a
   token, or the browser sign-in) is the only step a member takes; sessions then get the proxy,
   the team plugins, their MCP servers, agents and instructions, as today.
7. **G7 No user-visible regression** apart from what G5 removes. Existing `config.toml`,
   `state.json`, journal, `web-state.json` and `plugins.json` keep working.

## Non-goals

- Runtime-loaded plugins (dynamic libraries, WASM). Addons are crates compiled into the binary
  and selected with cargo features; they switch on at runtime by configuration.
- New features. This work moves, merges and deletes code; it changes behaviour only where G5 and
  G6 say so.
- Changing the external extension protocol v1 (`docs/extensions.md`) beyond moving it.
- Separate release flavours (a core-only download). The release binary keeps every addon (the
  `default` features); publishing a second flavour is a later decision.
- Rewriting the web front-end. It changes only where a generic renderer replaces an
  addon-specific screen (stats, plugin setup) and where proxy fields become addon data.

## Current state

Verified maps (file:line) live in the delivery workspace; the essentials:

- **Modes** are chosen in `src/main.rs:38-309`: `__probe`, `--slave`, diag commands, hook relays
  (print's `<Event> <port>` at :109 and the legacy `__hook <Event>` at :144; the manager's
  `__hook <Event> <sock> <token>` at :129), `__context` (:136), `--daemon`, `upgrade`, `proxy …`
  (:161), `daemon …`, `sessions`, `web` (:176), `--plain` (:182), bare (:188), `--proxy`/
  `--no-proxy`/`--first-party` (:191), then `cli::parse`: help, `--api` (:238), passthrough
  (:246), `-p` (:276).
- **print, api, cli, msglog** form a closed cluster: they depend only on `term` (probe responder,
  `strip_escapes`) and `paths::unix_now`. Nothing else uses them.
- **The daemon** never depends on `tui`, `web` or `client`. Its feature ops share the `request`
  match (`daemon/server.rs:417-587`): core (spawn, attach, kill…), git viewer, wizard reads,
  reader (`Answers`), snippets (`LastPrompt`), stats, `UpdateClaude`, `Search`, `Manifest`.
- **Spawn composition** today: `send_spawn` (tui/sessions.rs:402) → `send_with_proxy` (:465) →
  `compose` (tui/plugins.rs:175: per plugin `--agents` then `--mcp-config`, env layers
  `[env]` → secrets → proxy env, instructions ≤ 8158 bytes) → `Msg::merge_env`. The web's copy is
  `web/spawn.rs:166-285`. The daemon applies `SpawnSpec.env` through `apply_env`
  (session.rs:380), which calls `proxy::env::env_diff` (proxy/env.rs:151): it always removes
  `claude::SESSION_MARKERS`, and with an `ANTHROPIC_AUTH_TOKEN` also `ANTHROPIC_API_KEY` and, for
  first-party, `CLAUDE_CODE_USE_GATEWAY`.
- **No proxy config works today**: every proxy path is gated on `proxy: Option<String>` being
  `None`, and the loaders treat missing files as empty.
- **Extensions** (`tui/extensions/`): a registry of built-ins (latex, diagram, search, snippets),
  external programs (protocol v1: `render` for lenses, `action` for leader actions) and plugin
  rows (`Source::Plugin`, a bolt-on). It has no launch-time contribution, no status segment and
  no daemon call.
- **Lenses in the daemon**: `MetaRefresher` (session.rs:604-715) extracts `<latex>` and tag-lens
  items from the transcript and sends `SessionEvent::Latex` and `SessionEvent::Lens`; two copies
  of one routine (`latex_event` :717, `lens_items_event` :743).

## Design

### Crates

```
Cargo.toml            workspace, plus the `claudio` binary package (src/main.rs: wiring only)
crates/core           claudio-core   the engine: proto, paths, config, claude/, term/, client,
                                     remote/, freshness, release (upgrade), daemon/, the addon
                                     API (addon/), and the shared app layer (app/)
crates/tui            claudio-tui    the console manager and --plain
addons/api            claudio-api    -p print emulation and the --api server (print/, api/,
                                     cli, msglog)
addons/web            claudio-web    `claudio web`: server (src/), page (web/), build.rs
addons/vocdoni        claudio-vocdoni the claude-proxy client and the team plugins
```

Dependency direction (A2): `claudio` → {api, web, vocdoni} → tui → core; api and web may depend
on core only (web does not need the TUI); no addon depends on another. The binary's features
`api`, `web`, `vocdoni` (all in `default`) select which addons it links; `src/main.rs` builds the
addon registry from them.

**The shared app layer** (`claudio_core::app`) holds what both front-ends need and today lives in
`tui`: `ClientState`/`SavedSession` (state.json), `Connections`, the extension registry, host,
manifest and protocol v1, snippets, `sanitize_label`, and the launch composer (below). It has no
ratatui, crossterm or axum.

### The addon API (`claudio_core::addon`)

An addon is a value implementing `Addon`, registered once per process in an `Addons` registry
(core's own TUI, the web server, the daemon and `--plain` each build it from the same list). The
API is UI-agnostic: addons hand the core **data**, and each front-end renders it.

```rust
pub trait Addon: Send + Sync + 'static {
    /// `[a-z][a-z0-9-]{0,31}`: its config section, its CLI word and its registry key.
    fn name(&self) -> &'static str;
    /// Argv the addon claims, checked after the core's own commands and before passthrough.
    /// Returns `None` to let the next addon or the passthrough have it.
    fn dispatch(&self, argv: &[String]) -> Option<ExitCode> { None }
    /// Lines appended to `claudio --help`.
    fn help(&self) -> &'static str { "" }
    /// Launch flags it owns on `claudio [flags]` and `claudio --plain [flags] …`
    /// (e.g. `--proxy NAME`); parsed by the core into a `LaunchChoice`.
    fn launch_flags(&self) -> &'static [FlagDecl] { &[] }
    /// The options it adds to the wizard's launch row (e.g. one per proxy profile).
    fn launch_options(&self, ctx: &AddonCtx) -> Vec<LaunchOption> { vec![] }
    /// Its contribution to a claude launch (never a terminal), for the chosen option or the
    /// default. May wait (fetch a config, wait for a host's answer); the core bounds it.
    fn prepare_launch<'a>(&'a self, req: &'a LaunchRequest, ctx: &'a AddonCtx)
        -> BoxFuture<'a, Result<LaunchPart, String>> { ready(Ok(LaunchPart::default())) }
    /// Status segments for a session (TUI status bar, web strip and hover info).
    fn segments(&self, session: &SessionRef, ctx: &AddonCtx) -> Vec<Segment> { vec![] }
    /// Panels it offers (TUI popup bound to a key, web drawer tab or dialog).
    fn panels(&self) -> &'static [PanelDecl] { &[] }
    fn panel<'a>(&'a self, id: &'a str, req: &'a PanelRequest, ctx: &'a AddonCtx)
        -> BoxFuture<'a, Panel>;
    fn panel_action<'a>(&'a self, id: &'a str, action: &'a str, req: &'a PanelRequest,
        ctx: &'a AddonCtx) -> BoxFuture<'a, Result<Notice, String>>;
    /// A checklist the user must review (shown queued, like a confirm), and its answer.
    fn checklists(&self, ctx: &AddonCtx) -> Vec<Checklist> { vec![] }
    fn save_checklist(&self, id: &str, answer: ChecklistAnswer, ctx: &AddonCtx) -> Result<(), String>;
    /// Rows for the extensions screen (on/off toggles, details, an Enter action).
    fn extension_rows(&self, ctx: &AddonCtx) -> Vec<ExtensionRow> { vec![] }
    /// Lifecycle: a host connected or went away, a timer, a config change.
    fn on_event(&self, event: &AddonEvent, ctx: &AddonCtx) {}
    /// Daemon side: ops this addon serves inside a host's daemon.
    fn daemon(&self) -> Option<&dyn DaemonAddon> { None }
}
```

- **`AddonCtx`** is what the front-end offers an addon: `request(host, op, body)` (an `AddonOp` to
  a host's daemon), `hosts()`, `config()` (the addon's own `config.toml` section, read; writes go
  through `config::save_section_line`, the existing single-line editor generalised),
  `notify(text)` and `changed()` (asks the front-end to re-read segments, panels, checklists and
  rows). TUI and web each implement it; addon state (caches, per-host answers) lives inside the
  addon value behind its own lock, so both front-ends run the same code (A5).
- **Data, not widgets.** `Segment { text, tone }`; `Panel { title, pages: [Page { title, blocks:
  [Block] }], actions: [PanelAction { id, label, key, confirm }] }` where `Block` is `Text`,
  `KeyValues`, `Table`, `Gauge { label, used, limit, resets }` or `Bars { label, series }`;
  `Checklist { id, title, intro, items: [{ id, label, detail, on }] }`; `Tone` is `Ok | Warn |
  Hot | Muted | Accent`. The TUI renders them with ratatui (one generic panel view replaces
  `stats_view` + `ui/stats.rs`; one checklist view replaces `plugin_setup`), the web page with
  one generic panel and one checklist component (replacing `stats.js` and `plugin-setup.js`),
  both following DESIGN.md for the web.
- **The TUI stays pure (A6).** `App` gains generic variants only: `Effect::Addon(AddonCall)` and
  `HostEvent::Addon(AddonResult)`, `Modal::Panel` and `Modal::Checklist`; key bindings for panels
  come from `PanelDecl.key` (checked for collisions by `keymap.rs` like every other binding).
  The runner awaits addon futures off the UI loop.

### Launches

`LaunchRequest { host, cwd, kind: Claude, resume, option: Option<LaunchChoice> }` goes to every
addon's `prepare_launch`, in registry order, under one 25 s deadline (today's 5 s config fetch +
20 s manifest wait). Each returns a `LaunchPart { env, env_remove, args: [LaunchArg], instructions,
notices }`. The core's `compose` (moved from `tui/plugins.rs`, the one implementation for TUI, web
and `--plain`) merges them: env in registry order with a later addon's key winning and no key
twice; `args` appended in order (`LaunchArg::Optional { flag, value }` is dropped by the daemon
when the host's claude does not list `flag`, today's `ClaudeFlags` rule made generic);
instructions joined by a blank line and cut at 8158 bytes, as today. A terminal launch skips the
addons entirely. A failure from an addon whose option was explicitly chosen fails the launch with
its message; any other failure becomes a notice and the launch goes on without that part.

`env_diff` splits: the core always removes `claude::SESSION_MARKERS` (daemon); the proxy-specific
removals become the Vocdoni addon's `env_remove`. `SpawnSpec` gains `env_remove: Vec<String>`
(`#[serde(default)]`; an old daemon ignoring it is harmless only because a new client never talks
to an old daemon, since freshness replaces it first; the plan proves this or uses a new op per B1).

The chosen option persists per session as `launch: Option<LaunchChoice>` (`{ addon, option,
flags }`) in `state.json`, `web-state.json` and the daemon journal; the old `proxy`/
`first_party` fields are read into it (B3) and no longer written.

### Daemon ops for addons

One new op pair replaces `Msg::Manifest`/`Msg::Applied`: `Msg::AddonOp { addon, op, body: Value }`
→ `Msg::AddonReply { body: Value }` (or `Error`). The daemon finds the addon in its registry and
calls `DaemonAddon::handle(op, body, DaemonCtx)` on a task (the "reply later" path every slow op
already uses, now one helper). `DaemonCtx` offers `claude_bin()`, `path()`, `claude_flags()`,
`cache_dir(addon)`, the `updating` lock (shared with `claude update`) and `forget_on_update`
(called after a claude install, as `Manifests::forget_failures` is today). `Welcome.features`
lists `addon:<name>` per registered daemon addon. Body size ≤ 512 KiB both ways.

### The Vocdoni addon (`claudio-vocdoni`)

Everything in `src/proxy/`, `src/plugins/`, `daemon/manifest.rs`, `tui/plugins.rs`,
`tui/plugin_setup.rs`, `tui/proxy_state.rs`, `tui/stats_view.rs`, `tui/ui/stats.rs`,
`web/proxy.rs` and `web/plugins.rs` becomes this one addon, with one plugin state machine and one
proxy cache:

- **Commands**: `claudio vocdoni login [--token] | status | use NAME | logout`. `login` with no URL
  signs in to `https://claude.vocdoni.net` (the addon's constant); `--token` pastes a token
  instead. `claudio proxy …` stays as an alias with the same subcommands (existing members'
  muscle memory and docs).
- **Launch**: options are the profiles (`[proxy.profiles.*]`, unchanged in `config.toml`), plus
  first-party per profile; flags `--proxy NAME`, `--no-proxy`, `--first-party`.
  `CLAUDIO_PROXY_URL=<token>@host` keeps working. `prepare_launch` returns the profile env, the
  team plugins' share (`--agents`/`--mcp-config` as optional args, their secrets as env, their
  instructions) and the `env_remove` list.
- **UI**: segments (`⇄ proxy:<name>` / credential / 5h and 7d windows), one panel "Proxy" with
  today's stats pages and the "switch subscription" action (Alt+s by default), one checklist per
  manifest (today's setup screen), extension rows for its plugins.
- **Daemon**: `AddonOp vocdoni/apply` = today's `Manifest` (download, verify, install with
  `claude plugin … --json`, back-off, `Applied`), unchanged in behaviour.
- **Config**: `[proxy]`, `[plugins.<name>]` and `plugins.json` stay where and as they are.

### The API addon (`claudio-api`)

`print/`, `api/`, `cli.rs`, `msglog.rs`. `dispatch` claims argv containing `-p`/`--print` or
`--api`, and the print hook relays (`<Event> <port>`, legacy `__hook <Event>`), and contributes
its `HELP_APPENDIX` through `help()`. `PtySession` stays in this addon (only it uses it).

### The web addon (`claudio-web`)

`src/web/` and `web/` with `build.rs` (the only user of it). It depends on `claudio-core` only.
`dispatch` claims `claudio web …`. It talks to addons only through the API: its proxy and plugin
ops become generic `addons.*` ops (segments, panels, panel actions, checklists, extension rows)
in `docs/web-protocol.md` (doc first, B2); the session DTO's `proxy`/`proxy_known`/`first_party`
become `launch` and `segments`.

Formulas, diagrams and lenses are web-only (G5): the page keeps its KaTeX and Mermaid rendering
and its in-pane formula layer, fed by the daemon's lens events. The `latex` and `diagram`
instructions to claude are contributed by the web addon's `prepare_launch` when enabled
(`[extensions] latex`/`diagram`, defaults as today), so a core-only build asks claude for no tags.

### The plain console TUI (G5)

Removed from `claudio-tui`: `typeset/`, `latex.rs`, `formulas.rs`, `diagram/`, the pane overlay,
the lens and formula state on `SessionView`, `lens_hosts`, the render worker's formula and
external-lens paths, `Effect::RenderLatex`/`RenderExternal`. The reader (Alt+u) stays: it shows
answers as markdown, with formulas and diagram fences as their source text. Leader actions
(search, snippets, external actions) and the extensions screen stay. `rust-latex-parser` leaves
the dependency tree if nothing else uses it. The daemon keeps lens extraction (generic transcript
capture, used by the web UI); `latex_event`/`lens_items_event` merge into one routine.

### Untangling (before the split)

Move the misplaced helpers to where they belong: `claude::latex::sanitize` → `claudio_core::text`;
`paths` stops importing `proto` (the socket file names take `PROTO` from a small `version`
module); `proxy::plugins::{name_ok, key_ok}` → the Vocdoni addon (config's call goes away when
`save_plugin_choices` becomes the generic `save_section_line`); `upgrade::SemVer`, the release
constants and the arch helpers → `release`; `proxy::login::{browser_wanted, open_browser}` →
`claudio_core::app`; `proxy::{api::parse_rfc3339, cmd::fmt_tokens}` → core `text`. Break
`freshness ↔ client` by passing a connect function into freshness.

### Alternatives considered

| Option | Why not |
|---|---|
| Separate executables (`claudio-web`, `claudio-api`) found on `PATH`, git-style | The remote bootstrap uploads one binary; install, `upgrade` and freshness all assume one file. The Vocdoni addon must run inside the TUI, the web server and every daemon, so it would need a process protocol for env, segments and panels anyway. |
| Dynamic plugins (`dlopen`, WASM) | No stable Rust ABI; a static musl binary; a large attack surface for no user need. |
| Keep one crate, enforce boundaries with a script | The compiler is the cheapest boundary checker; a script misses re-exports and macros. Kept only as the interim check during untangling. |
| Addon UI as ratatui/Preact code inside each addon | Each addon would depend on both front-ends, and every addon screen would exist twice. Data plus generic renderers keeps A2 and A5. |
| Extend extension protocol v1 for addons | It is a stdio program protocol; in-process addons need typed, async calls and daemon ops. v1 stays for external extensions. |

## Compatibility and rollout

- **Wire**: `AddonOp`/`AddonReply` are new ops (B1). `Manifest`/`Applied` are removed: a new
  client never talks to an old daemon (freshness replaces an idle older daemon first; a busy one
  is marked outdated, and the client treats its `unsupported op` as "daemon too old", as for
  every newer op today). `SpawnSpec.env_remove` and `launch` are `#[serde(default)]`.
- **Persisted**: `config.toml` sections unchanged; `state.json`/`web-state.json`/journal read old
  `proxy`/`first_party` into `launch`; `plugins.json` unchanged.
- **CLI**: every existing command and flag keeps working (`claudio proxy …`, `--proxy`,
  `--no-proxy`, `--first-party`, `-p`, `--api`, `web`, `--plain`). New: `claudio vocdoni …`.
- **Web protocol**: the page is embedded in the binary, so page and server always ship together;
  the doc records the changed ops and keeps version 1.
- **Rollout**: each phase lands on `refactor/core-addons`, all suites green, then merges to local
  `main`. Push, tag and release wait for the owner. Rollback: revert the merge commit.

### Phases (each its own plan, audited before execution)

| Phase | Delivers | Exit check |
|---|---|---|
| P0 Rules | `ACCEPTANCE.md`, `scripts/check-rules.sh`, size baseline, `make check`, CI job; one `cargo fmt` commit; clippy to zero | `make check` green |
| P1 Plain TUI | G5 | unit, manager, web, node, e2e green; no `typeset`/`diagram` in the TUI |
| P2 Untangle | helpers moved, cycles broken, `app` layer extracted so `web` imports no `tui::` item | `grep crate::tui src/web` empty |
| P3 Workspace | crates core, tui, api, web created by moving files; features; core-only build in CI | A2 and A4 checks green |
| P4 Addon API | `addon/`, launch composer, `AddonOp`, generic panels/segments/checklists in TUI and web | API tests with a test addon |
| P5 Vocdoni addon | proxy + plugins moved into `addons/vocdoni` behind the API; TUI and web proxy code deleted; `claudio vocdoni …` | A1 check green; proxy e2e, manager plugin tests green |
| P6 Docs | `CLAUDE.md` map, README (generic first, addons after), `docs/addons.md` | review |

## Failure modes and security

- **Secrets** (C1): addon env reaches claude only in `SpawnSpec.env`; `LaunchPart`, `AddonOp`
  bodies, `Panel`, `Segment` and `Checklist` never carry a secret value (the Vocdoni addon's
  `apply` body carries secret names, as `Manifest` does). `LaunchPart`'s `Debug` redacts env
  values.
- **Untrusted data**: addon data reaching a front-end is sanitized by the core (the existing
  `sanitize`, caps per field: segment 64 chars, panel ≤ 64 KiB encoded, checklist ≤ 256 items)
  before rendering, so a hostile proxy response cannot inject terminal escapes or HTML.
- **A slow or failing addon**: `prepare_launch` is bounded (25 s total, then the launch goes on
  without the late part unless that addon's option was chosen); panel futures time out at 10 s
  with an error page; `AddonOp` handlers run off the daemon's request loop, one in flight per
  (addon, op) key, bodies ≤ 512 KiB.
- **Core-only build**: no addon means no `AddonOp` handlers; an `AddonOp` to such a daemon gets
  `Error "unknown addon"`, shown as a notice.
- **Partial moves**: every commit builds and passes unit tests, and every phase passes every
  suite before it merges, so a half-done refactor never ships.

## Acceptance criteria

- [ ] AC1 (G1): `make check` runs fmt, clippy `-D warnings`, `scripts/check-rules.sh` (A1, A2, A8,
  D2, D4) and the core-only build; CI runs it on Linux and macOS.
- [ ] AC2 (G2): `cargo test -p claudio-core -p claudio-tui` passes; `cargo build
  --no-default-features` succeeds; the manager integration tests pass on that build.
- [ ] AC3 (G2, A1): `grep -rniE 'vocdoni|claude-proxy|claude\.vocdoni' crates/` finds nothing.
- [ ] AC4 (G3, A2): `cargo metadata` shows no edge core → tui/addons, tui → addons, addon →
  addon (checked by the rules script).
- [ ] AC5 (G4): `web/plugins.rs`, `web/proxy.rs`, `tui/plugins.rs`, `tui/plugin_setup.rs`,
  `tui/stats_view.rs`, `tui/ui/stats.rs` no longer exist; `compose` has one definition.
- [ ] AC6 (G5): the TUI crate has no `typeset`, `diagram` or `latex` module; a manager test
  shows a `<latex>…</latex>` answer drawn as claude printed it; the web e2e formula tests pass.
- [ ] AC7 (G6): a manager test with the fake proxy: `claudio vocdoni login --token`, then a new
  session gets the proxy env, the plugin args and the instructions (today's plugin tests, moved).
- [ ] AC8 (G7): every existing suite passes (unit, manager, web, node, Playwright, and the
  integration tests where gated), with tests moved but never weakened; fixtures of the previous
  release's `state.json`, `web-state.json` and journal load (new tests).
- [ ] AC9: `scripts/size-baseline.txt` shrinks; `run_effect`, `event_loop` and the daemon
  `request` are each under 150 lines.

## Constraints

- Follow `ACCEPTANCE.md`; cite rule numbers in reviews.
- Every commit builds and passes unit tests; each phase passes every suite before it merges.
- Moves are moves: `git mv` where a file moves whole; behaviour changes and moves never share a
  commit.
- Protocol docs first (B2): `docs/web-protocol.md` and the daemon op doc before code.
- Secrets rule C1 and the web perimeter C5 hold throughout.
- No push, tag or release without the owner. Never `--amend`; `git add <exact paths>`.
- Test scratch under `~/.cache/claudio/`, never `/tmp`; never touch the owner's daemon or
  `claudio web`.

## Open questions

None blocking. Recorded decisions the owner may revisit: the addon is named `vocdoni` (the proxy
client is generic claude-proxy, but the owner framed it as the Vocdoni addon); the release binary
keeps all addons; the reader stays in the TUI without formula rendering.
