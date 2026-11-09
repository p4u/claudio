# Core and addons, P4: the Cargo workspace (implementation plan)

Spec: docs/specs/2026-11-09-core-addons-design.md (audited; §Crates, §Compatibility "Tests",
the "P4 Workspace" row of the Phases table, AC1-AC4, AC8). Umbrella plan:
docs/plans/2026-11-09-core-addons-plan.md. Previous phase: docs/plans/2026-11-09-core-addons-p3-plan.md.
Class: architectural
Repo/branch: /volumes/repos/claudio → refactor/core-addons (base: the commit that ends P3: P3
ended at 676b605, plus the P3 gate-fix round running on top of it; the base is that round's
last commit). Parallel tasks run on branches `p4/tui`, `p4/api`, `p4/web` and `p4/vocdoni` in
worktrees under `~/.cache/claudio/p4/`.
Written against: 307f801, with the pre-flight audit run against cb4f9bf and re-checked at 0a8adea.
Paths are of those trees. The end-of-P3 facts this plan relies on are listed in "P4 start
checks" and verified before P4.1. The gate-fix round (its F5) may already have done some of
P4.1's A1 renames. Those steps say "verify at P4 start; do only what remains" and give the grep.

## Pre-flight rulings applied

From `.delivery/2026-11-09-core-addons/preflight-p4.md` (verdict EXECUTE AFTER FIXES). The
controller accepted F1-F14, each with its "Change" text, and ruling R24:

- F1 / R24 (Critical): a build without the api addon refuses `-p`/`--print`/`--api` with
  `claudio: print mode and the API server need the api addon; this build has none` and exit 2.
  claudio never invokes `claude -p`. Covered in P4.7 commit 1 (`refuse_print_without_api` + test),
  P4.7 docs and Spec issue 8.
- F2: the start checks and P4.1 commits 4/6 now name what remains after 83342e3 and the gate-fix
  round: the eight A1 lines, verified by grep, done only if still there.
- F3: the manager split list gains five tests and the plugin helpers. Classification reads each
  test's body. Two core tests drop their convenience `--no-proxy` and stay in the core suite.
- F4: P4.5 checks the embedded page through build-script outputs, not `strings`.
- F5: P4.1 step 8 runs the subset suites as auto-discovered targets; the `[[test]]` table comes
  in step 9 only.
- F6: Merge step 4 compares `--list` output with `--all-features` on both sides.
- F7: P4.2 keeps the root's `test-support` dev-dependency, and says why.
- F8-F14: Spec issue 4's counts; build.rs comment paths allowed in a move commit; `.dockerignore`
  keeps the vendored LICENSES.md; slot comments go between `#[cfg]` and the item; the rustfmt-wrap
  fallback never grows a listed file; `tests/fixtures/claude/` stays; `--features ""` is commented.

**End state of P4.**

```
Cargo.toml        [workspace] + the `claudio` binary package (features api, web, vocdoni: default)
src/main.rs       wiring only: core words, the cfg'd addon registry, help, passthrough
crates/core       claudio-core    addon/ app/ claude/ client config daemon/ freshness paths proto
                                  release remote/ term/ (screen, probe) text upgrade version
crates/tui        claudio-tui     the console manager, --plain (+ keys.rs and pane.rs from term/)
                  tests/          manager.rs, common/ (the PTY harness)
addons/api        claudio-api     print/, api/, cli.rs, msglog.rs     tests/integration.rs
addons/web        claudio-web     src/ (was src/addons/web), web/ (the page), build.rs
                                  tests/web.rs, tests/manager.rs (TUI sessions with the web addon)
addons/vocdoni    claudio-vocdoni the claude-proxy client and the team plugins
                                  tests/manager.rs, tests/web.rs
tests/            remote.rs, screenshots.rs (the whole binary), fixtures/ (v0.8.0 files)
```

Dependencies (A2): `claudio` → {core, tui, api?, web?, vocdoni?}; tui → core; each addon → core;
dev-dependencies only on core (with `test-support`). `cargo build --no-default-features` is core +
TUI and passes the manager suite (A4).

## P4 start checks (run before P4.1; stop and report if one fails)

```bash
git status --short                      # clean
git log -1 --format=%s                  # P3's last commit (P3.20 or its merge review fix)
ls src                                  # exactly: addon/ addons/ app/ claude/ client.rs config.rs daemon/
                                        # freshness.rs main.rs paths/ paths.rs plain.rs proto/ proto.rs
                                        # release.rs remote/ term/ text.rs tui/ upgrade.rs version.rs
ls src/addons                           # api/ mod.rs vocdoni/ web/
ls src/envmerge.rs src/web src/proxy src/plugins src/print src/api src/cli.rs 2>&1 | grep -c 'No such'   # 7
rtk proxy grep -rn 'crate::addons' src --include='*.rs' | rtk proxy grep -v '^src/addons/\|^src/main.rs'
                                        # empty (83342e3 did P4.1 commit 4's registry rewrite)
rtk proxy grep -rniE 'vocdoni|claude-proxy|claude\.vocdoni' src --include='*.rs' | rtk proxy grep -v '^src/addons/'
                                        # at most these eight lines (the gate-fix round may have renamed
                                        # some): app/state.rs:695, addon/test_support.rs:97-103 (doc),
                                        # tui/test_support.rs:14,18, tui/keymap.rs:1035,
                                        # tui/panel_view/tests.rs:2,112,200,203. P4.1 commits 4/6
                                        # rename whatever is left; anything else is reported
rtk proxy grep -rn 'crate::tui' src --include='*.rs' | rtk proxy grep -v '^src/tui/'   # only src/plain.rs
rtk proxy grep -n 'allow(dead_code)' src/addon/mod.rs src/app/mod.rs                    # empty (P3.20)
git ls-files tests/fixtures | sort       # includes state-v0.8.0.json, web-state-v0.8.0.json (P3.19)
make check                               # green; paste the "rules:" totals line into the report
```

If `ls src` shows a module this plan does not place (for example `envmerge.rs` survived), it goes
to claudio-core when both front-ends or an addon use it, else with its only user; say so in the
P4.2 report.

## Global Constraints

From the umbrella plan (unchanged):
- Follow ACCEPTANCE.md; cite rule numbers in reports and reviews.
- Commit convention: conventional commits `type(scope): lowercase summary`; one logical change
  per commit; NO AI attribution, no Co-Authored-By or similar trailers.
- Never push, tag or switch branches; commit with explicit paths (`git commit -m … -- <paths>`,
  `git add <exact new paths>`); never `git add -A`, never `--amend`, never `git stash`.
- Moves are moves: `git mv` for whole files; a move and a behaviour change never share a commit.
- Never weaken a test to get to green (E2). A test moved to another file keeps every assertion.
- Toolchain: rustup's (`TC=$(dirname $(rustup which cargo))`). Clippy only as
  `PATH=$TC:$PATH CARGO_TARGET_DIR=$HOME/.cache/claudio/clippy-target $TC/cargo clippy --locked
  --workspace --all-targets --all-features -- -D warnings` (from P4.2 on; before it, without
  `--workspace`). `make check` runs everything.
- Build and test scratch under `~/.cache/claudio/`, never `/tmp`. Kill every process you start.
  Never touch the owner's own `claudio` daemon or `claudio web` (tests use isolated env).
- Secrets (C1) and the web perimeter (C5) hold throughout. Do not edit web/vendor/**,
  web/fonts/** or the import-map element (P4.5 moves them byte-identical with `git mv`).
- Web UI changes follow DESIGN.md (P4 makes none).

Added for P4:
- **Behaviour changes are forbidden.** The default-features binary behaves exactly as P3's: same
  words, same addon registration order (vocdoni, web, api), same help appendix, same daemon ops,
  same files. The only new behaviour is in builds without some addons: they lack those addons,
  and a build without `api` refuses `-p`/`--print`/`--api` with exit 2 (R24, P4.7 commit 1).
- **A move commit** contains `git mv`, and only the edits the move needs to build: `mod`/`use`
  lines, `crate::` path rewrites, `pub(crate)` → `pub` for items another crate now reaches,
  `cfg(test)` → `cfg(any(test, feature = "test-support"))` for fixtures another crate's tests
  use, `Cargo.toml`/`Cargo.lock`, fixture paths, a path in a comment that names a moved file,
  rustfmt's reordering, and renamed baseline entries (`scripts/size-baseline.txt`, same counts).
  Nothing else. Doc comments, clippy fixes
  and test changes are their own commits.
- **Green at every commit:** `cargo build --locked` and `cargo test --locked --workspace --lib
  --bins` (before P4.2: `cargo test --locked --bin claudio`). Every task ends with its crate
  building alone (`cargo build --locked -p <crate>`), its unit tests passing
  (`cargo test --locked -p <crate>`), `python3 scripts/check_rules.py` green and the suites the
  task lists green.
- **`--write-baseline`** only in P4.1's moves, P4.2 and P4.7, and its printed totals equal the
  previous ones. The parallel tasks (P4.3-P4.6) never run it: they rename their own block's lines
  with the `sed` given in the task.
- **Paths inside a moved crate.** `crate::<core module>` becomes `claudio_core::<core module>`
  and the crate's own `crate::<old prefix>::` becomes `crate::`, rewritten in that order (core
  first). The core modules: `addon app claude client config daemon freshness paths proto release
  remote term text upgrade version`. If rustfmt then wraps a rewritten line and a listed file or
  function grows (or an unlisted one crosses 800/80), shorten that path with a
  `use claudio_core::<module>;` at the top of the file instead, in the same move commit. In a
  baseline-listed file, fold the shortened path into an existing `use claudio_core::{…}` line
  instead of adding one. For a function near 80 lines, prefer a `use` at the top of the file (a
  function body never grows from a `use`).
- **Test fixtures across crates** go behind claudio-core's `test-support` feature, enabled only
  by dev-dependencies: `#[cfg(any(test, feature = "test-support"))] pub mod test_support;`.
  `cargo tree -p claudio -e normal -f '{p} {f}' | grep test-support` is empty (a release binary
  never links them).
- **D6.** Every item P4 makes `pub` in claudio-core has a doc comment (P4.2 commit 3). Module
  docs are the files' `//!` headers.
- **D7.** claudio-core's normal dependencies include no ratatui, crossterm, axum or tower-http:
  `cargo tree -p claudio-core -e normal --depth 1 | grep -E 'ratatui|crossterm|axum|tower'` is
  empty from P4.2 on. axum stays a core dev-dependency (fake release server in two tests).
- **Heavy suites across worktrees run one at a time:** wrap PTY and web suites in
  `flock ~/.cache/claudio/p4/suites.lock …` so parallel implementers do not starve each other's
  10 s waits. A timeout that does not reproduce alone is reported, never retried to green.
- Model roles: **strong** for P4.1, P4.2 and P4.7 (test rewrites, the workspace, the wiring and
  the rules); **standard** for the four crate moves.

Suites (names unchanged from P3; targets are the root package's, see D2):
- S-unit: `cargo build --locked && cargo test --locked --workspace --lib --bins`
- S-manager: `cargo test --locked --test manager -- --test-threads=1`
- S-manager-core: `cargo test --locked --no-default-features --test manager -- --test-threads=1`
- S-web: `cargo test --locked --test web --test web_manager -- --test-threads=1`
- S-vocdoni: `cargo test --locked --test vocdoni_manager --test vocdoni_web -- --test-threads=1`
- S-node / S-e2e: `make test-web` (paths updated in P4.5)
- S-api: `cargo test --locked --test integration` (no-op without `CLAUDIO_E2E=1`; must compile)
- S-remote: `cargo test --locked --features diag --test remote -- --test-threads=1`
- S-shots: `cargo test --locked --test screenshots` (gated: compiles and skips)
- S-check: `make check`; S-matrix: `make matrix` (from P4.7)

## Review Focus

- **Moves are moves.** `git diff -M --stat <P3 end>..HEAD` shows renames; for each move commit,
  `git show -M --stat <sha>` has no file below 90 % similarity except the new `lib.rs`s and
  `Cargo.toml`s, and its content diff is only the edits the Global Constraints allow.
- **Registry order and cfg.** `src/main.rs::addons()` (P4.7) keeps vocdoni, web, api with api
  last (it parses every remaining argv). The `diag` feature still reaches `remote::diag` and the
  Vocdoni diag word.
- **test-support never in a release** (command above), and D7 for the core.
- **A1 in the TUI's tests.** `crates/tui/tests/**` and the TUI's unit tests name no team service
  (P4.1 moves those tests out or onto `TestAddon`). Moved tests keep every assertion (E2).
- **The web page.** `git diff --find-renames=100% --name-status <P3 end>..HEAD -- web/vendor
  web/fonts web/index.html` lists only `R100` lines; the CSP import-map hash is unchanged (the
  e2e asserts zero CSP violations).
- **One binary.** `target/<triple>/release/claudio` is still what `make static`, `make dev`,
  `install`, release.yml and the remote bootstrap use; release.yml's version check still reads
  `0.8.0` from Cargo.toml's first `version =` line.

## Decisions this plan makes

- **D1 Root `[[test]]` targets.** Integration tests drive the `claudio` binary, and Cargo gives
  `CARGO_BIN_EXE_claudio` only to the binary's own package (artifact dependencies are nightly-only).
  So each suite's file lives in the crate it tests (`crates/tui/tests/manager.rs`,
  `addons/<a>/tests/*.rs`) but is a `[[test]]` target of the root package with
  `required-features` naming the addons it needs. Cargo then builds the binary with exactly those
  features, and skips the suite in a build without them. Every member sets `autotests = false`.
  `cargo test -p claudio-tui` runs the TUI's unit tests; its manager suite is S-manager-core (A4)
  and S-manager.
- **D2 The PTY harness** stays one file, `crates/tui/tests/common/mod.rs`. Other suites include
  it with `#[path = "../../../crates/tui/tests/common/mod.rs"] mod common;` (root `tests/`:
  `"../crates/tui/tests/common/mod.rs"`). It is a test file (`is_test_file`), measured by no
  budget, and links no crate.
- **D3 Parallel moves with trivial merges.** P4.2 pre-declares every member with a complete
  `Cargo.toml` and a one-line stub `lib.rs`, the root's optional dependencies and features, and a
  complete `Cargo.lock`. It leaves one-line "slots" in `src/main.rs` and `src/addons/mod.rs`,
  separated by unchanged lines, and groups the size baseline in per-crate blocks separated by
  comment lines. A crate-move task then changes only its own files, one or two slot lines and its
  own baseline block. Disjoint hunks merge without conflicts, and each worktree builds the whole
  binary on its own: the slot turns `mod web;` into `pub use claudio_web as web;`.
- **D4 `[workspace.dependencies]`** pins every external crate once, with today's
  requirements. Members use `x.workspace = true`, so `Cargo.lock` keeps today's versions
  (`git diff Cargo.lock | grep -E '^[-+]version'` lists only claudio-* entries).
- **D5 Versions.** `[workspace.package] version = "0.8.0"` is the first `version =` line in
  Cargo.toml (release.yml reads it). Every package uses `version.workspace = true`, so each
  crate's `CARGO_PKG_VERSION` (Welcome, probe, bootstrap, upgrade, web hello) stays the release
  version.
- **D6 `--plain`'s argv parser** (`src/plain.rs`) becomes `crates/tui/src/plain/args.rs`. That
  is a child of `plain.rs`, so the over-budget `lib.rs` (was `tui/mod.rs`, 838) gains no line.
  The TUI's `mod plain;` becomes `pub mod plain;`.
- **D7 The terminal model stays in the core; its UI ends go to the TUI** (P4.1). `term/keys.rs`
  (crossterm key encoding, used by the TUI only) moves whole to `tui/keys.rs`. `Screen::render`
  and `alac_to_ratatui` (ratatui) move to `tui/pane.rs` as `render(&Screen, Rect, &mut
  Buffer)`, over a new `Screen::grid()` accessor and a `pub` `named_index`. The core keeps
  `Screen`, the snapshot, `probe` and `strip_escapes` (the daemon and the API use them).
- **D8 Shared fixtures.** `tests/fixtures/{state,web-state}-v0.8.0.json` stay at the root and are
  read by three crates as `concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/…")`.
  Every member is two levels deep. `stream{1,2}.raw` (core only) move to
  `crates/core/tests/fixtures/`, so `screen.rs`'s `include_bytes!("../../tests/fixtures/…")` is
  unchanged. `tests/fixtures/claude/` (hook payload samples, referenced by nothing) stays at the
  root, untouched.
- **D9 Unused dependencies.** `tokio-stream` and `tower` have no user in the tree today. P4.2
  keeps them in the root's list (no change), and P4.7 drops them with the root's other leftovers
  (D7 "dependencies are earned"; `tower` stays in the lock through axum).

## Task order and parallelism

| Task | Model | Depends on | Runs in | Owns (writes only these) | Parallel with |
|---|---|---|---|---|---|
| P4.1 prep: D7 split, features, tests decoupled and split | strong | P3 end | main checkout | src/tui/{mod,keys,pane,test_support,plain,…}.rs (tests), src/term/**, src/addon/{mod,test_support}.rs, src/config.rs (doc), src/addons/mod.rs, src/addons/web/** (tests), Cargo.toml, tests/** → crates/tui/tests, addons/*/tests, scripts/size-baseline.txt | — |
| P4.2 workspace, claudio-core, stubs, slots | strong | P4.1 | main checkout | Cargo.toml, Cargo.lock, crates/** (new), addons/*/Cargo.toml + src/lib.rs stubs, src/{addon,app,claude,daemon,paths,proto,remote,term}/ and the core .rs files (moved), src/main.rs, src/addons/mod.rs (slot spacing), tests/fixtures, Makefile `check`/`test-unit`, .github/workflows/ci.yml (`--bin claudio` → workspace), scripts/size-baseline.txt | — |
| P4.3 claudio-tui | standard | P4.2 | worktree p4/tui | src/tui/** → crates/tui/src/**, src/plain.rs → crates/tui/src/plain/args.rs, crates/tui/src/lib.rs, crates/tui/Cargo.toml (deps only), src/main.rs (the two TUI slot lines only), baseline block `claudio-tui`, Cargo.lock entry `claudio-tui` only | P4.4, P4.5, P4.6 |
| P4.4 claudio-api | standard | P4.2 | worktree p4/api | src/addons/api/** → addons/api/src/**, addons/api/src/lib.rs, addons/api/Cargo.toml (deps only), src/addons/mod.rs (the `api` slot line only), baseline block `claudio-api`, Cargo.lock entry `claudio-api` only | P4.3, P4.5, P4.6 |
| P4.5 claudio-web | standard | P4.2 | worktree p4/web | src/addons/web/** → addons/web/src/**, build.rs → addons/web/build.rs, web/ → addons/web/web/, addons/web/web/e2e/fixtures/server.mjs (one path), addons/web/src/lib.rs, addons/web/Cargo.toml (deps only), src/addons/mod.rs (the `web` slot line only), Makefile `test-web` target only, baseline block `claudio-web`, Cargo.lock entry `claudio-web` only | P4.3, P4.4, P4.6 |
| P4.6 claudio-vocdoni | standard | P4.2 | worktree p4/vocdoni | src/addons/vocdoni/** → addons/vocdoni/src/**, addons/vocdoni/src/lib.rs, addons/vocdoni/Cargo.toml (deps only), src/addons/mod.rs (the `vocdoni` slot line only), baseline block `claudio-vocdoni`, Cargo.lock entry `claudio-vocdoni` only | P4.3, P4.4, P4.5 |
| P4.7 integration: wiring, rules, CI, docs | strong | P4.3-P4.6 merged | main checkout | Cargo.toml, Cargo.lock, src/main.rs, src/addons/mod.rs (deleted), scripts/check_rules.py, scripts/size-baseline.txt (re-sort), Makefile, .github/workflows/ci.yml, Dockerfile, .dockerignore, CLAUDE.md, README.md, ACCEPTANCE.md ("Checking" only), DESIGN.md, docs/{web,addons,extensions,print-and-api}.md | — |

**What no parallel task touches:** the root `Cargo.toml`, `crates/core/**`, another task's crate
or slot, `tests/**`, `crates/*/tests/**`, `addons/*/tests/**`, `scripts/check_rules.py`, CI,
docs, and (except P4.5's `test-web` lines) the Makefile. Anything a parallel task finds missing
in claudio-core (an item still `pub(crate)`, a `cfg(test)` fixture) is a P4.2 defect. The task
stops and reports it, and the controller fixes it on refactor/core-addons and merges that fix
into the worktrees (`git merge refactor/core-addons`) before they continue.

**Why the merges are trivial (D3).** After P4.2:
- `src/main.rs` has the TUI's two slots and no other line a parallel task edits.
- `src/addons/mod.rs` has three slots (`api`, `vocdoni`, `web`), each preceded by its own
  unchanged `#[cfg(feature = …)]` line and separated by a blank line.
- `scripts/size-baseline.txt` has one block per destination, each opened by an unchanged
  `# -- <crate>` comment.
- `Cargo.lock` already has all five packages with their final dependency lists. A task changes
  its own entry only if the compiler asks for a dependency P4.2 missed. If two tasks did that,
  the controller resolves the lock with `cargo update --workspace --offline` (it rewrites the
  claudio-* entries only).
- The four tasks move disjoint directories.

**Worktrees** (the controller creates them; agent-isolation worktrees start from main and must not
be used):

```bash
B=$(git rev-parse refactor/core-addons)       # P4.2's last commit
for t in tui api web vocdoni; do
  git worktree add -b p4/$t ~/.cache/claudio/p4/$t "$B"
done
mkdir -p ~/.cache/claudio/p4 && touch ~/.cache/claudio/p4/suites.lock
# in each worktree's shell (the tasks below use $B, $CORE and $TC):
export B=<P4.2's last commit> CARGO_TARGET_DIR=$HOME/.cache/claudio/p4/<task>-target
export TC=$(dirname $(rustup which cargo))
export CORE='addon\|app\|claude\|client\|config\|daemon\|freshness\|paths\|proto\|release\|remote\|term\|text\|upgrade\|version'
```

---

## P4.1: prep in the single crate: the TUI's UI ends, the features, the tests split by need

Model: strong   Runs on: refactor/core-addons (main checkout)
Files: src/term/{mod,keys,screen}.rs, src/tui/{mod,keys,pane,test_support,plain,keymap,
wizard_route,panel_view/tests,checklist/tests,ui/status,ui/panel/tests,ui/checklist,
ui/mod}.rs, src/addon/{mod,test_support}.rs, src/config.rs, src/addons/mod.rs,
src/addons/web/addons/tests.rs (and any other web test that calls `crate::addons::all()`),
Cargo.toml, tests/** → crates/tui/tests/, addons/{api,web,vocdoni}/tests/,
scripts/size-baseline.txt

Goal: before any crate exists, (a) the core no longer needs ratatui or crossterm (D7), (b) the
binary can be built with any subset of the addons, (c) no core or TUI test needs an addon, and
(d) every integration suite sits in the file and directory of the crate it tests, as a root
target with the features it needs. After P4.1, P4.2 is pure plumbing.

Commits, in order (each green: `cargo build --locked && cargo test --locked --bin claudio`):

1. `refactor(tui): merge mod.rs's app-layer imports` (pays A8 for commit 2/3's two `mod` lines):
   `src/tui/mod.rs:76-79`'s four `use crate::app::…` lines become
   `use crate::app::{connections::Connections, extensions, snippets, state::ClientState};`
   (−3 lines; rustfmt keeps it on one line).
2. `refactor(tui): the key encoder moves from term to the TUI`: `git mv src/term/keys.rs
   src/tui/keys.rs`; `src/term/mod.rs` loses `#[cfg_attr(…)] pub mod keys;`; `src/tui/mod.rs`
   gains `mod keys;`; `crate::term::keys` → `crate::tui::keys` (interaction.rs, leader.rs); keys.rs
   imports `crate::term::screen::Modes`. Baseline: `fn src/term/keys.rs::encode_key 127` →
   `fn src/tui/keys.rs::encode_key 127`.
3. `refactor(tui): the pane renderer moves from term::screen to the TUI` (a split; D7):
   new `src/tui/pane.rs` with `pub(crate) fn render(screen: &Screen, area: Rect, buf: &mut
   Buffer)` (today's `Screen::render` body, `self.term.grid()` → `screen.grid()`) and
   `alac_to_ratatui`. The `render` tests of `screen.rs` (`wide_chars_render` and every other test
   that calls `.render(`) move to `pane.rs`'s tests unchanged except `s.render(r, b)` →
   `render(&s, r, b)`. `screen.rs` gains `/// The alacritty grid this screen draws from (the TUI
   paints it).` `pub fn grid(&self) -> &Grid<Cell>`, and `named_index` becomes `pub fn` with a
   doc comment. `ui/mod.rs:331` `view.mirror.render(area, frame.buffer_mut());` →
   `super::pane::render(&view.mirror, area, frame.buffer_mut());` (no new line). `src/tui/mod.rs`
   gains `mod pane;`.
   Check: `rtk proxy grep -rln 'ratatui\|crossterm' src --include='*.rs' | rtk proxy grep -v
   '^src/tui/\|^src/addons/'` → empty.
4. `test(tui): no team name in the TUI's tests and docs` (A1). 83342e3 already built the
   registries from `TestAddon`. **Verify at P4 start; do only what remains**:
   `rtk proxy grep -rniE 'vocdoni|claude-proxy|claude\.vocdoni' src/tui --include='*.rs'`.
   At 0a8adea it lists:
   - `tui/panel_view/tests.rs`: rename `alt_s_opens_the_vocdoni_panel_about_the_active_session`
     → `alt_s_opens_an_addon_panel_about_the_active_session`, and make its module doc say "an
     addon panel". The error string `"vocdoni: switch failed: 429"` becomes
     `"team: switch failed: 429"` in both the call and the assertion (same assertion, same shape).
   - The doc comments at `tui/test_support.rs:14,18` and `tui/keymap.rs:1035` say "a routing
     addon" / "the test addon's panel key".

   Every assertion stays; the message lists the renamed test (E2). If the grep is empty, skip
   this commit.
5. `test(web): unit tests build their registries from TestAddon, not the registered addons`:
   `src/addons/web/addons/tests.rs::addons()` (and any other `crate::addons::all()` in
   `src/addons/web`) becomes a registry of `TestAddon::new("team")` with `.adopts(…)`/`.segments(…)`
   as the assertions need, plus `WebAddon` where a test reads the web rows. Expected values follow
   TestAddon's semantics (e.g. its `adopt_legacy` writes the flag `--first-party`). The adoption
   of the Vocdoni addon itself stays covered by `src/addons/vocdoni/launch/tests.rs`
   (`v080_fixtures_adopt_their_proxy_profiles`). The message names each changed expectation and
   where the real addon's behaviour is still tested.
6. `test(core): no team names in the core's tests and docs` (A1). **Verify at P4 start; do only
   what remains**: `rtk proxy grep -rniE 'vocdoni|claude-proxy|claude\.vocdoni' src
   --include='*.rs' | rtk proxy grep -v '^src/addons/\|^src/tui/'`. The items:
   - `src/addon/mod.rs`'s `name_ok` test: `"vocdoni"` → `"team"`.
   - `src/app/state.rs:695`: `a_build_without_vocdoni_keeps_the_choice_it_cannot_adopt` →
     `a_build_without_the_adopting_addon_keeps_the_choice_it_cannot_adopt`.
   - `src/addon/test_support.rs:97-103`: the doc says "as a routing addon does".

   config.rs's doc is already generic. If the grep is empty, skip this commit.
   Check: `rtk proxy grep -rniE 'vocdoni|claude-proxy' src --include='*.rs' | rtk proxy grep -v
   '^src/addons/'` → empty; `rtk proxy grep -rn 'crate::addons' src --include='*.rs' |
   rtk proxy grep -v '^src/addons/\|^src/main.rs'` → empty.
7. `build: api, web and vocdoni features select the linked addons`: Cargo.toml `[features]` (below);
   `src/addons/mod.rs` becomes:

   ```rust
   //! The addons this build registers (docs/addons.md). The core never names
   //! them; `main` hands `all()` to `addon::Addons::new`. Each is a cargo
   //! feature (`api`, `web`, `vocdoni`, all default); without any, the core alone.

   #[cfg(feature = "api")]
   pub mod api;

   #[cfg(feature = "vocdoni")]
   pub mod vocdoni;

   #[cfg(feature = "web")]
   pub mod web;

   use std::sync::Arc;

   use crate::addon::Addon;

   /// The addons this build registers, in registry order. The API addon
   /// parses every remaining argv (`cli::parse`), so it is last.
   pub fn all() -> Vec<Arc<dyn Addon>> {
       vec![
           #[cfg(feature = "vocdoni")]
           Arc::new(vocdoni::Vocdoni::new()),
           #[cfg(feature = "web")]
           Arc::new(web::WebAddon),
           #[cfg(feature = "api")]
           Arc::new(api::ApiAddon),
       ]
   }
   ```
   (keep its `addons_all_registers` test). A core-only single-crate build may warn about core
   items only addons use; that is transitional (P4.2 makes the core a library) and no check
   denies it.
8. `test: integration suites split by the addons they need` (functions moved verbatim, new files
   at `tests/`): a test goes to
   - `tests/vocdoni_manager.rs` if it needs the Vocdoni addon: it names a proxy profile, `--proxy`/
     `--no-proxy`/`--first-party`, `claudio vocdoni|proxy …`, the fake proxy, a plugin manifest,
     `CLAUDIO_PROXY_*`, the `vocdoni` addon or a proxy segment. At 307f801 these are
     `test_proxy_env_injection`, `test_proxy_real_claude`, `test_plain_keeps_proxy_stats_and_reset`,
     `test_plain_injects_proxy_env`, `test_plain_no_proxy_keeps_api_key`,
     `test_plain_with_unreadable_proxy_profiles_never_starts_direct`, `test_plain_first_party_flag`,
     `test_default_profile_with_unreachable_config_starts_through_the_proxy`,
     `test_plugin_manifest_setup_then_sessions_get_what_was_kept`,
     `test_a_launch_reviews_an_unsaved_plugin_and_skip_starts_without_it`,
     `test_new_client_old_daemon_gets_proxy_env_without_plugins`, plus P3.20's
     `test_vocdoni_login_token_then_a_session_gets_the_proxy_and_plugins`. At HEAD these are the
     twelve listed plus `test_plain_reviews_before_starting`,
     `test_restart_of_a_session_started_elsewhere_never_goes_direct`,
     `test_old_client_spawn_is_scrubbed_by_a_new_daemon`,
     `test_reset_restarts_claude_in_the_same_tab` and
     `test_env_recording_fakes_keep_inherited_secrets_out_of_fixtures` (each spawns through a
     profile or asserts on the proxy token). The helpers `fake_plugin_proxy`,
     `write_plugin_claude`, `write_plugin_claude_noting`, `context_files` and the `PLUGIN*`
     constants go with them. Classify by reading each test body for a profile, a token, a plugin
     or the fake proxy, not by this list; a test that only *mentions* proxies in a comment stays.
     Two core tests pass `--no-proxy` as a convenience
     (`test_plain_is_the_bare_session_full_screen`,
     `test_plain_exits_when_claude_ends_and_leaves_nothing`). They stay in the core suite with
     the flag removed, and the message names them. Otherwise a core-only build would forward the
     flag to claude unnoticed.
   - `tests/web_manager.rs` if it needs the web addon in a TUI session (the `latex`/`diagram`
     rows or lenses): `test_tui_session_requests_the_latex_lens_from_the_web_addon`,
     `test_extensions_screen_turns_extensions_off_and_saves_it`.
   - `tests/vocdoni_web.rs` if a `tests/web.rs` test needs the Vocdoni addon (the fake proxy,
     `PLUGIN`, `"addon": "vocdoni"`, `vocdoni_of`): at 307f801 about 18, e.g.
     `web_spawn_keeps_proxy_and_plugin_secrets_out_of_the_browser`, `web_addons_list_has_vocdoni_panels_and_launch_options`,
     `web_checklist_save_then_spawn_gets_the_plugin`, `web_proxy_stats_render_the_fixture`,
     `web_respawn_without_a_known_proxy_asks`, `web_v1_proxy_and_plugin_ops_are_unknown`.
   Then **verify empirically** (step 7's features make the subsets buildable):
   ```bash
   cargo test --locked --no-default-features --test manager -- --test-threads=1          # core only: all pass
   cargo test --locked --test manager -- --test-threads=1                                # all addons: all pass
   cargo test --locked --no-default-features --features web --test web --test web_manager -- --test-threads=1
   cargo test --locked --no-default-features --features vocdoni --test vocdoni_manager -- --test-threads=1
   cargo test --locked --no-default-features --features vocdoni,web --test vocdoni_web -- --test-threads=1
   ```
   At step 8 the files are auto-discovered root targets without `required-features`, so run the
   five commands exactly as written (`--test <name>` with the features that file needs). The
   `[[test]]` table and `autotests = false` come in step 9; do not add them earlier. In a
   core-only run only `--test manager` is invoked.
   A test that fails under its file's features moves to the file whose features it needs
   (stricter, never looser). Helpers used by two files move to `tests/common/mod.rs` if they name no
   team service; a team-specific helper stays in the one Vocdoni file that uses it (manager's and
   web's fake proxies are already separate). Each file starts with a `//!` line saying what it
   covers and which features it needs. The commit message lists each moved test (E2).
9. `test: integration suites move next to the crate they test` (moves; D1, D2):
   ```bash
   mkdir -p crates/tui/tests addons/api/tests addons/web/tests addons/vocdoni/tests
   git mv tests/manager.rs crates/tui/tests/manager.rs
   git mv tests/common crates/tui/tests/common
   git mv tests/web.rs addons/web/tests/web.rs
   git mv tests/web_manager.rs addons/web/tests/manager.rs
   git mv tests/vocdoni_manager.rs addons/vocdoni/tests/manager.rs
   git mv tests/vocdoni_web.rs addons/vocdoni/tests/web.rs
   git mv tests/integration.rs addons/api/tests/integration.rs
   ```
   `mod common;` becomes `#[path = "../../../crates/tui/tests/common/mod.rs"] mod common;` in the
   four addon files and `#[path = "../crates/tui/tests/common/mod.rs"] mod common;` in
   `tests/screenshots.rs`. `env!("CARGO_MANIFEST_DIR")` in these files is still the repo root.
   Cargo.toml gets the `[[test]]` table below.
   Check: `rtk proxy grep -rniE 'vocdoni|claude-proxy|claude\.vocdoni' crates/` → empty
   (`python3 scripts/check_rules.py`'s A1 now scans `crates/tui/tests`).

Cargo.toml at P4.1's end (complete):

```toml
[package]
name = "claudio"
version = "0.8.0"
edition = "2021"
description = "Drop-in `claude` wrapper emulating `-p`, with an optional OpenAI-compatible API server (--api) routed through the same PTY-driven backend (responsible-disclosure research)"
# Every integration suite is listed below. They drive the claudio binary, so they are this
# package's targets even where the file lives next to the crate it tests (P4 plan, D1).
autotests = false

[[bin]]
name = "claudio"
path = "src/main.rs"

[[test]]
name = "manager"
path = "crates/tui/tests/manager.rs"

[[test]]
name = "web"
path = "addons/web/tests/web.rs"
required-features = ["web"]

[[test]]
name = "web_manager"
path = "addons/web/tests/manager.rs"
required-features = ["web"]

[[test]]
name = "vocdoni_manager"
path = "addons/vocdoni/tests/manager.rs"
required-features = ["vocdoni"]

[[test]]
name = "vocdoni_web"
path = "addons/vocdoni/tests/web.rs"
required-features = ["vocdoni", "web"]

[[test]]
name = "integration"
path = "addons/api/tests/integration.rs"
required-features = ["api"]

[[test]]
name = "remote"
path = "tests/remote.rs"
required-features = ["diag", "vocdoni"]

[[test]]
name = "screenshots"
path = "tests/screenshots.rs"
required-features = ["vocdoni"]

[features]
default = ["api", "web", "vocdoni"]
# The addons this binary links (docs/addons.md). Without any, the core alone (ACCEPTANCE A4).
api = []
web = []
vocdoni = []
# Enables diagnostic / test-harness CLI subcommands (`__bootstrap`,
# `__connect-check`, `__remote-session`, `__ssh-hosts`, `__remote-manifest`).
# Not included in production release builds.
diag = []

# [dependencies], [build-dependencies], [dev-dependencies], [profile.*]: unchanged from P3's end.
```

(Write the unchanged tables out in full; only the sections shown change. If the step 8 split leaves
`web_manager` or a features list different from the above, the table follows the empirical result.)

Run at the end: S-unit; `for f in "" api web vocdoni; do cargo test --locked --no-default-features
--features "$f" --bin claudio || break; done` (every subset's unit tests pass); the five suite
commands of step 8; S-api; S-shots; `cargo test --locked --features diag --test remote --
--test-threads=1` (t1/t7/t8); S-check. Paste the `rules:` line (totals equal to the start's).

---

## P4.2: the workspace, claudio-core, the crate stubs and the slots

Model: strong   Runs on: refactor/core-addons (main checkout)
Files: Cargo.toml, Cargo.lock, crates/core/** (new, by `git mv`), crates/tui/{Cargo.toml,
src/lib.rs}, addons/{api,web,vocdoni}/{Cargo.toml,src/lib.rs}, src/main.rs, src/addons/mod.rs (a
comment per slot), tests/fixtures/stream{1,2}.raw → crates/core/tests/fixtures/, Makefile
(`check`, `test-unit`), .github/workflows/ci.yml (`build-and-test`'s test line),
scripts/size-baseline.txt

Goal: claudio-core is a library crate, and the root binary (still holding the TUI and the addons in
`src/`) builds against it. Every other member exists with its final manifest, the lock is final, and
the four crate moves can start in parallel.

### Commit 1: `refactor(core): claudio-core is the workspace's library crate` (move commit)

Moves:

```bash
mkdir -p crates/core/src crates/core/tests/fixtures
for m in addon app claude daemon paths proto remote term; do git mv src/$m crates/core/src/$m; done
for f in client config freshness paths proto release text upgrade version; do git mv src/$f.rs crates/core/src/$f.rs; done
git mv tests/fixtures/stream1.raw tests/fixtures/stream2.raw crates/core/tests/fixtures/
```

New `crates/core/src/lib.rs`:

```rust
//! claudio-core: the generic core of claudio. The daemon protocol and the
//! session daemon, paths and config, what claudio knows about `claude`, the
//! terminal model, the client, SSH remotes and freshness, release and
//! upgrade, the addon API (`addon`) and the layer both front-ends share
//! (`app`). It names no addon and draws nothing (ACCEPTANCE A1, D7).

pub mod addon;
pub mod app;
pub mod claude;
pub mod client;
pub mod config;
pub mod daemon;
pub mod freshness;
pub mod paths;
pub mod proto;
pub mod release;
pub mod remote;
pub mod term;
pub mod text;
pub mod upgrade;
pub mod version;
```

Edits the move needs (and nothing else):
- Visibility. These are reached from `src/tui` and `src/addons` (still in the root crate) or from
  their tests. Found by grep at 307f801; the compiler (E0603, E0616, `private_interfaces`) has
  the final word.
  - `app/mod.rs`: `pub(crate) mod {addons, browser, config_edit, connections, extensions, label,
    launch, snippets, state}` → `pub mod`.
  - `app/config_edit.rs`: `edit_text` (and `edit_section` if not `pub`) → `pub`.
    `app/launch/mod.rs`: `run_within` → `pub`.
  - Test fixtures: `addon/mod.rs`, `app/mod.rs`, `app/addons/mod.rs` and `daemon/mod.rs` declare
    `#[cfg(any(test, feature = "test-support"))] pub mod test_support;`. In those four
    `test_support.rs` files every `pub(crate)` becomes `pub` (TestAddon, Script, fake_host,
    next_request, reply, TestDaemon, Client, hello, within, private_temp_dir, raw, request, start,
    BANNER, READ_TIMEOUT, make_private, …). `daemon/host.rs`'s
    `#[cfg(test)] pub(crate) fn write_executable` → `#[cfg(any(test, feature = "test-support"))]
    pub fn`. `paths.rs`'s `#[cfg(test)] pub fn socket_test_dir` →
    `#[cfg(any(test, feature = "test-support"))]` (the web's tests use it).
  - Any other `pub(crate)` item the build reports as reached from `src/tui`, `src/addons` or
    `src/main.rs` → `pub`. List them in the commit message.
- Paths. `app/state.rs`'s fixture becomes
  `PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/state-v0.8.0.json"))`.
  `app/extensions/manifest.rs`'s `include_str!("../../../examples/…")` →
  `include_str!("../../../../../examples/extensions/keys/extension.toml")`.
- `src/main.rs`: the fifteen core `mod` lines go. The root's remaining modules reach the core
  through one aliasing `use`, and the TUI gets two slots (D3):

  ```rust
  // The core library. The TUI and the addons, still modules of this package
  // until P4.3-P4.6, reach it through these names; P4.7 removes this line.
  // Allowed unused: a build without some addons uses fewer of them.
  #[allow(unused_imports)]
  use claudio_core::{
      addon, app, claude, client, config, daemon, freshness, paths, proto, release, remote, term,
      text, upgrade, version,
  };

  mod addons;

  // P4.3 slot: becomes `use claudio_tui::plain::args as plain;`
  mod plain;

  // P4.3 slot: becomes `use claudio_tui as tui;`
  mod tui;
  ```
  Keep the root's `[dev-dependencies] claudio-core = { …, features = ["test-support"] }`. It is
  what lets the tests of `src/main.rs` and of the root modules see
  `claudio_core::*::test_support`. The `#[cfg(any(test, feature = "test-support"))]` is evaluated
  in claudio-core, where `cfg(test)` is never set for a dependency. `main.rs`'s test reaches
  `crate::addon::test_support::TestAddon` through the alias.
- `src/addons/mod.rs`: for each of its three `pub mod` lines, add one comment naming the slot,
  between the `#[cfg]` line and the `pub mod` line (never above the `#[cfg]`), e.g.
  `// P4.4 slot: becomes \`pub use claudio_api as api;\``. The blank lines between the three
  blocks stay.
- Cargo.toml (complete, below), the four stubs (complete, below), `Cargo.lock` (first build with
  `cargo build --offline`, then everything `--locked`; `git diff Cargo.lock | grep -E
  '^[-+](name|version)'` shows only the five new `claudio-*` packages).
- Baseline: rename the core entries, then group the file by destination (the parallel tasks
  edit disjoint blocks; P4.7 re-sorts):

  ```bash
  python3 scripts/check_rules.py --write-baseline    # totals printed must equal P4.1's
  python3 - <<'EOF'
  p = "scripts/size-baseline.txt"
  rows = [l.rstrip("\n") for l in open(p) if l.strip() and not l.startswith("#")]
  groups = [("claudio-core", ("crates/core/",)),
            ("claudio-tui (src/tui and src/plain.rs until P4.3)", ("src/tui/", "src/plain.rs")),
            ("claudio-api (src/addons/api until P4.4)", ("src/addons/api/",)),
            ("claudio-web (src/addons/web until P4.5)", ("src/addons/web/",)),
            ("claudio-vocdoni (src/addons/vocdoni until P4.6)", ("src/addons/vocdoni/",)),
            ("claudio (root)", ("",))]
  out = ["# Over-budget items (ACCEPTANCE.md A8, D2). Only shrinks; see scripts/check_rules.py.",
         "# P4: grouped by destination crate so the parallel moves edit disjoint blocks; P4.7 re-sorts."]
  left = rows
  for title, prefixes in groups:
      mine = [r for r in left if r.split(" ", 1)[1].startswith(prefixes)]
      left = [r for r in left if r not in mine]
      out += ["", f"# -- {title}"] + mine
  open(p, "w").write("\n".join(out) + "\n")
  EOF
  python3 scripts/check_rules.py                      # green, same totals
  ```

### Commit 2: `style(core): what clippy asks of a library's public API`
Exporting the core can surface lints that a binary never reports: `new_without_default`,
`len_without_is_empty`, `private_interfaces`/`private_bounds`, `result_unit_err`. Fix them
without behaviour change (`impl Default` delegating to `new`, `is_empty`, a type made `pub`). If
nothing fires, skip this commit.

### Commit 3: `docs(core): doc comments on the items the split makes public` (D6)
Every item made `pub` in commit 1 that lacks a `///` comment gets one that says why it exists.
Doc comments only.

### Commit 4: `build: make check and CI test every crate of the workspace`
- Makefile `check`: `fmt --check` → `fmt --all --check`; `clippy --locked --all-targets
  --all-features` → `clippy --locked --workspace --all-targets --all-features`; `test --locked
  --bin claudio` → `test --locked --workspace --lib --bins`. `test-unit`: `$(CARGO) test --locked
  --workspace --lib --bins`.
- `.github/workflows/ci.yml` `build-and-test`: `cargo test --locked --bin claudio` →
  `cargo test --locked --workspace --lib --bins`.

### Root Cargo.toml at P4.2 (complete)

```toml
# The claudio workspace: the `claudio` binary (this package) and its crates.
# `[workspace.package]` stays first: release.yml reads the release version from
# the first `version =` line of this file.
[workspace]
members = ["crates/core", "crates/tui", "addons/api", "addons/web", "addons/vocdoni"]
resolver = "2"

[workspace.package]
version = "0.8.0"
edition = "2021"

[workspace.dependencies]
claudio-core = { path = "crates/core" }
claudio-tui = { path = "crates/tui" }
claudio-api = { path = "addons/api" }
claudio-web = { path = "addons/web" }
claudio-vocdoni = { path = "addons/vocdoni" }
alacritty_terminal = "0.26"
axum = { version = "0.8", features = ["ws"] }
base64 = "0.22"
crossterm = { version = "0.29", features = ["event-stream"] }
flate2 = "1"
futures = "0.3"
libc = "0.2"
portable-pty = "0.8"
# The reader view's markdown (claude's answers). Pure Rust; no default
# features (no HTML output). Pinned exactly: the reader's hostile-input
# bounds were measured against this version.
pulldown-cmark = { version = "=0.13.6", default-features = false }
ratatui = "0.30"
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls", "json"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
sha2 = "0.10"
thiserror = "2"
tokio = { version = "1", features = ["full"] }
# `claudio web`'s WebSocket in the web suites: the version axum's `ws` feature
# pulls, so one copy builds.
tokio-tungstenite = "0.29"
toml = "0.9"
tower-http = { version = "0.6", features = ["trace", "cors"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
unicode-segmentation = "1"
unicode-width = "0.2"
uuid = { version = "1", features = ["v4", "serde"] }
vte = "0.13"

[package]
name = "claudio"
version.workspace = true
edition.workspace = true
description = "Drop-in `claude` wrapper emulating `-p`, with an optional OpenAI-compatible API server (--api) routed through the same PTY-driven backend (responsible-disclosure research)"
# Every integration suite is listed below. They drive the claudio binary, so they are this
# package's targets even where the file lives next to the crate it tests (P4 plan, D1).
autotests = false

[[bin]]
name = "claudio"
path = "src/main.rs"

# [[test]] manager, web, web_manager, vocdoni_manager, vocdoni_web, integration, remote,
# screenshots: exactly as at P4.1's end (write them out).

[features]
default = ["api", "web", "vocdoni"]
# The addons this binary links (docs/addons.md). Without any, the core alone (ACCEPTANCE A4).
api = ["dep:claudio-api"]
web = ["dep:claudio-web"]
vocdoni = ["dep:claudio-vocdoni"]
# Enables diagnostic / test-harness CLI subcommands (`__bootstrap`,
# `__connect-check`, `__remote-session`, `__ssh-hosts`, `__remote-manifest`).
# Not included in production release builds.
diag = ["claudio-core/diag", "claudio-vocdoni?/diag"]

[dependencies]
claudio-core.workspace = true
claudio-tui.workspace = true
claudio-api = { workspace = true, optional = true }
claudio-web = { workspace = true, optional = true }
claudio-vocdoni = { workspace = true, optional = true }
# The TUI and the addons are still modules of this package until P4.3-P4.6;
# P4.7 drops what then has no user here.
alacritty_terminal.workspace = true
axum.workspace = true
base64.workspace = true
crossterm.workspace = true
flate2.workspace = true
futures.workspace = true
libc.workspace = true
portable-pty.workspace = true
pulldown-cmark.workspace = true
ratatui.workspace = true
reqwest.workspace = true
serde.workspace = true
serde_json.workspace = true
sha2.workspace = true
thiserror.workspace = true
tokio.workspace = true
tokio-stream = "0.1"
toml.workspace = true
tower = "0.5"
tower-http.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
unicode-segmentation.workspace = true
unicode-width.workspace = true
uuid.workspace = true
vte.workspace = true

# build.rs (the web page) moves to addons/web in P4.5; P4.7 drops these.
[build-dependencies]
base64.workspace = true
flate2.workspace = true
sha2.workspace = true

[dev-dependencies]
claudio-core = { workspace = true, features = ["test-support"] }
tokio = { workspace = true, features = ["test-util"] }
tokio-tungstenite.workspace = true
uuid.workspace = true

[profile.release]
strip = true

# Every daemon start and connect hashes the claudio binary (the freshness
# check); unoptimized, that takes seconds on a debug build.
[profile.dev.package.sha2]
opt-level = 3
```

### crates/core/Cargo.toml (complete, final)

```toml
[package]
name = "claudio-core"
description = "claudio's generic core: the daemon and its protocol, the client, SSH, claude install and update, the addon API and the shared app layer"
version.workspace = true
edition.workspace = true
publish = false
autotests = false

[features]
# Fixtures other crates' tests use (TestAddon, the daemon harness, the fake
# host, scratch socket dirs). Enabled only by dev-dependencies, never in a release.
test-support = []
# The SSH test harness's diag words (`__bootstrap`, …); never in a release.
diag = []

[dependencies]
alacritty_terminal.workspace = true
futures.workspace = true
libc.workspace = true
portable-pty.workspace = true
reqwest.workspace = true
serde.workspace = true
serde_json.workspace = true
sha2.workspace = true
thiserror.workspace = true
tokio.workspace = true
toml.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
unicode-width.workspace = true
uuid.workspace = true
vte.workspace = true

[dev-dependencies]
# A fake release server in claude::update's and upgrade's tests (D7: tests only).
axum.workspace = true
tokio = { workspace = true, features = ["test-util"] }
```

### Stubs (complete manifests, final; `src/lib.rs` is one line until the task moves the code)

`crates/tui/Cargo.toml`:

```toml
[package]
name = "claudio-tui"
description = "claudio's console session manager (bare `claudio`) and `claudio --plain`"
version.workspace = true
edition.workspace = true
publish = false
# crates/tui/tests/ holds the manager suite, a target of the root package (it drives
# the claudio binary); `cargo test -p claudio-tui` runs this crate's unit tests.
autotests = false

[dependencies]
claudio-core.workspace = true
alacritty_terminal.workspace = true
crossterm.workspace = true
futures.workspace = true
pulldown-cmark.workspace = true
ratatui.workspace = true
tokio.workspace = true
unicode-segmentation.workspace = true
unicode-width.workspace = true
uuid.workspace = true

[dev-dependencies]
claudio-core = { workspace = true, features = ["test-support"] }
serde_json.workspace = true
tokio = { workspace = true, features = ["test-util"] }
```

`addons/api/Cargo.toml`:

```toml
[package]
name = "claudio-api"
description = "claudio addon: `-p` print emulation and the OpenAI-compatible `--api` server"
version.workspace = true
edition.workspace = true
publish = false
# addons/api/tests/ is a target of the root package (it drives the claudio binary).
autotests = false

[dependencies]
claudio-core.workspace = true
axum.workspace = true
futures.workspace = true
portable-pty.workspace = true
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
tokio.workspace = true
tower-http.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
uuid.workspace = true
```

`addons/web/Cargo.toml` (cargo finds `build.rs` itself once P4.5 moves it in; no `build =` key, so
the stub builds without one):

```toml
[package]
name = "claudio-web"
description = "claudio addon: `claudio web`, the session manager in a browser (server and page)"
version.workspace = true
edition.workspace = true
publish = false
# addons/web/tests/ are targets of the root package (they drive the claudio binary).
autotests = false

[dependencies]
claudio-core.workspace = true
axum.workspace = true
base64.workspace = true
flate2.workspace = true
futures.workspace = true
serde.workspace = true
serde_json.workspace = true
sha2.workspace = true
tokio.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
uuid.workspace = true

# build.rs embeds web/ gzipped and hashes the import map for the CSP.
[build-dependencies]
base64.workspace = true
flate2.workspace = true
sha2.workspace = true

[dev-dependencies]
claudio-core = { workspace = true, features = ["test-support"] }
# A test AddonCtx returns these (the addon API's http() and config()).
reqwest.workspace = true
toml.workspace = true
```

`addons/vocdoni/Cargo.toml`:

```toml
[package]
name = "claudio-vocdoni"
description = "claudio addon: the claude-proxy client and the Vocdoni team plugins"
version.workspace = true
edition.workspace = true
publish = false
# addons/vocdoni/tests/ are targets of the root package (they drive the claudio binary).
autotests = false

[features]
# `__remote-manifest`, a diag word of the SSH test harness; never in a release.
diag = []

[dependencies]
claudio-core.workspace = true
libc.workspace = true
reqwest.workspace = true
serde.workspace = true
serde_json.workspace = true
sha2.workspace = true
thiserror.workspace = true
tokio.workspace = true
toml.workspace = true
tracing.workspace = true
uuid.workspace = true

[dev-dependencies]
claudio-core = { workspace = true, features = ["test-support"] }
# The fake proxy and fake daemons of the unit tests.
axum.workspace = true
futures.workspace = true
tokio = { workspace = true, features = ["test-util"] }
```

Stub `src/lib.rs` files, one line each:
`//! claudio-tui: the console session manager (P4.3 moves the code here).`, and likewise for
api (P4.4), web (P4.5) and vocdoni (P4.6).

The dependency lists come from a scan of `use <crate>::` at 307f801. If a move finds one missing,
that task adds it to its own manifest and lock entry (D3).

Run at the end: S-unit, `cargo test --locked -p claudio-core`, `cargo build --locked
--no-default-features`, S-manager, S-manager-core, S-web, S-vocdoni, S-api, S-shots, S-remote
(t1/t7/t8), S-e2e, S-check. Also the D7 and test-support `cargo tree` checks (Global
Constraints), and A2's current check (`python3 scripts/check_rules.py`; P4.7 tightens it).
Report: the visibility list, the `rules:` line (totals unchanged), `git rev-parse HEAD` (the base
of the worktrees).

---

## P4.3: claudio-tui (parallel; worktree `p4/tui`)

Model: standard   Base: P4.2's last commit
Owns: see the parallelism table. Must not touch: root Cargo.toml, crates/core/**, src/addons/**,
any tests directory, Makefile, CI, docs, the other baseline blocks.

Commit 1, `refactor(tui): the console manager moves to crates/tui (claudio-tui)` (move commit):

```bash
git rm -q crates/tui/src/lib.rs                      # the stub
git mv src/tui/mod.rs crates/tui/src/lib.rs
for e in src/tui/*; do git mv "$e" crates/tui/src/; done   # every remaining file and directory
mkdir -p crates/tui/src/plain
git mv src/plain.rs crates/tui/src/plain/args.rs     # D6: a child of tui/plain.rs
find crates/tui/src -name '*.rs' -exec sed -i \
  -e "s/\bcrate::\($CORE\)\b/claudio_core::\1/g" -e 's/\bcrate::tui::/crate::/g' {} +
```

Then by hand:
- `lib.rs`: `mod plain;` → `pub mod plain;`. `run` and `run_plain` stay `pub`. The crate doc is
  tui/mod.rs's `//!` header.
- `plain.rs` gains `pub mod args;` (plain.rs is under budget).
- `plain/args.rs`: `use crate::tui;` goes; `tui::run_plain(…)` → `crate::run_plain(…)`. Its module
  doc keeps saying what it parses.
- `src/main.rs`: the two slots become `use claudio_tui::plain::args as plain;` and
  `use claudio_tui as tui;` (their comments go). Nothing else in main.rs changes.
- `cargo fmt --all` (import order only).
- Baseline, the `claudio-tui` block only:
  `sed -i -e 's#^\(file\|fn\|unwrap\) src/tui/mod\.rs#\1 crates/tui/src/lib.rs#'
  -e 's#^\(file\|fn\|unwrap\) src/tui/#\1 crates/tui/src/#'
  -e 's#^\(file\|fn\|unwrap\) src/plain\.rs#\1 crates/tui/src/plain/args.rs#'
  scripts/size-baseline.txt`; the block's `# -- …` title stays as written (P4.7 rewrites the file).

Commit 2 (only if clippy asks), `style(tui): what clippy asks of a library's public API`.

Verify (in the worktree, `CARGO_TARGET_DIR` set):
```bash
cargo build --locked -p claudio-tui && cargo test --locked -p claudio-tui
cargo build --locked && cargo build --locked --no-default-features
cargo test --locked --workspace --lib --bins
flock ~/.cache/claudio/p4/suites.lock cargo test --locked --no-default-features --test manager -- --test-threads=1
flock ~/.cache/claudio/p4/suites.lock cargo test --locked --test manager -- --test-threads=1
PATH=$TC:$PATH $TC/cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo fmt --all --check && python3 scripts/check_rules.py     # green, totals unchanged
rtk proxy grep -rn 'crate::tui\b' crates/tui/src             # empty
git diff --stat "$B"..HEAD -- . ':!crates/tui' ':!src/tui' ':!src/plain.rs'
                                                              # only src/main.rs, scripts/size-baseline.txt
                                                              # (and Cargo.lock if a dep was added)
```

---

## P4.4: claudio-api (parallel; worktree `p4/api`)

Model: standard   Base: P4.2's last commit
Must not touch: root Cargo.toml, crates/**, src/main.rs, src/tui, other addons, tests, Makefile,
CI, docs, other baseline blocks.

Commit 1, `refactor(api): the print and API server addon moves to addons/api (claudio-api)`:

```bash
git rm -q addons/api/src/lib.rs
git mv src/addons/api/mod.rs addons/api/src/lib.rs
for e in src/addons/api/*; do git mv "$e" addons/api/src/; done
find addons/api/src -name '*.rs' -exec sed -i \
  -e "s/\bcrate::\($CORE\)\b/claudio_core::\1/g" -e 's/\bcrate::addons::api::/crate::/g' {} +
```

By hand: `lib.rs` drops the `#[allow(clippy::module_inception)]` and its two-line comment above
`pub mod api;` (a crate root is not a module named `api`, so nothing triggers it; D4). Clippy
must stay clean. `src/addons/mod.rs`'s `api` slot → `pub use claudio_api as api;` (its comment
goes, the `#[cfg(feature = "api")]` line stays). `cargo fmt --all`. Baseline block `claudio-api`:
`sed -i -e 's#^\(file\|fn\|unwrap\) src/addons/api/mod\.rs#\1 addons/api/src/lib.rs#'
-e 's#^\(file\|fn\|unwrap\) src/addons/api/#\1 addons/api/src/#' scripts/size-baseline.txt`.

Commit 2 (only if clippy asks), `style(api): …`.

Verify:
```bash
cargo build --locked -p claudio-api && cargo test --locked -p claudio-api
cargo build --locked && cargo build --locked --no-default-features --features api
cargo test --locked --no-default-features --features api --bins     # root routing with api alone
cargo test --locked --workspace --lib --bins
cargo test --locked --test integration                               # compiles; no-op without CLAUDIO_E2E
target=$CARGO_TARGET_DIR/debug/claudio; $target --help >/dev/null 2>&1; echo $?   # help appendix still printed
clippy (workspace, as P4.3); cargo fmt --all --check; python3 scripts/check_rules.py
rtk proxy grep -rn 'crate::addons' addons/api/src                    # empty
```
The owner may run `CLAUDIO_E2E=1 CLAUDIO_CADENCE=0 cargo test --locked --test integration --
--test-threads=1` (real claude, costs tokens) at the Merge.

---

## P4.5: claudio-web (parallel; worktree `p4/web`)

Model: standard   Base: P4.2's last commit
Must not touch: root Cargo.toml (its `[build-dependencies]` stay until P4.7), crates/**,
src/main.rs, src/tui, other addons, tests other than its own paths, CI, docs, the Makefile except
`test-web`, other baseline blocks. `web/vendor/**`, `web/fonts/**` and `web/index.html` move
byte-identical.

Commit 1, `refactor(web): the web addon, its page and build script move to addons/web (claudio-web)`:

```bash
git rm -q addons/web/src/lib.rs
git mv src/addons/web/mod.rs addons/web/src/lib.rs
for e in src/addons/web/*; do git mv "$e" addons/web/src/; done
git mv build.rs addons/web/build.rs
git mv web addons/web/web
find addons/web/src -name '*.rs' -exec sed -i \
  -e "s/\bcrate::\($CORE\)\b/claudio_core::\1/g" -e 's/\bcrate::addons::web::/crate::/g' {} +
```

By hand:
- `build.rs`: no code change. It reads `web/` relative to its package (`addons/web/web`). Its
  three mentions of `src/addons/web/assets.rs` (lines 3, 131, 143) become `src/assets.rs` (a
  comment path naming a moved file, allowed in a move commit).
- `addons/web/src/state.rs`'s fixture → `concat!(env!("CARGO_MANIFEST_DIR"),
  "/../../tests/fixtures/web-state-v0.8.0.json")` (D8).
- `addons/web/web/e2e/fixtures/server.mjs:16`: `path.resolve(here, '../../..')` →
  `path.resolve(here, '../../../../..')` (the repo root, for `target/debug/claudio`).
- Makefile `test-web`: `node --test addons/web/web/tests/*.test.mjs`, `cd addons/web/web/e2e &&
  …`.
- `src/addons/mod.rs`'s `web` slot → `pub use claudio_web as web;`.
- `cargo fmt --all`. Baseline block `claudio-web`:
  `sed -i -e 's#^\(file\|fn\|unwrap\) src/addons/web/mod\.rs#\1 addons/web/src/lib.rs#'
  -e 's#^\(file\|fn\|unwrap\) src/addons/web/#\1 addons/web/src/#' scripts/size-baseline.txt`.
- `CLAUDIO_BUILD_LABEL` is emitted by the moved build script for claudio-web, its only reader
  (`env!` in `src/lib.rs`). `make dev` sets it in the environment as before.

Commit 2 (only if clippy asks), `style(web): …`.

Verify:
```bash
cargo build --locked -p claudio-web && cargo test --locked -p claudio-web
cargo build --locked && cargo build --locked --no-default-features --features web
cargo test --locked --no-default-features --features web --bins
cargo test --locked --workspace --lib --bins
flock ~/.cache/claudio/p4/suites.lock cargo test --locked --test web --test web_manager -- --test-threads=1
flock ~/.cache/claudio/p4/suites.lock env CLAUDIO_BIN=$CARGO_TARGET_DIR/debug/claudio make test-web   # node + Playwright, zero CSP violations
git diff --find-renames=100% --name-status "$B"..HEAD -- web addons/web/web | rtk proxy grep -v '^R100'
                                                              # empty: the page moved byte-identical
ls $CARGO_TARGET_DIR/debug/build/claudio-web-*/out/web/*.gz | wc -l   # ≥ 200 (the 276 tracked files minus tests/, e2e/, dev/, npm files: the real page is embedded, not the stub)
# no curl needed: the web suite and the e2e (zero CSP violations) exercise the embedded page
clippy (workspace); cargo fmt --all --check; python3 scripts/check_rules.py
rtk proxy grep -rn 'crate::addons' addons/web/src                    # empty
```
After the merge, the main checkout may keep ignored leftovers at `web/e2e/node_modules` and
`web/e2e/test-results`. `git status --ignored web` shows them, and the controller removes them.

---

## P4.6: claudio-vocdoni (parallel; worktree `p4/vocdoni`)

Model: standard   Base: P4.2's last commit
Must not touch: root Cargo.toml, crates/**, src/main.rs, src/tui, other addons, tests, Makefile,
CI, docs, other baseline blocks.

Commit 1, `refactor(vocdoni): the Vocdoni addon moves to addons/vocdoni (claudio-vocdoni)`:

```bash
git rm -q addons/vocdoni/src/lib.rs
git mv src/addons/vocdoni/mod.rs addons/vocdoni/src/lib.rs
for e in src/addons/vocdoni/*; do git mv "$e" addons/vocdoni/src/; done
find addons/vocdoni/src -name '*.rs' -exec sed -i \
  -e "s/\bcrate::\($CORE\)\b/claudio_core::\1/g" -e 's/\bcrate::addons::vocdoni::/crate::/g' {} +
```

By hand: `launch/tests.rs`'s fixture dir →
`PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures"))` (D8).
`#[cfg(feature = "diag")]` now names this crate's `diag` feature, which the root's `diag` enables
(`claudio-vocdoni?/diag`, P4.2). `include_str!("testdata/…")` moves with its file.
`src/addons/mod.rs`'s `vocdoni` slot → `pub use claudio_vocdoni as vocdoni;`. `cargo fmt --all`.
Baseline block `claudio-vocdoni`: `sed -i -e 's#^\(file\|fn\|unwrap\) src/addons/vocdoni/mod\.rs#\1
addons/vocdoni/src/lib.rs#' -e 's#^\(file\|fn\|unwrap\) src/addons/vocdoni/#\1 addons/vocdoni/src/#'
scripts/size-baseline.txt`.

Commit 2 (only if clippy asks), `style(vocdoni): …` (expect `new_without_default` on `Vocdoni`:
`impl Default` delegating to `new`).

Verify:
```bash
cargo build --locked -p claudio-vocdoni && cargo test --locked -p claudio-vocdoni
cargo test --locked -p claudio-vocdoni --features diag
cargo build --locked && cargo build --locked --no-default-features --features vocdoni
cargo test --locked --no-default-features --features vocdoni --bins
cargo test --locked --workspace --lib --bins
flock ~/.cache/claudio/p4/suites.lock cargo test --locked --test vocdoni_manager --test vocdoni_web -- --test-threads=1
flock ~/.cache/claudio/p4/suites.lock cargo test --locked --features diag --test remote -- --test-threads=1   # t1/t7/t8
clippy (workspace); cargo fmt --all --check; python3 scripts/check_rules.py
rtk proxy grep -rn 'crate::addons' addons/vocdoni/src                # empty
```
The gated `CLAUDIO_PROXY_TEST=1` suite (claude-proxy from `../claude-proxy`, needs `go`) runs at
the Merge if the owner's machine has it.

---

## P4.7: integration: main.rs wiring, the root's own dependencies, the rules, CI, docs

Model: strong   Runs on: refactor/core-addons after the four merges (see Merge 1-2)

Commits:

1. `build: main.rs links the TUI and the addons as crates`:
   - `src/main.rs`: the aliasing `use claudio_core::{…}` and the slots go. main.rs names
     `claudio_core::…`, `claudio_tui::run`, and `claudio_tui::plain::args::run` directly.
     `src/addons/mod.rs` is deleted (`git rm`), and its `all()` becomes `fn addons() ->
     Vec<Arc<dyn Addon>>` in main.rs, with the same cfg'd `vec!` (vocdoni, web, api) and its
     `addons_all_registers` test.
   - New root unit test, compiled only without the web addon (P3.18's "P4 checks it"):
     `#[cfg(not(feature = "web"))] fn a_build_without_the_web_addon_asks_claude_for_no_lenses()`.
     It composes a direct launch with this build's registry, through the same `app::launch`
     helpers the core's launch tests use, and asserts that `extensions.lenses` names neither
     `latex` nor `mermaid`.
   - R24 (pre-flight F1): `fn refuse_print_without_api(args: &[String]) -> Option<ExitCode>`.
     It is under 20 lines and `#[cfg(not(feature = "api"))]`; `route` calls it after
     `addons.dispatch` and before the help check. If any argument before a bare `--` is `-p`,
     `--print` or `--api`, it prints
     `claudio: print mode and the API server need the api addon; this build has none` on stderr
     and returns `ExitCode::from(2)`. The default build's `route` is unchanged. A root unit test
     compiled with `#[cfg(not(feature = "api"))]`, `a_build_without_the_api_addon_refuses_print_mode`,
     asserts exit 2 for all three flags and that `claudio -- -p` (a prompt) is not refused. The
     matrix cells core only, web and vocdoni run it (`--bins`).
   - `main.rs` stays under 800 lines and every function under 80.
2. `build: the root package keeps only its own dependencies` (D9). Root `[dependencies]`:
   the five `claudio-*` and `tokio.workspace = true`. No `[build-dependencies]`. Root
   `[dev-dependencies]` is what the integration suites use. Cargo.lock: `cargo build --offline`
   then `--locked`, and `git diff Cargo.lock | grep -E '^[-+]version'` shows no external version
   change (`tokio-stream` may leave the lock). The final root Cargo.toml is below.
3. `build(rules): A2 checks dependency kinds and the workspace's crates`
   (`scripts/check_rules.py`):
   - Normal and build dependencies: core → none of ours; tui → {core}; an addon → {core}.
   - Dev-dependencies: core → none; tui → {core}; an addon → {core, tui, another addon}. The
     binary may depend on anything.
   - Once `crates/` exists, each of claudio, claudio-core, claudio-tui, claudio-api, claudio-web
     and claudio-vocdoni must be a package; a missing one is reported.

   Replacement for `dependency_violations` (the rest of the script is unchanged):

   ```python
   ADDONS = {"claudio-api", "claudio-web", "claudio-vocdoni"}
   PACKAGES = {"claudio", CORE, TUI} | ADDONS


   def dependency_violations():
       try:
           meta = subprocess.run(
               ["cargo", "metadata", "--format-version", "1", "--no-deps", "--offline"],
               cwd=ROOT, capture_output=True, text=True, check=True,
           )
       except (OSError, subprocess.CalledProcessError) as e:
           return [f"A2 cargo metadata failed: {e}"]
       packages = json.loads(meta.stdout)["packages"]
       names = {p["name"] for p in packages}
       bad = []
       if os.path.isdir(os.path.join(ROOT, "crates")):
           bad += [f"A2 the workspace has no package {n}" for n in sorted(PACKAGES - names)]
       for p in packages:
           me = p["name"]
           if me == "claudio":
               continue
           for d in p["dependencies"]:
               dep, dev = d["name"], d.get("kind") == "dev"
               if dep not in names or dep == me:
                   continue
               if me == CORE:
                   allowed = set()
               elif me == TUI:
                   allowed = {CORE}
               else:  # an addon: a dev-dependency may be the TUI or another addon (tests)
                   allowed = (names - {"claudio", me}) if dev else {CORE}
               if dep not in allowed:
                   bad.append(f"A2 {me} {'dev-' if dev else ''}depends on {dep}")
       return bad
   ```
   Negative check, then discard: in a scratch worktree (`git worktree add --detach
   ~/.cache/claudio/p4/rules-neg HEAD`), add `claudio-tui.workspace = true` to
   `addons/web/Cargo.toml [dependencies]` and run the script. Expect `A2 claudio-web depends on
   claudio-tui` and exit 1. Remove the worktree.
   The same commit re-sorts the baseline: `python3 scripts/check_rules.py --write-baseline`,
   with totals equal to P4.2's.
4. `ci: the feature matrix on Linux and macOS` (Makefile + `.github/workflows/ci.yml`):

   ```make
   ## One cell of the feature matrix (ACCEPTANCE E5). FEATURES="" is the core alone (A4),
   ## which also runs the TUI's manager suite against that binary.
   ## cargo accepts --features "" (no feature).
   FEATURES ?=
   features:
   	$(CHECK_CARGO) clippy --locked -p claudio --no-default-features --features "$(FEATURES)" --all-targets -- -D warnings
   	$(CHECK_CARGO) build --locked -p claudio --no-default-features --features "$(FEATURES)"
   	$(CHECK_CARGO) test --locked -p claudio --no-default-features --features "$(FEATURES)" --bins
   	$(if $(FEATURES),,$(CHECK_CARGO) test --locked -p claudio --no-default-features --test manager -- --test-threads=1)

   ## The whole matrix: core only, each addon alone, all of them.
   matrix:
   	@for f in "" api web vocdoni api,web,vocdoni; do \
   		$(MAKE) --no-print-directory features FEATURES="$$f" || exit 1; done
   ```
   Add `features matrix` to `.PHONY` and the header list. `make check` keeps fmt --all, clippy
   --workspace --all-features, the rules, `build --locked --no-default-features` and
   `test --locked --workspace --lib --bins` (AC1). CI gains:

   ```yaml
     features:
       name: features [${{ matrix.features || 'core only' }}] (${{ matrix.os }})
       runs-on: ${{ matrix.os }}
       strategy:
         fail-fast: false
         matrix:
           os: [ubuntu-latest, macos-latest]
           features: ["", "api", "web", "vocdoni", "api,web,vocdoni"]
       steps:
         - uses: actions/checkout@v4
         - uses: dtolnay/rust-toolchain@stable
           with:
             components: clippy
         - uses: Swatinem/rust-cache@v2
         - name: make features
           run: make features FEATURES="${{ matrix.features }}"
           env:
             RUST_BACKTRACE: 1
   ```
   The core-only cells run the manager suite (a PTY and a fake claude, no network). If a hosted
   runner cannot run it, report that to the owner. Do not drop it silently.
5. `build(docker): the image builds the workspace`. `Dockerfile` copies `crates`, `addons` and
   `src` (it copied only `src` and missed `build.rs` and `web/`, so the image build fails today).
   `.dockerignore` gains `addons/web/web/e2e/node_modules` and `addons/web/web/e2e/test-results`.
   It also gains `!addons/web/web/**/*.md` after `*.md`, so the image embeds the same page as a
   native build (the vendored `LICENSES.md`, pre-flight F10).
6. `docs: the workspace layout, commands and paths` (F1; P5 rewrites the map in full):
   - CLAUDE.md: Commands, the "Test suites and gates" commands (target names unchanged), the
     unit-test line (`cargo test --workspace --lib --bins <name>` or `-p <crate>`), and every
     path in the architecture map (`src/tui/` → `crates/tui/src/`, `src/web`/`web/` →
     `addons/web/{src,web}/`, …). Also the D1/D2 test layout and the slotless `main.rs`.
   - README: build and test commands; a line on building without addons
     (`cargo build --no-default-features --features …`).
   - docs/web.md and DESIGN.md: `web/…` → `addons/web/web/…` (`CLAUDIO_WEB_DIR=addons/web/web`,
     `addons/web/web/styles/tokens.css`).
   - docs/addons.md: the crate names, the dependency rule and the `test-support` feature.
     docs/extensions.md: changed paths only. docs/print-and-api.md and README: changed paths,
     and that a build without the api addon refuses `-p`/`--print`/`--api` (exit 2; R24).
   - ACCEPTANCE.md "Checking": the sentence "A1 and A2 apply once the workspace has the core and
     addon crates" becomes "A2 checks dependency kinds (`cargo metadata`); `make matrix` runs the
     feature matrix (E5)". The commit message says why (the workspace now exists).

### Root Cargo.toml at P4.7 (complete, final)

```toml
# The claudio workspace: the `claudio` binary (this package) and its crates.
# `[workspace.package]` stays first: release.yml reads the release version from
# the first `version =` line of this file.
[workspace]
members = ["crates/core", "crates/tui", "addons/api", "addons/web", "addons/vocdoni"]
resolver = "2"

[workspace.package]
version = "0.8.0"
edition = "2021"

[workspace.dependencies]
# (unchanged from P4.2: the five claudio-* paths and the external crates)

[package]
name = "claudio"
version.workspace = true
edition.workspace = true
description = "Drop-in `claude` wrapper emulating `-p`, with an optional OpenAI-compatible API server (--api) routed through the same PTY-driven backend (responsible-disclosure research)"
# Every integration suite is listed below. They drive the claudio binary, so they are this
# package's targets even where the file lives next to the crate it tests (P4 plan, D1).
autotests = false

[[bin]]
name = "claudio"
path = "src/main.rs"

[[test]]
name = "manager"
path = "crates/tui/tests/manager.rs"

[[test]]
name = "web"
path = "addons/web/tests/web.rs"
required-features = ["web"]

[[test]]
name = "web_manager"
path = "addons/web/tests/manager.rs"
required-features = ["web"]

[[test]]
name = "vocdoni_manager"
path = "addons/vocdoni/tests/manager.rs"
required-features = ["vocdoni"]

[[test]]
name = "vocdoni_web"
path = "addons/vocdoni/tests/web.rs"
required-features = ["vocdoni", "web"]

[[test]]
name = "integration"
path = "addons/api/tests/integration.rs"
required-features = ["api"]

[[test]]
name = "remote"
path = "tests/remote.rs"
required-features = ["diag", "vocdoni"]

[[test]]
name = "screenshots"
path = "tests/screenshots.rs"
required-features = ["vocdoni"]

[features]
default = ["api", "web", "vocdoni"]
# The addons this binary links (docs/addons.md). Without any, the core alone (ACCEPTANCE A4).
api = ["dep:claudio-api"]
web = ["dep:claudio-web"]
vocdoni = ["dep:claudio-vocdoni"]
# Enables diagnostic / test-harness CLI subcommands (`__bootstrap`,
# `__connect-check`, `__remote-session`, `__ssh-hosts`, `__remote-manifest`).
# Not included in production release builds.
diag = ["claudio-core/diag", "claudio-vocdoni?/diag"]

[dependencies]
claudio-core.workspace = true
claudio-tui.workspace = true
claudio-api = { workspace = true, optional = true }
claudio-web = { workspace = true, optional = true }
claudio-vocdoni = { workspace = true, optional = true }
# `claudio sessions`' one-shot runtime.
tokio.workspace = true

# The integration suites (crates/tui/tests, addons/*/tests, tests/).
[dev-dependencies]
claudio-core = { workspace = true, features = ["test-support"] }
alacritty_terminal.workspace = true
futures.workspace = true
libc.workspace = true
portable-pty.workspace = true
serde_json.workspace = true
sha2.workspace = true
tokio = { workspace = true, features = ["test-util"] }
tokio-tungstenite.workspace = true
uuid.workspace = true

[profile.release]
strip = true

# Every daemon start and connect hashes the claudio binary (the freshness
# check); unoptimized, that takes seconds on a debug build.
[profile.dev.package.sha2]
opt-level = 3
```

(Write `[workspace.dependencies]` out in full. Drop a dev-dependency the suites turn out not to
use, and add one the compiler asks for.)

Run at the end: everything in Merge step 3.

---

## Merge

1. **Parallel branches** (controller, on refactor/core-addons, in this order: tui, api, vocdoni,
   web; web last because its diff is the largest):
   ```bash
   git merge --no-ff p4/tui -m "merge: P4.3 claudio-tui"
   cargo build --locked && cargo test --locked --workspace --lib --bins && python3 scripts/check_rules.py
   # then p4/api ("merge: P4.4 claudio-api"), p4/vocdoni (P4.6), p4/web (P4.5), with the same three checks after each
   ```
   Expected: no conflicts (D3). On a `Cargo.lock` conflict, take either side, run
   `cargo update --workspace --offline`, and check that `git diff Cargo.lock | grep -E
   '^[-+]version'` shows no external change. On any other conflict, stop: a task wrote outside
   what it owns. Then remove the worktrees (`git worktree remove ~/.cache/claudio/p4/<t>`) and
   delete the `p4/*` branches after the final merge.
2. **P4.7** on refactor/core-addons.
3. **Full suites** at P4.7's end, every one green:
   S-check; S-matrix (core only, api, web, vocdoni, all); S-unit; `cargo test --locked
   -p claudio-core -p claudio-tui` (AC2); `cargo build --locked --no-default-features` (AC2);
   S-manager-core (A4); S-manager; S-web; S-vocdoni; S-api; S-shots; S-remote (t1/t7/t8, and
   t2-t6/t9 with `CLAUDIO_SSH_TEST_HOST=z6` when reachable: the bootstrap still uploads the one
   binary); S-node and S-e2e (zero CSP violations); `make static` and
   `file dist/claudio` (one static binary, name unchanged); `make dev` / `make dev-undo` on this
   host only if the owner asks.
   Also paste:
   - the `rules:` totals line (equal to P3's end);
   - `cargo tree -p claudio -e normal -f '{p} {f}' | grep test-support` (empty);
   - `cargo tree -p claudio-core -e normal --depth 1 | grep -E 'ratatui|crossterm|axum|tower'`
     (empty);
   - `python3 scripts/check_rules.py` (A1 and A2 clean: AC3, AC4);
   - `git diff --find-renames=100% --name-status <P3 end>..HEAD -- web addons/web/web |
     grep -v '^R100'` (empty);
   - `grep '^version' Cargo.toml | head -1` (`version = "0.8.0"`);
   - `pgrep -af 'claudio --daemon'` (only the owner's).
   The gated suites the owner can run: `CLAUDIO_E2E=1` (integration, manager's real-claude test)
   and `CLAUDIO_PROXY_TEST=1` (vocdoni_manager's proxy test).
4. **Review.** A reviewer audits the branch against this plan, the spec's P4 row, AC1-AC4 and
   AC8, and ACCEPTANCE.md (A1-A4, A8, D1, D4, D6, D7, E2, E5, F1, F2), citing rules. Moves are
   moves (Review Focus); no `--write-baseline` outside P4.1/P4.2/P4.7; no test lost an assertion
   (compare test names before and after: `cargo test --locked --all-features -- --list
   2>/dev/null | grep ': test$' | sed 's/: test$//' | sort` at P3's end (single crate), and
   `cargo test --locked --workspace --all-features -- --list …` now; every old name is present.
   Module paths may gain a crate prefix, so compare the last path segment. Names renamed in
   P4.1 are matched through the commit messages that list them).
5. `git log --format=%b <P3 end>..HEAD | grep -i 'co-authored\|generated with'` is empty (F2).
   Then `git checkout main && git merge --no-ff refactor/core-addons` into local `main`, only
   after the review.
6. **Owner OK required before any push, tag or release.**

## Spec issues

1. **"Tests split by crate" with dev-dependencies cannot work as Cargo dev-dependencies.** The
   integration suites drive the `claudio` binary. Cargo sets `CARGO_BIN_EXE_claudio` only for
   the binary's own package (artifact dependencies are nightly-only), and a test in
   `crates/tui` linking claudio-tui could not start the binary. So the files live in each
   crate's `tests/` but are root `[[test]]` targets with `required-features` (D1). The harness
   is shared by `#[path]`, not by a dev-dependency on claudio-tui or claudio-web (D2), and none
   of the suites links a crate. E5's "`cargo test -p <crate>`" holds for unit tests; a crate's
   integration suite runs as `cargo test --test <name>` with the features it needs (A4:
   `--no-default-features --test manager`). Probed with cargo 1.93 before writing this plan.
2. **`term/` in the core contradicts D7.** `term/keys.rs` (crossterm) and `Screen::render`
   (ratatui) are UI-toolkit code. P4.1 moves them to the TUI (D7) so claudio-core takes no UI
   toolkit. The terminal model itself stays in the core, because the daemon and the API need it.
3. **axum in the core's tests.** Two core tests (`claude::update`, `upgrade`) fake a release server
   with axum, so axum is a dev-dependency of claudio-core. D7 is read as applying to what the
   core links; the owner may prefer a std `TcpListener` fake.
4. **Test counts.** The spec says 6 manager and 8 web proxy/plugin tests. At HEAD there are
   about 17 (manager) and about 18 (web), plus 2 manager tests that need the web addon (its `latex`/`diagram` rows, AC6).
   P4.1 splits by the addons each test needs and checks the result by running each subset build.
5. **Suites the spec does not place.** `tests/remote.rs` (t9 needs the Vocdoni diag word) and
   `tests/screenshots.rs` (the proxy stats panel) exercise the whole binary. They stay in the root
   `tests/`, with `required-features = ["diag", "vocdoni"]` and `["vocdoni"]`.
6. **Unit tests that used the real registry.** The TUI's and the web's unit tests built
   `crate::addons::all()` (the Vocdoni panel key, its legacy adoption). As crates they cannot see
   the addons (A2), and the TUI's may not name a team service (A1). P4.1 rewrites them onto
   `TestAddon`. These are test changes the spec did not list; the real addon's behaviour stays
   covered by its own tests.
7. **`web/` becomes `addons/web/web/`** ("page (web/)" under `addons/web`). The page's bytes and
   the CSP hash do not change, but DESIGN.md, docs/web.md, CLAUDE.md, the Makefile and the e2e
   fixture's repo path do.
8. **A build without `api` refuses print mode** (controller ruling R24, pre-flight F1).
   Unclaimed argv is passthrough, but claudio never invokes `claude -p` (CLAUDE.md). So P4.7
   adds a `#[cfg(not(feature = "api"))]` step to `src/main.rs::route`, after `addons.dispatch`
   and before the help check. If any argument before a bare `--` is `-p`, `--print` or `--api`,
   it prints `claudio: print mode and the API server need the api addon; this build has none` on
   stderr and returns `ExitCode::from(2)`. A root unit test compiled with
   `#[cfg(not(feature = "api"))]`, `a_build_without_the_api_addon_refuses_print_mode`, asserts
   the exit code for all three flags and that `claudio -- -p` (a prompt) is not refused.
   docs/print-and-api.md and README say so. The release binary keeps every addon.
9. **The Dockerfile build is already broken.** It copies only `src/`, without `build.rs` or
   `web/`. P4.7 fixes it by copying the workspace, which goes beyond "no behaviour change" but is
   needed for the image to build at all.
