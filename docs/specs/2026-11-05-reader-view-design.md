# Typeset reader view: design

Status: draft   Class: architectural
Owner decisions so far (2026-11-05):
- Build option B: a reader view where claudio renders claude's recent answers itself, with
  formulas typeset in 2D ("like real mathematics"), in every terminal.
- Make the 2D math "a bit more condensed" than term-maths' layout.
Controller decisions (open to owner objection at spec approval):
- Replace term-maths with our own compact 2D layout engine over the same parser
  (`rust-latex-parser`); the pane's one-line form and the Alt+u view both use it.
- Alt+u opens the reader view; the formulas-only list stays as a mode inside it (Tab).
- Markdown via `pulldown-cmark` (pure Rust, MIT/Apache).

## Problem

v0.4.0 shows formulas inside claude's own screen, where each tag has one row, so formulas
needing scripts that have no Unicode form fall back to `_(…)`:
`S_(i,f) = ∑_(s:Pₛ∉X) s_(Pₛ,i,f) + G_(f)(i)`. The Alt+u popup shows the 2D form, but
term-maths lays it out loosely and with bugs (measured 2026-11-05, term-maths 1.0.0):

```
S_{i,f} = \sum_{s : P_s \notin X} s_{P_s,i,f} + G_f(i)      (2 rows, 33 cols)
S    =     ∑     s       + G (i)
 i,f   s : Pₛ ∉X  Pₛ,i,f    f

x = \frac{-b \pm \sqrt{b^2 - 4ac}}{2a}                      (4 rows: a row for the √ bar,
           ────────                                          drawn two columns off)
     -b ± √b² - 4ac
x = ────────────────
           2a

p(x) = \frac{1}{\sqrt{2\pi\sigma^2}} e^{-\frac{(x-\mu)^2}{2\sigma^2}}   (6 rows: the exponent's
                  (x - μ)²                                               fraction is stacked
                -──────────                                              full size)
          1         2σ²
p(x) = ─────── e
         ────
        √2πσ²
```

Known term-maths bugs besides: `\sin x` → `sinx`, `cases` braces wrong.

## Goals

1. Alt+u opens a full-pane **reader view** of the session's most recent answer, rendered by
   claudio from the transcript's exact text: markdown (paragraphs, headings, emphasis, inline
   code, code blocks, lists, block quotes, tables, rules, links as text) with every `<latex>`
   formula typeset. Older answers (up to 10) are one key away.
2. **Display formulas** (a paragraph that is only one `<latex>` tag) are typeset in 2D, as
   many rows as they need. **Inline formulas** are typeset too: scripts raised or lowered, the
   text line growing to fit (TeX's line boxes), so `S_{i,f}` reads as S with a lowered `i,f`.
3. The 2D layout is **condensed**, following TeX's styles: display style only at the top of a
   display formula; everything inside scripts, limits, exponents and inline formulas in text
   style (flat `a/b` fractions, no spaces around relations inside scripts, no padding).
   Targets (AC6): the quadratic formula in 3 rows, the Gaussian above in at most 4, the
   example `S_{i,f}` formula in at most 3 and narrower than term-maths' 33 columns.
4. The term-maths bugs go away (√ without an overline row, `\sin x`, `cases`), and so does the
   term-maths dependency.
5. Works locally, over SSH and in `--plain`; old daemons fail safe.
6. The v0.4.0 security properties hold for the new text path (answers are attacker-influenced).

## Non-goals

- Pixel images (sixel/kitty) — option C, later.
- Changing what the pane shows (claude's own screen, v0.4.0 overlay), except that the
  overlay's one-line strings come from the new engine.
- Syntax highlighting in code blocks; clickable links; editing or copying from the reader view
  (copy stays on the pane).
- Answers older than the last 10, or than the transcript tail read (below).
- `-p` / `--api`.

## Current state

- `<latex>` pipeline (v0.4.0): `claude::latex::{sanitize, extract_latex}`, daemon
  `SessionEvent::Latex { seq, formulas }`, client render worker in `tui/mod.rs`
  (`render_formula`: `too_complex` pre-check, scratch thread with 32 MiB stack, 150 ms budget,
  `MAX_ABANDONED` 2, LRU cache 1024, `QUIET_PANIC`), pure `tui/latex.rs` (`render` = term-maths
  block + one-line walk of the parse tree; `overlay`), `tui/formulas.rs` + `ui/formulas.rs`
  (Alt+u popup, `Modal::Formulas`).
- Request ops: `Msg::ListClaudeSessions { cwd }` → `Msg::ClaudeSessions` (src/proto.rs:117,
  served by `list_claude_sessions` in src/daemon/server.rs:678 via `spawn_blocking`). An old
  daemon answers an unknown op with `Error "unsupported op…"`; `App::older_daemon_notice`
  turns that into "run `claudio daemon restart`".
- Transcripts: `claude::projects::transcript_path(cwd, sid)` (resolves symlinks since v0.4.0);
  `is_real_prompt` (src/claude/projects.rs:483) tells real user prompts from tool results.
  One record per content block; sidechain records belong to subagents.

## Design

### 1. Protocol: `Answers`

- New request op `Msg::Answers { id: SessionId }`; reply
  `Msg::AnswersReply { id, answers: Vec<Answer> }`, with
  `Answer { prompt: String, text: String }`, oldest first. New fields/ops follow the
  backward-compatibility rules; no `PROTO` bump.
- The daemon serves it like `ListClaudeSessions`: off the client loop, the transcript read in
  `spawn_blocking`. It uses the session's cwd and current claude session id; no conversation
  yet → empty list.
- Reading (pure function over bytes, `claude::projects::recent_answers(bytes) -> Vec<Answer>`):
  the last `ANSWERS_TAIL` = 4 MiB of the transcript (skipping the first, partial line when not
  at the start). An answer is the user's real prompt (`is_real_prompt`, not `isMeta`, not
  sidechain) and the `text` blocks of the non-sidechain assistant records after it, joined
  with blank lines, in order. Keep the last `MAX_ANSWERS` = 10. Caps: prompt 2 KiB, text
  64 KiB (cut on a char boundary, ` …` appended), whole reply 768 KiB (oldest answers dropped
  first). Everything passes `sanitize` (control, format, private-use, noncharacters) before it
  leaves the daemon.
- Old daemon → `Error "unsupported op…"` → the view shows the usual "run `claudio daemon
  restart`" notice instead of content.

### 2. Rendering pipeline (client)

- Opening the view (Alt+u) sends `Answers { id }`; the reply becomes
  `Effect::RenderAnswers { host, id, answers }`. The `latex-render` worker turns each answer
  into a `Doc` (width-independent): markdown blocks, with each `<latex>` span replaced before
  markdown parsing by a placeholder (a private-use char inserted *after* `sanitize`, so input
  can never forge one) and each formula typeset (inline: text style; display: display style)
  through the existing guard (`render_formula`: pre-check, scratch thread, budget, cache).
  Results come back as `HostEvent::AnswersRendered`; `App` stays pure.
- Layout (wrapping a `Doc` to the view's width) is a pure function, cached per
  (answer, width) in the view state; drawing only slices visible lines.
- The worker also keeps rendering the v0.4.0 `Latex` lists for the pane overlay (unchanged).

### 3. Compact 2D layout engine (`tui/typeset.rs`, pure)

Replaces term-maths. A box model over the `rust-latex-parser` tree: `Block { rows:
Vec<Vec<Cell-text>>, width, baseline }`, combinators for horizontal concatenation on the
baseline, fractions, scripts, big operators, roots, delimiters, matrices, cases, accents.
Rules:
- **Styles.** `Display` only at the top of a display formula; `Text` for inline formulas and
  everything inside scripts, limits, exponents, roots' indices and inside `Text`-style
  fractions. In `Text`: fractions are flat (`a/b`, parenthesised when not atomic, the v0.4.0
  one-line rule); big-operator limits go as scripts to the right; no spaces around
  relations/binary operators inside scripts.
- **Scripts.** All characters have Unicode sub/superscript forms → those glyphs, one row.
  Otherwise the script is a raised (superscript) or lowered (subscript) row of its own,
  adjacent to the base, no padding. Both scripts stack (super above, sub below).
- **Fractions (Display).** Numerator over a bar over denominator; the bar exactly as wide as
  the wider of the two; both centred; the bar on the baseline. No side padding.
- **Big operators (Display).** ∑ ∏ ∐ ⋃ ⋂ ⨁ and `\lim`: limits centred above/below in Text
  style. ∫ ∮: limits as scripts to the right (TeX's default).
- **Roots.** `√` followed by the radicand, parenthesised unless atomic; no overline row.
- **Spacing.** One space around relations and binary operators at the top level of Display
  and Text style (not in scripts); operator names (`\sin`, `\log`, `\max`, …) keep a space
  before their argument; `\,` `\;` `\quad` map to 1/1/2 spaces.
- **Matrices, cases.** Columns separated by two spaces, brackets drawn with ⎡⎢⎣/⎛⎜⎝/│/‖ for
  the matrix kinds (one-row matrices use plain brackets); `cases` with ⎧⎨⎩ (⎰⎱ for two rows)
  and `if`-free columns.
- **One-line form** (pane overlay, fallback): the v0.4.0 walk, unchanged in output, moves
  into this module.
- **Limits.** The same caps and guard as v0.4.0 (`too_complex`, 200×40 block, 512-column
  line), and the engine must not recurse deeper than the parse tree (bounded by the
  pre-check).

### 4. Reader view (`tui/reader.rs` + `ui/reader.rs`)

- Alt+u (`Action::Formulas` renamed `Action::Reader`, key name `reader`; `formulas` kept as an
  accepted alias in `[keys]` so existing configs keep working) opens `Modal::Reader`
  covering the pane area (tab bar and status bar stay). Read-only; yields to actions; Alt+u
  closes it. Also in `--plain`.
- Header row: `answer k/N · <prompt, one line, dimmed>`. Body: the laid-out `Doc`.
- Keys: ↑/↓ j/k scroll, PgUp/PgDn/Space, Home/g, End/G; ←/→ or h/l previous/next answer
  (opens on the newest); Tab toggles the formulas-only list (the v0.4.0 popup content, now
  with the new layout); Esc/q close.
- Markdown mapping: headings bold (level 1 also underlined); emphasis italic/bold; inline code
  and code blocks dimmed, code blocks indented two columns and never wrapped (cut at the edge);
  lists with `•`/`1.` and hanging indents; block quotes with a `│ ` gutter; tables as aligned
  columns when they fit, else rows as `a │ b │ c` lines; rules as `─`; links as their text;
  HTML other than the formula placeholders shown as text.
- Formulas: inline formulas are unbreakable boxes in the text flow; a line's height is its
  tallest box; baselines align. A display formula is its own block, indented two columns; if
  wider than the view, it falls back to its wrapped one-line form. A formula the guard
  rejected shows its source dimmed.
- While the reply or the render is pending: "Loading…"; empty list: "No answers yet".

### Alternatives considered

| Option | Why not |
|---|---|
| Patch term-maths (fork) | Its box model has no text style; condensing means rewriting most of layout.rs anyway, and we'd carry its bugs. Our engine reuses the one-line walk we own. |
| Push answers in `Latex`-style events | Up to 768 KiB per change to every client; a request on open is cheaper and simpler. |
| Incremental answers in `TranscriptCursor` | Per-session memory for a rarely used view; a tail read on demand is stateless. |
| Hand-rolled markdown | Claude uses tables, nested lists, code fences; pulldown-cmark is the standard, pure-Rust parser. |
| Keep the formulas popup as Alt+u and add another key | Two views for the same need; the reader shows formulas in context and keeps the list (Tab). |

## Compatibility and rollout

| Writer → reader | Behaviour |
|---|---|
| New client → old daemon (`Answers`) | `Error "unsupported op…"` → "run `claudio daemon restart`" in the view. Supported (fails safe). |
| Old client → new daemon | Never sends `Answers`. Unaffected. |
| `[keys] formulas = …` in an existing config | Still accepted (alias of `reader`). |
| Pane overlay | Same one-line strings (the walk moves, unchanged); covered by the v0.4.0 tests. |

Rollout: one release (v0.5.0). Rollback: previous release; nothing persistent changes.

## Failure modes and security

Threat model as v0.4.0: answer text and formulas are attacker-influenced (prompt injection).
Must hold: no escape/format characters reach the terminal; no UI-thread blocking; no panic or
stack overflow; bounded memory, threads and frames.
- `sanitize` on the daemon before sending, and again on every string before it becomes a cell
  (the `text_cells` path); placeholders inserted after sanitizing.
- Markdown parsing and formula typesetting happen on the worker, formulas through the guarded
  scratch threads; pulldown-cmark is linear but runs on the worker too.
- Caps: 10 answers, 2 KiB prompt, 64 KiB text, 768 KiB reply, 4 MiB tail read; formula caps
  as v0.4.0; layout cached per width; drawing touches only visible lines.
- Deeply nested markdown (lists/quotes): indentation capped (e.g. 8 levels), deeper content
  flattened, so layout recursion is bounded.

## Acceptance criteria

- [ ] AC1: proto — `Answers`/`AnswersReply` round-trip; an old daemon's `Unknown` op path
  yields the restart notice (proto + app tests).
- [ ] AC2: `recent_answers` groups prompts and text blocks correctly (string/array content,
  tool results and `isMeta` not prompts, sidechains skipped, thinking/tool_use skipped), keeps
  the last 10, applies every cap and `sanitize`, handles a tail cut mid-line (unit tests).
- [ ] AC3: daemon serves `Answers` off the client loop via `spawn_blocking`, empty when no
  conversation (daemon unit test of the handler with a temp transcript, or the manager test).
- [ ] AC4: typeset engine golden tests: at least the three formulas in "Problem" plus
  `\sum_{k=1}^{n} k = \frac{n(n+1)}{2}`, a `pmatrix`, a `vmatrix`, a `cases`, `\sin x`,
  nested scripts, an integral with limits — exact row strings in the test.
- [ ] AC5: one-line outputs identical to v0.4.0 (existing table test passes unchanged).
- [ ] AC6: condensed targets: quadratic 3 rows; Gaussian ≤ 4 rows; the `S_{i,f}` formula ≤ 3
  rows and < 33 columns; no row is entirely blank; no leading/trailing blank columns.
- [ ] AC7: hostile corpus (v0.4.0's, through `render_formula`) still bounded and sanitized
  with the new engine; hostile markdown (deep nesting, 64 KiB single line, huge tables, PUA
  placeholders in input) renders bounded and sanitized (unit tests).
- [ ] AC8: reader layout tests: wrapping, line boxes with inline formulas (baseline aligned,
  height = tallest), display formula fallback when too wide, tables, lists, code blocks,
  headings (pure tests on `Doc` → lines).
- [ ] AC9: keys and modal: Alt+u opens/closes, answers navigation, Tab list mode, scroll,
  plain mode, `formulas` alias in `[keys]`, no collision (app/keymap tests).
- [ ] AC10: manager test: a fake claude writes a transcript with a prompt and an answer with
  text, a list, an inline and a display formula; Alt+u shows the answer with the formula in 2D.
- [ ] AC11: manual check with real claude, isolated daemon (as v0.4.0): a math answer reads
  well in the reader; older answers navigable; old daemon shows the restart notice.
- [ ] AC12: docs (README keys/reader section, manager-design, CLAUDE.md map); term-maths
  removed from Cargo.toml/lock; binary size delta reported.

## Constraints

- `app.rs` pure; rendering/parsing on the worker; I/O and threads only in `tui/mod.rs`.
- Protocol additions backward compatible (new op + reply only).
- Alt keys only; no collisions; tests assert on the rendered screen model.
- No secrets or answer text in argv, logs or state files.
- Linux/macOS/static musl (pure-Rust deps only).
- `umask 022` for tests; no repo-wide `cargo fmt`; manager suite alone and single-threaded.
- Commits: conventional `type(scope): summary`, no AI co-author trailer or attribution.

## Open questions

None.
