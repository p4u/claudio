# Extensions: design

Status: audited (rev 2: spec audit F1–F13 folded in)   Class: architectural
Owner decisions so far (2026-11-05):
- Turn the LaTeX feature into one instance of a general mechanism ("plugins"): an add-on
  declares tags, gets its instruction injected through the SessionStart hook, is called when
  its tag shows up, and returns what the user sees; an add-on may also bind a key under a
  leader that runs something or shows a screen.
- Shape: **built-in + external**. Built-ins are Rust code; external ones are separate programs
  claudio talks to over JSON lines on stdio. claudio sanitizes, times out, caps and draws
  everything.
- Leader key **Alt+z** (Alt+p belongs to Claude Code).
- First batch, after migrating latex and the reader onto the mechanism: **search**,
  **diagram**, **snippets**.
- 16:22: "design and implement the extension framework and the selected plugins … auto
  approve the plans and execute all until the end" (the controller approves spec and plan in
  the owner's place; push and release are authorized for this work).

Controller decisions (recorded for the owner):
- The name is **extensions**. An extension has **lenses** (things claudio renders) and
  **actions** (keys under the leader).
- Two kinds of lens: a **tag lens** (`<name>…</name>` inline in claude's text: latex) and a
  **fence lens** (a fenced code block whose info string is the lens name: ` ```mermaid `).
  Tag lenses render in the pane (one line) and in the reader; fence lenses render in the
  reader only.
- **diagram** is a fence lens on ` ```mermaid ` blocks (flowcharts and sequence diagrams),
  not a `<diagram>` tag: claude already writes Mermaid in fences, Claude Code shows a fence
  line by line, so the pane stays readable, and the reader draws it with box characters.
- **search** runs on the daemon of the session's host and resumes the chosen conversation.
- **snippets** are files in `~/.config/claudio/snippets/`, pasted into the session (not sent).

## Problem

claudio's one add-on, LaTeX, is wired through every layer by name: the daemon's
`[claude] latex` flag adds a constant SessionStart hook (`src/claude/hooks.rs:60-71`,
`LATEX_CONTEXT` at `:46`), the transcript cursor extracts only `<latex>`
(`src/claude/projects.rs:327,362`), the daemon sends `SessionEvent::Latex`
(`src/proto.rs:559`, `src/daemon/session.rs:605`), the client renders it on the
`latex-render` worker (`src/tui/mod.rs:519-640`), overlays `<latex>` only
(`src/tui/latex.rs:307-320`) and the reader replaces only `<latex>` tags
(`src/tui/reader/doc.rs:93`). Adding a second renderer, or letting the user add one, means
touching all of them again. There is also no place for user-triggered tools: every manager
key is a hard-coded `Alt+<letter>` in `src/tui/keymap.rs:97`, and few free Alt letters remain.

## Goals

1. **One mechanism.** A registry of extensions on the client. Each declares lenses (tag or
   fence, with the instruction claude gets) and actions (a key under the leader). LaTeX and
   the reader view become built-in extensions, with no visible change in behaviour.
2. **Leader key.** Alt+z opens a small popup listing the available actions (`key  title`);
   the next key runs one; Esc or any other key closes it. Alt+u keeps opening the reader.
3. **External extensions.** A directory `~/.config/claudio/extensions/<name>/` with an
   `extension.toml` manifest and a program. claudio starts the program on first use and talks
   to it over JSON lines on stdin/stdout to render lens content and run actions. Its results
   can be: a document (markdown shown in the reader-style view), a notice, text pasted into
   the session, a list to pick from (each item carrying one of the other results), and,
   for actions that declare it, a line of input asked from the user first.
4. **search** (Alt+z s): asks for a query, searches every claude conversation on the active
   session's host (prompts and answers, not tool output), lists matching conversations
   newest first with a snippet, and Enter resumes the chosen one in a new tab (or switches to
   the tab that already has it).
5. **diagram**: ` ```mermaid ` blocks in claude's answers are drawn with box-drawing
   characters in the reader view (flowcharts `graph`/`flowchart` TD/TB/BT/LR/RL, and
   `sequenceDiagram`); anything unsupported stays as the code block it was. Its instruction
   asks claude to use Mermaid fences for diagrams.
6. **snippets** (Alt+z p): a filterable list of saved prompts; Enter pastes the chosen one
   into the active session without sending it. Alt+z P saves the active session's last
   prompt as a new snippet.
7. Per-extension on/off in config; old daemons and old clients keep working; the v0.4/v0.5
   security properties (sanitize, caps, no rendering on the UI thread, no secrets in argv or
   state) hold for every new path.

## Non-goals

- No pane rendering for fence lenses (a diagram has more rows than its source; the overlay
  never moves rows). No status-bar segments from extensions (later).
- No extension marketplace, install command or auto-update; external extensions are files
  the user puts in place.
- External extensions run on the client's machine only; they cannot run code on an SSH host.
  claudio never hands them a proxy token or API key (env scrubbed, nothing secret in
  requests), but they run as the user and could read `config.toml` themselves: they are
  trusted like it.
- No sandboxing of external programs beyond what is listed: they are the user's own code,
  trusted like `config.toml`. What they *return* is untrusted.
- Search is a substring search (no index, no regex, no fuzzy ranking).
- Mermaid coverage is a subset (see the diagram section); no class, state, gantt, ER or pie
  diagrams.

## Current state

- **Hook context.** `hooks::settings(bin, socket, token, latex)` (`src/claude/hooks.rs:60`)
  builds the `--settings` JSON; with `latex`, SessionStart also runs
  `'<bin>' __context latex`, which drains stdin and prints one JSON line with
  `hookSpecificOutput.additionalContext = LATEX_CONTEXT` (`:97-118`). `latex` is the
  daemon host's `[claude] latex` (`src/daemon/mod.rs:78,107`; `src/config.rs:111-128`).
  Claude sessions started by older daemons keep calling `__context latex` with whatever
  `claudio` binary is at that path.
- **Extraction.** `latex_tags(text)` (`src/claude/latex.rs:105`) finds `<latex>…</latex>`
  outside fenced code and inline code, sanitizes, drops empty or > 2048-byte formulas.
  `TranscriptCursor` keeps the last 64 (`src/claude/projects.rs:215-370`).
  `MetaRefresher::refresh` (`src/daemon/session.rs:556-600`) sends `SessionEvent::Latex
  {seq, formulas}` when the list changes and replays the last one on attach (`:459-490,
  849`).
- **Client.** `SessionView.latex_seq/latex_srcs/latex` (`src/tui/sessions.rs:73-124`);
  `Effect::RenderLatex` → the `latex-render` worker (`src/tui/mod.rs:519-640`:
  `render_formula` = `too_complex` pre-check, scratch thread with 32 MiB stack, 150 ms
  budget, ≤ 2 abandoned, LRU 1024) → `App::on_latex_rendered`. `latex::overlay`
  (`src/tui/latex.rs:320`) swaps on-screen `<latex>` tags for their one-line form, matching
  by keys built from the source.
- **Reader.** Alt+u (`Action::Formulas`) sends `Msg::Answers{id}`; the worker
  (`Effect::RenderAnswers`) runs `reader::doc::prepare` (sanitize, then `<latex>` tags →
  private-use placeholders) and `parse` (pulldown-cmark), and renders formulas through the
  same guard; `reader::layout` wraps; `ui/reader.rs` draws.
- **Keys.** `DEFAULT_BINDINGS` (`src/tui/keymap.rs:97`) with `[keys]` overrides and collision
  checks; plain mode keeps Alt+h/s/l/e/u (`build_plain`, `:287`).
- **Spawn.** `SpawnSpec{id,cwd,name,args,env,rows,cols,ephemeral}` and `RespawnSpec`
  (`src/proto.rs:330-370`); the journal `Entry` (`src/daemon/journal.rs:35`) stores
  cwd/name/args/kind/claude_session_id; the client re-spawns dormant entries with `Spawn` at
  recovery (`src/tui/sessions.rs:704-736`); the daemon's `respawn` and `retry_spawn_fresh`
  rebuild a spec from the entry.
- **Resume.** `App::spawn(host, cwd, resume, proxy)` (`src/tui/sessions.rs:316`) adds
  `--resume <id>`.

## Design

### 1. Concepts and registry (client)

```
Extension { name, description, enabled, lenses: Vec<Lens>, actions: Vec<ActionDecl>, kind: BuiltIn | External }
Lens      { name /* tag or fence info word */, kind: Tag | Fence, instruction: String }
ActionDecl{ key: char, title, id /* action name passed back */, input: Option<String> /* prompt */ }
```

`src/tui/extensions/` owns the registry: built-ins (latex, diagram, search, snippets) plus external ones loaded from manifests. Everything else in the TUI asks the
registry: which tag lenses exist (overlay, reader), which fence lenses (reader), the
session context to send at spawn, and the leader's actions. Built-ins are Rust code (a
trait or a closed enum) whose rendering runs on the render worker, never on the UI thread.

Built-ins and their contributions:

| Extension | Lens | Action(s) | Default |
|---|---|---|---|
| latex | tag `latex` (existing instruction, renderer, overlay) | — | on (also off with `[claude] latex = false`) |
| diagram | fence `mermaid`, instruction below | — | on |
| search | — | `s` search conversations | on |
| snippets | — | `p` insert snippet, `P` save last prompt as snippet | on |

The leader's `u` row is the built-in reader action (`Action::Formulas`), not an extension; it
is always present.

Config (`config.toml`, client side):

```toml
[extensions]
diagram = false        # any extension, built-in or external, by name; default true
```

`[claude] latex = false` keeps working: on the client it disables the latex extension; on a
daemon's host it removes the latex lens from that host's sessions (§2). A name that matches nothing is ignored with a one-time notice.

### 2. Session context: from the client to claude

The client builds, for every claude spawn and respawn, the session's extension context from
its enabled lenses:

```rust
pub struct SessionExtensions {          // proto.rs
    /// The session's lenses, in registry order.
    pub lenses: Vec<LensSpec>,
}
pub struct LensSpec {
    pub name: String,                   // ^[a-z][a-z0-9-]{0,23}$
    pub kind: LensKind,                 // Tag | Fence (serde: "tag" | "fence"; unknown → the lens is dropped)
    pub instruction: String,            // ≤ 2048 bytes after `sanitize`; may be empty
}
SpawnSpec   { …, #[serde(default)] pub extensions: Option<SessionExtensions> }
RespawnSpec { …, #[serde(default)] pub extensions: Option<SessionExtensions> }
```

Caps (validated by the daemon, which rejects the spawn with `Error` beyond them, and by the
client, which never builds such a value): ≤ 16 lenses, unique names, each instruction ≤ 2048
bytes after `sanitize`, ≤ 8192 bytes of instructions in total. The daemon extracts the `Tag`
lenses' names; the context is the non-empty instructions joined by a blank line.

Daemon:
- The effective value of a launch: `Some(ext)` in the request replaces the journal entry's;
  `None` (an old client) keeps the journal entry's value; an entry without one gets the
  legacy value, `[LensSpec { name: "latex", kind: Tag, instruction: LATEX_CONTEXT }]` when the
  daemon's `[claude] latex` is on, else `[]`. This applies to `Spawn` (new or journaled id),
  `Respawn`, `retry_spawn_fresh` and the client's recovery (which is a `Spawn`).
- The daemon's `[claude] latex = false` removes the lens named `latex` from every effective
  value before use (so a host that turned math off stays off, as today).
- The journal `Entry` gains `#[serde(default)] extensions: Option<SessionExtensions>`,
  written with the effective value.
- At each launch the daemon writes the context to `<daemon runtime dir>/context/<session-uuid>.txt`
  (dir 0700, file 0600, atomic write), and SessionStart runs `'<bin>' __context file '<path>'`
  instead of `__context latex`. Empty context → no file and no context hook. The file is
  removed when the session is removed, and orphans in `context/` are removed at daemon start.
  The path is stable for the session's life (`<session-uuid>.txt`, not per launch): `/clear`
  and compaction re-read the same file. At daemon start the `context/` dir is cleared before
  any launch; a running claude from a previous daemon is being stopped at that point and its
  hook, if it fires, prints nothing.
- `claudio __context file <path>`: drains stdin like today, opens the file with
  `O_NOFOLLOW | O_RDONLY` (no symlink is followed), then `fstat`s the open descriptor and reads
  at most 16 KiB only if it is a regular file owned by the current uid, sanitizes it, and prints
  the same one JSON line with that text as `additionalContext`; prints nothing if the file
  is missing, empty or not acceptable; always exits 0. `__context latex` keeps working
  unchanged (old sessions).
- The instruction text never appears in argv, the `--settings` JSON, logs or `state.json`.

### 3. Tag lens items: daemon → client

```rust
SessionEvent::Lens { seq: u64, items: Vec<LensItem> }      // proto.rs
pub struct LensItem { pub tag: String, pub text: String }
Welcome { …, #[serde(default)] pub features: Vec<String> }  // contains "lens", "search", "last_prompt"
```

- `latex_tags` generalizes to `lens_tags(text, tags: &[&str])` with the same rules (outside
  fenced and inline code, case-sensitive, across lines, trimmed, `sanitize`d, empty or
  > 2048 bytes dropped); `extract_latex`/`latex_tags` remain as wrappers.
- `TranscriptCursor` keeps the session's last 64 lens items (all tags together, oldest
  first) in addition to the latex list. `MetaRefresher` sends `Lens` when the item list
  changes, with the same `seq` rules as `Latex` (grows for the actor's whole life; a new
  conversation sends an empty list under the next seq), and replays the last one on attach
  right after the latex replay. It keeps sending `Latex` exactly as today, for old clients.
- Encoded `Lens` events stay ≤ 256 KiB (oldest items dropped first).
- Client: a daemon whose `Welcome.features` contains `"lens"` is a lens daemon: the client
  ignores its `Latex` events and uses `Lens`. For an older daemon the client maps each
  `Latex` event to `Lens` items with tag `latex`. `seq` handling stays as for latex today
  (reset on `Attached` / `ClaudeSession`).

### 4. Rendering lenses (client)

All built-in rendering stays on the `latex-render` worker (renaming it `lens-render` is
allowed) through the existing guard: pre-check, scratch thread, 150 ms budget, abandon cap, LRU cache
keyed by `(lens, source)`.

- **Tag lens → a rendering with `inline: Option<String>`, `block: Option<Laid>`, `text:
  Option<Laid>`**, the generalization of `latex::Formula`/`Rendered`. latex keeps its
  renderer exactly.
- **Pane overlay**: `overlay` works for every enabled tag lens: it finds `<tag>…</tag>` on
  screen and matches the item by the same keys (strict/loose/html), with latex's behaviour
  unchanged (table borders, spans up to 8 rows, rows never move).
- **Reader**: `doc::prepare` replaces the tags of every enabled tag lens (first 64 overall)
  with placeholders; a placeholder alone in its paragraph/item/cell is display style as for
  latex today. A fenced code block whose info string's first word is an enabled fence lens
  becomes a lens block, rendered on the worker to rows; a failed or too-large rendering
  shows the code block as it was. Diagram rows wider than the reader width show the code
  block plus one dim line `diagram is N columns wide`.
- **External lens**: never rendered on the render worker. The worker renders built-ins only
  and leaves an external lens's items `raw`; `App::on_lens_rendered` then asks the extension
  host (§6) for the still-raw external items as one batch (`Effect::RenderExternal { host,
  id, seq, items }`). The host runs them asynchronously (1 s timeout per item, ≤ 16 in
  flight), caches results by `(lens, source)` (also a timeout or error, as raw, for 60 s), and
  answers with `HostEvent::ExternalRendered { host, id, seq, items }`, applied only if `seq`
  is still the session's current list. The reader does the same for its document's external
  placeholders and fence blocks under its `gen`. Inline renderings are used by the pane and
  inline reader spans, block renderings by display placements and fence lenses.

### 5. Leader and actions (client)

- New keymap action `Extensions` (config name `extensions`), default **Alt+z**, global
  scope, also in plain mode. The help popup lists it and the leader's table.
- Alt+z opens the leader popup: one line per action, `key  title  (extension)`, in
  registry order. A key that matches runs the action; Esc or any other key closes the popup;
  nothing reaches the session. Actions that need a session (paste, search resume, reader,
  save prompt) say "no active session" when there is none. In plain mode search is not
  offered.
- Action results (built-in and external share them):

```rust
pub enum ActionResult {
    Document { title: String, markdown: String },   // reader-style modal, lenses rendered
    Notice { text: String },                         // status-line notice
    Paste { text: String },                          // bracketed paste into the active session
    List { title: String, items: Vec<ListItem> },    // picker
    // internal only (built-ins), e.g. Resume { host, cwd, conversation }
}
pub struct ListItem { label: String, detail: Option<String>, result: ActionResult /* not List */ }
```

- **Input prompt**: an action with `input = Some(prompt)` first opens a one-line input modal
  (typing, Backspace, Ctrl+U, ←/→, Home/End, Enter, Esc; ≤ 512 bytes); the text is passed to
  the action.
- **List picker**: title, an always-on filter line (typing filters by case-insensitive
  substring of label and detail), ↑/↓/PgUp/PgDn/Home/End, mouse wheel, Enter applies the
  selected item's result, Esc closes. Shows `label` and, dimmed, `detail` cut to the width.
- **Document**: the reader view's drawing and keys (scroll, wheel, Esc) on a single
  document; markdown goes through the same `doc`/`layout` path on the worker.
- **Paste**: the text is sanitized (no controls but `\n`/`\t`), ≤ 64 KiB. It is written to the
  active session only when that session is a claude session (`SessionKind::Claude`), is
  attached, and its mirror reports bracketed paste on (`Screen::modes().bracketed_paste`, the
  same check `encode_paste` makes); then as `ESC[200~ … ESC[201~`, never followed by Enter.
  Otherwise nothing is written and a notice says `paste needs claude's prompt (bracketed
  paste is off)`. A paste into a terminal tab is refused with `paste goes to claude sessions
  only`.

### 6. External extensions

Layout: `~/.config/claudio/extensions/<name>/extension.toml` (`$XDG_CONFIG_HOME` honoured,
as for `config.toml`).

```toml
name = "keys"                      # must equal the directory name; ^[a-z][a-z0-9-]{0,31}$
description = "keyboard keys"      # ≤ 120 chars
command = ["python3", "keys.py"]   # argv, no shell; a first element containing '/' resolves
                                   # against the extension dir; otherwise looked up in PATH
[[lens]]
tag = "keys"                       # or: fence = "abc"  (exactly one of tag/fence)
instruction = "Write keyboard keys as <keys>Ctrl+C</keys>."
[[action]]
key = "k"                          # one char [A-Za-z0-9], unique among all actions
title = "insert a timestamp"       # ≤ 60 chars
id = "stamp"                       # ^[a-z][a-z0-9_-]{0,31}$
input = "label"                    # optional: ask for a line of input first
```

Loading (at client start): manifests ≤ 64 KiB; invalid manifests, names that clash with a
built-in or another extension, tags/fences already taken, and action keys already taken are
skipped (the whole extension, or the one action for a key clash) with one notice each at
start. At most 16 external extensions. A tag that is an HTML element name (the WHATWG list:
`a`, `b`, `br`, `code`, `details`, `kbd`, `p`, `pre`, `sub`, `sup`, `summary`, `table`, … — a
`const` in `extensions/manifest.rs`) is rejected with the notice `tag <x> is an HTML element;
pick another name`, except that a manifest may set `html = true` on the lens to accept the
shadowing knowingly.

Process: started lazily on first request, cwd = the extension dir, stdin/stdout piped,
stderr discarded, its own process group. Environment: the client's environment minus
`SESSION_MARKERS`, `CLAUDIO_PROXY_URL`, `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN` and every
variable whose name starts with `CLAUDIO_`, plus `CLAUDIO_EXTENSION=<name>` and
`CLAUDIO_EXTENSION_PROTOCOL=1`. Killed (SIGTERM to the group, SIGKILL after 1 s) when the
client exits. A crash restarts it on the next request; after 3 failures (crash, protocol
error, timeout) within 5 minutes the extension is disabled for the rest of the client's run
with a notice.

Protocol v1 (UTF-8 JSON, one object per line, each line ≤ 1 MiB, ids are u64 chosen by
claudio; the program may answer out of order):

```
→ {"id":1,"method":"render","params":{"lens":"keys","kind":"tag","source":"Ctrl+C"}}
← {"id":1,"result":{"inline":"⌃C","block":["┌────┐","│ ⌃C │","└────┘"]}}
→ {"id":2,"method":"action","params":{"action":"stamp","input":"x",
     "session":{"host":"local","cwd":"/home/u/p","name":null,"conversation":"<uuid>"}}}
← {"id":2,"result":{"kind":"paste","text":"2026-11-05 16:40"}}
← {"id":3,"error":"message"}          (any request may fail)
```

`result.kind` is `document` (`title`, `markdown`), `notice` (`text`), `paste` (`text`) or
`list` (`title`, `items: [{label, detail?, result}]`, nested results not `list`). A render
result's `inline` (one line) and `block` (rows) are both optional. `session` is `null`
when no session is active. Timeouts: render 1 s, action 15 s; at most 16 requests in flight
per extension. Caps applied to every result before use: inline ≤ 512 columns, block ≤ 200
columns × 100 rows, document markdown ≤ 256 KiB, list ≤ 500 items with label ≤ 200 and
detail ≤ 400 chars, paste ≤ 64 KiB, notice ≤ 200 chars, title ≤ 120 chars. Every string
goes through `sanitize`; inline, labels, details, notices and titles also lose `\n`/`\t`.
Widths are `unicode-width` columns after `sanitize`. A render result over any cap is
discarded (the item stays raw, counted as an error). For action results: markdown and paste
text over cap are discarded with a notice `result too large`; lists keep their first 500
items; labels, details, notices and titles are cut to the cap on a char boundary.
A line that is not valid JSON is a protocol error (the process is restarted). A JSON
object without a known `id` is ignored.

The extension host runs off the UI thread (tokio tasks owned by `tui/mod.rs`); `App` only
emits effects and receives events. Both render and action calls are async requests to the
host, answered as `HostEvent`s. The render worker never waits on an external process.

### 7. diagram (built-in fence lens `mermaid`)

Instruction:

> This terminal draws Mermaid diagrams. When a diagram helps (architecture, data or control
> flow, state machines, message sequences), write it as a fenced code block with the
> language `mermaid`, using `flowchart TD`/`flowchart LR` or `sequenceDiagram`. Keep labels
> short (under 30 characters) and diagrams small (under 20 nodes). Do not draw box diagrams
> with characters yourself.

Engine: `src/tui/diagram/`, pure, called only on the worker through the guard.

```rust
pub fn too_complex(src: &str) -> bool;   // cheap pre-check: > 4096 bytes or > 200 lines
pub fn render(src: &str) -> Result<Vec<String>, Unsupported>;
```

- **Flowchart** (`graph|flowchart` + `TD|TB|BT|LR|RL`, default TD): nodes `id`, `id[t]`,
  `id(t)`, `id([t])`, `id((t))`, `id{t}`, `id[[t]]`, `id[(t)]`, `id>t]`, `id{{t}}`, quoted
  `id["t"]`; edges `-->`, `---`, `-.->`, `-.-`, `==>`, `===`, `--o`, `--x`, `<-->`, labels
  `-->|t|` and `-- t -->`; chains `a --> b --> c`; `a & b --> c`; `;` separators; `%%`
  comments; `subgraph … end` (members kept, frame optional); `style`, `classDef`, `class`,
  `click`, `linkStyle` lines ignored. Layout: layered (cycles broken by reversing back
  edges, longest-path layers, dummy nodes for long edges, barycenter ordering), boxes with
  `┌─┐` (rect), `╭─╮` (rounded/stadium/circle), a distinct mark for decisions (`{}`),
  orthogonal edges with merged junctions, `▼ ▶ ▲ ◀` arrowheads, dotted (`┆`/`┄`) and thick
  (`┃`/`━`) lines where possible, labels beside the edge. BT/RL draw as TD/LR with the
  layer order reversed. An edge the router cannot place is listed under the diagram as
  `a ──label──▶ b`. Labels wrap at 24 columns, at most 3 lines.
  v1 draws distinctly: rect `[]`, rounded `()`/`([])`, decision `{}`; solid `-->`/`---` and
  dotted `-.->`/`-.-`; `|t|` labels. Everything else listed is parsed and drawn as the
  nearest of those (other shapes as rect, `==>`/`===` as solid, `--o`/`--x` as plain arrows,
  `<-->` as two edges, `-- t -->` as `|t|`). BT/RL draw as TD/LR reversed; `subgraph`
  members are kept, no frame.
- **Sequence** (`sequenceDiagram`): `participant`/`actor` (with `as` alias), implicit
  participants, messages `->`, `-->`, `->>`, `-->>`, `-x`, `--x`, `-)`, `--)` with text,
  self-messages, `Note left of|right of|over a[,b]: t`, `loop|alt|else|opt|par|and|critical|
  break|rect … end` drawn as labeled separators across the lifelines, `autonumber`,
  `activate`/`deactivate` and `+`/`-` suffixes ignored.
  v1 draws distinctly: solid and dotted arrows with/without heads, `Note`, and
  `loop|alt|else|opt` blocks; `par|and|critical|break|rect` are drawn as `opt`-style labeled
  blocks; `-x`/`-)` variants as plain arrows. AC8 goldens cover the v1-distinct set.
- Caps: ≤ 64 nodes/participants, ≤ 128 edges/messages, output ≤ 200 columns × 150 rows;
  beyond → `Unsupported`. Everything else (`classDiagram`, `stateDiagram`, `gantt`, …) →
  `Unsupported`, and the reader shows the code block.

### 8. search (built-in action `s`)

Protocol:

```rust
Msg::Search { query: String, limit: u32 }                 // client → daemon
Msg::SearchReply { query: String, hits: Vec<SearchHit>, scanned: u32, total: u32, truncated: bool }
pub struct SearchHit {
    pub conversation: String, pub cwd: String, pub modified: u64,
    pub title: String, pub snippet: String, pub matches: u32,
}
```

Daemon (`src/claude/search.rs`, run in `spawn_blocking` from a spawned task, as `Answers`):
- Query: trimmed, 2..=200 bytes after `sanitize`, split on whitespace into ≤ 8 terms; a
  message matches when it contains every term, case-insensitively (Unicode lowercase).
- Scope: `~/.claude/projects/*/*.jsonl` (the same directory `projects.rs` scans; no
  subdirectories), newest file first. Messages: real user prompts by `recent_answers`'
  rules and assistant `text` blocks; never tool results, thinking, meta or sidechain lines.
- One hit per conversation: `matches` = matching messages; `snippet` = ≤ 160 chars around
  the first term in the newest matching message, newlines as spaces; `title` = the first
  real prompt (≤ 80 chars); `cwd` from the transcript lines; all `sanitize`d.
- Budget: files are read as a stream with a `BufReader` (64 KiB buffer) line by line; a line
  > 1 MiB is skipped without being buffered (bytes are discarded until the next `\n`); at most
  the last 64 MiB of a file are read (seek to `len - 64 MiB`, skip the partial first line).
  The search stops after 3 s, 1 GiB read in total, or `limit` (≤ 100, default 50) hits,
  setting `truncated`; `scanned`/`total` count files. One search runs at a time per daemon:
  a second `Search` while one runs is answered `Error "search busy"` at once, and the client
  shows `search busy, try again`. Reply ≤ 512 KiB encoded (hits dropped from the end).
- Unknown op on an old daemon → `Error "unsupported op…"` → the usual restart notice.
  `features` contains `"search"`.

Client: Alt+z s → input "search conversations" → `searching…` notice → list titled
`N conversations · "query"` (or a notice "no matches"), labels `<when>  <~/cwd>  <title>`,
detail the snippet. Enter: if a tab on that host has that conversation, switch to it;
otherwise spawn on that host in the hit's cwd with `--resume <conversation>` and the proxy
choice the wizard would preselect there. The search runs on the active session's host
(local when none).

### 9. snippets (built-in actions `p`, `P`)

- Store: `~/.config/claudio/snippets/` (`$XDG_CONFIG_HOME`), files `*.md` and `*.txt`, ≤ 64
  KiB each, ≤ 500 files, name = file stem; sorted by name. Read on each Alt+z p (no cache),
  off the UI thread.
- Alt+z p → list `Snippets` (label = name, detail = first non-empty line); Enter pastes the
  snippet's text (§5 Paste). Empty or missing dir → notice `no snippets in <dir>`.
- Alt+z P → `Msg::LastPrompt { id }` (new op; `features` contains `"last_prompt"`), answered
  by `Msg::LastPromptReply { id, prompt: Option<String>, cut: bool }`: the newest real prompt
  of the transcript tail (`answer_prompt`'s rules, answered or not), sanitized, ≤ 64 KiB (cut,
  with `cut` set). Saved as `<slug>.md` (slug: the first words, lowercase `[a-z0-9-]`, ≤ 40
  chars, `-2`, `-3`… if taken; `snippet` if empty), dir created 0700, file 0600, atomic
  write; notice `saved snippet <name>` (`… (cut at 64 KiB)` when `cut`). No prompt → notice
  `no prompt to save`. On an old daemon the usual restart notice.

### Alternatives considered

| Option | Why not |
|---|---|
| Daemon extracts every `<word>…</word>` tag, no tag list | Claude writes HTML/XML and generics; false positives and wasted events. |
| Instructions through the `--settings` hook command or env vars | Text in argv/env of claude and every tool it runs; quoting risks. A 0600 file is simpler. |
| Hook relay asks the daemon for the context | Changes the relay's contract ("never prints"); the file is enough. |
| `<diagram>` tag lens | Tag text would show raw (joined into a paragraph) in the pane; Mermaid fences are what claude already writes and Claude Code shows them line by line. |
| Diagrams overlaid in the pane | A drawn diagram has more rows than its source; the overlay never moves rows. |
| Extensions as dynamic libraries / WASM | Unsafe or heavy; a process over JSON lines is language-agnostic and isolated by the OS. |
| One process per request | Startup cost per render (Python ≈ 30 ms × 64 items). |
| Search index | Extra state and staleness; a 3 s bounded scan of recent transcripts is enough for v1. |

## Compatibility and rollout

| Client \ Daemon | old (≤ 0.5) | new |
|---|---|---|
| old | as today | `extensions` absent → legacy `[claude] latex`; old client ignores `Lens` (Unknown) and still gets `Latex`; no `Search` sent |
| new | `extensions` ignored (daemon's own `[claude] latex`); no `"lens"` feature → `Latex` mapped to items; `Search` → restart notice | full |

- Journals without `extensions` load as `None` (legacy behaviour at recovery). v0.5.0
  reading a journal with the new field must ignore it (`Entry` has no
  `deny_unknown_fields`; verify in the plan).
- Running claude sessions started by old daemons keep their `__context latex` hook.
- Keys: Alt+z is new; a user's `[keys]` binding on Alt+z goes through the existing collision
  handling.
- Rollback: reinstall v0.5.0, then `claudio daemon stop` on every host (an older binary never
  replaces a newer daemon). The v0.5.0 daemon ignores the journal's `extensions` field and
  leaves `<runtime dir>/context/` alone (harmless; removed by the next new daemon or a
  reboot). Sessions started by the new daemon keep calling `__context file`, which v0.5.0's
  `__context` answers with nothing (exit 0).

## Failure modes and security

- **Untrusted text**: claude's answers (prompt-injectable) reach the daemon's extraction,
  the client's renderers and external programs' stdin. Every string that reaches the
  terminal (renderings, document markdown, list labels, notices, paste text, search
  snippets) goes through `sanitize` and the caps above, then through the reader's `safe`
  width rules when drawn. Paste text cannot contain ESC (sanitize removes controls), so it
  cannot end the bracketed paste early. A paste is refused unless the pane has bracketed
  paste on, so a `\n` in the text can never act as Enter (AC7 and AC10 test the refusal).
- **External programs**: lazily started, killed with the client, restarted at most 3 times
  per 5 min, timeouts on every request, line and in-flight caps, no tokens handed to them
  (env scrubbed, none in requests), no
  claude content in argv. A hung program never blocks the UI thread, and blocks the render
  worker at most for the stated budgets.
- **Context file**: daemon-chosen path in its 0700 runtime dir; `__context file` refuses
  symlinks, non-regular files and files not owned by the uid; content sanitized and capped.
- **Search**: reads only the uid's `~/.claude/projects`; bounded time, bytes and reply; runs
  off the daemon's async executor; the query is never put in a shell or regex.
- **Diagram engine**: hostile Mermaid inside the caps must not hang or overflow: it runs only
  through the guard (scratch thread, budget, abandon cap), iterative where depth could grow,
  with explicit node/edge/size caps checked before allocating.
- **Snippets**: names are generated by claudio (slug), never taken from claude's text as a
  path; files written 0600 via temp + rename.
- **Concurrency**: one leader popup / modal at a time; replies are matched to the modal
  generation that asked (as the reader does) and dropped otherwise.

## Acceptance criteria

- [ ] AC1 (no regression): all existing unit and manager tests pass, the only permitted edits
  being the plain-mode key-set assertions (`src/tui/plain.rs`, `keymap.rs` `PLAIN_ACTIONS`)
  extended with Alt+z; latex pane overlay, reader view, Tab list and `[claude] latex = false`
  behave as in v0.5.0.
- [ ] AC2 (context): a spawn with `extensions` writes the context file 0600 and the
  SessionStart hooks run `__context file`; `__context file` prints the JSON line for a good
  file and nothing for a missing, symlinked, foreign-owned or non-regular one (unit tests);
  `__context latex` still works (existing test); the `--settings` JSON and the claude argv the
  fake records contain the hook path but no instruction text (manager test).
- [ ] AC3 (journal/respawn): a respawn with `extensions` replaces the journaled value; a
  journal entry without the field loads as `None` and a new one round-trips (unit tests).
- [ ] AC4 (lens events): `lens_tags` extracts several tags with latex's rules; the daemon
  sends `Lens` with the seq rules and replays it on attach; the client ignores `Latex` from a
  lens daemon and maps it from an old one (unit tests).
- [ ] AC5 (external lens): a manager test with a fake external extension (shell script) that
  renders `<keys>` shows the rendering in the pane overlay and in the reader.
- [ ] AC6 (leader): Alt+z shows the leader popup with the built-in actions and the external
  one; Esc closes it and nothing reaches the session; Alt+z u opens the reader (manager test).
- [ ] AC7 (external action): the fake extension's `paste`, `list` and `document` results work
  end to end (paste reaches the fake claude without Enter; list picks; document shows);
  a hanging action times out with a notice; a crashing program is restarted and disabled
  after 3 failures (manager or unit tests with fakes).
- [ ] AC8 (diagram): unit goldens for a TD flowchart with a decision and labels, an LR chain,
  a cycle, a sequence diagram with a note and a loop, an unsupported type, and hostile input
  hitting each cap (returns `Unsupported` fast); a manager test shows a ` ```mermaid `
  answer drawn in the reader.
- [ ] AC9 (search): unit tests over temp transcripts (term AND, case-insensitive, tool output
  and meta excluded, one hit per conversation, newest first, budget truncation, sanitized
  snippet); a manager test finds a fake conversation with Alt+z s and Enter resumes it
  (`--resume <id>` reaches the fake claude).
- [ ] AC10 (snippets): unit tests for load (types, caps, sort) and save (slug, collisions,
  0600); manager test: Alt+z p, filter, Enter pastes into the fake claude; Alt+z P saves the
  last prompt.
- [ ] AC11 (config): `[extensions] name = false` removes a built-in's lens (no context, no
  overlay) and actions; invalid external manifests are skipped with a notice (unit tests).
- [ ] AC12 (real claude, isolated daemon): with diagram on, asking claude for an
  architecture diagram yields a ` ```mermaid ` block that the reader draws; search finds a
  real conversation and resumes it; a snippet pastes into real claude; latex still renders.
- [ ] AC13 (builds): release build with 0 warnings, `make static` ok, unit + manager suites
  green, rustfmt clean; binary growth reported.

## Constraints

- No AI attribution or co-author trailers in commits (owner's global rule); conventional
  commits; commit with explicit paths; never push from a task.
- Protocol additions backward compatible (`#[serde(default)]`, `Unknown` catch-alls); new
  request ops fail safe on old daemons.
- `app.rs` stays pure; I/O only in `tui/mod.rs` (and the extension host it drives).
- Nothing is rendered or parsed on the UI thread: latex, diagram, markdown and external
  renders all go through the worker and its guard (external: through the host with
  timeouts).
- Secrets never in argv, logs, journal or `state.json`; claudio never passes external
  extensions a proxy token or API key.
- Tests: manager tests assert on the screen model only; fake claudes ignore `--help` and
  `--version`; test configs set `update_check/remote_check = "off"`; never touch the user's
  daemon (isolated XDG dirs).
- Build output and scratch outside `/tmp` and the working tree (`~/.cache/claudio/…`).
- Public repo content must not contain personal paths or host names.

## Open questions

None (owner delegated approval on 2026-11-05 16:22).
