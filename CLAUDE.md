# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`claudio` is a session manager TUI and drop-in wrapper for the `claude` CLI: a generic core
plus addons built into the same binary (the web UI, the API, the Vocdoni proxy client; see
"Workspace" and "Core and addons" below). It has four modes; `src/main.rs` only routes argv to
the core's words or an addon's (`Addons::relay`, the core words, `Addons::dispatch`, `--help`,
passthrough):

- **No arguments (bare `claudio`)**: it opens the **session manager** TUI — a terminal multiplexer for many Claude Code sessions, local and over SSH.
- **No `-p`/`--print`**: it `exec`s the real `claude` unchanged (passthrough).
- **`-p`** (the API addon): it emulates print mode by driving the *interactive* TUI under a PTY. It types the prompt, waits for the `Stop` hook, then reads the answer and real token usage from the session JSONL.
- **`--api`** (the API addon): it serves an OpenAI-compatible server (axum) on the same PTY backend, using a pool of persistent interactive sessions.

A build without the API addon refuses `-p`/`--print`/`--api` (before a bare `--`) with exit 2:
`claudio: print mode and the API server need the api addon; this build has none`.

**claudio never invokes `claude -p`.** Every path goes through the interactive TUI. Don't install the binary *as* `claude`, because it has to find the real one on `PATH` (or `$CLAUDIO_CLAUDE_PATH`).

## Acceptance rules

`ACCEPTANCE.md` is the bar every change meets before it merges (architecture: a generic core
plus addons, compatibility, security, quality, tests, docs). Check a diff against it and cite
the rule in reviews; `make check` runs the mechanical rules. The core/addons refactor that
introduces this shape is `docs/specs/2026-11-09-core-addons-design.md`.

## Commands

```bash
make                 # release build → target/release/claudio
make static          # static musl binary → dist/claudio (ARCH=aarch64 for cross)
make dev             # install this checkout to ~/.local/bin for testing, no release (make dev-undo reverts)
make check           # acceptance checks (ACCEPTANCE.md): fmt, clippy -D warnings, rules, core-only build, unit tests (workspace, then each crate alone)
make matrix          # the feature matrix: core only, each addon alone, all (make features FEATURES=web for one cell)
make test-unit       # unit tests only: cargo test --locked --workspace --lib --bins (no claude needed)
make test-manager    # manager integration tests (fake claude, no API key required)
make test            # unit + E2E (E2E needs an authenticated `claude` on PATH, ~5 min)
make e2e             # E2E only
make fmt             # cargo fmt
```

### Test suites and gates

**Manager integration tests** (fake `claude`, no real API):

```bash
make test-manager
# equivalent (crates/tui/tests/manager.rs):
cargo test --test manager -- --test-threads=1
# the same suite against the core alone (ACCEPTANCE A4):
cargo test --no-default-features --test manager -- --test-threads=1
# the TUI with the Vocdoni addon (addons/vocdoni/tests/manager.rs):
cargo test --test vocdoni_manager -- --test-threads=1
```

Must run single-threaded. Gated tests:

- `CLAUDIO_E2E=1` — enables `test_real_claude_session` (sends a prompt to the
  real `claude`, costs tokens, requires an authenticated `claude` on PATH).
- `CLAUDIO_PROXY_TEST=1` — enables the proxy-integration test (`vocdoni_manager`). It builds and
  runs claude-proxy from source (`$CLAUDIO_PROXY_SRC`, default `../claude-proxy`;
  needs `go`).
  The test for real-claude via proxy additionally needs `CLAUDIO_E2E=1` and
  `CLAUDIO_PROXY_URL`.

**Print-mode / API E2E tests** (addons/api/tests/integration.rs):

```bash
CLAUDIO_E2E=1 CLAUDIO_CADENCE=0 cargo test --test integration -- --test-threads=1 --nocapture
```

Without `CLAUDIO_E2E=1` the integration tests silently no-op. They call the
real Claude API and must run single-threaded.

**Remote / SSH tests** (tests/remote.rs):

```bash
# the suite needs the diag subcommands (required-features = ["diag", "vocdoni"]):
cargo test --features diag --test remote -- --test-threads=1 --nocapture
CLAUDIO_SSH_TEST_HOST=<host> cargo test --features diag --test remote -- --test-threads=1 --nocapture
```

Tests `t1`, `t7`, `t8` are unit-style and always run. Tests `t2`–`t6` require
`CLAUDIO_SSH_TEST_HOST` to be set to a reachable SSH host alias.

**Web UI** (`claudio web`; fake claude and fake proxy, no API key):

```bash
cargo test --test web --test web_manager -- --test-threads=1   # server and TUI-with-web (addons/web/tests/)
cargo test --test vocdoni_web -- --test-threads=1              # proxy and plugins in the web (addons/vocdoni/tests/web.rs)
node --test addons/web/web/tests/*.test.mjs   # browser logic, no browser (glob form: the directory form fails on Node 22)
make test-web                                 # node tests, then Playwright e2e against a debug build (addons/web/web/e2e)
```

The e2e starts a real `claudio web` per spec file in `~/.cache/claudio/web-e2e/<hex>/`
with an isolated env (`XDG_*`, `HOME`, `CLAUDE_CONFIG_DIR`, `CLAUDIO_CLAUDE_PATH` = a fake
claude), so it never touches your own daemon or `~/.claude`. The `--remote` tests (the Rust
suites and `remote.spec.mjs`) use the committed fixture password
(`addons/web/web/e2e/fixtures/web-password`, password `claudio-e2e-password`). Needs Chromium in
`~/.cache/ms-playwright` and `npm ci` in `addons/web/web/e2e`. Run one spec:
`cd addons/web/web/e2e && npx playwright test board`.

- Run one unit test: `cargo test --workspace --lib --bins <name_substring>`, or one crate's:
  `cargo test -p claudio-tui <name_substring>`.
- Run one E2E test: `CLAUDIO_E2E=1 CLAUDIO_CADENCE=0 cargo test --test integration <name> -- --test-threads=1 --nocapture`.
- CI runs the release build, the unit tests, `make check` and the feature matrix (`make features`,
  each cell), on Linux and macOS (claudio is Unix-only). Keep code compiling on both.

## Architecture

### Workspace

One binary, `claudio`, built from a Cargo workspace (`docs/addons.md`, "Crates"):

```
Cargo.toml       [workspace] + the `claudio` package (features api, web, vocdoni: default; diag)
src/main.rs      wiring only: the core's words, `addons()` (the cfg'd registry), help, passthrough
crates/core      claudio-core     addon/ app/ claude/ client config daemon/ freshness paths proto
                                  release remote/ sys term/ text upgrade version
crates/tui       claudio-tui      the console manager and `--plain`        tests/manager.rs, tests/common/
addons/api       claudio-api      print/, api/, cli.rs, msglog.rs          tests/integration.rs
addons/web       claudio-web      src/ (the server), web/ (the page), build.rs   tests/web.rs, tests/manager.rs
addons/vocdoni   claudio-vocdoni  the claude-proxy client, the team plugins     tests/manager.rs, tests/web.rs
tests/           remote.rs, screenshots.rs (the whole binary), fixtures/
```

- **Dependencies (ACCEPTANCE A2, checked by `scripts/check_rules.py`):** the binary depends on
  everything; claudio-tui and each addon on claudio-core only; claudio-core on none of ours.
  An addon's dev-dependencies may add the TUI or another addon; the core's and the TUI's may
  not. `cargo build --no-default-features` is the core alone (A4).
- **`test-support`:** claudio-core's fixtures for other crates' tests (`TestAddon`, the daemon
  harness, the fake host) are `#[cfg(any(test, feature = "test-support"))]`, enabled only by
  dev-dependencies, so a release never links them.
- **Integration suites are root targets.** They drive the `claudio` binary, and Cargo gives
  `CARGO_BIN_EXE_claudio` only to its own package, so each file lives next to the crate it tests
  but is a `[[test]]` of the root `Cargo.toml` with the `required-features` it needs (`autotests
  = false` everywhere). The PTY harness is one file, `crates/tui/tests/common/mod.rs`, included
  by the other suites with `#[path]`.
- **`src/main.rs`** names `claudio_core::…`, `claudio_tui::run`, `claudio_tui::plain::args::run`
  and the addon crates directly; `addons()` registers vocdoni, web, api (api last: it parses
  whatever argv is left). Without the api feature, `route` refuses `-p`/`--print`/`--api`.

### Core and addons

The core (claudio-core and claudio-tui) wraps claude: the manager, `--plain`, the
daemon, SSH hosts, the installer, the passthrough. Addons (`addons/*`) add the rest through one API.
`docs/addons.md` is that API; `docs/daemon-protocol.md` is the daemon protocol (addon ops,
`env_remove`, `optional`, `Welcome.features`); `docs/web-protocol.md` (v2) is the page's.

**`crates/core/src/addon/`** — the addon API: `mod.rs` (`Addon` and its capabilities, `Commands`,
`AddonCtx`, `AddonEvent`, `AddonError`, `name_ok`), `launch.rs` (`Launch`, `LaunchSelect`,
`LaunchChoice`, `LaunchPart` with a C1 `Debug`, `Routing`, `LegacyChoice`), `ui.rs`
(`Ui`, segments, panels, checklists, extension rows, notices), `daemon.rs` (`DaemonOps`,
`DaemonCtx`, `ClaudeFlags`), `clean.rs` (the C2 sanitising and caps every addon string
goes through before a front-end sees it, applied by the runtime), `registry.rs` (`Addons`:
the immutable registry, its refusals — reserved words, core sections, shared flags or
panel keys — and `relay`/`dispatch`/`help`), `test_support.rs` (`TestAddon`, `test-support`).

**`addons/`** — the addon crates (`src/main.rs`'s `addons()` registers them, in registry order
vocdoni, web, api; the API addon parses whatever argv is left, so it is last):
- `api/` (claudio-api, `addons/api/src/`) — `Commands` only: the print relays (`is_relay`),
  `-p`/`--print` and `--api` (decided by `cli::parse`), `--help`'s `HELP_APPENDIX`. `print/`, `api/`, `cli.rs`,
  `msglog.rs` as described under "Print-mode pipeline" and "API server" below.
- `web/` (claudio-web, `addons/web/src/`) — `claudio web` (below), plus `addon.rs`
  (`WebAddon`: `Commands` for `web …`; `Launch` gives every session the `latex` and
  `mermaid` lenses while `[extensions] latex`/`diagram` are on (`[claude] latex = false` is the alias the runtime maps); `Ui`
  lists those two rows; `DIAGRAM_CONTEXT`).
- `vocdoni/` (claudio-vocdoni, `addons/vocdoni/src/`) — the claude-proxy client and the team
  plugins: `lib.rs` (`Vocdoni`, `NAME`,
  `DEFAULT_URL`, `Commands` for `vocdoni …` and its alias `proxy …`), `cmd.rs`
  (`login|status|use|logout`), `proxy/` (`profile.rs` `[proxy]` profiles and
  `CLAUDIO_PROXY_URL`, `resolve.rs`, `env.rs` the session env and `env_diff`, `login.rs`
  the browser sign-in, `plugins.rs` `GET /v1/claudio/plugins`, `admit`, the `plugins.json`
  cache, `test_http_ok`, `api.rs` the HTTP client (`read_json` ≤ 256 KiB), `stats.rs` the
  stats `Window`), `plugins/` (`manifest.rs` the one manifest parser, `names.rs`,
  `choices.rs` `[plugins.<name>]`, `state.rs` the plugin state's types and `plugins_part`,
  a launch's plugin share), `state/` (the one plugin state machine and proxy cache, pure),
  `runner.rs` (its I/O on the runtime: lists every 15 min, manifests, `vocdoni/apply`
  re-asked on `HostUp`/`ClaudeInstalled`, credential polls), `launch.rs` (`impl Launch`:
  profiles as options, `--proxy`/`--no-proxy`/`--first-party`, a default profile is
  `Required`, `prepare` gives the profile env, the plugins' optional args and secrets,
  `env_remove`), `ui.rs` + `panel/` (segments, the "Proxy stats" panel on Alt+s with its
  switch action, one checklist per manifest), `rows.rs` (a row per plugin), `profiles.rs`,
  `daemon/` (`vocdoni/apply`, was the daemon's `Manifest` op: download, verify, install
  with `claude plugin … --json`, back-off, `Applied`; cache `<cache>/vocdoni/`), `diag.rs`
  (`__remote-manifest`, `--features diag`).

**`crates/core/src/app/`** (below) holds the addon runtime and the launch composer the
front-ends share.

**A3 across crates:** the core crates cannot import an addon (A2: no such dependency) and
never branch on an addon's name; an addon imports `claudio_core::addon`, the core's public
services and its own modules, and reads and writes only the `config.toml` tables it
declares. The services addons use today: `paths` (all); `term::{probe, strip_escapes}`
(api); `proto`, `text`, `client`, `remote::bootstrap`, `term::secret` and
`app::{addons, launch, browser}` (web and vocdoni); `claude::SESSION_MARKERS` and
`app::config_edit` (vocdoni); `config`, `freshness`, `claude::{hooks, update}`,
`remote::{hosts, validate_host}`, `sys::hostname` and `app::label` (web). The web addon (the former in-tree web UI, a second front-end) also uses
`app::{state, connections, extensions, snippets}`: the layer both front-ends share (A5).

### Session manager (bare `claudio`)

A bare `claudio` calls `claudio_tui::run()`. The manager's architecture spans claudio-core
(`crates/core/src/`: every path below not under `crates/tui/src/`) and claudio-tui:

**Small shared modules** (`crates/core/src/`, no dependencies of ours): `text.rs` (`sanitize`
and `is_unsafe`/`is_combining`, `parse_rfc3339`, `fmt_tokens`), `release.rs`
(`SemVer`, `http_client`, `RELEASES_REPO`, `release_os`/`release_arch`: used
by `upgrade`, `remote::bootstrap`, `freshness` and `claude::update`),
`version.rs` (`PROTO`, re-exported by `proto`, so `paths` does not import
`proto`) and `sys.rs` (`hostname()`).

**`crates/core/src/proto.rs`** — wire framing shared by daemon and client. Frames are
`[u32-BE length][u8 tag][payload]`. Tag `J` carries JSON control messages
(`Envelope { req, msg: Msg }`); unknown `op` fields are ignored. Tag `D`
carries terminal bytes: `[16-byte session UUID][raw bytes]`. The protocol
version (`PROTO = 1`) is embedded in the socket file name so old and new
daemons coexist.

**`crates/core/src/daemon/`** — the per-host session daemon (invoked as `claudio --daemon`):
- `mod.rs` — shared `Daemon` state: the registry of journaled and live
  sessions, spawn as reserve → spawn → commit (`in_flight`/`to_kill`), kill,
  `respawn` (Alt+e; a reserved transition, so a concurrent Kill wins), and
  `close_exited` (a clean claude exit, or any shell exit, removes the session
  unless something replaced it meanwhile). `Config::claude_bin()` resolves
  claude on every use: `$CLAUDIO_CLAUDE_PATH` if set, else `claude` in
  `Config::path`, then well-known install dirs (`~/.local/bin`,
  `~/.npm-packages/bin`, …).
- `login_path.rs` — at daemon start, runs the user's login shell (`$SHELL
  -l -i -c`, setsid, stdin `/dev/null`, 3 s deadline, group killed on
  timeout) and reads its `PATH` between markers. It is merged with the
  inherited `PATH` (login first) into `Config::path`. A daemon started over
  ssh only has the system `PATH`, which can find a stale `/usr/local/bin/claude`.
  `Config::path` is used to find claude, and is the `PATH` of every session,
  terminal, `claude --version/--help` probe and `claude update`. A spec's own
  `PATH` still wins. On failure the inherited `PATH` stays. Tests never read
  real rc files. Unit tests use fake shell scripts, daemon tests build
  `Config` directly, and the manager harness pins `SHELL=/bin/sh` with a temp
  `HOME`.
- `server.rs` — accepts Unix socket connections, uid-checks peers, runs the
  per-client loop. Slow requests (git, `UpdateClaude`, attach, `Answers`)
  reply from spawned tasks through the client's queue, never inline.
  `Answers` reads the session transcript's tail (`projects::read_answers`)
  in `spawn_blocking`; `proto::answers_reply` caps the reply's encoded size
  (512 KiB, oldest answers dropped) so it always fits a frame.
- `session.rs` — one actor task per live session: owns the PTY writer, VT
  engine (`alacritty_terminal`), and subscriber list. Fans PTY output to
  all attached clients. Hook events (`__hook <Event> <socket> <token>`) are
  authenticated by uid plus a per-spawn token, enforced with a read deadline
  and size cap. On `SessionStart` the claude session id is journaled
  immediately so recovery works even when no client is attached. A session
  is `SessionKind::Claude` or `Shell` (terminal tab: a login shell, no hooks).
  `command()` adds `--allow-dangerously-skip-permissions` when the config
  allows it and this host's `claude --help` lists it. `MetaRefresher`
  produces `SessionEvent::Meta` (git status, model, context, output tokens,
  turns) off the actor task: on hooks (tool-use bursts share one refresh)
  and every 10 s while a client is attached; one refresh runs at a time. The
  transcript is read incrementally (`claude::projects::TranscriptCursor`).
  The refresh also sends the conversation's `<latex>` formulas as
  `SessionEvent::Latex { seq, formulas }`: the whole list, only when it
  changed (`seq` + 1, for the session's whole life; a new conversation
  sends an empty list under the next `seq`). The last list is replayed to a
  client right after its `Attached` + snapshot. The same refresh sends the
  tag lenses' items as `SessionEvent::Lens` (own `seq`, replayed right
  after the formulas, ≤ 256 KiB). `spawn` writes the session's extension
  instructions to `<runtime dir>/context/<id>.txt` and `command()` adds the
  `__context file <path>` SessionStart hook when there are any.
- `git_status.rs` — `git status --porcelain=v2 --branch` for the status
  bar's `⎇` segment: 1 s deadline, capped output, same scrubbed env as
  `git.rs`.
- `journal.rs` — durable session list (`daemon-sessions-v1.json`): writes via
  temp + fsync + rename (`paths::write_atomic`). Never stores secrets or the
  daemon's `--settings` injection; ephemeral (`--plain`) sessions are not
  written. On fresh daemon start, non-killed entries are re-spawned with
  `claude --resume <claude_session_id>`.
- `git.rs` — read-only git queries for the git viewer (log, commit, diff):
  bounded output, sanitized text, no textconv/ext-diff, repo env scrubbed.
- `update.rs` — `claude update` (or the official installer) on request, in its
  own process group, killed as a group on timeout. The installer needs
  `curl` and `bash` on `Config::path` and is refused when
  `CLAUDIO_CLAUDE_PATH` names a path (the reply's tail says why, nothing
  runs); a successful install calls every daemon-side addon's
  `on_claude_installed`.
- `addon_ops.rs` — `Msg::AddonOp` for the registered addons' `DaemonOps`
  (`DaemonCtx`: claude binary, PATH, `ClaudeFlags`, `<cache>/<addon>`): run off
  the client loop through `reply_later`, ≤ 8 in flight per addon, 150 s, bodies
  ≤ 512 KiB both ways and never logged; `on_claude_installed` after an
  install. `Welcome.features` lists `addon:<name>` for each.
- `admit.rs` — what a spawn or respawn is admitted as: the caps, the effective
  extensions (the request's, the journal's, else (compat(v0.10)) the latex
  lens), and `session_env`, the spawn env kept in memory only for a retry.
- `command.rs` — the session's command line and env: claude with the hook
  settings, `--allow-dangerously-skip-permissions` when allowed and known,
  the spec's `optional` flags (and, compat(v0.10), `--plugin-dir`, `--agents`,
  `--mcp-config`) dropped when this claude's `--help` lacks them; the env
  scrub: `env_remove: Some(list)` removes `SESSION_MARKERS` plus the list,
  `None` (an old client) the previous release's rule (compat(v0.10)); a shell
  gets the markers removed only.
- `lifecycle.rs` — `respawn`, `retry_spawn_fresh` (with the kept env, never
  direct) and `close_exited`.
- `host.rs`, `stats.rs` — host facts for `Welcome`, directory listing, CPU and
  memory samples. `ctl.rs` — `claudio daemon status|stop|restart` (status
  shows the claude path and version the daemon uses).

**`crates/core/src/client.rs`** — generic client connection and local daemon autostart.
Connects to the socket; if unavailable, starts the daemon (double-fork +
`setsid`, or `systemd-run --user` on systemd hosts) and retries.

**`crates/core/src/freshness.rs`** — keeps each host's daemon at least as new as the claudio
binary on it. `Welcome.build`/`build_time` identify the daemon's binary;
`ensure_current` (given the connected `Client`; the caller connects) runs
before the local TUI connects and in the remote bridge before it relays. An older daemon is shut down (SIGTERM by the lock's PID if
it predates `Shutdown`) and restarted from the current binary, unless
`busy` (a claude turn or permission prompt, or a `--plain` session): then it
is left running, the client is flagged `daemon_outdated` (over SSH the
bridge prints `DEFERRED` on stderr), and the TUI reconnects that host once it
is idle (checked every 2 min).

**`crates/core/src/app/`** — the layer both front-ends share (the TUI and the web addon), with
no ratatui, crossterm or axum. The web imports nothing from claudio-tui:
- `addons/` — `AddonRuntime`, the one `AddonCtx` implementation (one per addon, D2):
  events (`Started`, `Tick` 1 s, hosts, sessions, `ClaudeInstalled`, `ConfigChanged`),
  `ctx.spawn` (≤ 64 tasks per addon, 300 s), `ctx.request` (`AddonOp`, 170 s), config
  sections read and edited (`reload.rs`, single-flight re-reads), the extension rows
  (`rows.rs`: `toggle_row` writes `[extensions]` then tells the addon), `extension_on`
  (with the `[claude] latex` alias), and what it tells its front-end (`RuntimeOut`:
  notices, coalesced `Changed`, offers; a bounded queue).
- `launch/` — the launch composer, the one path of a claude session's `Spawn`/`Respawn`
  from the TUI, `--plain` and the web: resolve (every `Launch` addon's `Routing`; a
  `Required` addon that fails fails the launch, never direct) → prepare (25 s budget) →
  review (`NeedsReview` parks the launch, the clock stopped) → `compose.rs` (the one
  `compose`: env layered, `env_remove` unioned, optional args, lenses merged, the
  instructions cut at 8158 bytes, the frame cap). `adopt_legacy`/`legacy` convert the
  pre-addon `proxy`/`first_party`.
- `config_edit.rs` — the minimal, atomic `config.toml` section edit under one
  process-wide lock (every writer goes through it).
- `state.rs` — `ClientState` (`state.json`): session order, names, recent dirs, and each
  session's `launch` (`SavedRoute`: `launch` wins, else the legacy `proxy`/`first_party`
  as an addon adopts them, which are still written (compat(v0.10)); a route this build
  cannot make is kept and fails its launches, never direct; `route_unknown` for one
  started elsewhere, R19).
- `connections.rs` — local + SSH connections, reconnect backoff.
- `snippets.rs` — the saved prompts of `~/.config/claudio/snippets/`; the TUI
  (`crates/tui/src/lib.rs`) and the web (`ext.rs`) both call its async functions
  (`load_all`, `save_text`, `save_text_named`, `delete_named`).
- `extensions/` — the extension registry (`mod.rs`: built-ins search and snippets;
  `Registry`, `ActionResult`, `core_choices`; the addons' rows as `Source::Addon`,
  `Registry::set_addon_rows`), external
  manifests (`manifest.rs`: `~/.config/claudio/extensions/<name>/`, the
  HTML-tag rule, admission, at most 16), protocol v1 (`protocol.rs`: request
  lines, result caps and sanitizing; `render` serves only the web UI's
  `extensions.render`) and the extension host (`host.rs`: one
  program per extension, lazy start, scrubbed env, own process group,
  ≤ 16 in flight, render 1 s / action 15 s, 3 failures in 5 min disable it,
  killed on exit; it knows every loaded external extension, and one turned
  off is refused unstarted and its program stopped; driven by the TUI's
  `lib.rs`, never by `App`).
- `label.rs` — `sanitize_label`, for text that leaves the process as a label.
- `browser.rs` — `browser_wanted` and `open_browser`, the sign-in page opener
  (`claudio vocdoni login`, `claudio web`).
- `test_support.rs` — session fixtures for unit tests.

**`crates/tui/src/`** (claudio-tui) — the manager UI, driven by tokio + ratatui:
- `lib.rs` — I/O edge: owns the terminal, runs the event loop, gathers the
  `AppConfig`, drives daemon connections and every `Effect`. Emits OSC 9
  notifications (`ESC ] 9 ; … BEL`) for background sessions that need
  attention. Parses the reader's answers off the UI thread
  (`Effect::RenderAnswers` → `spawn_blocking` → `HostEvent::AnswersRendered`):
  markdown into a `reader::doc::Doc`.
- `app.rs` — pure coordinator (`App::new(AppConfig)`): processes events into
  state transitions and returns `Effect` values. No I/O here; all I/O is in
  `lib.rs`. `Mode::Manager` or `Mode::Plain`.
- `sessions.rs` — `SessionView` (labels go through `app::label::sanitize_label`)
  and spawning (`send_spawn`, the one path that builds spawns and
  respawns: a claude session is an `Effect::Launch` with a `LaunchSelect`,
  composed by `app::launch` in `addon_runner.rs`; `SessionView.launch` is
  its `LaunchChoice`, state.json's legacy `proxy`/`first_party` adopted
  through the addons, an unadopted one failing its launches), recovery,
  the wizard's opening (its route row: `wizard_route.rs`, from
  `App.launch_options`).
- `interaction.rs` — keys, mouse, modals (`Modal`), daemon events and replies.
- `wizard.rs` — the new-session wizard (start screen → directory browser →
  resume?), a pure state machine.
- `git_view.rs` + `git_app.rs` — the git viewer (Alt+l): pure view state
  machine, and its `App` glue.
- `addon_runner.rs` — the TUI's edge of the addon runtime (built in `run`, fed
  from the event loop: hosts, sessions, focus, installs) and of the launch
  composer (`Effect::Launch` → `app::launch`, parked launches for a review kept
  here, never in `App`); runs `App`'s `AddonCall`s (`addon_calls.rs`: panel
  10 s, action 15 s, row toggles, checklist saves), cleaning every addon string.
- `panel_view.rs` + `ui/panel.rs` — the generic addon panel (pages, blocks,
  windows, actions with a confirm; Alt+s is the Vocdoni addon's `PanelDecl.key`).
- `checklist.rs` + `ui/checklist.rs` — the generic addon checklist (a plugin's
  setup): offered at first sight (`AddonCtx::offer_checklist`) or a launch's
  review; Space, `a`, `n`, Enter saves, Skip/Esc never reach the addon.
- `reader/` — the reader view (Alt+u): `doc.rs` (an answer as markdown
  blocks; a `<latex>` tag is kept as written, as inline code, through
  placeholders; parsed off the UI thread), `layout.rs` (a `Doc` wrapped to
  a width) and `mod.rs` (`ReaderView`: answer, scroll, layout cache).
  Drawn by `ui/reader.rs`. The console draws claude's text as claude wrote
  it: no formulas, diagrams or lens renderings (spec G5; those are the web
  UI's).
- `extensions_view.rs` — the extensions screen (Alt+i, or Alt+z e): rows,
  details, toggles. A toggle flips the registry at once and emits
  `Effect::SetExtension` (host; externals) and `Effect::SaveExtension`
  (`lib.rs` writes `[extensions] name = on` with `config::save_extension`,
  a minimal line edit of config.toml, never a whole-file rewrite); an
  addon's row (`Source::Addon`, set by `Registry::set_addon_rows`) is
  toggled through `AddonCall::ToggleRow` (the runtime writes
  `[extensions]`, re-reads the file, then tells the addon), Enter on a row
  with a setup opens that checklist, and `r` sends every addon with rows
  `RowAction::Refresh`. Drawn by `ui/extensions.rs`.
  The leader keys `u` and `e` are reserved.
- `leader.rs` — Alt+z: the leader popup, action input, list picker,
  documents (a reader view) and paste (attached claude session with
  bracketed paste on only, never Enter). `builtins.rs` — the built-in
  actions: search (`Msg::Search`, resume or switch) and snippets
  (`Effect::LoadSnippets`/`SaveSnippet`, `Msg::LastPrompt`).
- `confirm.rs` — the generic confirm prompt (kill, reset, claude update).
- `claude_update.rs` — local/remote claude version checks and updates, and
  `[claude] install` (`auto` by default, `ask`, `off`): a host whose
  `Welcome` has no claude, `local` included (`check_local_install`, also in
  `--plain`, whose session waits for it), gets the installer once per run
  (`claude_checked`). A successful install goes to the addon runtime
  (`ClaudeInstalled`).
- `plain.rs` — `claudio --plain`: `Mode::Plain` is one local, ephemeral
  session full screen (no tab/status bar, wizard or state.json).
  `Keymap::build_plain` keeps only Alt+h/s/l/e/u/z/i; the session is killed when
  the UI exits, and the UI exits with claude. `plain/args.rs` — its argv (`run`, called
  by `src/main.rs`).
- `keys.rs` — key → byte encoding (crossterm events) for input forwarding to the PTY.
- `pane.rs` — draws a core `term::screen::Screen` into a ratatui buffer.
- `ui/` — ratatui rendering: `mod.rs` (layout, tab bar, pane, popups),
  `status.rs` (two-line bottom bar; line 2 is prioritised segments — cpu,
  ctx gauge, git, mem, conversation and the addons' segments by priority
  (0-99 line 1, 100-255 line 2) — dropped whole when narrow),
  `git.rs`, `confirm.rs`, `reader.rs`, `leader.rs`, `extensions.rs`,
  `panel.rs`, `checklist.rs`.
- `fmt.rs` — shared text and number formatting.
- `keymap.rs` — `DEFAULT_BINDINGS` table (the single source of truth); `Keymap`
  (defaults + config overrides); `parse_key_spec`; collision detection.
  `Alt+Shift+<digit>` (go to tab N) is fixed, outside the table.
- `notifications.rs` — debounce logic for attention notifications.
- `selection.rs` — copy-on-select (`[ui] copy_on_select`, default on): the
  pure `Selection` (anchor/head spans, cell/word/line unit, `contains`),
  multi-click counting and the OSC 52 encoder. A left drag in the pane
  selects only while the session's mouse reporting is off (otherwise the
  mouse is forwarded as before); the release emits `Effect::CopyToClipboard`
  with `Screen::text` and a "copied N chars" notice. The selection is
  cleared by a click, key, paste, tab switch, resize or re-attach. Glue:
  `App::select_mouse` in `interaction.rs`; highlight in `ui/mod.rs`.
- `test_support.rs` — shared fixtures for the TUI's unit tests (the session
  fixtures are `app::test_support`).

**`crates/core/src/remote/`** — SSH bootstrap and bridge:
- `bootstrap.rs` — `ensure_remote(host)`: runs `__probe` + `uname -sm` in one
  ssh round trip; uploads the binary if missing or stale (SHA-256 check).
  Same-platform: uploads self. Cross-platform: downloads release asset to
  `~/.cache/claudio/<version>/` and uploads that.
- `bridge.rs` — `--slave` mode: the stdio↔socket bridge the local client runs
  on the remote as `ssh HOST ~/.local/bin/claudio --slave`. Starts the remote
  daemon if not running (`systemd-run --user` preferred, `setsid` fallback),
  and replaces an outdated one (`freshness.rs`).
- `hosts.rs` — SSH host candidates: MRU list (`hosts.json`) + `~/.ssh/config`
  aliases.
- `probe.rs` — `__probe` subcommand: prints `{version, proto, os, arch,
  build, mtime}` as JSON (`Probe::own()`: this binary, hashed once).
- `diag.rs` — the SSH test harness's `__…` words (`--features diag`, never in a release).

**`crates/core/src/term/`** — terminal primitives (no UI toolkit; drawing and key encoding
are the TUI's `pane.rs` and `keys.rs`):
- `screen.rs` — `alacritty_terminal::Term`-based screen + snapshot logic.
- `probe.rs` — answers the terminal probes Ink sends at startup (print mode and the daemon).
- `secret.rs` — `read_line_no_echo`: one line from a terminal with echo off, restored on every
  exit path (SIGINT included); the web's `--set-password` and the Vocdoni addon's token prompt.
- `mod.rs` — `strip_escapes`.

**`crates/core/src/claude/`** — claude knowledge:
- `mod.rs` — `SESSION_MARKERS` (env a parent claude sets for its children),
  `binary()`, `exec()`.
- `projects.rs` — scan `~/.claude/projects/` for existing sessions (feeds the
  wizard's resume list), and `recent_answers` (the reader's last 10 answers
  of a transcript tail: real prompts and the assistant text after them).
- `hooks.rs` — `__hook <Event> <socket> <token>` relay: sends the hook payload
  to the daemon socket. Also `__context file <path>`, the SessionStart hook
  that prints the session's context file (the extensions' instructions: no
  symlinks, a regular file of the uid, 16 KiB, sanitized) as one JSON line;
  any other kind (the pre-0.6 `latex`) prints nothing. It never touches the
  socket.
- `latex.rs` — `lens_tags` (the items of any tag lenses in an assistant
  text, code excluded, with their byte ranges), `extract_latex` and
  `latex_tags` (its `<latex>` wrappers). The character filter every formula
  and answer goes through is `text::sanitize`.
- `state.rs` — session state tracker derived from hook events.
- `update.rs` — version parsing and the daily release-channel check.

### Web UI (`claudio web`, `addons/web/src/`, `addons/web/web/`)

`claudio web` (the web addon's `Commands`; flags in `docs/web.md`) serves the manager to a
browser: one more client of the daemons, like the TUI, over the same socket and ssh bridge.
`docs/web-protocol.md` is the contract between server and page (version 2: `addons.*` ops,
the Session's `launch` and `segments`); change it first, in its own commit. User docs, the security model and the TUI parity table: `docs/web.md`.

**`addons/web/src/`** (claudio-web; axum, loopback only unless `--remote`):
- `lib.rs` — args (`--listen`, `--remote`, `--rotate-cert`, `--allow-host`, `--no-open`,
  `--print-url`, `--rotate-token`, `--set-password`, `--print-cert`), routes, startup,
  graceful shutdown (tabs closed 1001, `web-state.json` written).
- `auth.rs` — token file (`<config>/web-token`, 0600, constant-time compare), cookie sessions
  (24 h idle, 64), the `Host`/`Origin` sets (loopback, or `--remote`'s with IP literals and
  https only), the local rule (`Auth::caller`), `guard` (Host 421, Origin 403, cookie 401,
  `/api/me`'s login methods), the CSP (`csp`, with the import-map hash) and security headers
  on every response.
- `login.rs` — `POST /api/login`: token (local requests only with `--remote`) or password
  (argon2 in `spawn_blocking`, 4 at once).
- `limit.rs` — login reservations: 5 per peer, 30 global per minute, taken before any check.
- `peer.rs` — `Peer` (`ConnectInfo`), `PeerKey`.
- `password.rs` — `web-password` (argon2id, fixed parameters), `--set-password`.
- `remote.rs` — `--remote`'s address grammar, certificate names, start-up banner, `prepare`.
- `tls/` — `cert.rs` (self-signed certificate kept in `web-tls/`, fingerprint, `--print-cert`),
  `listener.rs` (the `axum::serve::Listener` over TLS: handshake tasks, connection caps,
  a header deadline for a connection's first request).
- `assets.rs` — the embedded app, gzipped by `build.rs`; `CLAUDIO_WEB_DIR=<dir>` serves from
  disk in a debug build.
- `ws.rs` — the WebSocket: `hello` + `snapshot`, JSON requests (§5 ops), binary PTY frames,
  size limits and close codes, per-tab queue (8 MiB, close 1013).
- `hub.rs` — one actor owning hosts, daemon connections, sessions, tabs and `web-state.json`;
  attach refcount per session and fan-out; the approve guard runs here.
- `dto.rs` — Session and Host DTOs, built field by field (never a serialized `Profile`,
  `SpawnSpec`, `Applied`).
- `input.rs` — `approve_key` (state + attention kind + seq) and `paste_bytes` (bracketed,
  controls stripped, 64 KiB).
- `state.rs` — `web-state.json` (layout, order, each session's `launch` and its legacy
  `proxy`/`first_party`, remote hosts); reads the TUI's `state.json`, never writes it.
- `spawn.rs`, `addons.rs`, `addon_feed.rs`, `ext.rs` — every claude spawn and respawn through
  the launch composer (`app::launch`, as the TUI's: `launch`/`defer` args, a `review` reply),
  the `addons.*` ops and the Session's `launch`/`segments` (`addons.rs`), the runtime's
  notices, changes and host feed (`addon_feed.rs`; no plugin state or proxy cache of its own),
  and the extension registry, external host, snippets and config.toml toggles.
- `addons/web/build.rs` — gzips `addons/web/web/` into `$OUT_DIR`, writes the import map's
  SHA-256 for the CSP.

**`addons/web/web/`** (the page; no build step; ES modules through the import map):
- `index.html` — the import map (the only inline script) and the stylesheet links.
- `app/` — `boot.js` (login, CSP violation log `window.__claudioCsp`), `store.js` (model +
  WebSocket), `proto.js`, `term.js` (xterm.js), pure logic with node tests (`queue.js`,
  `keymap.js`, `layout.js`, `panes.js`, `wizard.js`, `model.js`, `notify.js`, ...);
  `app/ui/` Preact components (`htm`, signals), the addon ones generic (`panel.js`,
  `checklist.js`, `segments.js`; pure parts in `launch.js`, `addon-view.js`); `app/render/`
  markdown, KaTeX, mermaid, highlight, all behind DOMPurify.
- `styles/`, `fonts/` — tokens and themes (arcade and plain, dark and light); vendored fonts.
- `vendor/` — pinned upstream files, byte-identical, listed in `MANIFEST.txt` with version,
  URL and SHA-256; licenses in `LICENSES.md`.
- `tests/` — `node --test` suites for the pure modules. `e2e/` — Playwright specs and
  fixtures (`server.mjs` starts a real `claudio web`, `fake-claude.sh`, `fake-proxy.mjs`,
  `transcript.mjs`). `dev/mock-server.mjs` — the page against a mock (`MOCK_MANY=1`,
  `MOCK_FOREIGN=1`).

### Non-obvious constraints (don't "fix" these — manager)

- **Keys must never collide with claude/terminal bindings.** All manager keys
  use the `Alt` modifier. `keymap.rs` enforces no-collision among manager
  bindings; the `config.toml` parser rejects collisions with a notice.
- **`Attached` precedes the snapshot `D` frames.** The daemon sends the
  `Attached` control message, then immediately the snapshot as a `D` frame.
  The client must not render anything until it has received `Attached`.
- **Protocol additions must be backward compatible.** New `op` values in `Msg`
  must be accepted (ignored) by older daemons. New `SessionEvent` variants
  deserialize to `SessionEvent::Unknown` so an old client never panics.
- **The PTY test harness asserts on the rendered screen model, never on raw
  bytes.** All `wait_for` calls in the manager suites (`crates/tui/tests/common/mod.rs`) operate on the
  `alacritty_terminal::Term` VT model. Asserting on raw bytes is fragile and
  forbidden.
- **Don't send `ESC` immediately before `Alt+key` in tests.** `ESC ESC q` is
  ambiguous — the VT parser may interpret it as two separate `ESC` sequences.
  Use the crossterm key-event encoding instead.
- **Secrets never go in argv or state files.** Proxy tokens and plugin env
  are passed only through `Spawn.env` over the Unix socket (from an addon's
  `LaunchPart.env`, whose `Debug` shows keys only); an `AddonOp` body
  (`vocdoni/apply`) carries secret names, never values, and is never logged. The journal and `state.json`
  never hold secrets (profile names only); `config.toml` and
  `plugins.json` do, both mode 0600. Logs and notices name keys, never
  values. The `--mcp-config` JSON on argv holds `${NAME}` placeholders
  that claude expands from the env; `--agents`/`--mcp-config` are added by
  claudio's spawn path only, never parsed from the user's args.
- **Plugins are installed by the CLI, never with `-y`.** The daemon runs
  `claude plugin … --json` and never writes `~/.claude` itself; a
  marketplace-declared command (`shownCommand`) is left for a person.
- **New request ops must fail safe on old daemons.** An old daemon answers an
  unknown op with `Error "unsupported op…"`; `App::older_daemon_notice` turns
  that into "run `claudio daemon restart`". Never add a field to an existing
  op that an old daemon would silently ignore into wrong behaviour (that is
  why terminals are `SpawnShell`, not `Spawn{kind}`, and plugin manifests
  are an `AddonOp` (`vocdoni/apply`, behind `addon:vocdoni` in
  `Welcome.features`), not a `SpawnSpec` field: an old daemon's error
  becomes `daemon too old` and the session starts without the plugin;
  `SessionExtensions.instructions` is `#[serde(default)]`, which an old
  daemon ignores harmlessly).
- **A daemon is replaced only by a newer build.** `freshness::compare` orders
  builds by version, then binary mtime; "different" is not enough, or two
  binaries (a release and a dev build) would replace each other's daemon
  forever. A daemon keeps its startup lock until it has fully stopped, so
  its successor never runs beside it.
- **Test fakes and the test config.** The daemon runs `claude --help` once to
  probe for flags, so fake claudes must ignore `--help` (and `--version`)
  without side effects. Every test claudio starts with `[claude]
  update_check/remote_check/install = "off"`: the update prompts are modal
  and would swallow the keys a test types, and an install would run the
  installer (`test_missing_claude_is_installed_at_start` turns it on with a
  fake `curl` and a `PATH` that reaches no real one).
- **`app.rs` is pure; I/O is only in `crates/tui/src/lib.rs`.** `App::update` takes
  events and returns `Effect` values. `lib.rs` drives the effects. Don't add
  I/O to `app.rs`.
- **The console TUI stays plain (A7, spec G5).** The pane shows claude's
  screen as claude drew it, and the reader shows formulas and diagrams as
  their source. Formulas, diagrams and lenses render in the web UI only;
  the TUI still sends the registry's lenses with every spawn, because the
  web UI renders them from the daemon's `Lens`/`Latex` events. Don't add an
  overlay or renderer to the TUI.
- **The reader's markdown is parsed off the UI thread** (`spawn_blocking`):
  the transcript is attacker-influenced. `App` and the drawing only lay out
  the parsed `Doc`.

### Non-obvious constraints (don't "fix" these — web)

- **`DESIGN.md` is the web UI's style reference; every UI change follows it.** Dark only;
  surfaces Void `#0b0e12` → Carbon `#181a1d` → Graphite `#1f2124`, 1px Slate `#303235`
  hairlines, radius 1px, no shadows, weight 400 only, mono (JetBrains Mono) for every
  technical label/tab/badge/button with wide tracking, sans (Inter) for prose; Phosphor Green
  `#00d892` only for active/status, Deep Teal `#002923` the only filled button. Two owner
  decisions on top of it: text sizes stay at the owner's larger scale (not DESIGN.md's
  marketing-site 11–14px), and session states map to working = green, waiting = Syntax
  Violet `#c58aff`, stopped = Steel `#818284`, error = Syntax Coral `#ff6285`. The tokens live
  in `addons/web/web/styles/tokens.css`; never hard-code a colour, radius or font outside it.

- **The import map is the only inline script, and its hash is in the CSP.** `build.rs`
  hashes the map's exact text; editing it changes the hash (a rebuild picks it up). No other
  inline script, inline handler, `eval` or CDN, ever. The e2e asserts zero
  `securitypolicyviolation` events.
- **Vendored files under `addons/web/web/vendor/` are byte-identical to upstream.** Delete whole files
  (mermaid reduction rule) but never edit one; every file is in `MANIFEST.txt`.
- **The web never writes `state.json`.** It reads it; its own state is `web-state.json` and
  `web-token`, both written with `paths::write_atomic` (0600).
- **`--remote` is the only non-loopback listener, and it is TLS.** `listen_addr` still refuses
  non-loopback `--listen`; with `--remote` the token works for local requests only (loopback
  peer **and** loopback `Host`), so a reverse proxy on the same host never turns internet users
  into token holders. No HSTS: it would stop browsers from accepting the self-signed
  certificate later.
- **The suites' remote peer is a debug-only hook** (`CLAUDIO_TEST_REMOTE_PEER`); it only ever
  takes the token away.
- **DTO fields are fixed by `docs/web-protocol.md`.** A new field or message changes the doc
  first. Session ids are `sid` in every request and event (an `id` argument would collide with
  the request id); only the Session DTO keeps `id`.
- **`SessionEvent::Attention` is nested** (`SessionEvent::Attention { attention }`) because the
  payload's own `kind` would collide with the enum's `kind` tag.
- **Secrets stay server-side.** Proxy tokens and plugin env go only in `Msg::Spawn`/`Respawn`
  over the daemon socket; nothing env-bearing is serialized into a DTO, log or notice.
- **Approve is guarded in the hub actor** (state + attention kind + seq, no await between
  check and send); a stale click answers `"stale"`, it never types into the wrong prompt.
- **Pure browser modules import nothing through bare specifiers**, so `node --test` runs
  them without a browser; keep UI-only imports in `app/ui/`.
- **Stable `data-testid`s are the e2e's interface** (listed in the plan's Global Constraints); rename one and
  the Playwright specs break.

### Print-mode pipeline (`-p`, the API addon: `addons/api/src/`)

1. `cli.rs` handles arguments *dynamically*. It owns only a few flags (`-p`, `--api`, `--fast`, `--log-messages[-file]`, `--output-format`, `--input-format`, `--settings`, `--session-id`), drops `--bare` and `--no-session-persistence` with a warning (they would disable hooks and the JSONL), and forwards everything else verbatim. Never add a mirror of claude's flag grammar. Wrapper-only knobs belong in `CLAUDIO_*` env vars (`WrapperEnv`) so the CLI surface stays exactly claude's. `cli::HELP_APPENDIX` documents these flags and env vars and is appended to `claudio --help`; update it when you add one.
2. `print/driver.rs` spawns claude under a PTY (`portable-pty`). `PtySession` is the reusable unit: spawn, then `turn(prompt)` repeatedly. `driver::run` is the single-shot `-p` path built on it.
3. `claudio_core::term::probe` answers the terminal probes that Ink sends at startup (DA1/DA2, DSR, XTVERSION, OSC colors…), byte-for-byte as xterm-380 would.
4. `print/hooks.rs` signals completion. `SessionStart`/`Stop` hooks are injected via a merged `--settings` and report back over loopback TCP (default) or a watched directory (`CLAUDIO_HOOK_TRANSPORT=file`). The hook command is a copy of this binary under a random hex name in `/tmp`, so relay invocations are recognized first (`ApiAddon::is_relay`, run by `Addons::relay` before any core word): argv `<EventName> <port>` (concealed relay) or `__hook <Event>` (legacy; the five-word `__hook` is the core's manager relay).
5. `print/session.rs` reads the session JSONL, which is the source of truth for the final assistant text and usage. The terminal render is never parsed for answers.
6. `print/emit.rs` formats output as `text`, `json`, or `stream-json`, shaped like `claude -p`.

### API server (`--api`, `addons/api/src/api/`)

- `routes/chat.rs` takes a request down one of two paths:
  - **plain**: `prompt.rs` flattens `messages[]`.
  - **agentic** (request has `tools` and `CLAUDIO_API_AGENTIC` is on): `agentic.rs` builds a "tool-execution gateway" prompt and parses a fenced ```` ```tool_calls ```` block back into OpenAI `tool_calls`.

  Either way, the turn is resolved whole, so streaming is "resolve then chunk" into a fixed SSE set.
- `backend.rs` dispatches to `pool.rs` (`SessionPool`). OpenAI clients resend the full history, so the pool matches a request to a live conversation with a cumulative **prefix-hash chain** and types only the delta.
  - Past `CLAUDIO_API_MAX_LIVE`, LRU sessions are demoted: the process is killed and the mapping kept. They are revived later with `claude --resume`.
  - The system/agentic prompt is re-injected every `CLAUDIO_API_REINJECT_TURNS` turns.
- `config.rs` reads `CLAUDIO_API_*` env vars, falling back to legacy `OPENAI_PROXY_*` names. `--api` implies `CLAUDIO_FAST=1`.

### Non-obvious constraints (don't "fix" these)

- **The system prompt goes in the typed user message, never `--system-prompt`.** Anthropic classifies the system prompt of interactive requests. A third-party-looking one gets the request billed as a third-party app (`400 … extra usage`). The user message isn't scored.
- **The client's system prompt is never relayed verbatim in agentic mode.** A prompt describing another harness ("you are operating inside pi…") makes Opus/Sonnet treat it as prompt injection. `agentic.rs` instead reads it for facts: `ClientEnv::detect` fingerprints pi, opencode, hermes, or generic, and extracts the working directory and date. It then presents its own neutral gateway framing.
- **Tool argument help is rendered from each client's JSON Schema.** Don't add hardcoded per-name argument tables, because clients name the same concept differently (pi `path` vs opencode `filePath`).
- **API sessions run with `--tools ""`, `--strict-mcp-config`, `--disable-slash-commands`, and `--setting-sources project`, in a clean cwd** (`CLAUDIO_API_CWD`, default `/tmp`). The server never executes tools; the client does.
- **Prompts are typed with bracketed paste** (auto-detected from `?2004h`) so multi-line prompts submit atomically. Typing cadence and delays are cosmetic (`CLAUDIO_CADENCE`, `CLAUDIO_FAST`). The Ink-quiescence wait is a correctness wait.

### Debugging

- `CLAUDIO_DEBUG=1` prints a trace timeline.
- `CLAUDIO_RAW_LOG=<path>` dumps the raw PTY bytes.
- `--log-messages` (`msglog.rs`) logs all four hops (CLI→claudio→claude→claudio→CLI), correlated by id. `--log-messages-file <path>` writes them untruncated as JSONL.
- The API server logs pool decisions at INFO (`MATCH`, `RESUME`, `demoting LRU`).

`docs/print-and-api.md` has the full env-var tables and client configs for pi, opencode, and hermes. Docker (`docker-compose.yml`) mounts `./workspace` as `/work`.
