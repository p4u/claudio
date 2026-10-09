# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`claudio` is a session manager TUI and drop-in wrapper for the `claude` CLI. It has four modes, chosen in `src/main.rs`:

- **No arguments (bare `claudio`)**: it opens the **session manager** TUI — a terminal multiplexer for many Claude Code sessions, local and over SSH.
- **No `-p`/`--print`**: it `exec`s the real `claude` unchanged (passthrough).
- **`-p`**: it emulates print mode by driving the *interactive* TUI under a PTY. It types the prompt, waits for the `Stop` hook, then reads the answer and real token usage from the session JSONL.
- **`--api`**: it serves an OpenAI-compatible server (axum) on the same PTY backend, using a pool of persistent interactive sessions.

**claudio never invokes `claude -p`.** Every path goes through the interactive TUI. Don't install the binary *as* `claude`, because it has to find the real one on `PATH` (or `$CLAUDIO_CLAUDE_PATH`).

## Commands

```bash
make                 # release build → target/release/claudio
make static          # static musl binary → dist/claudio (ARCH=aarch64 for cross)
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

- Run one unit test: `cargo test --bin claudio <name_substring>`.
- Run one E2E test: `CLAUDIO_E2E=1 CLAUDIO_CADENCE=0 cargo test --test integration <name> -- --test-threads=1 --nocapture`.
- CI runs only the release build and unit tests, on Linux, macOS, and Windows. Keep code compiling on all three.

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
- `server.rs` — accepts Unix socket connections, uid-checks peers, runs the
  per-client loop.
- `session.rs` — one actor task per live session: owns the PTY writer, VT
  engine (`alacritty_terminal`), and subscriber list. Fans PTY output to
  all attached clients. Hook events (`__hook <Event> <socket> <token>`) are
  authenticated by uid plus a per-spawn token, enforced with a read deadline
  and size cap. On `SessionStart` the claude session id is journaled
  immediately so recovery works even when no client is attached.
- `journal.rs` — durable session list (`daemon-sessions-v1.json`): writes via
  temp + fsync + rename (`paths::write_atomic`). Never stores secrets or the
  daemon's `--settings` injection. On fresh daemon start, non-killed entries
  are re-spawned with `claude --resume <claude_session_id>`.
- `ctl.rs` — `claudio daemon status|stop|restart`.

**`client.rs`** — generic client connection and local daemon autostart.
Connects to the socket; if unavailable, starts the daemon (double-fork +
`setsid`, or `systemd-run --user` on systemd hosts) and retries.

**`tui/`** — the manager UI, driven by tokio + ratatui:
- `mod.rs` — I/O edge: owns the terminal, runs the event loop, drives
  daemon connections. Emits OSC 9 notifications (`ESC ] 9 ; … BEL`) for
  background sessions that need attention.
- `app.rs` — pure coordinator (`App`): processes events into state transitions
  and returns `Effect` values. No I/O here; all I/O is in `mod.rs`.
- `sessions.rs` — `SessionView`: the UI-side view of a session; `label()`,
  state glyphs, attention logic.
- `interaction.rs` — handles daemon events, wizard transitions, rename, kill.
- `wizard.rs` — the new-session wizard (where → dir → resume? → options).
- `connections.rs` — manages local + SSH connections, reconnect backoff.
- `ui.rs` — ratatui rendering: tab bar, status bar, term pane, popups
  (overview, help, proxy stats, rename, kill confirm).
- `keymap.rs` — `DEFAULT_BINDINGS` table (the single source of truth); `Keymap`
  (defaults + config overrides); `parse_key_spec`; collision detection.
- `state.rs` — `ClientState`: persisted session order and names
  (`state.json`).
- `proxy_state.rs` — per-session proxy UI state (stats, badge).
- `notifications.rs` — debounce logic for attention notifications.

**`remote/`** — SSH bootstrap and bridge:
- `bootstrap.rs` — `ensure_remote(host)`: runs `__probe` + `uname -sm` in one
  ssh round trip; uploads the binary if missing or stale (SHA-256 check).
  Same-platform: uploads self. Cross-platform: downloads release asset to
  `~/.cache/claudio/<version>/` and uploads that.
- `bridge.rs` — `--slave` mode: the stdio↔socket bridge the local client runs
  on the remote as `ssh HOST ~/.local/bin/claudio --slave`. Starts the remote
  daemon if not running (`systemd-run --user` preferred, `setsid` fallback).
- `hosts.rs` — SSH host candidates: MRU list (`hosts.json`) + `~/.ssh/config`
  aliases.
- `probe.rs` — `__probe` subcommand: prints `{version, proto, os, arch,
  build}` as JSON.

**`proxy/`** — claude-proxy HTTP client and profile management:
- `profile.rs` — load/save profiles from `config.toml` (`[proxy]` section,
  mode 0600); `from_env()` parses `CLAUDIO_PROXY_URL=<token>@host`.
- `env.rs` — `session_env(profile, config)`: builds the `SpawnSpec.env` for a
  proxy-backed session. Falls back to built-in model defaults when the proxy's
  `/v1/claudio/config` is unreachable.
- `cmd.rs` — `claudio proxy login|status|logout|use`.
- `api.rs` — HTTP client for `/v1/claudio/config`, `/v1/claudio/me/stats`,
  `/v1/claudio/pool/health`.

**`term/`** — terminal primitives:
- `mod.rs` — `alacritty_terminal::Term`-based screen + snapshot logic.
- `keys.rs` — key → byte encoding for input forwarding to the PTY.

**`claude/`** — claude knowledge:
- `projects.rs` — scan `~/.claude/projects/` for existing sessions (feeds the
  wizard's resume list).
- `hooks.rs` — `__hook <Event> <socket> <token>` relay: sends the hook payload
  to the daemon socket.
- `state.rs` — session state tracker derived from hook events.

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
- **Secrets never go in argv or state files.** Proxy tokens are passed only
  through `Spawn.env` over the Unix socket. The journal and `state.json` store
  only profile names, never tokens.
- **`app.rs` is pure; I/O is only in `tui/mod.rs`.** `App::update` takes
  events and returns `Effect` values. `mod.rs` drives the effects. Don't add
  I/O to `app.rs`.

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
