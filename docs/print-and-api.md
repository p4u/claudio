# claudio: print mode, API server, and proxy

This document covers `claudio -p` (drop-in for `claude -p`), the `--api` OpenAI-compatible server, and the `claude-proxy` integration.

> **Background.** `claudio` demonstrates that client-side restrictions on programmatic Claude Code use cannot be enforced: every signal that would distinguish "human" from "automation" lives on the user's machine, and the model requests produced are identical either way. It uses only your own authenticated session and documented, supported Claude Code features. It does **not** bypass authentication or billing — every request counts against your account.

---

## Drop-in `-p` (print mode)

The wrapper proxies the real `claude` (found on `PATH`, or via `$CLAUDIO_CLAUDE_PATH`):

- **Without `-p`/`--print`** it `exec`s the real `claude` unchanged — interactive sessions, subcommands (`auth`, `mcp`, …), `--help`, and `--version` are 100% native.
- **With `-p`/`--print`** it emulates print mode: it forwards every other flag verbatim to interactive claude, types your prompt into the TUI, waits for the turn to finish via the `Stop` lifecycle hook, and reads the exact answer (and real token usage) from the canonical session JSONL.
- **With `--api`** it starts an OpenAI-compatible HTTP server (see [OpenAI-compatible API mode](#openai-compatible-api-mode)).

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
```

### Flags

The command-line surface **is claude's** — every flag is forwarded to claude except the few the wrapper must own:

| Owned flag | Handling |
|------------|----------|
| `-p`, `--print` | triggers print-mode emulation (not forwarded) |
| `--api` | starts the OpenAI-compatible server (not forwarded) |
| `--fast` | strip human-like typing delays for lowest latency (not forwarded) |
| `--output-format <text\|json\|stream-json>` | wrapper formats the output |
| `--input-format` | only `text` supported by this backend |
| `--settings <file-or-json>` | merged with the wrapper's hooks, then forwarded |
| `--session-id <uuid>` | captured (to locate the transcript) and forwarded |

Two claude flags are dropped (with a warning): `--bare` (skips hooks) and `--no-session-persistence` (suppresses the session JSONL).

### Wrapper-only controls (environment variables)

| Variable | Default | Meaning |
|----------|---------|---------|
| `CLAUDIO_DEBUG` | `0` | trace timeline to stderr |
| `CLAUDIO_TIMEOUT_SEC` | `300` | wall-clock cap |
| `CLAUDIO_HOOK_TRANSPORT` | `tcp` | `tcp` or `file` (sandboxes blocking loopback) |
| `CLAUDIO_RAW_LOG` | — | dump the raw PTY byte stream to this path |
| `CLAUDIO_COLS` / `CLAUDIO_ROWS` | `120`/`40` | PTY size |
| `CLAUDIO_CLAUDE_PATH` | `claude` | path to the real claude binary |
| `CLAUDIO_CADENCE` | `1` | `0` disables per-character human typing cadence (burst instead) |
| `CLAUDIO_FAST` | `0` | `1` strips *all* cosmetic typing delays; on by default under `--api` |
| `CLAUDIO_PROXY_URL` | — | ephemeral proxy profile: `<token>@<host>`; overrides any saved default profile |

---

## OpenAI-compatible API mode

`claudio --api` starts an HTTP server that speaks the OpenAI Chat Completions API. Point any OpenAI client at it and your calls are served by your local `claude` — using whatever auth it already has.

```sh
claudio --api                       # binds 127.0.0.1:8080 by default
```

```sh
curl localhost:8080/v1/chat/completions \
  -H 'content-type: application/json' \
  -d '{"model":"sonnet","messages":[{"role":"user","content":"Hello!"}]}'
```

Works with the official OpenAI SDKs and OpenAI-compatible coding agents:

```python
from openai import OpenAI
client = OpenAI(base_url="http://127.0.0.1:8080/v1", api_key="dummy")
client.chat.completions.create(model="haiku",
    messages=[{"role": "user", "content": "hi"}])
```

### Using it from coding agents (pi, opencode, hermes)

| Agent | Detected as | Config file | Verified |
|-------|-------------|-------------|----------|
| [pi](https://github.com/badlogic/pi-mono) | `pi` | `~/.pi/agent/models.json` | read · write · bash |
| [opencode](https://github.com/sst/opencode) | `opencode` | `~/.config/opencode/opencode.json` | read · write · glob |
| [hermes](https://github.com/NousResearch/hermes-agent) | `hermes` | `~/.hermes/config.yaml` | read_file · write_file · terminal |
| any other OpenAI-compatible client | `generic` | — | works; paths used as given |

**[pi](https://github.com/badlogic/pi-mono)** — add a provider to `~/.pi/agent/models.json`:

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

**[opencode](https://github.com/sst/opencode)** — add a provider to `~/.config/opencode/opencode.json`:

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

**[hermes](https://github.com/NousResearch/hermes-agent)** — point its `model` section at claudio:

```yaml
model:
  provider: "custom"
  base_url: "http://127.0.0.1:8080/v1"
  api_key: "dummy"
  default: "opus"
```

### Endpoints

| Method | Path | Notes |
|--------|------|-------|
| POST | `/v1/chat/completions` | Streaming (SSE) and non-streaming |
| GET | `/v1/models` | Curated list of Claude models |
| GET | `/v1/models/{id}` | Retrieve one model |
| GET | `/health` | Liveness check |

### Configuration (environment variables)

| Variable | Default | Description |
|----------|---------|-------------|
| `CLAUDIO_API_BIND` | `127.0.0.1:8080` | Listen address |
| `CLAUDIO_API_KEY` | _(unset)_ | If set, require `Authorization: Bearer <key>` |
| `CLAUDIO_CLAUDE_PATH` | `claude` | Path to the Claude CLI |
| `CLAUDIO_API_DEFAULT_MODEL` | `sonnet` | Fallback for missing/non-Claude model names |
| `CLAUDIO_API_CWD` | system temp dir | Working dir for sessions |
| `CLAUDIO_API_TIMEOUT_SECS` | `600` | Per-turn timeout |
| `CLAUDIO_API_MAX_CONCURRENCY` | `8` | Max concurrent in-flight turns |
| `CLAUDIO_API_AGENTIC` | `true` | OpenAI tool-calling passthrough |
| `CLAUDIO_API_MAX_SESSIONS` | `32` | Max conversation mappings kept |
| `CLAUDIO_API_MAX_LIVE` | `6` | Max live `claude` processes at once |
| `CLAUDIO_API_SESSION_TTL` | `600` | Seconds before an idle mapping is dropped |
| `CLAUDIO_API_REINJECT_TURNS` | `6` | Re-inject system prompt every N turns |

### Run in Docker

```bash
docker compose build
docker compose run --rm claudio -p --dangerously-skip-permissions "Reply with exactly: DOCKER_OK"
docker compose up claudio-api        # serves http://127.0.0.1:8080/v1
```

---

## claude-proxy integration

claudio integrates with **[claude-proxy](https://github.com/p4u/claude-proxy)**, an optional self-hosted gateway that lets remote sessions authenticate without a local `claude` login.

**Login:**

```bash
claudio proxy login [URL]
```

The URL is prompted if omitted. The token is **always read interactively** (no echo) or piped on stdin.

**Ephemeral profile (no saved file):**

```
CLAUDIO_PROXY_URL=<token>@<host>
```

where `<host>` may be `proxy.example.com`, `https://proxy.example.com`, or `host:port`. `http://` is only allowed for `127.0.0.1` / `localhost`.

**Other proxy commands:**

```bash
claudio proxy status              # show profiles and live stats
claudio proxy use NAME|none       # set or clear the default profile
claudio proxy logout [NAME]       # remove a saved profile
```

When a session is spawned with a proxy profile the daemon injects `ANTHROPIC_BASE_URL`, `ANTHROPIC_AUTH_TOKEN`, and several `CLAUDE_CODE_*` / `ANTHROPIC_DEFAULT_*_MODEL` variables. Secrets are passed through the daemon's environment plumbing and are never written to any state file.
