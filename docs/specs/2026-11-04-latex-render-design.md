# LaTeX formulas in the session pane: design

Status: audited (rev 2, spec audit folded in)   Class: architectural
Owner decisions so far (2026-11-04):
- Scope: steps 1–3 only (instruction injection, inline one-line rendering, formulas popup).
  Pixel rendering (sixel/kitty) is out of scope.
- The instruction is **opt-in**: `[claude] latex = true` in `config.toml`, read by each host's
  daemon from its own config (the `allow_skip_permissions` pattern).
- The formulas popup is on **Alt+u** (free in both claude and claudio; rebindable via `[keys]`).
- Formulas whose 2D rendering needs several rows are shown inline as **one-line math**
  (`x = (−b ± √(b²−4ac))/(2a)`); the popup shows the 2D form.

## Problem

Claude writes math as raw LaTeX, which reads badly in a terminal. The owner wants claudio to tell
claude to wrap formulas in `<latex>…</latex>` and to show them rendered in the session pane, in
every common terminal (gnome-terminal, konsole, ghostty, kitty, WezTerm, foot, xterm).

Verified facts (2026-11-04, claude 2.1.312) that shape the design:
- **No pixel graphics in VTE terminals.** VTE strips sixel from every stable release and has no
  kitty graphics protocol, so gnome-terminal, Tilix, Ptyxis and xfce4-terminal can only show
  text. A universal solution renders math as Unicode text.
- **A SessionStart hook's `additionalContext` reaches the model.** With
  `{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"…wrap math in
  <latex>…</latex>…"}}` printed by a hook, haiku answered "what is the quadratic formula" with
  `<latex>x = \dfrac{-b \pm \sqrt{b^2 - 4ac}}{2a}</latex>` and inline tags.
- **Claude Code prints the tags verbatim, but its markdown pass strips backslash escapes on
  screen.** `\{a_i\} \cup \{b_i\} \\ … \_ \# \&` showed as `{a_i} \cup {b_i} \ … _ # &`, and
  `x^2\,dx` as `x^2,dx`. The transcript JSONL keeps the exact text. So formula *source* must
  come from the transcript; the screen is only used to *locate* each formula.
- **Claude Code wraps long lines itself** (hard newlines with a two-space indent, not terminal
  soft wraps): `<latex>a \neq` / `  0</latex>`.
- **`term-maths` 1.0.0** (MIT OR Apache-2.0; parser `rust-latex-parser` 0.1.1, MIT) renders
  LaTeX math as a 2D Unicode grid and handles what the model wrote (`\dfrac`, `\neq`, `\cdots`,
  `\sqrt`, sums, integrals, `pmatrix`). Known bugs: the `√` overline is offset by two columns,
  `\sin x` renders as `sinx`, `cases` is wrong. It has no one-line output mode. Its ratatui and
  crossterm features are optional (no `default` features); claudio's `unicode-width` 0.2.2 is
  compatible; it adds `unicode-segmentation` (pure Rust).
- **The parser is slow and deep on hostile input** (spec audit, release build): `x/` × 1000
  (2000 B) takes 67 s; `\frac ` × 340 takes 1.4 s; `sqrt(` × 800 takes 3 s; braceless
  `\frac `/`\hat `/`\not` chains overflow a 2 MiB stack inside 2048 B. term-maths layout panics
  on some inputs under overflow checks (`\binom{}{}`, debug only). So rendering is treated as an
  untrusted, possibly non-terminating computation.

## Goals

1. With `[claude] latex = true`, every claude session the daemon starts is told, through a
   SessionStart hook, to wrap math in `<latex>…</latex>`.
2. The daemon extracts the exact LaTeX of every `<latex>` tag in the session's assistant text
   from the transcript and keeps the client's list of recent formulas current, including for a
   client that attaches later.
3. In the session pane, every complete on-screen `<latex>…</latex>` span whose formula is in that
   list is replaced by a one-line Unicode rendering, in an accent colour, without moving any
   other row.
4. Alt+u opens a scrollable popup with the session's recent formulas rendered in 2D Unicode, each
   with its LaTeX source.
5. All of the above works the same over SSH (remote daemon, local client) and in `--plain`.
6. No formula, however hostile, can freeze, crash or corrupt the manager's UI.

## Non-goals

- Pixel rendering (sixel, kitty, iTerm2) and any LaTeX-to-image engine.
- `-p` print mode and `--api` (they have their own hooks and no pane).
- Rendering `$…$` / `$$…$$` or any syntax other than `<latex>` tags.
- Fixing `term-maths` typesetting bugs (√ bar offset, `sinx`, `cases`); they are tracked, not
  fixed here.
- Copying a rendered formula (selection copy keeps returning the on-screen text) and a toggle to
  show raw tags.
- Formulas in shell (terminal) tabs.
- Formulas with a relation written without spaces that markdown reads as an HTML tag
  (`a<b>c`): they may fail to match and stay raw (the instruction asks for spaces).

## Current state

- Hooks: `claude::hooks::settings(claudio_bin, socket, token)` (src/claude/hooks.rs:46) builds the
  `--settings` JSON, one `{"matcher":"*","timeout":5,"hooks":[{"type":"command","command":
  "'<bin>' __hook <Event> '<socket>' <token>"}]}` entry per event in `EVENTS` (:23). The relay
  (`relay`, :91) is documented to print nothing to stdout. `command()` (src/daemon/session.rs:265)
  passes `--settings` only for `SessionKind::Claude`. Hidden subcommands are dispatched in
  src/main.rs before `cli::parse` (`__probe` :42, `__hook` :115); `__context latex` (argv len 3)
  is not caught by the concealed-relay branch (its port does not parse) nor by
  `proxy::cmd::dispatch`.
- Daemon config: `daemon::Config::from_env` reads `crate::config::load().claude.allow_skip_permissions`
  once at start (src/daemon/mod.rs:102). `ClaudeSection` is in src/config.rs:97.
- Transcript: `TranscriptCursor` (src/claude/projects.rs:210) reads appended bytes incrementally;
  `apply_record` (:296) parses `assistant` records but returns early without `message.usage`
  (:310) and never reads `message.content`. One record is written per content block.
- `MetaRefresher::refresh` (src/daemon/session.rs:519) advances the cursor off the actor on every
  hook (no gap for SessionStart/Stop/StopFailure/UserPromptSubmit) and every 10 s while a client
  is attached, then `daemon.broadcast(id, meta_event(…))`. `broadcast` goes through the
  `daemon.events` tokio broadcast channel, forwarded by each client's loop task
  (src/daemon/mod.rs:451-456, src/daemon/server.rs:265-301), which can drop events for a slow
  client (`Lagged`). Nothing is replayed on attach: `Actor::attach` (:764) queues only
  `Attached` + the snapshot straight into the subscriber's queue (`Subscriber::sync`, :422).
- Protocol: `SessionEvent` (src/proto.rs:485) is `#[serde(tag="kind")]` with a `#[serde(other)]
  Unknown` arm (:544), so an old client ignores a new kind (precedent: `ClaudeSessionCleared`,
  test at :1075). `MAX_FRAME` is 1 MiB (:27).
- Client: the TUI runs `App` on the main thread inside `rt.block_on` (src/tui/mod.rs:167-177).
  `App::on_event` (src/tui/interaction.rs:640) merges events into `SessionView`
  (src/tui/sessions.rs:39). The TUI installs a panic hook that restores the terminal before
  unwinding (src/tui/mod.rs:344-348), so even a caught panic would leave the UI broken.
  `[profile.release]` sets only `strip = true` (no `panic = "abort"`).
- Pane: `draw_pane` (src/tui/ui/mod.rs:299) calls `Screen::render` (src/term/screen.rs:389),
  then highlights the selection. `Screen::row_chars` (:457) exposes a visible row's characters
  (a wide char in both its columns); `cell_grid` (:527) is `#[cfg(test)]`. Nothing scrolls the
  mirror (`display_offset` is always 0). Styles are read from the ratatui `Buffer` that
  `Screen::render` has just filled.
- Popups: `Modal` (src/tui/interaction.rs:33) with `opened_by` / `yields_to_actions`,
  `modal_key` (:201), `draw_modal` (src/tui/ui/mod.rs:82). Keys: `DEFAULT_BINDINGS`
  (src/tui/keymap.rs:92); plain mode keeps `PLAIN_ACTIONS: [Action; 4]` (Help, ProxyStats,
  GitLog, Reset).

## Design

### 1. Instruction injection (daemon host)

- `ClaudeSection` gains `latex: bool`, `#[serde(default)]` (false), documented in the config
  header comment and README.
- `daemon::Config` gains `latex: bool`, read from `crate::config::load().claude.latex` at start
  like `skip_permissions`. The value is read at daemon start: after editing `config.toml`, run
  `claudio daemon restart` on the host that runs the session. The popup hint and the README say
  exactly this, and that the setting lives on the session's host.
- `hooks::settings` gains a `latex: bool` parameter. When true, the `SessionStart` entry's
  `hooks` array gets a second command, `'<bin>' __context latex` (same matcher and timeout).
  Every other entry is unchanged.
- New hidden subcommand `claudio __context latex`, dispatched in src/main.rs next to `__hook`:
  drains stdin (capped like the relay) and prints exactly one line,
  `{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":LATEX_CONTEXT}}`,
  then exits 0. Any other `__context` argument prints nothing and exits 0. It never touches the
  socket.
- `LATEX_CONTEXT` (one constant in src/claude/hooks.rs) says: when writing a mathematical formula
  or expression, wrap its LaTeX in `<latex>…</latex>` instead of `$…$`/`$$…$$`; a display formula
  goes alone on its own line; never inside code blocks or inline code; no tags around plain
  numbers or code; write relations with spaces around them (`a < b`, not `a<b`).
- It is context, not the system prompt: `--append-system-prompt` is not used (an altered system
  prompt risks the third-party classification described in CLAUDE.md).

### 2. Formula extraction and transport (daemon)

- `TranscriptCursor` keeps `latex: VecDeque<String>`, the most recent formulas in conversation
  order, and exposes `latex() -> &VecDeque<String>`. It is reset with the totals when the file
  shrinks, and a new cursor (new claude session id, `/clear`) starts empty.
- In `apply_record`, for an `assistant` record that is not `isSidechain == true`, the text is
  scanned **before** the `usage` early return. `message.content` is either a string (scanned as
  one text) or an array, in which only blocks with `type == "text"` are scanned (`thinking`,
  `tool_use`, `tool_result` are not). Extraction (`extract_latex(text) -> Vec<String>`, pure):
  - non-greedy `<latex>` … `</latex>` (case-sensitive), may span newlines;
  - skipped inside fenced code blocks (a line whose trimmed start is ``` or `~~~` toggles) and
    inside inline code spans (backtick runs on one line);
  - the inner text is trimmed; empty formulas are dropped;
  - `sanitize` (shared with the client, in a module both can use) removes every `char` in
    general category `Cc` except `\n` and `\t`, `Cf` (bidi controls, zero-width, BOM, soft
    hyphen), `Co`, `Cn` and `Cs`; a combining mark (`Mn`) with no base character before it is
    dropped. A table-free check is acceptable: `char::is_control()` plus explicit `Cf` ranges
    U+00AD, U+0600–U+0605, U+061C, U+06DD, U+070F, U+180E, U+200B–U+200F, U+202A–U+202E,
    U+2060–U+206F, U+FEFF, U+FFF9–U+FFFB, U+1D173–U+1D17A, U+E0001, U+E0020–U+E007F, plus the
    private-use ranges (U+E000–U+F8FF, U+F0000–U+10FFFF);
  - a formula longer than `MAX_FORMULA_BYTES` = 2048 after sanitizing is dropped (not
    truncated);
  - the deque keeps the last `MAX_FORMULAS` = 64.
- `SessionEvent::Latex { seq: u64, formulas: Vec<String> }`, always the **whole** list (the
  client replaces its copy). `MetaRefresher` holds the last sent state in
  `Arc<Mutex<Option<(u64, Vec<String>)>>>`. After each refresh, if the cursor's list differs
  from the stored one, it increments `seq` (starting at 1) and stores the new list under the
  lock, then broadcasts it. An empty list is sent when a non-empty one is cleared. `seq` keeps
  growing for the actor's whole life: a new claude session id (new cursor) starts an empty list
  with the next `seq`, so a new conversation's lists always outrank anything sent before (a
  replay racing a `/clear` can't pin a stale list).
- Replay on attach: `Actor::attach` reads the slot and, if set, queues `(seq, list)` as a
  `Msg::Event` control frame right after `Subscriber::sync`. Ordering against a concurrent
  broadcast does not matter because of `seq` (below). Residual: a `Lagged` drop of a broadcast
  leaves the list stale until the next change or re-attach (same as `Meta`); accepted.
- Worst-case event size: 64 × 2048 B ≈ 128 KiB, ≤ 2× with JSON escaping, under `MAX_FRAME`.
- Extraction runs regardless of `[claude] latex`: a user can ask for tags in their own
  CLAUDE.md. It costs nothing when no record contains `<latex>`.

### 3. Rendering (client): pure module `src/tui/latex.rs` + a render worker

- Client state: `SessionView` gains `latex_seq: u64`, `latex_srcs: Vec<String>` and
  `latex: Vec<Formula>`. On `Latex { seq, formulas }`, `App` ignores it when `seq <=
  latex_seq`; otherwise it stores `seq` and the sources and returns
  `Effect::RenderLatex { host, id, seq, srcs }`. `App` resets `latex_seq` to 0 and clears the
  lists on `ClaudeSession` / `ClaudeSessionCleared`. `App` never renders.
- `Formula { src, inline: Option<String>, block: Option<Vec<String>>, key_strict: String,
  key_loose: String }`. The keys are the matching keys of §4, computed once with the rendering.
- **Rendering never runs on the UI thread.** `tui/mod.rs` owns one long-lived `std::thread`
  named `latex-render`, fed by a channel of `RenderLatex` effects. It keeps a process-wide cache
  `source → Formula` (LRU, 1024 entries); a cached source is never rendered again, so an event
  whose sources are all cached costs no render work. For each uncached source it:
  1. sanitizes it again (`sanitize`, §2; `\n`/`\t` become one space);
  2. runs the pre-check `latex::too_complex(src) -> bool`: reject when the maximum `{`…`}`
     nesting exceeds `MAX_DEPTH` = 32, or when the total count of recursion-prone tokens
     exceeds `MAX_TOKENS` = 256 — every `\<letters>` command, `\left`, `\begin{`, every `/`, and
     every `(` that directly follows a bareword (`sqrt(`, `frac(`, …);
  3. otherwise spawns a scratch thread (`stack_size(32 << 20)`) that sets the thread-local
     `QUIET_PANIC` flag, runs `term_maths::render` and the one-line walker inside
     `std::panic::catch_unwind`, and sends the result back; the worker waits
     `RENDER_BUDGET` = 150 ms (`recv_timeout`);
  4. on timeout or panic records `inline = None, block = None` (shown raw). A timed-out scratch
     thread is abandoned (it finishes on its own) and counted in an `AtomicUsize` that the thread
     decrements when it ends. While `MAX_ABANDONED` = 2 threads are still running, new uncached
     sources are not rendered: they are returned raw and **not** cached, so a later event
     renders them once capacity frees. A timeout or panic result is cached (never retried).
  The worker posts `Event::LatexRendered { host, id, seq, formulas: Vec<Formula> }` back to the
  event loop; `App` stores it in `SessionView::latex` only if `seq == latex_seq` (a stale
  render is dropped). Until it arrives, the previous `latex` list stays.
- The TUI panic hook returns early (no `restore_terminal`, no print) when the current thread
  has `QUIET_PANIC` set, so a renderer panic never touches the terminal. Release builds have
  overflow checks off, so the set of panicking inputs differs between test and release builds;
  the guard covers both.
- `block`: `term_maths::render(src)` as rows of strings, capped to 200 columns × 40 rows (larger
  → `None`).
- `inline`: if the 2D rendering is exactly one row, that row; otherwise a one-line rendering of
  the `rust-latex-parser` AST (our own walker, modelled on term-maths' `MathRenderer`):
  `\frac{a}{b}` → `a/b`; a numerator is bracketed only when it has a top-level space or `/`, a
  denominator unless it is a number, one symbol (with scripts or a `√`) or already bracketed
  (`n(n + 1)/2`; `(-b ± √(b² - 4ac))/(2a)`; the parser keeps ASCII `-` and spaced operators);
  `\sqrt{x}` → `√x` for an atom, else `√(…)` (`\sqrt[n]{…}` has no root index in this parser and
  renders as it parses); sub/superscripts → Unicode
  sub/superscript characters when every character has one, else `_(…)` / `^(…)`; large operators
  with limits → operator + scripts (`∑ᵢ₌₁ⁿ`, else `∑_(i=1)^(n)`); `\left`/`\right` delimiters →
  the plain delimiter; matrices → `[a b; c d]`, except `vmatrix` → `|a b; c d|` and `Vmatrix` →
  `‖…‖`; `cases` → `{ 1 if x > 0; 0 otherwise }`;
  symbols, Greek, `\mathbb` etc. as term-maths maps them. Output is one line, sanitized, at most
  512 columns (longer → `None`).

### 4. Inline replacement in the pane

- `draw_pane` runs `latex::overlay(buf, area, &view.mirror, &view.latex)` after
  `mirror.render` and before the selection highlight, only when `view.latex` is non-empty. The
  overlay allocates nothing per frame except the rebuilt rows, and skips rows without `<latex>`.
- Span finding (pure, over `Screen::row_chars` of the visible rows, all spans located on the
  original rows): a span starts at `<latex>` and ends at the first `</latex>` on the same row or
  on one of the next `MAX_SPAN_ROWS` = 8 rows. A continuation row contributes its text after its
  leading whitespace, joined with one space. An unclosed or over-long span is left alone. A
  multi-row span may only start on a row that has no other span; otherwise it is left raw.
- Matching a span to a formula, in three passes, newest formula first within each:
  1. strict: `normalize(span_text) == formula.key_strict`, where `key_strict =
     normalize(md_unescape(src))`, `md_unescape` removes a backslash that precedes an ASCII
     punctuation character (CommonMark escapes) and `normalize` removes all whitespace;
  2. loose: the same with `normalize_loose`, which also drops `*`, `_`, `~` and `` ` ``
     (markdown emphasis eaten on screen) — `key_loose`;
  3. html: `normalize_loose` of the span text after also removing, from the *screen side only*,
     the formula's `<…>` runs that are not `<latex>`/`</latex>` (markdown may have removed an
     HTML-looking run): i.e. compare against `key_loose` with those runs removed from the source
     (precomputed as a third key `key_html` on `Formula`).
  No match, or `inline == None` → the span is left raw.
- Rebuilding a row (cells only; no row moves): each row containing spans is rebuilt once, left
  to right: original cells between spans are copied, each matched span becomes its inline text,
  each unmatched span keeps its original cells, and the rest of the row is padded with blanks.
  For a multi-row span, the first row ends with the inline text plus the last row's suffix if it
  fits; the later rows of the span become blank, except that a suffix which did not fit stays on
  the last row after its indentation. If a rebuilt row would be wider than the pane, or the
  inline text alone does not fit after the prefix, the whole row (or span) is left raw — no
  partial replacement. A tag that continues onto rows containing another `<latex>` stays raw
  (those rows are processed on their own).
- Cells are written as grapheme clusters (`unicode-segmentation`): one cluster per cell, width
  from `unicode-width` of the cluster; a width-2 cluster takes its cell plus a spacer (ratatui
  `Buffer::set_string` semantics); a width-0 cluster is dropped; a cluster wider than two
  columns, or one that would straddle the right edge, makes the row stay raw. Copied cells keep their own styles; the inline text gets
  the accent foreground (one constant) on the span's original background.
- Selection, copy and the cursor still use the mirror's cells (unchanged).

### 5. Formulas popup (Alt+u)

- `Action::Formulas` (`name()` = `"formulas"`), bound to Alt+u in `DEFAULT_BINDINGS` (before
  Quit), also in `PLAIN_ACTIONS` (which grows to 5). `Modal::Formulas(FormulasView)` is
  read-only: it `yields_to_actions`, and `opened_by(Formulas)` makes Alt+u close it. In terminal
  tabs Alt+u takes precedence over readline's `upcase-word` (as Alt+l/Alt+c do today); the README
  keys table notes it.
- The popup is centered, up to 100 columns × 90% height, titled `formulas`. Entries run oldest to
  newest, and it opens scrolled to the newest. Each entry shows its 2D block (or the inline form,
  or the raw source when both are `None`) followed by its source, dimmed and truncated to one
  line, with a blank line between entries. All text goes through `sanitize` and the same
  cluster-based cell writing as §4.
- Keys: Down/j, Up/k, PageDown/Space, PageUp, Home/g, End/G scroll; Esc/q close; Ctrl/Alt chords
  are ignored (the `StatsView` pattern). An empty list shows "No formulas yet" plus the hint
  from §1 (`[claude] latex = true` in config.toml on the session's host, then `claudio daemon
  restart`).
- The help popup lists the binding automatically (`help_entries`).

### Alternatives considered

| Option | Why not |
|---|---|
| Pixel images (sixel/kitty) inline | gnome-terminal/VTE has neither; images can't fit claude's row layout; an image engine (typst+mitex) is heavy next to an 11 MB static binary. Owner deferred it. |
| Render from the on-screen text | Markdown strips `\{`, `\\`, `\,`, `\_`…; formulas would render wrong. |
| `--append-system-prompt` for the instruction | Changes the system prompt (classification risk); hook context is verified to work. |
| A Claude Code skill file | Model-invoked on relevance (not guaranteed), and it writes into the user's `~/.claude`. |
| Client-side setting sent in `SpawnSpec` | Owner chose the per-host `[claude]` setting; it needs no protocol field and survives journal respawns. |
| Multi-row inline overlay | claudio can't add rows to claude's screen; it would cover the next lines. |
| `tui-math` crate | Pins ratatui 0.29 / crossterm 0.28 (duplicates), fails on `cases`/`pmatrix`. |
| Incremental `Latex` events (only new formulas) | Needs ordering/dedup state on both sides; the bounded whole list with `seq` is idempotent and replays trivially. |
| Render in `App::on_event` / on the UI thread | One hostile formula would freeze every tab for up to a minute (spec audit F1). |
| Render in a child process | Strongest isolation, but costs a process per formula and a new subcommand; a budgeted thread with an abandon cap bounds the same risk. |

## Compatibility and rollout

| Writer → reader | Behaviour |
|---|---|
| New daemon → old client | `SessionEvent::Latex` decodes as `Unknown` and is ignored; tags show raw (only if `[claude] latex` is on). Supported. |
| Old daemon → new client | No `Latex` events: no inline rendering, popup says "No formulas yet". Old daemons never inject the instruction either. Freshness replaces an idle older daemon anyway. Supported. |
| New config → old binary | `[claude] latex` is an unknown key; the old binary ignores it (no `deny_unknown_fields`). Supported. |
| Hook command → binary | `__context` is run from the daemon's own binary path. A binary downgraded under a live session would lack `__context`; the hook's 5 s timeout bounds it, and claude ignores a failing SessionStart hook. Accepted residual. |
| Journal | Unchanged: the `--settings` injection is never journaled; a respawn uses the daemon's current config. `SessionStart` matcher `*` covers startup/resume/clear/compact, so the instruction is re-injected after `/clear` and auto-compact. |

No version bump of `PROTO`. Rollout: one release; the daemon updates through the normal freshness
path. Rollback: ship the previous release; nothing persistent changes.

## Failure modes and security

Threat model: assistant text is attacker-influenced (prompt injection through files or tool
output). The attacker controls the formula text, its size and count. The following must hold:
no terminal escape sequence or text-reordering character reaches the user's terminal; the UI
thread never blocks on rendering; no panic or stack overflow reaches the client or daemon; memory,
threads and frame sizes stay bounded.
- `sanitize` (Cc/Cf/Co/Cn/Cs) at extraction (daemon) and again before rendering and before any
  cell is written (client).
- Limits: 2048 bytes per formula, 64 formulas per session, `MAX_DEPTH` 32, `MAX_TOKENS` 256,
  `RENDER_BUDGET` 150 ms, `MAX_ABANDONED` 2 render threads, cache 1024 entries, render caps
  (200×40 block, 512-column inline).
- Rendering is off the UI thread; a hang costs at most `MAX_ABANDONED` background threads and
  never blocks the event loop.
- `catch_unwind` + `QUIET_PANIC` around every `term-maths`/parser call; the token pre-check and
  the 32 MiB scratch stack guard against stack overflow, which `catch_unwind` cannot catch.
- The `__context` subcommand prints a constant; formulas never appear in any argv, log or state
  file.
- A transcript that is rewritten or shrinks resets the list (existing cursor logic).
- A stale list (refresh lag while claude streams) leaves new spans raw until the next refresh;
  a Stop hook refreshes with no gap.

## Acceptance criteria

- [ ] AC1: `[claude] latex` parses (default false); `hooks::settings(…, latex)` adds exactly one
  `__context latex` command to `SessionStart` iff `latex` is true, and nothing else changes
  (unit tests in config.rs and hooks.rs).
- [ ] AC2: `claudio __context latex` prints one valid JSON line with
  `hookSpecificOutput.hookEventName == "SessionStart"` and the `<latex>` instruction, and exits
  0; unknown arguments print nothing (unit test of the function main.rs dispatches to).
- [ ] AC3: `extract_latex` honours code fences, inline code, multi-line formulas, string and
  array `content`, non-text blocks and sidechain records skipped, `sanitize` (a Cc, a bidi
  override and a zero-width space are removed), the 2048-byte and 64-formula limits;
  `TranscriptCursor` exposes the list and resets it on a rewrite (unit tests in projects.rs).
- [ ] AC4: the daemon broadcasts `SessionEvent::Latex` only when the list changes, with `seq`
  increasing across conversations (pure `latex_event` test); a client attaching later receives
  the latest `(seq, list)` right after `Attached` and the snapshot (unit test through
  `Subscriber::attach`, plus AC10's manager test, whose second client can only get the formula
  through the replay); `App::on_event` given seq 2 then seq 1 keeps seq 2's list, and a
  `ClaudeSession` event resets it (app test).
- [ ] AC5: proto: `Latex` round-trips; an old client decodes it as `Unknown` (test in proto.rs,
  `Older`-enum pattern).
- [ ] AC6: `latex.rs` renders a table of formulas to the expected one-line strings (at least:
  `E = mc^2` → `E = mc²`, the quadratic formula, `\sum_{i=1}^{n} i = \frac{n(n+1)}{2}`,
  `\int_0^1 x^2\,dx`, `a \neq 0`, a `pmatrix`, a `cases`). Through the production guard entry
  point (worker + budget), a hostile corpus — unbalanced braces, 5000-deep nesting, braceless
  `\frac `/`\hat `/`\not` chains, `x/` × 1000, `sqrt(` × 400, escape sequences, bidi and
  zero-width characters, `\binom{}{}`, 10 KB input — yields `None` or a sanitized string, never
  panics, never aborts, and the caller is never blocked longer than 2 × `RENDER_BUDGET` per
  formula.
- [ ] AC7: a panic raised inside the render guard does not run the TUI panic hook's
  terminal restore (unit test in `tui/mod.rs`).
- [ ] AC8: the overlay replaces a single-row span, two spans on one row, and a two-row span
  (two-space continuation indent) using formulas whose screen text lost backslashes, emphasis
  characters or an HTML-looking run; unmatched, unclosed, too-wide and multi-row-after-span
  cases stay raw; wide characters in the suffix keep their two cells; no other row changes
  (unit tests on a `Screen` fed bytes).
- [ ] AC9: Alt+u opens and closes the popup, it scrolls, it is in plain mode's keymap and help,
  and there is no key collision (keymap and app tests).
- [ ] AC10: manager integration test: a fake claude prints a line with `<latex>…</latex>` and
  writes the matching assistant record to its transcript; the pane shows the rendered form and
  Alt+u shows the formula (tests/manager.rs, single-threaded).
- [ ] AC11: manual check by the controller with real claude through claudio on a local session
  and an SSH session (`z6`): with `[claude] latex = true`, a math question renders inline and in
  the popup; with it off, no instruction is injected.
- [ ] AC12: docs: README (config key and restart note, Alt+u and the readline note, what is
  rendered), docs/manager-design.md (hook, event with `seq`, extraction, render worker),
  CLAUDE.md architecture map (`latex.rs`, render worker, `__context`), config header comment.
  Release binary size delta reported.

## Constraints

- `app.rs` stays pure; rendering and matching are pure functions; I/O and threads stay in
  `tui/mod.rs`.
- Protocol additions are backward compatible (new `SessionEvent` kind only).
- Manager keys use Alt and never collide with claude's bindings.
- No secrets in argv, logs or state; formulas are not logged.
- Tests assert on the rendered screen model, never raw bytes; manager tests run single-threaded
  and alone on the machine (the PTY harness is load-sensitive).
- Builds on Linux and macOS; static musl build keeps working (pure-Rust deps only).
- Run claudio tests under `umask 022`; no repo-wide `cargo fmt` (format only touched files).
- Commits: conventional `type(scope): summary`; **no AI co-author trailer** (owner's global
  CLAUDE.md).

## Open questions

None.
