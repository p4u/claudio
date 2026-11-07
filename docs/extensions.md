# Extensions

An extension adds to what claudio shows and does. It has **lenses** (things
claudio renders from claude's answers) and **actions** (keys under the leader,
`Alt+z`). Four extensions are built in: `latex`, `diagram`, `search` and
`snippets`. You can add your own as a small program in any language.

- [Using extensions](#using-extensions)
- [Writing an external extension](#writing-an-external-extension)
- [Protocol v1](#protocol-v1)
- [Limits and safety](#limits-and-safety)

## Using extensions

| Extension | Lens | Keys | What it does |
|---|---|---|---|
| `latex` | tag `<latex>` | — | Math typeset in the pane (one line) and the reader (2D). |
| `diagram` | fence ```` ```mermaid ```` | — | Mermaid flowcharts and sequence diagrams drawn with box characters in the reader. |
| `search` | — | `Alt+z s` | Search the claude conversations on the active session's host; Enter resumes one. |
| `snippets` | — | `Alt+z p`, `Alt+z P` | Paste a saved prompt; save the last prompt as one. |

`Alt+z` opens the leader popup: one line per action. Press the action's key to
run it; `Esc` or any other key closes the popup and nothing reaches claude.
`Alt+z u` opens the reader view (also `Alt+u`), and `Alt+z e` the extensions
screen (also `Alt+i`).

### The extensions screen (`Alt+i`)

```
[x] latex             built-in  —          <latex>
[ ] diagram           built-in  —          ```mermaid
[x] search            built-in  Alt+z s    —
[x] snippets          built-in  Alt+z p/P  —
[x] keys              external  Alt+z k    <keys>
[x] vocdoni-internal  plugin    —          —
```

One row per extension: the built-ins, then the external ones that loaded
(one that failed to load is not listed; its notice at start said why), then
the plugins of your proxy profiles (see below).
Below the list, the selected extension's description, its actions and its
lenses. `↑`/`↓`, `PgUp`/`PgDn`, `Home`/`End` and the mouse wheel move;
`Space` turns the selected extension on or off, and so does `Enter` except
on a plugin's row, where it opens the plugin's setup screen; `r` fetches the
plugin lists and manifests again; `Esc` or `Alt+i` closes the screen.

A change applies at once: the extension's keys leave the `Alt+z` popup, its
tag lenses stop rendering in the pane (at the next draw) and in the reader,
and sessions started from then on carry only the lenses that are on.
Sessions already running keep the instructions they started with; `Alt+e`
restarts one with the new ones. Turning an external extension off stops its
program; turning it on starts it again on the next request. An extension
disabled after 3 failures shows `(disabled after failures)` and stays
disabled until claudio restarts.

The choice is saved in `config.toml` as `name = true|false` under
`[extensions]`: the line is replaced, or added to that table, or the table
is appended. Comments, other sections and the file's mode stay as they
were. If `config.toml` is not valid TOML, nothing is written and the status
bar says so (the change still applies until claudio exits). With
`[claude] latex = false`, the latex row says `also off by [claude] latex =
false`: that setting turns latex off again at the next start.

Each lens comes with an instruction that tells claude how to write for it. It
reaches claude through a `SessionStart` hook that reads a private file of the
session daemon. The text never goes into claude's command line. Turn any
extension off in `~/.config/claudio/config.toml`:

```toml
[extensions]
diagram = false        # by name, built-in or external; every extension is on by default
```

`[claude] latex = false` still turns math off. On the client it turns the
`latex` extension off; on a host it keeps the latex instruction out of that
host's sessions. On a host whose daemon has `[claude] latex = false`, a new
client overlays no `<latex>` tags for that host's sessions (the daemon extracts
none); v0.5.0 clients still get them through `Latex`.

### Proxy plugins

A claude-proxy profile can serve plugins (`GET /v1/claudio/plugins`): for
each, a pinned manifest (a TOML file, by URL and SHA-256) and the env its MCP
servers need (their secrets). claudio fetches the list of every profile in
use at start, every 15 minutes, after `claudio proxy login` and on `r` in
the extensions screen, and caches it in `~/.config/claudio/plugins.json`
(mode 0600: it holds the secrets). It fetches each manifest itself (https,
256 KiB at most, hash-checked, kept in memory).

A manifest declares marketplaces and the plugins to install from them, MCP
servers (http, sse or stdio, with the names of the secrets each needs),
subagents, instructions for every session's context, and env:

```toml
format = 1
name = "vocdoni-internal"
version = "1.0.0"
description = "Vocdoni team setup"
instructions = """Search the vocdoni-kb server before asking a colleague."""

[[marketplace]]
name = "vocdoni"                       # the name its .claude-plugin/marketplace.json declares
source = "github:vocdoni/skills"       # github:<owner>/<repo> or an https URL

[[plugin]]
name = "agentic-dev"
marketplace = "vocdoni"

[[mcp]]
name = "vocdoni-kb"
type = "http"
url = "https://kb-gw.vocdoni.net/mcp"
headers = { Authorization = "Bearer ${VOCDONI_KB_TOKEN}" }
secrets = ["VOCDONI_KB_TOKEN"]         # served by the proxy as env; never on argv

[[agent]]
name = "reviewer"
description = "Read-only code reviewer"
tools = ["Read", "Grep", "Glob"]
prompt = """You are a code reviewer…"""
```

The first time claudio sees a manifest (no `[plugins.<name>]` table in
`config.toml`), its setup screen lists every item by section, all on:
`↑`/`↓` (or `j`/`k`) move, `Space` toggles, `a` turns all on, `n` all off,
`Enter` saves the choice and applies it, `Esc` puts it off until the next
start (nothing is applied meanwhile). `Enter` on the plugin's row of the
extensions screen opens it again. The choice is saved as

```toml
[plugins.vocdoni-internal]
disabled = ["mcp:jina-mcp-server"]
reviewed = "1.0.0"
seen = ["plugin:agentic-dev@vocdoni", "mcp:vocdoni-kb", "mcp:jina-mcp-server", "agent:reviewer", "instructions"]
```

(the table is replaced or appended; the rest of the file stays as it was).
A newer version's items that are not in `seen` are on, and a notice names
them.

Each host's daemon applies the chosen items: it adds a marketplace Claude
Code does not know (`claude plugin marketplace add --scope user`) and
installs the plugins in your user scope (`claude plugin install --scope
user`), in the background, one at a time, never with `-y`: a plugin whose
marketplace declares a command to run is `needs manual install` with the
command to type. A failed install is tried again after 15 seconds, then
after twice as long each time it fails again, up to every 5 minutes (its
reason says `retrying in N s`), and at once when claude has just been
installed on that host. What you already have is left alone: an installed
plugin is `present`, an MCP server in your `~/.claude.json` or an agent in
`~/.claude/agents/` of the same name is skipped as `yours`. A server whose
secret the proxy does not serve is skipped too.

Every session started with that profile then gets, per plugin that is on,
`--agents` and `--mcp-config` (the server definitions with `${NAME}`
placeholders; the secrets are only in the session's env), the manifest's
env and its instructions (through the same context hook as the lenses'). A
new session waits up to 20 s for a host that has not answered yet, then
starts without the plugin; installs still under way do not hold it. A
claude without `--agents` or `--mcp-config` starts without them (the
details say `claude too old`). At most 8 plugins go on one session.

A plugin's details say, per connected host, `ready · <i> installed, <p>
present, <s> installing, <k> skipped` with a line per skipped item and its
reason, `failed: …` or `daemon too old`; the manifest's state when it could
not be used (`manifest: <reason>`); `setup pending (Enter)` or `<n> items
disabled (Enter to change)`; and `proxy unreachable, cached <age>` when the
proxy did not answer. A plugin claudio refuses (a bad name, URL or hash, an
env key it may not set such as `PATH`, `LD_*` or `ANTHROPIC_*`, a name
another extension has) is listed as `refused: <reason>` and never used.
Turning a plugin off is saved as `name = false` under `[extensions]` and
applies to new sessions; nothing is uninstalled (`claude plugin uninstall
<id>` does that). `Alt+e` keeps a session's flags and gives it the current
env and instructions.

Snippets are files in `~/.config/claudio/snippets/` (`*.md` or `*.txt`, the
file name without its extension is the snippet's name). A paste never presses
Enter: the text lands in claude's prompt for you to edit or send. It needs
claude's prompt to accept a bracketed paste; otherwise claudio says so and
pastes nothing.

## Writing an external extension

An external extension is a directory under `~/.config/claudio/extensions/`
(or `$XDG_CONFIG_HOME/claudio/extensions/`) with a manifest and a program:

```
~/.config/claudio/extensions/keys/
├── extension.toml
└── keys.py
```

```toml
name = "keys"                      # must equal the directory name: ^[a-z][a-z0-9-]{0,31}$
description = "keyboard keys"      # at most 120 characters
command = ["python3", "keys.py"]   # argv, no shell; a first element with a '/' is
                                   # relative to this directory, otherwise found in PATH

[[lens]]
tag = "keys"                       # or: fence = "abc"  (exactly one of tag and fence)
instruction = "Write keyboard keys as <keys>Ctrl+C</keys>."

[[action]]
key = "k"                          # one character [A-Za-z0-9], unique among all actions
title = "insert a timestamp"       # at most 60 characters
id = "stamp"                       # ^[a-z][a-z0-9_-]{0,31}$, passed back to the program
input = "label"                    # optional: ask for a line of input first
```

- A **tag lens** renders `<tag>…</tag>` in the pane (on one line, nothing moves)
  and in the reader. A **fence lens** renders a fenced code block whose info
  string starts with its name, in the reader only.
- A tag that is an HTML element name (`kbd`, `code`, `sub`, …) is refused,
  because claude and markdown use those as HTML. Add `html = true` to the lens
  to take the name anyway.
- An extension whose name, tag or fence is already taken is skipped. An action
  whose key is taken is skipped on its own; `u` (the reader) and `e` (the
  extensions screen) are always taken. At most 16 external extensions are
  loaded. Each skip shows one notice at start.
- Manifests are read when claudio starts. The program starts on the first
  request and keeps running until claudio exits or the extension is turned
  off.

[`examples/extensions/keys/`](../examples/extensions/keys/) is a complete
extension in Python: a `keys` tag lens and one action.

## Protocol v1

claudio and the program exchange UTF-8 JSON objects, one per line, on the
program's stdin and stdout. The program's stderr is discarded. Request ids are
numbers chosen by claudio, and answers may come in any order.

The program runs in its extension directory, in its own process group, with
claudio's environment minus Claude Code's session variables,
`ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN` and every `CLAUDIO_*` variable,
plus `CLAUDIO_EXTENSION=<name>` and `CLAUDIO_EXTENSION_PROTOCOL=1`.

### render

```
→ {"id":1,"method":"render","params":{"kind":"tag","lens":"keys","source":"Ctrl+C"}}
← {"id":1,"result":{"inline":"⌃C","block":["┌────┐","│ ⌃C │","└────┘"]}}
```

`inline` is one line: it replaces the tag in the pane and in a sentence of the
reader. `block` is a list of rows: the reader shows it for a tag alone in its
paragraph and for a fence lens. Both are optional. A missing or failed
rendering leaves the source as claude wrote it.

### action

```
→ {"id":2,"method":"action","params":{"action":"stamp","input":"x",
     "session":{"conversation":"<uuid>","cwd":"/home/u/p","host":"local","name":null}}}
← {"id":2,"result":{"kind":"paste","text":"2026-11-05 16:40"}}
```

`session` is the active session, or `null` when there is none. `input` is the
line the user typed, or `null` for an action without `input`. `result.kind` is
one of:

| kind | fields | claudio |
|---|---|---|
| `document` | `title`, `markdown` | shows the markdown like the reader, with its lenses rendered |
| `notice` | `text` | shows it in the status bar |
| `paste` | `text` | pastes it into the active claude session, without Enter |
| `list` | `title`, `items: [{label, detail?, result}]` | lets the user pick; Enter applies the item's `result` (not a `list`) |

### errors

Any request may fail: `{"id":3,"error":"message"}`. A render error leaves the
source as written; an action error is shown as a notice.

## Limits and safety

Everything a program returns is treated as untrusted:

- Every string is stripped of control and format characters. Inline text,
  labels, details, notices and titles also lose newlines and tabs.
- Caps: inline ≤ 512 columns; block ≤ 200 columns × 100 rows; document
  markdown ≤ 256 KiB; list ≤ 500 items, label ≤ 200 and detail ≤ 400
  characters; paste ≤ 64 KiB; notice ≤ 200 and title ≤ 120 characters. A
  rendering over a cap is dropped. Markdown or paste text over its cap is
  dropped with the notice `result too large`. Lists keep their first 500
  items, and long labels, details, notices and titles are cut.
- Timeouts: a render has 1 s, an action 15 s. A request that cannot be
  written to the program within its timeout counts as a timeout too, and the
  program is stopped. At most 16 requests are in flight per extension; failed
  renders are not asked again for 60 s.
- A line that is not JSON, or one over 1 MiB, is a protocol error: the
  program is stopped and started again on the next request. After 3 failures
  (crash, protocol error, timeout) within 5 minutes the extension is disabled
  until claudio restarts, with a notice.
- A session's context carries at most 16 lenses and 8192 bytes of lens
  instructions; the lenses that do not fit are left out, and claudio says so
  with a notice at start.
- On exit claudio sends SIGTERM to the program's process group, then SIGKILL
  after a second.

External extensions run on the machine where you run `claudio`, never on an
SSH host. They run as you and could read your `config.toml`, so trust them as
you trust that file.
