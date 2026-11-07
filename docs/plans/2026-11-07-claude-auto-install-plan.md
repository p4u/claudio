# Claude auto-install and manifest re-apply: implementation plan

Spec: inline below (owner-approved intent, 2026-11-07; the backoff change folded in the same day)
Class: bounded
Repos and branches: /volumes/repos/claudio → feat/claude-auto-install (base 92c62b9 = main, v0.7.0)

Every task below was implemented, built and tested in order in a scratch worktree
(`~/.cache/claudio/scratch/claude-auto-install`, detached at 92c62b9, commits a8d9881, 1c7cb03, e87d5a9, e63502b, 69ed1ab).
Each task's code is its patch, `.delivery/2026-11-07-claude-auto-install/patches/task-<N>.patch`
(a `git format-patch` of that commit), reproduced in Steps 1 and 2: Step 1 is the patch's test
part, Step 2 the rest. Apply each block with `/usr/bin/git apply <block>` from the repo root on
the tree the previous step left (or the whole task at once with
`/usr/bin/git apply .delivery/2026-11-07-claude-auto-install/patches/task-<N>.patch`), run the
commands, then commit as given. Do not hand-merge; if a block does not apply, stop and report.

Base state (92c62b9), measured in the scratch: `cargo build` OK, 0 warnings; `cargo test --locked
--bin claudio` 1100 passed; `cargo test --test manager -- --test-threads=1` 51 passed;
`env -u CLAUDIO_SSH_TEST_HOST cargo test --features diag --test remote -- --test-threads=1`
9 passed. Seen once each while building this plan, passing on rerun (the ETXTBSY family the
base already shows, not caused by this work): `daemon::host::tests::probe_runs_claude_with_the_given_path`
and `tui::extensions::host::tests::a_disabled_extension_is_not_started_and_disabling_stops_it`.
Not rustfmt-clean and not clippy-clean at the base: the snap clippy reports 74 distinct findings, among them the deny-level `src/tui/wizard.rs:193` (`nonminimal_bool`) that stops the bin-test build; the tip of this plan reports the same 74 and none on a line this branch adds.

## Spec (Bounded)

**Problem.** A machine without claude gets no install locally (`check_local_claude`: "a missing
local claude is not ours to install"). A remote host without claude is offered one only when
this machine has a claude and `remote_check` allows it, and even under `auto` it waits for a
`y`. A host whose claude is installed after it connected keeps its manifest plugin installs
failed (they ran without claude) for 10 minutes, and nothing asks it again. That 10-minute
failure memory is also too long for a transient failure (a network blip).

**Goals.**
1. `[claude] install = "auto" | "ask" | "off"`, default `auto`. When a host (local or remote)
   reports no claude in its `Welcome`, claudio runs the official installer there with the
   existing op `Msg::UpdateClaude { install: true }`: at once under `auto`, after the confirm
   prompt under `ask`, never under `off`. No local claude version is needed to decide, and the
   local host is covered. Notices: `installing claude on <host>…`, then `claude <v> installed on
   <host>; new sessions use it` or `claude install failed on <host>: <why>`. Once per host per
   run (like `claude_checked`). `update_check`/`remote_check` keep governing version updates
   only.
2. After a successful install reply (`ClaudeUpdated { ok: true, version: Some(_) }`) the TUI asks
   that host again for every enabled manifest, and the daemon forgets its failed plugin installs
   (`Manifests::forget_failures`, called by `update::run` on that success), so they run at once.
3. `--plain` and the manager behave the same; the local host is `local`. `--plain`'s session
   waits for a local install (or the answer to its prompt).
4. The installer runs with the daemon's login `PATH`, as `update.rs` does. It needs `curl` and
   `bash`; a host without them gets `claude installer needs curl` (or `bash`) and nothing runs.
   Linux and macOS, no Linux-only API.
5. (Owner change, same delivery.) Failed plugin installs back off per (manifest hash, id): retried
   15 s after the first failure, the wait doubling with each failure in a row up to 300 s; reset
   by a success, by a new manifest hash, and by `forget_failures`. Within the backoff the reason
   is the remembered one plus `(retrying in <n> s)`. The client asks a host again while any id is
   installing (every 30 s, as now) and while any id waits for a retry (when it is due), so a blip
   recovers and the details line shows `retrying in <n> s`, never a stale failure.
   `needs manual install` stays for the daemon's life. `r` still asks again. No new wire field.

**Non-goals.** Package-manager installs; a claudio-bundled claude; changing the update policy for
version upgrades; changing the installer URL; installing claude when `CLAUDIO_CLAUDE_PATH`
names a missing file (reported instead).

**Acceptance criteria.**
- AC1: unit tests in `claude_update.rs`: `auto` → `start_claude_update(install: true)` for local
  and remote without a local version; `ask` → prompt; `off` → nothing; once per host.
- AC2: unit test: a successful `ClaudeUpdated` with a version re-requests the manifests of that
  host (the effects hold the `Manifest` requests); a failed one does not.
- AC3: daemon tests: `forget_failures` clears the failure memory so the next `Manifest` request
  queues the install again (clock-independent); missing `curl` gives the clear message (a fake
  `PATH` without it); the backoff, clock-injected: retried after 15 s, doubling, the cap, reset on
  success and by `forget_failures`.
- AC4: manager test: a claude-less local host installs claude at start through a fake `curl`
  and its session runs the installed claude (see D13 for why this is safe and testable).
- AC5: unit and manager suites green; README documents `[claude] install`; every "10 min" of the
  failure memory is updated (CLAUDE.md, README, docs/extensions.md, the details line).

**Decisions taken while planning** (for the audit; none changes the intent above):
- D1. `install` reuses `UpdatePolicy` (`ask|auto|off`) with its own default, `auto`. `decide`
  loses its `install` argument (no caller passes `true` any more): `decide(policy, skipped)`; an
  install is `decide(install, false)`.
- D2. A missing claude is governed by `install` alone: `remote_check` no longer offers an install
  (the version prompt's `None` branch is gone). The install prompt, titled `Claude is not
  installed`, offers `[y] install` and `[n] not now` (no version to skip) and names `this
  machine` for `local`.
- D3. "Once per host per run" shares `claude_checked` with the version checks: a host looked at
  for an install is not also prompted for an update in that run (it has the latest). `off`
  marks nothing.
- D4. `App::claude_installing` holds the hosts with an install request in flight, so the reply
  says `installed` or `install failed`, and `--plain` can wait. The update notices are
  unchanged.
- D5. `--plain` without a local claude: `plain_try_start` waits while `claude_install_pending("local")`
  (an install in flight, or its prompt open or queued). A failed install ends the UI with the
  notice as its exit message (printed on stderr): the session cannot start. `n` at the prompt
  starts it anyway (and claude's own failure follows, as today).
- D6. The re-ask after an install is `reapply_host`: `request_manifests(host, Some(name))` for
  each enabled plugin, as `r` does. `request_manifests(host, None)` would skip the host: its
  post-connect answer is `Ready` (with the failures in it). The `Ready` answer stays until the new
  one comes. An update (not an install) re-asks nothing.
- D7. The daemon refuses `install: true` before taking the lock when `Config::claude` is a path
  (`CLAUDIO_CLAUDE_PATH` names one; a bare name is looked up like `claude` and passes), or when
  `curl` or `bash` is not an executable file on `Config::path` (else the inherited `PATH`).
  Reply: `ClaudeUpdated { version: None, ok: false, tail: <why> }`.
- D8. `forget_failures` runs only after an install whose reply is `ok` with a version. It clears
  every remembered failure (all hashes, marketplaces included); `manual` stays.
- D9. `run_claude` fails at once with `claude is not installed` when `claude_bin()` is not a
  file, without taking `Daemon::updating`. Otherwise a plugin install queued on connect (before
  the client's `UpdateClaude`) could hold the lock when `update::run`'s `try_lock` runs and the
  install would be refused as "already running".
- D10. Backoff: 15, 30, 60, 120, 240 s, then 300 s (the cap) for each further failure in a row
  (the owner's "15 s, then 30 s, 60 s, 120 s, capped at 300 s" read as doubling to the cap).
  Per (hash, key): a new hash has no entry, so it starts at 15 s. `Done` and `Manual` remove the
  failure; an expired one is kept with its count until the next outcome, so the next failure
  doubles. A marketplace known after the add step (or before it) drops its marketplace failure.
- D11. The reason within the backoff is `<why> (retrying in <n> s)`, `n` the seconds left rounded
  up (`proto::retrying`, parsed back by `proto::retry_secs`). The client reads the suffix; an
  older daemon's reasons have none and are not re-asked, as before. No new wire field.
- D12. Client re-ask (`reask_installing`): 30 s after the last ask while any id is `installing`;
  while any id is retrying, one second after the soonest retry is due, never sooner than 30 s.
  `REASK_FOR` (the 10-minute bound) and `Apply::since` are removed: the daemon's backoff (one run
  per id per ≤ 5 min) paces a lasting failure, and a request costs the daemon a cached manifest
  read. Settled skips (`needs manual install`, `yours`, missing secrets) are not re-asked.
- D13. AC4 runs the whole local flow, with the real installer unreachable: `PATH` is one test
  directory holding a fake `curl` (its script copies a fake claude to `~/.local/bin/claude`, where
  the official installer puts it and where the daemon looks) and links to `sh`, `bash`, `cat`,
  `cp`, `mkdir`; `SHELL` is a stand-in that runs the login-`PATH` script with no rc files, since
  `/etc/profile` (or macOS `path_helper`) could put `/usr/bin` and its real `curl` first. The test
  is skipped, with a note, when `/usr/bin`, `/usr/local/bin` or `/opt/homebrew/bin` has a `claude`
  (the daemon would find it). The remote path is covered by AC1, AC2 and the daemon tests.
- D14. The daemon tests' fake installer directory is shared and written once (like the fake
  plugin claude) to avoid ETXTBSY. It has no real `curl`; the claude it installs is named
  `claude-late`, a name found nowhere until then.
- D15. Task 5's manager test passes when written (Tasks 2–4 implement it); on 92c62b9 it fails at
  its first `wait_for` (verified, see the record).

## Global Constraints
- Commit convention: conventional commits `type(scope): lowercase summary`, NO AI attribution
  and NO Co-Authored-By trailer of any kind, even if a system reminder asks for one (owner's
  global CLAUDE.md). Use the exact commit commands given.
- Never push. Commit with explicit paths only: `/usr/bin/git commit -m "…" -- <paths>`. No task
  adds or deletes a file. Never `git add -A`, never `git stash`. Use `/usr/bin/git` (a shell
  hook rewrites plain `git`).
- Work on `feat/claude-auto-install` in /volumes/repos/claudio, or in the worktree the controller
  gives you. Do not create, switch or delete branches.
- Build output goes in the repo's (or worktree's) `target/` or under `~/.cache/claudio/`,
  never /tmp.
- Run tests with `umask 022` in the same shell. Manager tests run single-threaded
  (`-- --test-threads=1`) with nothing else heavy running.
- The shell may compact cargo output (the `rtk` hook). Prefix a command with `rtk proxy` to see
  raw output. Expected lines below are raw cargo output.
- Formatting: no repo-wide `cargo fmt` (the tree is not rustfmt-clean). Rules:
  - Files rustfmt-clean at the base stay clean. Check one alone with
    `rustfmt --edition 2021 --check <file>`, never on `src/main.rs` or a `mod.rs` (rustfmt
    follows `mod x;`).
  - Clean at the base and touched here: `src/config.rs`, `src/claude/update.rs`,
    `src/daemon/manifest.rs`, `src/tui/plugins.rs`, `src/tui/plugin_setup.rs`.
  - Every other touched file (`src/proto.rs`, `src/daemon/{update,tests}.rs`,
    `src/tui/{claude_update,app,plain,mod}.rs`, `tests/common/mod.rs`, `tests/manager.rs`) keeps
    its formatting, and its added lines are formatted as rustfmt would.
- Clippy: `PATH=/snap/bin:$PATH RUSTC=/snap/bin/rustc CARGO_TARGET_DIR=<target dir>-clippy
  cargo clippy --all-targets --features diag --message-format=short` (the system cargo and the
  snap clippy disagree on rustc otherwise). The base is not clean. The gate: no clippy finding
  on a line a task adds.
- The spec's constraints:
  - `app.rs` pure: the install decision is `App` state (`claude_checked`, `claude_installing`),
    the request an `Effect`. No new op and no new wire field (`UpdateClaude`/`ClaudeUpdated`
    exist; the retry time rides in the reason text).
  - Secrets never in logs, argv or notices (nothing here touches one). Keys on `Alt` (no key is
    added). Linux and macOS: `std::os::unix` only, no Linux-only API.
  - Conventional commits, no AI attribution; explicit-path commits; never push.
- Harness rules:
  - Manager (PTY) tests assert on the rendered screen model, never on raw bytes. Files a fake
    writes are fine to read.
  - Every test claudio's config has `[claude] update_check = "off"`, `remote_check = "off"` and
    `install = "off"` (the harness writes it, Task 3); only `test_missing_claude_is_installed_at_start`
    turns `install` on.
  - Fake claudes ignore `--help` and `--version`.
  - Never run the real installer and never let a test reach a real `curl` with the installer
    command: the daemon tests use `fake_installer_dir()`, the manager test its own `PATH`
    (D13, D14). Tests never touch the user's daemon, config or `~/.claude`.
- Nothing in this plan needs a real `claude`, the proxy or an SSH host.

## Review Focus
Line numbers are as the tree stands after Task 5.
- Install trigger conditions (`src/tui/claude_update.rs`):
  - `:29` `check_local_install` (only with no local claude), called at start and on a local
    reconnect (`src/tui/mod.rs:337`, `:1168`), before `--plain` starts its session.
  - `:46` `check_remote_claude`: `None` → `claude_missing`; a version is compared only when a
    local one exists.
  - `:59` `claude_missing`: `off` returns before marking; `claude_checked.insert` is the
    once-per-host rule (D3); `decide(install, false)`.
  - `:95` `start_claude_update` marks `claude_installing`; `:129` `on_claude_updated` clears it,
    words the outcome, re-asks the host's manifests after an install only (`reapply_host`), and
    in `--plain` turns a failed local install into the exit message (`:166`).
  - `:110` `claude_install_pending`; `src/tui/plugin_setup.rs:377` the `--plain` wait.
  - `src/claude/update.rs:63` `decide(policy, skipped)`; `src/config.rs:119` `install`,
    default `auto` (`:132`, and `Default`).
- Notice wording: `installing claude on <host>…`, `claude <short v> installed on <host>; new
  sessions use it`, `claude install failed on <host>: <last non-empty tail line>`, and the
  daemon's `claude installer needs curl|bash`, `CLAUDIO_CLAUDE_PATH is <path>, which the
  installer does not write: install claude there`. Update wording unchanged.
- Daemon install path (`src/daemon/update.rs`): `:45` `run`: the refusal before `try_lock`
  (`:52`), `forget_failures` only on an `ok` install with a version (`:74`); `:82`
  `cannot_install`, `:96` `on_path` (absolute dirs, a regular file with an exec bit).
- Forget-failures path and fast failure (`src/daemon/manifest.rs`): `:213` `forget_failures`
  (failed only, manual kept); `:631` `run_claude` returns `claude is not installed` before
  `updating.lock()` (D9).
- Backoff (`src/daemon/manifest.rs`): `:113` `retry_after` (15 s doubling, 300 s cap, shift
  bounded), `:119` `ceil_secs`, `:126` `Memory::failure` (None once over; the entry stays),
  `:133` `Memory::fail` (count + 1); `:263` the reason via `proto::retrying`; `:552`/`:560`/`:563`
  success and manual clear the failure, a failure counts; `:578` a remembered marketplace failure
  returns the bare reason (no double suffix); `:609`/`:613` the marketplace failure counted, and
  dropped once the name is known.
- Wire text: `src/proto.rs:465` `retrying`, `:470` `retry_secs` (suffix only; anything else is
  `None`).
- Client re-ask (`src/tui/plugins.rs`): `:460` `reask_installing` and `:694` `reask_after` (30 s
  while installing; retry + 1 s, ≥ 30 s; settled skips never), `:490` `reapply_host` (connected
  hosts only; keeps the `Ready` answer, `ask` at `:411`). `REASK_FOR` and `Apply::since` are gone.
- Harness default: `tests/common/mod.rs:651` writes `install = "off"`; `tests/manager.rs:3089`
  `test_missing_claude_is_installed_at_start` (D13: `PATH` reaches no real `curl`; the login
  shell stand-in; the skip guard).

## Task order and parallelism
| Task | Depends on | Repo/files | May run in parallel with |
|---|---|---|---|
| 1. daemon: plugin install backoff | — | daemon/manifest.rs, daemon/tests.rs, proto.rs | — |
| 2. daemon: installer checks, fast failure, forget failures | 1 | daemon/update.rs, daemon/manifest.rs, daemon/tests.rs | — |
| 3. client: `[claude] install`, `--plain` wait, harness off | 2 | config.rs, claude/update.rs, tui/{app,claude_update,mod,plain,plugin_setup}.rs, tests/common/mod.rs | — |
| 4. client: re-ask after an install and when a retry is due | 3 | proto.rs, tui/{claude_update,plugins}.rs | — |
| 5. manager test (AC4) and docs | 4 | tests/manager.rs, README.md, CLAUDE.md, docs/extensions.md | — |

Tasks are one commit each and were built in this order; the patches apply only in this order
(later tasks edit lines earlier ones add: `daemon/tests.rs` in 1 and 2, `claude_update.rs` in 3
and 4), so nothing runs in parallel.

## Tasks

### Task 1: daemon: plugin install backoff
Model: standard   Repo: /volumes/repos/claudio   Files: src/daemon/manifest.rs, src/daemon/tests.rs, src/proto.rs
Goal: failed plugin installs back off 15 s doubling to 300 s, reset by a success, with `(retrying in <n> s)` in the reason (Goal 5; AC3 backoff part).
Patch: .delivery/2026-11-07-claude-auto-install/patches/task-1.patch

Step 1: write the failing test
`````diff
diff --git a/src/daemon/manifest.rs b/src/daemon/manifest.rs
index fa2a6b1..c885a52 100644
--- a/src/daemon/manifest.rs
+++ b/src/daemon/manifest.rs
@@ -867,6 +899,38 @@ mod tests {
         assert_eq!(why, "two");
     }
 
+    #[test]
+    fn the_backoff_doubles_from_15_s_up_to_300_s() {
+        let secs: Vec<u64> = (1..=8).map(|n| retry_after(n).as_secs()).collect();
+        assert_eq!(secs, [15, 30, 60, 120, 240, 300, 300, 300]);
+        assert_eq!(retry_after(u32::MAX), RETRY_MAX);
+        assert_eq!(ceil_secs(Duration::from_millis(14_001)), 15);
+        assert_eq!(ceil_secs(Duration::from_secs(1)), 1);
+    }
+
+    #[test]
+    fn a_failure_waits_out_its_backoff_per_hash() {
+        let mut mem = Memory::default();
+        let key = |sha: &str| (sha.to_owned(), "plugin:a@m".to_owned());
+        let t0 = Instant::now();
+        let secs = Duration::from_secs;
+        mem.fail(key("h1"), "down".into(), t0);
+        assert_eq!(
+            mem.failure("h1", "plugin:a@m", t0 + secs(14)),
+            Some(("down".to_owned(), secs(1)))
+        );
+        assert_eq!(mem.failure("h1", "plugin:a@m", t0 + secs(15)), None);
+        assert_eq!(mem.failure("h2", "plugin:a@m", t0), None, "another hash");
+        // Failing again doubles the wait.
+        let t1 = t0 + secs(15);
+        mem.fail(key("h1"), "still down".into(), t1);
+        assert_eq!(
+            mem.failure("h1", "plugin:a@m", t1 + secs(29)),
+            Some(("still down".to_owned(), secs(1)))
+        );
+        assert_eq!(mem.failure("h1", "plugin:a@m", t1 + secs(30)), None);
+    }
+
     #[test]
     fn stale_zips_go_and_a_manifests_sidecar_stays() {
         let dir = std::env::temp_dir().join(format!("claudio-zips-{}", uuid::Uuid::new_v4()));
diff --git a/src/daemon/tests.rs b/src/daemon/tests.rs
index 2de9fd3..0c45d10 100644
--- a/src/daemon/tests.rs
+++ b/src/daemon/tests.rs
@@ -2258,6 +2258,13 @@ fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
         .collect()
 }
 
+/// `item` is `id` skipped for `why`, tried again in `secs` s (or one less:
+/// the clock moved on since the failure).
+fn assert_retrying(item: &(String, String), id: &str, why: &str, secs: u64) {
+    let due = (secs.saturating_sub(1)..=secs).any(|s| item.1 == proto::retrying(why, s));
+    assert!(item.0 == id && due, "{item:?}: want {id}, {why}, {secs} s");
+}
+
 fn sha256_hex(bytes: &[u8]) -> String {
     use sha2::Digest;
     format!("{:x}", sha2::Sha256::digest(bytes))
@@ -2399,50 +2406,67 @@ async fn manifest_leaves_the_members_own_alone_and_honours_disabled() {
     assert_eq!(unreadable.len(), 3, "{:?}", a.skipped);
 }
 
-/// A failed install is skipped with the CLI's reason and tried again only
-/// after 10 minutes; one that needs a person stays `needs manual install`
-/// for the daemon's life.
+/// A failed install is skipped with the CLI's reason and the seconds to
+/// its retry: 15 s after the first failure, doubling with each one in a
+/// row, and 15 s again after a success. One that needs a person stays
+/// `needs manual install` for the daemon's life.
 #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
-async fn failed_installs_retry_after_the_window_and_manual_ones_never() {
+async fn failed_installs_back_off_and_manual_ones_never_retry() {
     let h = PluginHost::start().await;
     h.set("fail-a@market", true);
     h.set("manual-b@market", true);
     let mut c = h.d.client().await;
     c.call(team(&[], &[])).await;
     h.idle().await;
-    let a = applied(c.call(team(&[], &[])).await);
     let manual = "needs manual install: claude plugin install b@market";
-    let failed = pairs(&[
-        ("plugin:a@market", "network down"),
-        ("plugin:b@market", manual),
-    ]);
-    assert_eq!(&a.skipped[..2], &failed[..]);
-    let runs = h.log().len();
-
-    // Within the window: nothing runs again.
-    h.d.daemon.manifests.advance(Duration::from_secs(9 * 60));
     let a = applied(c.call(team(&[], &[])).await);
-    assert_eq!(&a.skipped[..2], &failed[..]);
+    assert_retrying(&a.skipped[0], "plugin:a@market", "network down", 15);
+    assert_eq!(a.skipped[1], pairs(&[("plugin:b@market", manual)])[0]);
+    let installs = |h: &PluginHost, spec: &str| {
+        let line = format!("plugin install {spec} --scope user --json");
+        h.log().iter().filter(|l| **l == line).count()
+    };
+    assert_eq!(installs(&h, "a@market"), 1);
+
+    // Within the backoff nothing runs again; after it, one more run, and
+    // the next wait is twice as long.
+    h.d.daemon.manifests.advance(Duration::from_secs(14));
+    let a = applied(c.call(team(&[], &[])).await);
+    assert_retrying(&a.skipped[0], "plugin:a@market", "network down", 1);
+    h.idle().await;
+    assert_eq!(installs(&h, "a@market"), 1);
+    h.d.daemon.manifests.advance(Duration::from_secs(1));
+    let a = applied(c.call(team(&[], &[])).await);
+    assert_eq!(a.skipped[0].1, "installing");
     h.idle().await;
-    assert_eq!(h.log().len(), runs);
+    assert_eq!(installs(&h, "a@market"), 2);
+    let a = applied(c.call(team(&[], &[])).await);
+    assert_retrying(&a.skipped[0], "plugin:a@market", "network down", 30);
 
-    // After it: the failure is tried again (and now works); the manual one
-    // still waits for a person.
+    // Then it works; the manual one still waits for a person.
     h.set("fail-a@market", false);
-    h.d.daemon.manifests.advance(Duration::from_secs(2 * 60));
-    let a = applied(c.call(team(&[], &[])).await);
-    assert_eq!(a.skipped[0], pairs(&[("plugin:a@market", "installing")])[0]);
-    assert_eq!(a.skipped[1].1, manual);
+    h.d.daemon.manifests.advance(Duration::from_secs(30));
+    c.call(team(&[], &[])).await;
     h.idle().await;
     let a = applied(c.call(team(&[], &[])).await);
     assert_eq!(a.installed, ["plugin:a@market"]);
     assert_eq!(a.skipped[0], pairs(&[("plugin:b@market", manual)])[0]);
-    let installs = h
-        .log()
-        .iter()
-        .filter(|l| l.contains("install b@market"))
-        .count();
-    assert_eq!(installs, 1);
+    assert_eq!(installs(&h, "b@market"), 1);
+
+    // Uninstalled and failing again: the backoff starts over at 15 s.
+    std::fs::remove_file(h.ctl.join("installed")).unwrap();
+    std::fs::write(
+        h.claude.join("plugins/installed_plugins.json"),
+        r#"{"version":2,"plugins":{}}"#,
+    )
+    .unwrap();
+    h.set("fail-a@market", true);
+    let a = applied(c.call(team(&[], &[])).await);
+    assert_eq!(a.skipped[0].1, "installing");
+    h.idle().await;
+    let a = applied(c.call(team(&[], &[])).await);
+    assert_retrying(&a.skipped[0], "plugin:a@market", "network down", 15);
+    assert_eq!(installs(&h, "a@market"), 4);
 }
 
 /// A marketplace Claude Code files under another name than the manifest's
@@ -2456,10 +2480,8 @@ async fn a_marketplace_added_under_another_name_skips_its_plugins() {
     h.idle().await;
     let a = applied(c.call(team(&[], &[])).await);
     let why = "marketplace market: added under another name";
-    assert_eq!(
-        &a.skipped[..2],
-        &pairs(&[("plugin:a@market", why), ("plugin:b@market", why)])[..]
-    );
+    assert_retrying(&a.skipped[0], "plugin:a@market", why, 15);
+    assert_retrying(&a.skipped[1], "plugin:b@market", why, 15);
     assert_eq!(
         h.log(),
         ["plugin marketplace add acme/market --scope user --json"]
@@ -2467,8 +2489,8 @@ async fn a_marketplace_added_under_another_name_skips_its_plugins() {
 }
 
 /// An install that exits 0 but leaves the plugin out of
-/// `installed_plugins.json` is remembered as a failure for the window, not
-/// run again on every re-ask.
+/// `installed_plugins.json` is remembered as a failure for its backoff,
+/// not run again on every re-ask.
 #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
 async fn an_install_claude_does_not_list_is_remembered_as_failed() {
     let h = PluginHost::start().await;
@@ -2478,15 +2500,13 @@ async fn an_install_claude_does_not_list_is_remembered_as_failed() {
     h.idle().await;
     let a = applied(c.call(team(&[], &[])).await);
     assert_eq!(a.installed, ["plugin:b@market"]);
-    assert_eq!(
-        a.skipped[0],
-        pairs(&[("plugin:a@market", "installed but not listed")])[0]
-    );
+    let why = "installed but not listed";
+    assert_retrying(&a.skipped[0], "plugin:a@market", why, 15);
 
-    // Within the window: not run again.
-    h.d.daemon.manifests.advance(Duration::from_secs(9 * 60));
+    // Within the backoff: not run again.
+    h.d.daemon.manifests.advance(Duration::from_secs(10));
     let a = applied(c.call(team(&[], &[])).await);
-    assert_eq!(a.skipped[0].1, "installed but not listed");
+    assert_retrying(&a.skipped[0], "plugin:a@market", why, 5);
     h.idle().await;
     let installs = |h: &PluginHost| {
         h.log()
@@ -2497,7 +2517,7 @@ async fn an_install_claude_does_not_list_is_remembered_as_failed() {
     assert_eq!(installs(&h), 1);
 
     // After it: tried again.
-    h.d.daemon.manifests.advance(Duration::from_secs(2 * 60));
+    h.d.daemon.manifests.advance(Duration::from_secs(5));
     let a = applied(c.call(team(&[], &[])).await);
     assert_eq!(a.skipped[0].1, "installing");
     h.idle().await;
`````
Run: `umask 022; cargo test --locked --bin claudio daemon::`
Expected: the build fails: `` error[E0425]: cannot find value `RETRY_MAX` in this scope `` (also `retry_after`, `ceil_secs`, `proto::retrying`, `Memory::fail`; `could not compile ... due to 10 previous errors`)

Step 2: implement
`````diff
diff --git a/src/daemon/manifest.rs b/src/daemon/manifest.rs
index 3c5544b..c885a52 100644
--- a/src/daemon/manifest.rs
+++ b/src/daemon/manifest.rs
@@ -18,7 +18,10 @@
 //!
 //! What the installs did is remembered per (manifest hash, id): installed
 //! by this daemon (for its life), needing a manual install (for its life:
-//! a person has to act), or failed (retried after [`RETRY_AFTER`]).
+//! a person has to act), or failed: tried again once its backoff is over
+//! ([`RETRY_FIRST`], doubling with each failure in a row up to
+//! [`RETRY_MAX`]; a success starts it over). Meanwhile its reason says
+//! when (`proto::retrying`).
 
 use std::collections::{HashMap, HashSet, VecDeque};
 use std::io::{Read, Write};
@@ -38,7 +41,7 @@ use crate::claude::latex::sanitize;
 use crate::claude::SESSION_MARKERS;
 use crate::paths;
 use crate::plugins::manifest::{self, Manifest, Marketplace, PluginRef};
-use crate::proto::{Applied, Msg};
+use crate::proto::{retrying, Applied, Msg};
 use crate::proxy::plugins::{key_ok, name_ok, sha256_ok, test_http_ok};
 
 /// A manifest's size cap.
@@ -52,8 +55,10 @@ const MAX_CONFIG: u64 = 16 << 20;
 /// A request's caps: secret names and disabled ids.
 const MAX_SECRETS: usize = 64;
 const MAX_DISABLED: usize = 256;
-/// A failed install is tried again after this long.
-pub const RETRY_AFTER: Duration = Duration::from_secs(600);
+/// A failed install is tried again after this long, the wait doubling with
+/// each failure in a row up to [`RETRY_MAX`].
+const RETRY_FIRST: Duration = Duration::from_secs(15);
+const RETRY_MAX: Duration = Duration::from_secs(300);
 /// One `claude plugin …` run's deadline.
 const RUN_TIMEOUT: Duration = Duration::from_secs(120);
 /// Each output stream of a run is kept up to this size.
@@ -90,27 +95,51 @@ struct Dirs {
 struct Memory {
     installed: HashSet<(String, String)>,
     manual: HashMap<(String, String), String>,
-    failed: HashMap<(String, String), (String, Instant)>,
+    failed: HashMap<(String, String), Failure>,
     /// Plugin installs queued or running.
     busy: HashSet<(String, String)>,
     /// Per manifest hash, the installs left; present while its task runs.
     queues: HashMap<String, VecDeque<Job>>,
 }
 
+/// A failed install: why, when, and how many failures in a row.
+struct Failure {
+    why: String,
+    at: Instant,
+    count: u32,
+}
+
+/// The wait after the `count`-th failure in a row.
+fn retry_after(count: u32) -> Duration {
+    let doublings = count.saturating_sub(1).min(8);
+    (RETRY_FIRST * (1 << doublings)).min(RETRY_MAX)
+}
+
+/// `left` in whole seconds, rounded up.
+fn ceil_secs(left: Duration) -> u64 {
+    left.as_secs() + u64::from(left.subsec_nanos() > 0)
+}
+
 impl Memory {
-    /// The failure of `key` under `sha`, while it is recent.
-    fn failure(&mut self, sha: &str, key: &str, now: Instant) -> Option<String> {
-        let k = (sha.to_owned(), key.to_owned());
-        match self.failed.get(&k) {
-            Some((why, at)) if now.saturating_duration_since(*at) < RETRY_AFTER => {
-                Some(why.clone())
-            }
-            Some(_) => {
-                self.failed.remove(&k);
-                None
-            }
-            None => None,
-        }
+    /// The failure of `key` under `sha` and the time left until it is tried
+    /// again, while its backoff is not over.
+    fn failure(&self, sha: &str, key: &str, now: Instant) -> Option<(String, Duration)> {
+        let f = self.failed.get(&(sha.to_owned(), key.to_owned()))?;
+        let left = retry_after(f.count).checked_sub(now.saturating_duration_since(f.at))?;
+        (!left.is_zero()).then(|| (f.why.clone(), left))
+    }
+
+    /// `key` failed (again) at `now`, for `why`.
+    fn fail(&mut self, key: (String, String), why: String, now: Instant) {
+        let count = self.failed.get(&key).map_or(0, |f| f.count) + 1;
+        self.failed.insert(
+            key,
+            Failure {
+                why,
+                at: now,
+                count,
+            },
+        );
     }
 }
 
@@ -224,8 +253,8 @@ impl Manifests {
                 applied.skipped.push((id, why.clone()));
                 continue;
             }
-            if let Some(why) = mem.failure(sha, &id, now) {
-                applied.skipped.push((id, why));
+            if let Some((why, left)) = mem.failure(sha, &id, now) {
+                applied.skipped.push((id, retrying(&why, ceil_secs(left))));
                 continue;
             }
             applied.skipped.push((id, INSTALLING.to_owned()));
@@ -497,8 +526,8 @@ async fn run_queue(daemon: &Daemon, sha: &str) {
         };
         let id = job.plugin.id();
         let outcome = match install(daemon, sha, &job).await {
-            // Claude Code said yes but does not list it: remembered as a
-            // failure for the window, not run again on every re-ask.
+            // Claude Code said yes but does not list it: a failure, not run
+            // again on every re-ask.
             Outcome::Done
                 if !installed_plugins(&manifests.dirs().claude_config)
                     .contains(&job.plugin.spec()) =>
@@ -514,6 +543,7 @@ async fn run_queue(daemon: &Daemon, sha: &str) {
         mem.busy.remove(&key);
         match outcome {
             Outcome::Done => {
+                mem.failed.remove(&key);
                 mem.installed.insert(key);
             }
             Outcome::Manual => {
@@ -521,11 +551,10 @@ async fn run_queue(daemon: &Daemon, sha: &str) {
                     "needs manual install: claude plugin install {}",
                     job.plugin.spec()
                 );
+                mem.failed.remove(&key);
                 mem.manual.insert(key, why);
             }
-            Outcome::Failed(why) => {
-                mem.failed.insert(key, (why, now));
-            }
+            Outcome::Failed(why) => mem.fail(key, why, now),
         }
     }
 }
@@ -540,7 +569,7 @@ async fn install(daemon: &Daemon, sha: &str, job: &Job) -> Outcome {
     let remembered = manifests
         .memory()
         .failure(sha, &market_key, manifests.now());
-    if let Some(why) = remembered {
+    if let Some((why, _)) = remembered {
         return Outcome::Failed(why);
     }
     if !known_marketplaces(&config).contains(market) {
@@ -570,13 +599,16 @@ async fn install(daemon: &Daemon, sha: &str, job: &Job) -> Outcome {
         if let Some(why) = failed {
             let why = format!("marketplace {market}: {why}");
             let key = (sha.to_owned(), market_key);
-            manifests
-                .memory()
-                .failed
-                .insert(key, (why.clone(), manifests.now()));
+            let now = manifests.now();
+            manifests.memory().fail(key, why.clone(), now);
             return Outcome::Failed(why);
         }
     }
+    // Known: a failure to add it is over.
+    manifests
+        .memory()
+        .failed
+        .remove(&(sha.to_owned(), market_key));
     let spec = job.plugin.spec();
     run_claude(
         daemon,
diff --git a/src/proto.rs b/src/proto.rs
index b4c731b..0b25c09 100644
--- a/src/proto.rs
+++ b/src/proto.rs
@@ -460,6 +460,12 @@ pub struct Applied {
     pub dropped: Vec<String>,
 }
 
+/// The reason in [`Applied::skipped`] of a failed plugin install the daemon
+/// tries again in `secs` seconds.
+pub fn retrying(why: &str, secs: u64) -> String {
+    format!("{why} (retrying in {secs} s)")
+}
+
 /// The lenses of a claude session, in the client's registry order, and
 /// the proxy plugins' instructions. The daemon checks the caps
 /// ([`SessionExtensions::validate`]), writes the instructions to the
`````
Run: `umask 022; cargo test --locked --bin claudio`
Expected: `test result: ok. 1102 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` (1100 + 2)

Step 3: commit
`/usr/bin/git commit -m "feat(daemon): back off failed plugin installs from 15 s to 300 s" -- src/daemon/manifest.rs src/daemon/tests.rs src/proto.rs`

Interfaces this task provides to later tasks: `proto::retrying(why: &str, secs: u64) -> String`;
`Memory::failed: HashMap<(String, String), Failure>` (`Failure { why, at, count }`),
`Memory::failure(&self, sha, key, now) -> Option<(String, Duration)>`,
`Memory::fail(&mut self, key: (String, String), why: String, now: Instant)`; test helper
`assert_retrying(item, id, why, secs)` in `daemon/tests.rs`.

### Task 2: daemon: installer checks, fast failure, forget failures
Model: standard   Repo: /volumes/repos/claudio   Files: src/daemon/update.rs, src/daemon/manifest.rs, src/daemon/tests.rs
Goal: `install: true` says why it cannot run (no `curl`/`bash`, a pinned `CLAUDIO_CLAUDE_PATH`); plugin installs fail at once without a claude; a successful install forgets the failures (Goals 2 and 4; AC3).
Patch: .delivery/2026-11-07-claude-auto-install/patches/task-2.patch

Step 1: write the failing test
`````diff
diff --git a/src/daemon/tests.rs b/src/daemon/tests.rs
index 0c45d10..745f256 100644
--- a/src/daemon/tests.rs
+++ b/src/daemon/tests.rs
@@ -2152,6 +2152,47 @@ echo '{"ok":true}'
     })
 }
 
+/// The name of a claude that is installed only during the test (never a
+/// real one: no such file in any directory claude is looked up in).
+const LATE_CLAUDE: &str = "claude-late";
+
+/// A directory for a `PATH` with claude's official installer faked, shared
+/// (written once): `sh`, `bash`, `cat`, `cp`, `mkdir` and `sleep` link to
+/// the system's, and `curl` logs its arguments to `<ctl>/log` (`<ctl>`:
+/// the `PATH`'s first entry) and prints a script that copies the fake
+/// plugin claude to `<ctl>/claude-late`. No real `curl` is on such a
+/// `PATH`, so the real installer can never run.
+fn fake_installer_dir() -> &'static Path {
+    static DIR: OnceLock<PathBuf> = OnceLock::new();
+    DIR.get_or_init(|| {
+        let dir = std::env::temp_dir().join(format!("claudio-fake-installer-{}", Uuid::new_v4()));
+        std::fs::create_dir_all(&dir).unwrap();
+        for tool in ["sh", "bash", "cat", "cp", "mkdir", "sleep"] {
+            let real = ["/usr/bin", "/bin"]
+                .iter()
+                .map(|d| Path::new(d).join(tool))
+                .find(|p| p.is_file())
+                .unwrap_or_else(|| panic!("no {tool} in /usr/bin or /bin"));
+            std::os::unix::fs::symlink(real, dir.join(tool)).unwrap();
+        }
+        let script = format!(
+            "#!/bin/sh\n\
+             ctl=\"${{PATH%%:*}}\"\n\
+             echo \"curl $*\" >> \"$ctl/log\"\n\
+             echo \"cp '{}' '$ctl/{LATE_CLAUDE}'\"\n\
+             echo 'echo claude installed'\n",
+            fake_plugin_claude().display()
+        );
+        std::fs::write(dir.join("curl"), script).unwrap();
+        std::fs::set_permissions(
+            dir.join("curl"),
+            std::os::unix::fs::PermissionsExt::from_mode(0o755),
+        )
+        .unwrap();
+        dir
+    })
+}
+
 /// A `Manifest` test's daemon: the fake plugin claude, manifests cached in
 /// `<dir>/cache/plugins` (TEAM there already), Claude Code's config in
 /// `<dir>/claude`, and the fake's control dir `<dir>/ctl`.
@@ -2164,6 +2205,15 @@ struct PluginHost {
 
 impl PluginHost {
     async fn start() -> PluginHost {
+        Self::start_with(|ctl| {
+            let path = format!("{}:/usr/bin:/bin", ctl.display());
+            (fake_plugin_claude().to_path_buf(), path)
+        })
+        .await
+    }
+
+    /// With the claude and `PATH` that `setup` gives for the control dir.
+    async fn start_with(setup: impl FnOnce(&Path) -> (PathBuf, String)) -> PluginHost {
         let dir = TestDaemon::new_dir();
         let (cache, claude, ctl) = (
             dir.join("cache/plugins"),
@@ -2179,9 +2229,10 @@ impl PluginHost {
             format!("{}\n", sha256_hex(TEAM.as_bytes())),
         )
         .unwrap();
+        let (bin, path) = setup(&ctl);
         let config = Config {
-            claude: fake_plugin_claude().to_path_buf(),
-            path: Some(format!("{}:/usr/bin:/bin", ctl.display()).into()),
+            claude: bin,
+            path: Some(path.into()),
             ..TestDaemon::config(&dir)
         };
         let d = TestDaemon::start_with_config(dir, config);
@@ -2599,6 +2650,107 @@ async fn installs_and_claude_update_take_turns() {
     ));
 }
 
+/// Claude installed on the host: the failures are forgotten, so the next
+/// request queues those installs at once; a manual one still waits.
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn forgotten_failures_install_on_the_next_request() {
+    let h = PluginHost::start().await;
+    h.set("fail-a@market", true);
+    h.set("manual-b@market", true);
+    let mut c = h.d.client().await;
+    c.call(team(&[], &[])).await;
+    h.idle().await;
+    let a = applied(c.call(team(&[], &[])).await);
+    assert_retrying(&a.skipped[0], "plugin:a@market", "network down", 15);
+    h.set("fail-a@market", false);
+    h.d.daemon.manifests.forget_failures();
+    let a = applied(c.call(team(&[], &[])).await);
+    let manual = "needs manual install: claude plugin install b@market";
+    assert_eq!(
+        &a.skipped[..2],
+        &pairs(&[
+            ("plugin:a@market", "installing"),
+            ("plugin:b@market", manual)
+        ])[..]
+    );
+    h.idle().await;
+    let a = applied(c.call(team(&[], &[])).await);
+    assert_eq!(a.installed, ["plugin:a@market"]);
+}
+
+/// A claude installed after the plugins' first request: until then each
+/// plugin install fails at once (`claude is not installed`, never holding
+/// the lock the installer takes). The installer (a fake `curl`: no real
+/// one is on this `PATH`) puts claude in place, the failures are
+/// forgotten, and the next request installs the plugins.
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn installing_claude_forgets_the_plugin_failures() {
+    let tools = fake_installer_dir();
+    let h = PluginHost::start_with(|ctl| {
+        let path = format!("{}:{}", ctl.display(), tools.display());
+        (PathBuf::from(LATE_CLAUDE), path)
+    })
+    .await;
+    let mut c = h.d.client().await;
+    c.call(team(&[], &[])).await;
+    h.idle().await;
+    let a = applied(c.call(team(&[], &[])).await);
+    let why = "marketplace market: claude is not installed";
+    assert_retrying(&a.skipped[0], "plugin:a@market", why, 15);
+    assert_retrying(&a.skipped[1], "plugin:b@market", why, 15);
+    assert!(h.log().is_empty(), "{:?}", h.log());
+
+    match c.call(Msg::UpdateClaude { install: true }).await {
+        Msg::ClaudeUpdated { version, ok, tail } => {
+            assert!(ok, "{tail}");
+            assert_eq!(version.as_deref(), Some("9.9.9 (Claude Code)"));
+            assert_eq!(tail, "claude installed");
+        }
+        other => panic!("expected ClaudeUpdated, got {other:?}"),
+    }
+    assert_eq!(h.log(), ["curl -fsSL https://claude.ai/install.sh"]);
+    let a = applied(c.call(team(&[], &[])).await);
+    assert_eq!(a.skipped[0].1, "installing");
+    h.idle().await;
+    let a = applied(c.call(team(&[], &[])).await);
+    assert_eq!(a.installed, ["plugin:a@market", "plugin:b@market"]);
+}
+
+/// The installer does not run without `curl` on the daemon's `PATH`, nor
+/// for a claude that `CLAUDIO_CLAUDE_PATH` pins: the reply says why.
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn install_says_why_it_cannot_run() {
+    let dir = TestDaemon::new_dir();
+    let empty = dir.join("bin");
+    std::fs::create_dir_all(&empty).unwrap();
+    let config = Config {
+        claude: PathBuf::from(LATE_CLAUDE),
+        path: Some(empty.into_os_string()),
+        ..TestDaemon::config(&dir)
+    };
+    let d = TestDaemon::start_with_config(dir, config);
+    let mut c = d.client().await;
+    assert_eq!(
+        c.call(Msg::UpdateClaude { install: true }).await,
+        Msg::ClaudeUpdated {
+            version: None,
+            ok: false,
+            tail: "claude installer needs curl".into()
+        }
+    );
+
+    let pinned = TestDaemon::start_in(TestDaemon::new_dir()).await;
+    let mut c = pinned.client().await;
+    match c.call(Msg::UpdateClaude { install: true }).await {
+        Msg::ClaudeUpdated {
+            ok: false, tail, ..
+        } => {
+            assert!(tail.starts_with("CLAUDIO_CLAUDE_PATH is /"), "{tail}");
+        }
+        other => panic!("expected ClaudeUpdated, got {other:?}"),
+    }
+}
+
 /// The fake's `--help` lists the flags: nothing is dropped.
 #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
 async fn applied_names_the_flags_this_claude_lacks() {
diff --git a/src/daemon/update.rs b/src/daemon/update.rs
index e5ff367..5838982 100644
--- a/src/daemon/update.rs
+++ b/src/daemon/update.rs
@@ -148,6 +196,47 @@ fn printable(bytes: &[u8]) -> String {
 mod tests {
     use super::*;
 
+    #[test]
+    fn the_installer_needs_curl_and_bash_and_no_pinned_claude() {
+        let dir = std::env::temp_dir().join(format!("claudio-tools-{}", uuid::Uuid::new_v4()));
+        std::fs::create_dir_all(&dir).unwrap();
+        let tool = |name: &str, mode: u32| {
+            std::fs::write(dir.join(name), "#!/bin/sh\n").unwrap();
+            std::fs::set_permissions(dir.join(name), std::fs::Permissions::from_mode(mode))
+                .unwrap();
+        };
+        let claude = Path::new("claude");
+        let path = std::env::join_paths([Path::new("relative"), &dir]).unwrap();
+        tool("bash", 0o755);
+        assert_eq!(
+            cannot_install(claude, Some(&path)).as_deref(),
+            Some("claude installer needs curl")
+        );
+        tool("curl", 0o644);
+        assert_eq!(
+            cannot_install(claude, Some(&path)).as_deref(),
+            Some("claude installer needs curl"),
+            "not executable"
+        );
+        tool("curl", 0o755);
+        assert_eq!(cannot_install(claude, Some(&path)), None);
+        std::fs::remove_file(dir.join("bash")).unwrap();
+        assert_eq!(
+            cannot_install(claude, Some(&path)).as_deref(),
+            Some("claude installer needs bash")
+        );
+        assert_eq!(
+            cannot_install(claude, None).as_deref(),
+            Some("claude installer needs curl")
+        );
+        let pinned = cannot_install(Path::new("/opt/claude/bin/claude"), Some(&path));
+        assert_eq!(
+            pinned.as_deref(),
+            Some("CLAUDIO_CLAUDE_PATH is /opt/claude/bin/claude, which the installer does not write: install claude there")
+        );
+        std::fs::remove_dir_all(&dir).unwrap();
+    }
+
     #[test]
     fn output_is_stripped_of_escapes_and_control_characters() {
         assert_eq!(
`````
Run: `umask 022; cargo test --locked --bin claudio daemon::`
Expected: the build fails: `` error[E0599]: no function or associated item named `from_mode` found for struct `Permissions` in the current scope `` (the import is Step 2's), then `cannot_install` and `` no method named `forget_failures` found for struct `Manifests` `` (`due to 8 previous errors`)

Step 2: implement
`````diff
diff --git a/src/daemon/manifest.rs b/src/daemon/manifest.rs
index c885a52..2508a84 100644
--- a/src/daemon/manifest.rs
+++ b/src/daemon/manifest.rs
@@ -208,6 +208,12 @@ impl Manifests {
         !self.memory().queues.is_empty()
     }
 
+    /// Forget every failed install, so the next request tries each at once:
+    /// claude was just installed on this host. Manual installs stay.
+    pub(crate) fn forget_failures(&self) {
+        self.memory().failed.clear();
+    }
+
     /// Move this daemon's clock forward by `by`.
     #[cfg(test)]
     pub(crate) fn advance(&self, by: Duration) {
@@ -620,9 +626,15 @@ async fn install(daemon: &Daemon, sha: &str, job: &Job) -> Outcome {
 /// Run `claude <args>` while holding `Daemon::updating`: the daemon's
 /// claude on its `PATH`, stdin `/dev/null`, the session markers scrubbed,
 /// git never prompting, in a process group of its own killed on timeout.
+/// Without a claude it fails at once, never holding the lock the install
+/// of claude takes.
 async fn run_claude(daemon: &Daemon, args: &[&str]) -> Outcome {
+    let claude = daemon.config.claude_bin();
+    if !claude.is_file() {
+        return Outcome::Failed("claude is not installed".to_owned());
+    }
     let _held = daemon.updating.lock().await;
-    let mut cmd = Command::new(daemon.config.claude_bin());
+    let mut cmd = Command::new(claude);
     cmd.args(args);
     for marker in SESSION_MARKERS {
         cmd.env_remove(marker);
diff --git a/src/daemon/update.rs b/src/daemon/update.rs
index a976911..5838982 100644
--- a/src/daemon/update.rs
+++ b/src/daemon/update.rs
@@ -1,12 +1,19 @@
 //! `UpdateClaude`: run `claude update` (or the official installer when claude
 //! is missing) on this host, then re-probe `claude --version`.
 //!
+//! The installer needs `curl` and `bash` on the daemon's `PATH` (Linux and
+//! macOS have both as a rule); without one, or with `CLAUDIO_CLAUDE_PATH`
+//! naming a path (the installer does not write there), the reply says why
+//! and nothing runs. After an install, the plugin installs that failed are
+//! tried again at once (`Manifests::forget_failures`).
+//!
 //! The command runs as a task of its own, never inline in a client loop (the
 //! reply is queued like `Attach`'s), under a deadline and with its output
 //! bounded to the last few KiB. Running sessions are never touched: they keep
 //! the binary they started with, new ones pick up the new one.
 
 use std::ffi::OsStr;
+use std::os::unix::fs::PermissionsExt;
 use std::path::Path;
 use std::process::Stdio;
 use std::time::Duration;
@@ -28,13 +35,28 @@ const KILL_GRACE: Duration = Duration::from_secs(2);
 /// Output kept, counted from the end.
 const TAIL_BYTES: usize = 4096;
 
-/// Anthropic's official installer. Only ever run after an explicit `y`.
+/// Anthropic's official installer, run where claude is missing (as
+/// `[claude] install` says: at once, after a `y`, or never).
 const INSTALLER: &str = "{ curl -fsSL https://claude.ai/install.sh | bash; } 2>&1";
 
 /// Run the update (or the install) and answer `ClaudeUpdated`. Only one runs
 /// at a time per daemon, and never beside a plugin install (which waits
 /// for the same lock).
 pub async fn run(daemon: &Daemon, install: bool) -> Msg {
+    if install {
+        let path = daemon
+            .config
+            .path
+            .clone()
+            .or_else(|| std::env::var_os("PATH"));
+        if let Some(why) = cannot_install(&daemon.config.claude, path.as_deref()) {
+            return Msg::ClaudeUpdated {
+                version: None,
+                ok: false,
+                tail: why,
+            };
+        }
+    }
     let Ok(_running) = daemon.updating.try_lock() else {
         return Msg::Error {
             message: "a claude update or plugin install is already running on this host".into(),
@@ -46,11 +68,37 @@ pub async fn run(daemon: &Daemon, install: bool) -> Msg {
         Ok(done) => done,
         Err(e) => (false, e),
     };
-    Msg::ClaudeUpdated {
-        version: daemon.refresh_host().await.claude.map(|c| c.version),
-        ok,
-        tail,
+    let version = daemon.refresh_host().await.claude.map(|c| c.version);
+    if install && ok && version.is_some() {
+        // Plugin installs that failed for want of claude go at once.
+        daemon.manifests.forget_failures();
     }
+    Msg::ClaudeUpdated { version, ok, tail }
+}
+
+/// Why the installer cannot run on this host: `claude` (the daemon's
+/// setting) is a path, so `CLAUDIO_CLAUDE_PATH` pins one the installer
+/// does not write; or `curl` or `bash` is not on `path`.
+fn cannot_install(claude: &Path, path: Option<&OsStr>) -> Option<String> {
+    if claude.components().count() > 1 {
+        return Some(format!(
+            "CLAUDIO_CLAUDE_PATH is {}, which the installer does not write: install claude there",
+            claude.display()
+        ));
+    }
+    ["curl", "bash"]
+        .into_iter()
+        .find(|tool| !on_path(tool, path))
+        .map(|tool| format!("claude installer needs {tool}"))
+}
+
+/// Whether an executable `name` is in one of `path`'s directories.
+fn on_path(name: &str, path: Option<&OsStr>) -> bool {
+    path.into_iter()
+        .flat_map(std::env::split_paths)
+        .filter(|dir| dir.is_absolute())
+        .filter_map(|dir| std::fs::metadata(dir.join(name)).ok())
+        .any(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
 }
 
 /// `(succeeded, output tail)`, or a description of why it could not run.
`````
Run: `umask 022; cargo test --locked --bin claudio`
Expected: `test result: ok. 1106 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` (+1 in `update.rs`, +3 in `daemon/tests.rs`)

Step 3: commit
`/usr/bin/git commit -m "feat(daemon): check the installer's needs and retry plugins once claude is installed" -- src/daemon/update.rs src/daemon/manifest.rs src/daemon/tests.rs`

Interfaces this task provides to later tasks: `Manifests::forget_failures(&self)`;
`update::run` replies `ClaudeUpdated { version: None, ok: false, tail: "claude installer needs curl" }`
(or `bash`, or the `CLAUDIO_CLAUDE_PATH` sentence) without running anything.

### Task 3: client: `[claude] install`, `--plain` wait, harness off
Model: standard   Repo: /volumes/repos/claudio   Files: src/config.rs, src/claude/update.rs, src/tui/app.rs, src/tui/claude_update.rs, src/tui/mod.rs, src/tui/plain.rs, src/tui/plugin_setup.rs, tests/common/mod.rs
Goal: a host without claude, local included, gets the installer as `install` says, once per run, with the notices; `--plain` waits for it; every test claudio has `install = "off"` (Goals 1 and 3; AC1).
Patch: .delivery/2026-11-07-claude-auto-install/patches/task-3.patch

Step 1: write the failing test
`````diff
diff --git a/src/claude/update.rs b/src/claude/update.rs
index 102721d..cb96d43 100644
--- a/src/claude/update.rs
+++ b/src/claude/update.rs
@@ -203,16 +202,12 @@ mod tests {
     fn policy_decides_prompt_update_or_nothing() {
         use Decision as D;
         use UpdatePolicy as P;
-        assert_eq!(decide(P::Ask, false, false), D::Ask);
-        assert_eq!(decide(P::Ask, false, true), D::Nothing, "skipped version");
-        assert_eq!(decide(P::Auto, false, false), D::Run);
-        assert_eq!(decide(P::Auto, false, true), D::Run, "auto ignores skips");
-        assert_eq!(decide(P::Off, false, false), D::Nothing);
-        assert_eq!(decide(P::Off, true, false), D::Nothing);
-        // Installing needs consent even under auto.
-        assert_eq!(decide(P::Auto, true, false), D::Ask);
-        assert_eq!(decide(P::Auto, true, true), D::Nothing);
-        assert_eq!(decide(P::Ask, true, false), D::Ask);
+        assert_eq!(decide(P::Ask, false), D::Ask);
+        assert_eq!(decide(P::Ask, true), D::Nothing, "skipped version");
+        assert_eq!(decide(P::Auto, false), D::Run);
+        assert_eq!(decide(P::Auto, true), D::Run, "auto ignores skips");
+        assert_eq!(decide(P::Off, false), D::Nothing);
+        assert_eq!(decide(P::Off, true), D::Nothing);
     }
 
     #[test]
diff --git a/src/config.rs b/src/config.rs
index 8313fbc..9c50f1d 100644
--- a/src/config.rs
+++ b/src/config.rs
@@ -643,6 +655,7 @@ mod tests {
         assert!(cfg.keys.is_empty());
         assert!(cfg.claude.allow_skip_permissions);
         assert_eq!(cfg.claude.update_check, UpdatePolicy::Ask);
+        assert_eq!(cfg.claude.install, UpdatePolicy::Auto);
         let _ = std::fs::remove_dir_all(&dir);
     }
 
@@ -653,13 +666,14 @@ mod tests {
         let path = dir.join("config.toml");
         std::fs::write(
             &path,
-            "[claude]\nallow_skip_permissions = false\nupdate_check = \"auto\"\nremote_check = \"off\"\n",
+            "[claude]\nallow_skip_permissions = false\nupdate_check = \"auto\"\nremote_check = \"off\"\ninstall = \"ask\"\n",
         )
         .unwrap();
         let cfg = load_from(&path);
         assert!(!cfg.claude.allow_skip_permissions);
         assert_eq!(cfg.claude.update_check, UpdatePolicy::Auto);
         assert_eq!(cfg.claude.remote_check, UpdatePolicy::Off);
+        assert_eq!(cfg.claude.install, UpdatePolicy::Ask);
         let _ = std::fs::remove_dir_all(&dir);
     }
 
@@ -674,6 +688,7 @@ mod tests {
         // The other keys keep their defaults.
         assert!(claude.allow_skip_permissions);
         assert_eq!(claude.update_check, UpdatePolicy::Ask);
+        assert_eq!(claude.install, UpdatePolicy::Auto);
     }
 
     #[test]
diff --git a/src/tui/claude_update.rs b/src/tui/claude_update.rs
index aac1064..9b5ba62 100644
--- a/src/tui/claude_update.rs
+++ b/src/tui/claude_update.rs
@@ -166,6 +239,7 @@ mod tests {
         let mut config = test_support::config();
         config.claude.update_check = policy;
         config.claude.remote_check = policy;
+        config.claude.install = policy;
         App::new(AppConfig {
             local_claude: Some(LOCAL.into()),
             ..config
@@ -246,7 +320,10 @@ mod tests {
     fn missing_remote_claude_offers_the_installer() {
         let mut app = new_app(UpdatePolicy::Ask);
         app.check_remote_claude("devbox", None);
+        assert_eq!(prompt(&app).title, "Claude is not installed");
+        assert_eq!(prompt(&app).choices.len(), 2, "no version to skip");
         assert_eq!(prompt(&app).choices[0].label, "install");
+        assert!(prompt(&app).text.contains("not installed on devbox"));
         assert!(prompt(&app).text.contains("claude.ai/install.sh"));
         arm(&mut app);
         press(&mut app, KeyCode::Char('y'));
@@ -320,7 +397,7 @@ mod tests {
     }
 
     #[test]
-    fn auto_updates_without_asking_but_never_installs_unasked() {
+    fn auto_updates_and_installs_without_asking() {
         let mut app = new_app(UpdatePolicy::Auto);
         app.check_remote_claude("devbox", Some("2.1.280"));
         assert!(app.modal.is_none());
@@ -331,10 +408,64 @@ mod tests {
         assert!(notice(&app).contains("updating claude on devbox"));
 
         app.check_remote_claude("bare", None);
-        assert!(has_prompt(&app), "installing needs a y");
+        assert!(app.modal.is_none());
+        assert_eq!(
+            update_requests(&mut app),
+            [("bare".to_owned(), Msg::UpdateClaude { install: true })]
+        );
+        assert_eq!(notice(&app), "installing claude on bare…");
+    }
+
+    #[test]
+    fn a_missing_claude_is_installed_with_none_here_and_on_this_machine() {
+        let mut app = App::new(test_support::config());
+        assert_eq!(app.claude_policy.install, UpdatePolicy::Auto, "the default");
+        assert_eq!(app.local_claude, None);
+        app.check_local_install();
+        assert_eq!(notice(&app), "installing claude on local…");
+        app.check_remote_claude("devbox", None);
+        assert_eq!(
+            update_requests(&mut app),
+            [
+                ("local".to_owned(), Msg::UpdateClaude { install: true }),
+                ("devbox".to_owned(), Msg::UpdateClaude { install: true }),
+            ]
+        );
+        assert!(app.claude_install_pending("local") && app.claude_install_pending("devbox"));
+        // Nothing here to compare an installed one with.
+        app.check_remote_claude("old", Some("1.0.0"));
+        assert!(app.modal.is_none() && update_requests(&mut app).is_empty());
+
+        // This machine has claude: nothing to install.
+        let mut app = new_app(UpdatePolicy::Auto);
+        app.check_local_install();
         assert!(update_requests(&mut app).is_empty());
     }
 
+    #[test]
+    fn install_has_its_own_policy() {
+        // Installs asked about with every version check off...
+        let mut app = new_app(UpdatePolicy::Off);
+        app.claude_policy.install = UpdatePolicy::Ask;
+        app.check_remote_claude("devbox", Some("2.1.280"));
+        assert!(app.modal.is_none());
+        app.check_remote_claude("bare", None);
+        assert_eq!(prompt(&app).choices[0].label, "install");
+        assert!(app.claude_install_pending("bare"), "the question waits");
+        arm(&mut app);
+        press(&mut app, KeyCode::Char('n'));
+        assert!(!app.claude_install_pending("bare"));
+        assert!(update_requests(&mut app).is_empty());
+
+        // ...and none with them on.
+        let mut app = new_app(UpdatePolicy::Ask);
+        app.claude_policy.install = UpdatePolicy::Off;
+        app.local_claude = None;
+        app.check_local_install();
+        app.check_remote_claude("bare", None);
+        assert!(app.modal.is_none() && app.take_effects().is_empty());
+    }
+
     #[test]
     fn off_and_up_to_date_do_nothing() {
         let mut off = new_app(UpdatePolicy::Off);
@@ -358,6 +489,17 @@ mod tests {
         press(&mut app, KeyCode::Char('n'));
         app.check_remote_claude("devbox", Some("2.1.280")); // reconnect
         assert!(app.modal.is_none());
+
+        let mut app = App::new(test_support::config());
+        app.check_local_install();
+        app.check_remote_claude("bare", None);
+        app.on_reply(
+            ReplyTo::ClaudeUpdate("bare".into()),
+            Err(io::Error::other("connection lost")),
+        );
+        app.check_remote_claude("bare", None); // reconnect
+        app.check_local_install(); // the local daemon restarted
+        assert_eq!(update_requests(&mut app).len(), 2);
     }
 
     #[test]
@@ -433,4 +575,35 @@ mod tests {
         app.on_reply(ReplyTo::ClaudeUpdate("local".into()), Ok(local));
         assert_eq!(app.local_claude.as_deref(), Some("2.2.0"));
     }
+
+    #[test]
+    fn install_outcomes_say_installed() {
+        let mut app = App::new(test_support::config());
+        app.check_local_install();
+        let ok = Msg::ClaudeUpdated {
+            version: Some("2.1.296 (Claude Code)".into()),
+            ok: true,
+            tail: "Installing\nclaude installed\n".into(),
+        };
+        app.on_reply(ReplyTo::ClaudeUpdate("local".into()), Ok(ok));
+        assert_eq!(
+            notice(&app),
+            "claude 2.1.296 installed on local; new sessions use it"
+        );
+        assert_eq!(app.local_claude.as_deref(), Some("2.1.296 (Claude Code)"));
+        assert!(!app.claude_install_pending("local"));
+
+        app.check_remote_claude("devbox", None);
+        let failed = Msg::ClaudeUpdated {
+            version: None,
+            ok: false,
+            tail: "claude installer needs curl".into(),
+        };
+        app.on_reply(ReplyTo::ClaudeUpdate("devbox".into()), Ok(failed));
+        assert_eq!(
+            notice(&app),
+            "claude install failed on devbox: claude installer needs curl"
+        );
+        assert!(!app.quit, "only --plain leaves");
+    }
 }
diff --git a/src/tui/plain.rs b/src/tui/plain.rs
index 1fef507..4d56f29 100644
--- a/src/tui/plain.rs
+++ b/src/tui/plain.rs
@@ -194,6 +194,82 @@ mod tests {
         assert!(app.modal.is_none(), "no wizard");
     }
 
+    /// No claude here: the session waits for its install (or the answer to
+    /// the question), and a failed install ends the UI with the reason.
+    #[test]
+    fn waits_for_the_claude_install_and_leaves_if_it_fails() {
+        use crate::config::UpdatePolicy;
+        use crate::tui::test_support::press;
+        use crossterm::event::KeyCode;
+        let new = |install: UpdatePolicy| {
+            let mut notices = Vec::new();
+            let mut app = App::new(AppConfig {
+                mode: Mode::Plain,
+                keymap: Keymap::build_plain(&Default::default(), &mut notices),
+                notify: false,
+                proxy_override: ProxyChoice::Direct,
+                ..test_support::config()
+            });
+            app.claude_policy.install = install;
+            app.check_local_install();
+            app.start_plain(PlainStart {
+                id: Uuid::new_v4(),
+                args: Vec::new(),
+                cwd: "/work".into(),
+            });
+            app
+        };
+        let spawns = |app: &mut App| {
+            app.take_effects()
+                .iter()
+                .filter(|e| {
+                    matches!(
+                        e,
+                        Effect::Request {
+                            msg: Msg::Spawn(_),
+                            ..
+                        }
+                    )
+                })
+                .count()
+        };
+        let reply = |app: &mut App, ok: bool, tail: &str| {
+            let msg = Msg::ClaudeUpdated {
+                version: ok.then(|| "2.1.296 (Claude Code)".to_owned()),
+                ok,
+                tail: tail.to_owned(),
+            };
+            app.on_reply(ReplyTo::ClaudeUpdate("local".into()), Ok(msg));
+        };
+
+        let mut app = new(UpdatePolicy::Auto);
+        assert_eq!(spawns(&mut app), 0);
+        app.on_tick();
+        assert_eq!(spawns(&mut app), 0, "still installing");
+        reply(&mut app, true, "");
+        app.on_tick();
+        assert_eq!(spawns(&mut app), 1);
+
+        let mut app = new(UpdatePolicy::Auto);
+        reply(&mut app, false, "claude installer needs curl");
+        assert!(app.quit);
+        assert_eq!(
+            app.exit.message.as_deref(),
+            Some("claude install failed on local: claude installer needs curl")
+        );
+        assert_eq!(spawns(&mut app), 0);
+
+        let mut app = new(UpdatePolicy::Ask);
+        assert!(matches!(app.modal, Some(Modal::Confirm(_))));
+        for _ in 0..4 {
+            app.on_tick();
+        }
+        assert_eq!(spawns(&mut app), 0, "the question waits");
+        app.on_terminal(press(KeyCode::Char('n')));
+        app.on_tick();
+        assert_eq!(spawns(&mut app), 1);
+    }
+
     #[test]
     fn renders_the_pane_full_screen_without_tab_or_status_bar() {
         let (mut app, id) = plain_app(&[]);
diff --git a/tests/common/mod.rs b/tests/common/mod.rs
index 23904dd..1294578 100644
--- a/tests/common/mod.rs
+++ b/tests/common/mod.rs
@@ -641,13 +641,14 @@ pub struct ManagerHarness {
 }
 
 /// The `config.toml` every test claudio starts with: no claude version
-/// checks, whose prompts would capture the keys a test types.
+/// checks, whose prompts would capture the keys a test types, and no
+/// claude installs (a test that wants one turns `install` on).
 pub fn write_test_config(config_home: &std::path::Path) {
     let dir = config_home.join("claudio");
     fs::create_dir_all(&dir).expect("create config dir");
     fs::write(
         dir.join("config.toml"),
-        "[claude]\nupdate_check = \"off\"\nremote_check = \"off\"\n",
+        "[claude]\nupdate_check = \"off\"\nremote_check = \"off\"\ninstall = \"off\"\n",
     )
     .expect("write test config.toml");
 }
`````
Run: `umask 022; cargo test --locked --bin claudio claude`
Expected: the build fails: `` error[E0061]: this function takes 3 arguments but 2 arguments were supplied `` (`decide`), then `` no field `install` on type `config::ClaudeSection` `` and `` no method named `check_local_install` `` / `claude_install_pending` (`due to 26 previous errors`)

Step 2: implement
`````diff
diff --git a/src/claude/update.rs b/src/claude/update.rs
index c7aaf93..cb96d43 100644
--- a/src/claude/update.rs
+++ b/src/claude/update.rs
@@ -58,15 +58,14 @@ pub enum Decision {
     Run,
 }
 
-/// Apply `policy`. `skipped` is the user's "skip this version" for this host.
-/// Installing claude where it is missing always needs a `y`, even under
-/// `auto`.
-pub fn decide(policy: UpdatePolicy, install: bool, skipped: bool) -> Decision {
+/// Apply `policy` (`update_check`, `remote_check` or `install`). `skipped`
+/// is the user's "skip this version" for this host; an install has none.
+pub fn decide(policy: UpdatePolicy, skipped: bool) -> Decision {
     match policy {
         UpdatePolicy::Off => Decision::Nothing,
-        UpdatePolicy::Auto if !install => Decision::Run,
-        UpdatePolicy::Auto | UpdatePolicy::Ask if skipped => Decision::Nothing,
-        UpdatePolicy::Auto | UpdatePolicy::Ask => Decision::Ask,
+        UpdatePolicy::Auto => Decision::Run,
+        UpdatePolicy::Ask if skipped => Decision::Nothing,
+        UpdatePolicy::Ask => Decision::Ask,
     }
 }
 
diff --git a/src/config.rs b/src/config.rs
index 30e00b6..9c50f1d 100644
--- a/src/config.rs
+++ b/src/config.rs
@@ -13,6 +13,7 @@
 //! allow_skip_permissions = true   # pass --allow-dangerously-skip-permissions
 //! update_check = "ask"            # local claude vs its release channel
 //! remote_check = "ask"            # remote claude vs the local version
+//! install = "auto"                # claude where a host has none
 //! latex = true                    # ask claude to wrap math in <latex> tags
 //!                                 # (read at daemon start: `claudio daemon restart`)
 //!
@@ -83,7 +84,8 @@ impl Default for UpdateSection {
     }
 }
 
-/// What to do when a claude install is out of date.
+/// What to do about a claude that is out of date (or, for `install`,
+/// missing).
 #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
 #[serde(rename_all = "lowercase")]
 pub enum UpdatePolicy {
@@ -110,6 +112,11 @@ pub struct ClaudeSection {
     /// Compare a remote host's claude with the local version on connect.
     #[serde(default)]
     pub remote_check: UpdatePolicy,
+    /// Install claude with the official installer on a host that has none
+    /// (this machine or an SSH host): without asking (`auto`, the default),
+    /// after a `y` (`ask`), or never (`off`).
+    #[serde(default = "default_install")]
+    pub install: UpdatePolicy,
     /// Tell every claude session this host's daemon starts to wrap math in
     /// `<latex>…</latex>`, which the manager renders. On unless turned off.
     /// Read when the daemon starts, from the config of the host that runs
@@ -122,12 +129,17 @@ fn default_true() -> bool {
     true
 }
 
+fn default_install() -> UpdatePolicy {
+    UpdatePolicy::Auto
+}
+
 impl Default for ClaudeSection {
     fn default() -> Self {
         ClaudeSection {
             allow_skip_permissions: true,
             update_check: UpdatePolicy::Ask,
             remote_check: UpdatePolicy::Ask,
+            install: UpdatePolicy::Auto,
             latex: true,
         }
     }
diff --git a/src/tui/app.rs b/src/tui/app.rs
index da57ec9..a88cafb 100644
--- a/src/tui/app.rs
+++ b/src/tui/app.rs
@@ -355,6 +355,8 @@ pub struct App {
     pub claude_skipped: HashMap<String, String>,
     /// Hosts whose claude was already checked in this run.
     pub(super) claude_checked: HashSet<String>,
+    /// Hosts where an install of claude is under way.
+    pub(super) claude_installing: HashSet<String>,
     /// Prompts and plugin setup screens waiting for the open modal to close.
     pub(super) confirms: VecDeque<super::confirm::Queued>,
     /// The SSH hosts the wizard offers: recently used ones, then
@@ -466,6 +468,7 @@ impl App {
             local_claude: cfg.local_claude,
             claude_skipped: cfg.claude_skipped,
             claude_checked: HashSet::new(),
+            claude_installing: HashSet::new(),
             confirms: VecDeque::new(),
             ssh_hosts: cfg.ssh_hosts,
             modal_gen: 0,
diff --git a/src/tui/claude_update.rs b/src/tui/claude_update.rs
index e704425..9b5ba62 100644
--- a/src/tui/claude_update.rs
+++ b/src/tui/claude_update.rs
@@ -3,8 +3,10 @@
 //!
 //! Two checks feed it. The local one compares this machine's claude with the
 //! newest release (looked up at most once a day by the event loop); the remote
-//! one compares a host's claude, as reported in its `Welcome`, with ours. The
-//! update itself is one daemon request, `Msg::UpdateClaude`, for every host.
+//! one compares a host's claude, as reported in its `Welcome`, with ours. A
+//! host with no claude at all (this machine included) gets it installed as
+//! `install` says, whatever the other hosts have. The update or install is
+//! one daemon request, `Msg::UpdateClaude`, for every host.
 
 use std::io;
 
@@ -13,7 +15,8 @@ use crate::config::UpdatePolicy;
 use crate::proto::Msg;
 
 use super::app::{App, ReplyTo};
-use super::confirm::{Choice, ConfirmAction, ConfirmPrompt, Skip};
+use super::confirm::{Choice, ConfirmAction, ConfirmPrompt, Queued, Skip};
+use super::interaction::Modal;
 
 impl App {
     /// Record this machine's `claude --version` (from the local `Welcome`).
@@ -21,28 +24,55 @@ impl App {
         self.local_claude = version;
     }
 
+    /// The local `Welcome` is in: no claude here is installed as `install`
+    /// says.
+    pub fn check_local_install(&mut self) {
+        if self.local_claude.is_none() {
+            self.claude_missing("local");
+        }
+    }
+
     /// The newest release is `latest`: compare it with the local claude.
     pub fn check_local_claude(&mut self, latest: &str) {
-        // A missing local claude is not ours to install.
         if let Some(have) = self.local_claude.clone() {
             let policy = self.claude_policy.update_check;
-            self.claude_behind("local", Some(&have), latest, policy);
+            self.claude_behind("local", &have, latest, policy);
         }
     }
 
-    /// `host` just connected and reports `remote` (`None`: no claude).
+    /// `host` just connected and reports `remote` (`None`: no claude). A
+    /// missing claude is installed as `install` says; one older than ours
+    /// is updated as `remote_check` says.
     pub fn check_remote_claude(&mut self, host: &str, remote: Option<&str>) {
+        let Some(remote) = remote else {
+            self.claude_missing(host);
+            return;
+        };
         if let Some(local) = self.local_claude.clone() {
             let policy = self.claude_policy.remote_check;
             self.claude_behind(host, remote, &local, policy);
         }
     }
 
+    /// `host` has no claude: install it, ask first, or leave it, as
+    /// `install` says. Each host is looked at once per run, as below.
+    fn claude_missing(&mut self, host: &str) {
+        let policy = self.claude_policy.install;
+        if policy == UpdatePolicy::Off || !self.claude_checked.insert(host.to_owned()) {
+            return;
+        }
+        match decide(policy, false) {
+            Decision::Nothing => {}
+            Decision::Run => self.start_claude_update(host, true),
+            Decision::Ask => self.queue_confirm(install_prompt(host)),
+        }
+    }
+
     /// If `host`'s claude (`have`) is behind `want`, act on `policy`. Each
     /// host is looked at once per run, so reconnects do not nag.
-    fn claude_behind(&mut self, host: &str, have: Option<&str>, want: &str, policy: UpdatePolicy) {
+    fn claude_behind(&mut self, host: &str, have: &str, want: &str, policy: UpdatePolicy) {
         if policy == UpdatePolicy::Off
-            || !needs_update(have, want)
+            || !needs_update(Some(have), want)
             || !self.claude_checked.insert(host.to_owned())
         {
             return;
@@ -54,7 +84,7 @@ impl App {
             .claude_skipped
             .get(host)
             .is_some_and(|v| short(v) == short(want));
-        match decide(policy, have.is_none(), skipped) {
+        match decide(policy, skipped) {
             Decision::Nothing => {}
             Decision::Run => self.start_claude_update(host, false),
             Decision::Ask => self.queue_confirm(claude_prompt(host, have, want)),
@@ -65,6 +95,9 @@ impl App {
     pub(super) fn start_claude_update(&mut self, host: &str, install: bool) {
         let what = if install { "installing" } else { "updating" };
         self.notify(format!("{what} claude on {host}…"));
+        if install {
+            self.claude_installing.insert(host.to_owned());
+        }
         self.request(
             host,
             Msg::UpdateClaude { install },
@@ -72,21 +105,46 @@ impl App {
         );
     }
 
+    /// Whether claude is being installed on `host`, or a prompt open or
+    /// waiting asks to install it there.
+    pub(super) fn claude_install_pending(&self, host: &str) -> bool {
+        let install = ConfirmAction::UpdateClaude {
+            host: host.to_owned(),
+            install: true,
+        };
+        let asks = |p: &ConfirmPrompt| {
+            p.choices
+                .iter()
+                .any(|c| c.action.as_ref() == Some(&install))
+        };
+        self.claude_installing.contains(host)
+            || matches!(&self.modal, Some(Modal::Confirm(p)) if asks(p))
+            || self
+                .confirms
+                .iter()
+                .any(|q| matches!(q, Queued::Confirm(p) if asks(p)))
+    }
+
     /// The outcome of [`App::start_claude_update`].
     pub(super) fn on_claude_updated(&mut self, host: &str, reply: io::Result<Msg>) {
-        match reply {
+        let install = self.claude_installing.remove(host);
+        let what = if install { "install" } else { "update" };
+        let failure = match reply {
             Ok(Msg::ClaudeUpdated {
                 version: Some(version),
                 ok: true,
                 ..
             }) => {
-                self.notify(format!(
-                    "claude updated to {} on {host}; new sessions use it",
-                    short(&version)
-                ));
+                let done = if install {
+                    format!("claude {} installed", short(&version))
+                } else {
+                    format!("claude updated to {}", short(&version))
+                };
+                self.notify(format!("{done} on {host}; new sessions use it"));
                 if host == "local" {
                     self.local_claude = Some(version);
                 }
+                return;
             }
             Ok(Msg::ClaudeUpdated { tail, .. }) => {
                 let line = tail
@@ -94,47 +152,39 @@ impl App {
                     .map(str::trim)
                     .rfind(|l| !l.is_empty())
                     .unwrap_or("no output");
-                self.notify(format!("claude update failed on {host}: {line}"));
-            }
-            Ok(_) => {}
-            Err(e) => {
-                let notice = App::older_daemon_notice(host, &e)
-                    .unwrap_or_else(|| format!("claude update failed on {host}: {e}"));
-                self.notify(notice);
+                format!("claude {what} failed on {host}: {line}")
             }
+            Ok(_) => return,
+            Err(e) => App::older_daemon_notice(host, &e)
+                .unwrap_or_else(|| format!("claude {what} failed on {host}: {e}")),
+        };
+        // `--plain` has no session without claude: leave, saying why.
+        if install && host == "local" && self.plain_pending.is_some() {
+            self.plain_failed(failure);
+        } else {
+            self.notify(failure);
         }
     }
 }
 
-/// The question for a claude on `host` that is `have` (`None`: missing)
-/// while `want` is current.
-fn claude_prompt(host: &str, have: Option<&str>, want: &str) -> ConfirmPrompt {
+/// The question for a claude on `host` that is `have` while `want` is
+/// current.
+fn claude_prompt(host: &str, have: &str, want: &str) -> ConfirmPrompt {
     let want_short = short(want);
-    let (text, yes_label) = match have {
-        Some(have) if host == "local" => (
-            format!(
-                "claude {} is behind the latest release ({want_short}).\nRun `claude update`?",
-                short(have)
-            ),
-            "update",
-        ),
-        Some(have) => (
-            format!(
-                "claude on {host} is {}, older than the {want_short} on this machine.\nRun `claude update` there?",
-                short(have)
-            ),
-            "update",
-        ),
-        None => (
-            format!(
-                "claude is not installed on {host} (this machine has {want_short}).\nInstall it with the official installer\n(curl -fsSL https://claude.ai/install.sh | bash)?"
-            ),
-            "install",
-        ),
+    let text = if host == "local" {
+        format!(
+            "claude {} is behind the latest release ({want_short}).\nRun `claude update`?",
+            short(have)
+        )
+    } else {
+        format!(
+            "claude on {host} is {}, older than the {want_short} on this machine.\nRun `claude update` there?",
+            short(have)
+        )
     };
     let update = ConfirmAction::UpdateClaude {
         host: host.to_owned(),
-        install: have.is_none(),
+        install: false,
     };
     let skip = ConfirmAction::SkipClaude(Skip {
         host: host.to_owned(),
@@ -144,19 +194,42 @@ fn claude_prompt(host: &str, have: Option<&str>, want: &str) -> ConfirmPrompt {
         "Claude is out of date",
         text,
         vec![
-            Choice::new('y', yes_label, Some(update)),
+            Choice::new('y', "update", Some(update)),
             Choice::new('n', "not now", None),
             Choice::new('s', "skip this version", Some(skip)),
         ],
     )
 }
 
+/// The question for `host`, which has no claude.
+fn install_prompt(host: &str) -> ConfirmPrompt {
+    let place = if host == "local" {
+        "this machine"
+    } else {
+        host
+    };
+    let text = format!(
+        "claude is not installed on {place}.\nInstall it with the official installer\n(curl -fsSL https://claude.ai/install.sh | bash)?"
+    );
+    let install = ConfirmAction::UpdateClaude {
+        host: host.to_owned(),
+        install: true,
+    };
+    ConfirmPrompt::new(
+        "Claude is not installed",
+        text,
+        vec![
+            Choice::new('y', "install", Some(install)),
+            Choice::new('n', "not now", None),
+        ],
+    )
+}
+
 #[cfg(test)]
 mod tests {
     use super::*;
     use crate::tui::app::Effect;
     use crate::tui::app::AppConfig;
-    use crate::tui::interaction::Modal;
     use crate::tui::test_support;
     use crossterm::event::KeyCode;
 
diff --git a/src/tui/mod.rs b/src/tui/mod.rs
index 9d38fb8..1f37391 100644
--- a/src/tui/mod.rs
+++ b/src/tui/mod.rs
@@ -334,6 +334,7 @@ async fn main(launch: Launch) -> ExitCode {
     );
     app.set_host_features("local", &local_features);
     app.on_plugin_host("local");
+    app.check_local_install();
     let start = match plain {
         Some(start) => Start::Plain(start),
         None => Start::Manager { saved, live },
@@ -1164,6 +1165,7 @@ async fn event_loop(
                     app.set_host_features("local", &client.welcome().features);
                     conns.connected("local", client);
                     app.on_plugin_host("local");
+                    app.check_local_install();
                     app.on_reconnected_local(&sessions, home);
                 }
                 Some(HostEvent::LocalReconnectFailed { generation }) => {
diff --git a/src/tui/plugin_setup.rs b/src/tui/plugin_setup.rs
index 8bc3eb0..8e88fe0 100644
--- a/src/tui/plugin_setup.rs
+++ b/src/tui/plugin_setup.rs
@@ -373,6 +373,10 @@ impl App {
         if self.mode != Mode::Plain {
             return;
         }
+        // No claude here: its install (or the question) comes first.
+        if self.claude_install_pending("local") {
+            return;
+        }
         if let Some(profile) = profile {
             if self.plain_waits(&profile, deadline) {
                 return;
`````
Run: `umask 022; cargo test --locked --bin claudio`
Expected: `test result: ok. 1110 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` (+3 in `claude_update.rs`, +1 in `plain.rs`)

Step 3: commit
`/usr/bin/git commit -m "feat(tui): install claude where a host has none, as [claude] install says" -- src/claude/update.rs src/config.rs src/tui/app.rs src/tui/claude_update.rs src/tui/mod.rs src/tui/plain.rs src/tui/plugin_setup.rs tests/common/mod.rs`

Interfaces this task provides to later tasks: `ClaudeSection::install: UpdatePolicy`;
`App::check_local_install()`, `App::claude_install_pending(host) -> bool`,
`App::claude_installing: HashSet<String>`; in `on_claude_updated`, the local `install` flag of
the reply's request.

### Task 4: client: re-ask after an install and when a retry is due
Model: standard   Repo: /volumes/repos/claudio   Files: src/proto.rs, src/tui/claude_update.rs, src/tui/plugins.rs
Goal: a successful install asks that host again for every enabled manifest; a host is asked again while a plugin installs or once a retry is due (Goals 2 and 5; AC2).
Patch: .delivery/2026-11-07-claude-auto-install/patches/task-4.patch

Step 1: write the failing test
`````diff
diff --git a/src/proto.rs b/src/proto.rs
index 0b25c09..d3549a1 100644
--- a/src/proto.rs
+++ b/src/proto.rs
@@ -1225,6 +1231,21 @@ mod tests {
         Frame::decode(&bytes[4..]).unwrap()
     }
 
+    #[test]
+    fn a_retrying_reason_carries_its_seconds() {
+        let why = retrying("marketplace m: down (503)", 15);
+        assert_eq!(why, "marketplace m: down (503) (retrying in 15 s)");
+        assert_eq!(retry_secs(&why), Some(15));
+        for other in [
+            "installing",
+            "network down",
+            "x (retrying in s)",
+            "(retrying in 5 s",
+        ] {
+            assert_eq!(retry_secs(other), None, "{other}");
+        }
+    }
+
     #[test]
     fn welcome_from_an_old_daemon_has_no_build() {
         let old = r#"{"claudio_version":"0.2.0","proto":1,
diff --git a/src/tui/plugins.rs b/src/tui/plugins.rs
index 2b03481..46091e5 100644
--- a/src/tui/plugins.rs
+++ b/src/tui/plugins.rs
@@ -927,9 +954,13 @@ KB_REGION = "eu"
     }
 
     #[test]
-    fn a_host_still_installing_is_asked_again_every_30_s_for_10_minutes() {
-        let mut installing = applied("kb");
-        installing.skipped = vec![("plugin:p@m".into(), "installing".into())];
+    fn a_host_is_asked_again_while_it_installs_or_until_a_retry_is_due() {
+        let answer = |why: &str| {
+            let mut a = applied("kb");
+            a.skipped = vec![("plugin:p@m".into(), why.into())];
+            a
+        };
+        let installing = answer("installing");
         let mut app = app_with(&[]);
         app.now = 1000;
         app.plugins
@@ -954,15 +985,83 @@ KB_REGION = "eu"
             app.apply_state("local", "kb"),
             Some(&ApplyState::Ready(Box::new(installing.clone())))
         );
+        // As long as it installs.
         app.on_applied("local", "kb", &sha(), Ok(Msg::Applied(installing)));
-        app.now = 1000 + 601;
+        app.now = 1030 + 30 * 60;
         app.reask_installing();
-        assert!(asked(&mut app).is_empty(), "10 minutes at most");
-        // Done installing: no more asking.
-        app.on_applied("local", "kb", &sha(), Ok(Msg::Applied(applied("kb"))));
-        app.now = 1030;
+        assert_eq!(asked(&mut app), [pair("local", "kb")]);
+
+        // A failed install: a second after its retry is due...
+        let asked_at = app.now;
+        let later = answer("network down (retrying in 120 s)");
+        app.on_applied("local", "kb", &sha(), Ok(Msg::Applied(later)));
+        app.now = asked_at + 120;
+        app.reask_installing();
+        assert!(asked(&mut app).is_empty());
+        app.now = asked_at + 121;
+        app.reask_installing();
+        assert_eq!(asked(&mut app), [pair("local", "kb")]);
+        // ...and no sooner than 30 s.
+        let asked_at = app.now;
+        let soon = answer("network down (retrying in 5 s)");
+        app.on_applied("local", "kb", &sha(), Ok(Msg::Applied(soon)));
+        app.now = asked_at + 29;
         app.reask_installing();
         assert!(asked(&mut app).is_empty());
+        app.now = asked_at + 30;
+        app.reask_installing();
+        assert_eq!(asked(&mut app), [pair("local", "kb")]);
+
+        // Installed, or waiting for a person: no more asking.
+        for settled in [
+            applied("kb"),
+            answer("needs manual install: claude plugin install p@m"),
+        ] {
+            app.on_applied("local", "kb", &sha(), Ok(Msg::Applied(settled)));
+            app.now += 3600;
+            app.reask_installing();
+            assert!(asked(&mut app).is_empty());
+        }
+    }
+
+    #[test]
+    fn claude_installed_on_a_host_asks_it_again_for_every_manifest() {
+        let mut app = app_with(&[]);
+        app.plugins
+            .choices
+            .insert("kb".into(), PluginChoices::default());
+        app.on_plugin_host("local");
+        app.on_plugin_host("devbox");
+        app.on_plugins(
+            "work",
+            PluginsFetch::Listed(list("work", vec![sample("kb")])),
+        );
+        app.on_manifest("kb", &sha(), Ok(manifest_of("kb")));
+        for host in ["local", "devbox"] {
+            app.on_applied(host, "kb", &sha(), Ok(Msg::Applied(applied("kb"))));
+        }
+        app.take_effects();
+        let reply = |app: &mut App, install: bool, ok: bool| {
+            app.start_claude_update("devbox", install);
+            let msg = Msg::ClaudeUpdated {
+                version: ok.then(|| "2.1.296 (Claude Code)".to_owned()),
+                ok,
+                tail: "claude installer needs curl".into(),
+            };
+            app.on_reply(ReplyTo::ClaudeUpdate("devbox".into()), Ok(msg));
+        };
+
+        reply(&mut app, true, false);
+        assert!(asked(&mut app).is_empty(), "the install failed");
+        reply(&mut app, true, true);
+        assert_eq!(asked(&mut app), [pair("devbox", "kb")]);
+        assert_eq!(
+            app.apply_state("devbox", "kb"),
+            Some(&ApplyState::Ready(Box::new(applied("kb")))),
+            "the answer stays until the new one comes"
+        );
+        reply(&mut app, false, true);
+        assert!(asked(&mut app).is_empty(), "an update is not an install");
     }
 
     fn part(name: &str, ready: bool, secrets: &[(&str, &str)]) -> PluginPart {
`````
Run: `umask 022; cargo test --locked --bin claudio`
Expected: the build fails: `` error[E0425]: cannot find function `retry_secs` in this scope `` (`due to 2 previous errors`)

Step 2: implement
`````diff
diff --git a/src/proto.rs b/src/proto.rs
index a7f3028..d3549a1 100644
--- a/src/proto.rs
+++ b/src/proto.rs
@@ -466,6 +466,12 @@ pub fn retrying(why: &str, secs: u64) -> String {
     format!("{why} (retrying in {secs} s)")
 }
 
+/// The seconds of a [`retrying`] reason.
+pub fn retry_secs(reason: &str) -> Option<u64> {
+    let (_, secs) = reason.strip_suffix(" s)")?.rsplit_once(" (retrying in ")?;
+    secs.parse().ok()
+}
+
 /// The lenses of a claude session, in the client's registry order, and
 /// the proxy plugins' instructions. The daemon checks the caps
 /// ([`SessionExtensions::validate`]), writes the instructions to the
diff --git a/src/tui/claude_update.rs b/src/tui/claude_update.rs
index 9b5ba62..6f0a658 100644
--- a/src/tui/claude_update.rs
+++ b/src/tui/claude_update.rs
@@ -144,6 +144,9 @@ impl App {
                 if host == "local" {
                     self.local_claude = Some(version);
                 }
+                if install {
+                    self.reapply_host(host);
+                }
                 return;
             }
             Ok(Msg::ClaudeUpdated { tail, .. }) => {
diff --git a/src/tui/plugins.rs b/src/tui/plugins.rs
index b314667..46091e5 100644
--- a/src/tui/plugins.rs
+++ b/src/tui/plugins.rs
@@ -8,14 +8,15 @@
 //! runs. The registry lists the plugins (`Source::Plugin`) so the
 //! extensions screen shows and toggles them; turning one off is saved as
 //! `[extensions] <name> = false` and applies to new sessions only. A host
-//! is asked on connect, when the list's hash changes, on `r`, and every
-//! 30 s while it still installs.
+//! is asked on connect, when the list's hash changes, on `r`, once claude
+//! is installed there, every 30 s while it still installs, and when a
+//! failed install's retry is due.
 
 use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
 
 use crate::config::PluginChoices;
 use crate::plugins::manifest::Manifest;
-use crate::proto::{Applied, Msg, MAX_PLUGIN_INSTRUCTIONS};
+use crate::proto::{retry_secs, Applied, Msg, MAX_PLUGIN_INSTRUCTIONS};
 use crate::proxy::env::merge;
 use crate::proxy::plugins::{Plugin, PluginList};
 
@@ -82,9 +83,8 @@ pub struct Apply {
     /// The manifest hash asked for.
     pub sha: String,
     pub state: ApplyState,
-    /// When it was last asked, and first asked at this hash (Unix seconds).
+    /// When it was last asked (Unix seconds).
     pub asked: u64,
-    pub since: u64,
 }
 
 /// The client's plugin state.
@@ -161,9 +161,9 @@ impl std::fmt::Debug for Composed {
 pub const MAX_SPAWN_PLUGINS: usize = 8;
 
 /// While a host still installs a manifest's plugins it is asked again this
-/// often (seconds), for at most [`REASK_FOR`] after the first ask.
+/// often (seconds); while it waits to retry one, when that is due (and no
+/// sooner than this).
 pub const REASK_EVERY: u64 = 30;
-pub const REASK_FOR: u64 = 10 * 60;
 
 /// The plugins' share of a spawn, from `parts` in registry order: for each
 /// plugin the host answered for (at most [`MAX_SPAWN_PLUGINS`]), its
@@ -424,7 +424,6 @@ impl App {
                 sha: p.bundle_sha256.clone(),
                 state: ApplyState::Pending,
                 asked: now,
-                since: now,
             },
         };
         self.plugins.applies.insert(key, apply);
@@ -455,8 +454,9 @@ impl App {
         }
     }
 
-    /// Ask again, every [`REASK_EVERY`] s, a host whose answer still lists
-    /// an install under way, for [`REASK_FOR`] after the first ask.
+    /// Ask again a host whose answer still lists an install under way
+    /// (every [`REASK_EVERY`] s) or a failed one to retry (once it is due).
+    /// The daemon's backoff, up to 5 min, paces a lasting failure.
     pub(super) fn reask_installing(&mut self) {
         let now = self.now;
         let due: Vec<(String, String)> = self
@@ -464,15 +464,11 @@ impl App {
             .applies
             .iter()
             .filter(|(_, a)| {
-                let installing = match &a.state {
-                    ApplyState::Ready(applied) => {
-                        applied.skipped.iter().any(|(_, why)| why == INSTALLING)
-                    }
-                    _ => false,
+                let after = match &a.state {
+                    ApplyState::Ready(applied) => reask_after(applied),
+                    _ => None,
                 };
-                installing
-                    && now.saturating_sub(a.asked) >= REASK_EVERY
-                    && now.saturating_sub(a.since) <= REASK_FOR
+                after.is_some_and(|after| now.saturating_sub(a.asked) >= after)
             })
             .map(|(key, _)| key.clone())
             .collect();
@@ -488,6 +484,23 @@ impl App {
         }
     }
 
+    /// Claude was just installed on `host`: ask it again for every enabled
+    /// manifest, answered or not, so the installs that failed without it
+    /// run now. An answer stays until the new one comes.
+    pub(super) fn reapply_host(&mut self, host: &str) {
+        if !self.plugins.hosts.contains(host) {
+            return;
+        }
+        let names: Vec<String> = self
+            .all_enabled_plugins()
+            .iter()
+            .map(|p| p.name.clone())
+            .collect();
+        for name in names {
+            self.request_manifests(host, Some(&name));
+        }
+    }
+
     /// `host`'s answer for plugin `name`'s manifest at hash `sha256`.
     pub(super) fn on_applied(
         &mut self,
@@ -675,6 +688,20 @@ impl App {
     }
 }
 
+/// How long after its last ask a host that answered `a` is asked again, if
+/// it is: [`REASK_EVERY`] while a plugin installs, else a second after the
+/// soonest retry (no sooner than [`REASK_EVERY`]).
+fn reask_after(a: &Applied) -> Option<u64> {
+    a.skipped
+        .iter()
+        .filter_map(|(_, why)| match why.as_str() {
+            INSTALLING => Some(0),
+            why => retry_secs(why).map(|secs| secs + 1),
+        })
+        .min()
+        .map(|secs| secs.max(REASK_EVERY))
+}
+
 /// The extensions screen's lines for `host`'s answer `a`: the counts, one
 /// line per skipped id that is not installing, and the flags its claude
 /// lacks.
`````
Run: `umask 022; cargo test --locked --bin claudio`
Expected: `test result: ok. 1112 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` (+1 in `proto.rs`, +1 in `plugins.rs`; the re-ask test is rewritten)

Step 3: commit
`/usr/bin/git commit -m "feat(tui): ask a host again once claude is installed or a plugin retry is due" -- src/proto.rs src/tui/claude_update.rs src/tui/plugins.rs`

Interfaces this task provides to later tasks: `proto::retry_secs(&str) -> Option<u64>`;
`App::reapply_host(host)`.

### Task 5: manager test (AC4) and docs
Model: standard   Repo: /volumes/repos/claudio   Files: tests/manager.rs, README.md, CLAUDE.md, docs/extensions.md
Goal: the local install end to end in the real binary with a fake `curl`; `[claude] install` and the backoff documented, the "10 min" wording gone (AC4, AC5).
Patch: .delivery/2026-11-07-claude-auto-install/patches/task-5.patch

Step 1: write the test (it passes at once: Tasks 2–4 implement it; on 92c62b9 it fails, D15)
`````diff
diff --git a/tests/manager.rs b/tests/manager.rs
index 82f14a6..3e95c21 100644
--- a/tests/manager.rs
+++ b/tests/manager.rs
@@ -3075,3 +3075,92 @@ fn test_plugin_manifest_setup_then_sessions_get_what_was_kept() {
     assert!(!env.contains(PLUGIN_KEY), "{env}");
     tui.quit(WAIT);
 }
+
+// ── Installing a missing claude ──────────────────────────────────────────────
+
+/// A machine without claude gets it from the official installer at start
+/// (`[claude] install = "auto"`), and its sessions use it. The installer is
+/// faked and nothing real is reachable: `PATH` is one directory of the
+/// test's, with a fake `curl` (its script puts a fake claude in
+/// `~/.local/bin`, where the real installer puts claude) and links to the
+/// few system tools the run needs, and the login shell is a stand-in that
+/// only prints that `PATH`.
+#[test]
+fn test_missing_claude_is_installed_at_start() {
+    use std::os::unix::fs::{symlink, PermissionsExt};
+    use std::path::Path;
+    // The daemon also looks for claude in these: one there means this
+    // machine cannot play one without claude.
+    for dir in ["/usr/bin", "/usr/local/bin", "/opt/homebrew/bin"] {
+        if Path::new(dir).join("claude").exists() {
+            eprintln!("skipped: {dir}/claude exists");
+            return;
+        }
+    }
+    let h = ManagerHarness::new();
+    fs::write(
+        h.config_home.join("claudio/config.toml"),
+        "[claude]\nupdate_check = \"off\"\nremote_check = \"off\"\ninstall = \"auto\"\n",
+    )
+    .unwrap();
+    let script = |path: &Path, text: &str| {
+        fs::write(path, text).unwrap();
+        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
+    };
+    let bin = h.root.join("bin");
+    fs::create_dir_all(&bin).unwrap();
+    for tool in ["sh", "bash", "cat", "cp", "mkdir"] {
+        let real = ["/usr/bin", "/bin"]
+            .iter()
+            .map(|d| Path::new(d).join(tool))
+            .find(|p| p.is_file())
+            .unwrap_or_else(|| panic!("no {tool} in /usr/bin or /bin"));
+        symlink(real, bin.join(tool)).unwrap();
+    }
+    let fake = h.root.join("installed-claude");
+    script(
+        &fake,
+        "#!/bin/sh\n\
+         if [ \"$1\" = \"--version\" ]; then echo '2.1.296 (Claude Code)'; exit 0; fi\n\
+         if [ \"$1\" = \"--help\" ]; then exit 0; fi\n\
+         echo \"FAKE_CLAUDE_BANNER args=$*\"\n\
+         exec cat\n",
+    );
+    let log = h.root.join("curl.log");
+    script(
+        &bin.join("curl"),
+        &format!(
+            "#!/bin/sh\n\
+             echo \"$*\" >> '{}'\n\
+             echo 'mkdir -p \"$HOME/.local/bin\" && cp \"{}\" \"$HOME/.local/bin/claude\"'\n\
+             echo 'echo claude installed'\n",
+            log.display(),
+            fake.display()
+        ),
+    );
+    // Run as `$SHELL -l -i -c <script>`: the script, without rc files.
+    let shell = h.root.join("login-shell");
+    script(
+        &shell,
+        "#!/bin/sh\nfor arg; do script=$arg; done\nexec /bin/sh -c \"$script\"\n",
+    );
+
+    let mut tui = h.start_tui_with(|cmd| {
+        cmd.env_remove("CLAUDIO_CLAUDE_PATH");
+        cmd.env("PATH", &bin);
+        cmd.env("SHELL", &shell);
+    });
+    tui.wait_for(
+        "claude 2.1.296 installed on local; new sessions use it",
+        Region::Screen,
+        WAIT,
+    );
+    assert_eq!(
+        fs::read_to_string(&log).unwrap(),
+        "-fsSL https://claude.ai/install.sh\n"
+    );
+    assert!(h.home.join(".local/bin/claude").is_file());
+    // The wizard is open (no sessions yet); the session runs the new claude.
+    wizard_pick_dir(&mut tui, &h.dirs[0]);
+    tui.quit(WAIT);
+}
`````
Run: `umask 022; cargo test --locked --test manager test_missing_claude_is_installed_at_start -- --test-threads=1`
Expected: `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 51 filtered out` (about 3 s)

Step 2: docs
`````diff
diff --git a/CLAUDE.md b/CLAUDE.md
index a88e2f6..8427d0e 100644
--- a/CLAUDE.md
+++ b/CLAUDE.md
@@ -141,7 +141,10 @@ daemons coexist.
 - `git.rs` — read-only git queries for the git viewer (log, commit, diff):
   bounded output, sanitized text, no textconv/ext-diff, repo env scrubbed.
 - `update.rs` — `claude update` (or the official installer) on request, in its
-  own process group, killed as a group on timeout.
+  own process group, killed as a group on timeout. The installer needs
+  `curl` and `bash` on `Config::path` and is refused when
+  `CLAUDIO_CLAUDE_PATH` names a path (the reply's tail says why, nothing
+  runs); a successful install calls `Manifests::forget_failures`.
 - `manifest.rs` — `Manifest` (a proxy plugin's manifest, parsed by
   `crate::plugins::manifest`): checks the op itself (name `[a-z0-9-]{1,40}`,
   64-hex hash, https, ≤ 64 secret names by `key_ok`, ≤ 256 disabled item
@@ -160,7 +163,11 @@ daemons coexist.
   each holding `Daemon::updating` (shared with `claude update`), stdin null,
   output capped, markers scrubbed, `GIT_TERMINAL_PROMPT=0`, own process
   group, 120 s. Outcomes are remembered per (hash, id): installed (daemon's
-  life), `needs manual install` (daemon's life), a failure (10 min).
+  life), `needs manual install` (daemon's life), a failure (retried after
+  15 s, doubling per failure in a row up to 300 s, reset by a success;
+  `forget_failures` drops them all; meanwhile the reason ends `(retrying
+  in N s)`, `proto::retrying`). Without a claude, `run_claude` fails at
+  once (`claude is not installed`), never taking the lock.
   `Daemon::claude_flags` (one `claude --help`) says whether claude knows
   `--plugin-dir`, `--agents`, `--mcp-config`; `command()` drops such a pair
   when it does not, and `Applied.dropped` names them. Stale zips of a
@@ -258,7 +265,12 @@ is idle (checked every 2 min).
   actions: search (`Msg::Search`, resume or switch) and snippets
   (`Effect::LoadSnippets`/`SaveSnippet`, `Msg::LastPrompt`).
 - `confirm.rs` — the generic confirm prompt (kill, reset, claude update).
-- `claude_update.rs` — local/remote claude version checks and updates.
+- `claude_update.rs` — local/remote claude version checks and updates, and
+  `[claude] install` (`auto` by default, `ask`, `off`): a host whose
+  `Welcome` has no claude, `local` included (`check_local_install`, also in
+  `--plain`, whose session waits for it), gets the installer once per run
+  (`claude_checked`). A successful install asks that host again for every
+  manifest (`reapply_host`).
 - `plain.rs` — `claudio --plain`: `Mode::Plain` is one local, ephemeral
   session full screen (no tab/status bar, wizard or state.json).
   `Keymap::build_plain` keeps only Alt+h/s/l/e/u/z/i; the session is killed when
@@ -281,7 +293,8 @@ is idle (checked every 2 min).
   plugin's manifest (`Effect::FetchManifest`: fetched by the client, 256
   KiB, hash-checked, memory only), the registry's `Source::Plugin` rows,
   each `(host, plugin)` `Applied` answer (`ApplyState`; asked again every
-  30 s while it lists `installing`, ≤ 10 min), and what a spawn gets
+  30 s while it lists `installing`, and a second after a `(retrying in N
+  s)` retry is due, no sooner than 30 s), and what a spawn gets
   (`compose`: per answered plugin, ≤ 8, `--agents` then `--mcp-config`
   after every other arg; env layers `[env]`, then the secrets, then the
   profile env; the instructions in `SessionExtensions.instructions`, cut at
@@ -415,8 +428,10 @@ Reasons name ids and fields, never values.
 - **Test fakes and the test config.** The daemon runs `claude --help` once to
   probe for flags, so fake claudes must ignore `--help` (and `--version`)
   without side effects. Every test claudio starts with `[claude]
-  update_check/remote_check = "off"`: the update prompts are modal and would
-  swallow the keys a test types.
+  update_check/remote_check/install = "off"`: the update prompts are modal
+  and would swallow the keys a test types, and an install would run the
+  installer (`test_missing_claude_is_installed_at_start` turns it on with a
+  fake `curl` and a `PATH` that reaches no real one).
 - **`app.rs` is pure; I/O is only in `tui/mod.rs`.** `App::update` takes
   events and returns `Effect` values. `mod.rs` drives the effects. Don't add
   I/O to `app.rs`.
diff --git a/README.md b/README.md
index cfd5c1c..aea44bc 100644
--- a/README.md
+++ b/README.md
@@ -65,7 +65,7 @@ curl -fsSL https://github.com/p4u/claudio/releases/latest/download/install.sh |
 
 ### Without a proxy
 
-**Requirements:** [Claude Code](https://claude.ai/claude-code) installed and logged in (`claude` works in your shell), and `~/.local/bin` on your `PATH`.
+**Requirements:** a [Claude Code](https://claude.ai/claude-code) account, and `~/.local/bin` on your `PATH`. Where claude is missing, claudio installs it (see [Keeping claude up to date](#keeping-claude-up-to-date)); you log in the first time it starts.
 
 1. [Install](#install) claudio and run `claudio`.
 2. The new-session wizard opens. Pick **Explore local dirs…** to browse from your home directory, one of your recent directories, or an SSH host under *Remote*.
@@ -86,7 +86,7 @@ claudio                                         # new sessions now use the proxy
 
 `claudio proxy login` prints a page and a one-time code, opens the page in a browser when there is a display (`CLAUDIO_NO_BROWSER=1` only prints it), and waits while you sign in and type the code there. The proxy then hands claudio your own token, saved with your e-mail in the profile (`claudio proxy status` shows it). Without a URL it signs in again to the default profile's proxy. `--token`, or a proxy without browser sign-in, asks for a pasted token instead; a token is never an argument.
 
-The proxy can also serve **plugins**: a pinned manifest (a TOML file, hash-checked) that says what a member needs: marketplaces and the Claude Code plugins to install from them, MCP servers (with the secrets they need, which the proxy serves as env), subagents, instructions for every session and env. The first time claudio sees a manifest it shows every item in a setup screen, all on; you turn off what you don't want and press Enter (the choice is saved under `[plugins.<name>]` in `config.toml`; Esc asks again at the next start). Then each host's daemon installs the plugins in your user scope with `claude plugin install` (never with `-y`: a plugin whose marketplace runs a command is left for you to install by hand), and every session claudio opens with that proxy profile (manager, `--plain`, SSH hosts) gets the servers (`--mcp-config`, secrets only in its env), the agents (`--agents`) and the instructions. What you already have stays yours: an installed plugin, a marketplace, an MCP server or an agent of the same name is left alone. The list and its env are cached in `~/.config/claudio/plugins.json` (mode 0600), so a session still gets them when the proxy is unreachable, and refreshed every 15 minutes. `claudio proxy logout` deletes them. Plugins are listed in the extensions screen (`Alt+i`): `Space` turns one off for new sessions, `Enter` opens its setup screen again, and its details say, per host, what was installed, what was already there, what is still installing and what was skipped and why. Installed plugins stay until you remove them (`claude plugin uninstall <id>`).
+The proxy can also serve **plugins**: a pinned manifest (a TOML file, hash-checked) that says what a member needs: marketplaces and the Claude Code plugins to install from them, MCP servers (with the secrets they need, which the proxy serves as env), subagents, instructions for every session and env. The first time claudio sees a manifest it shows every item in a setup screen, all on; you turn off what you don't want and press Enter (the choice is saved under `[plugins.<name>]` in `config.toml`; Esc asks again at the next start). Then each host's daemon installs the plugins in your user scope with `claude plugin install` (never with `-y`: a plugin whose marketplace runs a command is left for you to install by hand), and every session claudio opens with that proxy profile (manager, `--plain`, SSH hosts) gets the servers (`--mcp-config`, secrets only in its env), the agents (`--agents`) and the instructions. What you already have stays yours: an installed plugin, a marketplace, an MCP server or an agent of the same name is left alone. The list and its env are cached in `~/.config/claudio/plugins.json` (mode 0600), so a session still gets them when the proxy is unreachable, and refreshed every 15 minutes. `claudio proxy logout` deletes them. Plugins are listed in the extensions screen (`Alt+i`): `Space` turns one off for new sessions, `Enter` opens its setup screen again, and its details say, per host, what was installed, what was already there, what is still installing and what was skipped and why. A failed install is tried again after 15 seconds, then twice as long each time it fails again, up to every 5 minutes (the details say `retrying in N s`). Installed plugins stay until you remove them (`claude plugin uninstall <id>`).
 
 - `claudio --no-proxy` or `claudio --proxy <profile>` overrides the default for one run.
 - `CLAUDIO_PROXY_URL=<token>@proxy.example.com claudio` uses a proxy for one run without saving a profile.
@@ -223,7 +223,9 @@ In the pane, each tag is replaced by a one-line Unicode rendering in cyan, such
 
 ### Keeping claude up to date
 
-Once a day claudio compares your `claude --version` with the latest release on your update channel, and asks before running `claude update`. When an SSH host connects, its claude is compared with yours. If it is older, claudio offers to update it; if it is missing, claudio offers to install it with the official installer. Either way it asks first. Prompts offer `[y] update`, `[n] not now` and `[s] skip this version`. Running sessions keep their binary; new sessions use the updated one.
+Once a day claudio compares your `claude --version` with the latest release on your update channel, and asks before running `claude update`. When an SSH host connects, its claude is compared with yours, and if it is older, claudio offers to update it. Prompts offer `[y] update`, `[n] not now` and `[s] skip this version`. Running sessions keep their binary; new sessions use the updated one.
+
+Where claude is missing, on this machine or on an SSH host, claudio installs it with the official installer (`curl -fsSL https://claude.ai/install.sh | bash`), without asking, once per host each time claudio starts. A notice says `installing claude on <host>…`, then the version installed or why it failed. The installer runs with your login shell's `PATH` and needs `curl` and `bash` there; a host without them gets `claude installer needs curl` (or `bash`) and nothing runs. If `CLAUDIO_CLAUDE_PATH` names a file that does not exist, claudio says so instead of installing. Once claude is installed on a host, the proxy plugins it could not install there are installed right away. `install = "ask"` asks first and `install = "off"` never installs (see [Configuration](#configuration)). The first session still asks you to log in.
 
 claudio also starts claude with `--allow-dangerously-skip-permissions`, so bypass-permissions mode is available with `Shift+Tab` (claude does not start in it). Turn this off with `allow_skip_permissions = false` (see [Configuration](#configuration)).
 
@@ -295,6 +297,7 @@ check = true                    # check for new claudio releases
 allow_skip_permissions = true   # start claude with --allow-dangerously-skip-permissions
 update_check = "ask"            # this machine's claude vs its release channel: "ask" | "auto" | "off"
 remote_check = "ask"            # SSH hosts' claude vs this machine's: "ask" | "auto" | "off"
+install = "auto"                # install claude where a host has none: "auto" | "ask" | "off"
 latex = true                    # ask claude to write math in <latex> tags, rendered in the pane
                                 # (this host's sessions; `claudio daemon restart` after a change)
 
diff --git a/docs/extensions.md b/docs/extensions.md
index e99abeb..c5c0b18 100644
--- a/docs/extensions.md
+++ b/docs/extensions.md
@@ -143,11 +143,13 @@ Code does not know (`claude plugin marketplace add --scope user`) and
 installs the plugins in your user scope (`claude plugin install --scope
 user`), in the background, one at a time, never with `-y`: a plugin whose
 marketplace declares a command to run is `needs manual install` with the
-command to type. A failed install is tried again after 10 minutes. What you
-already have is left alone: an installed plugin is `present`, an MCP server
-in your `~/.claude.json` or an agent in `~/.claude/agents/` of the same name
-is skipped as `yours`. A server whose secret the proxy does not serve is
-skipped too.
+command to type. A failed install is tried again after 15 seconds, then
+after twice as long each time it fails again, up to every 5 minutes (its
+reason says `retrying in N s`), and at once when claude has just been
+installed on that host. What you already have is left alone: an installed
+plugin is `present`, an MCP server in your `~/.claude.json` or an agent in
+`~/.claude/agents/` of the same name is skipped as `yours`. A server whose
+secret the proxy does not serve is skipped too.
 
 Every session started with that profile then gets, per plugin that is on,
 `--agents` and `--mcp-config` (the server definitions with `${NAME}`
`````
Run: `grep -rn '10 min' README.md CLAUDE.md docs/extensions.md src` → nothing; then the full
suites of Merge and release point 1.

Step 3: commit
`/usr/bin/git commit -m "test(manager): a machine without claude gets it installed at start; docs" -- tests/manager.rs README.md CLAUDE.md docs/extensions.md`

## Merge and release
<!-- Not copied into briefs; the controller executes this after the gate audit. -->
1. Full suites on the tip of `feat/claude-auto-install`, each alone on the machine (measured in
   the scratch on 69ed1ab):
   - `umask 022; make test-unit` → `test result: ok. 1112 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`,
     and the dev build has 0 warnings (on a base flake named in the header, rerun once).
   - `umask 022; make test-manager` → `test result: ok. 52 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` (about 175 s).
   - `umask 022; env -u CLAUDIO_SSH_TEST_HOST cargo test --features diag --test remote -- --test-threads=1` → `test result: ok. 9 passed`.
   - `make` → release build OK.
   - `rustfmt --edition 2021 --check` on each clean file of Global Constraints → nothing.
   - Clippy as in Global Constraints: 74 findings, the same as the base (compared by file and message), none on a line this branch adds; the deny-level `src/tui/wizard.rs:193` still stops the bin-test build, as at the base.
2. Manual check (owner or controller, optional, never on a machine whose claude matters): on an
   SSH host without claude (or a fresh container with `curl` and `bash`), connect from the
   branch's release build with default config: the notice `installing claude on <host>…`, then
   `claude <v> installed on <host>; new sessions use it`; with a proxy profile serving
   `vocdoni-internal`, `Alt+i` shows that host's plugins `installing`, then installed, with no
   `retrying` line left. A host without `curl` says `claude install failed on <host>: claude
   installer needs curl`.
3. Gate audit of the branch; fixes re-reviewed.
4. Merge: `/usr/bin/git checkout main && /usr/bin/git merge --ff-only feat/claude-auto-install`
   (`main` is at 92c62b9; the branch is linear on top of it: the five task commits).
5. Release v0.7.1 (the convention of `git show --stat 92c62b9`):
   - Set `version = "0.7.1"` in Cargo.toml and run `cargo update --workspace --offline`, which
     changes only claudio's own entry; `git diff --stat` shows `Cargo.lock | 2 +-` and
     `Cargo.toml | 2 +-`, and `cargo build --locked` succeeds.
   - `/usr/bin/git commit -m "release v0.7.1" -- Cargo.toml Cargo.lock` (no trailer).
   - `/usr/bin/git tag v0.7.1` (check `/usr/bin/git tag -l v0.7.1` is empty first). The release
     workflow checks that the tag matches Cargo.toml.
6. **Owner OK required before**, each on its own:
   - `/usr/bin/git push origin main`;
   - `/usr/bin/git push origin v0.7.1` (starts the release workflow: builds and publishes the
     binaries);
   - announcing the release (members' claudio installs claude on their hosts without asking from
     then on; `install = "ask"` or `"off"` under `[claude]` opts out).
7. Post-release checks:
   - `gh run list --limit 5` → the `ci` run on `main` (Linux and macOS) and the `release` run on
     `v0.7.1` succeed (`gh run watch <id>`).
   - `gh release view v0.7.1` lists every target of `release.yml`.
   - `claudio upgrade` from v0.7.0 reaches 0.7.1, and `claudio daemon status` shows the new
     daemon (a v0.7.0 daemon is replaced when idle, `freshness.rs`).

## Scratch verification record
- Where: `~/.cache/claudio/scratch/claude-auto-install`, a detached worktree of /volumes/repos/claudio at 92c62b9, one commit per task: a8d9881, 1c7cb03, e87d5a9, e63502b, 69ed1ab. The patches are `git format-patch -1` of those commits; Steps 1 and 2 of a task together are its patch.
- Split check: each task was rebuilt on a second worktree (`~/.cache/claudio/scratch/cai/verify`) as its Step 1 (test part) and then its Step 2, running the commands above. Steps 1 of Tasks 1–4 failed as quoted; Task 5's Step 1 passes by design and, applied alone on 92c62b9, fails with `wait_for timeout: "claude 2.1.296 installed on local; new sessions use it" not found in Screen` (D15). Every Step 2 tree equals the task's commit.
- Replay: the assembled plan's blocks were applied with `git apply`, in order, on a fresh worktree at 92c62b9 (`~/.cache/claudio/scratch/cai/replay`), each task committed with its Step 3 command: every block applied, each commit's tree equals the scratch commit, no trailer was added, nothing was left uncommitted. The five patch files, applied in order with `git apply` on another fresh worktree (`cai/patchcheck`), give 69ed1ab's tree. On the replayed tip: `cargo build` 0 warnings, unit 1112 passed, remote (no host) 9 passed, manager 52 passed.
- Unit totals: 1100 (base) → 1102 (Task 1) → 1106 (2) → 1110 (3) → 1112 (4) → 1112 (5). Manager: 51 (base) → 52 (Task 5; the new test 5/5 stable alone, about 3 s). Remote (no host): 9 at the base and the tip.
- Flakiness: twice a unit run hit a base flake of the ETXTBSY family (named in the header); reruns passed. No flake seen in the tests this plan adds (the daemon fake installer is written once, D14).
- Clippy (snap toolchain), base and tip: 74 findings each, identical by file and message; none in `daemon/{manifest,update}.rs`, `tui/{claude_update,plugins,plugin_setup}.rs`, `config.rs` or `claude/update.rs`, and none on an added line elsewhere. rustfmt: the five clean files check clean at the tip; added lines in the other touched files are formatted as rustfmt would.
- Safety of the installer tests: no test can reach a real `curl`. The daemon tests' `PATH` is the control dir plus the fake installer dir (links to `sh`, `bash`, `cat`, `cp`, `mkdir`, `sleep`, and a fake `curl`); the manager test's `PATH` is one test dir and its login shell a stand-in (D13). The real installer was never run.
- Not verified here: macOS (CI builds and unit-tests it; the manager test was run on Linux only, its tools are looked up in `/usr/bin` or `/bin`, where macOS has them), `make` and `make static`, the release workflow, a real claude, proxy or SSH host (the remote install path is covered by unit and daemon tests; Merge and release point 2 is the manual check), an old v0.7.0 daemon against this client (no new field is parsed; an old daemon's reasons carry no retry suffix and are not re-asked, as before).
