# Code acceptance rules

Every change to claudio, written by a person or an agent, meets these rules before it merges.
`make check` runs the mechanical ones (marked **[checked]**); the reviewer checks the rest
against the diff. A rule changes only through a commit to this file that says why.

claudio is a generic product: a small **core** that wraps the `claude` CLI, and **addons** that
add everything else. The rules protect that shape first.

## A. Architecture

**A1. The core is generic.** The core crates (`claudio-core`, `claudio-tui`) name no
organisation, company service or team product other than Claude Code itself, git and ssh: no
`vocdoni`, no proxy hosts, no team plugin names, not even in tests. Those live in an addon.
**[checked]**

**A2. Dependencies point one way.** `claudio` (the binary) → addons → `claudio-tui` →
`claudio-core`. The core never depends on an addon or on the TUI; the TUI never depends on an
addon; an addon never depends on another addon. **[checked]**

**A3. Addons use the addon API, never core internals.** An addon extends claudio only through
`claudio_core::addon`, the daemon protocol and its own config section. When an addon needs
something new, the hook goes into the addon API in its own commit: generic, documented and
tested without that addon. Core code never branches on an addon's name.

**A4. The core works alone.** `cargo build --no-default-features` builds a claudio that runs the
session manager, the passthrough, the daemon, SSH hosts and the claude installer on a host with
a normal `claude` login: no config file, no addon, no network beyond claude's own. The manager
integration tests pass in that build. **[checked]**

**A5. One implementation per behaviour.** Logic that two front-ends (TUI, web) or two addons need
lives once, in the lowest crate both depend on. Front-ends render and route input; they do not
re-implement state machines. A copy "re-expressed" for another front-end is a defect.

**A6. Pure logic, I/O at the edges.** State machines (the TUI `App`, the wizard, views, addon
state) take events and return effects. Files, network, processes and sockets are touched only in
the runners (event loops, daemon actors, server handlers). A pure module has no `std::fs`,
`tokio::fs`, `reqwest` or `Command`.

**A7. The console TUI stays plain.** It shows claude's screen as claude drew it. Anything that
rewrites or overlays claude's output (formulas, diagrams, tag lenses) belongs to the web UI only.
The TUI may add chrome around the pane (tabs, status bar, popups), never inside it.

**A8. Small units.** A source file stays under 800 lines without its tests, a function under 80
lines. A file or function already over budget may not grow; the baseline in
`scripts/size-baseline.txt` only shrinks. **[checked]**

**A9. Open sets are registries, closed sets are enums.** What addons extend (commands, launch
contributions, status segments, panels, daemon ops) is a registry an addon adds to, never a core
enum or `match` that each addon edits.

## B. Compatibility

**B1. Wire protocols change additively.** New daemon ops and events are ignored or refused safely
by an older peer (`Unknown` variants; `unsupported op` → "daemon too old"). Never add a field to
an existing op that an old daemon would silently ignore into wrong behaviour.

**B2. Protocol docs first.** A change to the daemon protocol or the web protocol
(`docs/web-protocol.md`) lands its documentation in an earlier commit than its code.

**B3. Persisted files stay readable.** `config.toml`, `state.json`, the journal,
`web-state.json` and `plugins.json` from the last release load in the new one, and a test proves
it. `config.toml` belongs to the user: claudio edits single lines and never rewrites the file.

**B4. A daemon is replaced only by a newer build** (version, then binary mtime).

## C. Security

**C1. Secrets travel one way.** Tokens and plugin env reach claude only through `Spawn.env` over
the daemon socket: never in argv, logs, notices, state files, DTOs, `Debug` output or test
snapshots. Messages name keys, never values.

**C2. Outside input is bounded and never panics.** Data from files, sockets, ssh, transcripts,
extension programs, the network or a browser has a size cap and is parsed without `unwrap`,
`expect`, unchecked indexing or unbounded recursion.

**C3. Child processes are contained.** Own process group, a deadline, a scrubbed env, capped
output, stdin `/dev/null` unless interactive, killed as a group on timeout.

**C4. Files claudio writes** are 0600 in 0700 directories, written atomically
(`paths::write_atomic`).

**C5. The web UI keeps its perimeter.** Loopback only, guard order Host → Origin → cookie, the
import map is the only inline script, no eval or CDN, vendored files byte-identical.

## D. Code quality

**D1. Formatted and lint-clean.** `cargo fmt --check` and `cargo clippy --workspace --all-targets
-- -D warnings` pass. **[checked]**

**D2. No panics on purpose.** No `unwrap()` outside tests; `expect("…")` only on an invariant the
message explains. No `todo!`, `unimplemented!` or `dbg!`. **[checked]**

**D3. Async code never blocks.** Blocking I/O and CPU-heavy work in async code go through
`spawn_blocking` or a dedicated thread.

**D4. No dead code.** No `#[allow(dead_code)]` or `#[allow(unused…)]` without a comment saying why
the item must stay; unused code is deleted, not parked. **[checked]**

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

**E2. Tests are never weakened** to get to green: no deleted assertions, skips or retries. A
flaky test gets fixed.

**E3. Tests are hermetic.** No network, real `claude`, real `HOME` or real daemon: temp dirs,
fakes and an isolated env (`XDG_*`, `HOME`, `CLAUDIO_CLAUDE_PATH`).

**E4. TUI tests assert on the rendered screen model**, never on raw terminal bytes.

**E5. Each crate's tests run on their own** (`cargo test -p <crate>`); an addon's tests never need
another addon.

## F. Docs and process

**F1. Docs move with the code.** `CLAUDE.md`'s architecture map, `README.md` and the user docs
change in the same branch as the behaviour they describe.

**F2. Commits** are conventional (`type(scope): summary`), one logical change each, with no AI
attribution or co-author trailers.

**F3. The web UI follows `DESIGN.md`.**

## Checking

```bash
make check     # fmt, clippy -D warnings, the rules script (A1 A2 A8 D2 D4), core-only build, unit tests
```

`scripts/check-rules.sh` holds the mechanical checks. A reviewer cites the rule for every
finding ("A5: the web copy of the plugin state machine").
