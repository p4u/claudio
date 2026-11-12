# Switch subscription and continue on a usage limit: implementation plan

Spec: `docs/specs/2026-11-12-limit-auto-switch-design.md` (audited 2026-11-12; `.delivery/2026-11-12-limit-auto-switch/audit-spec.md` F1-F11 folded in)
Class: architectural
Repos and branches: /home/p4u/claudio → feat/limit-auto-switch (base 88f344d)

A second delivery, `docs/specs/2026-11-12-ssh-agent-forwarding-design.md` on
`feat/agent-forwarding`, runs in parallel from the same base. This plan's edits to files that
delivery also touches are small and localized; the hunks that may conflict are listed under
"Shared files" at the end of Global Constraints. Whichever branch merges second resolves them,
keeping both sides.

Every patch below was generated from a scratch worktree of 88f344d and replayed there with
`git apply --whitespace=error-all`, task after task. For each task, step 1 fails as written
under "Expected" and step 2 passes with the counts given. After each code task `cargo fmt
--all --check`, `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`
and `scripts/check_rules.py` passed. On the final tree these passed too: `make check`'s
sequence, the five `make matrix` cells, and the `manager` (43), `vocdoni_manager` (17), `web`
(52), `web_manager` (4) and `vocdoni_web` (13) suites, plus the core-only manager suite and
`cargo check --release -p claudio-core`. Not run: the opt-in real-claude E2E (AC6; it needs an
authenticated claude and costs tokens; only its skip path ran, and its trigger and watcher were
checked once against a fake claude) and `make test-web` (no page change; the controller runs it).

## Global Constraints
- Commit rule (verbatim): conventional commits `type(scope): lowercase summary`; no AI
  attribution, no Co-Authored-By or similar trailers; commit with explicit paths only
  (`git add <new files>` first, then `git commit -m … -- <paths>`); never `git add -A`; never
  push.
- Build output under the worktree's `target/` or `~/.cache/claudio/`, never `/tmp`.
- Subagents never dispatch subagents.
- Apply each patch exactly. The plan's patches are numbered 1-12 in order (each step says
  which). Extract patch N with its final newline, then apply it from the repo root:
  `awk -v n=N '/^````diff$/{c++; if(c==n){f=1; next}} f&&/^````$/{f=0} f' /home/p4u/claudio/docs/plans/2026-11-12-limit-auto-switch-plan.md > ~/.cache/claudio/limit-N.patch`
  then `git apply --whitespace=error-all ~/.cache/claudio/limit-N.patch`. (Copying a block by
  hand works too if the file ends with a newline; without one `git apply` reports a corrupt
  patch.) If a patch does not apply, stop and report: do not hand-merge it.
- `make check` passes at every commit (CI runs it with the latest stable toolchain; this plan
  was checked with rustup stable 1.99).
- ACCEPTANCE.md throughout:
  - A1/A2/A3: the core never names vocdoni or the proxy. Detection and typing are generic, and
    the switch is the addon's. The new hook lands in the API in its own commit (Task 2), tested
    with `TestAddon`.
  - A6/A10: the guard is pure. Every task it starts is bounded: the attempt's timeout and abort,
    one-shot timers, the `seq` stale-guard, a weak channel, and an end at shutdown.
  - A8: `session.rs` (baseline 1036), `session.rs::spawn` (105) and `::run` (86) must not grow.
    `addons/vocdoni/src/daemon/mod.rs` (862) gets no edit. `daemon/mod.rs` must stay under 800.
    Leave `scripts/size-baseline.txt` unchanged.
  - C1: the session env and the proxy token are never logged, put in a notice, or printed by
    `Debug` (`UsageLimit` has none). The token goes only to the session's own proxy URL.
  - C2: every string an addon returns is cleaned by the core before a notice uses it, and so is
    claude's message (≤ 256 bytes).
  - D3: no blocking I/O on the runtime (the config read is `spawn_blocking`).
  - E2: no test weakened. E3: tests are hermetic.
  - B1/B2: no daemon or web protocol change. If one becomes necessary, stop and ask the
    controller.
- Tests are hermetic: fake claude, fake proxy and the suites' isolated env only. The owner runs
  their own `claudio --daemon`, `claudio web` and claude-proxy: never touch, signal or connect to
  them. The real-claude E2E (AC6) runs only with `CLAUDIO_E2E=1`, and only the controller runs it.
- Linux and macOS: no `/proc`, no `setsid` in tests. Shell fakes use POSIX `sh`, `stty`, `dd`,
  `od`, `tr`, `sed` and `printf` (never `echo` for data: dash's `echo` reads backslashes).
- Do not reformat, reorder or "tidy" unrelated code, least of all in the shared files listed at
  the end of this section.

**Decisions this plan made where the spec left them open (binding).**

1. **Where the API lives.** `UsageLimit`, `LimitOutcome`, `DaemonOps::on_usage_limit` and
   `DaemonCtx::config` go in `crates/core/src/addon/daemon.rs`, beside `DaemonOps`. The existing
   `pub use daemon::*` re-exports them as `claudio_core::addon::{UsageLimit, LimitOutcome}`. The
   cleaning is `addon::clean::limit_outcome` with `LIMIT_TEXT_BYTES = 120`, applied by the core
   in `daemon::addon_ops::usage_limit`, never by the addon (C2).
2. **`DaemonCtx::config` (D3).** The method is sync, as the spec gives it, so it cannot read a
   file without blocking the runtime. The daemon reads `config.toml` once each time it makes an
   addon's context: per op, per `on_usage_limit` call and per `on_claude_installed`. It reads off
   the runtime (`spawn_blocking`, `service::read_config`: a regular file, at most 1 MiB, TOML)
   and keeps only the addon's declared sections that are tables (`addon_ops::ctx`). The spec's
   purpose holds: an edit to `[proxy] auto_switch` applies at the next hit without a restart.
   `on_claude_installed`'s caller becomes async for this (`daemon/update.rs` awaits it).
3. **The actor's code goes in `daemon/session/limit.rs`, a child module of `session`.** It can
   add `impl Actor` methods there. `session.rs` is at its A8 baseline (1036 lines) and
   `session.rs::run` (86) and `::spawn` (105) may not grow. So `run` gets no new `select!` arm.
   The guard's timers are tasks that send the actor a `Cmd` later. `session.rs` shrinks by 3
   lines: `read_git_branch` moves to `git_status::head_branch`, and `spawn` stays at 105 by
   inlining `spawn_time`.
4. **One `Cmd` variant: `Cmd::Limit(limit::Step)`.** `Step` is `Done { seq, outcome }` (the
   spec's `Cmd::LimitDone`), `Settled { seq, from, to }` (the 1 s settle is over) and
   `Enter { seq, from, to }` (the 150 ms dwell is over). `from`/`to` travel in the step, so a
   newer attempt can never relabel an older one's notice.
5. **The actor's own channel, weakly.** `AutoSwitch` holds `tx.downgrade()`. Timers and the
   attempt upgrade it only to send. A strong sender in the actor would keep its channel open
   forever and break `run`'s `None` → kill path. After a respawn the old receiver is gone, so
   a late report is dropped (spec: "never reaches a new actor").
6. **Client input vs the daemon's writes.** `Cmd::Input` is sent only by `server.rs` for a
   client's `D` frame. The daemon's own bytes (`continue`, `\r`, probe replies) go through
   `Actor::write` directly. **Gap decided:** a front-end's focus reports (`ESC [ I`/`ESC [ O`)
   and SGR mouse reports are not typing (`limit::is_typing`). The TUI forwards a focus report
   to the active session whenever its window gains or loses focus (when claude enabled focus
   reporting; `crates/tui/src/interaction.rs:624`). Counting them would turn the feature off
   for the active session as soon as its user switched windows, the very case it is for.
   Anything else, an unknown sequence included, counts as typing (fail-safe).
7. **The Enter step re-checks the guard.** Both `Settled` and `Enter` require `seq` unchanged,
   state `Error` and no typing (`Guard::may_type`). Goal 2 ("never when someone typed") covers
   the 150 ms between `continue` and Enter too. If the check fails at Enter, `continue` stays
   in the box unsubmitted and the "scheduled" notice shows.
8. **"The process is alive"** is the actor handling the step: the child's exit ends the actor,
   and a step queued after it is dropped.
9. **A second hit while an attempt is in flight** raises `seq`, so per the spec's rule
   (`seq == limit_seq`) the first attempt's `Switched` does not type and shows the "scheduled"
   notice. A test pins this.
10. **The attempt** (`limit::attempt`): it copies the env out of `Registry.session_env` under
    the lock, in a sync method, before any await. It runs `addon_ops::usage_limit` (daemon-side
    addons in registry order, the first answer other than `NotMine`, strings cleaned) in a child
    `tokio::spawn` under `tokio::time::timeout(Config::limit_attempt_timeout)`. A timeout aborts
    the child and gives `Failed("timed out")`; a `JoinError` gives `Failed("addon panicked")`.
    It logs the outcome only, never the env.
11. **`Config::limit_attempt_timeout`** (default 20 s) goes right after `addon_timeout`, not at
    the struct's end, to stay off any field the parallel delivery appends. Tests shorten it
    with struct update (`..TestDaemon::config(&dir)`). Every full `Config { … }` literal gets
    the line (`perl` one-liner in Task 4).
12. **Daemon tests** drive hooks with `test_support::send_hook`, the relay's own frame. A lib
    test has no claudio binary, so its fake claude cannot run the real relay. The fake claude
    puts its terminal in raw mode and logs each `read` of its stdin as one line, so `continue`
    and the separate `\r` show as two lines. The `vocdoni_manager` test (AC4) runs the real
    relay from the merged `--settings`, as `crates/tui/tests/manager.rs:1708` does.
13. **AC6's trigger:** `CLAUDIO_TEST_LIMIT_SWITCH=1` in the daemon's env, debug builds only
    (`#[cfg(debug_assertions)]`, the `CLAUDIO_TEST_REMOTE_PEER` pattern). It turns each
    session's first `Stop` into a usage-limit `StopFailure`, and that hit's attempt answers
    `Switched { test → test }` without asking an addon. A release build compiles it to `false`.
    `make static`'s R8 grep gains the name, so a release binary that contains it fails the build.
14. **AC3 reuses the stubs.** `proxy::api`'s test module becomes `pub(crate)`, with `serve`,
    `switch_router` and `switch_router_with` `pub(crate)`. "No request sent" is checked with a
    counting axum middleware around `switch_router`.
15. **Vocdoni's code goes in `proxy/limit.rs`, declared in `proxy/mod.rs`.**
    `addons/vocdoni/src/daemon/mod.rs` is at its 862-line baseline, so even a `mod` line there
    would grow it. `ops.rs` delegates `on_usage_limit` with one call.
16. **Timeouts in the addon.** `switch_session` keeps its own 5 s per request; the addon wraps
    the whole call in 15 s, which leaves room for its capability probe after a 404. A 404 that
    the probe shows as "too old" keeps the client's own text, "proxy too old for switching".

**Shared files with feat/agent-forwarding.** The hunks that may conflict, all small:
- `crates/core/src/daemon/mod.rs`: `Config` gets `limit_attempt_timeout` (+2 lines after
  `addon_timeout`). `Config::from_env` gets one line after `addon_timeout:`, and the test
  literal in `claude_path_tests::config` one line. The production part goes from 780 to 783
  lines, out of the 800 budget that both deliveries share (A8).
- `crates/core/src/daemon/command.rs`: one line in each of its five test `Config` literals,
  after `addon_timeout`.
- `crates/core/src/daemon/test_support.rs`: one line in `TestDaemon::config`.
- `docs/manager-design.md`: a new subsection at the end of §2.2, before "### 2.3".
- `CLAUDE.md`: one sentence at the end of the "vocdoni" bullet in "Core and addons".
- If the other branch also adds a `Config` field, the side that merges second adds the other
  side's field to every literal it touches.

Not shared, but placed to avoid trouble: AC6 goes after `test_e2e_real_claude` in
`crates/tui/tests/manager.rs`, not at the end of the file. The `Makefile` change is the R8
grep only.

## Review Focus
- The predicate, `crates/core/src/claude/hooks.rs:52` (`is_usage_limit`): an exact `rate_limit`,
  the trimmed message, claude 2.1.322's prefixes. The proxy's own "API Error: 429 proxy: usage
  limit reached" must not match.
- The guard, `crates/core/src/daemon/session/limit.rs:76-164`. `hit` (135) raises `seq` on every
  hit, says nothing while an attempt is pending, and checks the cap before the conversation.
  `step` (101) applies `may_type` (161: seq, `Error`, no typing) to both `Settled` and `Enter`;
  the count rises only after Enter is written; a `Stop` resets it and `UserPromptSubmit` clears
  the typing flag.
- Timers and dwell, `limit.rs:271-278` (`later`): one-shot tasks, a 1 s settle, a 150 ms dwell,
  `continue` and `\r` as two writes. Nothing sleeps on the actor, and `Actor::run` is unchanged.
- The LimitDone and panic path, `limit.rs:284-310` (`attempt`). The child task runs under the
  timeout, is aborted on expiry (301), and a `JoinError` is "addon panicked" (299). The report
  goes through the weak sender (306), so it is dropped after a respawn. The actor holds only
  `tx.downgrade()` (`crates/core/src/daemon/session.rs:266`). `pending` is cleared by every
  `Done`.
- Client input vs the daemon's writes: `session.rs:826` (`Cmd::Input` → `client_input`,
  `limit.rs:212`) and `is_typing` (`limit.rs:250`, slice patterns, no indexing). The daemon's
  writes never pass through `client_input`.
- The env handed to addons. `crates/core/src/daemon/addon_ops.rs:167-190` (`usage_limit`) uses
  a lookup closure and logs nothing; `limit.rs:314` (`spawn_env`) is sync and releases the lock
  before returning; `crates/core/src/addon/daemon.rs:50` (`UsageLimit`) has no `Debug`; and
  `limit.rs:305` logs the outcome only.
- `DaemonCtx::config` scoping: `addon_ops.rs:56-70` (`ctx`: declared sections only, tables
  only, `spawn_blocking`, the supervisor's 1 MiB `read_config`), `addon_ops.rs:108` and the
  default at `addon/daemon.rs:129`.
- Cleaning: `crates/core/src/addon/clean.rs:182` (`limit_outcome`), applied at
  `addon_ops.rs:186` before the actor sees any string.
- The vocdoni token, `addons/vocdoni/src/proxy/limit.rs:16-45`. `auto_switch` defaults to true.
  `NotMine` unless both env vars are present and non-empty. The token goes only to that env's
  own URL, under the 15 s timeout, and the error texts come from `SwitchError` (no token, no
  header).
- The test trigger, `limit.rs:322-335`: `#[cfg(debug_assertions)]` only, and the `Makefile`
  R8 grep at lines 56-57.

## Task order and parallelism
| Task | Depends on | Repo/files | May run in parallel with |
|---|---|---|---|
| 1 Docs: the usage-limit hook and `DaemonCtx::config` | none | docs/addons.md | 3 |
| 2 The addon API and the daemon's `DaemonCtx::config` | 1 | crates/core/src/addon/{daemon,clean,test_support}.rs, crates/core/src/daemon/{addon_ops,service,update}.rs, crates/core/src/daemon/tests/addon_ops.rs | 3 |
| 3 `claude::hooks::is_usage_limit` | none | crates/core/src/claude/hooks.rs | 1, 2 |
| 4 The daemon switches and continues | 2, 3 | crates/core/src/daemon/{session.rs,session/limit.rs,git_status.rs,mod.rs,addon_ops.rs,command.rs,test_support.rs,tests.rs,tests/limit.rs}, Makefile | 5 |
| 5 Vocdoni's `on_usage_limit` | 2 | addons/vocdoni/src/proxy/{limit,mod,api}.rs, addons/vocdoni/src/daemon/ops.rs | 3, 4 |
| 6 End-to-end tests (AC4, AC6) | 4, 5 | addons/vocdoni/tests/manager.rs, crates/tui/tests/manager.rs | 7 |
| 7 Docs: design, README, vocdoni, CLAUDE.md | 4, 5 | docs/addons.md, docs/manager-design.md, README.md, CLAUDE.md | 6 |

The commits land in task order on one branch: 1, 2, 3, 4, 5, 6 (two commits), 7. Tasks that
may run in parallel touch disjoint files. They still commit one at a time, in order.

## Tasks

### Task 1: Docs: the usage-limit hook and `DaemonCtx::config`
Model: haiku   Repo: /home/p4u/claudio   Files: `docs/addons.md`
Goal: B2-style docs first (AC5, docs/addons.md "the new method, first"). Document
`DaemonOps::on_usage_limit`, `UsageLimit`, `LimitOutcome` and `DaemonCtx::config` in "Daemon
ops", and their two limits in "Limits". The Vocdoni paragraph comes in Task 7.

Step 1: apply the patch (patch 1)
````diff
diff --git a/docs/addons.md b/docs/addons.md
index 73f25f3..11147ea 100644
--- a/docs/addons.md
+++ b/docs/addons.md
@@ -304,9 +304,54 @@ through its spawn env. The daemon logs an op as addon, op and body length, never
 
 `DaemonCtx` gives the op `claude_bin()`, the daemon's login-shell `path()`, `claude_flags()`
 (the flags this claude's `--help` lists, probed once), `cache_dir()` (`<cache>/<addon>`, which
-the addon creates 0700), and `updating()`, the lock held while `claude update` or a `claude
-plugin …` runs, so `~/.claude` has one writer. `on_claude_installed` is called after the daemon
-installs claude.
+the addon creates 0700), `updating()`, the lock held while `claude update` or a `claude
+plugin …` runs, so `~/.claude` has one writer, and `config(section)`: one of the addon's
+declared `config.toml` sections on this host. The daemon reads the file (off the runtime, at
+most 1 MiB) when it makes the context, once per op, per `on_usage_limit` and per
+`on_claude_installed`, so an edit applies at the next call without a daemon restart; an
+undeclared section, or a file that is missing, over 1 MiB or not TOML, reads as empty.
+`on_claude_installed` is called after the daemon installs claude.
+
+**Usage limits.** When a claude session's turn ends on a usage limit (claude's `StopFailure`
+hook with `error` `rate_limit` and a message that starts with one of claude's own usage-limit
+prefixes, `claude::hooks::USAGE_LIMIT_PREFIXES`), the session's daemon calls
+`on_usage_limit(hit, ctx)` of each daemon-side addon in registry order, on a task off the
+session, and takes the first answer that is not `LimitOutcome::NotMine` (the default).
+
+```rust
+fn on_usage_limit<'a>(&'a self, hit: UsageLimit<'a>, ctx: Arc<dyn DaemonCtx>)
+    -> BoxFuture<'a, LimitOutcome>;
+
+pub struct UsageLimit<'a> {
+    pub conversation: &'a str,                              // claude's session id
+    pub message: &'a str,                                   // sanitised, one line, ≤ 256 bytes
+    pub env: &'a (dyn Fn(&str) -> Option<String> + Sync),   // the session's spawn env, by name
+}
+pub enum LimitOutcome {
+    NotMine,                                 // not this addon's session, or switched off here
+    Switched { from: String, to: String },   // the next message goes to another subscription
+    Failed(String),                          // tried; why it did not work
+}
+```
+
+`env` reads the session's spawn env, secrets included: an addon never logs a value, and
+`UsageLimit` has no `Debug`. The whole round has 20 s (`daemon::Config::limit_attempt_timeout`);
+past it the outcome is `Failed("timed out")`, and a future that panics is `Failed("addon
+panicked")`. The core cleans the outcome's strings (one safe line, at most 120 bytes) and shows
+them as session notices:
+- `Failed(why)`: `usage limit: could not switch subscription: <why>`.
+- `Switched { from, to }`: after a 1 s settle the daemon types `continue`, and 150 ms later
+  Enter as a separate write, then shows `usage limit: switched <from> → <to>, continuing (n/3)`.
+  It types only while the session still sits in the error of that hit, no newer hit came, and
+  no client typed since its last submitted prompt (a front-end's focus and mouse reports do
+  not count). Otherwise it types nothing more and shows `usage limit: switch to <to>
+  scheduled; not continuing automatically`.
+- `NotMine` from every addon: nothing.
+
+One attempt runs per session at a time; a hit meanwhile starts none. After 3 automatic
+continues in a row the next hit shows `usage limit: 3 automatic switches in a row; continue by
+hand` and asks no addon; a turn that ends normally (`Stop`) starts the count over. The daemon
+acts with no client attached, once per hit whatever the number of clients.
 
 **Services.** `DaemonOps::service(config)` maps the addon's own config sections (`config` hands it
 only those) to the argv of one long-running child the daemon keeps running: `Ok(Some(argv))`, the
@@ -479,6 +524,8 @@ its flags: `--plain`, `--daemon`, `--slave`, `-p`, `--print`, `--api`, `-h`, `--
 | panel | 64 KiB encoded |
 | checklist | 256 items |
 | `AddonOp` body | 512 KiB encoded, each way |
+| `LimitOutcome` string | 120 bytes, one line |
+| `on_usage_limit` round, every addon's answer included | 20 s |
 | composed instructions | 8158 bytes |
 
 Errors an addon sees from `ctx.request` (`AddonError`): `NotConnected(host)`, `Missing(host)` (the
````
Run: `grep -c "on_usage_limit" docs/addons.md`
Expected: `4`

Step 2: commit
`git commit -m "docs(addons): the usage-limit hook and DaemonCtx::config" -- docs/addons.md`

Interfaces this task provides to later tasks: none (the contract the next tasks implement).

### Task 2: The addon API and the daemon's `DaemonCtx::config`
Model: sonnet   Repo: /home/p4u/claudio   Files: `crates/core/src/addon/daemon.rs`,
`crates/core/src/addon/clean.rs`, `crates/core/src/addon/test_support.rs`,
`crates/core/src/daemon/addon_ops.rs`, `crates/core/src/daemon/service.rs`,
`crates/core/src/daemon/update.rs`, `crates/core/src/daemon/tests/addon_ops.rs`
Goal: the new hook in the API, in its own commit, generic and tested without vocdoni (A3; spec
§2, "Compatibility": two defaulted trait methods). It covers the cleaning of outcomes (C2), the
daemon's `config(section)` scoped to declared sections and read per context (decision 2), and
`TestAddon`'s `on_limit` script for Task 4's tests.

Step 1: write the failing tests (`limit_outcome`'s cleaning; `DaemonCtx::config` through a
`config` op of `TestAddon`) (patch 2)
````diff
diff --git a/crates/core/src/addon/clean.rs b/crates/core/src/addon/clean.rs
index e461c56..048c195 100644
--- a/crates/core/src/addon/clean.rs
+++ b/crates/core/src/addon/clean.rs
@@ -176,6 +176,7 @@ pub fn notice(n: Notice) -> Notice {
     }
 }
 
+
 /// A one-page panel that says why the real one is not shown.
 pub fn error_panel(title: &str, why: &str) -> Panel {
     Panel {
@@ -335,4 +336,24 @@ mod tests {
         });
         assert_eq!((o.label.as_str(), o.detail.as_deref()), ("L", Some("x")));
     }
+
+    #[test]
+    fn limit_outcomes_are_one_safe_line_of_at_most_120_bytes() {
+        let long = format!("sub\x1b[31m-a\n{}", "é".repeat(100));
+        let LimitOutcome::Switched { from, to } = limit_outcome(LimitOutcome::Switched {
+            from: long.clone(),
+            to: "b\x07".into(),
+        }) else {
+            panic!("not switched");
+        };
+        assert!(from.len() <= LIMIT_TEXT_BYTES, "{}", from.len());
+        assert!(from.starts_with("sub[31m-a "), "{from}");
+        assert!(!from.contains('\x1b') && !from.contains('\n'));
+        assert_eq!(to, "b");
+        assert_eq!(
+            limit_outcome(LimitOutcome::Failed("no\talternative".into())),
+            LimitOutcome::Failed("no alternative".into())
+        );
+        assert_eq!(limit_outcome(LimitOutcome::NotMine), LimitOutcome::NotMine);
+    }
 }
diff --git a/crates/core/src/daemon/tests/addon_ops.rs b/crates/core/src/daemon/tests/addon_ops.rs
index 69b2022..3a059d3 100644
--- a/crates/core/src/daemon/tests/addon_ops.rs
+++ b/crates/core/src/daemon/tests/addon_ops.rs
@@ -192,3 +192,45 @@ async fn unknown_op_from_a_newer_client_is_refused() {
         .await;
     assert_eq!(reply, error("unsupported op: unknown"));
 }
+
+/// `DaemonCtx::config` hands an addon only the sections it declares, read
+/// afresh for each op, and nothing from a file over 1 MiB.
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn daemon_ctx_config_is_the_declared_sections_read_per_op() {
+    let dir = TestDaemon::new_dir();
+    let file = dir.join("config.toml");
+    std::fs::write(&file, "top = 1\n[echo]\na = 1\n[other]\nb = 2\n").unwrap();
+    let echo = Arc::new(TestAddon::new("echo").sections(&["echo"]).with_daemon());
+    let other = Arc::new(TestAddon::new("other").sections(&["other"]));
+    let addons = Addons::new(vec![echo as Arc<dyn Addon>, other]).unwrap();
+    let config = Config {
+        config_file: Some(file.clone()),
+        ..TestDaemon::config(&dir)
+    };
+    let d = TestDaemon::start_with_addons(dir, config, addons);
+    let mut c = d.client().await;
+    assert_eq!(section(&mut c, "echo").await, json!({"a": 1}));
+    let theirs = section(&mut c, "other").await;
+    assert_eq!(theirs, json!({}), "another addon's section");
+    assert_eq!(section(&mut c, "top").await, json!({}), "not a table");
+    std::fs::write(&file, "[echo]\na = 2\n").unwrap();
+    assert_eq!(
+        section(&mut c, "echo").await,
+        json!({"a": 2}),
+        "read afresh"
+    );
+    let big = format!("[echo]\na = 3\npad = \"{}\"\n", "x".repeat(1 << 20));
+    std::fs::write(&file, big).unwrap();
+    assert_eq!(section(&mut c, "echo").await, json!({}), "over 1 MiB");
+}
+
+/// `echo`'s `DaemonCtx::config(name)`, through its `config` op.
+async fn section(c: &mut Client, name: &str) -> Value {
+    match c
+        .call(op("echo", "config", json!({ "section": name })))
+        .await
+    {
+        Msg::AddonReply { body } => body,
+        other => panic!("expected a reply, got {other:?}"),
+    }
+}
````
Run: `cargo test --locked -p claudio-core --lib -- limit_outcomes daemon_ctx_config`
Expected: ``error[E0425]: cannot find function `limit_outcome` in this scope`` (and the same
for `LimitOutcome` and `LIMIT_TEXT_BYTES`)

Step 2: implement (patch 3)
````diff
diff --git a/crates/core/src/addon/clean.rs b/crates/core/src/addon/clean.rs
index 048c195..c35e4ca 100644
--- a/crates/core/src/addon/clean.rs
+++ b/crates/core/src/addon/clean.rs
@@ -2,8 +2,8 @@
 //! The runtime applies them, never the addon.
 
 use super::{
-    name_ok, Block, Checklist, ChecklistItem, ExtensionRow, LaunchOption, Notice, Page, Panel,
-    PanelAction, Segment, Tone,
+    name_ok, Block, Checklist, ChecklistItem, ExtensionRow, LaunchOption, LimitOutcome, Notice,
+    Page, Panel, PanelAction, Segment, Tone,
 };
 
 /// A segment's text, in characters.
@@ -14,6 +14,8 @@ pub const PANEL_BYTES: usize = 64 * 1024;
 pub const CHECKLIST_ITEMS: usize = 256;
 /// Any one string of a panel, checklist, row, option or notice, in bytes.
 pub const TEXT_BYTES: usize = 4096;
+/// Any one string of a [`LimitOutcome`], in bytes: it goes into a one-line notice.
+pub const LIMIT_TEXT_BYTES: usize = 120;
 
 /// `s` as one safe line: `text::sanitize`, newlines and tabs as spaces,
 /// cut at `max_bytes` on a char boundary.
@@ -176,6 +178,18 @@ pub fn notice(n: Notice) -> Notice {
     }
 }
 
+/// A usage-limit outcome with every string one safe line of at most [`LIMIT_TEXT_BYTES`].
+pub fn limit_outcome(o: LimitOutcome) -> LimitOutcome {
+    let cut = |s: String| line(&s, LIMIT_TEXT_BYTES);
+    match o {
+        LimitOutcome::NotMine => LimitOutcome::NotMine,
+        LimitOutcome::Switched { from, to } => LimitOutcome::Switched {
+            from: cut(from),
+            to: cut(to),
+        },
+        LimitOutcome::Failed(why) => LimitOutcome::Failed(cut(why)),
+    }
+}
 
 /// A one-page panel that says why the real one is not shown.
 pub fn error_panel(title: &str, why: &str) -> Panel {
diff --git a/crates/core/src/addon/daemon.rs b/crates/core/src/addon/daemon.rs
index 5f0cb5b..0c0da1e 100644
--- a/crates/core/src/addon/daemon.rs
+++ b/crates/core/src/addon/daemon.rs
@@ -32,6 +32,45 @@ pub trait DaemonOps: Send + Sync {
         let _ = config;
         Ok(None)
     }
+    /// A session of this daemon ended a turn on a usage limit (docs/addons.md, "Usage
+    /// limits"). Asked in registry order until one answers other than `NotMine`, off the
+    /// session's actor, within `daemon::Config::limit_attempt_timeout` (20 s) for the round.
+    fn on_usage_limit<'a>(
+        &'a self,
+        hit: UsageLimit<'a>,
+        ctx: Arc<dyn DaemonCtx>,
+    ) -> BoxFuture<'a, LimitOutcome> {
+        let _ = (hit, ctx);
+        Box::pin(async { LimitOutcome::NotMine })
+    }
+}
+
+/// A usage-limit hit, as `DaemonOps::on_usage_limit` gets it. No `Debug`: `env` reads the
+/// session's spawn env, secrets included (C1).
+pub struct UsageLimit<'a> {
+    /// claude's session id (the conversation).
+    pub conversation: &'a str,
+    /// claude's message, sanitised, one line, at most 256 bytes.
+    pub message: &'a str,
+    /// The session's spawn env, by name. Never log a value.
+    pub env: &'a (dyn Fn(&str) -> Option<String> + Sync),
+}
+
+/// What an addon did about a usage-limit hit. The core cleans every string (one safe line,
+/// at most 120 bytes) before a notice shows it.
+#[derive(Debug, Clone, PartialEq, Eq)]
+pub enum LimitOutcome {
+    /// Not this addon's session, or the addon is switched off here.
+    NotMine,
+    /// The conversation's next message goes to another subscription.
+    Switched {
+        /// The subscription it leaves.
+        from: String,
+        /// The subscription it moves to.
+        to: String,
+    },
+    /// The addon tried; why it did not work.
+    Failed(String),
 }
 
 /// An addon service in this daemon (docs/addons.md, "Daemon ops").
@@ -84,6 +123,13 @@ pub trait DaemonCtx: Send + Sync {
     fn service_reload(&self, force: bool) -> BoxFuture<'static, ServiceState>;
     /// This addon's service as of now.
     fn service_state(&self) -> ServiceState;
+    /// A declared section of this host's `config.toml` as it was when this context was
+    /// made (each op and each `on_usage_limit` reads the file afresh, off the runtime);
+    /// empty for an undeclared section, or a file that is missing, over 1 MiB or not TOML.
+    fn config(&self, section: &str) -> toml::Table {
+        let _ = section;
+        toml::Table::new()
+    }
 }
 
 /// Lets claude offer bypass-permissions mode; any mention in `--help` counts.
diff --git a/crates/core/src/addon/test_support.rs b/crates/core/src/addon/test_support.rs
index 035cd59..cef223a 100644
--- a/crates/core/src/addon/test_support.rs
+++ b/crates/core/src/addon/test_support.rs
@@ -25,6 +25,17 @@ pub enum Script {
     Review(LaunchPart),
 }
 
+/// What `on_usage_limit` answers.
+#[derive(Debug, Clone)]
+pub enum LimitScript {
+    /// This outcome.
+    Answer(LimitOutcome),
+    /// The inner answer after a delay.
+    Delay(Duration, Box<LimitScript>),
+    /// A panic inside the future.
+    Panic,
+}
+
 /// The fake addon. Build with `TestAddon::new(name)` and the builder methods.
 pub struct TestAddon {
     name: &'static str,
@@ -47,6 +58,8 @@ pub struct TestAddon {
     saved: AtomicBool,
     installs: AtomicUsize,
     calls: Mutex<Vec<String>>,
+    /// What `on_usage_limit` answers.
+    limit: Mutex<LimitScript>,
     /// How long its start's task reads its state, if it has one.
     start_load: Option<Duration>,
     /// That task has read it.
@@ -77,6 +90,7 @@ impl TestAddon {
             saved: AtomicBool::new(false),
             installs: AtomicUsize::new(0),
             calls: Mutex::new(Vec::new()),
+            limit: Mutex::new(LimitScript::Answer(LimitOutcome::NotMine)),
             start_load: None,
             loaded: Arc::default(),
         }
@@ -159,7 +173,8 @@ impl TestAddon {
         self
     }
 
-    /// Has `DaemonOps`: `echo`, `sleep` (`{"secs": n}`), `big` (600 KiB), `fail`.
+    /// Has `DaemonOps`: `echo`, `sleep` (`{"secs": n}`), `big` (600 KiB), `fail`, and
+    /// `config` (`{"section": name}`: `DaemonCtx::config` as JSON).
     pub fn with_daemon(mut self) -> Self {
         self.has_daemon = true;
         self
@@ -174,6 +189,18 @@ impl TestAddon {
         self
     }
 
+    /// Has `DaemonOps`; `on_usage_limit` follows `script`.
+    pub fn on_limit(mut self, script: LimitScript) -> Self {
+        self.limit = Mutex::new(script);
+        self.has_daemon = true;
+        self
+    }
+
+    /// What `on_usage_limit` answers from now on.
+    pub fn set_limit(&self, script: LimitScript) {
+        *lock(&self.limit) = script;
+    }
+
     /// `claudio <word>` exits with `code`.
     pub fn command(mut self, word: &str, code: u8) -> Self {
         self.words.insert(word.into(), code);
@@ -477,10 +504,44 @@ impl DaemonOps for TestAddon {
                     Ok(Value::String(format!("{state:?}")))
                 }
                 "state" => Ok(Value::String(format!("{:?}", ctx.service_state()))),
+                "config" => {
+                    let section = body.get("section").and_then(Value::as_str).unwrap_or("");
+                    serde_json::to_value(ctx.config(section)).map_err(|e| e.to_string())
+                }
                 other => Err(format!("unknown op {other}")),
             }
         })
     }
+    /// Records `usage_limit <conversation> <message> <env>`, `<env>` saying whether the
+    /// session's `SECRET_ENV` (the daemon harness's spec) reached it, never its value.
+    fn on_usage_limit<'a>(
+        &'a self,
+        hit: UsageLimit<'a>,
+        ctx: Arc<dyn DaemonCtx>,
+    ) -> BoxFuture<'a, LimitOutcome> {
+        let _ = ctx;
+        let env = match (hit.env)("SECRET_ENV") {
+            Some(_) => "SECRET_ENV=<set>",
+            None => "SECRET_ENV=<unset>",
+        };
+        self.record(format!(
+            "usage_limit {} {} {env}",
+            hit.conversation, hit.message
+        ));
+        let mut script = lock(&self.limit).clone();
+        Box::pin(async move {
+            loop {
+                match script {
+                    LimitScript::Answer(outcome) => return outcome,
+                    LimitScript::Delay(wait, inner) => {
+                        tokio::time::sleep(wait).await;
+                        script = *inner;
+                    }
+                    LimitScript::Panic => panic!("the test addon panics on a usage limit"),
+                }
+            }
+        })
+    }
     fn on_claude_installed(&self, ctx: &Arc<dyn DaemonCtx>) {
         let _ = ctx;
         self.record("installed".into());
diff --git a/crates/core/src/daemon/addon_ops.rs b/crates/core/src/daemon/addon_ops.rs
index c81e333..139d52e 100644
--- a/crates/core/src/daemon/addon_ops.rs
+++ b/crates/core/src/daemon/addon_ops.rs
@@ -11,7 +11,7 @@ use std::time::Duration;
 use serde_json::Value;
 use tokio::sync::Semaphore;
 
-use super::Daemon;
+use super::{service, Daemon};
 use crate::addon::{Addons, BoxFuture, ClaudeFlags, DaemonCtx, ServiceState};
 use crate::proto::{self, Msg};
 
@@ -45,6 +45,26 @@ pub(crate) fn features(daemon: &Daemon) -> Vec<String> {
 pub(crate) struct Ctx {
     daemon: Arc<Daemon>,
     addon: &'static str,
+    /// The addon's declared sections of `config.toml`, read when the context was made.
+    config: toml::Table,
+}
+
+/// Addon `addon`'s context, with its declared `config.toml` sections read now, off the
+/// runtime, under the service supervisor's 1 MiB cap.
+pub(crate) async fn ctx(daemon: &Arc<Daemon>, addon: &'static str) -> Arc<dyn DaemonCtx> {
+    let sections = daemon
+        .addons
+        .get(addon)
+        .map_or(&[][..], |a| a.config_sections());
+    let path = daemon.config.config_file.clone();
+    let read = tokio::task::spawn_blocking(move || path.and_then(|p| service::read_config(&p)));
+    let mut config = read.await.ok().flatten().unwrap_or_default();
+    config.retain(|name, value| sections.contains(&name) && value.is_table());
+    Arc::new(Ctx {
+        daemon: Arc::clone(daemon),
+        addon,
+        config,
+    })
 }
 
 impl DaemonCtx for Ctx {
@@ -82,6 +102,13 @@ impl DaemonCtx for Ctx {
     fn service_state(&self) -> ServiceState {
         self.daemon.services.state(self.addon)
     }
+
+    fn config(&self, section: &str) -> toml::Table {
+        match self.config.get(section) {
+            Some(toml::Value::Table(t)) => t.clone(),
+            _ => toml::Table::new(),
+        }
+    }
 }
 
 /// Run one AddonOp: body cap, find the addon's DaemonOps, take a slot (none
@@ -114,10 +141,7 @@ async fn handle(daemon: Arc<Daemon>, addon: &str, op: &str, body: Value) -> Resu
     let _permit = Arc::clone(slot)
         .try_acquire_owned()
         .map_err(|_| format!("addon {addon} busy"))?;
-    let ctx: Arc<dyn DaemonCtx> = Arc::new(Ctx {
-        daemon: Arc::clone(&daemon),
-        addon: found.name(),
-    });
+    let ctx = ctx(&daemon, found.name()).await;
     let reply = tokio::time::timeout(daemon.config.addon_timeout, ops.handle(op, body, ctx))
         .await
         .map_err(|_| format!("addon {addon} timed out"))??;
@@ -127,14 +151,10 @@ async fn handle(daemon: Arc<Daemon>, addon: &str, op: &str, body: Value) -> Resu
 
 /// Every daemon-side addon's `on_claude_installed`: claude was just
 /// installed on this host.
-pub(crate) fn claude_installed(daemon: &Arc<Daemon>) {
+pub(crate) async fn claude_installed(daemon: &Arc<Daemon>) {
     for addon in daemon.addons.iter() {
         if let Some(ops) = addon.daemon() {
-            let ctx: Arc<dyn DaemonCtx> = Arc::new(Ctx {
-                daemon: Arc::clone(daemon),
-                addon: addon.name(),
-            });
-            ops.on_claude_installed(&ctx);
+            ops.on_claude_installed(&ctx(daemon, addon.name()).await);
         }
     }
 }
diff --git a/crates/core/src/daemon/service.rs b/crates/core/src/daemon/service.rs
index 2b09f3d..abe895f 100644
--- a/crates/core/src/daemon/service.rs
+++ b/crates/core/src/daemon/service.rs
@@ -332,7 +332,7 @@ async fn wanted(daemon: &Daemon, addon: &str) -> Result<Option<Vec<String>>, Str
 /// config.toml as a table: empty when missing, `None` when it cannot be
 /// read, is not a regular file (a FIFO would block the open), is over
 /// 1 MiB or is not TOML.
-fn read_config(path: &Path) -> Option<toml::Table> {
+pub(super) fn read_config(path: &Path) -> Option<toml::Table> {
     match std::fs::metadata(path) {
         Ok(meta) if meta.is_file() => {}
         Ok(_) => return None,
diff --git a/crates/core/src/daemon/update.rs b/crates/core/src/daemon/update.rs
index 21954e9..ac0c7bf 100644
--- a/crates/core/src/daemon/update.rs
+++ b/crates/core/src/daemon/update.rs
@@ -87,7 +87,7 @@ pub async fn run(daemon: &Arc<Daemon>, install: bool) -> Msg {
     if install && ok && version.is_some() {
         // Every daemon-side addon hears it (plugin installs that failed
         // for want of claude go at once).
-        super::addon_ops::claude_installed(daemon);
+        super::addon_ops::claude_installed(daemon).await;
     }
     Msg::ClaudeUpdated { version, ok, tail }
 }
````
Run: `cargo test --locked -p claudio-core --lib -- limit_outcomes daemon_ctx_config addon_ops`
Expected: `test result: ok. 8 passed; 0 failed`
Run: `make check`
Expected: every crate's `test result: ok`, `rules: 0 problem(s)`

Step 3: commit
`git commit -m "feat(addon): on_usage_limit, LimitOutcome and DaemonCtx::config" -- crates/core/src/addon/daemon.rs crates/core/src/addon/clean.rs crates/core/src/addon/test_support.rs crates/core/src/daemon/addon_ops.rs crates/core/src/daemon/service.rs crates/core/src/daemon/update.rs crates/core/src/daemon/tests/addon_ops.rs`

Interfaces this task provides to later tasks:
- `claudio_core::addon::{UsageLimit<'a> { conversation: &'a str, message: &'a str, env: &'a (dyn Fn(&str) -> Option<String> + Sync) }, LimitOutcome { NotMine, Switched { from, to }, Failed(String) }}`
- `DaemonOps::on_usage_limit<'a>(&'a self, UsageLimit<'a>, Arc<dyn DaemonCtx>) -> BoxFuture<'a, LimitOutcome>` (default `NotMine`)
- `DaemonCtx::config(&self, section: &str) -> toml::Table` (default empty)
- `claudio_core::addon::{limit_outcome, LIMIT_TEXT_BYTES}`
- `daemon::addon_ops::ctx(&Arc<Daemon>, &'static str) -> Arc<dyn DaemonCtx>` (async)
- `addon::test_support::{LimitScript { Answer, Delay, Panic }, TestAddon::on_limit, TestAddon::set_limit}`; `TestAddon` records `usage_limit <conversation> <message> SECRET_ENV=<set|unset>`

### Task 3: `claude::hooks::is_usage_limit`
Model: haiku   Repo: /home/p4u/claudio   Files: `crates/core/src/claude/hooks.rs`
Goal: §1's pure predicate and its prefix list (AC1).

Step 1: write the failing tests (patch 4)
````diff
diff --git a/crates/core/src/claude/hooks.rs b/crates/core/src/claude/hooks.rs
index 89165fb..8330a2e 100644
--- a/crates/core/src/claude/hooks.rs
+++ b/crates/core/src/claude/hooks.rs
@@ -494,4 +494,51 @@ mod tests {
         // A missing daemon socket is silently ignored.
         relay_from(&b"{}"[..], "Stop", Path::new("/nonexistent/sock"), "tok");
     }
+
+    // ── is_usage_limit ────────────────────────────────────────────────────────
+
+    fn stop_failure(error: &str, message: Value) -> Value {
+        serde_json::json!({"hook_event_name": "StopFailure", "error": error,
+                           "last_assistant_message": message})
+    }
+
+    #[test]
+    fn usage_limit_messages_of_claude_are_limits() {
+        for message in [
+            // The subscription variants (claude 2.1.322).
+            "You've hit your 5-hour limit · resets 4pm (UTC)",
+            "You've hit your weekly limit · resets Mon 9am (UTC)",
+            "You've hit your Opus limit · resets 4pm (UTC)",
+            "You've hit your monthly spend limit · ask your admin for a higher limit",
+            "You've hit your team budget · ask your admin for a higher limit",
+            // One per other prefix.
+            "You've reached your usage limit",
+            "You're out of usage credits · add funds to continue",
+            "Your org is out of usage · add funds to continue",
+            "Your seat type doesn't include usage credits",
+            "You're out of extra usage · resets 4pm (UTC)",
+            // Trimmed.
+            "  You've hit your 5-hour limit\n",
+        ] {
+            let payload = stop_failure("rate_limit", Value::from(message));
+            assert!(is_usage_limit(&payload), "{message:?}");
+        }
+    }
+
+    #[test]
+    fn other_failures_are_not_usage_limits() {
+        let hit = "You've hit your 5-hour limit · resets 4pm (UTC)";
+        let proxy = "API Error: 429 proxy: usage limit reached - try again later";
+        for payload in [
+            stop_failure("overloaded", Value::from(hit)),
+            stop_failure("rate_limit", Value::from(proxy)),
+            stop_failure("rate_limit", Value::Null),
+            stop_failure("rate_limit", serde_json::json!(429)),
+            serde_json::json!({"error": "rate_limit"}),
+            serde_json::json!({"last_assistant_message": hit}),
+            Value::Null,
+        ] {
+            assert!(!is_usage_limit(&payload), "{payload}");
+        }
+    }
 }
````
Run: `cargo test --locked -p claudio-core --lib -- usage_limit`
Expected: ``error[E0425]: cannot find function `is_usage_limit` in this scope``

Step 2: implement (patch 5)
````diff
diff --git a/crates/core/src/claude/hooks.rs b/crates/core/src/claude/hooks.rs
index 8330a2e..905a1df 100644
--- a/crates/core/src/claude/hooks.rs
+++ b/crates/core/src/claude/hooks.rs
@@ -35,6 +35,29 @@ pub const EVENTS: &[&str] = &[
     "SessionEnd",
 ];
 
+/// The prefixes of claude's own usage-limit messages: claude 2.1.322's `usage_limit_reached`
+/// list (subscription windows, credits, org and seat limits). The proxy's own per-user
+/// limit ("API Error: 429 proxy: usage limit reached …") matches none of them.
+pub const USAGE_LIMIT_PREFIXES: &[&str] = &[
+    "You've hit your",
+    "You've reached your",
+    "You're out of usage credits",
+    "Your org is out of usage",
+    "Your seat type doesn't include usage",
+    "You're out of extra usage",
+];
+
+/// Whether a `StopFailure` payload is a usage limit: `error` is `rate_limit` and
+/// `last_assistant_message`, trimmed, starts with one of [`USAGE_LIMIT_PREFIXES`].
+pub fn is_usage_limit(payload: &Value) -> bool {
+    let rate_limit = payload.get("error").and_then(Value::as_str) == Some("rate_limit");
+    let message = payload
+        .get("last_assistant_message")
+        .and_then(Value::as_str)
+        .map(str::trim);
+    rate_limit && message.is_some_and(|m| USAGE_LIMIT_PREFIXES.iter().any(|p| m.starts_with(p)))
+}
+
 /// The tools whose `PostToolUse` is relayed: the ones that wait for the user,
 /// so an ordinary tool call costs no extra hook process.
 pub const ASKING_TOOLS: &str = "AskUserQuestion|ExitPlanMode";
````
Run: `cargo test --locked -p claudio-core --lib -- usage_limit`
Expected: `test result: ok. 2 passed; 0 failed`
Run: `make check`
Expected: every crate's `test result: ok`, `rules: 0 problem(s)`

Step 3: commit
`git commit -m "feat(claude): tell claude's usage-limit stop from other failures" -- crates/core/src/claude/hooks.rs`

Interfaces this task provides to later tasks: `claude::hooks::{USAGE_LIMIT_PREFIXES, is_usage_limit(&Value) -> bool}`.

### Task 4: The daemon switches and continues a session that hits a usage limit
Model: opus   Repo: /home/p4u/claudio   Files: `crates/core/src/daemon/session/limit.rs` (new),
`crates/core/src/daemon/session.rs`, `crates/core/src/daemon/git_status.rs`,
`crates/core/src/daemon/mod.rs`, `crates/core/src/daemon/addon_ops.rs`,
`crates/core/src/daemon/command.rs`, `crates/core/src/daemon/test_support.rs`,
`crates/core/src/daemon/tests.rs`, `crates/core/src/daemon/tests/limit.rs` (new), `Makefile`
Goal: §1-§2: detection in the actor, the guard, the attempt and the type-dwell-Enter
sequence, plus the notices (AC2) and the debug-only AC6 trigger (decision 13). It is
concurrency work: the actor, timers and the `LimitDone` path.

This patch implements decisions 3-13. The `Config` field is added to every full literal by
hand in the patch; the same edit on a tree that has gained literals since (after a rebase on
feat/agent-forwarding) is
`perl -0pi -e 's/^(\s*)addon_timeout: std::time::Duration::from_secs\(150\),\n/$&$1limit_attempt_timeout: std::time::Duration::from_secs(20),\n/mg' crates/core/src/daemon/{test_support,command,mod,tests}.rs`.

Step 1: write the failing daemon tests (AC2: switched with two clients, repeated, failed,
not mine, client input, in flight, timeout, panic) (patch 6)
````diff
diff --git a/crates/core/src/daemon/tests.rs b/crates/core/src/daemon/tests.rs
index 525429f..396e632 100644
--- a/crates/core/src/daemon/tests.rs
+++ b/crates/core/src/daemon/tests.rs
@@ -1046,6 +1046,7 @@ async fn spawn_for_a_journaled_shell_starts_a_shell() {
 mod addon_ops;
 mod freshness;
 mod git;
+mod limit;
 mod login_path;
 mod service;
 mod spawn_env;
diff --git a/crates/core/src/daemon/tests/limit.rs b/crates/core/src/daemon/tests/limit.rs
new file mode 100644
index 0000000..aa2e3b7
--- /dev/null
+++ b/crates/core/src/daemon/tests/limit.rs
@@ -0,0 +1,310 @@
+//! Daemon tests for the usage-limit auto-switch (`session/limit.rs`): a
+//! `TestAddon` answers `on_usage_limit`, the test sends the hooks a claude
+//! would (`send_hook`), and a fake claude with a raw terminal logs each read
+//! of its input as one line of `input.log`, so two PTY writes stay two lines.
+
+use std::sync::OnceLock;
+
+use serde_json::json;
+
+use super::*;
+use crate::addon::test_support::{LimitScript, TestAddon};
+use crate::addon::{Addon, LimitOutcome};
+
+/// claude's message for the hits the tests send.
+const LIMIT: &str = "You've hit your 5-hour limit · resets 4pm (UTC)";
+
+/// The notice of a switch that was not continued.
+const NOT_CONTINUING: &str = "usage limit: switch to sub-b scheduled; not continuing automatically";
+
+/// A fake claude with a raw terminal: it saves its `--settings` (the hook
+/// token) and prints the banner like the shared fake, then appends each read
+/// of its stdin to `input.log` in its cwd as one line (`od -c`'s characters,
+/// spaces dropped: `continue`, `\r`).
+fn recording_claude() -> &'static Path {
+    static BIN: OnceLock<PathBuf> = OnceLock::new();
+    BIN.get_or_init(|| {
+        let dir = private_temp_dir("claudio-fake-claude-limit");
+        let bin = dir.join("claude");
+        let script = format!(
+            "#!/bin/sh\n\
+             if [ \"$1\" = \"--version\" ]; then echo '9.9.9 (Claude Code)'; exit 0; fi\n\
+             if [ \"$1\" = \"--help\" ]; then exit 0; fi\n\
+             stty raw -echo\n\
+             printf '%s' \"$2\" > settings.json\n\
+             echo {BANNER}\n\
+             while :; do\n\
+             chunk=$(dd bs=256 count=1 2>/dev/null | od -An -c | tr -d ' \\n')\n\
+             [ -n \"$chunk\" ] || exit 0\n\
+             printf '%s\\n' \"$chunk\" >> input.log\n\
+             done\n"
+        );
+        write_executable(&bin, &script);
+        bin
+    })
+}
+
+fn switched() -> LimitScript {
+    LimitScript::Answer(LimitOutcome::Switched {
+        from: "sub-a".into(),
+        to: "sub-b".into(),
+    })
+}
+
+/// A daemon running the recording claude, `addons` (in this order) and an
+/// attempt budget of `budget`.
+fn start_with(addons: &[&Arc<TestAddon>], budget: Duration) -> TestDaemon {
+    let dir = TestDaemon::new_dir();
+    let list = addons
+        .iter()
+        .map(|a| Arc::clone(a) as Arc<dyn Addon>)
+        .collect();
+    let config = Config {
+        claude: recording_claude().to_path_buf(),
+        limit_attempt_timeout: budget,
+        ..TestDaemon::config(&dir)
+    };
+    TestDaemon::start_with_addons(dir, config, Addons::new(list).unwrap())
+}
+
+/// A daemon whose one addon, `limit`, follows `script`.
+fn start(script: LimitScript) -> (TestDaemon, Arc<TestAddon>) {
+    let addon = Arc::new(TestAddon::new("limit").on_limit(script));
+    (start_with(&[&addon], Duration::from_secs(20)), addon)
+}
+
+/// Spawn a session, attach `c` to it, and tell the daemon its conversation
+/// (`conv-1`); its id and hook token.
+async fn session(d: &TestDaemon, c: &mut Client) -> (SessionId, String) {
+    let id = Uuid::new_v4();
+    c.spawn(d.spec(id)).await;
+    let token = d.token().await;
+    c.attach_until(id, BANNER).await;
+    let start = json!({"session_id": "conv-1", "source": "startup"});
+    send_hook(&d.config.socket, &token, "SessionStart", start).await;
+    (id, token)
+}
+
+/// A usage-limit `StopFailure`, as claude sends it.
+async fn hit(d: &TestDaemon, token: &str) {
+    let payload = json!({"hook_event_name": "StopFailure", "error": "rate_limit",
+                         "last_assistant_message": LIMIT});
+    send_hook(&d.config.socket, token, "StopFailure", payload).await;
+}
+
+/// The reads the fake claude logged so far.
+fn input(d: &TestDaemon) -> Vec<String> {
+    let log = std::fs::read_to_string(d.work().join("input.log")).unwrap_or_default();
+    log.lines().map(str::to_owned).collect()
+}
+
+/// The reads, once there are `n`.
+async fn input_until(d: &TestDaemon, n: usize) -> Vec<String> {
+    within(async {
+        loop {
+            let got = input(d);
+            if got.len() >= n {
+                return got;
+            }
+            tokio::time::sleep(Duration::from_millis(20)).await;
+        }
+    })
+    .await
+}
+
+/// The next notice of session `id`.
+async fn notice(c: &mut Client, id: SessionId) -> String {
+    c.event(id, |e| match e {
+        SessionEvent::Notice { text } => Some(text.clone()),
+        _ => None,
+    })
+    .await
+}
+
+/// The usage-limit calls `addon` recorded.
+fn asked(addon: &TestAddon) -> Vec<String> {
+    let calls = addon.calls();
+    calls
+        .into_iter()
+        .filter(|c| c.starts_with("usage_limit"))
+        .collect()
+}
+
+/// Until `addon` has been asked `n` times.
+async fn asked_until(addon: &TestAddon, n: usize) {
+    within(async {
+        while asked(addon).len() < n {
+            tokio::time::sleep(Duration::from_millis(20)).await;
+        }
+    })
+    .await
+}
+
+/// AC2 Switched, with two subscribed clients (one write, whatever the
+/// clients): `continue` and then a separate `\r`, once, and the notice. The
+/// addons are asked in registry order until one answers.
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn switched_types_continue_then_a_separate_enter_once() {
+    let quiet =
+        Arc::new(TestAddon::new("quiet").on_limit(LimitScript::Answer(LimitOutcome::NotMine)));
+    let proxy = Arc::new(TestAddon::new("proxy").on_limit(switched()));
+    let d = start_with(&[&quiet, &proxy], Duration::from_secs(20));
+    let mut a = d.client().await;
+    let (id, token) = session(&d, &mut a).await;
+    let mut b = d.client().await;
+    b.attach_until(id, BANNER).await;
+    hit(&d, &token).await;
+    let want = "usage limit: switched sub-a → sub-b, continuing (1/3)";
+    assert_eq!(notice(&mut a, id).await, want);
+    assert_eq!(notice(&mut b, id).await, want);
+    assert_eq!(input_until(&d, 2).await, ["continue", "\\r"]);
+    tokio::time::sleep(Duration::from_millis(1500)).await;
+    assert_eq!(input(&d), ["continue", "\\r"], "typed once");
+    let call = format!("usage_limit conv-1 {LIMIT} SECRET_ENV=<set>");
+    assert_eq!(asked(&quiet), [call.as_str()]);
+    assert_eq!(asked(&proxy), [call.as_str()]);
+}
+
+/// AC2 Repeated: three continues in a row, the give-up on the fourth hit; a
+/// turn that ends normally (`Stop`) starts the count over.
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn four_hits_without_a_stop_continue_three_times_then_give_up() {
+    let (d, addon) = start(switched());
+    let mut c = d.client().await;
+    let (id, token) = session(&d, &mut c).await;
+    for n in 1..=3 {
+        hit(&d, &token).await;
+        let want = format!("usage limit: switched sub-a → sub-b, continuing ({n}/3)");
+        assert_eq!(notice(&mut c, id).await, want);
+    }
+    hit(&d, &token).await;
+    let give_up = "usage limit: 3 automatic switches in a row; continue by hand";
+    assert_eq!(notice(&mut c, id).await, give_up);
+    assert_eq!(asked(&addon).len(), 3, "the fourth hit asks no addon");
+    assert_eq!(input_until(&d, 6).await.len(), 6);
+
+    send_hook(&d.config.socket, &token, "Stop", json!({})).await;
+    hit(&d, &token).await;
+    let want = "usage limit: switched sub-a → sub-b, continuing (1/3)";
+    assert_eq!(notice(&mut c, id).await, want);
+}
+
+/// AC2 Failed: a notice with the addon's reason, nothing typed.
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn a_failed_switch_is_a_notice_and_types_nothing() {
+    let why = "no other eligible credential can serve this session";
+    let (d, _) = start(LimitScript::Answer(LimitOutcome::Failed(why.into())));
+    let mut c = d.client().await;
+    let (id, token) = session(&d, &mut c).await;
+    hit(&d, &token).await;
+    let want = format!("usage limit: could not switch subscription: {why}");
+    assert_eq!(notice(&mut c, id).await, want);
+    tokio::time::sleep(Duration::from_millis(1500)).await;
+    assert!(input(&d).is_empty(), "{:?}", input(&d));
+}
+
+/// AC2 NotMine: nothing typed and no notice.
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn not_mine_types_nothing_and_says_nothing() {
+    let (d, addon) = start(LimitScript::Answer(LimitOutcome::NotMine));
+    let mut c = d.client().await;
+    let (id, token) = session(&d, &mut c).await;
+    hit(&d, &token).await;
+    asked_until(&addon, 1).await;
+    let any_notice = c.event(id, |e| {
+        matches!(e, SessionEvent::Notice { .. }).then_some(())
+    });
+    let quiet = tokio::time::timeout(Duration::from_millis(1500), any_notice).await;
+    assert!(quiet.is_err(), "a notice for a session no addon claims");
+    assert!(input(&d).is_empty(), "{:?}", input(&d));
+}
+
+/// AC2 Client input: typed during the failing turn or between the hit and
+/// the settle, nothing is typed and the "scheduled" notice says why; a
+/// submitted prompt (`UserPromptSubmit`) clears it. A focus report is not
+/// typing.
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn client_input_blocks_continue_until_a_prompt_is_submitted() {
+    let (d, addon) = start(switched());
+    let mut c = d.client().await;
+    let (id, token) = session(&d, &mut c).await;
+    let typed = |text: &str| Frame::Data {
+        session: id,
+        bytes: text.as_bytes().to_vec(),
+    };
+    let continued = |n: u8| format!("usage limit: switched sub-a → sub-b, continuing ({n}/3)");
+    // A front-end's focus report blocks nothing.
+    c.send(typed("\x1b[O")).await;
+    input_until(&d, 1).await;
+    hit(&d, &token).await;
+    assert_eq!(notice(&mut c, id).await, continued(1));
+    assert_eq!(input_until(&d, 3).await[1..], ["continue", "\\r"]);
+    // Typed during the turn, before the hit.
+    c.send(typed("x")).await;
+    assert_eq!(input_until(&d, 4).await[3], "x");
+    hit(&d, &token).await;
+    assert_eq!(notice(&mut c, id).await, NOT_CONTINUING);
+    // The prompt was submitted: the next hit continues.
+    send_hook(&d.config.socket, &token, "UserPromptSubmit", json!({})).await;
+    hit(&d, &token).await;
+    assert_eq!(notice(&mut c, id).await, continued(2));
+    assert_eq!(input_until(&d, 6).await[4..], ["continue", "\\r"]);
+    // Typed between the hit and the settle.
+    hit(&d, &token).await;
+    asked_until(&addon, 4).await;
+    c.send(typed("y")).await;
+    assert_eq!(notice(&mut c, id).await, NOT_CONTINUING);
+    assert_eq!(input_until(&d, 7).await[4..], ["continue", "\\r", "y"]);
+}
+
+/// AC2: a hit while an attempt is in flight starts no second attempt (and,
+/// being newer, keeps the first from typing).
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn a_second_hit_while_an_attempt_is_in_flight_starts_none() {
+    let slow = LimitScript::Delay(Duration::from_millis(800), Box::new(switched()));
+    let (d, addon) = start(slow);
+    let mut c = d.client().await;
+    let (id, token) = session(&d, &mut c).await;
+    hit(&d, &token).await;
+    hit(&d, &token).await;
+    assert_eq!(notice(&mut c, id).await, NOT_CONTINUING);
+    assert_eq!(asked(&addon).len(), 1);
+    assert!(input(&d).is_empty(), "{:?}", input(&d));
+}
+
+/// AC2 Timeout: past `limit_attempt_timeout`, `Failed("timed out")`; the
+/// abandoned answer never arrives, and the next hit can attempt again.
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn an_attempt_past_its_budget_fails_and_the_next_hit_tries_again() {
+    let never = LimitScript::Delay(Duration::from_secs(30), Box::new(switched()));
+    let addon = Arc::new(TestAddon::new("limit").on_limit(never));
+    let d = start_with(&[&addon], Duration::from_millis(300));
+    let mut c = d.client().await;
+    let (id, token) = session(&d, &mut c).await;
+    let started = std::time::Instant::now();
+    hit(&d, &token).await;
+    let want = "usage limit: could not switch subscription: timed out";
+    assert_eq!(notice(&mut c, id).await, want);
+    assert!(started.elapsed() < Duration::from_secs(5));
+    addon.set_limit(switched());
+    hit(&d, &token).await;
+    let want = "usage limit: switched sub-a → sub-b, continuing (1/3)";
+    assert_eq!(notice(&mut c, id).await, want);
+    assert_eq!(input_until(&d, 2).await, ["continue", "\\r"]);
+}
+
+/// AC2 Panic: an addon future that panics is `Failed("addon panicked")`,
+/// and the next hit can attempt again.
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn a_panicking_addon_fails_and_the_next_hit_tries_again() {
+    let (d, addon) = start(LimitScript::Panic);
+    let mut c = d.client().await;
+    let (id, token) = session(&d, &mut c).await;
+    hit(&d, &token).await;
+    let want = "usage limit: could not switch subscription: addon panicked";
+    assert_eq!(notice(&mut c, id).await, want);
+    addon.set_limit(switched());
+    hit(&d, &token).await;
+    let want = "usage limit: switched sub-a → sub-b, continuing (1/3)";
+    assert_eq!(notice(&mut c, id).await, want);
+}
````
Run: `cargo test --locked -p claudio-core --lib -- daemon::tests::limit`
Expected: ``error[E0560]: struct `daemon::Config` has no field named `limit_attempt_timeout` ``

Step 2: implement (the new `session/limit.rs` carries the guard's unit tests) (patch 7)
````diff
diff --git a/Makefile b/Makefile
index 83a7dbc..64f9f26 100644
--- a/Makefile
+++ b/Makefile
@@ -53,7 +53,8 @@ static:
 	@file $(DIST)/$(BIN) || true
 	@echo "static check:"; ldd $(DIST)/$(BIN) 2>&1 || true
 	@# R8: the debug-only test hooks never ship.
-	@if grep -q -e CLAUDIO_TEST_REMOTE_PEER -e CLAUDIO_TEST_SERVICE_HANG $(DIST)/$(BIN); then \
+	@if grep -q -e CLAUDIO_TEST_REMOTE_PEER -e CLAUDIO_TEST_SERVICE_HANG \
+	  -e CLAUDIO_TEST_LIMIT_SWITCH $(DIST)/$(BIN); then \
 	  echo "error: a debug-only test hook is in the release binary"; exit 1; fi
 
 ## Reproducible static build with zero host toolchain — uses a musl container.
diff --git a/crates/core/src/daemon/addon_ops.rs b/crates/core/src/daemon/addon_ops.rs
index 139d52e..ec36db3 100644
--- a/crates/core/src/daemon/addon_ops.rs
+++ b/crates/core/src/daemon/addon_ops.rs
@@ -12,7 +12,9 @@ use serde_json::Value;
 use tokio::sync::Semaphore;
 
 use super::{service, Daemon};
-use crate::addon::{Addons, BoxFuture, ClaudeFlags, DaemonCtx, ServiceState};
+use crate::addon::{
+    Addons, BoxFuture, ClaudeFlags, DaemonCtx, LimitOutcome, ServiceState, UsageLimit,
+};
 use crate::proto::{self, Msg};
 
 /// How long one addon op may run (`Config::addon_timeout`).
@@ -158,3 +160,31 @@ pub(crate) async fn claude_installed(daemon: &Arc<Daemon>) {
         }
     }
 }
+
+/// Ask every daemon-side addon, in registry order, about a usage-limit hit on a session
+/// whose spawn env is `env`; the first answer other than `NotMine` wins, its strings
+/// cleaned ([`crate::addon::limit_outcome`]). The env goes to the addons only, never a log.
+pub(crate) async fn usage_limit(
+    daemon: Arc<Daemon>,
+    conversation: String,
+    message: String,
+    env: Vec<(String, String)>,
+) -> LimitOutcome {
+    let lookup = move |name: &str| env.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());
+    for addon in daemon.addons.iter() {
+        let Some(ops) = addon.daemon() else { continue };
+        let hit = UsageLimit {
+            conversation: &conversation,
+            message: &message,
+            env: &lookup,
+        };
+        match ops
+            .on_usage_limit(hit, ctx(&daemon, addon.name()).await)
+            .await
+        {
+            LimitOutcome::NotMine => {}
+            outcome => return crate::addon::limit_outcome(outcome),
+        }
+    }
+    LimitOutcome::NotMine
+}
diff --git a/crates/core/src/daemon/command.rs b/crates/core/src/daemon/command.rs
index b8b3c30..08b8715 100644
--- a/crates/core/src/daemon/command.rs
+++ b/crates/core/src/daemon/command.rs
@@ -324,6 +324,7 @@ mod tests {
             build: None,
             path: None,
             addon_timeout: std::time::Duration::from_secs(150),
+            limit_attempt_timeout: std::time::Duration::from_secs(20),
             old_peer: false,
             config_file: None,
             service_timing: Default::default(),
@@ -406,6 +407,7 @@ mod tests {
             build: None,
             path: None,
             addon_timeout: std::time::Duration::from_secs(150),
+            limit_attempt_timeout: std::time::Duration::from_secs(20),
             old_peer: false,
             config_file: None,
             service_timing: Default::default(),
@@ -451,6 +453,7 @@ mod tests {
             build: None,
             path: None,
             addon_timeout: std::time::Duration::from_secs(150),
+            limit_attempt_timeout: std::time::Duration::from_secs(20),
             old_peer: false,
             config_file: None,
             service_timing: Default::default(),
@@ -508,6 +511,7 @@ mod tests {
             build: None,
             path: None,
             addon_timeout: std::time::Duration::from_secs(150),
+            limit_attempt_timeout: std::time::Duration::from_secs(20),
             old_peer: false,
             config_file: None,
             service_timing: Default::default(),
@@ -580,6 +584,7 @@ mod tests {
             build: None,
             path: None,
             addon_timeout: std::time::Duration::from_secs(150),
+            limit_attempt_timeout: std::time::Duration::from_secs(20),
             old_peer: false,
             config_file: None,
             service_timing: Default::default(),
diff --git a/crates/core/src/daemon/git_status.rs b/crates/core/src/daemon/git_status.rs
index ef52bfe..45a7cba 100644
--- a/crates/core/src/daemon/git_status.rs
+++ b/crates/core/src/daemon/git_status.rs
@@ -22,6 +22,16 @@ const OUTPUT_CAP: usize = 256 * 1024;
 /// Short commit id for a detached HEAD.
 const SHORT_OID: usize = 7;
 
+/// Read the current git branch from `cwd/.git/HEAD` (the fallback when git
+/// is not installed). `None` when the directory is not a git repo or HEAD is
+/// detached.
+pub(super) fn head_branch(cwd: &Path) -> Option<String> {
+    let head_path = cwd.join(".git/HEAD");
+    let content = std::fs::read_to_string(head_path).ok()?;
+    let line = content.trim();
+    line.strip_prefix("ref: refs/heads/").map(str::to_owned)
+}
+
 /// The status of the repository containing `cwd`; `None` when it is not a
 /// repository, git is missing, or the run exceeded the deadline.
 pub async fn read(cwd: &Path) -> Option<GitStatus> {
diff --git a/crates/core/src/daemon/mod.rs b/crates/core/src/daemon/mod.rs
index 704b5a4..2c6a610 100644
--- a/crates/core/src/daemon/mod.rs
+++ b/crates/core/src/daemon/mod.rs
@@ -97,6 +97,8 @@ pub struct Config {
     pub path: Option<OsString>,
     /// How long one addon op may run ([`addon_ops::TIMEOUT`]).
     pub addon_timeout: Duration,
+    /// The budget of one usage-limit attempt, every addon's answer included (20 s).
+    pub limit_attempt_timeout: Duration,
     /// Play the previous release (mixed-version tests only; docs/daemon-protocol.md,
     /// "Mixed versions"): no `addon:` features, `AddonOp` refused as unknown,
     /// every `env_remove` taken as absent and `optional` ignored.
@@ -133,6 +135,7 @@ impl Config {
                 .ok(),
             path,
             addon_timeout: addon_ops::TIMEOUT,
+            limit_attempt_timeout: Duration::from_secs(20),
             old_peer: admit::old_peer(std::env::var_os("CLAUDIO_TEST_OLD_DAEMON").as_deref()),
             config_file: Some(paths::config_file()),
             service_timing: ServiceTiming::default(),
@@ -870,6 +873,7 @@ mod claude_path_tests {
             build: None,
             path,
             addon_timeout: std::time::Duration::from_secs(150),
+            limit_attempt_timeout: std::time::Duration::from_secs(20),
             old_peer: false,
             config_file: None,
             service_timing: Default::default(),
diff --git a/crates/core/src/daemon/session.rs b/crates/core/src/daemon/session.rs
index c4dd043..94f8735 100644
--- a/crates/core/src/daemon/session.rs
+++ b/crates/core/src/daemon/session.rs
@@ -15,6 +15,8 @@
 //! `Attached` + snapshot instead of the bytes it missed. A slow client
 //! therefore never blocks the PTY.
 
+mod limit;
+
 use std::collections::{HashMap, VecDeque};
 use std::io::{self, Read, Write};
 use std::path::{Path, PathBuf};
@@ -112,6 +114,8 @@ pub enum Cmd {
     /// Like `Kill`, first handing over the subscribers so a respawn can
     /// attach them to the new process.
     Stop(oneshot::Sender<Vec<Subscription>>),
+    /// The usage-limit auto-switch's own steps ([`limit`]).
+    Limit(limit::Step),
 }
 
 /// An attached client's id and output queue.
@@ -224,7 +228,6 @@ pub fn spawn(
     // Detect whether this spawn uses --resume (for retry logic, §2.3).
     let spawned_with_resume =
         kind == SessionKind::Claude && spec.args.iter().any(|a| a == "--resume");
-    let spawn_time = std::time::Instant::now();
 
     // A shell has no hooks to report state: it is simply idle, forever.
     let mut tracker = Tracker::new();
@@ -252,7 +255,7 @@ pub fn spawn(
         title: None,
         subs: HashMap::new(),
         spawned_with_resume,
-        spawn_time,
+        spawn_time: std::time::Instant::now(),
         kind,
         meta: MetaRefresher::new(spec.id, Arc::clone(daemon), cwd, kind).with_tags(
             spec.extensions
@@ -260,6 +263,7 @@ pub fn spawn(
                 .map(|e| e.tags())
                 .unwrap_or_default(),
         ),
+        limit: limit::AutoSwitch::new(tx.downgrade()),
     };
     tokio::spawn(actor.run(cmds, io.output, io.exit, committed_rx));
     Ok((Handle { tx, token, pid }, committed_tx))
@@ -501,6 +505,8 @@ struct Actor {
     kind: SessionKind,
     /// Git status and transcript totals for the status bar (owns the cwd).
     meta: MetaRefresher,
+    /// The usage-limit auto-switch.
+    limit: limit::AutoSwitch,
 }
 
 /// Produces [`SessionEvent::Meta`] for one session: the git status of its
@@ -680,7 +686,7 @@ fn meta_event(
 ) -> SessionEvent {
     let branch = match &git {
         Some(status) => status.branch().map(str::to_owned),
-        None => read_git_branch(cwd),
+        None => super::git_status::head_branch(cwd),
     };
     let (model, context_tokens, output_tokens, turns) = match transcript {
         Some(t) => (
@@ -817,7 +823,7 @@ impl Actor {
 
     fn on_cmd(&mut self, cmd: Cmd) {
         match cmd {
-            Cmd::Input(bytes) => self.write(bytes),
+            Cmd::Input(bytes) => self.client_input(bytes),
             Cmd::Attach {
                 client,
                 queue,
@@ -829,7 +835,8 @@ impl Actor {
                 self.subs.remove(&client);
             }
             Cmd::Resize { client, rows, cols } => self.resize(client, rows, cols),
-            Cmd::Hook { event, payload } => self.on_hook(&event, &payload),
+            Cmd::Hook { event, payload } => self.hook(event, payload),
+            Cmd::Limit(step) => self.limit_step(step),
             Cmd::Status(reply) => {
                 let _ = reply.send(Status {
                     state: self.tracker.state,
@@ -1024,16 +1031,6 @@ impl Actor {
     }
 }
 
-/// Read the current git branch from `cwd/.git/HEAD` (the fallback when git
-/// is not installed). `None` when the directory is not a git repo or HEAD is
-/// detached.
-fn read_git_branch(cwd: &std::path::Path) -> Option<String> {
-    let head_path = cwd.join(".git/HEAD");
-    let content = std::fs::read_to_string(head_path).ok()?;
-    let line = content.trim();
-    line.strip_prefix("ref: refs/heads/").map(str::to_owned)
-}
-
 #[cfg(test)]
 mod tests {
     use super::*;
diff --git a/crates/core/src/daemon/session/limit.rs b/crates/core/src/daemon/session/limit.rs
new file mode 100644
index 0000000..2e21279
--- /dev/null
+++ b/crates/core/src/daemon/session/limit.rs
@@ -0,0 +1,588 @@
+//! The usage-limit auto-switch (docs/manager-design.md, "Usage limits"): when a turn ends
+//! on a usage limit, the daemon asks its addons to move the conversation to another
+//! subscription and, when one did, types `continue`, then Enter, into the session.
+//!
+//! [`Guard`] decides and is pure: it takes hooks, client input and its own [`Step`]s, and
+//! returns [`Effect`]s. The actor runs them: notices, PTY writes, timers (tasks that send it
+//! a `Cmd::Limit` later, never a sleep on the actor) and the attempt, a task of its own under
+//! `Config::limit_attempt_timeout`. Timers and attempts hold the actor's channel weakly and
+//! report to it only, so a respawned session's new actor never hears them. Names no addon.
+
+use std::sync::Arc;
+use std::time::Duration;
+
+use serde_json::Value;
+use tokio::sync::mpsc::WeakSender;
+
+use super::{Actor, Cmd};
+use crate::addon::LimitOutcome;
+use crate::claude::hooks;
+use crate::daemon::{addon_ops, Daemon};
+use crate::proto::{SessionEvent, SessionId, SessionState};
+
+/// Automatic continues in a row before the session is left to its user.
+const MAX_CONTINUES: u8 = 3;
+/// From an accepted switch to typing `continue`.
+const SETTLE: Duration = Duration::from_secs(1);
+/// From `continue` to Enter: Enter too early drops the just-typed text (the api driver's rule).
+const DWELL: Duration = Duration::from_millis(150);
+/// claude's message as an addon gets it, in bytes.
+const MESSAGE_BYTES: usize = 256;
+/// What the daemon types, in two writes.
+const CONTINUE: &[u8] = b"continue";
+const ENTER: &[u8] = b"\r";
+
+/// The guard's own messages to its actor (`Cmd::Limit`).
+#[derive(Debug, Clone, PartialEq, Eq)]
+pub(in crate::daemon) enum Step {
+    /// The attempt for hit `seq` ended with `outcome` (strings already cleaned).
+    Done { seq: u64, outcome: LimitOutcome },
+    /// The settle after hit `seq`'s switch is over: type `continue`.
+    Settled { seq: u64, from: String, to: String },
+    /// The dwell after `continue` is over: press Enter.
+    Enter { seq: u64, from: String, to: String },
+}
+
+/// What the actor does for the guard.
+#[derive(Debug, Clone, PartialEq, Eq)]
+pub(super) enum Effect {
+    /// Broadcast this session notice.
+    Notice(String),
+    /// Ask the addons about hit `seq`, off the actor.
+    Attempt {
+        seq: u64,
+        conversation: String,
+        message: String,
+    },
+    /// Write these bytes to the session's PTY.
+    Write(&'static [u8]),
+    /// Send the actor `Cmd::Limit(step)` after this long.
+    Later(Duration, Step),
+}
+
+/// The auto-switch state of one live session. A respawn starts a new one at zero.
+#[derive(Debug, Default)]
+pub(super) struct Guard {
+    /// Raised on every usage-limit hit: a step for an older hit types nothing.
+    seq: u64,
+    /// Automatic continues since the last turn that ended normally (`Stop`).
+    continues: u8,
+    /// An attempt is in flight.
+    pending: bool,
+    /// A client typed since its last submitted prompt (`UserPromptSubmit`).
+    typed: bool,
+}
+
+impl Guard {
+    /// A hook, after the tracker's share; `conversation` is claude's session id, if known.
+    pub(super) fn hook(
+        &mut self,
+        event: &str,
+        payload: &Value,
+        conversation: Option<&str>,
+    ) -> Vec<Effect> {
+        match event {
+            "Stop" => self.continues = 0,
+            "UserPromptSubmit" => self.typed = false,
+            "StopFailure" if hooks::is_usage_limit(payload) => {
+                return self.hit(payload, conversation)
+            }
+            _ => {}
+        }
+        Vec::new()
+    }
+
+    /// A client's bytes reached the session (never the daemon's own writes).
+    pub(super) fn typed(&mut self) {
+        self.typed = true;
+    }
+
+    /// One of the guard's own steps; `state` is the session's state now.
+    pub(super) fn step(&mut self, step: Step, state: SessionState) -> Vec<Effect> {
+        match step {
+            Step::Done { seq, outcome } => {
+                self.pending = false;
+                match outcome {
+                    LimitOutcome::NotMine => Vec::new(),
+                    LimitOutcome::Failed(why) => vec![Effect::Notice(format!(
+                        "usage limit: could not switch subscription: {why}"
+                    ))],
+                    LimitOutcome::Switched { from, to } => {
+                        vec![Effect::Later(SETTLE, Step::Settled { seq, from, to })]
+                    }
+                }
+            }
+            Step::Settled { seq, from, to } if self.may_type(seq, state) => vec![
+                Effect::Write(CONTINUE),
+                Effect::Later(DWELL, Step::Enter { seq, from, to }),
+            ],
+            Step::Enter { seq, from, to } if self.may_type(seq, state) => {
+                self.continues = self.continues.saturating_add(1);
+                let n = self.continues;
+                vec![
+                    Effect::Write(ENTER),
+                    Effect::Notice(format!(
+                        "usage limit: switched {from} → {to}, continuing ({n}/{MAX_CONTINUES})"
+                    )),
+                ]
+            }
+            Step::Settled { to, .. } | Step::Enter { to, .. } => vec![Effect::Notice(format!(
+                "usage limit: switch to {to} scheduled; not continuing automatically"
+            ))],
+        }
+    }
+
+    fn hit(&mut self, payload: &Value, conversation: Option<&str>) -> Vec<Effect> {
+        self.seq += 1;
+        if self.pending {
+            // The attempt in flight reports for this hit too.
+            return Vec::new();
+        }
+        if self.continues >= MAX_CONTINUES {
+            return vec![Effect::Notice(format!(
+                "usage limit: {MAX_CONTINUES} automatic switches in a row; continue by hand"
+            ))];
+        }
+        let Some(conversation) = conversation else {
+            return Vec::new();
+        };
+        self.pending = true;
+        let message = payload["last_assistant_message"].as_str().unwrap_or("");
+        vec![Effect::Attempt {
+            seq: self.seq,
+            conversation: conversation.to_owned(),
+            message: crate::addon::line(message.trim(), MESSAGE_BYTES),
+        }]
+    }
+
+    /// Whether the daemon may type for hit `seq`: no newer hit, the session still sits in
+    /// the error that hit left, and no client typed since its last submitted prompt. The
+    /// actor handles the step, so its process has not been reaped.
+    fn may_type(&self, seq: u64, state: SessionState) -> bool {
+        seq == self.seq && state == SessionState::Error && !self.typed
+    }
+}
+
+/// One actor's guard and what it acts with.
+pub(super) struct AutoSwitch {
+    guard: Guard,
+    /// The actor's own channel, weak: a timer or attempt never keeps it open.
+    tx: WeakSender<Cmd>,
+    /// Debug builds only ([`test_trigger`]): the next `Stop` is a usage limit that
+    /// switches without asking an addon.
+    test: bool,
+}
+
+impl AutoSwitch {
+    /// A guard at zero for the actor behind `tx`.
+    pub(super) fn new(tx: WeakSender<Cmd>) -> AutoSwitch {
+        AutoSwitch {
+            guard: Guard::default(),
+            tx,
+            test: test_trigger(),
+        }
+    }
+}
+
+impl Actor {
+    /// A hook: the tracker's share ([`Actor::on_hook`]), then the guard's.
+    pub(super) fn hook(&mut self, event: String, payload: Value) {
+        let test = self.limit.test && event == "Stop";
+        let (event, payload) = match test {
+            true => {
+                self.limit.test = false;
+                test_hit()
+            }
+            false => (event, payload),
+        };
+        self.on_hook(&event, &payload);
+        let conversation = self.tracker.claude_session_id.clone();
+        let mut effects = self
+            .limit
+            .guard
+            .hook(&event, &payload, conversation.as_deref());
+        if test {
+            effects = effects.into_iter().map(switched_here).collect();
+        }
+        self.limit_effects(effects);
+    }
+
+    /// A client's bytes: written; typing turns the next automatic `continue` off until
+    /// the client submits a prompt.
+    pub(super) fn client_input(&mut self, bytes: Vec<u8>) {
+        if is_typing(&bytes) {
+            self.limit.guard.typed();
+        }
+        self.write(bytes);
+    }
+
+    /// One of the guard's own steps.
+    pub(super) fn limit_step(&mut self, step: Step) {
+        let effects = self.limit.guard.step(step, self.tracker.state);
+        self.limit_effects(effects);
+    }
+
+    fn limit_effects(&mut self, effects: Vec<Effect>) {
+        for effect in effects {
+            match effect {
+                Effect::Notice(text) => self
+                    .daemon
+                    .broadcast(self.id, SessionEvent::Notice { text }),
+                Effect::Write(bytes) => self.write(bytes.to_vec()),
+                Effect::Later(after, step) => later(self.limit.tx.clone(), after, step),
+                Effect::Attempt {
+                    seq,
+                    conversation,
+                    message,
+                } => {
+                    let daemon = Arc::clone(&self.daemon);
+                    let tx = self.limit.tx.clone();
+                    attempt(daemon, self.id, seq, conversation, message, tx);
+                }
+            }
+        }
+    }
+}
+
+/// Whether a client's bytes may put text in claude's input box: anything but focus
+/// reports (`ESC [ I`, `ESC [ O`) and SGR mouse reports (`ESC [ < … M` or `m`), which a
+/// front-end sends on its own when its window loses focus or the user scrolls.
+fn is_typing(mut bytes: &[u8]) -> bool {
+    loop {
+        bytes = match bytes {
+            [] => return false,
+            [0x1b, b'[', b'I' | b'O', rest @ ..] => rest,
+            [0x1b, b'[', b'<', rest @ ..] => {
+                let params = rest
+                    .iter()
+                    .take_while(|b| b.is_ascii_digit() || **b == b';')
+                    .count();
+                match rest.get(params..) {
+                    Some([b'M' | b'm', rest @ ..]) => rest,
+                    _ => return true,
+                }
+            }
+            _ => return true,
+        }
+    }
+}
+
+/// Send the actor `step` after `after`, if it is still there.
+fn later(tx: WeakSender<Cmd>, after: Duration, step: Step) {
+    tokio::spawn(async move {
+        tokio::time::sleep(after).await;
+        if let Some(tx) = tx.upgrade() {
+            let _ = tx.send(Cmd::Limit(step)).await;
+        }
+    });
+}
+
+/// Ask the addons about hit `seq` of session `id` ([`addon_ops::usage_limit`]) on a task
+/// of its own, within `Config::limit_attempt_timeout`, and report one `Step::Done` to the
+/// actor if it is still there. A timeout aborts the addons' task; a panic in it is
+/// `Failed("addon panicked")`, so the guard is never left pending.
+fn attempt(
+    daemon: Arc<Daemon>,
+    id: SessionId,
+    seq: u64,
+    conversation: String,
+    message: String,
+    tx: WeakSender<Cmd>,
+) {
+    tokio::spawn(async move {
+        let env = daemon.spawn_env(id);
+        let budget = daemon.config.limit_attempt_timeout;
+        let ask = addon_ops::usage_limit(Arc::clone(&daemon), conversation, message, env);
+        let mut task = tokio::spawn(ask);
+        let outcome = match tokio::time::timeout(budget, &mut task).await {
+            Ok(Ok(outcome)) => outcome,
+            Ok(Err(_)) => LimitOutcome::Failed("addon panicked".to_owned()),
+            Err(_) => {
+                task.abort();
+                LimitOutcome::Failed("timed out".to_owned())
+            }
+        };
+        tracing::info!(%id, seq, ?outcome, "usage limit attempt");
+        if let Some(tx) = tx.upgrade() {
+            let _ = tx.send(Cmd::Limit(Step::Done { seq, outcome })).await;
+        }
+    });
+}
+
+impl Daemon {
+    /// Session `id`'s last spawn env, copied out of the registry (empty for none).
+    fn spawn_env(&self, id: SessionId) -> Vec<(String, String)> {
+        let reg = self.registry();
+        reg.session_env
+            .get(&id)
+            .map(|kept| kept.env.clone())
+            .unwrap_or_default()
+    }
+}
+
+/// `CLAUDIO_TEST_LIMIT_SWITCH=1` in a debug build's daemon env: each session's first
+/// `Stop` becomes a usage limit whose attempt switches without asking an addon, so a
+/// real-claude test can drive the type-dwell-Enter path. A release build has no such
+/// trigger, and `make static` checks that the name is not in the binary.
+#[cfg(debug_assertions)]
+fn test_trigger() -> bool {
+    std::env::var_os("CLAUDIO_TEST_LIMIT_SWITCH").is_some_and(|v| v == "1")
+}
+
+#[cfg(not(debug_assertions))]
+fn test_trigger() -> bool {
+    false
+}
+
+/// The usage-limit `StopFailure` the test trigger puts in place of a `Stop`.
+fn test_hit() -> (String, Value) {
+    let payload = serde_json::json!({
+        "hook_event_name": "StopFailure",
+        "error": "rate_limit",
+        "last_assistant_message": "You've hit your test limit",
+    });
+    ("StopFailure".to_owned(), payload)
+}
+
+/// The test trigger's attempt: switched here, without asking an addon.
+fn switched_here(effect: Effect) -> Effect {
+    match effect {
+        Effect::Attempt { seq, .. } => {
+            let outcome = LimitOutcome::Switched {
+                from: "test".to_owned(),
+                to: "test".to_owned(),
+            };
+            Effect::Later(Duration::ZERO, Step::Done { seq, outcome })
+        }
+        other => other,
+    }
+}
+
+#[cfg(test)]
+mod tests {
+    use super::*;
+    use serde_json::json;
+
+    const LIMIT: &str = "You've hit your 5-hour limit · resets 4pm (UTC)";
+
+    fn limit() -> Value {
+        json!({"error": "rate_limit", "last_assistant_message": format!(" {LIMIT}\n")})
+    }
+
+    fn switched(seq: u64) -> Step {
+        let outcome = LimitOutcome::Switched {
+            from: "a".into(),
+            to: "b".into(),
+        };
+        Step::Done { seq, outcome }
+    }
+
+    fn settled(seq: u64) -> Step {
+        Step::Settled {
+            seq,
+            from: "a".into(),
+            to: "b".into(),
+        }
+    }
+
+    fn enter(seq: u64) -> Step {
+        Step::Enter {
+            seq,
+            from: "a".into(),
+            to: "b".into(),
+        }
+    }
+
+    fn notice(text: &str) -> Vec<Effect> {
+        vec![Effect::Notice(text.to_owned())]
+    }
+
+    const NOT_CONTINUING: &str = "usage limit: switch to b scheduled; not continuing automatically";
+
+    /// One hit through to Enter; the continue notice it ends with.
+    fn continue_once(g: &mut Guard) -> Vec<Effect> {
+        let seq = match &g.hook("StopFailure", &limit(), Some("conv"))[..] {
+            [Effect::Attempt { seq, .. }] => *seq,
+            other => panic!("expected an attempt, got {other:?}"),
+        };
+        assert_eq!(
+            g.step(switched(seq), SessionState::Error),
+            [Effect::Later(SETTLE, settled(seq))]
+        );
+        assert_eq!(
+            g.step(settled(seq), SessionState::Error),
+            [Effect::Write(b"continue"), Effect::Later(DWELL, enter(seq))]
+        );
+        g.step(enter(seq), SessionState::Error)
+    }
+
+    #[test]
+    fn a_hit_asks_once_with_the_conversation_and_the_trimmed_message() {
+        let mut g = Guard::default();
+        let attempt = Effect::Attempt {
+            seq: 1,
+            conversation: "conv".into(),
+            message: LIMIT.into(),
+        };
+        assert_eq!(g.hook("StopFailure", &limit(), Some("conv")), [attempt]);
+        // A second hit while the attempt is in flight asks nothing and says nothing.
+        assert!(g.hook("StopFailure", &limit(), Some("conv")).is_empty());
+        // Other failures, and a hit before claude named its conversation, do nothing.
+        let mut g = Guard::default();
+        let overloaded = json!({"error": "overloaded", "last_assistant_message": LIMIT});
+        assert!(g.hook("StopFailure", &overloaded, Some("conv")).is_empty());
+        assert!(g.hook("StopFailure", &limit(), None).is_empty());
+        assert!(!g.pending);
+    }
+
+    #[test]
+    fn switched_types_continue_then_enter_and_counts() {
+        let mut g = Guard::default();
+        assert_eq!(
+            continue_once(&mut g),
+            [
+                Effect::Write(b"\r"),
+                Effect::Notice("usage limit: switched a → b, continuing (1/3)".into())
+            ]
+        );
+    }
+
+    #[test]
+    fn three_in_a_row_then_the_give_up_and_a_stop_resets() {
+        let mut g = Guard::default();
+        for n in 1..=3 {
+            let want = format!("usage limit: switched a → b, continuing ({n}/3)");
+            assert_eq!(continue_once(&mut g)[1], Effect::Notice(want));
+        }
+        let give_up = notice("usage limit: 3 automatic switches in a row; continue by hand");
+        assert_eq!(g.hook("StopFailure", &limit(), Some("conv")), give_up);
+        assert_eq!(g.hook("StopFailure", &limit(), Some("conv")), give_up);
+        assert!(g.hook("Stop", &json!({}), Some("conv")).is_empty());
+        let want = "usage limit: switched a → b, continuing (1/3)";
+        assert_eq!(continue_once(&mut g)[1], Effect::Notice(want.into()));
+    }
+
+    #[test]
+    fn failed_and_not_mine_type_nothing() {
+        let mut g = Guard::default();
+        g.hook("StopFailure", &limit(), Some("conv"));
+        let failed = Step::Done {
+            seq: 1,
+            outcome: LimitOutcome::Failed("no other eligible credential".into()),
+        };
+        assert_eq!(
+            g.step(failed, SessionState::Error),
+            notice("usage limit: could not switch subscription: no other eligible credential")
+        );
+        // Not pending any more: the next hit asks again.
+        assert!(matches!(
+            g.hook("StopFailure", &limit(), Some("conv"))[..],
+            [Effect::Attempt { seq: 2, .. }]
+        ));
+        let not_mine = Step::Done {
+            seq: 2,
+            outcome: LimitOutcome::NotMine,
+        };
+        assert!(g.step(not_mine, SessionState::Error).is_empty());
+        assert!(!g.pending);
+    }
+
+    #[test]
+    fn client_input_blocks_continue_until_a_prompt_is_submitted() {
+        // Typed during the failing turn: the hit does not clear it.
+        let mut g = Guard::default();
+        g.typed();
+        g.hook("StopFailure", &limit(), Some("conv"));
+        g.step(switched(1), SessionState::Error);
+        assert_eq!(
+            g.step(settled(1), SessionState::Error),
+            notice(NOT_CONTINUING)
+        );
+        // Typed between the hit and the settle.
+        let mut g = Guard::default();
+        g.hook("StopFailure", &limit(), Some("conv"));
+        g.step(switched(1), SessionState::Error);
+        g.typed();
+        assert_eq!(
+            g.step(settled(1), SessionState::Error),
+            notice(NOT_CONTINUING)
+        );
+        // Typed between `continue` and Enter: no Enter.
+        let mut g = Guard::default();
+        g.hook("StopFailure", &limit(), Some("conv"));
+        g.step(switched(1), SessionState::Error);
+        g.step(settled(1), SessionState::Error);
+        g.typed();
+        assert_eq!(
+            g.step(enter(1), SessionState::Error),
+            notice(NOT_CONTINUING)
+        );
+        assert_eq!(g.continues, 0);
+        // A submitted prompt clears it.
+        let mut g = Guard::default();
+        g.typed();
+        g.hook("UserPromptSubmit", &json!({}), Some("conv"));
+        assert_eq!(continue_once(&mut g)[0], Effect::Write(b"\r"));
+    }
+
+    #[test]
+    fn a_newer_hit_or_a_state_change_types_nothing() {
+        let mut g = Guard::default();
+        g.hook("StopFailure", &limit(), Some("conv"));
+        g.hook("StopFailure", &limit(), Some("conv"));
+        g.step(switched(1), SessionState::Error);
+        assert_eq!(
+            g.step(settled(1), SessionState::Error),
+            notice(NOT_CONTINUING)
+        );
+        let mut g = Guard::default();
+        g.hook("StopFailure", &limit(), Some("conv"));
+        g.step(switched(1), SessionState::Error);
+        let working = SessionState::Working;
+        assert_eq!(g.step(settled(1), working), notice(NOT_CONTINUING));
+    }
+
+    #[test]
+    fn focus_and_mouse_reports_are_not_typing() {
+        for quiet in [
+            &b""[..],
+            b"\x1b[I",
+            b"\x1b[O\x1b[I",
+            b"\x1b[<64;10;5M",
+            b"\x1b[<0;1;1m\x1b[O",
+        ] {
+            assert!(!is_typing(quiet), "{quiet:?}");
+        }
+        for typing in [
+            &b"x"[..],
+            b"\r",
+            b"\x1b[A",
+            b"\x1b[I x",
+            b"\x1b[<64;10",
+            b"\x1b[<64;10;5X",
+            b"\x1b[200~hi\x1b[201~",
+        ] {
+            assert!(is_typing(typing), "{typing:?}");
+        }
+    }
+
+    #[test]
+    fn the_test_trigger_is_a_usage_limit_answered_here() {
+        let (event, payload) = test_hit();
+        assert_eq!(event, "StopFailure");
+        assert!(hooks::is_usage_limit(&payload));
+        let attempt = Effect::Attempt {
+            seq: 4,
+            conversation: "c".into(),
+            message: "m".into(),
+        };
+        let outcome = LimitOutcome::Switched {
+            from: "test".into(),
+            to: "test".into(),
+        };
+        let done = Step::Done { seq: 4, outcome };
+        assert_eq!(switched_here(attempt), Effect::Later(Duration::ZERO, done));
+        let other = Effect::Notice("n".into());
+        assert_eq!(switched_here(other.clone()), other);
+    }
+}
diff --git a/crates/core/src/daemon/test_support.rs b/crates/core/src/daemon/test_support.rs
index 37a51db..c6dd57a 100644
--- a/crates/core/src/daemon/test_support.rs
+++ b/crates/core/src/daemon/test_support.rs
@@ -159,6 +159,7 @@ impl TestDaemon {
             build: None,
             path: None,
             addon_timeout: std::time::Duration::from_secs(150),
+            limit_attempt_timeout: std::time::Duration::from_secs(20),
             old_peer: false,
             config_file: None,
             service_timing: Default::default(),
diff --git a/crates/core/src/daemon/tests.rs b/crates/core/src/daemon/tests.rs
index 396e632..e2e6f87 100644
--- a/crates/core/src/daemon/tests.rs
+++ b/crates/core/src/daemon/tests.rs
@@ -569,6 +569,7 @@ async fn resume_retry_spawns_fresh_on_quick_exit() {
         build: None,
         path: None,
         addon_timeout: std::time::Duration::from_secs(150),
+        limit_attempt_timeout: std::time::Duration::from_secs(20),
         old_peer: false,
         config_file: None,
         service_timing: Default::default(),
@@ -693,6 +694,7 @@ async fn immediate_exit_leaves_session_dormant() {
         build: None,
         path: None,
         addon_timeout: std::time::Duration::from_secs(150),
+        limit_attempt_timeout: std::time::Duration::from_secs(20),
         old_peer: false,
         config_file: None,
         service_timing: Default::default(),
````
Run: `cargo test --locked -p claudio-core --lib -- session::limit daemon::tests::limit`
Expected: `test result: ok. 16 passed; 0 failed` (8 guard unit tests, 8 daemon tests; about 5 s)
Run: `python3 scripts/check_rules.py`
Expected: `rules: 0 problem(s)` (`session.rs` and `session.rs::spawn` within their baselines)
Run: `make check`
Expected: every crate's `test result: ok` (claudio-core: 562 passed), `rules: 0 problem(s)`

Step 3: commit
`git add crates/core/src/daemon/session/limit.rs crates/core/src/daemon/tests/limit.rs`
`git commit -m "feat(daemon): switch and continue a session that hits a usage limit" -- crates/core/src/daemon/session/limit.rs crates/core/src/daemon/session.rs crates/core/src/daemon/git_status.rs crates/core/src/daemon/mod.rs crates/core/src/daemon/addon_ops.rs crates/core/src/daemon/command.rs crates/core/src/daemon/test_support.rs crates/core/src/daemon/tests.rs crates/core/src/daemon/tests/limit.rs Makefile`

Interfaces this task provides to later tasks:
- `daemon::Config::limit_attempt_timeout: Duration` (default 20 s)
- the notices `usage limit: switched <from> → <to>, continuing (<n>/3)`, `usage limit: switch to <to> scheduled; not continuing automatically`, `usage limit: could not switch subscription: <why>`, `usage limit: 3 automatic switches in a row; continue by hand`
- `CLAUDIO_TEST_LIMIT_SWITCH=1` (debug builds): the first `Stop` of each session is a usage limit that switches `test → test`

### Task 5: Vocdoni's `on_usage_limit`
Model: sonnet   Repo: /home/p4u/claudio   Files: `addons/vocdoni/src/proxy/limit.rs` (new),
`addons/vocdoni/src/proxy/mod.rs`, `addons/vocdoni/src/proxy/api.rs`,
`addons/vocdoni/src/daemon/ops.rs`
Goal: §3 (AC3): `[proxy] auto_switch`, the env's base URL and token, `switch_session` with 15 s,
and the 202/409/404 mapping. The token is never logged.

Step 1: write the failing tests against the existing `switch_router` stub, with a placeholder
that answers `NotMine` (step 2 replaces it) (patch 8)
````diff
diff --git a/addons/vocdoni/src/proxy/api.rs b/addons/vocdoni/src/proxy/api.rs
index ad01c0b..4c5bb20 100644
--- a/addons/vocdoni/src/proxy/api.rs
+++ b/addons/vocdoni/src/proxy/api.rs
@@ -529,7 +529,7 @@ async fn has_capability(
 // ── Tests ─────────────────────────────────────────────────────────────────────
 
 #[cfg(test)]
-mod tests {
+pub(crate) mod tests {
     use super::*;
     use axum::routing::get;
     use axum::{Json, Router};
@@ -537,7 +537,7 @@ mod tests {
     use tokio::net::TcpListener;
 
     /// Spin up a local axum server serving the given router. Returns the base URL.
-    async fn serve(router: Router) -> String {
+    pub(crate) async fn serve(router: Router) -> String {
         let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
         let addr: SocketAddr = listener.local_addr().unwrap();
         tokio::spawn(async move {
@@ -787,7 +787,7 @@ mod tests {
         }
     }
 
-    fn switch_router(code: u16, body: serde_json::Value) -> Router {
+    pub(crate) fn switch_router(code: u16, body: serde_json::Value) -> Router {
         use axum::routing::post;
         Router::new().route(
             "/v1/claudio/session/switch",
@@ -833,7 +833,11 @@ mod tests {
     }
 
     /// `switch_router` plus the `/v1/claudio` discovery, listing `capabilities`.
-    fn switch_router_with(code: u16, body: serde_json::Value, capabilities: &[&str]) -> Router {
+    pub(crate) fn switch_router_with(
+        code: u16,
+        body: serde_json::Value,
+        capabilities: &[&str],
+    ) -> Router {
         let discovery = serde_json::json!({"version": "1", "capabilities": capabilities});
         switch_router(code, body).route("/v1/claudio", get(move || async move { Json(discovery) }))
     }
diff --git a/addons/vocdoni/src/proxy/limit.rs b/addons/vocdoni/src/proxy/limit.rs
new file mode 100644
index 0000000..e40ab7c
--- /dev/null
+++ b/addons/vocdoni/src/proxy/limit.rs
@@ -0,0 +1,176 @@
+//! The Vocdoni addon's `DaemonOps::on_usage_limit` (docs/addons.md, "The Vocdoni addon"):
+//! a proxy session whose turn ended on a usage limit is moved to the proxy's next
+//! subscription. The token comes from the session's spawn env and goes only to the
+//! session's own proxy URL; it is never logged and never in an outcome (C1).
+
+use std::sync::Arc;
+
+use claudio_core::addon::{DaemonCtx, LimitOutcome, UsageLimit};
+
+/// Not yet: every hit is `NotMine` (Task 5 step 2 replaces this).
+pub async fn on_usage_limit(hit: UsageLimit<'_>, ctx: Arc<dyn DaemonCtx>) -> LimitOutcome {
+    let _ = (hit, ctx);
+    LimitOutcome::NotMine
+}
+
+#[cfg(test)]
+mod tests {
+    use std::ffi::OsString;
+    use std::path::PathBuf;
+    use std::sync::atomic::{AtomicUsize, Ordering};
+
+    use axum::Router;
+    use claudio_core::addon::{BoxFuture, ClaudeFlags, ServiceState};
+    use serde_json::json;
+
+    use super::*;
+    use crate::proxy::api::tests::{serve, switch_router, switch_router_with};
+
+    /// A daemon context whose `config.toml` is this table.
+    struct Ctx(toml::Table);
+
+    impl DaemonCtx for Ctx {
+        fn claude_bin(&self) -> PathBuf {
+            PathBuf::from("claude")
+        }
+        fn path(&self) -> Option<OsString> {
+            None
+        }
+        fn claude_flags(&self) -> BoxFuture<'static, ClaudeFlags> {
+            Box::pin(async { ClaudeFlags::default() })
+        }
+        fn cache_dir(&self) -> PathBuf {
+            std::env::temp_dir()
+        }
+        fn updating(&self) -> Arc<tokio::sync::Mutex<()>> {
+            Arc::default()
+        }
+        fn service_reload(&self, _force: bool) -> BoxFuture<'static, ServiceState> {
+            Box::pin(async { ServiceState::Off })
+        }
+        fn service_state(&self) -> ServiceState {
+            ServiceState::Off
+        }
+        fn config(&self, section: &str) -> toml::Table {
+            match self.0.get(section) {
+                Some(toml::Value::Table(t)) => t.clone(),
+                _ => toml::Table::new(),
+            }
+        }
+    }
+
+    /// `router`, counting the requests it gets.
+    fn counted(router: Router) -> (Router, Arc<AtomicUsize>) {
+        let hits = Arc::new(AtomicUsize::new(0));
+        let seen = Arc::clone(&hits);
+        let count = move |req: axum::extract::Request, next: axum::middleware::Next| {
+            seen.fetch_add(1, Ordering::SeqCst);
+            next.run(req)
+        };
+        (router.layer(axum::middleware::from_fn(count)), hits)
+    }
+
+    /// `on_usage_limit` for conversation `0a1b` of a session with `env`, on a host
+    /// whose `config.toml` is `config`.
+    async fn ask(env: &[(&str, &str)], config: &str) -> LimitOutcome {
+        let env: Vec<(String, String)> = env
+            .iter()
+            .map(|(k, v)| (k.to_string(), v.to_string()))
+            .collect();
+        let lookup = move |name: &str| env.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());
+        let hit = UsageLimit {
+            conversation: "0a1b",
+            message: "You've hit your 5-hour limit · resets 4pm (UTC)",
+            env: &lookup,
+        };
+        on_usage_limit(hit, Arc::new(Ctx(config.parse().unwrap()))).await
+    }
+
+    fn proxied(base: &str) -> [(&str, &str); 2] {
+        [
+            ("ANTHROPIC_BASE_URL", base),
+            ("ANTHROPIC_AUTH_TOKEN", "tok"),
+        ]
+    }
+
+    /// AC3: 202 is `Switched` with the proxy's names; the request carries the
+    /// session's conversation and token (`switch_router` checks both).
+    #[tokio::test]
+    async fn accepted_is_switched_with_the_proxys_names() {
+        let body = json!({"session_id": "0a1b", "state": "pending",
+            "from": {"id": "c1", "label": "work-max"}, "to": {"id": "c2", "label": "personal"}});
+        let base = serve(switch_router(202, body)).await;
+        let want = LimitOutcome::Switched {
+            from: "work-max".into(),
+            to: "personal".into(),
+        };
+        assert_eq!(ask(&proxied(&base), "").await, want);
+        // No target chosen yet, and no names at all.
+        let body = json!({"session_id": "0a1b", "from": {}, "to": null, "state": "pending"});
+        let base = serve(switch_router(202, body)).await;
+        let want = LimitOutcome::Switched {
+            from: "current subscription".into(),
+            to: "next available".into(),
+        };
+        assert_eq!(ask(&proxied(&base), "").await, want);
+    }
+
+    /// AC3: 409 fails with the proxy's own reason; 404 says the proxy does not
+    /// know the session.
+    #[tokio::test]
+    async fn refused_fails_with_the_proxys_reason() {
+        let body = json!({"error": {"type": "no_alternative",
+            "message": "claudio: no other eligible credential can serve this session"}});
+        let base = serve(switch_router(409, body)).await;
+        let want = "no other eligible credential can serve this session";
+        assert_eq!(
+            ask(&proxied(&base), "").await,
+            LimitOutcome::Failed(want.into())
+        );
+        let unknown = json!({"error": {"type": "not_found"}});
+        let base = serve(switch_router_with(404, unknown, &["session_switch"])).await;
+        let want = "the proxy does not know this session";
+        assert_eq!(
+            ask(&proxied(&base), "").await,
+            LimitOutcome::Failed(want.into())
+        );
+    }
+
+    /// AC3: `[proxy] auto_switch = false` is `NotMine` and sends nothing.
+    #[tokio::test]
+    async fn switched_off_is_not_mine_and_sends_nothing() {
+        let body = json!({"session_id": "0a1b", "from": {"id": "c1"}, "state": "pending"});
+        let (router, hits) = counted(switch_router(202, body));
+        let base = serve(router).await;
+        let off = "[proxy]\nauto_switch = false\n";
+        assert_eq!(ask(&proxied(&base), off).await, LimitOutcome::NotMine);
+        assert_eq!(hits.load(Ordering::SeqCst), 0);
+        let on = "[proxy]\nauto_switch = true\n";
+        assert!(matches!(
+            ask(&proxied(&base), on).await,
+            LimitOutcome::Switched { .. }
+        ));
+        assert_eq!(hits.load(Ordering::SeqCst), 1);
+    }
+
+    /// AC3: a session without the proxy's base URL or token is `NotMine` and
+    /// sends nothing.
+    #[tokio::test]
+    async fn no_proxy_env_is_not_mine_and_sends_nothing() {
+        let body = json!({"session_id": "0a1b", "from": {"id": "c1"}, "state": "pending"});
+        let (router, hits) = counted(switch_router(202, body));
+        let base = serve(router).await;
+        for env in [
+            vec![("ANTHROPIC_AUTH_TOKEN", "tok")],
+            vec![("ANTHROPIC_BASE_URL", base.as_str())],
+            vec![
+                ("ANTHROPIC_BASE_URL", base.as_str()),
+                ("ANTHROPIC_AUTH_TOKEN", ""),
+            ],
+            vec![],
+        ] {
+            assert_eq!(ask(&env, "").await, LimitOutcome::NotMine, "{env:?}");
+        }
+        assert_eq!(hits.load(Ordering::SeqCst), 0);
+    }
+}
diff --git a/addons/vocdoni/src/proxy/mod.rs b/addons/vocdoni/src/proxy/mod.rs
index 32c9295..935b2d1 100644
--- a/addons/vocdoni/src/proxy/mod.rs
+++ b/addons/vocdoni/src/proxy/mod.rs
@@ -5,6 +5,7 @@
 
 pub mod api;
 pub mod env;
+pub mod limit;
 pub mod login;
 pub mod plugins;
 pub mod profile;
````
Run: `cargo test --locked -p claudio-vocdoni --lib -- proxy::limit`
Expected: `test result: FAILED. 1 passed; 3 failed` (the two "not mine" cases already hold)

Step 2: implement (patch 9)
````diff
diff --git a/addons/vocdoni/src/daemon/ops.rs b/addons/vocdoni/src/daemon/ops.rs
index 0cb53cf..573e101 100644
--- a/addons/vocdoni/src/daemon/ops.rs
+++ b/addons/vocdoni/src/daemon/ops.rs
@@ -9,7 +9,7 @@ use tokio::process::Child;
 
 use super::{handle, remove_stale_zips, ApplyRequest, Manifests, OP_APPLY};
 use crate::plugins::manifest::name_ok;
-use claudio_core::addon::{BoxFuture, ClaudeFlags, DaemonCtx, DaemonOps};
+use claudio_core::addon::{BoxFuture, ClaudeFlags, DaemonCtx, DaemonOps, LimitOutcome, UsageLimit};
 
 /// The plugin flags a plugin's sessions use.
 const AGENTS: &str = "--agents";
@@ -106,6 +106,16 @@ impl DaemonOps for VocdoniDaemon {
     fn on_claude_installed(&self, _ctx: &Arc<dyn DaemonCtx>) {
         self.manifests.forget_failures();
     }
+
+    /// A proxy session hit a usage limit: move its conversation to the
+    /// proxy's next subscription ([`crate::proxy::limit`]).
+    fn on_usage_limit<'a>(
+        &'a self,
+        hit: UsageLimit<'a>,
+        ctx: Arc<dyn DaemonCtx>,
+    ) -> BoxFuture<'a, LimitOutcome> {
+        Box::pin(crate::proxy::limit::on_usage_limit(hit, ctx))
+    }
 }
 
 /// The refusal of a request that is not one: names the plugin only when
diff --git a/addons/vocdoni/src/proxy/limit.rs b/addons/vocdoni/src/proxy/limit.rs
index e40ab7c..714c888 100644
--- a/addons/vocdoni/src/proxy/limit.rs
+++ b/addons/vocdoni/src/proxy/limit.rs
@@ -4,13 +4,44 @@
 //! session's own proxy URL; it is never logged and never in an outcome (C1).
 
 use std::sync::Arc;
+use std::time::Duration;
 
 use claudio_core::addon::{DaemonCtx, LimitOutcome, UsageLimit};
 
-/// Not yet: every hit is `NotMine` (Task 5 step 2 replaces this).
+use super::api::{self, SwitchError};
+
+/// The most one switch may take, the capability probe after a 404 included.
+const SWITCH_TIMEOUT: Duration = Duration::from_secs(15);
+
+/// Unless `[proxy] auto_switch = false` on this host: `POST /v1/claudio/session/switch`
+/// for the conversation with the session's own `ANTHROPIC_BASE_URL` and
+/// `ANTHROPIC_AUTH_TOKEN`. A session without both is not a proxy session: `NotMine`.
 pub async fn on_usage_limit(hit: UsageLimit<'_>, ctx: Arc<dyn DaemonCtx>) -> LimitOutcome {
-    let _ = (hit, ctx);
-    LimitOutcome::NotMine
+    let proxy = ctx.config("proxy");
+    if proxy.get("auto_switch").and_then(toml::Value::as_bool) == Some(false) {
+        return LimitOutcome::NotMine;
+    }
+    let var = |name| (hit.env)(name).filter(|v| !v.is_empty());
+    let (Some(url), Some(token)) = (var("ANTHROPIC_BASE_URL"), var("ANTHROPIC_AUTH_TOKEN")) else {
+        return LimitOutcome::NotMine;
+    };
+    let switch = api::switch_session(&url, &token, hit.conversation);
+    match tokio::time::timeout(SWITCH_TIMEOUT, switch).await {
+        Ok(Ok(o)) => LimitOutcome::Switched {
+            from: o.from.name().unwrap_or("current subscription").to_owned(),
+            to: o
+                .to
+                .as_ref()
+                .and_then(|t| t.name())
+                .unwrap_or("next available")
+                .to_owned(),
+        },
+        Ok(Err(SwitchError::UnknownSession)) => {
+            LimitOutcome::Failed("the proxy does not know this session".to_owned())
+        }
+        Ok(Err(e)) => LimitOutcome::Failed(e.to_string()),
+        Err(_) => LimitOutcome::Failed("the proxy did not answer within 15 s".to_owned()),
+    }
 }
 
 #[cfg(test)]
````
Run: `cargo test --locked -p claudio-vocdoni --lib -- proxy::limit`
Expected: `test result: ok. 4 passed; 0 failed`
Run: `make check`
Expected: every crate's `test result: ok`, `rules: 0 problem(s)`

Step 3: commit
`git add addons/vocdoni/src/proxy/limit.rs`
`git commit -m "feat(vocdoni): switch a proxy session's subscription on a usage limit" -- addons/vocdoni/src/proxy/limit.rs addons/vocdoni/src/proxy/mod.rs addons/vocdoni/src/proxy/api.rs addons/vocdoni/src/daemon/ops.rs`

Interfaces this task provides to later tasks: `claudio_vocdoni::proxy::limit::on_usage_limit(UsageLimit<'_>, Arc<dyn DaemonCtx>) -> LimitOutcome`; `VocdoniDaemon` answers `on_usage_limit` through it.

### Task 6: End-to-end tests: the vocdoni_manager suite (AC4) and the opt-in real claude (AC6)
Model: sonnet   Repo: /home/p4u/claudio   Files: `addons/vocdoni/tests/manager.rs`,
`crates/tui/tests/manager.rs`
Goal: AC4 runs the real binary with a fake proxy, and a fake claude that fires the
`StopFailure` relay from its merged `--settings` and echoes input. It checks the switch request
(conversation, token), the typed `continue`, the TUI notice, and that the token is in no notice
or daemon log. AC6 is the opt-in real-claude check that the daemon's typed `continue` starts a
turn, through the debug-only trigger. It reports a skipped run.

These tests cover behaviour that Tasks 4 and 5 already built, so "failing first" is shown by
turning the feature off.

Step 1: add AC4, then prove it detects a missing switch (patch 10)
````diff
diff --git a/addons/vocdoni/tests/manager.rs b/addons/vocdoni/tests/manager.rs
index a21ad1a..0aa8d59 100644
--- a/addons/vocdoni/tests/manager.rs
+++ b/addons/vocdoni/tests/manager.rs
@@ -1431,3 +1431,124 @@ fn test_new_client_old_daemon_gets_proxy_env_without_plugins() {
     );
     tui.quit(WAIT);
 }
+
+// ── Usage limits ─────────────────────────────────────────────────────────────
+
+/// The conversation the usage-limit fake claude reports.
+const LIMIT_SID: &str = "5e55-limit-test";
+
+/// A fake claude-proxy on loopback that accepts every `POST
+/// /v1/claudio/session/switch` (202, from `sub-a` to `sub-b`) and sends each
+/// one's `authorization` header and body down the channel; every other path
+/// is a 404. Returns its `host:port`.
+fn fake_switch_proxy() -> (String, std::sync::mpsc::Receiver<(String, String)>) {
+    use std::io::{Read, Write};
+    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind fake proxy");
+    let addr = listener.local_addr().unwrap().to_string();
+    let (tx, rx) = std::sync::mpsc::channel();
+    thread::spawn(move || {
+        for stream in listener.incoming() {
+            let Ok(mut stream) = stream else { continue };
+            let mut head = Vec::new();
+            let mut byte = [0u8; 1];
+            while !head.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap_or(0) == 1 {
+                head.push(byte[0]);
+            }
+            let head = String::from_utf8_lossy(&head).into_owned();
+            let header = |name: &str| {
+                head.lines().find_map(|l| {
+                    let (k, v) = l.split_once(':')?;
+                    k.eq_ignore_ascii_case(name).then(|| v.trim().to_owned())
+                })
+            };
+            let len = header("content-length").and_then(|v| v.parse().ok());
+            let mut body = vec![0u8; len.unwrap_or(0).min(4096)];
+            let _ = stream.read_exact(&mut body);
+            let (status, reply) = if head.starts_with("POST /v1/claudio/session/switch ") {
+                let body = String::from_utf8_lossy(&body).into_owned();
+                let _ = tx.send((header("authorization").unwrap_or_default(), body));
+                let reply = serde_json::json!({"session_id": LIMIT_SID, "state": "pending",
+                    "from": {"id": "c1", "label": "sub-a"}, "to": {"id": "c2", "label": "sub-b"}});
+                ("202 Accepted", reply.to_string())
+            } else {
+                ("404 Not Found", "{}".to_owned())
+            };
+            let _ = write!(
+                stream,
+                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n\
+                 Content-Length: {}\r\nConnection: close\r\n\r\n{reply}",
+                reply.len()
+            );
+        }
+    });
+    (addr, rx)
+}
+
+/// A fake claude whose turn ends on a usage limit: a second after its banner
+/// it reports conversation [`LIMIT_SID`] (`SessionStart`) and then the limit
+/// (`StopFailure`), through the hook commands of its merged `--settings`, as
+/// claude does; then it echoes each line it reads as `GOT <line>`.
+fn write_limit_claude(h: &ManagerHarness) {
+    use std::os::unix::fs::PermissionsExt;
+    let script = format!(
+        r#"#!/bin/sh
+[ "$1" = "--version" ] && echo 'claude 0.0.0-fake' && exit 0
+[ "$1" = "--help" ] && exit 0
+echo "FAKE_CLAUDE_BANNER pid=$$"
+START=$(printf '%s' "$2" | sed -n 's/.*"command":"\([^"]*__hook SessionStart [^"]*\)".*/\1/p')
+FAIL=$(printf '%s' "$2" | sed -n 's/.*"command":"\([^"]*__hook StopFailure [^"]*\)".*/\1/p')
+sleep 1
+printf '{{"session_id":"{LIMIT_SID}","source":"startup"}}' | eval "$START"
+eval "$FAIL" <<'JSON'
+{{"error":"rate_limit","last_assistant_message":"You've hit your 5-hour limit · resets 4pm (UTC)"}}
+JSON
+while IFS= read -r line; do printf 'GOT %s\n' "$line"; done
+"#
+    );
+    fs::write(&h.fake_claude, script).unwrap();
+    fs::set_permissions(&h.fake_claude, fs::Permissions::from_mode(0o755)).unwrap();
+}
+
+/// AC4: a proxy session whose turn ends on a usage limit is moved by its
+/// host's daemon, with no one asking: the fake proxy gets the switch for the
+/// session's conversation with the profile's token, the session gets
+/// `continue` and Enter, and the TUI shows the notice. The token is in no
+/// notice and no daemon log line.
+#[test]
+fn test_a_usage_limit_switches_the_subscription_and_continues() {
+    let h = ManagerHarness::new();
+    write_limit_claude(&h);
+    let (proxy, switches) = fake_switch_proxy();
+    let config = h.config_home.join("claudio/config.toml");
+    let mut text = fs::read_to_string(&config).unwrap();
+    text.push_str(&format!(
+        "[proxy]\ndefault = \"work\"\n[proxy.profiles.work]\n\
+         url = \"http://{proxy}\"\ntoken = \"work-test-token\"\n"
+    ));
+    fs::write(&config, text).unwrap();
+    let mut tui = h.start_tui();
+    tui.wait_for("New session", Region::Screen, WAIT);
+    wizard_pick_dir(&mut tui, &h.dirs[0]);
+
+    let (auth, body) = switches
+        .recv_timeout(DAEMON_WAIT)
+        .expect("no switch request reached the proxy");
+    assert_eq!(auth, "Bearer work-test-token");
+    let body: serde_json::Value = serde_json::from_str(&body).unwrap();
+    assert_eq!(body, serde_json::json!({ "id": LIMIT_SID }));
+    // The notice first: it leaves the status bar after a few seconds.
+    tui.wait_for(
+        "usage limit: switched sub-a → sub-b, continuing (1/3)",
+        Region::StatusBar,
+        WAIT,
+    );
+    tui.wait_for("GOT continue", Region::Pane, WAIT);
+    assert!(switches.try_recv().is_err(), "one switch per limit hit");
+    assert!(!tui.screen_text(Region::Screen).contains("work-test-token"));
+    let log = fs::read_to_string(h.runtime_dir.join("claudio/daemon.log")).unwrap_or_default();
+    assert!(
+        !log.contains("work-test-token"),
+        "the token in the daemon log"
+    );
+    tui.quit(WAIT);
+}
````
In the new test only, temporarily make the `[proxy]` table read
`"[proxy]\ndefault = \"work\"\nauto_switch = false\n[proxy.profiles.work]\n\` (one
`auto_switch = false\n` after `default = \"work\"\n`), then:
Run: `cargo test --locked --test vocdoni_manager -- --test-threads=1 test_a_usage_limit`
Expected: `panicked at addons/vocdoni/tests/manager.rs:…: no switch request reached the proxy: Timeout`
Revert that one line (`git diff addons/vocdoni/tests/manager.rs` shows only the patch again).

Step 2: AC4 passes
Run: `cargo test --locked --test vocdoni_manager -- --test-threads=1 test_a_usage_limit`
Expected: `test result: ok. 1 passed; 0 failed` (about 5 s)
Run: `cargo test --locked --test vocdoni_manager -- --test-threads=1`
Expected: `test result: ok. 17 passed; 0 failed`

Step 3: commit
`git commit -m "test(vocdoni): a usage limit switches the subscription and continues" -- addons/vocdoni/tests/manager.rs`

Step 4: add AC6 (gated; it reports when skipped) (patch 11)
````diff
diff --git a/crates/tui/tests/manager.rs b/crates/tui/tests/manager.rs
index 9f7a413..7dfcfdb 100644
--- a/crates/tui/tests/manager.rs
+++ b/crates/tui/tests/manager.rs
@@ -1011,6 +1011,149 @@ fn test_e2e_real_claude() {
     tui.quit(WAIT);
 }
 
+/// Watch the daemon at `socket` from a connection of its own until a
+/// session's notice containing `needle` is followed by that session going
+/// `working` (a `UserPromptSubmit`): true if it is, false once `timeout`
+/// passes without it.
+fn notice_then_working(
+    socket: std::path::PathBuf,
+    needle: &'static str,
+    timeout: Duration,
+) -> thread::JoinHandle<bool> {
+    use std::io::{Read, Write};
+    thread::spawn(move || {
+        let Ok(mut s) = std::os::unix::net::UnixStream::connect(&socket) else {
+            return false;
+        };
+        let _ = s.set_read_timeout(Some(timeout));
+        let hello = serde_json::json!({"op": "hello", "claudio_version": "0.0.0-test", "proto": 1});
+        let body = serde_json::to_vec(&hello).unwrap();
+        let len = u32::try_from(body.len() + 1).unwrap().to_be_bytes();
+        if s.write_all(&len)
+            .and_then(|()| s.write_all(b"J"))
+            .and_then(|()| s.write_all(&body))
+            .is_err()
+        {
+            return false;
+        }
+        let deadline = Instant::now() + timeout;
+        let mut noticed = None;
+        while Instant::now() < deadline {
+            let mut len = [0u8; 4];
+            if s.read_exact(&mut len).is_err() {
+                return false;
+            }
+            let len = u32::from_be_bytes(len) as usize;
+            let mut frame = vec![0u8; len.min(1 << 20)];
+            if len > 1 << 20 || s.read_exact(&mut frame).is_err() {
+                return false;
+            }
+            let Some((b'J', json)) = frame.split_first() else {
+                continue;
+            };
+            let Ok(v) = serde_json::from_slice::<serde_json::Value>(json) else {
+                continue;
+            };
+            let event = &v["event"];
+            match event["kind"].as_str() {
+                Some("notice") if event["text"].as_str().is_some_and(|t| t.contains(needle)) => {
+                    noticed = Some(v["id"].clone());
+                }
+                Some("state")
+                    if event["state"] == "working" && noticed.as_ref() == Some(&v["id"]) =>
+                {
+                    return true;
+                }
+                _ => {}
+            }
+        }
+        false
+    })
+}
+
+/// AC6, gated by `CLAUDIO_E2E=1` (it costs two short turns of a real
+/// claude). The daemon's own type-dwell-Enter path delivers a prompt that
+/// starts a turn: with the debug-only `CLAUDIO_TEST_LIMIT_SWITCH=1`, the
+/// session's first `Stop` counts as a usage limit that "switched", so the
+/// daemon types `continue` and Enter into the real claude; its
+/// `UserPromptSubmit` (the session going `working`) follows the notice.
+#[test]
+fn test_e2e_usage_limit_continue_starts_a_turn() {
+    if std::env::var("CLAUDIO_E2E").as_deref() != Ok("1") {
+        eprintln!("skipped: set CLAUDIO_E2E=1 (needs an authenticated claude; costs tokens)");
+        return;
+    }
+    let root = std::env::temp_dir().join(format!("cl-e2e-limit-{}", std::process::id()));
+    let runtime_dir = root.join("run");
+    let config_home = root.join("cfg");
+    let session_dir = root.join("sess");
+    for p in [&runtime_dir, &config_home, &session_dir] {
+        fs::create_dir_all(p).unwrap();
+    }
+    common::write_test_config(&config_home);
+    let (lock, cleanup_root) = (runtime_dir.join("claudio/daemon-v1.lock"), root.clone());
+    let _guard = scopeguard(move || {
+        if let Some(pid) = fs::read_to_string(&lock)
+            .ok()
+            .and_then(|s| s.trim().parse::<i32>().ok())
+        {
+            // SAFETY: kill(2) has no memory-safety preconditions.
+            unsafe { libc::kill(pid, libc::SIGTERM) };
+        }
+        let _ = fs::remove_dir_all(&cleanup_root);
+    });
+    let _project_guard = ClaudeProjectGuard::new(&session_dir);
+
+    let mut cmd = CommandBuilder::new(BINARY);
+    // Isolated claudio state; the real HOME, where claude keeps its login.
+    for (k, _) in std::env::vars() {
+        if k.starts_with("CLAUDIO_") {
+            cmd.env_remove(&k);
+        }
+    }
+    cmd.env("XDG_RUNTIME_DIR", &runtime_dir);
+    cmd.env("XDG_CONFIG_HOME", &config_home);
+    cmd.env("ANTHROPIC_MODEL", "claude-haiku-4-5");
+    cmd.env("TERM", "xterm-256color");
+    cmd.env_remove("COLORTERM");
+    cmd.env("CLAUDIO_TEST_LIMIT_SWITCH", "1");
+    let mut tui = TuiProcess::spawn(cmd);
+    tui.wait_for("Alt+h help", Region::StatusBar, Duration::from_secs(30));
+    tui.send_keys(ENTER);
+    thread::sleep(Duration::from_millis(80));
+    tui.send_paste(session_dir.to_str().unwrap());
+    tui.send_keys(ENTER);
+    // claude is ready: its prompt in the pane and the SessionStart's `?`.
+    let ready = Instant::now() + Duration::from_secs(90);
+    let mut trusted = false;
+    loop {
+        let pane = tui.screen_text(Region::Pane);
+        if !trusted && pane.contains("Do you trust") {
+            tui.send_keys(ENTER);
+            trusted = true;
+            continue;
+        }
+        if pane.contains("❯") && tui.screen_text(Region::TabBar).contains('?') {
+            break;
+        }
+        assert!(Instant::now() < ready, "claude never got ready:\n{pane}");
+        thread::sleep(Duration::from_millis(200));
+    }
+    thread::sleep(Duration::from_millis(500));
+
+    let socket = runtime_dir.join("claudio/daemon-v1.sock");
+    let needle = "usage limit: switched test → test, continuing (1/3)";
+    let watch = notice_then_working(socket, needle, Duration::from_secs(120));
+    tui.send_paste("Reply with only the word ok");
+    tui.send_keys(ENTER);
+    tui.wait_for(needle, Region::StatusBar, Duration::from_secs(90));
+    assert!(
+        watch.join().unwrap(),
+        "no turn started after the daemon typed continue"
+    );
+    tui.quit(WAIT);
+}
+
 // ── Soak test ─────────────────────────────────────────────────────────────────
 
 /// Soak test: stay idle for 75 s (longer than the 45 s liveness deadline) then
````
Run: `cargo test --locked --test manager -- --test-threads=1 --nocapture test_e2e_usage_limit`
Expected: `skipped: set CLAUDIO_E2E=1 (needs an authenticated claude; costs tokens)` and
`test result: ok. 1 passed`
Run: `cargo test --locked --test manager -- --test-threads=1` and
`cargo test --locked --no-default-features --test manager -- --test-threads=1`
Expected: `test result: ok. 43 passed; 0 failed` both times
Do not run it with `CLAUDIO_E2E=1`: the controller does, if an authenticated claude is present.
**Unverified:** the `CLAUDIO_E2E=1` path against a real claude (it needs an authenticated claude
and costs tokens). Its debug trigger and its event watcher (`notice_then_working`) were checked
in the scratch tree against a fake claude that fires `Stop`, then `UserPromptSubmit` on each
line it reads: the watcher saw the notice and then `working`.

Step 5: commit
`git commit -m "test(tui): the daemon's typed continue starts a real claude turn (opt-in)" -- crates/tui/tests/manager.rs`

Interfaces this task provides to later tasks: none.

### Task 7: Docs: the design, the README, the Vocdoni section and CLAUDE.md
Model: haiku   Repo: /home/p4u/claudio   Files: `docs/addons.md`, `docs/manager-design.md`,
`README.md`, `CLAUDE.md`
Goal: AC5 (F1). The Vocdoni section documents `[proxy] auto_switch` and the mapping. The design
gets §2.2's "Usage limits: switch and continue". The README's Vocdoni section says what users
see and the config key. CLAUDE.md gets one sentence.

Step 1: apply the patch (patch 12)
````diff
diff --git a/CLAUDE.md b/CLAUDE.md
index 59a4b08..5560a5a 100644
--- a/CLAUDE.md
+++ b/CLAUDE.md
@@ -172,7 +172,9 @@ words, core config sections, shared flags and shared panel keys.
   alias `proxy …`; `[proxy]` profiles; the "Proxy stats" panel on Alt+s) and the team plugins
   (manifests from `GET /v1/claudio/plugins`, one checklist per manifest, installed on the
   daemon by the `vocdoni/apply` addon op). `state/` is the one plugin state machine, pure;
-  `runner.rs` does its I/O.
+  `runner.rs` does its I/O. Its `on_usage_limit` (`proxy/limit.rs`) moves a proxy session to
+  another subscription when a turn ends on a usage limit, and the daemon
+  (`daemon/session/limit.rs`) then types `continue` (`[proxy] auto_switch = false` turns it off).
 
 **A3 across crates:** the core crates cannot import an addon (A2: no such dependency) and
 never branch on an addon's name; an addon imports `claudio_core::addon`, the core's public
diff --git a/README.md b/README.md
index a6ac6ed..4cc4429 100644
--- a/README.md
+++ b/README.md
@@ -155,6 +155,8 @@ New sessions use the default profile. `--proxy NAME`, `--no-proxy` and `--first-
 
 *The Vocdoni addon's panel: usage, the pool, your limit and the top models, with an action to move the session to the next subscription.*
 
+When a proxy session's turn ends on a subscription limit ("You've hit your 5-hour limit"), the daemon on its host asks the proxy to move the conversation to another subscription and types `continue`, at most 3 times in a row; a notice says what happened, and a direct session is left alone. `auto_switch = false` under `[proxy]` in that host's `config.toml` turns it off.
+
 The proxy can also serve **plugins**: a pinned, hash-checked manifest of Claude Code plugins, MCP servers (their secrets come as env), subagents and instructions for every session. The first time claudio sees one it asks what to install; each host's daemon then installs the plugins with `claude plugin install` (never with `-y`), and every session on that proxy profile gets the servers, agents and instructions. What you already have is left alone. The extensions screen shows each plugin's state per host.
 
 Two ways to run through the proxy:
diff --git a/docs/addons.md b/docs/addons.md
index 11147ea..bf0d478 100644
--- a/docs/addons.md
+++ b/docs/addons.md
@@ -449,6 +449,16 @@ The manifest is downloaded once per hash into `<cache>/vocdoni/` (the addon's `c
 `<cache>/plugins/` is downloaded again once. Installs run after the reply, under the
 `updating()` lock, as `claude plugin … --json`; `on_claude_installed` forgets their failures.
 
+**Usage limits.** Its `on_usage_limit` answers `NotMine` when the host's `config.toml` has
+`[proxy] auto_switch = false` (a bool, default `true`, read through `DaemonCtx::config`), and
+when the session's env lacks `ANTHROPIC_BASE_URL` or `ANTHROPIC_AUTH_TOKEN` (a direct session).
+Otherwise it asks that proxy to move the conversation, `POST /v1/claudio/session/switch`
+within 15 s: 202 is `Switched` with the proxy's names (`current subscription` and `next
+available` when it sends none), 409 is `Failed` with the proxy's reason (no other eligible
+subscription, or a session bound to its account), 404 is `Failed("the proxy does not know this
+session")`, and anything else `Failed` with the client's error. The token goes only to the
+session's own proxy URL, never to a log or an outcome.
+
 **Launch.** Flags `--proxy NAME`, `--no-proxy` and `--first-party` (the wizard checkbox
 "First-party session"); one option per proxy profile (group "Proxy", the configured default
 marked). The default profile, or a chosen one, is `Routing::Required`: a profile that does not
diff --git a/docs/manager-design.md b/docs/manager-design.md
index 702f4f7..31be943 100644
--- a/docs/manager-design.md
+++ b/docs/manager-design.md
@@ -236,6 +236,41 @@ Extra metrics come from the JSONL tail:
 
 **[later]** The statusLine feed, chained to the user's own statusLine command.
 
+#### Usage limits: switch and continue
+
+A proxy session that hits a subscription limit ("You've hit your 5-hour limit · resets 4pm
+(UTC)") would sit stopped until someone switched it by hand. Its host's daemon does that
+instead (`crates/core/src/daemon/session/limit.rs`), with no client attached and once per hit.
+
+- **Detection.** `StopFailure` with `error: "rate_limit"` and a `last_assistant_message` that
+  starts with one of claude 2.1.322's own usage-limit prefixes (`claude::hooks::is_usage_limit`).
+  The proxy's own per-user limit ("API Error: 429 proxy: usage limit reached …") and other
+  errors do not count.
+- **The guard**, pure, in the session actor: a hit counter `seq`, the automatic continues in a
+  row (a `Stop` resets them), one attempt in flight, and whether a client typed since its last
+  `UserPromptSubmit`. Typing is a client's `Input` other than focus and mouse reports; the
+  daemon's own writes never count.
+- **The attempt** runs on a task of its own. It copies the session's spawn env out of the
+  registry and asks each daemon-side addon's `DaemonOps::on_usage_limit` in registry order
+  (docs/addons.md, "Usage limits"), within `Config::limit_attempt_timeout` (20 s). It reports
+  one `Cmd::Limit(Done)` to the actor that started it, through a weak handle on that actor's
+  channel, so a respawned session never hears it. A panic is `Failed("addon panicked")`.
+- **Typing.** After `Switched`: a 1 s timer, `continue`, a 150 ms timer, then `\r` as a
+  separate write (the api driver's text-dwell-Enter rule). The timers are tasks that send the
+  actor a `Cmd`, never a sleep on it. Each step types only while `seq` is still the hit's, the
+  state is still `Error` and no client typed; otherwise the notice says the switch applies to
+  the next message.
+- **Bounds.** 3 automatic continues in a row, then a notice to continue by hand. A 409 from the
+  proxy (no other subscription) is a `Failed` notice and stops at once. The counters live in
+  the actor, so a respawn starts at zero.
+- **Rollout.** The feature lives in the daemon: a host gets it when its daemon is from this
+  release. `[proxy] auto_switch = false` in that host's `config.toml` turns it off at the next
+  hit. No protocol change: clients show the `Notice` events.
+- **Testing.** In debug builds only, `CLAUDIO_TEST_LIMIT_SWITCH=1` in the daemon's env makes
+  each session's first `Stop` a usage limit that switches without an addon. The opt-in
+  real-claude test `test_e2e_usage_limit_continue_starts_a_turn` uses it to check that the typed
+  `continue` starts a turn.
+
 ### 2.3 Persistence and recovery
 
 There are two journals, each written durably (write-tmp + fsync + rename):
````
Run: `grep -c "auto_switch" docs/addons.md docs/manager-design.md README.md CLAUDE.md`
Expected: `docs/addons.md:1`, `docs/manager-design.md:1`, `README.md:1`, `CLAUDE.md:1`
Run: `make check`
Expected: every crate's `test result: ok`, `rules: 0 problem(s)`

Step 2: commit
`git commit -m "docs(limit): the usage-limit auto-switch in the design, readme and CLAUDE.md" -- docs/addons.md docs/manager-design.md README.md CLAUDE.md`

Interfaces this task provides to later tasks: none.

## Merge and release
1. Full suites on `feat/limit-auto-switch` after Task 7, from the repo root:
   - `make check` → every crate's `test result: ok` (claudio-core 562), `rules: 0 problem(s)`.
   - `make matrix` → the five cells pass.
   - `cargo test --locked --test manager --test vocdoni_manager --test web --test web_manager --test vocdoni_web -- --test-threads=1`
     → `manager` 43, `vocdoni_manager` 17, `web` 52, `web_manager` 4, `vocdoni_web` 13 passed.
   - `make test-web` → every spec passes, no CSP violation (no page change in this work).
   - Opt-in, only if an authenticated `claude` is on PATH (it costs two short Haiku turns of
     tokens, uses the real HOME's claude login, and removes the
     `~/.claude/projects/<tmp dir>` it creates; it runs its own daemon under a temp
     `XDG_RUNTIME_DIR`):
     `CLAUDIO_E2E=1 cargo test --locked --test manager -- --test-threads=1 --nocapture test_e2e_usage_limit_continue_starts_a_turn`
     → `ok. 1 passed`. Without a claude, say so in the gate report: AC6 was not run.
   - `make static` (or, without the musl target, `cargo build --release --locked` and
     `! grep -q CLAUDIO_TEST_LIMIT_SWITCH target/release/claudio`) → the R8 grep passes.
2. Fable gate audit of `main...feat/limit-auto-switch`; fixes re-reviewed and the suites re-run.
3. Merge: `git switch main && git merge --no-ff feat/limit-auto-switch`. If `feat/agent-forwarding`
   merged first, resolve the "Shared files" hunks keeping both sides, then re-run `make check`
   and the five suites on `main` before the release commit.
4. Release (owner, 2026-11-10: merge and release without asking once the gate passes):
   - The version is the next patch after the root `Cargo.toml`'s `version` at merge time:
     0.9.2 → 0.9.3, or 0.9.4 if the agent-forwarding release took 0.9.3. Never v0.10.0
     (reserved for removing the `compat(v0.10)` paths).
   - Bump `version` in the root `Cargo.toml`, then `cargo update -w` (the lock's `claudio*`
     entries). Commit with
     `git commit -m "chore(release): vX.Y.Z" -- Cargo.toml Cargo.lock`, then
     `git tag -a vX.Y.Z -m "vX.Y.Z"`.
   - Push over HTTPS with the gh credential helper:
     `git -c credential.helper= -c 'credential.helper=!gh auth git-credential' push https://github.com/p4u/claudio.git main vX.Y.Z`.
   - After `.github/workflows/release.yml` publishes, set the notes:
     `gh release edit vX.Y.Z --repo p4u/claudio --notes-file <notes.md>`, in the style of
     v0.9.0. Items:
     - **Usage limits switch and continue (Vocdoni proxy sessions).** When a proxy session's
       turn ends on a subscription limit, the daemon on its host asks the proxy to move the
       conversation to another subscription and types `continue`, at most 3 times in a row; a
       notice says what happened. It runs with no TUI or browser attached. Off with
       `[proxy] auto_switch = false` in that host's `config.toml`. A host gets it when its
       daemon runs this release (`claudio daemon restart` on an idle host).
     - Direct sessions, other API errors and the proxy's own per-user limit are left as they
       were.
     - Addon API: `DaemonOps::on_usage_limit` and `DaemonCtx::config` (docs/addons.md).
5. **Owner OK required before:** nothing beyond a passed gate audit (owner, 2026-11-10: "don't
   ask me, just merge and publish the new release"). Still stop and ask for a genuine design or
   security trade-off that the gate raises.
6. Post-release checks: the release workflow is green and `gh release view vX.Y.Z --repo
   p4u/claudio` lists the four platform assets; on this machine, after installing the release,
   `claudio daemon status` shows the new version once the daemon is idle and restarted.
