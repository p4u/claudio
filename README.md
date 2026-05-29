# claude-poc

A **drop-in wrapper for `claude`** that emulates `claude -p` (print mode) by
driving the *interactive* Claude Code TUI under a real pseudo-terminal, instead
of calling `claude -p`.

> **What this is.** A responsible-disclosure proof of concept. It demonstrates
> that client-side restrictions on programmatic Claude Code use cannot be
> enforced: every signal that would distinguish "human" from "automation" lives
> on the user's machine, and the model requests produced are identical either
> way. Full argument in [`../REPORT.md`](../REPORT.md).
>
> It uses only your own authenticated session and documented, supported Claude
> Code features. It does **not** bypass authentication or billing — every
> request counts against your account, and the wrapper prints the real token
> usage to prove it.

## Drop-in behavior

The wrapper proxies the real `claude` (found on `PATH`, or via
`$CLAUDE_POC_CLAUDE_PATH`):

- **Without `-p`/`--print`** it `exec`s the real `claude` unchanged — interactive
  sessions, subcommands (`auth`, `mcp`, …), `--help`, and `--version` are 100%
  native.
- **With `-p`/`--print`** it emulates print mode: it forwards every other flag
  verbatim to interactive claude, types your prompt into the TUI, waits for the
  turn to finish via the `Stop` lifecycle hook, and reads the exact answer (and
  real token usage) from the canonical session JSONL.

Because it only special-cases the flags it owns (`-p`, `--output-format`,
`--input-format`, `--settings`, `--session-id`) and forwards everything else,
**a new claude flag keeps working with no code change** — there is no hardcoded
copy of claude's grammar.

Point any tool that runs `claude -p ...` at `claude-poc -p ...` (rename the
invocation or alias it). Do **not** install it *as* `claude` — the wrapper needs
to find the real `claude`.

## Build

Requires a Rust toolchain and a locally authenticated `claude` on `PATH`.

```bash
make            # native release build → target/release/claude-poc
make static     # portable static musl binary → dist/claude-poc (≈1 MB, no libc dep)
make test       # 32 unit tests (no network/claude needed)
```

`make static` uses rustup's bundled musl toolchain (self-contained — no
`musl-gcc` required). For a host with no Rust toolchain at all, `make
static-docker` builds the same static binary inside a container.

## Use

```bash
# default text, like `claude -p`
claude-poc -p "Explain quicksort in one sentence."

# JSON result with the real token-usage object
claude-poc -p --output-format json "Reply with exactly: OK"

# streaming-shaped events
claude-poc -p --output-format stream-json "Reply with exactly: OK"

# any claude flag is forwarded verbatim
claude-poc -p --model opus --dangerously-skip-permissions \
    --allowedTools Bash Read "Run 'uname -a' and report the kernel."

# prompt from stdin
git diff | claude-poc -p --output-format json "Summarize the staged diff."

# unambiguous prompt delimiter (recommended for scripts / dashy prompts)
claude-poc -p --model opus -- "--this is definitely the prompt--"
```

### Flags

The command-line surface **is claude's** — every flag is forwarded to claude
except the few the wrapper must own:

| Owned flag | Handling |
|------------|----------|
| `-p`, `--print` | triggers print-mode emulation (not forwarded) |
| `--output-format <text\|json\|stream-json>` | wrapper formats the output |
| `--input-format` | only `text` supported by this backend |
| `--settings <file-or-json>` | merged with the wrapper's hooks, then forwarded |
| `--session-id <uuid>` | captured (to locate the transcript) and forwarded |

Two claude flags are dropped (with a warning) because they would disable the
machinery the wrapper depends on: `--bare` (skips hooks) and
`--no-session-persistence` (suppresses the session JSONL).

### Wrapper-only controls (environment variables)

So the CLI surface stays byte-for-byte claude's, wrapper knobs are env vars:

| Variable | Default | Meaning |
|----------|---------|---------|
| `CLAUDE_POC_DEBUG` | `0` | trace timeline to stderr |
| `CLAUDE_POC_TIMEOUT_SEC` | `300` | wall-clock cap |
| `CLAUDE_POC_HOOK_TRANSPORT` | `tcp` | `tcp` or `file` (sandboxes blocking loopback) |
| `CLAUDE_POC_RAW_LOG` | — | dump the raw PTY byte stream to this path |
| `CLAUDE_POC_COLS` / `CLAUDE_POC_ROWS` | `120`/`40` | PTY size |
| `CLAUDE_POC_CLAUDE_PATH` | `claude` | path to the real claude binary |

## How it works (and why it's version-robust)

It depends only on stable, documented contracts:

1. A **PTY** (`portable-pty`; ConPTY on Windows) — Ink needs a TTY.
2. Answering the **terminal device queries** Ink emits at boot, via a real VT
   parser (`vte`).
3. The **`SessionStart`/`Stop` lifecycle hooks** (registered through inline
   `--settings`), relayed back to the wrapper by *our own binary* over loopback
   TCP or a watched file. `Stop` is a deterministic turn-finished edge — not
   screen-scraping.
4. Typing the prompt the way a person does (the TUI's core input contract),
   rather than relying on any version-specific shortcut.
5. Reading the **session JSONL** for the exact final text and real usage.

## Run in Docker

The image bundles Node + the Claude Code CLI + the wrapper; your host login
state is mounted in.

```bash
docker compose build
docker compose run --rm claude-poc -p --dangerously-skip-permissions "Reply with exactly: DOCKER_OK"
docker compose run --rm claude-poc -p --output-format json "Reply with exactly: OK"
```

**Auth.** The wrapper passes the whole environment through, and the compose file
forwards the `ANTHROPIC_*` variables **by name** (no secrets are written to
disk). So whatever your host uses works in the container:

- **Env-var / gateway auth** (`ANTHROPIC_AUTH_TOKEN` + `ANTHROPIC_BASE_URL`, or
  `ANTHROPIC_API_KEY`): export them in your shell — they flow straight through.
  This is the simplest path and needs no mounts.
- **OAuth / keychain auth:** the `~/.claude` mounts carry it *only if it's
  file-portable*. Claude Code often keeps the live token in the OS keychain
  (macOS Keychain; Linux libsecret) and leaves `~/.claude/.credentials.json` as
  a stale fallback — a keychain-backed host then mounts an expired token and
  gets `401` in the container. In that case use env-var auth above, log in
  inside the container (`docker compose run --rm --entrypoint claude claude-poc
  /login`), or just run the native binary.

If you see `401 Invalid authentication credentials`, your auth isn't reaching
the container — set the `ANTHROPIC_*` vars in your shell and retry.

Other notes: mounts are read-write (claude writes the session JSONL the wrapper
reads). If host uid:gid ≠ 1000:1000, run with `USERSPEC="$(id -u):$(id -g)"`.
Pin the CLI with `CLAUDE_VERSION=2.1.156 docker compose build`.

## Evidence

`make evidence` (or `scripts/capture-evidence.sh`) runs representative cases
against real `claude` and writes artifacts to `../evidence/`: transparent
passthrough, text/json/stream-json, a tool-use turn, a variadic-flag case, a
`--model` passthrough, the file transport, and a timing trace.

## Layout

```
src/cli.rs       transparent/dynamic arg handling (owns 5 flags, forwards the rest)
src/vt.rs        terminal-probe responder (vte)
src/hooks.rs     inline --settings merge, self-exec relay, tcp + file transports
src/session.rs   session-JSONL parse, stop-reason logic, flush-race retry
src/driver.rs    PTY lifecycle, type-the-prompt, Stop-hook completion, teardown
src/emit.rs      text / json / stream-json formatters
```

## Scope & ethics

Single account, local only, own authenticated session, no auth or billing
bypass, fully documented. See [`../REPORT.md` §7](../REPORT.md).
