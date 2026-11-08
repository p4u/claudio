# Web UI (`claudio web`): implementation plan

Spec: docs/specs/2026-11-08-web-ui-design.md (audited; findings folded in at 1bbc659; Rulings R1, R2 in the ledger)
Protocol contract: docs/web-protocol.md (browser protocol v1). Both lanes build against it. Change it first, in its own commit, or not at all.
Class: architectural
Repos and branches: /volumes/repos/claudio → feat/web-ui (base 2a3f3ce = main, v0.7.1; spec commits 0a3b0b5, 1bbc659)
Lanes: backend worktree (Tasks 1–5) and frontend worktree (Tasks 6–10) run in parallel, each created from the feat/web-ui commit that holds this plan (not from main); each lane is merged back into feat/web-ui before Task 11.
Patches: none. Per Ruling R3 there is no drafter scratch: this plan gives full code for the security and protocol pieces (Tracker attention, worktree path validation, auth.rs and its integration tests, the approve guard and the paste sanitizer) and exact files, signatures, rules and test names elsewhere. Write each task's tests first.
Persona input (what the user wants from the UI): ~/.cache/claudio/web-ui/persona-round1.md

Base state (2a3f3ce = the v0.7.1 release; not re-measured while drafting this plan): `cargo build` clean, `make test-unit` and `make test-manager` green. Measure both before Task 1 and Task 6 and record the counts in the ledger. Known flakes on the base that pass on rerun (the ETXTBSY family): `daemon::host::tests::probe_runs_claude_with_the_given_path`, `tui::extensions::host::tests::a_disabled_extension_is_not_started_and_disabling_stops_it`. The tree is not rustfmt- or clippy-clean on the base: do not reformat untouched code (no `cargo fmt` on the crate); format only files you create, with `rustfmt --edition 2021 <file>`.

## Global Constraints
- Commits: conventional commits (`feat(scope): …`, `test(…)`, `docs: …`), no AI attribution, no `Co-Authored-By` trailer, no "Generated with" line. The message ends with its own content.
- Commit with explicit paths only (`git commit -m "…" -- <paths>`), never `git add -A` or `git commit -a`. Never push. Do not switch branches inside your worktree.
- Build output only under `target/` or `~/.cache/claudio/` (downloads, vendor sources, e2e scratch). Never `/tmp` for anything you create by hand. Test fixtures created by test code follow the existing helpers (`ManagerHarness`, `TestDaemon`, `temp_repo`).
- The TUI's behaviour must not change. `make test-unit` and `make test-manager` stay green after every task. In `src/tui/` you may only (a) raise visibility (`mod x` → `pub(crate) mod x`, `fn` → `pub(crate) fn`), (b) add `attention: None` to `SessionInfo { … }` literals, (c) add `SessionEvent::Attention { .. }` to the match arm that already ignores `SessionEvent::Unknown`. Nothing else in `src/tui/` changes.
- The web never writes the TUI's `state.json`; it reads it with `tui::state::ClientState::load`. The web's own state is `paths::config_dir()/web-state.json` and `paths::config_dir()/web-token`, written with `paths::write_atomic` (0600).
- Secrets never reach logs, argv, WS frames or HTTP responses: proxy tokens, plugin env values, `SpawnSpec.env`/`RespawnSpec.env`, the web token (except on stdout at start and with `--print-url`), session cookies, hook tokens, socket paths. Browser DTOs are built field by field (never `serde_json::to_value` of `Profile`, `Plugin`, `PluginList`, `SpawnSpec`, `RespawnSpec`, `Applied`, `Composed`). Proto reply structs that hold only daemon-sanitized data (`DirEntry`, `ProjectDir`, `ClaudeSession`, `Answer`, `SearchHit`, `GitLogPage`, `GitCommitInfo`, `GitPatch`, `GitWorktreeStatus`, `GitStatus`, `Attention`) may be serialized as they are. Logs name keys and ids, never values or typed text. `claudio web` logs (INFO) every `session.shell`, `session.spawn`, `session.kill`, `session.approve`, `extensions.action` with op, host and session id only.
- No blocking work on the async executor: file I/O (config.toml, state files, snippets, extension manifests, the token file after start, `find_profile`, which reads config.toml) goes through `tokio::task::spawn_blocking`. Never hold a `std::sync::Mutex` guard across an `.await`.
- Vendored files under `web/vendor/` are pinned upstream release files, byte-identical to upstream (deleting whole files under the mermaid reduction rule is allowed, editing is not), each listed in `web/vendor/MANIFEST.txt` with version, source URL and SHA-256, licenses in `web/vendor/LICENSES.md`.
- No node at build time: `cargo build --release` alone produces the full binary. Node 22 (`node --test`) and npm/Playwright are dev-time tools for `web/tests` and `web/e2e` only.
- Unix only; Linux and macOS must compile (CI builds both; the release cross-compiles darwin x86_64). New Rust dependencies must be pure Rust (flate2 with its default miniz_oxide backend).
- Daemon protocol additions follow CLAUDE.md: new `Msg` ops answer `Error "unsupported op…"` on old daemons and are gated by a `Welcome.features` entry; new `SessionEvent` variants deserialize to `Unknown` on old clients; new fields are `#[serde(default)]`.
- Manager and web integration tests run single-threaded: `cargo test --test manager -- --test-threads=1`, `cargo test --test web -- --test-threads=1`. Every test claudio (TUI or `claudio web`) starts with the config.toml written by `tests/common::write_test_config` (`[claude] update_check = "off"`, `remote_check = "off"`, `install = "off"`), an isolated `XDG_CONFIG_HOME`/`XDG_RUNTIME_DIR`/`HOME`/`CLAUDE_CONFIG_DIR`/`XDG_CACHE_HOME`, `CLAUDIO_NO_BROWSER=1`, and no inherited `CLAUDIO_*` variable. Fake claudes ignore `--help` and `--version` without side effects.
- Browser code: ES modules loaded through the import map, no build step, no inline script or event-handler attribute, no `eval`/`new Function`, no CDN. Pure logic (anything with a `web/tests/*.test.mjs`) lives in modules that import nothing through bare specifiers, so `node --test web/tests` runs them without a browser.
- Stable `data-testid` hooks the e2e (Task 11) relies on; Tasks 7–10 must render them exactly: `board`, `rail`, `session-card` (+`data-sid`, `data-state`), `queue`, `queue-card` (+`data-sid`, `data-kind`), `approve-btn`, `deny-btn`, `open-btn`, `pane` (+`data-pane`), `terminal` (the xterm host element), `composer`, `mode-badge`, `palette`, `palette-input`, `palette-item`, `toast`, `help`, `settings`, `theme-select`, `wizard`, `wizard-host`, `wizard-dir-input`, `wizard-kind`, `wizard-proxy`, `wizard-name`, `wizard-prompt`, `wizard-start`, `session-actions`, `rename-input`, `kill-btn`, `restart-resume-btn`, `restart-new-btn`, `terminal-tab-btn`, `confirm-yes`, `confirm-no`, `drawer`, `drawer-tab-reader`, `drawer-tab-changes`, `drawer-tab-history`, `drawer-tab-stats`, `drawer-tab-session`, `reader`, `reader-answer`, `mermaid-diagram`, `changes`, `changes-file` (+`data-path`), `diff`, `history`, `stats`, `stats-chart`, `search-input`, `search-hit`, `snippets`, `snippet`, `snippet-save`, `extensions`, `extension-row` (+`data-name`), `extension-toggle`, `plugin-setup`, `plugin-save`, `plugin-skip`, `claude-toast`, `host-chip` (+`data-host`). KaTeX output keeps its own `.katex` class.

## Review Focus
- `src/web/auth.rs` in full: token file creation/reuse/permissions, constant-time compares (token and cookie), Host and Origin acceptance (`Allowed`), cookie flags (`Secure` with `--allow-host`), session expiry and cap, the login rate limit and its ordering against the Origin check, the guard's check order (421 → 403 → 401) and that security headers are on every response including errors.
- `src/web/input.rs`: `approve_key` (state + kind + seq; seq 0 only without the `attention` feature) and `paste_bytes` (no ESC, CR or other control survives inside the brackets; 64 KiB cap). `src/web/hub.rs`: approve runs the guard and sends the key inside the hub actor, with no await between check and send; the hub drops a session's attention on `Created` and `Exited`.
- `src/claude/state.rs` (new attention logic) and `src/proto.rs` additions: emitted for every asking Notification and for `SessionStart`, sanitized, one line, capped on char boundaries; cleared on `PreToolUse`/`UserPromptSubmit`/`Stop`/`StopFailure`; `SessionEvent::Attention` nests its payload (its `kind` would collide with the enum tag).
- `src/daemon/git.rs` worktree ops: `check_worktree_path`, exact-bytes match against the daemon's own `status -z` output, `lstat` regular-file check for untracked paths, `--no-index` exit code 1 accepted, no textconv/ext-diff, same deadline and caps; `src/daemon/server.rs` routes both through `spawn_git` (`src/daemon/server.rs:487`).
- Secrets path in `src/web/spawn.rs` and `src/web/proxy.rs`: env is composed (`tui::plugins::compose`, `with_plugins`, `Msg::merge_env`) and sent only in `Msg::Spawn`/`Msg::Respawn` over the daemon socket; nothing env-bearing is serialized into a DTO, a log or a notice; AC3 test (`web_spawn_keeps_proxy_and_plugin_secrets_out_of_the_browser`).
- Attach refcount, fan-out and backpressure in `src/web/hub.rs`/`src/web/ws.rs`: first subscriber attaches at its size, later ones re-attach at the current size (no resize), last unsubscribe detaches, tab close releases everything; the per-tab queue is bounded at 8 MiB (close 1013); binary input is size-checked; no tab can stall the hub actor.
- Reader sanitizing in `web/app/render/`: DOMPurify on every marked output and on every mermaid SVG (SVG profile), KaTeX `trust:false` with `maxSize`/`maxExpand`, placeholders that model text cannot forge, highlight.js output purified, everything else text-only.
- CSP: `auth::csp` plus `build.rs` import-map hash; `web/index.html` has no inline script other than the import map; no inline handlers anywhere in `web/app/`; the e2e asserts zero `securitypolicyviolation` events.

## Task order and parallelism
| Task | Depends on | Repo/files | May run in parallel with |
|---|---|---|---|
| 1 Daemon attention event | — | src/proto.rs, src/claude/state.rs, src/daemon/{session,mod,tests}.rs, mechanical lines in src/tui/*, src/freshness.rs | Tasks 6–10 (frontend lane) |
| 2 Daemon worktree ops | 1 | src/proto.rs, src/daemon/{git,server,tests}.rs | Tasks 6–10 |
| 3 Web server core (CLI, auth, assets, WS skeleton) | 2 | Cargo.toml, Cargo.lock, build.rs, src/main.rs, src/cli.rs, src/web/{mod,auth,assets,ws}.rs, tests/web.rs | Tasks 6–10 |
| 4 Hub: hosts, sessions, attach, input, ops | 3 | src/web/{hub,dto,input,state,spawn,ws,mod}.rs, src/client.rs, src/tui/mod.rs (visibility), tests/web.rs | Tasks 6–10 |
| 5 Spawn composition, proxy, plugins, extensions, snippets | 4 | src/web/{spawn,proxy,plugins,ext,ws,mod,hub}.rs, src/tui/{mod,plugins}.rs (visibility), tests/web.rs | Tasks 6–10 |
| 6 Vendoring and the asset budget | — | web/vendor/**, web/fonts/**, web/index.html (import map) | Tasks 1–5 |
| 7 App foundation and mock server | 6 | web/index.html, web/app/{boot,main,proto,model,store,keymap,fuzzy,palette,settings,theme,lazy}.js, web/app/ui/{app,topbar,toasts,help,settings,palette,modal}.js, web/styles/*, web/dev/mock-server.mjs, web/e2e/package.json(+lock), web/.gitignore, web/tests/{proto,model,keymap,fuzzy}.test.mjs | Tasks 1–5 |
| 8 Sessions UI | 7 | web/app/{queue,notify,panes,gauge,term}.js, web/app/ui/{rail,board,queue,pane,composer,strip,session-actions,narrow}.js, web/styles/*, web/dev/mock-server.mjs, web/tests/{queue,notify,panes,gauge}.test.mjs | Tasks 1–5 |
| 9 Wizard, reader, search, snippets, extension actions | 8 | web/app/{wizard,render/*}.js, web/app/ui/{wizard,reader,search,snippets,actions}.js, web/styles/*, web/dev/mock-server.mjs, web/tests/{markdown,wizard}.test.mjs | Tasks 1–5 |
| 10 Changes, history, stats, extensions/plugins, claude toasts, host panel | 9 | web/app/{diff,charts}.js, web/app/ui/{drawer,changes,history,stats,extensions,plugin-setup,claude,hosts}.js, web/styles/*, web/dev/mock-server.mjs, web/tests/{diff,charts}.test.mjs | Tasks 1–5 |
| 11 E2E, Makefile, docs | 5 and 10 (both lanes merged into feat/web-ui) | web/e2e/**, Makefile, docs/web.md, README.md, CLAUDE.md, .delivery/2026-11-08-web-ui/parity.md (not committed) | — |

Backend lane (1→2→3→4→5) and frontend lane (6→7→8→9→10) run concurrently in two worktrees; inside a lane tasks run in order. The lanes share no file. The frontend lane develops against `web/dev/mock-server.mjs`, which implements docs/web-protocol.md; the backend lane's `tests/web.rs` works with or without a real `web/` (build.rs embeds a stub `index.html` when `web/index.html` is missing). Task 11 runs on feat/web-ui after both lanes are merged into it.

## Tasks

### Task 1: Daemon attention event
Model: standard   Repo: /volumes/repos/claudio   Files: src/proto.rs, src/claude/state.rs, src/daemon/session.rs, src/daemon/mod.rs, src/daemon/tests.rs; mechanical only: src/tui/interaction.rs (the `SessionEvent::Unknown => {}` arm at :906), every `SessionInfo { … }` literal the compiler names (src/tui/state.rs:274, src/tui/test_support.rs:54 and :73, src/tui/git_app.rs:81, src/tui/app.rs:1272, :1331, :1361, :1623, src/freshness.rs:278, src/daemon/mod.rs:592 via `dormant_info`)
Goal: AC5. `Tracker` reports what a waiting session asks (`SessionEvent::Attention`), `SessionInfo.attention` carries the current one, `DAEMON_FEATURES` gains `"attention"`.
Depends on: none (backend lane start: feat/web-ui at the commit that adds this plan)
Patch: none

Step 1: write the failing tests.

In `src/claude/state.rs`, replace the bodies of the two tests `worker_permission_prompt_needs_approval` and `elicitation_url_dialog_needs_approval`, change `permission_working_stop_flow`, and add the new tests (keep every other existing test as it is):
```rust
    fn attention(evs: &[SessionEvent]) -> Option<&Attention> {
        evs.iter().find_map(|e| match e {
            SessionEvent::Attention { attention } => Some(attention),
            _ => None,
        })
    }

    fn state_event(state: SessionState) -> SessionEvent {
        SessionEvent::State { state }
    }

    #[test]
    fn permission_working_stop_flow() {
        let mut t = Tracker::new();
        assert_eq!(state(&t), SessionState::Starting);
        let evs = t.apply("UserPromptSubmit", &json!({"prompt": "do something"}));
        assert_eq!(evs, vec![state_event(SessionState::Working)]);
        let evs = t.apply(
            "Notification",
            &json!({"notification_type": "permission_prompt", "message": "Allow bash?"}),
        );
        assert_eq!(evs.len(), 2);
        assert_eq!(evs[0], state_event(SessionState::NeedsApproval));
        let a = attention(&evs).expect("an attention");
        assert_eq!((a.seq, a.kind.as_str(), a.message.as_str()), (1, "permission_prompt", "Allow bash?"));
        let evs = t.apply("PreToolUse", &json!({"tool_name": "Bash"}));
        assert_eq!(evs, vec![state_event(SessionState::Working)]);
        assert_eq!(t.attention, None);
        let evs = t.apply("Stop", &json!({"stop_hook_active": false}));
        assert_eq!(evs, vec![state_event(SessionState::Idle)]);
        assert_eq!(state(&t), SessionState::Idle);
    }

    #[test]
    fn worker_permission_prompt_needs_approval() {
        let mut t = Tracker::new();
        t.apply("UserPromptSubmit", &json!({}));
        let evs = t.apply("Notification", &json!({"notification_type": "worker_permission_prompt"}));
        assert_eq!(evs[0], state_event(SessionState::NeedsApproval));
        assert_eq!(attention(&evs).unwrap().kind, "worker_permission_prompt");
    }

    #[test]
    fn elicitation_url_dialog_needs_approval() {
        let mut t = Tracker::new();
        let evs = t.apply("Notification", &json!({"notification_type": "elicitation_url_dialog"}));
        assert_eq!(evs[0], state_event(SessionState::NeedsApproval));
        let a = attention(&evs).unwrap();
        assert_eq!((a.kind.as_str(), a.tool.as_deref()), ("elicitation_url_dialog", None));
    }

    /// AC5: the tool call before the prompt names what is asked.
    #[test]
    fn permission_after_pretooluse_names_the_tool_and_its_command() {
        let mut t = Tracker::new();
        t.apply("UserPromptSubmit", &json!({"prompt": "test it"}));
        t.apply(
            "PreToolUse",
            &json!({"tool_name": "Bash", "tool_input": {"command": "cargo test --all", "description": "run"}}),
        );
        let evs = t.apply(
            "Notification",
            &json!({"notification_type": "permission_prompt", "message": "Claude needs your permission to use Bash"}),
        );
        let expected = Attention {
            seq: 1,
            kind: "permission_prompt".into(),
            message: "Claude needs your permission to use Bash".into(),
            tool: Some("Bash".into()),
            detail: Some("cargo test --all".into()),
        };
        assert_eq!(
            evs,
            vec![
                state_event(SessionState::NeedsApproval),
                SessionEvent::Attention { attention: expected.clone() },
            ]
        );
        assert_eq!(t.attention, Some(expected));
    }

    /// AC5: an attention without a state change is still sent.
    #[test]
    fn idle_prompt_after_session_start_emits_an_attention_without_a_state() {
        let mut t = Tracker::new();
        let evs = t.apply("SessionStart", &json!({"session_id": "c1", "source": "startup"}));
        assert_eq!(evs.len(), 3, "{evs:?}");
        assert!(evs.contains(&state_event(SessionState::NeedsInput)));
        let start = attention(&evs).unwrap();
        assert_eq!((start.seq, start.kind.as_str(), start.tool.as_deref()), (1, SESSION_START, None));
        let evs = t.apply(
            "Notification",
            &json!({"notification_type": "idle_prompt", "message": "Claude is waiting for your input"}),
        );
        assert_eq!(
            evs,
            vec![SessionEvent::Attention {
                attention: Attention {
                    seq: 2,
                    kind: "idle_prompt".into(),
                    message: "Claude is waiting for your input".into(),
                    tool: None,
                    detail: None,
                }
            }]
        );
    }

    /// AC5: two prompts in a row, two attentions with a rising seq.
    #[test]
    fn permission_then_elicitation_emits_two_attentions_with_rising_seq() {
        let mut t = Tracker::new();
        t.apply("PreToolUse", &json!({"tool_name": "Bash", "tool_input": {"command": "ls"}}));
        let first = t.apply("Notification", &json!({"notification_type": "permission_prompt", "message": "a"}));
        let second = t.apply("Notification", &json!({"notification_type": "elicitation_dialog", "message": "pick one"}));
        assert_eq!(attention(&first).unwrap().seq, 1);
        assert_eq!(second.len(), 1, "no State: the session still needs approval");
        let a = attention(&second).unwrap();
        assert_eq!((a.seq, a.kind.as_str(), a.message.as_str()), (2, "elicitation_dialog", "pick one"));
        assert_eq!(t.attention.as_ref().map(|a| a.seq), Some(2));
    }

    #[test]
    fn tool_summaries_pick_the_useful_field() {
        let cases = [
            (json!({"tool_name": "Edit", "tool_input": {"file_path": "/w/src/a.rs", "old_string": "x"}}), "Edit", "/w/src/a.rs"),
            (json!({"tool_name": "Write", "tool_input": {"file_path": "/w/b.md", "content": "y"}}), "Write", "/w/b.md"),
            (json!({"tool_name": "Read", "tool_input": {"file_path": "/w/c"}}), "Read", "/w/c"),
            (json!({"tool_name": "WebFetch", "tool_input": {"url": "https://example.org", "prompt": "p"}}), "WebFetch", "https://example.org"),
            (json!({"tool_name": "Grep", "tool_input": {"pattern": "fn main"}}), "Grep", "fn main"),
            (json!({"tool_name": "Glob", "tool_input": {"pattern": "**/*.rs"}}), "Glob", "**/*.rs"),
            (json!({"tool_name": "mcp__kb__search", "tool_input": {"q": "x"}}), "mcp__kb__search", r#"{"q":"x"}"#),
        ];
        for (pre, tool, detail) in cases {
            let mut t = Tracker::new();
            t.apply("PreToolUse", &pre);
            let evs = t.apply("Notification", &json!({"notification_type": "permission_prompt"}));
            let a = attention(&evs).unwrap();
            assert_eq!((a.tool.as_deref(), a.detail.as_deref()), (Some(tool), Some(detail)), "{pre}");
        }
    }

    #[test]
    fn attention_text_is_sanitized_one_line_and_capped() {
        let mut t = Tracker::new();
        let long = "é".repeat(3000); // 6000 bytes
        t.apply(
            "PreToolUse",
            &json!({"tool_name": "Bash", "tool_input": {"command": format!("echo \x1b[31mred\u{202e}\n{long}")}}),
        );
        let evs = t.apply(
            "Notification",
            &json!({"notification_type": "permission_prompt", "message": format!("line1\nline2\x07{long}")}),
        );
        let a = attention(&evs).unwrap();
        let detail = a.detail.as_deref().unwrap();
        for text in [a.message.as_str(), detail] {
            assert!(!text.chars().any(|c| c.is_control() || c == '\u{202e}'), "{text:?}");
        }
        assert!(a.message.len() <= MAX_MESSAGE && a.message.starts_with("line1 line2é"), "{}", a.message);
        assert!(detail.len() <= MAX_DETAIL && detail.starts_with("echo [31mred é"), "{detail}");
    }

    #[test]
    fn turn_events_clear_the_attention() {
        for event in ["PreToolUse", "UserPromptSubmit", "Stop", "StopFailure"] {
            let mut t = Tracker::new();
            t.apply("Notification", &json!({"notification_type": "permission_prompt"}));
            assert!(t.attention.is_some());
            t.apply(event, &json!({"tool_name": "Bash"}));
            assert_eq!(t.attention, None, "{event}");
        }
    }

    #[test]
    fn unknown_notifications_leave_the_attention_and_seq_alone() {
        let mut t = Tracker::new();
        t.apply("Notification", &json!({"notification_type": "idle_prompt"}));
        let evs = t.apply("Notification", &json!({"notification_type": "push_notification", "message": "foo"}));
        assert!(evs.is_empty());
        assert_eq!(t.attention.as_ref().map(|a| a.seq), Some(1));
        let evs = t.apply("Notification", &json!({"notification_type": "idle_prompt"}));
        assert_eq!(attention(&evs).unwrap().seq, 2);
    }
```

In `src/proto.rs` `mod tests`, add:
```rust
    #[test]
    fn attention_is_nested_and_unknown_to_old_clients() {
        #[derive(Debug, PartialEq, Deserialize)]
        #[serde(tag = "kind", rename_all = "snake_case")]
        enum OldEvent {
            Removed,
            #[serde(other)]
            Unknown,
        }
        let attention = Attention {
            seq: 3,
            kind: "permission_prompt".into(),
            message: "m".into(),
            tool: Some("Bash".into()),
            detail: Some("ls".into()),
        };
        let ev = SessionEvent::Attention { attention: attention.clone() };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(
            json.starts_with(r#"{"kind":"attention","attention":{"seq":3,"kind":"permission_prompt""#),
            "{json}"
        );
        assert_eq!(serde_json::from_str::<OldEvent>(&json).unwrap(), OldEvent::Unknown);
        assert_eq!(serde_json::from_str::<SessionEvent>(&json).unwrap(), ev);
        let old = r#"{"id":"00000000-0000-0000-0000-000000000000","cwd":"/w",
            "state":"needs_approval","created_at":1}"#;
        let mut info: SessionInfo = serde_json::from_str(old).unwrap();
        assert_eq!(info.attention, None);
        info.attention = Some(attention);
        let back: SessionInfo = serde_json::from_str(&serde_json::to_string(&info).unwrap()).unwrap();
        assert_eq!(back, info);
        assert!(DAEMON_FEATURES.contains(&"attention"));
    }
```

In `src/daemon/tests.rs`, add (next to `hooks_update_state_and_journal`):
```rust
/// AC5: `ListSessions` carries the attention while the session waits, and
/// none once a tool runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_info_carries_the_attention_while_waiting() {
    let d = TestDaemon::start().await;
    let mut c = d.client().await;
    let id = Uuid::new_v4();
    c.spawn(d.spec(id)).await;
    let token = d.token().await;
    let socket = &d.config.socket;
    let pre = serde_json::json!({"tool_name": "Bash", "tool_input": {"command": "rm -rf build"}});
    send_hook(socket, &token, "PreToolUse", pre).await;
    let ask = serde_json::json!({
        "notification_type": "permission_prompt",
        "message": "Claude needs your permission to use Bash"
    });
    send_hook(socket, &token, "Notification", ask).await;
    let attention = c
        .event(id, |e| match e {
            SessionEvent::Attention { attention } => Some(attention.clone()),
            _ => None,
        })
        .await;
    assert_eq!(attention.kind, "permission_prompt");
    assert_eq!(attention.tool.as_deref(), Some("Bash"));
    assert_eq!(attention.detail.as_deref(), Some("rm -rf build"));
    let info = c.sessions().await.into_iter().find(|s| s.id == id).unwrap();
    assert_eq!(info.state, SessionState::NeedsApproval);
    assert_eq!(info.attention, Some(attention));

    send_hook(socket, &token, "PreToolUse", serde_json::json!({"tool_name": "Bash"})).await;
    c.event(id, |e| match e {
        SessionEvent::State { state: SessionState::Working } => Some(()),
        _ => None,
    })
    .await;
    let info = c.sessions().await.into_iter().find(|s| s.id == id).unwrap();
    assert_eq!(info.attention, None);
}
```
Run: `cargo test --locked --bin claudio claude::state`
Expected: build failure, `error[E0412]: cannot find type `Attention`` / `no variant named `Attention``.

Step 2: implement.

`src/proto.rs`: add next to `SessionInfo`:
```rust
/// What a session waits for, from the hook that asked
/// ([`SessionEvent::Attention`], [`SessionInfo::attention`]). Every string is
/// sanitized and on one line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attention {
    /// Grows with every attention of the session's process, from 1.
    pub seq: u64,
    /// The Notification's `notification_type` (`permission_prompt`,
    /// `worker_permission_prompt`, `elicitation_dialog`,
    /// `elicitation_url_dialog`, `idle_prompt`, `agent_needs_input`), or
    /// `session_start`; at most 64 bytes.
    pub kind: String,
    /// The Notification's `message`; at most 512 bytes.
    #[serde(default)]
    pub message: String,
    /// The most recent `PreToolUse`'s tool, for the approval kinds. Best
    /// effort: in a parallel burst it may not be the tool that asks.
    #[serde(default)]
    pub tool: Option<String>,
    /// That tool call in one line (its command, file, URL or pattern, else
    /// its input as compact JSON); at most 1024 bytes.
    #[serde(default)]
    pub detail: Option<String>,
}
```
Add the field at the end of `SessionInfo`:
```rust
    /// What the session waits for, while its state wants attention. Old
    /// daemons omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attention: Option<Attention>,
```
Add the variant to `SessionEvent` (before `Unknown`):
```rust
    /// The session asks for the user: a permission or dialog answer, or its
    /// next message. Sent for every such hook, after any `State` event of the
    /// same hook, whether or not the state changed. Nested, not flattened:
    /// the payload's `kind` would collide with this enum's tag. Old clients
    /// map it to `Unknown`. A client drops its attention on any `State` that
    /// leaves `needs_approval`/`needs_input`.
    Attention { attention: Attention },
```
Extend `DAEMON_FEATURES` and its doc: `` `attention` — it sends [`SessionEvent::Attention`] and fills [`SessionInfo::attention`] `` → `pub const DAEMON_FEATURES: &[&str] = &["lens", "search", "last_prompt", "attention"];`.

`src/claude/state.rs`: replace everything above `#[cfg(test)]` with:
```rust
//! Pure state machine that turns Claude Code hook events into
//! [`proto::SessionEvent`]s.
//!
//! The tracker holds the current [`SessionState`], the active claude
//! conversation id and what the session waits for ([`Attention`]). Every
//! call to [`Tracker::apply`] returns the events to broadcast: real state or
//! id changes, and one `Attention` for every hook that asks for the user,
//! whether or not the state changed (a second prompt in a row is news).

use serde_json::Value;

use crate::claude::latex::sanitize;
use crate::proto::{Attention, SessionEvent, SessionState};

/// The most an attention's `kind` takes, in bytes.
pub const MAX_KIND: usize = 64;
/// The most an attention's `message` takes, in bytes.
pub const MAX_MESSAGE: usize = 512;
/// The most an attention's `detail` takes, in bytes.
pub const MAX_DETAIL: usize = 1024;
/// The most a tool name takes, in bytes.
pub const MAX_TOOL: usize = 64;
/// The kind of the attention a `SessionStart` (startup, resume, clear) sets.
pub const SESSION_START: &str = "session_start";

/// Notification types that wait for an approval or a dialog's answer.
const APPROVAL: [&str; 4] = [
    "permission_prompt",
    "worker_permission_prompt",
    "elicitation_dialog",
    "elicitation_url_dialog",
];
/// Notification types that wait for the next message.
const INPUT: [&str; 2] = ["idle_prompt", "agent_needs_input"];

/// Per-session hook event tracker.
///
/// Start a fresh `Tracker` when a session is spawned. Feed each incoming hook
/// payload to [`apply`](Self::apply); it returns the `SessionEvent`s to
/// broadcast.
#[derive(Debug, Clone)]
pub struct Tracker {
    /// Current derived state.
    pub state: SessionState,
    /// The claude conversation id reported by the most recent `SessionStart`.
    pub claude_session_id: Option<String>,
    /// What the session waits for; `None` unless a hook asked and no turn
    /// event came since.
    pub attention: Option<Attention>,
    /// The last `seq` given out (0: none yet).
    seq: u64,
    /// The most recent `PreToolUse` of this turn: tool name and summary.
    last_tool: Option<(String, String)>,
}

impl Tracker {
    /// Construct a tracker in the `Starting` state (no hook has fired yet).
    pub fn new() -> Self {
        Self {
            state: SessionState::Starting,
            claude_session_id: None,
            attention: None,
            seq: 0,
            last_tool: None,
        }
    }

    /// Apply one hook event and return the list of protocol events to broadcast.
    ///
    /// Redundant state transitions are suppressed; an asking hook always
    /// yields an `Attention`. Unknown events and unknown notification types
    /// are ignored.
    pub fn apply(&mut self, event: &str, payload: &Value) -> Vec<SessionEvent> {
        match event {
            "SessionStart" => self.handle_session_start(payload),
            "PreToolUse" => {
                self.last_tool = tool_summary(payload);
                self.attention = None;
                self.transition(SessionState::Working)
            }
            "UserPromptSubmit" => self.end_wait(SessionState::Working),
            "Notification" => self.handle_notification(payload),
            "Stop" => self.end_wait(SessionState::Idle),
            "StopFailure" => self.end_wait(SessionState::Error),
            // `SessionEnd`: process exit is reported separately.
            _ => vec![],
        }
    }

    // ── private helpers ───────────────────────────────────────────────────────

    /// A turn boundary: nothing is asked any more, and the next prompt's
    /// tool comes from a new `PreToolUse`.
    fn end_wait(&mut self, next: SessionState) -> Vec<SessionEvent> {
        self.last_tool = None;
        self.attention = None;
        self.transition(next)
    }

    fn handle_session_start(&mut self, payload: &Value) -> Vec<SessionEvent> {
        let mut events = Vec::new();

        // Journal the new claude session id if it changed.
        if let Some(new_id) = payload.get("session_id").and_then(Value::as_str) {
            if self.claude_session_id.as_deref() != Some(new_id) {
                self.claude_session_id = Some(new_id.to_owned());
                events.push(SessionEvent::ClaudeSession {
                    claude_session_id: new_id.to_owned(),
                });
            }
        }

        // On startup/resume/clear the session is at its prompt — NeedsInput.
        let source = payload.get("source").and_then(Value::as_str).unwrap_or("");
        if matches!(source, "startup" | "resume" | "clear") {
            self.last_tool = None;
            events.extend(self.transition(SessionState::NeedsInput));
            events.push(self.attend(SESSION_START, "", false));
        }

        events
    }

    fn handle_notification(&mut self, payload: &Value) -> Vec<SessionEvent> {
        let ntype = payload
            .get("notification_type")
            .and_then(Value::as_str)
            .unwrap_or("");
        let (next, approval) = if APPROVAL.contains(&ntype) {
            (SessionState::NeedsApproval, true)
        } else if INPUT.contains(&ntype) {
            (SessionState::NeedsInput, false)
        } else {
            // All other notification types change nothing.
            return vec![];
        };
        let message = payload.get("message").and_then(Value::as_str).unwrap_or("");
        let mut events = self.transition(next);
        events.push(self.attend(ntype, message, approval));
        events
    }

    /// Record and return a new attention. The tool of this turn is named for
    /// the approval kinds only.
    fn attend(&mut self, kind: &str, message: &str, with_tool: bool) -> SessionEvent {
        self.seq += 1;
        let (tool, detail) = match (&self.last_tool, with_tool) {
            (Some((tool, detail)), true) => (
                Some(tool.clone()),
                Some(detail.clone()).filter(|d| !d.is_empty()),
            ),
            _ => (None, None),
        };
        let attention = Attention {
            seq: self.seq,
            kind: one_line(kind, MAX_KIND),
            message: one_line(message, MAX_MESSAGE),
            tool,
            detail,
        };
        self.attention = Some(attention.clone());
        SessionEvent::Attention { attention }
    }

    /// Transition to `next`, returning a `State` event only if the state
    /// actually changed.
    fn transition(&mut self, next: SessionState) -> Vec<SessionEvent> {
        if self.state == next {
            return vec![];
        }
        self.state = next;
        vec![SessionEvent::State { state: next }]
    }
}

impl Default for Tracker {
    fn default() -> Self {
        Self::new()
    }
}

/// A `PreToolUse` payload's tool name and one-line summary: the command
/// (Bash), the file (Edit, Write, Read), the URL (WebFetch), the pattern
/// (Grep, Glob), else the input as compact JSON.
fn tool_summary(payload: &Value) -> Option<(String, String)> {
    let name = payload.get("tool_name")?.as_str()?;
    let input = payload.get("tool_input").filter(|i| !i.is_null());
    let field = |key: &str| input.and_then(|i| i.get(key)).and_then(Value::as_str);
    let picked = match name {
        "Bash" => field("command"),
        "Edit" | "Write" | "Read" => field("file_path"),
        "WebFetch" => field("url"),
        "Grep" | "Glob" => field("pattern"),
        _ => None,
    };
    let summary = match picked {
        Some(text) => one_line(text, MAX_DETAIL),
        None => input
            .map(|i| one_line(&i.to_string(), MAX_DETAIL))
            .unwrap_or_default(),
    };
    Some((one_line(name, MAX_TOOL), summary))
}

/// `text` sanitized, on one line (newlines and tabs become spaces), cut to
/// at most `max` bytes on a char boundary.
fn one_line(text: &str, max: usize) -> String {
    // Bound the work on a huge field before sanitizing it.
    let text = &text[..char_floor(text, max.saturating_mul(4))];
    let flat: String = text
        .chars()
        .map(|c| if matches!(c, '\n' | '\r' | '\t') { ' ' } else { c })
        .collect();
    let mut out = sanitize(&flat);
    out.truncate(char_floor(&out, max));
    out
}

/// The largest char boundary of `s` at or below `max`.
fn char_floor(s: &str, max: usize) -> usize {
    if s.len() <= max {
        return s.len();
    }
    let mut cut = max;
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    cut
}
```
Add `use crate::proto::Attention;` to the test module's imports if `use super::*;` does not already bring it (it does).

`src/daemon/session.rs`: `pub struct Status` gains `pub attention: Option<crate::proto::Attention>,`; the `Cmd::Status(reply)` arm (:916) sends `attention: self.tracker.attention.clone()`.

`src/daemon/mod.rs`: in `sessions()` (:766) set `info.attention = status.attention;` beside `info.state`/`info.title`; `dormant_info` sets `attention: None`.

Mechanical: add `attention: None,` to every `SessionInfo { … }` literal the compiler names (list in Files); in `src/tui/interaction.rs:906` change `SessionEvent::Unknown => {}` to `SessionEvent::Attention { .. } | SessionEvent::Unknown => {}` (the TUI ignores attentions exactly as an old client does). Same for any other exhaustive `SessionEvent` match the compiler names outside `src/web/`.

Run: `cargo test --locked --bin claudio claude::state` → `test result: ok. 16 passed`
Run: `cargo test --locked --bin claudio attention` → the proto test and `daemon::tests::session_info_carries_the_attention_while_waiting` pass (plus the state tests whose names contain `attention`), `0 failed`
Run: `make test-unit` → `0 failed`; `make test-manager` → `0 failed`

Step 3: commit
`git commit -m "feat(daemon): attention events say what a waiting session asks" -- src/proto.rs src/claude/state.rs src/daemon/session.rs src/daemon/mod.rs src/daemon/tests.rs src/freshness.rs src/tui/interaction.rs src/tui/state.rs src/tui/test_support.rs src/tui/git_app.rs src/tui/app.rs` (plus any other file the compiler made you touch for a literal, listed explicitly)

Interfaces this task provides to later tasks: `proto::Attention { seq: u64, kind: String, message: String, tool: Option<String>, detail: Option<String> }`; `proto::SessionEvent::Attention { attention: Attention }`; `proto::SessionInfo::attention: Option<Attention>`; daemon feature `"attention"`; `claude::state::{MAX_KIND, MAX_MESSAGE, MAX_DETAIL, MAX_TOOL, SESSION_START}`.

### Task 2: Daemon worktree ops
Model: strong   Repo: /volumes/repos/claudio   Files: src/proto.rs, src/daemon/git.rs, src/daemon/server.rs, src/daemon/tests.rs
Goal: AC6. `Msg::GitWorktree` → `Msg::GitWorktreeStatus` and `Msg::GitWorktreeDiff` → `Msg::GitPatch`, read-only, through the existing git runner and `spawn_git`; feature `"worktree"`.
Depends on: Task 1
Patch: none

Step 1: write the failing tests.

`src/proto.rs` tests:
```rust
    #[test]
    fn worktree_ops_are_unknown_to_older_peers() {
        #[derive(Debug, PartialEq, Deserialize)]
        #[serde(tag = "op", rename_all = "snake_case")]
        enum Old {
            Ping,
            #[serde(other)]
            Unknown,
        }
        let status = GitWorktreeStatus {
            root: "/w".into(),
            branch: Some("main".into()),
            files: vec![GitWorktreeFile {
                path: "a b.txt".into(),
                old_path: None,
                index: ".".into(),
                worktree: "M".into(),
                added: Some(1),
                removed: Some(0),
            }],
            truncated: false,
        };
        for msg in [
            Msg::GitWorktree { cwd: "/w".into() },
            Msg::GitWorktreeDiff { cwd: "/w".into(), path: "a b.txt".into(), staged: true },
            Msg::GitWorktreeStatus(status),
        ] {
            let json = serde_json::to_string(&Envelope::request(1, msg.clone())).unwrap();
            assert_eq!(serde_json::from_str::<Old>(&json).unwrap(), Old::Unknown, "{json}");
            assert_eq!(serde_json::from_str::<Envelope>(&json).unwrap().msg, msg);
        }
        assert!(DAEMON_FEATURES.contains(&"worktree"));
    }
```

`src/daemon/git.rs` tests (pure):
```rust
    #[test]
    fn worktree_paths_must_be_relative_and_plain() {
        for ok in ["a.txt", "src/main.rs", "dir with space/f", ".hidden", "a..b"] {
            assert_eq!(check_worktree_path(ok), Ok(()), "{ok}");
        }
        for bad in ["", "/etc/passwd", "../x", "a/../b", "a/..", "..", "a\0b"] {
            assert_eq!(check_worktree_path(bad), Err(NO_SUCH_PATH.to_owned()), "{bad:?}");
        }
        assert!(check_worktree_path(&"x".repeat(MAX_PATH + 1)).is_err());
    }

    #[test]
    fn porcelain_v2_records_parse_with_renames_unmerged_and_untracked() {
        let data = b"# branch.oid 0123456789012345678901234567890123456789\0\
# branch.head main\0\
1 .M N... 100644 100644 100644 aaaa bbbb a file.txt\0\
2 R. N... 100644 100644 100644 aaaa bbbb R100 new.txt\0old.txt\0\
u UU N... 100644 100644 100644 100644 aaaa bbbb cccc conflict.rs\0\
? untracked dir/u.txt\0\
! ignored.log\0\
1 M. N... 100644 100644 100644 aaaa bbbb cut-off";
        let parsed = parse_worktree(data);
        assert_eq!(parsed.branch.as_deref(), Some("main"));
        assert!(!parsed.truncated);
        let got: Vec<(&[u8], Option<&[u8]>, u8, u8)> = parsed
            .entries
            .iter()
            .map(|e| (e.path.as_slice(), e.old_path.as_deref(), e.index, e.worktree))
            .collect();
        assert_eq!(
            got,
            vec![
                (&b"a file.txt"[..], None, b'.', b'M'),
                (&b"new.txt"[..], Some(&b"old.txt"[..]), b'R', b'.'),
                (&b"conflict.rs"[..], None, b'U', b'U'),
                (&b"untracked dir/u.txt"[..], None, b'?', b'?'),
            ]
        );
        let detached = parse_worktree(b"# branch.head (detached)\0");
        assert_eq!(detached.branch, None);
    }

    #[test]
    fn worktree_entries_are_capped() {
        let mut data = Vec::new();
        for i in 0..=proto::GIT_WORKTREE_MAX {
            data.extend_from_slice(format!("? f{i}\0").as_bytes());
        }
        let parsed = parse_worktree(&data);
        assert_eq!(parsed.entries.len(), proto::GIT_WORKTREE_MAX);
        assert!(parsed.truncated);
    }
```

`src/daemon/tests.rs` (daemon level; one repo builder plus five tests):
```rust
// ── Working tree (the web's Changes view) ────────────────────────────────────

/// A repo on `main` with: `a.txt` modified (unstaged), `b.txt` staged then
/// modified again, `c.txt` added, `old.txt` renamed to `new.txt`, `u.txt`
/// untracked, `link` an untracked symlink to /etc/passwd, `clean.txt`
/// unchanged.
fn worktree_repo() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("claudio-worktree-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let git = |args: &[&str]| {
        let ok = std::process::Command::new("git")
            .args(args)
            .current_dir(&dir)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.org")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.org")
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    };
    git(&["init", "-q", "-b", "main"]);
    for (name, text) in [("a.txt", "a\n"), ("b.txt", "b\n"), ("old.txt", "old\n"), ("clean.txt", "clean\n")] {
        std::fs::write(dir.join(name), text).unwrap();
    }
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "base"]);
    std::fs::write(dir.join("a.txt"), "a\nchanged\n").unwrap();
    std::fs::write(dir.join("b.txt"), "b\nstaged\n").unwrap();
    git(&["add", "b.txt"]);
    std::fs::write(dir.join("b.txt"), "b\nstaged\nagain\n").unwrap();
    std::fs::write(dir.join("c.txt"), "c\n").unwrap();
    git(&["add", "c.txt"]);
    git(&["mv", "old.txt", "new.txt"]);
    std::fs::write(dir.join("u.txt"), "u\n").unwrap();
    std::os::unix::fs::symlink("/etc/passwd", dir.join("link")).unwrap();
    dir
}

async fn worktree_patch(c: &mut Client, cwd: &str, path: &str, staged: bool) -> Msg {
    c.call(Msg::GitWorktreeDiff {
        cwd: cwd.to_owned(),
        path: path.to_owned(),
        staged,
    })
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worktree_status_lists_staged_unstaged_untracked_and_renamed() {
    let d = TestDaemon::start().await;
    let repo = worktree_repo();
    let mut c = d.client().await;
    let cwd = repo.to_string_lossy().into_owned();
    let Msg::GitWorktreeStatus(s) = c.call(Msg::GitWorktree { cwd }).await else {
        panic!("not a status");
    };
    assert_eq!(s.branch.as_deref(), Some("main"));
    assert_eq!(Path::new(&s.root), std::fs::canonicalize(&repo).unwrap());
    assert!(!s.truncated);
    let f = |p: &str| {
        s.files
            .iter()
            .find(|f| f.path == p)
            .unwrap_or_else(|| panic!("{p} not in {:?}", s.files))
    };
    let a = f("a.txt");
    assert_eq!((a.index.as_str(), a.worktree.as_str(), a.added, a.removed), (".", "M", Some(1), Some(0)));
    let b = f("b.txt");
    assert_eq!((b.index.as_str(), b.worktree.as_str(), b.added, b.removed), ("M", "M", Some(2), Some(0)));
    let c_txt = f("c.txt");
    assert_eq!((c_txt.index.as_str(), c_txt.worktree.as_str()), ("A", "."));
    let renamed = f("new.txt");
    assert_eq!((renamed.index.as_str(), renamed.old_path.as_deref()), ("R", Some("old.txt")));
    let u = f("u.txt");
    assert_eq!((u.index.as_str(), u.worktree.as_str(), u.added), ("?", "?", None));
    assert!(s.files.iter().all(|f| f.path != "clean.txt"));
    let _ = std::fs::remove_dir_all(&repo);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worktree_status_caps_the_file_count() {
    let d = TestDaemon::start().await;
    // The full fixture (it has a commit), plus more untracked files than fit.
    let repo = worktree_repo();
    for i in 0..=proto::GIT_WORKTREE_MAX {
        std::fs::write(repo.join(format!("f{i:04}")), "x").unwrap();
    }
    let mut c = d.client().await;
    let Msg::GitWorktreeStatus(s) = c.call(Msg::GitWorktree { cwd: repo.to_string_lossy().into_owned() }).await else {
        panic!("not a status");
    };
    assert_eq!(s.files.len(), proto::GIT_WORKTREE_MAX);
    assert!(s.truncated);
    let _ = std::fs::remove_dir_all(&repo);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worktree_diff_shows_staged_unstaged_untracked_and_renamed() {
    let d = TestDaemon::start().await;
    let repo = worktree_repo();
    let cwd = repo.to_string_lossy().into_owned();
    let mut c = d.client().await;
    let patch = |reply: Msg| match reply {
        Msg::GitPatch(p) => p,
        other => panic!("{other:?}"),
    };
    let a = patch(worktree_patch(&mut c, &cwd, "a.txt", false).await);
    assert!(a.patch.contains("+changed"), "{}", a.patch);
    assert_eq!((a.id.as_str(), a.path.as_deref()), ("worktree", Some("a.txt")));
    let b_staged = patch(worktree_patch(&mut c, &cwd, "b.txt", true).await);
    assert!(b_staged.patch.contains("+staged") && !b_staged.patch.contains("+again"), "{}", b_staged.patch);
    assert_eq!(b_staged.id, "index");
    let b_work = patch(worktree_patch(&mut c, &cwd, "b.txt", false).await);
    assert!(b_work.patch.contains("+again"), "{}", b_work.patch);
    let added = patch(worktree_patch(&mut c, &cwd, "c.txt", true).await);
    assert!(added.patch.contains("+c"), "{}", added.patch);
    let renamed = patch(worktree_patch(&mut c, &cwd, "new.txt", true).await);
    assert!(renamed.patch.contains("rename from old.txt"), "{}", renamed.patch);
    let untracked = patch(worktree_patch(&mut c, &cwd, "u.txt", false).await);
    assert!(untracked.patch.contains("/dev/null") && untracked.patch.contains("+u"), "{}", untracked.patch);
    let _ = std::fs::remove_dir_all(&repo);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worktree_diff_refuses_anything_but_a_listed_path() {
    let d = TestDaemon::start().await;
    let repo = worktree_repo();
    let cwd = repo.to_string_lossy().into_owned();
    let mut c = d.client().await;
    for path in ["../x", "/etc/passwd", "sub/../a.txt", "a.txt\0", "", "link", "nothere.txt", "clean.txt"] {
        let reply = worktree_patch(&mut c, &cwd, path, false).await;
        assert_eq!(reply, Msg::Error { message: "no such path".into() }, "{path:?}");
    }
    let _ = std::fs::remove_dir_all(&repo);
}

/// AC6: a git that never answers holds its own request only.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stuck_git_does_not_block_other_requests() {
    let d = TestDaemon::start().await;
    let stuck = d.dir.join("stuck");
    std::fs::create_dir_all(&stuck).unwrap();
    let fake = d.dir.join("fake-git");
    std::fs::write(&fake, "#!/bin/sh\nsleep 30\n").unwrap();
    std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    super::git::use_git_for(&stuck, &fake);
    let mut c = d.client().await;
    let _pending = c.request(Msg::GitWorktree { cwd: stuck.to_string_lossy().into_owned() }).await;
    let started = std::time::Instant::now();
    let reply = c.call(Msg::ListSessions).await;
    assert!(matches!(reply, Msg::Sessions { .. }), "{reply:?}");
    assert!(started.elapsed() < Duration::from_secs(2), "ListSessions waited on git");
}
```
(Adapt the helper names to `daemon/tests.rs`'s `Client` if they differ: `call` sends a request and returns its reply, `request` sends one and returns its `req`.)

Run: `cargo test --locked --bin claudio worktree`
Expected: build failure (`no variant named `GitWorktree``).

Step 2: implement.

`src/proto.rs`:
- In `Msg`, after `GitDiff`:
```rust
    /// The working tree of the repository `cwd` is in: every changed path,
    /// staged and unstaged, with line counts. Answered by
    /// `GitWorktreeStatus`. An older daemon answers `Error "unsupported
    /// op…"` (clients check the `worktree` feature first).
    GitWorktree {
        cwd: String,
    },
    /// One working-tree file's patch: the index against HEAD when `staged`,
    /// else the working tree against the index; an untracked file against
    /// /dev/null. `path` must be an entry of the daemon's own status,
    /// exactly; anything else answers `Error "no such path"`. Answered by
    /// `GitPatch` (`id` is `"index"` or `"worktree"`).
    GitWorktreeDiff {
        cwd: String,
        path: String,
        #[serde(default)]
        staged: bool,
    },
```
- In the replies, after `GitPatch(GitPatch)`: `GitWorktreeStatus(GitWorktreeStatus),`
- After `GitPatch`'s struct:
```rust
/// The most files a `GitWorktreeStatus` lists.
pub const GIT_WORKTREE_MAX: usize = 2000;

/// Reply to `GitWorktree`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitWorktreeStatus {
    /// The repository's top-level directory.
    pub root: String,
    /// The checked-out branch; `None` for a detached HEAD.
    #[serde(default)]
    pub branch: Option<String>,
    pub files: Vec<GitWorktreeFile>,
    /// More than [`GIT_WORKTREE_MAX`] files, or git's output was cut.
    #[serde(default)]
    pub truncated: bool,
}

/// A changed path of the working tree, relative to the root.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitWorktreeFile {
    pub path: String,
    /// The name before a rename in the index.
    #[serde(default)]
    pub old_path: Option<String>,
    /// One status letter for the index: `.` unchanged, `M`, `T`, `A`, `D`,
    /// `R`, `C`, `U` (unmerged), `?` (untracked).
    pub index: String,
    /// The same for the working tree.
    pub worktree: String,
    /// Lines added, staged and unstaged together; `None` for a binary or
    /// untracked file.
    #[serde(default)]
    pub added: Option<u32>,
    #[serde(default)]
    pub removed: Option<u32>,
}
```
- `DAEMON_FEATURES`: add `"worktree"` and its doc line (`` `worktree` — it answers [`Msg::GitWorktree`] and [`Msg::GitWorktreeDiff`] ``).

`src/daemon/git.rs` (module doc: "Five requests"; keep everything else):
```rust
/// The error for any working-tree path the daemon will not diff.
const NO_SUCH_PATH: &str = "no such path";

/// `status` for the working tree: NUL-separated, every untracked file
/// listed (not its directory), branch header included.
const WORKTREE_STATUS: [&str; 5] = [
    "status",
    "--porcelain=v2",
    "-z",
    "--branch",
    "--untracked-files=all",
];
```
In `handle`, two more arms:
```rust
            Msg::GitWorktree { cwd } => worktree(&cwd).await,
            Msg::GitWorktreeDiff { cwd, path, staged } => worktree_diff(&cwd, &path, staged).await,
```
New functions (the security-relevant ones in full):
```rust
async fn worktree(cwd: &str) -> Result<Msg, String> {
    let cwd = work_dir(cwd).await?;
    let root = repo_root(&cwd).await?;
    let unstaged = ["diff", "--numstat", "-z", "--no-ext-diff", "--no-textconv"];
    let staged = ["diff", "--cached", "--numstat", "-z", "--no-ext-diff", "--no-textconv", "-M"];
    let (status, unstaged, staged) = tokio::try_join!(
        run(&root, &WORKTREE_STATUS, META_CAP),
        run(&root, &unstaged, META_CAP),
        run(&root, &staged, META_CAP),
    )?;
    let parsed = parse_worktree(&status.stdout);
    // Staged and unstaged counts per path, summed; binary in either: none.
    let mut counts: std::collections::HashMap<Vec<u8>, (Option<u32>, Option<u32>)> =
        std::collections::HashMap::new();
    let entries = parse_stat_entries(&unstaged.stdout)
        .into_iter()
        .chain(parse_stat_entries(&staged.stdout));
    for e in entries {
        let sum = counts.entry(e.path).or_insert((Some(0), Some(0)));
        sum.0 = sum.0.zip(e.added).map(|(a, b)| a.saturating_add(b));
        sum.1 = sum.1.zip(e.removed).map(|(a, b)| a.saturating_add(b));
    }
    let files = parsed
        .entries
        .iter()
        .map(|e| {
            let (added, removed) = match e.index {
                b'?' => (None, None),
                _ => counts.get(&e.path).copied().unwrap_or((None, None)),
            };
            GitWorktreeFile {
                path: clean_line(&e.path, MAX_PATH),
                old_path: e.old_path.as_deref().map(|p| clean_line(p, MAX_PATH)),
                index: letter(e.index),
                worktree: letter(e.worktree),
                added,
                removed,
            }
        })
        .collect();
    Ok(Msg::GitWorktreeStatus(GitWorktreeStatus {
        root: clean_line(root.as_os_str().as_encoded_bytes(), MAX_PATH),
        branch: parsed.branch,
        files,
        truncated: parsed.truncated || status.truncated,
    }))
}

/// A working-tree path as a client may name it: relative, without a `..`
/// component or NUL, at most [`MAX_PATH`] bytes. Passing this is not
/// enough: [`worktree_diff`] also requires the daemon's own status to list
/// it, byte for byte.
fn check_worktree_path(path: &str) -> Result<(), String> {
    let plain = !path.is_empty()
        && path.len() <= MAX_PATH
        && !path.contains('\0')
        && !path.starts_with('/')
        && !path.split('/').any(|part| part == "..");
    if plain {
        Ok(())
    } else {
        Err(NO_SUCH_PATH.to_owned())
    }
}

async fn worktree_diff(cwd: &str, path: &str, staged: bool) -> Result<Msg, String> {
    check_worktree_path(path)?;
    let cwd = work_dir(cwd).await?;
    let root = repo_root(&cwd).await?;
    // Only a path this daemon's own status lists, byte for byte.
    let status = run(&root, &WORKTREE_STATUS, META_CAP).await?;
    let entry = parse_worktree(&status.stdout)
        .entries
        .into_iter()
        .find(|e| e.path == path.as_bytes())
        .ok_or_else(|| NO_SUCH_PATH.to_owned())?;
    let out = if entry.index == b'?' {
        // Untracked: a regular file only; `lstat`, so a symlink is refused.
        let meta = tokio::fs::symlink_metadata(root.join(path))
            .await
            .map_err(|_| NO_SUCH_PATH.to_owned())?;
        if !meta.file_type().is_file() {
            return Err(NO_SUCH_PATH.to_owned());
        }
        let args = [
            "diff", "--no-index", "--patch", "--no-ext-diff", "--no-textconv",
            "--", "/dev/null", path,
        ];
        // `--no-index` exits 1 when the two differ, which they do.
        run_codes(&root, &args, PATCH_CAP, &[0, 1]).await?
    } else {
        let mut args: Vec<OsString> = ["diff", "--patch", "--no-ext-diff", "--no-textconv", "-M"]
            .into_iter()
            .map(OsString::from)
            .collect();
        if staged {
            args.push("--cached".into());
        }
        args.push("--".into());
        args.push(os_string(entry.path));
        if let Some(old) = entry.old_path {
            args.push(os_string(old));
        }
        run(&root, &args, PATCH_CAP).await?
    };
    let id = if staged { "index" } else { "worktree" };
    let label = Some(clean_line(path.as_bytes(), MAX_PATH));
    Ok(Msg::GitPatch(patch_reply(id, label, out)))
}

/// One record of `status --porcelain=v2 -z`, its names exactly as git
/// printed them.
struct WorktreeEntry {
    path: Vec<u8>,
    old_path: Option<Vec<u8>>,
    index: u8,
    worktree: u8,
}

struct ParsedWorktree {
    branch: Option<String>,
    entries: Vec<WorktreeEntry>,
    truncated: bool,
}

/// Parse `status --porcelain=v2 -z --branch`: `# ` headers, then `1 XY` +
/// 6 fields + path, `2 XY` + 7 fields + path followed by the original path
/// as a field of its own, `u XY` + 8 fields + path, `? path`. Ignored (`!`)
/// and unknown records are skipped; a record cut by the capture is dropped
/// ([`nul_fields`]). At most [`proto::GIT_WORKTREE_MAX`] entries.
fn parse_worktree(data: &[u8]) -> ParsedWorktree {
    let mut out = ParsedWorktree {
        branch: None,
        entries: Vec::new(),
        truncated: false,
    };
    let mut fields = nul_fields(data).into_iter();
    while let Some(record) = fields.next() {
        let entry = match record.first() {
            Some(b'#') => {
                if let Some(head) = record.strip_prefix(b"# branch.head ") {
                    if head != b"(detached)" {
                        out.branch = Some(clean_line(head, MAX_NAME));
                    }
                }
                continue;
            }
            Some(b'1') => changed(record, 9),
            Some(b'2') => changed(record, 10).map(|mut e| {
                e.old_path = fields.next().map(<[u8]>::to_vec);
                e
            }),
            Some(b'u') => changed(record, 11),
            Some(b'?') => record.strip_prefix(b"? ").map(|p| WorktreeEntry {
                path: p.to_vec(),
                old_path: None,
                index: b'?',
                worktree: b'?',
            }),
            _ => None,
        };
        let Some(entry) = entry.filter(|e| !e.path.is_empty()) else {
            continue;
        };
        if out.entries.len() == proto::GIT_WORKTREE_MAX {
            out.truncated = true;
            break;
        }
        out.entries.push(entry);
    }
    out
}

/// A `1`/`2`/`u` record of `n` space-separated fields, the last one the
/// path (which may hold spaces).
fn changed(record: &[u8], n: usize) -> Option<WorktreeEntry> {
    let parts: Vec<&[u8]> = record.splitn(n, |&b| b == b' ').collect();
    let (xy, path) = (*parts.get(1)?, *parts.get(n - 1)?);
    let &[index, worktree] = xy else {
        return None;
    };
    Some(WorktreeEntry {
        path: path.to_vec(),
        old_path: None,
        index,
        worktree,
    })
}

/// A status letter for the client.
fn letter(b: u8) -> String {
    char::from(if b.is_ascii_graphic() { b } else { b'?' }).to_string()
}
```
(`as_encoded_bytes` needs Rust 1.74; use `std::os::unix::ffi::OsStrExt::as_bytes` instead if the toolchain is older.)

Refactor, behaviour unchanged for the existing ops:
- Extract the tail of `diff()` (sanitize, cut a truncated last line, fit the frame) into `fn patch_reply(id: &str, path: Option<String>, out: Captured) -> GitPatch`; `diff()` returns `Ok(Msg::GitPatch(patch_reply(id, label, out)))`.
- `run(cwd, args, cap)` becomes `run_codes(cwd, args, cap, &[0])`; in `run_codes` the success test is `status.code().is_some_and(|c| ok.contains(&c))` instead of `status.success()`.
- Test hook for the stuck-git test; `command(cwd)` starts `git_program(cwd)` instead of `"git"`:
```rust
/// Test hook: run `program` instead of git for directories under `dir`.
#[cfg(test)]
static GIT_FOR: std::sync::Mutex<Vec<(PathBuf, PathBuf)>> = std::sync::Mutex::new(Vec::new());

#[cfg(test)]
pub(in crate::daemon) fn use_git_for(dir: &Path, program: &Path) {
    GIT_FOR
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push((dir.to_path_buf(), program.to_path_buf()));
}

/// The git to run in `cwd`: always `git` outside tests.
fn git_program(cwd: &Path) -> PathBuf {
    #[cfg(test)]
    if let Some((_, program)) = GIT_FOR
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|(dir, _)| cwd.starts_with(dir))
    {
        return program.clone();
    }
    let _ = cwd;
    PathBuf::from("git")
}
```

`src/daemon/server.rs:487`: the git arm becomes `Msg::GitLog { .. } | Msg::GitCommit { .. } | Msg::GitDiff { .. } | Msg::GitWorktree { .. } | Msg::GitWorktreeDiff { .. } => { … spawn_git … }` (same body).

Run: `cargo test --locked --bin claudio worktree` → the proto test, the three git.rs tests and the four daemon worktree tests pass, `0 failed`
Run: `cargo test --locked --bin claudio a_stuck_git` → `1 passed`
Run: `cargo test --locked --bin claudio daemon::git` → every existing git test still passes
Run: `make test-unit` → `0 failed`; `make test-manager` → `0 failed`

Step 3: commit
`git commit -m "feat(daemon): working-tree status and patches for the web viewer" -- src/proto.rs src/daemon/git.rs src/daemon/server.rs src/daemon/tests.rs`

Interfaces this task provides to later tasks: `Msg::GitWorktree { cwd }`, `Msg::GitWorktreeDiff { cwd, path, staged }`, `Msg::GitWorktreeStatus(GitWorktreeStatus)`, `proto::{GitWorktreeStatus, GitWorktreeFile, GIT_WORKTREE_MAX}`, daemon feature `"worktree"`, error text `no such path`.

### Task 3: Web server core: `claudio web`, auth, embedded assets, WebSocket skeleton
Model: strong   Repo: /volumes/repos/claudio   Files: Cargo.toml, Cargo.lock, build.rs (new), src/main.rs, src/cli.rs, src/web/mod.rs (new), src/web/auth.rs (new), src/web/assets.rs (new), src/web/ws.rs (new), tests/web.rs (new)
Goal: AC1, AC2. `claudio web` listens on loopback only, prints the token URL, serves the embedded app, trades the token for a cookie, enforces Host/Origin/cookie, sets the CSP, and opens a WebSocket that says `hello` and sends a (still empty) `snapshot`.
Depends on: Task 2
Patch: none

Dependencies (`Cargo.toml`):
- `axum = { version = "0.8", features = ["ws"] }`
- `flate2 = "1"` (runtime: decompress for a client without gzip), `base64 = "0.22"` (runtime: the import-map hash in debug disk mode; already in Cargo.lock)
- `[build-dependencies]`: `flate2 = "1"`, `sha2 = "0.10"`, `base64 = "0.22"`
- `[dev-dependencies]`: `tokio-tungstenite = "<the version axum's ws feature pulls>"` (run `cargo tree -i tokio-tungstenite -e normal` after enabling `ws` and pin that exact minor so only one copy builds).
- Update Cargo.lock with `cargo build` (network: crates.io only), then build with `--locked` from here on.

Step 1: write the failing tests.

`tests/web.rs` (new; Tasks 4 and 5 add to it). The harness and the auth tests, in full:
```rust
//! `claudio web` integration tests: the CLI, authentication and request
//! checks, the browser protocol against a fake claude, spawn secrets.
//! Run single-threaded: `cargo test --test web -- --test-threads=1`.

mod common;

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::os::unix::fs::PermissionsExt;
use std::process::{Child, Command, Stdio};
use std::{fs, thread};

use common::{ManagerHarness, BINARY, DAEMON_WAIT, WAIT};
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::{self, client::IntoClientRequest, Message};

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// The isolated env of a test `claudio web` (and of the daemon it starts).
fn apply_env(h: &ManagerHarness, cmd: &mut Command) {
    for (k, _) in std::env::vars() {
        if k.starts_with("CLAUDIO_") {
            cmd.env_remove(&k);
        }
    }
    cmd.env("XDG_RUNTIME_DIR", &h.runtime_dir)
        .env("XDG_CONFIG_HOME", &h.config_home)
        .env("XDG_CACHE_HOME", h.root.join("xdg-cache"))
        .env("HOME", &h.home)
        .env("CLAUDE_CONFIG_DIR", h.root.join("claude-config"))
        .env("CLAUDIO_CLAUDE_PATH", &h.fake_claude)
        .env("CLAUDIO_NO_UPDATE_CHECK", "1")
        .env("CLAUDIO_NO_BROWSER", "1")
        .env("ANTHROPIC_API_KEY", "test-key-not-real")
        .env("TERM", "xterm-256color")
        .env("SHELL", "/bin/sh")
        .env_remove("COLORTERM")
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        .env_remove("ANTHROPIC_BASE_URL");
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// A running `claudio web` on a free loopback port, in a harness's dirs.
struct Web {
    h: Option<ManagerHarness>,
    child: Child,
    port: u16,
    token: String,
}

impl Web {
    fn start() -> Web {
        Web::start_with(ManagerHarness::new(), &[], |_| {})
    }

    fn start_with(h: ManagerHarness, args: &[&str], setup: impl FnOnce(&mut Command)) -> Web {
        let port = free_port();
        let mut cmd = Command::new(BINARY);
        cmd.arg("web")
            .arg("--listen")
            .arg(format!("127.0.0.1:{port}"))
            .arg("--no-open")
            .args(args);
        apply_env(&h, &mut cmd);
        setup(&mut cmd);
        let log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(h.root.join("web.log"))
            .unwrap();
        cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(log);
        let mut child = cmd.spawn().expect("start claudio web");
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let _ = tx.send(line);
            }
        });
        let url = loop {
            let line = rx.recv_timeout(DAEMON_WAIT).expect("claudio web printed no URL");
            if let Some(url) = line.strip_prefix("claudio web: ") {
                break url.to_owned();
            }
        };
        let token = fs::read_to_string(h.config_home.join("claudio/web-token"))
            .unwrap()
            .trim()
            .to_owned();
        assert_eq!(url, format!("http://127.0.0.1:{port}/#t={token}"));
        Web { h: Some(h), child, port, token }
    }

    fn h(&self) -> &ManagerHarness {
        self.h.as_ref().unwrap()
    }

    /// Stop the server; the harness (and the daemon) stay.
    fn stop(mut self) -> ManagerHarness {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.h.take().unwrap()
    }

    fn host(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }

    fn origin(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn get(&self, path: &str, extra: &[(&str, &str)]) -> Reply {
        http(self.port, "GET", path, &self.host(), extra, "")
    }

    fn post(&self, path: &str, origin: Option<&str>, body: &str, extra: &[(&str, &str)]) -> Reply {
        let mut headers: Vec<(&str, &str)> = extra.to_vec();
        if let Some(origin) = origin {
            headers.push(("Origin", origin));
        }
        http(self.port, "POST", path, &self.host(), &headers, body)
    }

    /// Log in with the right token; the cookie's value.
    fn login(&self) -> String {
        let body = json!({"token": self.token}).to_string();
        let reply = self.post("/api/login", Some(&self.origin()), &body, &[]);
        assert_eq!(reply.status, 204, "{reply:?}");
        let cookie = reply.header("set-cookie").expect("a cookie").to_owned();
        for flag in ["HttpOnly", "SameSite=Strict", "Path=/"] {
            assert!(cookie.contains(flag), "{cookie}");
        }
        assert!(!cookie.contains("Secure"), "{cookie}");
        let value = cookie
            .strip_prefix("claudio_web=")
            .and_then(|rest| rest.split(';').next())
            .expect("claudio_web=…")
            .to_owned();
        assert_eq!(value.len(), 64);
        value
    }

    async fn ws_with(&self, cookie: &str, origin: &str) -> Result<Ws, u16> {
        let mut req = format!("ws://127.0.0.1:{}/ws", self.port)
            .into_client_request()
            .unwrap();
        req.headers_mut().insert("Origin", origin.parse().unwrap());
        if !cookie.is_empty() {
            let value = format!("claudio_web={cookie}");
            req.headers_mut().insert("Cookie", value.parse().unwrap());
        }
        match tokio_tungstenite::connect_async(req).await {
            Ok((ws, _)) => Ok(ws),
            Err(tungstenite::Error::Http(resp)) => Err(resp.status().as_u16()),
            Err(e) => panic!("ws: {e}"),
        }
    }
}

impl Drop for Web {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Debug)]
struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// One raw HTTP/1.1 request, so a test controls `Host` and `Origin`.
fn http(port: u16, method: &str, path: &str, host: &str, extra: &[(&str, &str)], body: &str) -> Reply {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(WAIT)).unwrap();
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    if !body.is_empty() {
        req.push_str("Content-Type: application/json\r\n");
    }
    for (k, v) in extra {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(body);
    s.write_all(req.as_bytes()).unwrap();
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let mut lines = head.lines();
    let status = lines
        .next()
        .and_then(|l| l.split(' ').nth(1))
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect();
    Reply { status, headers, body: body.to_owned() }
}

/// One browser tab's WebSocket: requests, events and session output, every
/// frame recorded in `seen`.
struct Tab {
    ws: Ws,
    next: u32,
    pending: VecDeque<Message>,
    hello: Value,
    snapshot: Value,
    seen: Vec<Vec<u8>>,
}

impl Tab {
    async fn open(web: &Web, cookie: &str) -> Tab {
        let ws = web.ws_with(cookie, &web.origin()).await.expect("ws");
        let mut tab = Tab {
            ws,
            next: 0,
            pending: VecDeque::new(),
            hello: Value::Null,
            snapshot: Value::Null,
            seen: Vec::new(),
        };
        tab.hello = tab.text_where(|v| v["t"] == "hello").await;
        tab.snapshot = tab.text_where(|v| v["t"] == "snapshot").await;
        tab
    }

    async fn next_message(&mut self) -> Message {
        let msg = tokio::time::timeout(WAIT, self.ws.next())
            .await
            .expect("no frame in time")
            .expect("socket closed")
            .expect("ws error");
        self.seen.push(msg.clone().into_data().to_vec());
        msg
    }

    /// The first text frame (buffered or new) matching `pick`; the others
    /// stay buffered in order.
    async fn text_where(&mut self, pick: impl Fn(&Value) -> bool) -> Value {
        let found = self
            .pending
            .iter()
            .position(|m| text_json(m).is_some_and(|v| pick(&v)));
        if let Some(i) = found {
            return text_json(&self.pending.remove(i).unwrap()).unwrap();
        }
        loop {
            let msg = self.next_message().await;
            match text_json(&msg) {
                Some(v) if pick(&v) => return v,
                _ => self.pending.push_back(msg),
            }
        }
    }

    async fn req(&mut self, op: &str, args: Value) -> Result<Value, String> {
        self.next += 1;
        let id = self.next;
        let mut frame = if args.is_object() { args } else { json!({}) };
        frame["t"] = "req".into();
        frame["id"] = id.into();
        frame["op"] = op.into();
        self.ws.send(Message::text(frame.to_string())).await.unwrap();
        let res = self.text_where(|v| v["t"] == "res" && v["id"] == id).await;
        if res["ok"] == true {
            Ok(res["data"].clone())
        } else {
            Err(res["error"].as_str().unwrap_or_default().to_owned())
        }
    }

    async fn event(&mut self, ev: &str, pick: impl Fn(&Value) -> bool) -> Value {
        self.text_where(|v| v["t"] == "ev" && v["ev"] == ev && pick(v)).await
    }

    /// Session `sid`'s output (0x01 frames, buffered ones first) until it
    /// contains `needle`.
    async fn output_until(&mut self, sid: &str, needle: &str) -> String {
        let id = uuid_bytes(sid);
        let mut text = String::new();
        let mut keep = VecDeque::new();
        while let Some(m) = self.pending.pop_front() {
            match output_of(&m, &id) {
                Some(b) => text.push_str(&String::from_utf8_lossy(b)),
                None => keep.push_back(m),
            }
        }
        self.pending = keep;
        while !text.contains(needle) {
            let msg = self.next_message().await;
            match output_of(&msg, &id) {
                Some(b) => text.push_str(&String::from_utf8_lossy(b)),
                None => self.pending.push_back(msg),
            }
        }
        text
    }

    async fn input(&mut self, sid: &str, bytes: &[u8]) {
        let mut frame = vec![0x02];
        frame.extend(uuid_bytes(sid));
        frame.extend_from_slice(bytes);
        self.ws.send(Message::binary(frame)).await.unwrap();
    }

    /// The close code the server ends the socket with.
    async fn close_code(&mut self) -> Option<u16> {
        loop {
            match tokio::time::timeout(WAIT, self.ws.next()).await.ok()?? {
                Ok(Message::Close(frame)) => return frame.map(|f| u16::from(f.code)),
                Ok(_) => continue,
                Err(_) => return None,
            }
        }
    }
}

fn text_json(m: &Message) -> Option<Value> {
    match m {
        Message::Text(t) => serde_json::from_str(t.as_str()).ok(),
        _ => None,
    }
}

fn output_of<'a>(m: &'a Message, id: &[u8; 16]) -> Option<&'a [u8]> {
    match m {
        Message::Binary(b) if b.len() >= 17 && b[0] == 0x01 && &b[1..17] == id => Some(&b[17..]),
        _ => None,
    }
}

fn uuid_bytes(sid: &str) -> [u8; 16] {
    *uuid::Uuid::parse_str(sid).unwrap().as_bytes()
}

// ── AC1: loopback only ────────────────────────────────────────────────────────

#[test]
fn web_refuses_a_non_loopback_listen() {
    let h = ManagerHarness::new();
    let mut cmd = Command::new(BINARY);
    cmd.args(["web", "--listen", "0.0.0.0:7337", "--no-open"]);
    apply_env(&h, &mut cmd);
    let out = cmd.output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("ssh -L 7337:127.0.0.1:7337"), "{err}");
    assert!(err.contains("--allow-host"), "{err}");
    assert!(!h.config_home.join("claudio/web-token").exists(), "no token before the address is accepted");
}

// ── AC2: authentication and request checks ───────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_auth_gates_api_and_ws() {
    let web = Web::start();
    // No cookie.
    assert_eq!(web.get("/api/x", &[]).status, 401);
    assert_eq!(web.get("/api/me", &[]).status, 401);
    assert_eq!(web.ws_with("", &web.origin()).await.err(), Some(401));
    // DNS rebinding: a foreign Host.
    let rebound = http(web.port, "GET", "/", &format!("evil.example:{}", web.port), &[], "");
    assert_eq!(rebound.status, 421);
    // The page needs no cookie and carries the security headers.
    let page = web.get("/", &[]);
    assert_eq!(page.status, 200);
    let csp = page.header("content-security-policy").expect("a CSP");
    for part in [
        "default-src 'self'",
        "script-src 'self'",
        "object-src 'none'",
        "base-uri 'none'",
        "frame-ancestors 'none'",
        "connect-src 'self'",
    ] {
        assert!(csp.contains(part), "{csp}");
    }
    assert!(!csp.contains("unsafe-eval"), "{csp}");
    assert_eq!(page.header("x-content-type-options"), Some("nosniff"));
    assert_eq!(page.header("referrer-policy"), Some("no-referrer"));
    assert_eq!(page.header("cache-control"), Some("no-store"));
    assert!(rebound.header("content-security-policy").is_some(), "errors carry the headers too");
    // A cross-site or origin-less login is refused before the token is read.
    let body = json!({"token": web.token}).to_string();
    assert_eq!(web.post("/api/login", Some("http://evil.example"), &body, &[]).status, 403);
    assert_eq!(web.post("/api/login", None, &body, &[]).status, 403);
    // The right token from the page's origin: a cookie (flags checked in login()).
    let cookie = web.login();
    // A WebSocket from another origin is refused even with the cookie.
    assert_eq!(web.ws_with(&cookie, "http://evil.example").await.err(), Some(403));
    // From the page's origin: hello, then the snapshot.
    let mut tab = Tab::open(&web, &cookie).await;
    assert_eq!(tab.hello["v"], 1);
    assert_eq!(tab.hello["version"], env!("CARGO_PKG_VERSION"));
    assert!(tab.snapshot["sessions"].is_array() && tab.snapshot["hosts"].is_array());
    assert_eq!(tab.req("no.such.op", json!({})).await, Err("unknown op".to_owned()));
    // Authenticated API paths pass the gate.
    let with_cookie = format!("claudio_web={cookie}");
    assert_eq!(web.get("/api/x", &[("Cookie", &with_cookie)]).status, 404);
    assert_eq!(web.get("/api/me", &[("Cookie", &with_cookie)]).status, 204);
    // Logout ends the session.
    let out = web.post("/api/logout", Some(&web.origin()), "", &[("Cookie", &with_cookie)]);
    assert_eq!(out.status, 204);
    assert_eq!(web.ws_with(&cookie, &web.origin()).await.err(), Some(401));
}

#[test]
fn web_login_is_rate_limited() {
    let web = Web::start();
    let wrong = json!({"token": "0".repeat(64)}).to_string();
    for i in 0..5 {
        assert_eq!(web.post("/api/login", Some(&web.origin()), &wrong, &[]).status, 401, "attempt {i}");
    }
    assert_eq!(web.post("/api/login", Some(&web.origin()), &wrong, &[]).status, 429);
    let right = json!({"token": web.token}).to_string();
    assert_eq!(
        web.post("/api/login", Some(&web.origin()), &right, &[]).status,
        429,
        "locked for the rest of the minute, right token or not"
    );
}

#[test]
fn web_token_is_private_reused_and_rotated() {
    let web = Web::start();
    let path = web.h().config_home.join("claudio/web-token");
    assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    let first = web.token.clone();
    let h = web.stop();
    let mut cmd = Command::new(BINARY);
    cmd.args(["web", "--print-url"]);
    apply_env(&h, &mut cmd);
    let out = cmd.output().unwrap();
    assert!(out.status.success());
    let printed = String::from_utf8_lossy(&out.stdout);
    assert!(printed.trim().ends_with(&format!("#t={first}")), "{printed}");
    let web = Web::start_with(h, &[], |_| {});
    assert_eq!(web.token, first, "reused across restarts");
    let h = web.stop();
    let web = Web::start_with(h, &["--rotate-token"], |_| {});
    assert_ne!(web.token, first);
    assert!(web.token.len() == 64 && web.token.bytes().all(|b| b.is_ascii_hexdigit()));
}

#[test]
fn web_allow_host_accepts_the_proxy_name_and_secures_the_cookie() {
    let web = Web::start_with(ManagerHarness::new(), &["--allow-host", "Claudio.Example"], |_| {});
    assert_eq!(http(web.port, "GET", "/", "claudio.example", &[], "").status, 200);
    let with_port = format!("CLAUDIO.EXAMPLE:{}", web.port);
    assert_eq!(http(web.port, "GET", "/", &with_port, &[], "").status, 200);
    assert_eq!(http(web.port, "GET", "/", "other.example", &[], "").status, 421);
    let body = json!({"token": web.token}).to_string();
    let reply = http(
        web.port,
        "POST",
        "/api/login",
        "claudio.example",
        &[("Origin", "https://claudio.example")],
        &body,
    );
    assert_eq!(reply.status, 204);
    assert!(reply.header("set-cookie").unwrap().contains("; Secure"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_bad_frames_close_the_socket() {
    let web = Web::start();
    let cookie = web.login();
    let mut tab = Tab::open(&web, &cookie).await;
    tab.ws.send(Message::text("x".repeat(64 * 1024 + 1))).await.unwrap();
    assert_eq!(tab.close_code().await, Some(1009));
    let mut tab = Tab::open(&web, &cookie).await;
    tab.ws.send(Message::text("not json")).await.unwrap();
    assert_eq!(tab.close_code().await, Some(1003));
    let mut tab = Tab::open(&web, &cookie).await;
    tab.ws.send(Message::binary(vec![0x02; 16])).await.unwrap();
    assert_eq!(tab.close_code().await, Some(1003));
    let mut tab = Tab::open(&web, &cookie).await;
    tab.ws.send(Message::binary(vec![0x02; 17 + 64 * 1024 + 1])).await.unwrap();
    assert_eq!(tab.close_code().await, Some(1009));
}
```
(Adapt `Message::text`/`Message::binary`/`into_data`/`Utf8Bytes::as_str` to the tungstenite version Cargo resolves; keep the assertions.)

Unit tests in `src/web/mod.rs` (AC1):
```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn listen_must_be_loopback() {
        assert_eq!(listen_addr("127.0.0.1:7337").unwrap().to_string(), "127.0.0.1:7337");
        assert_eq!(listen_addr("127.1.2.3:80").unwrap().to_string(), "127.1.2.3:80");
        assert_eq!(listen_addr("[::1]:9000").unwrap().to_string(), "[::1]:9000");
        assert_eq!(listen_addr("localhost:8080").unwrap().to_string(), "127.0.0.1:8080");
        assert_eq!(listen_addr("localhost").unwrap().to_string(), "127.0.0.1:7337");
        assert_eq!(listen_addr("127.0.0.1").unwrap().to_string(), "127.0.0.1:7337");
        for bad in ["0.0.0.0:7337", "192.168.1.2:7337", "[::]:7337", "10.0.0.1"] {
            let err = listen_addr(bad).unwrap_err();
            assert!(err.contains("ssh -L 7337:127.0.0.1:7337") && err.contains("--allow-host"), "{err}");
        }
        assert!(listen_addr("example.org:7337").is_err());
    }

    #[test]
    fn args_parse_flags_and_defaults() {
        let a = parse_args(&[]).unwrap();
        assert_eq!(a.listen.to_string(), "127.0.0.1:7337");
        assert!(a.allow_hosts.is_empty() && !a.no_open && !a.print_url && !a.rotate_token);
        let a = parse_args(&args(&[
            "--allow-host", "Claudio.Example", "--no-open", "--print-url", "--rotate-token",
            "--listen=127.0.0.1:9000", "--allow-host=b.example",
        ]))
        .unwrap();
        assert_eq!(a.listen.to_string(), "127.0.0.1:9000");
        assert_eq!(a.allow_hosts, vec!["claudio.example", "b.example"]);
        assert!(a.no_open && a.print_url && a.rotate_token);
        assert!(parse_args(&args(&["--allow-host", "bad/host"])).is_err());
        assert!(parse_args(&args(&["--allow-host"])).is_err());
        assert!(parse_args(&args(&["--bogus"])).is_err());
    }
}
```
`src/web/auth.rs` unit tests (in full, below with the code). `src/web/assets.rs` unit tests: `every_asset_has_a_mime_an_etag_and_index_exists`, `the_import_map_hash_matches_the_embedded_index` (when `IMPORTMAP_SHA256` is `Some`, it equals `importmap_sha256` of the decompressed `/index.html`), `mime_by_extension` (`.html`→`text/html; charset=utf-8`, `.js`/`.mjs`→`text/javascript; charset=utf-8`, `.css`→`text/css; charset=utf-8`, `.woff2`→`font/woff2`, `.svg`→`image/svg+xml`, `.json`→`application/json`, unknown→`application/octet-stream`), `disk_paths_cannot_escape` (`disk_path(root, "/../x")`, `"/a/../../x"`, `"/./x"`, `"//etc/passwd"` → `None`; `"/app/boot.js"` → `Some(root/app/boot.js)`), `gzip_is_accepted_only_when_asked` (`accepts_gzip("gzip, deflate")`, `"br;q=1, gzip;q=0.5"` true; `""`, `"identity"`, `"gzip;q=0"` false). `src/cli.rs`: extend `help_appendix_documents_the_flags` (the test at :620) with `assert!(HELP_APPENDIX.contains("claudio web"));` and `assert!(HELP_APPENDIX.contains("CLAUDIO_WEB_DIR"));`.

Run: `cargo test --locked --test web -- --test-threads=1`
Expected: build failure (`claudio web` does not exist: the tests fail with "claudio web printed no URL" or do not compile until `tokio-tungstenite` is a dev-dependency).

Step 2: implement.

`build.rs` (new; embeds `web/` gzipped; a stub index when `web/index.html` is missing, so the backend lane builds without the frontend lane):
```rust
//! Embeds the browser app (`web/`) into the binary: every file gzipped
//! (level 9, mtime 0, so builds are reproducible) and listed in
//! `$OUT_DIR/web_assets.rs`, which `src/web/assets.rs` includes. Dev-only
//! directories (tests, e2e, dev, node_modules), dotfiles and npm files are
//! left out. Without `web/index.html` a stub page is embedded.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use base64::Engine;
use flate2::{Compression, GzBuilder};
use sha2::{Digest, Sha256};

const SKIP_DIRS: &[&str] = &["tests", "e2e", "dev", "node_modules"];
const SKIP_FILES: &[&str] = &["package.json", "package-lock.json"];
const STUB_INDEX: &str = "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
<title>claudio</title></head><body><p>claudio web: this build has no browser app \
(web/index.html was missing at build time).</p></body></html>\n";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=web");
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let gz_dir = out.join("web");
    let _ = fs::remove_dir_all(&gz_dir);
    fs::create_dir_all(&gz_dir).expect("create OUT_DIR/web");

    let web = Path::new("web");
    let mut files = Vec::new();
    if web.is_dir() {
        walk(web, web, &mut files);
    }
    files.sort();
    let mut inputs: Vec<(String, Vec<u8>)> = files
        .into_iter()
        .map(|(rel, path)| {
            let bytes = fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            (rel, bytes)
        })
        .collect();
    if !inputs.iter().any(|(rel, _)| rel == "index.html") {
        inputs.push(("index.html".to_owned(), STUB_INDEX.as_bytes().to_vec()));
    }

    let mut table = String::from("pub(crate) static ASSETS: &[Asset] = &[\n");
    let mut importmap = None;
    for (n, (rel, bytes)) in inputs.iter().enumerate() {
        if rel == "index.html" {
            importmap = importmap_sha256(bytes);
        }
        let gz_path = gz_dir.join(format!("{n}.gz"));
        let mut gz = GzBuilder::new()
            .mtime(0)
            .write(Vec::new(), Compression::best());
        gz.write_all(bytes).expect("gzip");
        fs::write(&gz_path, gz.finish().expect("gzip")).expect("write gz");
        let digest = Sha256::digest(bytes);
        let etag: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
        table.push_str(&format!(
            "    Asset {{ path: {:?}, mime: {:?}, etag: \"\\\"{etag}\\\"\", gz: include_bytes!({:?}) }},\n",
            format!("/{rel}"),
            mime(rel),
            gz_path.display().to_string(),
        ));
    }
    table.push_str("];\n");
    table.push_str(&format!(
        "pub(crate) const IMPORTMAP_SHA256: Option<&str> = {:?};\n",
        importmap
    ));
    fs::write(out.join("web_assets.rs"), table).expect("write web_assets.rs");
}

/// Every file under `dir`, as (path relative to `root` with `/`, path).
fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .map(|e| e.expect("dir entry").path())
        .collect();
    entries.sort();
    for path in entries {
        let name = path.file_name().and_then(|n| n.to_str()).expect("UTF-8 file names under web/");
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            let top_level_dev = dir == root && SKIP_DIRS.contains(&name);
            if !top_level_dev && name != "node_modules" {
                walk(root, &path, out);
            }
        } else if !SKIP_FILES.contains(&name) {
            let rel = path.strip_prefix(root).unwrap().to_str().expect("UTF-8 path");
            out.push((rel.replace('\\', "/"), path.clone()));
        }
    }
}

/// The CSP hash of `index.html`'s `<script type="importmap">` text.
/// Keep in step with `src/web/assets.rs`'s copy.
fn importmap_sha256(html: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(html).ok()?;
    let open = "<script type=\"importmap\">";
    let start = text.find(open)? + open.len();
    let end = start + text[start..].find("</script>")?;
    Some(base64::engine::general_purpose::STANDARD.encode(Sha256::digest(text[start..end].as_bytes())))
}

fn mime(rel: &str) -> &'static str {
    match rel.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "txt" | "md" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}
```

`src/main.rs`: `mod web;` (alphabetical, after `upgrade`), and right after the `sessions` dispatch:
```rust
    // `claudio web [...]` — the session manager in a browser (src/web/).
    if argv.get(1).map(String::as_str) == Some("web") {
        return web::run(&argv[2..]);
    }
```

`src/cli.rs` `HELP_APPENDIX`: a "Web UI" block before "Daemon commands", and the `CLAUDIO_NO_BROWSER` line extended:
```text
 Web UI:
   claudio web [--listen 127.0.0.1:7337] [--allow-host NAME]... [--no-open] [--print-url] [--rotate-token]
                               Serve the session manager to a browser on this machine (loopback only).
                               Prints http://127.0.0.1:7337/#t=<token> and opens it. The token (in
                               ~/.config/claudio/web-token, 0600) is as sensitive as an SSH key: it
                               reaches every session on every connected host. From another machine:
                               ssh -L 7337:127.0.0.1:7337 HOST. Behind a TLS reverse proxy on this
                               host: --allow-host NAME. --print-url prints the URL and exits;
                               --rotate-token makes a new token. See docs/web.md.
   CLAUDIO_WEB_DIR=<dir>            (debug builds only) serve the browser app from <dir>, not the binary.
```
and `CLAUDIO_NO_BROWSER=1   claudio proxy login and claudio web print their URL without opening a browser.`

`src/web/mod.rs` (new). In full where it decides (args), shape elsewhere:
```rust
//! `claudio web`: the session manager served to a browser. One more client
//! of the claudio daemons (local socket, remote ssh bridge), like the TUI.
//! See docs/specs/2026-11-08-web-ui-design.md and docs/web-protocol.md.

pub(crate) mod assets;
pub(crate) mod auth;
mod ws;

use std::io::{self, IsTerminal, Write};
use std::net::{IpAddr, SocketAddr};
use std::process::ExitCode;
use std::sync::Arc;

use axum::extract::FromRef;
use axum::routing::{get, post};
use axum::{middleware, Router};

pub(crate) const DEFAULT_PORT: u16 = 7337;

const USAGE: &str = "usage: claudio web [--listen 127.0.0.1:7337] [--allow-host NAME]... \
[--no-open] [--print-url] [--rotate-token]";

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WebArgs {
    pub listen: SocketAddr,
    /// Lowercase host names a reverse proxy on this host may use.
    pub allow_hosts: Vec<String>,
    pub no_open: bool,
    pub print_url: bool,
    pub rotate_token: bool,
}

/// Shared by every handler.
#[derive(Clone)]
pub(crate) struct AppState {
    pub auth: Arc<auth::Auth>,
    pub assets: Arc<assets::Assets>,
    // Task 4 adds `hub: hub::HubHandle`; Task 5 adds `ctx: Arc<spawn::SpawnCtx>`.
}

impl FromRef<AppState> for Arc<auth::Auth> {
    fn from_ref(state: &AppState) -> Self {
        Arc::clone(&state.auth)
    }
}

pub(crate) fn parse_args(args: &[String]) -> Result<WebArgs, String> {
    let mut out = WebArgs {
        listen: SocketAddr::from(([127, 0, 0, 1], DEFAULT_PORT)),
        allow_hosts: Vec::new(),
        no_open: false,
        print_url: false,
        rotate_token: false,
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value.to_owned())),
            _ => (arg.as_str(), None),
        };
        match flag {
            "--listen" => out.listen = listen_addr(&value(inline, &mut it, flag)?)?,
            "--allow-host" => {
                let name = value(inline, &mut it, flag)?;
                if !host_name_ok(&name) {
                    return Err(format!("--allow-host {name:?}: not a host name"));
                }
                out.allow_hosts.push(name.to_ascii_lowercase());
            }
            "--no-open" => out.no_open = true,
            "--print-url" => out.print_url = true,
            "--rotate-token" => out.rotate_token = true,
            other => return Err(format!("unknown argument {other:?}\n{USAGE}")),
        }
    }
    Ok(out)
}

fn value(inline: Option<String>, it: &mut std::slice::Iter<'_, String>, flag: &str) -> Result<String, String> {
    inline
        .or_else(|| it.next().cloned())
        .ok_or_else(|| format!("{flag} needs a value"))
}

/// `[A-Za-z0-9.-]{1,253}`, not starting with `-`.
fn host_name_ok(name: &str) -> bool {
    (1..=253).contains(&name.len())
        && !name.starts_with('-')
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
}

/// `addr` as a socket address, if it is a loopback one: `127.0.0.0/8`,
/// `::1`, or `localhost` (127.0.0.1). Without a port: 7337.
pub(crate) fn listen_addr(addr: &str) -> Result<SocketAddr, String> {
    let localhost = |port: u16| SocketAddr::from(([127, 0, 0, 1], port));
    let parsed = if addr == "localhost" {
        Some(localhost(DEFAULT_PORT))
    } else if let Some(port) = addr.strip_prefix("localhost:") {
        port.parse().ok().map(localhost)
    } else {
        addr.parse::<SocketAddr>()
            .ok()
            .or_else(|| addr.parse::<IpAddr>().ok().map(|ip| SocketAddr::new(ip, DEFAULT_PORT)))
    };
    match parsed {
        Some(sock) if sock.ip().is_loopback() => Ok(sock),
        Some(_) => Err(format!(
            "refusing to listen on {addr}: claudio web listens on loopback only \
             (127.0.0.1, ::1, localhost).\n\
             From another machine, tunnel it: ssh -L 7337:127.0.0.1:7337 <this host>\n\
             Behind a reverse proxy on this host (e.g. tailscale serve), add --allow-host <its name>."
        )),
        None => Err(format!("--listen {addr:?}: not an address (example: 127.0.0.1:7337)")),
    }
}

/// `claudio web …` (`args` are the words after `web`).
pub fn run(args: &[String]) -> ExitCode { … }
```
`run` rules: `-h`/`--help` anywhere → print `USAGE`, exit 0. `parse_args` error → `claudio web: <error>` on stderr, exit **2**, nothing else done (no token file). Then `auth::load_or_create_token(&auth::token_path(), rotate_token)` (error → exit 1). `url = format!("http://{listen}/#t={token}")` (`SocketAddr`'s `Display` brackets IPv6). `--print-url` → print the URL alone on stdout, exit 0. Otherwise: tracing to stderr (`tracing_subscriber::fmt().with_writer(std::io::stderr).with_env_filter(…)`, default `claudio=info`, `CLAUDIO_DEBUG=1` → `claudio=debug`), a multi-thread tokio runtime, then `serve`:
1. bind `tokio::net::TcpListener` on `listen` (error → `cannot listen on <addr>: <why>`, exit 1);
2. `Assets::load()`; `auth::Auth::new(token, auth::Allowed::new(listen.port(), &allow_hosts), auth::csp(assets.importmap_sha256()))`;
3. print exactly `claudio web: <url>` on stdout and flush (the tests read this line; nothing else goes to stdout);
4. open the browser when `!no_open && proxy::login::browser_wanted(std::env::consts::OS, |k| std::env::var(k).ok(), io::stdout().is_terminal())` with `proxy::login::open_browser(&url)`;
5. `axum::serve(listener, router(state)).with_graceful_shutdown(shutdown())`, `shutdown()` = Ctrl+C or SIGTERM (`tokio::signal::unix`). Task 4 makes the hub close every tab (1001) on shutdown.

```rust
pub(crate) fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/login", post(auth::login))
        .route("/api/logout", post(auth::logout))
        .route("/api/me", get(auth::me))
        .route("/ws", get(ws::upgrade))
        .fallback(assets::serve)
        .layer(middleware::from_fn_with_state(Arc::clone(&state.auth), auth::guard))
        .with_state(state)
}
```

`src/web/auth.rs` (new), in full:
```rust
//! Authentication and request checks for `claudio web`.
//!
//! A secret token (32 random bytes, hex) lives in `<config>/web-token`
//! (0600). The page gets it in the URL fragment and trades it at
//! `POST /api/login` for a cookie naming a server-side session (in memory,
//! 24 h idle expiry, at most 64). [`guard`] checks every request: its
//! `Host` must be one this server answers to (DNS rebinding); anything but
//! `GET`/`HEAD`, and every WebSocket upgrade, must come from an accepted
//! `Origin` (CSRF, cross-site WebSocket hijacking); `/api/*` (but login)
//! and `/ws` need the cookie. Every response, errors included, gets the CSP
//! and the other security headers. Holding the cookie is holding a shell on
//! every connected host.

use std::collections::VecDeque;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::paths;

pub(crate) const COOKIE: &str = "claudio_web";
pub(crate) const TOKEN_BYTES: usize = 32;
pub(crate) const SESSION_IDLE: Duration = Duration::from_secs(24 * 60 * 60);
pub(crate) const MAX_SESSIONS: usize = 64;
pub(crate) const MAX_FAILURES: usize = 5;
pub(crate) const FAILURE_WINDOW: Duration = Duration::from_secs(60);
pub(crate) const LOGIN_BODY_MAX: usize = 4096;

/// `<config>/web-token`.
pub(crate) fn token_path() -> PathBuf {
    paths::config_dir().join("web-token")
}

/// `n` random bytes from the kernel, as lowercase hex.
pub(crate) fn random_hex(n: usize) -> io::Result<String> {
    let mut bytes = vec![0u8; n];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Whether `s` is a token this server could have written: 64 lowercase hex.
pub(crate) fn token_ok(s: &str) -> bool {
    s.len() == 2 * TOKEN_BYTES && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The saved token, or a new one written 0600 when there is none, it is not
/// a regular private file of ours holding a well-formed token, or `rotate`.
/// Blocking I/O: call before the runtime starts (or in `spawn_blocking`).
pub(crate) fn load_or_create_token(path: &Path, rotate: bool) -> io::Result<String> {
    use std::os::unix::fs::MetadataExt;
    if !rotate {
        if let Ok(meta) = std::fs::symlink_metadata(path) {
            let private = meta.file_type().is_file()
                && meta.mode() & 0o077 == 0
                && meta.uid() == paths::uid();
            if private {
                let text = std::fs::read_to_string(path)?;
                let token = text.trim();
                if token_ok(token) {
                    return Ok(token.to_owned());
                }
            }
        }
    }
    let token = random_hex(TOKEN_BYTES)?;
    paths::write_atomic(path, format!("{token}\n").as_bytes())?;
    Ok(token)
}

/// `a == b`, in time that depends only on the lengths.
pub(crate) fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let diff = a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y));
    std::hint::black_box(diff) == 0
}

/// The `Host` and `Origin` values this server answers to.
#[derive(Debug, Clone)]
pub(crate) struct Allowed {
    /// Every accepted `host[:port]`, lowercase.
    hosts: Vec<String>,
    /// Any `--allow-host` given: cookies then get `Secure`.
    proxied: bool,
}

impl Allowed {
    /// The three loopback forms at `port` (also bare when the port is 80),
    /// and each of `allow_hosts` (lowercase) bare or at `port`.
    pub(crate) fn new(port: u16, allow_hosts: &[String]) -> Allowed {
        let mut hosts = Vec::new();
        for name in ["127.0.0.1", "localhost", "[::1]"] {
            hosts.push(format!("{name}:{port}"));
            if port == 80 {
                hosts.push(name.to_owned());
            }
        }
        for name in allow_hosts {
            let name = name.trim().to_ascii_lowercase();
            hosts.push(format!("{name}:{port}"));
            hosts.push(name);
        }
        Allowed {
            hosts,
            proxied: !allow_hosts.is_empty(),
        }
    }

    pub(crate) fn host_ok(&self, host: &str) -> bool {
        let host = host.trim().to_ascii_lowercase();
        !host.is_empty() && self.hosts.iter().any(|h| *h == host)
    }

    /// `http://` or `https://` and an accepted host, nothing after it.
    /// `null` and every other scheme are refused.
    pub(crate) fn origin_ok(&self, origin: &str) -> bool {
        let origin = origin.trim().to_ascii_lowercase();
        let rest = origin
            .strip_prefix("http://")
            .or_else(|| origin.strip_prefix("https://"));
        match rest {
            Some(host) if !host.contains('/') => self.host_ok(host),
            _ => false,
        }
    }

    pub(crate) fn secure_cookie(&self) -> bool {
        self.proxied
    }
}

/// The CSP: `'self'` only, plus the hash of the one inline script allowed,
/// `index.html`'s import map.
pub(crate) fn csp(importmap_sha256: Option<&str>) -> String {
    let script = match importmap_sha256 {
        Some(hash) => format!("script-src 'self' 'sha256-{hash}'"),
        None => "script-src 'self'".to_owned(),
    };
    format!(
        "default-src 'self'; {script}; style-src 'self' 'unsafe-inline'; \
         img-src 'self' data: blob:; font-src 'self'; connect-src 'self'; \
         object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'"
    )
}

#[derive(Default)]
struct Inner {
    /// Cookie values and their last use.
    sessions: Vec<(String, Instant)>,
    /// When the recent failed logins happened, oldest first.
    failures: VecDeque<Instant>,
}

pub(crate) struct Auth {
    token: String,
    allowed: Allowed,
    csp: HeaderValue,
    inner: Mutex<Inner>,
}

impl Auth {
    pub(crate) fn new(token: String, allowed: Allowed, csp: String) -> Auth {
        Auth {
            token,
            allowed,
            csp: HeaderValue::from_str(&csp).expect("an ASCII CSP"),
            inner: Mutex::new(Inner::default()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn allowed(&self) -> &Allowed {
        &self.allowed
    }

    /// Check a login: `Ok` for the right token; `429` while
    /// [`MAX_FAILURES`] failures are under [`FAILURE_WINDOW`] old (the
    /// token is not even compared then); else `401`, which counts.
    pub(crate) fn login(&self, token: &str, now: Instant) -> Result<(), StatusCode> {
        let mut inner = self.lock();
        while inner
            .failures
            .front()
            .is_some_and(|t| now.duration_since(*t) >= FAILURE_WINDOW)
        {
            inner.failures.pop_front();
        }
        if inner.failures.len() >= MAX_FAILURES {
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
        if ct_eq(token.as_bytes(), self.token.as_bytes()) {
            Ok(())
        } else {
            inner.failures.push_back(now);
            Err(StatusCode::UNAUTHORIZED)
        }
    }

    /// A new session's cookie value. Idle ones are dropped first; past
    /// [`MAX_SESSIONS`], the least recently used goes.
    pub(crate) fn create_session(&self, now: Instant) -> io::Result<String> {
        let value = random_hex(TOKEN_BYTES)?;
        let mut inner = self.lock();
        inner
            .sessions
            .retain(|(_, seen)| now.duration_since(*seen) < SESSION_IDLE);
        if inner.sessions.len() >= MAX_SESSIONS {
            let oldest = inner
                .sessions
                .iter()
                .enumerate()
                .min_by_key(|(_, (_, seen))| *seen)
                .map(|(i, _)| i);
            if let Some(i) = oldest {
                inner.sessions.remove(i);
            }
        }
        inner.sessions.push((value.clone(), now));
        Ok(value)
    }

    /// Whether `value` names a live session (every one is compared in
    /// constant time); refreshes its last use.
    pub(crate) fn session_ok(&self, value: &str, now: Instant) -> bool {
        let mut inner = self.lock();
        inner
            .sessions
            .retain(|(_, seen)| now.duration_since(*seen) < SESSION_IDLE);
        let mut found = None;
        for (i, (v, _)) in inner.sessions.iter().enumerate() {
            if ct_eq(v.as_bytes(), value.as_bytes()) {
                found = Some(i);
            }
        }
        match found {
            Some(i) => {
                inner.sessions[i].1 = now;
                true
            }
            None => false,
        }
    }

    pub(crate) fn end_session(&self, value: &str) {
        self.lock()
            .sessions
            .retain(|(v, _)| !ct_eq(v.as_bytes(), value.as_bytes()));
    }
}

/// Our cookie's value in `headers`, if any.
pub(crate) fn cookie_value(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == COOKIE)
        .map(|(_, value)| value.trim().to_owned())
}

fn set_cookie(value: &str, secure: bool) -> HeaderValue {
    let secure = if secure { "; Secure" } else { "" };
    HeaderValue::from_str(&format!(
        "{COOKIE}={value}; HttpOnly; SameSite=Strict; Path=/{secure}"
    ))
    .expect("an ASCII cookie")
}

fn clear_cookie(secure: bool) -> HeaderValue {
    let secure = if secure { "; Secure" } else { "" };
    HeaderValue::from_str(&format!(
        "{COOKIE}=; Max-Age=0; HttpOnly; SameSite=Strict; Path=/{secure}"
    ))
    .expect("an ASCII cookie")
}

fn is_api(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/")
}

/// The checks, in order: Host (421), Origin for anything that is not a
/// plain GET/HEAD (403), the session cookie for `/ws` and `/api/*` but
/// login (401).
fn check(auth: &Auth, req: &Request) -> Result<(), StatusCode> {
    let headers = req.headers();
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .or_else(|| req.uri().authority().map(|a| a.as_str()))
        .unwrap_or("");
    if !auth.allowed.host_ok(host) {
        return Err(StatusCode::MISDIRECTED_REQUEST);
    }
    let upgrade = headers
        .get(header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
    let plain_read = matches!(*req.method(), Method::GET | Method::HEAD) && !upgrade;
    if !plain_read {
        let origin = headers
            .get(header::ORIGIN)
            .and_then(|o| o.to_str().ok())
            .unwrap_or("");
        if !auth.allowed.origin_ok(origin) {
            return Err(StatusCode::FORBIDDEN);
        }
    }
    let path = req.uri().path();
    if path == "/ws" || (is_api(path) && path != "/api/login") {
        let live = cookie_value(headers).is_some_and(|v| auth.session_ok(&v, Instant::now()));
        if !live {
            return Err(StatusCode::UNAUTHORIZED);
        }
    }
    Ok(())
}

/// Middleware on every route and the asset fallback.
pub(crate) async fn guard(State(auth): State<Arc<Auth>>, req: Request, next: Next) -> Response {
    let path = req.uri().path().to_owned();
    let mut res = match check(&auth, &req) {
        Ok(()) => next.run(req).await,
        Err(status) => status.into_response(),
    };
    let h = res.headers_mut();
    h.insert(header::CONTENT_SECURITY_POLICY, auth.csp.clone());
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    if is_api(&path) || path == "/" || path == "/index.html" {
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    res
}

#[derive(Deserialize)]
struct LoginBody {
    token: String,
}

/// `POST /api/login {"token": …}`: the guard already checked Host and
/// Origin, so only same-origin attempts reach the rate limit.
pub(crate) async fn login(State(auth): State<Arc<Auth>>, req: Request) -> Response {
    let body = match axum::body::to_bytes(req.into_body(), LOGIN_BODY_MAX).await {
        Ok(body) => body,
        Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
    };
    let Ok(LoginBody { token }) = serde_json::from_slice(&body) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if let Err(status) = auth.login(&token, Instant::now()) {
        tracing::info!(status = status.as_u16(), "web: login refused");
        return status.into_response();
    }
    match auth.create_session(Instant::now()) {
        Ok(value) => {
            let mut res = StatusCode::NO_CONTENT.into_response();
            let cookie = set_cookie(&value, auth.allowed.secure_cookie());
            res.headers_mut().insert(header::SET_COOKIE, cookie);
            res
        }
        Err(e) => {
            tracing::error!(error = %e, "web: no random bytes for a session");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `GET /api/me`: 204, the guard having checked the cookie (the page asks
/// at load when its URL carries no token).
pub(crate) async fn me() -> StatusCode {
    StatusCode::NO_CONTENT
}

/// `POST /api/logout`: forget the session (the guard required it).
pub(crate) async fn logout(State(auth): State<Arc<Auth>>, headers: HeaderMap) -> Response {
    if let Some(value) = cookie_value(&headers) {
        auth.end_session(&value);
    }
    let mut res = StatusCode::NO_CONTENT.into_response();
    let cookie = clear_cookie(auth.allowed.secure_cookie());
    res.headers_mut().insert(header::SET_COOKIE, cookie);
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auth() -> Auth {
        Auth::new("ab".repeat(32), Allowed::new(7337, &[]), csp(None))
    }

    #[test]
    fn constant_time_eq_compares_whole_values() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"abcd"));
        assert!(!ct_eq(b"", b"a"));
        assert!(ct_eq(b"", b""));
    }

    #[test]
    fn hosts_and_origins_accept_only_the_listen_forms_and_allowed_names() {
        let a = Allowed::new(7337, &["Claudio.Example".to_owned()]);
        for ok in ["127.0.0.1:7337", "LOCALHOST:7337", "[::1]:7337", "claudio.example", "claudio.example:7337"] {
            assert!(a.host_ok(ok), "{ok}");
        }
        for bad in ["127.0.0.1:7338", "evil.example:7337", "127.0.0.1", "localhost", "", "claudio.example:80"] {
            assert!(!a.host_ok(bad), "{bad}");
        }
        for ok in ["http://127.0.0.1:7337", "https://claudio.example", "http://localhost:7337"] {
            assert!(a.origin_ok(ok), "{ok}");
        }
        for bad in [
            "null",
            "",
            "127.0.0.1:7337",
            "ftp://127.0.0.1:7337",
            "http://127.0.0.1:7337/x",
            "http://evil.example:7337",
            "http://127.0.0.1:7337.evil.example",
        ] {
            assert!(!a.origin_ok(bad), "{bad}");
        }
        assert!(a.secure_cookie());
        assert!(!Allowed::new(7337, &[]).secure_cookie());
        assert!(Allowed::new(80, &[]).host_ok("127.0.0.1"));
    }

    #[test]
    fn login_locks_after_five_failures_for_a_minute() {
        let a = auth();
        let t0 = Instant::now();
        for _ in 0..MAX_FAILURES {
            assert_eq!(a.login("wrong", t0), Err(StatusCode::UNAUTHORIZED));
        }
        assert_eq!(a.login("wrong", t0), Err(StatusCode::TOO_MANY_REQUESTS));
        assert_eq!(a.login(&"ab".repeat(32), t0 + Duration::from_secs(59)), Err(StatusCode::TOO_MANY_REQUESTS));
        assert_eq!(a.login(&"ab".repeat(32), t0 + Duration::from_secs(61)), Ok(()));
    }

    #[test]
    fn sessions_expire_when_idle_and_are_capped() {
        let a = auth();
        let t0 = Instant::now();
        let s = a.create_session(t0).unwrap();
        assert!(token_ok(&s));
        let t1 = t0 + SESSION_IDLE - Duration::from_secs(1);
        assert!(a.session_ok(&s, t1), "a use within the idle time keeps it");
        assert!(a.session_ok(&s, t1 + SESSION_IDLE - Duration::from_secs(1)), "and refreshes it");
        assert!(!a.session_ok(&s, t1 + 2 * SESSION_IDLE), "idle too long");
        let a = auth();
        let oldest = a.create_session(t0).unwrap();
        for i in 1..=MAX_SESSIONS as u64 {
            a.create_session(t0 + Duration::from_secs(i)).unwrap();
        }
        assert!(!a.session_ok(&oldest, t0 + Duration::from_secs(100)), "the least recently used went");
        a.end_session("nothing");
        assert!(!a.session_ok("", t0 + Duration::from_secs(100)));
    }

    #[test]
    fn cookie_value_finds_ours_among_others() {
        let mut h = HeaderMap::new();
        h.append(header::COOKIE, HeaderValue::from_static("a=1; claudio_web=abc ; b=2"));
        assert_eq!(cookie_value(&h).as_deref(), Some("abc"));
        let mut h = HeaderMap::new();
        h.append(header::COOKIE, HeaderValue::from_static("claudio_webx=1"));
        assert_eq!(cookie_value(&h), None);
    }

    #[test]
    fn token_file_is_private_reused_and_rotated() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("claudio-web-token-{}", uuid::Uuid::new_v4()));
        let path = dir.join("web-token");
        let first = load_or_create_token(&path, false).unwrap();
        assert!(token_ok(&first));
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(load_or_create_token(&path, false).unwrap(), first);
        let rotated = load_or_create_token(&path, true).unwrap();
        assert_ne!(rotated, first);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let replaced = load_or_create_token(&path, false).unwrap();
        assert_ne!(replaced, rotated, "a token others can read is replaced");
        std::fs::write(&path, "short\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(token_ok(&load_or_create_token(&path, false).unwrap()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn csp_allows_only_the_import_map_inline() {
        assert!(csp(None).contains("script-src 'self';"));
        let with = csp(Some("AAAA"));
        assert!(with.contains("script-src 'self' 'sha256-AAAA';"), "{with}");
        let script = with.split(';').find(|d| d.trim().starts_with("script-src")).unwrap();
        assert!(!script.contains("unsafe"), "{with}");
        assert!(!with.contains("unsafe-eval"), "{with}");
    }
}
```

`src/web/assets.rs` (new). Shape and rules:
```rust
pub(crate) struct Asset {
    pub path: &'static str,
    pub mime: &'static str,
    pub etag: &'static str,
    pub gz: &'static [u8],
}
include!(concat!(env!("OUT_DIR"), "/web_assets.rs")); // ASSETS, IMPORTMAP_SHA256

pub(crate) struct Assets { /* `disk: Option<PathBuf>` (debug builds), `importmap: Option<String>` */ }
impl Assets {
    /// Embedded assets; in a debug build with `CLAUDIO_WEB_DIR` set, files
    /// from that directory instead (the import-map hash is read from its
    /// index.html at start: restart after editing the map).
    pub(crate) fn load() -> Assets;
    pub(crate) fn importmap_sha256(&self) -> Option<&str>;
}
pub(crate) fn importmap_sha256(html: &[u8]) -> Option<String>; // the same function as build.rs
pub(crate) fn mime(path: &str) -> &'static str;                // the same table as build.rs
pub(crate) fn disk_path(root: &Path, url_path: &str) -> Option<PathBuf>;
pub(crate) fn accepts_gzip(accept_encoding: &str) -> bool;
pub(crate) async fn serve(State(state): State<AppState>, method: Method, uri: Uri, headers: HeaderMap) -> Response;
```
`serve` rules: methods other than GET/HEAD → 405. `/` means `/index.html`. Embedded: unknown path → 404; `If-None-Match` equal to the ETag → 304; `Accept-Encoding` accepting gzip → the gzip bytes with `Content-Encoding: gzip` and `Vary: Accept-Encoding`; otherwise decompress with `flate2::read::GzDecoder` inside `spawn_blocking`. Always `Content-Type` from the table and `ETag`; `Cache-Control: no-cache` except `/index.html` (the guard sets `no-store` there). Disk mode (`#[cfg(debug_assertions)]` only; the field does not exist in release builds): `disk_path` rejects any `..`, `.` or empty segment and any path that is not under the canonicalized root after canonicalization; files read in `spawn_blocking`, served uncompressed.

`src/web/ws.rs` (new; Task 4 fills in dispatch):
```rust
pub(crate) const MAX_TEXT: usize = 64 * 1024;
pub(crate) const MAX_INPUT: usize = 64 * 1024;
pub(crate) const KIND_OUTPUT: u8 = 0x01;
pub(crate) const KIND_INPUT: u8 = 0x02;

pub(crate) async fn upgrade(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
    ws.max_message_size(2 << 20)
        .max_frame_size(2 << 20)
        .on_upgrade(move |socket| tab(state, socket))
}
```
`tab` in this task: send `{"t":"hello","v":1,"version":env!("CARGO_PKG_VERSION"),"features":[]}`, then `{"t":"snapshot","hosts":[],"sessions":[],"profiles":{"names":[],"default":null},"layout":{"grid":1,"panes":[null],"focus":0},"order":[]}`; then loop: a text frame over `MAX_TEXT` → close 1009; text that is not a JSON object with `"t":"req"`, a `u32` `id` and a string `op` → close 1003; a request → `{"t":"res","id":…,"ok":false,"error":"unknown op"}`; a binary frame over `MAX_INPUT + 17` → close 1009; shorter than 17 bytes or not kind `0x02` → close 1003; a well-formed input frame is ignored for now; `Close` ends the loop. Close frames carry the code and a short reason (`"too big"`, `"bad frame"`).

Run: `cargo test --locked --bin claudio web::` → mod, auth and assets unit tests pass
Run: `cargo test --locked --test web -- --test-threads=1` → `test result: ok. 6 passed`
Run: `cargo build --release --locked` → builds; `make test-unit`, `make test-manager` → `0 failed`

Step 3: commit
`git commit -m "feat(web): claudio web serves the app with token login, host and origin checks" -- Cargo.toml Cargo.lock build.rs src/main.rs src/cli.rs src/web/mod.rs src/web/auth.rs src/web/assets.rs src/web/ws.rs tests/web.rs`

Interfaces this task provides to later tasks: `web::run`, `web::{AppState, router, parse_args, listen_addr}`, `auth::{Auth, Allowed, csp, cookie_value, token_path, load_or_create_token}`, `assets::{Assets, ASSETS, IMPORTMAP_SHA256, importmap_sha256}`, `ws::{upgrade, MAX_TEXT, MAX_INPUT, KIND_OUTPUT, KIND_INPUT}`; the test harness in tests/web.rs (`Web`, `Tab`, `apply_env`, `http`, `Reply`).

### Task 4: The hub: hosts, sessions, attach fan-out, input, approve, paste, read-only ops
Model: strong   Repo: /volumes/repos/claudio   Files: src/web/hub.rs (new), src/web/dto.rs (new), src/web/input.rs (new), src/web/state.rs (new), src/web/spawn.rs (new, minimal), src/web/ws.rs, src/web/mod.rs, src/client.rs, src/tui/mod.rs (visibility only), tests/web.rs
Goal: AC4, AC7 (and the protocol of docs/web-protocol.md §5 for hosts, sessions without proxy, files, reader, search, git, claude update, layout). The hub is one more daemon client per host, exactly like the TUI.
Depends on: Task 3
Patch: none

Visibility (only): in `src/tui/mod.rs` make `connections`, `sessions` and `state` `pub(crate) mod`; in `src/tui/sessions.rs` `sanitize_label` is already `pub`. Nothing else in `src/tui/`.

`src/client.rs` (not TUI): add
```rust
/// [`connect_ssh`], keeping ssh's last stderr lines: a failed connect's
/// error ends with them (sanitized, at most 3, each ≤ 200 chars), so the
/// web can say why (host keys, agent, permissions). The TUI keeps
/// `connect_ssh`, whose errors do not change.
pub async fn connect_ssh_reporting(host: &str) -> io::Result<Client>
```
by moving `connect_ssh`'s body into `async fn connect_ssh_with(host: &str, tail: Option<Arc<Mutex<VecDeque<String>>>>)`: the stderr supervisor pushes each line (through `claude::latex::sanitize`, trimmed, cut at 200 chars) into `tail` when set, keeping the last 3. `connect_ssh(host)` calls it with `None`. `connect_ssh_reporting` calls it with `Some`; on error it waits up to 500 ms for stderr to end, then returns `io::Error::new(kind, format!("{e}: {}", lines.join(" | ")))` (just `e` when there are none).

`src/web/input.rs` (new), in full:
```rust
//! What the web may type into a session for the user: an approval key,
//! guarded by the session's state and attention, and a paste, bracketed and
//! stripped of what could escape the brackets.

use crate::proto::{Attention, SessionState};

pub(crate) const STALE: &str = "stale";
pub(crate) const NOT_PERMISSION: &str = "not a permission prompt";
pub(crate) const TOO_LARGE: &str = "too large";
/// Enter: picks the option claude highlights by default ("Yes").
pub(crate) const APPROVE_KEY: &[u8] = b"\r";
/// Esc: rejects the tool and interrupts the turn.
pub(crate) const DENY_KEY: &[u8] = b"\x1b";
/// The most text one paste takes, in bytes.
pub(crate) const PASTE_MAX: usize = 64 * 1024;
const PASTE_START: &[u8] = b"\x1b[200~";
const PASTE_END: &str = "\x1b[201~";

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Decision {
    Approve,
    Deny,
}

/// The key `session.approve` may send, or why not. `attention` is the
/// server's current one for the session and `has_feature` whether its
/// daemon reports attentions at all; without, `attention_seq` must be 0
/// (real attentions start at 1, so 0 never matches one).
pub(crate) fn approve_key(
    state: SessionState,
    attention: Option<&Attention>,
    has_feature: bool,
    decision: Decision,
    attention_seq: u64,
) -> Result<&'static [u8], &'static str> {
    if state != SessionState::NeedsApproval {
        return Err(STALE);
    }
    if has_feature {
        let Some(a) = attention else {
            return Err(STALE);
        };
        if a.seq != attention_seq {
            return Err(STALE);
        }
        if !matches!(a.kind.as_str(), "permission_prompt" | "worker_permission_prompt") {
            return Err(NOT_PERMISSION);
        }
    } else if attention_seq != 0 {
        return Err(STALE);
    }
    Ok(match decision {
        Decision::Approve => APPROVE_KEY,
        Decision::Deny => DENY_KEY,
    })
}

/// The bytes of a paste of `text`: `ESC[200~`, the text, `ESC[201~`, then
/// `\r` when `submit`. Line ends become `\n`; any `ESC[201~` in the text and
/// every control but `\t` and `\n` (C0, DEL, C1) are removed, so the text
/// can neither end the paste early nor send keys.
pub(crate) fn paste_bytes(text: &str, submit: bool) -> Result<Vec<u8>, &'static str> {
    if text.len() > PASTE_MAX {
        return Err(TOO_LARGE);
    }
    let text = text
        .replace(PASTE_END, "")
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let clean: String = text
        .chars()
        .filter(|&c| c == '\t' || c == '\n' || !c.is_control())
        .collect();
    let mut out = Vec::with_capacity(clean.len() + 2 * PASTE_START.len() + 1);
    out.extend_from_slice(PASTE_START);
    out.extend_from_slice(clean.as_bytes());
    out.extend_from_slice(PASTE_END.as_bytes());
    if submit {
        out.push(b'\r');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use SessionState::{Idle, NeedsApproval, NeedsInput, Working};

    fn attention(seq: u64, kind: &str) -> Attention {
        Attention {
            seq,
            kind: kind.into(),
            message: String::new(),
            tool: None,
            detail: None,
        }
    }

    #[test]
    fn approve_needs_the_state_the_kind_and_the_seq() {
        let a = attention(4, "permission_prompt");
        assert_eq!(approve_key(NeedsApproval, Some(&a), true, Decision::Approve, 4), Ok(APPROVE_KEY));
        assert_eq!(approve_key(NeedsApproval, Some(&a), true, Decision::Deny, 4), Ok(DENY_KEY));
        assert_eq!(approve_key(NeedsApproval, Some(&a), true, Decision::Approve, 3), Err(STALE));
        assert_eq!(approve_key(NeedsApproval, Some(&a), true, Decision::Approve, 5), Err(STALE));
        assert_eq!(approve_key(Working, Some(&a), true, Decision::Approve, 4), Err(STALE));
        assert_eq!(approve_key(NeedsInput, Some(&a), true, Decision::Approve, 4), Err(STALE));
        assert_eq!(approve_key(NeedsApproval, None, true, Decision::Approve, 0), Err(STALE));
        let w = attention(2, "worker_permission_prompt");
        assert_eq!(approve_key(NeedsApproval, Some(&w), true, Decision::Approve, 2), Ok(APPROVE_KEY));
        for kind in ["elicitation_dialog", "elicitation_url_dialog", "idle_prompt", "session_start"] {
            let a = attention(7, kind);
            assert_eq!(
                approve_key(NeedsApproval, Some(&a), true, Decision::Approve, 7),
                Err(NOT_PERMISSION),
                "{kind}"
            );
        }
    }

    #[test]
    fn without_the_feature_only_seq_zero_passes() {
        assert_eq!(approve_key(NeedsApproval, None, false, Decision::Approve, 0), Ok(APPROVE_KEY));
        assert_eq!(approve_key(NeedsApproval, None, false, Decision::Approve, 1), Err(STALE));
        assert_eq!(approve_key(Idle, None, false, Decision::Approve, 0), Err(STALE));
    }

    #[test]
    fn paste_is_bracketed_and_cannot_escape() {
        assert_eq!(paste_bytes("hi", false).unwrap(), b"\x1b[200~hi\x1b[201~");
        assert_eq!(paste_bytes("hi", true).unwrap(), b"\x1b[200~hi\x1b[201~\r");
        let evil = "a\x1b[201~\rrm -rf /\r\x03\x7f\u{9b}2J\tb\r\nc";
        let out = paste_bytes(evil, false).unwrap();
        assert_eq!(out, "\x1b[200~a\nrm -rf /\n2J\tb\nc\x1b[201~".as_bytes());
        let body = &out[PASTE_START.len()..out.len() - PASTE_END.len()];
        assert!(!body.contains(&0x1b) && !body.contains(&b'\r'));
        // Removing one end marker cannot assemble another.
        let nested = paste_bytes("\x1b[20\x1b[201~1~x", false).unwrap();
        let body = &nested[PASTE_START.len()..nested.len() - PASTE_END.len()];
        assert!(!body.contains(&0x1b), "{body:?}");
    }

    #[test]
    fn paste_has_a_size_cap() {
        assert!(paste_bytes(&"x".repeat(PASTE_MAX), false).is_ok());
        assert_eq!(paste_bytes(&"x".repeat(PASTE_MAX + 1), false), Err(TOO_LARGE));
    }
}
```

`src/web/dto.rs` (new): the Session and Host DTOs of docs/web-protocol.md §4, built field by field with `serde_json::json!` (or `#[derive(Serialize)]` structs with exactly those fields, no `flatten`, no `Option` skipping: absent values are `null`):
```rust
pub(crate) const SESSION_FIELDS: &[&str] = &[
    "id", "host", "kind", "cwd", "name", "title", "label", "state", "state_since", "attention",
    "claude_session_id", "model", "context_tokens", "output_tokens", "turns", "branch", "git",
    "created_at", "pid", "dormant", "proxy", "proxy_known", "first_party", "ephemeral",
    "prompt_pending",
];
pub(crate) const HOST_FIELDS: &[&str] = &[
    "name", "connected", "connecting", "error", "outdated", "hostname", "os", "arch", "claude",
    "features", "version",
];
pub(crate) fn session(view: &hub::SessionView) -> serde_json::Value;
pub(crate) fn host(state: &hub::HostState) -> serde_json::Value;
/// The TUI's label rule (tui::sessions::SessionView::label), via
/// `tui::sessions::sanitize_label(raw, 200)`.
pub(crate) fn label(kind: SessionKind, name: Option<&str>, title: Option<&str>, cwd: &str) -> String;
```
Tests: `session_dto_has_exactly_the_documented_fields` (the JSON object's key set equals `SESSION_FIELDS`), `host_dto_has_exactly_the_documented_fields`, `label_follows_the_tui_rule` (name wins; claude title used unless empty or `"Claude Code"`; shell ignores its title and says `term`; claude falls back to the cwd's last component, `/` for `/`), `dormant_is_exited_without_a_pid`.

`src/web/state.rs` (new): `web-state.json`.
```rust
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct WebState {
    #[serde(default)] pub sessions: BTreeMap<SessionId, SessionMeta>,
    #[serde(default)] pub order: Vec<SessionId>,
    #[serde(default)] pub layout: Layout,
    /// Remote hosts the web was connected to: reconnected at start.
    #[serde(default)] pub hosts: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct SessionMeta { pub host: String, #[serde(default)] pub proxy: Option<String>, #[serde(default)] pub first_party: bool }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Layout { pub grid: u8, pub panes: Vec<Option<SessionId>>, pub focus: u8 }
impl Default for Layout { /* {grid 1, panes [None], focus 0} */ }
impl Layout { pub(crate) fn valid(&self) -> bool; /* grid ∈ {1,2,4}, panes.len() == grid, focus < grid */ }
pub(crate) fn path() -> PathBuf; // paths::config_dir().join("web-state.json")
pub(crate) fn load(path: &Path) -> WebState; // missing/unreadable/invalid → default; an invalid layout → default layout
pub(crate) fn save(path: &Path, s: &WebState) -> io::Result<()>; // paths::write_atomic (0600)
/// What is known of `sid`'s proxy: the web's own record first, then the
/// TUI's state.json (read only). `None`: nothing is known.
pub(crate) fn known_proxy(web: &WebState, tui: &ClientState, sid: SessionId) -> Option<(Option<String>, bool)>;
```
Tests: `web_state_round_trips_and_is_private` (mode 0600), `invalid_layouts_fall_back_to_the_default`, `known_proxy_prefers_web_state_then_tui_state`, `the_web_never_writes_state_json` (a `save` touches only its own path: the sibling `state.json` stays absent).

`src/web/hub.rs` (new): the actor. Types and signatures (exact), behaviour rules (binding):
```rust
pub(crate) type TabId = u64;
/// The hub's own subscription (an opening prompt), never a real tab.
pub(crate) const PROMPT_TAB: TabId = 0;
pub(crate) const MAX_TABS: usize = 16;
pub(crate) const TAB_QUEUE_MAX: usize = 8 << 20;
pub(crate) const OUTDATED_RETRY: Duration = Duration::from_secs(120);
pub(crate) const PROMPT_DEADLINE: Duration = Duration::from_secs(60);
pub(crate) const PROMPT_QUIET: Duration = Duration::from_millis(500);
pub(crate) const TICK: Duration = Duration::from_millis(100);

pub(crate) enum Out { Text(String), Binary(axum::body::Bytes), Close(u16, &'static str) }

/// A tab's outgoing queue, bounded by queued bytes.
#[derive(Clone)]
pub(crate) struct TabTx { /* UnboundedSender<Out>, Arc<AtomicUsize> queued, Arc<AtomicBool> cut */ }
impl TabTx {
    pub(crate) fn new() -> (TabTx, mpsc::UnboundedReceiver<Out>);
    /// Queue `out`. Past TAB_QUEUE_MAX queued bytes the tab is cut: one
    /// Close(1013, "too slow") is queued, false is returned, and every later
    /// send is dropped.
    pub(crate) fn send(&self, out: Out) -> bool;
    /// The writer took `n` bytes off the queue.
    pub(crate) fn sent(&self, n: usize);
}

/// Per-session subscribers: a set of tabs (PROMPT_TAB included).
#[derive(Default)]
pub(crate) struct Subs { /* BTreeSet<TabId> */ }
pub(crate) enum AttachStep { Attach { rows: u16, cols: u16 }, Reattach { rows: u16, cols: u16 } }
impl Subs {
    /// First subscriber: attach at its size. Later: re-attach at `current`
    /// (the session's size from the last `Attached`), so the PTY is not
    /// resized and every subscriber gets a fresh snapshot.
    pub(crate) fn subscribe(&mut self, tab: TabId, want: (u16, u16), current: Option<(u16, u16)>) -> AttachStep;
    /// Whether that was the last subscriber (send `Detach`).
    pub(crate) fn unsubscribe(&mut self, tab: TabId) -> bool;
    pub(crate) fn tabs(&self) -> impl Iterator<Item = TabId> + '_;
}

/// An opening prompt waiting for the session to take it.
pub(crate) struct PendingPrompt { /* text, deadline, saw_input, bracketed, last_output, tail: [u8; 7] window */ }
pub(crate) enum PromptStep { Wait, Paste, GiveUp }
impl PendingPrompt {
    pub(crate) fn new(text: String, now: Instant) -> PendingPrompt;
    pub(crate) fn on_state(&mut self, state: SessionState);       // NeedsInput sets saw_input
    pub(crate) fn on_output(&mut self, bytes: &[u8], now: Instant); // finds ESC[?2004h across chunk edges; stamps last_output
    pub(crate) fn step(&self, now: Instant) -> PromptStep;          // Paste when saw_input && bracketed && quiet ≥ 500 ms; GiveUp at the deadline
}

pub(crate) struct HostState { /* name, client: Option<Client>, generation, connecting, error, welcome: Option<Welcome>, outdated, stats, features, claude_offer: Option<Value>, offered: bool */ }
pub(crate) struct SessionView {
    pub host: String,
    pub info: SessionInfo,          // as the daemon reported it, updated by events
    pub git: Option<GitStatus>,
    pub output_tokens: Option<u64>,
    pub turns: Option<u32>,
    pub state_since: u64,
    pub lens_seq: u64,
    pub lens: Vec<LensItem>,
    pub subs: Subs,
    pub size: Option<(u16, u16)>,   // from the last Attached
    pub prompt: Option<PendingPrompt>,
    pub meta: Option<(Option<String>, bool)>, // known proxy/first_party
}
/// Apply one daemon event to a session as `App::on_event` does for the
/// DTO's fields; `lens_host` says whether its daemon has the `lens`
/// feature. Returns whether the DTO changed and the `lens`/`latex` events
/// to send (if any). Pure: no I/O.
pub(crate) fn apply_event(view: &mut SessionView, event: SessionEvent, lens_host: bool, now: u64) -> Applied;
pub(crate) struct Applied { pub changed: bool, pub removed: bool, pub lens: Option<(u64, Vec<LensItem>)>, pub notice: Option<String> }

pub(crate) enum HubEvent { HostUp { host: String, features: Vec<String>, has_claude: bool }, HostDown { host: String }, ClaudeInstalled { host: String } }

pub(crate) struct HubConfig {
    pub config: crate::config::Config,
    pub state_path: PathBuf,                    // web-state.json
    pub tui_state: crate::tui::state::ClientState, // read at start, never written
    pub profiles: (Vec<String>, Option<String>),
}

#[derive(Clone)]
pub(crate) struct HubHandle { /* mpsc::Sender<Cmd> (capacity 1024), broadcast::Sender<HubEvent> */ }

/// Start the actor; it connects `local` and every remembered remote host.
pub(crate) fn start(cfg: HubConfig) -> HubHandle;

pub(crate) struct SessionRef { pub host: String, pub client: Client, pub features: Vec<String>, pub info: SessionInfo, pub proxy: Option<String>, pub first_party: bool, pub proxy_known: bool }

impl HubHandle {
    pub(crate) async fn open_tab(&self, tx: TabTx) -> Result<TabId, &'static str>; // "too many tabs"
    pub(crate) fn close_tab(&self, tab: TabId);
    pub(crate) async fn attach(&self, tab: TabId, sid: SessionId, rows: u16, cols: u16) -> Result<(), String>;
    pub(crate) async fn detach(&self, tab: TabId, sid: SessionId) -> Result<(), String>;
    pub(crate) async fn resize(&self, sid: SessionId, rows: u16, cols: u16) -> Result<(), String>;
    pub(crate) fn input(&self, sid: SessionId, bytes: Vec<u8>);
    pub(crate) async fn approve(&self, sid: SessionId, decision: Decision, seq: u64) -> Result<(), String>;
    pub(crate) async fn paste(&self, sid: SessionId, text: String, submit: bool) -> Result<(), String>;
    pub(crate) async fn session(&self, sid: SessionId) -> Result<SessionRef, String>;
    pub(crate) async fn host_client(&self, host: &str) -> Result<(Client, Vec<String>), String>;
    pub(crate) async fn hosts(&self) -> Vec<serde_json::Value>;
    pub(crate) async fn connect(&self, host: String) -> Result<serde_json::Value, String>;
    pub(crate) async fn disconnect(&self, host: String) -> Result<(), String>;
    /// A spawn this server made: record its proxy (web-state.json) and, with
    /// `prompt`, subscribe PROMPT_TAB at `size` and wait to paste it.
    pub(crate) async fn spawned(&self, host: String, sid: SessionId, meta: SessionMeta, prompt: Option<(String, (u16, u16))>);
    pub(crate) async fn order(&self, tab: TabId, sids: Vec<SessionId>);
    pub(crate) async fn layout(&self, tab: TabId, layout: Layout) -> Result<(), String>;
    pub(crate) fn broadcast(&self, ev: serde_json::Value);
    pub(crate) fn subscribe(&self) -> broadcast::Receiver<HubEvent>;
    pub(crate) async fn claude_update(&self, host: String, install: bool) -> Result<(), String>;
    pub(crate) async fn claude_skip(&self, host: String);
    pub(crate) async fn set_profiles(&self, names: Vec<String>, default: Option<String>);
    pub(crate) async fn shutdown(&self);
}
```
Behaviour rules:
1. One actor task owns all state (`HostState`s, `SessionView`s, tabs, web-state). Everything that waits on a daemon reply runs in a spawned task that reports back with a `Cmd`; the actor never awaits a daemon request. Tab request handlers (ws.rs) take a `Client` clone from `session()`/`host_client()` and call `client.request(..)` themselves.
2. Connections: per host one connector task: local = `client::ensure_local_daemon()` then `client::connect(&paths::daemon_socket())` (`DaemonCheck::Deferred` → `mark_daemon_outdated`); remote = `remote::bootstrap::ensure_remote(host)` (skipped on a reconnect unless the last failure was a bootstrap one) then `client::connect_ssh_reporting(host)`. Then `ListSessions`, `SubscribeHostStats` (an error is ignored: old daemon), `take_incoming()` forwarded to the actor as `Cmd::Incoming { host, generation, msg }` until `Incoming::Disconnected`. Backoff and generations via `tui::connections::Connections` (1 s doubling to 30 s, ±20 % jitter); a message of an older generation is dropped. At start: `local`, then every host in `web-state.json.hosts` and every remote host of the TUI's `state.json` sessions (read only). `hosts.connect` success adds the host to `web-state.json.hosts` and calls `remote::hosts::touch` (in `spawn_blocking`); `hosts.disconnect` drops the connection, sends `session_removed` for its sessions then `host_removed`, and removes it from `web-state.json.hosts`. `remote::validate_host` runs before anything; `"local"` cannot be connected or disconnected.
3. Session model: `Connected` replaces the host's sessions with `ListSessions` (ephemeral ones skipped), sends `session_removed` for ids gone and `session` for the others, re-attaches every session with subscribers (at its last size, else 24×80), and emits `HubEvent::HostUp`. `Lost` marks the host disconnected, its sessions `unknown` (state, `state_since` now), sends `host` + `session` events, emits `HostDown`, schedules a reconnect. `apply_event` follows `App::on_event` (src/tui/interaction.rs:801) for the DTO's fields: `Created` adds unless ephemeral (or updates in place on a respawn, keeping subscribers, resetting `lens_seq` to 0 and dropping the attention); `Removed` deletes; `State` sets state and `state_since`, and drops the attention when the new state is neither `needs_approval` nor `needs_input`; `Attention` stores it; `Exited` sets `exited`, clears pid and attention; `ClaudeSession`/`ClaudeSessionCleared`/`Title`/`Renamed` as the TUI; `Meta`: `None` is no news; `Notice` → a `notice` event (`level: "info"`, `host`, `sid`); `Latex` taken as the `latex` lens list only from a host without the `lens` feature, `Lens` only from a host with it, a list with `seq` lower than the held one dropped; `Attached` resets `lens_seq` to 0. Every DTO change sends `session` to every tab. Every taken lens list sends `lens` and `latex` (its `latex`-tagged texts, same seq).
4. Attach: `session.attach` → `Subs::subscribe`; `Attach` sends `Msg::Attach { id, rows, cols }` with the tab's size, `Reattach` with the current size. `Incoming::Attached` → `size` updated, `lens_seq` reset, `attached {sid, rows, cols}` to every subscribed tab. `Incoming::Data` → one `0x01 ‖ uuid ‖ bytes` binary frame (one `Bytes`, cloned per tab) to every subscribed tab (and `on_output` of a pending prompt). A tab whose `TabTx::send` fails is closed (1013) and unsubscribed everywhere. `session.detach` and tab close → `Subs::unsubscribe`; the last one sends `Msg::Detach`. `session.resize` → `Msg::Resize` (no hub-side size bookkeeping beyond the next `Attached`).
5. Input: binary `0x02` frames → `client.send_input(id, bytes)` if the session exists and its host is connected, else dropped. `session.approve` runs `input::approve_key(state, attention, host has "attention", decision, seq)` and `send_input` in the same actor step (no await between) and logs `op=session.approve host sid`. `session.paste` → `input::paste_bytes` → `send_input`.
6. Opening prompt (`spawned(.., Some(prompt))`): the view gets a `PendingPrompt` and `prompt_pending: true`; PROMPT_TAB subscribes at the spawn's size. Every `TICK` the actor steps every pending prompt: `Paste` → `send_input(paste_bytes(text, true))`, PROMPT_TAB unsubscribes, `prompt_pending: false`; `GiveUp` → the same without the paste plus a `notice` "opening prompt not sent: the session did not get ready in 60 s".
7. Tabs: `open_tab` refuses past `MAX_TABS`; the actor queues the snapshot (`hosts`, `sessions` in board order, `profiles`, `layout`, `order`) and the replay events (docs/web-protocol.md §3.1) on the tab's `TabTx` before registering it, so no event can come before them. `order`/`layout` are saved (web-state.json) and sent to the other tabs. `shutdown` closes every tab with 1001.
8. Host stats → `host_stats` events (cpu as `cpu_pct`). Outdated hosts: every `OUTDATED_RETRY`, `ListSessions`; when `!freshness::busy(&sessions)`, reconnect (the freshness check replaces the daemon).
9. Claude on connect, once per host per run: no claude in `Welcome` → `[claude] install`: `auto` runs the install, `ask` sends `claude {phase:"offer_install"}`, `off` nothing. A remote claude older than the local one (`claude::update::needs_update`) → `[claude] remote_check` likewise (`offer_update` with `have`/`want`), unless the TUI's `state.json` `claude_skipped[host]` names that version. `claude.update` → `claude {phase:"installing"|"updating"}`, `Msg::UpdateClaude { install }` in a task, then `done {version}` (updating Host.claude) or `failed {tail}` (sanitized, ≤ 2 KiB); a successful install emits `HubEvent::ClaudeInstalled`. `claude.skip` closes the offer for this run.
10. web-state.json is written in `spawn_blocking`, at most every 500 ms (dirty flag + tick). The TUI's state.json is never written.

`src/web/spawn.rs` (new, minimal; Task 5 replaces it):
```rust
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum ProxyArg { Default, Direct, Profile { name: String } }
#[derive(Debug, Deserialize)]
pub(crate) struct SpawnReq { pub host: String, pub cwd: String, pub rows: u16, pub cols: u16, pub proxy: ProxyArg, #[serde(default)] pub first_party: bool, #[serde(default)] pub resume: Option<String>, #[serde(default)] pub name: Option<String>, #[serde(default)] pub prompt: Option<String>, #[serde(default)] pub skip_plugins: Vec<String> }
#[derive(Debug, Deserialize)]
pub(crate) struct RespawnReq { pub sid: SessionId, pub fresh: bool, pub rows: u16, pub cols: u16, #[serde(default)] pub proxy: Option<ProxyArg>, #[serde(default)] pub first_party: bool }
/// Checks shared with Task 5: cwd 1..=4096 bytes; rows/cols 1..=1000; name ≤ 200 chars after `sanitize_label`; resume `[A-Za-z0-9-]{1,64}`; prompt ≤ 64 KiB.
pub(crate) fn check_spawn(req: &SpawnReq) -> Result<(), String>;
pub(crate) async fn spawn_claude(hub: &HubHandle, req: SpawnReq) -> Result<SessionId, String>;
pub(crate) async fn spawn_shell(hub: &HubHandle, host: String, cwd: String, name: Option<String>, rows: u16, cols: u16) -> Result<SessionId, String>;
pub(crate) async fn respawn(hub: &HubHandle, req: RespawnReq) -> Result<SessionId, String>;
```
In this task `spawn_claude` builds `Msg::Spawn(SpawnSpec { id: Uuid::new_v4(), cwd, name, args: resume → ["--resume", id], env: vec![], rows, cols, ephemeral: false, extensions: None })` only when the proxy resolves to none (`direct`, or `default` with no `[proxy] default` and no `CLAUDIO_PROXY_URL`); anything else answers `"proxy profiles are not wired yet"` (Task 5 replaces this). `first_party` without a proxy → `"first-party needs a proxy profile"`. `respawn`: a shell → `Msg::Respawn` with no env; a claude session → the request's `proxy`, else `hub.session(sid)`'s known proxy; none known → `"proxy unknown"`; a known direct one → `Msg::Respawn` with no env and `extensions: None`. `spawn_shell` → `Msg::SpawnShell`. Every spawn/respawn calls `hub.spawned(…)` and logs `op host sid`.

`src/web/ws.rs`: the tab loop becomes: queue `hello` on a new `TabTx`, then `hub.open_tab(TabTx)` (the actor queues the snapshot and replay after it) (503 was already decided at upgrade time: check a shared tab counter in `upgrade` and answer `503 too many tabs` before upgrading); a writer task drains the `Out` queue (calling `TabTx::sent`); the reader task enforces §2 limits, routes binary input to `hub.input`, and spawns one task per request (a per-tab `Semaphore(64)`; none free → `"busy"`). `dispatch(state, tab, op, args) -> Result<Value, String>` covers, in this task: `hosts.list|candidates|connect|disconnect`, `session.attach|detach|resize|spawn|shell|respawn|kill|rename|paste|approve|order`, `fs.list|projects`, `claude.sessions`, `reader.answers|last_prompt`, `search`, `git.status|worktree_diff|log|commit|diff`, `claude.update|skip`, `layout.save`; everything else `"unknown op"`. Args are parsed into `#[derive(Deserialize)] #[serde(deny_unknown_fields)]` structs per op (session ids under `sid`); a parse error answers `"bad request: <serde message>"`. `reader.answers` reverses the daemon's oldest-first list. `search` follows docs/web-protocol.md §5 (a per-host `tokio::sync::Semaphore(1)` held in `AppState`, a 10 s deadline counting the wait, one retry after 500 ms on `search busy`). `git.*` use the session's `cwd`; `git.status`/`git.worktree_diff` need the host's `worktree` feature (`"unsupported by this host's daemon"`). Daemon errors are passed through (`io::Error` text, sanitized, ≤ 300 chars).

`src/web/mod.rs`: `mod dto; mod hub; mod input; mod spawn; mod state;`; `AppState` gains `hub: hub::HubHandle` and `search_slots`; `serve` loads config (`config::load`), the TUI's `ClientState::load(&paths::client_state())`, `web-state.json` and `proxy::resolve::load_proxy_profiles()` in `spawn_blocking` before `hub::start`; shutdown calls `hub.shutdown()`.

Step 1: write the failing tests.

Unit tests (pure), named and asserting:
- `src/web/input.rs`: the four tests above.
- `src/web/hub.rs`: `meta_none_is_no_news` (a `Meta` with only `model` keeps branch/git/tokens); `lens_only_from_lens_hosts_latex_only_from_others`; `lower_seq_lists_are_dropped_until_reattached` (seq 5 taken, 3 dropped, after `Attached` 1 taken); `leaving_attention_states_clears_the_attention` (`State working` after an attention → `attention: null`; `State needs_input` keeps it); `created_and_exited_drop_the_attention`; `ephemeral_sessions_are_never_listed`; `attach_is_refcounted_across_tabs` (tab 1 → `Attach {24,80}`, tab 2 with want 40×100 and current 24×80 → `Reattach {24,80}`, unsubscribe 2 → false, unsubscribe 1 → true, unsubscribing an unknown tab → false); `a_tab_past_8_mib_is_cut` (sends of 1 MiB: the 9th returns false, the queue's last item is `Close(1013, _)`, later sends dropped; `sent()` makes room again only for an uncut tab); `opening_prompt_waits_for_input_bracketed_paste_and_quiet` (with injected `Instant`s: `Wait` before needs_input, `Wait` before `ESC[?2004h` (split across two chunks: `"\x1b[?20"` + `"04h"`), `Wait` until 500 ms of quiet, then `Paste`; another prompt `GiveUp` at 60 s).
- `src/web/dto.rs`, `src/web/state.rs`: as listed above.

Integration tests (`tests/web.rs`; add the fake claude and the hook helper, then the tests). The fake and helpers, in full:
```rust
/// A fake claude for the web tests: answers --version/--help, saves its
/// --settings JSON (the hook commands) to `.fake-settings.json` in its cwd,
/// optionally turns bracketed paste on, prints a banner, then logs every
/// input line to `.fake-input.log`, answers `size` with `stty size` and
/// echoes any other line as `line:<text>`.
fn write_web_fake_claude(h: &ManagerHarness, bracketed_paste: bool) {
    let bp = if bracketed_paste { r"printf '\033[?2004h'" } else { ":" };
    let script = format!(
        r#"#!/bin/sh
[ "$1" = "--version" ] && echo 'claude 0.0.0-fake' && exit 0
[ "$1" = "--help" ] && exit 0
printf '%s' "$2" > .fake-settings.json
{bp}
echo "FAKE_CLAUDE_BANNER pid=$$"
while IFS= read -r line; do
  printf '%s\n' "$line" >> .fake-input.log
  case "$line" in
    size) stty size ;;
    *) printf 'line:%s\n' "$line" ;;
  esac
done
"#
    );
    fs::write(&h.fake_claude, script).unwrap();
    fs::set_permissions(&h.fake_claude, fs::Permissions::from_mode(0o755)).unwrap();
}

/// Run the hook command for `event` of the session started in `dir` (from
/// the settings its fake claude saved) with `payload` on stdin, as claude
/// does.
fn fire_hook(dir: &std::path::Path, event: &str, payload: Value) {
    let path = dir.join(".fake-settings.json");
    let deadline = std::time::Instant::now() + WAIT;
    let settings: Value = loop {
        if let Some(v) = fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()) {
            break v;
        }
        assert!(std::time::Instant::now() < deadline, "no settings in {dir:?}");
        thread::sleep(std::time::Duration::from_millis(50));
    };
    let command = settings["hooks"][event][0]["hooks"][0]["command"]
        .as_str()
        .expect("a hook command")
        .to_owned();
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(&command)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(payload.to_string().as_bytes()).unwrap();
    assert!(child.wait().unwrap().success());
}

/// `dir/.fake-input.log` once it contains `needle`.
fn input_log_until(dir: &std::path::Path, needle: &str) -> String {
    let deadline = std::time::Instant::now() + WAIT;
    loop {
        let log = fs::read_to_string(dir.join(".fake-input.log")).unwrap_or_default();
        if log.contains(needle) {
            return log;
        }
        assert!(std::time::Instant::now() < deadline, "{needle:?} not in {log:?}");
        thread::sleep(std::time::Duration::from_millis(50));
    }
}

impl Tab {
    /// Wait until `host` is connected (the snapshot may already say so).
    async fn connected(&mut self, host: &str) {
        let in_snapshot = self.snapshot["hosts"]
            .as_array()
            .is_some_and(|hs| hs.iter().any(|h| h["name"] == host && h["connected"] == true));
        if !in_snapshot {
            self.event("host", |v| v["host"]["name"] == host && v["host"]["connected"] == true).await;
        }
    }

    /// Spawn a direct claude session in `dir` at 24×80 and attach to it; its sid.
    async fn spawn_attached(&mut self, dir: &std::path::Path) -> String {
        self.connected("local").await;
        let reply = self
            .req(
                "session.spawn",
                json!({"host": "local", "cwd": dir, "rows": 24, "cols": 80, "proxy": {"kind": "direct"}}),
            )
            .await
            .expect("spawn");
        let sid = reply["sid"].as_str().unwrap().to_owned();
        self.req("session.attach", json!({"sid": sid, "rows": 24, "cols": 80})).await.expect("attach");
        let attached = self.event("attached", |v| v["sid"] == sid.as_str()).await;
        assert_eq!((attached["rows"].as_u64(), attached["cols"].as_u64()), (Some(24), Some(80)));
        self.output_until(&sid, "FAKE_CLAUDE_BANNER").await;
        sid
    }
}
```
AC4 and AC7, in full:
```rust
/// AC4: attach → `attached` + the snapshot frame; input reaches the PTY;
/// a resize reaches it too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_attach_snapshot_input_and_resize() {
    let h = ManagerHarness::new();
    write_web_fake_claude(&h, false);
    let dir = h.dirs[0].clone();
    let web = Web::start_with(h, &[], |_| {});
    let cookie = web.login();
    let mut tab = Tab::open(&web, &cookie).await;
    let sid = tab.spawn_attached(&dir).await;
    tab.input(&sid, b"ping-web\r").await;
    tab.output_until(&sid, "line:ping-web").await;
    tab.req("session.resize", json!({"sid": sid, "rows": 30, "cols": 100})).await.unwrap();
    tab.input(&sid, b"size\r").await;
    tab.output_until(&sid, "30 100").await;
}

/// AC7: approve only for a permission prompt at the current seq.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_approve_is_guarded_by_state_kind_and_seq() {
    let h = ManagerHarness::new();
    write_web_fake_claude(&h, false);
    let dir = h.dirs[0].clone();
    let web = Web::start_with(h, &[], |_| {});
    let cookie = web.login();
    let mut tab = Tab::open(&web, &cookie).await;
    let sid = tab.spawn_attached(&dir).await;
    fire_hook(&dir, "PreToolUse", json!({"tool_name": "Bash", "tool_input": {"command": "ls -la"}}));
    fire_hook(
        &dir,
        "Notification",
        json!({"notification_type": "permission_prompt", "message": "Claude needs your permission to use Bash"}),
    );
    let ev = tab
        .event("session", |v| {
            v["session"]["id"] == sid.as_str()
                && v["session"]["state"] == "needs_approval"
                && v["session"]["attention"]["kind"] == "permission_prompt"
        })
        .await;
    let a = &ev["session"]["attention"];
    assert_eq!(a["tool"], "Bash");
    assert_eq!(a["detail"], "ls -la");
    assert_eq!(a["message"], "Claude needs your permission to use Bash");
    let seq = a["seq"].as_u64().unwrap();
    let approve = |s: u64| json!({"sid": sid, "decision": "approve", "attention_seq": s});
    assert_eq!(tab.req("session.approve", approve(seq + 1)).await, Err("stale".to_owned()));
    assert_eq!(tab.req("session.approve", approve(seq - 1)).await, Err("stale".to_owned()));
    tab.input(&sid, b"abc").await;
    tab.req("session.approve", approve(seq)).await.expect("approve");
    input_log_until(&dir, "abc\n"); // the Enter reached the PTY and ended the line
    fire_hook(&dir, "Notification", json!({"notification_type": "elicitation_dialog", "message": "pick one"}));
    let ev = tab
        .event("session", |v| {
            v["session"]["id"] == sid.as_str() && v["session"]["attention"]["kind"] == "elicitation_dialog"
        })
        .await;
    let seq = ev["session"]["attention"]["seq"].as_u64().unwrap();
    assert_eq!(tab.req("session.approve", approve(seq)).await, Err("not a permission prompt".to_owned()));
    let unknown = json!({"sid": uuid::Uuid::new_v4().to_string(), "decision": "deny", "attention_seq": 1});
    assert_eq!(tab.req("session.approve", unknown).await, Err("no such session".to_owned()));
}
```
The other integration tests of this task (write them with the helpers above; names and assertions are binding):
- `web_paste_reaches_the_pty_bracketed`: `session.paste {sid, text: "hello\u001b[201~ web", submit: true}` → `input_log_until(dir, "\u{1b}[200~hello web\u{1b}[201~")`; a 64 KiB + 1 text → `"too large"`.
- `web_two_tabs_share_one_attach`: tab A attached; tab B attaches at 40×100 → both A and B get `attached {rows: 24, cols: 80}`; input from B → `line:` output in both; B detaches → A still gets output; `size` still answers `24 80`.
- `web_session_lifecycle_shell_rename_kill`: `session.shell` → a `session` event with `kind: "shell"`, label `term`; `session.rename {name: "renamed"}` → `session` event with that name and label; `{name: null}` → name null; `session.kill` → `session_removed {sid}`; a second kill → `"no such session"`.
- `web_refuses_unknown_ids_and_bad_args`: `session.kill` of a random uuid → `"no such session"`; `session.attach` with `rows: 0` → error starting `"bad request"`; `session.spawn` with an unknown extra key → `"bad request…"`; `hosts.connect {host: "local"}` → `"local is always connected"`; `hosts.disconnect {host: "local"}` → `"cannot disconnect local"`; `hosts.connect {host: "-oProxyCommand=x"}` → an error (validate_host); `fs.list {host: "nohost", path: "/"}` → `"no such host"`.
- `web_opening_prompt_is_pasted_after_bracketed_paste_mode`: fake with bracketed paste; `session.spawn` with `prompt: "first task"` → a `session` event with `prompt_pending: true`; nothing in the input log yet; `fire_hook(SessionStart {session_id: "c1", source: "startup"})` → `input_log_until(dir, "\u{1b}[200~first task\u{1b}[201~")`; a later `session` event has `prompt_pending: false`.
- `web_tab_limit_is_16`: 16 tabs open; the 17th upgrade answers 503; closing one lets a new one in.
- `web_git_status_and_worktree_diff`: a session whose cwd is a repo made with `common::run_git` (commit `a.txt`, then modify it); `git.status` lists `a.txt` with `worktree: "M"`; `git.worktree_diff {path: "a.txt", staged: false}` has `+changed`; `{path: "../x"}` → `"no such path"`; `git.log` has ≥ 1 commit.
- `web_reader_and_search_read_a_fixture_transcript`: write `<CLAUDE_CONFIG_DIR>/projects/<encode_cwd(dir)>/conv-1.jsonl` with two prompt/answer pairs (record shapes as in the `recent_answers` unit tests of src/claude/projects.rs, `cwd` = the canonical dir, the second answer containing `fixtureword`); spawn in `dir`; `fire_hook(SessionStart {session_id: "conv-1", source: "resume"})`; `reader.answers` → two answers, newest first; `search {query: "fixtureword"}` → a hit with `host: "local"`, `conversation: "conv-1"`, `failed: []`.
- `web_hosts_report_daemon_features`: `hosts.list` has `local` connected, `features` containing `attention` and `worktree`, `claude.version` `0.0.0-fake`.
- `web_order_and_layout_persist_in_web_state`: `session.order {sids: [b, a]}` → the other tab gets `order`; `layout.save {grid: 2, panes: [a, null], focus: 0}` → `layout` in the other tab; an invalid layout (`grid: 3`) → `"bad request…"`; after a server restart the snapshot has that layout and order; `config_home/claudio/web-state.json` is 0600 and `config_home/claudio/state.json` does not exist.

Run: `cargo test --locked --bin claudio web::` → `0 failed`
Run: `cargo test --locked --test web -- --test-threads=1` → all Task 3 and Task 4 tests pass, `0 failed`
Run: `make test-unit` → `0 failed`; `make test-manager` → `0 failed`

Step 3: commit (two commits are fine: the hub, then its tests, each with explicit paths)
`git commit -m "feat(web): the hub serves sessions, terminals and approvals to the browser" -- src/web/hub.rs src/web/dto.rs src/web/input.rs src/web/state.rs src/web/spawn.rs src/web/ws.rs src/web/mod.rs src/client.rs src/tui/mod.rs tests/web.rs`

Interfaces this task provides to later tasks: `hub::{HubHandle, HubEvent, SessionRef, TabTx, Out, MAX_TABS}`, `HubHandle::{session, host_client, spawned, broadcast, subscribe, set_profiles}`, `input::{Decision, approve_key, paste_bytes}`, `state::{WebState, SessionMeta, Layout, known_proxy}`, `spawn::{ProxyArg, SpawnReq, RespawnReq, check_spawn}`, `ws::dispatch` (Task 5 adds ops to its match), `client::connect_ssh_reporting`, tests/web.rs helpers (`write_web_fake_claude`, `fire_hook`, `input_log_until`, `Tab::{connected, spawn_attached}`).

### Task 5: Spawn composition, proxy, plugins, extensions, snippets
Model: strong   Repo: /volumes/repos/claudio   Files: src/web/spawn.rs (rewrite), src/web/proxy.rs (new), src/web/plugins.rs (new), src/web/ext.rs (new), src/web/ws.rs, src/web/mod.rs, src/web/hub.rs (only what the config re-read needs: `set_profiles`, `set_claude_policy`), src/tui/mod.rs and src/tui/plugins.rs (visibility only), tests/web.rs
Goal: AC3, and parity of spawn/respawn with the TUI (proxy env, first-party, plugins, extensions), plus the proxy, snippets, extensions and plugins ops of docs/web-protocol.md §5.
Depends on: Task 4
Patch: none

Visibility (only): `src/tui/mod.rs`: `extensions`, `plugins`, `plugin_setup`, `snippets`, `stats_view` → `pub(crate) mod`. `src/tui/plugins.rs`: `fn reask_after` and `const INSTALLING` → `pub(crate)`. Reused as they are: `tui::plugins::{compose, with_plugins, PluginPart, Composed, ListState, ManifestState, ApplyState, Apply, REFRESH_SECS, REASK_EVERY, MAX_SPAWN_PLUGINS, reask_after}`, `tui::plugin_setup::choices_for`, `tui::extensions::{Registry, Extension, Source, ActionResult, ListItem, manifest, host::ExtensionHost, protocol::SessionContext}`, `tui::snippets::{load, save, dir}`, `tui::stats_view::Window::param`, `proxy::resolve::{load_proxy_profiles, find_profile, proxy_env_for}`, `proxy::api::{fetch_config, fetch_stats, fetch_pool_health, fetch_session, switch_session}`, `proxy::plugins::{fetch, fetch_manifest, save_cache, remove_cache, load_all}`, `plugins::manifest::Manifest::parse_named`, `config::{load, save_extension, save_plugin_choices}`. Logic that lives in `App` methods (`plugin_parts`, `request_manifests`, `ask`, `on_applied`, `reask_installing`, `reapply_host`, `send_spawn`, `send_with_proxy`) is re-expressed in async code here; the TUI is not touched.

Interfaces (exact signatures; bodies follow the rules):
```rust
// src/web/proxy.rs — every DTO built field by field; no Profile or token leaves this file.
pub(crate) struct ProxyCache { /* Mutex<HashMap<String, (ConfigResponse, Instant)>> */ }
impl ProxyCache {
    pub(crate) fn new() -> ProxyCache;
    /// `find_profile` in spawn_blocking; `"proxy profile '<name>' not found"`.
    pub(crate) async fn profile(name: &str) -> Result<Profile, String>;
    /// `/v1/claudio/config` of `name`, cached 300 s; a failure (5 s timeout) is `None`
    /// (session_env then uses its built-in model defaults, as the TUI).
    pub(crate) async fn config(&self, name: &str, profile: &Profile) -> Option<ConfigResponse>;
}
pub(crate) fn window_param(window: &str) -> Result<&'static str, String>; // "24h"|"7d"|"30d" via stats_view::Window::param, else "bad request: window"
pub(crate) async fn stats(profile: &str, window: &str) -> Result<serde_json::Value, String>; // fetch_stats + fetch_pool_health (pool None on failure)
pub(crate) async fn credential(profile: &str, claude_session_id: &str) -> Result<serde_json::Value, String>;
pub(crate) async fn switch(profile: &str, claude_session_id: &str) -> Result<serde_json::Value, String>;

// src/web/plugins.rs — the web's copy of the TUI's plugin state machine.
pub(crate) struct PluginState {
    pub enabled: BTreeMap<String, bool>,               // [extensions]
    pub choices: BTreeMap<String, PluginChoices>,      // [plugins.<name>]
    pub lists: BTreeMap<String, (PluginList, ListState)>,
    pub manifests: HashMap<String, (String, ManifestState)>,
    pub applies: HashMap<(String, String), Apply>,
    pub hosts: BTreeSet<String>,
}
#[derive(Clone)]
pub(crate) struct Plugins { /* Arc<Mutex<PluginState>>, HubHandle, Arc<Mutex<Registry>> */ }
impl Plugins {
    /// Start from plugins.json's lists and config.toml; spawns the list
    /// refresher (now, then every REFRESH_SECS), the manifest fetcher, the
    /// HubEvent listener (HostUp: ask that host for every requestable
    /// enabled manifest; HostDown: forget its applies; ClaudeInstalled: ask
    /// it again for all) and a 5 s re-ask ticker (`reask_after`).
    pub(crate) fn start(hub: HubHandle, registry: Arc<Mutex<Registry>>, cached: Vec<PluginList>, enabled: BTreeMap<String, bool>, choices: BTreeMap<String, PluginChoices>) -> Plugins;
    /// What `profile`'s enabled plugins bring to a spawn on `host`, in
    /// registry order, minus `skip` and minus plugins without saved choices.
    /// Waits ≤ 20 s for this host's pending `Manifest` answers (sending a
    /// `notice` "applying plugin …" when it has to wait).
    pub(crate) async fn parts(&self, host: &str, profile: &str, skip: &[String]) -> Vec<PluginPart>;
    pub(crate) fn list(&self) -> serde_json::Value;                       // plugins.list
    pub(crate) fn items(&self, name: &str) -> Result<serde_json::Value, String>; // plugins.items
    pub(crate) async fn save(&self, name: &str, disabled: Vec<String>) -> Result<(), String>; // choices_for + save_plugin_choices (spawn_blocking), then ask every host again
    pub(crate) fn status(&self) -> serde_json::Value;                     // plugins.status
    pub(crate) fn set_config(&self, enabled: BTreeMap<String, bool>, choices: BTreeMap<String, PluginChoices>); // config re-read; asks again where choices changed
    pub(crate) fn toggle(&self, name: &str, on: bool);
}

// src/web/ext.rs — the extensions registry, the external extension host, snippets, the config re-read.
pub(crate) struct Extensions { /* Arc<Mutex<Registry>>, Arc<ExtensionHost>, config path, last mtime */ }
impl Extensions {
    /// Registry::new(config.extensions, config.claude.latex, manifest::load(dir) in spawn_blocking);
    /// ExtensionHost::new; its notices become `notice` events.
    pub(crate) async fn load(config: &Config, hub: &HubHandle) -> Arc<Extensions>;
    pub(crate) fn registry(&self) -> Arc<Mutex<Registry>>;
    pub(crate) fn session_extensions(&self) -> SessionExtensions;
    pub(crate) fn list(&self) -> serde_json::Value;                                // Extension DTOs, registry order
    pub(crate) async fn toggle(&self, plugins: &Plugins, hub: &HubHandle, name: &str, on: bool) -> Result<(), String>;
    pub(crate) async fn action(&self, hub: &HubHandle, ext: &str, action: &str, input: Option<String>, sid: Option<SessionId>) -> Result<serde_json::Value, String>;
    pub(crate) async fn render(&self, lens: &str, kind: &str, source: &str) -> Result<serde_json::Value, String>;
    /// Every 5 s: config.toml's mtime (spawn_blocking); on a change, `config::load`
    /// (spawn_blocking) and apply [extensions], [plugins.*], [claude] and the proxy
    /// profiles; then `extensions_changed` (and `profiles` if they changed).
    pub(crate) fn watch_config(self: &Arc<Self>, plugins: Plugins, hub: HubHandle);
    pub(crate) async fn shutdown(&self);                                           // ExtensionHost::shutdown
}
pub(crate) async fn snippets_list() -> Result<serde_json::Value, String>;
pub(crate) async fn snippets_save(text: String) -> Result<serde_json::Value, String>; // ≤ 64 KiB

// src/web/spawn.rs — replaces Task 4's minimal version (same request types).
pub(crate) struct SpawnCtx { pub hub: HubHandle, pub proxy: ProxyCache, pub plugins: Plugins, pub ext: Arc<Extensions> }
/// default → `default` (the `[proxy] default`, or "env" with CLAUDIO_PROXY_URL); direct → none; profile → its name.
pub(crate) fn pick_profile(arg: &ProxyArg, default: Option<&str>) -> Option<String>;
pub(crate) async fn spawn_claude(ctx: &SpawnCtx, req: SpawnReq) -> Result<SessionId, String>;
pub(crate) async fn spawn_shell(ctx: &SpawnCtx, host: String, cwd: String, name: Option<String>, rows: u16, cols: u16) -> Result<SessionId, String>;
pub(crate) async fn respawn(ctx: &SpawnCtx, req: RespawnReq) -> Result<SessionId, String>;
```
Rules:
1. `spawn_claude`: `check_spawn`; the host's client (`hub.host_client`); `profile = pick_profile(&req.proxy, default)` with `default` from `load_proxy_profiles()` (spawn_blocking); `first_party` without a profile → `"first-party needs a proxy profile"`; `parts = plugins.parts(host, profile, skip_plugins)` (none without a profile); `composed = compose(&parts)` (its notices → `notice` events, which name plugins, never values); `SpawnSpec { id: new v4, cwd, name, args: ["--resume", id] when resuming, env: vec![], rows, cols, ephemeral: false, extensions: Some(ext.session_extensions()) }`; proxy env `proxy_env_for(profile, cache.config(..), first_party)` in spawn_blocking (it reads config.toml); `msg = with_plugins(Msg::Spawn(spec), composed).merge_env(env)` exactly as `send_with_proxy` (src/tui/sessions.rs:465); `client.request(msg)` → `Spawned { id }`; `hub.spawned(host, id, SessionMeta { host, proxy: profile, first_party }, prompt)`; log `op=session.spawn host sid`.
2. `respawn`: `hub.session(sid)`; a shell → `Msg::Respawn` without env or extensions; a claude session → profile from `req.proxy` (with `req.first_party`), else the known one; nothing known → `"proxy unknown"`; then the same plugins, extensions, env and `with_plugins`/`merge_env` as a spawn on `RespawnSpec { id, fresh, env: vec![], rows, cols, extensions: Some(..) }`; `hub.spawned(..)` records what was used.
3. Plugins: as the TUI (src/tui/plugins.rs:235–700): a host is asked for a plugin's manifest only when the manifest is fetched at the list's hash and its choices are saved; the `Manifest` request carries secret names and disabled ids only; answers kept per (host, plugin) as `Apply`; re-asked every 30 s while `installing` or after a retry is due (`reask_after`); a `TooOld` answer when the daemon says unsupported op; every change sends `plugin_status` (shape in docs/web-protocol.md §6, `retry_secs` from `proto::retry_secs`). Lists: plugins.json read at start (`load_all`, spawn_blocking), fetched at start and every 15 min, saved with `save_cache` on success, dropped with `remove_cache` on 401/403 (notice "plugins of <profile>: signed out, run `claudio proxy login`"); the registry's plugin rows follow (`Registry::set_plugins`). Manifests: `fetch_manifest` then `Manifest::parse_named`, in memory only.
4. Extensions: `extensions.toggle` → `registry.set_enabled`; an external one → `host.set_enabled(name, on).await`; a plugin → `plugins.toggle` (asks hosts when turned on); then `config::save_extension(&paths::config_file(), name, on)` in spawn_blocking (an error → its text), `extensions_changed` to every tab. `extensions.action` → only `Source::External` (`"not an external extension"` otherwise); the session context from `hub.session(sid)` (`host`, `cwd`, `name`, `claude_session_id`); the result → ActionResult DTO (docs/web-protocol.md §4); log `op=extensions.action ext action sid`. `extensions.render` → `ExtensionHost::render(lens, kind, source)` → `{inline, block}` (block = the `Laid` rows), both null when there is none.
5. Config re-read (5 s poll): `[extensions]` → `set_enabled` for each listed name (and latex off when `[claude] latex = false`); `[plugins.*]` → `plugins.set_config`; `[proxy]` → `load_proxy_profiles` → `hub.set_profiles` (a `profiles` event when changed); `[claude]` → the hub's install/update policy (`hub.set_claude_policy`).
6. Never serialize `Profile`, `Plugin`, `PluginList`, `Applied`, `Composed`, `PluginPart`, `SpawnSpec`, `RespawnSpec`: DTOs pick fields (`Applied` → only `installed`, `present`, `skipped`, `dropped`; a `Plugin` → `name`, `version`, never `env` or the bundle URL).

`src/web/ws.rs` dispatch gains: `session.spawn`/`session.respawn`/`session.shell` through `SpawnCtx`, `proxy.stats|credential|switch` (credential/switch use the session's known proxy and `claude_session_id`: `"no proxy for this session"`, `"no conversation yet"`), `snippets.list|save`, `extensions.list|toggle|action|render`, `plugins.list|items|save|status`. `src/web/mod.rs`: `AppState.ctx: Arc<SpawnCtx>`; `serve` builds `Extensions`, `Plugins` (after the hub), starts `watch_config`, and calls `ext.shutdown()` on exit.

Step 1: write the failing tests.

Unit tests:
- `spawn.rs`: `pick_profile_follows_the_wizard` (Default + Some("work") → Some("work"); Default + None → None; Direct + Some("work") → None; Profile{lab} → Some("lab")).
- `proxy.rs`: `windows_are_checked` (24h, 7d, 30d accepted; 1h, "" refused); `stats_dto_has_the_documented_fields` (from a hand-built `StatsResponse`: keys `profile, window, user_name, totals, by_model, limit, pool`); `credential_dto_never_has_a_token` (the DTO of a `SessionCredential` has exactly `name, plan, provider, five_hour_pct, seven_day_pct, switched_secs_ago`).
- `plugins.rs`: `items_mark_disabled_ids_unchecked` (a parsed test manifest with `mcp:kb` disabled → that item `checked: false`, the others true; no choices → all true); `status_dto_carries_retry_seconds` (`skipped` reason "down (retrying in 15 s)" → `retry_secs: 15`); `list_dto_has_no_env` (a `Plugin` with an env value: the DTO's JSON does not contain it).

Integration tests (`tests/web.rs`). The fake proxy and the env-dumping fake claude:
```rust
const PLUGIN: &str = "vocdoni-test";
const PROXY_TOKEN: &str = "tok-SECRET-proxy-7f3a9c";
const PLUGIN_SECRET: &str = "plugin-SECRET-value-51d2e8";
const MANIFEST: &str = r#"format = 1
name = "vocdoni-test"
version = "1.0.0"
description = "Test setup"
instructions = "Manifest instructions for the test."

[[mcp]]
name = "kb"
type = "http"
url = "https://kb.example.org/mcp"
headers = { Authorization = "Bearer ${VOCDONI_TEST_TOKEN}" }
secrets = ["VOCDONI_TEST_TOKEN"]

[[agent]]
name = "reviewer"
description = "Reviews"
prompt = "Review the change."
"#;

/// A fake claude-proxy on loopback: `/v1/claudio/config`, `/plugins`
/// (PLUGIN, whose env holds PLUGIN_SECRET), the manifest, `/me/stats`
/// (any period) and `/pool/health`; anything else 404. Its `host:port`.
fn fake_proxy() -> String {
    use sha2::Digest;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind fake proxy");
    let addr = listener.local_addr().unwrap().to_string();
    let sha = format!("{:x}", sha2::Sha256::digest(MANIFEST.as_bytes()));
    let plugins = json!({"plugins": [{
        "name": PLUGIN, "version": "1.0.0",
        "bundle": {"url": format!("http://{addr}/{PLUGIN}.toml"), "sha256": sha},
        "env": {"VOCDONI_TEST_TOKEN": PLUGIN_SECRET}}]})
    .to_string();
    let stats = json!({"version": 1, "user_name": "tester", "period": "24h",
        "totals": {"requests": 12, "errors": 1, "input_tokens": 3400, "output_tokens": 5600, "cache_read": 0, "cache_creation": 0},
        "by_model": [{"model": "claude-opus-5-5", "requests": 12, "errors": 1, "input_tokens": 3400, "output_tokens": 5600, "cache_read": 0, "cache_creation": 0}],
        "limit": null})
    .to_string();
    let config = json!({"version": 1, "env": {}}).to_string();
    let pool = json!({"version": 1, "providers": [{"name": "anthropic", "status": "ok"}]}).to_string();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap_or(0) == 1 {
                head.push(byte[0]);
            }
            let line = String::from_utf8_lossy(&head).into_owned();
            let path = line.split(' ').nth(1).unwrap_or("").split('?').next().unwrap_or("");
            let (status, reply) = match path {
                "/v1/claudio/config" => ("200 OK", config.as_str()),
                "/v1/claudio/plugins" => ("200 OK", plugins.as_str()),
                "/v1/claudio/me/stats" => ("200 OK", stats.as_str()),
                "/v1/claudio/pool/health" => ("200 OK", pool.as_str()),
                p if p == format!("/{PLUGIN}.toml") => ("200 OK", MANIFEST),
                _ => ("404 Not Found", "{}"),
            };
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                reply.len()
            );
        }
    });
    addr
}

/// A fake claude that dumps its env to `env_file` (never to the terminal),
/// lists `--agents`/`--mcp-config` in `--help`, then prints the banner and
/// echoes lines.
fn write_env_fake_claude(h: &ManagerHarness, env_file: &std::path::Path) {
    let env_path = env_file.display();
    let script = format!(
        r#"#!/bin/sh
[ "$1" = "--version" ] && echo 'claude 0.0.0-fake' && exit 0
if [ "$1" = "--help" ]; then printf '  --agents <json>  Agents\n  --mcp-config <configs...>  MCP\n'; exit 0; fi
env > "{env_path}.tmp" && mv "{env_path}.tmp" "{env_path}"
printf '%s' "$2" > .fake-settings.json
echo "FAKE_CLAUDE_BANNER pid=$$"
while IFS= read -r line; do printf 'line:%s\n' "$line"; done
"#
    );
    fs::write(&h.fake_claude, script).unwrap();
    fs::set_permissions(&h.fake_claude, fs::Permissions::from_mode(0o755)).unwrap();
}

/// Poll `op` until `pick` accepts its data.
async fn until_req(tab: &mut Tab, op: &str, args: Value, pick: impl Fn(&Value) -> bool) -> Value {
    let deadline = std::time::Instant::now() + DAEMON_WAIT;
    loop {
        if let Ok(data) = tab.req(op, args.clone()).await {
            if pick(&data) {
                return data;
            }
        }
        assert!(std::time::Instant::now() < deadline, "{op} never satisfied");
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}
```
AC3, in full:
```rust
/// AC3: the proxy token and a plugin's secret reach claude's env and never
/// the browser (no WS frame, no HTTP body) nor the server's log.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_spawn_keeps_proxy_and_plugin_secrets_out_of_the_browser() {
    let h = ManagerHarness::new();
    let env_file = h.root.join("claude-env.txt");
    write_env_fake_claude(&h, &env_file);
    let config = h.config_home.join("claudio/config.toml");
    let mut text = fs::read_to_string(&config).unwrap();
    text.push_str(&format!("\n[plugins.{PLUGIN}]\ndisabled = []\nreviewed = \"1.0.0\"\n"));
    fs::write(&config, text).unwrap();
    let proxy = fake_proxy();
    let dir = h.dirs[0].clone();
    let web = Web::start_with(h, &[], |cmd| {
        cmd.env("CLAUDIO_PROXY_URL", format!("{PROXY_TOKEN}@{proxy}"));
        cmd.env("CLAUDIO_TEST_LOOPBACK_HTTP", "1");
    });
    let cookie = web.login();
    let mut tab = Tab::open(&web, &cookie).await;
    assert_eq!(tab.snapshot["profiles"]["default"], "env");
    tab.connected("local").await;
    until_req(&mut tab, "plugins.status", json!({}), |d| {
        d["status"].as_array().is_some_and(|s| {
            s.iter().any(|p| p["host"] == "local" && p["name"] == PLUGIN && p["state"] == "ready")
        })
    })
    .await;
    let reply = tab
        .req(
            "session.spawn",
            json!({"host": "local", "cwd": dir, "rows": 24, "cols": 80, "proxy": {"kind": "default"}}),
        )
        .await
        .expect("spawn");
    let sid = reply["sid"].as_str().unwrap().to_owned();
    let deadline = std::time::Instant::now() + WAIT;
    let env = loop {
        if let Ok(env) = fs::read_to_string(&env_file) {
            break env;
        }
        assert!(std::time::Instant::now() < deadline, "claude never started");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };
    assert!(env.contains(&format!("ANTHROPIC_AUTH_TOKEN={PROXY_TOKEN}")), "the proxy token reaches claude");
    assert!(env.contains(&format!("VOCDONI_TEST_TOKEN={PLUGIN_SECRET}")), "the plugin secret reaches claude");

    // Everything a browser can see about this session and its plugins.
    tab.req("session.attach", json!({"sid": sid, "rows": 24, "cols": 80})).await.unwrap();
    tab.output_until(&sid, "FAKE_CLAUDE_BANNER").await;
    for (op, args) in [
        ("hosts.list", json!({})),
        ("plugins.list", json!({})),
        ("plugins.items", json!({"name": PLUGIN})),
        ("plugins.status", json!({})),
        ("extensions.list", json!({})),
        ("proxy.stats", json!({"profile": "env", "window": "24h"})),
    ] {
        tab.req(op, args).await.unwrap_or_else(|e| panic!("{op}: {e}"));
    }
    let _ = tab.req("proxy.credential", json!({"sid": sid})).await; // no conversation yet: an error
    let second = Tab::open(&web, &cookie).await; // a snapshot with the session in it
    for frame in tab.seen.iter().chain(second.seen.iter()) {
        let text = String::from_utf8_lossy(frame);
        assert!(!text.contains(PROXY_TOKEN), "the proxy token in a frame: {text}");
        assert!(!text.contains(PLUGIN_SECRET), "the plugin secret in a frame: {text}");
    }
    for path in ["/", "/index.html"] {
        let body = web.get(path, &[("Accept-Encoding", "identity")]).body;
        assert!(!body.contains(PROXY_TOKEN) && !body.contains(PLUGIN_SECRET));
    }
    let log = fs::read_to_string(web.h().root.join("web.log")).unwrap();
    assert!(!log.contains(PROXY_TOKEN) && !log.contains(PLUGIN_SECRET), "a secret in the server log");
}
```
The other integration tests (names and assertions binding):
- `web_respawn_without_a_known_proxy_asks`: spawn a direct session; stop the server; delete `web-state.json`; start again (no state.json either) → `session.respawn {sid, fresh: false, rows: 24, cols: 80}` → `"proxy unknown"`; with `proxy: {kind: "direct"}` → `{sid}` and a `session` event for it; `web-state.json` now records it (`proxy: null`).
- `web_plugin_without_choices_is_left_out_until_saved`: the fake proxy without the saved `[plugins.vocdoni-test]`; `plugins.list` → `choices_saved: false`; a `default` spawn → claude's env has the proxy token and not `PLUGIN_SECRET`; `plugins.save {name, disabled: ["agent:reviewer"]}` → config.toml contains `[plugins.vocdoni-test]` with `disabled = ["agent:reviewer"]`; `plugins.status` becomes ready on `local`; the next spawn's env has `PLUGIN_SECRET`; a spawn with `skip_plugins: ["vocdoni-test"]` has not.
- `web_extension_toggle_edits_config_and_is_reread`: `extensions.toggle {name: "diagram", on: false}` → config.toml has `diagram = false` under `[extensions]`, an `extensions_changed` event, `extensions.list` shows it off; rewrite the line to `diagram = true` by hand → within 7 s an `extensions_changed` event and `extensions.list` shows it on.
- `web_snippets_save_and_list`: `snippets.save {text: "run the tests"}` → `{name}`; `snippets.list` contains it; a file exists under `config_home/claudio/snippets/`.
- `web_external_extension_action_runs_its_program`: an external extension under `config_home/claudio/extensions/hello/extension.toml` with one action, its program answering `{"kind":"notice","text":"hi"}` (manifest and program shaped like the unit tests of src/tui/extensions/host.rs and manifest.rs); `extensions.action {ext: "hello", action: <id>}` → `{result: {kind: "notice", text: "hi"}}`; `extensions.action {ext: "search", action: "search"}` → `"not an external extension"`.
- `web_proxy_stats_render_the_fixture`: `proxy.stats {profile: "env", window: "7d"}` → `user_name: "tester"`, `totals.output_tokens: 5600`, `by_model[0].model: "claude-opus-5-5"`, `pool[0].status: "ok"`; `window: "1h"` → `"bad request: window"`.

Run: `cargo test --locked --bin claudio web::` → `0 failed`
Run: `cargo test --locked --test web -- --test-threads=1` → `0 failed`
Run: `make test-unit` → `0 failed`; `make test-manager` → `0 failed`; `cargo build --release --locked` → builds

Step 3: commit
`git commit -m "feat(web): spawn with proxy env, plugins and extensions as the TUI does" -- src/web/spawn.rs src/web/proxy.rs src/web/plugins.rs src/web/ext.rs src/web/ws.rs src/web/mod.rs src/web/hub.rs src/tui/mod.rs src/tui/plugins.rs tests/web.rs`

Interfaces this task provides to later tasks: the full op set of docs/web-protocol.md §5 on the server; `spawn::{SpawnCtx, pick_profile}`, `plugins::Plugins`, `ext::Extensions`, `proxy::{ProxyCache, stats, credential, switch}`; tests/web.rs `fake_proxy`, `write_env_fake_claude`, `until_req` (Task 11's fake proxy mirrors `fake_proxy`'s routes and bodies).

### Task 6: Vendoring and the asset budget
Model: fast   Repo: /volumes/repos/claudio   Files: web/vendor/** (new), web/fonts/** (new), web/vendor/MANIFEST.txt (new), web/vendor/LICENSES.md (new), web/index.html (new: the head with the import map; Task 7 completes it)
Goal: the pinned third-party files the app loads (spec §The browser app), their licenses, the import map, and the gzipped-size measurement that decides the mermaid reduction rule (AC11's budget).
Depends on: none (frontend lane start: feat/web-ui at the commit that adds this plan)
Patch: none

Downloads are untrusted data: each package goes into its own new, empty directory under `~/.cache/claudio/web-ui/vendor-src/<package>@<version>/` (never /tmp, never the repo), fetched with `npm pack <package>@<version> --pack-destination <that dir>` and unpacked there with `tar -xzf <tgz> -C <that dir>`. Do not run any script from a package. Copy files byte for byte.

Step 1: fetch and copy (paths are inside each tarball's `package/`):
| Package (pinned) | Files → destination |
|---|---|
| `preact@11.2.0` | the ESM files named by its package.json `exports["."]` and `exports["./hooks"]` (`import`/`module` condition) → `web/vendor/preact/` and `web/vendor/preact/hooks/`, upstream file names; `LICENSE` |
| `htm@3.1.1` | `dist/htm.module.js` → `web/vendor/htm/htm.module.js`; `preact/index.module.js` → `web/vendor/htm/preact/index.module.js` |
| `@preact/signals@2.11.4` | its ESM entry (`exports["."].import`/`module`) → `web/vendor/signals/` |
| `@preact/signals-core@<the newest version satisfying @preact/signals 2.11.4's dependency range>` | its ESM entry → `web/vendor/signals-core/` |
| `@xterm/xterm@6.0.0` | `lib/xterm.mjs`, `css/xterm.css` → `web/vendor/xterm/` |
| `@xterm/addon-fit`, `@xterm/addon-unicode11`, `@xterm/addon-web-links`, `@xterm/addon-search` at the newest versions whose `peerDependencies["@xterm/xterm"]` accepts 6.0.0 (check with `npm view <pkg>@<ver> peerDependencies`; expected 0.11.0, 0.9.0, 0.12.0, 0.16.0) | `lib/addon-*.mjs` → `web/vendor/xterm/` |
| `katex@0.19.1` | `dist/katex.min.js`, `dist/katex.min.css` → `web/vendor/katex/`; `dist/fonts/*.woff2` only → `web/vendor/katex/fonts/` (the CSS's ttf/woff `url()`s 404 harmlessly; never edit the CSS) |
| `mermaid@12.3.0` | `dist/mermaid.esm.min.mjs` → `web/vendor/mermaid/`; `dist/chunks/mermaid.esm.min/*.mjs` → `web/vendor/mermaid/chunks/mermaid.esm.min/` (same relative layout as upstream) |
| `marked@18.1.3` | `lib/marked.esm.js` → `web/vendor/marked/` |
| `dompurify@3.4.17` | `dist/purify.es.mjs` → `web/vendor/dompurify/` |
| `@highlightjs/cdn-assets@11.12.0` (highlight.js 11.12.0's prebuilt release: the common languages in one ES module) | `es/highlight.min.js`, `styles/github-dark.min.css`, `styles/github.min.css` → `web/vendor/highlight/` |
| Fonts (OFL-1.1), from the upstream GitHub release archives, each into its own download dir | JetBrains Mono 2.304 `JetBrainsMono-Regular.woff2`, `JetBrainsMono-Bold.woff2`; Inter 4.1 `Inter-Regular.woff2`, `Inter-SemiBold.woff2`; Departure Mono 1.500 `DepartureMono-Regular.woff2` → `web/fonts/` |

If a pinned package names a file differently than the table says, take the file its package.json points to and record it; do not substitute another version.

Step 2: `web/index.html` (head only; Task 7 adds the body):
```html
<!doctype html>
<html lang="en" data-theme="arcade-dark">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>claudio</title>
<script type="importmap">
{"imports": {
  "preact": "/vendor/preact/<entry>",
  "preact/hooks": "/vendor/preact/hooks/<entry>",
  "htm": "/vendor/htm/htm.module.js",
  "htm/preact": "/vendor/htm/preact/index.module.js",
  "@preact/signals-core": "/vendor/signals-core/<entry>",
  "@preact/signals": "/vendor/signals/<entry>",
  "@xterm/xterm": "/vendor/xterm/xterm.mjs",
  "@xterm/addon-fit": "/vendor/xterm/addon-fit.mjs",
  "@xterm/addon-unicode11": "/vendor/xterm/addon-unicode11.mjs",
  "@xterm/addon-web-links": "/vendor/xterm/addon-web-links.mjs",
  "@xterm/addon-search": "/vendor/xterm/addon-search.mjs",
  "marked": "/vendor/marked/marked.esm.js",
  "dompurify": "/vendor/dompurify/purify.es.mjs",
  "highlight.js": "/vendor/highlight/highlight.min.js"
}}
</script>
</head>
<body></body>
</html>
```
Then check that every bare specifier any vendored module imports is mapped: `grep -rhoE "(from|import)\s*\(?\s*[\"'][^\"'./][^\"']*[\"']" web/vendor | sed -E "s/.*[\"']([^\"']+)[\"'].*/\1/" | sort -u` must list only keys of the map. KaTeX (`katex.min.js`, UMD) is loaded later by a `<script src>` it adds itself (Task 9), not through the map. The map's text is hashed into the CSP by build.rs: keep it byte-stable once written.

Step 3: `web/vendor/MANIFEST.txt`: `#` comment header, then one line per file under `web/vendor/` and `web/fonts/`: `<path relative to web/>  <package>@<version>  <source URL>#<path inside the archive>  sha256:<hex>` (registry tarball URL `https://registry.npmjs.org/<pkg>/-/<name>-<ver>.tgz`, or the release asset URL). `web/vendor/LICENSES.md`: per package and font: name, version, SPDX id, the full LICENSE/OFL text copied from the archive.

Step 4: measure. Run from the repo root:
```sh
find web -type f -not -path 'web/tests/*' -not -path 'web/e2e/*' -not -path 'web/dev/*' -not -path '*/node_modules/*' -not -name '.*' -print0 \
  | xargs -0 -n1 sh -c 'gzip -9c "$0" | wc -c' | awk '{s+=$1} END {print s}'
```
Expected: ≤ 3670016 (3.5 MiB). Record the number as `# gzipped total: <n> bytes (budget 3670016)` in MANIFEST.txt. Over budget → the mermaid reduction rule: delete (never edit) the chunk files of diagram types other than flowchart, sequence, class, state, er, gantt and pie (files whose names start with another diagram's name, e.g. `mindmap-definition-`, `gitGraphDiagram-`, `c4Diagram-`, `quadrantDiagram-`, `xychartDiagram-`, `sankeyDiagram-`, `timeline-definition-`, `requirementDiagram-`, `blockDiagram-`, `architectureDiagram-`, `kanban-definition-`, `journeyDiagram-`, `packet`, `radar`, `treemap`, `infoDiagram-`), after checking with grep that no kept file imports a deleted one; remove their MANIFEST lines, add `# mermaid reduced: <list>` and measure again. Task 9 renders a missing type as a code block with a notice.

Step 5: verify the files: `sha256sum` of each listed file matches its MANIFEST line (a short `awk`/`sha256sum -c` loop); `node --input-type=module -e "await import('./web/vendor/marked/marked.esm.js'); await import('./web/vendor/dompurify/purify.es.mjs')"` exits 0.

Step 6: commit
`git commit -m "chore(web): vendor the pinned browser libraries and fonts" -- web/vendor web/fonts web/index.html`

Interfaces this task provides to later tasks: the import-map specifiers above; `/vendor/katex/katex.min.js` (sets `window.katex`), `/vendor/katex/katex.min.css`, `/vendor/mermaid/mermaid.esm.min.mjs` (default export `mermaid`), `/vendor/xterm/xterm.css`, `/vendor/highlight/styles/github{,-dark}.min.css`, `/fonts/*.woff2`; the measured gzipped total.

### Task 7: App foundation, design system and mock server
Model: standard   Repo: /volumes/repos/claudio   Files: web/index.html, web/app/{boot,main,proto,model,store,keymap,fuzzy,palette,settings,theme,lazy}.js, web/app/ui/{app,topbar,toasts,help,settings,palette,modal}.js, web/styles/{tokens,base,layout,components}.css, web/dev/mock-server.mjs, web/e2e/package.json, web/e2e/package-lock.json, web/.gitignore, web/tests/{proto,model,keymap,fuzzy}.test.mjs
Goal: AC9 (protocol client, keymap/focus model), and the shell every later UI task fills: login, a reconnecting WS client, a signals store fed by snapshot and events, themes, focus model, palette, toasts, help, settings; a node mock server of docs/web-protocol.md for development.
Depends on: Task 6
Patch: none

Module interfaces (exact exports; pure modules import nothing through bare specifiers):
```js
// web/app/proto.js — pure apart from the injected WebSocket and timers
export const PROTO_VERSION = 1;
export const KIND_OUTPUT = 0x01, KIND_INPUT = 0x02;
export const MAX_INPUT = 64 * 1024;
export function uuidToBytes(id)            // "xxxxxxxx-…" → Uint8Array(16)
export function bytesToUuid(u8)            // Uint8Array(16) → lowercase uuid
export function encodeInput(id, data)      // data: string|Uint8Array → Uint8Array[] (frames ≤ 17 + 64 KiB each)
export function decodeOutput(buf)          // ArrayBuffer → {sid, bytes} | null (kind ≠ 0x01 or < 17 bytes)
export function backoffDelay(attempt, rand = Math.random) // 1000·2^attempt capped 30000, ±20 %
export function versionAction(v, reloadedBefore)           // 'ok' | 'reload' | 'message'
export class Connection {
  constructor({ url, WebSocketImpl = globalThis.WebSocket, setTimeout = globalThis.setTimeout,
                clearTimeout = globalThis.clearTimeout, rand = Math.random,
                onHello, onSnapshot, onEvent, onOutput, onStatus })
  start(); stop();
  get status()                              // 'connecting' | 'open' | 'closed'
  request(op, args = {}, { timeoutMs = 30000 } = {}) // Promise<data>; rejects Error(error string), Error('disconnected'), Error('timeout')
  sendInput(sid, data)
}
// web/app/model.js — pure reducers over plain objects
export function emptyModel()  // {hosts: {}, sessions: {}, order: [], layout, profiles: {names: [], default: null}, lens: {}, stats: {}, claude: {}, plugins: {}}
export function applySnapshot(model, snap)
export function applyEvent(model, ev)       // every event of docs/web-protocol.md §6; lens seq rule of §6
export function orderedSessions(model)      // model.order first, then the rest by created_at
export function groupSessions(sessions)     // [{host, groups: [{cwd, label, sessions}]}], host "local" first
// web/app/store.js — @preact/signals around model.js
export const model   // signal(emptyModel())
export const conn    // signal('connecting'|'open'|'closed')
export const mode    // signal('terminal'|'app')
export function connect()                   // builds the Connection to ws(s)://location.host/ws
export function req(op, args)               // Connection.request + an error toast on failure (returns the promise)
export function onOutput(sid, fn)           // subscribe a pane's writer; returns unsubscribe
// web/app/keymap.js — pure
export const BINDINGS   // [{keys: 'k', action: 'palette', help: 'command palette'}, …] — the help overlay's single source
export function isLeaveTerminal(ev)        // Ctrl+Shift+Space only
export function appAction(ev, ctx)         // ctx: {typing: bool, overlay: bool} → action name | null
export function forbidden(ev)              // true for Alt+letter, Ctrl+W/T/N/Tab (never bound)
// web/app/fuzzy.js — pure
export function score(query, text)         // subsequence match → number (higher better) | null
export function filter(items, query, key = (x) => x) // sorted by score, ties keep input order
```
App-mode bindings (`appAction`), only when no input/textarea has focus (`ctx.typing`) and in app mode: `k` or `Ctrl+K` → `palette`; `a` → `next-attention`; `n` → `new-session`; `t` → `new-terminal`; `1`–`4` → `focus-pane-1…4`; `ArrowDown`/`ArrowUp` → `rail-next`/`rail-prev`; `r` → `reader`; `g` → `changes`; `?` → `help`; `Enter` → `enter-terminal`; `Escape` → `close-overlay`. In terminal mode the app intercepts nothing but `Ctrl+Shift+Space` (→ app mode); clicking outside the terminal also enters app mode; the mode badge (`data-testid="mode-badge"`) shows `TERM` or `APP`. The palette lists every action of BINDINGS plus every session (go to, kill, restart, rename, reader, changes, stats, switch subscription) and settings/theme; later tasks register their commands through `palette.js`'s `register(command)` (`{id, title, keys?, run, when?}`).

Rules:
- `boot.js`: first thing, `document.addEventListener('securitypolicyviolation', e => (window.__claudioCsp ||= []).push(e.violatedDirective))`. If `location.hash` starts with `#t=`: POST it to `/api/login` (JSON, same-origin), then `history.replaceState(null, '', location.pathname)`; 204 → start; 401 → message "This link's token is not valid. Run `claudio web --print-url`."; 429 → "Too many attempts: wait a minute.". Without a token: `GET /api/me`; 204 → start, else the same "open the URL `claudio web` printed" message. Start = `import('./main.js')`.
- Version: `hello.v` ≠ 1 → `versionAction` with `sessionStorage['claudio.reloaded']`: reload once, then show "server updated, reload the page" without reloading; a matching hello clears the flag.
- Reconnect: on close, pending requests reject with `disconnected`, the status turns `closed`, a reconnect follows `backoffDelay` (attempt reset on `hello`); the store resets from the next snapshot; panes re-attach (Task 8).
- Themes: `data-theme` on `<html>`: `arcade-dark` (default), `arcade-light`, `plain-dark`, `plain-light`. Tokens (tokens.css), dark arcade: background `#0b0f14`, panel `#121821`, border `#1f2a37`, text `#e6edf3`, muted `#8b98a5`; state colours and glyphs as the TUI (src/tui/ui/mod.rs:119): working cyan `#22d3ee` braille spinner `⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏`, needs approval red `#ff4d4f` `◆`, needs input amber `#f5b32b` `?`, idle green `#3ddc84` `✓`, error `#ff2e63` `✗`, exited muted `○`, starting/unknown `·`, reconnecting steel `#5c6b7a` `⇄`, terminal tab bold green `$`. Light variants keep text contrast ≥ 4.5:1. Arcade: 2 px borders, 2 px radii, one hard offset shadow on popovers only, the pixel face (Departure Mono) for logo, headers and badges only, JetBrains Mono for UI and terminals, Inter for reader prose (Task 9), optional 3 % scanline overlay on the board only (never terminals or the reader). Plain: no pixel face, no scanlines. `prefers-reduced-motion` stops spinners and the one-time attention blink. Every state shows its glyph as well as its colour. Visible focus rings everywhere.
- Settings (`localStorage['claudio.settings']`): theme family, light/dark, scanlines, terminal font size, sound (off), notifications (off until enabled from a user gesture).
- No inline script, no `on*=` attributes, no `innerHTML` with server data in this task (text nodes only).

`web/dev/mock-server.mjs`: `node web/dev/mock-server.mjs` serves `web/` on `127.0.0.1:${MOCK_PORT ?? 7338}` with the same CSP as the server (import-map hash computed like build.rs), prints `mock: http://127.0.0.1:7338/#t=dev`, accepts token `dev`, implements `/api/login|logout|me` and the WS protocol v1 with fixture data: hosts `local` (all features) and `z6` (no `attention` feature), six sessions covering `needs_approval` (permission_prompt with tool Bash and detail), `needs_approval` (elicitation_dialog), `needs_input`, `working`, `idle`, a shell; `session.attach` sends `attached` then a banner frame and echoes `0x02` input back as output; every op of docs/web-protocol.md §5 answers fixture data of the documented shape (Tasks 8–10 extend fixtures as they need them). `ws` is required through `createRequire(new URL('../e2e/package.json', import.meta.url))('ws')`.
`web/e2e/package.json`: `{"private": true, "type": "module", "devDependencies": {"@playwright/test": "1.64.1", "ws": "<current 8.x, exact>"}}` (1.64.1 matches the cached chromium-1248 in ~/.cache/ms-playwright); create the lock with `cd web/e2e && PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 npm install`. `web/.gitignore`: `e2e/node_modules/`, `e2e/test-results/`, `e2e/playwright-report/`.

Step 1: write the failing tests (`node:test` + `node:assert/strict`; a fake WebSocket class in the test drives `Connection`):
- `web/tests/proto.test.mjs`: `uuid round-trips through 16 bytes`; `encodeInput chunks at 64 KiB with kind 0x02 and the id` (a 150 000-byte input → 3 frames, each starting `0x02` + id, payload sizes 65536/65536/18928); `decodeOutput reads kind 0x01 only` (0x02 or 16-byte buffers → null); `backoff doubles from 1 s to 30 s within ±20 %` (attempts 0..6 with rand 0 and 1); `request resolves on ok and rejects with the error string` (fake socket answers `{"t":"res","id":1,"ok":false,"error":"stale"}` → rejects `Error('stale')`); `pending requests reject with "disconnected" and the client reconnects after backoff` (fake timers); `versionAction reloads once then shows the message`; `hello then snapshot reach their callbacks in order`.
- `web/tests/model.test.mjs`: `snapshot replaces hosts, sessions, order and layout`; `session upserts and session_removed deletes`; `host_removed drops the host`; `lens keeps the higher seq and attached resets it`; `orderedSessions follows order then created_at`; `groupSessions puts local first and groups by cwd`.
- `web/tests/keymap.test.mjs`: `only Ctrl+Shift+Space leaves the terminal`; `app mode maps k, Ctrl+K, a, n, t, 1-4, r, g, ?, Enter, Escape`; `typing in an input suppresses single-key actions`; `no binding is forbidden` (every BINDINGS entry fails `forbidden`, and `forbidden` is true for Alt+a, Ctrl+W, Ctrl+T, Ctrl+N, Ctrl+Tab).
- `web/tests/fuzzy.test.mjs`: `subsequences match and contiguous runs rank higher`; `no match is null`; `filter keeps input order on ties`.
Run: `node --test web/tests` → fails (modules missing).

Step 2: implement the modules, styles, shell UI, mock server.
Run: `node --test web/tests` → `# fail 0`
Run: `node web/dev/mock-server.mjs` and load `http://127.0.0.1:7338/#t=dev` in Chromium (Playwright from web/e2e, or the playwright MCP if available): the shell renders, the fragment is gone, the mode badge toggles with Ctrl+Shift+Space, `k` opens the palette, `?` the help, the theme setting switches `data-theme`, `window.__claudioCsp` is empty.

Step 3: commit
`git commit -m "feat(web): app shell, protocol client, store, keymap and mock server" -- web/index.html web/app web/styles web/dev web/e2e/package.json web/e2e/package-lock.json web/.gitignore web/tests`

Interfaces this task provides to later tasks: the exports above; `ui/modal.js` `confirm({title, body, yes, no}) → Promise<bool>` (buttons `confirm-yes`/`confirm-no`); `ui/toasts.js` `toast({text, level, actions?: [{label, run}], sticky?})`; `palette.js` `register(command)`; `store.req`, `store.model`, `store.mode`, `store.onOutput`; `lazy.js` `loadScript(src)`, `loadStyle(href)`; the mock server and its fixtures.

### Task 8: Sessions UI: rail, attention queue, board, panes, composer, notifications
Model: standard   Repo: /volumes/repos/claudio   Files: web/app/{queue,notify,panes,gauge,term}.js, web/app/ui/{rail,board,queue,pane,composer,strip,session-actions,narrow}.js, web/app/ui/app.js, web/styles/{layout,components}.css, web/dev/mock-server.mjs, web/tests/{queue,notify,panes,gauge}.test.mjs
Goal: AC9 (queue ordering, notification debounce) and the spec's sessions surface: grouped rail with drag reorder, attention queue with Enter/Esc approve/deny, live board, xterm panes in a 1/2/4 grid with drag-to-pane and the resize rules, composer, status strip, session actions, notifications, narrow layout.
Depends on: Task 7
Patch: none

Pure modules (exact exports):
```js
// web/app/queue.js
export const JUST_IDLE_MS = 10 * 60 * 1000;
export const PERMISSION_KINDS = ['permission_prompt', 'worker_permission_prompt'];
export function rank(session, now)          // 0 needs_approval, 1 needs_input, 2 error, 3 idle within JUST_IDLE_MS of state_since; null otherwise (shells: null)
export function attentionQueue(sessions, now) // ranked ones, by rank then oldest state_since first
export function cardActions(session, host)  // {approve: bool, deny: bool, open: true, warning: string|null}
//   needs_approval + attention.kind ∈ PERMISSION_KINDS → approve/deny; needs_approval + attention null on a host
//   without the 'attention' feature → approve/deny + warning "permission or elicitation: check the terminal";
//   anything else (elicitation, input, error, idle) → Open only
export function approveArgs(session, host, decision) // {sid, decision, attention_seq: seq, or 0 without the feature}
// web/app/notify.js — the debounce of src/tui/notifications.rs
export function makeNotifier()              // {check(sessions, visibleSids) → [{sid, title, body}]}
//   once per (session, state) while the state wants attention (needs_approval, needs_input, error) and the
//   session is in no visible pane; leaving those states re-arms it; body = attention.message (else the state name)
export function titleText(count)            // count > 0 ? `(${count}) claudio` : 'claudio'
// web/app/panes.js
export function emptyLayout()               // {grid: 1, panes: [null], focus: 0}
export function setGrid(layout, grid)       // 1|2|4; keeps panes in order, drops extras, focus clamped
export function dropOnPane(layout, pane, sid) // puts sid there (removing it from another pane)
export function resizePolicy()              // {debounceMs: 250, sendOn: 'own-fit-of-focused-pane', never: 'attached'}
export function shouldSendResize({focused, cause, size, lastSent}) // true only for the focused pane, cause 'fit', size ≠ lastSent
// web/app/gauge.js
export function segments(fraction, n = 8)   // '█'/'░' blocks (8 segments)
export function level(fraction)             // 'ok' < 0.6 ≤ 'warn' < 0.85 ≤ 'hot'
export function ctxFraction(session, model) // context_tokens / window (1M when the model name says [1m], else 200k)
```
UI rules:
- Rail (`data-testid="rail"`): attention queue (`queue`, cards `queue-card` with `data-sid`/`data-kind`) on top, then sessions grouped by host then cwd (collapsible), cards `session-card` with `data-sid`/`data-state`: glyph + label + host chip + branch with dirty counts + ctx gauge + age. Drag a card to reorder → `session.order`; drag onto a pane → that pane shows it. Queue card: label, host, `attention.message` first, then `tool`/`detail` (one line, ellipsis, full text in a tooltip), buttons `open-btn`, and when `cardActions` allows, `approve-btn` labelled `Enter` and `deny-btn` labelled `Esc` (the keys they send). Narrow screens (< 760 px): board and queue only, no panes; Approve/Deny there ask `confirm()` first. A session that newly wants attention blinks twice (`insert coin`), then holds its colour (no blink under reduced motion).
- Board (`board`): the main area when no pane is open: one row per session (6 facts: state, label, host, repo/branch+dirty, model, ctx %, age), scanlines optional (arcade only).
- Panes (`pane` + `data-pane`): grid 1, 2 side by side, or 2×2 (`setGrid`); header with glyph, label, branch, a foreign-size badge when the last `attached` size differs from the pane's fit, and actions (`session-actions`: rename (`rename-input`), kill (`kill-btn`, confirm), restart resume/new (`restart-resume-btn`/`restart-new-btn`, confirm; `"proxy unknown"` → a small chooser default/direct/profile pre-selecting `[proxy] default`, then retry with `proxy`), terminal tab beside it (`terminal-tab-btn` → `session.shell` in the same cwd), Resume for dormant sessions (never automatic)). Layout changes → `layout.save`; `layout` events from other tabs apply.
- Terminal (`term.js`, element `terminal`): xterm.js owned by the pane component through a ref (Preact never re-renders it); addons fit, unicode11 (`allowProposedApi: true`), web-links (handler: `confirm()` then `window.open(url, '_blank', 'noopener')`), search; no clipboard addon (OSC 52 writes are ignored); `linkHandler` for OSC 8 links also asks first; copy-on-select. On mount: `session.attach {sid, rows, cols}` with the fitted size; on `attached`: `term.reset()` then the next output frame is the snapshot; `resize` follows `shouldSendResize` (focused pane, own fit, 250 ms debounce, never in reply to `attached`); input `term.onData`/`onBinary` → `sendInput`; on unmount `session.detach`; on reconnect re-attach.
- Composer (`composer`, claude sessions only): textarea; Ctrl+Enter → `session.paste {sid, text, submit: true}`; Shift+Enter and Enter insert a newline; Esc returns focus to the terminal; disabled while the session is not live.
- Status strip: model, ctx gauge (`segments`, `level`), git counts (`+staged ~unstaged ?untracked !conflicts ↑ahead ↓behind`), output tokens, turns; `prompt_pending` shows "opening prompt pending".
- Notifications: the Notification permission is requested only from a click in Settings; `makeNotifier().check` runs on every model change with the sids visible in panes; each result → `new Notification(title, {body, tag: sid})` whose click focuses the window and opens the session; `document.title = titleText(count)`; optional sound off by default (a short WebAudio blip, no asset).
- `a` (next attention) focuses the first queue card's session in the focused pane.

Step 1: failing tests:
- `web/tests/queue.test.mjs`: `queue orders approval > input > error > just-idle, oldest first`; `idle older than 10 minutes and shells are left out`; `permission prompts get Enter/Esc; elicitation and input get Open only`; `a host without the attention feature gets approve with seq 0 and the warning`.
- `web/tests/notify.test.mjs`: `a hidden session needing approval notifies once`; `a second attention in the same state does not notify again`; `a visible session never notifies`; `leaving attention re-arms it`; `titleText counts`.
- `web/tests/panes.test.mjs`: `grid 1/2/4 keeps panes in order and clamps focus`; `dropping a session on a pane moves it there`; `only the focused pane's own fit sends a resize, never an attached echo, and not the same size twice`.
- `web/tests/gauge.test.mjs`: `8 segments with 60/85 thresholds`; `the context window is 1M for [1m] models`.
Run: `node --test web/tests` → the new files fail.

Step 2: implement; extend the mock server so attention changes can be scripted (a `mock.trigger` dev-only op is not allowed in the protocol: instead, the mock moves one fixture session through working → needs_approval every 20 s with a new seq, and accepts `session.approve` by moving it to working).
Run: `node --test web/tests` → `# fail 0`
Manual check against the mock (record it in the commit message body in one line): two panes side by side, typing echoes, Approve on the permission card sends Enter (the mock shows it), the elicitation card has only Open, Ctrl+Enter in the composer pastes, the narrow layout at 700 px shows only board and queue.

Step 3: commit
`git commit -m "feat(web): session rail, attention queue, board, split panes and composer" -- web/app web/styles web/dev/mock-server.mjs web/tests`

Interfaces this task provides to later tasks: `queue.js`, `notify.js`, `panes.js`, `gauge.js` exports; the pane component's `openDrawer(tab)` hook (`reader`, `changes`, `history`, `stats`, `session`), the focused session signal (`store.focused`).

### Task 9: Wizard, reader, search, snippets, extension actions
Model: standard   Repo: /volumes/repos/claudio   Files: web/app/wizard.js, web/app/spawn-hooks.js, web/app/render/{markdown,answer,katex,mermaid,highlight}.js, web/app/ui/{wizard,reader,search,snippets,actions}.js, web/app/ui/app.js, web/styles/{layout,components}.css, web/dev/mock-server.mjs, web/tests/{markdown,wizard}.test.mjs
Goal: AC9 (markdown/latex/mermaid preprocessing) and the spec's wizard, reader (KaTeX, mermaid, tables, highlighted code with copy buttons, formulas list, lens items), search, snippets and extension actions.
Depends on: Task 8
Patch: none

Pure modules (exact exports):
```js
// web/app/render/markdown.js
export const MARKDOWN_PURIFY = { FORBID_TAGS: ['style', 'script', 'iframe', 'form', 'input', 'object', 'embed', 'link', 'meta', 'base'], FORBID_ATTR: ['style'], ALLOW_DATA_ATTR: false };
export const SVG_PURIFY = { USE_PROFILES: { svg: true, svgFilters: true } };
export const KATEX_OPTIONS = { trust: false, maxSize: 20, maxExpand: 1000, throwOnError: true, strict: 'ignore', output: 'htmlAndMathml' };
export const MERMAID_CONFIG = { startOnLoad: false, securityLevel: 'strict', htmlLabels: false, flowchart: { htmlLabels: false } };
/// `<latex>…</latex>` outside fenced and inline code → placeholders
/// `<nonce>:<i>` (private-use characters: the daemon's sanitize
/// strips them from model text, so the model cannot forge one). A tag alone
/// on its line is display math.
export function extractLatex(markdown, nonce)   // → { text, formulas: [{ tex, display }] }
export function placeholderRe(nonce)            // RegExp matching this nonce's placeholders
export function fenceLang(codeClassName)        // 'language-mermaid' → 'mermaid', '' → null
```
`render/answer.js` (browser): `renderAnswer(container, markdown, { onCopy })`: nonce from `crypto.getRandomValues`; `extractLatex`; `marked.parse(text, { gfm: true })`; `DOMPurify.sanitize(html, MARKDOWN_PURIFY)` into the container; then walk text nodes replacing placeholders with KaTeX (`katex.renderToString(tex, { ...KATEX_OPTIONS, displayMode })` purified with DOMPurify's default profile; an error → a `<code>` with the source as text); `pre > code.language-mermaid` → `mermaid.render(id, textContent)` → `DOMPurify.sanitize(svg, SVG_PURIFY)` in a `mermaid-diagram` element (a missing diagram type or an error → the code block stays, plus a one-line notice); other code blocks → `hljs.highlight(text, { language, ignoreIllegals: true })` (auto-detect when no language) purified, plus a copy button. `render/katex.js` loads `/vendor/katex/katex.min.js` (script element) and its CSS once; `render/mermaid.js` `import('/vendor/mermaid/mermaid.esm.min.mjs')` once, `initialize({ ...MERMAID_CONFIG, theme })`; `render/highlight.js` imports `highlight.js` and the theme CSS for the current theme.
```js
// web/app/wizard.js
export function initialWizard({ hosts, profiles, recent, focusedHost })
export function proxyOptions(profiles)        // [{value: 'default', label: `default (${profiles.default ?? 'direct'})`}, {value: 'direct', …}, …profiles.names as 'profile:<name>']
export function proxyArg(value)               // 'default' → {kind:'default'}, 'direct' → {kind:'direct'}, 'profile:x' → {kind:'profile', name:'x'}
export function firstPartyAllowed(value, profiles) // true only when the choice resolves to a profile
export function spawnRequest(state, size)     // {op: 'session.spawn', args} or {op: 'session.shell', args} (a terminal: no proxy, no resume, no prompt)
export function filterDirs(dirs, typed)       // fuzzy over paths, typed absolute path first when it is not listed
// web/app/spawn-hooks.js
export function setBeforeSpawn(fn)            // Task 10 sets the plugin-setup gate
export async function beforeSpawn(profile)    // default: { skip: [] }; returns { skip: [names] } or null (cancelled)
```
UI rules:
- Wizard (modal `wizard`, opened by `n`, the palette and a "+" button; `t` opens it on kind terminal): host (`wizard-host`: local, connected hosts, ssh candidates from `hosts.candidates`; choosing a candidate runs `hosts.connect` and shows its error lines on failure), kind (`wizard-kind`), directory (`wizard-dir-input` typed path with `filterDirs` over `fs.projects` (branch and age shown) and a browser over `fs.list`), resume (claude: `claude.sessions` of the dir, newest first, or "new conversation"), proxy (`wizard-proxy`, `proxyOptions`, first-party checkbox enabled by `firstPartyAllowed`), name (`wizard-name`), opening prompt (`wizard-prompt`), start (`wizard-start`): `beforeSpawn(profile)` then `spawnRequest` with the focused pane's fitted size; the new session opens in the focused pane.
- Reader (drawer tab `reader`): `reader.answers {sid}` on open and when the session's turn count changes; each answer (`reader-answer`) shows its prompt (collapsed), the rendered answer, copy-answer and per-block copy buttons (`navigator.clipboard.writeText` on click only); a formulas list (Tab) from the session's `lens` items tagged `latex`, rendered with KaTeX; items of other tags: `extensions.render {lens, kind: 'tag', source}` → the `block` rows (else `inline`) in a monospace `<pre>` as text.
- Search (palette "search conversations" and `/` in app mode): `search-input`; `search {query}` → hits (`search-hit`) newest first with host chip, title, snippet and age, plus a line per `failed` host; choosing a hit focuses a live session with that `claude_session_id`, else opens the wizard filled with host, cwd and that conversation to resume.
- Snippets (`snippets`, palette "insert snippet" / "save last prompt"): `snippets.list` items (`snippet`) insert their text into the focused composer (never sent); `snippet-save` saves the composer's text, and "save last prompt" saves `reader.last_prompt {sid}`.
- Extension actions: the palette lists each enabled external extension's actions (`extensions.list`); one with `input` asks for a line first; `extensions.action {ext, action, input, sid}` → `document` opens a reader-style modal (same `renderAnswer`), `notice` a toast, `paste` → `session.paste {submit: false}` into the focused claude session, `list` a palette-style picker whose items carry results, `resume` like a search hit.

Step 1: failing tests:
- `web/tests/markdown.test.mjs`: `extractLatex replaces tags outside code with this render's placeholders`; `latex inside fenced and inline code is left alone`; `a tag alone on its line is display math, inside a sentence inline`; `model text cannot forge a placeholder` (text already containing `x:0` with nonce `y` yields no formula match for it); `KATEX_OPTIONS keep trust off and cap size and expansion`; `MERMAID_CONFIG is strict with SVG labels`; `MARKDOWN_PURIFY forbids script, style, iframe and style attributes`; `fenceLang reads the language class`.
- `web/tests/wizard.test.mjs`: `proxy options pre-select and label the [proxy] default`; `first-party only with a proxy`; `spawn request for claude carries resume, prompt, name and size`; `a terminal spawns a shell without proxy, resume or prompt`; `typed paths filter the recent dirs fuzzily`.
Run: `node --test web/tests` → the new files fail.

Step 2: implement; extend the mock (a fixture answer with a display and an inline `<latex>`, a mermaid flowchart, a highlighted rust block and a table; fixture search hits; snippets).
Run: `node --test web/tests` → `# fail 0`
Manual check against the mock (one line in the commit body): the reader shows KaTeX and the mermaid SVG, `window.__claudioCsp` is empty after rendering both, the wizard starts a session in a typed directory.

Step 3: commit
`git commit -m "feat(web): new-session wizard, reader with KaTeX and mermaid, search and snippets" -- web/app web/styles web/dev/mock-server.mjs web/tests`

Interfaces this task provides to later tasks: `renderAnswer`, `setBeforeSpawn`, the wizard's `open({host, cwd, resume, kind})`, the drawer's reader tab component.

### Task 10: Changes, history, stats, extensions and plugin setup, claude toasts, host panel
Model: standard   Repo: /volumes/repos/claudio   Files: web/app/{diff,charts}.js, web/app/ui/{drawer,changes,history,stats,extensions,plugin-setup,claude,hosts}.js, web/app/ui/{app,topbar}.js, web/styles/{layout,components}.css, web/dev/mock-server.mjs, web/tests/{diff,charts}.test.mjs
Goal: the spec's right drawer (Changes, History, Stats, Session), the extensions screen and the plugin setup before spawn, claude install/update toasts, host panel.
Depends on: Task 9
Patch: none

Pure modules (exact exports):
```js
// web/app/diff.js
export function parsePatch(text)   // {files: [{oldPath, newPath, binary, hunks: [{header, lines: [{kind: 'ctx'|'add'|'del'|'meta', text, old, new}]}]}]}
export function sideBySide(hunk)   // [{left: line|null, right: line|null}]: runs of del then add paired row by row
export function groupWorktree(files) // {conflicts, staged, unstaged, untracked}: U → conflicts; ? → untracked; index ≠ '.' → staged; worktree ≠ '.' → unstaged (a file can be in both)
// web/app/charts.js
export function linearScale([d0, d1], [r0, r1])
export function niceTicks(max, count = 4)
export function barLayout(values, width, height, gap = 2) // [{x, y, w, h}]
export function formatTokens(n)    // 950 → '950', 5600 → '5.6k', 2_300_000 → '2.3M'
```
UI rules:
- Drawer (`drawer`, tabs `drawer-tab-reader|changes|history|stats|session`, `r`/`g` in app mode open reader/changes): one session at a time, the focused pane's.
- Changes (`changes`): hidden (tab disabled with a tooltip) when the host lacks `worktree`; `git.status {sid}` on open and when the session's `git` changes (debounced 1 s); groups from `groupWorktree` with counts; rows `changes-file` (`data-path`, the `+added −removed` counts, `old_path → path` for renames); a click → `git.worktree_diff {sid, path, staged}` (staged for staged-only rows; a file in both groups opens unstaged first, with a staged toggle) → `diff` view, unified by default, side-by-side toggle (`sideBySide`), line numbers, a `truncated` notice; text only (no HTML from the patch).
- History (`history`): `git.log {sid, skip, limit: 100}` pages with "more", `all` toggle; a commit → `git.commit`; its files → `git.diff {sid, oid, file}` in the same diff view.
- Stats (`stats`): profile picker (the focused session's proxy, else `[proxy] default`), windows 24h/7d/30d → `proxy.stats`; SVG (no canvas, no library) charts (`stats-chart`): output tokens per model as bars (`barLayout`), totals tiles, the limit as a gauge (`gauge.js`), pool health list; the focused session's credential (`proxy.credential`, refreshed every 60 s while shown) with 5h/7d gauges; "Move to next subscription" → `confirm()` → `proxy.switch` → a toast with from → to. The top bar shows the focused session's 5h/7d gauges.
- Session tab: host, cwd, kind, ids, proxy (+first-party), model, tokens, turns, git, credential; the session actions of Task 8.
- Extensions (`extensions`, from the palette and settings): rows `extension-row` (`data-name`) with `extension-toggle` (→ `extensions.toggle`, refreshed on `extensions_changed`), description, source, lenses, actions, refused reason; plugin rows add per-host status from `plugins.status` and `plugin_status` events (installed, present, skipped with reasons, a live `retrying in N s` countdown from `retry_secs`); a plugin row opens its setup.
- Plugin setup (`plugin-setup`): `plugins.items {name}` grouped by `section`, a checkbox per item (all on at first sight); `plugin-save` → `plugins.save {name, disabled}`; `plugin-skip` closes it. `setBeforeSpawn(async profile => …)`: from `plugins.list`, every enabled plugin of that profile with a ready manifest and `choices_saved: false` opens its setup in turn; Save continues, Skip adds the name to `skip`, Escape cancels the spawn (returns null).
- Claude toasts (`claude-toast`, non-modal, never steal keys): `offer_install` "Install claude on <host>?" [Install] [Skip] → `claude.update {host, install: true}` / `claude.skip {host}`; `offer_update` "claude <have> → <want> on <host>" [Update] [Skip]; `installing`/`updating` a sticky progress toast; `done` "claude <version> on <host>"; `failed` an error toast with `tail` as text.
- Hosts: top-bar chips (`host-chip`, `data-host`) with cpu/mem mini bars from `host_stats` and the state glyph (`⇄` reconnecting, outdated badge); a chip opens the host panel: hostname/os/arch, claude version and path, daemon version, features, the error text, connect/disconnect (remotes), "add host" (`hosts.candidates` + typed alias → `hosts.connect`, ssh error lines shown), and a claude update/install button (`claude.update`).

Step 1: failing tests:
- `web/tests/diff.test.mjs`: `a unified patch parses into hunks with line numbers`; `side-by-side pairs removals with additions`; `binary files and renames are recognised`; `worktree files group into conflicts, staged, unstaged and untracked`.
- `web/tests/charts.test.mjs`: `a linear scale maps the domain onto the range`; `ticks are round numbers up to the max`; `bars fill the width with gaps`; `token counts format compactly`.
Run: `node --test web/tests` → the new files fail.

Step 2: implement; extend the mock (a worktree status and patches, a log page, stats for each window, plugins with one unsaved plugin, `claude` offer events on connect for `z6`).
Run: `node --test web/tests` → `# fail 0`
Manual check against the mock (one line in the commit body): Changes lists the fixture files and shows a side-by-side diff; Stats draws the bars; the plugin setup opens before a spawn on the profile with the unsaved plugin and Skip spawns; the claude offer toast does not take keyboard focus.

Step 3: commit
`git commit -m "feat(web): working-tree changes, history, stats, extensions and plugin setup" -- web/app web/styles web/dev/mock-server.mjs web/tests`

Interfaces this task provides to later tasks: every `data-testid` of Global Constraints is now rendered by some component.

### Task 11: End-to-end tests, Makefile, docs
Model: standard   Repo: /volumes/repos/claudio (feat/web-ui, after both lanes are merged into it)   Files: web/e2e/playwright.config.mjs, web/e2e/fixtures/{server,fake-proxy,transcript}.mjs, web/e2e/fixtures/fake-claude.sh, web/e2e/tests/{auth,board,terminal,wizard,reader,changes,palette,notify,lifecycle,proxy}.spec.mjs, Makefile, docs/web.md (new), README.md, CLAUDE.md; not committed: .delivery/2026-11-08-web-ui/parity.md
Goal: AC8, AC14 (browser e2e against a real `claudio web` and fake claude/proxy), AC12 and AC13 (docs and parity table), `make test-web`.
Depends on: Tasks 5 and 10, both lanes merged into feat/web-ui
Patch: none

Before anything: `cargo build --locked` and `cargo test --locked --test web -- --test-threads=1` pass on the merged branch; `node --test web/tests` passes; fix contract drift between the lanes here (the protocol doc wins; a change to it is its own commit `docs(web): …`).

Fixtures:
- `fixtures/fake-claude.sh` (copied into each server's root, chmod 755): answers `--version` (`0.0.0-fake`, or `0.0.1-fake` once `$FAKE_DIR/updated` exists), `--help` (lists `--agents`/`--mcp-config`), `update` (sleeps 1 s, touches `$FAKE_DIR/updated`, prints "updated", exit 0); otherwise saves `$2` to `$FAKE_DIR/settings/<cwd with / → _>.json`, prints `FAKE_CLAUDE_BANNER`, then for each input line appends it to `$FAKE_DIR/input/<same>.log` and prints `line:<text>` (a terminal-tab test uses the real `/bin/sh`).
- `fixtures/server.mjs`: `export async function startServer({ proxy = false } = {})` → `{ url, token, port, root, dirs: {a, b, repo}, fireHook(cwd, event, payload), inputLog(cwd), api(op, args), stop() }`. Scratch root under `~/.cache/claudio/web-e2e/<8 hex>/` (short for the macOS socket limit): `run`, `cfg`, `home`, `claude-config`, `xdg-cache`, `fake`, `work/a`, `work/b`, `work/repo` (git repo: `a.txt` committed, then modified). `cfg/claudio/config.toml` = the test config (`[claude] update_check = "off"`, `remote_check = "off"`, `install = "off"`). Env: the isolation of tests/web.rs's `apply_env` (every `CLAUDIO_*` removed, `XDG_RUNTIME_DIR`, `XDG_CONFIG_HOME`, `XDG_CACHE_HOME`, `HOME`, `CLAUDE_CONFIG_DIR`, `CLAUDIO_CLAUDE_PATH` = the fake, `CLAUDIO_NO_BROWSER=1`, `SHELL=/bin/sh`, `ANTHROPIC_API_KEY` dummy), plus `FAKE_DIR`, `CLAUDIO_TEST_LOOPBACK_HTTP=1`, and with `proxy` `CLAUDIO_PROXY_URL=tok-e2e@127.0.0.1:<fake proxy port>`. Starts `${CLAUDIO_BIN ?? '<repo>/target/debug/claudio'} web --listen 127.0.0.1:<free port> --no-open`, reads `claudio web: <url>`. `fireHook` runs the saved hook command with `sh -c` and the JSON on stdin; `api` is a `ws` client logged in through `/api/login` (Origin and Cookie headers) for setup and assertions; `stop` kills the server, runs `claudio daemon stop` with the same env, removes the root.
- `fixtures/fake-proxy.mjs`: node `http` server with the routes and bodies of tests/web.rs `fake_proxy` (config, plugins listing `vocdoni-test` whose choices are not saved, its manifest, `/me/stats` for every period, `/pool/health`).
- `fixtures/transcript.mjs`: `writeTranscript(claudeConfig, cwd, conversation)` writes `projects/<canonical cwd, every non-alphanumeric → '-'>/<conversation>.jsonl` with one prompt and one answer whose text has a display `<latex>E = mc^2</latex>` on its own line, an inline `<latex>a^2 + b^2</latex>`, a mermaid ` ```mermaid ` flowchart `graph TD; A-->B`, a ` ```rust ` block, a table and the word `fixtureword` (record shapes as the `recent_answers` tests in src/claude/projects.rs).
`playwright.config.mjs`: `testDir: 'tests'`, `workers: 1`, `fullyParallel: false`, `timeout: 60_000`, chromium headless, `reporter: 'line'`, no `webServer` (fixtures start servers per spec file in `beforeAll`).

Specs (test titles are binding; selectors are the Global Constraints' `data-testid`s):
- AC8 `auth.spec.mjs`: `logs in from the token URL and clears the fragment` (goto `url`, the board appears, `location.hash` is empty, a reload without the token still works through the cookie).
- AC8 `board.spec.mjs`: `the board lists sessions with their states` (two sessions spawned through `api`, one moved to idle with a `Stop` hook: cards show `data-state` working/idle with their glyphs); `a permission prompt shows its text and Approve resolves it` (`PreToolUse` Bash + `Notification permission_prompt` hooks → the `queue-card` shows the message and `ls -la`; click `approve-btn` → the input log gets an empty line; a `PreToolUse` hook → the card leaves the queue); `an elicitation card offers Open only` (no `approve-btn`/`deny-btn` in that card).
- AC8 `terminal.spec.mjs`: `a pane shows the terminal and typed keys echo` (open the session in a pane, the xterm shows `FAKE_CLAUDE_BANNER`, typing `hello⏎` shows `line:hello`).
- AC8 `wizard.spec.mjs`: `the wizard starts a session in a chosen dir` (`n`, type `dirs.b` in `wizard-dir-input`, choose direct, `wizard-start` → a new `session-card` with cwd `b` and the banner in the focused pane).
- AC8 `reader.spec.mjs`: `the reader renders KaTeX and mermaid with no CSP violation` (a session whose `SessionStart` hook names the fixture conversation; drawer reader: a `.katex-display` and an inline `.katex`, an `svg` inside `mermaid-diagram`, an `.hljs` code block; then `expect(await page.evaluate(() => window.__claudioCsp ?? [])).toEqual([])` with a terminal pane open too).
- AC8 `changes.spec.mjs`: `Changes lists a modified file and shows its diff` (session in `dirs.repo`; `changes-file[data-path="a.txt"]`; click → `diff` contains `+changed`).
- AC8 `palette.spec.mjs`: `Ctrl+Shift+Space then k opens the palette` (focus the terminal, press Ctrl+Shift+Space, the mode badge says APP, `k` → `palette` visible); `theme switch applies plain` (settings → `theme-select` plain → `html[data-theme^="plain"]`).
- AC8 `notify.spec.mjs`: `a hidden session needing approval notifies once` (`context.grantPermissions(['notifications'])`, `page.addInitScript` replacing `window.Notification` with a stub class counting constructions and `permission = 'granted'`; enable notifications in settings; a session not in any pane gets `PreToolUse` + `permission_prompt` → one notification; a second `permission_prompt` (same state, new seq) → still one).
- AC14 `lifecycle.spec.mjs`: `a terminal tab runs echo ok` (`terminal-tab-btn`, type `echo ok⏎`, the pane shows `ok` on its own line); `rename, kill and restart a session` (rename via `rename-input` → the card label; `restart-resume-btn` + `confirm-yes` → a new banner nonce in the pane; `kill-btn` + `confirm-yes` → the card is gone); `search finds a fixture conversation` (`search-input` `fixtureword` → a `search-hit`); `save and insert a snippet` (`snippet-save` from the composer text, then `snippet` inserts it into the composer); `toggling an extension rewrites its config.toml line` (`extension-row[data-name="diagram"] extension-toggle` off → `cfg/claudio/config.toml` contains `diagram = false`; on → `diagram = true`); `claude update shows progress and completion` (host panel → update local claude → a `claude-toast` with progress, then one with `0.0.1-fake`).
- AC14 `proxy.spec.mjs` (a server with `proxy: true`): `a plugin without choices opens setup before spawn and Skip spawns without it` (wizard with the default proxy → `plugin-setup` for `vocdoni-test`; `plugin-skip` → the session starts; the fake claude's env (dumped by the spec through `api`'s session cwd: the fake writes `env` when `FAKE_DUMP_ENV=1`) has the proxy token and no plugin secret); `proxy stats render the fixture window` (drawer stats, window 24h → `stats-chart` has a bar for `claude-opus-5-5`, totals show `5.6k`).

`Makefile`: add `test-web` to `.PHONY` and the header comment, and:
```make
## Browser app: node unit tests, then Playwright e2e against a debug build
## (fake claude and proxy; uses the browsers cached in ~/.cache/ms-playwright).
test-web:
	node --test web/tests
	$(CARGO) build --locked
	cd web/e2e && PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 npm ci --no-audit --no-fund && npx playwright test
```

Docs:
- `docs/web.md` (new): usage (`claudio web` flags, the token URL, `--print-url`, `--rotate-token`, `ssh -L 7337:127.0.0.1:7337 host`, a TLS reverse proxy with `--allow-host`, a PWA/installed window frees browser keys); the security model (loopback only, token file ≈ SSH key, cookie flags, Host/Origin checks, CSP and the import-map hash, secrets never in the page, what is logged); keys (terminal mode vs app mode, every binding from `keymap.js` BINDINGS, what is never bound); the AC13 parity table: one row per Goal-2 capability (sessions on many hosts, terminal panes, terminal tabs, wizard (dirs, resume, proxy profile, first-party), rename, kill, restart (resume/new), reader with formulas and diagrams, git viewer (history) and working tree, proxy stats, subscription switch, search, snippets, extensions screen, plugin setup, claude install/update, host stats, notifications, next attention, help) with columns TUI key | web action | WS op(s) | test (e2e title or Rust test name).
- `README.md`: a short "Web UI" section (what it is, `claudio web`, the security one-liner, link to docs/web.md).
- `CLAUDE.md`: architecture entries for `src/web/` (mod, auth, assets, ws, hub, dto, input, state, spawn, proxy, plugins, ext; build.rs) and `web/` (index.html import map, app/, render/, styles/, vendor/ + MANIFEST, fonts/, tests/, e2e/, dev/mock-server.mjs); test commands (`cargo test --test web -- --test-threads=1`, `node --test web/tests`, `make test-web`); non-obvious constraints: the import map is the only inline script and its hash is in the CSP (editing it changes the hash; no other inline script ever), vendored files are byte-identical, the web never writes state.json, DTO fields are fixed by docs/web-protocol.md, session ids are `sid` in the protocol, `SessionEvent::Attention` is nested.
- `.delivery/2026-11-08-web-ui/parity.md` (not committed): the parity table with, per row, the test that covers it or a one-line manual check and its result.

Run: `make test-web` → node `# fail 0`, Playwright `N passed` (no failures, no retries); `cargo test --locked --test web -- --test-threads=1` → `0 failed`; `make test-unit`, `make test-manager` → `0 failed`.

Step 3: commit (explicit paths; package-lock.json only if it changed)
`git commit -m "test(web): browser end-to-end tests, make test-web and docs" -- web/e2e/playwright.config.mjs web/e2e/fixtures web/e2e/tests Makefile docs/web.md README.md CLAUDE.md`

Interfaces this task provides: `make test-web`; docs/web.md's parity table (AC13).

## Merge and release
<!-- Not copied into briefs; the controller executes this after the gate audit. -->
1. Lanes: merge the backend worktree branch (Tasks 1–5) and then the frontend worktree branch (Tasks 6–10) into feat/web-ui (fast-forward or a plain merge; they share no file). Run Task 11 on feat/web-ui.
2. Full suites on feat/web-ui (single-threaded where noted):
   - `make test-unit` → `0 failed` (base ≈ 1100 + the new unit tests)
   - `make test-manager` → `0 failed`
   - `cargo test --locked --test web -- --test-threads=1` → `0 failed` (Tasks 3–5: 6 + 12 + 7 = 25 tests)
   - `node --test web/tests` → `# fail 0`
   - `make test-web` → Playwright all passed
   - `env -u CLAUDIO_SSH_TEST_HOST cargo test --locked --features diag --test remote -- --test-threads=1` → `0 failed`
   - `cargo build --release --locked` on Linux; CI covers macOS.
3. Gate audit of the whole branch diff against the spec and this plan's Review Focus; fixes re-reviewed by the controller before release.
4. Release: version 0.8.0 in `Cargo.toml` (`version = "0.8.0"`) and `Cargo.lock` (`cargo build` updates the claudio entry); one commit `release v0.8.0` (explicit paths `Cargo.toml Cargo.lock`). Merge feat/web-ui into main the way the repo does: fast-forward (main's history is linear: `git checkout main && git merge --ff-only feat/web-ui`). Tag `v0.8.0` on that commit (annotated, message `v0.8.0`).
5. **Owner OK required before** (the controller holds a standing autonomous instruction for this run and records each step in the ledger as done under it): `git push origin main`; `git push origin v0.8.0` (triggers the release workflow); any publish of release assets.
6. Post-release checks:
   - CI (`ci.yml`) green on Linux and macOS for main; the release workflow (`release.yml`) green, all four assets attached to v0.8.0.
   - `claudio web` smoke test on the released linux x86_64 binary: start it in a temp HOME/XDG dirs with `--no-open`, `curl -s -o /dev/null -w '%{http_code}' -H 'Host: 127.0.0.1:7337' http://127.0.0.1:7337/` → 200 with the CSP header; login with the printed token → 204 + cookie; `/ws` upgrade with that cookie and Origin → 101; `claudio web --listen 0.0.0.0:7337` → exit 2.
   - AC11: binary size delta, v0.7.1 vs v0.8.0 release asset (linux x86_64, static): growth ≤ 4 MiB; record both sizes and the delta in the ledger.
