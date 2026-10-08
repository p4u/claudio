# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`claudio` is a Rust drop-in wrapper for the `claude` CLI, written as a responsible-disclosure proof of concept. It has three modes, chosen in `src/main.rs`:

- **No `-p`/`--print`**: it `exec`s the real `claude` unchanged (passthrough).
- **`-p`**: it emulates print mode by driving the *interactive* TUI under a PTY. It types the prompt, waits for the `Stop` hook, then reads the answer and real token usage from the session JSONL.
- **`--api`**: it serves an OpenAI-compatible server (axum) on the same PTY backend, using a pool of persistent interactive sessions.

**claudio never invokes `claude -p`.** Every path goes through the interactive TUI. Don't install the binary *as* `claude`, because it has to find the real one on `PATH` (or `$CLAUDIO_CLAUDE_PATH`).

## Commands

```bash
make                 # release build → target/release/claudio
make static          # static musl binary → dist/claudio (ARCH=aarch64 for cross)
make test-unit       # unit tests only: cargo test --locked --bin claudio (no claude needed)
make test-manager    # manager integration tests: fake claude, no API, --test-threads=1
make test            # unit + E2E (E2E needs an authenticated `claude` on PATH, ~5 min)
make e2e             # E2E only
make fmt             # cargo fmt
```

- Run one unit test with `cargo test --bin claudio <name_substring>`. Unit tests are `#[cfg(test)]` modules inside `src/`.
- Run one E2E test with `CLAUDIO_E2E=1 CLAUDIO_CADENCE=0 cargo test --test integration <name> -- --test-threads=1 --nocapture`. Without `CLAUDIO_E2E=1`, the tests in `tests/integration.rs` silently no-op. They call the real Claude API, so they cost tokens, and they must run single-threaded.
- CI (`.github/workflows/ci.yml`) runs only the release build and unit tests, on Linux, macOS, and Windows. Keep code compiling on all three.

## Architecture

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

`README.md` has the full env-var tables and client configs for pi, opencode, and hermes. Docker (`docker-compose.yml`) mounts `./workspace` as `/work`.
