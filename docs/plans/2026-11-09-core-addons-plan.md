# Core and addons: implementation plan

Spec: docs/specs/2026-11-09-core-addons-design.md (audited; owner delegated approval)
Class: architectural — planned one phase at a time; each phase section is added here before the
phase runs, and audited when it changes an interface or a format.
Repos and branches: /volumes/repos/claudio → refactor/core-addons (base f90163f, main)

## Global Constraints
- Follow ACCEPTANCE.md; cite rule numbers in reports and reviews.
- Commit convention: conventional commits `type(scope): lowercase summary`; one logical change
  per commit; NO AI attribution, no Co-Authored-By or similar trailers.
- Never push, tag or switch branches; commit with explicit paths (`git commit -m … -- <paths>`,
  `git add <exact new paths>`); never `git add -A`, never `--amend`, never `git stash`.
- Every commit builds (`cargo build --locked`) and passes `cargo test --locked --bin claudio`.
- Moves are moves: `git mv` for whole files; a move and a behaviour change never share a commit.
- Never weaken a test to get to green (E2). A deliberately changed contract (this refactor's G5)
  replaces its assertions with tests of the new contract, said in the commit message.
- Toolchain: rustup's (`TC=$(dirname $(rustup which cargo))`). Clippy only as `PATH=$TC:$PATH
  CARGO_TARGET_DIR=$HOME/.cache/claudio/clippy-target $TC/cargo clippy --locked --all-targets
  --all-features -- -D warnings` (the distro clippy is a different version). `make check` runs
  everything.
- Build and test scratch under `~/.cache/claudio/`, never `/tmp`. Kill every process you start.
  Never touch the owner's own `claudio` daemon or `claudio web` (tests use isolated env).
- Secrets (C1) and the web perimeter (C5) hold throughout. Do not edit web/vendor/**,
  web/fonts/** or the import-map element.
- Web UI changes follow DESIGN.md.

## Review Focus
- The web UI must keep rendering formulas, diagrams and lenses (it depends on the daemon's
  `Lens`/`Latex` events and on the extension registry's lens specs, `src/web/hub.rs:484-500`,
  `src/web/ext.rs`).
- Spawns must keep sending the same `SessionExtensions` (lenses + instructions) as before P1
  (`src/tui/extensions/mod.rs:282` `session_extensions`).
- No behaviour change outside G5's removal.

## Task order and parallelism
| Task | Depends on | Files | Parallel with |
|---|---|---|---|
| P0.1 fmt | — | whole tree | — |
| P0.2 clippy + flakes | P0.1 | src/**, tests/** | P0.3 (disjoint files) |
| P0.3 rules tooling | P0.1 | scripts/, Makefile, CI, CLAUDE.md | P0.2 |
| P1.1 plain TUI | P0 | src/tui/**, src/claude/hooks.rs, src/main.rs, tests/manager.rs, tests/screenshots.rs, Cargo.toml, docs | — |

## Tasks

### P0.1: cargo fmt (done, a68b90b)

### P0.2: clippy clean on all targets; fix two flaky unit tests
Model: standard. Done by brief (ledger), not a patch.

### P0.3: rules tooling
Model: controller. `scripts/check_rules.py` (A1, A2, A8, D2, D4; baseline
`scripts/size-baseline.txt`, written after P0.2), `make check`, CI job `check`, CLAUDE.md
pointer. Expected: `make check` green.

### P1.1: the console TUI stops rewriting claude's screen (spec G5, AC6 TUI part)
Model: strong   Files: src/tui/**, src/claude/hooks.rs, src/main.rs, Cargo.toml, Cargo.lock,
tests/manager.rs, tests/screenshots.rs, README.md, docs/extensions.md, CLAUDE.md
Goal: the TUI draws the pane exactly as claude printed it; formulas, diagrams and tag lenses
render only in the web UI.

What goes (TUI rendering only):
- `src/tui/typeset/`, `src/tui/latex.rs`, `src/tui/formulas.rs` (the reader's formula list),
  `src/tui/diagram/`, the in-pane overlay (`ui/mod.rs`), the per-session formula/lens state on
  `SessionView` (`sessions.rs:72-85`: latex_seq, latex_srcs, latex, lens_items, lens),
  `App::lens_hosts`, the render worker's formula and external-lens paths (`mod.rs:607-911`),
  `Effect::RenderLatex` / `Effect::RenderExternal` and their `HostEvent`s, the reader's formula
  boxes and Tab formula list (`reader/doc.rs`, `reader/layout.rs`, `ui/reader.rs`), and the
  pre-0.6 `__context latex` hook (`claude/hooks.rs:53-60,128`; `main.rs` keeps
  `__context file`). The keymap's legacy `Formulas` action name goes if nothing else uses it.
- `rust-latex-parser` leaves Cargo.toml/Cargo.lock if nothing else uses it.

What stays:
- The extension registry with its built-ins (latex, diagram, search, snippets), their lens
  specs and instructions, and `Registry::session_extensions()`: spawns send the same lenses and
  instructions as today, because the web UI renders them from the daemon's lens events (the web
  addon takes them over in P3).
- `ExtensionHost::render` and protocol v1's `render` (the web's `extensions.render` op uses
  them). If `extensions/protocol.rs` needs `typeset::Laid` only as a data type, move that type
  next to the protocol; the web must keep compiling and passing `cargo test --test web`.
- The reader (Alt+u): answers as markdown; `<latex>` formulas as their source text (inline code
  style is fine) and ```` ```mermaid ```` fences as code blocks.
- Leader actions (search, snippets, external actions), the extensions screen, Alt+z, Alt+i.
- The daemon is untouched.

Tests (E2: G5 changes the contract; replace, don't just delete):
- `tests/manager.rs`: `test_latex_formula_is_rendered_in_the_pane_and_the_reader_list`,
  `test_reader_shows_the_answers_with_formulas_in_2d`, `test_reader_draws_a_mermaid_diagram`
  become tests that the pane and the reader show claude's `<latex>…</latex>` text and the
  mermaid source unchanged. `test_external_extension_lens_leader_and_actions` keeps its leader
  and action assertions; its pane-lens assertion becomes "the tag text is shown unchanged".
- Unit tests of deleted modules go with them; reader tests assert source text.
- `tests/screenshots.rs`: drop or adapt shots of 2D formulas/diagrams.
- Run: `cargo build --locked`, `cargo test --locked --bin claudio`, `cargo test --locked --test
  manager -- --test-threads=1`, `cargo test --locked --test web -- --test-threads=1`,
  `cargo test --locked --test screenshots`, `node --test web/tests/*.test.mjs`, `cd web/e2e &&
  npx playwright test` (formula and diagram e2e specs must pass unchanged), clippy as above.

Docs: README (reader and status bar text about formulas), docs/extensions.md (latex/diagram
render in the web UI; the console shows the tags), CLAUDE.md's architecture map (remove the
typeset/latex/diagram/formulas entries and the "Formulas are never rendered on the UI thread"
constraint's TUI parts; keep what still applies to the daemon/web).

Commits: one per logical step, e.g. `refactor(tui): the pane shows claude's text without the
formula overlay`, `refactor(tui): the reader shows formulas and diagrams as source`,
`refactor(tui): remove the typesetter and the diagram renderer`, `docs: formulas and diagrams
render in the web UI only`.

Expected size: about −9k lines.

### P2: untangle (spec §Untangling; exit: `grep -rn 'crate::tui' src/web` is empty)
Model: standard   Files: src/** (moves and import updates), tests only where imports move
Goal: every helper lives where it belongs, the module cycles are gone, and the code both
front-ends share sits in a UI-free `app` layer, so `src/web` imports nothing from `src/tui`.
No behaviour change; each step is its own commit (moves via `git mv` where a file moves whole,
then a separate commit for any edit). Every commit: `cargo build --locked` and
`cargo test --locked --bin claudio` pass; at the end `make check`, manager, web, screenshots,
node and Playwright suites pass and `python3 scripts/check_rules.py --write-baseline` renames
baseline entries for moved files (same counts; totals must not grow).

Steps (in this order):
1. `src/text.rs`: `sanitize` (from `claude::latex`), `parse_rfc3339` (from `proxy::api`),
   `fmt_tokens` (from `proxy::cmd`). Callers import `crate::text`. `claude::latex` keeps its
   extraction functions. Breaks `proto → claude`, and `tui::fmt → proxy`.
2. `src/release.rs`: `SemVer` and `http_client` (from `upgrade`), `RELEASES_REPO`,
   `release_arch`, `release_os` (from `remote::bootstrap`). `upgrade`, `remote`, `freshness`,
   `claude::update` import `crate::release`.
3. `paths` stops importing `proto`: `PROTO` moves to `src/version.rs` (`pub const PROTO`), and
   `proto` re-exports it (`pub use crate::version::PROTO;`) so its users don't change.
4. `proto → proxy`: `Msg::merge_env` uses a private env-merge function in `proto` (the body of
   `proxy::env::merge`, which then calls the proto one or is deleted if unused).
5. `plugins ↔ proxy`: `name_ok` and `key_ok` move from `proxy::plugins` to
   `plugins::manifest` (the parser of the names they check); `proxy`, `config` and
   `daemon::manifest` import them from there.
6. `freshness ↔ client`: `freshness::ensure_current` takes the connected `Client` (the caller
   connects) instead of calling `client::connect`; `freshness` no longer names `client`.
7. `src/app/` (the shared layer, no ratatui/crossterm/axum): `git mv` `tui/state.rs` →
   `app/state.rs`, `tui/connections.rs` → `app/connections.rs`, `tui/snippets.rs` →
   `app/snippets.rs`, `tui/extensions/` → `app/extensions/`, then edit: `sanitize_label` from
   `tui/sessions.rs` → `app/label.rs`; `stats_view::Window` → `proxy/stats.rs` (it is proxy
   stats data); `plugin_setup::choices_for` → `plugins/choices.rs`; `proxy::login::
   {browser_wanted, open_browser}` → `app/browser.rs`. `tui/test_support.rs` helpers used by
   the moved files move with them (`app/test_support.rs`, `#[cfg(test)]`).
8. `tui/plugins.rs` (the pure plugin state machine and `compose`) → `plugins/state.rs`, beside
   the manifest parser; the TUI's `impl App` glue that lives in it stays in
   `tui/plugins_glue.rs` (or `tui/plugins.rs` keeps only the glue). Web imports
   `crate::plugins::state`.
9. Snippets once (A5): the TUI runner (`tui/mod.rs` `LoadSnippets`/`SaveSnippet`) and
   `web/ext.rs` call the same `app::snippets` functions; no duplicated directory or naming
   logic remains.

Interfaces this phase provides to P3: `crate::text`, `crate::release`, `crate::version`,
`crate::app::{state, connections, snippets, extensions, label, browser}`,
`crate::plugins::{manifest, state, choices}`.

## Merge and release
1. Each phase: all suites green (`make check`, manager, web, screenshots, node, Playwright),
   then `git merge --no-ff refactor/core-addons` into local `main` after the phase's review.
2. **Owner OK required before:** any push, tag or release.
