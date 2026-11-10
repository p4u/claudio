# `claudio web` as a daemon service: implementation plan

Spec: `docs/specs/2026-11-09-web-service-design.md` (approved 2026-11-09; audits
`.delivery/2026-11-09-web-service/audit-spec.md` and `audit-spec-2.md` folded in)
Class: architectural
Repos and branches: /home/p4u/claudio → feat/web-service (base d7755ec)

Every patch in this plan was generated from a scratch worktree of d7755ec and checked there:
each task's two patches apply with `git apply` on top of the previous task's, step 1 fails as
written under "Expected", step 2 passes, and after every code task the `make check` sequence
(below) passed, with `make matrix`, the `web`, `web_manager`, `vocdoni_web`, `manager` and
`vocdoni_manager` suites passing on the final tree. Not run there: the Playwright e2e (no
Chromium or `node_modules` on the machine; Task 7 says what to run) and `make static`.

## Spec issues found

1. **`docs/web-protocol.md:23` states the password range too.** The login row reads
   `{"password":"<12–1024 bytes>"}`; the spec allows that document to change only in its Cookie
   bullet (lines 53-55). Resolution (Task 8): that cell becomes `<8–1024 bytes>` in its own docs
   commit before the code (B2). The server's check does not change (`Stored::verify` checks the
   maximum only).
2. **AC3's "service child ignores SIGTERM" needs a test hook.** The real `claudio web --service`
   stops gracefully on SIGTERM and on stdin's EOF, so no test can make it ignore its stop
   without one. Resolution (Task 11): `CLAUDIO_TEST_SERVICE_HANG=1`, debug builds only, as
   `CLAUDIO_TEST_REMOTE_PEER` is (`addons/web/src/auth.rs` `test_remote_peer`): a `--service`
   that ignores stdin's EOF and catches SIGTERM without acting. A release build compiles it to
   `false`, and `make static`'s final grep (`Makefile:55-57`) checks for its name too.
3. **"The tab task marks that session used once an hour" (spec §3, "Use").** `ws.rs::tab` is at
   its A8 baseline (`fn addons/web/src/ws.rs::tab 103` in `scripts/size-baseline.txt`) and may
   not grow. Resolution (Task 6): `ws::upgrade` starts a `sessions::OpenTab` inside its
   `on_upgrade` closure, around `tab(...)`: a task that lives exactly as long as the tab
   (aborted when dropped), with the same behaviour.
4. **The hub cannot take a "connect `local` plainly" field.** `addons/web/src/hub.rs` is at
   2061 of its 2065-line baseline; a `HubConfig` field and its plumbing need about 5. Resolution
   (Task 11): a process-wide `AtomicBool` in `hub_io.rs`, set once by `--service` before the
   runtime starts (`hub_io::connect_local_plainly`); `connect_once` then skips
   `ensure_local_daemon`. The hub's existing reconnect loop is the spec's "plus reconnect".
5. **`--rotate-cert` without `--remote` is refused at parse time today** (`addons/web/src/lib.rs`
   `combination_ok`, lines 166-168 at base), while the spec's merge lets
   `claudio web --rotate-cert` act on a stored `remote` unit. Resolution (Task 13): the
   parse-time rule applies with `--foreground` only; the hand-off checks the merged unit
   (`service::run_args`: "--rotate-cert needs --remote (here, or stored in [web])"). The unit
   test's `["--rotate-cert"]` case becomes `["--rotate-cert", "--foreground"]`, plus an
   assertion that a bare `--rotate-cert` parses (E2: the old assertion is replaced by the new
   rule's). `web_remote_arguments_are_checked` (`addons/web/tests/web.rs:348`) passes unchanged:
   with nothing stored the merged unit is loopback and is refused with exit 2.
6. **`service()` and a disabled but invalid `[web]`.** Spec §2 gives "Err naming the key, or
   argv when `enabled = true`, else `Ok(None)`" without an order. Resolution (Tasks 12-13):
   `enabled` is read first and a disabled section is `Ok(None)` unchecked, so
   `claudio web stop` always reaches `Off`, even after a bad hand edit. The stored unit stays
   after `stop`, and a later bare `claudio web` reuses it.
7. **AC2 "the child exits when the daemon is SIGKILLed".** A daemon unit test cannot SIGKILL its
   own process. The core test drops every stdin without a signal (`Services::abandon`,
   `#[cfg(test)]`), which is what the kernel does when a daemon dies. The real path is in
   every service integration test's teardown (`ServiceWeb::finish`): the daemon is SIGTERMed
   (it has no handler, so it dies like a SIGKILL and stops nothing), then the service pid must
   be gone within 5 s (AC11).
8. **The 10 s lock wait (spec §1, "Stop") has no deterministic failing test**: it is a margin
   (3 s + 1 s against the old 5 s). It is folded into the supervisor commit (Task 9) and is
   exercised by `web_daemon_restart_kills_a_service_that_ignores_its_stop` (Task 13).
9. **Environment, not spec:** this machine's rustup stable is 1.95 (clippy 0.1.95), which
   flags `crates/core/src/term/probe.rs:98` and `:109` (`clippy::collapsible_match`) already at
   d7755ec, so `make check` fails here before any change. CI uses the latest stable
   (`.github/workflows/ci.yml`, `dtolnay/rust-toolchain@stable`). The Global Constraints give
   the exact fallback; the controller should tell the owner.
10. **Process, not spec:** the working tree holds the owner's uncommitted rewrite of
    `CLAUDE.md` (132 lines added, 425 removed) over the very sections the spec asks to update.
    Task 16 (CLAUDE.md) is gated on the owner.

## Global Constraints

- Commit convention: conventional commits `type(scope): lowercase summary`; no AI attribution, no Co-Authored-By or similar trailers.
- Never push. Commit with explicit paths only (`git commit -m … -- <paths>`, `git add <new files>` first); never `git add -A`. Never stage CLAUDE.md unless the task says so.
- Build output under the repo's target/ (or ~/.cache/claudio/), never /tmp.
- Subagents never dispatch subagents.
- Work in `/home/p4u/claudio` on `feat/web-service`. The working tree has the owner's
  unrelated, uncommitted `CLAUDE.md` edit: never touch, stage, stash or revert it.
- **Applying a patch.** Every code block marked `diff` is exact. Save it verbatim (without the
  fence lines) to `~/.cache/claudio/plan-patches/<task>-<step>.diff`, then from the repo root
  run `git apply --check <file>` and `git apply <file>`. Never hand-copy a patch line by line;
  if `git apply --check` fails, stop and report (the tree is not what the plan expects). Then
  `git diff --stat` must list exactly the files the step names.
- **`make check` passes at every commit.** On this machine it stops at clippy with two errors
  that are already at base d7755ec: `clippy::collapsible_match` at
  `crates/core/src/term/probe.rs:98` and `:109` (clippy 0.1.95; CI's newer stable accepts the
  code). Do not fix them (out of scope). When those two are clippy's only errors, run the rest
  of `make check` exactly as below; every command must succeed:
  ```bash
  export CARGO_TARGET_DIR=target/check
  cargo fmt --all --check
  cargo clippy --locked --workspace --all-targets --all-features -- -D warnings -A clippy::collapsible_match
  python3 scripts/check_rules.py
  cargo build --locked --no-default-features
  cargo test --locked --workspace --lib --bins
  for p in claudio-core claudio-tui claudio-api claudio-web claudio-vocdoni; do cargo test --locked -p $p --lib || exit 1; done
  ```
  `scripts/check_rules.py` must print `rules: 0 problem(s)`. `make matrix` has the same
  fallback, every command succeeding:
  ```bash
  export CARGO_TARGET_DIR=target/check
  for f in "" api web vocdoni api,web,vocdoni; do
    cargo clippy --locked -p claudio --no-default-features --features "$f" --all-targets -- -D warnings -A clippy::collapsible_match || exit 1
    cargo build --locked -p claudio --no-default-features --features "$f" || exit 1
    cargo test --locked -p claudio --no-default-features --features "$f" --bins || exit 1
  done
  cargo test --locked -p claudio --no-default-features --test manager -- --test-threads=1
  ```
- The shell's `rtk` hook may summarise `cargo`, `git` and `diff` output. Prefix a command with
  `rtk proxy` when a step needs its exact lines.
- Integration suites run single-threaded: `cargo test --locked --test <suite> -- --test-threads=1`
  (`manager`, `web`, `web_manager`, `vocdoni_web`, `vocdoni_manager`).
- Hermetic tests (E3): run claudio only through the suites' isolated env (`XDG_*`, `HOME`,
  `CLAUDIO_CLAUDE_PATH`). Never run `claudio web`, `claudio daemon stop|restart` or a bare
  `claudio` against the owner's real config or daemon. Unit tests keep the repo's scratch
  convention (`claudio_core::paths::socket_test_dir`, under `/tmp`); that is test data, not
  build output.
- ACCEPTANCE.md throughout. **A2:** claudio-core depends on nothing of ours; claudio-web on
  claudio-core only (its dev-dependencies may add more). **A3:** the supervisor is generic: no
  code under `crates/core/src/daemon` or `crates/core/src/addon` names the web addon
  (`grep -rn '"web"' crates/core/src/daemon crates/core/src/addon` matches only tests and doc
  examples; today: `addon/mod.rs:205`, a test). The web addon reads and writes only `[web]`.
  **A4:** `cargo build --no-default-features` builds, and the core-only claudio runs no service.
  **A8:** no file over 800 production lines, no function over 80, and baseline items never
  grow (`daemon/server.rs` is at 900, `web/src/hub.rs` at 2065, `ws.rs::tab` at 103).
- **C3, as amended in Task 2:** a long-lived child has an owner that cancels it and kills its
  process group; a supervised service's stdin is a pipe its owner holds and never writes, so the
  child sees its owner's death.
- **Secrets (C1):** never in argv, logs, notices, `Debug` output or `AddonOp` bodies. The web
  token is printed only by the `claudio web` CLI (from the 0600 token file) and by
  `--print-url`; `claudio web --service` never prints or logs it. `web-sessions.json` holds
  SHA-256 hashes of cookie values, never a value. The password, the certificate's key and the
  token stay in their 0600 files.
- The spec's Constraints: `app.rs` stays pure; no TUI change. `docs/web-protocol.md` changes
  only in its Cookie bullet (and line 23, Spec issue 1). Docs come before code (B2): Tasks 1-3
  and 8's first commit. The C3 amendment is its own commit, before the supervisor.
- **E2:** existing tests are never weakened or deleted. A replaced test tests the new rule
  (Task 4 replaces `auth.rs`'s `sessions_expire_when_idle_and_are_capped` with store tests of
  the 30-day rule; Task 8 replaces `password_length_is_12_to_1024_bytes`; Task 13 changes one
  case of `args_parse_flags_and_defaults`). Existing web tests change only by
  `--foreground` (Task 10, AC6); the shared harness only gains helpers (Tasks 8 and 13).
- Never bump the version, tag or release; the controller handles merge and release.

## Review Focus

Paths and lines are those of the final tree (after Task 14).

- **The supervisor** (`crates/core/src/daemon/service.rs`): `start` (`:352`: spawn, register
  the child before waiting, the 20 s ready read, the failed-start tail after the copier's EOF),
  `stop` (`:558`: stdin dropped, SIGTERM to the group only while the leader lives, SIGKILL after
  the grace, reap, SIGKILL for leftovers), `monitor` (`:417`: an exit is unasked only while the
  epoch is unchanged and the slot open; the backoff reset after 5 min), `fail` (`:448`: the
  doubling backoff and the epoch-guarded restart task under the slot's turn), `converge`
  (`:262`: the no-op rule, a `Failed` service started at once, `Err` as `Failed` without
  retry), `Services::reload` and the tickets (`:170`), `Services::stop_all` (`:189`), `spawn`
  (`:492`: process group, env without `SESSION_MARKERS`, `PATH`, the three pipes), `wait_ready`
  (`:540`: ≤ 4 KiB, a full line starting `ready`), the log rotation (`LogFile::write`, `:660`).
- **Shutdown ordering** (`crates/core/src/daemon/mod.rs`): `serve_until_shutdown` (`:240`)
  starts the services after the socket listens and stops them after `serve()` returns; `run`
  then calls `runtime.shutdown_timeout` and drops the lock (`:229-230`). `freshness::EXIT_WAIT`
  (`freshness.rs:47`) is the lock wait in `client::ensure_daemon` (`client.rs:718`) and
  `bridge::ensure_daemon_detached` (`bridge.rs:148`).
- **`--service` mode** (`addons/web/src/lib.rs`): `serve_here` (`:290`: `watch_stdin` first; no
  URL built; `hub_io::connect_local_plainly()` at `:332`), `token_for` (`:346`: the service only
  loads the token, `auth::load_token`, `auth.rs:76`), `watch_stdin` (`:392`: an OS thread, EOF →
  graceful stop → `exit` 3 s later), `test_hang` (`:410-418`: debug only), `started` (`:546`:
  `ready` and nothing else on stdout), `shutdown` (`:637`). `hub_io.rs:19,34` (`PLAIN_LOCAL`:
  never `ensure_local_daemon` in a service).
- **`[web]` validation** (`addons/web/src/service.rs`): `enabled` (`:58`), `unit` (`:70`:
  every value through the flags' own checks, then `parse_args` on the unit's flags for the
  combination rules), `argv` (`:122`: values in their flag's word), `merge` (`:135`) and
  `run_args` (`:145`), `read_reply` (`:221`: unknown state → `Newer`).
- **The session store** (`addons/web/src/sessions.rs`): `epoch` (`:73`: domain prefix and NUL
  separators; computed once per start in `lib.rs::load_sessions`, `:423`), `load`/`read`
  (`:214`, `:228`: missing or other epoch silent; `O_NOFOLLOW`, checks on the open file, 64 KiB,
  version, malformed or repeated hash), `Store::find` (`:121`: every hash compared with
  `auth::ct_eq`), `hash_of` (`:55`: only 64 lowercase hex is hashed), the writer (`:397`: a
  `watch`, subscribed before any change, the retry delays), `Sessions::written` (`:371`: ≤ 2 s),
  `OpenTab` (`:434`).
- **Cookies and replies** (`addons/web/src/auth.rs`): `set_cookie` (`:460`: `Max-Age=34560000`,
  flags unchanged), `create_session` (`:430`: the old cookie's session out in the same step),
  `logout` (`:549`: waits for the write); `login.rs:59` (the cookie only after the write);
  `ws.rs:65` (`upgrade`: the hash of the cookie the guard checked).
- **One password read** (`addons/web/src/password.rs:108,126`; `remote.rs:195`): the bytes
  that feed the epoch are the bytes `--remote` parses.
- **The `apply` op path**: `WebAddon`'s `DaemonOps` (`addons/web/src/addon.rs:60-84`) →
  `DaemonCtx::service_reload` (`crates/core/src/daemon/addon_ops.rs:77`) →
  `Services::reload`. The CLI (`addons/web/src/cli.rs`): `ask` (`:37`: features only, never an
  error's text; `start` only for `claudio web`), `stored`/`save` (`:75`, `:93`: one unit
  through `config_edit`), `start` (`:124`: checks, then the token, then `remote::prepare`, then
  `[web]`, then the op; nothing saved on a check's failure), `announce` (`:183`: from the
  reply's argv), `status` (`:259`), `restart_if_enabled` (`:325`).
- **The PTY-driven `--set-password` test**: `set_password_typed`
  (`addons/web/tests/web_harness/mod.rs:53`) and its two users in `addons/web/tests/web.rs`
  (`web_set_password_takes_8_bytes_typed_at_a_terminal`,
  `web_service_sign_ins_outlive_a_daemon_restart_but_not_a_new_secret`).
- **Test teardown (AC11)**: `ServiceWeb::finish` and `Drop`
  (`addons/web/tests/web_harness/service.rs`): stop, kill the daemon, every seen service pid
  gone within 5 s, no token in `web.log` or `daemon.log`.

## Task order and parallelism

| Task | Depends on | Repo/files | May run in parallel with |
|---|---|---|---|
| 1 Docs: the daemon protocol and the addon API | none | `docs/daemon-protocol.md`, `docs/addons.md` | 2, 3, 4 |
| 2 C3 amendment | none | `ACCEPTANCE.md` | 1, 3, 4 |
| 3 Cookie bullet | none | `docs/web-protocol.md` | 1, 2 |
| 4 Session store in memory | none | `addons/web/src/{sessions,auth,login,lib}.rs` | 1, 2, 9 |
| 5 Sign-ins persist | 3, 4 | `addons/web/{src/sessions,auth,login,lib,password,remote}.rs`, `addons/web/Cargo.toml`, `Cargo.lock`, `addons/web/tests/web.rs` | 9 |
| 6 An open tab keeps its sign-in | 5 | `addons/web/src/{sessions,ws}.rs` | 9 |
| 7 Palette "Sign out" | 5 | `addons/web/web/app/ui/palette.js`, `addons/web/web/e2e/tests/signout.spec.mjs` | 6, 8, 9 |
| 8 Password minimum | 5 | `docs/web-protocol.md`, `addons/web/src/password.rs`, `addons/web/tests/{web.rs,web_harness/mod.rs,web_harness/tls.rs}` | 6, 7, 9 |
| 9 Core supervisor | 1, 2 | `crates/core/src/{addon,daemon}/…`, `freshness.rs`, `client.rs`, `remote/bridge.rs` | 4-8, 10, 11 |
| 10 `--foreground` and the suites | 8 | `addons/web/src/lib.rs`, `addons/web/tests/…`, `addons/web/web/e2e/fixtures/server.mjs` | 9 |
| 11 `--service` mode | 10 | `addons/web/src/{lib,hub_io,auth}.rs`, `Makefile`, `addons/web/tests/web.rs` | 9 |
| 12 The web addon's service | 1, 9, 11 | `addons/web/src/{service.rs,addon.rs,addon/tests.rs,lib.rs}`, `addons/web/Cargo.toml` | none |
| 13 `claudio web` hands over; `stop`, `status` | 12 | `addons/web/src/{cli,service,lib}.rs`, `addons/web/src/tls/cert.rs`, `addons/web/tests/…` | none |
| 14 A new token or password restarts the service | 13, 8 | `addons/web/src/{cli,lib}.rs`, `addons/web/tests/web.rs` | none |
| 15 User docs | 14 | `docs/web.md`, `README.md`, `Makefile` | none |
| 16 CLAUDE.md (gated on the owner) | 15 | `CLAUDE.md` | none |

Task 9 touches only `crates/core` and may run beside the `addons/web` tasks, but only in a
separate git worktree started from the branch head (its patches apply to either order), with
commits brought back in task order. In one working tree run tasks one at a time: git's index
takes one commit at a time. The step order inside the table is the spec's commit order, with
steps 4-5 (sign-ins, password) before 2-3 (supervisor, web service), as the spec allows.

Acceptance criteria to tasks and tests:

| AC | Task | Tests |
|---|---|---|
| AC1 | 12, 13 | `service.rs`: `an_enabled_section_is_the_service_argv`, `every_argv_parses_back_to_its_unit`, `a_bad_section_names_its_key`, `any_listener_flag_replaces_the_stored_unit_and_none_keeps_it`, `run_args_check_the_merged_unit` |
| AC2 | 9 | the 14 tests of `crates/core/src/daemon/tests/service.rs` |
| AC3 | 13 | `web_hands_its_server_to_the_daemon_and_stop_takes_it_back`, `web_daemon_restart_kills_a_service_that_ignores_its_stop`, `web_stop_and_status_never_start_a_daemon` |
| AC4 | 13 | `web_remote_through_the_daemon_serves_tls_and_prints_the_banner` |
| AC5 | 11, 13 | `web_service_says_ready_never_prints_the_token_and_stops_at_eof`; `ServiceWeb::finish` in every service test |
| AC6 | 10 | the `web`, `vocdoni_web` and e2e suites with `--foreground`; their start-line assertions unchanged |
| AC7 | 9, 13 | `a_daemon_playing_the_previous_release_runs_no_service`; `web_against_the_previous_release_s_daemon_saves_and_does_not_serve` |
| AC8 | every code task; 9, 12 | `make check` at each commit; `make matrix` in Tasks 9 and 12 and before the merge |
| AC9 | 13 | `web_service_comes_back_after_a_sigkill` |
| AC10 | 9 | the grep of Task 9 and of the merge checks |
| AC11 | 9, 13 | `shutdown_stops_the_whole_group`, `a_service_ends_on_stdin_eof_when_its_daemon_dies`; `ServiceWeb::finish` |
| AC12 | 4, 5, 6 | `sessions.rs` tests; `login.rs`: `a_login_and_a_sign_out_are_written_before_their_replies` |
| AC13 | 5, 14 | `web_cookie_outlives_the_browser_and_logout_expires_it`, `web_sign_ins_and_sign_outs_survive_a_killed_server`, `web_a_loopback_sign_in_is_refused_by_a_remote_start`; `web_service_sign_ins_outlive_a_daemon_restart_but_not_a_new_secret` |
| AC14 | 8 | `password_length_is_8_to_1024_bytes`, `web_set_password_takes_8_bytes_typed_at_a_terminal` |
| AC15 | 7 | `signout.spec.mjs` |

## Tasks
### Task 1: Docs: addon services in the daemon protocol and the addon API
Model: sonnet   Repo: /home/p4u/claudio   Files: `docs/daemon-protocol.md`, `docs/addons.md`
Goal: the protocol and API docs land before any code (B2, A3; spec "Commit order" step 1); they
document what AC2, AC7 and AC9 test.

This task has no test: it is documentation. Two commits, one per file.

Step 1: the daemon protocol (features, the service contract, `old_peer`, mixed versions)

Patch `task1-daemon-protocol` (`docs/daemon-protocol.md`):

`````diff
diff --git a/docs/daemon-protocol.md b/docs/daemon-protocol.md
index c9f58ff..e6e1de4 100644
--- a/docs/daemon-protocol.md
+++ b/docs/daemon-protocol.md
@@ -10,6 +10,7 @@ daemon's handling in `crates/core/src/daemon/server.rs`.
 - [Handshake and features](#handshake-and-features)
 - [Requests and replies](#requests-and-replies)
 - [Addon ops](#addon-ops)
+- [Addon services](#addon-services)
 - [Spawn env: `env_remove`](#spawn-env-env_remove)
 - [Optional args: `optional`](#optional-args-optional)
 - [Mixed versions](#mixed-versions)
@@ -44,7 +45,7 @@ the daemon can do beyond the base protocol; a daemon that omits it has none of t
 | `last_prompt` | answers `LastPrompt` |
 | `attention` | sends `SessionEvent::Attention` and fills `SessionInfo.attention` |
 | `worktree` | answers `GitWorktree` and `GitWorktreeDiff` |
-| `addon:<name>` | runs addon `<name>`'s daemon ops (one entry per daemon-side addon) |
+| `addon:<name>` | runs addon `<name>`'s daemon ops (one entry per daemon-side addon), and its service if it asks for one |
 
 A core-only daemon lists no `addon:` feature. A client sends a request that depends on a
 feature only to a daemon that lists it.
@@ -101,6 +102,44 @@ the flags this claude knows, `<cache>/<addon>`, and the lock held while `claude
 `claude plugin …` runs. After the daemon installs claude it calls each daemon-side addon's
 `on_claude_installed`.
 
+## Addon services
+
+A daemon-side addon may ask the daemon to keep one long-running child running: its
+`DaemonOps::service` (docs/addons.md, "Daemon ops") turns the addon's own `config.toml` sections
+into an argv. The daemon's supervisor (`crates/core/src/daemon/service.rs`) names no addon.
+
+- **When it asks.** At daemon start, once the socket listens, the daemon reads `config.toml`
+  (off its runtime) and asks each daemon-side addon; again at every `DaemonCtx::service_reload`
+  (an addon's op asks for one). A `config.toml` that cannot be read or is not TOML fails every
+  addon's service with `config.toml does not parse`; an addon's `Err` fails its own, naming the
+  key. Neither is retried before the next reload or daemon start.
+- **The child.** `Config::claudio` (the binary at the daemon's own path, which an upgrade may
+  have replaced since) with the argv, in a process group of its own, with the daemon's env minus
+  the session markers and with `PATH` set to the login shell's (`Config::path`). Its stdin is a
+  pipe the daemon holds and never writes. Its stdout carries the ready line. Its stderr goes to
+  `<runtime>/<addon>.log` (0600), renamed to `<addon>.log.1` before a write would take it past
+  1 MiB, so an addon's logs never pass 2 MiB; the last 4 KiB since this start are kept in memory
+  as the failure's `tail`.
+- **Ready.** The child's first stdout line starts with `ready` once it serves. The daemon reads
+  at most 4 KiB for it, within 20 s; otherwise the start failed and the child is stopped. Then
+  the daemon reads and discards stdout until EOF and never closes its end while the child runs.
+- **Exit.** Any exit nobody asked for, 0 included, is a failure: the service starts again after
+  a backoff of 1 s that doubles up to 60 s and is reset after 5 min of running. A reload starts a
+  failed service at once.
+- **Stop** (a reload to nothing, a restart, the daemon's shutdown): stdin closed, SIGTERM to the
+  group, SIGKILL to the group after 3 s, reaped. At shutdown every service stops, in parallel,
+  after the socket stopped listening and before the daemon lets go of its startup lock, so a
+  successor's service never runs beside it; a starter waits up to 10 s for that lock. A daemon
+  killed by a signal stops nothing: its children see stdin's EOF and stop themselves within 3 s.
+- **Mixed versions (B1).** A daemon may run a child one release newer or older than itself (an
+  upgrade replaces the binary under a running daemon). For one release these change only
+  compatibly: the argv an addon defines, the first stdout line `ready` (any line that starts
+  with it), stdin's EOF meaning stop, and any unrequested exit being a failure. A child that does
+  not know its argv (a downgraded binary) exits non-zero: an ordinary failure, retried at the
+  backoff cap, with its usage text as the tail.
+- The state is the daemon's own, never journaled, and not in `Welcome`: the addon's ops report
+  it (docs/addons.md, "The web addon").
+
 ## Spawn env: `env_remove`
 
 A claude session's process starts from the daemon's own environment, then loses some variables
@@ -158,10 +197,13 @@ A client and a daemon one release apart work together in both directions.
 | new client → old daemon (busy, deferred, failed check) | no `addon:vocdoni` in features → no `AddonOp`; the launch gets the proxy env without the plugins' share and one notice ("daemon too old, plugins after `claudio daemon restart`"). The old daemon applies its own `env_diff` (a superset of what the addon sends in `env_remove`). |
 | old client → new daemon | `Spawn`/`Respawn` without `env_remove` (`None`) → the daemon's **compatibility scrub** applies today's `env_diff` rule. Its `Manifest` op is refused (`unsupported op`) → the old client's existing "daemon too old" path, session without plugins. |
 | new client → old daemon, `optional` | ignored; the old daemon drops its own fixed three (`--plugin-dir`, `--agents`, `--mcp-config`) when its claude lacks them, which covers every flag a client marks optional within the window. |
+| new `claudio web` → a daemon without `addon:web` (older, or built without the web addon) | no `AddonOp` (the CLI decides on `Welcome.features` only, never on an error's text); `[web]` stays saved and the next daemon start with the web addon serves it. |
+| old `claudio web` → new daemon | it never sends a web op and serves in the foreground, as before; the daemon serves `[web]` only when a new `claudio web` wrote it. |
 
 Debug builds take `CLAUDIO_TEST_OLD_DAEMON=1` (tests only; a release build ignores it): the
 daemon then plays the previous release. It lists no `addon:` feature, answers `AddonOp` with
-`unsupported op: addon_op`, treats every `env_remove` as absent and ignores `optional`.
+`unsupported op: addon_op`, treats every `env_remove` as absent, ignores `optional`, and runs no
+addon service (it ignores `[web]`, as the previous release reads only `[claude]`).
 
 **Removal date.** The compatibility paths for older peers (the compatibility scrub for a spawn
 without `env_remove`, the legacy optional-flag list, the refused-`Manifest` test, and the legacy
`````

Run: `git diff --stat`
Expected: `docs/daemon-protocol.md | 46 ++++++++++++++++++++++++++++++++++++++++++++--`

Commit: `git commit -m "docs(daemon): addon services: the child's contract, features and mixed versions" -- docs/daemon-protocol.md`

Step 2: the addon API (`service`, `service_reload`, `service_state`; the web addon's ops)

Patch `task1-addons` (`docs/addons.md`):

`````diff
diff --git a/docs/addons.md b/docs/addons.md
index b7c08aa..73f25f3 100644
--- a/docs/addons.md
+++ b/docs/addons.md
@@ -308,6 +308,21 @@ the addon creates 0700), and `updating()`, the lock held while `claude update` o
 plugin …` runs, so `~/.claude` has one writer. `on_claude_installed` is called after the daemon
 installs claude.
 
+**Services.** `DaemonOps::service(config)` maps the addon's own config sections (`config` hands it
+only those) to the argv of one long-running child the daemon keeps running: `Ok(Some(argv))`, the
+words after the claudio binary; `Ok(None)` for none (the default); or `Err(msg)` when the
+sections are invalid, `msg` naming the key. It is pure, no I/O. The daemon asks at its start and
+whenever the addon calls `DaemonCtx::service_reload(force)`, which converges: stop, start, or
+restart when the argv changed or `force`; a `Failed` service starts at once, its backoff reset,
+even with the same argv. It resolves once the child is ready, has failed its first attempt, or is
+off (≤ 25 s: a 3 s stop and a 20 s start). Reloads of one addon run one at a time; those queued
+behind a running one run as one. `DaemonCtx::service_state()` reads it: `ServiceState::Off |
+Starting | Running { pid, since, argv } | Failed { exit, signal, tail, retry_in }` (`since` in Unix
+seconds; `tail` the child's last ≤ 4 KiB of stderr, sanitised, or why it did not start;
+`retry_in` until the next start, `None` when only a reload starts it). The child's contract (ready
+line, stdin, exit, logs, stop, mixed versions) is in docs/daemon-protocol.md, "Addon services". A
+daemon playing the previous release runs no service.
+
 **Mixed versions (B1).** A client sends an `AddonOp` only to a host whose `Welcome.features`
 lists `addon:<name>`; `ctx.request` returns `AddonError::Missing` otherwise, and the addon goes on
 without that host's share and says so once (the Vocdoni addon: "daemon on <host> is older:
@@ -337,6 +352,26 @@ maps it; an explicit `[extensions] latex` wins). Its `Ui` lists the `latex` and
 rows (on by default); their toggles are the core's to write. A build without the web addon asks
 claude for no tags.
 
+Its `DaemonOps` make the web a daemon service (docs/web.md, "The web service"). It claims the
+`[web]` config section, which only `claudio web` writes (through `app::config_edit`): `enabled`,
+and one unit of `listen` (loopback) or `remote` (TLS) with `allow_hosts`. While `enabled = true`,
+`service()` answers `web --service`, then `--listen=<addr>` or `--remote=<addr>`, then one
+`--allow-host=<name>` per host (each value in its flag's word, so none can be read as another
+flag), after checking the section as the flags are checked; else nothing, or an error naming the
+key. Its ops:
+
+| Op | Body | Does |
+|---|---|---|
+| `apply` | `{"force": bool}` | `service_reload(force)`: re-reads `[web]` and converges |
+| `status` | `{}` | `service_state()` |
+
+Both answer one object: `{"state":"off"}`, `{"state":"starting"}`,
+`{"state":"running","pid":<u32>,"since":<unix seconds>,"argv":[…]}` or
+`{"state":"failed","exit":<code|null>,"signal":<number|null>,"tail":"…","retry_in":<seconds|null>}`.
+Unknown fields are ignored, and a `state` the CLI does not know means the daemon is newer than it.
+Bodies carry no secret. The CLI sends an op only to a daemon whose `Welcome.features` list
+`addon:web`, and prints the URL of the reply's `argv`, so it describes what runs.
+
 ## The Vocdoni addon
 
 `addons/vocdoni/` (claudio-vocdoni): the claude-proxy client and the team plugins. It owns the `[proxy]` and
`````

Run: `git diff --stat`
Expected: `docs/addons.md | 35 +++++++++++++++++++++++++++++++++++`

Step 3: commit
`git commit -m "docs(addons): DaemonOps::service, service_reload and the web addon's ops" -- docs/addons.md`

Interfaces this task provides to later tasks: none in code. The names it fixes, which Tasks 9 and
12 implement exactly: `DaemonOps::service(&self, config: &dyn Fn(&str) -> toml::Table) ->
Result<Option<Vec<String>>, String>`, `DaemonCtx::service_reload(&self, force: bool) ->
BoxFuture<'static, ServiceState>`, `DaemonCtx::service_state(&self) -> ServiceState`,
`ServiceState::{Off, Starting, Running { pid, since, argv }, Failed { exit, signal, tail,
retry_in }}`; ops `apply` (`{"force": bool}`) and `status` (`{}`) of addon `web`.

### Task 2: C3 admits a supervised service's stdin pipe
Model: haiku   Repo: /home/p4u/claudio   Files: `ACCEPTANCE.md`
Goal: amend C3 in its own commit, before the supervisor (spec "Constraints"; ACCEPTANCE's own
rule: "A rule changes only through a commit to this file that says why").

This task has no test: it changes a rule's text.

Step 1: the amendment, word for word as the spec gives it

Patch `task2` (`ACCEPTANCE.md`):

`````diff
diff --git a/ACCEPTANCE.md b/ACCEPTANCE.md
index e184d74..9b751a8 100644
--- a/ACCEPTANCE.md
+++ b/ACCEPTANCE.md
@@ -99,7 +99,8 @@ sanitised and capped by the core.
 
 **C3. Child processes are owned.** A finite helper command has a deadline and capped output; a
 long-lived child has an owner that cancels it and kills its process group. Both run with a
-scrubbed env and stdin `/dev/null` unless interactive.
+scrubbed env and stdin `/dev/null` unless interactive; a supervised service's stdin is a pipe its
+owner holds and never writes, so the child sees its owner's death.
 
 **C4. Files claudio writes** are private data at 0600 in 0700 directories, replaced atomically
 (`paths::write_atomic`); executables keep their executable mode; claudio never chmods a
`````

Run: `git diff --stat`
Expected: `ACCEPTANCE.md | 3 ++-`

Step 2: commit (the body says why, as ACCEPTANCE requires)
`git commit -m "docs(acceptance): C3 admits a supervised service's stdin pipe" -m "An addon service's stdin is a pipe the daemon holds and never writes: however the daemon dies, the child sees EOF and stops itself, so no service outlives its owner (docs/specs/2026-11-09-web-service-design.md)." -- ACCEPTANCE.md`

Interfaces this task provides to later tasks: none.

### Task 3: The cookie's Max-Age and sessions kept across restarts in the web protocol
Model: haiku   Repo: /home/p4u/claudio   Files: `docs/web-protocol.md`
Goal: `docs/web-protocol.md`'s Cookie bullet (lines 53-55) in its own commit before the code
(B2; spec "Commit order" step 4; serves AC13).

This task has no test: it is the protocol document.

Step 1: the Cookie bullet

Patch `task3` (`docs/web-protocol.md`):

`````diff
diff --git a/docs/web-protocol.md b/docs/web-protocol.md
index 10c9349..9540e94 100644
--- a/docs/web-protocol.md
+++ b/docs/web-protocol.md
@@ -50,9 +50,9 @@ request with it, `["password"]` for any other request.
   each `--allow-host` name; all at the listen port, and also bare when the port is 443.
 - Accepted `Origin`: `https://` (and, without `--remote`, `http://`) followed by an accepted
   Host value, nothing after it. `null` is refused.
-- Cookie: `claudio_web=<64 hex>; HttpOnly; SameSite=Strict; Path=/`, plus `; Secure` when the
-  server runs with `--remote` or with any `--allow-host`. Server-side sessions: 24 h idle expiry, at most 64
-  (the least recently used goes).
+- Cookie: `claudio_web=<64 hex>; Max-Age=34560000; HttpOnly; SameSite=Strict; Path=/`, plus
+  `; Secure` when the server runs with `--remote` or with any `--allow-host`. Server-side sessions:
+  30 days idle, kept across restarts, at most 64 (the least recently used goes).
 - Login limits, taken **before** a token is compared or a password hashed (a reservation,
   so concurrent attempts cannot all pass): 5 attempts per peer per 60 s (every loopback
   address is one peer; an IPv4 address is one, an IPv6 address counts as its /64), 30 per
`````

Run: `git diff --stat`
Expected: `docs/web-protocol.md | 6 +++---`

Step 2: commit
`git commit -m "docs(web-protocol): the cookie's max-age and sessions kept across restarts" -- docs/web-protocol.md`

Interfaces this task provides to later tasks: none.

### Task 4: Sessions are hashed and live 30 days idle on wall-clock time
Model: opus   Repo: /home/p4u/claudio   Files: `addons/web/src/sessions.rs` (new),
`addons/web/src/auth.rs`, `addons/web/src/login.rs`, `addons/web/src/lib.rs`
Goal: the pure session store of spec §3 (AC12: idle at 30 days on wall-clock `seen`, a `seen`
in the future counts as now, the cap of 64, a login with a live cookie replaces its session),
in memory only for now; `Auth` uses it.

`auth.rs`'s `Inner` (a `Vec<(String, Instant)>` of cookie values) goes. The store keeps the
SHA-256 of each cookie value and its last use in Unix seconds; a value that is not 64
lowercase hex is never hashed. `Auth::new` takes the store (`sessions::Sessions`). The test
`sessions_expire_when_idle_and_are_capped` (it asserted the old 24 h `Instant` rule) is replaced
by the store's tests of the 30-day rule and the cap, and by
`a_session_is_made_used_replaced_and_ended_on_unix_seconds` (E2).

Step 1: write the failing tests (the store's test module; the replaced `auth.rs` test; every
test `Auth::new` with its new argument; `mod sessions;`)

Patch `task4-step1` (`addons/web/src/auth.rs`, `addons/web/src/lib.rs`, `addons/web/src/login.rs`, `addons/web/src/sessions.rs`):

`````diff
diff --git a/addons/web/src/auth.rs b/addons/web/src/auth.rs
index f24aa19..1e4a7ef 100644
--- a/addons/web/src/auth.rs
+++ b/addons/web/src/auth.rs
@@ -586,7 +586,8 @@ mod tests {
     use super::*;
 
     fn auth() -> Auth {
-        Auth::new("ab".repeat(32), Allowed::new(lo(7337), &[]), csp(None)).unwrap()
+        let allowed = Allowed::new(lo(7337), &[]);
+        Auth::new("ab".repeat(32), allowed, csp(None), Sessions::memory()).unwrap()
     }
 
     fn lo(port: u16) -> SocketAddr {
@@ -725,6 +726,7 @@ mod tests {
             "ab".repeat(32),
             Allowed::new(lo(7337), &names(&["proxy.example"])),
             csp(None),
+            Sessions::memory(),
         )
         .unwrap();
         for (p, host) in [
@@ -817,7 +819,7 @@ mod tests {
         assert!(a.host_ok("192.168.1.20:7337"));
         assert!(!a.is_local_host("192.168.1.20:7337"));
         assert!(a.is_local_host("127.0.0.1:7337") && a.is_local_host("[::1]:7337"));
-        let auth = Auth::new("ab".repeat(32), a, csp(None)).unwrap();
+        let auth = Auth::new("ab".repeat(32), a, csp(None), Sessions::memory()).unwrap();
         let c = auth.caller(peer("127.0.0.1:1"), "192.168.1.20:7337");
         assert!(!c.local && !auth.token_allowed(&c));
         let a = Allowed::new(listen, &[]);
@@ -833,7 +835,13 @@ mod tests {
 
     #[test]
     fn with_remote_the_token_is_for_local_requests_only() {
-        let a = Auth::new("ab".repeat(32), remote_allowed(), csp(None)).unwrap();
+        let a = Auth::new(
+            "ab".repeat(32),
+            remote_allowed(),
+            csp(None),
+            Sessions::memory(),
+        )
+        .unwrap();
         for (p, host) in [
             ("127.0.0.1:1", "127.0.0.1:7337"),
             ("[::ffff:127.0.0.1]:1", "localhost:7337"),
@@ -867,9 +875,14 @@ mod tests {
     #[test]
     fn with_a_password_every_caller_may_use_it() {
         const FIXTURE: &str = include_str!("../web/e2e/fixtures/web-password");
-        let a = Auth::new("ab".repeat(32), remote_allowed(), csp(None))
-            .unwrap()
-            .with_password(password::Stored::parse(FIXTURE).unwrap());
+        let a = Auth::new(
+            "ab".repeat(32),
+            remote_allowed(),
+            csp(None),
+            Sessions::memory(),
+        )
+        .unwrap()
+        .with_password(password::Stored::parse(FIXTURE).unwrap());
         let local = a.caller(peer("127.0.0.1:1"), "127.0.0.1:7337");
         assert_eq!(a.login_methods(&local), ["token", "password"]);
         let remote = a.caller(peer("192.168.1.5:1"), "box:7337");
@@ -878,7 +891,13 @@ mod tests {
 
     #[test]
     fn the_test_peer_hook_only_takes_the_token_away() {
-        let mut a = Auth::new("ab".repeat(32), remote_allowed(), csp(None)).unwrap();
+        let mut a = Auth::new(
+            "ab".repeat(32),
+            remote_allowed(),
+            csp(None),
+            Sessions::memory(),
+        )
+        .unwrap();
         a.test_remote_peer = Some("127.0.0.2".parse().unwrap());
         let c = a.caller(peer("127.0.0.2:1"), "127.0.0.1:7337");
         assert!(!c.local);
@@ -887,29 +906,18 @@ mod tests {
     }
 
     #[test]
-    fn sessions_expire_when_idle_and_are_capped() {
+    fn a_session_is_made_used_replaced_and_ended_on_unix_seconds() {
         let a = auth();
-        let t0 = Instant::now();
-        let s = a.create_session(t0).unwrap();
+        let t0 = 1_800_000_000;
+        let s = a.create_session(None, t0).unwrap();
         assert!(token_ok(&s));
-        let t1 = t0 + SESSION_IDLE - Duration::from_secs(1);
-        assert!(a.session_ok(&s, t1), "a use within the idle time keeps it");
-        assert!(
-            a.session_ok(&s, t1 + SESSION_IDLE - Duration::from_secs(1)),
-            "and refreshes it"
-        );
-        assert!(!a.session_ok(&s, t1 + 2 * SESSION_IDLE), "idle too long");
-        let a = auth();
-        let oldest = a.create_session(t0).unwrap();
-        for i in 1..=MAX_SESSIONS as u64 {
-            a.create_session(t0 + Duration::from_secs(i)).unwrap();
-        }
-        assert!(
-            !a.session_ok(&oldest, t0 + Duration::from_secs(100)),
-            "the least recently used went"
-        );
+        assert!(a.session_ok(&s, t0 + sessions::IDLE - 1));
+        let replaced = a.create_session(Some(&s), t0 + 10).unwrap();
+        assert!(!a.session_ok(&s, t0 + 11), "the login replaced it");
+        assert!(a.session_ok(&replaced, t0 + 11));
+        a.end_session(&replaced);
+        assert!(!a.session_ok(&replaced, t0 + 12));
         a.end_session("nothing");
-        assert!(!a.session_ok("", t0 + Duration::from_secs(100)));
     }
 
     #[test]
diff --git a/addons/web/src/lib.rs b/addons/web/src/lib.rs
index 9131506..060995b 100644
--- a/addons/web/src/lib.rs
+++ b/addons/web/src/lib.rs
@@ -22,6 +22,7 @@ mod password;
 mod peer;
 mod remote;
 mod session_events;
+mod sessions;
 mod spawn;
 mod state;
 mod tls;
diff --git a/addons/web/src/login.rs b/addons/web/src/login.rs
index 70d9b55..bea78ee 100644
--- a/addons/web/src/login.rs
+++ b/addons/web/src/login.rs
@@ -127,15 +127,21 @@ mod tests {
     use crate::limit::GLOBAL;
     use crate::password;
     use crate::peer::PeerKey;
+    use crate::sessions::Sessions;
 
     const FIXTURE: &str = include_str!("../web/e2e/fixtures/web-password");
     const TEST_PASSWORD: &str = "claudio-e2e-password";
 
     fn remote() -> Auth {
         let any = std::net::SocketAddr::from(([0, 0, 0, 0], 7337));
-        Auth::new("ab".repeat(32), Allowed::remote(any, &[]), csp(None))
-            .unwrap()
-            .with_password(password::Stored::parse(FIXTURE).unwrap())
+        Auth::new(
+            "ab".repeat(32),
+            Allowed::remote(any, &[]),
+            csp(None),
+            Sessions::memory(),
+        )
+        .unwrap()
+        .with_password(password::Stored::parse(FIXTURE).unwrap())
     }
 
     const REMOTE: Caller = Caller {
@@ -145,7 +151,13 @@ mod tests {
 
     fn plain() -> Auth {
         let lo = std::net::SocketAddr::from(([127, 0, 0, 1], 7337));
-        Auth::new("ab".repeat(32), Allowed::new(lo, &[]), csp(None)).unwrap()
+        Auth::new(
+            "ab".repeat(32),
+            Allowed::new(lo, &[]),
+            csp(None),
+            Sessions::memory(),
+        )
+        .unwrap()
     }
 
     const LOCAL: Caller = Caller {
@@ -180,7 +192,13 @@ mod tests {
     #[test]
     fn with_remote_a_non_local_token_is_refused_and_counts() {
         let any = std::net::SocketAddr::from(([0, 0, 0, 0], 7337));
-        let a = Auth::new("ab".repeat(32), Allowed::remote(any, &[]), csp(None)).unwrap();
+        let a = Auth::new(
+            "ab".repeat(32),
+            Allowed::remote(any, &[]),
+            csp(None),
+            Sessions::memory(),
+        )
+        .unwrap();
         let right = "ab".repeat(32);
         let remote = Caller {
             local: false,
diff --git a/addons/web/src/sessions.rs b/addons/web/src/sessions.rs
new file mode 100644
index 0000000..7808fc8
--- /dev/null
+++ b/addons/web/src/sessions.rs
@@ -0,0 +1,98 @@
+#[cfg(test)]
+mod tests {
+    use super::*;
+
+    const DAY: u64 = 24 * 60 * 60;
+    const T0: u64 = 1_800_000_000;
+
+    /// A cookie value: 64 lowercase hex from `n`.
+    fn value(n: u64) -> String {
+        format!("{n:064x}")
+    }
+
+    fn login(s: &mut Store, n: u64, now: u64) -> String {
+        let v = value(n);
+        s.login(hash_of(&v).unwrap(), None, now);
+        v
+    }
+
+    /// Whether `v` names a live session at `now` (a use).
+    fn used(s: &mut Store, v: &str, now: u64) -> bool {
+        s.use_value(v, now)
+    }
+
+    #[test]
+    fn only_64_lowercase_hex_is_hashed() {
+        assert!(hash_of(&value(1)).is_some());
+        for bad in ["", "ab", &"A".repeat(64), &"g".repeat(64), &"a".repeat(65)] {
+            assert_eq!(hash_of(bad), None, "{bad}");
+        }
+        let mut s = Store::default();
+        let v = login(&mut s, 0xabc, T0);
+        assert!(!used(&mut s, &v.to_uppercase(), T0));
+        assert!(used(&mut s, &v, T0));
+    }
+
+    #[test]
+    fn a_session_lives_30_days_from_its_last_use_on_wall_clock_time() {
+        let mut s = Store::default();
+        let v = login(&mut s, 1, T0);
+        assert!(used(&mut s, &v, T0 + IDLE - 1), "live at 30 days - 1 s");
+        assert!(
+            used(&mut s, &v, T0 + IDLE - 1 + IDLE - 1),
+            "a use refreshes it"
+        );
+        let mut s = Store::default();
+        let v = login(&mut s, 1, T0);
+        assert!(!used(&mut s, &v, T0 + IDLE), "gone at 30 days");
+        assert_eq!(IDLE, 30 * DAY);
+    }
+
+    #[test]
+    fn a_seen_later_than_now_counts_as_now() {
+        let mut s = Store::default();
+        // The clock ran 10 days ahead at the login, then was corrected.
+        let v = login(&mut s, 1, T0 + 10 * DAY);
+        assert!(used(&mut s, &v, T0), "pulled back to now");
+        assert!(!used(&mut s, &v, T0 + IDLE), "and idle from there");
+    }
+
+    #[test]
+    fn a_65th_login_drops_the_least_recently_seen() {
+        let mut s = Store::default();
+        let first = login(&mut s, 0, T0);
+        let second = login(&mut s, 1, T0 + 1);
+        assert!(used(&mut s, &first, T0 + 2), "first used last");
+        for n in 2..MAX as u64 {
+            login(&mut s, n, T0 + 2 + n);
+        }
+        assert_eq!(s.entries.len(), MAX);
+        login(&mut s, 999, T0 + 100);
+        assert_eq!(s.entries.len(), MAX);
+        assert!(
+            !used(&mut s, &second, T0 + 101),
+            "the least recently seen went"
+        );
+        assert!(used(&mut s, &first, T0 + 101));
+    }
+
+    #[test]
+    fn a_login_with_a_live_cookie_replaces_its_session() {
+        let mut s = Store::default();
+        let old = login(&mut s, 1, T0);
+        let new = value(2);
+        s.login(hash_of(&new).unwrap(), Some(&old), T0 + 1);
+        assert_eq!(s.entries.len(), 1);
+        assert!(!used(&mut s, &old, T0 + 2));
+        assert!(used(&mut s, &new, T0 + 2));
+    }
+
+    #[test]
+    fn a_sign_out_ends_only_its_session() {
+        let mut s = Store::default();
+        let (a, b) = (login(&mut s, 1, T0), login(&mut s, 2, T0));
+        s.remove(&a);
+        s.remove("not a cookie");
+        assert!(!used(&mut s, &a, T0) && used(&mut s, &b, T0));
+    }
+}
`````

Run: `cargo test --locked -p claudio-web --lib sessions`
Expected: compilation fails, among others with `` error[E0425]: cannot find type `Store` in this scope ``
and `error[E0061]: this function takes 3 arguments but 4 arguments were supplied`.

Step 2: implement

Patch `task4-step2` (`addons/web/src/auth.rs`, `addons/web/src/lib.rs`, `addons/web/src/login.rs`, `addons/web/src/sessions.rs`):

`````diff
diff --git a/addons/web/src/auth.rs b/addons/web/src/auth.rs
index 1e4a7ef..3356b01 100644
--- a/addons/web/src/auth.rs
+++ b/addons/web/src/auth.rs
@@ -2,8 +2,8 @@
 //!
 //! A secret token (32 random bytes, hex) lives in `<config>/web-token`
 //! (0600). The page gets it in the URL fragment and trades it at
-//! `POST /api/login` for a cookie naming a server-side session (in memory,
-//! 24 h idle expiry, at most 64). [`guard`] checks every request: its
+//! `POST /api/login` for a cookie naming a server-side session
+//! (`sessions.rs`: 30 days idle on wall-clock time, at most 64). [`guard`] checks every request: its
 //! `Host` must be one this server answers to (DNS rebinding); anything but
 //! `GET`/`HEAD`, and every WebSocket upgrade, must come from an accepted
 //! `Origin` (CSRF, cross-site WebSocket hijacking); `/api/*` (but login)
@@ -15,8 +15,7 @@ use std::fs::File;
 use std::io::{self, Read};
 use std::net::{IpAddr, SocketAddr};
 use std::path::{Path, PathBuf};
-use std::sync::{Arc, Mutex, MutexGuard};
-use std::time::{Duration, Instant};
+use std::sync::{Arc, Mutex};
 
 use axum::extract::{Request, State};
 use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
@@ -30,11 +29,10 @@ use tokio::sync::Semaphore;
 use crate::limit::Limiter;
 use crate::password;
 use crate::peer::{Peer, PeerKey};
+use crate::sessions::{self, Sessions};
 
 pub(crate) const COOKIE: &str = "claudio_web";
 pub(crate) const TOKEN_BYTES: usize = 32;
-pub(crate) const SESSION_IDLE: Duration = Duration::from_secs(24 * 60 * 60);
-pub(crate) const MAX_SESSIONS: usize = 64;
 pub(crate) const LOGIN_BODY_MAX: usize = 4096;
 /// Concurrent argon2 verifications (each 19 MiB).
 pub(crate) const HASH_PERMITS: usize = 4;
@@ -285,12 +283,6 @@ pub(crate) fn csp(importmap_sha256: Option<&str>) -> String {
     )
 }
 
-#[derive(Default)]
-struct Inner {
-    /// Cookie values and their last use.
-    sessions: Vec<(String, Instant)>,
-}
-
 /// A request's sender as the login rules see it.
 #[derive(Debug, Clone, Copy, PartialEq, Eq)]
 pub(crate) struct Caller {
@@ -320,7 +312,8 @@ pub(crate) struct Auth {
     /// Login attempts, reserved before any check (`limit.rs`).
     pub(crate) limiter: Limiter,
     csp: HeaderValue,
-    inner: Mutex<Inner>,
+    /// The sessions cookies name (`sessions.rs`).
+    pub(crate) sessions: Arc<Sessions>,
     /// `/dev/urandom`, opened once at start: a login opens no file on the
     /// executor. Read under its own lock, never held across an `.await`.
     urandom: Mutex<File>,
@@ -347,13 +340,18 @@ fn test_remote_peer() -> Option<IpAddr> {
 
 impl Auth {
     /// Fails only when `/dev/urandom` cannot be opened.
-    pub(crate) fn new(token: String, allowed: Allowed, csp: String) -> io::Result<Auth> {
+    pub(crate) fn new(
+        token: String,
+        allowed: Allowed,
+        csp: String,
+        sessions: Sessions,
+    ) -> io::Result<Auth> {
         Ok(Auth {
             token,
             allowed,
             limiter: Limiter::default(),
             csp: HeaderValue::from_str(&csp).expect("an ASCII CSP"),
-            inner: Mutex::new(Inner::default()),
+            sessions: Arc::new(sessions),
             urandom: Mutex::new(File::open("/dev/urandom")?),
             test_remote_peer: test_remote_peer(),
             password: None,
@@ -369,10 +367,6 @@ impl Auth {
         self
     }
 
-    fn lock(&self) -> MutexGuard<'_, Inner> {
-        self.inner.lock().unwrap_or_else(|e| e.into_inner())
-    }
-
     /// `n` random bytes from the open `/dev/urandom`, as lowercase hex.
     fn random_hex(&self, n: usize) -> io::Result<String> {
         let mut urandom = self.urandom.lock().unwrap_or_else(|e| e.into_inner());
@@ -420,55 +414,23 @@ impl Auth {
         ct_eq(token.as_bytes(), self.token.as_bytes())
     }
 
-    /// A new session's cookie value. Idle ones are dropped first; past
-    /// [`MAX_SESSIONS`], the least recently used goes.
-    pub(crate) fn create_session(&self, now: Instant) -> io::Result<String> {
+    /// A new session's cookie value; the session of `old` (the cookie the
+    /// login came with) goes in the same step. `now`: Unix seconds.
+    pub(crate) fn create_session(&self, old: Option<&str>, now: u64) -> io::Result<String> {
         let value = self.random_hex(TOKEN_BYTES)?;
-        let mut inner = self.lock();
-        inner
-            .sessions
-            .retain(|(_, seen)| now.duration_since(*seen) < SESSION_IDLE);
-        if inner.sessions.len() >= MAX_SESSIONS {
-            let oldest = inner
-                .sessions
-                .iter()
-                .enumerate()
-                .min_by_key(|(_, (_, seen))| *seen)
-                .map(|(i, _)| i);
-            if let Some(i) = oldest {
-                inner.sessions.remove(i);
-            }
-        }
-        inner.sessions.push((value.clone(), now));
+        let hash = sessions::hash_of(&value).ok_or_else(|| io::Error::other("not a token"))?;
+        self.sessions.login(hash, old, now);
         Ok(value)
     }
 
-    /// Whether `value` names a live session (every one is compared in
-    /// constant time); refreshes its last use.
-    pub(crate) fn session_ok(&self, value: &str, now: Instant) -> bool {
-        let mut inner = self.lock();
-        inner
-            .sessions
-            .retain(|(_, seen)| now.duration_since(*seen) < SESSION_IDLE);
-        let mut found = None;
-        for (i, (v, _)) in inner.sessions.iter().enumerate() {
-            if ct_eq(v.as_bytes(), value.as_bytes()) {
-                found = Some(i);
-            }
-        }
-        match found {
-            Some(i) => {
-                inner.sessions[i].1 = now;
-                true
-            }
-            None => false,
-        }
+    /// Whether `value` names a live session (a use).
+    pub(crate) fn session_ok(&self, value: &str, now: u64) -> bool {
+        self.sessions.check(value, now)
     }
 
+    /// Sign `value`'s session out.
     pub(crate) fn end_session(&self, value: &str) {
-        self.lock()
-            .sessions
-            .retain(|(v, _)| !ct_eq(v.as_bytes(), value.as_bytes()));
+        self.sessions.sign_out(value);
     }
 }
 
@@ -528,7 +490,8 @@ fn check(auth: &Auth, req: &Request) -> Result<(), StatusCode> {
     }
     let path = req.uri().path();
     if path == "/ws" || (is_api(path) && path != "/api/login") {
-        let live = cookie_value(headers).is_some_and(|v| auth.session_ok(&v, Instant::now()));
+        let now = paths::unix_now();
+        let live = cookie_value(headers).is_some_and(|v| auth.session_ok(&v, now));
         if !live {
             return Err(StatusCode::UNAUTHORIZED);
         }
diff --git a/addons/web/src/lib.rs b/addons/web/src/lib.rs
index 060995b..fb0b3e4 100644
--- a/addons/web/src/lib.rs
+++ b/addons/web/src/lib.rs
@@ -360,7 +360,8 @@ async fn serve(
         Some(r) => r.allowed.clone(),
         None => auth::Allowed::new(args.listen, &args.allow_hosts),
     };
-    let mut auth = auth::Auth::new(token, allowed, auth::csp(assets.importmap_sha256()))
+    let csp = auth::csp(assets.importmap_sha256());
+    let mut auth = auth::Auth::new(token, allowed, csp, sessions::Sessions::memory())
         .map_err(|e| format!("cannot open /dev/urandom: {e}"))?;
     if let Some(r) = &remote {
         auth = auth.with_password(r.password.clone());
diff --git a/addons/web/src/login.rs b/addons/web/src/login.rs
index bea78ee..823c0d3 100644
--- a/addons/web/src/login.rs
+++ b/addons/web/src/login.rs
@@ -14,6 +14,7 @@ use axum::response::{IntoResponse, Response};
 use serde::Deserialize;
 
 use crate::auth::{self, Auth, Caller};
+use claudio_core::paths;
 
 /// How long a password login waits for a hashing slot before `503`.
 pub(crate) const HASH_WAIT: Duration = Duration::from_secs(2);
@@ -27,6 +28,8 @@ struct LoginBody {
 
 pub(crate) async fn login(State(auth): State<Arc<Auth>>, req: Request) -> Response {
     let caller = auth.caller(auth::peer_of(&req), auth::host_of(&req));
+    // A login with a cookie replaces that cookie's session.
+    let old = auth::cookie_value(req.headers());
     let body = match axum::body::to_bytes(req.into_body(), auth::LOGIN_BODY_MAX).await {
         Ok(body) => body,
         Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
@@ -50,7 +53,7 @@ pub(crate) async fn login(State(auth): State<Arc<Auth>>, req: Request) -> Respon
         );
         return status.into_response();
     }
-    match auth.create_session(Instant::now()) {
+    match auth.create_session(old.as_deref(), paths::unix_now()) {
         Ok(value) => {
             let mut res = StatusCode::NO_CONTENT.into_response();
             let cookie = auth::set_cookie(&value, auth.allowed.secure_cookie());
diff --git a/addons/web/src/sessions.rs b/addons/web/src/sessions.rs
index 7808fc8..1cdc917 100644
--- a/addons/web/src/sessions.rs
+++ b/addons/web/src/sessions.rs
@@ -1,3 +1,144 @@
+//! Sign-ins (docs/web.md, "Cookie"; docs/web-protocol.md §1): the
+//! server-side sessions a `claudio_web` cookie names.
+//!
+//! [`Store`] is pure: each entry is the SHA-256 of a cookie value and its
+//! last use in wall-clock Unix seconds; a session unused for 30 days is
+//! gone, and past 64 the least recently used goes. `now` is a parameter
+//! (A6). [`Sessions`] holds the store the server uses.
+
+use std::sync::{Mutex, MutexGuard};
+
+use sha2::{Digest, Sha256};
+
+use crate::auth::{ct_eq, token_ok};
+
+/// A session unused this long (seconds) is gone.
+pub(crate) const IDLE: u64 = 30 * 24 * 60 * 60;
+/// Sessions kept at most; past it the least recently used goes.
+pub(crate) const MAX: usize = 64;
+
+/// The SHA-256 of a cookie value.
+pub(crate) type Hash = [u8; 32];
+
+/// The SHA-256 of `value` when it is one this server could have made (64
+/// lowercase hex); anything else is refused without hashing.
+pub(crate) fn hash_of(value: &str) -> Option<Hash> {
+    token_ok(value).then(|| Sha256::digest(value.as_bytes()).into())
+}
+
+#[derive(Debug, Clone, PartialEq, Eq)]
+struct Entry {
+    hash: Hash,
+    /// Last use, Unix seconds.
+    seen: u64,
+}
+
+/// The sessions, pure: every method takes `now`.
+#[derive(Debug, Clone, Default, PartialEq, Eq)]
+pub(crate) struct Store {
+    entries: Vec<Entry>,
+}
+
+impl Store {
+    /// A `seen` later than `now` pulled back to `now`, idle entries
+    /// dropped, and of more than [`MAX`] the most recently seen kept.
+    fn tidy(&mut self, now: u64) {
+        for e in &mut self.entries {
+            e.seen = e.seen.min(now);
+        }
+        self.entries.retain(|e| now - e.seen < IDLE);
+        if self.entries.len() > MAX {
+            self.entries.sort_by_key(|e| std::cmp::Reverse(e.seen));
+            self.entries.truncate(MAX);
+        }
+    }
+
+    /// The entry `hash` names, every hash compared in constant time.
+    fn find(&self, hash: &Hash) -> Option<usize> {
+        let mut found = None;
+        for (i, e) in self.entries.iter().enumerate() {
+            if ct_eq(&e.hash, hash) {
+                found = Some(i);
+            }
+        }
+        found
+    }
+
+    /// A login: `new`'s session in, and `old`'s (the cookie the login came
+    /// with) out in the same step; past [`MAX`] the least recently seen goes.
+    pub(crate) fn login(&mut self, new: Hash, old: Option<&str>, now: u64) {
+        if let Some(old) = old {
+            self.remove(old);
+        }
+        self.tidy(now);
+        if self.entries.len() >= MAX {
+            let oldest = (0..self.entries.len()).min_by_key(|&i| self.entries[i].seen);
+            if let Some(i) = oldest {
+                self.entries.remove(i);
+            }
+        }
+        self.entries.push(Entry {
+            hash: new,
+            seen: now,
+        });
+    }
+
+    /// Whether `value` names a live session; a use: its `seen` becomes `now`.
+    pub(crate) fn use_value(&mut self, value: &str, now: u64) -> bool {
+        let Some(hash) = hash_of(value) else {
+            return false;
+        };
+        self.tidy(now);
+        match self.find(&hash) {
+            Some(i) => {
+                self.entries[i].seen = now;
+                true
+            }
+            None => false,
+        }
+    }
+
+    /// A sign-out: `value`'s session out.
+    pub(crate) fn remove(&mut self, value: &str) {
+        if let Some(i) = hash_of(value).and_then(|h| self.find(&h)) {
+            self.entries.remove(i);
+        }
+    }
+}
+
+/// The store the server uses, shared by every request.
+#[derive(Default)]
+pub(crate) struct Sessions {
+    store: Mutex<Store>,
+}
+
+impl Sessions {
+    /// An empty store.
+    pub(crate) fn memory() -> Sessions {
+        Sessions::default()
+    }
+
+    /// Never held across an await.
+    fn lock(&self) -> MutexGuard<'_, Store> {
+        self.store.lock().unwrap_or_else(|e| e.into_inner())
+    }
+
+    /// [`Store::login`].
+    pub(crate) fn login(&self, new: Hash, old: Option<&str>, now: u64) {
+        self.lock().login(new, old, now);
+    }
+
+    /// [`Store::use_value`].
+    pub(crate) fn check(&self, value: &str, now: u64) -> bool {
+        self.lock().use_value(value, now)
+    }
+
+    /// [`Store::remove`].
+    pub(crate) fn sign_out(&self, value: &str) {
+        self.lock().remove(value);
+    }
+}
+
 #[cfg(test)]
 mod tests {
     use super::*;
`````

Run: `cargo test --locked -p claudio-web --lib`, then the `make check` sequence of the Global
Constraints.
Expected: `test result: ok. 98 passed; 0 failed`, and `rules: 0 problem(s)`.

Step 3: commit
`git add addons/web/src/sessions.rs && git commit -m "feat(web): sessions are hashed and live 30 days idle on wall-clock time" -- addons/web/src/sessions.rs addons/web/src/auth.rs addons/web/src/login.rs addons/web/src/lib.rs`

Interfaces this task provides to later tasks: `sessions::{IDLE: u64, MAX: usize, Hash = [u8;
32], hash_of(&str) -> Option<Hash>, Store}` with `Store::login(&mut self, new: Hash, old:
Option<&str>, now: u64)`, `Store::use_value(&mut self, value: &str, now: u64) -> bool` (replaced
in Task 5), `Store::remove(&mut self, value: &str)`; `sessions::Sessions::{memory, login, check,
sign_out}`; `auth::Auth::new(token, allowed, csp, sessions: Sessions)`, `Auth.sessions:
Arc<Sessions>`, `Auth::create_session(&self, old: Option<&str>, now: u64)`,
`Auth::session_ok(&self, value, now: u64)`.

### Task 5: Sign-ins persist in web-sessions.json, bound to an epoch
Model: opus   Repo: /home/p4u/claudio   Files: `addons/web/src/sessions.rs`,
`addons/web/src/auth.rs`, `addons/web/src/login.rs`, `addons/web/src/lib.rs`,
`addons/web/src/password.rs`, `addons/web/src/remote.rs`, `addons/web/Cargo.toml`, `Cargo.lock`,
`addons/web/tests/web.rs`
Goal: spec §3's file, epoch, loader and writer (AC12: round trip, 0600, no cookie value or
token in the file; the epoch's three inputs; silent missing/other-epoch files and one-line
reasons for bad ones; a login's entry in the file and a signed-out one out of it before the
reply; `seen` written at most once an hour; a failed write kept and retried) and the cookie's
`Max-Age=34560000` (AC13: the `Set-Cookie` of a login and of a sign-out; a cookie valid across a
killed `--foreground` server and a signed-out one refused across it; `--rotate-token` refuses
the old cookie; a loopback sign-in refused by a `--remote` start).

What changes: `sessions.rs` gains `epoch`, `Mode`, `load`, the version-1 file format and the
writer task (a `tokio::sync::watch` of change numbers; the reply of a login or sign-out waits
up to 2 s for its write; a failed write is retried at the next change or after 60 s doubling
to 1 h). `Store::use_value` becomes `Store::use_hash`, which also says when the file's `seen`
is over an hour old. `password::load` splits into `password::read` (the private-file checks,
the bytes) and `password::parse_file`, so one read feeds both the epoch and, with `--remote`,
`remote::prepare` (now given the bytes). `lib.rs::run` loads the store before the runtime
(`load_sessions`; a bad file prints one `eprintln!` line, never a failed start); `serve` starts
the writer and flushes in its stop sequence after `hub.shutdown()`. `addons/web/Cargo.toml`
takes `libc` (already a workspace dependency, and claudio-core's) for `O_NOFOLLOW`; the patch
includes the one-line `Cargo.lock` change, so `--locked` keeps working.

Step 1: write the failing tests (the store's file, epoch and writer tests; the login/logout
test; the `auth.rs` cookie test; three `web.rs` tests)

Patch `task5-step1` (`addons/web/src/auth.rs`, `addons/web/src/login.rs`, `addons/web/src/sessions.rs`, `addons/web/tests/web.rs`):

`````diff
diff --git a/addons/web/src/auth.rs b/addons/web/src/auth.rs
index 3356b01..4eae9a2 100644
--- a/addons/web/src/auth.rs
+++ b/addons/web/src/auth.rs
@@ -872,10 +872,10 @@ mod tests {
     fn a_session_is_made_used_replaced_and_ended_on_unix_seconds() {
         let a = auth();
         let t0 = 1_800_000_000;
-        let s = a.create_session(None, t0).unwrap();
+        let (s, _) = a.create_session(None, t0).unwrap();
         assert!(token_ok(&s));
         assert!(a.session_ok(&s, t0 + sessions::IDLE - 1));
-        let replaced = a.create_session(Some(&s), t0 + 10).unwrap();
+        let (replaced, _) = a.create_session(Some(&s), t0 + 10).unwrap();
         assert!(!a.session_ok(&s, t0 + 11), "the login replaced it");
         assert!(a.session_ok(&replaced, t0 + 11));
         a.end_session(&replaced);
@@ -883,6 +883,27 @@ mod tests {
         a.end_session("nothing");
     }
 
+    #[test]
+    fn the_cookie_outlives_the_browser_and_the_sign_out_cookie_expires_it() {
+        let set = set_cookie(&"ab".repeat(32), false);
+        assert_eq!(
+            set.to_str().unwrap(),
+            format!(
+                "claudio_web={}; Max-Age=34560000; HttpOnly; SameSite=Strict; Path=/",
+                "ab".repeat(32)
+            )
+        );
+        assert!(set_cookie("x", true)
+            .to_str()
+            .unwrap()
+            .ends_with("; Path=/; Secure"));
+        let clear = clear_cookie(true);
+        assert_eq!(
+            clear.to_str().unwrap(),
+            "claudio_web=; Max-Age=0; HttpOnly; SameSite=Strict; Path=/; Secure"
+        );
+    }
+
     #[test]
     fn cookie_value_finds_ours_among_others() {
         let mut h = HeaderMap::new();
diff --git a/addons/web/src/login.rs b/addons/web/src/login.rs
index 823c0d3..99b2186 100644
--- a/addons/web/src/login.rs
+++ b/addons/web/src/login.rs
@@ -314,6 +314,53 @@ mod tests {
         assert_eq!(token_login(&a, &LOCAL, &"ab".repeat(32), t0), Ok(()));
     }
 
+    /// A login's cookie is in the sessions file (as its hash) before the
+    /// reply carries it, and a sign-out is out of it before its reply.
+    #[tokio::test]
+    async fn a_login_and_a_sign_out_are_written_before_their_replies() {
+        use crate::sessions::{self, Mode, Store};
+        use sha2::{Digest, Sha256};
+        let dir = paths::socket_test_dir("web-login");
+        let file = dir.join("web-sessions.json");
+        let token = "ab".repeat(32);
+        let epoch = sessions::epoch(Mode::Local, &token, b"");
+        let lo = std::net::SocketAddr::from(([127, 0, 0, 1], 7337));
+        let kept = Sessions::new(Store::default(), epoch, file.clone());
+        let a = Arc::new(Auth::new(token.clone(), Allowed::new(lo, &[]), csp(None), kept).unwrap());
+        let writer = a.sessions.start_writer().unwrap();
+        let body = serde_json::json!({ "token": token }).to_string();
+        let req = Request::builder()
+            .method("POST")
+            .uri("/api/login")
+            .body(axum::body::Body::from(body))
+            .unwrap();
+        let res = login(State(Arc::clone(&a)), req).await;
+        assert_eq!(res.status(), StatusCode::NO_CONTENT);
+        let set = res.headers()[header::SET_COOKIE]
+            .to_str()
+            .unwrap()
+            .to_owned();
+        let value = set["claudio_web=".len()..]
+            .split(';')
+            .next()
+            .unwrap()
+            .to_owned();
+        let hash: String = Sha256::digest(value.as_bytes())
+            .iter()
+            .map(|b| format!("{b:02x}"))
+            .collect();
+        let text = std::fs::read_to_string(&file).unwrap();
+        assert!(text.contains(&hash) && !text.contains(&value), "{text}");
+        let mut headers = axum::http::HeaderMap::new();
+        let cookie = format!("claudio_web={value}");
+        headers.insert(header::COOKIE, cookie.parse().unwrap());
+        let res = auth::logout(State(Arc::clone(&a)), headers).await;
+        assert_eq!(res.status(), StatusCode::NO_CONTENT);
+        assert!(!std::fs::read_to_string(&file).unwrap().contains(&hash));
+        writer.abort();
+        let _ = std::fs::remove_dir_all(&dir);
+    }
+
     #[test]
     fn successes_do_not_count() {
         let a = plain();
diff --git a/addons/web/src/sessions.rs b/addons/web/src/sessions.rs
index 1cdc917..70eb702 100644
--- a/addons/web/src/sessions.rs
+++ b/addons/web/src/sessions.rs
@@ -142,6 +142,7 @@ impl Sessions {
 #[cfg(test)]
 mod tests {
     use super::*;
+    use std::os::unix::fs::PermissionsExt;
 
     const DAY: u64 = 24 * 60 * 60;
     const T0: u64 = 1_800_000_000;
@@ -159,7 +160,7 @@ mod tests {
 
     /// Whether `v` names a live session at `now` (a use).
     fn used(s: &mut Store, v: &str, now: u64) -> bool {
-        s.use_value(v, now)
+        hash_of(v).is_some_and(|h| s.use_hash(&h, now).is_some())
     }
 
     #[test]
@@ -236,4 +237,216 @@ mod tests {
         s.remove("not a cookie");
         assert!(!used(&mut s, &a, T0) && used(&mut s, &b, T0));
     }
+    // ── The file, the epoch, the writer ────────────────────────────────────
+
+    /// A scratch dir, removed on drop.
+    struct Dir(PathBuf);
+
+    impl Dir {
+        fn new() -> Dir {
+            let d = paths::socket_test_dir("web-sessions");
+            std::fs::create_dir_all(&d).unwrap();
+            Dir(d)
+        }
+        fn file(&self) -> PathBuf {
+            self.0.join("web-sessions.json")
+        }
+    }
+
+    impl Drop for Dir {
+        fn drop(&mut self) {
+            let _ = std::fs::remove_dir_all(&self.0);
+        }
+    }
+
+    fn token() -> String {
+        "ab".repeat(32)
+    }
+
+    fn ep() -> String {
+        epoch(Mode::Local, &token(), b"")
+    }
+
+    #[test]
+    fn a_store_round_trips_through_a_private_file_of_hashes() {
+        let dir = Dir::new();
+        let mut s = Store::default();
+        let (a, b) = (login(&mut s, 0xabc, T0), login(&mut s, 0xdef, T0 + 5));
+        paths::write_atomic(&dir.file(), &s.to_file(&ep())).unwrap();
+        let text = std::fs::read_to_string(dir.file()).unwrap();
+        assert!(!text.contains(&a) && !text.contains(&b), "no cookie value");
+        assert!(!text.contains(&token()), "no token");
+        assert!(text.contains(&hex(&hash_of(&a).unwrap())));
+        let mode = std::fs::metadata(dir.file()).unwrap().permissions().mode() & 0o777;
+        assert_eq!(mode, 0o600);
+        let (mut back, why) = load(&dir.file(), &ep(), T0 + 10);
+        assert_eq!(why, None);
+        assert_eq!(back.entries.len(), 2);
+        assert!(used(&mut back, &a, T0 + 10) && used(&mut back, &b, T0 + 10));
+    }
+
+    #[test]
+    fn the_epoch_binds_the_mode_the_token_and_the_password() {
+        let base = epoch(Mode::Local, &token(), b"pw");
+        assert!(base.len() == 64 && token_ok(&base), "{base}");
+        let others = [
+            epoch(Mode::Local, &"cd".repeat(32), b"pw"),
+            epoch(Mode::Local, &token(), b"pw2"),
+            epoch(Mode::Remote, &token(), b"pw"),
+        ];
+        let dir = Dir::new();
+        let mut s = Store::default();
+        login(&mut s, 1, T0);
+        paths::write_atomic(&dir.file(), &s.to_file(&base)).unwrap();
+        for other in others {
+            assert_ne!(other, base);
+            assert_eq!(load(&dir.file(), &other, T0), (Store::default(), None));
+        }
+        assert_eq!(load(&dir.file(), &base, T0).0.entries.len(), 1);
+        // The separators keep the parts apart.
+        assert_ne!(
+            epoch(Mode::Local, &token(), b""),
+            epoch(Mode::Local, &token(), b"\0")
+        );
+    }
+
+    #[test]
+    fn a_missing_file_is_an_empty_store_and_a_bad_one_says_why() {
+        let dir = Dir::new();
+        assert_eq!(load(&dir.file(), &ep(), T0), (Store::default(), None));
+        let h = hex(&hash_of(&value(1)).unwrap());
+        let with = |sessions: &str| {
+            format!(
+                r#"{{"version":1,"epoch":"{}","sessions":[{sessions}]}}"#,
+                ep()
+            )
+        };
+        let entry = |hash: &str| format!(r#"{{"hash":"{hash}","seen":{T0}}}"#);
+        let cases = [
+            (b"{not json".to_vec(), "not a sessions file"),
+            (
+                format!(r#"{{"version":2,"epoch":"{}","sessions":[]}}"#, ep()).into_bytes(),
+                "version 2 is not 1",
+            ),
+            (with(&entry("xyz")).into_bytes(), "a malformed hash"),
+            (
+                with(&entry(&h.to_uppercase())).into_bytes(),
+                "a malformed hash",
+            ),
+            (
+                with(&format!("{},{}", entry(&h), entry(&h))).into_bytes(),
+                "a hash twice",
+            ),
+            (vec![b' '; 64 * 1024 + 1], "over 64 KiB"),
+        ];
+        for (bytes, why) in cases {
+            paths::write_atomic(&dir.file(), &bytes).unwrap();
+            let (store, got) = load(&dir.file(), &ep(), T0);
+            assert_eq!((store, got.as_deref()), (Store::default(), Some(why)));
+        }
+        paths::write_atomic(&dir.file(), with(&entry(&h)).as_bytes()).unwrap();
+        assert_eq!(load(&dir.file(), &ep(), T0).0.entries.len(), 1);
+        let link = dir.0.join("link.json");
+        std::os::unix::fs::symlink(dir.file(), &link).unwrap();
+        assert_eq!(load(&link, &ep(), T0).1.as_deref(), Some("a symlink"));
+        let open = std::fs::Permissions::from_mode(0o644);
+        std::fs::set_permissions(dir.file(), open).unwrap();
+        let why = load(&dir.file(), &ep(), T0).1;
+        assert_eq!(
+            why.as_deref(),
+            Some("not a private (0600) regular file of yours")
+        );
+    }
+
+    #[test]
+    fn a_file_of_70_keeps_the_64_most_recently_seen() {
+        let dir = Dir::new();
+        let mut s = Store::default();
+        let values: Vec<String> = (0..70).map(|n| login(&mut s, n, T0 + n)).collect();
+        // Past the cap of a running server: written as a file of 70.
+        assert_eq!(s.entries.len(), MAX);
+        s.entries.extend((0..6).map(|n| Entry {
+            hash: hash_of(&values[n]).unwrap(),
+            seen: T0 + n as u64,
+            written: 0,
+        }));
+        paths::write_atomic(&dir.file(), &s.to_file(&ep())).unwrap();
+        let (mut back, _) = load(&dir.file(), &ep(), T0 + 100);
+        assert_eq!(back.entries.len(), MAX);
+        assert!(!used(&mut back, &values[5], T0 + 100));
+        assert!(used(&mut back, &values[6], T0 + 100));
+    }
+
+    #[test]
+    fn steady_use_is_due_for_the_file_once_an_hour() {
+        let mut s = Store::default();
+        let v = login(&mut s, 1, T0);
+        let hash = hash_of(&v).unwrap();
+        // The login's write.
+        let snapshot = s.clone();
+        s.written(&snapshot);
+        let mut due = Vec::new();
+        for minute in 1..=180 {
+            let now = T0 + 60 * minute;
+            if s.use_hash(&hash, now) == Some(true) {
+                due.push(now - T0);
+                let snapshot = s.clone();
+                s.written(&snapshot);
+            }
+        }
+        assert_eq!(due, [3660, 7320]);
+    }
+
+    #[tokio::test]
+    async fn a_login_is_written_before_it_returns_and_with_the_start_s_epoch() {
+        let dir = Dir::new();
+        let s = Arc::new(Sessions::new(Store::default(), ep(), dir.file()));
+        let writer = s.start_writer().unwrap();
+        let v = value(1);
+        let change = s.login(hash_of(&v).unwrap(), None, T0);
+        // The token file changes on disk (a rotation); the epoch does not.
+        std::fs::write(dir.0.join("web-token"), "cd".repeat(32)).unwrap();
+        s.written(change).await;
+        let text = std::fs::read_to_string(dir.file()).unwrap();
+        assert!(text.contains(&hex(&hash_of(&v).unwrap())));
+        assert!(text.contains(&ep()), "{text}");
+        let change = s.sign_out(&v);
+        s.written(change).await;
+        let text = std::fs::read_to_string(dir.file()).unwrap();
+        assert!(!text.contains(&hex(&hash_of(&v).unwrap())));
+        writer.abort();
+    }
+
+    #[tokio::test]
+    async fn a_failed_write_keeps_the_session_and_is_tried_again() {
+        if paths::uid() == 0 {
+            return; // root writes into a 0500 directory
+        }
+        let dir = Dir::new();
+        let locked = dir.0.join("locked");
+        std::fs::create_dir(&locked).unwrap();
+        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();
+        let file = locked.join("web-sessions.json");
+        let s = Arc::new(Sessions::new(Store::default(), ep(), file.clone()));
+        let writer = s.start_writer().unwrap();
+        let first = value(1);
+        let change = s.login(hash_of(&first).unwrap(), None, T0);
+        s.written(change).await;
+        assert!(!file.exists());
+        assert!(s.check(&first, T0 + 1), "kept in memory");
+        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
+        let second = value(2);
+        let change = s.login(hash_of(&second).unwrap(), None, T0 + 2);
+        s.written(change).await;
+        let text = std::fs::read_to_string(&file).unwrap();
+        for v in [&first, &second] {
+            assert!(text.contains(&hex(&hash_of(v).unwrap())), "{text}");
+        }
+        writer.abort();
+        let minutes = |m: u64| Duration::from_secs(60 * m);
+        assert_eq!(next_retry(None), minutes(1));
+        assert_eq!(next_retry(Some(minutes(1))), minutes(2));
+        assert_eq!(next_retry(Some(minutes(40))), minutes(60));
+        assert_eq!(next_retry(Some(minutes(60))), minutes(60));
+    }
 }
diff --git a/addons/web/tests/web.rs b/addons/web/tests/web.rs
index a38689d..a3780f3 100644
--- a/addons/web/tests/web.rs
+++ b/addons/web/tests/web.rs
@@ -289,6 +289,80 @@ fn web_answers_on_its_listen_address_and_a_proxy_port() {
     assert!(reply.header("set-cookie").unwrap().contains("; Secure"));
 }
 
+// ── Persistent sign-ins ───────────────────────────────────────────────────────
+
+fn cookie_header(cookie: &str) -> String {
+    format!("claudio_web={cookie}")
+}
+
+/// `/api/me` with `cookie`.
+fn me_with(web: &Web, cookie: &str) -> u16 {
+    web.get("/api/me", &[("Cookie", &cookie_header(cookie))])
+        .status
+}
+
+#[test]
+fn web_cookie_outlives_the_browser_and_logout_expires_it() {
+    let web = Web::start();
+    let body = json!({"token": web.token}).to_string();
+    let reply = web.post("/api/login", Some(&web.origin()), &body, &[]);
+    assert_eq!(reply.status, 204);
+    let set = reply.header("set-cookie").unwrap().to_owned();
+    let cookie = cookie_of(&set);
+    assert_eq!(
+        set,
+        format!("claudio_web={cookie}; Max-Age=34560000; HttpOnly; SameSite=Strict; Path=/")
+    );
+    let out = web.post(
+        "/api/logout",
+        Some(&web.origin()),
+        "",
+        &[("Cookie", &cookie_header(&cookie))],
+    );
+    assert_eq!(out.status, 204);
+    assert_eq!(
+        out.header("set-cookie"),
+        Some("claudio_web=; Max-Age=0; HttpOnly; SameSite=Strict; Path=/")
+    );
+}
+
+#[test]
+fn web_sign_ins_and_sign_outs_survive_a_killed_server() {
+    let web = Web::start();
+    let (kept, ended) = (web.login(), web.login());
+    let out = web.post(
+        "/api/logout",
+        Some(&web.origin()),
+        "",
+        &[("Cookie", &cookie_header(&ended))],
+    );
+    assert_eq!(out.status, 204);
+    // SIGKILL: only what was written before each reply is in the file.
+    let h = web.stop();
+    let web = Web::start_with(h, &[], |_| {});
+    assert_eq!(me_with(&web, &kept), 204, "kept across the restart");
+    assert_eq!(me_with(&web, &ended), 401, "a sign-out stays");
+    let h = web.stop();
+    let web = Web::start_with(h, &["--rotate-token"], |_| {});
+    assert_eq!(me_with(&web, &kept), 401, "a new token signs everyone out");
+}
+
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn web_a_loopback_sign_in_is_refused_by_a_remote_start() {
+    let h = ManagerHarness::new();
+    // The same password bytes in both starts: only the mode differs.
+    write_test_password(&h);
+    let web = Web::start_with(h, &[], |_| {});
+    let cookie = web.login();
+    assert_eq!(me_with(&web, &cookie), 204);
+    let h = web.stop();
+    let r = RemoteWeb::start(h, &[], |_| {});
+    let me = r
+        .req("GET", "/api/me", &[("Cookie", &cookie_header(&cookie))], "")
+        .await;
+    assert_eq!(me.status, 401);
+}
+
 // ── --set-password (AC3) ──────────────────────────────────────────────────────
 
 #[test]
`````

Run: `cargo test --locked -p claudio-web --lib sessions`, then
`cargo test --locked --test web -- --test-threads=1 web_cookie_outlives web_sign_ins_and web_a_loopback`
Expected: the unit tests do not compile (`` error[E0425]: cannot find function `epoch` in module `sessions` ``,
`` error[E0425]: cannot find function `load` in this scope ``, …); the web suite reports
`web_cookie_outlives_the_browser_and_logout_expires_it ... FAILED` (`left: "claudio_web=…; HttpOnly; SameSite=Strict; Path=/"`,
`right: "claudio_web=…; Max-Age=34560000; HttpOnly; SameSite=Strict; Path=/"`) and
`web_sign_ins_and_sign_outs_survive_a_killed_server ... FAILED` (`kept across the restart`,
`left: 401`, `right: 204`). `web_a_loopback_sign_in_is_refused_by_a_remote_start` passes before
and after: it guards the epoch's mode once sessions persist.

Step 2: implement

Patch `task5-step2` (`Cargo.lock`, `addons/web/Cargo.toml`, `addons/web/src/auth.rs`, `addons/web/src/lib.rs`, `addons/web/src/login.rs`, `addons/web/src/password.rs`, `addons/web/src/remote.rs`, `addons/web/src/sessions.rs`):

`````diff
diff --git a/Cargo.lock b/Cargo.lock
index daa0f46..4d034c3 100644
--- a/Cargo.lock
+++ b/Cargo.lock
@@ -454,6 +454,7 @@ dependencies = [
  "claudio-core",
  "flate2",
  "futures",
+ "libc",
  "rcgen",
  "reqwest",
  "rustls",
diff --git a/addons/web/Cargo.toml b/addons/web/Cargo.toml
index bd68b31..a60067e 100644
--- a/addons/web/Cargo.toml
+++ b/addons/web/Cargo.toml
@@ -14,6 +14,8 @@ axum.workspace = true
 base64.workspace = true
 flate2.workspace = true
 futures.workspace = true
+# O_NOFOLLOW for the sessions file (sessions.rs), as claude/hooks.rs opens its context file.
+libc.workspace = true
 rcgen.workspace = true
 rustls.workspace = true
 rustls-webpki.workspace = true
diff --git a/addons/web/src/auth.rs b/addons/web/src/auth.rs
index 4eae9a2..69d7879 100644
--- a/addons/web/src/auth.rs
+++ b/addons/web/src/auth.rs
@@ -32,6 +32,9 @@ use crate::peer::{Peer, PeerKey};
 use crate::sessions::{self, Sessions};
 
 pub(crate) const COOKIE: &str = "claudio_web";
+/// The cookie's life (400 days, the longest browsers keep): closing the
+/// browser does not drop it; the server decides when a session ends.
+pub(crate) const COOKIE_MAX_AGE: u64 = 34_560_000;
 pub(crate) const TOKEN_BYTES: usize = 32;
 pub(crate) const LOGIN_BODY_MAX: usize = 4096;
 /// Concurrent argon2 verifications (each 19 MiB).
@@ -414,13 +417,14 @@ impl Auth {
         ct_eq(token.as_bytes(), self.token.as_bytes())
     }
 
-    /// A new session's cookie value; the session of `old` (the cookie the
-    /// login came with) goes in the same step. `now`: Unix seconds.
-    pub(crate) fn create_session(&self, old: Option<&str>, now: u64) -> io::Result<String> {
+    /// A new session's cookie value, and the change its reply waits for
+    /// (`Sessions::written`); the session of `old` (the cookie the login
+    /// came with) goes in the same step. `now`: Unix seconds.
+    pub(crate) fn create_session(&self, old: Option<&str>, now: u64) -> io::Result<(String, u64)> {
         let value = self.random_hex(TOKEN_BYTES)?;
         let hash = sessions::hash_of(&value).ok_or_else(|| io::Error::other("not a token"))?;
-        self.sessions.login(hash, old, now);
-        Ok(value)
+        let change = self.sessions.login(hash, old, now);
+        Ok((value, change))
     }
 
     /// Whether `value` names a live session (a use).
@@ -428,9 +432,9 @@ impl Auth {
         self.sessions.check(value, now)
     }
 
-    /// Sign `value`'s session out.
-    pub(crate) fn end_session(&self, value: &str) {
-        self.sessions.sign_out(value);
+    /// Sign `value`'s session out; the change its reply waits for.
+    pub(crate) fn end_session(&self, value: &str) -> u64 {
+        self.sessions.sign_out(value)
     }
 }
 
@@ -449,7 +453,7 @@ pub(crate) fn cookie_value(headers: &HeaderMap) -> Option<String> {
 pub(crate) fn set_cookie(value: &str, secure: bool) -> HeaderValue {
     let secure = if secure { "; Secure" } else { "" };
     HeaderValue::from_str(&format!(
-        "{COOKIE}={value}; HttpOnly; SameSite=Strict; Path=/{secure}"
+        "{COOKIE}={value}; Max-Age={COOKIE_MAX_AGE}; HttpOnly; SameSite=Strict; Path=/{secure}"
     ))
     .expect("an ASCII cookie")
 }
@@ -533,10 +537,12 @@ pub(crate) async fn me() -> StatusCode {
     StatusCode::NO_CONTENT
 }
 
-/// `POST /api/logout`: forget the session (the guard required it).
+/// `POST /api/logout`: forget the session (the guard required it); the
+/// reply waits until the sessions file no longer has it (≤ 2 s).
 pub(crate) async fn logout(State(auth): State<Arc<Auth>>, headers: HeaderMap) -> Response {
     if let Some(value) = cookie_value(&headers) {
-        auth.end_session(&value);
+        let change = auth.end_session(&value);
+        auth.sessions.written(change).await;
     }
     let mut res = StatusCode::NO_CONTENT.into_response();
     let cookie = clear_cookie(auth.allowed.secure_cookie());
diff --git a/addons/web/src/lib.rs b/addons/web/src/lib.rs
index fb0b3e4..6449624 100644
--- a/addons/web/src/lib.rs
+++ b/addons/web/src/lib.rs
@@ -273,7 +273,11 @@ pub fn run(args: &[String], addons: Arc<claudio_core::addon::Addons>) -> ExitCod
         println!("{url}");
         return ExitCode::SUCCESS;
     }
-    let remote = match args.remote.map(|addr| remote::prepare(&args, addr, &token)) {
+    let password = password::read(&password::path());
+    let remote = match args
+        .remote
+        .map(|addr| remote::prepare(&args, addr, &token, &password))
+    {
         None => None,
         Some(Ok(r)) => Some(r),
         Some(Err(e)) => {
@@ -281,6 +285,7 @@ pub fn run(args: &[String], addons: Arc<claudio_core::addon::Addons>) -> ExitCod
             return ExitCode::FAILURE;
         }
     };
+    let sessions = load_sessions(&args, &token, &password);
 
     // A fixed filter, not RUST_LOG: a dependency's trace output could carry
     // request headers (the cookie).
@@ -304,7 +309,7 @@ pub fn run(args: &[String], addons: Arc<claudio_core::addon::Addons>) -> ExitCod
             return ExitCode::FAILURE;
         }
     };
-    match runtime.block_on(serve(args, token, url, remote, addons)) {
+    match runtime.block_on(serve(args, token, url, remote, sessions, addons)) {
         Ok(()) => ExitCode::SUCCESS,
         Err(e) => {
             eprintln!("claudio web: {e}");
@@ -313,6 +318,31 @@ pub fn run(args: &[String], addons: Arc<claudio_core::addon::Addons>) -> ExitCod
     }
 }
 
+/// The sign-ins of earlier runs, bound to this start's mode, token and
+/// password bytes (a file of another epoch: none, silently). Blocking I/O,
+/// before the runtime exists and before `serve` binds.
+fn load_sessions(
+    args: &WebArgs,
+    token: &str,
+    password: &Result<Vec<u8>, String>,
+) -> sessions::Sessions {
+    let mode = match args.remote {
+        Some(_) => sessions::Mode::Remote,
+        None => sessions::Mode::Local,
+    };
+    let epoch = sessions::epoch(mode, token, password.as_deref().unwrap_or_default());
+    let path = sessions::path();
+    let (store, why) = sessions::load(&path, &epoch, claudio_core::paths::unix_now());
+    if let Some(why) = why {
+        // The tracing subscriber is not installed yet.
+        eprintln!(
+            "claudio web: ignoring {}: {why}; browsers sign in again",
+            path.display()
+        );
+    }
+    sessions::Sessions::new(store, epoch, path)
+}
+
 /// `--set-password` or `--print-cert`: one thing, then exit.
 fn one_shot(args: &WebArgs) -> Option<ExitCode> {
     if args.set_password {
@@ -349,6 +379,7 @@ async fn serve(
     token: String,
     url: String,
     remote: Option<remote::Remote>,
+    sessions: sessions::Sessions,
     addons: Arc<claudio_core::addon::Addons>,
 ) -> Result<(), String> {
     let listen = remote.as_ref().map_or(args.listen, |r| r.addr);
@@ -361,17 +392,19 @@ async fn serve(
         None => auth::Allowed::new(args.listen, &args.allow_hosts),
     };
     let csp = auth::csp(assets.importmap_sha256());
-    let mut auth = auth::Auth::new(token, allowed, csp, sessions::Sessions::memory())
+    let mut auth = auth::Auth::new(token, allowed, csp, sessions)
         .map_err(|e| format!("cannot open /dev/urandom: {e}"))?;
     if let Some(r) = &remote {
         auth = auth.with_password(r.password.clone());
     }
     let state = build_state(auth, assets, addons).await?;
-    let (hub, ext, addon_rt) = (
+    let (hub, ext, addon_rt, sessions) = (
         state.hub.clone(),
         Arc::clone(&state.ctx.ext),
         state.addons.clone(),
+        Arc::clone(&state.auth.sessions),
     );
+    let writer = sessions.start_writer();
     if !env!("CLAUDIO_BUILD_LABEL").is_empty() {
         eprintln!("claudio web: local build {}", version());
     }
@@ -386,11 +419,13 @@ async fn serve(
     }
     tracing::info!(listen = %listen, tls = remote.is_some(), "web: serving");
 
-    // Tabs are closed (1001) and web-state.json written before the server
-    // stops waiting for its connections.
+    // Tabs are closed (1001), web-state.json and the sessions written
+    // before the server stops waiting for its connections.
     let stop = async move {
         shutdown().await;
         hub.shutdown().await;
+        sessions.flush().await;
+        writer.inspect(|w| w.abort());
         ext.shutdown().await;
         addon_rt.shutdown();
     };
diff --git a/addons/web/src/login.rs b/addons/web/src/login.rs
index 99b2186..6b353f2 100644
--- a/addons/web/src/login.rs
+++ b/addons/web/src/login.rs
@@ -54,7 +54,9 @@ pub(crate) async fn login(State(auth): State<Arc<Auth>>, req: Request) -> Respon
         return status.into_response();
     }
     match auth.create_session(old.as_deref(), paths::unix_now()) {
-        Ok(value) => {
+        Ok((value, change)) => {
+            // The cookie goes out once the sessions file has it (≤ 2 s).
+            auth.sessions.written(change).await;
             let mut res = StatusCode::NO_CONTENT.into_response();
             let cookie = auth::set_cookie(&value, auth.allowed.secure_cookie());
             res.headers_mut().insert(header::SET_COOKIE, cookie);
diff --git a/addons/web/src/password.rs b/addons/web/src/password.rs
index 06cf7f1..7afcd6c 100644
--- a/addons/web/src/password.rs
+++ b/addons/web/src/password.rs
@@ -97,6 +97,14 @@ impl Stored {
 /// Read the stored hash: a regular file of ours, not readable by others,
 /// at most 512 bytes, one valid PHC string. Every failure says how to fix it.
 pub(crate) fn load(path: &Path) -> Result<Stored, String> {
+    parse_file(path, &read(path)?)
+}
+
+/// The password file's bytes: a regular file of ours, not readable by
+/// others, at most 512 bytes. Every failure says how to fix it. One read
+/// feeds both the password a `--remote` server checks and its sessions'
+/// epoch (`sessions::epoch`). Blocking I/O.
+pub(crate) fn read(path: &Path) -> Result<Vec<u8>, String> {
     let fix = "set one with `claudio web --set-password`";
     let meta = std::fs::symlink_metadata(path).map_err(|e| match e.kind() {
         io::ErrorKind::NotFound => format!("--remote needs a password: {fix}"),
@@ -110,9 +118,15 @@ pub(crate) fn load(path: &Path) -> Result<Stored, String> {
             path.display()
         ));
     }
-    let text = std::fs::read_to_string(path)
-        .map_err(|e| format!("cannot read {}: {e}; {fix}", path.display()))?;
-    Stored::parse(&text).map_err(|e| format!("{}: {e}; {fix}", path.display()))
+    std::fs::read(path).map_err(|e| format!("cannot read {}: {e}; {fix}", path.display()))
+}
+
+/// The stored hash in `bytes`, as [`read`] read them from `path`.
+pub(crate) fn parse_file(path: &Path, bytes: &[u8]) -> Result<Stored, String> {
+    let fix = "set one with `claudio web --set-password`";
+    let text =
+        std::str::from_utf8(bytes).map_err(|_| format!("{}: not text; {fix}", path.display()))?;
+    Stored::parse(text).map_err(|e| format!("{}: {e}; {fix}", path.display()))
 }
 
 /// `claudio web --set-password`: the password twice from the terminal
diff --git a/addons/web/src/remote.rs b/addons/web/src/remote.rs
index 33f42a6..3b89cfd 100644
--- a/addons/web/src/remote.rs
+++ b/addons/web/src/remote.rs
@@ -185,14 +185,17 @@ pub(crate) struct Remote {
     pub banner: Banner,
 }
 
-/// Check the password file first (nothing is made or bound without one),
-/// then load or make the certificate. Blocking I/O.
+/// Check the password file first (`password`, as `password::read` read it:
+/// nothing is made or bound without one), then load or make the
+/// certificate. Blocking I/O.
 pub(crate) fn prepare(
     args: &crate::WebArgs,
     addr: SocketAddr,
     token: &str,
+    password: &Result<Vec<u8>, String>,
 ) -> Result<Remote, String> {
-    let password = crate::password::load(&crate::password::path())?;
+    let bytes = password.as_deref().map_err(Clone::clone)?;
+    let password = crate::password::parse_file(&crate::password::path(), bytes)?;
     let hostname = claudio_core::sys::hostname();
     let names = cert_names(&hostname, addr, &args.allow_hosts);
     let cert =
diff --git a/addons/web/src/sessions.rs b/addons/web/src/sessions.rs
index 70eb702..24abcd0 100644
--- a/addons/web/src/sessions.rs
+++ b/addons/web/src/sessions.rs
@@ -1,14 +1,25 @@
 //! Sign-ins (docs/web.md, "Cookie"; docs/web-protocol.md §1): the
-//! server-side sessions a `claudio_web` cookie names.
+//! server-side sessions a `claudio_web` cookie names, kept across restarts
+//! in `<config>/web-sessions.json` (0600).
 //!
 //! [`Store`] is pure: each entry is the SHA-256 of a cookie value and its
 //! last use in wall-clock Unix seconds; a session unused for 30 days is
 //! gone, and past 64 the least recently used goes. `now` is a parameter
-//! (A6). [`Sessions`] holds the store the server uses.
+//! (A6). The file format, [`epoch`], [`load`] and the writer task of
+//! [`Sessions`] are the I/O edge. The file holds hashes only: a copy of it
+//! signs no one in.
 
-use std::sync::{Mutex, MutexGuard};
+use std::io::{self, Read};
+use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
+use std::path::{Path, PathBuf};
+use std::sync::{Arc, Mutex, MutexGuard};
+use std::time::Duration;
 
+use serde::{Deserialize, Serialize};
 use sha2::{Digest, Sha256};
+use tokio::sync::watch;
+
+use claudio_core::paths;
 
 use crate::auth::{ct_eq, token_ok};
 
@@ -16,21 +27,72 @@ use crate::auth::{ct_eq, token_ok};
 pub(crate) const IDLE: u64 = 30 * 24 * 60 * 60;
 /// Sessions kept at most; past it the least recently used goes.
 pub(crate) const MAX: usize = 64;
+/// The file's `seen` of a session is rewritten once memory's is this much
+/// newer (seconds): uses do not each write the disk.
+pub(crate) const WRITE_EVERY: u64 = 60 * 60;
+/// How long a login or a sign-out waits for its write, and the last write
+/// at stop.
+pub(crate) const WRITE_WAIT: Duration = Duration::from_secs(2);
+/// A failed write is tried again after this, doubling up to [`RETRY_MAX`].
+const RETRY_FIRST: Duration = Duration::from_secs(60);
+const RETRY_MAX: Duration = Duration::from_secs(60 * 60);
+/// The largest sessions file read.
+const FILE_MAX: u64 = 64 * 1024;
+const VERSION: u64 = 1;
 
 /// The SHA-256 of a cookie value.
 pub(crate) type Hash = [u8; 32];
 
+/// `<config>/web-sessions.json`.
+pub(crate) fn path() -> PathBuf {
+    paths::config_dir().join("web-sessions.json")
+}
+
 /// The SHA-256 of `value` when it is one this server could have made (64
 /// lowercase hex); anything else is refused without hashing.
 pub(crate) fn hash_of(value: &str) -> Option<Hash> {
     token_ok(value).then(|| Sha256::digest(value.as_bytes()).into())
 }
 
+/// Which listener the server runs: the epoch's `mode`.
+#[derive(Debug, Clone, Copy, PartialEq, Eq)]
+pub(crate) enum Mode {
+    /// Loopback (`--listen`).
+    Local,
+    /// `--remote`.
+    Remote,
+}
+
+/// What the sessions file is bound to: the lowercase hex SHA-256 of
+/// `"claudio-web-sessions-v1" ‖ 0 ‖ mode ‖ 0 ‖ token ‖ 0 ‖ password`, where
+/// `password` is the bytes of `web-password` as read at this start (empty
+/// when it is missing or not acceptable). Computed once per start: a new
+/// token, password or mode signs every browser out at the next start.
+pub(crate) fn epoch(mode: Mode, token: &str, password: &[u8]) -> String {
+    let mode = match mode {
+        Mode::Local => "local",
+        Mode::Remote => "remote",
+    };
+    let mut h = Sha256::new();
+    h.update(b"claudio-web-sessions-v1");
+    for part in [mode.as_bytes(), token.as_bytes(), password] {
+        h.update([0u8]);
+        h.update(part);
+    }
+    hex(&h.finalize())
+}
+
+fn hex(bytes: &[u8]) -> String {
+    bytes.iter().map(|b| format!("{b:02x}")).collect()
+}
+
 #[derive(Debug, Clone, PartialEq, Eq)]
 struct Entry {
     hash: Hash,
     /// Last use, Unix seconds.
     seen: u64,
+    /// `seen` as the file holds it (0: not written yet).
+    written: u64,
 }
 
 /// The sessions, pure: every method takes `now`.
@@ -80,22 +142,19 @@ impl Store {
         self.entries.push(Entry {
             hash: new,
             seen: now,
+            written: 0,
         });
     }
 
-    /// Whether `value` names a live session; a use: its `seen` becomes `now`.
-    pub(crate) fn use_value(&mut self, value: &str, now: u64) -> bool {
-        let Some(hash) = hash_of(value) else {
-            return false;
-        };
+    /// A use of `hash`'s session if it is live (its `seen` becomes `now`):
+    /// `Some(true)` when the file's `seen` is then over [`WRITE_EVERY`]
+    /// older than memory's.
+    pub(crate) fn use_hash(&mut self, hash: &Hash, now: u64) -> Option<bool> {
         self.tidy(now);
-        match self.find(&hash) {
-            Some(i) => {
-                self.entries[i].seen = now;
-                true
-            }
-            None => false,
-        }
+        let i = self.find(hash)?;
+        let e = &mut self.entries[i];
+        e.seen = now;
+        Some(now - e.written.min(now) > WRITE_EVERY)
     }
 
     /// A sign-out: `value`'s session out.
@@ -104,18 +163,160 @@ impl Store {
             self.entries.remove(i);
         }
     }
+
+    /// The file's bytes for `epoch`.
+    fn to_file(&self, epoch: &str) -> Vec<u8> {
+        let sessions = self
+            .entries
+            .iter()
+            .map(|e| FileEntry {
+                hash: hex(&e.hash),
+                seen: e.seen,
+            })
+            .collect();
+        let file = FileV1 {
+            version: VERSION,
+            epoch: epoch.to_owned(),
+            sessions,
+        };
+        serde_json::to_vec(&file).unwrap_or_default()
+    }
+
+    /// `snapshot` was written: what the file now holds of each session.
+    fn written(&mut self, snapshot: &Store) {
+        for e in &mut self.entries {
+            if let Some(w) = snapshot.entries.iter().find(|w| w.hash == e.hash) {
+                e.written = w.seen;
+            }
+        }
+    }
+}
+
+/// `web-sessions.json`, version 1.
+#[derive(Serialize, Deserialize)]
+struct FileV1 {
+    version: u64,
+    epoch: String,
+    sessions: Vec<FileEntry>,
 }
 
-/// The store the server uses, shared by every request.
-#[derive(Default)]
+#[derive(Serialize, Deserialize)]
+struct FileEntry {
+    hash: String,
+    seen: u64,
+}
+
+/// The store the file at `path` holds for `epoch`, tidied at `now`, and why
+/// it was ignored if it was (one line for stderr). A missing file and one
+/// of another epoch give an empty store and no reason. Blocking I/O.
+pub(crate) fn load(path: &Path, epoch: &str, now: u64) -> (Store, Option<String>) {
+    match read(path, epoch) {
+        Ok(Some(mut store)) => {
+            store.tidy(now);
+            (store, None)
+        }
+        Ok(None) => (Store::default(), None),
+        Err(why) => (Store::default(), Some(why)),
+    }
+}
+
+/// `Ok(None)`: missing, or bound to another epoch. Opened without
+/// following a symlink and checked on the open file (as
+/// `claude/hooks.rs`'s `read_context`).
+fn read(path: &Path, epoch: &str) -> Result<Option<Store>, String> {
+    let file = match std::fs::OpenOptions::new()
+        .read(true)
+        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
+        .open(path)
+    {
+        Ok(f) => f,
+        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
+        Err(e) if e.raw_os_error() == Some(libc::ELOOP) => return Err("a symlink".to_owned()),
+        Err(e) => return Err(e.to_string()),
+    };
+    let meta = file.metadata().map_err(|e| e.to_string())?;
+    if !meta.file_type().is_file() || meta.uid() != paths::uid() || meta.mode() & 0o077 != 0 {
+        return Err("not a private (0600) regular file of yours".to_owned());
+    }
+    let mut bytes = Vec::new();
+    file.take(FILE_MAX + 1)
+        .read_to_end(&mut bytes)
+        .map_err(|e| e.to_string())?;
+    if bytes.len() as u64 > FILE_MAX {
+        return Err("over 64 KiB".to_owned());
+    }
+    let file: FileV1 =
+        serde_json::from_slice(&bytes).map_err(|_| "not a sessions file".to_owned())?;
+    if file.version != VERSION {
+        return Err(format!("version {} is not {VERSION}", file.version));
+    }
+    if file.epoch != epoch {
+        return Ok(None);
+    }
+    let mut store = Store::default();
+    for s in file.sessions {
+        let hash = parse_hash(&s.hash).ok_or("a malformed hash")?;
+        if store.find(&hash).is_some() {
+            return Err("a hash twice".to_owned());
+        }
+        store.entries.push(Entry {
+            hash,
+            seen: s.seen,
+            written: s.seen,
+        });
+    }
+    Ok(Some(store))
+}
+
+/// 64 lowercase hex as 32 bytes.
+fn parse_hash(s: &str) -> Option<Hash> {
+    if !token_ok(s) {
+        return None;
+    }
+    let mut out = [0u8; 32];
+    for (byte, pair) in out.iter_mut().zip(s.as_bytes().chunks(2)) {
+        *byte = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
+    }
+    Some(out)
+}
+
+/// The store the server uses, its epoch and file, and the writer that
+/// keeps the file current: one task that always writes the newest store
+/// (a `watch`, so nothing queues: A10).
 pub(crate) struct Sessions {
     store: Mutex<Store>,
+    /// Fixed at start: a write never reads the token or the password.
+    epoch: String,
+    /// `None`: in memory only.
+    path: Option<PathBuf>,
+    /// Bumped by every change the file must get; the writer writes the newest.
+    wanted: watch::Sender<u64>,
+    /// The last change the writer has tried to write (or failed to).
+    tried: watch::Sender<u64>,
 }
 
 impl Sessions {
-    /// An empty store.
+    /// `store` (from [`load`]), kept in `path` for `epoch`.
+    pub(crate) fn new(store: Store, epoch: String, path: PathBuf) -> Sessions {
+        Sessions {
+            store: Mutex::new(store),
+            epoch,
+            path: Some(path),
+            wanted: watch::Sender::new(0),
+            tried: watch::Sender::new(0),
+        }
+    }
+
+    /// An empty store, never written (unit tests).
+    #[cfg(test)]
     pub(crate) fn memory() -> Sessions {
-        Sessions::default()
+        Sessions {
+            store: Mutex::default(),
+            epoch: String::new(),
+            path: None,
+            wanted: watch::Sender::new(0),
+            tried: watch::Sender::new(0),
+        }
     }
 
     /// Never held across an await.
@@ -123,22 +324,113 @@ impl Sessions {
         self.store.lock().unwrap_or_else(|e| e.into_inner())
     }
 
-    /// [`Store::login`].
-    pub(crate) fn login(&self, new: Hash, old: Option<&str>, now: u64) {
+    /// Ask the writer for the newest store; the change's number.
+    fn change(&self) -> u64 {
+        let mut gen = 0;
+        self.wanted.send_modify(|w| {
+            *w += 1;
+            gen = *w;
+        });
+        gen
+    }
+
+    /// [`Store::login`]; the change a reply waits for ([`Sessions::written`]).
+    pub(crate) fn login(&self, new: Hash, old: Option<&str>, now: u64) -> u64 {
         self.lock().login(new, old, now);
+        self.change()
     }
 
-    /// [`Store::use_value`].
+    /// Whether `value` names a live session (a use); asks for a write when
+    /// the file's copy is over an hour old.
     pub(crate) fn check(&self, value: &str, now: u64) -> bool {
-        self.lock().use_value(value, now)
+        let Some(hash) = hash_of(value) else {
+            return false;
+        };
+        self.mark(&hash, now)
+    }
+
+    /// A use of `hash`'s session; `false` when it is gone (nothing happens).
+    pub(crate) fn mark(&self, hash: &Hash, now: u64) -> bool {
+        let used = self.lock().use_hash(hash, now);
+        if used == Some(true) {
+            self.change();
+        }
+        used.is_some()
     }
 
-    /// [`Store::remove`].
-    pub(crate) fn sign_out(&self, value: &str) {
+    /// [`Store::remove`]; the change a reply waits for.
+    pub(crate) fn sign_out(&self, value: &str) -> u64 {
         self.lock().remove(value);
+        self.change()
+    }
+
+    /// Until the writer has tried change `gen` (written or failed), at most
+    /// [`WRITE_WAIT`]; at once in memory.
+    pub(crate) async fn written(&self, gen: u64) {
+        if self.path.is_none() {
+            return;
+        }
+        let mut tried = self.tried.subscribe();
+        let _ = tokio::time::timeout(WRITE_WAIT, tried.wait_for(|t| *t >= gen)).await;
+    }
+
+    /// The last write, in `serve`'s stop sequence (≤ [`WRITE_WAIT`]).
+    pub(crate) async fn flush(&self) {
+        let gen = self.change();
+        self.written(gen).await;
+    }
+
+    /// The writer task (none in memory); aborted at stop. Changes from now
+    /// on are written.
+    pub(crate) fn start_writer(self: &Arc<Self>) -> Option<tokio::task::JoinHandle<()>> {
+        let path = self.path.clone()?;
+        let wanted = self.wanted.subscribe();
+        Some(tokio::spawn(writer(Arc::clone(self), path, wanted)))
     }
 }
 
+/// Write the newest store at each change; after a failure, again at the
+/// next change or after the retry delay (60 s, doubling to 1 h:
+/// `write_atomic` leaves a temp file behind when it fails).
+async fn writer(sessions: Arc<Sessions>, path: PathBuf, mut wanted: watch::Receiver<u64>) {
+    let mut retry: Option<Duration> = None;
+    loop {
+        match retry {
+            None => {
+                if wanted.changed().await.is_err() {
+                    return;
+                }
+            }
+            Some(delay) => {
+                let _ = tokio::time::timeout(delay, wanted.changed()).await;
+            }
+        }
+        let gen = *wanted.borrow_and_update();
+        let snapshot = sessions.lock().clone();
+        let bytes = snapshot.to_file(&sessions.epoch);
+        let p = path.clone();
+        let wrote = tokio::task::spawn_blocking(move || paths::write_atomic(&p, &bytes))
+            .await
+            .unwrap_or_else(|e| Err(io::Error::other(e)));
+        match wrote {
+            Ok(()) => {
+                sessions.lock().written(&snapshot);
+                retry = None;
+            }
+            Err(e) => {
+                tracing::warn!(file = %path.display(), error = %e, "web: cannot write the sessions file");
+                retry = Some(next_retry(retry));
+            }
+        }
+        sessions.tried.send_modify(|t| *t = (*t).max(gen));
+    }
+}
+
+/// The next retry delay after a failed write.
+fn next_retry(retry: Option<Duration>) -> Duration {
+    retry.map_or(RETRY_FIRST, |r| (r * 2).min(RETRY_MAX))
+}
+
 #[cfg(test)]
 mod tests {
     use super::*;
`````

Run: `cargo test --locked -p claudio-web --lib`, then
`cargo test --locked --test web -- --test-threads=1 web_cookie_outlives web_sign_ins_and web_a_loopback`,
then `cargo test --locked --test web --test vocdoni_web -- --test-threads=1`, then the `make check`
sequence.
Expected: `test result: ok. 107 passed; 0 failed`; `test result: ok. 3 passed; 0 failed`;
vocdoni_web `ok. 13 passed` and web `ok. 38 passed`; `rules: 0 problem(s)`.

Step 3: commit
`git commit -m "feat(web): sign-ins persist in web-sessions.json, bound to an epoch" -m "libc (already a workspace dependency) opens the sessions file with O_NOFOLLOW, as claude/hooks.rs opens its context file." -- addons/web/src/sessions.rs addons/web/src/auth.rs addons/web/src/login.rs addons/web/src/lib.rs addons/web/src/password.rs addons/web/src/remote.rs addons/web/Cargo.toml Cargo.lock addons/web/tests/web.rs`

Interfaces this task provides to later tasks: `sessions::{path() -> PathBuf, Mode::{Local,
Remote}, epoch(Mode, token: &str, password: &[u8]) -> String, load(&Path, epoch: &str, now:
u64) -> (Store, Option<String>), WRITE_EVERY, WRITE_WAIT}`; `Store::use_hash(&mut self, &Hash,
now) -> Option<bool>`; `Sessions::new(Store, epoch: String, path: PathBuf)`,
`Sessions::{check(&self, &str, now) -> bool, mark(&self, &Hash, now) -> bool, login(..) -> u64,
sign_out(&self, &str) -> u64, written(&self, u64), flush(&self), start_writer(self: &Arc<Self>)
-> Option<JoinHandle<()>>}`; `password::{read(&Path) -> Result<Vec<u8>, String>,
parse_file(&Path, &[u8]) -> Result<Stored, String>}`; `remote::prepare(args, addr, token,
password: &Result<Vec<u8>, String>)`; `lib.rs::load_sessions`; `auth::COOKIE_MAX_AGE`.

### Task 6: An open tab keeps its sign-in alive
Model: sonnet   Repo: /home/p4u/claudio   Files: `addons/web/src/sessions.rs`, `addons/web/src/ws.rs`
Goal: spec §3 "Use": an open WebSocket marks its session used once an hour (AC12: with an
injected clock, a session whose only activity is an open tab is live after 30 days).

`ws::upgrade` takes the request's `HeaderMap`, hashes the cookie the guard already checked,
and runs `tab(...)` inside a closure that holds a `sessions::OpenTab`: a task that marks the
session every hour (`MARK_EVERY`) at `paths::unix_now()`, ends when the session is gone, and is
aborted when the tab ends. `ws.rs::tab` is unchanged (it is at its A8 baseline; Spec issue 3).

Step 1: write the failing test (paused tokio time; the test's own clock)

Patch `task6-step1` (`addons/web/src/sessions.rs`):

`````diff
diff --git a/addons/web/src/sessions.rs b/addons/web/src/sessions.rs
index 24abcd0..d4f4756 100644
--- a/addons/web/src/sessions.rs
+++ b/addons/web/src/sessions.rs
@@ -709,6 +709,33 @@ mod tests {
         writer.abort();
     }
 
+    /// The marker's clock in the test below (its only user).
+    static CLOCK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
+
+    fn clock() -> u64 {
+        CLOCK.load(std::sync::atomic::Ordering::SeqCst)
+    }
+
+    #[tokio::test(start_paused = true)]
+    async fn an_open_tab_keeps_its_session_alive() {
+        use std::sync::atomic::Ordering::SeqCst;
+        CLOCK.store(T0, SeqCst);
+        let s = Arc::new(Sessions::memory());
+        let (open, idle) = (value(1), value(2));
+        s.login(hash_of(&open).unwrap(), None, T0);
+        s.login(hash_of(&idle).unwrap(), None, T0);
+        let tab = OpenTab::start(Arc::clone(&s), hash_of(&open).unwrap(), clock);
+        for _ in 0..31 * 24 {
+            CLOCK.fetch_add(60 * 60, SeqCst);
+            tokio::time::advance(MARK_EVERY).await;
+            tokio::task::yield_now().await;
+        }
+        assert_eq!(clock(), T0 + 31 * DAY);
+        assert!(s.check(&open, clock()), "an open tab's session is live");
+        assert!(!s.check(&idle, clock()), "the other idled out");
+        drop(tab);
+    }
+
     #[tokio::test]
     async fn a_failed_write_keeps_the_session_and_is_tried_again() {
         if paths::uid() == 0 {
`````

Run: `cargo test --locked -p claudio-web --lib sessions`
Expected: `` error[E0425]: cannot find value `MARK_EVERY` in this scope `` and
`` error[E0433]: cannot find type `OpenTab` in this scope ``.

Step 2: implement

Patch `task6-step2` (`addons/web/src/sessions.rs`, `addons/web/src/ws.rs`):

`````diff
diff --git a/addons/web/src/sessions.rs b/addons/web/src/sessions.rs
index d4f4756..cf5fc11 100644
--- a/addons/web/src/sessions.rs
+++ b/addons/web/src/sessions.rs
@@ -33,6 +33,8 @@ pub(crate) const WRITE_EVERY: u64 = 60 * 60;
 /// How long a login or a sign-out waits for its write, and the last write
 /// at stop.
 pub(crate) const WRITE_WAIT: Duration = Duration::from_secs(2);
+/// An open tab marks its session used this often ([`OpenTab`]).
+pub(crate) const MARK_EVERY: Duration = Duration::from_secs(60 * 60);
 /// A failed write is tried again after this, doubling up to [`RETRY_MAX`].
 const RETRY_FIRST: Duration = Duration::from_secs(60);
 const RETRY_MAX: Duration = Duration::from_secs(60 * 60);
@@ -426,6 +428,34 @@ async fn writer(sessions: Arc<Sessions>, path: PathBuf, mut wanted: watch::Recei
     }
 }
 
+/// While a tab's WebSocket is open its session counts as used: the page
+/// talks over the socket only after it loads. Marks it every
+/// [`MARK_EVERY`]; stops when the session is gone or the tab drops this.
+pub(crate) struct OpenTab(tokio::task::JoinHandle<()>);
+
+impl OpenTab {
+    /// Mark `hash`'s session used every [`MARK_EVERY`], at `clock()` (Unix
+    /// seconds).
+    pub(crate) fn start(sessions: Arc<Sessions>, hash: Hash, clock: fn() -> u64) -> OpenTab {
+        OpenTab(tokio::spawn(async move {
+            let first = tokio::time::Instant::now() + MARK_EVERY;
+            let mut every = tokio::time::interval_at(first, MARK_EVERY);
+            loop {
+                every.tick().await;
+                if !sessions.mark(&hash, clock()) {
+                    return;
+                }
+            }
+        }))
+    }
+}
+
+impl Drop for OpenTab {
+    fn drop(&mut self) {
+        self.0.abort();
+    }
+}
+
 /// The next retry delay after a failed write.
 fn next_retry(retry: Option<Duration>) -> Duration {
     retry.map_or(RETRY_FIRST, |r| (r * 2).min(RETRY_MAX))
diff --git a/addons/web/src/ws.rs b/addons/web/src/ws.rs
index 7d13a6a..6d6e237 100644
--- a/addons/web/src/ws.rs
+++ b/addons/web/src/ws.rs
@@ -9,7 +9,7 @@ use std::time::Duration;
 
 use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
 use axum::extract::State;
-use axum::http::StatusCode;
+use axum::http::{HeaderMap, StatusCode};
 use axum::response::{IntoResponse, Response};
 use futures::{SinkExt, StreamExt};
 use serde::de::DeserializeOwned;
@@ -22,7 +22,8 @@ use super::input::Decision;
 use super::spawn::{self, check_size, RespawnReq, SpawnReq};
 use super::state::Layout;
 use super::AppState;
-use super::{addons, ext, git_ops};
+use super::{addons, auth, ext, git_ops, sessions};
+use claudio_core::paths;
 use claudio_core::proto::{Msg, SearchHit, SessionId};
 
 /// The largest client text frame: a 64 KiB text and its JSON envelope.
@@ -59,14 +60,24 @@ impl Drop for TabSlot {
     }
 }
 
-/// `GET /ws` (the guard checked Host, Origin and the cookie).
-pub(crate) async fn upgrade(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
+/// `GET /ws` (the guard checked Host, Origin and the cookie). While the
+/// tab is open, its session counts as used (`sessions::OpenTab`).
+pub(crate) async fn upgrade(
+    State(state): State<AppState>,
+    headers: HeaderMap,
+    ws: WebSocketUpgrade,
+) -> Response {
     let Some(slot) = TabSlot::take(&state.tabs) else {
         return (StatusCode::SERVICE_UNAVAILABLE, "too many tabs").into_response();
     };
+    let session = auth::cookie_value(&headers).and_then(|v| sessions::hash_of(&v));
     ws.max_message_size(2 << 20)
         .max_frame_size(2 << 20)
-        .on_upgrade(move |socket| tab(state, socket, slot))
+        .on_upgrade(move |socket| async move {
+            let kept = Arc::clone(&state.auth.sessions);
+            let _open = session.map(|hash| sessions::OpenTab::start(kept, hash, paths::unix_now));
+            tab(state, socket, slot).await;
+        })
 }
 
 /// What a client frame asks for.
`````

Run: `cargo test --locked -p claudio-web --lib sessions`, then
`cargo test --locked --test web -- --test-threads=1`, then the `make check` sequence.
Expected: `test result: ok. 16 passed; 0 failed` (the `sessions` filter); the web suite
`ok. 38 passed`; `rules: 0 problem(s)`.

Step 3: commit
`git commit -m "feat(web): an open tab keeps its sign-in alive" -- addons/web/src/sessions.rs addons/web/src/ws.rs`

Interfaces this task provides to later tasks: `sessions::{MARK_EVERY, OpenTab::start(Arc<Sessions>,
Hash, clock: fn() -> u64) -> OpenTab}`.

### Task 7: The palette signs this browser out
Model: sonnet   Repo: /home/p4u/claudio   Files: `addons/web/web/app/ui/palette.js`,
`addons/web/web/e2e/tests/signout.spec.mjs` (new)
Goal: spec §3 "Sign-out in the page" (AC15): the palette's "Sign out" posts `/api/logout` and
reloads; the page then shows its login, and `/api/me` answers 401 with the old cookie, after a
reload too. No new message, DTO or test id; the item is text in the existing palette
(DESIGN.md unchanged: F3).

**Not verified in the scratch run** (no Chromium in `~/.cache/ms-playwright`, no
`node_modules`): the two files parse (`node --check`) and the node suites pass. Run the e2e
yourself as below; it needs `npm ci` (network) once.

Step 1: write the failing e2e spec

Patch `task7-step1` (`addons/web/web/e2e/tests/signout.spec.mjs`):

`````diff
diff --git a/addons/web/web/e2e/tests/signout.spec.mjs b/addons/web/web/e2e/tests/signout.spec.mjs
new file mode 100644
index 0000000..77ec0b0
--- /dev/null
+++ b/addons/web/web/e2e/tests/signout.spec.mjs
@@ -0,0 +1,27 @@
+import { test, expect } from '@playwright/test';
+import { startServer } from '../fixtures/server.mjs';
+
+let srv;
+test.beforeAll(async () => { srv = await startServer(); });
+test.afterAll(async () => { await srv?.stop(); });
+
+test('the palette signs this browser out, and its cookie stays refused', async ({ page, request }) => {
+  await page.goto(srv.url);
+  await expect(page.getByTestId('rail')).toBeVisible();
+  const cookie = (await page.context().cookies()).find((c) => c.name === 'claudio_web');
+  expect(cookie?.value).toMatch(/^[0-9a-f]{64}$/);
+  // Max-Age=34560000: it outlives the browser (400 days, not a session cookie).
+  expect(cookie.expires).toBeGreaterThan(Date.now() / 1000 + 399 * 24 * 3600);
+  const me = () => request.get(`http://127.0.0.1:${srv.port}/api/me`, { headers: { Cookie: `claudio_web=${cookie.value}` } });
+  expect((await me()).status()).toBe(204);
+  await page.keyboard.press('Control+k');
+  await page.getByTestId('palette-input').fill('Sign out');
+  await page.getByTestId('palette-item').first().click();
+  await expect(page.getByText('Open the URL')).toBeVisible();
+  expect((await page.context().cookies()).find((c) => c.name === 'claudio_web')).toBeUndefined();
+  expect((await me()).status()).toBe(401);
+  await page.reload();
+  await expect(page.getByText('Open the URL')).toBeVisible();
+  expect((await me()).status()).toBe(401);
+  expect(await page.evaluate(() => window.__claudioCsp ?? [])).toEqual([]);
+});
`````

Run: `cargo build --locked && cd addons/web/web/e2e && PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 npm ci --no-audit --no-fund && npx playwright test signout`
(if Chromium is missing: `npx playwright install chromium` once, in `addons/web/web/e2e`).
Expected (not run in the scratch check): `1 failed`: the palette has no "Sign out", so the
filtered list shows "No match", clicking its first item times out
(`locator.click: Timeout 10000ms exceeded`), and the page never shows "Open the URL".

Step 2: implement

Patch `task7-step2` (`addons/web/web/app/ui/palette.js`):

`````diff
diff --git a/addons/web/web/app/ui/palette.js b/addons/web/web/app/ui/palette.js
index b753be0..bd6df71 100644
--- a/addons/web/web/app/ui/palette.js
+++ b/addons/web/web/app/ui/palette.js
@@ -14,6 +14,13 @@ const SKIP = new Set(['palette', 'palette-ctrl', 'close-overlay', 'leave-termina
 
 const quiet = (p) => p.catch(() => {});
 
+// Ends this browser's sign-in (POST /api/logout), then reloads: the page shows its login (the
+// password form, or the card that asks for the URL `claudio web` printed).
+async function signOut() {
+  try { await fetch('/api/logout', { method: 'POST', credentials: 'same-origin' }); } catch { /* reload anyway */ }
+  location.reload();
+}
+
 export function sessionCommands(s) {
   const name = s.label || s.id.slice(0, 8);
   const tag = s.host === 'local' ? name : `${name} @${s.host}`;
@@ -59,6 +66,7 @@ export function buildCommands(m, cfg) {
     out.push({ id: `action:${b.action}`, group: 'Actions', title: b.help[0].toUpperCase() + b.help.slice(1), keys: b.keys, when: () => hasAction(b.action), run: () => runAction(b.action) });
   }
   out.push({ id: 'settings', group: 'App', title: 'Settings', run: () => { overlay.value = 'settings'; } });
+  out.push({ id: 'sign-out', group: 'App', title: 'Sign out', run: signOut });
   out.push(...addonCommands(addonDecls.value, panelActions.value, m.sessions[focused.value]));
   for (const s of orderedSessions(m)) out.push(...sessionCommands(s));
   return out;
`````

Run: `node --experimental-default-type=module --check addons/web/web/app/ui/palette.js && node --test addons/web/web/tests/*.test.mjs`,
then `make test-web` (all specs: the node tests, a debug build, every Playwright spec).
Expected: `# fail 0`; Playwright `passed` for every spec, `signout.spec.mjs` included, and no
`securitypolicyviolation`.

Step 3: commit
`git add addons/web/web/e2e/tests/signout.spec.mjs && git commit -m "feat(web): the palette signs this browser out" -- addons/web/web/app/ui/palette.js addons/web/web/e2e/tests/signout.spec.mjs`

Interfaces this task provides to later tasks: the palette command `sign-out` ("Sign out").

### Task 8: A --remote password may be 8 bytes
Model: sonnet   Repo: /home/p4u/claudio   Files: `docs/web-protocol.md`, `addons/web/src/password.rs`,
`addons/web/tests/web.rs`, `addons/web/tests/web_harness/mod.rs`, `addons/web/tests/web_harness/tls.rs`
Goal: spec §4 (AC14): `MIN_BYTES` 8; 8 bytes accepted, 7 refused with "at least 8 bytes", 1024
and 1025 as today (`password_length_is_12_to_1024_bytes` is replaced by
`password_length_is_8_to_1024_bytes`); `claudio web --set-password` typed through the PTY
harness with an 8-byte password exits 0 and that password logs in to a `--remote` server.

The harness gains `set_password_typed` (a `TuiProcess` PTY: `--set-password` refuses a pipe)
and `RemoteWeb::start_keeping_password` (`RemoteWeb::start` minus writing the fixture).

Step 0: the protocol doc's password range, in its own commit first (Spec issue 1, B2)

Patch `task8-docs` (`docs/web-protocol.md`):

`````diff
diff --git a/docs/web-protocol.md b/docs/web-protocol.md
index 9540e94..d82c913 100644
--- a/docs/web-protocol.md
+++ b/docs/web-protocol.md
@@ -21,7 +21,7 @@ is in git history.
 | Method and path | Cookie | Origin check | Answer |
 |---|---|---|---|
 | `GET`/`HEAD` `/`, `/index.html`, any embedded asset path | no | no | the asset; `404` when unknown |
-| `POST /api/login` body `{"token":"<64 hex>"}` or `{"password":"<12–1024 bytes>"}` (≤ 4 KiB) | no | yes | `204` + `Set-Cookie`; `401` wrong token or password, or a token from a non-local request with `--remote`; `429` locked; `400` bad body (neither or both fields, or a password without `--remote`); `413` too large; `503` no hashing slot within 2 s (does not count) |
+| `POST /api/login` body `{"token":"<64 hex>"}` or `{"password":"<8–1024 bytes>"}` (≤ 4 KiB) | no | yes | `204` + `Set-Cookie`; `401` wrong token or password, or a token from a non-local request with `--remote`; `429` locked; `400` bad body (neither or both fields, or a password without `--remote`); `413` too large; `503` no hashing slot within 2 s (does not count) |
 | `POST /api/logout` | yes | yes | `204` + an expiring `Set-Cookie` |
 | `GET /api/me` **(decided here)** | yes | no | `204`: the cookie is live (the page asks this at load when the URL has no token). `401` without one, body `{"login":[…]}`: the methods this request may use, `"token"` and/or `"password"` (below) |
 | `GET /ws` (WebSocket upgrade) | yes | yes | `101`; `503` past 16 tabs **(decided here)** |
`````

Commit: `git commit -m "docs(web-protocol): a password is 8 to 1024 bytes" -- docs/web-protocol.md`

Step 1: write the failing tests

Patch `task8-step1` (`addons/web/src/password.rs`, `addons/web/tests/web.rs`, `addons/web/tests/web_harness/mod.rs`, `addons/web/tests/web_harness/tls.rs`):

`````diff
diff --git a/addons/web/src/password.rs b/addons/web/src/password.rs
index 7afcd6c..9bdf282 100644
--- a/addons/web/src/password.rs
+++ b/addons/web/src/password.rs
@@ -215,14 +215,15 @@ mod tests {
     }
 
     #[test]
-    fn password_length_is_12_to_1024_bytes() {
-        let short = acceptable(&"x".repeat(11)).unwrap_err();
-        assert!(short.contains("12"), "{short}");
-        assert!(acceptable(&"x".repeat(12)).is_ok());
-        assert!(acceptable(&"é".repeat(6)).is_ok());
+    fn password_length_is_8_to_1024_bytes() {
+        let short = acceptable(&"x".repeat(7)).unwrap_err();
+        assert_eq!(short, "the password needs at least 8 bytes");
+        assert!(acceptable(&"x".repeat(8)).is_ok());
+        assert!(acceptable(&"é".repeat(4)).is_ok(), "bytes, not characters");
         assert!(acceptable(&"x".repeat(1024)).is_ok());
-        assert!(acceptable(&"x".repeat(1025)).is_err());
-        assert!(hash(&"x".repeat(11)).is_err());
+        let long = acceptable(&"x".repeat(1025)).unwrap_err();
+        assert_eq!(long, "the password takes at most 1024 bytes");
+        assert!(hash(&"x".repeat(7)).is_err());
         let s = Stored::parse(FIXTURE).unwrap();
         assert!(
             !s.verify(&"x".repeat(1025)),
diff --git a/addons/web/tests/web.rs b/addons/web/tests/web.rs
index a3780f3..e384ff6 100644
--- a/addons/web/tests/web.rs
+++ b/addons/web/tests/web.rs
@@ -391,6 +391,15 @@ fn web_set_password_refuses_a_pipe() {
     assert!(!h.config_home.join("claudio/web-password").exists());
 }
 
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn web_set_password_takes_8_bytes_typed_at_a_terminal() {
+    let h = ManagerHarness::new();
+    assert_eq!(set_password_typed(&h, "8-bytes!"), 0);
+    let r = RemoteWeb::start_keeping_password(h, &[], |_| {});
+    assert_eq!(r.login(&json!({"password": "8-bytes!"})).await.status, 204);
+    assert_eq!(r.login(&json!({"password": "8-bytes?"})).await.status, 401);
+}
+
 // ── --remote: TLS (AC2, AC4, AC6) ────────────────────────────────────────────
 
 #[test]
diff --git a/addons/web/tests/web_harness/mod.rs b/addons/web/tests/web_harness/mod.rs
index 6058961..e35b6ea 100644
--- a/addons/web/tests/web_harness/mod.rs
+++ b/addons/web/tests/web_harness/mod.rs
@@ -15,7 +15,7 @@ use std::os::unix::fs::PermissionsExt;
 use std::process::{Child, Command, Stdio};
 use std::{fs, thread};
 
-use crate::common::{ManagerHarness, BINARY, DAEMON_WAIT, WAIT};
+use crate::common::{ManagerHarness, Region, TuiProcess, BINARY, DAEMON_WAIT, WAIT};
 use futures::{SinkExt, StreamExt};
 use serde_json::{json, Value};
 use tokio_tungstenite::tungstenite::{self, client::IntoClientRequest, Message};
@@ -46,6 +46,20 @@ pub fn apply_env(h: &ManagerHarness, cmd: &mut Command) {
         .env_remove("ANTHROPIC_BASE_URL");
 }
 
+/// `claudio web --set-password` in a PTY (it refuses a pipe), `password`
+/// typed twice; its exit code.
+pub fn set_password_typed(h: &ManagerHarness, password: &str) -> u32 {
+    let mut cmd = portable_pty::CommandBuilder::new(BINARY);
+    cmd.args(["web", "--set-password"]);
+    h.apply(&mut cmd);
+    let mut tty = TuiProcess::spawn(cmd);
+    tty.wait_for("New password:", Region::Screen, WAIT);
+    tty.send_keys(format!("{password}\r").as_bytes());
+    tty.wait_for("Again:", Region::Screen, WAIT);
+    tty.send_keys(format!("{password}\r").as_bytes());
+    tty.wait_exit_code(DAEMON_WAIT)
+}
+
 pub fn free_port() -> u16 {
     std::net::TcpListener::bind("127.0.0.1:0")
         .unwrap()
diff --git a/addons/web/tests/web_harness/tls.rs b/addons/web/tests/web_harness/tls.rs
index 9cd8c7e..5bd2f68 100644
--- a/addons/web/tests/web_harness/tls.rs
+++ b/addons/web/tests/web_harness/tls.rs
@@ -67,6 +67,15 @@ impl RemoteWeb {
     /// fingerprint is `cert.pem`'s.
     pub fn start(h: ManagerHarness, args: &[&str], setup: impl FnOnce(&mut Command)) -> RemoteWeb {
         write_test_password(&h);
+        RemoteWeb::start_keeping_password(h, args, setup)
+    }
+
+    /// [`RemoteWeb::start`] with the password file `h` already has.
+    pub fn start_keeping_password(
+        h: ManagerHarness,
+        args: &[&str],
+        setup: impl FnOnce(&mut Command),
+    ) -> RemoteWeb {
         let port = free_port();
         let mut cmd = Command::new(BINARY);
         cmd.arg("web")
`````

Run: `cargo test --locked -p claudio-web --lib password`, then
`cargo test --locked --test web -- --test-threads=1 web_set_password`
Expected: `password_length_is_8_to_1024_bytes ... FAILED` (`left: "the password needs at least 12 bytes"`,
`right: "the password needs at least 8 bytes"`); `web_set_password_takes_8_bytes_typed_at_a_terminal ... FAILED`
(`wait_for timeout: "Again:" not found in Screen`).

Step 2: implement

Patch `task8-step2` (`addons/web/src/password.rs`):

`````diff
diff --git a/addons/web/src/password.rs b/addons/web/src/password.rs
index 9bdf282..e1274a7 100644
--- a/addons/web/src/password.rs
+++ b/addons/web/src/password.rs
@@ -18,8 +18,9 @@ use claudio_core::paths;
 pub(crate) const M_COST_KIB: u32 = 19 * 1024;
 pub(crate) const T_COST: u32 = 2;
 pub(crate) const P_COST: u32 = 1;
-/// A password is 12 to 1024 bytes of UTF-8.
-pub(crate) const MIN_BYTES: usize = 12;
+/// A password is 8 to 1024 bytes of UTF-8 (a stored one is checked
+/// against the maximum only).
+pub(crate) const MIN_BYTES: usize = 8;
 pub(crate) const MAX_BYTES: usize = 1024;
 /// A PHC string with these parameters is about 100 bytes; anything past
 /// this is not one.
`````

Run: the same two commands, then `cargo test --locked --test vocdoni_web --no-run` (it shares
the harness), then the `make check` sequence.
Expected: `test result: ok. 10 passed; 0 failed`; `test result: ok. 2 passed; 0 failed`; the
build succeeds; `rules: 0 problem(s)`.

Step 3: commit
`git commit -m "feat(web): a --remote password may be 8 bytes" -- addons/web/src/password.rs addons/web/tests/web.rs addons/web/tests/web_harness/mod.rs addons/web/tests/web_harness/tls.rs`

Interfaces this task provides to later tasks: `web_harness::set_password_typed(&ManagerHarness,
&str) -> u32` (the exit code); `web_harness::tls::RemoteWeb::start_keeping_password(h, args,
setup)`.

### Task 9: The daemon supervises an addon's service
Model: opus   Repo: /home/p4u/claudio   Files: `crates/core/src/addon/daemon.rs`,
`crates/core/src/addon/test_support.rs`, `crates/core/src/daemon/addon_ops.rs`,
`crates/core/src/daemon/command.rs`, `crates/core/src/daemon/mod.rs`,
`crates/core/src/daemon/server.rs`, `crates/core/src/daemon/service.rs` (new),
`crates/core/src/daemon/test_support.rs`, `crates/core/src/daemon/tests.rs`,
`crates/core/src/daemon/tests/service.rs` (new), `crates/core/src/freshness.rs`,
`crates/core/src/client.rs`, `crates/core/src/remote/bridge.rs`
Goal: spec §1, generic and tested with a `TestAddon` (A3, AC2, AC10): started at daemon start;
`ready` → `Running`; a crash and an unrequested exit 0 restart with backoff; a reload to `None`
→ `Off`; the same argv is a no-op; a new argv or `force` restarts; a `Failed` service starts
at once on a reload; shutdown stops the group; the child ends at stdin's EOF; no ready line in
time → `Failed`; 3 MiB of stderr leaves ≤ 2 MiB of logs; argv[0] is `Config::claudio`. No web
code.

What it adds: `ServiceState` and `DaemonOps::service` (default `Ok(None)`) and
`DaemonCtx::{service_reload, service_state}` in the addon API; `daemon::Config` gains
`config_file: Option<PathBuf>` (`paths::config_file()` in `from_env`, `None` in the tests'
configs) and `service_timing: ServiceTiming` (the 20 s / 3 s / 1-60 s / 5 min of the spec;
tests shorten it); the supervisor `daemon/service.rs` (names no addon); `Daemon.services`;
`serve_until_shutdown` (services start after the socket listens and stop, in parallel, after
`serve()` returns and before the lock goes); `Listening::daemon()` loses its `cfg` (net 0 lines:
`server.rs` is at its 900-line baseline); `TestDaemon` serves through `serve_until_shutdown`;
`TestAddon::with_service` (its first declared section's `argv`, and ops `reload`/`state`);
`freshness::EXIT_WAIT` (10 s) becomes public and is the lock wait in `client::ensure_daemon`
and `bridge::ensure_daemon_detached` (Spec issue 8). The fake claudio of the tests answers
`--version` first: `write_executable` runs it once to beat `ETXTBSY`.

Step 1: write the failing tests (the 14 service tests; the tests' `Config` literals gain the two
fields)

Patch `task9-step1` (`crates/core/src/daemon/command.rs`, `crates/core/src/daemon/tests.rs`, `crates/core/src/daemon/tests/service.rs`):

`````diff
diff --git a/crates/core/src/daemon/command.rs b/crates/core/src/daemon/command.rs
index 2896512..b8b3c30 100644
--- a/crates/core/src/daemon/command.rs
+++ b/crates/core/src/daemon/command.rs
@@ -325,6 +325,8 @@ mod tests {
             path: None,
             addon_timeout: std::time::Duration::from_secs(150),
             old_peer: false,
+            config_file: None,
+            service_timing: Default::default(),
         };
         let spec = SpawnSpec {
             id: uuid::Uuid::nil(),
@@ -405,6 +407,8 @@ mod tests {
             path: None,
             addon_timeout: std::time::Duration::from_secs(150),
             old_peer: false,
+            config_file: None,
+            service_timing: Default::default(),
         };
         for given in [ALLOW, "--dangerously-skip-permissions"] {
             let spec = SpawnSpec {
@@ -448,6 +452,8 @@ mod tests {
             path: None,
             addon_timeout: std::time::Duration::from_secs(150),
             old_peer: false,
+            config_file: None,
+            service_timing: Default::default(),
         };
         let spec = SpawnSpec {
             id: uuid::Uuid::nil(),
@@ -503,6 +509,8 @@ mod tests {
             path: None,
             addon_timeout: std::time::Duration::from_secs(150),
             old_peer: false,
+            config_file: None,
+            service_timing: Default::default(),
         };
         let agents = r#"{"reviewer":{"description":"d","prompt":"p"}}"#;
         let mcp = r#"{"mcpServers":{"kb":{"type":"http","url":"https://e.org"}}}"#;
@@ -573,6 +581,8 @@ mod tests {
             path: None,
             addon_timeout: std::time::Duration::from_secs(150),
             old_peer: false,
+            config_file: None,
+            service_timing: Default::default(),
         };
         let spec = SpawnSpec {
             args: vec![
diff --git a/crates/core/src/daemon/tests.rs b/crates/core/src/daemon/tests.rs
index d605698..525429f 100644
--- a/crates/core/src/daemon/tests.rs
+++ b/crates/core/src/daemon/tests.rs
@@ -570,6 +570,8 @@ async fn resume_retry_spawns_fresh_on_quick_exit() {
         path: None,
         addon_timeout: std::time::Duration::from_secs(150),
         old_peer: false,
+        config_file: None,
+        service_timing: Default::default(),
     };
     // Pre-populate a journal entry with a claude_session_id so the spawn
     // request carries --resume.
@@ -692,6 +694,8 @@ async fn immediate_exit_leaves_session_dormant() {
         path: None,
         addon_timeout: std::time::Duration::from_secs(150),
         old_peer: false,
+        config_file: None,
+        service_timing: Default::default(),
     };
 
     let listening = server::start(config, Arc::new(Addons::empty()))
@@ -1043,6 +1047,7 @@ mod addon_ops;
 mod freshness;
 mod git;
 mod login_path;
+mod service;
 mod spawn_env;
 
 // ── Respawn (reset a session in place) ───────────────────────────────────────
diff --git a/crates/core/src/daemon/tests/service.rs b/crates/core/src/daemon/tests/service.rs
new file mode 100644
index 0000000..2b6ee3b
--- /dev/null
+++ b/crates/core/src/daemon/tests/service.rs
@@ -0,0 +1,384 @@
+//! Daemon tests for addon services ([`crate::daemon::service`]): a
+//! `TestAddon` named `svc` whose service is `[svc] argv` of a config.toml in
+//! the test's dir, run by a fake claudio (a shell script) that acts on its
+//! first argument and records each start (`starts`) and its `$0` (`argv0`).
+
+use serde_json::json;
+
+use super::*;
+use crate::addon::test_support::TestAddon;
+use crate::addon::{Addon, ServiceState};
+use crate::daemon::service::LOG_MAX;
+use crate::daemon::ServiceTiming;
+
+/// The fake claudio a service runs (`--version`: `write_executable`'s check).
+const FAKE_SERVICE: &str = r#"#!/bin/sh
+[ "$1" = "--version" ] && exit 0
+dir=$(dirname "$0")
+echo "$$" >> "$dir/starts"
+printf '%s\n' "$0" > "$dir/argv0"
+case "$1" in
+  serve) echo ready; exec cat > /dev/null ;;
+  group) sleep 600 & echo $! > "$dir/grandchild"; echo ready; exec cat > /dev/null ;;
+  exit) echo ready; sleep 0.2; exit "$2" ;;
+  silent) exec sleep 600 ;;
+  boom) echo boom >&2; exit 2 ;;
+  stubborn) trap '' TERM; echo ready; while :; do sleep 1; done ;;
+  noisy) echo ready; head -c 3145728 /dev/zero | tr '\0' x >&2; : > "$dir/written"; exec cat > /dev/null ;;
+  mode) if [ -e "$dir/ok" ]; then echo ready; exec cat > /dev/null; fi; echo 'not yet' >&2; exit 4 ;;
+esac
+exit 9
+"#;
+
+fn fast() -> ServiceTiming {
+    ServiceTiming {
+        ready: Duration::from_secs(5),
+        stop: Duration::from_millis(500),
+        backoff: Duration::from_millis(100),
+        backoff_max: Duration::from_millis(400),
+        stable: Duration::from_secs(60),
+    }
+}
+
+/// `[svc]` with `argv` (none: an empty section) in `dir/config.toml`.
+fn write_config(dir: &Path, argv: Option<&[&str]>) {
+    let text = match argv {
+        Some(argv) => format!("[svc]\nargv = {argv:?}\n"),
+        None => "[svc]\n".to_owned(),
+    };
+    std::fs::write(dir.join("config.toml"), text).unwrap();
+}
+
+/// A daemon running the fake service with `argv`, under `timing`.
+fn start_svc(argv: Option<&[&str]>, timing: ServiceTiming, old_peer: bool) -> TestDaemon {
+    let dir = TestDaemon::new_dir();
+    let script = dir.join("fake-claudio");
+    write_executable(&script, FAKE_SERVICE);
+    write_config(&dir, argv);
+    let svc: Arc<dyn Addon> = Arc::new(TestAddon::new("svc").sections(&["svc"]).with_service());
+    let config = Config {
+        claudio: script,
+        config_file: Some(dir.join("config.toml")),
+        service_timing: timing,
+        old_peer,
+        ..TestDaemon::config(&dir)
+    };
+    TestDaemon::start_with_addons(dir, config, Addons::new(vec![svc]).unwrap())
+}
+
+fn state(d: &TestDaemon) -> ServiceState {
+    d.daemon.services.state("svc")
+}
+
+async fn reload(d: &TestDaemon, force: bool) -> ServiceState {
+    d.daemon.services.reload(&d.daemon, "svc", force).await
+}
+
+/// The first state `pick` accepts, within the read timeout.
+async fn until(d: &TestDaemon, pick: impl Fn(&ServiceState) -> bool) -> ServiceState {
+    within(async {
+        loop {
+            let s = state(d);
+            if pick(&s) {
+                return s;
+            }
+            tokio::time::sleep(Duration::from_millis(20)).await;
+        }
+    })
+    .await
+}
+
+fn pid_of(s: &ServiceState) -> Option<u32> {
+    match s {
+        ServiceState::Running { pid, .. } => Some(*pid),
+        _ => None,
+    }
+}
+
+async fn running(d: &TestDaemon) -> u32 {
+    let s = until(d, |s| pid_of(s).is_some()).await;
+    pid_of(&s).unwrap()
+}
+
+fn alive(pid: u32) -> bool {
+    // SAFETY: kill(2) with signal 0 only checks that the process exists.
+    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
+}
+
+async fn gone(pid: u32) {
+    within(async {
+        while alive(pid) {
+            tokio::time::sleep(Duration::from_millis(20)).await;
+        }
+    })
+    .await
+}
+
+fn starts(d: &TestDaemon) -> usize {
+    std::fs::read_to_string(d.dir.join("starts"))
+        .map(|s| s.lines().count())
+        .unwrap_or(0)
+}
+
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn a_service_starts_with_the_daemon_and_runs_its_binary() {
+    let d = start_svc(Some(&["serve"]), fast(), false);
+    let pid = running(&d).await;
+    let ServiceState::Running { argv, since, .. } = state(&d) else {
+        panic!("not running");
+    };
+    assert_eq!(argv, ["serve"]);
+    assert!(since > 1_700_000_000, "{since}");
+    let argv0 = std::fs::read_to_string(d.dir.join("argv0")).unwrap();
+    assert_eq!(argv0.trim_end(), d.config.claudio.to_str().unwrap());
+    assert!(alive(pid));
+    // The same argv again: nothing happens.
+    assert_eq!(pid_of(&reload(&d, false).await), Some(pid));
+    assert_eq!(starts(&d), 1);
+}
+
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn a_crash_and_an_exit_0_both_restart_with_a_backoff() {
+    let d = start_svc(Some(&["exit", "3"]), fast(), false);
+    let failed = until(&d, |s| matches!(s, ServiceState::Failed { .. })).await;
+    let ServiceState::Failed { exit, retry_in, .. } = failed else {
+        unreachable!()
+    };
+    assert_eq!(exit, Some(3));
+    assert!(retry_in.is_some_and(|r| r <= Duration::from_millis(400)));
+    within(async {
+        while starts(&d) < 3 {
+            tokio::time::sleep(Duration::from_millis(20)).await;
+        }
+    })
+    .await;
+
+    write_config(&d.dir, Some(&["exit", "0"]));
+    assert!(pid_of(&reload(&d, false).await).is_some());
+    let before = starts(&d);
+    let failed = until(&d, |s| {
+        matches!(s, ServiceState::Failed { exit: Some(0), .. })
+    })
+    .await;
+    assert!(matches!(
+        failed,
+        ServiceState::Failed {
+            retry_in: Some(_),
+            ..
+        }
+    ));
+    within(async {
+        while starts(&d) < before + 2 {
+            tokio::time::sleep(Duration::from_millis(20)).await;
+        }
+    })
+    .await;
+}
+
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn a_reload_to_nothing_stops_it_for_good() {
+    let d = start_svc(Some(&["serve"]), fast(), false);
+    let pid = running(&d).await;
+    write_config(&d.dir, None);
+    assert_eq!(reload(&d, false).await, ServiceState::Off);
+    gone(pid).await;
+    tokio::time::sleep(Duration::from_millis(500)).await;
+    assert_eq!(state(&d), ServiceState::Off);
+    assert_eq!(starts(&d), 1);
+}
+
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn a_new_argv_or_force_restarts_and_a_failed_one_starts_at_once() {
+    let timing = ServiceTiming {
+        backoff: Duration::from_secs(30),
+        backoff_max: Duration::from_secs(60),
+        ..fast()
+    };
+    let d = start_svc(Some(&["serve"]), timing, false);
+    let first = running(&d).await;
+    write_config(&d.dir, Some(&["serve", "b"]));
+    let second = pid_of(&reload(&d, false).await).expect("running");
+    assert_ne!(second, first);
+    gone(first).await;
+    let third = pid_of(&reload(&d, true).await).expect("running");
+    assert_ne!(third, second);
+    gone(second).await;
+
+    write_config(&d.dir, Some(&["mode"]));
+    let failed = reload(&d, false).await;
+    let ServiceState::Failed {
+        exit,
+        tail,
+        retry_in,
+        ..
+    } = failed
+    else {
+        panic!("{failed:?}");
+    };
+    assert_eq!((exit, tail.as_str()), (Some(4), "not yet"));
+    assert!(retry_in.is_some_and(|r| r > Duration::from_secs(20)));
+    // The cause is gone; the same argv starts now, not after the backoff.
+    std::fs::write(d.dir.join("ok"), "").unwrap();
+    assert!(pid_of(&reload(&d, false).await).is_some());
+}
+
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn reloads_queued_behind_one_run_as_one() {
+    let d = start_svc(Some(&["serve"]), fast(), false);
+    running(&d).await;
+    let before = starts(&d);
+    let (a, b, c) = tokio::join!(reload(&d, true), reload(&d, true), reload(&d, true));
+    for s in [a, b, c] {
+        assert!(pid_of(&s).is_some(), "{s:?}");
+    }
+    // The first restarts; the two queued behind it run as one.
+    assert_eq!(starts(&d), before + 2);
+}
+
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn shutdown_stops_the_whole_group() {
+    let d = start_svc(Some(&["group"]), fast(), false);
+    let pid = running(&d).await;
+    let grandchild: u32 = std::fs::read_to_string(d.dir.join("grandchild"))
+        .unwrap()
+        .trim()
+        .parse()
+        .unwrap();
+    assert!(alive(grandchild));
+    let mut c = d.client().await;
+    c.request(Msg::Shutdown).await;
+    gone(pid).await;
+    gone(grandchild).await;
+}
+
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn a_service_ends_on_stdin_eof_when_its_daemon_dies() {
+    let d = start_svc(Some(&["serve"]), fast(), false);
+    let pid = running(&d).await;
+    // No signal: only the pipe closes, as when the daemon is SIGKILLed.
+    d.daemon.services.abandon();
+    gone(pid).await;
+}
+
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn a_stubborn_service_is_killed_after_the_grace() {
+    let d = start_svc(Some(&["stubborn"]), fast(), false);
+    let pid = running(&d).await;
+    write_config(&d.dir, None);
+    let started = std::time::Instant::now();
+    assert_eq!(reload(&d, false).await, ServiceState::Off);
+    assert!(started.elapsed() >= fast().stop);
+    gone(pid).await;
+}
+
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn no_ready_line_in_time_is_a_failure() {
+    let timing = ServiceTiming {
+        ready: Duration::from_millis(300),
+        backoff: Duration::from_secs(30),
+        ..fast()
+    };
+    let d = start_svc(Some(&["silent"]), timing, false);
+    let failed = until(&d, |s| matches!(s, ServiceState::Failed { .. })).await;
+    let ServiceState::Failed { tail, .. } = failed else {
+        unreachable!()
+    };
+    assert_eq!(tail, "no ready line within 300ms");
+    let pid: u32 = std::fs::read_to_string(d.dir.join("starts"))
+        .unwrap()
+        .trim()
+        .parse()
+        .unwrap();
+    gone(pid).await;
+}
+
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn a_failed_start_reports_its_exit_and_stderr() {
+    let d = start_svc(Some(&["boom"]), fast(), false);
+    let failed = until(&d, |s| matches!(s, ServiceState::Failed { .. })).await;
+    let ServiceState::Failed {
+        exit,
+        signal,
+        tail,
+        retry_in,
+    } = failed
+    else {
+        unreachable!()
+    };
+    assert_eq!((exit, signal, tail.as_str()), (Some(2), None, "boom"));
+    assert!(retry_in.is_some());
+}
+
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn its_log_never_passes_2_mib() {
+    use std::os::unix::fs::PermissionsExt;
+    let d = start_svc(Some(&["noisy"]), fast(), false);
+    running(&d).await;
+    let (log, old) = (d.dir.join("rt/svc.log"), d.dir.join("rt/svc.log.1"));
+    let size = |p: &Path| std::fs::metadata(p).map_or(0, |m| m.len());
+    within(async {
+        // The child wrote 3 MiB; the copier has drained the pipe once the
+        // two files hold more than 2 MiB minus the 1 MiB rotated away.
+        while !d.dir.join("written").exists() || size(&log) + size(&old) < LOG_MAX {
+            tokio::time::sleep(Duration::from_millis(20)).await;
+        }
+    })
+    .await;
+    tokio::time::sleep(Duration::from_millis(300)).await;
+    assert!(size(&log) <= LOG_MAX && size(&old) <= LOG_MAX);
+    assert!(size(&log) + size(&old) <= 2 * LOG_MAX);
+    assert!(size(&old) > 0, "rotated");
+    let mode = std::fs::metadata(&log).unwrap().permissions().mode() & 0o777;
+    assert_eq!(mode, 0o600);
+}
+
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn bad_sections_and_a_bad_config_fail_without_a_start() {
+    let d = start_svc(None, fast(), false);
+    std::fs::write(d.dir.join("config.toml"), "[svc]\nargv = 3\n").unwrap();
+    let expected = ServiceState::Failed {
+        exit: None,
+        signal: None,
+        tail: "[svc] argv: not a list of strings".to_owned(),
+        retry_in: None,
+    };
+    assert_eq!(reload(&d, false).await, expected);
+    std::fs::write(d.dir.join("config.toml"), "[svc\n").unwrap();
+    let ServiceState::Failed { tail, retry_in, .. } = reload(&d, false).await else {
+        panic!("not failed");
+    };
+    assert_eq!(
+        (tail.as_str(), retry_in),
+        ("config.toml does not parse", None)
+    );
+    tokio::time::sleep(Duration::from_millis(300)).await;
+    assert_eq!(starts(&d), 0);
+}
+
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn a_daemon_playing_the_previous_release_runs_no_service() {
+    let d = start_svc(Some(&["serve"]), fast(), true);
+    tokio::time::sleep(Duration::from_millis(500)).await;
+    assert_eq!(state(&d), ServiceState::Off);
+    assert_eq!(starts(&d), 0);
+}
+
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn an_addon_reloads_and_reads_its_service_through_its_ctx() {
+    let d = start_svc(None, fast(), false);
+    let mut c = d.client().await;
+    let op = |op: &str, body| Msg::AddonOp {
+        addon: "svc".into(),
+        op: op.into(),
+        body,
+    };
+    let reply = |m: Msg| match m {
+        Msg::AddonReply { body } => body.as_str().unwrap_or_default().to_owned(),
+        other => panic!("{other:?}"),
+    };
+    assert_eq!(reply(c.call(op("state", json!({}))).await), "Off");
+    write_config(&d.dir, Some(&["serve"]));
+    let got = reply(c.call(op("reload", json!({"force": false}))).await);
+    assert!(got.starts_with("Running"), "{got}");
+    assert!(reply(c.call(op("state", json!({}))).await).starts_with("Running"));
+}
`````

Run: `cargo test --locked -p claudio-core --lib daemon::tests::service`
Expected: compilation fails: `` error[E0432]: unresolved import `crate::addon::ServiceState` ``,
`` error[E0432]: unresolved import `crate::daemon::ServiceTiming` ``,
`` error[E0609]: no field `services` on type `std::sync::Arc<Daemon>` ``,
`` error[E0560]: struct `daemon::Config` has no field named `config_file` ``.

Step 2: implement

Patch `task9-step2` (`crates/core/src/addon/daemon.rs`, `crates/core/src/addon/test_support.rs`, `crates/core/src/client.rs`, `crates/core/src/daemon/addon_ops.rs`, `crates/core/src/daemon/mod.rs`, `crates/core/src/daemon/server.rs`, `crates/core/src/daemon/service.rs`, `crates/core/src/daemon/test_support.rs`, `crates/core/src/freshness.rs`, `crates/core/src/remote/bridge.rs`):

`````diff
diff --git a/crates/core/src/addon/daemon.rs b/crates/core/src/addon/daemon.rs
index 4b88288..5f0cb5b 100644
--- a/crates/core/src/addon/daemon.rs
+++ b/crates/core/src/addon/daemon.rs
@@ -5,6 +5,7 @@ use std::collections::BTreeSet;
 use std::ffi::OsString;
 use std::path::PathBuf;
 use std::sync::Arc;
+use std::time::Duration;
 
 use serde_json::Value;
 
@@ -24,6 +25,43 @@ pub trait DaemonOps: Send + Sync {
     fn on_claude_installed(&self, ctx: &Arc<dyn DaemonCtx>) {
         let _ = ctx;
     }
+    /// What the daemon should keep running for this addon: argv after the claudio binary,
+    /// from the addon's declared config sections. `Ok(None)`: nothing. `Err(msg)`: the
+    /// sections are invalid; `msg` names the key. Pure, no I/O.
+    fn service(&self, config: &dyn Fn(&str) -> toml::Table) -> Result<Option<Vec<String>>, String> {
+        let _ = config;
+        Ok(None)
+    }
+}
+
+/// An addon service in this daemon (docs/addons.md, "Daemon ops").
+#[derive(Debug, Clone, PartialEq, Eq)]
+pub enum ServiceState {
+    /// Nothing runs: `service()` answered `Ok(None)`, or nothing asked yet.
+    Off,
+    /// Started; its `ready` line has not come yet.
+    Starting,
+    /// It said `ready`.
+    Running {
+        /// The child's pid, also its process group.
+        pid: u32,
+        /// When it said `ready`, in Unix seconds.
+        since: u64,
+        /// What it was started with, after the claudio binary.
+        argv: Vec<String>,
+    },
+    /// Its start failed, it exited unasked, or `service()` refused the sections.
+    Failed {
+        /// Its exit code, when it exited with one.
+        exit: Option<i32>,
+        /// The signal that ended it, when one did.
+        signal: Option<i32>,
+        /// The last ≤ 4 KiB it wrote to stderr since this start (sanitised),
+        /// or why it did not start.
+        tail: String,
+        /// When it is started again (`None`: at the next reload or daemon start).
+        retry_in: Option<Duration>,
+    },
 }
 
 /// What the daemon offers an addon's op.
@@ -38,6 +76,14 @@ pub trait DaemonCtx: Send + Sync {
     fn cache_dir(&self) -> PathBuf;
     /// Held while claude update or a `claude plugin …` runs: one writer of `~/.claude`.
     fn updating(&self) -> Arc<tokio::sync::Mutex<()>>;
+    /// Re-ask `service()` and converge: stop, start, or restart when the argv changed (or
+    /// `force`). A service that is `Failed` is started now and its backoff reset, even with
+    /// the same argv. Resolves once the child is ready, has failed its first attempt, or is
+    /// off (≤ 25 s: a 3 s stop and a 20 s start). A second reload for the same addon waits for
+    /// the first; reloads that queue behind it run as one.
+    fn service_reload(&self, force: bool) -> BoxFuture<'static, ServiceState>;
+    /// This addon's service as of now.
+    fn service_state(&self) -> ServiceState;
 }
 
 /// Lets claude offer bypass-permissions mode; any mention in `--help` counts.
diff --git a/crates/core/src/addon/test_support.rs b/crates/core/src/addon/test_support.rs
index b5ba810..035cd59 100644
--- a/crates/core/src/addon/test_support.rs
+++ b/crates/core/src/addon/test_support.rs
@@ -37,6 +37,7 @@ pub struct TestAddon {
     has_launch: bool,
     has_ui: bool,
     has_daemon: bool,
+    has_service: bool,
     adopts: bool,
     routing: Mutex<Result<Routing, String>>,
     prepare: Mutex<Script>,
@@ -66,6 +67,7 @@ impl TestAddon {
             has_launch: false,
             has_ui: false,
             has_daemon: false,
+            has_service: false,
             adopts: false,
             routing: Mutex::new(Ok(Routing::Optional)),
             prepare: Mutex::new(Script::Ok(LaunchPart::default())),
@@ -163,6 +165,15 @@ impl TestAddon {
         self
     }
 
+    /// Has `DaemonOps` with a service: its first declared section's `argv`
+    /// (a list of strings; absent: no service), and ops `reload`
+    /// (`{"force": bool}`) and `state`, both answering the state's `Debug`.
+    pub fn with_service(mut self) -> Self {
+        self.has_daemon = true;
+        self.has_service = true;
+        self
+    }
+
     /// `claudio <word>` exits with `code`.
     pub fn command(mut self, word: &str, code: u8) -> Self {
         self.words.insert(word.into(), code);
@@ -449,7 +460,6 @@ impl DaemonOps for TestAddon {
         body: Value,
         ctx: Arc<dyn DaemonCtx>,
     ) -> BoxFuture<'a, Result<Value, String>> {
-        let _ = ctx;
         self.record(format!("daemon {op}"));
         Box::pin(async move {
             match op {
@@ -461,6 +471,12 @@ impl DaemonOps for TestAddon {
                 }
                 "big" => Ok(Value::String("x".repeat(600 * 1024))),
                 "fail" => Err("echo failed".into()),
+                "reload" => {
+                    let force = body.get("force").and_then(Value::as_bool) == Some(true);
+                    let state = ctx.service_reload(force).await;
+                    Ok(Value::String(format!("{state:?}")))
+                }
+                "state" => Ok(Value::String(format!("{:?}", ctx.service_state()))),
                 other => Err(format!("unknown op {other}")),
             }
         })
@@ -470,4 +486,19 @@ impl DaemonOps for TestAddon {
         self.record("installed".into());
         self.installs.fetch_add(1, Ordering::SeqCst);
     }
+    fn service(&self, config: &dyn Fn(&str) -> toml::Table) -> Result<Option<Vec<String>>, String> {
+        let Some(section) = self.sections.first().filter(|_| self.has_service) else {
+            return Ok(None);
+        };
+        let bad = || format!("[{section}] argv: not a list of strings");
+        match config(section).get("argv") {
+            None => Ok(None),
+            Some(toml::Value::Array(words)) => words
+                .iter()
+                .map(|w| w.as_str().map(str::to_owned).ok_or_else(bad))
+                .collect::<Result<Vec<_>, _>>()
+                .map(Some),
+            Some(_) => Err(bad()),
+        }
+    }
 }
diff --git a/crates/core/src/client.rs b/crates/core/src/client.rs
index 54e3b6b..3c16aa9 100644
--- a/crates/core/src/client.rs
+++ b/crates/core/src/client.rs
@@ -38,7 +38,7 @@ use tokio::net::UnixStream;
 use tokio::sync::{mpsc, oneshot, Notify};
 use tokio::time::Instant as TokioInstant;
 
-use crate::freshness::DaemonCheck;
+use crate::freshness::{DaemonCheck, EXIT_WAIT};
 use crate::paths;
 use crate::proto::{
     read_frame, write_frame, Envelope, Frame, Hello, Msg, SessionEvent, SessionId, Welcome, PROTO,
@@ -715,7 +715,7 @@ pub fn ensure_daemon() -> io::Result<()> {
     // Validate the runtime directory first.
     paths::ensure_private_dir(&dir)?;
 
-    if crate::freshness::wait_for_lock(&socket, &paths::daemon_lock(), Duration::from_secs(5)) {
+    if crate::freshness::wait_for_lock(&socket, &paths::daemon_lock(), EXIT_WAIT) {
         return Ok(());
     }
     let log_path = dir.join("daemon.log");
diff --git a/crates/core/src/daemon/addon_ops.rs b/crates/core/src/daemon/addon_ops.rs
index c9411b3..c81e333 100644
--- a/crates/core/src/daemon/addon_ops.rs
+++ b/crates/core/src/daemon/addon_ops.rs
@@ -12,7 +12,7 @@ use serde_json::Value;
 use tokio::sync::Semaphore;
 
 use super::Daemon;
-use crate::addon::{Addons, BoxFuture, ClaudeFlags, DaemonCtx};
+use crate::addon::{Addons, BoxFuture, ClaudeFlags, DaemonCtx, ServiceState};
 use crate::proto::{self, Msg};
 
 /// How long one addon op may run (`Config::addon_timeout`).
@@ -73,6 +73,15 @@ impl DaemonCtx for Ctx {
     fn updating(&self) -> Arc<tokio::sync::Mutex<()>> {
         Arc::clone(&self.daemon.updating)
     }
+
+    fn service_reload(&self, force: bool) -> BoxFuture<'static, ServiceState> {
+        let (daemon, addon) = (Arc::clone(&self.daemon), self.addon);
+        Box::pin(async move { daemon.services.reload(&daemon, addon, force).await })
+    }
+
+    fn service_state(&self) -> ServiceState {
+        self.daemon.services.state(self.addon)
+    }
 }
 
 /// Run one AddonOp: body cap, find the addon's DaemonOps, take a slot (none
diff --git a/crates/core/src/daemon/mod.rs b/crates/core/src/daemon/mod.rs
index c845f18..704b5a4 100644
--- a/crates/core/src/daemon/mod.rs
+++ b/crates/core/src/daemon/mod.rs
@@ -13,6 +13,7 @@
 //! - [`login_path`] — the user's login-shell `PATH`, for claude and sessions.
 //! - [`manifest`] — proxy plugin manifests: what a plugin's sessions get
 //!   here, and the plugin installs it needs.
+//! - [`service`] — the addons' long-running services, supervised.
 //!
 //! This module holds the shared [`Daemon`] state: the registry of journaled
 //! and live sessions, and the all-clients event broadcast.
@@ -28,6 +29,7 @@ mod journal;
 mod lifecycle;
 mod login_path;
 mod server;
+mod service;
 mod session;
 mod stats;
 #[cfg(any(test, feature = "test-support"))]
@@ -53,6 +55,7 @@ use crate::proto::{
 };
 use crate::remote::probe::Probe;
 use journal::{Entry, Journal};
+pub use service::ServiceTiming;
 use session::{Cmd, Handle};
 
 /// Capacity of the all-clients event broadcast. A client that falls this far
@@ -98,6 +101,11 @@ pub struct Config {
     /// "Mixed versions"): no `addon:` features, `AddonOp` refused as unknown,
     /// every `env_remove` taken as absent and `optional` ignored.
     pub old_peer: bool,
+    /// The `config.toml` the addons' services are read from, at start and at
+    /// every reload ([`service`]); `None`: no addon has a service.
+    pub config_file: Option<PathBuf>,
+    /// How the addons' services are started, stopped and restarted.
+    pub service_timing: ServiceTiming,
 }
 
 impl Config {
@@ -126,6 +134,8 @@ impl Config {
             path,
             addon_timeout: addon_ops::TIMEOUT,
             old_peer: admit::old_peer(std::env::var_os("CLAUDIO_TEST_OLD_DAEMON").as_deref()),
+            config_file: Some(paths::config_file()),
+            service_timing: ServiceTiming::default(),
         })
     }
 
@@ -200,7 +210,7 @@ pub fn run(addons: Arc<crate::addon::Addons>) -> std::process::ExitCode {
         match server::start(config, addons) {
             Ok(Some(listening)) => (
                 std::process::ExitCode::SUCCESS,
-                Some(listening.serve().await),
+                Some(serve_until_shutdown(listening).await),
             ),
             Ok(None) => {
                 tracing::info!("another daemon is already running");
@@ -224,6 +234,18 @@ pub fn run(addons: Arc<crate::addon::Addons>) -> std::process::ExitCode {
 /// How long a shutting-down daemon waits for its blocking tasks.
 const SHUTDOWN_GRACE: Duration = Duration::from_secs(1);
 
+/// Start the addons' services, serve until asked to shut down, then stop
+/// every service (in parallel) before handing back the startup lock: a
+/// successor's services never run beside ours.
+async fn serve_until_shutdown(listening: server::Listening) -> std::fs::File {
+    let daemon = listening.daemon();
+    service::Services::start_all(&daemon);
+    let lock = listening.serve().await;
+    let grace = daemon.config.service_timing.stop;
+    daemon.services.stop_all(grace).await;
+    lock
+}
+
 /// State shared by every connection and session actor.
 pub struct Daemon {
     config: Config,
@@ -252,6 +274,8 @@ pub struct Daemon {
     /// The addons this binary runs, and each daemon-side one's op slots.
     addons: Arc<crate::addon::Addons>,
     addon_slots: HashMap<&'static str, Arc<tokio::sync::Semaphore>>,
+    /// The addons' services ([`service`]).
+    pub(crate) services: service::Services,
 }
 
 /// Journaled sessions, and the live subset with a running process.
@@ -280,6 +304,7 @@ impl Daemon {
         let (events, _) = broadcast::channel(EVENTS_CAPACITY);
         let (stats_tx, stats_rx) = tokio::sync::watch::channel(stats::HostSnapshot::default());
         tokio::spawn(stats::sample_loop(stats_tx));
+        let services = service::Services::new(&addons, config.service_timing);
         Daemon {
             config,
             registry: Mutex::new(Registry {
@@ -298,6 +323,7 @@ impl Daemon {
             stats_rx,
             search: server::SearchSlot::new(),
             addon_slots: addon_ops::slots(&addons),
+            services,
             addons,
         }
     }
@@ -845,6 +871,8 @@ mod claude_path_tests {
             path,
             addon_timeout: std::time::Duration::from_secs(150),
             old_peer: false,
+            config_file: None,
+            service_timing: Default::default(),
         }
     }
 
diff --git a/crates/core/src/daemon/server.rs b/crates/core/src/daemon/server.rs
index 58ff06f..5df0725 100644
--- a/crates/core/src/daemon/server.rs
+++ b/crates/core/src/daemon/server.rs
@@ -125,8 +125,8 @@ fn bind(path: &Path) -> io::Result<UnixListener> {
 }
 
 impl Listening {
-    /// The shared state, for tests that drive it directly.
-    #[cfg(any(test, feature = "test-support"))]
+    /// The shared state: the services stop through it after `serve`, and
+    /// tests drive it directly.
     pub(super) fn daemon(&self) -> Arc<Daemon> {
         Arc::clone(&self.daemon)
     }
diff --git a/crates/core/src/daemon/service.rs b/crates/core/src/daemon/service.rs
new file mode 100644
index 0000000..027cc4d
--- /dev/null
+++ b/crates/core/src/daemon/service.rs
@@ -0,0 +1,673 @@
+//! Addon services (docs/addons.md, "Daemon ops"; docs/daemon-protocol.md,
+//! "Addon services"): the one long-running child each daemon-side addon may
+//! ask for. At daemon start and at every reload, the addon's
+//! `DaemonOps::service` turns its own `config.toml` sections into an argv;
+//! the supervisor runs `Config::claudio` with it in a process group of its
+//! own, waits for its `ready` line, restarts it with a backoff whenever it
+//! exits unasked, copies its stderr into `<runtime>/<addon>.log` (at most
+//! 2 MiB with `.log.1`) and stops it (stdin closed, SIGTERM, SIGKILL) at a
+//! reload to nothing and at shutdown. Its stdin is a pipe the daemon holds
+//! and never writes (C3): however the daemon dies, the child sees EOF and
+//! stops itself. This module names no addon (A3).
+
+use std::collections::HashMap;
+use std::io::{self, Read};
+use std::os::unix::process::ExitStatusExt;
+use std::path::{Path, PathBuf};
+use std::process::Stdio;
+use std::sync::{Arc, Mutex, MutexGuard};
+use std::time::{Duration, Instant};
+
+use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
+use tokio::process::{ChildStderr, ChildStdin, ChildStdout, Command};
+use tokio::sync::watch;
+
+use super::{Config, Daemon};
+use crate::addon::{Addons, ServiceState, TEXT_BYTES};
+use crate::claude::SESSION_MARKERS;
+
+/// How the supervisor times a service ([`Config::service_timing`]; tests
+/// shorten it).
+#[derive(Debug, Clone, Copy, PartialEq, Eq)]
+pub struct ServiceTiming {
+    /// How long a start may take to print its `ready` line.
+    pub ready: Duration,
+    /// From SIGTERM to SIGKILL when a service is stopped.
+    pub stop: Duration,
+    /// The first backoff after a failure; it doubles up to `backoff_max`.
+    pub backoff: Duration,
+    /// The longest backoff.
+    pub backoff_max: Duration,
+    /// Running this long resets the backoff.
+    pub stable: Duration,
+}
+
+impl Default for ServiceTiming {
+    fn default() -> ServiceTiming {
+        ServiceTiming {
+            ready: Duration::from_secs(20),
+            stop: Duration::from_secs(3),
+            backoff: Duration::from_secs(1),
+            backoff_max: Duration::from_secs(60),
+            stable: Duration::from_secs(5 * 60),
+        }
+    }
+}
+
+/// A log past this size is renamed to `<addon>.log.1` before the next write.
+pub(crate) const LOG_MAX: u64 = 1 << 20;
+/// The stderr bytes kept in memory for `Failed.tail`.
+const TAIL_MAX: usize = 4096;
+/// The most read while waiting for the `ready` line.
+const READY_MAX: u64 = 4096;
+/// The largest `config.toml` the supervisor reads.
+const CONFIG_MAX: u64 = 1 << 20;
+/// How long an exit report waits for the stderr copier to reach EOF.
+const TAIL_WAIT: Duration = Duration::from_millis(500);
+
+/// `(exit code, signal)` once the child has exited and been reaped.
+type Exit = Option<(Option<i32>, Option<i32>)>;
+
+/// Every daemon-side addon's service slot, by addon name.
+pub(crate) struct Services {
+    slots: HashMap<&'static str, Arc<Slot>>,
+}
+
+/// One addon's service.
+struct Slot {
+    addon: &'static str,
+    /// Held across a reload or a restart: one at a time.
+    turn: tokio::sync::Mutex<()>,
+    tickets: Mutex<Tickets>,
+    inner: Mutex<Inner>,
+}
+
+/// Reloads asked and run: a reload that queued behind a running one is
+/// covered by the next one that runs.
+#[derive(Default)]
+struct Tickets {
+    asked: u64,
+    done: u64,
+    force: bool,
+}
+
+struct Inner {
+    state: ServiceState,
+    /// When a `Failed` service starts again.
+    retry_at: Option<Instant>,
+    /// What the last start ran (a restart runs it again).
+    argv: Option<Vec<String>>,
+    child: Option<Child>,
+    /// Bumped by every reload that acts and by shutdown: a pending restart
+    /// or the exit of a child stopped on purpose is then stale.
+    epoch: u64,
+    backoff: Duration,
+    /// Shutdown: nothing starts again.
+    closed: bool,
+}
+
+/// A started child: what stops it.
+struct Child {
+    pid: u32,
+    stdin: Option<ChildStdin>,
+    exited: watch::Receiver<Exit>,
+    tail: Arc<Mutex<Vec<u8>>>,
+    copied: watch::Receiver<bool>,
+    started: Instant,
+}
+
+impl Services {
+    /// A slot for each addon with daemon ops; all `Off`.
+    pub(crate) fn new(addons: &Addons, timing: ServiceTiming) -> Services {
+        let slots = addons
+            .iter()
+            .filter(|a| a.daemon().is_some())
+            .map(|a| {
+                let slot = Slot {
+                    addon: a.name(),
+                    turn: tokio::sync::Mutex::new(()),
+                    tickets: Mutex::default(),
+                    inner: Mutex::new(Inner {
+                        state: ServiceState::Off,
+                        retry_at: None,
+                        argv: None,
+                        child: None,
+                        epoch: 0,
+                        backoff: timing.backoff,
+                        closed: false,
+                    }),
+                };
+                (a.name(), Arc::new(slot))
+            })
+            .collect();
+        Services { slots }
+    }
+
+    /// `addon`'s service as of now (`Off` for an addon without a slot).
+    pub(crate) fn state(&self, addon: &str) -> ServiceState {
+        self.slots
+            .get(addon)
+            .map_or(ServiceState::Off, |s| s.state())
+    }
+
+    /// At daemon start (the socket listens): every addon's service, from
+    /// config.toml. Nothing for a daemon playing the previous release, or
+    /// without a config file.
+    pub(crate) fn start_all(daemon: &Arc<Daemon>) {
+        if daemon.config.old_peer || daemon.config.config_file.is_none() {
+            return;
+        }
+        for addon in daemon.services.slots.keys().copied() {
+            let daemon = Arc::clone(daemon);
+            tokio::spawn(async move {
+                daemon.services.reload(&daemon, addon, false).await;
+            });
+        }
+    }
+
+    /// `DaemonCtx::service_reload`: one reload at a time per addon; reloads
+    /// queued behind a running one run as one (with `force` if any asked).
+    pub(crate) async fn reload(
+        &self,
+        daemon: &Arc<Daemon>,
+        addon: &str,
+        force: bool,
+    ) -> ServiceState {
+        let Some(slot) = self.slots.get(addon) else {
+            return ServiceState::Off;
+        };
+        let ticket = slot.ticket(force);
+        let _turn = slot.turn.lock().await;
+        if let Some(force) = slot.take(ticket) {
+            converge(daemon, slot, force).await;
+        }
+        slot.state()
+    }
+
+    /// At shutdown, after the socket stopped listening: stop every service
+    /// at once; nothing starts again.
+    pub(crate) async fn stop_all(&self, grace: Duration) {
+        let children: Vec<Child> = self
+            .slots
+            .values()
+            .filter_map(|slot| {
+                let mut inner = slot.lock();
+                inner.closed = true;
+                inner.epoch += 1;
+                inner.retry_at = None;
+                inner.child.take()
+            })
+            .collect();
+        futures::future::join_all(children.into_iter().map(|c| stop(c, grace))).await;
+    }
+
+    /// What a daemon killed by a signal leaves: every stdin closed by the
+    /// kernel, no signal sent, nothing restarted.
+    #[cfg(test)]
+    pub(crate) fn abandon(&self) {
+        for slot in self.slots.values() {
+            let mut inner = slot.lock();
+            inner.closed = true;
+            if let Some(child) = inner.child.as_mut() {
+                child.stdin = None;
+            }
+        }
+    }
+}
+
+impl Slot {
+    /// The inner state; never held across an await.
+    fn lock(&self) -> MutexGuard<'_, Inner> {
+        self.inner.lock().unwrap_or_else(|e| e.into_inner())
+    }
+
+    fn state(&self) -> ServiceState {
+        let inner = self.lock();
+        match &inner.state {
+            ServiceState::Failed {
+                exit, signal, tail, ..
+            } => ServiceState::Failed {
+                exit: *exit,
+                signal: *signal,
+                tail: tail.clone(),
+                retry_in: inner
+                    .retry_at
+                    .map(|at| at.saturating_duration_since(Instant::now())),
+            },
+            state => state.clone(),
+        }
+    }
+
+    fn ticket(&self, force: bool) -> u64 {
+        let mut t = self.tickets.lock().unwrap_or_else(|e| e.into_inner());
+        t.asked += 1;
+        t.force |= force;
+        t.asked
+    }
+
+    /// `Some(force)` when `ticket` still needs a run (this run covers every
+    /// ticket asked so far); `None` when one that started later has run.
+    fn take(&self, ticket: u64) -> Option<bool> {
+        let mut t = self.tickets.lock().unwrap_or_else(|e| e.into_inner());
+        if t.done >= ticket {
+            return None;
+        }
+        t.done = t.asked;
+        Some(std::mem::take(&mut t.force))
+    }
+}
+
+/// Re-ask the addon and act: nothing when it runs as asked (and not
+/// `force`), else stop what runs and start what is asked.
+async fn converge(daemon: &Arc<Daemon>, slot: &Arc<Slot>, force: bool) {
+    let wanted = wanted(daemon, slot.addon).await;
+    let stale = {
+        let mut inner = slot.lock();
+        if inner.closed {
+            return;
+        }
+        let running = matches!(inner.state, ServiceState::Running { .. });
+        let same = matches!(&wanted, Ok(Some(argv)) if inner.argv.as_ref() == Some(argv));
+        if running && same && !force {
+            return;
+        }
+        inner.epoch += 1;
+        inner.retry_at = None;
+        inner.backoff = daemon.config.service_timing.backoff;
+        inner.child.take()
+    };
+    if let Some(child) = stale {
+        stop(child, daemon.config.service_timing.stop).await;
+    }
+    match wanted {
+        Ok(Some(argv)) => start(daemon, slot, argv).await,
+        Ok(None) => {
+            let mut inner = slot.lock();
+            inner.state = ServiceState::Off;
+            inner.argv = None;
+        }
+        Err(why) => {
+            let mut inner = slot.lock();
+            inner.argv = None;
+            inner.state = ServiceState::Failed {
+                exit: None,
+                signal: None,
+                tail: crate::addon::line(&why, TEXT_BYTES),
+                retry_in: None,
+            };
+        }
+    }
+}
+
+/// What `addon` asks for now: its `service()` over its own sections of the
+/// config file, read off the runtime.
+async fn wanted(daemon: &Daemon, addon: &str) -> Result<Option<Vec<String>>, String> {
+    let Some(path) = daemon.config.config_file.clone() else {
+        return Ok(None);
+    };
+    if daemon.config.old_peer {
+        return Ok(None);
+    }
+    let Some(found) = daemon.addons.get(addon) else {
+        return Ok(None);
+    };
+    let Some(ops) = found.daemon() else {
+        return Ok(None);
+    };
+    let table = tokio::task::spawn_blocking(move || read_config(&path))
+        .await
+        .ok()
+        .flatten()
+        .ok_or_else(|| "config.toml does not parse".to_owned())?;
+    let sections = found.config_sections();
+    let section = |name: &str| -> toml::Table {
+        match table.get(name) {
+            Some(toml::Value::Table(t)) if sections.contains(&name) => t.clone(),
+            _ => toml::Table::new(),
+        }
+    };
+    ops.service(&section)
+}
+
+/// config.toml as a table: empty when missing, `None` when it cannot be
+/// read, is not a regular file (a FIFO would block the open), is over
+/// 1 MiB or is not TOML.
+fn read_config(path: &Path) -> Option<toml::Table> {
+    match std::fs::metadata(path) {
+        Ok(meta) if meta.is_file() => {}
+        Ok(_) => return None,
+        Err(e) if e.kind() == io::ErrorKind::NotFound => return Some(toml::Table::new()),
+        Err(_) => return None,
+    }
+    let file = std::fs::File::open(path).ok()?;
+    let mut text = String::new();
+    file.take(CONFIG_MAX + 1).read_to_string(&mut text).ok()?;
+    if text.len() as u64 > CONFIG_MAX {
+        return None;
+    }
+    text.parse().ok()
+}
+
+/// Start `argv` and wait for its ready line (under the slot's turn).
+async fn start(daemon: &Arc<Daemon>, slot: &Arc<Slot>, argv: Vec<String>) {
+    let timing = daemon.config.service_timing;
+    {
+        let mut inner = slot.lock();
+        inner.state = ServiceState::Starting;
+        inner.retry_at = None;
+        inner.argv = Some(argv.clone());
+    }
+    let log = log_path(&daemon.config, slot.addon);
+    let (child, stdout) = match spawn(&daemon.config, &argv, &log) {
+        Ok(started) => started,
+        Err(e) => {
+            let why = format!("cannot run {}: {e}", daemon.config.claudio.display());
+            return fail(daemon, slot, None, why);
+        }
+    };
+    let pid = child.pid;
+    let closed = {
+        let mut inner = slot.lock();
+        if inner.closed {
+            Some(child)
+        } else {
+            inner.child = Some(child);
+            None
+        }
+    };
+    if let Some(child) = closed {
+        stop(child, timing.stop).await;
+        return;
+    }
+    match wait_ready(stdout, timing.ready).await {
+        Ok(out) => {
+            tokio::spawn(async move {
+                let mut out = out;
+                let _ = tokio::io::copy(&mut out, &mut tokio::io::sink()).await;
+            });
+            let mut inner = slot.lock();
+            let Some(child) = inner.child.as_ref() else {
+                return;
+            };
+            let watch = (child.exited.clone(), child.started, inner.epoch);
+            inner.state = ServiceState::Running {
+                pid,
+                since: crate::paths::unix_now(),
+                argv,
+            };
+            drop(inner);
+            tracing::info!(addon = slot.addon, pid, "service running");
+            let (daemon, slot) = (Arc::clone(daemon), Arc::clone(slot));
+            tokio::spawn(monitor(daemon, slot, watch.0, watch.1, watch.2));
+        }
+        Err(why) => {
+            let child = slot.lock().child.take();
+            let Some(child) = child else {
+                return;
+            };
+            let (tail, mut copied) = (Arc::clone(&child.tail), child.copied.clone());
+            let exit = stop(child, timing.stop).await;
+            let _ = tokio::time::timeout(TAIL_WAIT, copied.wait_for(|done| *done)).await;
+            fail(daemon, slot, exit, tail_or(&tail, why));
+        }
+    }
+}
+
+/// Wait for a running child's exit; one nobody asked for is a failure.
+async fn monitor(
+    daemon: Arc<Daemon>,
+    slot: Arc<Slot>,
+    mut exited: watch::Receiver<Exit>,
+    started: Instant,
+    epoch: u64,
+) {
+    let exit = wait_exit(&mut exited).await;
+    let child = {
+        let mut inner = slot.lock();
+        if inner.epoch != epoch || inner.closed {
+            return;
+        }
+        if started.elapsed() >= daemon.config.service_timing.stable {
+            inner.backoff = daemon.config.service_timing.backoff;
+        }
+        inner.child.take()
+    };
+    let Some(mut child) = child else {
+        return;
+    };
+    tracing::warn!(addon = slot.addon, pid = child.pid, ?exit, "service exited");
+    // Its leftovers in its group, then its last stderr lines.
+    signal_group(child.pid, libc::SIGKILL);
+    let _ = tokio::time::timeout(TAIL_WAIT, child.copied.wait_for(|done| *done)).await;
+    let why = "it exited".to_owned();
+    fail(&daemon, &slot, exit, tail_or(&child.tail, why));
+}
+
+/// `Failed` with the next start after the backoff (which then doubles),
+/// unless the daemon is shutting down.
+fn fail(daemon: &Arc<Daemon>, slot: &Arc<Slot>, exit: Exit, tail: String) {
+    let timing = daemon.config.service_timing;
+    let (code, signal) = exit.unwrap_or((None, None));
+    let mut inner = slot.lock();
+    if inner.closed {
+        return;
+    }
+    let delay = inner.backoff;
+    inner.backoff = (delay * 2).min(timing.backoff_max);
+    inner.retry_at = Some(Instant::now() + delay);
+    inner.state = ServiceState::Failed {
+        exit: code,
+        signal,
+        tail,
+        retry_in: None,
+    };
+    let epoch = inner.epoch;
+    drop(inner);
+    let (daemon, slot) = (Arc::clone(daemon), Arc::clone(slot));
+    tokio::spawn(async move {
+        tokio::time::sleep(delay).await;
+        let _turn = slot.turn.lock().await;
+        let argv = {
+            let inner = slot.lock();
+            if inner.epoch != epoch || inner.closed {
+                return;
+            }
+            inner.argv.clone()
+        };
+        if let Some(argv) = argv {
+            start(&daemon, &slot, argv).await;
+        }
+    });
+}
+
+/// `<runtime>/<addon>.log`: beside the daemon's socket.
+pub(crate) fn log_path(config: &Config, addon: &str) -> PathBuf {
+    let dir = config.socket.parent().unwrap_or(Path::new("."));
+    dir.join(format!("{addon}.log"))
+}
+
+/// Run `Config::claudio` with `argv` in a process group of its own: the
+/// daemon's env without the session markers and with `Config::path`; stdin
+/// a pipe held here, stdout for the ready line, stderr to the log.
+fn spawn(config: &Config, argv: &[String], log: &Path) -> io::Result<(Child, ChildStdout)> {
+    let mut cmd = Command::new(&config.claudio);
+    cmd.args(argv)
+        .stdin(Stdio::piped())
+        .stdout(Stdio::piped())
+        .stderr(Stdio::piped())
+        .process_group(0);
+    for marker in SESSION_MARKERS {
+        cmd.env_remove(marker);
+    }
+    if let Some(path) = &config.path {
+        cmd.env("PATH", path);
+    }
+    let mut child = cmd.spawn()?;
+    let pid = child.id().ok_or_else(|| io::Error::other("no pid"))?;
+    let (stdin, stdout, stderr) = (child.stdin.take(), child.stdout.take(), child.stderr.take());
+    let (Some(stdout), Some(stderr)) = (stdout, stderr) else {
+        return Err(io::Error::other("no pipes"));
+    };
+    let (exit_tx, exited) = watch::channel(None);
+    tokio::spawn(async move {
+        let exit = match child.wait().await {
+            Ok(status) => (status.code(), status.signal()),
+            Err(_) => (None, None),
+        };
+        let _ = exit_tx.send(Some(exit));
+    });
+    let tail = Arc::new(Mutex::new(Vec::new()));
+    let (copied_tx, copied) = watch::channel(false);
+    tokio::spawn(copy_stderr(
+        stderr,
+        log.to_path_buf(),
+        Arc::clone(&tail),
+        copied_tx,
+    ));
+    let child = Child {
+        pid,
+        stdin,
+        exited,
+        tail,
+        copied,
+        started: Instant::now(),
+    };
+    Ok((child, stdout))
+}
+
+/// The first stdout line, read within `within` and at most 4 KiB: a line
+/// that starts with `ready`, or why not.
+async fn wait_ready(
+    stdout: ChildStdout,
+    within: Duration,
+) -> Result<BufReader<ChildStdout>, String> {
+    let mut out = BufReader::new(stdout);
+    let mut line = Vec::new();
+    let mut first = (&mut out).take(READY_MAX);
+    match tokio::time::timeout(within, first.read_until(b'\n', &mut line)).await {
+        Err(_) => Err(format!("no ready line within {within:?}")),
+        Ok(Err(e)) => Err(format!("cannot read its output: {e}")),
+        Ok(Ok(0)) => Err("it exited before it was ready".to_owned()),
+        Ok(Ok(_)) if line.starts_with(b"ready") && line.ends_with(b"\n") => Ok(out),
+        Ok(Ok(_)) => Err("its first line is not `ready`".to_owned()),
+    }
+}
+
+/// Stop a child: stdin closed, SIGTERM to its group, SIGKILL after
+/// `grace`, reaped, and SIGKILL for whatever of its group is left.
+async fn stop(mut child: Child, grace: Duration) -> Exit {
+    child.stdin = None;
+    if child.exited.borrow().is_none() {
+        signal_group(child.pid, libc::SIGTERM);
+    }
+    if tokio::time::timeout(grace, wait_exit(&mut child.exited))
+        .await
+        .is_err()
+    {
+        signal_group(child.pid, libc::SIGKILL);
+    }
+    let exit = wait_exit(&mut child.exited).await;
+    // A group id is not reused while a member lives: this reaches only them.
+    signal_group(child.pid, libc::SIGKILL);
+    exit
+}
+
+async fn wait_exit(exited: &mut watch::Receiver<Exit>) -> Exit {
+    match exited.wait_for(Option::is_some).await {
+        Ok(exit) => *exit,
+        Err(_) => None,
+    }
+}
+
+fn signal_group(pid: u32, signal: i32) {
+    if let Ok(pid) = i32::try_from(pid) {
+        if pid > 1 {
+            // SAFETY: kill(2) with a negative pid signals that process
+            // group, which the child leads (`process_group(0)`).
+            unsafe { libc::kill(-pid, signal) };
+        }
+    }
+}
+
+/// The stderr tail as text (sanitised, its last 4 KiB), or `why` when the
+/// child wrote nothing.
+fn tail_or(tail: &Mutex<Vec<u8>>, why: String) -> String {
+    let bytes = tail.lock().unwrap_or_else(|e| e.into_inner()).clone();
+    let text = crate::text::sanitize(&String::from_utf8_lossy(&bytes));
+    let text = text.trim();
+    if text.is_empty() {
+        return why;
+    }
+    let mut start = text.len().saturating_sub(TAIL_MAX);
+    while !text.is_char_boundary(start) {
+        start += 1;
+    }
+    text[start..].to_owned()
+}
+
+/// Copy the child's stderr into its log (0600; renamed to `.log.1` before
+/// a write would take it past [`LOG_MAX`]) and keep its last 4 KiB.
+async fn copy_stderr(
+    mut err: ChildStderr,
+    path: PathBuf,
+    tail: Arc<Mutex<Vec<u8>>>,
+    done: watch::Sender<bool>,
+) {
+    let mut log = LogFile::open(&path).await;
+    let mut buf = vec![0u8; 8192];
+    loop {
+        let n = match err.read(&mut buf).await {
+            Ok(0) | Err(_) => break,
+            Ok(n) => n,
+        };
+        {
+            let mut t = tail.lock().unwrap_or_else(|e| e.into_inner());
+            t.extend_from_slice(&buf[..n]);
+            let excess = t.len().saturating_sub(TAIL_MAX);
+            t.drain(..excess);
+        }
+        if let Some(l) = log.as_mut() {
+            l.write(&buf[..n]).await;
+        }
+    }
+    let _ = done.send(true);
+}
+
+/// The open log and its size.
+struct LogFile {
+    path: PathBuf,
+    file: tokio::fs::File,
+    len: u64,
+}
+
+impl LogFile {
+    async fn open(path: &Path) -> Option<LogFile> {
+        let file = tokio::fs::OpenOptions::new()
+            .create(true)
+            .append(true)
+            .mode(0o600)
+            .open(path)
+            .await
+            .ok()?;
+        let len = file.metadata().await.map_or(0, |m| m.len());
+        Some(LogFile {
+            path: path.to_path_buf(),
+            file,
+            len,
+        })
+    }
+
+    async fn write(&mut self, bytes: &[u8]) {
+        if self.len > 0 && self.len + bytes.len() as u64 > LOG_MAX {
+            let old = self.path.with_extension("log.1");
+            if tokio::fs::rename(&self.path, &old).await.is_ok() {
+                if let Some(fresh) = LogFile::open(&self.path).await {
+                    *self = fresh;
+                }
+            }
+        }
+        if self.file.write_all(bytes).await.is_ok() && self.file.flush().await.is_ok() {
+            self.len += bytes.len() as u64;
+        }
+    }
+}
diff --git a/crates/core/src/daemon/test_support.rs b/crates/core/src/daemon/test_support.rs
index ee6ab12..37a51db 100644
--- a/crates/core/src/daemon/test_support.rs
+++ b/crates/core/src/daemon/test_support.rs
@@ -160,6 +160,8 @@ impl TestDaemon {
             path: None,
             addon_timeout: std::time::Duration::from_secs(150),
             old_peer: false,
+            config_file: None,
+            service_timing: Default::default(),
         }
     }
 
@@ -188,7 +190,7 @@ impl TestDaemon {
             .unwrap()
             .expect("lock is free");
         let daemon = listening.daemon();
-        tokio::spawn(listening.serve());
+        tokio::spawn(super::serve_until_shutdown(listening));
         TestDaemon {
             dir,
             config,
diff --git a/crates/core/src/freshness.rs b/crates/core/src/freshness.rs
index 6f5037a..6a75222 100644
--- a/crates/core/src/freshness.rs
+++ b/crates/core/src/freshness.rs
@@ -40,8 +40,11 @@ use crate::remote::probe::Probe;
 /// What the bridge prints on stderr when it left an outdated daemon running.
 pub const DEFERRED: &str = "daemon-update-deferred";
 
-/// How long an old daemon may take to exit after `Shutdown`.
-const EXIT_WAIT: Duration = Duration::from_secs(10);
+/// How long an old daemon may take to exit after `Shutdown`: it stops its
+/// addons' services first (≤ 3 s), then its blocking tasks (≤ 1 s), and
+/// holds its startup lock until then. Also how long a starter waits for a
+/// predecessor's lock ([`wait_for_lock`]).
+pub const EXIT_WAIT: Duration = Duration::from_secs(10);
 
 /// How a daemon's binary relates to ours.
 #[derive(Debug, Clone, Copy, PartialEq, Eq)]
diff --git a/crates/core/src/remote/bridge.rs b/crates/core/src/remote/bridge.rs
index cca0c50..d4a1e3e 100644
--- a/crates/core/src/remote/bridge.rs
+++ b/crates/core/src/remote/bridge.rs
@@ -145,7 +145,7 @@ async fn ensure_daemon_detached(socket: &Path) -> io::Result<()> {
     // Already running? Or on its way out (or in), holding the lock: a new
     // one would find it taken and quit.
     let (sock, lock) = (socket.to_owned(), paths::daemon_lock());
-    let wait = move || freshness::wait_for_lock(&sock, &lock, Duration::from_secs(5));
+    let wait = move || freshness::wait_for_lock(&sock, &lock, freshness::EXIT_WAIT);
     if tokio::task::spawn_blocking(wait)
         .await
         .map_err(io::Error::other)?
`````

Run: `cargo test --locked -p claudio-core --lib daemon::tests::service` (run it three times:
it must not flake), then `cargo test --locked -p claudio-core --lib`, then
`grep -rn '"web"' crates/core/src/daemon crates/core/src/addon`, then the `make check` sequence,
then `make matrix` (or its fallback in the Global Constraints), then
`cargo test --locked --test manager -- --test-threads=1`.
Expected: `test result: ok. 14 passed; 0 failed` each time; `test result: ok. 532 passed; 0 failed`;
the grep prints only `crates/core/src/addon/mod.rs:205:` (a test); `rules: 0 problem(s)`; every
matrix cell passes; `test result: ok. 42 passed`.

Step 3: commit
`git add crates/core/src/daemon/service.rs crates/core/src/daemon/tests/service.rs && git commit -m "feat(core): the daemon supervises an addon's service" -- crates/core/src/addon/daemon.rs crates/core/src/addon/test_support.rs crates/core/src/daemon/addon_ops.rs crates/core/src/daemon/command.rs crates/core/src/daemon/mod.rs crates/core/src/daemon/server.rs crates/core/src/daemon/service.rs crates/core/src/daemon/test_support.rs crates/core/src/daemon/tests.rs crates/core/src/daemon/tests/service.rs crates/core/src/freshness.rs crates/core/src/client.rs crates/core/src/remote/bridge.rs`

Interfaces this task provides to later tasks: `claudio_core::addon::ServiceState::{Off,
Starting, Running { pid: u32, since: u64, argv: Vec<String> }, Failed { exit: Option<i32>,
signal: Option<i32>, tail: String, retry_in: Option<Duration> }}`;
`DaemonOps::service(&self, config: &dyn Fn(&str) -> toml::Table) -> Result<Option<Vec<String>>,
String>`; `DaemonCtx::service_reload(&self, force: bool) -> BoxFuture<'static, ServiceState>`,
`DaemonCtx::service_state(&self) -> ServiceState`; `claudio_core::daemon::{Config { config_file,
service_timing, .. }, ServiceTiming { ready, stop, backoff, backoff_max, stable }}`;
`claudio_core::freshness::EXIT_WAIT`; the service's log `<runtime>/<addon>.log`.

### Task 10: claudio web --foreground; every suite serves with it
Model: sonnet   Repo: /home/p4u/claudio   Files: `addons/web/src/lib.rs`, `addons/web/tests/web.rs`,
`addons/web/tests/web_harness/mod.rs`, `addons/web/tests/web_harness/tls.rs`,
`addons/web/web/e2e/fixtures/server.mjs`
Goal: AC6: the flag exists before bare `claudio web` changes (Task 13), and every existing suite
that runs `claudio web` as a server passes with `--foreground` as its only change. Nothing else
changes yet: `--foreground` and a bare `claudio web` both serve in the foreground.

The places that start a server, found with `grep -rnE '"web"|arg\("web"\)|args\(\["web"' addons src tests`
and `grep -rn "'web'" addons/web/web/e2e`: `web_harness/mod.rs` (`Web::start_with`),
`web_harness/tls.rs` (`RemoteWeb::start`), `web.rs` (`web_answers_on_its_listen_address_and_a_proxy_port`,
`web_remote_refuses_to_start_without_a_password`) and the e2e fixture `server.mjs`.
`addons/vocdoni/tests/web.rs` uses the shared harness. The other `claudio web` calls in the
suites (`--print-url`, `--set-password`, `--print-cert`, parse errors) start no server and stay
as they are.

Step 1: write the failing tests (the suites switched; the parse test)

Patch `task10-step1` (`addons/web/src/lib.rs`, `addons/web/tests/web.rs`, `addons/web/tests/web_harness/mod.rs`, `addons/web/tests/web_harness/tls.rs`, `addons/web/web/e2e/fixtures/server.mjs`):

`````diff
diff --git a/addons/web/src/lib.rs b/addons/web/src/lib.rs
index 6449624..91f4b92 100644
--- a/addons/web/src/lib.rs
+++ b/addons/web/src/lib.rs
@@ -659,5 +659,10 @@ mod tests {
         ]))
         .is_ok());
         assert!(parse_args(&args(&["--print-cert"])).unwrap().print_cert);
+        assert!(!a.foreground);
+        let a = parse_args(&args(&["--foreground", "--listen", "127.0.0.1:9000"])).unwrap();
+        assert!(a.foreground && a.listen_given);
+        let err = parse_args(&args(&["--set-password", "--foreground"])).unwrap_err();
+        assert!(err.contains("no other argument"), "{err}");
     }
 }
diff --git a/addons/web/tests/web.rs b/addons/web/tests/web.rs
index e384ff6..e713aae 100644
--- a/addons/web/tests/web.rs
+++ b/addons/web/tests/web.rs
@@ -236,7 +236,7 @@ fn web_answers_on_its_listen_address_and_a_proxy_port() {
     };
     let h = ManagerHarness::new();
     let mut cmd = Command::new(BINARY);
-    cmd.arg("web")
+    cmd.args(["web", "--foreground"])
         .arg("--listen")
         .arg(format!("127.0.0.2:{port}"))
         .args(["--no-open", "--allow-host", "proxy.example:8443"]);
@@ -407,7 +407,7 @@ fn web_remote_refuses_to_start_without_a_password() {
     let h = ManagerHarness::new();
     let port = free_port();
     let mut cmd = Command::new(BINARY);
-    cmd.arg("web")
+    cmd.args(["web", "--foreground"])
         .arg("--remote")
         .arg(format!("127.0.0.1:{port}"))
         .arg("--no-open");
diff --git a/addons/web/tests/web_harness/mod.rs b/addons/web/tests/web_harness/mod.rs
index e35b6ea..0f5800a 100644
--- a/addons/web/tests/web_harness/mod.rs
+++ b/addons/web/tests/web_harness/mod.rs
@@ -84,7 +84,7 @@ impl Web {
     pub fn start_with(h: ManagerHarness, args: &[&str], setup: impl FnOnce(&mut Command)) -> Web {
         let port = free_port();
         let mut cmd = Command::new(BINARY);
-        cmd.arg("web")
+        cmd.args(["web", "--foreground"])
             .arg("--listen")
             .arg(format!("127.0.0.1:{port}"))
             .arg("--no-open")
diff --git a/addons/web/tests/web_harness/tls.rs b/addons/web/tests/web_harness/tls.rs
index 5bd2f68..3c5cd77 100644
--- a/addons/web/tests/web_harness/tls.rs
+++ b/addons/web/tests/web_harness/tls.rs
@@ -78,7 +78,7 @@ impl RemoteWeb {
     ) -> RemoteWeb {
         let port = free_port();
         let mut cmd = Command::new(BINARY);
-        cmd.arg("web")
+        cmd.args(["web", "--foreground"])
             .arg("--remote")
             .arg(format!("127.0.0.1:{port}"))
             .arg("--no-open")
diff --git a/addons/web/web/e2e/fixtures/server.mjs b/addons/web/web/e2e/fixtures/server.mjs
index 0de1d4a..68c44e6 100644
--- a/addons/web/web/e2e/fixtures/server.mjs
+++ b/addons/web/web/e2e/fixtures/server.mjs
@@ -159,8 +159,8 @@ export async function startServer({ proxy = false, hosts = [], remote = false, a
   const port = await freePort();
   const log = fs.openSync(path.join(root, 'web.log'), 'a');
   const args = remote
-    ? ['web', '--remote', `127.0.0.1:${port}`, '--no-open', ...allowHosts.flatMap((h) => ['--allow-host', h])]
-    : ['web', '--listen', `127.0.0.1:${port}`, '--no-open'];
+    ? ['web', '--foreground', '--remote', `127.0.0.1:${port}`, '--no-open', ...allowHosts.flatMap((h) => ['--allow-host', h])]
+    : ['web', '--foreground', '--listen', `127.0.0.1:${port}`, '--no-open'];
   const child = spawn(BIN, args, { env, stdio: ['ignore', 'pipe', log] });
   const url = await new Promise((resolve, reject) => {
     let buf = '';
`````

Run: `cargo test --locked -p claudio-web --lib tests::args`, then
`cargo test --locked --test web -- --test-threads=1 web_auth_gates`
Expected: `` error[E0609]: no field `foreground` on type `WebArgs` ``; `web_auth_gates_api_and_ws ... FAILED`
(the harness's `claudio web printed no URL`: the binary refuses `--foreground` and exits 2).

Step 2: implement

Patch `task10-step2` (`addons/web/src/lib.rs`):

`````diff
diff --git a/addons/web/src/lib.rs b/addons/web/src/lib.rs
index 91f4b92..5641a09 100644
--- a/addons/web/src/lib.rs
+++ b/addons/web/src/lib.rs
@@ -55,7 +55,7 @@ pub(crate) fn version() -> String {
 }
 
 const USAGE: &str =
-    "usage: claudio web [--listen 127.0.0.1:7337 | --remote [ADDR][:PORT] [--rotate-cert]] \
+    "usage: claudio web [--foreground] [--listen 127.0.0.1:7337 | --remote [ADDR][:PORT] [--rotate-cert]] \
 [--allow-host NAME[:PORT]]... [--no-open] [--print-url] [--rotate-token]
        claudio web --set-password
        claudio web --print-cert";
@@ -78,6 +78,8 @@ pub(crate) struct WebArgs {
     pub rotate_token: bool,
     /// `--set-password`: store the `--remote` password, then exit.
     pub set_password: bool,
+    /// `--foreground`: serve from this terminal, as `claudio web` always did.
+    pub foreground: bool,
 }
 
 /// Shared by every handler.
@@ -116,6 +118,7 @@ pub(crate) fn parse_args(args: &[String]) -> Result<WebArgs, String> {
         print_url: false,
         rotate_token: false,
         set_password: false,
+        foreground: false,
     };
     let mut it = args.iter().peekable();
     while let Some(arg) = it.next() {
@@ -149,6 +152,7 @@ pub(crate) fn parse_args(args: &[String]) -> Result<WebArgs, String> {
             "--print-url" => out.print_url = true,
             "--rotate-token" => out.rotate_token = true,
             "--set-password" => out.set_password = true,
+            "--foreground" => out.foreground = true,
             other => return Err(format!("unknown argument {other:?}\n{USAGE}")),
         }
     }
`````

Run: `cargo test --locked -p claudio-web --lib tests::args`, then
`cargo test --locked --test web --test vocdoni_web --test web_manager -- --test-threads=1`, then
`node --test addons/web/web/tests/*.test.mjs`, then the `make check` sequence; run `make test-web`
when Chromium is available (Task 7).
Expected: `test result: ok. 1 passed; 0 failed`; vocdoni_web `ok. 13 passed`, web
`ok. 39 passed`, web_manager `ok. 4 passed`; `# fail 0`; `rules: 0 problem(s)`.

Step 3: commit
`git commit -m "feat(web): claudio web --foreground; the suites serve with it" -- addons/web/src/lib.rs addons/web/tests/web.rs addons/web/tests/web_harness/mod.rs addons/web/tests/web_harness/tls.rs addons/web/web/e2e/fixtures/server.mjs`

Interfaces this task provides to later tasks: `WebArgs.foreground: bool` (`--foreground`).

### Task 11: claudio web --service, the child the daemon runs
Model: opus   Repo: /home/p4u/claudio   Files: `addons/web/src/lib.rs`, `addons/web/src/hub_io.rs`,
`addons/web/src/auth.rs`, `Makefile`, `addons/web/tests/web.rs`
Goal: the CLI table's `--service` row (AC5, part of AC3's machinery): `--service` refuses every
one-shot flag and `--foreground`; it serves like `--foreground` but never builds, prints or
logs the token URL, opens no browser, connects `local` with plain `client::connect` (never
`ensure_local_daemon`: the daemon that runs it is the authority), writes `ready` on stdout once
the listener is bound and the hub runs, and stops on stdin's EOF as on SIGTERM. An OS thread
reads stdin; at EOF it starts the graceful stop and exits the process 3 s later if it still
runs. It only loads the token (spec: "the service only ever loads it"; `claudio web` makes it
before it has the service started): without a valid token file it exits 1 with
`no token in <path>: run claudio web` and makes none (`auth::load_token`, split out of
`load_or_create_token`). Plus the debug-only `CLAUDIO_TEST_SERVICE_HANG` hook (Spec issue 2)
and `make static`'s grep for it.

`run` splits: `serve_here` (today's body, plus the service branches) and helpers `token_for`,
`start_url`, `init_tracing`, `watch_stdin`, `test_hang`, `started` (keeps `run` and `serve`
under 80 lines; `run` was at 77). A foreground start's messages and output stay byte for byte
(AC6).

Step 1: write the failing tests

Patch `task11-step1` (`addons/web/src/lib.rs`, `addons/web/tests/web.rs`):

`````diff
diff --git a/addons/web/src/lib.rs b/addons/web/src/lib.rs
index 5641a09..db2b2fb 100644
--- a/addons/web/src/lib.rs
+++ b/addons/web/src/lib.rs
@@ -668,5 +668,24 @@ mod tests {
         assert!(a.foreground && a.listen_given);
         let err = parse_args(&args(&["--set-password", "--foreground"])).unwrap_err();
         assert!(err.contains("no other argument"), "{err}");
+        let a = parse_args(&args(&[
+            "--service",
+            "--remote=0.0.0.0:7999",
+            "--allow-host=a.example",
+        ]))
+        .unwrap();
+        assert!(a.service && a.remote.is_some() && a.allow_hosts == ["a.example"]);
+        for one_shot in [
+            "--no-open",
+            "--print-url",
+            "--rotate-token",
+            "--rotate-cert",
+            "--set-password",
+            "--print-cert",
+            "--foreground",
+        ] {
+            let err = parse_args(&args(&["--service", "--remote", one_shot])).unwrap_err();
+            assert!(err.contains("--service takes only"), "{one_shot}: {err}");
+        }
     }
 }
diff --git a/addons/web/tests/web.rs b/addons/web/tests/web.rs
index e713aae..560962d 100644
--- a/addons/web/tests/web.rs
+++ b/addons/web/tests/web.rs
@@ -289,6 +289,78 @@ fn web_answers_on_its_listen_address_and_a_proxy_port() {
     assert!(reply.header("set-cookie").unwrap().contains("; Secure"));
 }
 
+// ── --service: what the daemon runs ──────────────────────────────────────────
+
+#[test]
+fn web_service_says_ready_never_prints_the_token_and_stops_at_eof() {
+    let h = ManagerHarness::new();
+    let port = free_port();
+    let listen = format!("--listen=127.0.0.1:{port}");
+    // It only loads the token: without one it refuses to start, and makes none.
+    let mut cmd = Command::new(BINARY);
+    cmd.args(["web", "--service", &listen]);
+    apply_env(&h, &mut cmd);
+    let out = cmd.stdin(Stdio::null()).output().unwrap();
+    assert_eq!(out.status.code(), Some(1), "{out:?}");
+    let err = String::from_utf8_lossy(&out.stderr);
+    assert!(
+        err.contains("no token in ") && err.contains("run claudio web"),
+        "{err}"
+    );
+    assert!(out.stdout.is_empty(), "no ready line");
+    let token_file = h.config_home.join("claudio/web-token");
+    assert!(!token_file.exists(), "it made no token");
+    let mut cmd = Command::new(BINARY);
+    cmd.args(["web", "--foreground", "--print-url"]);
+    apply_env(&h, &mut cmd);
+    assert!(cmd.output().unwrap().status.success(), "the token is made");
+    let mut cmd = Command::new(BINARY);
+    cmd.args(["web", "--service", &listen]);
+    apply_env(&h, &mut cmd);
+    let err = fs::File::create(h.root.join("service.err")).unwrap();
+    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(err);
+    let mut child = cmd.spawn().expect("start claudio web --service");
+    let mut out = BufReader::new(child.stdout.take().unwrap());
+    let mut line = String::new();
+    out.read_line(&mut line).unwrap();
+    assert_eq!(line, "ready\n");
+    let token = fs::read_to_string(h.config_home.join("claudio/web-token")).unwrap();
+    let token = token.trim();
+    let (host, origin) = (
+        format!("127.0.0.1:{port}"),
+        format!("http://127.0.0.1:{port}"),
+    );
+    let body = json!({"token": token}).to_string();
+    let login = http(
+        port,
+        "POST",
+        "/api/login",
+        &host,
+        &[("Origin", &origin)],
+        &body,
+    );
+    assert_eq!(login.status, 204, "it serves: {login:?}");
+    // The daemon's stop (or death): stdin closes.
+    drop(child.stdin.take());
+    let deadline = std::time::Instant::now() + WAIT;
+    while child.try_wait().unwrap().is_none() {
+        assert!(
+            std::time::Instant::now() < deadline,
+            "still running after EOF"
+        );
+        thread::sleep(std::time::Duration::from_millis(50));
+    }
+    let mut rest = String::new();
+    std::io::Read::read_to_string(&mut out, &mut rest).unwrap();
+    assert_eq!(rest, "", "stdout carries the ready line only");
+    let log = fs::read_to_string(h.root.join("service.err")).unwrap();
+    assert!(!log.contains(token), "the token is never printed: {log}");
+    assert!(
+        !h.runtime_dir.join("claudio/daemon.log").exists(),
+        "it never starts a daemon"
+    );
+}
+
 // ── Persistent sign-ins ───────────────────────────────────────────────────────
 
 fn cookie_header(cookie: &str) -> String {
`````

Run: `cargo test --locked -p claudio-web --lib tests::args`, then
`cargo test --locked --test web -- --test-threads=1 web_service_says`
Expected: `` error[E0609]: no field `service` on type `WebArgs` ``; `web_service_says_ready_never_prints_the_token_and_stops_at_eof ... FAILED`
(`left: Some(2)`, `right: Some(1)`: the binary refuses `--service` as an unknown argument).

Step 2: implement

Patch `task11-step2` (`Makefile`, `addons/web/src/auth.rs`, `addons/web/src/hub_io.rs`, `addons/web/src/lib.rs`):

`````diff
diff --git a/Makefile b/Makefile
index 7741126..1abbc5e 100644
--- a/Makefile
+++ b/Makefile
@@ -52,9 +52,9 @@ static:
 	@echo "→ $(DIST)/$(BIN)"
 	@file $(DIST)/$(BIN) || true
 	@echo "static check:"; ldd $(DIST)/$(BIN) 2>&1 || true
-	@# R8: the debug-only test peer hook never ships.
-	@if grep -q CLAUDIO_TEST_REMOTE_PEER $(DIST)/$(BIN); then \
-	  echo "error: the debug-only test hook is in the release binary"; exit 1; fi
+	@# R8: the debug-only test hooks never ship.
+	@if grep -q -e CLAUDIO_TEST_REMOTE_PEER -e CLAUDIO_TEST_SERVICE_HANG $(DIST)/$(BIN); then \
+	  echo "error: a debug-only test hook is in the release binary"; exit 1; fi
 
 ## Reproducible static build with zero host toolchain — uses a musl container.
 ## Works on any machine with Docker.
diff --git a/addons/web/src/auth.rs b/addons/web/src/auth.rs
index 69d7879..f1df647 100644
--- a/addons/web/src/auth.rs
+++ b/addons/web/src/auth.rs
@@ -71,23 +71,30 @@ pub(crate) fn token_ok(s: &str) -> bool {
     s.len() == 2 * TOKEN_BYTES && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
 }
 
-/// The saved token, or a new one written 0600 when there is none, it is not
-/// a regular private file of ours holding a well-formed token, or `rotate`.
-/// Blocking I/O: call before the runtime starts (or in `spawn_blocking`).
-pub(crate) fn load_or_create_token(path: &Path, rotate: bool) -> io::Result<String> {
+/// The saved token, if `path` is a regular private file of ours holding a
+/// well-formed one. Blocking I/O.
+pub(crate) fn load_token(path: &Path) -> io::Result<Option<String>> {
     use std::os::unix::fs::MetadataExt;
+    let Ok(meta) = std::fs::symlink_metadata(path) else {
+        return Ok(None);
+    };
+    let private =
+        meta.file_type().is_file() && meta.mode() & 0o077 == 0 && meta.uid() == paths::uid();
+    if !private {
+        return Ok(None);
+    }
+    let text = std::fs::read_to_string(path)?;
+    let token = text.trim();
+    Ok(token_ok(token).then(|| token.to_owned()))
+}
+
+/// The saved token ([`load_token`]), or a new one written 0600 when there
+/// is none or `rotate`. Blocking I/O: call before the runtime starts (or in
+/// `spawn_blocking`).
+pub(crate) fn load_or_create_token(path: &Path, rotate: bool) -> io::Result<String> {
     if !rotate {
-        if let Ok(meta) = std::fs::symlink_metadata(path) {
-            let private = meta.file_type().is_file()
-                && meta.mode() & 0o077 == 0
-                && meta.uid() == paths::uid();
-            if private {
-                let text = std::fs::read_to_string(path)?;
-                let token = text.trim();
-                if token_ok(token) {
-                    return Ok(token.to_owned());
-                }
-            }
+        if let Some(token) = load_token(path)? {
+            return Ok(token);
         }
     }
     let token = random_hex(TOKEN_BYTES)?;
diff --git a/addons/web/src/hub_io.rs b/addons/web/src/hub_io.rs
index 364b542..018db65 100644
--- a/addons/web/src/hub_io.rs
+++ b/addons/web/src/hub_io.rs
@@ -2,6 +2,7 @@
 //! sessions), and the one writer of web-state.json.
 
 use std::path::PathBuf;
+use std::sync::atomic::{AtomicBool, Ordering};
 
 use tokio::sync::mpsc;
 
@@ -12,6 +13,17 @@ use claudio_core::freshness::DaemonCheck;
 use claudio_core::paths;
 use claudio_core::proto::{Msg, SessionInfo};
 
+/// Set once by `claudio web --service`, before the runtime starts: the
+/// daemon that runs us is the authority, so `local` is connected as it is,
+/// never started or replaced (`client::ensure_local_daemon`).
+static PLAIN_LOCAL: AtomicBool = AtomicBool::new(false);
+
+/// `--service`: from now on `local` is reached with plain `client::connect`
+/// (and the hub's reconnects).
+pub(crate) fn connect_local_plainly() {
+    PLAIN_LOCAL.store(true, Ordering::Relaxed);
+}
+
 /// Connect `host` and list its sessions. The error says whether the
 /// bootstrap failed (the next attempt runs it again).
 pub(super) async fn connect_once(
@@ -19,9 +31,12 @@ pub(super) async fn connect_once(
     bootstrap: bool,
 ) -> Result<(Client, Vec<SessionInfo>), (String, bool)> {
     let client = if host == "local" {
-        let check = client::ensure_local_daemon()
-            .await
-            .map_err(|e| (e.to_string(), false))?;
+        let check = match PLAIN_LOCAL.load(Ordering::Relaxed) {
+            true => DaemonCheck::UpToDate,
+            false => client::ensure_local_daemon()
+                .await
+                .map_err(|e| (e.to_string(), false))?,
+        };
         let client = client::connect(&paths::daemon_socket())
             .await
             .map_err(|e| (e.to_string(), false))?;
diff --git a/addons/web/src/lib.rs b/addons/web/src/lib.rs
index db2b2fb..0ff8e61 100644
--- a/addons/web/src/lib.rs
+++ b/addons/web/src/lib.rs
@@ -80,6 +80,9 @@ pub(crate) struct WebArgs {
     pub set_password: bool,
     /// `--foreground`: serve from this terminal, as `claudio web` always did.
     pub foreground: bool,
+    /// `--service`: the child the daemon runs (docs/daemon-protocol.md,
+    /// "Addon services"): `ready` on stdout, never the token, stops at stdin EOF.
+    pub service: bool,
 }
 
 /// Shared by every handler.
@@ -119,6 +122,7 @@ pub(crate) fn parse_args(args: &[String]) -> Result<WebArgs, String> {
         rotate_token: false,
         set_password: false,
         foreground: false,
+        service: false,
     };
     let mut it = args.iter().peekable();
     while let Some(arg) = it.next() {
@@ -153,6 +157,7 @@ pub(crate) fn parse_args(args: &[String]) -> Result<WebArgs, String> {
             "--rotate-token" => out.rotate_token = true,
             "--set-password" => out.set_password = true,
             "--foreground" => out.foreground = true,
+            "--service" => out.service = true,
             other => return Err(format!("unknown argument {other:?}\n{USAGE}")),
         }
     }
@@ -162,6 +167,10 @@ pub(crate) fn parse_args(args: &[String]) -> Result<WebArgs, String> {
 
 /// The flags that do not go together.
 fn combination_ok(a: &WebArgs, n_args: usize) -> Result<(), String> {
+    let one_shot = a.no_open || a.print_url || a.rotate_token || a.rotate_cert;
+    if a.service && (one_shot || a.set_password || a.print_cert || a.foreground) {
+        return Err("--service takes only --listen=, --remote= and --allow-host=".to_owned());
+    }
     if (a.set_password || a.print_cert) && n_args != 1 {
         return Err("--set-password and --print-cert take no other argument".to_owned());
     }
@@ -259,22 +268,26 @@ pub fn run(args: &[String], addons: Arc<claudio_core::addon::Addons>) -> ExitCod
     if let Some(code) = one_shot(&args) {
         return code;
     }
+    serve_here(args, addons)
+}
+
+/// Serve from this process: `claudio web --foreground` (and, until the daemon
+/// serves it, a bare `claudio web`), or `--service`, the daemon's child.
+fn serve_here(args: WebArgs, addons: Arc<claudio_core::addon::Addons>) -> ExitCode {
+    // First thing: a daemon that stops us (or dies) during the start is obeyed.
+    let eof = args.service.then(watch_stdin);
     // Blocking I/O, before the runtime exists.
-    let token = match auth::load_or_create_token(&auth::token_path(), args.rotate_token) {
+    let token = match token_for(&args) {
         Ok(token) => token,
         Err(e) => {
-            eprintln!("claudio web: cannot read or write the token file: {e}");
+            eprintln!("claudio web: {e}");
             return ExitCode::FAILURE;
         }
     };
-    let url = match args.remote {
-        // `SocketAddr`'s `Display` brackets an IPv6 address.
-        None => format!("http://{}/#t={token}", args.listen),
-        Some(addr) => remote::token_url(addr, &token)
-            .unwrap_or_else(|| remote::public_url(addr, &claudio_core::sys::hostname())),
-    };
+    // `--service` never prints the token, so it never makes this URL.
+    let url = (!args.service).then(|| start_url(&args, &token));
     if args.print_url {
-        println!("{url}");
+        println!("{}", url.unwrap_or_default());
         return ExitCode::SUCCESS;
     }
     let password = password::read(&password::path());
@@ -290,19 +303,7 @@ pub fn run(args: &[String], addons: Arc<claudio_core::addon::Addons>) -> ExitCod
         }
     };
     let sessions = load_sessions(&args, &token, &password);
-
-    // A fixed filter, not RUST_LOG: a dependency's trace output could carry
-    // request headers (the cookie).
-    let debug = std::env::var("CLAUDIO_DEBUG").is_ok_and(|v| v == "1");
-    tracing_subscriber::fmt()
-        .with_writer(io::stderr)
-        .with_ansi(io::stderr().is_terminal())
-        .with_env_filter(EnvFilter::new(if debug {
-            "claudio=debug"
-        } else {
-            "claudio=info"
-        }))
-        .init();
+    init_tracing();
     let runtime = match tokio::runtime::Builder::new_multi_thread()
         .enable_all()
         .build()
@@ -313,7 +314,10 @@ pub fn run(args: &[String], addons: Arc<claudio_core::addon::Addons>) -> ExitCod
             return ExitCode::FAILURE;
         }
     };
-    match runtime.block_on(serve(args, token, url, remote, sessions, addons)) {
+    if args.service {
+        hub_io::connect_local_plainly();
+    }
+    match runtime.block_on(serve(args, token, url, remote, sessions, eof, addons)) {
         Ok(()) => ExitCode::SUCCESS,
         Err(e) => {
             eprintln!("claudio web: {e}");
@@ -322,6 +326,83 @@ pub fn run(args: &[String], addons: Arc<claudio_core::addon::Addons>) -> ExitCod
     }
 }
 
+/// The token this server serves with. `--service` only loads it (`claudio
+/// web` made it before it had the service started); a foreground start
+/// makes one when there is none, or a new one with `--rotate-token`.
+fn token_for(args: &WebArgs) -> Result<String, String> {
+    let path = auth::token_path();
+    if !args.service {
+        return auth::load_or_create_token(&path, args.rotate_token)
+            .map_err(|e| format!("cannot read or write the token file: {e}"));
+    }
+    match auth::load_token(&path) {
+        Ok(Some(token)) => Ok(token),
+        Ok(None) => Err(format!("no token in {}: run claudio web", path.display())),
+        Err(e) => Err(format!("cannot read the token file: {e}")),
+    }
+}
+
+/// The URL a start prints and opens: the token link, or with `--remote`
+/// the one for this machine (else the public one).
+pub(crate) fn start_url(args: &WebArgs, token: &str) -> String {
+    match args.remote {
+        // `SocketAddr`'s `Display` brackets an IPv6 address.
+        None => format!("http://{}/#t={token}", args.listen),
+        Some(addr) => remote::token_url(addr, token)
+            .unwrap_or_else(|| remote::public_url(addr, &claudio_core::sys::hostname())),
+    }
+}
+
+/// Logs on stderr. A fixed filter, not RUST_LOG: a dependency's trace
+/// output could carry request headers (the cookie).
+fn init_tracing() {
+    let debug = std::env::var("CLAUDIO_DEBUG").is_ok_and(|v| v == "1");
+    tracing_subscriber::fmt()
+        .with_writer(io::stderr)
+        .with_ansi(io::stderr().is_terminal())
+        .with_env_filter(EnvFilter::new(if debug {
+            "claudio=debug"
+        } else {
+            "claudio=info"
+        }))
+        .init();
+}
+
+/// How long `--service` may take to stop after stdin's EOF before it exits.
+const STOP_DEADLINE: std::time::Duration = std::time::Duration::from_secs(3);
+
+/// `--service`: a thread (not a task: a blocked runtime must not stop it)
+/// reads stdin, a pipe the daemon holds and never writes. EOF means the
+/// daemon is gone or asks us to stop: the graceful shutdown starts, and 3 s
+/// later the process exits if it still runs, so nothing stays on the port.
+fn watch_stdin() -> tokio::sync::oneshot::Receiver<()> {
+    let (tx, rx) = tokio::sync::oneshot::channel();
+    std::thread::spawn(move || {
+        let _ = io::copy(&mut io::stdin().lock(), &mut io::sink());
+        if test_hang() {
+            return;
+        }
+        let _ = tx.send(());
+        std::thread::sleep(STOP_DEADLINE);
+        std::process::exit(0);
+    });
+    rx
+}
+
+/// `CLAUDIO_TEST_SERVICE_HANG=1`, debug builds only: a `--service` that
+/// ignores its stop (stdin's EOF and SIGTERM), so the suites can check the
+/// daemon's SIGKILL. A release build has no such body: the variable is not
+/// read, and `make static`'s final check greps for its name.
+#[cfg(debug_assertions)]
+fn test_hang() -> bool {
+    std::env::var("CLAUDIO_TEST_SERVICE_HANG").is_ok_and(|v| v == "1")
+}
+
+#[cfg(not(debug_assertions))]
+fn test_hang() -> bool {
+    false
+}
+
 /// The sign-ins of earlier runs, bound to this start's mode, token and
 /// password bytes (a file of another epoch: none, silently). Blocking I/O,
 /// before the runtime exists and before `serve` binds.
@@ -378,12 +459,15 @@ fn print_cert() -> ExitCode {
     }
 }
 
+/// Serve until Ctrl+C, SIGTERM or (`--service`) stdin's EOF. `url`: what a
+/// foreground start prints and opens; `None` for `--service`.
 async fn serve(
     args: WebArgs,
     token: String,
-    url: String,
+    url: Option<String>,
     remote: Option<remote::Remote>,
     sessions: sessions::Sessions,
+    eof: Option<tokio::sync::oneshot::Receiver<()>>,
     addons: Arc<claudio_core::addon::Addons>,
 ) -> Result<(), String> {
     let listen = remote.as_ref().map_or(args.listen, |r| r.addr);
@@ -409,24 +493,13 @@ async fn serve(
         Arc::clone(&state.auth.sessions),
     );
     let writer = sessions.start_writer();
-    if !env!("CLAUDIO_BUILD_LABEL").is_empty() {
-        eprintln!("claudio web: local build {}", version());
-    }
-    announce(&url, remote.as_ref());
-    let wanted = claudio_core::app::browser::browser_wanted(
-        std::env::consts::OS,
-        |k| std::env::var(k).ok(),
-        io::stdout().is_terminal(),
-    );
-    if !args.no_open && wanted {
-        claudio_core::app::browser::open_browser(&url);
-    }
+    started(&args, url.as_deref(), remote.as_ref());
     tracing::info!(listen = %listen, tls = remote.is_some(), "web: serving");
 
     // Tabs are closed (1001), web-state.json and the sessions written
     // before the server stops waiting for its connections.
     let stop = async move {
-        shutdown().await;
+        shutdown(eof).await;
         hub.shutdown().await;
         sessions.flush().await;
         writer.inspect(|w| w.abort());
@@ -451,6 +524,31 @@ async fn serve(
     served.map_err(|e| format!("server error: {e}"))
 }
 
+/// Once the listener is bound and the hub runs: `--service` (no `url`)
+/// writes `ready` on stdout, the line the daemon waits for, and nothing
+/// else (never the token); a foreground start prints its URL (and banner)
+/// and opens a browser.
+fn started(args: &WebArgs, url: Option<&str>, remote: Option<&remote::Remote>) {
+    if !env!("CLAUDIO_BUILD_LABEL").is_empty() {
+        eprintln!("claudio web: local build {}", version());
+    }
+    let Some(url) = url else {
+        let mut out = io::stdout();
+        let _ = writeln!(out, "ready");
+        let _ = out.flush();
+        return;
+    };
+    announce(url, remote);
+    let wanted = claudio_core::app::browser::browser_wanted(
+        std::env::consts::OS,
+        |k| std::env::var(k).ok(),
+        io::stdout().is_terminal(),
+    );
+    if !args.no_open && wanted {
+        claudio_core::app::browser::open_browser(url);
+    }
+}
+
 /// The start lines: with `--remote` the banner's stderr lines first, then
 /// the one stdout line the tests read.
 fn announce(url: &str, remote: Option<&remote::Remote>) {
@@ -520,9 +618,22 @@ async fn build_state(
     })
 }
 
-/// Ctrl+C or SIGTERM.
-async fn shutdown() {
+/// Ctrl+C, SIGTERM, or `eof` (`--service`: stdin closed).
+async fn shutdown(eof: Option<tokio::sync::oneshot::Receiver<()>>) {
     use tokio::signal::unix::{signal, SignalKind};
+    if eof.is_some() && test_hang() {
+        // SIGTERM is caught and never acted on; only SIGKILL ends us.
+        let _term = signal(SignalKind::terminate());
+        std::future::pending::<()>().await;
+    }
+    let eof = async {
+        match eof {
+            Some(rx) => {
+                let _ = rx.await;
+            }
+            None => std::future::pending::<()>().await,
+        }
+    };
     let term = async {
         match signal(SignalKind::terminate()) {
             Ok(mut s) => {
@@ -534,6 +645,7 @@ async fn shutdown() {
     tokio::select! {
         _ = tokio::signal::ctrl_c() => {}
         _ = term => {}
+        _ = eof => {}
     }
     tracing::info!("web: shutting down");
 }
`````

Run: `cargo test --locked -p claudio-web --lib`, then
`cargo test --locked --test web -- --test-threads=1`, then the `make check` sequence.
Expected: `test result: ok. 108 passed; 0 failed`; the web suite `ok. 40 passed`;
`rules: 0 problem(s)`.

Step 3: commit
`git commit -m "feat(web): claudio web --service, the child the daemon runs" -- addons/web/src/lib.rs addons/web/src/hub_io.rs addons/web/src/auth.rs Makefile addons/web/tests/web.rs`

Interfaces this task provides to later tasks: `WebArgs.service: bool` (`--service`);
`auth::load_token(&Path) -> io::Result<Option<String>>`; `lib.rs::token_for(&WebArgs) ->
Result<String, String>`;
`lib.rs::{start_url(&WebArgs, token: &str) -> String, announce(url, Option<&remote::Remote>)}`
(made `pub(crate)` in Task 13); `hub_io::connect_local_plainly()`; the service contract
(`ready` line; stdin EOF stops it) that Task 12's `service()` argv starts.

### Task 12: The web addon's service from [web], and its apply and status ops
Model: opus   Repo: /home/p4u/claudio   Files: `addons/web/src/service.rs` (new),
`addons/web/src/addon.rs`, `addons/web/src/addon/tests.rs`, `addons/web/src/lib.rs`,
`addons/web/Cargo.toml`
Goal: spec §2's `service()` and ops (AC1's mapping half: `[web]` ↔ argv, an invalid section →
an error naming the key; AC7's `addon:web` feature): `WebAddon` declares `[web]` and implements
`DaemonOps`. `service()` checks `[web]` with the flags' own checks and answers
`web --service --listen=…|--remote=… [--allow-host=…]…` while `enabled = true`; `apply`
(`{"force": bool}`) calls `service_reload`, `status` calls `service_state`; both answer
`service::reply`. A disabled section is `Ok(None)` without further checks (Spec issue 6).
`toml` moves from claudio-web's dev-dependencies to its dependencies (already in `Cargo.lock`).

Step 1: write the failing tests (the section and reply tests; a real daemon running a fake
claudio from `[web]`; `mod service;`)

Patch `task12-step1` (`addons/web/src/addon/tests.rs`, `addons/web/src/lib.rs`, `addons/web/src/service.rs`):

`````diff
diff --git a/addons/web/src/addon/tests.rs b/addons/web/src/addon/tests.rs
index 523fcc1..4a4afac 100644
--- a/addons/web/src/addon/tests.rs
+++ b/addons/web/src/addon/tests.rs
@@ -161,3 +161,60 @@ fn web_claims_its_word_only() {
         Some(ExitCode::SUCCESS)
     );
 }
+
+/// The web addon in a real daemon: `[web] enabled` in the test's
+/// config.toml makes the daemon run its claudio (a fake that records its
+/// argv and says `ready`) as `web --service …`; the ops answer the state.
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn the_daemon_runs_web_service_from_the_web_section() {
+    use claudio_core::daemon::test_support::{write_executable, Client, TestDaemon};
+    use claudio_core::daemon::Config;
+    use claudio_core::proto::Msg;
+    let dir = TestDaemon::new_dir();
+    let fake = dir.join("fake-claudio");
+    let script = "#!/bin/sh\n[ \"$1\" = --version ] && exit 0\n\
+                  printf '%s\\n' \"$@\" > \"$(dirname \"$0\")/argv\"\necho ready\nexec cat > /dev/null\n";
+    write_executable(&fake, script);
+    std::fs::write(dir.join("config.toml"), "[web]\nenabled = true\n").unwrap();
+    let config = Config {
+        claudio: fake,
+        config_file: Some(dir.join("config.toml")),
+        ..TestDaemon::config(&dir)
+    };
+    let addons = Addons::new(vec![Arc::new(WebAddon)]).unwrap();
+    let d = TestDaemon::start_with_addons(dir, config, addons);
+    let mut c = Client::connect(&d.config.socket).await;
+    let op = |op: &str, body: Value| Msg::AddonOp {
+        addon: "web".into(),
+        op: op.into(),
+        body,
+    };
+    let running = |m: &Msg| matches!(m, Msg::AddonReply { body } if body["state"] == "running");
+    let mut reply = c.call(op("status", serde_json::json!({}))).await;
+    for _ in 0..200 {
+        if running(&reply) {
+            break;
+        }
+        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
+        reply = c.call(op("status", serde_json::json!({}))).await;
+    }
+    let Msg::AddonReply { body } = reply else {
+        panic!("{reply:?}");
+    };
+    assert_eq!(
+        body["argv"],
+        serde_json::json!(["web", "--service", "--listen=127.0.0.1:7337"])
+    );
+    let argv = std::fs::read_to_string(d.dir.join("argv")).unwrap();
+    assert_eq!(argv, "web\n--service\n--listen=127.0.0.1:7337\n");
+    std::fs::write(d.dir.join("config.toml"), "[web]\nenabled = false\n").unwrap();
+    let off = c
+        .call(op("apply", serde_json::json!({"force": false})))
+        .await;
+    assert_eq!(
+        off,
+        Msg::AddonReply {
+            body: serde_json::json!({"state": "off"})
+        }
+    );
+}
diff --git a/addons/web/src/lib.rs b/addons/web/src/lib.rs
index 0ff8e61..0fb12ad 100644
--- a/addons/web/src/lib.rs
+++ b/addons/web/src/lib.rs
@@ -21,6 +21,7 @@ mod login;
 mod password;
 mod peer;
 mod remote;
+mod service;
 mod session_events;
 mod sessions;
 mod spawn;
diff --git a/addons/web/src/service.rs b/addons/web/src/service.rs
new file mode 100644
index 0000000..f31cc4a
--- /dev/null
+++ b/addons/web/src/service.rs
@@ -0,0 +1,113 @@
+#[cfg(test)]
+mod tests {
+    use super::*;
+    use std::time::Duration;
+
+    fn table(text: &str) -> Table {
+        text.parse().unwrap()
+    }
+
+    fn words(list: &[&str]) -> Vec<String> {
+        list.iter().map(|s| s.to_string()).collect()
+    }
+
+    #[test]
+    fn an_enabled_section_is_the_service_argv() {
+        let argv = |text: &str| argv(&table(text)).unwrap();
+        assert_eq!(argv(""), None);
+        assert_eq!(argv("enabled = false\nlisten = \"0.0.0.0:1\""), None);
+        assert_eq!(
+            argv("enabled = true"),
+            Some(words(&["web", "--service", "--listen=127.0.0.1:7337"]))
+        );
+        assert_eq!(
+            argv("enabled = true\nlisten = \"localhost:9000\"\nallow_hosts = [\"A.Example\"]"),
+            Some(words(&[
+                "web",
+                "--service",
+                "--listen=127.0.0.1:9000",
+                "--allow-host=a.example"
+            ]))
+        );
+        assert_eq!(
+            argv("enabled = true\nremote = \"[::]:8443\"\nallow_hosts = [\"b.example:8443\"]"),
+            Some(words(&[
+                "web",
+                "--service",
+                "--remote=[::]:8443",
+                "--allow-host=b.example:8443"
+            ]))
+        );
+    }
+
+    #[test]
+    fn every_argv_parses_back_to_its_unit() {
+        for text in [
+            "enabled = true",
+            "enabled = true\nlisten = \"127.0.0.2:80\"\nallow_hosts = [\"x.example\"]",
+            "enabled = true\nremote = \":7999\"\nallow_hosts = [\"a.example:7999\", \"b.example\"]",
+            "enabled = true\nremote = \"[::1]\"",
+        ] {
+            let unit = unit(&table(text)).unwrap().unwrap_or_default();
+            let argv = argv(&table(text)).unwrap().unwrap();
+            let back = parse_args(&argv[1..]).unwrap();
+            assert!(back.service, "{text}");
+            let addr = back.remote.unwrap_or(back.listen);
+            assert_eq!((back.remote.is_some(), addr), (unit.remote, unit.addr));
+            assert_eq!(back.allow_hosts, unit.allow_hosts, "{text}");
+        }
+    }
+
+    #[test]
+    fn a_bad_section_names_its_key() {
+        let err = |text: &str| argv(&table(&format!("enabled = true\n{text}"))).unwrap_err();
+        assert_eq!(
+            argv(&table("enabled = \"yes\"")).unwrap_err(),
+            "[web] enabled: not true or false"
+        );
+        let e = err("listen = \"0.0.0.0:7337\"");
+        assert!(
+            e.starts_with("[web] listen: refusing to listen on 0.0.0.0:7337"),
+            "{e}"
+        );
+        assert_eq!(err("listen = 7337"), "[web] listen: not a string");
+        assert!(err("remote = \"fe80::1\"").starts_with("[web] remote: "));
+        let e = err("allow_hosts = [\"bad/host\"]");
+        assert_eq!(e, "[web] allow_hosts: \"bad/host\" is not a host name");
+        assert!(err("allow_hosts = \"a.example\"").starts_with("[web] allow_hosts: "));
+        let e = err("listen = \"127.0.0.1:1\"\nremote = \":2\"");
+        assert_eq!(e, "[web] listen, remote: one or the other");
+        let e = err("remote = \":7999\"\nallow_hosts = [\"a.example:8443\"]");
+        assert!(
+            e.starts_with("[web] remote, allow_hosts: ") && e.contains("one port"),
+            "{e}"
+        );
+    }
+
+    #[test]
+    fn replies_carry_the_state_and_no_more() {
+        let running = ServiceState::Running {
+            pid: 42,
+            since: 1_800_000_000,
+            argv: words(&["web", "--service", "--listen=127.0.0.1:7337"]),
+        };
+        assert_eq!(
+            reply(&running),
+            json!({"state": "running", "pid": 42, "since": 1_800_000_000u64,
+                   "argv": ["web", "--service", "--listen=127.0.0.1:7337"]})
+        );
+        let failed = ServiceState::Failed {
+            exit: Some(1),
+            signal: None,
+            tail: "cannot listen".into(),
+            retry_in: Some(Duration::from_millis(1500)),
+        };
+        assert_eq!(
+            reply(&failed),
+            json!({"state": "failed", "exit": 1, "signal": null, "tail": "cannot listen",
+                   "retry_in": 1})
+        );
+        assert_eq!(reply(&ServiceState::Off), json!({"state": "off"}));
+        assert_eq!(reply(&ServiceState::Starting), json!({"state": "starting"}));
+    }
+}
`````

Run: `cargo test --locked -p claudio-web --lib`
Expected: compilation fails in `service.rs`'s tests (`` error[E0425]: cannot find function `argv` in this scope ``, …).

Step 2: implement

Patch `task12-step2` (`addons/web/Cargo.toml`, `addons/web/src/addon.rs`, `addons/web/src/lib.rs`, `addons/web/src/service.rs`):

`````diff
diff --git a/addons/web/Cargo.toml b/addons/web/Cargo.toml
index a60067e..39b5861 100644
--- a/addons/web/Cargo.toml
+++ b/addons/web/Cargo.toml
@@ -24,6 +24,8 @@ serde_json.workspace = true
 sha2.workspace = true
 tokio.workspace = true
 tokio-rustls.workspace = true
+# `[web]`, the service's config section (`DaemonOps::service`).
+toml.workspace = true
 tracing.workspace = true
 tracing-subscriber.workspace = true
 uuid.workspace = true
@@ -40,4 +42,3 @@ claudio-core = { workspace = true, features = ["test-support"] }
 reqwest.workspace = true
 # login.rs's paused-clock test (`start_paused`).
 tokio = { workspace = true, features = ["test-util"] }
-toml.workspace = true
diff --git a/addons/web/src/addon.rs b/addons/web/src/addon.rs
index 41046ee..f0c96e5 100644
--- a/addons/web/src/addon.rs
+++ b/addons/web/src/addon.rs
@@ -2,16 +2,19 @@
 //! its `Launch` asks every claude session, from any front-end, for the
 //! `<latex>` and ```` ```mermaid ```` lenses the page renders (G5), while
 //! `[extensions] latex`/`diagram` are on; its `Ui` lists those two rows on
-//! the extensions screens. The toggles are the core's (D10).
+//! the extensions screens (the toggles are the core's, D10). Its
+//! `DaemonOps` make the web a daemon service while `[web] enabled`: the ops
+//! `apply` and `status` (docs/addons.md, "The web addon").
 
 use std::process::ExitCode;
 use std::sync::Arc;
 
 use claudio_core::addon::{
-    Addon, AddonCtx, Addons, BoxFuture, Commands, ExtensionRow, Launch, LaunchError, LaunchPart,
-    LaunchRequest, LaunchSelect, Notice, Routing, RowAction, Ui,
+    Addon, AddonCtx, Addons, BoxFuture, Commands, DaemonCtx, DaemonOps, ExtensionRow, Launch,
+    LaunchError, LaunchPart, LaunchRequest, LaunchSelect, Notice, Routing, RowAction, Ui,
 };
 use claudio_core::proto::{LensKind, LensSpec};
+use serde_json::Value;
 
 /// The diagram lens's instruction (spec §7).
 pub const DIAGRAM_CONTEXT: &str = "This terminal draws Mermaid diagrams. When a diagram \
@@ -33,6 +36,10 @@ impl Addon for WebAddon {
         "web"
     }
 
+    fn config_sections(&self) -> &'static [&'static str] {
+        &["web"]
+    }
+
     fn commands(&self) -> Option<&dyn Commands> {
         Some(self)
     }
@@ -44,6 +51,36 @@ impl Addon for WebAddon {
     fn ui(&self) -> Option<&dyn Ui> {
         Some(self)
     }
+
+    fn daemon(&self) -> Option<&dyn DaemonOps> {
+        Some(self)
+    }
+}
+
+impl DaemonOps for WebAddon {
+    /// `apply` (`{"force": bool}`) re-reads `[web]` and converges;
+    /// `status` (`{}`) reads the state. Both answer `service::reply`.
+    fn handle<'a>(
+        &'a self,
+        op: &'a str,
+        body: Value,
+        ctx: Arc<dyn DaemonCtx>,
+    ) -> BoxFuture<'a, Result<Value, String>> {
+        Box::pin(async move {
+            match op {
+                "apply" => {
+                    let force = body.get("force").and_then(Value::as_bool) == Some(true);
+                    Ok(super::service::reply(&ctx.service_reload(force).await))
+                }
+                "status" => Ok(super::service::reply(&ctx.service_state())),
+                other => Err(format!("unknown op {other}")),
+            }
+        })
+    }
+
+    fn service(&self, config: &dyn Fn(&str) -> toml::Table) -> Result<Option<Vec<String>>, String> {
+        super::service::argv(&config("web"))
+    }
 }
 
 impl Commands for WebAddon {
diff --git a/addons/web/src/lib.rs b/addons/web/src/lib.rs
index 0fb12ad..fced30c 100644
--- a/addons/web/src/lib.rs
+++ b/addons/web/src/lib.rs
@@ -208,7 +208,7 @@ fn value(
 
 /// `[A-Za-z0-9.-]{1,253}`, not starting with `-`, optionally `:<port>`
 /// (1..=65535).
-fn host_name_ok(name: &str) -> bool {
+pub(crate) fn host_name_ok(name: &str) -> bool {
     let (host, port) = match name.rsplit_once(':') {
         Some((h, p)) => (h, Some(p)),
         None => (name, None),
diff --git a/addons/web/src/service.rs b/addons/web/src/service.rs
index f31cc4a..b245555 100644
--- a/addons/web/src/service.rs
+++ b/addons/web/src/service.rs
@@ -1,3 +1,146 @@
+//! `claudio web` as a daemon service (docs/addons.md, "The web addon";
+//! docs/web.md, "The web service"): `[web]` in config.toml, the argv the
+//! daemon runs from it (`WebAddon::service`), and the one reply of the
+//! `apply` and `status` ops. Pure: the CLI (`cli.rs`) does the I/O.
+
+use std::net::SocketAddr;
+
+use claudio_core::addon::ServiceState;
+use serde_json::{json, Value};
+use toml::Table;
+
+use crate::{host_name_ok, listen_addr, parse_args, remote, DEFAULT_PORT};
+
+/// What `[web]` stores as one unit: the listener and its allowed hosts.
+#[derive(Debug, Clone, PartialEq, Eq)]
+pub(crate) struct Unit {
+    /// `remote = "…"` (TLS, a password) when true, else `listen = "…"`.
+    pub remote: bool,
+    pub addr: SocketAddr,
+    pub allow_hosts: Vec<String>,
+}
+
+impl Default for Unit {
+    /// `--listen 127.0.0.1:7337`, no allowed host.
+    fn default() -> Unit {
+        Unit {
+            remote: false,
+            addr: SocketAddr::from(([127, 0, 0, 1], DEFAULT_PORT)),
+            allow_hosts: Vec::new(),
+        }
+    }
+}
+
+impl Unit {
+    /// Its flags, each value in its flag's word (`--listen=…`): no value
+    /// can be read as another flag.
+    pub(crate) fn flags(&self) -> Vec<String> {
+        let listener = match self.remote {
+            true => format!("--remote={}", self.addr),
+            false => format!("--listen={}", self.addr),
+        };
+        let hosts = self.allow_hosts.iter().map(|h| format!("--allow-host={h}"));
+        std::iter::once(listener).chain(hosts).collect()
+    }
+}
+
+/// `[web] enabled` (absent: false).
+pub(crate) fn enabled(t: &Table) -> Result<bool, String> {
+    match t.get("enabled") {
+        None => Ok(false),
+        Some(toml::Value::Boolean(b)) => Ok(*b),
+        Some(_) => Err("[web] enabled: not true or false".to_owned()),
+    }
+}
+
+/// The unit `[web]` stores (`None`: no `listen`, `remote` or
+/// `allow_hosts`), checked as the flags are (`listen_addr`,
+/// `remote_addr`, `host_name_ok`, `combination_ok`, the value types); an
+/// error names the key.
+pub(crate) fn unit(t: &Table) -> Result<Option<Unit>, String> {
+    let text = |key: &str| match t.get(key) {
+        None => Ok(None),
+        Some(toml::Value::String(s)) => Ok(Some(s.clone())),
+        Some(_) => Err(format!("[web] {key}: not a string")),
+    };
+    let named = |key: &'static str| move |e: String| format!("[web] {key}: {e}");
+    let listen = text("listen")?
+        .map(|s| listen_addr(&s).map_err(named("listen")))
+        .transpose()?;
+    let remote = text("remote")?
+        .map(|s| remote::remote_addr(&s).map_err(named("remote")))
+        .transpose()?;
+    let allow_hosts: Vec<String> = match t.get("allow_hosts") {
+        None => Vec::new(),
+        Some(toml::Value::Array(hosts)) => hosts
+            .iter()
+            .map(|h| match h.as_str() {
+                Some(h) if host_name_ok(h) => Ok(h.to_ascii_lowercase()),
+                _ => Err(format!("[web] allow_hosts: {h} is not a host name")),
+            })
+            .collect::<Result<_, _>>()?,
+        Some(_) => return Err("[web] allow_hosts: not a list of host names".to_owned()),
+    };
+    let unit = match (listen, remote) {
+        (Some(_), Some(_)) => return Err("[web] listen, remote: one or the other".to_owned()),
+        (None, None) if allow_hosts.is_empty() => return Ok(None),
+        (None, Some(addr)) => Unit {
+            remote: true,
+            addr,
+            allow_hosts,
+        },
+        (listen, None) => Unit {
+            addr: listen.unwrap_or(Unit::default().addr),
+            allow_hosts,
+            ..Unit::default()
+        },
+    };
+    // The combination rules, as the flags have them (the one port with --remote).
+    parse_args(&unit.flags()).map_err(|e| format!("[web] {}: {e}", unit_keys(&unit)))?;
+    Ok(Some(unit))
+}
+
+fn unit_keys(unit: &Unit) -> &'static str {
+    match unit.remote {
+        true => "remote, allow_hosts",
+        false => "listen, allow_hosts",
+    }
+}
+
+/// `WebAddon::service`: `web --service <flags>` while `[web] enabled`. A
+/// disabled section is not checked further: off is always possible.
+pub(crate) fn argv(t: &Table) -> Result<Option<Vec<String>>, String> {
+    if !enabled(t)? {
+        return Ok(None);
+    }
+    let unit = unit(t)?.unwrap_or_default();
+    let head = ["web", "--service"].map(String::from);
+    Ok(Some(head.into_iter().chain(unit.flags()).collect()))
+}
+
+/// The one reply of `apply` and `status` (docs/addons.md, "The web addon").
+pub(crate) fn reply(state: &ServiceState) -> Value {
+    match state {
+        ServiceState::Off => json!({"state": "off"}),
+        ServiceState::Starting => json!({"state": "starting"}),
+        ServiceState::Running { pid, since, argv } => {
+            json!({"state": "running", "pid": pid, "since": since, "argv": argv})
+        }
+        ServiceState::Failed {
+            exit,
+            signal,
+            tail,
+            retry_in,
+        } => json!({
+            "state": "failed",
+            "exit": exit,
+            "signal": signal,
+            "tail": tail,
+            "retry_in": retry_in.map(|d| d.as_secs()),
+        }),
+    }
+}
+
 #[cfg(test)]
 mod tests {
     use super::*;
`````

Run: `cargo test --locked -p claudio-web --lib`, then
`cargo test --locked --test web --test vocdoni_web --test web_manager -- --test-threads=1`,
then the `make check` sequence and `make matrix` (or their fallbacks).
Expected: `test result: ok. 113 passed; 0 failed`; vocdoni_web `ok. 13 passed`, web
`ok. 40 passed`, web_manager `ok. 4 passed`; `rules: 0 problem(s)`; every matrix cell passes.

Step 3: commit
`git add addons/web/src/service.rs && git commit -m "feat(web): the web addon's service from [web], and its apply and status ops" -m "toml becomes a normal dependency of claudio-web (it was a dev-dependency): DaemonOps::service reads the [web] table." -- addons/web/src/service.rs addons/web/src/addon.rs addons/web/src/addon/tests.rs addons/web/src/lib.rs addons/web/Cargo.toml`

Interfaces this task provides to later tasks: `service::{Unit { remote: bool, addr:
SocketAddr, allow_hosts: Vec<String> }, Unit::flags(&self) -> Vec<String>, enabled(&Table) ->
Result<bool, String>, unit(&Table) -> Result<Option<Unit>, String>, argv(&Table) ->
Result<Option<Vec<String>>, String>, reply(&ServiceState) -> Value}`; `lib.rs::host_name_ok` is
`pub(crate)`; the daemon's `Welcome.features` lists `addon:web`.

### Task 13: claudio web hands the server to the daemon; stop and status
Model: opus   Repo: /home/p4u/claudio   Files: `addons/web/src/cli.rs` (new),
`addons/web/src/service.rs`, `addons/web/src/lib.rs`, `addons/web/src/tls/cert.rs`,
`addons/web/tests/web.rs`, `addons/web/tests/web_harness/service.rs` (new),
`addons/web/tests/web_harness/mod.rs`, `addons/web/tests/web_harness/tls.rs`
Goal: the CLI table (AC1's merge half, AC3, AC4, AC5, AC7, AC9, AC11): a bare `claudio web`
merges its listener flags with `[web]` (any of `--listen`, `--remote`, `--allow-host` replaces
the stored unit; none keeps it), checks the result, creates or rotates the token, runs
`remote::prepare` in the terminal for `--remote`, writes `[web]` with `enabled = true`
(`app::config_edit`), ensures the local daemon, sends `apply {force: rotate-token or
rotate-cert}`, and on `Running` prints what a foreground start prints (built from the reply's
argv) and opens the browser; on `Failed` it prints the tail and the log path and exits 1.
Without `addon:web` it sends nothing and prints the no-web-service message (exit 1, `[web]`
kept). `claudio web stop` (`enabled = false`, then `apply`) and `claudio web status` reach a
daemon only with plain `client::connect`. This commit makes bare `claudio web` hand off; Task
10 already moved every suite to `--foreground`.

The parse-time `--rotate-cert` rule moves (Spec issue 5). The harness gains `ServiceWeb`
(`web_harness/service.rs`): `claudio(...)` runs a command with a 60 s deadline (a command that
keeps serving fails the test instead of hanging it), `status`/`pid` parsing, and `finish()`:
`claudio web stop`, the daemon killed, every seen service pid gone within 5 s (AC11), no token
in `web.log` or `daemon.log` (AC5); `Drop` does the same without asserting. The AC3 test of a
service that ignores SIGTERM sets `CLAUDIO_TEST_SERVICE_HANG=1` on the `claudio web` that
starts the daemon (the daemon and its service inherit it; `claudio daemon restart` does not).

Step 1: write the failing tests

Patch `task13-step1` (`addons/web/src/lib.rs`, `addons/web/src/service.rs`, `addons/web/tests/web.rs`, `addons/web/tests/web_harness/mod.rs`, `addons/web/tests/web_harness/service.rs`, `addons/web/tests/web_harness/tls.rs`):

`````diff
diff --git a/addons/web/src/lib.rs b/addons/web/src/lib.rs
index fced30c..a81b1f9 100644
--- a/addons/web/src/lib.rs
+++ b/addons/web/src/lib.rs
@@ -759,9 +759,12 @@ mod tests {
         );
         let a = parse_args(&args(&["--remote", ":7999", "--rotate-cert"])).unwrap();
         assert!(a.rotate_cert);
+        // Without --foreground, --rotate-cert is checked on the merged
+        // settings (`service::run_args`), which may be a stored --remote.
+        assert!(parse_args(&args(&["--rotate-cert"])).is_ok());
         for bad in [
             &["--remote", "--listen", "127.0.0.1:1"][..],
-            &["--rotate-cert"][..],
+            &["--rotate-cert", "--foreground"][..],
             &["--remote", ":7999", "--allow-host", "a.example:8443"][..],
             &["--print-cert", "--no-open"][..],
             &["--remote", "fe80::1"][..],
diff --git a/addons/web/src/service.rs b/addons/web/src/service.rs
index b245555..54c6439 100644
--- a/addons/web/src/service.rs
+++ b/addons/web/src/service.rs
@@ -227,6 +227,102 @@ mod tests {
         );
     }
 
+    fn flags(list: &[&str]) -> WebArgs {
+        parse_args(&words(list)).unwrap()
+    }
+
+    #[test]
+    fn any_listener_flag_replaces_the_stored_unit_and_none_keeps_it() {
+        let stored = table("enabled = true\nremote = \":8443\"\nallow_hosts = [\"a.example\"]");
+        let kept = merge(&stored, &flags(&["--no-open"])).unwrap();
+        assert_eq!(kept, unit(&stored).unwrap().unwrap(), "none given: stored");
+        let hosts = merge(&stored, &flags(&["--allow-host", "b.example"])).unwrap();
+        assert_eq!(
+            hosts,
+            Unit {
+                allow_hosts: words(&["b.example"]),
+                ..Unit::default()
+            },
+            "--allow-host alone: the whole unit, the listener at its default"
+        );
+        let listen = merge(&stored, &flags(&["--listen", "127.0.0.1:9000"])).unwrap();
+        assert_eq!(
+            (
+                listen.remote,
+                listen.addr.to_string(),
+                listen.allow_hosts.len()
+            ),
+            (false, "127.0.0.1:9000".to_owned(), 0),
+            "hosts cleared too"
+        );
+        assert_eq!(merge(&table(""), &flags(&[])).unwrap(), Unit::default());
+        let bad = table("listen = \"0.0.0.0:1\"");
+        assert!(merge(&bad, &flags(&[]))
+            .unwrap_err()
+            .starts_with("[web] listen: "));
+        assert!(
+            merge(&bad, &flags(&["--listen", "127.0.0.1:1"])).is_ok(),
+            "replaced"
+        );
+    }
+
+    #[test]
+    fn run_args_check_the_merged_unit() {
+        let remote = unit(&table("remote = \":7999\"")).unwrap().unwrap();
+        let asked = flags(&["--rotate-cert", "--no-open", "--rotate-token"]);
+        let run = run_args(&remote, &asked).unwrap();
+        assert!(run.rotate_cert && run.no_open && run.rotate_token && !run.print_url);
+        assert_eq!(
+            run.remote.map(|a| a.to_string()),
+            Some("0.0.0.0:7999".into())
+        );
+        let err = run_args(&Unit::default(), &asked).unwrap_err();
+        assert!(err.contains("--rotate-cert needs --remote"), "{err}");
+        let foreign = Unit {
+            allow_hosts: words(&["a.example:8443"]),
+            ..remote
+        };
+        assert!(run_args(&foreign, &flags(&[]))
+            .unwrap_err()
+            .contains("one port"));
+    }
+
+    #[test]
+    fn replies_read_back_and_an_unknown_state_is_a_newer_daemon() {
+        let running = ServiceState::Running {
+            pid: 7,
+            since: 9,
+            argv: words(&["web", "--service", "--listen=127.0.0.1:1"]),
+        };
+        assert_eq!(
+            read_reply(&reply(&running)),
+            Reported::Running {
+                pid: 7,
+                since: 9,
+                argv: words(&["web", "--service", "--listen=127.0.0.1:1"])
+            }
+        );
+        let failed = ServiceState::Failed {
+            exit: None,
+            signal: Some(9),
+            tail: "killed".into(),
+            retry_in: None,
+        };
+        assert!(matches!(
+            read_reply(&reply(&failed)),
+            Reported::Failed {
+                signal: Some(9),
+                retry_in: None,
+                ..
+            }
+        ));
+        assert_eq!(read_reply(&reply(&ServiceState::Off)), Reported::Off);
+        let extra = json!({"state": "off", "later": true});
+        assert_eq!(read_reply(&extra), Reported::Off, "unknown fields ignored");
+        assert_eq!(read_reply(&json!({"state": "paused"})), Reported::Newer);
+        assert_eq!(read_reply(&json!("off")), Reported::Newer);
+    }
+
     #[test]
     fn replies_carry_the_state_and_no_more() {
         let running = ServiceState::Running {
diff --git a/addons/web/tests/web.rs b/addons/web/tests/web.rs
index 560962d..a7f6f35 100644
--- a/addons/web/tests/web.rs
+++ b/addons/web/tests/web.rs
@@ -15,6 +15,7 @@ use common::{ManagerHarness, BINARY, WAIT};
 use futures::SinkExt;
 use serde_json::{json, Value};
 use tokio_tungstenite::tungstenite::Message;
+use web_harness::service::*;
 use web_harness::tls::*;
 use web_harness::*;
 
@@ -361,6 +362,173 @@ fn web_service_says_ready_never_prints_the_token_and_stops_at_eof() {
     );
 }
 
+// ── The web service: claudio web hands its server to the daemon ─────────────
+
+/// AC3.
+#[test]
+fn web_hands_its_server_to_the_daemon_and_stop_takes_it_back() {
+    let mut svc = ServiceWeb::start(ManagerHarness::new(), &[], |_| {});
+    let cookie = svc.login();
+    assert_eq!(svc.me(&cookie), 204, "the printed URL logs in");
+    let (code, out) = svc.status();
+    assert_eq!(code, Some(0), "{out}");
+    assert!(out.contains("web: enabled"), "{out}");
+    assert!(
+        out.contains(&format!("url: http://127.0.0.1:{}/\n", svc.port)),
+        "{out}"
+    );
+    let first = pid_in(&out).expect("a pid");
+    let restart = svc.claudio(&["daemon", "restart"]);
+    assert!(restart.status.success(), "{restart:?}");
+    assert!(gone_within(first, std::time::Duration::from_secs(5)));
+    svc.running_other_than(Some(first), common::DAEMON_WAIT);
+    let stop = svc.claudio(&["web", "stop"]);
+    assert!(stop.status.success(), "{stop:?}");
+    assert!(
+        std::net::TcpListener::bind(("127.0.0.1", svc.port)).is_ok(),
+        "the port is closed"
+    );
+    let config = fs::read_to_string(svc.h().config_home.join("claudio/config.toml")).unwrap();
+    assert!(config.contains("[web]\nenabled = false"), "{config}");
+    let (code, out) = svc.status();
+    assert_eq!(code, Some(3), "{out}");
+    assert!(
+        out.contains("web: disabled") && out.contains("service: off"),
+        "{out}"
+    );
+    svc.finish();
+}
+
+/// `stop` and `status` reach a daemon only if one runs: they never start one.
+#[test]
+fn web_stop_and_status_never_start_a_daemon() {
+    let h = ManagerHarness::new();
+    let status = claudio(&h, &["web", "status"], |_| {});
+    assert_eq!(status.status.code(), Some(3), "{status:?}");
+    let out = String::from_utf8_lossy(&status.stdout);
+    assert!(
+        out.contains("web: disabled") && out.contains("daemon: not running"),
+        "{out}"
+    );
+    let stop = claudio(&h, &["web", "stop"], |_| {});
+    assert_eq!(stop.status.code(), Some(0), "{stop:?}");
+    let config = fs::read_to_string(h.config_home.join("claudio/config.toml")).unwrap();
+    assert!(config.contains("[web]\nenabled = false"), "{config}");
+    assert!(
+        !h.runtime_dir.join("claudio/daemon.log").exists(),
+        "no daemon started"
+    );
+}
+
+/// AC9.
+#[test]
+fn web_service_comes_back_after_a_sigkill() {
+    let mut svc = ServiceWeb::start(ManagerHarness::new(), &[], |_| {});
+    let first = svc.running_other_than(None, common::WAIT);
+    // SAFETY: kill(2); `first` is the service this test started.
+    unsafe { libc::kill(first as libc::pid_t, libc::SIGKILL) };
+    svc.running_other_than(Some(first), std::time::Duration::from_secs(5));
+    svc.finish();
+}
+
+/// AC4.
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn web_remote_through_the_daemon_serves_tls_and_prints_the_banner() {
+    let h = ManagerHarness::new();
+    let port = free_port();
+    let remote = format!("127.0.0.1:{port}");
+    let args = ["web", "--remote", remote.as_str(), "--no-open"];
+    let config = h.config_home.join("claudio/config.toml");
+    let before = fs::read_to_string(&config).unwrap();
+    let refused = claudio(&h, &args, |_| {});
+    assert_eq!(refused.status.code(), Some(1), "{refused:?}");
+    assert!(String::from_utf8_lossy(&refused.stderr).contains("--set-password"));
+    assert_eq!(
+        fs::read_to_string(&config).unwrap(),
+        before,
+        "config.toml untouched"
+    );
+    write_test_password(&h);
+    let out = claudio(&h, &args, |_| {});
+    assert_eq!(out.status.code(), Some(0), "{out:?}");
+    let mut svc = ServiceWeb::adopt(h, port);
+    let token = svc.token();
+    let stdout = String::from_utf8_lossy(&out.stdout);
+    assert_eq!(stdout, format!("claudio web: https://{remote}/\n"));
+    let stderr = String::from_utf8_lossy(&out.stderr);
+    let fp = kept_fingerprint(svc.h());
+    assert!(
+        stderr.contains(&format!("claudio web: TLS certificate SHA-256 {fp}\n")),
+        "{stderr}"
+    );
+    let here = format!("claudio web: on this machine: https://{remote}/#t={token}");
+    assert!(stderr.contains(&here), "{stderr}");
+    let page = request_text("GET", "/", &remote, &[], "");
+    let reply = https_from("127.0.0.1", port, &fp, &page).await.unwrap();
+    assert_eq!(reply.status, 200, "TLS through the daemon");
+    let (code, status) = svc.status();
+    assert_eq!(code, Some(0), "{status}");
+    assert!(
+        status.contains(&format!("certificate SHA-256 {fp}")),
+        "{status}"
+    );
+    svc.finish();
+}
+
+/// AC7.
+#[test]
+fn web_against_the_previous_release_s_daemon_saves_and_does_not_serve() {
+    let h = ManagerHarness::new();
+    let port = free_port();
+    let listen = format!("127.0.0.1:{port}");
+    let args = ["web", "--listen", listen.as_str(), "--no-open"];
+    let out = claudio(&h, &args, |cmd| {
+        cmd.env("CLAUDIO_TEST_OLD_DAEMON", "1");
+    });
+    assert_eq!(out.status.code(), Some(1), "{out:?}");
+    let err = String::from_utf8_lossy(&out.stderr);
+    assert!(
+        err.contains("the daemon has no web service (older, or built without"),
+        "{err}"
+    );
+    // An AddonOp would have been refused as `unsupported op`: none was sent.
+    assert!(!err.contains("unsupported op"), "{err}");
+    let config = fs::read_to_string(h.config_home.join("claudio/config.toml")).unwrap();
+    assert!(config.contains("[web]\nenabled = true"), "{config}");
+    assert!(
+        std::net::TcpListener::bind(("127.0.0.1", port)).is_ok(),
+        "nothing listens"
+    );
+    let mut svc = ServiceWeb::adopt(h, port);
+    let restart = svc.claudio(&["daemon", "restart"]);
+    assert!(restart.status.success(), "{restart:?}");
+    svc.running_other_than(None, common::DAEMON_WAIT);
+    assert_eq!(http(port, "GET", "/", &svc.host(), &[], "").status, 200);
+    svc.token();
+    svc.finish();
+}
+
+/// AC3: a service that ignores SIGTERM (and stdin's EOF) is SIGKILLed
+/// after 3 s, and the new daemon still starts.
+#[test]
+fn web_daemon_restart_kills_a_service_that_ignores_its_stop() {
+    let mut svc = ServiceWeb::start(ManagerHarness::new(), &[], |cmd| {
+        // The daemon this starts, and so its service, inherit it.
+        cmd.env("CLAUDIO_TEST_SERVICE_HANG", "1");
+    });
+    let first = svc.running_other_than(None, WAIT);
+    let started = std::time::Instant::now();
+    let restart = svc.claudio(&["daemon", "restart"]);
+    assert!(restart.status.success(), "{restart:?}");
+    assert!(
+        started.elapsed() >= std::time::Duration::from_secs(3),
+        "SIGKILL after 3 s"
+    );
+    assert!(!alive(first));
+    svc.running_other_than(Some(first), common::DAEMON_WAIT);
+    svc.finish();
+}
+
 // ── Persistent sign-ins ───────────────────────────────────────────────────────
 
 fn cookie_header(cookie: &str) -> String {
diff --git a/addons/web/tests/web_harness/mod.rs b/addons/web/tests/web_harness/mod.rs
index 0f5800a..2188def 100644
--- a/addons/web/tests/web_harness/mod.rs
+++ b/addons/web/tests/web_harness/mod.rs
@@ -5,6 +5,8 @@
 
 #![allow(dead_code)]
 
+/// `claudio web` handed to the harness daemon (the web service).
+pub mod service;
 /// `claudio web --remote`: a TLS server and a pinned client.
 pub mod tls;
 
diff --git a/addons/web/tests/web_harness/service.rs b/addons/web/tests/web_harness/service.rs
new file mode 100644
index 0000000..50c9ff9
--- /dev/null
+++ b/addons/web/tests/web_harness/service.rs
@@ -0,0 +1,219 @@
+//! `claudio web` handed to the harness daemon (the web service): run the
+//! CLI, read `claudio web status`, and end every test with no process of
+//! its own left and no token in a log (AC5, AC11).
+
+use std::process::{Command, Output, Stdio};
+use std::time::{Duration, Instant};
+use std::{fs, thread};
+
+use super::{apply_env, http};
+use crate::common::{ManagerHarness, BINARY};
+
+/// `claudio <args>` in `h`'s isolated env, run to its end: at most 60 s,
+/// so a command that keeps running fails the test instead of hanging it.
+pub fn claudio(h: &ManagerHarness, args: &[&str], setup: impl FnOnce(&mut Command)) -> Output {
+    let mut cmd = Command::new(BINARY);
+    cmd.args(args);
+    apply_env(h, &mut cmd);
+    setup(&mut cmd);
+    cmd.stdin(Stdio::null())
+        .stdout(Stdio::piped())
+        .stderr(Stdio::piped());
+    let mut child = cmd.spawn().expect("run claudio");
+    let deadline = Instant::now() + Duration::from_secs(60);
+    while child.try_wait().expect("wait for claudio").is_none() {
+        if Instant::now() >= deadline {
+            let _ = child.kill();
+            let _ = child.wait();
+            panic!("claudio {args:?} did not exit within 60 s");
+        }
+        thread::sleep(Duration::from_millis(50));
+    }
+    child.wait_with_output().expect("claudio's output")
+}
+
+/// Whether process `pid` exists (a zombie counts).
+pub fn alive(pid: u32) -> bool {
+    // SAFETY: kill(2) with signal 0 only checks that the process exists.
+    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
+}
+
+/// Wait up to `within` for `pid` to be gone; whether it is.
+pub fn gone_within(pid: u32, within: Duration) -> bool {
+    let deadline = Instant::now() + within;
+    while alive(pid) {
+        if Instant::now() >= deadline {
+            return false;
+        }
+        thread::sleep(Duration::from_millis(50));
+    }
+    true
+}
+
+/// The pid in `claudio web status`'s `service: running (pid N, …)` line.
+pub fn pid_in(status: &str) -> Option<u32> {
+    let rest = status.split("service: running (pid ").nth(1)?;
+    rest.split(',').next()?.parse().ok()
+}
+
+/// A web the harness daemon serves.
+pub struct ServiceWeb {
+    pub h: Option<ManagerHarness>,
+    pub port: u16,
+    /// Every token it served with (none may reach a log).
+    pub tokens: Vec<String>,
+    /// Every service pid seen (all must be gone at the end).
+    pub pids: Vec<u32>,
+}
+
+impl ServiceWeb {
+    /// `claudio web --listen 127.0.0.1:<port> --no-open` plus `args`: exits
+    /// 0 and prints the token URL.
+    pub fn start(h: ManagerHarness, args: &[&str], setup: impl FnOnce(&mut Command)) -> ServiceWeb {
+        let port = super::free_port();
+        let listen = format!("127.0.0.1:{port}");
+        let mut all = vec!["web", "--listen", &listen, "--no-open"];
+        all.extend_from_slice(args);
+        let out = claudio(&h, &all, setup);
+        let mut web = ServiceWeb {
+            h: Some(h),
+            port,
+            tokens: Vec::new(),
+            pids: Vec::new(),
+        };
+        assert_eq!(out.status.code(), Some(0), "{out:?}");
+        let token = web.token();
+        let printed = String::from_utf8_lossy(&out.stdout);
+        assert_eq!(
+            printed,
+            format!("claudio web: http://{listen}/#t={token}\n")
+        );
+        web
+    }
+
+    /// A web some other command handed over (`port` is its listener).
+    pub fn adopt(h: ManagerHarness, port: u16) -> ServiceWeb {
+        ServiceWeb {
+            h: Some(h),
+            port,
+            tokens: Vec::new(),
+            pids: Vec::new(),
+        }
+    }
+
+    pub fn h(&self) -> &ManagerHarness {
+        self.h.as_ref().unwrap()
+    }
+
+    /// The token file now (remembered for the log check).
+    pub fn token(&mut self) -> String {
+        let path = self.h().config_home.join("claudio/web-token");
+        let token = fs::read_to_string(path).unwrap().trim().to_owned();
+        if !self.tokens.contains(&token) {
+            self.tokens.push(token.clone());
+        }
+        token
+    }
+
+    /// `claudio <args>` in this harness.
+    pub fn claudio(&self, args: &[&str]) -> Output {
+        claudio(self.h(), args, |_| {})
+    }
+
+    /// `claudio web status`: its exit code and stdout; a running pid is
+    /// remembered.
+    pub fn status(&mut self) -> (Option<i32>, String) {
+        let out = self.claudio(&["web", "status"]);
+        let text = String::from_utf8_lossy(&out.stdout).into_owned();
+        if let Some(pid) = pid_in(&text).filter(|p| !self.pids.contains(p)) {
+            self.pids.push(pid);
+        }
+        (out.status.code(), text)
+    }
+
+    /// Wait until `status` says running with a pid other than `not`.
+    pub fn running_other_than(&mut self, not: Option<u32>, within: Duration) -> u32 {
+        let deadline = Instant::now() + within;
+        loop {
+            let (code, text) = self.status();
+            match pid_in(&text) {
+                Some(pid) if code == Some(0) && Some(pid) != not => return pid,
+                _ => assert!(Instant::now() < deadline, "not running: {text}"),
+            }
+            thread::sleep(Duration::from_millis(100));
+        }
+    }
+
+    /// A token login on the loopback listener: the cookie.
+    pub fn login(&mut self) -> String {
+        let token = self.token();
+        let (host, origin) = (self.host(), format!("http://{}", self.host()));
+        let body = serde_json::json!({ "token": token }).to_string();
+        let reply = http(
+            self.port,
+            "POST",
+            "/api/login",
+            &host,
+            &[("Origin", &origin)],
+            &body,
+        );
+        assert_eq!(reply.status, 204, "{reply:?}");
+        let set = reply.header("set-cookie").expect("a cookie").to_owned();
+        super::tls::cookie_of(&set)
+    }
+
+    /// `/api/me` with `cookie`.
+    pub fn me(&self, cookie: &str) -> u16 {
+        let with = format!("claudio_web={cookie}");
+        http(
+            self.port,
+            "GET",
+            "/api/me",
+            &self.host(),
+            &[("Cookie", &with)],
+            "",
+        )
+        .status
+    }
+
+    pub fn host(&self) -> String {
+        format!("127.0.0.1:{}", self.port)
+    }
+
+    /// The end of a test (AC11, AC5): `claudio web stop`, the daemon
+    /// killed, every service pid gone within 5 s, and no token in
+    /// `web.log` or `daemon.log`. Then the dirs go.
+    pub fn finish(mut self) {
+        let out = self.claudio(&["web", "stop"]);
+        assert!(out.status.success(), "{out:?}");
+        let h = self.h.take().unwrap();
+        h.kill_daemon();
+        for pid in &self.pids {
+            assert!(
+                gone_within(*pid, Duration::from_secs(5)),
+                "service {pid} left"
+            );
+        }
+        for log in ["web.log", "daemon.log"] {
+            let text = fs::read_to_string(h.runtime_dir.join("claudio").join(log));
+            let text = text.unwrap_or_default();
+            for token in &self.tokens {
+                assert!(!text.contains(token.as_str()), "the token is in {log}");
+            }
+        }
+    }
+}
+
+impl Drop for ServiceWeb {
+    /// A failed test still leaves nothing running before its dirs go.
+    fn drop(&mut self) {
+        let Some(h) = self.h.take() else {
+            return;
+        };
+        let _ = claudio(&h, &["web", "stop"], |_| {});
+        h.kill_daemon();
+        for pid in &self.pids {
+            gone_within(*pid, Duration::from_secs(5));
+        }
+    }
+}
diff --git a/addons/web/tests/web_harness/tls.rs b/addons/web/tests/web_harness/tls.rs
index 3c5cd77..4f5428a 100644
--- a/addons/web/tests/web_harness/tls.rs
+++ b/addons/web/tests/web_harness/tls.rs
@@ -36,6 +36,12 @@ pub fn fingerprint(der: &[u8]) -> String {
         .join(":")
 }
 
+/// The fingerprint of the certificate `h`'s config dir keeps.
+pub fn kept_fingerprint(h: &ManagerHarness) -> String {
+    let pem = h.config_home.join("claudio/web-tls/cert.pem");
+    fingerprint(&CertificateDer::from_pem_file(&pem).unwrap())
+}
+
 /// Write the fixture password file (0600) into `h`'s config dir.
 pub fn write_test_password(h: &ManagerHarness) {
     let dir = h.config_home.join("claudio");
`````

Run: `cargo test --locked -p claudio-web --lib service::`, then
`cargo test --locked --test web -- --test-threads=1 web_hands_its_server`
Expected: compilation fails (`` error[E0425]: cannot find function `merge` in this scope ``, …);
`web_hands_its_server_to_the_daemon_and_stop_takes_it_back ... FAILED` after 60 s with
`claudio ["web", "--listen", "127.0.0.1:<port>", "--no-open"] did not exit within 60 s` (a
bare `claudio web` still serves in the foreground).

Step 2: implement

Patch `task13-step2` (`addons/web/src/cli.rs`, `addons/web/src/lib.rs`, `addons/web/src/service.rs`, `addons/web/src/tls/cert.rs`):

`````diff
diff --git a/addons/web/src/cli.rs b/addons/web/src/cli.rs
new file mode 100644
index 0000000..8c7068c
--- /dev/null
+++ b/addons/web/src/cli.rs
@@ -0,0 +1,316 @@
+//! `claudio web` without `--foreground` (docs/web.md, "The web service"):
+//! the settings go to `[web]` in config.toml, the local daemon serves them
+//! (`apply`), and this command prints what a foreground start prints, then
+//! exits. Also `claudio web stop` and `status`. Whether the daemon has the
+//! service is decided on `Welcome.features` (`addon:web`) only, never on
+//! an error's text.
+
+use std::future::Future;
+use std::io::{self, IsTerminal};
+use std::path::Path;
+use std::process::ExitCode;
+
+use claudio_core::client;
+use claudio_core::paths;
+use claudio_core::proto::Msg;
+use serde_json::{json, Value};
+
+use crate::service::{self, Reported, Unit};
+use crate::{auth, parse_args, password, remote, start_url, tls, WebArgs};
+
+/// `claudio web` against a daemon without `addon:web`.
+const NO_SERVICE: &str = "the daemon has no web service (older, or built without the web \
+addon): claudio daemon restart when idle, or claudio web --foreground";
+
+/// What the local daemon said to a web op.
+enum Answer {
+    /// No daemon listens (only when not asked to start one).
+    NotRunning,
+    /// Its `Welcome.features` lack `addon:web`: no op was sent.
+    NoService,
+    Replied(Reported),
+}
+
+/// `op` to the local daemon's web addon. `start`: start the daemon (or
+/// replace an outdated idle one) first; only `claudio web` does.
+async fn ask(op: &str, body: Value, start: bool) -> Result<Answer, String> {
+    if start {
+        client::ensure_local_daemon()
+            .await
+            .map_err(|e| format!("cannot start the daemon: {e}"))?;
+    }
+    let daemon = match client::connect(&paths::daemon_socket()).await {
+        Ok(daemon) => daemon,
+        Err(_) if !start => return Ok(Answer::NotRunning),
+        Err(e) => return Err(format!("cannot reach the daemon: {e}")),
+    };
+    let feature = claudio_core::addon::feature("web");
+    if !daemon.welcome().features.contains(&feature) {
+        return Ok(Answer::NoService);
+    }
+    let msg = Msg::AddonOp {
+        addon: "web".into(),
+        op: op.into(),
+        body,
+    };
+    match daemon.request(msg).await {
+        Ok(Msg::AddonReply { body }) => Ok(Answer::Replied(service::read_reply(&body))),
+        Ok(_) => Err("the daemon answered something else".to_owned()),
+        Err(e) => Err(format!("the daemon refused the web op: {e}")),
+    }
+}
+
+/// `f` on a runtime of its own (the CLI has none).
+fn block_on<T>(f: impl Future<Output = Result<T, String>>) -> Result<T, String> {
+    tokio::runtime::Builder::new_current_thread()
+        .enable_all()
+        .build()
+        .map_err(|e| format!("cannot start the runtime: {e}"))?
+        .block_on(f)
+}
+
+/// The `[web]` table of the config file at `path` (empty when there is
+/// none). Blocking I/O.
+fn stored(path: &Path) -> Result<toml::Table, String> {
+    let text = match std::fs::read_to_string(path) {
+        Ok(text) => text,
+        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
+        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
+    };
+    let table: toml::Table = text
+        .parse()
+        .map_err(|_| format!("{} is not valid TOML", path.display()))?;
+    match table.get("web") {
+        None => Ok(toml::Table::new()),
+        Some(toml::Value::Table(web)) => Ok(web.clone()),
+        Some(_) => Err("[web] is not a table".to_owned()),
+    }
+}
+
+/// Write `[web] enabled`, and `unit` when given (replacing the stored one),
+/// through `config_edit` (minimal, atomic, serialised).
+fn save(path: &Path, enabled: bool, unit: Option<&Unit>) -> Result<(), String> {
+    use toml::Value as V;
+    let edited = claudio_core::app::config_edit::edit_section(path, "web", |t| {
+        t.insert("enabled".into(), V::Boolean(enabled));
+        let Some(u) = unit else {
+            return;
+        };
+        let (set, unset) = match u.remote {
+            true => ("remote", "listen"),
+            false => ("listen", "remote"),
+        };
+        t.insert(set.into(), V::String(u.addr.to_string()));
+        t.remove(unset);
+        let hosts: Vec<V> = u.allow_hosts.iter().cloned().map(V::String).collect();
+        match hosts.is_empty() {
+            true => t.remove("allow_hosts"),
+            false => t.insert("allow_hosts".into(), V::Array(hosts)),
+        };
+    });
+    edited
+        .map(|_| ())
+        .map_err(|e| format!("cannot save [web] in {}: {e}", path.display()))
+}
+
+/// `<runtime>/web.log`: the service's stderr, kept by the daemon.
+fn log_path() -> std::path::PathBuf {
+    paths::runtime_dir().join("web.log")
+}
+
+/// `claudio web [flags]`: check, save `[web]`, have the daemon serve it,
+/// print what a foreground start prints.
+pub(crate) fn start(args: WebArgs) -> ExitCode {
+    let config = paths::config_file();
+    let run = match stored(&config) {
+        Err(e) => return fail(&e),
+        Ok(web) => service::merge(&web, &args).and_then(|unit| service::run_args(&unit, &args)),
+    };
+    let run = match run {
+        Ok(run) => run,
+        Err(e) => {
+            eprintln!("claudio web: {e}");
+            return ExitCode::from(2);
+        }
+    };
+    // The token first: the service only ever loads it.
+    let token = match auth::load_or_create_token(&auth::token_path(), run.rotate_token) {
+        Ok(token) => token,
+        Err(e) => return fail(&format!("cannot read or write the token file: {e}")),
+    };
+    if run.print_url {
+        println!("{}", start_url(&run, &token));
+        return ExitCode::SUCCESS;
+    }
+    // The password and the certificate are checked (or made) here, so
+    // their errors show in this terminal and nothing is saved.
+    if let Some(addr) = run.remote {
+        let password = password::read(&password::path());
+        if let Err(e) = remote::prepare(&run, addr, &token, &password) {
+            return fail(&e);
+        }
+    }
+    if let Err(e) = save(&config, true, Some(&Unit::of(&run))) {
+        return fail(&e);
+    }
+    let force = run.rotate_token || run.rotate_cert;
+    match block_on(ask("apply", json!({ "force": force }), true)) {
+        Ok(Answer::Replied(Reported::Running { argv, .. })) => announce(&argv, &token, &run),
+        Ok(Answer::Replied(Reported::Failed { tail, .. })) => {
+            eprintln!("claudio web: the web service did not start: {tail}");
+            fail(&format!(
+                "see {}; it is tried again (claudio web status)",
+                log_path().display()
+            ))
+        }
+        Ok(Answer::Replied(other)) => fail(&format!("the web service is {}", what(&other))),
+        Ok(Answer::NoService | Answer::NotRunning) => fail(NO_SERVICE),
+        Err(e) => fail(&e),
+    }
+}
+
+fn fail(why: &str) -> ExitCode {
+    eprintln!("claudio web: {why}");
+    ExitCode::FAILURE
+}
+
+/// What a foreground start prints, for what runs (the reply's argv: two
+/// `claudio web` at once each print the one that won), then the browser.
+fn announce(argv: &[String], token: &str, asked: &WebArgs) -> ExitCode {
+    let Some(Ok(running)) = argv.get(2..).map(parse_args) else {
+        return fail("the daemon runs a web service this claudio cannot read");
+    };
+    let url = start_url(&running, token);
+    let remote = match running.remote {
+        None => None,
+        Some(addr) => {
+            let password = password::read(&password::path());
+            match remote::prepare(&running, addr, token, &password) {
+                Ok(r) => Some(r),
+                Err(e) => return fail(&e),
+            }
+        }
+    };
+    crate::announce(&url, remote.as_ref());
+    let wanted = claudio_core::app::browser::browser_wanted(
+        std::env::consts::OS,
+        |k| std::env::var(k).ok(),
+        io::stdout().is_terminal(),
+    );
+    if !asked.no_open && wanted {
+        claudio_core::app::browser::open_browser(&url);
+    }
+    ExitCode::SUCCESS
+}
+
+/// A state in words.
+fn what(state: &Reported) -> String {
+    match state {
+        Reported::Off => "off".to_owned(),
+        Reported::Starting => "starting".to_owned(),
+        Reported::Running { pid, since, .. } => {
+            let up = paths::unix_now().saturating_sub(*since);
+            format!("running (pid {pid}, for {up} s)")
+        }
+        Reported::Failed {
+            exit,
+            signal,
+            retry_in,
+            ..
+        } => {
+            let how = match (exit, signal) {
+                (Some(code), _) => format!("exit {code}"),
+                (None, Some(sig)) => format!("signal {sig}"),
+                (None, None) => "not started".to_owned(),
+            };
+            let next = match retry_in {
+                Some(s) => format!(", next try in {s} s"),
+                None => ", not tried again until claudio web or a daemon start".to_owned(),
+            };
+            format!("failed ({how}{next})")
+        }
+        Reported::Newer => "unknown to this claudio (the daemon is newer)".to_owned(),
+    }
+}
+
+/// `claudio web stop`: `[web] enabled = false`, then the daemon stops it.
+pub(crate) fn stop() -> ExitCode {
+    if let Err(e) = save(&paths::config_file(), false, None) {
+        return fail(&e);
+    }
+    match block_on(ask("apply", json!({ "force": false }), false)) {
+        Ok(Answer::NotRunning | Answer::NoService) => ExitCode::SUCCESS,
+        Ok(Answer::Replied(Reported::Off)) => {
+            eprintln!("claudio web: stopped");
+            ExitCode::SUCCESS
+        }
+        Ok(Answer::Replied(other)) => fail(&format!("the web service is {}", what(&other))),
+        Err(e) => fail(&e),
+    }
+}
+
+/// `claudio web status`: enabled or not, the daemon's state, the URL
+/// (without the token) and, with `--remote`, the certificate's
+/// fingerprint. Exit 0 while it runs, 3 otherwise.
+pub(crate) fn status() -> ExitCode {
+    let web = stored(&paths::config_file());
+    match web
+        .as_ref()
+        .map_err(Clone::clone)
+        .and_then(service::enabled)
+    {
+        Ok(true) => println!("web: enabled"),
+        Ok(false) => println!("web: disabled"),
+        Err(e) => println!("web: {e}"),
+    }
+    let kept = web.ok().and_then(|w| service::unit(&w).ok().flatten());
+    let (state, code) = match block_on(ask("status", json!({}), false)) {
+        Ok(Answer::NotRunning) => {
+            println!("daemon: not running");
+            (None, 3)
+        }
+        Ok(Answer::NoService) => {
+            println!("daemon: no web service (older, or built without the web addon)");
+            (None, 3)
+        }
+        Ok(Answer::Replied(state)) => {
+            println!("service: {}", what(&state));
+            let code = if matches!(state, Reported::Running { .. }) {
+                0
+            } else {
+                3
+            };
+            (Some(state), code)
+        }
+        Err(e) => return fail(&e),
+    };
+    if let Some(Reported::Failed { tail, .. }) = &state {
+        tail.lines().for_each(|l| println!("  {l}"));
+        println!("log: {}", log_path().display());
+    }
+    // What runs (the reply's argv), else what is stored.
+    let running = match &state {
+        Some(Reported::Running { argv, .. }) => argv.get(2..).and_then(|a| parse_args(a).ok()),
+        _ => None,
+    };
+    let shown = running.or_else(|| parse_args(&kept.unwrap_or_default().flags()).ok());
+    if let Some(shown) = shown {
+        listener(&shown);
+    }
+    ExitCode::from(code)
+}
+
+/// `args`' URL without the token and, with `--remote`, the kept
+/// certificate's fingerprint.
+fn listener(args: &WebArgs) {
+    let Some(addr) = args.remote else {
+        println!("url: http://{}/", args.listen);
+        return;
+    };
+    let url = remote::public_url(addr, &claudio_core::sys::hostname());
+    println!("url: {url}");
+    match tls::cert::kept_fingerprint(&tls::cert::dir()) {
+        Ok(fp) => println!("certificate SHA-256 {fp}"),
+        Err(e) => println!("certificate: {e}"),
+    }
+}
diff --git a/addons/web/src/lib.rs b/addons/web/src/lib.rs
index a81b1f9..129be9d 100644
--- a/addons/web/src/lib.rs
+++ b/addons/web/src/lib.rs
@@ -8,6 +8,7 @@ mod addons;
 pub(crate) mod assets;
 mod atomic;
 pub(crate) mod auth;
+mod cli;
 mod dto;
 mod events;
 mod ext;
@@ -58,6 +59,7 @@ pub(crate) fn version() -> String {
 const USAGE: &str =
     "usage: claudio web [--foreground] [--listen 127.0.0.1:7337 | --remote [ADDR][:PORT] [--rotate-cert]] \
 [--allow-host NAME[:PORT]]... [--no-open] [--print-url] [--rotate-token]
+       claudio web stop | status
        claudio web --set-password
        claudio web --print-cert";
 
@@ -178,7 +180,9 @@ fn combination_ok(a: &WebArgs, n_args: usize) -> Result<(), String> {
     if a.remote.is_some() && a.listen_given {
         return Err("--remote is the one listener: drop --listen".to_owned());
     }
-    if a.rotate_cert && a.remote.is_none() {
+    // Without --foreground the stored [web] may be a --remote one: `cli`
+    // checks the settings it merges.
+    if a.rotate_cert && a.remote.is_none() && a.foreground {
         return Err("--rotate-cert needs --remote".to_owned());
     }
     if let Some(remote) = a.remote {
@@ -259,6 +263,11 @@ pub fn run(args: &[String], addons: Arc<claudio_core::addon::Addons>) -> ExitCod
         println!("{USAGE}");
         return ExitCode::SUCCESS;
     }
+    match args {
+        [word] if word == "stop" => return cli::stop(),
+        [word] if word == "status" => return cli::status(),
+        _ => {}
+    }
     let args = match parse_args(args) {
         Ok(args) => args,
         Err(e) => {
@@ -269,11 +278,15 @@ pub fn run(args: &[String], addons: Arc<claudio_core::addon::Addons>) -> ExitCod
     if let Some(code) = one_shot(&args) {
         return code;
     }
-    serve_here(args, addons)
+    match args.foreground || args.service {
+        true => serve_here(args, addons),
+        // The daemon serves it (docs/web.md, "The web service").
+        false => cli::start(args),
+    }
 }
 
-/// Serve from this process: `claudio web --foreground` (and, until the daemon
-/// serves it, a bare `claudio web`), or `--service`, the daemon's child.
+/// Serve from this process: `claudio web --foreground`, or `--service`, the
+/// daemon's child.
 fn serve_here(args: WebArgs, addons: Arc<claudio_core::addon::Addons>) -> ExitCode {
     // First thing: a daemon that stops us (or dies) during the start is obeyed.
     let eof = args.service.then(watch_stdin);
@@ -552,7 +565,7 @@ fn started(args: &WebArgs, url: Option<&str>, remote: Option<&remote::Remote>) {
 
 /// The start lines: with `--remote` the banner's stderr lines first, then
 /// the one stdout line the tests read.
-fn announce(url: &str, remote: Option<&remote::Remote>) {
+pub(crate) fn announce(url: &str, remote: Option<&remote::Remote>) {
     let line = match remote {
         Some(r) => {
             for l in &r.banner.stderr {
diff --git a/addons/web/src/service.rs b/addons/web/src/service.rs
index 54c6439..278c290 100644
--- a/addons/web/src/service.rs
+++ b/addons/web/src/service.rs
@@ -6,10 +6,11 @@
 use std::net::SocketAddr;
 
 use claudio_core::addon::ServiceState;
+use serde::Deserialize;
 use serde_json::{json, Value};
 use toml::Table;
 
-use crate::{host_name_ok, listen_addr, parse_args, remote, DEFAULT_PORT};
+use crate::{host_name_ok, listen_addr, parse_args, remote, WebArgs, DEFAULT_PORT};
 
 /// What `[web]` stores as one unit: the listener and its allowed hosts.
 #[derive(Debug, Clone, PartialEq, Eq)]
@@ -32,6 +33,15 @@ impl Default for Unit {
 }
 
 impl Unit {
+    /// The unit the command line names.
+    pub(crate) fn of(args: &WebArgs) -> Unit {
+        Unit {
+            remote: args.remote.is_some(),
+            addr: args.remote.unwrap_or(args.listen),
+            allow_hosts: args.allow_hosts.clone(),
+        }
+    }
+
     /// Its flags, each value in its flag's word (`--listen=…`): no value
     /// can be read as another flag.
     pub(crate) fn flags(&self) -> Vec<String> {
@@ -118,6 +128,32 @@ pub(crate) fn argv(t: &Table) -> Result<Option<Vec<String>>, String> {
     Ok(Some(head.into_iter().chain(unit.flags()).collect()))
 }
 
+/// The unit `claudio web` serves: the command line's when it names any of
+/// `--listen`, `--remote` or `--allow-host` (what it does not name takes
+/// its default), else the stored one (`web`, the `[web]` table), else the
+/// defaults. A stored unit is checked only when it is used.
+pub(crate) fn merge(web: &Table, args: &WebArgs) -> Result<Unit, String> {
+    if args.listen_given || args.remote.is_some() || !args.allow_hosts.is_empty() {
+        return Ok(Unit::of(args));
+    }
+    Ok(unit(web)?.unwrap_or_default())
+}
+
+/// The command line `claudio web` acts on: `unit`'s flags (checked as
+/// flags are) with `args`' one-shot flags. `--rotate-cert` needs a
+/// `--remote` unit.
+pub(crate) fn run_args(unit: &Unit, args: &WebArgs) -> Result<WebArgs, String> {
+    if args.rotate_cert && !unit.remote {
+        return Err("--rotate-cert needs --remote (here, or stored in [web])".to_owned());
+    }
+    let mut run = parse_args(&unit.flags())?;
+    run.no_open = args.no_open;
+    run.print_url = args.print_url;
+    run.rotate_token = args.rotate_token;
+    run.rotate_cert = args.rotate_cert;
+    Ok(run)
+}
+
 /// The one reply of `apply` and `status` (docs/addons.md, "The web addon").
 pub(crate) fn reply(state: &ServiceState) -> Value {
     match state {
@@ -141,6 +177,67 @@ pub(crate) fn reply(state: &ServiceState) -> Value {
     }
 }
 
+/// A reply as this claudio reads it.
+#[derive(Debug, Clone, PartialEq, Eq)]
+pub(crate) enum Reported {
+    Off,
+    Starting,
+    Running {
+        pid: u32,
+        since: u64,
+        argv: Vec<String>,
+    },
+    Failed {
+        exit: Option<i32>,
+        signal: Option<i32>,
+        tail: String,
+        retry_in: Option<u64>,
+    },
+    /// A state (or shape) this claudio does not know: the daemon is newer.
+    Newer,
+}
+
+/// The wire form of [`reply`]; unknown fields are ignored.
+#[derive(Deserialize)]
+#[serde(tag = "state", rename_all = "lowercase")]
+enum Wire {
+    Off,
+    Starting,
+    Running {
+        pid: u32,
+        since: u64,
+        argv: Vec<String>,
+    },
+    Failed {
+        exit: Option<i32>,
+        signal: Option<i32>,
+        #[serde(default)]
+        tail: String,
+        retry_in: Option<u64>,
+    },
+}
+
+/// `body` of an `apply` or `status` reply.
+pub(crate) fn read_reply(body: &Value) -> Reported {
+    match Wire::deserialize(body) {
+        Ok(Wire::Off) => Reported::Off,
+        Ok(Wire::Starting) => Reported::Starting,
+        Ok(Wire::Running { pid, since, argv }) => Reported::Running { pid, since, argv },
+        Ok(Wire::Failed {
+            exit,
+            signal,
+            tail,
+            retry_in,
+        }) => Reported::Failed {
+            exit,
+            signal,
+            tail,
+            retry_in,
+        },
+        Err(_) => Reported::Newer,
+    }
+}
+
 #[cfg(test)]
 mod tests {
     use super::*;
diff --git a/addons/web/src/tls/cert.rs b/addons/web/src/tls/cert.rs
index 2975a40..f98bcf2 100644
--- a/addons/web/src/tls/cert.rs
+++ b/addons/web/src/tls/cert.rs
@@ -104,6 +104,13 @@ pub(crate) fn read_cert_pem(dir: &Path) -> Result<String, String> {
     Ok(pem)
 }
 
+/// The kept certificate's SHA-256 fingerprint (`claudio web status`).
+pub(crate) fn kept_fingerprint(dir: &Path) -> Result<String, String> {
+    let pem = read_cert_pem(dir)?;
+    let der = CertificateDer::from_pem_slice(pem.as_bytes()).map_err(|e| e.to_string())?;
+    Ok(fingerprint(&der))
+}
+
 /// A private regular file of ours, at most [`PEM_MAX`] bytes, as text.
 fn read_private(path: &Path) -> Result<String, String> {
     let meta = std::fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
`````

Run: `cargo test --locked -p claudio-web --lib`, then
`cargo test --locked --test web -- --test-threads=1 web_hands web_service_comes web_remote_through web_against web_daemon_restart_kills web_stop_and_status`,
then `cargo test --locked --test web --test vocdoni_web --test web_manager -- --test-threads=1`,
then `pgrep -af "web --service"` (must print nothing of the suites), then the `make check`
sequence.
Expected: `test result: ok. 116 passed; 0 failed`; `test result: ok. 6 passed; 0 failed`;
vocdoni_web `ok. 13 passed`, web `ok. 46 passed`, web_manager `ok. 4 passed`; no leftover
process; `rules: 0 problem(s)`.

Step 3: commit
`git add addons/web/src/cli.rs addons/web/tests/web_harness/service.rs && git commit -m "feat(web): claudio web hands the server to the daemon; stop and status" -- addons/web/src/cli.rs addons/web/src/service.rs addons/web/src/lib.rs addons/web/src/tls/cert.rs addons/web/tests/web.rs addons/web/tests/web_harness/service.rs addons/web/tests/web_harness/mod.rs addons/web/tests/web_harness/tls.rs`

Interfaces this task provides to later tasks: `cli::{start(WebArgs) -> ExitCode, stop() ->
ExitCode, status() -> ExitCode}` and the private `cli::{ask, block_on, stored, save, what,
Answer}`; `service::{Unit::of(&WebArgs) -> Unit, merge(&Table, &WebArgs) -> Result<Unit,
String>, run_args(&Unit, &WebArgs) -> Result<WebArgs, String>, Reported, read_reply(&Value) ->
Reported}`; `tls::cert::kept_fingerprint(&Path) -> Result<String, String>`; the harness's
`web_harness::service::{claudio, alive, gone_within, pid_in, ServiceWeb}` and
`web_harness::tls::kept_fingerprint`. `claudio web status` prints `web: enabled|disabled`, then
`daemon: not running` / `daemon: no web service …` or `service: running (pid N, for S s)` /
`service: failed (…)` / `service: off`, then `url: …` (and `certificate SHA-256 …` with
`--remote`); exit 0 while running, 3 otherwise.

### Task 14: A new token or password restarts the web service
Model: sonnet   Repo: /home/p4u/claudio   Files: `addons/web/src/cli.rs`, `addons/web/src/lib.rs`,
`addons/web/tests/web.rs`
Goal: the CLI table's `--set-password` and `--print-url` rows (AC13's service-mode bullets):
after `--set-password`, and after `--print-url --rotate-token`, `apply { force: true }` when
`[web] enabled` and the daemon lists `addon:web` (plain connect: no daemon is started). With
Task 13's `claudio web --rotate-token` (which forces the restart itself), a sign-in outlives a
daemon restart but not a new token or password.

Step 1: write the failing test

Patch `task14-step1` (`addons/web/tests/web.rs`):

`````diff
diff --git a/addons/web/tests/web.rs b/addons/web/tests/web.rs
index a7f6f35..58f3295 100644
--- a/addons/web/tests/web.rs
+++ b/addons/web/tests/web.rs
@@ -529,6 +529,42 @@ fn web_daemon_restart_kills_a_service_that_ignores_its_stop() {
     svc.finish();
 }
 
+/// AC13, service mode: sign-ins outlive a daemon restart; a new token
+/// (`claudio web --rotate-token`, `--print-url --rotate-token`) or a new
+/// password (`--set-password`, typed) restarts the service and signs
+/// everyone out at once.
+#[test]
+fn web_service_sign_ins_outlive_a_daemon_restart_but_not_a_new_secret() {
+    let mut svc = ServiceWeb::start(ManagerHarness::new(), &[], |_| {});
+    let cookie = svc.login();
+    let first = svc.running_other_than(None, WAIT);
+    let restart = svc.claudio(&["daemon", "restart"]);
+    assert!(restart.status.success(), "{restart:?}");
+    let pid = svc.running_other_than(Some(first), common::DAEMON_WAIT);
+    assert_eq!(svc.me(&cookie), 204, "kept across the daemon restart");
+
+    let out = svc.claudio(&["web", "--rotate-token", "--no-open"]);
+    assert!(out.status.success(), "{out:?}");
+    let token = svc.token();
+    assert!(String::from_utf8_lossy(&out.stdout).contains(&format!("#t={token}")));
+    let pid = svc.running_other_than(Some(pid), WAIT);
+    assert_eq!(svc.me(&cookie), 401, "a new token signs everyone out");
+
+    let cookie = svc.login();
+    let out = svc.claudio(&["web", "--print-url", "--rotate-token"]);
+    assert!(out.status.success(), "{out:?}");
+    let token = svc.token();
+    assert!(String::from_utf8_lossy(&out.stdout).ends_with(&format!("#t={token}\n")));
+    let pid = svc.running_other_than(Some(pid), WAIT);
+    assert_eq!(svc.me(&cookie), 401, "--print-url --rotate-token too");
+
+    let cookie = svc.login();
+    assert_eq!(set_password_typed(svc.h(), "8-bytes!"), 0);
+    svc.running_other_than(Some(pid), WAIT);
+    assert_eq!(svc.me(&cookie), 401, "a new password signs everyone out");
+    svc.finish();
+}
+
 // ── Persistent sign-ins ───────────────────────────────────────────────────────
 
 fn cookie_header(cookie: &str) -> String {
`````

Run: `cargo test --locked --test web -- --test-threads=1 web_service_sign_ins`
Expected: `web_service_sign_ins_outlive_a_daemon_restart_but_not_a_new_secret ... FAILED`, after
10 s at `--print-url --rotate-token` with `not running: web: enabled` followed by the same
`service: running (pid …)` (no restart happened).

Step 2: implement

Patch `task14-step2` (`addons/web/src/cli.rs`, `addons/web/src/lib.rs`):

`````diff
diff --git a/addons/web/src/cli.rs b/addons/web/src/cli.rs
index 8c7068c..d2197ad 100644
--- a/addons/web/src/cli.rs
+++ b/addons/web/src/cli.rs
@@ -1,9 +1,10 @@
 //! `claudio web` without `--foreground` (docs/web.md, "The web service"):
 //! the settings go to `[web]` in config.toml, the local daemon serves them
 //! (`apply`), and this command prints what a foreground start prints, then
-//! exits. Also `claudio web stop` and `status`. Whether the daemon has the
-//! service is decided on `Welcome.features` (`addon:web`) only, never on
-//! an error's text.
+//! exits. Also `claudio web stop` and `status`, and the service restart
+//! `--set-password` and `--print-url --rotate-token` ask for. Whether the
+//! daemon has the service is decided on `Welcome.features` (`addon:web`)
+//! only, never on an error's text.
 
 use std::future::Future;
 use std::io::{self, IsTerminal};
@@ -140,6 +141,9 @@ pub(crate) fn start(args: WebArgs) -> ExitCode {
     };
     if run.print_url {
         println!("{}", start_url(&run, &token));
+        if run.rotate_token {
+            restart_if_enabled("a new token");
+        }
         return ExitCode::SUCCESS;
     }
     // The password and the certificate are checked (or made) here, so
@@ -314,3 +318,25 @@ fn listener(args: &WebArgs) {
         Err(e) => println!("certificate: {e}"),
     }
 }
+
+/// After `--set-password` or `--print-url --rotate-token`: restart the
+/// service (`apply { force: true }`) so `what` takes effect now, when
+/// `[web]` is enabled and the daemon has the service; else nothing.
+pub(crate) fn restart_if_enabled(what_changed: &str) {
+    let enabled = stored(&paths::config_file())
+        .ok()
+        .and_then(|web| service::enabled(&web).ok());
+    if enabled != Some(true) {
+        return;
+    }
+    match block_on(ask("apply", json!({ "force": true }), false)) {
+        Ok(Answer::Replied(Reported::Running { .. })) => eprintln!(
+            "claudio web: the web service restarted with {what_changed}; browsers sign in again"
+        ),
+        Ok(Answer::Replied(other)) => {
+            eprintln!("claudio web: the web service is {}", what(&other))
+        }
+        Ok(Answer::NotRunning | Answer::NoService) => {}
+        Err(e) => eprintln!("claudio web: {e}"),
+    }
+}
diff --git a/addons/web/src/lib.rs b/addons/web/src/lib.rs
index 129be9d..ed09600 100644
--- a/addons/web/src/lib.rs
+++ b/addons/web/src/lib.rs
@@ -448,6 +448,7 @@ fn one_shot(args: &WebArgs) -> Option<ExitCode> {
         return Some(match password::set_password() {
             Ok(done) => {
                 eprintln!("claudio web: {done}");
+                cli::restart_if_enabled("the new password");
                 ExitCode::SUCCESS
             }
             Err(e) => {
`````

Run: `cargo test --locked --test web -- --test-threads=1`, then
`cargo test --locked --test vocdoni_web --test web_manager -- --test-threads=1`, then the
`make check` sequence.
Expected: web `test result: ok. 47 passed; 0 failed`; vocdoni_web `ok. 13 passed`,
web_manager `ok. 4 passed`; `rules: 0 problem(s)`.

Step 3: commit
`git commit -m "feat(web): a new token or password restarts the web service" -- addons/web/src/cli.rs addons/web/src/lib.rs addons/web/tests/web.rs`

Interfaces this task provides to later tasks: `cli::restart_if_enabled(what_changed: &str)`.

### Task 15: User docs: the web service, persistent sign-ins, 8-byte passwords
Model: sonnet   Repo: /home/p4u/claudio   Files: `docs/web.md`, `README.md`, `Makefile`
Goal: F1 and the spec's docs list: `docs/web.md` (usage and line 27's `--rotate-token` note;
a new "The web service" section with `[web]`, `stop`/`status`, restarts, logout and reboot,
the environment note and its fix, `--foreground` for scripts; the `--remote` steps and the
"keeps the old password …" sentence; "24 h idle"; the Cookie bullet with `Max-Age`,
`web-sessions.json`, the epoch, two servers and the stolen-cookie trade-off; the password
range in bytes; the palette's Sign out; the state files; `CLAUDIO_WEB_DIR` with `--foreground`
under "Developing the page"), `README.md` (the `claudio web` lines
and the `--remote` paragraph), the `Makefile`'s `dev` notes. (The module docs `auth.rs:1-12`
and `password.rs:21` changed with their code in Tasks 4 and 8.)

This task has no test: it is documentation. Check every command it names against the code of
Tasks 4-14 (`claudio web --help`'s usage, `addons/web/src/cli.rs`).

Step 1: the docs

Patch `task15` (`Makefile`, `README.md`, `docs/web.md`):

`````diff
diff --git a/Makefile b/Makefile
index 1abbc5e..83a7dbc 100644
--- a/Makefile
+++ b/Makefile
@@ -155,8 +155,8 @@ install: static
 ## The page's footer then reads e.g. `claudio 0.8.0+dev.20f0b49` (`.dirty` with
 ## uncommitted changes). The previous binary is kept as $(BIN).prev (undo:
 ## `make dev-undo`). Running daemons are NOT restarted: each is replaced by this
-## build once it is idle, when a claudio client (TUI or `claudio web`) connects.
-## Restart a running `claudio web` yourself (Ctrl+C, run it again, reload the page).
+## build once it is idle, when a claudio client (TUI or `claudio web`) connects; the web service
+## restarts with its daemon. Restart a `claudio web --foreground` yourself (Ctrl+C, run it again).
 ## Remote hosts get this build too when the TUI or `claudio web` connects to them.
 dev:
 	CLAUDIO_BUILD_LABEL="+dev.$$(git rev-parse --short HEAD)$$(git diff --quiet HEAD -- 2>/dev/null || echo .dirty)" \
@@ -166,7 +166,7 @@ dev:
 	@install -m755 $(DIST)/$(BIN) $(PREFIX)/bin/.$(BIN).new
 	@mv -f $(PREFIX)/bin/.$(BIN).new $(PREFIX)/bin/$(BIN)
 	@echo "installed $(PREFIX)/bin/$(BIN) (local build; previous one in $(BIN).prev)"
-	@echo "next: restart \`claudio web\` (Ctrl+C, run it again) and reload the page"
+	@echo "next: run claudio or claudio web: an idle daemon, and its web service, move onto this build; reload the page"
 
 ## Put back the binary `make dev` replaced.
 dev-undo:
diff --git a/README.md b/README.md
index 1671842..4276721 100644
--- a/README.md
+++ b/README.md
@@ -11,10 +11,11 @@ Run many [Claude Code](https://claude.ai/claude-code) sessions, on this machine
 ## The web UI
 
 ```bash
-claudio web          # serves http://127.0.0.1:7337 and opens it in your browser
+claudio web          # serves http://127.0.0.1:7337 through the daemon and opens it in your browser
+claudio web status   # is it running? (claudio web stop turns it off)
 ```
 
-It prints `claudio web: http://127.0.0.1:7337/#t=<token>`. The first visit trades the token for a cookie; later visits need nothing. The page is one more client of your daemons, like the terminal UI: the same sessions, hosts and `config.toml`. The page makes no external requests and runs under a strict CSP.
+It prints `claudio web: http://127.0.0.1:7337/#t=<token>` and exits: the daemon keeps the web running, restarts it if it stops, and starts it again with the daemon (after a reboot, at the next `claudio`). `claudio web --foreground` serves from the terminal instead. The first visit trades the token for a cookie; later visits need nothing until the sign-in goes unused for 30 days, and closing the browser keeps it (the palette's Sign out ends it). The page is one more client of your daemons, like the terminal UI: the same sessions, hosts and `config.toml`. The page makes no external requests and runs under a strict CSP.
 
 ![The board: one row per session, with state, host, branch, what it needs, model and context](docs/screenshots/web-board.png)
 
@@ -49,7 +50,7 @@ Also: one, two or four panes, terminal tabs, search across every conversation (`
 By default (without `--remote`), `claudio web` listens on loopback only. Two ways to reach it from a laptop or a phone:
 
 - **An SSH tunnel.** `ssh -L 7337:127.0.0.1:7337 host`, then open the URL `claudio web` printed on the host.
-- **`--remote`.** Set a password once with `claudio web --set-password`, then run `claudio web --remote`. It serves HTTPS on the network (port 7337 by default) with a self-signed certificate made on the first start and kept. The terminal prints the certificate's SHA-256 fingerprint at every start: check it against the one your browser shows before you accept the certificate. Remote browsers sign in with the password; the token link works only from the machine itself. `claudio web --print-cert` prints the certificate, to install it as trusted on the devices you use.
+- **`--remote`.** Set a password once with `claudio web --set-password` (8 bytes or more), then run `claudio web --remote`. The daemon serves HTTPS on the network (port 7337 by default) with a self-signed certificate made on the first start and kept. `claudio web --remote` and `claudio web status` print the certificate's SHA-256 fingerprint: check it against the one your browser shows before you accept the certificate. Remote browsers sign in with the password; the token link works only from the machine itself. `claudio web --print-cert` prints the certificate, to install it as trusted on the devices you use.
 
 The token, the password's hash and the certificate's key stay in `~/.config/claudio/` (mode 0600). Whoever has the token or the password can type into your sessions. The security model is in [docs/web.md](docs/web.md).
 
@@ -174,7 +175,7 @@ An exported `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC` is kept and turns the fea
 | Command | What it does |
 |---|---|
 | `claudio [--proxy NAME\|--no-proxy] [--first-party]` | The terminal UI |
-| `claudio web [--listen ADDR] [--no-open]` | The web UI (also `--remote`, `--set-password`, `--print-cert`, `--print-url`, `--rotate-token`) |
+| `claudio web [--listen ADDR] [--no-open]` | The web UI, served by the daemon (also `stop`, `status`, `--foreground`, `--remote`, `--set-password`, `--print-cert`, `--print-url`, `--rotate-token`) |
 | `claudio --plain [claude args…]` | One full-screen claude session |
 | `claudio sessions` | List the local sessions as a table |
 | `claudio daemon status\|stop\|restart` | Inspect or control this host's daemon |
diff --git a/docs/web.md b/docs/web.md
index 0159f9e..76bcba7 100644
--- a/docs/web.md
+++ b/docs/web.md
@@ -21,10 +21,13 @@ through the version handshake. Addon text is rendered as text only.
 ## Usage
 
 ```bash
-claudio web                      # serve on 127.0.0.1:7337 and open the browser
+claudio web                      # the daemon serves on 127.0.0.1:7337; open the browser, exit
 claudio web --listen 127.0.0.1:9000 --no-open
-claudio web --print-url          # print the sign-in URL for the stored token and exit
-claudio web --rotate-token       # make a new token (every browser signs in again)
+claudio web status               # whether it runs, where, and why not (exit 0 while it runs)
+claudio web stop                 # stop it, and keep it off
+claudio web --foreground         # serve from this terminal instead, until Ctrl+C
+claudio web --print-url          # print the sign-in URL (stored settings and token) and exit
+claudio web --rotate-token       # make a new token: every browser signs in again
 claudio web --set-password       # set the password other machines log in with (asks twice)
 claudio web --remote             # TLS on 0.0.0.0:7337 for other machines; needs the password
 claudio web --remote [::]:8443   # IPv6 and IPv4, port 8443
@@ -32,13 +35,56 @@ claudio web --print-cert         # the certificate, to install as trusted on a l
 claudio web --remote --rotate-cert   # a new certificate (browsers warn once more)
 ```
 
-On start it prints `claudio web: http://127.0.0.1:7337/#t=<token>`. Open that URL once: the
-page posts the token to `/api/login`, gets a cookie and removes the token from the address bar.
-Later visits need no token while the cookie lives (24 h idle).
+It prints `claudio web: http://127.0.0.1:7337/#t=<token>` and exits; the server keeps running in
+the background (next section). Open that URL once: the page posts the token to `/api/login`,
+gets a cookie and removes the token from the address bar. Later visits need no token while the
+sign-in lives: until it goes unused for 30 days (an open tab counts as use), across server and
+daemon restarts and closed browsers. The palette's (`k`) **Sign out** ends this browser's
+sign-in; `--rotate-token` (or a new password) ends every browser's.
 
 Flags: `--listen ADDR` (loopback only: `127.0.0.0/8`, `::1`, `localhost`; default
 `127.0.0.1:7337`), `--allow-host NAME[:PORT]` (repeatable), `--no-open`, `--print-url`,
-`--rotate-token`. `CLAUDIO_NO_BROWSER=1` also stops it from opening a browser.
+`--rotate-token`, `--foreground`. `CLAUDIO_NO_BROWSER=1` also stops it from opening a browser.
+
+### The web service
+
+`claudio web` checks its flags in the terminal, saves them in `config.toml`, has this host's
+daemon start the server, prints what a start prints (with `--remote`, the certificate banner on
+stderr too), opens the browser and exits. The daemon runs `claudio web --service` from `[web]`:
+
+```toml
+[web]
+enabled = true
+listen = "127.0.0.1:7337"         # or: remote = "0.0.0.0:7337" (TLS, the password)
+allow_hosts = ["claudio.example.org"]
+```
+
+- `--listen`, `--remote` and `--allow-host` replace the stored settings as one unit (what you do
+  not give takes its default); with none of them, `claudio web` uses the stored ones. The one-shot
+  flags are never stored. `[web]` is checked like the flags: a hand-edited
+  `listen = "0.0.0.0:7337"` fails with the flag's message and never binds; a hand edit takes
+  effect at the next `claudio web` or daemon start.
+- While `enabled = true`, every start of the daemon starts the web, and the daemon starts it again
+  when it exits (after 1 s, doubling to 60 s while it keeps failing). A new build takes over with
+  the daemon (`claudio upgrade`, `make dev`, or an outdated idle daemon being replaced). After a
+  reboot it comes back when the daemon next starts: the next `claudio` of any kind. No systemd
+  unit or launchd agent is installed, so it survives a logout only where the daemon does.
+- `claudio web stop` turns it off (`enabled = false`). `claudio web status` shows whether it
+  runs (its pid), the URL without the token, with `--remote` the certificate's SHA-256
+  fingerprint, or why it failed and its log, `<runtime>/web.log` (the server's stderr; 2 MiB at
+  most with `web.log.1`; never the token). It exits 0 while the web runs, 3 otherwise.
+- `claudio web --rotate-token`, `--print-url --rotate-token` and `--set-password` restart a
+  running service at once: every browser signs in again.
+- The server runs with the daemon's environment, not the terminal's that ran `claudio web`: its
+  SSH hosts use the daemon's `SSH_AUTH_SOCK` (stale or absent when the daemon was started over ssh
+  or by `systemd-run`), and `CLAUDIO_*` set in the terminal do not reach it. Run
+  `claudio daemon restart` from a shell with the environment you want, or use `--foreground`.
+- A daemon older than this claudio (or a build without the web addon) has no web service:
+  `claudio web` says so, keeps `[web]` saved, and exits 1. `claudio daemon restart` when its
+  sessions are idle, or `claudio web --foreground`.
+- **`--foreground`** keeps the old behaviour exactly: the server runs in this terminal until
+  Ctrl+C and never reads or writes `[web]`. A script or unit that ran `claudio web` as a
+  long-running process adds `--foreground`.
 
 `--remote [ADDR][:PORT]` serves TLS for other machines (below). Its value is `ADDR`,
 `ADDR:PORT`, `:PORT`, `[V6]` or `[V6]:PORT` (a bare IPv6 address needs brackets, `[::]:7337`);
@@ -65,12 +111,13 @@ Without `--remote` the server never listens on a non-loopback address. Three way
 
 ### `--remote`: TLS and a password
 
-1. **Set the password**: `claudio web --set-password`. It asks twice and never echoes; 12 to
-   1024 bytes. A running server keeps the old one until it restarts, and a restart signs every
-   browser out.
-2. **Start the server**: `claudio web --remote`. Without a password it exits before it makes a
-   certificate or opens a port. On stderr it prints `TLS certificate SHA-256 AB:CD:…` (at every
-   start), the names the certificate covers, and the token link for this machine
+1. **Set the password**: `claudio web --set-password`. It asks twice and never echoes; 8 to
+   1024 bytes. It restarts a running web service at once, which signs every browser out; a
+   `--foreground` server keeps the old password until it restarts (that restart signs every
+   browser out).
+2. **Start the server**: `claudio web --remote`. Without a password it exits before it saves
+   anything, makes a certificate or opens a port. On stderr it prints `TLS certificate SHA-256
+   AB:CD:…` (at every start), the names the certificate covers, and the token link for this machine
    (`https://127.0.0.1:7337/#t=…`); on stdout, the URL for other machines (no token). If a
    firewall (ufw, firewalld, macOS's) blocks the port, open it. Another machine may not
    resolve the host name: use `<host>.local` or the machine's address. A name the certificate
@@ -115,7 +162,8 @@ the terminal in it, and the page's own keys are not fighting the browser's.
   local ones, so a browser on another machine (or behind a proxy that keeps `Host`) never logs
   in with the token.
 - **The password** (`--remote` only). An argon2id hash (m=19 MiB, t=2, p=1) in
-  `~/.config/claudio/web-password`, mode 0600; 12 to 1024 bytes. It is never in argv, a log, a
+  `~/.config/claudio/web-password`, mode 0600; 8 to 1024 bytes (the length counts bytes). It is
+  never in argv, a log, a
   notice or the page's storage (the page clears the field before it posts it). Whoever has it
   can type into your sessions.
 - **The certificate** (`--remote` only). A self-signed P-256 server certificate (10 years) and
@@ -138,14 +186,26 @@ the terminal in it, and the page's own keys are not fighting the browser's.
   unaffected. Many addresses together can fill the 256 connection slots while an attack lasts.
 - **The token is a key.** It lives in `~/.config/claudio/web-token` (mode 0600, 64 hex),
   is reused across runs and is compared in constant time. Treat it like an SSH private key:
-  whoever has it can type into your sessions. `--rotate-token` replaces it. It is printed on
-  stdout at start and by `--print-url`, and nowhere else (never in a log, a query string, a
-  WebSocket frame or an HTTP body). With `--remote` it is printed on stderr instead, in the
-  link for this machine (when the listen address is loopback or every interface), and by
-  `--print-url`, and accepted only from local requests.
-- **Cookie.** `claudio_web=<64 hex>; HttpOnly; SameSite=Strict; Path=/`, plus `Secure` with
-  `--remote` or when `--allow-host` is set. Sessions are kept in the server's memory: 24 h idle
-  expiry, at most 64.
+  whoever has it can type into your sessions. `--rotate-token` replaces it. `claudio web` prints
+  it on stdout, from the token file, and so does `--print-url`; nothing else does (never a log,
+  the web service's included, a query string, a WebSocket frame or an HTTP body). With
+  `--remote` it is printed on stderr instead, in the link for this machine (when the listen
+  address is loopback or every interface), and by `--print-url`, and accepted only from local
+  requests.
+- **Cookie.** `claudio_web=<64 hex>; Max-Age=34560000; HttpOnly; SameSite=Strict; Path=/`, plus
+  `Secure` with `--remote` or when `--allow-host` is set. 400 days is the longest a browser keeps
+  a cookie; the server decides when a sign-in ends: 30 days unused (an open tab marks it used
+  once an hour), a sign-out, the least recently used of more than 64, or a server start after
+  the token, the password or the mode (loopback or `--remote`) changed. Sign-ins are kept in
+  `~/.config/claudio/web-sessions.json` (0600) as the SHA-256 of each cookie and its last use,
+  never a cookie: a copy of the file signs no one in. The file is bound to an epoch, the SHA-256
+  of the mode, the token and the password file as read at the server's start, so a new token or
+  password, or the other mode, empties it at the next start. A stolen cookie is worth more than
+  before (up to 30 days of disuse); sign out of a browser you no longer trust, or rotate the
+  token. Two servers on one config dir (a `--foreground` one beside the service) share the file
+  unlocked: one may forget a sign-in the other made, or bring back one the other signed out
+  (it still holds it in memory); with two, a sign-out is sure only when made on each, or with
+  `--rotate-token` and a restart of both.
 - **Host and Origin.** Every request needs an accepted `Host` (the listen address and the
   loopback names with the port, plus `--allow-host` names), else `421`. With `--remote` the
   accepted `Host`s are also any IP literal at the port, this machine's host name and
@@ -170,9 +230,10 @@ the terminal in it, and the page's own keys are not fighting the browser's.
   login refused` with the status and whether the request was local. Never values, typed text,
   tokens or passwords.
 - **State.** The web writes `web-state.json` (layout, order, each session's `launch`, still
-  with its `proxy`/`first_party` for one release, remote hosts) and
+  with its `proxy`/`first_party` for one release, remote hosts), `web-sessions.json` and
   `web-token` in the config dir (with `--set-password` and `--remote`, also `web-password` and
-  `web-tls/`). It reads the TUI's `state.json` and never writes it.
+  `web-tls/`), and `claudio web` writes `[web]` in `config.toml`. It reads the TUI's
+  `state.json` and never writes it.
 
 ## Keys
 
@@ -270,7 +331,7 @@ Rust test in `addons/web/tests/web.rs`; rows without an automated test say how t
 | Next attention | `Alt+a` | `Alt+a`, `a` | (client side) | manual check (`a` focuses the session that needs approval); approve path: `a permission prompt shows its text…` |
 | Help | `Alt+h` | `Alt+h`, `?` | (client side) | `the help has the key check and describes keys without sending them` |
 | Overview of all sessions | `Alt+g` | `Alt+g`, Board | (client side) | `the board lists sessions with their states` |
-| Login and security | n/a | token URL, cookie; password form with `--remote` | `/api/login`, `/api/me` | `logs in from the token URL and clears the fragment`; `over --remote the page asks for the password and the session works`; `web_auth_gates_api_and_ws` and the other `web_*auth*` tests; `web_remote_login_rules`, `web_remote_serves_tls_with_a_kept_certificate` |
+| Login and security | n/a | token URL, cookie; password form with `--remote`; palette: Sign out | `/api/login`, `/api/me`, `/api/logout` | `logs in from the token URL and clears the fragment`; `over --remote the page asks for the password and the session works`; `the palette signs this browser out, and its cookie stays refused`; `web_auth_gates_api_and_ws` and the other `web_*auth*` tests; `web_remote_login_rules`, `web_remote_serves_tls_with_a_kept_certificate`, `web_sign_ins_and_sign_outs_survive_a_killed_server` |
 | Layout and style | n/a | dark only ([DESIGN.md](../DESIGN.md)); Settings → Text size | `layout.save` | `the page follows DESIGN.md`; `web_order_and_layout_persist_in_web_state` |
 | Keys | `Alt+…` (see Keys) | the same Alt keys, `Ctrl+Shift+Space`, `k` | (client side) | `Ctrl+Shift+Space then k opens the palette`; `Alt+u in a terminal opens the reader and sends nothing to claude`; `Alt+a focuses the session that needs attention, Alt+Right switches session` |
 | Newline in the prompt | `Shift+Enter` | `Shift+Enter` in a terminal | `session.input` | `Shift+Enter puts ESC CR into the input, Enter submits` |
@@ -288,10 +349,11 @@ cargo test --test web --test web_manager -- --test-threads=1   # the server, wit
 make test-web                                 # node tests, then Playwright against a debug build
 ```
 
-A debug build run with `CLAUDIO_WEB_DIR=addons/web/web` serves the files from disk, so edits
-show on reload; every build embeds them gzipped (`addons/web/build.rs`). `make test-web` needs
-the Playwright browsers in `~/.cache/ms-playwright` (`npx playwright install chromium` once, in
-`addons/web/web/e2e`).
+A debug build run as `CLAUDIO_WEB_DIR=addons/web/web claudio web --foreground` serves the files
+from disk, so edits show on reload (without `--foreground` the daemon's service runs, with the
+daemon's environment); every build embeds them gzipped (`addons/web/build.rs`).
+`make test-web` needs the Playwright browsers in `~/.cache/ms-playwright`
+(`npx playwright install chromium` once, in `addons/web/web/e2e`).
 
 A file added or removed directly under `addons/web/web/` is not noticed by cargo until another
 embedded file changes or `build.rs` is touched (the directory itself is not watched because of
`````

Run: `git diff --stat`
Expected: `3 files changed, 99 insertions(+), 36 deletions(-)` (`Makefile`, `README.md`,
`docs/web.md`).

Step 2: commit
`git commit -m "docs(web): the web service, persistent sign-ins and the 8-byte password" -- docs/web.md README.md Makefile`

Interfaces this task provides to later tasks: none.

### Task 16: CLAUDE.md for the web service and the daemon's supervisor (gated on the owner)
Model: sonnet   Repo: /home/p4u/claudio   Files: `CLAUDE.md`
Goal: F1: `CLAUDE.md`'s web and daemon lines describe what Tasks 4-14 built (spec "Constraints").

**Precondition, checked first:** `git diff --quiet HEAD -- CLAUDE.md` must succeed (the owner
has committed or set aside their own CLAUDE.md rewrite). If it fails, stop and report to the
controller; never stage, stash, revert or edit around the owner's change. No patch is given:
the owner's rewrite changes the text it would apply to.

Step 1: edit `CLAUDE.md` (keep its style: terse, file paths, no new section)
- "What this is", the `claudio web` mention, and the "Web UI" section's first paragraph: a bare
  `claudio web` saves `[web]` and the local daemon runs `claudio web --service` from it
  (supervised: restarts with backoff, stops with the daemon); `--foreground` serves from the
  terminal; `claudio web stop|status`.
- `crates/core/src/addon/` → `daemon.rs`: `DaemonOps` (with `service`), `DaemonCtx` (with
  `service_reload`, `service_state`), `ServiceState`, `ClaudeFlags`.
- `crates/core/src/daemon/`: a `service.rs` bullet: the addons' services (one child per
  daemon-side addon from its config sections; `Config::claudio` in its own process group; stdin
  a pipe the daemon never writes; the `ready` line; `<runtime>/<addon>.log` ≤ 2 MiB; backoff
  1-60 s; stop = stdin closed, SIGTERM, SIGKILL after 3 s; started after the socket listens and
  stopped before the lock goes, `serve_until_shutdown` in `mod.rs`); `Config` gains
  `config_file` and `service_timing`; `freshness::EXIT_WAIT` is also the starter's lock wait.
- `addons/web/src/`: `cli.rs` (the hand-off, `stop`, `status`, `restart_if_enabled`),
  `service.rs` (`[web]` ↔ argv, the merge, the op replies), `sessions.rs` (the hashed session
  store, `web-sessions.json`, the epoch, the writer, `OpenTab`); `auth.rs`'s cookie has
  `Max-Age=34560000`; `hub_io.rs`'s plain connect for `--service`.
- The web addon's services list (A3 paragraph): it also uses `app::config_edit` and
  `daemon::test_support` in its tests.
- Non-obvious constraints (web): `claudio web --service` never prints or logs the token and
  only loads it; `CLAUDIO_TEST_SERVICE_HANG` is debug-only (like `CLAUDIO_TEST_REMOTE_PEER`);
  the suites start servers with `--foreground`; `CLAUDIO_WEB_DIR` (`assets.rs`) works with
  `claudio web --foreground` (a service has the daemon's environment).
- Test suites: the web service tests need nothing new (the isolated env); `ServiceWeb::finish`
  ends each.

Run: `git diff --stat -- CLAUDE.md`, and read the diff: only those lines change.
Expected: one file changed.

Step 2: commit
`git commit -m "docs: CLAUDE.md for the web service and the daemon's supervisor" -- CLAUDE.md`

Interfaces this task provides to later tasks: none.

## Merge and release

1. Full suites, on `feat/web-service` after Task 16 (or after Task 15 while Task 16 waits for
   the owner), each from the repo root:
   - `make check` (or its fallback sequence of the Global Constraints when only
     `crates/core/src/term/probe.rs:98,109` fail under clippy 0.1.95) → every crate's
     `test result: ok`, `rules: 0 problem(s)`.
   - `make matrix` (same fallback per cell) → the five cells pass; the core-only cell runs the
     manager suite: `ok. 42 passed`.
   - `cargo test --locked --test manager -- --test-threads=1` → `ok. 42 passed`.
   - `cargo test --locked --test web -- --test-threads=1` → `ok. 47 passed`.
   - `cargo test --locked --test web_manager -- --test-threads=1` → `ok. 4 passed`.
   - `cargo test --locked --test vocdoni_web -- --test-threads=1` → `ok. 13 passed`.
   - `cargo test --locked --test vocdoni_manager -- --test-threads=1` → `ok. 16 passed`.
   - `node --test addons/web/web/tests/*.test.mjs` → `# fail 0`.
   - `make test-web` (needs `npm ci` and Chromium; Task 7) → every spec passes, `signout`
     included, no CSP violation.
   - `grep -rn '"web"' crates/core/src/daemon crates/core/src/addon` → only
     `crates/core/src/addon/mod.rs:205` (AC10).
   - `pgrep -af "web --service"` and `pgrep -af "claudio --daemon"` show nothing of the suites
     (AC11).
   - `make static` builds and its final grep passes (neither `CLAUDIO_TEST_REMOTE_PEER` nor
     `CLAUDIO_TEST_SERVICE_HANG` in the binary).
2. Merge order: after the gate audit of the branch, `git checkout main && git merge --no-ff feat/web-service`
   (one repo, one branch).
3. Release: the repo's convention (`git log`: `01ec14a release v0.9.0`, tag `v0.9.0`) is a
   separate commit `release vX.Y.Z` that bumps `[workspace.package] version` in `Cargo.toml`
   and the lock's `claudio*` entries, then an annotated tag `vX.Y.Z`, whose push runs
   `.github/workflows/release.yml` (static builds, a GitHub release with generated notes).
   This plan bumps no version and tags nothing; the next release (by its size, a minor one,
   v0.10.0) is the owner's call. Release-note items:
   - **Behaviour change:** `claudio web` no longer stays in the foreground: it saves its
     settings in `[web]` of `config.toml`, the daemon serves them (and restarts them, and starts
     them again with the daemon), and the command exits. `claudio web --foreground` keeps the old
     behaviour; a script or unit that ran `claudio web` as a long-running process must add it.
     New: `claudio web stop`, `claudio web status`.
   - **Persisted sign-ins:** a browser stays signed in across server and daemon restarts and
     closed browsers, until 30 days unused (an open tab counts); `web-sessions.json` holds
     hashes only; a new token, password or mode signs every browser out at the next start
     (`--rotate-token`, `--set-password` restart the service at once).
   - **8-byte passwords:** a `--remote` password may now be 8 to 1024 bytes.
   - **The palette's "Sign out"** ends this browser's sign-in.
   - The web service runs with the daemon's environment (SSH agent, `CLAUDIO_*`): restart the
     daemon from the right shell, or use `--foreground`.
4. **Owner OK required before:**
   - pushing `feat/web-service` (or `main`) to any remote;
   - the merge commit into `main`, if the owner wants to review the branch first;
   - any `release vX.Y.Z` commit (the version bump);
   - creating any tag;
   - pushing any tag (it publishes a GitHub release);
   - Task 16, while the owner's own `CLAUDE.md` edit is uncommitted.
5. Post-release checks (after the owner's release): on a host with the new binary, `claudio web`
   prints its URL and exits 0; `claudio web status` exits 0 with a pid; `claudio daemon restart`
   brings the web back with a new pid and the browser still signed in; `claudio web stop` closes
   the port; a `--remote` start shows the same fingerprint in the terminal and in
   `claudio web status`.

