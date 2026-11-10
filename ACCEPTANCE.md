# Code acceptance rules

Every change to claudio, written by a person or an agent, meets these rules before it merges.
`make check` runs the mechanical ones (marked **[checked]**); the reviewer checks the rest
against the diff and cites the rule for every finding ("A5: the web copy of the plugin state
machine"). A rule changes only through a commit to this file that says why.

claudio is a generic product: a small **core** that wraps the `claude` CLI (`claudio-core`, and
the console TUI `claudio-tui`), and **addons** that add everything else (`claudio-api`,
`claudio-web`, `claudio-vocdoni`). The rules protect that shape first. Mechanical checks catch
the obvious; they do not prove panic freedom or architectural independence, which need review
and tests.

## A. Architecture

**A1. The core is generic.** The core contains no organisation-specific endpoints, credentials,
plugin identities or business policy, in code or tests. Claude Code's and Anthropic's own names
(`claude`, `ANTHROPIC_*`, `CLAUDE_CODE_*`), git, ssh and claudio's release repository are fine.
The check rejects known team identifiers (`vocdoni`, `claude-proxy`, proxy hosts); the reviewer
checks the rest. **[checked]**

**A2. Dependencies point one way.** The binary `claudio` depends on the TUI and the addons; the
TUI depends on the core; every addon depends on the core only. The core depends on nothing of
ours. An addon may take the TUI or another addon as a dev-dependency for integration tests,
never as a dependency. **[checked]**

**A3. Addons use the public API.** An addon extends claudio through `claudio_core::addon`, the
documented public services of `claudio-core` and the daemon protocol, never through private
modules. It reads and writes only the `config.toml` tables it declares. When an addon needs a new
hook, the hook lands in the API in its own commit: generic, documented and tested without that
addon. Core code never branches on an addon's name.

**A4. The core works alone.** `cargo build --no-default-features` builds a claudio that runs the
session manager, the passthrough, the daemon, SSH hosts and the claude installer on a host with
a normal `claude` login, with no addon configuration or credentials. It never contacts an addon's
service; SSH, install, update and bootstrap keep their documented network needs. The TUI crate's
manager tests pass in that build. **[checked]**

**A5. One implementation per domain behaviour.** Domain policy and state transitions that two
front-ends (TUI, web) or two addons need live once, in the lowest crate both use. Front-ends
keep their own navigation and interaction state, and render and route input; they never
re-implement a domain state machine.

**A6. Pure logic, I/O at the edges.** Domain state machines (the TUI `App`, the wizard, addon
state) take events and return effects; they touch no files, network, processes or sockets.
Effects run in explicit async runners, never on a rendering path. Blocking work is offloaded;
locks are not held across an await or a callback into a host.

**A7. The console TUI stays plain.** It shows claude's screen as claude drew it. Anything that
rewrites or overlays claude's output (formulas, diagrams, tag lenses) belongs to the web UI. The
TUI adds chrome around the pane (tabs, status bar, popups), never inside it.

**A8. Small units.** In authored code (vendored and generated files excluded), a source file
stays under 800 lines without its tests and a function under 80 lines. What is over budget today
is listed in `scripts/size-baseline.txt`; a listed item may shrink but not grow, and anything
unlisted meets the budget. The baseline is keyed by path: a commit that only moves files may
rename entries (same counts) and nothing else. **[checked]**

**A9. Open sets are registries, closed sets are enums.** What addons extend (commands, launch
contributions, segments, panels, checklists, daemon ops) is a registry an addon adds to, never a
core enum or `match` that each addon edits.

**A10. Async services are bounded.** Every background task or queue has a size bound, a
timeout, cancellation, an end at shutdown and a guard against stale results (a request
generation). A composed message is checked against its frame cap before it is sent.

## B. Compatibility

**B1. Mixed versions work for one release.** A client and a daemon one release apart work
together in both directions; freshness is not a compatibility guarantee (it leaves a busy daemon
running). New daemon ops and events are ignored or refused safely by an older peer (`Unknown`
variants; `unsupported op` → "daemon too old"). A change whose correctness depends on the peer
uses a new op or a capability in `Welcome.features`, never a field an old peer silently drops.
Tests cover both directions.

**B2. Protocol docs first.** A change to the daemon protocol or the web protocol
(`docs/web-protocol.md`) lands its documentation in an earlier commit than its code.

**B3. Persisted files stay readable, and a rollback keeps working.** `config.toml`,
`state.json`, the journal, `web-state.json` and `plugins.json` from the last release load in the
new one without losing routing or settings (fixture tests). Config edits change the smallest set
of keys atomically, keep every other line and comment, and are serialised. A format change keeps
writing what the previous release reads for one release, so reverting the merge loses nothing.

**B4. A daemon is replaced only by a newer build** (version, then binary mtime).

## C. Security

**C1. Secrets stay where they belong.** Secrets live in their private files (0600) and in
redacted in-memory launch data, and reach a managed session's claude only through
`Spawn`/`Respawn` env over the daemon socket (a print/API backend's child env directly). Never in
argv, logs, notices, public DTOs, ordinary session state, `Debug` output or test snapshots;
messages name keys, never values.

**C2. Outside input is bounded and never panics.** Data from files, sockets, ssh, transcripts,
extension programs, the network or a browser has a size cap and is parsed without `unwrap`,
`expect`, unchecked indexing or unbounded recursion. Strings an addon hands a front-end are
sanitised and capped by the core.

**C3. Child processes are owned.** A finite helper command has a deadline and capped output; a
long-lived child has an owner that cancels it and kills its process group. Both run with a
scrubbed env and stdin `/dev/null` unless interactive; a supervised service's stdin is a pipe its
owner holds and never writes, so the child sees its owner's death.

**C4. Files claudio writes** are private data at 0600 in 0700 directories, replaced atomically
(`paths::write_atomic`); executables keep their executable mode; claudio never chmods a
directory it does not own.

**C5. The web UI keeps its perimeter.** Loopback only, or with `--remote` one TLS listener where
every non-local request needs the password (docs/web.md); guard order Host → Origin → cookie,
the import map is the only inline script, no eval or CDN, vendored files byte-identical.

## D. Code quality

**D1. Formatted and lint-clean.** `cargo fmt --check` and `cargo clippy --workspace --all-targets
-- -D warnings` pass. **[checked]**

**D2. No panics on purpose.** No `.unwrap()` outside tests (existing ones are in the baseline,
which only shrinks); `expect("…")` only on an invariant its message explains. No `todo!`,
`unimplemented!` or `dbg!`. **[checked]**

**D3. Async code never blocks.** Blocking I/O and heavy CPU work in async code go through
`spawn_blocking` or a dedicated thread.

**D4. No dead code.** No `#[allow(dead_code)]` or `#[allow(unused…)]` without a comment saying
why the item must stay; unused code is deleted, not parked. **[checked]**

**D5. Errors are handled where they mean something.** They carry context; a user-facing message
says what to do next; nothing is swallowed silently.

**D6. Comments say why.** Every `pub` item of `claudio-core` and of the addon API has a doc
comment. No commented-out code.

**D7. Dependencies are earned.** A new crate is justified in its commit message, pure Rust where
possible, MIT/Apache/BSD-licensed, and pinned exactly when it parses hostile input. The core
takes no HTTP-server, browser or UI-toolkit dependency.

## E. Tests

**E1. Behaviour changes come with a test** that fails without the change; a bug fix comes with a
regression test.

**E2. Tests are not weakened** to hide a defect: no deleted assertions, skips or retries to get
to green. A deliberately changed contract replaces its obsolete assertions with reviewed
coverage of the new one. A flaky test gets fixed.

**E3. Default suites are hermetic.** No network, real `claude`, real `HOME` or real daemon: temp
dirs, fakes and an isolated env (`XDG_*`, `HOME`, `CLAUDIO_CLAUDE_PATH`). Suites against a real
claude or SSH host are opt-in and report when they are skipped.

**E4. TUI tests assert on the rendered screen model**, never on raw terminal bytes.

**E5. Every crate's tests run on their own** (`cargo test -p <crate>`), and CI builds and tests
the feature matrix: core only, each addon alone, and all of them.

## F. Docs and process

**F1. Docs move with the code.** `CLAUDE.md`'s architecture map, `README.md` and the user docs
change in the same branch as the behaviour they describe.

**F2. Commits** are conventional (`type(scope): summary`), one logical change each, with no AI
attribution or co-author trailers.

**F3. The web UI follows `DESIGN.md`.**

## Checking

```bash
make check     # fmt, clippy -D warnings, scripts/check_rules.py, core-only build, library tests
```

`scripts/check_rules.py` holds the mechanical checks (A1, A2, A8, D2, D4). A2 checks dependency
kinds (`cargo metadata`); `make matrix` runs the feature matrix (E5).
