# Typeset reader view: design

Status: audited (rev 2: spec audit folded in as Appendix A)   Class: architectural
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

## Appendix A: accepted spec-audit amendments (normative)

The spec audit of 2026-11-05 (findings F1–F16 below, copied verbatim) is accepted in full. Each finding's **Change** text is part of this spec and wins over the sections above where they differ. F10: the first option (the action keeps its config name `formulas`; only its help label changes; no alias). F14/F3: AC6 reads "≤ 30 columns". The golden rows in F3/F4 are the exact AC4/AC6 targets.

### F1 [Critical] The 768 KiB reply cap is on raw text; `MAX_FRAME` is on the encoded frame, and an oversize reply drops the client's connection
Where: spec §Design 1 ("whole reply 768 KiB"); src/proto.rs:27, src/proto.rs:804-811, src/daemon/server.rs:379-385
Problem: `MAX_FRAME` is 1 MiB of *encoded* frame. serde_json escapes `\` (every LaTeX command), `"` and `\n` to two bytes each, so 768 KiB of raw answer text can encode to up to 1.5 MiB. `write_frame` then returns `InvalidData` and `write_loop` breaks, which kills the writer task: the client is disconnected on open of the reader view, with no reply at all (not even the Error the spec promises). `list_dir` already caps on estimated encoded size for this reason (server.rs:650-652).
Evidence: proto.rs:806 `if bytes.len() - 4 > MAX_FRAME { return Err(...) }`; server.rs:381-383 `if let Err(e) = proto::write_frame(...) { ...; break; }`; a 64 KiB answer made of `\frac…` is ~1.3× larger encoded.
Change: replace the caps sentence in §Design 1 with:
"Caps: prompt 2 KiB, text 64 KiB (each cut on a char boundary, ` …` appended). The whole reply is capped on its **encoded** size: the daemon serialises `AnswersReply` and, while `serde_json::to_vec(&msg).len() > ANSWERS_REPLY_MAX` (= 512 KiB), drops the oldest answer and re-serialises. The reply therefore always fits `proto::MAX_FRAME` (1 MiB) with room for the envelope; a unit test asserts it for 10 answers of 64 KiB made entirely of `\\`."

### F2 [Important] `is_real_prompt` accepts slash-command records, compaction summaries and the daemon's own caveats; API-error assistant records count as answer text
Where: spec §Design 1 ("An answer is the user's real prompt (`is_real_prompt`, not `isMeta`, not sidechain)"), §AC2; src/claude/projects.rs:483-521
Problem: `is_real_prompt` is structural: any `type:"user"` record with non-empty string content is a prompt. In a real transcript, `<command-name>/model</command-name>…` records (51 of 52 have no `isMeta`), `<local-command-stdout>` records (22 of 27 without `isMeta`), and the compaction summary (`isCompactSummary:true`, `isVisibleInTranscriptOnly:true`, string content starting "This session is being continued from a previous conversation…", no `isMeta`) all pass. The reader would show "answers" whose prompt is `<command-name>/model…` with empty text, and after every compaction an answer whose prompt is the 2 KiB-truncated summary. Assistant records with `isApiErrorMessage:true` (`"model":"<synthetic>"`) carry `text` "API Error: stream error: …" and would be joined into the answer.
Evidence: `~/.claude/projects/-volumes-repos-claude-proxy/530ea643-….jsonl`: `grep -c command-name` 52 (1 with isMeta); `isCompactSummary` 18 (0 with isMeta); `isApiErrorMessage` 5, content `[{"type":"text","text":"API Error: stream error: stream disconnected before completion: …"}]`. projects.rs:493-496 string content → `!s.trim().is_empty()`.
Change: replace the "An answer is…" sentence with:
"An answer starts at a *real prompt*: a non-sidechain `type:"user"` record with `is_real_prompt`, without `isMeta`, `isCompactSummary` or `isVisibleInTranscriptOnly`, and whose string content does not start with `<command-name>`, `<command-message>`, `<local-command-stdout>`, `<local-command-caveat>` or `<system-reminder>` (Claude Code's own records). Its `text` is the `text` blocks of the following non-sidechain `type:"assistant"` records without `isApiErrorMessage` (tool results and other non-prompt user records in between do not end the answer), joined with blank lines, in order. An answer whose text is empty after this is dropped. A compaction boundary (`type:"system"`, `subtype:"compact_boundary"`) ends the current answer. `is_real_prompt` becomes `pub(crate)`." Add to AC2: "slash-command records, compaction summaries and API-error records are neither prompts nor text".

### F3 [Important] The layout rules leave choices two implementers would make differently; the golden outputs are not derivable from the spec as written
Where: spec §Design 3 (Styles, Scripts, Fractions, Big operators, Spacing), §AC4, §AC6
Problem: the following are undefined, and each changes the AC4 row strings:
(a) the style of a Display fraction's numerator and denominator (TeX: text style; the spec lists what is Text and never mentions them);
(b) whether big-operator limits follow the script rule "no spaces around relations" — the Styles bullet puts limits in Text style, and the Spacing bullet gives Text style spaces, so `k = 1` under the ∑ unless limits count as scripts;
(c) what "top level" means: inside parentheses (`n(n + 1)`), inside `\left…\right`, inside a Display fraction's numerator (`-b ± √(…)`);
(d) centring when the slack is odd (bar of width 16 over `2a` is exact, but `n(n + 1)` over `2` is not);
(e) a `SupSub` where both scripts have glyph forms: one row in which order (v0.4.0's walk writes sub then sup: `zᵢ²`; term-maths writes `z²ᵢ`);
(f) a `SupSub` where exactly one script has glyph forms (`\int_0^\infty`: `₀` exists, `∞` has no superscript);
(g) source whitespace: the parser emits `Text(" ")` for every source space and `Space(4.0)` around operators (parser.rs:706-708, 720-727), so ` = ` carries up to five space nodes; the collapse rule (v0.4.0's `seq`, tui/latex.rs:273-295) must be stated;
(h) the baseline of a matrix/cases block with an even number of rows.
Evidence: scratch dump: `\sum_{k=1}^{n} k = …` → `BigOp{lower: Seq([Text("k"), Seq([Space(4.0), Text("="), Space(4.0)]), Text("1")])}`; `x^{a^b} + y_{i_j} + z_{i}^{2}` → `SupSub(Text("z"), Text("2"), Text("i"))`.
Change: add to §Design 3, after "Rules:":
"- **Precise style rule.** A node is in `Display` iff the formula is a display formula and the node is not inside a script, a big-operator limit, a `\lim` limit, a fraction (either part) or a root. Everything else is `Text`. Delimiters, parentheses and `\text` do not change the style.
- **Scripts and limits share one rule:** whitespace (source spaces and `Space` nodes) is dropped, relations and binary operators get no spaces, fractions are flat. `SupSub`: when both scripts have glyph forms they go on the base's row, subscript first (`zᵢ²`, as the v0.4.0 walk); when either lacks them, both are rows (super above, sub below), left-aligned with each other, so `\int_0^\infty` puts `∞` above `0`.
- **Centring.** When a box is centred in a wider one and the slack is odd, the extra column goes to the right (the content sits one column left of centre).
- **Whitespace.** Outside scripts, a run of source spaces and `Space` nodes becomes one space; `Space(w)` with `w < 2` alone becomes nothing, `w ≥ 12` (`\quad`, `\qquad`) two spaces; a run never starts or ends a box. A relation or binary operator at any nesting that is not a script (inside parentheses, delimiters, a Display fraction's parts) has one space each side.
- **Baselines.** A fraction's baseline is its bar; a big operator's is its symbol row; a matrix, cases or delimited block of n rows has its baseline at row ⌊(n−1)/2⌋ (top-biased); a text run's is its row.
Golden outputs (AC4), exact rows, no trailing blanks:
```
x = \frac{-b \pm \sqrt{b^2 - 4ac}}{2a}
    -b ± √(b² - 4ac)
x = ────────────────
           2a

p(x) = \frac{1}{\sqrt{2\pi\sigma^2}} e^{-\frac{(x-\mu)^2}{2\sigma^2}}
          1     -(x-μ)²/(2σ²)
p(x) = ─────── e
       √(2πσ²)

S_{i,f} = \sum_{s : P_s \notin X} s_{P_s,i,f} + G_f(i)          (29 cols)
S    =   ∑    s       + G (i)
 i,f   s:Pₛ∉X  Pₛ,i,f    f

\sum_{k=1}^{n} k = \frac{n(n+1)}{2}
 n      n(n + 1)
 ∑ k = ────────
k=1        2

\begin{pmatrix} a & b \\ c & d \end{pmatrix}
⎛a  b⎞
⎝c  d⎠

\begin{vmatrix} a & b \\ c & d \end{vmatrix}
│a  b│
│c  d│

f(x) = \begin{cases} 1 & x > 0 \\ 0 & \text{otherwise} \end{cases}
f(x) = ⎰1  x > 0
       ⎱0  otherwise

\sin x                     →  sin x
\sin(x) + \log_2 n         →  sin(x) + log₂ n

x^{a^b} + y_{i_j} + z_{i}^{2}
 aᵇ
x   + y   + zᵢ²
       iⱼ

\int_0^\infty e^{-x^2} \, dx = \frac{\sqrt{\pi}}{2}
 ∞  -x²      √π
∫ e    dx = ──
 0           2

\lim_{x \to 0} \frac{\sin x}{x} = 1
    sin x
lim ───── = 1
x→0   x
```"
And in AC6 replace "< 33 columns" with "≤ 30 columns" (term-maths measures 32 in the scratch run; the golden is 29).

### F4 [Important] Rules name things the parser does not represent: `\sqrt[n]`, `\limits`, `aligned`/`align`, and the space after an operator name
Where: spec §Design 3 ("roots' indices", "∫ ∮: limits as scripts to the right (TeX's default)", "operator names keep a space before their argument"); rust-latex-parser 0.1.1 parser.rs:42-43, 363, 582-584, 1040-1043, 1256-1275
Problem: (a) `\sqrt[3]{x}` parses as `Seq([Sqrt(Text("[")), Text("3"), Text("]"), Text("x")])` — there is no index in the AST, so "roots' indices" cannot be laid out; term-maths prints `√[3]x`. (b) `\limits`/`\nolimits`/`\displaystyle` are consumed as no-ops, so `\int\limits_a^b` and `\int_a^b` are the same tree: "TeX's default" is the only behaviour possible. (c) `\begin{aligned}`, `align*`, `array`, `gather` fall into the unknown-environment path and come back as literal text (`\begin{aligned} a & = b \\c & = d \end{aligned}`); claude uses `aligned` for multi-line derivations, so a math answer "reading well" (AC11) depends on it. (d) `\sin x` parses as `Seq([Limit{sin}, Text("x")])` with no space node between: the space must be synthesised by the engine from the `Limit` node, and `\sin(x)` must not get one.
Evidence: scratch dump of `\sqrt[3]{x}`, `\int\limits_a^b f`, `\begin{aligned} a &= b \\ c &= d \end{aligned}`, `\sin x` (see above); parser.rs:21 "commands with no visible output (`\displaystyle`, `\limits`".
Change: in §Design 3 replace "roots' indices" with "roots" and add under Roots: "`\sqrt[n]{x}`: the parser has no index node (it yields `√[n]x`); before parsing, the engine rewrites `\sqrt[<idx>]{<body>}` to `\sqrt{<body>}` and prefixes the root with `<idx>` as a raised script (`ⁿ√x`, or a raised row). This is the only source rewrite." Under Big operators add: "`\limits`/`\nolimits` are dropped by the parser and cannot be honoured." Under Spacing: "After a `Limit` node (an operator name) one space is inserted unless the next node starts with `(`, `[` or `{`, or is a script of the name." Add a bullet: "**Environments the parser does not know** (`aligned`, `align`, `align*`, `gather`, `array`): before parsing, the engine splits the body on `\\` and drops `&`; each row is parsed and laid out on its own and the rows are stacked, left-aligned, as one block. Any other unknown environment is shown as its dimmed source (the guard's rejected-formula path)." Add `\begin{aligned} a &= b \\ c &= d \end{aligned}` to the AC4 list with golden rows `a = b` / `c = d`.

### F5 [Important] Inline formula boxes have no height cap, formulas per answer have no count cap, and one `Answers` job can hold the pane's formula lists for seconds
Where: spec §Design 4 ("a line's height is its tallest box"), §Design 2, §Failure modes; src/tui/mod.rs:88-93, 510-523
Problem: (a) an inline formula renders in Text style but nested scripts are rows, so a hostile `x_{a_{b_{c…}}}` 32 deep inside the 2048-byte cap makes a 33-row line box; `BLOCK_MAX_ROWS` (40) is the only bound. Every text line containing one such formula becomes 40 rows. (b) 64 KiB of text can hold ~4000 `<latex>` tags; each goes through `render_formula` (a scratch thread each, 150 ms budget when hostile); the PUA placeholder range U+E000–U+F8FF is 6400 code points, with no stated behaviour past it. (c) the `latex-render` worker is one FIFO thread (mod.rs:510-523); an `Answers` job of 10 answers × many formulas runs before any later `Latex` list, so the pane overlay stops updating while it runs, and the reader shows "Loading…" behind a slow `Latex` job (64 × 150 ms worst case).
Evidence: mod.rs:91 `MAX_ABANDONED = 2` only stops *starting* renders while two are over budget; a benign-but-numerous list is not bounded; `spawn_latex_worker` has one `for job in queue` loop.
Change: add to §Design 4 Formulas: "An inline box is at most 3 rows (one above and one below its baseline); a formula whose Text-style layout is taller falls back to its one-line form; a display block is capped as in v0.4.0 (200×40)." Add to §Design 2: "At most `MAX_FORMULAS` (64) tags per answer are typeset, in order; later tags stay as text. Placeholders are U+E000 + index. The worker renders an `Answers` job's formulas through the same cache; a job is one unit of work and may follow a `Latex` job, so the view shows "Loading…" until its event arrives (the pane overlay is not touched by the reader)." Add to AC7: "an answer with 4000 tags and one with a 32-deep nested script render bounded (≤ 64 formulas typeset; no line box taller than 3 rows)".

### F6 [Important] "One-line form: the v0.4.0 walk, unchanged in output" is not what v0.4.0 does, and the walk depends on term-maths' `mathfont`
Where: spec §Design 3 (One-line form), §Goals 4, §AC5; src/tui/latex.rs:131-147, 220-222
Problem: v0.4.0's inline string is term-maths' *block* when that block is one row (`[one] => one.trim()`), and the walk only otherwise. `E = mc²`, `a ≠ 0`, `∫₀¹ x² dx`, `rank(A) ≤ min(m, n)` in the existing table come from term-maths, not from the walk. With term-maths gone, AC5 ("existing table test passes unchanged") holds only if the new engine's one-row Text-style output reproduces those strings exactly (same operator spacing, same `\,` → one space, same script glyphs). Separately, the walk calls `term_maths::mathfont::map_str` for `\mathbb{R}` → `ℝ` (latex.rs:220-222; mathfont.rs is 194 lines of code-point tables), which must be ported or `ℝⁿ` in the table breaks.
Evidence: latex.rs:139-142; tests `the_block_is_term_maths_rendering` ("one row is its own inline") and `("\\mathbb{R}^n", "ℝⁿ")`.
Change: replace the One-line bullet with: "**One-line form** (pane overlay, fallback): the engine's Text-style layout when it comes out as one row; otherwise the v0.4.0 walk, moved into this module unchanged. The math-font tables (`\mathbb`, `\mathbf`, `\mathcal`, `\mathfrak`, `\mathsf`, `\mathtt`, bold Greek) move from term-maths' `mathfont.rs` into `typeset.rs` (same mappings, so `ℝⁿ` is unchanged). AC5 holds for every row of the existing table."

### F7 [Important] Placeholder lifecycle is underspecified against the second `sanitize`, grapheme clustering and markdown's own code-span rules
Where: spec §Design 2, §Failure modes ("placeholders inserted after sanitizing", "again on every string before it becomes a cell (the `text_cells` path)"); src/claude/latex.rs:69-73, 96-134; src/tui/latex.rs:678-697
Problem: (a) `text_cells` drops any grapheme that contains an `is_unsafe` char, and PUA is unsafe (latex.rs:70-71): a placeholder that reaches `text_cells` silently vanishes, so the Doc must resolve placeholders into formula boxes *before* any text run is sanitised again. (b) `sanitize` keeps a combining mark after any non-space base, and `>` is a base: in `…</latex>\u{301}x` the mark survives, and after tag replacement it clusters with the placeholder in `graphemes(true)`, so a grapheme-based scan misses the placeholder. (c) Tag detection must use `extract_latex`'s code exclusions (fence lines, backtick spans) so tags in code stay text, but pulldown-cmark decides code spans by its own rules, so a placeholder can still end up inside a `Code` event (e.g. a backtick run that `code_spans` closes differently). (d) pulldown-cmark's `ENABLE_MATH` (0.10+) would consume `$…$`; the option set is not stated.
Evidence: latex.rs:683 `.any(|c| is_unsafe(c) || …) { continue; }`; latex.rs:36-41 combining kept when `base` is true.
Change: replace the Design 2 placeholder sentence with: "Each `<latex>` tag found by `extract_latex`'s rules (same fence and backtick exclusions, same size and emptiness rules) is replaced by one placeholder char, U+E000 + index, *after* `sanitize` (which removes all private-use chars, so input cannot forge one). Placeholders are found by iterating `chars`, never graphemes; a combining mark directly after a placeholder is dropped. In the `Doc`, a placeholder inside a pulldown-cmark `Code`/`CodeBlock` event is replaced by the formula's source text (it was code to markdown). Text runs are re-sanitised only after placeholders have been resolved into formula boxes. pulldown-cmark options: `ENABLE_TABLES | ENABLE_STRIKETHROUGH | ENABLE_TASKLISTS`, nothing else (no footnotes, no `ENABLE_MATH`, no smart punctuation)."

### F8 [Important] "Display formula = a paragraph that is only one tag" is undefined for tight lists, quotes and multi-line tags
Where: spec §Goals 2, §Design 4 ("A display formula is its own block, indented two columns")
Problem: pulldown-cmark emits no `Paragraph` events inside *tight* list items (the common case for claude's lists), so an item that is only a formula has no paragraph to test. A tag inside a block quote paragraph is a paragraph, but the "indented two columns" rule and the quote's `│ ` gutter are not reconciled. `extract_latex` matches tags across lines, so a tag spanning two paragraphs collapses them into one placeholder and markdown sees one paragraph.
Evidence: latex.rs:88-89 "across lines"; pulldown-cmark tight-list behaviour (documented in its `Tag::List`/`Tag::Item` docs: paragraphs are omitted in tight lists).
Change: add to §Design 4 Formulas: "A formula is *display* when its placeholder is the only non-whitespace content of a paragraph **or of a tight list item** (the text directly under `Item`), **or of a table cell**. A display block is indented two columns relative to its container (so inside a list item or quote it is indented from the item's or gutter's content column). A formula that is display but whose source held line breaks is laid out like any other (the parser sees one line, as `prepare` does)."

### F9 [Important] Hostile input can still remove whole lines and misalign code: `text_cells` drops a line on one wide cluster, and tabs vanish
Where: spec §Design 4 (Markdown mapping, code blocks "never wrapped"); src/tui/latex.rs:689-693; src/tui/ui/formulas.rs:46-48
Problem: `text_cells` returns `None` when any cluster is wider than two columns, and the only caller drops the line (`let Some(mut cells) = … else { continue; }`). A reader line holding one such grapheme (an emoji ZWJ sequence, a wide CJK compatibility form) disappears. `sanitize` keeps `\t`, and `text_cells` drops any cluster containing one, so tabs inside code blocks are silently removed and indentation shifts.
Evidence: latex.rs:693 `_ => return None`; latex.rs:683 drops `\t`.
Change: add to §Failure modes: "A grapheme wider than two columns is replaced by `�` (width 1), never the whole line. Tabs are expanded to four spaces in code blocks and to one space elsewhere before layout, so no `\t` reaches `text_cells`."

### F10 [Minor] The `[keys]` alias needs a lookup change, and a simpler option exists
Where: spec §Design 4 ("`formulas` kept as an accepted alias"), §Compatibility; src/tui/keymap.rs:313-320
Problem: `build_with` resolves a config name with `DEFAULT_BINDINGS.iter().find(|b| b.action.name() == name)`; an alias needs a normalisation step before that, and the behaviour when a config has both `formulas` and `reader` is not stated (second one hits the collision check at 331-340 with the message "already bound to 'reader'"). Keeping the action name `formulas` and changing only the help label avoids all of this with no user-visible loss.
Change: either replace the alias sentence with "the action keeps its config name `formulas` (only its help label changes to 'reader view'); no alias is needed", or add: "`Keymap::build_with` maps `formulas` to `reader` before lookup; when both appear, the later key (BTreeMap order) wins and a notice says so."

### F11 [Minor] The per-(answer, width) layout cache has no eviction
Where: spec §Design 2 ("cached per (answer, width) in the view state"), §Failure modes
Problem: a user resizing the terminal produces one layout per width seen; nothing bounds it.
Change: replace with "cached for the current width only (one layout per answer); a resize invalidates the cache".

### F12 [Minor] Reader-view details an implementer would have to guess
Where: spec §Design 4
Problem: not stated: how a multi-line prompt becomes "one line" in the header; what label a compaction-summary answer gets (if kept); a minimum width; mouse wheel; a reply that arrives after the view closed or for another session; the daemon's reply for an unknown session id; what the Tab list mode shows (its data source is the v0.4.0 `Latex` list of up to 64 formulas, which is not the same set as the 10 answers' formulas).
Change: add to §Design 4: "Header prompt: whitespace runs collapsed to one space, cut at the header width with `…`. Below 20 columns the body shows "too narrow". Mouse wheel scrolls the body (`wants_mouse`). A reply for a view that is closed or for another session is dropped. `Error "no such session"` shows as that text. Tab shows the session's v0.4.0 formula list (`SessionView.latex_srcs`, the conversation's last 64 formulas, independent of the 10 answers), each formula typeset by the new engine above its source."

### F13 [Minor] Delimiters, accents, `\binom`, braces and `\overset` have no layout rule
Where: spec §Design 3 lists "roots, delimiters, matrices, cases, accents" as combinators
Problem: the AST has `Delimited`, `Accent`, `Binom`, `Brace`, `StackRel` (ast.rs:127-170); the rules give no output for them.
Change: add: "`\left…\right` around a block taller than one row draws ⎛⎜⎝ / ⎡⎢⎣ / ⎧⎨⎩ / │ stretched to the block's rows (one-row content: plain brackets). Accents: a combining mark over a one-character body, else `hat(…)`-style names, as the v0.4.0 walk. `\binom`: `(n choose k)` on one line in Text; two rows in parentheses in Display. `\overbrace`/`\underbrace`/`\overset`/`\underset`: the annotation as a raised/lowered row over/under the content, centred."

### F14 [Minor] The spec measures term-maths' `S_{i,f}` output as 33 columns; it is 32
Where: spec §Problem, §AC6
Problem: the scratch run prints `S    =     ∑     s       + G (i)` (32 columns). The AC6 target "< 33" is met by anything that is not wider, which is weak; the golden in F3 is 29.
Change: "term-maths' 32 columns"; AC6 "≤ 30 columns".

### F15 [Minor] Spacing commands are named by LaTeX command; the AST only has widths
Where: spec §Design 3 ("`\,` `\;` `\quad` map to 1/1/2 spaces")
Problem: the parser emits `Space(3.0)`, `Space(5.0)`, `Space(18.0)`, `Space(-3.0)` and `Space(4.0)` for operators (parser.rs:814-820); the rule must be by width or `\!` and `\qquad` are undefined.
Change: covered by the Whitespace bullet in F3's replacement text.

### F16 [Minor] `is_real_prompt` is private and `Answer` needs an identity for caching
Where: spec §Current state, §Design 2
Problem: `fn is_real_prompt` (projects.rs:483) is not `pub`; the layout cache is "per answer" but `Answer { prompt, text }` has no id, and the same text can appear twice.
Change: "`Answer { prompt, text }`; the client caches by position in the reply (the view state is rebuilt on every reply)."

---

