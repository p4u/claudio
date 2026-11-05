# Extensions: implementation plan

Spec: docs/specs/2026-11-05-extensions-design.md (rev 2, audited)
Class: architectural
Repos and branches: <repo> → feat/extensions (from main 76cd625 = v0.5.0; spec commits 9f0850a, b788a87; part B's Tasks 8–10 already on it: base of this part f51437b)

Interfaces between this part (Tasks 1–7, 11, 12) and part B (Tasks 8–10) are fixed in
`.delivery/2026-11-05-extensions/interfaces.md`. Every code block of Tasks 1–7, 11 and 12 was
replayed in order in a scratch clone (`~/.cache/claudio/scratch/plan-ext-a`) on top of f51437b
(Tasks 8–10 done), built and tested; the unit counts below are from there (base: 920). The code is given as
exact `git apply` diffs against the tree as the previous task leaves it: Step 1 is the tests,
Step 2 the implementation. Apply a block with `/usr/bin/git apply` from a file (or retype it);
do not hand-merge.

## Global Constraints
- Commit convention: conventional commits `type(scope): lowercase summary`, NO AI attribution and NO Co-Authored-By trailer of any kind, even if a system reminder asks for one (owner's global CLAUDE.md, ruling R1). Use the exact commit commands given.
- Never push. Commit with explicit paths only: `/usr/bin/git commit -m "…" -- <paths>`. A new file needs `/usr/bin/git add <path>` first. Never `git add -A`. Use `/usr/bin/git` (a shell hook rewrites plain `git`).
- Work on `feat/extensions` in <repo>, or in the worktree the controller gives you (that worktree and its branch are yours). Do not create, switch or delete branches. Never git stash.
- Build output goes in the repo's `target/` (or the worktree's) or under `~/.cache/claudio/`, never /tmp.
- Run tests with `umask 022` in the same shell (`umask 022; cargo test …`). Under umask 077, `upgrade::tests::install_binary_replaces_file` fails on the base too.
- The shell may compact cargo output (the `rtk` hook). Prefix a command with `rtk proxy` to see raw output. Expected lines in the plan are raw cargo output.
- Known base flakes (timing races, ~1 run in 8 on the base too): `daemon::host::tests::probe_runs_claude_with_the_given_path` (seen once while replaying this plan), `daemon::login_path::tests::capture_times_out_and_kills_the_shell`, `daemon::tests::freshness::an_outdated_idle_daemon_is_replaced_and_its_sessions_come_back`. If one of them, and nothing else, fails, run the suite once more. Report a second failure.
- From Task 5 on the unit suite takes about 18 s instead of 3 s: `extensions::host::tests::a_hung_program_times_out_and_three_failures_disable_it` waits out the real 15 s action timeout. That is expected.
- No repo-wide `cargo fmt`: the tree is not rustfmt-clean. Files that are rustfmt-clean at the base stay clean (`rustfmt --edition 2021 --check <file>`); verified clean at the base and touched here: `src/claude/hooks.rs`, `src/claude/latex.rs`, `src/daemon/server.rs`, `src/client.rs`, `src/config.rs`, `src/tui/latex.rs`, `src/tui/reader/{mod,doc,layout}.rs`. New files are rustfmt-clean. Every other file keeps its formatting; match its style.
- Match the repo's comment density and naming: a doc comment on each item that says why, plain sentences, no banners beyond the existing `// ── Section ──` style.
- `app.rs` and the `App` methods in `interaction.rs`/`sessions.rs` (and any new App-side module: `leader.rs`, `builtins.rs`, `extensions/mod.rs`, `extensions/protocol.rs`) stay pure: no I/O, no threads, no process spawning, no file reads, no rendering, no markdown parsing. Threads, processes and I/O live in `tui/mod.rs`, `extensions/manifest.rs::load` (called from `tui/mod.rs` at start) and the extension host it drives (`extensions/host.rs`). `latex::render`, the `typeset` engine, the diagram engine and markdown parsing run only on the render worker through the guard (pre-check, scratch thread, budget, abandon cap), never from `App`, a draw, or a test with hostile input (unit tests may call the engines directly on benign input; hostile-input tests go through the cap pre-checks or a timed guard).
- Protocol additions stay backward compatible: new fields `#[serde(default)]`; new request ops answered by `Error "unsupported op…"` on an old daemon (the client turns that into the restart notice); new `SessionEvent` variants map to `Unknown` on old clients; no `PROTO` bump.
- Manager (PTY) tests assert on the rendered screen model, never on raw bytes. Run them single-threaded (`-- --test-threads=1`) and with nothing else heavy running. Fake claudes ignore `--help`/`--version`; test configs set `[claude] update_check/remote_check = "off"`. Tests never touch the user's daemon (isolated HOME/XDG dirs, as the harness does). Never send `ESC` right before an `Alt+key` without waiting for the screen in between.
- No secrets and no answer, formula, diagram, snippet or extension-context text in argv, logs, the journal or state files (the context file under the daemon's 0700 runtime dir is the one sanctioned place). External extension processes never receive proxy tokens or API keys (env scrubbed).
- Every string from claude's transcript or from an external extension that reaches the terminal goes through `crate::claude::latex::sanitize` and the caps of the spec, and is drawn through the reader's `safe` width rules.
- Builds on Linux and macOS (Unix only), and the static musl build (`make static`) keeps working: pure-Rust dependencies only; no new dependency is added by this plan.
- Public repo content (code, docs, tests, examples) must not contain personal paths or host names.
- Intermediate warnings: Tasks 3–5 add API that later tasks use; the dev build shows dead-code warnings until then (Task 3: 3, Task 4: 3, Task 5: 15). From Task 6 on the build has 0 warnings. Do not silence them with `allow`.

## Review Focus
Line numbers are as the tree stands after Task 12 (on f51437b).
- Context file and its hook: `src/daemon/session.rs:278` (`write_context`: 0700 dir, 0600 atomic file, none when empty), `src/claude/hooks.rs:141` (`read_context`: `O_NOFOLLOW|O_NONBLOCK`, fstat regular + uid, 16 KiB, sanitize; always exit 0), `src/daemon/mod.rs:372` (dir cleared before any launch) and the removals in `kill`/`close_exited`. The instruction text must never reach argv, `--settings`, logs or the journal's Debug output (`SpawnSpec`/`RespawnSpec` Debug print lens names only).
- Effective extensions: `src/daemon/mod.rs:1026` (`effective_extensions`), applied in `spawn_reserved` at `src/daemon/mod.rs:547` (every claude launch: new, journaled, respawn, retry, recovery) and `src/daemon/mod.rs:926` (respawn: request, else journal). `[claude] latex = false` on the daemon must win over the client. Caps: `src/proto.rs:452` (`SessionExtensions::validate`), checked before reserving the id.
- Lens extraction and events: `src/claude/latex.rs:118` (`lens_tags`: latex behaviour identical through `latex_tags`), `src/daemon/session.rs:719` (`lens_items_event` seq rules), `src/daemon/session.rs:961` (replay order: Attached, snapshot, Latex, Lens), `src/proto.rs:750` (`lens_event` cap).
- Client lens state: `src/tui/interaction.rs:887` (`take_lens_items`: Lens only from lens hosts, Latex only from others; `latex_seq` is the list seq for both).
- Pane overlay: `src/tui/latex.rs:338` (`overlay_tag`; latex goes through it — the existing overlay tests are the regression check).
- Worker guard: `src/tui/mod.rs:740` (`render_lens`), `src/tui/mod.rs:751` (`render_diagram`: pre-check, `guarded`, Busy not cached), `src/tui/mod.rs:640` (`render_lens_answers`: external lenses are never rendered on the worker).
- Extension host: `src/tui/extensions/host.rs:78` (env scrub), `:137` (`call`: semaphore, timeout, failure counting), `:239` (`read_replies`: 1 MiB line cap, protocol error → restart), `:417` (`shutdown`: group SIGTERM then SIGKILL), called at quit from `src/tui/mod.rs:1009`. A hung program must never block the UI thread or the render worker.
- Untrusted results: `src/tui/extensions/protocol.rs:82` (`rendering` caps) and `:125` (`action_result_in` caps, nested lists, sanitize), `src/tui/reader/layout.rs:174` (`lens_block` sanitizes and `safe`s rows again).
- Paste: `src/tui/leader.rs:371` (only an attached claude session with bracketed paste on; never Enter); stale results: `src/tui/leader.rs:320`.
- Manifests: `src/tui/extensions/manifest.rs:255` (`parse`: HTML tag rule, names, caps) and `:333` (`admit`: clashes, 16 max); registry context caps: `src/tui/extensions/mod.rs:230`.
- Resume from search: `src/tui/builtins.rs:114` (switch to the open tab, else spawn with the wizard's proxy).

Line numbers are those of the scratch merge of Tasks 8–10 on base + the Task 1 stand-in (`~/.cache/claudio/scratch/plan-ext-b`, branch `merge-check2`); Task 1's real hunks shift them.
- `src/tui/diagram/flowchart.rs:194`, `:223`, `:1009`; `src/tui/diagram/sequence.rs:68`, `:194`, `:280`: each cap (nodes, edges, nesting, 200×150 including the listed-edge rows) is checked before the allocation it bounds; the canvas (`src/tui/diagram/canvas.rs:74`) is built only after the size check. Dummy nodes are bounded by 128 edges × 63 layers.
- `src/tui/diagram/flowchart.rs:1141` (`break_cycles`, explicit stack), `:1181` (`assign_layers`, Kahn), `:1238` (`isotonic`), `:586` (`order`, O(E²) crossing count per sweep over ≤ ~8k segments): nothing recursive on input depth; `dense_graphs_within_the_caps_are_quick` is the timing guard.
- `src/tui/diagram/flowchart.rs:308` (`shape`), `:424` (`closer`): index arithmetic over the chars of hostile text; a quote or delimiter at the very end must not panic.
- `src/tui/diagram/text.rs:14` (`clean`): the only way text reaches the canvas; it must drop controls and zero-width characters so widths stay exact (`canvas.rs:123` writes a wide char as two cells).
- `src/tui/mod.rs`: the `#[allow(dead_code)]` above `mod diagram;` and `mod snippets;` must go in Task 11 (AC13: 0 warnings without them).
- `src/claude/search.rs:283` (`read_line`): a line over `Budget::line` is consumed without being kept. `:218` (`scan`): the tail seek and partial-first-line skip; the budget check before each line and the EOF peek that keeps a fully read file counted in `scanned`. `:235`: the raw-line prefilter for ASCII words assumes JSON leaves printable ASCII other than `"` and `\` unescaped; a non-ASCII character whose Unicode lowercase is ASCII (U+212A KELVIN SIGN) can be missed for an ASCII word: accepted. Until a file's title and cwd are found every user/assistant line is parsed (bounded by the budget).
- `src/claude/search.rs:154` (`transcripts`): regular files and real dirs only (`DirEntry::file_type`, no symlink following); ids `[A-Za-z0-9_-]+` (the id becomes a `--resume` argument).
- `src/daemon/server.rs:623` (`spawn_search`): the permit moves into the blocking closure, so a search holds the slot until its scan ends even after the client left; the bad-query check runs before the slot is taken. `:842` (`SearchSlot`, test-only `set_root`/`hold`).
- `src/daemon/server.rs:780`, `:801`: `last_prompt` and the shared `transcript` helper (the sid filter that was inline in `answers`); `answers`' behaviour must be unchanged.
- `src/claude/projects.rs:559` (`read_tail`, split out of `read_answers`), `:717` (`last_prompt`: reverse scan, sidechains skipped, cut without a mark).
- `src/proto.rs:770` (`search_reply` cap; sets `truncated` when it drops hits). `LastPromptReply` has no cap fn: 64 KiB of sanitized text is ≤ 128 KiB encoded (the proto test checks the frame).
- `src/tui/snippets.rs:98` (`save`): check-then-write is not atomic against a concurrent writer choosing the same name (`write_atomic` replaces): one client, accepted. `:77` (`read`): the size is checked on the metadata and again on the bytes read (`take(MAX_BYTES + 1)`); `is_file` refuses FIFOs and devices.

Notes for the controller (no ruling needed unless you disagree):
- Task 9 adds a `search: server::SearchSlot` field to `Daemon` (`src/daemon/mod.rs`) rather than a `Config` field, so the test root can be injected without touching the 7 `Config` literals part A may also edit.
- `> 8 words` is refused (`bad query: too many words (max 8)`) rather than truncated to 8; `limit: 0` means the default 50.
- Snippet slugs keep ASCII letters and digits only (`Résumé ok` → `r-sum-ok`); a `-N` suffix may take a name past 40 characters.
- Sequence diagrams wrap message text at the same 24 columns as flowchart labels; `title`, `box … end` and other unlisted sequence statements are `Unsupported` (the reader keeps the code block).

## Task order and parallelism
| Task | Depends on | Repo/files | May run in parallel with |
|---|---|---|---|
| 1. proto + daemon session context | 8–10 (on the branch) | proto.rs, daemon/{mod,session,journal,server,tests}.rs, claude/hooks.rs, main.rs, client.rs, freshness.rs, tui/sessions.rs, tests/manager.rs (one assertion) | — |
| 2. daemon lens extraction | 1 | claude/{latex,projects}.rs, daemon/session.rs, proto.rs, tui/interaction.rs | — |
| 3. client registry + config | 2 | tui/extensions/{mod,manifest}.rs (new), config.rs, tui/{app,interaction,sessions,mod}.rs | — |
| 4. lens rendering | 3 | tui/{latex,mod}.rs, tui/reader/{doc,layout}.rs, tui/ui/mod.rs | — |
| 5. extension host | 4 | tui/extensions/{host,protocol}.rs (new), extensions/mod.rs, tui/{app,interaction,mod}.rs, reader/mod.rs | — |
| 6. leader and action UI | 5 | tui/leader.rs, tui/ui/leader.rs (new), tui/{app,interaction,keymap,mod,plain}.rs, reader/mod.rs, ui/mod.rs, tests/manager.rs (plain key set) | — |
| 7. framework manager tests | 6 | tests/manager.rs | 12 (docs only touch other files; run 12 after 11 if one worker) |
| 8. diagram engine (part B) | — | done: 1ddd880 | — |
| 9. search op (part B) | — | done: f51437b | — |
| 10. last prompt + snippets store (part B) | — | done: d323c1c | — |
| 11. wiring of 8–10 | 7, 8, 9, 10 | tui/builtins.rs (new), tui/{app,interaction,leader,mod,sessions}.rs, extensions/mod.rs, tests/manager.rs | — |
| 12. docs | 11 | README.md, CLAUDE.md, docs/manager-design.md, docs/extensions.md (new), examples/extensions/keys/ (new), extensions/manifest.rs (one test) | — |

Tasks 1–7 are a chain (each edits files the next one edits), then 11, then 12. Part B's Tasks 8–10
are already on the branch (f51437b), so nothing runs in parallel with this part: each diff below
applies to the tree the previous task leaves.

## Tasks

### Task 1: proto and daemon session context
Model: opus   Repo: <repo>   Files: src/proto.rs, src/daemon/mod.rs, src/daemon/session.rs, src/daemon/journal.rs, src/daemon/server.rs, src/daemon/tests.rs, src/claude/hooks.rs, src/main.rs, src/client.rs, src/freshness.rs, src/tui/sessions.rs, tests/manager.rs
Goal: the client's lenses reach claude through a private context file, with the spec's effective-value rules, caps and journal field (AC2, AC3; AC11's "no context" on the daemon side).

What this task does:
- `proto.rs`: `SessionExtensions { lenses: Vec<LensSpec> }`, `LensSpec { name, kind, instruction }`, `LensKind { Tag, Fence, Unknown (#[serde(other)]) }`, caps `MAX_LENSES = 16`, `MAX_INSTRUCTION = 2048`, `MAX_INSTRUCTIONS = 8192`, `SessionExtensions::validate/context/names`, `lens_name_ok`; `#[serde(default)] extensions: Option<SessionExtensions>` on `SpawnSpec` and `RespawnSpec` (their manual `Debug` prints lens names only); `Welcome.features` (`#[serde(default)]`) and `DAEMON_FEATURES = ["lens", "search", "last_prompt"]` (ruling R9: Tasks 9–10, already on the branch, answer `Search` and `LastPrompt`).
- Daemon: `Spawn`/`Respawn` with extensions over the caps → `Error "bad extensions: …"`. The effective value (`effective_extensions`): the request's, else the journal entry's, else the legacy latex lens when `[claude] latex` is on; lenses of an unknown kind and, with `[claude] latex = false`, the `latex` lens are dropped. Applied in `spawn_reserved`, so it covers a new spawn, a journaled id, `Respawn` (`req.extensions.or(entry.extensions)`), `retry_spawn_fresh` (the entry's) and the client's recovery (a `Spawn`). The journal `Entry` keeps it (`#[serde(default, skip_serializing_if = "Option::is_none")]`).
- Context file: `<socket dir>/context/<session-uuid>.txt` (the socket's dir is the daemon's runtime dir; tests get their own), dir 0700 (`ensure_private_dir`), file 0600 (`write_atomic`); no instructions → no file and no hook. Removed by `kill` and `close_exited`; the dir is emptied in `Daemon::new`, before any launch.
- `hooks::settings(bin, socket, token, context: Option<&Path>)` adds `'<bin>' __context file '<path>'` to SessionStart (the path, never the text). `claudio __context file <path>` (`main.rs` passes `argv[3]`): drains stdin, opens with `O_NOFOLLOW | O_NONBLOCK` (a FIFO must not block the hook), checks the open descriptor is a regular file of the uid, reads ≤ 16 KiB, sanitizes, prints the one JSON line or nothing; exit 0. `__context latex` is unchanged.
- Existing tests that build these structs get `extensions: None` / `features: vec![]`; the tests of the old `latex: bool` parameter now check the context-file hook (`hooks::tests::a_context_file_adds_one_command_to_session_start_only` replaces `latex_adds_one_context_command_to_session_start_only`; `session::tests::command_line_shape`'s last block; the manager test `test_latex_formula_is_rendered_in_the_pane_and_the_reader_list` now finds `__context file` in the hooks). See the deviations noted for the controller at the end of this plan.

Step 1: write the failing tests
`````diff
diff --git a/src/claude/hooks.rs b/src/claude/hooks.rs
index dee835c..4aff4b9 100644
--- a/src/claude/hooks.rs
+++ b/src/claude/hooks.rs
@@ -201,7 +254,7 @@ mod tests {
             Path::new("/usr/bin/claudio"),
             Path::new("/run/claudio/d.sock"),
             "tok",
-            false,
+            None,
         );
         let hooks = v["hooks"].as_object().unwrap();
         for event in EVENTS {
@@ -215,7 +268,7 @@ mod tests {
             Path::new("/bin/cl"),
             Path::new("/run/s.sock"),
             "mytoken",
-            false,
+            None,
         );
         let cmd = v["hooks"]["Stop"][0]["hooks"][0]["command"]
             .as_str()
@@ -231,7 +284,7 @@ mod tests {
             Path::new("/home/user name/bin/claudio"),
             Path::new("/run/user data/s.sock"),
             "tok",
-            false,
+            None,
         );
         let cmd = v["hooks"]["Stop"][0]["hooks"][0]["command"]
             .as_str()
@@ -242,10 +295,10 @@ mod tests {
     }
 
     #[test]
-    fn latex_adds_one_context_command_to_session_start_only() {
+    fn a_context_file_adds_one_command_to_session_start_only() {
         let (bin, sock) = (Path::new("/usr/bin/claudio"), Path::new("/run/d.sock"));
-        let off = settings(bin, sock, "tok", false);
-        let on = settings(bin, sock, "tok", true);
+        let off = settings(bin, sock, "tok", None);
+        let on = settings(bin, sock, "tok", Some(Path::new("/run/context/s 1.txt")));
         for event in EVENTS.iter().filter(|e| **e != "SessionStart") {
             assert_eq!(on["hooks"][event], off["hooks"][event], "{event}");
         }
@@ -256,7 +309,10 @@ mod tests {
         assert_eq!(hooks.len(), 2);
         assert_eq!(hooks[0], off_start["hooks"][0], "the relay stays first");
         assert_eq!(hooks[1]["type"], "command");
-        assert_eq!(hooks[1]["command"], "'/usr/bin/claudio' __context latex");
+        assert_eq!(
+            hooks[1]["command"],
+            "'/usr/bin/claudio' __context file '/run/context/s 1.txt'"
+        );
         assert_eq!(on_start["matcher"], off_start["matcher"]);
         assert_eq!(on_start["timeout"], off_start["timeout"]);
     }
@@ -285,6 +341,75 @@ mod tests {
         }
     }
 
+    /// A fresh private scratch dir for context files.
+    fn context_dir() -> std::path::PathBuf {
+        let dir = crate::paths::socket_test_dir("cl-ctx");
+        std::fs::create_dir_all(&dir).unwrap();
+        dir
+    }
+
+    fn context_of(path: &Path) -> String {
+        let mut out = Vec::new();
+        context_file_to(path, &b"{}"[..], &mut out);
+        String::from_utf8(out).unwrap()
+    }
+
+    #[test]
+    fn context_file_prints_its_sanitized_text() {
+        let dir = context_dir();
+        let path = dir.join("s.txt");
+        std::fs::write(&path, "Use <keys>.\u{1b}[31m\n\nDraw mermaid.\n").unwrap();
+        let text = context_of(&path);
+        assert_eq!(text.matches('\n').count(), 1, "one line: {text}");
+        let v: Value = serde_json::from_str(text.trim_end()).unwrap();
+        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "SessionStart");
+        let context = v["hookSpecificOutput"]["additionalContext"]
+            .as_str()
+            .unwrap();
+        assert!(context.starts_with("Use <keys>."), "{context:?}");
+        assert!(context.ends_with("Draw mermaid."), "{context:?}");
+        assert!(!context.contains('\u{1b}'), "{context:?}");
+        std::fs::remove_dir_all(&dir).ok();
+    }
+
+    #[test]
+    fn context_file_reads_at_most_16_kib() {
+        let dir = context_dir();
+        let path = dir.join("big.txt");
+        std::fs::write(&path, "a".repeat(20_000)).unwrap();
+        let v: Value = serde_json::from_str(context_of(&path).trim_end()).unwrap();
+        let context = v["hookSpecificOutput"]["additionalContext"]
+            .as_str()
+            .unwrap();
+        assert_eq!(context.len(), 16 * 1024);
+        std::fs::remove_dir_all(&dir).ok();
+    }
+
+    #[test]
+    fn context_file_prints_nothing_for_what_it_must_not_read() {
+        let dir = context_dir();
+        let good = dir.join("good.txt");
+        std::fs::write(&good, "secret instructions").unwrap();
+        // Missing, empty, a symlink to a good file, a directory, a FIFO.
+        let empty = dir.join("empty.txt");
+        std::fs::write(&empty, " \n").unwrap();
+        let link = dir.join("link.txt");
+        std::os::unix::fs::symlink(&good, &link).unwrap();
+        let fifo = dir.join("fifo");
+        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
+        // SAFETY: a valid NUL-terminated path.
+        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
+        for path in [dir.join("missing.txt"), empty, link, dir.clone(), fifo] {
+            assert_eq!(context_of(&path), "", "{}", path.display());
+        }
+        // Owned by another user (root's files, when we are not root).
+        if crate::paths::uid() != 0 {
+            assert_eq!(context_of(Path::new("/etc/passwd")), "");
+        }
+        assert!(!context_of(&good).is_empty());
+        std::fs::remove_dir_all(&dir).ok();
+    }
+
     #[test]
     fn new_token_is_32_hex_chars() {
         let tok = new_token();
diff --git a/src/client.rs b/src/client.rs
index 444c486..a26db7e 100644
--- a/src/client.rs
+++ b/src/client.rs
@@ -797,6 +797,7 @@ mod tests {
             },
             build: None,
             build_time: None,
+            features: vec![],
         })
     }
 
diff --git a/src/daemon/journal.rs b/src/daemon/journal.rs
index 1ff5370..784821a 100644
--- a/src/daemon/journal.rs
+++ b/src/daemon/journal.rs
@@ -284,6 +292,7 @@ mod tests {
             rows: 24,
             cols: 80,
             ephemeral: false,
+            extensions: None,
         }
     }
 
@@ -455,4 +464,52 @@ mod tests {
         std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
         std::fs::remove_dir_all(&dir).unwrap();
     }
+
+    #[test]
+    fn extensions_round_trip_and_old_journals_load_without_them() {
+        use crate::proto::{LensKind, LensSpec, SessionExtensions};
+        let dir = std::env::temp_dir().join(format!("claudio-journal-{}", Uuid::new_v4()));
+        let path = dir.join("j.json");
+        // A v0.5.0 journal: no `extensions`.
+        let id = Uuid::new_v4();
+        let old = format!(
+            r#"{{"sessions":[{{"id":"{id}","cwd":"/w","args":[],"created_at":1}}]}}"#
+        );
+        std::fs::create_dir_all(&dir).unwrap();
+        std::fs::write(&path, old).unwrap();
+        let mut j = Journal::load(&path);
+        assert_eq!(j.entries()[0].extensions, None);
+
+        // A spawn with extensions records them, and they come back.
+        let ext = SessionExtensions {
+            lenses: vec![LensSpec {
+                name: "keys".into(),
+                kind: LensKind::Tag,
+                instruction: "Write <keys>.".into(),
+            }],
+        };
+        let mut s = spec(id);
+        s.extensions = Some(ext.clone());
+        j.record_spawn(&s, SessionKind::Claude).1.unwrap();
+        let back = Journal::load(&path);
+        assert_eq!(back.entries()[0].extensions, Some(ext));
+
+        // v0.5.0's `Entry` (no such field, no deny_unknown_fields) reads it.
+        #[derive(serde::Deserialize)]
+        #[allow(dead_code)]
+        struct V050Entry {
+            id: SessionId,
+            cwd: String,
+            #[serde(default)]
+            args: Vec<String>,
+            created_at: u64,
+        }
+        #[derive(serde::Deserialize)]
+        struct V050File {
+            sessions: Vec<V050Entry>,
+        }
+        let file: V050File = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
+        assert_eq!(file.sessions[0].id, id);
+        std::fs::remove_dir_all(&dir).unwrap();
+    }
 }
diff --git a/src/daemon/session.rs b/src/daemon/session.rs
index 6d2d9e3..3edb4b6 100644
--- a/src/daemon/session.rs
+++ b/src/daemon/session.rs
@@ -1162,8 +1216,9 @@ mod tests {
             rows: 24,
             cols: 80,
             ephemeral: false,
+            extensions: None,
         };
-        let cmd = command(&cfg, &spec, Path::new("/"), "tok123", false);
+        let cmd = command(&cfg, &spec, Path::new("/"), "tok123", false, None);
         let argv = argv_of(&cmd);
         assert_eq!(argv[0], "/bin/claude");
         assert_eq!(argv[1], "--settings");
@@ -1176,22 +1231,25 @@ mod tests {
 
         // With skip-permissions on, the flag follows --settings, ahead of
         // the spec args (which are what the journal stores).
-        let cmd = command(&cfg, &spec, Path::new("/"), "tok123", true);
+        let cmd = command(&cfg, &spec, Path::new("/"), "tok123", true, None);
         assert_eq!(
             &argv_of(&cmd)[3..],
             [ALLOW, "--resume", "abc", "-n", "work"]
         );
         assert_eq!(spec.args, ["--resume", "abc"]);
 
-        // `[claude] latex` adds the context hook to SessionStart.
-        let start = |cfg: &super::super::Config| {
-            let argv = argv_of(&command(cfg, &spec, Path::new("/"), "t", false));
+        // A context file adds its hook to SessionStart: the path, not the text.
+        let start = |context: Option<&Path>| {
+            let argv = argv_of(&command(&cfg, &spec, Path::new("/"), "t", false, context));
             let settings: Value = serde_json::from_str(&argv[2]).unwrap();
             settings["hooks"]["SessionStart"][0]["hooks"].clone()
         };
-        assert_eq!(start(&cfg).as_array().unwrap().len(), 1);
-        let latex = super::super::Config { latex: true, ..cfg.clone() };
-        assert_eq!(start(&latex)[1]["command"], "'/bin/claudio' __context latex");
+        assert_eq!(start(None).as_array().unwrap().len(), 1);
+        let ctx = Path::new("/run/context/x.txt");
+        assert_eq!(
+            start(Some(ctx))[1]["command"],
+            "'/bin/claudio' __context file '/run/context/x.txt'"
+        );
     }
 
     fn formulas(list: &[&str]) -> VecDeque<String> {
@@ -1288,8 +1346,9 @@ mod tests {
                 rows: 24,
                 cols: 80,
                 ephemeral: false,
+                extensions: None,
             };
-            let argv = argv_of(&command(&cfg, &spec, Path::new("/"), "t", true));
+            let argv = argv_of(&command(&cfg, &spec, Path::new("/"), "t", true, None));
             assert_eq!(&argv[3..], [given]);
         }
     }
diff --git a/src/daemon/tests.rs b/src/daemon/tests.rs
index 8163d95..333bfb4 100644
--- a/src/daemon/tests.rs
+++ b/src/daemon/tests.rs
@@ -168,6 +168,7 @@ impl TestDaemon {
             rows: 24,
             cols: 80,
             ephemeral: false,
+            extensions: None,
         }
     }
 
@@ -934,6 +935,7 @@ async fn resume_retry_spawns_fresh_on_quick_exit() {
         rows: 24,
         cols: 80,
         ephemeral: false,
+        extensions: None,
     };
 
     // spawner handles the Spawn/Spawned handshake; observer watches events.
@@ -1025,6 +1027,7 @@ async fn immediate_exit_leaves_session_dormant() {
         rows: 24,
         cols: 80,
         ephemeral: false,
+        extensions: None,
     };
     c.spawn(spec).await;
 
@@ -1409,6 +1412,7 @@ impl Client {
             env: vec![("SECRET_ENV".into(), "hunter2".into())],
             rows: 24,
             cols: 80,
+            extensions: None,
         };
         match self.call_seen(Msg::Respawn(spec)).await {
             (Msg::Spawned { id: spawned, pid }, seen) => {
@@ -1533,6 +1537,7 @@ async fn respawn_revives_a_dormant_session_and_rejects_unknown_ones() {
             env: vec![],
             rows: 24,
             cols: 80,
+            extensions: None,
         }))
         .await;
     assert!(matches!(unknown, Msg::Error { message } if message.contains("no such session")));
@@ -1612,6 +1617,7 @@ fn respawn_spec(id: SessionId, fresh: bool) -> proto::RespawnSpec {
         env: vec![],
         rows: 24,
         cols: 80,
+        extensions: None,
     }
 }
 
@@ -1870,3 +1876,155 @@ async fn meta_carries_the_git_status_of_the_session_directory() {
     assert!(git.is_dirty());
     let _ = std::fs::remove_dir_all(&repo);
 }
+
+// ── Extensions: the context file and the effective value ─────────────────────
+
+fn lens(name: &str, instruction: &str) -> proto::LensSpec {
+    proto::LensSpec {
+        name: name.into(),
+        kind: proto::LensKind::Tag,
+        instruction: instruction.into(),
+    }
+}
+
+fn extensions(lenses: &[(&str, &str)]) -> proto::SessionExtensions {
+    proto::SessionExtensions {
+        lenses: lenses.iter().map(|(n, i)| lens(n, i)).collect(),
+    }
+}
+
+/// The lens names a journal entry holds (`None`: no `extensions` field).
+fn journaled_lenses(entry: &serde_json::Value) -> Option<Vec<String>> {
+    let lenses = entry.get("extensions")?["lenses"].as_array()?.clone();
+    Some(lenses.iter().map(|l| l["name"].as_str().unwrap().to_owned()).collect())
+}
+
+#[test]
+fn effective_extensions_follow_request_then_journal_then_legacy() {
+    use super::effective_extensions;
+    let names = |e: proto::SessionExtensions| -> Vec<String> {
+        e.lenses.into_iter().map(|l| l.name).collect()
+    };
+    let asked = extensions(&[("keys", "k")]);
+    let journaled = extensions(&[("abc", "a")]);
+    // The request wins; without one the journal's; without both the legacy.
+    let e = effective_extensions(Some(asked.clone()), Some(journaled.clone()), true);
+    assert_eq!(names(e), ["keys"]);
+    let e = effective_extensions(None, Some(journaled), true);
+    assert_eq!(names(e), ["abc"]);
+    let legacy = effective_extensions(None, None, true);
+    assert_eq!(legacy.lenses.len(), 1);
+    assert_eq!(legacy.lenses[0].name, "latex");
+    assert_eq!(legacy.lenses[0].kind, proto::LensKind::Tag);
+    assert_eq!(legacy.lenses[0].instruction, crate::claude::hooks::LATEX_CONTEXT);
+    // A host with `[claude] latex = false` drops the latex lens, asked or not.
+    assert!(effective_extensions(None, None, false).lenses.is_empty());
+    let both = extensions(&[("latex", "math"), ("keys", "k")]);
+    assert_eq!(names(effective_extensions(Some(both), None, false)), ["keys"]);
+    // A lens of a kind this daemon does not know is dropped.
+    let mut odd = extensions(&[("keys", "k"), ("new", "n")]);
+    odd.lenses[1].kind = proto::LensKind::Unknown;
+    assert_eq!(names(effective_extensions(Some(odd), None, true)), ["keys"]);
+}
+
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn a_spawn_writes_a_private_context_file_and_a_kill_removes_it() {
+    use std::os::unix::fs::PermissionsExt;
+    let d = TestDaemon::start().await;
+    let mut c = d.client().await;
+    let id = Uuid::new_v4();
+    let mut spec = d.spec(id);
+    // This host has `[claude] latex = false`: the latex lens goes.
+    spec.extensions = Some(extensions(&[
+        ("latex", "MATH-INSTRUCTION"),
+        ("keys", "KEYS-INSTRUCTION"),
+        ("abc", ""),
+    ]));
+    c.spawn(spec).await;
+    d.token().await;
+
+    let path = d.dir.join("rt/context").join(format!("{id}.txt"));
+    assert_eq!(std::fs::read_to_string(&path).unwrap(), "KEYS-INSTRUCTION");
+    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
+    assert_eq!(mode(&path), 0o600);
+    assert_eq!(mode(path.parent().unwrap()), 0o700);
+
+    // The hook has the path; the settings never the text.
+    let settings = std::fs::read_to_string(d.work().join("settings.json")).unwrap();
+    assert!(!settings.contains("INSTRUCTION"), "{settings}");
+    let settings: serde_json::Value = serde_json::from_str(&settings).unwrap();
+    let hooks = settings["hooks"]["SessionStart"][0]["hooks"].as_array().unwrap();
+    let expected = format!("'/bin/true' __context file '{}'", path.display());
+    assert_eq!(hooks[1]["command"], expected.as_str());
+
+    // The journal keeps the effective value.
+    let journal = d.journal();
+    assert_eq!(journaled_lenses(&journal["sessions"][0]).unwrap(), ["keys", "abc"]);
+
+    assert_eq!(c.call(Msg::Kill { id }).await, Msg::Ok);
+    assert!(!path.exists());
+}
+
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn no_instructions_means_no_context_file_and_no_hook() {
+    let d = TestDaemon::start().await;
+    let mut c = d.client().await;
+    let id = Uuid::new_v4();
+    // An old client: no extensions, and this host has latex off.
+    c.spawn(d.spec(id)).await;
+    d.token().await;
+    let settings = std::fs::read_to_string(d.work().join("settings.json")).unwrap();
+    let settings: serde_json::Value = serde_json::from_str(&settings).unwrap();
+    assert_eq!(settings["hooks"]["SessionStart"][0]["hooks"].as_array().unwrap().len(), 1);
+    assert!(!d.dir.join("rt/context").join(format!("{id}.txt")).exists());
+    assert_eq!(journaled_lenses(&d.journal()["sessions"][0]).unwrap(), Vec::<String>::new());
+}
+
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn a_respawn_with_extensions_replaces_the_journaled_ones() {
+    let d = TestDaemon::start_logging_argv().await;
+    let mut c = d.client().await;
+    let id = Uuid::new_v4();
+    let mut spec = d.spec(id);
+    spec.extensions = Some(extensions(&[("keys", "OLD")]));
+    c.spawn(spec).await;
+    d.runs_reach(1).await;
+    let path = d.dir.join("rt/context").join(format!("{id}.txt"));
+
+    // No extensions in the respawn (an old client): the journal's stay.
+    c.respawn(id, false).await;
+    d.runs_reach(2).await;
+    assert_eq!(std::fs::read_to_string(&path).unwrap(), "OLD");
+
+    let mut respawn = respawn_spec(id, false);
+    respawn.extensions = Some(extensions(&[("abc", "NEW")]));
+    assert!(matches!(c.call(Msg::Respawn(respawn)).await, Msg::Spawned { .. }));
+    d.runs_reach(3).await;
+    assert_eq!(std::fs::read_to_string(&path).unwrap(), "NEW");
+    assert_eq!(journaled_lenses(&d.journal()["sessions"][0]).unwrap(), ["abc"]);
+}
+
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn extensions_over_the_caps_are_refused() {
+    let d = TestDaemon::start().await;
+    let mut c = d.client().await;
+    let id = Uuid::new_v4();
+    let mut spec = d.spec(id);
+    spec.extensions = Some(extensions(&[("keys", "a"), ("keys", "b")]));
+    let reply = c.call(Msg::Spawn(spec)).await;
+    assert!(
+        matches!(&reply, Msg::Error { message } if message.contains("bad extensions")),
+        "{reply:?}"
+    );
+    assert!(c.sessions().await.is_empty());
+}
+
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn a_daemon_start_clears_old_context_files() {
+    let dir = TestDaemon::new_dir();
+    let context = dir.join("rt/context");
+    std::fs::create_dir_all(&context).unwrap();
+    std::fs::write(context.join("old.txt"), "stale").unwrap();
+    let _d = TestDaemon::start_in(dir).await;
+    assert!(!context.join("old.txt").exists());
+}
diff --git a/src/freshness.rs b/src/freshness.rs
index d8a918a..5dda39a 100644
--- a/src/freshness.rs
+++ b/src/freshness.rs
@@ -231,6 +231,7 @@ mod tests {
             },
             build: build.map(Into::into),
             build_time,
+            features: vec![],
         }
     }
 
diff --git a/src/proto.rs b/src/proto.rs
index ad2c5cb..f898845 100644
--- a/src/proto.rs
+++ b/src/proto.rs
@@ -1019,6 +1134,7 @@ mod tests {
             env: vec![("ANTHROPIC_AUTH_TOKEN".into(), "sekrit".into())],
             rows: 24,
             cols: 80,
+            extensions: None,
         });
         let json = serde_json::to_string(&Envelope::request(4, respawn.clone())).unwrap();
         assert!(json.contains(r#""op":"respawn""#) && json.contains(r#""fresh":true"#));
@@ -1034,6 +1150,7 @@ mod tests {
                 env: vec![],
                 rows: 24,
                 cols: 80,
+                extensions: None,
             })
         );
     }
@@ -1117,6 +1234,7 @@ mod tests {
             rows: 24,
             cols: 80,
             ephemeral: false,
+            extensions: None,
         };
         let dbg = format!("{spec:?}");
         assert!(dbg.contains("ANTHROPIC_AUTH_TOKEN"));
@@ -1485,4 +1603,87 @@ mod tests {
         let back: Msg = serde_json::from_str(&serde_json::to_string(&reply).unwrap()).unwrap();
         assert_eq!(back, reply);
     }
+
+    fn lens(name: &str, instruction: &str) -> LensSpec {
+        LensSpec {
+            name: name.into(),
+            kind: LensKind::Tag,
+            instruction: instruction.into(),
+        }
+    }
+
+    #[test]
+    fn extensions_are_optional_on_the_wire_and_unknown_kinds_survive() {
+        // An old client's Spawn and Respawn: no extensions.
+        let spawn = r#"{"id":"00000000-0000-0000-0000-000000000000","cwd":"/","rows":1,"cols":1}"#;
+        assert_eq!(serde_json::from_str::<SpawnSpec>(spawn).unwrap().extensions, None);
+        let respawn = r#"{"id":"00000000-0000-0000-0000-000000000000","rows":1,"cols":1}"#;
+        assert_eq!(serde_json::from_str::<RespawnSpec>(respawn).unwrap().extensions, None);
+        // A newer client's lens kind still parses (the daemon drops it).
+        let ext = r#"{"lenses":[{"name":"x","kind":"widget","instruction":"i"},{"name":"keys","kind":"tag"}]}"#;
+        let ext: SessionExtensions = serde_json::from_str(ext).unwrap();
+        assert_eq!(ext.lenses[0].kind, LensKind::Unknown);
+        assert_eq!(ext.lenses[1].kind, LensKind::Tag);
+        assert_eq!(ext.lenses[1].instruction, "");
+        // An old daemon's Welcome: no features.
+        let welcome = serde_json::json!({
+            "claudio_version": "0.5.0", "proto": 1,
+            "host": {"hostname": "h", "os": "linux", "arch": "x86_64", "home": "/h"}
+        });
+        let welcome: Welcome = serde_json::from_value(welcome).unwrap();
+        assert!(welcome.features.is_empty());
+        assert_eq!(DAEMON_FEATURES, ["lens", "search", "last_prompt"]);
+    }
+
+    #[test]
+    fn extension_caps() {
+        let ok = |lenses: Vec<LensSpec>| SessionExtensions { lenses }.validate();
+        assert_eq!(ok(vec![lens("latex", "x"), lens("a-1", "")]), Ok(()));
+        assert!(ok((0..17).map(|i| lens(&format!("l{i}"), "")).collect()).is_err());
+        assert!(ok(vec![lens("keys", ""), lens("keys", "")]).is_err());
+        for bad in ["", "Keys", "1x", "a_b", "a b", &"a".repeat(25)] {
+            assert!(ok(vec![lens(bad, "")]).is_err(), "{bad:?}");
+        }
+        assert_eq!(ok(vec![lens(&"a".repeat(24), "")]), Ok(()));
+        // 2048 bytes after sanitize: controls do not count.
+        let at_cap = format!("{}\u{7}", "i".repeat(2048));
+        assert_eq!(ok(vec![lens("a", &at_cap)]), Ok(()));
+        assert!(ok(vec![lens("a", &"i".repeat(2049))]).is_err());
+        // 8192 in total.
+        let full: Vec<LensSpec> = (0..4).map(|i| lens(&format!("l{i}"), &"i".repeat(2048))).collect();
+        assert_eq!(ok(full.clone()), Ok(()));
+        let mut over = full;
+        over.push(lens("one", "i"));
+        assert!(ok(over).is_err());
+    }
+
+    #[test]
+    fn extension_context_and_debug_never_show_instructions() {
+        let ext = SessionExtensions {
+            lenses: vec![lens("latex", " Math.\u{1b} "), lens("abc", ""), lens("keys", "Keys.")],
+        };
+        assert_eq!(ext.context(), "Math.\n\nKeys.");
+        let spec = SpawnSpec {
+            id: Uuid::nil(),
+            cwd: "/".into(),
+            name: None,
+            args: vec![],
+            env: vec![],
+            rows: 1,
+            cols: 1,
+            ephemeral: false,
+            extensions: Some(ext.clone()),
+        };
+        let shown = format!("{spec:?}");
+        assert!(shown.contains("\"latex\"") && !shown.contains("Math"), "{shown}");
+        let respawn = RespawnSpec {
+            id: Uuid::nil(),
+            fresh: false,
+            env: vec![],
+            rows: 1,
+            cols: 1,
+            extensions: Some(ext),
+        };
+        assert!(!format!("{respawn:?}").contains("Keys."));
+    }
 }
diff --git a/tests/manager.rs b/tests/manager.rs
index a32833e..6340333 100644
--- a/tests/manager.rs
+++ b/tests/manager.rs
@@ -2163,7 +2163,7 @@ exec cat
 /// A formula in claude's answer is rendered in place of its `<latex>` tag
 /// and listed by the reader view's formulas list (Alt+u, Tab); a TUI
 /// started later gets it from the daemon's replay on attach. The default
-/// config gives the session the `__context latex` SessionStart hook.
+/// config gives the session the context-file SessionStart hook.
 #[test]
 fn test_latex_formula_is_rendered_in_the_pane_and_the_reader_list() {
     let h = ManagerHarness::new();
@@ -2177,7 +2177,7 @@ fn test_latex_formula_is_rendered_in_the_pane_and_the_reader_list() {
         tui.wait_for("Energy: E = mc² as promised.", Region::Pane, DAEMON_WAIT);
         assert!(!tui.screen_text(Region::Pane).contains("<latex>"));
         let hooks = fs::read_to_string(&settings).unwrap();
-        assert!(hooks.contains("__context latex"), "{hooks}");
+        assert!(hooks.contains("__context file"), "{hooks}");
 
         // The transcript has no prompt, so no answer; Tab lists the formula.
         tui.send_keys(ALT_U);
`````
Run: `umask 022; cargo test --locked --bin claudio 2>&1 | grep -E '^error' | head -3`
Expected: `` error[E0432]: unresolved imports `crate::proto::LensKind`, `crate::proto::LensSpec`, `crate::proto::SessionExtensions` ``

Step 2: implement
`````diff
diff --git a/src/claude/hooks.rs b/src/claude/hooks.rs
index 229c728..4aff4b9 100644
--- a/src/claude/hooks.rs
+++ b/src/claude/hooks.rs
@@ -8,11 +8,14 @@
 //! - [`EVENTS`] — the hook events the daemon registers.
 //! - [`settings`] — build the `--settings` JSON blob for a session.
 //! - [`relay`] — the short-lived relay process that ships the payload.
-//! - [`context`] — `claudio __context latex`, the SessionStart hook that asks
-//!   claude to wrap math in `<latex>` tags (`[claude] latex`).
+//! - [`context`] — `claudio __context file <path>`, the SessionStart hook
+//!   that adds the session's extension instructions (the daemon's context
+//!   file) to claude's context; `__context latex` is kept for sessions an
+//!   older daemon started.
 //! - [`new_token`] — generate a per-spawn secret token.
 
 use std::io::{Read, Write};
+use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
 use std::os::unix::net::UnixStream;
 use std::path::Path;
 use std::time::Duration;
@@ -39,6 +42,9 @@ const WRITE_TIMEOUT: Duration = Duration::from_secs(2);
 /// Maximum hook payload size. Hooks should be tiny; 1 MiB is a hard cap.
 const MAX_PAYLOAD: usize = 1 << 20;
 
+/// The most of a context file `__context file` reads.
+const MAX_CONTEXT: u64 = 16 * 1024;
+
 /// What `claudio __context latex` adds to every conversation's context when
 /// `[claude] latex` is on: the manager renders `<latex>` tags in the pane.
 /// Context, not the system prompt, so the session stays a plain interactive
@@ -56,8 +62,14 @@ numbers or code in them. Write relations with spaces around them (a < b, not a<b
 ///
 /// The hook command is `'<claudio_bin>' __hook <Event> '<socket>' <token>`,
 /// with paths shell-quoted so that spaces and special characters are safe.
-/// With `latex`, `SessionStart` runs `'<claudio_bin>' __context latex` too.
-pub fn settings(claudio_bin: &Path, socket: &Path, token: &str, latex: bool) -> serde_json::Value {
+/// With a `context` file, `SessionStart` runs
+/// `'<claudio_bin>' __context file '<context>'` too: the path, never the text.
+pub fn settings(
+    claudio_bin: &Path,
+    socket: &Path,
+    token: &str,
+    context: Option<&Path>,
+) -> serde_json::Value {
     let bin = shell_quote(&claudio_bin.to_string_lossy());
     let sock = shell_quote(&socket.to_string_lossy());
 
@@ -65,8 +77,9 @@ pub fn settings(claudio_bin: &Path, socket: &Path, token: &str, latex: bool) ->
     for event in EVENTS {
         let command = format!("{bin} __hook {event} {sock} {token}");
         let mut hooks = vec![serde_json::json!({"type": "command", "command": command})];
-        if latex && *event == "SessionStart" {
-            let context = format!("{bin} __context latex");
+        if let Some(path) = context.filter(|_| *event == "SessionStart") {
+            let path = shell_quote(&path.to_string_lossy());
+            let context = format!("{bin} __context file {path}");
             hooks.push(serde_json::json!({"type": "command", "command": context}));
         }
         let entry = serde_json::json!([{
@@ -90,12 +103,18 @@ fn shell_quote(s: &str) -> String {
 
 // ── Context ───────────────────────────────────────────────────────────────────
 
-/// `claudio __context <kind>`: a SessionStart hook that adds to claude's
-/// context. Drains the hook payload (capped like the relay), then prints the
-/// one JSON line that adds [`LATEX_CONTEXT`] for `latex`, and nothing for any
-/// other kind. Always exits 0 and never touches the daemon's socket.
-pub fn context(kind: Option<&str>) -> std::process::ExitCode {
-    context_to(kind, std::io::stdin(), &mut std::io::stdout());
+/// `claudio __context <kind> [<path>]`: a SessionStart hook that adds to
+/// claude's context. Drains the hook payload (capped like the relay), then
+/// prints the one JSON line that adds [`LATEX_CONTEXT`] for `latex`, or the
+/// context file's text for `file <path>` ([`read_context`]), and nothing for
+/// anything else. Always exits 0 and never touches the daemon's socket.
+pub fn context(kind: Option<&str>, path: Option<&str>) -> std::process::ExitCode {
+    match (kind, path) {
+        (Some("file"), Some(path)) => {
+            context_file_to(Path::new(path), std::io::stdin(), &mut std::io::stdout())
+        }
+        _ => context_to(kind, std::io::stdin(), &mut std::io::stdout()),
+    }
     std::process::ExitCode::SUCCESS
 }
 
@@ -104,10 +123,44 @@ fn context_to(kind: Option<&str>, input: impl Read, out: &mut impl Write) {
     if kind != Some("latex") {
         return;
     }
+    print_context(LATEX_CONTEXT, out);
+}
+
+/// `__context file <path>`: the file's text as context, if it is acceptable.
+fn context_file_to(path: &Path, input: impl Read, out: &mut impl Write) {
+    let _ = std::io::copy(&mut input.take(MAX_PAYLOAD as u64), &mut std::io::sink());
+    if let Some(text) = read_context(path) {
+        print_context(&text, out);
+    }
+}
+
+/// A context file's text: opened without following a symlink (and without
+/// blocking on a FIFO), then, by the open descriptor, only a regular file
+/// owned by this uid; at most [`MAX_CONTEXT`] bytes, sanitized and trimmed.
+/// `None` when missing, empty or not acceptable.
+fn read_context(path: &Path) -> Option<String> {
+    let file = std::fs::OpenOptions::new()
+        .read(true)
+        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
+        .open(path)
+        .ok()?;
+    let meta = file.metadata().ok()?;
+    if !meta.file_type().is_file() || meta.uid() != crate::paths::uid() {
+        return None;
+    }
+    let mut bytes = Vec::new();
+    file.take(MAX_CONTEXT).read_to_end(&mut bytes).ok()?;
+    let text = crate::claude::latex::sanitize(&String::from_utf8_lossy(&bytes));
+    let text = text.trim();
+    (!text.is_empty()).then(|| text.to_owned())
+}
+
+/// The one JSON line that adds `text` to claude's context.
+fn print_context(text: &str, out: &mut impl Write) {
     let line = serde_json::json!({
         "hookSpecificOutput": {
             "hookEventName": "SessionStart",
-            "additionalContext": LATEX_CONTEXT,
+            "additionalContext": text,
         }
     });
     let _ = writeln!(out, "{line}");
diff --git a/src/daemon/journal.rs b/src/daemon/journal.rs
index b659f18..784821a 100644
--- a/src/daemon/journal.rs
+++ b/src/daemon/journal.rs
@@ -28,7 +28,7 @@ use std::path::{Path, PathBuf};
 use serde::{Deserialize, Serialize};
 
 use crate::paths;
-use crate::proto::{SessionId, SessionKind, SpawnSpec};
+use crate::proto::{SessionExtensions, SessionId, SessionKind, SpawnSpec};
 
 /// One journaled session.
 #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
@@ -52,6 +52,11 @@ pub struct Entry {
     /// the entry) but never written, so a daemon restart forgets it.
     #[serde(skip)]
     pub ephemeral: bool,
+    /// The extensions the session last launched with (the daemon's
+    /// effective value). Absent in journals from before extensions: such an
+    /// entry gets the legacy value at its next launch.
+    #[serde(default, skip_serializing_if = "Option::is_none")]
+    pub extensions: Option<SessionExtensions>,
 }
 
 #[derive(Default, Serialize, Deserialize)]
@@ -145,7 +150,8 @@ impl Journal {
 
     /// Update in-memory only (no disk write). A re-spawn of a known id keeps
     /// its `created_at`, last claude session id, `kind` and `ephemeral` (those
-    /// only apply to a new entry); cwd, name and args follow the new spec.
+    /// only apply to a new entry); cwd, name, args and extensions follow the
+    /// new spec.
     /// Returns the (possibly updated) entry.
     pub fn upsert_entry(&mut self, spec: &SpawnSpec, kind: SessionKind) -> Entry {
         match self.sessions.iter_mut().find(|e| e.id == spec.id) {
@@ -153,6 +159,7 @@ impl Journal {
                 e.cwd = spec.cwd.clone();
                 e.name = spec.name.clone();
                 e.args = spec.args.clone();
+                e.extensions = spec.extensions.clone();
                 e.clone()
             }
             None => {
@@ -165,6 +172,7 @@ impl Journal {
                     claude_session_id: None,
                     created_at: crate::paths::unix_now(),
                     ephemeral: spec.ephemeral,
+                    extensions: spec.extensions.clone(),
                 };
                 self.sessions.push(e.clone());
                 e
diff --git a/src/daemon/mod.rs b/src/daemon/mod.rs
index a462d04..7a60f3c 100644
--- a/src/daemon/mod.rs
+++ b/src/daemon/mod.rs
@@ -40,8 +40,8 @@ use tracing_subscriber::EnvFilter;
 
 use crate::paths;
 use crate::proto::{
-    Envelope, Frame, HostInfo, Msg, RespawnSpec, SessionEvent, SessionId, SessionInfo,
-    SessionKind, SessionState, SpawnSpec,
+    Envelope, Frame, HostInfo, LensKind, LensSpec, Msg, RespawnSpec, SessionEvent,
+    SessionExtensions, SessionId, SessionInfo, SessionKind, SessionState, SpawnSpec,
 };
 use crate::remote::probe::Probe;
 use journal::{Entry, Journal};
@@ -368,6 +368,8 @@ impl Daemon {
             sessions = journal.entries().len(),
             "journal loaded; all sessions dormant"
         );
+        // Context files of a previous daemon's sessions: none is ours yet.
+        session::clear_contexts(&config.socket);
         let (events, _) = broadcast::channel(EVENTS_CAPACITY);
         let (stats_tx, stats_rx) = tokio::sync::watch::channel(stats::HostSnapshot::default());
         tokio::spawn(stats::sample_loop(stats_tx));
@@ -501,6 +503,9 @@ impl Daemon {
         spec: SpawnSpec,
         requested: SessionKind,
     ) -> io::Result<Option<u32>> {
+        if let Some(ext) = &spec.extensions {
+            ext.validate().map_err(|why| io::Error::other(format!("bad extensions: {why}")))?;
+        }
         // Phase 1: idempotency check and reservation (fast, no I/O).
         let kind = {
             let mut reg = self.registry();
@@ -531,6 +536,15 @@ impl Daemon {
         if kind == SessionKind::Shell {
             spec.args.clear();
             spec.env.clear();
+            spec.extensions = None;
+        } else {
+            let journaled = {
+                let reg = self.registry();
+                let entry = reg.journal.entries().iter().find(|e| e.id == spec.id);
+                entry.and_then(|e| e.extensions.clone())
+            };
+            let requested = spec.extensions.take();
+            spec.extensions = Some(effective_extensions(requested, journaled, self.config.latex));
         }
 
         // Phase 2: PTY open + process spawn + thread creation (blocking,
@@ -693,6 +707,7 @@ impl Daemon {
             let _ = handle.tx.send(Cmd::Kill).await;
         }
 
+        session::remove_context(&self.config.socket, id);
         tracing::info!(%id, "session removed");
         self.broadcast(id, SessionEvent::Removed);
         Ok(())
@@ -812,6 +827,7 @@ impl Daemon {
                     rows: 24,
                     cols: 80,
                     ephemeral: e.ephemeral,
+                    extensions: e.extensions.clone(),
                 };
                 (spec, e.kind)
             })
@@ -858,6 +874,9 @@ impl Daemon {
     /// Respawn (or a Spawn) is refused meanwhile, and a Kill is deferred to
     /// `to_kill` and wins, so a killed session is never brought back.
     async fn respawn(self: &Arc<Self>, req: RespawnSpec) -> io::Result<Option<u32>> {
+        if let Some(ext) = &req.extensions {
+            ext.validate().map_err(|why| io::Error::other(format!("bad extensions: {why}")))?;
+        }
         let id = req.id;
         let (entry, old) = {
             let mut reg = self.registry();
@@ -904,6 +923,7 @@ impl Daemon {
             rows: req.rows,
             cols: req.cols,
             ephemeral: entry.ephemeral,
+            extensions: req.extensions.or(entry.extensions),
         };
         let spawned = self.spawn_reserved(spec, entry.kind).await;
         match (&spawned, self.session(id)) {
@@ -973,6 +993,7 @@ impl Daemon {
             self.registry().journal.reinsert(entry);
             return;
         }
+        session::remove_context(&self.config.socket, id);
         tracing::info!(%id, "session closed after its process exited");
         self.broadcast(id, SessionEvent::Removed);
     }
@@ -997,6 +1018,28 @@ impl Daemon {
     }
 }
 
+/// The extensions a claude launch runs with: the request's when it has
+/// them, else the journaled ones, else the legacy value (the latex lens
+/// when this host's `[claude] latex` is on). Lenses of a kind this daemon
+/// does not know are dropped, and `[claude] latex = false` drops the latex
+/// lens whatever the client asked: a host that turned math off stays off.
+fn effective_extensions(
+    requested: Option<SessionExtensions>,
+    journaled: Option<SessionExtensions>,
+    latex: bool,
+) -> SessionExtensions {
+    let mut ext = requested.or(journaled).unwrap_or_else(|| SessionExtensions {
+        lenses: vec![LensSpec {
+            name: "latex".to_owned(),
+            kind: LensKind::Tag,
+            instruction: crate::claude::hooks::LATEX_CONTEXT.to_owned(),
+        }],
+    });
+    ext.lenses
+        .retain(|l| l.kind != LensKind::Unknown && (latex || l.name != "latex"));
+    ext
+}
+
 /// The refusal for a spawn or respawn of an id that is mid-(re)spawn; the
 /// client hears `Created` when that one completes.
 fn in_progress(id: SessionId) -> io::Error {
diff --git a/src/daemon/server.rs b/src/daemon/server.rs
index e04f9b4..fcd59e5 100644
--- a/src/daemon/server.rs
+++ b/src/daemon/server.rs
@@ -31,8 +31,8 @@ use crate::claude::projects;
 use crate::claude::search::{self, BadQuery, Budget};
 use crate::paths;
 use crate::proto::{
-    self, Envelope, Frame, Msg, ProjectDir, SessionId, SessionKind, SpawnSpec, Welcome, MAX_FRAME,
-    PROTO,
+    self, Envelope, Frame, Msg, ProjectDir, SessionId, SessionKind, SpawnSpec, Welcome,
+    DAEMON_FEATURES, MAX_FRAME, PROTO,
 };
 
 /// Deadline for a connection's first frame.
@@ -280,6 +280,7 @@ async fn client_loop(
         host: client.daemon.host().await,
         build: build.map(|b| b.build.clone()),
         build_time: build.and_then(|b| b.mtime),
+        features: DAEMON_FEATURES.iter().map(|f| f.to_string()).collect(),
     };
     // Queued directly (no event flush): Welcome is always the first frame.
     let welcome = Envelope {
@@ -436,6 +437,7 @@ impl Client {
                     rows: shell.rows,
                     cols: shell.cols,
                     ephemeral: false,
+                    extensions: None,
                 };
                 self.spawn(spec, SessionKind::Shell).await
             }
diff --git a/src/daemon/session.rs b/src/daemon/session.rs
index a50617e..3edb4b6 100644
--- a/src/daemon/session.rs
+++ b/src/daemon/session.rs
@@ -175,12 +175,14 @@ pub fn spawn(
     let path = daemon.config.path.as_deref();
     let (cmd, program) = match kind {
         SessionKind::Claude => {
+            let context = write_context(&daemon.config.socket, spec);
             let cmd = command(
                 &daemon.config,
                 spec,
                 &cwd,
                 &token,
                 daemon.skip_permissions(),
+                context.as_deref(),
             );
             let program = PathBuf::from(cmd.get_argv()[0].clone());
             (cmd, program)
@@ -256,8 +258,59 @@ pub fn spawn(
     Ok((Handle { tx, token, pid }, committed_tx))
 }
 
+/// The daemon's context files: `context/` beside its socket (in its 0700
+/// runtime dir).
+fn context_dir(socket: &Path) -> PathBuf {
+    socket.parent().unwrap_or(Path::new(".")).join("context")
+}
+
+/// Session `id`'s context file. Stable for the session's life, so `/clear`
+/// and compaction re-read the same file.
+pub(super) fn context_path(socket: &Path, id: SessionId) -> PathBuf {
+    context_dir(socket).join(format!("{id}.txt"))
+}
+
+/// Write the instructions of `spec`'s lenses to its context file (dir
+/// 0700, file 0600, atomic) and return its path; with no instructions,
+/// remove the file and return `None`. A failed write is logged and the
+/// session starts without context.
+fn write_context(socket: &Path, spec: &SpawnSpec) -> Option<PathBuf> {
+    let path = context_path(socket, spec.id);
+    let context = spec.extensions.as_ref().map(|e| e.context()).unwrap_or_default();
+    if context.is_empty() {
+        let _ = std::fs::remove_file(&path);
+        return None;
+    }
+    let written = crate::paths::ensure_private_dir(&context_dir(socket))
+        .and_then(|()| crate::paths::write_atomic(&path, context.as_bytes()));
+    match written {
+        Ok(()) => Some(path),
+        Err(e) => {
+            tracing::warn!(id = %spec.id, error = %e, "could not write the context file");
+            None
+        }
+    }
+}
+
+/// Remove session `id`'s context file (it was removed).
+pub(super) fn remove_context(socket: &Path, id: SessionId) {
+    let _ = std::fs::remove_file(context_path(socket, id));
+}
+
+/// Remove every context file: at daemon start, before any launch. A claude
+/// a previous daemon started finds its file gone and gets no context.
+pub(super) fn clear_contexts(socket: &Path) {
+    let Ok(entries) = std::fs::read_dir(context_dir(socket)) else {
+        return;
+    };
+    for entry in entries.flatten() {
+        let _ = std::fs::remove_file(entry.path());
+    }
+}
+
 /// `<claude> --settings <hooks> [--allow-dangerously-skip-permissions]
-/// [args…] [-n <name>]`, in `cwd`, with the session env applied.
+/// [args…] [-n <name>]`, in `cwd`, with the session env applied. With a
+/// `context` file, SessionStart also runs `__context file <context>`.
 ///
 /// The skip-permissions flag is added here, on every launch path (new spawn,
 /// `--resume`, resume-retry), rather than in `spec.args`, so the journal never
@@ -268,8 +321,9 @@ fn command(
     cwd: &Path,
     token: &str,
     skip_permissions: bool,
+    context: Option<&Path>,
 ) -> CommandBuilder {
-    let settings = hooks::settings(&cfg.claudio, &cfg.socket, token, cfg.latex);
+    let settings = hooks::settings(&cfg.claudio, &cfg.socket, token, context);
     let mut cmd = CommandBuilder::new(cfg.claude_bin());
     cmd.arg("--settings");
     cmd.arg(settings.to_string());
diff --git a/src/main.rs b/src/main.rs
index 7c41cf1..c4e3fcb 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -116,10 +116,14 @@ fn main() -> std::process::ExitCode {
         return claude::hooks::relay(&argv[2], std::path::Path::new(&argv[3]), &argv[4]);
     }
 
-    // SessionStart context hook: `claudio __context latex` prints the
-    // instruction to wrap math in `<latex>` tags (`[claude] latex`).
+    // SessionStart context hook: `claudio __context file <path>` prints the
+    // session's extension instructions; `__context latex` (sessions of an
+    // older daemon) the one to wrap math in `<latex>` tags.
     if argv.get(1).map(String::as_str) == Some("__context") {
-        return claude::hooks::context(argv.get(2).map(String::as_str));
+        return claude::hooks::context(
+            argv.get(2).map(String::as_str),
+            argv.get(3).map(String::as_str),
+        );
     }
 
     // Legacy relay mode, invoked as `claudio __hook <Event>`.
diff --git a/src/proto.rs b/src/proto.rs
index a6b26cc..f898845 100644
--- a/src/proto.rs
+++ b/src/proto.rs
@@ -323,8 +323,18 @@ pub struct Welcome {
     /// builds of one version.
     #[serde(default)]
     pub build_time: Option<u64>,
+    /// What this daemon can do beyond the base protocol ([`DAEMON_FEATURES`]).
+    /// Old daemons omit it: they have none of them.
+    #[serde(default)]
+    pub features: Vec<String>,
 }
 
+/// The features a daemon of this build reports in [`Welcome::features`]:
+/// `lens` — it takes [`SpawnSpec::extensions`] and sends
+/// [`SessionEvent::Lens`]; `search` — it answers [`Msg::Search`];
+/// `last_prompt` — it answers [`Msg::LastPrompt`].
+pub const DAEMON_FEATURES: &[&str] = &["lens", "search", "last_prompt"];
+
 #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
 pub struct Colors {
     pub fg: [u8; 3],
@@ -369,6 +379,12 @@ pub struct SpawnSpec {
     /// An old daemon ignores the field and treats it as any other session.
     #[serde(default)]
     pub ephemeral: bool,
+    /// The session's extension lenses (the client's registry): what claude
+    /// is told through the context hook and which tags the daemon extracts.
+    /// `None` (an old client) keeps the journaled value. Ignored by an old
+    /// daemon, which goes by its own `[claude] latex`.
+    #[serde(default)]
+    pub extensions: Option<SessionExtensions>,
 }
 
 /// How to restart a session. Everything else (cwd, name, claude arguments,
@@ -384,6 +400,103 @@ pub struct RespawnSpec {
     pub env: Vec<(String, String)>,
     pub rows: u16,
     pub cols: u16,
+    /// Replaces the journaled extensions when `Some` (see
+    /// [`SpawnSpec::extensions`]).
+    #[serde(default)]
+    pub extensions: Option<SessionExtensions>,
+}
+
+/// The lenses of a claude session, in the client's registry order. The
+/// daemon checks the caps ([`SessionExtensions::validate`]), writes the
+/// instructions to the session's context file and extracts the tag lenses'
+/// tags from the transcript.
+#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
+pub struct SessionExtensions {
+    #[serde(default)]
+    pub lenses: Vec<LensSpec>,
+}
+
+/// One lens: its name (the tag, or the fence's info word), its kind, and
+/// the instruction claude gets for it.
+#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
+pub struct LensSpec {
+    pub name: String,
+    pub kind: LensKind,
+    #[serde(default)]
+    pub instruction: String,
+}
+
+/// A tag lens (`<name>…</name>` in claude's text) or a fence lens (a fenced
+/// code block whose info string starts with the name).
+#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
+#[serde(rename_all = "snake_case")]
+pub enum LensKind {
+    Tag,
+    Fence,
+    /// A kind from a newer client: the daemon drops the lens.
+    #[serde(other)]
+    Unknown,
+}
+
+/// At most this many lenses per session.
+pub const MAX_LENSES: usize = 16;
+/// One instruction's cap, in bytes after `sanitize`.
+pub const MAX_INSTRUCTION: usize = 2048;
+/// All instructions together, in bytes after `sanitize`.
+pub const MAX_INSTRUCTIONS: usize = 8192;
+
+impl SessionExtensions {
+    /// Why this value breaks the caps: more than [`MAX_LENSES`] lenses, a
+    /// name that is not `^[a-z][a-z0-9-]{0,23}$` or is repeated, or
+    /// instructions over [`MAX_INSTRUCTION`] / [`MAX_INSTRUCTIONS`].
+    pub fn validate(&self) -> Result<(), String> {
+        if self.lenses.len() > MAX_LENSES {
+            return Err(format!("more than {MAX_LENSES} lenses"));
+        }
+        let mut total = 0;
+        for (i, lens) in self.lenses.iter().enumerate() {
+            if !lens_name_ok(&lens.name) {
+                return Err(format!("bad lens name {:?}", lens.name));
+            }
+            if self.lenses[..i].iter().any(|l| l.name == lens.name) {
+                return Err(format!("lens {} given twice", lens.name));
+            }
+            let len = crate::claude::latex::sanitize(&lens.instruction).len();
+            if len > MAX_INSTRUCTION {
+                return Err(format!("lens {} instruction over {MAX_INSTRUCTION} bytes", lens.name));
+            }
+            total += len;
+        }
+        if total > MAX_INSTRUCTIONS {
+            return Err(format!("instructions over {MAX_INSTRUCTIONS} bytes"));
+        }
+        Ok(())
+    }
+
+    /// What claude is told: the non-empty instructions, sanitized, joined
+    /// by a blank line.
+    pub fn context(&self) -> String {
+        let parts: Vec<String> = self
+            .lenses
+            .iter()
+            .map(|l| crate::claude::latex::sanitize(&l.instruction).trim().to_owned())
+            .filter(|i| !i.is_empty())
+            .collect();
+        parts.join("\n\n")
+    }
+
+    /// The lens names, for `Debug` and logs (never the instructions).
+    pub fn names(&self) -> Vec<&str> {
+        self.lenses.iter().map(|l| l.name.as_str()).collect()
+    }
+}
+
+/// `^[a-z][a-z0-9-]{0,23}$`.
+pub fn lens_name_ok(name: &str) -> bool {
+    let mut chars = name.chars();
+    name.len() <= 24
+        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
+        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
 }
 
 /// How to start a terminal tab: the login shell, in `cwd`.
@@ -429,6 +542,7 @@ impl std::fmt::Debug for RespawnSpec {
             .field("env", &env_keys)
             .field("rows", &self.rows)
             .field("cols", &self.cols)
+            .field("extensions", &self.extensions.as_ref().map(|e| e.names()))
             .finish()
     }
 }
@@ -445,6 +559,7 @@ impl std::fmt::Debug for SpawnSpec {
             .field("rows", &self.rows)
             .field("cols", &self.cols)
             .field("ephemeral", &self.ephemeral)
+            .field("extensions", &self.extensions.as_ref().map(|e| e.names()))
             .finish()
     }
 }
diff --git a/src/tui/sessions.rs b/src/tui/sessions.rs
index 03d1106..116cf51 100644
--- a/src/tui/sessions.rs
+++ b/src/tui/sessions.rs
@@ -423,6 +423,7 @@ impl App {
             cols,
             // `--plain`'s session is not one for a manager to show.
             ephemeral: self.mode == Mode::Plain,
+            extensions: None,
         };
         self.send_with_proxy(
             host,
@@ -545,6 +546,7 @@ impl App {
             env: vec![], // filled by `send_with_proxy`
             rows,
             cols,
+            extensions: None,
         });
         match self.send_with_proxy(
             &host,
`````
Run: `umask 022; cargo test --locked --bin claudio`
Expected: `test result: ok. 933 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` (base f51437b: 920)
Also: `rustfmt --edition 2021 --check src/claude/hooks.rs src/daemon/server.rs src/client.rs` prints nothing (the `Welcome { …, features: DAEMON_FEATURES.iter().map(|f| f.to_string()).collect() }` line fits on one line, as rustfmt keeps it).

Step 3: commit
`/usr/bin/git commit -m "feat(daemon): session extensions, context file and lens feature" -- src/proto.rs src/daemon/mod.rs src/daemon/session.rs src/daemon/journal.rs src/daemon/server.rs src/daemon/tests.rs src/claude/hooks.rs src/main.rs src/client.rs src/freshness.rs src/tui/sessions.rs tests/manager.rs`

Interfaces this task provides to later tasks: `proto::{SessionExtensions, LensSpec, LensKind, lens_name_ok, MAX_LENSES, MAX_INSTRUCTION, MAX_INSTRUCTIONS, DAEMON_FEATURES}`, `Welcome.features: Vec<String>`, `SpawnSpec.extensions`/`RespawnSpec.extensions: Option<SessionExtensions>`, `journal::Entry.extensions`, `daemon::session::{context_path, remove_context, clear_contexts}`, `hooks::settings(bin, socket, token, Option<&Path>)`, `hooks::context(kind, path)`.

### Task 2: daemon tag-lens extraction and the Lens event
Model: sonnet   Repo: <repo>   Files: src/claude/latex.rs, src/claude/projects.rs, src/daemon/session.rs, src/proto.rs, src/tui/interaction.rs
Goal: the daemon sends every tag lens's items as `SessionEvent::Lens` beside `Latex` (AC4, daemon side).

What this task does:
- `lens_tags(text, tags) -> Vec<(Range, tag, text)>` with latex's rules (outside fences and inline code, case-sensitive, across lines, trimmed, sanitized, empty or > 2048 bytes dropped; the tag that opens first wins; an unclosed tag is dropped from the search so the others after it are still found). `latex_tags`/`extract_latex` stay as wrappers (`latex_tags` = `lens_tags(text, &["latex"])`).
- `TranscriptCursor::with_tags(tags)`; `lens()` keeps the last 64 items (all tags, oldest first; reset with the file like `latex`).
- `SessionEvent::Lens { seq, items: Vec<LensItem> }`, `LensItem { tag, text }`, `LENS_EVENT_MAX = 256 KiB` and `proto::lens_event(seq, items)` (drops the oldest items until it fits). `SessionExtensions::tags()`.
- `MetaRefresher::with_tags` (from the spawn's effective extensions); each refresh sends `Latex` (unchanged) and then `Lens` when the item list changed (`lens_items_event`: its own seq, by `latex_event`'s rules); an attach replays `Latex` then `Lens` (`Subscriber::replay`).
- The client ignores `Lens` for now (one match arm); Task 3 takes it.

Step 1: write the failing tests
`````diff
diff --git a/src/claude/latex.rs b/src/claude/latex.rs
index aad65e9..180f9a8 100644
--- a/src/claude/latex.rs
+++ b/src/claude/latex.rs
@@ -272,4 +302,46 @@ mod tests {
         );
         assert_eq!(sanitize("a\n\u{338}b"), "a\nb");
     }
+
+    #[test]
+    fn lens_tags_find_several_tags_by_the_latex_rules() {
+        let text = "Press <keys> Ctrl+C </keys> then <latex>x^2</latex>.\n\
+                    ```\n<keys>in a fence</keys>\n```\n\
+                    `<keys>inline code</keys>` <KEYS>caps</KEYS> <keys></keys>\n\
+                    <keys>across\nlines</keys> <keys>a\u{202e}b</keys> <keys>open";
+        let found: Vec<(String, String)> = lens_tags(text, &["latex", "keys"])
+            .into_iter()
+            .map(|(range, tag, item)| {
+                assert!(text[range].starts_with(&format!("<{tag}>")));
+                (tag, item)
+            })
+            .collect();
+        let want = [
+            ("keys", "Ctrl+C"),
+            ("latex", "x^2"),
+            ("keys", "across\nlines"),
+            ("keys", "ab"),
+        ];
+        let want: Vec<(String, String)> = want
+            .iter()
+            .map(|(t, i)| (t.to_string(), i.to_string()))
+            .collect();
+        assert_eq!(found, want);
+        // Only the asked tags; too long is dropped.
+        assert_eq!(lens_tags(text, &["latex"]).len(), 1);
+        assert!(lens_tags(text, &[]).is_empty());
+        let long = format!("<keys>{}</keys><keys>k</keys>", "a".repeat(2049));
+        let items: Vec<String> = lens_tags(&long, &["keys"])
+            .into_iter()
+            .map(|t| t.2)
+            .collect();
+        assert_eq!(items, ["k"]);
+        // An unclosed tag does not hide the others after it.
+        for text in ["<latex>open <keys>k</keys>", "<latex>a <keys>k</keys> b"] {
+            let items = lens_tags(text, &["latex", "keys"]);
+            assert_eq!(items.into_iter().map(|t| t.2).collect::<Vec<_>>(), ["k"]);
+        }
+        let items = lens_tags("<keys>open <latex>x</latex>", &["latex", "keys"]);
+        assert_eq!(items.into_iter().map(|t| t.2).collect::<Vec<_>>(), ["x"]);
+    }
 }
diff --git a/src/claude/projects.rs b/src/claude/projects.rs
index 1a27d8c..9ce3648 100644
--- a/src/claude/projects.rs
+++ b/src/claude/projects.rs
@@ -1525,7 +1576,6 @@ mod tests {
         let _ = std::fs::remove_dir_all(&dir);
     }
 
-
     // ── last_prompt ───────────────────────────────────────────────────────
 
     #[test]
@@ -1591,4 +1641,40 @@ mod tests {
         assert_eq!(read_last_prompt(&dir.join("missing.jsonl")), None);
         let _ = std::fs::remove_dir_all(&dir);
     }
+
+    #[test]
+    fn cursor_collects_the_items_of_its_tag_lenses_in_order() {
+        let mut c = cursor().with_tags(vec!["latex".into(), "keys".into()]);
+        let mut jsonl = String::new();
+        jsonl += &said(
+            r#"[{"type":"thinking","thinking":"<keys>t</keys>"},{"type":"text","text":"press <keys>Ctrl+C</keys> for <latex>a^2</latex>, not <kbd>x</kbd>"}]"#,
+        );
+        jsonl += &said(r#"[{"type":"text","text":"<keys>side</keys>"}]"#)
+            .replacen('{', r#"{"isSidechain":true,"#, 1);
+        jsonl += &said(r#""then <keys>Esc</keys>""#);
+        c.feed(jsonl.as_bytes());
+        let items: Vec<(&str, &str)> = c
+            .lens()
+            .iter()
+            .map(|i| (i.tag.as_str(), i.text.as_str()))
+            .collect();
+        assert_eq!(items, [("keys", "Ctrl+C"), ("latex", "a^2"), ("keys", "Esc")]);
+        // The latex list is kept as before.
+        assert_eq!(c.latex().iter().collect::<Vec<_>>(), ["a^2"]);
+        // Without tags, no items.
+        let mut plain = cursor();
+        plain.feed(jsonl.as_bytes());
+        assert!(plain.lens().is_empty());
+    }
+
+    #[test]
+    fn cursor_keeps_the_last_64_lens_items() {
+        let mut c = cursor().with_tags(vec!["keys".into()]);
+        for i in 0..70 {
+            c.feed(said(&format!(r#""<keys>k{i}</keys>""#)).as_bytes());
+        }
+        assert_eq!(c.lens().len(), 64);
+        assert_eq!(c.lens().front().map(|i| i.text.as_str()), Some("k6"));
+        assert_eq!(c.lens().back().map(|i| i.text.as_str()), Some("k69"));
+    }
 }
diff --git a/src/daemon/session.rs b/src/daemon/session.rs
index 3edb4b6..5784782 100644
--- a/src/daemon/session.rs
+++ b/src/daemon/session.rs
@@ -1314,6 +1372,61 @@ mod tests {
         assert!(rx.try_recv().is_err());
     }
 
+    fn items(list: &[(&str, &str)]) -> VecDeque<LensItem> {
+        list.iter()
+            .map(|(tag, text)| LensItem {
+                tag: tag.to_string(),
+                text: text.to_string(),
+            })
+            .collect()
+    }
+
+    fn lens(seq: u64, list: &[(&str, &str)]) -> Option<SessionEvent> {
+        let items = items(list).into_iter().collect();
+        Some(SessionEvent::Lens { seq, items })
+    }
+
+    #[test]
+    fn lens_items_are_sent_when_they_change_with_their_own_growing_seq() {
+        let mut sent = None;
+        assert_eq!(lens_items_event(&mut sent, &items(&[])), None, "nothing yet");
+        let one = items(&[("keys", "a")]);
+        assert_eq!(lens_items_event(&mut sent, &one), lens(1, &[("keys", "a")]));
+        assert_eq!(lens_items_event(&mut sent, &one), None, "unchanged");
+        let two = items(&[("keys", "a"), ("latex", "x")]);
+        let want = lens(2, &[("keys", "a"), ("latex", "x")]);
+        assert_eq!(lens_items_event(&mut sent, &two), want);
+        // A new conversation: the empty list under the next seq.
+        assert_eq!(lens_items_event(&mut sent, &items(&[])), lens(3, &[]));
+        assert_eq!(last_lens(&sent), lens(3, &[]));
+        assert_eq!(last_lens(&None), None);
+    }
+
+    #[test]
+    fn an_attach_gets_the_lens_items_right_after_the_formulas() {
+        let id = uuid::Uuid::new_v4();
+        let mut latex_sent = None;
+        latex_event(&mut latex_sent, &formulas(&["x"]));
+        let mut lens_sent = None;
+        lens_items_event(&mut lens_sent, &items(&[("keys", "k")]));
+        let (queue, mut rx) = mpsc::channel(8);
+        let mut sub = Subscriber {
+            queue,
+            lagging: false,
+            pending_req: None,
+        };
+        assert!(sub.attach(id, (24, 80), b"SNAP", last_latex(&latex_sent)));
+        assert!(sub.replay(id, last_lens(&lens_sent)));
+        assert!(matches!(rx.try_recv(), Ok(Frame::Control(_))), "attached");
+        assert!(matches!(rx.try_recv(), Ok(Frame::Data { .. })), "snapshot");
+        let event = |e: Option<SessionEvent>| {
+            Frame::Control(Envelope::event(Msg::Event { id, event: e.unwrap() }))
+        };
+        assert_eq!(rx.try_recv().unwrap(), event(latex(1, &["x"])));
+        assert_eq!(rx.try_recv().unwrap(), event(lens(1, &[("keys", "k")])));
+        assert!(rx.try_recv().is_err());
+    }
+
     const ALLOW: &str = "--allow-dangerously-skip-permissions";
 
     fn argv_of(cmd: &CommandBuilder) -> Vec<String> {
diff --git a/src/proto.rs b/src/proto.rs
index f898845..701e1f6 100644
--- a/src/proto.rs
+++ b/src/proto.rs
@@ -1686,4 +1731,35 @@ mod tests {
         };
         assert!(!format!("{respawn:?}").contains("Keys."));
     }
+
+    #[test]
+    fn lens_events_stay_under_their_cap_dropping_the_oldest_items() {
+        let item = |i: usize| LensItem {
+            tag: "keys".into(),
+            text: format!("{i}{}", "\\".repeat(2040)),
+        };
+        // 64 items of 2 KiB backslashes encode to about 260 KiB.
+        let items: Vec<LensItem> = (0..64).map(item).collect();
+        let event = lens_event(7, items);
+        let encoded = serde_json::to_vec(&event).unwrap().len();
+        assert!(encoded <= LENS_EVENT_MAX, "{encoded}");
+        let SessionEvent::Lens { seq, items } = event else {
+            panic!("not a lens event")
+        };
+        assert_eq!(seq, 7);
+        assert!(items.len() < 64);
+        assert!(items.last().unwrap().text.starts_with("63"));
+        // A small list is untouched; an old client reads the event as Unknown.
+        let small = lens_event(1, vec![item(0)]);
+        let json = serde_json::to_string(&small).unwrap();
+        assert!(json.contains("\"lens\""), "{json}");
+        #[derive(serde::Deserialize)]
+        #[serde(tag = "kind", rename_all = "snake_case")]
+        enum OldEvent {
+            Latex {},
+            #[serde(other)]
+            Unknown,
+        }
+        assert!(matches!(serde_json::from_str::<OldEvent>(&json), Ok(OldEvent::Unknown)));
+    }
 }
`````
Run: `umask 022; cargo test --locked --bin claudio 2>&1 | grep -E '^error' | head -3`
Expected: `` error[E0425]: cannot find type `LensItem` in this scope ``

Step 2: implement
`````diff
diff --git a/src/claude/latex.rs b/src/claude/latex.rs
index 9695b82..180f9a8 100644
--- a/src/claude/latex.rs
+++ b/src/claude/latex.rs
@@ -11,9 +11,6 @@ pub const MAX_FORMULA_BYTES: usize = 2048;
 /// How many of a conversation's most recent formulas are kept.
 pub const MAX_FORMULAS: usize = 64;
 
-const OPEN: &[u8] = b"<latex>";
-const CLOSE: &[u8] = b"</latex>";
-
 /// `s` without what could reach the terminal as anything but text: controls
 /// (`Cc`, except `\n` and `\t`), format characters (`Cf`: bidi controls,
 /// zero-width characters, BOM, soft hyphen), private-use code points and
@@ -105,8 +102,27 @@ pub fn extract_latex(text: &str) -> Vec<String> {
 /// `</latex>`) and its formula. Tags that are not formulas (in code, empty,
 /// too long) are not listed.
 pub fn latex_tags(text: &str) -> Vec<(std::ops::Range<usize>, String)> {
+    lens_tags(text, &["latex"])
+        .into_iter()
+        .map(|(range, _, formula)| (range, formula))
+        .collect()
+}
+
+/// The tags of any of `tags` in one assistant text, in order, by
+/// [`extract_latex`]'s rules for `<latex>`: what is between `<tag>` and the
+/// next `</tag>` (case-sensitive, across lines), trimmed and [`sanitize`]d,
+/// outside fenced code blocks and inline code spans; empty texts and ones
+/// longer than [`MAX_FORMULA_BYTES`] are dropped. Each tag's byte range in
+/// `text` (from its `<tag>` to the end of its `</tag>`), its tag and its
+/// text. Where tags start inside one another, the one that opens first wins.
+pub fn lens_tags(text: &str, tags: &[&str]) -> Vec<(std::ops::Range<usize>, String, String)> {
     let mut out = Vec::new();
-    if !text.contains("<latex>") {
+    let pairs: Vec<(&str, String, String)> = tags
+        .iter()
+        .map(|t| (*t, format!("<{t}>"), format!("</{t}>")))
+        .filter(|(_, open, _)| text.contains(open.as_str()))
+        .collect();
+    if pairs.is_empty() {
         return out;
     }
     // The text with every byte of code blanked out: tags are searched for
@@ -129,17 +145,31 @@ pub fn latex_tags(text: &str) -> Vec<(std::ops::Range<usize>, String)> {
         }
         start = end;
     }
+    let mut pairs = pairs;
     let mut at = 0;
-    while let Some(open) = find(&visible, OPEN, at) {
-        let inner = open + OPEN.len();
-        let Some(close) = find(&visible, CLOSE, inner) else {
+    loop {
+        let next = pairs
+            .iter()
+            .enumerate()
+            .filter_map(|(i, p)| Some((find(&visible, p.1.as_bytes(), at)?, i)))
+            .min_by_key(|(open, _)| *open);
+        let Some((open, i)) = next else {
             break;
         };
-        let formula = sanitize(&text[inner..close]);
-        let formula = formula.trim();
-        at = close + CLOSE.len();
-        if !formula.is_empty() && formula.len() <= MAX_FORMULA_BYTES {
-            out.push((open..at, formula.to_owned()));
+        let (tag, open_tag, close_tag) = &pairs[i];
+        let inner = open + open_tag.len();
+        let Some(close) = find(&visible, close_tag.as_bytes(), inner) else {
+            // Unclosed: no later tag of this name closes either.
+            pairs.remove(i);
+            at = inner;
+            continue;
+        };
+        let item = sanitize(&text[inner..close]);
+        let item = item.trim();
+        let tag = tag.to_string();
+        at = close + close_tag.len();
+        if !item.is_empty() && item.len() <= MAX_FORMULA_BYTES {
+            out.push((open..at, tag, item.to_owned()));
         }
     }
     out
diff --git a/src/claude/projects.rs b/src/claude/projects.rs
index 63d21a2..9ce3648 100644
--- a/src/claude/projects.rs
+++ b/src/claude/projects.rs
@@ -7,8 +7,8 @@ use std::collections::VecDeque;
 use std::path::{Path, PathBuf};
 use std::time::UNIX_EPOCH;
 
-use crate::claude::latex::{extract_latex, sanitize, MAX_FORMULAS};
-use crate::proto::{Answer, ClaudeSession};
+use crate::claude::latex::{extract_latex, lens_tags, sanitize, MAX_FORMULAS};
+use crate::proto::{Answer, ClaudeSession, LensItem};
 
 // ── Constants ─────────────────────────────────────────────────────────────────
 
@@ -227,6 +227,11 @@ pub struct TranscriptCursor {
     /// The `<latex>` formulas of the conversation's answers, oldest first:
     /// the last [`MAX_FORMULAS`].
     latex: VecDeque<String>,
+    /// The tags of the session's tag lenses ([`TranscriptCursor::with_tags`]).
+    tags: Vec<String>,
+    /// Their items in the conversation's answers, all tags together, oldest
+    /// first: the last [`MAX_FORMULAS`].
+    lens: VecDeque<LensItem>,
 }
 
 impl TranscriptCursor {
@@ -240,9 +245,17 @@ impl TranscriptCursor {
             last_message_id: None,
             totals: TranscriptTotals::default(),
             latex: VecDeque::new(),
+            tags: Vec::new(),
+            lens: VecDeque::new(),
         }
     }
 
+    /// The cursor, also gathering the items of tag lenses `tags`.
+    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
+        self.tags = tags;
+        self
+    }
+
     /// The claude session id this cursor follows.
     pub fn session_id(&self) -> &str {
         &self.session_id
@@ -257,6 +270,11 @@ impl TranscriptCursor {
         &self.latex
     }
 
+    /// The conversation's most recent tag-lens items, oldest first.
+    pub fn lens(&self) -> &VecDeque<LensItem> {
+        &self.lens
+    }
+
     /// Whether the cursor has seen an assistant record.
     pub fn has_assistant(&self) -> bool {
         self.totals.model.is_some()
@@ -277,6 +295,7 @@ impl TranscriptCursor {
             self.last_message_id = None;
             self.totals = TranscriptTotals::default();
             self.latex.clear();
+            self.lens.clear();
         }
         if len == self.offset {
             return false;
@@ -325,6 +344,7 @@ impl TranscriptCursor {
                 // A subagent's work is not the answer on screen.
                 if v.get("isSidechain").and_then(|s| s.as_bool()) != Some(true) {
                     push_latex(&mut self.latex, msg.get("content"));
+                    push_lens(&mut self.lens, &self.tags, msg.get("content"));
                 }
                 let Some(usage) = msg.get("usage") else { return };
                 let count = |key: &str| usage.get(key).and_then(|v| v.as_u64()).unwrap_or(0);
@@ -356,6 +376,37 @@ impl TranscriptCursor {
     }
 }
 
+/// Append the items of tag lenses `tags` in an assistant message to `lens`,
+/// keeping the last [`MAX_FORMULAS`]; only its `text` blocks count, as for
+/// [`push_latex`].
+fn push_lens(lens: &mut VecDeque<LensItem>, tags: &[String], content: Option<&serde_json::Value>) {
+    if tags.is_empty() {
+        return;
+    }
+    let tags: Vec<&str> = tags.iter().map(String::as_str).collect();
+    let mut add = |text: &str| {
+        for (_, tag, text) in lens_tags(text, &tags) {
+            if lens.len() == MAX_FORMULAS {
+                lens.pop_front();
+            }
+            lens.push_back(LensItem { tag, text });
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
 /// Append the formulas of an assistant message to `latex`, keeping the last
 /// [`MAX_FORMULAS`]. `content` is a string or an array of blocks, of which
 /// only `text` blocks are the answer (not `thinking` or `tool_use`).
diff --git a/src/daemon/session.rs b/src/daemon/session.rs
index 7441c00..5784782 100644
--- a/src/daemon/session.rs
+++ b/src/daemon/session.rs
@@ -32,7 +32,7 @@ use super::{host, Daemon};
 use crate::claude::hooks;
 use crate::claude::state::Tracker;
 use crate::proto::{
-    Envelope, Frame, Msg, SessionEvent, SessionId, SessionKind, SessionState, SpawnSpec,
+    Envelope, Frame, LensItem, Msg, SessionEvent, SessionId, SessionKind, SessionState, SpawnSpec,
 };
 use crate::proxy::env::env_diff;
 use crate::term::probe::ProbeResponder;
@@ -252,7 +252,8 @@ pub fn spawn(
         spawned_with_resume,
         spawn_time,
         kind,
-        meta: MetaRefresher::new(spec.id, Arc::clone(daemon), cwd, kind),
+        meta: MetaRefresher::new(spec.id, Arc::clone(daemon), cwd, kind)
+            .with_tags(spec.extensions.as_ref().map(|e| e.tags()).unwrap_or_default()),
     };
     tokio::spawn(actor.run(cmds, io.output, io.exit, committed_rx));
     Ok((Handle { tx, token, pid }, committed_tx))
@@ -524,7 +525,13 @@ impl Subscriber {
         if !self.sync(id, size, snapshot) {
             return false;
         }
-        let Some(event) = latex else { return true };
+        self.replay(id, latex)
+    }
+
+    /// Queue `event` (a replay after [`Subscriber::attach`]), if any.
+    /// Returns `false` if the client is gone.
+    fn replay(&mut self, id: SessionId, event: Option<SessionEvent>) -> bool {
+        let Some(event) = event else { return true };
         let frame = Frame::Control(Envelope::event(Msg::Event { id, event }));
         match self.queue.try_send(frame) {
             Ok(()) => true,
@@ -579,12 +586,18 @@ struct MetaRefresher {
     cursor: Arc<std::sync::Mutex<Option<crate::claude::projects::TranscriptCursor>>>,
     /// The formulas last sent to clients, replayed to a client that attaches.
     latex: Arc<std::sync::Mutex<LatexSent>>,
+    /// The session's tag lenses: the tags the transcript cursor gathers.
+    tags: Vec<String>,
+    /// The lens items last sent, replayed like `latex`.
+    lens: Arc<std::sync::Mutex<LensSent>>,
     busy: Arc<std::sync::atomic::AtomicBool>,
     started_at: Option<std::time::Instant>,
 }
 
 /// The last `SessionEvent::Latex` sent: its `seq` and formulas.
 type LatexSent = Option<(u64, Vec<String>)>;
+/// The last `SessionEvent::Lens` sent: its `seq` and items.
+type LensSent = Option<(u64, Vec<LensItem>)>;
 
 impl MetaRefresher {
     fn new(id: SessionId, daemon: Arc<Daemon>, cwd: PathBuf, kind: SessionKind) -> Self {
@@ -595,11 +608,24 @@ impl MetaRefresher {
             kind,
             cursor: Arc::default(),
             latex: Arc::default(),
+            tags: Vec::new(),
+            lens: Arc::default(),
             busy: Arc::default(),
             started_at: None,
         }
     }
 
+    /// The refresher, also sending the items of tag lenses `tags`.
+    fn with_tags(mut self, tags: Vec<String>) -> Self {
+        self.tags = tags;
+        self
+    }
+
+    /// The last lens items sent, as the event a client attaching now needs.
+    fn last_lens(&self) -> Option<SessionEvent> {
+        last_lens(&self.lens.lock().unwrap_or_else(|e| e.into_inner()))
+    }
+
     /// The last formulas sent, as the event a client attaching now needs.
     fn last_latex(&self) -> Option<SessionEvent> {
         last_latex(&self.latex.lock().unwrap_or_else(|e| e.into_inner()))
@@ -619,10 +645,11 @@ impl MetaRefresher {
         let (id, daemon, cwd, kind) = (self.id, Arc::clone(&self.daemon), self.cwd.clone(), self.kind);
         let (cursor, busy) = (Arc::clone(&self.cursor), Arc::clone(&self.busy));
         let latex = Arc::clone(&self.latex);
+        let (lens, tags) = (Arc::clone(&self.lens), self.tags.clone());
         let session_id = session_id.map(str::to_owned);
         tokio::spawn(async move {
             let git = super::git_status::read(&cwd).await;
-            let (transcript, formulas) = match (kind, session_id) {
+            let (transcript, formulas, items) = match (kind, session_id) {
                 (SessionKind::Claude, Some(sid)) => {
                     let cwd = cwd.clone();
                     tokio::task::spawn_blocking(move || {
@@ -630,22 +657,29 @@ impl MetaRefresher {
                         let same = slot.take().filter(|c| c.session_id() == sid);
                         let cursor = slot.insert(same.unwrap_or_else(|| {
                             crate::claude::projects::TranscriptCursor::new(&cwd, &sid)
+                                .with_tags(tags)
                         }));
                         cursor.advance();
                         let mut sent = latex.lock().unwrap_or_else(|e| e.into_inner());
                         let formulas = latex_event(&mut sent, cursor.latex());
+                        let mut sent = lens.lock().unwrap_or_else(|e| e.into_inner());
+                        let items = lens_items_event(&mut sent, cursor.lens());
                         let totals = cursor.has_assistant().then(|| cursor.totals().clone());
-                        (totals, formulas)
+                        (totals, formulas, items)
                     })
                     .await
                     .unwrap_or_default()
                 }
-                _ => (None, None),
+                _ => (None, None, None),
             };
             daemon.broadcast(id, meta_event(&cwd, git, transcript));
+            // Latex first, for old clients and in the order a replay has.
             if let Some(formulas) = formulas {
                 daemon.broadcast(id, formulas);
             }
+            if let Some(items) = items {
+                daemon.broadcast(id, items);
+            }
             busy.store(false, Ordering::Release);
         });
     }
@@ -680,6 +714,27 @@ fn last_latex(sent: &LatexSent) -> Option<SessionEvent> {
     })
 }
 
+/// The `Lens` event a refresh owes clients, by [`latex_event`]'s rules
+/// (its own `seq`), capped by [`crate::proto::lens_event`].
+fn lens_items_event(sent: &mut LensSent, lens: &VecDeque<LensItem>) -> Option<SessionEvent> {
+    let (seq, unchanged) = match sent {
+        Some((seq, list)) => (*seq, list.iter().eq(lens.iter())),
+        None => (0, lens.is_empty()),
+    };
+    if unchanged {
+        return None;
+    }
+    let items: Vec<LensItem> = lens.iter().cloned().collect();
+    *sent = Some((seq + 1, items.clone()));
+    Some(crate::proto::lens_event(seq + 1, items))
+}
+
+/// The event that replays `sent` to a client that attaches.
+fn last_lens(sent: &LensSent) -> Option<SessionEvent> {
+    sent.as_ref()
+        .map(|(seq, items)| crate::proto::lens_event(*seq, items.clone()))
+}
+
 /// The `Meta` event for a refresh's findings. The branch falls back to
 /// `.git/HEAD` when git itself could not answer.
 fn meta_event(
@@ -901,7 +956,10 @@ impl Actor {
             pending_req: req,
         };
         let latex = self.meta.last_latex();
-        if sub.attach(self.id, self.screen.size(), &self.screen.snapshot(), latex) {
+        // Lens items right after the formulas.
+        if sub.attach(self.id, self.screen.size(), &self.screen.snapshot(), latex)
+            && sub.replay(self.id, self.meta.last_lens())
+        {
             self.subs.insert(client, sub);
         }
     }
diff --git a/src/proto.rs b/src/proto.rs
index b985ae6..701e1f6 100644
--- a/src/proto.rs
+++ b/src/proto.rs
@@ -473,6 +473,15 @@ impl SessionExtensions {
         Ok(())
     }
 
+    /// The names of the tag lenses: the tags the daemon extracts.
+    pub fn tags(&self) -> Vec<String> {
+        self.lenses
+            .iter()
+            .filter(|l| l.kind == LensKind::Tag)
+            .map(|l| l.name.clone())
+            .collect()
+    }
+
     /// What claude is told: the non-empty instructions, sanitized, joined
     /// by a blank line.
     pub fn context(&self) -> String {
@@ -710,12 +719,48 @@ pub enum SessionEvent {
         seq: u64,
         formulas: Vec<String>,
     },
+    /// The conversation's recent tag-lens items (every tag lens of the
+    /// session, `latex` included), oldest first, by `Latex`'s rules: the
+    /// whole list, `seq` growing for the session's whole life. Sent beside
+    /// `Latex` by a daemon with the `lens` feature; at most
+    /// [`LENS_EVENT_MAX`] bytes encoded ([`lens_event`]). Old clients map
+    /// this to `Unknown`.
+    Lens {
+        seq: u64,
+        items: Vec<LensItem>,
+    },
     /// Catch-all for event kinds this client doesn't recognise yet.
     /// Keeps older clients alive when the daemon sends a newer event kind.
     #[serde(other)]
     Unknown,
 }
 
+/// One tag-lens item: the tag (the lens name) and its text, sanitized.
+#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
+pub struct LensItem {
+    pub tag: String,
+    pub text: String,
+}
+
+/// The most a `Lens` event takes encoded.
+pub const LENS_EVENT_MAX: usize = 256 * 1024;
+
+/// The `Lens` event for `items`, its oldest items dropped until it encodes
+/// to at most [`LENS_EVENT_MAX`] bytes.
+pub fn lens_event(seq: u64, items: Vec<LensItem>) -> SessionEvent {
+    let mut event = SessionEvent::Lens { seq, items };
+    while serde_json::to_vec(&event).map_or(0, |b| b.len()) > LENS_EVENT_MAX {
+        let SessionEvent::Lens { items, .. } = &mut event else {
+            break;
+        };
+        if items.is_empty() {
+            break;
+        }
+        items.remove(0);
+    }
+    event
+}
+
 /// A working tree's state, from `git status --porcelain=v2 --branch`.
 /// Counts are capped by the daemon; a zero everywhere is a clean tree.
 #[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
diff --git a/src/tui/interaction.rs b/src/tui/interaction.rs
index 61d9206..1d4cc7c 100644
--- a/src/tui/interaction.rs
+++ b/src/tui/interaction.rs
@@ -835,6 +835,8 @@ impl App {
                     self.emit(render);
                 }
             }
+            // Taken from Task 3 on (`Welcome.features`).
+            SessionEvent::Lens { .. } => {}
             SessionEvent::Unknown => {}
         }
     }
`````
Run: `umask 022; cargo test --locked --bin claudio`
Expected: `test result: ok. 939 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`
Also: `rustfmt --edition 2021 --check src/claude/latex.rs` prints nothing.

Step 3: commit
`/usr/bin/git commit -m "feat(daemon): lens items in the transcript and the lens event" -- src/claude/latex.rs src/claude/projects.rs src/daemon/session.rs src/proto.rs src/tui/interaction.rs`

Interfaces this task provides to later tasks: `claude::latex::lens_tags`, `proto::{LensItem, LENS_EVENT_MAX, lens_event}`, `SessionEvent::Lens { seq, items }`, `SessionExtensions::tags()`, `TranscriptCursor::{with_tags, lens}`.

### Task 3: client registry, manifests and config
Model: sonnet   Repo: <repo>   Files: src/tui/extensions/mod.rs (new), src/tui/extensions/manifest.rs (new), src/config.rs, src/tui/app.rs, src/tui/interaction.rs, src/tui/sessions.rs, src/tui/mod.rs
Goal: one registry of built-in and external extensions drives the spawn context and which lens items the client keeps (AC4 client side, AC11).

What this task does:
- `tui/extensions/mod.rs`: `Lens`, `ActionDecl`, `Source { BuiltIn, External(Program) }`, `Extension`, `builtins()` (latex: tag lens with `LATEX_CONTEXT`; diagram: fence lens `mermaid` with the spec's instruction `DIAGRAM_CONTEXT`; search: `s` with input "search conversations"; snippets: `p`, `P`), `READER_KEY = 'u'`, `Registry` (`new(enabled, latex, external, notices)`, `enabled`, `lenses(kind)`, `tag_lenses`, `fence_lenses`, `external_lens`, `session_extensions` (registry order, within the protocol caps), `actions`).
- `tui/extensions/manifest.rs`: `Program { dir, command }` (`program()` resolves a first element with `/` against the dir), `HTML_ELEMENTS` (the WHATWG list plus obsolete ones), `parse(dir_name, dir, text)` (all manifest rules; `html = true` takes an HTML name), `admit(candidates, notices)` (name/tag/fence clashes skip the extension, a key clash the action, at most 16), `load(dir, notices)` (file I/O: called from `tui/mod.rs` at start only; manifests ≤ 64 KiB), `dir()`.
- `config.rs`: `[extensions] name = bool` (`ExtensionsSection`). `tui/mod.rs` loads the manifests and builds the registry at start (notices with the key notices); `App.extensions` (built-ins by default) and `App.set_host_features` (from every `Welcome`: local at start, `Connected`, `LocalReconnected`) → `lens_hosts`.
- `send_spawn` and `reset_session` (the one place spawns and respawns are built) send `extensions: Some(registry.session_extensions())`.
- `SessionView` keeps `latex_seq` (now the seq of the lens list taken), `latex_srcs`, `latex`, and gains `lens_items` and `lens` (external renderings, filled by Task 5). `take_lens_items`: a `Lens` event from a lens host, or an old daemon's `Latex` event mapped to `latex` items; only enabled tag lenses' items are kept; latex items go to the worker as today (`Effect::RenderLatex`).

Step 1: write the failing tests
`````diff
diff --git a/src/config.rs b/src/config.rs
index 14ba329..8652d35 100644
--- a/src/config.rs
+++ b/src/config.rs
@@ -321,4 +332,17 @@ default = "x"
 
         let _ = std::fs::remove_dir_all(&dir);
     }
+
+    #[test]
+    fn extensions_section_turns_extensions_on_and_off() {
+        let dir = std::env::temp_dir().join(format!("claudio-cfg-test-{}", uuid::Uuid::new_v4()));
+        std::fs::create_dir_all(&dir).unwrap();
+        let path = dir.join("config.toml");
+        std::fs::write(&path, "[extensions]\ndiagram = false\nkeys = true\n").unwrap();
+        let cfg = load_from(&path);
+        assert_eq!(cfg.extensions.get("diagram"), Some(&false));
+        assert_eq!(cfg.extensions.get("keys"), Some(&true));
+        assert!(load_from(&dir.join("missing.toml")).extensions.is_empty());
+        let _ = std::fs::remove_dir_all(&dir);
+    }
 }
diff --git a/src/tui/extensions/manifest.rs b/src/tui/extensions/manifest.rs
new file mode 100644
index 0000000..d5b04c8
--- /dev/null
+++ b/src/tui/extensions/manifest.rs
@@ -0,0 +1,179 @@
+#[cfg(test)]
+mod tests {
+    use super::*;
+
+    const KEYS: &str = r#"
+name = "keys"
+description = "keyboard keys"
+command = ["./keys.sh", "--fast"]
+[[lens]]
+tag = "keys"
+instruction = "Write keys as <keys>Ctrl+C</keys>."
+[[lens]]
+fence = "abc"
+[[action]]
+key = "k"
+title = "insert a timestamp"
+id = "stamp"
+input = "label"
+"#;
+
+    fn parsed(text: &str) -> Result<Extension, String> {
+        parse("keys", Path::new("/ext/keys"), text)
+    }
+
+    #[test]
+    fn a_manifest_gives_its_lenses_actions_and_program() {
+        let ext = parsed(KEYS).unwrap();
+        assert_eq!(ext.name, "keys");
+        assert_eq!(ext.description, "keyboard keys");
+        assert_eq!(ext.lenses.len(), 2);
+        assert_eq!(
+            (ext.lenses[0].name.as_str(), ext.lenses[0].kind),
+            ("keys", LensKind::Tag)
+        );
+        assert_eq!(
+            (ext.lenses[1].name.as_str(), ext.lenses[1].kind),
+            ("abc", LensKind::Fence)
+        );
+        let action = &ext.actions[0];
+        assert_eq!((action.key, action.id.as_str()), ('k', "stamp"));
+        assert_eq!(action.input.as_deref(), Some("label"));
+        let Source::External(program) = &ext.source else {
+            panic!("not external")
+        };
+        assert_eq!(program.program(), Path::new("/ext/keys/./keys.sh"));
+        let bare = Program {
+            dir: "/ext/keys".into(),
+            command: vec!["python3".into()],
+        };
+        assert_eq!(bare.program(), Path::new("python3"));
+    }
+
+    #[test]
+    fn invalid_manifests_say_why() {
+        let cases = [
+            (
+                KEYS.replace("name = \"keys\"", "name = \"other\""),
+                "not its directory",
+            ),
+            (
+                KEYS.replace("name = \"keys\"", "name = \"Keys\""),
+                "bad name",
+            ),
+            (
+                KEYS.replace("[\"./keys.sh\", \"--fast\"]", "[]"),
+                "command is empty",
+            ),
+            (
+                KEYS.replace("fence = \"abc\"", "tag = \"x\"\nfence = \"y\""),
+                "exactly one",
+            ),
+            (
+                KEYS.replace("tag = \"keys\"", "tag = \"kbd\""),
+                "tag kbd is an HTML element; pick another name",
+            ),
+            (
+                KEYS.replace("tag = \"keys\"", "tag = \"Bad tag\""),
+                "bad lens name",
+            ),
+            (
+                KEYS.replace("key = \"k\"", "key = \"kk\""),
+                "bad action key",
+            ),
+            (KEYS.replace("key = \"k\"", "key = \"-\""), "bad action key"),
+            (
+                KEYS.replace("id = \"stamp\"", "id = \"9\""),
+                "bad action id",
+            ),
+            (
+                KEYS.replace("insert a timestamp", &"t".repeat(61)),
+                "title is longer",
+            ),
+            (
+                KEYS.replace("Write keys", &"w".repeat(2049)),
+                "instruction over",
+            ),
+            ("name = 3".to_owned(), "invalid manifest"),
+        ];
+        for (text, why) in cases {
+            let err = parsed(&text).unwrap_err();
+            assert!(err.contains(why), "{err:?} lacks {why:?}");
+        }
+        // `html = true` takes the HTML name knowingly.
+        let html = KEYS.replace("tag = \"keys\"", "tag = \"kbd\"\nhtml = true");
+        assert_eq!(parsed(&html).unwrap().lenses[0].name, "kbd");
+    }
+
+    fn ext(name: &str, lens: &str, key: char) -> Extension {
+        let text = KEYS
+            .replace("name = \"keys\"", &format!("name = \"{name}\""))
+            .replace("tag = \"keys\"", &format!("tag = \"{lens}\""))
+            .replace("fence = \"abc\"", &format!("fence = \"{lens}-f\""))
+            .replace("key = \"k\"", &format!("key = \"{key}\""));
+        parse(name, Path::new("/ext"), &text).unwrap()
+    }
+
+    #[test]
+    fn admission_skips_clashes_with_one_notice_each() {
+        let mut notices = Vec::new();
+        let admitted = admit(
+            vec![
+                ext("keys", "keys", 'k'),
+                ext("search", "x", 'x'),    // a built-in's name
+                ext("other", "keys", 'o'),  // a taken tag
+                ext("third", "latex", 't'), // a built-in's lens
+                ext("fourth", "four", 's'), // a taken key: the action goes
+                ext("fifth", "five", 'u'),  // the reader's key
+            ],
+            &mut notices,
+        );
+        let names: Vec<&str> = admitted.iter().map(|e| e.name.as_str()).collect();
+        assert_eq!(names, ["keys", "fourth", "fifth"]);
+        assert!(admitted[1].actions.is_empty() && admitted[2].actions.is_empty());
+        assert_eq!(
+            notices,
+            [
+                "extension search: the name is taken",
+                "extension other: lens keys is taken",
+                "extension third: lens latex is taken",
+                "extension fourth: key s is taken",
+                "extension fifth: key u is taken",
+            ]
+        );
+        let many: Vec<Extension> = (0..18)
+            .map(|i| {
+                ext(
+                    &format!("e{i}"),
+                    &format!("t{i}"),
+                    char::from(b'a' + i as u8),
+                )
+            })
+            .collect();
+        let mut notices = Vec::new();
+        assert_eq!(admit(many, &mut notices).len(), MAX_EXTERNAL);
+        assert_eq!(notices.len(), 2 + 1, "two over the cap, and key p is taken");
+    }
+
+    #[test]
+    fn load_reads_each_directory_and_reports_bad_ones() {
+        let dir = std::env::temp_dir().join(format!("claudio-ext-{}", uuid::Uuid::new_v4()));
+        for (name, text) in [("keys", KEYS.to_owned()), ("bad", "nonsense".to_owned())] {
+            std::fs::create_dir_all(dir.join(name)).unwrap();
+            std::fs::write(dir.join(name).join("extension.toml"), text).unwrap();
+        }
+        std::fs::create_dir_all(dir.join("big")).unwrap();
+        std::fs::write(dir.join("big/extension.toml"), "#".repeat(70_000)).unwrap();
+        std::fs::create_dir_all(dir.join("empty")).unwrap();
+        let mut notices = Vec::new();
+        let loaded = load(&dir, &mut notices);
+        assert_eq!(loaded.len(), 1);
+        assert_eq!(loaded[0].name, "keys");
+        assert_eq!(notices.len(), 3, "{notices:?}");
+        assert!(notices[0].starts_with("extension bad: invalid manifest"));
+        assert_eq!(notices[1], "extension big: manifest over 64 KiB");
+        assert!(notices[2].starts_with("extension empty: "));
+        assert!(load(&dir.join("missing"), &mut notices).is_empty());
+        std::fs::remove_dir_all(&dir).unwrap();
+    }
+}
diff --git a/src/tui/extensions/mod.rs b/src/tui/extensions/mod.rs
new file mode 100644
index 0000000..9544839
--- /dev/null
+++ b/src/tui/extensions/mod.rs
@@ -0,0 +1,111 @@
+#[cfg(test)]
+mod tests {
+    use super::*;
+
+    fn external(name: &str, tag: &str, key: char) -> Extension {
+        Extension {
+            name: name.into(),
+            description: String::new(),
+            enabled: true,
+            lenses: vec![Lens {
+                name: tag.into(),
+                kind: LensKind::Tag,
+                instruction: format!("write <{tag}>"),
+            }],
+            actions: vec![ActionDecl {
+                key,
+                title: "t".into(),
+                id: "a".into(),
+                input: None,
+            }],
+            source: Source::External(manifest::Program {
+                dir: "/x".into(),
+                command: vec!["prog".into()],
+            }),
+        }
+    }
+
+    fn names(ext: &SessionExtensions) -> Vec<&str> {
+        ext.lenses.iter().map(|l| l.name.as_str()).collect()
+    }
+
+    #[test]
+    fn the_builtins_give_latex_and_mermaid_and_the_leader_actions() {
+        let r = Registry::default();
+        assert_eq!(r.tag_lenses(), ["latex"]);
+        assert_eq!(r.fence_lenses(), ["mermaid"]);
+        let ext = r.session_extensions();
+        assert_eq!(names(&ext), ["latex", "mermaid"]);
+        assert_eq!(
+            ext.lenses[0].instruction,
+            crate::claude::hooks::LATEX_CONTEXT
+        );
+        assert_eq!(ext.lenses[1].kind, LensKind::Fence);
+        assert_eq!(ext.validate(), Ok(()));
+        let keys: Vec<char> = r.actions().iter().map(|(_, a)| a.key).collect();
+        assert_eq!(keys, ['s', 'p', 'P']);
+        assert!(r.external_lens("latex").is_none());
+    }
+
+    #[test]
+    fn config_turns_extensions_off_and_names_unknown_ones() {
+        let mut notices = Vec::new();
+        let enabled = BTreeMap::from([
+            ("diagram".to_owned(), false),
+            ("snippets".to_owned(), false),
+            ("nope".to_owned(), false),
+        ]);
+        let r = Registry::new(&enabled, true, vec![], &mut notices);
+        assert_eq!(names(&r.session_extensions()), ["latex"]);
+        assert!(r.fence_lenses().is_empty());
+        let keys: Vec<char> = r.actions().iter().map(|(_, a)| a.key).collect();
+        assert_eq!(keys, ['s']);
+        assert_eq!(
+            notices,
+            ["config.toml [extensions]: no extension named 'nope'"]
+        );
+        // `[claude] latex = false` is the latex extension off.
+        let r = Registry::new(&BTreeMap::new(), false, vec![], &mut notices);
+        assert!(r.tag_lenses().is_empty());
+        assert_eq!(names(&r.session_extensions()), ["mermaid"]);
+    }
+
+    #[test]
+    fn external_extensions_follow_the_builtins() {
+        let mut notices = Vec::new();
+        let ext = vec![external("keys", "keys", 'k')];
+        let r = Registry::new(&BTreeMap::new(), true, ext, &mut notices);
+        assert_eq!(r.tag_lenses(), ["latex", "keys"]);
+        assert_eq!(names(&r.session_extensions()), ["latex", "mermaid", "keys"]);
+        assert_eq!(
+            r.external_lens("keys").map(|e| e.name.as_str()),
+            Some("keys")
+        );
+        let keys: Vec<char> = r.actions().iter().map(|(_, a)| a.key).collect();
+        assert_eq!(keys, ['s', 'p', 'P', 'k']);
+        let off = BTreeMap::from([("keys".to_owned(), false)]);
+        let r = Registry::new(
+            &off,
+            true,
+            vec![external("keys", "keys", 'k')],
+            &mut notices,
+        );
+        assert!(r.external_lens("keys").is_none());
+        assert!(notices.is_empty());
+    }
+
+    #[test]
+    fn the_session_context_stays_within_the_caps() {
+        let mut ext: Vec<Extension> = (0..16)
+            .map(|i| external(&format!("e{i}"), &format!("t{i}"), 'k'))
+            .collect();
+        ext[0].lenses[0].instruction = "i".repeat(2048);
+        ext[1].lenses[0].instruction = "i".repeat(2048);
+        ext[2].lenses[0].instruction = "i".repeat(2048);
+        let r = Registry::new(&BTreeMap::new(), true, ext, &mut Vec::new());
+        let ctx = r.session_extensions();
+        assert_eq!(ctx.validate(), Ok(()));
+        assert_eq!(ctx.lenses.len(), 16);
+        assert_eq!(&names(&ctx)[..3], ["latex", "mermaid", "t0"]);
+    }
+}
diff --git a/src/tui/interaction.rs b/src/tui/interaction.rs
index 1d4cc7c..078539a 100644
--- a/src/tui/interaction.rs
+++ b/src/tui/interaction.rs
@@ -996,6 +1028,7 @@ pub fn overview_matches(v: &SessionView, filter: &str) -> bool {
 mod tests {
     use super::*;
     use crate::tui::app::AppConfig;
+    use crate::tui::extensions::Registry;
     use crate::tui::test_support::{self, app_with, info, press};
     use crossterm::event::Event;
     use ratatui::{backend::TestBackend, Terminal};
@@ -1262,6 +1295,87 @@ mod tests {
             .any(|e| matches!(e, Effect::RenderLatex { seq: 1, .. })));
     }
 
+    #[test]
+    fn a_lens_daemon_sends_lens_items_and_an_old_one_latex() {
+        let mut app = app_with(&[info(Some(1), None)]);
+        let (id, host) = (app.sessions[0].id, app.sessions[0].host.clone());
+        let item = |tag: &str, text: &str| LensItem {
+            tag: tag.into(),
+            text: text.into(),
+        };
+        let lens = |seq, items: Vec<LensItem>| Incoming::Event {
+            id,
+            event: SessionEvent::Lens { seq, items },
+        };
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
+        // An old daemon: its formulas are latex items; Lens is not its.
+        app.on_incoming_from(&host, lens(5, vec![item("latex", "z")]));
+        assert!(renders(&mut app).is_empty());
+        app.on_incoming_from(&host, latex(1, &["a"]));
+        assert_eq!(renders(&mut app), [(1, vec!["a".to_owned()])]);
+        assert_eq!(app.sessions[0].lens_items, [item("latex", "a")]);
+
+        // A lens daemon: Lens is taken, Latex ignored; items of lenses
+        // that are not enabled are dropped.
+        app.set_host_features(&host, &["lens".to_owned()]);
+        app.on_incoming_from(&host, latex(7, &["ignored"]));
+        assert!(renders(&mut app).is_empty());
+        let items = vec![item("keys", "k"), item("latex", "b"), item("latex", "c")];
+        app.on_incoming_from(&host, lens(2, items));
+        assert_eq!(renders(&mut app), [(2, vec!["b".to_owned(), "c".to_owned()])]);
+        assert_eq!(app.sessions[0].lens_items, [item("latex", "b"), item("latex", "c")]);
+        // An older list is ignored, as for latex.
+        app.on_incoming_from(&host, lens(1, vec![item("latex", "old")]));
+        assert!(renders(&mut app).is_empty());
+        // A daemon that lost the feature (a downgrade) is an old one again.
+        app.set_host_features(&host, &[]);
+        app.on_incoming_from(&host, latex(3, &["d"]));
+        assert_eq!(renders(&mut app), [(3, vec!["d".to_owned()])]);
+    }
+
+    #[test]
+    fn with_latex_off_formulas_are_not_rendered_and_spawns_say_so() {
+        let mut app = app_with(&[info(Some(1), None)]);
+        let (id, host) = (app.sessions[0].id, app.sessions[0].host.clone());
+        let off = std::collections::BTreeMap::from([("latex".to_owned(), false)]);
+        app.extensions = Registry::new(&off, true, vec![], &mut Vec::new());
+        let event = SessionEvent::Latex {
+            seq: 1,
+            formulas: vec!["x".into()],
+        };
+        app.on_incoming_from(&host, Incoming::Event { id, event });
+        assert!(app.sessions[0].latex_srcs.is_empty());
+        assert!(app.sessions[0].lens_items.is_empty());
+        // A respawn carries the client's lenses: mermaid only.
+        app.take_effects();
+        app.reset_session(id, false);
+        let sent = app.take_effects().into_iter().find_map(|e| match e {
+            Effect::Request {
+                msg: Msg::Respawn(spec),
+                ..
+            } => Some(spec),
+            _ => None,
+        });
+        let ext = sent.expect("a respawn").extensions.expect("extensions");
+        let names: Vec<&str> = ext.lenses.iter().map(|l| l.name.as_str()).collect();
+        assert_eq!(names, ["mermaid"]);
+    }
+
     #[test]
     fn alt_u_opens_and_closes_the_reader() {
         use crate::tui::test_support::alt;
`````
Run: `umask 022; cargo test --locked --bin claudio 2>&1 | grep -E '^error' | head -3`
Expected: `` error[E0432]: unresolved import `crate::tui::extensions` ``

Step 2: implement
`````diff
diff --git a/src/config.rs b/src/config.rs
index 227f6bb..8652d35 100644
--- a/src/config.rs
+++ b/src/config.rs
@@ -16,6 +16,9 @@
 //! latex = true                    # ask claude to wrap math in <latex> tags
 //!                                 # (read at daemon start: `claudio daemon restart`)
 //!
+//! [extensions]
+//! diagram = false                 # any extension, built-in or external, by name
+//!
 //! [keys]
 //! overview    = "alt+g"
 //! help        = "alt+h"
@@ -136,6 +139,10 @@ impl Default for ClaudeSection {
 /// names and invalid key specs are ignored with a notice.
 pub type KeysSection = BTreeMap<String, String>;
 
+/// The `[extensions]` section of `config.toml`: extension name → on/off
+/// (every extension is on unless listed as `false`).
+pub type ExtensionsSection = BTreeMap<String, bool>;
+
 /// The subset of `config.toml` that this module owns. Other sections (e.g.
 /// `[proxy]`) are preserved via `extra` on round-trip.
 #[derive(Debug, Default, Serialize, Deserialize)]
@@ -148,6 +155,8 @@ struct ConfigFile {
     pub update: Option<UpdateSection>,
     #[serde(default)]
     pub claude: Option<ClaudeSection>,
+    #[serde(default)]
+    pub extensions: Option<ExtensionsSection>,
     /// All other TOML keys — preserved on write.
     #[serde(flatten)]
     extra: toml::Table,
@@ -162,6 +171,7 @@ pub struct Config {
     pub keys: KeysSection,
     pub update: UpdateSection,
     pub claude: ClaudeSection,
+    pub extensions: ExtensionsSection,
 }
 
 // ── Loading ───────────────────────────────────────────────────────────────────
@@ -215,6 +225,7 @@ fn try_load_from(path: &Path) -> io::Result<Config> {
         keys: file.keys.unwrap_or_default(),
         update: file.update.unwrap_or_default(),
         claude: file.claude.unwrap_or_default(),
+        extensions: file.extensions.unwrap_or_default(),
     })
 }
 
diff --git a/src/tui/app.rs b/src/tui/app.rs
index b534488..3c5bf55 100644
--- a/src/tui/app.rs
+++ b/src/tui/app.rs
@@ -301,6 +301,13 @@ pub struct App {
     pub selection: Option<Selection>,
     /// Press timing for double and triple clicks.
     pub(super) clicks: Clicks,
+    /// The extensions (built-in and external) this client runs with; the
+    /// event loop sets the configured ones at start.
+    pub extensions: super::extensions::Registry,
+    /// Hosts whose daemon has the `lens` feature: their `Lens` events are
+    /// taken and their `Latex` events ignored (an older daemon's `Latex`
+    /// events are taken as `latex` items instead).
+    pub(super) lens_hosts: HashSet<String>,
 }
 
 /// What the app starts from. The event loop gathers it (reading config.toml,
@@ -340,6 +347,7 @@ pub struct AppConfig {
 impl App {
     pub fn new(cfg: AppConfig) -> App {
         let (width, height) = cfg.size;
+        let latex = cfg.claude.latex;
         App {
             mode: cfg.mode,
             exit: Default::default(),
@@ -383,6 +391,22 @@ impl App {
             copy_on_select: cfg.copy_on_select,
             selection: None,
             clicks: Clicks::default(),
+            extensions: super::extensions::Registry::new(
+                &Default::default(),
+                latex,
+                Vec::new(),
+                &mut Vec::new(),
+            ),
+            lens_hosts: HashSet::new(),
+        }
+    }
+
+    /// What `host`'s daemon said it can do (its `Welcome.features`).
+    pub fn set_host_features(&mut self, host: &str, features: &[String]) {
+        if features.iter().any(|f| f == "lens") {
+            self.lens_hosts.insert(host.to_owned());
+        } else {
+            self.lens_hosts.remove(host);
         }
     }
 
diff --git a/src/tui/extensions/manifest.rs b/src/tui/extensions/manifest.rs
index 5d38e7c..d5b04c8 100644
--- a/src/tui/extensions/manifest.rs
+++ b/src/tui/extensions/manifest.rs
@@ -1,3 +1,424 @@
+//! External extensions: `~/.config/claudio/extensions/<name>/extension.toml`.
+//!
+//! ```toml
+//! name = "keys"                      # the directory's name
+//! description = "keyboard keys"
+//! command = ["python3", "keys.py"]   # argv, no shell
+//! [[lens]]
+//! tag = "keys"                       # or: fence = "abc"
+//! instruction = "Write keyboard keys as <keys>Ctrl+C</keys>."
+//! [[action]]
+//! key = "k"
+//! title = "insert a timestamp"
+//! id = "stamp"
+//! input = "label"                    # optional
+//! ```
+//!
+//! [`parse`] checks one manifest, [`admit`] checks them against the
+//! built-ins and each other, and [`load`] (file I/O: called by `tui/mod.rs`
+//! at start, never by `App`) reads the directory. Whatever is skipped gets
+//! one notice.
+
+use std::path::{Path, PathBuf};
+
+use serde::Deserialize;
+
+use super::{builtins, ActionDecl, Extension, Lens, Source, READER_KEY};
+use crate::claude::latex::sanitize;
+use crate::proto::{lens_name_ok, LensKind, MAX_INSTRUCTION};
+
+/// The most external extensions loaded.
+pub const MAX_EXTERNAL: usize = 16;
+/// The largest manifest read.
+const MAX_MANIFEST: u64 = 64 * 1024;
+const MAX_DESCRIPTION: usize = 120;
+const MAX_TITLE: usize = 60;
+const MAX_PROMPT: usize = 120;
+
+/// HTML element names (the WHATWG list, with obsolete ones claude may still
+/// write): a tag lens with one of these names would take what markdown and
+/// claude mean as HTML, unless its manifest sets `html = true`.
+pub const HTML_ELEMENTS: &[&str] = &[
+    "a",
+    "abbr",
+    "acronym",
+    "address",
+    "applet",
+    "area",
+    "article",
+    "aside",
+    "audio",
+    "b",
+    "base",
+    "basefont",
+    "bdi",
+    "bdo",
+    "big",
+    "blink",
+    "blockquote",
+    "body",
+    "br",
+    "button",
+    "canvas",
+    "caption",
+    "center",
+    "cite",
+    "code",
+    "col",
+    "colgroup",
+    "data",
+    "datalist",
+    "dd",
+    "del",
+    "details",
+    "dfn",
+    "dialog",
+    "dir",
+    "div",
+    "dl",
+    "dt",
+    "em",
+    "embed",
+    "fieldset",
+    "figcaption",
+    "figure",
+    "font",
+    "footer",
+    "form",
+    "frame",
+    "frameset",
+    "h1",
+    "h2",
+    "h3",
+    "h4",
+    "h5",
+    "h6",
+    "head",
+    "header",
+    "hgroup",
+    "hr",
+    "html",
+    "i",
+    "iframe",
+    "img",
+    "input",
+    "ins",
+    "kbd",
+    "label",
+    "legend",
+    "li",
+    "link",
+    "main",
+    "map",
+    "mark",
+    "marquee",
+    "menu",
+    "meta",
+    "meter",
+    "nav",
+    "noembed",
+    "noframes",
+    "noscript",
+    "object",
+    "ol",
+    "optgroup",
+    "option",
+    "output",
+    "p",
+    "param",
+    "picture",
+    "plaintext",
+    "pre",
+    "progress",
+    "q",
+    "rb",
+    "rp",
+    "rt",
+    "rtc",
+    "ruby",
+    "s",
+    "samp",
+    "script",
+    "search",
+    "section",
+    "select",
+    "slot",
+    "small",
+    "source",
+    "span",
+    "strike",
+    "strong",
+    "style",
+    "sub",
+    "summary",
+    "sup",
+    "table",
+    "tbody",
+    "td",
+    "template",
+    "textarea",
+    "tfoot",
+    "th",
+    "thead",
+    "time",
+    "title",
+    "tr",
+    "track",
+    "tt",
+    "u",
+    "ul",
+    "var",
+    "video",
+    "wbr",
+    "xmp",
+];
+
+/// How an external extension's program is started.
+#[derive(Debug, Clone, PartialEq)]
+pub struct Program {
+    /// The extension's directory: the program's cwd.
+    pub dir: PathBuf,
+    /// Its argv; a first element with a `/` is relative to `dir`.
+    pub command: Vec<String>,
+}
+
+impl Program {
+    /// The program to execute: the first element resolved against `dir`
+    /// when it contains a `/`, else as given (looked up in `PATH`).
+    pub fn program(&self) -> PathBuf {
+        let first = self.command.first().map(String::as_str).unwrap_or("");
+        if first.contains('/') {
+            self.dir.join(first)
+        } else {
+            PathBuf::from(first)
+        }
+    }
+}
+
+#[derive(Deserialize)]
+struct File {
+    name: String,
+    #[serde(default)]
+    description: String,
+    command: Vec<String>,
+    #[serde(default, rename = "lens")]
+    lenses: Vec<LensFile>,
+    #[serde(default, rename = "action")]
+    actions: Vec<ActionFile>,
+}
+
+#[derive(Deserialize)]
+struct LensFile {
+    tag: Option<String>,
+    fence: Option<String>,
+    #[serde(default)]
+    instruction: String,
+    #[serde(default)]
+    html: bool,
+}
+
+#[derive(Deserialize)]
+struct ActionFile {
+    key: String,
+    title: String,
+    id: String,
+    input: Option<String>,
+}
+
+/// `^[a-z][a-z0-9-]{0,31}$`.
+fn extension_name_ok(name: &str) -> bool {
+    let mut chars = name.chars();
+    name.len() <= 32
+        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
+        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
+}
+
+/// `^[a-z][a-z0-9_-]{0,31}$`.
+fn action_id_ok(id: &str) -> bool {
+    let mut chars = id.chars();
+    id.len() <= 32
+        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
+        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
+}
+
+/// A text field: sanitized, on one line, at most `max` chars.
+fn line_field(what: &str, text: &str, max: usize) -> Result<String, String> {
+    let clean = sanitize(text).replace(['\n', '\t'], " ");
+    if clean.chars().count() > max {
+        return Err(format!("{what} is longer than {max} characters"));
+    }
+    Ok(clean)
+}
+
+/// The extension in directory `dir` (named `dir_name`) whose manifest is
+/// `text`, or why it is not acceptable.
+pub fn parse(dir_name: &str, dir: &Path, text: &str) -> Result<Extension, String> {
+    let file: File =
+        toml::from_str(text).map_err(|e| format!("invalid manifest: {}", e.message()))?;
+    if !extension_name_ok(&file.name) {
+        return Err(format!("bad name '{}'", file.name));
+    }
+    if file.name != dir_name {
+        return Err(format!("name '{}' is not its directory's", file.name));
+    }
+    let description = line_field("description", &file.description, MAX_DESCRIPTION)?;
+    if file.command.first().is_none_or(|c| c.is_empty()) {
+        return Err("command is empty".to_owned());
+    }
+    let mut lenses = Vec::new();
+    for lens in file.lenses {
+        let (name, kind) = match (lens.tag, lens.fence) {
+            (Some(tag), None) => (tag, LensKind::Tag),
+            (None, Some(fence)) => (fence, LensKind::Fence),
+            _ => return Err("a lens needs exactly one of tag and fence".to_owned()),
+        };
+        if !lens_name_ok(&name) {
+            return Err(format!("bad lens name '{name}'"));
+        }
+        if kind == LensKind::Tag && !lens.html && HTML_ELEMENTS.contains(&name.as_str()) {
+            return Err(format!("tag {name} is an HTML element; pick another name"));
+        }
+        if sanitize(&lens.instruction).len() > MAX_INSTRUCTION {
+            return Err(format!(
+                "lens {name}: instruction over {MAX_INSTRUCTION} bytes"
+            ));
+        }
+        if lenses.iter().any(|l: &Lens| l.name == name) {
+            return Err(format!("lens {name} given twice"));
+        }
+        lenses.push(Lens {
+            name,
+            kind,
+            instruction: lens.instruction,
+        });
+    }
+    let mut actions = Vec::new();
+    for action in file.actions {
+        let mut chars = action.key.chars();
+        let key = match (chars.next(), chars.next()) {
+            (Some(k), None) if k.is_ascii_alphanumeric() => k,
+            _ => return Err(format!("bad action key '{}'", action.key)),
+        };
+        if !action_id_ok(&action.id) {
+            return Err(format!("bad action id '{}'", action.id));
+        }
+        let input = match action.input {
+            Some(prompt) => Some(line_field("input", &prompt, MAX_PROMPT)?),
+            None => None,
+        };
+        actions.push(ActionDecl {
+            key,
+            title: line_field("title", &action.title, MAX_TITLE)?,
+            id: action.id,
+            input,
+        });
+    }
+    Ok(Extension {
+        name: file.name,
+        description,
+        enabled: true,
+        lenses,
+        actions,
+        source: Source::External(Program {
+            dir: dir.to_path_buf(),
+            command: file.command,
+        }),
+    })
+}
+
+/// `candidates` (parsed, in directory order) that can join the built-ins:
+/// at most [`MAX_EXTERNAL`]; an extension whose name, tag or fence is
+/// taken is skipped whole, an action whose key is taken alone. Each skip
+/// gets a notice.
+pub fn admit(candidates: Vec<Extension>, notices: &mut Vec<String>) -> Vec<Extension> {
+    let mut taken = builtins();
+    let mut keys: Vec<char> = taken
+        .iter()
+        .flat_map(|e| e.actions.iter().map(|a| a.key))
+        .chain([READER_KEY])
+        .collect();
+    let builtin_count = taken.len();
+    for mut ext in candidates {
+        if taken.len() - builtin_count == MAX_EXTERNAL {
+            notices.push(format!(
+                "extension {}: skipped (at most {MAX_EXTERNAL} extensions)",
+                ext.name
+            ));
+            continue;
+        }
+        if taken.iter().any(|e| e.name == ext.name) {
+            notices.push(format!("extension {}: the name is taken", ext.name));
+            continue;
+        }
+        let clash = ext.lenses.iter().find(|l| {
+            taken
+                .iter()
+                .flat_map(|e| e.lenses.iter())
+                .any(|t| t.name == l.name)
+        });
+        if let Some(lens) = clash {
+            notices.push(format!(
+                "extension {}: lens {} is taken",
+                ext.name, lens.name
+            ));
+            continue;
+        }
+        let name = ext.name.clone();
+        ext.actions.retain(|a| {
+            if keys.contains(&a.key) {
+                notices.push(format!("extension {name}: key {} is taken", a.key));
+                return false;
+            }
+            keys.push(a.key);
+            true
+        });
+        taken.push(ext);
+    }
+    taken.split_off(builtin_count)
+}
+
+/// The external extensions under `dir` (`<name>/extension.toml`, by name),
+/// admitted; what is skipped gets a notice. A missing `dir` has none.
+pub fn load(dir: &Path, notices: &mut Vec<String>) -> Vec<Extension> {
+    use std::io::Read;
+    let Ok(entries) = std::fs::read_dir(dir) else {
+        return Vec::new();
+    };
+    let mut dirs: Vec<PathBuf> = entries
+        .flatten()
+        .map(|e| e.path())
+        .filter(|p| p.is_dir())
+        .collect();
+    dirs.sort();
+    let mut candidates = Vec::new();
+    for ext_dir in dirs {
+        let name = ext_dir
+            .file_name()
+            .map(|n| n.to_string_lossy().into_owned())
+            .unwrap_or_default();
+        let manifest = ext_dir.join("extension.toml");
+        let mut text = String::new();
+        let read = std::fs::File::open(&manifest)
+            .and_then(|f| f.take(MAX_MANIFEST + 1).read_to_string(&mut text));
+        match read {
+            Ok(n) if n as u64 > MAX_MANIFEST => {
+                notices.push(format!("extension {name}: manifest over 64 KiB"));
+            }
+            Ok(_) => match parse(&name, &ext_dir, &text) {
+                Ok(ext) => candidates.push(ext),
+                Err(why) => notices.push(format!("extension {name}: {why}")),
+            },
+            Err(e) => notices.push(format!("extension {name}: {e}")),
+        }
+    }
+    admit(candidates, notices)
+}
+
+/// `~/.config/claudio/extensions` (honours `$XDG_CONFIG_HOME`).
+pub fn dir() -> PathBuf {
+    crate::paths::config_dir().join("extensions")
+}
+
 #[cfg(test)]
 mod tests {
     use super::*;
diff --git a/src/tui/extensions/mod.rs b/src/tui/extensions/mod.rs
index aa6b04b..9544839 100644
--- a/src/tui/extensions/mod.rs
+++ b/src/tui/extensions/mod.rs
@@ -1,3 +1,228 @@
+//! The extension registry: what claudio renders (lenses) and what the
+//! leader key (Alt+z) offers (actions), from built-in extensions and from
+//! external ones (`~/.config/claudio/extensions/<name>/`, see
+//! [`manifest`]).
+//!
+//! Pure: the manifests are read by `tui/mod.rs` at start, external programs
+//! run in the extension host it drives, and built-in renderers on the
+//! render worker. Everything else in the TUI asks the registry: which tag
+//! lenses the pane and the reader render, which fence lenses the reader
+//! draws, the context a spawn carries ([`Registry::session_extensions`]) and
+//! the leader's actions.
+
+pub mod manifest;
+
+use std::collections::BTreeMap;
+
+use crate::proto::{LensKind, LensSpec, SessionExtensions, MAX_INSTRUCTIONS, MAX_LENSES};
+
+/// The diagram extension's instruction (spec §7).
+pub const DIAGRAM_CONTEXT: &str = "This terminal draws Mermaid diagrams. When a diagram \
+helps (architecture, data or control flow, state machines, message sequences), write it as \
+a fenced code block with the language `mermaid`, using `flowchart TD`/`flowchart LR` or \
+`sequenceDiagram`. Keep labels short (under 30 characters) and diagrams small (under 20 \
+nodes). Do not draw box diagrams with characters yourself.";
+
+/// One lens of an extension.
+#[derive(Debug, Clone, PartialEq)]
+pub struct Lens {
+    /// The tag, or the fence's info word.
+    pub name: String,
+    pub kind: LensKind,
+    /// What claude is told about it (may be empty).
+    pub instruction: String,
+}
+
+/// One action under the leader key.
+#[derive(Debug, Clone, PartialEq)]
+pub struct ActionDecl {
+    pub key: char,
+    pub title: String,
+    /// The name passed back to the extension.
+    pub id: String,
+    /// Ask for a line of input first, with this prompt.
+    pub input: Option<String>,
+}
+
+/// Where an extension's code is.
+#[derive(Debug, Clone, PartialEq)]
+pub enum Source {
+    /// Rust code in claudio.
+    BuiltIn,
+    /// A program the extension host runs.
+    External(manifest::Program),
+}
+
+#[derive(Debug, Clone, PartialEq)]
+pub struct Extension {
+    pub name: String,
+    pub description: String,
+    pub enabled: bool,
+    pub lenses: Vec<Lens>,
+    pub actions: Vec<ActionDecl>,
+    pub source: Source,
+}
+
+impl Extension {
+    pub fn is_external(&self) -> bool {
+        matches!(self.source, Source::External(_))
+    }
+}
+
+/// The built-in extensions, in registry order, all enabled.
+pub fn builtins() -> Vec<Extension> {
+    let builtin = |name: &str, description: &str| Extension {
+        name: name.to_owned(),
+        description: description.to_owned(),
+        enabled: true,
+        lenses: Vec::new(),
+        actions: Vec::new(),
+        source: Source::BuiltIn,
+    };
+    let action = |key, title: &str, id: &str, input: Option<&str>| ActionDecl {
+        key,
+        title: title.to_owned(),
+        id: id.to_owned(),
+        input: input.map(str::to_owned),
+    };
+    let lens = |name: &str, kind, instruction: &str| Lens {
+        name: name.to_owned(),
+        kind,
+        instruction: instruction.to_owned(),
+    };
+    let mut latex = builtin("latex", "math typeset from <latex> tags");
+    latex.lenses = vec![lens(
+        "latex",
+        LensKind::Tag,
+        crate::claude::hooks::LATEX_CONTEXT,
+    )];
+    let mut diagram = builtin("diagram", "Mermaid diagrams drawn in the reader");
+    diagram.lenses = vec![lens("mermaid", LensKind::Fence, DIAGRAM_CONTEXT)];
+    let mut search = builtin("search", "search claude conversations");
+    search.actions = vec![action(
+        's',
+        "search conversations",
+        "search",
+        Some("search conversations"),
+    )];
+    let mut snippets = builtin("snippets", "saved prompts");
+    snippets.actions = vec![
+        action('p', "insert snippet", "insert", None),
+        action('P', "save last prompt as snippet", "save", None),
+    ];
+    vec![latex, diagram, search, snippets]
+}
+
+/// The reader's leader key: always offered, not an extension's.
+pub const READER_KEY: char = 'u';
+
+/// The extensions this client runs with.
+#[derive(Debug, Clone, PartialEq)]
+pub struct Registry {
+    pub extensions: Vec<Extension>,
+}
+
+impl Default for Registry {
+    /// The built-ins, all on.
+    fn default() -> Self {
+        Registry {
+            extensions: builtins(),
+        }
+    }
+}
+
+impl Registry {
+    /// The built-ins, then `external` (already validated against each
+    /// other and the built-ins by [`manifest::admit`]), with `[extensions]`
+    /// (`enabled`) applied; `[claude] latex = false` (`latex`) turns latex
+    /// off too. A name in `enabled` that matches nothing gets a notice.
+    pub fn new(
+        enabled: &BTreeMap<String, bool>,
+        latex: bool,
+        external: Vec<Extension>,
+        notices: &mut Vec<String>,
+    ) -> Registry {
+        let mut extensions = builtins();
+        extensions.extend(external);
+        for ext in &mut extensions {
+            if let Some(on) = enabled.get(&ext.name) {
+                ext.enabled = *on;
+            }
+            if ext.name == "latex" && !latex {
+                ext.enabled = false;
+            }
+        }
+        for name in enabled.keys() {
+            if !extensions.iter().any(|e| &e.name == name) {
+                notices.push(format!(
+                    "config.toml [extensions]: no extension named '{name}'"
+                ));
+            }
+        }
+        Registry { extensions }
+    }
+
+    /// The enabled extensions, in order.
+    pub fn enabled(&self) -> impl Iterator<Item = &Extension> {
+        self.extensions.iter().filter(|e| e.enabled)
+    }
+
+    /// The enabled lenses of `kind`, with their extension, in order.
+    pub fn lenses(&self, kind: LensKind) -> Vec<(&Extension, &Lens)> {
+        self.enabled()
+            .flat_map(|e| e.lenses.iter().map(move |l| (e, l)))
+            .filter(|(_, l)| l.kind == kind)
+            .collect()
+    }
+
+    /// The names of the enabled tag lenses.
+    pub fn tag_lenses(&self) -> Vec<&str> {
+        let lenses = self.lenses(LensKind::Tag);
+        lenses.into_iter().map(|(_, l)| l.name.as_str()).collect()
+    }
+
+    /// The names of the enabled fence lenses.
+    pub fn fence_lenses(&self) -> Vec<&str> {
+        let lenses = self.lenses(LensKind::Fence);
+        lenses.into_iter().map(|(_, l)| l.name.as_str()).collect()
+    }
+
+    /// The external extension that owns enabled lens `name`, if one does.
+    pub fn external_lens(&self, name: &str) -> Option<&Extension> {
+        self.enabled()
+            .find(|e| e.lenses.iter().any(|l| l.name == name))
+            .filter(|e| e.is_external())
+    }
+
+    /// The session context a claude spawn carries: the enabled lenses in
+    /// registry order, within the protocol's caps (a lens that would break
+    /// them is left out, so the daemon never refuses the spawn).
+    pub fn session_extensions(&self) -> SessionExtensions {
+        let mut lenses = Vec::new();
+        let mut total = 0;
+        for lens in self.enabled().flat_map(|e| e.lenses.iter()) {
+            let size = crate::claude::latex::sanitize(&lens.instruction).len();
+            if lenses.len() == MAX_LENSES || total + size > MAX_INSTRUCTIONS {
+                continue;
+            }
+            total += size;
+            lenses.push(LensSpec {
+                name: lens.name.clone(),
+                kind: lens.kind,
+                instruction: lens.instruction.clone(),
+            });
+        }
+        SessionExtensions { lenses }
+    }
+
+    /// The leader's actions: each enabled extension's, in order.
+    pub fn actions(&self) -> Vec<(&Extension, &ActionDecl)> {
+        self.enabled()
+            .flat_map(|e| e.actions.iter().map(move |a| (e, a)))
+            .collect()
+    }
+}
+
 #[cfg(test)]
 mod tests {
     use super::*;
diff --git a/src/tui/interaction.rs b/src/tui/interaction.rs
index 7e6f6ad..078539a 100644
--- a/src/tui/interaction.rs
+++ b/src/tui/interaction.rs
@@ -13,7 +13,7 @@ use std::io;
 use std::time::Instant;
 
 use crate::client::Incoming;
-use crate::proto::{Msg, SessionEvent, SessionId, SessionKind, SessionState};
+use crate::proto::{LensItem, Msg, SessionEvent, SessionId, SessionKind, SessionState};
 use crate::term::keys::{encode_focus, encode_key, encode_mouse, encode_paste};
 use crate::term::screen::{MouseMode, Screen};
 
@@ -820,27 +820,59 @@ impl App {
                     v.turns = turns;
                 }
             }
-            // A replay and a broadcast may cross: only a newer list counts.
-            // It is rendered off the UI thread; the old one stays meanwhile.
+            // A daemon without the `lens` feature sends formulas only: they
+            // are its `latex` items. A lens daemon's are in its `Lens` events.
             SessionEvent::Latex { seq, formulas } => {
-                if seq > v.latex_seq {
-                    v.latex_seq = seq;
-                    v.latex_srcs = formulas;
-                    let render = Effect::RenderLatex {
-                        host: v.host.clone(),
-                        id: v.id,
-                        seq,
-                        srcs: v.latex_srcs.clone(),
-                    };
-                    self.emit(render);
+                if !self.lens_hosts.contains(host) {
+                    let items = formulas
+                        .into_iter()
+                        .map(|text| LensItem {
+                            tag: "latex".to_owned(),
+                            text,
+                        })
+                        .collect();
+                    self.take_lens_items(i, seq, items);
+                }
+            }
+            SessionEvent::Lens { seq, items } => {
+                if self.lens_hosts.contains(host) {
+                    self.take_lens_items(i, seq, items);
                 }
             }
-            // Taken from Task 3 on (`Welcome.features`).
-            SessionEvent::Lens { .. } => {}
             SessionEvent::Unknown => {}
         }
     }
 
+    /// Take lens list `seq` of session `i`, unless a newer one is held (a
+    /// replay and a broadcast may cross): the items of the enabled tag
+    /// lenses. Its formulas are rendered off the UI thread; the old ones
+    /// stay meanwhile.
+    fn take_lens_items(&mut self, i: usize, seq: u64, items: Vec<LensItem>) {
+        let tags = self.extensions.tag_lenses();
+        let items: Vec<LensItem> = items
+            .into_iter()
+            .filter(|item| tags.contains(&item.tag.as_str()))
+            .collect();
+        let v = &mut self.sessions[i];
+        if seq <= v.latex_seq {
+            return;
+        }
+        v.latex_seq = seq;
+        v.latex_srcs = items
+            .iter()
+            .filter(|item| item.tag == "latex")
+            .map(|item| item.text.clone())
+            .collect();
+        v.lens_items = items;
+        let render = Effect::RenderLatex {
+            host: v.host.clone(),
+            id: v.id,
+            seq,
+            srcs: v.latex_srcs.clone(),
+        };
+        self.emit(render);
+    }
+
     /// The render worker is done with list `seq` of session `id` on `host`:
     /// shown, unless a newer list came since (it is on its way).
     pub fn on_latex_rendered(
diff --git a/src/tui/mod.rs b/src/tui/mod.rs
index 0a5317e..eda1dcf 100644
--- a/src/tui/mod.rs
+++ b/src/tui/mod.rs
@@ -10,6 +10,7 @@ mod confirm;
 mod connections;
 #[allow(dead_code)] // Wired into the reader by Task 11, which removes this.
 mod diagram;
+mod extensions;
 mod fmt;
 mod formulas;
 mod git_app;
@@ -261,7 +262,16 @@ async fn main(launch: Launch) -> ExitCode {
         }
     };
     let (proxy_profiles, proxy_default) = crate::proxy::resolve::load_proxy_profiles();
-    let app = App::new(AppConfig {
+    // External extensions are read once, at start.
+    let external = extensions::manifest::load(&extensions::manifest::dir(), &mut key_notices);
+    let registry = extensions::Registry::new(
+        &config.extensions,
+        config.claude.latex,
+        external,
+        &mut key_notices,
+    );
+    let local_features = local_client.welcome().features.clone();
+    let mut app = App::new(AppConfig {
         mode: if plain.is_some() { Mode::Plain } else { Mode::Manager },
         size,
         home: local_client.welcome().host.home.clone(),
@@ -282,6 +292,8 @@ async fn main(launch: Launch) -> ExitCode {
             None => crate::remote::hosts::candidates(),
         },
     });
+    app.extensions = registry;
+    app.set_host_features("local", &local_features);
     let start = match plain {
         Some(start) => Start::Plain(start),
         None => Start::Manager { saved, live },
@@ -879,6 +891,7 @@ async fn event_loop(
                     }
                     let home = client.welcome().host.home.clone();
                     let remote_claude = client_claude(&client);
+                    app.set_host_features(&host, &client.welcome().features);
                     conns.reset_delay(&host);
                     // Start the reader tagged with the current generation.
                     let cur_gen = conns.current_generation(&host);
@@ -921,6 +934,7 @@ async fn event_loop(
                         spawn_reader("local".to_owned(), cur_gen, rx, ev_tx.clone());
                     }
                     app.set_local_claude(client_claude(&client));
+                    app.set_host_features("local", &client.welcome().features);
                     conns.connected("local", client);
                     app.on_reconnected_local(&sessions, home);
                 }
diff --git a/src/tui/sessions.rs b/src/tui/sessions.rs
index 116cf51..892ab72 100644
--- a/src/tui/sessions.rs
+++ b/src/tui/sessions.rs
@@ -7,8 +7,8 @@
 use uuid::Uuid;
 
 use crate::proto::{
-    GitStatus, Msg, RespawnSpec, SessionId, SessionInfo, SessionKind, SessionState, ShellSpec,
-    SpawnSpec,
+    GitStatus, LensItem, Msg, RespawnSpec, SessionId, SessionInfo, SessionKind, SessionState,
+    ShellSpec, SpawnSpec,
 };
 use crate::term::screen::Screen;
 
@@ -68,13 +68,20 @@ pub struct SessionView {
     pub turns: Option<u32>,
     /// Claude, or a plain terminal tab.
     pub kind: SessionKind,
-    /// The `seq` of the newest formula list taken (0: none yet), and its
-    /// formulas' sources, oldest first.
+    /// The `seq` of the newest lens list taken (0: none yet; a `Latex`
+    /// list from a daemon without the `lens` feature), and its formulas'
+    /// sources, oldest first.
     pub latex_seq: u64,
     pub latex_srcs: Vec<String>,
     /// Those formulas rendered; until the render worker is done with a new
     /// list, the previous one.
     pub latex: Vec<Formula>,
+    /// That list's items of every enabled tag lens (`latex` included),
+    /// oldest first.
+    pub lens_items: Vec<LensItem>,
+    /// The external tag lenses' items rendered, by tag (the extension host
+    /// fills it); until a list's renderings come, the previous ones.
+    pub lens: Vec<(String, Formula)>,
 }
 
 impl SessionView {
@@ -114,6 +121,8 @@ impl SessionView {
             latex_seq: 0,
             latex_srcs: Vec::new(),
             latex: Vec::new(),
+            lens_items: Vec::new(),
+            lens: Vec::new(),
         }
     }
 
@@ -122,6 +131,8 @@ impl SessionView {
         self.latex_seq = 0;
         self.latex_srcs.clear();
         self.latex.clear();
+        self.lens_items.clear();
+        self.lens.clear();
     }
 
     /// The view of a session `host`'s daemon reports.
@@ -423,7 +434,7 @@ impl App {
             cols,
             // `--plain`'s session is not one for a manager to show.
             ephemeral: self.mode == Mode::Plain,
-            extensions: None,
+            extensions: Some(self.extensions.session_extensions()),
         };
         self.send_with_proxy(
             host,
@@ -546,7 +557,7 @@ impl App {
             env: vec![], // filled by `send_with_proxy`
             rows,
             cols,
-            extensions: None,
+            extensions: Some(self.extensions.session_extensions()),
         });
         match self.send_with_proxy(
             &host,
`````
Run: `umask 022; cargo test --locked --bin claudio`
Expected: `test result: ok. 950 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` (3 dead-code warnings until Tasks 5–6)
Also: `rustfmt --edition 2021 --check src/tui/extensions/mod.rs src/config.rs` prints nothing.

Step 3: commit
`/usr/bin/git add src/tui/extensions/mod.rs src/tui/extensions/manifest.rs && /usr/bin/git commit -m "feat(tui): extension registry, manifests and lens events" -- src/tui/extensions/mod.rs src/tui/extensions/manifest.rs src/config.rs src/tui/app.rs src/tui/interaction.rs src/tui/sessions.rs src/tui/mod.rs`

Interfaces this task provides to later tasks: `tui::extensions::{Extension, Lens, ActionDecl, Source, Registry, builtins, READER_KEY, DIAGRAM_CONTEXT}`, `tui::extensions::manifest::{Program, parse, admit, load, dir, HTML_ELEMENTS, MAX_EXTERNAL}`, `config::ExtensionsSection`, `App.extensions`, `App::set_host_features`, `SessionView.{lens_items, lens}`.

### Task 4: rendering lenses in the pane and the reader
Model: sonnet   Repo: <repo>   Files: src/tui/latex.rs, src/tui/mod.rs, src/tui/reader/doc.rs, src/tui/reader/layout.rs, src/tui/ui/mod.rs
Goal: every enabled tag lens is overlaid in the pane and placed in the reader, and fence lenses become reader blocks, with latex unchanged (AC1, AC5's and AC8's drawing paths).

What this task does:
- `latex::overlay_tag(buf, area, screen, tag, items)`; `overlay` = `overlay_tag(…, "latex", …)`; the tag strings are a `Tags` value instead of the `<latex>` constants. Latex's behaviour is byte-identical (all existing overlay tests pass unchanged). `ui::draw_pane` overlays each enabled external tag lens with `view.lens`.
- `reader::doc::prepare_lenses(text, tags, fences)` (`prepare` = latex only, now test-only), `Prepared.lenses` (the lens of each tag) and `.fences`; a fenced block whose info string's first word is a fence lens becomes `Block::Lens { lens, lines, rows: None }`; `Doc::each_lens` walks them (lists and quotes too).
- `reader::layout`: `Block::Lens` with rows that fit → the rows (sanitized, `safe`, two columns in); rows too wide → the code block and one dim line `diagram is N columns wide` (`<lens> is …` for another lens); no rows → the code block.
- Worker: `RenderCache` keyed by `(lens, source)` (`render(lens, src)`; `formula(src)` = `render("latex", src)`); `render_lens` (latex → `render_formula`; any other lens raw — Task 11 adds `mermaid`); `AnswersJob` carries the registry's tag and fence lenses; `render_lens_answers` (the old `render_answers` stays for tests).

Decision (asked by the brief): the `mermaid` renderer is wired in Task 11, not here, so Tasks 4–7 never depend on part B. Until Task 11 a ```` ```mermaid ```` block shows as its code block (`rows: None`).

Step 1: write the failing tests
`````diff
diff --git a/src/tui/latex.rs b/src/tui/latex.rs
index ed8b2a7..ce8c141 100644
--- a/src/tui/latex.rs
+++ b/src/tui/latex.rs
@@ -1008,4 +1030,21 @@ mod tests {
         // A wide cluster takes its cell and the one it covers.
         assert_eq!(text_cells("漢", style).unwrap().len(), 2);
     }
+
+    #[test]
+    fn any_tag_lens_is_laid_over_its_own_tags_only() {
+        let screen =
+            screen("Press <keys>Ctrl+C</keys> or <latex>Ctrl+C</latex>.\n<keys>a\n   b</keys> end");
+        let (rows, cols) = screen.size();
+        let area = Rect::new(0, 0, cols, rows);
+        let mut buf = Buffer::empty(area);
+        screen.render(area, &mut buf);
+        let items = [formula("Ctrl+C", "⌃C"), formula("a b", "[a b]")];
+        overlay_tag(&mut buf, area, &screen, "keys", &items);
+        let shown = rows_of(&buf);
+        assert_eq!(shown[0], "Press ⌃C or <latex>Ctrl+C</latex>.");
+        assert_eq!(shown[1], "[a b] end");
+        assert_eq!(shown[2], "");
+        assert_eq!(buf[(6, 0)].fg, ACCENT);
+    }
 }
diff --git a/src/tui/mod.rs b/src/tui/mod.rs
index eda1dcf..4c718e1 100644
--- a/src/tui/mod.rs
+++ b/src/tui/mod.rs
@@ -1581,6 +1634,8 @@ mod tests {
             gen: 7,
             id: uuid::Uuid::new_v4(),
             answers,
+            tags: vec!["latex".into()],
+            fences: vec![],
         };
         jobs.send(RenderJob::Answers(job)).unwrap();
         let (gen, rendered) = rx.recv_timeout(Duration::from_secs(30)).unwrap();
@@ -1635,7 +1690,62 @@ mod tests {
         cache.formula("0", &running); // used again: "1" is now the oldest
         cache.formula("new", &running);
         assert_eq!(cache.entries.len(), CACHE_SIZE);
-        assert!(cache.entries.contains_key("0") && cache.entries.contains_key("new"));
-        assert!(!cache.entries.contains_key("1"));
+        let has = |src: &str| cache.entries.contains_key(&("latex".to_owned(), src.to_owned()));
+        assert!(has("0") && has("new"));
+        assert!(!has("1"));
+    }
+
+    #[test]
+    fn answers_render_every_tag_lens_and_fence_lens_by_its_renderer() {
+        use crate::proto::Answer;
+        let (tx, rx) = std::sync::mpsc::channel();
+        let jobs = spawn_latex_worker(move |done| {
+            if let Rendered::Answers(_, answers) = done {
+                let _ = tx.send(answers);
+            }
+        })
+        .unwrap();
+        let text = "Press <keys>Ctrl+C</keys> for <latex>x^2</latex>.\n\n\
+                    ```mermaid\nflowchart LR\n  a --> b\n```\n\n```rust\nfn main() {}\n```\n";
+        let job = AnswersJob {
+            gen: 1,
+            id: uuid::Uuid::new_v4(),
+            answers: vec![Answer {
+                prompt: "p".into(),
+                text: text.into(),
+            }],
+            tags: vec!["latex".into(), "keys".into()],
+            fences: vec!["mermaid".into()],
+        };
+        jobs.send(RenderJob::Answers(job)).unwrap();
+        let answers = rx.recv_timeout(Duration::from_secs(10)).unwrap();
+        let answer = &answers[0];
+        // The external lens's item stays raw; latex is rendered.
+        assert_eq!(answer.formulas[0], latex::Formula::raw("Ctrl+C"));
+        assert_eq!(answer.formulas[1].inline.as_deref(), Some("x²"));
+        // The fence lens is a lens block; another fence stays code.
+        let lens: Vec<&reader::doc::Block> = answer
+            .doc
+            .blocks
+            .iter()
+            .filter(|b| matches!(b, reader::doc::Block::Lens { .. }))
+            .collect();
+        assert_eq!(lens.len(), 1);
+        assert!(answer
+            .doc
+            .blocks
+            .iter()
+            .any(|b| matches!(b, reader::doc::Block::Code(lines) if lines == &["fn main() {}"])));
+    }
+
+    #[test]
+    fn the_cache_keeps_lenses_apart() {
+        let running = Arc::new(AtomicUsize::new(0));
+        let mut cache = RenderCache::default();
+        let latex = cache.render("latex", "x^2", &running);
+        let keys = cache.render("keys", "x^2", &running);
+        assert_eq!(latex.inline.as_deref(), Some("x²"));
+        assert_eq!(keys, latex::Formula::raw("x^2"));
+        assert_eq!(cache.entries.len(), 2);
     }
 }
diff --git a/src/tui/reader/doc.rs b/src/tui/reader/doc.rs
index 3f2a371..a48e699 100644
--- a/src/tui/reader/doc.rs
+++ b/src/tui/reader/doc.rs
@@ -598,7 +668,9 @@ mod tests {
         for b in blocks {
             match b {
                 Block::Para(s) | Block::Heading(_, s) => spans(s),
-                Block::Code(lines) => lines.iter().for_each(|l| assert_eq!(*l, sanitize(l))),
+                Block::Code(lines) | Block::Lens { lines, .. } => {
+                    lines.iter().for_each(|l| assert_eq!(*l, sanitize(l)))
+                }
                 Block::List(_, items) => items.iter().for_each(|i| check(i, formulas, depth + 1)),
                 Block::Quote(inner) => check(inner, formulas, depth + 1),
                 Block::Table(rows) => rows.iter().flatten().for_each(|c| spans(c)),
@@ -607,4 +679,52 @@ mod tests {
             }
         }
     }
+
+    #[test]
+    fn every_tag_lens_gets_placeholders_and_fence_lenses_become_lens_blocks() {
+        let text = "Press <keys>Ctrl+C</keys> for <latex>x</latex> <kbd>k</kbd>.\n\n\
+                    ```mermaid extra\nflowchart LR\n\ta --> b\n```\n\n\
+                    - ```mermaid\n  graph TD\n  ```\n\n```text\nplain\n```\n";
+        let p = prepare_lenses(text, &["latex", "keys"], &["mermaid"]);
+        assert_eq!(p.lenses, ["keys", "latex"]);
+        assert_eq!(p.tags[0].src, "Ctrl+C");
+        assert_eq!(p.tags[0].raw, "<keys>Ctrl+C</keys>");
+        let mut d = parse(&p);
+        assert_eq!(
+            d.blocks[0],
+            Block::Para(vec![
+                plain("Press "),
+                Span::Formula(0),
+                plain(" for "),
+                Span::Formula(1),
+                plain(" <kbd>k</kbd>."),
+            ])
+        );
+        assert_eq!(
+            d.blocks[1],
+            Block::Lens {
+                lens: "mermaid".into(),
+                lines: vec!["flowchart LR".into(), "    a --> b".into()],
+                rows: None,
+            }
+        );
+        assert_eq!(d.blocks[3], Block::Code(vec!["plain".into()]));
+        // Every lens block, nested ones too, in order.
+        let mut seen = Vec::new();
+        d.each_lens(&mut |lens, src, rows| {
+            seen.push((lens.to_owned(), src));
+            *rows = Some(vec!["drawn".into()]);
+        });
+        assert_eq!(
+            seen,
+            [
+                ("mermaid".to_owned(), "flowchart LR\n    a --> b".to_owned()),
+                ("mermaid".to_owned(), "graph TD".to_owned()),
+            ]
+        );
+        assert!(matches!(&d.blocks[1], Block::Lens { rows: Some(r), .. } if r == &["drawn"]));
+        // Without fence lenses, fences are code.
+        let d = parse(&prepare_lenses(text, &["latex"], &[]));
+        assert!(matches!(&d.blocks[1], Block::Code(_)));
+    }
 }
diff --git a/src/tui/reader/layout.rs b/src/tui/reader/layout.rs
index 17f6439..d872d37 100644
--- a/src/tui/reader/layout.rs
+++ b/src/tui/reader/layout.rs
@@ -739,4 +775,38 @@ mod tests {
         let lines = shown(&format!("a {wide} b"), &[], 20);
         assert_eq!(lines, ["a \u{fffd} b"]);
     }
+
+    #[test]
+    fn lens_blocks_show_their_rows_or_their_code_with_why() {
+        let lines = vec!["graph TD".to_owned(), "a --> b".to_owned()];
+        let block = |rows: Option<Vec<&str>>| Doc {
+            blocks: vec![Block::Lens {
+                lens: "mermaid".into(),
+                lines: lines.clone(),
+                rows: rows.map(|r| r.iter().map(|s| s.to_string()).collect()),
+            }],
+        };
+        let text = |doc: &Doc, width| -> Vec<String> {
+            layout(doc, &[], width).iter().map(|l| l.text()).collect()
+        };
+        let drawn = block(Some(vec!["┌───┐", "│ a │\u{1b}", "└───┘"]));
+        assert_eq!(text(&drawn, 20), ["  ┌───┐", "  │ a │", "  └───┘"]);
+        // Too wide for the reader: the code, and how wide it is.
+        assert_eq!(
+            text(&drawn, 6),
+            ["  grap", "  a --", "  diag"],
+            "cut to the width"
+        );
+        assert_eq!(text(&drawn, 7)[2], "  └───┘", "two columns in, it fits");
+        let wide = layout(&drawn, &[], 6);
+        assert!(wide[0].0.iter().all(|s| s.style == dim()));
+        assert_eq!(text(&drawn, 40).len(), 3);
+        let narrow = block(Some(vec!["0123456789"]));
+        assert_eq!(
+            text(&narrow, 11),
+            ["  graph TD", "  a --> b", "  diagram i"]
+        );
+        // No rendering: the code block.
+        assert_eq!(text(&block(None), 20), ["  graph TD", "  a --> b"]);
+    }
 }
`````
Run: `umask 022; cargo test --locked --bin claudio 2>&1 | grep -E '^error' | head -3`
Expected: `` error[E0425]: cannot find function `overlay_tag` in this scope ``

Step 2: implement
`````diff
diff --git a/src/tui/latex.rs b/src/tui/latex.rs
index cc0935a..ce8c141 100644
--- a/src/tui/latex.rs
+++ b/src/tui/latex.rs
@@ -304,8 +304,20 @@ const ACCENT: Color = Color::Cyan;
 /// The most screen rows one tag may span: claude wraps long lines itself,
 /// indenting the rows that continue one.
 const MAX_SPAN_ROWS: u16 = 8;
-const OPEN_TAG: &str = "<latex>";
-const CLOSE_TAG: &str = "</latex>";
+/// The tags of a tag lens, as written on screen.
+struct Tags {
+    open: String,
+    close: String,
+}
+
+impl Tags {
+    fn of(tag: &str) -> Tags {
+        Tags {
+            open: format!("<{tag}>"),
+            close: format!("</{tag}>"),
+        }
+    }
+}
 /// The vertical border of Claude Code's tables: a tag followed by one on
 /// its row is a table cell.
 const TABLE_BORDER: &str = "│";
@@ -318,21 +330,28 @@ const TABLE_BORDER: &str = "│";
 /// moves; a row (or tag) whose result would not fit stays as it was. The
 /// selection, copy and the cursor still go by `screen`.
 pub fn overlay(buf: &mut Buffer, area: Rect, screen: &Screen, formulas: &[Formula]) {
+    overlay_tag(buf, area, screen, "latex", formulas);
+}
+
+/// [`overlay`] for the tags of tag lens `tag` (`<tag>…</tag>`), matched
+/// with `items` (the lens's renderings) by the same keys.
+pub fn overlay_tag(buf: &mut Buffer, area: Rect, screen: &Screen, tag: &str, items: &[Formula]) {
+    let tags = Tags::of(tag);
     let rows = area.height.min(screen.size().0);
     let mut row = 0;
     while row < rows {
-        row = if shows_open_tag(buf, area, row) {
-            overlay_row(buf, area, screen, formulas, row, rows)
+        row = if shows_open_tag(buf, area, row, &tags) {
+            overlay_row(buf, area, screen, items, row, rows, &tags)
         } else {
             row + 1
         };
     }
 }
 
-/// Whether row `row` of the drawn pane shows `<latex>`: a look at its
-/// cells, so a row without one costs nothing more.
-fn shows_open_tag(buf: &Buffer, area: Rect, row: u16) -> bool {
-    let tag = OPEN_TAG.as_bytes();
+/// Whether row `row` of the drawn pane shows the opening tag: a look at
+/// its cells, so a row without one costs nothing more.
+fn shows_open_tag(buf: &Buffer, area: Rect, row: u16, tags: &Tags) -> bool {
+    let tag = tags.open.as_bytes();
     let mut matched = 0;
     for x in area.left()..area.right() {
         let Some(cell) = buf.cell((x, area.y + row)) else {
@@ -359,15 +378,16 @@ fn overlay_row(
     formulas: &[Formula],
     row: u16,
     rows: u16,
+    tags: &Tags,
 ) -> u16 {
     let line = Row::read(screen, row, area.width);
-    let tags = line.tags();
-    match tags.as_slice() {
+    let found = line.tags(tags);
+    match found.as_slice() {
         [Tag { open, close: None }] => {
-            overlay_span(buf, area, screen, formulas, (row, rows), &line, *open)
+            overlay_span(buf, area, screen, formulas, (row, rows), &line, *open, tags)
         }
         _ => {
-            rebuild_row(buf, area, row, &line, &tags, formulas);
+            rebuild_row(buf, area, row, &line, &found, formulas, tags);
             row + 1
         }
     }
@@ -384,16 +404,17 @@ fn rebuild_row(
     area: Rect,
     row: u16,
     line: &Row,
-    tags: &[Tag],
+    found: &[Tag],
     formulas: &[Formula],
+    tags: &Tags,
 ) {
     let original = cells_of(buf, area, row);
     let mut out = Vec::with_capacity(original.len());
     let mut copied = 0;
-    for tag in tags {
+    for tag in found {
         let Some(close) = tag.close else { continue };
-        let (start, end) = (line.col(tag.open), line.col(close + CLOSE_TAG.len()));
-        let text = line.text(tag.open + OPEN_TAG.len(), close);
+        let (start, end) = (line.col(tag.open), line.col(close + tags.close.len()));
+        let text = line.text(tag.open + tags.open.len(), close);
         let Some(mut cells) = matching(formulas, &text)
             .and_then(|inline| text_cells(inline, accent_on(&original[start])))
         else {
@@ -436,17 +457,18 @@ fn overlay_span(
     (row, rows): (u16, u16),
     first: &Row,
     open: usize,
+    tags: &Tags,
 ) -> u16 {
-    let mut text = first.text(open + OPEN_TAG.len(), first.glyphs.len());
+    let mut text = first.text(open + tags.open.len(), first.glyphs.len());
     let mut closing = None;
     for last in (row + 1..rows).take(MAX_SPAN_ROWS as usize) {
         let line = Row::read(screen, last, area.width);
         // Another tag in the way: this one is not laid over.
-        if line.find(OPEN_TAG, 0).is_some() {
+        if line.find(&tags.open, 0).is_some() {
             break;
         }
         let indent = line.indent();
-        let close = line.find(CLOSE_TAG, indent);
+        let close = line.find(&tags.close, indent);
         text.push(' ');
         text.push_str(&line.text(indent, close.unwrap_or(line.glyphs.len())));
         if let Some(close) = close {
@@ -471,7 +493,7 @@ fn overlay_span(
         return last + 1;
     }
     let last_cells = cells_of(buf, area, last);
-    let suffix_start = line.col(close + CLOSE_TAG.len());
+    let suffix_start = line.col(close + tags.close.len());
     let suffix = &last_cells[suffix_start..content_end(&last_cells).max(suffix_start)];
     let suffix_fits = head.len() + suffix.len() <= width;
     if suffix_fits {
@@ -565,8 +587,8 @@ fn write_row(buf: &mut Buffer, area: Rect, row: u16, cells: Vec<Cell>, original:
     }
 }
 
-/// A `<latex>` on a row, by glyph index: its `<`, and the `<` of its
-/// `</latex>` when it closes on the same row.
+/// An opening tag on a row, by glyph index: its `<`, and the `<` of its
+/// closing tag when it closes on the same row.
 struct Tag {
     open: usize,
     close: Option<usize>,
@@ -597,20 +619,20 @@ impl Row {
         }
     }
 
-    /// Every `<latex>` that starts on the row, in order; only the last one
-    /// can be unclosed.
-    fn tags(&self) -> Vec<Tag> {
-        let mut tags = Vec::new();
+    /// Every opening tag that starts on the row, in order; only the last
+    /// one can be unclosed.
+    fn tags(&self, tags: &Tags) -> Vec<Tag> {
+        let mut found = Vec::new();
         let mut at = 0;
-        while let Some(open) = self.find(OPEN_TAG, at) {
-            let close = self.find(CLOSE_TAG, open + OPEN_TAG.len());
-            tags.push(Tag { open, close });
+        while let Some(open) = self.find(&tags.open, at) {
+            let close = self.find(&tags.close, open + tags.open.len());
+            found.push(Tag { open, close });
             match close {
-                Some(close) => at = close + CLOSE_TAG.len(),
+                Some(close) => at = close + tags.close.len(),
                 None => break,
             }
         }
-        tags
+        found
     }
 
     /// The glyph index of the next `tag` (ASCII) at or after glyph `from`.
diff --git a/src/tui/mod.rs b/src/tui/mod.rs
index 109995f..4c718e1 100644
--- a/src/tui/mod.rs
+++ b/src/tui/mod.rs
@@ -530,6 +530,9 @@ struct AnswersJob {
     gen: u64,
     id: crate::proto::SessionId,
     answers: Vec<crate::proto::Answer>,
+    /// The enabled tag and fence lenses (the registry's, when sent).
+    tags: Vec<String>,
+    fences: Vec<String>,
 }
 
 /// Work for the `latex-render` thread, one job at a time.
@@ -569,7 +572,12 @@ fn spawn_latex_worker(
                         done(Rendered::Latex(job, formulas));
                     }
                     RenderJob::Answers(job) => {
-                        let answers = render_answers(&job.answers, &mut cache, &running);
+                        let answers = render_lens_answers(
+                            &job.answers,
+                            (&job.tags, &job.fences),
+                            &mut cache,
+                            &running,
+                        );
                         done(Rendered::Answers(job, answers));
                     }
                 }
@@ -578,67 +586,104 @@ fn spawn_latex_worker(
     Ok(jobs)
 }
 
-/// `answers` as the reader shows them: each a markdown document whose
-/// formulas (the first `MAX_FORMULAS`) are rendered through the guard and
-/// its cache, like the pane's.
+/// `answers` as the reader shows them with only the latex lens (see
+/// [`render_lens_answers`]).
+#[cfg(test)]
 fn render_answers(
     answers: &[crate::proto::Answer],
     cache: &mut RenderCache,
     running: &Arc<AtomicUsize>,
 ) -> Vec<reader::RenderedAnswer> {
+    let tags = ["latex".to_owned()];
+    render_lens_answers(answers, (&tags, &[]), cache, running)
+}
+
+/// `answers` as the reader shows them: each a markdown document whose
+/// items of tag lenses `lenses.0` (the first `MAX_FORMULAS`) and blocks of
+/// fence lenses `lenses.1` are rendered through the guard and its cache,
+/// like the pane's. An external lens's are left raw here: the extension
+/// host renders them.
+fn render_lens_answers(
+    answers: &[crate::proto::Answer],
+    lenses: (&[String], &[String]),
+    cache: &mut RenderCache,
+    running: &Arc<AtomicUsize>,
+) -> Vec<reader::RenderedAnswer> {
+    let tags: Vec<&str> = lenses.0.iter().map(String::as_str).collect();
+    let fences: Vec<&str> = lenses.1.iter().map(String::as_str).collect();
     answers
         .iter()
         .map(|answer| {
-            let prepared = reader::doc::prepare(&answer.text);
+            let prepared = reader::doc::prepare_lenses(&answer.text, &tags, &fences);
             let formulas = prepared
                 .tags
                 .iter()
-                .map(|tag| cache.formula(&tag.src, running))
+                .zip(&prepared.lenses)
+                .map(|(tag, lens)| cache.render(lens, &tag.src, running))
                 .collect();
+            let mut doc = reader::doc::parse(&prepared);
+            doc.each_lens(&mut |lens, src, rows| {
+                *rows = cache.render(lens, &src, running).block.map(|b| b.rows);
+            });
             reader::RenderedAnswer {
                 prompt: answer.prompt.clone(),
-                doc: reader::doc::parse(&prepared),
+                doc,
                 formulas,
             }
         })
         .collect()
 }
 
-/// Rendered formulas by source, the [`CACHE_SIZE`] most recently used: each
-/// list resends every formula, and a source is rendered once.
+/// Renderings by lens and source, the [`CACHE_SIZE`] most recently used:
+/// each list resends every item, and a source is rendered once.
 #[derive(Default)]
 struct RenderCache {
-    entries: HashMap<String, (u64, latex::Formula)>,
+    entries: HashMap<(String, String), (u64, latex::Formula)>,
     /// Bumped on every lookup; an entry keeps the value of its last use.
     clock: u64,
 }
 
 impl RenderCache {
+    /// A `<latex>` formula's rendering.
     fn formula(&mut self, src: &str, running: &Arc<AtomicUsize>) -> latex::Formula {
+        self.render("latex", src, running)
+    }
+
+    fn render(&mut self, lens: &str, src: &str, running: &Arc<AtomicUsize>) -> latex::Formula {
         self.clock += 1;
-        if let Some((used, formula)) = self.entries.get_mut(src) {
+        let key = (lens.to_owned(), src.to_owned());
+        if let Some((used, formula)) = self.entries.get_mut(&key) {
             *used = self.clock;
             return formula.clone();
         }
-        let (formula, keep) = render_formula(src, running);
+        let (formula, keep) = render_lens(lens, src, running);
         if keep {
             if self.entries.len() >= CACHE_SIZE {
                 let oldest = self
                     .entries
                     .iter()
                     .min_by_key(|(_, (used, _))| *used)
-                    .map(|(src, _)| src.clone());
+                    .map(|(key, _)| key.clone());
                 if let Some(oldest) = oldest {
                     self.entries.remove(&oldest);
                 }
             }
-            self.entries
-                .insert(src.to_owned(), (self.clock, formula.clone()));
+            self.entries.insert(key, (self.clock, formula.clone()));
         }
         formula
     }
 }
 
+/// Render `src` of lens `lens` with its built-in renderer: a tag lens's
+/// item, or a fence lens's block (its rows as `block`). A lens without one
+/// (an external lens's) stays raw. The flag says whether it may be cached.
+fn render_lens(lens: &str, src: &str, running: &Arc<AtomicUsize>) -> (latex::Formula, bool) {
+    match lens {
+        "latex" => render_formula(src, running),
+        _ => (latex::Formula::raw(src), true),
+    }
+}
+
 /// Render one formula within [`RENDER_BUDGET`]: what the pre-check turns
 /// away, or what times out or panics, is shown raw. The flag says whether
 /// the result may be cached: not when too many renders were still running
@@ -1083,7 +1128,15 @@ fn run_effect(
             }
         }
         Effect::RenderAnswers { gen, id, answers } => {
-            let job = RenderJob::Answers(AnswersJob { gen, id, answers });
+            let tags = app.extensions.tag_lenses().iter().map(|t| t.to_string()).collect();
+            let fences = app.extensions.fence_lenses().iter().map(|f| f.to_string()).collect();
+            let job = RenderJob::Answers(AnswersJob {
+                gen,
+                id,
+                answers,
+                tags,
+                fences,
+            });
             if latex_jobs.is_none_or(|jobs| jobs.send(job).is_err()) {
                 app.on_answers_failed(gen, id, "the render thread is not running");
             }
diff --git a/src/tui/reader/doc.rs b/src/tui/reader/doc.rs
index 0cc7bae..a48e699 100644
--- a/src/tui/reader/doc.rs
+++ b/src/tui/reader/doc.rs
@@ -14,9 +14,9 @@
 //! - Lists and quotes nest at most [`MAX_NEST`] deep; deeper content is
 //!   flattened into the deepest one, so layout recursion is bounded.
 
-use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
+use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
 
-use crate::claude::latex::{is_combining, latex_tags, sanitize, MAX_FORMULAS};
+use crate::claude::latex::{is_combining, lens_tags, sanitize, MAX_FORMULAS};
 
 /// The first placeholder: formula `i` of an answer is `PLACEHOLDER + i`.
 const PLACEHOLDER: u32 = 0xE000;
@@ -31,6 +31,11 @@ const TAB: &str = "    ";
 pub struct Prepared {
     pub text: String,
     pub tags: Vec<FormulaTag>,
+    /// The tag lens of each of `tags` (`latex`, …).
+    pub lenses: Vec<String>,
+    /// The fence lenses: a fenced code block whose info string starts with
+    /// one becomes a [`Block::Lens`].
+    pub fences: Vec<String>,
 }
 
 /// One formula tag of an answer.
@@ -64,6 +69,14 @@ pub enum Block {
     Rule,
     /// A display formula: a paragraph or tight item that is only a formula.
     Display(usize),
+    /// A fenced code block of fence lens `lens`: its lines (as `Code`'s),
+    /// and its rendering's rows once the render worker (or the extension
+    /// host) made one.
+    Lens {
+        lens: String,
+        lines: Vec<String>,
+        rows: Option<Vec<String>>,
+    },
 }
 
 #[derive(Debug, Clone, PartialEq)]
@@ -90,23 +103,37 @@ pub struct Emphasis {
 /// [`MAX_FORMULAS`] formula tags replaced by a placeholder, and a combining
 /// mark right after a tag dropped (it would join the placeholder). Later
 /// tags stay as they are written.
+#[cfg(test)]
 pub fn prepare(text: &str) -> Prepared {
+    prepare_lenses(text, &["latex"], &[])
+}
+
+/// [`prepare`] for the tags of tag lenses `tags` (the first
+/// [`MAX_FORMULAS`] of them all), with fence lenses `fences` for [`parse`].
+pub fn prepare_lenses(text: &str, tags: &[&str], fences: &[&str]) -> Prepared {
     let clean = sanitize(text);
     let mut out = String::with_capacity(clean.len());
-    let mut tags = Vec::new();
+    let mut found = Vec::new();
+    let mut lenses = Vec::new();
     let mut copied = 0;
-    for (range, src) in latex_tags(&clean).into_iter().take(MAX_FORMULAS) {
+    for (range, lens, src) in lens_tags(&clean, tags).into_iter().take(MAX_FORMULAS) {
         out.push_str(&clean[copied..range.start]);
-        out.push(placeholder(tags.len()));
-        tags.push(FormulaTag {
+        out.push(placeholder(found.len()));
+        found.push(FormulaTag {
             src,
             raw: clean[range.clone()].to_owned(),
         });
+        lenses.push(lens);
         let rest = &clean[range.end..];
         copied = range.end + (rest.len() - rest.trim_start_matches(is_combining).len());
     }
     out.push_str(&clean[copied..]);
-    Prepared { text: out, tags }
+    Prepared {
+        text: out,
+        tags: found,
+        lenses,
+        fences: fences.iter().map(|f| f.to_string()).collect(),
+    }
 }
 
 /// The placeholder of formula `i` (`i` < [`MAX_FORMULAS`]).
@@ -127,7 +154,7 @@ fn formula_of(c: char, count: usize) -> Option<usize> {
 pub fn parse(prepared: &Prepared) -> Doc {
     let options =
         Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
-    let mut b = Builder::new(&prepared.tags);
+    let mut b = Builder::new(&prepared.tags, &prepared.fences);
     for event in Parser::new_ext(&prepared.text, options) {
         b.event(event);
     }
@@ -135,6 +162,28 @@ pub fn parse(prepared: &Prepared) -> Doc {
     Doc { blocks: b.root }
 }
 
+impl Doc {
+    /// Call `f` with each fence-lens block's lens, source (its lines) and
+    /// rows, in document order, inside lists and quotes too.
+    pub fn each_lens(&mut self, f: &mut impl FnMut(&str, String, &mut Option<Vec<String>>)) {
+        fn walk(blocks: &mut [Block], f: &mut impl FnMut(&str, String, &mut Option<Vec<String>>)) {
+            for block in blocks {
+                match block {
+                    Block::Lens { lens, lines, rows } => f(lens, lines.join("\n"), rows),
+                    Block::List(_, items) => {
+                        for item in items {
+                            walk(item, f);
+                        }
+                    }
+                    Block::Quote(inner) => walk(inner, f),
+                    _ => {}
+                }
+            }
+        }
+        walk(&mut self.blocks, f);
+    }
+}
+
 /// A container being read.
 enum Frame {
     Quote(Vec<Block>),
@@ -146,6 +195,9 @@ enum Frame {
 /// being gathered.
 struct Builder<'a> {
     tags: &'a [FormulaTag],
+    fences: &'a [String],
+    /// The fence lens of the code block being read, if it is one's.
+    fence: Option<String>,
     root: Vec<Block>,
     frames: Vec<Frame>,
     /// Per open container start: whether it got a frame (`false`: it was
@@ -166,9 +218,11 @@ struct Builder<'a> {
 }
 
 impl<'a> Builder<'a> {
-    fn new(tags: &'a [FormulaTag]) -> Builder<'a> {
+    fn new(tags: &'a [FormulaTag], fences: &'a [String]) -> Builder<'a> {
         Builder {
             tags,
+            fences,
+            fence: None,
             root: Vec::new(),
             frames: Vec::new(),
             open: Vec::new(),
@@ -216,9 +270,17 @@ impl<'a> Builder<'a> {
                 self.flush();
                 self.heading = Some(level as u8);
             }
-            Tag::CodeBlock(_) => {
+            Tag::CodeBlock(kind) => {
                 self.flush();
                 self.code = Some(String::new());
+                self.fence = match kind {
+                    CodeBlockKind::Fenced(info) => info
+                        .split_whitespace()
+                        .next()
+                        .filter(|word| self.fences.iter().any(|f| f == word))
+                        .map(str::to_owned),
+                    CodeBlockKind::Indented => None,
+                };
             }
             Tag::BlockQuote(_) => self.open_frame(Frame::Quote(Vec::new())),
             Tag::List(start) => self.open_frame(Frame::List(start, Vec::new())),
@@ -249,7 +311,15 @@ impl<'a> Builder<'a> {
             TagEnd::CodeBlock => {
                 let code = self.code.take().unwrap_or_default();
                 let code = self.code_text(code.trim_end_matches('\n'), TAB);
-                self.push(Block::Code(code.split('\n').map(str::to_owned).collect()));
+                let lines = code.split('\n').map(str::to_owned).collect();
+                match self.fence.take() {
+                    Some(lens) => self.push(Block::Lens {
+                        lens,
+                        lines,
+                        rows: None,
+                    }),
+                    None => self.push(Block::Code(lines)),
+                }
             }
             TagEnd::BlockQuote(_) | TagEnd::List(_) | TagEnd::Item => self.close_frame(),
             TagEnd::TableCell => {
diff --git a/src/tui/reader/layout.rs b/src/tui/reader/layout.rs
index 8da0a7c..d872d37 100644
--- a/src/tui/reader/layout.rs
+++ b/src/tui/reader/layout.rs
@@ -161,6 +161,43 @@ fn fits_inline(laid: &Laid) -> bool {
     laid.baseline <= INLINE_REACH && laid.rows.len() <= laid.baseline + 1 + INLINE_REACH
 }
 
+/// A code block's lines, indented, dim and cut at `width`.
+fn code(lines: &[String], width: usize, out: &mut Vec<Line>) {
+    for line in lines {
+        out.push(cut(&format!("{INDENT}{}", safe(line)), dim(), width));
+    }
+}
+
+/// A fence lens's block: its rendering's rows, indented, when there is one
+/// that fits `width`; else the code block it was, with one dim line saying
+/// how wide the rendering is when that is why.
+fn lens_block(
+    lens: &str,
+    lines: &[String],
+    rows: Option<&[String]>,
+    width: usize,
+    out: &mut Vec<Line>,
+) {
+    let Some(rows) = rows else {
+        return code(lines, width, out);
+    };
+    let rows: Vec<String> = rows.iter().map(|r| safe(&prepare(r))).collect();
+    let wide = rows.iter().map(|r| r.width()).max().unwrap_or(0);
+    if INDENT.width() + wide <= width {
+        for row in rows {
+            out.push(cut(&format!("{INDENT}{row}"), Style::default(), width));
+        }
+        return;
+    }
+    code(lines, width, out);
+    let what = if lens == "mermaid" { "diagram" } else { lens };
+    out.push(cut(
+        &format!("{INDENT}{what} is {wide} columns wide"),
+        dim(),
+        width,
+    ));
+}
+
 struct Ctx<'a> {
     formulas: &'a [Formula],
 }
@@ -188,10 +225,9 @@ impl Ctx<'_> {
                 };
                 out.extend(self.para(spans, style, width));
             }
-            Block::Code(lines) => {
-                for line in lines {
-                    out.push(cut(&format!("{INDENT}{}", safe(line)), dim(), width));
-                }
+            Block::Code(lines) => code(lines, width, out),
+            Block::Lens { lens, lines, rows } => {
+                lens_block(lens, lines, rows.as_deref(), width, out)
             }
             Block::List(start, items) => self.list(*start, items, width, out),
             Block::Quote(inner) => {
diff --git a/src/tui/ui/mod.rs b/src/tui/ui/mod.rs
index 145ccc8..fd955e0 100644
--- a/src/tui/ui/mod.rs
+++ b/src/tui/ui/mod.rs
@@ -319,6 +319,18 @@ fn draw_pane(frame: &mut Frame, app: &App, area: Rect) {
     if !view.latex.is_empty() {
         super::latex::overlay(frame.buffer_mut(), area, &view.mirror, &view.latex);
     }
+    // The external tag lenses' renderings, each over its own tags.
+    for tag in app.extensions.tag_lenses().into_iter().filter(|t| *t != "latex") {
+        let items: Vec<super::latex::Formula> = view
+            .lens
+            .iter()
+            .filter(|(t, _)| t == tag)
+            .map(|(_, f)| f.clone())
+            .collect();
+        if !items.is_empty() {
+            super::latex::overlay_tag(frame.buffer_mut(), area, &view.mirror, tag, &items);
+        }
+    }
     if let Some(sel) = app.selection.as_ref().filter(|s| s.session == view.id) {
         highlight_selection(frame.buffer_mut(), area, sel);
     }
`````
Run: `umask 022; cargo test --locked --bin claudio`
Expected: `test result: ok. 955 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` (3 dead-code warnings)
Also: `rustfmt --edition 2021 --check src/tui/latex.rs src/tui/reader/doc.rs src/tui/reader/layout.rs` prints nothing.

Step 3: commit
`/usr/bin/git commit -m "feat(tui): render every lens in the pane and the reader" -- src/tui/latex.rs src/tui/mod.rs src/tui/reader/doc.rs src/tui/reader/layout.rs src/tui/ui/mod.rs`

Interfaces this task provides to later tasks: `latex::overlay_tag`, `reader::doc::{prepare_lenses, Prepared.lenses, Prepared.fences, Block::Lens, Doc::each_lens}`, `RenderCache::render(lens, src, running)`, `render_lens(lens, src, running) -> (Formula, bool)` (Task 11 adds the `mermaid` arm), `render_lens_answers(answers, (tags, fences), cache, running)`.

### Task 5: the extension host for external programs
Model: opus   Repo: <repo>   Files: src/tui/extensions/host.rs (new), src/tui/extensions/protocol.rs (new), src/tui/extensions/mod.rs, src/tui/app.rs, src/tui/interaction.rs, src/tui/mod.rs, src/tui/reader/mod.rs
Goal: external lens items are rendered by their programs, asynchronously and bounded, in the pane and the reader; actions can be run (AC5, AC7's host part).

What this task does:
- `extensions/protocol.rs` (pure): request lines (`request`, `render_params`, `action_params`, `SessionContext`), `reply` (id + result/error; not JSON → protocol error; no id → ignored), `rendering` (caps: inline ≤ 512 cols, block ≤ 200 × 100; over a cap → `None`), `action_result` (document ≤ 256 KiB and paste ≤ 64 KiB else `Err("result too large")`, list ≤ 500 items, labels 200, details 400, notices 200, titles 120 chars cut; nested lists dropped; everything `sanitize`d, one-line fields lose `\n`/`\t`). `type Rendering = (Option<String>, Option<Laid>)`.
- `extensions/host.rs` (I/O, driven by `tui/mod.rs`): `ExtensionHost::new(registry, notices)`; per external extension an `Ext` with a lazily started program (cwd = its dir, `process_group(0)`, stdin/stdout piped, stderr null, `kill_on_drop`, env minus `SESSION_MARKERS`, `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN` and `CLAUDIO_*`, plus `CLAUDIO_EXTENSION`, `CLAUDIO_EXTENSION_PROTOCOL=1`), a reader task per start (lines ≤ 1 MiB), `MAX_IN_FLIGHT = 16` (semaphore), `RENDER_TIMEOUT = 1 s`, `ACTION_TIMEOUT = 15 s`; a crash, protocol error or timeout is a failure; the program restarts on the next request; 3 within 5 minutes disable the extension with a notice. `render(lens, kind, source)` (cached by lens and source; a failure for 60 s), `action(ext, action, input, session)`, `shutdown()` (SIGTERM to the group, SIGKILL after 1 s; called when the UI quits).
- `ActionResult` and `ListItem` in `extensions/mod.rs` (used from Task 6).
- `Effect::RenderExternal { host, id, seq, items }` (from `take_lens_items` for the items of external tag lenses) → `HostEvent::ExternalRendered` → `App::on_external_rendered` (kept only for the current `latex_seq`) → `SessionView.lens`.
- Reader: the worker lists an external lens's items and blocks (`reader::External { answer, slot: Slot::{Formula, Fence}, lens, kind, source }`) instead of rendering them; `tui/mod.rs` has the host render them on `AnswersRendered` → `HostEvent::ReaderExternalRendered` → `App::on_reader_external` (by the view's `gen`) → `ReaderView::set_external`.
- `HostEvent::ExtensionNotice` (a disabled extension) → `App::notify`.

Step 1: write the failing tests
`````diff
diff --git a/src/tui/extensions/host.rs b/src/tui/extensions/host.rs
new file mode 100644
index 0000000..1607cf4
--- /dev/null
+++ b/src/tui/extensions/host.rs
@@ -0,0 +1,179 @@
+#[cfg(test)]
+mod tests {
+    use super::*;
+    use crate::tui::extensions::{Extension, Lens};
+
+    /// A scratch dir with an extension program `prog.sh` (`script`).
+    fn program(script: &str) -> (std::path::PathBuf, Program) {
+        let dir = std::env::temp_dir().join(format!("claudio-exthost-{}", uuid::Uuid::new_v4()));
+        std::fs::create_dir_all(&dir).unwrap();
+        let path = dir.join("prog.sh");
+        std::fs::write(&path, format!("#!/bin/sh\n{script}")).unwrap();
+        use std::os::unix::fs::PermissionsExt;
+        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
+        let program = Program {
+            dir: dir.clone(),
+            command: vec!["./prog.sh".into()],
+        };
+        (dir, program)
+    }
+
+    /// A host with one extension `fake` (lens `keys`) running `script`.
+    fn host(
+        script: &str,
+    ) -> (
+        std::path::PathBuf,
+        ExtensionHost,
+        mpsc::UnboundedReceiver<String>,
+    ) {
+        let (dir, program) = program(script);
+        let ext = Extension {
+            name: "fake".into(),
+            description: String::new(),
+            enabled: true,
+            lenses: vec![Lens {
+                name: "keys".into(),
+                kind: LensKind::Tag,
+                instruction: String::new(),
+            }],
+            actions: vec![],
+            source: Source::External(program),
+        };
+        let registry = Registry::new(&Default::default(), true, vec![ext], &mut Vec::new());
+        let (tx, rx) = mpsc::unbounded_channel();
+        (dir, ExtensionHost::new(&registry, tx), rx)
+    }
+
+    /// A shell loop answering each request line with `reply`, where `$id`
+    /// is the request's id (sed-extracted).
+    fn answering(reply: &str) -> String {
+        format!(
+            "while IFS= read -r line; do\n\
+             id=$(printf '%s' \"$line\" | sed 's/^{{\"id\":\\([0-9]*\\).*/\\1/')\n\
+             {reply}\n\
+             done\n"
+        )
+    }
+
+    #[test]
+    fn secrets_markers_and_claudio_variables_are_scrubbed() {
+        for name in [
+            "ANTHROPIC_API_KEY",
+            "ANTHROPIC_AUTH_TOKEN",
+            "CLAUDIO_PROXY_URL",
+            "CLAUDIO_CLAUDE_PATH",
+            "CLAUDECODE",
+            "CLAUDE_CODE_SESSION_ID",
+        ] {
+            assert!(scrubbed(name), "{name}");
+        }
+        for name in ["PATH", "HOME", "LANG", "ANTHROPIC_MODEL"] {
+            assert!(!scrubbed(name), "{name}");
+        }
+    }
+
+    #[tokio::test]
+    async fn renders_and_actions_go_through_the_program() {
+        let script = answering(
+            "case \"$line\" in\n\
+             *render*) printf '{\"id\":%s,\"result\":{\"inline\":\"[%s]\"}}\\n' \"$id\" \"$CLAUDIO_EXTENSION\" ;;\n\
+             *) printf '{\"id\":%s,\"result\":{\"kind\":\"paste\",\"text\":\"%s %s\"}}\\n' \"$id\" \"$(pwd | sed 's|.*/||')\" \"$CLAUDIO_EXTENSION_PROTOCOL\" ;;\n\
+             esac",
+        );
+        let (dir, host, _notices) = host(&script);
+        let r = host.render("keys", LensKind::Tag, "Ctrl+C").await;
+        assert_eq!(r, Some((Some("[fake]".to_owned()), None)));
+        // Not this host's lens.
+        assert_eq!(host.render("other", LensKind::Tag, "x").await, None);
+        // It runs in the extension's directory, told the protocol version.
+        let result = host.action("fake", "stamp", None, None).await;
+        let here = dir.file_name().unwrap().to_string_lossy().into_owned();
+        assert_eq!(
+            result,
+            Ok(ActionResult::Paste {
+                text: format!("{here} 1")
+            })
+        );
+        host.shutdown().await;
+        std::fs::remove_dir_all(&dir).ok();
+    }
+
+    #[tokio::test]
+    async fn a_hung_program_times_out_and_three_failures_disable_it() {
+        let (dir, host, mut notices) = host("sleep 30\n");
+        let started = Instant::now();
+        let result = host.action("fake", "stamp", None, None);
+        let result = tokio::time::timeout(Duration::from_secs(20), result)
+            .await
+            .unwrap();
+        assert_eq!(result, Err("extension fake timed out".to_owned()));
+        assert!(started.elapsed() >= ACTION_TIMEOUT);
+        // Renders time out after a second each.
+        assert_eq!(host.render("keys", LensKind::Tag, "a").await, None);
+        assert_eq!(host.render("keys", LensKind::Tag, "b").await, None);
+        assert_eq!(
+            notices.try_recv().unwrap(),
+            "extension fake disabled after 3 failures (timed out)"
+        );
+        // Disabled: no more requests, and the failed render is remembered.
+        assert_eq!(host.render("keys", LensKind::Tag, "c").await, None);
+        let result = host.action("fake", "stamp", None, None).await;
+        assert_eq!(
+            result,
+            Err("extension fake: disabled after 3 failures".to_owned())
+        );
+        host.shutdown().await;
+        std::fs::remove_dir_all(&dir).ok();
+    }
+
+    #[tokio::test]
+    async fn a_crash_or_a_bad_line_restarts_the_program() {
+        // Each start answers once, then exits (first) or writes garbage.
+        let script = "n=$(cat runs 2>/dev/null || echo 0); echo $((n+1)) > runs\n\
+                      IFS= read -r line\n\
+                      id=$(printf '%s' \"$line\" | sed 's/^{\"id\":\\([0-9]*\\).*/\\1/')\n\
+                      printf '{\"id\":%s,\"result\":{\"kind\":\"notice\",\"text\":\"run %s\"}}\\n' \"$id\" \"$((n+1))\"\n\
+                      if [ $n -eq 1 ]; then echo 'not json'; fi\n\
+                      sleep 1\n";
+        let (dir, host, mut notices) = host(script);
+        for run in 1..=3 {
+            let result = host.action("fake", "a", None, None).await;
+            assert_eq!(
+                result,
+                Ok(ActionResult::Notice {
+                    text: format!("run {run}")
+                })
+            );
+            // Let the program exit (or its bad line arrive).
+            tokio::time::sleep(Duration::from_millis(1500)).await;
+        }
+        assert_eq!(
+            notices.try_recv().unwrap(),
+            "extension fake disabled after 3 failures (exited)"
+        );
+        host.shutdown().await;
+        std::fs::remove_dir_all(&dir).ok();
+    }
+
+    #[tokio::test]
+    async fn shutdown_stops_the_whole_process_group() {
+        let script =
+            format!("sleep 60 &\necho $! > child\n{}", answering(
+            "printf '{\"id\":%s,\"result\":{\"kind\":\"notice\",\"text\":\"up\"}}\\n' \"$id\"",
+        ));
+        let (dir, host, _notices) = host(&script);
+        let up = host.action("fake", "a", None, None).await;
+        assert_eq!(up, Ok(ActionResult::Notice { text: "up".into() }));
+        let child: i32 = std::fs::read_to_string(dir.join("child"))
+            .unwrap()
+            .trim()
+            .parse()
+            .unwrap();
+        host.shutdown().await;
+        tokio::time::sleep(Duration::from_millis(200)).await;
+        // SAFETY: signal 0 only checks that the process exists.
+        let alive = unsafe { libc::kill(child, 0) } == 0;
+        assert!(!alive, "the program's own child is gone too");
+        std::fs::remove_dir_all(&dir).ok();
+    }
+}
diff --git a/src/tui/extensions/protocol.rs b/src/tui/extensions/protocol.rs
new file mode 100644
index 0000000..3fac90c
--- /dev/null
+++ b/src/tui/extensions/protocol.rs
@@ -0,0 +1,89 @@
+#[cfg(test)]
+mod tests {
+    use super::*;
+
+    #[test]
+    fn requests_are_one_json_line() {
+        let line = request(7, "render", render_params("keys", "tag", "Ctrl+C\n"));
+        assert!(!line.contains('\n'));
+        let v: Value = serde_json::from_str(&line).unwrap();
+        assert_eq!(v["id"], 7);
+        assert_eq!(v["params"]["source"], "Ctrl+C\n");
+        let session = SessionContext {
+            host: "local".into(),
+            cwd: "/p".into(),
+            name: None,
+            conversation: Some("c".into()),
+        };
+        let v = action_params("stamp", Some("x"), Some(&session));
+        assert_eq!(v["session"]["host"], "local");
+        assert_eq!(v["session"]["name"], Value::Null);
+        assert_eq!(action_params("stamp", None, None)["session"], Value::Null);
+    }
+
+    #[test]
+    fn replies_are_matched_by_id_and_bad_lines_are_protocol_errors() {
+        let ok = reply(br#"{"id":3,"result":{"inline":"x"}}"#).unwrap();
+        assert_eq!(ok, Some((3, Ok(json!({"inline": "x"})))));
+        let err = reply(br#"{"id":4,"error":"no\u001b[2J"}"#).unwrap();
+        assert_eq!(err, Some((4, Err("no[2J".to_owned()))));
+        assert_eq!(reply(br#"{"hello":1}"#).unwrap(), None);
+        assert!(reply(b"not json").is_err());
+    }
+
+    #[test]
+    fn renderings_are_capped_and_sanitized() {
+        let r = rendering(&json!({"inline": "⌃\u{1b}C\n", "block": ["┌─┐", "│\tC│"]})).unwrap();
+        assert_eq!(r.0.as_deref(), Some("⌃C "));
+        assert_eq!(r.1.unwrap().rows, ["┌─┐", "│ C│"]);
+        assert_eq!(rendering(&json!({})), Some((None, None)));
+        assert!(rendering(&json!({"inline": "x".repeat(513)})).is_none());
+        assert!(rendering(&json!({"block": ["x".repeat(201)]})).is_none());
+        let rows: Vec<String> = (0..101).map(|i| i.to_string()).collect();
+        assert!(rendering(&json!({ "block": rows })).is_none());
+        assert!(rendering(&json!({"inline": 3})).is_none());
+        assert!(rendering(&json!("x")).is_none());
+    }
+
+    #[test]
+    fn action_results_are_capped_and_sanitized() {
+        let doc =
+            action_result(&json!({"kind": "document", "title": "T\n", "markdown": "# a\u{7}"}));
+        assert_eq!(
+            doc,
+            Ok(ActionResult::Document {
+                title: "T ".into(),
+                markdown: "# a".into()
+            })
+        );
+        let paste = action_result(&json!({"kind": "paste", "text": "a\nb\u{1b}[201~"}));
+        assert_eq!(
+            paste,
+            Ok(ActionResult::Paste {
+                text: "a\nb[201~".into()
+            })
+        );
+        let big = "x".repeat(64 * 1024 + 1);
+        let too_big = action_result(&json!({"kind": "paste", "text": big}));
+        assert_eq!(too_big, Err("result too large".to_owned()));
+        let notice = action_result(&json!({"kind": "notice", "text": "n".repeat(300)}));
+        assert_eq!(
+            notice,
+            Ok(ActionResult::Notice {
+                text: "n".repeat(200)
+            })
+        );
+        let items: Vec<Value> = (0..600)
+            .map(|i| json!({"label": format!("l{i}"), "detail": "d", "result": {"kind": "paste", "text": "p"}}))
+            .chain([json!({"label": "nested", "result": {"kind": "list", "items": []}})])
+            .collect();
+        let Ok(ActionResult::List { items, .. }) =
+            action_result(&json!({"kind": "list", "title": "L", "items": items}))
+        else {
+            panic!("not a list")
+        };
+        assert_eq!(items.len(), 500);
+        assert_eq!(items[0].detail.as_deref(), Some("d"));
+        assert!(action_result(&json!({"kind": "shell"})).is_err());
+    }
+}
diff --git a/src/tui/interaction.rs b/src/tui/interaction.rs
index 078539a..fcf9b4f 100644
--- a/src/tui/interaction.rs
+++ b/src/tui/interaction.rs
@@ -1348,6 +1412,75 @@ mod tests {
         assert_eq!(renders(&mut app), [(3, vec!["d".to_owned()])]);
     }
 
+    /// A registry with an external extension `keys` (tag lens `keys`).
+    fn with_keys(app: &mut App) {
+        use crate::tui::extensions::{manifest::Program, Extension, Lens, Source};
+        let keys = Extension {
+            name: "keys".into(),
+            description: String::new(),
+            enabled: true,
+            lenses: vec![Lens {
+                name: "keys".into(),
+                kind: crate::proto::LensKind::Tag,
+                instruction: "use <keys>".into(),
+            }],
+            actions: vec![],
+            source: Source::External(Program {
+                dir: "/x".into(),
+                command: vec!["prog".into()],
+            }),
+        };
+        app.extensions = Registry::new(&Default::default(), true, vec![keys], &mut Vec::new());
+    }
+
+    #[test]
+    fn external_items_go_to_the_host_and_their_renderings_to_the_pane() {
+        let mut app = app_with(&[info(Some(1), None)]);
+        with_keys(&mut app);
+        let (id, host) = (app.sessions[0].id, app.sessions[0].host.clone());
+        app.set_host_features(&host, &["lens".to_owned()]);
+        let item = |tag: &str, text: &str| LensItem {
+            tag: tag.into(),
+            text: text.into(),
+        };
+        let items = vec![item("keys", "Ctrl+C"), item("latex", "x")];
+        let event = SessionEvent::Lens { seq: 3, items };
+        app.on_incoming_from(&host, Incoming::Event { id, event });
+        let external: Vec<(u64, Vec<LensItem>)> = app
+            .take_effects()
+            .into_iter()
+            .filter_map(|e| match e {
+                Effect::RenderExternal { seq, items, .. } => Some((seq, items)),
+                _ => None,
+            })
+            .collect();
+        assert_eq!(external, [(3, vec![item("keys", "Ctrl+C")])]);
+        // A stale list's renderings are dropped; the current one's kept.
+        let rendered = |seq| {
+            let rendering = Some((Some("⌃C".to_owned()), None));
+            (seq, vec![(item("keys", "Ctrl+C"), rendering)])
+        };
+        let (seq, items) = rendered(2);
+        app.on_external_rendered(&host, id, seq, items);
+        assert!(app.sessions[0].lens.is_empty());
+        let (seq, items) = rendered(3);
+        app.on_external_rendered(&host, id, seq, items);
+        let (tag, formula) = &app.sessions[0].lens[0];
+        assert_eq!(tag, "keys");
+        assert_eq!(formula.inline.as_deref(), Some("⌃C"));
+        // Without the extension, nothing goes to the host.
+        app.extensions = Registry::default();
+        let event = SessionEvent::Lens {
+            seq: 4,
+            items: vec![item("keys", "k")],
+        };
+        app.on_incoming_from(&host, Incoming::Event { id, event });
+        assert!(!app
+            .take_effects()
+            .iter()
+            .any(|e| matches!(e, Effect::RenderExternal { .. })));
+    }
+
     #[test]
     fn with_latex_off_formulas_are_not_rendered_and_spawns_say_so() {
         let mut app = app_with(&[info(Some(1), None)]);
diff --git a/src/tui/mod.rs b/src/tui/mod.rs
index 4c718e1..f7b52df 100644
--- a/src/tui/mod.rs
+++ b/src/tui/mod.rs
@@ -1600,7 +1738,7 @@ mod tests {
         use reader::layout::layout;
         let (tx, rx) = std::sync::mpsc::channel();
         let jobs = spawn_latex_worker(move |done| {
-            if let Rendered::Answers(job, answers) = done {
+            if let Rendered::Answers(job, answers, _) = done {
                 let _ = tx.send((job.gen, answers));
             }
         })
@@ -1636,6 +1774,7 @@ mod tests {
             answers,
             tags: vec!["latex".into()],
             fences: vec![],
+            external: vec![],
         };
         jobs.send(RenderJob::Answers(job)).unwrap();
         let (gen, rendered) = rx.recv_timeout(Duration::from_secs(30)).unwrap();
@@ -1700,8 +1839,8 @@ mod tests {
         use crate::proto::Answer;
         let (tx, rx) = std::sync::mpsc::channel();
         let jobs = spawn_latex_worker(move |done| {
-            if let Rendered::Answers(_, answers) = done {
-                let _ = tx.send(answers);
+            if let Rendered::Answers(_, answers, external) = done {
+                let _ = tx.send((answers, external));
             }
         })
         .unwrap();
@@ -1716,12 +1855,22 @@ mod tests {
             }],
             tags: vec!["latex".into(), "keys".into()],
             fences: vec!["mermaid".into()],
+            external: vec!["keys".into()],
         };
         jobs.send(RenderJob::Answers(job)).unwrap();
-        let answers = rx.recv_timeout(Duration::from_secs(10)).unwrap();
+        let (answers, external) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
         let answer = &answers[0];
-        // The external lens's item stays raw; latex is rendered.
+        // The external lens's item stays raw, listed for the host; latex is
+        // rendered.
         assert_eq!(answer.formulas[0], latex::Formula::raw("Ctrl+C"));
+        let want = reader::External {
+            answer: 0,
+            slot: reader::Slot::Formula(0),
+            lens: "keys".into(),
+            kind: crate::proto::LensKind::Tag,
+            source: "Ctrl+C".into(),
+        };
+        assert_eq!(external, [want]);
         assert_eq!(answer.formulas[1].inline.as_deref(), Some("x²"));
         // The fence lens is a lens block; another fence stays code.
         let lens: Vec<&reader::doc::Block> = answer
diff --git a/src/tui/reader/mod.rs b/src/tui/reader/mod.rs
index b0765b3..3ae1d27 100644
--- a/src/tui/reader/mod.rs
+++ b/src/tui/reader/mod.rs
@@ -272,7 +339,6 @@ fn list_lines(session: &SessionView) -> Vec<Line> {
 mod tests {
     use super::*;
     use crate::proto::SessionKind;
-    use crate::tui::latex::Rendered;
 
     fn key(code: KeyCode) -> KeyEvent {
         KeyEvent::new(code, KeyModifiers::NONE)
@@ -409,4 +475,30 @@ mod tests {
         assert_eq!(texts(&view.body(Some(&v), 40)), ["text"]);
         assert_eq!(view.scroll(), 0);
     }
+
+    #[test]
+    fn external_renderings_fill_their_slots() {
+        use crate::tui::typeset::Laid;
+        let text = "<keys>k</keys>\n\n```abc\nsrc\n```\n";
+        let prepared = doc::prepare_lenses(text, &["keys"], &["abc"]);
+        let answer = RenderedAnswer {
+            prompt: "p".into(),
+            doc: doc::parse(&prepared),
+            formulas: vec![Formula::raw("k")],
+        };
+        let mut view = ReaderView::new(uuid::Uuid::nil(), "local", 1);
+        view.set_answers(vec![answer]);
+        let before: Vec<String> = view.body(None, 40).iter().map(|l| l.text()).collect();
+        // Alone in its paragraph: display style, two columns in.
+        assert_eq!(before, ["  k", "", "  src"]);
+        let block = Laid {
+            rows: vec!["[abc]".into()],
+            baseline: 0,
+        };
+        view.set_external(0, &Slot::Formula(0), "k", Some((Some("⌨k".into()), None)));
+        view.set_external(0, &Slot::Fence(0), "src", Some((None, Some(block))));
+        view.set_external(0, &Slot::Fence(5), "x", None);
+        let after: Vec<String> = view.body(None, 40).iter().map(|l| l.text()).collect();
+        assert_eq!(after, ["  ⌨k", "", "  [abc]"]);
+    }
 }
`````
Run: `umask 022; cargo test --locked --bin claudio 2>&1 | grep -E '^error' | head -3`
Expected: `` error[E0422]: cannot find struct, variant or union type `Rendered` in this scope ``

Step 2: implement
`````diff
diff --git a/src/tui/app.rs b/src/tui/app.rs
index 3c5bf55..48f28f4 100644
--- a/src/tui/app.rs
+++ b/src/tui/app.rs
@@ -153,6 +153,14 @@ pub enum Effect {
         seq: u64,
         srcs: Vec<String>,
     },
+    /// Render session `id`'s external tag-lens items of list `seq` with the
+    /// extension host; they come back through [`App::on_external_rendered`].
+    RenderExternal {
+        host: String,
+        id: SessionId,
+        seq: u64,
+        items: Vec<crate::proto::LensItem>,
+    },
     /// Render the answers of session `id` for the reader view opened under
     /// `gen`, on the `latex-render` worker; they come back through
     /// [`App::on_answers_rendered`].
diff --git a/src/tui/extensions/host.rs b/src/tui/extensions/host.rs
index 87a2f7a..1607cf4 100644
--- a/src/tui/extensions/host.rs
+++ b/src/tui/extensions/host.rs
@@ -1,3 +1,436 @@
+//! The extension host: runs external extensions' programs and talks to
+//! them over protocol v1 (JSON lines on stdin/stdout, [`super::protocol`]).
+//!
+//! Driven by `tui/mod.rs` (never by `App`): each request is a future the
+//! event loop spawns, so a slow or hung program never blocks the UI thread
+//! or the render worker. Per extension:
+//! - the program starts on the first request, in the extension's directory,
+//!   in its own process group, stdin/stdout piped, stderr discarded, with
+//!   the client's environment minus anything secret or claudio's;
+//! - at most [`MAX_IN_FLIGHT`] requests are in flight, each with a timeout;
+//! - a crash, a protocol error (a line that is not JSON, or longer than
+//!   [`protocol::MAX_LINE`]) or a timeout is a failure; the program is
+//!   started again on the next request, and [`FAILURES`] failures within
+//!   [`FAILURE_WINDOW`] disable the extension for the rest of the run;
+//! - [`ExtensionHost::shutdown`] stops every program (SIGTERM to its group,
+//!   SIGKILL after a second).
+
+use std::collections::{HashMap, VecDeque};
+use std::future::Future;
+use std::sync::atomic::{AtomicU64, Ordering};
+use std::sync::{Arc, Mutex as StdMutex, Weak};
+use std::time::{Duration, Instant};
+
+use serde_json::Value;
+use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
+use tokio::process::{Child, ChildStdin, ChildStdout, Command};
+use tokio::sync::{mpsc, oneshot, Mutex, Semaphore};
+
+use super::manifest::Program;
+use super::protocol::{self, Rendering, SessionContext};
+use super::{ActionResult, Registry, Source};
+use crate::proto::LensKind;
+
+/// How long a render may take.
+pub const RENDER_TIMEOUT: Duration = Duration::from_secs(1);
+/// How long an action may take.
+pub const ACTION_TIMEOUT: Duration = Duration::from_secs(15);
+/// Requests in flight per extension; more wait for a slot.
+pub const MAX_IN_FLIGHT: usize = 16;
+/// Failures within [`FAILURE_WINDOW`] that disable an extension.
+pub const FAILURES: usize = 3;
+pub const FAILURE_WINDOW: Duration = Duration::from_secs(300);
+/// How long a failed render is remembered (shown raw, not asked again).
+const FAILED_RENDER_TTL: Duration = Duration::from_secs(60);
+/// Renderings remembered, by lens and source.
+const RENDER_CACHE: usize = 1024;
+/// How long a stopped program has to exit before it is killed.
+const STOP_GRACE: Duration = Duration::from_secs(1);
+
+/// Why a request got no result.
+#[derive(Debug, Clone, PartialEq)]
+pub enum Failure {
+    /// The extension was disabled after too many failures.
+    Disabled,
+    /// The program could not be started.
+    Start(String),
+    /// It exited, or broke the protocol, before answering.
+    Crashed,
+    Timeout,
+    /// It answered with an error.
+    Error(String),
+}
+
+impl std::fmt::Display for Failure {
+    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
+        match self {
+            Failure::Disabled => write!(f, "disabled after {FAILURES} failures"),
+            Failure::Start(why) => write!(f, "could not start: {why}"),
+            Failure::Crashed => write!(f, "stopped before answering"),
+            Failure::Timeout => write!(f, "timed out"),
+            Failure::Error(why) => write!(f, "failed: {why}"),
+        }
+    }
+}
+
+/// Whether the client's variable `name` stays out of an extension's
+/// environment: claude's session markers, anything secret, and claudio's.
+fn scrubbed(name: &str) -> bool {
+    crate::claude::SESSION_MARKERS.contains(&name)
+        || matches!(name, "ANTHROPIC_API_KEY" | "ANTHROPIC_AUTH_TOKEN")
+        || name.starts_with("CLAUDIO_")
+}
+
+type Pending = Arc<StdMutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;
+
+/// A running program.
+struct Proc {
+    /// Which start this is: a reader that ends only clears its own.
+    gen: u64,
+    stdin: ChildStdin,
+    pid: Option<u32>,
+    child: Child,
+}
+
+/// One external extension's program and its health.
+struct Ext {
+    name: String,
+    program: Program,
+    proc: Mutex<Option<Proc>>,
+    pending: Pending,
+    /// Failure times within the window, and whether it is disabled.
+    health: StdMutex<(VecDeque<Instant>, bool)>,
+    slots: Semaphore,
+    next_id: AtomicU64,
+    gen: AtomicU64,
+    notices: mpsc::UnboundedSender<String>,
+}
+
+impl Ext {
+    fn disabled(&self) -> bool {
+        self.health.lock().unwrap_or_else(|e| e.into_inner()).1
+    }
+
+    /// Count a failure; the [`FAILURES`]th within the window disables the
+    /// extension, with a notice.
+    fn fail(&self, why: &str) {
+        let mut health = self.health.lock().unwrap_or_else(|e| e.into_inner());
+        let now = Instant::now();
+        health.0.push_back(now);
+        while health
+            .0
+            .front()
+            .is_some_and(|t| now.duration_since(*t) > FAILURE_WINDOW)
+        {
+            health.0.pop_front();
+        }
+        if !health.1 && health.0.len() >= FAILURES {
+            health.1 = true;
+            let _ = self.notices.send(format!(
+                "extension {} disabled after {FAILURES} failures ({why})",
+                self.name
+            ));
+        }
+    }
+
+    /// Send `method` with `params` and wait at most `timeout` for its result.
+    async fn call(
+        self: &Arc<Self>,
+        method: &str,
+        params: Value,
+        timeout: Duration,
+    ) -> Result<Value, Failure> {
+        if self.disabled() {
+            return Err(Failure::Disabled);
+        }
+        let _slot = self.slots.acquire().await.map_err(|_| Failure::Disabled)?;
+        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
+        let (tx, rx) = oneshot::channel();
+        self.lock_pending().insert(id, tx);
+        let mut line = protocol::request(id, method, params);
+        line.push('\n');
+        let written = {
+            let mut slot = self.proc.lock().await;
+            if slot.is_none() {
+                match self.start() {
+                    Ok(proc) => *slot = Some(proc),
+                    Err(e) => {
+                        self.lock_pending().remove(&id);
+                        self.fail("could not start");
+                        return Err(Failure::Start(e.to_string()));
+                    }
+                }
+            }
+            let proc = slot.as_mut().expect("started above");
+            let stdin = &mut proc.stdin;
+            match stdin.write_all(line.as_bytes()).await {
+                Ok(()) => stdin.flush().await,
+                Err(e) => Err(e),
+            }
+        };
+        if written.is_err() {
+            // The reader sees the exit and counts it.
+            self.lock_pending().remove(&id);
+            return Err(Failure::Crashed);
+        }
+        match tokio::time::timeout(timeout, rx).await {
+            Ok(Ok(Ok(value))) => Ok(value),
+            Ok(Ok(Err(why))) => Err(Failure::Error(why)),
+            Ok(Err(_)) => Err(Failure::Crashed),
+            Err(_) => {
+                self.lock_pending().remove(&id);
+                self.fail("timed out");
+                Err(Failure::Timeout)
+            }
+        }
+    }
+
+    fn lock_pending(
+        &self,
+    ) -> std::sync::MutexGuard<'_, HashMap<u64, oneshot::Sender<Result<Value, String>>>> {
+        self.pending.lock().unwrap_or_else(|e| e.into_inner())
+    }
+
+    /// Start the program, and the task that reads its replies.
+    fn start(self: &Arc<Self>) -> std::io::Result<Proc> {
+        let mut cmd = Command::new(self.program.program());
+        cmd.args(self.program.command.iter().skip(1))
+            .current_dir(&self.program.dir)
+            .stdin(std::process::Stdio::piped())
+            .stdout(std::process::Stdio::piped())
+            .stderr(std::process::Stdio::null())
+            .process_group(0)
+            .kill_on_drop(true);
+        for (name, _) in std::env::vars_os() {
+            if scrubbed(&name.to_string_lossy()) {
+                cmd.env_remove(name);
+            }
+        }
+        cmd.env("CLAUDIO_EXTENSION", &self.name);
+        cmd.env("CLAUDIO_EXTENSION_PROTOCOL", protocol::VERSION.to_string());
+        let mut child = cmd.spawn()?;
+        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
+            return Err(std::io::Error::other("no pipes"));
+        };
+        let gen = self.gen.fetch_add(1, Ordering::Relaxed) + 1;
+        tokio::spawn(read_replies(Arc::downgrade(self), gen, stdout));
+        Ok(Proc {
+            gen,
+            stdin,
+            pid: child.id(),
+            child,
+        })
+    }
+
+    /// Start `gen` ended (`why`): stop it, fail what waits on it, count it.
+    async fn lost(&self, gen: u64, why: &str) {
+        let mut slot = self.proc.lock().await;
+        if slot.as_ref().is_some_and(|p| p.gen == gen) {
+            if let Some(proc) = slot.take() {
+                stop(proc);
+            }
+            self.lock_pending().clear();
+            self.fail(why);
+        }
+    }
+}
+
+/// Read start `gen`'s replies until it exits or breaks the protocol.
+async fn read_replies(ext: Weak<Ext>, gen: u64, stdout: ChildStdout) {
+    let mut reader = BufReader::new(stdout);
+    let why = loop {
+        let mut line = Vec::new();
+        let limit = protocol::MAX_LINE as u64 + 1;
+        match (&mut reader).take(limit).read_until(b'\n', &mut line).await {
+            Ok(0) | Err(_) => break "exited",
+            Ok(_) => {}
+        }
+        if line.pop() != Some(b'\n') {
+            break if line.len() >= protocol::MAX_LINE {
+                "sent a line over 1 MiB"
+            } else {
+                "exited"
+            };
+        }
+        let Some(ext) = ext.upgrade() else { return };
+        match protocol::reply(&line) {
+            Ok(Some((id, result))) => {
+                if let Some(tx) = ext.lock_pending().remove(&id) {
+                    let _ = tx.send(result);
+                }
+            }
+            Ok(None) => {}
+            Err(_) => break "protocol error",
+        }
+    };
+    if let Some(ext) = ext.upgrade() {
+        ext.lost(gen, why).await;
+    }
+}
+
+/// SIGTERM to `proc`'s group, SIGKILL after [`STOP_GRACE`]; reaped in the
+/// background.
+fn stop(mut proc: Proc) {
+    signal_group(proc.pid, libc::SIGTERM);
+    tokio::spawn(async move {
+        if tokio::time::timeout(STOP_GRACE, proc.child.wait())
+            .await
+            .is_err()
+        {
+            signal_group(proc.pid, libc::SIGKILL);
+            let _ = proc.child.wait().await;
+        }
+    });
+}
+
+fn signal_group(pid: Option<u32>, signal: i32) {
+    if let Some(pid) = pid.filter(|p| *p > 1) {
+        // SAFETY: kill(2) with a negative pid signals that process group,
+        // which the program leads (it was started with process_group(0)).
+        unsafe {
+            libc::kill(-(pid as i32), signal);
+        }
+    }
+}
+
+/// Renderings by lens and source: a success is kept, a failure for
+/// [`FAILED_RENDER_TTL`].
+#[derive(Default)]
+struct RenderCache {
+    entries: HashMap<(String, String), (Instant, Option<Rendering>)>,
+}
+
+/// The external extensions' programs, by extension name.
+pub struct ExtensionHost {
+    exts: HashMap<String, Arc<Ext>>,
+    /// Which extension owns each lens.
+    lenses: HashMap<String, String>,
+    cache: Arc<StdMutex<RenderCache>>,
+}
+
+impl ExtensionHost {
+    /// The host of `registry`'s enabled external extensions (none started
+    /// yet). Notices (an extension disabled) go to `notices`.
+    pub fn new(registry: &Registry, notices: mpsc::UnboundedSender<String>) -> ExtensionHost {
+        let mut exts = HashMap::new();
+        let mut lenses = HashMap::new();
+        for ext in registry.enabled() {
+            let Source::External(program) = &ext.source else {
+                continue;
+            };
+            for lens in &ext.lenses {
+                lenses.insert(lens.name.clone(), ext.name.clone());
+            }
+            let handle = Ext {
+                name: ext.name.clone(),
+                program: program.clone(),
+                proc: Mutex::new(None),
+                pending: Arc::default(),
+                health: StdMutex::new((VecDeque::new(), false)),
+                slots: Semaphore::new(MAX_IN_FLIGHT),
+                next_id: AtomicU64::new(0),
+                gen: AtomicU64::new(0),
+                notices: notices.clone(),
+            };
+            exts.insert(ext.name.clone(), Arc::new(handle));
+        }
+        ExtensionHost {
+            exts,
+            lenses,
+            cache: Arc::default(),
+        }
+    }
+
+    /// Render `source` of lens `lens` with its extension (within
+    /// [`RENDER_TIMEOUT`]); `None` leaves it raw (no such lens, a failure,
+    /// an error, or a result over the caps). Cached by lens and source.
+    pub fn render(
+        &self,
+        lens: &str,
+        kind: LensKind,
+        source: &str,
+    ) -> impl Future<Output = Option<Rendering>> + Send + 'static {
+        let ext = self
+            .lenses
+            .get(lens)
+            .and_then(|e| self.exts.get(e))
+            .cloned();
+        let cache = Arc::clone(&self.cache);
+        let key = (lens.to_owned(), source.to_owned());
+        let kind = match kind {
+            LensKind::Fence => "fence",
+            _ => "tag",
+        };
+        async move {
+            let ext = ext?;
+            if let Some((at, cached)) = cache
+                .lock()
+                .unwrap_or_else(|e| e.into_inner())
+                .entries
+                .get(&key)
+            {
+                if cached.is_some() || at.elapsed() < FAILED_RENDER_TTL {
+                    return cached.clone();
+                }
+            }
+            let params = protocol::render_params(&key.0, kind, &key.1);
+            let result = ext.call("render", params, RENDER_TIMEOUT).await;
+            let rendering = result.ok().as_ref().and_then(protocol::rendering);
+            let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
+            if cache.entries.len() >= RENDER_CACHE {
+                cache.entries.clear();
+            }
+            cache
+                .entries
+                .insert(key, (Instant::now(), rendering.clone()));
+            rendering
+        }
+    }
+
+    /// Run action `action` of extension `ext` (within [`ACTION_TIMEOUT`]):
+    /// its result, or the notice that says why there is none.
+    pub fn action(
+        &self,
+        ext: &str,
+        action: &str,
+        input: Option<String>,
+        session: Option<SessionContext>,
+    ) -> impl Future<Output = Result<ActionResult, String>> + Send + 'static {
+        let handle = self.exts.get(ext).cloned();
+        let (name, params) = (
+            ext.to_owned(),
+            protocol::action_params(action, input.as_deref(), session.as_ref()),
+        );
+        async move {
+            let Some(handle) = handle else {
+                return Err(format!("extension {name} is not running"));
+            };
+            match handle.call("action", params, ACTION_TIMEOUT).await {
+                Ok(result) => protocol::action_result(&result),
+                Err(Failure::Timeout) => Err(format!("extension {name} timed out")),
+                Err(failure) => Err(format!("extension {name}: {failure}")),
+            }
+        }
+    }
+
+    /// Stop every program: SIGTERM to its group, SIGKILL after a second.
+    pub async fn shutdown(&self) {
+        for ext in self.exts.values() {
+            let Some(mut proc) = ext.proc.lock().await.take() else {
+                continue;
+            };
+            signal_group(proc.pid, libc::SIGTERM);
+            if tokio::time::timeout(STOP_GRACE, proc.child.wait())
+                .await
+                .is_err()
+            {
+                signal_group(proc.pid, libc::SIGKILL);
+                let _ = proc.child.wait().await;
+            }
+        }
+    }
+}
+
 #[cfg(test)]
 mod tests {
     use super::*;
diff --git a/src/tui/extensions/mod.rs b/src/tui/extensions/mod.rs
index 9544839..8dd01b6 100644
--- a/src/tui/extensions/mod.rs
+++ b/src/tui/extensions/mod.rs
@@ -10,7 +10,9 @@
 //! draws, the context a spawn carries ([`Registry::session_extensions`]) and
 //! the leader's actions.
 
+pub mod host;
 pub mod manifest;
+pub mod protocol;
 
 use std::collections::BTreeMap;
 
@@ -44,6 +46,27 @@ pub struct ActionDecl {
     pub input: Option<String>,
 }
 
+/// What an action gives (built-in and external actions share it).
+#[derive(Debug, Clone, PartialEq)]
+pub enum ActionResult {
+    /// Markdown shown in a reader-style modal, its lenses rendered.
+    Document { title: String, markdown: String },
+    /// A status-line notice.
+    Notice { text: String },
+    /// Text pasted into the active claude session (never followed by Enter).
+    Paste { text: String },
+    /// A list to pick from; each item carries its own result (not a list).
+    List { title: String, items: Vec<ListItem> },
+}
+
+/// One entry of [`ActionResult::List`].
+#[derive(Debug, Clone, PartialEq)]
+pub struct ListItem {
+    pub label: String,
+    pub detail: Option<String>,
+    pub result: ActionResult,
+}
+
 /// Where an extension's code is.
 #[derive(Debug, Clone, PartialEq)]
 pub enum Source {
diff --git a/src/tui/extensions/protocol.rs b/src/tui/extensions/protocol.rs
index 46f54b9..3fac90c 100644
--- a/src/tui/extensions/protocol.rs
+++ b/src/tui/extensions/protocol.rs
@@ -1,3 +1,186 @@
+//! Protocol v1 with external extensions: the request lines claudio writes
+//! and what it accepts back. Pure: the extension host (`host.rs`) moves the
+//! lines; this module builds them and turns results into capped, sanitized
+//! values. Every string a program returns is untrusted.
+
+use serde_json::{json, Value};
+use unicode_width::UnicodeWidthStr;
+
+use super::{ActionResult, ListItem};
+use crate::claude::latex::sanitize;
+use crate::tui::typeset::Laid;
+
+/// The protocol version in `CLAUDIO_EXTENSION_PROTOCOL`.
+pub const VERSION: u32 = 1;
+/// The longest line read from a program.
+pub const MAX_LINE: usize = 1 << 20;
+const INLINE_MAX_COLS: usize = 512;
+const BLOCK_MAX_COLS: usize = 200;
+const BLOCK_MAX_ROWS: usize = 100;
+const MARKDOWN_MAX: usize = 256 * 1024;
+const PASTE_MAX: usize = 64 * 1024;
+const LIST_MAX: usize = 500;
+const LABEL_MAX: usize = 200;
+const DETAIL_MAX: usize = 400;
+const NOTICE_MAX: usize = 200;
+const TITLE_MAX: usize = 120;
+
+/// A render result: its one-line form and its block, each optional.
+pub type Rendering = (Option<String>, Option<Laid>);
+
+/// What a session an action runs for is, as the program is told.
+#[derive(Debug, Clone, PartialEq)]
+pub struct SessionContext {
+    pub host: String,
+    pub cwd: String,
+    pub name: Option<String>,
+    pub conversation: Option<String>,
+}
+
+/// A request line (without its newline).
+pub fn request(id: u64, method: &str, params: Value) -> String {
+    json!({ "id": id, "method": method, "params": params }).to_string()
+}
+
+/// `render`'s params.
+pub fn render_params(lens: &str, kind: &str, source: &str) -> Value {
+    json!({ "lens": lens, "kind": kind, "source": source })
+}
+
+/// `action`'s params.
+pub fn action_params(action: &str, input: Option<&str>, session: Option<&SessionContext>) -> Value {
+    let session = session.map(
+        |s| json!({ "host": s.host, "cwd": s.cwd, "name": s.name, "conversation": s.conversation }),
+    );
+    json!({ "action": action, "input": input, "session": session })
+}
+
+/// One line from a program: the id it answers and its result or error.
+/// `None` for a line that is valid JSON but no answer (ignored); `Err` for
+/// one that is not JSON (a protocol error).
+#[allow(clippy::type_complexity)]
+pub fn reply(line: &[u8]) -> Result<Option<(u64, Result<Value, String>)>, String> {
+    let v: Value = serde_json::from_slice(line).map_err(|e| format!("bad line: {e}"))?;
+    let Some(id) = v.get("id").and_then(Value::as_u64) else {
+        return Ok(None);
+    };
+    if let Some(result) = v.get("result") {
+        return Ok(Some((id, Ok(result.clone()))));
+    }
+    let error = v.get("error").map(|e| match e {
+        Value::String(s) => s.clone(),
+        other => other.to_string(),
+    });
+    Ok(Some((
+        id,
+        Err(clip(&error.unwrap_or_default(), NOTICE_MAX)),
+    )))
+}
+
+/// A render result: its one-line and block forms, each optional. `None`
+/// (the item stays raw) when it breaks a cap or is not an object.
+pub fn rendering(result: &Value) -> Option<Rendering> {
+    let inline = match result.get("inline") {
+        None | Some(Value::Null) => None,
+        Some(Value::String(s)) => {
+            let line = one_line(s);
+            if line.width() > INLINE_MAX_COLS {
+                return None;
+            }
+            Some(line).filter(|l| !l.is_empty())
+        }
+        Some(_) => return None,
+    };
+    let block = match result.get("block") {
+        None | Some(Value::Null) => None,
+        Some(Value::Array(rows)) => {
+            let mut out = Vec::with_capacity(rows.len());
+            for row in rows {
+                let row = one_line(row.as_str()?);
+                if row.width() > BLOCK_MAX_COLS {
+                    return None;
+                }
+                out.push(row);
+            }
+            if out.len() > BLOCK_MAX_ROWS {
+                return None;
+            }
+            Some(Laid {
+                rows: out,
+                baseline: 0,
+            })
+            .filter(|l| !l.rows.is_empty())
+        }
+        Some(_) => return None,
+    };
+    result.is_object().then_some((inline, block))
+}
+
+/// An action result, capped and sanitized; `Err` is the notice to show
+/// instead (a malformed result, or one too large).
+pub fn action_result(result: &Value) -> Result<ActionResult, String> {
+    action_result_in(result, true)
+}
+
+fn action_result_in(result: &Value, top: bool) -> Result<ActionResult, String> {
+    let text = |key: &str| result.get(key).and_then(Value::as_str);
+    match text("kind") {
+        Some("document") => {
+            let markdown = sanitize(text("markdown").unwrap_or_default());
+            if markdown.len() > MARKDOWN_MAX {
+                return Err("result too large".to_owned());
+            }
+            Ok(ActionResult::Document {
+                title: clip(text("title").unwrap_or_default(), TITLE_MAX),
+                markdown,
+            })
+        }
+        Some("notice") => Ok(ActionResult::Notice {
+            text: clip(text("text").unwrap_or_default(), NOTICE_MAX),
+        }),
+        Some("paste") => {
+            let text = sanitize(text("text").unwrap_or_default());
+            if text.len() > PASTE_MAX {
+                return Err("result too large".to_owned());
+            }
+            Ok(ActionResult::Paste { text })
+        }
+        Some("list") if top => {
+            let items = result.get("items").and_then(Value::as_array);
+            let items = items
+                .into_iter()
+                .flatten()
+                .filter_map(|item| {
+                    let label = clip(item.get("label")?.as_str()?, LABEL_MAX);
+                    let detail = item.get("detail").and_then(Value::as_str);
+                    let result = action_result_in(item.get("result")?, false).ok()?;
+                    Some(ListItem {
+                        label,
+                        detail: detail.map(|d| clip(d, DETAIL_MAX)),
+                        result,
+                    })
+                })
+                .take(LIST_MAX)
+                .collect();
+            Ok(ActionResult::List {
+                title: clip(text("title").unwrap_or_default(), TITLE_MAX),
+                items,
+            })
+        }
+        _ => Err("the extension gave an unknown result".to_owned()),
+    }
+}
+
+/// `s` sanitized on one line (tabs and newlines as spaces).
+fn one_line(s: &str) -> String {
+    sanitize(s).replace(['\n', '\t'], " ")
+}
+
+/// `s` on one line, cut to `max` chars on a char boundary.
+fn clip(s: &str, max: usize) -> String {
+    one_line(s).chars().take(max).collect()
+}
+
 #[cfg(test)]
 mod tests {
     use super::*;
diff --git a/src/tui/interaction.rs b/src/tui/interaction.rs
index e41cb32..fcf9b4f 100644
--- a/src/tui/interaction.rs
+++ b/src/tui/interaction.rs
@@ -21,8 +21,9 @@ use super::app::{App, Effect, Mode, ReplyTo};
 use super::confirm::ConfirmPrompt;
 use super::git_view::GitView;
 use super::keymap::Action;
-use super::latex::Formula;
-use super::reader::{ReaderView, RenderedAnswer};
+use super::extensions::protocol::Rendering;
+use super::latex::{Formula, Rendered};
+use super::reader::{External, ReaderView, RenderedAnswer};
 use super::selection::{self, Pos, Selection, Unit};
 use super::sessions::SessionView;
 use super::stats_view::StatsView;
@@ -853,6 +854,11 @@ impl App {
             .into_iter()
             .filter(|item| tags.contains(&item.tag.as_str()))
             .collect();
+        let external: Vec<LensItem> = items
+            .iter()
+            .filter(|item| self.extensions.external_lens(&item.tag).is_some())
+            .cloned()
+            .collect();
         let v = &mut self.sessions[i];
         if seq <= v.latex_seq {
             return;
@@ -864,13 +870,71 @@ impl App {
             .map(|item| item.text.clone())
             .collect();
         v.lens_items = items;
+        let (host, id) = (v.host.clone(), v.id);
         let render = Effect::RenderLatex {
-            host: v.host.clone(),
-            id: v.id,
+            host: host.clone(),
+            id,
             seq,
             srcs: v.latex_srcs.clone(),
         };
         self.emit(render);
+        if !external.is_empty() {
+            self.emit(Effect::RenderExternal {
+                host,
+                id,
+                seq,
+                items: external,
+            });
+        }
+    }
+
+    /// The extension host rendered list `seq`'s external items of session
+    /// `id` on `host`: shown, unless a newer list came since.
+    pub fn on_external_rendered(
+        &mut self,
+        host: &str,
+        id: SessionId,
+        seq: u64,
+        items: Vec<(LensItem, Option<Rendering>)>,
+    ) {
+        let Some(i) = self.index_of(id).filter(|&i| self.sessions[i].host == host) else {
+            return;
+        };
+        let v = &mut self.sessions[i];
+        if v.latex_seq != seq {
+            return;
+        }
+        v.lens = items
+            .into_iter()
+            .map(|(item, rendering)| {
+                let formula = match rendering {
+                    Some((inline, block)) => {
+                        let text = None;
+                        Formula::new(&item.text, Rendered { inline, block, text })
+                    }
+                    None => Formula::raw(&item.text),
+                };
+                (item.tag, formula)
+            })
+            .collect();
+        if self.active == Some(i) {
+            self.redraw = true;
+        }
+    }
+
+    /// The extension host rendered the external items of reader view `gen`.
+    pub fn on_reader_external(
+        &mut self,
+        gen: u64,
+        id: SessionId,
+        rendered: Vec<(External, Option<Rendering>)>,
+    ) {
+        if let Some(view) = self.reader_mut(gen, id) {
+            for (work, rendering) in rendered {
+                view.set_external(work.answer, &work.slot, &work.source, rendering);
+            }
+            self.redraw = true;
+        }
     }
 
     /// The render worker is done with list `seq` of session `id` on `host`:
diff --git a/src/tui/mod.rs b/src/tui/mod.rs
index 797f996..f7b52df 100644
--- a/src/tui/mod.rs
+++ b/src/tui/mod.rs
@@ -506,12 +506,30 @@ enum HostEvent {
         seq: u64,
         formulas: Vec<latex::Formula>,
     },
-    /// The render worker finished the answers of a reader view.
+    /// The render worker finished the answers of a reader view; the
+    /// external lenses' items and blocks in them are still to render.
     AnswersRendered {
         gen: u64,
         id: crate::proto::SessionId,
         answers: Vec<reader::RenderedAnswer>,
+        external: Vec<reader::External>,
     },
+    /// The extension host rendered a session's external lens items (list
+    /// `seq`), in order; `None` stays raw.
+    ExternalRendered {
+        host: String,
+        id: crate::proto::SessionId,
+        seq: u64,
+        items: Vec<(crate::proto::LensItem, Option<extensions::protocol::Rendering>)>,
+    },
+    /// The extension host rendered the external items of reader view `gen`.
+    ReaderExternalRendered {
+        gen: u64,
+        id: crate::proto::SessionId,
+        rendered: Vec<(reader::External, Option<extensions::protocol::Rendering>)>,
+    },
+    /// A notice from the extension host (an extension was disabled).
+    ExtensionNotice(String),
 }
 
 // ── Formula rendering ─────────────────────────────────────────────────────────
@@ -530,9 +548,11 @@ struct AnswersJob {
     gen: u64,
     id: crate::proto::SessionId,
     answers: Vec<crate::proto::Answer>,
-    /// The enabled tag and fence lenses (the registry's, when sent).
+    /// The enabled tag and fence lenses (the registry's, when sent), and
+    /// which of them are external.
     tags: Vec<String>,
     fences: Vec<String>,
+    external: Vec<String>,
 }
 
 /// Work for the `latex-render` thread, one job at a time.
@@ -544,7 +564,7 @@ enum RenderJob {
 /// What the `latex-render` thread made of a job.
 enum Rendered {
     Latex(LatexJob, Vec<latex::Formula>),
-    Answers(AnswersJob, Vec<reader::RenderedAnswer>),
+    Answers(AnswersJob, Vec<reader::RenderedAnswer>, Vec<reader::External>),
 }
 
 /// Start the `latex-render` thread: it renders each job (its formulas from
@@ -572,13 +592,10 @@ fn spawn_latex_worker(
                         done(Rendered::Latex(job, formulas));
                     }
                     RenderJob::Answers(job) => {
-                        let answers = render_lens_answers(
-                            &job.answers,
-                            (&job.tags, &job.fences),
-                            &mut cache,
-                            &running,
-                        );
-                        done(Rendered::Answers(job, answers));
+                        let lenses = (&job.tags[..], &job.fences[..], &job.external[..]);
+                        let (answers, work) =
+                            render_lens_answers(&job.answers, lenses, &mut cache, &running);
+                        done(Rendered::Answers(job, answers, work));
                     }
                 }
             }
@@ -595,35 +612,60 @@ fn render_answers(
     running: &Arc<AtomicUsize>,
 ) -> Vec<reader::RenderedAnswer> {
     let tags = ["latex".to_owned()];
-    render_lens_answers(answers, (&tags, &[]), cache, running)
+    render_lens_answers(answers, (&tags, &[], &[]), cache, running).0
 }
 
 /// `answers` as the reader shows them: each a markdown document whose
 /// items of tag lenses `lenses.0` (the first `MAX_FORMULAS`) and blocks of
 /// fence lenses `lenses.1` are rendered through the guard and its cache,
-/// like the pane's. An external lens's are left raw here: the extension
-/// host renders them.
+/// like the pane's. The external lenses (`lenses.2`) are left raw here and
+/// listed: the extension host renders them.
 fn render_lens_answers(
     answers: &[crate::proto::Answer],
-    lenses: (&[String], &[String]),
+    lenses: (&[String], &[String], &[String]),
     cache: &mut RenderCache,
     running: &Arc<AtomicUsize>,
-) -> Vec<reader::RenderedAnswer> {
+) -> (Vec<reader::RenderedAnswer>, Vec<reader::External>) {
+    use crate::proto::LensKind;
     let tags: Vec<&str> = lenses.0.iter().map(String::as_str).collect();
     let fences: Vec<&str> = lenses.1.iter().map(String::as_str).collect();
-    answers
+    let external = |lens: &str| lenses.2.iter().any(|e| e == lens);
+    let mut work = Vec::new();
+    let rendered = answers
         .iter()
-        .map(|answer| {
+        .enumerate()
+        .map(|(a, answer)| {
             let prepared = reader::doc::prepare_lenses(&answer.text, &tags, &fences);
-            let formulas = prepared
-                .tags
-                .iter()
-                .zip(&prepared.lenses)
-                .map(|(tag, lens)| cache.render(lens, &tag.src, running))
-                .collect();
+            let mut formulas = Vec::with_capacity(prepared.tags.len());
+            for (i, (tag, lens)) in prepared.tags.iter().zip(&prepared.lenses).enumerate() {
+                if external(lens) {
+                    work.push(reader::External {
+                        answer: a,
+                        slot: reader::Slot::Formula(i),
+                        lens: lens.clone(),
+                        kind: LensKind::Tag,
+                        source: tag.src.clone(),
+                    });
+                    formulas.push(latex::Formula::raw(&tag.src));
+                } else {
+                    formulas.push(cache.render(lens, &tag.src, running));
+                }
+            }
             let mut doc = reader::doc::parse(&prepared);
+            let mut n = 0;
             doc.each_lens(&mut |lens, src, rows| {
-                *rows = cache.render(lens, &src, running).block.map(|b| b.rows);
+                if external(lens) {
+                    work.push(reader::External {
+                        answer: a,
+                        slot: reader::Slot::Fence(n),
+                        lens: lens.to_owned(),
+                        kind: LensKind::Fence,
+                        source: src,
+                    });
+                } else {
+                    *rows = cache.render(lens, &src, running).block.map(|b| b.rows);
+                }
+                n += 1;
             });
             reader::RenderedAnswer {
                 prompt: answer.prompt.clone(),
@@ -631,7 +673,8 @@ fn render_lens_answers(
                 formulas,
             }
         })
-        .collect()
+        .collect();
+    (rendered, work)
 }
 
 /// Renderings by lens and source, the [`CACHE_SIZE`] most recently used:
@@ -746,6 +789,31 @@ fn guarded<T: Send + 'static>(
     }
 }
 
+/// Render each of `requests` (lens, kind, source) with the extension host,
+/// concurrently; the renderings in order (`None`: stays raw).
+fn render_all(
+    host: &extensions::host::ExtensionHost,
+    requests: Vec<(String, crate::proto::LensKind, String)>,
+) -> impl std::future::Future<Output = Vec<Option<extensions::protocol::Rendering>>> + Send + 'static
+{
+    let futures: Vec<_> = requests
+        .iter()
+        .map(|(lens, kind, source)| host.render(lens, *kind, source))
+        .collect();
+    async move {
+        let mut set = tokio::task::JoinSet::new();
+        let count = futures.len();
+        for (i, f) in futures.into_iter().enumerate() {
+            set.spawn(async move { (i, f.await) });
+        }
+        let mut out = vec![None; count];
+        while let Some(Ok((i, rendering))) = set.join_next().await {
+            out[i] = rendering;
+        }
+        out
+    }
+}
+
 // ── Event loop ────────────────────────────────────────────────────────────────
 
 /// Resolves when the terminal's session is hung up or the process is asked to
@@ -854,16 +922,29 @@ async fn event_loop(
                 seq: job.seq,
                 formulas,
             },
-            Rendered::Answers(job, answers) => HostEvent::AnswersRendered {
+            Rendered::Answers(job, answers, external) => HostEvent::AnswersRendered {
                 gen: job.gen,
                 id: job.id,
                 answers,
+                external,
             },
         };
         let _ = rendered.blocking_send(event);
     })
     .ok();
 
+    // External extensions' programs: started on first use, stopped on quit.
+    let (notice_tx, mut notice_rx) = mpsc::unbounded_channel();
+    let ext_host = extensions::host::ExtensionHost::new(&app.extensions, notice_tx);
+    let notices = ev_tx.clone();
+    tokio::spawn(async move {
+        while let Some(text) = notice_rx.recv().await {
+            if notices.send(HostEvent::ExtensionNotice(text)).await.is_err() {
+                break;
+            }
+        }
+    });
+
     let (reply_tx, mut reply_rx) = mpsc::unbounded_channel::<(ReplyTo, io::Result<Msg>)>();
     let mut events = EventStream::new();
     let mut tick = tokio::time::interval(TICK);
@@ -880,9 +961,11 @@ async fn event_loop(
 
     loop {
         for effect in app.take_effects() {
-            run_effect(effect, &mut app, conns, &reply_tx, &ev_tx, latex_jobs.as_ref());
+            let workers = (latex_jobs.as_ref(), &ext_host);
+            run_effect(effect, &mut app, conns, &reply_tx, &ev_tx, workers);
         }
         if app.quit {
+            ext_host.shutdown().await;
             return Ok(std::mem::take(&mut app.exit));
         }
         let wait = FRAME.saturating_sub(last_draw.elapsed());
@@ -1029,9 +1112,29 @@ async fn event_loop(
                 Some(HostEvent::LatexRendered { host, id, seq, formulas }) => {
                     app.on_latex_rendered(&host, id, seq, formulas);
                 }
-                Some(HostEvent::AnswersRendered { gen, id, answers }) => {
+                Some(HostEvent::AnswersRendered { gen, id, answers, external }) => {
                     app.on_answers_rendered(gen, id, answers);
+                    if !external.is_empty() {
+                        let requests = external
+                            .iter()
+                            .map(|w| (w.lens.clone(), w.kind, w.source.clone()))
+                            .collect();
+                        let all = render_all(&ext_host, requests);
+                        let tx = ev_tx.clone();
+                        tokio::spawn(async move {
+                            let rendered = external.into_iter().zip(all.await).collect();
+                            let event = HostEvent::ReaderExternalRendered { gen, id, rendered };
+                            let _ = tx.send(event).await;
+                        });
+                    }
+                }
+                Some(HostEvent::ExternalRendered { host, id, seq, items }) => {
+                    app.on_external_rendered(&host, id, seq, items);
                 }
+                Some(HostEvent::ReaderExternalRendered { gen, id, rendered }) => {
+                    app.on_reader_external(gen, id, rendered);
+                }
+                Some(HostEvent::ExtensionNotice(text)) => app.notify(text),
                 None => return Ok(Default::default()),
             },
             Some((to, reply)) = reply_rx.recv() => app.on_reply(to, reply),
@@ -1068,7 +1171,10 @@ fn run_effect(
     conns: &mut Connections,
     reply_tx: &mpsc::UnboundedSender<(ReplyTo, io::Result<Msg>)>,
     ev_tx: &mpsc::Sender<HostEvent>,
-    latex_jobs: Option<&std::sync::mpsc::Sender<RenderJob>>,
+    (latex_jobs, ext_host): (
+        Option<&std::sync::mpsc::Sender<RenderJob>>,
+        &extensions::host::ExtensionHost,
+    ),
 ) {
     match effect {
         Effect::Request { host, msg, to } => {
@@ -1127,15 +1233,47 @@ fn run_effect(
                 let _ = jobs.send(RenderJob::Latex(LatexJob { host, id, seq, srcs }));
             }
         }
+        Effect::RenderExternal {
+            host,
+            id,
+            seq,
+            items,
+        } => {
+            let requests = items
+                .iter()
+                .map(|i| (i.tag.clone(), crate::proto::LensKind::Tag, i.text.clone()))
+                .collect();
+            let all = render_all(ext_host, requests);
+            let tx = ev_tx.clone();
+            tokio::spawn(async move {
+                let items = items.into_iter().zip(all.await).collect();
+                let _ = tx
+                    .send(HostEvent::ExternalRendered {
+                        host,
+                        id,
+                        seq,
+                        items,
+                    })
+                    .await;
+            });
+        }
         Effect::RenderAnswers { gen, id, answers } => {
-            let tags = app.extensions.tag_lenses().iter().map(|t| t.to_string()).collect();
-            let fences = app.extensions.fence_lenses().iter().map(|f| f.to_string()).collect();
+            let names = |list: Vec<&str>| list.iter().map(|t| t.to_string()).collect::<Vec<_>>();
+            let tags = names(app.extensions.tag_lenses());
+            let fences = names(app.extensions.fence_lenses());
+            let external = tags
+                .iter()
+                .chain(&fences)
+                .filter(|l| app.extensions.external_lens(l).is_some())
+                .cloned()
+                .collect();
             let job = RenderJob::Answers(AnswersJob {
                 gen,
                 id,
                 answers,
                 tags,
                 fences,
+                external,
             });
             if latex_jobs.is_none_or(|jobs| jobs.send(job).is_err()) {
                 app.on_answers_failed(gen, id, "the render thread is not running");
diff --git a/src/tui/reader/mod.rs b/src/tui/reader/mod.rs
index d08e830..3ae1d27 100644
--- a/src/tui/reader/mod.rs
+++ b/src/tui/reader/mod.rs
@@ -18,8 +18,9 @@ use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
 use ratatui::style::{Modifier, Style};
 use unicode_width::UnicodeWidthStr;
 
+use super::extensions::protocol::Rendering;
 use super::formulas;
-use super::latex::Formula;
+use super::latex::{Formula, Rendered};
 use super::sessions::SessionView;
 use crate::proto::SessionId;
 use layout::{Line, Seg};
@@ -43,6 +44,26 @@ pub struct RenderedAnswer {
     pub formulas: Vec<Formula>,
 }
 
+/// Where an external lens's rendering goes in an answer.
+#[derive(Debug, Clone, PartialEq)]
+pub enum Slot {
+    /// Formula `i` (a tag lens's item).
+    Formula(usize),
+    /// The `n`th fence-lens block, in `Doc::each_lens` order.
+    Fence(usize),
+}
+
+/// An external lens's item or block in the answers: the extension host
+/// renders it after the render worker, into `slot` of answer `answer`.
+#[derive(Debug, Clone, PartialEq)]
+pub struct External {
+    pub answer: usize,
+    pub slot: Slot,
+    pub lens: String,
+    pub kind: crate::proto::LensKind,
+    pub source: String,
+}
+
 /// What the view has to show.
 #[derive(Debug, Clone, PartialEq)]
 pub enum Content {
@@ -99,6 +120,52 @@ impl ReaderView {
         }
     }
 
+    /// Put an external lens's rendering (from the extension host) at
+    /// `slot` of answer `answer`, whose source is `src`: a formula's
+    /// renderings, or a fence block's rows. `None` leaves it as it is.
+    pub fn set_external(
+        &mut self,
+        answer: usize,
+        slot: &Slot,
+        src: &str,
+        rendering: Option<Rendering>,
+    ) {
+        let Some((inline, block)) = rendering else {
+            return;
+        };
+        let Content::Ready(answers) = &mut self.content else {
+            return;
+        };
+        let Some(a) = answers.get_mut(answer) else {
+            return;
+        };
+        match slot {
+            Slot::Formula(i) => {
+                if let Some(f) = a.formulas.get_mut(*i) {
+                    let text = None;
+                    *f = Formula::new(
+                        src,
+                        Rendered {
+                            inline,
+                            block,
+                            text,
+                        },
+                    );
+                }
+            }
+            Slot::Fence(n) => {
+                let mut k = 0;
+                a.doc.each_lens(&mut |_, _, rows| {
+                    if k == *n {
+                        *rows = block.as_ref().map(|b| b.rows.clone());
+                    }
+                    k += 1;
+                });
+            }
+        }
+        self.layouts.borrow_mut().1.remove(&answer);
+    }
+
     /// Show why there are no answers.
     pub fn set_failed(&mut self, why: impl Into<String>) {
         self.content = Content::Failed(why.into());
`````
Run: `umask 022; cargo test --locked --bin claudio`
Expected: `test result: ok. 966 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` (about 18 s: the hung-program test waits out the 15 s action timeout; 15 dead-code warnings until Task 6)
Also: `rustfmt --edition 2021 --check src/tui/extensions/mod.rs src/tui/reader/mod.rs` prints nothing.

Step 3: commit
`/usr/bin/git add src/tui/extensions/host.rs src/tui/extensions/protocol.rs && /usr/bin/git commit -m "feat(tui): the extension host for external programs" -- src/tui/extensions/host.rs src/tui/extensions/protocol.rs src/tui/extensions/mod.rs src/tui/app.rs src/tui/interaction.rs src/tui/mod.rs src/tui/reader/mod.rs`

Interfaces this task provides to later tasks: `extensions::{ActionResult, ListItem}`, `extensions::protocol::{Rendering, SessionContext, action_result, rendering}`, `extensions::host::{ExtensionHost::{new, render, action, shutdown}, Failure, RENDER_TIMEOUT, ACTION_TIMEOUT, MAX_IN_FLIGHT}`, `Effect::RenderExternal`, `HostEvent::{ExternalRendered, ReaderExternalRendered, ExtensionNotice}`, `App::{on_external_rendered, on_reader_external}`, `reader::{Slot, External}`, `ReaderView::set_external`, `render_all(host, requests)` in `tui/mod.rs`.

### Task 6: the leader key and action UI
Model: sonnet   Repo: <repo>   Files: src/tui/leader.rs (new), src/tui/ui/leader.rs (new), src/tui/app.rs, src/tui/interaction.rs, src/tui/keymap.rs, src/tui/mod.rs, src/tui/plain.rs, src/tui/reader/mod.rs, src/tui/ui/mod.rs, tests/manager.rs
Goal: Alt+z lists and runs actions; results show as notices, lists, documents or pastes (AC6, AC7's UI, AC1's permitted plain key-set edits).

What this task does:
- `Action::Extensions` (config name `extensions`, Alt+z, global), in `PLAIN_ACTIONS` too (6 entries). The plain key-set assertions grow by Alt+z: `keymap::tests::plain_help_lists_exactly_the_plain_keys_in_order`, `plain::tests::help_popup_lists_only_the_plain_keys`, and the manager test `test_plain_help_lists_only_the_plain_keys` (its count goes from 6 to 8: the binding and the leader's `Alt+z then:` line).
- `tui/leader.rs` (pure): `Modal::{Leader, ActionInput(Box<Input>), Picker(Box<Picker>)}`; `App::leader_entries` (`u` reader view first, then each enabled extension's actions; search not offered in plain mode); `leader_key` (a listed key runs, any other key or Esc closes; `u` needs a session: "no active session"); `Input` (typing, Backspace, Ctrl+U, ←/→, Home/End, Enter, Esc; ≤ 512 bytes; a terminal paste goes in too); `Picker` (case-insensitive filter on label and detail, ↑/↓/PgUp/PgDn/Home/End, wheel, Enter applies, Esc closes); `start_action` (an external action → `Effect::RunAction { gen, … }` with the active session's `SessionContext`; `pending_action = Some(gen)`); `on_action_result` (dropped unless it is the pending action and no modal is open); `apply_result`; `open_document` (a `ReaderView::document` rendered by the worker like an answer, its lenses included); `paste` (sanitized, ≤ 64 KiB, only into the active, attached claude session with bracketed paste on, as `ESC[200~…ESC[201~` and never Enter; otherwise the spec's notices).
- `tui/ui/leader.rs`: the popup (`key  title  (extension)`), the input line and the picker drawing (label, dimmed detail cut to the width). The help popup lists `Alt+z then:` and the leader's lines.
- `tui/mod.rs`: `Effect::RunAction` → `ExtensionHost::action` → `HostEvent::ActionDone` → `App::on_action_result`.
- Built-in actions are routed to `builtin_action`, which says "<ext>: <action> is not available" until Task 11.

Step 1: write the failing tests
`````diff
diff --git a/src/tui/keymap.rs b/src/tui/keymap.rs
index 5d6f652..850d3e7 100644
--- a/src/tui/keymap.rs
+++ b/src/tui/keymap.rs
@@ -944,7 +958,10 @@ mod tests {
     fn plain_help_lists_exactly_the_plain_keys_in_order() {
         let (km, _) = plain_keymap(&[]);
         let keys: Vec<String> = km.help_entries().into_iter().map(|(k, _)| k).collect();
-        assert_eq!(keys, ["Alt+h", "Alt+s", "Alt+s → n", "Alt+l", "Alt+e", "Alt+u"]);
+        assert_eq!(
+            keys,
+            ["Alt+h", "Alt+s", "Alt+s → n", "Alt+l", "Alt+e", "Alt+u", "Alt+z"]
+        );
     }
 
     #[test]
diff --git a/src/tui/leader.rs b/src/tui/leader.rs
new file mode 100644
index 0000000..c7879b8
--- /dev/null
+++ b/src/tui/leader.rs
@@ -0,0 +1,251 @@
+#[cfg(test)]
+mod tests {
+    use super::*;
+    use crate::tui::extensions::{manifest::Program, ActionDecl, Extension, Registry};
+    use crate::tui::test_support::{alt, app_with, info, press};
+
+    fn key(c: char) -> crossterm::event::Event {
+        press(KeyCode::Char(c))
+    }
+
+    /// An app with an external extension `fake` whose action `k` runs `go`
+    /// (asking for input when `input`).
+    fn app_with_fake(input: Option<&str>) -> App {
+        let mut app = app_with(&[info(Some(1), Some("conv"))]);
+        let fake = Extension {
+            name: "fake".into(),
+            description: String::new(),
+            enabled: true,
+            lenses: vec![],
+            actions: vec![ActionDecl {
+                key: 'k',
+                title: "do it".into(),
+                id: "go".into(),
+                input: input.map(str::to_owned),
+            }],
+            source: Source::External(Program {
+                dir: "/x".into(),
+                command: vec!["p".into()],
+            }),
+        };
+        app.extensions = Registry::new(&Default::default(), true, vec![fake], &mut Vec::new());
+        app.activate(0);
+        app.take_effects();
+        app
+    }
+
+    fn runs(app: &mut App) -> Vec<(u64, String, Option<String>, Option<String>)> {
+        app.take_effects()
+            .into_iter()
+            .filter_map(|e| match e {
+                Effect::RunAction {
+                    gen,
+                    action,
+                    input,
+                    session,
+                    ..
+                } => Some((gen, action, input, session.and_then(|s| s.conversation))),
+                _ => None,
+            })
+            .collect()
+    }
+
+    #[test]
+    fn the_leader_lists_the_reader_and_every_action() {
+        let app = app_with_fake(None);
+        let entries: Vec<(char, Option<String>)> = app
+            .leader_entries()
+            .into_iter()
+            .map(|e| (e.key, e.extension))
+            .collect();
+        let ext = |n: &str| Some(n.to_owned());
+        assert_eq!(
+            entries,
+            [
+                ('u', None),
+                ('s', ext("search")),
+                ('p', ext("snippets")),
+                ('P', ext("snippets")),
+                ('k', ext("fake")),
+            ]
+        );
+    }
+
+    #[test]
+    fn alt_z_then_a_key_runs_its_action_and_other_keys_just_close() {
+        let mut app = app_with_fake(None);
+        app.on_terminal(alt('z'));
+        assert!(matches!(app.modal, Some(Modal::Leader)));
+        // Esc and an unknown key close it; nothing reaches the session.
+        app.on_terminal(press(KeyCode::Esc));
+        assert!(app.modal.is_none());
+        app.on_terminal(alt('z'));
+        app.on_terminal(key('q'));
+        assert!(app.modal.is_none());
+        assert!(!app
+            .take_effects()
+            .iter()
+            .any(|e| matches!(e, Effect::Input(..))));
+        // Alt+z again closes it too.
+        app.on_terminal(alt('z'));
+        app.on_terminal(alt('z'));
+        assert!(app.modal.is_none());
+        app.on_terminal(alt('z'));
+        app.on_terminal(key('k'));
+        let ran = runs(&mut app);
+        assert_eq!(ran.len(), 1);
+        assert_eq!((ran[0].1.as_str(), ran[0].2.as_deref()), ("go", None));
+        assert_eq!(ran[0].3.as_deref(), Some("conv"), "the session goes along");
+        // Alt+z u is the reader.
+        app.on_terminal(alt('z'));
+        app.on_terminal(key('u'));
+        assert!(matches!(app.modal, Some(Modal::Reader(_))));
+    }
+
+    #[test]
+    fn an_action_with_input_asks_for_a_line_first() {
+        let mut app = app_with_fake(Some("label"));
+        app.on_terminal(alt('z'));
+        app.on_terminal(key('k'));
+        assert!(matches!(app.modal, Some(Modal::ActionInput(_))));
+        for c in "héllo".chars() {
+            app.on_terminal(key(c));
+        }
+        app.on_terminal(press(KeyCode::Left));
+        app.on_terminal(press(KeyCode::Backspace));
+        app.on_terminal(press(KeyCode::Home));
+        app.on_terminal(key('>'));
+        app.on_terminal(press(KeyCode::End));
+        app.on_terminal(key('!'));
+        assert!(runs(&mut app).is_empty(), "nothing runs while typing");
+        app.on_terminal(press(KeyCode::Enter));
+        let ran = runs(&mut app);
+        assert_eq!(ran[0].2.as_deref(), Some(">hélo!"));
+        // Ctrl+U clears; Esc cancels.
+        app.on_terminal(alt('z'));
+        app.on_terminal(key('k'));
+        app.on_terminal(key('x'));
+        let ctrl_u = KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL);
+        app.on_terminal(crossterm::event::Event::Key(ctrl_u));
+        let Some(Modal::ActionInput(input)) = &app.modal else {
+            panic!("no input")
+        };
+        assert!(input.text.is_empty());
+        app.on_terminal(press(KeyCode::Esc));
+        assert!(app.modal.is_none() && runs(&mut app).is_empty());
+    }
+
+    #[test]
+    fn results_show_or_paste_and_stale_ones_are_dropped() {
+        let mut app = app_with_fake(None);
+        app.on_terminal(alt('z'));
+        app.on_terminal(key('k'));
+        let first = runs(&mut app)[0].0;
+        app.on_terminal(alt('z'));
+        app.on_terminal(key('k'));
+        let second = runs(&mut app)[0].0;
+        // The first answer comes late: dropped.
+        app.on_action_result(
+            first,
+            Ok(ActionResult::Notice {
+                text: "late".into(),
+            }),
+        );
+        assert!(app.notice.is_none());
+        app.on_action_result(second, Err("extension fake timed out".into()));
+        assert_eq!(
+            app.notice.as_ref().unwrap().text,
+            "extension fake timed out"
+        );
+
+        // A list: filter, move, Enter applies the item's result.
+        app.on_terminal(alt('z'));
+        app.on_terminal(key('k'));
+        let gen = runs(&mut app)[0].0;
+        let item = |label: &str, text: &str| ListItem {
+            label: label.into(),
+            detail: Some(format!("about {label}")),
+            result: ActionResult::Notice { text: text.into() },
+        };
+        let items = vec![item("alpha", "A"), item("beta", "B"), item("gamma", "G")];
+        let list = ActionResult::List {
+            title: "Pick".into(),
+            items,
+        };
+        app.on_action_result(gen, Ok(list));
+        let Some(Modal::Picker(picker)) = &app.modal else {
+            panic!("no picker")
+        };
+        assert_eq!(picker.shown(), [0, 1, 2]);
+        app.on_terminal(key('A'));
+        let Some(Modal::Picker(picker)) = &app.modal else {
+            panic!("no picker")
+        };
+        assert_eq!(picker.shown(), [0, 1, 2], "every label or detail has an a");
+        app.on_terminal(press(KeyCode::Backspace));
+        for c in "mm".chars() {
+            app.on_terminal(key(c));
+        }
+        app.on_terminal(press(KeyCode::Enter));
+        assert!(app.modal.is_none());
+        assert_eq!(app.notice.as_ref().unwrap().text, "G");
+
+        // A document opens a reader view on the worker.
+        app.on_terminal(alt('z'));
+        app.on_terminal(key('k'));
+        let gen = runs(&mut app)[0].0;
+        let doc = ActionResult::Document {
+            title: "Doc".into(),
+            markdown: "# hi".into(),
+        };
+        app.on_action_result(gen, Ok(doc));
+        assert!(matches!(app.modal, Some(Modal::Reader(_))));
+        assert!(app.take_effects().iter().any(
+            |e| matches!(e, Effect::RenderAnswers { answers, .. } if answers[0].text == "# hi")
+        ));
+    }
+
+    #[test]
+    fn a_paste_needs_an_attached_claude_with_bracketed_paste() {
+        let mut app = app_with_fake(None);
+        app.sessions[0].attached = true;
+        app.paste("hi\nthere\u{1b}[201~");
+        assert_eq!(
+            app.notice.as_ref().unwrap().text,
+            "paste needs claude's prompt (bracketed paste is off)"
+        );
+        assert!(app.take_effects().is_empty());
+        app.sessions[0].mirror.feed(b"\x1b[?2004h");
+        app.paste("hi\nthere\u{1b}[201~");
+        let inputs: Vec<Vec<u8>> = app
+            .take_effects()
+            .into_iter()
+            .filter_map(|e| match e {
+                Effect::Input(_, bytes) => Some(bytes),
+                _ => None,
+            })
+            .collect();
+        assert_eq!(inputs, [b"\x1b[200~hi\nthere[201~\x1b[201~".to_vec()]);
+        app.sessions[0].kind = SessionKind::Shell;
+        app.paste("x");
+        assert_eq!(
+            app.notice.as_ref().unwrap().text,
+            "paste goes to claude sessions only"
+        );
+        app.sessions.clear();
+        app.active = None;
+        app.paste("x");
+        assert_eq!(app.notice.as_ref().unwrap().text, "no active session");
+    }
+
+    #[test]
+    fn plain_mode_offers_no_search() {
+        use crate::tui::app::{AppConfig, Mode};
+        let app = App::new(AppConfig {
+            mode: Mode::Plain,
+            ..crate::tui::test_support::config()
+        });
+        let keys: Vec<char> = app.leader_entries().iter().map(|e| e.key).collect();
+        assert_eq!(keys, ['u', 'p', 'P']);
+    }
+}
diff --git a/src/tui/plain.rs b/src/tui/plain.rs
index 6f5765a..eb4ad2c 100644
--- a/src/tui/plain.rs
+++ b/src/tui/plain.rs
@@ -223,7 +223,7 @@ mod tests {
         app.take_effects();
         app.on_terminal(alt('h'));
         let text = screen(&app).join("\n");
-        for key in ["Alt+h", "Alt+s", "Alt+l", "Alt+e", "Alt+u"] {
+        for key in ["Alt+h", "Alt+s", "Alt+l", "Alt+e", "Alt+u", "Alt+z"] {
             assert!(text.contains(key), "{key} missing:\n{text}");
         }
         for gone in ["Alt+n", "Alt+x", "Alt+q", "Alt+g", "Alt+c", "Alt+Shift"] {
diff --git a/tests/manager.rs b/tests/manager.rs
index 6340333..46c0a92 100644
--- a/tests/manager.rs
+++ b/tests/manager.rs
@@ -1800,8 +1800,8 @@ fn test_plain_is_the_bare_session_full_screen() {
     drop(tui);
 }
 
-/// Alt+h lists exactly the five plain keys (and the stats popup's `n`); any
-/// key closes it.
+/// Alt+h lists exactly the six plain keys (and the stats popup's `n`, and
+/// the leader's line); any key closes it.
 #[test]
 fn test_plain_help_lists_only_the_plain_keys() {
     let harness = ManagerHarness::new();
@@ -1810,10 +1810,10 @@ fn test_plain_help_lists_only_the_plain_keys() {
     tui.send_keys(ALT_H);
     tui.wait_for("claudio keys", Region::Screen, WAIT);
     let screen = tui.screen_text(Region::Screen);
-    for key in ["Alt+h", "Alt+s", "Alt+s → n", "Alt+l", "Alt+e", "Alt+u"] {
+    for key in ["Alt+h", "Alt+s", "Alt+s → n", "Alt+l", "Alt+e", "Alt+u", "Alt+z", "Alt+z then:"] {
         assert!(screen.contains(key), "{key} missing:\n{screen}");
     }
-    assert_eq!(screen.matches("Alt+").count(), 6, "exactly the plain keys:\n{screen}");
+    assert_eq!(screen.matches("Alt+").count(), 8, "exactly the plain keys:\n{screen}");
 
     tui.send_keys(ESC);
     tui.wait_until(|s| !s.contains("claudio keys", Region::Screen), WAIT);
`````
Run: `umask 022; cargo test --locked --bin claudio 2>&1 | grep -E 'FAILED|^test result'`
Expected: `test tui::keymap::tests::plain_help_lists_exactly_the_plain_keys_in_order ... FAILED`, `test tui::plain::tests::help_popup_lists_only_the_plain_keys ... FAILED` and `test result: FAILED. 964 passed; 2 failed` (the new `leader.rs` tests compile only once Step 2 declares the module)

Step 2: implement
`````diff
diff --git a/src/tui/app.rs b/src/tui/app.rs
index 48f28f4..0bb5e05 100644
--- a/src/tui/app.rs
+++ b/src/tui/app.rs
@@ -161,6 +161,15 @@ pub enum Effect {
         seq: u64,
         items: Vec<crate::proto::LensItem>,
     },
+    /// Run an external extension's action in the extension host; the
+    /// result goes to [`App::on_action_result`] with `gen`.
+    RunAction {
+        gen: u64,
+        extension: String,
+        action: String,
+        input: Option<String>,
+        session: Option<super::extensions::protocol::SessionContext>,
+    },
     /// Render the answers of session `id` for the reader view opened under
     /// `gen`, on the `latex-render` worker; they come back through
     /// [`App::on_answers_rendered`].
@@ -316,6 +325,8 @@ pub struct App {
     /// taken and their `Latex` events ignored (an older daemon's `Latex`
     /// events are taken as `latex` items instead).
     pub(super) lens_hosts: HashSet<String>,
+    /// The `modal_gen` of the external action whose result is awaited.
+    pub(super) pending_action: Option<u64>,
 }
 
 /// What the app starts from. The event loop gathers it (reading config.toml,
@@ -406,6 +417,7 @@ impl App {
                 &mut Vec::new(),
             ),
             lens_hosts: HashSet::new(),
+            pending_action: None,
         }
     }
 
diff --git a/src/tui/interaction.rs b/src/tui/interaction.rs
index fcf9b4f..8a86b61 100644
--- a/src/tui/interaction.rs
+++ b/src/tui/interaction.rs
@@ -21,6 +21,7 @@ use super::app::{App, Effect, Mode, ReplyTo};
 use super::confirm::ConfirmPrompt;
 use super::git_view::GitView;
 use super::keymap::Action;
+use super::leader::{Input, Picker};
 use super::extensions::protocol::Rendering;
 use super::latex::{Formula, Rendered};
 use super::reader::{External, ReaderView, RenderedAnswer};
@@ -59,6 +60,12 @@ pub enum Modal {
     /// The reader view of a session's answers (full pane, see
     /// `reader/`). Boxed like the history viewer.
     Reader(Box<ReaderView>),
+    /// The leader popup (Alt+z): the extension actions' keys.
+    Leader,
+    /// The line of input an extension action asks for (see `leader.rs`).
+    ActionInput(Box<Input>),
+    /// A list an extension action gave, to pick from.
+    Picker(Box<Picker>),
 }
 
 impl Modal {
@@ -72,6 +79,8 @@ impl Modal {
                 | Modal::Help
                 | Modal::ProxyStats { .. }
                 | Modal::Reader(_)
+                | Modal::Leader
+                | Modal::Picker(_)
         )
     }
 
@@ -85,12 +94,13 @@ impl Modal {
                 | (Modal::Help, Action::Help)
                 | (Modal::ProxyStats { .. }, Action::ProxyStats)
                 | (Modal::Reader(_), Action::Formulas)
+                | (Modal::Leader, Action::Extensions)
         )
     }
 
     /// Whether the modal takes mouse wheel and clicks.
     fn wants_mouse(&self) -> bool {
-        matches!(self, Modal::Git(_) | Modal::Reader(_))
+        matches!(self, Modal::Git(_) | Modal::Reader(_) | Modal::Picker(_))
     }
 }
 
@@ -194,6 +204,7 @@ impl App {
             Action::Help => self.open_help(),
             Action::GitLog => self.open_git_log(),
             Action::Formulas => self.open_reader(),
+            Action::Extensions => self.open_leader(),
             _ => {}
         }
         self.redraw = true;
@@ -213,7 +224,7 @@ impl App {
     /// The reader view of the active session (none without one): it asks
     /// the session's daemon for the answers, and shows "Loading…" until
     /// they are rendered.
-    fn open_reader(&mut self) {
+    pub(super) fn open_reader(&mut self) {
         let Some((id, host)) = self.active_view().map(|v| (v.id, v.host.clone())) else {
             return;
         };
@@ -376,6 +387,9 @@ impl App {
                     self.modal = None;
                 }
             }
+            Modal::Leader => self.leader_key(&key),
+            Modal::ActionInput(_) => self.action_input_key(&key),
+            Modal::Picker(_) => self.picker_key(&key),
         }
     }
 
@@ -391,6 +405,15 @@ impl App {
             self.redraw = true;
             return;
         }
+        if let Some(Modal::Picker(picker)) = &mut self.modal {
+            match m.kind {
+                MouseEventKind::ScrollDown => picker.on_wheel(true),
+                MouseEventKind::ScrollUp => picker.on_wheel(false),
+                _ => return,
+            }
+            self.redraw = true;
+            return;
+        }
         let Some(Modal::Git(view)) = &mut self.modal else { return };
         // Pane rows start below the tab bar; the status bar is not the pane.
         let above = self.mode.rows_above() as usize;
@@ -421,8 +444,21 @@ impl App {
                 | Modal::Overview { .. }
                 | Modal::Help
                 | Modal::Confirm(_)
-                | Modal::Reader(_),
+                | Modal::Reader(_)
+                | Modal::Leader
+                | Modal::Picker(_),
             ) => {}
+            Some(Modal::ActionInput(input)) => {
+                let line = text.lines().next().unwrap_or("");
+                for c in line.chars().filter(|c| !c.is_control()) {
+                    if input.text.len() + c.len_utf8() > super::leader::INPUT_MAX {
+                        break;
+                    }
+                    input.text.insert(input.cursor, c);
+                    input.cursor += c.len_utf8();
+                }
+                self.redraw = true;
+            }
             None => {
                 if let Some(v) = self.active_view().filter(|v| v.attached) {
                     let bytes = encode_paste(text, &v.mirror.modes());
diff --git a/src/tui/keymap.rs b/src/tui/keymap.rs
index c62b3b6..850d3e7 100644
--- a/src/tui/keymap.rs
+++ b/src/tui/keymap.rs
@@ -41,6 +41,8 @@ pub enum Action {
     /// name stays `formulas` (the v0.4.0 popup it replaced), so existing
     /// `[keys]` keep working.
     Formulas,
+    /// Open the leader popup: the extensions' actions, one key each.
+    Extensions,
     /// Show/hide dot-directories in the new-session wizard (wizard scope).
     ToggleHidden,
     /// Jump to tab `n` (`Alt+Shift+1`…`9` → 1…9, `Alt+Shift+0` → 10).
@@ -66,6 +68,7 @@ impl Action {
             Action::GitLog => "git_log",
             Action::Reset => "reset",
             Action::Formulas => "formulas",
+            Action::Extensions => "extensions",
             Action::ToggleHidden => "toggle_hidden",
             Action::GotoSession(_) => "goto_session",
         }
@@ -210,6 +213,16 @@ pub const DEFAULT_BINDINGS: &[Binding] = &[
         action: Action::Formulas,
         scope: Scope::Global,
     },
+    // Alt+z: the extensions' leader key (Alt+p is Claude Code's model
+    // picker). Verified free in Claude Code 2.1.312 (no alt+z/meta+z in
+    // its bundle).
+    Binding {
+        code: KeyCode::Char('z'),
+        mods: KeyModifiers::ALT,
+        label: "extension actions (then a key)",
+        action: Action::Extensions,
+        scope: Scope::Global,
+    },
     // Quit is last so it's always visible in the status bar even when truncated.
     Binding {
         code: KeyCode::Char('q'),
@@ -245,12 +258,13 @@ fn goto_session(key: &KeyEvent) -> Option<Action> {
 
 /// The only actions `claudio --plain` keeps, in help order. Everything else
 /// (tabs, wizard, rename, quit, ...) belongs to the manager and goes to claude.
-pub const PLAIN_ACTIONS: [Action; 5] = [
+pub const PLAIN_ACTIONS: [Action; 6] = [
     Action::Help,
     Action::ProxyStats,
     Action::GitLog,
     Action::Reset,
     Action::Formulas,
+    Action::Extensions,
 ];
 
 /// The effective keymap: defaults + config overrides.
diff --git a/src/tui/leader.rs b/src/tui/leader.rs
index 02c0d77..c7879b8 100644
--- a/src/tui/leader.rs
+++ b/src/tui/leader.rs
@@ -1,3 +1,394 @@
+//! The leader key (Alt+z) and what extension actions show: the leader
+//! popup, the one-line input an action may ask for, the list picker, the
+//! document (a reader view of one markdown text) and pasting into the
+//! active session. Pure, like `app.rs`: external actions run in the
+//! extension host through [`Effect::RunAction`], their results come back
+//! through [`App::on_action_result`].
+
+use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
+
+use super::app::{App, Effect};
+use super::extensions::protocol::SessionContext;
+use super::extensions::{ActionResult, ListItem, Source, READER_KEY};
+use super::interaction::Modal;
+use super::reader::ReaderView;
+use crate::claude::latex::sanitize;
+use crate::proto::{Answer, SessionId, SessionKind};
+
+/// The longest input an action gets, in bytes.
+pub const INPUT_MAX: usize = 512;
+/// The most text a paste writes, in bytes.
+pub const PASTE_MAX: usize = 64 * 1024;
+/// How far PageUp/PageDown move in the picker.
+const PAGE: usize = 10;
+
+/// One line of the leader popup.
+#[derive(Debug, Clone, PartialEq)]
+pub struct LeaderEntry {
+    pub key: char,
+    pub title: String,
+    /// The extension's name (`None`: the reader, which is not one).
+    pub extension: Option<String>,
+}
+
+/// The one-line input an action asks for before it runs.
+#[derive(Debug, Clone, PartialEq)]
+pub struct Input {
+    pub extension: String,
+    pub action: String,
+    pub prompt: String,
+    pub text: String,
+    /// The cursor, as a byte offset into `text` (on a char boundary).
+    pub cursor: usize,
+}
+
+/// What a key did to an [`Input`].
+enum Typed {
+    Editing,
+    Done,
+    Cancelled,
+}
+
+impl Input {
+    fn on_key(&mut self, key: &KeyEvent) -> Typed {
+        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
+        match key.code {
+            KeyCode::Esc => return Typed::Cancelled,
+            KeyCode::Enter => return Typed::Done,
+            KeyCode::Char('u') if ctrl => {
+                self.text.clear();
+                self.cursor = 0;
+            }
+            KeyCode::Char(c) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => {
+                if self.text.len() + c.len_utf8() <= INPUT_MAX && !c.is_control() {
+                    self.text.insert(self.cursor, c);
+                    self.cursor += c.len_utf8();
+                }
+            }
+            KeyCode::Backspace => {
+                if let Some(c) = self.text[..self.cursor].chars().next_back() {
+                    self.cursor -= c.len_utf8();
+                    self.text.remove(self.cursor);
+                }
+            }
+            KeyCode::Left => {
+                if let Some(c) = self.text[..self.cursor].chars().next_back() {
+                    self.cursor -= c.len_utf8();
+                }
+            }
+            KeyCode::Right => {
+                if let Some(c) = self.text[self.cursor..].chars().next() {
+                    self.cursor += c.len_utf8();
+                }
+            }
+            KeyCode::Home => self.cursor = 0,
+            KeyCode::End => self.cursor = self.text.len(),
+            _ => {}
+        }
+        Typed::Editing
+    }
+}
+
+/// A list to pick from, with an always-on filter.
+#[derive(Debug, Clone, PartialEq)]
+pub struct Picker {
+    pub title: String,
+    pub items: Vec<ListItem>,
+    pub filter: String,
+    /// An index into [`Picker::shown`].
+    pub selected: usize,
+}
+
+/// What a key did to a [`Picker`].
+enum Picked {
+    Browsing,
+    Chose(ActionResult),
+    Closed,
+}
+
+impl Picker {
+    pub fn new(title: String, items: Vec<ListItem>) -> Picker {
+        Picker {
+            title,
+            items,
+            filter: String::new(),
+            selected: 0,
+        }
+    }
+
+    /// The items the filter keeps (a case-insensitive substring of the
+    /// label or the detail), by index.
+    pub fn shown(&self) -> Vec<usize> {
+        let filter = self.filter.to_lowercase();
+        (0..self.items.len())
+            .filter(|&i| {
+                let item = &self.items[i];
+                item.label.to_lowercase().contains(&filter)
+                    || item
+                        .detail
+                        .as_ref()
+                        .is_some_and(|d| d.to_lowercase().contains(&filter))
+            })
+            .collect()
+    }
+
+    fn on_key(&mut self, key: &KeyEvent) -> Picked {
+        let shown = self.shown();
+        let last = shown.len().saturating_sub(1);
+        match key.code {
+            KeyCode::Esc => return Picked::Closed,
+            KeyCode::Enter => {
+                return match shown.get(self.selected) {
+                    Some(&i) => Picked::Chose(self.items[i].result.clone()),
+                    None => Picked::Browsing,
+                }
+            }
+            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
+            KeyCode::Down => self.selected = (self.selected + 1).min(last),
+            KeyCode::PageUp => self.selected = self.selected.saturating_sub(PAGE),
+            KeyCode::PageDown => self.selected = (self.selected + PAGE).min(last),
+            KeyCode::Home => self.selected = 0,
+            KeyCode::End => self.selected = last,
+            KeyCode::Backspace => {
+                self.filter.pop();
+                self.selected = 0;
+            }
+            KeyCode::Char(c)
+                if !key
+                    .modifiers
+                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
+            {
+                if self.filter.len() < INPUT_MAX {
+                    self.filter.push(c);
+                }
+                self.selected = 0;
+            }
+            _ => {}
+        }
+        Picked::Browsing
+    }
+
+    /// The mouse wheel: down (`down`) or up, one item.
+    pub fn on_wheel(&mut self, down: bool) {
+        let last = self.shown().len().saturating_sub(1);
+        self.selected = if down {
+            (self.selected + 1).min(last)
+        } else {
+            self.selected.saturating_sub(1)
+        };
+    }
+}
+
+impl App {
+    /// Alt+z: the leader popup.
+    pub(super) fn open_leader(&mut self) {
+        self.modal = Some(Modal::Leader);
+        self.redraw = true;
+    }
+
+    /// The leader's lines: the reader, then each enabled extension's
+    /// actions (search is not offered in plain mode).
+    pub fn leader_entries(&self) -> Vec<LeaderEntry> {
+        let mut entries = vec![LeaderEntry {
+            key: READER_KEY,
+            title: "reader view".to_owned(),
+            extension: None,
+        }];
+        let plain = self.mode == super::app::Mode::Plain;
+        for (ext, action) in self.extensions.actions() {
+            if plain && ext.name == "search" {
+                continue;
+            }
+            entries.push(LeaderEntry {
+                key: action.key,
+                title: action.title.clone(),
+                extension: Some(ext.name.clone()),
+            });
+        }
+        entries
+    }
+
+    /// A key in the leader popup: an action's key runs it; anything else
+    /// closes the popup. Nothing reaches the session.
+    pub(super) fn leader_key(&mut self, key: &KeyEvent) {
+        self.modal = None;
+        let KeyCode::Char(c) = key.code else { return };
+        if key
+            .modifiers
+            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
+        {
+            return;
+        }
+        if !self.leader_entries().iter().any(|e| e.key == c) {
+            return;
+        }
+        if c == READER_KEY {
+            if self.active_view().is_none() {
+                self.notify("no active session");
+                return;
+            }
+            return self.open_reader();
+        }
+        let found = self
+            .extensions
+            .actions()
+            .into_iter()
+            .find(|(_, a)| a.key == c);
+        let Some((ext, action)) = found else { return };
+        let (ext, action) = (ext.name.clone(), action.clone());
+        match action.input {
+            Some(prompt) => {
+                self.modal = Some(Modal::ActionInput(Box::new(Input {
+                    extension: ext,
+                    action: action.id,
+                    prompt,
+                    text: String::new(),
+                    cursor: 0,
+                })))
+            }
+            None => self.start_action(&ext, &action.id, None),
+        }
+    }
+
+    /// A key in an action's input line.
+    pub(super) fn action_input_key(&mut self, key: &KeyEvent) {
+        let Some(Modal::ActionInput(input)) = &mut self.modal else {
+            return;
+        };
+        match input.on_key(key) {
+            Typed::Editing => {}
+            Typed::Cancelled => self.modal = None,
+            Typed::Done => {
+                let (ext, action, text) = (
+                    input.extension.clone(),
+                    input.action.clone(),
+                    input.text.clone(),
+                );
+                self.modal = None;
+                self.start_action(&ext, &action, Some(text));
+            }
+        }
+    }
+
+    /// A key in the list picker: Enter applies the chosen item's result.
+    pub(super) fn picker_key(&mut self, key: &KeyEvent) {
+        let Some(Modal::Picker(picker)) = &mut self.modal else {
+            return;
+        };
+        match picker.on_key(key) {
+            Picked::Browsing => {}
+            Picked::Closed => self.modal = None,
+            Picked::Chose(result) => {
+                self.modal = None;
+                self.apply_result(result);
+            }
+        }
+    }
+
+    /// Run action `action` of extension `ext` with `input`: a built-in's
+    /// here, an external one's in the extension host (its result comes
+    /// back through [`App::on_action_result`]).
+    pub(super) fn start_action(&mut self, ext: &str, action: &str, input: Option<String>) {
+        let external = self
+            .extensions
+            .enabled()
+            .any(|e| e.name == ext && matches!(e.source, Source::External(_)));
+        if !external {
+            return self.builtin_action(ext, action, input);
+        }
+        let session = self.active_view().map(|v| SessionContext {
+            host: v.host.clone(),
+            cwd: v.cwd.clone(),
+            name: v.name.clone(),
+            conversation: v.claude_session_id.clone(),
+        });
+        self.modal_gen += 1;
+        let gen = self.modal_gen;
+        self.pending_action = Some(gen);
+        self.emit(Effect::RunAction {
+            gen,
+            extension: ext.to_owned(),
+            action: action.to_owned(),
+            input,
+            session,
+        });
+    }
+
+    /// A built-in extension's action.
+    fn builtin_action(&mut self, ext: &str, action: &str, _input: Option<String>) {
+        self.notify(format!("{ext}: {action} is not available"));
+    }
+
+    /// The result of the external action started under `gen` (or the
+    /// notice saying why there is none). Dropped when another action has
+    /// started since, or a modal is open.
+    pub fn on_action_result(&mut self, gen: u64, result: Result<ActionResult, String>) {
+        if self.pending_action != Some(gen) {
+            return;
+        }
+        self.pending_action = None;
+        if self.modal.is_some() {
+            return;
+        }
+        match result {
+            Ok(result) => self.apply_result(result),
+            Err(notice) => self.notify(notice),
+        }
+        self.redraw = true;
+    }
+
+    /// Show or do what an action gave.
+    pub(super) fn apply_result(&mut self, result: ActionResult) {
+        match result {
+            ActionResult::Notice { text } => self.notify(text),
+            ActionResult::Paste { text } => self.paste(&text),
+            ActionResult::List { title, items } => {
+                self.modal = Some(Modal::Picker(Box::new(Picker::new(title, items))));
+            }
+            ActionResult::Document { title, markdown } => self.open_document(title, markdown),
+        }
+        self.redraw = true;
+    }
+
+    /// `markdown` in a reader view of its own, rendered on the worker like
+    /// an answer.
+    pub(super) fn open_document(&mut self, title: String, markdown: String) {
+        self.modal_gen += 1;
+        let gen = self.modal_gen;
+        let id = SessionId::nil();
+        let view = ReaderView::document(id, title.clone(), gen);
+        self.modal = Some(Modal::Reader(Box::new(view)));
+        let answers = vec![Answer {
+            prompt: title,
+            text: markdown,
+        }];
+        self.emit(Effect::RenderAnswers { gen, id, answers });
+    }
+
+    /// Paste `text` into the active session, as bracketed paste and never
+    /// followed by Enter: only into an attached claude session whose prompt
+    /// has bracketed paste on (so a newline in the text cannot submit it).
+    pub(super) fn paste(&mut self, text: &str) {
+        let text = sanitize(text);
+        if text.len() > PASTE_MAX {
+            return self.notify("result too large");
+        }
+        let Some(v) = self.active_view() else {
+            return self.notify("no active session");
+        };
+        if v.kind != SessionKind::Claude {
+            return self.notify("paste goes to claude sessions only");
+        }
+        let modes = v.mirror.modes();
+        if !v.attached || !modes.bracketed_paste {
+            return self.notify("paste needs claude's prompt (bracketed paste is off)");
+        }
+        let bytes = crate::term::keys::encode_paste(&text, &modes);
+        let id = v.id;
+        self.emit(Effect::Input(id, bytes));
+    }
+}
+
 #[cfg(test)]
 mod tests {
     use super::*;
diff --git a/src/tui/mod.rs b/src/tui/mod.rs
index f7b52df..6c32d66 100644
--- a/src/tui/mod.rs
+++ b/src/tui/mod.rs
@@ -18,6 +18,7 @@ mod git_view;
 mod interaction;
 mod keymap;
 mod latex;
+mod leader;
 mod notifications;
 mod plain;
 mod proxy_state;
@@ -530,6 +531,11 @@ enum HostEvent {
     },
     /// A notice from the extension host (an extension was disabled).
     ExtensionNotice(String),
+    /// An external action started under `gen` ended.
+    ActionDone {
+        gen: u64,
+        result: Result<extensions::ActionResult, String>,
+    },
 }
 
 // ── Formula rendering ─────────────────────────────────────────────────────────
@@ -1135,6 +1141,7 @@ async fn event_loop(
                     app.on_reader_external(gen, id, rendered);
                 }
                 Some(HostEvent::ExtensionNotice(text)) => app.notify(text),
+                Some(HostEvent::ActionDone { gen, result }) => app.on_action_result(gen, result),
                 None => return Ok(Default::default()),
             },
             Some((to, reply)) = reply_rx.recv() => app.on_reply(to, reply),
@@ -1257,6 +1264,20 @@ fn run_effect(
                     .await;
             });
         }
+        Effect::RunAction {
+            gen,
+            extension,
+            action,
+            input,
+            session,
+        } => {
+            let done = ext_host.action(&extension, &action, input, session);
+            let tx = ev_tx.clone();
+            tokio::spawn(async move {
+                let result = done.await;
+                let _ = tx.send(HostEvent::ActionDone { gen, result }).await;
+            });
+        }
         Effect::RenderAnswers { gen, id, answers } => {
             let names = |list: Vec<&str>| list.iter().map(|t| t.to_string()).collect::<Vec<_>>();
             let tags = names(app.extensions.tag_lenses());
diff --git a/src/tui/reader/mod.rs b/src/tui/reader/mod.rs
index 3ae1d27..253776b 100644
--- a/src/tui/reader/mod.rs
+++ b/src/tui/reader/mod.rs
@@ -82,6 +82,9 @@ pub struct ReaderView {
     /// The `App::modal_gen` it was opened under: replies for another are
     /// dropped.
     pub gen: u64,
+    /// A document's title (an extension's result), shown as the header;
+    /// `None` for a session's answers.
+    pub title: Option<String>,
     pub content: Content,
     /// The answer shown (an index into `Ready`).
     answer: usize,
@@ -101,6 +104,7 @@ impl ReaderView {
             session,
             host: host.into(),
             gen,
+            title: None,
             content: Content::Loading,
             answer: 0,
             list: false,
@@ -110,6 +114,15 @@ impl ReaderView {
         }
     }
 
+    /// A view of one document titled `title` (rendered like an answer,
+    /// under `gen`; `session` is the id its rendering is sent with).
+    pub fn document(session: SessionId, title: String, gen: u64) -> ReaderView {
+        ReaderView {
+            title: Some(title),
+            ..ReaderView::new(session, "local", gen)
+        }
+    }
+
     /// Show `answers` (oldest first), from the top of the newest.
     pub fn set_answers(&mut self, answers: Vec<RenderedAnswer>) {
         self.answer = answers.len().saturating_sub(1);
@@ -247,6 +260,10 @@ impl ReaderView {
         };
         match &self.content {
             _ if self.list => push(&mut line, "formulas", Style::default()),
+            _ if self.title.is_some() => {
+                let title = self.title.as_deref().unwrap_or_default();
+                push(&mut line, &fit(title, width), Style::default());
+            }
             Content::Ready(answers) if !answers.is_empty() => {
                 let label = format!("answer {}/{}", self.answer + 1, answers.len());
                 let prompt = answers[self.answer]
diff --git a/src/tui/ui/leader.rs b/src/tui/ui/leader.rs
new file mode 100644
index 0000000..06a96be
--- /dev/null
+++ b/src/tui/ui/leader.rs
@@ -0,0 +1,94 @@
+//! Drawing the leader popup, an action's input line and the list picker
+//! (`tui/leader.rs` has their state and keys).
+
+use ratatui::style::{Modifier, Style};
+use ratatui::text::{Line, Span};
+use ratatui::widgets::Paragraph;
+use ratatui::Frame;
+
+use super::{centered, draw_input, popup};
+use crate::tui::app::App;
+use crate::tui::fmt::{str_width, truncate};
+use crate::tui::leader::{Input, Picker};
+use crate::tui::reader::layout::safe;
+
+/// The leader popup: `key  title  (extension)` per action.
+pub fn draw_leader(frame: &mut Frame, app: &App) {
+    let entries = app.leader_entries();
+    let area = frame.area();
+    let height = (entries.len() as u16 + 2).min(area.height);
+    let rect = centered(area, 56.min(area.width.saturating_sub(2)), height);
+    let inner = popup(frame, rect, "Extensions (a key runs · Esc closes)");
+    let dim = Style::default().add_modifier(Modifier::DIM);
+    let bold = Style::default().add_modifier(Modifier::BOLD);
+    let lines: Vec<Line> = entries
+        .iter()
+        .map(|e| {
+            let mut spans = vec![
+                Span::styled(format!(" {}  ", e.key), bold),
+                Span::raw(safe(&e.title)),
+            ];
+            if let Some(ext) = &e.extension {
+                spans.push(Span::styled(format!("  ({ext})"), dim));
+            }
+            Line::from(spans)
+        })
+        .collect();
+    frame.render_widget(Paragraph::new(lines), inner);
+}
+
+/// An action's line of input.
+pub fn draw_action_input(frame: &mut Frame, input: &Input) {
+    let area = frame.area();
+    let rect = centered(area, 64.min(area.width.saturating_sub(4)), 3);
+    let title = format!("{} (Enter run · Esc cancel)", safe(&input.prompt));
+    let inner = popup(frame, rect, &title);
+    draw_input(frame, inner, "", &input.text);
+}
+
+/// The list picker: the filter line, then `label  detail` rows (the detail
+/// dimmed and cut to the width), the selection reversed.
+pub fn draw_picker(frame: &mut Frame, picker: &Picker) {
+    let area = frame.area();
+    let rect = centered(
+        area,
+        100.min(area.width.saturating_sub(2)),
+        area.height.saturating_sub(4).max(3),
+    );
+    let title = format!("{} (Enter · Esc · type to filter)", safe(&picker.title));
+    let inner = popup(frame, rect, &title);
+    if inner.height == 0 {
+        return;
+    }
+    let width = inner.width as usize;
+    let dim = Style::default().add_modifier(Modifier::DIM);
+    let shown = picker.shown();
+    let count = format!("  {} of {}", shown.len(), picker.items.len());
+    let mut lines = vec![Line::from(vec![
+        Span::styled("/ ", dim),
+        Span::raw(safe(&picker.filter)),
+        Span::styled(count, dim),
+    ])];
+    let visible = (inner.height as usize).saturating_sub(1);
+    let offset = picker.selected.saturating_sub(visible.saturating_sub(1));
+    for (row, &i) in shown.iter().enumerate().skip(offset).take(visible) {
+        let item = &picker.items[i];
+        let label = truncate(&safe(&item.label), width);
+        let room = width.saturating_sub(str_width(&label) + 2);
+        let detail = item
+            .detail
+            .as_deref()
+            .filter(|_| room > 0)
+            .map(|d| truncate(&safe(d), room))
+            .unwrap_or_default();
+        let mut line = Line::from(vec![
+            Span::raw(label),
+            Span::styled(format!("  {detail}"), dim),
+        ]);
+        if row == picker.selected {
+            line = line.patch_style(Style::default().add_modifier(Modifier::REVERSED));
+        }
+        lines.push(line);
+    }
+    frame.render_widget(Paragraph::new(lines), inner);
+}
diff --git a/src/tui/ui/mod.rs b/src/tui/ui/mod.rs
index fd955e0..5b2b8e0 100644
--- a/src/tui/ui/mod.rs
+++ b/src/tui/ui/mod.rs
@@ -13,6 +13,7 @@ use crate::proto::{SessionKind, SessionState};
 
 mod confirm;
 mod git;
+mod leader;
 mod reader;
 mod stats;
 mod status;
@@ -98,6 +99,9 @@ fn draw_modal(frame: &mut Frame, app: &App, pane: Rect) {
         Some(Modal::Confirm(prompt)) => draw_confirm(frame, prompt),
         Some(Modal::Git(view)) => draw_git(frame, view, app.now, pane),
         Some(Modal::Reader(view)) => draw_reader(frame, app, view, pane),
+        Some(Modal::Leader) => leader::draw_leader(frame, app),
+        Some(Modal::ActionInput(input)) => leader::draw_action_input(frame, input),
+        Some(Modal::Picker(picker)) => leader::draw_picker(frame, picker),
         None => {}
     }
 }
@@ -793,7 +797,10 @@ pub fn draw_overview(frame: &mut Frame, app: &App, selected: usize, filter: &str
 /// Render the help popup listing all key bindings.
 pub fn draw_help(frame: &mut Frame, app: &App) {
     let entries = app.keymap.help_entries();
-    let extra = if app.upgrade_notice.is_some() { 2 } else { 0 };
+    let leader = app.leader_entries();
+    let leader_key = app.keymap.key_for(super::keymap::Action::Extensions);
+    let extra = if app.upgrade_notice.is_some() { 2 } else { 0 }
+        + leader_key.as_ref().map_or(0, |_| leader.len() as u16 + 2);
     let height = (entries.len() as u16 + 6 + extra).min(frame.area().height.saturating_sub(2));
     let area = frame.area();
     let rect = centered(area, 72.min(area.width.saturating_sub(2)), height);
@@ -815,9 +822,29 @@ pub fn draw_help(frame: &mut Frame, app: &App) {
             ])
         })
         .collect();
+    let dim = Style::default().add_modifier(Modifier::DIM);
+    // The leader's keys, after it.
+    if let Some(key) = leader_key {
+        lines.push(Line::from(""));
+        lines.push(Line::from(Span::styled(format!("  {key} then:"), dim)));
+        for entry in &leader {
+            let ext = entry
+                .extension
+                .as_ref()
+                .map(|e| format!("  ({e})"))
+                .unwrap_or_default();
+            lines.push(Line::from(vec![
+                Span::styled(
+                    format!("  {:>14}  ", entry.key),
+                    Style::default().add_modifier(Modifier::BOLD),
+                ),
+                Span::raw(super::reader::layout::safe(&entry.title)),
+                Span::styled(ext, dim),
+            ]));
+        }
+    }
     // Claude Code's own key, not ours: listed because it closes the tab.
     lines.push(Line::from(""));
-    let dim = Style::default().add_modifier(Modifier::DIM);
     let (ends, rest) = match app.mode {
         Mode::Manager => ("exit claude; the tab closes", None),
         Mode::Plain => (
`````
Run: `umask 022; cargo test --locked --bin claudio`
Expected: `test result: ok. 972 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` and `cargo build` with 0 warnings
Run: `umask 022; cargo test --locked --test manager -- --test-threads=1`
Expected: `test result: ok. 45 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` (the base's 45 tests; this task edits only the plain help test. In the scratch replay the full suite ran at Tasks 7 and 11; at Task 7 before this edit existed, `test_plain_help_lists_only_the_plain_keys` was the one failure, which this edit fixes)

Step 3: commit
`/usr/bin/git add src/tui/leader.rs src/tui/ui/leader.rs && /usr/bin/git commit -m "feat(tui): the leader key and extension actions" -- src/tui/leader.rs src/tui/ui/leader.rs src/tui/app.rs src/tui/interaction.rs src/tui/keymap.rs src/tui/mod.rs src/tui/plain.rs src/tui/reader/mod.rs src/tui/ui/mod.rs tests/manager.rs`

Interfaces this task provides to later tasks: `keymap::Action::Extensions`, `Modal::{Leader, ActionInput, Picker}`, `leader::{LeaderEntry, Input, Picker, INPUT_MAX, PASTE_MAX}`, `App::{leader_entries, start_action, on_action_result, apply_result, open_document, paste, open_reader (now pub(super))}`, `App.pending_action`, `Effect::RunAction`, `HostEvent::ActionDone`, `ReaderView::document`, `ReaderView.title`. Task 11 provides `App::builtin_action` (this task's placeholder is removed there).

### Task 7: framework manager tests
Model: sonnet   Repo: <repo>   Files: tests/manager.rs
Goal: one end-to-end test with a fake external extension (a shell script) and a fake claude: AC2 (argv and `--settings` without the instruction, context file with it), AC5 (pane and reader render `<keys>`), AC6 (leader lists built-in and external actions, Esc sends nothing, Alt+z u), AC7 (paste reaches claude without Enter and with no API key in the program's env; list; document with its own lens rendered; refused when bracketed paste is off; a hanging action times out with a notice). Crash/restart/disable is covered by Task 5's unit tests.

Step 1: write the test (it passes at once: Tasks 1–6 implement what it checks)
`````diff
diff --git a/tests/manager.rs b/tests/manager.rs
index 46c0a92..c383db2 100644
--- a/tests/manager.rs
+++ b/tests/manager.rs
@@ -2279,3 +2279,214 @@ fn test_reader_shows_the_answers_with_formulas_in_2d() {
     tui.send_keys(ALT_Q);
     tui.wait_exit(WAIT);
 }
+
+// ── Extensions ────────────────────────────────────────────────────────────────
+
+const ALT_Z: &[u8] = b"\x1bz";
+
+/// The external extension the extension tests install: a tag lens `keys`
+/// drawn as `⌨ <source>`, and actions that paste (with the API key it
+/// sees, which must be none), list, show a document, and hang.
+const FAKE_EXTENSION: &str = r#"name = "fake"
+description = "a fake extension"
+command = ["./fake.sh"]
+[[lens]]
+tag = "keys"
+instruction = "Write keys as <keys>Ctrl+C</keys>. EXT-INSTRUCTION"
+[[action]]
+key = "k"
+title = "paste a stamp"
+id = "stamp"
+[[action]]
+key = "l"
+title = "pick one"
+id = "pick"
+[[action]]
+key = "d"
+title = "show a document"
+id = "doc"
+[[action]]
+key = "h"
+title = "hang"
+id = "hang"
+"#;
+
+const FAKE_EXTENSION_PROGRAM: &str = r##"#!/bin/sh
+while IFS= read -r line; do
+  id=$(printf '%s' "$line" | sed 's/^{"id":\([0-9]*\).*/\1/')
+  case "$line" in
+    *'"method":"render"'*)
+      src=$(printf '%s' "$line" | sed 's/.*"source":"\([^"]*\)".*/\1/')
+      printf '{"id":%s,"result":{"inline":"⌨ %s"}}\n' "$id" "$src" ;;
+    *'"action":"stamp"'*)
+      printf '{"id":%s,"result":{"kind":"paste","text":"PASTED-%s"}}\n' "$id" "${ANTHROPIC_API_KEY:-nokey}" ;;
+    *'"action":"pick"'*)
+      printf '{"id":%s,"result":{"kind":"list","title":"Fake list","items":[{"label":"first","result":{"kind":"paste","text":"PICKED-FIRST"}},{"label":"second","detail":"the other one","result":{"kind":"paste","text":"PICKED-SECOND"}}]}}\n' "$id" ;;
+    *'"action":"doc"'*)
+      printf '{"id":%s,"result":{"kind":"document","title":"Fake doc","markdown":"# Heading\\n\\nPress <keys>Esc</keys> now."}}\n' "$id" ;;
+    *) ;;
+  esac
+done
+"##;
+
+/// Install the fake extension in the harness's config dir.
+fn install_fake_extension(h: &ManagerHarness) {
+    let dir = h.config_home.join("claudio/extensions/fake");
+    fs::create_dir_all(&dir).unwrap();
+    fs::write(dir.join("extension.toml"), FAKE_EXTENSION).unwrap();
+    let program = dir.join("fake.sh");
+    fs::write(&program, FAKE_EXTENSION_PROGRAM).unwrap();
+    fs::set_permissions(&program, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
+}
+
+/// A fake claude whose answer has a `<keys>` tag: it saves its `--settings`
+/// and argv, writes the transcript (a prompt and the answer), prints the
+/// answer, turns bracketed paste on, runs its SessionStart hook, then
+/// echoes lines; a line ending in `off` turns bracketed paste off.
+fn write_keys_fake_claude(h: &ManagerHarness, settings: &std::path::Path, argv: &std::path::Path) {
+    let sid = "cccccccc-dddd-4eee-8fff-000000000000";
+    let cwd = fs::canonicalize(&h.dirs[0]).unwrap();
+    let project = h.home.join(".claude/projects").join(encode_cwd(&cwd));
+    let prompt = serde_json::json!({"type": "user", "message": {"role": "user", "content": "How do I stop it?"}});
+    let answer = serde_json::json!({
+        "type": "assistant",
+        "message": {
+            "id": "m1",
+            "model": "claude-fake",
+            "content": [{"type": "text", "text": "Press <keys>Ctrl+C</keys> to stop."}],
+            "usage": {"input_tokens": 1, "output_tokens": 1},
+        },
+    });
+    let script = format!(
+        r#"#!/bin/sh
+if [ "$1" = "--version" ]; then echo "claude 0.0.0-fake"; exit 0; fi
+if [ "$1" = "--help" ]; then exit 0; fi
+printf '%s' "$2" > "{settings}"
+printf '%s\n' "$@" > "{argv}"
+mkdir -p "{project}"
+printf '%s\n' '{prompt}' '{answer}' > "{project}/{sid}.jsonl"
+echo "FAKE_CLAUDE_BANNER pid=$$"
+echo 'Press <keys>Ctrl+C</keys> to stop.'
+printf '\033[?2004h'
+HOOK=$(printf '%s' "$2" | sed -n 's/.*"command":"\([^"]*__hook SessionStart[^"]*\)".*/\1/p')
+printf '{{"session_id":"{sid}","source":"startup"}}' | eval "$HOOK"
+while IFS= read -r line; do
+  case "$line" in
+    *off) printf '\033[?2004l' ;;
+    *) printf 'GOT %s\n' "$line" ;;
+  esac
+done
+"#,
+        settings = settings.display(),
+        argv = argv.display(),
+        project = project.display(),
+    );
+    fs::write(&h.fake_claude, script).unwrap();
+    fs::set_permissions(&h.fake_claude, std::os::unix::fs::PermissionsExt::from_mode(0o755))
+        .unwrap();
+}
+
+/// AC2, AC5, AC6, AC7: an external extension's tag lens is rendered by its
+/// program in the pane and in the reader; its instruction reaches claude
+/// through the context file only; Alt+z lists the built-in and external
+/// actions, Esc closes it with nothing sent, Alt+z u is the reader; the
+/// program's paste reaches claude without Enter (and is refused once
+/// bracketed paste is off), its list picks, its document shows (its own
+/// lens rendered), and a hanging action times out with a notice.
+#[test]
+fn test_external_extension_lens_leader_and_actions() {
+    let h = ManagerHarness::new();
+    install_fake_extension(&h);
+    let (settings, argv) = (h.root.join("settings.json"), h.root.join("argv.txt"));
+    write_keys_fake_claude(&h, &settings, &argv);
+    let mut tui = h.start_tui();
+    wizard_pick_dir(&mut tui, &h.dirs[0]);
+
+    // AC5: the pane shows the program's rendering of the tag.
+    tui.wait_for("Press ⌨ Ctrl+C to stop.", Region::Pane, DAEMON_WAIT);
+
+    // AC2: the hook has the context file's path; the text is only in it.
+    let hooks = fs::read_to_string(&settings).unwrap();
+    assert!(hooks.contains("__context file"), "{hooks}");
+    assert!(!hooks.contains("EXT-INSTRUCTION"), "{hooks}");
+    assert!(!fs::read_to_string(&argv).unwrap().contains("EXT-INSTRUCTION"));
+    let context_dir = h.runtime_dir.join("claudio/context");
+    let context: Vec<String> = fs::read_dir(&context_dir)
+        .unwrap()
+        .map(|e| fs::read_to_string(e.unwrap().path()).unwrap())
+        .collect();
+    assert_eq!(context.len(), 1);
+    assert!(context[0].contains("EXT-INSTRUCTION"), "{context:?}");
+    assert!(context[0].contains("<latex>"), "the built-in lenses too");
+
+    // AC5: the reader renders it as well.
+    tui.send_keys(ALT_U);
+    tui.wait_for("answer 1/1 · How do I stop it?", Region::Pane, DAEMON_WAIT);
+    tui.wait_for("Press ⌨ Ctrl+C to stop.", Region::Pane, WAIT);
+    tui.send_keys(ESC);
+    tui.wait_until(|s| !s.contains("answer 1/1", Region::Pane), WAIT);
+
+    // AC6: the leader lists built-in and external actions; Esc sends nothing.
+    tui.send_keys(ALT_Z);
+    tui.wait_for("search conversations", Region::Screen, WAIT);
+    let shown = tui.screen_text(Region::Screen);
+    for row in ["u  reader view", "p  insert snippet", "k  paste a stamp  (fake)"] {
+        assert!(shown.contains(row), "missing {row:?} in:\n{shown}");
+    }
+    tui.send_keys(ESC);
+    tui.wait_until(|s| !s.contains("paste a stamp", Region::Screen), WAIT);
+    thread::sleep(Duration::from_millis(300));
+    assert!(!tui.screen_text(Region::Pane).contains("^["), "nothing reached claude");
+    tui.send_keys(ALT_Z);
+    tui.wait_for("paste a stamp", Region::Screen, WAIT);
+    tui.send_keys(b"u");
+    tui.wait_for("answer 1/1 · How do I stop it?", Region::Pane, DAEMON_WAIT);
+    tui.send_keys(ESC);
+    tui.wait_until(|s| !s.contains("answer 1/1", Region::Pane), WAIT);
+
+    // AC7: a paste reaches claude without Enter (no `GOT` line), and no API
+    // key reaches the program.
+    tui.send_keys(ALT_Z);
+    tui.wait_for("paste a stamp", Region::Screen, WAIT);
+    tui.send_keys(b"k");
+    tui.wait_for("PASTED-nokey", Region::Pane, WAIT);
+    thread::sleep(Duration::from_millis(300));
+    let pane = tui.screen_text(Region::Pane);
+    assert_eq!(pane.matches("PASTED-nokey").count(), 1, "{pane}");
+    assert!(!pane.contains("GOT"), "no Enter followed the paste:\n{pane}");
+
+    // A list: Down, Enter pastes the second item's text.
+    tui.send_keys(ALT_Z);
+    tui.wait_for("pick one", Region::Screen, WAIT);
+    tui.send_keys(b"l");
+    tui.wait_for("Fake list", Region::Screen, WAIT);
+    tui.wait_for("the other one", Region::Screen, WAIT);
+    tui.send_keys(DOWN_ARROW);
+    tui.send_keys(ENTER);
+    tui.wait_for("PICKED-SECOND", Region::Pane, WAIT);
+
+    // A document, its own lens rendered by the program.
+    tui.send_keys(ALT_Z);
+    tui.wait_for("show a document", Region::Screen, WAIT);
+    tui.send_keys(b"d");
+    tui.wait_for("Fake doc", Region::Pane, WAIT);
+    tui.wait_for("Press ⌨ Esc now.", Region::Pane, WAIT);
+    tui.send_keys(ESC);
+    tui.wait_until(|s| !s.contains("Fake doc", Region::Pane), WAIT);
+
+    // Bracketed paste off: the paste is refused with a notice.
+    tui.send_keys(b"off\r");
+    thread::sleep(Duration::from_millis(300));
+    tui.send_keys(ALT_Z);
+    tui.wait_for("paste a stamp", Region::Screen, WAIT);
+    tui.send_keys(b"k");
+    tui.wait_for("bracketed paste is off", Region::StatusBar, WAIT);
+
+    // A hanging action times out (15 s) with a notice.
+    tui.send_keys(ALT_Z);
+    tui.wait_for("hang", Region::Screen, WAIT);
+    tui.send_keys(b"h");
+    tui.wait_for("extension fake timed out", Region::StatusBar, Duration::from_secs(25));
+    tui.send_keys(ALT_Q);
+    tui.wait_exit(WAIT);
+}
`````
Run: `umask 022; cargo test --locked --test manager -- --test-threads=1 test_external_extension_lens_leader_and_actions`
Expected: `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 45 filtered out` (about 20 s: the hang waits out the 15 s timeout)
Run: `umask 022; cargo test --locked --test manager -- --test-threads=1`
Expected: `test result: ok. 46 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` (Tasks 8–10 add no manager test; derived from the Task 11 run: 49 = 46 + 3)

Step 2: none.

Step 3: commit
`/usr/bin/git commit -m "test(manager): external extension lens, leader and actions" -- tests/manager.rs`

Interfaces this task provides to later tasks: `tests/manager.rs` helpers `ALT_Z`, `FAKE_EXTENSION`, `FAKE_EXTENSION_PROGRAM`, `install_fake_extension`, `write_keys_fake_claude`.

> Tasks 8–10 were executed before the rest of the plan (controller decision, ledger 17:30–17:45): they are on feat/extensions as 1ddd880 (T8), d323c1c (T10), f51437b (T9). Kept here for the record and the gate audit; Tasks 1–7, 11, 12 run on top of f51437b. Ruling R9 replaced their DAEMON_FEATURES steps.

### Task 8: diagram engine (`src/tui/diagram/`)
Model: opus   Repo: <repo> (feat/extensions, or the worktree the controller gives)
Files: create `src/tui/diagram/mod.rs`, `src/tui/diagram/text.rs`, `src/tui/diagram/canvas.rs`, `src/tui/diagram/flowchart.rs`, `src/tui/diagram/sequence.rs`, `src/tui/diagram/tests.rs`; modify `src/tui/mod.rs` (two lines after `mod connections;`).
Parallel: independent of every other task (Task 1 included). In `src/tui/mod.rs` Task 10 adds its two lines after `mod sessions;` and part A may add `mod extensions;`: different lines of the `mod` list.
Goal: the pure Mermaid engine of spec §7 (AC8, unit part): `too_complex`, `render`, `Unsupported`, for flowcharts (`graph`/`flowchart`, TD/TB/BT/LR/RL) and `sequenceDiagram`; the v1-distinct set drawn distinctly and the rest mapped to it; every cap checked before allocating; nothing recursive on the input.

How it works (for the implementer and the reviewer):
- `mod.rs`: caps (`MAX_SOURCE_BYTES` 4096, `MAX_SOURCE_LINES` 200, `MAX_NODES` 64, `MAX_EDGES` 128, `MAX_COLS` 200, `MAX_ROWS` 150), `too_complex`, `render` (header → flowchart or sequence; anything else `Unsupported("unsupported diagram: <word>")`); `%%` comments/directives and a leading `---` front-matter block are skipped.
- `text.rs`: `clean` (`sanitize`, `<br>` → line break, tab → space, only width-1/2 characters kept so the canvas never miscounts), `wrap` (24 columns, ≤ 3 lines, the last ending `…` when cut), `fit`.
- `canvas.rs`: a cell grid. Lines are recorded as up/down/left/right links per cell and become `│ ─ ┌ ┐ └ ┘ ├ ┤ ┬ ┴ ┼` at the end, so lines that meet merge into junctions; dotted `┆ ┄` only where no solid line shares the cell; text and box borders override lines; a wide character takes two cells.
- `flowchart.rs`: hand-written parser: ids (letters, digits, `_`, inner `-`), 12 shape delimiters with optional quoted text, links `-->`, `---`, `-.->`, `-.-`, `==>`, `===`, `--o`, `--x`, `<-->`/`<-.->` (two edges), `~~~` (nodes only), labels `|t|` and `-- t -->`/`-. t .->`/`== t ==>`, chains, `&`, `;`, `:::class`; `subgraph`/`end`/`direction`/`style`/`classDef`/`class`/`click`/`linkStyle` lines ignored; node and edge caps checked as each is added. Shapes: `{}` decision (`╱──╲`/`╲──╱` corners), `()`/`([])`/`(())` rounded (`╭╮╰╯`), every other shape a rect (`┌┐└┘`); `==>`/`===` solid, `--o`/`--x` plain arrows. Layout: self-loops and repeated edges are listed under the drawing; cycles broken by an iterative DFS (back edges reversed); longest-path layers, then each source moved down next to its nearest successor; dummy nodes on long edges; 8 barycenter sweeps, keeping the order with the fewest crossings; cross positions by weighted isotonic regression (pool adjacent violators; dummies weigh 0.1 so boxes line up and long edges bend around them). Edges: one shared center port per side for forward edges (fan-out and fan-in merge into `┬`/`┴`), an own port slot (±2 columns; ±1 row in LR) for each reversed edge, on the side that crosses fewest other lines, with `▲` (`◀`…) at the original target. Per gap between layers: source-label band, tracks (greedy interval packing, reversed edges first so they take the top track), target-label band, arrow row. A label sits beside a port that only its edge uses (target side first, else source side); otherwise the edge is drawn and also listed as `a ──label──▶ b`. Everything is laid out in abstract (cross, main) coordinates and mapped to TD/BT/LR/RL only when drawn (BT/RL mirror the main axis; text is never mirrored). The total size, listed rows included, is checked against 200×150 before the canvas is allocated.
- `sequence.rs`: participants (`participant`/`actor`, `as` alias, implicit ones in order of appearance), arrows `->>`/`-->>` (head), `->`/`-->` (no head), `-x`/`--x`/`-)`/`--)` (plain head), `+`/`-` suffixes stripped, self-messages, `Note left of|right of|over a[,b]` (dashed box), `loop|alt|opt|par|critical|break|rect … else|and|option … end` as labeled frames (≤ 8 deep; unclosed ones closed at the end), `autonumber`/`activate`/`deactivate` ignored, anything else `cannot parse`. Lifeline columns are a longest-path solution of spacing constraints (box widths, message texts, notes, self-loops); the size is checked before the canvas.

What four of the goldens draw (each is its own test in `tests.rs`; side by side here only for reading):
```text
            ┌───────┐                    ┌───────┐  ┌──────┐  ┌───────┐  ╭──────╮
            │ Start │                    │ Parse ├─▶│ Plan ├─▶│ Build ├─▶│ Done │
            └───┬───┘                    └───────┘  └──────┘  └───────┘  ╰──────╯
                │
                ▼                          ┌─────┐       ┌───────┐      ┌─────┐
        ╱──────────────╲                   │  A  │       │ Alice │      │ Bob │
        │ Is it ready? │                   └──┬──┘       └───┬───┘      └──┬──┘
        ╲───────┬──────╱                      │ ▲            │  Hello Bob  │
                │ ▲                           │ └─┐          ├────────────▶│
                │ └────┐                      ▼   │          │             │ ┌┄┄┄┄┄┄┄┄┐
     ┌──────────┴────┐ │                    ┌───┐ │          │             │ ┆ thinks ┆
     │               │ │                    │ B │ │          │             │ └┄┄┄┄┄┄┄┄┘
    yes             no │                    └─┬─┘ │    ┌─ loop: every minute ─────────────┐
     ▼               ▼ │                      │ ┌─┘    │     │ still here  │              │
┌─────────┐  ╭─────────┴─────╮                ▼ │      │     │◀┄┄┄┄┄┄┄┄┄┄┄┄┤              │
│ Ship it │  │ Fix the build │             ┌────┴┐     └─────┼─────────────┼──────────────┘
└─────────┘  ╰───────────────╯             │  C  │           │     bye     │
                                           └─────┘           ├────────────▶│
                                                             │             │
```

Step 1: write the failing tests

In `src/tui/mod.rs`, after the line `mod connections;` add:
```rust
#[allow(dead_code)] // Wired into the reader by Task 11, which removes this.
mod diagram;
```
Create `src/tui/diagram/mod.rs` with only this (Step 2 replaces it):
```rust
//! Mermaid diagrams (Task 8, step 1).

#[cfg(test)]
mod tests;
```
Create `src/tui/diagram/tests.rs`:

```rust
use std::time::{Duration, Instant};

use super::*;

/// `src` drawn, and every row within the caps and free of controls.
fn draw(src: &str) -> Vec<String> {
    let rows = render(src).unwrap_or_else(|e| panic!("{src}: {}", e.reason));
    assert!(rows.len() <= MAX_ROWS);
    for row in &rows {
        assert!(text::width(row) <= MAX_COLS, "{row}");
        assert!(!row.chars().any(char::is_control), "{row:?}");
        assert_eq!(row.trim_end(), row);
    }
    rows
}

/// The reason `src` is not drawn, found within a second.
fn refused(src: &str) -> String {
    let start = Instant::now();
    let reason = render(src).expect_err("unsupported").reason;
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "{reason}: {:?}",
        start.elapsed()
    );
    reason
}

/// AC8: a TD flowchart with a decision, labels, and a loop back up.
#[test]
fn td_flowchart_with_a_decision_and_labels() {
    let src = "flowchart TD
    A[Start] --> B{Is it ready?}
    B -->|yes| C[Ship it]
    B -->|no| D(Fix the build)
    D --> B";
    let expected = [
        "            ┌───────┐",
        "            │ Start │",
        "            └───┬───┘",
        "                │",
        "                ▼",
        "        ╱──────────────╲",
        "        │ Is it ready? │",
        "        ╲───────┬──────╱",
        "                │ ▲",
        "                │ └────┐",
        "     ┌──────────┴────┐ │",
        "     │               │ │",
        "    yes             no │",
        "     ▼               ▼ │",
        "┌─────────┐  ╭─────────┴─────╮",
        "│ Ship it │  │ Fix the build │",
        "└─────────┘  ╰───────────────╯",
    ];
    assert_eq!(draw(src), expected);
}

/// AC8: an LR chain (`a --> b --> c`), with a rounded end.
#[test]
fn lr_chain() {
    let src = "graph LR
    a[Parse] --> b[Plan] --> c[Build] --> d([Done])";
    let expected = [
        "┌───────┐  ┌──────┐  ┌───────┐  ╭──────╮",
        "│ Parse ├─▶│ Plan ├─▶│ Build ├─▶│ Done │",
        "└───────┘  └──────┘  └───────┘  ╰──────╯",
    ];
    assert_eq!(draw(src), expected);
}

/// AC8: a cycle: the back edge leaves its own port with `▲`.
#[test]
fn cycle() {
    let src = "graph TD
    A --> B
    B --> C
    C --> A";
    let expected = [
        "┌─────┐",
        "│  A  │",
        "└──┬──┘",
        "   │ ▲",
        "   │ └─┐",
        "   ▼   │",
        " ┌───┐ │",
        " │ B │ │",
        " └─┬─┘ │",
        "   │ ┌─┘",
        "   ▼ │",
        "┌────┴┐",
        "│  C  │",
        "└─────┘",
    ];
    assert_eq!(draw(src), expected);
}

/// AC8: a sequence diagram with a note and a loop.
#[test]
fn sequence_with_a_note_and_a_loop() {
    let src = "sequenceDiagram
    participant A as Alice
    participant B as Bob
    A->>B: Hello Bob
    Note right of B: thinks
    loop every minute
        B-->>A: still here
    end
    A-)B: bye";
    let expected = [
        "  ┌───────┐      ┌─────┐",
        "  │ Alice │      │ Bob │",
        "  └───┬───┘      └──┬──┘",
        "      │  Hello Bob  │",
        "      ├────────────▶│",
        "      │             │ ┌┄┄┄┄┄┄┄┄┐",
        "      │             │ ┆ thinks ┆",
        "      │             │ └┄┄┄┄┄┄┄┄┘",
        "┌─ loop: every minute ─────────────┐",
        "│     │ still here  │              │",
        "│     │◀┄┄┄┄┄┄┄┄┄┄┄┄┤              │",
        "└─────┼─────────────┼──────────────┘",
        "      │     bye     │",
        "      ├────────────▶│",
        "      │             │",
    ];
    assert_eq!(draw(src), expected);
}

/// Fan-out and merges share junctions; dotted edges stay dotted.
#[test]
fn merged_junctions_and_dotted_edges() {
    let src = "graph TD
  A[Client] --> B[API gateway]
  B --> C[Auth service]
  B --> D[Orders]
  D --> E[(Database)]
  C --> E
  D -.-> F[Queue]
  F -.-> G[Worker]
  G --> E";
    let expected = [
        "          ┌────────┐",
        "          │ Client │",
        "          └────┬───┘",
        "               │",
        "               ▼",
        "        ┌─────────────┐",
        "        │ API gateway │",
        "        └──────┬──────┘",
        "        ┌──────┴───────┐",
        "        ▼              ▼",
        "┌──────────────┐  ┌────────┐",
        "│ Auth service │  │ Orders │",
        "└───────┬──────┘  └────┬───┘",
        "        └───┐      ┌┄┄┄┴─┐",
        "            │      ▼     │",
        "            │  ┌───────┐ │",
        "            │  │ Queue │ │",
        "            │  └───┬───┘ │",
        "            │      ┆     │",
        "            │      ▼     │",
        "            │ ┌────────┐ │",
        "            │ │ Worker │ │",
        "            │ └────┬───┘ │",
        "            └──────┤     │",
        "                   ├─────┘",
        "                   ▼",
        "             ┌──────────┐",
        "             │ Database │",
        "             └──────────┘",
    ];
    assert_eq!(draw(src), expected);
}

/// `%%`, `;`, `&`, `-- text -->`, `---`, `subgraph`/`style` lines, BT;
/// a self-loop cannot be drawn and is listed under the drawing.
#[test]
fn syntax_variants_and_an_unplaceable_edge() {
    let src = "%% a comment
graph BT;
  subgraph s [Group]
  a & b -- uses --> c;
  end
  c --- d
  style a fill:#f9f
  a --> a";
    let expected = [
        "     ┌───┐",
        "     │ d │",
        "     └─┬─┘",
        "       │",
        "       │",
        "     ┌─┴─┐",
        "     │ c │",
        "     └───┘",
        "       ▲",
        "       ├───┐",
        "   ┌───┘   │",
        " uses    uses",
        "   │       │",
        " ┌─┴─┐   ┌─┴─┐",
        " │ a │   │ b │",
        " └───┘   └───┘",
        "",
        "a ──▶ a",
    ];
    assert_eq!(draw(src), expected);
}

/// RL, `<-.->` as two dotted edges, a stadium as rounded.
#[test]
fn right_to_left_and_both_ways() {
    let src = "flowchart RL
  x([Out]) <-.-> y[In]";
    let expected = [
        "┌────┐   ╭─────╮",
        "│    │   │     │",
        "│ In │◀┄┄┤ Out │",
        "│    ├┄┄▶│     │",
        "└────┘   ╰─────╯",
    ];
    assert_eq!(draw(src), expected);
}

/// Self-messages, solid and dotted lines without heads, `alt`/`else`,
/// a note over two participants.
#[test]
fn sequence_blocks_and_self_messages() {
    let src = "sequenceDiagram
  Alice->>Bob: Hi
  Bob->>Bob: think
  alt is sick
    Bob-->Alice: Not so good
  else is well
    Bob->Alice: Feeling fresh
  end
  Note over Alice,Bob: done";
    let expected = [
        "  ┌───────┐         ┌─────┐",
        "  │ Alice │         │ Bob │",
        "  └───┬───┘         └──┬──┘",
        "      │      Hi        │",
        "      ├───────────────▶│",
        "      │                ├──┐ think",
        "      │                │  │",
        "      │                │◀─┘",
        "┌─ alt: is sick ───────┼────────────┐",
        "│     │  Not so good   │            │",
        "│     ├┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┤            │",
        "├┄ else: is well ┄┄┄┄┄┄┼┄┄┄┄┄┄┄┄┄┄┄┄┤",
        "│     │ Feeling fresh  │            │",
        "│     ├────────────────┤            │",
        "└─────┼────────────────┼────────────┘",
        "    ┌┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┐",
        "    ┆ done               ┆",
        "    └┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┘",
        "      │                │",
    ];
    assert_eq!(draw(src), expected);
}

/// AC8: other diagram types and unknown syntax are not drawn.
#[test]
fn unsupported_types_and_syntax() {
    assert_eq!(
        refused("classDiagram\n  Animal <|-- Duck"),
        "unsupported diagram: classDiagram"
    );
    assert_eq!(
        refused("stateDiagram-v2\n  [*] --> A"),
        "unsupported diagram: stateDiagram-v2"
    );
    assert_eq!(refused("pie\n  \"a\": 1"), "unsupported diagram: pie");
    assert_eq!(refused(""), "empty diagram");
    assert_eq!(refused("%% only a comment"), "empty diagram");
    assert_eq!(refused("graph XY\n  a --> b"), "unsupported direction: XY");
    assert_eq!(
        refused("graph TD\n  a --> b[unclosed"),
        "unclosed node shape"
    );
    assert_eq!(refused("graph TD\n  a b"), "cannot parse: a b");
    assert_eq!(refused("sequenceDiagram\n  end"), "end without a block");
    assert_eq!(
        refused("sequenceDiagram\n  Alice says hi"),
        "cannot parse: Alice says hi"
    );
}

/// AC8: hostile input hits each cap and is refused fast.
#[test]
fn hostile_input_hits_each_cap() {
    // Source size, checked before parsing.
    let long = format!("graph TD\n  a[{}] --> b", "x".repeat(MAX_SOURCE_BYTES));
    assert!(too_complex(&long));
    assert_eq!(refused(&long), "diagram source too large");
    let many = format!("graph TD\n{}", "  a --> b\n".repeat(MAX_SOURCE_LINES));
    assert!(too_complex(&many));
    assert_eq!(refused(&many), "diagram source too large");
    assert!(!too_complex("graph TD\n  a --> b"));
    // Nodes and edges, counted as they are parsed.
    let chain: Vec<String> = (0..MAX_NODES)
        .map(|i| format!("  n{i} --> n{}", i + 1))
        .collect();
    let nodes = format!("graph TD\n{}", chain.join("\n"));
    assert_eq!(refused(&nodes), "more than 64 nodes");
    let fan = "graph LR\n  a & b & c & d & e & f & g & h & i & j & k & l --> \
               m & n & o & p & q & r & s & t & u & v & w";
    assert_eq!(refused(fan), "more than 128 edges");
    // Drawing size, computed before the canvas is allocated.
    let wide: Vec<String> = (0..12)
        .map(|i| format!("  r --> c{i}[abcdefghijklmnopqrst]"))
        .collect();
    assert_eq!(
        refused(&format!("graph TD\n{}", wide.join("\n"))),
        "diagram too large"
    );
    let tall: Vec<String> = (0..40).map(|i| format!("  n{i} --> n{}", i + 1)).collect();
    assert_eq!(
        refused(&format!("graph TD\n{}", tall.join("\n"))),
        "diagram too large"
    );
    // Sequence diagrams: participants, messages, rows, nesting.
    let people: Vec<String> = (0..=MAX_NODES)
        .map(|i| format!("  participant p{i}"))
        .collect();
    let people = format!("sequenceDiagram\n{}", people.join("\n"));
    assert_eq!(refused(&people), "more than 64 participants");
    let chatty = format!("sequenceDiagram\n{}", "  a->>b: hi\n".repeat(MAX_EDGES + 1));
    assert_eq!(refused(&chatty), "more than 128 messages");
    let rows = format!("sequenceDiagram\n{}", "  a->>b: hi\n".repeat(80));
    assert_eq!(refused(&rows), "diagram too large");
    let deep = format!("sequenceDiagram\n{}  a->>b: hi\n", "  loop x\n".repeat(9));
    assert_eq!(refused(&deep), "blocks nested too deep");
}

/// The largest graphs the caps allow lay out quickly, with or without a
/// drawing at the end.
#[test]
fn dense_graphs_within_the_caps_are_quick() {
    let mut src = String::from("graph TD\n");
    for i in 0..MAX_NODES {
        src.push_str(&format!("  n{i} --> n{}\n", (i * 7 + 3) % MAX_NODES));
        src.push_str(&format!("  n{i} -.-> n{}\n", (i * 13 + 5) % MAX_NODES));
    }
    assert!(!too_complex(&src));
    let start = Instant::now();
    let _ = render(&src);
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "{:?}",
        start.elapsed()
    );
}

/// Labels wrap at 24 columns, at most 3 lines, and lose what cannot be
/// drawn.
#[test]
fn labels_wrap_and_are_cleaned() {
    assert_eq!(text::wrap("short"), ["short"]);
    assert_eq!(
        text::wrap("one two three four five six seven eight"),
        ["one two three four five", "six seven eight"]
    );
    let long = "word ".repeat(30);
    let lines = text::wrap(&long);
    assert_eq!(lines.len(), text::MAX_LINES);
    assert!(lines[2].ends_with('…'), "{lines:?}");
    assert!(lines.iter().all(|l| text::width(l) <= text::WRAP));
    assert_eq!(text::wrap(&"x".repeat(30)), ["x".repeat(24), "x".repeat(6)]);
    assert_eq!(
        text::clean("a<br>b\u{1b}[31m\u{200b}\u{301}\tc"),
        "a\nb[31m c"
    );
}
```

Run: `umask 022; cargo test --locked --bin claudio diagram`
Expected (compile failure), among others:
```
error[E0425]: cannot find function `render` in this scope
error[E0425]: cannot find function `too_complex` in this scope
error[E0433]: failed to resolve: use of unresolved module or unlinked crate `text`
error: could not compile `claudio` (bin "claudio" test) due to 27 previous errors; 1 warning emitted
```

Step 2: implement

Replace `src/tui/diagram/mod.rs` with:

```rust
//! Mermaid diagrams drawn with box-drawing characters, for the reader view's
//! ` ```mermaid ` fence lens.
//!
//! Pure functions only. The input is claude's text, so it is hostile: every
//! size is capped before anything is allocated for it, nothing recurses on
//! the input's depth, and [`render`] still runs only on the render worker
//! behind its guard (pre-check, scratch thread, budget), never on the UI
//! thread. Two kinds are drawn: flowcharts ([`flowchart`]) and sequence
//! diagrams ([`sequence`]); anything else is [`Unsupported`] and the reader
//! keeps the code block.

mod canvas;
mod flowchart;
mod sequence;
mod text;

/// The most bytes a diagram's source may have ([`too_complex`]).
pub const MAX_SOURCE_BYTES: usize = 4096;
/// The most lines a diagram's source may have ([`too_complex`]).
pub const MAX_SOURCE_LINES: usize = 200;
/// The most nodes of a flowchart, or participants of a sequence diagram.
pub const MAX_NODES: usize = 64;
/// The most edges of a flowchart, or messages of a sequence diagram.
pub const MAX_EDGES: usize = 128;
/// The widest drawing, in terminal columns.
pub const MAX_COLS: usize = 200;
/// The tallest drawing, in rows.
pub const MAX_ROWS: usize = 150;

/// Why a diagram is not drawn; the reader shows its code block instead.
#[derive(Debug, Clone, PartialEq)]
pub struct Unsupported {
    pub reason: String,
}

impl Unsupported {
    fn new(reason: impl Into<String>) -> Unsupported {
        Unsupported {
            reason: reason.into(),
        }
    }
}

/// Whether `src` is too big to try: more than [`MAX_SOURCE_BYTES`] bytes or
/// [`MAX_SOURCE_LINES`] lines. Cheap; no parsing.
pub fn too_complex(src: &str) -> bool {
    src.len() > MAX_SOURCE_BYTES || src.lines().nth(MAX_SOURCE_LINES).is_some()
}

/// The rows of the drawing of `src`, a ` ```mermaid ` fence's content
/// (without the fence lines). Every row is at most [`MAX_COLS`] columns,
/// free of control characters and trimmed at the end; there are at most
/// [`MAX_ROWS`] rows. Edges that cannot be drawn are listed under the
/// drawing as `a ──label──▶ b`.
pub fn render(src: &str) -> Result<Vec<String>, Unsupported> {
    if too_complex(src) {
        return Err(Unsupported::new("diagram source too large"));
    }
    let mut lines = statements(src);
    let Some(header) = lines.next() else {
        return Err(Unsupported::new("empty diagram"));
    };
    let (kind, rest) = match header.split_once(char::is_whitespace) {
        Some((kind, rest)) => (kind, rest.trim()),
        None => (header, ""),
    };
    let rows = match kind.trim_end_matches(';') {
        "graph" | "flowchart" => flowchart::render(rest, lines)?,
        "sequenceDiagram" if rest.is_empty() => sequence::render(lines)?,
        other => {
            let other: String = other.chars().take(24).collect();
            return Err(Unsupported::new(format!("unsupported diagram: {other}")));
        }
    };
    debug_assert!(rows.len() <= MAX_ROWS);
    Ok(rows)
}

/// The meaningful lines of `src`, trimmed: no blank lines, no `%%`
/// comments or directives, and no `---` front matter block at the start.
fn statements(src: &str) -> impl Iterator<Item = &str> {
    let mut front_matter = false;
    let mut first = true;
    src.lines().map(str::trim).filter(move |line| {
        if first && *line == "---" {
            front_matter = true;
            first = false;
            return false;
        }
        if front_matter {
            front_matter = *line != "---";
            return false;
        }
        if line.is_empty() || line.starts_with("%%") {
            return false;
        }
        first = false;
        true
    })
}

#[cfg(test)]
mod tests;
```

Create `src/tui/diagram/text.rs`:

```rust
//! Labels: cleaning, measuring and wrapping.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Labels wrap at this many columns.
pub const WRAP: usize = 24;
/// A label has at most this many lines; the last one ends in `…` if cut.
pub const MAX_LINES: usize = 3;

/// `raw` made safe to draw: [`sanitize`](crate::claude::latex::sanitize)d,
/// `<br>` and `\n` as line breaks, tabs as spaces, and characters without a
/// width of 1 or 2 (combining marks, zero-width) dropped, so that a
/// character always takes the columns it is counted for.
pub fn clean(raw: &str) -> String {
    let raw = raw
        .replace("<br/>", "\n")
        .replace("<br />", "\n")
        .replace("<br>", "\n");
    crate::claude::latex::sanitize(&raw)
        .chars()
        .map(|c| if c == '\t' { ' ' } else { c })
        .filter(|&c| c == '\n' || matches!(c.width(), Some(1 | 2)))
        .collect()
}

/// Display columns of `s` (already [`clean`]).
pub fn width(s: &str) -> usize {
    s.width()
}

/// `label` ([`clean`]ed) in lines of at most [`WRAP`] columns, at most
/// [`MAX_LINES`] of them. Words longer than a line are split.
pub fn wrap(label: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for paragraph in label.split('\n') {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            for piece in split_word(word) {
                let sep = usize::from(!line.is_empty());
                if !line.is_empty() && width(&line) + sep + width(&piece) > WRAP {
                    lines.push(std::mem::take(&mut line));
                }
                if !line.is_empty() {
                    line.push(' ');
                }
                line.push_str(&piece);
            }
            if lines.len() > MAX_LINES {
                break;
            }
        }
        if !line.is_empty() {
            lines.push(line);
        }
        if lines.len() > MAX_LINES {
            break;
        }
    }
    if lines.len() > MAX_LINES {
        lines.truncate(MAX_LINES);
        let last = &mut lines[MAX_LINES - 1];
        *last = fit(&format!("{last} …"), WRAP);
    }
    lines
}

/// `word` in pieces of at most [`WRAP`] columns.
fn split_word(word: &str) -> Vec<String> {
    let mut pieces = vec![String::new()];
    for c in word.chars() {
        let piece = pieces.last_mut().expect("never empty");
        if width(piece) + c.width().unwrap_or(0) > WRAP {
            pieces.push(String::new());
        }
        pieces.last_mut().expect("never empty").push(c);
    }
    pieces
}

/// `s` cut to at most `max` columns, ending in `…` when cut.
pub fn fit(s: &str, max: usize) -> String {
    if width(s) <= max {
        return s.to_owned();
    }
    let mut out = String::new();
    for c in s.chars() {
        if width(&out) + c.width().unwrap_or(0) + 1 > max {
            break;
        }
        out.push(c);
    }
    let mut out = out.trim_end().to_owned();
    out.push('…');
    out
}

/// The widest line of `lines`.
pub fn widest(lines: &[String]) -> usize {
    lines.iter().map(|l| width(l)).max().unwrap_or(0)
}
```

Create `src/tui/diagram/canvas.rs`:

```rust
//! A grid of terminal cells that lines, boxes and text are drawn on.
//!
//! Lines are recorded as connections (up, down, left, right) per cell and
//! turned into box-drawing characters at the end, so two lines that meet
//! merge into one junction (`┬`, `┴`, `├`, `┼`…). Text and box borders are
//! written as characters and win over lines.

use unicode_width::UnicodeWidthChar;

pub const UP: u8 = 1;
pub const DOWN: u8 = 2;
pub const LEFT: u8 = 4;
pub const RIGHT: u8 = 8;

/// Marks the second column of a wide character.
const WIDE_TAIL: char = '\0';

pub struct Canvas {
    width: usize,
    height: usize,
    chars: Vec<Option<char>>,
    links: Vec<u8>,
    /// Per cell: some solid line passes (else, with links, only dotted ones).
    solid: Vec<bool>,
}

/// The four corners and the two sides of a box.
pub struct BoxStyle {
    pub top_left: char,
    pub top_right: char,
    pub bottom_left: char,
    pub bottom_right: char,
    pub horizontal: char,
    pub vertical: char,
}

pub const RECT: BoxStyle = BoxStyle {
    top_left: '┌',
    top_right: '┐',
    bottom_left: '└',
    bottom_right: '┘',
    horizontal: '─',
    vertical: '│',
};
pub const ROUNDED: BoxStyle = BoxStyle {
    top_left: '╭',
    top_right: '╮',
    bottom_left: '╰',
    bottom_right: '╯',
    horizontal: '─',
    vertical: '│',
};
/// A decision: slanted corners.
pub const DECISION: BoxStyle = BoxStyle {
    top_left: '╱',
    top_right: '╲',
    bottom_left: '╲',
    bottom_right: '╱',
    horizontal: '─',
    vertical: '│',
};
/// A sequence diagram's note: dashed sides.
pub const NOTE: BoxStyle = BoxStyle {
    top_left: '┌',
    top_right: '┐',
    bottom_left: '└',
    bottom_right: '┘',
    horizontal: '┄',
    vertical: '┆',
};

impl Canvas {
    /// A blank canvas. The caller has checked the size against the caps.
    pub fn new(width: usize, height: usize) -> Canvas {
        let cells = width * height;
        Canvas {
            width,
            height,
            chars: vec![None; cells],
            links: vec![0; cells],
            solid: vec![false; cells],
        }
    }

    fn index(&self, x: usize, y: usize) -> Option<usize> {
        (x < self.width && y < self.height).then(|| y * self.width + x)
    }

    /// Add `links` to cell (x, y); `dotted` lines draw dashed when no solid
    /// line shares the cell.
    pub fn link(&mut self, x: usize, y: usize, links: u8, dotted: bool) {
        if let Some(i) = self.index(x, y) {
            self.links[i] |= links;
            self.solid[i] |= !dotted;
        }
    }

    /// A vertical line from row `y0` to `y1` (inclusive) at column `x`. Its
    /// end cells link only inwards, unless `open` asks them to link onwards
    /// too (`(top, bottom)`).
    pub fn vline(&mut self, x: usize, y0: usize, y1: usize, open: (bool, bool), dotted: bool) {
        let (top, bottom) = (y0.min(y1), y0.max(y1));
        for y in top..=bottom {
            let mut l = 0;
            if y > top || open.0 {
                l |= UP;
            }
            if y < bottom || open.1 {
                l |= DOWN;
            }
            self.link(x, y, l, dotted);
        }
    }

    /// Write `c` at (x, y), over any line.
    pub fn put(&mut self, x: usize, y: usize, c: char) {
        if let Some(i) = self.index(x, y) {
            self.chars[i] = Some(c);
        }
    }

    /// Write `s` from (x, y) rightwards; a wide character takes two cells.
    pub fn text(&mut self, x: usize, y: usize, s: &str) {
        let mut x = x;
        for c in s.chars() {
            let w = c.width().unwrap_or(0);
            if w == 0 {
                continue;
            }
            self.put(x, y, c);
            if w == 2 {
                self.put(x + 1, y, WIDE_TAIL);
            }
            x += w;
        }
    }

    /// A box with its top-left corner at (x, y), `w`×`h` cells, its inside
    /// cleared of lines.
    pub fn frame(&mut self, x: usize, y: usize, w: usize, h: usize, style: &BoxStyle) {
        for dy in 0..h {
            for dx in 0..w {
                let c = match (dx, dy) {
                    (0, 0) => style.top_left,
                    (dx, 0) if dx == w - 1 => style.top_right,
                    (0, dy) if dy == h - 1 => style.bottom_left,
                    (dx, dy) if dx == w - 1 && dy == h - 1 => style.bottom_right,
                    (_, 0) => style.horizontal,
                    (_, dy) if dy == h - 1 => style.horizontal,
                    (0, _) => style.vertical,
                    (dx, _) if dx == w - 1 => style.vertical,
                    _ => ' ',
                };
                self.put(x + dx, y + dy, c);
            }
        }
    }

    /// The character at (x, y), if one was written.
    pub fn get(&self, x: usize, y: usize) -> Option<char> {
        self.index(x, y).and_then(|i| self.chars[i])
    }

    /// The finished rows, trailing spaces trimmed.
    pub fn rows(&self) -> Vec<String> {
        (0..self.height)
            .map(|y| {
                let mut row = String::with_capacity(self.width);
                for x in 0..self.width {
                    let i = y * self.width + x;
                    match self.chars[i] {
                        Some(WIDE_TAIL) => {}
                        Some(c) => row.push(c),
                        None => row.push(glyph(self.links[i], self.solid[i])),
                    }
                }
                row.trim_end().to_owned()
            })
            .collect()
    }
}

/// The box-drawing character for a cell's links.
fn glyph(links: u8, solid: bool) -> char {
    match links {
        0 => ' ',
        l if l & (LEFT | RIGHT) == 0 => {
            if solid {
                '│'
            } else {
                '┆'
            }
        }
        l if l & (UP | DOWN) == 0 => {
            if solid {
                '─'
            } else {
                '┄'
            }
        }
        l if l == DOWN | RIGHT => '┌',
        l if l == DOWN | LEFT => '┐',
        l if l == UP | RIGHT => '└',
        l if l == UP | LEFT => '┘',
        l if l == UP | DOWN | RIGHT => '├',
        l if l == UP | DOWN | LEFT => '┤',
        l if l == DOWN | LEFT | RIGHT => '┬',
        l if l == UP | LEFT | RIGHT => '┴',
        _ => '┼',
    }
}
```

Create `src/tui/diagram/flowchart.rs`:

```rust
//! Flowcharts (`graph`/`flowchart`): parsing, a layered layout and drawing.
//!
//! The layout is the classic layered one: cycles are broken by reversing
//! back edges, nodes go to layers by longest path, long edges get dummy
//! nodes, barycenter sweeps order each layer, and an isotonic fit places
//! nodes as close to their neighbours as their widths allow. Edges are
//! orthogonal: down from the source, along a track between the layers,
//! down into the target. Everything is laid out on an abstract (cross,
//! main) grid and mapped to TD, BT, LR or RL only when drawn.

use std::collections::HashMap;

use super::canvas::{self, Canvas, DOWN, LEFT, RIGHT, UP};
use super::text;
use super::{Unsupported, MAX_COLS, MAX_EDGES, MAX_NODES, MAX_ROWS};

#[derive(Clone, Copy, PartialEq, Debug)]
enum Dir {
    Td,
    Bt,
    Lr,
    Rl,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Shape {
    Rect,
    Rounded,
    Decision,
}

struct Node {
    label: Vec<String>,
    shape: Shape,
}

#[derive(Clone)]
struct Edge {
    from: usize,
    to: usize,
    label: Vec<String>,
    dotted: bool,
    head: bool,
}

#[derive(Default)]
struct Graph {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    index: HashMap<String, usize>,
}

/// The drawing of a flowchart whose header was `graph <rest>` (or
/// `flowchart <rest>`), with `lines` its statements.
pub fn render<'a>(
    rest: &'a str,
    lines: impl Iterator<Item = &'a str>,
) -> Result<Vec<String>, Unsupported> {
    let (dir, first) = match rest.split_once(';') {
        Some((dir, first)) => (dir.trim(), first),
        None => (rest.trim(), ""),
    };
    let dir = match dir {
        "" | "TD" | "TB" => Dir::Td,
        "BT" => Dir::Bt,
        "LR" => Dir::Lr,
        "RL" => Dir::Rl,
        other => {
            let other: String = other.chars().take(24).collect();
            return Err(Unsupported::new(format!("unsupported direction: {other}")));
        }
    };
    let mut graph = Graph::default();
    for line in std::iter::once(first).chain(lines) {
        for statement in split_statements(line) {
            graph.statement(statement)?;
        }
    }
    if graph.nodes.is_empty() {
        return Err(Unsupported::new("empty flowchart"));
    }
    Layout::new(&graph, dir).draw(&graph)
}

/// `line` split on the `;` that are not inside quotes, brackets or `|…|`.
fn split_statements(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut quote, mut pipe, mut start) = (0usize, false, false, 0);
    for (i, c) in line.char_indices() {
        match c {
            '"' => quote = !quote,
            '[' | '(' | '{' if !quote => depth += 1,
            ']' | ')' | '}' if !quote => depth = depth.saturating_sub(1),
            '|' if !quote && depth == 0 => pipe = !pipe,
            ';' if !quote && !pipe && depth == 0 => {
                out.push(&line[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&line[start..]);
    out
}

// ── Parsing ──────────────────────────────────────────────────────────────

/// A link between two node groups.
struct Link {
    dotted: bool,
    head: bool,
    both: bool,
    invisible: bool,
    label: Option<String>,
}

/// Words that start a statement that draws nothing.
const IGNORED: [&str; 8] = [
    "subgraph",
    "end",
    "direction",
    "style",
    "classDef",
    "class",
    "click",
    "linkStyle",
];

impl Graph {
    fn statement(&mut self, s: &str) -> Result<(), Unsupported> {
        let s = s.trim();
        let word = s.split(|c: char| c.is_whitespace()).next().unwrap_or("");
        if s.is_empty() || IGNORED.contains(&word) {
            return Ok(());
        }
        let chars: Vec<char> = s.chars().collect();
        let mut p = Parser { s: &chars, i: 0 };
        let mut left = self.group(&mut p)?;
        loop {
            p.skip_ws();
            if p.done() {
                return Ok(());
            }
            let Some(mut link) = p.link() else {
                return Err(Unsupported::new(format!("cannot parse: {}", fit_reason(s))));
            };
            p.skip_ws();
            if p.peek() == Some('|') {
                link.label = Some(p.pipe_label()?);
                p.skip_ws();
            }
            let right = self.group(&mut p)?;
            let label = text::wrap(&text::clean(link.label.as_deref().unwrap_or("")));
            for &a in &left {
                for &b in &right {
                    if link.invisible {
                        continue;
                    }
                    self.edge(a, b, label.clone(), &link)?;
                    if link.both {
                        self.edge(b, a, Vec::new(), &link)?;
                    }
                }
            }
            left = right;
        }
    }

    /// Nodes joined by `&`.
    fn group(&mut self, p: &mut Parser) -> Result<Vec<usize>, Unsupported> {
        let mut ids = Vec::new();
        loop {
            p.skip_ws();
            ids.push(self.node(p)?);
            p.skip_ws();
            if p.peek() != Some('&') {
                return Ok(ids);
            }
            p.i += 1;
        }
    }

    /// A node reference, with or without its shape and text.
    fn node(&mut self, p: &mut Parser) -> Result<usize, Unsupported> {
        let id = p.ident();
        if id.is_empty() {
            return Err(Unsupported::new("expected a node"));
        }
        let shape = p.shape()?;
        p.class_suffix();
        let index = match self.index.get(&id) {
            Some(&i) => i,
            None => {
                if self.nodes.len() == MAX_NODES {
                    return Err(Unsupported::new(format!("more than {MAX_NODES} nodes")));
                }
                self.nodes.push(Node {
                    label: text::wrap(&text::clean(&id)),
                    shape: Shape::Rect,
                });
                self.index.insert(id, self.nodes.len() - 1);
                self.nodes.len() - 1
            }
        };
        if let Some((shape, label)) = shape {
            let node = &mut self.nodes[index];
            node.shape = shape;
            node.label = text::wrap(&text::clean(&label));
            if node.label.is_empty() {
                node.label = vec![String::new()];
            }
        }
        Ok(index)
    }

    fn edge(
        &mut self,
        from: usize,
        to: usize,
        label: Vec<String>,
        link: &Link,
    ) -> Result<(), Unsupported> {
        if self.edges.len() == MAX_EDGES {
            return Err(Unsupported::new(format!("more than {MAX_EDGES} edges")));
        }
        self.edges.push(Edge {
            from,
            to,
            label,
            dotted: link.dotted,
            head: link.head,
        });
        Ok(())
    }
}

/// A statement, shortened for an error message.
fn fit_reason(s: &str) -> String {
    text::fit(&text::clean(s).replace('\n', " "), 40)
}

struct Parser<'a> {
    s: &'a [char],
    i: usize,
}

/// Node shapes: opening and closing delimiters. Longer openings first.
const SHAPES: [(&str, &str, Shape); 12] = [
    ("([", "])", Shape::Rounded),
    ("((", "))", Shape::Rounded),
    ("[[", "]]", Shape::Rect),
    ("[(", ")]", Shape::Rect),
    ("{{", "}}", Shape::Rect),
    ("[/", "/]", Shape::Rect),
    ("[\\", "\\]", Shape::Rect),
    ("[/", "\\]", Shape::Rect),
    ("[", "]", Shape::Rect),
    ("(", ")", Shape::Rounded),
    ("{", "}", Shape::Decision),
    (">", "]", Shape::Rect),
];

impl Parser<'_> {
    fn done(&self) -> bool {
        self.i >= self.s.len()
    }

    fn peek(&self) -> Option<char> {
        self.s.get(self.i).copied()
    }

    fn at(&self, i: usize) -> Option<char> {
        self.s.get(i).copied()
    }

    fn starts_with(&self, i: usize, pat: &str) -> bool {
        let mut j = i;
        for c in pat.chars() {
            if self.at(j) != Some(c) {
                return false;
            }
            j += 1;
        }
        true
    }

    fn skip_ws(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.i += 1;
        }
    }

    /// A node id: letters, digits, `_`, and `-` between two of those.
    fn ident(&mut self) -> String {
        let start = self.i;
        while let Some(c) = self.peek() {
            let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
            if word(Some(c)) || (c == '-' && self.i > start && word(self.at(self.i + 1))) {
                self.i += 1;
            } else {
                break;
            }
        }
        self.s[start..self.i].iter().collect()
    }

    /// A shape and its text, if one follows.
    fn shape(&mut self) -> Result<Option<(Shape, String)>, Unsupported> {
        let Some(&(open, _, shape)) = SHAPES.iter().find(|(o, _, _)| self.starts_with(self.i, o))
        else {
            return Ok(None);
        };
        let start = self.i + open.chars().count();
        let closers: Vec<&str> = SHAPES
            .iter()
            .filter(|(o, _, _)| *o == open)
            .map(|(_, c, _)| *c)
            .collect();
        let mut j = start;
        // A quoted text may contain the closing delimiter.
        if self.at(j) == Some('"') {
            j += 1;
            while self.at(j).is_some_and(|c| c != '"') {
                j += 1;
            }
            j += 1;
        }
        while j < self.s.len() {
            if let Some(close) = closers.iter().find(|c| self.starts_with(j, c)) {
                let raw: String = self.s[start..j].iter().collect();
                let raw = raw.trim();
                let label = raw
                    .strip_prefix('"')
                    .and_then(|r| r.strip_suffix('"'))
                    .unwrap_or(raw);
                self.i = j + close.chars().count();
                return Ok(Some((shape, label.to_owned())));
            }
            j += 1;
        }
        Err(Unsupported::new("unclosed node shape"))
    }

    /// Skip a `:::class` suffix.
    fn class_suffix(&mut self) {
        if self.starts_with(self.i, ":::") {
            self.i += 3;
            self.ident();
        }
    }

    /// A link (`-->`, `-.->`, `== t ==>`, …), if one starts here.
    fn link(&mut self) -> Option<Link> {
        let start = self.i;
        let both = self.peek() == Some('<');
        if both {
            self.i += 1;
        }
        let run = self.run();
        if run.chars().count() < 2 {
            self.i = start;
            return None;
        }
        let tip = self.head();
        let mut head = both || tip;
        let mut dotted = run.contains('.');
        let mut label = None;
        if !head && matches!(run.as_str(), "--" | "==" | "-.") && self.peek() == Some(' ') {
            // `-- text -->`: the text runs to the closing link.
            let close = self.closer(&run)?;
            label = Some(
                self.s[self.i..close]
                    .iter()
                    .collect::<String>()
                    .trim()
                    .to_owned(),
            );
            self.i = close;
            let end = self.run();
            dotted |= end.contains('.');
            head = self.head();
        }
        Some(Link {
            dotted,
            head,
            both,
            invisible: run.contains('~'),
            label,
        })
    }

    /// A run of link characters.
    fn run(&mut self) -> String {
        let start = self.i;
        while self
            .peek()
            .is_some_and(|c| matches!(c, '-' | '.' | '=' | '~'))
        {
            self.i += 1;
        }
        self.s[start..self.i].iter().collect()
    }

    /// An arrowhead: `>`, or `o`/`x` before a space, `|` or the end.
    fn head(&mut self) -> bool {
        match self.peek() {
            Some('>') => {
                self.i += 1;
                true
            }
            Some('o' | 'x')
                if self
                    .at(self.i + 1)
                    .is_none_or(|c| c.is_whitespace() || c == '|') =>
            {
                self.i += 1;
                true
            }
            _ => false,
        }
    }

    /// Where the link that closes a `-- text` opening starts.
    fn closer(&self, open: &str) -> Option<usize> {
        let ends: &[&str] = match open {
            "--" => &["-->", "---", "--o ", "--x "],
            "==" => &["==>", "==="],
            _ => &[".->", ".-"],
        };
        (self.i..self.s.len()).find(|&j| ends.iter().any(|e| self.starts_with(j, e)))
    }

    /// `|text|`.
    fn pipe_label(&mut self) -> Result<String, Unsupported> {
        let start = self.i + 1;
        let Some(len) = self.s[start..].iter().position(|&c| c == '|') else {
            return Err(Unsupported::new("unclosed edge label"));
        };
        self.i = start + len + 1;
        let raw: String = self.s[start..start + len].iter().collect();
        let raw = raw.trim();
        Ok(raw
            .strip_prefix('"')
            .and_then(|r| r.strip_suffix('"'))
            .unwrap_or(raw)
            .to_owned())
    }
}

// ── Layout ───────────────────────────────────────────────────────────────

/// A node of the layered graph: a real node or a dummy on a long edge.
#[derive(Clone, Copy, PartialEq)]
enum LNode {
    Real(usize),
    Dummy,
}

/// One layer-to-next-layer piece of an edge.
struct Seg {
    from: usize,
    to: usize,
    edge: usize,
    first: bool,
    last: bool,
}

/// Where an edge's label is drawn.
#[derive(Clone, Copy, PartialEq)]
enum Place {
    /// Beside the segment's end, above its target.
    Target(usize),
    /// Beside the segment's start, below its source.
    Source(usize),
}

struct Layout {
    dir: Dir,
    edges: Vec<Edge>,
    /// Kept edges drawn upwards (cycles broken).
    reversed: Vec<bool>,
    lnodes: Vec<LNode>,
    layer_of: Vec<usize>,
    layers: Vec<Vec<usize>>,
    segs: Vec<Seg>,
    /// Per lnode: the slot offset of each segment's port, out (main+) side
    /// and in (main−) side, by segment index.
    out_slot: HashMap<usize, i64>,
    in_slot: HashMap<usize, i64>,
    /// Per lnode: cross size, main size, and the cross extent used for
    /// spacing (labels included).
    cross: Vec<i64>,
    main: Vec<i64>,
    extent: Vec<i64>,
    labels: HashMap<usize, Place>,
    /// Edges listed under the drawing instead (index into `edges`), and
    /// whether only their label is missing from the drawing.
    unplaced: Vec<usize>,
}

impl Layout {
    fn new(graph: &Graph, dir: Dir) -> Layout {
        let n = graph.nodes.len();
        // Self-loops and repeats cannot be drawn: they are listed.
        let mut edges = Vec::new();
        let mut unplaced_edges = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for e in &graph.edges {
            if e.from == e.to || !seen.insert((e.from, e.to)) {
                unplaced_edges.push(e.clone());
            } else {
                edges.push(e.clone());
            }
        }
        let reversed = break_cycles(n, &edges);
        let layer_of_real = assign_layers(n, &edges, &reversed);
        let mut layout = Layout {
            dir,
            edges,
            reversed,
            lnodes: (0..n).map(LNode::Real).collect(),
            layer_of: layer_of_real,
            layers: Vec::new(),
            segs: Vec::new(),
            out_slot: HashMap::new(),
            in_slot: HashMap::new(),
            cross: Vec::new(),
            main: Vec::new(),
            extent: Vec::new(),
            labels: HashMap::new(),
            unplaced: Vec::new(),
        };
        // Listed edges go after the kept ones, so their indices stay apart.
        for e in unplaced_edges {
            layout.unplaced.push(layout.edges.len());
            layout.reversed.push(false);
            layout.edges.push(e);
        }
        layout.split_long_edges();
        layout.order();
        layout.ports(graph);
        layout
    }

    /// The number of edges that are drawn (the rest are listed).
    fn drawn_edges(&self) -> usize {
        self.edges.len() - self.unplaced.len()
    }

    /// Segments for every drawn edge, with dummy nodes on long ones.
    fn split_long_edges(&mut self) {
        for e in 0..self.drawn_edges() {
            let (mut a, b) = if self.reversed[e] {
                (self.edges[e].to, self.edges[e].from)
            } else {
                (self.edges[e].from, self.edges[e].to)
            };
            let (la, lb) = (self.layer_of[a], self.layer_of[b]);
            for layer in la + 1..=lb {
                let next = if layer == lb {
                    b
                } else {
                    self.lnodes.push(LNode::Dummy);
                    self.layer_of.push(layer);
                    self.lnodes.len() - 1
                };
                self.segs.push(Seg {
                    from: a,
                    to: next,
                    edge: e,
                    first: layer == la + 1,
                    last: layer == lb,
                });
                a = next;
            }
        }
        let count = self.layer_of.iter().max().map_or(0, |&l| l + 1);
        self.layers = vec![Vec::new(); count];
        for (v, &l) in self.layer_of.iter().enumerate() {
            self.layers[l].push(v);
        }
    }

    /// Order each layer by barycenter sweeps, keeping the order with the
    /// fewest crossings.
    fn order(&mut self) {
        let mut preds = vec![Vec::new(); self.lnodes.len()];
        let mut succs = vec![Vec::new(); self.lnodes.len()];
        for s in &self.segs {
            preds[s.to].push(s.from);
            succs[s.from].push(s.to);
        }
        let mut best = self.layers.clone();
        let mut best_crossings = self.crossings(&best);
        let mut layers = self.layers.clone();
        for sweep in 0..8 {
            let down = sweep % 2 == 0;
            let range: Vec<usize> = if down {
                (1..layers.len()).collect()
            } else {
                (0..layers.len().saturating_sub(1)).rev().collect()
            };
            for l in range {
                let reference = if down { l - 1 } else { l + 1 };
                let pos = positions(&layers[reference]);
                let neighbours = if down { &preds } else { &succs };
                let mut keyed: Vec<(f64, usize, usize)> = layers[l]
                    .iter()
                    .enumerate()
                    .map(|(i, &v)| {
                        let ps: Vec<f64> = neighbours[v]
                            .iter()
                            .filter_map(|u| pos.get(u).map(|&p| p as f64))
                            .collect();
                        let key = if ps.is_empty() {
                            i as f64
                        } else {
                            ps.iter().sum::<f64>() / ps.len() as f64
                        };
                        (key, i, v)
                    })
                    .collect();
                keyed.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
                layers[l] = keyed.into_iter().map(|(_, _, v)| v).collect();
            }
            let crossings = self.crossings(&layers);
            if crossings < best_crossings {
                best_crossings = crossings;
                best = layers.clone();
            }
        }
        self.layers = best;
    }

    /// Edge crossings between adjacent layers in `layers`.
    fn crossings(&self, layers: &[Vec<usize>]) -> usize {
        let mut pos = vec![0usize; self.lnodes.len()];
        for layer in layers {
            for (i, &v) in layer.iter().enumerate() {
                pos[v] = i;
            }
        }
        let mut by_gap: Vec<Vec<(usize, usize)>> = vec![Vec::new(); layers.len()];
        for s in &self.segs {
            by_gap[self.layer_of[s.from]].push((pos[s.from], pos[s.to]));
        }
        let mut count = 0;
        for pairs in &by_gap {
            for (i, a) in pairs.iter().enumerate() {
                for b in &pairs[i + 1..] {
                    if (a.0 < b.0 && a.1 > b.1) || (a.0 > b.0 && a.1 < b.1) {
                        count += 1;
                    }
                }
            }
        }
        count
    }

    /// Port slots, node sizes and label places.
    fn ports(&mut self, graph: &Graph) {
        let lr = matches!(self.dir, Dir::Lr | Dir::Rl);
        let count = self.lnodes.len();
        // Per lnode and side: the slots in use. Slot 0 is shared by the
        // forward edges; each reversed edge gets its own.
        let mut out_groups: Vec<usize> = vec![0; count];
        let mut in_groups: Vec<usize> = vec![0; count];
        let mut out_shared = vec![false; count];
        let mut in_shared = vec![false; count];
        for s in &self.segs {
            let reversed = self.reversed[s.edge];
            let real_from = matches!(self.lnodes[s.from], LNode::Real(_));
            let real_to = matches!(self.lnodes[s.to], LNode::Real(_));
            if !reversed || !real_from {
                out_shared[s.from] = true;
            }
            if !reversed || !real_to {
                in_shared[s.to] = true;
            }
        }
        for (i, s) in self.segs.iter().enumerate() {
            let reversed = self.reversed[s.edge];
            let real_from = matches!(self.lnodes[s.from], LNode::Real(_));
            let real_to = matches!(self.lnodes[s.to], LNode::Real(_));
            let out = if reversed && real_from {
                let k = out_groups[s.from] + usize::from(out_shared[s.from]);
                out_groups[s.from] += 1;
                slot_offset(k, lr)
            } else {
                0
            };
            let inn = if reversed && real_to {
                let k = in_groups[s.to] + usize::from(in_shared[s.to]);
                in_groups[s.to] += 1;
                slot_offset(k, lr)
            } else {
                0
            };
            self.out_slot.insert(i, out);
            self.in_slot.insert(i, inn);
        }
        // Sizes. Cross is along the layer, main across layers.
        for (v, lnode) in self.lnodes.iter().enumerate() {
            let (cross, main) = match *lnode {
                LNode::Dummy => (1, 0),
                LNode::Real(n) => {
                    let label = &graph.nodes[n].label;
                    let slots = out_groups[v] + usize::from(out_shared[v]);
                    let slots = slots.max(in_groups[v] + usize::from(in_shared[v]));
                    let need = 2 * slot_offset(slots.saturating_sub(1), lr) + 3;
                    let w = (text::widest(label) as i64 + 4).max(5);
                    let h = label.len() as i64 + 2;
                    if lr {
                        (h.max(need), w)
                    } else {
                        (w.max(need), h)
                    }
                }
            };
            self.cross.push(cross);
            self.main.push(main);
        }
        self.extent = self.cross.clone();
        // Labels: beside a port only that edge uses, else listed.
        let mut ins: HashMap<(usize, i64), usize> = HashMap::new();
        let mut outs: HashMap<(usize, i64), usize> = HashMap::new();
        for (i, s) in self.segs.iter().enumerate() {
            *ins.entry((s.to, self.in_slot[&i])).or_default() += 1;
            *outs.entry((s.from, self.out_slot[&i])).or_default() += 1;
        }
        for (i, s) in self.segs.iter().enumerate() {
            let edge = &self.edges[s.edge];
            if edge.label.is_empty() {
                continue;
            }
            let place = if s.last && ins[&(s.to, self.in_slot[&i])] == 1 {
                Place::Target(i)
            } else if s.first && outs[&(s.from, self.out_slot[&i])] == 1 {
                Place::Source(i)
            } else {
                continue;
            };
            if self.labels.contains_key(&s.edge) {
                continue;
            }
            let (node, _) = match place {
                Place::Target(_) => (s.to, ()),
                Place::Source(_) => (s.from, ()),
            };
            let (cross, _) = self.label_size(&edge.label);
            self.extent[node] = self.extent[node].max(cross + 2);
            self.labels.insert(s.edge, place);
        }
        for e in 0..self.drawn_edges() {
            if !self.edges[e].label.is_empty() && !self.labels.contains_key(&e) {
                self.unplaced.push(e);
            }
        }
    }

    /// A label's (cross, main) size beside a line.
    fn label_size(&self, label: &[String]) -> (i64, i64) {
        let w = text::widest(label) as i64;
        let n = label.len() as i64;
        match self.dir {
            Dir::Td | Dir::Bt => (w, n + 1),
            Dir::Lr | Dir::Rl => (2 * n - 1, w + 4),
        }
    }

    /// Cross positions (centers) of every lnode.
    fn place_cross(&self) -> Vec<i64> {
        let mut preds = vec![Vec::new(); self.lnodes.len()];
        let mut succs = vec![Vec::new(); self.lnodes.len()];
        for s in &self.segs {
            preds[s.to].push(s.from);
            succs[s.from].push(s.to);
        }
        let seps: Vec<Vec<f64>> = self
            .layers
            .iter()
            .map(|layer| {
                layer
                    .windows(2)
                    .map(|w| self.sep(w[0], w[1]) as f64)
                    .collect()
            })
            .collect();
        let mut x = vec![0f64; self.lnodes.len()];
        for (layer, seps) in self.layers.iter().zip(&seps) {
            let mut at = 0.0;
            for (i, &v) in layer.iter().enumerate() {
                if i > 0 {
                    at += seps[i - 1];
                }
                x[v] = at;
            }
        }
        for pass in 0..8 {
            let down = pass % 2 == 0;
            let order: Vec<usize> = if down {
                (1..self.layers.len()).collect()
            } else {
                (0..self.layers.len().saturating_sub(1)).rev().collect()
            };
            for l in order {
                let layer = &self.layers[l];
                let neighbours = if down { &preds } else { &succs };
                // Real nodes pull harder than dummies, so boxes line up
                // and long edges bend around them.
                let desired: Vec<f64> = layer
                    .iter()
                    .map(|&v| {
                        let ns = &neighbours[v];
                        let total: f64 = ns.iter().map(|&u| self.weight(u)).sum();
                        if ns.is_empty() {
                            x[v]
                        } else {
                            ns.iter().map(|&u| self.weight(u) * x[u]).sum::<f64>() / total
                        }
                    })
                    .collect();
                let weights: Vec<f64> = layer.iter().map(|&v| self.weight(v)).collect();
                let placed = isotonic(&desired, &weights, &seps[l]);
                for (&v, p) in layer.iter().zip(placed) {
                    x[v] = p;
                }
            }
        }
        // Whole cells, still apart, and nothing left of column 0.
        let mut out = vec![0i64; self.lnodes.len()];
        for (layer, seps) in self.layers.iter().zip(&seps) {
            for (i, &v) in layer.iter().enumerate() {
                let mut p = x[v].round() as i64;
                if i > 0 {
                    p = p.max(out[layer[i - 1]] + seps[i - 1] as i64);
                }
                out[v] = p;
            }
        }
        let min = (0..self.lnodes.len())
            .map(|v| out[v] - self.extent[v] / 2)
            .min()
            .unwrap_or(0);
        out.iter().map(|p| p - min).collect()
    }

    /// How hard `v` pulls in placement: dummies give way to real nodes.
    fn weight(&self, v: usize) -> f64 {
        match self.lnodes[v] {
            LNode::Real(_) => 1.0,
            LNode::Dummy => 0.1,
        }
    }

    /// The least distance between the centers of neighbours `a` and `b`.
    fn sep(&self, a: usize, b: usize) -> i64 {
        let real = |v: usize| matches!(self.lnodes[v], LNode::Real(_));
        let lr = matches!(self.dir, Dir::Lr | Dir::Rl);
        let gap = if real(a) && real(b) && !lr { 3 } else { 2 };
        let hi = self.extent[a] - 1 - self.extent[a] / 2;
        let lo = self.extent[b] / 2;
        hi + gap + lo
    }

    // ── Drawing ──────────────────────────────────────────────────────────

    fn draw(&self, graph: &Graph) -> Result<Vec<String>, Unsupported> {
        let cx = self.place_cross();
        // A reversed edge's slots lie on the side towards its other end at
        // its start, and away from it at its end: then, on the top track,
        // it crosses neither the shared stub nor the shared drop.
        let mut ports: Vec<(i64, i64)> = (self.segs.iter().enumerate())
            .map(|(i, s)| {
                let sign = if cx[s.to] >= cx[s.from] { 1 } else { -1 };
                (sign * self.out_slot[&i], sign * self.in_slot[&i])
            })
            .collect();
        // Unless the other side crosses fewer of the gap's other lines. A
        // reversed edge has the top track: its horizontal crosses the other
        // stubs below its span, and its drop crosses the other horizontals.
        for (i, s) in self.segs.iter().enumerate() {
            if self.out_slot[&i] == 0 && self.in_slot[&i] == 0 {
                continue;
            }
            let g = self.layer_of[s.from];
            let others: Vec<(i64, i64)> = (self.segs.iter().enumerate())
                .filter(|&(j, o)| j != i && self.layer_of[o.from] == g)
                .map(|(j, o)| (cx[o.from] + ports[j].0, cx[o.to] + ports[j].1))
                .collect();
            let crossed = |(a, b): (i64, i64)| {
                let (sx, tx) = (cx[s.from] + a, cx[s.to] + b);
                let (lo, hi) = (sx.min(tx), sx.max(tx));
                let stubs = others.iter().filter(|o| lo <= o.0 && o.0 <= hi).count();
                let drops = (others.iter())
                    .filter(|o| o.0.min(o.1) <= tx && tx <= o.0.max(o.1))
                    .count();
                stubs + drops
            };
            let (a, b) = (self.out_slot[&i], self.in_slot[&i]);
            let best = [ports[i], (a, b), (-a, b), (a, -b), (-a, -b)]
                .into_iter()
                .min_by_key(|&p| crossed(p))
                .expect("candidates");
            ports[i] = best;
        }
        let gaps = self.layers.len().saturating_sub(1);
        // Per gap: the track of each segment, and the band sizes.
        let mut track = vec![0i64; self.segs.len()];
        let mut tracks = vec![1i64; gaps];
        let mut src_band = vec![0i64; gaps];
        let mut dst_band = vec![0i64; gaps];
        for g in 0..gaps {
            // Nets: segments that leave the same port share a track.
            let mut nets: Vec<((usize, i64), i64, i64, Vec<usize>)> = Vec::new();
            for (i, s) in self.segs.iter().enumerate() {
                if self.layer_of[s.from] != g {
                    continue;
                }
                let port = (s.from, ports[i].0);
                let sx = cx[s.from] + ports[i].0;
                let tx = cx[s.to] + ports[i].1;
                match nets.iter_mut().find(|n| n.0 == port) {
                    Some(net) => {
                        net.1 = net.1.min(tx);
                        net.2 = net.2.max(tx);
                        net.3.push(i);
                    }
                    None => nets.push((port, sx.min(tx), sx.max(tx), vec![i])),
                }
            }
            // Reversed edges first, so they get the top tracks.
            let reversed =
                |n: &(_, _, _, Vec<usize>)| n.3.iter().any(|&i| self.reversed[self.segs[i].edge]);
            nets.sort_by_key(|n| (!reversed(n), n.1, n.2));
            let mut ends: Vec<i64> = Vec::new();
            for (_, lo, hi, members) in &nets {
                if lo == hi {
                    continue;
                }
                let t = match ends.iter().position(|&end| end + 2 <= *lo) {
                    Some(t) => t,
                    None => {
                        ends.push(i64::MIN);
                        ends.len() - 1
                    }
                };
                ends[t] = *hi;
                for &i in members {
                    track[i] = t as i64;
                }
            }
            tracks[g] = (ends.len() as i64).max(1);
        }
        for (&e, place) in &self.labels {
            let (_, main) = self.label_size(&self.edges[e].label);
            match *place {
                Place::Target(i) => {
                    let g = self.layer_of[self.segs[i].from];
                    dst_band[g] = dst_band[g].max(main);
                }
                Place::Source(i) => {
                    let g = self.layer_of[self.segs[i].from];
                    src_band[g] = src_band[g].max(main);
                }
            }
        }
        // A `▲` under a box needs a row of line below it before a turn.
        for s in &self.segs {
            if self.reversed[s.edge] && self.edges[s.edge].head && s.first {
                let g = self.layer_of[s.from];
                src_band[g] = src_band[g].max(1);
            }
        }
        // Main positions: layer starts and the rows of each gap.
        let mut layer_start = Vec::with_capacity(self.layers.len());
        let mut layer_size = Vec::with_capacity(self.layers.len());
        let mut m = 0i64;
        for (l, layer) in self.layers.iter().enumerate() {
            layer_start.push(m);
            let size = layer
                .iter()
                .map(|&v| self.main[v])
                .max()
                .unwrap_or(1)
                .max(1);
            layer_size.push(size);
            m += size;
            if l < gaps {
                m += src_band[l] + tracks[l] + dst_band[l] + 1;
            }
        }
        let total_main = m;
        let total_cross = (0..self.lnodes.len())
            .map(|v| cx[v] - self.extent[v] / 2 + self.extent[v])
            .max()
            .unwrap_or(1);
        let listed = self.listing(graph);
        let (w, h) = match self.dir {
            Dir::Td | Dir::Bt => (total_cross, total_main),
            Dir::Lr | Dir::Rl => (total_main, total_cross),
        };
        let listed_rows = if listed.is_empty() {
            0
        } else {
            listed.len() + 1
        };
        if w as usize > MAX_COLS || h as usize + listed_rows > MAX_ROWS {
            return Err(Unsupported::new("diagram too large"));
        }
        let mut pen = Pen {
            dir: self.dir,
            total_main,
            canvas: Canvas::new(w as usize, h as usize),
        };
        // Lines first, so boxes and text are drawn over them.
        for (i, s) in self.segs.iter().enumerate() {
            let g = self.layer_of[s.from];
            let edge = &self.edges[s.edge];
            let sx = cx[s.from] + ports[i].0;
            let tx = cx[s.to] + ports[i].1;
            let start = match self.lnodes[s.from] {
                LNode::Real(_) => layer_start[g] + self.main[s.from],
                LNode::Dummy => layer_start[g],
            };
            let gap = layer_start[g] + layer_size[g];
            let mt = gap + src_band[g] + track[i];
            let arrow = layer_start[g + 1] - 1;
            pen.vline(sx, start, mt, (true, false), edge.dotted);
            if sx != tx {
                pen.hline(mt, sx, tx, edge.dotted);
            }
            pen.vline(tx, mt, arrow, (false, true), edge.dotted);
            if let LNode::Dummy = self.lnodes[s.to] {
                let l = g + 1;
                let end = layer_start[l] + layer_size[l] - 1;
                pen.vline(tx, layer_start[l], end, (true, true), edge.dotted);
            }
        }
        // Boxes.
        for (v, lnode) in self.lnodes.iter().enumerate() {
            let LNode::Real(n) = *lnode else { continue };
            let node = &graph.nodes[n];
            let l = self.layer_of[v];
            let c0 = cx[v] - self.cross[v] / 2;
            let (x, y, w, h) = pen.rect(c0, layer_start[l], self.cross[v], self.main[v]);
            let style = match node.shape {
                Shape::Rect => &canvas::RECT,
                Shape::Rounded => &canvas::ROUNDED,
                Shape::Decision => &canvas::DECISION,
            };
            pen.canvas.frame(x, y, w, h, style);
            let top = y + (h - node.label.len()) / 2;
            for (i, line) in node.label.iter().enumerate() {
                let lw = text::width(line);
                pen.canvas.text(x + (w - lw) / 2, top + i, line);
            }
        }
        // Ports and arrowheads.
        for (i, s) in self.segs.iter().enumerate() {
            let g = self.layer_of[s.from];
            let edge = &self.edges[s.edge];
            let reversed = self.reversed[s.edge];
            let sx = cx[s.from] + ports[i].0;
            let tx = cx[s.to] + ports[i].1;
            if let LNode::Real(_) = self.lnodes[s.from] {
                let border = layer_start[g] + self.main[s.from] - 1;
                if reversed && edge.head {
                    pen.arrow(sx, border + 1, false);
                } else {
                    pen.tee(sx, border, true);
                }
            }
            if let LNode::Real(_) = self.lnodes[s.to] {
                let top = layer_start[g + 1];
                if !reversed && edge.head {
                    pen.arrow(tx, top - 1, true);
                } else if !pen.is_arrow(tx, top - 1) {
                    pen.tee(tx, top, false);
                }
            }
        }
        // Labels, over the lines they sit on.
        for (&e, place) in &self.labels {
            let label = &self.edges[e].label;
            let (_, size) = self.label_size(label);
            let (at, main0) = match *place {
                Place::Target(i) => {
                    let s = &self.segs[i];
                    let arrow = layer_start[self.layer_of[s.to]] - 1;
                    (cx[s.to] + ports[i].1, arrow - size)
                }
                Place::Source(i) => {
                    let s = &self.segs[i];
                    let g = self.layer_of[s.from];
                    let start = match self.lnodes[s.from] {
                        LNode::Real(_) => layer_start[g] + self.main[s.from],
                        LNode::Dummy => layer_start[g],
                    };
                    (cx[s.from] + ports[i].0, start)
                }
            };
            pen.label(label, at, main0, size);
        }
        let mut rows = pen.canvas.rows();
        if !listed.is_empty() {
            rows.push(String::new());
            rows.extend(listed);
        }
        Ok(rows)
    }

    /// The `a ──label──▶ b` lines for the edges not drawn.
    fn listing(&self, graph: &Graph) -> Vec<String> {
        let name = |n: usize| text::fit(&graph.nodes[n].label.join(" "), text::WRAP);
        let mut seen = std::collections::HashSet::new();
        self.unplaced
            .iter()
            .filter(|&&e| seen.insert(e))
            .map(|&e| {
                let edge = &self.edges[e];
                let line = if edge.dotted { "┄┄" } else { "──" };
                let head = if edge.head { "▶" } else { "" };
                let label = edge.label.join(" ");
                let mid = if label.is_empty() {
                    format!("{line}{head}")
                } else {
                    format!("{line}{label}{line}{head}")
                };
                text::fit(
                    &format!("{} {mid} {}", name(edge.from), name(edge.to)),
                    MAX_COLS,
                )
            })
            .collect()
    }
}

/// Which edges to reverse so that the graph has no cycle: the back edges
/// of a depth-first search, run with an explicit stack.
fn break_cycles(n: usize, edges: &[Edge]) -> Vec<bool> {
    let mut out: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, e) in edges.iter().enumerate() {
        out[e.from].push(i);
    }
    // 0: not seen, 1: on the stack, 2: done.
    let mut state = vec![0u8; n];
    let mut reversed = vec![false; edges.len()];
    for root in 0..n {
        if state[root] != 0 {
            continue;
        }
        state[root] = 1;
        let mut stack = vec![(root, 0usize)];
        while let Some(top) = stack.last_mut() {
            let (v, k) = *top;
            if k < out[v].len() {
                top.1 += 1;
                let e = out[v][k];
                let w = edges[e].to;
                match state[w] {
                    0 => {
                        state[w] = 1;
                        stack.push((w, 0));
                    }
                    1 => reversed[e] = true,
                    _ => {}
                }
            } else {
                state[v] = 2;
                stack.pop();
            }
        }
    }
    reversed
}

/// Each node's layer: the longest path to it from a source, with the
/// edges in `reversed` turned around; then a source moves down to just
/// above its nearest successor, so it does not dangle far above it.
fn assign_layers(n: usize, edges: &[Edge], reversed: &[bool]) -> Vec<usize> {
    let ends = |i: usize| {
        let e = &edges[i];
        if reversed[i] {
            (e.to, e.from)
        } else {
            (e.from, e.to)
        }
    };
    let mut indegree = vec![0usize; n];
    let mut out: Vec<Vec<usize>> = vec![Vec::new(); n];
    for i in 0..edges.len() {
        let (a, b) = ends(i);
        indegree[b] += 1;
        out[a].push(b);
    }
    let mut layer = vec![0usize; n];
    let mut ready: std::collections::VecDeque<usize> =
        (0..n).filter(|&v| indegree[v] == 0).collect();
    while let Some(v) = ready.pop_front() {
        for &w in &out[v] {
            layer[w] = layer[w].max(layer[v] + 1);
            indegree[w] -= 1;
            if indegree[w] == 0 {
                ready.push_back(w);
            }
        }
    }
    let sources: Vec<usize> = (0..n)
        .filter(|&v| !out[v].is_empty() && (0..edges.len()).all(|i| ends(i).1 != v))
        .collect();
    for v in sources {
        if let Some(nearest) = out[v].iter().map(|&w| layer[w]).min() {
            layer[v] = nearest - 1;
        }
    }
    layer
}

/// How far the k-th port slot of a side is from its center (the side it
/// lies on is chosen when drawing): two columns apart, or one row.
fn slot_offset(k: usize, lr: bool) -> i64 {
    if lr {
        k as i64
    } else {
        2 * k as i64
    }
}

/// Each lnode's index in `layer`.
fn positions(layer: &[usize]) -> HashMap<usize, usize> {
    layer.iter().enumerate().map(|(i, &v)| (v, i)).collect()
}

/// Positions as close as possible (weighted least squares) to `desired`,
/// in order, with at least `seps[i]` between the i-th and the next. Pool
/// adjacent violators: iterative, linear.
fn isotonic(desired: &[f64], weights: &[f64], seps: &[f64]) -> Vec<f64> {
    // Shift out the separations: then the positions only need to grow.
    let mut offset = vec![0.0; desired.len()];
    for i in 1..desired.len() {
        offset[i] = offset[i - 1] + seps[i - 1];
    }
    // Blocks of (weighted sum, total weight, members).
    let mut blocks: Vec<(f64, f64, usize)> = Vec::new();
    for (i, d) in desired.iter().enumerate() {
        blocks.push((weights[i] * (d - offset[i]), weights[i], 1));
        while blocks.len() > 1 {
            let (s1, c1, n1) = blocks[blocks.len() - 1];
            let (s0, c0, n0) = blocks[blocks.len() - 2];
            if s0 / c0 <= s1 / c1 {
                break;
            }
            blocks.pop();
            *blocks.last_mut().expect("two blocks") = (s0 + s1, c0 + c1, n0 + n1);
        }
    }
    let mut out = Vec::with_capacity(desired.len());
    for (sum, count, n) in blocks {
        for _ in 0..n {
            out.push(sum / count + offset[out.len()]);
        }
    }
    out
}

/// Draws on the canvas in abstract (cross, main) coordinates.
struct Pen {
    dir: Dir,
    total_main: i64,
    canvas: Canvas,
}

impl Pen {
    /// The canvas cell of (cross, main).
    fn at(&self, c: i64, m: i64) -> (usize, usize) {
        let flip = self.total_main - 1 - m;
        let (x, y) = match self.dir {
            Dir::Td => (c, m),
            Dir::Bt => (c, flip),
            Dir::Lr => (m, c),
            Dir::Rl => (flip, c),
        };
        (x.max(0) as usize, y.max(0) as usize)
    }

    /// The canvas rectangle (x, y, w, h) of a cross × main rectangle.
    fn rect(&self, c0: i64, m0: i64, cross: i64, main: i64) -> (usize, usize, usize, usize) {
        let (x0, y0) = self.at(c0, m0);
        let (x1, y1) = self.at(c0 + cross - 1, m0 + main - 1);
        (
            x0.min(x1),
            y0.min(y1),
            x0.abs_diff(x1) + 1,
            y0.abs_diff(y1) + 1,
        )
    }

    /// Canvas links for main+ and main−.
    fn main_links(&self) -> (u8, u8) {
        match self.dir {
            Dir::Td => (DOWN, UP),
            Dir::Bt => (UP, DOWN),
            Dir::Lr => (RIGHT, LEFT),
            Dir::Rl => (LEFT, RIGHT),
        }
    }

    fn cross_links(&self) -> (u8, u8) {
        match self.dir {
            Dir::Td | Dir::Bt => (RIGHT, LEFT),
            Dir::Lr | Dir::Rl => (DOWN, UP),
        }
    }

    fn vline(&mut self, c: i64, m0: i64, m1: i64, open: (bool, bool), dotted: bool) {
        let (plus, minus) = self.main_links();
        for m in m0..=m1 {
            let mut l = 0;
            if m > m0 || open.0 {
                l |= minus;
            }
            if m < m1 || open.1 {
                l |= plus;
            }
            let (x, y) = self.at(c, m);
            self.canvas.link(x, y, l, dotted);
        }
    }

    fn hline(&mut self, m: i64, c0: i64, c1: i64, dotted: bool) {
        let (plus, minus) = self.cross_links();
        let (lo, hi) = (c0.min(c1), c0.max(c1));
        for c in lo..=hi {
            let mut l = 0;
            if c > lo {
                l |= minus;
            }
            if c < hi {
                l |= plus;
            }
            let (x, y) = self.at(c, m);
            self.canvas.link(x, y, l, dotted);
        }
    }

    /// An arrowhead at (c, m) pointing main+ (`forward`) or main−.
    fn arrow(&mut self, c: i64, m: i64, forward: bool) {
        let heads = match self.dir {
            Dir::Td => ('▼', '▲'),
            Dir::Bt => ('▲', '▼'),
            Dir::Lr => ('▶', '◀'),
            Dir::Rl => ('◀', '▶'),
        };
        let (x, y) = self.at(c, m);
        self.canvas
            .put(x, y, if forward { heads.0 } else { heads.1 });
    }

    fn is_arrow(&self, c: i64, m: i64) -> bool {
        let (x, y) = self.at(c, m);
        matches!(self.canvas.get(x, y), Some('▼' | '▲' | '▶' | '◀'))
    }

    /// A box border cell where a line leaves (`out`, main+) or enters.
    fn tee(&mut self, c: i64, m: i64, out: bool) {
        let (x, y) = self.at(c, m);
        let tee = match (self.dir, out) {
            (Dir::Td, true) | (Dir::Bt, false) => '┬',
            (Dir::Td, false) | (Dir::Bt, true) => '┴',
            (Dir::Lr, true) | (Dir::Rl, false) => '├',
            (Dir::Lr, false) | (Dir::Rl, true) => '┤',
        };
        if matches!(self.canvas.get(x, y), Some('─' | '│')) {
            self.canvas.put(x, y, tee);
        }
    }

    /// An edge label centered on the line at cross `c`, filling the main
    /// range from `m0` over `size` cells.
    fn label(&mut self, label: &[String], c: i64, m0: i64, size: i64) {
        let n = label.len() as i64;
        match self.dir {
            Dir::Td | Dir::Bt => {
                let w = text::widest(label) as i64;
                // The last `n` rows of the band; the first stays a line.
                let (x, y, _, _) = self.rect(c - w / 2, m0 + size - n, w, n);
                for (i, line) in label.iter().enumerate() {
                    let lw = text::width(line) as i64;
                    self.canvas.text(x + ((w - lw) / 2) as usize, y + i, line);
                }
            }
            Dir::Lr | Dir::Rl => {
                let w = text::widest(label) as i64;
                // " text " after one cell of line.
                let (x, y, _, _) = self.rect(c - (n - 1) / 2, m0 + 1, n, w + 2);
                for (i, line) in label.iter().enumerate() {
                    let lw = text::width(line) as i64;
                    let pad = ((w - lw) / 2) as usize;
                    let row = format!(
                        "{}{line}{}",
                        " ".repeat(pad + 1),
                        " ".repeat((w - lw) as usize - pad + 1)
                    );
                    self.canvas.text(x, y + i, &row);
                }
            }
        }
    }
}
```

Create `src/tui/diagram/sequence.rs`:

```rust
//! Sequence diagrams (`sequenceDiagram`): participants side by side,
//! lifelines down, one row band per message, note or block line.

use super::canvas::{self, Canvas};
use super::text;
use super::{Unsupported, MAX_COLS, MAX_EDGES, MAX_NODES, MAX_ROWS};

/// The deepest nesting of `loop`/`alt`/… blocks.
const MAX_DEPTH: usize = 8;

/// Message arrows, longest first so that `-->>` is not read as `-->`.
/// (pattern, dotted, head)
const ARROWS: [(&str, bool, bool); 8] = [
    ("-->>", true, true),
    ("->>", false, true),
    ("--x", true, true),
    ("--)", true, true),
    ("-->", true, false),
    ("-x", false, true),
    ("-)", false, true),
    ("->", false, false),
];

enum Side {
    Left,
    Right,
    Over,
}

enum Item {
    Message {
        from: usize,
        to: usize,
        text: Vec<String>,
        dotted: bool,
        head: bool,
    },
    Note {
        side: Side,
        a: usize,
        b: usize,
        text: Vec<String>,
    },
    /// `loop`, `alt`, `opt`, …: a frame opens.
    Open(String),
    /// `else`, `and`, `option`: a frame's next part.
    Split(String),
    End,
}

#[derive(Default)]
struct Diagram {
    ids: Vec<String>,
    labels: Vec<Vec<String>>,
    items: Vec<Item>,
    messages: usize,
}

/// The drawing of a sequence diagram with statements `lines`.
pub fn render<'a>(lines: impl Iterator<Item = &'a str>) -> Result<Vec<String>, Unsupported> {
    let mut d = Diagram::default();
    let mut depth = 0usize;
    for line in lines {
        let item = d.statement(line)?;
        match &item {
            Some(Item::Open(_)) => {
                depth += 1;
                if depth > MAX_DEPTH {
                    return Err(Unsupported::new("blocks nested too deep"));
                }
            }
            Some(Item::Split(_)) if depth == 0 => {
                return Err(Unsupported::new("else outside a block"));
            }
            Some(Item::End) => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| Unsupported::new("end without a block"))?;
            }
            _ => {}
        }
        d.items.extend(item);
    }
    d.items.extend((0..depth).map(|_| Item::End));
    if d.ids.is_empty() {
        return Err(Unsupported::new("empty sequence diagram"));
    }
    d.draw()
}

impl Diagram {
    /// The participant `id`, added if new.
    fn participant(&mut self, id: &str) -> Result<usize, Unsupported> {
        let id = id.trim();
        if id.is_empty() {
            return Err(Unsupported::new("expected a participant"));
        }
        if let Some(i) = self.ids.iter().position(|p| p == id) {
            return Ok(i);
        }
        if self.ids.len() == MAX_NODES {
            return Err(Unsupported::new(format!(
                "more than {MAX_NODES} participants"
            )));
        }
        self.ids.push(id.to_owned());
        self.labels.push(text::wrap(&text::clean(id)));
        Ok(self.ids.len() - 1)
    }

    fn statement(&mut self, line: &str) -> Result<Option<Item>, Unsupported> {
        let (word, rest) = match line.split_once(char::is_whitespace) {
            Some((word, rest)) => (word, rest.trim()),
            None => (line, ""),
        };
        let label = |s: &str| text::fit(&text::clean(s).replace('\n', " "), 60);
        match word {
            "participant" | "actor" => {
                let (id, alias) = match rest.split_once(" as ") {
                    Some((id, alias)) => (id.trim(), Some(alias.trim())),
                    None => (rest, None),
                };
                let i = self.participant(id)?;
                if let Some(alias) = alias {
                    self.labels[i] = text::wrap(&text::clean(alias));
                }
                Ok(None)
            }
            "autonumber" | "activate" | "deactivate" => Ok(None),
            "loop" | "alt" | "opt" | "par" | "critical" | "break" => {
                Ok(Some(Item::Open(format!("{word}: {}", label(rest)))))
            }
            "rect" => Ok(Some(Item::Open(String::new()))),
            "else" | "and" | "option" => Ok(Some(Item::Split(format!("{word}: {}", label(rest))))),
            "end" if rest.is_empty() => Ok(Some(Item::End)),
            _ if word.eq_ignore_ascii_case("note") => self.note(rest).map(Some),
            _ => self.message(line).map(Some),
        }
    }

    /// `left of a: t`, `right of a: t`, `over a: t`, `over a,b: t`.
    fn note(&mut self, rest: &str) -> Result<Item, Unsupported> {
        let Some((place, body)) = rest.split_once(':') else {
            return Err(Unsupported::new("note without text"));
        };
        let place = place.trim();
        let (side, who) = if let Some(who) = place.strip_prefix("left of ") {
            (Side::Left, who)
        } else if let Some(who) = place.strip_prefix("right of ") {
            (Side::Right, who)
        } else if let Some(who) = place.strip_prefix("over ") {
            (Side::Over, who)
        } else {
            return Err(Unsupported::new("note needs left of, right of or over"));
        };
        let (a, b) = match who.split_once(',') {
            Some((a, b)) if matches!(side, Side::Over) => {
                (self.participant(a)?, self.participant(b)?)
            }
            Some(_) => return Err(Unsupported::new("only a note over may span")),
            None => {
                let a = self.participant(who)?;
                (a, a)
            }
        };
        Ok(Item::Note {
            side,
            a: a.min(b),
            b: a.max(b),
            text: text::wrap(&text::clean(body.trim())),
        })
    }

    /// `a->>b: text` and the other arrows.
    fn message(&mut self, line: &str) -> Result<Item, Unsupported> {
        let (head, body) = line.split_once(':').unwrap_or((line, ""));
        let found = (0..head.len())
            .filter(|&i| head.is_char_boundary(i))
            .find_map(|i| {
                ARROWS
                    .iter()
                    .find(|(p, _, _)| head[i..].starts_with(p))
                    .map(|a| (i, a))
            });
        let Some((at, &(pattern, dotted, arrowhead))) = found else {
            let shown = text::fit(&text::clean(line).replace('\n', " "), 40);
            return Err(Unsupported::new(format!("cannot parse: {shown}")));
        };
        let from = self.participant(&head[..at])?;
        let to = head[at + pattern.len()..]
            .trim()
            .trim_start_matches(['+', '-']);
        let to = self.participant(to)?;
        if self.messages == MAX_EDGES {
            return Err(Unsupported::new(format!("more than {MAX_EDGES} messages")));
        }
        self.messages += 1;
        Ok(Item::Message {
            from,
            to,
            text: text::wrap(&text::clean(body.trim())),
            dotted,
            head: arrowhead,
        })
    }

    fn draw(&self) -> Result<Vec<String>, Unsupported> {
        let n = self.ids.len();
        let widths: Vec<i64> = self
            .labels
            .iter()
            .map(|l| text::widest(l) as i64 + 4)
            .collect();
        let heights: Vec<i64> = self.labels.iter().map(|l| l.len() as i64 + 2).collect();
        // Positions 0 (left edge), 1..=n (lifelines), n + 1 (right edge);
        // constraints (a, b, d): position b is at least d right of a.
        let mut rules: Vec<(usize, usize, i64)> = Vec::new();
        rules.push((0, 1, widths[0] / 2));
        rules.push((n, n + 1, widths[n - 1] - widths[n - 1] / 2));
        for i in 1..n {
            let (wa, wb) = (widths[i - 1], widths[i]);
            rules.push((i, i + 1, wa - wa / 2 + 2 + wb / 2));
        }
        let mut depth = 0usize;
        let mut max_depth = 0usize;
        for item in &self.items {
            match item {
                Item::Message { from, to, text, .. } if from == to => {
                    rules.push((from + 1, from + 2, text::widest(text) as i64 + 6));
                }
                Item::Message { from, to, text, .. } => {
                    let (a, b) = (from.min(to) + 1, from.max(to) + 1);
                    rules.push((a, b, (text::widest(text) as i64 + 4).max(6)));
                }
                Item::Note { side, a, b, text } => {
                    let w = text::widest(text) as i64 + 4;
                    let (a, b) = (a + 1, b + 1);
                    match side {
                        Side::Right => rules.push((a, a + 1, w + 3)),
                        Side::Left => rules.push((a - 1, a, w + 3)),
                        Side::Over if a == b => {
                            rules.push((a - 1, a, w / 2 + 2));
                            rules.push((a, a + 1, w - w / 2 + 2));
                        }
                        Side::Over => {
                            rules.push((a - 1, a, 3));
                            rules.push((a, b, w - 5));
                            rules.push((b, b + 1, 3));
                        }
                    }
                }
                Item::Open(_) => {
                    depth += 1;
                    max_depth = max_depth.max(depth);
                }
                Item::End => depth -= 1,
                Item::Split(_) => {}
            }
        }
        let mut xs = vec![0i64; n + 2];
        for b in 1..n + 2 {
            xs[b] = xs[b - 1] + 1;
            for &(a, rb, d) in &rules {
                if rb == b {
                    xs[b] = xs[b].max(xs[a] + d);
                }
            }
        }
        // Frames take two columns each side per nesting level.
        let margin = 2 * max_depth as i64;
        let xs: Vec<i64> = xs.iter().map(|x| x + margin).collect();
        let width = xs[n + 1] + margin + 1;
        let head = heights.iter().copied().max().unwrap_or(3);
        let mut height = head;
        for item in &self.items {
            height += rows_of(item);
        }
        height += 1;
        if width as usize > MAX_COLS || height as usize > MAX_ROWS {
            return Err(Unsupported::new("diagram too large"));
        }
        let mut c = Canvas::new(width as usize, height as usize);
        let at = |i: usize| xs[i + 1] as usize;
        for i in 0..n {
            let x = at(i) - (widths[i] / 2) as usize;
            let (w, h) = (widths[i] as usize, heights[i] as usize);
            c.vline(at(i), h, height as usize - 1, (true, false), false);
            c.frame(x, 0, w, h, &canvas::RECT);
            for (k, line) in self.labels[i].iter().enumerate() {
                c.text(x + (w - text::width(line)) / 2, 1 + k, line);
            }
            c.put(at(i), h - 1, '┬');
        }
        let mut y = head as usize;
        let mut open: Vec<(usize, usize)> = Vec::new();
        for item in &self.items {
            match item {
                Item::Message {
                    from,
                    to,
                    text,
                    dotted,
                    head,
                } if from == to => {
                    let x = at(*from);
                    let rows = rows_of(item) as usize;
                    let (h, v) = if *dotted {
                        ('┄', '┆')
                    } else {
                        ('─', '│')
                    };
                    c.text(x, y, &format!("├{h}{h}┐"));
                    for k in 1..rows - 1 {
                        c.put(x + 3, y + k, v);
                    }
                    let back = if *head { '◀' } else { h };
                    c.text(x + 1, y + rows - 1, &format!("{back}{h}┘"));
                    for (k, line) in text.iter().enumerate() {
                        c.text(x + 5, y + k, line);
                    }
                    y += rows;
                }
                Item::Message {
                    from,
                    to,
                    text,
                    dotted,
                    head,
                } => {
                    let (xf, xt) = (at(*from), at(*to));
                    let (a, b) = (xf.min(xt), xf.max(xt));
                    for line in text {
                        let lw = text::width(line);
                        let x = ((a + b) / 2).saturating_sub(lw / 2).max(a + 2);
                        c.text(x, y, line);
                        y += 1;
                    }
                    let line = if *dotted { '┄' } else { '─' };
                    for x in a + 1..b {
                        c.put(x, y, line);
                    }
                    let right = xt > xf;
                    c.put(xf, y, if right { '├' } else { '┤' });
                    if *head {
                        let tip = if right {
                            (xt - 1, '▶')
                        } else {
                            (xt + 1, '◀')
                        };
                        c.put(tip.0, y, tip.1);
                    } else {
                        c.put(xt, y, if right { '┤' } else { '├' });
                    }
                    y += 1;
                }
                Item::Note { side, a, b, text } => {
                    let w = text::widest(text) + 4;
                    let x = match side {
                        Side::Right => at(*a) + 2,
                        Side::Left => at(*a) - 1 - w,
                        Side::Over if a == b => at(*a) - w / 2,
                        Side::Over => at(*a) - 2,
                    };
                    let w = match side {
                        Side::Over if a != b => w.max(at(*b) + 3 - x),
                        _ => w,
                    };
                    let h = text.len() + 2;
                    c.frame(x, y, w, h, &canvas::NOTE);
                    for (k, line) in text.iter().enumerate() {
                        c.text(x + 2, y + 1 + k, line);
                    }
                    y += h;
                }
                Item::Open(label) => {
                    let level = open.len();
                    let (l, r) = frame_columns(level, max_depth, width as usize);
                    self.rule(&mut c, y, l, r, ('┌', '─', '┐'), label, &xs);
                    open.push((level, y));
                    y += 1;
                }
                Item::Split(label) => {
                    let level = open.len() - 1;
                    let (l, r) = frame_columns(level, max_depth, width as usize);
                    self.rule(&mut c, y, l, r, ('├', '┄', '┤'), label, &xs);
                    y += 1;
                }
                Item::End => {
                    let (level, top) = open.pop().expect("blocks are balanced");
                    let (l, r) = frame_columns(level, max_depth, width as usize);
                    self.rule(&mut c, y, l, r, ('└', '─', '┘'), "", &xs);
                    for row in top + 1..y {
                        for x in [l, r] {
                            if c.get(x, row).is_none() {
                                c.put(x, row, '│');
                            }
                        }
                    }
                    y += 1;
                }
            }
        }
        Ok(c.rows())
    }

    /// A frame's top, split or bottom row from column `l` to `r`, with
    /// `label` near its left end; lifelines cross it.
    #[allow(clippy::too_many_arguments)]
    fn rule(
        &self,
        c: &mut Canvas,
        y: usize,
        l: usize,
        r: usize,
        (left, line, right): (char, char, char),
        label: &str,
        xs: &[i64],
    ) {
        c.put(l, y, left);
        for x in l + 1..r {
            c.put(x, y, line);
        }
        c.put(r, y, right);
        for &x in &xs[1..xs.len() - 1] {
            c.put(x as usize, y, '┼');
        }
        if !label.is_empty() {
            let room = r.saturating_sub(l + 4);
            c.text(
                l + 2,
                y,
                &format!(" {} ", text::fit(label, room.saturating_sub(2))),
            );
        }
    }
}

/// The rows `item` takes.
fn rows_of(item: &Item) -> i64 {
    match item {
        Item::Message { from, to, text, .. } if from == to => text.len().max(1) as i64 + 2,
        Item::Message { text, .. } => text.len() as i64 + 1,
        Item::Note { text, .. } => text.len() as i64 + 2,
        Item::Open(_) | Item::Split(_) | Item::End => 1,
    }
}

/// The left and right columns of a frame at nesting `level` (0 = outer).
fn frame_columns(level: usize, max_depth: usize, width: usize) -> (usize, usize) {
    let inset = 2 * level;
    debug_assert!(level < max_depth);
    (inset, width - 1 - inset)
}
```

Run: `umask 022; cargo test --locked --bin claudio diagram`
Expected:
```
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 888 filtered out; finished in 0.02s
```
Run: `umask 022; cargo test --locked --bin claudio`
Expected: the previous total + 12 (scratch, on base: `test result: ok. 900 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`).
Run: `rustfmt --edition 2021 --check src/tui/diagram/mod.rs` (checks the new module tree) → no output; `cargo build --locked` → 0 warnings.
`dense_graphs_within_the_caps_are_quick` (64 nodes, 128 edges with cycles) asserts < 2 s in a debug build; it takes a few ms.

Step 3: commit
`/usr/bin/git add src/tui/diagram && /usr/bin/git commit -m "feat(tui): mermaid diagram engine for the reader" -- src/tui/diagram src/tui/mod.rs`

Interfaces this task provides to later tasks:
- `crate::tui::diagram::{too_complex, render, Unsupported}` exactly as interfaces.md: `pub fn too_complex(src: &str) -> bool` (> 4096 bytes or > 200 lines), `pub fn render(src: &str) -> Result<Vec<String>, Unsupported>` (rows ≤ 200 columns, ≤ 150 rows, no controls, trailing spaces trimmed; `src` is the fence content without the fence lines), `#[derive(Debug, Clone, PartialEq)] pub struct Unsupported { pub reason: String }`. Also `MAX_COLS`, `MAX_ROWS` (for the reader's "diagram is N columns wide" rule).
- Task 11 removes the `#[allow(dead_code)]` above `mod diagram;` and calls `render` only on the render worker through the guard (`too_complex` first; `render` re-checks it).


### Task 9: search (`src/claude/search.rs`, `Msg::Search`, daemon arm)
Model: sonnet   Repo: <repo> (feat/extensions after Task 1, or the controller's worktree)
Files: create `src/claude/search.rs`; modify `src/claude/mod.rs` (one line), `src/claude/projects.rs` (`flag`, `answer_prompt`, `push_texts` become `pub(crate)`), `src/proto.rs` (`Msg::Search` after `GitDiff`, `Msg::SearchReply` after `GitPatch`, `SEARCH_REPLY_MAX`/`SearchHit`/`search_reply` after `answers_reply`, two tests after `an_answers_reply_always_fits_a_frame`, `"search"` in `DAEMON_FEATURES`), `src/daemon/mod.rs` (one `Daemon` field), `src/daemon/server.rs` (one `request` arm before `other =>`, `spawn_search` after `spawn_git`, `SearchSlot` after `recent_projects`), `src/daemon/tests.rs` (one test before `list_dir_sorts_dirs_first`).
Depends on: Task 1 (`DAEMON_FEATURES`, `Welcome.features`). Parallel with Tasks 8 and 10. Task 10 edits `proto.rs`, `server.rs`, `projects.rs` and `daemon/tests.rs` in other places (after `Msg::Answers`/`AnswersReply`; after the `Answers` arm and the `answers` fn; `read_answers` and new fns after `cut`; before `kill_removes_session_everywhere`). A scratch merge of both branches conflicts only on the `DAEMON_FEATURES` line → resolve to `&["lens", "search", "last_prompt"]`. Neither task edits a `use` line of `server.rs` (both write `std::path::PathBuf`/`std::sync::Mutex` in full): keep it so.
Goal: spec §8 daemon side and the AC9 unit tests: query rules (trimmed, sanitized, 2..=200 bytes, ≤ 8 words, every word must appear, Unicode lowercase); scope `<projects>/*/*.jsonl` (real dirs and regular files only, no deeper levels, ids `[A-Za-z0-9_-]+`), newest first; messages are real prompts (`answer_prompt`) and assistant `text` blocks (never tool results, thinking, tool input, Claude Code's own records, sidechains or API errors); one hit per conversation (`matches`; `snippet` = ≤ 160 chars of the newest match around its first word, one line; `title` = the first real prompt read, ≤ 80 chars; `cwd` from the records; all sanitized); streaming `BufReader` 64 KiB, lines > 1 MiB skipped without being kept, last 64 MiB per file (partial first line skipped), stop at 3 s / 1 GiB / `limit` (0 → 50, ≤ 100) with `truncated`, `scanned`/`total` count files; reply ≤ 512 KiB; one search per daemon (`Error "search busy"` at once); `Error "bad query: <why>"`.

`search.rs` API (names left to the drafter by interfaces.md):
```rust
pub struct Budget { pub time: Duration, pub bytes: u64, pub tail: u64, pub line: usize } // Default: 3 s, 1 GiB, 64 MiB, 1 MiB
pub struct Outcome { pub hits: Vec<SearchHit>, pub scanned: u32, pub total: u32, pub truncated: bool }
pub struct BadQuery(pub String);
pub fn terms(query: &str) -> Result<Vec<String>, BadQuery>;
pub fn search(projects: &Path, query: &str, limit: usize, budget: Budget) -> Result<Outcome, BadQuery>;
```

Step 1: write the failing tests

In `src/claude/mod.rs`, after `pub mod projects;` add `pub mod search;`.
Create `src/claude/search.rs` with a module doc and the tests (Step 2 puts the implementation between them):

```rust
//! Substring search over this host's claude conversations, for the
//! `search` extension (`Msg::Search`).

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A fresh projects root with one project dir.
    fn root() -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("claudio-search-{}", uuid::Uuid::new_v4()));
        let project = root.join("-home-u-p");
        std::fs::create_dir_all(&project).unwrap();
        (root, project)
    }

    fn prompt(text: &str) -> serde_json::Value {
        json!({"type": "user", "cwd": "/home/u/p", "message": {"role": "user", "content": text}})
    }

    fn answer(text: &str) -> serde_json::Value {
        json!({"type": "assistant", "message": {"role": "assistant",
            "content": [{"type": "text", "text": text}]}})
    }

    /// Write transcript `<id>.jsonl` with `records`, modified `age` seconds
    /// before a fixed time.
    fn transcript(dir: &Path, id: &str, age: u64, records: &[serde_json::Value]) -> PathBuf {
        let path = dir.join(format!("{id}.jsonl"));
        let mut bytes = Vec::new();
        for r in records {
            bytes.extend_from_slice(r.to_string().as_bytes());
            bytes.push(b'\n');
        }
        std::fs::write(&path, bytes).unwrap();
        let when = UNIX_EPOCH + Duration::from_secs(2_000_000_000 - age);
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(when)
            .unwrap();
        path
    }

    fn find(root: &Path, query: &str) -> Outcome {
        search(root, query, 0, Budget::default()).unwrap()
    }

    #[test]
    fn queries_are_trimmed_sanitized_and_bounded() {
        assert_eq!(terms("  Foo BAR "), Ok(vec!["foo".into(), "bar".into()]));
        assert_eq!(terms("a\u{1b}"), Err(BadQuery("too short".into())));
        assert_eq!(terms(" x "), Err(BadQuery("too short".into())));
        assert!(terms(&"x".repeat(MAX_QUERY)).is_ok());
        assert_eq!(
            terms(&"x".repeat(MAX_QUERY + 1)),
            Err(BadQuery("too long (max 200 bytes)".into()))
        );
        assert_eq!(
            terms("a b c d e f g h i"),
            Err(BadQuery("too many words (max 8)".into()))
        );
        assert_eq!(terms("\u{1b}[2Jab"), Ok(vec!["[2jab".into()]));
    }

    #[test]
    fn every_word_matches_in_any_case_and_one_hit_per_conversation() {
        let (root, dir) = root();
        transcript(
            &dir,
            "c1",
            10,
            &[
                prompt("How do I parse TOML in Rust?"),
                answer("Use the toml crate."),
                prompt("and write toml?"),
                answer("Rust's toml crate writes it too: `toml::to_string`."),
            ],
        );
        transcript(
            &dir,
            "c2",
            5,
            &[prompt("only rust here"), answer("Ünïcode ÉCOLE")],
        );
        let out = find(&root, "rust TOML");
        assert_eq!((out.scanned, out.total, out.truncated), (2, 2, false));
        assert_eq!(out.hits.len(), 1);
        let hit = &out.hits[0];
        assert_eq!(hit.conversation, "c1");
        assert_eq!(hit.matches, 2);
        assert_eq!(hit.title, "How do I parse TOML in Rust?");
        assert_eq!(hit.cwd, "/home/u/p");
        assert_eq!(
            hit.snippet,
            "Rust's toml crate writes it too: `toml::to_string`."
        );
        assert_eq!(hit.modified, 2_000_000_000 - 10);
        // Unicode lowercase on both sides.
        assert_eq!(find(&root, "école").hits[0].conversation, "c2");
        assert!(find(&root, "rust python").hits.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tool_output_thinking_and_claude_codes_own_records_are_not_searched() {
        let (root, dir) = root();
        let mut meta = prompt("secret in meta");
        meta["isMeta"] = json!(true);
        let mut side = answer("secret in a subagent");
        side["isSidechain"] = json!(true);
        let mut api = answer("secret in an api error");
        api["isApiErrorMessage"] = json!(true);
        transcript(
            &dir,
            "c1",
            1,
            &[
                prompt("the question"),
                json!({"type": "assistant", "message": {"content": [
                    {"type": "thinking", "thinking": "secret in thinking"},
                    {"type": "tool_use", "id": "t", "name": "Bash", "input": {"command": "secret"}},
                ]}}),
                json!({"type": "user", "message": {"content": [
                    {"type": "tool_result", "tool_use_id": "t", "content": "secret in a tool result"},
                ]}}),
                prompt("<command-name>/secret</command-name>"),
                meta,
                side,
                api,
                json!({"type": "summary", "summary": "secret summary"}),
            ],
        );
        assert!(find(&root, "secret").hits.is_empty());
        assert_eq!(find(&root, "question").hits.len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn newest_first_and_only_plain_transcripts() {
        let (root, dir) = root();
        transcript(&dir, "old", 100, &[prompt("needle old")]);
        transcript(&dir, "new", 1, &[prompt("needle new")]);
        let other = root.join("-home-u-q");
        std::fs::create_dir_all(other.join("sub")).unwrap();
        transcript(&other, "mid", 50, &[prompt("needle mid")]);
        // Deeper levels, odd names, other extensions and symlinks are not read.
        transcript(&other.join("sub"), "deep", 0, &[prompt("needle deep")]);
        transcript(&other, "bad name", 0, &[prompt("needle bad")]);
        std::fs::write(other.join("notes.txt"), "needle").unwrap();
        std::os::unix::fs::symlink(dir.join("new.jsonl"), other.join("link.jsonl")).unwrap();
        let out = find(&root, "needle");
        let ids: Vec<&str> = out.hits.iter().map(|h| h.conversation.as_str()).collect();
        assert_eq!(ids, ["new", "mid", "old"]);
        assert_eq!(out.total, 3);
        assert!(
            search(&root.join("missing"), "needle", 0, Budget::default())
                .unwrap()
                .hits
                .is_empty()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_budget_and_the_limit_stop_a_search() {
        let (root, dir) = root();
        for i in 0..3 {
            transcript(
                &dir,
                &format!("c{i}"),
                i,
                &[prompt(&format!("needle {i} {}", "x".repeat(100)))],
            );
        }
        let limited = search(&root, "needle", 2, Budget::default()).unwrap();
        assert_eq!(
            (limited.hits.len(), limited.scanned, limited.truncated),
            (2, 2, true)
        );
        let exact = search(&root, "needle", 3, Budget::default()).unwrap();
        assert_eq!((exact.hits.len(), exact.truncated), (3, false));
        // Bytes: the first file alone uses it up.
        let bytes = Budget {
            bytes: 10,
            ..Budget::default()
        };
        let out = search(&root, "needle", 0, bytes).unwrap();
        assert_eq!((out.scanned, out.total, out.truncated), (1, 3, true));
        // Time.
        let time = Budget {
            time: Duration::ZERO,
            ..Budget::default()
        };
        let out = search(&root, "needle", 0, time).unwrap();
        assert_eq!((out.hits.len(), out.scanned, out.truncated), (0, 0, true));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn long_lines_are_skipped_and_only_the_tail_is_read() {
        let (root, dir) = root();
        let budget = Budget {
            tail: 4096,
            line: 1024,
            ..Budget::default()
        };
        transcript(
            &dir,
            "c1",
            1,
            &[
                prompt("needle early"),
                answer(&"y".repeat(5000)),
                answer(&format!("needle in a long line {}", "z".repeat(2000))),
                prompt("needle late"),
            ],
        );
        let out = search(&root, "needle", 0, budget).unwrap();
        assert_eq!(out.hits[0].matches, 1);
        assert_eq!(out.hits[0].snippet, "needle late");
        // The title is the first prompt that was read.
        assert_eq!(out.hits[0].title, "needle late");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn snippets_are_sanitized_one_line_and_cut_around_the_word() {
        let (root, dir) = root();
        let long = format!(
            "{} the needle\n\tis \u{1b}[31mhere {}",
            "a ".repeat(100),
            "b ".repeat(100)
        );
        transcript(&dir, "c1", 1, &[prompt("q\u{202e}?"), answer(&long)]);
        let hit = &find(&root, "needle").hits[0];
        assert_eq!(hit.title, "q?");
        assert_eq!(hit.snippet.chars().count(), SNIPPET_CHARS);
        assert!(
            hit.snippet.starts_with('…') && hit.snippet.ends_with('…'),
            "{}",
            hit.snippet
        );
        assert!(
            hit.snippet.contains("the needle is [31mhere"),
            "{}",
            hit.snippet
        );
        assert!(!hit.snippet.chars().any(char::is_control));
        let _ = std::fs::remove_dir_all(&root);
    }
}
```

In `src/proto.rs` (tests module), before `fn latex_round_trips_and_is_unknown_to_older_clients`:

```diff
--- a/src/proto.rs
+++ b/src/proto.rs
@@ -1205,6 +1274,75 @@ mod tests {
         );
     }
 
+    #[test]
+    fn search_round_trips_and_is_unknown_to_older_daemons() {
+        #[derive(Debug, PartialEq, Deserialize)]
+        #[serde(tag = "op", rename_all = "snake_case")]
+        enum V05 {
+            Ping,
+            #[serde(other)]
+            Unknown,
+        }
+        let ask: Msg = serde_json::from_str(r#"{"op":"search","query":"toml"}"#).unwrap();
+        assert_eq!(
+            ask,
+            Msg::Search {
+                query: "toml".into(),
+                limit: 0
+            }
+        );
+        let json = serde_json::to_string(&Envelope::request(3, ask.clone())).unwrap();
+        assert_eq!(serde_json::from_str::<V05>(&json).unwrap(), V05::Unknown);
+        let reply = search_reply(
+            "toml".into(),
+            vec![SearchHit {
+                conversation: "c1".into(),
+                cwd: "/p".into(),
+                modified: 7,
+                title: "t".into(),
+                snippet: "s".into(),
+                matches: 2,
+            }],
+            1,
+            3,
+            false,
+        );
+        let frame = Frame::Control(Envelope::request(3, reply.clone()));
+        assert_eq!(roundtrip(frame.clone()), frame);
+        let json = serde_json::to_string(&reply).unwrap();
+        assert!(json.contains(r#""op":"search_reply""#), "{json}");
+        assert_eq!(serde_json::from_str::<V05>(&json).unwrap(), V05::Unknown);
+    }
+
+    #[test]
+    fn a_search_reply_always_fits_a_frame() {
+        // 100 hits with 8 KiB of quotes each: 1.6 MiB once encoded.
+        let hits: Vec<SearchHit> = (0..100)
+            .map(|i| SearchHit {
+                conversation: format!("c{i}"),
+                cwd: "\"".repeat(4096),
+                modified: i,
+                title: "\"".repeat(4096),
+                snippet: String::new(),
+                matches: 1,
+            })
+            .collect();
+        let msg = search_reply("q".into(), hits.clone(), 100, 100, false);
+        assert!(serde_json::to_vec(&msg).unwrap().len() <= SEARCH_REPLY_MAX);
+        let Msg::SearchReply {
+            hits: kept,
+            truncated,
+            ..
+        } = msg
+        else {
+            panic!("{msg:?}")
+        };
+        // The newest stay, in order; the reply says it is cut.
+        assert!(truncated);
+        assert_eq!(kept[..], hits[..kept.len()]);
+        assert_eq!(kept.len(), 31);
+    }
+
     #[test]
     fn latex_round_trips_and_is_unknown_to_older_clients() {
         let event = SessionEvent::Latex {
```

In `src/daemon/tests.rs`, before `async fn list_dir_sorts_dirs_first`:

```diff
--- a/src/daemon/tests.rs
+++ b/src/daemon/tests.rs
@@ -725,6 +725,60 @@ async fn resize_resyncs_other_subscribers() {
     assert_eq!(size, (30, 100));
 }
 
+/// `Search` runs off the client loop over the daemon's projects root, one
+/// at a time; a bad query and a second search are refused at once.
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn search_finds_conversations_one_search_at_a_time() {
+    let d = TestDaemon::start().await;
+    let project = d.dir.join("projects/-w");
+    std::fs::create_dir_all(&project).unwrap();
+    let record = serde_json::json!({"type": "user", "cwd": "/w",
+        "message": {"role": "user", "content": "find the Needle"}});
+    std::fs::write(project.join("c1.jsonl"), format!("{record}\n")).unwrap();
+    d.daemon.search.set_root(d.dir.join("projects"));
+    let mut c = d.client().await;
+    let ask = |query: &str| Msg::Search {
+        query: query.into(),
+        limit: 0,
+    };
+    match c.call(ask("needle")).await {
+        Msg::SearchReply {
+            query,
+            hits,
+            scanned,
+            total,
+            truncated,
+        } => {
+            assert_eq!(
+                (query.as_str(), scanned, total, truncated),
+                ("needle", 1, 1, false)
+            );
+            assert_eq!(hits.len(), 1);
+            assert_eq!(
+                (hits[0].conversation.as_str(), hits[0].cwd.as_str()),
+                ("c1", "/w")
+            );
+            assert_eq!(hits[0].snippet, "find the Needle");
+        }
+        other => panic!("expected a reply, got {other:?}"),
+    }
+    match c.call(ask(" x ")).await {
+        Msg::Error { message } => assert_eq!(message, "bad query: too short"),
+        other => panic!("expected an error, got {other:?}"),
+    }
+    let running = d.daemon.search.hold();
+    match c.call(ask("needle")).await {
+        Msg::Error { message } => assert_eq!(message, "search busy"),
+        other => panic!("expected busy, got {other:?}"),
+    }
+    drop(running);
+    assert!(matches!(
+        c.call(ask("needle")).await,
+        Msg::SearchReply { .. }
+    ));
+    assert!(proto::DAEMON_FEATURES.contains(&"search"));
+}
+
 #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
 async fn list_dir_sorts_dirs_first() {
     let d = TestDaemon::start().await;
```

Run: `umask 022; cargo test --locked --bin claudio search`
Expected (compile failure), among others:
```
error[E0425]: cannot find function `search` in this scope
error[E0599]: no variant named `SearchReply` found for enum `proto::Msg`
error[E0609]: no field `search` on type `std::sync::Arc<Daemon>`
error: could not compile `claudio` (bin "claudio" test) due to 54 previous errors; 1 warning emitted
```

Step 2: implement

`src/claude/search.rs`: replace the two step-1 doc lines with the following, keeping the tests module after it:

```rust
//! Substring search over this host's claude conversations, for the
//! `search` extension (`Msg::Search`).
//!
//! Blocking and bounded: transcripts are streamed line by line through a
//! 64 KiB buffer, a line longer than [`Budget::line`] is skipped without
//! being kept, only the last [`Budget::tail`] bytes of a file are read, and
//! the whole search stops at [`Budget::time`], [`Budget::bytes`] or its hit
//! limit. Only real prompts and claude's answer text are searched (the
//! rules of [`projects::recent_answers`]); never tool output, thinking,
//! Claude Code's own records or subagents. The query is plain text: it never
//! reaches a shell or a regex.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

use crate::claude::latex::sanitize;
use crate::claude::projects::{answer_prompt, flag, push_texts};
use crate::proto::SearchHit;

/// A query's length after trimming and [`sanitize`], in bytes.
pub const MIN_QUERY: usize = 2;
pub const MAX_QUERY: usize = 200;
/// The most words a query may have; a message must contain all of them.
pub const MAX_TERMS: usize = 8;
/// Hits when the request asks for 0, and the most it may ask for.
pub const DEFAULT_LIMIT: usize = 50;
pub const MAX_LIMIT: usize = 100;
/// A hit's title and snippet, in characters.
const TITLE_CHARS: usize = 80;
const SNIPPET_CHARS: usize = 160;
/// Characters of the snippet before the first term.
const SNIPPET_LEAD: usize = 60;
/// A longer `cwd` is not a path claude ran in; it is dropped.
const MAX_CWD: usize = 4096;
/// The read buffer.
const BUFFER: usize = 64 * 1024;

/// When a search stops.
#[derive(Debug, Clone, Copy)]
pub struct Budget {
    /// Wall time for the whole search.
    pub time: Duration,
    /// Bytes read, all files together.
    pub bytes: u64,
    /// Bytes read from the end of each file.
    pub tail: u64,
    /// The longest line examined; longer ones are skipped.
    pub line: usize,
}

impl Default for Budget {
    fn default() -> Budget {
        Budget {
            time: Duration::from_secs(3),
            bytes: 1 << 30,
            tail: 64 << 20,
            line: 1 << 20,
        }
    }
}

/// A search's result: hits newest conversation first, how many transcripts
/// were read to the end of `total`, and whether it stopped early.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub hits: Vec<SearchHit>,
    pub scanned: u32,
    pub total: u32,
    pub truncated: bool,
}

/// Why a query is refused; the daemon answers `Error "bad query: <why>"`.
#[derive(Debug, Clone, PartialEq)]
pub struct BadQuery(pub String);

/// The words of `query`, lowercase: trimmed and [`sanitize`]d, it must
/// have [`MIN_QUERY`]..=[`MAX_QUERY`] bytes and at most [`MAX_TERMS`] words.
pub fn terms(query: &str) -> Result<Vec<String>, BadQuery> {
    let query = sanitize(query.trim());
    let query = query.trim();
    if query.len() < MIN_QUERY {
        return Err(BadQuery("too short".into()));
    }
    if query.len() > MAX_QUERY {
        return Err(BadQuery(format!("too long (max {MAX_QUERY} bytes)")));
    }
    let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if terms.len() > MAX_TERMS {
        return Err(BadQuery(format!("too many words (max {MAX_TERMS})")));
    }
    Ok(terms)
}

/// Search the transcripts `<projects>/*/*.jsonl`, newest first, for
/// messages that contain every word of `query` (case-insensitive). One hit
/// per conversation; at most `limit` hits (0: [`DEFAULT_LIMIT`]; capped at
/// [`MAX_LIMIT`]).
pub fn search(
    projects: &Path,
    query: &str,
    limit: usize,
    budget: Budget,
) -> Result<Outcome, BadQuery> {
    let terms = terms(query)?;
    let limit = match limit {
        0 => DEFAULT_LIMIT,
        n => n.min(MAX_LIMIT),
    };
    let files = transcripts(projects);
    let mut spent = Spent {
        started: Instant::now(),
        bytes: 0,
    };
    let mut out = Outcome {
        hits: Vec::new(),
        scanned: 0,
        total: u32::try_from(files.len()).unwrap_or(u32::MAX),
        truncated: false,
    };
    for (modified, path, conversation) in &files {
        if out.hits.len() >= limit || spent.over(&budget) {
            out.truncated = true;
            break;
        }
        let (found, finished) = scan(path, &terms, &budget, &mut spent);
        if let Some(found) = found {
            out.hits.push(found.hit(conversation, *modified));
        }
        if !finished {
            out.truncated = true;
            break;
        }
        out.scanned += 1;
    }
    Ok(out)
}

/// What was used of the budget.
struct Spent {
    started: Instant,
    bytes: u64,
}

impl Spent {
    fn over(&self, budget: &Budget) -> bool {
        self.bytes >= budget.bytes || self.started.elapsed() >= budget.time
    }
}

/// Every `<root>/<dir>/<id>.jsonl` (no symlinks, no deeper levels) whose
/// `<id>` is a plain conversation id: (mtime, path, id), newest first.
fn transcripts(root: &Path) -> Vec<(u64, PathBuf, String)> {
    let mut files = Vec::new();
    let Ok(dirs) = std::fs::read_dir(root) else {
        return files;
    };
    for dir in dirs.flatten() {
        if !dir.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(dir.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let id = match (path.extension(), path.file_stem()) {
                (Some(ext), Some(stem)) if ext == "jsonl" => stem.to_string_lossy().into_owned(),
                _ => continue,
            };
            let safe = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
            if id.is_empty() || !id.chars().all(safe) {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            if !entry.file_type().is_ok_and(|t| t.is_file()) {
                continue;
            }
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs());
            files.push((modified, path, id));
        }
    }
    files.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    files
}

/// What one transcript has for the query.
#[derive(Default)]
struct Found {
    matches: u32,
    /// The newest matching message, whitespace collapsed.
    newest: String,
    title: Option<String>,
    cwd: Option<String>,
}

impl Found {
    fn hit(self, conversation: &str, modified: u64) -> SearchHit {
        SearchHit {
            conversation: conversation.to_owned(),
            cwd: self.cwd.unwrap_or_default(),
            modified,
            title: self.title.unwrap_or_default(),
            snippet: self.newest,
            matches: self.matches,
        }
    }
}

/// Scan the tail of the transcript at `path`. Returns what matched (if
/// anything did) and whether the file was read to its end within the
/// budget. An unreadable file is read to its end, with nothing in it.
fn scan(
    path: &Path,
    terms: &[String],
    budget: &Budget,
    spent: &mut Spent,
) -> (Option<Found>, bool) {
    let Ok(mut file) = File::open(path) else {
        return (None, true);
    };
    let len = file.metadata().map_or(0, |m| m.len());
    let start = len.saturating_sub(budget.tail);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return (None, true);
    }
    let mut reader = BufReader::with_capacity(BUFFER, file.take(budget.tail));
    // ASCII words can be looked for in the raw line first: JSON leaves
    // them as they are. Lines without them are not parsed.
    let ascii: Vec<&[u8]> = (terms.iter())
        .filter(|t| {
            t.bytes()
                .all(|b| b.is_ascii_graphic() && b != b'"' && b != b'\\')
        })
        .map(|t| t.as_bytes())
        .collect();
    let mut found = Found::default();
    let (mut line, mut lower) = (Vec::new(), Vec::new());
    // A tail starts inside a line: that line is skipped.
    let mut partial = start > 0;
    let mut finished = true;
    loop {
        if spent.over(budget) {
            // Out of budget, unless there was nothing left anyway.
            finished = reader.fill_buf().map_or(true, |rest| rest.is_empty());
            break;
        }
        let Ok((used, fits)) = read_line(&mut reader, &mut line, budget.line) else {
            break;
        };
        if used == 0 {
            break;
        }
        spent.bytes += used as u64;
        if std::mem::take(&mut partial) || !fits {
            continue;
        }
        let wanted = contains(&line, b"\"user\"") || contains(&line, b"\"assistant\"");
        let meta_missing = found.title.is_none() || found.cwd.is_none();
        if !wanted {
            continue;
        }
        if !meta_missing {
            lower.clear();
            lower.extend(line.iter().map(u8::to_ascii_lowercase));
            if !ascii.iter().all(|t| contains(&lower, t)) {
                continue;
            }
        }
        examine(&line, terms, &mut found);
    }
    ((found.matches > 0).then_some(found), finished)
}

/// Read the next line into `line` (without its `\n`), unless it is longer
/// than `max`: then its bytes are read and dropped, never kept. Returns the
/// bytes read (0 at the end) and whether `line` holds the line.
fn read_line(
    reader: &mut impl BufRead,
    line: &mut Vec<u8>,
    max: usize,
) -> io::Result<(usize, bool)> {
    line.clear();
    let (mut used, mut fits) = (0, true);
    loop {
        let buf = reader.fill_buf()?;
        if buf.is_empty() {
            return Ok((used, fits));
        }
        let newline = buf.iter().position(|&b| b == b'\n');
        let chunk = &buf[..newline.unwrap_or(buf.len())];
        if fits && line.len() + chunk.len() > max {
            fits = false;
            line.clear();
        } else if fits {
            line.extend_from_slice(chunk);
        }
        let n = newline.map_or(chunk.len(), |i| i + 1);
        reader.consume(n);
        used += n;
        if newline.is_some() {
            return Ok((used, fits));
        }
    }
}

/// Whether `haystack` contains `needle`.
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    needle.is_empty() || haystack.windows(needle.len()).any(|w| w == needle)
}

/// Fold one transcript record into `found`.
fn examine(line: &[u8], terms: &[String], found: &mut Found) {
    let Ok(record) = serde_json::from_slice::<serde_json::Value>(line) else {
        return;
    };
    if flag(&record, "isSidechain") {
        return;
    }
    if found.cwd.is_none() {
        let cwd = record.get("cwd").and_then(|c| c.as_str());
        found.cwd = cwd
            .map(sanitize)
            .filter(|c| c.len() <= MAX_CWD && !c.contains('\n'));
    }
    let text = match record.get("type").and_then(|t| t.as_str()) {
        Some("user") => {
            let prompt = answer_prompt(&record);
            if found.title.is_none() {
                found.title = prompt
                    .as_deref()
                    .map(|p| one_line(&sanitize(p), TITLE_CHARS));
            }
            prompt
        }
        Some("assistant") if !flag(&record, "isApiErrorMessage") => {
            let mut texts = Vec::new();
            push_texts(
                &mut texts,
                record.get("message").and_then(|m| m.get("content")),
            );
            Some(texts.join("\n\n"))
        }
        _ => None,
    };
    let Some(text) = text else { return };
    let text = sanitize(&text);
    let lower = text.to_lowercase();
    if terms.iter().all(|t| lower.contains(t.as_str())) {
        found.matches = found.matches.saturating_add(1);
        found.newest = snippet(&text, &terms[0]);
    }
}

/// `text` on one line (whitespace runs as one space), cut to `max`
/// characters with `…`.
fn one_line(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let mut cut: String = flat.chars().take(max - 1).collect();
    cut.push('…');
    cut
}

/// At most [`SNIPPET_CHARS`] characters of `text` on one line, around the
/// first place `term` (lowercase) appears, `…` where cut.
fn snippet(text: &str, term: &str) -> String {
    let flat: Vec<char> = one_line(text, usize::MAX).chars().collect();
    if flat.len() <= SNIPPET_CHARS {
        return flat.into_iter().collect();
    }
    let lower: String = flat.iter().collect::<String>().to_lowercase();
    // Lowercasing can change a character's length: the position is a
    // character count of the lowercase text, close enough for a window.
    let at = lower.find(term).map_or(0, |i| lower[..i].chars().count());
    let start = at
        .saturating_sub(SNIPPET_LEAD)
        .min(flat.len() - SNIPPET_CHARS);
    let end = start + SNIPPET_CHARS;
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    let from = start + usize::from(start > 0);
    let to = end - usize::from(end < flat.len());
    out.extend(&flat[from..to]);
    if end < flat.len() {
        out.push('…');
    }
    out
}
```

`src/claude/projects.rs` (visibility only):

```diff
--- a/src/claude/projects.rs
+++ b/src/claude/projects.rs
@@ -622,14 +622,14 @@ pub fn recent_answers(bytes: &[u8]) -> Vec<Answer> {
 }
 
 /// Whether `record` has `key: true`.
-fn flag(record: &serde_json::Value, key: &str) -> bool {
+pub(crate) fn flag(record: &serde_json::Value, key: &str) -> bool {
     record.get(key).and_then(|f| f.as_bool()) == Some(true)
 }
 
 /// The prompt of a `type:"user"` record that starts an answer (see
 /// [`recent_answers`]): its string content, or its text blocks joined by
 /// newlines.
-fn answer_prompt(record: &serde_json::Value) -> Option<String> {
+pub(crate) fn answer_prompt(record: &serde_json::Value) -> Option<String> {
     let own = ["isMeta", "isCompactSummary", "isVisibleInTranscriptOnly"];
     if own.iter().any(|key| flag(record, key)) || !is_real_prompt(record) {
         return None;
@@ -644,7 +644,7 @@ fn answer_prompt(record: &serde_json::Value) -> Option<String> {
 
 /// Append the text of a message's `content` to `texts`: the string itself,
 /// or its `text` blocks (not `thinking`, `tool_use` or `tool_result`).
-fn push_texts(texts: &mut Vec<String>, content: Option<&serde_json::Value>) {
+pub(crate) fn push_texts(texts: &mut Vec<String>, content: Option<&serde_json::Value>) {
     match content {
         Some(serde_json::Value::String(text)) => texts.push(text.clone()),
         Some(serde_json::Value::Array(blocks)) => {
```

`src/proto.rs` (the other hunks). Also, in `DAEMON_FEATURES` (added by Task 1), append `"search"`: `&["lens", "search"]` (or `&["lens", "search", "last_prompt"]` if Task 10 landed first).

```diff
--- a/src/proto.rs
+++ b/src/proto.rs
@@ -190,6 +190,16 @@ pub enum Msg {
         #[serde(default)]
         old_path: Option<String>,
     },
+    /// Search this host's claude conversations (prompts and answers) for
+    /// every word of `query`; at most `limit` hits (0: the default, see
+    /// [`crate::claude::search`]). Answered by `SearchReply`, or `Error
+    /// "search busy"` while another search runs on this daemon, or `Error
+    /// "bad query: …"`. An older daemon answers `Error "unsupported op…"`.
+    Search {
+        query: String,
+        #[serde(default)]
+        limit: u32,
+    },
 
     // ── daemon → client replies ──────────────────────────────────────────
     Sessions {
@@ -237,6 +247,16 @@ pub enum Msg {
     GitLogPage(GitLogPage),
     GitCommitInfo(GitCommitInfo),
     GitPatch(GitPatch),
+    /// Reply to `Search`, built by [`search_reply`]: one hit per
+    /// conversation, newest first. `scanned` of `total` transcripts were
+    /// read to their end; `truncated` when the search stopped early.
+    SearchReply {
+        query: String,
+        hits: Vec<SearchHit>,
+        scanned: u32,
+        total: u32,
+        truncated: bool,
+    },
     Ok,
     Error {
         message: String,
@@ -711,6 +731,55 @@ pub fn answers_reply(id: SessionId, answers: Vec<Answer>) -> Msg {
     msg
 }
 
+/// The largest encoded `SearchReply` the daemon sends (as for answers).
+pub const SEARCH_REPLY_MAX: usize = 512 * 1024;
+
+/// One conversation that matched a `Search`. Every text is sanitized by
+/// the daemon; `title` is its first prompt and `snippet` the newest match,
+/// each on one line.
+#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
+pub struct SearchHit {
+    /// The claude session id (the transcript's name), for `--resume`.
+    pub conversation: String,
+    pub cwd: String,
+    /// Unix seconds of the transcript's last modification.
+    pub modified: u64,
+    pub title: String,
+    pub snippet: String,
+    /// How many messages matched.
+    pub matches: u32,
+}
+
+/// The `SearchReply` for `query`, without as many of the last hits as it
+/// takes for its JSON to fit [`SEARCH_REPLY_MAX`] (then `truncated`).
+pub fn search_reply(
+    query: String,
+    hits: Vec<SearchHit>,
+    scanned: u32,
+    total: u32,
+    truncated: bool,
+) -> Msg {
+    let mut msg = Msg::SearchReply {
+        query,
+        hits,
+        scanned,
+        total,
+        truncated,
+    };
+    while serde_json::to_vec(&msg).map_or(false, |json| json.len() > SEARCH_REPLY_MAX) {
+        match &mut msg {
+            Msg::SearchReply {
+                hits, truncated, ..
+            } if !hits.is_empty() => {
+                hits.pop();
+                *truncated = true;
+            }
+            _ => break,
+        }
+    }
+    msg
+}
+
 /// The most commits one `GitLog` request may ask for.
 pub const GIT_LOG_MAX: u32 = 500;
 
```

`src/daemon/mod.rs`:

```diff
--- a/src/daemon/mod.rs
+++ b/src/daemon/mod.rs
@@ -346,6 +346,8 @@ pub(crate) struct Daemon {
     pub(crate) shutdown: tokio::sync::Notify,
     /// Latest host resource snapshot, updated every ~2 s by the stats sampler.
     pub(crate) stats_rx: tokio::sync::watch::Receiver<stats::HostSnapshot>,
+    /// `Search`: one at a time, and the transcripts it reads.
+    pub(crate) search: server::SearchSlot,
 }
 
 /// Journaled sessions, and the live subset with a running process.
@@ -384,6 +386,7 @@ impl Daemon {
             skip_permissions_supported: Mutex::new(None),
             shutdown: tokio::sync::Notify::new(),
             stats_rx,
+            search: server::SearchSlot::new(),
         }
     }
 
```

`src/daemon/server.rs`:

```diff
--- a/src/daemon/server.rs
+++ b/src/daemon/server.rs
@@ -28,6 +28,7 @@ use tokio::sync::{broadcast, mpsc, Semaphore};
 use super::session::{ClientId, Cmd};
 use super::{git, host, update, Config, Daemon};
 use crate::claude::projects;
+use crate::claude::search::{self, BadQuery, Budget};
 use crate::paths;
 use crate::proto::{
     self, Envelope, Frame, Msg, ProjectDir, SessionId, SessionKind, SpawnSpec, Welcome, MAX_FRAME,
@@ -542,6 +546,10 @@ impl Client {
                 });
                 return;
             }
+            Msg::Search { query, limit } => match self.spawn_search(req, query, limit) {
+                Ok(()) => return,
+                Err(refused) => refused,
+            },
             other => Msg::Error {
                 message: format!("unsupported op: {}", op_name(&other)),
             },
@@ -599,6 +607,44 @@ impl Client {
         Ok(())
     }
 
+    /// Run a search off the client loop and queue its reply when done. One
+    /// search runs per daemon: while one does, another is refused at once
+    /// with "search busy". A bad query is refused before anything runs.
+    fn spawn_search(&self, req: Option<u64>, query: String, limit: u32) -> Result<(), Msg> {
+        if let Err(BadQuery(why)) = search::terms(&query) {
+            return Err(Msg::Error {
+                message: format!("bad query: {why}"),
+            });
+        }
+        let permit = Arc::clone(&self.daemon.search.running)
+            .try_acquire_owned()
+            .map_err(|_| Msg::Error {
+                message: "search busy".into(),
+            })?;
+        let root = self.daemon.search.root();
+        let queue = self.queue.clone();
+        tokio::spawn(async move {
+            let limit = usize::try_from(limit).unwrap_or(usize::MAX);
+            let scan = tokio::task::spawn_blocking(move || {
+                // Held until the scan ends, even if the client has left.
+                let _permit = permit;
+                let outcome = search::search(&root, &query, limit, Budget::default());
+                outcome.map(|o| proto::search_reply(query, o.hits, o.scanned, o.total, o.truncated))
+            });
+            let msg = match scan.await {
+                Ok(Ok(reply)) => reply,
+                Ok(Err(BadQuery(why))) => Msg::Error {
+                    message: format!("bad query: {why}"),
+                },
+                Err(e) => Msg::Error {
+                    message: format!("search failed: {e}"),
+                },
+            };
+            let _ = queue.send(Frame::Control(Envelope { req, msg })).await;
+        });
+        Ok(())
+    }
+
     /// Send `cmd` to a live session; the reply is `Ok` or `Error`.
     async fn to_session(&self, id: SessionId, cmd: Cmd) -> Msg {
         match self.daemon.session(id) {
@@ -760,6 +806,41 @@ async fn recent_projects(limit: u32) -> Msg {
     }
 }
 
+/// `Search` state shared by a daemon's clients.
+pub(crate) struct SearchSlot {
+    /// One permit: the search that holds it is the one running.
+    running: Arc<Semaphore>,
+    /// Where the transcripts are: [`projects::projects_root`] unless a test
+    /// set its own.
+    root: std::sync::Mutex<Option<std::path::PathBuf>>,
+}
+
+impl SearchSlot {
+    pub(crate) fn new() -> SearchSlot {
+        SearchSlot {
+            running: Arc::new(Semaphore::new(1)),
+            root: std::sync::Mutex::new(None),
+        }
+    }
+
+    fn root(&self) -> std::path::PathBuf {
+        let root = self.root.lock().unwrap_or_else(|e| e.into_inner());
+        root.clone().unwrap_or_else(projects::projects_root)
+    }
+
+    /// Search `root` instead of the user's transcripts.
+    #[cfg(test)]
+    pub(crate) fn set_root(&self, root: std::path::PathBuf) {
+        *self.root.lock().unwrap_or_else(|e| e.into_inner()) = Some(root);
+    }
+
+    /// Take the slot as a running search would, until the permit drops.
+    #[cfg(test)]
+    pub(crate) fn hold(&self) -> tokio::sync::OwnedSemaphorePermit {
+        Arc::clone(&self.running).try_acquire_owned().expect("free")
+    }
+}
+
 /// The wire `op` of a message, for error text.
 /// The reply to a spawn or respawn of `id`.
 fn spawned(id: SessionId, result: io::Result<Option<u32>>) -> Msg {
```

Run: `umask 022; cargo test --locked --bin claudio search`
Expected (the 10 new tests plus the existing `tui::wizard::tests::non_path_input_is_still_a_fuzzy_seed_search`; the filtered-out count depends on what is merged):
```
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 899 filtered out; finished in 0.88s
```
Run: `umask 022; cargo test --locked --bin claudio`
Expected: the previous total + 10 (scratch, on base + Task 8 + the Task 1 stand-in: `test result: ok. 910 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`).
Run: `rustfmt --edition 2021 --check src/claude/search.rs src/daemon/server.rs` → no output. `cargo build --locked` → 0 warnings.

Step 3: commit
`/usr/bin/git add src/claude/search.rs && /usr/bin/git commit -m "feat(daemon): search claude conversations on the host" -- src/claude/search.rs src/claude/mod.rs src/claude/projects.rs src/proto.rs src/daemon/mod.rs src/daemon/server.rs src/daemon/tests.rs`

Interfaces this task provides to later tasks:
- `Msg::Search { query: String, #[serde(default)] limit: u32 }`, `Msg::SearchReply { query, hits: Vec<SearchHit>, scanned: u32, total: u32, truncated: bool }`, `proto::SearchHit { conversation, cwd, modified, title, snippet, matches }`, `proto::SEARCH_REPLY_MAX`, `proto::search_reply(query, hits, scanned, total, truncated) -> Msg`: as interfaces.md. `Welcome.features` contains `"search"`.
- Daemon errors the client maps (Task 11): `"search busy"` → notice `search busy, try again`; `"bad query: <why>"` → a notice with the reason; `"unsupported op: search"` → the restart notice.
- `hit.conversation` always matches `[A-Za-z0-9_-]+` (other file names are skipped): safe as the `--resume` argument. `hit.cwd` is `""` when no record carried one: Task 11 must not spawn in `""`.
- `crate::claude::search::{search, terms, Budget, Outcome, BadQuery}`; test hooks `Daemon.search.set_root(path)` and `Daemon.search.hold()` (cfg(test)) for daemon-level tests.


### Task 10: last prompt op and snippets store
Model: sonnet   Repo: <repo> (feat/extensions after Task 1, or the controller's worktree)
Files: create `src/tui/snippets.rs`; modify `src/tui/mod.rs` (two lines after `mod sessions;`), `src/claude/projects.rs` (`read_answers` split into `read_tail`; `read_last_prompt`; `MAX_LAST_PROMPT` and `last_prompt` after `cut`; tests at the end), `src/proto.rs` (`Msg::LastPrompt` after `Answers`, `Msg::LastPromptReply` after `AnswersReply`, one test before `an_answers_reply_always_fits_a_frame`, `"last_prompt"` in `DAEMON_FEATURES`), `src/daemon/server.rs` (arm after the `Answers` arm; `answers` uses the new `transcript` helper; `last_prompt` and `transcript` after `answers`), `src/daemon/tests.rs` (one test before `kill_removes_session_everywhere`).
Depends on: Task 1 (`DAEMON_FEATURES`). Parallel with Tasks 8 and 9 (see Task 9: only the `DAEMON_FEATURES` line conflicts).
Goal: spec §9 and AC10 (load/save unit parts) plus the F4 op. `LastPrompt` answers the newest real prompt of the transcript tail (`answer_prompt`'s rules, answered or not; sidechains skipped), sanitized and trimmed, ≤ 64 KiB cut on a char boundary with no mark and `cut: true`; `prompt: None` when there is none; an unknown session → `Error "no such session: …"`. Snippets: `load` (`*.md`/`*.txt`, ≤ 64 KiB each, read without trusting the size; the first 500 by name; hidden, non-regular and non-UTF-8 files skipped; name = stem; sanitized; missing dir → empty), `save` (`slug`, then `-2`, `-3`… while a `.md` or `.txt` has the name; dir 0700 via `paths::ensure_private_dir`, file 0600 via `paths::write_atomic`), `slug` (first words, `[a-z0-9-]`, ≤ 40, `snippet` when empty), `dir()`.

Step 1: write the failing tests

In `src/tui/mod.rs`, after the line `mod sessions;` add:
```rust
#[allow(dead_code)] // Wired into the snippets actions by Task 11, which removes this.
mod snippets;
```
Create `src/tui/snippets.rs` with a module doc and the tests (Step 2 puts the implementation between them):

```rust
//! Saved prompts: the files of `~/.config/claudio/snippets/`.

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn scratch() -> PathBuf {
        std::env::temp_dir().join(format!("claudio-snippets-{}", uuid::Uuid::new_v4()))
    }

    fn snippet(name: &str, text: &str) -> Snippet {
        Snippet {
            name: name.into(),
            text: text.into(),
        }
    }

    #[test]
    fn slugs_are_the_first_words_in_lowercase_ascii() {
        assert_eq!(slug("Review this PR, please!"), "review-this-pr-please");
        assert_eq!(slug("  --fix   the_tests\n now"), "fix-the-tests-now");
        assert_eq!(
            slug("one two three four five six seven eight nine ten"),
            "one-two-three-four-five-six-seven-eight"
        );
        assert_eq!(slug(&"x".repeat(50)), "x".repeat(MAX_SLUG));
        assert_eq!(slug("Résumé ok"), "r-sum-ok");
        assert_eq!(slug("¿?"), "snippet");
        assert_eq!(slug(""), "snippet");
        assert!(slug("../../etc/passwd")
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-'));
    }

    #[test]
    fn load_reads_md_and_txt_sorted_capped_and_sanitized() {
        let dir = scratch();
        assert_eq!(load(&dir).unwrap(), []);
        fs::create_dir_all(dir.join("sub.md")).unwrap();
        fs::write(dir.join("b.md"), "bee\n").unwrap();
        fs::write(dir.join("a.txt"), "ay\u{1b}[2J\tx").unwrap();
        fs::write(dir.join("c.json"), "{}").unwrap();
        fs::write(dir.join(".hidden.md"), "no").unwrap();
        fs::write(dir.join("big.md"), "x".repeat(MAX_BYTES as usize + 1)).unwrap();
        fs::write(dir.join("max.md"), "y".repeat(MAX_BYTES as usize)).unwrap();
        fs::write(dir.join("bad.md"), [0xff, 0xfe]).unwrap();
        let loaded = load(&dir).unwrap();
        let names: Vec<&str> = loaded.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["a", "b", "max"]);
        assert_eq!(loaded[0], snippet("a", "ay[2J\tx"));
        assert_eq!(loaded[1], snippet("b", "bee\n"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_keeps_the_first_500_by_name() {
        let dir = scratch();
        fs::create_dir_all(&dir).unwrap();
        for i in 0..MAX_FILES + 5 {
            fs::write(dir.join(format!("s{i:04}.md")), "t").unwrap();
        }
        let loaded = load(&dir).unwrap();
        assert_eq!(loaded.len(), MAX_FILES);
        assert_eq!(loaded[0].name, "s0000");
        assert_eq!(loaded[MAX_FILES - 1].name, "s0499");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_names_by_slug_avoids_collisions_and_is_private() {
        let dir = scratch().join("snippets");
        assert_eq!(save(&dir, "Fix the tests").unwrap(), "fix-the-tests");
        assert_eq!(save(&dir, "fix the tests!").unwrap(), "fix-the-tests-2");
        fs::write(dir.join("fix-the-tests-3.txt"), "taken").unwrap();
        assert_eq!(save(&dir, "FIX THE TESTS").unwrap(), "fix-the-tests-4");
        assert_eq!(save(&dir, "¿").unwrap(), "snippet");
        let text = "line one\n\tline two\n";
        let name = save(&dir, text).unwrap();
        let path = dir.join(format!("{name}.md"));
        assert_eq!(fs::read_to_string(&path).unwrap(), text);
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(&dir), 0o700);
        // Saved snippets load back under their names.
        let loaded = load(&dir).unwrap();
        assert!(
            loaded.contains(&snippet("line-one-line-two", text)),
            "{loaded:?}"
        );
        assert!(loaded.contains(&snippet("fix-the-tests-2", "fix the tests!")));
        let _ = fs::remove_dir_all(dir.parent().unwrap());
    }
}
```

In `src/claude/projects.rs`, at the end of the tests module:

```diff
--- a/src/claude/projects.rs
+++ b/src/claude/projects.rs
@@ -1478,4 +1524,70 @@ mod tests {
         assert!(read_answers(&dir.join("missing.jsonl")).is_empty());
         let _ = std::fs::remove_dir_all(&dir);
     }
+
+    // ── last_prompt ───────────────────────────────────────────────────────
+
+    #[test]
+    fn the_last_prompt_is_the_newest_real_one_answered_or_not() {
+        use serde_json::json;
+        let mut meta = prompt_record(json!("caveat"));
+        meta["isMeta"] = json!(true);
+        let mut side = prompt_record(json!("a subagent's task"));
+        side["isSidechain"] = json!(true);
+        let transcript = jsonl(&[
+            prompt_record(json!("old question")),
+            answer_record(json!("old answer")),
+            prompt_record(json!([{"type": "text", "text": "  the newest\nquestion  "}])),
+            // Still unanswered: tool calls and their results only.
+            answer_record(json!([{"type": "tool_use", "id": "t", "name": "Bash", "input": {}}])),
+            prompt_record(json!([{"type": "tool_result", "tool_use_id": "t", "content": "out"}])),
+            prompt_record(json!("<command-name>/model</command-name>")),
+            meta,
+            side,
+            json!({"type": "system", "subtype": "compact_boundary"}),
+        ]);
+        assert_eq!(
+            last_prompt(&transcript),
+            Some(("the newest\nquestion".into(), false))
+        );
+        assert_eq!(last_prompt(b""), None);
+        assert_eq!(
+            last_prompt(&jsonl(&[answer_record(json!("no prompt"))])),
+            None
+        );
+    }
+
+    #[test]
+    fn the_last_prompt_is_sanitized_and_cut_without_a_mark() {
+        use serde_json::json;
+        let dirty = jsonl(&[prompt_record(json!("why\u{1b}[2J\u{202e}?"))]);
+        assert_eq!(last_prompt(&dirty), Some(("why[2J?".into(), false)));
+        let long = format!("{}é{}", "p".repeat(MAX_LAST_PROMPT - 1), "q".repeat(10));
+        let (text, cut) = last_prompt(&jsonl(&[prompt_record(json!(long))])).unwrap();
+        assert!(cut);
+        assert_eq!(text, "p".repeat(MAX_LAST_PROMPT - 1));
+        let exact = "x".repeat(MAX_LAST_PROMPT);
+        assert_eq!(
+            last_prompt(&jsonl(&[prompt_record(json!(exact.clone()))])),
+            Some((exact, false))
+        );
+    }
+
+    #[test]
+    fn the_last_prompt_is_read_from_the_tail_of_a_transcript() {
+        use serde_json::json;
+        let dir = std::env::temp_dir().join(format!("claudio-last-{}", uuid::Uuid::new_v4()));
+        std::fs::create_dir_all(&dir).unwrap();
+        let path = dir.join("t.jsonl");
+        let mut transcript = jsonl(&[prompt_record(json!("x".repeat(ANSWERS_TAIL as usize)))]);
+        transcript.extend(jsonl(&[answer_record(json!("a"))]));
+        std::fs::write(&path, &transcript).unwrap();
+        // The only prompt starts before the tail: none is read.
+        assert_eq!(read_last_prompt(&path), None);
+        transcript.extend(jsonl(&[prompt_record(json!("again"))]));
+        std::fs::write(&path, &transcript).unwrap();
+        assert_eq!(read_last_prompt(&path), Some(("again".into(), false)));
+        assert_eq!(read_last_prompt(&dir.join("missing.jsonl")), None);
+        let _ = std::fs::remove_dir_all(&dir);
+    }
 }
```

In `src/proto.rs` (tests module), before `fn an_answers_reply_always_fits_a_frame`:

```diff
--- a/src/proto.rs
+++ b/src/proto.rs
@@ -1173,6 +1188,44 @@ mod tests {
         assert_eq!(serde_json::from_str::<V04>(&json).unwrap(), V04::Unknown);
     }
 
+    #[test]
+    fn last_prompt_round_trips_and_is_unknown_to_older_daemons() {
+        #[derive(Debug, PartialEq, Deserialize)]
+        #[serde(tag = "op", rename_all = "snake_case")]
+        enum V05 {
+            Ping,
+            #[serde(other)]
+            Unknown,
+        }
+        let ask = Msg::LastPrompt { id: Uuid::nil() };
+        let json = serde_json::to_string(&Envelope::request(4, ask.clone())).unwrap();
+        assert!(json.contains(r#""op":"last_prompt""#), "{json}");
+        assert_eq!(serde_json::from_str::<V05>(&json).unwrap(), V05::Unknown);
+        assert_eq!(serde_json::from_str::<Envelope>(&json).unwrap().msg, ask);
+        // The largest reply (64 KiB of quotes, escaped) fits a frame.
+        let reply = Msg::LastPromptReply {
+            id: Uuid::nil(),
+            prompt: Some("\"".repeat(64 * 1024)),
+            cut: true,
+        };
+        let frame = Frame::Control(Envelope::request(4, reply.clone()));
+        assert!(frame.encode().len() - 4 <= MAX_FRAME);
+        assert_eq!(roundtrip(frame.clone()), frame);
+        let old: Msg = serde_json::from_str(&format!(
+            r#"{{"op":"last_prompt_reply","id":"{}","prompt":null}}"#,
+            Uuid::nil()
+        ))
+        .unwrap();
+        assert_eq!(
+            old,
+            Msg::LastPromptReply {
+                id: Uuid::nil(),
+                prompt: None,
+                cut: false
+            }
+        );
+    }
+
     #[test]
     fn an_answers_reply_always_fits_a_frame() {
         // Ten answers of 64 KiB made only of backslashes: twice that once
```

In `src/daemon/tests.rs`, before `async fn kill_removes_session_everywhere`:

```diff
--- a/src/daemon/tests.rs
+++ b/src/daemon/tests.rs
@@ -579,6 +579,38 @@ async fn answers_are_empty_without_a_transcript_and_refused_for_unknown_sessions
     }
 }
 
+/// `LastPrompt` is served like `Answers`: no conversation (or no
+/// transcript) has no prompt, an unknown session is an error.
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn last_prompt_is_none_without_a_transcript_and_refused_for_unknown_sessions() {
+    let d = TestDaemon::start().await;
+    let mut c = d.client().await;
+    let id = Uuid::new_v4();
+    c.spawn(d.spec(id)).await;
+    let none = Msg::LastPromptReply {
+        id,
+        prompt: None,
+        cut: false,
+    };
+    assert_eq!(c.call(Msg::LastPrompt { id }).await, none);
+    let payload = serde_json::json!({
+        "session_id": "0a1b2c3d-0000-4000-8000-000000000001",
+        "source": "startup",
+    });
+    send_hook(&d.config.socket, &d.token().await, "SessionStart", payload).await;
+    c.event(id, |e| {
+        matches!(e, SessionEvent::ClaudeSession { .. }).then_some(())
+    })
+    .await;
+    assert_eq!(c.call(Msg::LastPrompt { id }).await, none);
+    let unknown = Uuid::new_v4();
+    match c.call(Msg::LastPrompt { id: unknown }).await {
+        Msg::Error { message } => assert!(message.contains("no such session"), "{message}"),
+        other => panic!("expected an error, got {other:?}"),
+    }
+    assert!(proto::DAEMON_FEATURES.contains(&"last_prompt"));
+}
+
 #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
 async fn kill_removes_session_everywhere() {
     let d = TestDaemon::start().await;
```

Run: `umask 022; cargo test --locked --bin claudio`
Expected (compile failure), among others:
```
error[E0425]: cannot find function `slug` in this scope
error[E0425]: cannot find function `last_prompt` in this scope
error[E0599]: no variant named `LastPrompt` found for enum `proto::Msg`
error: could not compile `claudio` (bin "claudio" test) due to 63 previous errors; 1 warning emitted
```

Step 2: implement

`src/tui/snippets.rs`: replace the step-1 doc line with the following, keeping the tests module after it:

```rust
//! Saved prompts: the files of `~/.config/claudio/snippets/`.
//!
//! File I/O, so it is called only from the effect runner in `tui/mod.rs`,
//! never from `App`. Names on disk are always made by [`slug`], never taken
//! from claude's text as a path; files are written 0600 in a 0700 dir.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use crate::claude::latex::sanitize;
use crate::paths;

/// The largest snippet file read.
pub const MAX_BYTES: u64 = 64 * 1024;
/// The most snippets listed.
pub const MAX_FILES: usize = 500;
/// The longest name [`slug`] makes.
pub const MAX_SLUG: usize = 40;

/// A saved prompt: its file's stem and its text, both sanitized.
#[derive(Debug, Clone, PartialEq)]
pub struct Snippet {
    pub name: String,
    pub text: String,
}

/// The snippets directory (`$XDG_CONFIG_HOME` honoured).
pub fn dir() -> PathBuf {
    paths::config_dir().join("snippets")
}

/// The snippets in `dir`: its `*.md` and `*.txt` files of at most
/// [`MAX_BYTES`], sorted by name, at most [`MAX_FILES`]. Hidden, larger,
/// unreadable and non-UTF-8 files are skipped; a missing dir has none.
pub fn load(dir: &Path) -> io::Result<Vec<Snippet>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some((stem, ext)) = name.rsplit_once('.') else {
            continue;
        };
        if !stem.is_empty() && !name.starts_with('.') && matches!(ext, "md" | "txt") {
            files.push((stem.to_owned(), path));
        }
    }
    files.sort();
    let mut snippets = Vec::new();
    for (stem, path) in files {
        if snippets.len() == MAX_FILES {
            break;
        }
        if let Some(text) = read(&path) {
            let name: String = sanitize(&stem)
                .chars()
                .map(|c| if c == '\n' || c == '\t' { ' ' } else { c })
                .collect();
            snippets.push(Snippet {
                name,
                text: sanitize(&text),
            });
        }
    }
    Ok(snippets)
}

/// The text of the snippet file at `path`, if it is a regular file of at
/// most [`MAX_BYTES`] of UTF-8 (read without trusting its size).
fn read(path: &Path) -> Option<String> {
    let meta = fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_BYTES {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// Save `text` as a new snippet in `dir` (created 0700) and return its
/// name: [`slug`] of the text, with `-2`, `-3`… when the name is taken by
/// a `.md` or `.txt` file. The file is `<name>.md`, 0600, written whole
/// (temp file + rename).
pub fn save(dir: &Path, text: &str) -> io::Result<String> {
    paths::ensure_private_dir(dir)?;
    let base = slug(text);
    for n in 1..=1000 {
        let name = if n == 1 {
            base.clone()
        } else {
            format!("{base}-{n}")
        };
        let taken = ["md", "txt"]
            .iter()
            .any(|ext| dir.join(format!("{name}.{ext}")).symlink_metadata().is_ok());
        if !taken {
            paths::write_atomic(&dir.join(format!("{name}.md")), text.as_bytes())?;
            return Ok(name);
        }
    }
    Err(io::Error::other(format!("too many snippets named {base}")))
}

/// A file name for `text`: its first words, lowercase ASCII letters and
/// digits joined by `-`, at most [`MAX_SLUG`] characters; `snippet` when
/// nothing is left.
pub fn slug(text: &str) -> String {
    let mut out = String::new();
    let words = text
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty());
    for word in words {
        let word = word.to_ascii_lowercase();
        let sep = usize::from(!out.is_empty());
        if out.len() + sep + word.len() > MAX_SLUG {
            if out.is_empty() {
                out = word[..MAX_SLUG].to_owned();
            }
            break;
        }
        if sep == 1 {
            out.push('-');
        }
        out.push_str(&word);
    }
    if out.is_empty() {
        "snippet".into()
    } else {
        out
    }
}
```

`src/claude/projects.rs`:

```diff
--- a/src/claude/projects.rs
+++ b/src/claude/projects.rs
@@ -541,9 +541,22 @@ const NOT_PROMPTS: [&str; 5] = [
 ];
 
 /// The [`recent_answers`] of the transcript at `path`, from its last
-/// [`ANSWERS_TAIL`] bytes; a tail's first (partial) line is skipped. A
-/// missing or unreadable transcript has none. Blocking I/O.
+/// [`ANSWERS_TAIL`] bytes ([`read_tail`]). A missing or unreadable
+/// transcript has none. Blocking I/O.
 pub fn read_answers(path: &Path) -> Vec<Answer> {
+    recent_answers(&read_tail(path))
+}
+
+/// The [`last_prompt`] of the transcript at `path`, from its last
+/// [`ANSWERS_TAIL`] bytes ([`read_tail`]). Blocking I/O.
+pub fn read_last_prompt(path: &Path) -> Option<(String, bool)> {
+    last_prompt(&read_tail(path))
+}
+
+/// The whole lines of the last [`ANSWERS_TAIL`] bytes of the transcript at
+/// `path`: a tail's first (partial) line is skipped. Empty when the file is
+/// missing or unreadable.
+fn read_tail(path: &Path) -> Vec<u8> {
     use std::io::{Read, Seek, SeekFrom};
     let Ok(mut f) = std::fs::File::open(path) else {
         return Vec::new();
@@ -556,12 +569,11 @@ pub fn read_answers(path: &Path) -> Vec<Answer> {
     {
         return Vec::new();
     }
-    let lines = match buf.iter().position(|&b| b == b'\n') {
-        Some(nl) if start > 0 => &buf[nl + 1..],
-        None if start > 0 => &[],
-        _ => &buf[..],
-    };
-    recent_answers(lines)
+    match buf.iter().position(|&b| b == b'\n') {
+        Some(nl) if start > 0 => buf.split_off(nl + 1),
+        None if start > 0 => Vec::new(),
+        _ => buf,
+    }
 }
 
 /// The answers of a transcript's lines, oldest first: the last
@@ -694,6 +706,40 @@ fn cut(s: &str, max: usize) -> String {
     format!("{}{CUT_MARK}", &s[..end])
 }
 
+/// The longest prompt [`last_prompt`] returns whole.
+pub const MAX_LAST_PROMPT: usize = 64 * 1024;
+
+/// The newest real prompt of a transcript's lines, answered or not (the
+/// prompts of [`recent_answers`]; sidechain records are skipped):
+/// [`sanitize`]d and trimmed, and cut to [`MAX_LAST_PROMPT`] bytes on a char
+/// boundary, with no mark, when longer (then `true`). `None` when there is
+/// none.
+pub fn last_prompt(bytes: &[u8]) -> Option<(String, bool)> {
+    bytes.split(|&b| b == b'\n').rev().find_map(|line| {
+        if !line.windows(6).any(|w| w == b"\"user\"") {
+            return None;
+        }
+        let record: serde_json::Value = serde_json::from_slice(line).ok()?;
+        let user = record.get("type").and_then(|t| t.as_str()) == Some("user");
+        if !user || flag(&record, "isSidechain") {
+            return None;
+        }
+        let prompt = sanitize(answer_prompt(&record)?.trim());
+        let prompt = prompt.trim();
+        if prompt.is_empty() {
+            return None;
+        }
+        if prompt.len() <= MAX_LAST_PROMPT {
+            return Some((prompt.to_owned(), false));
+        }
+        let mut end = MAX_LAST_PROMPT;
+        while !prompt.is_char_boundary(end) {
+            end -= 1;
+        }
+        Some((prompt[..end].to_owned(), true))
+    })
+}
+
 /// Extract `ai-title` (last occurrence) and `last-prompt` from the tail bytes.
 fn extract_tail_meta(content: &str) -> (Option<String>, Option<String>) {
     let mut title: Option<String> = None;
```

`src/proto.rs` (the other hunks). Also, in `DAEMON_FEATURES` (added by Task 1), append `"last_prompt"`.

```diff
--- a/src/proto.rs
+++ b/src/proto.rs
@@ -123,6 +123,13 @@ pub enum Msg {
     Answers {
         id: SessionId,
     },
+    /// The newest prompt of the session's conversation, answered or not,
+    /// for saving as a snippet. Answered by `LastPromptReply`; an unknown
+    /// session by `Error "no such session: …"`. An older daemon answers
+    /// `Error "unsupported op…"`.
+    LastPrompt {
+        id: SessionId,
+    },
     /// Directories with claude history on this host, newest first (seeds the
     /// new-session directory picker).
     RecentProjects {
@@ -223,6 +230,14 @@ pub enum Msg {
         id: SessionId,
         answers: Vec<Answer>,
     },
+    /// Reply to `LastPrompt`: sanitized, at most 64 KiB (`cut` when it was
+    /// longer); `None` when the conversation has no prompt yet.
+    LastPromptReply {
+        id: SessionId,
+        prompt: Option<String>,
+        #[serde(default)]
+        cut: bool,
+    },
     Projects {
         dirs: Vec<ProjectDir>,
     },
```

`src/daemon/server.rs`:

```diff
--- a/src/daemon/server.rs
+++ b/src/daemon/server.rs
@@ -502,6 +505,16 @@ impl Client {
                 });
                 return;
             }
+            Msg::LastPrompt { id } => {
+                // A transcript read, like `Answers`.
+                let conversation = self.daemon.conversation(id);
+                let queue = self.queue.clone();
+                tokio::spawn(async move {
+                    let msg = last_prompt(id, conversation).await;
+                    let _ = queue.send(Frame::Control(Envelope { req, msg })).await;
+                });
+                return;
+            }
             Msg::RecentProjects { limit } => recent_projects(limit).await,
             Msg::SubscribeHostStats => {
                 // Spawn a task that watches the stats channel and pushes
@@ -706,17 +719,9 @@ async fn answers(id: SessionId, conversation: Option<(String, Option<String>)>)
             message: format!("no such session: {id}"),
         };
     };
-    // A claude session id is a UUID; anything else must not become a path.
-    let sid = sid.filter(|sid| {
-        let safe = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
-        !sid.is_empty() && sid.chars().all(safe)
-    });
     let read = tokio::task::spawn_blocking(move || {
-        let answers = match sid {
-            Some(sid) => {
-                let path = projects::transcript_path(&host::expand_tilde(&cwd), &sid);
-                projects::read_answers(&path)
-            }
+        let answers = match transcript(&cwd, sid) {
+            Some(path) => projects::read_answers(&path),
             None => Vec::new(),
         };
         proto::answers_reply(id, answers)
@@ -726,6 +731,36 @@ async fn answers(id: SessionId, conversation: Option<(String, Option<String>)>)
     })
 }
 
+/// The `LastPromptReply` for session `id`, whose `conversation` is
+/// [`Daemon::conversation`]'s, read on a blocking thread. No conversation
+/// yet, no prompt.
+async fn last_prompt(id: SessionId, conversation: Option<(String, Option<String>)>) -> Msg {
+    let Some((cwd, sid)) = conversation else {
+        return Msg::Error {
+            message: format!("no such session: {id}"),
+        };
+    };
+    let read = tokio::task::spawn_blocking(move || {
+        let found = transcript(&cwd, sid).and_then(|path| projects::read_last_prompt(&path));
+        let (prompt, cut) = match found {
+            Some((prompt, cut)) => (Some(prompt), cut),
+            None => (None, false),
+        };
+        Msg::LastPromptReply { id, prompt, cut }
+    });
+    read.await.unwrap_or_else(|e| Msg::Error {
+        message: format!("last prompt failed: {e}"),
+    })
+}
+
+/// The transcript of claude session `sid` run in `cwd`. A claude session id
+/// is a UUID; anything else must not become a path.
+fn transcript(cwd: &str, sid: Option<String>) -> Option<std::path::PathBuf> {
+    let safe = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
+    let sid = sid.filter(|sid| !sid.is_empty() && sid.chars().all(safe))?;
+    Some(projects::transcript_path(&host::expand_tilde(cwd), &sid))
+}
+
 async fn recent_projects(limit: u32) -> Msg {
     let limit = usize::try_from(limit).unwrap_or(usize::MAX);
     match tokio::task::spawn_blocking(move || projects::recent_project_dirs(limit)).await {
```

Run: `umask 022; cargo test --locked --bin claudio last_prompt` → `test result: ok. 5 passed; 0 failed; …`
Run: `umask 022; cargo test --locked --bin claudio snippets` → `test result: ok. 4 passed; 0 failed; …`
Run: `umask 022; cargo test --locked --bin claudio`
Expected: the previous total + 9 (scratch, on base + Task 8 + the Task 1 stand-in: `test result: ok. 909 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`; with Tasks 8, 9 and 10 merged: `919 passed`).
Run: `rustfmt --edition 2021 --check src/tui/snippets.rs src/daemon/server.rs` → no output. `cargo build --locked` → 0 warnings.

Step 3: commit
`/usr/bin/git add src/tui/snippets.rs && /usr/bin/git commit -m "feat(tui): last prompt op and snippets store" -- src/tui/snippets.rs src/tui/mod.rs src/claude/projects.rs src/proto.rs src/daemon/server.rs src/daemon/tests.rs`

Interfaces this task provides to later tasks:
- `Msg::LastPrompt { id: SessionId }`, `Msg::LastPromptReply { id, prompt: Option<String>, #[serde(default)] cut: bool }`; `Welcome.features` contains `"last_prompt"`. Client mapping (Task 11): `prompt: None` → notice `no prompt to save`; `cut` → `saved snippet <name> (cut at 64 KiB)`; `Error "no such session: …"` → a notice; `Error "unsupported op: last_prompt"` → the restart notice.
- `crate::claude::projects::{last_prompt(&[u8]) -> Option<(String, bool)>, read_last_prompt(&Path) -> Option<(String, bool)>, MAX_LAST_PROMPT}` (the interface's "reader of the tail like `read_answers`" is `read_last_prompt`).
- `crate::tui::snippets::{Snippet { name, text }, dir() -> PathBuf, load(&Path) -> io::Result<Vec<Snippet>>, save(&Path, &str) -> io::Result<String>, slug(&str) -> String, MAX_BYTES, MAX_FILES, MAX_SLUG}`: blocking file I/O for the effect runner in `tui/mod.rs` only (never `App`). Task 11 removes the `#[allow(dead_code)]` above `mod snippets;`. `load` returns sanitized text; the paste path (§5) sanitizes and caps again.




### Task 11: wiring diagram, search and snippets into the framework
Model: sonnet   Repo: <repo>   Files: src/tui/builtins.rs (new), src/tui/app.rs, src/tui/extensions/mod.rs, src/tui/interaction.rs, src/tui/leader.rs, src/tui/mod.rs, src/tui/sessions.rs, tests/manager.rs
Goal: the three built-in extensions work end to end (AC8, AC9, AC10), coded against interfaces.md.

Uses Tasks 8–10 as they are on the branch (f51437b): `tui::diagram::{too_complex, render, Unsupported}`, `Msg::Search`/`SearchReply`/`SearchHit`, `Msg::LastPrompt`/`LastPromptReply`, `tui::snippets::{Snippet, dir, load, save}`. The diffs below were replayed on those real modules. Step 2 also removes the two `#[allow(dead_code)] // Wired into … by Task 11, which removes this.` lines above `mod diagram;` and `mod snippets;` in `src/tui/mod.rs`.

What this task does:
- Diagram: `render_lens("mermaid", …)` → `render_diagram`: `diagram::too_complex` pre-check, then `diagram::render` on a scratch thread through `guarded` (budget, abandon cap); `Ok(rows)` → the block (`Laid { rows, baseline: 0 }`); `Unsupported`, a timeout or a panic → raw (the code block); `Busy` → raw and not cached.
- `ActionResult::Resume { host, cwd, conversation }` (built-ins only; protocol v1 never produces it). `apply_result` → `resume_conversation`: switch to the tab on that host with that conversation, else `App::spawn(host, cwd, Some(conversation), preselected_proxy())`. `App::preselected_proxy` is the wizard's rule (active session's proxy, else the startup override, else the config default), now shared with `open_wizard`.
- `tui/builtins.rs` (pure): `builtin_action` (replaces Task 6's placeholder in `leader.rs`). Search (`s`, input "search conversations"): `searching…` notice, `Msg::Search { query, limit: 50 }` to the active session's host (local when none) with `ReplyTo::Search { gen, host }`; the reply → a picker titled `N conversations · "query"`, labels `<age>  <~/cwd>  <title>`, detail the snippet; no hits → "no matches"; `search busy` → "search busy, try again"; an old daemon → the restart notice. Snippets `p`: needs a session; `Effect::LoadSnippets { gen }` → `tui/mod.rs` runs `snippets::load(&snippets::dir())` in `spawn_blocking` → `HostEvent::SnippetsLoaded` → `App::on_snippets` (picker "Snippets": name, first non-empty line, Enter pastes; none → `no snippets in <dir>`). `P`: needs a claude session; `Msg::LastPrompt { id }` with `ReplyTo::LastPrompt { gen, host }` → `Effect::SaveSnippet { text, cut }` → `snippets::save` in `spawn_blocking` → `HostEvent::SnippetSaved` → `saved snippet <name>` (`… (cut at 64 KiB)`), `no prompt to save`, or the restart notice.
- Manager tests: `test_reader_draws_a_mermaid_diagram` (AC8; asserts only what any correct engine draws: both labels, a box corner, no `flowchart LR` source), `test_search_finds_and_resumes_a_conversation` (AC9: the fake claude logs `<pwd> <args>`; the resumed launch runs in the hit's directory with `--resume <id>`), `test_snippets_paste_and_save_the_last_prompt` (AC10).

Step 1: write the failing tests
`````diff
diff --git a/src/tui/builtins.rs b/src/tui/builtins.rs
new file mode 100644
index 0000000..b736f88
--- /dev/null
+++ b/src/tui/builtins.rs
@@ -0,0 +1,186 @@
+#[cfg(test)]
+mod tests {
+    use super::*;
+    use crate::tui::test_support::{app_with, info};
+
+    fn hit(conversation: &str, cwd: &str) -> SearchHit {
+        SearchHit {
+            conversation: conversation.into(),
+            cwd: cwd.into(),
+            modified: 0,
+            title: "Fix the parser".into(),
+            snippet: "…the needle…".into(),
+            matches: 2,
+        }
+    }
+
+    fn requests(app: &mut App) -> Vec<(Msg, ReplyTo)> {
+        app.take_effects()
+            .into_iter()
+            .filter_map(|e| match e {
+                Effect::Request { msg, to, .. } => Some((msg, to)),
+                _ => None,
+            })
+            .collect()
+    }
+
+    #[test]
+    fn a_search_lists_its_hits_and_enter_resumes_or_switches() {
+        let mut app = app_with(&[info(Some(1), Some("open-conv"))]);
+        app.activate(0);
+        app.take_effects();
+        app.builtin_action("search", "search", Some("needle".into()));
+        let sent = requests(&mut app);
+        let (Msg::Search { query, limit }, ReplyTo::Search { gen, host }) = sent[0].clone() else {
+            panic!("no search: {sent:?}")
+        };
+        assert_eq!(
+            (query.as_str(), limit, host.as_str()),
+            ("needle", 50, "local")
+        );
+        let reply = Msg::SearchReply {
+            query: "needle".into(),
+            hits: vec![hit("new-conv", "/home/u/p"), hit("open-conv", "/srv/app")],
+            scanned: 2,
+            total: 2,
+            truncated: false,
+        };
+        app.on_reply(
+            ReplyTo::Search {
+                gen,
+                host: host.clone(),
+            },
+            Ok(reply),
+        );
+        let Some(Modal::Picker(picker)) = &app.modal else {
+            panic!("no list")
+        };
+        assert_eq!(picker.title, "2 conversations · \"needle\"");
+        assert!(
+            picker.items[0].label.ends_with("~/p  Fix the parser"),
+            "{}",
+            picker.items[0].label
+        );
+        // The open conversation: its tab.
+        let open = picker.items[1].result.clone();
+        app.modal = None;
+        let tabs = app.sessions.len();
+        app.apply_result(open);
+        assert_eq!(app.sessions.len(), tabs);
+        // Another: a new tab resuming it, in its directory.
+        app.apply_result(ActionResult::Resume {
+            host: "local".into(),
+            cwd: "/home/u/p".into(),
+            conversation: "new-conv".into(),
+        });
+        assert_eq!(app.sessions.len(), tabs + 1);
+        let spawned = app.take_effects().into_iter().find_map(|e| match e {
+            Effect::Request {
+                msg: Msg::Spawn(spec),
+                ..
+            } => Some(spec),
+            _ => None,
+        });
+        let spec = spawned.expect("a spawn");
+        assert_eq!(spec.cwd, "/home/u/p");
+        assert_eq!(spec.args, ["--resume", "new-conv"]);
+    }
+
+    #[test]
+    fn search_notices_say_what_went_wrong() {
+        let mut app = app_with(&[info(Some(1), None)]);
+        let mut ask = |app: &mut App, reply: io::Result<Msg>| {
+            app.builtin_action("search", "search", Some("x".into()));
+            let (_, to) = requests(app).remove(0);
+            app.on_reply(to, reply);
+            app.notice
+                .as_ref()
+                .map(|n| n.text.clone())
+                .unwrap_or_default()
+        };
+        let none = Msg::SearchReply {
+            query: "x".into(),
+            hits: vec![],
+            scanned: 0,
+            total: 0,
+            truncated: false,
+        };
+        assert_eq!(ask(&mut app, Ok(none)), "no matches");
+        let busy = io::Error::other("search busy");
+        assert_eq!(ask(&mut app, Err(busy)), "search busy, try again");
+        let old = io::Error::other("unsupported op: search");
+        assert_eq!(
+            ask(&mut app, Err(old)),
+            "daemon on local is older: run `claudio daemon restart`"
+        );
+    }
+
+    #[test]
+    fn snippets_list_and_paste_and_the_last_prompt_is_saved() {
+        let mut app = app_with(&[info(Some(1), None)]);
+        app.activate(0);
+        app.take_effects();
+        app.builtin_action("snippets", "insert", None);
+        let gen = app
+            .take_effects()
+            .into_iter()
+            .find_map(|e| match e {
+                Effect::LoadSnippets { gen } => Some(gen),
+                _ => None,
+            })
+            .expect("a load");
+        let snippets = vec![(
+            "review".to_owned(),
+            "\n  Review this.\nThen more.".to_owned(),
+        )];
+        app.on_snippets(gen, "/cfg/snippets", Ok(snippets));
+        let Some(Modal::Picker(picker)) = &app.modal else {
+            panic!("no list")
+        };
+        assert_eq!(picker.items[0].detail.as_deref(), Some("Review this."));
+        app.modal = None;
+        app.builtin_action("snippets", "insert", None);
+        let gen = app.modal_gen;
+        app.on_snippets(gen, "/cfg/snippets", Ok(vec![]));
+        assert_eq!(
+            app.notice.as_ref().unwrap().text,
+            "no snippets in /cfg/snippets"
+        );
+
+        app.builtin_action("snippets", "save", None);
+        let (msg, to) = requests(&mut app).remove(0);
+        assert!(matches!(msg, Msg::LastPrompt { .. }));
+        let id = app.sessions[0].id;
+        let reply = Msg::LastPromptReply {
+            id,
+            prompt: Some("Fix it".into()),
+            cut: true,
+        };
+        app.on_reply(to, Ok(reply));
+        let saves: Vec<(String, bool)> = app
+            .take_effects()
+            .into_iter()
+            .filter_map(|e| match e {
+                Effect::SaveSnippet { text, cut } => Some((text, cut)),
+                _ => None,
+            })
+            .collect();
+        assert_eq!(saves, [("Fix it".to_owned(), true)]);
+        app.on_snippet_saved(Ok("fix-it".into()), true);
+        assert_eq!(
+            app.notice.as_ref().unwrap().text,
+            "saved snippet fix-it (cut at 64 KiB)"
+        );
+        app.builtin_action("snippets", "save", None);
+        let (_, to) = requests(&mut app).remove(0);
+        app.on_reply(
+            to,
+            Ok(Msg::LastPromptReply {
+                id,
+                prompt: None,
+                cut: false,
+            }),
+        );
+        assert_eq!(app.notice.as_ref().unwrap().text, "no prompt to save");
+    }
+}
diff --git a/src/tui/mod.rs b/src/tui/mod.rs
index 6c32d66..1a11bba 100644
--- a/src/tui/mod.rs
+++ b/src/tui/mod.rs
@@ -1918,4 +1988,20 @@ mod tests {
         assert_eq!(keys, latex::Formula::raw("x^2"));
         assert_eq!(cache.entries.len(), 2);
     }
+
+    /// AC8: a Mermaid fence is drawn on the worker through the guard; an
+    /// unsupported or too large one stays its code block.
+    #[test]
+    fn mermaid_fences_are_drawn_through_the_guard() {
+        let running = Arc::new(AtomicUsize::new(0));
+        let drawn = render_lens("mermaid", "flowchart LR\n  alpha --> beta", &running);
+        let rows = drawn.0.block.expect("drawn").rows;
+        assert!(rows.iter().any(|r| r.contains("alpha")), "{rows:?}");
+        assert!(rows.iter().any(|r| r.contains("beta")), "{rows:?}");
+        for src in ["classDiagram\n  A <|-- B".to_owned(), "flowchart LR\n".repeat(300)] {
+            let (raw, keep) = render_lens("mermaid", &src, &running);
+            assert_eq!(raw, latex::Formula::raw(&src));
+            assert!(keep);
+        }
+    }
 }
diff --git a/tests/manager.rs b/tests/manager.rs
index c383db2..3e52138 100644
--- a/tests/manager.rs
+++ b/tests/manager.rs
@@ -2490,3 +2490,180 @@ fn test_external_extension_lens_leader_and_actions() {
     tui.send_keys(ALT_Q);
     tui.wait_exit(WAIT);
 }
+
+// ── Built-in extensions: diagram, search, snippets ───────────────────────────
+
+/// A fake claude for the built-in extension tests: logs `<pwd> <args>` per
+/// launch to `log`; unless resuming, writes `records` (JSON lines) as
+/// conversation `sid` under its directory's project; prints a banner, turns
+/// bracketed paste on, runs its SessionStart hook, then echoes lines.
+fn write_conversation_fake_claude(
+    h: &ManagerHarness,
+    sid: &str,
+    records: &[serde_json::Value],
+    log: &std::path::Path,
+) {
+    let cwd = fs::canonicalize(&h.dirs[0]).unwrap();
+    let project = h.home.join(".claude/projects").join(encode_cwd(&cwd));
+    let lines: Vec<String> = records.iter().map(|r| format!("'{r}'")).collect();
+    let script = format!(
+        r#"#!/bin/sh
+if [ "$1" = "--version" ]; then echo "claude 0.0.0-fake"; exit 0; fi
+if [ "$1" = "--help" ]; then exit 0; fi
+echo "$(pwd) $*" >> "{log}"
+case "$*" in
+  *--resume*) ;;
+  *) mkdir -p "{project}"; printf '%s\n' {lines} > "{project}/{sid}.jsonl" ;;
+esac
+echo "FAKE_CLAUDE_BANNER pid=$$"
+printf '\033[?2004h'
+HOOK=$(printf '%s' "$2" | sed -n 's/.*"command":"\([^"]*__hook SessionStart[^"]*\)".*/\1/p')
+printf '{{"session_id":"{sid}","source":"startup"}}' | eval "$HOOK"
+while IFS= read -r line; do printf 'GOT %s\n' "$line"; done
+"#,
+        log = log.display(),
+        project = project.display(),
+        lines = lines.join(" "),
+    );
+    fs::write(&h.fake_claude, script).unwrap();
+    fs::set_permissions(&h.fake_claude, std::os::unix::fs::PermissionsExt::from_mode(0o755))
+        .unwrap();
+}
+
+fn user_record(text: &str, cwd: &std::path::Path) -> serde_json::Value {
+    serde_json::json!({
+        "type": "user",
+        "cwd": cwd.display().to_string(),
+        "message": {"role": "user", "content": text},
+    })
+}
+
+fn assistant_record(text: &str, cwd: &std::path::Path) -> serde_json::Value {
+    serde_json::json!({
+        "type": "assistant",
+        "cwd": cwd.display().to_string(),
+        "message": {
+            "model": "claude-fake",
+            "content": [{"type": "text", "text": text}],
+            "usage": {"input_tokens": 1, "output_tokens": 1},
+        },
+    })
+}
+
+/// AC8: a ```mermaid answer is drawn with box characters in the reader;
+/// its source is not shown.
+#[test]
+fn test_reader_draws_a_mermaid_diagram() {
+    let h = ManagerHarness::new();
+    let cwd = fs::canonicalize(&h.dirs[0]).unwrap();
+    let answer = "Here it is:\n\n```mermaid\nflowchart LR\n  alpha --> beta\n```\n";
+    let records = [user_record("Draw it", &cwd), assistant_record(answer, &cwd)];
+    let log = h.root.join("launches.log");
+    write_conversation_fake_claude(&h, "dddddddd-eeee-4fff-8000-111111111111", &records, &log);
+    let mut tui = h.start_tui();
+    wizard_pick_dir(&mut tui, &h.dirs[0]);
+
+    tui.send_keys(ALT_U);
+    tui.wait_for("answer 1/1 · Draw it", Region::Pane, DAEMON_WAIT);
+    tui.wait_for("alpha", Region::Pane, WAIT);
+    let pane = tui.screen_text(Region::Pane);
+    assert!(pane.contains("beta"), "{pane}");
+    assert!(pane.contains('┌') || pane.contains('╭'), "boxes:\n{pane}");
+    assert!(!pane.contains("flowchart LR"), "the source is not shown:\n{pane}");
+    tui.send_keys(ESC);
+    tui.wait_until(|s| !s.contains("answer 1/1", Region::Pane), WAIT);
+    tui.send_keys(ALT_Q);
+    tui.wait_exit(WAIT);
+}
+
+/// AC9: Alt+z s finds a conversation of another directory by a word in its
+/// prompt, and Enter resumes it there in a new tab (`--resume <id>` reaches
+/// claude).
+#[test]
+fn test_search_finds_and_resumes_a_conversation() {
+    let h = ManagerHarness::new();
+    let here = fs::canonicalize(&h.dirs[0]).unwrap();
+    let there = fs::canonicalize(&h.dirs[1]).unwrap();
+    let log = h.root.join("launches.log");
+    let records = [user_record("Say hi", &here), assistant_record("Hi.", &here)];
+    write_conversation_fake_claude(&h, "eeeeeeee-ffff-4000-8111-222222222222", &records, &log);
+    // Another conversation, in the other directory.
+    let other = "ffffffff-0000-4111-8222-333333333333";
+    let project = h.home.join(".claude/projects").join(encode_cwd(&there));
+    fs::create_dir_all(&project).unwrap();
+    let lines = [
+        user_record("Where is the needle in this haystack?", &there),
+        assistant_record("Under the hay.", &there),
+    ];
+    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
+    fs::write(project.join(format!("{other}.jsonl")), text).unwrap();
+
+    let mut tui = h.start_tui();
+    wizard_pick_dir(&mut tui, &h.dirs[0]);
+    tui.send_keys(ALT_Z);
+    tui.wait_for("search conversations", Region::Screen, WAIT);
+    tui.send_keys(b"s");
+    tui.wait_for("(Enter run · Esc cancel)", Region::Screen, WAIT);
+    tui.send_keys(b"needle");
+    tui.send_keys(ENTER);
+    tui.wait_for("1 conversations · \"needle\"", Region::Screen, DAEMON_WAIT);
+    tui.send_keys(ENTER);
+    // The new launch runs in the other directory, resuming that conversation.
+    let resumed = |log: &str| {
+        log.lines().any(|l| {
+            l.starts_with(&format!("{} ", there.display()))
+                && l.ends_with(&format!("--resume {other}"))
+        })
+    };
+    let deadline = Instant::now() + DAEMON_WAIT;
+    while !resumed(&fs::read_to_string(&log).unwrap_or_default()) {
+        assert!(Instant::now() < deadline, "no resume of {other} in {}", there.display());
+        thread::sleep(Duration::from_millis(100));
+    }
+    tui.send_keys(ALT_Q);
+    tui.wait_exit(WAIT);
+}
+
+/// AC10: Alt+z p lists the snippets, the filter picks one, Enter pastes it
+/// into claude without Enter; Alt+z P saves the last prompt as a snippet.
+#[test]
+fn test_snippets_paste_and_save_the_last_prompt() {
+    let h = ManagerHarness::new();
+    let here = fs::canonicalize(&h.dirs[0]).unwrap();
+    let snippets = h.config_home.join("claudio/snippets");
+    fs::create_dir_all(&snippets).unwrap();
+    fs::write(snippets.join("review.md"), "Review this change carefully.").unwrap();
+    fs::write(snippets.join("deploy.txt"), "Deploy to staging.").unwrap();
+    let log = h.root.join("launches.log");
+    let records = [
+        user_record("Say hello to the world", &here),
+        assistant_record("Hello, world.", &here),
+    ];
+    write_conversation_fake_claude(&h, "00000000-1111-4222-8333-444444444444", &records, &log);
+    let mut tui = h.start_tui();
+    wizard_pick_dir(&mut tui, &h.dirs[0]);
+
+    tui.send_keys(ALT_Z);
+    tui.wait_for("insert snippet", Region::Screen, WAIT);
+    tui.send_keys(b"p");
+    tui.wait_for("Snippets", Region::Screen, WAIT);
+    tui.wait_for("Deploy to staging.", Region::Screen, WAIT);
+    tui.send_keys(b"rev");
+    tui.wait_until(|s| !s.contains("Deploy to staging.", Region::Screen), WAIT);
+    tui.send_keys(ENTER);
+    tui.wait_for("Review this change carefully.", Region::Pane, WAIT);
+    thread::sleep(Duration::from_millis(300));
+    assert!(!tui.screen_text(Region::Pane).contains("GOT"), "no Enter after the paste");
+
+    tui.send_keys(ALT_Z);
+    tui.wait_for("save last prompt as snippet", Region::Screen, WAIT);
+    tui.send_keys(b"P");
+    tui.wait_for("saved snippet ", Region::StatusBar, DAEMON_WAIT);
+    let saved: Vec<String> = fs::read_dir(&snippets)
+        .unwrap()
+        .map(|e| fs::read_to_string(e.unwrap().path()).unwrap())
+        .collect();
+    assert!(saved.iter().any(|t| t.contains("Say hello to the world")), "{saved:?}");
+    tui.send_keys(ALT_Q);
+    tui.wait_exit(WAIT);
+}
`````
Run: `umask 022; cargo test --locked --bin claudio 2>&1 | grep -E 'FAILED|^test result'`
Expected: `test tui::tests::mermaid_fences_are_drawn_through_the_guard ... FAILED` and `test result: FAILED. 972 passed; 1 failed` (the `builtins.rs` tests compile once Step 2 declares the module)

Step 2: implement
`````diff
diff --git a/src/tui/app.rs b/src/tui/app.rs
index 0bb5e05..81bfdb4 100644
--- a/src/tui/app.rs
+++ b/src/tui/app.rs
@@ -161,6 +161,12 @@ pub enum Effect {
         seq: u64,
         items: Vec<crate::proto::LensItem>,
     },
+    /// Read the snippets (off the UI thread); they go to
+    /// [`App::on_snippets`] with `gen`.
+    LoadSnippets { gen: u64 },
+    /// Save `text` (the last prompt; `cut` at 64 KiB) as a snippet; the
+    /// outcome goes to [`App::on_snippet_saved`].
+    SaveSnippet { text: String, cut: bool },
     /// Run an external extension's action in the extension host; the
     /// result goes to [`App::on_action_result`] with `gen`.
     RunAction {
@@ -212,6 +218,10 @@ pub enum ReplyTo {
     Git { gen: u64, seq: u64 },
     /// `Answers` for the reader view opened under `gen` on session `id`.
     Answers { gen: u64, id: SessionId },
+    /// `Search` on `host`, for the search action started under `gen`.
+    Search { gen: u64, host: String },
+    /// `LastPrompt` on `host`, for the snippet save started under `gen`.
+    LastPrompt { gen: u64, host: String },
 }
 
 impl ReplyTo {
diff --git a/src/tui/builtins.rs b/src/tui/builtins.rs
index fcbca57..b736f88 100644
--- a/src/tui/builtins.rs
+++ b/src/tui/builtins.rs
@@ -1,3 +1,232 @@
+//! The built-in extensions' actions: search (`s`: find a conversation on
+//! the active session's host and resume it) and snippets (`p`: paste a
+//! saved prompt, `P`: save the last one). Pure, like `app.rs`: the daemon
+//! searches and reads the last prompt (`Msg::Search`, `Msg::LastPrompt`),
+//! and the snippet files are read and written by the event loop
+//! ([`Effect::LoadSnippets`], [`Effect::SaveSnippet`]).
+
+use std::io;
+
+use super::app::{App, Effect, ReplyTo};
+use super::extensions::{ActionResult, ListItem};
+use super::fmt::{abbreviate_home, fmt_age};
+use super::interaction::Modal;
+use super::leader::Picker;
+use crate::proto::{Msg, SearchHit, SessionKind};
+
+/// The most hits a search asks for.
+const SEARCH_LIMIT: u32 = 50;
+
+impl App {
+    /// A built-in extension's action (`input`: what its input line got).
+    pub(super) fn builtin_action(&mut self, ext: &str, action: &str, input: Option<String>) {
+        match (ext, action) {
+            ("search", "search") => self.start_search(input.unwrap_or_default()),
+            ("snippets", "insert") => self.load_snippets(),
+            ("snippets", "save") => self.save_last_prompt(),
+            _ => self.notify(format!("{ext}: {action} is not available")),
+        }
+    }
+
+    /// Search the conversations on the active session's host (local when
+    /// there is none) for `query`.
+    fn start_search(&mut self, query: String) {
+        if query.trim().is_empty() {
+            return;
+        }
+        let host = self.active_host();
+        self.modal_gen += 1;
+        let gen = self.modal_gen;
+        self.pending_action = Some(gen);
+        self.notify("searching…");
+        let msg = Msg::Search {
+            query,
+            limit: SEARCH_LIMIT,
+        };
+        self.request(
+            &host,
+            msg,
+            ReplyTo::Search {
+                gen,
+                host: host.clone(),
+            },
+        );
+    }
+
+    /// `host`'s answer to the search started under `gen`: a list of the
+    /// conversations (Enter resumes one), or a notice.
+    pub(super) fn on_search_reply(&mut self, gen: u64, host: &str, reply: io::Result<Msg>) {
+        if self.pending_action != Some(gen) {
+            return;
+        }
+        self.pending_action = None;
+        match reply {
+            Ok(Msg::SearchReply { query, hits, .. }) if hits.is_empty() => {
+                let _ = query;
+                self.notify("no matches")
+            }
+            Ok(Msg::SearchReply { query, hits, .. }) => {
+                let title = format!("{} conversations · \"{query}\"", hits.len());
+                let items = hits
+                    .into_iter()
+                    .map(|hit| self.search_item(host, hit))
+                    .collect();
+                if self.modal.is_none() {
+                    self.modal = Some(Modal::Picker(Box::new(Picker::new(title, items))));
+                }
+            }
+            Ok(_) => self.notify("unexpected reply from the daemon"),
+            Err(e) if e.to_string().contains("search busy") => {
+                self.notify("search busy, try again")
+            }
+            Err(e) => {
+                let notice = App::older_daemon_notice(host, &e);
+                self.notify(notice.unwrap_or_else(|| format!("search failed: {e}")));
+            }
+        }
+        self.redraw = true;
+    }
+
+    /// One hit as a list line: `<when>  <~/cwd>  <title>`, the snippet as
+    /// its detail, resuming it as its result.
+    fn search_item(&self, host: &str, hit: SearchHit) -> ListItem {
+        let home = if host == "local" {
+            self.home.as_str()
+        } else {
+            ""
+        };
+        let age = fmt_age(self.now.saturating_sub(hit.modified));
+        let cwd = abbreviate_home(&hit.cwd, home);
+        ListItem {
+            label: format!("{age}  {cwd}  {}", hit.title),
+            detail: Some(hit.snippet),
+            result: ActionResult::Resume {
+                host: host.to_owned(),
+                cwd: hit.cwd,
+                conversation: hit.conversation,
+            },
+        }
+    }
+
+    /// Show `conversation`: switch to the tab on `host` that has it, else
+    /// start one there resuming it, in `cwd`, with the proxy the wizard
+    /// would preselect.
+    pub(super) fn resume_conversation(&mut self, host: String, cwd: String, conversation: String) {
+        let open = self.sessions.iter().position(|v| {
+            v.host == host && v.claude_session_id.as_deref() == Some(conversation.as_str())
+        });
+        match open {
+            Some(i) => self.activate(i),
+            None => {
+                let proxy = self.preselected_proxy();
+                self.spawn(host, cwd, Some(conversation), proxy);
+            }
+        }
+    }
+
+    /// Alt+z p: read the snippets (off the UI thread), then list them.
+    fn load_snippets(&mut self) {
+        if self.active_view().is_none() {
+            return self.notify("no active session");
+        }
+        self.modal_gen += 1;
+        let gen = self.modal_gen;
+        self.pending_action = Some(gen);
+        self.emit(Effect::LoadSnippets { gen });
+    }
+
+    /// The snippets in `dir` (name, text), read for the action started
+    /// under `gen`: a list whose Enter pastes one, or a notice.
+    pub fn on_snippets(
+        &mut self,
+        gen: u64,
+        dir: &str,
+        result: Result<Vec<(String, String)>, String>,
+    ) {
+        if self.pending_action != Some(gen) {
+            return;
+        }
+        self.pending_action = None;
+        match result {
+            Err(e) => self.notify(format!("could not read snippets: {e}")),
+            Ok(snippets) if snippets.is_empty() => self.notify(format!("no snippets in {dir}")),
+            Ok(snippets) => {
+                let items = snippets
+                    .into_iter()
+                    .map(|(name, text)| ListItem {
+                        label: name,
+                        detail: text
+                            .lines()
+                            .map(str::trim)
+                            .find(|l| !l.is_empty())
+                            .map(str::to_owned),
+                        result: ActionResult::Paste { text },
+                    })
+                    .collect();
+                if self.modal.is_none() {
+                    let picker = Picker::new("Snippets".to_owned(), items);
+                    self.modal = Some(Modal::Picker(Box::new(picker)));
+                }
+            }
+        }
+        self.redraw = true;
+    }
+
+    /// Alt+z P: ask the active session's daemon for its last prompt.
+    fn save_last_prompt(&mut self) {
+        let Some(v) = self.active_view() else {
+            return self.notify("no active session");
+        };
+        if v.kind != SessionKind::Claude {
+            return self.notify("no prompt to save");
+        }
+        let (id, host) = (v.id, v.host.clone());
+        self.modal_gen += 1;
+        let gen = self.modal_gen;
+        self.pending_action = Some(gen);
+        let to = ReplyTo::LastPrompt {
+            gen,
+            host: host.clone(),
+        };
+        self.request(&host, Msg::LastPrompt { id }, to);
+    }
+
+    /// `host`'s answer to the last-prompt request started under `gen`:
+    /// saved as a snippet by the event loop, or a notice.
+    pub(super) fn on_last_prompt_reply(&mut self, gen: u64, host: &str, reply: io::Result<Msg>) {
+        if self.pending_action != Some(gen) {
+            return;
+        }
+        self.pending_action = None;
+        match reply {
+            Ok(Msg::LastPromptReply {
+                prompt: Some(text),
+                cut,
+                ..
+            }) => self.emit(Effect::SaveSnippet { text, cut }),
+            Ok(Msg::LastPromptReply { prompt: None, .. }) => self.notify("no prompt to save"),
+            Ok(_) => self.notify("unexpected reply from the daemon"),
+            Err(e) => {
+                let notice = App::older_daemon_notice(host, &e);
+                self.notify(
+                    notice.unwrap_or_else(|| format!("could not read the last prompt: {e}")),
+                );
+            }
+        }
+        self.redraw = true;
+    }
+
+    /// The snippet was saved as `name` (or why not); `cut`: the prompt was
+    /// cut at 64 KiB.
+    pub fn on_snippet_saved(&mut self, result: Result<String, String>, cut: bool) {
+        match result {
+            Ok(name) if cut => self.notify(format!("saved snippet {name} (cut at 64 KiB)")),
+            Ok(name) => self.notify(format!("saved snippet {name}")),
+            Err(e) => self.notify(format!("could not save the snippet: {e}")),
+        }
+    }
+}
+
 #[cfg(test)]
 mod tests {
     use super::*;
diff --git a/src/tui/extensions/mod.rs b/src/tui/extensions/mod.rs
index 8dd01b6..b0fd9c7 100644
--- a/src/tui/extensions/mod.rs
+++ b/src/tui/extensions/mod.rs
@@ -57,6 +57,13 @@ pub enum ActionResult {
     Paste { text: String },
     /// A list to pick from; each item carries its own result (not a list).
     List { title: String, items: Vec<ListItem> },
+    /// Built-ins only (search; never from an external program): show
+    /// `conversation` on `host`, resuming it in `cwd` if no tab has it.
+    Resume {
+        host: String,
+        cwd: String,
+        conversation: String,
+    },
 }
 
 /// One entry of [`ActionResult::List`].
diff --git a/src/tui/interaction.rs b/src/tui/interaction.rs
index 8a86b61..dd2ca04 100644
--- a/src/tui/interaction.rs
+++ b/src/tui/interaction.rs
@@ -1057,6 +1057,10 @@ impl App {
             }
             (ReplyTo::Git { gen, seq }, reply) => self.on_git_reply(gen, seq, reply),
             (ReplyTo::Answers { gen, id }, reply) => self.on_answers_reply(gen, id, reply),
+            (ReplyTo::Search { gen, host }, reply) => self.on_search_reply(gen, &host, reply),
+            (ReplyTo::LastPrompt { gen, host }, reply) => {
+                self.on_last_prompt_reply(gen, &host, reply)
+            }
             (ReplyTo::Respawned(id), reply) => self.on_respawned(id, reply),
             (ReplyTo::SpawnedDeferred(id), Ok(_)) => {
                 // Re-attach the active view: its first Attach raced the Spawn.
diff --git a/src/tui/leader.rs b/src/tui/leader.rs
index c7879b8..d8eb4d5 100644
--- a/src/tui/leader.rs
+++ b/src/tui/leader.rs
@@ -314,11 +314,6 @@ impl App {
         });
     }
 
-    /// A built-in extension's action.
-    fn builtin_action(&mut self, ext: &str, action: &str, _input: Option<String>) {
-        self.notify(format!("{ext}: {action} is not available"));
-    }
-
     /// The result of the external action started under `gen` (or the
     /// notice saying why there is none). Dropped when another action has
     /// started since, or a modal is open.
@@ -346,6 +341,11 @@ impl App {
                 self.modal = Some(Modal::Picker(Box::new(Picker::new(title, items))));
             }
             ActionResult::Document { title, markdown } => self.open_document(title, markdown),
+            ActionResult::Resume {
+                host,
+                cwd,
+                conversation,
+            } => self.resume_conversation(host, cwd, conversation),
         }
         self.redraw = true;
     }
diff --git a/src/tui/mod.rs b/src/tui/mod.rs
index 0028592..1a11bba 100644
--- a/src/tui/mod.rs
+++ b/src/tui/mod.rs
@@ -5,10 +5,10 @@
 //! logic live in [`app`], rendering in [`ui`].
 
 pub mod app;
+mod builtins;
 mod claude_update;
 mod confirm;
 mod connections;
-#[allow(dead_code)] // Wired into the reader by Task 11, which removes this.
 mod diagram;
 mod extensions;
 mod fmt;
@@ -25,7 +25,6 @@ mod proxy_state;
 mod reader;
 mod selection;
 mod sessions;
-#[allow(dead_code)] // Wired into the snippets actions by Task 11, which removes this.
 mod snippets;
 mod state;
 mod stats_view;
@@ -531,6 +530,18 @@ enum HostEvent {
     },
     /// A notice from the extension host (an extension was disabled).
     ExtensionNotice(String),
+    /// The snippets (name, text) read for the action started under `gen`,
+    /// from `dir`.
+    SnippetsLoaded {
+        gen: u64,
+        dir: String,
+        result: Result<Vec<(String, String)>, String>,
+    },
+    /// A snippet was saved (its name), or why not; `cut` as asked.
+    SnippetSaved {
+        result: Result<String, String>,
+        cut: bool,
+    },
     /// An external action started under `gen` ended.
     ActionDone {
         gen: u64,
@@ -729,10 +740,34 @@ impl RenderCache {
 fn render_lens(lens: &str, src: &str, running: &Arc<AtomicUsize>) -> (latex::Formula, bool) {
     match lens {
         "latex" => render_formula(src, running),
+        "mermaid" => render_diagram(src, running),
         _ => (latex::Formula::raw(src), true),
     }
 }
 
+/// Draw one Mermaid diagram within [`RENDER_BUDGET`], as `render_formula`
+/// renders a formula: what the pre-check turns away, is unsupported, times
+/// out or panics stays its code block (raw); its rows are the block.
+fn render_diagram(src: &str, running: &Arc<AtomicUsize>) -> (latex::Formula, bool) {
+    if diagram::too_complex(src) {
+        return (latex::Formula::raw(src), true);
+    }
+    let owned = src.to_owned();
+    match guarded(move || diagram::render(&owned), running) {
+        Guarded::Done(Ok(rows)) => {
+            let block = Some(typeset::Laid { rows, baseline: 0 });
+            let rendered = latex::Rendered {
+                inline: None,
+                block,
+                text: None,
+            };
+            (latex::Formula::new(src, rendered), true)
+        }
+        Guarded::Done(Err(_)) | Guarded::Failed => (latex::Formula::raw(src), true),
+        Guarded::Busy => (latex::Formula::raw(src), false),
+    }
+}
+
 /// Render one formula within [`RENDER_BUDGET`]: what the pre-check turns
 /// away, or what times out or panics, is shown raw. The flag says whether
 /// the result may be cached: not when too many renders were still running
@@ -1142,6 +1177,10 @@ async fn event_loop(
                 }
                 Some(HostEvent::ExtensionNotice(text)) => app.notify(text),
                 Some(HostEvent::ActionDone { gen, result }) => app.on_action_result(gen, result),
+                Some(HostEvent::SnippetsLoaded { gen, dir, result }) => {
+                    app.on_snippets(gen, &dir, result);
+                }
+                Some(HostEvent::SnippetSaved { result, cut }) => app.on_snippet_saved(result, cut),
                 None => return Ok(Default::default()),
             },
             Some((to, reply)) = reply_rx.recv() => app.on_reply(to, reply),
@@ -1264,6 +1303,37 @@ fn run_effect(
                     .await;
             });
         }
+        Effect::LoadSnippets { gen } => {
+            let tx = ev_tx.clone();
+            tokio::spawn(async move {
+                let dir = snippets::dir();
+                let shown = dir.display().to_string();
+                let read = tokio::task::spawn_blocking(move || snippets::load(&dir)).await;
+                let result = match read {
+                    Ok(Ok(list)) => Ok(list.into_iter().map(|s| (s.name, s.text)).collect()),
+                    Ok(Err(e)) => Err(e.to_string()),
+                    Err(e) => Err(e.to_string()),
+                };
+                let event = HostEvent::SnippetsLoaded {
+                    gen,
+                    dir: shown,
+                    result,
+                };
+                let _ = tx.send(event).await;
+            });
+        }
+        Effect::SaveSnippet { text, cut } => {
+            let tx = ev_tx.clone();
+            tokio::spawn(async move {
+                let save = move || snippets::save(&snippets::dir(), &text);
+                let result = match tokio::task::spawn_blocking(save).await {
+                    Ok(Ok(name)) => Ok(name),
+                    Ok(Err(e)) => Err(e.to_string()),
+                    Err(e) => Err(e.to_string()),
+                };
+                let _ = tx.send(HostEvent::SnippetSaved { result, cut }).await;
+            });
+        }
         Effect::RunAction {
             gen,
             extension,
diff --git a/src/tui/sessions.rs b/src/tui/sessions.rs
index 892ab72..e5fdd9f 100644
--- a/src/tui/sessions.rs
+++ b/src/tui/sessions.rs
@@ -595,17 +595,23 @@ impl App {
         }
     }
 
-    pub(super) fn open_wizard(&mut self) {
+    /// The proxy the wizard preselects: the active session's, else the
+    /// startup override, else the config default.
+    pub(super) fn preselected_proxy(&self) -> Option<String> {
         let active_proxy = self.active_view().and_then(|v| v.proxy.clone());
+        active_proxy.or_else(|| {
+            let picked = self.proxy_override.pick(self.proxy_default.as_deref());
+            picked.map(str::to_owned)
+        })
+    }
+
+    pub(super) fn open_wizard(&mut self) {
         let active_host = self.active_host();
         // The wizard starts on the active host; picking another host seeds
         // it again (see `on_host_connected`).
         let seeds = wizard::assemble(None, &self.wizard_seeds(&active_host), &self.projects);
-        // Effective proxy default for the wizard:
-        // active session's proxy > startup override > config default.
-        let proxy_default = active_proxy
-            .as_deref()
-            .or_else(|| self.proxy_override.pick(self.proxy_default.as_deref()));
+        let proxy_default = self.preselected_proxy();
+        let proxy_default = proxy_default.as_deref();
         self.modal_gen += 1;
         let wizard = Wizard::new(
             seeds,
`````
Run: `umask 022; cargo test --locked --bin claudio`
Expected: `test result: ok. 976 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` and 0 warnings
Run: `umask 022; cargo test --locked --test manager -- --test-threads=1`
Expected: `test result: ok. 49 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`
Also: `rustfmt --edition 2021 --check src/tui/builtins.rs` prints nothing.

Step 3: commit
`/usr/bin/git add src/tui/builtins.rs && /usr/bin/git commit -m "feat(tui): wire diagram, search and snippets into the extensions" -- src/tui/builtins.rs src/tui/app.rs src/tui/extensions/mod.rs src/tui/interaction.rs src/tui/leader.rs src/tui/mod.rs src/tui/sessions.rs tests/manager.rs`

Interfaces this task provides to later tasks: `ActionResult::Resume`, `App::{builtin_action, resume_conversation, preselected_proxy, on_search_reply, on_snippets, on_last_prompt_reply, on_snippet_saved}`, `ReplyTo::{Search, LastPrompt}`, `Effect::{LoadSnippets, SaveSnippet}`, `HostEvent::{SnippetsLoaded, SnippetSaved}`, `render_diagram`.

### Task 12: docs and the example extension
Model: sonnet   Repo: <repo>   Files: README.md, CLAUDE.md, docs/manager-design.md, docs/extensions.md (new), examples/extensions/keys/extension.toml (new), examples/extensions/keys/keys.py (new), src/tui/extensions/manifest.rs
Goal: users and extension authors can use and write extensions; the example stays valid (AC12's documentation, the spec's "docs" deliverable).

Step 1: write the failing test (the example manifest must parse and be admitted)
`````diff
diff --git a/src/tui/extensions/manifest.rs b/src/tui/extensions/manifest.rs
index d5b04c8..14fe92c 100644
--- a/src/tui/extensions/manifest.rs
+++ b/src/tui/extensions/manifest.rs
@@ -597,4 +597,13 @@ input = "label"
         assert!(load(&dir.join("missing"), &mut notices).is_empty());
         std::fs::remove_dir_all(&dir).unwrap();
     }
+
+    #[test]
+    fn the_example_extension_is_valid() {
+        let text = include_str!("../../../examples/extensions/keys/extension.toml");
+        let ext = parse("keys", Path::new("/ext/keys"), text).unwrap();
+        assert_eq!(ext.lenses[0].name, "keys");
+        assert_eq!(ext.actions[0].key, 'k');
+        assert!(admit(vec![ext], &mut Vec::new()).len() == 1);
+    }
 }
`````
Run: `umask 022; cargo test --locked --bin claudio the_example_extension_is_valid 2>&1 | grep -E '^error' | head -1`
Expected: `` error: couldn't read `src/tui/extensions/../../../examples/extensions/keys/extension.toml`: No such file or directory (os error 2) ``

Step 2: the docs and the example
`````diff
diff --git a/CLAUDE.md b/CLAUDE.md
index c882bee..93a09e4 100644
--- a/CLAUDE.md
+++ b/CLAUDE.md
@@ -125,8 +125,11 @@ daemons coexist.
   `SessionEvent::Latex { seq, formulas }`: the whole list, only when it
   changed (`seq` + 1, for the session's whole life; a new conversation
   sends an empty list under the next `seq`). The last list is replayed to a
-  client right after its `Attached` + snapshot. `command()` adds the
-  `__context latex` SessionStart hook unless `[claude] latex = false`.
+  client right after its `Attached` + snapshot. The same refresh sends the
+  tag lenses' items as `SessionEvent::Lens` (own `seq`, replayed right
+  after the formulas, ≤ 256 KiB). `spawn` writes the session's extension
+  instructions to `<runtime dir>/context/<id>.txt` and `command()` adds the
+  `__context file <path>` SessionStart hook when there are any.
 - `git_status.rs` — `git status --porcelain=v2 --branch` for the status
   bar's `⎇` segment: 1 s deadline, capped output, same scrubbed env as
   `git.rs`.
@@ -197,6 +200,19 @@ is idle (checked every 2 min).
   (`ReaderView`: answer, scroll, Tab list, layout cache). Drawn by
   `ui/reader.rs`.
 - `formulas.rs` — the lines of the reader's formulas list (Tab).
+- `extensions/` — the extension registry (`mod.rs`: built-ins latex,
+  diagram, search, snippets; `Registry`, `ActionResult`), external
+  manifests (`manifest.rs`: `~/.config/claudio/extensions/<name>/`, the
+  HTML-tag rule, admission, at most 16), protocol v1 (`protocol.rs`: request
+  lines, result caps and sanitizing) and the extension host (`host.rs`: one
+  program per extension, lazy start, scrubbed env, own process group,
+  ≤ 16 in flight, render 1 s / action 15 s, 3 failures in 5 min disable it,
+  killed on exit; driven by `mod.rs`, never by `App`).
+- `leader.rs` — Alt+z: the leader popup, action input, list picker,
+  documents (a reader view) and paste (attached claude session with
+  bracketed paste on only, never Enter). `builtins.rs` — the built-in
+  actions: search (`Msg::Search`, resume or switch) and snippets
+  (`Effect::LoadSnippets`/`SaveSnippet`, `Msg::LastPrompt`).
 - `confirm.rs` — the generic confirm prompt (kill, reset, claude update).
 - `claude_update.rs` — local/remote claude version checks and updates.
 - `plain.rs` — `claudio --plain`: `Mode::Plain` is one local, ephemeral
@@ -265,11 +281,14 @@ is idle (checked every 2 min).
   wizard's resume list), and `recent_answers` (the reader's last 10 answers
   of a transcript tail: real prompts and the assistant text after them).
 - `hooks.rs` — `__hook <Event> <socket> <token>` relay: sends the hook payload
-  to the daemon socket. Also `__context latex`, the SessionStart hook that asks
-  claude to wrap math in `<latex>` tags (`[claude] latex`, on by default); it prints one
-  constant JSON line and never touches the socket.
-- `latex.rs` — `extract_latex` and `latex_tags` (the formulas of an
-  assistant text, code excluded; the latter with their byte ranges) and
+  to the daemon socket. Also `__context file <path>`, the SessionStart hook
+  that prints the session's context file (the extensions' instructions: no
+  symlinks, a regular file of the uid, 16 KiB, sanitized) as one JSON line,
+  and `__context latex` for sessions of older daemons; neither touches the
+  socket.
+- `latex.rs` — `lens_tags` (the items of any tag lenses in an assistant
+  text, code excluded, with their byte ranges), `extract_latex` and
+  `latex_tags` (its `<latex>` wrappers) and
   `sanitize`, the character filter every formula and answer goes through
   in the daemon and again in the client.
 - `state.rs` — session state tracker derived from hook events.
diff --git a/README.md b/README.md
index 31e6311..a15ab3f 100644
--- a/README.md
+++ b/README.md
@@ -204,6 +204,17 @@ In the pane, each tag is replaced by a one-line Unicode rendering in cyan, such
 
 `Alt+u` opens the reader view over the pane: claude's latest answer, rendered by claudio from the transcript, with its prompt on top. Markdown is laid out (headings, emphasis, lists, quotes, tables, code blocks) and every formula is typeset in 2D: a formula on its own line is a display formula, as tall as it needs; one inside a sentence grows its line by a row above and below at most (taller ones stay on one line). `←`/`→` (or `h`/`l`) go to the previous and next of the last 10 answers; `↑`/`↓`, `PgUp`/`PgDn`, `g`/`G` and the mouse wheel scroll; `Tab` switches to the session's last 64 formulas, each drawn in 2D above its LaTeX source; `Esc` or `Alt+u` closes. It works over SSH and in `--plain` too. A daemon older than the client shows "run `claudio daemon restart`" instead. In a terminal tab, `Alt+u` shadows readline's upcase-word.
 
+### Extensions (`Alt+z`)
+
+`Alt+z` opens a small popup of extension actions; the next key runs one, and `Esc` closes it. The built-in extensions:
+
+- **diagram**: claude is asked to draw diagrams as ```` ```mermaid ```` blocks (flowcharts and sequence diagrams), and the reader view (`Alt+u`) draws them with box characters. A diagram it cannot draw, or one wider than the view, stays as its code block.
+- **search** (`Alt+z s`): searches every claude conversation on the active session's host (prompts and answers, not tool output) and lists the matches, newest first. Enter resumes the chosen one in a new tab, or switches to the tab that has it.
+- **snippets** (`Alt+z p`): a filterable list of the prompts saved in `~/.config/claudio/snippets/` (`*.md`, `*.txt`); Enter pastes one into claude's prompt without sending it. `Alt+z P` saves the session's last prompt as a new snippet.
+- **latex**: the formulas above.
+
+`Alt+z u` opens the reader view too. You can write your own extensions in any language: a directory in `~/.config/claudio/extensions/` with a manifest and a program that claudio runs and talks to over JSON lines. See [docs/extensions.md](docs/extensions.md) and the example in [`examples/extensions/keys/`](examples/extensions/keys/). Each extension can be turned off under `[extensions]` in `config.toml`.
+
 ### Keeping claude up to date
 
 Once a day claudio compares your `claude --version` with the latest release on your update channel, and asks before running `claude update`. When an SSH host connects, its claude is compared with yours. If it is older, claudio offers to update it; if it is missing, claudio offers to install it with the official installer. Either way it asks first. Prompts offer `[y] update`, `[n] not now` and `[s] skip this version`. Running sessions keep their binary; new sessions use the updated one.
@@ -229,6 +240,7 @@ Manager keys use `Alt`, so they don't clash with Claude Code. In terminal tabs t
 | `Alt+s` | Proxy stats |
 | `Alt+s` → `n` | Move the session to the next proxy subscription (asks first) |
 | `Alt+u` | Reader view: the session's answers, formulas typeset (see [Formulas](#formulas-and-the-reader-view-altu)) |
+| `Alt+z` | Extension actions: `s` search, `p`/`P` snippets, `u` reader, and your own (see [Extensions](#extensions-altz)) |
 | `Alt+h` | Help |
 | `Alt+.` | In the wizard: show or hide dot-directories |
 | `Alt+q` | Quit the UI (sessions keep running) |
@@ -279,12 +291,16 @@ remote_check = "ask"            # SSH hosts' claude vs this machine's: "ask" | "
 latex = true                    # ask claude to write math in <latex> tags, rendered in the pane
                                 # (this host's sessions; `claudio daemon restart` after a change)
 
+[extensions]                    # every extension is on unless turned off here
+diagram = false                 # e.g. no Mermaid instruction or drawing
+
 [keys]                          # rebind any manager key
 new_session = "alt+n"
 terminal    = "alt+c"
 git_log     = "alt+l"
 reset       = "alt+e"
 formulas    = "alt+u"           # the reader view (its v0.4.0 name)
+extensions  = "alt+z"           # the extensions' leader key
 ```
 
 `CLAUDIO_NO_UPDATE_CHECK=1` disables both the claudio and the claude release checks.
diff --git a/docs/extensions.md b/docs/extensions.md
new file mode 100644
index 0000000..525ba80
--- /dev/null
+++ b/docs/extensions.md
@@ -0,0 +1,159 @@
+# Extensions
+
+An extension adds to what claudio shows and does. It has **lenses** (things
+claudio renders from claude's answers) and **actions** (keys under the leader,
+`Alt+z`). Four extensions are built in: `latex`, `diagram`, `search` and
+`snippets`. You can add your own as a small program in any language.
+
+- [Using extensions](#using-extensions)
+- [Writing an external extension](#writing-an-external-extension)
+- [Protocol v1](#protocol-v1)
+- [Limits and safety](#limits-and-safety)
+
+## Using extensions
+
+| Extension | Lens | Keys | What it does |
+|---|---|---|---|
+| `latex` | tag `<latex>` | — | Math typeset in the pane (one line) and the reader (2D). |
+| `diagram` | fence ```` ```mermaid ```` | — | Mermaid flowcharts and sequence diagrams drawn with box characters in the reader. |
+| `search` | — | `Alt+z s` | Search the claude conversations on the active session's host; Enter resumes one. |
+| `snippets` | — | `Alt+z p`, `Alt+z P` | Paste a saved prompt; save the last prompt as one. |
+
+`Alt+z` opens the leader popup: one line per action. Press the action's key to
+run it; `Esc` or any other key closes the popup and nothing reaches claude.
+`Alt+z u` opens the reader view (also `Alt+u`).
+
+Each lens comes with an instruction that tells claude how to write for it. It
+reaches claude through a `SessionStart` hook that reads a private file of the
+session daemon. The text never goes into claude's command line. Turn any
+extension off in `~/.config/claudio/config.toml`:
+
+```toml
+[extensions]
+diagram = false        # by name, built-in or external; every extension is on by default
+```
+
+`[claude] latex = false` still turns math off. On the client it turns the
+`latex` extension off; on a host it keeps the latex instruction out of that
+host's sessions.
+
+Snippets are files in `~/.config/claudio/snippets/` (`*.md` or `*.txt`, the
+file name without its extension is the snippet's name). A paste never presses
+Enter: the text lands in claude's prompt for you to edit or send. It needs
+claude's prompt to accept a bracketed paste; otherwise claudio says so and
+pastes nothing.
+
+## Writing an external extension
+
+An external extension is a directory under `~/.config/claudio/extensions/`
+(or `$XDG_CONFIG_HOME/claudio/extensions/`) with a manifest and a program:
+
+```
+~/.config/claudio/extensions/keys/
+├── extension.toml
+└── keys.py
+```
+
+```toml
+name = "keys"                      # must equal the directory name: ^[a-z][a-z0-9-]{0,31}$
+description = "keyboard keys"      # at most 120 characters
+command = ["python3", "keys.py"]   # argv, no shell; a first element with a '/' is
+                                   # relative to this directory, otherwise found in PATH
+
+[[lens]]
+tag = "keys"                       # or: fence = "abc"  (exactly one of tag and fence)
+instruction = "Write keyboard keys as <keys>Ctrl+C</keys>."
+
+[[action]]
+key = "k"                          # one character [A-Za-z0-9], unique among all actions
+title = "insert a timestamp"       # at most 60 characters
+id = "stamp"                       # ^[a-z][a-z0-9_-]{0,31}$, passed back to the program
+input = "label"                    # optional: ask for a line of input first
+```
+
+- A **tag lens** renders `<tag>…</tag>` in the pane (on one line, nothing moves)
+  and in the reader. A **fence lens** renders a fenced code block whose info
+  string starts with its name, in the reader only.
+- A tag that is an HTML element name (`kbd`, `code`, `sub`, …) is refused,
+  because claude and markdown use those as HTML. Add `html = true` to the lens
+  to take the name anyway.
+- An extension whose name, tag or fence is already taken is skipped. An action
+  whose key is taken is skipped on its own. At most 16 external extensions are
+  loaded. Each skip shows one notice at start.
+- Manifests are read when claudio starts. The program starts on the first
+  request and keeps running until claudio exits.
+
+[`examples/extensions/keys/`](../examples/extensions/keys/) is a complete
+extension in Python: a `keys` tag lens and one action.
+
+## Protocol v1
+
+claudio and the program exchange UTF-8 JSON objects, one per line, on the
+program's stdin and stdout. The program's stderr is discarded. Request ids are
+numbers chosen by claudio, and answers may come in any order.
+
+The program runs in its extension directory, in its own process group, with
+claudio's environment minus Claude Code's session variables,
+`ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN` and every `CLAUDIO_*` variable,
+plus `CLAUDIO_EXTENSION=<name>` and `CLAUDIO_EXTENSION_PROTOCOL=1`.
+
+### render
+
+```
+→ {"id":1,"method":"render","params":{"kind":"tag","lens":"keys","source":"Ctrl+C"}}
+← {"id":1,"result":{"inline":"⌃C","block":["┌────┐","│ ⌃C │","└────┘"]}}
+```
+
+`inline` is one line: it replaces the tag in the pane and in a sentence of the
+reader. `block` is a list of rows: the reader shows it for a tag alone in its
+paragraph and for a fence lens. Both are optional. A missing or failed
+rendering leaves the source as claude wrote it.
+
+### action
+
+```
+→ {"id":2,"method":"action","params":{"action":"stamp","input":"x",
+     "session":{"conversation":"<uuid>","cwd":"/home/u/p","host":"local","name":null}}}
+← {"id":2,"result":{"kind":"paste","text":"2026-11-05 16:40"}}
+```
+
+`session` is the active session, or `null` when there is none. `input` is the
+line the user typed, or `null` for an action without `input`. `result.kind` is
+one of:
+
+| kind | fields | claudio |
+|---|---|---|
+| `document` | `title`, `markdown` | shows the markdown like the reader, with its lenses rendered |
+| `notice` | `text` | shows it in the status bar |
+| `paste` | `text` | pastes it into the active claude session, without Enter |
+| `list` | `title`, `items: [{label, detail?, result}]` | lets the user pick; Enter applies the item's `result` (not a `list`) |
+
+### errors
+
+Any request may fail: `{"id":3,"error":"message"}`. A render error leaves the
+source as written; an action error is shown as a notice.
+
+## Limits and safety
+
+Everything a program returns is treated as untrusted:
+
+- Every string is stripped of control and format characters. Inline text,
+  labels, details, notices and titles also lose newlines and tabs.
+- Caps: inline ≤ 512 columns; block ≤ 200 columns × 100 rows; document
+  markdown ≤ 256 KiB; list ≤ 500 items, label ≤ 200 and detail ≤ 400
+  characters; paste ≤ 64 KiB; notice ≤ 200 and title ≤ 120 characters. A
+  rendering over a cap is dropped. Markdown or paste text over its cap is
+  dropped with the notice `result too large`. Lists keep their first 500
+  items, and long labels, details, notices and titles are cut.
+- Timeouts: a render has 1 s, an action 15 s. At most 16 requests are in
+  flight per extension; failed renders are not asked again for 60 s.
+- A line that is not JSON, or one over 1 MiB, is a protocol error: the
+  program is stopped and started again on the next request. After 3 failures
+  (crash, protocol error, timeout) within 5 minutes the extension is disabled
+  until claudio restarts, with a notice.
+- On exit claudio sends SIGTERM to the program's process group, then SIGKILL
+  after a second.
+
+External extensions run on the machine where you run `claudio`, never on an
+SSH host. They run as you and could read your `config.toml`, so trust them as
+you trust that file.
diff --git a/docs/manager-design.md b/docs/manager-design.md
index b6fa584..c9e697e 100644
--- a/docs/manager-design.md
+++ b/docs/manager-design.md
@@ -441,6 +441,48 @@ Spec: `docs/specs/2026-11-04-latex-render-design.md`.
   session or an older open are dropped (`modal_gen`). `Tab` shows the
   `Latex` list (the v0.4.0 popup's content).
 
+### 2.8 Extensions
+
+Spec: `docs/specs/2026-11-05-extensions-design.md`; author guide:
+`docs/extensions.md`.
+
+- **Registry** (`tui/extensions/`). Built-ins `latex` (tag lens),
+  `diagram` (fence lens `mermaid`), `search` (`s`) and `snippets`
+  (`p`, `P`), then external extensions from
+  `~/.config/claudio/extensions/<name>/extension.toml` (validated and admitted
+  by `manifest.rs`; at most 16). `[extensions] name = false` turns one off;
+  `[claude] latex = false` turns latex off.
+- **Context.** Every claude `Spawn`/`Respawn` carries `extensions` (the
+  enabled lenses and their instructions, capped). The daemon's effective
+  value: the request's, else the journal's, else the legacy latex lens (when
+  its `[claude] latex` is on); its `latex = false` drops the latex lens. The
+  instructions go to `<runtime dir>/context/<session>.txt` (0700/0600,
+  atomic, removed with the session, cleared at daemon start) and SessionStart
+  runs `__context file <path>` (no symlinks, regular files of the uid,
+  16 KiB, sanitized). `__context latex` still answers older sessions.
+- **Lens items.** The transcript cursor gathers the tag lenses' items
+  (`lens_tags`, latex's rules) and the daemon sends `SessionEvent::Lens`
+  (seq rules as `Latex`, replayed after it on attach, ≤ 256 KiB) beside
+  `Latex`. `Welcome.features` has `lens`; for a daemon without it, its
+  `Latex` events are taken as `latex` items.
+- **Rendering.** Built-in lenses render on the render worker through the
+  guard, cached by (lens, source). External lenses are rendered by the
+  extension host, never on the worker: the pane's items through
+  `Effect::RenderExternal`, the reader's through the worker's list of
+  external slots. The pane overlays every enabled tag lens; the reader turns
+  fence lenses into blocks, with the code block as the fallback.
+- **Host** (`tui/extensions/host.rs`, driven by `tui/mod.rs`). One program
+  per external extension, started lazily in its directory and process
+  group with a scrubbed env; JSON lines (protocol v1), ≤ 16 requests in
+  flight, render 1 s / action 15 s, results capped and sanitized
+  (`protocol.rs`); 3 failures in 5 minutes disable it; stopped on exit.
+- **Leader** (`Alt+z`, `tui/leader.rs`). The popup, an action's input line,
+  the list picker, documents (a reader view) and paste (only into an
+  attached claude session with bracketed paste on). Built-in actions are in
+  `tui/builtins.rs`: search (`Msg::Search` on the active host, Enter resumes
+  or switches) and snippets (files read and written by the event loop;
+  `Msg::LastPrompt`).
+
 ## 3. UX
 
 ### 3.1 Layout
diff --git a/examples/extensions/keys/extension.toml b/examples/extensions/keys/extension.toml
new file mode 100644
index 0000000..448b17f
--- /dev/null
+++ b/examples/extensions/keys/extension.toml
@@ -0,0 +1,15 @@
+# An example claudio extension: copy this directory to
+# ~/.config/claudio/extensions/keys/ and restart claudio.
+name = "keys"
+description = "keyboard keys drawn as keycaps"
+command = ["python3", "keys.py"]
+
+[[lens]]
+tag = "keys"
+instruction = "This terminal draws keyboard keys. Write a key or a key combination as <keys>Ctrl+C</keys>, one combination per tag."
+
+[[action]]
+key = "k"
+title = "insert a timestamp"
+id = "stamp"
+input = "label"
diff --git a/examples/extensions/keys/keys.py b/examples/extensions/keys/keys.py
new file mode 100644
index 0000000..fca779d
--- /dev/null
+++ b/examples/extensions/keys/keys.py
@@ -0,0 +1,52 @@
+"""An example claudio extension (protocol v1): the `keys` tag lens draws a
+key combination as keycaps, and the `stamp` action pastes a timestamp.
+
+claudio writes one JSON request per line on stdin and reads one JSON answer
+per line on stdout. See docs/extensions.md."""
+
+import json
+import sys
+import time
+
+SYMBOLS = {"ctrl": "⌃", "alt": "⌥", "shift": "⇧", "cmd": "⌘", "enter": "⏎", "esc": "⎋", "tab": "⇥"}
+
+
+def keycaps(source):
+    """`Ctrl+C` as `⌃C` (inline) and as a row of boxed keys (block)."""
+    keys = [k.strip() for k in source.split("+") if k.strip()]
+    names = [SYMBOLS.get(k.lower(), k.upper() if len(k) == 1 else k) for k in keys]
+    inline = "".join(names)
+    widths = [len(n) + 2 for n in names]
+    top = " ".join("┌" + "─" * w + "┐" for w in widths)
+    mid = " ".join("│ " + n + " │" for n in names)
+    bottom = " ".join("└" + "─" * w + "┘" for w in widths)
+    return {"inline": inline, "block": [top, mid, bottom]}
+
+
+def handle(request):
+    params = request.get("params", {})
+    if request.get("method") == "render":
+        return keycaps(params.get("source", ""))
+    if request.get("method") == "action" and params.get("action") == "stamp":
+        label = params.get("input") or "now"
+        stamp = time.strftime("%Y-%m-%d %H:%M")
+        return {"kind": "paste", "text": f"{label}: {stamp}"}
+    raise ValueError("unknown request")
+
+
+def main():
+    for line in sys.stdin:
+        try:
+            request = json.loads(line)
+        except ValueError:
+            continue
+        answer = {"id": request.get("id")}
+        try:
+            answer["result"] = handle(request)
+        except Exception as e:  # any failure is that request's error
+            answer["error"] = str(e)
+        print(json.dumps(answer, ensure_ascii=False), flush=True)
+
+
+if __name__ == "__main__":
+    main()
`````
Run: `umask 022; cargo test --locked --bin claudio`
Expected: `test result: ok. 977 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`
Run: `printf '%s\n' '{"id":1,"method":"render","params":{"kind":"tag","lens":"keys","source":"Ctrl+C"}}' | python3 -I examples/extensions/keys/keys.py`
Expected: `{"id": 1, "result": {"inline": "⌃C", "block": ["┌───┐ ┌───┐", "│ ⌃ │ │ C │", "└───┘ └───┘"]}}`
Run: a grep for personal paths and host names over `docs/extensions.md examples/ src/tui/extensions/ src/tui/leader.rs src/tui/builtins.rs` → nothing.

Step 3: commit
`/usr/bin/git add docs/extensions.md examples/extensions/keys/extension.toml examples/extensions/keys/keys.py && /usr/bin/git commit -m "docs: extensions, the leader key and the protocol" -- README.md CLAUDE.md docs/manager-design.md docs/extensions.md examples/extensions/keys/extension.toml examples/extensions/keys/keys.py src/tui/extensions/manifest.rs`

Interfaces this task provides to later tasks: none.

## Merge and release
<!-- Not copied into briefs; the controller executes this after the gate audit. -->
1. Full suites, on the tip of `feat/extensions`, each alone on the machine:
   - `umask 022; make test-unit` → `test result: ok. 977 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` with no `warning:` lines (about 18 s; on a known base flake, rerun once).
   - `umask 022; cargo test --locked --test manager -- --test-threads=1` → `test result: ok. 49 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`.
   - `make` → a clean release build with 0 warnings; `stat -c %s target/release/claudio` against v0.5.0's; report the growth (AC13).
   - `make static` → `statically linked`.
   - `rustfmt --edition 2021 --check` on every file listed in Global Constraints and every new `.rs` file → nothing.
2. AC12, by the controller, with the branch's `target/release/claudio`, before the gate audit, on an isolated daemon: `XDG_CONFIG_HOME` and `XDG_RUNTIME_DIR` under `~/.cache/claudio/ac12/` (never the user's daemon or sessions); `HOME` stays real so claude is logged in and writes its transcripts where the daemon reads them.
   - Copy `examples/extensions/keys/` to `~/.cache/claudio/ac12/config/claudio/extensions/keys/`; start the TUI: no extension notice at start.
   - New session in a scratch directory. Ask "draw the architecture of a web app with a load balancer, two servers and a database as a diagram": the answer has a ```` ```mermaid ```` block, and Alt+u draws it with boxes (or, if wider than the view, the code block and `diagram is N columns wide`).
   - Ask a math question: the pane overlays the formula, the reader typesets it (latex unchanged).
   - Ask "which key stops a running command?": with the keys extension, `<keys>Ctrl+C</keys>` shows as `⌃C` in the pane and the reader.
   - Alt+z s, a word of the first question, Enter: the list shows that conversation; Enter switches to its tab. From a second tab in another directory, a search for it and Enter: still switches. Quit claude in the first tab, search again, Enter: a new tab resumes it (`--resume`), with its history.
   - Put a file in `~/.cache/claudio/ac12/config/claudio/snippets/`; Alt+z p, filter, Enter: the text is in claude's prompt, not sent. Alt+z P: `saved snippet <name>`, and the file holds the last prompt.
   - `[extensions] diagram = false` in that config.toml, restart the TUI, new session: `cat` the session's context file under `~/.cache/claudio/ac12/run/claudio/context/`: no Mermaid instruction.
   - Kill every process started for this (the isolated daemon and its sessions: `XDG_RUNTIME_DIR=… XDG_CONFIG_HOME=… target/release/claudio daemon stop`), remove `~/.cache/claudio/ac12/`, and record the results in progress.md. A failure is a fix task, not a release note.
3. Fable gate audit of the branch; fixes re-reviewed.
4. Merge (pre-authorized by the owner, ruling R2): `/usr/bin/git checkout main && /usr/bin/git merge --ff-only feat/extensions` (`main` is 76cd625 and the branch is linear on top of it).
5. Release v0.6.0, as `git show --stat 76cd625` did for v0.5.0:
   - `version = "0.6.0"` in Cargo.toml, `cargo update --workspace --offline` (only claudio's own entry), `git diff --stat` shows `Cargo.lock | 2 +-` and `Cargo.toml | 2 +-`; `cargo build --locked` succeeds.
   - `/usr/bin/git commit -m "release v0.6.0" -- Cargo.toml Cargo.lock` (no trailer).
   - `/usr/bin/git tag v0.6.0` (the release workflow checks that the tag matches Cargo.toml).
6. Push (pre-authorized by the owner, ruling R2; still a deliberate step): `/usr/bin/git push origin main` and `/usr/bin/git push origin v0.6.0`.
7. Post-release checks: `gh run list --limit 5` → the `ci` run on `main` and the `release` run on `v0.6.0` succeed (`gh run watch <id>` until done); `gh release view v0.6.0` lists the assets of every target in `release.yml`; `claudio upgrade` from a v0.5.0 install reaches 0.6.0.

## Scratch verification record
- Where: `~/.cache/claudio/scratch/plan-ext-a`, a clone of <repo> at `feat/extensions`, rebased onto f51437b once part B landed (tags N0 = f51437b, N1…N7, N11, N12 = the task commits). Every task was rebuilt as two commits on top of the previous one (Step 1: tests only, Step 2: the rest), and every diff block here is the `git diff` between them; the Step 2 tree equals the task commit. Every Expected line is copied from those runs.
- Replay: the blocks were extracted from this file and applied with `git apply`, with each Step 3 commit command, onto a fresh clone at f51437b (`~/.cache/claudio/scratch/plan-ext-a-replay`): every block applied, nothing was left uncommitted, and the result is identical to the scratch tip (`git diff` empty).
- Unit totals: 920 (f51437b) → 933 (Task 1) → 939 (Task 2) → 950 (Task 3) → 955 (Task 4) → 966 (Task 5) → 972 (Task 6) → 972 (Task 7) → 976 (Task 11) → 977 (Task 12).
- Step 1 states fail as stated (compile errors where the tests name what the step adds; Tasks 6 and 11 fail at run time as stated).
- Flakiness: the first run of the Task 5 tree had one failing test in a loaded machine (the split verification ran builds back to back); the rerun passed (966). Before the rebase one unit run hit the known base flake `daemon::host::tests::probe_runs_claude_with_the_given_path`; every other run passed first time.
- Manager suite: `test result: ok. 49 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` on the Task 11 tree (run alone); Task 7's test alone: 1 passed in about 20 s. Before the rebase the same tests ran on stand-ins for Tasks 8–10 (49 passed); after it, Task 11's three manager tests and the full suite ran on the real diagram, search and snippets modules.
- Not verified here: macOS (CI builds it), the release workflow, real claude (AC12 is manual), an old v0.5.0 daemon against this client (covered by the `unsupported op` and `features` unit tests).

## Deviations from the spec or interfaces.md (each needs a ruling)
- D1 (AC1, "only permitted edits"): adding fields and an enum variant forces mechanical edits of existing tests beyond the plain key sets: struct literals gain `extensions: None` (`SpawnSpec`, `RespawnSpec`) and `features: vec![]` (`Welcome`) in daemon/proto/client/freshness/session tests; `tui/mod.rs`'s `AnswersJob` literal gains `tags`/`fences`/`external`; the cache test checks `(lens, source)` keys; `reader::doc::tests::check` matches `Block::Lens`. The tests of the replaced `latex: bool` parameter now check the context-file hook (`hooks::tests::a_context_file_adds_one_command_to_session_start_only` replaces `latex_adds_one_context_command_to_session_start_only`; `session::tests::command_line_shape`'s last block; the manager test `test_latex_formula_is_rendered_in_the_pane_and_the_reader_list` looks for `__context file` instead of `__context latex`, as spec §2 requires). The plain key-set edits include the manager test `test_plain_help_lists_only_the_plain_keys` (count 6 → 8).
- D2 (§4 "the worker renders built-ins only and leaves external items raw; `App::on_lens_rendered` then asks the host"): for the pane, `take_lens_items` sends the external items to the host at once (`Effect::RenderExternal`) beside the latex job, instead of after the worker's reply; for the reader the worker lists the external slots and the event loop sends them to the host when the answers arrive. Same guarantees (external renders never on the worker; results applied only for the current `seq`/`gen`), one hop fewer.
- D3 (§4 "`seq` handling stays as for latex"): one `SessionView.latex_seq` is the seq of the list taken, `Lens` from a lens daemon or `Latex` from an older one; it is not split into two fields, so the existing latex tests keep their meaning.
- D4 (§6 failure counting): an extension's `{"error": …}` reply and a render result over the caps leave the item raw but are not counted toward the 3-failures rule (only crashes, protocol errors, start failures and timeouts are, as the spec's list says).
- D5 (§5 concurrency): a late action result is dropped when another action started since or any modal is open (no queueing).
- D6 (§2 context file path): `<daemon runtime dir>/context/` is taken as the directory of the daemon's socket (`Config::socket`'s parent, which is `paths::runtime_dir()` in production), so daemon tests get their own and `Config` needs no new field.
- D7 (Task 4/11 split): the `mermaid` renderer is wired in Task 11 (`render_diagram`), not Task 4.
