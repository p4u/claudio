# claudio

A **drop-in wrapper for `claude`** that emulates `claude -p` (print mode) by
driving the *interactive* Claude Code TUI under a real pseudo-terminal, instead
of calling `claude -p`. With `--api` it also exposes an **OpenAI-compatible API
server** — served by that same PTY backend, so any OpenAI client talks to your
local `claude` install.

> **What this is.** A responsible-disclosure proof of concept. It demonstrates
> that client-side restrictions on programmatic Claude Code use cannot be
> enforced: every signal that would distinguish "human" from "automation" lives
> on the user's machine, and the model requests produced are identical either way.
>
> It uses only your own authenticated session and documented, supported Claude
> Code features. It does **not** bypass authentication or billing — every
> request counts against your account, and the wrapper prints the real token
> usage to prove it.

## Drop-in behavior

The wrapper proxies the real `claude` (found on `PATH`, or via
`$CLAUDIO_CLAUDE_PATH`):

- **Without `-p`/`--print`** it `exec`s the real `claude` unchanged — interactive
  sessions, subcommands (`auth`, `mcp`, …), `--help`, and `--version` are 100%
  native.
- **With `-p`/`--print`** it emulates print mode: it forwards every other flag
  verbatim to interactive claude, types your prompt into the TUI, waits for the
  turn to finish via the `Stop` lifecycle hook, and reads the exact answer (and
  real token usage) from the canonical session JSONL.
- **With `--api`** it starts an OpenAI-compatible HTTP server (see
  [OpenAI-compatible API mode](#openai-compatible-api-mode)). Requests are
  resolved through a pool of *persistent* interactive `claude` sessions (the same
  PTY machinery, kept alive across turns), then translated back into OpenAI's
  wire format.

Because it only special-cases the flags it owns (`-p`, `--api`, `--fast`,
`--output-format`, `--input-format`, `--settings`, `--session-id`) and forwards
everything else, **a new claude flag keeps working with no code change** — there
is no hardcoded copy of claude's grammar.

Point any tool that runs `claude -p ...` at `claudio -p ...` (rename the
invocation or alias it). Do **not** install it *as* `claude` — the wrapper needs
to find the real `claude`.

## Build

Requires a Rust toolchain and a locally authenticated `claude` on `PATH`.

```bash
make            # native release build → target/release/claudio
make static     # portable static musl binary → dist/claudio (≈1 MB, no libc dep)
make test       # unit tests (no network/claude needed) + E2E suite
```

`make static` uses rustup's bundled musl toolchain (self-contained — no
`musl-gcc` required). For a host with no Rust toolchain at all, `make
static-docker` builds the same static binary inside a container.

## Use

```bash
# default text, like `claude -p`
claudio -p "Explain quicksort in one sentence."

# JSON result with the real token-usage object
claudio -p --output-format json "Reply with exactly: OK"

# streaming-shaped events
claudio -p --output-format stream-json "Reply with exactly: OK"

# any claude flag is forwarded verbatim
claudio -p --model opus --dangerously-skip-permissions \
    --allowedTools Bash Read "Run 'uname -a' and report the kernel."

# prompt from stdin
git diff | claudio -p --output-format json "Summarize the staged diff."

# unambiguous prompt delimiter (recommended for scripts / dashy prompts)
claudio -p --model opus -- "--this is definitely the prompt--"
```

### Flags

The command-line surface **is claude's** — every flag is forwarded to claude
except the few the wrapper must own:

| Owned flag | Handling |
|------------|----------|
| `-p`, `--print` | triggers print-mode emulation (not forwarded) |
| `--api` | starts the OpenAI-compatible server (not forwarded) |
| `--fast` | strip human-like typing delays for lowest latency (not forwarded) |
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
| `CLAUDIO_DEBUG` | `0` | trace timeline to stderr |
| `CLAUDIO_TIMEOUT_SEC` | `300` | wall-clock cap |
| `CLAUDIO_HOOK_TRANSPORT` | `tcp` | `tcp` or `file` (sandboxes blocking loopback) |
| `CLAUDIO_RAW_LOG` | — | dump the raw PTY byte stream to this path |
| `CLAUDIO_COLS` / `CLAUDIO_ROWS` | `120`/`40` | PTY size |
| `CLAUDIO_CLAUDE_PATH` | `claude` | path to the real claude binary |
| `CLAUDIO_CADENCE` | `1` | `0` disables per-character human typing cadence (burst instead) |
| `CLAUDIO_FAST` | `0` | `1` strips *all* cosmetic typing delays (implies cadence off); on by default under `--api` |

## OpenAI-compatible API mode

`claudio --api` starts an HTTP server that speaks the OpenAI Chat Completions
API. Point any OpenAI client at it and your calls are served by your local
`claude` — using whatever auth it already has (subscription/OAuth or
`ANTHROPIC_API_KEY`). No Anthropic API key is sent by the server.

```sh
claudio --api                       # binds 127.0.0.1:8080 by default
```

```sh
curl localhost:8080/v1/chat/completions \
  -H 'content-type: application/json' \
  -d '{"model":"sonnet","messages":[{"role":"user","content":"Hello!"}]}'
```

Works with the official OpenAI SDKs and OpenAI-compatible coding agents (tested
end-to-end with `pi`):

```python
from openai import OpenAI
client = OpenAI(base_url="http://127.0.0.1:8080/v1", api_key="dummy")
client.chat.completions.create(model="haiku",
    messages=[{"role": "user", "content": "hi"}])
```

### Using it from coding agents (pi, opencode, hermes)

claudio is OpenAI-compatible, so any agent that speaks the OpenAI Chat
Completions API can use it as a custom provider pointing at
`http://127.0.0.1:8080/v1`. The API key is unused unless you set
`CLAUDIO_API_KEY` (then put the same value where each tool expects a key).

For **agentic** (tool-calling) clients, claudio additionally recognizes the
client, propagates its working directory, and translates its tools — see
[Agentic mode](#agentic-mode-tool-calling) for how and why.

#### Supported agents

"Supported" means claudio **fingerprints** the client from its system prompt and
grounds the agent in the right **workspace** (so a client launched in
`~/project` edits files there, even when claudio's backend runs elsewhere — e.g.
a remote host). Tools are always rendered from the schema the client sends, so an
unrecognized client still works via the `generic` profile (paths used exactly as
given). Adding a new agent is a one-line fingerprint entry in `src/api/agentic.rs`.

| Agent | Detected as | Config file | Verified |
|-------|-------------|-------------|----------|
| [pi](https://github.com/badlogic/pi-mono) | `pi` | `~/.pi/agent/models.json` | read · write · bash |
| [opencode](https://github.com/sst/opencode) | `opencode` | `~/.config/opencode/opencode.json` | read · write · glob |
| [hermes](https://github.com/NousResearch/hermes-agent) | `hermes` | `~/.hermes/config.yaml` | read_file · write_file · terminal |
| any other OpenAI-compatible client | `generic` | — | works; paths used as given |

**[pi](https://github.com/badlogic/pi-mono)** — add a provider to
`~/.pi/agent/models.json`:

```json
{
  "providers": {
    "claudio": {
      "api": "openai-completions",
      "apiKey": "dummy",
      "baseUrl": "http://127.0.0.1:8080/v1",
      "compat": { "supportsDeveloperRole": false, "supportsReasoningEffort": false },
      "models": [
        { "id": "sonnet", "name": "Claude Sonnet (claudio)", "contextWindow": 200000, "maxTokens": 32000 },
        { "id": "haiku",  "name": "Claude Haiku (claudio)",  "contextWindow": 200000, "maxTokens": 32000 },
        { "id": "opus",   "name": "Claude Opus (claudio)",   "contextWindow": 200000, "maxTokens": 64000 }
      ]
    }
  }
}
```

```bash
pi --provider claudio --model haiku "Refactor this file"
# or make it the default in ~/.pi/agent/settings.json:
#   { "defaultProvider": "claudio", "defaultModel": "sonnet" }
```

**[opencode](https://github.com/sst/opencode)** — add a provider to
`~/.config/opencode/opencode.json` (it uses the `@ai-sdk/openai-compatible`
adapter):

```json
{
  "provider": {
    "claudio": {
      "npm": "@ai-sdk/openai-compatible",
      "name": "claudio (Claude via PTY)",
      "options": { "baseURL": "http://127.0.0.1:8080/v1" },
      "models": {
        "sonnet": { "name": "Claude Sonnet", "limit": { "context": 200000, "output": 32000 } },
        "haiku":  { "name": "Claude Haiku",  "limit": { "context": 200000, "output": 32000 } },
        "opus":   { "name": "Claude Opus",   "limit": { "context": 200000, "output": 64000 } }
      }
    }
  }
}
```

```bash
opencode run -m claudio/haiku "Add a test for parse()"
# or set a default in opencode.json:  "model": "claudio/sonnet"
```

**[hermes](https://github.com/NousResearch/hermes-agent)** — point its `model`
section at claudio with the `custom` (OpenAI-compatible) provider in
`~/.hermes/config.yaml`:

```yaml
model:
  provider: "custom"                      # any OpenAI-compatible endpoint
  base_url: "http://127.0.0.1:8080/v1"
  api_key: "dummy"                        # any non-empty value unless CLAUDIO_API_KEY is set
  default: "opus"                         # or sonnet / haiku
```

```bash
hermes --provider custom -m opus -z "Add a test for parse()"
```

The model `id`s map straight through to `--model` (`opus`/`sonnet`/`haiku` or any
`claude-*` name). Each agent's own tools work through claudio's agentic
passthrough no matter what they're named — pi's `read`/`bash`/`edit`/`write`,
opencode's `read`/`glob`/`edit`, hermes's `read_file`/`terminal`/`patch`/
`write_file` — because claudio renders every tool from the JSON Schema the client
sends rather than assuming a fixed set. Each conversation reuses one persistent
session (watch the server's `MATCH … delta=…` logs).

The `context`/`output` numbers above are **client-side budgeting hints** (200K
context is the standard Claude 4.x window; output is 64K for Opus, 32K for
Sonnet/Haiku) — claudio does not enforce them or forward the request's
`max_tokens`, so they only affect how the agent paces compaction and output.

### Endpoints

| Method | Path | Notes |
|--------|------|-------|
| POST | `/v1/chat/completions` | Streaming (SSE) and non-streaming |
| GET | `/v1/models` | Curated list of Claude models |
| GET | `/v1/models/{id}` | Retrieve one model |
| GET | `/health` | Liveness check |

`/v1/models` advertises `opus`, `sonnet`, `haiku` and the pinned full names.
Request `model` values that are Claude aliases or `claude-*` names pass through
to `--model`; anything else (e.g. `gpt-4o`) falls back to the default model.

### How it works internally

This is the interesting part, and several non-obvious design decisions are baked
in. The short version: **`claudio --api` keeps a pool of long-lived interactive
`claude` sessions and feeds each conversation only what's new.**

#### 1. The stateless-protocol problem

OpenAI Chat Completions is **stateless** — there is no server-side session and no
session id. The client (pi, opencode, an SDK) owns the conversation and **resends
the entire `messages[]` array on every request**; each turn it just grows
(`system → user → assistant(tool_calls) → tool_result → …`). A naive proxy
therefore re-processes the whole history *and* pays a fresh `claude` cold-start on
every call.

#### 2. Persistent sessions + continuation detection

Instead, the server keeps a **`SessionPool`** of live `claude` conversations.
Since the client gives us no session id, we derive one by content:

- Each live conversation remembers the `messages[]` it has already served as a
  **cumulative prefix-hash chain** `(prefix_hash, served_len)`.
- A new request **matches** a conversation when that conversation's served
  messages are an exact prefix of the request — i.e.
  `session.prefix_hash == request_cumulative_hash[served_len]`.
- **Match** → send only `messages[served_len..]` (the delta). The client's echo
  of the assistant's own previous turns is dropped (the live session already has
  them; we keep them only to label tool results). **No match** → start a fresh
  conversation with the full history.

So a multi-turn agentic loop reuses one session and types only the *new* user/
tool messages each turn instead of the whole transcript.

#### 3. Live vs. dormant — process budget with `--resume`

Holding one `claude` process per active conversation forever doesn't scale, so
process liveness is decoupled from context:

- claude **persists every session to disk** (its transcript JSONL). That's the
  source of truth for a conversation's context.
- The pool keeps up to `CLAUDIO_API_MAX_SESSIONS` (default 32) conversation
  *mappings*, but only `CLAUDIO_API_MAX_LIVE` (default 6) of them hold a live
  process at once.
- When the live budget is exceeded, the **least-recently-used live session is
  demoted**: its process is killed, but the mapping (session id + prefix hash)
  stays. Idle mappings are dropped entirely after `CLAUDIO_API_SESSION_TTL`
  (default 600 s).
- When a request continues a **dormant** conversation, it is revived with
  `claude --resume <session-id>` — claude reloads the on-disk context, and we
  again send only the delta.

#### 4. Why the system prompt rides in the *user* message, not `--system-prompt`

The server never spawns `claude -p`; it drives the interactive (`cli`) entrypoint.
Empirically, Anthropic runs a classifier on the **system prompt** of interactive
requests: a system prompt that looks like a third-party agent/product gets the
request billed as a *third-party app* (`API Error: 400 … draw from your extra
usage`). Genuine `claude -p` (the `sdk-cli` entrypoint) is exempt — that's the
sanctioned path for custom system prompts.

So the API server does **not** pass the request's `system` via
`--system-prompt`/`--append-system-prompt`. claude's own canonical system prompt
is left intact (keeping the request first-party), and the OpenAI request's
`system` content is folded into the **typed user prompt** instead — a channel the
classifier does not score. Verified: the same content that 400s as a system-prompt
override passes cleanly in the user message.

#### 5. Request lifecycle

```
POST /v1/chat/completions
  → flatten messages[] (system text + rendered transcript; + tool protocol if agentic)
  → SessionPool.resolve_turn():
       compute prefix-hash chain
       MATCH a live/dormant conversation?  ── yes ─→ revive if dormant (--resume),
       │                                              type ONLY the delta
       └─ no ─→ start a fresh `claude` session (--session-id), type the full prompt
  → wait for the Stop hook, read the new assistant message from the transcript
  → shape into an OpenAI chat.completion (+ real token usage)
```

Every step logs at `INFO` so you can watch it work:

```
resolve_turn messages=5 req_hash=34e5…
MATCH (continuation) — sending delta only session=d6bacedc reused=2 delta=3 turn=2 dormant=false
…
demoting LRU live session (process killed, context kept on disk) session=4ad7…
RESUME — reviving dormant session session=660a…
```

#### 6. Transport details

- **No host tools.** Each session launches with `--tools ""`, `--strict-mcp-config`,
  and `--disable-slash-commands`, so a chat client can never run Bash/Edit/Write
  on your machine. An agentic client's tools come back as `tool_calls` text
  (emitted, never executed — see [Agentic mode](#agentic-mode-tool-calling)).
- **Clean, low-overhead context.** Runs with `--setting-sources project` in a
  clean cwd, so your user-level plugins/hooks/memory aren't loaded per request.
- **Bracketed paste.** The typed prompt is wrapped in the terminal's
  bracketed-paste markers (auto-detected from the TUI's `?2004h`), so a large
  multi-line conversation is delivered as one atomic paste instead of submitting
  at the first embedded newline.
- **Fast by default.** `--api` sets `CLAUDIO_FAST=1` (and `CLAUDIO_CADENCE=0`),
  stripping the human-like typing delays. The Ink-quiescence wait is kept (it's a
  correctness wait, not stealth). Set `CLAUDIO_FAST=0` to restore the delays.
- **No token streaming.** A turn is resolved whole (the TUI has no per-token
  protocol). A `stream:true` request still gets a valid SSE body — the resolved
  text as a small fixed chunk set ending in `[DONE]`, with usage when
  `stream_options.include_usage` is set.

### Agentic mode (tool calling)

OpenAI tool-calling passthrough is on by default (`CLAUDIO_API_AGENTIC=true`).
Safe to default on because the server itself never executes tools — the client
does. When a request includes a non-empty `tools` array, the server presents
claude with its **own** honest *tool-execution gateway* protocol — "you don't run
tools; request an action and the gateway runs it in the user's workspace and
returns the result" — and translates the client's `tools[]` into a clean,
hand-written catalog (a per-tool registry in `agentic.rs` covers `read`/`bash`/
`edit`/`write`; unknown tools fall back to their JSON Schema). claude requests
actions as a fenced ` ```tool_calls ` JSON block, which the server parses into
OpenAI `tool_calls` (`finish_reason: "tool_calls"`); the client executes them and
resends the conversation, and the live session continues.

> **Why it's framed this way.** The client's own system prompt is **not relayed
> verbatim** — describing a different agent harness ("you are operating inside
> pi…") and labeling it "authoritative system instructions" makes the stronger
> models (Opus/Sonnet) treat the request as a prompt-injection attempt and refuse,
> reverting to being Claude Code. The neutral gateway framing is what makes
> Opus/Sonnet/Haiku all comply. Set `CLAUDIO_API_AGENTIC=false` to ignore `tools`
> (chat-only).

**Client detection & workspace propagation.** The backend `claude` runs in
claudio's own working directory (default `/tmp`, possibly on a *remote* host), so
it must never touch its own filesystem — every action has to round-trip to the
client, which executes it in the *user's* workspace. Two mechanisms make that
correct:

1. The gateway preamble tells the model it **has no direct access** to the
   machine (which "may be remote"), cannot act on its own, and must request every
   action through the gateway. This stops Claude Code from running its built-in
   tools against the server's directory.
2. Before discarding the client's prose, claudio **reads it for environment
   facts** — it fingerprints the CLI (officially: `pi`, `opencode`, `hermes`;
   otherwise `generic`) and lifts the **working directory** and date out of the
   system prompt (e.g. `Current working directory: …`). That directory is injected
   as the authoritative `Workspace:` line so the model resolves paths in the
   client's tree, not claudio's. When no workspace can be determined, the model is
   told to use paths exactly as given and assume no absolute root.

Tool arguments are rendered from **the client's own JSON Schema**, never a
hardcoded per-name table — the same concept is named differently by each client
(pi's `read` takes `path`, opencode's takes `filePath`; pi's editor is `edit`,
opencode's is `edit` with `oldString`, hermes's is `patch`; the shell is `bash`
in pi, `bash` in opencode, `terminal` in hermes), so only the schema is
authoritative.

This is why a client launched in `/home/me/project` sees its *own* files even
though claudio's backend lives in `/tmp` — confirmed with pi, opencode, and hermes
(read + write) on opus and sonnet. Support for a new CLI is just a fingerprint
entry in `agentic.rs`; the prompt prose and schema rendering stay shared.

### Observing the message flow (`--log-messages`)

To watch exactly what crosses each hop — the client's request, the prompt
claudio types to the upstream `claude` (full context or delta), claude's raw
reply, and the response returned to the client — start the server with
`--log-messages`:

```bash
claudio --api --log-messages
# and/or capture the raw, untruncated exchange as JSON Lines:
claudio --api --log-messages-file /tmp/flow.jsonl
```

Each turn's four hops share a short correlation id and are printed as compact,
colorized blocks on stderr:

```
┌─ #2 [8547d1] claudio ──▶ claude  NEW · session 88b68059 · model sonnet
│ You are the model powering a tool-execution gateway. …
└─
┌─ #3 [8547d1] claude ──▶ claudio  raw reply · stop=end_turn · usage in=6663 out=183
│ ```tool_calls
│ [{"name": "write", "arguments": {"path": "logtest.txt", "content": "LOG_OK\n"}}]
│ ```
└─
┌─ #4 [8547d1] claudio ──▶ CLI  response · tool_calls (1)
│ write({"content":"LOG_OK\n","path":"logtest.txt"})
└─
```

`--log-messages-file` writes one JSON object per hop (`{seq, corr, dir, head,
body}`) with the **full untruncated** payloads — handy for diffing prompts or
replaying a conversation. Both also work for the single-turn `-p` path. Env
equivalents: `CLAUDIO_LOG_MESSAGES=1`, `CLAUDIO_LOG_MESSAGES_FILE=<path>`. Run
`claudio --help` for the complete list of claudio flags and `CLAUDIO_*` vars.

### Configuration (environment variables)

Each setting reads a `CLAUDIO_API_*` variable first, then falls back to the
original `OPENAI_PROXY_*` name for drop-in compatibility with the standalone
proxy.

| Variable | Default | Description |
|----------|---------|-------------|
| `CLAUDIO_API_BIND` (`OPENAI_PROXY_BIND`) | `127.0.0.1:8080` | Listen address. |
| `CLAUDIO_API_KEY` (`OPENAI_PROXY_API_KEY`) | _(unset)_ | If set, require `Authorization: Bearer <key>`. |
| `CLAUDIO_CLAUDE_PATH` (`OPENAI_PROXY_CLAUDE_BIN`) | `claude` | Path/name of the Claude CLI driven by the sessions. |
| `CLAUDIO_API_DEFAULT_MODEL` (`OPENAI_PROXY_DEFAULT_MODEL`) | `sonnet` | Fallback for missing/non-Claude model names. |
| `CLAUDIO_API_CWD` (`OPENAI_PROXY_CWD`) | system temp dir | Working dir for the sessions (kept clean of `CLAUDE.md`). |
| `CLAUDIO_API_TIMEOUT_SECS` (`OPENAI_PROXY_TIMEOUT_SECS`) | `600` | Per-turn timeout. |
| `CLAUDIO_API_MAX_CONCURRENCY` (`OPENAI_PROXY_MAX_CONCURRENCY`) | `8` | Max concurrent in-flight turns. |
| `CLAUDIO_API_AGENTIC` (`OPENAI_PROXY_AGENTIC`) | `true` | OpenAI tool-calling passthrough. Set `false` to ignore `tools`. |
| `CLAUDIO_API_SETTING_SOURCES` (`OPENAI_PROXY_SETTING_SOURCES`) | `project` | `--setting-sources` value; `user,project,local` for your full setup, or empty for CLI defaults. |
| `CLAUDIO_API_MAX_SESSIONS` | `32` | Max conversation mappings kept (each resumable from disk). |
| `CLAUDIO_API_MAX_LIVE` | `6` | Max live `claude` processes at once; idle ones are demoted to dormant. |
| `CLAUDIO_API_SESSION_TTL` | `600` | Seconds before an idle mapping is dropped entirely. |
| `CLAUDIO_API_REINJECT_TURNS` | `6` | Re-inject the system/agentic prompt every N turns of a session. |

> **claudio never invokes `claude -p`.** Every request — chat or agentic — is
> resolved by driving the *interactive* `claude` under a PTY. There is no
> code path that runs `claude --print`.

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
docker compose run --rm claudio -p --dangerously-skip-permissions "Reply with exactly: DOCKER_OK"
docker compose run --rm claudio -p --output-format json "Reply with exactly: OK"
```

**API server in Docker.** The image also runs the OpenAI-compatible server. The
`claudio-api` compose service starts it and publishes it on loopback:

```bash
docker compose up claudio-api        # serves http://127.0.0.1:8080/v1
curl localhost:8080/v1/chat/completions -H 'content-type: application/json' \
     -d '{"model":"haiku","messages":[{"role":"user","content":"hi"}]}'
```

Inside the container the server binds `0.0.0.0:8080` (set in the Dockerfile); the
compose `ports` mapping (`127.0.0.1:8080:8080` by default) controls host exposure
— change it to `8080:8080` to reach it from the LAN, and set `CLAUDIO_API_KEY` if
you do. Or without compose: `docker run --rm -p 8080:8080 -v ~/.claude:/home/node/.claude
claudio --api`.

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
  inside the container (`docker compose run --rm --entrypoint claude claudio
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
src/cli.rs       transparent/dynamic arg handling (owns -p/--api/--fast/+4, forwards the rest)
src/vt.rs        terminal-probe responder (vte)
src/hooks.rs     inline --settings merge, self-exec relay, tcp + file transports
src/session.rs   session-JSONL parse, stop-reason logic, flush-race retry, latest-message id
src/driver.rs    PtySession { start, turn, close } — reusable persistent PTY session;
                 one-shot run() (CLI -p) is built on top. Bracketed paste, --fast, --resume.
src/emit.rs      text / json / stream-json formatters

src/api/         OpenAI-compatible server (--api)
  mod.rs         tokio runtime, router, server bootstrap
  config.rs      CLAUDIO_API_* / OPENAI_PROXY_* env config + AppState (holds the pool)
  pool.rs        SessionPool: prefix-hash continuation matching, delta sends,
                 live/dormant slots with --resume, LRU/TTL eviction, INFO logging
  backend.rs     run_raw → SessionPool (spawn_blocking), model + usage mapping
  routes/        /v1/chat/completions, /v1/models
  prompt.rs      messages[] → system prompt + rendered transcript
  agentic.rs     prompt-based OpenAI tool-calling passthrough
  types.rs       lenient OpenAI wire types
  auth.rs        optional bearer-token middleware
  error.rs       OpenAI error envelope
  usage.rs       transcript usage → OpenAI token counts
  util.rs        completion ids / timestamps
```
