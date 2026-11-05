# LaTeX formulas in the session pane: implementation plan

Spec: docs/specs/2026-11-04-latex-render-design.md (rev 2, audited; rulings R1–R10 in .delivery/2026-11-04-latex-render/progress.md)
Class: architectural
Repos and branches: /volumes/repos/claudio → feat/latex-render (base 76c2b27 main v0.3.7; spec commit 5ef0aed)

Every task's code is given as exact diffs against the commit the previous task leaves. All of them were applied in order to a scratch clone and run there; the "Expected" lines are the real outputs. Step 1 adds the tests (and must fail as stated), Step 2 adds the implementation, Step 3 commits.

## Global Constraints
- Commit convention: conventional commits `type(scope): lowercase summary`, NO AI attribution and NO Co-Authored-By trailer of any kind (owner's global CLAUDE.md, ruling R10). Use the exact commit commands given.
- Never push. Commit with explicit paths only: `git commit -m "…" -- <paths>`. A new file needs `git add <path>` first. Never `git add -A`.
- Work on `feat/latex-render` in /volumes/repos/claudio (the controller may give you a worktree of it). Do not create, switch or delete branches.
- How to apply a diff block: copy it verbatim (everything between the five-backtick fences) into `~/.cache/claudio/plan-latex/T<n>-step<k>.diff`, then `git apply --check <file> && git apply <file>`. If `--check` fails, stop and report: the tree is not the state the plan expects. Editing by hand to the same result is fine.
- Build output goes in the repo's `target/` or under `~/.cache/claudio/`, never /tmp.
- Run tests with `umask 022` in the same shell (`umask 022; cargo test …`). Under umask 077, `upgrade::tests::install_binary_replaces_file` fails on the base too.
- The shell may compact cargo output (the `rtk` hook). Prefix a command with `rtk proxy` to see raw output. The expected lines below are raw cargo output.
- Known base flakes (timing races, also seen on the base commit at about 1 run in 8): `daemon::host::tests::probe_runs_claude_with_the_given_path`, `daemon::login_path::tests::capture_times_out_and_kills_the_shell`, `daemon::tests::freshness::an_outdated_idle_daemon_is_replaced_and_its_sessions_come_back`. If one of them, and nothing else, fails, run the suite once more. Report a second failure.
- No repo-wide `cargo fmt`: the tree is not rustfmt-clean. Only `src/config.rs`, `src/claude/hooks.rs` and `src/cli.rs` were clean before, and they must stay clean (`rustfmt --edition 2021 --check <file>`). The new files `src/claude/latex.rs`, `src/tui/latex.rs`, `src/tui/formulas.rs` and `src/tui/ui/formulas.rs` must be rustfmt-clean. Leave the formatting of every other file alone and match its style.
- Match the repo's comment density and naming: a doc comment on each item that says why, plain sentences, no banners beyond the existing `// ── Section ──` style.
- `app.rs` and the `App` methods in `interaction.rs`/`sessions.rs` stay pure (no I/O, no threads, no rendering). Threads and I/O live in `tui/mod.rs`. `tui/latex.rs`'s `render` is called only from `render_formula` in `tui/mod.rs`, never from `App`, a draw, or a test with hostile input (Task 4's unit tests call it with benign formulas only).
- Protocol additions stay backward compatible: a new `SessionEvent` kind only (old clients decode it as `Unknown`), no `PROTO` bump, no new field on an existing message.
- Manager (PTY) tests assert on the rendered screen model, never on raw bytes. Run them single-threaded (`-- --test-threads=1`) and with nothing else heavy running on the machine.
- No secrets and no formula text in argv, logs or state files. `__context` prints a constant.
- Builds on Linux and macOS, and the static musl build (`make static`) keeps working: pure-Rust dependencies only.

## Review Focus
- src/tui/mod.rs:398 `panic_hook` and :105 `QUIET_PANIC`. A panic on a render scratch thread must neither restore the terminal nor print. Every other panic still restores, then reports.
- src/tui/mod.rs:594 `guarded`. The `running` count is incremented before the spawn and decremented before the result is sent, so after `guarded` returns it counts only abandoned threads. A spawn failure is `Busy`. `recv_timeout(RENDER_BUDGET)` is the only wait. Check that nothing else can block the worker or the UI.
- src/tui/mod.rs:567 `render_formula` and :531 `RenderCache`. `too_complex` runs before any parse. `Busy` results are never cached; timeouts and panics are cached raw. LRU eviction at 1024.
- src/tui/mod.rs:507 `spawn_latex_worker`, :986 `Effect::RenderLatex`, :894 `HostEvent::LatexRendered`. Rendering never happens on the event loop.
- src/claude/latex.rs:24 `sanitize` and :49 `is_unsafe`: the Cc/Cf/Co/noncharacter ranges, and orphan combining marks.
- src/claude/latex.rs:96 `extract_latex` and :140 `code_spans`: fences, inline code, byte offsets (the cut points are always ASCII).
- src/claude/projects.rs:323 the scan happens before the `usage` early return, skips sidechains and reads only `text` blocks; :276 the list is cleared on a rewritten transcript; :359 `push_latex`.
- src/tui/latex.rs:89 `too_complex` (token and depth counting) and :131 `render` (caps, sanitizing).
- src/tui/latex.rs:493 `overlay`, :552 `rebuild_row`, :591 `overlay_span`, :658 `matching`, :678 `text_cells`, :719 `write_row`. Rows are rebuilt from the original cells, left to right. Any overflow leaves the row raw. Wide clusters keep their covered cell. No row outside a span changes.
- src/daemon/session.rs:605 `latex_event` (seq grows for the actor's whole life; a new cursor sends an empty list under the next seq, never seq 1 again — ruling R15) and :583 its call under the slot lock; :463 `Subscriber::attach` and :856 the replay of the last list after `Attached` + snapshot.
- src/tui/interaction.rs:756 the `seq > latex_seq` rule, :708/:716 the resets on `ClaudeSession`/`ClaudeSessionCleared`, :775 `on_latex_rendered` (kept only for the `seq` held).
- src/claude/hooks.rs:68 the only change to the settings JSON (an extra SessionStart command when `latex`); :97 `context` / :102 `context_to`: the drain is in `context_to` and touches no socket.

## Task order and parallelism
| Task | Depends on | Repo/files | May run in parallel with |
|---|---|---|---|
| 1 | none | src/config.rs, src/claude/hooks.rs, src/daemon/mod.rs, src/daemon/session.rs, src/daemon/tests.rs, src/main.rs | 2 |
| 2 | none | src/claude/latex.rs (new), src/claude/mod.rs, src/claude/projects.rs | 1 |
| 3 | 1, 2 | src/proto.rs, src/daemon/session.rs, src/tui/interaction.rs | 4 |
| 4 | 2 | Cargo.toml, Cargo.lock, src/tui/latex.rs (new), src/tui/mod.rs | 3 |
| 5 | 3, 4 | src/tui/mod.rs, src/tui/app.rs, src/tui/sessions.rs, src/tui/interaction.rs | none |
| 6 | 5 | src/tui/latex.rs, src/tui/ui/mod.rs | none |
| 7 | 6 | src/tui/keymap.rs, src/tui/interaction.rs, src/tui/plain.rs, src/tui/formulas.rs (new), src/tui/ui/formulas.rs (new), src/tui/mod.rs, src/tui/ui/mod.rs, src/cli.rs, tests/manager.rs | none |
| 8 | 7 | tests/manager.rs, tests/common/mod.rs | 9 |
| 9 | 7 | README.md, docs/manager-design.md, CLAUDE.md | 8 |

The parallel pairs (1∥2, 3∥4, 8∥9) touch disjoint files, so the controller may run each pair in two git worktrees of `feat/latex-render` and then cherry-pick both commits onto the branch, in task order. The diffs apply in either order within a pair. The `N passed` figures per task do not depend on the order. The `M filtered out` figures and the `make test-unit` totals do: they assume strict task order (base 783; after Task 1: 787, 2: 796, 3: 799, 4: 804, 5: 810, 6: 820, 7: 827). In a worktree without its pair partner, Task 2 reads `57 passed … 735 filtered out` then `792 passed`, and Task 4 reads `5 passed … 796 filtered out` then `801 passed`; Tasks 1 and 3 are unchanged.

## Tasks

### Task 1: `[claude] latex` and the `__context latex` SessionStart hook
Model: haiku   Repo: /volumes/repos/claudio   Files: src/config.rs, src/claude/hooks.rs, src/daemon/mod.rs, src/daemon/session.rs, src/daemon/tests.rs, src/main.rs
Goal: AC1 and AC2, plus the config header comment of AC12. `ClaudeSection` and `daemon::Config` gain `latex: bool`, and `hooks::settings` gains a `latex` parameter that adds exactly one `'<bin>' __context latex` command to `SessionStart`. The new hidden subcommand prints the instruction as one JSON line.

Step 1: write the failing tests. New tests in src/config.rs, src/claude/hooks.rs and src/daemon/session.rs; the existing `settings(…)` calls gain a `false` argument. This step also adds the `latex: bool` field to `daemon::Config` and reads it in `from_env` (so the error list below includes the three `daemon::Config` literals of src/daemon/tests.rs that Step 2 updates, and `claude.latex` on a `ClaudeSection` that has no such field yet).
`````diff
diff --git a/src/claude/hooks.rs b/src/claude/hooks.rs
index a11751b..a7e0f32 100644
--- a/src/claude/hooks.rs
+++ b/src/claude/hooks.rs
@@ -156,6 +156,7 @@ mod tests {
             Path::new("/usr/bin/claudio"),
             Path::new("/run/claudio/d.sock"),
             "tok",
+            false,
         );
         let hooks = v["hooks"].as_object().unwrap();
         for event in EVENTS {
@@ -165,7 +166,12 @@ mod tests {
 
     #[test]
     fn settings_command_contains_socket_and_token() {
-        let v = settings(Path::new("/bin/cl"), Path::new("/run/s.sock"), "mytoken");
+        let v = settings(
+            Path::new("/bin/cl"),
+            Path::new("/run/s.sock"),
+            "mytoken",
+            false,
+        );
         let cmd = v["hooks"]["Stop"][0]["hooks"][0]["command"]
             .as_str()
             .unwrap();
@@ -180,6 +186,7 @@ mod tests {
             Path::new("/home/user name/bin/claudio"),
             Path::new("/run/user data/s.sock"),
             "tok",
+            false,
         );
         let cmd = v["hooks"]["Stop"][0]["hooks"][0]["command"]
             .as_str()
@@ -189,6 +196,50 @@ mod tests {
         assert!(cmd.contains("'/run/user data/s.sock'"), "cmd: {cmd}");
     }
 
+    #[test]
+    fn latex_adds_one_context_command_to_session_start_only() {
+        let (bin, sock) = (Path::new("/usr/bin/claudio"), Path::new("/run/d.sock"));
+        let off = settings(bin, sock, "tok", false);
+        let on = settings(bin, sock, "tok", true);
+        for event in EVENTS.iter().filter(|e| **e != "SessionStart") {
+            assert_eq!(on["hooks"][event], off["hooks"][event], "{event}");
+        }
+        let start = |v: &serde_json::Value| v["hooks"]["SessionStart"][0].clone();
+        let (off_start, on_start) = (start(&off), start(&on));
+        assert_eq!(off_start["hooks"].as_array().unwrap().len(), 1);
+        let hooks = on_start["hooks"].as_array().unwrap();
+        assert_eq!(hooks.len(), 2);
+        assert_eq!(hooks[0], off_start["hooks"][0], "the relay stays first");
+        assert_eq!(hooks[1]["type"], "command");
+        assert_eq!(hooks[1]["command"], "'/usr/bin/claudio' __context latex");
+        assert_eq!(on_start["matcher"], off_start["matcher"]);
+        assert_eq!(on_start["timeout"], off_start["timeout"]);
+    }
+
+    #[test]
+    fn context_latex_prints_one_session_start_line() {
+        let mut out = Vec::new();
+        context_to(Some("latex"), &b"{\"session_id\":\"s\"}"[..], &mut out);
+        let text = String::from_utf8(out).unwrap();
+        assert_eq!(text.matches('\n').count(), 1, "one line: {text}");
+        let v: Value = serde_json::from_str(text.trim_end()).unwrap();
+        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "SessionStart");
+        let context = v["hookSpecificOutput"]["additionalContext"]
+            .as_str()
+            .unwrap();
+        assert_eq!(context, LATEX_CONTEXT);
+        assert!(context.contains("<latex>") && context.contains("</latex>"));
+    }
+
+    #[test]
+    fn context_prints_nothing_for_anything_else() {
+        for kind in [None, Some(""), Some("other"), Some("LATEX")] {
+            let mut out = Vec::new();
+            context_to(kind, &b"{}"[..], &mut out);
+            assert!(out.is_empty(), "{kind:?}");
+        }
+    }
+
     #[test]
     fn new_token_is_32_hex_chars() {
         let tok = new_token();
diff --git a/src/config.rs b/src/config.rs
index 8cdcf34..8edd2a0 100644
--- a/src/config.rs
+++ b/src/config.rs
@@ -247,6 +247,19 @@ mod tests {
         let _ = std::fs::remove_dir_all(&dir);
     }
 
+    #[test]
+    fn latex_is_off_unless_turned_on() {
+        let file: ConfigFile = toml::from_str("[claude]\nupdate_check = \"off\"\n").unwrap();
+        assert!(!file.claude.unwrap().latex);
+        assert!(!ClaudeSection::default().latex);
+        let file: ConfigFile = toml::from_str("[claude]\nlatex = true\n").unwrap();
+        let claude = file.claude.unwrap();
+        assert!(claude.latex);
+        // The other keys keep their defaults.
+        assert!(claude.allow_skip_permissions);
+        assert_eq!(claude.update_check, UpdatePolicy::Ask);
+    }
+
     #[test]
     fn parses_ui_and_keys_sections() {
         let toml = r#"
diff --git a/src/daemon/mod.rs b/src/daemon/mod.rs
index dd187da..cecabdd 100644
--- a/src/daemon/mod.rs
+++ b/src/daemon/mod.rs
@@ -73,6 +73,9 @@ pub struct Config {
     /// `[claude] allow_skip_permissions` from this host's `config.toml`:
     /// launch claude with `--allow-dangerously-skip-permissions`.
     pub skip_permissions: bool,
+    /// `[claude] latex` from this host's `config.toml`: every claude session
+    /// gets the `__context latex` SessionStart hook.
+    pub latex: bool,
     /// The binary this daemon runs, as of its start (reported in `Welcome`
     /// so clients can tell an outdated daemon; see [`crate::freshness`]).
     /// `None` when it could not be read.
@@ -93,13 +96,15 @@ impl Config {
             inherited.as_deref(),
             login_path::TIMEOUT,
         );
+        let claude = crate::config::load().claude;
         Ok(Config {
             socket: paths::daemon_socket(),
             lock: paths::daemon_lock(),
             journal: paths::daemon_journal(),
             claude: claude_setting(std::env::var_os("CLAUDIO_CLAUDE_PATH")),
             claudio: std::env::current_exe()?,
-            skip_permissions: crate::config::load().claude.allow_skip_permissions,
+            skip_permissions: claude.allow_skip_permissions,
+            latex: claude.latex,
             // Hashed now: an upgrade may replace the file while we run.
             build: Probe::current()
                 .inspect_err(|e| tracing::warn!(error = %e, "cannot identify this binary"))
@@ -239,6 +244,7 @@ mod claude_path_tests {
             claude,
             claudio: "/bin/claudio".into(),
             skip_permissions: false,
+            latex: false,
             build: None,
             path,
         }
diff --git a/src/daemon/session.rs b/src/daemon/session.rs
index 42bc1ed..f95e95a 100644
--- a/src/daemon/session.rs
+++ b/src/daemon/session.rs
@@ -1079,6 +1079,7 @@ mod tests {
             claude: "/bin/claude".into(),
             claudio: "/bin/claudio".into(),
             skip_permissions: true,
+            latex: false,
             build: None,
             path: None,
         };
@@ -1111,6 +1112,16 @@ mod tests {
             [ALLOW, "--resume", "abc", "-n", "work"]
         );
         assert_eq!(spec.args, ["--resume", "abc"]);
+
+        // `[claude] latex` adds the context hook to SessionStart.
+        let start = |cfg: &super::super::Config| {
+            let argv = argv_of(&command(cfg, &spec, Path::new("/"), "t", false));
+            let settings: Value = serde_json::from_str(&argv[2]).unwrap();
+            settings["hooks"]["SessionStart"][0]["hooks"].clone()
+        };
+        assert_eq!(start(&cfg).as_array().unwrap().len(), 1);
+        let latex = super::super::Config { latex: true, ..cfg.clone() };
+        assert_eq!(start(&latex)[1]["command"], "'/bin/claudio' __context latex");
     }
 
     const ALLOW: &str = "--allow-dangerously-skip-permissions";
@@ -1131,6 +1142,7 @@ mod tests {
             claude: "/bin/claude".into(),
             claudio: "/bin/claudio".into(),
             skip_permissions: true,
+            latex: false,
             build: None,
             path: None,
         };
`````
Run: `umask 022; cargo test --locked --bin claudio -- config:: claude::hooks daemon::session`
Expected: compilation fails with `error: could not compile `claudio` (bin "claudio" test) due to 15 previous errors`, made of:
```
      5 error[E0061]: this function takes 3 arguments but 4 arguments were supplied
      4 error[E0609]: no field `latex` on type `config::ClaudeSection`
      3 error[E0063]: missing field `latex` in initializer of `daemon::Config`
      2 error[E0425]: cannot find function `context_to` in this scope
      1 error[E0425]: cannot find value `LATEX_CONTEXT` in this scope
```

Step 2: implement.
`````diff
diff --git a/src/claude/hooks.rs b/src/claude/hooks.rs
index a7e0f32..dee835c 100644
--- a/src/claude/hooks.rs
+++ b/src/claude/hooks.rs
@@ -8,6 +8,8 @@
 //! - [`EVENTS`] — the hook events the daemon registers.
 //! - [`settings`] — build the `--settings` JSON blob for a session.
 //! - [`relay`] — the short-lived relay process that ships the payload.
+//! - [`context`] — `claudio __context latex`, the SessionStart hook that asks
+//!   claude to wrap math in `<latex>` tags (`[claude] latex`).
 //! - [`new_token`] — generate a per-spawn secret token.
 
 use std::io::{Read, Write};
@@ -37,23 +39,40 @@ const WRITE_TIMEOUT: Duration = Duration::from_secs(2);
 /// Maximum hook payload size. Hooks should be tiny; 1 MiB is a hard cap.
 const MAX_PAYLOAD: usize = 1 << 20;
 
+/// What `claudio __context latex` adds to every conversation's context when
+/// `[claude] latex` is on: the manager renders `<latex>` tags in the pane.
+/// Context, not the system prompt, so the session stays a plain interactive
+/// one (see CLAUDE.md on system prompts).
+pub const LATEX_CONTEXT: &str = "This terminal renders math written as LaTeX inside \
+<latex>…</latex> tags. Whenever you write a mathematical formula or expression, wrap its \
+LaTeX in <latex>…</latex> instead of $…$ or $$…$$, for example \
+<latex>x = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}{2a}</latex>. Put a display formula alone on \
+its own line. Never put the tags inside code blocks or inline code, and do not wrap plain \
+numbers or code in them. Write relations with spaces around them (a < b, not a<b).";
+
 // ── Settings generation ───────────────────────────────────────────────────────
 
 /// Build the `--settings` JSON value that registers all daemon hooks.
 ///
 /// The hook command is `'<claudio_bin>' __hook <Event> '<socket>' <token>`,
 /// with paths shell-quoted so that spaces and special characters are safe.
-pub fn settings(claudio_bin: &Path, socket: &Path, token: &str) -> serde_json::Value {
+/// With `latex`, `SessionStart` runs `'<claudio_bin>' __context latex` too.
+pub fn settings(claudio_bin: &Path, socket: &Path, token: &str, latex: bool) -> serde_json::Value {
     let bin = shell_quote(&claudio_bin.to_string_lossy());
     let sock = shell_quote(&socket.to_string_lossy());
 
     let mut hooks_map = serde_json::Map::new();
     for event in EVENTS {
         let command = format!("{bin} __hook {event} {sock} {token}");
+        let mut hooks = vec![serde_json::json!({"type": "command", "command": command})];
+        if latex && *event == "SessionStart" {
+            let context = format!("{bin} __context latex");
+            hooks.push(serde_json::json!({"type": "command", "command": context}));
+        }
         let entry = serde_json::json!([{
             "matcher": "*",
             "timeout": 5,
-            "hooks": [{"type": "command", "command": command}]
+            "hooks": hooks
         }]);
         hooks_map.insert(event.to_string(), entry);
     }
@@ -69,6 +88,32 @@ fn shell_quote(s: &str) -> String {
     format!("'{escaped}'")
 }
 
+// ── Context ───────────────────────────────────────────────────────────────────
+
+/// `claudio __context <kind>`: a SessionStart hook that adds to claude's
+/// context. Drains the hook payload (capped like the relay), then prints the
+/// one JSON line that adds [`LATEX_CONTEXT`] for `latex`, and nothing for any
+/// other kind. Always exits 0 and never touches the daemon's socket.
+pub fn context(kind: Option<&str>) -> std::process::ExitCode {
+    context_to(kind, std::io::stdin(), &mut std::io::stdout());
+    std::process::ExitCode::SUCCESS
+}
+
+fn context_to(kind: Option<&str>, input: impl Read, out: &mut impl Write) {
+    let _ = std::io::copy(&mut input.take(MAX_PAYLOAD as u64), &mut std::io::sink());
+    if kind != Some("latex") {
+        return;
+    }
+    let line = serde_json::json!({
+        "hookSpecificOutput": {
+            "hookEventName": "SessionStart",
+            "additionalContext": LATEX_CONTEXT,
+        }
+    });
+    let _ = writeln!(out, "{line}");
+    let _ = out.flush();
+}
+
 // ── Token generation ─────────────────────────────────────────────────────────
 
 /// Generate a 128-bit random token as a 32-character hex string.
diff --git a/src/config.rs b/src/config.rs
index 8edd2a0..4cd1185 100644
--- a/src/config.rs
+++ b/src/config.rs
@@ -13,6 +13,8 @@
 //! allow_skip_permissions = true   # pass --allow-dangerously-skip-permissions
 //! update_check = "ask"            # local claude vs its release channel
 //! remote_check = "ask"            # remote claude vs the local version
+//! latex = false                   # ask claude to wrap math in <latex> tags
+//!                                 # (read at daemon start: `claudio daemon restart`)
 //!
 //! [keys]
 //! overview    = "alt+g"
@@ -105,6 +107,11 @@ pub struct ClaudeSection {
     /// Compare a remote host's claude with the local version on connect.
     #[serde(default)]
     pub remote_check: UpdatePolicy,
+    /// Tell every claude session this host's daemon starts to wrap math in
+    /// `<latex>…</latex>`, which the manager renders. Read when the daemon
+    /// starts, from the config of the host that runs the session.
+    #[serde(default)]
+    pub latex: bool,
 }
 
 fn default_true() -> bool {
@@ -117,6 +124,7 @@ impl Default for ClaudeSection {
             allow_skip_permissions: true,
             update_check: UpdatePolicy::Ask,
             remote_check: UpdatePolicy::Ask,
+            latex: false,
         }
     }
 }
diff --git a/src/daemon/session.rs b/src/daemon/session.rs
index f95e95a..27d2bf9 100644
--- a/src/daemon/session.rs
+++ b/src/daemon/session.rs
@@ -269,7 +269,7 @@ fn command(
     token: &str,
     skip_permissions: bool,
 ) -> CommandBuilder {
-    let settings = hooks::settings(&cfg.claudio, &cfg.socket, token);
+    let settings = hooks::settings(&cfg.claudio, &cfg.socket, token, cfg.latex);
     let mut cmd = CommandBuilder::new(cfg.claude_bin());
     cmd.arg("--settings");
     cmd.arg(settings.to_string());
diff --git a/src/daemon/tests.rs b/src/daemon/tests.rs
index 42d889f..63ad296 100644
--- a/src/daemon/tests.rs
+++ b/src/daemon/tests.rs
@@ -107,6 +107,7 @@ impl TestDaemon {
             claude: fake_claude().to_path_buf(),
             claudio: PathBuf::from("/bin/true"),
             skip_permissions: true,
+            latex: false,
             build: None,
             path: None,
         }
@@ -763,6 +764,7 @@ async fn resume_retry_spawns_fresh_on_quick_exit() {
         claude: fake_claude_resume_fails().to_path_buf(),
         claudio: PathBuf::from("/bin/true"),
         skip_permissions: true,
+        latex: false,
         build: None,
         path: None,
     };
@@ -877,6 +879,7 @@ async fn immediate_exit_leaves_session_dormant() {
         claude: fake_claude_exits_immediately().to_path_buf(),
         claudio: PathBuf::from("/bin/true"),
         skip_permissions: true,
+        latex: false,
         build: None,
         path: None,
     };
diff --git a/src/main.rs b/src/main.rs
index 6399476..7c41cf1 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -116,6 +116,12 @@ fn main() -> std::process::ExitCode {
         return claude::hooks::relay(&argv[2], std::path::Path::new(&argv[3]), &argv[4]);
     }
 
+    // SessionStart context hook: `claudio __context latex` prints the
+    // instruction to wrap math in `<latex>` tags (`[claude] latex`).
+    if argv.get(1).map(String::as_str) == Some("__context") {
+        return claude::hooks::context(argv.get(2).map(String::as_str));
+    }
+
     // Legacy relay mode, invoked as `claudio __hook <Event>`.
     if argv.len() >= 3 && argv[1] == "__hook" {
         print::hooks::run_relay(&argv[2]);
`````
Run: `umask 022; cargo test --locked --bin claudio -- config:: claude::hooks daemon::session && make test-unit && rustfmt --edition 2021 --check src/config.rs src/claude/hooks.rs && cargo build --locked && echo '{}' | target/debug/claudio __context latex && echo '{}' | target/debug/claudio __context other; echo "exit=$?"`
Expected:
- `test result: ok. 27 passed; 0 failed; 0 ignored; 0 measured; 760 filtered out`
- `test result: ok. 787 passed; 0 failed` (the base has 783);
- no rustfmt output;
- one line starting `{"hookSpecificOutput":{"additionalContext":"This terminal renders math written as LaTeX inside <latex>…</latex> tags.`;
- nothing for `other`, then `exit=0`.

Step 3: commit
`git commit -m "feat(daemon): tell claude to wrap math in <latex> tags when [claude] latex is on" -- src/config.rs src/claude/hooks.rs src/daemon/mod.rs src/daemon/session.rs src/daemon/tests.rs src/main.rs`

Interfaces this task provides to later tasks: `claude::hooks::settings(claudio_bin: &Path, socket: &Path, token: &str, latex: bool) -> serde_json::Value`, `claude::hooks::LATEX_CONTEXT: &str`, `claude::hooks::context(kind: Option<&str>) -> ExitCode`, `config::ClaudeSection::latex: bool`, `daemon::Config::latex: bool`.

### Task 2: `sanitize`, `extract_latex` and the transcript's formula list
Model: sonnet   Repo: /volumes/repos/claudio   Files: src/claude/latex.rs (new), src/claude/mod.rs, src/claude/projects.rs
Goal: AC3. A new shared module, `claude::latex`, holds `sanitize`, `is_unsafe` and `extract_latex`; the daemon and the TUI both use it. `TranscriptCursor` keeps the last 64 formulas of a conversation's answers.

Step 1: write the failing tests. Create src/claude/latex.rs with only its test module, declare the module in src/claude/mod.rs, and add the cursor tests to src/claude/projects.rs.
`````diff
diff --git a/src/claude/latex.rs b/src/claude/latex.rs
new file mode 100644
index 0000000..28c1de4
--- /dev/null
+++ b/src/claude/latex.rs
@@ -0,0 +1,71 @@
+#[cfg(test)]
+mod tests {
+    use super::*;
+
+    #[test]
+    fn formulas_come_in_order_and_trimmed() {
+        let text = "Here <latex> E = mc^2 </latex> and <latex>a \\neq 0</latex>.";
+        assert_eq!(extract_latex(text), ["E = mc^2", "a \\neq 0"]);
+        assert!(extract_latex("no math here").is_empty());
+        // Case-sensitive, and a tag needs its closing tag.
+        assert!(extract_latex("<LATEX>x</LATEX> <latex>y").is_empty());
+    }
+
+    #[test]
+    fn a_formula_may_span_lines() {
+        let text = "<latex>\nx = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}\n  {2a}\n</latex>";
+        assert_eq!(
+            extract_latex(text),
+            ["x = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}\n  {2a}"]
+        );
+    }
+
+    #[test]
+    fn code_blocks_and_inline_code_are_not_formulas() {
+        let text = "```\n<latex>fenced</latex>\n```\n\
+                    ~~~ rust\n<latex>tilde</latex>\n~~~\n\
+                    Use `<latex>x</latex>` or ``a ` <latex>y</latex>`` to write \
+                    <latex>z</latex>; a lone ` stays text: <latex>w</latex>";
+        assert_eq!(extract_latex(text), ["z", "w"]);
+    }
+
+    #[test]
+    fn empty_and_oversized_formulas_are_dropped() {
+        let big = "x".repeat(MAX_FORMULA_BYTES + 1);
+        let max = "y".repeat(MAX_FORMULA_BYTES);
+        let text = format!("<latex>  </latex><latex>{big}</latex><latex>{max}</latex>");
+        assert_eq!(extract_latex(&text), [max]);
+    }
+
+    #[test]
+    fn sanitize_drops_controls_format_and_private_use() {
+        // A C0 escape, a C1 control, a bidi override, a zero-width space, a
+        // BOM, a soft hyphen and a private-use character.
+        let dirty = "a\u{1b}[31mb\u{9b}c\u{202e}d\u{200b}e\u{feff}f\u{ad}g\u{e000}h";
+        assert_eq!(sanitize(dirty), "a[31mbcdefgh");
+        // Newlines and tabs stay; other controls (CR) go.
+        assert_eq!(sanitize("x\r\n\ty"), "x\n\ty");
+        // Through extraction too.
+        assert_eq!(
+            extract_latex("<latex>a\u{202e}b\u{200b}c\u{7}</latex>"),
+            ["abc"]
+        );
+    }
+
+    #[test]
+    fn sanitize_keeps_combining_marks_only_after_a_base() {
+        assert_eq!(sanitize("x\u{338}"), "x\u{338}");
+        assert_eq!(
+            sanitize("x\u{302}\u{303}"),
+            "x\u{302}\u{303}",
+            "stacked marks stay"
+        );
+        assert_eq!(sanitize("\u{338}x"), "x", "a leading mark goes");
+        assert_eq!(
+            sanitize("a \u{338}b"),
+            "a b",
+            "after a space it has no base"
+        );
+        assert_eq!(sanitize("a\n\u{338}b"), "a\nb");
+    }
+}
diff --git a/src/claude/mod.rs b/src/claude/mod.rs
index 871cfa8..af51e56 100644
--- a/src/claude/mod.rs
+++ b/src/claude/mod.rs
@@ -4,6 +4,8 @@
 //!
 //! - [`hooks`] — hook event constants, settings builder, per-spawn token
 //!   generator, and the short-lived relay process.
+//! - [`latex`] — the `<latex>` formulas of claude's answers, and the
+//!   character filter they go through before they are shown.
 //! - [`projects`] — locating project directories, listing sessions, and
 //!   reading transcript metadata.
 //! - [`update`] — version parsing, release-channel lookup and the daily
@@ -12,6 +14,7 @@
 //!   [`proto::SessionEvent`]s.
 
 pub mod hooks;
+pub mod latex;
 pub mod projects;
 pub mod state;
 pub mod update;
diff --git a/src/claude/projects.rs b/src/claude/projects.rs
index 7aa1ba7..2ac84ec 100644
--- a/src/claude/projects.rs
+++ b/src/claude/projects.rs
@@ -900,6 +900,61 @@ mod tests {
         let _ = std::fs::remove_dir_all(&dir);
     }
 
+    /// An assistant record whose `message.content` is `content` (JSON).
+    fn said(content: &str) -> String {
+        format!(
+            r#"{{"type":"assistant","message":{{"id":"m","model":"claude-x","content":{content},"usage":{{"output_tokens":1}}}}}}"#
+        ) + "\n"
+    }
+
+    #[test]
+    fn cursor_collects_formulas_from_the_answer_text_only() {
+        let mut c = cursor();
+        let mut jsonl = String::new();
+        jsonl += &said(
+            r#"[{"type":"thinking","thinking":"<latex>t</latex>"},{"type":"text","text":"so <latex>a^2</latex>"},{"type":"tool_use","input":{"x":"<latex>u</latex>"}}]"#,
+        );
+        jsonl += &said(r#""a string <latex>b^2</latex>""#);
+        // A subagent's record and the user's own text are not the answer.
+        jsonl += &said(r#"[{"type":"text","text":"<latex>side</latex>"}]"#)
+            .replacen('{', r#"{"isSidechain":true,"#, 1);
+        jsonl += r#"{"type":"user","message":{"content":"<latex>user</latex>"}}"#;
+        jsonl += "\n";
+        // A record without usage is still read for formulas.
+        jsonl += r#"{"type":"assistant","message":{"content":[{"type":"text","text":"<latex>c</latex>"}]}}"#;
+        jsonl += "\n";
+        c.feed(jsonl.as_bytes());
+        assert_eq!(c.latex().iter().collect::<Vec<_>>(), ["a^2", "b^2", "c"]);
+    }
+
+    #[test]
+    fn cursor_keeps_the_last_64_formulas() {
+        let mut c = cursor();
+        for i in 0..70 {
+            c.feed(said(&format!(r#""<latex>x_{{{i}}}</latex>""#)).as_bytes());
+        }
+        assert_eq!(c.latex().len(), crate::claude::latex::MAX_FORMULAS);
+        assert_eq!(c.latex().front().map(String::as_str), Some("x_{6}"));
+        assert_eq!(c.latex().back().map(String::as_str), Some("x_{69}"));
+    }
+
+    #[test]
+    fn cursor_forgets_the_formulas_of_a_rewritten_file() {
+        let dir = std::env::temp_dir().join(format!("claudio-cursor-{}", uuid::Uuid::new_v4()));
+        std::fs::create_dir_all(&dir).unwrap();
+        let path = dir.join("t.jsonl");
+        let mut c = TranscriptCursor::new(Path::new("/x"), "sid");
+        c.path = path.clone();
+        std::fs::write(&path, said(r#""<latex>old</latex>""#)).unwrap();
+        assert!(c.advance());
+        assert_eq!(c.latex().iter().collect::<Vec<_>>(), ["old"]);
+        // A shorter file is a new one.
+        std::fs::write(&path, said(r#""<latex>n</latex>""#)).unwrap();
+        c.advance();
+        assert_eq!(c.latex().iter().collect::<Vec<_>>(), ["n"]);
+        let _ = std::fs::remove_dir_all(&dir);
+    }
+
     fn recent_project_dirs_no_panic_wrapper() {
         // This just must not panic; we don't care about the actual result.
         let _ = recent_project_dirs(10);
`````
Run: `umask 022; cargo test --locked --bin claudio -- claude::`
Expected: compilation fails with `error: could not compile `claudio` (bin "claudio" test) due to 23 previous errors; 1 warning emitted`, made of:
```
      7 error[E0425]: cannot find function `extract_latex` in this scope
      7 error[E0423]: expected function, found built-in attribute `sanitize`
      6 error[E0599]: no method named `latex` found for struct `projects::TranscriptCursor` in the current scope
      2 error[E0425]: cannot find value `MAX_FORMULA_BYTES` in this scope
      1 error[E0425]: cannot find value `MAX_FORMULAS` in module `crate::claude::latex`
```

Step 2: implement. This puts the implementation above the Step 1 test module of src/claude/latex.rs and changes the cursor.
`````diff
diff --git a/src/claude/latex.rs b/src/claude/latex.rs
index 28c1de4..93ab436 100644
--- a/src/claude/latex.rs
+++ b/src/claude/latex.rs
@@ -1,3 +1,185 @@
+//! `<latex>…</latex>` formulas in claude's answers.
+//!
+//! The daemon finds them in the transcript's assistant text
+//! ([`extract_latex`]), and both the daemon and the TUI pass every formula
+//! through [`sanitize`]: the text is attacker-influenced (prompt injection
+//! through files or tool output) and ends up in the user's terminal.
+
+/// A formula longer than this after [`sanitize`] is dropped, not truncated.
+pub const MAX_FORMULA_BYTES: usize = 2048;
+
+/// How many of a conversation's most recent formulas are kept.
+pub const MAX_FORMULAS: usize = 64;
+
+const OPEN: &[u8] = b"<latex>";
+const CLOSE: &[u8] = b"</latex>";
+
+/// `s` without what could reach the terminal as anything but text: controls
+/// (`Cc`, except `\n` and `\t`), format characters (`Cf`: bidi controls,
+/// zero-width characters, BOM, soft hyphen), private-use code points and
+/// noncharacters, and a combining mark with no base character before it.
+///
+/// Table-free: `Cf` is the list of its ranges, and only noncharacters stand
+/// for unassigned code points.
+pub fn sanitize(s: &str) -> String {
+    let mut out = String::with_capacity(s.len());
+    // Whether the last kept character can carry a combining mark.
+    let mut base = false;
+    for c in s.chars() {
+        if c == '\n' || c == '\t' {
+            out.push(c);
+            base = false;
+        } else if is_unsafe(c) {
+            continue;
+        } else if is_combining(c) {
+            if base {
+                out.push(c);
+            }
+        } else {
+            base = !c.is_whitespace();
+            out.push(c);
+        }
+    }
+    out
+}
+
+/// Whether [`sanitize`] removes `c` wherever it is: a control other than
+/// `\n` and `\t`, a format character, a private-use code point or a
+/// noncharacter. (A combining mark goes only when it has no base.)
+pub fn is_unsafe(c: char) -> bool {
+    (c.is_control() && c != '\n' && c != '\t') || is_format(c) || is_private_or_unassigned(c)
+}
+
+/// General category `Cf`.
+fn is_format(c: char) -> bool {
+    matches!(
+        c as u32,
+        0xAD | 0x600..=0x605
+            | 0x61C
+            | 0x6DD
+            | 0x70F
+            | 0x180E
+            | 0x200B..=0x200F
+            | 0x202A..=0x202E
+            | 0x2060..=0x206F
+            | 0xFEFF
+            | 0xFFF9..=0xFFFB
+            | 0x1D173..=0x1D17A
+            | 0xE0001
+            | 0xE0020..=0xE007F
+    )
+}
+
+/// Private use (`Co`) and the noncharacters among the unassigned (`Cn`).
+fn is_private_or_unassigned(c: char) -> bool {
+    let n = c as u32;
+    matches!(n, 0xE000..=0xF8FF | 0xF0000..=0x10FFFF | 0xFDD0..=0xFDEF) || n & 0xFFFE == 0xFFFE
+}
+
+/// The combining-mark (`Mn`) blocks a formula may carry: diacritics, their
+/// extensions and supplements, the marks for symbols, half marks and
+/// variation selectors.
+fn is_combining(c: char) -> bool {
+    matches!(
+        c as u32,
+        0x300..=0x36F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE00..=0xFE0F
+            | 0xFE20..=0xFE2F
+    )
+}
+
+/// The formulas of one assistant text, in order: what is between `<latex>`
+/// and the next `</latex>` (case-sensitive, across lines), trimmed and
+/// [`sanitize`]d. Tags inside fenced code blocks (a line whose trimmed start
+/// is ```` ``` ```` or `~~~` opens and closes one) and inline code spans
+/// are not formulas. Empty formulas, and ones longer than
+/// [`MAX_FORMULA_BYTES`], are dropped.
+pub fn extract_latex(text: &str) -> Vec<String> {
+    let mut out = Vec::new();
+    if !text.contains("<latex>") {
+        return out;
+    }
+    // The text with every byte of code blanked out: tags are searched for
+    // here, and cut from `text` at the same (ASCII, so char-boundary) offsets.
+    let mut visible = text.as_bytes().to_vec();
+    let mut in_fence = false;
+    let mut start = 0;
+    for line in text.split_inclusive('\n') {
+        let end = start + line.len();
+        let trimmed = line.trim_start();
+        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
+            in_fence = !in_fence;
+            visible[start..end].fill(0);
+        } else if in_fence {
+            visible[start..end].fill(0);
+        } else {
+            for (from, to) in code_spans(line.as_bytes()) {
+                visible[start + from..start + to].fill(0);
+            }
+        }
+        start = end;
+    }
+    let mut at = 0;
+    while let Some(open) = find(&visible, OPEN, at) {
+        let inner = open + OPEN.len();
+        let Some(close) = find(&visible, CLOSE, inner) else {
+            break;
+        };
+        let formula = sanitize(&text[inner..close]);
+        let formula = formula.trim();
+        if !formula.is_empty() && formula.len() <= MAX_FORMULA_BYTES {
+            out.push(formula.to_owned());
+        }
+        at = close + CLOSE.len();
+    }
+    out
+}
+
+/// The inline code spans of one line, as byte ranges including their
+/// backticks: a run of backticks up to the next run of the same length. A
+/// run with no closing run is plain text.
+fn code_spans(line: &[u8]) -> Vec<(usize, usize)> {
+    let run_at = |i: usize| line[i..].iter().take_while(|&&b| b == b'`').count();
+    let mut spans = Vec::new();
+    let mut i = 0;
+    while i < line.len() {
+        if line[i] != b'`' {
+            i += 1;
+            continue;
+        }
+        let run = run_at(i);
+        let mut j = i + run;
+        let mut closed = None;
+        while j < line.len() {
+            if line[j] == b'`' {
+                let other = run_at(j);
+                if other == run {
+                    closed = Some(j + other);
+                    break;
+                }
+                j += other;
+            } else {
+                j += 1;
+            }
+        }
+        match closed {
+            Some(end) => {
+                spans.push((i, end));
+                i = end;
+            }
+            None => i += run,
+        }
+    }
+    spans
+}
+
+fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
+    haystack
+        .get(from..)?
+        .windows(needle.len())
+        .position(|w| w == needle)
+        .map(|p| from + p)
+}
+
 #[cfg(test)]
 mod tests {
     use super::*;
diff --git a/src/claude/projects.rs b/src/claude/projects.rs
index 2ac84ec..045ca36 100644
--- a/src/claude/projects.rs
+++ b/src/claude/projects.rs
@@ -3,9 +3,11 @@
 //! All paths honour `$CLAUDE_CONFIG_DIR`; the default is `~/.claude`.
 //! Subagent transcripts (under `<uuid>/subagents/`) are ignored.
 
+use std::collections::VecDeque;
 use std::path::{Path, PathBuf};
 use std::time::UNIX_EPOCH;
 
+use crate::claude::latex::{extract_latex, MAX_FORMULAS};
 use crate::proto::ClaudeSession;
 
 // ── Constants ─────────────────────────────────────────────────────────────────
@@ -219,6 +221,9 @@ pub struct TranscriptCursor {
     /// be counted once.
     last_message_id: Option<String>,
     totals: TranscriptTotals,
+    /// The `<latex>` formulas of the conversation's answers, oldest first:
+    /// the last [`MAX_FORMULAS`].
+    latex: VecDeque<String>,
 }
 
 impl TranscriptCursor {
@@ -231,6 +236,7 @@ impl TranscriptCursor {
             partial: Vec::new(),
             last_message_id: None,
             totals: TranscriptTotals::default(),
+            latex: VecDeque::new(),
         }
     }
 
@@ -243,6 +249,11 @@ impl TranscriptCursor {
         &self.totals
     }
 
+    /// The conversation's most recent formulas, oldest first.
+    pub fn latex(&self) -> &VecDeque<String> {
+        &self.latex
+    }
+
     /// Whether the cursor has seen an assistant record.
     pub fn has_assistant(&self) -> bool {
         self.totals.model.is_some()
@@ -262,6 +273,7 @@ impl TranscriptCursor {
             self.partial.clear();
             self.last_message_id = None;
             self.totals = TranscriptTotals::default();
+            self.latex.clear();
         }
         if len == self.offset {
             return false;
@@ -307,6 +319,10 @@ impl TranscriptCursor {
         match v.get("type").and_then(|t| t.as_str()) {
             Some("assistant") => {
                 let Some(msg) = v.get("message") else { return };
+                // A subagent's work is not the answer on screen.
+                if v.get("isSidechain").and_then(|s| s.as_bool()) != Some(true) {
+                    push_latex(&mut self.latex, msg.get("content"));
+                }
                 let Some(usage) = msg.get("usage") else { return };
                 let count = |key: &str| usage.get(key).and_then(|v| v.as_u64()).unwrap_or(0);
                 if let Some(model) = msg.get("model").and_then(|m| m.as_str()) {
@@ -337,6 +353,33 @@ impl TranscriptCursor {
     }
 }
 
+/// Append the formulas of an assistant message to `latex`, keeping the last
+/// [`MAX_FORMULAS`]. `content` is a string or an array of blocks, of which
+/// only `text` blocks are the answer (not `thinking` or `tool_use`).
+fn push_latex(latex: &mut VecDeque<String>, content: Option<&serde_json::Value>) {
+    let mut add = |text: &str| {
+        for formula in extract_latex(text) {
+            if latex.len() == MAX_FORMULAS {
+                latex.pop_front();
+            }
+            latex.push_back(formula);
+        }
+    };
+    match content {
+        Some(serde_json::Value::String(text)) => add(text),
+        Some(serde_json::Value::Array(blocks)) => {
+            for block in blocks {
+                if block.get("type").and_then(|t| t.as_str()) == Some("text") {
+                    if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
+                        add(text);
+                    }
+                }
+            }
+        }
+        _ => {}
+    }
+}
+
 /// Check whether any line in `content` (tail bytes) has `"cwd": "<expected>"`.
 fn verify_cwd_in_tail(content: &str, expected_cwd: &str) -> bool {
     for line in content.lines() {
`````
Run: `umask 022; cargo test --locked --bin claudio -- claude:: && make test-unit && rustfmt --edition 2021 --check src/claude/latex.rs`
Expected: `test result: ok. 57 passed; 0 failed; 0 ignored; 0 measured; 739 filtered out`, then `test result: ok. 796 passed; 0 failed` (after Task 1), and no rustfmt output.

Step 3: commit
`git add src/claude/latex.rs && git commit -m "feat(daemon): collect the <latex> formulas of a conversation from its transcript" -- src/claude/latex.rs src/claude/mod.rs src/claude/projects.rs`

Interfaces this task provides to later tasks: `crate::claude::latex::{sanitize(&str) -> String, is_unsafe(char) -> bool, extract_latex(&str) -> Vec<String>, MAX_FORMULA_BYTES = 2048, MAX_FORMULAS = 64}`, `TranscriptCursor::latex(&self) -> &VecDeque<String>`.

### Task 3: the `Latex { seq, formulas }` event, the refresher's slot and the attach replay
Model: sonnet   Repo: /volumes/repos/claudio   Files: src/proto.rs, src/daemon/session.rs, src/tui/interaction.rs
Goal: AC5 and the daemon half of AC4. After each refresh, `MetaRefresher` compares the cursor's list with the last one it sent. When they differ it stores the new list under its lock with the next `seq` and broadcasts it. A new conversation (a new cursor) sends its empty list under the next `seq`; `seq` never restarts while the actor lives (ruling R15 — apply it on top of this task's diffs). `Actor::attach` queues the last list right after `Attached` + snapshot. The TUI ignores the event until Task 5 (a placeholder match arm).

Step 1: write the failing tests.
`````diff
diff --git a/src/daemon/session.rs b/src/daemon/session.rs
index 27d2bf9..3761083 100644
--- a/src/daemon/session.rs
+++ b/src/daemon/session.rs
@@ -1124,6 +1124,68 @@ mod tests {
         assert_eq!(start(&latex)[1]["command"], "'/bin/claudio' __context latex");
     }
 
+    fn formulas(list: &[&str]) -> VecDeque<String> {
+        list.iter().map(|f| f.to_string()).collect()
+    }
+
+    fn latex(seq: u64, list: &[&str]) -> Option<SessionEvent> {
+        let formulas = list.iter().map(|f| f.to_string()).collect();
+        Some(SessionEvent::Latex { seq, formulas })
+    }
+
+    #[test]
+    fn formulas_are_sent_when_they_change_with_a_growing_seq() {
+        let mut sent = None;
+        assert_eq!(latex_event(&mut sent, &formulas(&[]), true), None, "nothing yet");
+        assert_eq!(latex_event(&mut sent, &formulas(&["a"]), false), latex(1, &["a"]));
+        assert_eq!(latex_event(&mut sent, &formulas(&["a"]), false), None, "unchanged");
+        let both = formulas(&["a", "b"]);
+        assert_eq!(latex_event(&mut sent, &both, false), latex(2, &["a", "b"]));
+        // A rewritten transcript without them: the empty list goes out.
+        assert_eq!(latex_event(&mut sent, &formulas(&[]), false), latex(3, &[]));
+        assert_eq!(latex_event(&mut sent, &formulas(&["c"]), false), latex(4, &["c"]));
+        // A new conversation starts over (clients reset on its ClaudeSession).
+        assert_eq!(latex_event(&mut sent, &formulas(&[]), true), None);
+        assert_eq!(sent, None);
+        assert_eq!(latex_event(&mut sent, &formulas(&["d"]), false), latex(1, &["d"]));
+    }
+
+    #[test]
+    fn an_attach_gets_the_last_formulas_right_after_the_snapshot() {
+        let id = uuid::Uuid::new_v4();
+        let mut sent = None;
+        latex_event(&mut sent, &formulas(&["E = mc^2"]), false);
+        let (queue, mut rx) = mpsc::channel(8);
+        let mut sub = Subscriber {
+            queue,
+            lagging: false,
+            pending_req: Some(3),
+        };
+        assert!(sub.attach(id, (24, 80), b"SNAP", last_latex(&sent)));
+        let attached = Envelope {
+            req: Some(3),
+            msg: Msg::Attached {
+                id,
+                rows: 24,
+                cols: 80,
+            },
+        };
+        assert_eq!(rx.try_recv().unwrap(), Frame::Control(attached));
+        let snapshot = Frame::Data {
+            session: id,
+            bytes: b"SNAP".to_vec(),
+        };
+        assert_eq!(rx.try_recv().unwrap(), snapshot);
+        let event = latex(1, &["E = mc^2"]).unwrap();
+        let replay = Frame::Control(Envelope::event(Msg::Event { id, event }));
+        assert_eq!(rx.try_recv().unwrap(), replay);
+        // Nothing sent yet: nothing replayed.
+        assert!(sub.attach(id, (24, 80), b"SNAP", last_latex(&None)));
+        assert!(matches!(rx.try_recv(), Ok(Frame::Control(_))));
+        assert!(matches!(rx.try_recv(), Ok(Frame::Data { .. })));
+        assert!(rx.try_recv().is_err());
+    }
+
     const ALLOW: &str = "--allow-dangerously-skip-permissions";
 
     fn argv_of(cmd: &CommandBuilder) -> Vec<String> {
diff --git a/src/proto.rs b/src/proto.rs
index 6cbbaaa..ce924f5 100644
--- a/src/proto.rs
+++ b/src/proto.rs
@@ -1087,6 +1087,28 @@ mod tests {
         assert_eq!(back, SessionEvent::ClaudeSessionCleared);
     }
 
+    #[test]
+    fn latex_round_trips_and_is_unknown_to_older_clients() {
+        let event = SessionEvent::Latex {
+            seq: 7,
+            formulas: vec!["E = mc^2".into(), "a \\neq 0".into()],
+        };
+        let json = serde_json::to_string(&event).unwrap();
+        assert_eq!(
+            json,
+            r#"{"kind":"latex","seq":7,"formulas":["E = mc^2","a \\neq 0"]}"#
+        );
+        assert_eq!(serde_json::from_str::<SessionEvent>(&json).unwrap(), event);
+        #[derive(Debug, PartialEq, Deserialize)]
+        #[serde(tag = "kind", rename_all = "snake_case")]
+        enum Older {
+            Removed,
+            #[serde(other)]
+            Unknown,
+        }
+        assert_eq!(serde_json::from_str::<Older>(&json).unwrap(), Older::Unknown);
+    }
+
     #[test]
     fn meta_git_and_conversation_fields_are_optional_both_ways() {
         // An old daemon's Meta: only the three original fields.
`````
Run: `umask 022; cargo test --locked --bin claudio -- proto:: daemon::session`
Expected: compilation fails with `error: could not compile `claudio` (bin "claudio" test) due to 16 previous errors`, made of:
```
      9 error[E0425]: cannot find function `latex_event` in this scope
      2 error[E0599]: no variant named `Latex` found for enum `proto::SessionEvent`
      2 error[E0599]: no method named `attach` found for struct `daemon::session::Subscriber` in the current scope
      2 error[E0425]: cannot find function `last_latex` in this scope
      1 error[E0425]: cannot find type `VecDeque` in this scope
```

Step 2: implement.
`````diff
diff --git a/src/daemon/session.rs b/src/daemon/session.rs
index 3761083..2ce8643 100644
--- a/src/daemon/session.rs
+++ b/src/daemon/session.rs
@@ -15,7 +15,7 @@
 //! `Attached` + snapshot instead of the bytes it missed. A slow client
 //! therefore never blocks the PTY.
 
-use std::collections::HashMap;
+use std::collections::{HashMap, VecDeque};
 use std::ffi::OsStr;
 use std::io::{self, Read, Write};
 use std::path::{Path, PathBuf};
@@ -456,6 +456,32 @@ impl Subscriber {
         true
     }
 
+    /// [`Subscriber::sync`], then `latex` (the last formulas sent, if any):
+    /// formulas are broadcast only when they change, so a client attaching
+    /// now would not see them otherwise. Returns `false` if the client is
+    /// gone.
+    fn attach(
+        &mut self,
+        id: SessionId,
+        size: (u16, u16),
+        snapshot: &[u8],
+        latex: Option<SessionEvent>,
+    ) -> bool {
+        if !self.sync(id, size, snapshot) {
+            return false;
+        }
+        let Some(event) = latex else { return true };
+        let frame = Frame::Control(Envelope::event(Msg::Event { id, event }));
+        match self.queue.try_send(frame) {
+            Ok(()) => true,
+            Err(TrySendError::Full(_)) => {
+                self.lagging = true;
+                true
+            }
+            Err(TrySendError::Closed(_)) => false,
+        }
+    }
+
     /// A lagging subscriber is ready for a resync.
     fn can_resync(&self) -> bool {
         self.lagging && self.queue.capacity() > RESYNC_ROOM
@@ -497,10 +523,15 @@ struct MetaRefresher {
     kind: SessionKind,
     /// The transcript cursor, held by the running refresh.
     cursor: Arc<std::sync::Mutex<Option<crate::claude::projects::TranscriptCursor>>>,
+    /// The formulas last sent to clients, replayed to a client that attaches.
+    latex: Arc<std::sync::Mutex<LatexSent>>,
     busy: Arc<std::sync::atomic::AtomicBool>,
     started_at: Option<std::time::Instant>,
 }
 
+/// The last `SessionEvent::Latex` sent: its `seq` and formulas.
+type LatexSent = Option<(u64, Vec<String>)>;
+
 impl MetaRefresher {
     fn new(id: SessionId, daemon: Arc<Daemon>, cwd: PathBuf, kind: SessionKind) -> Self {
         Self {
@@ -509,11 +540,17 @@ impl MetaRefresher {
             cwd,
             kind,
             cursor: Arc::default(),
+            latex: Arc::default(),
             busy: Arc::default(),
             started_at: None,
         }
     }
 
+    /// The last formulas sent, as the event a client attaching now needs.
+    fn last_latex(&self) -> Option<SessionEvent> {
+        last_latex(&self.latex.lock().unwrap_or_else(|e| e.into_inner()))
+    }
+
     /// Start a refresh unless one is running or the last one started less
     /// than `min_gap` ago. `session_id` is the current claude conversation.
     fn refresh(&mut self, session_id: Option<&str>, min_gap: Duration) {
@@ -527,36 +564,75 @@ impl MetaRefresher {
         self.started_at = Some(std::time::Instant::now());
         let (id, daemon, cwd, kind) = (self.id, Arc::clone(&self.daemon), self.cwd.clone(), self.kind);
         let (cursor, busy) = (Arc::clone(&self.cursor), Arc::clone(&self.busy));
+        let latex = Arc::clone(&self.latex);
         let session_id = session_id.map(str::to_owned);
         tokio::spawn(async move {
             let git = super::git_status::read(&cwd).await;
-            let transcript = match (kind, session_id) {
+            let (transcript, formulas) = match (kind, session_id) {
                 (SessionKind::Claude, Some(sid)) => {
                     let cwd = cwd.clone();
                     tokio::task::spawn_blocking(move || {
                         let mut slot = cursor.lock().unwrap_or_else(|e| e.into_inner());
-                        let cursor = slot
-                            .take()
-                            .filter(|c| c.session_id() == sid)
-                            .unwrap_or_else(|| {
-                                crate::claude::projects::TranscriptCursor::new(&cwd, &sid)
-                            });
-                        let cursor = slot.insert(cursor);
+                        let same = slot.take().filter(|c| c.session_id() == sid);
+                        let new_cursor = same.is_none();
+                        let cursor = slot.insert(same.unwrap_or_else(|| {
+                            crate::claude::projects::TranscriptCursor::new(&cwd, &sid)
+                        }));
                         cursor.advance();
-                        cursor.has_assistant().then(|| cursor.totals().clone())
+                        let mut sent = latex.lock().unwrap_or_else(|e| e.into_inner());
+                        let formulas = latex_event(&mut sent, cursor.latex(), new_cursor);
+                        let totals = cursor.has_assistant().then(|| cursor.totals().clone());
+                        (totals, formulas)
                     })
                     .await
-                    .ok()
-                    .flatten()
+                    .unwrap_or_default()
                 }
-                _ => None,
+                _ => (None, None),
             };
             daemon.broadcast(id, meta_event(&cwd, git, transcript));
+            if let Some(formulas) = formulas {
+                daemon.broadcast(id, formulas);
+            }
             busy.store(false, Ordering::Release);
         });
     }
 }
 
+/// The `Latex` event a refresh owes clients, recorded in `sent`: `None` when
+/// `latex` is the list last sent. Each change takes the next `seq`; a new
+/// transcript cursor (a new conversation, whose `ClaudeSession` made clients
+/// drop theirs) starts over, so its first list is seq 1.
+fn latex_event(
+    sent: &mut LatexSent,
+    latex: &VecDeque<String>,
+    new_cursor: bool,
+) -> Option<SessionEvent> {
+    if new_cursor {
+        *sent = None;
+    }
+    let (seq, unchanged) = match sent {
+        Some((seq, list)) => (*seq, list.iter().eq(latex.iter())),
+        None => (0, latex.is_empty()),
+    };
+    if unchanged {
+        return None;
+    }
+    let formulas: Vec<String> = latex.iter().cloned().collect();
+    *sent = Some((seq + 1, formulas.clone()));
+    Some(SessionEvent::Latex {
+        seq: seq + 1,
+        formulas,
+    })
+}
+
+/// The event that replays `sent` to a client that attaches.
+fn last_latex(sent: &LatexSent) -> Option<SessionEvent> {
+    sent.as_ref().map(|(seq, formulas)| SessionEvent::Latex {
+        seq: *seq,
+        formulas: formulas.clone(),
+    })
+}
+
 /// The `Meta` event for a refresh's findings. The branch falls back to
 /// `.git/HEAD` when git itself could not answer.
 fn meta_event(
@@ -777,7 +853,8 @@ impl Actor {
             lagging: false,
             pending_req: req,
         };
-        if sub.sync(self.id, self.screen.size(), &self.screen.snapshot()) {
+        let latex = self.meta.last_latex();
+        if sub.attach(self.id, self.screen.size(), &self.screen.snapshot(), latex) {
             self.subs.insert(client, sub);
         }
     }
diff --git a/src/proto.rs b/src/proto.rs
index ce924f5..0586cc1 100644
--- a/src/proto.rs
+++ b/src/proto.rs
@@ -539,6 +539,15 @@ pub enum SessionEvent {
         #[serde(default, skip_serializing_if = "Option::is_none")]
         turns: Option<u32>,
     },
+    /// The conversation's recent `<latex>` formulas, oldest first: always
+    /// the whole list, which replaces the client's. `seq` grows with every
+    /// change (a new conversation starts over at 1), so a client keeps the
+    /// newer of an attach replay and a broadcast that cross. Old clients map
+    /// this to `Unknown`.
+    Latex {
+        seq: u64,
+        formulas: Vec<String>,
+    },
     /// Catch-all for event kinds this client doesn't recognise yet.
     /// Keeps older clients alive when the daemon sends a newer event kind.
     #[serde(other)]
diff --git a/src/tui/interaction.rs b/src/tui/interaction.rs
index 36fc2bf..5cae81e 100644
--- a/src/tui/interaction.rs
+++ b/src/tui/interaction.rs
@@ -721,6 +721,8 @@ impl App {
                     v.turns = turns;
                 }
             }
+            // Rendered and drawn by the formulas overlay (a later change).
+            SessionEvent::Latex { .. } => {}
             SessionEvent::Unknown => {}
         }
     }
`````
Run: `umask 022; cargo test --locked --bin claudio -- proto:: daemon::session && make test-unit`
Expected: `test result: ok. 31 passed; 0 failed; 0 ignored; 0 measured; 768 filtered out`, then `test result: ok. 799 passed; 0 failed`.

Step 3: commit
`git commit -m "feat(daemon): send a session's formulas to clients as a seq-numbered Latex event" -- src/proto.rs src/daemon/session.rs src/tui/interaction.rs`

Interfaces this task provides to later tasks: `SessionEvent::Latex { seq: u64, formulas: Vec<String> }` (serialized `{"kind":"latex","seq":…,"formulas":[…]}`); in src/tui/interaction.rs the arm `// Rendered and drawn by the formulas overlay (a later change).` / `SessionEvent::Latex { .. } => {}`, which Task 5 replaces.

### Task 4: `tui/latex.rs`: the pre-check, the 2D render and the one-line walk
Model: sonnet   Repo: /volumes/repos/claudio   Files: Cargo.toml, Cargo.lock, src/tui/latex.rs (new), src/tui/mod.rs
Goal: the formula table of AC6. Adds the pure-Rust dependencies term-maths =1.0.0 (no default features) and rust-latex-parser =0.1.1, and makes the already-locked unicode-segmentation and unicode-width direct dependencies. `Formula` carries the source, the two renderings and the three match keys. `too_complex` turns away formulas deeper than 32 braces or with more than 256 recursion-prone tokens. `render` returns the term-maths block (≤ 200×40) and a one-line form (≤ 512 columns), both sanitized. Nothing calls this yet: `mod latex;` carries a temporary `#[allow(dead_code)]`, which Task 5 removes.

Step 1: write the failing tests. Create src/tui/latex.rs with only its test module, and declare the module.
`````diff
diff --git a/src/tui/latex.rs b/src/tui/latex.rs
new file mode 100644
index 0000000..f102b92
--- /dev/null
+++ b/src/tui/latex.rs
@@ -0,0 +1,102 @@
+#[cfg(test)]
+mod tests {
+    use super::*;
+
+    /// The one-line rendering of `src`.
+    fn inline(src: &str) -> String {
+        render(&prepare(src))
+            .0
+            .unwrap_or_else(|| panic!("no inline for {src}"))
+    }
+
+    #[test]
+    fn formulas_render_to_one_line() {
+        let table = [
+            ("E = mc^2", "E = mc²"),
+            (
+                "x = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}{2a}",
+                "x = (-b ± √(b² - 4ac))/(2a)",
+            ),
+            (
+                "\\sum_{i=1}^{n} i = \\frac{n(n+1)}{2}",
+                "∑ᵢ₌₁ⁿ i = n(n + 1)/2",
+            ),
+            ("\\int_0^1 x^2\\,dx", "∫₀¹ x² dx"),
+            ("a \\neq 0", "a ≠ 0"),
+            (
+                "\\begin{pmatrix} a & b \\\\ c & d \\end{pmatrix}",
+                "[a b; c d]",
+            ),
+            (
+                "f(x) = \\begin{cases} 1 & x > 0 \\\\ 0 & \\text{otherwise} \\end{cases}",
+                "f(x) = { 1 if x > 0; 0 otherwise }",
+            ),
+            (
+                "\\begin{vmatrix} a & b \\\\ c & d \\end{vmatrix}",
+                "|a b; c d|",
+            ),
+            ("\\mathbb{R}^n", "ℝⁿ"),
+            ("\\frac{1}{\\sqrt{x}}", "1/√x"),
+            ("\\binom{n}{k}", "(n choose k)"),
+            ("e^{i\\pi} + 1 = 0", "e^(iπ) + 1 = 0"),
+            ("\\hat{x} + \\vec{v}", "x\u{302} + v\u{20d7}"),
+        ];
+        for (src, want) in table {
+            assert_eq!(inline(src), want, "{src}");
+        }
+    }
+
+    #[test]
+    fn the_block_is_term_maths_rendering() {
+        let (_, block) = render(&prepare("\\frac{a}{b}"));
+        assert_eq!(block.unwrap(), [" a", "───", " b"]);
+        let (inline, block) = render(&prepare("E = mc^2"));
+        assert_eq!(block.unwrap(), ["E = mc²"]);
+        assert_eq!(
+            inline.as_deref(),
+            Some("E = mc²"),
+            "one row is its own inline"
+        );
+    }
+
+    #[test]
+    fn renderings_are_sanitized() {
+        // A bidi override and a zero-width space never reach a cell.
+        let src = prepare("a\u{202e}b\u{200b} + c");
+        assert_eq!(src, "ab + c");
+        let (inline, block) = render("\\text{a\u{202e}b}");
+        assert_eq!(inline.as_deref(), Some("ab"));
+        assert_eq!(block.unwrap(), ["ab"]);
+        assert_eq!(prepare("a\nb\tc"), "a b c");
+    }
+
+    #[test]
+    fn the_pre_check_turns_away_deep_and_long_formulas() {
+        assert!(!too_complex("x = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}{2a}"));
+        assert!(!too_complex(&"{".repeat(MAX_DEPTH)));
+        assert!(too_complex(&"{".repeat(MAX_DEPTH + 1)));
+        // Recursion without braces: commands, slashes, bareword calls.
+        assert!(!too_complex(&"\\frac ".repeat(MAX_TOKENS)));
+        assert!(too_complex(&"\\frac ".repeat(MAX_TOKENS + 1)));
+        assert!(too_complex(&"\\not".repeat(MAX_TOKENS + 1)));
+        assert!(too_complex(&"x/".repeat(MAX_TOKENS + 1)));
+        assert!(too_complex(&"sqrt(".repeat(MAX_TOKENS + 1)));
+        assert!(
+            !too_complex(&"(x)".repeat(MAX_TOKENS + 1)),
+            "plain parentheses"
+        );
+        assert!(too_complex(&"x".repeat(MAX_FORMULA_BYTES + 1)));
+    }
+
+    #[test]
+    fn keys_undo_what_markdown_does_on_screen() {
+        let f = Formula::raw("\\{a_i\\} \\cup \\{b_i\\} \\\\ x^2\\,dx");
+        // Escapes lose their backslash, whitespace goes.
+        assert_eq!(f.key_strict, "{a_i}\\cup{b_i}\\x^2,dx");
+        // Emphasis characters go too.
+        assert_eq!(f.key_loose, "{ai}\\cup{bi}\\x^2,dx");
+        let f = Formula::raw("x <b> y </b> < z");
+        assert_eq!(f.key_html, "xy<z");
+        assert_eq!(f.key_loose, "x<b>y</b><z");
+    }
+}
diff --git a/src/tui/mod.rs b/src/tui/mod.rs
index a60e9f3..bad0fca 100644
--- a/src/tui/mod.rs
+++ b/src/tui/mod.rs
@@ -13,6 +13,9 @@ mod git_app;
 mod git_view;
 mod interaction;
 mod keymap;
+// Its first user, the render worker, comes with the next change.
+#[allow(dead_code)]
+mod latex;
 mod notifications;
 mod plain;
 mod proxy_state;
`````
Run: `umask 022; cargo test --locked --bin claudio tui::latex`
Expected: compilation fails with `error: could not compile `claudio` (bin "claudio" test) due to 35 previous errors; 1 warning emitted`, made of:
```
     10 error[E0425]: cannot find function `too_complex` in this scope
      6 error[E0425]: cannot find value `MAX_TOKENS` in this scope
      5 error[E0425]: cannot find function `prepare` in this scope
      5 error[E0282]: type annotations needed
      4 error[E0425]: cannot find function `render` in this scope
      2 error[E0433]: failed to resolve: use of undeclared type `Formula`
      2 error[E0425]: cannot find value `MAX_DEPTH` in this scope
      1 error[E0425]: cannot find value `MAX_FORMULA_BYTES` in this scope
```

Step 2: implement. This changes the manifest and adds exactly these 21 lines to Cargo.lock (no other lock change). Then it puts the implementation above the Step 1 test module.
`````diff
diff --git a/Cargo.lock b/Cargo.lock
index 220eb7e..a8cbfe9 100644
--- a/Cargo.lock
+++ b/Cargo.lock
@@ -259,9 +259,11 @@ dependencies = [
  "portable-pty",
  "ratatui",
  "reqwest",
+ "rust-latex-parser",
  "serde",
  "serde_json",
  "sha2",
+ "term-maths",
  "thiserror 2.0.18",
  "tokio",
  "tokio-stream",
@@ -270,6 +272,8 @@ dependencies = [
  "tower-http",
  "tracing",
  "tracing-subscriber",
+ "unicode-segmentation",
+ "unicode-width",
  "uuid",
  "vte 0.13.1",
 ]
@@ -1948,6 +1952,12 @@ dependencies = [
  "windows-sys 0.52.0",
 ]
 
+[[package]]
+name = "rust-latex-parser"
+version = "0.1.1"
+source = "registry+https://github.com/rust-lang/crates.io-index"
+checksum = "4d8acd59feae81354fa3e31bb843418e76d83c49941f417ae5fbaa8a6c97fac5"
+
 [[package]]
 name = "rustc-hash"
 version = "2.1.3"
@@ -2372,6 +2382,17 @@ dependencies = [
  "syn 3.0.6",
 ]
 
+[[package]]
+name = "term-maths"
+version = "1.0.0"
+source = "registry+https://github.com/rust-lang/crates.io-index"
+checksum = "26ccccf7b1e429ef47ef4a82726346feefdb67169e6a9d8d97fc6d3707640a05"
+dependencies = [
+ "rust-latex-parser",
+ "unicode-segmentation",
+ "unicode-width",
+]
+
 [[package]]
 name = "termina"
 version = "0.3.3"
diff --git a/Cargo.toml b/Cargo.toml
index a733dc7..1c92cff 100644
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -51,6 +51,13 @@ libc = "0.2"
 toml = "0.9"
 sha2 = "0.10"
 reqwest = { version = "0.12", default-features = false, features = ["rustls-tls", "json"] }
+# `<latex>` formulas in the session pane, rendered as Unicode text. Pure Rust;
+# term-maths' ratatui and crossterm features stay off. Pinned exactly: the
+# render guard's limits (tui/latex.rs) were measured against these versions.
+term-maths = { version = "=1.0.0", default-features = false }
+rust-latex-parser = "=0.1.1"
+unicode-segmentation = "1"
+unicode-width = "0.2"
 
 # Only needed in integration tests (no runtime dep).
 [dev-dependencies]
diff --git a/src/tui/latex.rs b/src/tui/latex.rs
index f102b92..1972bc9 100644
--- a/src/tui/latex.rs
+++ b/src/tui/latex.rs
@@ -1,3 +1,471 @@
+//! `<latex>` formulas: rendering them as Unicode text.
+//!
+//! Pure functions only. The render itself ([`render`]) parses and lays out
+//! attacker-influenced text with a parser that is slow and deep on hostile
+//! input, so it never runs on the UI thread: `tui/mod.rs` calls it from its
+//! `latex-render` worker, on a scratch thread with a time budget, after
+//! [`too_complex`] has turned away what could overflow that thread's stack.
+//!
+//! - [`Formula`] — one formula's source, its renderings and matching keys.
+//! - [`render`] — the 2D block (term-maths) and the one-line form.
+//! - [`too_complex`] — the pre-check that runs before any parsing.
+
+use rust_latex_parser::{AccentKind, EqNode, MatrixKind};
+use unicode_width::UnicodeWidthStr;
+
+use crate::claude::latex::{sanitize, MAX_FORMULA_BYTES};
+
+/// Deepest `{`…`}` nesting a formula may have to be rendered.
+const MAX_DEPTH: usize = 32;
+/// Most recursion-prone tokens a formula may have to be rendered: every
+/// `\command` (`\left`, `\begin` included), every `/`, and every `(` right
+/// after a bareword (`sqrt(`, `frac(`). The quadratic formula has about 12.
+const MAX_TOKENS: usize = 256;
+/// The largest 2D rendering shown in the formulas popup.
+const BLOCK_MAX_COLS: usize = 200;
+const BLOCK_MAX_ROWS: usize = 40;
+/// The widest one-line rendering put in the pane.
+const INLINE_MAX_COLS: usize = 512;
+
+/// One `<latex>` formula as the client shows it.
+#[derive(Debug, Clone, PartialEq)]
+pub struct Formula {
+    /// The LaTeX, as the daemon sent it.
+    pub src: String,
+    /// The one-line rendering that replaces the tag in the pane; `None`
+    /// leaves the tag raw.
+    pub inline: Option<String>,
+    /// The 2D rendering shown in the formulas popup, one string per row.
+    pub block: Option<Vec<String>>,
+    /// The matching keys of the on-screen tag text (see `overlay`):
+    /// whitespace dropped, after undoing markdown's backslash escapes...
+    pub key_strict: String,
+    /// ...and without the characters markdown takes as emphasis...
+    pub key_loose: String,
+    /// ...and without what markdown may take for an HTML tag (`<b>`).
+    pub key_html: String,
+}
+
+impl Formula {
+    /// `src` with its renderings (see [`render`]); the keys come from `src`.
+    pub fn new(src: &str, (inline, block): Rendered) -> Formula {
+        let unescaped = md_unescape(src);
+        Formula {
+            src: src.to_owned(),
+            inline,
+            block,
+            key_strict: normalize(&unescaped),
+            key_loose: normalize_loose(&unescaped),
+            key_html: normalize_loose(&without_html(&unescaped)),
+        }
+    }
+
+    /// `src` shown as it is: not rendered (too complex, too slow, or the
+    /// renderer failed).
+    pub fn raw(src: &str) -> Formula {
+        Formula::new(src, (None, None))
+    }
+}
+
+/// What [`render`] makes of a formula: its one-line and 2D renderings.
+pub type Rendered = (Option<String>, Option<Vec<String>>);
+
+/// `src` as the renderer gets it: [`sanitize`]d, on one line.
+pub fn prepare(src: &str) -> String {
+    sanitize(src).replace(['\n', '\t'], " ")
+}
+
+/// Whether `src` must not be parsed at all: longer than a formula may be,
+/// nested deeper than [`MAX_DEPTH`] braces, or with more than [`MAX_TOKENS`]
+/// tokens that make the parser recurse (it recurses per argument, not per
+/// brace: `\frac \frac \frac …` nests without any `{`).
+pub fn too_complex(src: &str) -> bool {
+    if src.len() > MAX_FORMULA_BYTES {
+        return true;
+    }
+    let (mut depth, mut deepest, mut tokens) = (0usize, 0usize, 0usize);
+    let mut after_word = false;
+    let mut chars = src.chars().peekable();
+    while let Some(c) = chars.next() {
+        let mut word = false;
+        match c {
+            '{' => {
+                depth += 1;
+                deepest = deepest.max(depth);
+            }
+            '}' => depth = depth.saturating_sub(1),
+            '\\' => {
+                let mut letters = 0;
+                while chars.next_if(char::is_ascii_alphabetic).is_some() {
+                    letters += 1;
+                }
+                if letters > 0 {
+                    tokens += 1;
+                }
+            }
+            '/' => tokens += 1,
+            '(' if after_word => tokens += 1,
+            c if c.is_ascii_alphabetic() => word = true,
+            _ => {}
+        }
+        after_word = word;
+    }
+    deepest > MAX_DEPTH || tokens > MAX_TOKENS
+}
+
+/// Render a [`prepare`]d formula: the 2D block term-maths lays out (at most
+/// [`BLOCK_MAX_COLS`] × [`BLOCK_MAX_ROWS`]), and the one-line form, which is
+/// that block when it is one row and otherwise our own one-line walk of the
+/// parse tree (at most [`INLINE_MAX_COLS`] wide). Either is `None` when it
+/// does not fit or comes out empty. Both are [`sanitize`]d.
+///
+/// May be slow, deep or panic on hostile input: call it only through the
+/// guard in `tui/mod.rs`, after [`too_complex`].
+pub fn render(src: &str) -> Rendered {
+    let tree = rust_latex_parser::parse_equation(src);
+    let laid_out = term_maths::layout::layout(&tree);
+    let rows: Vec<String> = laid_out
+        .cells()
+        .iter()
+        .map(|row| sanitize(&row.concat()).trim_end().to_owned())
+        .collect();
+    let line = match rows.as_slice() {
+        [one] => one.trim().to_owned(),
+        _ => sanitize(&one_line(&tree)).trim().to_owned(),
+    };
+    let inline = (!line.is_empty() && line.width() <= INLINE_MAX_COLS).then_some(line);
+    let fits = laid_out.width() <= BLOCK_MAX_COLS && rows.len() <= BLOCK_MAX_ROWS;
+    let block = (fits && rows.iter().any(|r| !r.trim().is_empty())).then_some(rows);
+    (inline, block)
+}
+
+// ── One-line walk ─────────────────────────────────────────────────────────────
+
+/// One line of text for `node`, modelled on term-maths' layout: fractions
+/// become `a/b`, scripts Unicode super- and subscripts (else `^(…)` and
+/// `_(…)`), roots `√x`, matrices `[a b; c d]`, cases `{ 1 if x > 0; 0
+/// otherwise }`. Symbols are already Unicode in the parse tree.
+fn one_line(node: &EqNode) -> String {
+    match node {
+        EqNode::Text(s) => s.clone(),
+        EqNode::Space(points) => {
+            if *points < 2.0 {
+                String::new()
+            } else {
+                " ".to_owned()
+            }
+        }
+        EqNode::Seq(children) => seq(children),
+        EqNode::Frac(num, den) => {
+            let (num, den) = (one_line(num), one_line(den));
+            let num = if top_level(&num, |c| c.is_whitespace() || c == '/') {
+                format!("({num})")
+            } else {
+                num
+            };
+            format!("{num}/{}", operand(den))
+        }
+        EqNode::Sup(base, sup) => format!("{}{}", operand(one_line(base)), superscript(sup)),
+        EqNode::Sub(base, sub) => format!("{}{}", operand(one_line(base)), subscript(sub)),
+        EqNode::SupSub(base, sup, sub) => format!(
+            "{}{}{}",
+            operand(one_line(base)),
+            subscript(sub),
+            superscript(sup)
+        ),
+        EqNode::Sqrt(body) => format!("√{}", operand(one_line(body))),
+        EqNode::BigOp {
+            symbol,
+            lower,
+            upper,
+        } => {
+            let lower = lower.as_deref().map(subscript).unwrap_or_default();
+            let upper = upper.as_deref().map(superscript).unwrap_or_default();
+            format!("{symbol}{lower}{upper}")
+        }
+        EqNode::Accent(body, kind) => accent(&one_line(body), *kind),
+        EqNode::Limit { name, lower } => {
+            let lower = lower.as_deref().map(subscript).unwrap_or_default();
+            format!("{name}{lower}")
+        }
+        EqNode::TextBlock(s) => s.clone(),
+        EqNode::MathFont { kind, content } => {
+            term_maths::mathfont::map_str(kind, &one_line(content))
+        }
+        EqNode::Delimited {
+            left,
+            right,
+            content,
+        } => format!("{left}{}{right}", one_line(content).trim()),
+        EqNode::Matrix { kind, rows } => {
+            let rows: Vec<String> = rows
+                .iter()
+                .map(|row| {
+                    let cells: Vec<String> =
+                        row.iter().map(|c| one_line(c).trim().to_owned()).collect();
+                    cells.join(" ")
+                })
+                .collect();
+            let (open, close) = match kind {
+                MatrixKind::VBar => ("|", "|"),
+                MatrixKind::DoubleVBar => ("‖", "‖"),
+                _ => ("[", "]"),
+            };
+            format!("{open}{}{close}", rows.join("; "))
+        }
+        EqNode::Cases { rows } => {
+            let rows: Vec<String> = rows
+                .iter()
+                .map(|(value, condition)| {
+                    let value = one_line(value).trim().to_owned();
+                    match condition.as_ref().map(|c| one_line(c).trim().to_owned()) {
+                        None => value,
+                        Some(c) if starts_with_word(&c) => format!("{value} {c}"),
+                        Some(c) => format!("{value} if {c}"),
+                    }
+                })
+                .collect();
+            format!("{{ {} }}", rows.join("; "))
+        }
+        EqNode::Binom(top, bottom) => {
+            format!(
+                "({} choose {})",
+                one_line(top).trim(),
+                one_line(bottom).trim()
+            )
+        }
+        EqNode::Brace {
+            content,
+            label,
+            over,
+        } => {
+            let label = match (label.as_deref(), over) {
+                (Some(l), true) => superscript(l),
+                (Some(l), false) => subscript(l),
+                (None, _) => String::new(),
+            };
+            format!("{}{label}", one_line(content))
+        }
+        EqNode::StackRel {
+            base,
+            annotation,
+            over,
+        } => {
+            let annotation = if *over {
+                superscript(annotation)
+            } else {
+                subscript(annotation)
+            };
+            format!("{}{annotation}", one_line(base))
+        }
+    }
+}
+
+/// A sequence: its parts side by side, every run of spaces one space, none
+/// at the ends.
+fn seq(children: &[EqNode]) -> String {
+    let mut out = String::new();
+    let mut space = false;
+    let mut parts = Vec::new();
+    flatten(children, &mut parts);
+    for part in parts {
+        let spacing = match part {
+            EqNode::Space(_) => true,
+            EqNode::Text(s) => s.chars().all(char::is_whitespace),
+            _ => false,
+        };
+        if spacing {
+            space = true;
+            continue;
+        }
+        if space && !out.is_empty() {
+            out.push(' ');
+        }
+        space = false;
+        out.push_str(&one_line(part));
+    }
+    out
+}
+
+fn flatten<'a>(children: &'a [EqNode], out: &mut Vec<&'a EqNode>) {
+    for child in children {
+        match child {
+            EqNode::Seq(inner) => flatten(inner, out),
+            other => out.push(other),
+        }
+    }
+}
+
+/// `s` as an operand: as it is when it reads as one (see [`is_atom`]),
+/// else in parentheses.
+fn operand(s: String) -> String {
+    if is_atom(&s) {
+        s
+    } else {
+        format!("({s})")
+    }
+}
+
+/// One operand without parentheses: a number, one symbol (with scripts or a
+/// root sign), or something already in brackets.
+fn is_atom(s: &str) -> bool {
+    let s = s.strip_prefix('√').unwrap_or(s);
+    let mut chars = s.chars();
+    let Some(first) = chars.next() else {
+        return true;
+    };
+    s.chars().all(|c| c.is_ascii_digit() || c == '.')
+        || chars.all(is_script_glyph)
+        || (matches!(first, '(' | '[' | '{' | '|' | '⟨')
+            && !top_level(&s[first.len_utf8()..], |_| false))
+}
+
+/// Whether `c` is one of the super- or subscript characters scripts become.
+fn is_script_glyph(c: char) -> bool {
+    "⁰¹²³⁴⁵⁶⁷⁸⁹⁺⁻⁼⁽⁾ᵃᵇᶜᵈᵉᶠᵍʰⁱʲᵏˡᵐⁿᵒᵖʳˢᵗᵘᵛʷˣʸᶻ₀₁₂₃₄₅₆₇₈₉₊₋₌₍₎ₐₑₕᵢⱼₖₗₘₙₒₚᵣₛₜᵤᵥₓ'′″".contains(c)
+}
+
+/// Whether `s` has, outside any brackets, a character `pick` accepts. A
+/// bracket that closes the outermost one before the end also counts (so
+/// `(a)(b)` is not one bracketed operand).
+fn top_level(s: &str, pick: impl Fn(char) -> bool) -> bool {
+    let mut depth = 0usize;
+    let last = s.chars().count().saturating_sub(1);
+    for (i, c) in s.chars().enumerate() {
+        match c {
+            '(' | '[' | '{' | '⟨' => depth += 1,
+            ')' | ']' | '}' | '⟩' => {
+                if depth == 0 && i < last {
+                    return true;
+                }
+                depth = depth.saturating_sub(1);
+            }
+            c if depth == 0 && pick(c) => return true,
+            _ => {}
+        }
+    }
+    false
+}
+
+/// Whether `s` starts with a word of two or more letters (`otherwise`,
+/// `if`, `for`): a cases condition that needs no `if`.
+fn starts_with_word(s: &str) -> bool {
+    s.chars().take_while(|c| c.is_alphabetic()).count() >= 2
+}
+
+fn superscript(node: &EqNode) -> String {
+    script(node, superscript_char, '^')
+}
+
+fn subscript(node: &EqNode) -> String {
+    script(node, subscript_char, '_')
+}
+
+/// A script in Unicode script characters when every character has one,
+/// else `^(…)` / `_(…)`. Spaces are dropped: `i = 1` is `ᵢ₌₁`.
+fn script(node: &EqNode, map: fn(char) -> Option<char>, mark: char) -> String {
+    let text: String = one_line(node)
+        .chars()
+        .filter(|c| !c.is_whitespace())
+        .collect();
+    match text.chars().map(map).collect::<Option<String>>() {
+        Some(mapped) if !mapped.is_empty() => mapped,
+        _ => format!("{mark}({text})"),
+    }
+}
+
+fn superscript_char(c: char) -> Option<char> {
+    let i = "0123456789+-=()abcdefghijklmnoprstuvwxyz".find(c)?;
+    "⁰¹²³⁴⁵⁶⁷⁸⁹⁺⁻⁼⁽⁾ᵃᵇᶜᵈᵉᶠᵍʰⁱʲᵏˡᵐⁿᵒᵖʳˢᵗᵘᵛʷˣʸᶻ".chars().nth(i)
+}
+
+fn subscript_char(c: char) -> Option<char> {
+    let i = "0123456789+-=()aehijklmnoprstuvx".find(c)?;
+    "₀₁₂₃₄₅₆₇₈₉₊₋₌₍₎ₐₑₕᵢⱼₖₗₘₙₒₚᵣₛₜᵤᵥₓ".chars().nth(i)
+}
+
+/// `body` with an accent: a combining mark over a one-character body, else
+/// `hat(…)` and the like.
+fn accent(body: &str, kind: AccentKind) -> String {
+    let (mark, name) = match kind {
+        AccentKind::Hat => ('\u{302}', "hat"),
+        AccentKind::Bar => ('\u{305}', "bar"),
+        AccentKind::Dot => ('\u{307}', "dot"),
+        AccentKind::DoubleDot => ('\u{308}', "ddot"),
+        AccentKind::Tilde => ('\u{303}', "tilde"),
+        AccentKind::Vec => ('\u{20d7}', "vec"),
+    };
+    let body = body.trim();
+    if body.chars().count() == 1 {
+        format!("{body}{mark}")
+    } else {
+        format!("{name}({body})")
+    }
+}
+
+// ── Matching keys ─────────────────────────────────────────────────────────────
+
+/// `s` without the backslash of each CommonMark escape (`\{`, `\\`, `\,`),
+/// as Claude Code's markdown shows them.
+fn md_unescape(s: &str) -> String {
+    let mut out = String::with_capacity(s.len());
+    let mut chars = s.chars();
+    while let Some(c) = chars.next() {
+        if c != '\\' {
+            out.push(c);
+            continue;
+        }
+        match chars.next() {
+            Some(next) if next.is_ascii_punctuation() => out.push(next),
+            Some(next) => {
+                out.push(c);
+                out.push(next);
+            }
+            None => out.push(c),
+        }
+    }
+    out
+}
+
+/// `s` without whitespace.
+fn normalize(s: &str) -> String {
+    s.chars().filter(|c| !c.is_whitespace()).collect()
+}
+
+/// [`normalize`], also without the characters markdown may take as emphasis
+/// or code (`*`, `_`, `~`, `` ` ``).
+fn normalize_loose(s: &str) -> String {
+    s.chars()
+        .filter(|c| !c.is_whitespace() && !matches!(c, '*' | '_' | '~' | '`'))
+        .collect()
+}
+
+/// `s` without the runs markdown may take for an HTML tag: `<` (or `</`),
+/// a letter, then anything but `<` up to the next `>`.
+fn without_html(s: &str) -> String {
+    let mut out = String::with_capacity(s.len());
+    let mut rest = s;
+    while let Some(open) = rest.find('<') {
+        out.push_str(&rest[..open]);
+        let after = &rest[open + 1..];
+        let name = after.strip_prefix('/').unwrap_or(after);
+        let tag_end = name
+            .starts_with(|c: char| c.is_ascii_alphabetic())
+            .then(|| after.find(['<', '>']))
+            .flatten()
+            .filter(|&end| after.as_bytes()[end] == b'>');
+        match tag_end {
+            Some(end) => rest = &after[end + 1..],
+            None => {
+                out.push('<');
+                rest = after;
+            }
+        }
+    }
+    out.push_str(rest);
+    out
+}
+
 #[cfg(test)]
 mod tests {
     use super::*;
`````
Run: `umask 022; cargo build --locked 2>&1 | grep -cE '^(warning|error)'; cargo test --locked --bin claudio tui::latex && make test-unit && rustfmt --edition 2021 --check src/tui/latex.rs`
Expected: `0`, then `test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 799 filtered out`, then `test result: ok. 804 passed; 0 failed`, and no rustfmt output. If `cargo build --locked` says the lock needs updating, the Cargo.lock hunk was not applied verbatim. Do not run `cargo update`.

Step 3: commit
`git add src/tui/latex.rs && git commit -m "feat(tui): render <latex> formulas as one-line and 2D Unicode text" -- Cargo.toml Cargo.lock src/tui/latex.rs src/tui/mod.rs`

Interfaces this task provides to later tasks: in `crate::tui::latex`: `struct Formula { src, inline: Option<String>, block: Option<Vec<String>>, key_strict, key_loose, key_html }`, `Formula::new(src: &str, rendered: Rendered) -> Formula`, `Formula::raw(src: &str) -> Formula`, `type Rendered = (Option<String>, Option<Vec<String>>)`, `prepare(&str) -> String`, `too_complex(&str) -> bool`, `render(&str) -> Rendered`; private `normalize`, `normalize_loose` (used by Task 6).

### Task 5: the render worker, the panic guard and `App`'s seq handling
Model: opus   Repo: /volumes/repos/claudio   Files: src/tui/mod.rs, src/tui/app.rs, src/tui/sessions.rs, src/tui/interaction.rs
Goal: AC7, the guard half of AC6, and the app half of AC4.
- `App` takes a `Latex` event only when its `seq` is newer than the one it holds. It stores the sources and emits `Effect::RenderLatex`.
- `tui/mod.rs` runs one `latex-render` thread. Each uncached source is pre-checked, then rendered on a `latex-scratch` thread (32 MiB stack, `QUIET_PANIC` set, `catch_unwind`) within 150 ms. A thread over budget is abandoned. While 2 are still running, new sources come back raw and uncached. Results are cached (LRU 1024) and come back as `HostEvent::LatexRendered`; `App` keeps them only for the `seq` it holds.
- The TUI panic hook returns at once on a `QUIET_PANIC` thread.
- `ClaudeSession` and `ClaudeSessionCleared` reset the view's formulas.

Step 1: write the failing tests.
`````diff
diff --git a/src/tui/interaction.rs b/src/tui/interaction.rs
index 5cae81e..f5b5e78 100644
--- a/src/tui/interaction.rs
+++ b/src/tui/interaction.rs
@@ -1048,6 +1048,54 @@ mod tests {
         assert_eq!(copied(&mut app), ["world"]);
     }
 
+    #[test]
+    fn the_newest_formula_list_wins_and_a_new_conversation_drops_it() {
+        use crate::tui::latex::Formula;
+        let mut app = app_with(&[info(Some(1), None)]);
+        let (id, host) = (app.sessions[0].id, app.sessions[0].host.clone());
+        let latex = |seq, list: &[&str]| Incoming::Event {
+            id,
+            event: SessionEvent::Latex {
+                seq,
+                formulas: list.iter().map(|f| f.to_string()).collect(),
+            },
+        };
+        let renders = |app: &mut App| -> Vec<(u64, Vec<String>)> {
+            app.take_effects()
+                .into_iter()
+                .filter_map(|e| match e {
+                    Effect::RenderLatex { seq, srcs, .. } => Some((seq, srcs)),
+                    _ => None,
+                })
+                .collect()
+        };
+        app.on_incoming_from(&host, latex(2, &["b"]));
+        assert_eq!(renders(&mut app), [(2, vec!["b".to_owned()])]);
+        // An older list (a replay that crossed a broadcast) is ignored.
+        app.on_incoming_from(&host, latex(1, &["a"]));
+        assert!(renders(&mut app).is_empty());
+        assert_eq!(app.sessions[0].latex_srcs, ["b"]);
+        // Only the rendering of the list held is kept.
+        app.on_latex_rendered(&host, id, 1, vec![Formula::raw("a")]);
+        assert!(app.sessions[0].latex.is_empty());
+        app.on_latex_rendered(&host, id, 2, vec![Formula::raw("b")]);
+        assert_eq!(app.sessions[0].latex, [Formula::raw("b")]);
+        // A new conversation starts over at seq 1.
+        let new_conversation = SessionEvent::ClaudeSession {
+            claude_session_id: "c2".into(),
+        };
+        app.on_incoming_from(&host, Incoming::Event { id, event: new_conversation });
+        let v = &app.sessions[0];
+        assert_eq!(v.latex_seq, 0);
+        assert!(v.latex.is_empty() && v.latex_srcs.is_empty());
+        app.on_incoming_from(&host, latex(1, &["c"]));
+        assert_eq!(renders(&mut app), [(1, vec!["c".to_owned()])]);
+        // So does a fresh reset.
+        let cleared = SessionEvent::ClaudeSessionCleared;
+        app.on_incoming_from(&host, Incoming::Event { id, event: cleared });
+        assert_eq!(app.sessions[0].latex_seq, 0);
+    }
+
     #[test]
     fn the_selection_is_drawn_reversed() {
         let mut app = app_with_text();
diff --git a/src/tui/mod.rs b/src/tui/mod.rs
index bad0fca..18cf5c3 100644
--- a/src/tui/mod.rs
+++ b/src/tui/mod.rs
@@ -1110,3 +1110,142 @@ async fn do_connect(host: String, gen: u64, tx: mpsc::Sender<HostEvent>, skip_bo
         }
     }
 }
+
+#[cfg(test)]
+mod tests {
+    use super::*;
+
+    /// A panic in a render guard leaves the terminal alone: the UI's panic
+    /// hook neither restores it nor prints. One on a thread without the
+    /// guard's flag still does.
+    #[test]
+    fn a_render_panic_does_not_restore_the_terminal() {
+        static RESTORES: AtomicUsize = AtomicUsize::new(0);
+        // Other tests panic too: count only the threads this one names.
+        fn restore() {
+            if std::thread::current().name() == Some(SCRATCH_THREAD) {
+                RESTORES.fetch_add(1, Ordering::SeqCst);
+            }
+        }
+        let previous: Arc<dyn Fn(&std::panic::PanicHookInfo<'_>) + Sync + Send> =
+            std::panic::take_hook().into();
+        let report = Arc::clone(&previous);
+        std::panic::set_hook(panic_hook(restore, Box::new(move |info| report(info))));
+        let running = Arc::new(AtomicUsize::new(0));
+        let caught = guarded(|| -> u8 { panic!("a renderer bug") }, &running);
+        let loud = std::thread::Builder::new()
+            .name(SCRATCH_THREAD.to_owned())
+            .spawn(|| panic!("not a render"))
+            .unwrap()
+            .join();
+        std::panic::set_hook(Box::new(move |info| previous(info)));
+        assert!(matches!(caught, Guarded::Failed));
+        assert!(loud.is_err());
+        assert_eq!(RESTORES.load(Ordering::SeqCst), 1, "only the unguarded one");
+        assert_eq!(running.load(Ordering::SeqCst), 0);
+    }
+
+    /// Hostile formulas go through the worker's own entry point: each
+    /// comes back raw or rendered and sanitized, nothing panics or aborts the
+    /// test binary, and none holds the worker longer than twice the budget.
+    #[test]
+    fn hostile_formulas_are_raw_or_sanitized_and_bounded_in_time() {
+        let corpus = [
+            "\\frac{a}{b".to_owned(),
+            "}}}{{{x".to_owned(),
+            "{".repeat(5000) + "x" + &"}".repeat(5000),
+            "\\frac ".repeat(340),
+            "\\hat ".repeat(400),
+            "\\not".repeat(512),
+            "x/".repeat(1000),
+            "sqrt(".repeat(400),
+            "x\u{1b}[2J\u{1b}]52;c;AAAA\u{7}y\u{9b}31m".to_owned(),
+            "a\u{202e}b\u{2066}c\u{200b}d\u{feff}e".to_owned(),
+            "\\binom{}{}".to_owned(),
+            "x".repeat(10 * 1024),
+            // Past the pre-check, but seconds of parsing: it times out.
+            "x/".repeat(255),
+        ];
+        let running = Arc::new(AtomicUsize::new(0));
+        for src in &corpus {
+            let started = Instant::now();
+            let (formula, _) = render_formula(src, &running);
+            let took = started.elapsed();
+            let head: String = src.chars().take(16).collect();
+            assert!(took < 2 * RENDER_BUDGET, "{took:?} for {head:?}…");
+            for text in formula.inline.iter().chain(formula.block.iter().flatten()) {
+                assert_eq!(*text, crate::claude::latex::sanitize(text), "{head:?}");
+                assert!(!text.contains(['\u{1b}', '\u{9b}', '\u{202e}', '\u{200b}']));
+            }
+        }
+    }
+
+    /// A render over budget is abandoned. While MAX_ABANDONED of them still
+    /// run, nothing new starts (raw, and not cached); once they end,
+    /// rendering resumes. Sleeps stand in for slow parses, so the test
+    /// burns no CPU.
+    #[test]
+    fn slow_renders_are_abandoned_up_to_a_cap() {
+        let running = Arc::new(AtomicUsize::new(0));
+        for _ in 0..MAX_ABANDONED {
+            let started = Instant::now();
+            let nap = || std::thread::sleep(4 * RENDER_BUDGET);
+            assert!(matches!(guarded(nap, &running), Guarded::Failed));
+            assert!(started.elapsed() < 2 * RENDER_BUDGET);
+        }
+        assert_eq!(running.load(Ordering::SeqCst), MAX_ABANDONED);
+        let started = Instant::now();
+        let (formula, cache) = render_formula("E = mc^2", &running);
+        assert!(started.elapsed() < RENDER_BUDGET, "not even started");
+        assert_eq!((formula.inline, cache), (None, false), "busy: raw, not cached");
+        let deadline = Instant::now() + Duration::from_secs(10);
+        while running.load(Ordering::SeqCst) > 0 {
+            assert!(Instant::now() < deadline, "the abandoned renders never ended");
+            std::thread::sleep(Duration::from_millis(20));
+        }
+        let (formula, cache) = render_formula("E = mc^2", &running);
+        assert_eq!((formula.inline.as_deref(), cache), (Some("E = mc²"), true));
+    }
+
+    #[test]
+    fn the_worker_renders_each_job_and_passes_its_seq_on() {
+        let (tx, rx) = std::sync::mpsc::channel();
+        let jobs = spawn_latex_worker(move |job, formulas| {
+            let _ = tx.send((job.seq, formulas));
+        })
+        .unwrap();
+        let id = uuid::Uuid::new_v4();
+        let job = |seq, srcs: Vec<String>| LatexJob {
+            host: "local".into(),
+            id,
+            seq,
+            srcs,
+        };
+        let deep = "\\frac ".repeat(300);
+        jobs.send(job(1, vec!["E = mc^2".into()])).unwrap();
+        jobs.send(job(2, vec!["E = mc^2".into(), deep.clone()])).unwrap();
+        let wait = Duration::from_secs(10);
+        let (seq, first) = rx.recv_timeout(wait).unwrap();
+        assert_eq!(seq, 1);
+        assert_eq!(first[0].inline.as_deref(), Some("E = mc²"));
+        let (seq, second) = rx.recv_timeout(wait).unwrap();
+        assert_eq!(seq, 2);
+        assert_eq!(second[0], first[0]);
+        assert_eq!(second[1], latex::Formula::raw(&deep), "too complex: raw");
+    }
+
+    #[test]
+    fn the_cache_forgets_the_least_recently_used() {
+        let running = Arc::new(AtomicUsize::new(0));
+        let mut cache = RenderCache::default();
+        cache.formula("0", &running);
+        for i in 1..CACHE_SIZE {
+            cache.formula(&i.to_string(), &running);
+        }
+        cache.formula("0", &running); // used again: "1" is now the oldest
+        cache.formula("new", &running);
+        assert_eq!(cache.entries.len(), CACHE_SIZE);
+        assert!(cache.entries.contains_key("0") && cache.entries.contains_key("new"));
+        assert!(!cache.entries.contains_key("1"));
+    }
+}
`````
Run: `umask 022; cargo test --locked --bin claudio -- tui::tests tui::interaction`
Expected: compilation fails with `error: could not compile `claudio` (bin "claudio" test) due to 43 previous errors`, among them `cannot find function `guarded``, `cannot find function `panic_hook``, `cannot find function `render_formula``, `cannot find function `spawn_latex_worker``, `no variant named `RenderLatex` found for enum `app::Effect``, `no method named `on_latex_rendered` found for struct `app::App``, and `no field `latex_seq` on type `sessions::SessionView``.

Step 2: implement.
`````diff
diff --git a/src/tui/app.rs b/src/tui/app.rs
index dce1f68..5a562d1 100644
--- a/src/tui/app.rs
+++ b/src/tui/app.rs
@@ -145,6 +145,14 @@ pub enum Effect {
     },
     /// Put this text on the outer terminal's clipboard (OSC 52).
     CopyToClipboard(String),
+    /// Render session `id`'s formulas, as of `seq`, on the `latex-render`
+    /// worker; they come back through [`App::on_latex_rendered`].
+    RenderLatex {
+        host: String,
+        id: SessionId,
+        seq: u64,
+        srcs: Vec<String>,
+    },
 }
 
 /// What a request's reply is for.
diff --git a/src/tui/interaction.rs b/src/tui/interaction.rs
index f5b5e78..ac81b33 100644
--- a/src/tui/interaction.rs
+++ b/src/tui/interaction.rs
@@ -21,6 +21,7 @@ use super::app::{App, Effect, Mode, ReplyTo};
 use super::confirm::ConfirmPrompt;
 use super::git_view::GitView;
 use super::keymap::Action;
+use super::latex::Formula;
 use super::selection::{self, Pos, Selection, Unit};
 use super::sessions::SessionView;
 use super::stats_view::StatsView;
@@ -677,6 +678,7 @@ impl App {
             SessionEvent::State { state } => v.state = state,
             SessionEvent::ClaudeSession { claude_session_id } => {
                 v.claude_session_id = Some(claude_session_id);
+                v.clear_latex();
                 self.save();
                 self.refresh_session_cred(super::proxy_state::SESSION_CRED_MIN_GAP_SECS);
             }
@@ -684,6 +686,7 @@ impl App {
             // from what we saved.
             SessionEvent::ClaudeSessionCleared => {
                 v.claude_session_id = None;
+                v.clear_latex();
                 self.save();
             }
             SessionEvent::Title { title } => v.title = Some(title),
@@ -721,12 +724,47 @@ impl App {
                     v.turns = turns;
                 }
             }
-            // Rendered and drawn by the formulas overlay (a later change).
-            SessionEvent::Latex { .. } => {}
+            // A replay and a broadcast may cross: only a newer list counts.
+            // It is rendered off the UI thread; the old one stays meanwhile.
+            SessionEvent::Latex { seq, formulas } => {
+                if seq > v.latex_seq {
+                    v.latex_seq = seq;
+                    v.latex_srcs = formulas;
+                    let render = Effect::RenderLatex {
+                        host: v.host.clone(),
+                        id: v.id,
+                        seq,
+                        srcs: v.latex_srcs.clone(),
+                    };
+                    self.emit(render);
+                }
+            }
             SessionEvent::Unknown => {}
         }
     }
 
+    /// The render worker is done with list `seq` of session `id` on `host`:
+    /// shown, unless a newer list came since (it is on its way).
+    pub fn on_latex_rendered(
+        &mut self,
+        host: &str,
+        id: SessionId,
+        seq: u64,
+        formulas: Vec<Formula>,
+    ) {
+        let Some(i) = self.index_of(id).filter(|&i| self.sessions[i].host == host) else {
+            return;
+        };
+        let v = &mut self.sessions[i];
+        if v.latex_seq != seq {
+            return;
+        }
+        v.latex = formulas;
+        if self.active == Some(i) {
+            self.redraw = true;
+        }
+    }
+
     /// A reply (or failure) for a request made through [`Effect::Request`].
     pub fn on_reply(&mut self, to: ReplyTo, reply: io::Result<Msg>) {
         self.redraw = true;
diff --git a/src/tui/mod.rs b/src/tui/mod.rs
index 18cf5c3..d6bff3b 100644
--- a/src/tui/mod.rs
+++ b/src/tui/mod.rs
@@ -13,8 +13,6 @@ mod git_app;
 mod git_view;
 mod interaction;
 mod keymap;
-// Its first user, the render worker, comes with the next change.
-#[allow(dead_code)]
 mod latex;
 mod notifications;
 mod plain;
@@ -41,9 +39,12 @@ mod test_support;
 mod ui;
 mod wizard;
 
+use std::cell::Cell;
+use std::collections::HashMap;
 use std::io::{self, Stdout};
 use std::process::ExitCode;
-use std::sync::atomic::{AtomicBool, Ordering};
+use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
+use std::sync::Arc;
 use std::time::{Duration, Instant};
 
 use crossterm::event::{
@@ -81,6 +82,28 @@ const OUTDATED_RETRY: Duration = Duration::from_secs(120);
 /// Whether keyboard enhancement flags were pushed (and must be popped).
 static KEYBOARD_ENHANCED: AtomicBool = AtomicBool::new(false);
 
+/// How long the render worker waits for one formula before it gives up on
+/// it (it is shown raw).
+const RENDER_BUDGET: Duration = Duration::from_millis(150);
+/// How many renders that ran over budget may still be running before new
+/// formulas are left raw (and tried again on a later list).
+const MAX_ABANDONED: usize = 2;
+/// How many rendered formulas the worker remembers, by source.
+const CACHE_SIZE: usize = 1024;
+/// The stack of a render scratch thread: deep enough for anything that
+/// passes `latex::too_complex`, in a debug or a release build.
+const SCRATCH_STACK: usize = 32 << 20;
+/// The thread names of the render worker and of its scratch threads.
+const RENDER_THREAD: &str = "latex-render";
+const SCRATCH_THREAD: &str = "latex-scratch";
+
+thread_local! {
+    /// Set on a render scratch thread. A panic there is the renderer's,
+    /// caught and shown as a raw formula: the UI's panic hook must not
+    /// touch the terminal for it (see [`panic_hook`]).
+    static QUIET_PANIC: Cell<bool> = const { Cell::new(false) };
+}
+
 type Term = Terminal<CrosstermBackend<Stdout>>;
 
 /// Emit an OSC 9 desktop notification + single BEL to the outer terminal for a
@@ -344,11 +367,7 @@ async fn list_sessions(client: &Client) -> io::Result<Vec<SessionInfo>> {
 
 /// Set the terminal up for the UI; returns it with its size `(width, height)`.
 fn enter_terminal() -> io::Result<(Term, (u16, u16))> {
-    let default_hook = std::panic::take_hook();
-    std::panic::set_hook(Box::new(move |info| {
-        restore_terminal();
-        default_hook(info);
-    }));
+    std::panic::set_hook(panic_hook(restore_terminal, std::panic::take_hook()));
     enable_raw_mode()?;
     let mut out = io::stdout();
     execute!(
@@ -370,6 +389,21 @@ fn enter_terminal() -> io::Result<(Term, (u16, u16))> {
     Ok((terminal, (size.width, size.height)))
 }
 
+type PanicHook = Box<dyn Fn(&std::panic::PanicHookInfo<'_>) + Sync + Send + 'static>;
+
+/// The UI's panic hook: put the terminal back (`restore`), then report the
+/// panic as `default` would. A panic on a render scratch thread does
+/// neither: it is caught there, and the UI goes on.
+fn panic_hook(restore: fn(), default: PanicHook) -> PanicHook {
+    Box::new(move |info| {
+        if QUIET_PANIC.with(Cell::get) {
+            return;
+        }
+        restore();
+        default(info);
+    })
+}
+
 /// Undo everything `enter_terminal` did. Safe to call more than once and
 /// from the panic hook.
 fn restore_terminal() {
@@ -446,6 +480,145 @@ enum HostEvent {
     UpgradeAvailable(Option<String>),
     /// The newest claude version on its release channel.
     ClaudeLatest(String),
+    /// The render worker finished a session's formula list.
+    LatexRendered {
+        host: String,
+        id: crate::proto::SessionId,
+        seq: u64,
+        formulas: Vec<latex::Formula>,
+    },
+}
+
+// ── Formula rendering ─────────────────────────────────────────────────────────
+
+/// One [`Effect::RenderLatex`]: session `id`'s formulas as of `seq`.
+struct LatexJob {
+    host: String,
+    id: crate::proto::SessionId,
+    seq: u64,
+    srcs: Vec<String>,
+}
+
+/// Start the `latex-render` thread: it renders each job's formulas (from
+/// its cache when it can) and hands them to `done`. It ends when the
+/// returned sender is dropped. Rendering never runs on the UI thread: one
+/// hostile formula can take a minute to parse.
+fn spawn_latex_worker(
+    done: impl Fn(LatexJob, Vec<latex::Formula>) + Send + 'static,
+) -> io::Result<std::sync::mpsc::Sender<LatexJob>> {
+    let (jobs, queue) = std::sync::mpsc::channel::<LatexJob>();
+    std::thread::Builder::new()
+        .name(RENDER_THREAD.to_owned())
+        .spawn(move || {
+            let mut cache = RenderCache::default();
+            let running = Arc::new(AtomicUsize::new(0));
+            for job in queue {
+                let formulas = job
+                    .srcs
+                    .iter()
+                    .map(|src| cache.formula(src, &running))
+                    .collect();
+                done(job, formulas);
+            }
+        })?;
+    Ok(jobs)
+}
+
+/// Rendered formulas by source, the [`CACHE_SIZE`] most recently used: each
+/// list resends every formula, and a source is rendered once.
+#[derive(Default)]
+struct RenderCache {
+    entries: HashMap<String, (u64, latex::Formula)>,
+    /// Bumped on every lookup; an entry keeps the value of its last use.
+    clock: u64,
+}
+
+impl RenderCache {
+    fn formula(&mut self, src: &str, running: &Arc<AtomicUsize>) -> latex::Formula {
+        self.clock += 1;
+        if let Some((used, formula)) = self.entries.get_mut(src) {
+            *used = self.clock;
+            return formula.clone();
+        }
+        let (formula, keep) = render_formula(src, running);
+        if keep {
+            if self.entries.len() >= CACHE_SIZE {
+                let oldest = self
+                    .entries
+                    .iter()
+                    .min_by_key(|(_, (used, _))| *used)
+                    .map(|(src, _)| src.clone());
+                if let Some(oldest) = oldest {
+                    self.entries.remove(&oldest);
+                }
+            }
+            self.entries
+                .insert(src.to_owned(), (self.clock, formula.clone()));
+        }
+        formula
+    }
+}
+
+/// Render one formula within [`RENDER_BUDGET`]: what the pre-check turns
+/// away, or what times out or panics, is shown raw. The flag says whether
+/// the result may be cached: not when too many renders were still running
+/// to start this one (a later list tries it again).
+fn render_formula(src: &str, running: &Arc<AtomicUsize>) -> (latex::Formula, bool) {
+    let clean = latex::prepare(src);
+    if latex::too_complex(&clean) {
+        return (latex::Formula::raw(src), true);
+    }
+    match guarded(move || latex::render(&clean), running) {
+        Guarded::Done(rendered) => (latex::Formula::new(src, rendered), true),
+        Guarded::Failed => (latex::Formula::raw(src), true),
+        Guarded::Busy => (latex::Formula::raw(src), false),
+    }
+}
+
+/// How a [`guarded`] call ended.
+enum Guarded<T> {
+    Done(T),
+    /// It panicked or ran over budget.
+    Failed,
+    /// It was not started: [`MAX_ABANDONED`] renders still run (or no
+    /// thread could be made).
+    Busy,
+}
+
+/// Run `f` on a fresh scratch thread with a [`SCRATCH_STACK`] stack and a
+/// panic in it caught quietly ([`QUIET_PANIC`]), waiting at most
+/// [`RENDER_BUDGET`]. A thread still running then is abandoned and ends on
+/// its own. `running` counts the scratch threads alive; when this returns,
+/// those are all abandoned ones.
+fn guarded<T: Send + 'static>(
+    f: impl FnOnce() -> T + Send + 'static,
+    running: &Arc<AtomicUsize>,
+) -> Guarded<T> {
+    if running.load(Ordering::Acquire) >= MAX_ABANDONED {
+        return Guarded::Busy;
+    }
+    let (tx, rx) = std::sync::mpsc::sync_channel(1);
+    let alive = Arc::clone(running);
+    alive.fetch_add(1, Ordering::AcqRel);
+    let spawned = std::thread::Builder::new()
+        .name(SCRATCH_THREAD.to_owned())
+        .stack_size(SCRATCH_STACK)
+        .spawn(move || {
+            QUIET_PANIC.with(|quiet| quiet.set(true));
+            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
+            // Counted out before the result goes, so a caller that has it
+            // never counts this thread as running.
+            alive.fetch_sub(1, Ordering::AcqRel);
+            let _ = tx.send(result);
+        });
+    if spawned.is_err() {
+        running.fetch_sub(1, Ordering::AcqRel);
+        return Guarded::Busy;
+    }
+    match rx.recv_timeout(RENDER_BUDGET) {
+        Ok(Ok(value)) => Guarded::Done(value),
+        Ok(Err(_)) | Err(_) => Guarded::Failed,
+    }
 }
 
 // ── Event loop ────────────────────────────────────────────────────────────────
@@ -545,6 +718,18 @@ async fn event_loop(
         });
     }
 
+    // Formulas are rendered on their own thread; the results come back here.
+    let rendered = ev_tx.clone();
+    let latex_jobs = spawn_latex_worker(move |job, formulas| {
+        let _ = rendered.blocking_send(HostEvent::LatexRendered {
+            host: job.host,
+            id: job.id,
+            seq: job.seq,
+            formulas,
+        });
+    })
+    .ok();
+
     let (reply_tx, mut reply_rx) = mpsc::unbounded_channel::<(ReplyTo, io::Result<Msg>)>();
     let mut events = EventStream::new();
     let mut tick = tokio::time::interval(TICK);
@@ -561,7 +746,7 @@ async fn event_loop(
 
     loop {
         for effect in app.take_effects() {
-            run_effect(effect, &mut app, conns, &reply_tx, &ev_tx);
+            run_effect(effect, &mut app, conns, &reply_tx, &ev_tx, latex_jobs.as_ref());
         }
         if app.quit {
             return Ok(std::mem::take(&mut app.exit));
@@ -705,6 +890,9 @@ async fn event_loop(
                     }
                 }
                 Some(HostEvent::ClaudeLatest(latest)) => app.check_local_claude(&latest),
+                Some(HostEvent::LatexRendered { host, id, seq, formulas }) => {
+                    app.on_latex_rendered(&host, id, seq, formulas);
+                }
                 None => return Ok(Default::default()),
             },
             Some((to, reply)) = reply_rx.recv() => app.on_reply(to, reply),
@@ -741,6 +929,7 @@ fn run_effect(
     conns: &mut Connections,
     reply_tx: &mpsc::UnboundedSender<(ReplyTo, io::Result<Msg>)>,
     ev_tx: &mpsc::Sender<HostEvent>,
+    latex_jobs: Option<&std::sync::mpsc::Sender<LatexJob>>,
 ) {
     match effect {
         Effect::Request { host, msg, to } => {
@@ -793,6 +982,12 @@ fn run_effect(
         }
         Effect::LoadSshHosts => app.on_ssh_hosts(crate::remote::hosts::candidates()),
         Effect::CopyToClipboard(text) => copy_to_clipboard(&text),
+        Effect::RenderLatex { host, id, seq, srcs } => {
+            // Without a worker (it could not start) formulas stay raw.
+            if let Some(jobs) = latex_jobs {
+                let _ = jobs.send(LatexJob { host, id, seq, srcs });
+            }
+        }
         Effect::Save => {
             if let Err(e) = app.to_state().save(&paths::client_state()) {
                 app.notify(format!("could not save state: {e}"));
diff --git a/src/tui/sessions.rs b/src/tui/sessions.rs
index ba89003..d954179 100644
--- a/src/tui/sessions.rs
+++ b/src/tui/sessions.rs
@@ -15,6 +15,7 @@ use crate::term::screen::Screen;
 use super::app::{App, Effect, Mode, ReplyTo, PROJECTS_LIMIT};
 use super::confirm::{Choice, ConfirmAction, ConfirmPrompt};
 use super::interaction::Modal;
+use super::latex::Formula;
 use super::state::{self, ClientState, KillTombstone, SavedSession};
 use super::wizard::{self, Wizard};
 
@@ -67,6 +68,13 @@ pub struct SessionView {
     pub turns: Option<u32>,
     /// Claude, or a plain terminal tab.
     pub kind: SessionKind,
+    /// The `seq` of the newest formula list taken (0: none yet), and its
+    /// formulas' sources, oldest first.
+    pub latex_seq: u64,
+    pub latex_srcs: Vec<String>,
+    /// Those formulas rendered; until the render worker is done with a new
+    /// list, the previous one.
+    pub latex: Vec<Formula>,
 }
 
 impl SessionView {
@@ -103,9 +111,19 @@ impl SessionView {
             output_tokens: None,
             turns: None,
             kind,
+            latex_seq: 0,
+            latex_srcs: Vec::new(),
+            latex: Vec::new(),
         }
     }
 
+    /// Forget the formulas: a new conversation has its own, from seq 1.
+    pub fn clear_latex(&mut self) {
+        self.latex_seq = 0;
+        self.latex_srcs.clear();
+        self.latex.clear();
+    }
+
     /// The view of a session `host`'s daemon reports.
     pub fn from_info(host: &str, info: &SessionInfo, size: (u16, u16)) -> SessionView {
         SessionView {
`````
Run: `umask 022; cargo build --locked 2>&1 | grep -cE '^(warning|error)'; cargo test --locked --bin claudio -- tui::tests tui::interaction && make test-unit`
Expected: `0`, then `test result: ok. 17 passed; 0 failed; 0 ignored; 0 measured; 793 filtered out`, then `test result: ok. 810 passed; 0 failed`. With `--nocapture`, the hostile-corpus test may print a `thread 'latex-scratch' panicked … layout.rs:877 … attempt to subtract with overflow` line. That is the `\binom{}{}` renderer bug caught by the guard: no TUI hook is installed in tests, so the default hook prints it. Each hostile formula returns within 2 × 150 ms, and the test binary must not abort.

Step 3: commit
`git commit -m "feat(tui): render formulas on a budgeted worker thread, newest list wins" -- src/tui/mod.rs src/tui/app.rs src/tui/sessions.rs src/tui/interaction.rs`

Interfaces this task provides to later tasks: `SessionView::{latex_seq: u64, latex_srcs: Vec<String>, latex: Vec<Formula>}`, `SessionView::clear_latex()`, `App::on_latex_rendered(host: &str, id: SessionId, seq: u64, formulas: Vec<Formula>)`, `Effect::RenderLatex { host, id, seq, srcs }`, `HostEvent::LatexRendered { host, id, seq, formulas }`.

### Task 6: the pane overlay
Model: sonnet   Repo: /volumes/repos/claudio   Files: src/tui/latex.rs, src/tui/ui/mod.rs
Goal: AC8. `latex::overlay` runs in `draw_pane` after `Screen::render` (and before the selection highlight), only when the view has formulas.
- It finds `<latex>…</latex>` on the visible rows: one row, or up to 8 when the tag is alone on its first row and continues on indented rows.
- It matches each tag to the newest formula by `key_strict`, then `key_loose`, then `key_html`.
- It rebuilds each row once, left to right, from its own cells, writing grapheme clusters (`text_cells`). A row that would overflow stays raw, and no other row changes.

Step 1: write the failing tests.
`````diff
diff --git a/src/tui/latex.rs b/src/tui/latex.rs
index 1972bc9..7f362e0 100644
--- a/src/tui/latex.rs
+++ b/src/tui/latex.rs
@@ -567,4 +567,172 @@ mod tests {
         assert_eq!(f.key_html, "xy<z");
         assert_eq!(f.key_loose, "x<b>y</b><z");
     }
+
+    // ── overlay ───────────────────────────────────────────────────────────────
+
+    /// A 60×6 screen showing `text`.
+    fn screen(text: &str) -> Screen {
+        let mut screen = Screen::new(6, 60);
+        screen.feed(text.replace('\n', "\r\n").as_bytes());
+        screen
+    }
+
+    /// The pane `screen` draws, with `formulas` laid over it.
+    fn drawn(screen: &Screen, formulas: &[Formula]) -> Buffer {
+        let (rows, cols) = screen.size();
+        let area = Rect::new(0, 0, cols, rows);
+        let mut buf = Buffer::empty(area);
+        screen.render(area, &mut buf);
+        overlay(&mut buf, area, screen, formulas);
+        buf
+    }
+
+    /// The rows of `buf` as text: each cell's symbol once (a wide one's
+    /// covered cell skipped), trailing blanks dropped.
+    fn rows_of(buf: &Buffer) -> Vec<String> {
+        let area = buf.area;
+        (0..area.height)
+            .map(|y| {
+                let mut line = String::new();
+                let mut covered = 0;
+                for x in 0..area.width {
+                    if covered > 0 {
+                        covered -= 1;
+                        continue;
+                    }
+                    let symbol = buf[(x, y)].symbol();
+                    line.push_str(symbol);
+                    covered = symbol.width().saturating_sub(1);
+                }
+                line.trim_end().to_owned()
+            })
+            .collect()
+    }
+
+    fn shown(screen: &Screen, formulas: &[Formula]) -> Vec<String> {
+        rows_of(&drawn(screen, formulas))
+    }
+
+    /// A formula `src` whose one-line rendering is `inline`.
+    fn formula(src: &str, inline: &str) -> Formula {
+        Formula::new(src, (Some(inline.to_owned()), None))
+    }
+
+    #[test]
+    fn a_tag_on_one_row_is_replaced_in_the_accent_colour() {
+        let s = screen("before\nso <latex>E = mc^2</latex>, done\nafter");
+        let buf = drawn(&s, &[formula("E = mc^2", "E = mc²")]);
+        assert_eq!(rows_of(&buf)[..3], ["before", "so E = mc², done", "after"]);
+        assert_eq!(buf[(3, 1)].fg, ACCENT);
+        assert_ne!(buf[(10, 1)].fg, ACCENT, "the rest keeps its colour");
+        // Without formulas nothing changes.
+        assert_eq!(shown(&s, &[])[1], "so <latex>E = mc^2</latex>, done");
+    }
+
+    #[test]
+    fn two_tags_on_a_row_are_both_replaced() {
+        let s = screen("<latex>\\alpha</latex> and <latex>\\beta</latex>!");
+        let formulas = [formula("\\alpha", "α"), formula("\\beta", "β")];
+        assert_eq!(shown(&s, &formulas)[0], "α and β!");
+    }
+
+    #[test]
+    fn tags_match_after_markdown_ate_escapes_emphasis_or_html() {
+        let cases = [
+            // `\{` and `\,` lost their backslash on screen.
+            (
+                "\\{a_i\\} \\cup \\{b_i\\}",
+                "{a_i} \\cup {b_i}",
+                "{aᵢ} ∪ {bᵢ}",
+            ),
+            ("x^2\\,dx", "x^2,dx", "x² dx"),
+            // `_i + b_` was taken for emphasis.
+            ("a_i + b_i", "ai + bi", "aᵢ + bᵢ"),
+            // `<y>` was taken for an HTML tag.
+            ("x <y> z", "x  z", "x y z"),
+        ];
+        for (src, on_screen, inline) in cases {
+            let s = screen(&format!("<latex>{on_screen}</latex>"));
+            assert_eq!(shown(&s, &[formula(src, inline)])[0], inline, "{src}");
+        }
+    }
+
+    #[test]
+    fn the_newest_matching_formula_wins() {
+        let s = screen("<latex>x</latex>");
+        let formulas = [formula("x", "old"), formula("x", "new")];
+        assert_eq!(shown(&s, &formulas)[0], "new");
+    }
+
+    #[test]
+    fn a_tag_continued_on_an_indented_row_is_joined() {
+        let s =
+            screen("root: <latex>x = \\frac{-b \\pm\n  \\sqrt{b^2 - 4ac}}{2a}</latex> here\nnext");
+        let f = formula(
+            "x = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}{2a}",
+            "x = (-b ± √(b² - 4ac))/(2a)",
+        );
+        assert_eq!(
+            shown(&s, &[f])[..3],
+            ["root: x = (-b ± √(b² - 4ac))/(2a) here", "", "next"]
+        );
+    }
+
+    #[test]
+    fn a_suffix_that_no_longer_fits_stays_on_the_last_row() {
+        let s = screen(&format!("{}<latex>a\n  b</latex> tail", "p".repeat(50)));
+        let f = formula("a b", "abcdefgh");
+        let want = [format!("{}abcdefgh", "p".repeat(50)), "   tail".to_owned()];
+        assert_eq!(shown(&s, &[f])[..2], want);
+    }
+
+    #[test]
+    fn tags_that_cannot_be_replaced_stay_raw() {
+        let raw = |text: &str, formulas: &[Formula]| {
+            let s = screen(text);
+            assert_eq!(shown(&s, formulas), shown(&s, &[]), "{text}");
+        };
+        let ab = [formula("a b", "AB")];
+        raw("<latex>z</latex>", &ab); // no formula matches
+        raw("<latex>a b", &ab); // never closed
+        raw("<latex>a b</latex>", &[Formula::raw("a b")]); // no rendering
+        let wide = [formula("a b", &"w".repeat(30))];
+        raw(&format!("{}<latex>a b</latex>", "p".repeat(40)), &wide); // too wide
+                                                                      // A tag continued on the next row after another tag on its first row
+                                                                      // stays raw, while that other tag is still replaced.
+        raw("<latex>q</latex> <latex>a\n  b</latex>", &ab);
+        let s = screen("<latex>q</latex> <latex>a\n  b</latex>");
+        assert_eq!(
+            shown(&s, &[formula("q", "Q")])[..2],
+            ["Q <latex>a", "  b</latex>"]
+        );
+        // A tag that runs past MAX_SPAN_ROWS rows.
+        let long = format!(
+            "<latex>a{}\n  b</latex>",
+            "\n  x".repeat(MAX_SPAN_ROWS as usize)
+        );
+        let mut s = Screen::new(14, 60);
+        s.feed(long.replace('\n', "\r\n").as_bytes());
+        assert_eq!(shown(&s, &ab), shown(&s, &[]));
+    }
+
+    #[test]
+    fn wide_characters_after_a_tag_keep_their_two_cells() {
+        let s = screen("<latex>a</latex>漢字x");
+        let buf = drawn(&s, &[formula("a", "α")]);
+        assert_eq!(rows_of(&buf)[0], "α漢字x");
+        assert_eq!(buf[(1, 0)].symbol(), "漢");
+        assert_eq!(buf[(3, 0)].symbol(), "字");
+        assert_eq!(buf[(5, 0)].symbol(), "x");
+    }
+
+    #[test]
+    fn unsafe_clusters_never_become_cells() {
+        let style = Style::default();
+        let cells = text_cells("a\u{202e}b\u{1b}c\u{200b}", style).unwrap();
+        let text: String = cells.iter().map(Cell::symbol).collect();
+        assert_eq!(text, "abc");
+        // A wide cluster takes its cell and the one it covers.
+        assert_eq!(text_cells("漢", style).unwrap().len(), 2);
+    }
 }
diff --git a/src/tui/ui/mod.rs b/src/tui/ui/mod.rs
index 535b1aa..6bba81c 100644
--- a/src/tui/ui/mod.rs
+++ b/src/tui/ui/mod.rs
@@ -863,6 +863,27 @@ mod tests {
         }
     }
 
+    /// The active session's rendered formulas replace their tags in the
+    /// pane; the mirror (what selection copies) keeps the tags.
+    #[test]
+    fn the_pane_shows_formulas_rendered() {
+        use ratatui::{backend::TestBackend, Terminal};
+        let mut app = crate::tui::test_support::app_with(&[crate::tui::test_support::info(
+            Some(1),
+            None,
+        )]);
+        let v = &mut app.sessions[0];
+        v.mirror.feed(b"energy: <latex>E = mc^2</latex>.");
+        let rendered = (Some("E = mc\u{b2}".to_owned()), None);
+        v.latex = vec![crate::tui::latex::Formula::new("E = mc^2", rendered)];
+        let mut term = Terminal::new(TestBackend::new(app.width, app.height)).unwrap();
+        term.draw(|f| draw(f, &app)).unwrap();
+        let buf = term.backend().buffer();
+        let row: String = (0..buf.area.width).map(|x| buf[(x, 1)].symbol()).collect();
+        assert_eq!(row.trim_end(), "energy: E = mc\u{b2}.");
+        assert!(app.sessions[0].mirror.row_chars(0).iter().collect::<String>().contains("<latex>"));
+    }
+
     fn total_width(titles: &[(String, String)]) -> usize {
         titles
             .iter()
`````
Run: `umask 022; cargo test --locked --bin claudio -- tui::latex tui::ui::tests`
Expected: compilation fails with `error: could not compile `claudio` (bin "claudio" test) due to 17 previous errors`, among them `cannot find function `overlay``, `cannot find function `text_cells``, `cannot find value `ACCENT``, `cannot find value `MAX_SPAN_ROWS`` and `cannot find type `Screen``.

Step 2: implement.
`````diff
diff --git a/src/tui/latex.rs b/src/tui/latex.rs
index 7f362e0..2ea59ae 100644
--- a/src/tui/latex.rs
+++ b/src/tui/latex.rs
@@ -1,4 +1,5 @@
-//! `<latex>` formulas: rendering them as Unicode text.
+//! `<latex>` formulas: rendering them as Unicode text, and laying them over
+//! the session pane.
 //!
 //! Pure functions only. The render itself ([`render`]) parses and lays out
 //! attacker-influenced text with a parser that is slow and deep on hostile
@@ -9,11 +10,17 @@
 //! - [`Formula`] — one formula's source, its renderings and matching keys.
 //! - [`render`] — the 2D block (term-maths) and the one-line form.
 //! - [`too_complex`] — the pre-check that runs before any parsing.
+//! - [`overlay`] — the pane's `<latex>` tags replaced by one-line renderings.
 
+use ratatui::buffer::{Buffer, Cell};
+use ratatui::layout::Rect;
+use ratatui::style::{Color, Style};
 use rust_latex_parser::{AccentKind, EqNode, MatrixKind};
-use unicode_width::UnicodeWidthStr;
+use unicode_segmentation::UnicodeSegmentation;
+use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
 
-use crate::claude::latex::{sanitize, MAX_FORMULA_BYTES};
+use crate::claude::latex::{is_unsafe, sanitize, MAX_FORMULA_BYTES};
+use crate::term::screen::Screen;
 
 /// Deepest `{`…`}` nesting a formula may have to be rendered.
 const MAX_DEPTH: usize = 32;
@@ -466,6 +473,333 @@ fn without_html(s: &str) -> String {
     out
 }
 
+// ── Pane overlay ──────────────────────────────────────────────────────────────
+
+/// The colour of a formula rendered in the pane.
+const ACCENT: Color = Color::Cyan;
+/// The most screen rows one tag may span: claude wraps long lines itself,
+/// indenting the rows that continue one.
+const MAX_SPAN_ROWS: u16 = 8;
+const OPEN_TAG: &str = "<latex>";
+const CLOSE_TAG: &str = "</latex>";
+
+/// Replace, in the pane `buf` has just been drawn with from `screen` at
+/// `area`, each complete `<latex>…</latex>` whose text matches one of
+/// `formulas` by that formula's one-line rendering, in [`ACCENT`]. A tag may
+/// continue on the next rows (up to [`MAX_SPAN_ROWS`]) when it is the only
+/// one on its first row. Rows are rebuilt from their own cells, so no row
+/// moves; a row (or tag) whose result would not fit stays as it was. The
+/// selection, copy and the cursor still go by `screen`.
+pub fn overlay(buf: &mut Buffer, area: Rect, screen: &Screen, formulas: &[Formula]) {
+    let rows = area.height.min(screen.size().0);
+    let mut row = 0;
+    while row < rows {
+        row = if shows_open_tag(buf, area, row) {
+            overlay_row(buf, area, screen, formulas, row, rows)
+        } else {
+            row + 1
+        };
+    }
+}
+
+/// Whether row `row` of the drawn pane shows `<latex>`: a look at its
+/// cells, so a row without one costs nothing more.
+fn shows_open_tag(buf: &Buffer, area: Rect, row: u16) -> bool {
+    let tag = OPEN_TAG.as_bytes();
+    let mut matched = 0;
+    for x in area.left()..area.right() {
+        let Some(cell) = buf.cell((x, area.y + row)) else {
+            return false;
+        };
+        matched = match cell.symbol().as_bytes() {
+            [b] if *b == tag[matched] => matched + 1,
+            b"<" => 1,
+            _ => 0,
+        };
+        if matched == tag.len() {
+            return true;
+        }
+    }
+    false
+}
+
+/// Lay the formulas over the tags that start on `row`; returns the next row
+/// to look at (past a tag that continues on later rows).
+fn overlay_row(
+    buf: &mut Buffer,
+    area: Rect,
+    screen: &Screen,
+    formulas: &[Formula],
+    row: u16,
+    rows: u16,
+) -> u16 {
+    let line = Row::read(screen, row, area.width);
+    let tags = line.tags();
+    match tags.as_slice() {
+        [Tag { open, close: None }] => {
+            overlay_span(buf, area, screen, formulas, (row, rows), &line, *open)
+        }
+        _ => {
+            rebuild_row(buf, area, row, &line, &tags, formulas);
+            row + 1
+        }
+    }
+}
+
+/// Rebuild `row` with each closed tag that matches a formula replaced by
+/// its rendering, unless the row would no longer fit. An unmatched or
+/// unclosed tag stays as it is.
+fn rebuild_row(
+    buf: &mut Buffer,
+    area: Rect,
+    row: u16,
+    line: &Row,
+    tags: &[Tag],
+    formulas: &[Formula],
+) {
+    let original = cells_of(buf, area, row);
+    let mut out = Vec::with_capacity(original.len());
+    let mut copied = 0;
+    for tag in tags {
+        let Some(close) = tag.close else { continue };
+        let (start, end) = (line.col(tag.open), line.col(close + CLOSE_TAG.len()));
+        let text = line.text(tag.open + OPEN_TAG.len(), close);
+        let Some(cells) = matching(formulas, &text)
+            .and_then(|inline| text_cells(inline, accent_on(&original[start])))
+        else {
+            continue;
+        };
+        out.extend_from_slice(&original[copied..start]);
+        out.extend(cells);
+        copied = end;
+    }
+    if copied == 0 {
+        return;
+    }
+    out.extend_from_slice(&original[copied..content_end(&original).max(copied)]);
+    if out.len() <= original.len() {
+        write_row(buf, area, row, out, &original);
+    }
+}
+
+/// A tag opened on `row` (its only one) that closes on one of the next
+/// [`MAX_SPAN_ROWS`] rows; its text is its rows' text after their
+/// indentation, joined by spaces. When it matches, the first row ends with
+/// the rendering, then the text after the tag if it fits there (else that
+/// text stays on the last row, after its indentation), and the tag's other
+/// rows are blanked. Returns the next row to look at.
+fn overlay_span(
+    buf: &mut Buffer,
+    area: Rect,
+    screen: &Screen,
+    formulas: &[Formula],
+    (row, rows): (u16, u16),
+    first: &Row,
+    open: usize,
+) -> u16 {
+    let mut text = first.text(open + OPEN_TAG.len(), first.glyphs.len());
+    let mut closing = None;
+    for last in (row + 1..rows).take(MAX_SPAN_ROWS as usize) {
+        let line = Row::read(screen, last, area.width);
+        // Another tag in the way: this one is not laid over.
+        if line.find(OPEN_TAG, 0).is_some() {
+            break;
+        }
+        let indent = line.indent();
+        let close = line.find(CLOSE_TAG, indent);
+        text.push(' ');
+        text.push_str(&line.text(indent, close.unwrap_or(line.glyphs.len())));
+        if let Some(close) = close {
+            closing = Some((last, line, indent, close));
+            break;
+        }
+    }
+    let Some((last, line, indent, close)) = closing else {
+        return row + 1;
+    };
+    let first_cells = cells_of(buf, area, row);
+    let start = first.col(open);
+    let Some(rendered) = matching(formulas, &text)
+        .and_then(|inline| text_cells(inline, accent_on(&first_cells[start])))
+    else {
+        return last + 1;
+    };
+    let width = first_cells.len();
+    let mut head = first_cells[..start].to_vec();
+    head.extend(rendered);
+    if head.len() > width {
+        return last + 1;
+    }
+    let last_cells = cells_of(buf, area, last);
+    let suffix_start = line.col(close + CLOSE_TAG.len());
+    let suffix = &last_cells[suffix_start..content_end(&last_cells).max(suffix_start)];
+    let suffix_fits = head.len() + suffix.len() <= width;
+    if suffix_fits {
+        head.extend_from_slice(suffix);
+    }
+    write_row(buf, area, row, head, &first_cells);
+    for blank in row + 1..last {
+        let cells = cells_of(buf, area, blank);
+        write_row(buf, area, blank, Vec::new(), &cells);
+    }
+    let mut tail = Vec::new();
+    if !suffix_fits {
+        tail.extend_from_slice(&last_cells[..line.col(indent)]);
+        tail.extend_from_slice(suffix);
+    }
+    write_row(buf, area, last, tail, &last_cells);
+    last + 1
+}
+
+/// The one-line rendering of the newest formula whose key matches a tag's
+/// on-screen `text`: strictly, then without emphasis characters, then also
+/// without HTML-looking runs (markdown ate them from the screen). `None`
+/// when no formula matches, or the matching one has no rendering.
+fn matching<'a>(formulas: &'a [Formula], text: &str) -> Option<&'a str> {
+    let (strict, loose) = (normalize(text), normalize_loose(text));
+    let newest = || formulas.iter().rev();
+    newest()
+        .find(|f| f.key_strict == strict)
+        .or_else(|| newest().find(|f| f.key_loose == loose))
+        .or_else(|| newest().find(|f| f.key_html == loose))?
+        .inline
+        .as_deref()
+}
+
+/// The accent colour on the background the tag had.
+fn accent_on(cell: &Cell) -> Style {
+    Style::default().fg(ACCENT).bg(cell.bg)
+}
+
+/// `text` as pane cells in `style`: one grapheme cluster per cell, a wide
+/// one followed by the blank cell it covers (as `Buffer::set_string` lays
+/// them out). Clusters of width 0, or holding a character [`sanitize`]
+/// removes, are dropped; `None` when one is wider than two columns.
+pub(super) fn text_cells(text: &str, style: Style) -> Option<Vec<Cell>> {
+    let mut cells = Vec::with_capacity(text.len());
+    for cluster in text.graphemes(true) {
+        if cluster
+            .chars()
+            .any(|c| is_unsafe(c) || c == '\n' || c == '\t')
+        {
+            continue;
+        }
+        let mut cell = Cell::default();
+        cell.set_symbol(cluster).set_style(style);
+        match cluster.width() {
+            0 => {}
+            1 => cells.push(cell),
+            2 => cells.extend([cell, Cell::default()]),
+            _ => return None,
+        }
+    }
+    Some(cells)
+}
+
+/// The cells of pane row `row`.
+fn cells_of(buf: &Buffer, area: Rect, row: u16) -> Vec<Cell> {
+    (area.left()..area.right())
+        .map(|x| buf[(x, area.y + row)].clone())
+        .collect()
+}
+
+/// Where a row's content ends: past its last non-blank cell (and the cell a
+/// wide one covers).
+fn content_end(cells: &[Cell]) -> usize {
+    cells
+        .iter()
+        .rposition(|c| c.symbol() != " ")
+        .map_or(0, |i| {
+            (i + cells[i].symbol().width().max(1)).min(cells.len())
+        })
+}
+
+/// Put `cells` on pane row `row`, padded with blanks styled like the row's
+/// last cell (of `original`).
+fn write_row(buf: &mut Buffer, area: Rect, row: u16, cells: Vec<Cell>, original: &[Cell]) {
+    let mut blank = original.last().cloned().unwrap_or_default();
+    blank.set_symbol(" ");
+    let padded = cells.into_iter().chain(std::iter::repeat(blank));
+    for (x, cell) in (area.left()..area.right()).zip(padded) {
+        buf[(x, area.y + row)] = cell;
+    }
+}
+
+/// A `<latex>` on a row, by glyph index: its `<`, and the `<` of its
+/// `</latex>` when it closes on the same row.
+struct Tag {
+    open: usize,
+    close: Option<usize>,
+}
+
+/// The characters of a screen row: each once (a wide one at its first
+/// column), with the column it starts at.
+struct Row {
+    glyphs: Vec<(usize, char)>,
+    width: usize,
+}
+
+impl Row {
+    fn read(screen: &Screen, row: u16, width: u16) -> Row {
+        let mut chars = screen.row_chars(row);
+        chars.truncate(width as usize);
+        let mut glyphs = Vec::with_capacity(chars.len());
+        let mut col = 0;
+        while col < chars.len() {
+            let c = chars[col];
+            glyphs.push((col, c));
+            // `row_chars` repeats a wide character in its second column.
+            col += c.width().unwrap_or(1).max(1);
+        }
+        Row {
+            glyphs,
+            width: chars.len(),
+        }
+    }
+
+    /// Every `<latex>` that starts on the row, in order; only the last one
+    /// can be unclosed.
+    fn tags(&self) -> Vec<Tag> {
+        let mut tags = Vec::new();
+        let mut at = 0;
+        while let Some(open) = self.find(OPEN_TAG, at) {
+            let close = self.find(CLOSE_TAG, open + OPEN_TAG.len());
+            tags.push(Tag { open, close });
+            match close {
+                Some(close) => at = close + CLOSE_TAG.len(),
+                None => break,
+            }
+        }
+        tags
+    }
+
+    /// The glyph index of the next `tag` (ASCII) at or after glyph `from`.
+    fn find(&self, tag: &str, from: usize) -> Option<usize> {
+        let n = tag.len();
+        let last = self.glyphs.len().checked_sub(n)?;
+        (from..=last).find(|&i| self.glyphs[i..i + n].iter().map(|g| g.1).eq(tag.chars()))
+    }
+
+    /// The text of glyphs `from..to`, trailing blanks dropped.
+    fn text(&self, from: usize, to: usize) -> String {
+        let text: String = self.glyphs[from..to].iter().map(|g| g.1).collect();
+        text.trim_end().to_owned()
+    }
+
+    /// The column glyph `i` starts at (the row's width past its end).
+    fn col(&self, i: usize) -> usize {
+        self.glyphs.get(i).map_or(self.width, |g| g.0)
+    }
+
+    /// The index of the first non-blank glyph.
+    fn indent(&self) -> usize {
+        self.glyphs
+            .iter()
+            .position(|g| !g.1.is_whitespace())
+            .unwrap_or(self.glyphs.len())
+    }
+}
+
 #[cfg(test)]
 mod tests {
     use super::*;
diff --git a/src/tui/ui/mod.rs b/src/tui/ui/mod.rs
index 6bba81c..ce0b2ea 100644
--- a/src/tui/ui/mod.rs
+++ b/src/tui/ui/mod.rs
@@ -313,6 +313,9 @@ fn draw_pane(frame: &mut Frame, app: &App, area: Rect) {
         return;
     };
     view.mirror.render(area, frame.buffer_mut());
+    if !view.latex.is_empty() {
+        super::latex::overlay(frame.buffer_mut(), area, &view.mirror, &view.latex);
+    }
     if let Some(sel) = app.selection.as_ref().filter(|s| s.session == view.id) {
         highlight_selection(frame.buffer_mut(), area, sel);
     }
`````
Run: `umask 022; cargo build --locked 2>&1 | grep -cE '^(warning|error)'; cargo test --locked --bin claudio -- tui::latex tui::ui::tests && make test-unit && rustfmt --edition 2021 --check src/tui/latex.rs`
Expected: `0`, then `test result: ok. 22 passed; 0 failed; 0 ignored; 0 measured; 798 filtered out`, then `test result: ok. 820 passed; 0 failed`, and no rustfmt output.

Step 3: commit
`git commit -m "feat(tui): show rendered formulas in place of their <latex> tags in the pane" -- src/tui/latex.rs src/tui/ui/mod.rs`

Interfaces this task provides to later tasks: `pub fn overlay(buf: &mut Buffer, area: Rect, screen: &Screen, formulas: &[Formula])`, `pub(super) fn text_cells(text: &str, style: Style) -> Option<Vec<Cell>>`.

### Task 7: Alt+u, the formulas popup, plain mode
Model: sonnet   Repo: /volumes/repos/claudio   Files: src/tui/keymap.rs, src/tui/interaction.rs, src/tui/plain.rs, src/tui/formulas.rs (new), src/tui/ui/formulas.rs (new), src/tui/mod.rs, src/tui/ui/mod.rs, src/cli.rs, tests/manager.rs
Goal: AC9.
- `Action::Formulas` (`"formulas"`) is bound to Alt+u before Quit, and `PLAIN_ACTIONS` grows to 5.
- `Modal::Formulas(FormulasView)` is read-only: it yields to actions, and its own key closes it.
- The popup is centered, at most 100 columns by 90% of the height, and titled `formulas`. It opens scrolled to the newest formula and lists each one's 2D rendering (else its one-line form, else its source) above its dimmed one-line source.
- With no formulas, it shows how to turn them on.
- The `--plain` help text and the plain manager test count Alt+u.

Step 1: write the failing tests (keymap, app and plain-mode tests; the popup's own tests come with its new files in Step 2).
`````diff
diff --git a/src/tui/interaction.rs b/src/tui/interaction.rs
index ac81b33..f56a809 100644
--- a/src/tui/interaction.rs
+++ b/src/tui/interaction.rs
@@ -1134,6 +1134,30 @@ mod tests {
         assert_eq!(app.sessions[0].latex_seq, 0);
     }
 
+    #[test]
+    fn alt_u_opens_and_closes_the_formulas_popup() {
+        use crate::tui::test_support::alt;
+        let mut app = app_with(&[info(Some(1), None)]);
+        app.on_terminal(alt('u'));
+        let Some(Modal::Formulas(view)) = &app.modal else {
+            panic!("no formulas popup")
+        };
+        assert_eq!(view.session, app.sessions[0].id);
+        // Its own key closes it; so does Esc.
+        app.on_terminal(alt('u'));
+        assert!(app.modal.is_none());
+        app.on_terminal(alt('u'));
+        app.on_terminal(press(KeyCode::Esc));
+        assert!(app.modal.is_none());
+        // It is read-only: another manager key closes it and acts, and a
+        // paste goes nowhere.
+        app.on_terminal(alt('u'));
+        app.on_terminal(Event::Paste("text".into()));
+        assert!(app.take_effects().is_empty(), "the paste reached the session");
+        app.on_terminal(alt('h'));
+        assert!(matches!(app.modal, Some(Modal::Help)));
+    }
+
     #[test]
     fn the_selection_is_drawn_reversed() {
         let mut app = app_with_text();
diff --git a/src/tui/keymap.rs b/src/tui/keymap.rs
index 58dd867..1e4c71f 100644
--- a/src/tui/keymap.rs
+++ b/src/tui/keymap.rs
@@ -578,6 +578,22 @@ mod tests {
             Some(Action::Reset)
         );
         assert_eq!(km.key_for(Action::Reset).as_deref(), Some("Alt+e"));
+        assert_eq!(
+            km.lookup(&key(KeyCode::Char('u'), KeyModifiers::ALT)),
+            Some(Action::Formulas)
+        );
+        assert_eq!(Action::Formulas.name(), "formulas");
+    }
+
+    /// No two default bindings of one scope share a key.
+    #[test]
+    fn default_bindings_never_collide() {
+        for (i, a) in DEFAULT_BINDINGS.iter().enumerate() {
+            for b in &DEFAULT_BINDINGS[i + 1..] {
+                let same = a.scope == b.scope && a.code == b.code && a.mods == b.mods;
+                assert!(!same, "{} and {}", a.action.name(), b.action.name());
+            }
+        }
     }
 
     #[test]
@@ -874,6 +890,7 @@ mod tests {
         assert_eq!(km.lookup(&alt('s')), Some(Action::ProxyStats));
         assert_eq!(km.lookup(&alt('l')), Some(Action::GitLog));
         assert_eq!(km.lookup(&alt('e')), Some(Action::Reset));
+        assert_eq!(km.lookup(&alt('u')), Some(Action::Formulas));
         // Every other manager key goes to claude.
         for c in ['n', 'x', 'r', 'g', 'c', 'a', 'q'] {
             assert_eq!(km.lookup(&alt(c)), None, "Alt+{c}");
@@ -893,7 +910,7 @@ mod tests {
     fn plain_help_lists_exactly_the_plain_keys_in_order() {
         let (km, _) = plain_keymap(&[]);
         let keys: Vec<String> = km.help_entries().into_iter().map(|(k, _)| k).collect();
-        assert_eq!(keys, ["Alt+h", "Alt+s", "Alt+s → n", "Alt+l", "Alt+e"]);
+        assert_eq!(keys, ["Alt+h", "Alt+s", "Alt+s → n", "Alt+l", "Alt+e", "Alt+u"]);
     }
 
     #[test]
diff --git a/src/tui/plain.rs b/src/tui/plain.rs
index 9916e9f..6f5765a 100644
--- a/src/tui/plain.rs
+++ b/src/tui/plain.rs
@@ -223,7 +223,7 @@ mod tests {
         app.take_effects();
         app.on_terminal(alt('h'));
         let text = screen(&app).join("\n");
-        for key in ["Alt+h", "Alt+s", "Alt+l", "Alt+e"] {
+        for key in ["Alt+h", "Alt+s", "Alt+l", "Alt+e", "Alt+u"] {
             assert!(text.contains(key), "{key} missing:\n{text}");
         }
         for gone in ["Alt+n", "Alt+x", "Alt+q", "Alt+g", "Alt+c", "Alt+Shift"] {
diff --git a/tests/manager.rs b/tests/manager.rs
index b77f824..6a745f2 100644
--- a/tests/manager.rs
+++ b/tests/manager.rs
@@ -1800,7 +1800,7 @@ fn test_plain_is_the_bare_session_full_screen() {
     drop(tui);
 }
 
-/// Alt+h lists exactly the four plain keys (and the stats popup's `n`); any
+/// Alt+h lists exactly the five plain keys (and the stats popup's `n`); any
 /// key closes it.
 #[test]
 fn test_plain_help_lists_only_the_plain_keys() {
@@ -1810,10 +1810,10 @@ fn test_plain_help_lists_only_the_plain_keys() {
     tui.send_keys(ALT_H);
     tui.wait_for("claudio keys", Region::Screen, WAIT);
     let screen = tui.screen_text(Region::Screen);
-    for key in ["Alt+h", "Alt+s", "Alt+s → n", "Alt+l", "Alt+e"] {
+    for key in ["Alt+h", "Alt+s", "Alt+s → n", "Alt+l", "Alt+e", "Alt+u"] {
         assert!(screen.contains(key), "{key} missing:\n{screen}");
     }
-    assert_eq!(screen.matches("Alt+").count(), 5, "exactly the plain keys:\n{screen}");
+    assert_eq!(screen.matches("Alt+").count(), 6, "exactly the plain keys:\n{screen}");
 
     tui.send_keys(ESC);
     tui.wait_until(|s| !s.contains("claudio keys", Region::Screen), WAIT);
`````
Run: `umask 022; cargo test --locked --bin claudio -- tui::keymap tui::interaction tui::plain tui::formulas tui::ui::formulas`
Expected: compilation fails with `error: could not compile `claudio` (bin "claudio" test) due to 4 previous errors`:
```
      3 error[E0599]: no variant or associated item named `Formulas` found for enum `keymap::Action` in the current scope
      1 error[E0599]: no variant or associated item named `Formulas` found for enum `interaction::Modal` in the current scope
```

Step 2: implement.
`````diff
diff --git a/src/cli.rs b/src/cli.rs
index 7559d3b..65af804 100644
--- a/src/cli.rs
+++ b/src/cli.rs
@@ -344,9 +344,10 @@ pub const HELP_APPENDIX: &str = "\n\
 \x20  --plain [--proxy NAME|--no-proxy] [--first-party] [claude args…]\n\
 \x20                              Run one plain interactive claude, full screen, with the proxy env of a\n\
 \x20                              manager session: no tabs, status bar or wizard. Alt+h (help), Alt+s\n\
-\x20                              (proxy stats), Alt+l (git history) and Alt+e (restart) are claudio's;\n\
-\x20                              every other key goes to claude. Exits with claude; the session is\n\
-\x20                              not kept. Must be the first argument; -p/--api are rejected.\n\
+\x20                              (proxy stats), Alt+l (git history), Alt+e (restart) and Alt+u\n\
+\x20                              (formulas) are claudio's; every other key goes to claude. Exits with\n\
+\x20                              claude; the session is not kept. Must be the first argument;\n\
+\x20                              -p/--api are rejected.\n\
 \x20  --fast                      Strip human-like typing/quiescence delays for lowest latency.\n\
 \x20  --log-messages              Log the full CLI⇄claudio⇄claude message flow to stderr (colorized).\n\
 \x20  --log-messages-file <path>  Also append the raw message flow to <path> as JSON Lines (untruncated).\n\
diff --git a/src/tui/formulas.rs b/src/tui/formulas.rs
new file mode 100644
index 0000000..4ff54f2
--- /dev/null
+++ b/src/tui/formulas.rs
@@ -0,0 +1,200 @@
+//! The formulas popup (Alt+u): the active session's recent `<latex>`
+//! formulas, oldest first, each rendered in 2D above its source. The view's
+//! state and keys are here, its lines too (pure); drawing is in
+//! `ui/formulas.rs`.
+
+use std::cell::Cell;
+
+use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
+
+use super::latex;
+use super::sessions::SessionView;
+use crate::proto::SessionId;
+
+/// How far PageUp/PageDown (and Space) scroll.
+const PAGE: u16 = 10;
+
+/// What the popup shows when the session has no formulas.
+pub const EMPTY: [&str; 4] = [
+    "No formulas yet.",
+    "",
+    "claude writes them in <latex> tags with `latex = true` under [claude]",
+    "in config.toml on the session's host, then `claudio daemon restart` there.",
+];
+
+/// The popup's state: whose formulas, and how far down.
+pub struct FormulasView {
+    pub session: SessionId,
+    scroll: u16,
+    /// The largest useful scroll, known once drawn (see `ui/formulas.rs`).
+    max_scroll: Cell<u16>,
+}
+
+/// One line of the popup.
+#[derive(Debug, PartialEq)]
+pub enum Line {
+    /// A row of a formula's rendering.
+    Formula(String),
+    /// A formula's source, on one line (shown dimmed).
+    Source(String),
+    /// The text shown when there are no formulas (dimmed).
+    Hint(&'static str),
+    Blank,
+}
+
+impl FormulasView {
+    /// The popup for `session`, scrolled to its newest formulas.
+    pub fn new(session: SessionId) -> FormulasView {
+        FormulasView {
+            session,
+            scroll: u16::MAX,
+            max_scroll: Cell::new(u16::MAX),
+        }
+    }
+
+    /// The scroll offset to draw at.
+    pub fn scroll(&self) -> u16 {
+        self.scroll.min(self.max_scroll.get())
+    }
+
+    /// Record the largest useful scroll offset of what is being drawn.
+    pub fn set_max_scroll(&self, max: u16) {
+        self.max_scroll.set(max);
+    }
+
+    /// Handle a key; `true` closes the popup. Ctrl and Alt chords are not
+    /// its own (a manager key reaches the app before this).
+    pub fn on_key(&mut self, key: &KeyEvent) -> bool {
+        if key
+            .modifiers
+            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
+        {
+            return false;
+        }
+        let at = self.scroll();
+        self.scroll = match key.code {
+            KeyCode::Esc | KeyCode::Char('q') => return true,
+            KeyCode::Down | KeyCode::Char('j') => at.saturating_add(1),
+            KeyCode::Up | KeyCode::Char('k') => at.saturating_sub(1),
+            KeyCode::PageDown | KeyCode::Char(' ') => at.saturating_add(PAGE),
+            KeyCode::PageUp => at.saturating_sub(PAGE),
+            KeyCode::Home | KeyCode::Char('g') => 0,
+            KeyCode::End | KeyCode::Char('G') => u16::MAX,
+            _ => at,
+        };
+        false
+    }
+}
+
+/// The popup's lines for session `v`: its formulas oldest first, each its
+/// 2D rendering (else its one-line one, else its source) then its source,
+/// with a blank line between them. A formula of a list still being rendered
+/// shows its source. With no formulas, how to get them.
+pub fn lines(v: &SessionView) -> Vec<Line> {
+    if v.latex_srcs.is_empty() {
+        return EMPTY
+            .iter()
+            .map(|t| match *t {
+                "" => Line::Blank,
+                t => Line::Hint(t),
+            })
+            .collect();
+    }
+    let mut lines = Vec::new();
+    for src in &v.latex_srcs {
+        if !lines.is_empty() {
+            lines.push(Line::Blank);
+        }
+        let source = latex::prepare(src);
+        let shown = v.latex.iter().find(|f| f.src == *src);
+        match shown.map(|f| (&f.block, &f.inline)) {
+            Some((Some(block), _)) => {
+                lines.extend(block.iter().cloned().map(Line::Formula));
+            }
+            Some((None, Some(inline))) => lines.push(Line::Formula(inline.clone())),
+            _ => lines.push(Line::Formula(source.clone())),
+        }
+        lines.push(Line::Source(source));
+    }
+    lines
+}
+
+#[cfg(test)]
+mod tests {
+    use super::*;
+    use crate::proto::SessionKind;
+    use crate::tui::latex::Formula;
+    use crossterm::event::KeyEvent;
+
+    fn view_with(srcs: &[&str], rendered: Vec<Formula>) -> SessionView {
+        let mut v = SessionView::new(
+            uuid::Uuid::nil(),
+            "local",
+            "/w",
+            SessionKind::Claude,
+            (24, 80),
+        );
+        v.latex_srcs = srcs.iter().map(|s| s.to_string()).collect();
+        v.latex = rendered;
+        v
+    }
+
+    #[test]
+    fn each_formula_shows_its_rendering_then_its_source() {
+        let frac = Formula::new(
+            "\\frac{a}{b}",
+            (None, Some(vec![" a".into(), "───".into(), " b".into()])),
+        );
+        let one = Formula::new("x^2", (Some("x²".into()), None));
+        let v = view_with(&["\\frac{a}{b}", "x^2", "y\n+ 1"], vec![frac, one]);
+        assert_eq!(
+            lines(&v),
+            [
+                Line::Formula(" a".into()),
+                Line::Formula("───".into()),
+                Line::Formula(" b".into()),
+                Line::Source("\\frac{a}{b}".into()),
+                Line::Blank,
+                Line::Formula("x²".into()),
+                Line::Source("x^2".into()),
+                Line::Blank,
+                // Not rendered yet: its source, on one line.
+                Line::Formula("y + 1".into()),
+                Line::Source("y + 1".into()),
+            ]
+        );
+    }
+
+    #[test]
+    fn no_formulas_says_how_to_get_them() {
+        let v = view_with(&[], Vec::new());
+        let text: Vec<Line> = lines(&v);
+        assert_eq!(text[0], Line::Hint("No formulas yet."));
+        assert!(text.contains(&Line::Hint(EMPTY[3])));
+    }
+
+    #[test]
+    fn keys_scroll_and_close() {
+        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
+        let mut view = FormulasView::new(uuid::Uuid::nil());
+        view.set_max_scroll(30);
+        assert_eq!(view.scroll(), 30, "opens at the newest");
+        assert!(!view.on_key(&key(KeyCode::Up)));
+        assert_eq!(view.scroll(), 29);
+        view.on_key(&key(KeyCode::PageUp));
+        assert_eq!(view.scroll(), 19);
+        view.on_key(&key(KeyCode::Char('g')));
+        assert_eq!(view.scroll(), 0);
+        view.on_key(&key(KeyCode::Char('j')));
+        view.on_key(&key(KeyCode::Char(' ')));
+        assert_eq!(view.scroll(), 11);
+        view.on_key(&key(KeyCode::End));
+        assert_eq!(view.scroll(), 30);
+        // A chord is not the popup's.
+        let alt_k = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::ALT);
+        assert!(!view.on_key(&alt_k));
+        assert_eq!(view.scroll(), 30);
+        assert!(view.on_key(&key(KeyCode::Char('q'))));
+        assert!(view.on_key(&key(KeyCode::Esc)));
+    }
+}
diff --git a/src/tui/interaction.rs b/src/tui/interaction.rs
index f56a809..c201333 100644
--- a/src/tui/interaction.rs
+++ b/src/tui/interaction.rs
@@ -19,6 +19,7 @@ use crate::term::screen::{MouseMode, Screen};
 
 use super::app::{App, Effect, Mode, ReplyTo};
 use super::confirm::ConfirmPrompt;
+use super::formulas::FormulasView;
 use super::git_view::GitView;
 use super::keymap::Action;
 use super::latex::Formula;
@@ -54,6 +55,8 @@ pub enum Modal {
     Confirm(ConfirmPrompt),
     /// The commit-history viewer (full pane).
     Git(Box<GitView>),
+    /// The active session's rendered formulas (see `formulas.rs`).
+    Formulas(FormulasView),
 }
 
 impl Modal {
@@ -62,7 +65,11 @@ impl Modal {
     pub fn yields_to_actions(&self) -> bool {
         matches!(
             self,
-            Modal::Git(_) | Modal::Overview { .. } | Modal::Help | Modal::ProxyStats { .. }
+            Modal::Git(_)
+                | Modal::Overview { .. }
+                | Modal::Help
+                | Modal::ProxyStats { .. }
+                | Modal::Formulas(_)
         )
     }
 
@@ -75,6 +82,7 @@ impl Modal {
                 | (Modal::Overview { .. }, Action::Overview)
                 | (Modal::Help, Action::Help)
                 | (Modal::ProxyStats { .. }, Action::ProxyStats)
+                | (Modal::Formulas(_), Action::Formulas)
         )
     }
 
@@ -183,6 +191,7 @@ impl App {
             Action::Overview => self.open_overview(),
             Action::Help => self.open_help(),
             Action::GitLog => self.open_git_log(),
+            Action::Formulas => self.open_formulas(),
             _ => {}
         }
         self.redraw = true;
@@ -199,6 +208,13 @@ impl App {
         self.redraw = true;
     }
 
+    /// The formulas popup of the active session (none without one).
+    fn open_formulas(&mut self) {
+        if let Some(id) = self.active_view().map(|v| v.id) {
+            self.modal = Some(Modal::Formulas(FormulasView::new(id)));
+        }
+    }
+
     pub fn modal_key(&mut self, key: KeyEvent) {
         if key.kind == KeyEventKind::Release {
             return;
@@ -299,6 +315,11 @@ impl App {
                 let outcome = view.on_key(&key, rows);
                 self.git_outcome(outcome);
             }
+            Modal::Formulas(view) => {
+                if view.on_key(&key) {
+                    self.modal = None;
+                }
+            }
         }
     }
 
@@ -330,7 +351,13 @@ impl App {
                 view.on_paste(text);
                 self.redraw = true;
             }
-            Some(Modal::ProxyStats(_) | Modal::Overview { .. } | Modal::Help | Modal::Confirm(_)) => {}
+            Some(
+                Modal::ProxyStats(_)
+                | Modal::Overview { .. }
+                | Modal::Help
+                | Modal::Confirm(_)
+                | Modal::Formulas(_),
+            ) => {}
             None => {
                 if let Some(v) = self.active_view().filter(|v| v.attached) {
                     let bytes = encode_paste(text, &v.mirror.modes());
diff --git a/src/tui/keymap.rs b/src/tui/keymap.rs
index 1e4c71f..70a85ae 100644
--- a/src/tui/keymap.rs
+++ b/src/tui/keymap.rs
@@ -37,6 +37,8 @@ pub enum Action {
     GitLog,
     /// Restart the active session in its tab (asks how).
     Reset,
+    /// Open the popup of the active session's rendered formulas.
+    Formulas,
     /// Show/hide dot-directories in the new-session wizard (wizard scope).
     ToggleHidden,
     /// Jump to tab `n` (`Alt+Shift+1`…`9` → 1…9, `Alt+Shift+0` → 10).
@@ -61,6 +63,7 @@ impl Action {
             Action::Terminal => "terminal",
             Action::GitLog => "git_log",
             Action::Reset => "reset",
+            Action::Formulas => "formulas",
             Action::ToggleHidden => "toggle_hidden",
             Action::GotoSession(_) => "goto_session",
         }
@@ -195,6 +198,16 @@ pub const DEFAULT_BINDINGS: &[Binding] = &[
         action: Action::ToggleHidden,
         scope: Scope::Wizard,
     },
+    // Alt+u: the formulas popup. Verified free in Claude Code 2.1.312 (no
+    // alt+u/meta+u in its bundle); readline's Alt+u (upcase-word) is
+    // shadowed in terminal tabs, like Alt+l and Alt+c.
+    Binding {
+        code: KeyCode::Char('u'),
+        mods: KeyModifiers::ALT,
+        label: "rendered formulas of the session",
+        action: Action::Formulas,
+        scope: Scope::Global,
+    },
     // Quit is last so it's always visible in the status bar even when truncated.
     Binding {
         code: KeyCode::Char('q'),
@@ -230,11 +243,12 @@ fn goto_session(key: &KeyEvent) -> Option<Action> {
 
 /// The only actions `claudio --plain` keeps, in help order. Everything else
 /// (tabs, wizard, rename, quit, ...) belongs to the manager and goes to claude.
-pub const PLAIN_ACTIONS: [Action; 4] = [
+pub const PLAIN_ACTIONS: [Action; 5] = [
     Action::Help,
     Action::ProxyStats,
     Action::GitLog,
     Action::Reset,
+    Action::Formulas,
 ];
 
 /// The effective keymap: defaults + config overrides.
diff --git a/src/tui/mod.rs b/src/tui/mod.rs
index d6bff3b..12d1af7 100644
--- a/src/tui/mod.rs
+++ b/src/tui/mod.rs
@@ -9,6 +9,7 @@ mod claude_update;
 mod confirm;
 mod connections;
 mod fmt;
+mod formulas;
 mod git_app;
 mod git_view;
 mod interaction;
diff --git a/src/tui/ui/formulas.rs b/src/tui/ui/formulas.rs
new file mode 100644
index 0000000..2918c44
--- /dev/null
+++ b/src/tui/ui/formulas.rs
@@ -0,0 +1,103 @@
+//! The formulas popup (Alt+u); its state and lines are in `tui/formulas.rs`.
+
+use ratatui::style::{Modifier, Style};
+use ratatui::Frame;
+use unicode_width::UnicodeWidthStr;
+
+use super::{centered, popup};
+use crate::claude::latex::sanitize;
+use crate::tui::app::App;
+use crate::tui::formulas::{self, FormulasView, Line};
+use crate::tui::latex::text_cells;
+
+/// The popup's widest: it is also at most 90% of the screen's height.
+const MAX_WIDTH: u16 = 100;
+
+/// Draw the popup over the whole screen, scrolled where `view` says. Every
+/// line is written cell by cell like the pane's formulas, and cut at the
+/// popup's edge.
+pub fn draw_formulas(frame: &mut Frame, app: &App, view: &FormulasView) {
+    let area = frame.area();
+    let width = MAX_WIDTH.min(area.width.saturating_sub(2));
+    let height = (u32::from(area.height) * 9 / 10) as u16;
+    let inner = popup(frame, centered(area, width, height), "formulas");
+    let lines = app
+        .sessions
+        .iter()
+        .find(|v| v.id == view.session)
+        .map(formulas::lines)
+        .unwrap_or_default();
+    let total = u16::try_from(lines.len()).unwrap_or(u16::MAX);
+    view.set_max_scroll(total.saturating_sub(inner.height));
+    let plain = Style::default();
+    let dim = plain.add_modifier(Modifier::DIM);
+    let shown = lines
+        .iter()
+        .skip(usize::from(view.scroll()))
+        .take(usize::from(inner.height));
+    let buf = frame.buffer_mut();
+    for (y, line) in (inner.y..).zip(shown) {
+        let (text, style) = match line {
+            Line::Formula(text) => (text.as_str(), plain),
+            Line::Source(text) => (text.as_str(), dim),
+            Line::Hint(text) => (*text, dim),
+            Line::Blank => continue,
+        };
+        let Some(mut cells) = text_cells(&sanitize(text), style) else {
+            continue;
+        };
+        cells.truncate(usize::from(inner.width));
+        // A wide character cut in half at the edge goes.
+        if cells.last().is_some_and(|c| c.symbol().width() > 1) {
+            cells.pop();
+        }
+        for (x, cell) in (inner.x..).zip(cells) {
+            buf[(x, y)] = cell;
+        }
+    }
+}
+
+#[cfg(test)]
+mod tests {
+    use crate::tui::app::{App, Modal};
+    use crate::tui::latex::Formula;
+    use crate::tui::test_support::{alt, app_with, info};
+    use ratatui::{backend::TestBackend, Terminal};
+
+    /// The screen's rows after drawing `app`.
+    fn screen(app: &App) -> Vec<String> {
+        let mut term = Terminal::new(TestBackend::new(app.width, app.height)).unwrap();
+        term.draw(|f| super::super::draw(f, app)).unwrap();
+        let buf = term.backend().buffer();
+        (0..buf.area.height)
+            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
+            .collect()
+    }
+
+    #[test]
+    fn the_popup_shows_each_formula_over_its_source_newest_in_view() {
+        let mut app = app_with(&[info(Some(1), None)]);
+        let v = &mut app.sessions[0];
+        v.latex_srcs = (0..20).map(|i| format!("x_{i}")).collect();
+        v.latex = vec![Formula::new(
+            "x_19",
+            (Some("x₁₉".into()), Some(vec!["x₁₉".into()])),
+        )];
+        app.on_terminal(alt('u'));
+        assert!(matches!(app.modal, Some(Modal::Formulas(_))));
+        let text = screen(&app).join("\n");
+        assert!(text.contains(" formulas "), "{text}");
+        // Scrolled to the bottom: the newest is shown, rendered, the oldest not.
+        assert!(text.contains("x₁₉") && text.contains("x_19"), "{text}");
+        assert!(!text.contains("x_0 "), "{text}");
+    }
+
+    #[test]
+    fn the_empty_popup_says_how_to_get_formulas() {
+        let mut app = app_with(&[info(Some(1), None)]);
+        app.on_terminal(alt('u'));
+        let text = screen(&app).join("\n");
+        assert!(text.contains("No formulas yet."), "{text}");
+        assert!(text.contains("claudio daemon restart"), "{text}");
+    }
+}
diff --git a/src/tui/ui/mod.rs b/src/tui/ui/mod.rs
index ce0b2ea..053532c 100644
--- a/src/tui/ui/mod.rs
+++ b/src/tui/ui/mod.rs
@@ -12,11 +12,13 @@ use ratatui::Frame;
 use crate::proto::{SessionKind, SessionState};
 
 mod confirm;
+mod formulas;
 mod git;
 mod stats;
 mod status;
 
 use confirm::draw_confirm;
+use formulas::draw_formulas;
 use git::draw_git;
 use stats::draw_proxy_stats;
 use status::{draw_status_machine, draw_status_session};
@@ -95,6 +97,7 @@ fn draw_modal(frame: &mut Frame, app: &App, pane: Rect) {
         Some(Modal::Help) => draw_help(frame, app),
         Some(Modal::Confirm(prompt)) => draw_confirm(frame, prompt),
         Some(Modal::Git(view)) => draw_git(frame, view, app.now, pane),
+        Some(Modal::Formulas(view)) => draw_formulas(frame, app, view),
         None => {}
     }
 }
`````
Run: `umask 022; cargo build --locked 2>&1 | grep -cE '^(warning|error)'; cargo test --locked --bin claudio -- tui::keymap tui::interaction tui::plain tui::formulas tui::ui::formulas && make test-unit && rustfmt --edition 2021 --check src/cli.rs src/tui/formulas.rs src/tui/ui/formulas.rs && cargo test --locked --test manager test_plain_help_lists_only_the_plain_keys -- --test-threads=1`
Expected: `0`, then `test result: ok. 58 passed; 0 failed; 0 ignored; 0 measured; 769 filtered out`, then `test result: ok. 827 passed; 0 failed`, no rustfmt output, then `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 42 filtered out`.

Step 3: commit
`git add src/tui/formulas.rs src/tui/ui/formulas.rs && git commit -m "feat(tui): Alt+u opens the session's formulas rendered in 2D" -- src/tui/keymap.rs src/tui/interaction.rs src/tui/plain.rs src/tui/formulas.rs src/tui/ui/formulas.rs src/tui/mod.rs src/tui/ui/mod.rs src/cli.rs tests/manager.rs`

Interfaces this task provides to later tasks: Alt+u (`ESC u`) opens a popup titled ` formulas `, which shows each formula's rendering above its source. `tests/manager.rs`'s plain-help test expects 6 `Alt+` keys.

### Task 8: manager integration test with a fake claude that writes a transcript
Model: sonnet   Repo: /volumes/repos/claudio   Files: tests/manager.rs, tests/common/mod.rs
Goal: AC10, end to end through a real daemon, plus the AC4 attach replay and the AC1 wiring. The test turns on `[claude] latex = true` and uses a fake claude that:
- saves its `--settings`;
- writes an assistant record with `<latex>E = mc^2</latex>` to `$HOME/.claude/projects/<encoded cwd>/<sid>.jsonl`;
- prints the same line, then runs its own SessionStart hook command (taken from `--settings` with sed) with `{"session_id":…}`.

The pane must show `Energy: E = mc² as promised.`, the settings must contain `__context latex`, and Alt+u must show the source `E = mc^2`. A second TUI, started after Alt+q, must show the rendered line again. Nothing changes in the conversation by then, so only the daemon's attach replay can bring it.

Step 1: write the test. It passes as soon as it is written, because Tasks 1–7 implement the behaviour. Its bite was checked in the scratch run: with `let latex = None::<SessionEvent>;` in place of `let latex = self.meta.last_latex();` in `Actor::attach` (src/daemon/session.rs), it fails with `wait_for timeout: "Energy: E = mc² as promised." not found in Pane` at the second TUI. Do not commit that mutation.
`````diff
diff --git a/tests/common/mod.rs b/tests/common/mod.rs
index fe146d8..23904dd 100644
--- a/tests/common/mod.rs
+++ b/tests/common/mod.rs
@@ -63,6 +63,7 @@ pub const ALT_H: &[u8] = b"\x1bh";
 pub const ALT_C: &[u8] = b"\x1bc";
 pub const ALT_L: &[u8] = b"\x1bl";
 pub const ALT_E: &[u8] = b"\x1be";
+pub const ALT_U: &[u8] = b"\x1bu";
 pub const ENTER: &[u8] = b"\r";
 pub const CTRL_U: &[u8] = b"\x15";
 pub const ESC: &[u8] = b"\x1b";
diff --git a/tests/manager.rs b/tests/manager.rs
index 6a745f2..6263533 100644
--- a/tests/manager.rs
+++ b/tests/manager.rs
@@ -25,10 +25,10 @@ use std::time::{Duration, Instant};
 use std::{fs, thread};
 
 use common::{
-    current_nonce, extract_nonce, run_git, wizard_pick_dir, ClaudeProjectGuard, ManagerHarness,
-    Region, TuiProcess, ALT_C, ALT_E, ALT_G, ALT_H, ALT_L, ALT_LEFT, ALT_N, ALT_Q, ALT_R,
-    ALT_RIGHT, ALT_SHIFT_1, ALT_SHIFT_2, ALT_X, BINARY, CTRL_U, DAEMON_WAIT, DOWN_ARROW, ENTER,
-    ESC, LEFT_ARROW, RECONNECT_WAIT, RIGHT_ARROW, UP_ARROW, WAIT,
+    current_nonce, encode_cwd, extract_nonce, run_git, wizard_pick_dir, ClaudeProjectGuard,
+    ManagerHarness, Region, TuiProcess, ALT_C, ALT_E, ALT_G, ALT_H, ALT_L, ALT_LEFT, ALT_N, ALT_Q,
+    ALT_R, ALT_RIGHT, ALT_SHIFT_1, ALT_SHIFT_2, ALT_U, ALT_X, BINARY, CTRL_U, DAEMON_WAIT,
+    DOWN_ARROW, ENTER, ESC, LEFT_ARROW, RECONNECT_WAIT, RIGHT_ARROW, UP_ARROW, WAIT,
 };
 use portable_pty::CommandBuilder;
 
@@ -2115,3 +2115,80 @@ fn test_wizard_recent_dir_offers_resume() {
     thread::sleep(Duration::from_millis(1500));
     tui.wait_for("resume?", Region::Screen, WAIT);
 }
+
+// ── LaTeX formulas ────────────────────────────────────────────────────────────
+
+/// A fake claude that answers with a formula, as real claude would with
+/// `[claude] latex = true`: it saves its `--settings`, writes the assistant
+/// record to its transcript (under the test's `$HOME`), prints the answer
+/// with the tag, then runs its own SessionStart hook so the daemon reads it.
+fn write_latex_fake_claude(h: &ManagerHarness, settings: &std::path::Path) {
+    let sid = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
+    let project = h.home.join(".claude/projects").join(encode_cwd(&h.dirs[0]));
+    let record = serde_json::json!({
+        "type": "assistant",
+        "message": {
+            "id": "m1",
+            "model": "claude-fake",
+            "content": [{"type": "text", "text": "Energy: <latex>E = mc^2</latex> as promised."}],
+            "usage": {"input_tokens": 1, "output_tokens": 1},
+        },
+    });
+    let script = format!(
+        r#"#!/bin/sh
+if [ "$1" = "--version" ]; then echo "claude 0.0.0-fake"; exit 0; fi
+if [ "$1" = "--help" ]; then exit 0; fi
+printf '%s' "$2" > "{settings}"
+mkdir -p "{project}"
+printf '%s\n' '{record}' > "{project}/{sid}.jsonl"
+echo "FAKE_CLAUDE_BANNER pid=$$"
+echo 'Energy: <latex>E = mc^2</latex> as promised.'
+HOOK=$(printf '%s' "$2" | sed -n 's/.*"command":"\([^"]*__hook SessionStart[^"]*\)".*/\1/p')
+printf '{{"session_id":"{sid}","source":"startup"}}' | eval "$HOOK"
+exec cat
+"#,
+        settings = settings.display(),
+        project = project.display(),
+    );
+    fs::write(&h.fake_claude, script).unwrap();
+    fs::set_permissions(&h.fake_claude, std::os::unix::fs::PermissionsExt::from_mode(0o755))
+        .unwrap();
+}
+
+/// A formula in claude's answer is rendered in place of its `<latex>` tag
+/// and listed by Alt+u; a TUI started later gets it from the daemon's
+/// replay on attach. `[claude] latex = true` gives the session the
+/// `__context latex` SessionStart hook.
+#[test]
+fn test_latex_formula_is_rendered_in_the_pane_and_the_popup() {
+    let h = ManagerHarness::new();
+    fs::write(
+        h.config_home.join("claudio/config.toml"),
+        "[claude]\nupdate_check = \"off\"\nremote_check = \"off\"\nlatex = true\n",
+    )
+    .unwrap();
+    let settings = h.root.join("settings.json");
+    write_latex_fake_claude(&h, &settings);
+
+    {
+        let mut tui = h.start_tui();
+        wizard_pick_dir(&mut tui, &h.dirs[0]);
+        tui.wait_for("Energy: E = mc² as promised.", Region::Pane, DAEMON_WAIT);
+        assert!(!tui.screen_text(Region::Pane).contains("<latex>"));
+        let hooks = fs::read_to_string(&settings).unwrap();
+        assert!(hooks.contains("__context latex"), "{hooks}");
+
+        tui.send_keys(ALT_U);
+        tui.wait_for(" formulas ", Region::Screen, WAIT);
+        tui.wait_for("E = mc^2", Region::Screen, WAIT);
+        tui.send_keys(ALT_U);
+        tui.wait_until(|s| !s.contains(" formulas ", Region::Screen), WAIT);
+        tui.send_keys(ALT_Q);
+        tui.wait_exit(WAIT);
+    }
+
+    // Nothing changes in the conversation now, so only the attach replay
+    // can bring the formula to a new TUI.
+    let tui = h.start_tui();
+    tui.wait_for("Energy: E = mc² as promised.", Region::Pane, DAEMON_WAIT);
+}
`````
Run (alone on the machine): `umask 022; cargo test --locked --test manager test_latex -- --test-threads=1`
Expected: `test test_latex_formula_is_rendered_in_the_pane_and_the_popup ... ok` and `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 43 filtered out` (about 5 s).

Step 2: implement. Nothing to implement. Run the whole manager suite alone: `umask 022; cargo test --locked --test manager -- --test-threads=1`
Expected: `test result: ok. 44 passed; 0 failed` (about 2 minutes; the base has 43).

Step 3: commit
`git commit -m "test(manager): a formula renders in the pane and the popup, and replays on attach" -- tests/manager.rs tests/common/mod.rs`

Interfaces this task provides to later tasks: `common::ALT_U`.

### Task 9: docs
Model: haiku   Repo: /volumes/repos/claudio   Files: README.md, docs/manager-design.md, CLAUDE.md
Goal: AC12 (the config header comment was done in Task 1; the binary size delta is reported at release).
- README: the `[claude] latex` key and the restart note, what is rendered, Alt+u and its readline note, plain mode's fifth key, the config example.
- docs/manager-design.md §2.7: the hook, the event with `seq`, extraction, the render worker, the overlay and the popup.
- CLAUDE.md: the architecture map (`latex.rs`, `formulas.rs`, the render worker, `__context`, the `Latex` event) and a new non-obvious constraint.

Step 1: no test. Check that the docs do not already describe it: `grep -c "Alt+u" README.md` prints `0`.

Step 2: implement.
`````diff
diff --git a/CLAUDE.md b/CLAUDE.md
index 1f9469f..9017be7 100644
--- a/CLAUDE.md
+++ b/CLAUDE.md
@@ -118,6 +118,11 @@ daemons coexist.
   turns) off the actor task: on hooks (tool-use bursts share one refresh)
   and every 10 s while a client is attached; one refresh runs at a time. The
   transcript is read incrementally (`claude::projects::TranscriptCursor`).
+  The refresh also sends the conversation's `<latex>` formulas as
+  `SessionEvent::Latex { seq, formulas }`: the whole list, only when it
+  changed (`seq` + 1; a new conversation starts over at 1). The last list is
+  replayed to a client right after its `Attached` + snapshot. `command()`
+  adds the `__context latex` SessionStart hook with `[claude] latex`.
 - `git_status.rs` — `git status --porcelain=v2 --branch` for the status
   bar's `⎇` segment: 1 s deadline, capped output, same scrubbed env as
   `git.rs`.
@@ -152,7 +157,11 @@ is idle (checked every 2 min).
 - `mod.rs` — I/O edge: owns the terminal, runs the event loop, gathers the
   `AppConfig`, drives daemon connections and every `Effect`. Emits OSC 9
   notifications (`ESC ] 9 ; … BEL`) for background sessions that need
-  attention.
+  attention. Runs the `latex-render` worker thread (`Effect::RenderLatex` →
+  `HostEvent::LatexRendered`). It renders each formula on a scratch thread
+  (32 MiB stack) within a 150 ms budget, at most 2 abandoned at a time, and
+  caches by source (LRU 1024). Its panic hook stays silent for panics on
+  threads flagged `QUIET_PANIC`.
 - `app.rs` — pure coordinator (`App::new(AppConfig)`): processes events into
   state transitions and returns `Effect` values. No I/O here; all I/O is in
   `mod.rs`. `Mode::Manager` or `Mode::Plain`.
@@ -165,17 +174,24 @@ is idle (checked every 2 min).
 - `git_view.rs` + `git_app.rs` — the git viewer (Alt+l): pure view state
   machine, and its `App` glue.
 - `stats_view.rs` — the multi-page proxy stats popup (Alt+s).
+- `latex.rs` — `<latex>` formulas, pure: `Formula` (source, one-line and 2D
+  renderings, match keys), the `too_complex` pre-check, `render` (term-maths
+  plus a one-line walk of the rust-latex-parser tree), and `overlay`. The
+  overlay swaps matched on-screen tags for their rendering in the pane's
+  buffer, cell by cell, and moves no row.
+- `formulas.rs` — the formulas popup (Alt+u): view state, keys and lines;
+  drawn by `ui/formulas.rs`.
 - `confirm.rs` — the generic confirm prompt (kill, reset, claude update).
 - `claude_update.rs` — local/remote claude version checks and updates.
 - `plain.rs` — `claudio --plain`: `Mode::Plain` is one local, ephemeral
   session full screen (no tab/status bar, wizard or state.json).
-  `Keymap::build_plain` keeps only Alt+h/s/l/e; the session is killed when
+  `Keymap::build_plain` keeps only Alt+h/s/l/e/u; the session is killed when
   the UI exits, and the UI exits with claude.
 - `connections.rs` — local + SSH connections, reconnect backoff.
 - `ui/` — ratatui rendering: `mod.rs` (layout, tab bar, pane, popups),
   `status.rs` (two-line bottom bar; line 2 is prioritised segments — cpu,
   ctx gauge, git, mem, 5h, conversation, 7d — dropped whole when narrow),
-  `stats.rs`, `git.rs`, `confirm.rs`.
+  `stats.rs`, `git.rs`, `confirm.rs`, `formulas.rs`.
 - `fmt.rs` — shared text and number formatting.
 - `keymap.rs` — `DEFAULT_BINDINGS` table (the single source of truth); `Keymap`
   (defaults + config overrides); `parse_key_spec`; collision detection.
@@ -232,7 +248,12 @@ is idle (checked every 2 min).
 - `projects.rs` — scan `~/.claude/projects/` for existing sessions (feeds the
   wizard's resume list).
 - `hooks.rs` — `__hook <Event> <socket> <token>` relay: sends the hook payload
-  to the daemon socket.
+  to the daemon socket. Also `__context latex`, the SessionStart hook that asks
+  claude to wrap math in `<latex>` tags (`[claude] latex`); it prints one
+  constant JSON line and never touches the socket.
+- `latex.rs` — `extract_latex` (the formulas of an assistant text, code
+  excluded) and `sanitize`, the character filter every formula goes through
+  in the daemon and again in the client.
 - `state.rs` — session state tracker derived from hook events.
 - `update.rs` — version parsing and the daily release-channel check.
 
@@ -275,6 +296,11 @@ is idle (checked every 2 min).
 - **`app.rs` is pure; I/O is only in `tui/mod.rs`.** `App::update` takes
   events and returns `Effect` values. `mod.rs` drives the effects. Don't add
   I/O to `app.rs`.
+- **Formulas are never rendered on the UI thread.** On hostile input inside
+  the 2048-byte cap, rust-latex-parser can take a minute or overflow a
+  2 MiB stack. Call `latex::render` only through `render_formula` in
+  `tui/mod.rs` (pre-check, scratch thread, budget, abandon cap), never from
+  `App`, a draw or a test thread.
 
 ### Print-mode pipeline (`-p`)
 
diff --git a/README.md b/README.md
index 9552562..545eb4c 100644
--- a/README.md
+++ b/README.md
@@ -101,7 +101,7 @@ claudio                                         # new sessions now use the proxy
 claudio --plain [--proxy NAME | --no-proxy] [--first-party] [claude args…]
 ```
 
-One full-screen claude session with the proxy configured: no tabs, no status bar, no saved state. It feels like plain `claude`, and exits when claude does, with claude's exit status. Only four keys are claudio's: `Alt+h` help, `Alt+s` proxy stats, `Alt+l` git history, `Alt+e` restart. Everything else, and every argument, goes to claude.
+One full-screen claude session with the proxy configured: no tabs, no status bar, no saved state. It feels like plain `claude`, and exits when claude does, with claude's exit status. Only five keys are claudio's: `Alt+h` help, `Alt+s` proxy stats, `Alt+l` git history, `Alt+e` restart, `Alt+u` formulas. Everything else, and every argument, goes to claude.
 
 ### Everyday commands
 
@@ -196,6 +196,14 @@ A multi-page popup for the session's claude-proxy profile:
 
 `←`/`→` or `1`–`5` switch pages, `w` changes the time window, and `r` refreshes. `n` asks the proxy to move the session to its next subscription (after a confirmation); the switch takes effect on the next message and rebuilds the prompt cache.
 
+### Formulas (`Alt+u`)
+
+With `latex = true` under `[claude]` in `config.toml`, each claude session is told (by a `SessionStart` hook) to write math as LaTeX inside `<latex>…</latex>` tags. The setting belongs to the host that runs the session, and its daemon reads it when it starts: after changing it, run `claudio daemon restart` on that host.
+
+In the pane, each tag is replaced by a one-line Unicode rendering in cyan, such as `E = mc²` or `x = (-b ± √(b² - 4ac))/(2a)`. Nothing else on the screen moves, and selecting still copies the original text. A formula claudio cannot render, or does not recognise on screen, stays as written. The formulas come from the conversation's transcript, not from the screen (Claude Code's markdown drops backslashes there), so they render too when you ask claude for the tags yourself, without the setting.
+
+`Alt+u` opens the session's last 64 formulas, oldest first, each drawn in 2D above its LaTeX source. `↑`/`↓`, `PgUp`/`PgDn` and `g`/`G` scroll; `Esc` closes. In a terminal tab, `Alt+u` shadows readline's upcase-word.
+
 ### Keeping claude up to date
 
 Once a day claudio compares your `claude --version` with the latest release on your update channel, and asks before running `claude update`. When an SSH host connects, its claude is compared with yours. If it is older, claudio offers to update it; if it is missing, claudio offers to install it with the official installer. Either way it asks first. Prompts offer `[y] update`, `[n] not now` and `[s] skip this version`. Running sessions keep their binary; new sessions use the updated one.
@@ -220,6 +228,7 @@ Manager keys use `Alt`, so they don't clash with Claude Code. In terminal tabs t
 | `Alt+g` | Overview of all sessions |
 | `Alt+s` | Proxy stats |
 | `Alt+s` → `n` | Move the session to the next proxy subscription (asks first) |
+| `Alt+u` | The session's formulas, rendered (see [Formulas](#formulas-altu)) |
 | `Alt+h` | Help |
 | `Alt+.` | In the wizard: show or hide dot-directories |
 | `Alt+q` | Quit the UI (sessions keep running) |
@@ -267,12 +276,15 @@ check = true                    # check for new claudio releases
 allow_skip_permissions = true   # start claude with --allow-dangerously-skip-permissions
 update_check = "ask"            # this machine's claude vs its release channel: "ask" | "auto" | "off"
 remote_check = "ask"            # SSH hosts' claude vs this machine's: "ask" | "auto" | "off"
+latex = false                   # ask claude to write math in <latex> tags, rendered in the pane
+                                # (this host's sessions; `claudio daemon restart` after a change)
 
 [keys]                          # rebind any manager key
 new_session = "alt+n"
 terminal    = "alt+c"
 git_log     = "alt+l"
 reset       = "alt+e"
+formulas    = "alt+u"
 ```
 
 `CLAUDIO_NO_UPDATE_CHECK=1` disables both the claudio and the claude release checks.
diff --git a/docs/manager-design.md b/docs/manager-design.md
index 5edb4eb..c7eb7f6 100644
--- a/docs/manager-design.md
+++ b/docs/manager-design.md
@@ -368,6 +368,50 @@ the proxy directly. Quota signals come from `/me/stats` (`blocked_until`).
 - JSON control frames tolerate minor skew, so not every version bump is an
   upgrade.
 
+### 2.7 LaTeX formulas
+
+Spec: `docs/specs/2026-11-04-latex-render-design.md`.
+
+- **Instruction.** With `[claude] latex = true` (read at daemon start, per
+  host), `hooks::settings` gives `SessionStart` a second command,
+  `<claudio> __context latex`. It prints one `hookSpecificOutput` line whose
+  `additionalContext` asks claude to wrap math in `<latex>…</latex>`. This is
+  context, not the system prompt, so the session stays a plain interactive
+  one. The matcher `*` re-injects it after `/clear`, a resume and a compact.
+- **Extraction.** `TranscriptCursor` keeps the conversation's last 64
+  formulas. They come from the `text` blocks (or string `content`) of
+  non-sidechain assistant records, outside code fences and inline code. Each
+  is sanitized (no `Cc` but `\n`/`\t`, no `Cf`, private use or
+  noncharacters, no orphan combining marks) and capped at 2048 bytes.
+  Extraction runs whatever the setting says.
+- **Event.** After a refresh that changed the list, `MetaRefresher` sends
+  `SessionEvent::Latex { seq, formulas }`, always the whole list. `seq`
+  grows with every change, and a new conversation starts over at 1. The
+  last list is replayed to a client right after its `Attached` + snapshot,
+  since it is not broadcast again. A client keeps only a list newer than its
+  own, so a replay and a broadcast may cross. It resets on `ClaudeSession`
+  and `ClaudeSessionCleared`. Old clients decode the event as `Unknown`.
+- **Rendering.** `App` never renders: a newer list becomes
+  `Effect::RenderLatex`. The `latex-render` thread renders each uncached
+  source (LRU cache, 1024 entries) on a scratch thread with a 32 MiB stack.
+  A pre-check turns away deep or token-heavy input first (brace depth 32;
+  256 `\commands`, `/` and bareword `(`). The worker then waits 150 ms. A
+  render over budget or one that panics is shown raw. An over-budget thread
+  is abandoned, and while two still run, new sources stay raw and uncached.
+  The TUI's panic hook ignores panics on scratch threads (`QUIET_PANIC`), so
+  a caught renderer panic never restores the terminal. Results come back as
+  `HostEvent::LatexRendered` and are kept only for the `seq` held.
+- **Pane.** `latex::overlay` runs after `Screen::render`. It locates each
+  `<latex>…</latex>` on the visible rows: one row, or up to 8 when the tag
+  is alone on its first row and continues on indented rows. It matches the
+  tag to the newest formula by key: strict (backslash escapes undone,
+  whitespace dropped), then loose (also no `*_~` and backticks), then
+  without HTML-looking runs. It rebuilds the row cell by cell with the
+  one-line rendering. A row that would not fit stays raw, no row moves, and
+  the mirror (selection, copy) is untouched.
+- **Popup.** `Alt+u` (`Modal::Formulas`, read-only) shows the list in 2D
+  (term-maths), each formula above its source.
+
 ## 3. UX
 
 ### 3.1 Layout
`````
Run: `grep -c "Alt+u" README.md; grep -c "__context latex" CLAUDE.md docs/manager-design.md; grep -n "### 2.7 LaTeX formulas" docs/manager-design.md`
Expected: `4`; `CLAUDE.md:2` and `docs/manager-design.md:1`; one line `…:371:### 2.7 LaTeX formulas`.

Step 3: commit
`git commit -m "docs: <latex> formulas, [claude] latex and Alt+u" -- README.md docs/manager-design.md CLAUDE.md`

Interfaces this task provides to later tasks: none.

## Merge and release
1. Full suites, on the tip of `feat/latex-render`, each alone on the machine:
   - `umask 022; make test-unit` → `test result: ok. 827 passed; 0 failed` with no `warning:` lines. On a known base flake (see Global Constraints), rerun once.
   - `umask 022; cargo test --locked --test manager -- --test-threads=1` → `test result: ok. 44 passed; 0 failed` (about 126 s in the scratch run).
   - `make` → a clean release build. Then `stat -c %s target/release/claudio`. The scratch run measured 11,631,232 bytes against 11,324,032 at v0.3.7: +307,200 bytes (+2.7%). Report the real figure in the gate audit and the release notes (AC12).
   - `make static` → `statically linked` (it was in the scratch run, with no warnings).
2. AC11, by the controller, with the branch's `target/release/claudio` (before any merge):
   - Local: `[claude] latex = true` in `~/.config/claudio/config.toml`, then `target/release/claudio daemon restart`; start `target/release/claudio`, open a new session and ask "what is the quadratic formula?". The answer shows a cyan one-line formula in place of the `<latex>` tag, and Alt+u shows it in 2D above its source.
   - `ssh z6` (owner-approved test host): connect from that same TUI so the branch binary is uploaded; set the same key in z6's `~/.config/claudio/config.toml` and run `claudio daemon restart` on z6. The same check passes in a z6 session.
   - With the key off (or removed) and the daemon restarted, a new session's `--settings` has no `__context` command (`ps -o args= -p <claude pid>` shows the `--settings` JSON), and asking claude whether it was told about `<latex>` tags gets "no".
   - An old client (v0.3.7 binary) on the new daemon keeps working (the `Unknown` event).
   Record the four results in progress.md; a failure here is a fix task, not a release note.
3. Fable gate audit of the branch (Lifecycle 8); fixes re-reviewed.
4. **Owner OK required** to merge. Merge: `feat/latex-render` → `main`, fast-forward or a merge commit (the owner's choice). `main` is 76c2b27 and the branch is linear on top of it.
5. Release v0.4.0 (a feature release):
   - Set `version = "0.4.0"` in Cargo.toml, run `cargo update --workspace --offline` (updates only claudio's own entry) and check `git diff --stat` shows `Cargo.lock | 2 +-` and `Cargo.toml | 2 +-`. Then `cargo build --locked` must succeed.
   - Commit `git commit -m "release v0.4.0" -- Cargo.toml Cargo.lock`, with no trailer.
   - Tag: `git tag v0.4.0`. The release workflow checks that the tag matches Cargo.toml.
6. **Owner OK required before:**
   - pushing `main`;
   - pushing the tag `v0.4.0`.

## Scratch verification record
- Where: `~/.cache/claudio/scratch/plan-latex-replay`, a fresh clone at 5ef0aed. The tasks were applied in order by script: Step 1 diff → Step 1 command → Step 2 diff → Step 2 commands → the Step 3 commit command. Every diff block in this plan is the `git diff` of that run, and every "Expected" line is copied from its output. Build output went to `~/.cache/claudio/scratch/plan-latex/target`.
- Verified, failing then passing: Tasks 1–7, all Step 1 and Step 2 blocks. Unit totals 787 → 796 → 799 → 804 → 810 → 820 → 827. No warnings in `cargo build --locked` after Tasks 4–7, and no test-build warnings.
- Task 8: the test passes (1 passed); with the attach replay removed it fails at the second TUI (checked). The full manager suite has 44 passed, 0 failed.
- Task 9: the docs diff applied, and the grep checks give the stated counts.
- Release: `make` passed (size above), `make static` gave a statically linked binary, and `cargo test --locked --no-run` builds every test target. The `screenshots` target warns about `maybe_accept_trust_dialog`, which predates this work.
- Not verified here: macOS (CI builds it; nothing here is platform-specific beyond what claudio already uses), the release workflow, real claude (AC11 is manual), and `ssh z6`.
- Flakiness: across 10 runs of the final unit suite, one failed, in `daemon::host::tests::probe_runs_claude_with_the_given_path`. Across 8 runs of the base, one failed in the same test. The first version of the abandonment test burned CPU in two parse threads and seemed to make timing tests fail more often, so it now uses sleeps (`slow_renders_are_abandoned_up_to_a_cap`). Only one real slow parse remains (`"x/"` × 255 in the hostile corpus): it times out and is abandoned after about 2.4 s of parsing in a debug build.

## Spec deviations needing a ruling
- **D1. `\sqrt[n]{x}` → `ⁿ√…` (spec §3) is not possible with this parser.** rust-latex-parser 0.1.1 has no root index. `\sqrt[3]{x}` parses as `Sqrt(Text("["))` followed by `3]x` (checked: the 2D block is `√[3]x` under a bar), so the walker renders `√[3]x`. Options: (a) drop the clause from the spec (the plan's behaviour), or (b) rewrite `\sqrt[n]{…}` to `\sqrt{…}` plus a prefix before parsing, a small pre-pass in `latex::render`. Recommendation: (a).
- **D2. Restarting `seq` at 1 for a new conversation (spec §2) leaves a stale-list window.** Suppose a client attaches while a `/clear` is being processed. The actor may read the old list (seq N) for the replay just before the refresh resets the slot. If that replay reaches the client after its `ClaudeSession` reset (to 0), the client holds the old list at seq N and ignores the new conversation's lists until their seq exceeds N. A respawn or a daemon restart is not affected: a new actor's lists always follow its `ClaudeSession`, and a reconnecting manager rebuilds its views. The plan implements the spec as written (`if new_cursor { *sent = None; }` in `latex_event`).
  Proposed fix, if accepted: let `seq` grow for the actor's whole life, so a new conversation's lists, its empty one included, always outrank anything sent before.
  - In src/daemon/session.rs, delete those three lines and the `new_cursor` parameter (and its computation and argument in `MetaRefresher::refresh`).
  - In the test, the last three assertions become `assert_eq!(latex_event(&mut sent, &formulas(&[])), latex(5, &[]));` and `assert_eq!(latex_event(&mut sent, &formulas(&["d"])), latex(6, &["d"]));`, and the `assert_eq!(sent, None)` line goes.
  - The client side is unchanged: it still resets on `ClaudeSession`, and any later seq is accepted.
  Recommendation: accept.
- **D3. The fraction rule needs a precise reading.** The spec says "parentheses around a numerator or denominator that is not a single atom", but its own example `n(n+1)/2` keeps a non-atom numerator bare. The plan brackets a numerator only when it has a top-level space or `/`, and a denominator unless it is a number, one symbol (with scripts or a `√`), or already bracketed. That gives `x = (-b ± √(b² - 4ac))/(2a)` and `∑ᵢ₌₁ⁿ i = n(n + 1)/2`. The parser keeps ASCII `-` and spaces its operators, so the outputs read `b² - 4ac`, not the spec's `b²−4ac`.
- **D4. Matrices.** The spec's `[a b; c d]` is used for every kind except `vmatrix` (`|a b; c d|`) and `Vmatrix` (`‖…‖`). A determinant or a norm shown in brackets would change its meaning.
- **D5. Two cases the spec leaves open, filled conservatively.** A tag that continues onto rows containing another `<latex>` stays raw, and those rows are processed on their own. A grapheme cluster wider than two columns leaves its row raw, like overflow.
- **D6. AC4 asks for "daemon tests" of the attach replay.** An in-process daemon test cannot put a transcript where the daemon looks without changing the process-global `HOME`/`CLAUDE_CONFIG_DIR`. That would race `claude::projects`' own test, which sets and removes `CLAUDE_CONFIG_DIR`. The plan instead covers it with:
  - the unit test `an_attach_gets_the_last_formulas_right_after_the_snapshot`, which checks frame order through `Subscriber::attach`, the function `Actor::attach` calls;
  - the pure `latex_event` test;
  - Task 8's manager test, whose second TUI can only get the formula through the replay (checked by mutation).
