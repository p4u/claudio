# Remote hosts without ~/.ssh/config, and sign-out closing open tabs: implementation plan

Spec: inline below (Bounded)
Class: bounded
Repos and branches: /home/p4u/claudio → fix/remote-hosts (base 5a3b4de, v0.9.1)

## Spec

**Problem.**
- Adding a remote host fails with `bootstrap: unknown remote OS from uname: ""` whenever ssh
  itself fails. The usual causes: an alias that only an ssh config could resolve, a machine
  without `~/.ssh/config`, or a host never connected to, whose key `BatchMode=yes` cannot
  accept. `remote::bootstrap::probe_and_platform` (crates/core/src/remote/bootstrap.rs:160-196)
  ignores ssh's exit status and stderr, so the real reason is lost.
- The web's Alt+n wizard (addons/web/web/app/ui/wizard.js `HostField`) lists only connected
  hosts and the `hosts.candidates` (MRU + `~/.ssh/config`), so a new host cannot be typed
  there. The TUI wizard can already take one (`connect_raw`).
- Sign out (v0.9.1) ends the session, but another tab already open with it keeps its socket
  (gate audit F2, ruling R23).

**Owner decisions (2026-11-10).**
- A first connection to an unknown host accepts its key silently
  (`StrictHostKeyChecking=accept-new`). A changed key is still refused.
- Hosts not in `~/.ssh/config` must be addable from Alt+n.

**Goals.**
1. Every ssh failure in the probe reports ssh's own words: the exit status and the last lines
   of its stderr, through `text::sanitize`, at most 1 KiB. Never the uname message.
2. `remote::ssh_cmd` adds `-o StrictHostKeyChecking=accept-new`. It is used by bootstrap and by
   the bridge, so the TUI, the web and `--slave` connections all take a new host's key on
   first contact.
3. The web wizard's host select gets an "Another host…" entry: a text field (alias or
   `user@host`) and a Connect button that send `hosts.connect`. On success the wizard goes on
   with that host; on failure the server's error (now ssh's) is shown in `wizard-host-error`.
4. When a session is removed (sign-out, a login that replaces it, eviction past 64, idle
   expiry), every open tab opened with it is closed by the server. A page whose socket closes
   asks `GET /api/me` before reconnecting, and reloads (showing the login) on a 401. That also
   ends the "retries forever" state of a tab whose session expired.

**Non-goals.**
- Password logins (BatchMode stays).
- A fingerprint confirmation UI.
- Any change to the web protocol's messages, DTOs or close codes.
- TUI changes beyond what goals 1–2 give it for free.

**Acceptance criteria.**
- AC1: unit tests of a pure parse function for the probe's output.
  - Exit 255 with stderr `ssh: Could not resolve hostname x: …` gives an error containing that
    line, not "uname".
  - A missing `---UNAME---` marker with exit 0 gives an error naming the missing marker.
  - Exit 0 with `Linux x86_64` gives linux/x86_64.
  - Stderr over 1 KiB is cut.
  - Control characters are removed.
- AC2: a unit test asserts `ssh_cmd`'s args contain `StrictHostKeyChecking=accept-new` before
  `--`.
- AC3: a Playwright spec. In the wizard, "Another host…" with `nonexistent.invalid` shows an
  error containing `Could not resolve hostname` (or, where DNS differs, ssh's message) and
  never `uname`. The host field stays usable. Zero CSP violations.
- AC4: a Rust test (`addons/web/tests/web.rs`). Two WebSockets opened with one cookie; a
  `POST /api/logout` with that cookie closes both sockets within 2 s. A socket of another
  session stays open.
- AC5: a node test of `proto.js`: on close, with an injected `me()` answering 401, the client
  calls its injected `reload()` and does not reconnect. With `me()` answering 204 it
  reconnects as before.
- AC6: docs updated (docs/web.md: adding a host, keys accepted on first contact, sign-out
  closes the open tabs; CLAUDE.md: one line in "Remote"); `make check`, `make matrix`, the
  web suites and `make test-web` pass.

## Global Constraints
- Commit convention: conventional commits `type(scope): lowercase summary`; no AI attribution,
  no Co-Authored-By or similar trailers.
- Never push. Commit with explicit paths only (`git add <new files>` first; `git commit -m … --
  <paths>`); never `git add -A`.
- Build output under the repo's target/ (or ~/.cache/claudio/), never /tmp.
- Subagents never dispatch subagents.
- ACCEPTANCE.md throughout:
  - A2/A3: the core never names an addon.
  - A8: `ws.rs::tab` is at its size baseline and must not grow; `hub.rs` is near its baseline.
    Put new code in `upgrade`'s closure and in `sessions.rs`.
  - C1: no secret in logs.
  - C2: strings from ssh are sanitised before any front-end sees them.
  - E2: no test weakened.
  - B2: no web-protocol change in this work. If one becomes necessary, stop and ask the
    controller.
- Tests are hermetic: claudio runs only in the suites' isolated env. The owner runs their own
  `claudio --daemon` and `claudio web --remote`: never touch, signal or connect to them.
- Pure browser modules import nothing through bare specifiers. New page code follows DESIGN.md
  and tokens.css (no hard-coded colours); new `data-testid`s are `wizard-host-other`,
  `wizard-host-input`, `wizard-host-connect`.
- `make check` passes at every commit.

## Review Focus
- `bootstrap.rs` probe parse: ssh's stderr reaches the UI only sanitised and capped. No other
  bootstrap path changes behaviour.
- `ssh_cmd`: the option goes before `--`, so the host stays the only operand. `validate_host`
  still refuses a leading `-`.
- `sessions.rs`: the removal notification can neither leak (unbounded) nor miss a removal
  between a tab's subscribe and its first wait. The closure in `ws::upgrade` drops the tab
  future, which closes the socket and releases its `TabSlot`.
- `proto.js`: the 401 → reload path cannot loop (a reload is a page load) and cannot trigger on
  a transient network error (only an explicit 401 reloads).

## Task order and parallelism
| Task | Depends on | Repo/files | May run in parallel with |
|---|---|---|---|
| 1 Probe errors + accept-new | none | crates/core/src/remote/{bootstrap.rs,mod.rs} | 2 |
| 2 Sign-out closes open tabs; page reloads on 401 | none | addons/web/src/{sessions.rs,ws.rs}, addons/web/web/app/proto.js, addons/web/web/tests/, addons/web/tests/web.rs | 1 |
| 3 Wizard "Another host…" + e2e | 1, 2 | addons/web/web/app/ui/wizard.js, styles if needed, addons/web/web/e2e/tests/hosts-typed.spec.mjs | none |
| 4 Docs | 1-3 | docs/web.md, CLAUDE.md | none |

## Tasks

### Task 1: The probe reports ssh's own error; new hosts' keys are accepted
Model: sonnet   Files: `crates/core/src/remote/bootstrap.rs`, `crates/core/src/remote/mod.rs`
Goal: Goals 1-2 (AC1, AC2).
- Extract from `probe_and_platform` a pure
  `fn parse_probe(status: Option<i32>, stdout: &str, stderr: &str) -> Result<(Option<Probe>, String, String), String>`.
  - When stdout has no `---UNAME---` marker, or the status is not 0 and the uname part is empty,
    return `Err` with `ssh failed (exit N): <stderr tail>`. The tail is the last lines of
    stderr, `text::sanitize`d, trimmed, at most 1024 bytes cut at a char boundary.
  - With an empty stderr, say `no output from <what>`.
  - Otherwise keep today's uname mapping and its errors.
- `probe_and_platform` calls it with `output.status.code()`.
- Add `-o StrictHostKeyChecking=accept-new` to `ssh_cmd`, before `--`. Update its doc comment.
- Tests first (unit, in the files' test modules): the AC1 cases and AC2.
- `make check`. Commit: `fix(remote): the probe reports ssh's error, and new hosts' keys are accepted`.

### Task 2: A sign-out closes the tabs opened with that session; the page reloads to the login
Model: opus   Files: `addons/web/src/sessions.rs`, `addons/web/src/ws.rs`,
`addons/web/web/app/proto.js` (+ its caller that constructs it, for the injected `me`/`reload`),
`addons/web/web/tests/proto.test.mjs` (or the existing proto test file),
`addons/web/tests/web.rs`
Goal: Goal 4 (AC4, AC5).
- Server: `Sessions` notifies the removal of a session's hash. Use a
  `tokio::sync::watch` of a removal generation plus a check of the store, or a broadcast with
  a bounded capacity whose lag is treated as "re-check". Never unbounded.
  - It covers `sign_out`, login replacement, cap eviction, and idle expiry found during `tidy`.
  - In `ws::upgrade`'s `on_upgrade` closure, select between `tab(state, socket, slot)` and
    "this tab's session hash is gone". On the latter, drop the tab future: the socket and its
    `TabSlot` go with it.
  - A tab opened without a session (no cookie hash) is unaffected.
  - Subscribe before checking, so a removal between the two is not missed.
- Page: `proto.js`'s client gets two injected functions, `me()` (resolves to the HTTP status of
  `GET /api/me`) and `reload()`. Defaults: `fetch('/api/me', {credentials:'same-origin'})` and
  `location.reload()`, wired by the caller, not imported, so the module stays pure.
  - On a close, it calls `me()` before scheduling the reconnect. A 401 calls `reload()` and
    stops. Any other status, or a failure, reconnects as today.
- Tests first: AC5 (node, injected fakes) and AC4 (Rust integration, two sockets plus one of
  another session; use the existing web harness's login and ws helpers).
- `make check`, the web suites, `node --test addons/web/web/tests/*.test.mjs`, `make test-web`.
  Commit: `fix(web): a sign-out closes the tabs opened with it, and the page shows the login`.

### Task 3: The wizard's "Another host…"
Model: sonnet   Files: `addons/web/web/app/ui/wizard.js` (styles only if needed, through
tokens.css), `addons/web/web/e2e/tests/hosts-typed.spec.mjs` (new)
Goal: Goal 3 (AC3).
- In `HostField`'s select, add a last option "Another host…" (`data-testid="wizard-host-other"`,
  value `other:`). Picking it shows a text input (`wizard-host-input`, placeholder `alias or
  user@host`, no autocomplete or spellcheck) and a Connect button (`wizard-host-connect`,
  disabled while empty or busy).
- Enter or Connect calls the same path as an `ssh:` candidate: `hosts.connect` with the trimmed
  text, then `set({host, hosts: […, host], resume: null})`. An error shows in
  `wizard-host-error`.
- Reuse the busy note ("Connecting to …").
- e2e first (fails before): open the wizard (Alt+n), pick "Another host…", type
  `nonexistent.invalid`, Connect. Expect `wizard-host-error` to contain `resolve` (case
  insensitive) or `ssh failed`, never `uname`. Expect no CSP violation.
- `make test-web`. Commit: `feat(web): the wizard connects a host typed by hand`.

### Task 4: Docs
Model: sonnet   Files: `docs/web.md`, `CLAUDE.md`
- docs/web.md:
  - Hosts: a host is added by name or `user@host` from the hosts panel or Alt+n's "Another
    host…". A first connection records the host's key (`accept-new`), and a changed key is
    refused with ssh's message.
  - ssh's own error is shown.
  - Sign out closes the tabs opened with that sign-in. Remove the v0.9.1 caveat that another
    tab keeps its connection.
- CLAUDE.md "Remote" bullet: one clause, `ssh_cmd` uses `StrictHostKeyChecking=accept-new`.
- Commit: `docs(web): typed hosts, accepted host keys, sign-out closes open tabs`.

## Merge and release
1. Controller runs: `make check`, `make matrix`, `cargo test --locked --test manager --test
   vocdoni_manager --test web --test web_manager --test vocdoni_web -- --test-threads=1`,
   `make test-web`, `cargo test --features diag --test remote -- --test-threads=1`.
2. Fable gate audit of `main...fix/remote-hosts`; fixes re-reviewed.
3. `git switch main && git merge --no-ff fix/remote-hosts`, version 0.9.2 (Cargo.toml, `cargo
   update -w`), `chore(release): v0.9.2`, annotated tag `v0.9.2`, push main and the tag; set the
   release notes after the workflow publishes.
4. Owner OK: the owner said "don't ask me, just merge and publish" for this release cycle
   (2026-11-10); this release fixes the bug they reported.
