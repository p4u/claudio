# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`claudio` is a session manager TUI and drop-in wrapper for the `claude` CLI. It has four modes, chosen in `src/main.rs`:

- **No arguments (bare `claudio`)**: it opens the **session manager** TUI — a terminal multiplexer for many Claude Code sessions, local and over SSH.
- **No `-p`/`--print`**: it `exec`s the real `claude` unchanged (passthrough).
- **`-p`**: it emulates print mode by driving the *interactive* TUI under a PTY. It types the prompt, waits for the `Stop` hook, then reads the answer and real token usage from the session JSONL.
- **`--api`**: it serves an OpenAI-compatible server (axum) on the same PTY backend, using a pool of persistent interactive sessions.

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
make check           # acceptance checks (ACCEPTANCE.md): fmt, clippy -D warnings, rules, core-only build, unit tests
make test-unit       # unit tests only: cargo test --locked --bin claudio (no claude needed)
make test-manager    # manager integration tests (fake claude, no API key required)
make test            # unit + E2E (E2E needs an authenticated `claude` on PATH, ~5 min)
make e2e             # E2E only
make fmt             # cargo fmt
```

### Test suites and gates

**Manager integration tests** (fake `claude`, no real API):

```bash
make test-manager
# equivalent:
cargo test --test manager -- --test-threads=1
```

Must run single-threaded. Two tests inside are gated:

- `CLAUDIO_E2E=1` — enables `test_real_claude_session` (sends a prompt to the
  real `claude`, costs tokens, requires an authenticated `claude` on PATH).
- `CLAUDIO_PROXY_TEST=1` — enables the proxy-integration test. It builds and
  runs claude-proxy from source (`$CLAUDIO_PROXY_SRC`, default `../claude-proxy`;
  needs `go`).
  The test for real-claude via proxy additionally needs `CLAUDIO_E2E=1` and
  `CLAUDIO_PROXY_URL`.

**Print-mode / API E2E tests** (tests/integration.rs):

```bash
CLAUDIO_E2E=1 CLAUDIO_CADENCE=0 cargo test --test integration -- --test-threads=1 --nocapture
```

Without `CLAUDIO_E2E=1` the integration tests silently no-op. They call the
real Claude API and must run single-threaded.

**Remote / SSH tests** (tests/remote.rs):

```bash
CLAUDIO_SSH_TEST_HOST=<host> cargo test --test remote -- --test-threads=1 --nocapture
# diag subcommands (compiled in by default; may be gated behind --features diag in future):
cargo test --features diag --test remote -- --test-threads=1 --nocapture
```

Tests `t1`, `t7`, `t8` are unit-style and always run. Tests `t2`–`t6` require
`CLAUDIO_SSH_TEST_HOST` to be set to a reachable SSH host alias.

**Web UI** (`claudio web`; fake claude and fake proxy, no API key):

```bash
cargo test --test web -- --test-threads=1   # server: auth, hub, spawn, proxy, plugins (tests/web.rs)
node --test web/tests/*.test.mjs            # browser logic, no browser (glob form: the directory form fails on Node 22)
make test-web                               # node tests, then Playwright e2e against a debug build (web/e2e)
```

The e2e starts a real `claudio web` per spec file in `~/.cache/claudio/web-e2e/<hex>/`
with an isolated env (`XDG_*`, `HOME`, `CLAUDE_CONFIG_DIR`, `CLAUDIO_CLAUDE_PATH` = a fake
claude), so it never touches your own daemon or `~/.claude`. Needs Chromium in
`~/.cache/ms-playwright` and `npm ci` in `web/e2e`. Run one spec: `cd web/e2e && npx playwright test board`.

- Run one unit test: `cargo test --bin claudio <name_substring>`.
- Run one E2E test: `CLAUDIO_E2E=1 CLAUDIO_CADENCE=0 cargo test --test integration <name> -- --test-threads=1 --nocapture`.
- CI runs only the release build and unit tests, on Linux and macOS (claudio is Unix-only). Keep code compiling on both.

## Architecture

### Session manager (bare `claudio`)

A bare `claudio` calls `tui::run()`. The manager's architecture spans:

**`proto.rs`** — wire framing shared by daemon and client. Frames are
`[u32-BE length][u8 tag][payload]`. Tag `J` carries JSON control messages
(`Envelope { req, msg: Msg }`); unknown `op` fields are ignored. Tag `D`
carries terminal bytes: `[16-byte session UUID][raw bytes]`. The protocol
version (`PROTO = 1`) is embedded in the socket file name so old and new
daemons coexist.

**`daemon/`** — the per-host session daemon (invoked as `claudio --daemon`):
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
  runs); a successful install calls `Manifests::forget_failures`.
- `manifest.rs` — `Manifest` (a proxy plugin's manifest, parsed by
  `crate::plugins::manifest`): checks the op itself (name `[a-z0-9-]{1,40}`,
  64-hex hash, https, ≤ 64 secret names by `key_ok`, ≤ 256 disabled item
  ids), keeps `<cache>/plugins/<name>.toml` + `.sha256` (downloaded once:
  30 s connect, 120 s, 256 KiB, https redirects only, temp file, verified,
  renamed; 0700 dir, ≤ 32 manifests, one download per name), and
  replies `Applied` before any install runs: the `--mcp-config` and
  `--agents` JSON (placeholders only), the sanitized instructions, the
  `[env]`, and per id `installed` / `present` / `skipped` (`installing`,
  `yours`, `missing secret X`, a failure). It reads Claude Code's files
  under `$CLAUDE_CONFIG_DIR` else `~/.claude` (`.claude.json`,
  `plugins/known_marketplaces.json`, `plugins/installed_plugins.json`,
  `agents/`; regular files of this uid, ≤ 16 MiB, never logged) and never
  writes there itself: one task per manifest hash runs `claude plugin
  marketplace add … --json` / `claude plugin install … --json` (never `-y`),
  each holding `Daemon::updating` (shared with `claude update`), stdin null,
  output capped, markers scrubbed, `GIT_TERMINAL_PROMPT=0`, own process
  group, 120 s. Outcomes are remembered per (hash, id): installed (daemon's
  life), `needs manual install` (daemon's life), a failure (retried after
  15 s, doubling per failure in a row up to 300 s, reset by a success;
  `forget_failures` drops them all; meanwhile the reason ends `(retrying
  in N s)`, `proto::retrying`). Without a claude, `run_claude` fails at
  once (`claude is not installed`), never taking the lock.
  `Daemon::claude_flags` (one `claude --help`) says whether claude knows
  `--plugin-dir`, `--agents`, `--mcp-config`; `command()` drops such a pair
  when it does not, and `Applied.dropped` names them. Stale zips of a
  pre-manifest build are removed at daemon start.
- `host.rs`, `stats.rs` — host facts for `Welcome`, directory listing, CPU and
  memory samples. `ctl.rs` — `claudio daemon status|stop|restart` (status
  shows the claude path and version the daemon uses).

**`client.rs`** — generic client connection and local daemon autostart.
Connects to the socket; if unavailable, starts the daemon (double-fork +
`setsid`, or `systemd-run --user` on systemd hosts) and retries.

**`freshness.rs`** — keeps each host's daemon at least as new as the claudio
binary on it. `Welcome.build`/`build_time` identify the daemon's binary;
`ensure_current` runs before the local TUI connects and in the remote bridge
before it relays. An older daemon is shut down (SIGTERM by the lock's PID if
it predates `Shutdown`) and restarted from the current binary, unless
`busy` (a claude turn or permission prompt, or a `--plain` session): then it
is left running, the client is flagged `daemon_outdated` (over SSH the
bridge prints `DEFERRED` on stderr), and the TUI reconnects that host once it
is idle (checked every 2 min).

**`tui/`** — the manager UI, driven by tokio + ratatui:
- `mod.rs` — I/O edge: owns the terminal, runs the event loop, gathers the
  `AppConfig`, drives daemon connections and every `Effect`. Emits OSC 9
  notifications (`ESC ] 9 ; … BEL`) for background sessions that need
  attention. Runs the `latex-render` worker thread (`Effect::RenderLatex` →
  `HostEvent::LatexRendered`). It renders each formula on a scratch thread
  (32 MiB stack) within a 150 ms budget, at most 2 abandoned at a time, and
  caches by source (LRU 1024). The same thread renders the reader's
  answers (`Effect::RenderAnswers` → `HostEvent::AnswersRendered`):
  markdown into a `reader::doc::Doc`, formulas through the same guard and
  cache. Its panic hook stays silent for panics on threads flagged
  `QUIET_PANIC`.
- `app.rs` — pure coordinator (`App::new(AppConfig)`): processes events into
  state transitions and returns `Effect` values. No I/O here; all I/O is in
  `mod.rs`. `Mode::Manager` or `Mode::Plain`.
- `sessions.rs` — `SessionView` and spawning (`send_spawn`/`send_with_proxy`,
  the one path that builds spawns, respawns and their proxy env), recovery,
  the wizard's opening.
- `interaction.rs` — keys, mouse, modals (`Modal`), daemon events and replies.
- `wizard.rs` — the new-session wizard (start screen → directory browser →
  resume?), a pure state machine.
- `git_view.rs` + `git_app.rs` — the git viewer (Alt+l): pure view state
  machine, and its `App` glue.
- `stats_view.rs` — the multi-page proxy stats popup (Alt+s).
- `latex.rs` — `<latex>` formulas, pure: `Formula` (source, one-line form,
  display- and text-style layouts, match keys), the `too_complex`
  pre-check, `render` (the `typeset` engine; `aligned`-like environments
  row by row, aligned on their first `&`) and `overlay`. The overlay swaps matched on-screen tags for
  their one-line rendering in the pane's buffer, cell by cell, and moves
  no row.
- `typeset/` — the 2D layout engine, pure: TeX's display and text styles
  over the rust-latex-parser tree (`typeset`), the `\sqrt[n]` rewrite, the
  rows of `aligned`/`gather`/`array`, the math-font tables (`mathfont.rs`,
  ported from term-maths) and the v0.4.0 one-line walk (`walk.rs`).
- `reader/` — the reader view (Alt+u): `doc.rs` (an answer as markdown
  blocks with formula placeholders; runs on the worker), `layout.rs` (a
  `Doc` wrapped to a width, line boxes for formulas) and `mod.rs`
  (`ReaderView`: answer, scroll, Tab list, layout cache). Drawn by
  `ui/reader.rs`.
- `formulas.rs` — the lines of the reader's formulas list (Tab).
- `extensions/` — the extension registry (`mod.rs`: built-ins latex,
  diagram, search, snippets; `Registry`, `ActionResult`; the proxy plugins
  as `Source::Plugin` rows, `Registry::set_plugins`), external
  manifests (`manifest.rs`: `~/.config/claudio/extensions/<name>/`, the
  HTML-tag rule, admission, at most 16), protocol v1 (`protocol.rs`: request
  lines, result caps and sanitizing) and the extension host (`host.rs`: one
  program per extension, lazy start, scrubbed env, own process group,
  ≤ 16 in flight, render 1 s / action 15 s, 3 failures in 5 min disable it,
  killed on exit; it knows every loaded external extension, and one turned
  off is refused unstarted and its program stopped; driven by `mod.rs`,
  never by `App`).
- `extensions_view.rs` — the extensions screen (Alt+i, or Alt+z e): rows,
  details, toggles. A toggle flips the registry at once and emits
  `Effect::SetExtension` (host; externals) and `Effect::SaveExtension`
  (`mod.rs` writes `[extensions] name = on` with `config::save_extension`,
  a minimal line edit of config.toml, never a whole-file rewrite); a
  plugin's toggle (Space) also updates `App::plugins.config`, Enter on a
  plugin's row opens its setup screen, and `r` refetches the plugin lists
  and manifests and asks every host again. Drawn by `ui/extensions.rs`.
  The leader keys `u` and `e` are reserved.
- `plugin_setup.rs` — a proxy plugin's setup screen, pure: every item of
  its manifest with a checkbox (Space, `a`, `n`, Enter saves to
  `[plugins.<name>]` through `Effect::SavePluginChoices` and
  `config::save_plugin_choices`, Esc). It opens on first sight (no table
  yet; Esc then defers for the run: nothing written or sent) and from the
  extensions screen; it waits its turn in the confirm queue (`Queued`) and
  swallows every key. `--plain` holds its spawn (`App::plain_pending`)
  until the profile's new manifests arrived (≤ 20 s) and their screens
  closed. Drawn by `ui/plugin_setup.rs`.
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
  (`claude_checked`). A successful install asks that host again for every
  manifest (`reapply_host`).
- `plain.rs` — `claudio --plain`: `Mode::Plain` is one local, ephemeral
  session full screen (no tab/status bar, wizard or state.json).
  `Keymap::build_plain` keeps only Alt+h/s/l/e/u/z/i; the session is killed when
  the UI exits, and the UI exits with claude.
- `connections.rs` — local + SSH connections, reconnect backoff.
- `ui/` — ratatui rendering: `mod.rs` (layout, tab bar, pane, popups),
  `status.rs` (two-line bottom bar; line 2 is prioritised segments — cpu,
  ctx gauge, git, mem, 5h, conversation, 7d — dropped whole when narrow),
  `stats.rs`, `git.rs`, `confirm.rs`, `reader.rs`, `leader.rs`,
  `extensions.rs`.
- `fmt.rs` — shared text and number formatting.
- `keymap.rs` — `DEFAULT_BINDINGS` table (the single source of truth); `Keymap`
  (defaults + config overrides); `parse_key_spec`; collision detection.
  `Alt+Shift+<digit>` (go to tab N) is fixed, outside the table.
- `state.rs` — `ClientState`: persisted session order, names and recent dirs
  (`state.json`).
- `proxy_state.rs` — per-profile proxy stats and per-session credential cache.
- `plugins.rs` — proxy plugins, pure: each profile's list (`plugins.json` at
  start, `Effect::FetchPlugins` at start, every 15 min and on `r`), each
  plugin's manifest (`Effect::FetchManifest`: fetched by the client, 256
  KiB, hash-checked, memory only), the registry's `Source::Plugin` rows,
  each `(host, plugin)` `Applied` answer (`ApplyState`; asked again every
  30 s while it lists `installing`, and a second after a `(retrying in N
  s)` retry is due, no sooner than 30 s), and what a spawn gets
  (`compose`: per answered plugin, ≤ 8, `--agents` then `--mcp-config`
  after every other arg; env layers `[env]`, then the secrets, then the
  profile env; the instructions in `SessionExtensions.instructions`, cut at
  just under 8 KiB (8158 bytes)). A host is asked only once the member saved the plugin's choices.
  A spawn whose host has not answered is deferred
  (`Effect::SpawnWithProxy { plugins, pending }`; `tui/mod.rs` waits
  ≤ 20 s).
- `notifications.rs` — debounce logic for attention notifications.
- `selection.rs` — copy-on-select (`[ui] copy_on_select`, default on): the
  pure `Selection` (anchor/head spans, cell/word/line unit, `contains`),
  multi-click counting and the OSC 52 encoder. A left drag in the pane
  selects only while the session's mouse reporting is off (otherwise the
  mouse is forwarded as before); the release emits `Effect::CopyToClipboard`
  with `Screen::text` and a "copied N chars" notice. The selection is
  cleared by a click, key, paste, tab switch, resize or re-attach. Glue:
  `App::select_mouse` in `interaction.rs`; highlight in `ui/mod.rs`.
- `test_support.rs` — shared fixtures for the TUI's unit tests.

**`remote/`** — SSH bootstrap and bridge:
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

**`proxy/`** — claude-proxy HTTP client and profile management:
- `profile.rs` — load/save profiles from `config.toml` (`[proxy]` section,
  mode 0600); `from_env()` parses `CLAUDIO_PROXY_URL=<token>@host`.
- `resolve.rs` — `ProxyChoice` and profile resolution, shared by the TUI and
  `--plain`.
- `env.rs` — `session_env(profile, config)`: builds the env for a
  proxy-backed session (falls back to built-in model defaults when the
  proxy's `/v1/claudio/config` is unreachable); `env_diff` is the one rule
  for what a launched claude inherits (session markers scrubbed, an API key
  dropped when a proxy token is set).
- `cmd.rs` — `claudio proxy login|status|logout|use`.
- `login.rs` — the browser sign-in (`/v1/claudio/login/start|poll`, device
  style: the member types the code; `--token` pastes one instead).
- `plugins.rs` — `GET /v1/claudio/plugins`, `admit` (names, https, hash,
  env keys a plugin may not set (`key_ok`), caps), the `plugins.json` cache
  (0600, one entry per profile; a 401/403 drops it), `fetch_manifest`.
  `Debug` never shows a value. `test_http_ok` lets plain http to
  `127.0.0.1` through in a debug build run with
  `CLAUDIO_TEST_LOOPBACK_HTTP=1` (the manager tests' fake proxy), never in
  a release build.

**`plugins/`** — `manifest.rs`, the one parser of a plugin manifest (TOML,
≤ 256 KiB; marketplaces, plugins, MCP servers with their secret names,
agents, instructions, env), shared by the client and the daemon: every cap,
charset and `${NAME}` placeholder rule, item ids (`plugin:<name>@<market>`,
`mcp:<name>`, `agent:<name>`, `instructions`, `env`), and the renderers
(`mcp_json`, `agents_json`, checked against 64/32 KiB at parse time).
Reasons name ids and fields, never values.
- `api.rs` — HTTP client for `/v1/claudio/config`, `/models`, `/me/stats`,
  `/pool/health` and `/session` (which credential a claude session is on).

**`term/`** — terminal primitives:
- `mod.rs` — `alacritty_terminal::Term`-based screen + snapshot logic.
- `keys.rs` — key → byte encoding for input forwarding to the PTY.

**`claude/`** — claude knowledge:
- `mod.rs` — `SESSION_MARKERS` (env a parent claude sets for its children),
  `binary()`, `exec()`.
- `projects.rs` — scan `~/.claude/projects/` for existing sessions (feeds the
  wizard's resume list), and `recent_answers` (the reader's last 10 answers
  of a transcript tail: real prompts and the assistant text after them).
- `hooks.rs` — `__hook <Event> <socket> <token>` relay: sends the hook payload
  to the daemon socket. Also `__context file <path>`, the SessionStart hook
  that prints the session's context file (the extensions' instructions: no
  symlinks, a regular file of the uid, 16 KiB, sanitized) as one JSON line,
  and `__context latex` for sessions of older daemons; neither touches the
  socket.
- `latex.rs` — `lens_tags` (the items of any tag lenses in an assistant
  text, code excluded, with their byte ranges), `extract_latex` and
  `latex_tags` (its `<latex>` wrappers) and
  `sanitize`, the character filter every formula and answer goes through
  in the daemon and again in the client.
- `state.rs` — session state tracker derived from hook events.
- `update.rs` — version parsing and the daily release-channel check.

### Web UI (`claudio web`, `src/web/`, `web/`)

`claudio web` (`src/main.rs` routes it; flags in `docs/web.md`) serves the manager to a
browser: one more client of the daemons, like the TUI, over the same socket and ssh bridge.
`docs/web-protocol.md` is the contract between server and page (version 1); change it first,
in its own commit. User docs, the security model and the TUI parity table: `docs/web.md`.

**`src/web/`** (axum, loopback only):
- `mod.rs` — args (`--listen`, `--allow-host`, `--no-open`, `--print-url`, `--rotate-token`),
  routes, startup, graceful shutdown (tabs closed 1001, `web-state.json` written).
- `auth.rs` — token file (`<config>/web-token`, 0600, constant-time compare), cookie sessions
  (24 h idle, 64), login rate limit, `guard` (Host 421, Origin 403, cookie 401), the CSP
  (`csp`, with the import-map hash) and security headers on every response.
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
- `state.rs` — `web-state.json` (layout, order, per-session proxy name, remote hosts); reads
  the TUI's `state.json`, never writes it.
- `spawn.rs`, `proxy.rs`, `plugins.rs`, `ext.rs` — spawns composed as the TUI does
  (`tui::plugins::compose`, `Msg::merge_env`), proxy stats/credential/switch, the plugin
  state machine re-expressed async, and the extension registry, external host, snippets and
  config.toml toggles.
- `build.rs` — gzips `web/` into `$OUT_DIR`, writes the import map's SHA-256 for the CSP.

**`web/`** (no build step; ES modules through the import map):
- `index.html` — the import map (the only inline script) and the stylesheet links.
- `app/` — `boot.js` (login, CSP violation log `window.__claudioCsp`), `store.js` (model +
  WebSocket), `proto.js`, `term.js` (xterm.js), pure logic with node tests (`queue.js`,
  `keymap.js`, `layout.js`, `panes.js`, `wizard.js`, `model.js`, `notify.js`, ...);
  `app/ui/` Preact components (`htm`, signals); `app/render/` markdown, KaTeX, mermaid,
  highlight, all behind DOMPurify.
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
  bytes.** All `wait_for` calls in `tests/manager.rs` operate on the
  `alacritty_terminal::Term` VT model. Asserting on raw bytes is fragile and
  forbidden.
- **Don't send `ESC` immediately before `Alt+key` in tests.** `ESC ESC q` is
  ambiguous — the VT parser may interpret it as two separate `ESC` sequences.
  Use the crossterm key-event encoding instead.
- **Secrets never go in argv or state files.** Proxy tokens and plugin env
  are passed only through `Spawn.env` over the Unix socket; a `Manifest`
  request carries secret names, never values. The journal and `state.json`
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
  are a `Manifest` op, not a `SpawnSpec` field: an old daemon's error
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
- **`app.rs` is pure; I/O is only in `tui/mod.rs`.** `App::update` takes
  events and returns `Effect` values. `mod.rs` drives the effects. Don't add
  I/O to `app.rs`.
- **Formulas are never rendered on the UI thread.** On hostile input inside
  the 2048-byte cap, rust-latex-parser can take a minute or overflow a
  2 MiB stack. Call `latex::render` only through `render_formula` in
  `tui/mod.rs` (pre-check, scratch thread, budget, abandon cap), never from
  `App`, a draw, or a test with hostile input (unit tests may call it on
  benign formulas only). The same goes for the `typeset` engine, and the
  reader's markdown is parsed on the worker too; `App` and the drawing only
  lay out what the worker made.

### Non-obvious constraints (don't "fix" these — web)

- **`DESIGN.md` is the web UI's style reference; every UI change follows it.** Dark only;
  surfaces Void `#0b0e12` → Carbon `#181a1d` → Graphite `#1f2124`, 1px Slate `#303235`
  hairlines, radius 1px, no shadows, weight 400 only, mono (JetBrains Mono) for every
  technical label/tab/badge/button with wide tracking, sans (Inter) for prose; Phosphor Green
  `#00d892` only for active/status, Deep Teal `#002923` the only filled button. Two owner
  decisions on top of it: text sizes stay at the owner's larger scale (not DESIGN.md's
  marketing-site 11–14px), and session states map to working = green, waiting = Syntax
  Violet `#c58aff`, stopped = Steel `#818284`, error = Syntax Coral `#ff6285`. The tokens live
  in `web/styles/tokens.css`; never hard-code a colour, radius or font outside it.

- **The import map is the only inline script, and its hash is in the CSP.** `build.rs`
  hashes the map's exact text; editing it changes the hash (a rebuild picks it up). No other
  inline script, inline handler, `eval` or CDN, ever. The e2e asserts zero
  `securitypolicyviolation` events.
- **Vendored files under `web/vendor/` are byte-identical to upstream.** Delete whole files
  (mermaid reduction rule) but never edit one; every file is in `MANIFEST.txt`.
- **The web never writes `state.json`.** It reads it; its own state is `web-state.json` and
  `web-token`, both written with `paths::write_atomic` (0600).
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

### Print-mode pipeline (`-p`)

1. `cli.rs` handles arguments *dynamically*. It owns only a few flags (`-p`, `--api`, `--fast`, `--log-messages[-file]`, `--output-format`, `--input-format`, `--settings`, `--session-id`), drops `--bare` and `--no-session-persistence` with a warning (they would disable hooks and the JSONL), and forwards everything else verbatim. Never add a mirror of claude's flag grammar. Wrapper-only knobs belong in `CLAUDIO_*` env vars (`WrapperEnv`) so the CLI surface stays exactly claude's. `cli::HELP_APPENDIX` documents these flags and env vars and is appended to `claudio --help`; update it when you add one.
2. `driver.rs` spawns claude under a PTY (`portable-pty`). `PtySession` is the reusable unit: spawn, then `turn(prompt)` repeatedly. `driver::run` is the single-shot `-p` path built on it.
3. `vt.rs` answers the terminal probes that Ink sends at startup (DA1/DA2, DSR, XTVERSION, OSC colors…), byte-for-byte as xterm-380 would.
4. `hooks.rs` signals completion. `SessionStart`/`Stop` hooks are injected via a merged `--settings` and report back over loopback TCP (default) or a watched directory (`CLAUDIO_HOOK_TRANSPORT=file`). The hook command is a copy of this binary under a random hex name in `/tmp`, so `main.rs` has to recognize relay invocations first: argv `<EventName> <port>` (concealed relay) or `__hook <Event>` (legacy).
5. `session.rs` reads the session JSONL, which is the source of truth for the final assistant text and usage. The terminal render is never parsed for answers.
6. `emit.rs` formats output as `text`, `json`, or `stream-json`, shaped like `claude -p`.

### API server (`--api`, `src/api/`)

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
