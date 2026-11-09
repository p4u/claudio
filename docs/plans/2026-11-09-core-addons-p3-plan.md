# Core and addons, P3: the addon API by migration (implementation plan)

Spec: docs/specs/2026-11-09-core-addons-design.md (audited; the "P3 Addon API by migration" row
of the Phases table). Umbrella plan: docs/plans/2026-11-09-core-addons-plan.md.
Class: architectural
Repo/branch: /volumes/repos/claudio → refactor/core-addons (base: 0af5a29, the commit that ends P2)
Written against: 5eca0b5, re-anchored after the pre-flight audit to 0af5a29 (P2 complete, merged
to main). Line numbers are of 0af5a29. P2's destinations are now real: `crate::text`, `crate::release`,
`crate::version`, `crate::app::{state, connections, snippets, extensions, label, browser}`,
`crate::plugins::{manifest, state, choices}`, `src/tui/plugins_glue.rs` (the TUI's `impl App`
plugin glue left by P2 step 8), `proxy/stats.rs` (`Window`). `freshness::ensure_current` takes a
`Client`.

## Pre-flight rulings applied

From `.delivery/2026-11-09-core-addons/preflight-p3.md` (verdict EXECUTE AFTER FIXES). The
controller accepted F1-F23, every D amendment and new spec issues 12-13:

- F1 (A8 gate): the A8 constraint now plans a behaviour-free split commit before each growth
  (P3.2 `proto/spawn.rs`, `daemon/lifecycle.rs`; P3.3 `daemon/command.rs`; P3.4 `addon_runner`,
  `on_connected`, `web/lenses.rs`, `build_state`; P3.5b `tui/proxy_glue.rs`; P3.10/P3.14
  `dispatch_v1`, `web/addons.rs`). New files are sized as directories (`app/addons/`,
  `app/launch/`, `vocdoni/state/`, `vocdoni/panel/`).
- F2: P3.9 keeps `Msg::merge_env` & co. and `envmerge.rs`; P3.10 deletes them.
- F3: the resume retry reads a daemon-held `session_env` map, not the `Handle` (P3.3, spec
  issue 9).
- F4: an un-adopted legacy `proxy: Some(_)` fails the launch, never direct (D4, P3.9, P3.19).
- F5: one process-wide config edit lock for every writer (P3.4, P3.13, D10).
- F6: `src/addon/testing.rs` → `src/addon/test_support.rs`; fixture modules are named
  `test_support.rs` (Global Constraints).
- F7: P3.7 gets its docs commit, moves only :2231-3095 (the optional-flag test stays in core),
  builds `Addons::new` correctly, and `serve` moves to `vocdoni/test_support.rs` (P3.8 cites it).
- F8: `remote/diag.rs` keeps `__remote-manifest` (AddonOp) until P3.17 makes it a Vocdoni word;
  listed among the A3 exceptions and in P3.13's grep.
- F9: no static: `Addon::as_any` (transitional, required method) for `Bridge::of`; removed in
  P3.13.
- F10: the old-daemon switch also ignores `optional`; `daemon_request` matches `req`; Merge asks
  for one real v0.8.0 mixed-version check each way.
- F11: `AddonResult::Options` carries launch options to `App` (P3.9).
- F12: tasks reordered and renumbered: P3.17 is now the api/web command addons (moves),
  P3.18 the web addon's lenses (under `src/addons/web/`), P3.19 persistence.
- F13: `RuntimeOut` full-queue policy (P3.4, D13); TUI panel 10 s / action 15 s with `gen` check
  (P3.12).
- F14: P3.5, P3.8, P3.9, P3.12, P3.14 and P3.15 split into a/b(/c) sub-tasks, each ending green.
- F15: P3.2/P3.4 parallel-merge instructions (task table note).
- F16: `ClaudeFlags` is `Clone`, `command()` takes `&ClaudeFlags` (P3.2, D6).
- F17: spec issue 12 (login URL rule).
- F18: spec issue 13 (`LaunchRequest`/`resolve`/`select`/snapshot deltas; D3 amended).
- F19: the TUI's `enum Launch` becomes `enum Entry` (P3.4 commit 0).
- F20: the web's runtime starts with `watch_config: false` until P3.13 (D15).
- F21: replaced unit tests are named in commit messages (Global Constraints; P3.9, P3.11,
  P3.12).
- F22: base re-anchored to 0af5a29; baseline 291/305/171; `web/plugins.rs` 880 production lines.
- F23: an `AddonOp` body is never logged (P3.2, docs/daemon-protocol.md).
- Audit §4 and §6: `docs/extensions.md` added to P3.18; the Merge section gains the rules totals,
  the compat grep, the vendor/import-map and leaked-daemon checks, and the no-attribution grep.
- Not applied as written: F9's "default method" (`as_any` cannot have a default body for a
  `?Sized` `Self`), so it is a required method that each addon implements as `{ self }`.

**End state of P3.** Still one crate. `src/addon/` is the addon API (`Addon` and its capabilities
`Commands`, `Launch`, `Ui`, `DaemonOps`; `AddonCtx`, `DaemonCtx`; the data types; the registry
`Addons`; the core's sanitising and caps). `src/app/` holds the addon runtime (`AddonRuntime`, the
one `AddonCtx` implementation, used by the TUI runner and the web hub), the generic config editor
and the launch composer (resolve → prepare → review → compose), used by the TUI, `--plain` and the
web. The addons live in `src/addons/{vocdoni,api,web}/`. The Vocdoni addon holds everything that
was `src/proxy/`, `src/plugins/`, `daemon/manifest.rs`, the TUI's plugin glue, setup screen, proxy
state and stats popup, and the web's `proxy.rs`/`plugins.rs`. It keeps one plugin state machine and
one proxy cache. The daemon runs addon ops (`Msg::AddonOp`) and lists `addon:<name>` in
`Welcome.features`. Spawns carry `env_remove`. Old clients get the compatibility scrub. The TUI and
the page render addon data (segments, panels, checklists, extension rows) through generic
renderers. The web speaks protocol v2 (`addons.*`). `src/main.rs` is wiring: it builds one
`Arc<Addons>` and hands it to every entry point. `src/tui` and `src/web` name no `proxy` or
`plugins` module. P4 then moves these directories into crates.

```
src/addon/          the API: mod.rs (Addon, Commands, AddonCtx, events, errors), launch.rs, ui.rs,
                    daemon.rs, clean.rs, registry.rs, test_support.rs (cfg(test) TestAddon)
src/app/            + addons/ (AddonRuntime: mod, ctx, events), config_edit.rs, launch/ (mod, compose, review)
src/addons/mod.rs   all(): the addons this build registers
src/addons/vocdoni/ mod.rs, proxy/, plugins/, state/, runner.rs, launch.rs, ui.rs, panel/,
                    rows.rs, daemon/ (was daemon/manifest.rs), cmd.rs
src/addons/api/     print/, api/, cli.rs, msglog.rs, mod.rs (Commands)
src/addons/web/     was src/web (+ addon.rs: Commands, Launch lenses, Ui rows); web/ stays at the root
src/daemon/         + addon_ops.rs (DaemonCtx impl, AddonOp dispatch)
src/tui/            + addon_runner.rs (I/O edge for launches and addon calls), checklist.rs,
                    panel_view.rs, ui/checklist.rs, ui/panel.rs
docs/               + addons.md (API), daemon-protocol.md (new), web-protocol.md v2
```

## Global Constraints

From the umbrella plan (unchanged):
- Follow ACCEPTANCE.md; cite rule numbers in reports and reviews.
- Commit convention: conventional commits `type(scope): lowercase summary`; one logical change
  per commit; NO AI attribution, no Co-Authored-By or similar trailers.
- Never push, tag or switch branches; commit with explicit paths (`git commit -m … -- <paths>`,
  `git add <exact new paths>`); never `git add -A`, never `--amend`, never `git stash`.
- Every commit builds (`cargo build --locked`) and passes `cargo test --locked --bin claudio`.
- Moves are moves: `git mv` for whole files; a move and a behaviour change never share a commit.
  A move commit may rename `scripts/size-baseline.txt` entries (same counts) and nothing else.
- Never weaken a test to get to green (E2). A deliberately changed contract replaces its
  assertions with tests of the new contract, said in the commit message.
- Toolchain: rustup's (`TC=$(dirname $(rustup which cargo))`). Clippy only as `PATH=$TC:$PATH
  CARGO_TARGET_DIR=$HOME/.cache/claudio/clippy-target $TC/cargo clippy --locked --all-targets
  --all-features -- -D warnings`. `make check` runs everything.
- Build and test scratch under `~/.cache/claudio/`, never `/tmp`. Kill every process you start.
  Never touch the owner's own `claudio` daemon or `claudio web` (tests use isolated env).
- Secrets (C1) and the web perimeter (C5) hold throughout. Do not edit web/vendor/**,
  web/fonts/** or the import-map element.
- Web UI changes follow DESIGN.md (tokens in `web/styles/tokens.css` only).

Added for P3:
- **B2, docs first.** Each of these lands in its own commit before any code that implements it:
  `docs/addons.md` (the API, P3.1, extended before each later hook), `docs/daemon-protocol.md`
  (new file; the daemon protocol's addon ops, `env_remove`, `optional`, features, P3.2), and
  `docs/web-protocol.md` v2 (P3.14).
- **Green at every task end.** Every commit builds and passes the unit tests. At each task's end
  the suites below pass, including the **proxy/plugin regression set**: manager
  `test_plain_keeps_proxy_stats_and_reset`, `test_plain_injects_proxy_env`,
  `test_plain_no_proxy_keeps_api_key`, `test_plain_first_party_flag`,
  `test_plugin_manifest_setup_then_sessions_get_what_was_kept`,
  `test_extensions_screen_turns_extensions_off_and_saves_it` (the gated
  `test_proxy_env_injection`/`test_proxy_real_claude` must compile); web
  `web_spawn_keeps_proxy_and_plugin_secrets_out_of_the_browser`,
  `web_respawn_without_a_known_proxy_asks`, `web_plugin_without_choices_is_left_out_until_saved`,
  `web_proxy_stats_render_the_fixture`, `web_extension_toggle_edits_config_and_is_reread`;
  Playwright `web/e2e/tests/proxy.spec.mjs`; the daemon's manifest tests (`src/daemon/tests.rs`
  :2453-3116, moved in P3.7). A test is moved or adapted only where the spec changes the contract
  (generic renderers, protocol v2, review timing), and the commit message says which assertion
  changed and why.
- **A3 inside one crate.** `src/addons/**` imports only `crate::addon`, the documented core
  services (`crate::{proto, paths, text, version, release, client::Client, claude::{binary,
  SESSION_MARKERS}, app::label, app::browser}`) and its own modules. Core code (everything
  outside `src/addons/`) imports nothing from `crate::addons` except `main.rs` calling
  `crate::addons::all()`. The exceptions, all transitional: the bridge
  `crate::addons::vocdoni::bridge` (P3.8-P3.13, deleted in P3.13), the web's v1 adapter
  `src/web/plugins.rs` (P3.10-P3.16), and the diag-only `src/remote/diag.rs::remote_manifest_cmd`
  naming `vocdoni`/`apply` (P3.7-P3.17). Core code never branches
  on an addon's name: at P3's end, `grep -rn '"vocdoni"' src --include=*.rs | grep -v
  '^src/addons/'` is empty.
- **A8, and the gate** (pre-flight F1). `scripts/check_rules.py` fails on any net growth of a
  listed file or function and on any unlisted file over 800 / function over 80 production lines.
  So a task that adds lines to a listed file pays for them in the same task, before the addition,
  with a behaviour-free split commit (`refactor(<scope>): split <what> out of <file>`): at least
  as many lines move to a new module as the task will add. `run_effect`, `event_loop` and
  `daemon::server::request` (baseline 291/305/171) never grow and should end under 80 (AC9).
  Planned splits:
  - P3.2 (before the proto change): `SpawnSpec`, `RespawnSpec`, `SessionExtensions`, `LensSpec`,
    their caps and `Debug` impls (proto.rs:400-720) → `src/proto/spawn.rs`, re-exported from
    `proto` so no caller changes; `Daemon::respawn`, `retry_spawn_fresh`, `close_exited` and
    `without_conversation` (one `impl Daemon` block) → `src/daemon/lifecycle.rs`.
  - P3.3: `command`, `apply_env`, `shell_command`, `clean_title` (session.rs:331-420) →
    `src/daemon/command.rs` (P3.3 rewrites them anyway).
  - P3.4: `tui/mod.rs`: the runtime is built and fed in `src/tui/addon_runner.rs` (`start`,
    `on_host_up`, `on_host_down`, `on_session_event`, `drain`); every call from `main`/`event_loop`
    is one line, and the task first moves the `Connected` arm's body (:739-763) into
    `fn on_connected(…)` and the four proxy `HostEvent` arms (:816-829) into
    `App::on_proxy_event`, so both functions end smaller. `web/mod.rs::serve`: the hub/ext/runtime
    construction goes to `fn build_state(…)`. `web/hub.rs`: the runtime feeding is one line per
    `HubEvent` emission (:1767, :1866, :1956, :2141), paid for by moving the lens relay
    (`hub.rs:484-520`, `lens_events`) to `src/web/lenses.rs`.
  - P3.5 (P3.5b): `tui/app.rs`: before adding `Effect::Launch`/`HostEvent::Launched`, move the
    proxy methods (`on_proxy_config`, `on_proxy_stats`, `proxy_config_cached`,
    `maybe_fetch_proxy_config`, `refresh_session_cred`, `on_session_credential`,
    `on_switch_result`, app.rs:525-655) into `src/tui/proxy_glue.rs` as an `impl App` block
    (P3.8/P3.12 delete them there). `tui/mod.rs`: the `SpawnWithProxy` arm body (:1098-1178, 80
    lines) moves whole into `addon_runner::spawn_with_proxy`, so `run_effect` shrinks.
    `tui/sessions.rs`: `send_with_proxy` + `proxy_env_for` move to `proxy_glue.rs`. `Launched` is
    handled in `app.rs` (`on_launched`), not in `interaction.rs::on_reply`.
  - P3.10/P3.14: `ws.rs::dispatch` routes `addons.*` with one guard arm (`op if
    op.starts_with("addons.") => addons::dispatch(…)`) and, in the same commit, moves the
    `proxy.*` and `plugins.*` arms (:758-830) into `fn dispatch_v1(…)` in `src/web/plugins.rs`
    (deleted in P3.16). `hub.rs` additions (segments cache, `addons.changed`) go to
    `src/web/addons.rs`, called from one line.
  - New files are sized at design time: `src/app/addons/{mod,ctx,events}.rs`,
    `src/app/launch/{mod,compose,review}.rs`, `src/addons/vocdoni/state/{mod,lists,applies}.rs`,
    `src/addons/vocdoni/panel/{mod,pages,blocks}.rs` (`ui/stats.rs` has 999 production lines and
    cannot become one file), `src/tui/ui/panel.rs` ≤ 800. New functions (`AddonRuntime::start`,
    `launch::run`, `compose`, `Vocdoni::prepare`, `panel`) are written as ≤ 80-line drivers over
    helpers.
  - `scripts/check_rules.py --write-baseline` is run only in a move/split commit and its printed
    totals must not grow; a task report pastes the "rules:" line of `make check`.
- **Test fixture modules** (pre-flight F6) are named `test_support.rs` (or live under a `tests/`
  directory, or are `tests.rs`), because `check_rules.py::is_test_file` measures any other
  `#[cfg(test)]` file as production code.
- **Replaced unit tests** (pre-flight F21) are named in the commit message with the new test
  that covers the same contract (E2).
- **C2 for addon output.** Every string an addon hands a front-end (segment, panel, checklist,
  extension row, launch option, notice) goes through `crate::addon::clean` before a front-end
  sees it, in the runtime and never in the addon.
- **Transitional allow.** `src/addon/mod.rs` carries one `#![allow(dead_code)]` with a comment
  pointing to this plan, because the API lands before its consumers. P3.20 removes it, and the
  tree must then build clean.
- Model roles: **strong** for anything touching env, secrets, the compatibility scrub, daemon ops,
  launch routing or web ops; **standard** otherwise (each task says which).

Suites used below:
- S-unit: `cargo build --locked && cargo test --locked --bin claudio`
- S-manager: `cargo test --locked --test manager -- --test-threads=1`
- S-web: `cargo test --locked --test web -- --test-threads=1`
- S-node: `node --test web/tests/*.test.mjs`
- S-e2e: `make test-web` (node, then Playwright on a debug build)
- S-shots: `cargo test --locked --test screenshots` (gated: must compile and skip)
- S-remote: `cargo test --locked --test remote -- --test-threads=1` (t1/t7/t8 run; t2-t6, t9
  need `CLAUDIO_SSH_TEST_HOST`, e.g. `z6`)
- S-check: `make check`

## Review Focus

- **C1, secrets in launch data.** `LaunchPart.env` and the composed env must never reach `App`,
  a DTO, a log, a notice or `Debug`. Today's redaction to copy: `src/proto.rs:691-718` (`SpawnSpec`
  and `RespawnSpec` `Debug` print env keys only). The plugin `Composed`/`PluginPart` `Debug` is in
  `src/plugins/state.rs` after P2 (was `src/tui/plugins.rs:134-158`). `env_remove` carries names
  only. `AddonOp` bodies carry secret names, never values (as `Msg::Manifest` does today,
  `src/proto.rs:228-242`).
- **The compatibility scrub.** Today's rule is `src/proxy/env.rs:135-144` (`env_diff`), applied
  in `src/daemon/session.rs:389-401` (`apply_env`) for every Spawn/Respawn. With `env_remove:
  None` the new daemon must do exactly that. With `Some(list)` it removes exactly
  `claude::SESSION_MARKERS` plus `list`. A shell always gets the markers only.
- **Routing::Required never falls back to direct.** Today a missing profile fails the launch
  (`src/proxy/resolve.rs:111-131`) and an unreachable config only falls back to the built-in models
  (`src/tui/mod.rs:1098-1178`, `src/web/spawn.rs:169-203`). The composer must fail a launch whose
  Required addon errs or times out, and never send that Spawn. A pre-existing hole is that
  `retry_spawn_fresh` re-spawns with an empty env (`src/daemon/mod.rs:714-760`). P3.3 fixes it.
- **The NeedsReview clock.** The 25 s budget (today 5 s config at `src/tui/mod.rs:1121` plus 20 s
  `MANIFEST_WAIT` at `:1180`) stops while a checklist is open and resumes with what is left. A
  Respawn/Recover never waits for review (today's reset waits for no manifest,
  `src/web/spawn.rs:316-321`).
- **AddonOp caps.** Bodies are at most 512 KiB encoded, checked by `proto` on both sides; frames
  stay ≤ `MAX_FRAME` (`src/proto.rs:25`). The handler runs off the request loop
  (`src/daemon/server.rs:419-588`), with at most 8 in flight per addon and a timeout.
- **Sanitising addon strings.** `crate::text::sanitize` plus control stripping and the caps
  (segment 64 chars, panel ≤ 64 KiB encoded, checklist ≤ 256 items) in `src/addon/clean.rs`. The
  page renders them as text nodes only (docs/web-protocol.md §7).
- **Web perimeter.** The new `addons.*` ops sit behind the same guard (`src/web/auth.rs:362`)
  and dispatch (`src/web/ws.rs:476`). Args are `deny_unknown_fields`. Session-bound actions
  resolve profile and conversation from server state, never from browser input. Hello moves to
  `v: 2` (`src/web/ws.rs:128`).

## Decisions this plan makes (the spec leaves them open)

- **D1** `on_event` is a default method on `Addon` (`fn on_event(&self, &AddonEvent, &Arc<dyn
  AddonCtx>)`). The spec names the events but no trait.
- **D2** Each addon gets its own `AddonCtx` (`AddonRuntime::ctx(name)`). So `request`, `config`,
  `edit_config` and `host_has(host)` know the addon. The spec writes `host_has(addon)`.
- **D3** The persisted choice is `LaunchChoice { addon, option, flags }`. The wizard/flag input is
  `LaunchSelect { pick: Default | Direct | Option{addon, option}, flags }`. `Routing::Required`
  carries the effective `LaunchChoice`. At most one addon may answer `Required`; two is a launch
  error. Amended (pre-flight): `LaunchRequest` also carries this addon's `routing` and
  `deferred` ids, `resolve` returns `Result`, and `select` parses the addon's own flags (spec
  issue 13).
- **D4** Legacy persistence goes through `Launch::adopt_legacy(&LegacyChoice)` and
  `Launch::legacy(&LaunchChoice)` hooks, so the core never names `vocdoni` (A3). The spec says
  "derived from launch when its addon is vocdoni". Amended (F4): a `proxy: Some(_)` that no addon
  adopts fails the launch, never direct.
- **D5** Wizard checkboxes are `FlagDecl.label` (`--first-party` → "First-party session").
  `addons.list` adds `launch_flags`.
- **D6** Optional args are a new wire field, `SpawnSpec.optional: Vec<String>`, journaled. The
  daemon drops a flag listed there (plus the legacy three: `--plugin-dir`, `--agents`,
  `--mcp-config`) when its claude's `--help` does not list it. `ClaudeFlags` becomes generic
  (`knows(flag)`) and moves to `crate::addon`. An old daemon ignores `optional` but drops its own
  fixed three, which covers every flag the Vocdoni addon marks optional. Amended: the old-daemon
  test switch ignores `optional` too (F10); `ClaudeFlags` is `Clone`, not `Copy` (F16).
- **D7** `DaemonCtx::cache_dir()` is `paths::cache_dir().join(<addon>)`, so the Vocdoni manifest
  cache moves from `<cache>/plugins/` to `<cache>/vocdoni/`. It is re-downloaded once. t9 seeds
  the new path.
- **D8** First-sight review: `AddonCtx::offer_checklist(id)` asks a front-end to show a checklist
  when idle (the TUI queues it like today's first-sight setup; the web ignores offers, as today).
  Launch-time review is `LaunchError::NeedsReview`. `ChecklistAnswer::{Save{revision, off},
  Defer}`; Skip (this launch only) and Esc (cancel the launch) never reach the addon.
- **D9** `ChecklistItem` gains `section: Option<String>` (front-ends print a heading when it
  changes). `Segment` has no line field. The TUI puts priorities 0-99 on status line 1 (left to
  right) and 100-255 on line 2 (dropped lowest-first when narrow). docs/addons.md documents the
  bands.
- **D10** Extension-row toggles are written by the core to `[extensions] <row> = on` (its own
  table) and read by addons through `AddonCtx::extension_on(name)`. Addons never write
  `[extensions]` (A3). Amended (F5): every writer of config.toml goes through one process-wide
  edit lock.
- **D11** Web review: `session.spawn` answers `{"review": {"addon","id","revision"}}` instead of
  `{"sid"}` when a launch needs review. The page shows `addons.checklist`, then re-sends
  `session.spawn` after `addons.checklist.save` (Enter), or with `defer: [{"addon","id"}]` (Skip),
  or drops it (Esc). The server keeps no parked launch for the web.
- **D12** Session DTO v2: `launch: {"known": bool, "choice": LaunchChoice|null}` (choice null =
  direct; known false = the old `proxy_known: false`) and `segments: [Segment+addon]`.
- **D13** Timers and bounds: runtime `Tick` every 1 s; web config poll 5 s (as today), TUI none
  (as today); `ctx.spawn` ≤ 64 live tasks per addon, each under a 300 s timeout; `RuntimeOut`
  queue 256 with per-addon `changed` coalescing; `ctx.request` timeout 170 s; daemon
  `DaemonOps::handle` timeout 150 s; panel 10 s, action 15 s, row action 15 s; launch prepare
  budget 25 s; Vocdoni list refresh 15 min (`REFRESH_SECS`), re-ask 30 s (`REASK_EVERY`),
  credential poll 30 s for focused sessions (`SESSION_CRED_POLL_SECS`), config cache 5 min.
  Amended (F13): `RuntimeOut` is sent with `try_send` (a full queue drops a `Changed`, which the
  dirty flag re-sends, and drops the oldest `Notice`/`Offer` with a warning); the TUI's panel
  call has the same 10 s timeout and error page as the web's, actions 15 s, stale `gen` dropped.
- **D14** Compatibility removals are dated: the compatibility scrub, the legacy optional-flag
  three, the refused-`Manifest` test, the legacy `proxy`/`first_party` writes and the
  `effective_extensions` latex default for `extensions: None` go in the first release after the
  one that ships P3 (planned v0.10.0), not before 2027-01-31. Each carries a `// compat(v0.10)`
  comment so `grep -rn 'compat(v0.10)' src` finds them all.
- **D15** Web config watching (5 s mtime poll) moves into `AddonRuntime` behind
  `RuntimeOptions { watch_config: bool }`. Amended (F20): the web passes `false` until P3.13
  (its own `ext.rs` poll calls `config_changed()`), then `true` with `ext.rs`'s poll deleted; the
  TUI passes `false`.

## Task order and parallelism

| Task | Model | Depends on | Main files | Parallel with |
|---|---|---|---|---|
| P3.1 API types, registry, docs/addons.md | strong | P2 | src/addon/**, docs/addons.md, src/main.rs | — |
| P3.2 daemon: AddonOp, DaemonCtx, reply-later, features | strong | P3.1 | docs/daemon-protocol.md, src/proto.rs (+ proto/spawn.rs split), src/daemon/{mod,server,update,host,addon_ops,lifecycle}.rs | P3.4 |
| P3.3 env_remove, compatibility scrub, optional args, retry env | strong | P3.2 | src/proto/spawn.rs, src/daemon/{command,session,mod,journal,server}.rs, tests/common, tests/manager.rs | P3.4 |
| P3.4 addon runtime and config editor | strong | P3.1 | src/app/addons/, src/app/config_edit.rs, src/config.rs, src/tui/{mod,addon_runner}.rs, src/web/{mod,hub,ext,lenses}.rs | P3.2, P3.3 |
| P3.5a launch composer | strong | P3.3, P3.4 | src/app/launch/ | — |
| P3.5b TUI and --plain direct launches | strong | P3.5a | src/tui/{proxy_glue,addon_runner,app,mod,sessions,plain}.rs | — |
| P3.5c web direct launches | strong | P3.5b | src/web/spawn.rs | — |
| P3.6 move proxy/ and plugins/ into addons/vocdoni | standard | P3.5c | git mv only | — |
| P3.7 Vocdoni daemon slice (`vocdoni/apply`) | strong | P3.6 | docs, src/addons/vocdoni/{daemon/**,test_support.rs}, src/daemon/**, proto, plugins glue call sites, remote/diag.rs | — |
| P3.8a Vocdoni `State` port (pure) | strong | P3.7 | src/addons/vocdoni/state/ | — |
| P3.8b runner, bridge, `[plugins]` out of config | strong | P3.8a | src/addons/vocdoni/{runner,bridge}.rs, src/addon/mod.rs, src/config.rs | — |
| P3.8c TUI reads the bridge, glue deleted | strong | P3.8b | src/tui/** | — |
| P3.9a `impl Launch for Vocdoni` | strong | P3.8c | src/addons/vocdoni/launch.rs | — |
| P3.9b TUI, --plain, flags through it | strong | P3.9a | src/tui/**, src/main.rs, src/plain.rs, tests/manager.rs | — |
| P3.10 web onto the addon (v1 protocol kept) | strong | P3.9b | src/web/{spawn,plugins,proxy,hub,ext,mod,ws}.rs, src/envmerge.rs, src/proto | — |
| P3.11 review slice: checklists | strong | P3.10 | src/addons/vocdoni/ui.rs, src/tui/{checklist,plugin_setup,…}, src/app/launch/ | — |
| P3.12a Vocdoni segments, panel, action | strong | P3.11 | src/addons/vocdoni/{ui.rs,panel/,state/} | — |
| P3.12b generic TUI panel view, keymap | strong | P3.12a | src/tui/{panel_view,ui/panel,keymap,interaction,addon_runner}.rs | — |
| P3.12c status segments; stats popup deleted | strong | P3.12b | src/tui/{ui/status,app,…}.rs, deletions | — |
| P3.13 extension rows; bridge and `as_any` removed | standard | P3.12c | src/addons/vocdoni/rows.rs, src/app/extensions, src/tui/extensions_view.rs, src/web/ext.rs | — |
| P3.14a web protocol v2 docs | strong | P3.13 | docs/web-protocol.md | — |
| P3.14b `addons.*` read ops, segments, DTO | strong | P3.14a | src/web/{addons,hub,ws,dto}.rs, tests/web.rs | — |
| P3.14c launch/review on spawn, write ops | strong | P3.14b | src/web/{addons,spawn,ws}.rs, tests/web.rs | — |
| P3.15a page components | standard | P3.14c | web/app/{launch.js,ui/panel.js,ui/checklist.js,ui/segments.js}, web/tests/** | — |
| P3.15b wizard, session actions, topbar on them | standard | P3.15a | web/app/{wizard.js,model.js,ui/*} | — |
| P3.15c deletions, proxy.spec.mjs | standard | P3.15b | web/app/ui/{stats,plugin-setup}.js, spawn-hooks.js, web/e2e | — |
| P3.16 web v2: drop proxy.*/plugins.*, v=2 | strong | P3.15c | src/web/**, web/app/proto.js, tests/web.rs | — |
| P3.17 api and web as command addons; main is wiring | standard | P3.16 | git mv src/{print,api,cli.rs,msglog.rs,web}, src/addons/{api,web}, src/main.rs, src/remote/diag.rs | — |
| P3.18 web addon: latex/mermaid lenses and rows | standard | P3.17 | src/addons/web/addon.rs, src/app/extensions, src/daemon/mod.rs, docs/extensions.md, tests | — |
| P3.19 persistence: `launch` with legacy fields | standard | P3.18 | src/app/state.rs, src/addons/web/state.rs, tests/fixtures | — |
| P3.20 `claudio vocdoni`, proxy alias, docs, exit checks | standard | P3.19 | src/addons/vocdoni/cmd.rs, README, CLAUDE.md, docs/ | — |

P3.2/P3.3 and P3.4 touch mostly disjoint files (daemon and proto vs app and front-end wiring) and
may run in parallel worktrees. Rebase-free: merge P3.4's branch after P3.3. Both touch
`src/main.rs::main` and `scripts/size-baseline.txt`; the merger resolves by hand, runs `python3
scripts/check_rules.py` (no `--write-baseline`) and S-unit before continuing (F15). Everything
else is sequential: each slice deletes code the next one would otherwise have to port. P3.17 runs
before the web addon's lenses (P3.18) so `WebAddon` is born under `src/addons/web/` (F12).

---

## P3.1: the addon API, its registry and docs/addons.md (spec "The addon API"; G3, A3, A9, C2)

Model: strong   Files: docs/addons.md (new), src/addon/{mod,launch,ui,daemon,clean,registry,
test_support}.rs (new), src/main.rs (`mod addon;`, build `Arc<Addons>` empty), src/addons/mod.rs (new,
`all()` returns `Vec::new()`)

Goal: the whole API surface, documented and tested with a test addon, with no consumer yet.

Commit 1 (docs, B2): `docs/addons.md`. It covers the capability model, every type below with its
caps, the execution model (pure state plus runner, `AddonCtx`, events and their order, the
bounds of D13), launches (the four steps, the 25 s budget, Required vs Optional, NeedsReview and
the offer, compose rules), data-not-widgets with sanitising, the segment priority bands (D9),
daemon ops and the mixed-version rule, config sections (A3) and reserved words. It is ≤ 400
lines; P5 polishes it.

Commit 2 (code). Complete definitions (doc comments abbreviated here; every `pub` item gets one,
D6):

```rust
// src/addon/mod.rs
//! The addon API (docs/addons.md).
// P3: the API lands before its consumers (docs/plans/2026-11-09-core-addons-p3-plan.md);
// removed in P3.20.
#![allow(dead_code)]
mod clean; mod daemon; mod launch; mod registry; mod ui;
#[cfg(test)] pub(crate) mod test_support;
pub use clean::*; pub use daemon::*; pub use launch::*; pub use registry::*; pub use ui::*;

use std::{future::Future, pin::Pin, process::ExitCode, sync::Arc};
use serde_json::Value;
use crate::proto::{SessionId, SessionKind};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
/// A config edit: gets the addon's section, changes it.
pub type ConfigEdit = Box<dyn FnOnce(&mut toml::Table) + Send>;

/// `[a-z][a-z0-9-]{0,31}`.
pub fn name_ok(name: &str) -> bool {
    let mut c = name.chars();
    name.len() <= 32
        && c.next().is_some_and(|c| c.is_ascii_lowercase())
        && c.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}
/// The `Welcome.features` entry of a daemon-side addon.
pub fn feature(addon: &str) -> String { format!("addon:{addon}") }

pub trait Addon: Send + Sync + 'static {
    fn name(&self) -> &'static str;
    fn config_sections(&self) -> &'static [&'static str] { &[] }
    fn commands(&self) -> Option<&dyn Commands> { None }
    fn launch(&self) -> Option<&dyn Launch> { None }
    fn ui(&self) -> Option<&dyn Ui> { None }
    fn daemon(&self) -> Option<&dyn DaemonOps> { None }
    /// A front-end event (D1). Must not block: spawn work with `ctx.spawn`.
    fn on_event(&self, event: &AddonEvent, ctx: &Arc<dyn AddonCtx>) { let _ = (event, ctx); }
}

pub trait Commands: Send + Sync {
    /// Checked before anything else (print's hook relays).
    fn is_relay(&self, argv: &[String]) -> bool { let _ = argv; false }
    /// After the core's own words, in registry order; `None`: not mine.
    fn dispatch(&self, argv: &[String], addons: &Arc<Addons>) -> Option<ExitCode>;
    /// Appended to `claudio --help`.
    fn help(&self) -> &'static str { "" }
}

pub trait AddonCtx: Send + Sync {
    fn addon(&self) -> &'static str;
    /// A task on the front-end's runtime: ≤ 64 live per addon, 300 s each, aborted at shutdown.
    fn spawn(&self, task: BoxFuture<'static, ()>);
    /// The one shared HTTP client (cheap clone).
    fn http(&self) -> reqwest::Client;
    /// `AddonOp { addon: self.addon(), op, body }` to `host`'s daemon; `Missing` without
    /// `addon:<name>` in its features; bodies ≤ 512 KiB both ways; 170 s.
    fn request(&self, host: &str, op: &str, body: Value) -> BoxFuture<'static, Result<Value, AddonError>>;
    /// Whether `host` is connected and its daemon lists `addon:<self>`.
    fn host_has(&self, host: &str) -> bool;
    fn sessions(&self) -> Vec<SessionSnap>;
    fn hosts(&self) -> Vec<HostSnap>;
    /// A declared section of config.toml (empty for an undeclared one).
    fn config(&self, section: &str) -> toml::Table;
    /// Serialised, atomic, minimal edit of a declared section (`app::config_edit`).
    fn edit_config(&self, section: &str, edit: ConfigEdit) -> BoxFuture<'static, Result<(), String>>;
    /// `[extensions] <name>` as config.toml has it (D10).
    fn extension_on(&self, name: &str) -> Option<bool>;
    fn notify(&self, text: String);
    /// Segments, rows, options or an open panel changed: front-ends re-read them.
    fn changed(&self);
    /// Ask front-ends to show checklist `id` when idle (D8).
    fn offer_checklist(&self, id: String);
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionSnap {
    pub sid: SessionId, pub host: String, pub kind: SessionKind, pub cwd: String,
    pub label: String, pub conversation: Option<String>,
    pub launch: Option<LaunchChoice>,
    /// The TUI's active session, or one a browser tab has attached.
    pub focused: bool,
}
#[derive(Debug, Clone, PartialEq)]
pub struct HostSnap { pub name: String, pub connected: bool, pub features: Vec<String>, pub claude: bool }

#[derive(Debug, Clone, PartialEq)]
pub enum AddonEvent {
    /// First, once the snapshots are filled.
    Started,
    Tick,
    HostUp { host: String, features: Vec<String> },
    HostDown { host: String },
    SessionAdded { sid: SessionId, conversation: Option<String> },
    SessionRemoved { sid: SessionId },
    /// The conversation id changed (`/clear`), or focus moved.
    SessionChanged { sid: SessionId, conversation: Option<String> },
    ClaudeInstalled { host: String },
    ConfigChanged,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum AddonError {
    #[error("{0}: not connected")] NotConnected(String),
    #[error("daemon on {0} is older: run `claudio daemon restart`")] Missing(String),
    #[error("message over 512 KiB")] TooLarge,
    #[error("timed out")] Timeout,
    #[error("{0}")] Daemon(String),
}
```

```rust
// src/addon/launch.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlagDecl {
    /// `--[a-z][a-z0-9-]*`.
    pub name: &'static str,
    /// The value's placeholder (`NAME`), or `None` for a switch.
    pub value: Option<&'static str>,
    pub help: &'static str,
    /// Shown as a wizard checkbox when set (D5); only switches.
    pub label: Option<&'static str>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlagArg { pub name: String, pub value: Option<String> }

/// What a session was launched with, persisted (state.json, web-state.json).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LaunchChoice { pub addon: String, pub option: String, #[serde(default)] pub flags: Vec<String> }

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "pick", rename_all = "snake_case")]
pub enum Pick { #[default] Default, Direct, Option { addon: String, option: String } }

/// What the member asked for: from flags, the wizard or a session's persisted choice.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LaunchSelect { #[serde(flatten)] pub pick: Pick, #[serde(default)] pub flags: Vec<String> }

impl LaunchSelect {
    /// A persisted choice as a select (`None` = direct).
    pub fn of(choice: Option<&LaunchChoice>) -> LaunchSelect;
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct LaunchOption { pub id: String, pub label: String, pub group: String, pub default: bool, pub detail: Option<String> }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Routing { Required(LaunchChoice), Optional, Off }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchOp { New, Resume, Respawn, Recover }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frontend { Tui, Plain, Web }

#[derive(Debug, Clone, PartialEq)]
pub struct LaunchRequest {
    pub op: LaunchOp, pub sid: SessionId, pub host: String, pub cwd: String,
    /// The launch's effective choice (the Required addon's), `None` = direct.
    pub choice: Option<LaunchChoice>,
    /// This addon's routing.
    pub routing: Routing,
    pub frontend: Frontend,
    /// This addon's checklist ids skipped for this launch.
    pub deferred: Vec<String>,
}

#[derive(Clone, PartialEq, Eq)]
pub enum LaunchArg { Plain(String), Optional { flag: String, value: String } }

/// An addon's share of a launch. `Debug` prints env keys only (C1).
#[derive(Clone, Default, PartialEq)]
pub struct LaunchPart {
    pub env: Vec<(String, String)>,
    pub env_remove: Vec<String>,
    pub args: Vec<LaunchArg>,
    pub lenses: Vec<crate::proto::LensSpec>,
    pub instructions: String,
    pub notices: Vec<String>,
}
impl std::fmt::Debug for LaunchPart { /* env: keys; args: Optional values as "<n bytes>"; instructions: len */ }
impl std::fmt::Debug for LaunchArg { /* Plain shown; Optional { flag, value: "<n bytes>" } */ }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchError {
    /// Show checklist `checklist`, then prepare again (D8).
    NeedsReview { checklist: String, revision: String },
    Failed(String),
}

/// The pre-P3 persisted routing (state.json/web-state.json `proxy`, `first_party`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LegacyChoice { pub proxy: Option<String>, pub first_party: bool }

pub trait Launch: Send + Sync {
    fn flags(&self) -> &'static [FlagDecl] { &[] }
    /// This addon's flags as given on the command line → a select (`Err`: a usage message).
    fn select(&self, given: &[FlagArg]) -> Result<LaunchSelect, String> { let _ = given; Ok(LaunchSelect::default()) }
    fn options(&self, ctx: &dyn AddonCtx) -> Vec<LaunchOption> { let _ = ctx; Vec::new() }
    /// `Err`: an unknown or unusable choice of this addon (fails the launch).
    fn resolve(&self, select: &LaunchSelect, ctx: &dyn AddonCtx) -> Result<Routing, String>;
    fn prepare<'a>(&'a self, req: LaunchRequest, ctx: Arc<dyn AddonCtx>) -> BoxFuture<'a, Result<LaunchPart, LaunchError>>;
    fn adopt_legacy(&self, legacy: &LegacyChoice) -> Option<LaunchChoice> { let _ = legacy; None }
    fn legacy(&self, choice: &LaunchChoice) -> Option<LegacyChoice> { let _ = choice; None }
}
```

```rust
// src/addon/ui.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Tone { Ok, Warn, Hot, Muted, Accent }

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Segment { pub id: String, pub text: String, pub tone: Tone, pub priority: u8 }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PanelKey { pub action: &'static str, pub default: &'static str }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PanelDecl {
    /// `[a-z][a-z0-9-]{0,31}`, unique per addon.
    pub id: &'static str,
    pub title: &'static str,
    /// Keymap action name and default key spec (`proxy_stats`, `alt+s`).
    pub key: Option<PanelKey>,
    /// The windows a request may name, first = default (`["24h", "7d", "30d"]`).
    pub windows: &'static [&'static str],
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct PanelRequest { pub id: String, #[serde(default)] pub page: usize, #[serde(default)] pub window: Option<String>, #[serde(default)] pub sid: Option<SessionId> }

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Panel { pub title: String, pub pages: Vec<Page>, pub actions: Vec<PanelAction>, pub updated: Option<u64> }
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Page { pub title: String, pub blocks: Vec<Block> }
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Block {
    Text { text: String, #[serde(skip_serializing_if = "Option::is_none")] tone: Option<Tone> },
    KeyValues { items: Vec<(String, String)> },
    Table { header: Vec<String>, rows: Vec<Vec<String>> },
    Gauge { label: String, used: f64, limit: f64, resets: Option<String> },
    Bars { label: String, series: Vec<(String, f64)> },
}
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct PanelAction { pub id: String, pub label: String, pub key: Option<char>, pub confirm: Option<String> }
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct ActionRequest { pub panel: String, pub action: String, #[serde(default)] pub sid: Option<SessionId> }

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Checklist { pub id: String, pub revision: String, pub title: String, pub intro: String, pub items: Vec<ChecklistItem> }
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ChecklistItem { pub id: String, pub label: String, pub detail: Option<String>, pub section: Option<String>, pub on: bool }
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChecklistAnswer { Save { revision: String, off: Vec<String> }, Defer }

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ExtensionRow {
    pub name: String, pub description: String,
    /// When `[extensions] <name>` is absent.
    pub default_on: bool,
    pub refused: Option<String>,
    pub details: Vec<String>,
    /// The checklist Enter opens (a plugin's setup).
    pub setup: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowAction { Toggled(bool), Refresh }

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Level { Info, Warn, Error }
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Notice { pub text: String, pub level: Level }

pub trait Ui: Send + Sync {
    /// No I/O: a snapshot under a std Mutex never held across an await.
    fn segments(&self, session: &SessionSnap, ctx: &dyn AddonCtx) -> Vec<Segment> { let _ = (session, ctx); Vec::new() }
    fn panels(&self) -> &'static [PanelDecl] { &[] }
    fn panel<'a>(&'a self, req: PanelRequest, ctx: Arc<dyn AddonCtx>) -> BoxFuture<'a, Panel>;
    fn action<'a>(&'a self, req: ActionRequest, ctx: Arc<dyn AddonCtx>) -> BoxFuture<'a, Result<Notice, String>>;
    fn checklist(&self, id: &str, ctx: &dyn AddonCtx) -> Option<Checklist> { let _ = (id, ctx); None }
    fn save_checklist<'a>(&'a self, id: String, answer: ChecklistAnswer, ctx: Arc<dyn AddonCtx>) -> BoxFuture<'a, Result<(), String>>;
    fn extension_rows(&self, ctx: &dyn AddonCtx) -> Vec<ExtensionRow> { let _ = ctx; Vec::new() }
    fn row_action<'a>(&'a self, row: String, action: RowAction, ctx: Arc<dyn AddonCtx>) -> BoxFuture<'a, Result<Option<Notice>, String>>;
}
```
`panel`, `action`, `save_checklist` and `row_action` get default bodies that return a "not
supported" panel or `Err`.

```rust
// src/addon/daemon.rs
pub trait DaemonOps: Send + Sync {
    fn handle<'a>(&'a self, op: &'a str, body: Value, ctx: Arc<dyn DaemonCtx>) -> BoxFuture<'a, Result<Value, String>>;
    /// claude was just installed on this host (today's `Manifests::forget_failures`).
    fn on_claude_installed(&self, ctx: &Arc<dyn DaemonCtx>) { let _ = ctx; }
}
pub trait DaemonCtx: Send + Sync {
    fn addon(&self) -> &'static str;
    fn claude_bin(&self) -> std::path::PathBuf;
    /// The daemon's login-shell PATH (`daemon::Config::path`).
    fn path(&self) -> Option<std::ffi::OsString>;
    /// The flags this claude knows (one `--help`, cached; probed off the runtime).
    fn claude_flags(&self) -> BoxFuture<'static, ClaudeFlags>;
    /// `<cache>/<addon>` (D7); the addon creates it 0700.
    fn cache_dir(&self) -> std::path::PathBuf;
    /// Held while claude update or a `claude plugin …` runs: one writer of `~/.claude`.
    fn updating(&self) -> Arc<tokio::sync::Mutex<()>>;
}
/// The optional flags a claude's `--help` lists (D6).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClaudeFlags { pub skip_permissions: bool, pub listed: std::collections::BTreeSet<String> }
impl ClaudeFlags {
    /// Every `--flag` that starts a line of `help` (today's `host::lists` rule).
    pub fn from_help(help: &str) -> ClaudeFlags;
    pub fn knows(&self, flag: &str) -> bool;
}
```

```rust
// src/addon/clean.rs — the core's caps (C2); used by the runtime, never by addons
pub const SEGMENT_CHARS: usize = 64;
pub const PANEL_BYTES: usize = 64 * 1024;
pub const CHECKLIST_ITEMS: usize = 256;
pub const TEXT_BYTES: usize = 4096;          // any one string of a panel, checklist, row
pub fn line(s: &str, max_bytes: usize) -> String;      // text::sanitize, controls and newlines out, cut on a char boundary
pub fn segment(s: Segment) -> Segment;                  // id via name_ok-or-"segment", text ≤ 64 chars
pub fn panel(p: Panel) -> Panel;                        // every string; over 64 KiB encoded → one error page
pub fn checklist(c: Checklist) -> Checklist;            // ≤ 256 items, strings capped
pub fn row(r: ExtensionRow) -> ExtensionRow;
pub fn option(o: LaunchOption) -> LaunchOption;
pub fn notice(n: Notice) -> Notice;
pub fn error_panel(title: &str, why: &str) -> Panel;
```

```rust
// src/addon/registry.rs
pub const CORE_SECTIONS: &[&str] = &["ui", "keys", "update", "claude", "extensions"];
pub const RESERVED_WORDS: &[&str] = &["__probe", "--slave", "__hook", "__context", "--daemon",
    "upgrade", "daemon", "sessions", "--plain"];
pub const CORE_FLAGS: &[&str] = &["--plain", "--daemon", "--slave", "-p", "--print", "--api", "-h", "--help"];
/// The immutable set of addons this binary runs with.
pub struct Addons { list: Vec<Arc<dyn Addon>> }
impl Addons {
    /// Refuses: a bad or repeated name or a reserved word; a core or twice-claimed config
    /// section; a launch flag not `--[a-z][a-z0-9-]*`, repeated, or a core flag; a bad or
    /// repeated panel id, or a panel key action named twice.
    pub fn new(list: Vec<Arc<dyn Addon>>) -> Result<Addons, String>;
    pub fn empty() -> Addons;
    pub fn iter(&self) -> impl Iterator<Item = &Arc<dyn Addon>>;
    pub fn get(&self, name: &str) -> Option<&Arc<dyn Addon>>;
    /// The first addon whose `is_relay` takes `argv` runs it.
    pub fn relay(self: &Arc<Self>, argv: &[String]) -> Option<ExitCode>;
    pub fn dispatch(self: &Arc<Self>, argv: &[String]) -> Option<ExitCode>;
    pub fn help(&self) -> String;
    /// `addon:<name>` for each addon with `daemon()`.
    pub fn daemon_features(&self) -> Vec<String>;
    /// `(addon, flag)` for every launch flag.
    pub fn launch_flags(&self) -> Vec<(&'static str, FlagDecl)>;
}
```

`src/addon/test_support.rs` (cfg(test)) defines `TestAddon`, a configurable fake used by every later
task's unit tests: a name, sections, flags, a scripted `resolve`, a scripted `prepare`
(`Ok(part)`, `Err`, a delay, `NeedsReview` until a checklist is saved), `segments`, one panel, one
checklist, rows, and an echoing `DaemonOps` (`echo`, `sleep`, `big`, `fail`). It records every
call.

`main.rs`: `mod addon; mod addons;`. At the top of `main()`, build `let addons =
Arc::new(Addons::new(addons::all()).unwrap_or_else(|e| { eprintln!("claudio: {e}");
std::process::exit(1) }))`. It is not passed anywhere yet (later tasks do).

Tests (in `src/addon/*`):
- `name_ok_accepts_the_documented_charset_only`
- `registry_refuses_duplicates_reserved_words_core_sections_and_shared_sections`
- `registry_refuses_bad_or_repeated_flags_and_panel_keys`
- `registry_lists_daemon_features_only_for_daemon_addons`
- `relay_runs_before_dispatch_and_dispatch_follows_registry_order`
- `launch_part_debug_shows_env_keys_never_values` (C1)
- `launch_select_json_round_trips` (`{"pick":"default"}`, `{"pick":"option","addon":"a","option":"b","flags":["x"]}`)
- `clean_caps_segments_panels_checklists_and_strips_controls` (ESC, BEL, C1, newlines, a 70 KiB panel → error page, 300 items → 256)
- `addons_all_registers` (src/addons/mod.rs: `Addons::new(all())` is Ok)

Run S-unit and S-check. Expected: green. Clippy is clean thanks to the module allow.

Commits: `docs(addons): the addon API`, `feat(addon): the addon API types, registry and caps`.

Interfaces this task provides: everything in `crate::addon::*`; `crate::addons::all()`;
`addon::test_support::TestAddon`.

---

## P3.2: daemon ops for addons (spec "Daemon ops for addons"; B1, A10, AC8)

Model: strong   Files: docs/daemon-protocol.md (new), src/proto.rs, src/daemon/{mod.rs,
server.rs, update.rs, host.rs, session.rs}, src/daemon/addon_ops.rs (new), src/daemon/tests.rs,
src/main.rs (`daemon::run(addons)`), src/addon/daemon.rs (ClaudeFlags body),
src/proto/spawn.rs (new, split), src/daemon/lifecycle.rs (new, split)

Goal: a daemon runs `DaemonOps` off its request loop and lists `addon:<name>`. Slow ops share one
reply-later helper.

Commit 0 (A8 split, F1): `refactor(proto): split the spawn specs out of proto.rs` (to
`src/proto/spawn.rs`, re-exported) and `refactor(daemon): split the session lifecycle out of
daemon/mod.rs` (to `src/daemon/lifecycle.rs`). No behaviour change; baseline rewritten in these
commits only.

Commit 1 (docs, B2): `docs/daemon-protocol.md`. It holds the framing summary (pointing to
`src/proto.rs`), Hello/Welcome and the features list (existing ones plus `addon:<name>`),
`AddonOp`/`AddonReply` (body ≤ 512 KiB both ways, refusals: `unsupported op` from an older
daemon; `no such addon <name>`; `addon <name> busy` past 8 in flight; `body over 512 KiB`;
`addon <name> timed out` after 150 s), plus the sections P3.3 fills (`env_remove`, `optional`).
It also holds the mixed-version table copied from the spec and the D14 removal date, and says
that the daemon logs an `AddonOp` as `addon`, `op` and the body's encoded length, never the body
(F23). This file
is the "daemon-protocol section" the spec asks for; CLAUDE.md points to it in P3.20.

Commit 2 (code):
- `src/proto.rs`:
  ```rust
  /// Run addon `addon`'s daemon op `op`. Sent only to a daemon whose `Welcome.features`
  /// lists `addon:<addon>`; an older one answers `Error "unsupported op…"`.
  AddonOp { addon: String, op: String, #[serde(default)] body: serde_json::Value },
  /// The reply to `AddonOp`.
  AddonReply { body: serde_json::Value },
  ```
  `pub const ADDON_BODY_MAX: usize = 512 * 1024;` and `pub fn addon_body_ok(body: &Value) ->
  Result<(), String>` (encoded length).
- `src/addon/daemon.rs`: give `ClaudeFlags::from_help`/`knows` their bodies (move `host::lists`
  there). `daemon/host.rs::claude_flags` returns `addon::ClaudeFlags`. The four-bool struct and
  its consts stay only as `ALLOW_SKIP_PERMISSIONS` plus the three legacy flag names used by
  `session::command` (P3.3 generalises that). `ClaudeFlags::optional()`/`dropped()` callers
  (`daemon/session.rs:337-382`, `daemon/manifest.rs:360`) use `knows()`. `host.rs` tests
  :330-383 become `from_help` tests with the same help texts. `ClaudeFlags` is `Clone` (not
  `Copy`, F16): `Daemon::claude_flags()` returns a clone and `command()` takes `&ClaudeFlags`.
- `Daemon` (`src/daemon/mod.rs:219-249`) gains `addons: Arc<Addons>` and `addon_slots:
  HashMap<&'static str, Arc<Semaphore>>` (8 permits each). `updating` becomes
  `Arc<tokio::sync::Mutex<()>>`. `daemon::run(addons: Arc<Addons>)`, `server::start(config,
  addons)`, `Daemon::new(config, addons)`. Test daemons pass `Addons::empty()` or a `TestAddon`.
- `src/daemon/addon_ops.rs`:
  ```rust
  /// The `DaemonCtx` of addon `addon` in this daemon.
  pub(crate) struct Ctx { daemon: Arc<Daemon>, addon: &'static str }
  impl DaemonCtx for Ctx { /* claude_bin/path from daemon.config; claude_flags via
      spawn_blocking(daemon.claude_flags()); cache_dir = paths::cache_dir().join(addon);
      updating = Arc::clone(&daemon.updating) */ }
  /// Run one AddonOp: body cap, find the addon's DaemonOps, take a slot (none free → Error
  /// "addon <name> busy"), `handle` under a 150 s timeout, reply cap.
  pub(crate) async fn run(daemon: Arc<Daemon>, addon: String, op: String, body: Value) -> Msg;
  /// Logs `addon`, `op` and the body's encoded length, never the body (F23).
  /// Every daemon-side addon's `on_claude_installed`.
  pub(crate) fn claude_installed(daemon: &Arc<Daemon>);
  ```
- `src/daemon/server.rs`: `impl Client { fn reply_later(&self, req: Option<u64>, work: impl
  Future<Output = Msg> + Send + 'static) }` spawns the task and queues `Frame::Control(Envelope
  { req, msg })`. `Answers`, `LastPrompt`, `UpdateClaude`, `Manifest` (until P3.7) and `AddonOp`
  use it, which shrinks `request` (:419-588). Welcome features
  (`server.rs:283`) become `DAEMON_FEATURES` plus `daemon.addons.daemon_features()`. `op_name`
  stays.
- `src/daemon/update.rs:86-89`: after a successful install, `daemon.manifests.forget_failures()`
  stays and `addon_ops::claude_installed(daemon)` is added (P3.7 moves the first behind the
  second).

Tests (`src/daemon/tests.rs`, with a `TestAddon` named `echo`):
- `welcome_lists_addon_features_only_for_daemon_addons`: empty registry → no `addon:`;
  `echo` → `addon:echo`.
- `addon_op_replies_from_a_task_beside_other_requests`: `sleep` (2 s) then `Ping` answered
  first.
- `addon_op_refuses_unknown_addons_big_bodies_and_a_ninth_in_flight`: `no such addon`,
  `body over 512 KiB` (600 KiB string), 9 concurrent `sleep` → one `busy`.
- `addon_op_reply_over_the_cap_is_an_error` (`big` returns 600 KiB).
- `addon_op_times_out` (`sleep` 200 s with the timeout made a `Config` field, 1 s in tests).
- `claude_install_calls_on_claude_installed` (extends
  `installing_claude_forgets_the_plugin_failures` :2810 with the echo addon's counter).
- proto: `addon_op_json_shape` and `unknown_op_from_a_newer_client_is_refused`
  (`{"op":"addon_op",…}` sent as raw JSON to a daemon built without the variant cannot be had,
  so this asserts `op_name` → `unsupported op: …` for `Msg::Unknown`).

Run S-unit, S-manager, S-web, S-check. Expected: green, and `request` is shorter than 171 lines
(update the baseline down).

Commits: `docs(daemon): the daemon protocol, addon ops`, `refactor(daemon): slow requests reply
from tasks through one helper`, `feat(daemon): addon ops and addon features`.

Interfaces: `Msg::AddonOp/AddonReply`, `proto::ADDON_BODY_MAX/addon_body_ok`,
`daemon::run(Arc<Addons>)`, `addon_ops::claude_installed`, `Client::reply_later`,
`addon::ClaudeFlags`.

---

## P3.3: `env_remove`, the compatibility scrub, generic optional args (spec "Mixed versions"; B1, C1, AC8)

Model: strong   Files: docs/daemon-protocol.md (commit 1), src/proto.rs, src/daemon/{session.rs,
mod.rs, journal.rs, server.rs, tests.rs}, src/claude/mod.rs (doc comment), tests/common/mod.rs,
tests/manager.rs, every `SpawnSpec {…}`/`RespawnSpec {…}` literal (tui/sessions.rs, web/spawn.rs,
daemon) gets the new fields

Commit 1 (docs): the `env_remove` and `optional` sections of docs/daemon-protocol.md, plus the
retry rule below.

Commit 1b (A8 split, F1): `refactor(daemon): split the session command builder out of
session.rs` (`command`, `apply_env`, `shell_command`, `clean_title`, session.rs:331-420 →
`src/daemon/command.rs`). The changes below then land in `command.rs`.

Commit 2 (code):
- `SpawnSpec` and `RespawnSpec` (`src/proto.rs:404`, `:433`) gain
  ```rust
  /// `None`: an old client — the daemon applies its compatibility scrub (today's rule).
  /// `Some(list)`: remove exactly `claude::SESSION_MARKERS` plus `list`.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub env_remove: Option<Vec<String>>,
  ```
  and `SpawnSpec` gains `#[serde(default, skip_serializing_if = "Vec::is_empty")] pub optional:
  Vec<String>` (D6). Their `Debug` prints both (names only).
- `src/daemon/session.rs`: drop `use crate::proxy::env::env_diff` (:37). Add
  ```rust
  /// What a claude session's process loses from the daemon's env.
  fn removed_env<'a>(env: &[(String, String)], env_remove: Option<&'a [String]>) -> Vec<Cow<'a, str>>
  /// compat(v0.10): today's `proxy::env::env_diff` rule for clients that send no `env_remove`.
  fn legacy_scrub(env: &[(String, String)]) -> Vec<&'static str>
  fn env_remove_ok(list: &[String]) -> Result<(), String>   // ≤ 64 names, `[A-Za-z_][A-Za-z0-9_]{0,127}`
  fn optional_ok(list: &[String]) -> Result<(), String>     // ≤ 16, `--[a-z0-9][a-z0-9-]{0,39}`
  ```
  `apply_env(cmd, path, env, env_remove)` uses `removed_env`. `shell_command` passes
  `Some(&[])` (markers only, as today). `command()`'s drop rule (`:362-376`) becomes: drop a flag
  (and its value, or `--flag=value`) when it is in `spec.optional ∪ LEGACY_OPTIONAL` (compat(v0.10):
  `--plugin-dir`, `--agents`, `--mcp-config`) and `!flags.knows(flag)`.
- `Daemon::spawn`/`respawn` (`src/daemon/mod.rs`) refuse bad `env_remove`/`optional` like bad
  extensions ("bad env_remove: …"). The journal `Entry` (`src/daemon/journal.rs:35`) gains
  `#[serde(default)] optional: Vec<String>` (written by `upsert_entry`). `respawn` and
  `retry_spawn_fresh` copy it into the spec.
- **Retry keeps the routing** (fixes the hole named in Review Focus; F3). `Daemon` gains
  `session_env: Mutex<HashMap<SessionId, SessionEnv>>` with `struct SessionEnv { env:
  Vec<(String, String)>, env_remove: Option<Vec<String>> }` (`Debug` prints keys only, C1;
  never journaled). `spawn` and `respawn` store the spec's pair under the id; `kill`,
  `close_exited` and a `Shutdown` drop it; `retry_spawn_fresh` (which runs after the actor's
  `forget_live`, session.rs:1074) reads it instead of `env: vec![]`, and a missing entry (a
  daemon restarted meanwhile) keeps today's empty env with a `tracing::warn!`. The journal is
  unchanged.
- `proxy::env::env_diff` and `EnvDiff` are deleted (the daemon was their last user). The
  `src/claude/mod.rs:30` comment points to `daemon::session::legacy_scrub`.
- Test-only old-daemon switch: `daemon::Config` gains `old_peer: bool`, set from
  `CLAUDIO_TEST_OLD_DAEMON=1` in debug builds only (`#[cfg(debug_assertions)]`, like
  `CLAUDIO_TEST_LOOPBACK_HTTP`). With it the daemon lists no `addon:*` feature, answers
  `AddonOp` with `unsupported op: addon_op`, treats every `env_remove` as `None`, and ignores
  `optional` (drops only `LEGACY_OPTIONAL`). It simulates the previous release (F10); the Merge
  section asks the owner to run a v0.8.0 `claudio --daemon` against a P3 client once. P3.9 uses
  it.

Tests (AC8, both directions, an inherited `ANTHROPIC_API_KEY`):
- `src/daemon/session.rs` unit tests (CommandBuilder, like :1226-1298):
  `old_client_without_env_remove_gets_the_compatibility_scrub` (inherited `ANTHROPIC_API_KEY` +
  env `ANTHROPIC_AUTH_TOKEN` → key removed; first-party env without `CLAUDE_CODE_USE_GATEWAY` →
  inherited gateway var removed; no token → key kept);
  `env_remove_some_removes_exactly_the_markers_and_the_list` (`Some([])` keeps an inherited key,
  `Some(["ANTHROPIC_API_KEY"])` drops it, `CLAUDECODE` always dropped);
  `a_shell_loses_only_the_markers`; `optional_flags_are_dropped_only_when_unknown` (generic
  flag `--x-test` listed in `optional`, kept when `--help` lists it).
- `src/daemon/tests.rs`: `bad_env_remove_or_optional_is_refused`;
  `optional_is_journaled_and_applied_on_respawn`; `a_resume_retry_keeps_the_session_env` (the
  `fake_claude_resume_fails` fixture and an env-recording fake: the retried process sees the
  spawn's `ANTHROPIC_AUTH_TOKEN` and not an inherited `ANTHROPIC_API_KEY`); `the_old_daemon_switch_hides_addons_and_scrubs_as_before`.
- `tests/common/mod.rs`: `ManagerHarness::daemon_request(&self, msg: serde_json::Value) ->
  serde_json::Value` (std `UnixStream` to the harness daemon's socket; `Hello`, then one `J`
  frame; it reads frames until the one whose `req` matches, discarding `Welcome` and events).
- `plugin_dirs_are_dropped_for_an_old_claude_but_journaled` (`src/daemon/tests.rs:3096`, kept in
  core, F7) gains a generic optional flag.
- `tests/manager.rs`: `test_old_client_spawn_is_scrubbed_by_a_new_daemon`. Start a TUI so the
  daemon runs, then send a raw `{"op":"spawn",…}` JSON without `env_remove`, with env
  `ANTHROPIC_AUTH_TOKEN=t`, through `daemon_request`. The env-dumping fake shows no
  `ANTHROPIC_API_KEY` (the harness sets one) and no `CLAUDECODE`.

Run S-unit, S-manager, S-web, S-check. Expected: green, regression set included.

Commits: `docs(daemon): env_remove and optional claude flags`, `feat(daemon): env_remove with
a compatibility scrub for older clients`, `feat(daemon): generic optional claude flags, journaled`,
`fix(daemon): a resume retry keeps the session's env`.

Interfaces: `SpawnSpec.env_remove/optional`, `RespawnSpec.env_remove`, the old-daemon test
switch, `daemon_request` harness helper.

---

## P3.4: the addon runtime and the config editor (spec "Execution model"; A6, A10, B3)

Model: strong   Files: src/app/addons/ (new), src/app/config_edit.rs (new), src/app/mod.rs,
src/tui/addon_runner.rs (new: `start`, `on_host_up`, `on_host_down`, `on_session_event`, `drain`),
src/web/lenses.rs (new, split from hub.rs),
src/config.rs, src/tui/mod.rs (construct and feed the runtime), src/web/{mod.rs,hub.rs,ext.rs}
(construct and feed it; the web's 5 s config poll moves in), tests in the new files

Goal: one `AddonCtx` implementation, fed by both front-ends, with no addon registered yet.

Commit 0 (A8 splits, F1, F19): rename the TUI's private `enum Launch` (`tui/mod.rs:120-135`) to
`enum Entry`, so `use crate::addon::*` never shadows it; move the `Connected` arm's body
(`tui/mod.rs:739-763`) into `fn on_connected(…)` and the four proxy `HostEvent` arms (:816-829)
into `App::on_proxy_event`; move `web/mod.rs::serve`'s hub/ext construction into `fn
build_state(…)`; move the lens relay (`hub.rs:484-520`, `lens_events`) to `src/web/lenses.rs`.

```rust
// src/app/addons/{mod,ctx,events}.rs
pub struct RuntimeOptions { pub config_path: PathBuf, pub watch_config: bool }
/// What the runtime tells its front-end (bounded queue, 256, sent with `try_send`: a full queue
/// drops a `Changed`, which the dirty flag re-sends on the next drain, and drops the oldest
/// `Notice`/`Offer` with a `tracing::warn!`; F13).
#[derive(Debug, Clone, PartialEq)]
pub enum RuntimeOut {
    Notice { addon: &'static str, notice: addon::Notice },     // cleaned
    Changed { addon: &'static str },                            // coalesced per addon
    Offer { addon: &'static str, checklist: String },
}
#[derive(Clone)]
pub struct AddonRuntime { inner: Arc<Inner> }   // Inner: addons, http, hosts (std Mutex),
    // sessions, config (toml::Table), config edit lock, per-addon JoinSet + live count,
    // out tx, dirty flags, event tx
impl AddonRuntime {
    pub fn start(addons: Arc<Addons>, opts: RuntimeOptions) -> (AddonRuntime, mpsc::Receiver<RuntimeOut>);
    pub fn addons(&self) -> &Arc<Addons>;
    pub fn ctx(&self, addon: &'static str) -> Arc<dyn AddonCtx>;
    pub fn host_up(&self, host: &str, client: Client);   // features/claude from client.welcome()
    pub fn host_down(&self, host: &str);
    pub fn claude_installed(&self, host: &str);
    pub fn sessions_reset(&self, sessions: Vec<SessionSnap>);   // a host's list after (re)connect
    pub fn session_added(&self, s: SessionSnap);
    pub fn session_changed(&self, s: SessionSnap);   // emits SessionChanged when conversation or focus changed
    pub fn session_removed(&self, sid: SessionId);
    pub fn set_focused(&self, sids: &[SessionId]);
    /// Re-read config.toml (spawn_blocking) and send ConfigChanged.
    pub fn config_changed(&self);
    /// Abort every addon task and the tick/watch tasks.
    pub fn shutdown(&self);
}
```
Behaviour: one event task delivers `AddonEvent`s in order to every addon's `on_event` (never under a
runtime lock). It sends `Started` once after construction, then `Tick` every 1 s. `ctx.spawn` wraps
each task in `timeout(300 s)`; past 64 live tasks of one addon the task is dropped with a
`tracing::warn!` naming the addon. `ctx.request` checks the host's features for
`addon::feature(addon)` (`Missing`), checks `addon_body_ok` both ways, sends `Msg::AddonOp` through
the host's `Client`, maps `Error {message}` to `AddonError::Daemon`, and times out after 170 s.
`config(section)` answers only the addon's declared sections (else empty). `edit_config` refuses an
undeclared section and runs `config_edit::edit_section` in `spawn_blocking` under the runtime's
edit lock. On success it updates the cached table and sends `ConfigChanged`. `notify` goes out
cleaned. `changed` sets the addon's dirty flag and sends one `Changed` (coalesced until the
front-end drains it). `shutdown` aborts everything and is idempotent. The `Drop` of the last handle
calls it.

```rust
// src/app/config_edit.rs
/// Apply `edit` to top-level table `section` of the config file at `path`, minimally: a changed
/// scalar or string-array key of `[section]` → one `key = value` line replaced or added (indent
/// and trailing comment kept); a changed sub-table `[section.name]` → its header and key lines
/// replaced or the table appended; a removed one → its lines removed. Anything else (deeper
/// tables, inline tables, dates, floats) → Err(Unsupported). Written atomically keeping the
/// file's mode (0600 when created); nothing is written unless the result parses and differs
/// from the input exactly in `section`. Returns whether it changed the file.
pub fn edit_section(path: &Path, section: &str, edit: impl FnOnce(&mut toml::Table)) -> Result<bool, SaveError>;
```
`edit_section` takes a process-wide `static EDIT: std::sync::Mutex<()>` for the read-modify-write
(F5). Every writer of config.toml (`save_extension`, the runtime's `edit_config`, the web's
toggle) goes through it, and the web's `ext.rs` `edit` lock is removed in P3.13.
`save_extension` is called from `spawn_blocking` in both front-ends (the TUI moves its call,
`tui/mod.rs:996`, into `addon_runner`).
`config::set_extension`/`save_extension` (`src/config.rs:407-517`) become thin wrappers over
`edit_section(path, "extensions", …)`, and their tests stay as they are and must pass unchanged.
`set_plugin_choices`/`save_plugin_choices` stay until P3.8 (their tests then move to the Vocdoni
addon as `edit_section` tests with the same fixtures). The minimal-edit line code
(`split_eol`, `header`, `replace_key`, `track_multiline`) moves from `config.rs` to
`config_edit.rs`.

Wiring (no behaviour change, the registry is empty):
- TUI (`tui/mod.rs`, through `addon_runner`; each call from `main`/`event_loop` is one line):
  `main` builds the runtime (`watch_config: false`) with `addons: Arc<Addons>`.
  `run(addons)`/`run_with_proxy`/`run_plain` take it from main.rs from here on. The event loop
  calls `host_up`/`host_down` where it calls `app.on_plugin_host`/`plugin_host_gone` (:308, :757,
  :790; `interaction.rs:794`), `session_*` from the daemon event handling, `set_focused(active)`
  on activation, and `claude_installed` where `reapply_host` is called (`claude_update.rs:155`). It
  drains `RuntimeOut` in the `select!` loop (Notice → `app.notify`; others ignored until P3.8).
  `shutdown()` runs on exit.
- Web: `build_state` builds the runtime (`watch_config: false`, F20) and keeps it in `AppState`.
  The hub feeds it with one line per `HubEvent` emission (`hub.rs:1767, :1866, :1956, :2141`;
  the three moments of `hub.rs:532-544`, plus sessions). `ext.rs::watch_config` (:218-275) keeps
  its own poll and registry re-read and calls `runtime.config_changed()` after a re-read. P3.13
  flips `watch_config` to `true` and deletes `ext.rs`'s poll.

Tests (`src/app/addons/`, `src/app/config_edit.rs`, with `TestAddon` and a fake `Client`
over `tokio::io::duplex` running `proto` frames — reuse `client.rs`'s handshake test helper):
- `events_arrive_in_order_after_started`
- `a_request_needs_the_hosts_addon_feature`, `request_bodies_are_capped_both_ways`,
  `request_maps_daemon_errors_and_times_out`
- `spawned_tasks_are_bounded_timed_out_and_aborted_at_shutdown`
- `config_is_limited_to_declared_sections`, `edit_config_refuses_undeclared_sections`
- `changed_is_coalesced`, `notices_are_cleaned`
- `edit_section_*`: today's `set_extension`/`set_plugin_choices` fixtures (comments, CRLF,
  missing table, symlinked file, mode kept), plus `edit_section_refuses_nested_tables` and
  `edit_section_writes_nothing_when_another_section_would_change`.

Run S-unit, S-manager, S-web, S-e2e, S-check. Expected: green.

Commits: `refactor(config): a generic minimal config editor`, `feat(app): the addon runtime`,
`refactor(tui,web): feed the addon runtime`.

Interfaces: `app::addons::{AddonRuntime, RuntimeOut, RuntimeOptions}`,
`app::config_edit::edit_section`, `tui::run(addons)`, `tui::run_plain(addons, …)`.

---

## P3.5: the launch composer; direct launches go through it (spec "Launches"; A5, A10, AC8)

Model: strong   Files: src/app/launch/ (new), src/tui/addon_runner.rs (from P3.4),
src/tui/proxy_glue.rs (new, split), src/tui/{app.rs, mod.rs, sessions.rs, plain.rs}, src/web/spawn.rs

**Runs as three tasks (F14), each ending green:**
- **P3.5a** (strong) `feat(app): the launch composer`: `src/app/launch/{mod,compose,review}.rs`
  and its unit tests below; no front-end change. Run S-unit, S-check.
- **P3.5b** (strong) the TUI and `--plain`: first the A8 split commit (`refactor(tui): split the
  proxy glue out of app.rs and sessions.rs`: the proxy methods of app.rs:525-655 and
  `send_with_proxy`/`proxy_env_for` → `src/tui/proxy_glue.rs`; the `SpawnWithProxy` arm body
  `tui/mod.rs:1098-1178` → `addon_runner::spawn_with_proxy`), then the TUI wiring below and the
  `tui::app` tests. Run S-unit, S-manager, S-check.
- **P3.5c** (strong) web direct launches (`web/spawn.rs`). Run S-unit, S-web, S-e2e, S-check.

Goal: one composer for TUI, `--plain` and web, used now for every launch that has no proxy
profile (the old proxy path stays until P3.9/P3.10).

```rust
// src/app/launch/{mod,compose,review}.rs
/// What a front-end asks for. `base` is a `Spawn` or `Respawn` with no env, env_remove,
/// optional or extensions; `core_lenses` the core registry's lenses (external extensions, and
/// until P3.18 the built-ins).
pub struct LaunchSpec { pub op: LaunchOp, pub sid: SessionId, pub host: String, pub cwd: String,
    pub select: LaunchSelect, pub frontend: Frontend, pub base: Msg, pub core_lenses: SessionExtensions }
/// The prepare budget (5 s config + 20 s manifests, today).
pub const BUDGET: Duration = Duration::from_secs(25);
/// A launch stopped for a review: what is done so far and how much budget is left.
pub struct Parked { /* spec, routings, parts so far, the addon waiting, deferred, remaining */ }
pub enum Step {
    /// Sent; `reply` is the daemon's answer.
    Sent { choice: Option<LaunchChoice>, notices: Vec<String>, reply: io::Result<Msg> },
    Review { addon: &'static str, checklist: String, revision: String, parked: Parked },
    Failed(String),
}
pub enum ReviewAnswer { Save(ChecklistAnswer), Skip, Cancel }

/// Leading launch flags of any addon in `args` → the select and the rest (today's
/// `proxy::resolve::split_launch_flags`, generic).
pub fn split_flags<'a>(addons: &Addons, args: &'a [String]) -> Result<(LaunchSelect, &'a [String]), String>;
/// Every addon's routing. Errors: a `Pick::Option` of no registered Launch addon ("launch option
/// of `X` is not available in this build"), an addon's `Err`, two `Required`.
pub fn resolve(rt: &AddonRuntime, select: &LaunchSelect) -> Result<(Option<LaunchChoice>, Vec<(&'static str, Routing)>), String>;
/// Resolve, prepare every non-Off addon concurrently within BUDGET, compose, frame-check, send.
pub async fn run(rt: &AddonRuntime, spec: LaunchSpec, client: Client) -> Step;
/// Continue a parked launch: Save → `ui.save_checklist` then that addon's prepare again;
/// Skip → its prepare again with the checklist deferred; Cancel → Failed("cancelled").
pub async fn resume(rt: &AddonRuntime, parked: Parked, answer: ReviewAnswer, client: Client) -> Step;
/// The parts, in registry order, into the message's share (pure; unit-tested).
pub fn compose(base: Msg, core: SessionExtensions, parts: Vec<(&'static str, LaunchPart)>) -> Result<(Msg, Vec<String>), String>;
```
Rules for `run`:
- A `SpawnShell` base never reaches `run`: the front-end sends it directly ("a terminal launch
  skips the addons").
- A Required addon's `Failed`, or its timeout, fails the launch, and nothing is sent. An Optional
  addon's becomes a notice ("<addon>: <why>") and the launch goes on without it. NeedsReview on a
  Respawn/Recover counts as Skip.
- `compose`: env in registry order, a later addon's key replacing an earlier one's, no key twice.
  `env_remove` is the union, sorted, and always `Some` (empty = markers only). `args` are
  appended in order (`Plain` as is; `Optional{flag, value}` as `flag value`, with `flag` added to
  `optional`; for a Respawn, args and `optional` are dropped because the journal has them).
  Lenses: core first, then the addons' in order, merged by name (first wins), within
  `MAX_LENSES`/`MAX_INSTRUCTION`/`MAX_INSTRUCTIONS` (one breaking a cap is left out with a notice,
  as `Registry::session_extensions` does). `instructions` are joined by a blank line and cut at
  the last whole part that fits `MAX_PLUGIN_INSTRUCTIONS` (8158), with a notice naming those left
  out. The encoded `Frame::Control(Envelope::request(0, msg))` must be ≤ `MAX_FRAME`, else the
  launch fails ("launch too large").
- The env never leaves `run`/`resume`: no `Debug` of the message, no log of env values.

TUI wiring (A6: `LaunchPart` and env never enter `App`):
- `Effect::Launch(LaunchCall)` with `pub struct LaunchCall { pub id: u64, pub op: LaunchOp, pub sid:
  SessionId, pub host: String, pub cwd: String, pub select: LaunchSelect, pub base: Msg, pub to:
  ReplyTo }` (the base carries no env). `HostEvent::Launched { id: u64, sid: SessionId, choice:
  Option<LaunchChoice>, notices: Vec<String>, result: Result<Msg, String> }` (the spec's
  `Launched { id, notices }`, with what `App` needs to record the choice and route the reply as
  today's `ReplyTo` does). It is handled in `app.rs` by `App::on_launched` (not in
  `interaction.rs::on_reply`, F1), which routes the result exactly as the old reply path did.
- `src/tui/addon_runner.rs` runs `Effect::Launch` (`app::launch::run` on a task; on `Step::Review`,
  until P3.11, it fails the launch with "review needed"). This keeps `run_effect` from growing.
- `send_spawn`/`reset_session`/recovery (`tui/sessions.rs:375-542`, `:666-746`) and `--plain`'s
  start emit `Effect::Launch` when the session has no proxy profile (`select.pick =
  Pick::Direct`); with a profile they keep the old `send_with_proxy` path.
- Web `spawn_claude`/`respawn` (`web/spawn.rs:206`, `:288`) use `app::launch::run` when
  `profile.is_none()`, else the old `with_proxy`.

Tests (`src/app/launch/`, TestAddon, fake client):
- `compose_layers_env_later_addon_wins_no_key_twice`
- `compose_unions_env_remove_and_always_sends_some`
- `compose_marks_optional_args_and_respawn_drops_args`
- `compose_merges_lenses_first_wins_within_caps_and_cuts_instructions_at_8158`
- `compose_refuses_a_frame_over_the_cap`
- `a_required_addon_failure_fails_the_launch_and_sends_nothing` (AC8)
- `a_required_addon_timeout_fails_the_launch` (BUDGET shortened via a test constructor)
- `an_optional_addon_failure_is_a_notice_and_the_launch_goes_on`
- `two_required_addons_fail_the_launch`, `an_unknown_persisted_choice_fails_the_launch` (AC8)
- `needs_review_parks_with_the_remaining_budget_and_resume_continues` (the clock is stopped:
  a 10 s parked wait does not eat the budget)
- `skip_defers_only_for_that_launch`, `cancel_fails`, `respawn_never_parks`
- `split_flags_consumes_leading_addon_flags_in_any_order` (the cases of
  `proxy/resolve.rs` tests, with a TestAddon's flags)
- `tui::app` tests: a direct spawn emits `Effect::Launch` with a base without env;
  `Launched` routes the reply like `ReplyTo::Spawned`.

Run S-unit, S-manager, S-web, S-e2e, S-check. Expected: green. Direct sessions now spawn with
`env_remove: Some([])`, and `test_plain_no_proxy_keeps_api_key` proves the key stays.

Commits: `feat(app): the launch composer`, `refactor(tui): launches without a proxy go through
the composer`, `refactor(web): launches without a proxy go through the composer`.

Interfaces: `app::launch::{run, resume, compose, split_flags, resolve, LaunchSpec, Step,
Parked, ReviewAnswer, BUDGET}`, `Effect::Launch`, `HostEvent::Launched`, `tui::addon_runner`.

---

## P3.6: the Vocdoni code moves under src/addons/vocdoni (moves only)

Model: standard   Files: `git mv src/proxy src/addons/vocdoni/proxy`, `git mv src/plugins
src/addons/vocdoni/plugins`, src/addons/vocdoni/mod.rs (new), src/addons/mod.rs, src/main.rs (mod
lines), every importer of `crate::proxy`/`crate::plugins` (path updates only),
scripts/size-baseline.txt (renames)

Goal: the Vocdoni code sits where it will live; nothing else changes.

`src/addons/vocdoni/mod.rs`:
```rust
//! The Vocdoni addon: the claude-proxy client and the team plugins.
pub mod plugins; pub mod proxy;
pub const NAME: &str = "vocdoni";
/// The proxy `claudio vocdoni login` signs in to without a URL.
pub const DEFAULT_URL: &str = "https://claude.vocdoni.net";
pub struct Vocdoni;
impl Addon for Vocdoni { fn name(&self) -> &'static str { NAME }
    fn config_sections(&self) -> &'static [&'static str] { &["proxy", "plugins"] } }
```
`addons::all()` returns `vec![Arc::new(vocdoni::Vocdoni)]`. Importers use
`crate::addons::vocdoni::{proxy, plugins}` (transitional; P3.8-P3.13 remove every core importer).

Run S-unit, S-check (baseline renamed with the same counts). Expected: green; `git show --stat`
shows renames and import lines only.

Commit: `refactor(vocdoni): move the proxy client and plugin code under addons/vocdoni`.

---

## P3.7: the Vocdoni daemon slice, `vocdoni/apply` (spec "The Vocdoni addon / Daemon"; B1)

Model: strong   Files: `git mv src/daemon/manifest.rs src/addons/vocdoni/daemon/mod.rs`,
src/addons/vocdoni/daemon/tests.rs and src/addons/vocdoni/test_support.rs (moved from
src/daemon/tests.rs :2231-3095), docs/daemon-protocol.md, docs/addons.md, src/daemon/{mod.rs,
server.rs, update.rs, tests.rs}, src/proto.rs, src/tui/plugins_glue.rs (ask via AddonOp),
src/tui/extensions_view.rs (:588), src/web/plugins.rs (ask via AddonOp), src/remote/diag.rs
(AddonOp), tests/remote.rs (t9 seed path, :453)

Goal: today's `Manifest` op becomes `AddonOp { addon: "vocdoni", op: "apply" }`, unchanged in
behaviour, and the `Manifest` op is refused.

Steps:
0. Docs (B2, F7): `docs/daemon-protocol.md` drops `Manifest`/`Applied` (kept in a "refused ops"
   note with the D14 date) and `docs/addons.md` gains the Vocdoni section "daemon op `apply`:
   body `{name, url, sha256, secrets: [names], disabled: [ids]}`, reply `Applied`".
1. Move commit: `git mv` only, plus the module path changes.
2. Code:
   - `src/addons/vocdoni/daemon/mod.rs`: `pub struct VocdoniDaemon { manifests: Manifests }`
     (today's `Manifests`, :75-225) implementing `DaemonOps`. `handle("apply", body, ctx)` parses
     `ApplyRequest { name, url, sha256, secrets, disabled }` (the old `Msg::Manifest` fields,
     `deny_unknown_fields`) and runs today's `handle` (:315) with `daemon` replaced by `ctx`
     (`ctx.claude_bin()`, `ctx.path()`, `ctx.claude_flags()`, `ctx.updating()`,
     `ctx.cache_dir()` (D7), the claude config dir computed as today). It answers
     `serde_json::to_value(Applied)` or `Err("manifest <name>: …")` (same strings).
     `on_claude_installed` → `forget_failures()`. `remove_stale_zips` runs once, at the first
     `handle`, on `paths::cache_dir().join("plugins")` (compat(v0.10)).
   - `Applied`, `retrying`, `retry_secs` move from `src/proto.rs:453-497` to
     `src/addons/vocdoni/daemon/applied.rs` (serde-identical). `Msg::Manifest`/`Msg::Applied` are
     deleted. An old client's `{"op":"manifest"}` now deserialises to `Msg::Unknown` and gets
     `unsupported op: unknown`, which its `older_daemon_notice` (`tui/app.rs:854`) turns into
     "daemon too old". `UPDATE_BUSY` stays in proto (claude update is core).
   - `Daemon` loses `manifests`. `update.rs:88` loses `forget_failures` (the addon hook does it).
     `daemon/mod.rs:190` loses the `remove_stale_zips` call. The `Vocdoni` addon's `daemon()`
     returns its `VocdoniDaemon`, so `Welcome.features` lists `addon:vocdoni`.
   - Clients: `manifest_msg` (`tui/plugins_glue.rs`, `web/plugins.rs:196`) builds `Msg::AddonOp
     { addon: "vocdoni".into(), op: "apply".into(), body: json!(ApplyRequest{…}) }`.
     `on_applied` and the web's equivalent parse `Msg::AddonReply { body }` into `Applied`. A
     host whose `Welcome.features` lacks `addon:vocdoni` is `ApplyState::TooOld` without being
     asked (the hosts' features are at hand where `on_plugin_host` is called). This is
     transitional client code; P3.8 replaces it.
   - `remote/diag.rs::remote_manifest_cmd` stays in core under `cfg(feature = "diag")` and
     sends `Msg::AddonOp { addon: "vocdoni".into(), op: "apply".into(), body }` built from its
     arguments, parsing `AddonReply` as JSON (a transitional name in a diag-only path, listed
     with the P3.10 web adapter as the A3 exceptions; F8). P3.17 moves it.
   - `daemon::test_support` (cfg(test), `pub(crate)`): `TestDaemon` and its helpers, extracted
     from `src/daemon/tests.rs` so the addon's tests start a daemon with
     `Addons::new(vec![Arc::new(Vocdoni::with_daemon(VocdoniDaemon::with_dirs(cache, claude)))])`.
3. Tests: the manifest helpers and tests (:2231-3095: the fake plugin claude, `PluginHost`,
   `team`, `applied`, `serve`, `download`, and every test from
   `manifest_replies_first_then_installs_each_plugin_once` through
   `the_cache_keeps_at_most_32_manifests`, names unchanged) move to
   `src/addons/vocdoni/daemon/tests.rs`, the helpers (`serve` included) to
   `src/addons/vocdoni/test_support.rs` (cfg(test)), and send `AddonOp apply`.
   `plugin_dirs_are_dropped_for_an_old_claude_but_journaled` stays in `src/daemon/tests.rs` as
   the core test of the optional-flag rule (P3.3 extends it with a generic flag). Their assertions are
   unchanged except the request and reply envelope (said in the commit). New ones:
   - `a_manifest_op_from_an_old_client_is_refused_as_unsupported` (raw JSON `{"op":"manifest",…}`
     → `Error` starting `unsupported op`) (AC8, old client → new daemon)
   - `welcome_lists_addon_vocdoni`
   - `apply_bodies_carry_secret_names_never_values` (C1: the request JSON has `secrets:
     ["KB_TOKEN"]` and no value appears in any frame)
   - tui unit test `a_host_without_addon_vocdoni_is_too_old_and_never_asked`.
   - `tests/remote.rs` t9 seeds `~/.cache/claudio/vocdoni/<name>.toml` (run on `z6` if available).

Run S-unit, S-manager (regression set: the plugin manager test must pass unchanged), S-web,
S-e2e, S-remote, S-check.

Commits: `docs(daemon): manifest becomes the vocdoni apply op`, `refactor(vocdoni): move the
daemon's manifest handling into the addon` (move),
`feat(vocdoni): plugin manifests are the vocdoni/apply daemon op`, `test(vocdoni): manifest
daemon tests run against the addon op`.

Interfaces: `VocdoniDaemon`, `vocdoni::daemon::{Applied, ApplyRequest, retrying, retry_secs}`,
`daemon::test_support::TestDaemon`.

---

## P3.8: the Vocdoni client state and runner (spec "one plugin state machine and one proxy cache"; A5, A6)

Model: strong   Files: src/addons/vocdoni/{mod.rs, state/ (new: mod, lists, applies),
runner.rs (new), bridge.rs (new, temporary)}, src/addon/mod.rs (`as_any`, transitional),
docs/addons.md, src/addons/vocdoni/plugins/state.rs (from P2), `git rm
src/tui/plugins_glue.rs` (content moved), src/tui/{app.rs, mod.rs, sessions.rs, interaction.rs,
plugin_setup.rs, extensions_view.rs, claude_update.rs, plain.rs, test_support.rs},
src/config.rs (`[plugins]` parsing out)

**Runs as three tasks (F14), each ending green:**
- **P3.8a** (strong) the `State` port and its tests: pure, no TUI change, the glue keeps
  running. Run S-unit, S-check.
- **P3.8b** (strong) runner, bridge (with `Addon::as_any`, documented first in docs/addons.md),
  `[plugins]` out of config.rs. Run S-unit, S-manager, S-web, S-check.
- **P3.8c** (strong) the TUI reads the bridge, `plugins_glue.rs` deleted. Run S-unit, S-manager,
  S-web, S-e2e, S-check.

Goal: the TUI keeps no plugin or proxy-config state. The addon owns profiles, the config cache,
plugin lists, manifests, host answers and choices. The TUI's old setup screen and extension rows
read and write it through a temporary bridge, until P3.11/P3.13 replace them.

```rust
// src/addons/vocdoni/state/{mod,lists,applies}.rs — pure (A6): no I/O, unit-tested
pub struct State {
    pub profiles: Vec<String>, pub default: Option<String>,   // load_proxy_profiles, env profile first
    pub config: HashMap<String, (ConfigResponse, u64)>,        // per profile, fetched at (5 min)
    pub plugins: plugins::state::Plugins,                      // lists, manifests, applies, choices, deferred, noticed
    pub hosts: BTreeMap<String, bool>,                         // connected host → has addon:vocdoni
    pub extensions: BTreeMap<String, bool>,                    // [extensions] as read (rows' on/off)
    pub now: u64,
}
/// What the runner must do (I/O) after a transition.
pub enum Task {
    FetchList { profile: String }, FetchManifest { name: String, url: String, sha256: String },
    Ask { host: String, name: String, sha256: String, body: ApplyRequest },
    FetchConfig { profile: String }, SaveChoices { name: String, choices: PluginChoices },
    Notice(String), Changed, Offer(String),
}
impl State {
    pub fn load(cfg: &toml::Table /* proxy */, plugins: &toml::Table, ext: &toml::Table, cached: Vec<PluginList>, now: u64) -> State;
    pub fn on_list(&mut self, profile: &str, fetch: PluginsFetch) -> Vec<Task>;
    pub fn on_manifest(&mut self, name: &str, sha256: &str, r: Result<Manifest, String>) -> Vec<Task>;
    pub fn host_up(&mut self, host: &str, has_addon: bool) -> Vec<Task>;
    pub fn host_down(&mut self, host: &str);
    pub fn claude_installed(&mut self, host: &str) -> Vec<Task>;
    pub fn on_applied(&mut self, host: &str, name: &str, sha256: &str, r: Result<Applied, AddonError>);
    pub fn tick(&mut self, now: u64) -> Vec<Task>;            // reask_installing, the 15 min refresh
    pub fn save_choices(&mut self, name: &str, choices: PluginChoices) -> Vec<Task>;
    pub fn defer(&mut self, name: &str);
    pub fn set_enabled(&mut self, name: &str, on: bool) -> Vec<Task>;
    pub fn refresh(&mut self) -> Vec<Task>;                   // today's `r`
    pub fn on_config(&mut self, profile: &str, c: Option<ConfigResponse>);
    pub fn config_cached(&self, profile: &str) -> Option<&ConfigResponse>;
    /// The plugins' parts for a spawn of `profile` on `host`, and the asks it must wait for.
    pub fn plugin_parts(&self, host: &str, profile: &str, skip: &[String]) -> (Vec<PluginPart>, Vec<(String, ApplyRequest)>);
    pub fn first_sight(&self) -> Vec<String>;                 // ready manifests without choices, not deferred
}
```
This is a port, not a rewrite. The bodies come from `src/tui/plugins_glue.rs` (P2's glue: every
`impl App` method of today's `tui/plugins.rs:235-760`), with `self.plugins`/`self.extensions`/
`self.now` read from `State` and every `self.emit(...)`/`self.request(...)` returned as a `Task`.
Its tests (`tui/plugins.rs` tests from :735) move with it and assert on the returned `Task`s
instead of `Effect`s.

`src/addons/vocdoni/runner.rs`: `struct Runner { state: Mutex<State>, ctx: OnceLock<Arc<dyn
AddonCtx>> }`. `Vocdoni::on_event` maps `Started` (load: profiles and `plugins.json` via
`spawn_blocking`; config tables via `ctx.config`), `HostUp/Down`, `ClaudeInstalled`, `Tick`
(every 15 min `FetchList` for each profile in use) and `ConfigChanged` (re-read profiles and
choices) to `State` calls. It executes the `Task`s with `ctx.spawn`: list fetch with today's
`fetch_plugins` cache rule (`tui/mod.rs:1231-1260`, moved here; 401/403 drops the cache), manifest
fetch (`proxy::plugins::fetch_manifest` + `Manifest::parse_named`), `Ask` via
`ctx.request(host, "apply", body)`, config fetch (5 s), and `SaveChoices` via
`ctx.edit_config("plugins", …)` (the `set_plugin_choices` table shape).
`config::{set_plugin_choices, save_plugin_choices, PluginChoices, PluginsSection}` and
`Config.plugins` leave `src/config.rs`. Their tests move to
`src/addons/vocdoni/plugins/choices.rs` as `edit_section` tests with the same fixtures. The web
still reads `[plugins]` in this task, through `vocdoni::plugins::choices::from_table`.

`src/addons/vocdoni/bridge.rs` (temporary, deleted in P3.13; the only core→addon import):
```rust
/// The TUI's view of the addon's state for the old setup screen and extension rows.
pub struct Bridge(Arc<Runner>);
impl Bridge {
    pub fn of(addons: &Addons) -> Option<Bridge>;          // the registered Vocdoni's runner
    pub fn lists(&self) -> Vec<PluginList>;                 // for Registry::set_plugins
    pub fn manifest(&self, name: &str) -> Option<ManifestState>;
    pub fn choices(&self, name: &str) -> Option<PluginChoices>;
    pub fn plugin_details(&self, name: &str, profile: &str, refused: Option<&str>, now: u64) -> Vec<String>;
    pub fn first_sight(&self) -> Vec<String>;
    pub fn save_choices(&self, name: &str, choices: PluginChoices);   // through the runner
    pub fn defer(&self, name: &str);
    pub fn set_enabled(&self, name: &str, on: bool);
    pub fn refresh(&self);
    pub fn plugin_parts(&self, host: &str, profile: &str) -> (Vec<PluginPart>, Vec<(String, Msg)>);
    pub fn config_cached(&self, profile: &str) -> Option<ConfigResponse>;
    pub fn on_applied_reply(&self, host: &str, name: &str, sha256: &str, reply: io::Result<Msg>);
}
```
`Vocdoni` holds `Arc<Runner>` (F9). `Addon` gains a transitional method `fn as_any(&self) -> &dyn
std::any::Any` (documented in docs/addons.md as "P3 bridge only, removed in P3.13"). It has no
default body (a default cannot coerce `Self: ?Sized` to `Any`), so each addon writes `{ self }`.
`Bridge::of(addons)` is `addons.get(NAME)?.as_any().downcast_ref::<Vocdoni>().map(|v|
Bridge(Arc::clone(&v.runner)))`. No static.

TUI changes:
- `App` loses `plugins`, `proxy_config` (`app.rs:314`) and its methods `on_proxy_config`,
  `proxy_config_cached`, `maybe_fetch_proxy_config` (`:530`, `:637-655`). It gains `addons:
  Arc<Addons>` in `AppConfig` (pure queries only). It also gets a cached `plugin_rows:
  Vec<PluginList>` and `first_sight: Vec<String>`, refreshed by `HostEvent::Addon(AddonResult::Changed)`
  (the runner reads the bridge on `RuntimeOut::Changed`).
- `Effect::{FetchPlugins, FetchManifest, SavePluginChoices, FetchProxyConfig}` and their
  `HostEvent`s, `fetch_plugins`, the plugins tick (`tui/mod.rs:675-676`, `:868`),
  `ReplyTo::Manifest` (`interaction.rs:996`, `extensions_view.rs:588`) and `on_plugin_host`/
  `plugin_host_gone`/`reapply_host`/`reask_installing` calls are deleted (the runtime events
  drive the addon).
- `SpawnWithProxy` (`tui/mod.rs:1098-1178`) and `send_with_proxy` (`sessions.rs:438`) stay one
  more task, fed by `Bridge::plugin_parts`/`config_cached`.
- `plugin_setup.rs` opens first-sight screens from `App.first_sight` and saves or defers
  through new `Effect::Bridge(BridgeCall)` variants, run by `addon_runner.rs`.
  `extensions_view.rs` plugin rows read `App.plugin_rows`/details via the same cache.

Tests:
- The ported `State` tests (every test of the old glue, by its old name).
- `runner_asks_a_host_once_choices_are_saved` (TestCtx: records `request`s), and
  `runner_drops_the_cache_on_401` (a `tokio` HTTP stub: `serve` in `src/addons/vocdoni/test_support.rs`, moved there by P3.7).
- The manager regression set passes unchanged, the plugin test included.

Run S-unit, S-manager, S-web, S-e2e, S-check.

Commits: `refactor(vocdoni): the plugin state machine and proxy config cache live in the addon`,
`refactor(tui): the TUI keeps no plugin or proxy config state`,
`refactor(config): [plugins] belongs to the vocdoni addon`.

Interfaces: `vocdoni::state::{State, Task}`, `vocdoni::runner::Runner`,
`vocdoni::bridge::Bridge` (temporary), `HostEvent::Addon(AddonResult::Changed)`.

---

## P3.9: Vocdoni `Launch`: routing, env and plugin share in TUI, `--plain` and flags (spec "Launches", "The Vocdoni addon / Launch"; AC7, AC8)

Model: strong   Files: src/addons/vocdoni/launch.rs (new), src/addons/vocdoni/mod.rs,
src/app/launch/ (no change expected), src/tui/{app.rs, mod.rs, sessions.rs, wizard.rs,
plain.rs, plugin_setup.rs, ui/mod.rs, ui/status.rs, stats_view.rs (profile lookup)}, src/plain.rs,
src/main.rs, src/addons/vocdoni/proxy/{resolve.rs, env.rs}, src/proto.rs, tests/manager.rs

**Runs as two tasks (F14), each ending green:**
- **P3.9a** (strong) `impl Launch for Vocdoni` and its unit tests (unused by the TUI yet). Run
  S-unit, S-check.
- **P3.9b** (strong) TUI, `--plain`, main.rs, deletions, manager tests. Run the full set below.

Goal: every TUI and `--plain` launch goes through `app::launch`. The proxy path in the TUI is
gone.

`src/addons/vocdoni/launch.rs` (`impl Launch for Vocdoni`):
- `flags()`: `--proxy NAME` (value), `--no-proxy`, `--first-party` (label "First-party session").
- `select(given)`: today's `split_launch_flags` semantics (`proxy/resolve.rs:47-68`): `--proxy
  N` → `Pick::Option{vocdoni, N}`, `--no-proxy` → `Pick::Direct`, `--first-party` → flag
  `first-party`. `--proxy` without a name is "--proxy requires a profile name".
- `options(ctx)`: one per profile (group "Proxy", `default` for the configured default, detail
  "first-party" when the profile is). The `env` profile appears when `CLAUDIO_PROXY_URL` is set
  (`load_proxy_profiles` semantics).
- `resolve`: `Pick::Default` gives `Required(default profile, flags)` when a default exists,
  else `Off`. `Pick::Option{vocdoni, p}` gives `Required` when `p` exists, else
  `Err("proxy profile 'p' not found (was it deleted?)")` (and the `env` message of
  `proxy_env_for`). `Pick::Direct` and other addons' options give `Off`.
- `prepare` (Required only):
  1. Find the profile (`Failed` if gone).
  2. Config: cached (5 min) or fetched with a 5 s timeout. A failure keeps the built-in model
     defaults ("unreachable config → built-in model defaults stays inside the addon"), and the
     routing stays the proxy.
  3. Plugin share for New/Resume (Respawn/Recover: what is answered now, no wait): `plugin_parts`
     → asks via `ctx.request` → wait ≤ 20 s → `plugins::state::compose`. The parts become
     `LaunchArg::Optional { --agents/--mcp-config, json }`, env (the `[env]` layers then secrets,
     then the profile env, today's `merge` order), `instructions`, `notices`. With
     `!ctx.host_has(host)` the plugin share is skipped with the notice "daemon on <host> is
     older: plugins after `claudio daemon restart`" (spec mixed-version row 2).
  4. `env_remove`: `ANTHROPIC_API_KEY`, plus `CLAUDE_CODE_USE_GATEWAY` when first-party (the
     profile's own `first_party` or the `first-party` flag).
  5. Unsaved plugins are left out as today (the review comes in P3.11).
- `adopt_legacy`/`legacy`: `proxy: Some(p)` ↔ `LaunchChoice { vocdoni, p, flags:
  ["first-party"] if first_party }`.

TUI:
- `App` loses `proxy_profiles`, `proxy_default`, `proxy_override`, `first_party`
  (`app.rs:326-333`, and `AppConfig` `:411-417`). It gains `launch_default: LaunchSelect` (from flags) and `launch_options:
  Vec<(&'static str, LaunchOption)>` (refreshed on `Changed` via `ui`-free `launch.options(ctx)`
  called by the runner). `SessionView.proxy/first_party` (`sessions.rs:39-42`) become `launch:
  Option<LaunchChoice>`. `to_state`/recovery convert to and from `SavedSession.proxy/first_party`
  through `Launch::legacy/adopt_legacy` (state.json unchanged until P3.19). A `proxy: Some(_)`
  that no addon adopts fails that session's launch (`Step::Failed` with a notice), never
  direct (F4, the rule P3.19 also applies).
- `AddonResult::Options(Vec<(&'static str, LaunchOption)>)` (F11): on `RuntimeOut::Changed` and
  at start the runner collects every Launch addon's `options(ctx)`, cleans them
  (`addon::clean::option`) and sends them; `App::on_launch_options` stores them and re-filters
  the wizard if it is open.
- The wizard (`wizard.rs:436-554`, `:588`, `:707`, `:886-900`) shows one row per option group
  ("Proxy: default (work) / direct / work / lab", Left/Right) built from `launch_options`. Its
  outcome is `Spawn { cwd, resume, select: LaunchSelect }`.
- `send_spawn`/`reset_session`/recovery always emit `Effect::Launch`. `send_with_proxy`,
  `Effect::SpawnWithProxy`, `wait_manifests` and `ReplyTo::deferred/SpawnedDeferred` (once
  unused) are deleted (F2). `Msg::merge_env`, `with_plugin_args`, `with_instructions`,
  `plugins::state::with_plugins` and `src/envmerge.rs` stay until P3.10 (the web's last use).
- `--plain` (`src/plain.rs`, `tui/plain.rs:54-80`): flags via `app::launch::split_flags`, start
  via `Effect::Launch` with `Frontend::Plain`. `plain_pending` keeps only the claude-install wait
  (`Option<PlainStart>`). The plugin wait (`plugin_setup.rs:322-366`) goes, because `prepare`
  waits for manifests itself.
- `main.rs:191-208`: `claudio <launch flags>` is any leading flag in
  `addons.launch_flags()` → `split_flags` → `tui::run(addons, select)`. The error text for extra
  args stays.
- `proxy::resolve::{split_launch_flags, LaunchFlags, ProxyChoice}` and `proxy_env_for` lose their
  core callers (the Vocdoni launch uses its own copy).
- The stats popup and the status bar (still old until P3.12) read `SessionView.launch`'s profile
  where they read `v.proxy`.

Tests:
- `vocdoni::launch` unit tests: `default_profile_is_required`,
  `unknown_profile_fails_never_direct` (AC8), `unreachable_config_keeps_the_proxy_with_builtin_models`
  (AC8, an unbound port), `first_party_removes_the_gateway_var_and_skips_gateway_env`,
  `plugin_share_becomes_optional_args_secrets_and_instructions`,
  `an_old_daemon_gets_the_proxy_env_without_plugins_and_a_notice`,
  `respawn_waits_for_no_manifest`.
- `tests/manager.rs`:
  - `test_new_client_old_daemon_gets_proxy_env_without_plugins` (AC8 row 2): harness with
    `CLAUDIO_TEST_OLD_DAEMON=1` on the daemon, the fake proxy's plugin, choices saved. The
    session env has `ANTHROPIC_AUTH_TOKEN`, no plugin secret and no `ANTHROPIC_API_KEY` (the old
    scrub), and the screen shows "plugins after `claudio daemon restart`".
  - `test_default_profile_with_unreachable_config_starts_through_the_proxy` (AC8): config.toml
    default profile on `http://127.0.0.1:1`, Alt+n, start. The env has `ANTHROPIC_BASE_URL` of
    that profile, never direct.
  - The regression set passes unchanged (plain env, first-party, no-proxy, plugin manifest test).
  - `an_unadopted_legacy_choice_fails_the_launch_never_direct` (unit, F4): a registry without an
    adopter and a recovered `SavedSession { proxy: Some("work") }` → no `Effect::Launch`, a
    notice.
- Replaced unit tests (`src/plain.rs` `split_args` tests, `tui/plain.rs` tests on `ProxyChoice`)
  are named in the commit message with the tests that now cover them (F21).

Run S-unit, S-manager, S-web, S-e2e, S-shots, S-check. `run_effect` and `event_loop` shrink;
update the baseline down.

Commits: `feat(vocdoni): the addon's launch: routing, proxy env and plugin share`,
`refactor(tui): every launch goes through the composer`, `refactor: claudio's launch flags come
from the addons`.

Interfaces: `impl Launch for Vocdoni`, `SessionView.launch`, `App.launch_options`,
`App.launch_default`, `AddonResult::Options`.

---

## P3.10: the web onto the addon (protocol v1 unchanged) (spec G4, AC5)

Model: strong   Files: src/web/{spawn.rs, plugins.rs, proxy.rs, hub.rs, ext.rs, mod.rs, ws.rs,
state.rs}, tests/web.rs (only if an assertion's envelope changes)

Goal: the web has no plugin state machine and no proxy cache of its own. Spawns and respawns go
through `app::launch`. The v1 ops are answered from the addon.

- `web/spawn.rs`: `spawn_claude`/`respawn` build a `LaunchSelect` from the v1 `ProxyArg` and
  `first_party` (`default` → `Pick::Default`, `direct` → `Pick::Direct`, `profile` →
  `Pick::Option{vocdoni, name}`; this mapping is the web's v1 adapter, deleted in P3.16) and call
  `app::launch::run(Frontend::Web)`. `skip_plugins` becomes `deferred` on a first pass.
  `with_proxy`/`proxy_env`/`resolve` go, and with them `Msg::merge_env`, `with_plugin_args`,
  `with_instructions`, `plugins::state::with_plugins` and `src/envmerge.rs` (F2; the Vocdoni
  `prepare` carries its own ten-line copy of the env layering, since `envmerge` is not a
  documented core service). "proxy unknown" stays for a respawn without a known
  meta. `FIRST_PARTY` ("first-party needs a proxy profile") is checked against the resolved
  routing.
- A8 (F1): in the same commit as any `ws.rs` change, the `proxy.*`/`plugins.*` arms of
  `dispatch` (`ws.rs:758-830`) move into `fn dispatch_v1(…)` in `src/web/plugins.rs` (deleted in
  P3.16).
- `web/plugins.rs` (880 production lines, 1016 with tests) shrinks to the v1 DTO functions (`list_dto`, `items_dto`,
  `status_dto`), fed by the bridge (extended with `apply_states() -> Vec<(host, name, Apply)>`
  and `items(name)`). `plugins.save` → `Bridge::save_choices`. The `plugin_status` event comes
  from `RuntimeOut::Changed{vocdoni}`: the hub diffs the bridge's apply states and broadcasts the
  changed ones.
- `web/proxy.rs` keeps the stats/credential/switch v1 handlers but takes the profile from the
  session's `launch` (server state). `ProxyCache` goes (the addon's state has the config cache).
- `web/ext.rs`: `watch_config` keeps the built-in/external registry re-read and leaves plugins
  and profiles to the runtime's `ConfigChanged`. The `profiles` event is sent by the hub on
  `Changed{vocdoni}` when `launch_options` change. `crate::config::Config.plugins` reads go.
- `SessionMeta` keeps `proxy`/`first_party` on disk; the hub maps them through
  `adopt_legacy`/`legacy` (P3.19 adds `launch`).

Tests: tests/web.rs regression set passes unchanged. Add `web_spawn_uses_the_composer_env_remove`:
an inherited `ANTHROPIC_API_KEY` is gone with a proxy and kept direct (env-recording fake).

Run S-unit, S-web, S-e2e, S-manager, S-check.

Commits: `refactor(web): spawns go through the launch composer`, `refactor(web): plugin and
proxy state come from the vocdoni addon`.

---

## P3.11: the review slice: checklists (spec "Review"; G4, AC5)

Model: strong   Files: docs/addons.md (commit 1: offer and the review flow, if changed),
src/addons/vocdoni/ui.rs (new: `impl Ui for Vocdoni` checklist parts), src/addons/vocdoni/launch.rs
(NeedsReview), src/addons/vocdoni/state.rs, src/tui/{checklist.rs (new), ui/checklist.rs (new),
addon_runner.rs, app.rs, interaction.rs, confirm.rs, plain.rs, extensions_view.rs},
`git rm src/tui/plugin_setup.rs src/tui/ui/plugin_setup.rs`, src/app/launch/ (only if needed),
src/web/spawn.rs (v1: NeedsReview → left out)

Goal: plugin review is an addon checklist shown by one generic TUI view. The launch path reviews,
and first sight offers.

Vocdoni:
- `checklist(id = plugin name)`: title `Set up <name> <version>` (today's text), intro, one item
  per manifest item (id, label as today's line text, e.g. `kb (http https://kb.example.org/mcp)`,
  `reviewer: Reviews`; `section` = the manifest section heading; `on` = not in saved
  `disabled`; first sight: all on). `revision` = the manifest sha256.
- `save_checklist(id, Save{revision, off})`: stale revision → `Err("stale")`. Otherwise
  `choices_for` (P2: `plugins/choices.rs`) → `State::save_choices` (edit_config + re-ask hosts).
  `Defer` → `State::defer`.
- `prepare` (New/Resume): an enabled plugin of the profile without saved choices and not deferred
  (state or `req.deferred`): wait ≤ the remaining budget for its manifest, then
  `Err(NeedsReview{ checklist: name, revision: sha })`. Manifest failed → left out with a notice.
- On a manifest arriving at first sight: `Task::Offer(name)` → `ctx.offer_checklist(name)`.

TUI:
- `src/tui/checklist.rs` (pure): `ChecklistView { addon, checklist: Checklist, selected, armed:
  u8 (ARM_TICKS 4), origin: Origin }` with `enum Origin { Launch { id: u64 }, Offer, Row { back:
  ExtensionsView } }`. Keys as today's setup screen (`plugin_setup.rs` header): Up/Down/j/k, Space,
  `a`, `n`, Enter (Save), Esc. Esc on Launch → Cancel; on Offer → Defer; on Row → close. `s` on
  Launch → Skip. The hint line names them. It is queued in `App.confirms` like a confirm
  (`Queued`). A revision change while open (a new `Checklist` for the same id) replaces the items
  (today's `PluginSetup::replace`).
- `src/tui/ui/checklist.rs`: draws title, intro, section headings, `[x]`/`[ ]` label, detail dim.
- `Modal::Checklist(Box<ChecklistView>)` replaces `Modal::PluginSetup`.
  `Effect::Addon(AddonCall)` lands with `AddonCall::{OpenChecklist{addon, id, origin},
  SaveChecklist{addon, id, answer}, ReviewAnswer{launch: u64, answer: ReviewAnswer}}`, and
  `HostEvent::Addon(AddonResult::{Checklist{addon, origin, checklist}, Offer{addon, id},
  Changed{addon}, Notice(Notice)})`. `addon_runner` keeps parked launches (`HashMap<u64,
  Parked>`, at most 8, dropped at exit) and turns `Step::Review` into `OpenChecklist{origin:
  Launch}`.
- `plugin_setup.rs` and `ui/plugin_setup.rs` are deleted. So are `App::first_sight`, the bridge's
  `first_sight/save_choices/defer` and the old `Effect::Bridge` variants for them.
  `plain_pending` stays install-only.
- Web (v1): `Step::Review` → `resume(Skip)` with a notice "plugin <name> left out: set it up
  first" (today's server behaviour); the browser gate (`spawn-hooks.js`) is unchanged until
  P3.15.

Tests:
- `tui::checklist` unit tests: the keys, arming, Esc per origin, replace on revision change.
- `vocdoni::ui` tests: `checklist_lists_manifest_items_with_saved_choices`,
  `a_stale_revision_is_refused`, `prepare_needs_review_for_an_unsaved_plugin`,
  `deferred_skips_review_for_that_launch_only`, `first_sight_offers_once`.
- `tests/manager.rs`: `test_plugin_manifest_setup_then_sessions_get_what_was_kept` passes
  unchanged (the offer opens the screen after the wizard's Esc, with the same texts). Add
  `test_a_launch_reviews_an_unsaved_plugin_and_skip_starts_without_it` (Esc the offer, Alt+n,
  start; the checklist opens; `s` starts the session without the plugin secret; the next launch
  asks again).
- `--plain` with an unsaved plugin: `test_plain_reviews_before_starting` (Enter saves, the
  session gets the plugin).
- Replaced unit tests (`plugin_setup.rs` tests, among them
  `plain_waits_for_a_new_manifest_and_its_setup_then_spawns` and
  `setup_screens_wait_their_turn_with_the_prompts`) are named in the commit message with the
  tests that now cover them (F21).

Run S-unit, S-manager, S-web, S-e2e, S-check.

Commits: `feat(vocdoni): plugin review as an addon checklist`, `feat(tui): one checklist view
for addon reviews`, `refactor(tui): remove the plugin setup screen`.

Interfaces: `tui::checklist::ChecklistView`, `Modal::Checklist`, `Effect::Addon(AddonCall)`,
`HostEvent::Addon(AddonResult)`, `impl Ui for Vocdoni` (checklists).

---

## P3.12: segments, the panel and actions (spec "Ui", "Data, not widgets"; AC5, AC9)

Model: strong   Files: src/addons/vocdoni/{ui.rs, panel/ (new: mod, pages, blocks), state/ (stats, creds)},
src/addons/vocdoni/proxy/stats.rs, src/tui/{panel_view.rs (new), ui/panel.rs (new), ui/status.rs,
ui/mod.rs, app.rs, interaction.rs, keymap.rs, plain.rs, addon_runner.rs, mod.rs}, `git rm
src/tui/stats_view.rs src/tui/ui/stats.rs src/tui/proxy_state.rs`, tests

**Runs as three tasks (F14), each ending green:**
- **P3.12a** (strong) Vocdoni segments, panel and action with their unit tests
  (`src/addons/vocdoni/panel/{mod,pages,blocks}.rs`; unused by the TUI yet). Run S-unit,
  S-check.
- **P3.12b** (strong) `PanelView`, `ui/panel.rs`, keymap `Action::Panel`, `Modal::Panel`; the
  stats popup is replaced. Run S-unit, S-manager, S-check.
- **P3.12c** (strong) status segments, then deletion of `stats_view.rs`, `ui/stats.rs`,
  `proxy_state.rs` and the App proxy fields. Run the full set below.

Goal: the proxy stats popup is the Vocdoni panel drawn by a generic TUI panel view. The status
bar's proxy, credential and 5h/7d parts are Vocdoni segments.

Vocdoni:
- `State` gains `stats: HashMap<profile, ProxyStatus>` (from `tui/proxy_state.rs`, with `apply`,
  `cached_windows`), `creds: HashMap<SessionId, SessionCred>` and `wanted` (focused sessions). On
  `Tick`, for focused vocdoni sessions with a conversation, `Task::FetchCred` when older than 30 s
  (5 s after a conversation change), with today's stale check by conversation id
  (`app.rs:554-610`). `Task::FetchStats{profile, windows, models}` runs with a 10 s timeout.
- `segments(session)`: for a session whose `launch.addon == vocdoni`: `⇄ proxy:<profile>`
  (priority 10, Accent), credential `work-max (max)` and switch notice (20/21), `5h NN%` (110,
  tone by %), `7d NN%` (150). The texts are today's `status.rs:145-170` strings. A focused direct
  session gets nothing.
- `panels()`: `[PanelDecl { id: "proxy", title: "Proxy stats", key: Some(PanelKey { action:
  "proxy_stats", default: "alt+s" }), windows: &["24h", "7d", "30d"] }]`.
- `panel(req)` (`panel/`): the profile is the session's (`req.sid` → snapshot) else the default
  else the first. With no profile: one page whose Text block is "No proxy configured. …" (today's
  `ui/stats.rs::no_proxy_lines`, so `test_plain_keeps_proxy_stats_and_reset` passes). Otherwise
  it fetches what the page needs (Trends: all windows; others: `req.window`) and builds the five
  pages (Overview, Models, Trends, Pool & limits, Sessions) as Blocks from `ui/stats.rs`'s
  content: KeyValues for totals, Table for models and sessions, Gauge for limits and the 5h/7d
  windows, Bars for trends. The title is `Proxy stats: <profile>`, `updated` the fetch time, and
  actions are `[PanelAction { id: "switch", label: "move to another subscription", key:
  Some('n'), confirm: Some("Move this conversation to another subscription?") }]` when the
  session is on the proxy with a conversation.
- `action({panel: "proxy", action: "switch", sid})`: the profile and conversation come from the
  addon's own snapshot of `sid`, never from the request. Today's `switch_subscription` →
  `Notice`.

TUI:
- `src/tui/panel_view.rs` (pure): `PanelView { addon, decl: PanelDecl, req: PanelRequest, panel:
  Option<Panel>, scroll, max_scroll: Cell<u16>, loading, gen }`. It keeps today's keys
  (`stats_view.rs:142-167`): Esc/q, Left/Right/Tab/h/l, 1-9, `w` (next of `decl.windows`), `r`,
  j/k, PgUp/PgDn/space, g/G. A panel action's key asks its `confirm` through the confirm queue,
  then emits `AddonCall::Action`. Its outcomes are `AddonCall::Panel{gen, addon, req}`.
- `addon_runner` (F13): `AddonCall::Panel` runs `ui.panel(req, ctx)` under `timeout(10 s)`; a
  timeout or panic becomes `clean::error_panel(title, "timed out")`; `AddonCall::Action` 15 s; a
  reply whose `gen` is older than the view's is dropped (A10).
- `src/tui/ui/panel.rs`: draws any `Panel`: header (title, pages with the current one marked,
  window), body (Text → wrapped lines with tone; KeyValues → aligned `k  v`; Table → columns
  sized to content; Gauge → `label [■■■□□] used/limit resets`; Bars → one bar per series),
  footer (updated N s ago, action keys, scroll). It is pure and uses `tui::fmt`.
- `ui/status.rs`: addon segments per session come from `App.segments: HashMap<SessionId,
  Vec<(addon, Segment)>>`, refreshed by `AddonResult::Segments` (the runner calls
  `ui.segments(snap)` for each session on `Changed` and on activation). Line 1 takes priorities
  0-99, line 2 100-255 merged with the core segments by priority (D9). `credential_segments`,
  `five_hour_segment`, `seven_day_segment` and the `SessionCredential` import go.
- `keymap.rs`: `Action::ProxyStats` becomes `Action::Panel(u8)`, with bindings built from every
  addon's `PanelDecl.key` (default spec, overridable by `[keys] <action>`, collision-checked like
  the table). `PLAIN_ACTIONS` keeps every panel action. `keymap.rs:439` (help line) is generic.
- `App` loses `proxy_status`, `models_requested`, `session_creds`, `open_proxy_stats`,
  `stats_key`, `on_proxy_stats`, `refresh_session_cred`, `on_session_credential`,
  `on_switch_result`, `schedule_proxy_stats`, the `Effect::{FetchProxyStats,
  FetchSessionCredential, SwitchSubscription}` and their `HostEvent`s. `Modal::ProxyStats`
  becomes `Modal::Panel(Box<PanelView>)`.
- `src/tui/fmt.rs` keeps only generic helpers; proxy-specific formatters move to
  `vocdoni/panel/`.

Tests:
- `vocdoni::panel` unit tests: each page's blocks from fixture `ProxyStatus` (the numbers of
  today's `ui/stats.rs` tests), `no_profile_says_no_proxy_configured`,
  `switch_uses_the_sessions_own_profile_and_conversation`, `the_switch_action_needs_a_conversation`.
- `vocdoni::ui` segment tests (today's `status.rs` credential tests, as Segments).
- `tui::panel_view` key tests (today's `stats_view.rs` tests, generic), `tui::ui::panel` render
  tests (TestBackend: KeyValues, Table, Gauge, Bars; long text clipped).
- `keymap` tests: `panel_keys_come_from_addons_and_are_collision_checked`,
  `proxy_stats_override_still_rebinds_the_panel` (`keymap.rs:1015-1029` adapted).
- manager: `test_plain_keeps_proxy_stats_and_reset` unchanged; `test_proxy_env_injection` (gated)
  still sees `proxy:env` on the status bar.
- `tests/screenshots.rs` `stats.png` still opens with Alt+s (gated).
- Replaced unit tests (`stats_view.rs`, `ui/stats.rs`, the `status.rs` credential tests) are
  named in the commit message with the tests that now cover them (F21).

Run S-unit, S-manager, S-web, S-e2e, S-shots, S-check.

Commits: `feat(vocdoni): proxy segments, the proxy panel and the switch action`, `feat(tui):
generic panel view and addon status segments`, `refactor(tui): remove the stats popup and proxy
state`.

Interfaces: `Modal::Panel`, `tui::panel_view::PanelView`, `Action::Panel`, `App.segments`,
`AddonCall::{Panel, Action}`, `AddonResult::{Panel, ActionDone, Segments}`.

---

## P3.13: extension rows; the bridge goes (spec "Ui: extension_rows, row_action"; AC5)

Model: standard   Files: src/addons/vocdoni/{rows.rs (new), ui.rs}, `git rm
src/addons/vocdoni/bridge.rs`, src/app/extensions/{mod.rs, …}, src/tui/{extensions_view.rs,
ui/extensions.rs, app.rs, addon_runner.rs}, src/web/{plugins.rs, ext.rs, hub.rs}

Goal: addon rows on the extensions screen. `Source::Plugin` and `Registry::set_plugins` are gone,
and so is the bridge.

- Vocdoni `extension_rows`: one row per admitted plugin of every profile's list (`name`,
  `description`, `default_on: true`, `refused`, `details` = today's `plugin_details` lines
  (`plugins_glue.rs`), `setup: Some(name)`). `row_action(name, Toggled(on))` →
  `State::set_enabled` (re-ask hosts or not, as today). `Refresh` → `State::refresh`.
- The core registry (`app/extensions`) replaces `Source::Plugin { profile, refused }` with
  `Source::Addon { addon: &'static str, refused: Option<String> }` and `set_plugins` with
  `set_addon_rows(rows: &[(&'static str, ExtensionRow)], enabled: &BTreeMap<String, bool>)`
  (name collisions with built-ins/externals are refused as today).
- Toggle (core, D10): write `[extensions] name = on` (`config::save_extension`), update the
  runtime's config (`config_changed`), then `AddonCall::RowAction{Toggled}`. `r` on the screen →
  `RowAction::Refresh` for every addon with rows. Enter on an addon row with `setup` →
  `OpenChecklist{origin: Row}`.
- `AddonCtx::extension_on` is answered from the runtime's config.
- Web (v1): `extensions.list`'s plugin rows and `plugins.list` come from the addon's rows and
  checklists. `extensions.toggle` on an addon row goes through the same core path. `plugins.items`
  comes from `checklist(name)`, `plugins.save` from `save_checklist`, and `plugin_status`/
  `plugins.status` from a temporary `Vocdoni::apply_states()` reached through
  `Ui::extension_rows` details. If the v1 `plugin_status` shape cannot be rebuilt from rows,
  `src/web/plugins.rs` keeps one adapter function `status_from(addons)` that calls a
  `pub(crate)` Vocdoni accessor; it is listed in the A3 exceptions until P3.16 deletes it.
- The bridge and `Addon::as_any` are deleted (F9). `grep -rn 'addons::vocdoni' src
  --include=*.rs | grep -v '^src/addons/'` must show only `src/web/plugins.rs`'s adapter (until
  P3.16), and `grep -rn '"vocdoni"' src/remote` only `src/remote/diag.rs` (diag, until P3.17; F8).
- The web's `ext.rs` `edit` lock goes (every writer uses `config_edit`'s lock, F5), and the
  runtime's `watch_config` flips to `true` with `ext.rs`'s 5 s poll deleted (F20).

Tests: `vocdoni::rows` tests (the old `plugin_details` tests as row details),
`app::extensions` tests for `set_addon_rows` collisions, extensions_view tests adapted to
`Source::Addon` (the screen's texts unchanged). Manager `test_plugin_manifest_setup_then_
sessions_get_what_was_kept` and `test_extensions_screen_turns_extensions_off_and_saves_it`
unchanged; web `web_extension_toggle_edits_config_and_is_reread` unchanged.

Run S-unit, S-manager, S-web, S-e2e, S-check.

Commits: `feat(vocdoni): plugin rows through the addon API`, `refactor(app): addon rows replace
plugin rows in the extension registry`, `refactor(vocdoni): remove the TUI bridge`.

---

## P3.14: web protocol v2: docs, then the server's `addons.*` ops (additive) (spec "Web protocol v2"; B2, C5)

Model: strong   Files: docs/web-protocol.md (commit 1), src/web/{addons.rs (new), ws.rs, hub.rs,
dto.rs, mod.rs}, tests/web.rs

**Runs as three tasks (F14), each ending green:**
- **P3.14a** (strong) the docs commit (B2). Run S-check.
- **P3.14b** (strong) `addons.*` read ops (`addons.list`, `addons.panel`, `addons.rows`,
  `addons.checklist`), the hub's segments and `addons.changed`, the Session DTO additions, and
  their tests. Run S-unit, S-web, S-e2e, S-check.
- **P3.14c** (strong) `session.spawn`/`respawn` `launch`/`defer`/`review`, `addons.action`,
  `addons.checklist.save`, `addons.row_action`, and their tests. Run S-unit, S-web, S-e2e,
  S-check.

Commit 1 (docs, B2): docs/web-protocol.md becomes "version 2" with:
- §3.1: hello `v: 2`; the snapshot drops `profiles`.
- §4 Session: drops `proxy`, `proxy_known`, `first_party`; adds `launch` (D12) and
  `segments: [{"addon","id","text","tone","priority"}]`. Host `features` lists `addon:<name>`.
  New DTOs: AddonDecl `{"name","panels":[{"id","title","windows":[s],"key":s|null}],
  "launch_options":[LaunchOption],"launch_flags":[{"name","label"}]}`, Panel, Block (the five
  kinds, §4), PanelAction, Checklist, ChecklistItem, ExtensionRow `{"addon","name","description",
  "on","refused","details":[s],"setup":s|null}`, Notice `{"text","level"}`, LaunchSelect,
  LaunchChoice.
- §5: `session.spawn` takes `launch` (LaunchSelect; default `{"pick":"default"}`) and `defer?:
  [{"addon","id"}]` instead of `proxy`, `first_party`, `skip_plugins`, and may answer
  `{"review":{"addon","id","revision"}}` (D11). `session.respawn` takes `launch?` instead of
  `proxy?`/`first_party?`; "launch unknown" replaces "proxy unknown". New ops `addons.list`,
  `addons.panel {addon,id,page?,window?,sid?}` (10 s; an error page on timeout), `addons.action
  {addon,panel,action,sid?}` (15 s), `addons.checklist {addon,id}`, `addons.checklist.save
  {addon,id,revision,answer: {"off":[ids]} | "defer"}` ("stale" on a revision change),
  `addons.rows`, `addons.row_action {addon,name,action:"toggle"|"refresh",on?}`. Removed:
  `proxy.stats`, `proxy.credential`, `proxy.switch`, `plugins.list/items/save/status`.
- §6: new `addons.changed {addon}`; removed `plugin_status`, `profiles`.
- §7: every addon string is a text node; Panel blocks render as text, tables and SVG-free bars
  (CSS widths).
- A compatibility note: P3.14-P3.15 serve both op sets under `v: 1`; P3.16 switches to `v: 2`.

Commit 2 (code, additive; hello stays `v: 1` until P3.16):
- `src/web/addons.rs`: arg structs (`deny_unknown_fields`; `addon` must be a registered
  addon's name, `id`/`name` `[a-z0-9-]{1,64}`, `answer.off` ≤ 256 ids of ≤ 128 bytes), handlers
  calling the runtime's addons with the timeouts above, results through `addon::clean`, JSON per
  §4. `sid` must name a known session.
- `dispatch` (`ws.rs:476`) routes `addons.*` to it. `session.spawn`/`respawn` accept `launch`/
  `defer` beside the v1 args (v1 args map as in P3.10; giving both is "bad request").
  `Step::Review` answers `{"review": …}` when the request used `launch`, else the v1 auto-skip.
- `hub.rs`: `RuntimeOut::Changed{addon}` → `addons.changed` to every tab, and recompute the
  segments of every session (`ui.segments(snap)`), sending a `session` event when they differ
  (cached per sid). `RuntimeOut::Notice` → `notice`. The Session DTO (`dto.rs:74-140`) gains
  `launch` and `segments` beside the v1 fields until P3.16.
- Focus: `set_focused` with the sids that any tab has attached.

Tests (`tests/web.rs`, fake claude + fake proxy):
- `web_addons_list_has_vocdoni_panels_and_launch_options`
- `web_addons_panel_renders_the_fixture_and_never_leaks_the_token` (the `proxy.stats` fixture
  through `addons.panel`; the token check of
  `web_spawn_keeps_proxy_and_plugin_secrets_out_of_the_browser`)
- `web_spawn_with_launch_needs_review_then_defer_spawns_without_the_plugin` (D11)
- `web_checklist_save_then_spawn_gets_the_plugin`, `web_checklist_save_refuses_a_stale_revision`
- `web_addons_action_switch_uses_server_state` (sid only; a forged `profile` arg is "bad
  request")
- `web_addons_ops_refuse_unknown_addons_and_bad_args`
- `web_session_dto_has_launch_and_segments`
- The v1 regression set passes unchanged.

Run S-unit, S-web, S-e2e (the page is unchanged and still v1), S-check.

Commits: `docs(web): protocol v2: addons.* ops, launch and segments`, `feat(web): addons.* ops
and launch selects (beside v1)`.

---

## P3.15: the page on `addons.*` (spec "Web protocol v2"; F3, C5)

Model: standard   Files: web/app/{model.js, launch.js (new), wizard.js, info.js, proto.js
(unchanged v), spawn-hooks.js (deleted)}, web/app/ui/{panel.js (new), checklist.js (new),
segments.js (new), wizard.js, session-tab.js, session-actions.js, strip.js, palette.js,
extensions.js, app.js, topbar.js, stats.js (deleted), plugin-setup.js (deleted)},
web/styles/components.css, web/tests/{launch.test.mjs (new), model.test.mjs, wizard.test.mjs},
web/e2e/tests/proxy.spec.mjs

**Runs as three tasks (F14), each ending green:**
- **P3.15a** (standard) the components (`panel.js`, `checklist.js`, `segments.js`, `launch.js`)
  and their node tests. Run S-node, S-e2e, S-check.
- **P3.15b** (standard) the wizard, session actions and topbar on them. Run S-node, S-e2e.
- **P3.15c** (standard) the deletions and `proxy.spec.mjs`. Run S-node, S-e2e, S-web, S-check.

Goal: the page uses only `addons.*` and the v2 session args. It has generic panel, checklist,
segment and launch-option components following DESIGN.md.

- `launch.js` (pure, node-tested): `optionsByGroup(decls)`, `selectFrom(value, flags)`,
  `labelFor(launch)`, `reviewLoop(spawn, ask)` (spawn → review → checklist → save → spawn again /
  defer / cancel).
- `ui/panel.js`: a Dialog with page tabs, window buttons (`data-testid="panel-window-<w>"`),
  blocks as text nodes (`panel-text`, `panel-kv`, `panel-table`, `panel-gauge`, `panel-bars`) and
  actions (a `confirm` goes through `modal.js`). It re-fetches on `addons.changed` while open.
  The palette lists each addon panel ("Proxy stats") and its actions.
- `ui/checklist.js`: title, intro, sections, checkboxes (`checklist-item`), All/None, Save
  (`checklist-save`), Skip (`checklist-skip`, launch reviews only), Cancel. It is the review step
  of the wizard and the setup of an addon row (`extensions.js`).
- `ui/segments.js`: chips by priority with tone tokens (`data-testid="segment"`,
  `data-segment="<addon>/<id>"`), in the topbar (focused session) and the strip.
- The wizard renders one select per `LaunchOption.group` (`data-testid="wizard-launch-<group>"`;
  the Proxy group keeps `data-testid="wizard-proxy"`) and one checkbox per labelled flag
  (`wizard-first-party` kept). It sends `launch`.
- session-actions' respawn chooser lists the launch options (`proxy-chooser`/`proxy-choice`
  testids kept). Info rows show "Launch".
- Deleted: `stats.js`, `plugin-setup.js`, `spawn-hooks.js`, `model.profiles`, `model.plugins`,
  the `plugin_status`/`profiles` handlers.
- DESIGN.md: Graphite surfaces, 1px Slate hairlines, mono labels, Phosphor Green only for
  active/status, tones from tokens (Ok green, Warn violet, Hot coral, Muted steel, Accent teal).
  No new colours outside `tokens.css`.
- `proxy.spec.mjs`: the plugin test uses `checklist`/`checklist-skip` and the stats test uses
  `panel-window-24h`, `panel-bars` and `panel` (testids renamed with the components; the commit
  says so; same flows and numbers, `5.6k` still asserted).

Run S-node, S-e2e (all specs; zero CSP violations), S-web, S-check.

Commits: `feat(web): generic panel, checklist and segment components`, `refactor(web): the
wizard and session actions use launch options`, `refactor(web): remove the stats and plugin
setup screens`.

---

## P3.16: web v2 only: v1 ops removed, `v: 2` (spec AC5; B2, C5)

Model: strong   Files: `git rm src/web/proxy.rs src/web/plugins.rs`, src/web/{ws.rs, spawn.rs,
hub.rs, dto.rs, ext.rs, mod.rs, state.rs}, web/app/proto.js (`PROTO_VERSION = 2`),
web/tests/proto.test.mjs, tests/web.rs

- The `proxy.*`/`plugins.*` arms, the v1 `ProxyArg`/`first_party`/`skip_plugins` args, the v1
  Session fields, the `profiles` snapshot field and event, the `plugin_status` event and the
  P3.13 adapter are deleted. `spawn.rs` keeps its arg checks and the composer call. `ws.rs:128`
  says `"v": 2`.
- Tests (contract changed, said in the commit): `web_spawn_keeps_proxy_and_plugin_secrets_out_of_
  the_browser` walks the v2 ops (`addons.list`, `addons.rows`, `addons.checklist`, `addons.panel`)
  instead of the v1 ones, with the same leak assertions on every frame, page and log line.
  `web_respawn_without_a_known_proxy_asks` expects "launch unknown" and retries with `launch`.
  `web_plugin_without_choices_is_left_out_until_saved` uses `session.spawn {launch}` → `review`
  → `defer`, then `addons.checklist.save`, then spawn. Same env assertions.
  `web_proxy_stats_render_the_fixture` reads `addons.panel`. New:
  `web_v1_proxy_and_plugin_ops_are_unknown` and `hello_is_v2`.
- Exit check: `grep -rn 'proxy\|plugins' src/web --include=*.rs | grep -n 'crate::addons::vocdoni\|mod proxy\|mod plugins'`
  is empty.

Run S-unit, S-web, S-node, S-e2e, S-check.

Commit: `feat(web)!: protocol v2, proxy and plugin ops replaced by addons.*`.

---

## P3.17: the API and the web as command addons; main.rs is wiring (spec "The API addon", "The web addon", "Crates"; A3, A9)

Model: standard   Files: `git mv src/print src/addons/api/print`, `git mv src/api
src/addons/api/api`, `git mv src/cli.rs src/addons/api/cli.rs`, `git mv src/msglog.rs
src/addons/api/msglog.rs`, `git mv src/web src/addons/web` (the `web/` assets and `build.rs` stay
at the root; `build.rs` paths unchanged), src/addons/api/mod.rs (new), src/addons/web/mod.rs,
src/addons/web/addon.rs (new: `WebAddon`, which P3.18 extends), src/addons/mod.rs (register
`ApiAddon`, `WebAddon`), src/addons/vocdoni/diag.rs (new), src/remote/diag.rs,
src/main.rs, src/tui/mod.rs, src/daemon/mod.rs, Makefile/CI only if a path is named,
scripts/size-baseline.txt (renames)

Steps: a moves-only commit, then the code commit.
- `ApiAddon` (name `api`) implements `Commands`. `is_relay(argv)` is today's `main.rs:110-126`
  (`<Event> <port>`) or `__hook <Event>` with `argv.len() >= 3 && argv.len() != 5` (`:144`).
  `dispatch` runs `cli::parse` (`:210-308`) and handles `--api` and `-p`/`--print` (with
  msglog init), returning `None` otherwise. `help()` is `HELP_APPENDIX`.
- `WebAddon` (`src/addons/web/addon.rs`, F12) implements `Commands` for `claudio web …`
  (`run(&argv[2..], addons)`).
- `main.rs` keeps only the core words in the spec's order (`__probe`, `--slave`, diag,
  `__hook <Event> <sock> <token>`, `__context`, `--daemon` → `daemon::run(addons)`, `upgrade`,
  `daemon`, `sessions`, `--plain` → `plain::run(addons, …)`, bare → `tui::run(addons, default)`,
  launch flags → `tui::run(addons, select)`). Order: `addons.relay(argv)` first, then those, then
  `addons.dispatch(argv)`, then help (`claude --help` + `addons.help()`), then passthrough. The
  diag `__remote-manifest` becomes a Vocdoni `Commands` word under `cfg(feature = "diag")`
  (`src/addons/vocdoni/diag.rs`); `remote/diag.rs` loses it (F8).
- `main.rs::main` must end under 80 lines (baseline entry removed).

Tests: `main_routes_relays_before_core_words` (unit, with TestAddon), the existing
`tests/integration.rs` (gated) compiles, `cli` tests move with the file. Manager tests that run
`claudio --help`/passthrough pass.

Run S-unit, S-manager, S-web, S-e2e, `CLAUDIO_E2E=` (unset) `cargo test --locked --test
integration` (no-op, compiles), S-check.

Commits: `refactor(api): print and the API server move under addons/api` (moves),
`refactor(web): the web UI moves under addons/web` (moves), `refactor: the api and web addons
claim their commands; main.rs is wiring`.

---

## P3.18: the web addon's lenses and rows (spec "The web addon"; G5, AC6)

Model: standard   Files: src/addons/web/addon.rs (from P3.17), docs/extensions.md,
src/app/extensions/mod.rs (built-ins latex/diagram leave), src/claude/hooks.rs (`LATEX_CONTEXT`
stays, used by the web addon), src/daemon/mod.rs (`effective_extensions` default:
compat(v0.10)), src/tui/{extensions_view.rs}, src/addons/web/ext.rs, tests/manager.rs,
tests/web.rs

- `WebAddon` (P3.17's, name `web`, no config sections) gains `Launch` and `Ui`. `Launch`: `resolve` is always
  `Optional`. `prepare` contributes `LensSpec { latex, Tag, LATEX_CONTEXT }` when
  `ctx.extension_on("latex") != Some(false)` and `[claude] latex` is not false (read as
  `extension_on("latex")` after the runtime maps the alias: the runtime's `extension_on("latex")`
  returns `Some(false)` when `[claude] latex = false`, the documented alias), and `LensSpec {
  mermaid, Fence, DIAGRAM_CONTEXT }` when `extension_on("diagram") != Some(false)`. It also
  contributes their instructions (in the lens specs, as today). `Ui::extension_rows`: `latex`
  ("math rendered from <latex> tags in the web UI") and `diagram` ("Mermaid diagrams drawn in the
  web UI"), `default_on: true`. Toggles are handled by the core (D10).
- `app/extensions/mod.rs::builtins()` keeps `search` and `snippets` only. `DIAGRAM_CONTEXT`
  moves to `src/addons/web/addon.rs`. `Registry::session_extensions` keeps the external extensions'
  lenses (core part of `compose`). The `[claude] latex` special case (`app/extensions/mod.rs:181-183`, `:376`,
  `web/ext.rs:264`, now `src/addons/web/ext.rs`) is mapped once in the runtime's `extension_on`.
- `web/ext.rs` (`session_extensions` :97, the `extensions.list` rows) no longer has the latex
  and diagram built-ins: their rows are the web addon's (`addons.rows`), and their lenses come
  from its `prepare`. The hub's relay of the daemon's `Lens`/`Latex` events (`hub.rs:484-500`) and
  the page's KaTeX/Mermaid rendering are unchanged.
- A core-only build would ask claude for no tags, because `WebAddon` is not registered there (P4
  checks it).

Tests:
- `addons::web::addon` unit tests: lenses on by default, off by `[extensions] latex = false` and by
  `[claude] latex = false`, diagram independent.
- manager `test_tui_session_requests_the_latex_lens_from_the_web_addon` (AC6): start a session in
  the TUI and read the journal (`journal_sessions()`); `extensions.lenses` names `latex` and
  `mermaid`. Turning `latex` off on the extensions screen drops it for the next session.
- `test_latex_formula_is_shown_as_claude_printed_it` (:2255) unchanged; web
  `web_extension_toggle_edits_config_and_is_reread` unchanged; Playwright `latex-pane` and the
  reader specs unchanged.

Run S-unit, S-manager, S-web, S-e2e, S-check.

Docs: `docs/extensions.md` says the latex/diagram rows are the web addon's.

Commits: `feat(web): the web addon contributes the latex and mermaid lenses`, `refactor(app):
latex and diagram leave the core's built-in extensions`.

---

## P3.19: persistence: `launch`, with the legacy fields still written (spec "Persistence"; B3, AC8)

Model: standard   Files: src/app/state.rs, src/web/state.rs, src/tui/{app.rs, sessions.rs},
src/web/hub.rs, tests/fixtures/state-v0.8.0.json (new), tests/fixtures/web-state-v0.8.0.json
(new), unit tests

- `SavedSession` gains `#[serde(default, skip_serializing_if = "Option::is_none")] launch:
  Option<LaunchChoice>`. It keeps writing `proxy`/`first_party` derived through
  `Launch::legacy` (compat(v0.10)). On load, `launch` wins; without it,
  `ClientState::launch_of(&self, &Addons, sid) -> Result<Option<LaunchChoice>, String>` asks every
  `Launch::adopt_legacy` (F4): `proxy: None` is `Ok(None)` (direct); `proxy: Some(_)` adopted by
  one addon is `Ok(Some(choice))`; adopted by none is `Err("this session's launch option is not
  available in this build")`, which fails the launch (`Step::Failed`) and is shown as a notice,
  never a direct session (AC8). `SavedSession` keeps the raw `proxy`/`first_party` fields beside
  `launch` so an un-adopted value is written back unchanged.
  `SessionMeta` (web-state.json) gets the same. `state::known_proxy` becomes `known_launch`.
- The conversion lives in `app::state` (`launch_of` above), called at the edges. `App`/hub hold only `Option<LaunchChoice>`.
- Fixtures: a v0.8.0 `state.json` with a direct session, a `work` session and a first-party
  `lab` session, and a `web-state.json` likewise. Both copied from a real v0.8.0 run's shape,
  with no secrets.

Tests: `v080_state_json_loads_with_its_proxy_choices`, `v080_web_state_loads_with_its_proxy`,
`saving_writes_launch_and_the_legacy_fields` (a v0.8.0 reader, the old struct shape kept in the
test, reads `proxy`/`first_party` back), `an_unknown_legacy_profile_is_kept_and_fails_at_launch`
(AC8: not turned direct), `a_build_without_vocdoni_keeps_the_choice_it_cannot_adopt` (the raw
fields are preserved through load/save), `an_unadopted_legacy_choice_fails_the_launch_never_direct`
(TestAddon registry without an adopter; a `SavedSession { proxy: Some("work") }` → no
`Effect::Launch` / `Step::Failed`).

Run S-unit, S-manager, S-web, S-check.

Commit: `feat(app): sessions persist their launch choice, still writing proxy and first_party`.

---

## P3.20: `claudio vocdoni`, the proxy alias, docs and the exit checks (spec "The Vocdoni addon / Commands"; G6, F1)

Model: standard   Files: src/addons/vocdoni/cmd.rs (was proxy/cmd.rs), src/addons/vocdoni/mod.rs
(`Commands`), src/addon/mod.rs (allow removed), README.md, CLAUDE.md, docs/{addons.md, web.md,
extensions.md, print-and-api.md, daemon-protocol.md}, tests/manager.rs

- `Vocdoni` implements `Commands`. `dispatch` takes `vocdoni <sub>` and `proxy <sub>` (alias,
  same subcommands). `login [URL] [--name] [--token] [--unverified]`: without a URL it uses the
  default profile's URL, else `DEFAULT_URL` (no prompt). `status`, `use NAME|none`,
  `logout [NAME]`. `help()` lists both words. `cmd_login` (baseline 173 lines) is split under 80.
- Remove `#![allow(dead_code)]` from `src/addon/mod.rs`. Anything still unused is deleted (D4),
  or justified in the commit if a P4 consumer needs it.
- Docs (F1): CLAUDE.md's architecture map gets `addon/`, `app/{addons,launch,config_edit}`,
  `addons/{vocdoni,api,web}` (replacing the `proxy/`, `plugins/`, `daemon/manifest.rs`,
  `tui/plugins.rs`, `plugin_setup.rs`, `stats_view.rs`, `proxy_state.rs`, `web/proxy.rs`,
  `web/plugins.rs` entries), a pointer to docs/daemon-protocol.md and docs/addons.md, and the
  constraint "core code never imports `crate::addons`; addons only `crate::addon` and the listed
  services". README: `claudio vocdoni login` first, `claudio proxy` as the alias. docs/web.md:
  protocol v2 and the panel/checklist UI.

Tests: `test_vocdoni_login_token_then_a_session_gets_the_proxy_and_plugins` (AC7: the fake
proxy, `claudio vocdoni login --token` with the token on stdin and the URL given; then a new
session's env has the token, the plugin args and the instructions in its context file).
`proxy_alias_runs_the_same_subcommands` (unit). The `cmd` tests move with the file.

Exit checks (all must hold; paste the outputs into the task report):
- `grep -rn 'crate::\(addons::vocdoni\|proxy\|plugins\)' src/tui src/addons/web` → empty.
- `grep -rn '"vocdoni"' src --include=*.rs | grep -v '^src/addons/'` → empty.
- `grep -rn 'crate::addons' src --include=*.rs | grep -v '^src/addons/\|^src/main.rs'` → empty.
- `ls src/web src/proxy src/plugins src/tui/plugins*.rs src/tui/plugin_setup.rs
  src/tui/stats_view.rs src/tui/ui/stats.rs src/tui/proxy_state.rs 2>&1` → all missing.
- `grep -rn 'fn compose' src` → one definition (`src/app/launch/compose.rs`); the plugin share helper in
  `vocdoni/plugins/state.rs` is renamed `plugin_share` so `compose` has one definition (AC5).
- `grep -rn 'compat(v0.10)' src` lists the D14 items.
- `scripts/size-baseline.txt`: total smaller than at P2's end; `run_effect`, `event_loop`,
  `request` under 80 or below their old counts (AC9).

Run every suite: S-unit, S-manager, S-web, S-node, S-e2e, S-shots, S-remote (with `z6` when
reachable), S-check.

Commits: `feat(vocdoni): claudio vocdoni login|status|use|logout, with proxy as an alias`,
`refactor(addon): the API is fully consumed`, `docs: the addon architecture in CLAUDE.md,
README and the user docs`.

---

## Merge

1. On `refactor/core-addons` at P3.20's end, run: S-check (`make check`), S-manager, S-web,
   S-shots, S-node, S-e2e (Playwright, every spec, zero CSP violations), S-remote (t9 on `z6`
   when reachable), and the gated suites the owner can run (`CLAUDIO_E2E=1`, and
   `CLAUDIO_PROXY_TEST=1` with `../claude-proxy`).
   Also: paste the `rules:` totals line of `make check` at P2's end and at P3.20's end (totals
   must be lower) and run `grep -rn 'compat(v0.10)' src` (every D14 item listed). Ask the owner
   (or a worktree with the v0.8.0 binary) for one real mixed-version check in each direction: a
   v0.8.0 daemon under a P3 client (the session gets the proxy env and the "plugins after
   `claudio daemon restart`" notice), and a v0.8.0 client under a P3 daemon (its `Manifest` is
   refused as "daemon too old"; a spawn with `ANTHROPIC_AUTH_TOKEN` loses an inherited
   `ANTHROPIC_API_KEY`). The `old_peer` switch is a simulation (F10).
2. A reviewer audits the branch against this plan, the spec's P3 exit row and ACCEPTANCE.md
   (A3, A5, A6, A9, A10, B1-B3, C1, C2, E1-E5), citing rules. The reviewer also checks: no
   `web/vendor/**`, `web/fonts/**` or import-map change in `git diff --stat <P2 end>..HEAD`; no
   `--write-baseline` outside move/split commits; no leaked test daemons (`pgrep -af 'claudio
   --daemon'` shows only the owner's).
3. Before the merge, `git log --format=%b <P2 end>..HEAD | grep -i 'co-authored\|generated
   with'` is empty (ACCEPTANCE F2). Then `git checkout main && git merge --no-ff
   refactor/core-addons` into local `main`, only after that review.
4. **Owner OK required before any push, tag or release.** The D14 compatibility code ships in the
   first release containing P3. Its removal is planned for the release after it.

## Spec issues

1. **"The proxy and plugin tests pass unchanged" vs the generic renderers.** The TUI tests can
   pass unchanged because the Vocdoni addon reproduces today's texts (setup title and item lines,
   "No proxy configured", `proxy:env`). The Playwright `proxy.spec.mjs` cannot: its testids
   (`plugin-setup`, `plugin-skip`, `stats-window-24h`, `stats-chart`, `stats`) belong to the
   components the spec deletes. P3.15 renames them with the same flows and numbers. tests/web.rs
   changes only in P3.16, where protocol v2 removes the ops it calls.
2. **Review timing (G7).** The spec reviews at launch only, but today's TUI opens a plugin's setup
   at first sight (`test_plugin_manifest_setup_then_sessions_get_what_was_kept` waits for it after
   closing the wizard). D8 adds `AddonCtx::offer_checklist` so the TUI keeps that; the web ignores
   offers, as it does today.
3. **Fields the spec's data types lack.** `ChecklistItem.section` (the setup screen's headings),
   `FlagDecl.label` and `addons.list.launch_flags` (the web's first-party checkbox), and a
   documented convention for which status line a `Segment` goes on (D9). The web topbar's 5h/7d
   gauge bars become text segments ("5h 62%") with a tone. That is a small visual change. The
   proxy panel keeps real gauges.
4. **`on_event` and `host_has(addon)`.** The spec lists them without a trait or receiver. Decided
   in D1/D2.
5. **"Derived from launch when its addon is vocdoni"** would make the core branch on an addon's
   name (A3). Replaced by the `Launch::legacy`/`adopt_legacy` hooks (D4).
6. **`LaunchArg::Optional` needs a wire field.** The daemon's drop rule can only be generic if
   the spawn says which flags are optional. D6 adds `SpawnSpec.optional` (journaled for respawns).
   An old daemon ignores it but drops its own fixed three, which cover the Vocdoni flags, so B1
   holds.
7. **`DaemonCtx::cache_dir(addon)`** moves the manifest cache (`<cache>/plugins` →
   `<cache>/vocdoni`). The cost is one re-download and t9's seed path.
8. **Extension-row toggles write `[extensions]`.** That is a core table an addon may not write
   (A3). D10 has the core write it and addons read it through `AddonCtx::extension_on`, an added
   hook.
9. **Pre-existing routing hole.** `Daemon::retry_spawn_fresh` re-spawns a session whose
   `--resume` failed with `env: vec![]` (`src/daemon/mod.rs:714-760`). A proxied session thus
   restarts direct, against AC8's "never direct". P3.3 keeps the spawn's env in a daemon-held
   `session_env` map (memory only, not the journal) for that retry, since the actor drops the
   live `Handle` before the retry runs (F3). This is a behaviour fix the spec did not list.
10. **The web's review flow and the Session DTO's "unknown" launch** are not specified. Decided
    in D11/D12.
11. **The daemon's default latex lens for `extensions: None`** (`effective_extensions`,
    `src/daemon/mod.rs:930-948`) is a core-side lens default for old clients. It stays for the
    one-release window (D14), so "a core-only build asks claude for no tags" holds for new clients
    only until then.
12. **`claudio vocdoni login` without a URL** (F17) prefers the default profile's URL, so re-login
    of a self-hosted profile keeps working (today's `claudio proxy login`, G7); a fresh install
    gets `DEFAULT_URL`. The spec says it always signs in to `https://claude.vocdoni.net`.
13. **Deltas the spec's types lack** (F18): `LaunchRequest` carries this addon's `routing` and
    `deferred` ids (the spec's `choice` alone cannot tell an Optional addon from a Required one);
    `resolve` returns `Result` so an unknown option fails the launch (spec step 1); `select`
    parses the addon's own flags; `SessionSnap.focused`/`HostSnap.claude` feed credential polling
    and the installer hook; the registry's refusal rules are the plan's.
