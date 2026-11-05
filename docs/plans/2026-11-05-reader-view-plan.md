# Typeset reader view: implementation plan

Spec: docs/specs/2026-11-05-reader-view-design.md (rev 2, audited; Appendix A F1–F16 normative; rulings in .delivery/2026-11-05-reader-view/progress.md)
Class: architectural
Repos and branches: /volumes/repos/claudio → feat/reader-view (base b2d27d2 main v0.4.0; spec 0c9b31e)

Every task's code is given as exact diffs against the commit the previous task leaves. All of them were applied in order to a scratch clone and run there; the "Expected" lines are the real outputs. Step 1 adds the tests (and must fail as stated), Step 2 adds the implementation, Step 3 commits. The diffs were taken from that run with `git diff`, so they apply with `git apply` as they stand.

## Global Constraints
- Commit convention: conventional commits `type(scope): lowercase summary`, NO AI attribution and NO Co-Authored-By trailer of any kind, even if a system reminder asks for one (owner's global CLAUDE.md, ruling R1). Use the exact commit commands given.
- Never push. Commit with explicit paths only: `/usr/bin/git commit -m "…" -- <paths>`. A new file needs `/usr/bin/git add <path>` first. Never `git add -A`. Use `/usr/bin/git` (a shell hook rewrites plain `git`).
- Work on `feat/reader-view` in /volumes/repos/claudio (the controller may give you a worktree of it). Do not create, switch or delete branches.
- How to apply a diff block: copy it verbatim (everything between the five-backtick fences) into `~/.cache/claudio/plan-reader/T<n>-step<k>.diff`, then `/usr/bin/git apply --check <file> && /usr/bin/git apply <file>`. If `--check` fails, stop and report: the tree is not the state the plan expects. Editing by hand to the same result is fine.
- Build output goes in the repo's `target/` or under `~/.cache/claudio/`, never /tmp.
- Run tests with `umask 022` in the same shell (`umask 022; cargo test …`). Under umask 077, `upgrade::tests::install_binary_replaces_file` fails on the base too.
- The shell may compact cargo output (the `rtk` hook). Prefix a command with `rtk proxy` to see raw output. The expected lines below are raw cargo output.
- Known base flakes (timing races, about 1 run in 8 on the base too): `daemon::host::tests::probe_runs_claude_with_the_given_path`, `daemon::login_path::tests::capture_times_out_and_kills_the_shell`, `daemon::tests::freshness::an_outdated_idle_daemon_is_replaced_and_its_sessions_come_back`. If one of them, and nothing else, fails, run the suite once more. Report a second failure.
- No repo-wide `cargo fmt`: the tree is not rustfmt-clean. Files clean at the base stay clean: `src/config.rs`, `src/claude/hooks.rs`, `src/cli.rs`, `src/claude/latex.rs`, `src/tui/latex.rs`, `src/tui/formulas.rs`, `src/daemon/server.rs`, `src/tui/test_support.rs` (`rustfmt --edition 2021 --check <file>`). New files are rustfmt-clean: `src/tui/typeset/{mod,mathfont,walk}.rs`, `src/tui/reader/{mod,doc,layout}.rs`, `src/tui/ui/reader.rs`. Every other file keeps its formatting; match its style.
- Match the repo's comment density and naming: a doc comment on each item that says why, plain sentences, no banners beyond the existing `// ── Section ──` style.
- `app.rs` and the `App` methods in `interaction.rs`/`sessions.rs` stay pure: no I/O, no threads, no formula rendering, no markdown parsing. Threads and I/O live in `tui/mod.rs`. `latex::render` and the `typeset` engine run only from `render_formula` in `tui/mod.rs` (pre-check, scratch thread, budget), never from `App`, a draw, or a test with hostile input (unit tests call them on benign formulas only). Markdown is parsed on the `latex-render` worker.
- Protocol additions stay backward compatible: one new request op (`Answers`) and its reply (`AnswersReply`), no `PROTO` bump, no new field on an existing message. An old daemon answers `Error "unsupported op…"`.
- Manager (PTY) tests assert on the rendered screen model, never on raw bytes. Run them single-threaded (`-- --test-threads=1`) and with nothing else heavy running on the machine.
- No secrets and no answer or formula text in argv, logs or state files.
- Builds on Linux and macOS, and the static musl build (`make static`) keeps working: pure-Rust dependencies only.

## Review Focus
- src/proto.rs:680 `ANSWERS_REPLY_MAX` and :695 `answers_reply` (F1). The cap is on the serde_json encoding, oldest answers dropped first, so the reply always fits `MAX_FRAME`; an oversize frame would kill the client's writer task.
- src/claude/projects.rs:582 `recent_answers`, :632 `answer_prompt`, :665 `finish` (F2). Which `type:"user"` records start an answer (not `isMeta`/`isCompactSummary`/`isVisibleInTranscriptOnly`, not Claude Code's own `<command-…>`/`<local-command-…>`/`<system-reminder>` records), API-error records excluded, the compaction boundary, sidechains, caps cut on char boundaries, `sanitize` on both strings. :546 `read_answers`: the 4 MiB tail and its partial first line.
- src/daemon/server.rs:493 the `Answers` arm (replies from a spawned task, never inline) and :702 `answers`: unknown session → `Error "no such session: …"`, no conversation → empty, the claude session id filtered to `[A-Za-z0-9_-]` before it becomes a path, the read and the capping in `spawn_blocking`. src/daemon/mod.rs:467 `conversation`.
- src/tui/typeset/mod.rs:476 `lay`, :556 `seq`, :688 `attach`, :379 `hcat`, :367 `fits`: recursion only follows the parse tree; every box past 512 × 40 aborts the layout (`None`), so no part grows without bound. :91 `rewrite_roots` and :141 `environment` / :204 `split_rows`: linear scans of attacker text; the rewritten source is pre-checked again in `latex::render`.
- src/tui/latex.rs:145 `render` and :176 `capped`: `too_complex` on every rewritten part, caps 200 × 40 (blocks) and 512 columns (one line), `sanitize` on every row.
- src/tui/reader/doc.rs:93 `prepare` (sanitize first, placeholders after, combining mark after a tag dropped, 64 tags at most), :127 `parse` (pulldown-cmark options: tables, strikethrough, task lists only), :359 `text` (placeholders found by `chars`, text runs re-sanitized after), :390 `code_text` (a placeholder in code is its tag again), :292 `open_frame` (nesting capped at 8, deeper content flattened), :344 `flush` (the display-formula rule).
- src/tui/reader/layout.rs:160 `fits_inline` and :248 `inline` (an inline box is at most 3 rows; else the one-line form, else the dimmed source), :108 `safe` (a cluster wider than two columns is `�`, never a lost line), :390 `wrap`.
- src/tui/mod.rs:536 `spawn_latex_worker` and :568 `render_answers`: one job at a time, answers through the same `RenderCache` and guard; :1067 `Effect::RenderAnswers`, :969 `HostEvent::AnswersRendered`.
- src/tui/interaction.rs:215 `open_reader`, :227 `reader_mut`, :236 `on_answers_reply`, :258 `on_answers_rendered` (F12): a reply or a rendering for a closed view, another session or an older open (`modal_gen`) is dropped; an old daemon's refusal becomes the restart notice in the view.
- src/tui/ui/reader.rs:48 `draw_line`: every string sanitized again before it becomes a cell; lines cut at the pane's edge. src/tui/reader/mod.rs:204 `body`: the layout cache is for one width only.

## Task order and parallelism
| Task | Depends on | Repo/files | May run in parallel with |
|---|---|---|---|
| 1 | none | src/proto.rs, src/claude/projects.rs, src/daemon/mod.rs, src/daemon/server.rs, src/daemon/tests.rs | 2, 3, 4, 5, 6 |
| 2 | none | src/tui/mod.rs, src/tui/typeset/mod.rs (new), src/tui/typeset/mathfont.rs (new) | 1 |
| 3 | 2 | Cargo.toml, Cargo.lock, src/tui/latex.rs, src/tui/formulas.rs, src/tui/mod.rs, src/tui/typeset/mod.rs, src/tui/typeset/walk.rs (new), src/tui/ui/formulas.rs, src/tui/ui/mod.rs | 1 |
| 4 | 3 | Cargo.toml, Cargo.lock, src/claude/latex.rs, src/tui/mod.rs, src/tui/reader/mod.rs (new), src/tui/reader/doc.rs (new) | 1 |
| 5 | 3, 4 | src/tui/reader/mod.rs, src/tui/reader/layout.rs (new) | 1 |
| 6 | 5 | src/tui/reader/mod.rs | 1 |
| 7 | 1, 6 | src/cli.rs, src/tui/app.rs, src/tui/formulas.rs, src/tui/interaction.rs, src/tui/keymap.rs, src/tui/mod.rs, src/tui/reader/layout.rs, src/tui/ui/formulas.rs (deleted), src/tui/ui/mod.rs, src/tui/ui/reader.rs (new), tests/manager.rs | none |
| 8 | 7 | tests/manager.rs | 9 |
| 9 | 7 | README.md, docs/manager-design.md, CLAUDE.md | 8 |

Task 1 touches none of the files of Tasks 2–6, so it may run in a worktree of `feat/reader-view` beside any of them; 8 and 9 are disjoint too. The controller cherry-picks each pair's commits onto the branch in task order; the diffs apply in either order within a pair. Task 4 must follow Task 3 (both edit Cargo.toml/Cargo.lock around the same lines). The `N passed` figures do not depend on the order; the `M filtered out` figures and the unit totals assume strict task order: base 829; after Task 1: 837, 2: 856, 3: 859, 4: 867, 5: 875, 6: 881, 7: 886, 8: 886, 9: 886. Run without Task 1, Tasks 2–6 read 8 fewer in `filtered out`.

## Tasks

### Task 1: the `Answers` op: transcript answers, capped reply, daemon handler
Model: sonnet   Repo: /volumes/repos/claudio   Files: src/proto.rs, src/claude/projects.rs, src/daemon/mod.rs, src/daemon/server.rs, src/daemon/tests.rs
Goal: AC1 (protocol part), AC2, AC3, with F1 (encoded-size cap) and F2 (what is a prompt). `Msg::Answers { id }` is answered by `Msg::AnswersReply { id, answers }`, `Answer { prompt, text }` oldest first. The daemon finds the session's cwd and claude session id in its journal, reads the transcript's last 4 MiB in `spawn_blocking`, keeps the last 10 answers (prompt ≤ 2 KiB, text ≤ 64 KiB, cut on a char boundary with ` …`, both sanitized), and caps the encoded reply at 512 KiB by dropping the oldest. It replies from a spawned task, like `UpdateClaude`. `is_real_prompt` becomes `pub(crate)`.

Step 1: write the failing tests (proto round trip and old-daemon `Unknown`, the frame cap with ten 64 KiB answers of backslashes, the grouping and caps of `recent_answers`, the tail read, and a daemon test for the empty and unknown-session replies).
`````diff
diff --git a/src/claude/projects.rs b/src/claude/projects.rs
index 7bc95cd..4a014e6 100644
--- a/src/claude/projects.rs
+++ b/src/claude/projects.rs
@@ -1109,4 +1284,198 @@ mod tests {
             "live cwd must appear in results; got: {paths:?}"
         );
     }
+
+    // ── recent_answers ────────────────────────────────────────────────────────
+
+    /// A transcript of `records`, one JSON line each.
+    fn jsonl(records: &[serde_json::Value]) -> Vec<u8> {
+        let mut out = Vec::new();
+        for record in records {
+            out.extend_from_slice(record.to_string().as_bytes());
+            out.push(b'\n');
+        }
+        out
+    }
+
+    fn prompt_record(content: serde_json::Value) -> serde_json::Value {
+        serde_json::json!({"type": "user", "message": {"role": "user", "content": content}})
+    }
+
+    fn answer_record(content: serde_json::Value) -> serde_json::Value {
+        serde_json::json!({"type": "assistant", "message": {"role": "assistant", "content": content}})
+    }
+
+    fn answer(prompt: &str, text: &str) -> Answer {
+        Answer {
+            prompt: prompt.into(),
+            text: text.into(),
+        }
+    }
+
+    #[test]
+    fn answers_are_a_prompt_and_the_text_blocks_after_it() {
+        use serde_json::json;
+        let transcript = jsonl(&[
+            prompt_record(json!("first question")),
+            answer_record(json!([
+                {"type": "thinking", "thinking": "hmm"},
+                {"type": "text", "text": "Part one."},
+                {"type": "tool_use", "id": "t1", "name": "Read", "input": {}},
+            ])),
+            // A tool result does not end the answer.
+            prompt_record(json!([{"type": "tool_result", "tool_use_id": "t1", "content": "file"}])),
+            answer_record(json!([{"type": "text", "text": "Part two: <latex>x^2</latex>"}])),
+            prompt_record(
+                json!([{"type": "text", "text": "second"}, {"type": "text", "text": "question"}]),
+            ),
+            answer_record(json!("A string answer.")),
+        ]);
+        assert_eq!(
+            recent_answers(&transcript),
+            [
+                answer(
+                    "first question",
+                    "Part one.\n\nPart two: <latex>x^2</latex>"
+                ),
+                answer("second\nquestion", "A string answer."),
+            ]
+        );
+    }
+
+    #[test]
+    fn answers_skip_claude_codes_own_records() {
+        use serde_json::json;
+        let mut meta = prompt_record(json!("caveat text"));
+        meta["isMeta"] = json!(true);
+        let mut summary = prompt_record(json!(
+            "This session is being continued from a previous conversation"
+        ));
+        summary["isCompactSummary"] = json!(true);
+        let mut visible = prompt_record(json!("only in the transcript"));
+        visible["isVisibleInTranscriptOnly"] = json!(true);
+        let mut api_error =
+            answer_record(json!([{"type": "text", "text": "API Error: stream error"}]));
+        api_error["isApiErrorMessage"] = json!(true);
+        let mut side_prompt = prompt_record(json!("a subagent's task"));
+        side_prompt["isSidechain"] = json!(true);
+        let mut side_text = answer_record(json!("a subagent's answer"));
+        side_text["isSidechain"] = json!(true);
+        let transcript = jsonl(&[
+            prompt_record(json!("the question")),
+            meta,
+            prompt_record(json!(
+                "<command-name>/model</command-name>\n<command-args></command-args>"
+            )),
+            prompt_record(json!("<command-message>model</command-message>")),
+            prompt_record(json!(
+                "<local-command-stdout>Set model</local-command-stdout>"
+            )),
+            prompt_record(json!("<local-command-caveat>Caveat</local-command-caveat>")),
+            prompt_record(json!("  <system-reminder>remember</system-reminder>")),
+            visible,
+            side_prompt,
+            side_text,
+            api_error,
+            answer_record(json!([{"type": "text", "text": "The answer."}])),
+            // A compaction ends the answer; what follows has no prompt.
+            json!({"type": "system", "subtype": "compact_boundary"}),
+            summary,
+            answer_record(json!("after the summary")),
+            // A prompt that got no text is no answer.
+            prompt_record(json!("unanswered")),
+            answer_record(json!([{"type": "tool_use", "id": "t", "name": "Bash", "input": {}}])),
+        ]);
+        assert_eq!(
+            recent_answers(&transcript),
+            [answer("the question", "The answer.")]
+        );
+    }
+
+    #[test]
+    fn answers_are_the_last_ten_capped_and_sanitized() {
+        use serde_json::json;
+        let mut records = Vec::new();
+        for i in 0..12 {
+            records.push(prompt_record(json!(format!("q{i}"))));
+            records.push(answer_record(json!(format!("a{i}"))));
+        }
+        let answers = recent_answers(&jsonl(&records));
+        assert_eq!(answers.len(), MAX_ANSWERS);
+        assert_eq!(answers[0], answer("q2", "a2"));
+        assert_eq!(answers[9], answer("q11", "a11"));
+
+        // A long prompt and a long text are cut on a char boundary.
+        let prompt = "p".repeat(MAX_PROMPT_BYTES + 100);
+        let text = format!("{}é{}", "t".repeat(MAX_ANSWER_BYTES - 1), "t".repeat(10));
+        let answers = recent_answers(&jsonl(&[
+            prompt_record(json!(prompt)),
+            answer_record(json!(text)),
+        ]));
+        assert_eq!(
+            answers[0].prompt,
+            format!("{} …", "p".repeat(MAX_PROMPT_BYTES))
+        );
+        assert_eq!(
+            answers[0].text,
+            format!("{} …", "t".repeat(MAX_ANSWER_BYTES - 1))
+        );
+
+        // Escapes, bidi controls and private-use characters never leave.
+        let dirty = jsonl(&[
+            prompt_record(json!("why\u{1b}[2J\u{202e}?")),
+            answer_record(json!("because\u{e000}\u{9b}31m\u{200b}.")),
+        ]);
+        assert_eq!(recent_answers(&dirty), [answer("why[2J?", "because31m.")]);
+    }
+
+    #[test]
+    fn answers_skip_lines_cut_by_a_tail_read() {
+        use serde_json::json;
+        let transcript = jsonl(&[
+            prompt_record(json!("old question")),
+            answer_record(json!("old answer")),
+            prompt_record(json!("new question")),
+            answer_record(json!("new answer")),
+        ]);
+        // Cut inside the first line, and inside a multi-byte character of
+        // the last one.
+        let last = answer_record(json!("cut ééé")).to_string();
+        let mut bytes = transcript[10..].to_vec();
+        bytes.extend_from_slice(&last.as_bytes()[..last.find('é').unwrap() + 1]);
+        assert_eq!(
+            recent_answers(&bytes),
+            [answer("new question", "new answer")]
+        );
+        assert!(recent_answers(b"").is_empty());
+        assert!(recent_answers(b"not json\n{\"type\":\"user\"").is_empty());
+    }
+
+    #[test]
+    fn answers_are_read_from_the_tail_of_a_long_transcript() {
+        use serde_json::json;
+        let dir = std::env::temp_dir().join(format!("claudio-answers-{}", uuid::Uuid::new_v4()));
+        std::fs::create_dir_all(&dir).unwrap();
+        let path = dir.join("t.jsonl");
+        // An old answer whose text alone is longer than the tail read: its
+        // prompt is not read, and the rest of its line is skipped.
+        let mut transcript = jsonl(&[
+            prompt_record(json!("old")),
+            answer_record(json!("x".repeat(ANSWERS_TAIL as usize))),
+        ]);
+        transcript.extend(jsonl(&[
+            prompt_record(json!("new")),
+            answer_record(json!("fresh")),
+        ]));
+        std::fs::write(&path, &transcript).unwrap();
+        assert_eq!(read_answers(&path), [answer("new", "fresh")]);
+        // A short transcript is read whole.
+        std::fs::write(
+            &path,
+            jsonl(&[prompt_record(json!("q")), answer_record(json!("a"))]),
+        )
+        .unwrap();
+        assert_eq!(read_answers(&path), [answer("q", "a")]);
+        assert!(read_answers(&dir.join("missing.jsonl")).is_empty());
+        let _ = std::fs::remove_dir_all(&dir);
+    }
 }
diff --git a/src/daemon/tests.rs b/src/daemon/tests.rs
index 63ad296..0598b57 100644
--- a/src/daemon/tests.rs
+++ b/src/daemon/tests.rs
@@ -547,6 +547,38 @@ async fn hooks_update_state_and_journal() {
     assert_eq!(c.sessions().await[0].state, SessionState::NeedsInput);
 }
 
+/// `Answers` is served off the client loop: no conversation yet (or no
+/// transcript) is an empty list, an unknown session an error.
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn answers_are_empty_without_a_transcript_and_refused_for_unknown_sessions() {
+    let d = TestDaemon::start().await;
+    let mut c = d.client().await;
+    let id = Uuid::new_v4();
+    c.spawn(d.spec(id)).await;
+    let empty = Msg::AnswersReply {
+        id,
+        answers: Vec::new(),
+    };
+    assert_eq!(c.call(Msg::Answers { id }).await, empty);
+    // A conversation whose transcript does not exist (the test's work dir
+    // has none) has no answers either.
+    let payload = serde_json::json!({
+        "session_id": "0a1b2c3d-0000-4000-8000-000000000000",
+        "source": "startup",
+    });
+    send_hook(&d.config.socket, &d.token().await, "SessionStart", payload).await;
+    c.event(id, |e| {
+        matches!(e, SessionEvent::ClaudeSession { .. }).then_some(())
+    })
+    .await;
+    assert_eq!(c.call(Msg::Answers { id }).await, empty);
+    let unknown = Uuid::new_v4();
+    match c.call(Msg::Answers { id: unknown }).await {
+        Msg::Error { message } => assert!(message.contains("no such session"), "{message}"),
+        other => panic!("expected an error, got {other:?}"),
+    }
+}
+
 #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
 async fn kill_removes_session_everywhere() {
     let d = TestDaemon::start().await;
diff --git a/src/proto.rs b/src/proto.rs
index c6e072e..63b9721 100644
--- a/src/proto.rs
+++ b/src/proto.rs
@@ -1097,6 +1138,66 @@ mod tests {
         assert_eq!(back, SessionEvent::ClaudeSessionCleared);
     }
 
+    #[test]
+    fn answers_round_trip_and_are_unknown_to_older_daemons() {
+        #[derive(Debug, PartialEq, Deserialize)]
+        #[serde(tag = "op", rename_all = "snake_case")]
+        enum V04 {
+            Ping,
+            #[serde(other)]
+            Unknown,
+        }
+        let ask = Msg::Answers { id: Uuid::nil() };
+        let json = serde_json::to_string(&Envelope::request(5, ask.clone())).unwrap();
+        assert!(json.contains(r#""op":"answers""#), "{json}");
+        assert_eq!(serde_json::from_str::<V04>(&json).unwrap(), V04::Unknown);
+        assert_eq!(serde_json::from_str::<Envelope>(&json).unwrap().msg, ask);
+        let reply = Msg::AnswersReply {
+            id: Uuid::nil(),
+            answers: vec![Answer {
+                prompt: "why?".into(),
+                text: "Because <latex>x^2</latex>.".into(),
+            }],
+        };
+        let frame = Frame::Control(Envelope::request(5, reply.clone()));
+        assert_eq!(roundtrip(frame.clone()), frame);
+        let json = serde_json::to_string(&reply).unwrap();
+        assert!(json.contains(r#""op":"answers_reply""#), "{json}");
+        assert_eq!(serde_json::from_str::<V04>(&json).unwrap(), V04::Unknown);
+    }
+
+    #[test]
+    fn an_answers_reply_always_fits_a_frame() {
+        // Ten answers of 64 KiB made only of backslashes: twice that once
+        // encoded, 1.25 MiB in all.
+        let answers: Vec<Answer> = (0..10)
+            .map(|i| Answer {
+                prompt: format!("p{i}"),
+                text: "\\\\".repeat(32 * 1024),
+            })
+            .collect();
+        let msg = answers_reply(Uuid::nil(), answers.clone());
+        let json = serde_json::to_vec(&msg).unwrap();
+        assert!(json.len() <= ANSWERS_REPLY_MAX, "{}", json.len());
+        let frame = Frame::Control(Envelope::request(u64::MAX, msg.clone())).encode();
+        assert!(frame.len() - 4 <= MAX_FRAME);
+        let Msg::AnswersReply { answers: kept, .. } = msg else {
+            panic!("{msg:?}")
+        };
+        // The oldest went; the newest stay, in order.
+        assert_eq!(kept.len(), 3);
+        assert_eq!(kept[..], answers[7..]);
+        // A reply that fits is left alone.
+        let small = answers_reply(Uuid::nil(), answers[..2].to_vec());
+        assert_eq!(
+            small,
+            Msg::AnswersReply {
+                id: Uuid::nil(),
+                answers: answers[..2].to_vec(),
+            }
+        );
+    }
+
     #[test]
     fn latex_round_trips_and_is_unknown_to_older_clients() {
         let event = SessionEvent::Latex {
`````
Run: `umask 022; cargo test --locked --bin claudio answers`
Expected: compilation fails, `error: could not compile `claudio` (bin "claudio" test) due to 33 previous errors`, made of `E0425` (cannot find `recent_answers`, `read_answers`, `ANSWERS_TAIL`, `MAX_ANSWERS`, `MAX_PROMPT_BYTES`, `MAX_ANSWER_BYTES`, `ANSWERS_REPLY_MAX`, type `Answer`), `E0422` (`Answer`) and `E0599` (no variant `Answers`/`AnswersReply` on `proto::Msg`).

Step 2: implement.
`````diff
diff --git a/src/claude/projects.rs b/src/claude/projects.rs
index 886406d..4a014e6 100644
--- a/src/claude/projects.rs
+++ b/src/claude/projects.rs
@@ -7,8 +7,8 @@ use std::collections::VecDeque;
 use std::path::{Path, PathBuf};
 use std::time::UNIX_EPOCH;
 
-use crate::claude::latex::{extract_latex, MAX_FORMULAS};
-use crate::proto::ClaudeSession;
+use crate::claude::latex::{extract_latex, sanitize, MAX_FORMULAS};
+use crate::proto::{Answer, ClaudeSession};
 
 // ── Constants ─────────────────────────────────────────────────────────────────
 
@@ -480,7 +480,7 @@ fn count_user_messages(path: &Path) -> u32 {
 ///
 /// This correctly handles prompts that start with `<` (e.g. XML-tagged tasks)
 /// which were previously misclassified.
-fn is_real_prompt(record: &serde_json::Value) -> bool {
+pub(crate) fn is_real_prompt(record: &serde_json::Value) -> bool {
     let Some(msg) = record.get("message") else {
         return false;
     };
@@ -519,6 +519,181 @@ fn is_real_prompt(record: &serde_json::Value) -> bool {
     }
 }
 
+// ── Recent answers ────────────────────────────────────────────────────────────
+
+/// How many of a conversation's answers the reader view gets.
+pub const MAX_ANSWERS: usize = 10;
+/// How much of a transcript's end [`read_answers`] reads.
+const ANSWERS_TAIL: u64 = 4 * 1024 * 1024;
+/// The longest prompt and answer text sent; longer ones are cut.
+const MAX_PROMPT_BYTES: usize = 2 * 1024;
+const MAX_ANSWER_BYTES: usize = 64 * 1024;
+/// What is appended to a prompt or answer that was cut.
+const CUT_MARK: &str = " …";
+/// How Claude Code's own `type:"user"` records start: slash commands, their
+/// output and caveats, and reminders. None of them is a prompt.
+const NOT_PROMPTS: [&str; 5] = [
+    "<command-name>",
+    "<command-message>",
+    "<local-command-stdout>",
+    "<local-command-caveat>",
+    "<system-reminder>",
+];
+
+/// The [`recent_answers`] of the transcript at `path`, from its last
+/// [`ANSWERS_TAIL`] bytes; a tail's first (partial) line is skipped. A
+/// missing or unreadable transcript has none. Blocking I/O.
+pub fn read_answers(path: &Path) -> Vec<Answer> {
+    use std::io::{Read, Seek, SeekFrom};
+    let Ok(mut f) = std::fs::File::open(path) else {
+        return Vec::new();
+    };
+    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
+    let start = len.saturating_sub(ANSWERS_TAIL);
+    let mut buf = Vec::new();
+    if f.seek(SeekFrom::Start(start)).is_err()
+        || f.take(ANSWERS_TAIL).read_to_end(&mut buf).is_err()
+    {
+        return Vec::new();
+    }
+    let lines = match buf.iter().position(|&b| b == b'\n') {
+        Some(nl) if start > 0 => &buf[nl + 1..],
+        None if start > 0 => &[],
+        _ => &buf[..],
+    };
+    recent_answers(lines)
+}
+
+/// The answers of a transcript's lines, oldest first: the last
+/// [`MAX_ANSWERS`], each a prompt and the text claude answered it with.
+///
+/// An answer starts at a real prompt: a `type:"user"` record that
+/// [`is_real_prompt`] accepts, without `isMeta`, `isCompactSummary` or
+/// `isVisibleInTranscriptOnly`, and not one of Claude Code's own records
+/// ([`NOT_PROMPTS`]). Its text is the `text` blocks (or string content) of
+/// the `type:"assistant"` records that follow, joined by blank lines;
+/// records with `isApiErrorMessage` are not claude's answer, and tool
+/// results in between do not end it. A compaction boundary ends it. Sidechain
+/// records (subagents) are skipped, and so is an answer with no text.
+///
+/// `bytes` may start or end mid-line (a tail read): a line that is not a
+/// JSON record is skipped. Prompt and text are [`sanitize`]d and cut to
+/// [`MAX_PROMPT_BYTES`] and [`MAX_ANSWER_BYTES`].
+pub fn recent_answers(bytes: &[u8]) -> Vec<Answer> {
+    let mut answers = VecDeque::with_capacity(MAX_ANSWERS + 1);
+    // The answer being read: its prompt and its text blocks so far.
+    let mut current: Option<(String, Vec<String>)> = None;
+    for line in bytes.split(|&b| b == b'\n') {
+        // Cheap pre-filter: the records of interest name their type.
+        let looks_relevant = line.windows(11).any(|w| w == b"\"assistant\"")
+            || line.windows(6).any(|w| w == b"\"user\"")
+            || line.windows(16).any(|w| w == b"compact_boundary");
+        if !looks_relevant {
+            continue;
+        }
+        let Ok(v) = serde_json::from_slice::<serde_json::Value>(line) else {
+            continue;
+        };
+        if flag(&v, "isSidechain") {
+            continue;
+        }
+        match v.get("type").and_then(|t| t.as_str()) {
+            Some("user") => {
+                if let Some(prompt) = answer_prompt(&v) {
+                    finish(&mut answers, current.replace((prompt, Vec::new())));
+                }
+            }
+            Some("assistant") if !flag(&v, "isApiErrorMessage") => {
+                if let Some((_, texts)) = current.as_mut() {
+                    let content = v.get("message").and_then(|m| m.get("content"));
+                    push_texts(texts, content);
+                }
+            }
+            Some("system") => {
+                if v.get("subtype").and_then(|s| s.as_str()) == Some("compact_boundary") {
+                    finish(&mut answers, current.take());
+                }
+            }
+            _ => {}
+        }
+    }
+    finish(&mut answers, current);
+    answers.into()
+}
+
+/// Whether `record` has `key: true`.
+fn flag(record: &serde_json::Value, key: &str) -> bool {
+    record.get(key).and_then(|f| f.as_bool()) == Some(true)
+}
+
+/// The prompt of a `type:"user"` record that starts an answer (see
+/// [`recent_answers`]): its string content, or its text blocks joined by
+/// newlines.
+fn answer_prompt(record: &serde_json::Value) -> Option<String> {
+    let own = ["isMeta", "isCompactSummary", "isVisibleInTranscriptOnly"];
+    if own.iter().any(|key| flag(record, key)) || !is_real_prompt(record) {
+        return None;
+    }
+    let content = record.get("message")?.get("content")?;
+    let mut texts = Vec::new();
+    push_texts(&mut texts, Some(content));
+    let prompt = texts.join("\n");
+    let start = prompt.trim_start();
+    (!NOT_PROMPTS.iter().any(|tag| start.starts_with(tag))).then_some(prompt)
+}
+
+/// Append the text of a message's `content` to `texts`: the string itself,
+/// or its `text` blocks (not `thinking`, `tool_use` or `tool_result`).
+fn push_texts(texts: &mut Vec<String>, content: Option<&serde_json::Value>) {
+    match content {
+        Some(serde_json::Value::String(text)) => texts.push(text.clone()),
+        Some(serde_json::Value::Array(blocks)) => {
+            for block in blocks {
+                if block.get("type").and_then(|t| t.as_str()) == Some("text") {
+                    if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
+                        texts.push(text.to_owned());
+                    }
+                }
+            }
+        }
+        _ => {}
+    }
+}
+
+/// Add the answer `done` (if any, and if it has text) to `answers`, keeping
+/// the last [`MAX_ANSWERS`].
+fn finish(answers: &mut VecDeque<Answer>, done: Option<(String, Vec<String>)>) {
+    let Some((prompt, texts)) = done else { return };
+    let texts: Vec<&str> = texts
+        .iter()
+        .map(|t| t.trim())
+        .filter(|t| !t.is_empty())
+        .collect();
+    if texts.is_empty() {
+        return;
+    }
+    if answers.len() == MAX_ANSWERS {
+        answers.pop_front();
+    }
+    answers.push_back(Answer {
+        prompt: cut(&sanitize(prompt.trim()), MAX_PROMPT_BYTES),
+        text: cut(&sanitize(&texts.join("\n\n")), MAX_ANSWER_BYTES),
+    });
+}
+
+/// `s` cut to at most `max` bytes on a char boundary, with [`CUT_MARK`]
+/// appended when it was cut.
+fn cut(s: &str, max: usize) -> String {
+    if s.len() <= max {
+        return s.to_owned();
+    }
+    let mut end = max;
+    while !s.is_char_boundary(end) {
+        end -= 1;
+    }
+    format!("{}{CUT_MARK}", &s[..end])
+}
+
 /// Extract `ai-title` (last occurrence) and `last-prompt` from the tail bytes.
 fn extract_tail_meta(content: &str) -> (Option<String>, Option<String>) {
     let mut title: Option<String> = None;
diff --git a/src/daemon/mod.rs b/src/daemon/mod.rs
index cecabdd..121f33d 100644
--- a/src/daemon/mod.rs
+++ b/src/daemon/mod.rs
@@ -461,6 +461,17 @@ impl Daemon {
             .send(Frame::Control(Envelope::event(Msg::Event { id, event })));
     }
 
+    /// Where session `id`'s conversation is: its directory, and its claude
+    /// session id (`None` until claude reports one, and for a terminal).
+    /// `None` for a session the daemon does not know.
+    pub(crate) fn conversation(&self, id: SessionId) -> Option<(String, Option<String>)> {
+        let reg = self.registry();
+        let entry = reg.journal.entries().iter().find(|e| e.id == id)?;
+        let claude = entry.kind == SessionKind::Claude;
+        let sid = entry.claude_session_id.clone().filter(|_| claude);
+        Some((entry.cwd.clone(), sid))
+    }
+
     /// The command channel of a live session.
     fn session(&self, id: SessionId) -> Option<tokio::sync::mpsc::Sender<Cmd>> {
         self.registry().live.get(&id).map(|h| h.tx.clone())
diff --git a/src/daemon/server.rs b/src/daemon/server.rs
index e320b4c..909ff72 100644
--- a/src/daemon/server.rs
+++ b/src/daemon/server.rs
@@ -490,6 +490,17 @@ impl Client {
             }
             Msg::ListDir { path } => list_dir(path).await,
             Msg::ListClaudeSessions { cwd } => list_claude_sessions(cwd).await,
+            Msg::Answers { id } => {
+                // A transcript read: run beside the client loop and reply
+                // from the task, like `UpdateClaude`.
+                let conversation = self.daemon.conversation(id);
+                let queue = self.queue.clone();
+                tokio::spawn(async move {
+                    let msg = answers(id, conversation).await;
+                    let _ = queue.send(Frame::Control(Envelope { req, msg })).await;
+                });
+                return;
+            }
             Msg::RecentProjects { limit } => recent_projects(limit).await,
             Msg::SubscribeHostStats => {
                 // Spawn a task that watches the stats channel and pushes
@@ -685,6 +696,35 @@ async fn list_claude_sessions(cwd: String) -> Msg {
     }
 }
 
+/// The `AnswersReply` for session `id`, whose `conversation` is
+/// [`Daemon::conversation`]'s: the recent answers of its transcript, read
+/// and capped on a blocking thread. No conversation yet, no answers.
+async fn answers(id: SessionId, conversation: Option<(String, Option<String>)>) -> Msg {
+    let Some((cwd, sid)) = conversation else {
+        return Msg::Error {
+            message: format!("no such session: {id}"),
+        };
+    };
+    // A claude session id is a UUID; anything else must not become a path.
+    let sid = sid.filter(|sid| {
+        let safe = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
+        !sid.is_empty() && sid.chars().all(safe)
+    });
+    let read = tokio::task::spawn_blocking(move || {
+        let answers = match sid {
+            Some(sid) => {
+                let path = projects::transcript_path(&host::expand_tilde(&cwd), &sid);
+                projects::read_answers(&path)
+            }
+            None => Vec::new(),
+        };
+        proto::answers_reply(id, answers)
+    });
+    read.await.unwrap_or_else(|e| Msg::Error {
+        message: format!("answers failed: {e}"),
+    })
+}
+
 async fn recent_projects(limit: u32) -> Msg {
     let limit = usize::try_from(limit).unwrap_or(usize::MAX);
     match tokio::task::spawn_blocking(move || projects::recent_project_dirs(limit)).await {
diff --git a/src/proto.rs b/src/proto.rs
index e1d96f3..63b9721 100644
--- a/src/proto.rs
+++ b/src/proto.rs
@@ -117,6 +117,12 @@ pub enum Msg {
     ListClaudeSessions {
         cwd: String,
     },
+    /// The session's most recent answers, for the reader view. Answered by
+    /// `AnswersReply` (empty when the session has no conversation yet). An
+    /// older daemon answers `Error "unsupported op…"`.
+    Answers {
+        id: SessionId,
+    },
     /// Directories with claude history on this host, newest first (seeds the
     /// new-session directory picker).
     RecentProjects {
@@ -212,6 +218,11 @@ pub enum Msg {
         cwd: String,
         sessions: Vec<ClaudeSession>,
     },
+    /// Reply to `Answers`: oldest first, built by [`answers_reply`].
+    AnswersReply {
+        id: SessionId,
+        answers: Vec<Answer>,
+    },
     Projects {
         dirs: Vec<ProjectDir>,
     },
@@ -664,6 +675,36 @@ pub struct ClaudeSession {
     pub messages: u32,
 }
 
+/// The largest encoded `AnswersReply` the daemon sends: half of
+/// [`MAX_FRAME`], so the reply and its envelope always fit a frame.
+pub const ANSWERS_REPLY_MAX: usize = 512 * 1024;
+
+/// One of claude's answers, for the reader view: the user's prompt and the
+/// text claude answered it with (markdown, `<latex>` tags included). Both
+/// are capped and sanitized by the daemon.
+#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
+pub struct Answer {
+    pub prompt: String,
+    pub text: String,
+}
+
+/// The `AnswersReply` for session `id`: `answers` (oldest first) without as
+/// many of the oldest as it takes for its JSON to fit
+/// [`ANSWERS_REPLY_MAX`]. JSON escapes every `\` and `"` of a formula, so
+/// the cap is on the encoded size, not on the text's.
+pub fn answers_reply(id: SessionId, answers: Vec<Answer>) -> Msg {
+    let mut msg = Msg::AnswersReply { id, answers };
+    while serde_json::to_vec(&msg).map_or(false, |json| json.len() > ANSWERS_REPLY_MAX) {
+        match &mut msg {
+            Msg::AnswersReply { answers, .. } if !answers.is_empty() => {
+                answers.remove(0);
+            }
+            _ => break,
+        }
+    }
+    msg
+}
+
 /// The most commits one `GitLog` request may ask for.
 pub const GIT_LOG_MAX: u32 = 500;
 
`````
Run: `umask 022; cargo test --locked --bin claudio answers && cargo build --locked 2>&1 | grep -cE '^(warning|error)'; rustfmt --edition 2021 --check src/daemon/server.rs && make test-unit`
Expected: `test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 829 filtered out`, then `0`, then (make) `test result: ok. 837 passed; 0 failed`.

Step 3: commit
`/usr/bin/git commit -m "feat(daemon): serve a session's recent answers" -- src/proto.rs src/claude/projects.rs src/daemon/mod.rs src/daemon/server.rs src/daemon/tests.rs`

Interfaces this task provides to later tasks: `proto::Msg::Answers { id: SessionId }`, `proto::Msg::AnswersReply { id: SessionId, answers: Vec<Answer> }`, `proto::Answer { pub prompt: String, pub text: String }`, `proto::ANSWERS_REPLY_MAX`, `proto::answers_reply(id, Vec<Answer>) -> Msg`, `claude::projects::{recent_answers(&[u8]) -> Vec<Answer>, read_answers(&Path) -> Vec<Answer>, MAX_ANSWERS}`, `Daemon::conversation(id) -> Option<(String, Option<String>)>`.

### Task 2: the typesetting engine
Model: opus   Repo: /volumes/repos/claudio   Files: src/tui/mod.rs, src/tui/typeset/mod.rs (new), src/tui/typeset/mathfont.rs (new)
Goal: AC4 (every golden of Appendix F3/F4 but `aligned`, which needs Task 3's environment split) and AC6. A pure box model over the rust-latex-parser tree: `Block { rows, width, baseline }`, `hcat` on the baseline, `stack` (centred, the odd column on the right), the precise style rule (display only at the top; text inside scripts, limits, fractions and roots; script style drops whitespace), scripts as glyphs or raised/lowered rows (subscript first when both are glyphs), display fractions with a bar as wide as the wider part, flat `a/b` with the v0.4.0 parenthesis rule otherwise, big operators with limits above/below (∑ ∏ ∐ ⋃ ⋂ ⨁, `\lim`) or as scripts (integrals, text style), `√` with the radicand parenthesised unless atomic, the `\sqrt[n]` source rewrite to a raised index, whitespace runs (one space, two for ≥ 12 pt, none for a lone thin space, never at a box's ends), a space after an operator name unless a bracket follows, stretched delimiters, accents, `\binom`, braces and `\overset` annotations, matrices and cases (top-biased baseline), and the math-font tables ported from term-maths (`mathfont.rs`, same mappings). Any box past 512 × 40 aborts the layout. Nothing calls the engine yet, so `mod typeset;` carries a temporary `#[allow(dead_code)]`, which Task 3 removes.

The ∑ and ∫ goldens of F3 are one column inconsistent inside themselves; the tests use the engine's self-consistent rows, marked in a comment (see "Spec deviations needing a ruling", D1).

Step 1: write the failing tests (the module holds only its tests for now; the ported `mathfont.rs` tests come with it).
`````diff
diff --git a/src/tui/mod.rs b/src/tui/mod.rs
index 763b97b..ae76f3b 100644
--- a/src/tui/mod.rs
+++ b/src/tui/mod.rs
@@ -22,6 +22,9 @@ mod selection;
 mod sessions;
 mod state;
 mod stats_view;
+// Nothing calls the engine until `latex::render` does (the next task).
+#[allow(dead_code)]
+mod typeset;
 
 /// `recent` without the local directories that no longer exist, so the
 /// wizard never offers a deleted directory (the check is I/O, so it happens
diff --git a/src/tui/typeset/mathfont.rs b/src/tui/typeset/mathfont.rs
new file mode 100644
index 0000000..575ca22
--- /dev/null
+++ b/src/tui/typeset/mathfont.rs
@@ -0,0 +1,61 @@
+#[cfg(test)]
+mod tests {
+    use super::*;
+
+    #[test]
+    fn test_bold() {
+        assert_eq!(to_bold('A'), '𝐀');
+        assert_eq!(to_bold('Z'), '𝐙');
+        assert_eq!(to_bold('a'), '𝐚');
+        assert_eq!(to_bold('z'), '𝐳');
+        assert_eq!(to_bold('0'), '𝟎');
+    }
+
+    #[test]
+    fn test_double_struck() {
+        assert_eq!(to_double_struck('R'), 'ℝ');
+        assert_eq!(to_double_struck('Z'), 'ℤ');
+        assert_eq!(to_double_struck('N'), 'ℕ');
+        assert_eq!(to_double_struck('C'), 'ℂ');
+        assert_eq!(to_double_struck('Q'), 'ℚ');
+        // Non-exception uppercase
+        assert_eq!(to_double_struck('A'), '𝔸');
+    }
+
+    #[test]
+    fn test_script() {
+        assert_eq!(to_script('L'), 'ℒ');
+        assert_eq!(to_script('H'), 'ℋ');
+        assert_eq!(to_script('B'), 'ℬ');
+        // Non-exception
+        assert_eq!(to_script('A'), '𝒜');
+    }
+
+    #[test]
+    fn test_fraktur() {
+        assert_eq!(to_fraktur('H'), 'ℌ');
+        assert_eq!(to_fraktur('R'), 'ℜ');
+        assert_eq!(to_fraktur('a'), '𝔞');
+        assert_eq!(to_fraktur('g'), '𝔤');
+    }
+
+    #[test]
+    fn test_sans_serif() {
+        assert_eq!(to_sans_serif('A'), '𝖠');
+        assert_eq!(to_sans_serif('a'), '𝖺');
+    }
+
+    #[test]
+    fn test_monospace() {
+        assert_eq!(to_monospace('A'), '𝙰');
+        assert_eq!(to_monospace('a'), '𝚊');
+        assert_eq!(to_monospace('0'), '𝟶');
+    }
+
+    #[test]
+    fn test_non_letter_passthrough() {
+        // Non-letter characters should pass through unchanged
+        assert_eq!(map_char(MathFontKind::Bold, '+'), '+');
+        assert_eq!(map_char(MathFontKind::Blackboard, ' '), ' ');
+    }
+}
diff --git a/src/tui/typeset/mod.rs b/src/tui/typeset/mod.rs
new file mode 100644
index 0000000..821c18d
--- /dev/null
+++ b/src/tui/typeset/mod.rs
@@ -0,0 +1,238 @@
+#[cfg(test)]
+mod tests {
+    use super::*;
+
+    /// The rows of `src` typeset in display style (`display`) or text style.
+    fn rows(src: &str, display: bool) -> Vec<String> {
+        let tree = rust_latex_parser::parse_equation(&rewrite_roots(src));
+        typeset(&tree, display)
+            .unwrap_or_else(|| panic!("nothing for {src}"))
+            .rows
+    }
+
+    fn display(src: &str) -> Vec<String> {
+        rows(src, true)
+    }
+
+    fn text(src: &str) -> Vec<String> {
+        rows(src, false)
+    }
+
+    /// AC4: the golden rows of the spec (Appendix A, F3).
+    #[test]
+    fn display_formulas_match_the_golden_rows() {
+        let golden: [(&str, &[&str]); 10] = [
+            (
+                "x = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}{2a}",
+                &[
+                    "    -b ± √(b² - 4ac)",
+                    "x = ────────────────",
+                    "           2a",
+                ],
+            ),
+            (
+                "p(x) = \\frac{1}{\\sqrt{2\\pi\\sigma^2}} e^{-\\frac{(x-\\mu)^2}{2\\sigma^2}}",
+                &[
+                    "          1     -(x-μ)²/(2σ²)",
+                    "p(x) = ─────── e",
+                    "       √(2πσ²)",
+                ],
+            ),
+            (
+                "S_{i,f} = \\sum_{s : P_s \\notin X} s_{P_s,i,f} + G_f(i)",
+                &[
+                    "S    =   ∑    s       + G (i)",
+                    " i,f   s:Pₛ∉X  Pₛ,i,f    f",
+                ],
+            ),
+            (
+                "\\sum_{k=1}^{n} k = \\frac{n(n+1)}{2}",
+                // The spec's middle row, " ∑ k = ────────", is one column
+                // short of its other two (ruling requested in the plan).
+                &[" n      n(n + 1)", " ∑  k = ────────", "k=1        2"],
+            ),
+            (
+                "\\begin{pmatrix} a & b \\\\ c & d \\end{pmatrix}",
+                &["⎛a  b⎞", "⎝c  d⎠"],
+            ),
+            (
+                "\\begin{vmatrix} a & b \\\\ c & d \\end{vmatrix}",
+                &["│a  b│", "│c  d│"],
+            ),
+            (
+                "f(x) = \\begin{cases} 1 & x > 0 \\\\ 0 & \\text{otherwise} \\end{cases}",
+                &["f(x) = ⎰1  x > 0", "       ⎱0  otherwise"],
+            ),
+            (
+                "x^{a^b} + y_{i_j} + z_{i}^{2}",
+                &[" aᵇ", "x   + y   + zᵢ²", "       iⱼ"],
+            ),
+            (
+                "\\int_0^\\infty e^{-x^2} \\, dx = \\frac{\\sqrt{\\pi}}{2}",
+                // The spec's first and last rows, " ∞  -x²      √π" and
+                // " 0           2", are one column off its middle row
+                // (ruling requested in the plan).
+                &[" ∞ -x²      √π", "∫ e    dx = ──", " 0          2"],
+            ),
+            (
+                "\\lim_{x \\to 0} \\frac{\\sin x}{x} = 1",
+                &["    sin x", "lim ───── = 1", "x→0   x"],
+            ),
+        ];
+        for (src, want) in golden {
+            assert_eq!(display(src), want, "{src}");
+        }
+        assert_eq!(display("\\sin x"), ["sin x"]);
+        assert_eq!(display("\\sin(x) + \\log_2 n"), ["sin(x) + log₂ n"]);
+    }
+
+    /// AC6: the condensed targets.
+    #[test]
+    fn the_layout_is_condensed() {
+        let quadratic = display("x = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}{2a}");
+        assert_eq!(quadratic.len(), 3);
+        let gaussian =
+            display("p(x) = \\frac{1}{\\sqrt{2\\pi\\sigma^2}} e^{-\\frac{(x-\\mu)^2}{2\\sigma^2}}");
+        assert!(gaussian.len() <= 4, "{gaussian:?}");
+        let s = display("S_{i,f} = \\sum_{s : P_s \\notin X} s_{P_s,i,f} + G_f(i)");
+        let width = s.iter().map(|r| r.width()).max().unwrap();
+        assert!(s.len() <= 3 && width <= 30, "{s:?}");
+        for laid in [&quadratic, &gaussian, &s] {
+            assert!(
+                laid.iter().all(|r| !r.trim().is_empty()),
+                "a blank row: {laid:?}"
+            );
+            assert!(
+                laid.iter().any(|r| !r.starts_with(' ')),
+                "a blank first column"
+            );
+            assert!(laid.iter().all(|r| !r.ends_with(' ')), "trailing blanks");
+        }
+    }
+
+    #[test]
+    fn text_style_is_flat_with_limits_as_scripts() {
+        assert_eq!(text("\\frac{a+b}{c}"), ["(a + b)/c"]);
+        assert_eq!(
+            text("\\sum_{i=1}^{n} i = \\frac{n(n+1)}{2}"),
+            ["∑ᵢ₌₁ⁿ i = n(n + 1)/2"]
+        );
+        assert_eq!(text("\\int_0^1 x^2\\,dx"), ["∫₀¹ x² dx"]);
+        assert_eq!(text("\\lim_{n \\to \\infty} a_n"), ["lim    aₙ", "   n→∞"]);
+        // Inside a display formula, a fraction's parts are text style.
+        assert_eq!(display("\\frac{\\frac{a}{b}}{c}"), ["a/b", "───", " c"]);
+    }
+
+    #[test]
+    fn whitespace_runs_are_one_space_and_quads_two() {
+        assert_eq!(display("a \\quad b \\qquad c \\! d \\; e"), ["a  b  c d e"]);
+        assert_eq!(display("a\\!b\\,c"), ["ab c"]);
+        assert_eq!(display("  x  =  y  "), ["x = y"]);
+        // Operators the parser leaves bare are spaced like the others.
+        assert_eq!(display("a \\cdot b \\cup c"), ["a · b ∪ c"]);
+        // Not in scripts.
+        assert_eq!(display("x_{a \\cdot b}"), ["x", " a·b"]);
+    }
+
+    #[test]
+    fn operator_names_are_spaced_from_their_argument() {
+        assert_eq!(display("\\sin\\theta + \\cos(x)"), ["sin θ + cos(x)"]);
+        assert_eq!(display("\\sin^2 x"), ["sin² x"]);
+        assert_eq!(
+            display("\\operatorname{rank}(A) \\leq \\min(m, n)"),
+            ["rank(A) ≤ min(m, n)"]
+        );
+    }
+
+    #[test]
+    fn centring_leaves_the_odd_column_on_the_right() {
+        assert_eq!(display("\\frac{ab}{c}"), ["ab", "──", "c"]);
+        assert_eq!(display("\\frac{abcd}{c}"), ["abcd", "────", " c"]);
+        assert_eq!(display("\\frac{a}{b}"), ["a", "─", "b"]);
+    }
+
+    #[test]
+    fn roots_take_their_index_as_a_raised_script() {
+        assert_eq!(rewrite_roots("\\sqrt[3]{x}"), "{}^{3}\\sqrt{x}");
+        assert_eq!(
+            rewrite_roots("\\sqrt [n]x + \\sqrt{y}"),
+            "{}^{n}\\sqrt x + \\sqrt{y}"
+        );
+        assert_eq!(rewrite_roots("\\sqrt[{a]}]{x}"), "{}^{{a]}}\\sqrt{x}");
+        assert_eq!(rewrite_roots("\\sqrt[3"), "\\sqrt[3");
+        assert_eq!(display("\\sqrt[3]{x}"), ["³√x"]);
+        assert_eq!(display("\\sqrt[3]{x+1}"), ["³√(x + 1)"]);
+        assert_eq!(display("\\sqrt[\\alpha]{x}"), ["α", " √x"]);
+        assert_eq!(display("\\sqrt{x}"), ["√x"]);
+    }
+
+    #[test]
+    fn delimiters_stretch_to_their_content() {
+        assert_eq!(display("\\left( x \\right)"), ["(x)"]);
+        assert_eq!(
+            display("\\left( \\frac{a}{b} \\right)^2"),
+            ["⎛a⎞²", "⎜─⎟", "⎝b⎠"]
+        );
+        assert_eq!(
+            display("\\left\\{ \\frac{a}{b} \\right."),
+            ["⎧a", "⎨─", "⎩b"]
+        );
+        assert_eq!(
+            display("\\left| \\frac{a}{b} \\right|"),
+            ["│a│", "│─│", "│b│"]
+        );
+        assert_eq!(
+            display("\\left[ \\frac{a}{b} \\right]"),
+            ["⎡a⎤", "⎢─⎥", "⎣b⎦"]
+        );
+    }
+
+    #[test]
+    fn accents_binomials_and_annotations() {
+        assert_eq!(display("\\hat{x} + \\vec{AB}"), ["x\u{302} + vec(AB)"]);
+        assert_eq!(text("\\binom{n}{k}"), ["(n choose k)"]);
+        assert_eq!(display("\\binom{n}{k}"), ["⎛n⎞", "⎝k⎠"]);
+        assert_eq!(display("\\overbrace{a+b}^{n}"), ["  n", "a + b"]);
+        assert_eq!(display("\\underbrace{x+y}_{2}"), ["x + y", "  2"]);
+        assert_eq!(display("\\overset{def}{=}"), ["def", " ="]);
+    }
+
+    #[test]
+    fn matrices_and_cases_of_other_shapes() {
+        assert_eq!(display("\\begin{bmatrix} 1 & 2 \\end{bmatrix}"), ["[1  2]"]);
+        assert_eq!(
+            display("\\begin{bmatrix} 1 & 20 \\\\ 300 & 4 \\\\ 5 & 6 \\end{bmatrix}"),
+            ["⎡ 1   20⎤", "⎢300  4 ⎥", "⎣ 5   6 ⎦"]
+        );
+        assert_eq!(
+            display("\\begin{cases} a \\\\ b \\\\ c \\end{cases}"),
+            ["⎧a", "⎨b", "⎩c"]
+        );
+        assert_eq!(
+            display("x = \\begin{Vmatrix} a \\\\ b \\end{Vmatrix}"),
+            ["x = ‖a‖", "    ‖b‖"]
+        );
+    }
+
+    #[test]
+    fn math_fonts_map_letters() {
+        assert_eq!(display("\\mathbb{R}^n"), ["ℝⁿ"]);
+        assert_eq!(display("\\mathbf{v} + \\mathcal{L}"), ["𝐯 + ℒ"]);
+    }
+
+    #[test]
+    fn layouts_past_the_limits_are_none() {
+        let wide = "x + ".repeat(MAX_COLS / 4 + 1);
+        let tree = rust_latex_parser::parse_equation(&wide);
+        assert!(typeset(&tree, true).is_none());
+        let tall = format!(
+            "\\begin{{pmatrix}} {} \\end{{pmatrix}}",
+            "a \\\\ ".repeat(MAX_ROWS + 1)
+        );
+        let tree = rust_latex_parser::parse_equation(&tall);
+        assert!(typeset(&tree, true).is_none());
+        // Nothing visible is nothing.
+        let tree = rust_latex_parser::parse_equation("\\, \\quad");
+        assert!(typeset(&tree, true).is_none());
+    }
+}
`````
Run: `umask 022; cargo test --locked --bin claudio typeset`
Expected: compilation fails, `error: could not compile `claudio` (bin "claudio" test) due to 12 previous errors; 1 warning emitted`: `E0425` cannot find function `typeset` (4), `rewrite_roots` (5), values `MAX_COLS` and `MAX_ROWS`, `E0599` no method `width` on `&String`; the warning is the unused `use super::*`.

Step 2: implement (the engine above the tests, and `mathfont.rs`'s tables above its tests).
`````diff
diff --git a/src/tui/typeset/mathfont.rs b/src/tui/typeset/mathfont.rs
index 359fe13..575ca22 100644
--- a/src/tui/typeset/mathfont.rs
+++ b/src/tui/typeset/mathfont.rs
@@ -1,3 +1,135 @@
+//! Unicode Mathematical Alphanumeric Symbols mapping (U+1D400–U+1D7FF).
+//!
+//! Maps ASCII Latin letters (and digits) to their styled variants in the
+//! Unicode Mathematical Alphanumeric Symbols block, keyed by `MathFontKind`.
+//!
+//! Ported unchanged from term-maths 1.0.0 (`src/mathfont.rs`, MIT OR
+//! Apache-2.0), which claudio no longer depends on: `\mathbb{R}` must stay
+//! `ℝ`.
+
+use rust_latex_parser::MathFontKind;
+
+/// Convert a character to its mathematical font variant.
+/// Returns the original character if no mapping exists.
+pub fn map_char(kind: MathFontKind, ch: char) -> char {
+    match kind {
+        MathFontKind::Bold => to_bold(ch),
+        MathFontKind::Blackboard => to_double_struck(ch),
+        MathFontKind::Calligraphic => to_script(ch),
+        MathFontKind::Fraktur => to_fraktur(ch),
+        MathFontKind::Roman => ch, // upright, no transformation
+        MathFontKind::SansSerif => to_sans_serif(ch),
+        MathFontKind::Monospace => to_monospace(ch),
+    }
+}
+
+// U+1D400 MATHEMATICAL BOLD CAPITAL A .. U+1D419 MATHEMATICAL BOLD CAPITAL Z
+// U+1D41A MATHEMATICAL BOLD SMALL A .. U+1D433 MATHEMATICAL BOLD SMALL Z
+// U+1D7CE MATHEMATICAL BOLD DIGIT ZERO .. U+1D7D7 MATHEMATICAL BOLD DIGIT NINE
+fn to_bold(ch: char) -> char {
+    match ch {
+        'A'..='Z' => char::from_u32(0x1D400 + (ch as u32 - 'A' as u32)).unwrap_or(ch),
+        'a'..='z' => char::from_u32(0x1D41A + (ch as u32 - 'a' as u32)).unwrap_or(ch),
+        '0'..='9' => char::from_u32(0x1D7CE + (ch as u32 - '0' as u32)).unwrap_or(ch),
+        // Bold Greek uppercase: U+1D6A8–U+1D6C0
+        'Α'..='Ω' => char::from_u32(0x1D6A8 + (ch as u32 - 'Α' as u32)).unwrap_or(ch),
+        // Bold Greek lowercase: U+1D6C2–U+1D6DA
+        'α'..='ω' => char::from_u32(0x1D6C2 + (ch as u32 - 'α' as u32)).unwrap_or(ch),
+        _ => ch,
+    }
+}
+
+// U+1D538 MATHEMATICAL DOUBLE-STRUCK CAPITAL A .. U+1D551
+// Exceptions: C=ℂ, H=ℍ, N=ℕ, P=ℙ, Q=ℚ, R=ℝ, Z=ℤ (in Letterlike Symbols block)
+// U+1D552 MATHEMATICAL DOUBLE-STRUCK SMALL A .. U+1D56B
+// U+1D7D8 MATHEMATICAL DOUBLE-STRUCK DIGIT ZERO .. U+1D7E1
+fn to_double_struck(ch: char) -> char {
+    match ch {
+        'C' => 'ℂ',
+        'H' => 'ℍ',
+        'N' => 'ℕ',
+        'P' => 'ℙ',
+        'Q' => 'ℚ',
+        'R' => 'ℝ',
+        'Z' => 'ℤ',
+        'A' | 'B' | 'D'..='G' | 'I'..='M' | 'O' | 'S'..='Y' => {
+            char::from_u32(0x1D538 + (ch as u32 - 'A' as u32)).unwrap_or(ch)
+        }
+        'a'..='z' => char::from_u32(0x1D552 + (ch as u32 - 'a' as u32)).unwrap_or(ch),
+        '0'..='9' => char::from_u32(0x1D7D8 + (ch as u32 - '0' as u32)).unwrap_or(ch),
+        _ => ch,
+    }
+}
+
+// U+1D49C MATHEMATICAL SCRIPT CAPITAL A .. U+1D4B5
+// Exceptions: B=ℬ, E=ℰ, F=ℱ, H=ℋ, I=ℐ, L=ℒ, M=ℳ, R=ℛ (Letterlike Symbols)
+// U+1D4B6 MATHEMATICAL SCRIPT SMALL A .. U+1D4CF
+// Exceptions: e=ℯ, g=ℊ, o=ℴ
+fn to_script(ch: char) -> char {
+    match ch {
+        'B' => 'ℬ',
+        'E' => 'ℰ',
+        'F' => 'ℱ',
+        'H' => 'ℋ',
+        'I' => 'ℐ',
+        'L' => 'ℒ',
+        'M' => 'ℳ',
+        'R' => 'ℛ',
+        'e' => 'ℯ',
+        'g' => 'ℊ',
+        'o' => 'ℴ',
+        'A' | 'C' | 'D' | 'G' | 'J' | 'K' | 'N'..='Q' | 'S'..='Z' => {
+            char::from_u32(0x1D49C + (ch as u32 - 'A' as u32)).unwrap_or(ch)
+        }
+        'a'..='d' | 'f' | 'h'..='n' | 'p'..='z' => {
+            char::from_u32(0x1D4B6 + (ch as u32 - 'a' as u32)).unwrap_or(ch)
+        }
+        _ => ch,
+    }
+}
+
+// U+1D504 MATHEMATICAL FRAKTUR CAPITAL A .. U+1D51C
+// Exceptions: C=ℭ, H=ℌ, I=ℑ, R=ℜ, Z=ℨ
+// U+1D51E MATHEMATICAL FRAKTUR SMALL A .. U+1D537
+fn to_fraktur(ch: char) -> char {
+    match ch {
+        'C' => 'ℭ',
+        'H' => 'ℌ',
+        'I' => 'ℑ',
+        'R' => 'ℜ',
+        'Z' => 'ℨ',
+        'A' | 'B' | 'D'..='G' | 'J'..='Q' | 'S'..='Y' => {
+            char::from_u32(0x1D504 + (ch as u32 - 'A' as u32)).unwrap_or(ch)
+        }
+        'a'..='z' => char::from_u32(0x1D51E + (ch as u32 - 'a' as u32)).unwrap_or(ch),
+        _ => ch,
+    }
+}
+
+// U+1D5A0 MATHEMATICAL SANS-SERIF CAPITAL A .. U+1D5B9
+// U+1D5BA MATHEMATICAL SANS-SERIF SMALL A .. U+1D5D3
+// U+1D7E2 MATHEMATICAL SANS-SERIF DIGIT ZERO .. U+1D7EB
+fn to_sans_serif(ch: char) -> char {
+    match ch {
+        'A'..='Z' => char::from_u32(0x1D5A0 + (ch as u32 - 'A' as u32)).unwrap_or(ch),
+        'a'..='z' => char::from_u32(0x1D5BA + (ch as u32 - 'a' as u32)).unwrap_or(ch),
+        '0'..='9' => char::from_u32(0x1D7E2 + (ch as u32 - '0' as u32)).unwrap_or(ch),
+        _ => ch,
+    }
+}
+
+// U+1D670 MATHEMATICAL MONOSPACE CAPITAL A .. U+1D689
+// U+1D68A MATHEMATICAL MONOSPACE SMALL A .. U+1D6A3
+// U+1D7F6 MATHEMATICAL MONOSPACE DIGIT ZERO .. U+1D7FF
+fn to_monospace(ch: char) -> char {
+    match ch {
+        'A'..='Z' => char::from_u32(0x1D670 + (ch as u32 - 'A' as u32)).unwrap_or(ch),
+        'a'..='z' => char::from_u32(0x1D68A + (ch as u32 - 'a' as u32)).unwrap_or(ch),
+        '0'..='9' => char::from_u32(0x1D7F6 + (ch as u32 - '0' as u32)).unwrap_or(ch),
+        _ => ch,
+    }
+}
+
 #[cfg(test)]
 mod tests {
     use super::*;
diff --git a/src/tui/typeset/mod.rs b/src/tui/typeset/mod.rs
index 3176baa..821c18d 100644
--- a/src/tui/typeset/mod.rs
+++ b/src/tui/typeset/mod.rs
@@ -1,3 +1,845 @@
+//! Typesetting formulas as rows of Unicode text: a compact 2D layout of the
+//! `rust-latex-parser` tree, after TeX's display and text styles.
+//!
+//! Pure functions only, and as deep as the tree: the parser behind it is
+//! slow and deep on hostile input, so this runs where the parse does, on
+//! the render guard's scratch thread (`tui/mod.rs`), after the pre-check.
+//!
+//! - [`typeset`] — a parsed formula as rows, in display or text style.
+//! - [`rewrite_roots`] — `\sqrt[n]{x}` as the parser can read it.
+//! - [`Laid`] — the result: rows and the row of the baseline.
+//!
+//! The rules (spec "Typeset reader view", §3 and Appendix A):
+//! - Display style only at the top of a display formula. Inside a script,
+//!   a limit, a fraction or a root everything is text style, and scripts
+//!   and limits drop whitespace.
+//! - In text style fractions are flat (`a/b`) and big operators take their
+//!   limits as scripts.
+//! - A script whose characters all have Unicode script forms is those
+//!   glyphs, on the base's row; otherwise it is a row of its own, raised or
+//!   lowered next to the base.
+//! - A run of spaces is one space (two for `\quad`), never at a box's ends.
+
+mod mathfont;
+
+use rust_latex_parser::{AccentKind, EqNode, MatrixKind};
+use unicode_width::UnicodeWidthStr;
+
+/// The widest and tallest any part of a layout may grow: past these the
+/// formula is not typeset at all (it can only grow from there).
+const MAX_COLS: usize = 512;
+const MAX_ROWS: usize = 40;
+
+/// Big operators whose limits go above and below in display style; the
+/// others (integrals) keep them as scripts, as TeX does.
+const LIMITS_ABOVE: [&str; 6] = ["∑", "∏", "∐", "⋃", "⋂", "⨁"];
+
+/// Binary operators and relations the parser leaves without spacing; they
+/// get one space each side like the ones it spaces.
+const SPACED: [&str; 12] = ["·", "∉", "∋", "∪", "∩", "∧", "∨", "∘", "⊕", "⊗", "∖", "⊥"];
+
+/// A typeset formula: its rows (no trailing blanks, no blank row at either
+/// end, no blank column on the left) and which of them is the baseline,
+/// where the surrounding text sits.
+#[derive(Debug, Clone, PartialEq)]
+pub struct Laid {
+    pub rows: Vec<String>,
+    pub baseline: usize,
+}
+
+impl Laid {
+    /// The width of the widest row, in columns.
+    pub fn width(&self) -> usize {
+        self.rows.iter().map(|r| r.width()).max().unwrap_or(0)
+    }
+}
+
+/// Lay `tree` out in display style (`display`) or text style. `None` when
+/// a part grows past [`MAX_COLS`] × [`MAX_ROWS`] or nothing visible is
+/// left.
+pub fn typeset(tree: &EqNode, display: bool) -> Option<Laid> {
+    let style = if display { Style::Display } else { Style::Text };
+    trimmed(lay(tree, style)?)
+}
+
+/// `src` with each `\sqrt[<index>]` written as `{}^{<index>}\sqrt`: the
+/// parser has no root index, so it becomes a raised script in front of the
+/// root (`³√x`). An index with no closing `]` is left alone.
+pub fn rewrite_roots(src: &str) -> String {
+    const ROOT: &str = "\\sqrt";
+    let mut out = String::with_capacity(src.len());
+    let mut rest = src;
+    while let Some(at) = rest.find(ROOT) {
+        let after = &rest[at + ROOT.len()..];
+        let open = after.len() - after.trim_start().len();
+        let index = after[open..]
+            .strip_prefix('[')
+            .and_then(|tail| bracket_end(tail).map(|end| &tail[..end]));
+        out.push_str(&rest[..at]);
+        match index {
+            Some(index) => {
+                out.push_str("{}^{");
+                out.push_str(index);
+                out.push('}');
+                out.push_str(ROOT);
+                rest = &after[open + 1 + index.len() + 1..];
+                // `\sqrt[n]x`: the root's name must not run into its argument.
+                if rest.starts_with(|c: char| c.is_ascii_alphabetic()) {
+                    out.push(' ');
+                }
+            }
+            None => {
+                out.push_str(ROOT);
+                rest = after;
+            }
+        }
+    }
+    out.push_str(rest);
+    out
+}
+
+/// Where the `]` that closes a bracket ends `s` (the text after `[`): the
+/// first one outside braces.
+fn bracket_end(s: &str) -> Option<usize> {
+    let mut depth = 0usize;
+    for (i, c) in s.char_indices() {
+        match c {
+            '{' => depth += 1,
+            '}' => depth = depth.saturating_sub(1),
+            ']' if depth == 0 => return Some(i),
+            _ => {}
+        }
+    }
+    None
+}
+
+// ── Boxes ─────────────────────────────────────────────────────────────────────
+
+/// TeX's styles, plus the script style: text style without whitespace.
+#[derive(Debug, Clone, Copy, PartialEq, Eq)]
+enum Style {
+    Display,
+    Text,
+    Script,
+}
+
+impl Style {
+    /// The style inside a fraction (either part) or a root.
+    fn inner(self) -> Style {
+        match self {
+            Style::Script => Style::Script,
+            _ => Style::Text,
+        }
+    }
+}
+
+/// A box of text: `rows`, each exactly `width` columns, with the baseline
+/// at row `baseline`.
+#[derive(Debug, Clone, PartialEq)]
+struct Block {
+    rows: Vec<String>,
+    width: usize,
+    baseline: usize,
+}
+
+impl Block {
+    /// One row of text.
+    fn text(s: &str) -> Block {
+        Block {
+            rows: vec![s.to_owned()],
+            width: s.width(),
+            baseline: 0,
+        }
+    }
+
+    /// Nothing: one row, no columns (neutral in [`hcat`]).
+    fn empty() -> Block {
+        Block::text("")
+    }
+
+    fn height(&self) -> usize {
+        self.rows.len()
+    }
+
+    /// The text of a one-row block.
+    fn line(&self) -> Option<&str> {
+        match self.rows.as_slice() {
+            [one] => Some(one),
+            _ => None,
+        }
+    }
+
+    /// Every character mapped through `f`, rows padded again.
+    fn map(self, f: impl Fn(char) -> char) -> Block {
+        let rows: Vec<String> = self
+            .rows
+            .iter()
+            .map(|r| r.chars().map(&f).collect())
+            .collect();
+        let width = rows.iter().map(|r| r.width()).max().unwrap_or(0);
+        let rows = rows.into_iter().map(|r| padded(r, width)).collect();
+        Block {
+            rows,
+            width,
+            baseline: self.baseline,
+        }
+    }
+}
+
+/// `None` when a box of `width` × `height` is past the limits.
+fn fits(width: usize, height: usize) -> Option<()> {
+    (width <= MAX_COLS && height <= MAX_ROWS).then_some(())
+}
+
+/// `s` padded with spaces to `width` columns.
+fn padded(mut s: String, width: usize) -> String {
+    let w = s.width();
+    s.extend(std::iter::repeat_n(' ', width.saturating_sub(w)));
+    s
+}
+
+/// `parts` side by side, their baselines on one row.
+fn hcat(parts: &[Block]) -> Option<Block> {
+    let above = parts.iter().map(|b| b.baseline).max().unwrap_or(0);
+    let below = parts
+        .iter()
+        .map(|b| b.height() - 1 - b.baseline)
+        .max()
+        .unwrap_or(0);
+    let height = above + below + 1;
+    let width = parts.iter().map(|b| b.width).sum();
+    fits(width, height)?;
+    let mut rows = vec![String::new(); height];
+    for b in parts {
+        let top = above - b.baseline;
+        for (r, row) in rows.iter_mut().enumerate() {
+            match r.checked_sub(top).and_then(|i| b.rows.get(i)) {
+                Some(text) => row.push_str(text),
+                None => row.extend(std::iter::repeat_n(' ', b.width)),
+            }
+        }
+    }
+    Some(Block {
+        rows,
+        width,
+        baseline: above,
+    })
+}
+
+/// `parts` one above the other, each centred (an odd slack leaves the extra
+/// column on the right); the baseline is that of `parts[base]`.
+fn stack(parts: &[Block], base: usize) -> Option<Block> {
+    let width = parts.iter().map(|b| b.width).max().unwrap_or(0);
+    let height = parts.iter().map(Block::height).sum();
+    fits(width, height)?;
+    let mut rows = Vec::with_capacity(height);
+    let mut baseline = 0;
+    for (i, b) in parts.iter().enumerate() {
+        if i == base {
+            baseline = rows.len() + b.baseline;
+        }
+        let left = (width - b.width) / 2;
+        for row in &b.rows {
+            rows.push(padded(" ".repeat(left) + row, width));
+        }
+    }
+    Some(Block {
+        rows,
+        width,
+        baseline,
+    })
+}
+
+/// `rows` one above the other, left-aligned, with the baseline at the
+/// middle row (the upper one of two): matrices, cases, aligned rows.
+fn column(rows: &[Block]) -> Option<Block> {
+    let width = rows.iter().map(|b| b.width).max().unwrap_or(0);
+    let height: usize = rows.iter().map(Block::height).sum();
+    fits(width, height)?;
+    let rows: Vec<String> = rows
+        .iter()
+        .flat_map(|b| b.rows.iter().map(|r| padded(r.clone(), width)))
+        .collect();
+    Some(Block {
+        baseline: (rows.len().max(1) - 1) / 2,
+        rows,
+        width,
+    })
+}
+
+/// `b` in parentheses one row tall, on its baseline.
+fn paren(b: Block) -> Option<Block> {
+    hcat(&[Block::text("("), b, Block::text(")")])
+}
+
+/// The [`Laid`] of `b`: rows without trailing blanks, without blank rows at
+/// either end and without the blank columns all rows start with.
+fn trimmed(b: Block) -> Option<Laid> {
+    let rows: Vec<&str> = b.rows.iter().map(|r| r.trim_end()).collect();
+    let first = rows.iter().position(|r| !r.is_empty())?;
+    let last = rows.iter().rposition(|r| !r.is_empty())?;
+    let indent = rows[first..=last]
+        .iter()
+        .filter(|r| !r.is_empty())
+        .map(|r| r.len() - r.trim_start_matches(' ').len())
+        .min()
+        .unwrap_or(0);
+    Some(Laid {
+        rows: rows[first..=last]
+            .iter()
+            .map(|r| r.get(indent..).unwrap_or("").to_owned())
+            .collect(),
+        baseline: b.baseline.clamp(first, last) - first,
+    })
+}
+
+// ── Layout ────────────────────────────────────────────────────────────────────
+
+/// The box of `node` in `style`.
+fn lay(node: &EqNode, style: Style) -> Option<Block> {
+    match node {
+        EqNode::Text(s) => Some(text(s, style)),
+        EqNode::Space(_) => Some(Block::empty()),
+        EqNode::Seq(children) => seq(children, style),
+        EqNode::Sup(base, sup) => scripts(base, Some(sup), None, style),
+        EqNode::Sub(base, sub) => scripts(base, None, Some(sub), style),
+        EqNode::SupSub(base, sup, sub) => scripts(base, Some(sup), Some(sub), style),
+        EqNode::Frac(num, den) => frac(num, den, style),
+        EqNode::Sqrt(body) => {
+            let inner = lay(body, style.inner())?;
+            let inner = if atomic(&inner) { inner } else { paren(inner)? };
+            hcat(&[Block::text("√"), inner])
+        }
+        EqNode::BigOp {
+            symbol,
+            lower,
+            upper,
+        } => {
+            let lower = limit_box(lower.as_deref())?;
+            let upper = limit_box(upper.as_deref())?;
+            let sym = Block::text(symbol);
+            if style == Style::Display && LIMITS_ABOVE.contains(&symbol.as_str()) {
+                over_under(sym, upper, lower)
+            } else {
+                attach(sym, upper, lower)
+            }
+        }
+        EqNode::Limit { name, lower } => {
+            let lower = limit_box(lower.as_deref())?;
+            let name = Block::text(name);
+            if style == Style::Display && name.rows[0].starts_with("lim") {
+                over_under(name, None, lower)
+            } else {
+                attach(name, None, lower)
+            }
+        }
+        EqNode::Accent(body, kind) => accent(lay(body, style)?, *kind),
+        EqNode::TextBlock(s) => Some(Block::text(s)),
+        EqNode::MathFont { kind, content } => {
+            Some(lay(content, style)?.map(|c| mathfont::map_char(*kind, c)))
+        }
+        EqNode::Delimited {
+            left,
+            right,
+            content,
+        } => delimited(left, right, lay(content, style)?),
+        EqNode::Matrix { kind, rows } => matrix(*kind, rows, style),
+        EqNode::Cases { rows } => cases(rows, style),
+        EqNode::Binom(top, bottom) => binom(top, bottom, style),
+        EqNode::Brace {
+            content,
+            label,
+            over,
+        } => annotate(content, label.as_deref(), *over, style),
+        EqNode::StackRel {
+            base,
+            annotation,
+            over,
+        } => annotate(base, Some(annotation), *over, style),
+    }
+}
+
+/// A text node: as it is, without whitespace in script style. Whitespace
+/// alone is nothing (runs of it are [`seq`]'s).
+fn text(s: &str, style: Style) -> Block {
+    if s.trim().is_empty() {
+        Block::empty()
+    } else if style == Style::Script {
+        Block::text(&s.chars().filter(|c| !c.is_whitespace()).collect::<String>())
+    } else {
+        Block::text(s)
+    }
+}
+
+/// A sequence: its parts side by side. Outside script style a run of
+/// spacing is one space (two for `\quad` and wider, none for a lone thin
+/// space), never at either end; an operator name is followed by a space
+/// unless a bracket follows it; and the operators of [`SPACED`] get one
+/// space each side.
+fn seq(children: &[EqNode], style: Style) -> Option<Block> {
+    let mut nodes = Vec::new();
+    flatten(children, &mut nodes);
+    let mut parts: Vec<Block> = Vec::new();
+    let mut gap = Gap::default();
+    let mut after_name = false;
+    for node in nodes {
+        if let Some(points) = spacing(node) {
+            gap.add(points);
+            continue;
+        }
+        let spaced = matches!(node, EqNode::Text(s) if SPACED.contains(&s.as_str()));
+        if spaced {
+            gap.add(None);
+        }
+        if style != Style::Script && !parts.is_empty() {
+            let mut spaces = gap.columns();
+            if after_name && spaces == 0 && !opens_bracket(node) {
+                spaces = 1;
+            }
+            if spaces > 0 {
+                parts.push(Block::text(&" ".repeat(spaces)));
+            }
+        }
+        gap = Gap::default();
+        if spaced {
+            gap.add(None);
+        }
+        parts.push(lay(node, style)?);
+        after_name = is_name(node);
+    }
+    hcat(&parts)
+}
+
+/// A run of spacing nodes between two parts of a sequence.
+#[derive(Default)]
+struct Gap {
+    /// A source space, or a space of 2 points or more, was seen...
+    one: bool,
+    /// ...or one of 12 points or more (`\quad`).
+    two: bool,
+}
+
+impl Gap {
+    /// Add a source space (`None`) or a `Space(points)`.
+    fn add(&mut self, points: Option<f32>) {
+        match points {
+            None => self.one = true,
+            Some(p) if p >= 12.0 => self.two = true,
+            Some(p) if p >= 2.0 => self.one = true,
+            Some(_) => {}
+        }
+    }
+
+    fn columns(&self) -> usize {
+        if self.two {
+            2
+        } else {
+            usize::from(self.one)
+        }
+    }
+}
+
+/// `Some` for a spacing node: `None` inside for a source space, the points
+/// of a `Space`.
+fn spacing(node: &EqNode) -> Option<Option<f32>> {
+    match node {
+        EqNode::Space(points) => Some(Some(*points)),
+        EqNode::Text(s) if s.chars().all(char::is_whitespace) => Some(None),
+        _ => None,
+    }
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
+/// Whether `node` is an operator name (`\sin`, `\log`), with or without
+/// scripts.
+fn is_name(node: &EqNode) -> bool {
+    match node {
+        EqNode::Limit { .. } => true,
+        EqNode::Sup(base, _) | EqNode::Sub(base, _) | EqNode::SupSub(base, _, _) => {
+            matches!(**base, EqNode::Limit { .. })
+        }
+        _ => false,
+    }
+}
+
+/// Whether `node` starts with an opening bracket (`\sin(x)` keeps no space).
+fn opens_bracket(node: &EqNode) -> bool {
+    let first = match node {
+        EqNode::Text(s) => s.chars().next(),
+        EqNode::Delimited { left, .. } => left.chars().next(),
+        _ => None,
+    };
+    matches!(first, Some('(' | '[' | '{'))
+}
+
+/// The box of a script or limit: script style; empty is none.
+fn limit_box(node: Option<&EqNode>) -> Option<Option<Block>> {
+    match node {
+        None => Some(None),
+        Some(node) => {
+            let b = lay(node, Style::Script)?;
+            Some((b.width > 0).then_some(b))
+        }
+    }
+}
+
+/// A base with its scripts.
+fn scripts(
+    base: &EqNode,
+    sup: Option<&EqNode>,
+    sub: Option<&EqNode>,
+    style: Style,
+) -> Option<Block> {
+    let base = lay(base, style)?;
+    attach(base, limit_box(sup)?, limit_box(sub)?)
+}
+
+/// `base` with a superscript and a subscript (either may be missing). When
+/// every character of both has a script glyph, the glyphs follow the base
+/// (subscript first, `zᵢ²`): on its row, or on its top and bottom rows when
+/// it is taller. Otherwise both are rows of their own: the superscript
+/// right above the base's top, the subscript right below its bottom,
+/// left-aligned with each other.
+fn attach(base: Block, sup: Option<Block>, sub: Option<Block>) -> Option<Block> {
+    let sup_glyphs = sup.as_ref().map(|b| glyphs(b, superscript_char));
+    let sub_glyphs = sub.as_ref().map(|b| glyphs(b, subscript_char));
+    if sup_glyphs.as_ref().is_none_or(Option::is_some)
+        && sub_glyphs.as_ref().is_none_or(Option::is_some)
+    {
+        let (sup, sub) = (sup_glyphs.flatten(), sub_glyphs.flatten());
+        let (sup, sub) = (sup.unwrap_or_default(), sub.unwrap_or_default());
+        if base.height() == 1 {
+            let line = format!("{}{sub}{sup}", base.rows[0]);
+            return Some(Block::text(&line));
+        }
+        let mut rows = base.rows;
+        let last = rows.len() - 1;
+        rows[0].push_str(&sup);
+        rows[last].push_str(&sub);
+        let width = rows.iter().map(|r| r.width()).max().unwrap_or(0);
+        fits(width, rows.len())?;
+        let rows = rows.into_iter().map(|r| padded(r, width)).collect();
+        return Some(Block {
+            rows,
+            width,
+            baseline: base.baseline,
+        });
+    }
+    let above = sup.as_ref().map_or(0, Block::height);
+    let below = sub.as_ref().map_or(0, Block::height);
+    let script_width = sup.iter().chain(&sub).map(|b| b.width).max().unwrap_or(0);
+    let width = base.width + script_width;
+    let height = above + base.height() + below;
+    fits(width, height)?;
+    let blank = " ".repeat(base.width);
+    let mut rows = Vec::with_capacity(height);
+    for row in sup.iter().flat_map(|b| &b.rows) {
+        rows.push(padded(blank.clone() + row, width));
+    }
+    for row in &base.rows {
+        rows.push(padded(row.clone(), width));
+    }
+    for row in sub.iter().flat_map(|b| &b.rows) {
+        rows.push(padded(blank.clone() + row, width));
+    }
+    Some(Block {
+        rows,
+        width,
+        baseline: above + base.baseline,
+    })
+}
+
+/// `b` in script glyphs (`map`), when it is one row and every character
+/// has one.
+fn glyphs(b: &Block, map: fn(char) -> Option<char>) -> Option<String> {
+    let line = b.line()?;
+    (!line.is_empty())
+        .then(|| line.chars().map(map).collect())
+        .flatten()
+}
+
+/// `base` with limits centred above and below it.
+fn over_under(base: Block, upper: Option<Block>, lower: Option<Block>) -> Option<Block> {
+    let index = usize::from(upper.is_some());
+    let parts: Vec<Block> = upper.into_iter().chain([base]).chain(lower).collect();
+    stack(&parts, index)
+}
+
+/// A fraction: in display style the numerator over a bar over the
+/// denominator (both in text style), the bar as wide as the wider; else
+/// flat, `num/den`, with parentheses where the one-line form puts them.
+fn frac(num: &EqNode, den: &EqNode, style: Style) -> Option<Block> {
+    let (num, den) = (lay(num, style.inner())?, lay(den, style.inner())?);
+    if style == Style::Display {
+        let bar = Block::text(&"─".repeat(num.width.max(den.width).max(1)));
+        return stack(&[num, bar, den], 1);
+    }
+    let num = match num.line() {
+        Some(line) if !top_level(line, |c| c.is_whitespace() || c == '/') => num,
+        _ => paren(num)?,
+    };
+    let den = if atomic(&den) { den } else { paren(den)? };
+    hcat(&[num, Block::text("/"), den])
+}
+
+/// Whether `b` reads as one operand: one row that [`is_atom`].
+fn atomic(b: &Block) -> bool {
+    b.line().is_some_and(is_atom)
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
+pub(super) fn superscript_char(c: char) -> Option<char> {
+    let i = "0123456789+-=()abcdefghijklmnoprstuvwxyz".find(c)?;
+    "⁰¹²³⁴⁵⁶⁷⁸⁹⁺⁻⁼⁽⁾ᵃᵇᶜᵈᵉᶠᵍʰⁱʲᵏˡᵐⁿᵒᵖʳˢᵗᵘᵛʷˣʸᶻ".chars().nth(i)
+}
+
+pub(super) fn subscript_char(c: char) -> Option<char> {
+    let i = "0123456789+-=()aehijklmnoprstuvx".find(c)?;
+    "₀₁₂₃₄₅₆₇₈₉₊₋₌₍₎ₐₑₕᵢⱼₖₗₘₙₒₚᵣₛₜᵤᵥₓ".chars().nth(i)
+}
+
+/// `body` with an accent: a combining mark over a one-character body, else
+/// `hat(…)` and the like.
+fn accent(body: Block, kind: AccentKind) -> Option<Block> {
+    let (mark, name) = match kind {
+        AccentKind::Hat => ('\u{302}', "hat"),
+        AccentKind::Bar => ('\u{305}', "bar"),
+        AccentKind::Dot => ('\u{307}', "dot"),
+        AccentKind::DoubleDot => ('\u{308}', "ddot"),
+        AccentKind::Tilde => ('\u{303}', "tilde"),
+        AccentKind::Vec => ('\u{20d7}', "vec"),
+    };
+    match body.line().map(str::trim) {
+        Some(one) if one.chars().count() == 1 => Some(Block::text(&format!("{one}{mark}"))),
+        _ => hcat(&[Block::text(&format!("{name}(")), body, Block::text(")")]),
+    }
+}
+
+/// `\left…\right`: the delimiters as they are around one row, else drawn
+/// as tall as `inner` (which keeps its baseline).
+fn delimited(left: &str, right: &str, inner: Block) -> Option<Block> {
+    if inner.height() == 1 {
+        return hcat(&[Block::text(left), inner, Block::text(right)]);
+    }
+    let (l, r) = (
+        fence(left, inner.height(), inner.baseline),
+        fence(right, inner.height(), inner.baseline),
+    );
+    hcat(&[l, inner, r])
+}
+
+/// Delimiter `d` drawn `n` rows tall (n > 1) with its baseline at
+/// `baseline`: brackets from their pieces, bars repeated, anything else
+/// once on the middle row (the upper one of two).
+fn fence(d: &str, n: usize, baseline: usize) -> Block {
+    let (last, middle) = (n - 1, (n - 1) / 2);
+    let rows = (0..n)
+        .map(|r| {
+            let pick = |top: &str, ext: &str, bottom: &str| match r {
+                0 => top.to_owned(),
+                r if r == last => bottom.to_owned(),
+                _ => ext.to_owned(),
+            };
+            match d {
+                "(" => pick("⎛", "⎜", "⎝"),
+                ")" => pick("⎞", "⎟", "⎠"),
+                "[" => pick("⎡", "⎢", "⎣"),
+                "]" => pick("⎤", "⎥", "⎦"),
+                "{" if n == 2 => pick("⎰", "", "⎱"),
+                "}" if n == 2 => pick("⎱", "", "⎰"),
+                "{" if r == middle && r > 0 => "⎨".to_owned(),
+                "}" if r == middle && r > 0 => "⎬".to_owned(),
+                "{" => pick("⎧", "⎪", "⎩"),
+                "}" => pick("⎫", "⎪", "⎭"),
+                "|" => "│".to_owned(),
+                "⌊" | "⌋" if r < last => "│".to_owned(),
+                "⌈" | "⌉" if r > 0 => "│".to_owned(),
+                "‖" | "⌊" | "⌋" | "⌈" | "⌉" => d.to_owned(),
+                _ if r == middle => d.to_owned(),
+                _ => " ".repeat(d.width()),
+            }
+        })
+        .collect();
+    Block {
+        rows,
+        width: d.width(),
+        baseline,
+    }
+}
+
+/// A matrix: its cells in columns two spaces apart, each centred, between
+/// the kind's brackets (plain ones around a one-row matrix).
+fn matrix(kind: MatrixKind, rows: &[Vec<EqNode>], style: Style) -> Option<Block> {
+    let grid: Vec<Vec<Block>> = rows
+        .iter()
+        .map(|row| row.iter().map(|cell| lay(cell, style)).collect())
+        .collect::<Option<_>>()?;
+    let grid: Vec<Vec<Block>> = grid
+        .into_iter()
+        .filter(|row| row.iter().any(|b| b.width > 0))
+        .collect();
+    let columns = grid.iter().map(Vec::len).max().unwrap_or(0);
+    let widths: Vec<usize> = (0..columns)
+        .map(|c| {
+            grid.iter()
+                .filter_map(|row| row.get(c))
+                .map(|b| b.width)
+                .max()
+                .unwrap_or(0)
+        })
+        .collect();
+    let lines = grid
+        .iter()
+        .map(|row| {
+            let mut parts = Vec::new();
+            for (c, cell) in row.iter().enumerate() {
+                if c > 0 {
+                    parts.push(Block::text("  "));
+                }
+                let slack = widths[c] - cell.width;
+                parts.push(Block::text(&" ".repeat(slack / 2)));
+                parts.push(cell.clone());
+                parts.push(Block::text(&" ".repeat(slack - slack / 2)));
+            }
+            hcat(&parts)
+        })
+        .collect::<Option<Vec<_>>>()?;
+    let body = column(&lines)?;
+    let (left, right) = match kind {
+        MatrixKind::Plain => ("", ""),
+        MatrixKind::Paren => ("(", ")"),
+        MatrixKind::Bracket => ("[", "]"),
+        MatrixKind::VBar => ("|", "|"),
+        MatrixKind::DoubleVBar => ("‖", "‖"),
+        MatrixKind::Brace => ("{", "}"),
+    };
+    around(left, right, body)
+}
+
+/// `body` between `left` and `right`: plain around one row, else drawn as
+/// tall as it.
+fn around(left: &str, right: &str, body: Block) -> Option<Block> {
+    if body.height() == 1 {
+        return hcat(&[Block::text(left), body, Block::text(right)]);
+    }
+    let fence_of = |d: &str| {
+        if d.is_empty() {
+            Block {
+                rows: vec![String::new(); body.height()],
+                width: 0,
+                baseline: body.baseline,
+            }
+        } else {
+            fence(d, body.height(), body.baseline)
+        }
+    };
+    hcat(&[fence_of(left), body.clone(), fence_of(right)])
+}
+
+/// `cases`: values in one column, conditions two spaces after the widest,
+/// behind a brace (no `if`).
+fn cases(rows: &[(EqNode, Option<EqNode>)], style: Style) -> Option<Block> {
+    let laid: Vec<(Block, Option<Block>)> = rows
+        .iter()
+        .map(|(value, condition)| {
+            let condition = match condition {
+                Some(c) => Some(lay(c, style)?),
+                None => None,
+            };
+            Some((lay(value, style)?, condition))
+        })
+        .collect::<Option<_>>()?;
+    let width = laid.iter().map(|(v, _)| v.width).max().unwrap_or(0);
+    let lines = laid
+        .into_iter()
+        .map(|(value, condition)| match condition {
+            Some(c) => {
+                let gap = Block::text(&" ".repeat(width - value.width + 2));
+                hcat(&[value, gap, c])
+            }
+            None => Some(value),
+        })
+        .collect::<Option<Vec<_>>>()?;
+    around("{", "", column(&lines)?)
+}
+
+/// `\binom`: `(n choose k)` on one line, or stacked in parentheses in
+/// display style.
+fn binom(top: &EqNode, bottom: &EqNode, style: Style) -> Option<Block> {
+    let (top, bottom) = (lay(top, style.inner())?, lay(bottom, style.inner())?);
+    if style == Style::Display {
+        let pair = stack(&[top, bottom], 0)?;
+        let pair = column(std::slice::from_ref(&pair))?;
+        return around("(", ")", pair);
+    }
+    hcat(&[
+        Block::text("("),
+        top,
+        Block::text(" choose "),
+        bottom,
+        Block::text(")"),
+    ])
+}
+
+/// `\overbrace`, `\underbrace`, `\overset`, `\underset`: the annotation in
+/// script style, centred above (`over`) or below the content.
+fn annotate(content: &EqNode, label: Option<&EqNode>, over: bool, style: Style) -> Option<Block> {
+    let content = lay(content, style)?;
+    match limit_box(label)? {
+        None => Some(content),
+        Some(label) if over => stack(&[label, content], 1),
+        Some(label) => stack(&[content, label], 0),
+    }
+}
+
 #[cfg(test)]
 mod tests {
     use super::*;
`````
Run: `umask 022; cargo test --locked --bin claudio typeset && cargo build --locked 2>&1 | grep -cE '^(warning|error)'; rustfmt --edition 2021 --check src/tui/typeset/mod.rs src/tui/typeset/mathfont.rs && make test-unit`
Expected: `test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 837 filtered out`, then `0`, then `test result: ok. 856 passed; 0 failed`.

Step 3: commit
`/usr/bin/git add src/tui/typeset/mod.rs src/tui/typeset/mathfont.rs && /usr/bin/git commit -m "feat(tui): compact 2d typesetting engine for formulas" -- src/tui/mod.rs src/tui/typeset/mod.rs src/tui/typeset/mathfont.rs`

Interfaces this task provides to later tasks: `tui::typeset::{Laid { pub rows: Vec<String>, pub baseline: usize }, Laid::width(), typeset(&EqNode, display: bool) -> Option<Laid>, rewrite_roots(&str) -> String}`; private helpers `superscript_char`, `subscript_char`, `is_atom`, `top_level`, `flatten` (used by Task 3's `walk.rs`).

### Task 3: `render` on the engine; term-maths removed
Model: sonnet   Repo: /volumes/repos/claudio   Files: Cargo.toml, Cargo.lock, src/tui/latex.rs, src/tui/formulas.rs, src/tui/mod.rs, src/tui/typeset/mod.rs, src/tui/typeset/walk.rs (new), src/tui/ui/formulas.rs, src/tui/ui/mod.rs
Goal: AC5, the `aligned` golden of AC4 (F4), F6, the term-maths part of AC12, and AC7's v0.4.0 hostile corpus (its existing test, now on the new engine). `latex::render` returns `Rendered { inline, block, text }`: the display-style and text-style layouts (≤ 200 × 40, sanitized) and the one-line form, which is the text-style layout when it is one row and else the v0.4.0 walk, moved unchanged into `typeset/walk.rs` (its `\mathbb` now goes through the ported tables). A formula that is one `aligned`/`align`/`align*`/`gather`/`gather*`/`gathered`/`array` environment is split on `\\` (outside braces and nested environments), `&` dropped, each row rendered and the rows stacked left-aligned; any other environment the parser does not know (or a rows environment inside a larger formula) is not rendered (shown as source). Each rewritten part is pre-checked again. `Formula` gains `text: Option<Laid>` and its `block` becomes a `Laid`; the formulas popup and its tests follow. The AC5 table test is unchanged; its helper reads `.inline` instead of `.0`. `the_block_is_term_maths_rendering` is replaced by `the_renderings_are_the_engines`.

Step 1: write the failing tests.
`````diff
diff --git a/src/tui/formulas.rs b/src/tui/formulas.rs
index 62ffee7..a06889f 100644
--- a/src/tui/formulas.rs
+++ b/src/tui/formulas.rs
@@ -123,7 +123,8 @@ pub fn lines(v: &SessionView) -> Vec<Line> {
 mod tests {
     use super::*;
     use crate::proto::SessionKind;
-    use crate::tui::latex::Formula;
+    use crate::tui::latex::{Formula, Rendered};
+    use crate::tui::typeset::Laid;
     use crossterm::event::KeyEvent;
 
     fn view_with(srcs: &[&str], rendered: Vec<Formula>) -> SessionView {
@@ -141,11 +142,22 @@ mod tests {
 
     #[test]
     fn each_formula_shows_its_rendering_then_its_source() {
+        let block = Laid {
+            rows: vec![" a".into(), "───".into(), " b".into()],
+            baseline: 1,
+        };
         let frac = Formula::new(
             "\\frac{a}{b}",
-            (None, Some(vec![" a".into(), "───".into(), " b".into()])),
+            Rendered {
+                block: Some(block),
+                ..Rendered::default()
+            },
         );
-        let one = Formula::new("x^2", (Some("x²".into()), None));
+        let inline = Rendered {
+            inline: Some("x²".into()),
+            ..Rendered::default()
+        };
+        let one = Formula::new("x^2", inline);
         let v = view_with(&["\\frac{a}{b}", "x^2", "y\n+ 1"], vec![frac, one]);
         assert_eq!(
             lines(&v),
diff --git a/src/tui/latex.rs b/src/tui/latex.rs
index 2ea59ae..c0dcc40 100644
--- a/src/tui/latex.rs
+++ b/src/tui/latex.rs
@@ -807,7 +581,7 @@ mod tests {
     /// The one-line rendering of `src`.
     fn inline(src: &str) -> String {
         render(&prepare(src))
-            .0
+            .inline
             .unwrap_or_else(|| panic!("no inline for {src}"))
     }
 
@@ -848,27 +622,57 @@ mod tests {
         }
     }
 
+    /// The rows of a rendering.
+    fn rows(laid: Option<Laid>) -> Vec<String> {
+        laid.expect("rendered").rows
+    }
+
     #[test]
-    fn the_block_is_term_maths_rendering() {
-        let (_, block) = render(&prepare("\\frac{a}{b}"));
-        assert_eq!(block.unwrap(), [" a", "───", " b"]);
-        let (inline, block) = render(&prepare("E = mc^2"));
-        assert_eq!(block.unwrap(), ["E = mc²"]);
+    fn the_renderings_are_the_engines() {
+        let r = render(&prepare("\\frac{a}{b}"));
+        assert_eq!(rows(r.block), ["a", "─", "b"]);
+        assert_eq!(rows(r.text), ["a/b"]);
+        assert_eq!(r.inline.as_deref(), Some("a/b"));
+        let r = render(&prepare("E = mc^2"));
+        assert_eq!(rows(r.block), ["E = mc²"]);
+        // Taller than one row in text style: the one-line walk.
+        let r = render(&prepare("e^{i\\pi}"));
+        assert_eq!(rows(r.text), [" iπ", "e"]);
+        assert_eq!(r.inline.as_deref(), Some("e^(iπ)"));
         assert_eq!(
-            inline.as_deref(),
-            Some("E = mc²"),
-            "one row is its own inline"
+            render(&prepare("\\sqrt[3]{x}")).inline.as_deref(),
+            Some("³√x")
         );
     }
 
+    #[test]
+    fn aligned_rows_are_rendered_one_by_one() {
+        let r = render(&prepare(
+            "\\begin{aligned} a &= b \\\\ c &= d \\end{aligned}",
+        ));
+        assert_eq!(rows(r.block), ["a = b", "c = d"]);
+        assert_eq!(r.inline.as_deref(), Some("a = b; c = d"));
+        // An environment the parser does not know is not rendered at all.
+        assert_eq!(
+            render(&prepare("\\begin{tabular}{c} a \\end{tabular}")),
+            Rendered::default()
+        );
+        // A rewritten root counts against the pre-check too: its index
+        // gains a brace.
+        let deep = format!("\\sqrt[{}x{}]{{y}}", "{".repeat(32), "}".repeat(32));
+        assert!(!too_complex(&deep));
+        assert_eq!(render(&deep), Rendered::default());
+    }
+
     #[test]
     fn renderings_are_sanitized() {
         // A bidi override and a zero-width space never reach a cell.
         let src = prepare("a\u{202e}b\u{200b} + c");
         assert_eq!(src, "ab + c");
-        let (inline, block) = render("\\text{a\u{202e}b}");
-        assert_eq!(inline.as_deref(), Some("ab"));
-        assert_eq!(block.unwrap(), ["ab"]);
+        let r = render("\\text{a\u{202e}b}");
+        assert_eq!(r.inline.as_deref(), Some("ab"));
+        assert_eq!(rows(r.block), ["ab"]);
+        assert_eq!(rows(r.text), ["ab"]);
         assert_eq!(prepare("a\nb\tc"), "a b c");
     }
 
@@ -949,7 +753,11 @@ mod tests {
 
     /// A formula `src` whose one-line rendering is `inline`.
     fn formula(src: &str, inline: &str) -> Formula {
-        Formula::new(src, (Some(inline.to_owned()), None))
+        let rendered = Rendered {
+            inline: Some(inline.to_owned()),
+            ..Rendered::default()
+        };
+        Formula::new(src, rendered)
     }
 
     #[test]
diff --git a/src/tui/mod.rs b/src/tui/mod.rs
index ae76f3b..4fa8fd2 100644
--- a/src/tui/mod.rs
+++ b/src/tui/mod.rs
@@ -1375,7 +1373,8 @@ mod tests {
             let took = started.elapsed();
             let head: String = src.chars().take(16).collect();
             assert!(took < 2 * RENDER_BUDGET, "{took:?} for {head:?}…");
-            for text in formula.inline.iter().chain(formula.block.iter().flatten()) {
+            let laid = formula.block.iter().chain(&formula.text);
+            for text in formula.inline.iter().chain(laid.flat_map(|l| &l.rows)) {
                 assert_eq!(*text, crate::claude::latex::sanitize(text), "{head:?}");
                 assert!(!text.contains(['\u{1b}', '\u{9b}', '\u{202e}', '\u{200b}']));
             }
diff --git a/src/tui/typeset/mod.rs b/src/tui/typeset/mod.rs
index 821c18d..56a86b9 100644
--- a/src/tui/typeset/mod.rs
+++ b/src/tui/typeset/mod.rs
@@ -1062,6 +1239,51 @@ mod tests {
         assert_eq!(display("\\mathbf{v} + \\mathcal{L}"), ["𝐯 + ℒ"]);
     }
 
+    #[test]
+    fn environments_of_rows_are_split_and_unknown_ones_refused() {
+        let rows = |v: &[&str]| Environment::Rows(v.iter().map(|r| r.to_string()).collect());
+        assert_eq!(
+            environment("\\begin{aligned} a &= b \\\\ c &= d \\end{aligned}"),
+            rows(&["a = b", "c = d"])
+        );
+        assert_eq!(
+            environment(" \\begin{array}{l|r} x & 1 \\\\[2pt] y & 2 \\\\ \\end{array} "),
+            rows(&["x  1", "y  2"])
+        );
+        // `&` and `\\` of a nested matrix stay with it.
+        assert_eq!(
+            environment("\\begin{gather} \\begin{pmatrix} a & b \\\\ c & d \\end{pmatrix} \\\\ e \\end{gather}"),
+            rows(&["\\begin{pmatrix} a & b \\\\ c & d \\end{pmatrix}", "e"])
+        );
+        assert_eq!(environment("\\{a\\} \\\\ b"), Environment::None);
+        assert_eq!(
+            environment("\\begin{pmatrix} a \\end{pmatrix}"),
+            Environment::None
+        );
+        for unknown in [
+            "\\begin{tabular}{c} a \\end{tabular}",
+            "x = \\begin{aligned} a \\end{aligned}",
+            "\\begin{aligned} \\begin{CD} a \\end{CD} \\end{aligned}",
+        ] {
+            assert_eq!(environment(unknown), Environment::Unknown, "{unknown}");
+        }
+    }
+
+    #[test]
+    fn rows_are_stacked_left_aligned() {
+        let laid = |rows: &[&str], baseline| Laid {
+            rows: rows.iter().map(|r| r.to_string()).collect(),
+            baseline,
+        };
+        let one = laid(&["a", "─", "b"], 1);
+        assert_eq!(stacked(vec![Some(one.clone())]), Some(one.clone()));
+        assert_eq!(
+            stacked(vec![Some(laid(&["x = 1"], 0)), Some(one.clone())]),
+            Some(laid(&["x = 1", "a", "─", "b"], 1))
+        );
+        assert_eq!(stacked(vec![Some(one), None]), None);
+    }
+
     #[test]
     fn layouts_past_the_limits_are_none() {
         let wide = "x + ".repeat(MAX_COLS / 4 + 1);
diff --git a/src/tui/ui/formulas.rs b/src/tui/ui/formulas.rs
index 2918c44..ec55d35 100644
--- a/src/tui/ui/formulas.rs
+++ b/src/tui/ui/formulas.rs
@@ -60,8 +60,9 @@ pub fn draw_formulas(frame: &mut Frame, app: &App, view: &FormulasView) {
 #[cfg(test)]
 mod tests {
     use crate::tui::app::{App, Modal};
-    use crate::tui::latex::Formula;
+    use crate::tui::latex::{Formula, Rendered};
     use crate::tui::test_support::{alt, app_with, info};
+    use crate::tui::typeset::Laid;
     use ratatui::{backend::TestBackend, Terminal};
 
     /// The screen's rows after drawing `app`.
@@ -79,10 +80,15 @@ mod tests {
         let mut app = app_with(&[info(Some(1), None)]);
         let v = &mut app.sessions[0];
         v.latex_srcs = (0..20).map(|i| format!("x_{i}")).collect();
-        v.latex = vec![Formula::new(
-            "x_19",
-            (Some("x₁₉".into()), Some(vec!["x₁₉".into()])),
-        )];
+        let rendered = Rendered {
+            inline: Some("x₁₉".into()),
+            block: Some(Laid {
+                rows: vec!["x₁₉".into()],
+                baseline: 0,
+            }),
+            text: None,
+        };
+        v.latex = vec![Formula::new("x_19", rendered)];
         app.on_terminal(alt('u'));
         assert!(matches!(app.modal, Some(Modal::Formulas(_))));
         let text = screen(&app).join("\n");
diff --git a/src/tui/ui/mod.rs b/src/tui/ui/mod.rs
index 053532c..0209de4 100644
--- a/src/tui/ui/mod.rs
+++ b/src/tui/ui/mod.rs
@@ -880,7 +880,10 @@ mod tests {
         )]);
         let v = &mut app.sessions[0];
         v.mirror.feed(b"energy: <latex>E = mc^2</latex>.");
-        let rendered = (Some("E = mc\u{b2}".to_owned()), None);
+        let rendered = crate::tui::latex::Rendered {
+            inline: Some("E = mc\u{b2}".to_owned()),
+            ..Default::default()
+        };
         v.latex = vec![crate::tui::latex::Formula::new("E = mc^2", rendered)];
         let mut term = Terminal::new(TestBackend::new(app.width, app.height)).unwrap();
         term.draw(|f| draw(f, &app)).unwrap();
`````
Run: `umask 022; cargo test --locked --bin claudio -- tui::latex tui::typeset tui::formulas tui::ui tui::tests`
Expected: compilation fails, `error: could not compile `claudio` (bin "claudio" test) due to 33 previous errors`: `E0609` no field `inline`/`block`/`text` on type `(Option<String>, Option<Vec<String>>)`, `E0071` expected struct, found that tuple type, `E0425`/`E0433` cannot find `environment`, `Environment`, `Laid`.

Step 2: implement. Cargo.lock loses term-maths (no new packages).
`````diff
diff --git a/Cargo.lock b/Cargo.lock
index 5a8b3aa..d2231e7 100644
--- a/Cargo.lock
+++ b/Cargo.lock
@@ -263,7 +263,6 @@ dependencies = [
  "serde",
  "serde_json",
  "sha2",
- "term-maths",
  "thiserror 2.0.18",
  "tokio",
  "tokio-stream",
@@ -2382,17 +2381,6 @@ dependencies = [
  "syn 3.0.6",
 ]
 
-[[package]]
-name = "term-maths"
-version = "1.0.0"
-source = "registry+https://github.com/rust-lang/crates.io-index"
-checksum = "26ccccf7b1e429ef47ef4a82726346feefdb67169e6a9d8d97fc6d3707640a05"
-dependencies = [
- "rust-latex-parser",
- "unicode-segmentation",
- "unicode-width",
-]
-
 [[package]]
 name = "termina"
 version = "0.3.3"
diff --git a/Cargo.toml b/Cargo.toml
index 1e249f2..9d2d365 100644
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -51,10 +51,9 @@ libc = "0.2"
 toml = "0.9"
 sha2 = "0.10"
 reqwest = { version = "0.12", default-features = false, features = ["rustls-tls", "json"] }
-# `<latex>` formulas in the session pane, rendered as Unicode text. Pure Rust;
-# term-maths' ratatui and crossterm features stay off. Pinned exactly: the
-# render guard's limits (tui/latex.rs) were measured against these versions.
-term-maths = { version = "=1.0.0", default-features = false }
+# `<latex>` formulas, parsed for our own layout engine (tui/typeset). Pure
+# Rust. Pinned exactly: the render guard's limits (tui/latex.rs) were
+# measured against this version.
 rust-latex-parser = "=0.1.1"
 unicode-segmentation = "1"
 unicode-width = "0.2"
diff --git a/src/tui/formulas.rs b/src/tui/formulas.rs
index 81aaa01..a06889f 100644
--- a/src/tui/formulas.rs
+++ b/src/tui/formulas.rs
@@ -109,7 +109,7 @@ pub fn lines(v: &SessionView) -> Vec<Line> {
         let shown = v.latex.iter().find(|f| f.src == *src);
         match shown.map(|f| (&f.block, &f.inline)) {
             Some((Some(block), _)) => {
-                lines.extend(block.iter().cloned().map(Line::Formula));
+                lines.extend(block.rows.iter().cloned().map(Line::Formula));
             }
             Some((None, Some(inline))) => lines.push(Line::Formula(inline.clone())),
             _ => lines.push(Line::Formula(source.clone())),
diff --git a/src/tui/latex.rs b/src/tui/latex.rs
index 9cc3bde..c0dcc40 100644
--- a/src/tui/latex.rs
+++ b/src/tui/latex.rs
@@ -8,17 +8,17 @@
 //! [`too_complex`] has turned away what could overflow that thread's stack.
 //!
 //! - [`Formula`] — one formula's source, its renderings and matching keys.
-//! - [`render`] — the 2D block (term-maths) and the one-line form.
+//! - [`render`] — the 2D renderings (`typeset.rs`) and the one-line form.
 //! - [`too_complex`] — the pre-check that runs before any parsing.
 //! - [`overlay`] — the pane's `<latex>` tags replaced by one-line renderings.
 
 use ratatui::buffer::{Buffer, Cell};
 use ratatui::layout::Rect;
 use ratatui::style::{Color, Style};
-use rust_latex_parser::{AccentKind, EqNode, MatrixKind};
 use unicode_segmentation::UnicodeSegmentation;
 use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
 
+use super::typeset::{self, Environment, Laid};
 use crate::claude::latex::{is_unsafe, sanitize, MAX_FORMULA_BYTES};
 use crate::term::screen::Screen;
 
@@ -28,7 +28,7 @@ const MAX_DEPTH: usize = 32;
 /// `\command` (`\left`, `\begin` included), every `/`, and every `(` right
 /// after a bareword (`sqrt(`, `frac(`). The quadratic formula has about 12.
 const MAX_TOKENS: usize = 256;
-/// The largest 2D rendering shown in the formulas popup.
+/// The largest 2D rendering kept.
 const BLOCK_MAX_COLS: usize = 200;
 const BLOCK_MAX_ROWS: usize = 40;
 /// The widest one-line rendering put in the pane.
@@ -42,8 +42,11 @@ pub struct Formula {
     /// The one-line rendering that replaces the tag in the pane; `None`
     /// leaves the tag raw.
     pub inline: Option<String>,
-    /// The 2D rendering shown in the formulas popup, one string per row.
-    pub block: Option<Vec<String>>,
+    /// The display-style rendering: the reader's display formulas and its
+    /// formulas list.
+    pub block: Option<Laid>,
+    /// The text-style rendering: the reader's formulas inside a line.
+    pub text: Option<Laid>,
     /// The matching keys of the on-screen tag text (see `overlay`):
     /// whitespace dropped, after undoing markdown's backslash escapes...
     pub key_strict: String,
@@ -55,12 +58,13 @@ pub struct Formula {
 
 impl Formula {
     /// `src` with its renderings (see [`render`]); the keys come from `src`.
-    pub fn new(src: &str, (inline, block): Rendered) -> Formula {
+    pub fn new(src: &str, rendered: Rendered) -> Formula {
         let unescaped = md_unescape(src);
         Formula {
             src: src.to_owned(),
-            inline,
-            block,
+            inline: rendered.inline,
+            block: rendered.block,
+            text: rendered.text,
             key_strict: normalize(&unescaped),
             key_loose: normalize_loose(&unescaped),
             key_html: normalize_loose(&without_html(&unescaped)),
@@ -70,12 +74,21 @@ impl Formula {
     /// `src` shown as it is: not rendered (too complex, too slow, or the
     /// renderer failed).
     pub fn raw(src: &str) -> Formula {
-        Formula::new(src, (None, None))
+        Formula::new(src, Rendered::default())
     }
 }
 
-/// What [`render`] makes of a formula: its one-line and 2D renderings.
-pub type Rendered = (Option<String>, Option<Vec<String>>);
+/// What [`render`] makes of a formula; `None` for what does not fit or
+/// comes out empty.
+#[derive(Debug, Clone, Default, PartialEq)]
+pub struct Rendered {
+    /// One line, at most [`INLINE_MAX_COLS`] wide.
+    pub inline: Option<String>,
+    /// Display style, at most [`BLOCK_MAX_COLS`] × [`BLOCK_MAX_ROWS`].
+    pub block: Option<Laid>,
+    /// Text style, within the same limits.
+    pub text: Option<Laid>,
+}
 
 /// `src` as the renderer gets it: [`sanitize`]d, on one line.
 pub fn prepare(src: &str) -> String {
@@ -120,294 +133,55 @@ pub fn too_complex(src: &str) -> bool {
     deepest > MAX_DEPTH || tokens > MAX_TOKENS
 }
 
-/// Render a [`prepare`]d formula: the 2D block term-maths lays out (at most
-/// [`BLOCK_MAX_COLS`] × [`BLOCK_MAX_ROWS`]), and the one-line form, which is
-/// that block when it is one row and otherwise our own one-line walk of the
-/// parse tree (at most [`INLINE_MAX_COLS`] wide). Either is `None` when it
-/// does not fit or comes out empty. Both are [`sanitize`]d.
+/// Render a [`prepare`]d formula with the typesetting engine: in display
+/// style, in text style, and on one line, which is the text style when that
+/// is one row and the v0.4.0 walk of the parse tree otherwise. A formula
+/// made of an `aligned`-like environment is rendered row by row; one with
+/// an environment the parser does not know, or past [`too_complex`] once
+/// `\sqrt[n]` is rewritten, is not rendered. Everything is [`sanitize`]d.
 ///
 /// May be slow, deep or panic on hostile input: call it only through the
 /// guard in `tui/mod.rs`, after [`too_complex`].
 pub fn render(src: &str) -> Rendered {
-    let tree = rust_latex_parser::parse_equation(src);
-    let laid_out = term_maths::layout::layout(&tree);
-    let rows: Vec<String> = laid_out
-        .cells()
-        .iter()
-        .map(|row| sanitize(&row.concat()).trim_end().to_owned())
-        .collect();
-    let line = match rows.as_slice() {
-        [one] => one.trim().to_owned(),
-        _ => sanitize(&one_line(&tree)).trim().to_owned(),
+    let parts = match typeset::environment(src) {
+        Environment::None => vec![src.to_owned()],
+        Environment::Rows(rows) => rows,
+        Environment::Unknown => return Rendered::default(),
     };
-    let inline = (!line.is_empty() && line.width() <= INLINE_MAX_COLS).then_some(line);
-    let fits = laid_out.width() <= BLOCK_MAX_COLS && rows.len() <= BLOCK_MAX_ROWS;
-    let block = (fits && rows.iter().any(|r| !r.trim().is_empty())).then_some(rows);
-    (inline, block)
-}
-
-// ── One-line walk ─────────────────────────────────────────────────────────────
-
-/// One line of text for `node`, modelled on term-maths' layout: fractions
-/// become `a/b`, scripts Unicode super- and subscripts (else `^(…)` and
-/// `_(…)`), roots `√x`, matrices `[a b; c d]`, cases `{ 1 if x > 0; 0
-/// otherwise }`. Symbols are already Unicode in the parse tree.
-fn one_line(node: &EqNode) -> String {
-    match node {
-        EqNode::Text(s) => s.clone(),
-        EqNode::Space(points) => {
-            if *points < 2.0 {
-                String::new()
-            } else {
-                " ".to_owned()
-            }
-        }
-        EqNode::Seq(children) => seq(children),
-        EqNode::Frac(num, den) => {
-            let (num, den) = (one_line(num), one_line(den));
-            let num = if top_level(&num, |c| c.is_whitespace() || c == '/') {
-                format!("({num})")
-            } else {
-                num
-            };
-            format!("{num}/{}", operand(den))
-        }
-        EqNode::Sup(base, sup) => format!("{}{}", operand(one_line(base)), superscript(sup)),
-        EqNode::Sub(base, sub) => format!("{}{}", operand(one_line(base)), subscript(sub)),
-        EqNode::SupSub(base, sup, sub) => format!(
-            "{}{}{}",
-            operand(one_line(base)),
-            subscript(sub),
-            superscript(sup)
-        ),
-        EqNode::Sqrt(body) => format!("√{}", operand(one_line(body))),
-        EqNode::BigOp {
-            symbol,
-            lower,
-            upper,
-        } => {
-            let lower = lower.as_deref().map(subscript).unwrap_or_default();
-            let upper = upper.as_deref().map(superscript).unwrap_or_default();
-            format!("{symbol}{lower}{upper}")
-        }
-        EqNode::Accent(body, kind) => accent(&one_line(body), *kind),
-        EqNode::Limit { name, lower } => {
-            let lower = lower.as_deref().map(subscript).unwrap_or_default();
-            format!("{name}{lower}")
-        }
-        EqNode::TextBlock(s) => s.clone(),
-        EqNode::MathFont { kind, content } => {
-            term_maths::mathfont::map_str(kind, &one_line(content))
-        }
-        EqNode::Delimited {
-            left,
-            right,
-            content,
-        } => format!("{left}{}{right}", one_line(content).trim()),
-        EqNode::Matrix { kind, rows } => {
-            let rows: Vec<String> = rows
-                .iter()
-                .map(|row| {
-                    let cells: Vec<String> =
-                        row.iter().map(|c| one_line(c).trim().to_owned()).collect();
-                    cells.join(" ")
-                })
-                .collect();
-            let (open, close) = match kind {
-                MatrixKind::VBar => ("|", "|"),
-                MatrixKind::DoubleVBar => ("‖", "‖"),
-                _ => ("[", "]"),
-            };
-            format!("{open}{}{close}", rows.join("; "))
-        }
-        EqNode::Cases { rows } => {
-            let rows: Vec<String> = rows
-                .iter()
-                .map(|(value, condition)| {
-                    let value = one_line(value).trim().to_owned();
-                    match condition.as_ref().map(|c| one_line(c).trim().to_owned()) {
-                        None => value,
-                        Some(c) if starts_with_word(&c) => format!("{value} {c}"),
-                        Some(c) => format!("{value} if {c}"),
-                    }
-                })
-                .collect();
-            format!("{{ {} }}", rows.join("; "))
-        }
-        EqNode::Binom(top, bottom) => {
-            format!(
-                "({} choose {})",
-                one_line(top).trim(),
-                one_line(bottom).trim()
-            )
-        }
-        EqNode::Brace {
-            content,
-            label,
-            over,
-        } => {
-            let label = match (label.as_deref(), over) {
-                (Some(l), true) => superscript(l),
-                (Some(l), false) => subscript(l),
-                (None, _) => String::new(),
-            };
-            format!("{}{label}", one_line(content))
-        }
-        EqNode::StackRel {
-            base,
-            annotation,
-            over,
-        } => {
-            let annotation = if *over {
-                superscript(annotation)
-            } else {
-                subscript(annotation)
-            };
-            format!("{}{annotation}", one_line(base))
-        }
-    }
-}
-
-/// A sequence: its parts side by side, every run of spaces one space, none
-/// at the ends.
-fn seq(children: &[EqNode]) -> String {
-    let mut out = String::new();
-    let mut space = false;
-    let mut parts = Vec::new();
-    flatten(children, &mut parts);
+    let (mut blocks, mut texts, mut lines) = (Vec::new(), Vec::new(), Vec::new());
     for part in parts {
-        let spacing = match part {
-            EqNode::Space(_) => true,
-            EqNode::Text(s) => s.chars().all(char::is_whitespace),
-            _ => false,
-        };
-        if spacing {
-            space = true;
-            continue;
-        }
-        if space && !out.is_empty() {
-            out.push(' ');
-        }
-        space = false;
-        out.push_str(&one_line(part));
-    }
-    out
-}
-
-fn flatten<'a>(children: &'a [EqNode], out: &mut Vec<&'a EqNode>) {
-    for child in children {
-        match child {
-            EqNode::Seq(inner) => flatten(inner, out),
-            other => out.push(other),
-        }
-    }
-}
-
-/// `s` as an operand: as it is when it reads as one (see [`is_atom`]),
-/// else in parentheses.
-fn operand(s: String) -> String {
-    if is_atom(&s) {
-        s
-    } else {
-        format!("({s})")
-    }
-}
-
-/// One operand without parentheses: a number, one symbol (with scripts or a
-/// root sign), or something already in brackets.
-fn is_atom(s: &str) -> bool {
-    let s = s.strip_prefix('√').unwrap_or(s);
-    let mut chars = s.chars();
-    let Some(first) = chars.next() else {
-        return true;
-    };
-    s.chars().all(|c| c.is_ascii_digit() || c == '.')
-        || chars.all(is_script_glyph)
-        || (matches!(first, '(' | '[' | '{' | '|' | '⟨')
-            && !top_level(&s[first.len_utf8()..], |_| false))
-}
-
-/// Whether `c` is one of the super- or subscript characters scripts become.
-fn is_script_glyph(c: char) -> bool {
-    "⁰¹²³⁴⁵⁶⁷⁸⁹⁺⁻⁼⁽⁾ᵃᵇᶜᵈᵉᶠᵍʰⁱʲᵏˡᵐⁿᵒᵖʳˢᵗᵘᵛʷˣʸᶻ₀₁₂₃₄₅₆₇₈₉₊₋₌₍₎ₐₑₕᵢⱼₖₗₘₙₒₚᵣₛₜᵤᵥₓ'′″".contains(c)
-}
-
-/// Whether `s` has, outside any brackets, a character `pick` accepts. A
-/// bracket that closes the outermost one before the end also counts (so
-/// `(a)(b)` is not one bracketed operand).
-fn top_level(s: &str, pick: impl Fn(char) -> bool) -> bool {
-    let mut depth = 0usize;
-    let last = s.chars().count().saturating_sub(1);
-    for (i, c) in s.chars().enumerate() {
-        match c {
-            '(' | '[' | '{' | '⟨' => depth += 1,
-            ')' | ']' | '}' | '⟩' => {
-                if depth == 0 && i < last {
-                    return true;
-                }
-                depth = depth.saturating_sub(1);
-            }
-            c if depth == 0 && pick(c) => return true,
-            _ => {}
+        let part = typeset::rewrite_roots(&part);
+        if too_complex(&part) {
+            return Rendered::default();
         }
+        let tree = rust_latex_parser::parse_equation(&part);
+        let text = typeset::typeset(&tree, false);
+        lines.push(match &text {
+            Some(one) if one.rows.len() == 1 => one.rows[0].clone(),
+            _ => typeset::walk::one_line(&tree),
+        });
+        blocks.push(typeset::typeset(&tree, true));
+        texts.push(text);
+    }
+    let line = sanitize(&lines.join("; ")).trim().to_owned();
+    let inline = (!line.is_empty() && line.width() <= INLINE_MAX_COLS).then_some(line);
+    Rendered {
+        inline,
+        block: capped(typeset::stacked(blocks)),
+        text: capped(typeset::stacked(texts)),
     }
-    false
-}
-
-/// Whether `s` starts with a word of two or more letters (`otherwise`,
-/// `if`, `for`): a cases condition that needs no `if`.
-fn starts_with_word(s: &str) -> bool {
-    s.chars().take_while(|c| c.is_alphabetic()).count() >= 2
-}
-
-fn superscript(node: &EqNode) -> String {
-    script(node, superscript_char, '^')
-}
-
-fn subscript(node: &EqNode) -> String {
-    script(node, subscript_char, '_')
-}
-
-/// A script in Unicode script characters when every character has one,
-/// else `^(…)` / `_(…)`. Spaces are dropped: `i = 1` is `ᵢ₌₁`.
-fn script(node: &EqNode, map: fn(char) -> Option<char>, mark: char) -> String {
-    let text: String = one_line(node)
-        .chars()
-        .filter(|c| !c.is_whitespace())
-        .collect();
-    match text.chars().map(map).collect::<Option<String>>() {
-        Some(mapped) if !mapped.is_empty() => mapped,
-        _ => format!("{mark}({text})"),
-    }
-}
-
-fn superscript_char(c: char) -> Option<char> {
-    let i = "0123456789+-=()abcdefghijklmnoprstuvwxyz".find(c)?;
-    "⁰¹²³⁴⁵⁶⁷⁸⁹⁺⁻⁼⁽⁾ᵃᵇᶜᵈᵉᶠᵍʰⁱʲᵏˡᵐⁿᵒᵖʳˢᵗᵘᵛʷˣʸᶻ".chars().nth(i)
-}
-
-fn subscript_char(c: char) -> Option<char> {
-    let i = "0123456789+-=()aehijklmnoprstuvx".find(c)?;
-    "₀₁₂₃₄₅₆₇₈₉₊₋₌₍₎ₐₑₕᵢⱼₖₗₘₙₒₚᵣₛₜᵤᵥₓ".chars().nth(i)
 }
 
-/// `body` with an accent: a combining mark over a one-character body, else
-/// `hat(…)` and the like.
-fn accent(body: &str, kind: AccentKind) -> String {
-    let (mark, name) = match kind {
-        AccentKind::Hat => ('\u{302}', "hat"),
-        AccentKind::Bar => ('\u{305}', "bar"),
-        AccentKind::Dot => ('\u{307}', "dot"),
-        AccentKind::DoubleDot => ('\u{308}', "ddot"),
-        AccentKind::Tilde => ('\u{303}', "tilde"),
-        AccentKind::Vec => ('\u{20d7}', "vec"),
-    };
-    let body = body.trim();
-    if body.chars().count() == 1 {
-        format!("{body}{mark}")
-    } else {
-        format!("{name}({body})")
+/// `laid`, [`sanitize`]d, if it fits [`BLOCK_MAX_COLS`] × [`BLOCK_MAX_ROWS`].
+fn capped(laid: Option<Laid>) -> Option<Laid> {
+    let laid = laid?;
+    if laid.width() > BLOCK_MAX_COLS || laid.rows.len() > BLOCK_MAX_ROWS {
+        return None;
     }
+    Some(Laid {
+        rows: laid.rows.iter().map(|r| sanitize(r)).collect(),
+        baseline: laid.baseline,
+    })
 }
 
 // ── Matching keys ─────────────────────────────────────────────────────────────
diff --git a/src/tui/mod.rs b/src/tui/mod.rs
index 29c19d6..4fa8fd2 100644
--- a/src/tui/mod.rs
+++ b/src/tui/mod.rs
@@ -22,8 +22,6 @@ mod selection;
 mod sessions;
 mod state;
 mod stats_view;
-// Nothing calls the engine until `latex::render` does (the next task).
-#[allow(dead_code)]
 mod typeset;
 
 /// `recent` without the local directories that no longer exist, so the
diff --git a/src/tui/typeset/mod.rs b/src/tui/typeset/mod.rs
index b95a064..56a86b9 100644
--- a/src/tui/typeset/mod.rs
+++ b/src/tui/typeset/mod.rs
@@ -7,7 +7,11 @@
 //!
 //! - [`typeset`] — a parsed formula as rows, in display or text style.
 //! - [`rewrite_roots`] — `\sqrt[n]{x}` as the parser can read it.
+//! - [`environment`] — the rows of an `aligned`-like formula, which the
+//!   parser cannot read.
+//! - [`stacked`] — such rows, typeset one by one, as one block.
 //! - [`Laid`] — the result: rows and the row of the baseline.
+//! - [`walk::one_line`] — the one-line form when text style is taller.
 //!
 //! The rules (spec "Typeset reader view", §3 and Appendix A):
 //! - Display style only at the top of a display formula. Inside a script,
@@ -21,6 +25,7 @@
 //! - A run of spaces is one space (two for `\quad`), never at a box's ends.
 
 mod mathfont;
+pub mod walk;
 
 use rust_latex_parser::{AccentKind, EqNode, MatrixKind};
 use unicode_width::UnicodeWidthStr;
@@ -30,6 +35,24 @@ use unicode_width::UnicodeWidthStr;
 const MAX_COLS: usize = 512;
 const MAX_ROWS: usize = 40;
 
+/// The environments the parser reads; any other one it shows as source.
+const PARSED: [&str; 9] = [
+    "matrix",
+    "pmatrix",
+    "bmatrix",
+    "vmatrix",
+    "Bmatrix",
+    "Vmatrix",
+    "smallmatrix",
+    "cases",
+    "dcases",
+];
+/// The environments of rows a formula may be made of; the rows are laid
+/// out one by one.
+const ROWS: [&str; 7] = [
+    "aligned", "align", "align*", "gather", "gather*", "gathered", "array",
+];
+
 /// Big operators whose limits go above and below in display style; the
 /// others (integrals) keep them as scripts, as TeX does.
 const LIMITS_ABOVE: [&str; 6] = ["∑", "∏", "∐", "⋃", "⋂", "⨁"];
@@ -98,6 +121,160 @@ pub fn rewrite_roots(src: &str) -> String {
     out
 }
 
+/// What [`environment`] makes of a formula's source.
+#[derive(Debug, PartialEq)]
+pub enum Environment {
+    /// No environment the parser cannot read.
+    None,
+    /// The whole formula is one environment of rows (`aligned`, `gather`,
+    /// `array`, …): its rows, `&` dropped, to be laid out one by one.
+    Rows(Vec<String>),
+    /// An environment the parser does not know, or one of rows inside a
+    /// larger formula: shown as its source.
+    Unknown,
+}
+
+/// Whether `src` holds an environment the parser cannot read, and the rows
+/// of one it can be laid out from (see [`Environment`]). Rows are split on
+/// `\\` outside braces and nested environments; an `array`'s column spec
+/// and a row's `[2pt]` spacing are dropped.
+pub fn environment(src: &str) -> Environment {
+    let src = src.trim();
+    let begins = |s: &str| -> Option<(String, usize)> {
+        let name = s.strip_prefix("\\begin{")?;
+        let end = name.find('}')?;
+        Some((name[..end].to_owned(), "\\begin{".len() + end + 1))
+    };
+    if let Some((name, open)) = begins(src).filter(|(n, _)| ROWS.contains(&n.as_str())) {
+        let close = format!("\\end{{{name}}}");
+        if let Some(body) = src.strip_suffix(close.as_str()).map(|s| &s[open..]) {
+            if !body.contains(&close) {
+                let body = match name.as_str() {
+                    "array" => skip_group(body),
+                    _ => body,
+                };
+                if body.contains("\\begin{") && unknown_inside(body) {
+                    return Environment::Unknown;
+                }
+                return Environment::Rows(split_rows(body));
+            }
+        }
+    }
+    if unknown_inside(src) {
+        Environment::Unknown
+    } else {
+        Environment::None
+    }
+}
+
+/// Whether `src` has a `\begin{…}` the parser does not read.
+fn unknown_inside(src: &str) -> bool {
+    src.match_indices("\\begin{").any(|(at, tag)| {
+        let name = &src[at + tag.len()..];
+        let name = &name[..name.find('}').unwrap_or(name.len())];
+        !PARSED.contains(&name)
+    })
+}
+
+/// `s` without its leading `{…}` group (an `array`'s column spec).
+fn skip_group(s: &str) -> &str {
+    let t = s.trim_start();
+    if !t.starts_with('{') {
+        return s;
+    }
+    let mut depth = 0usize;
+    for (i, c) in t.char_indices() {
+        match c {
+            '{' => depth += 1,
+            '}' => {
+                depth -= 1;
+                if depth == 0 {
+                    return &t[i + 1..];
+                }
+            }
+            _ => {}
+        }
+    }
+    s
+}
+
+/// The rows of an environment's body: split on `\\` outside braces and
+/// nested environments, each without its `&` there, trimmed; empty rows
+/// dropped.
+fn split_rows(body: &str) -> Vec<String> {
+    let mut rows = Vec::new();
+    let mut row = String::new();
+    let (mut depth, mut nested) = (0usize, 0usize);
+    let mut rest = body;
+    while let Some(c) = rest.chars().next() {
+        if rest.starts_with("\\begin{") {
+            nested += 1;
+        } else if rest.starts_with("\\end{") {
+            nested = nested.saturating_sub(1);
+        }
+        let outside = depth == 0 && nested == 0;
+        if outside && rest.starts_with("\\\\") {
+            rows.push(std::mem::take(&mut row));
+            rest = &rest[2..];
+            // `\\[2pt]`: extra space after the row.
+            if let Some(end) = rest
+                .trim_start()
+                .strip_prefix('[')
+                .and_then(|t| t.find(']'))
+            {
+                let skipped = rest.len() - rest.trim_start().len();
+                rest = &rest[skipped + 1 + end + 1..];
+            }
+            continue;
+        }
+        if c == '\\' {
+            // A command (or an escaped character) is copied whole.
+            let len = rest[1..].chars().next().map_or(1, |next| {
+                1 + if next.is_ascii_alphabetic() {
+                    rest[1..]
+                        .chars()
+                        .take_while(char::is_ascii_alphabetic)
+                        .count()
+                } else {
+                    next.len_utf8()
+                }
+            });
+            row.push_str(&rest[..len]);
+            rest = &rest[len..];
+            continue;
+        }
+        match c {
+            '{' => depth += 1,
+            '}' => depth = depth.saturating_sub(1),
+            _ => {}
+        }
+        if !(outside && c == '&') {
+            row.push(c);
+        }
+        rest = &rest[c.len_utf8()..];
+    }
+    rows.push(row);
+    rows.into_iter()
+        .map(|r| r.trim().to_owned())
+        .filter(|r| !r.is_empty())
+        .collect()
+}
+
+/// `rows`, typeset one by one, as one block: one above the other,
+/// left-aligned, with the baseline at the middle row (the upper one of
+/// two). `None` when one of them is.
+pub fn stacked(rows: Vec<Option<Laid>>) -> Option<Laid> {
+    let rows: Vec<Laid> = rows.into_iter().collect::<Option<_>>()?;
+    if let [one] = rows.as_slice() {
+        return Some(one.clone());
+    }
+    let rows: Vec<String> = rows.into_iter().flat_map(|l| l.rows).collect();
+    (!rows.is_empty() && rows.len() <= MAX_ROWS).then(|| Laid {
+        baseline: (rows.len() - 1) / 2,
+        rows,
+    })
+}
+
 /// Where the `]` that closes a bracket ends `s` (the text after `[`): the
 /// first one outside braces.
 fn bracket_end(s: &str) -> Option<usize> {
@@ -636,12 +813,12 @@ fn top_level(s: &str, pick: impl Fn(char) -> bool) -> bool {
     false
 }
 
-pub(super) fn superscript_char(c: char) -> Option<char> {
+fn superscript_char(c: char) -> Option<char> {
     let i = "0123456789+-=()abcdefghijklmnoprstuvwxyz".find(c)?;
     "⁰¹²³⁴⁵⁶⁷⁸⁹⁺⁻⁼⁽⁾ᵃᵇᶜᵈᵉᶠᵍʰⁱʲᵏˡᵐⁿᵒᵖʳˢᵗᵘᵛʷˣʸᶻ".chars().nth(i)
 }
 
-pub(super) fn subscript_char(c: char) -> Option<char> {
+fn subscript_char(c: char) -> Option<char> {
     let i = "0123456789+-=()aehijklmnoprstuvx".find(c)?;
     "₀₁₂₃₄₅₆₇₈₉₊₋₌₍₎ₐₑₕᵢⱼₖₗₘₙₒₚᵣₛₜᵤᵥₓ".chars().nth(i)
 }
diff --git a/src/tui/typeset/walk.rs b/src/tui/typeset/walk.rs
new file mode 100644
index 0000000..25fb037
--- /dev/null
+++ b/src/tui/typeset/walk.rs
@@ -0,0 +1,210 @@
+//! The one-line walk of v0.4.0, moved here unchanged: one line of text
+//! for a parse tree, for formulas whose text-style layout is taller than one
+//! row (the pane shows one line per tag).
+
+use rust_latex_parser::{AccentKind, EqNode, MatrixKind};
+
+use super::{flatten, is_atom, mathfont, subscript_char, superscript_char, top_level};
+
+/// One line of text for `node`, modelled on term-maths' layout: fractions
+/// become `a/b`, scripts Unicode super- and subscripts (else `^(…)` and
+/// `_(…)`), roots `√x`, matrices `[a b; c d]`, cases `{ 1 if x > 0; 0
+/// otherwise }`. Symbols are already Unicode in the parse tree.
+pub fn one_line(node: &EqNode) -> String {
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
+        EqNode::MathFont { kind, content } => one_line(content)
+            .chars()
+            .map(|c| mathfont::map_char(*kind, c))
+            .collect(),
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
`````
Run: `umask 022; cargo build --locked 2>&1 | grep -cE '^(warning|error)'; cargo test --locked --bin claudio -- tui::latex tui::typeset tui::formulas tui::ui tui::tests && rustfmt --edition 2021 --check src/tui/latex.rs src/tui/formulas.rs src/tui/ui/formulas.rs src/tui/typeset/mod.rs src/tui/typeset/walk.rs src/tui/typeset/mathfont.rs && grep -c term-maths Cargo.toml Cargo.lock; make test-unit`
Expected: `0`, then `test result: ok. 98 passed; 0 failed; 0 ignored; 0 measured; 761 filtered out`, then `Cargo.toml:0` and `Cargo.lock:0`, then `test result: ok. 859 passed; 0 failed`.

Step 3: commit
`/usr/bin/git add src/tui/typeset/walk.rs && /usr/bin/git commit -m "feat(tui): render formulas with the typeset engine, drop term-maths" -- Cargo.toml Cargo.lock src/tui/latex.rs src/tui/formulas.rs src/tui/mod.rs src/tui/typeset/mod.rs src/tui/typeset/walk.rs src/tui/ui/formulas.rs src/tui/ui/mod.rs`

Interfaces this task provides to later tasks: `tui::latex::Rendered { pub inline: Option<String>, pub block: Option<Laid>, pub text: Option<Laid> }` (Default), `Formula { src, inline, block: Option<Laid>, text: Option<Laid>, … }`, `Formula::new(&str, Rendered)`, `tui::typeset::{environment(&str) -> Environment, Environment::{None, Rows(Vec<String>), Unknown}, stacked(Vec<Option<Laid>>) -> Option<Laid>, walk::one_line(&EqNode) -> String}`.

### Task 4: answers as markdown documents (pulldown-cmark)
Model: opus   Repo: /volumes/repos/claudio   Files: Cargo.toml, Cargo.lock, src/claude/latex.rs, src/tui/mod.rs, src/tui/reader/mod.rs (new), src/tui/reader/doc.rs (new)
Goal: AC7 (hostile markdown), with F5 (64 formulas per answer, placeholders U+E000 + index), F7 (placeholder lifecycle, options), F8 (display rule), F9 (tabs). Adds pulldown-cmark =0.13.6 without default features (pulls `unicase` 2.10.0; `bitflags` and `memchr` are already locked). `claude::latex::latex_tags` gives each formula tag's byte range by `extract_latex`'s rules (which now uses it), and `is_combining` becomes `pub`. `reader::doc::prepare` sanitizes the answer again, then replaces each of the first 64 tags by its placeholder and drops a combining mark right after it; `parse` reads the markdown with tables, strikethrough and task lists only, resolves placeholders by iterating chars, re-sanitizes each text run, turns a placeholder inside code back into its tag's text, expands tabs (4 spaces in code blocks, 1 elsewhere), makes a formula that is all of a paragraph, a tight list item or a table cell a display formula, and caps list/quote nesting at 8 (deeper content flattened). Nothing uses it yet: `mod reader;` carries a temporary `#[allow(dead_code)]`, removed in Task 7.

The new dependency needs the network once: after applying Step 2, run `cargo fetch --locked` (it downloads pulldown-cmark 0.13.6 and unicase 2.10.0; the checksums are in the Cargo.lock diff).

Step 1: write the failing tests.
`````diff
diff --git a/src/claude/latex.rs b/src/claude/latex.rs
index 93ab436..aad65e9 100644
--- a/src/claude/latex.rs
+++ b/src/claude/latex.rs
@@ -219,6 +230,17 @@ mod tests {
         assert_eq!(extract_latex(&text), [max]);
     }
 
+    #[test]
+    fn tags_come_with_their_byte_ranges() {
+        let text = "é <latex> x </latex> `<latex>y</latex>` <latex></latex><latex>z</latex>";
+        let tags = latex_tags(text);
+        assert_eq!(tags.len(), 2);
+        assert_eq!(&text[tags[0].0.clone()], "<latex> x </latex>");
+        assert_eq!(tags[0].1, "x");
+        assert_eq!(&text[tags[1].0.clone()], "<latex>z</latex>");
+        assert_eq!(tags[1].1, "z");
+    }
+
     #[test]
     fn sanitize_drops_controls_format_and_private_use() {
         // A C0 escape, a C1 control, a bidi override, a zero-width space, a
diff --git a/src/tui/mod.rs b/src/tui/mod.rs
index 4fa8fd2..290f04f 100644
--- a/src/tui/mod.rs
+++ b/src/tui/mod.rs
@@ -18,6 +18,9 @@ mod latex;
 mod notifications;
 mod plain;
 mod proxy_state;
+// Nothing uses the reader until the worker and the view do (later tasks).
+#[allow(dead_code)]
+mod reader;
 mod selection;
 mod sessions;
 mod state;
diff --git a/src/tui/reader/doc.rs b/src/tui/reader/doc.rs
new file mode 100644
index 0000000..3f2a371
--- /dev/null
+++ b/src/tui/reader/doc.rs
@@ -0,0 +1,187 @@
+#[cfg(test)]
+mod tests {
+    use super::*;
+
+    fn doc(text: &str) -> Doc {
+        parse(&prepare(text))
+    }
+
+    fn plain(s: &str) -> Span {
+        Span::Text(s.into(), Emphasis::default())
+    }
+
+    #[test]
+    fn tags_become_placeholders_after_sanitizing() {
+        let p = prepare("a <latex>x^2</latex>\u{301}b \u{e000}c `<latex>y</latex>`");
+        assert_eq!(p.text, "a \u{e000}b c `<latex>y</latex>`");
+        assert_eq!(
+            p.tags,
+            [FormulaTag {
+                src: "x^2".into(),
+                raw: "<latex>x^2</latex>".into(),
+            }]
+        );
+        // Past MAX_FORMULAS, tags stay text.
+        let many = "<latex>x</latex> ".repeat(MAX_FORMULAS + 2);
+        let p = prepare(&many);
+        assert_eq!(p.tags.len(), MAX_FORMULAS);
+        assert!(p.text.ends_with("<latex>x</latex> <latex>x</latex> "));
+    }
+
+    #[test]
+    fn markdown_becomes_blocks() {
+        let d = doc("# Title\n\nSome *em* and **bold** `code`.\n\n```\nfn\tx\n```\n\n---\n");
+        let em = Emphasis {
+            italic: true,
+            ..Emphasis::default()
+        };
+        let bold = Emphasis {
+            bold: true,
+            ..Emphasis::default()
+        };
+        assert_eq!(
+            d.blocks,
+            [
+                Block::Heading(1, vec![plain("Title")]),
+                Block::Para(vec![
+                    plain("Some "),
+                    Span::Text("em".into(), em),
+                    plain(" and "),
+                    Span::Text("bold".into(), bold),
+                    plain(" "),
+                    Span::Code("code".into()),
+                    plain("."),
+                ]),
+                Block::Code(vec!["fn    x".into()]),
+                Block::Rule,
+            ]
+        );
+    }
+
+    #[test]
+    fn lists_quotes_and_tables() {
+        let d = doc("- one\n- two\n  1. deep\n\n> quoted\n\n| a | b |\n|---|---|\n| 1 | <latex>x</latex> |\n");
+        assert_eq!(
+            d.blocks,
+            [
+                Block::List(
+                    None,
+                    vec![
+                        vec![Block::Para(vec![plain("one")])],
+                        vec![
+                            Block::Para(vec![plain("two")]),
+                            Block::List(Some(1), vec![vec![Block::Para(vec![plain("deep")])]]),
+                        ],
+                    ]
+                ),
+                Block::Quote(vec![Block::Para(vec![plain("quoted")])]),
+                Block::Table(vec![
+                    vec![vec![plain("a")], vec![plain("b")]],
+                    vec![vec![plain("1")], vec![Span::Display(0)]],
+                ]),
+            ]
+        );
+    }
+
+    #[test]
+    fn a_formula_alone_is_display() {
+        let d = doc("Text <latex>a</latex> here.\n\n<latex>b</latex>\n\n- <latex>c</latex>\n- item <latex>d</latex>\n\n> <latex>e</latex>\n");
+        assert_eq!(
+            d.blocks,
+            [
+                Block::Para(vec![plain("Text "), Span::Formula(0), plain(" here.")]),
+                Block::Display(1),
+                Block::List(
+                    None,
+                    vec![
+                        vec![Block::Display(2)],
+                        vec![Block::Para(vec![plain("item "), Span::Formula(3)])],
+                    ]
+                ),
+                Block::Quote(vec![Block::Display(4)]),
+            ]
+        );
+    }
+
+    #[test]
+    fn a_placeholder_in_code_is_its_tag_again() {
+        // Markdown takes this backtick run as code, `extract_latex` does
+        // not (its code spans end at the line).
+        let p = prepare("``a\n<latex>x</latex> b``");
+        assert_eq!(p.tags.len(), 1);
+        assert_eq!(
+            parse(&p).blocks,
+            [Block::Para(vec![Span::Code("a <latex>x</latex> b".into())])]
+        );
+        // `$` is text: no math extension.
+        assert_eq!(doc("$x$").blocks, [Block::Para(vec![plain("$x$")])]);
+    }
+
+    #[test]
+    fn nesting_is_flattened_past_the_cap() {
+        let quotes = format!("{} deep", ">".repeat(MAX_NEST + 5));
+        let mut d = &doc(&quotes).blocks;
+        let mut depth = 0;
+        while let [Block::Quote(inner)] = d.as_slice() {
+            depth += 1;
+            d = inner;
+        }
+        assert_eq!(depth, MAX_NEST);
+        assert_eq!(*d, [Block::Para(vec![plain("deep")])]);
+    }
+
+    /// AC7: hostile markdown is bounded and sanitized.
+    #[test]
+    fn hostile_markdown_is_bounded_and_sanitized() {
+        let started = std::time::Instant::now();
+        let deep_list: String = (0..250)
+            .map(|i| format!("{}- x\n", "  ".repeat(i)))
+            .collect();
+        let cases = [
+            "x".repeat(64 * 1024),
+            ">".repeat(64 * 1024),
+            deep_list,
+            "*".repeat(32 * 1024) + "x" + &"*".repeat(32 * 1024),
+            format!("{}\n{}\n", "|a".repeat(8000) + "|", "|-".repeat(8000) + "|"),
+            "<latex>x</latex>".repeat(4000),
+            "\u{e000}\u{e001}\u{1b}[2J \u{202e}text\u{9b}".to_owned(),
+        ];
+        for text in &cases {
+            let p = prepare(text);
+            assert!(p.tags.len() <= MAX_FORMULAS);
+            let d = parse(&p);
+            check(&d.blocks, p.tags.len(), 0);
+        }
+        // Debug build, all seven: well under a second each in practice.
+        assert!(started.elapsed() < std::time::Duration::from_secs(10));
+        let p = prepare(&cases[6]);
+        assert!(p.tags.is_empty(), "input cannot forge a placeholder");
+        assert_eq!(parse(&p).blocks, [Block::Para(vec![plain("[2J text")])]);
+    }
+
+    /// Every string is sanitized, every formula index valid, and lists and
+    /// quotes at most MAX_NEST deep.
+    fn check(blocks: &[Block], formulas: usize, depth: usize) {
+        assert!(depth <= MAX_NEST);
+        let spans = |spans: &[Span]| {
+            for s in spans {
+                match s {
+                    Span::Text(t, _) | Span::Code(t) => assert_eq!(*t, sanitize(t)),
+                    Span::Formula(i) | Span::Display(i) => assert!(*i < formulas),
+                    Span::Break => {}
+                }
+            }
+        };
+        for b in blocks {
+            match b {
+                Block::Para(s) | Block::Heading(_, s) => spans(s),
+                Block::Code(lines) => lines.iter().for_each(|l| assert_eq!(*l, sanitize(l))),
+                Block::List(_, items) => items.iter().for_each(|i| check(i, formulas, depth + 1)),
+                Block::Quote(inner) => check(inner, formulas, depth + 1),
+                Block::Table(rows) => rows.iter().flatten().for_each(|c| spans(c)),
+                Block::Rule => {}
+                Block::Display(i) => assert!(*i < formulas),
+            }
+        }
+    }
+}
diff --git a/src/tui/reader/mod.rs b/src/tui/reader/mod.rs
new file mode 100644
index 0000000..3782c13
--- /dev/null
+++ b/src/tui/reader/mod.rs
@@ -0,0 +1,6 @@
+//! The reader view (Alt+u): the session's recent answers, rendered by
+//! claudio from the transcript, with their formulas typeset.
+//!
+//! - [`doc`] — an answer as markdown blocks (on the render worker).
+
+pub mod doc;
`````
Run: `umask 022; cargo test --locked --bin claudio -- tui::reader claude::latex`
Expected: compilation fails, `error: could not compile `claudio` (bin "claudio" test) due to 74 previous errors; 1 warning emitted`: `E0425`/`E0433`/`E0422` cannot find `prepare`, `parse`, `latex_tags`, `Block`, `Span`, `Emphasis`, `Doc`, `FormulaTag`, `MAX_NEST`, `MAX_FORMULAS`, and `E0423` for `sanitize`; the warning is the unused `use super::*` of `doc.rs`.

Step 2: implement.
`````diff
diff --git a/Cargo.lock b/Cargo.lock
index d2231e7..279f02f 100644
--- a/Cargo.lock
+++ b/Cargo.lock
@@ -257,6 +257,7 @@ dependencies = [
  "futures",
  "libc",
  "portable-pty",
+ "pulldown-cmark",
  "ratatui",
  "reqwest",
  "rust-latex-parser",
@@ -1642,6 +1643,17 @@ dependencies = [
  "unicode-ident",
 ]
 
+[[package]]
+name = "pulldown-cmark"
+version = "0.13.6"
+source = "registry+https://github.com/rust-lang/crates.io-index"
+checksum = "3db2be46835f013dd3f4fa7f5f7402c537787b54ebb6fe2b94bbdd7e2669bb7b"
+dependencies = [
+ "bitflags 2.13.2",
+ "memchr",
+ "unicase",
+]
+
 [[package]]
 name = "quinn"
 version = "0.11.12"
@@ -2767,6 +2779,12 @@ version = "0.1.7"
 source = "registry+https://github.com/rust-lang/crates.io-index"
 checksum = "2896d95c02a80c6d6a5d6e953d479f5ddf2dfdb6a244441010e373ac0fb88971"
 
+[[package]]
+name = "unicase"
+version = "2.10.0"
+source = "registry+https://github.com/rust-lang/crates.io-index"
+checksum = "357cc3acc6a036009fd6c973ed009037c732d60d0b4f6c673e9041497482a28f"
+
 [[package]]
 name = "unicode-ident"
 version = "1.0.24"
diff --git a/Cargo.toml b/Cargo.toml
index 9d2d365..abeca38 100644
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -57,6 +57,10 @@ reqwest = { version = "0.12", default-features = false, features = ["rustls-tls"
 rust-latex-parser = "=0.1.1"
 unicode-segmentation = "1"
 unicode-width = "0.2"
+# The reader view's markdown (claude's answers). Pure Rust; no default
+# features (no HTML output). Pinned exactly like the parser: the reader's
+# hostile-input bounds were measured against this version.
+pulldown-cmark = { version = "=0.13.6", default-features = false }
 
 # Only needed in integration tests (no runtime dep).
 [dev-dependencies]
diff --git a/src/claude/latex.rs b/src/claude/latex.rs
index ba6229a..aad65e9 100644
--- a/src/claude/latex.rs
+++ b/src/claude/latex.rs
@@ -79,7 +79,7 @@ fn is_private_or_unassigned(c: char) -> bool {
 /// The combining-mark (`Mn`) blocks a formula may carry: diacritics, their
 /// extensions and supplements, the marks for symbols, half marks and
 /// variation selectors.
-fn is_combining(c: char) -> bool {
+pub fn is_combining(c: char) -> bool {
     matches!(
         c as u32,
         0x300..=0x36F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE00..=0xFE0F
@@ -94,6 +94,17 @@ fn is_combining(c: char) -> bool {
 /// are not formulas. Empty formulas, and ones longer than
 /// [`MAX_FORMULA_BYTES`], are dropped.
 pub fn extract_latex(text: &str) -> Vec<String> {
+    latex_tags(text)
+        .into_iter()
+        .map(|(_, formula)| formula)
+        .collect()
+}
+
+/// The formula tags of one assistant text, by [`extract_latex`]'s rules:
+/// each tag's byte range in `text` (from its `<latex>` to the end of its
+/// `</latex>`) and its formula. Tags that are not formulas (in code, empty,
+/// too long) are not listed.
+pub fn latex_tags(text: &str) -> Vec<(std::ops::Range<usize>, String)> {
     let mut out = Vec::new();
     if !text.contains("<latex>") {
         return out;
@@ -126,10 +137,10 @@ pub fn extract_latex(text: &str) -> Vec<String> {
         };
         let formula = sanitize(&text[inner..close]);
         let formula = formula.trim();
+        at = close + CLOSE.len();
         if !formula.is_empty() && formula.len() <= MAX_FORMULA_BYTES {
-            out.push(formula.to_owned());
+            out.push((open..at, formula.to_owned()));
         }
-        at = close + CLOSE.len();
     }
     out
 }
diff --git a/src/tui/reader/doc.rs b/src/tui/reader/doc.rs
index beccbed..3f2a371 100644
--- a/src/tui/reader/doc.rs
+++ b/src/tui/reader/doc.rs
@@ -1,3 +1,426 @@
+//! One of claude's answers as the reader view's document: markdown blocks
+//! (pulldown-cmark) whose `<latex>` formulas are indices into the answer's
+//! formula list. Width-independent; `layout.rs` wraps it.
+//!
+//! Pure, and run on the render worker (`tui/mod.rs`), never on the UI
+//! thread. The text is attacker-influenced, so:
+//! - [`prepare`] sanitizes it again, then replaces each formula tag (by
+//!   `extract_latex`'s rules) with one private-use placeholder; sanitizing
+//!   removed every private-use character, so the text cannot forge one.
+//! - [`parse`] turns placeholders back into formulas while it reads
+//!   markdown (iterating chars, never graphemes), and sanitizes each text
+//!   run again once its placeholders are gone. In code, a placeholder is
+//!   its tag's text again.
+//! - Lists and quotes nest at most [`MAX_NEST`] deep; deeper content is
+//!   flattened into the deepest one, so layout recursion is bounded.
+
+use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
+
+use crate::claude::latex::{is_combining, latex_tags, sanitize, MAX_FORMULAS};
+
+/// The first placeholder: formula `i` of an answer is `PLACEHOLDER + i`.
+const PLACEHOLDER: u32 = 0xE000;
+/// How deep lists and block quotes nest.
+pub const MAX_NEST: usize = 8;
+/// What a tab is in a code block (elsewhere it is one space).
+const TAB: &str = "    ";
+
+/// An answer's text ready for [`parse`]: formula tags replaced by
+/// placeholders, and the tags.
+#[derive(Debug, Clone, PartialEq)]
+pub struct Prepared {
+    pub text: String,
+    pub tags: Vec<FormulaTag>,
+}
+
+/// One formula tag of an answer.
+#[derive(Debug, Clone, PartialEq)]
+pub struct FormulaTag {
+    /// The formula, as `extract_latex` gives it (the renderer's input).
+    pub src: String,
+    /// The whole tag as written, shown where markdown made it code.
+    pub raw: String,
+}
+
+/// An answer as markdown blocks.
+#[derive(Debug, Clone, Default, PartialEq)]
+pub struct Doc {
+    pub blocks: Vec<Block>,
+}
+
+#[derive(Debug, Clone, PartialEq)]
+pub enum Block {
+    /// A paragraph, or the text of a tight list item.
+    Para(Vec<Span>),
+    /// A heading of level 1 to 6.
+    Heading(u8, Vec<Span>),
+    /// A code block's lines, tabs expanded.
+    Code(Vec<String>),
+    /// A list, numbered from its start or bulleted; each item its blocks.
+    List(Option<u64>, Vec<Vec<Block>>),
+    Quote(Vec<Block>),
+    /// A table's rows, the header first; each row its cells.
+    Table(Vec<Vec<Vec<Span>>>),
+    Rule,
+    /// A display formula: a paragraph or tight item that is only a formula.
+    Display(usize),
+}
+
+#[derive(Debug, Clone, PartialEq)]
+pub enum Span {
+    Text(String, Emphasis),
+    /// Inline code.
+    Code(String),
+    /// A formula in the text: an index into the answer's formulas.
+    Formula(usize),
+    /// A table cell that is only a formula, typeset in display style.
+    Display(usize),
+    /// A hard line break.
+    Break,
+}
+
+#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
+pub struct Emphasis {
+    pub bold: bool,
+    pub italic: bool,
+    pub strike: bool,
+}
+
+/// `text` (an answer) [`sanitize`]d, with each of its first
+/// [`MAX_FORMULAS`] formula tags replaced by a placeholder, and a combining
+/// mark right after a tag dropped (it would join the placeholder). Later
+/// tags stay as they are written.
+pub fn prepare(text: &str) -> Prepared {
+    let clean = sanitize(text);
+    let mut out = String::with_capacity(clean.len());
+    let mut tags = Vec::new();
+    let mut copied = 0;
+    for (range, src) in latex_tags(&clean).into_iter().take(MAX_FORMULAS) {
+        out.push_str(&clean[copied..range.start]);
+        out.push(placeholder(tags.len()));
+        tags.push(FormulaTag {
+            src,
+            raw: clean[range.clone()].to_owned(),
+        });
+        let rest = &clean[range.end..];
+        copied = range.end + (rest.len() - rest.trim_start_matches(is_combining).len());
+    }
+    out.push_str(&clean[copied..]);
+    Prepared { text: out, tags }
+}
+
+/// The placeholder of formula `i` (`i` < [`MAX_FORMULAS`]).
+fn placeholder(i: usize) -> char {
+    char::from_u32(PLACEHOLDER + i as u32).unwrap_or(char::REPLACEMENT_CHARACTER)
+}
+
+/// The formula a placeholder stands for, if `c` is one of `count`.
+fn formula_of(c: char, count: usize) -> Option<usize> {
+    let i = (c as u32).checked_sub(PLACEHOLDER)? as usize;
+    (i < count).then_some(i)
+}
+
+/// The markdown of `prepared` as a [`Doc`]. pulldown-cmark reads it with
+/// tables, strikethrough and task lists only (no math, footnotes or smart
+/// punctuation: `$` is text). A formula is display when its placeholder is
+/// all a paragraph, a tight list item or a table cell holds.
+pub fn parse(prepared: &Prepared) -> Doc {
+    let options =
+        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
+    let mut b = Builder::new(&prepared.tags);
+    for event in Parser::new_ext(&prepared.text, options) {
+        b.event(event);
+    }
+    b.flush();
+    Doc { blocks: b.root }
+}
+
+/// A container being read.
+enum Frame {
+    Quote(Vec<Block>),
+    List(Option<u64>, Vec<Vec<Block>>),
+    Item(Vec<Block>),
+}
+
+/// The state of [`parse`]: the open containers and the inline content
+/// being gathered.
+struct Builder<'a> {
+    tags: &'a [FormulaTag],
+    root: Vec<Block>,
+    frames: Vec<Frame>,
+    /// Per open container start: whether it got a frame (`false`: it was
+    /// nested too deep, its content goes to the frame around it).
+    open: Vec<bool>,
+    /// How many open containers got no frame.
+    flattened: usize,
+    spans: Vec<Span>,
+    emphasis: Emphasis,
+    /// Nesting counts behind `emphasis`.
+    bold: u32,
+    italic: u32,
+    strike: u32,
+    heading: Option<u8>,
+    code: Option<String>,
+    /// The table being read: its rows, and the row being read.
+    table: Option<(Vec<Vec<Vec<Span>>>, Vec<Vec<Span>>)>,
+}
+
+impl<'a> Builder<'a> {
+    fn new(tags: &'a [FormulaTag]) -> Builder<'a> {
+        Builder {
+            tags,
+            root: Vec::new(),
+            frames: Vec::new(),
+            open: Vec::new(),
+            flattened: 0,
+            spans: Vec::new(),
+            emphasis: Emphasis::default(),
+            bold: 0,
+            italic: 0,
+            strike: 0,
+            heading: None,
+            code: None,
+            table: None,
+        }
+    }
+
+    fn event(&mut self, event: Event) {
+        match event {
+            Event::Start(tag) => self.start(tag),
+            Event::End(tag) => self.end(tag),
+            Event::Text(text) => match &mut self.code {
+                Some(code) => code.push_str(&text),
+                None => self.text(&text),
+            },
+            Event::Code(code) => {
+                let code = self.code_text(&code, " ");
+                self.spans.push(Span::Code(code));
+            }
+            Event::Html(html) | Event::InlineHtml(html) => self.text(&html),
+            Event::InlineMath(math) | Event::DisplayMath(math) => self.text(&math),
+            Event::FootnoteReference(name) => self.text(&name),
+            Event::SoftBreak => self.text(" "),
+            Event::HardBreak => self.spans.push(Span::Break),
+            Event::Rule => {
+                self.flush();
+                self.push(Block::Rule);
+            }
+            Event::TaskListMarker(done) => self.text(if done { "[x] " } else { "[ ] " }),
+        }
+    }
+
+    fn start(&mut self, tag: Tag) {
+        match tag {
+            Tag::Paragraph | Tag::HtmlBlock => self.flush(),
+            Tag::Heading { level, .. } => {
+                self.flush();
+                self.heading = Some(level as u8);
+            }
+            Tag::CodeBlock(_) => {
+                self.flush();
+                self.code = Some(String::new());
+            }
+            Tag::BlockQuote(_) => self.open_frame(Frame::Quote(Vec::new())),
+            Tag::List(start) => self.open_frame(Frame::List(start, Vec::new())),
+            Tag::Item => self.open_frame(Frame::Item(Vec::new())),
+            Tag::Table(_) => {
+                self.flush();
+                self.table = Some((Vec::new(), Vec::new()));
+            }
+            Tag::TableHead | Tag::TableRow | Tag::TableCell => self.spans.clear(),
+            Tag::Emphasis => self.italic += 1,
+            Tag::Strong => self.bold += 1,
+            Tag::Strikethrough => self.strike += 1,
+            _ => {}
+        }
+        self.restyle();
+    }
+
+    fn end(&mut self, tag: TagEnd) {
+        match tag {
+            TagEnd::Paragraph | TagEnd::HtmlBlock => self.flush(),
+            TagEnd::Heading(_) => {
+                let level = self.heading.take().unwrap_or(1);
+                let spans = trimmed(std::mem::take(&mut self.spans));
+                if !spans.is_empty() {
+                    self.push(Block::Heading(level, spans));
+                }
+            }
+            TagEnd::CodeBlock => {
+                let code = self.code.take().unwrap_or_default();
+                let code = self.code_text(code.trim_end_matches('\n'), TAB);
+                self.push(Block::Code(code.split('\n').map(str::to_owned).collect()));
+            }
+            TagEnd::BlockQuote(_) | TagEnd::List(_) | TagEnd::Item => self.close_frame(),
+            TagEnd::TableCell => {
+                let mut cell = trimmed(std::mem::take(&mut self.spans));
+                if let [Span::Formula(i)] = cell.as_slice() {
+                    cell = vec![Span::Display(*i)];
+                }
+                if let Some((_, row)) = &mut self.table {
+                    row.push(cell);
+                }
+            }
+            TagEnd::TableHead | TagEnd::TableRow => {
+                if let Some((rows, row)) = &mut self.table {
+                    rows.push(std::mem::take(row));
+                }
+            }
+            TagEnd::Table => {
+                if let Some((rows, _)) = self.table.take() {
+                    self.push(Block::Table(rows));
+                }
+            }
+            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
+            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
+            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
+            _ => {}
+        }
+        self.restyle();
+    }
+
+    fn restyle(&mut self) {
+        self.emphasis = Emphasis {
+            bold: self.bold > 0,
+            italic: self.italic > 0,
+            strike: self.strike > 0,
+        };
+    }
+
+    /// Open a container: a frame of its own, unless lists and quotes are
+    /// already [`MAX_NEST`] deep (or an item outside a list frame).
+    fn open_frame(&mut self, frame: Frame) {
+        self.flush();
+        let depth = self
+            .frames
+            .iter()
+            .filter(|f| !matches!(f, Frame::Item(_)))
+            .count();
+        let fits = match frame {
+            Frame::Item(_) => matches!(self.frames.last(), Some(Frame::List(..))),
+            _ => depth < MAX_NEST,
+        };
+        let framed = self.flattened == 0 && fits;
+        if framed {
+            self.frames.push(frame);
+        } else {
+            self.flattened += 1;
+        }
+        self.open.push(framed);
+    }
+
+    /// Close the container opened last: its block goes to the one around
+    /// it (an item to its list).
+    fn close_frame(&mut self) {
+        self.flush();
+        if !self.open.pop().unwrap_or(false) {
+            self.flattened = self.flattened.saturating_sub(1);
+            return;
+        }
+        match self.frames.pop() {
+            Some(Frame::Quote(blocks)) => self.push(Block::Quote(blocks)),
+            Some(Frame::List(start, items)) => self.push(Block::List(start, items)),
+            Some(Frame::Item(blocks)) => match self.frames.last_mut() {
+                Some(Frame::List(_, items)) => items.push(blocks),
+                _ => self.root.extend(blocks),
+            },
+            None => {}
+        }
+    }
+
+    /// Add a block to the innermost container.
+    fn push(&mut self, block: Block) {
+        match self.frames.last_mut() {
+            Some(Frame::Quote(blocks) | Frame::Item(blocks)) => blocks.push(block),
+            // Not inside an item: cannot happen with pulldown-cmark.
+            Some(Frame::List(_, items)) => items.push(vec![block]),
+            None => self.root.push(block),
+        }
+    }
+
+    /// End the inline content gathered so far as a paragraph (a display
+    /// formula when it is only one), unless a heading or table cell is
+    /// gathering it.
+    fn flush(&mut self) {
+        if self.heading.is_some() || self.table.is_some() {
+            return;
+        }
+        let spans = trimmed(std::mem::take(&mut self.spans));
+        match spans.as_slice() {
+            [] => {}
+            [Span::Formula(i)] => self.push(Block::Display(*i)),
+            _ => self.push(Block::Para(spans)),
+        }
+    }
+
+    /// Inline text: its placeholders become formulas, the runs between
+    /// them are sanitized (tabs as one space) and joined to the text before
+    /// them when they share its emphasis.
+    fn text(&mut self, text: &str) {
+        let mut run = String::new();
+        for c in text.chars() {
+            match formula_of(c, self.tags.len()) {
+                Some(i) => {
+                    self.push_run(&mut run);
+                    self.spans.push(Span::Formula(i));
+                }
+                None if c == '\t' => run.push(' '),
+                None => run.push(c),
+            }
+        }
+        self.push_run(&mut run);
+    }
+
+    fn push_run(&mut self, run: &mut String) {
+        let text = sanitize(&std::mem::take(run));
+        if text.is_empty() {
+            return;
+        }
+        if let Some(Span::Text(last, emphasis)) = self.spans.last_mut() {
+            if *emphasis == self.emphasis {
+                last.push_str(&text);
+                return;
+            }
+        }
+        self.spans.push(Span::Text(text, self.emphasis));
+    }
+
+    /// Code as shown: each placeholder its tag's text again, tabs as `tab`,
+    /// sanitized.
+    fn code_text(&self, code: &str, tab: &str) -> String {
+        let mut out = String::with_capacity(code.len());
+        for c in code.chars() {
+            match formula_of(c, self.tags.len()) {
+                Some(i) => out.push_str(&self.tags[i].raw),
+                None if c == '\t' => out.push_str(tab),
+                None => out.push(c),
+            }
+        }
+        sanitize(&out).replace('\t', tab)
+    }
+}
+
+/// `spans` without the blank text and breaks at either end.
+fn trimmed(mut spans: Vec<Span>) -> Vec<Span> {
+    let blank = |s: &Span| match s {
+        Span::Text(t, _) => t.trim().is_empty(),
+        Span::Break => true,
+        _ => false,
+    };
+    while spans.last().is_some_and(blank) {
+        spans.pop();
+    }
+    let lead = spans.iter().take_while(|s| blank(s)).count();
+    spans.drain(..lead);
+    if let Some(Span::Text(t, _)) = spans.first_mut() {
+        *t = t.trim_start().to_owned();
+    }
+    if let Some(Span::Text(t, _)) = spans.last_mut() {
+        *t = t.trim_end().to_owned();
+    }
+    spans
+}
+
 #[cfg(test)]
 mod tests {
     use super::*;
`````
Run: `cargo fetch --locked; umask 022; cargo build --locked 2>&1 | grep -cE '^(warning|error)'; cargo test --locked --bin claudio -- tui::reader claude::latex && rustfmt --edition 2021 --check src/tui/reader/doc.rs src/tui/reader/mod.rs src/claude/latex.rs && make test-unit`
Expected: `0`, then `test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 853 filtered out` (the hostile test takes well under a second), then `test result: ok. 867 passed; 0 failed`.

Step 3: commit
`/usr/bin/git add src/tui/reader/mod.rs src/tui/reader/doc.rs && /usr/bin/git commit -m "feat(tui): reader documents from claude's markdown answers" -- Cargo.toml Cargo.lock src/claude/latex.rs src/tui/mod.rs src/tui/reader/mod.rs src/tui/reader/doc.rs`

Interfaces this task provides to later tasks: `claude::latex::{latex_tags(&str) -> Vec<(Range<usize>, String)>, is_combining}`, `tui::reader::doc::{prepare(&str) -> Prepared, parse(&Prepared) -> Doc, Prepared { text, tags: Vec<FormulaTag> }, FormulaTag { src, raw }, Doc { blocks }, Block::{Para, Heading, Code, List, Quote, Table, Rule, Display}, Span::{Text, Code, Formula, Display, Break}, Emphasis, MAX_NEST}`.

### Task 5: laying a document out (line boxes, tables, lists)
Model: sonnet   Repo: /volumes/repos/claudio   Files: src/tui/reader/mod.rs, src/tui/reader/layout.rs (new)
Goal: AC8, and AC7's "no inline box taller than 3 rows" (F5). `reader::layout::layout(&Doc, &[Formula], width) -> Vec<Line>` (a `Line` is styled `Seg`s). Paragraphs wrap at spaces (a word wider than the line is cut between graphemes; formula boxes are never cut). An inline formula is its text-style box when it has at most one row above and one below its baseline and fits the width, else its one-line form (breakable), else its source dimmed; a line is as tall as its tallest box, baselines aligned. A display formula is its display box two columns in, else its one-line form wrapped, else its source dimmed. Headings bold (level 1 also underlined), emphasis, inline code dimmed, code blocks two columns in, dimmed and cut at the edge, lists with `•`/`n.` and hanging indents (no blank lines inside), quotes with a `│ ` gutter, tables as aligned columns (header bold, `─┼─` rule, cells aligned on their baselines) when they fit, else one `a │ b` paragraph per row, rules as `─`. A grapheme wider than two columns becomes `�` (F9).

Step 1: write the failing tests.
`````diff
diff --git a/src/tui/reader/layout.rs b/src/tui/reader/layout.rs
new file mode 100644
index 0000000..3223bd4
--- /dev/null
+++ b/src/tui/reader/layout.rs
@@ -0,0 +1,201 @@
+#[cfg(test)]
+mod tests {
+    use super::*;
+    use crate::tui::latex::Rendered;
+    use crate::tui::reader::doc::{parse, prepare as prepare_doc};
+
+    fn laid(rows: &[&str], baseline: usize) -> Laid {
+        Laid {
+            rows: rows.iter().map(|r| r.to_string()).collect(),
+            baseline,
+        }
+    }
+
+    /// A formula with these renderings.
+    fn formula(
+        src: &str,
+        inline: Option<&str>,
+        text: Option<Laid>,
+        block: Option<Laid>,
+    ) -> Formula {
+        Formula::new(
+            src,
+            Rendered {
+                inline: inline.map(str::to_owned),
+                block,
+                text,
+            },
+        )
+    }
+
+    /// `markdown` laid out at `width`, with `formulas` for its tags, as text.
+    fn shown(markdown: &str, formulas: &[Formula], width: usize) -> Vec<String> {
+        let doc = parse(&prepare_doc(markdown));
+        layout(&doc, formulas, width)
+            .iter()
+            .map(Line::text)
+            .collect()
+    }
+
+    #[test]
+    fn paragraphs_wrap_at_spaces_and_cut_long_words() {
+        assert_eq!(
+            shown("the quick brown fox jumps over the dog", &[], 15),
+            ["the quick brown", "fox jumps over", "the dog"]
+        );
+        assert_eq!(shown("abcdefghij xy", &[], 4), ["abcd", "efgh", "ij", "xy"]);
+        assert_eq!(shown("one\ntwo  three", &[], 40), ["one two three"]);
+        assert_eq!(shown("hard  \nbreak", &[], 40), ["hard", "break"]);
+        assert_eq!(shown("a\n\nb", &[], 40), ["a", "", "b"]);
+    }
+
+    #[test]
+    fn inline_formulas_are_line_boxes_on_the_baseline() {
+        let s = formula("S_{i,f}", None, Some(laid(&["S", " i,f"], 0)), None);
+        let e = formula("e^{i\\pi}", None, Some(laid(&[" iπ", "e"], 1)), None);
+        let text = "so <latex>S_{i,f}</latex> and <latex>e^{i\\pi}</latex> end";
+        assert_eq!(
+            shown(text, &[s.clone(), e], 40),
+            ["             iπ", "so S    and e   end", "    i,f"]
+        );
+        // A line without boxes is one row; the next line is its own box.
+        assert_eq!(
+            shown("aaaa bbbb <latex>S_{i,f}</latex>", &[s], 10),
+            ["aaaa bbbb", "S", " i,f"]
+        );
+    }
+
+    #[test]
+    fn tall_or_wide_inline_formulas_fall_back_to_one_line() {
+        // 33 rows: a 32-deep nested script.
+        let rows: Vec<String> = (0..33).map(|i| format!("{}x", " ".repeat(i))).collect();
+        let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
+        let deep = formula("deep", Some("x_(…)"), Some(laid(&rows, 0)), None);
+        let lines = shown("a <latex>deep</latex> b", &[deep], 40);
+        assert_eq!(lines, ["a x_(…) b"]);
+        // Wider than the view.
+        let wide = formula("w", Some("w w w"), Some(laid(&["wwwwwwwwww"], 0)), None);
+        assert_eq!(shown("<latex>w</latex>x", &[wide], 8), ["w w wx"]);
+        // Not rendered at all: the source.
+        assert_eq!(
+            shown("a <latex>\\oops</latex>", &[Formula::raw("\\oops")], 40),
+            ["a \\oops"]
+        );
+    }
+
+    #[test]
+    fn display_formulas_are_indented_blocks_with_a_fallback() {
+        let quad = formula(
+            "q",
+            Some("x = (-b ± √(b² - 4ac))/(2a)"),
+            None,
+            Some(laid(
+                &[
+                    "    -b ± √(b² - 4ac)",
+                    "x = ────────────────",
+                    "           2a",
+                ],
+                1,
+            )),
+        );
+        let text = "Roots:\n\n<latex>q</latex>\n\ndone";
+        assert_eq!(
+            shown(text, &[quad.clone()], 40),
+            [
+                "Roots:",
+                "",
+                "      -b ± √(b² - 4ac)",
+                "  x = ────────────────",
+                "             2a",
+                "",
+                "done"
+            ]
+        );
+        // Too wide for the view: the one-line form, wrapped, indented.
+        assert_eq!(
+            shown("<latex>q</latex>", &[quad], 18),
+            ["  x = (-b ± √(b² -", "  4ac))/(2a)"]
+        );
+    }
+
+    #[test]
+    fn lists_quotes_headings_and_rules() {
+        let text = "# Title\n\n- one\n- two words here\n  1. sub\n\n> quoted\n>\n> more\n\n---";
+        assert_eq!(
+            shown(text, &[], 12),
+            [
+                "Title",
+                "",
+                "• one",
+                "• two words",
+                "  here",
+                "  1. sub",
+                "",
+                "│ quoted",
+                "│",
+                "│ more",
+                "",
+                "────────────",
+            ]
+        );
+        let doc = parse(&prepare_doc("# Big\n\n## Small"));
+        let lines = layout(&doc, &[], 20);
+        assert!(lines[0].0[0]
+            .style
+            .add_modifier
+            .contains(Modifier::UNDERLINED));
+        assert!(lines[2].0[0].style.add_modifier.contains(Modifier::BOLD));
+        assert!(!lines[2].0[0]
+            .style
+            .add_modifier
+            .contains(Modifier::UNDERLINED));
+        let ordered = shown("9. a\n10. b", &[], 20);
+        assert_eq!(ordered, ["9.  a", "10. b"]);
+    }
+
+    #[test]
+    fn code_blocks_are_indented_dim_and_cut() {
+        let doc = parse(&prepare_doc("```\nlet x = 1;\n\tlong line here\n```"));
+        let lines = layout(&doc, &[], 12);
+        let text: Vec<String> = lines.iter().map(Line::text).collect();
+        assert_eq!(text, ["  let x = 1;", "      long l"]);
+        assert!(lines[0].0[0].style.add_modifier.contains(Modifier::DIM));
+    }
+
+    #[test]
+    fn tables_align_when_they_fit_and_flow_when_not() {
+        let e = formula(
+            "x",
+            Some("x"),
+            Some(laid(&["x"], 0)),
+            Some(laid(&["a", "─", "b"], 1)),
+        );
+        let text = "| name | value |\n|---|---|\n| one | 1 |\n| frac | <latex>x</latex> |";
+        assert_eq!(
+            shown(text, &[e.clone()], 40),
+            [
+                "name │ value",
+                "─────┼──────",
+                "one  │ 1",
+                "     │ a",
+                "frac │ ─",
+                "     │ b",
+            ]
+        );
+        assert_eq!(
+            shown(text, &[e], 10),
+            ["name │", "value", "one │ 1", "frac │ x"]
+        );
+    }
+
+    #[test]
+    fn wide_clusters_become_one_replacement_character() {
+        // A cluster wider than two columns (two leading jamo and a vowel)
+        // does not take the line with it.
+        let wide = "\u{1100}\u{1100}\u{1161}";
+        assert_eq!(wide.graphemes(true).count(), 1);
+        assert!(wide.width() > 2);
+        let lines = shown(&format!("a {wide} b"), &[], 20);
+        assert_eq!(lines, ["a \u{fffd} b"]);
+    }
+}
diff --git a/src/tui/reader/mod.rs b/src/tui/reader/mod.rs
index 3782c13..6b2c6c1 100644
--- a/src/tui/reader/mod.rs
+++ b/src/tui/reader/mod.rs
@@ -2,5 +2,7 @@
 //! claudio from the transcript, with their formulas typeset.
 //!
 //! - [`doc`] — an answer as markdown blocks (on the render worker).
+//! - [`layout`] — such blocks wrapped to the view's width.
 
 pub mod doc;
+pub mod layout;
`````
Run: `umask 022; cargo test --locked --bin claudio -- tui::reader`
Expected: compilation fails, `error: could not compile `claudio` (bin "claudio" test) due to 19 previous errors; 1 warning emitted`: cannot find `layout`, `Line`, `Laid`, `Formula`, `Modifier`, and no method `graphemes`/`width` on `&str`; the warning is the unused `use super::*` of `layout.rs`.

Step 2: implement.
`````diff
diff --git a/src/tui/reader/layout.rs b/src/tui/reader/layout.rs
index 28f57a4..3223bd4 100644
--- a/src/tui/reader/layout.rs
+++ b/src/tui/reader/layout.rs
@@ -1,3 +1,543 @@
+//! A [`Doc`] wrapped to the reader view's width: styled lines, formulas in
+//! TeX's line boxes. Pure; the view caches the result for its width.
+//!
+//! - Inline formulas are unbreakable boxes in the text flow, at most three
+//!   rows (one above and one below the baseline); a line is as tall as its
+//!   tallest box, baselines aligned. A taller formula, or one wider than
+//!   the view, is its one-line form; one that was not rendered is its
+//!   source, dimmed.
+//! - A display formula is its own block, two columns in; wider than the
+//!   view, it is its one-line form, wrapped.
+//! - Code blocks are never wrapped (cut at the edge); tables are aligned
+//!   columns when they fit, else one `a │ b │ c` paragraph per row.
+
+use ratatui::style::{Modifier, Style};
+use unicode_segmentation::UnicodeSegmentation;
+use unicode_width::UnicodeWidthStr;
+
+use super::doc::{Block, Doc, Emphasis, Span};
+use crate::tui::latex::{prepare, Formula};
+use crate::tui::typeset::Laid;
+
+/// How many rows an inline formula may have above and below its baseline.
+const INLINE_REACH: usize = 1;
+/// The indentation of a display formula, a code block and a quote's text.
+const INDENT: &str = "  ";
+const GUTTER: &str = "│ ";
+const CELL_SEPARATOR: &str = " │ ";
+
+/// One drawn line: styled pieces of text, left to right.
+#[derive(Debug, Clone, Default, PartialEq)]
+pub struct Line(pub Vec<Seg>);
+
+#[derive(Debug, Clone, PartialEq)]
+pub struct Seg {
+    pub text: String,
+    pub style: Style,
+}
+
+impl Line {
+    /// The line's text, styles dropped.
+    pub fn text(&self) -> String {
+        self.0.iter().map(|s| s.text.as_str()).collect()
+    }
+
+    fn width(&self) -> usize {
+        self.0.iter().map(|s| s.text.width()).sum()
+    }
+
+    /// Append `text` in `style`, joined to the last piece when it has the
+    /// same style.
+    fn push(&mut self, text: &str, style: Style) {
+        if text.is_empty() {
+            return;
+        }
+        match self.0.last_mut() {
+            Some(last) if last.style == style => last.text.push_str(text),
+            _ => self.0.push(Seg {
+                text: text.to_owned(),
+                style,
+            }),
+        }
+    }
+
+    /// This line after `prefix` in `style`.
+    fn after(self, prefix: &str, style: Style) -> Line {
+        let mut line = Line::default();
+        line.push(prefix, style);
+        for seg in self.0 {
+            line.push(&seg.text, seg.style);
+        }
+        line
+    }
+}
+
+/// `doc`, whose formulas are `formulas`, as lines at most `width` columns
+/// wide (code blocks and over-deep indentation aside, which the drawing
+/// cuts).
+pub fn layout(doc: &Doc, formulas: &[Formula], width: usize) -> Vec<Line> {
+    Ctx { formulas }.blocks(&doc.blocks, width.max(1), true)
+}
+
+fn dim() -> Style {
+    Style::default().add_modifier(Modifier::DIM)
+}
+
+fn bold() -> Style {
+    Style::default().add_modifier(Modifier::BOLD)
+}
+
+/// `style` with `emphasis` added.
+fn emphasized(style: Style, emphasis: Emphasis) -> Style {
+    let mut style = style;
+    for (on, modifier) in [
+        (emphasis.bold, Modifier::BOLD),
+        (emphasis.italic, Modifier::ITALIC),
+        (emphasis.strike, Modifier::CROSSED_OUT),
+    ] {
+        if on {
+            style = style.add_modifier(modifier);
+        }
+    }
+    style
+}
+
+/// `s` with every grapheme wider than two columns replaced by `�`, so no
+/// line is lost to one (the cells take at most two columns each).
+fn safe(s: &str) -> String {
+    s.graphemes(true)
+        .map(|g| if g.width() > 2 { "\u{fffd}" } else { g })
+        .collect()
+}
+
+/// A piece of a paragraph, for wrapping.
+#[derive(Debug, Clone)]
+enum Item {
+    /// Text without spaces.
+    Text(String, Style),
+    /// A formula box.
+    Box(Laid),
+    /// A space a line may break at.
+    Space,
+    /// A forced line break.
+    Break,
+}
+
+impl Item {
+    fn width(&self) -> usize {
+        match self {
+            Item::Text(t, _) => t.width(),
+            Item::Box(laid) => laid.width(),
+            Item::Space => 1,
+            Item::Break => 0,
+        }
+    }
+}
+
+/// Text as items: words, and one space for each run of whitespace.
+fn words(text: &str, style: Style, out: &mut Vec<Item>) {
+    let mut word = String::new();
+    for c in text.chars() {
+        if c.is_whitespace() {
+            if !word.is_empty() {
+                out.push(Item::Text(safe(&std::mem::take(&mut word)), style));
+            }
+            if !matches!(out.last(), Some(Item::Space) | None) {
+                out.push(Item::Space);
+            }
+        } else {
+            word.push(c);
+        }
+    }
+    if !word.is_empty() {
+        out.push(Item::Text(safe(&word), style));
+    }
+}
+
+/// Whether `laid` may sit in a line of text: three rows at most, one above
+/// and one below its baseline.
+fn fits_inline(laid: &Laid) -> bool {
+    laid.baseline <= INLINE_REACH && laid.rows.len() <= laid.baseline + 1 + INLINE_REACH
+}
+
+struct Ctx<'a> {
+    formulas: &'a [Formula],
+}
+
+impl Ctx<'_> {
+    /// `blocks` as lines; with `spaced`, a blank line between two blocks.
+    fn blocks(&self, blocks: &[Block], width: usize, spaced: bool) -> Vec<Line> {
+        let mut out = Vec::new();
+        for (i, block) in blocks.iter().enumerate() {
+            if i > 0 && spaced {
+                out.push(Line::default());
+            }
+            self.block(block, width, &mut out);
+        }
+        out
+    }
+
+    fn block(&self, block: &Block, width: usize, out: &mut Vec<Line>) {
+        match block {
+            Block::Para(spans) => out.extend(self.para(spans, Style::default(), width)),
+            Block::Heading(level, spans) => {
+                let style = match level {
+                    1 => bold().add_modifier(Modifier::UNDERLINED),
+                    _ => bold(),
+                };
+                out.extend(self.para(spans, style, width));
+            }
+            Block::Code(lines) => {
+                for line in lines {
+                    out.push(cut(&format!("{INDENT}{}", safe(line)), dim(), width));
+                }
+            }
+            Block::List(start, items) => self.list(*start, items, width, out),
+            Block::Quote(inner) => {
+                let inner = self.blocks(inner, width.saturating_sub(GUTTER.width()).max(1), true);
+                for line in inner {
+                    let gutter = if line.0.is_empty() {
+                        GUTTER.trim_end()
+                    } else {
+                        GUTTER
+                    };
+                    out.push(line.after(gutter, dim()));
+                }
+            }
+            Block::Table(rows) => self.table(rows, width, out),
+            Block::Rule => {
+                let mut line = Line::default();
+                line.push(&"─".repeat(width), dim());
+                out.push(line);
+            }
+            Block::Display(i) => self.display(*i, width, out),
+        }
+    }
+
+    /// A paragraph of `spans` in `style`, wrapped to `width`.
+    fn para(&self, spans: &[Span], style: Style, width: usize) -> Vec<Line> {
+        let items = self.items(spans, style, width, false);
+        wrap(&items, width)
+            .iter()
+            .flat_map(|line| render(line).0)
+            .collect()
+    }
+
+    /// `spans` as items. A table cell's display formula (`cells`) is its
+    /// display box.
+    fn items(&self, spans: &[Span], style: Style, width: usize, cells: bool) -> Vec<Item> {
+        let mut out = Vec::new();
+        for span in spans {
+            match span {
+                Span::Text(text, emphasis) => words(text, emphasized(style, *emphasis), &mut out),
+                Span::Code(code) => words(code, style.patch(dim()), &mut out),
+                Span::Formula(i) => self.inline(*i, style, width, &mut out),
+                Span::Display(i) => match self.formulas.get(*i).and_then(|f| f.block.as_ref()) {
+                    Some(block) if cells => out.push(Item::Box(block.clone())),
+                    _ => self.inline(*i, style, width, &mut out),
+                },
+                Span::Break => out.push(Item::Break),
+            }
+        }
+        out
+    }
+
+    /// Formula `i` in a line: its text-style box when that fits a line and
+    /// the width, else its one-line form, else its source dimmed.
+    fn inline(&self, i: usize, style: Style, width: usize, out: &mut Vec<Item>) {
+        let Some(f) = self.formulas.get(i) else {
+            return;
+        };
+        match (&f.text, &f.inline) {
+            (Some(text), _) if fits_inline(text) && text.width() <= width => {
+                out.push(Item::Box(text.clone()))
+            }
+            (_, Some(line)) => words(line, style, out),
+            _ => words(&prepare(&f.src), style.patch(dim()), out),
+        }
+    }
+
+    /// Display formula `i`: its display box, two columns in, when it fits;
+    /// else its one-line form wrapped, else its source dimmed.
+    fn display(&self, i: usize, width: usize, out: &mut Vec<Line>) {
+        let Some(f) = self.formulas.get(i) else {
+            return;
+        };
+        let inner = width.saturating_sub(INDENT.width()).max(1);
+        let lines = match (&f.block, &f.inline) {
+            (Some(block), _) if block.width() <= inner => render(&[Item::Box(block.clone())]).0,
+            (_, Some(line)) => self.text_lines(line, Style::default(), inner),
+            _ => self.text_lines(&prepare(&f.src), dim(), inner),
+        };
+        out.extend(lines.into_iter().map(|l| l.after(INDENT, Style::default())));
+    }
+
+    /// Plain text wrapped to `width`.
+    fn text_lines(&self, text: &str, style: Style, width: usize) -> Vec<Line> {
+        let mut items = Vec::new();
+        words(text, style, &mut items);
+        wrap(&items, width)
+            .iter()
+            .flat_map(|line| render(line).0)
+            .collect()
+    }
+
+    /// A list: each item behind its marker (`•`, or its number), its other
+    /// lines indented as far (a hanging indent); no blank lines.
+    fn list(&self, start: Option<u64>, items: &[Vec<Block>], width: usize, out: &mut Vec<Line>) {
+        let markers: Vec<String> = (0..items.len())
+            .map(|k| match start {
+                Some(n) => format!("{}.", n.saturating_add(k as u64)),
+                None => "•".to_owned(),
+            })
+            .collect();
+        let indent = markers.iter().map(|m| m.width()).max().unwrap_or(1) + 1;
+        for (marker, item) in markers.iter().zip(items) {
+            let lines = self.blocks(item, width.saturating_sub(indent).max(1), false);
+            let first = format!("{marker:<indent$}");
+            if lines.is_empty() {
+                out.push(Line::default().after(first.trim_end(), Style::default()));
+            }
+            for (j, line) in lines.into_iter().enumerate() {
+                let prefix = if j == 0 {
+                    first.clone()
+                } else {
+                    " ".repeat(indent)
+                };
+                out.push(line.after(&prefix, Style::default()));
+            }
+        }
+    }
+
+    /// A table: aligned columns (the header bold, then a rule) when they
+    /// fit `width`, else each row one paragraph of its cells.
+    fn table(&self, rows: &[Vec<Vec<Span>>], width: usize, out: &mut Vec<Line>) {
+        let style = |r: usize| if r == 0 { bold() } else { Style::default() };
+        let cells: Vec<Vec<(Vec<Line>, usize)>> = rows
+            .iter()
+            .enumerate()
+            .map(|(r, row)| {
+                row.iter()
+                    .map(|cell| render(&self.items(cell, style(r), usize::MAX, true)))
+                    .collect()
+            })
+            .collect();
+        let columns = cells.iter().map(Vec::len).max().unwrap_or(0);
+        let widths: Vec<usize> = (0..columns)
+            .map(|c| {
+                let column = cells.iter().filter_map(|row| row.get(c));
+                column
+                    .flat_map(|(lines, _)| lines.iter().map(Line::width))
+                    .max()
+                    .unwrap_or(0)
+            })
+            .collect();
+        let total =
+            widths.iter().sum::<usize>() + CELL_SEPARATOR.width() * columns.saturating_sub(1);
+        if total > width {
+            for (r, row) in rows.iter().enumerate() {
+                let mut items = Vec::new();
+                for (c, cell) in row.iter().enumerate() {
+                    if c > 0 {
+                        items.extend([Item::Space, Item::Text("│".to_owned(), dim()), Item::Space]);
+                    }
+                    items.extend(self.items(cell, style(r), width, false));
+                }
+                out.extend(wrap(&items, width).iter().flat_map(|line| render(line).0));
+            }
+            return;
+        }
+        for (r, row) in cells.iter().enumerate() {
+            let above = row.iter().map(|(_, base)| *base).max().unwrap_or(0);
+            let below = row
+                .iter()
+                .map(|(lines, base)| lines.len().saturating_sub(base + 1))
+                .max()
+                .unwrap_or(0);
+            for k in 0..above + 1 + below {
+                let mut line = Line::default();
+                for (c, w) in widths.iter().enumerate() {
+                    if c > 0 {
+                        line.push(CELL_SEPARATOR, dim());
+                    }
+                    let cell = row.get(c).and_then(|(lines, base)| {
+                        k.checked_sub(above - base).and_then(|i| lines.get(i))
+                    });
+                    let shown = cell.map_or(0, Line::width);
+                    if let Some(cell) = cell {
+                        for seg in &cell.0 {
+                            line.push(&seg.text, seg.style);
+                        }
+                    }
+                    line.push(&" ".repeat(w - shown), Style::default());
+                }
+                out.push(trim_end(line));
+            }
+            if r == 0 && cells.len() > 1 {
+                let rule: Vec<String> = widths.iter().map(|w| "─".repeat(*w)).collect();
+                let mut line = Line::default();
+                line.push(&rule.join("─┼─"), dim());
+                out.push(line);
+            }
+        }
+    }
+}
+
+/// `items` broken into lines at most `width` wide: at spaces, at forced
+/// breaks, and inside a word only when the word alone is wider than the
+/// line (formula boxes are never cut).
+fn wrap(items: &[Item], width: usize) -> Vec<Vec<Item>> {
+    let mut lines = Vec::new();
+    let mut line: Vec<Item> = Vec::new();
+    let mut used = 0;
+    let mut space = false;
+    let mut i = 0;
+    while i < items.len() {
+        match &items[i] {
+            Item::Space => {
+                space = used > 0;
+                i += 1;
+                continue;
+            }
+            Item::Break => {
+                lines.push(std::mem::take(&mut line));
+                (used, space) = (0, false);
+                i += 1;
+                continue;
+            }
+            _ => {}
+        }
+        let end = items[i..]
+            .iter()
+            .position(|it| matches!(it, Item::Space | Item::Break))
+            .map_or(items.len(), |p| i + p);
+        let word = &items[i..end];
+        let word_width: usize = word.iter().map(Item::width).sum();
+        let gap = usize::from(space);
+        if used > 0 && used + gap + word_width > width {
+            lines.push(std::mem::take(&mut line));
+            used = 0;
+        } else if space {
+            line.push(Item::Space);
+            used += 1;
+        }
+        space = false;
+        if word_width <= width - used {
+            line.extend(word.iter().cloned());
+            used += word_width;
+        } else {
+            // A word wider than a line: cut between its graphemes.
+            for item in word {
+                match item {
+                    Item::Text(text, style) => {
+                        let mut piece = String::new();
+                        for g in text.graphemes(true) {
+                            if used + g.width() > width && used > 0 {
+                                if !piece.is_empty() {
+                                    line.push(Item::Text(std::mem::take(&mut piece), *style));
+                                }
+                                lines.push(std::mem::take(&mut line));
+                                used = 0;
+                            }
+                            piece.push_str(g);
+                            used += g.width();
+                        }
+                        if !piece.is_empty() {
+                            line.push(Item::Text(piece, *style));
+                        }
+                    }
+                    other => {
+                        if used > 0 && used + other.width() > width {
+                            lines.push(std::mem::take(&mut line));
+                            used = 0;
+                        }
+                        used += other.width();
+                        line.push(other.clone());
+                    }
+                }
+            }
+        }
+        i = end;
+    }
+    if !line.is_empty() || lines.is_empty() {
+        lines.push(line);
+    }
+    lines
+}
+
+/// One wrapped line of items as rows, and which row is the baseline: as
+/// tall as its tallest box, text on the baseline.
+fn render(items: &[Item]) -> (Vec<Line>, usize) {
+    let above = items
+        .iter()
+        .map(|it| match it {
+            Item::Box(laid) => laid.baseline,
+            _ => 0,
+        })
+        .max()
+        .unwrap_or(0);
+    let below = items
+        .iter()
+        .map(|it| match it {
+            Item::Box(laid) => laid.rows.len() - 1 - laid.baseline,
+            _ => 0,
+        })
+        .max()
+        .unwrap_or(0);
+    let rows = (0..above + 1 + below)
+        .map(|row| {
+            let mut line = Line::default();
+            for item in items {
+                let width = item.width();
+                match item {
+                    Item::Text(text, style) if row == above => line.push(text, *style),
+                    Item::Box(laid) => {
+                        let text = (row + laid.baseline)
+                            .checked_sub(above)
+                            .and_then(|i| laid.rows.get(i))
+                            .map_or(String::new(), |r| safe(r));
+                        let pad = width.saturating_sub(text.width());
+                        line.push(&text, Style::default());
+                        line.push(&" ".repeat(pad), Style::default());
+                    }
+                    _ => line.push(&" ".repeat(width), Style::default()),
+                }
+            }
+            trim_end(line)
+        })
+        .collect();
+    (rows, above)
+}
+
+/// `line` without trailing blanks.
+fn trim_end(mut line: Line) -> Line {
+    while let Some(last) = line.0.last_mut() {
+        let kept = last.text.trim_end().len();
+        if kept > 0 {
+            last.text.truncate(kept);
+            break;
+        }
+        line.0.pop();
+    }
+    line
+}
+
+/// `text` in `style`, cut to `width` columns.
+fn cut(text: &str, style: Style, width: usize) -> Line {
+    let mut kept = String::new();
+    let mut used = 0;
+    for g in text.graphemes(true) {
+        if used + g.width() > width {
+            break;
+        }
+        used += g.width();
+        kept.push_str(g);
+    }
+    let mut line = Line::default();
+    line.push(kept.trim_end(), style);
+    line
+}
+
 #[cfg(test)]
 mod tests {
     use super::*;
`````
Run: `umask 022; cargo build --locked 2>&1 | grep -cE '^(warning|error)'; cargo test --locked --bin claudio -- tui::reader && rustfmt --edition 2021 --check src/tui/reader/layout.rs src/tui/reader/mod.rs && make test-unit`
Expected: `0`, then `test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 860 filtered out`, then `test result: ok. 875 passed; 0 failed`.

Step 3: commit
`/usr/bin/git add src/tui/reader/layout.rs && /usr/bin/git commit -m "feat(tui): lay reader documents out with formula line boxes" -- src/tui/reader/mod.rs src/tui/reader/layout.rs`

Interfaces this task provides to later tasks: `tui::reader::layout::{layout(&Doc, &[Formula], usize) -> Vec<Line>, Line(pub Vec<Seg>), Line::text() (tests), Seg { pub text: String, pub style: Style }, safe(&str) -> String}` (`safe` becomes `pub` in Task 7).

### Task 6: the reader view's state
Model: haiku   Repo: /volumes/repos/claudio   Files: src/tui/reader/mod.rs
Goal: AC9 (keys, answers navigation, Tab list mode, scroll), F11 (layout cached for the current width only) and F12 (header prompt on one line, cut with `…`; "too narrow" below 20 columns; mouse wheel; Tab shows the session's v0.4.0 `latex_srcs` list). `ReaderView { session, host, gen, content: Content::{Loading, Failed, Ready}, … }` with `set_answers` (opens on the newest, at its top), `set_failed`, `on_key` (↑/↓ j/k, PgUp/PgDn/Space, Home/g, End/G, ←/→ h/l, Tab, Esc/q close; Ctrl/Alt chords are not its own), `on_wheel`, `scroll`/`set_max_scroll`, `header(width)` and `body(session, width)` (the layout, cached per answer for one width; the formulas list; or "Loading…", the failure text, "No answers yet", "too narrow"). `RenderedAnswer { prompt, doc, formulas }` is what the worker will produce. Pure; nothing opens it until Task 7.

Step 1: write the failing tests.
`````diff
diff --git a/src/tui/reader/mod.rs b/src/tui/reader/mod.rs
--- a/src/tui/reader/mod.rs
+++ b/src/tui/reader/mod.rs
@@ -6,3 +6,146 @@
 
 pub mod doc;
 pub mod layout;
+
+#[cfg(test)]
+mod tests {
+    use super::*;
+    use crate::proto::SessionKind;
+    use crate::tui::latex::Rendered;
+
+    fn key(code: KeyCode) -> KeyEvent {
+        KeyEvent::new(code, KeyModifiers::NONE)
+    }
+
+    fn answer(prompt: &str, markdown: &str) -> RenderedAnswer {
+        let prepared = doc::prepare(markdown);
+        RenderedAnswer {
+            prompt: prompt.into(),
+            doc: doc::parse(&prepared),
+            formulas: Vec::new(),
+        }
+    }
+
+    fn texts(lines: &[Line]) -> Vec<String> {
+        lines.iter().map(Line::text).collect()
+    }
+
+    fn ready(answers: Vec<RenderedAnswer>) -> ReaderView {
+        let mut view = ReaderView::new(uuid::Uuid::nil(), "local", 1);
+        view.set_answers(answers);
+        view
+    }
+
+    #[test]
+    fn it_opens_on_the_newest_answer_and_moves_between_them() {
+        let mut view = ready(vec![answer("first", "one"), answer("second", "two")]);
+        assert_eq!(view.header(40).text(), "answer 2/2 · second");
+        assert_eq!(texts(&view.body(None, 40)), ["two"]);
+        view.set_max_scroll(5);
+        view.on_key(&key(KeyCode::Down));
+        assert_eq!(view.scroll(), 1);
+        // A move to another answer starts at its top.
+        view.on_key(&key(KeyCode::Left));
+        assert_eq!(view.header(40).text(), "answer 1/2 · first");
+        assert_eq!(view.scroll(), 0);
+        view.on_key(&key(KeyCode::Char('h')));
+        assert_eq!(view.header(40).text(), "answer 1/2 · first", "none before");
+        view.on_key(&key(KeyCode::Char('l')));
+        view.on_key(&key(KeyCode::Right));
+        assert_eq!(view.header(40).text(), "answer 2/2 · second", "none after");
+    }
+
+    #[test]
+    fn keys_scroll_and_close() {
+        let mut view = ready(vec![answer("p", "x")]);
+        view.set_max_scroll(30);
+        assert_eq!(view.scroll(), 0, "opens at the top");
+        view.on_key(&key(KeyCode::PageDown));
+        assert_eq!(view.scroll(), 10);
+        view.on_key(&key(KeyCode::Char(' ')));
+        view.on_key(&key(KeyCode::Char('k')));
+        assert_eq!(view.scroll(), 19);
+        view.on_key(&key(KeyCode::PageUp));
+        assert_eq!(view.scroll(), 9);
+        view.on_key(&key(KeyCode::End));
+        assert_eq!(view.scroll(), 30);
+        view.on_key(&key(KeyCode::Char('g')));
+        assert_eq!(view.scroll(), 0);
+        view.on_key(&key(KeyCode::Char('G')));
+        view.on_key(&key(KeyCode::Home));
+        assert_eq!(view.scroll(), 0);
+        view.on_wheel(true);
+        view.on_wheel(true);
+        view.on_wheel(false);
+        assert_eq!(view.scroll(), 3);
+        // A chord is not the view's.
+        let alt_k = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::ALT);
+        assert!(!view.on_key(&alt_k));
+        assert_eq!(view.scroll(), 3);
+        assert!(view.on_key(&key(KeyCode::Char('q'))));
+        assert!(view.on_key(&key(KeyCode::Esc)));
+    }
+
+    #[test]
+    fn the_header_puts_the_prompt_on_one_cut_line() {
+        let view = ready(vec![answer("why  is\nthe sky\tblue at all", "x")]);
+        assert_eq!(
+            view.header(80).text(),
+            "answer 1/1 · why is the sky blue at all"
+        );
+        assert_eq!(view.header(24).text(), "answer 1/1 · why is the…");
+        assert_eq!(view.header(12).text(), "answer 1/1");
+        let header = view.header(80);
+        assert!(header.0[1].style.add_modifier.contains(Modifier::DIM));
+    }
+
+    #[test]
+    fn the_body_says_what_it_is_waiting_for() {
+        let mut view = ReaderView::new(uuid::Uuid::nil(), "local", 1);
+        assert_eq!(texts(&view.body(None, 40)), [LOADING]);
+        let old = "daemon on local is older: run `claudio daemon restart`";
+        view.set_failed(old);
+        assert_eq!(texts(&view.body(None, 60)), [old]);
+        view.set_answers(Vec::new());
+        assert_eq!(texts(&view.body(None, 40)), [NO_ANSWERS]);
+        assert_eq!(view.header(40).text(), "reader");
+        let view = ready(vec![answer("p", "text")]);
+        assert_eq!(texts(&view.body(None, MIN_WIDTH - 1)), [TOO_NARROW]);
+    }
+
+    #[test]
+    fn a_layout_is_kept_for_its_width_only() {
+        let view = ready(vec![answer("p", "a long line of words to wrap")]);
+        let first = view.body(None, 40);
+        assert!(Rc::ptr_eq(&first, &view.body(None, 40)), "cached");
+        let narrow = view.body(None, 20);
+        assert_eq!(texts(&narrow), ["a long line of words", "to wrap"]);
+        assert!(
+            !Rc::ptr_eq(&first, &view.body(None, 40)),
+            "a resize starts over"
+        );
+        assert_eq!(view.layouts.borrow().1.len(), 1);
+    }
+
+    #[test]
+    fn tab_shows_the_sessions_formulas_list() {
+        let size = (24, 80);
+        let mut v = SessionView::new(uuid::Uuid::nil(), "local", "/w", SessionKind::Claude, size);
+        v.latex_srcs = vec!["x^2".into()];
+        let rendered = Rendered {
+            inline: Some("x²".into()),
+            ..Rendered::default()
+        };
+        v.latex = vec![Formula::new("x^2", rendered)];
+        let mut view = ready(vec![answer("p", "text")]);
+        view.on_key(&key(KeyCode::Tab));
+        assert_eq!(view.header(40).text(), "formulas");
+        assert_eq!(texts(&view.body(Some(&v), 40)), ["x²", "x^2"]);
+        assert_eq!(view.scroll, u16::MAX, "the list opens on its newest");
+        // ←/→ stay put in the list; Tab goes back to the answer's top.
+        view.on_key(&key(KeyCode::Left));
+        view.on_key(&key(KeyCode::Tab));
+        assert_eq!(texts(&view.body(Some(&v), 40)), ["text"]);
+        assert_eq!(view.scroll(), 0);
+    }
+}
`````
Run: `umask 022; cargo test --locked --bin claudio -- tui::reader`
Expected: compilation fails, `error: could not compile `claudio` (bin "claudio" test) due to 42 previous errors`: cannot find `ReaderView`, `RenderedAnswer`, `Line`, `LOADING`, `NO_ANSWERS`, `TOO_NARROW`, `MIN_WIDTH`, `KeyCode`, `KeyEvent`, `KeyModifiers`, `Rc`, `Formula`, `SessionView`, `Modifier`.

Step 2: implement.
`````diff
diff --git a/src/tui/reader/mod.rs b/src/tui/reader/mod.rs
index 1f34262..b0765b3 100644
--- a/src/tui/reader/mod.rs
+++ b/src/tui/reader/mod.rs
@@ -1,12 +1,273 @@
 //! The reader view (Alt+u): the session's recent answers, rendered by
-//! claudio from the transcript, with their formulas typeset.
+//! claudio from the transcript, with their formulas typeset. Its state and
+//! keys are here (pure); drawing is in `ui/reader.rs`.
 //!
 //! - [`doc`] — an answer as markdown blocks (on the render worker).
 //! - [`layout`] — such blocks wrapped to the view's width.
+//! - [`ReaderView`] — which answer, how far down, and the formulas list
+//!   (Tab: the v0.4.0 popup's content).
 
 pub mod doc;
 pub mod layout;
 
+use std::cell::{Cell, RefCell};
+use std::collections::HashMap;
+use std::rc::Rc;
+
+use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
+use ratatui::style::{Modifier, Style};
+use unicode_width::UnicodeWidthStr;
+
+use super::formulas;
+use super::latex::Formula;
+use super::sessions::SessionView;
+use crate::proto::SessionId;
+use layout::{Line, Seg};
+
+/// How far PageUp/PageDown (and Space) scroll, and the mouse wheel.
+const PAGE: u16 = 10;
+const WHEEL: u16 = 3;
+/// Below this many columns the view shows only [`TOO_NARROW`].
+pub const MIN_WIDTH: usize = 20;
+const TOO_NARROW: &str = "too narrow";
+const LOADING: &str = "Loading…";
+const NO_ANSWERS: &str = "No answers yet";
+
+/// One answer, ready to lay out: from the render worker.
+#[derive(Debug, Clone, PartialEq)]
+pub struct RenderedAnswer {
+    /// The prompt, as the daemon sent it.
+    pub prompt: String,
+    pub doc: doc::Doc,
+    /// The formulas the doc's indices point at.
+    pub formulas: Vec<Formula>,
+}
+
+/// What the view has to show.
+#[derive(Debug, Clone, PartialEq)]
+pub enum Content {
+    /// The reply, or its rendering, is on its way.
+    Loading,
+    /// The request failed: why (an old daemon's restart notice, …).
+    Failed(String),
+    /// The answers, oldest first.
+    Ready(Vec<RenderedAnswer>),
+}
+
+/// The reader's state: whose answers, which one, and how far down.
+pub struct ReaderView {
+    pub session: SessionId,
+    pub host: String,
+    /// The `App::modal_gen` it was opened under: replies for another are
+    /// dropped.
+    pub gen: u64,
+    pub content: Content,
+    /// The answer shown (an index into `Ready`).
+    answer: usize,
+    /// Showing the formulas list (Tab) instead of an answer.
+    list: bool,
+    scroll: u16,
+    /// The largest useful scroll, known once drawn (see `ui/reader.rs`).
+    max_scroll: Cell<u16>,
+    /// The answers laid out for one width (a resize starts over).
+    layouts: RefCell<(usize, HashMap<usize, Rc<Vec<Line>>>)>,
+}
+
+impl ReaderView {
+    /// The view of `session` on `host`, waiting for its answers.
+    pub fn new(session: SessionId, host: impl Into<String>, gen: u64) -> ReaderView {
+        ReaderView {
+            session,
+            host: host.into(),
+            gen,
+            content: Content::Loading,
+            answer: 0,
+            list: false,
+            scroll: 0,
+            max_scroll: Cell::new(u16::MAX),
+            layouts: RefCell::new((0, HashMap::new())),
+        }
+    }
+
+    /// Show `answers` (oldest first), from the top of the newest.
+    pub fn set_answers(&mut self, answers: Vec<RenderedAnswer>) {
+        self.answer = answers.len().saturating_sub(1);
+        self.content = Content::Ready(answers);
+        self.layouts.borrow_mut().1.clear();
+        if !self.list {
+            self.scroll = 0;
+        }
+    }
+
+    /// Show why there are no answers.
+    pub fn set_failed(&mut self, why: impl Into<String>) {
+        self.content = Content::Failed(why.into());
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
+    /// Handle a key; `true` closes the view. Ctrl and Alt chords are not
+    /// its own (a manager key reaches the app before this).
+    pub fn on_key(&mut self, key: &KeyEvent) -> bool {
+        if key
+            .modifiers
+            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
+        {
+            return false;
+        }
+        let at = self.scroll();
+        let answers = match &self.content {
+            Content::Ready(answers) => answers.len(),
+            _ => 0,
+        };
+        self.scroll = match key.code {
+            KeyCode::Esc | KeyCode::Char('q') => return true,
+            KeyCode::Down | KeyCode::Char('j') => at.saturating_add(1),
+            KeyCode::Up | KeyCode::Char('k') => at.saturating_sub(1),
+            KeyCode::PageDown | KeyCode::Char(' ') => at.saturating_add(PAGE),
+            KeyCode::PageUp => at.saturating_sub(PAGE),
+            KeyCode::Home | KeyCode::Char('g') => 0,
+            KeyCode::End | KeyCode::Char('G') => u16::MAX,
+            KeyCode::Left | KeyCode::Char('h') if !self.list && self.answer > 0 => {
+                self.answer -= 1;
+                0
+            }
+            KeyCode::Right | KeyCode::Char('l') if !self.list && self.answer + 1 < answers => {
+                self.answer += 1;
+                0
+            }
+            KeyCode::Tab => {
+                self.list = !self.list;
+                // The list opens on its newest formulas, an answer on its top.
+                if self.list {
+                    u16::MAX
+                } else {
+                    0
+                }
+            }
+            _ => at,
+        };
+        false
+    }
+
+    /// The mouse wheel: scroll down (`down`) or up.
+    pub fn on_wheel(&mut self, down: bool) {
+        let at = self.scroll();
+        self.scroll = if down {
+            at.saturating_add(WHEEL)
+        } else {
+            at.saturating_sub(WHEEL)
+        };
+    }
+
+    /// The header row, at most `width` columns: `answer k/N · <prompt>`
+    /// (the prompt dimmed, on one line, cut with `…`), or what the body is.
+    pub fn header(&self, width: usize) -> Line {
+        let mut line = Line::default();
+        let push = |line: &mut Line, text: &str, style: Style| {
+            line.0.push(Seg {
+                text: text.to_owned(),
+                style,
+            })
+        };
+        match &self.content {
+            _ if self.list => push(&mut line, "formulas", Style::default()),
+            Content::Ready(answers) if !answers.is_empty() => {
+                let label = format!("answer {}/{}", self.answer + 1, answers.len());
+                let prompt = answers[self.answer]
+                    .prompt
+                    .split_whitespace()
+                    .collect::<Vec<_>>()
+                    .join(" ");
+                let room = width.saturating_sub(label.width() + 3);
+                push(&mut line, &label, Style::default());
+                if room > 0 && !prompt.is_empty() {
+                    let dim = Style::default().add_modifier(Modifier::DIM);
+                    push(&mut line, &format!(" · {}", fit(&prompt, room)), dim);
+                }
+            }
+            _ => push(&mut line, "reader", Style::default()),
+        }
+        line
+    }
+
+    /// The body's lines at `width`: the answer laid out (cached for the
+    /// width), the formulas list of `session`, or a message.
+    pub fn body(&self, session: Option<&SessionView>, width: usize) -> Rc<Vec<Line>> {
+        let message = |text: &str| {
+            let dim = Style::default().add_modifier(Modifier::DIM);
+            Rc::new(vec![Line(vec![Seg {
+                text: text.to_owned(),
+                style: dim,
+            }])])
+        };
+        if width < MIN_WIDTH {
+            return message(TOO_NARROW);
+        }
+        if self.list {
+            return Rc::new(session.map(list_lines).unwrap_or_default());
+        }
+        let answers = match &self.content {
+            Content::Loading => return message(LOADING),
+            Content::Failed(why) => return message(why),
+            Content::Ready(answers) => answers,
+        };
+        let Some(answer) = answers.get(self.answer) else {
+            return message(NO_ANSWERS);
+        };
+        let mut layouts = self.layouts.borrow_mut();
+        if layouts.0 != width {
+            *layouts = (width, HashMap::new());
+        }
+        let laid = layouts
+            .1
+            .entry(self.answer)
+            .or_insert_with(|| Rc::new(layout::layout(&answer.doc, &answer.formulas, width)));
+        Rc::clone(laid)
+    }
+}
+
+/// `text` cut to `width` columns, with `…` when cut.
+fn fit(text: &str, width: usize) -> String {
+    if text.width() <= width {
+        return text.to_owned();
+    }
+    let mut out = String::new();
+    for c in text.chars() {
+        if out.width() + c.to_string().width() + 1 > width {
+            break;
+        }
+        out.push(c);
+    }
+    out.push('…');
+    out
+}
+
+/// The formulas list (Tab): the v0.4.0 popup's lines for `session`.
+fn list_lines(session: &SessionView) -> Vec<Line> {
+    let dim = Style::default().add_modifier(Modifier::DIM);
+    formulas::lines(session)
+        .into_iter()
+        .map(|line| {
+            let (text, style) = match line {
+                formulas::Line::Formula(text) => (text, Style::default()),
+                formulas::Line::Source(text) => (text, dim),
+                formulas::Line::Hint(text) => (text.to_owned(), dim),
+                formulas::Line::Blank => return Line::default(),
+            };
+            Line(vec![Seg { text, style }])
+        })
+        .collect()
+}
+
 #[cfg(test)]
 mod tests {
     use super::*;
`````
Run: `umask 022; cargo build --locked 2>&1 | grep -cE '^(warning|error)'; cargo test --locked --bin claudio -- tui::reader && rustfmt --edition 2021 --check src/tui/reader/mod.rs && make test-unit`
Expected: `0`, then `test result: ok. 21 passed; 0 failed; 0 ignored; 0 measured; 860 filtered out`, then `test result: ok. 881 passed; 0 failed`.

Step 3: commit
`/usr/bin/git commit -m "feat(tui): reader view state, keys and formulas list" -- src/tui/reader/mod.rs`

Interfaces this task provides to later tasks: `tui::reader::{ReaderView, ReaderView::{new(SessionId, host, gen), set_answers, set_failed, on_key(&KeyEvent) -> bool, on_wheel(bool), scroll, set_max_scroll, header(usize) -> Line, body(Option<&SessionView>, usize) -> Rc<Vec<Line>>}, Content, RenderedAnswer, MIN_WIDTH}`.

### Task 7: Alt+u opens the reader: worker, App wiring, drawing
Model: sonnet   Repo: /volumes/repos/claudio   Files: src/cli.rs, src/tui/app.rs, src/tui/formulas.rs, src/tui/interaction.rs, src/tui/keymap.rs, src/tui/mod.rs, src/tui/reader/layout.rs, src/tui/ui/formulas.rs (deleted), src/tui/ui/mod.rs, src/tui/ui/reader.rs (new), tests/manager.rs
Goal: AC1 (the restart notice), AC7 (hostile answers through the worker), AC9 (modal, plain mode, the `formulas` name in `[keys]`, no collision), F10 (the action keeps its config name `formulas` and its Rust name `Action::Formulas`; only its help label becomes `reader view`), F12 (stale replies dropped; `Error "no such session…"` shown as it is). The `latex-render` worker takes `RenderJob::Latex` or `RenderJob::Answers` and hands back `Rendered::Latex`/`Rendered::Answers`; `render_answers` prepares, renders (through the same `RenderCache` and guard) and parses each answer. `Effect::RenderAnswers { gen, id, answers }` → `HostEvent::AnswersRendered { gen, id, answers }` → `App::on_answers_rendered`. Alt+u (`open_reader`) bumps `modal_gen`, opens `Modal::Reader(Box<ReaderView>)` and requests `Msg::Answers` (`ReplyTo::Answers { gen, id }`); the reply becomes `RenderAnswers` (none to render: shown at once) or the failure text (`older_daemon_notice` for an old daemon). The reader yields to actions, Alt+u closes it, it takes the mouse wheel, ignores paste, and draws over the pane (`ui/reader.rs`: a reversed header row, then the body from the scroll; every string sanitized again and made `safe` before it becomes cells; cut at the edge). The formulas popup goes: `FormulasView` and `ui/formulas.rs` are removed, `formulas.rs` keeps the list's lines for Tab. `Line::text` becomes test-only. The `--plain` help line and the help label follow. The existing manager test `test_latex_formula_is_rendered_in_the_pane_and_the_popup` becomes `…_and_the_reader_list` (Alt+u shows "No answers yet" for its prompt-less transcript, Tab lists `E = mc^2`).

Step 1: write the failing tests (worker, App, keymap, drawing, and the updated manager test).
`````diff
diff --git a/src/tui/formulas.rs b/src/tui/formulas.rs
index a06889f..10f09f0 100644
--- a/src/tui/formulas.rs
+++ b/src/tui/formulas.rs
@@ -125,7 +64,6 @@ mod tests {
     use crate::proto::SessionKind;
     use crate::tui::latex::{Formula, Rendered};
     use crate::tui::typeset::Laid;
-    use crossterm::event::KeyEvent;
 
     fn view_with(srcs: &[&str], rendered: Vec<Formula>) -> SessionView {
         let mut v = SessionView::new(
@@ -184,29 +122,4 @@ mod tests {
         assert_eq!(text[0], Line::Hint("No formulas yet."));
         assert!(text.contains(&Line::Hint(EMPTY[3])));
     }
-
-    #[test]
-    fn keys_scroll_and_close() {
-        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
-        let mut view = FormulasView::new(uuid::Uuid::nil());
-        view.set_max_scroll(30);
-        assert_eq!(view.scroll(), 30, "opens at the newest");
-        assert!(!view.on_key(&key(KeyCode::Up)));
-        assert_eq!(view.scroll(), 29);
-        view.on_key(&key(KeyCode::PageUp));
-        assert_eq!(view.scroll(), 19);
-        view.on_key(&key(KeyCode::Char('g')));
-        assert_eq!(view.scroll(), 0);
-        view.on_key(&key(KeyCode::Char('j')));
-        view.on_key(&key(KeyCode::Char(' ')));
-        assert_eq!(view.scroll(), 11);
-        view.on_key(&key(KeyCode::End));
-        assert_eq!(view.scroll(), 30);
-        // A chord is not the popup's.
-        let alt_k = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::ALT);
-        assert!(!view.on_key(&alt_k));
-        assert_eq!(view.scroll(), 30);
-        assert!(view.on_key(&key(KeyCode::Char('q'))));
-        assert!(view.on_key(&key(KeyCode::Esc)));
-    }
 }
diff --git a/src/tui/interaction.rs b/src/tui/interaction.rs
index 80e0214..61d9206 100644
--- a/src/tui/interaction.rs
+++ b/src/tui/interaction.rs
@@ -1196,14 +1261,26 @@ mod tests {
     }
 
     #[test]
-    fn alt_u_opens_and_closes_the_formulas_popup() {
+    fn alt_u_opens_and_closes_the_reader() {
         use crate::tui::test_support::alt;
         let mut app = app_with(&[info(Some(1), None)]);
+        let id = app.sessions[0].id;
         app.on_terminal(alt('u'));
-        let Some(Modal::Formulas(view)) = &app.modal else {
-            panic!("no formulas popup")
+        let Some(Modal::Reader(view)) = &app.modal else {
+            panic!("no reader")
         };
-        assert_eq!(view.session, app.sessions[0].id);
+        assert_eq!(view.session, id);
+        // It asks the session's daemon for the answers.
+        let gen = view.gen;
+        let effects = app.take_effects();
+        assert!(
+            matches!(
+                effects.as_slice(),
+                [Effect::Request { host, msg: Msg::Answers { id: asked }, to }]
+                    if host == "local" && *asked == id && *to == ReplyTo::Answers { gen, id }
+            ),
+            "{effects:?}"
+        );
         // Its own key closes it; so does Esc.
         app.on_terminal(alt('u'));
         assert!(app.modal.is_none());
@@ -1213,12 +1290,95 @@ mod tests {
         // It is read-only: another manager key closes it and acts, and a
         // paste goes nowhere.
         app.on_terminal(alt('u'));
+        app.take_effects();
         app.on_terminal(Event::Paste("text".into()));
-        assert!(app.take_effects().is_empty(), "the paste reached the session");
+        assert!(
+            app.take_effects().is_empty(),
+            "the paste reached the session"
+        );
         app.on_terminal(alt('h'));
         assert!(matches!(app.modal, Some(Modal::Help)));
     }
 
+    #[test]
+    fn answers_for_another_view_are_dropped() {
+        use crate::proto::Answer;
+        use crate::tui::reader::Content;
+        use crate::tui::test_support::alt;
+        let mut app = app_with(&[info(Some(1), None)]);
+        let id = app.sessions[0].id;
+        let reply = |answers: Vec<Answer>| Ok(Msg::AnswersReply { id, answers });
+        let one = vec![Answer {
+            prompt: "p".into(),
+            text: "t".into(),
+        }];
+        let renders = |app: &mut App| {
+            app.take_effects()
+                .into_iter()
+                .filter(|e| matches!(e, Effect::RenderAnswers { .. }))
+                .count()
+        };
+        app.on_terminal(alt('u'));
+        let first = app.modal_gen;
+        app.on_terminal(press(KeyCode::Esc));
+        // The view was closed: its reply goes nowhere.
+        app.on_reply(ReplyTo::Answers { gen: first, id }, reply(one.clone()));
+        assert_eq!(renders(&mut app), 0);
+        // Reopened: only the reply for the new view counts.
+        app.on_terminal(alt('u'));
+        let second = app.modal_gen;
+        app.on_reply(ReplyTo::Answers { gen: first, id }, reply(one.clone()));
+        let other = uuid::Uuid::new_v4();
+        app.on_reply(
+            ReplyTo::Answers {
+                gen: second,
+                id: other,
+            },
+            reply(one.clone()),
+        );
+        assert_eq!(renders(&mut app), 0);
+        app.on_reply(ReplyTo::Answers { gen: second, id }, reply(one));
+        assert_eq!(renders(&mut app), 1);
+        let content = |app: &App| match &app.modal {
+            Some(Modal::Reader(view)) => view.content.clone(),
+            _ => panic!("no reader"),
+        };
+        assert_eq!(content(&app), Content::Loading, "until rendered");
+        app.on_answers_rendered(first, id, Vec::new());
+        assert_eq!(content(&app), Content::Loading, "another view's rendering");
+        app.on_answers_rendered(second, id, Vec::new());
+        assert_eq!(content(&app), Content::Ready(Vec::new()));
+        // An empty reply needs no rendering; an error is shown as it is.
+        app.on_terminal(press(KeyCode::Esc));
+        app.on_terminal(alt('u'));
+        let gen = app.modal_gen;
+        app.on_reply(ReplyTo::Answers { gen, id }, reply(Vec::new()));
+        assert_eq!(
+            (renders(&mut app), content(&app)),
+            (0, Content::Ready(Vec::new()))
+        );
+        let missing = std::io::Error::other(format!("no such session: {id}"));
+        app.on_reply(ReplyTo::Answers { gen, id }, Err(missing));
+        assert_eq!(
+            content(&app),
+            Content::Failed(format!("no such session: {id}"))
+        );
+    }
+
+    #[test]
+    fn the_mouse_wheel_scrolls_the_reader() {
+        use crate::tui::test_support::alt;
+        let mut app = app_with(&[info(Some(1), None)]);
+        app.on_terminal(alt('u'));
+        app.on_terminal(mouse(MouseEventKind::ScrollDown, 10, 5));
+        app.on_terminal(mouse(MouseEventKind::ScrollDown, 10, 5));
+        app.on_terminal(mouse(MouseEventKind::ScrollUp, 10, 5));
+        let Some(Modal::Reader(view)) = &app.modal else {
+            panic!("no reader")
+        };
+        assert_eq!(view.scroll(), 3);
+    }
+
     #[test]
     fn the_selection_is_drawn_reversed() {
         let mut app = app_with_text();
diff --git a/src/tui/keymap.rs b/src/tui/keymap.rs
index 70a85ae..5d6f652 100644
--- a/src/tui/keymap.rs
+++ b/src/tui/keymap.rs
@@ -599,6 +601,24 @@ mod tests {
         assert_eq!(Action::Formulas.name(), "formulas");
     }
 
+    /// The reader view keeps the v0.4.0 popup's config name, `formulas`.
+    #[test]
+    fn the_reader_view_is_bound_as_formulas() {
+        let mut overrides = std::collections::BTreeMap::new();
+        overrides.insert("formulas".to_owned(), "alt+y".to_owned());
+        let mut notices = Vec::new();
+        let km = Keymap::build(&overrides, &mut notices);
+        assert!(notices.is_empty(), "{notices:?}");
+        let alt = |c| key(KeyCode::Char(c), KeyModifiers::ALT);
+        assert_eq!(km.lookup(&alt('y')), Some(Action::Formulas));
+        assert_eq!(km.lookup(&alt('u')), None);
+        let help = Keymap::default().help_entries();
+        assert!(
+            help.contains(&("Alt+u".to_owned(), "reader view")),
+            "{help:?}"
+        );
+    }
+
     /// No two default bindings of one scope share a key.
     #[test]
     fn default_bindings_never_collide() {
diff --git a/src/tui/mod.rs b/src/tui/mod.rs
index 290f04f..b1803d5 100644
--- a/src/tui/mod.rs
+++ b/src/tui/mod.rs
@@ -1414,16 +1491,20 @@ mod tests {
     #[test]
     fn the_worker_renders_each_job_and_passes_its_seq_on() {
         let (tx, rx) = std::sync::mpsc::channel();
-        let jobs = spawn_latex_worker(move |job, formulas| {
-            let _ = tx.send((job.seq, formulas));
+        let jobs = spawn_latex_worker(move |done| {
+            if let Rendered::Latex(job, formulas) = done {
+                let _ = tx.send((job.seq, formulas));
+            }
         })
         .unwrap();
         let id = uuid::Uuid::new_v4();
-        let job = |seq, srcs: Vec<String>| LatexJob {
-            host: "local".into(),
-            id,
-            seq,
-            srcs,
+        let job = |seq, srcs: Vec<String>| {
+            RenderJob::Latex(LatexJob {
+                host: "local".into(),
+                id,
+                seq,
+                srcs,
+            })
         };
         let deep = "\\frac ".repeat(300);
         jobs.send(job(1, vec!["E = mc^2".into()])).unwrap();
@@ -1438,6 +1519,93 @@ mod tests {
         assert_eq!(second[1], latex::Formula::raw(&deep), "too complex: raw");
     }
 
+    /// AC7: answers go through the same worker and guard. Hostile ones
+    /// (thousands of tags, a 32-deep nested script, the hostile formula
+    /// corpus, a 64 KiB line, private-use input) come back bounded and
+    /// sanitized, and lay out with no inline box taller than three rows.
+    #[test]
+    fn hostile_answers_are_rendered_bounded_and_sanitized() {
+        use crate::proto::Answer;
+        use reader::layout::layout;
+        let (tx, rx) = std::sync::mpsc::channel();
+        let jobs = spawn_latex_worker(move |done| {
+            if let Rendered::Answers(job, answers) = done {
+                let _ = tx.send((job.gen, answers));
+            }
+        })
+        .unwrap();
+        let nested = format!("x{}{}", "_{a".repeat(32), "}".repeat(32));
+        let corpus = [
+            "\\frac ".repeat(340),
+            "x/".repeat(255),
+            "x\u{1b}[2J\u{1b}]52;c;AAAA\u{7}y".to_owned(),
+            "a\u{202e}b\u{200b}c".to_owned(),
+        ];
+        let texts = [
+            "<latex>x</latex> ".repeat(4000),
+            format!("inline <latex>{nested}</latex> end\n\n<latex>{nested}</latex>"),
+            corpus
+                .iter()
+                .map(|f| format!("<latex>{f}</latex>\n\n"))
+                .collect(),
+            "w".repeat(64 * 1024),
+            "\u{e000}forged\u{e001} <latex>\u{e002}y</latex>".to_owned(),
+        ];
+        let answers: Vec<Answer> = texts
+            .iter()
+            .map(|text| Answer {
+                prompt: "p\u{1b}[2J".into(),
+                text: text.clone(),
+            })
+            .collect();
+        let started = Instant::now();
+        let job = AnswersJob {
+            gen: 7,
+            id: uuid::Uuid::new_v4(),
+            answers,
+        };
+        jobs.send(RenderJob::Answers(job)).unwrap();
+        let (gen, rendered) = rx.recv_timeout(Duration::from_secs(30)).unwrap();
+        assert_eq!(gen, 7);
+        assert!(
+            started.elapsed() < Duration::from_secs(10),
+            "{:?}",
+            started.elapsed()
+        );
+        assert_eq!(rendered.len(), texts.len());
+        assert_eq!(
+            rendered[0].formulas.len(),
+            crate::claude::latex::MAX_FORMULAS
+        );
+        for answer in &rendered {
+            for f in &answer.formulas {
+                let laid = f.block.iter().chain(&f.text);
+                for text in f.inline.iter().chain(laid.flat_map(|l| &l.rows)) {
+                    assert_eq!(*text, crate::claude::latex::sanitize(text));
+                }
+            }
+            for line in layout(&answer.doc, &answer.formulas, 80) {
+                let text = line.text();
+                assert_eq!(text, crate::claude::latex::sanitize(&text));
+                assert!(!text.contains(['\u{e000}', '\u{e001}', '\u{e002}']));
+            }
+        }
+        // The nested script is taller than three rows: inline, it is its
+        // one-line form, so its paragraph is one line.
+        let nested_doc = &rendered[1];
+        assert!(nested_doc.formulas[0].text.as_ref().unwrap().rows.len() > 3);
+        let lines = layout(&nested_doc.doc, &nested_doc.formulas, 200);
+        assert!(
+            lines[0].text().starts_with("inline x"),
+            "{:?}",
+            lines[0].text()
+        );
+        assert!(
+            lines[1].text().is_empty(),
+            "one line, then the display formula"
+        );
+    }
+
     #[test]
     fn the_cache_forgets_the_least_recently_used() {
         let running = Arc::new(AtomicUsize::new(0));
diff --git a/src/tui/ui/mod.rs b/src/tui/ui/mod.rs
index 0209de4..145ccc8 100644
--- a/src/tui/ui/mod.rs
+++ b/src/tui/ui/mod.rs
@@ -12,14 +12,14 @@ use ratatui::Frame;
 use crate::proto::{SessionKind, SessionState};
 
 mod confirm;
-mod formulas;
 mod git;
+mod reader;
 mod stats;
 mod status;
 
 use confirm::draw_confirm;
-use formulas::draw_formulas;
 use git::draw_git;
+use reader::draw_reader;
 use stats::draw_proxy_stats;
 use status::{draw_status_machine, draw_status_session};
 
diff --git a/src/tui/ui/reader.rs b/src/tui/ui/reader.rs
new file mode 100644
index 0000000..48919fa
--- /dev/null
+++ b/src/tui/ui/reader.rs
@@ -0,0 +1,115 @@
+#[cfg(test)]
+mod tests {
+    use crate::proto::{Answer, Msg};
+    use crate::tui::app::{App, AppConfig, Effect, Modal, Mode, ReplyTo};
+    use crate::tui::test_support::{self, alt, app_with, info, press};
+    use crate::tui::{render_answers, RenderCache};
+    use crossterm::event::KeyCode;
+    use ratatui::{backend::TestBackend, Terminal};
+    use std::sync::atomic::AtomicUsize;
+    use std::sync::Arc;
+
+    /// The screen's rows after drawing `app`, trailing blanks dropped.
+    fn screen(app: &App) -> Vec<String> {
+        let mut term = Terminal::new(TestBackend::new(app.width, app.height)).unwrap();
+        term.draw(|f| super::super::draw(f, app)).unwrap();
+        let buf = term.backend().buffer();
+        (0..buf.area.height)
+            .map(|y| {
+                let row: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect();
+                row.trim_end().to_owned()
+            })
+            .collect()
+    }
+
+    /// Open the reader on `app`'s first session and answer it with
+    /// `answers`, rendered as the worker would (benign formulas only).
+    fn open_with(app: &mut App, answers: Vec<Answer>) {
+        app.on_terminal(alt('u'));
+        let (gen, id) = match app.take_effects().as_slice() {
+            [Effect::Request {
+                msg: Msg::Answers { id },
+                to: ReplyTo::Answers { gen, .. },
+                ..
+            }] => (*gen, *id),
+            other => panic!("{other:?}"),
+        };
+        let reply = Msg::AnswersReply { id, answers };
+        app.on_reply(ReplyTo::Answers { gen, id }, Ok(reply));
+        let answers = match app.take_effects().pop() {
+            Some(Effect::RenderAnswers { answers, .. }) => answers,
+            other => panic!("{other:?}"),
+        };
+        let running = Arc::new(AtomicUsize::new(0));
+        let rendered = render_answers(&answers, &mut RenderCache::default(), &running);
+        app.on_answers_rendered(gen, id, rendered);
+    }
+
+    fn answer(prompt: &str, text: &str) -> Answer {
+        Answer {
+            prompt: prompt.into(),
+            text: text.into(),
+        }
+    }
+
+    #[test]
+    fn the_reader_covers_the_pane_and_typesets_the_answer() {
+        let mut app = app_with(&[info(Some(1), None)]);
+        let text = "Energy is <latex>E = mc^2</latex>, and\n\n<latex>\\frac{a}{b}</latex>";
+        open_with(&mut app, vec![answer("old", "first"), answer("why?", text)]);
+        let rows = screen(&app);
+        assert_eq!(rows[0], " 1 ✓ app", "the tab bar stays");
+        assert_eq!(rows[1], "answer 2/2 · why?");
+        assert_eq!(
+            rows[2..7],
+            ["Energy is E = mc², and", "", "  a", "  ─", "  b"]
+        );
+        assert!(rows[28].starts_with(" /srv/app"), "the status bar stays");
+        assert!(rows[29].ends_with("Alt+h help"), "{}", rows[29]);
+        // ← shows the older answer.
+        app.on_terminal(press(KeyCode::Left));
+        assert_eq!(screen(&app)[1..3], ["answer 1/2 · old", "first"]);
+    }
+
+    #[test]
+    fn tab_shows_the_formulas_list_and_how_to_get_formulas() {
+        let mut app = app_with(&[info(Some(1), None)]);
+        open_with(&mut app, vec![answer("p", "text")]);
+        app.on_terminal(press(KeyCode::Tab));
+        let text = screen(&app).join("\n");
+        assert!(text.contains("formulas"), "{text}");
+        assert!(text.contains("No formulas yet."), "{text}");
+        assert!(text.contains("claudio daemon restart"), "{text}");
+    }
+
+    #[test]
+    fn an_old_daemon_gets_the_restart_notice_in_the_view() {
+        let mut app = app_with(&[info(Some(1), None)]);
+        app.on_terminal(alt('u'));
+        assert_eq!(screen(&app)[1..3], ["reader", "Loading…"]);
+        let Some(Modal::Reader(view)) = &app.modal else {
+            panic!("no reader")
+        };
+        let (gen, id) = (view.gen, view.session);
+        let old = std::io::Error::other("unsupported op: answers");
+        app.on_reply(ReplyTo::Answers { gen, id }, Err(old));
+        assert_eq!(
+            screen(&app)[2],
+            "daemon on local is older: run `claudio daemon restart`"
+        );
+    }
+
+    #[test]
+    fn in_plain_mode_the_reader_fills_the_screen() {
+        let mut app = App::new(AppConfig {
+            mode: Mode::Plain,
+            ..test_support::config()
+        });
+        app.recover(&Default::default(), &[info(Some(1), None)]);
+        app.take_effects();
+        app.activate(0);
+        app.take_effects();
+        open_with(&mut app, vec![answer("q", "plain text")]);
+        assert_eq!(screen(&app)[..2], ["answer 1/1 · q", "plain text"]);
+    }
+}
diff --git a/tests/manager.rs b/tests/manager.rs
index ea2516f..4cfa614 100644
--- a/tests/manager.rs
+++ b/tests/manager.rs
@@ -2118,6 +2118,9 @@ fn test_wizard_recent_dir_offers_resume() {
 
 // ── LaTeX formulas ────────────────────────────────────────────────────────────
 
+/// Tab, as the terminal sends it.
+const TAB: &[u8] = b"\t";
+
 /// A fake claude that answers with a formula, as real claude does by
 /// default: it saves its `--settings`, writes the assistant record to its
 /// transcript (under the test's `$HOME`), prints the answer with the tag,
@@ -2158,11 +2161,11 @@ exec cat
 }
 
 /// A formula in claude's answer is rendered in place of its `<latex>` tag
-/// and listed by Alt+u; a TUI started later gets it from the daemon's
-/// replay on attach. The default config gives the session the
-/// `__context latex` SessionStart hook.
+/// and listed by the reader view's formulas list (Alt+u, Tab); a TUI
+/// started later gets it from the daemon's replay on attach. The default
+/// config gives the session the `__context latex` SessionStart hook.
 #[test]
-fn test_latex_formula_is_rendered_in_the_pane_and_the_popup() {
+fn test_latex_formula_is_rendered_in_the_pane_and_the_reader_list() {
     let h = ManagerHarness::new();
     // The default test config has no `latex` key: the hook is on by default.
     let settings = h.root.join("settings.json");
@@ -2176,11 +2179,13 @@ fn test_latex_formula_is_rendered_in_the_pane_and_the_popup() {
         let hooks = fs::read_to_string(&settings).unwrap();
         assert!(hooks.contains("__context latex"), "{hooks}");
 
+        // The transcript has no prompt, so no answer; Tab lists the formula.
         tui.send_keys(ALT_U);
-        tui.wait_for(" formulas ", Region::Screen, WAIT);
-        tui.wait_for("E = mc^2", Region::Screen, WAIT);
+        tui.wait_for("No answers yet", Region::Pane, WAIT);
+        tui.send_keys(TAB);
+        tui.wait_for("E = mc^2", Region::Pane, WAIT);
         tui.send_keys(ALT_U);
-        tui.wait_until(|s| !s.contains(" formulas ", Region::Screen), WAIT);
+        tui.wait_until(|s| !s.contains("E = mc^2", Region::Pane), WAIT);
         tui.send_keys(ALT_Q);
         tui.wait_exit(WAIT);
     }
`````
Run: `umask 022; cargo test --locked --bin claudio -- tui::`
Expected: compilation fails, `error: could not compile `claudio` (bin "claudio" test) due to 30 previous errors`: no variant `Answers` on `ReplyTo`, `RenderAnswers` on `Effect`, `Reader` on `Modal`; no method `on_answers_rendered`; cannot find `AnswersJob`, `Rendered`, `render_answers`, `draw_reader`, `draw_formulas`.

Step 2: implement. The diff deletes `src/tui/ui/formulas.rs`.
`````diff
diff --git a/src/cli.rs b/src/cli.rs
index 65af804..8ce336e 100644
--- a/src/cli.rs
+++ b/src/cli.rs
@@ -345,7 +345,7 @@ pub const HELP_APPENDIX: &str = "\n\
 \x20                              Run one plain interactive claude, full screen, with the proxy env of a\n\
 \x20                              manager session: no tabs, status bar or wizard. Alt+h (help), Alt+s\n\
 \x20                              (proxy stats), Alt+l (git history), Alt+e (restart) and Alt+u\n\
-\x20                              (formulas) are claudio's; every other key goes to claude. Exits with\n\
+\x20                              (reader view) are claudio's; every other key goes to claude. Exits with\n\
 \x20                              claude; the session is not kept. Must be the first argument;\n\
 \x20                              -p/--api are rejected.\n\
 \x20  --fast                      Strip human-like typing/quiescence delays for lowest latency.\n\
diff --git a/src/tui/app.rs b/src/tui/app.rs
index 5a562d1..b534488 100644
--- a/src/tui/app.rs
+++ b/src/tui/app.rs
@@ -153,6 +153,14 @@ pub enum Effect {
         seq: u64,
         srcs: Vec<String>,
     },
+    /// Render the answers of session `id` for the reader view opened under
+    /// `gen`, on the `latex-render` worker; they come back through
+    /// [`App::on_answers_rendered`].
+    RenderAnswers {
+        gen: u64,
+        id: SessionId,
+        answers: Vec<crate::proto::Answer>,
+    },
 }
 
 /// What a request's reply is for.
@@ -185,6 +193,8 @@ pub enum ReplyTo {
     /// A request of the history viewer: the view's generation and the
     /// request's sequence number (see `git_view`).
     Git { gen: u64, seq: u64 },
+    /// `Answers` for the reader view opened under `gen` on session `id`.
+    Answers { gen: u64, id: SessionId },
 }
 
 impl ReplyTo {
diff --git a/src/tui/formulas.rs b/src/tui/formulas.rs
index 3a7a2cf..10f09f0 100644
--- a/src/tui/formulas.rs
+++ b/src/tui/formulas.rs
@@ -1,20 +1,11 @@
-//! The formulas popup (Alt+u): the active session's recent `<latex>`
-//! formulas, oldest first, each rendered in 2D above its source. The view's
-//! state and keys are here, its lines too (pure); drawing is in
-//! `ui/formulas.rs`.
-
-use std::cell::Cell;
-
-use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
+//! The reader view's formulas list (Tab in Alt+u): the active session's
+//! recent `<latex>` formulas, oldest first, each rendered in 2D above its
+//! source. Its lines are here (pure); the reader draws them.
 
 use super::latex;
 use super::sessions::SessionView;
-use crate::proto::SessionId;
-
-/// How far PageUp/PageDown (and Space) scroll.
-const PAGE: u16 = 10;
 
-/// What the popup shows when the session has no formulas.
+/// What the list shows when the session has no formulas.
 pub const EMPTY: [&str; 4] = [
     "No formulas yet.",
     "",
@@ -22,15 +13,7 @@ pub const EMPTY: [&str; 4] = [
     "`latex = false` under [claude] on the session's host, then `claudio daemon restart`).",
 ];
 
-/// The popup's state: whose formulas, and how far down.
-pub struct FormulasView {
-    pub session: SessionId,
-    scroll: u16,
-    /// The largest useful scroll, known once drawn (see `ui/formulas.rs`).
-    max_scroll: Cell<u16>,
-}
-
-/// One line of the popup.
+/// One line of the list.
 #[derive(Debug, PartialEq)]
 pub enum Line {
     /// A row of a formula's rendering.
@@ -42,51 +25,7 @@ pub enum Line {
     Blank,
 }
 
-impl FormulasView {
-    /// The popup for `session`, scrolled to its newest formulas.
-    pub fn new(session: SessionId) -> FormulasView {
-        FormulasView {
-            session,
-            scroll: u16::MAX,
-            max_scroll: Cell::new(u16::MAX),
-        }
-    }
-
-    /// The scroll offset to draw at.
-    pub fn scroll(&self) -> u16 {
-        self.scroll.min(self.max_scroll.get())
-    }
-
-    /// Record the largest useful scroll offset of what is being drawn.
-    pub fn set_max_scroll(&self, max: u16) {
-        self.max_scroll.set(max);
-    }
-
-    /// Handle a key; `true` closes the popup. Ctrl and Alt chords are not
-    /// its own (a manager key reaches the app before this).
-    pub fn on_key(&mut self, key: &KeyEvent) -> bool {
-        if key
-            .modifiers
-            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
-        {
-            return false;
-        }
-        let at = self.scroll();
-        self.scroll = match key.code {
-            KeyCode::Esc | KeyCode::Char('q') => return true,
-            KeyCode::Down | KeyCode::Char('j') => at.saturating_add(1),
-            KeyCode::Up | KeyCode::Char('k') => at.saturating_sub(1),
-            KeyCode::PageDown | KeyCode::Char(' ') => at.saturating_add(PAGE),
-            KeyCode::PageUp => at.saturating_sub(PAGE),
-            KeyCode::Home | KeyCode::Char('g') => 0,
-            KeyCode::End | KeyCode::Char('G') => u16::MAX,
-            _ => at,
-        };
-        false
-    }
-}
-
-/// The popup's lines for session `v`: its formulas oldest first, each its
+/// The list's lines for session `v`: its formulas oldest first, each its
 /// 2D rendering (else its one-line one, else its source) then its source,
 /// with a blank line between them. A formula of a list still being rendered
 /// shows its source. With no formulas, how to get them.
diff --git a/src/tui/interaction.rs b/src/tui/interaction.rs
index 0e38a24..61d9206 100644
--- a/src/tui/interaction.rs
+++ b/src/tui/interaction.rs
@@ -19,10 +19,10 @@ use crate::term::screen::{MouseMode, Screen};
 
 use super::app::{App, Effect, Mode, ReplyTo};
 use super::confirm::ConfirmPrompt;
-use super::formulas::FormulasView;
 use super::git_view::GitView;
 use super::keymap::Action;
 use super::latex::Formula;
+use super::reader::{ReaderView, RenderedAnswer};
 use super::selection::{self, Pos, Selection, Unit};
 use super::sessions::SessionView;
 use super::stats_view::StatsView;
@@ -55,8 +55,9 @@ pub enum Modal {
     Confirm(ConfirmPrompt),
     /// The commit-history viewer (full pane).
     Git(Box<GitView>),
-    /// The active session's rendered formulas (see `formulas.rs`).
-    Formulas(FormulasView),
+    /// The reader view of a session's answers (full pane, see
+    /// `reader/`). Boxed like the history viewer.
+    Reader(Box<ReaderView>),
 }
 
 impl Modal {
@@ -69,7 +70,7 @@ impl Modal {
                 | Modal::Overview { .. }
                 | Modal::Help
                 | Modal::ProxyStats { .. }
-                | Modal::Formulas(_)
+                | Modal::Reader(_)
         )
     }
 
@@ -82,13 +83,13 @@ impl Modal {
                 | (Modal::Overview { .. }, Action::Overview)
                 | (Modal::Help, Action::Help)
                 | (Modal::ProxyStats { .. }, Action::ProxyStats)
-                | (Modal::Formulas(_), Action::Formulas)
+                | (Modal::Reader(_), Action::Formulas)
         )
     }
 
     /// Whether the modal takes mouse wheel and clicks.
     fn wants_mouse(&self) -> bool {
-        matches!(self, Modal::Git(_))
+        matches!(self, Modal::Git(_) | Modal::Reader(_))
     }
 }
 
@@ -191,7 +192,7 @@ impl App {
             Action::Overview => self.open_overview(),
             Action::Help => self.open_help(),
             Action::GitLog => self.open_git_log(),
-            Action::Formulas => self.open_formulas(),
+            Action::Formulas => self.open_reader(),
             _ => {}
         }
         self.redraw = true;
@@ -208,10 +209,64 @@ impl App {
         self.redraw = true;
     }
 
-    /// The formulas popup of the active session (none without one).
-    fn open_formulas(&mut self) {
-        if let Some(id) = self.active_view().map(|v| v.id) {
-            self.modal = Some(Modal::Formulas(FormulasView::new(id)));
+    /// The reader view of the active session (none without one): it asks
+    /// the session's daemon for the answers, and shows "Loading…" until
+    /// they are rendered.
+    fn open_reader(&mut self) {
+        let Some((id, host)) = self.active_view().map(|v| (v.id, v.host.clone())) else {
+            return;
+        };
+        self.modal_gen += 1;
+        let gen = self.modal_gen;
+        self.modal = Some(Modal::Reader(Box::new(ReaderView::new(id, &host, gen))));
+        self.request(&host, Msg::Answers { id }, ReplyTo::Answers { gen, id });
+    }
+
+    /// The reader view opened under `gen` for session `id`, if it is the
+    /// one open: a reply for a closed view or another session is dropped.
+    fn reader_mut(&mut self, gen: u64, id: SessionId) -> Option<&mut ReaderView> {
+        match &mut self.modal {
+            Some(Modal::Reader(view)) if view.gen == gen && view.session == id => Some(view),
+            _ => None,
+        }
+    }
+
+    /// The daemon's answers for the reader view: rendered on the worker
+    /// (none to render: shown at once), or why there are none.
+    fn on_answers_reply(&mut self, gen: u64, id: SessionId, reply: io::Result<Msg>) {
+        let Some(view) = self.reader_mut(gen, id) else {
+            return;
+        };
+        let host = view.host.clone();
+        match reply {
+            Ok(Msg::AnswersReply { answers, .. }) if answers.is_empty() => {
+                view.set_answers(Vec::new())
+            }
+            Ok(Msg::AnswersReply { answers, .. }) => {
+                self.emit(Effect::RenderAnswers { gen, id, answers })
+            }
+            Ok(_) => view.set_failed("unexpected reply from the daemon"),
+            Err(e) => {
+                let why = App::older_daemon_notice(&host, &e).unwrap_or_else(|| e.to_string());
+                view.set_failed(why);
+            }
+        }
+    }
+
+    /// The render worker is done with the answers of the reader view
+    /// opened under `gen` for session `id`: shown, if that view is open.
+    pub fn on_answers_rendered(&mut self, gen: u64, id: SessionId, answers: Vec<RenderedAnswer>) {
+        if let Some(view) = self.reader_mut(gen, id) {
+            view.set_answers(answers);
+            self.redraw = true;
+        }
+    }
+
+    /// The answers of the reader view `gen` could not be rendered.
+    pub fn on_answers_failed(&mut self, gen: u64, id: SessionId, why: &str) {
+        if let Some(view) = self.reader_mut(gen, id) {
+            view.set_failed(why);
+            self.redraw = true;
         }
     }
 
@@ -315,7 +370,7 @@ impl App {
                 let outcome = view.on_key(&key, rows);
                 self.git_outcome(outcome);
             }
-            Modal::Formulas(view) => {
+            Modal::Reader(view) => {
                 if view.on_key(&key) {
                     self.modal = None;
                 }
@@ -326,6 +381,15 @@ impl App {
     /// A mouse event while a modal that wants the mouse is open.
     fn modal_mouse(&mut self, m: MouseEvent) {
         let rows = self.pane_size().0 as usize;
+        if let Some(Modal::Reader(view)) = &mut self.modal {
+            match m.kind {
+                MouseEventKind::ScrollDown => view.on_wheel(true),
+                MouseEventKind::ScrollUp => view.on_wheel(false),
+                _ => return,
+            }
+            self.redraw = true;
+            return;
+        }
         let Some(Modal::Git(view)) = &mut self.modal else { return };
         // Pane rows start below the tab bar; the status bar is not the pane.
         let above = self.mode.rows_above() as usize;
@@ -356,7 +420,7 @@ impl App {
                 | Modal::Overview { .. }
                 | Modal::Help
                 | Modal::Confirm(_)
-                | Modal::Formulas(_),
+                | Modal::Reader(_),
             ) => {}
             None => {
                 if let Some(v) = self.active_view().filter(|v| v.attached) {
@@ -858,6 +922,7 @@ impl App {
                 }
             }
             (ReplyTo::Git { gen, seq }, reply) => self.on_git_reply(gen, seq, reply),
+            (ReplyTo::Answers { gen, id }, reply) => self.on_answers_reply(gen, id, reply),
             (ReplyTo::Respawned(id), reply) => self.on_respawned(id, reply),
             (ReplyTo::SpawnedDeferred(id), Ok(_)) => {
                 // Re-attach the active view: its first Attach raced the Spawn.
diff --git a/src/tui/keymap.rs b/src/tui/keymap.rs
index 7dfbed1..5d6f652 100644
--- a/src/tui/keymap.rs
+++ b/src/tui/keymap.rs
@@ -37,7 +37,9 @@ pub enum Action {
     GitLog,
     /// Restart the active session in its tab (asks how).
     Reset,
-    /// Open the popup of the active session's rendered formulas.
+    /// Open the reader view of the active session's answers. Its config
+    /// name stays `formulas` (the v0.4.0 popup it replaced), so existing
+    /// `[keys]` keep working.
     Formulas,
     /// Show/hide dot-directories in the new-session wizard (wizard scope).
     ToggleHidden,
@@ -198,13 +200,13 @@ pub const DEFAULT_BINDINGS: &[Binding] = &[
         action: Action::ToggleHidden,
         scope: Scope::Wizard,
     },
-    // Alt+u: the formulas popup. Verified free in Claude Code 2.1.312 (no
+    // Alt+u: the reader view. Verified free in Claude Code 2.1.312 (no
     // alt+u/meta+u in its bundle); readline's Alt+u (upcase-word) is
     // shadowed in terminal tabs, like Alt+l and Alt+c.
     Binding {
         code: KeyCode::Char('u'),
         mods: KeyModifiers::ALT,
-        label: "rendered formulas of the session",
+        label: "reader view",
         action: Action::Formulas,
         scope: Scope::Global,
     },
diff --git a/src/tui/mod.rs b/src/tui/mod.rs
index 30e4e7f..b1803d5 100644
--- a/src/tui/mod.rs
+++ b/src/tui/mod.rs
@@ -18,8 +18,6 @@ mod latex;
 mod notifications;
 mod plain;
 mod proxy_state;
-// Nothing uses the reader until the worker and the view do (later tasks).
-#[allow(dead_code)]
 mod reader;
 mod selection;
 mod sessions;
@@ -492,6 +490,12 @@ enum HostEvent {
         seq: u64,
         formulas: Vec<latex::Formula>,
     },
+    /// The render worker finished the answers of a reader view.
+    AnswersRendered {
+        gen: u64,
+        id: crate::proto::SessionId,
+        answers: Vec<reader::RenderedAnswer>,
+    },
 }
 
 // ── Formula rendering ─────────────────────────────────────────────────────────
@@ -504,31 +508,86 @@ struct LatexJob {
     srcs: Vec<String>,
 }
 
-/// Start the `latex-render` thread: it renders each job's formulas (from
-/// its cache when it can) and hands them to `done`. It ends when the
+/// One [`Effect::RenderAnswers`]: the answers of the reader view `gen`
+/// shows for session `id`.
+struct AnswersJob {
+    gen: u64,
+    id: crate::proto::SessionId,
+    answers: Vec<crate::proto::Answer>,
+}
+
+/// Work for the `latex-render` thread, one job at a time.
+enum RenderJob {
+    Latex(LatexJob),
+    Answers(AnswersJob),
+}
+
+/// What the `latex-render` thread made of a job.
+enum Rendered {
+    Latex(LatexJob, Vec<latex::Formula>),
+    Answers(AnswersJob, Vec<reader::RenderedAnswer>),
+}
+
+/// Start the `latex-render` thread: it renders each job (its formulas from
+/// its cache when it can) and hands the result to `done`. It ends when the
 /// returned sender is dropped. Rendering never runs on the UI thread: one
-/// hostile formula can take a minute to parse.
+/// hostile formula can take a minute to parse, and markdown is parsed here
+/// too.
 fn spawn_latex_worker(
-    done: impl Fn(LatexJob, Vec<latex::Formula>) + Send + 'static,
-) -> io::Result<std::sync::mpsc::Sender<LatexJob>> {
-    let (jobs, queue) = std::sync::mpsc::channel::<LatexJob>();
+    done: impl Fn(Rendered) + Send + 'static,
+) -> io::Result<std::sync::mpsc::Sender<RenderJob>> {
+    let (jobs, queue) = std::sync::mpsc::channel::<RenderJob>();
     std::thread::Builder::new()
         .name(RENDER_THREAD.to_owned())
         .spawn(move || {
             let mut cache = RenderCache::default();
             let running = Arc::new(AtomicUsize::new(0));
             for job in queue {
-                let formulas = job
-                    .srcs
-                    .iter()
-                    .map(|src| cache.formula(src, &running))
-                    .collect();
-                done(job, formulas);
+                match job {
+                    RenderJob::Latex(job) => {
+                        let formulas = job
+                            .srcs
+                            .iter()
+                            .map(|src| cache.formula(src, &running))
+                            .collect();
+                        done(Rendered::Latex(job, formulas));
+                    }
+                    RenderJob::Answers(job) => {
+                        let answers = render_answers(&job.answers, &mut cache, &running);
+                        done(Rendered::Answers(job, answers));
+                    }
+                }
             }
         })?;
     Ok(jobs)
 }
 
+/// `answers` as the reader shows them: each a markdown document whose
+/// formulas (the first `MAX_FORMULAS`) are rendered through the guard and
+/// its cache, like the pane's.
+fn render_answers(
+    answers: &[crate::proto::Answer],
+    cache: &mut RenderCache,
+    running: &Arc<AtomicUsize>,
+) -> Vec<reader::RenderedAnswer> {
+    answers
+        .iter()
+        .map(|answer| {
+            let prepared = reader::doc::prepare(&answer.text);
+            let formulas = prepared
+                .tags
+                .iter()
+                .map(|tag| cache.formula(&tag.src, running))
+                .collect();
+            reader::RenderedAnswer {
+                prompt: answer.prompt.clone(),
+                doc: reader::doc::parse(&prepared),
+                formulas,
+            }
+        })
+        .collect()
+}
+
 /// Rendered formulas by source, the [`CACHE_SIZE`] most recently used: each
 /// list resends every formula, and a source is rendered once.
 #[derive(Default)]
@@ -723,15 +782,24 @@ async fn event_loop(
         });
     }
 
-    // Formulas are rendered on their own thread; the results come back here.
+    // Formulas and answers are rendered on their own thread; the results
+    // come back here.
     let rendered = ev_tx.clone();
-    let latex_jobs = spawn_latex_worker(move |job, formulas| {
-        let _ = rendered.blocking_send(HostEvent::LatexRendered {
-            host: job.host,
-            id: job.id,
-            seq: job.seq,
-            formulas,
-        });
+    let latex_jobs = spawn_latex_worker(move |done| {
+        let event = match done {
+            Rendered::Latex(job, formulas) => HostEvent::LatexRendered {
+                host: job.host,
+                id: job.id,
+                seq: job.seq,
+                formulas,
+            },
+            Rendered::Answers(job, answers) => HostEvent::AnswersRendered {
+                gen: job.gen,
+                id: job.id,
+                answers,
+            },
+        };
+        let _ = rendered.blocking_send(event);
     })
     .ok();
 
@@ -898,6 +966,9 @@ async fn event_loop(
                 Some(HostEvent::LatexRendered { host, id, seq, formulas }) => {
                     app.on_latex_rendered(&host, id, seq, formulas);
                 }
+                Some(HostEvent::AnswersRendered { gen, id, answers }) => {
+                    app.on_answers_rendered(gen, id, answers);
+                }
                 None => return Ok(Default::default()),
             },
             Some((to, reply)) = reply_rx.recv() => app.on_reply(to, reply),
@@ -934,7 +1005,7 @@ fn run_effect(
     conns: &mut Connections,
     reply_tx: &mpsc::UnboundedSender<(ReplyTo, io::Result<Msg>)>,
     ev_tx: &mpsc::Sender<HostEvent>,
-    latex_jobs: Option<&std::sync::mpsc::Sender<LatexJob>>,
+    latex_jobs: Option<&std::sync::mpsc::Sender<RenderJob>>,
 ) {
     match effect {
         Effect::Request { host, msg, to } => {
@@ -990,7 +1061,13 @@ fn run_effect(
         Effect::RenderLatex { host, id, seq, srcs } => {
             // Without a worker (it could not start) formulas stay raw.
             if let Some(jobs) = latex_jobs {
-                let _ = jobs.send(LatexJob { host, id, seq, srcs });
+                let _ = jobs.send(RenderJob::Latex(LatexJob { host, id, seq, srcs }));
+            }
+        }
+        Effect::RenderAnswers { gen, id, answers } => {
+            let job = RenderJob::Answers(AnswersJob { gen, id, answers });
+            if latex_jobs.is_none_or(|jobs| jobs.send(job).is_err()) {
+                app.on_answers_failed(gen, id, "the render thread is not running");
             }
         }
         Effect::Save => {
diff --git a/src/tui/reader/layout.rs b/src/tui/reader/layout.rs
index 3223bd4..17f6439 100644
--- a/src/tui/reader/layout.rs
+++ b/src/tui/reader/layout.rs
@@ -38,6 +38,7 @@ pub struct Seg {
 
 impl Line {
     /// The line's text, styles dropped.
+    #[cfg(test)]
     pub fn text(&self) -> String {
         self.0.iter().map(|s| s.text.as_str()).collect()
     }
@@ -104,7 +105,7 @@ fn emphasized(style: Style, emphasis: Emphasis) -> Style {
 
 /// `s` with every grapheme wider than two columns replaced by `�`, so no
 /// line is lost to one (the cells take at most two columns each).
-fn safe(s: &str) -> String {
+pub fn safe(s: &str) -> String {
     s.graphemes(true)
         .map(|g| if g.width() > 2 { "\u{fffd}" } else { g })
         .collect()
diff --git a/src/tui/ui/formulas.rs b/src/tui/ui/formulas.rs
deleted file mode 100644
index ec55d35..0000000
--- a/src/tui/ui/formulas.rs
+++ /dev/null
@@ -1,109 +0,0 @@
-//! The formulas popup (Alt+u); its state and lines are in `tui/formulas.rs`.
-
-use ratatui::style::{Modifier, Style};
-use ratatui::Frame;
-use unicode_width::UnicodeWidthStr;
-
-use super::{centered, popup};
-use crate::claude::latex::sanitize;
-use crate::tui::app::App;
-use crate::tui::formulas::{self, FormulasView, Line};
-use crate::tui::latex::text_cells;
-
-/// The popup's widest: it is also at most 90% of the screen's height.
-const MAX_WIDTH: u16 = 100;
-
-/// Draw the popup over the whole screen, scrolled where `view` says. Every
-/// line is written cell by cell like the pane's formulas, and cut at the
-/// popup's edge.
-pub fn draw_formulas(frame: &mut Frame, app: &App, view: &FormulasView) {
-    let area = frame.area();
-    let width = MAX_WIDTH.min(area.width.saturating_sub(2));
-    let height = (u32::from(area.height) * 9 / 10) as u16;
-    let inner = popup(frame, centered(area, width, height), "formulas");
-    let lines = app
-        .sessions
-        .iter()
-        .find(|v| v.id == view.session)
-        .map(formulas::lines)
-        .unwrap_or_default();
-    let total = u16::try_from(lines.len()).unwrap_or(u16::MAX);
-    view.set_max_scroll(total.saturating_sub(inner.height));
-    let plain = Style::default();
-    let dim = plain.add_modifier(Modifier::DIM);
-    let shown = lines
-        .iter()
-        .skip(usize::from(view.scroll()))
-        .take(usize::from(inner.height));
-    let buf = frame.buffer_mut();
-    for (y, line) in (inner.y..).zip(shown) {
-        let (text, style) = match line {
-            Line::Formula(text) => (text.as_str(), plain),
-            Line::Source(text) => (text.as_str(), dim),
-            Line::Hint(text) => (*text, dim),
-            Line::Blank => continue,
-        };
-        let Some(mut cells) = text_cells(&sanitize(text), style) else {
-            continue;
-        };
-        cells.truncate(usize::from(inner.width));
-        // A wide character cut in half at the edge goes.
-        if cells.last().is_some_and(|c| c.symbol().width() > 1) {
-            cells.pop();
-        }
-        for (x, cell) in (inner.x..).zip(cells) {
-            buf[(x, y)] = cell;
-        }
-    }
-}
-
-#[cfg(test)]
-mod tests {
-    use crate::tui::app::{App, Modal};
-    use crate::tui::latex::{Formula, Rendered};
-    use crate::tui::test_support::{alt, app_with, info};
-    use crate::tui::typeset::Laid;
-    use ratatui::{backend::TestBackend, Terminal};
-
-    /// The screen's rows after drawing `app`.
-    fn screen(app: &App) -> Vec<String> {
-        let mut term = Terminal::new(TestBackend::new(app.width, app.height)).unwrap();
-        term.draw(|f| super::super::draw(f, app)).unwrap();
-        let buf = term.backend().buffer();
-        (0..buf.area.height)
-            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
-            .collect()
-    }
-
-    #[test]
-    fn the_popup_shows_each_formula_over_its_source_newest_in_view() {
-        let mut app = app_with(&[info(Some(1), None)]);
-        let v = &mut app.sessions[0];
-        v.latex_srcs = (0..20).map(|i| format!("x_{i}")).collect();
-        let rendered = Rendered {
-            inline: Some("x₁₉".into()),
-            block: Some(Laid {
-                rows: vec!["x₁₉".into()],
-                baseline: 0,
-            }),
-            text: None,
-        };
-        v.latex = vec![Formula::new("x_19", rendered)];
-        app.on_terminal(alt('u'));
-        assert!(matches!(app.modal, Some(Modal::Formulas(_))));
-        let text = screen(&app).join("\n");
-        assert!(text.contains(" formulas "), "{text}");
-        // Scrolled to the bottom: the newest is shown, rendered, the oldest not.
-        assert!(text.contains("x₁₉") && text.contains("x_19"), "{text}");
-        assert!(!text.contains("x_0 "), "{text}");
-    }
-
-    #[test]
-    fn the_empty_popup_says_how_to_get_formulas() {
-        let mut app = app_with(&[info(Some(1), None)]);
-        app.on_terminal(alt('u'));
-        let text = screen(&app).join("\n");
-        assert!(text.contains("No formulas yet."), "{text}");
-        assert!(text.contains("claudio daemon restart"), "{text}");
-    }
-}
diff --git a/src/tui/ui/mod.rs b/src/tui/ui/mod.rs
index 8d2e8b8..145ccc8 100644
--- a/src/tui/ui/mod.rs
+++ b/src/tui/ui/mod.rs
@@ -97,7 +97,7 @@ fn draw_modal(frame: &mut Frame, app: &App, pane: Rect) {
         Some(Modal::Help) => draw_help(frame, app),
         Some(Modal::Confirm(prompt)) => draw_confirm(frame, prompt),
         Some(Modal::Git(view)) => draw_git(frame, view, app.now, pane),
-        Some(Modal::Formulas(view)) => draw_formulas(frame, app, view),
+        Some(Modal::Reader(view)) => draw_reader(frame, app, view, pane),
         None => {}
     }
 }
diff --git a/src/tui/ui/reader.rs b/src/tui/ui/reader.rs
index a8925e3..48919fa 100644
--- a/src/tui/ui/reader.rs
+++ b/src/tui/ui/reader.rs
@@ -1,3 +1,66 @@
+//! The reader view (Alt+u) over the session pane; its state and lines are
+//! in `tui/reader/`.
+
+use ratatui::buffer::Buffer;
+use ratatui::layout::Rect;
+use ratatui::style::{Modifier, Style};
+use ratatui::widgets::Clear;
+use ratatui::Frame;
+use unicode_width::UnicodeWidthStr;
+
+use crate::claude::latex::sanitize;
+use crate::tui::app::App;
+use crate::tui::latex::text_cells;
+use crate::tui::reader::layout::{safe, Line};
+use crate::tui::reader::ReaderView;
+
+/// Draw the view over `pane` (the tab and status bars stay): its header on
+/// the first row, then the body from where `view` is scrolled. Every line
+/// is written cell by cell, sanitized again, and cut at the pane's edge.
+pub fn draw_reader(frame: &mut Frame, app: &App, view: &ReaderView, pane: Rect) {
+    frame.render_widget(Clear, pane);
+    if pane.height == 0 {
+        return;
+    }
+    let width = usize::from(pane.width);
+    let buf = frame.buffer_mut();
+    let header = view.header(width);
+    let bar = Style::default().add_modifier(Modifier::REVERSED);
+    for x in pane.left()..pane.right() {
+        buf[(x, pane.y)].set_style(bar);
+    }
+    draw_line(buf, pane, pane.y, &header, bar);
+    let session = app.sessions.iter().find(|v| v.id == view.session);
+    let lines = view.body(session, width);
+    let rows = pane.height - 1;
+    let total = u16::try_from(lines.len()).unwrap_or(u16::MAX);
+    view.set_max_scroll(total.saturating_sub(rows));
+    let shown = lines
+        .iter()
+        .skip(usize::from(view.scroll()))
+        .take(usize::from(rows));
+    for (y, line) in (pane.y + 1..).zip(shown) {
+        draw_line(buf, pane, y, line, Style::default());
+    }
+}
+
+/// Write `line` on row `y` of `area`, each piece's style over `base`.
+fn draw_line(buf: &mut Buffer, area: Rect, y: u16, line: &Line, base: Style) {
+    let mut cells = Vec::new();
+    for seg in &line.0 {
+        let text = safe(&sanitize(&seg.text));
+        cells.extend(text_cells(&text, base.patch(seg.style)).unwrap_or_default());
+    }
+    cells.truncate(usize::from(area.width));
+    // A wide character cut in half at the edge goes.
+    if cells.last().is_some_and(|c| c.symbol().width() > 1) {
+        cells.pop();
+    }
+    for (x, cell) in (area.x..).zip(cells) {
+        buf[(x, y)] = cell;
+    }
+}
+
 #[cfg(test)]
 mod tests {
     use crate::proto::{Answer, Msg};
`````
Run: `umask 022; cargo build --locked 2>&1 | grep -cE '^(warning|error)'; cargo test --locked --bin claudio -- tui:: && rustfmt --edition 2021 --check src/tui/ui/reader.rs src/tui/formulas.rs src/tui/reader/layout.rs src/cli.rs && make test-unit && cargo test --locked --test manager -- --test-threads=1 test_latex_formula test_plain_help`
Expected: `0`, then `test result: ok. 382 passed; 0 failed; 0 ignored; 0 measured; 504 filtered out` (the hostile-answers test takes about a second), then `test result: ok. 886 passed; 0 failed`, then (manager) `test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 42 filtered out`.

Step 3: commit
`/usr/bin/git add src/tui/ui/reader.rs && /usr/bin/git commit -m "feat(tui): alt+u opens the reader view of the session's answers" -- src/cli.rs src/tui/app.rs src/tui/formulas.rs src/tui/interaction.rs src/tui/keymap.rs src/tui/mod.rs src/tui/reader/layout.rs src/tui/ui/formulas.rs src/tui/ui/mod.rs src/tui/ui/reader.rs tests/manager.rs`

Interfaces this task provides to later tasks: `Modal::Reader`, `ReplyTo::Answers { gen, id }`, `Effect::RenderAnswers { gen, id, answers }`, `App::{on_answers_rendered, on_answers_failed}`; the test-only `TAB` constant in tests/manager.rs.

### Task 8: manager test: the reader shows a transcript's answers in 2D
Model: sonnet   Repo: /volumes/repos/claudio   Files: tests/manager.rs
Goal: AC10, end to end through a real daemon. A fake claude writes two prompt/answer pairs to `$HOME/.claude/projects/<encoded resolved cwd>/<sid>.jsonl` (the newest answer: text with an inline formula, a display formula and a list), prints only its banner, and runs its own SessionStart hook so the daemon journals the conversation. Alt+u must show `answer 2/2 · Explain the quadratic formula`, the inline formula typeset (`ax² + bx + c = 0`), the quadratic formula in three rows indented two columns, and the list; ← shows `answer 1/2 · Say hi` and its text; Esc closes.

Step 1: write the test. It passes as soon as it is written, because Tasks 1–7 implement the behaviour. Its bite was checked in the scratch run: with the daemon's claude session id forced to `None` in `answers` (src/daemon/server.rs, `let sid = None::<String>.filter(…)`), it fails with `wait_for timeout: "answer 2/2 · Explain the quadratic formula" not found in Pane`. Do not commit that mutation.
`````diff
diff --git a/tests/manager.rs b/tests/manager.rs
index 4cfa614..a32833e 100644
--- a/tests/manager.rs
+++ b/tests/manager.rs
@@ -2195,3 +2195,87 @@ fn test_latex_formula_is_rendered_in_the_pane_and_the_reader_list() {
     let tui = h.start_tui();
     tui.wait_for("Energy: E = mc² as promised.", Region::Pane, DAEMON_WAIT);
 }
+
+// ── Reader view ───────────────────────────────────────────────────────────────
+
+/// The answer the reader test's fake claude writes: text, an inline and a
+/// display formula, and a list.
+const ANSWER: &str = "The roots of <latex>ax^2 + bx + c = 0</latex> are\n\n\
+    <latex>x = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}{2a}</latex>\n\n\
+    - first item\n- second item";
+
+/// A fake claude with a conversation of two answers in its transcript. It
+/// prints neither, so what the reader shows comes from the transcript (via
+/// the daemon); its SessionStart hook tells the daemon which transcript.
+fn write_answers_fake_claude(h: &ManagerHarness) {
+    let sid = "bbbbbbbb-cccc-4ddd-8eee-ffffffffffff";
+    let cwd = fs::canonicalize(&h.dirs[0]).unwrap();
+    let project = h.home.join(".claude/projects").join(encode_cwd(&cwd));
+    let prompt = |text: &str| {
+        serde_json::json!({"type": "user", "message": {"role": "user", "content": text}})
+    };
+    let answer = |text: &str| {
+        serde_json::json!({
+            "type": "assistant",
+            "message": {
+                "model": "claude-fake",
+                "content": [{"type": "text", "text": text}],
+                "usage": {"input_tokens": 1, "output_tokens": 1},
+            },
+        })
+    };
+    let records = [
+        prompt("Say hi"),
+        answer("Hello there."),
+        prompt("Explain the quadratic formula"),
+        answer(ANSWER),
+    ];
+    let lines: Vec<String> = records.iter().map(|r| format!("'{r}'")).collect();
+    let script = format!(
+        r#"#!/bin/sh
+if [ "$1" = "--version" ]; then echo "claude 0.0.0-fake"; exit 0; fi
+if [ "$1" = "--help" ]; then exit 0; fi
+mkdir -p "{project}"
+printf '%s\n' {lines} > "{project}/{sid}.jsonl"
+echo "FAKE_CLAUDE_BANNER pid=$$"
+HOOK=$(printf '%s' "$2" | sed -n 's/.*"command":"\([^"]*__hook SessionStart[^"]*\)".*/\1/p')
+printf '{{"session_id":"{sid}","source":"startup"}}' | eval "$HOOK"
+exec cat
+"#,
+        project = project.display(),
+        lines = lines.join(" "),
+    );
+    fs::write(&h.fake_claude, script).unwrap();
+    fs::set_permissions(&h.fake_claude, std::os::unix::fs::PermissionsExt::from_mode(0o755))
+        .unwrap();
+}
+
+/// AC10: Alt+u opens the reader on the newest answer, rendered from the
+/// transcript: its prompt in the header, text with the inline formula
+/// typeset, the display formula in 2D, the list; ← shows the older one,
+/// and Esc closes it.
+#[test]
+fn test_reader_shows_the_answers_with_formulas_in_2d() {
+    let h = ManagerHarness::new();
+    write_answers_fake_claude(&h);
+    let mut tui = h.start_tui();
+    wizard_pick_dir(&mut tui, &h.dirs[0]);
+
+    tui.send_keys(ALT_U);
+    tui.wait_for("answer 2/2 · Explain the quadratic formula", Region::Pane, DAEMON_WAIT);
+    tui.wait_for("The roots of ax² + bx + c = 0 are", Region::Pane, WAIT);
+    let pane = tui.screen_text(Region::Pane);
+    for row in ["      -b ± √(b² - 4ac)", "  x = ────────────────", "             2a"] {
+        assert!(pane.contains(row), "missing {row:?} in:\n{pane}");
+    }
+    assert!(pane.contains("• first item") && pane.contains("• second item"), "{pane}");
+
+    tui.send_keys(LEFT_ARROW);
+    tui.wait_for("answer 1/2 · Say hi", Region::Pane, WAIT);
+    tui.wait_for("Hello there.", Region::Pane, WAIT);
+    tui.send_keys(ESC);
+    tui.wait_for("FAKE_CLAUDE_BANNER", Region::Pane, WAIT);
+    assert!(!tui.screen_text(Region::Pane).contains("Hello there."));
+    tui.send_keys(ALT_Q);
+    tui.wait_exit(WAIT);
+}
`````
Run: `umask 022; cargo test --locked --test manager -- --test-threads=1 test_reader_shows`
Expected: `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 44 filtered out` (about 3 s).

Step 2: none (the behaviour is Tasks 1–7's). Then run the whole manager suite, alone on the machine: `umask 022; cargo test --locked --test manager -- --test-threads=1` → `test result: ok. 45 passed; 0 failed` (about 130 s).

Step 3: commit
`/usr/bin/git commit -m "test(manager): the reader view shows answers with 2d formulas" -- tests/manager.rs`

### Task 9: docs
Model: haiku   Repo: /volumes/repos/claudio   Files: README.md, docs/manager-design.md, CLAUDE.md
Goal: AC12 (term-maths went in Task 3; the binary size delta is reported at release). README: the reader view (what it shows, keys, Tab, plain mode, old daemons), the keys table row and anchor, plain mode's Alt+u, the `formulas` config name. docs/manager-design.md §2.7: the engine, the `Answers` op, the reader pipeline. CLAUDE.md: the architecture map (`server.rs` `Answers`, the worker's answers jobs, `latex.rs`, `typeset/`, `reader/`, `formulas.rs`, `ui/reader.rs`, `projects.rs`, `claude/latex.rs`) and the "never on the UI thread" constraint extended to the engine and markdown.

Step 1: no test. Check that the docs do not describe it yet: `grep -c "opens the reader view" README.md` prints `0`.

Step 2: implement.
`````diff
diff --git a/CLAUDE.md b/CLAUDE.md
index e442707..2e0a3fb 100644
--- a/CLAUDE.md
+++ b/CLAUDE.md
@@ -103,8 +103,11 @@ daemons coexist.
   `Config` directly, and the manager harness pins `SHELL=/bin/sh` with a temp
   `HOME`.
 - `server.rs` — accepts Unix socket connections, uid-checks peers, runs the
-  per-client loop. Slow requests (git, `UpdateClaude`, attach) reply from
-  spawned tasks through the client's queue, never inline.
+  per-client loop. Slow requests (git, `UpdateClaude`, attach, `Answers`)
+  reply from spawned tasks through the client's queue, never inline.
+  `Answers` reads the session transcript's tail (`projects::read_answers`)
+  in `spawn_blocking`; `proto::answers_reply` caps the reply's encoded size
+  (512 KiB, oldest answers dropped) so it always fits a frame.
 - `session.rs` — one actor task per live session: owns the PTY writer, VT
   engine (`alacritty_terminal`), and subscriber list. Fans PTY output to
   all attached clients. Hook events (`__hook <Event> <socket> <token>`) are
@@ -161,8 +164,11 @@ is idle (checked every 2 min).
   attention. Runs the `latex-render` worker thread (`Effect::RenderLatex` →
   `HostEvent::LatexRendered`). It renders each formula on a scratch thread
   (32 MiB stack) within a 150 ms budget, at most 2 abandoned at a time, and
-  caches by source (LRU 1024). Its panic hook stays silent for panics on
-  threads flagged `QUIET_PANIC`.
+  caches by source (LRU 1024). The same thread renders the reader's
+  answers (`Effect::RenderAnswers` → `HostEvent::AnswersRendered`):
+  markdown into a `reader::doc::Doc`, formulas through the same guard and
+  cache. Its panic hook stays silent for panics on threads flagged
+  `QUIET_PANIC`.
 - `app.rs` — pure coordinator (`App::new(AppConfig)`): processes events into
   state transitions and returns `Effect` values. No I/O here; all I/O is in
   `mod.rs`. `Mode::Manager` or `Mode::Plain`.
@@ -175,13 +181,22 @@ is idle (checked every 2 min).
 - `git_view.rs` + `git_app.rs` — the git viewer (Alt+l): pure view state
   machine, and its `App` glue.
 - `stats_view.rs` — the multi-page proxy stats popup (Alt+s).
-- `latex.rs` — `<latex>` formulas, pure: `Formula` (source, one-line and 2D
-  renderings, match keys), the `too_complex` pre-check, `render` (term-maths
-  plus a one-line walk of the rust-latex-parser tree), and `overlay`. The
-  overlay swaps matched on-screen tags for their rendering in the pane's
-  buffer, cell by cell, and moves no row.
-- `formulas.rs` — the formulas popup (Alt+u): view state, keys and lines;
-  drawn by `ui/formulas.rs`.
+- `latex.rs` — `<latex>` formulas, pure: `Formula` (source, one-line form,
+  display- and text-style layouts, match keys), the `too_complex`
+  pre-check, `render` (the `typeset` engine; `aligned`-like environments
+  row by row) and `overlay`. The overlay swaps matched on-screen tags for
+  their one-line rendering in the pane's buffer, cell by cell, and moves
+  no row.
+- `typeset/` — the 2D layout engine, pure: TeX's display and text styles
+  over the rust-latex-parser tree (`typeset`), the `\sqrt[n]` rewrite, the
+  rows of `aligned`/`gather`/`array`, the math-font tables (`mathfont.rs`,
+  ported from term-maths) and the v0.4.0 one-line walk (`walk.rs`).
+- `reader/` — the reader view (Alt+u): `doc.rs` (an answer as markdown
+  blocks with formula placeholders; runs on the worker), `layout.rs` (a
+  `Doc` wrapped to a width, line boxes for formulas) and `mod.rs`
+  (`ReaderView`: answer, scroll, Tab list, layout cache). Drawn by
+  `ui/reader.rs`.
+- `formulas.rs` — the lines of the reader's formulas list (Tab).
 - `confirm.rs` — the generic confirm prompt (kill, reset, claude update).
 - `claude_update.rs` — local/remote claude version checks and updates.
 - `plain.rs` — `claudio --plain`: `Mode::Plain` is one local, ephemeral
@@ -192,7 +207,7 @@ is idle (checked every 2 min).
 - `ui/` — ratatui rendering: `mod.rs` (layout, tab bar, pane, popups),
   `status.rs` (two-line bottom bar; line 2 is prioritised segments — cpu,
   ctx gauge, git, mem, 5h, conversation, 7d — dropped whole when narrow),
-  `stats.rs`, `git.rs`, `confirm.rs`, `formulas.rs`.
+  `stats.rs`, `git.rs`, `confirm.rs`, `reader.rs`.
 - `fmt.rs` — shared text and number formatting.
 - `keymap.rs` — `DEFAULT_BINDINGS` table (the single source of truth); `Keymap`
   (defaults + config overrides); `parse_key_spec`; collision detection.
@@ -247,13 +262,15 @@ is idle (checked every 2 min).
 - `mod.rs` — `SESSION_MARKERS` (env a parent claude sets for its children),
   `binary()`, `exec()`.
 - `projects.rs` — scan `~/.claude/projects/` for existing sessions (feeds the
-  wizard's resume list).
+  wizard's resume list), and `recent_answers` (the reader's last 10 answers
+  of a transcript tail: real prompts and the assistant text after them).
 - `hooks.rs` — `__hook <Event> <socket> <token>` relay: sends the hook payload
   to the daemon socket. Also `__context latex`, the SessionStart hook that asks
   claude to wrap math in `<latex>` tags (`[claude] latex`, on by default); it prints one
   constant JSON line and never touches the socket.
-- `latex.rs` — `extract_latex` (the formulas of an assistant text, code
-  excluded) and `sanitize`, the character filter every formula goes through
+- `latex.rs` — `extract_latex` and `latex_tags` (the formulas of an
+  assistant text, code excluded; the latter with their byte ranges) and
+  `sanitize`, the character filter every formula and answer goes through
   in the daemon and again in the client.
 - `state.rs` — session state tracker derived from hook events.
 - `update.rs` — version parsing and the daily release-channel check.
@@ -302,7 +319,9 @@ is idle (checked every 2 min).
   2 MiB stack. Call `latex::render` only through `render_formula` in
   `tui/mod.rs` (pre-check, scratch thread, budget, abandon cap), never from
   `App`, a draw, or a test with hostile input (unit tests may call it on
-  benign formulas only).
+  benign formulas only). The same goes for the `typeset` engine, and the
+  reader's markdown is parsed on the worker too; `App` and the drawing only
+  lay out what the worker made.
 
 ### Print-mode pipeline (`-p`)
 
diff --git a/README.md b/README.md
index 6d3feb7..31e6311 100644
--- a/README.md
+++ b/README.md
@@ -101,7 +101,7 @@ claudio                                         # new sessions now use the proxy
 claudio --plain [--proxy NAME | --no-proxy] [--first-party] [claude args…]
 ```
 
-One full-screen claude session with the proxy configured: no tabs, no status bar, no saved state. It feels like plain `claude`, and exits when claude does, with claude's exit status. Only five keys are claudio's: `Alt+h` help, `Alt+s` proxy stats, `Alt+l` git history, `Alt+e` restart, `Alt+u` formulas. Everything else, and every argument, goes to claude.
+One full-screen claude session with the proxy configured: no tabs, no status bar, no saved state. It feels like plain `claude`, and exits when claude does, with claude's exit status. Only five keys are claudio's: `Alt+h` help, `Alt+s` proxy stats, `Alt+l` git history, `Alt+e` restart, `Alt+u` reader view. Everything else, and every argument, goes to claude.
 
 ### Everyday commands
 
@@ -196,13 +196,13 @@ A multi-page popup for the session's claude-proxy profile:
 
 `←`/`→` or `1`–`5` switch pages, `w` changes the time window, and `r` refreshes. `n` asks the proxy to move the session to its next subscription (after a confirmation); the switch takes effect on the next message and rebuilds the prompt cache.
 
-### Formulas (`Alt+u`)
+### Formulas and the reader view (`Alt+u`)
 
 Each claude session is told (by a `SessionStart` hook) to write math as LaTeX inside `<latex>…</latex>` tags. This is on by default; `latex = false` under `[claude]` in `config.toml` turns it off. The setting belongs to the host that runs the session, and its daemon reads it when it starts: after changing it, run `claudio daemon restart` on that host.
 
 In the pane, each tag is replaced by a one-line Unicode rendering in cyan, such as `E = mc²` or `x = (-b ± √(b² - 4ac))/(2a)`. Nothing else on the screen moves, and selecting still copies the original text. A formula claudio cannot render, or does not recognise on screen, stays as written. The formulas come from the conversation's transcript, not from the screen (Claude Code's markdown drops backslashes there), so they render too when you ask claude for the tags yourself, without the setting.
 
-`Alt+u` opens the session's last 64 formulas, oldest first, each drawn in 2D above its LaTeX source. `↑`/`↓`, `PgUp`/`PgDn` and `g`/`G` scroll; `Esc` closes. In a terminal tab, `Alt+u` shadows readline's upcase-word.
+`Alt+u` opens the reader view over the pane: claude's latest answer, rendered by claudio from the transcript, with its prompt on top. Markdown is laid out (headings, emphasis, lists, quotes, tables, code blocks) and every formula is typeset in 2D: a formula on its own line is a display formula, as tall as it needs; one inside a sentence grows its line by a row above and below at most (taller ones stay on one line). `←`/`→` (or `h`/`l`) go to the previous and next of the last 10 answers; `↑`/`↓`, `PgUp`/`PgDn`, `g`/`G` and the mouse wheel scroll; `Tab` switches to the session's last 64 formulas, each drawn in 2D above its LaTeX source; `Esc` or `Alt+u` closes. It works over SSH and in `--plain` too. A daemon older than the client shows "run `claudio daemon restart`" instead. In a terminal tab, `Alt+u` shadows readline's upcase-word.
 
 ### Keeping claude up to date
 
@@ -228,7 +228,7 @@ Manager keys use `Alt`, so they don't clash with Claude Code. In terminal tabs t
 | `Alt+g` | Overview of all sessions |
 | `Alt+s` | Proxy stats |
 | `Alt+s` → `n` | Move the session to the next proxy subscription (asks first) |
-| `Alt+u` | The session's formulas, rendered (see [Formulas](#formulas-altu)) |
+| `Alt+u` | Reader view: the session's answers, formulas typeset (see [Formulas](#formulas-and-the-reader-view-altu)) |
 | `Alt+h` | Help |
 | `Alt+.` | In the wizard: show or hide dot-directories |
 | `Alt+q` | Quit the UI (sessions keep running) |
@@ -284,7 +284,7 @@ new_session = "alt+n"
 terminal    = "alt+c"
 git_log     = "alt+l"
 reset       = "alt+e"
-formulas    = "alt+u"
+formulas    = "alt+u"           # the reader view (its v0.4.0 name)
 ```
 
 `CLAUDIO_NO_UPDATE_CHECK=1` disables both the claudio and the claude release checks.
diff --git a/docs/manager-design.md b/docs/manager-design.md
index b487606..b6fa584 100644
--- a/docs/manager-design.md
+++ b/docs/manager-design.md
@@ -397,6 +397,12 @@ Spec: `docs/specs/2026-11-04-latex-render-design.md`.
 - **Rendering.** `App` never renders: a newer list becomes
   `Effect::RenderLatex`. The `latex-render` thread renders each uncached
   source (LRU cache, 1024 entries) on a scratch thread with a 32 MiB stack.
+  Since v0.5.0 the layout is claudio's own (`tui/typeset/`, spec
+  `docs/specs/2026-11-05-reader-view-design.md`): TeX's display and text
+  styles over the rust-latex-parser tree, scripts as Unicode glyphs or
+  raised and lowered rows, flat fractions inside scripts and text style.
+  The pane's one-line form is the text-style layout when it is one row,
+  else the v0.4.0 walk of the tree.
   A pre-check turns away deep or token-heavy input first (brace depth 32;
   256 `\commands`, `/` and bareword `(`). The worker then waits 150 ms. A
   render over budget or one that panics is shown raw. An over-budget thread
@@ -412,8 +418,28 @@ Spec: `docs/specs/2026-11-04-latex-render-design.md`.
   without HTML-looking runs. It rebuilds the row cell by cell with the
   one-line rendering. A row that would not fit stays raw, no row moves, and
   the mirror (selection, copy) is untouched.
-- **Popup.** `Alt+u` (`Modal::Formulas`, read-only) shows the list in 2D
-  (term-maths), each formula above its source.
+- **Answers.** `Msg::Answers { id }` (an old daemon answers `Error
+  "unsupported op…"`, shown as "run `claudio daemon restart`") is served
+  off the client loop: the transcript's last 4 MiB are read in
+  `spawn_blocking` and `projects::recent_answers` keeps the last 10
+  answers. An answer is a real prompt (not `isMeta`, a compaction summary,
+  a slash command or Claude Code's own records) and the `text` of the
+  non-sidechain assistant records after it (API errors excluded); a
+  compaction ends it. Prompts are cut at 2 KiB, texts at 64 KiB, all
+  sanitized, and the reply's encoded size is capped at 512 KiB (oldest
+  answers dropped), so it always fits a frame.
+- **Reader.** `Alt+u` (`Modal::Reader`, read-only, full pane) asks for the
+  answers; the reply becomes `Effect::RenderAnswers`. The worker sanitizes
+  each text again, replaces each formula tag (the first 64) with a
+  private-use placeholder, parses the markdown (pulldown-cmark: tables,
+  strikethrough, task lists only) into a width-free `Doc`, and renders the
+  formulas through the same guard and cache (`HostEvent::AnswersRendered`).
+  The view lays a `Doc` out for its width (cached for that width only):
+  inline formulas are line boxes of at most three rows, display formulas
+  indented blocks with a one-line fallback, tables aligned or flowed, code
+  cut at the edge. Replies and renderings for a closed view, another
+  session or an older open are dropped (`modal_gen`). `Tab` shows the
+  `Latex` list (the v0.4.0 popup's content).
 
 ## 3. UX
 
`````
Run: `grep -c "opens the reader view" README.md; grep -c "term-maths" CLAUDE.md`
Expected: `1`, then `1` (the one mention left says the tables were ported from term-maths).

Step 3: commit
`/usr/bin/git commit -m "docs: the reader view and the typeset engine" -- README.md docs/manager-design.md CLAUDE.md`

## Merge and release
1. Full suites, on the tip of `feat/reader-view`, each alone on the machine:
   - `umask 022; make test-unit` → `test result: ok. 886 passed; 0 failed` with no `warning:` lines. On a known base flake (see Global Constraints), rerun once.
   - `umask 022; cargo test --locked --test manager -- --test-threads=1` → `test result: ok. 45 passed; 0 failed` (about 130 s in the scratch run).
   - `make` → a clean release build. Then `stat -c %s target/release/claudio`. The scratch run measured 12,188,288 bytes against 11,631,232 at v0.4.0: +557,056 bytes (+4.8%; pulldown-cmark and unicase in, term-maths out). Report the real figure in the gate audit and the release notes (AC12).
   - `make static` → `statically linked` (it was in the scratch run: 12,294,616 bytes).
2. AC11, by the controller, with the branch's `target/release/claudio`, before the gate audit and any merge, on an isolated daemon as for v0.4.0 (its own `XDG_CONFIG_HOME` and `XDG_RUNTIME_DIR` under `~/.cache/claudio/ac11/`, so the owner's running daemon and sessions are untouched; `HOME` stays real, so claude is logged in and writes its transcripts where the daemon reads them):
   - Start `target/release/claudio` with that environment, open a session in a scratch directory and ask claude two questions, the second a math one ("derive the quadratic formula, with the steps"). Alt+u: the newest answer reads well (markdown laid out, display formulas in 2D, inline formulas on the baseline, an `aligned` derivation as rows); ←/→ reach the older answer; Tab lists the formulas; Esc closes. Narrow the terminal below 20 columns: "too narrow".
   - Old daemon: a v0.4.0 daemon is replaced by a newer client unless it is busy (`freshness.rs`). So start a v0.4.0 `claudio` (built from tag v0.4.0 into `~/.cache/claudio/ac11/`) with the same environment, open a session and leave claude waiting on a permission prompt; quit that TUI and start the branch one: the daemon is kept (flagged outdated), and Alt+u on that session shows "daemon on local is older: run `claudio daemon restart`" in the view, while the pane still renders formulas.
   - `claudio --plain` (branch binary, same environment): Alt+u works full screen.
   - Optionally on z6 (owner-approved test host): copy the static branch binary to a scratch dir on z6 and run its own daemon and TUI there with their own `XDG_CONFIG_HOME`/`XDG_RUNTIME_DIR` (as for v0.4.0), never through the SSH bridge (which starts daemons via `systemd-run --user` without those variables) and never touching z6's daemon. claude on z6 may only work through the proxy; if no model turn runs, check that Alt+u opens the reader and shows a synthetic transcript answer.
   Kill every process started for this (the isolated daemons and their sessions), remove `~/.cache/claudio/ac11/`, and record the results in progress.md; a failure here is a fix task, not a release note.
3. Fable gate audit of the branch (Lifecycle 8); fixes re-reviewed.
4. **Owner OK required** to merge. Merge: `feat/reader-view` → `main`, fast-forward or a merge commit (the owner's choice). `main` is b2d27d2 and the branch is linear on top of it.
5. Release v0.5.0 (a feature release):
   - Set `version = "0.5.0"` in Cargo.toml, run `cargo update --workspace --offline` (updates only claudio's own entry) and check that `git diff --stat` shows `Cargo.lock | 2 +-` and `Cargo.toml | 2 +-` (pulldown-cmark already changed Cargo.lock in Task 4, term-maths in Task 3). Then `cargo build --locked` must succeed.
   - Commit `/usr/bin/git commit -m "release v0.5.0" -- Cargo.toml Cargo.lock`, with no trailer.
   - Tag: `/usr/bin/git tag v0.5.0`. The release workflow checks that the tag matches Cargo.toml.
6. **Owner OK required before:**
   - pushing `main`;
   - pushing the tag `v0.5.0`.

## Scratch verification record
- Where: `~/.cache/claudio/scratch/plan-reader`, a clone of /volumes/repos/claudio on `feat/reader-view` at 0c9b31e; build output in its `target/`. Every task was built as two commits (Step 1, then Step 2) on top of the previous one; every diff block in this plan is the `git diff` between them, and every "Expected" line is copied from that run's output. The blocks were then extracted from this file and replayed with `git apply` and the Step 3 commit commands onto a fresh clone at 0c9b31e (`~/.cache/claudio/scratch/plan-reader-replay`); the result is identical to the scratch tip (`git diff` empty).
- Verified, failing then passing: Tasks 1–7, all Step 1 and Step 2 blocks. Unit totals 829 → 837 → 856 → 859 → 867 → 875 → 881 → 886. `cargo build --locked` prints no warning after any task; the step-1 states each fail to compile as stated.
- Task 8: the test passes (1 passed); with the daemon's session id forced to `None` it fails (checked). The full manager suite: 45 passed, 0 failed, run alone, twice (after Task 8 and on the final tree).
- Task 9: the docs diff applied; the grep checks give the stated counts.
- Release: `make` passed (size above), `make static` gave a statically linked binary. `rustfmt --check` is clean on every file listed in Global Constraints.
- Flakiness: one full unit run (after Task 3) hit the known flake `daemon::host::tests::probe_runs_claude_with_the_given_path`; every other run, the final tree's included, passed first time.
- Not verified here: macOS (CI builds it; nothing here is platform-specific), the release workflow, real claude (AC11 is manual), `ssh z6`, and an old v0.4.0 daemon against the new client (covered by the unit tests of the `unsupported op` path; AC11 checks it live).

## Spec deviations needing a ruling
- **D1. Two AC4 goldens of F3 are not consistent 2D pictures; the tests use the engine's.** In `\sum_{k=1}^{n} k = \frac{n(n+1)}{2}`, the spec's rows are ` n      n(n + 1)` / ` ∑ k = ────────` / `k=1        2`: the bar starts at column 7 but the numerator (as wide as the bar) at column 8, and the `2` at column 11 is centred under a bar at 8–15. The middle row is one column short; the engine gives ` ∑  k = ────────` (the limits block is 3 wide, then one space, as in the spec's own `S_{i,f}` golden, which has the same `BigOp, " ", x` tree). In `\int_0^\infty e^{-x^2} \, dx = \frac{\sqrt{\pi}}{2}`, the spec's middle row `∫ e    dx = ──` puts `e` at column 2, but its first row puts the superscript `-x²` at column 4 (not 3) and `√π` at 13–14 over a bar at 12–13, and its last row the `2` at 13 (a bar at 12–13 centres it at 12): the first and last rows are one column right of the middle one; the parser gives no space between `∫` and `e`, so the engine's rows are ` ∞ -x²      √π` / `∫ e    dx = ──` / ` 0          2`. Both are marked in the test with a comment. Recommendation: accept the engine's rows as the goldens.
- **D2. `\left…\right` keeps its content's baseline.** F3 says "a matrix, cases or delimited block of n rows has its baseline at row ⌊(n−1)/2⌋". Matrices, cases, `\binom` and `aligned` rows follow it; for `\left…\right` the plan keeps the content's own baseline, because the rule would misplace a two-row content: `\left( e^{i\pi} \right) + 1` would put `e` a row below `+ 1`. With the content's baseline: `⎛ iπ⎞` / `⎝e  ⎠ + 1`. For a fraction inside (the common case) both rules agree. Recommendation: accept.
- **D3. Operators the parser leaves unspaced get one space each side.** rust-latex-parser spaces `=`, `+`, `±`, `≤`, `→`, … but not `·` (`\cdot`), `∉`, `∋`, `∪`, `∩`, `∧`, `∨`, `∘`, `⊕`, `⊗`, `∖`, `⊥`. Per F3 ("a relation or binary operator … has one space each side") the engine adds them outside scripts: `a \cdot b \cup c` → `a · b ∪ c` (v0.4.0's walk gave `a ·b ∪c`). This changes the pane's one-line form for such formulas; every row of the AC5 table is unchanged. Recommendation: accept.
- **D4. A placeholder inside markdown code becomes its whole tag text.** F7 says "the formula's source text"; the plan restores the tag as written (`<latex>…</latex>`, sanitized), which is what the code showed. Recommendation: accept.
- **D5. Small reads of the spec, for the record.** `Effect::RenderAnswers` carries the view's `gen` rather than the host (the worker needs no host; the stale-reply rule needs `gen`). The Rust variant stays `Action::Formulas` (F10 keeps the config name; renaming only the enum would make the two disagree). Only `\lim…` names take limits below in display style (`\max`/`\min` take scripts), as the spec lists. A rows environment inside a larger formula (`\left\{\begin{array}…`) is shown as source, as F4's "any other unknown environment". The daemon also refuses a claude session id that is not `[A-Za-z0-9_-]` before building a path from it (answers empty). pulldown-cmark is pinned exactly (=0.13.6), like the parser.
