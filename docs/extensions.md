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
[x] latex     built-in  —          <latex>
[ ] diagram   built-in  —          ```mermaid
[x] search    built-in  Alt+z s    —
[x] snippets  built-in  Alt+z p/P  —
[x] keys      external  Alt+z k    <keys>
```

One row per extension: the built-ins, then the external ones that loaded
(one that failed to load is not listed; its notice at start said why).
Below the list, the selected extension's description, its actions and its
lenses. `↑`/`↓`, `PgUp`/`PgDn`, `Home`/`End` and the mouse wheel move;
`Space` or `Enter` turns the selected extension on or off; `Esc` or `Alt+i`
closes the screen.

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
