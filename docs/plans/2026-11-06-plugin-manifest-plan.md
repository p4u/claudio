# Plugin manifests: implementation plan

Spec: docs/specs/2026-11-06-plugin-manifest-design.md (approved 2026-11-06)
Class: architectural
Repos and branches: /volumes/repos/claudio → feat/plugin-manifest (base c88c627)

Every task below was implemented, built and tested in order in a scratch worktree
(`~/.cache/claudio/scratch/plugin-manifest`, detached at c88c627, commits 70594b1 … 19647e4).
Each task's code is its patch, `.delivery/2026-11-06-plugin-manifest/patches/task-<N>.patch`
(a `git format-patch` of that commit), reproduced in Steps 1 and 2: Step 1 is the patch's test
part, Step 2 the rest. Apply each block with `/usr/bin/git apply <block>` from the repo root on
the tree the previous step left (or the whole task at once with
`/usr/bin/git apply .delivery/2026-11-06-plugin-manifest/patches/task-<N>.patch`), run the
commands, then commit as given. Do not hand-merge; if a block does not apply, stop and report.

Base state (c88c627), measured in the scratch: `cargo build` OK, 0 warnings; `cargo test --locked
--bin claudio` 1044 passed; `cargo test --test manager -- --test-threads=1` 52 passed;
`env -u CLAUDIO_SSH_TEST_HOST cargo test --features diag --test remote -- --test-threads=1`
9 passed. No test fails deterministically at the base. Seen once each while building this plan,
both passing on rerun (known flakes, not caused by this work):
`daemon::host::tests::probe_runs_claude_with_the_given_path` (first unit run at the base) and
`daemon::tests::freshness::an_outdated_idle_daemon_is_replaced_and_its_sessions_come_back`.
Not rustfmt-clean and not clippy-clean at the base: the snap clippy reports 74 distinct
findings, among them the deny-level `src/tui/wizard.rs:193` (`nonminimal_bool`) that stops the
build; the tip of this plan reports the same 74 and none on a line this branch adds.

Decisions taken while planning (none changes the spec's design; listed for the audit):
- D1. Test-only loopback http: `proxy::plugins::test_http_ok(url)` is true only in a debug build
  (`cfg!(debug_assertions)`), for `http://127.0.0.1:<port>/…`, with
  `CLAUDIO_TEST_LOOPBACK_HTTP=1`. `admit`, the client's manifest fetch and the daemon's download
  honour it. AC4 needs it: the client fetches the manifest itself and keeps no cache to seed.
  A release build is https only, as the spec says. The variable is not documented in
  `cli::HELP_APPENDIX` (test-only).
- D2. `Applied.present` lists plugins only. A server or agent the member has is
  `skipped: (id, "yours")`, following §3 steps 2–3 and AC2 (the `Applied` comment's
  "plugins, servers, agents" conflicts with them).
- D3. A missing `.claude.json` means "no servers of the member's own". An unreadable or
  unparsable one, a symlink, or one that is not the uid's regular file gives `cannot read
  .claude.json` (§3).
- D4. Install outcomes are remembered per (manifest hash, id). A marketplace whose add fails, or
  whose name comes out different, is remembered for 10 min per (hash, `marketplace:<name>`).
  Each of its plugins is then skipped with `marketplace <name>: <reason>` (`added under another
  name`, or the CLI's message). A marketplace has no item id, so it has no list entry of its own.
- D5. Grammar details §1 leaves open:
  - A `github:` owner or repo may not start with `-` or `.`: the CLI would read it as a flag or
    a path.
  - `${` in a stdio `command` is refused.
  - An agent's `description` and `prompt` must be non-empty.
  - The top-level `description` and `instructions` are optional.
  - Reasons name an unknown or missing key when it is `[A-Za-z0-9_-]{1,64}`. Any other TOML
    error gives `not valid TOML, or a value of the wrong type (line N)`, never the text.
- D6. `command()` keeps dropping `--plugin-dir` pairs: a pre-manifest dev build's journal may
  hold some. Flag detection matches option lines (`lists()`), because the `--bare` help line
  names `--mcp-config`, `--agents` and `--plugin-dir`.
- D7. The 8-plugin cap counts answered (`Ready`) plugins in registry order. The 9th and later
  are left out entirely: no flags, `[env]` or instructions. Every enabled plugin's secrets
  still apply, as the old `plugin_env` did.
- D8. Instructions over 8 KiB: the first plugin whose instructions do not fit, and every later
  one, are left out. One notice names them.
- D9. A first-sight setup screen appears unasked, so it ignores keys for 4 ticks, like a
  confirm prompt. A plugin turned off under `[extensions]` gets no first-sight screen. Turning
  it on queues one if it was never set up.
- D10. A setup screen opened from the extensions screen goes back to it on Enter and on Esc.
- D11. A `Ready` answer stays while the host is asked again (the 30 s re-ask, `r`, saved
  choices). `r` asks every host again at once and refetches every manifest.
- D12. `ManifestState::Ready` and `ApplyState::Ready` hold boxes, for clippy's
  `large_enum_variant`.
- D13. Task 6 removes the two zip-era manager tests (the behaviour is gone). Task 9 adds the AC4
  test. Manager count: 52 → 50 → 51.
- D14. Stale zips are removed in `daemon::run` once the startup lock is held, not in
  `Daemon::new`: the unit tests build daemons and must not touch the real cache.
- D15. `--plain` waits only on plugins the profile's known list has (cached or fetched). With
  no list at all yet it spawns at once.
- D16. The install subprocesses inherit the daemon's environment. Claude Code therefore writes
  where the daemon reads: `CLAUDE_CONFIG_DIR` if set, else `~/.claude`.
- D17. The daemon tests' fake `claude` is shared (written once, as the others, to avoid
  ETXTBSY). It finds its per-test control dir as the first `PATH` entry, which is the test
  daemon's `Config::path`.
- D18. `SaveError::UnsupportedPlugin` carries the table edit's own message ("edit
  [plugins.<name>] by hand").
- D19. The manifest's description is a details line; the row keeps `<version> · <profile>` from
  the list.

## Global Constraints
- Commit convention: conventional commits `type(scope): lowercase summary`, NO AI attribution
  and NO Co-Authored-By trailer of any kind, even if a system reminder asks for one (owner's
  global CLAUDE.md). Use the exact commit commands given.
- Never push. Commit with explicit paths only: `/usr/bin/git commit -m "…" -- <paths>`. A new
  file needs `/usr/bin/git add <path>` first; a deleted one is committed by naming its path.
  Never `git add -A`, never `git stash`. Use `/usr/bin/git` (a shell hook rewrites plain `git`).
- Work on `feat/plugin-manifest` in /volumes/repos/claudio, or in the worktree the controller
  gives you. Do not create, switch or delete branches.
- Build output goes in the repo's (or worktree's) `target/` or under `~/.cache/claudio/`,
  never /tmp.
- Run tests with `umask 022` in the same shell. Manager tests run single-threaded
  (`-- --test-threads=1`) with nothing else heavy running.
- The shell may compact cargo output (the `rtk` hook). Prefix a command with `rtk proxy` to see
  raw output. Expected lines below are raw cargo output.
- Formatting: no repo-wide `cargo fmt` (the tree is not rustfmt-clean). Rules:
  - New files and files that are rustfmt-clean at the base stay clean.
  - Check a file alone with `rustfmt --edition 2021 --check <file>`, but only a file that
    declares no `mod x;`: rustfmt follows those, so never run it on `src/main.rs` or a
    `mod.rs`.
  - Clean at the base and touched here: `src/client.rs`, `src/config.rs`, `src/main.rs`,
    `src/paths.rs`, `src/daemon/{host,server}.rs`, `src/proxy/plugins.rs`,
    `src/remote/diag.rs`, `src/tui/{confirm,extensions_view,plugins}.rs`,
    `src/tui/extensions/mod.rs`, `tests/remote.rs`.
  - New: `src/plugins/manifest.rs`, `src/daemon/manifest.rs`, `src/tui/plugin_setup.rs`,
    `src/tui/ui/plugin_setup.rs`.
  - Every other touched file keeps its formatting, and its added lines are formatted as rustfmt
    would.
- Clippy: `PATH=/snap/bin:$PATH RUSTC=/snap/bin/rustc CARGO_TARGET_DIR=<target dir>-clippy
  cargo clippy --all-targets --features diag --message-format=short` (the system cargo and the
  snap clippy disagree on rustc otherwise). The base is not clean. The gate: no clippy finding
  on a line a task adds. A task may leave the dead-code warnings listed for it, which later
  tasks resolve (none after Task 7).
- The spec's constraints, verbatim:
  - `app.rs` pure; no field added to a released op (`SessionExtensions.instructions` is
    `#[serde(default)]` and ignorable); secrets never in argv, journal, `state.json`, logs or
    notices; keys on `Alt` outside modals; `cli.rs` forwards claude's flags unchanged;
    conventional commits, no AI attribution; Linux and macOS.
  - Never `-y`/`--accept-command` on `claude plugin install`; the daemon never writes into
    `~/.claude` itself.
  - One parser (`src/plugins/manifest.rs`) for client and daemon; reasons name ids and fields,
    never values.
- Harness rules:
  - Manager (PTY) tests assert on the rendered screen model, never on raw bytes. Files a fake
    claude writes are fine to read.
  - Never send `ESC` right before an `Alt+key` without waiting for the screen in between.
  - Every test claudio's config has `[claude] update_check = "off"` and `remote_check = "off"`
    (the harness writes it).
  - Fake claudes ignore `--help` and `--version` (a fake may print help lines, nothing else).
  - Tests never touch the user's daemon, caches, config or `~/.claude`: isolated HOME/XDG dirs,
    `XDG_CACHE_HOME` and `CLAUDE_CONFIG_DIR` under the test root, and `Manifests::set_dirs` in
    daemon tests.
- Nothing in this plan needs a real `claude`, the real proxy or an SSH host. `t9` of
  `tests/remote.rs` is gated by `CLAUDIO_SSH_TEST_HOST` and needs a real claude there: do not
  run it; the controller runs it for AC6.
- Unix only, Linux and macOS. No Linux-only API without `cfg`; `libc::O_NOFOLLOW`,
  `OpenOptionsExt`, `MetadataExt::uid`, `process_group` and `setsid` are fine on both. No new
  dependency: `toml`, `reqwest` (rustls), `sha2`, `serde_json` and `axum` (tests) are already
  in Cargo.toml.

## Review Focus
Line numbers are as the tree stands after Task 10.
- Manifest grammar and caps:
  - `src/plugins/manifest.rs:247` (`parse`): 256 KiB, `format` checked on the raw table first,
    `deny_unknown_fields`, the rendered agents (32 KiB) and MCP (64 KiB) JSON checked at parse.
  - `:457` (`toml_reason`): the line, never the text.
  - `:518` (`source_ok`): `github:` parts that cannot read as flags or paths; https ≤ 512
    printable.
  - `:531`/`:547` (`placeholders`, `check_placeholders`): exactly `${NAME}` by `key_ok`, declared
    in `secrets` or `[env]`; any other `${` refused.
  - `:371` (`id_ok`): the request grammar. `RawServer::check`: per-type field sets, oauth with
    no headers or secrets, token header names, printable header values.
- Daemon request and file reads:
  - `src/daemon/manifest.rs:279` (`handle`): name, hash, https, ≤ 64 secret names by `key_ok`,
    ≤ 256 ids by `id_ok`, else `bad request`.
  - `:688`/`:742` (`ready`, `download`): 0700 cache, sidecar, ≤ 32 manifests, 256 KiB cap,
    `O_NOFOLLOW|O_EXCL` 0600 temp file, hash before rename.
  - `:433` (`read_json`): `symlink_metadata`, a regular file of this uid, ≤ 16 MiB,
    `O_NOFOLLOW`, content never logged. `Files::read` decides "missing" against "unreadable".
- Decision order and memory:
  - `:195` (`decide`): installed by this daemon under this hash, then present, then manual,
    then a failure under 10 min, then `installing` and queued once (`busy`).
  - Servers: `yours`, then the missing secret. Agents: `yours`. Disabled ids neither applied
    nor listed. Instructions sanitized.
- Installs:
  - `:460`/`:478` (`enqueue`, `run_queue`): one task per hash; the queue entry is removed
    under the same lock that checks it.
  - `:518` (`install`): a remembered marketplace failure, the add only when the name is
    unknown, re-read after the add.
  - `:571` (`run_claude`): `updating.lock().await` per subprocess, markers scrubbed,
    `GIT_TERMINAL_PROMPT=0`, `GIT_ASKPASS=/bin/true`, stdin null, 64 KiB per stream, own process
    group, 120 s, group killed (`update::kill_group`). Never `-y`.
  - `:631` (`outcome`): the last stdout line's JSON; `shownCommand` takes precedence.
  - `src/daemon/update.rs:38`: `try_lock` and its message.
- The reply comes before any install: `handle` decides, queues, then returns `Applied`.
  `src/daemon/server.rs` replies from a spawned task. The test
  `manifest_replies_first_then_installs_each_plugin_once` holds an install on a gate file.
- argv: `src/daemon/session.rs:327`/`:348` (`command`): a `--agents`, `--mcp-config` or
  `--plugin-dir` pair (or `=` form) is dropped when this claude lacks the flag, exactly one
  value with it; the args keep their order before `-n`. `src/daemon/host.rs:148` (`lists`).
- Client composition: `src/tui/plugins.rs:149` (`compose`):
  - Per plugin, `--agents` then `--mcp-config`, appended after every other argument (by
    `Msg::with_plugin_args`, `src/proto.rs:629`).
  - ≤ 8 plugins.
  - Env layers: `[env]`, then the secrets, then the profile env (`merge_env` in
    `src/tui/sessions.rs:465` and the deferred runner `src/tui/mod.rs:1640`).
  - The 8 KiB instruction cut.
- Client request rules, `src/tui/plugins.rs`:
  - `:355` (`requestable`): fetched and good at the list's hash, and choices saved.
  - `:366`/`:385` (`request_manifests`, `ask`): a `Ready` answer is kept while asked again.
  - `:416` (`manifest_msg`): secret names only.
  - `:434` (`reask_installing`): 30 s, ≤ 10 min.
  - `:504` (`plugin_parts`): what waits and what does not.
- Setup screen semantics, `src/tui/plugin_setup.rs`:
  - `:190` (`on_manifest_ready`): first sight queues; open or queued screens are replaced,
    keeping toggles; new ids get one notice per hash.
  - `:268`/`:296` (`plugin_setup_key`, `save_setup`): only Enter writes and requests; Esc on
    first sight defers for the run; back to the extensions screen.
  - `:146` (`choices_for`): gone disabled ids kept.
  - `:331`/`:349` (`plain_waits`, `plain_try_start`).
  - `src/tui/confirm.rs:88`/`:136` (`Queued`, `show_next_confirm`). `Modal::PluginSetup` is not
    in `yields_to_actions`.
- `SessionExtensions.instructions`: `src/proto.rs:516`/`:556` (`validate`: 8 KiB after
  `sanitize`, counted apart from the lenses; `context`: after the lenses). Journaled with the
  rest (`journal.rs` upsert clones `extensions`). `Respawn` carries the current value.
- Fail-safes:
  - An old daemon's `unsupported op` becomes `ApplyState::TooOld` ("daemon too old"), never
    the restart notice; the session starts with the secrets only.
  - An old daemon ignores `instructions`; an old client sends none.
  - `config.toml`'s `[plugins.<name>]` is preserved by an older claudio (`extra`) and written
    by `src/config.rs:527`/`:579` with a verified minimal table edit.
- Secrets: no value in `Msg::Manifest` (names), argv (the MCP JSON holds placeholders; t9 and
  AC4 assert it), the journal (args hold the agents JSON and placeholders only), logs (counts and
  reasons) or notices.

## Task order and parallelism
| Task | Depends on | Repo/files | May run in parallel with |
|---|---|---|---|
| 1. manifest parser and renderers | — | plugins/{mod,manifest}.rs (new), plugins/testdata/vocdoni-internal.toml (new), main.rs, proxy/plugins.rs | — |
| 2. proto: `Manifest`/`Applied`, `SessionExtensions.instructions` | 1 | proto.rs, client.rs, daemon/{journal,mod,tests}.rs, tui/extensions/mod.rs | — |
| 3. daemon: flag probe and pair drops | 2 | daemon/{host,session,mod,tests}.rs | — |
| 4. daemon: `manifest.rs` (fetch, apply, reply, install) | 3 | daemon/manifest.rs (new), daemon/bundle.rs (deleted), daemon/{mod,server,update,tests}.rs, proxy/plugins.rs | — |
| 5. config: `[plugins.<name>]` | 4 | config.rs | — |
| 6. client: manifests, `Applied` state, spawn | 5 | tui/{plugins,app,interaction,sessions,mod,extensions_view}.rs, proto.rs, proxy/plugins.rs, tests/manager.rs | — |
| 7. setup screen, choices, `--plain` | 6 | tui/plugin_setup.rs (new), tui/ui/plugin_setup.rs (new), tui/{confirm,app,interaction,mod,plugins,extensions_view,plain}.rs, tui/ui/mod.rs, tests/manager.rs | — |
| 8. drop the zip ops; t9 | 7 | proto.rs, client.rs, main.rs, remote/diag.rs, tests/remote.rs, proxy/plugins.rs, paths.rs | — |
| 9. manager test (AC4) | 8 | tests/manager.rs | — |
| 10. docs | 9 | CLAUDE.md, README.md, docs/extensions.md | — |

Tasks are one commit each and were built in this order; the patches apply only in this order
(later tasks edit lines earlier ones add), so nothing runs in parallel. Task 5 depends on
nothing but the order keeps the patches applying as recorded.

## Tasks

### Task 1: manifest parser and renderers
Model: strong   Repo: /volumes/repos/claudio   Files: src/plugins/mod.rs (new), src/plugins/manifest.rs (new), src/plugins/testdata/vocdoni-internal.toml (new), src/main.rs, src/proxy/plugins.rs
Goal: one pure parser for client and daemon, with every cap, charset and placeholder rule, the item ids and the MCP/agents renderers (AC1).
Patch: .delivery/2026-11-06-plugin-manifest/patches/task-1.patch

The fixture is the draft manifest the first release serves (`~/.cache/claudio/vocdoni-internal.toml`,
13 plugins, 3 servers, 3 agents), copied byte for byte; it ends with a blank line, so
`git apply` prints `new blank line at EOF` for it (expected, harmless).

Step 1: write the failing test (the test module, the fixture, and the module declarations)
`````diff
diff --git a/src/main.rs b/src/main.rs
index 89eaafa..fca2197 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -19,6 +19,7 @@ mod daemon;
 mod freshness;
 mod msglog;
 mod paths;
+mod plugins;
 mod plain;
 mod print;
 mod proto;
diff --git a/src/plugins/manifest.rs b/src/plugins/manifest.rs
new file mode 100644
index 0000000..d73f300
--- /dev/null
+++ b/src/plugins/manifest.rs
@@ -0,0 +1,575 @@
+#[cfg(test)]
+mod tests {
+    use super::*;
+
+    /// A manifest with one of everything.
+    const GOOD: &str = r#"
+format = 1
+name = "team"
+version = "1.0.0"
+description = "Team setup"
+instructions = """
+Search the knowledge base first.
+"""
+
+[[marketplace]]
+name = "vocdoni"
+source = "github:vocdoni/skills"
+
+[[marketplace]]
+name = "other"
+source = "https://example.org/market.git"
+
+[[plugin]]
+name = "agentic-dev"
+marketplace = "vocdoni"
+
+[[mcp]]
+name = "kb"
+type = "http"
+url = "https://kb.example.org/mcp"
+headers = { Authorization = "Bearer ${KB_TOKEN}" }
+secrets = ["KB_TOKEN"]
+
+[[mcp]]
+name = "notes"
+type = "sse"
+url = "https://notes.example.org/sse"
+auth = "oauth"
+
+[[mcp]]
+name = "local"
+type = "stdio"
+command = "npx"
+args = ["-y", "server", "--region=${REGION}"]
+env = { API_KEY = "${LOCAL_KEY}" }
+secrets = ["LOCAL_KEY"]
+
+[[agent]]
+name = "reviewer"
+description = "Reviews code"
+model = "opus"
+tools = ["Read", "Grep", "mcp__kb__*"]
+prompt = "You review code."
+
+[env]
+REGION = "eu"
+"#;
+
+    fn good() -> Manifest {
+        Manifest::parse(GOOD).unwrap()
+    }
+
+    /// Why `GOOD` with `from` replaced by `to` is refused.
+    fn refused(from: &str, to: &str) -> String {
+        assert!(GOOD.contains(from), "{from:?} is not in GOOD");
+        Manifest::parse(&GOOD.replacen(from, to, 1)).unwrap_err()
+    }
+
+    /// Why `GOOD` with `extra` appended is refused.
+    fn refused_with(extra: &str) -> String {
+        Manifest::parse(&format!("{GOOD}\n{extra}")).unwrap_err()
+    }
+
+    #[test]
+    fn a_good_manifest_parses_with_every_item() {
+        let m = good();
+        assert_eq!((m.name.as_str(), m.version.as_str()), ("team", "1.0.0"));
+        assert_eq!(m.marketplaces[0].cli_source(), "vocdoni/skills");
+        assert_eq!(
+            m.marketplaces[1].cli_source(),
+            "https://example.org/market.git"
+        );
+        assert_eq!(m.plugins[0].spec(), "agentic-dev@vocdoni");
+        assert_eq!(m.env, [("REGION".to_owned(), "eu".to_owned())]);
+        assert_eq!(
+            m.ids(),
+            [
+                "plugin:agentic-dev@vocdoni",
+                "mcp:kb",
+                "mcp:notes",
+                "mcp:local",
+                "agent:reviewer",
+                "instructions",
+                "env",
+            ]
+        );
+        let titles: Vec<String> = m.items().into_iter().map(|i| i.title).collect();
+        assert_eq!(
+            titles,
+            [
+                "agentic-dev@vocdoni",
+                "kb (http https://kb.example.org/mcp)",
+                "notes (sse https://notes.example.org/sse)",
+                "local (stdio npx)",
+                "reviewer: Reviews code",
+                "instructions",
+                "env (1 keys)",
+            ]
+        );
+        assert!(m.ids().iter().all(|id| id_ok(id)));
+    }
+
+    #[test]
+    fn the_teams_draft_manifest_parses() {
+        let src = include_str!("testdata/vocdoni-internal.toml");
+        let m = Manifest::parse_named(src, "vocdoni-internal").unwrap();
+        assert_eq!(
+            (m.plugins.len(), m.servers.len(), m.agents.len()),
+            (13, 3, 3)
+        );
+        assert!(agents_json(&m.agents).unwrap().len() <= MAX_AGENTS_JSON);
+        let err = Manifest::parse_named(src, "other").unwrap_err();
+        assert_eq!(err, "the manifest is vocdoni-internal's");
+    }
+
+    #[test]
+    fn the_mcp_json_has_each_type_as_claude_code_expects() {
+        let m = good();
+        let v: Value = serde_json::from_str(&mcp_json(&m.servers).unwrap()).unwrap();
+        assert_eq!(
+            v,
+            json!({"mcpServers": {
+                "kb": {"type": "http", "url": "https://kb.example.org/mcp",
+                       "headers": {"Authorization": "Bearer ${KB_TOKEN}"}},
+                "notes": {"type": "sse", "url": "https://notes.example.org/sse"},
+                "local": {"type": "stdio", "command": "npx",
+                          "args": ["-y", "server", "--region=${REGION}"],
+                          "env": {"API_KEY": "${LOCAL_KEY}"}},
+            }})
+        );
+        assert_eq!(mcp_json(&[]), None);
+        let only_kb = mcp_json(m.servers.iter().filter(|s| s.name == "kb")).unwrap();
+        assert!(!only_kb.contains("notes"), "{only_kb}");
+    }
+
+    #[test]
+    fn the_agents_json_has_the_optional_fields_only_when_given() {
+        let mut m = good();
+        let v: Value = serde_json::from_str(&agents_json(&m.agents).unwrap()).unwrap();
+        assert_eq!(
+            v,
+            json!({"reviewer": {"description": "Reviews code", "prompt": "You review code.",
+                                "model": "opus", "tools": ["Read", "Grep", "mcp__kb__*"]}})
+        );
+        m.agents[0].model = None;
+        m.agents[0].tools = None;
+        let v: Value = serde_json::from_str(&agents_json(&m.agents).unwrap()).unwrap();
+        assert_eq!(
+            v,
+            json!({"reviewer": {"description": "Reviews code", "prompt": "You review code."}})
+        );
+        assert_eq!(agents_json(&[]), None);
+    }
+
+    #[test]
+    fn format_and_toml_errors_name_no_value() {
+        assert_eq!(refused("format = 1", "format = 2"), "format is not 1");
+        assert_eq!(refused("format = 1", "format = \"1\""), "format is not 1");
+        assert_eq!(refused("format = 1\n", ""), "missing key format");
+        // A newer format's keys do not matter: the format is refused first.
+        let newer = GOOD.replacen("format = 1", "format = 2\nfuture = true", 1);
+        assert_eq!(Manifest::parse(&newer).unwrap_err(), "format is not 1");
+        let err = refused("version = \"1.0.0\"", "version = \"1.0.0");
+        assert!(err.starts_with("not valid TOML"), "{err}");
+        assert!(err.ends_with("(line 4)"), "{err}");
+        assert!(!err.contains("1.0.0"), "{err}");
+        let err = refused("model = \"opus\"", "model = 5");
+        assert!(
+            err.starts_with("not valid TOML, or a value of the wrong type"),
+            "{err}"
+        );
+        assert_eq!(
+            Manifest::parse(&"#".repeat(MAX_BYTES + 1)).unwrap_err(),
+            "over 256 KiB"
+        );
+    }
+
+    #[test]
+    fn unknown_and_missing_keys_are_refused_by_name() {
+        let err = refused(
+            "version = \"1.0.0\"",
+            "version = \"1.0.0\"\nhomepage = \"x\"",
+        );
+        assert!(err.starts_with("unknown key homepage"), "{err}");
+        let err = refused("model = \"opus\"", "model = \"opus\"\ncolor = \"red\"");
+        assert!(err.starts_with("unknown key color"), "{err}");
+        let err = refused("[env]", "[extra]\nx = 1\n[env]");
+        assert!(err.starts_with("unknown key extra"), "{err}");
+        let err = refused("name = \"team\"\n", "");
+        assert!(err.starts_with("missing key name"), "{err}");
+        let err = refused("prompt = \"You review code.\"\n", "");
+        assert!(err.starts_with("missing key prompt"), "{err}");
+    }
+
+    #[test]
+    fn top_level_caps_and_charsets() {
+        assert_eq!(refused("name = \"team\"", "name = \"Team\""), "bad name");
+        assert_eq!(
+            refused("version = \"1.0.0\"", "version = \"\""),
+            "bad version"
+        );
+        let long = format!("version = \"{}\"", "1".repeat(65));
+        assert_eq!(refused("version = \"1.0.0\"", &long), "bad version");
+        let long = format!("description = \"{}\"", "d".repeat(201));
+        assert_eq!(
+            refused("description = \"Team setup\"", &long),
+            "description over 200 bytes"
+        );
+        assert_eq!(
+            refused(
+                "description = \"Team setup\"",
+                "description = \"a\\u0007b\""
+            ),
+            "description: control character"
+        );
+        assert_eq!(
+            refused(
+                "Search the knowledge base first.",
+                &"i".repeat(MAX_INSTRUCTIONS)
+            ),
+            "instructions over 8192 bytes"
+        );
+    }
+
+    #[test]
+    fn env_keys_follow_the_key_rule_and_values_are_text() {
+        assert_eq!(
+            refused("REGION = \"eu\"", "LD_PRELOAD = \"x\""),
+            "env: env key LD_PRELOAD not allowed"
+        );
+        assert_eq!(
+            refused("REGION = \"eu\"", "region = \"x\""),
+            "env: bad env key"
+        );
+        let long = format!("REGION = \"{}\"", "v".repeat(MAX_VALUE + 1));
+        assert_eq!(
+            refused("REGION = \"eu\"", &long),
+            "env REGION over 4096 bytes"
+        );
+        let many: String = (0..65).map(|i| format!("K{i} = \"v\"\n")).collect();
+        assert_eq!(refused("REGION = \"eu\"", &many), "env: more than 64 keys");
+    }
+
+    #[test]
+    fn marketplace_sources_and_names() {
+        for (source, ok) in [
+            ("github:vocdoni/skills", true),
+            ("github:anthropics/claude-plugins-official", true),
+            ("https://example.org/m.git", true),
+            ("github:vocdoni", false),
+            ("github:a/b/c", false),
+            ("github:-x/skills", false),
+            ("github:../skills", false),
+            ("github:vocdoni/..", false),
+            ("github:voc doni/skills", false),
+            ("http://example.org/m.git", false),
+            ("file:///etc", false),
+            ("/home/me/market", false),
+            ("git@github.com:a/b.git", false),
+            ("https://", false),
+        ] {
+            let src = GOOD.replacen("github:vocdoni/skills", source, 1);
+            match Manifest::parse(&src) {
+                Ok(_) => assert!(ok, "{source} accepted"),
+                Err(e) => {
+                    assert!(!ok, "{source}: {e}");
+                    assert_eq!(e, "marketplace vocdoni: bad source");
+                }
+            }
+        }
+        let long = format!("source = \"https://e.org/{}\"", "a".repeat(500));
+        assert_eq!(
+            refused("source = \"https://example.org/market.git\"", &long),
+            "marketplace other: bad source"
+        );
+        assert_eq!(
+            refused("name = \"other\"", "name = \"vocdoni\""),
+            "marketplace vocdoni: given twice"
+        );
+        assert_eq!(
+            refused("name = \"other\"", "name = \"Other\""),
+            "marketplace #2: bad name"
+        );
+        let many: String = (0..17)
+            .map(|i| format!("[[marketplace]]\nname = \"m{i}\"\nsource = \"github:a/b\"\n"))
+            .collect();
+        assert_eq!(refused_with(&many), "more than 16 marketplaces");
+    }
+
+    #[test]
+    fn plugins_need_a_declared_marketplace_and_unique_names() {
+        assert_eq!(
+            refused("marketplace = \"vocdoni\"", "marketplace = \"nope\""),
+            "plugin:agentic-dev@nope: marketplace nope not declared"
+        );
+        assert_eq!(
+            refused_with("[[plugin]]\nname = \"agentic-dev\"\nmarketplace = \"other\""),
+            "plugin agentic-dev: given twice"
+        );
+        assert_eq!(
+            refused("name = \"agentic-dev\"", "name = \"Agentic\""),
+            "plugin #1: bad name"
+        );
+        let many: String = (0..64)
+            .map(|i| format!("[[plugin]]\nname = \"p{i}\"\nmarketplace = \"vocdoni\"\n"))
+            .collect();
+        assert_eq!(refused_with(&many), "more than 64 plugins");
+    }
+
+    #[test]
+    fn remote_servers_are_https_with_token_headers() {
+        let kb_url = "url = \"https://kb.example.org/mcp\"";
+        assert_eq!(
+            refused(kb_url, "url = \"http://kb.example.org/mcp\""),
+            "mcp:kb: url is not https"
+        );
+        let long = format!("url = \"https://e.org/{}\"", "a".repeat(500));
+        assert_eq!(refused(kb_url, &long), "mcp:kb: url is not https");
+        assert_eq!(
+            refused("{ Authorization =", "{ \"Bad Name\" ="),
+            "mcp:kb: bad header name"
+        );
+        assert_eq!(
+            refused("\"Bearer ${KB_TOKEN}\"", "\"Bearer\\n${KB_TOKEN}\""),
+            "mcp:kb: header Authorization: bad value"
+        );
+        assert_eq!(
+            refused("type = \"http\"", "type = \"ws\""),
+            "mcp:kb: type is not http, sse or stdio"
+        );
+        assert_eq!(
+            refused(kb_url, &format!("{kb_url}\ncommand = \"x\"")),
+            "mcp:kb: command, args or env on an http server"
+        );
+        assert_eq!(refused(kb_url, ""), "mcp:kb: missing key url");
+        let many: String = (0..17).map(|i| format!("H{i} = \"v\", ")).collect();
+        assert_eq!(
+            refused("{ Authorization =", &format!("{{ {many}Authorization =")),
+            "mcp:kb: more than 16 headers"
+        );
+    }
+
+    #[test]
+    fn oauth_servers_take_no_headers_or_secrets() {
+        let oauth = "auth = \"oauth\"";
+        assert_eq!(
+            refused(oauth, "auth = \"basic\""),
+            "mcp:notes: auth is not oauth"
+        );
+        assert_eq!(
+            refused(oauth, &format!("{oauth}\nheaders = {{ X = \"y\" }}")),
+            "mcp:notes: auth = oauth takes no headers or secrets"
+        );
+        assert_eq!(
+            refused(oauth, &format!("{oauth}\nsecrets = [\"NOTES_KEY\"]")),
+            "mcp:notes: auth = oauth takes no headers or secrets"
+        );
+    }
+
+    #[test]
+    fn stdio_servers_have_a_command_and_text_args() {
+        assert_eq!(
+            refused(
+                "command = \"npx\"",
+                "command = \"npx\"\nurl = \"https://e.org\""
+            ),
+            "mcp:local: url, headers or auth on a stdio server"
+        );
+        assert_eq!(
+            refused("command = \"npx\"\n", ""),
+            "mcp:local: missing key command"
+        );
+        assert_eq!(
+            refused("command = \"npx\"", "command = \"\""),
+            "mcp:local: bad command"
+        );
+        assert_eq!(
+            refused("command = \"npx\"", "command = \"${LOCAL_KEY}\""),
+            "mcp:local: command: bad placeholder"
+        );
+        assert_eq!(
+            refused("\"-y\",", "\"a\\u0000\","),
+            "mcp:local: arg: control character"
+        );
+        let long = format!("\"{}\",", "a".repeat(MAX_ARG + 1));
+        assert_eq!(refused("\"-y\",", &long), "mcp:local: arg over 1024 bytes");
+        let many: String = (0..32).map(|i| format!("\"a{i}\", ")).collect();
+        assert_eq!(
+            refused("args = [", &format!("args = [{many}")),
+            "mcp:local: more than 32 args"
+        );
+        assert_eq!(
+            refused("API_KEY =", "HOME ="),
+            "mcp:local: env: env key HOME not allowed"
+        );
+    }
+
+    #[test]
+    fn placeholders_must_be_declared_secrets_or_env() {
+        assert_eq!(
+            refused("secrets = [\"KB_TOKEN\"]", "secrets = []"),
+            "mcp:kb: headers: ${KB_TOKEN} is not in secrets or [env]"
+        );
+        assert_eq!(
+            refused("REGION = \"eu\"", "OTHER = \"eu\""),
+            "mcp:local: args: ${REGION} is not in secrets or [env]"
+        );
+        for bad in [
+            "${kb_token}",
+            "${KB_TOKEN",
+            "${KB_TOKEN:-x}",
+            "${HOME}",
+            "${}",
+        ] {
+            let to = format!("\"Bearer {bad}\"");
+            assert_eq!(
+                refused("\"Bearer ${KB_TOKEN}\"", &to),
+                "mcp:kb: headers: bad placeholder",
+                "{bad}"
+            );
+        }
+        assert_eq!(
+            refused("/mcp\"", "/${KB_TOKEN}/${X}\""),
+            "mcp:kb: url: ${X} is not in secrets or [env]"
+        );
+        assert_eq!(
+            refused("${LOCAL_KEY}\" }", "${NOPE}\" }"),
+            "mcp:local: env: ${NOPE} is not in secrets or [env]"
+        );
+        assert_eq!(
+            refused("secrets = [\"KB_TOKEN\"]", "secrets = [\"PATH\"]"),
+            "mcp:kb: secrets: env key PATH not allowed"
+        );
+    }
+
+    #[test]
+    fn servers_and_agents_have_unique_names_and_caps() {
+        assert_eq!(
+            refused("name = \"notes\"", "name = \"kb\""),
+            "mcp:kb: given twice"
+        );
+        assert_eq!(
+            refused("name = \"notes\"", "name = \"reviewer\""),
+            "mcp:reviewer: an agent has this name"
+        );
+        assert_eq!(
+            refused_with("[[agent]]\nname = \"reviewer\"\ndescription = \"d\"\nprompt = \"p\""),
+            "agent:reviewer: given twice"
+        );
+        let many: String = (0..30)
+            .map(|i| format!("[[mcp]]\nname = \"s{i}\"\ntype = \"sse\"\nurl = \"https://e.org\"\n"))
+            .collect();
+        assert_eq!(refused_with(&many), "more than 32 MCP servers");
+        let many: String = (0..32)
+            .map(|i| format!("[[agent]]\nname = \"a{i}\"\ndescription = \"d\"\nprompt = \"p\"\n"))
+            .collect();
+        assert_eq!(refused_with(&many), "more than 32 agents");
+        let secrets: Vec<String> = (0..17).map(|i| format!("\"S{i}\"")).collect();
+        assert_eq!(
+            refused(
+                "secrets = [\"KB_TOKEN\"]",
+                &format!("secrets = [\"KB_TOKEN\", {}]", secrets.join(", "))
+            ),
+            "mcp:kb: more than 16 secrets"
+        );
+    }
+
+    #[test]
+    fn agents_have_text_and_tool_and_model_charsets() {
+        assert_eq!(
+            refused("model = \"opus\"", "model = \"op us\""),
+            "agent:reviewer: bad model"
+        );
+        assert_eq!(
+            refused("\"Grep\",", "\"Gr ep\","),
+            "agent:reviewer: bad tool"
+        );
+        let tools: Vec<String> = (0..33).map(|i| format!("\"T{i}\"")).collect();
+        assert_eq!(
+            refused(
+                "tools = [\"Read\", \"Grep\", \"mcp__kb__*\"]",
+                &format!("tools = [{}]", tools.join(", "))
+            ),
+            "agent:reviewer: more than 32 tools"
+        );
+        assert_eq!(
+            refused("prompt = \"You review code.\"", "prompt = \"  \""),
+            "agent:reviewer: prompt is empty"
+        );
+        assert_eq!(
+            refused("description = \"Reviews code\"", "description = \"\""),
+            "agent:reviewer: description is empty"
+        );
+        let long = format!("prompt = \"{}\"", "p".repeat(MAX_PROMPT + 1));
+        assert_eq!(
+            refused("prompt = \"You review code.\"", &long),
+            "agent:reviewer: prompt over 16384 bytes"
+        );
+        let long = format!("description = \"{}\"", "d".repeat(501));
+        assert_eq!(
+            refused("description = \"Reviews code\"", &long),
+            "agent:reviewer: description over 500 bytes"
+        );
+    }
+
+    #[test]
+    fn the_rendered_caps_are_the_rule() {
+        // Three prompts at their own cap: each fits, the JSON does not.
+        let agents: String = (0..3)
+            .map(|i| {
+                format!(
+                    "[[agent]]\nname = \"big{i}\"\ndescription = \"d\"\nprompt = \"{}\"\n",
+                    "p".repeat(MAX_PROMPT - 100)
+                )
+            })
+            .collect();
+        assert_eq!(refused_with(&agents), "agents JSON over 32 KiB");
+        // Sixteen 4 KiB headers on two servers: over 64 KiB rendered.
+        let headers: Vec<String> = (0..16)
+            .map(|i| format!("H{i} = \"{}\"", "v".repeat(MAX_VALUE - 10)))
+            .collect();
+        let servers: String = (0..2)
+            .map(|i| {
+                format!(
+                    "[[mcp]]\nname = \"big{i}\"\ntype = \"http\"\nurl = \"https://e.org\"\nheaders = {{ {} }}\n",
+                    headers.join(", ")
+                )
+            })
+            .collect();
+        assert_eq!(refused_with(&servers), "MCP JSON over 64 KiB");
+    }
+
+    #[test]
+    fn ids_follow_the_grammar() {
+        for id in [
+            "plugin:a@b",
+            "mcp:kb",
+            "agent:reviewer",
+            "instructions",
+            "env",
+        ] {
+            assert!(id_ok(id), "{id}");
+        }
+        let long = format!("plugin:{}@{}", "a".repeat(40), "b".repeat(50));
+        for id in [
+            "plugin:a",
+            "plugin:A@b",
+            "mcp:",
+            "agent:x y",
+            "skill:x",
+            "",
+            "ENV",
+            long.as_str(),
+        ] {
+            assert!(!id_ok(id), "{id}");
+        }
+    }
+
+    #[test]
+    fn an_empty_manifest_has_no_items() {
+        let m = Manifest::parse("format = 1\nname = \"x\"\nversion = \"1\"").unwrap();
+        assert!(m.items().is_empty());
+        assert_eq!((mcp_json(&m.servers), agents_json(&m.agents)), (None, None));
+    }
+}
diff --git a/src/plugins/mod.rs b/src/plugins/mod.rs
new file mode 100644
index 0000000..d84fba7
--- /dev/null
+++ b/src/plugins/mod.rs
@@ -0,0 +1,6 @@
+//! Proxy plugins as manifests: what a member's sessions get from a plugin
+//! the proxy serves. [`manifest`] is the one parser, shared by the client
+//! (which shows the items and keeps the member's choices) and the daemon
+//! (which applies them on its host).
+
+pub mod manifest;
diff --git a/src/plugins/testdata/vocdoni-internal.toml b/src/plugins/testdata/vocdoni-internal.toml
new file mode 100644
index 0000000..2785363
--- /dev/null
+++ b/src/plugins/testdata/vocdoni-internal.toml
@@ -0,0 +1,189 @@
+# vocdoni-internal — claudio plugin manifest
+# Served by claude.vocdoni.net to every member's claudio; applied on each host.
+format = 1
+name = "vocdoni-internal"
+version = "1.0.0"
+description = "Vocdoni team setup: skills, MCP servers and subagents"
+
+instructions = """
+You are working for Vocdoni. The vocdoni-kb MCP server is the team knowledge base:
+search it before asking a colleague, and add to it what you learn that others will
+need. Non-trivial engineering work runs as a controlled delivery with the agentic-dev
+plugin. Commit messages and PR descriptions never carry AI attribution.
+"""
+
+[[marketplace]]
+name = "vocdoni"
+source = "github:vocdoni/skills"
+
+[[marketplace]]
+name = "claude-plugins-official"
+source = "github:anthropics/claude-plugins-official"
+
+[[plugin]]
+name = "vocdoni-go"
+marketplace = "vocdoni"
+
+[[plugin]]
+name = "vocdoni-comments"
+marketplace = "vocdoni"
+
+[[plugin]]
+name = "agentic-dev"
+marketplace = "vocdoni"
+
+[[plugin]]
+name = "vocdoni-internal-oauth"
+marketplace = "vocdoni"
+
+[[plugin]]
+name = "vocdoni-ui-style"
+marketplace = "vocdoni"
+
+[[plugin]]
+name = "vocdoni-stripe"
+marketplace = "vocdoni"
+
+[[plugin]]
+name = "vocdoni-sdk"
+marketplace = "vocdoni"
+
+[[plugin]]
+name = "davinci-sdk"
+marketplace = "vocdoni"
+
+[[plugin]]
+name = "vocdoni-integrator-sdk"
+marketplace = "vocdoni"
+
+[[plugin]]
+name = "pi-subagent"
+marketplace = "vocdoni"
+
+[[plugin]]
+name = "firecrawl"
+marketplace = "claude-plugins-official"
+
+[[plugin]]
+name = "context7"
+marketplace = "claude-plugins-official"
+
+[[plugin]]
+name = "remember"
+marketplace = "claude-plugins-official"
+
+[[mcp]]
+name = "vocdoni-kb"
+type = "http"
+url = "https://kb-gw.vocdoni.net/mcp"
+headers = { Authorization = "Bearer ${VOCDONI_KB_TOKEN}" }
+secrets = ["VOCDONI_KB_TOKEN"]
+
+[[mcp]]
+name = "jina-mcp-server"
+type = "http"
+url = "https://mcp.jina.ai/v1"
+headers = { Authorization = "Bearer ${JINA_API_KEY}" }
+secrets = ["JINA_API_KEY"]
+
+[[mcp]]
+name = "hackmd"
+type = "http"
+url = "https://mcp.hackmd.io/"
+auth = "oauth"
+
+[[agent]]
+name = "reviewer"
+description = "Read-only GPT Astra code reviewer and auditor. Use for evidence-backed reviews of specified changes or audits of a clearly scoped component, focusing on correctness, security, reliability, and meaningful test gaps. Reports findings without implementing fixes."
+model = "claude-gpt-6-astra"
+tools = ["Read", "Grep", "Glob"]
+prompt = """
+You are a code reviewer and auditor. Inspect the assigned scope and return actionable, evidence-backed findings. Do not modify files or delegate work.
+
+## Scope
+
+- Follow the applicable repository instructions and the caller's review objective.
+- Work from the supplied diff, file list, or component. If a change review lacks its diff or comparison baseline, ask the caller to supply it; do not guess what changed.
+- Read surrounding code and relevant callers to establish behavior. Separate defects introduced by a change from pre-existing issues.
+- You have read-only file tools, not a shell. Request command output from the caller when a diff, test result, or runtime observation is needed.
+
+## Review method
+
+- Prioritize incorrect behavior, data integrity, authorization and trust boundaries, concurrency, resource handling, and failure recovery.
+- Check tests against the important behavior and edge cases, rather than treating coverage numbers as proof of correctness.
+- Challenge each potential finding against the implementation and its actual callers. Identify the concrete conditions that trigger the problem and the resulting impact.
+- Do not present stylistic preferences, speculative concerns, or unverified assumptions as confirmed defects. Keep unresolved concerns separate from findings.
+- Recommend the smallest appropriate correction; leave implementation to the caller.
+
+## Report
+
+List findings in descending severity. For each, provide:
+
+1. Severity and a concise title.
+2. The relevant `file_path:line_number`.
+3. The triggering conditions, evidence, and impact.
+4. A minimal recommended fix and, when useful, a regression test.
+
+Finish with verification limits and any open questions. If no findings survive verification, say so explicitly without claiming the code is proven safe. Never claim to have executed tests or commands.
+"""
+
+[[agent]]
+name = "glm-worker"
+description = "GLM 5.3 implementation worker for specific, well-scoped tasks with clear requirements and acceptance criteria. Use for focused bug fixes, small features, test additions, or bounded refactors following an established design. Not for open-ended architecture or project-wide changes."
+model = "claude-glm-5.3"
+tools = ["Read", "Grep", "Glob", "Edit", "Write", "Bash"]
+prompt = """
+You are an implementation worker. Complete the specific task assigned by the caller with the smallest correct, verified change. Do not delegate work or invent additional goals.
+
+## Working boundaries
+
+- Follow applicable repository instructions and preserve existing conventions.
+- Establish the requested behavior, permitted scope, and acceptance criteria before editing. Resolve factual questions by reading the relevant code; return material ambiguities or missing design decisions to the caller.
+- Inspect the current state before changing files. Preserve unrelated work, including changes made by other agents.
+- Stay within the assigned task. Do not introduce new architecture, broad cleanup, unrelated refactoring, or speculative abstractions.
+- Do not commit, push, deploy, modify user-level configuration, or install dependencies unless that action is explicitly part of the assigned task and permitted.
+
+## Execution
+
+1. Read the relevant implementation, callers, and tests. Reuse existing helpers and patterns where appropriate.
+2. Make a focused change that satisfies the requested behavior while preserving unrelated behavior and public contracts.
+3. Add or update targeted tests when behavior changes. For a bug fix, cover the triggering case where practical.
+4. Run the most relevant available checks and inspect their actual results. Expand verification when the change's dependencies warrant it.
+5. Review the final diff for accidental edits, incomplete changes, and scope expansion.
+
+If the task requires a broader design decision or cannot be completed safely within scope, stop and report the blocker rather than improvising.
+
+## Handoff
+
+Return a concise summary of the changes and affected files, the checks run with their results, and any remaining risks or blockers. State clearly when work is partial, tests fail, or validation could not be performed. Do not claim success from an unexecuted check.
+"""
+
+[[agent]]
+name = "qwen-worker"
+description = "Qwen3.8 27B worker for small, straightforward tasks with explicit instructions and a narrow file scope. Use for documentation edits, mechanical updates, simple tests, and localized fixes with a known cause. Do not use for architecture, complex debugging, authentication/authorization, cryptography, database migrations, concurrency, or protocol design."
+model = "claude-openai-qwen38-27b"
+tools = ["Read", "Grep", "Glob", "Edit", "Write", "Bash"]
+prompt = """
+You are a worker for small, clearly specified tasks. Follow the supplied approach, make minimal changes, and verify the result. Do not delegate work.
+
+## Task limits
+
+- Accept tasks with an explicit outcome, a narrow scope, and an established implementation pattern.
+- Do not undertake architectural decisions, open-ended debugging, cross-cutting refactors, authentication or authorization changes, cryptography, database migrations, concurrency, or protocol design.
+- If the request or discovered code exceeds these limits, stop and explain what the caller needs to resolve or assign to a more capable agent. Do not expand the task yourself.
+
+## Execution
+
+1. Read the applicable repository instructions and the files relevant to the task.
+2. Check the existing implementation and nearby examples. Ask the caller about material ambiguity rather than guessing requirements.
+3. Apply only the requested change. Preserve unrelated edits, public behavior, and existing formatting conventions.
+4. Run the nearest relevant test or check when available. Inspect the actual output and correct only problems caused by your change.
+5. Review the diff to ensure it contains no unrelated edits.
+
+Do not commit, push, deploy, modify user-level configuration, or install dependencies unless explicitly required by the assigned task and permitted.
+
+## Handoff
+
+Briefly report what changed, which files were affected, and what was checked. Include exact failures or blockers when present. If validation was not possible, state that plainly; do not claim that tests passed or the task is fully verified.
+"""
+
`````
Run: `umask 022; cargo test --locked --bin claudio plugins::manifest`
Expected: the build fails, e.g. `` error[E0433]: failed to resolve: use of undeclared type `Manifest` `` and `` error[E0425]: cannot find function `mcp_json` in this scope ``, ending `` error: could not compile `claudio` (bin "claudio" test) due to 36 previous errors; 1 warning emitted ``

Step 2: implement (`key_ok` becomes `pub`)
`````diff
diff --git a/src/plugins/manifest.rs b/src/plugins/manifest.rs
index 08b0d20..d73f300 100644
--- a/src/plugins/manifest.rs
+++ b/src/plugins/manifest.rs
@@ -1,3 +1,823 @@
+//! A plugin manifest: the TOML file a proxy plugin's `bundle` points at. It
+//! says what a member's sessions get from the plugin: marketplaces and the
+//! plugins to install from them, MCP servers (with the secret env each
+//! needs), subagents, instructions for every session's context, and env.
+//!
+//! A manifest is untrusted input. [`Manifest::parse`] refuses anything
+//! outside the grammar below, with a reason that names ids and fields,
+//! never a value:
+//!
+//! - `format = 1`, `name` (`[a-z0-9-]{1,40}`), `version` (≤ 64 printable
+//!   ASCII), `description` (text ≤ 200), `instructions` (text ≤ 8 KiB);
+//! - `[[marketplace]]` ≤ 16: `name`, `source` (`github:<owner>/<repo>`,
+//!   each `[A-Za-z0-9_.-]{1,64}`, or `https://…` ≤ 512 printable ASCII);
+//! - `[[plugin]]` ≤ 64: `name`, `marketplace` (one of the manifest's);
+//! - `[[mcp]]` ≤ 32: `name`, `type` (`http`, `sse` or `stdio`), `url` and
+//!   `headers` (≤ 16, RFC 7230 token names ≤ 64, printable ASCII values ≤
+//!   4 KiB) and `auth = "oauth"` for http/sse, `command` (≤ 256 printable
+//!   ASCII), `args` (≤ 32 × text ≤ 1 KiB) and `env` (≤ 16) for stdio, and
+//!   `secrets` (≤ 16 env names);
+//! - `[[agent]]` ≤ 32: `name`, `description` (text ≤ 500), `model`
+//!   (`[A-Za-z0-9._-]{1,64}`), `tools` (≤ 32 × `[A-Za-z0-9_:*()-]{1,64}`),
+//!   `prompt` (text ≤ 16 KiB);
+//! - `[env]` ≤ 64 keys.
+//!
+//! "Text" is UTF-8 without C0 controls but `\n` and `\t`. Names are
+//! `[a-z0-9-]{1,40}`; env keys follow [`key_ok`]. Unknown keys, a name given
+//! twice in a section, and an MCP server named like an agent are refused.
+//! `${NAME}` in a server's `url`, `headers`, `args` and `env` is left for
+//! Claude Code to expand from the session env: `NAME` follows [`key_ok`] and
+//! is one of the server's `secrets` or a key of `[env]`; any other `${` is
+//! refused. The renderers run on every item at parse time: an agents JSON
+//! over 32 KiB or an MCP JSON over 64 KiB refuses the manifest.
+//!
+//! Every item has an id the member's choices and the hosts' reports use:
+//! `plugin:<name>@<marketplace>`, `mcp:<name>`, `agent:<name>`,
+//! `instructions` and `env`. A marketplace has none: it is added when one
+//! of its plugins is.
+
+use std::collections::BTreeMap;
+
+use serde::Deserialize;
+use serde_json::{json, Map, Value};
+
+use crate::proxy::plugins::{key_ok, name_ok};
+
+/// A manifest's size cap, in bytes.
+pub const MAX_BYTES: usize = 256 << 10;
+/// The instructions' cap, in bytes.
+pub const MAX_INSTRUCTIONS: usize = 8 << 10;
+/// The rendered `--agents` JSON's cap, in bytes.
+pub const MAX_AGENTS_JSON: usize = 32 << 10;
+/// The rendered `--mcp-config` JSON's cap, in bytes.
+pub const MAX_MCP_JSON: usize = 64 << 10;
+/// An item id's cap, in bytes.
+pub const MAX_ID: usize = 96;
+
+const MAX_DESCRIPTION: usize = 200;
+const MAX_MARKETPLACES: usize = 16;
+const MAX_PLUGINS: usize = 64;
+const MAX_SERVERS: usize = 32;
+const MAX_AGENTS: usize = 32;
+const MAX_ENV: usize = 64;
+const MAX_VALUE: usize = 4096;
+const MAX_URL: usize = 512;
+const MAX_HEADERS: usize = 16;
+const MAX_SECRETS: usize = 16;
+const MAX_COMMAND: usize = 256;
+const MAX_ARGS: usize = 32;
+const MAX_ARG: usize = 1024;
+const MAX_SERVER_ENV: usize = 16;
+const MAX_AGENT_DESCRIPTION: usize = 500;
+const MAX_PROMPT: usize = 16 << 10;
+const MAX_TOOLS: usize = 32;
+
+/// A parsed, checked manifest.
+#[derive(Debug, Clone, PartialEq)]
+pub struct Manifest {
+    pub name: String,
+    pub version: String,
+    pub description: String,
+    pub instructions: String,
+    pub marketplaces: Vec<Marketplace>,
+    pub plugins: Vec<PluginRef>,
+    pub servers: Vec<Server>,
+    pub agents: Vec<Agent>,
+    /// `[env]`, sorted by key.
+    pub env: Vec<(String, String)>,
+}
+
+/// A marketplace: the name it declares for itself (the key Claude Code
+/// keeps it under) and where it is.
+#[derive(Debug, Clone, PartialEq)]
+pub struct Marketplace {
+    pub name: String,
+    /// As written: `github:<owner>/<repo>` or an https URL.
+    pub source: String,
+}
+
+impl Marketplace {
+    /// The source as `claude plugin marketplace add` takes it:
+    /// `<owner>/<repo>` for a GitHub one, the URL otherwise.
+    pub fn cli_source(&self) -> &str {
+        self.source.strip_prefix("github:").unwrap_or(&self.source)
+    }
+}
+
+/// A plugin to install from one of the manifest's marketplaces.
+#[derive(Debug, Clone, PartialEq)]
+pub struct PluginRef {
+    pub name: String,
+    pub marketplace: String,
+}
+
+impl PluginRef {
+    /// `<name>@<marketplace>`: Claude Code's id.
+    pub fn spec(&self) -> String {
+        format!("{}@{}", self.name, self.marketplace)
+    }
+
+    /// `plugin:<name>@<marketplace>`.
+    pub fn id(&self) -> String {
+        format!("plugin:{}", self.spec())
+    }
+}
+
+/// An MCP server every session gets.
+#[derive(Debug, Clone, PartialEq)]
+pub struct Server {
+    pub name: String,
+    pub transport: Transport,
+    /// The env names (never values) the server needs from the proxy.
+    pub secrets: Vec<String>,
+}
+
+#[derive(Debug, Clone, PartialEq)]
+pub enum Transport {
+    Http {
+        url: String,
+        headers: Vec<(String, String)>,
+        /// `auth = "oauth"`: the member signs in with `/mcp`.
+        oauth: bool,
+    },
+    Sse {
+        url: String,
+        headers: Vec<(String, String)>,
+        oauth: bool,
+    },
+    Stdio {
+        command: String,
+        args: Vec<String>,
+        env: Vec<(String, String)>,
+    },
+}
+
+impl Server {
+    /// `mcp:<name>`.
+    pub fn id(&self) -> String {
+        format!("mcp:{}", self.name)
+    }
+
+    /// `http`, `sse` or `stdio`.
+    pub fn kind(&self) -> &'static str {
+        match self.transport {
+            Transport::Http { .. } => "http",
+            Transport::Sse { .. } => "sse",
+            Transport::Stdio { .. } => "stdio",
+        }
+    }
+
+    /// The server as Claude Code's MCP config has it.
+    fn config(&self) -> Value {
+        let map = |pairs: &[(String, String)]| {
+            pairs
+                .iter()
+                .map(|(k, v)| (k.clone(), Value::String(v.clone())))
+                .collect::<Map<String, Value>>()
+        };
+        match &self.transport {
+            Transport::Http { url, headers, .. } | Transport::Sse { url, headers, .. } => {
+                let mut v = json!({ "type": self.kind(), "url": url });
+                if !headers.is_empty() {
+                    v["headers"] = Value::Object(map(headers));
+                }
+                v
+            }
+            Transport::Stdio { command, args, env } => {
+                let mut v = json!({ "type": "stdio", "command": command, "args": args });
+                if !env.is_empty() {
+                    v["env"] = Value::Object(map(env));
+                }
+                v
+            }
+        }
+    }
+}
+
+/// A subagent every session gets (`--agents`).
+#[derive(Debug, Clone, PartialEq)]
+pub struct Agent {
+    pub name: String,
+    pub description: String,
+    pub model: Option<String>,
+    pub tools: Option<Vec<String>>,
+    pub prompt: String,
+}
+
+impl Agent {
+    /// `agent:<name>`.
+    pub fn id(&self) -> String {
+        format!("agent:{}", self.name)
+    }
+}
+
+/// Where an item is listed.
+#[derive(Debug, Clone, Copy, PartialEq, Eq)]
+pub enum Section {
+    Plugins,
+    Servers,
+    Agents,
+    Instructions,
+    Env,
+}
+
+impl Section {
+    pub fn heading(self) -> &'static str {
+        match self {
+            Section::Plugins => "Plugins",
+            Section::Servers => "MCP servers",
+            Section::Agents => "Agents",
+            Section::Instructions => "Instructions",
+            Section::Env => "Env",
+        }
+    }
+}
+
+/// One thing the member may turn off: its id, section and title.
+#[derive(Debug, Clone, PartialEq)]
+pub struct Item {
+    pub id: String,
+    pub section: Section,
+    pub title: String,
+}
+
+impl Manifest {
+    /// Parse and check `src` (see the module doc). The reason names ids
+    /// and fields, never a value.
+    pub fn parse(src: &str) -> Result<Manifest, String> {
+        if src.len() > MAX_BYTES {
+            return Err("over 256 KiB".to_owned());
+        }
+        // `format` first: a newer format may have keys this one lacks.
+        let table: toml::Table = toml::from_str(src).map_err(|e| toml_reason(src, &e))?;
+        match table.get("format") {
+            Some(toml::Value::Integer(1)) => {}
+            Some(_) => return Err("format is not 1".to_owned()),
+            None => return Err("missing key format".to_owned()),
+        }
+        let raw: Raw = toml::from_str(src).map_err(|e| toml_reason(src, &e))?;
+        let manifest = raw.check()?;
+        let all_agents = agents_json(&manifest.agents).unwrap_or_default();
+        if all_agents.len() > MAX_AGENTS_JSON {
+            return Err("agents JSON over 32 KiB".to_owned());
+        }
+        let all_servers = mcp_json(&manifest.servers).unwrap_or_default();
+        if all_servers.len() > MAX_MCP_JSON {
+            return Err("MCP JSON over 64 KiB".to_owned());
+        }
+        Ok(manifest)
+    }
+
+    /// [`Manifest::parse`], and the manifest must be plugin `name`'s.
+    pub fn parse_named(src: &str, name: &str) -> Result<Manifest, String> {
+        let manifest = Manifest::parse(src)?;
+        if manifest.name != name {
+            return Err(format!("the manifest is {}'s", manifest.name));
+        }
+        Ok(manifest)
+    }
+
+    /// The marketplace called `name`.
+    pub fn marketplace(&self, name: &str) -> Option<&Marketplace> {
+        self.marketplaces.iter().find(|m| m.name == name)
+    }
+
+    /// Every item, by section: plugins, servers, agents, then the
+    /// instructions and the env when there are any.
+    pub fn items(&self) -> Vec<Item> {
+        let mut items = Vec::new();
+        for p in &self.plugins {
+            items.push(Item {
+                id: p.id(),
+                section: Section::Plugins,
+                title: p.spec(),
+            });
+        }
+        for s in &self.servers {
+            let title = match &s.transport {
+                Transport::Http { url, .. } | Transport::Sse { url, .. } => {
+                    format!("{} ({} {url})", s.name, s.kind())
+                }
+                Transport::Stdio { command, .. } => format!("{} (stdio {command})", s.name),
+            };
+            items.push(Item {
+                id: s.id(),
+                section: Section::Servers,
+                title,
+            });
+        }
+        for a in &self.agents {
+            items.push(Item {
+                id: a.id(),
+                section: Section::Agents,
+                title: format!("{}: {}", a.name, a.description),
+            });
+        }
+        if !self.instructions.trim().is_empty() {
+            items.push(Item {
+                id: "instructions".to_owned(),
+                section: Section::Instructions,
+                title: "instructions".to_owned(),
+            });
+        }
+        if !self.env.is_empty() {
+            items.push(Item {
+                id: "env".to_owned(),
+                section: Section::Env,
+                title: format!("env ({} keys)", self.env.len()),
+            });
+        }
+        items
+    }
+
+    /// Every item's id, in [`Manifest::items`] order.
+    pub fn ids(&self) -> Vec<String> {
+        self.items().into_iter().map(|i| i.id).collect()
+    }
+}
+
+/// `{"mcpServers": {...}}` for `servers`, as `--mcp-config` takes it, or
+/// `None` when there are none. Placeholders stay as written.
+pub fn mcp_json<'a>(servers: impl IntoIterator<Item = &'a Server>) -> Option<String> {
+    let map: Map<String, Value> = servers
+        .into_iter()
+        .map(|s| (s.name.clone(), s.config()))
+        .collect();
+    (!map.is_empty()).then(|| json!({ "mcpServers": map }).to_string())
+}
+
+/// `{"<name>": {"description", "prompt", "model"?, "tools"?}}` for
+/// `agents`, as `--agents` takes it, or `None` when there are none.
+pub fn agents_json<'a>(agents: impl IntoIterator<Item = &'a Agent>) -> Option<String> {
+    let map: Map<String, Value> = agents
+        .into_iter()
+        .map(|a| {
+            let mut v = json!({ "description": a.description, "prompt": a.prompt });
+            if let Some(model) = &a.model {
+                v["model"] = json!(model);
+            }
+            if let Some(tools) = &a.tools {
+                v["tools"] = json!(tools);
+            }
+            (a.name.clone(), v)
+        })
+        .collect();
+    (!map.is_empty()).then(|| Value::Object(map).to_string())
+}
+
+/// Whether `id` is an item id of the grammar (≤ [`MAX_ID`] bytes):
+/// `plugin:<name>@<name>`, `mcp:<name>`, `agent:<name>`, `instructions`
+/// or `env`.
+pub fn id_ok(id: &str) -> bool {
+    if id.len() > MAX_ID {
+        return false;
+    }
+    if id == "instructions" || id == "env" {
+        return true;
+    }
+    if let Some(rest) = id.strip_prefix("plugin:") {
+        return rest
+            .split_once('@')
+            .is_some_and(|(p, m)| name_ok(p) && name_ok(m));
+    }
+    let name = id
+        .strip_prefix("mcp:")
+        .or_else(|| id.strip_prefix("agent:"));
+    name.is_some_and(name_ok)
+}
+
+// ── Parsing ───────────────────────────────────────────────────────────────────
+
+#[derive(Deserialize)]
+#[serde(deny_unknown_fields)]
+struct Raw {
+    // Checked on the table before this is read.
+    #[allow(dead_code)]
+    format: i64,
+    name: String,
+    version: String,
+    #[serde(default)]
+    description: String,
+    #[serde(default)]
+    instructions: String,
+    #[serde(default)]
+    marketplace: Vec<RawMarketplace>,
+    #[serde(default)]
+    plugin: Vec<RawPlugin>,
+    #[serde(default)]
+    mcp: Vec<RawServer>,
+    #[serde(default)]
+    agent: Vec<RawAgent>,
+    #[serde(default)]
+    env: BTreeMap<String, String>,
+}
+
+#[derive(Deserialize)]
+#[serde(deny_unknown_fields)]
+struct RawMarketplace {
+    name: String,
+    source: String,
+}
+
+#[derive(Deserialize)]
+#[serde(deny_unknown_fields)]
+struct RawPlugin {
+    name: String,
+    marketplace: String,
+}
+
+#[derive(Deserialize)]
+#[serde(deny_unknown_fields)]
+struct RawServer {
+    name: String,
+    #[serde(rename = "type")]
+    kind: String,
+    url: Option<String>,
+    headers: Option<BTreeMap<String, String>>,
+    #[serde(default)]
+    secrets: Vec<String>,
+    auth: Option<String>,
+    command: Option<String>,
+    args: Option<Vec<String>>,
+    env: Option<BTreeMap<String, String>>,
+}
+
+#[derive(Deserialize)]
+#[serde(deny_unknown_fields)]
+struct RawAgent {
+    name: String,
+    description: String,
+    model: Option<String>,
+    tools: Option<Vec<String>>,
+    prompt: String,
+}
+
+/// Why `src` did not deserialize: an unknown or missing key by name, else
+/// a generic reason; with the line, never the text.
+fn toml_reason(src: &str, e: &toml::de::Error) -> String {
+    let message = e.message();
+    let key = |prefix: &str| {
+        let rest = message.strip_prefix(prefix)?;
+        let key = rest.split('`').next()?;
+        let shown = (1..=64).contains(&key.len())
+            && key
+                .bytes()
+                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
+        Some(if shown { key } else { "?" })
+    };
+    let what = if let Some(k) = key("unknown field `") {
+        format!("unknown key {k}")
+    } else if let Some(k) = key("missing field `") {
+        format!("missing key {k}")
+    } else {
+        "not valid TOML, or a value of the wrong type".to_owned()
+    };
+    match e.span() {
+        Some(span) => {
+            let line = src[..span.start.min(src.len())].matches('\n').count() + 1;
+            format!("{what} (line {line})")
+        }
+        None => what,
+    }
+}
+
+/// UTF-8 without C0 controls but `\n` and `\t`.
+fn is_text(s: &str) -> bool {
+    !s.bytes().any(|b| b < 0x20 && b != b'\n' && b != b'\t')
+}
+
+/// Printable ASCII only.
+fn is_printable(s: &str) -> bool {
+    s.bytes().all(|b| (0x20..0x7f).contains(&b))
+}
+
+/// `what` holds text of at most `max` bytes.
+fn check_text(what: &str, s: &str, max: usize) -> Result<(), String> {
+    if s.len() > max {
+        return Err(format!("{what} over {max} bytes"));
+    }
+    if !is_text(s) {
+        return Err(format!("{what}: control character"));
+    }
+    Ok(())
+}
+
+/// `[<extra> alphanumeric]{1,max}`.
+fn word_ok(s: &str, max: usize, extra: &[u8]) -> bool {
+    (1..=max).contains(&s.len())
+        && s.bytes()
+            .all(|b| b.is_ascii_alphanumeric() || extra.contains(&b))
+}
+
+/// An RFC 7230 token of at most 64 bytes: a header name.
+fn header_name_ok(s: &str) -> bool {
+    word_ok(s, 64, b"!#$%&'*+-.^_`|~")
+}
+
+/// `github:<owner>/<repo>` or an https URL of at most 512 printable bytes.
+fn source_ok(s: &str) -> bool {
+    if let Some(rest) = s.strip_prefix("github:") {
+        // Neither part may read as a flag or a path to the CLI.
+        let part = |p: &str| word_ok(p, 64, b"_.-") && !p.starts_with(['-', '.']);
+        return rest
+            .split_once('/')
+            .is_some_and(|(o, r)| part(o) && part(r));
+    }
+    s.len() > "https://".len() && s.len() <= MAX_URL && s.starts_with("https://") && is_printable(s)
+}
+
+/// The names of the `${NAME}` placeholders in `s`, or `None` when a `${`
+/// is not one.
+fn placeholders(s: &str) -> Option<Vec<&str>> {
+    let mut names = Vec::new();
+    let mut rest = s;
+    while let Some(at) = rest.find("${") {
+        let after = &rest[at + 2..];
+        let end = after.find('}')?;
+        let name = &after[..end];
+        key_ok(name).ok()?;
+        names.push(name);
+        rest = &after[end + 1..];
+    }
+    Some(names)
+}
+
+/// Every `${NAME}` of `values` (field `what` of server `id`) is declared:
+/// one of `secrets` or a key of `env`.
+fn check_placeholders<'a>(
+    id: &str,
+    what: &str,
+    values: impl IntoIterator<Item = &'a String>,
+    secrets: &[String],
+    env: &BTreeMap<String, String>,
+) -> Result<(), String> {
+    for value in values {
+        let names = placeholders(value).ok_or_else(|| format!("{id}: {what}: bad placeholder"))?;
+        for name in names {
+            if !secrets.iter().any(|s| s == name) && !env.contains_key(name) {
+                return Err(format!(
+                    "{id}: {what}: ${{{name}}} is not in secrets or [env]"
+                ));
+            }
+        }
+    }
+    Ok(())
+}
+
+/// The pairs of `map`, each key by `key_ok` and each value text of at most
+/// `max` bytes; at most `count` of them.
+fn check_env(
+    what: &str,
+    map: &BTreeMap<String, String>,
+    count: usize,
+    max: usize,
+) -> Result<Vec<(String, String)>, String> {
+    if map.len() > count {
+        return Err(format!("{what}: more than {count} keys"));
+    }
+    for (key, value) in map {
+        key_ok(key).map_err(|why| format!("{what}: {why}"))?;
+        check_text(&format!("{what} {key}"), value, max)?;
+    }
+    Ok(map.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
+}
+
+impl Raw {
+    fn check(self) -> Result<Manifest, String> {
+        if !name_ok(&self.name) {
+            return Err("bad name".to_owned());
+        }
+        if !(1..=64).contains(&self.version.len()) || !is_printable(&self.version) {
+            return Err("bad version".to_owned());
+        }
+        check_text("description", &self.description, MAX_DESCRIPTION)?;
+        check_text("instructions", &self.instructions, MAX_INSTRUCTIONS)?;
+        let env = check_env("env", &self.env, MAX_ENV, MAX_VALUE)?;
+
+        if self.marketplace.len() > MAX_MARKETPLACES {
+            return Err(format!("more than {MAX_MARKETPLACES} marketplaces"));
+        }
+        let mut marketplaces: Vec<Marketplace> = Vec::new();
+        for (i, m) in self.marketplace.into_iter().enumerate() {
+            if !name_ok(&m.name) {
+                return Err(format!("marketplace #{}: bad name", i + 1));
+            }
+            if marketplaces.iter().any(|o| o.name == m.name) {
+                return Err(format!("marketplace {}: given twice", m.name));
+            }
+            if !source_ok(&m.source) {
+                return Err(format!("marketplace {}: bad source", m.name));
+            }
+            marketplaces.push(Marketplace {
+                name: m.name,
+                source: m.source,
+            });
+        }
+
+        if self.plugin.len() > MAX_PLUGINS {
+            return Err(format!("more than {MAX_PLUGINS} plugins"));
+        }
+        let mut plugins: Vec<PluginRef> = Vec::new();
+        for (i, p) in self.plugin.into_iter().enumerate() {
+            if !name_ok(&p.name) || !name_ok(&p.marketplace) {
+                return Err(format!("plugin #{}: bad name", i + 1));
+            }
+            let p = PluginRef {
+                name: p.name,
+                marketplace: p.marketplace,
+            };
+            if plugins.iter().any(|o| o.name == p.name) {
+                return Err(format!("plugin {}: given twice", p.name));
+            }
+            if !marketplaces.iter().any(|m| m.name == p.marketplace) {
+                return Err(format!(
+                    "{}: marketplace {} not declared",
+                    p.id(),
+                    p.marketplace
+                ));
+            }
+            plugins.push(p);
+        }
+
+        if self.agent.len() > MAX_AGENTS {
+            return Err(format!("more than {MAX_AGENTS} agents"));
+        }
+        let mut agents: Vec<Agent> = Vec::new();
+        for (i, a) in self.agent.into_iter().enumerate() {
+            if !name_ok(&a.name) {
+                return Err(format!("agent #{}: bad name", i + 1));
+            }
+            let id = format!("agent:{}", a.name);
+            if agents.iter().any(|o| o.name == a.name) {
+                return Err(format!("{id}: given twice"));
+            }
+            if a.description.trim().is_empty() {
+                return Err(format!("{id}: description is empty"));
+            }
+            check_text(
+                &format!("{id}: description"),
+                &a.description,
+                MAX_AGENT_DESCRIPTION,
+            )?;
+            if a.prompt.trim().is_empty() {
+                return Err(format!("{id}: prompt is empty"));
+            }
+            check_text(&format!("{id}: prompt"), &a.prompt, MAX_PROMPT)?;
+            if a.model.as_ref().is_some_and(|m| !word_ok(m, 64, b"._-")) {
+                return Err(format!("{id}: bad model"));
+            }
+            if let Some(tools) = &a.tools {
+                if tools.len() > MAX_TOOLS {
+                    return Err(format!("{id}: more than {MAX_TOOLS} tools"));
+                }
+                if tools.iter().any(|t| !word_ok(t, 64, b"_:*()-")) {
+                    return Err(format!("{id}: bad tool"));
+                }
+            }
+            agents.push(Agent {
+                name: a.name,
+                description: a.description,
+                model: a.model,
+                tools: a.tools,
+                prompt: a.prompt,
+            });
+        }
+
+        if self.mcp.len() > MAX_SERVERS {
+            return Err(format!("more than {MAX_SERVERS} MCP servers"));
+        }
+        let mut servers: Vec<Server> = Vec::new();
+        for (i, s) in self.mcp.into_iter().enumerate() {
+            if !name_ok(&s.name) {
+                return Err(format!("mcp #{}: bad name", i + 1));
+            }
+            let id = format!("mcp:{}", s.name);
+            if servers.iter().any(|o| o.name == s.name) {
+                return Err(format!("{id}: given twice"));
+            }
+            if agents.iter().any(|a| a.name == s.name) {
+                return Err(format!("{id}: an agent has this name"));
+            }
+            servers.push(s.check(&id, &self.env)?);
+        }
+
+        Ok(Manifest {
+            name: self.name,
+            version: self.version,
+            description: self.description,
+            instructions: self.instructions,
+            marketplaces,
+            plugins,
+            servers,
+            agents,
+            env,
+        })
+    }
+}
+
+impl RawServer {
+    /// The server `id`, with `env` the manifest's `[env]`.
+    fn check(self, id: &str, env: &BTreeMap<String, String>) -> Result<Server, String> {
+        if self.secrets.len() > MAX_SECRETS {
+            return Err(format!("{id}: more than {MAX_SECRETS} secrets"));
+        }
+        for secret in &self.secrets {
+            key_ok(secret).map_err(|why| format!("{id}: secrets: {why}"))?;
+        }
+        let transport = match self.kind.as_str() {
+            "http" | "sse" => {
+                if self.command.is_some() || self.args.is_some() || self.env.is_some() {
+                    return Err(format!(
+                        "{id}: command, args or env on an {} server",
+                        self.kind
+                    ));
+                }
+                let url = self.url.ok_or_else(|| format!("{id}: missing key url"))?;
+                if url.len() > MAX_URL || !url.starts_with("https://") || !is_printable(&url) {
+                    return Err(format!("{id}: url is not https"));
+                }
+                let headers = self.headers.unwrap_or_default();
+                if headers.len() > MAX_HEADERS {
+                    return Err(format!("{id}: more than {MAX_HEADERS} headers"));
+                }
+                for (name, value) in &headers {
+                    if !header_name_ok(name) {
+                        return Err(format!("{id}: bad header name"));
+                    }
+                    if value.len() > MAX_VALUE || !is_printable(value) {
+                        return Err(format!("{id}: header {name}: bad value"));
+                    }
+                }
+                let oauth = match self.auth.as_deref() {
+                    None => false,
+                    Some("oauth") => true,
+                    Some(_) => return Err(format!("{id}: auth is not oauth")),
+                };
+                if oauth && (!headers.is_empty() || !self.secrets.is_empty()) {
+                    return Err(format!("{id}: auth = oauth takes no headers or secrets"));
+                }
+                check_placeholders(id, "url", [&url], &self.secrets, env)?;
+                check_placeholders(id, "headers", headers.values(), &self.secrets, env)?;
+                let headers = headers.into_iter().collect();
+                if self.kind == "http" {
+                    Transport::Http {
+                        url,
+                        headers,
+                        oauth,
+                    }
+                } else {
+                    Transport::Sse {
+                        url,
+                        headers,
+                        oauth,
+                    }
+                }
+            }
+            "stdio" => {
+                if self.url.is_some() || self.headers.is_some() || self.auth.is_some() {
+                    return Err(format!("{id}: url, headers or auth on a stdio server"));
+                }
+                let command = self
+                    .command
+                    .ok_or_else(|| format!("{id}: missing key command"))?;
+                if command.is_empty() || command.len() > MAX_COMMAND || !is_printable(&command) {
+                    return Err(format!("{id}: bad command"));
+                }
+                if command.contains("${") {
+                    return Err(format!("{id}: command: bad placeholder"));
+                }
+                let args = self.args.unwrap_or_default();
+                if args.len() > MAX_ARGS {
+                    return Err(format!("{id}: more than {MAX_ARGS} args"));
+                }
+                for arg in &args {
+                    check_text(&format!("{id}: arg"), arg, MAX_ARG)?;
+                }
+                let server_env = self.env.unwrap_or_default();
+                let pairs = check_env(
+                    &format!("{id}: env"),
+                    &server_env,
+                    MAX_SERVER_ENV,
+                    MAX_VALUE,
+                )?;
+                check_placeholders(id, "args", &args, &self.secrets, env)?;
+                check_placeholders(id, "env", server_env.values(), &self.secrets, env)?;
+                Transport::Stdio {
+                    command,
+                    args,
+                    env: pairs,
+                }
+            }
+            _ => return Err(format!("{id}: type is not http, sse or stdio")),
+        };
+        Ok(Server {
+            name: self.name,
+            transport,
+            secrets: self.secrets,
+        })
+    }
+}
+
 #[cfg(test)]
 mod tests {
     use super::*;
diff --git a/src/proxy/plugins.rs b/src/proxy/plugins.rs
index faf1ee9..94d77c5 100644
--- a/src/proxy/plugins.rs
+++ b/src/proxy/plugins.rs
@@ -167,7 +167,7 @@ pub fn sha256_ok(hash: &str) -> bool {
 }
 
 /// `[A-Z_][A-Z0-9_]{0,63}`, and not one a plugin may not set.
-fn key_ok(key: &str) -> Result<(), String> {
+pub fn key_ok(key: &str) -> Result<(), String> {
     let mut bytes = key.bytes();
     let shape = key.len() <= 64
         && bytes
`````
Run: `umask 022; cargo test --locked --bin claudio plugins::manifest; cargo test --locked --bin claudio`
Expected: `test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 1044 filtered out`, then `test result: ok. 1063 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`. Dead-code warnings for the module's items until Tasks 4, 6 and 7 use them (`cargo build` lists them).

Step 3: commit
`/usr/bin/git add src/plugins/mod.rs src/plugins/manifest.rs src/plugins/testdata/vocdoni-internal.toml && /usr/bin/git commit -m "feat(plugins): manifest parser and renderers" -- src/plugins/mod.rs src/plugins/manifest.rs src/plugins/testdata/vocdoni-internal.toml src/main.rs src/proxy/plugins.rs`

Interfaces this task provides to later tasks: `crate::plugins::manifest::{Manifest, Marketplace, PluginRef, Server, Transport, Agent, Item, Section, mcp_json, agents_json, id_ok, MAX_BYTES, MAX_INSTRUCTIONS, MAX_AGENTS_JSON, MAX_MCP_JSON, MAX_ID}`; `Manifest::parse(&str) -> Result<Manifest, String>`, `Manifest::parse_named(&str, name)`, `Manifest::{items, ids, marketplace}`, `Marketplace::cli_source`, `PluginRef::{spec, id}`, `Server::{id, kind}`, `Agent::id`, `Section::heading`; `crate::proxy::plugins::key_ok` is `pub`.

### Task 2: proto: `Manifest` and `Applied`, plugin instructions in session extensions
Model: standard   Repo: /volumes/repos/claudio   Files: src/proto.rs, src/client.rs, src/daemon/journal.rs, src/daemon/mod.rs, src/daemon/tests.rs, src/tui/extensions/mod.rs
Goal: the wire ops and `SessionExtensions.instructions` (validated apart from the lenses, appended by `context()`, journaled with the rest) (AC3 part).
Patch: .delivery/2026-11-06-plugin-manifest/patches/task-2.patch

Step 1: write the failing test
`````diff
diff --git a/src/daemon/journal.rs b/src/daemon/journal.rs
index 784821a..9b9873c 100644
--- a/src/daemon/journal.rs
+++ b/src/daemon/journal.rs
@@ -487,6 +487,7 @@ mod tests {
                 kind: LensKind::Tag,
                 instruction: "Write <keys>.".into(),
             }],
+            ..Default::default()
         };
         let mut s = spec(id);
         s.extensions = Some(ext.clone());
diff --git a/src/daemon/tests.rs b/src/daemon/tests.rs
index f067b4e..3420dd2 100644
--- a/src/daemon/tests.rs
+++ b/src/daemon/tests.rs
@@ -1896,6 +1896,7 @@ fn lens(name: &str, instruction: &str) -> proto::LensSpec {
 fn extensions(lenses: &[(&str, &str)]) -> proto::SessionExtensions {
     proto::SessionExtensions {
         lenses: lenses.iter().map(|(n, i)| lens(n, i)).collect(),
+        ..Default::default()
     }
 }
 
diff --git a/src/proto.rs b/src/proto.rs
index 6e03f2a..bdeeeef 100644
--- a/src/proto.rs
+++ b/src/proto.rs
@@ -1251,6 +1322,58 @@ mod tests {
         );
     }
 
+    #[test]
+    fn manifest_and_applied_roundtrip_and_are_unknown_to_older_peers() {
+        #[derive(Debug, PartialEq, Deserialize)]
+        #[serde(tag = "op", rename_all = "snake_case")]
+        enum Old {
+            Ping,
+            #[serde(other)]
+            Unknown,
+        }
+        let request = Msg::Manifest {
+            name: "team".into(),
+            url: "https://e.org/team.toml".into(),
+            sha256: "a".repeat(64),
+            secrets: vec!["KB_TOKEN".into()],
+            disabled: vec!["mcp:jina".into()],
+        };
+        let reply = Msg::Applied(Applied {
+            name: "team".into(),
+            version: "1.0.0".into(),
+            mcp_config: Some(r#"{"mcpServers":{}}"#.into()),
+            agents: None,
+            instructions: "Rules.".into(),
+            env: vec![("REGION".into(), "eu".into())],
+            installed: vec!["plugin:a@m".into()],
+            present: vec!["plugin:b@m".into()],
+            skipped: vec![("plugin:c@m".into(), "installing".into())],
+            dropped: vec!["--agents".into()],
+        });
+        for msg in [request, reply] {
+            let json = serde_json::to_string(&Envelope::request(4, msg.clone())).unwrap();
+            assert_eq!(serde_json::from_str::<Old>(&json).unwrap(), Old::Unknown);
+            assert_eq!(serde_json::from_str::<Envelope>(&json).unwrap().msg, msg);
+        }
+        let json = serde_json::to_value(Msg::Applied(Applied::default())).unwrap();
+        assert_eq!(json["op"], "applied");
+        // Every list may be left out; so may a request's secrets and ids.
+        let minimal = r#"{"op":"applied","name":"team","version":"1"}"#;
+        assert_eq!(
+            serde_json::from_str::<Msg>(minimal).unwrap(),
+            Msg::Applied(Applied {
+                name: "team".into(),
+                version: "1".into(),
+                ..Applied::default()
+            })
+        );
+        let minimal = r#"{"op":"manifest","name":"t","url":"https://e.org","sha256":"x"}"#;
+        assert!(matches!(
+            serde_json::from_str::<Msg>(minimal).unwrap(),
+            Msg::Manifest { secrets, disabled, .. } if secrets.is_empty() && disabled.is_empty()
+        ));
+    }
+
     #[test]
     fn merge_env_keeps_plugin_keys_the_proxy_does_not_set() {
         let pair = |k: &str, v: &str| (k.to_owned(), v.to_owned());
@@ -1790,7 +1913,13 @@ mod tests {
 
     #[test]
     fn extension_caps() {
-        let ok = |lenses: Vec<LensSpec>| SessionExtensions { lenses }.validate();
+        let ok = |lenses: Vec<LensSpec>| {
+            SessionExtensions {
+                lenses,
+                ..Default::default()
+            }
+            .validate()
+        };
         assert_eq!(ok(vec![lens("latex", "x"), lens("a-1", "")]), Ok(()));
         assert!(ok((0..17).map(|i| lens(&format!("l{i}"), "")).collect()).is_err());
         assert!(ok(vec![lens("keys", ""), lens("keys", "")]).is_err());
@@ -1810,12 +1939,38 @@ mod tests {
         assert!(ok(over).is_err());
     }
 
+    #[test]
+    fn plugin_instructions_have_their_own_cap_and_are_optional() {
+        let with = |instructions: &str| SessionExtensions {
+            lenses: (0..4)
+                .map(|i| lens(&format!("l{i}"), &"i".repeat(2048)))
+                .collect(),
+            instructions: instructions.into(),
+        };
+        // Full lenses and full plugin instructions: both fit.
+        let at_cap = format!("{}\u{7}", "p".repeat(MAX_PLUGIN_INSTRUCTIONS));
+        assert_eq!(with(&at_cap).validate(), Ok(()));
+        assert_eq!(
+            with(&"p".repeat(MAX_PLUGIN_INSTRUCTIONS + 1)).validate(),
+            Err("plugin instructions over 8192 bytes".to_owned())
+        );
+        // An old client's (or journal's) extensions have none.
+        let old: SessionExtensions = serde_json::from_str(r#"{"lenses":[]}"#).unwrap();
+        assert_eq!(old.instructions, "");
+        let only = SessionExtensions {
+            instructions: "Rules.".into(),
+            ..Default::default()
+        };
+        assert_eq!(only.context(), "Rules.");
+    }
+
     #[test]
     fn extension_context_and_debug_never_show_instructions() {
         let ext = SessionExtensions {
             lenses: vec![lens("latex", " Math.\u{1b} "), lens("abc", ""), lens("keys", "Keys.")],
+            instructions: "\nTeam rules.\u{202e}\n".into(),
         };
-        assert_eq!(ext.context(), "Math.\n\nKeys.");
+        assert_eq!(ext.context(), "Math.\n\nKeys.\n\nTeam rules.");
         let spec = SpawnSpec {
             id: Uuid::nil(),
             cwd: "/".into(),
@@ -1829,6 +1984,7 @@ mod tests {
         };
         let shown = format!("{spec:?}");
         assert!(shown.contains("\"latex\"") && !shown.contains("Math"), "{shown}");
+        assert!(!shown.contains("Team rules"), "{shown}");
         let respawn = RespawnSpec {
             id: Uuid::nil(),
             fresh: false,
`````
Run: `umask 022; cargo test --locked --bin claudio proto::`
Expected: `` error[E0422]: cannot find struct, variant or union type `Applied` in this scope ``, `` error[E0560]: struct `proto::SessionExtensions` has no field named `instructions` ``, `` error[E0599]: no variant named `Manifest` found for enum `proto::Msg` ``, ending `` error: could not compile `claudio` (bin "claudio" test) due to 16 previous errors ``

Step 2: implement (`Msg::Bundle`/`Bundled` stay until Task 8; the client gives `Manifest` the download timeout)
`````diff
diff --git a/src/client.rs b/src/client.rs
index 7a3d752..acd007a 100644
--- a/src/client.rs
+++ b/src/client.rs
@@ -70,9 +70,9 @@ const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
 const SPAWN_TIMEOUT: Duration = Duration::from_secs(60);
 /// `UpdateClaude` runs up to 300 s on the daemon, plus the re-probe.
 const UPDATE_TIMEOUT: Duration = Duration::from_secs(330);
-/// A `Bundle` download may take the daemon's 120 s, after waiting for one
-/// already running.
-const BUNDLE_TIMEOUT: Duration = Duration::from_secs(250);
+/// A `Bundle` or `Manifest` download may take the daemon's 120 s, after
+/// waiting for one already running.
+const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(250);
 
 /// Something the daemon sent without being asked.
 #[derive(Debug, PartialEq)]
@@ -402,7 +402,7 @@ impl Client {
         let timeout = match msg {
             Msg::Spawn(_) | Msg::SpawnShell(_) => SPAWN_TIMEOUT,
             Msg::UpdateClaude { .. } => UPDATE_TIMEOUT,
-            Msg::Bundle { .. } => BUNDLE_TIMEOUT,
+            Msg::Bundle { .. } | Msg::Manifest { .. } => DOWNLOAD_TIMEOUT,
             _ => REQUEST_TIMEOUT,
         };
         let queued = self.enqueue(msg);
diff --git a/src/daemon/mod.rs b/src/daemon/mod.rs
index d9ecbe4..aa65931 100644
--- a/src/daemon/mod.rs
+++ b/src/daemon/mod.rs
@@ -1040,6 +1040,7 @@ fn effective_extensions(
             kind: LensKind::Tag,
             instruction: crate::claude::hooks::LATEX_CONTEXT.to_owned(),
         }],
+        ..Default::default()
     });
     ext.lenses
         .retain(|l| l.kind != LensKind::Unknown && (latex || l.name != "latex"));
diff --git a/src/proto.rs b/src/proto.rs
index 6ed7def..bdeeeef 100644
--- a/src/proto.rs
+++ b/src/proto.rs
@@ -219,6 +219,22 @@ pub enum Msg {
         #[serde(default)]
         installed_id: String,
     },
+    /// Apply proxy plugin `name`'s manifest on this host: the TOML at `url`
+    /// (https) whose SHA-256 is `sha256`, cached once downloaded. `secrets`
+    /// are the names (never the values) of the plugin's proxy env, which
+    /// the sessions get; `disabled` the item ids the member turned off.
+    /// Answered by `Applied` as soon as the manifest is parsed (plugin
+    /// installs run after the reply), or `Error "manifest <name>: …"`. An
+    /// older daemon answers `Error "unsupported op…"`.
+    Manifest {
+        name: String,
+        url: String,
+        sha256: String,
+        #[serde(default)]
+        secrets: Vec<String>,
+        #[serde(default)]
+        disabled: Vec<String>,
+    },
 
     // ── daemon → client replies ──────────────────────────────────────────
     Sessions {
@@ -293,6 +309,8 @@ pub enum Msg {
         #[serde(default)]
         installed: bool,
     },
+    /// Reply to `Manifest`.
+    Applied(Applied),
     Ok,
     Error {
         message: String,
@@ -427,14 +445,56 @@ pub struct RespawnSpec {
     pub extensions: Option<SessionExtensions>,
 }
 
-/// The lenses of a claude session, in the client's registry order. The
-/// daemon checks the caps ([`SessionExtensions::validate`]), writes the
-/// instructions to the session's context file and extracts the tag lenses'
-/// tags from the transcript.
+/// What a host made of a proxy plugin's manifest ([`Msg::Manifest`]): what
+/// every new session there gets from it, and each item's outcome by item id
+/// (`plugin:<name>@<marketplace>`, `mcp:<name>`, `agent:<name>`…). A
+/// disabled id is in no list.
+#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
+pub struct Applied {
+    pub name: String,
+    pub version: String,
+    /// `{"mcpServers": {...}}` for `--mcp-config` (≤ 64 KiB): `${NAME}`
+    /// placeholders, never a secret.
+    #[serde(default)]
+    pub mcp_config: Option<String>,
+    /// The `--agents` JSON (≤ 32 KiB).
+    #[serde(default)]
+    pub agents: Option<String>,
+    /// For the session context: sanitized, ≤ 8 KiB, empty when none.
+    #[serde(default)]
+    pub instructions: String,
+    /// The manifest's `[env]`.
+    #[serde(default)]
+    pub env: Vec<(String, String)>,
+    /// Plugin ids this daemon installed (under this manifest's hash).
+    #[serde(default)]
+    pub installed: Vec<String>,
+    /// Plugin ids Claude Code had installed already.
+    #[serde(default)]
+    pub present: Vec<String>,
+    /// `(id, reason)`: `installing`, `yours`, `missing secret X`, a
+    /// failure…
+    #[serde(default)]
+    pub skipped: Vec<(String, String)>,
+    /// `--mcp-config` and/or `--agents` when this host's claude lacks them.
+    #[serde(default)]
+    pub dropped: Vec<String>,
+}
+
+/// The lenses of a claude session, in the client's registry order, and
+/// the proxy plugins' instructions. The daemon checks the caps
+/// ([`SessionExtensions::validate`]), writes the instructions to the
+/// session's context file and extracts the tag lenses' tags from the
+/// transcript.
 #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
 pub struct SessionExtensions {
     #[serde(default)]
     pub lenses: Vec<LensSpec>,
+    /// The enabled proxy plugins' instructions, joined by a blank line:
+    /// at most [`MAX_PLUGIN_INSTRUCTIONS`] bytes after `sanitize`, counted
+    /// apart from the lenses. An old daemon ignores it.
+    #[serde(default)]
+    pub instructions: String,
 }
 
 /// One lens: its name (the tag, or the fence's info word), its kind, and
@@ -465,11 +525,15 @@ pub const MAX_LENSES: usize = 16;
 pub const MAX_INSTRUCTION: usize = 2048;
 /// All instructions together, in bytes after `sanitize`.
 pub const MAX_INSTRUCTIONS: usize = 8192;
+/// The plugins' instructions, in bytes after `sanitize`. With the lenses'
+/// they always fit the context hook's 16 KiB read.
+pub const MAX_PLUGIN_INSTRUCTIONS: usize = 8192;
 
 impl SessionExtensions {
     /// Why this value breaks the caps: more than [`MAX_LENSES`] lenses, a
     /// name that is not `^[a-z][a-z0-9-]{0,23}$` or is repeated, or
-    /// instructions over [`MAX_INSTRUCTION`] / [`MAX_INSTRUCTIONS`].
+    /// instructions over [`MAX_INSTRUCTION`] / [`MAX_INSTRUCTIONS`], or
+    /// plugin instructions over [`MAX_PLUGIN_INSTRUCTIONS`].
     pub fn validate(&self) -> Result<(), String> {
         if self.lenses.len() > MAX_LENSES {
             return Err(format!("more than {MAX_LENSES} lenses"));
@@ -491,6 +555,11 @@ impl SessionExtensions {
         if total > MAX_INSTRUCTIONS {
             return Err(format!("instructions over {MAX_INSTRUCTIONS} bytes"));
         }
+        if crate::claude::latex::sanitize(&self.instructions).len() > MAX_PLUGIN_INSTRUCTIONS {
+            return Err(format!(
+                "plugin instructions over {MAX_PLUGIN_INSTRUCTIONS} bytes"
+            ));
+        }
         Ok(())
     }
 
@@ -504,12 +573,14 @@ impl SessionExtensions {
     }
 
     /// What claude is told: the non-empty instructions, sanitized, joined
-    /// by a blank line.
+    /// by a blank line; the plugins' after the lenses'.
     pub fn context(&self) -> String {
         let parts: Vec<String> = self
             .lenses
             .iter()
-            .map(|l| crate::claude::latex::sanitize(&l.instruction).trim().to_owned())
+            .map(|l| l.instruction.as_str())
+            .chain([self.instructions.as_str()])
+            .map(|i| crate::claude::latex::sanitize(i).trim().to_owned())
             .filter(|i| !i.is_empty())
             .collect();
         parts.join("\n\n")
diff --git a/src/tui/extensions/mod.rs b/src/tui/extensions/mod.rs
index 2b1b571..18fa438 100644
--- a/src/tui/extensions/mod.rs
+++ b/src/tui/extensions/mod.rs
@@ -308,7 +308,11 @@ impl Registry {
                 instruction: lens.instruction.clone(),
             });
         }
-        (SessionExtensions { lenses }, dropped)
+        let ext = SessionExtensions {
+            lenses,
+            ..Default::default()
+        };
+        (ext, dropped)
     }
 
     /// The leader's actions: each enabled extension's, in order.
`````
Run: `umask 022; cargo test --locked --bin claudio`
Expected: `test result: ok. 1065 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`

Step 3: commit
`/usr/bin/git commit -m "feat(proto): Manifest and Applied ops, plugin instructions in session extensions" -- src/client.rs src/daemon/journal.rs src/daemon/mod.rs src/daemon/tests.rs src/proto.rs src/tui/extensions/mod.rs`

Interfaces this task provides to later tasks: `Msg::Manifest { name, url, sha256, secrets, disabled }`, `Msg::Applied(proto::Applied)`, `proto::Applied { name, version, mcp_config, agents, instructions, env, installed, present, skipped, dropped }`, `SessionExtensions.instructions`, `proto::MAX_PLUGIN_INSTRUCTIONS`.

### Task 3: daemon: drop `--agents` and `--mcp-config` for a claude without them
Model: standard   Repo: /volumes/repos/claudio   Files: src/daemon/host.rs, src/daemon/session.rs, src/daemon/mod.rs, src/daemon/tests.rs
Goal: one cached `claude --help` probe for the plugin flags; `command()` drops exactly one value with an unknown flag and keeps the args' order before `-n` (AC3 part: argv order and pair drops).
Patch: .delivery/2026-11-06-plugin-manifest/patches/task-3.patch

Step 1: write the failing test
`````diff
diff --git a/src/daemon/host.rs b/src/daemon/host.rs
index 9842752..9df0cc0 100644
--- a/src/daemon/host.rs
+++ b/src/daemon/host.rs
@@ -275,19 +319,38 @@ mod tests {
             let script = format!("#!/bin/sh\n[ \"$1\" = --help ] && printf '%s\\n' '{text}'\n");
             std::fs::write(&bin, script).unwrap();
             std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
-            claude_flags(&bin, None).unwrap()
+            // A script just written may be busy for a moment (ETXTBSY while
+            // another test thread forks): probe again.
+            (0..20)
+                .find_map(|_| {
+                    claude_flags(&bin, None).or_else(|| {
+                        std::thread::sleep(std::time::Duration::from_millis(50));
+                        None
+                    })
+                })
+                .unwrap()
         };
         let both = help("  --plugin-dir <paths...>  --allow-dangerously-skip-permissions");
         assert_eq!(
             both,
             ClaudeFlags {
                 skip_permissions: true,
-                plugin_dir: true
+                plugin_dir: true,
+                ..ClaudeFlags::default()
             }
         );
         let plugins = help("  --plugin-dir <paths...>");
         assert!(plugins.plugin_dir && !plugins.skip_permissions);
         assert_eq!(help(""), ClaudeFlags::default());
+        assert_eq!(help("").dropped(), ["--mcp-config", "--agents"]);
+        // Listed as options, not merely named in another one's description.
+        let current = help(
+            "  --agents <json-or-file>  Agents\n  --mcp-config <configs...>  MCP\n               --bare  Skips --plugin-dir, --strict-mcp-config",
+        );
+        assert!(current.agents && current.mcp_config && !current.plugin_dir);
+        assert!(current.dropped().is_empty());
+        let named = help("  --bare  Skips --mcp-config, --agents\n  --strict-mcp-config  Only");
+        assert!(!named.agents && !named.mcp_config, "{named:?}");
         assert_eq!(claude_flags(Path::new("/nonexistent/claude"), None), None);
         let _ = std::fs::remove_dir_all(&dir);
     }
diff --git a/src/daemon/session.rs b/src/daemon/session.rs
index 34ef3ce..00b3c67 100644
--- a/src/daemon/session.rs
+++ b/src/daemon/session.rs
@@ -1290,7 +1299,8 @@ mod tests {
             ephemeral: false,
             extensions: None,
         };
-        let cmd = command(&cfg, &spec, Path::new("/"), "tok123", false, true, None);
+        let flags = host::ClaudeFlags::default();
+        let cmd = command(&cfg, &spec, Path::new("/"), "tok123", false, flags, None);
         let argv = argv_of(&cmd);
         assert_eq!(argv[0], "/bin/claude");
         assert_eq!(argv[1], "--settings");
@@ -1303,7 +1313,7 @@ mod tests {
 
         // With skip-permissions on, the flag follows --settings, ahead of
         // the spec args (which are what the journal stores).
-        let cmd = command(&cfg, &spec, Path::new("/"), "tok123", true, true, None);
+        let cmd = command(&cfg, &spec, Path::new("/"), "tok123", true, flags, None);
         assert_eq!(
             &argv_of(&cmd)[3..],
             [ALLOW, "--resume", "abc", "-n", "work"]
@@ -1318,7 +1328,7 @@ mod tests {
                 Path::new("/"),
                 "t",
                 false,
-                true,
+                flags,
                 context,
             ));
             let settings: Value = serde_json::from_str(&argv[2]).unwrap();
@@ -1483,7 +1493,16 @@ mod tests {
                 ephemeral: false,
                 extensions: None,
             };
-            let argv = argv_of(&command(&cfg, &spec, Path::new("/"), "t", true, true, None));
+            let flags = host::ClaudeFlags::default();
+            let argv = argv_of(&command(
+                &cfg,
+                &spec,
+                Path::new("/"),
+                "t",
+                true,
+                flags,
+                None,
+            ));
             assert_eq!(&argv[3..], [given]);
         }
     }
@@ -1520,17 +1539,92 @@ mod tests {
             extensions: None,
         };
         let argv = |plugin_dir| {
+            let flags = host::ClaudeFlags {
+                plugin_dir,
+                ..Default::default()
+            };
             argv_of(&command(
                 &cfg,
                 &spec,
                 Path::new("/"),
                 "t",
                 false,
-                plugin_dir,
+                flags,
                 None,
             ))
         };
         assert_eq!(&argv(false)[3..], ["--resume", "abc"]);
         assert_eq!(&argv(true)[3..], &spec.args[..]);
     }
+
+    /// The plugin flags stay where the client put them, after every other
+    /// argument and right before `-n <name>`, so the variadic
+    /// `--mcp-config` never takes a positional; a claude without one of
+    /// them loses exactly that flag and its value.
+    #[test]
+    fn plugin_flags_keep_their_order_and_are_dropped_when_unknown() {
+        let cfg = super::super::Config {
+            socket: "/run/d.sock".into(),
+            lock: "/run/d.lock".into(),
+            journal: "/j.json".into(),
+            claude: "/bin/claude".into(),
+            claudio: "/bin/claudio".into(),
+            skip_permissions: false,
+            latex: false,
+            build: None,
+            path: None,
+        };
+        let agents = r#"{"reviewer":{"description":"d","prompt":"p"}}"#;
+        let mcp = r#"{"mcpServers":{"kb":{"type":"http","url":"https://e.org"}}}"#;
+        let spec = SpawnSpec {
+            id: uuid::Uuid::nil(),
+            cwd: "/".into(),
+            name: Some("s1".into()),
+            args: vec![
+                "--resume".into(),
+                "abc".into(),
+                "--agents".into(),
+                agents.into(),
+                "--mcp-config".into(),
+                mcp.into(),
+                "--agents".into(),
+                agents.into(),
+                format!("--mcp-config={mcp}"),
+            ],
+            env: vec![],
+            rows: 24,
+            cols: 80,
+            ephemeral: false,
+            extensions: None,
+        };
+        let argv = |agents, mcp_config| {
+            let flags = host::ClaudeFlags {
+                agents,
+                mcp_config,
+                ..Default::default()
+            };
+            argv_of(&command(
+                &cfg,
+                &spec,
+                Path::new("/"),
+                "t",
+                false,
+                flags,
+                None,
+            ))
+        };
+        let all = argv(true, true);
+        assert_eq!(&all[3..12], &spec.args[..]);
+        assert_eq!(&all[12..], ["-n", "s1"]);
+        let mcp_eq = format!("--mcp-config={mcp}");
+        assert_eq!(
+            &argv(false, true)[3..],
+            ["--resume", "abc", "--mcp-config", mcp, &mcp_eq, "-n", "s1"]
+        );
+        assert_eq!(
+            &argv(true, false)[3..],
+            ["--resume", "abc", "--agents", agents, "--agents", agents, "-n", "s1"]
+        );
+        assert_eq!(&argv(false, false)[3..], ["--resume", "abc", "-n", "s1"]);
+    }
 }
diff --git a/src/daemon/tests.rs b/src/daemon/tests.rs
index 3420dd2..8342ec7 100644
--- a/src/daemon/tests.rs
+++ b/src/daemon/tests.rs
@@ -2276,7 +2276,7 @@ async fn plugin_dirs_are_dropped_for_an_old_claude_but_journaled() {
     spec.args = vec!["--plugin-dir".into(), "/c/plugins/kb.zip".into()];
     c.spawn(spec).await;
     c.attach_until(id, BANNER).await;
-    assert!(!d.daemon.plugin_dir());
+    assert!(!d.daemon.claude_flags().plugin_dir);
     let entry = &d.journal()["sessions"][0];
     assert_eq!(
         entry["args"],
`````
Run: `umask 022; cargo test --locked --bin claudio daemon::`
Expected: `` error[E0609]: no field `agents` on type `daemon::host::ClaudeFlags` ``, `` error[E0599]: no method named `dropped` found for struct `daemon::host::ClaudeFlags` in the current scope ``, ending `` error: could not compile `claudio` (bin "claudio" test) due to 14 previous errors ``

Step 2: implement
`````diff
diff --git a/src/daemon/host.rs b/src/daemon/host.rs
index 80f5833..fa9342d 100644
--- a/src/daemon/host.rs
+++ b/src/daemon/host.rs
@@ -102,6 +102,12 @@ pub const ALLOW_SKIP_PERMISSIONS: &str = "--allow-dangerously-skip-permissions";
 /// Loads a plugin for one session, uninstalled (a directory or a zip).
 pub const PLUGIN_DIR: &str = "--plugin-dir";
 
+/// Defines subagents for one session (a JSON object).
+pub const AGENTS: &str = "--agents";
+
+/// Loads MCP servers for one session (JSON files or strings; variadic).
+pub const MCP_CONFIG: &str = "--mcp-config";
+
 /// The optional flags this claude knows, from one `--help` run.
 #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
 pub struct ClaudeFlags {
@@ -109,6 +115,42 @@ pub struct ClaudeFlags {
     pub skip_permissions: bool,
     /// [`PLUGIN_DIR`].
     pub plugin_dir: bool,
+    /// [`AGENTS`].
+    pub agents: bool,
+    /// [`MCP_CONFIG`].
+    pub mcp_config: bool,
+}
+
+impl ClaudeFlags {
+    /// The flags that take one value and that a session's args may carry
+    /// although this claude lacks them, each with whether it knows it.
+    pub fn optional(&self) -> [(&'static str, bool); 3] {
+        [
+            (PLUGIN_DIR, self.plugin_dir),
+            (AGENTS, self.agents),
+            (MCP_CONFIG, self.mcp_config),
+        ]
+    }
+
+    /// The plugin flags this claude lacks (`--mcp-config`, `--agents`):
+    /// what a plugin's sessions here go without.
+    pub fn dropped(&self) -> Vec<String> {
+        [(MCP_CONFIG, self.mcp_config), (AGENTS, self.agents)]
+            .into_iter()
+            .filter(|(_, known)| !known)
+            .map(|(flag, _)| flag.to_owned())
+            .collect()
+    }
+}
+
+/// Whether `help` lists `flag` as an option: a line that starts with it
+/// (another option's description may name it too).
+fn lists(help: &str, flag: &str) -> bool {
+    help.lines().any(|line| {
+        line.trim_start()
+            .strip_prefix(flag)
+            .is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', '=', ',']))
+    })
 }
 
 /// [`ClaudeFlags`] of `<claude> --help`, or `None` when it did not run
@@ -118,7 +160,9 @@ pub fn claude_flags(claude: &Path, path: Option<&OsStr>) -> Option<ClaudeFlags>
     let help = run_capture(claude, "--help", path)?;
     Some(ClaudeFlags {
         skip_permissions: help.contains(ALLOW_SKIP_PERMISSIONS),
-        plugin_dir: help.contains(PLUGIN_DIR),
+        plugin_dir: lists(&help, PLUGIN_DIR),
+        agents: lists(&help, AGENTS),
+        mcp_config: lists(&help, MCP_CONFIG),
     })
 }
 
diff --git a/src/daemon/mod.rs b/src/daemon/mod.rs
index aa65931..e6b8024 100644
--- a/src/daemon/mod.rs
+++ b/src/daemon/mod.rs
@@ -342,8 +342,9 @@ pub(crate) struct Daemon {
     /// Held while `claude update` runs: one at a time.
     updating: tokio::sync::Mutex<()>,
     /// Which optional flags this claude knows (`--allow-dangerously-skip-
-    /// permissions`, `--plugin-dir`; it runs `claude --help`). Probed on
-    /// first use, and again after a claude update or install, like `host`.
+    /// permissions`, `--plugin-dir`, `--agents`, `--mcp-config`; it runs
+    /// `claude --help`). Probed on first use, and again after a claude
+    /// update or install, like `host`.
     claude_flags: Mutex<Option<host::ClaudeFlags>>,
     /// Notified when the daemon should shut down cleanly.
     pub(crate) shutdown: tokio::sync::Notify,
@@ -447,12 +448,6 @@ impl Daemon {
         self.config.skip_permissions && self.claude_flags().skip_permissions
     }
 
-    /// Whether this claude takes `--plugin-dir`. Blocking the first time,
-    /// like [`Daemon::skip_permissions`].
-    pub(crate) fn plugin_dir(&self) -> bool {
-        self.claude_flags().plugin_dir
-    }
-
     /// The optional flags this claude knows: probed once (`claude --help`),
     /// until the next claude update. Blocking the first time.
     pub(crate) fn claude_flags(&self) -> host::ClaudeFlags {
diff --git a/src/daemon/session.rs b/src/daemon/session.rs
index eb846c3..00b3c67 100644
--- a/src/daemon/session.rs
+++ b/src/daemon/session.rs
@@ -182,7 +182,7 @@ pub fn spawn(
                 &cwd,
                 &token,
                 daemon.skip_permissions(),
-                daemon.plugin_dir(),
+                daemon.claude_flags(),
                 context.as_deref(),
             );
             let program = PathBuf::from(cmd.get_argv()[0].clone());
@@ -318,16 +318,19 @@ pub(super) fn clear_contexts(socket: &Path) {
 /// `--resume`, resume-retry), rather than in `spec.args`, so the journal never
 /// stores it. It is left out when the args already choose a permissions flag.
 ///
-/// Without `plugin_dir` (this claude does not know `--plugin-dir`) every
-/// `--plugin-dir <path>` pair of the args is dropped, so an old claude still
-/// starts; the journal keeps them for a newer one.
+/// A `--plugin-dir`, `--agents` or `--mcp-config` pair (or `--flag=value`)
+/// of the args is dropped when this claude lacks the flag (`flags`), so an
+/// old claude still starts; exactly one value goes with the flag. The
+/// journal keeps them for a newer claude. The args keep their order, and
+/// `-n <name>` follows them: the client puts the plugin flags last, so the
+/// variadic `--mcp-config` is always followed by a flag or nothing.
 fn command(
     cfg: &super::Config,
     spec: &SpawnSpec,
     cwd: &Path,
     token: &str,
     skip_permissions: bool,
-    plugin_dir: bool,
+    flags: host::ClaudeFlags,
     context: Option<&Path>,
 ) -> CommandBuilder {
     let settings = hooks::settings(&cfg.claudio, &cfg.socket, token, context);
@@ -342,13 +345,19 @@ fn command(
     {
         cmd.arg(host::ALLOW_SKIP_PERMISSIONS);
     }
+    let unknown = |flag: &str| {
+        flags
+            .optional()
+            .iter()
+            .any(|(name, known)| !known && *name == flag)
+    };
     let mut args = spec.args.iter();
     while let Some(a) = args.next() {
-        if !plugin_dir && a == host::PLUGIN_DIR {
+        if unknown(a) {
             args.next();
             continue;
         }
-        if !plugin_dir && a.starts_with("--plugin-dir=") {
+        if a.split_once('=').is_some_and(|(flag, _)| unknown(flag)) {
             continue;
         }
         cmd.arg(a);
`````
Run: `umask 022; cargo test --locked --bin claudio`
Expected: `test result: ok. 1066 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` (the base's ETXTBSY flake family can hit `daemon::host::tests::claude_flags_read_long_help` once; rerun)

Step 3: commit
`/usr/bin/git commit -m "feat(daemon): drop --agents and --mcp-config for a claude without them" -- src/daemon/host.rs src/daemon/session.rs src/daemon/mod.rs src/daemon/tests.rs`

Interfaces this task provides to later tasks: `host::{AGENTS, MCP_CONFIG}`, `ClaudeFlags { skip_permissions, plugin_dir, agents, mcp_config }`, `ClaudeFlags::{optional, dropped}`, `session::command(.., flags: ClaudeFlags, ..)`; `Daemon::plugin_dir()` is gone (use `claude_flags()`).

### Task 4: daemon: apply plugin manifests and install their plugins
Model: strong   Repo: /volumes/repos/claudio   Files: src/daemon/manifest.rs (new), src/daemon/bundle.rs (deleted), src/daemon/mod.rs, src/daemon/server.rs, src/daemon/update.rs, src/daemon/tests.rs, src/proxy/plugins.rs
Goal: the `Manifest` op: request caps, cached download (256 KiB), Claude Code's files read with caps, the reply before any install, the per-(hash, id) memory with the 10 min window (clock injected), installs under `Daemon::updating` with `--json` parsing and no `-y`, `dropped` from the probe, stale zips removed at start (AC2).
Patch: .delivery/2026-11-06-plugin-manifest/patches/task-4.patch

The daemon tests use one shared fake `claude` (see its doc comment): it logs every `plugin …`
argv, adds the marketplace or installs into `<ctl>/../claude` like Claude Code, and is steered
by control files (a gate that holds an install, a failure, a `shownCommand`, a marketplace name).
`Bundle` requests are now answered `unsupported op` (the client still sends them until Task 6,
which turns that into `daemon too old`).

Step 1: write the failing test
`````diff
diff --git a/src/daemon/manifest.rs b/src/daemon/manifest.rs
new file mode 100644
index 0000000..36dc2be
--- /dev/null
+++ b/src/daemon/manifest.rs
@@ -0,0 +1,109 @@
+#[cfg(test)]
+mod tests {
+    use super::*;
+
+    #[test]
+    fn a_runs_outcome_comes_from_its_last_json_line() {
+        let run = |ok, out: &str, err: &str| {
+            outcome(ok, Some(i32::from(!ok)), out.as_bytes(), err.as_bytes())
+        };
+        assert_eq!(run(true, "Installing…\n{\"ok\":true}\n", ""), Outcome::Done);
+        assert_eq!(run(true, "", ""), Outcome::Done);
+        let shown = r#"{"shownCommand":{"command":"npx x","sha256":"ab"}}"#;
+        assert_eq!(run(false, shown, ""), Outcome::Manual);
+        assert_eq!(run(true, shown, ""), Outcome::Manual);
+        assert_eq!(
+            run(false, "{\"error\":\"Plugin not found\"}", "trace"),
+            Outcome::Failed("Plugin not found".into())
+        );
+        assert_eq!(
+            run(true, r#"{"error":{"message":"denied","code":3}}"#, ""),
+            Outcome::Failed("denied".into())
+        );
+        assert_eq!(
+            run(false, "not json", "\n  fatal: repository not found\nmore"),
+            Outcome::Failed("fatal: repository not found".into())
+        );
+        assert_eq!(run(false, "", ""), Outcome::Failed("exit status 1".into()));
+        assert_eq!(
+            outcome(false, None, b"", b""),
+            Outcome::Failed("killed".into())
+        );
+        let long = format!("{{\"error\":\"{}\\u001b[31m\"}}", "e".repeat(300));
+        let Outcome::Failed(why) = run(false, &long, "") else {
+            panic!("a failure")
+        };
+        assert_eq!(why, "e".repeat(REASON_CHARS));
+        let Outcome::Failed(why) = run(false, "", "two\nlines\twith\u{1b}[1m codes") else {
+            panic!("a failure")
+        };
+        assert_eq!(why, "two");
+    }
+
+    #[test]
+    fn stale_zips_go_and_a_manifests_sidecar_stays() {
+        let dir = std::env::temp_dir().join(format!("claudio-zips-{}", uuid::Uuid::new_v4()));
+        std::fs::create_dir_all(&dir).unwrap();
+        for f in [
+            "old.zip",
+            "old.sha256",
+            "kb.zip",
+            "kb.toml",
+            "kb.sha256",
+            "x.toml",
+        ] {
+            std::fs::write(dir.join(f), "x").unwrap();
+        }
+        remove_stale_zips(&dir);
+        let mut left: Vec<String> = std::fs::read_dir(&dir)
+            .unwrap()
+            .flatten()
+            .map(|e| e.file_name().to_string_lossy().into_owned())
+            .collect();
+        left.sort();
+        assert_eq!(left, ["kb.sha256", "kb.toml", "x.toml"]);
+        remove_stale_zips(&dir.join("missing"));
+        std::fs::remove_dir_all(&dir).unwrap();
+    }
+
+    #[test]
+    fn claude_codes_files_are_read_only_when_regular_ours_and_json() {
+        use std::os::unix::fs::symlink;
+        let dir = std::env::temp_dir().join(format!("claudio-files-{}", uuid::Uuid::new_v4()));
+        std::fs::create_dir_all(dir.join("plugins")).unwrap();
+        std::fs::create_dir_all(dir.join("agents")).unwrap();
+        let agents = ["reviewer".to_owned(), "helper".to_owned()];
+        // Nothing there: nothing installed, no servers of the member's own.
+        let files = Files::read(&dir, &agents);
+        assert!(files.installed.is_empty() && files.agents.is_empty());
+        assert_eq!(files.servers, Some(HashSet::new()));
+        std::fs::write(
+            dir.join("plugins/installed_plugins.json"),
+            r#"{"version":2,"plugins":{"a@m":[{"scope":"project"}]}}"#,
+        )
+        .unwrap();
+        std::fs::write(dir.join(".claude.json"), r#"{"mcpServers":{"kb":{}}}"#).unwrap();
+        std::fs::write(dir.join("agents/reviewer.md"), "mine").unwrap();
+        let files = Files::read(&dir, &agents);
+        assert_eq!(files.installed, HashSet::from(["a@m".to_owned()]));
+        assert_eq!(files.servers, Some(HashSet::from(["kb".to_owned()])));
+        assert_eq!(files.agents, HashSet::from(["reviewer".to_owned()]));
+        // A symlink, or a file that is not JSON, is unreadable.
+        std::fs::remove_file(dir.join(".claude.json")).unwrap();
+        std::fs::write(dir.join("elsewhere.json"), r#"{"mcpServers":{}}"#).unwrap();
+        symlink(dir.join("elsewhere.json"), dir.join(".claude.json")).unwrap();
+        assert_eq!(Files::read(&dir, &agents).servers, None);
+        std::fs::write(dir.join("plugins/installed_plugins.json"), "{").unwrap();
+        assert!(Files::read(&dir, &agents).installed.is_empty());
+        std::fs::write(
+            dir.join("plugins/known_marketplaces.json"),
+            r#"{"vocdoni":{"source":{"source":"github","repo":"vocdoni/skills"}}}"#,
+        )
+        .unwrap();
+        assert_eq!(
+            known_marketplaces(&dir),
+            HashSet::from(["vocdoni".to_owned()])
+        );
+        std::fs::remove_dir_all(&dir).unwrap();
+    }
+}
diff --git a/src/daemon/mod.rs b/src/daemon/mod.rs
index e6b8024..5970f9c 100644
--- a/src/daemon/mod.rs
+++ b/src/daemon/mod.rs
@@ -11,19 +11,19 @@
 //! - [`update`] — `claude update` (or the installer), on request.
 //! - [`git`] — the read-only git queries behind the TUI's git viewer.
 //! - [`login_path`] — the user's login-shell `PATH`, for claude and sessions.
-//! - [`bundle`] — proxy plugin bundles: the zips sessions load with
-//!   `--plugin-dir`.
+//! - [`manifest`] — proxy plugin manifests: what a plugin's sessions get
+//!   here, and the plugin installs it needs.
 //!
 //! This module holds the shared [`Daemon`] state: the registry of journaled
 //! and live sessions, and the all-clients event broadcast.
 
-mod bundle;
 pub mod ctl;
 mod git;
 mod git_status;
 mod host;
 mod journal;
 mod login_path;
+mod manifest;
 mod server;
 mod session;
 mod stats;
diff --git a/src/daemon/tests.rs b/src/daemon/tests.rs
index 8342ec7..3e2cb5a 100644
--- a/src/daemon/tests.rs
+++ b/src/daemon/tests.rs
@@ -2036,85 +2036,533 @@ async fn a_daemon_start_clears_old_context_files() {
     assert!(!context.join("old.txt").exists());
 }
 
-// ── Bundle ───────────────────────────────────────────────────────────────────
+// ── Manifest ─────────────────────────────────────────────────────────────────
+
+/// The manifest the `Manifest` tests apply: one marketplace with plugins
+/// `a` and `b`, servers `kb` (secret `KB_TOKEN`), `mine` and `jina`
+/// (secret `JINA_KEY`), agents `reviewer` and `helper`, instructions, env.
+const TEAM: &str = r#"
+format = 1
+name = "team"
+version = "1.0.0"
+instructions = "Team rules.\u202e"
+
+[[marketplace]]
+name = "market"
+source = "github:acme/market"
+
+[[plugin]]
+name = "a"
+marketplace = "market"
+
+[[plugin]]
+name = "b"
+marketplace = "market"
+
+[[mcp]]
+name = "kb"
+type = "http"
+url = "https://kb.example.org/mcp"
+headers = { Authorization = "Bearer ${KB_TOKEN}" }
+secrets = ["KB_TOKEN"]
+
+[[mcp]]
+name = "mine"
+type = "sse"
+url = "https://mine.example.org/sse"
+
+[[mcp]]
+name = "jina"
+type = "http"
+url = "https://jina.example.org"
+headers = { Authorization = "Bearer ${JINA_KEY}" }
+secrets = ["JINA_KEY"]
+
+[[agent]]
+name = "reviewer"
+description = "Reviews"
+prompt = "Review."
+
+[[agent]]
+name = "helper"
+description = "Helps"
+prompt = "Help."
+
+[env]
+REGION = "eu"
+"#;
+
+/// The fake `claude` of the `Manifest` tests, shared (written once). Its
+/// control dir is the first entry of its `PATH` (the test daemon's
+/// `Config::path`), and Claude Code's config dir is `<ctl>/../claude`.
+/// Every `plugin …` run appends its argv to `<ctl>/log`. `marketplace
+/// add` registers the name in `<ctl>/market-name` (default `market`), or
+/// fails with `<ctl>/market-fails`. `install <spec>` waits while
+/// `<ctl>/gate-<spec>` exists, then fails with `<ctl>/fail-<spec>`, needs
+/// a person with `<ctl>/manual-<spec>`, or adds the spec to
+/// `installed_plugins.json`. `--help` prints `<ctl>/help`.
+fn fake_plugin_claude() -> &'static Path {
+    static BIN: OnceLock<PathBuf> = OnceLock::new();
+    BIN.get_or_init(|| {
+        let dir = std::env::temp_dir().join(format!("claudio-fake-plugins-{}", Uuid::new_v4()));
+        std::fs::create_dir_all(&dir).unwrap();
+        let bin = dir.join("claude");
+        let script = r#"#!/bin/sh
+ctl="${PATH%%:*}"
+cfg="$ctl/../claude"
+case "$1" in
+--version) echo '9.9.9 (Claude Code)'; exit 0;;
+--help) [ -f "$ctl/help" ] && cat "$ctl/help"; exit 0;;
+update) echo updated; exit 0;;
+plugin) ;;
+*) exit 2;;
+esac
+echo "$*" >> "$ctl/log"
+mkdir -p "$cfg/plugins"
+if [ "$2" = marketplace ]; then
+  if [ -f "$ctl/market-fails" ]; then echo '{"error":"repository not found"}'; exit 1; fi
+  name=$(cat "$ctl/market-name" 2>/dev/null || echo market)
+  printf '{"%s":{"source":{"source":"github","repo":"%s"}}}\n' "$name" "$4" \
+    > "$cfg/plugins/known_marketplaces.json"
+  echo '{"ok":true}'
+  exit 0
+fi
+spec="$3"
+while [ -e "$ctl/gate-$spec" ]; do sleep 0.05; done
+if [ -f "$ctl/fail-$spec" ]; then echo 'Installing'; echo '{"error":"network down"}'; exit 1; fi
+if [ -f "$ctl/manual-$spec" ]; then echo '{"shownCommand":{"command":"npx x","sha256":"ab"}}'; exit 1; fi
+echo "$spec" >> "$ctl/installed"
+{
+  printf '{"version":2,"plugins":{'
+  sep=
+  while read -r k; do printf '%s"%s":[{"scope":"user"}]' "$sep" "$k"; sep=,; done < "$ctl/installed"
+  printf '}}\n'
+} > "$cfg/plugins/installed_plugins.json"
+echo "done $spec" >> "$ctl/log"
+echo '{"ok":true}'
+"#;
+        std::fs::write(&bin, script).unwrap();
+        std::fs::set_permissions(&bin, std::os::unix::fs::PermissionsExt::from_mode(0o755))
+            .unwrap();
+        bin
+    })
+}
 
-/// A daemon whose bundles go to `<dir>/cache/plugins` and whose Claude Code
-/// config is `<dir>/claude` (http allowed, for the local test server).
-async fn bundle_daemon() -> (TestDaemon, PathBuf, PathBuf) {
-    let d = TestDaemon::start().await;
-    let cache = d.dir.join("cache/plugins");
-    let claude = d.dir.join("claude");
-    d.daemon.bundles.set_dirs(cache.clone(), claude.clone());
-    (d, cache, claude)
+/// A `Manifest` test's daemon: the fake plugin claude, manifests cached in
+/// `<dir>/cache/plugins` (TEAM there already), Claude Code's config in
+/// `<dir>/claude`, and the fake's control dir `<dir>/ctl`.
+struct PluginHost {
+    d: TestDaemon,
+    cache: PathBuf,
+    claude: PathBuf,
+    ctl: PathBuf,
 }
 
-fn bundle(name: &str, url: &str, sha256: &str, installed_id: &str) -> Msg {
-    Msg::Bundle {
-        name: name.into(),
-        url: url.into(),
-        sha256: sha256.into(),
-        installed_id: installed_id.into(),
+impl PluginHost {
+    async fn start() -> PluginHost {
+        let dir = TestDaemon::new_dir();
+        let (cache, claude, ctl) = (
+            dir.join("cache/plugins"),
+            dir.join("claude"),
+            dir.join("ctl"),
+        );
+        for d in [&cache, &claude, &ctl] {
+            std::fs::create_dir_all(d).unwrap();
+        }
+        std::fs::write(cache.join("team.toml"), TEAM).unwrap();
+        std::fs::write(
+            cache.join("team.sha256"),
+            format!("{}\n", sha256_hex(TEAM.as_bytes())),
+        )
+        .unwrap();
+        let config = Config {
+            claude: fake_plugin_claude().to_path_buf(),
+            path: Some(format!("{}:/usr/bin:/bin", ctl.display()).into()),
+            ..TestDaemon::config(&dir)
+        };
+        let d = TestDaemon::start_with_config(dir, config);
+        d.daemon.manifests.set_dirs(cache.clone(), claude.clone());
+        PluginHost {
+            d,
+            cache,
+            claude,
+            ctl,
+        }
+    }
+
+    /// Create (or with `on` false, remove) control file `name`.
+    fn set(&self, name: &str, on: bool) {
+        let path = self.ctl.join(name);
+        if on {
+            std::fs::write(path, "").unwrap();
+        } else {
+            let _ = std::fs::remove_file(path);
+        }
+    }
+
+    /// The fake's `plugin …` runs so far.
+    fn log(&self) -> Vec<String> {
+        std::fs::read_to_string(self.ctl.join("log"))
+            .unwrap_or_default()
+            .lines()
+            .map(str::to_owned)
+            .collect()
+    }
+
+    /// Wait until the log has a line equal to `line`.
+    async fn logged(&self, line: &str) {
+        within(async {
+            while !self.log().iter().any(|l| l == line) {
+                tokio::time::sleep(Duration::from_millis(20)).await;
+            }
+        })
+        .await
+    }
+
+    /// Wait until no install of manifest TEAM is queued or running.
+    async fn idle(&self) {
+        within(async {
+            while self.d.daemon.manifests.busy() {
+                tokio::time::sleep(Duration::from_millis(20)).await;
+            }
+        })
+        .await
+    }
+}
+
+/// A `Manifest` request for TEAM (cached; its URL is never reached).
+fn team(secrets: &[&str], disabled: &[&str]) -> Msg {
+    Msg::Manifest {
+        name: "team".into(),
+        url: "https://127.0.0.1:9/never.toml".into(),
+        sha256: sha256_hex(TEAM.as_bytes()),
+        secrets: secrets.iter().map(|s| s.to_string()).collect(),
+        disabled: disabled.iter().map(|s| s.to_string()).collect(),
     }
 }
 
+fn applied(reply: Msg) -> proto::Applied {
+    match reply {
+        Msg::Applied(a) => a,
+        other => panic!("expected Applied, got {other:?}"),
+    }
+}
+
+fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
+    list.iter()
+        .map(|(a, b)| (a.to_string(), b.to_string()))
+        .collect()
+}
+
 fn sha256_hex(bytes: &[u8]) -> String {
     use sha2::Digest;
     format!("{:x}", sha2::Sha256::digest(bytes))
 }
 
-/// Serve `body` at `/kb.zip` on loopback, counting the requests.
-async fn serve_zip(body: &'static [u8]) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
-    use std::sync::atomic::{AtomicUsize, Ordering};
-    let hits = Arc::new(AtomicUsize::new(0));
-    let counted = Arc::clone(&hits);
-    let router = axum::Router::new().route(
-        "/kb.zip",
-        axum::routing::get(move || {
-            counted.fetch_add(1, Ordering::SeqCst);
-            async move { body }
-        }),
+/// The reply comes before any install runs: plugins are `installing`,
+/// every static part is there (servers with their secrets, agents, the
+/// instructions sanitized, the env), and no `-y` reaches the CLI. The
+/// marketplace is added once for both plugins; later replies say
+/// `installed`.
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn manifest_replies_first_then_installs_each_plugin_once() {
+    let h = PluginHost::start().await;
+    h.set("gate-a@market", true);
+    let mut c = h.d.client().await;
+    let a = applied(c.call(team(&["KB_TOKEN", "JINA_KEY"], &[])).await);
+    assert_eq!((a.name.as_str(), a.version.as_str()), ("team", "1.0.0"));
+    assert_eq!(
+        a.skipped,
+        pairs(&[
+            ("plugin:a@market", "installing"),
+            ("plugin:b@market", "installing")
+        ])
     );
-    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
-    let port = listener.local_addr().unwrap().port();
-    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
-    (format!("http://127.0.0.1:{port}/kb.zip"), hits)
+    assert!(a.installed.is_empty() && a.present.is_empty());
+    let mcp: serde_json::Value = serde_json::from_str(a.mcp_config.as_deref().unwrap()).unwrap();
+    let servers: Vec<&String> = mcp["mcpServers"].as_object().unwrap().keys().collect();
+    assert_eq!(servers, ["jina", "kb", "mine"]);
+    assert_eq!(
+        mcp["mcpServers"]["kb"]["headers"]["Authorization"],
+        "Bearer ${KB_TOKEN}"
+    );
+    let agents: serde_json::Value = serde_json::from_str(a.agents.as_deref().unwrap()).unwrap();
+    assert_eq!(agents["reviewer"]["prompt"], "Review.");
+    assert_eq!(a.instructions, "Team rules.");
+    assert_eq!(a.env, pairs(&[("REGION", "eu")]));
+    // The fake's `--help` lists neither flag.
+    assert_eq!(a.dropped, ["--mcp-config", "--agents"]);
+
+    // Asked again while `a` is blocked: nothing is queued twice.
+    h.logged("plugin install a@market --scope user --json")
+        .await;
+    let again = applied(c.call(team(&["KB_TOKEN", "JINA_KEY"], &[])).await);
+    assert_eq!(again.skipped, a.skipped);
+    h.set("gate-a@market", false);
+    h.idle().await;
+    assert_eq!(
+        h.log(),
+        [
+            "plugin marketplace add acme/market --scope user --json",
+            "plugin install a@market --scope user --json",
+            "done a@market",
+            "plugin install b@market --scope user --json",
+            "done b@market",
+        ]
+    );
+    assert!(!h
+        .log()
+        .iter()
+        .any(|l| l.contains("-y") || l.contains("--yes") || l.contains("--accept-command")));
+    let done = applied(c.call(team(&["KB_TOKEN", "JINA_KEY"], &[])).await);
+    assert_eq!(done.installed, ["plugin:a@market", "plugin:b@market"]);
+    assert!(done.skipped.is_empty() && done.present.is_empty());
+    assert_eq!(h.log().len(), 5, "nothing ran again");
+    assert!(h.cache.join("team.toml").is_file());
+}
+
+/// What is already there is left alone: an installed plugin (any scope) is
+/// `present`, a server in `.claude.json` and an agent in `agents/` are the
+/// member's (`yours`), a server whose secret the request lacks is skipped,
+/// and a disabled id is neither applied nor listed.
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn manifest_leaves_the_members_own_alone_and_honours_disabled() {
+    let h = PluginHost::start().await;
+    std::fs::create_dir_all(h.claude.join("plugins")).unwrap();
+    std::fs::create_dir_all(h.claude.join("agents")).unwrap();
+    std::fs::write(
+        h.claude.join("plugins/installed_plugins.json"),
+        r#"{"version":2,"plugins":{"a@market":[{"scope":"project"}]}}"#,
+    )
+    .unwrap();
+    std::fs::write(
+        h.claude.join(".claude.json"),
+        r#"{"mcpServers":{"mine":{}}}"#,
+    )
+    .unwrap();
+    std::fs::write(h.claude.join("agents/helper.md"), "my helper").unwrap();
+    let mut c = h.d.client().await;
+    let a = applied(
+        c.call(team(
+            &["KB_TOKEN"],
+            &["plugin:b@market", "env", "instructions"],
+        ))
+        .await,
+    );
+    assert_eq!(a.present, ["plugin:a@market"]);
+    assert_eq!(
+        a.skipped,
+        pairs(&[
+            ("mcp:mine", "yours"),
+            ("mcp:jina", "missing secret JINA_KEY"),
+            ("agent:helper", "yours"),
+        ])
+    );
+    let mcp: serde_json::Value = serde_json::from_str(a.mcp_config.as_deref().unwrap()).unwrap();
+    let servers: Vec<&String> = mcp["mcpServers"].as_object().unwrap().keys().collect();
+    assert_eq!(servers, ["kb"]);
+    let agents: serde_json::Value = serde_json::from_str(a.agents.as_deref().unwrap()).unwrap();
+    assert!(agents.get("helper").is_none() && agents.get("reviewer").is_some());
+    assert!(a.instructions.is_empty() && a.env.is_empty());
+    h.idle().await;
+    assert!(h.log().is_empty(), "nothing to install: {:?}", h.log());
+
+    // Every server and agent disabled: no JSON at all.
+    let all = [
+        "plugin:b@market",
+        "mcp:kb",
+        "mcp:mine",
+        "mcp:jina",
+        "agent:reviewer",
+        "agent:helper",
+    ];
+    let a = applied(c.call(team(&["KB_TOKEN"], &all)).await);
+    assert_eq!((a.mcp_config, a.agents), (None, None));
+    assert!(a.skipped.is_empty(), "{:?}", a.skipped);
+
+    // An unreadable `.claude.json` skips every server, with the reason.
+    std::fs::write(h.claude.join(".claude.json"), "{").unwrap();
+    let a = applied(
+        c.call(team(&["KB_TOKEN", "JINA_KEY"], &["plugin:b@market"]))
+            .await,
+    );
+    assert_eq!(a.mcp_config, None);
+    let unreadable: Vec<&(String, String)> = a
+        .skipped
+        .iter()
+        .filter(|(_, why)| why == "cannot read .claude.json")
+        .collect();
+    assert_eq!(unreadable.len(), 3, "{:?}", a.skipped);
 }
 
-fn files_in(dir: &Path) -> Vec<String> {
-    let mut names: Vec<String> = std::fs::read_dir(dir)
-        .map(|rd| {
-            rd.flatten()
-                .map(|e| e.file_name().to_string_lossy().into_owned())
-                .collect()
-        })
-        .unwrap_or_default();
-    names.sort();
-    names
+/// A failed install is skipped with the CLI's reason and tried again only
+/// after 10 minutes; one that needs a person stays `needs manual install`
+/// for the daemon's life.
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn failed_installs_retry_after_the_window_and_manual_ones_never() {
+    let h = PluginHost::start().await;
+    h.set("fail-a@market", true);
+    h.set("manual-b@market", true);
+    let mut c = h.d.client().await;
+    c.call(team(&[], &[])).await;
+    h.idle().await;
+    let a = applied(c.call(team(&[], &[])).await);
+    let manual = "needs manual install: claude plugin install b@market";
+    let failed = pairs(&[
+        ("plugin:a@market", "network down"),
+        ("plugin:b@market", manual),
+    ]);
+    assert_eq!(&a.skipped[..2], &failed[..]);
+    let runs = h.log().len();
+
+    // Within the window: nothing runs again.
+    h.d.daemon.manifests.advance(Duration::from_secs(9 * 60));
+    let a = applied(c.call(team(&[], &[])).await);
+    assert_eq!(&a.skipped[..2], &failed[..]);
+    h.idle().await;
+    assert_eq!(h.log().len(), runs);
+
+    // After it: the failure is tried again (and now works); the manual one
+    // still waits for a person.
+    h.set("fail-a@market", false);
+    h.d.daemon.manifests.advance(Duration::from_secs(2 * 60));
+    let a = applied(c.call(team(&[], &[])).await);
+    assert_eq!(a.skipped[0], pairs(&[("plugin:a@market", "installing")])[0]);
+    assert_eq!(a.skipped[1].1, manual);
+    h.idle().await;
+    let a = applied(c.call(team(&[], &[])).await);
+    assert_eq!(a.installed, ["plugin:a@market"]);
+    assert_eq!(a.skipped[0], pairs(&[("plugin:b@market", manual)])[0]);
+    let installs = h
+        .log()
+        .iter()
+        .filter(|l| l.contains("install b@market"))
+        .count();
+    assert_eq!(installs, 1);
+}
+
+/// A marketplace Claude Code files under another name than the manifest's
+/// skips its plugins with that reason (and is not added again at once).
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn a_marketplace_added_under_another_name_skips_its_plugins() {
+    let h = PluginHost::start().await;
+    std::fs::write(h.ctl.join("market-name"), "acme").unwrap();
+    let mut c = h.d.client().await;
+    c.call(team(&[], &[])).await;
+    h.idle().await;
+    let a = applied(c.call(team(&[], &[])).await);
+    let why = "marketplace market: added under another name";
+    assert_eq!(
+        &a.skipped[..2],
+        &pairs(&[("plugin:a@market", why), ("plugin:b@market", why)])[..]
+    );
+    assert_eq!(
+        h.log(),
+        ["plugin marketplace add acme/market --scope user --json"]
+    );
 }
 
+/// Installs share `claude update`'s lock, one subprocess at a time: an
+/// update during an install is refused with a message naming both, and a
+/// waiter that queued during one install gets the lock before the next.
 #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
-async fn bundle_refuses_a_bad_name_hash_or_url() {
+async fn installs_and_claude_update_take_turns() {
+    let h = PluginHost::start().await;
+    h.set("gate-a@market", true);
+    let mut c = h.d.client().await;
+    c.call(team(&[], &[])).await;
+    h.logged("plugin install a@market --scope user --json")
+        .await;
+    assert_eq!(
+        c.call(Msg::UpdateClaude { install: false }).await,
+        Msg::Error {
+            message: "a claude update or plugin install is already running on this host".into()
+        }
+    );
+    // Queue for the lock while `a` installs; `b` must wait for us.
+    let daemon = Arc::clone(&h.d.daemon);
+    let (got, held) = tokio::sync::oneshot::channel();
+    let (release, released) = tokio::sync::oneshot::channel::<()>();
+    let waiter = tokio::spawn(async move {
+        let _lock = daemon.updating.lock().await;
+        let _ = got.send(());
+        let _ = released.await;
+    });
+    tokio::time::sleep(Duration::from_millis(100)).await;
+    h.set("gate-a@market", false);
+    within(held).await.unwrap();
+    h.logged("done a@market").await;
+    tokio::time::sleep(Duration::from_millis(300)).await;
+    assert!(
+        !h.log().iter().any(|l| l.contains("install b@market")),
+        "{:?}",
+        h.log()
+    );
+    release.send(()).unwrap();
+    waiter.await.unwrap();
+    h.idle().await;
+    // Between installs (here: after them) an update runs.
+    assert!(matches!(
+        c.call(Msg::UpdateClaude { install: false }).await,
+        Msg::ClaudeUpdated { ok: true, .. }
+    ));
+}
+
+/// The fake's `--help` lists the flags: nothing is dropped.
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn applied_names_the_flags_this_claude_lacks() {
+    let h = PluginHost::start().await;
+    std::fs::write(
+        h.ctl.join("help"),
+        "  --agents <json>  Agents\n  --mcp-config <configs...>  MCP\n",
+    )
+    .unwrap();
+    let mut c = h.d.client().await;
+    let a = applied(
+        c.call(team(&[], &["plugin:a@market", "plugin:b@market"]))
+            .await,
+    );
+    assert!(a.dropped.is_empty(), "{:?}", a.dropped);
+}
+
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn manifest_refuses_a_bad_request() {
     let d = TestDaemon::start().await;
     let cache = d.dir.join("cache/plugins");
     // The real daemon: no http, even on loopback.
     let mut c = d.client().await;
     let good = "a".repeat(64);
+    let req = |name: &str, url: &str, sha: &str, secrets: Vec<String>, disabled: Vec<String>| {
+        Msg::Manifest {
+            name: name.into(),
+            url: url.into(),
+            sha256: sha.into(),
+            secrets,
+            disabled,
+        }
+    };
+    let url = "https://e.org/x.toml";
+    let many_secrets: Vec<String> = (0..65).map(|i| format!("K{i}")).collect();
+    let many_ids: Vec<String> = (0..257).map(|i| format!("mcp:s{i}")).collect();
     for (msg, shown) in [
-        (bundle("../x", "https://e.org/x.zip", &good, ""), "?"),
-        (bundle("Kb", "https://e.org/x.zip", &good, ""), "?"),
+        (req("../x", url, &good, vec![], vec![]), "?"),
+        (req("kb", url, "abc", vec![], vec![]), "kb"),
         (
-            bundle("kb", "https://e.org/x.zip", &"A".repeat(64), ""),
+            req("kb", "http://e.org/x.toml", &good, vec![], vec![]),
             "kb",
         ),
-        (bundle("kb", "https://e.org/x.zip", "abc", ""), "kb"),
-        (bundle("kb", "http://e.org/x.zip", &good, ""), "kb"),
-        (bundle("kb", "file:///etc/passwd", &good, ""), "kb"),
+        (req("kb", "file:///etc/passwd", &good, vec![], vec![]), "kb"),
+        (req("kb", url, &good, vec!["PATH".into()], vec![]), "kb"),
+        (req("kb", url, &good, vec!["bad key".into()], vec![]), "kb"),
+        (req("kb", url, &good, many_secrets, vec![]), "kb"),
+        (req("kb", url, &good, vec![], vec!["skill:x".into()]), "kb"),
+        (req("kb", url, &good, vec![], many_ids), "kb"),
     ] {
         let reply = c.call(msg.clone()).await;
         assert_eq!(
             reply,
             Msg::Error {
-                message: format!("bundle {shown}: bad request")
+                message: format!("manifest {shown}: bad request")
             },
             "{msg:?}"
         );
@@ -2122,143 +2570,127 @@ async fn bundle_refuses_a_bad_name_hash_or_url() {
     assert!(!cache.exists(), "nothing written");
 }
 
-#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
-async fn bundle_is_skipped_when_claude_code_has_the_plugin_installed() {
-    let (d, cache, claude) = bundle_daemon().await;
-    std::fs::create_dir_all(claude.join("plugins")).unwrap();
-    std::fs::write(
-        claude.join("plugins/installed_plugins.json"),
-        r#"{"version":2,"plugins":{"kb@vocdoni":[{"scope":"user"}]}}"#,
-    )
-    .unwrap();
-    let mut c = d.client().await;
-    let url = "https://127.0.0.1:9/never.zip";
-    let reply = c
-        .call(bundle("kb", url, &"a".repeat(64), "kb@vocdoni"))
-        .await;
-    assert_eq!(
-        reply,
-        Msg::Bundled {
-            name: "kb".into(),
-            path: None,
-            installed: true
-        }
-    );
-    assert!(!cache.exists(), "no download");
-    // Another id is not installed: on to the cache and the download.
-    let reply = c.call(bundle("kb", url, &"a".repeat(64), "kb@other")).await;
-    assert!(
-        matches!(&reply, Msg::Error { message } if message.starts_with("bundle kb: download failed")),
-        "{reply:?}"
+/// Serve `body` at `/team.toml` on loopback, counting the requests.
+async fn serve(body: &'static [u8]) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
+    use std::sync::atomic::{AtomicUsize, Ordering};
+    let hits = Arc::new(AtomicUsize::new(0));
+    let counted = Arc::clone(&hits);
+    let router = axum::Router::new().route(
+        "/team.toml",
+        axum::routing::get(move || {
+            counted.fetch_add(1, Ordering::SeqCst);
+            async move { body }
+        }),
     );
-    // A broken file counts as not installed.
-    std::fs::write(claude.join("plugins/installed_plugins.json"), "{").unwrap();
-    let reply = c
-        .call(bundle("kb", url, &"a".repeat(64), "kb@vocdoni"))
-        .await;
-    assert!(matches!(reply, Msg::Error { .. }), "{reply:?}");
+    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
+    let port = listener.local_addr().unwrap().port();
+    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
+    (format!("http://127.0.0.1:{port}/team.toml"), hits)
 }
 
-#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
-async fn a_cached_zip_with_its_sidecar_is_reused() {
-    let (d, cache, _) = bundle_daemon().await;
-    std::fs::create_dir_all(&cache).unwrap();
-    std::fs::write(cache.join("kb.zip"), b"zip").unwrap();
-    let hash = sha256_hex(b"zip");
-    std::fs::write(cache.join("kb.sha256"), format!("{hash}\n")).unwrap();
-    let mut c = d.client().await;
-    // A URL the daemon never reaches.
-    let reply = c
-        .call(bundle("kb", "https://127.0.0.1:9/kb.zip", &hash, ""))
-        .await;
-    let path = cache.join("kb.zip").to_string_lossy().into_owned();
-    assert_eq!(
-        reply,
-        Msg::Bundled {
-            name: "kb".into(),
-            path: Some(path),
-            installed: false
-        }
-    );
+fn files_in(dir: &Path) -> Vec<String> {
+    let mut names: Vec<String> = std::fs::read_dir(dir)
+        .map(|rd| {
+            rd.flatten()
+                .map(|e| e.file_name().to_string_lossy().into_owned())
+                .collect()
+        })
+        .unwrap_or_default();
+    names.sort();
+    names
+}
+
+fn download(url: &str, sha256: &str) -> Msg {
+    Msg::Manifest {
+        name: "team".into(),
+        url: url.into(),
+        sha256: sha256.into(),
+        secrets: vec![],
+        disabled: vec!["plugin:a@market".into(), "plugin:b@market".into()],
+    }
 }
 
 #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
 async fn a_download_is_verified_stored_privately_and_shared() {
     use std::os::unix::fs::PermissionsExt;
     use std::sync::atomic::Ordering;
-    let (d, cache, _) = bundle_daemon().await;
-    let (url, hits) = serve_zip(b"the bundle").await;
-    let hash = sha256_hex(b"the bundle");
-    let mut a = d.client().await;
-    let mut b = d.client().await;
-    let (ra, rb) = tokio::join!(
-        a.call(bundle("kb", &url, &hash, "")),
-        b.call(bundle("kb", &url, &hash, ""))
-    );
-    let path = cache.join("kb.zip").to_string_lossy().into_owned();
-    for reply in [ra, rb] {
-        assert_eq!(
-            reply,
-            Msg::Bundled {
-                name: "kb".into(),
-                path: Some(path.clone()),
-                installed: false
-            }
-        );
-    }
+    let h = PluginHost::start().await;
+    std::fs::remove_dir_all(&h.cache).unwrap();
+    let (url, hits) = serve(TEAM.as_bytes()).await;
+    let hash = sha256_hex(TEAM.as_bytes());
+    let mut a = h.d.client().await;
+    let mut b = h.d.client().await;
+    let (ra, rb) = tokio::join!(a.call(download(&url, &hash)), b.call(download(&url, &hash)));
+    assert_eq!(applied(ra), applied(rb));
     assert_eq!(hits.load(Ordering::SeqCst), 1, "one download for both");
-    assert_eq!(std::fs::read(cache.join("kb.zip")).unwrap(), b"the bundle");
     assert_eq!(
-        std::fs::read_to_string(cache.join("kb.sha256")).unwrap(),
+        std::fs::read_to_string(h.cache.join("team.toml")).unwrap(),
+        TEAM
+    );
+    assert_eq!(
+        std::fs::read_to_string(h.cache.join("team.sha256")).unwrap(),
         format!("{hash}\n")
     );
-    assert_eq!(files_in(&cache), ["kb.sha256", "kb.zip"]);
-    let mode = std::fs::metadata(&cache).unwrap().permissions().mode() & 0o777;
+    assert_eq!(files_in(&h.cache), ["team.sha256", "team.toml"]);
+    let mode = std::fs::metadata(&h.cache).unwrap().permissions().mode() & 0o777;
     assert_eq!(mode, 0o700);
 }
 
 #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
-async fn a_wrong_hash_is_an_error_and_leaves_no_file() {
-    let (d, cache, _) = bundle_daemon().await;
-    let (url, _) = serve_zip(b"tampered").await;
-    let mut c = d.client().await;
-    let reply = c.call(bundle("kb", &url, &"b".repeat(64), "")).await;
+async fn a_wrong_hash_a_bad_manifest_or_an_oversized_one_is_an_error() {
+    static BIG: OnceLock<Vec<u8>> = OnceLock::new();
+    let h = PluginHost::start().await;
+    std::fs::remove_dir_all(&h.cache).unwrap();
+    let (url, _) = serve(b"tampered").await;
+    let mut c = h.d.client().await;
     assert_eq!(
-        reply,
+        c.call(download(&url, &"b".repeat(64))).await,
         Msg::Error {
-            message: "bundle kb: hash mismatch".into()
+            message: "manifest team: hash mismatch".into()
         }
     );
-    assert!(files_in(&cache).is_empty(), "{:?}", files_in(&cache));
-
-    // A new version replaces the old zip in place, and its sidecar.
-    std::fs::write(cache.join("kb.zip"), b"old").unwrap();
-    std::fs::write(cache.join("kb.sha256"), sha256_hex(b"old")).unwrap();
-    let reply = c
-        .call(bundle("kb", &url, &sha256_hex(b"tampered"), ""))
-        .await;
-    assert!(
-        matches!(reply, Msg::Bundled { path: Some(_), .. }),
-        "{reply:?}"
+    assert!(files_in(&h.cache).is_empty(), "{:?}", files_in(&h.cache));
+    // Downloaded and verified, but not a manifest: refused with the reason.
+    assert_eq!(
+        c.call(download(&url, &sha256_hex(b"tampered"))).await,
+        Msg::Error {
+            message: "manifest team: not valid TOML, or a value of the wrong type (line 1)".into()
+        }
+    );
+    let other = TEAM.replacen("name = \"team\"", "name = \"other\"", 1);
+    let other: &'static [u8] = Box::leak(other.into_bytes().into_boxed_slice());
+    let (url, _) = serve(other).await;
+    assert_eq!(
+        c.call(download(&url, &sha256_hex(other))).await,
+        Msg::Error {
+            message: "manifest team: the manifest is other's".into()
+        }
+    );
+    let big = BIG.get_or_init(|| vec![b'#'; (256 << 10) + 1]);
+    let (url, _) = serve(big).await;
+    assert_eq!(
+        c.call(download(&url, &sha256_hex(big))).await,
+        Msg::Error {
+            message: "manifest team: over 256 KiB".into()
+        }
     );
-    assert_eq!(std::fs::read(cache.join("kb.zip")).unwrap(), b"tampered");
-    assert_eq!(files_in(&cache), ["kb.sha256", "kb.zip"]);
 }
 
 #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
-async fn the_cache_keeps_at_most_32_bundles() {
-    let (d, cache, _) = bundle_daemon().await;
-    std::fs::create_dir_all(&cache).unwrap();
-    for i in 0..super::bundle::MAX_BUNDLES {
-        std::fs::write(cache.join(format!("p{i}.zip")), b"z").unwrap();
+async fn the_cache_keeps_at_most_32_manifests() {
+    let h = PluginHost::start().await;
+    std::fs::remove_dir_all(&h.cache).unwrap();
+    std::fs::create_dir_all(&h.cache).unwrap();
+    for i in 0..super::manifest::MAX_MANIFESTS {
+        std::fs::write(h.cache.join(format!("p{i}.toml")), b"z").unwrap();
     }
-    let (url, hits) = serve_zip(b"x").await;
-    let mut c = d.client().await;
-    let reply = c.call(bundle("kb", &url, &sha256_hex(b"x"), "")).await;
+    let (url, hits) = serve(TEAM.as_bytes()).await;
+    let mut c = h.d.client().await;
+    let reply = c.call(download(&url, &sha256_hex(TEAM.as_bytes()))).await;
     assert_eq!(
         reply,
         Msg::Error {
-            message: "bundle kb: cache holds 32 bundles already".into()
+            message: "manifest team: cache holds 32 manifests already".into()
         }
     );
     assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 0);
`````
Run: `umask 022; cargo test --locked --bin claudio daemon::`
Expected: `` error[E0609]: no field `manifests` on type `std::sync::Arc<Daemon>` ``, `` error[E0433]: failed to resolve: use of undeclared type `Outcome` ``, `` error[E0432]: unresolved import `super::bundle` `` (the test step's module hunk already swaps `mod bundle;` for `mod manifest;`), ending `` error: could not compile `claudio` (bin "claudio" test) due to 34 previous errors; 1 warning emitted ``

Step 2: implement
`````diff
diff --git a/src/daemon/bundle.rs b/src/daemon/bundle.rs
deleted file mode 100644
index cdcae31..0000000
--- a/src/daemon/bundle.rs
+++ /dev/null
@@ -1,267 +0,0 @@
-//! `Bundle`: a proxy plugin's zip on this host, for claude's `--plugin-dir`.
-//!
-//! The zips live in `<cache>/plugins` (0700): `<name>.zip` and its sidecar
-//! `<name>.sha256`, one pair per plugin name, so a journaled
-//! `--plugin-dir <path>` stays valid across versions. claudio never unpacks
-//! a zip; the hash pins the exact file the proxy's admin registered.
-//!
-//! Every request is checked here (the name is a path component). A download
-//! is https only, capped at [`MAX_BYTES`] and [`TOTAL_TIMEOUT`], streamed to
-//! a temp file in the cache dir, verified, then renamed into place; any
-//! failure leaves no partial file. One download per plugin name runs at a
-//! time: a second request waits for it and then finds the zip cached.
-
-use std::collections::HashMap;
-use std::io::Write;
-use std::os::unix::fs::OpenOptionsExt;
-use std::path::{Path, PathBuf};
-use std::sync::{Arc, Mutex};
-use std::time::Duration;
-
-use sha2::{Digest, Sha256};
-
-use crate::paths;
-use crate::proto::Msg;
-use crate::proxy::plugins::{name_ok, sha256_ok};
-
-/// A bundle's size cap.
-pub const MAX_BYTES: u64 = 64 << 20;
-/// At most this many zips are kept.
-pub const MAX_BUNDLES: usize = 32;
-const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
-const TOTAL_TIMEOUT: Duration = Duration::from_secs(120);
-/// `installed_plugins.json` is read up to this size.
-const MAX_INSTALLED: u64 = 1 << 20;
-
-/// Where bundles go and where Claude Code keeps its plugins, and the
-/// per-name download locks.
-pub(crate) struct Bundles {
-    dirs: Mutex<Dirs>,
-    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
-}
-
-#[derive(Clone)]
-struct Dirs {
-    /// `<cache>/plugins`.
-    cache: PathBuf,
-    /// `$CLAUDE_CONFIG_DIR`, else `~/.claude`.
-    claude_config: PathBuf,
-    /// Tests serve bundles over plain http on loopback.
-    allow_http: bool,
-}
-
-impl Bundles {
-    /// The standard places: [`paths::cache_dir`] and the Claude Code config
-    /// dir the sessions see (this daemon's `CLAUDE_CONFIG_DIR`, which every
-    /// session inherits).
-    pub(crate) fn new() -> Bundles {
-        let claude_config = std::env::var_os("CLAUDE_CONFIG_DIR")
-            .filter(|v| !v.is_empty())
-            .map(PathBuf::from)
-            .unwrap_or_else(|| paths::home().join(".claude"));
-        Bundles {
-            dirs: Mutex::new(Dirs {
-                cache: paths::cache_dir().join("plugins"),
-                claude_config,
-                allow_http: false,
-            }),
-            locks: Mutex::new(HashMap::new()),
-        }
-    }
-
-    fn dirs(&self) -> Dirs {
-        self.dirs.lock().unwrap_or_else(|e| e.into_inner()).clone()
-    }
-
-    /// Use `cache` and `claude_config` instead, and accept http URLs.
-    #[cfg(test)]
-    pub(crate) fn set_dirs(&self, cache: PathBuf, claude_config: PathBuf) {
-        *self.dirs.lock().unwrap_or_else(|e| e.into_inner()) = Dirs {
-            cache,
-            claude_config,
-            allow_http: true,
-        };
-    }
-
-    fn lock(&self, name: &str) -> Arc<tokio::sync::Mutex<()>> {
-        let mut locks = self.locks.lock().unwrap_or_else(|e| e.into_inner());
-        Arc::clone(locks.entry(name.to_owned()).or_default())
-    }
-}
-
-/// Answer a `Bundle` request: `Bundled`, or `Error "bundle <name>: …"`.
-pub(crate) async fn handle(
-    bundles: &Bundles,
-    name: String,
-    url: String,
-    sha256: String,
-    installed_id: String,
-) -> Msg {
-    let dirs = bundles.dirs();
-    let scheme_ok = url.starts_with("https://") || (dirs.allow_http && url.starts_with("http://"));
-    if !name_ok(&name) || !sha256_ok(&sha256) || !scheme_ok {
-        return Msg::Error {
-            message: format!("bundle {}: bad request", safe_name(&name)),
-        };
-    }
-    if !installed_id.is_empty() && is_installed(&dirs.claude_config, &installed_id).await {
-        return Msg::Bundled {
-            name,
-            path: None,
-            installed: true,
-        };
-    }
-    // One download per name at a time; whoever waited finds it cached.
-    let lock = bundles.lock(&name);
-    let _held = lock.lock().await;
-    match ready(&dirs, &name, &url, &sha256).await {
-        Ok(path) => Msg::Bundled {
-            name,
-            path: Some(path.to_string_lossy().into_owned()),
-            installed: false,
-        },
-        Err(why) => {
-            tracing::warn!(bundle = %name, error = %why, "bundle not ready");
-            Msg::Error {
-                message: format!("bundle {name}: {why}"),
-            }
-        }
-    }
-}
-
-/// A name for an error about a refused request: shown only when valid.
-fn safe_name(name: &str) -> &str {
-    if name_ok(name) {
-        name
-    } else {
-        "?"
-    }
-}
-
-/// Whether Claude Code has plugin `id` installed: a key of `plugins` in
-/// `<claude_config>/plugins/installed_plugins.json`. A missing, oversized
-/// or unparsable file counts as not installed.
-async fn is_installed(claude_config: &Path, id: &str) -> bool {
-    let file = claude_config.join("plugins/installed_plugins.json");
-    let id = id.to_owned();
-    tokio::task::spawn_blocking(move || {
-        let meta = std::fs::metadata(&file).ok()?;
-        if !meta.is_file() || meta.len() > MAX_INSTALLED {
-            return None;
-        }
-        let raw = std::fs::read(&file).ok()?;
-        let v: serde_json::Value = serde_json::from_slice(&raw).ok()?;
-        Some(v.get("plugins")?.as_object()?.contains_key(&id))
-    })
-    .await
-    .ok()
-    .flatten()
-    .unwrap_or(false)
-}
-
-/// The zip of `name` with hash `sha256`: cached, else downloaded.
-async fn ready(dirs: &Dirs, name: &str, url: &str, sha256: &str) -> Result<PathBuf, String> {
-    let dir = dirs.cache.clone();
-    paths::ensure_private_dir(&dir).map_err(|e| format!("cache dir: {e}"))?;
-    let zip = dir.join(format!("{name}.zip"));
-    let sidecar = dir.join(format!("{name}.sha256"));
-    if zip.is_file() && std::fs::read_to_string(&sidecar).is_ok_and(|s| s.trim() == sha256) {
-        return Ok(zip);
-    }
-    if !zip.exists() && count_zips(&dir) >= MAX_BUNDLES {
-        return Err(format!("cache holds {MAX_BUNDLES} bundles already"));
-    }
-    let tmp = dir.join(format!(".{name}.{}.part", uuid::Uuid::new_v4().simple()));
-    let result = download(url, &tmp, sha256, dirs.allow_http).await;
-    let placed = result.and_then(|()| place(&tmp, &zip, &sidecar, sha256));
-    if placed.is_err() {
-        let _ = std::fs::remove_file(&tmp);
-    }
-    placed.map(|()| zip)
-}
-
-fn count_zips(dir: &Path) -> usize {
-    std::fs::read_dir(dir)
-        .map(|entries| {
-            entries
-                .flatten()
-                .filter(|e| e.file_name().to_string_lossy().ends_with(".zip"))
-                .count()
-        })
-        .unwrap_or(0)
-}
-
-/// Move a verified download into place. The old sidecar goes first, so no
-/// moment pairs a hash with a zip it does not describe; a crash in between
-/// leaves no sidecar, and the next request downloads again.
-fn place(tmp: &Path, zip: &Path, sidecar: &Path, sha256: &str) -> Result<(), String> {
-    match std::fs::remove_file(sidecar) {
-        Ok(()) => {}
-        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
-        Err(e) => return Err(format!("cannot replace the sidecar: {e}")),
-    }
-    std::fs::rename(tmp, zip).map_err(|e| format!("cannot store the bundle: {e}"))?;
-    paths::write_atomic(sidecar, format!("{sha256}\n").as_bytes())
-        .map_err(|e| format!("cannot write the sidecar: {e}"))
-}
-
-/// Stream `url` into a new file `tmp` (0600), checking the size cap and,
-/// at the end, the SHA-256. Redirects are followed to https URLs only.
-async fn download(url: &str, tmp: &Path, sha256: &str, allow_http: bool) -> Result<(), String> {
-    let policy = reqwest::redirect::Policy::custom(move |attempt| {
-        let https = attempt.url().scheme() == "https";
-        if attempt.previous().len() >= 5 {
-            attempt.error("too many redirects")
-        } else if https || allow_http {
-            attempt.follow()
-        } else {
-            attempt.error("redirect to a non-https URL")
-        }
-    });
-    let client = reqwest::Client::builder()
-        .connect_timeout(CONNECT_TIMEOUT)
-        .timeout(TOTAL_TIMEOUT)
-        .redirect(policy)
-        .build()
-        .map_err(|e| format!("http client: {e}"))?;
-    let mut resp = client
-        .get(url)
-        .send()
-        .await
-        .map_err(|e| format!("download failed: {}", e.without_url()))?;
-    if !resp.status().is_success() {
-        return Err(format!("download failed: HTTP {}", resp.status().as_u16()));
-    }
-    if resp.content_length().is_some_and(|n| n > MAX_BYTES) {
-        return Err("bundle over 64 MiB".to_owned());
-    }
-    let mut file = std::fs::OpenOptions::new()
-        .write(true)
-        .create_new(true)
-        .mode(0o600)
-        .custom_flags(libc::O_NOFOLLOW)
-        .open(tmp)
-        .map_err(|e| format!("cannot create a temp file: {e}"))?;
-    let mut hasher = Sha256::new();
-    let mut total: u64 = 0;
-    while let Some(chunk) = resp
-        .chunk()
-        .await
-        .map_err(|e| format!("download failed: {}", e.without_url()))?
-    {
-        total += chunk.len() as u64;
-        if total > MAX_BYTES {
-            return Err("bundle over 64 MiB".to_owned());
-        }
-        hasher.update(&chunk);
-        file.write_all(&chunk)
-            .map_err(|e| format!("cannot write the bundle: {e}"))?;
-    }
-    file.sync_all()
-        .map_err(|e| format!("cannot write the bundle: {e}"))?;
-    let got = format!("{:x}", hasher.finalize());
-    if got != sha256 {
-        return Err("hash mismatch".to_owned());
-    }
-    Ok(())
-}
diff --git a/src/daemon/manifest.rs b/src/daemon/manifest.rs
index 1fb8474..36dc2be 100644
--- a/src/daemon/manifest.rs
+++ b/src/daemon/manifest.rs
@@ -1,3 +1,810 @@
+//! `Manifest`: apply a proxy plugin's manifest on this host.
+//!
+//! The manifest (`crate::plugins::manifest`) is downloaded once per hash
+//! into `<cache>/plugins` (0700) as `<name>.toml` with its sidecar
+//! `<name>.sha256`: https only, capped at 256 KiB and [`TOTAL_TIMEOUT`],
+//! streamed to a temp file, verified, then renamed into place; one download
+//! per plugin name at a time.
+//!
+//! The reply ([`Applied`]) goes as soon as the manifest is parsed and its
+//! static parts are rendered: the MCP servers and agents every session
+//! gets, the instructions and the env, and each item's outcome. Plugins
+//! that are neither installed nor remembered are `installing`: their
+//! installs run after the reply, in one task per manifest hash, each
+//! `claude plugin …` subprocess under `Daemon::updating` (the lock `claude
+//! update` takes), so `~/.claude` is written by one at a time. The daemon
+//! itself never writes there: it reads Claude Code's files (capped, regular
+//! files of this uid, never logged) and runs the CLI, never with `-y`.
+//!
+//! What the installs did is remembered per (manifest hash, id): installed
+//! by this daemon (for its life), needing a manual install (for its life:
+//! a person has to act), or failed (retried after [`RETRY_AFTER`]).
+
+use std::collections::{HashMap, HashSet, VecDeque};
+use std::io::{Read, Write};
+use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
+use std::path::{Path, PathBuf};
+use std::process::Stdio;
+use std::sync::{Arc, Mutex, MutexGuard};
+use std::time::{Duration, Instant};
+
+use serde_json::Value;
+use sha2::{Digest, Sha256};
+use tokio::io::{AsyncRead, AsyncReadExt};
+use tokio::process::Command;
+
+use super::Daemon;
+use crate::claude::latex::sanitize;
+use crate::claude::SESSION_MARKERS;
+use crate::paths;
+use crate::plugins::manifest::{self, Manifest, Marketplace, PluginRef};
+use crate::proto::{Applied, Msg};
+use crate::proxy::plugins::{key_ok, name_ok, sha256_ok, test_http_ok};
+
+/// A manifest's size cap.
+const MAX_BYTES: u64 = manifest::MAX_BYTES as u64;
+/// At most this many manifests are kept.
+pub const MAX_MANIFESTS: usize = 32;
+const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
+const TOTAL_TIMEOUT: Duration = Duration::from_secs(120);
+/// Claude Code's files are read up to this size.
+const MAX_CONFIG: u64 = 16 << 20;
+/// A request's caps: secret names and disabled ids.
+const MAX_SECRETS: usize = 64;
+const MAX_DISABLED: usize = 256;
+/// A failed install is tried again after this long.
+pub const RETRY_AFTER: Duration = Duration::from_secs(600);
+/// One `claude plugin …` run's deadline.
+const RUN_TIMEOUT: Duration = Duration::from_secs(120);
+/// Each output stream of a run is kept up to this size.
+const OUTPUT_CAP: usize = 64 << 10;
+/// A reason from the CLI is cut to this many characters.
+const REASON_CHARS: usize = 200;
+
+/// The reason of a plugin whose install is queued or running.
+pub const INSTALLING: &str = "installing";
+
+/// Where manifests go, where Claude Code keeps its files, the per-name
+/// download locks, and what the installs did.
+pub(crate) struct Manifests {
+    dirs: Mutex<Dirs>,
+    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
+    memory: Mutex<Memory>,
+    /// Added to the clock (tests move time forward).
+    skew: Mutex<Duration>,
+}
+
+#[derive(Clone)]
+struct Dirs {
+    /// `<cache>/plugins`.
+    cache: PathBuf,
+    /// `$CLAUDE_CONFIG_DIR`, else `~/.claude`.
+    claude_config: PathBuf,
+    /// Tests serve manifests over plain http on loopback.
+    allow_http: bool,
+}
+
+/// What the installs did, by (manifest hash, key); a key is a plugin id,
+/// or `marketplace:<name>` for a marketplace that could not be added.
+#[derive(Default)]
+struct Memory {
+    installed: HashSet<(String, String)>,
+    manual: HashMap<(String, String), String>,
+    failed: HashMap<(String, String), (String, Instant)>,
+    /// Plugin installs queued or running.
+    busy: HashSet<(String, String)>,
+    /// Per manifest hash, the installs left; present while its task runs.
+    queues: HashMap<String, VecDeque<Job>>,
+}
+
+impl Memory {
+    /// The failure of `key` under `sha`, while it is recent.
+    fn failure(&mut self, sha: &str, key: &str, now: Instant) -> Option<String> {
+        let k = (sha.to_owned(), key.to_owned());
+        match self.failed.get(&k) {
+            Some((why, at)) if now.saturating_duration_since(*at) < RETRY_AFTER => {
+                Some(why.clone())
+            }
+            Some(_) => {
+                self.failed.remove(&k);
+                None
+            }
+            None => None,
+        }
+    }
+}
+
+/// One plugin to install, with its marketplace.
+#[derive(Clone)]
+struct Job {
+    plugin: PluginRef,
+    marketplace: Marketplace,
+}
+
+/// How one `claude plugin …` run ended.
+#[derive(Debug, PartialEq)]
+enum Outcome {
+    Done,
+    /// A marketplace-declared command needs a person.
+    Manual,
+    Failed(String),
+}
+
+impl Manifests {
+    /// The standard places: [`paths::cache_dir`] and the Claude Code config
+    /// dir the sessions see (this daemon's `CLAUDE_CONFIG_DIR`, which every
+    /// session inherits).
+    pub(crate) fn new() -> Manifests {
+        let claude_config = std::env::var_os("CLAUDE_CONFIG_DIR")
+            .filter(|v| !v.is_empty())
+            .map(PathBuf::from)
+            .unwrap_or_else(|| paths::home().join(".claude"));
+        Manifests {
+            dirs: Mutex::new(Dirs {
+                cache: paths::cache_dir().join("plugins"),
+                claude_config,
+                allow_http: false,
+            }),
+            locks: Mutex::new(HashMap::new()),
+            memory: Mutex::new(Memory::default()),
+            skew: Mutex::new(Duration::ZERO),
+        }
+    }
+
+    fn dirs(&self) -> Dirs {
+        self.dirs.lock().unwrap_or_else(|e| e.into_inner()).clone()
+    }
+
+    fn memory(&self) -> MutexGuard<'_, Memory> {
+        self.memory.lock().unwrap_or_else(|e| e.into_inner())
+    }
+
+    fn now(&self) -> Instant {
+        Instant::now() + *self.skew.lock().unwrap_or_else(|e| e.into_inner())
+    }
+
+    /// Use `cache` and `claude_config` instead, and accept http URLs.
+    #[cfg(test)]
+    pub(crate) fn set_dirs(&self, cache: PathBuf, claude_config: PathBuf) {
+        *self.dirs.lock().unwrap_or_else(|e| e.into_inner()) = Dirs {
+            cache,
+            claude_config,
+            allow_http: true,
+        };
+    }
+
+    /// Whether an install is queued or running.
+    #[cfg(test)]
+    pub(crate) fn busy(&self) -> bool {
+        !self.memory().queues.is_empty()
+    }
+
+    /// Move this daemon's clock forward by `by`.
+    #[cfg(test)]
+    pub(crate) fn advance(&self, by: Duration) {
+        *self.skew.lock().unwrap_or_else(|e| e.into_inner()) += by;
+    }
+
+    fn lock(&self, name: &str) -> Arc<tokio::sync::Mutex<()>> {
+        let mut locks = self.locks.lock().unwrap_or_else(|e| e.into_inner());
+        Arc::clone(locks.entry(name.to_owned()).or_default())
+    }
+
+    /// The reply for `m` (hash `sha`) and the installs it needs, honouring
+    /// `disabled` (neither applied nor listed) and the request's `secrets`.
+    fn decide(
+        &self,
+        m: &Manifest,
+        sha: &str,
+        secrets: &[String],
+        disabled: &[String],
+        files: &Files,
+    ) -> (Applied, Vec<Job>) {
+        let on = |id: &str| !disabled.iter().any(|d| d == id);
+        let now = self.now();
+        let mut mem = self.memory();
+        let mut applied = Applied {
+            name: m.name.clone(),
+            version: m.version.clone(),
+            ..Applied::default()
+        };
+        let mut jobs = Vec::new();
+        for p in m.plugins.iter().filter(|p| on(&p.id())) {
+            let id = p.id();
+            let key = (sha.to_owned(), id.clone());
+            if files.installed.contains(&p.spec()) {
+                if mem.installed.contains(&key) {
+                    applied.installed.push(id);
+                } else {
+                    applied.present.push(id);
+                }
+                continue;
+            }
+            if let Some(why) = mem.manual.get(&key) {
+                applied.skipped.push((id, why.clone()));
+                continue;
+            }
+            if let Some(why) = mem.failure(sha, &id, now) {
+                applied.skipped.push((id, why));
+                continue;
+            }
+            applied.skipped.push((id, INSTALLING.to_owned()));
+            if mem.busy.insert(key) {
+                if let Some(marketplace) = m.marketplace(&p.marketplace) {
+                    jobs.push(Job {
+                        plugin: p.clone(),
+                        marketplace: marketplace.clone(),
+                    });
+                }
+            }
+        }
+        let mut servers = Vec::new();
+        for s in m.servers.iter().filter(|s| on(&s.id())) {
+            let reason = match &files.servers {
+                None => Some("cannot read .claude.json".to_owned()),
+                Some(theirs) if theirs.contains(&s.name) => Some("yours".to_owned()),
+                Some(_) => s
+                    .secrets
+                    .iter()
+                    .find(|k| !secrets.contains(k))
+                    .map(|k| format!("missing secret {k}")),
+            };
+            match reason {
+                Some(why) => applied.skipped.push((s.id(), why)),
+                None => servers.push(s),
+            }
+        }
+        applied.mcp_config = manifest::mcp_json(servers);
+        let mut agents = Vec::new();
+        for a in m.agents.iter().filter(|a| on(&a.id())) {
+            if files.agents.contains(&a.name) {
+                applied.skipped.push((a.id(), "yours".to_owned()));
+            } else {
+                agents.push(a);
+            }
+        }
+        applied.agents = manifest::agents_json(agents);
+        if on("instructions") {
+            applied.instructions = sanitize(&m.instructions).trim().to_owned();
+        }
+        if on("env") {
+            applied.env = m.env.clone();
+        }
+        (applied, jobs)
+    }
+}
+
+/// Answer a `Manifest` request: `Applied`, or `Error "manifest <name>: …"`.
+/// The installs it needs are queued after the reply is made.
+pub(crate) async fn handle(
+    daemon: &Arc<Daemon>,
+    name: String,
+    url: String,
+    sha256: String,
+    secrets: Vec<String>,
+    disabled: Vec<String>,
+) -> Msg {
+    let manifests = &daemon.manifests;
+    let dirs = manifests.dirs();
+    let scheme_ok = url.starts_with("https://")
+        || (dirs.allow_http && url.starts_with("http://"))
+        || test_http_ok(&url);
+    let request_ok = name_ok(&name)
+        && sha256_ok(&sha256)
+        && scheme_ok
+        && secrets.len() <= MAX_SECRETS
+        && secrets.iter().all(|s| key_ok(s).is_ok())
+        && disabled.len() <= MAX_DISABLED
+        && disabled.iter().all(|id| manifest::id_ok(id));
+    if !request_ok {
+        return Msg::Error {
+            message: format!("manifest {}: bad request", safe_name(&name)),
+        };
+    }
+    let text = {
+        // One download per name at a time; whoever waited finds it cached.
+        let lock = manifests.lock(&name);
+        let _held = lock.lock().await;
+        ready(&dirs, &name, &url, &sha256).await
+    };
+    let parsed = text.and_then(|text| Manifest::parse_named(&text, &name));
+    let m = match parsed {
+        Ok(m) => m,
+        Err(why) => {
+            tracing::warn!(manifest = %name, error = %why, "manifest refused");
+            return Msg::Error {
+                message: format!("manifest {name}: {why}"),
+            };
+        }
+    };
+    let (files, flags) = {
+        let config = dirs.claude_config.clone();
+        let agents: Vec<String> = m.agents.iter().map(|a| a.name.clone()).collect();
+        let d = Arc::clone(daemon);
+        let read = tokio::task::spawn_blocking(move || Files::read(&config, &agents));
+        let probe = tokio::task::spawn_blocking(move || d.claude_flags());
+        let (files, flags) = tokio::join!(read, probe);
+        (files.unwrap_or_default(), flags.unwrap_or_default())
+    };
+    let (mut applied, jobs) = manifests.decide(&m, &sha256, &secrets, &disabled, &files);
+    applied.dropped = flags.dropped();
+    tracing::info!(
+        manifest = %name,
+        version = %m.version,
+        installed = applied.installed.len(),
+        present = applied.present.len(),
+        skipped = applied.skipped.len(),
+        queued = jobs.len(),
+        "manifest applied"
+    );
+    enqueue(daemon, &sha256, jobs);
+    Msg::Applied(applied)
+}
+
+/// A name for an error about a refused request: shown only when valid.
+fn safe_name(name: &str) -> &str {
+    if name_ok(name) {
+        name
+    } else {
+        "?"
+    }
+}
+
+/// Remove the zips (and their sidecars) a pre-manifest build left in
+/// `dir`, the plugin cache. Run once at daemon start.
+pub(crate) fn remove_stale_zips(dir: &Path) {
+    let Ok(entries) = std::fs::read_dir(dir) else {
+        return;
+    };
+    for entry in entries.flatten() {
+        let file = entry.file_name().to_string_lossy().into_owned();
+        let Some(stem) = file.strip_suffix(".zip") else {
+            continue;
+        };
+        let _ = std::fs::remove_file(entry.path());
+        if !dir.join(format!("{stem}.toml")).exists() {
+            let _ = std::fs::remove_file(dir.join(format!("{stem}.sha256")));
+        }
+    }
+}
+
+// ── Claude Code's files ───────────────────────────────────────────────────────
+
+/// What Claude Code's files say, read once per request.
+#[derive(Default)]
+struct Files {
+    /// The keys of `plugins/installed_plugins.json` (any scope); none when
+    /// it is unreadable.
+    installed: HashSet<String>,
+    /// The `mcpServers` of `.claude.json`; `None` when it is unreadable.
+    servers: Option<HashSet<String>>,
+    /// The manifest's agents the member has their own `agents/<name>.md` of.
+    agents: HashSet<String>,
+}
+
+impl Files {
+    /// Blocking: reads under `config` (the Claude Code config dir).
+    fn read(config: &Path, agents: &[String]) -> Files {
+        let installed = read_json(&config.join("plugins/installed_plugins.json"))
+            .ok()
+            .flatten()
+            .map(|v| keys(&v, "plugins"))
+            .unwrap_or_default();
+        // No file at all: the member has no servers of their own.
+        let servers = match read_json(&config.join(".claude.json")) {
+            Ok(Some(v)) => Some(keys(&v, "mcpServers")),
+            Ok(None) => Some(HashSet::new()),
+            Err(()) => None,
+        };
+        let agents = agents
+            .iter()
+            .filter(|a| std::fs::symlink_metadata(config.join(format!("agents/{a}.md"))).is_ok())
+            .cloned()
+            .collect();
+        Files {
+            installed,
+            servers,
+            agents,
+        }
+    }
+}
+
+/// The keys of object `field` of `v`.
+fn keys(v: &Value, field: &str) -> HashSet<String> {
+    v.get(field)
+        .and_then(Value::as_object)
+        .map(|o| o.keys().cloned().collect())
+        .unwrap_or_default()
+}
+
+/// The marketplaces Claude Code knows: the keys of
+/// `plugins/known_marketplaces.json` (none when it is unreadable).
+fn known_marketplaces(config: &Path) -> HashSet<String> {
+    let v = read_json(&config.join("plugins/known_marketplaces.json"));
+    v.ok()
+        .flatten()
+        .and_then(|v| v.as_object().map(|o| o.keys().cloned().collect()))
+        .unwrap_or_default()
+}
+
+/// The JSON in `path`: `Ok(None)` when there is no such file, `Err` when
+/// it is a symlink, not a regular file of this uid, over [`MAX_CONFIG`],
+/// unreadable or not JSON. Its content is never logged.
+fn read_json(path: &Path) -> Result<Option<Value>, ()> {
+    let meta = match std::fs::symlink_metadata(path) {
+        Ok(meta) => meta,
+        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
+        Err(_) => return Err(()),
+    };
+    if !meta.file_type().is_file() || meta.uid() != paths::uid() || meta.len() > MAX_CONFIG {
+        return Err(());
+    }
+    let file = std::fs::OpenOptions::new()
+        .read(true)
+        .custom_flags(libc::O_NOFOLLOW)
+        .open(path)
+        .map_err(|_| ())?;
+    let mut raw = Vec::new();
+    file.take(MAX_CONFIG + 1)
+        .read_to_end(&mut raw)
+        .map_err(|_| ())?;
+    if raw.len() as u64 > MAX_CONFIG {
+        return Err(());
+    }
+    serde_json::from_slice(&raw).map(Some).map_err(|_| ())
+}
+
+// ── Installs ──────────────────────────────────────────────────────────────────
+
+/// Queue `jobs` for manifest hash `sha`, starting its task unless it runs.
+fn enqueue(daemon: &Arc<Daemon>, sha: &str, jobs: Vec<Job>) {
+    if jobs.is_empty() {
+        return;
+    }
+    let start = {
+        let mut mem = daemon.manifests.memory();
+        let start = !mem.queues.contains_key(sha);
+        mem.queues.entry(sha.to_owned()).or_default().extend(jobs);
+        start
+    };
+    if start {
+        let daemon = Arc::clone(daemon);
+        let sha = sha.to_owned();
+        tokio::spawn(async move { run_queue(&daemon, &sha).await });
+    }
+}
+
+/// Run the installs of manifest hash `sha` one by one until none is left.
+async fn run_queue(daemon: &Daemon, sha: &str) {
+    let manifests = &daemon.manifests;
+    loop {
+        let job = {
+            let mut mem = manifests.memory();
+            match mem.queues.get_mut(sha).and_then(VecDeque::pop_front) {
+                Some(job) => job,
+                None => {
+                    mem.queues.remove(sha);
+                    return;
+                }
+            }
+        };
+        let id = job.plugin.id();
+        let outcome = install(daemon, sha, &job).await;
+        tracing::info!(plugin = %id, outcome = ?outcome, "plugin install");
+        let now = manifests.now();
+        let key = (sha.to_owned(), id);
+        let mut mem = manifests.memory();
+        mem.busy.remove(&key);
+        match outcome {
+            Outcome::Done => {
+                mem.installed.insert(key);
+            }
+            Outcome::Manual => {
+                let why = format!(
+                    "needs manual install: claude plugin install {}",
+                    job.plugin.spec()
+                );
+                mem.manual.insert(key, why);
+            }
+            Outcome::Failed(why) => {
+                mem.failed.insert(key, (why, now));
+            }
+        }
+    }
+}
+
+/// Install `job`'s plugin, adding its marketplace first when Claude Code
+/// does not know it.
+async fn install(daemon: &Daemon, sha: &str, job: &Job) -> Outcome {
+    let manifests = &daemon.manifests;
+    let config = manifests.dirs().claude_config;
+    let market = &job.marketplace.name;
+    let market_key = format!("marketplace:{market}");
+    let remembered = manifests
+        .memory()
+        .failure(sha, &market_key, manifests.now());
+    if let Some(why) = remembered {
+        return Outcome::Failed(why);
+    }
+    if !known_marketplaces(&config).contains(market) {
+        let source = job.marketplace.cli_source();
+        let added = run_claude(
+            daemon,
+            &[
+                "plugin",
+                "marketplace",
+                "add",
+                source,
+                "--scope",
+                "user",
+                "--json",
+            ],
+        )
+        .await;
+        let failed = match added {
+            Outcome::Done if known_marketplaces(&config).contains(market) => None,
+            Outcome::Done => Some("added under another name".to_owned()),
+            Outcome::Manual => Some("needs a person to add it".to_owned()),
+            Outcome::Failed(why) => Some(why),
+        };
+        if let Some(why) = failed {
+            let why = format!("marketplace {market}: {why}");
+            let key = (sha.to_owned(), market_key);
+            manifests
+                .memory()
+                .failed
+                .insert(key, (why.clone(), manifests.now()));
+            return Outcome::Failed(why);
+        }
+    }
+    let spec = job.plugin.spec();
+    run_claude(
+        daemon,
+        &["plugin", "install", &spec, "--scope", "user", "--json"],
+    )
+    .await
+}
+
+/// Run `claude <args>` while holding `Daemon::updating`: the daemon's
+/// claude on its `PATH`, stdin `/dev/null`, the session markers scrubbed,
+/// git never prompting, in a process group of its own killed on timeout.
+async fn run_claude(daemon: &Daemon, args: &[&str]) -> Outcome {
+    let _held = daemon.updating.lock().await;
+    let mut cmd = Command::new(daemon.config.claude_bin());
+    cmd.args(args);
+    for marker in SESSION_MARKERS {
+        cmd.env_remove(marker);
+    }
+    if let Some(path) = daemon.config.path.as_deref() {
+        cmd.env("PATH", path);
+    }
+    cmd.env("GIT_TERMINAL_PROMPT", "0")
+        .env("GIT_ASKPASS", "/bin/true");
+    let spawned = cmd
+        .stdin(Stdio::null())
+        .stdout(Stdio::piped())
+        .stderr(Stdio::piped())
+        .process_group(0)
+        .kill_on_drop(true)
+        .spawn();
+    let mut child = match spawned {
+        Ok(child) => child,
+        Err(e) => return Outcome::Failed(format!("could not start claude: {e}")),
+    };
+    let (out, err) = (child.stdout.take(), child.stderr.take());
+    let finished = tokio::time::timeout(RUN_TIMEOUT, async {
+        let (out, err) = tokio::join!(read_capped(out), read_capped(err));
+        (child.wait().await, out, err)
+    })
+    .await;
+    match finished {
+        Ok((Ok(status), out, err)) => outcome(status.success(), status.code(), &out, &err),
+        Ok((Err(e), _, _)) => Outcome::Failed(format!("claude failed: {e}")),
+        Err(_) => {
+            super::update::kill_group(&mut child).await;
+            Outcome::Failed(format!("timed out after {} s", RUN_TIMEOUT.as_secs()))
+        }
+    }
+}
+
+/// Everything `stream` gives, keeping the first [`OUTPUT_CAP`] bytes.
+async fn read_capped(stream: Option<impl AsyncRead + Unpin>) -> Vec<u8> {
+    let mut kept = Vec::new();
+    let Some(mut stream) = stream else {
+        return kept;
+    };
+    let mut buf = [0u8; 4096];
+    loop {
+        match stream.read(&mut buf).await {
+            Ok(0) | Err(_) => return kept,
+            Ok(n) => {
+                let room = OUTPUT_CAP.saturating_sub(kept.len());
+                kept.extend_from_slice(&buf[..n.min(room)]);
+            }
+        }
+    }
+}
+
+/// What a `--json` run said: its last stdout line as JSON. A
+/// `shownCommand` needs a person; an `error`, a non-zero exit or no JSON
+/// is a failure with the CLI's message (else stderr's first line).
+fn outcome(success: bool, code: Option<i32>, stdout: &[u8], stderr: &[u8]) -> Outcome {
+    let stdout = String::from_utf8_lossy(stdout);
+    let json: Option<Value> = stdout
+        .lines()
+        .rev()
+        .find(|l| !l.trim().is_empty())
+        .and_then(|l| serde_json::from_str(l.trim()).ok());
+    let field = |name: &str| {
+        let v = json.as_ref()?.get(name)?;
+        match v {
+            Value::Null => None,
+            Value::String(s) => Some(s.clone()),
+            other => other
+                .get("message")
+                .and_then(Value::as_str)
+                .map(str::to_owned)
+                .or_else(|| Some(other.to_string())),
+        }
+    };
+    if json
+        .as_ref()
+        .and_then(|v| v.get("shownCommand"))
+        .is_some_and(|c| !c.is_null())
+    {
+        return Outcome::Manual;
+    }
+    if let Some(error) = field("error") {
+        return Outcome::Failed(short(&error));
+    }
+    if success {
+        return Outcome::Done;
+    }
+    let stderr = String::from_utf8_lossy(stderr);
+    let message = field("message")
+        .or_else(|| {
+            stderr
+                .lines()
+                .map(str::trim)
+                .find(|l| !l.is_empty())
+                .map(str::to_owned)
+        })
+        .unwrap_or_else(|| match code {
+            Some(code) => format!("exit status {code}"),
+            None => "killed".to_owned(),
+        });
+    Outcome::Failed(short(&message))
+}
+
+/// `text` on one line, sanitized, at most [`REASON_CHARS`] characters.
+fn short(text: &str) -> String {
+    let line = sanitize(text).replace(['\n', '\t', '\r'], " ");
+    line.trim().chars().take(REASON_CHARS).collect()
+}
+
+// ── Download ──────────────────────────────────────────────────────────────────
+
+/// The text of manifest `name` with hash `sha256`: cached, else downloaded.
+async fn ready(dirs: &Dirs, name: &str, url: &str, sha256: &str) -> Result<String, String> {
+    let dir = dirs.cache.clone();
+    paths::ensure_private_dir(&dir).map_err(|e| format!("cache dir: {e}"))?;
+    let file = dir.join(format!("{name}.toml"));
+    let sidecar = dir.join(format!("{name}.sha256"));
+    let cached =
+        file.is_file() && std::fs::read_to_string(&sidecar).is_ok_and(|s| s.trim() == sha256);
+    if !cached {
+        if !file.exists() && count(&dir, ".toml") >= MAX_MANIFESTS {
+            return Err(format!("cache holds {MAX_MANIFESTS} manifests already"));
+        }
+        let tmp = dir.join(format!(".{name}.{}.part", uuid::Uuid::new_v4().simple()));
+        let allow_http = dirs.allow_http || test_http_ok(url);
+        let result = download(url, &tmp, sha256, allow_http, MAX_BYTES).await;
+        let placed = result.and_then(|()| place(&tmp, &file, &sidecar, sha256));
+        if placed.is_err() {
+            let _ = std::fs::remove_file(&tmp);
+        }
+        placed?;
+    }
+    let mut raw = Vec::new();
+    std::fs::File::open(&file)
+        .and_then(|f| f.take(MAX_BYTES + 1).read_to_end(&mut raw))
+        .map_err(|e| format!("cannot read the manifest: {e}"))?;
+    String::from_utf8(raw).map_err(|_| "not UTF-8".to_owned())
+}
+
+fn count(dir: &Path, suffix: &str) -> usize {
+    std::fs::read_dir(dir)
+        .map(|entries| {
+            entries
+                .flatten()
+                .filter(|e| e.file_name().to_string_lossy().ends_with(suffix))
+                .count()
+        })
+        .unwrap_or(0)
+}
+
+/// Move a verified download into place. The old sidecar goes first, so no
+/// moment pairs a hash with a file it does not describe; a crash in between
+/// leaves no sidecar, and the next request downloads again.
+fn place(tmp: &Path, file: &Path, sidecar: &Path, sha256: &str) -> Result<(), String> {
+    match std::fs::remove_file(sidecar) {
+        Ok(()) => {}
+        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
+        Err(e) => return Err(format!("cannot replace the sidecar: {e}")),
+    }
+    std::fs::rename(tmp, file).map_err(|e| format!("cannot store the manifest: {e}"))?;
+    paths::write_atomic(sidecar, format!("{sha256}\n").as_bytes())
+        .map_err(|e| format!("cannot write the sidecar: {e}"))
+}
+
+/// Stream `url` into a new file `tmp` (0600), checking the size cap `cap`
+/// and, at the end, the SHA-256. Redirects are followed to https URLs only.
+async fn download(
+    url: &str,
+    tmp: &Path,
+    sha256: &str,
+    allow_http: bool,
+    cap: u64,
+) -> Result<(), String> {
+    let policy = reqwest::redirect::Policy::custom(move |attempt| {
+        let https = attempt.url().scheme() == "https";
+        if attempt.previous().len() >= 5 {
+            attempt.error("too many redirects")
+        } else if https || allow_http {
+            attempt.follow()
+        } else {
+            attempt.error("redirect to a non-https URL")
+        }
+    });
+    let client = reqwest::Client::builder()
+        .connect_timeout(CONNECT_TIMEOUT)
+        .timeout(TOTAL_TIMEOUT)
+        .redirect(policy)
+        .build()
+        .map_err(|e| format!("http client: {e}"))?;
+    let mut resp = client
+        .get(url)
+        .send()
+        .await
+        .map_err(|e| format!("download failed: {}", e.without_url()))?;
+    if !resp.status().is_success() {
+        return Err(format!("download failed: HTTP {}", resp.status().as_u16()));
+    }
+    let over = || format!("over {} KiB", cap >> 10);
+    if resp.content_length().is_some_and(|n| n > cap) {
+        return Err(over());
+    }
+    let mut file = std::fs::OpenOptions::new()
+        .write(true)
+        .create_new(true)
+        .mode(0o600)
+        .custom_flags(libc::O_NOFOLLOW)
+        .open(tmp)
+        .map_err(|e| format!("cannot create a temp file: {e}"))?;
+    let mut hasher = Sha256::new();
+    let mut total: u64 = 0;
+    while let Some(chunk) = resp
+        .chunk()
+        .await
+        .map_err(|e| format!("download failed: {}", e.without_url()))?
+    {
+        total += chunk.len() as u64;
+        if total > cap {
+            return Err(over());
+        }
+        hasher.update(&chunk);
+        file.write_all(&chunk)
+            .map_err(|e| format!("cannot write the manifest: {e}"))?;
+    }
+    file.sync_all()
+        .map_err(|e| format!("cannot write the manifest: {e}"))?;
+    let got = format!("{:x}", hasher.finalize());
+    if got != sha256 {
+        return Err("hash mismatch".to_owned());
+    }
+    Ok(())
+}
+
 #[cfg(test)]
 mod tests {
     use super::*;
diff --git a/src/daemon/mod.rs b/src/daemon/mod.rs
index a43c195..5970f9c 100644
--- a/src/daemon/mod.rs
+++ b/src/daemon/mod.rs
@@ -305,7 +305,14 @@ pub fn run() -> std::process::ExitCode {
     };
     let (code, lock) = runtime.block_on(async move {
         match server::start(config) {
-            Ok(Some(listening)) => (std::process::ExitCode::SUCCESS, Some(listening.serve().await)),
+            Ok(Some(listening)) => {
+                // Zips a pre-manifest build cached: nothing uses them now.
+                manifest::remove_stale_zips(&crate::paths::cache_dir().join("plugins"));
+                (
+                    std::process::ExitCode::SUCCESS,
+                    Some(listening.serve().await),
+                )
+            }
             Ok(None) => {
                 tracing::info!("another daemon is already running");
                 (std::process::ExitCode::SUCCESS, None)
@@ -339,7 +346,8 @@ pub(crate) struct Daemon {
     /// Probed on first use (it runs `claude --version`), and again after a
     /// claude update.
     host: tokio::sync::Mutex<Option<HostInfo>>,
-    /// Held while `claude update` runs: one at a time.
+    /// Held while `claude update` or one `claude plugin …` runs: one at a
+    /// time, so `~/.claude` has one writer.
     updating: tokio::sync::Mutex<()>,
     /// Which optional flags this claude knows (`--allow-dangerously-skip-
     /// permissions`, `--plugin-dir`, `--agents`, `--mcp-config`; it runs
@@ -352,8 +360,9 @@ pub(crate) struct Daemon {
     pub(crate) stats_rx: tokio::sync::watch::Receiver<stats::HostSnapshot>,
     /// `Search`: one at a time, and the transcripts it reads.
     pub(crate) search: server::SearchSlot,
-    /// `Bundle`: where plugin zips go, one download per name at a time.
-    pub(crate) bundles: bundle::Bundles,
+    /// `Manifest`: where manifests go, one download per name at a time,
+    /// and what the plugin installs did.
+    pub(crate) manifests: manifest::Manifests,
 }
 
 /// Journaled sessions, and the live subset with a running process.
@@ -395,7 +404,7 @@ impl Daemon {
             shutdown: tokio::sync::Notify::new(),
             stats_rx,
             search: server::SearchSlot::new(),
-            bundles: bundle::Bundles::new(),
+            manifests: manifest::Manifests::new(),
         }
     }
 
diff --git a/src/daemon/server.rs b/src/daemon/server.rs
index 406abb5..2fe4382 100644
--- a/src/daemon/server.rs
+++ b/src/daemon/server.rs
@@ -26,7 +26,7 @@ use tokio::sync::broadcast::error::{RecvError, TryRecvError};
 use tokio::sync::{broadcast, mpsc, Semaphore};
 
 use super::session::{ClientId, Cmd};
-use super::{bundle, git, host, update, Config, Daemon};
+use super::{git, host, manifest, update, Config, Daemon};
 use crate::claude::projects;
 use crate::claude::search::{self, BadQuery, Budget};
 use crate::paths;
@@ -558,19 +558,19 @@ impl Client {
                 Ok(()) => return,
                 Err(refused) => refused,
             },
-            Msg::Bundle {
+            Msg::Manifest {
                 name,
                 url,
                 sha256,
-                installed_id,
+                secrets,
+                disabled,
             } => {
                 // A download (up to minutes): reply from a task, like
-                // `UpdateClaude`.
+                // `UpdateClaude`. The installs run after the reply.
                 let daemon = Arc::clone(&self.daemon);
                 let queue = self.queue.clone();
                 tokio::spawn(async move {
-                    let msg =
-                        bundle::handle(&daemon.bundles, name, url, sha256, installed_id).await;
+                    let msg = manifest::handle(&daemon, name, url, sha256, secrets, disabled).await;
                     let _ = queue.send(Frame::Control(Envelope { req, msg })).await;
                 });
                 return;
diff --git a/src/daemon/update.rs b/src/daemon/update.rs
index 18df1c8..e5ff367 100644
--- a/src/daemon/update.rs
+++ b/src/daemon/update.rs
@@ -32,11 +32,12 @@ const TAIL_BYTES: usize = 4096;
 const INSTALLER: &str = "{ curl -fsSL https://claude.ai/install.sh | bash; } 2>&1";
 
 /// Run the update (or the install) and answer `ClaudeUpdated`. Only one runs
-/// at a time per daemon.
+/// at a time per daemon, and never beside a plugin install (which waits
+/// for the same lock).
 pub async fn run(daemon: &Daemon, install: bool) -> Msg {
     let Ok(_running) = daemon.updating.try_lock() else {
         return Msg::Error {
-            message: "a claude update is already running on this host".into(),
+            message: "a claude update or plugin install is already running on this host".into(),
         };
     };
     let claude = daemon.config.claude_bin();
@@ -117,7 +118,7 @@ async fn execute(
 
 /// Stop the update's whole process group: SIGTERM, a grace period for the
 /// leader to exit, then SIGKILL for whatever is left. Reaps the leader.
-async fn kill_group(child: &mut Child) {
+pub(super) async fn kill_group(child: &mut Child) {
     // The leader's pid is the group id (`process_group(0)`); `None` once
     // it has been reaped, and then its pid may already be reused.
     let Some(pgid) = child.id().and_then(|pid| i32::try_from(pid).ok()) else {
diff --git a/src/proxy/plugins.rs b/src/proxy/plugins.rs
index 94d77c5..0a5f486 100644
--- a/src/proxy/plugins.rs
+++ b/src/proxy/plugins.rs
@@ -184,6 +184,16 @@ pub fn key_ok(key: &str) -> Result<(), String> {
     Ok(())
 }
 
+/// Whether `url` is a plain-http loopback URL a test lets through:
+/// `http://127.0.0.1:<port>/…` in a debug build run with
+/// `CLAUDIO_TEST_LOOPBACK_HTTP=1` (the manager tests' fake proxy). Always
+/// false in a release build.
+pub fn test_http_ok(url: &str) -> bool {
+    cfg!(debug_assertions)
+        && url.starts_with("http://127.0.0.1:")
+        && std::env::var_os("CLAUDIO_TEST_LOOPBACK_HTTP").is_some_and(|v| v == "1")
+}
+
 /// `''` or `[a-z0-9-]{1,40}@[a-z0-9-]{1,40}`.
 fn installed_id_ok(id: &str) -> bool {
     id.is_empty()
`````
Run: `umask 022; cargo test --locked --bin claudio; cargo build --features diag && env -u CLAUDIO_SSH_TEST_HOST cargo test --features diag --test remote -- --test-threads=1`
Expected: `test result: ok. 1073 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`, then `test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`. Dead-code warnings remain for `manifest::{Section, Item, items, ids, heading}` until Task 7.

Step 3: commit
`/usr/bin/git add src/daemon/manifest.rs && /usr/bin/git commit -m "feat(daemon): apply plugin manifests and install their plugins" -- src/daemon/manifest.rs src/daemon/bundle.rs src/daemon/mod.rs src/daemon/server.rs src/daemon/update.rs src/daemon/tests.rs src/proxy/plugins.rs`

Interfaces this task provides to later tasks: `daemon::manifest::{Manifests, handle, remove_stale_zips, MAX_MANIFESTS, RETRY_AFTER, INSTALLING}`, `Daemon.manifests` (test-only `set_dirs`, `advance`, `busy`), `update::kill_group` is `pub(super)`, `crate::proxy::plugins::test_http_ok(url)`, the `Manifest` op answered by the daemon.

### Task 5: config: `[plugins.<name>]` manifest choices
Model: standard   Repo: /volumes/repos/claudio   Files: src/config.rs
Goal: `disabled`/`reviewed`/`seen` parsed, and written by a verified minimal table edit that replaces the table's lines or appends it, everything else untouched (AC3 part).
Patch: .delivery/2026-11-06-plugin-manifest/patches/task-5.patch

Step 1: write the failing test
`````diff
diff --git a/src/config.rs b/src/config.rs
index a7620cb..8313fbc 100644
--- a/src/config.rs
+++ b/src/config.rs
@@ -758,6 +912,97 @@ default = "x"
         );
     }
 
+    fn choices(disabled: &[&str], reviewed: &str, seen: &[&str]) -> PluginChoices {
+        let own = |l: &[&str]| l.iter().map(|s| s.to_string()).collect();
+        PluginChoices {
+            disabled: own(disabled),
+            reviewed: reviewed.into(),
+            seen: own(seen),
+        }
+    }
+
+    #[test]
+    fn plugin_choices_append_the_table_when_missing() {
+        let c = choices(&["mcp:jina"], "1.0.0", &["mcp:jina", "plugin:a@m"]);
+        assert_eq!(
+            set_plugin_choices("[ui]\nnotify = false", "team", &c),
+            "[ui]\nnotify = false\n\n[plugins.team]\ndisabled = [\"mcp:jina\"]\n\
+             reviewed = \"1.0.0\"\nseen = [\"mcp:jina\", \"plugin:a@m\"]\n"
+        );
+        let none = choices(&[], "2", &[]);
+        assert_eq!(
+            set_plugin_choices("", "team", &none),
+            "[plugins.team]\ndisabled = []\nreviewed = \"2\"\nseen = []\n"
+        );
+        // A version with quotes and backslashes is escaped.
+        let odd = choices(&[], "1.0 \"rc\" \\", &[]);
+        let out = set_plugin_choices("", "team", &odd);
+        let table: toml::Table = out.parse().unwrap();
+        assert_eq!(
+            table["plugins"]["team"]["reviewed"].as_str(),
+            Some("1.0 \"rc\" \\")
+        );
+    }
+
+    #[test]
+    fn plugin_choices_replace_the_table_and_leave_the_rest_untouched() {
+        let text = "# mine\r\n[plugins.team]\r\ndisabled = []\r\nreviewed = \"0.9\"\r\n\
+                    seen = [\r\n  \"mcp:kb\",\r\n]\r\n\r\n# next\r\n[plugins.other]\r\n\
+                    reviewed = \"1\"\r\n[ui]\r\nnotify = true\r\n";
+        let c = choices(&["mcp:kb"], "1.0", &["mcp:kb"]);
+        assert_eq!(
+            set_plugin_choices(text, "team", &c),
+            "# mine\r\n[plugins.team]\r\ndisabled = [\"mcp:kb\"]\r\nreviewed = \"1.0\"\r\n\
+             seen = [\"mcp:kb\"]\r\n\r\n# next\r\n[plugins.other]\r\nreviewed = \"1\"\r\n\
+             [ui]\r\nnotify = true\r\n"
+        );
+        // The last table of the file.
+        let text = "[ui]\nnotify = true\n\n[plugins.team]\nreviewed = \"1\"\n";
+        let out = set_plugin_choices(text, "team", &c);
+        assert_eq!(
+            out,
+            "[ui]\nnotify = true\n\n[plugins.team]\ndisabled = [\"mcp:kb\"]\n\
+             reviewed = \"1.0\"\nseen = [\"mcp:kb\"]\n"
+        );
+    }
+
+    #[test]
+    fn save_plugin_choices_writes_only_its_table() {
+        use std::os::unix::fs::PermissionsExt;
+        let text = "[proxy]\ndefault = \"work\"\n\n[extensions]\nkb = false\n";
+        let (dir, path) = scratch_config(Some(text));
+        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
+        let c = choices(&["env"], "1.0.0", &["env", "mcp:kb"]);
+        save_plugin_choices(&path, "team", &c).unwrap();
+        let saved = std::fs::read_to_string(&path).unwrap();
+        assert!(saved.starts_with(text), "{saved}");
+        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
+        assert_eq!(mode, 0o640);
+        let cfg = load_from(&path);
+        assert_eq!(cfg.plugins.get("team"), Some(&c));
+        assert_eq!(cfg.extensions.get("kb"), Some(&false));
+        // Saved again with other choices: replaced, not appended.
+        let c2 = choices(&[], "1.1.0", &["env", "mcp:kb", "agent:x"]);
+        save_plugin_choices(&path, "team", &c2).unwrap();
+        let saved = std::fs::read_to_string(&path).unwrap();
+        assert_eq!(saved.matches("[plugins.team]").count(), 1, "{saved}");
+        assert_eq!(load_from(&path).plugins.get("team"), Some(&c2));
+        // A layout the edit cannot handle is refused, and nothing is written.
+        let dotted = "plugins.team.reviewed = \"1\"\n";
+        std::fs::write(&path, dotted).unwrap();
+        let err = save_plugin_choices(&path, "team", &c).unwrap_err();
+        assert_eq!(
+            err.to_string(),
+            "config.toml layout not understood; edit [plugins.team] by hand"
+        );
+        assert_eq!(std::fs::read_to_string(&path).unwrap(), dotted);
+        assert!(matches!(
+            save_plugin_choices(&path, "Bad", &c),
+            Err(SaveError::BadName)
+        ));
+        let _ = std::fs::remove_dir_all(&dir);
+    }
+
     #[test]
     fn extensions_section_turns_extensions_on_and_off() {
         let dir = std::env::temp_dir().join(format!("claudio-cfg-test-{}", uuid::Uuid::new_v4()));
`````
Run: `umask 022; cargo test --locked --bin claudio config::`
Expected: `` error[E0425]: cannot find type `PluginChoices` in this scope ``, ending `` error: could not compile `claudio` (bin "claudio" test) due to 13 previous errors ``

Step 2: implement
`````diff
diff --git a/src/config.rs b/src/config.rs
index 21f0031..8313fbc 100644
--- a/src/config.rs
+++ b/src/config.rs
@@ -143,6 +143,27 @@ pub type KeysSection = BTreeMap<String, String>;
 /// (every extension is on unless listed as `false`).
 pub type ExtensionsSection = BTreeMap<String, bool>;
 
+/// `[plugins.<name>]`: what the member chose for proxy plugin `<name>`'s
+/// manifest. Written by [`save_plugin_choices`] when the setup screen
+/// closes with Enter.
+#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
+pub struct PluginChoices {
+    /// The item ids turned off (`mcp:jina`, `plugin:x@m`…), including ids
+    /// the manifest no longer has, so one that comes back stays off.
+    #[serde(default)]
+    pub disabled: Vec<String>,
+    /// The manifest version last reviewed.
+    #[serde(default)]
+    pub reviewed: String,
+    /// Every id of the reviewed version (and the disabled ids it lacks): an
+    /// id of a newer version not in here is new.
+    #[serde(default)]
+    pub seen: Vec<String>,
+}
+
+/// The `[plugins]` tables of `config.toml`, by plugin name.
+pub type PluginsSection = BTreeMap<String, PluginChoices>;
+
 /// The subset of `config.toml` that this module owns. Other sections (e.g.
 /// `[proxy]`) are preserved via `extra` on round-trip.
 #[derive(Debug, Default, Serialize, Deserialize)]
@@ -157,6 +178,8 @@ struct ConfigFile {
     pub claude: Option<ClaudeSection>,
     #[serde(default)]
     pub extensions: Option<ExtensionsSection>,
+    #[serde(default)]
+    pub plugins: Option<PluginsSection>,
     /// All other TOML keys — preserved on write.
     #[serde(flatten)]
     extra: toml::Table,
@@ -172,6 +195,7 @@ pub struct Config {
     pub update: UpdateSection,
     pub claude: ClaudeSection,
     pub extensions: ExtensionsSection,
+    pub plugins: PluginsSection,
 }
 
 // ── Loading ───────────────────────────────────────────────────────────────────
@@ -226,6 +250,7 @@ fn try_load_from(path: &Path) -> io::Result<Config> {
         update: file.update.unwrap_or_default(),
         claude: file.claude.unwrap_or_default(),
         extensions: file.extensions.unwrap_or_default(),
+        plugins: file.plugins.unwrap_or_default(),
     })
 }
 
@@ -239,7 +264,10 @@ pub enum SaveError {
     /// The file is valid TOML but laid out so the line edit cannot set the
     /// key (a dotted `extensions.x`, an inline table, `[[extensions]]`…).
     Unsupported(String),
-    /// The name is not an extension's (`^[a-z][a-z0-9-]{0,31}$`).
+    /// Like `Unsupported`, for table `[plugins.<name>]`.
+    UnsupportedPlugin(String),
+    /// The name is not an extension's (`^[a-z][a-z0-9-]{0,31}$`) or a
+    /// plugin's (`[a-z0-9-]{1,40}`).
     BadName,
     Io(io::Error),
 }
@@ -252,6 +280,10 @@ impl std::fmt::Display for SaveError {
                 f,
                 "config.toml layout not understood; set [extensions] {name} = … by hand"
             ),
+            SaveError::UnsupportedPlugin(name) => write!(
+                f,
+                "config.toml layout not understood; edit [plugins.{name}] by hand"
+            ),
             SaveError::BadName => write!(f, "not an extension name"),
             SaveError::Io(e) => write!(f, "{e}"),
         }
@@ -471,6 +503,128 @@ pub fn save_extension(path: &Path, name: &str, on: bool) -> Result<(), SaveError
     Ok(())
 }
 
+/// `[plugins.<name>]` with `choices`, as lines ending in `eol`.
+fn plugin_table(name: &str, choices: &PluginChoices, eol: &str) -> String {
+    let string = |s: &str| toml::Value::String(s.to_owned()).to_string();
+    let list = |items: &[String]| {
+        let items: Vec<String> = items.iter().map(|i| string(i)).collect();
+        format!("[{}]", items.join(", "))
+    };
+    format!(
+        "[plugins.{name}]{eol}disabled = {}{eol}reviewed = {}{eol}seen = {}{eol}",
+        list(&choices.disabled),
+        string(&choices.reviewed),
+        list(&choices.seen),
+    )
+}
+
+/// `text` (a `config.toml`) with table `[plugins.<name>]` set to
+/// `choices`, edited minimally: the table's header and key lines are
+/// replaced (the blank lines and comments after its last key stay), else
+/// the table is appended. Every other byte stays as it was; the line
+/// ending follows the file's. `name` must be a plugin name
+/// (`[a-z0-9-]{1,40}`, no quoting needed).
+pub fn set_plugin_choices(text: &str, name: &str, choices: &PluginChoices) -> String {
+    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
+    let table = plugin_table(name, choices, eol);
+    let wanted = format!("plugins.{name}");
+    let lines: Vec<&str> = text.split_inclusive('\n').collect();
+    let mut multiline: Option<&'static str> = None;
+    // The table's header line, and the line after its last key.
+    let mut found: Option<(usize, usize)> = None;
+    for (i, line) in lines.iter().enumerate() {
+        let (content, _) = split_eol(line);
+        let continued = multiline.is_some();
+        track_multiline(content, &mut multiline);
+        if let Some((_, end)) = &mut found {
+            if !continued && header(content).is_some() {
+                break;
+            }
+            let t = content.trim_start();
+            if continued || !(t.is_empty() || t.starts_with('#')) {
+                *end = i + 1;
+            }
+            continue;
+        }
+        if !continued && header(content).as_deref() == Some(wanted.as_str()) {
+            found = Some((i, i + 1));
+        }
+    }
+    match found {
+        Some((start, end)) => {
+            let mut out = lines[..start].concat();
+            out.push_str(&table);
+            out.push_str(&lines[end..].concat());
+            out
+        }
+        None => {
+            let mut out = text.to_owned();
+            if !out.is_empty() {
+                if !out.ends_with('\n') {
+                    out.push_str(eol);
+                }
+                out.push_str(eol);
+            }
+            out.push_str(&table);
+            out
+        }
+    }
+}
+
+/// Set `[plugins.<name>]` to `choices` in the config file at `path`
+/// (through a symlink, to its target) with [`set_plugin_choices`]; written
+/// atomically, keeping the file's mode (0600 when it is created). Nothing
+/// is written when the file does not parse as TOML, before or after the
+/// edit, or when anything but that table would change.
+pub fn save_plugin_choices(
+    path: &Path,
+    name: &str,
+    choices: &PluginChoices,
+) -> Result<(), SaveError> {
+    use std::os::unix::fs::PermissionsExt;
+    if !crate::proxy::plugins::name_ok(name) {
+        return Err(SaveError::BadName);
+    }
+    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
+    let (text, mode) = match std::fs::read(&path) {
+        Ok(bytes) => {
+            let text = String::from_utf8(bytes).map_err(|_| SaveError::InvalidToml)?;
+            let mode = std::fs::metadata(&path)?.permissions().mode() & 0o7777;
+            (text, Some(mode))
+        }
+        Err(e) if e.kind() == io::ErrorKind::NotFound => (String::new(), None),
+        Err(e) => return Err(e.into()),
+    };
+    let mut before: toml::Table = toml::from_str(&text).map_err(|_| SaveError::InvalidToml)?;
+    let edited = set_plugin_choices(&text, name, choices);
+    let unsupported = || SaveError::UnsupportedPlugin(name.to_owned());
+    let mut after: toml::Table = toml::from_str(&edited).map_err(|_| unsupported())?;
+    let set: Option<PluginChoices> = after
+        .get("plugins")
+        .and_then(|p| p.get(name))
+        .and_then(|t| t.clone().try_into().ok());
+    if set.as_ref() != Some(choices) {
+        return Err(unsupported());
+    }
+    // Only `plugins.<name>` may differ.
+    for t in [&mut before, &mut after] {
+        if let Some(toml::Value::Table(p)) = t.get_mut("plugins") {
+            p.remove(name);
+            if p.is_empty() {
+                t.remove("plugins");
+            }
+        }
+    }
+    if before != after {
+        return Err(unsupported());
+    }
+    paths::write_atomic(&path, edited.as_bytes())?;
+    if let Some(mode) = mode.filter(|m| *m != 0o600) {
+        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))?;
+    }
+    Ok(())
+}
+
 // ── Tests ─────────────────────────────────────────────────────────────────────
 
 #[cfg(test)]
`````
Run: `umask 022; cargo test --locked --bin claudio`
Expected: `test result: ok. 1076 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`. Dead-code warnings for the new config items until Task 7.

Step 3: commit
`/usr/bin/git commit -m "feat(config): [plugins.<name>] manifest choices with a minimal table edit" -- src/config.rs`

Interfaces this task provides to later tasks: `config::{PluginChoices { disabled, reviewed, seen }, PluginsSection, set_plugin_choices, save_plugin_choices, SaveError::UnsupportedPlugin}`, `Config.plugins`.

### Task 6: client: fetch manifests and spawn with what each host applied
Model: standard   Repo: /volumes/repos/claudio   Files: src/tui/plugins.rs, src/tui/app.rs, src/tui/interaction.rs, src/tui/sessions.rs, src/tui/mod.rs, src/tui/extensions_view.rs, src/proto.rs, src/proxy/plugins.rs, tests/manager.rs
Goal: the client side of §2, §4 and §5:
- It fetches each admitted plugin's manifest itself (`Effect::FetchManifest`: https, 256 KiB,
  hash-checked, memory only).
- It sends `Manifest` (secret names, disabled ids) per host on connect, on a list change, on
  `r` and every 30 s while a host still installs (≤ 10 min).
- `ApplyState` replaces `BundleState`, with `TooOld` for an old daemon.
- The extensions screen's details show each host's outcome.
- Spawns compose the plugins' share: flags after every other argument, ≤ 8 plugins, env layers,
  the instructions cut at 8 KiB.
- The 20 s deferral covers only hosts that have not answered. The reset sends the current env
  and instructions.

The zip-era manager tests go (D13).
Patch: .delivery/2026-11-06-plugin-manifest/patches/task-6.patch

Step 1: write the failing test
`````diff
diff --git a/src/proto.rs b/src/proto.rs
index bdeeeef..d8059a2 100644
--- a/src/proto.rs
+++ b/src/proto.rs
@@ -1390,7 +1401,8 @@ mod tests {
         });
         let merged = spawn
             .merge_env(vec![pair("API_TIMEOUT_MS", "30000")])
-            .with_plugin_dirs(&["/c/kb.zip".to_owned()]);
+            .with_plugin_args(&["--agents".to_owned(), "{}".to_owned()])
+            .with_instructions("Rules.".to_owned());
         let Msg::Spawn(spec) = merged else {
             panic!("a spawn")
         };
@@ -1398,8 +1410,10 @@ mod tests {
             spec.env,
             [pair("KB_TOKEN", "kb"), pair("API_TIMEOUT_MS", "30000")]
         );
-        assert_eq!(spec.args, ["--resume", "c1", "--plugin-dir", "/c/kb.zip"]);
+        assert_eq!(spec.args, ["--resume", "c1", "--agents", "{}"]);
+        assert_eq!(spec.extensions.unwrap().instructions, "Rules.");
         assert_eq!(Msg::Ping.merge_env(vec![pair("A", "b")]), Msg::Ping);
+        assert_eq!(Msg::Ping.with_instructions("x".into()), Msg::Ping);
     }
 
     #[test]
diff --git a/src/tui/extensions_view.rs b/src/tui/extensions_view.rs
index 614556e..bd0f942 100644
--- a/src/tui/extensions_view.rs
+++ b/src/tui/extensions_view.rs
@@ -547,38 +545,42 @@ mod tests {
             [
                 "1.4.0 · work",
                 "turning it on or off applies to new sessions",
-                "local: downloading",
+                "manifest: fetching",
             ]
         );
-        let ready = Msg::Bundled {
+        let src = "format = 1\nname = \"kb\"\nversion = \"1.4.0\"\ndescription = \"KB\"";
+        let manifest = crate::plugins::manifest::Manifest::parse(src).unwrap();
+        let sha = "a".repeat(64);
+        app.on_manifest("kb", &sha, Ok(manifest));
+        assert_eq!(details(&app, 5)[2..], ["KB", "local: applying"]);
+        let ready = Msg::Applied(crate::proto::Applied {
             name: "kb".into(),
-            path: Some("/c/kb.zip".into()),
-            installed: false,
+            version: "1.4.0".into(),
+            ..Default::default()
+        });
+        let to = ReplyTo::Manifest {
+            host: "local".into(),
+            name: "kb".into(),
+            sha256: sha,
         };
-        let sha = "a".repeat(64);
-        app.on_reply(
-            ReplyTo::Bundle {
-                host: "local".into(),
-                name: "kb".into(),
-                sha256: sha.clone(),
-            },
-            Ok(ready),
+        app.on_reply(to.clone(), Ok(ready));
+        assert_eq!(
+            details(&app, 5)[3],
+            "local: ready · 0 installed, 0 present, 0 installing, 0 skipped"
         );
-        assert_eq!(details(&app, 5)[2], "local: ready");
         app.on_reply(
-            ReplyTo::Bundle {
-                host: "local".into(),
-                name: "kb".into(),
-                sha256: sha,
-            },
+            to,
             Err(std::io::Error::other(
-                "bundle kb: download failed: HTTP 404",
+                "manifest kb: download failed: HTTP 404",
             )),
         );
-        assert_eq!(details(&app, 5)[2], "local: download failed: HTTP 404");
+        assert_eq!(
+            details(&app, 5)[3],
+            "local: failed: download failed: HTTP 404"
+        );
         assert!(
             app.notice.is_none(),
-            "a bundle error is shown here, not as a notice"
+            "a manifest error is shown here, not as a notice"
         );
         assert_eq!(
             details(&app, 6),
@@ -599,10 +601,10 @@ mod tests {
         }
         app.on_terminal(press(KeyCode::Char(' ')));
         assert_eq!(toggles(&mut app), [("save", "kb".to_owned(), false)]);
-        assert!(app.plugin_env("work").is_empty());
+        assert!(app.plugin_parts("local", "work").0.is_empty());
         assert_eq!(app.plugins.config.get("kb"), Some(&false));
-        // Back on: saved, and the hosts are asked for its bundle again only
-        // when they have no answer at its hash.
+        // Back on: saved, and the hosts are asked for its manifest again
+        // only when they have no answer at its hash (here: no manifest).
         app.on_terminal(press(KeyCode::Char(' ')));
         assert_eq!(toggles(&mut app), [("save", "kb".to_owned(), true)]);
         // A refused one does not toggle.
diff --git a/src/tui/plugins.rs b/src/tui/plugins.rs
index 536ed3f..5dd157d 100644
--- a/src/tui/plugins.rs
+++ b/src/tui/plugins.rs
@@ -329,59 +673,152 @@ mod tests {
         }
     }
 
-    /// The `Bundle` requests queued, as (host, name).
-    fn bundle_requests(app: &mut App) -> Vec<(String, String)> {
+    /// The manifest plugin `name` serves in these tests: a plugin, a
+    /// server needing `KB_TOKEN`, instructions and env.
+    fn manifest_of(name: &str) -> Manifest {
+        let src = format!(
+            r#"
+format = 1
+name = "{name}"
+version = "1.4.0"
+description = "The knowledge base"
+instructions = "Search the KB."
+
+[[marketplace]]
+name = "m"
+source = "github:o/r"
+
+[[plugin]]
+name = "p"
+marketplace = "m"
+
+[[mcp]]
+name = "kb"
+type = "http"
+url = "https://kb.example.org/mcp"
+headers = {{ Authorization = "Bearer ${{KB_TOKEN}}" }}
+secrets = ["KB_TOKEN"]
+
+[env]
+KB_REGION = "eu"
+"#
+        );
+        Manifest::parse(&src).unwrap()
+    }
+
+    /// What a host answers for `name`'s manifest.
+    fn applied(name: &str) -> Applied {
+        Applied {
+            name: name.into(),
+            version: "1.4.0".into(),
+            mcp_config: Some(format!(r#"{{"mcpServers":{{"{name}":{{}}}}}}"#)),
+            agents: Some(format!(r#"{{"{name}-agent":{{}}}}"#)),
+            instructions: format!("Use {name}."),
+            env: vec![("KB_REGION".into(), name.into())],
+            ..Applied::default()
+        }
+    }
+
+    fn sha() -> String {
+        "a".repeat(64)
+    }
+
+    /// A `Manifest` request: host, name, secrets, disabled.
+    type Asked = (String, String, Vec<String>, Vec<String>);
+
+    /// The `Manifest` requests queued.
+    fn manifest_requests(app: &mut App) -> Vec<Asked> {
         app.take_effects()
             .into_iter()
             .filter_map(|e| match e {
                 Effect::Request {
                     host,
-                    msg: Msg::Bundle { name, .. },
+                    msg:
+                        Msg::Manifest {
+                            name,
+                            secrets,
+                            disabled,
+                            ..
+                        },
                     ..
-                } => Some((host, name)),
+                } => Some((host, name, secrets, disabled)),
                 _ => None,
             })
             .collect()
     }
 
+    /// The hosts asked, as (host, name).
+    fn asked(app: &mut App) -> Vec<(String, String)> {
+        let mut asked: Vec<(String, String)> = manifest_requests(app)
+            .into_iter()
+            .map(|(h, n, _, _)| (h, n))
+            .collect();
+        asked.sort();
+        asked
+    }
+
     fn pair(host: &str, name: &str) -> (String, String) {
         (host.to_owned(), name.to_owned())
     }
 
     #[test]
-    fn a_list_is_listed_and_its_bundles_asked_of_every_connected_host() {
+    fn a_list_fetches_its_manifests_and_hosts_are_asked_once_they_arrive() {
         let mut app = app_with(&[]);
         app.on_plugin_host("local");
-        assert!(bundle_requests(&mut app).is_empty(), "no list yet");
+        assert!(asked(&mut app).is_empty(), "no list yet");
         app.on_plugins(
             "work",
             PluginsFetch::Listed(list("work", vec![sample("kb")])),
         );
-        assert_eq!(bundle_requests(&mut app), [pair("local", "kb")]);
-        let row = app.extensions.extensions.last().unwrap();
+        let effects = app.take_effects();
+        let fetched: Vec<&Effect> = effects
+            .iter()
+            .filter(|e| matches!(e, Effect::FetchManifest { name, .. } if name == "kb"))
+            .collect();
+        assert_eq!(fetched.len(), 1, "{effects:?}");
+        assert!(
+            !effects.iter().any(|e| matches!(e, Effect::Request { .. })),
+            "nothing asked before the manifest is here"
+        );
+        app.plugins.choices.insert(
+            "kb".into(),
+            PluginChoices {
+                disabled: vec!["mcp:kb".into()],
+                ..PluginChoices::default()
+            },
+        );
+        app.on_manifest("kb", &sha(), Ok(manifest_of("kb")));
         assert_eq!(
-            (row.name.as_str(), row.description.as_str()),
-            ("kb", "1.4.0 · work")
+            manifest_requests(&mut app),
+            [(
+                "local".to_owned(),
+                "kb".to_owned(),
+                vec!["KB_TOKEN".to_owned()],
+                vec!["mcp:kb".to_owned()]
+            )]
         );
-        // A second host gets asked on connect; the same list again asks no one.
+        // A second host is asked on connect; the same list again asks no
+        // one and fetches nothing.
         app.on_plugin_host("devbox");
-        assert_eq!(bundle_requests(&mut app), [pair("devbox", "kb")]);
+        assert_eq!(asked(&mut app), [pair("devbox", "kb")]);
         app.on_plugins(
             "work",
             PluginsFetch::Listed(list("work", vec![sample("kb")])),
         );
-        assert!(bundle_requests(&mut app).is_empty());
-        // A new version is asked again.
+        assert!(app.take_effects().is_empty());
+        // A new version is fetched; the hosts are asked once it is here.
         let mut v2 = sample("kb");
         v2.bundle_sha256 = "b".repeat(64);
         app.on_plugins("work", PluginsFetch::Listed(list("work", vec![v2])));
-        let mut asked = bundle_requests(&mut app);
-        asked.sort();
-        assert_eq!(asked, [pair("devbox", "kb"), pair("local", "kb")]);
+        assert!(asked(&mut app).is_empty());
+        app.on_manifest("kb", &sha(), Ok(manifest_of("kb")));
+        assert!(asked(&mut app).is_empty(), "stale: the old hash");
+        app.on_manifest("kb", &"b".repeat(64), Ok(manifest_of("kb")));
+        assert_eq!(asked(&mut app), [pair("devbox", "kb"), pair("local", "kb")]);
     }
 
     #[test]
-    fn answers_set_each_hosts_state_and_stale_ones_are_dropped() {
+    fn a_refused_manifest_is_asked_of_no_host_and_shown() {
         let mut app = app_with(&[]);
         app.on_plugin_host("local");
         app.on_plugins(
@@ -389,144 +826,182 @@ mod tests {
             PluginsFetch::Listed(list("work", vec![sample("kb")])),
         );
         app.take_effects();
-        let sha = "a".repeat(64);
-        assert_eq!(app.bundle_state("local", "kb"), Some(&BundleState::Pending));
-        let ready = Msg::Bundled {
-            name: "kb".into(),
-            path: Some("/c/kb.zip".into()),
-            installed: false,
-        };
-        app.on_bundled("local", "kb", &"b".repeat(64), Ok(ready.clone()));
-        assert_eq!(
-            app.bundle_state("local", "kb"),
-            Some(&BundleState::Pending),
-            "stale"
-        );
-        app.on_bundled("local", "kb", &sha, Ok(ready));
-        assert_eq!(
-            app.bundle_state("local", "kb"),
-            Some(&BundleState::Ready("/c/kb.zip".into()))
+        app.on_manifest("kb", &sha(), Err("hash mismatch".into()));
+        assert!(asked(&mut app).is_empty());
+        let details = app.plugin_details("kb", "work", None);
+        assert_eq!(details[1], "manifest: hash mismatch");
+    }
+
+    /// An app with `local` connected and profile `work` serving `kb`, whose
+    /// manifest is here and which `local` answered with `answer` (`None`:
+    /// not yet).
+    fn app_with_kb(answer: Option<Msg>) -> App {
+        let mut app = app_with(&[]);
+        app.modal = None;
+        app.on_plugin_host("local");
+        app.on_plugins(
+            "work",
+            PluginsFetch::Listed(list("work", vec![sample("kb")])),
         );
-        let installed = Msg::Bundled {
-            name: "kb".into(),
-            path: None,
-            installed: true,
-        };
-        app.on_bundled("local", "kb", &sha, Ok(installed));
+        app.on_manifest("kb", &sha(), Ok(manifest_of("kb")));
+        if let Some(msg) = answer {
+            app.on_applied("local", "kb", &sha(), Ok(msg));
+        }
+        app.take_effects();
+        app
+    }
+
+    #[test]
+    fn answers_set_each_hosts_state_and_stale_ones_are_dropped() {
+        let mut app = app_with_kb(None);
+        assert_eq!(app.apply_state("local", "kb"), Some(&ApplyState::Pending));
+        let ready = Msg::Applied(applied("kb"));
+        app.on_applied("local", "kb", &"b".repeat(64), Ok(ready.clone()));
+        assert_eq!(app.apply_state("local", "kb"), Some(&ApplyState::Pending));
+        app.on_applied("local", "kb", &sha(), Ok(ready));
         assert_eq!(
-            app.bundle_state("local", "kb"),
-            Some(&BundleState::Installed)
+            app.apply_state("local", "kb"),
+            Some(&ApplyState::Ready(Box::new(applied("kb"))))
         );
-        let old = std::io::Error::other("unsupported op: bundle");
-        app.on_bundled("local", "kb", &sha, Err(old));
-        assert_eq!(app.bundle_state("local", "kb"), Some(&BundleState::TooOld));
-        let failed = std::io::Error::other("bundle kb: hash mismatch");
-        app.on_bundled("local", "kb", &sha, Err(failed));
+        let old = std::io::Error::other("unsupported op: manifest");
+        app.on_applied("local", "kb", &sha(), Err(old));
+        assert_eq!(app.apply_state("local", "kb"), Some(&ApplyState::TooOld));
+        let failed = std::io::Error::other("manifest kb: format is not 1");
+        app.on_applied("local", "kb", &sha(), Err(failed));
         assert_eq!(
-            app.bundle_state("local", "kb"),
-            Some(&BundleState::Failed("hash mismatch".into()))
+            app.apply_state("local", "kb"),
+            Some(&ApplyState::Failed("format is not 1".into()))
         );
-        // A failed download is asked again on the next list arrival.
-        app.take_effects();
-        app.request_bundles("local");
-        assert_eq!(bundle_requests(&mut app), [pair("local", "kb")]);
-        assert_eq!(app.bundle_state("local", "kb"), Some(&BundleState::Pending));
+        // A failure is asked again on the next occasion; an answer is not.
+        app.request_all();
+        assert_eq!(asked(&mut app), [pair("local", "kb")]);
+        app.on_applied("local", "kb", &sha(), Ok(Msg::Applied(applied("kb"))));
+        app.request_all();
+        assert!(asked(&mut app).is_empty());
         // A host that left forgets its answers.
         app.plugin_host_gone("local");
-        assert_eq!(app.bundle_state("local", "kb"), None);
+        assert_eq!(app.apply_state("local", "kb"), None);
     }
 
     #[test]
-    fn env_comes_from_enabled_admitted_plugins_of_the_profile() {
+    fn a_host_still_installing_is_asked_again_every_30_s_for_10_minutes() {
+        let mut installing = applied("kb");
+        installing.skipped = vec![("plugin:p@m".into(), "installing".into())];
         let mut app = app_with(&[]);
-        let mut bad = sample("bad");
-        bad.env = vec![("LD_PRELOAD".into(), "/x.so".into())];
-        let mut other = sample("other");
-        other.env = vec![("OTHER_TOKEN".into(), "o".into())];
+        app.now = 1000;
+        app.on_plugin_host("local");
         app.on_plugins(
             "work",
-            PluginsFetch::Listed(list("work", vec![sample("kb"), bad, other])),
-        );
-        app.on_plugins(
-            "home",
-            PluginsFetch::Listed(list("home", vec![sample("mine")])),
+            PluginsFetch::Listed(list("work", vec![sample("kb")])),
         );
-        let pair = |k: &str, v: &str| (k.to_owned(), v.to_owned());
+        app.on_manifest("kb", &sha(), Ok(manifest_of("kb")));
+        app.on_applied("local", "kb", &sha(), Ok(Msg::Applied(installing.clone())));
+        app.take_effects();
+        app.now = 1029;
+        app.reask_installing();
+        assert!(asked(&mut app).is_empty());
+        app.now = 1030;
+        app.reask_installing();
+        assert_eq!(asked(&mut app), [pair("local", "kb")]);
+        // The last answer stays meanwhile.
         assert_eq!(
-            app.plugin_env("work"),
-            [pair("KB_TOKEN", "s3cret"), pair("OTHER_TOKEN", "o")]
+            app.apply_state("local", "kb"),
+            Some(&ApplyState::Ready(Box::new(installing.clone())))
         );
-        assert!(app.plugin_env("nope").is_empty());
-        // Off in the registry: no env.
-        app.plugins.config.insert("kb".into(), false);
-        app.list_plugins();
-        assert_eq!(app.plugin_env("work"), [pair("OTHER_TOKEN", "o")]);
+        app.on_applied("local", "kb", &sha(), Ok(Msg::Applied(installing)));
+        app.now = 1000 + 601;
+        app.reask_installing();
+        assert!(asked(&mut app).is_empty(), "10 minutes at most");
+        // Done installing: no more asking.
+        app.on_applied("local", "kb", &sha(), Ok(Msg::Applied(applied("kb"))));
+        app.now = 1030;
+        app.reask_installing();
+        assert!(asked(&mut app).is_empty());
+    }
+
+    fn part(name: &str, ready: bool, secrets: &[(&str, &str)]) -> PluginPart {
+        PluginPart {
+            name: name.into(),
+            applied: ready.then(|| applied(name)),
+            secrets: secrets
+                .iter()
+                .map(|(k, v)| (k.to_string(), v.to_string()))
+                .collect(),
+        }
     }
 
     #[test]
-    fn cached_lists_are_listed_at_start_and_every_profile_in_use_is_fetched() {
-        let mut app = app_with(&[]);
-        app.proxy_default = Some("work".into());
-        let choices = BTreeMap::from([("kb".to_owned(), false)]);
-        app.load_plugins(
-            vec![list("home", vec![sample("kb"), sample("mine")])],
-            choices,
+    fn compose_orders_flags_layers_env_and_joins_instructions() {
+        let parts = [
+            part("one", true, &[("ONE_TOKEN", "1"), ("KB_REGION", "secret")]),
+            part("two", false, &[("TWO_TOKEN", "2"), ("ONE_TOKEN", "x")]),
+            part("three", true, &[]),
+        ];
+        let c = compose(&parts);
+        assert_eq!(
+            c.args,
+            [
+                "--agents",
+                r#"{"one-agent":{}}"#,
+                "--mcp-config",
+                r#"{"mcpServers":{"one":{}}}"#,
+                "--agents",
+                r#"{"three-agent":{}}"#,
+                "--mcp-config",
+                r#"{"mcpServers":{"three":{}}}"#,
+            ]
         );
-        let rows: Vec<(&str, bool)> = app
-            .extensions
-            .extensions
-            .iter()
-            .filter(|e| e.is_plugin())
-            .map(|e| (e.name.as_str(), e.enabled))
-            .collect();
-        assert_eq!(rows, [("kb", false), ("mine", true)]);
-        // Read from the cache, the first fetch under way: no "unreachable".
-        app.now = 1000;
-        assert_eq!(app.plugin_details("mine", "home", None).len(), 1);
-        app.take_effects();
-        app.fetch_plugin_lists();
-        let fetched: Vec<String> = app
-            .take_effects()
-            .into_iter()
-            .filter_map(|e| match e {
-                Effect::FetchPlugins { profile } => Some(profile),
-                _ => None,
-            })
-            .collect();
-        assert_eq!(fetched, ["home", "work"]);
+        let pair = |k: &str, v: &str| (k.to_owned(), v.to_owned());
+        // [env] first (the first plugin's key wins), then the secrets.
+        assert_eq!(
+            c.env,
+            [
+                pair("ONE_TOKEN", "1"),
+                pair("KB_REGION", "secret"),
+                pair("TWO_TOKEN", "2"),
+            ]
+        );
+        assert_eq!(c.instructions, "Use one.\n\nUse three.");
+        assert!(c.notices.is_empty());
+        // A plugin without agents or servers brings no flag.
+        let mut bare = part("bare", true, &[]);
+        let a = bare.applied.as_mut().unwrap();
+        (a.agents, a.mcp_config) = (None, None);
+        assert!(compose(&[bare]).args.is_empty());
     }
 
-    /// An app with `local` connected and profile `work` serving `kb`,
-    /// whose bundle `local` answered with `answer` (`None`: not yet).
-    fn app_with_kb(answer: Option<Msg>) -> App {
-        let mut app = app_with(&[]);
-        app.modal = None;
-        app.on_plugin_host("local");
-        app.on_plugins(
-            "work",
-            PluginsFetch::Listed(list("work", vec![sample("kb")])),
+    #[test]
+    fn compose_takes_eight_plugins_and_cuts_instructions_at_8_kib() {
+        let parts: Vec<PluginPart> = (0..10).map(|i| part(&format!("p{i}"), true, &[])).collect();
+        let c = compose(&parts);
+        assert_eq!(c.args.len(), 8 * 4);
+        assert_eq!(
+            c.notices,
+            [
+                "plugin p8 left out: too many plugins",
+                "plugin p9 left out: too many plugins",
+            ]
         );
-        if let Some(msg) = answer {
-            app.on_bundled("local", "kb", &"a".repeat(64), Ok(msg));
-        }
-        app.take_effects();
-        app
+        let big = |name: &str, size: usize| {
+            let mut p = part(name, true, &[]);
+            p.applied.as_mut().unwrap().instructions = "i".repeat(size);
+            p
+        };
+        let parts = [big("a", 5000), big("b", 3190), big("c", 10)];
+        let c = compose(&parts);
+        assert_eq!(c.instructions.len(), 8192);
+        assert_eq!(c.notices, ["plugin instructions over 8 KiB: c left out"]);
+        let parts = [big("a", 5000), big("b", 4000), big("c", 10)];
+        let c = compose(&parts);
+        assert_eq!(c.instructions.len(), 5000);
+        assert_eq!(c.notices, ["plugin instructions over 8 KiB: b, c left out"]);
     }
 
-    fn ready() -> Msg {
-        Msg::Bundled {
-            name: "kb".into(),
-            path: Some("/c/plugins/kb.zip".into()),
-            installed: false,
-        }
-    }
+    /// A spawn's args, env, instructions, and the manifests it waits for.
+    type Spawned = (Vec<String>, Vec<(String, String)>, String, Vec<String>);
 
-    /// Start a claude session in `/w` on `local` with `proxy`; the deferred
-    /// spawn it queues: its args, env and the bundles it waits for.
-    fn spawn_with(
-        app: &mut App,
-        proxy: Option<&str>,
-    ) -> (Vec<String>, Vec<(String, String)>, Vec<String>) {
+    /// Start a claude session in `/w` on `local` with `proxy`.
+    fn spawn_with(app: &mut App, proxy: Option<&str>) -> Spawned {
         let id = uuid::Uuid::new_v4();
         let kind = crate::proto::SessionKind::Claude;
         let proxy = proxy.map(str::to_owned);
@@ -544,19 +1019,27 @@ mod tests {
         for e in effects {
             match e {
                 Effect::SpawnWithProxy {
-                    msg: Msg::Spawn(spec),
-                    bundles,
+                    msg,
+                    plugins,
+                    pending,
                     to,
                     ..
                 } => {
                     assert_eq!(to, ReplyTo::SpawnedDeferred(id));
-                    let names = bundles.into_iter().map(|b| b.name).collect();
-                    return (spec.args, spec.env, names);
+                    let names = pending.into_iter().map(|b| b.name).collect();
+                    let Msg::Spawn(spec) = with_plugins(msg, compose(&plugins)) else {
+                        panic!("a spawn")
+                    };
+                    let ext = spec.extensions.unwrap_or_default();
+                    return (spec.args, spec.env, ext.instructions, names);
                 }
                 Effect::Request {
                     msg: Msg::Spawn(spec),
                     ..
-                } => return (spec.args, spec.env, Vec::new()),
+                } => {
+                    let ext = spec.extensions.unwrap_or_default();
+                    return (spec.args, spec.env, ext.instructions, Vec::new());
+                }
                 _ => {}
             }
         }
@@ -564,76 +1047,208 @@ mod tests {
     }
 
     #[test]
-    fn a_spawn_gets_the_ready_bundle_and_the_plugin_env() {
-        let mut app = app_with_kb(Some(ready()));
-        let (args, env, waits) = spawn_with(&mut app, Some("work"));
-        assert_eq!(args, ["-c", "--plugin-dir", "/c/plugins/kb.zip"]);
-        assert_eq!(env, [("KB_TOKEN".to_owned(), "s3cret".to_owned())]);
+    fn a_spawn_gets_the_answered_plugins_flags_env_and_instructions() {
+        let mut app = app_with_kb(Some(Msg::Applied(applied("kb"))));
+        let (args, env, instructions, waits) = spawn_with(&mut app, Some("work"));
+        assert_eq!(
+            args,
+            [
+                "-c",
+                "--agents",
+                r#"{"kb-agent":{}}"#,
+                "--mcp-config",
+                r#"{"mcpServers":{"kb":{}}}"#
+            ]
+        );
+        let pair = |k: &str, v: &str| (k.to_owned(), v.to_owned());
+        assert_eq!(
+            env[..2],
+            [pair("KB_REGION", "kb"), pair("KB_TOKEN", "s3cret")]
+        );
+        assert_eq!(instructions, "Use kb.");
         assert!(waits.is_empty());
-        // Without a proxy: no plugin, no env.
-        let (args, env, waits) = spawn_with(&mut app, None);
+        // Without a proxy: no plugin.
+        let (args, env, instructions, waits) = spawn_with(&mut app, None);
         assert_eq!(
-            (args, env.len(), waits.len()),
-            (vec!["-c".to_owned()], 0, 0)
+            (args, env.len(), instructions.as_str(), waits.len()),
+            (vec!["-c".to_owned()], 0, "", 0)
         );
     }
 
     #[test]
-    fn a_spawn_waits_for_a_bundle_not_answered_yet() {
+    fn a_spawn_waits_for_a_host_that_has_not_answered() {
         let mut app = app_with_kb(None);
-        let (args, env, waits) = spawn_with(&mut app, Some("work"));
+        let (args, env, _, waits) = spawn_with(&mut app, Some("work"));
         assert_eq!(args, ["-c"]);
-        assert_eq!(env.len(), 1);
+        assert_eq!(env, [("KB_TOKEN".to_owned(), "s3cret".to_owned())]);
         assert_eq!(waits, ["kb"]);
         let notice = app.notice.as_ref().unwrap().text.clone();
-        assert_eq!(notice, "fetching plugin kb…");
+        assert_eq!(notice, "applying plugin kb…");
+        // A host that answers with installs under way is ready.
+        let mut installing = applied("kb");
+        installing.skipped = vec![("plugin:p@m".into(), "installing".into())];
+        app.on_applied("local", "kb", &sha(), Ok(Msg::Applied(installing)));
+        let (args, _, _, waits) = spawn_with(&mut app, Some("work"));
+        assert_eq!((args.len(), waits.len()), (5, 0));
     }
 
     #[test]
-    fn installed_failed_or_off_plugins_bring_no_bundle() {
-        let installed = Msg::Bundled {
-            name: "kb".into(),
-            path: None,
-            installed: true,
-        };
-        let mut app = app_with_kb(Some(installed));
-        let (args, env, waits) = spawn_with(&mut app, Some("work"));
+    fn failed_too_old_or_off_plugins_bring_no_flags() {
+        let mut app = app_with_kb(None);
+        let failed = std::io::Error::other("manifest kb: hash mismatch");
+        app.on_applied("local", "kb", &sha(), Err(failed));
+        let (args, env, _, waits) = spawn_with(&mut app, Some("work"));
         assert_eq!(
             (args.len(), env.len(), waits.len()),
             (1, 1, 0),
-            "env still applies"
+            "the secrets still apply"
         );
 
         let mut app = app_with_kb(None);
-        let failed = std::io::Error::other("bundle kb: hash mismatch");
-        app.on_bundled("local", "kb", &"a".repeat(64), Err(failed));
-        let (args, _, waits) = spawn_with(&mut app, Some("work"));
+        let old = std::io::Error::other("unsupported op: manifest");
+        app.on_applied("local", "kb", &sha(), Err(old));
+        let (args, _, _, waits) = spawn_with(&mut app, Some("work"));
         assert_eq!((args.len(), waits.len()), (1, 0));
 
-        let mut app = app_with_kb(Some(ready()));
+        let mut app = app_with_kb(Some(Msg::Applied(applied("kb"))));
         app.plugins.config.insert("kb".into(), false);
         app.list_plugins();
-        let (args, env, waits) = spawn_with(&mut app, Some("work"));
-        assert_eq!((args.len(), env.len(), waits.len()), (1, 0, 0));
+        let (args, env, instructions, waits) = spawn_with(&mut app, Some("work"));
+        assert_eq!(
+            (args.len(), env.len(), instructions.len(), waits.len()),
+            (1, 0, 0, 0)
+        );
     }
 
     #[test]
-    fn a_reset_sends_the_current_plugin_env_and_no_args() {
-        let mut app = app_with_kb(Some(ready()));
+    fn a_reset_sends_the_current_plugin_env_and_instructions_and_no_args() {
+        let mut app = app_with_kb(Some(Msg::Applied(applied("kb"))));
         spawn_with(&mut app, Some("work"));
         let id = app.sessions[0].id;
         app.reset_session(id, false);
         let respawn = app.take_effects().into_iter().find_map(|e| match e {
             Effect::SpawnWithProxy {
-                msg: Msg::Respawn(spec),
-                bundles,
+                msg,
+                plugins,
+                pending,
                 ..
-            } => Some((spec, bundles)),
+            } => Some((with_plugins(msg, compose(&plugins)), pending)),
             _ => None,
         });
-        let (spec, bundles) = respawn.expect("a respawn");
-        assert_eq!(spec.env, [("KB_TOKEN".to_owned(), "s3cret".to_owned())]);
-        assert!(bundles.is_empty());
+        let (msg, pending) = respawn.expect("a respawn");
+        let Msg::Respawn(spec) = msg else {
+            panic!("a respawn")
+        };
+        assert_eq!(spec.env.len(), 2);
+        assert_eq!(spec.extensions.unwrap().instructions, "Use kb.");
+        assert!(pending.is_empty());
+    }
+
+    #[test]
+    fn details_show_the_manifest_and_each_hosts_outcome() {
+        let mut a = applied("kb");
+        a.installed = vec!["plugin:p@m".into()];
+        a.skipped = vec![
+            ("plugin:q@m".into(), "installing".into()),
+            ("mcp:kb".into(), "missing secret KB_TOKEN".into()),
+        ];
+        a.dropped = vec!["--agents".into()];
+        let mut app = app_with_kb(Some(Msg::Applied(a)));
+        app.on_plugin_host("devbox");
+        let old = std::io::Error::other("unsupported op: manifest");
+        app.on_applied("devbox", "kb", &sha(), Err(old));
+        assert_eq!(
+            app.plugin_details("kb", "work", None),
+            [
+                "turning it on or off applies to new sessions",
+                "The knowledge base",
+                "local: ready · 1 installed, 0 present, 1 installing, 1 skipped",
+                "  mcp:kb: missing secret KB_TOKEN",
+                "local: claude too old: --agents",
+                "devbox: daemon too old",
+                "installed plugins stay until you remove them: claude plugin uninstall <id>",
+            ]
+        );
+    }
+
+    #[test]
+    fn secrets_come_from_enabled_admitted_plugins_of_the_profile() {
+        let mut app = app_with(&[]);
+        let mut bad = sample("bad");
+        bad.env = vec![("LD_PRELOAD".into(), "/x.so".into())];
+        let mut other = sample("other");
+        other.env = vec![("OTHER_TOKEN".into(), "o".into())];
+        app.on_plugins(
+            "work",
+            PluginsFetch::Listed(list("work", vec![sample("kb"), bad, other])),
+        );
+        app.on_plugins(
+            "home",
+            PluginsFetch::Listed(list("home", vec![sample("mine")])),
+        );
+        let secrets = |app: &App, profile: &str| -> Vec<(String, String)> {
+            let (parts, _) = app.plugin_parts("local", profile);
+            parts.into_iter().flat_map(|p| p.secrets).collect()
+        };
+        let pair = |k: &str, v: &str| (k.to_owned(), v.to_owned());
+        assert_eq!(
+            secrets(&app, "work"),
+            [pair("KB_TOKEN", "s3cret"), pair("OTHER_TOKEN", "o")]
+        );
+        assert!(secrets(&app, "nope").is_empty());
+        // Off in the registry: no secrets.
+        app.plugins.config.insert("kb".into(), false);
+        app.list_plugins();
+        assert_eq!(secrets(&app, "work"), [pair("OTHER_TOKEN", "o")]);
+    }
+
+    #[test]
+    fn cached_lists_are_listed_at_start_and_every_profile_in_use_is_fetched() {
+        let mut app = app_with(&[]);
+        app.proxy_default = Some("work".into());
+        let choices = BTreeMap::from([("kb".to_owned(), false)]);
+        app.load_plugins(
+            vec![list("home", vec![sample("kb"), sample("mine")])],
+            choices,
+            BTreeMap::new(),
+        );
+        let rows: Vec<(&str, bool)> = app
+            .extensions
+            .extensions
+            .iter()
+            .filter(|e| e.is_plugin())
+            .map(|e| (e.name.as_str(), e.enabled))
+            .collect();
+        assert_eq!(rows, [("kb", false), ("mine", true)]);
+        // Both manifests are fetched, on or off.
+        let manifests: Vec<String> = app
+            .take_effects()
+            .into_iter()
+            .filter_map(|e| match e {
+                Effect::FetchManifest { name, .. } => Some(name),
+                _ => None,
+            })
+            .collect();
+        assert_eq!(manifests, ["kb", "mine"]);
+        // Read from the cache, the first fetch under way: no "unreachable".
+        app.now = 1000;
+        assert_eq!(
+            app.plugin_details("mine", "home", None),
+            [
+                "turning it on or off applies to new sessions",
+                "manifest: fetching"
+            ]
+        );
+        app.fetch_plugin_lists();
+        let fetched: Vec<String> = app
+            .take_effects()
+            .into_iter()
+            .filter_map(|e| match e {
+                Effect::FetchPlugins { profile } => Some(profile),
+                _ => None,
+            })
+            .collect();
+        assert_eq!(fetched, ["home", "work"]);
     }
 
     #[test]
@@ -645,12 +1260,12 @@ mod tests {
         );
         app.on_plugins("work", PluginsFetch::Unreachable);
         assert_eq!(app.plugins.lists["work"].1, ListState::Unreachable);
-        assert_eq!(app.plugin_env("work").len(), 1);
+        assert_eq!(app.plugin_parts("local", "work").0.len(), 1);
         app.now = 100 + 180;
         let details = app.plugin_details("kb", "work", None);
         assert_eq!(details.last().unwrap(), "proxy unreachable, cached 3m");
         app.on_plugins("work", PluginsFetch::SignedOut);
-        assert!(app.plugin_env("work").is_empty());
+        assert!(app.plugin_parts("local", "work").0.is_empty());
         assert!(!app.extensions.extensions.iter().any(|e| e.name == "kb"));
         let notice = app.notice.as_ref().unwrap().text.clone();
         assert_eq!(
@@ -661,4 +1276,33 @@ mod tests {
         app.on_plugins("home", PluginsFetch::Unreachable);
         assert!(!app.plugins.lists.contains_key("home"));
     }
+
+    #[test]
+    fn r_asks_every_host_again_and_fetches_every_manifest() {
+        let mut app = app_with_kb(Some(Msg::Applied(applied("kb"))));
+        app.refresh_plugins();
+        let effects = app.take_effects();
+        let again = effects
+            .iter()
+            .filter(|e| {
+                matches!(
+                    e,
+                    Effect::Request {
+                        msg: Msg::Manifest { .. },
+                        ..
+                    }
+                )
+            })
+            .count();
+        let fetches = effects
+            .iter()
+            .filter(|e| matches!(e, Effect::FetchManifest { .. }))
+            .count();
+        assert_eq!((again, fetches), (1, 1));
+        // The answer stays while it is asked again.
+        assert!(matches!(
+            app.apply_state("local", "kb"),
+            Some(ApplyState::Ready(_))
+        ));
+    }
 }
diff --git a/tests/manager.rs b/tests/manager.rs
index e43958f..ad2681b 100644
--- a/tests/manager.rs
+++ b/tests/manager.rs
@@ -2757,207 +2757,3 @@ fn test_extensions_screen_turns_extensions_off_and_saves_it() {
     tui.send_keys(ALT_Q);
     tui.wait_exit(WAIT);
 }
-
-// ── Proxy plugins ────────────────────────────────────────────────────────────
-
-/// The fake plugin: its name, the bytes of its pre-seeded zip, and its env.
-const PLUGIN: &str = "vocdoni-test";
-const PLUGIN_ZIP: &[u8] = b"PK fake bundle";
-const PLUGIN_KEY: &str = "VOCDONI_TEST_TOKEN";
-
-/// A fake claude-proxy on loopback: `GET /v1/claudio/plugins` lists
-/// [`PLUGIN`] (a bundle URL nothing listens on: the daemon must use its
-/// cache), every other path is a 404. Returns its `host:port`.
-fn fake_plugin_proxy() -> String {
-    use sha2::Digest;
-    use std::io::{Read, Write};
-    let sha = format!("{:x}", sha2::Sha256::digest(PLUGIN_ZIP));
-    let body = serde_json::json!({"plugins": [{
-        "name": PLUGIN, "version": "1.0.0",
-        "bundle": {"url": "https://127.0.0.1:9/never.zip", "sha256": sha},
-        "installed_id": "", "env": {PLUGIN_KEY: "s3cret"}}]})
-    .to_string();
-    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind fake proxy");
-    let addr = listener.local_addr().unwrap().to_string();
-    thread::spawn(move || {
-        for stream in listener.incoming() {
-            let Ok(mut stream) = stream else { continue };
-            let mut head = Vec::new();
-            let mut byte = [0u8; 1];
-            while !head.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap_or(0) == 1 {
-                head.push(byte[0]);
-            }
-            let line = String::from_utf8_lossy(&head);
-            let (status, reply) = if line.starts_with("GET /v1/claudio/plugins ") {
-                ("200 OK", body.as_str())
-            } else {
-                ("404 Not Found", "{}")
-            };
-            let _ = write!(
-                stream,
-                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n\
-                 Content-Length: {}\r\nConnection: close\r\n\r\n{reply}",
-                reply.len()
-            );
-        }
-    });
-    addr
-}
-
-/// Pre-seed the daemon's bundle cache under `cache` (`XDG_CACHE_HOME`) with
-/// [`PLUGIN`]'s zip and sidecar; the zip's path.
-fn seed_plugin_cache(cache: &std::path::Path) -> String {
-    use sha2::Digest;
-    let dir = cache.join("claudio/plugins");
-    fs::create_dir_all(&dir).unwrap();
-    let zip = dir.join(format!("{PLUGIN}.zip"));
-    fs::write(&zip, PLUGIN_ZIP).unwrap();
-    let sha = format!("{:x}", sha2::Sha256::digest(PLUGIN_ZIP));
-    fs::write(dir.join(format!("{PLUGIN}.sha256")), format!("{sha}\n")).unwrap();
-    zip.to_string_lossy().into_owned()
-}
-
-/// A fake claude whose `--help` lists `--plugin-dir` when `knows_plugins`,
-/// and which records each launch in `<dumps>/<n>/` (`argv`, one argument
-/// per line, and `env`) before the usual banner and `cat`.
-fn write_plugin_claude(h: &ManagerHarness, dumps: &std::path::Path, knows_plugins: bool) {
-    use std::os::unix::fs::PermissionsExt;
-    fs::create_dir_all(dumps).unwrap();
-    let help = if knows_plugins {
-        "echo '  --plugin-dir <paths...>  Load plugins from directories or zips'"
-    } else {
-        ":"
-    };
-    let dumps = dumps.display();
-    let script = format!(
-        r#"#!/bin/sh
-[ "$1" = "--version" ] && echo 'claude 0.0.0-fake' && exit 0
-if [ "$1" = "--help" ]; then {help}; exit 0; fi
-d="{dumps}/$(ls "{dumps}" | wc -l | tr -d ' ')"
-mkdir -p "$d"
-printf '%s\n' "$@" > "$d/argv"
-env > "$d/env"
-echo "FAKE_CLAUDE_BANNER pid=$$"
-exec cat
-"#
-    );
-    fs::write(&h.fake_claude, script).unwrap();
-    fs::set_permissions(&h.fake_claude, fs::Permissions::from_mode(0o755)).unwrap();
-}
-
-/// Launch `n`'s recorded argv and env, once both are written.
-fn launch_dump(dumps: &std::path::Path, n: usize) -> (Vec<String>, String) {
-    let dir = dumps.join(n.to_string());
-    let deadline = Instant::now() + WAIT;
-    loop {
-        let argv = fs::read_to_string(dir.join("argv"));
-        let env = fs::read_to_string(dir.join("env"));
-        if let (Ok(argv), Ok(env)) = (argv, env) {
-            if !env.is_empty() {
-                return (argv.lines().map(str::to_owned).collect(), env);
-            }
-        }
-        assert!(
-            Instant::now() < deadline,
-            "launch {n} not recorded in {dir:?}"
-        );
-        thread::sleep(Duration::from_millis(100));
-    }
-}
-
-/// Start a TUI with the fake proxy as the `env` profile and the bundle
-/// cache under the harness root; wait until the plugin list is cached.
-fn start_with_plugins(h: &ManagerHarness, proxy: &str) -> TuiProcess {
-    let cache = h.root.join("xdg-cache");
-    let tui = h.start_tui_with(|cmd| {
-        cmd.env("CLAUDIO_PROXY_URL", format!("tok@{proxy}"));
-        cmd.env("XDG_CACHE_HOME", &cache);
-    });
-    let plugins_json = h.config_home.join("claudio/plugins.json");
-    let deadline = Instant::now() + WAIT;
-    while !fs::read_to_string(&plugins_json).is_ok_and(|s| s.contains(PLUGIN)) {
-        assert!(
-            Instant::now() < deadline,
-            "the plugin list was never cached"
-        );
-        thread::sleep(Duration::from_millis(100));
-    }
-    tui
-}
-
-/// A proxy plugin reaches a new session: its bundle as `--plugin-dir <the
-/// daemon's cached zip>` and its env key. Turned off in the extensions
-/// screen (saved in config.toml), the next session has neither.
-#[test]
-fn test_proxy_plugin_reaches_new_sessions_until_turned_off() {
-    let h = ManagerHarness::new();
-    let dumps = h.root.join("launches");
-    write_plugin_claude(&h, &dumps, true);
-    let zip = seed_plugin_cache(&h.root.join("xdg-cache"));
-    let proxy = fake_plugin_proxy();
-    let mut tui = start_with_plugins(&h, &proxy);
-
-    wizard_pick_dir(&mut tui, &h.dirs[0]);
-    let (argv, env) = launch_dump(&dumps, 0);
-    let at = argv.iter().position(|a| a == "--plugin-dir");
-    assert_eq!(
-        at.and_then(|i| argv.get(i + 1)),
-        Some(&zip),
-        "argv: {argv:?}"
-    );
-    assert!(env.contains(&format!("{PLUGIN_KEY}=s3cret")), "{env}");
-    assert!(
-        !argv.iter().any(|a| a.contains("s3cret")),
-        "no secret in argv"
-    );
-
-    // The extensions screen lists it, ready on this host; Space turns it off.
-    tui.send_keys(ALT_I);
-    tui.wait_for(&format!("[x] {PLUGIN}"), Region::Screen, WAIT);
-    for _ in 0..4 {
-        tui.send_keys(DOWN_ARROW);
-    }
-    tui.wait_for("1.0.0 · env", Region::Screen, WAIT);
-    tui.wait_for("local: ready", Region::Screen, WAIT);
-    tui.send_keys(b" ");
-    tui.wait_for(&format!("[ ] {PLUGIN}"), Region::Screen, WAIT);
-    tui.wait_for(
-        &format!("extension {PLUGIN} off (saved)"),
-        Region::StatusBar,
-        WAIT,
-    );
-    let config = fs::read_to_string(h.config_home.join("claudio/config.toml")).unwrap();
-    assert!(
-        config.contains(&format!("[extensions]\n{PLUGIN} = false")),
-        "{config}"
-    );
-    tui.send_keys(ESC);
-    tui.wait_until(|s| !s.contains("Space toggle", Region::Screen), WAIT);
-
-    tui.send_keys(ALT_N);
-    wizard_pick_dir(&mut tui, &h.dirs[1]);
-    let (argv, env) = launch_dump(&dumps, 1);
-    assert!(!argv.iter().any(|a| a == "--plugin-dir"), "{argv:?}");
-    assert!(!env.contains(PLUGIN_KEY), "{env}");
-    tui.quit(WAIT);
-}
-
-/// A claude whose `--help` does not list `--plugin-dir` still starts: the
-/// daemon leaves the bundle out, and the env applies.
-#[test]
-fn test_proxy_plugin_bundle_is_left_out_for_an_old_claude() {
-    let h = ManagerHarness::new();
-    let dumps = h.root.join("launches");
-    write_plugin_claude(&h, &dumps, false);
-    seed_plugin_cache(&h.root.join("xdg-cache"));
-    let proxy = fake_plugin_proxy();
-    let mut tui = start_with_plugins(&h, &proxy);
-    wizard_pick_dir(&mut tui, &h.dirs[0]);
-    let (argv, env) = launch_dump(&dumps, 0);
-    assert!(
-        !argv.iter().any(|a| a.starts_with("--plugin-dir")),
-        "{argv:?}"
-    );
-    assert!(env.contains(&format!("{PLUGIN_KEY}=s3cret")), "{env}");
-    tui.quit(WAIT);
-}
`````
Run: `umask 022; cargo test --locked --bin claudio tui::`
Expected: `` error[E0425]: cannot find type `Manifest` in this scope ``, `` error[E0433]: failed to resolve: use of undeclared type `Manifest` ``, ending `` error: could not compile `claudio` (bin "claudio" test) due to 76 previous errors ``

Step 2: implement
`````diff
diff --git a/src/proto.rs b/src/proto.rs
index 3abc0b8..d8059a2 100644
--- a/src/proto.rs
+++ b/src/proto.rs
@@ -644,17 +644,28 @@ impl Msg {
         self.with_env(crate::proxy::env::merge(&plugin, proxy))
     }
 
-    /// `self` with `--plugin-dir <path>` added for each of `paths`, if it is
-    /// a `Spawn` (a `Respawn` keeps the journaled args).
-    pub fn with_plugin_dirs(mut self, paths: &[String]) -> Msg {
+    /// `self` with the plugins' flags (`--agents <json>`, `--mcp-config
+    /// <json>`…) after its args, if it is a `Spawn` (a `Respawn` keeps the
+    /// journaled args).
+    pub fn with_plugin_args(mut self, args: &[String]) -> Msg {
         if let Msg::Spawn(spec) = &mut self {
-            for path in paths {
-                spec.args.push("--plugin-dir".to_owned());
-                spec.args.push(path.clone());
-            }
+            spec.args.extend(args.iter().cloned());
         }
         self
     }
+
+    /// `self` with the plugins' instructions, if it is a `Spawn` or a
+    /// `Respawn` (which then carries extensions).
+    pub fn with_instructions(mut self, instructions: String) -> Msg {
+        let ext = match &mut self {
+            Msg::Spawn(spec) => &mut spec.extensions,
+            Msg::Respawn(spec) => &mut spec.extensions,
+            _ => return self,
+        };
+        ext.get_or_insert_with(SessionExtensions::default)
+            .instructions = instructions;
+        self
+    }
 }
 
 impl std::fmt::Debug for RespawnSpec {
diff --git a/src/proxy/plugins.rs b/src/proxy/plugins.rs
index 0a5f486..c746da3 100644
--- a/src/proxy/plugins.rs
+++ b/src/proxy/plugins.rs
@@ -213,7 +213,8 @@ pub fn admit(p: &Plugin) -> Result<(), String> {
     if !(1..=64).contains(&p.version.len()) || !printable(&p.version) {
         return Err("bad version".to_owned());
     }
-    if !p.bundle_url.starts_with("https://") || !printable(&p.bundle_url) {
+    let https = p.bundle_url.starts_with("https://") || test_http_ok(&p.bundle_url);
+    if !https || !printable(&p.bundle_url) {
         return Err("bundle URL is not https".to_owned());
     }
     if !sha256_ok(&p.bundle_sha256) {
@@ -305,6 +306,65 @@ pub async fn fetch(name: &str, profile: &Profile) -> Result<PluginList, ApiError
     Ok(list_of(reply, name, paths::unix_now()))
 }
 
+// ── Manifest ──────────────────────────────────────────────────────────────────
+
+/// How long a manifest fetch may take.
+const MANIFEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
+
+/// Fetch a plugin's manifest: `url` (https, redirects to https only), at
+/// most 256 KiB, whose SHA-256 is `sha256`; its text. Kept in memory only.
+/// The reason names no value.
+pub async fn fetch_manifest(url: &str, sha256: &str) -> Result<String, String> {
+    use crate::plugins::manifest::MAX_BYTES;
+    use sha2::{Digest, Sha256};
+    let plain_ok = test_http_ok(url);
+    if !url.starts_with("https://") && !plain_ok {
+        return Err("not https".to_owned());
+    }
+    let policy = reqwest::redirect::Policy::custom(move |attempt| {
+        if attempt.previous().len() >= 5 {
+            attempt.error("too many redirects")
+        } else if attempt.url().scheme() == "https" || plain_ok {
+            attempt.follow()
+        } else {
+            attempt.error("redirect to a non-https URL")
+        }
+    });
+    let client = reqwest::Client::builder()
+        .connect_timeout(MANIFEST_TIMEOUT)
+        .timeout(MANIFEST_TIMEOUT)
+        .redirect(policy)
+        .build()
+        .map_err(|e| format!("http client: {e}"))?;
+    let mut resp = client
+        .get(url)
+        .send()
+        .await
+        .map_err(|e| format!("unreachable: {}", e.without_url()))?;
+    if !resp.status().is_success() {
+        return Err(format!("HTTP {}", resp.status().as_u16()));
+    }
+    let over = || "over 256 KiB".to_owned();
+    if resp.content_length().is_some_and(|n| n > MAX_BYTES as u64) {
+        return Err(over());
+    }
+    let mut body = Vec::new();
+    while let Some(chunk) = resp
+        .chunk()
+        .await
+        .map_err(|e| format!("unreachable: {}", e.without_url()))?
+    {
+        if body.len() + chunk.len() > MAX_BYTES {
+            return Err(over());
+        }
+        body.extend_from_slice(&chunk);
+    }
+    if format!("{:x}", Sha256::digest(&body)) != sha256 {
+        return Err("hash mismatch".to_owned());
+    }
+    String::from_utf8(body).map_err(|_| "not UTF-8".to_owned())
+}
+
 // ── Cache ─────────────────────────────────────────────────────────────────────
 
 /// `plugins.json`: one entry per profile name.
diff --git a/src/tui/app.rs b/src/tui/app.rs
index 256d4d8..f31e548 100644
--- a/src/tui/app.rs
+++ b/src/tui/app.rs
@@ -132,21 +132,23 @@ pub enum Effect {
     },
     /// Spawn (or respawn) a session with proxy config: the effect runner
     /// fetches the proxy config (with a 5 s timeout) unless it is cached,
-    /// waits up to 20 s for `bundles` (asking `host` for each), builds the
-    /// env, then sends the request. This avoids silently using fallback
-    /// model defaults when the proxy-config cache is cold, and starting a
-    /// session without a plugin whose bundle is a moment away.
+    /// waits up to 20 s for `pending` (asking `host` for each), adds the
+    /// plugins' share ([`super::plugins::compose`]) and the proxy env, then
+    /// sends the request. This avoids silently using fallback model
+    /// defaults when the proxy-config cache is cold, and starting a session
+    /// without a plugin whose manifest a host has not answered for yet.
     SpawnWithProxy {
         host: String,
-        /// A `Spawn` or `Respawn` whose `env` holds the plugin env only;
-        /// the runner adds the proxy env, and the ready bundles' paths.
+        /// A `Spawn` or `Respawn` without the plugins' share or proxy env.
         msg: Msg,
         proxy_name: String,
         /// `--first-party` for this session (see `SessionView::first_party`).
         first_party: bool,
         to: ReplyTo,
-        /// The plugins whose bundle `host` has not answered for yet.
-        bundles: Vec<PendingBundle>,
+        /// The profile's enabled plugins, in registry order.
+        plugins: Vec<super::plugins::PluginPart>,
+        /// The `Manifest` requests of the plugins `host` has not answered.
+        pending: Vec<PendingManifest>,
     },
     /// Put this text on the outer terminal's clipboard (OSC 52).
     CopyToClipboard(String),
@@ -198,12 +200,19 @@ pub enum Effect {
     /// Fetch `profile`'s plugin list, then save it to (or drop it from)
     /// `plugins.json`; the outcome goes to [`App::on_plugins`].
     FetchPlugins { profile: String },
+    /// Fetch plugin `name`'s manifest (https, 256 KiB, hash checked) and
+    /// parse it; the outcome goes to [`App::on_manifest`].
+    FetchManifest {
+        name: String,
+        url: String,
+        sha256: String,
+    },
 }
 
-/// A plugin bundle a deferred spawn waits for: its `Bundle` request, and
-/// how the answer reaches [`App::on_reply`].
+/// A plugin manifest a deferred spawn waits for: its `Manifest` request,
+/// and how the answer reaches [`App::on_reply`].
 #[derive(Debug, Clone, PartialEq)]
-pub struct PendingBundle {
+pub struct PendingManifest {
     pub name: String,
     pub msg: Msg,
     pub to: ReplyTo,
@@ -245,8 +254,8 @@ pub enum ReplyTo {
     Search { gen: u64, host: String },
     /// `LastPrompt` on `host`, for the snippet save started under `gen`.
     LastPrompt { gen: u64, host: String },
-    /// `Bundle` of plugin `name` at hash `sha256` on `host`.
-    Bundle {
+    /// `Manifest` of plugin `name` at hash `sha256` on `host`.
+    Manifest {
         host: String,
         name: String,
         sha256: String,
@@ -369,7 +378,8 @@ pub struct App {
     /// The external extensions the extension host disabled after
     /// failures (until claudio restarts), by name.
     pub failed_extensions: Vec<String>,
-    /// The proxy plugins: lists, saved choices, each host's bundles.
+    /// The proxy plugins: lists, saved choices, manifests, each host's
+    /// answers.
     pub plugins: super::plugins::Plugins,
 }
 
@@ -875,6 +885,7 @@ impl App {
         self.tick = self.tick.wrapping_add(1);
         self.now = crate::paths::unix_now();
         self.confirm_tick();
+        self.reask_installing();
         self.refresh_session_cred(SESSION_CRED_POLL_SECS);
         if let Some(n) = &mut self.notice {
             n.ticks_left = n.ticks_left.saturating_sub(1);
diff --git a/src/tui/extensions_view.rs b/src/tui/extensions_view.rs
index 8c12ef1..bd0f942 100644
--- a/src/tui/extensions_view.rs
+++ b/src/tui/extensions_view.rs
@@ -259,14 +259,12 @@ impl App {
                 self.notify(format!("plugin {} is refused: {why}", clean(&name)));
                 return;
             }
-            // Kept for a list that comes later; on: the hosts get its bundle.
+            // Kept for a list that comes later; on: the hosts get its
+            // manifest (unless they answered for it already).
             self.plugins.config.insert(name.clone(), on);
             self.extensions.extensions[index].enabled = on;
             if on {
-                let hosts: Vec<String> = self.plugins.hosts.iter().cloned().collect();
-                for host in hosts {
-                    self.request_bundles(&host);
-                }
+                self.request_all();
             }
             self.emit(Effect::SaveExtension { name, on });
             self.redraw = true;
diff --git a/src/tui/interaction.rs b/src/tui/interaction.rs
index 1243a49..569b3db 100644
--- a/src/tui/interaction.rs
+++ b/src/tui/interaction.rs
@@ -1116,8 +1116,8 @@ impl App {
                 }
             }
             (ReplyTo::ClaudeUpdate(host), reply) => self.on_claude_updated(&host, reply),
-            (ReplyTo::Bundle { host, name, sha256 }, reply) => {
-                self.on_bundled(&host, &name, &sha256, reply)
+            (ReplyTo::Manifest { host, name, sha256 }, reply) => {
+                self.on_applied(&host, &name, &sha256, reply)
             }
             // Typing a path that doesn't exist (yet) is not an error.
             (ReplyTo::DirEntries, Err(_)) => {}
diff --git a/src/tui/mod.rs b/src/tui/mod.rs
index 41cdf5f..3dfafee 100644
--- a/src/tui/mod.rs
+++ b/src/tui/mod.rs
@@ -326,7 +326,11 @@ async fn main(launch: Launch) -> ExitCode {
         },
     });
     app.extensions = registry;
-    app.load_plugins(cached_plugins, config.extensions.clone());
+    app.load_plugins(
+        cached_plugins,
+        config.extensions.clone(),
+        config.plugins.clone(),
+    );
     app.set_host_features("local", &local_features);
     app.on_plugin_host("local");
     let start = match plain {
@@ -587,6 +591,13 @@ enum HostEvent {
         profile: String,
         fetch: plugins::PluginsFetch,
     },
+    /// Plugin `name`'s manifest at hash `sha256` was fetched and parsed, or
+    /// why not.
+    Manifest {
+        name: String,
+        sha256: String,
+        result: Result<crate::plugins::manifest::Manifest, String>,
+    },
     /// A notice from a background task.
     Notice(String),
 }
@@ -1233,6 +1244,9 @@ async fn event_loop(
                 }
                 Some(HostEvent::SnippetSaved { result, cut }) => app.on_snippet_saved(result, cut),
                 Some(HostEvent::Plugins { profile, fetch }) => app.on_plugins(&profile, fetch),
+                Some(HostEvent::Manifest { name, sha256, result }) => {
+                    app.on_manifest(&name, &sha256, result);
+                }
                 Some(HostEvent::Notice(text)) => app.notify(text),
                 None => return Ok(Default::default()),
             },
@@ -1447,6 +1461,21 @@ fn run_effect(
                 let _ = tx.send(HostEvent::Plugins { profile, fetch }).await;
             });
         }
+        Effect::FetchManifest { name, url, sha256 } => {
+            let tx = ev_tx.clone();
+            tokio::spawn(async move {
+                let fetched = crate::proxy::plugins::fetch_manifest(&url, &sha256).await;
+                let result = fetched
+                    .and_then(|text| crate::plugins::manifest::Manifest::parse_named(&text, &name));
+                let _ = tx
+                    .send(HostEvent::Manifest {
+                        name,
+                        sha256,
+                        result,
+                    })
+                    .await;
+            });
+        }
         Effect::FetchProxyConfig { profile_name } => {
             let tx = ev_tx.clone();
             let name = profile_name.clone();
@@ -1515,7 +1544,8 @@ fn run_effect(
             proxy_name,
             first_party,
             to,
-            bundles,
+            plugins: mut parts,
+            pending,
         } => {
             // Resolve the client now (synchronous, cheap) so we can move it
             // into the async task. Client is Clone + Send + 'static.
@@ -1526,7 +1556,7 @@ fn run_effect(
             tokio::spawn(async move {
                 // Fetch proxy config with a 5 s timeout so the first spawn
                 // uses the real model/rate-limit overrides, not fallback
-                // defaults; meanwhile, wait for the plugin bundles.
+                // defaults; meanwhile, wait for the plugin manifests.
                 let config = async {
                     match cached {
                         Some(cfg) => Some(cfg),
@@ -1539,13 +1569,22 @@ fn run_effect(
                         .flatten(),
                     }
                 };
-                let waits = wait_bundles(client.as_ref(), bundles, &tx);
-                let (cfg, (dirs, left_out)) = tokio::join!(config, waits);
+                let waits = wait_manifests(client.as_ref(), pending, &tx);
+                let (cfg, (mut ready, left_out)) = tokio::join!(config, waits);
                 if !left_out.is_empty() {
                     let names = left_out.join(", ");
                     let text = format!("plugin {names} left out: not ready on {host}");
                     let _ = notices.send(HostEvent::Notice(text)).await;
                 }
+                for part in &mut parts {
+                    if part.applied.is_none() {
+                        part.applied = ready.remove(&part.name);
+                    }
+                }
+                let composed = plugins::compose(&parts);
+                for text in composed.notices.clone() {
+                    let _ = notices.send(HostEvent::Notice(text)).await;
+                }
 
                 // Build env from profile + (possibly fresh) config.
                 let env = crate::proxy::resolve::proxy_env_for(
@@ -1563,7 +1602,7 @@ fn run_effect(
                     Ok(env) => {
                         match client {
                             Some(c) => {
-                                let msg = msg.with_plugin_dirs(&dirs).merge_env(env);
+                                let msg = plugins::with_plugins(msg, composed).merge_env(env);
                                 let reply = c.request(msg);
                                 let _ = tx.send((to, reply.await));
                             }
@@ -1584,53 +1623,53 @@ fn run_effect(
     }
 }
 
-/// How long a spawn waits for its plugin bundles.
-const BUNDLE_WAIT: Duration = Duration::from_secs(20);
+/// How long a spawn waits for a host to answer its plugin manifests.
+pub(super) const BUNDLE_WAIT: Duration = Duration::from_secs(20);
 
-/// Ask `client` for each of `bundles` and wait up to [`BUNDLE_WAIT`]: the
-/// paths of those that came back ready, and the names of those that did
-/// not (pending, failed, or no client). Every answer, even a late one, also
-/// goes to `App` through `tx`.
-async fn wait_bundles(
+/// Ask `client` for each of `pending` and wait up to [`BUNDLE_WAIT`]: the
+/// answers that came back (`Applied`, by plugin name), and the names of
+/// those that did not (late, failed, or no client). Every answer, even a
+/// late one, also goes to `App` through `tx`.
+async fn wait_manifests(
     client: Option<&Client>,
-    bundles: Vec<app::PendingBundle>,
+    pending: Vec<app::PendingManifest>,
     tx: &mpsc::UnboundedSender<(ReplyTo, io::Result<Msg>)>,
-) -> (Vec<String>, Vec<String>) {
-    let mut dirs = Vec::new();
+) -> (
+    std::collections::HashMap<String, crate::proto::Applied>,
+    Vec<String>,
+) {
+    let mut ready = std::collections::HashMap::new();
     let mut left_out = Vec::new();
     let deadline = tokio::time::Instant::now() + BUNDLE_WAIT;
     let mut waiting = Vec::new();
-    for b in bundles {
+    for m in pending {
         let Some(client) = client else {
-            left_out.push(b.name);
+            left_out.push(m.name);
             continue;
         };
-        let reply = client.request(b.msg);
+        let reply = client.request(m.msg);
         let (done, answer) = tokio::sync::oneshot::channel();
         let tx = tx.clone();
         tokio::spawn(async move {
             let reply = reply.await;
-            // `Some(None)`: installed in Claude Code, no bundle needed.
-            let ready = match &reply {
-                Ok(Msg::Bundled {
-                    installed: true, ..
-                }) => Some(None),
-                Ok(Msg::Bundled { path: Some(p), .. }) => Some(Some(p.clone())),
+            let applied = match &reply {
+                Ok(Msg::Applied(a)) => Some(a.clone()),
                 _ => None,
             };
-            let _ = done.send(ready);
-            let _ = tx.send((b.to, reply));
+            let _ = done.send(applied);
+            let _ = tx.send((m.to, reply));
         });
-        waiting.push((b.name, answer));
+        waiting.push((m.name, answer));
     }
     for (name, answer) in waiting {
         match tokio::time::timeout_at(deadline, answer).await {
-            Ok(Ok(Some(Some(path)))) => dirs.push(path),
-            Ok(Ok(Some(None))) => {}
+            Ok(Ok(Some(applied))) => {
+                ready.insert(name, applied);
+            }
             _ => left_out.push(name),
         }
     }
-    (dirs, left_out)
+    (ready, left_out)
 }
 
 /// Fetch `profile`'s plugin list and keep `plugins.json` in step: a list
diff --git a/src/tui/plugins.rs b/src/tui/plugins.rs
index 2428a19..5dd157d 100644
--- a/src/tui/plugins.rs
+++ b/src/tui/plugins.rs
@@ -1,21 +1,33 @@
 //! Proxy plugins on the client: each profile's list (from the proxy or its
-//! cache in `plugins.json`), what each connected host answered for each
-//! bundle, and what a spawn gets from them.
+//! cache in `plugins.json`), each plugin's manifest (fetched here, kept in
+//! memory), what each connected host answered for it (`Applied`), and what
+//! a spawn gets from them ([`compose`]).
 //!
-//! Pure, like `app.rs`: fetching a list, writing the cache and asking a host
-//! for a bundle are [`Effect`]s that `tui/mod.rs` runs. The registry lists
-//! the plugins (`Source::Plugin`) so the extensions screen shows and toggles
-//! them; turning one off is saved as `[extensions] <name> = false` and
-//! applies to new sessions only.
+//! Pure, like `app.rs`: fetching a list or a manifest, writing the cache
+//! and asking a host for a manifest are [`Effect`]s that `tui/mod.rs`
+//! runs. The registry lists the plugins (`Source::Plugin`) so the
+//! extensions screen shows and toggles them; turning one off is saved as
+//! `[extensions] <name> = false` and applies to new sessions only. A host
+//! is asked on connect, when the list's hash changes, on `r`, and every
+//! 30 s while it still installs.
 
 use std::collections::{BTreeMap, BTreeSet, HashMap};
 
-use crate::proto::Msg;
+use crate::config::PluginChoices;
+use crate::plugins::manifest::Manifest;
+use crate::proto::{Applied, Msg, MAX_PLUGIN_INSTRUCTIONS};
+use crate::proxy::env::merge;
 use crate::proxy::plugins::{Plugin, PluginList};
 
-use super::app::{App, Effect, ReplyTo};
+use super::app::{App, Effect, PendingManifest, ReplyTo};
 use super::extensions::Source;
 
+/// `--agents`, `--mcp-config` (the daemon drops them for a claude without).
+const AGENTS: &str = "--agents";
+const MCP_CONFIG: &str = "--mcp-config";
+/// The reason the daemon gives a plugin whose install is under way.
+const INSTALLING: &str = "installing";
+
 /// How often every profile's list is fetched again.
 pub const REFRESH_SECS: u64 = 15 * 60;
 
@@ -42,32 +54,151 @@ pub enum PluginsFetch {
     Unreachable,
 }
 
-/// What a host answered for one plugin's bundle.
+/// A plugin's manifest on this client: fetched by the client itself (at
+/// the list's hash), kept in memory only.
 #[derive(Debug, Clone, PartialEq)]
-pub enum BundleState {
+pub enum ManifestState {
+    Fetching,
+    Ready(Box<Manifest>),
+    /// Refused or unreachable: nothing is asked of any host for it.
+    Failed(String),
+}
+
+/// What a host answered for one plugin's manifest.
+#[derive(Debug, Clone, PartialEq)]
+pub enum ApplyState {
     /// Asked; no answer yet.
     Pending,
-    /// The zip's path on that host.
-    Ready(String),
-    /// Claude Code there has the plugin installed: no bundle needed.
-    Installed,
+    /// What the host made of it (kept while it is asked again).
+    Ready(Box<Applied>),
     Failed(String),
-    /// The daemon does not know `Bundle`.
+    /// The daemon does not know `Manifest`.
     TooOld,
 }
 
+/// One host's state for one plugin's manifest.
+#[derive(Debug, Clone, PartialEq)]
+pub struct Apply {
+    /// The manifest hash asked for.
+    pub sha: String,
+    pub state: ApplyState,
+    /// When it was last asked, and first asked at this hash (Unix seconds).
+    pub asked: u64,
+    pub since: u64,
+}
+
 /// The client's plugin state.
 #[derive(Debug, Default)]
 pub struct Plugins {
     /// `[extensions]` of config.toml, kept so a plugin that appears later
     /// gets its saved choice (and a toggle updates it).
     pub config: BTreeMap<String, bool>,
+    /// `[plugins.<name>]` of config.toml: the member's manifest choices.
+    pub choices: BTreeMap<String, PluginChoices>,
     /// Per profile: its list (`None` once signed out) and how it was fetched.
     pub lists: BTreeMap<String, (PluginList, ListState)>,
-    /// The hosts whose daemon takes `Bundle` requests now.
+    /// The hosts whose daemon takes `Manifest` requests now.
     pub hosts: BTreeSet<String>,
-    /// Per `(host, plugin)`: the hash asked for and the answer.
-    pub bundles: HashMap<(String, String), (String, BundleState)>,
+    /// Per plugin: the manifest hash fetched, and the manifest.
+    pub manifests: HashMap<String, (String, ManifestState)>,
+    /// Per `(host, plugin)`: what that host answered.
+    pub applies: HashMap<(String, String), Apply>,
+}
+
+/// What one enabled plugin brings to a spawn on a host.
+#[derive(Debug, Clone, PartialEq)]
+pub struct PluginPart {
+    pub name: String,
+    /// The host's `Applied`, once it answered.
+    pub applied: Option<Applied>,
+    /// The plugin's proxy env: its secrets.
+    pub secrets: Vec<(String, String)>,
+}
+
+/// The plugins' share of a spawn ([`compose`]).
+#[derive(Debug, Clone, Default, PartialEq)]
+pub struct Composed {
+    /// `--agents <json>` then `--mcp-config <json>`, per plugin.
+    pub args: Vec<String>,
+    pub env: Vec<(String, String)>,
+    pub instructions: String,
+    pub notices: Vec<String>,
+}
+
+/// At most this many plugins go on one spawn (their flags are ≤ 96 KiB
+/// each; a frame is 1 MiB).
+pub const MAX_SPAWN_PLUGINS: usize = 8;
+
+/// While a host still installs a manifest's plugins it is asked again this
+/// often (seconds), for at most [`REASK_FOR`] after the first ask.
+pub const REASK_EVERY: u64 = 30;
+pub const REASK_FOR: u64 = 10 * 60;
+
+/// The plugins' share of a spawn, from `parts` in registry order: for each
+/// plugin the host answered for (at most [`MAX_SPAWN_PLUGINS`]), its
+/// `--agents` then its `--mcp-config`, its `[env]` and its instructions;
+/// every plugin's secrets. Env layers: the manifests' `[env]`, then the
+/// secrets, a later layer replacing an earlier one's key and the first
+/// plugin winning within a layer. The instructions are joined by a blank
+/// line, cut at the last whole one that fits 8 KiB.
+pub fn compose(parts: &[PluginPart]) -> Composed {
+    let mut c = Composed::default();
+    let mut manifest_env: Vec<(String, String)> = Vec::new();
+    let mut instructions: Vec<&str> = Vec::new();
+    let mut size = 0;
+    let mut cut: Vec<&str> = Vec::new();
+    let mut ready = 0;
+    for part in parts {
+        let Some(a) = &part.applied else { continue };
+        if ready == MAX_SPAWN_PLUGINS {
+            c.notices
+                .push(format!("plugin {} left out: too many plugins", part.name));
+            continue;
+        }
+        ready += 1;
+        if let Some(agents) = &a.agents {
+            c.args.push(AGENTS.to_owned());
+            c.args.push(agents.clone());
+        }
+        if let Some(mcp) = &a.mcp_config {
+            c.args.push(MCP_CONFIG.to_owned());
+            c.args.push(mcp.clone());
+        }
+        manifest_env.extend(a.env.iter().cloned());
+        let text = a.instructions.trim();
+        if text.is_empty() {
+            continue;
+        }
+        let sep = if instructions.is_empty() { 0 } else { 2 };
+        if !cut.is_empty() || size + sep + text.len() > MAX_PLUGIN_INSTRUCTIONS {
+            cut.push(&part.name);
+            continue;
+        }
+        size += sep + text.len();
+        instructions.push(text);
+    }
+    if !cut.is_empty() {
+        c.notices.push(format!(
+            "plugin instructions over 8 KiB: {} left out",
+            cut.join(", ")
+        ));
+    }
+    let secrets: Vec<(String, String)> = parts
+        .iter()
+        .flat_map(|p| p.secrets.iter().cloned())
+        .collect();
+    c.env = merge(&manifest_env, merge(&secrets, Vec::new()));
+    c.instructions = instructions.join("\n\n");
+    c
+}
+
+/// `msg` (a `Spawn` or `Respawn`) with the plugins' share: their flags
+/// after its args (a `Spawn`'s; a `Respawn` keeps the journaled ones),
+/// their env and their instructions.
+pub fn with_plugins(msg: Msg, c: Composed) -> Msg {
+    msg.with_plugin_args(&c.args)
+        .with_instructions(c.instructions)
+        .with_env(c.env)
 }
 
 impl App {
@@ -100,17 +231,16 @@ impl App {
             .collect()
     }
 
-    /// The env a session on `profile` gets from its enabled plugins.
-    pub(super) fn plugin_env(&self, profile: &str) -> Vec<(String, String)> {
-        let plugins = self.enabled_plugins(profile);
-        plugins
-            .into_iter()
-            .flat_map(|p| p.env.iter().cloned())
+    /// Every admitted plugin of every list (on or off).
+    fn admitted_plugins(&self) -> Vec<Plugin> {
+        let lists = self.plugins.lists.values();
+        lists
+            .flat_map(|(list, _)| list.admitted().into_iter().cloned())
             .collect()
     }
 
-    /// A list arrived (or failed): keep it, list it, and ask the connected
-    /// hosts for the bundles it changed.
+    /// A list arrived (or failed): keep it, list it, fetch the manifests it
+    /// changed and ask the connected hosts for them.
     pub fn on_plugins(&mut self, profile: &str, fetch: PluginsFetch) {
         match fetch {
             PluginsFetch::Listed(list) => {
@@ -131,10 +261,8 @@ impl App {
             }
         }
         self.list_plugins();
-        let hosts: Vec<String> = self.plugins.hosts.iter().cloned().collect();
-        for host in hosts {
-            self.request_bundles(&host);
-        }
+        self.fetch_manifests(false);
+        self.request_all();
         self.redraw = true;
     }
 
@@ -144,46 +272,185 @@ impl App {
         self.extensions.set_plugins(&lists, &self.plugins.config);
     }
 
-    /// `host`'s daemon is connected: ask it for every enabled bundle.
+    /// Fetch the manifest of each admitted plugin whose hash is new (every
+    /// one when `all`).
+    fn fetch_manifests(&mut self, all: bool) {
+        for p in self.admitted_plugins() {
+            let known = self.plugins.manifests.get(&p.name);
+            if !all && known.is_some_and(|(sha, _)| *sha == p.bundle_sha256) {
+                continue;
+            }
+            let entry = (p.bundle_sha256.clone(), ManifestState::Fetching);
+            self.plugins.manifests.insert(p.name.clone(), entry);
+            self.emit(Effect::FetchManifest {
+                name: p.name,
+                url: p.bundle_url,
+                sha256: p.bundle_sha256,
+            });
+        }
+    }
+
+    /// Plugin `name`'s manifest at hash `sha256` was fetched (and parsed),
+    /// or not.
+    pub fn on_manifest(&mut self, name: &str, sha256: &str, result: Result<Manifest, String>) {
+        match self.plugins.manifests.get_mut(name) {
+            Some((sha, state)) if sha == sha256 => {
+                *state = match result {
+                    Ok(m) => ManifestState::Ready(Box::new(m)),
+                    Err(why) => ManifestState::Failed(why),
+                };
+            }
+            // A hash since replaced.
+            _ => return,
+        }
+        self.request_all();
+        self.redraw = true;
+    }
+
+    /// `host`'s daemon is connected: ask it for every enabled manifest.
     pub fn on_plugin_host(&mut self, host: &str) {
         self.plugins.hosts.insert(host.to_owned());
         // A new connection may be a new daemon: ask again.
-        self.plugins.bundles.retain(|(h, _), _| h != host);
-        self.request_bundles(host);
+        self.plugins.applies.retain(|(h, _), _| h != host);
+        self.request_manifests(host, None);
     }
 
     /// `host`'s daemon is gone: its answers no longer hold.
     pub(super) fn plugin_host_gone(&mut self, host: &str) {
         self.plugins.hosts.remove(host);
-        self.plugins.bundles.retain(|(h, _), _| h != host);
+        self.plugins.applies.retain(|(h, _), _| h != host);
+    }
+
+    /// [`App::request_manifests`] on every connected host.
+    pub(super) fn request_all(&mut self) {
+        let hosts: Vec<String> = self.plugins.hosts.iter().cloned().collect();
+        for host in hosts {
+            self.request_manifests(&host, None);
+        }
+    }
+
+    /// Ask every connected host for plugin `name`'s manifest again, even
+    /// where it answered (its choices changed).
+    pub(super) fn request_again(&mut self, name: &str) {
+        let hosts: Vec<String> = self.plugins.hosts.iter().cloned().collect();
+        for host in hosts {
+            self.request_manifests(&host, Some(name));
+        }
+    }
+
+    /// Whether a host may be asked for `p`'s manifest now: the client has
+    /// it, good, at the list's hash.
+    fn requestable(&self, p: &Plugin) -> bool {
+        matches!(
+            self.plugins.manifests.get(&p.name),
+            Some((sha, ManifestState::Ready(_))) if *sha == p.bundle_sha256
+        )
     }
 
-    /// Ask `host` for each enabled bundle it has not answered for at its
-    /// current hash.
-    pub(super) fn request_bundles(&mut self, host: &str) {
+    /// Ask `host` for each enabled plugin's manifest it has not answered for
+    /// at its current hash (a failure is asked again); `again`: that
+    /// plugin's whatever the answer.
+    pub(super) fn request_manifests(&mut self, host: &str, again: Option<&str>) {
         let wanted: Vec<Plugin> = self.all_enabled_plugins().into_iter().cloned().collect();
         for p in wanted {
+            if !self.requestable(&p) {
+                continue;
+            }
             let key = (host.to_owned(), p.name.clone());
-            if self.plugins.bundles.get(&key).is_some_and(|(sha, state)| {
-                *sha == p.bundle_sha256 && !matches!(state, BundleState::Failed(_))
-            }) {
+            let answered = self.plugins.applies.get(&key).is_some_and(|a| {
+                a.sha == p.bundle_sha256 && !matches!(a.state, ApplyState::Failed(_))
+            });
+            if answered && again != Some(p.name.as_str()) {
                 continue;
             }
-            let sha = p.bundle_sha256.clone();
-            self.plugins
-                .bundles
-                .insert(key, (sha.clone(), BundleState::Pending));
-            let to = ReplyTo::Bundle {
-                host: host.to_owned(),
-                name: p.name.clone(),
-                sha256: sha.clone(),
-            };
-            self.request(host, bundle_msg(&p), to);
+            self.ask(host, &p);
         }
     }
 
-    /// `host`'s answer for plugin `name` at hash `sha256`.
-    pub(super) fn on_bundled(
+    /// Send `host` the `Manifest` request for `p`. A `Ready` answer at the
+    /// same hash stays until the new one comes.
+    fn ask(&mut self, host: &str, p: &Plugin) {
+        let key = (host.to_owned(), p.name.clone());
+        let now = self.now;
+        let apply = match self.plugins.applies.remove(&key) {
+            Some(a) if a.sha == p.bundle_sha256 && matches!(a.state, ApplyState::Ready(_)) => {
+                Apply { asked: now, ..a }
+            }
+            Some(a) if a.sha == p.bundle_sha256 => Apply {
+                state: ApplyState::Pending,
+                asked: now,
+                ..a
+            },
+            _ => Apply {
+                sha: p.bundle_sha256.clone(),
+                state: ApplyState::Pending,
+                asked: now,
+                since: now,
+            },
+        };
+        self.plugins.applies.insert(key, apply);
+        let to = ReplyTo::Manifest {
+            host: host.to_owned(),
+            name: p.name.clone(),
+            sha256: p.bundle_sha256.clone(),
+        };
+        let msg = self.manifest_msg(p);
+        self.request(host, msg, to);
+    }
+
+    /// The `Manifest` request for `p`: the names of its proxy env (never the
+    /// values) and the ids the member turned off.
+    pub(super) fn manifest_msg(&self, p: &Plugin) -> Msg {
+        let disabled = self
+            .plugins
+            .choices
+            .get(&p.name)
+            .map(|c| c.disabled.clone())
+            .unwrap_or_default();
+        Msg::Manifest {
+            name: p.name.clone(),
+            url: p.bundle_url.clone(),
+            sha256: p.bundle_sha256.clone(),
+            secrets: p.env.iter().map(|(k, _)| k.clone()).collect(),
+            disabled,
+        }
+    }
+
+    /// Ask again, every [`REASK_EVERY`] s, a host whose answer still lists
+    /// an install under way, for [`REASK_FOR`] after the first ask.
+    pub(super) fn reask_installing(&mut self) {
+        let now = self.now;
+        let due: Vec<(String, String)> = self
+            .plugins
+            .applies
+            .iter()
+            .filter(|(_, a)| {
+                let installing = match &a.state {
+                    ApplyState::Ready(applied) => {
+                        applied.skipped.iter().any(|(_, why)| why == INSTALLING)
+                    }
+                    _ => false,
+                };
+                installing
+                    && now.saturating_sub(a.asked) >= REASK_EVERY
+                    && now.saturating_sub(a.since) <= REASK_FOR
+            })
+            .map(|(key, _)| key.clone())
+            .collect();
+        for (host, name) in due {
+            let plugin = self
+                .all_enabled_plugins()
+                .into_iter()
+                .find(|p| p.name == name)
+                .cloned();
+            if let Some(p) = plugin.filter(|p| self.requestable(p)) {
+                self.ask(&host, &p);
+            }
+        }
+    }
+
+    /// `host`'s answer for plugin `name`'s manifest at hash `sha256`.
+    pub(super) fn on_applied(
         &mut self,
         host: &str,
         name: &str,
@@ -192,42 +459,71 @@ impl App {
     ) {
         let key = (host.to_owned(), name.to_owned());
         // An answer for a hash since replaced, or a host since gone.
-        if self
-            .plugins
-            .bundles
-            .get(&key)
-            .is_none_or(|(sha, _)| sha != sha256)
-        {
+        let Some(apply) = self.plugins.applies.get_mut(&key) else {
+            return;
+        };
+        if apply.sha != sha256 {
             return;
         }
-        let state = match reply {
-            Ok(Msg::Bundled {
-                installed: true, ..
-            }) => BundleState::Installed,
-            Ok(Msg::Bundled {
-                path: Some(path), ..
-            }) => BundleState::Ready(path),
-            Ok(other) => BundleState::Failed(format!("unexpected reply {other:?}")),
-            Err(e) if App::older_daemon_notice(host, &e).is_some() => BundleState::TooOld,
+        apply.state = match reply {
+            Ok(Msg::Applied(applied)) => ApplyState::Ready(Box::new(applied)),
+            Ok(other) => ApplyState::Failed(format!("unexpected reply {other:?}")),
+            Err(e) if App::older_daemon_notice(host, &e).is_some() => ApplyState::TooOld,
             Err(e) => {
                 let why = e.to_string();
-                let prefix = format!("bundle {name}: ");
-                BundleState::Failed(why.strip_prefix(&prefix).unwrap_or(&why).to_owned())
+                let prefix = format!("manifest {name}: ");
+                ApplyState::Failed(why.strip_prefix(&prefix).unwrap_or(&why).to_owned())
             }
         };
-        self.plugins.bundles.insert(key, (sha256.to_owned(), state));
         self.redraw = true;
     }
 
-    /// The bundle state of plugin `name` on `host`, if asked.
-    pub fn bundle_state(&self, host: &str, name: &str) -> Option<&BundleState> {
+    /// The state of plugin `name`'s manifest on `host`, if asked.
+    pub fn apply_state(&self, host: &str, name: &str) -> Option<&ApplyState> {
         let key = (host.to_owned(), name.to_owned());
-        self.plugins.bundles.get(&key).map(|(_, state)| state)
+        self.plugins.applies.get(&key).map(|a| &a.state)
+    }
+
+    /// What `profile`'s enabled plugins bring to a spawn on `host`, in
+    /// registry order, and the `Manifest` requests of those `host` has not
+    /// answered at all (the spawn waits for them). A plugin whose manifest
+    /// the client lacks, or whose host failed, brings its secrets only.
+    pub(super) fn plugin_parts(
+        &self,
+        host: &str,
+        profile: &str,
+    ) -> (Vec<PluginPart>, Vec<PendingManifest>) {
+        let mut parts = Vec::new();
+        let mut pending = Vec::new();
+        for p in self.enabled_plugins(profile) {
+            let applied = match self.apply_state(host, &p.name) {
+                Some(ApplyState::Ready(a)) => Some(Applied::clone(a)),
+                Some(ApplyState::Pending) | None if self.requestable(p) => {
+                    pending.push(PendingManifest {
+                        name: p.name.clone(),
+                        msg: self.manifest_msg(p),
+                        to: ReplyTo::Manifest {
+                            host: host.to_owned(),
+                            name: p.name.clone(),
+                            sha256: p.bundle_sha256.clone(),
+                        },
+                    });
+                    None
+                }
+                _ => None,
+            };
+            parts.push(PluginPart {
+                name: p.name.clone(),
+                applied,
+                secrets: p.env.clone(),
+            });
+        }
+        (parts, pending)
     }
 
     /// What the extensions screen says about plugin `name` of `profile`
-    /// (beyond its description): one line per host that was asked, and one
-    /// for the list.
+    /// (beyond its version): its manifest, one line per host that was
+    /// asked, and the list's state.
     pub(super) fn plugin_details(
         &self,
         name: &str,
@@ -239,21 +535,37 @@ impl App {
             lines.push(format!("refused: {why}"));
             return lines;
         }
+        let manifest = match self.plugins.manifests.get(name) {
+            Some((_, ManifestState::Ready(m))) => Some(m),
+            Some((_, ManifestState::Fetching)) => {
+                lines.push("manifest: fetching".to_owned());
+                None
+            }
+            Some((_, ManifestState::Failed(why))) => {
+                lines.push(format!("manifest: {why}"));
+                None
+            }
+            None => None,
+        };
+        if let Some(m) = manifest.filter(|m| !m.description.trim().is_empty()) {
+            lines.push(m.description.clone());
+        }
         let mut hosts: Vec<&String> = self.plugins.hosts.iter().collect();
         hosts.sort_by_key(|h| (h.as_str() != "local", h.as_str()));
         for host in hosts {
-            let Some(state) = self.bundle_state(host, name) else {
-                continue;
-            };
-            let text = match state {
-                BundleState::Pending => "downloading".to_owned(),
-                BundleState::Ready(_) => "ready".to_owned(),
-                BundleState::Installed => "installed in Claude Code (bundle skipped)".to_owned(),
-                BundleState::Failed(why) if why.starts_with("download failed") => why.clone(),
-                BundleState::Failed(why) => format!("download failed: {why}"),
-                BundleState::TooOld => "daemon too old".to_owned(),
-            };
-            lines.push(format!("{host}: {text}"));
+            match self.apply_state(host, name) {
+                None => {}
+                Some(ApplyState::Pending) => lines.push(format!("{host}: applying")),
+                Some(ApplyState::Ready(a)) => lines.extend(ready_lines(host, a)),
+                Some(ApplyState::Failed(why)) => lines.push(format!("{host}: failed: {why}")),
+                Some(ApplyState::TooOld) => lines.push(format!("{host}: daemon too old")),
+            }
+        }
+        if manifest.is_some_and(|m| !m.plugins.is_empty()) {
+            lines.push(
+                "installed plugins stay until you remove them: claude plugin uninstall <id>"
+                    .to_owned(),
+            );
         }
         if let Some((list, ListState::Unreachable)) = self.plugins.lists.get(profile) {
             let age = super::fmt::fmt_age(self.now.saturating_sub(list.fetched_at));
@@ -262,9 +574,19 @@ impl App {
         lines
     }
 
-    /// `r` on the extensions screen: fetch every known profile's list again.
+    /// `r` on the extensions screen: ask every host again for every
+    /// enabled manifest, and fetch every list and manifest again.
     pub(super) fn refresh_plugins(&mut self) {
         self.fetch_plugin_lists();
+        let names: Vec<String> = self
+            .all_enabled_plugins()
+            .iter()
+            .map(|p| p.name.clone())
+            .collect();
+        for name in names {
+            self.request_again(&name);
+        }
+        self.fetch_manifests(true);
         self.notify("refreshing plugins…");
     }
 
@@ -278,9 +600,16 @@ impl App {
         }
     }
 
-    /// The lists read from `plugins.json` at start, and `[extensions]`.
-    pub fn load_plugins(&mut self, cached: Vec<PluginList>, config: BTreeMap<String, bool>) {
+    /// The lists read from `plugins.json` at start, `[extensions]` and
+    /// `[plugins]`; their manifests are fetched.
+    pub fn load_plugins(
+        &mut self,
+        cached: Vec<PluginList>,
+        config: BTreeMap<String, bool>,
+        choices: BTreeMap<String, PluginChoices>,
+    ) {
         self.plugins.config = config;
+        self.plugins.choices = choices;
         for list in cached {
             let profile = list.profile.clone();
             self.plugins
@@ -288,6 +617,7 @@ impl App {
                 .insert(profile, (list, ListState::Cached));
         }
         self.list_plugins();
+        self.fetch_manifests(false);
     }
 
     /// The profiles sessions use: the one new sessions get, and every open
@@ -305,14 +635,28 @@ impl App {
     }
 }
 
-/// The `Bundle` request for `p`.
-pub fn bundle_msg(p: &Plugin) -> Msg {
-    Msg::Bundle {
-        name: p.name.clone(),
-        url: p.bundle_url.clone(),
-        sha256: p.bundle_sha256.clone(),
-        installed_id: p.installed_id.clone(),
+/// The extensions screen's lines for `host`'s answer `a`: the counts, one
+/// line per skipped id that is not installing, and the flags its claude
+/// lacks.
+fn ready_lines(host: &str, a: &Applied) -> Vec<String> {
+    let installing = a
+        .skipped
+        .iter()
+        .filter(|(_, why)| why == INSTALLING)
+        .count();
+    let skipped = a.skipped.len() - installing;
+    let mut lines = vec![format!(
+        "{host}: ready · {} installed, {} present, {installing} installing, {skipped} skipped",
+        a.installed.len(),
+        a.present.len()
+    )];
+    for (id, why) in a.skipped.iter().filter(|(_, why)| why != INSTALLING) {
+        lines.push(format!("  {id}: {why}"));
+    }
+    if !a.dropped.is_empty() {
+        lines.push(format!("{host}: claude too old: {}", a.dropped.join(", ")));
     }
+    lines
 }
 
 #[cfg(test)]
diff --git a/src/tui/sessions.rs b/src/tui/sessions.rs
index e5f0e92..0daf231 100644
--- a/src/tui/sessions.rs
+++ b/src/tui/sessions.rs
@@ -12,11 +12,11 @@ use crate::proto::{
 };
 use crate::term::screen::Screen;
 
-use super::app::{App, Effect, Mode, PendingBundle, ReplyTo, PROJECTS_LIMIT};
+use super::app::{App, Effect, Mode, PendingManifest, ReplyTo, PROJECTS_LIMIT};
 use super::confirm::{Choice, ConfirmAction, ConfirmPrompt};
 use super::interaction::Modal;
 use super::latex::Formula;
-use super::plugins::{bundle_msg, BundleState};
+use super::plugins::{compose, with_plugins, PluginPart};
 use super::state::{self, ClientState, KillTombstone, SavedSession};
 use super::wizard::{self, Wizard};
 
@@ -425,10 +425,10 @@ impl App {
             self.request(host, Msg::SpawnShell(shell), ReplyTo::Spawned(id));
             return Ok(());
         }
-        // The profile's plugins: their env now, their bundles' paths when
-        // this host has them (else the spawn waits for them).
-        let (dirs, bundles) = match proxy {
-            Some(profile) => self.plugin_bundles(host, profile),
+        // The profile's plugins: their share now when this host answered
+        // for each, else once it has (the spawn waits for it).
+        let (plugins, pending) = match proxy {
+            Some(profile) => self.plugin_parts(host, profile),
             None => (Vec::new(), Vec::new()),
         };
         let spec = SpawnSpec {
@@ -436,46 +436,32 @@ impl App {
             cwd,
             name,
             args,
-            env: proxy.map(|p| self.plugin_env(p)).unwrap_or_default(),
+            env: Vec::new(),
             rows,
             cols,
             // `--plain`'s session is not one for a manager to show.
             ephemeral: self.mode == Mode::Plain,
             extensions: Some(self.extensions.session_extensions()),
         };
-        let msg = Msg::Spawn(spec).with_plugin_dirs(&dirs);
-        self.send_with_proxy(host, msg, proxy, first_party, ReplyTo::Spawned(id), bundles)
-    }
-
-    /// The bundles of `profile`'s enabled plugins on `host`: the paths of
-    /// those ready there, and those it has not answered for (installed in
-    /// Claude Code, failed or too old: neither).
-    fn plugin_bundles(&self, host: &str, profile: &str) -> (Vec<String>, Vec<PendingBundle>) {
-        let mut dirs = Vec::new();
-        let mut pending = Vec::new();
-        for p in self.enabled_plugins(profile) {
-            match self.bundle_state(host, &p.name) {
-                Some(BundleState::Ready(path)) => dirs.push(path.clone()),
-                Some(BundleState::Pending) | None => pending.push(PendingBundle {
-                    name: p.name.clone(),
-                    msg: bundle_msg(p),
-                    to: ReplyTo::Bundle {
-                        host: host.to_owned(),
-                        name: p.name.clone(),
-                        sha256: p.bundle_sha256.clone(),
-                    },
-                }),
-                Some(_) => {}
-            }
-        }
-        (dirs, pending)
+        let to = ReplyTo::Spawned(id);
+        self.send_with_proxy(
+            host,
+            Msg::Spawn(spec),
+            proxy,
+            first_party,
+            to,
+            plugins,
+            pending,
+        )
     }
 
-    /// Send `msg` (a `Spawn` or `Respawn` carrying only the plugin env) to
-    /// `host` with the env of `proxy` added. A cold proxy-config cache, or
-    /// `bundles` still to come, go through the effect runner, which fetches
-    /// the config and waits for the bundles first and answers with
+    /// Send `msg` (a `Spawn` or `Respawn` without the plugins' share or any
+    /// env) to `host` with the share of `plugins` ([`compose`]) and the env
+    /// of `proxy` added. A cold proxy-config cache, or `pending` manifests
+    /// still to come, go through the effect runner, which fetches the
+    /// config and waits for them first and answers with
     /// [`ReplyTo::deferred`]. Fails when the profile cannot be resolved.
+    #[allow(clippy::too_many_arguments)]
     fn send_with_proxy(
         &mut self,
         host: &str,
@@ -483,13 +469,14 @@ impl App {
         proxy: Option<&str>,
         first_party: bool,
         to: ReplyTo,
-        bundles: Vec<PendingBundle>,
+        plugins: Vec<PluginPart>,
+        pending: Vec<PendingManifest>,
     ) -> Result<(), String> {
         match proxy {
-            Some(name) if self.proxy_config_cached(name).is_none() || !bundles.is_empty() => {
-                if !bundles.is_empty() {
-                    let names: Vec<&str> = bundles.iter().map(|b| b.name.as_str()).collect();
-                    self.notify(format!("fetching plugin {}…", names.join(", ")));
+            Some(name) if self.proxy_config_cached(name).is_none() || !pending.is_empty() => {
+                if !pending.is_empty() {
+                    let names: Vec<&str> = pending.iter().map(|b| b.name.as_str()).collect();
+                    self.notify(format!("applying plugin {}…", names.join(", ")));
                 }
                 self.emit(Effect::SpawnWithProxy {
                     host: host.to_owned(),
@@ -497,11 +484,17 @@ impl App {
                     proxy_name: name.to_owned(),
                     first_party,
                     to: to.deferred(),
-                    bundles,
+                    plugins,
+                    pending,
                 });
             }
             _ => {
-                let msg = msg.merge_env(self.proxy_env_for(proxy, first_party)?);
+                let profile_env = self.proxy_env_for(proxy, first_party)?;
+                let composed = compose(&plugins);
+                for notice in &composed.notices {
+                    self.notify(notice.clone());
+                }
+                let msg = with_plugins(msg, composed).merge_env(profile_env);
                 self.request(host, msg, to);
             }
         }
@@ -584,14 +577,16 @@ impl App {
         // A run's `--first-party` applies to the sessions it restarts too.
         let first_party = (stored || self.first_party) && proxy.is_some();
         let (rows, cols) = self.pane_size();
+        // The plugins' env and instructions now (the journaled args keep
+        // their flags); a reset waits for no manifest.
+        let plugins = match proxy.as_deref() {
+            Some(profile) => self.plugin_parts(&host, profile).0,
+            None => Vec::new(),
+        };
         let msg = Msg::Respawn(RespawnSpec {
             id,
             fresh,
-            // The plugin env now; the journaled args keep their bundles.
-            env: proxy
-                .as_deref()
-                .map(|p| self.plugin_env(p))
-                .unwrap_or_default(),
+            env: Vec::new(),
             rows,
             cols,
             extensions: Some(self.extensions.session_extensions()),
@@ -602,6 +597,7 @@ impl App {
             proxy.as_deref(),
             first_party,
             ReplyTo::Respawned(id),
+            plugins,
             Vec::new(),
         ) {
             Ok(()) => {
`````
Run: `umask 022; cargo test --locked --bin claudio; cargo test --test manager -- --test-threads=1`
Expected: `test result: ok. 1082 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`, then `test result: ok. 50 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` (manager: the two zip-era tests are gone). Dead-code warnings for the config items of Task 5 and `manifest::{Section, Item, items, ids, heading}` until Task 7.

Step 3: commit
`/usr/bin/git commit -m "feat(tui): fetch plugin manifests and spawn with what each host applied" -- src/tui/plugins.rs src/tui/app.rs src/tui/interaction.rs src/tui/sessions.rs src/tui/mod.rs src/tui/extensions_view.rs src/proto.rs src/proxy/plugins.rs tests/manager.rs`

Interfaces this task provides to later tasks: `tui::plugins::{ManifestState, ApplyState, Apply, PluginPart, Composed, compose, with_plugins, MAX_SPAWN_PLUGINS, REASK_EVERY, REASK_FOR}`, `Plugins.{choices, manifests, applies}`; `App::{on_manifest, request_all, request_again, request_manifests, manifest_msg, reask_installing, on_applied, apply_state, plugin_parts, load_plugins(cached, config, choices)}`; `Effect::FetchManifest`, `Effect::SpawnWithProxy { plugins, pending }`, `PendingManifest`, `ReplyTo::Manifest`; `Msg::{with_plugin_args, with_instructions}` (`with_plugin_dirs` is gone); `proxy::plugins::fetch_manifest`; `tui::BUNDLE_WAIT` is `pub(super)`; `HostEvent::Manifest`.

### Task 7: plugin setup screen, saved choices and `--plain` deferral
Model: standard   Repo: /volumes/repos/claudio   Files: src/tui/plugin_setup.rs (new), src/tui/ui/plugin_setup.rs (new), src/tui/confirm.rs, src/tui/app.rs, src/tui/interaction.rs, src/tui/ui/mod.rs, src/tui/mod.rs, src/tui/plugins.rs, src/tui/extensions_view.rs, src/tui/plain.rs, tests/manager.rs
Goal: the setup screen of §2 (AC3):
- It opens on first sight and from the extensions screen (Enter on a plugin row; Space still
  toggles). Keys: Up/Down, j/k, Space, `a`, `n`, Enter, Esc.
- It swallows every key and waits its turn in the confirm queue.
- Esc on first sight defers for the run.
- Enter saves `[plugins.<name>]` and asks every host again. Nothing is sent before.
- New ids get a notice.
- `--plain` waits for the profile's new manifests (≤ 20 s) and their screens.
- The details say `setup pending (Enter)` or `<n> items disabled (Enter to change)`.
Patch: .delivery/2026-11-06-plugin-manifest/patches/task-7.patch

Step 1: write the failing test
`````diff
diff --git a/src/tui/extensions_view.rs b/src/tui/extensions_view.rs
index bd0f942..44ad170 100644
--- a/src/tui/extensions_view.rs
+++ b/src/tui/extensions_view.rs
@@ -551,8 +563,16 @@ mod tests {
         let src = "format = 1\nname = \"kb\"\nversion = \"1.4.0\"\ndescription = \"KB\"";
         let manifest = crate::plugins::manifest::Manifest::parse(src).unwrap();
         let sha = "a".repeat(64);
+        app.plugins.choices.insert("kb".into(), Default::default());
         app.on_manifest("kb", &sha, Ok(manifest));
-        assert_eq!(details(&app, 5)[2..], ["KB", "local: applying"]);
+        assert_eq!(
+            details(&app, 5)[2..],
+            [
+                "KB",
+                "0 items disabled (Enter to change)",
+                "local: applying"
+            ]
+        );
         let ready = Msg::Applied(crate::proto::Applied {
             name: "kb".into(),
             version: "1.4.0".into(),
@@ -565,7 +585,7 @@ mod tests {
         };
         app.on_reply(to.clone(), Ok(ready));
         assert_eq!(
-            details(&app, 5)[3],
+            details(&app, 5)[4],
             "local: ready · 0 installed, 0 present, 0 installing, 0 skipped"
         );
         app.on_reply(
@@ -575,7 +595,7 @@ mod tests {
             )),
         );
         assert_eq!(
-            details(&app, 5)[3],
+            details(&app, 5)[4],
             "local: failed: download failed: HTTP 404"
         );
         assert!(
diff --git a/src/tui/mod.rs b/src/tui/mod.rs
index 3dfafee..af1e21b 100644
--- a/src/tui/mod.rs
+++ b/src/tui/mod.rs
@@ -22,6 +22,7 @@ mod latex;
 mod leader;
 mod notifications;
 mod plain;
+mod plugin_setup;
 mod plugins;
 mod proxy_state;
 mod reader;
diff --git a/src/tui/plugin_setup.rs b/src/tui/plugin_setup.rs
new file mode 100644
index 0000000..a4917b6
--- /dev/null
+++ b/src/tui/plugin_setup.rs
@@ -0,0 +1,422 @@
+#[cfg(test)]
+mod tests {
+    use super::*;
+    use crate::proto::Msg;
+    use crate::proxy::plugins::{sample, PluginList};
+    use crate::tui::confirm::{Choice, ConfirmPrompt};
+    use crate::tui::plugins::PluginsFetch;
+    use crate::tui::test_support::{alt, app_with, press};
+
+    const TEAM: &str = r#"
+format = 1
+name = "team"
+version = "1.0.0"
+instructions = "Rules."
+[[marketplace]]
+name = "m"
+source = "github:o/r"
+[[plugin]]
+name = "a"
+marketplace = "m"
+[[mcp]]
+name = "kb"
+type = "sse"
+url = "https://kb.example.org"
+[[agent]]
+name = "reviewer"
+description = "Reviews"
+prompt = "Review."
+"#;
+
+    fn team() -> Manifest {
+        Manifest::parse(TEAM).unwrap()
+    }
+
+    fn other(name: &str) -> Manifest {
+        Manifest::parse(&format!(
+            "format = 1\nname = \"{name}\"\nversion = \"2\"\nenv = {{ X = \"y\" }}"
+        ))
+        .unwrap()
+    }
+
+    fn sha() -> String {
+        "a".repeat(64)
+    }
+
+    /// An app (no modal open) with `local` connected and profile `work`
+    /// serving `names`, whose manifests are being fetched.
+    fn app_serving(names: &[&str]) -> App {
+        let mut app = app_with(&[]);
+        app.modal = None;
+        app.on_plugin_host("local");
+        let list = PluginList {
+            profile: "work".into(),
+            fetched_at: 1,
+            plugins: names.iter().map(|n| sample(n)).collect(),
+        };
+        app.on_plugins("work", PluginsFetch::Listed(list));
+        app.take_effects();
+        app
+    }
+
+    fn setup(app: &App) -> &PluginSetup {
+        match &app.modal {
+            Some(Modal::PluginSetup(s)) => s,
+            other => panic!("no setup screen: {}", other.is_some()),
+        }
+    }
+
+    /// Wait out a first-sight screen's arming.
+    fn arm(app: &mut App) {
+        for _ in 0..ARM_TICKS {
+            app.confirm_tick();
+        }
+    }
+
+    /// The `Manifest` requests queued, as (host, name, disabled).
+    fn requests(app: &mut App) -> Vec<(String, String, Vec<String>)> {
+        app.take_effects()
+            .into_iter()
+            .filter_map(|e| match e {
+                Effect::Request {
+                    host,
+                    msg: Msg::Manifest { name, disabled, .. },
+                    ..
+                } => Some((host, name, disabled)),
+                _ => None,
+            })
+            .collect()
+    }
+
+    #[test]
+    fn keys_move_toggle_and_answer() {
+        let mut s = PluginSetup::new(&team(), &["mcp:kb".to_owned()], false);
+        let ids: Vec<&str> = s.items.iter().map(|i| i.id.as_str()).collect();
+        assert_eq!(
+            ids,
+            ["plugin:a@m", "mcp:kb", "agent:reviewer", "instructions"]
+        );
+        assert_eq!(s.off_ids(), ["mcp:kb"]);
+        let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
+        for code in [KeyCode::Down, KeyCode::Char('j'), KeyCode::Char('j')] {
+            assert_eq!(s.on_key(&k(code)), Outcome::Stay);
+        }
+        assert_eq!(s.selected, 3);
+        s.on_key(&k(KeyCode::Down));
+        assert_eq!(s.selected, 3, "the last item");
+        s.on_key(&k(KeyCode::Char('k')));
+        s.on_key(&k(KeyCode::Char(' ')));
+        assert_eq!(s.off_ids(), ["mcp:kb", "agent:reviewer"]);
+        s.on_key(&k(KeyCode::Char('a')));
+        assert!(s.off_ids().is_empty());
+        s.on_key(&k(KeyCode::Char('n')));
+        assert_eq!(s.off_ids().len(), 4);
+        s.on_key(&k(KeyCode::Up));
+        s.on_key(&k(KeyCode::Up));
+        s.on_key(&k(KeyCode::Up));
+        assert_eq!(s.selected, 0);
+        // A chord is no answer; Enter saves, Esc closes.
+        let alt_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT);
+        assert_eq!(s.on_key(&alt_enter), Outcome::Stay);
+        assert_eq!(s.on_key(&k(KeyCode::Enter)), Outcome::Save);
+        assert_eq!(s.on_key(&k(KeyCode::Esc)), Outcome::Close);
+        // On first sight, keys count only once armed.
+        let mut first = PluginSetup::new(&team(), &[], true);
+        assert_eq!(first.on_key(&k(KeyCode::Enter)), Outcome::Stay);
+        first.armed = 0;
+        assert_eq!(first.on_key(&k(KeyCode::Enter)), Outcome::Save);
+        assert_eq!(
+            first.hint(),
+            "Space toggle · a all on · n all off · Enter save · Esc later"
+        );
+    }
+
+    #[test]
+    fn first_sight_shows_every_item_and_nothing_is_asked_until_enter() {
+        let mut app = app_serving(&["team"]);
+        app.on_manifest("team", &sha(), Ok(team()));
+        let s = setup(&app);
+        assert!(s.first_sight && s.items.iter().all(|i| i.on));
+        assert!(
+            requests(&mut app).is_empty(),
+            "nothing before the setup closes"
+        );
+        // Keys are swallowed while it arms, Alt+n included.
+        app.on_terminal(press(KeyCode::Char(' ')));
+        app.on_terminal(alt('n'));
+        assert!(setup(&app).off_ids().is_empty());
+        assert!(matches!(app.modal, Some(Modal::PluginSetup(_))));
+        arm(&mut app);
+        app.on_terminal(press(KeyCode::Down));
+        app.on_terminal(press(KeyCode::Char(' ')));
+        app.on_terminal(press(KeyCode::Enter));
+        assert!(app.modal.is_none());
+        let effects = app.take_effects();
+        let saved = effects.iter().find_map(|e| match e {
+            Effect::SavePluginChoices { name, choices } => Some((name.clone(), choices.clone())),
+            _ => None,
+        });
+        let ids = team().ids();
+        assert_eq!(
+            saved,
+            Some((
+                "team".to_owned(),
+                PluginChoices {
+                    disabled: vec!["mcp:kb".into()],
+                    reviewed: "1.0.0".into(),
+                    seen: ids,
+                }
+            ))
+        );
+        let asked: Vec<&Effect> = effects
+            .iter()
+            .filter(|e| matches!(e, Effect::Request { msg: Msg::Manifest { disabled, .. }, .. } if disabled == &["mcp:kb"]))
+            .collect();
+        assert_eq!(asked.len(), 1, "{effects:?}");
+        assert_eq!(
+            app.setup_details("team").as_deref(),
+            Some("1 items disabled (Enter to change)")
+        );
+    }
+
+    #[test]
+    fn esc_on_first_sight_defers_for_the_run() {
+        let mut app = app_serving(&["team"]);
+        app.on_manifest("team", &sha(), Ok(team()));
+        arm(&mut app);
+        app.on_terminal(press(KeyCode::Esc));
+        assert!(app.modal.is_none());
+        let effects = app.take_effects();
+        assert!(
+            !effects
+                .iter()
+                .any(|e| matches!(e, Effect::SavePluginChoices { .. } | Effect::Request { .. })),
+            "{effects:?}"
+        );
+        assert_eq!(
+            app.setup_details("team").as_deref(),
+            Some("setup pending (Enter)")
+        );
+        // Fetched again in this run: no screen.
+        app.refresh_plugins();
+        app.on_manifest("team", &sha(), Ok(team()));
+        assert!(app.modal.is_none());
+        assert!(requests(&mut app).is_empty());
+    }
+
+    #[test]
+    fn setup_screens_wait_their_turn_with_the_prompts() {
+        let mut app = app_serving(&["team", "two"]);
+        app.on_manifest("team", &sha(), Ok(team()));
+        app.queue_confirm(ConfirmPrompt::new(
+            "Q",
+            "Update?",
+            vec![Choice::new('n', "no", None)],
+        ));
+        app.on_manifest("two", &sha(), Ok(other("two")));
+        assert_eq!(setup(&app).name, "team");
+        // A newer fetch of a queued screen's manifest replaces its items.
+        let two_v3 = Manifest::parse("format = 1\nname = \"two\"\nversion = \"3\"").unwrap();
+        app.on_manifest("two", &sha(), Ok(two_v3));
+        arm(&mut app);
+        app.on_terminal(press(KeyCode::Enter));
+        assert!(
+            matches!(app.modal, Some(Modal::Confirm(_))),
+            "then the prompt"
+        );
+        app.confirm_tick();
+        for _ in 0..4 {
+            app.confirm_tick();
+        }
+        app.on_terminal(press(KeyCode::Char('n')));
+        let s = setup(&app);
+        assert_eq!((s.name.as_str(), s.version.as_str()), ("two", "3"));
+        assert!(s.items.is_empty());
+    }
+
+    #[test]
+    fn a_newer_manifest_keeps_the_toggles_of_the_ids_still_there() {
+        let mut s = PluginSetup::new(&team(), &["mcp:kb".into(), "plugin:a@m".into()], true);
+        s.selected = 3;
+        let newer = TEAM
+            .replace("name = \"a\"", "name = \"b\"")
+            .replace("1.0.0", "1.1.0");
+        s.replace(&Manifest::parse(&newer).unwrap());
+        assert_eq!(s.version, "1.1.0");
+        assert_eq!(s.off_ids(), ["mcp:kb"], "plugin:b@m is new: on");
+        assert_eq!((s.selected, s.armed, s.first_sight), (3, ARM_TICKS, true));
+    }
+
+    #[test]
+    fn new_items_of_a_newer_version_are_named_once_and_on() {
+        let mut app = app_serving(&["team"]);
+        let seen = vec!["plugin:a@m".to_owned(), "mcp:kb".to_owned()];
+        app.plugins.choices.insert(
+            "team".into(),
+            PluginChoices {
+                disabled: vec!["mcp:kb".into()],
+                reviewed: "0.9".into(),
+                seen,
+            },
+        );
+        app.on_manifest("team", &sha(), Ok(team()));
+        assert!(app.modal.is_none(), "set up before: no screen");
+        let notice = app.notice.as_ref().unwrap().text.clone();
+        assert_eq!(
+            notice,
+            "team 1.0.0 adds agent:reviewer, instructions; Alt+i, Enter to review"
+        );
+        assert_eq!(
+            requests(&mut app),
+            [("local".into(), "team".into(), vec!["mcp:kb".into()])]
+        );
+        app.notice = None;
+        app.on_manifest("team", &sha(), Ok(team()));
+        assert!(app.notice.is_none(), "once per manifest");
+    }
+
+    #[test]
+    fn saved_choices_keep_the_disabled_ids_a_manifest_dropped() {
+        let earlier = PluginChoices {
+            disabled: vec!["mcp:gone".into(), "mcp:kb".into()],
+            reviewed: "0.9".into(),
+            seen: vec![],
+        };
+        let c = choices_for(&team(), &["agent:reviewer".into()], Some(&earlier));
+        assert_eq!(c.disabled, ["agent:reviewer", "mcp:gone"]);
+        assert_eq!(c.reviewed, "1.0.0");
+        assert_eq!(
+            c.seen,
+            [
+                "plugin:a@m",
+                "mcp:kb",
+                "agent:reviewer",
+                "instructions",
+                "mcp:gone"
+            ]
+        );
+    }
+
+    #[test]
+    fn enter_on_a_plugin_row_opens_its_setup_and_esc_goes_back() {
+        let mut app = app_serving(&["team"]);
+        app.plugins
+            .choices
+            .insert("team".into(), PluginChoices::default());
+        app.on_manifest("team", &sha(), Ok(team()));
+        app.take_effects();
+        let row = app
+            .extensions
+            .extensions
+            .iter()
+            .position(|e| e.name == "team")
+            .unwrap();
+        app.on_terminal(alt('i'));
+        for _ in 0..row {
+            app.on_terminal(press(KeyCode::Down));
+        }
+        app.on_terminal(press(KeyCode::Enter));
+        let s = setup(&app);
+        assert!(!s.first_sight && s.back.is_some());
+        assert_eq!(
+            s.hint(),
+            "Space toggle · a all on · n all off · Enter save · Esc cancel"
+        );
+        app.on_terminal(press(KeyCode::Char(' ')));
+        app.on_terminal(press(KeyCode::Esc));
+        assert!(
+            matches!(&app.modal, Some(Modal::ExtensionsMenu(v)) if v.selected == row),
+            "back on the extensions screen"
+        );
+        assert!(app.take_effects().is_empty(), "nothing saved");
+        // Space still toggles the plugin.
+        app.on_terminal(press(KeyCode::Char(' ')));
+        let effects = app.take_effects();
+        assert!(effects
+            .iter()
+            .any(|e| matches!(e, Effect::SaveExtension { name, on: false } if name == "team")));
+        // Enter on a row that is not a plugin toggles it.
+        app.on_terminal(press(KeyCode::Home));
+        app.on_terminal(press(KeyCode::Enter));
+        let effects = app.take_effects();
+        assert!(effects
+            .iter()
+            .any(|e| matches!(e, Effect::SaveExtension { name, on: false } if name == "latex")));
+    }
+
+    /// A `--plain` app with profile `work` (serving `team`, cached).
+    fn plain_app() -> App {
+        use crate::tui::app::{AppConfig, Mode};
+        let mut app = App::new(AppConfig {
+            mode: Mode::Plain,
+            proxy_default: Some("work".into()),
+            proxy_profiles: vec!["work".into()],
+            ..crate::tui::test_support::config()
+        });
+        app.now = 1000;
+        let list = PluginList {
+            profile: "work".into(),
+            fetched_at: 1,
+            plugins: vec![sample("team")],
+        };
+        app.load_plugins(vec![list], Default::default(), Default::default());
+        app.on_plugin_host("local");
+        app.take_effects();
+        app
+    }
+
+    fn start(app: &mut App) {
+        app.start_plain(crate::tui::plain::PlainStart {
+            id: uuid::Uuid::new_v4(),
+            args: vec![],
+            cwd: "/w".into(),
+        });
+    }
+
+    fn spawned(app: &mut App) -> Option<Vec<String>> {
+        app.take_effects().into_iter().find_map(|e| match e {
+            Effect::SpawnWithProxy { pending, .. } => {
+                Some(pending.into_iter().map(|p| p.name).collect())
+            }
+            Effect::Request {
+                msg: Msg::Spawn(_), ..
+            } => Some(Vec::new()),
+            _ => None,
+        })
+    }
+
+    #[test]
+    fn plain_waits_for_a_new_manifest_and_its_setup_then_spawns() {
+        let mut app = plain_app();
+        start(&mut app);
+        assert_eq!(spawned(&mut app), None, "the manifest is on its way");
+        app.on_manifest("team", &sha(), Ok(team()));
+        assert_eq!(spawned(&mut app), None, "the setup is open");
+        arm(&mut app);
+        app.on_terminal(press(KeyCode::Enter));
+        assert_eq!(
+            spawned(&mut app),
+            Some(vec!["team".to_owned()]),
+            "waits for local's answer"
+        );
+    }
+
+    #[test]
+    fn plain_spawns_at_once_when_set_up_and_after_20_s_without_a_manifest() {
+        let mut app = plain_app();
+        app.plugins
+            .choices
+            .insert("team".into(), PluginChoices::default());
+        start(&mut app);
+        assert!(spawned(&mut app).is_some());
+
+        let mut app = plain_app();
+        start(&mut app);
+        app.now = 1019;
+        app.plain_try_start();
+        assert_eq!(spawned(&mut app), None);
+        app.now = 1020;
+        app.plain_try_start();
+        assert_eq!(spawned(&mut app), Some(Vec::new()), "without the plugin");
+    }
+}
diff --git a/src/tui/plugins.rs b/src/tui/plugins.rs
index 5dd157d..65ae494 100644
--- a/src/tui/plugins.rs
+++ b/src/tui/plugins.rs
@@ -832,12 +846,15 @@ KB_REGION = "eu"
         assert_eq!(details[1], "manifest: hash mismatch");
     }
 
-    /// An app with `local` connected and profile `work` serving `kb`, whose
-    /// manifest is here and which `local` answered with `answer` (`None`:
-    /// not yet).
+    /// An app with `local` connected and profile `work` serving `kb`, set
+    /// up, whose manifest is here and which `local` answered with `answer`
+    /// (`None`: not yet).
     fn app_with_kb(answer: Option<Msg>) -> App {
         let mut app = app_with(&[]);
         app.modal = None;
+        app.plugins
+            .choices
+            .insert("kb".into(), PluginChoices::default());
         app.on_plugin_host("local");
         app.on_plugins(
             "work",
@@ -889,6 +906,9 @@ KB_REGION = "eu"
         installing.skipped = vec![("plugin:p@m".into(), "installing".into())];
         let mut app = app_with(&[]);
         app.now = 1000;
+        app.plugins
+            .choices
+            .insert("kb".into(), PluginChoices::default());
         app.on_plugin_host("local");
         app.on_plugins(
             "work",
@@ -1162,6 +1182,7 @@ KB_REGION = "eu"
             [
                 "turning it on or off applies to new sessions",
                 "The knowledge base",
+                "0 items disabled (Enter to change)",
                 "local: ready · 1 installed, 0 present, 1 installing, 1 skipped",
                 "  mcp:kb: missing secret KB_TOKEN",
                 "local: claude too old: --agents",
diff --git a/src/tui/ui/mod.rs b/src/tui/ui/mod.rs
index 5520799..b9bd21d 100644
--- a/src/tui/ui/mod.rs
+++ b/src/tui/ui/mod.rs
@@ -15,6 +15,7 @@ mod confirm;
 mod extensions;
 mod git;
 mod leader;
+mod plugin_setup;
 mod reader;
 mod stats;
 mod status;
diff --git a/src/tui/ui/plugin_setup.rs b/src/tui/ui/plugin_setup.rs
new file mode 100644
index 0000000..f15996c
--- /dev/null
+++ b/src/tui/ui/plugin_setup.rs
@@ -0,0 +1,64 @@
+#[cfg(test)]
+mod tests {
+    use super::*;
+    use crate::plugins::manifest::Manifest;
+    use ratatui::{backend::TestBackend, Terminal};
+
+    fn setup() -> PluginSetup {
+        let src = r#"
+format = 1
+name = "team"
+version = "1.0.0"
+instructions = "Rules."
+[[marketplace]]
+name = "m"
+source = "github:o/r"
+[[plugin]]
+name = "a"
+marketplace = "m"
+[[mcp]]
+name = "kb"
+type = "sse"
+url = "https://kb.example.org"
+"#;
+        let m = Manifest::parse(src).unwrap();
+        PluginSetup::new(&m, &["mcp:kb".to_owned()], true)
+    }
+
+    #[test]
+    fn items_are_listed_by_section_with_their_boxes() {
+        let lines: Vec<String> = setup_lines(&setup()).into_iter().map(|(_, l)| l).collect();
+        assert_eq!(
+            lines,
+            [
+                "Plugins",
+                "  [x] a@m",
+                "MCP servers",
+                "  [ ] kb (sse https://kb.example.org)",
+                "Instructions",
+                "  [x] instructions",
+            ]
+        );
+    }
+
+    #[test]
+    fn the_screen_draws_its_title_items_and_hint() {
+        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
+        terminal.draw(|f| draw_plugin_setup(f, &setup())).unwrap();
+        let buf = terminal.backend().buffer().clone();
+        let text: String = (0..buf.area.height)
+            .map(|y| {
+                (0..buf.area.width)
+                    .map(|x| buf[(x, y)].symbol())
+                    .collect::<String>()
+                    + "\n"
+            })
+            .collect();
+        assert!(text.contains("Set up team 1.0.0"), "{text}");
+        assert!(
+            text.contains("[ ] kb (sse https://kb.example.org)"),
+            "{text}"
+        );
+        assert!(text.contains("Enter save · Esc later"), "{text}");
+    }
+}
diff --git a/tests/manager.rs b/tests/manager.rs
index ad2681b..e9f9350 100644
--- a/tests/manager.rs
+++ b/tests/manager.rs
@@ -2699,7 +2699,7 @@ fn test_extensions_screen_turns_extensions_off_and_saves_it() {
 
     tui.send_keys(ALT_I);
     tui.wait_for(
-        "Extensions (Space toggle · r refresh · Esc close)",
+        "Extensions (Space toggle · Enter plugin setup · r refresh · Esc close)",
         Region::Screen,
         WAIT,
     );
@@ -2748,7 +2748,7 @@ fn test_extensions_screen_turns_extensions_off_and_saves_it() {
     // `e` opens the screen too.
     tui.send_keys(b"e");
     tui.wait_for(
-        "Extensions (Space toggle · r refresh · Esc close)",
+        "Extensions (Space toggle · Enter plugin setup · r refresh · Esc close)",
         Region::Screen,
         WAIT,
     );
`````
Run: `umask 022; cargo test --locked --bin claudio tui::`
Expected: `` error[E0433]: failed to resolve: use of undeclared type `KeyCode` ``, `` error[E0425]: cannot find type `App` in this scope ``, ending `` error: could not compile `claudio` (bin "claudio" test) due to 85 previous errors; 2 warnings emitted ``

Step 2: implement
`````diff
diff --git a/src/tui/app.rs b/src/tui/app.rs
index f31e548..da57ec9 100644
--- a/src/tui/app.rs
+++ b/src/tui/app.rs
@@ -207,6 +207,11 @@ pub enum Effect {
         url: String,
         sha256: String,
     },
+    /// Write `[plugins.<name>]` to config.toml; a failure is a notice.
+    SavePluginChoices {
+        name: String,
+        choices: crate::config::PluginChoices,
+    },
 }
 
 /// A plugin manifest a deferred spawn waits for: its `Manifest` request,
@@ -350,8 +355,8 @@ pub struct App {
     pub claude_skipped: HashMap<String, String>,
     /// Hosts whose claude was already checked in this run.
     pub(super) claude_checked: HashSet<String>,
-    /// Prompts waiting for the open modal to close.
-    pub(super) confirms: VecDeque<ConfirmPrompt>,
+    /// Prompts and plugin setup screens waiting for the open modal to close.
+    pub(super) confirms: VecDeque<super::confirm::Queued>,
     /// The SSH hosts the wizard offers: recently used ones, then
     /// `~/.ssh/config` aliases.
     pub(super) ssh_hosts: Vec<String>,
@@ -381,6 +386,9 @@ pub struct App {
     /// The proxy plugins: lists, saved choices, manifests, each host's
     /// answers.
     pub plugins: super::plugins::Plugins,
+    /// `--plain`'s session while plugin setup holds it: what to start, its
+    /// proxy profile, and until when (Unix seconds) manifests are awaited.
+    pub(super) plain_pending: Option<(super::plain::PlainStart, Option<String>, u64)>,
 }
 
 /// What the app starts from. The event loop gathers it (reading config.toml,
@@ -474,6 +482,7 @@ impl App {
             pending_action: None,
             failed_extensions: Vec::new(),
             plugins: Default::default(),
+            plain_pending: None,
         }
     }
 
@@ -886,6 +895,7 @@ impl App {
         self.now = crate::paths::unix_now();
         self.confirm_tick();
         self.reask_installing();
+        self.plain_try_start();
         self.refresh_session_cred(SESSION_CRED_POLL_SECS);
         if let Some(n) = &mut self.notice {
             n.ticks_left = n.ticks_left.saturating_sub(1);
diff --git a/src/tui/confirm.rs b/src/tui/confirm.rs
index 4f7c1be..c37fb9b 100644
--- a/src/tui/confirm.rs
+++ b/src/tui/confirm.rs
@@ -3,7 +3,8 @@
 //! [`ConfirmPrompt`] is generic: it carries the question and the keys that
 //! answer it, each with the [`ConfirmAction`] it runs (or none, to just close).
 //! `Esc` always closes. Prompts raised while another modal is open wait their
-//! turn in `App::confirms`, and ignore keys for a moment after appearing so a
+//! turn in `App::confirms` (with the plugin setup screens, see
+//! `plugin_setup.rs`), and ignore keys for a moment after appearing so a
 //! stray keystroke meant for a session cannot answer one.
 
 use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
@@ -12,6 +13,7 @@ use crate::proto::SessionId;
 
 use super::app::App;
 use super::interaction::Modal;
+use super::plugin_setup::PluginSetup;
 
 /// Ticks (250 ms each) during which a new prompt ignores the keyboard.
 const ARM_TICKS: u8 = 4;
@@ -82,6 +84,12 @@ impl Choice {
     }
 }
 
+/// A modal waiting its turn: a prompt or a plugin setup screen.
+pub enum Queued {
+    Confirm(ConfirmPrompt),
+    Setup(Box<PluginSetup>),
+}
+
 pub struct ConfirmPrompt {
     pub title: String,
     /// The question; may span lines.
@@ -120,23 +128,29 @@ impl ConfirmPrompt {
 impl App {
     /// Show `prompt` now, or as soon as no other modal is open.
     pub(super) fn queue_confirm(&mut self, prompt: ConfirmPrompt) {
-        self.confirms.push_back(prompt);
+        self.confirms.push_back(Queued::Confirm(prompt));
         self.show_next_confirm();
     }
 
-    fn show_next_confirm(&mut self) {
+    /// Open the first waiting prompt or setup screen if no modal is open.
+    pub(super) fn show_next_confirm(&mut self) {
         if self.modal.is_none() {
-            if let Some(prompt) = self.confirms.pop_front() {
-                self.modal = Some(Modal::Confirm(prompt));
-                self.redraw = true;
+            match self.confirms.pop_front() {
+                Some(Queued::Confirm(prompt)) => self.modal = Some(Modal::Confirm(prompt)),
+                Some(Queued::Setup(setup)) => self.modal = Some(Modal::PluginSetup(setup)),
+                None => return,
             }
+            self.redraw = true;
         }
     }
 
-    /// Per-tick upkeep: arm the open prompt, open a waiting one.
+    /// Per-tick upkeep: arm the open prompt or setup screen, open a waiting
+    /// one.
     pub(super) fn confirm_tick(&mut self) {
-        if let Some(Modal::Confirm(prompt)) = &mut self.modal {
-            prompt.armed = prompt.armed.saturating_sub(1);
+        match &mut self.modal {
+            Some(Modal::Confirm(prompt)) => prompt.armed = prompt.armed.saturating_sub(1),
+            Some(Modal::PluginSetup(setup)) => setup.armed = setup.armed.saturating_sub(1),
+            _ => {}
         }
         self.show_next_confirm();
     }
diff --git a/src/tui/extensions_view.rs b/src/tui/extensions_view.rs
index d09eb09..44ad170 100644
--- a/src/tui/extensions_view.rs
+++ b/src/tui/extensions_view.rs
@@ -2,8 +2,9 @@
 //! extension (built-ins first, then the external ones that loaded, then the
 //! proxy plugins, on or off) with its kind, keys and lenses; below the list,
 //! the selected one's description, actions and lenses (for a plugin, its
-//! state on each host). Space or Enter turns it on or off; `r` fetches the
-//! plugin lists again.
+//! manifest and its state on each host). Space turns it on or off, as
+//! Enter does on every row but a plugin's, where Enter opens its setup
+//! screen (`plugin_setup.rs`); `r` fetches the plugin lists again.
 //!
 //! Pure, like `app.rs`: a toggle changes the registry at once and returns
 //! [`Effect::SaveExtension`] (the event loop writes `config.toml`) and, for
@@ -20,7 +21,7 @@ use crate::claude::latex::sanitize;
 use crate::proto::LensKind;
 
 /// The screen's title.
-pub const TITLE: &str = "Extensions (Space toggle · r refresh · Esc close)";
+pub const TITLE: &str = "Extensions (Space toggle · Enter plugin setup · r refresh · Esc close)";
 /// The screen's last line.
 pub const FOOTER: &str =
     "changes apply now; claude's instructions change for new sessions (Alt+e restarts one)";
@@ -40,6 +41,8 @@ pub struct ExtensionsView {
 enum Outcome {
     Stay,
     Toggle(usize),
+    /// Enter: a plugin's setup screen, else a toggle.
+    Enter(usize),
     /// `r`: fetch the plugin lists again.
     Refresh,
     Close,
@@ -51,9 +54,8 @@ impl ExtensionsView {
         let last = count.saturating_sub(1);
         match key.code {
             KeyCode::Esc => return Outcome::Close,
-            KeyCode::Char(' ') | KeyCode::Enter if count > 0 => {
-                return Outcome::Toggle(self.selected.min(last))
-            }
+            KeyCode::Char(' ') if count > 0 => return Outcome::Toggle(self.selected.min(last)),
+            KeyCode::Enter if count > 0 => return Outcome::Enter(self.selected.min(last)),
             KeyCode::Char('r') => return Outcome::Refresh,
             KeyCode::Up => self.selected = self.selected.saturating_sub(1),
             KeyCode::Down => self.selected = (self.selected + 1).min(last),
@@ -237,10 +239,15 @@ impl App {
         let Some(Modal::ExtensionsMenu(view)) = &mut self.modal else {
             return;
         };
+        let back = view.clone();
         match view.on_key(key, count) {
             Outcome::Stay => {}
             Outcome::Close => self.modal = None,
             Outcome::Toggle(index) => self.toggle_extension(index),
+            Outcome::Enter(index) => match self.extensions.extensions.get(index) {
+                Some(ext) if ext.is_plugin() => self.open_plugin_setup(index, back),
+                _ => self.toggle_extension(index),
+            },
             Outcome::Refresh => self.refresh_plugins(),
         }
         self.redraw = true;
@@ -264,6 +271,11 @@ impl App {
             self.plugins.config.insert(name.clone(), on);
             self.extensions.extensions[index].enabled = on;
             if on {
+                // Its setup if it was never set up, else its manifest.
+                let sha = self.plugins.manifests.get(&name).map(|(s, _)| s.clone());
+                if let Some(sha) = sha {
+                    self.on_manifest_ready(&name, &sha);
+                }
                 self.request_all();
             }
             self.emit(Effect::SaveExtension { name, on });
diff --git a/src/tui/interaction.rs b/src/tui/interaction.rs
index 569b3db..81c07e5 100644
--- a/src/tui/interaction.rs
+++ b/src/tui/interaction.rs
@@ -69,6 +69,9 @@ pub enum Modal {
     Picker(Box<Picker>),
     /// The extensions screen (Alt+i, see `extensions_view.rs`).
     ExtensionsMenu(ExtensionsView),
+    /// A plugin's setup screen (see `plugin_setup.rs`): swallows every
+    /// key, like `Confirm`.
+    PluginSetup(Box<super::plugin_setup::PluginSetup>),
 }
 
 impl Modal {
@@ -400,6 +403,7 @@ impl App {
             Modal::ActionInput(_) => self.action_input_key(&key),
             Modal::Picker(_) => self.picker_key(&key),
             Modal::ExtensionsMenu(_) => self.extensions_menu_key(&key),
+            Modal::PluginSetup(_) => self.plugin_setup_key(&key),
         }
     }
 
@@ -467,7 +471,8 @@ impl App {
                 | Modal::Reader(_)
                 | Modal::Leader
                 | Modal::Picker(_)
-                | Modal::ExtensionsMenu(_),
+                | Modal::ExtensionsMenu(_)
+                | Modal::PluginSetup(_),
             ) => {}
             Some(Modal::ActionInput(input)) => {
                 let line = text.lines().next().unwrap_or("");
diff --git a/src/tui/mod.rs b/src/tui/mod.rs
index 2399f9c..af1e21b 100644
--- a/src/tui/mod.rs
+++ b/src/tui/mod.rs
@@ -1421,6 +1421,12 @@ fn run_effect(
             // Stopping a program waits up to its grace: never here.
             tokio::spawn(ext_host.set_enabled(&name, on));
         }
+        Effect::SavePluginChoices { name, choices } => {
+            match crate::config::save_plugin_choices(&paths::config_file(), &name, &choices) {
+                Ok(()) => app.notify(format!("plugin {name} set up (saved)")),
+                Err(e) => app.notify(format!("could not save: {e}")),
+            }
+        }
         Effect::SaveExtension { name, on } => {
             let state = if on { "on" } else { "off" };
             match crate::config::save_extension(&paths::config_file(), &name, on) {
diff --git a/src/tui/plain.rs b/src/tui/plain.rs
index 8fbc873..2afd804 100644
--- a/src/tui/plain.rs
+++ b/src/tui/plain.rs
@@ -46,12 +46,24 @@ impl App {
     /// Start the plain UI's session on the local daemon, with the proxy
     /// profile the startup choice picks (like the wizard's default). The app
     /// is one made in [`Mode::Plain`].
+    ///
+    /// With a proxy profile whose enabled plugins include one never set up
+    /// (no `[plugins.<name>]`), the start waits until each such manifest
+    /// has arrived or failed (at most 20 s) and every setup screen they
+    /// open has closed (`plugin_setup.rs`).
     pub fn start_plain(&mut self, start: PlainStart) {
         debug_assert_eq!(self.mode, Mode::Plain);
         let proxy = self
             .proxy_override
             .pick(self.proxy_default.as_deref())
             .map(str::to_owned);
+        let deadline = self.now + super::BUNDLE_WAIT.as_secs();
+        self.plain_pending = Some((start, proxy, deadline));
+        self.plain_try_start();
+    }
+
+    /// Start the plain UI's session now, with `proxy`.
+    pub(super) fn start_plain_now(&mut self, start: PlainStart, proxy: Option<String>) {
         let PlainStart { id, args, cwd } = start;
         let started = self.start(id, "local".to_owned(), cwd, SessionKind::Claude, args, proxy, 0);
         if let Err(e) = started {
diff --git a/src/tui/plugin_setup.rs b/src/tui/plugin_setup.rs
index e8f38b5..a4917b6 100644
--- a/src/tui/plugin_setup.rs
+++ b/src/tui/plugin_setup.rs
@@ -1,3 +1,373 @@
+//! The plugin setup screen: every item of a proxy plugin's manifest, by
+//! section, each with a checkbox; the member chooses what to apply.
+//!
+//! It opens on a manifest's first sight (no `[plugins.<name>]` table in
+//! config.toml yet) and from the extensions screen (Enter on the plugin's
+//! row). Keys: Up/Down (or j/k) move, Space toggles, `a` turns every item
+//! on, `n` every item off, Enter saves and closes (the choices go to
+//! config.toml and every host is asked again), Esc closes without saving.
+//! Esc on first sight defers: nothing is written or sent for the manifest,
+//! the row says `setup pending (Enter)`, and the screen opens again at the
+//! next claudio start. The screen swallows every key, and it waits its turn
+//! in the confirm queue like a prompt: one at a time, in arrival order. A
+//! first-sight screen ignores keys for a moment after appearing, like a
+//! prompt, so a keystroke meant for a session cannot answer it.
+//!
+//! Pure, like `confirm.rs`: saving is [`Effect::SavePluginChoices`]; drawn
+//! by `ui/plugin_setup.rs`.
+
+use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
+
+use crate::config::PluginChoices;
+use crate::plugins::manifest::{Manifest, Section};
+
+use super::app::{App, Effect, Mode};
+use super::confirm::Queued;
+use super::extensions::Source;
+use super::extensions_view::ExtensionsView;
+use super::interaction::Modal;
+use super::plugins::ManifestState;
+
+/// Ticks (250 ms each) during which a first-sight screen ignores keys.
+const ARM_TICKS: u8 = 4;
+
+/// One item of the screen.
+#[derive(Debug, Clone, PartialEq)]
+pub struct SetupItem {
+    pub id: String,
+    pub section: Section,
+    pub title: String,
+    pub on: bool,
+}
+
+/// The setup screen of one plugin's manifest.
+#[derive(Debug, Clone, PartialEq)]
+pub struct PluginSetup {
+    pub name: String,
+    pub version: String,
+    pub items: Vec<SetupItem>,
+    pub selected: usize,
+    /// Opened on first sight: Esc defers instead of just closing.
+    pub first_sight: bool,
+    /// Remaining ticks before keys count.
+    pub(super) armed: u8,
+    /// The extensions screen it was opened from, shown again on close.
+    pub back: Option<ExtensionsView>,
+}
+
+/// What a key did on the screen.
+#[derive(Debug, PartialEq)]
+enum Outcome {
+    Stay,
+    Save,
+    Close,
+}
+
+impl PluginSetup {
+    /// The screen for `m`, with the ids of `disabled` off.
+    pub fn new(m: &Manifest, disabled: &[String], first_sight: bool) -> PluginSetup {
+        let items = m
+            .items()
+            .into_iter()
+            .map(|i| SetupItem {
+                on: !disabled.contains(&i.id),
+                id: i.id,
+                section: i.section,
+                title: i.title,
+            })
+            .collect();
+        PluginSetup {
+            name: m.name.clone(),
+            version: m.version.clone(),
+            items,
+            selected: 0,
+            first_sight,
+            armed: if first_sight { ARM_TICKS } else { 0 },
+            back: None,
+        }
+    }
+
+    /// Show `m` (a newer fetch) instead, keeping the toggle of every id
+    /// still there; a new id is on.
+    pub fn replace(&mut self, m: &Manifest) {
+        let off: Vec<String> = self.off_ids();
+        let mut next = PluginSetup::new(m, &off, self.first_sight);
+        next.armed = self.armed;
+        next.back = self.back.take();
+        next.selected = self.selected.min(next.items.len().saturating_sub(1));
+        *self = next;
+    }
+
+    /// The ids turned off.
+    pub fn off_ids(&self) -> Vec<String> {
+        self.items
+            .iter()
+            .filter(|i| !i.on)
+            .map(|i| i.id.clone())
+            .collect()
+    }
+
+    fn on_key(&mut self, key: &KeyEvent) -> Outcome {
+        // Ctrl or Alt chords are not answers; Shift only makes a capital.
+        if self.armed > 0 || !key.modifiers.difference(KeyModifiers::SHIFT).is_empty() {
+            return Outcome::Stay;
+        }
+        let last = self.items.len().saturating_sub(1);
+        match key.code {
+            KeyCode::Enter => return Outcome::Save,
+            KeyCode::Esc => return Outcome::Close,
+            KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
+            KeyCode::Down | KeyCode::Char('j') => self.selected = (self.selected + 1).min(last),
+            KeyCode::Char(' ') => {
+                if let Some(item) = self.items.get_mut(self.selected) {
+                    item.on = !item.on;
+                }
+            }
+            KeyCode::Char('a') => self.items.iter_mut().for_each(|i| i.on = true),
+            KeyCode::Char('n') => self.items.iter_mut().for_each(|i| i.on = false),
+            _ => {}
+        }
+        Outcome::Stay
+    }
+
+    /// The key hint: Esc defers on first sight, cancels otherwise.
+    pub fn hint(&self) -> &'static str {
+        if self.first_sight {
+            "Space toggle · a all on · n all off · Enter save · Esc later"
+        } else {
+            "Space toggle · a all on · n all off · Enter save · Esc cancel"
+        }
+    }
+}
+
+/// The choices to save for `m` from a screen whose off ids are `off`:
+/// `disabled` keeps the earlier disabled ids `m` no longer has (so one that
+/// comes back stays off), and `seen` is every id of `m` plus those.
+pub fn choices_for(m: &Manifest, off: &[String], earlier: Option<&PluginChoices>) -> PluginChoices {
+    let ids = m.ids();
+    let gone: Vec<String> = earlier
+        .map(|c| {
+            c.disabled
+                .iter()
+                .filter(|d| !ids.contains(d))
+                .cloned()
+                .collect()
+        })
+        .unwrap_or_default();
+    let mut disabled = off.to_vec();
+    disabled.extend(gone.iter().cloned());
+    let mut seen = ids;
+    seen.extend(gone);
+    PluginChoices {
+        disabled,
+        reviewed: m.version.clone(),
+        seen,
+    }
+}
+
+impl App {
+    /// The setup screen of plugin `name`, if one is open or queued.
+    fn setup_of(&mut self, name: &str) -> Option<&mut PluginSetup> {
+        if let Some(Modal::PluginSetup(s)) = &mut self.modal {
+            if s.name == name {
+                return Some(s);
+            }
+        }
+        self.confirms.iter_mut().find_map(|q| match q {
+            Queued::Setup(s) if s.name == name => Some(&mut **s),
+            _ => None,
+        })
+    }
+
+    /// Whether a setup screen of plugin `name` is open or queued.
+    pub(super) fn setup_shown(&mut self, name: &str) -> bool {
+        self.setup_of(name).is_some()
+    }
+
+    /// Plugin `name`'s manifest `m` arrived: a first sight queues its setup
+    /// screen (unless deferred in this run); an open or queued screen shows
+    /// it instead; with choices saved, ids not seen before are named once.
+    pub(super) fn on_manifest_ready(&mut self, name: &str, sha256: &str) {
+        let Some((_, ManifestState::Ready(m))) = self.plugins.manifests.get(name) else {
+            return;
+        };
+        let m = m.clone();
+        if let Some(setup) = self.setup_of(name) {
+            setup.replace(&m);
+            self.redraw = true;
+            return;
+        }
+        match self.plugins.choices.get(name) {
+            None if !self.plugins.deferred.contains(name) && self.plugin_enabled(name) => {
+                let setup = PluginSetup::new(&m, &[], true);
+                self.queue_setup(setup);
+            }
+            None => {}
+            Some(choices) => {
+                let new: Vec<String> = m
+                    .ids()
+                    .into_iter()
+                    .filter(|id| !choices.seen.contains(id))
+                    .collect();
+                let key = (name.to_owned(), sha256.to_owned());
+                if !new.is_empty() && self.plugins.noticed.insert(key) {
+                    self.notify(format!(
+                        "{name} {} adds {}; Alt+i, Enter to review",
+                        m.version,
+                        new.join(", ")
+                    ));
+                }
+            }
+        }
+    }
+
+    /// Show `setup` now, or once no other modal is open.
+    fn queue_setup(&mut self, setup: PluginSetup) {
+        self.confirms.push_back(Queued::Setup(Box::new(setup)));
+        self.show_next_confirm();
+    }
+
+    /// Enter on the extensions screen's plugin row `index`: its setup
+    /// screen, over (and back to) the extensions screen.
+    pub(super) fn open_plugin_setup(&mut self, index: usize, back: ExtensionsView) {
+        let Some(ext) = self.extensions.extensions.get(index) else {
+            return;
+        };
+        let name = ext.name.clone();
+        if let Source::Plugin {
+            refused: Some(why), ..
+        } = &ext.source
+        {
+            self.notify(format!("plugin {name} is refused: {why}"));
+            return;
+        }
+        let m = match self.plugins.manifests.get(&name) {
+            Some((_, ManifestState::Ready(m))) => m.clone(),
+            Some((_, ManifestState::Failed(why))) => {
+                self.notify(format!("manifest of {name}: {why}"));
+                return;
+            }
+            _ => {
+                self.notify(format!("manifest of {name}: not here yet"));
+                return;
+            }
+        };
+        let disabled = self
+            .plugins
+            .choices
+            .get(&name)
+            .map(|c| c.disabled.clone())
+            .unwrap_or_default();
+        let mut setup = PluginSetup::new(&m, &disabled, false);
+        setup.back = Some(back);
+        self.modal = Some(Modal::PluginSetup(Box::new(setup)));
+        self.redraw = true;
+    }
+
+    /// A key on the setup screen.
+    pub(super) fn plugin_setup_key(&mut self, key: &KeyEvent) {
+        let Some(Modal::PluginSetup(setup)) = &mut self.modal else {
+            return;
+        };
+        let outcome = setup.on_key(key);
+        self.redraw = true;
+        if outcome == Outcome::Stay {
+            return;
+        }
+        let Some(Modal::PluginSetup(setup)) = self.modal.take() else {
+            return;
+        };
+        let name = setup.name.clone();
+        match outcome {
+            Outcome::Save => self.save_setup(&setup),
+            _ if setup.first_sight => {
+                self.plugins.deferred.insert(name);
+            }
+            _ => {}
+        }
+        if let Some(back) = setup.back {
+            self.modal = Some(Modal::ExtensionsMenu(back));
+        }
+        self.show_next_confirm();
+        self.plain_try_start();
+    }
+
+    /// Enter on `setup`: save the choices and ask every host again.
+    fn save_setup(&mut self, setup: &PluginSetup) {
+        let name = setup.name.clone();
+        let Some((_, ManifestState::Ready(m))) = self.plugins.manifests.get(&name) else {
+            return;
+        };
+        let choices = choices_for(m, &setup.off_ids(), self.plugins.choices.get(&name));
+        self.plugins.deferred.remove(&name);
+        self.plugins.choices.insert(name.clone(), choices.clone());
+        self.emit(Effect::SavePluginChoices {
+            name: name.clone(),
+            choices,
+        });
+        self.request_again(&name);
+    }
+
+    /// The setup line of plugin `name`'s details on the extensions screen.
+    pub(super) fn setup_details(&self, name: &str) -> Option<String> {
+        let ids = match self.plugins.manifests.get(name) {
+            Some((_, ManifestState::Ready(m))) => m.ids(),
+            _ => return None,
+        };
+        match self.plugins.choices.get(name) {
+            None => Some("setup pending (Enter)".to_owned()),
+            Some(c) => {
+                let off = c.disabled.iter().filter(|d| ids.contains(d)).count();
+                Some(format!("{off} items disabled (Enter to change)"))
+            }
+        }
+    }
+
+    // ── --plain ───────────────────────────────────────────────────────────────
+
+    /// Whether `--plain`'s session must wait for plugin setup: its profile
+    /// has an enabled plugin without saved choices whose manifest has not
+    /// arrived (before `deadline`) or whose setup screen is open or queued.
+    fn plain_waits(&mut self, profile: &str, deadline: u64) -> bool {
+        let names: Vec<String> = self
+            .enabled_plugins(profile)
+            .iter()
+            .filter(|p| !self.plugins.choices.contains_key(&p.name))
+            .map(|p| p.name.clone())
+            .collect();
+        let now = self.now;
+        names.iter().any(|name| {
+            let fetching = !matches!(
+                self.plugins.manifests.get(name),
+                Some((_, ManifestState::Ready(_) | ManifestState::Failed(_)))
+            );
+            (fetching && now < deadline) || self.setup_shown(name)
+        })
+    }
+
+    /// Start `--plain`'s deferred session once nothing holds it.
+    pub(super) fn plain_try_start(&mut self) {
+        let Some((profile, deadline)) = self
+            .plain_pending
+            .as_ref()
+            .map(|(_, proxy, deadline)| (proxy.clone(), *deadline))
+        else {
+            return;
+        };
+        if self.mode != Mode::Plain {
+            return;
+        }
+        if let Some(profile) = profile {
+            if self.plain_waits(&profile, deadline) {
+                return;
+            }
+        }
+        if let Some((start, proxy, _)) = self.plain_pending.take() {
+            self.start_plain_now(start, proxy);
+        }
+    }
+}
+
 #[cfg(test)]
 mod tests {
     use super::*;
diff --git a/src/tui/plugins.rs b/src/tui/plugins.rs
index dd657c9..65ae494 100644
--- a/src/tui/plugins.rs
+++ b/src/tui/plugins.rs
@@ -11,7 +11,7 @@
 //! is asked on connect, when the list's hash changes, on `r`, and every
 //! 30 s while it still installs.
 
-use std::collections::{BTreeMap, BTreeSet, HashMap};
+use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
 
 use crate::config::PluginChoices;
 use crate::plugins::manifest::Manifest;
@@ -103,6 +103,11 @@ pub struct Plugins {
     pub manifests: HashMap<String, (String, ManifestState)>,
     /// Per `(host, plugin)`: what that host answered.
     pub applies: HashMap<(String, String), Apply>,
+    /// Plugins whose first-sight setup the member put off (Esc) in this
+    /// run: nothing is asked of a host for them until the setup is saved.
+    pub deferred: BTreeSet<String>,
+    /// `(plugin, hash)` whose new items were named in a notice already.
+    pub noticed: HashSet<(String, String)>,
 }
 
 /// What one enabled plugin brings to a spawn on a host.
@@ -223,6 +228,11 @@ impl App {
             .collect()
     }
 
+    /// Whether plugin `name` (of any profile) is on.
+    pub(super) fn plugin_enabled(&self, name: &str) -> bool {
+        self.all_enabled_plugins().iter().any(|p| p.name == name)
+    }
+
     /// Every enabled plugin of every profile, in registry order.
     fn all_enabled_plugins(&self) -> Vec<&Plugin> {
         let lists = self.plugins.lists.keys();
@@ -303,6 +313,7 @@ impl App {
             // A hash since replaced.
             _ => return,
         }
+        self.on_manifest_ready(name, sha256);
         self.request_all();
         self.redraw = true;
     }
@@ -339,12 +350,14 @@ impl App {
     }
 
     /// Whether a host may be asked for `p`'s manifest now: the client has
-    /// it, good, at the list's hash.
+    /// it, good, at the list's hash, and the member's choices are saved
+    /// (nothing is sent before the first-sight setup closes with Enter).
     fn requestable(&self, p: &Plugin) -> bool {
-        matches!(
+        let fetched = matches!(
             self.plugins.manifests.get(&p.name),
             Some((sha, ManifestState::Ready(_))) if *sha == p.bundle_sha256
-        )
+        );
+        fetched && self.plugins.choices.contains_key(&p.name)
     }
 
     /// Ask `host` for each enabled plugin's manifest it has not answered for
@@ -550,6 +563,7 @@ impl App {
         if let Some(m) = manifest.filter(|m| !m.description.trim().is_empty()) {
             lines.push(m.description.clone());
         }
+        lines.extend(self.setup_details(name));
         let mut hosts: Vec<&String> = self.plugins.hosts.iter().collect();
         hosts.sort_by_key(|h| (h.as_str() != "local", h.as_str()));
         for host in hosts {
diff --git a/src/tui/ui/mod.rs b/src/tui/ui/mod.rs
index 1dcc5c0..b9bd21d 100644
--- a/src/tui/ui/mod.rs
+++ b/src/tui/ui/mod.rs
@@ -105,6 +105,7 @@ fn draw_modal(frame: &mut Frame, app: &App, pane: Rect) {
         Some(Modal::ActionInput(input)) => leader::draw_action_input(frame, input),
         Some(Modal::Picker(picker)) => leader::draw_picker(frame, picker),
         Some(Modal::ExtensionsMenu(view)) => extensions::draw_extensions(frame, app, view),
+        Some(Modal::PluginSetup(setup)) => plugin_setup::draw_plugin_setup(frame, setup),
         None => {}
     }
 }
diff --git a/src/tui/ui/plugin_setup.rs b/src/tui/ui/plugin_setup.rs
index 8b8b697..f15996c 100644
--- a/src/tui/ui/plugin_setup.rs
+++ b/src/tui/ui/plugin_setup.rs
@@ -1,3 +1,98 @@
+//! Drawing the plugin setup screen (`tui/plugin_setup.rs` has its state
+//! and keys): the manifest's items by section, each with its checkbox, and
+//! the key hint. Every line is made safe and cut to the width: titles come
+//! from the manifest.
+
+use ratatui::layout::Rect;
+use ratatui::style::{Modifier, Style};
+use ratatui::text::Line;
+use ratatui::widgets::Paragraph;
+use ratatui::Frame;
+
+use super::{centered, popup};
+use crate::tui::fmt::truncate;
+use crate::tui::plugin_setup::PluginSetup;
+use crate::tui::reader::layout::safe;
+
+/// The lines of `setup`'s list: a heading before each section, then one
+/// line per item, with the index of the item each line shows.
+pub fn setup_lines(setup: &PluginSetup) -> Vec<(Option<usize>, String)> {
+    let mut lines = Vec::new();
+    let mut section = None;
+    for (i, item) in setup.items.iter().enumerate() {
+        if section != Some(item.section) {
+            section = Some(item.section);
+            lines.push((None, item.section.heading().to_owned()));
+        }
+        let mark = if item.on { 'x' } else { ' ' };
+        lines.push((Some(i), format!("  [{mark}] {}", item.title)));
+    }
+    if lines.is_empty() {
+        lines.push((None, "(nothing to apply)".to_owned()));
+    }
+    lines
+}
+
+/// The setup screen over the whole terminal.
+pub fn draw_plugin_setup(frame: &mut Frame, setup: &PluginSetup) {
+    let area = frame.area();
+    let lines = setup_lines(setup);
+    // The list, a blank line, the hint, and the borders; capped.
+    let wanted = (lines.len() + 4).min(u16::MAX as usize) as u16;
+    let rect = centered(
+        area,
+        100.min(area.width.saturating_sub(2)),
+        wanted.min(area.height.saturating_sub(2).max(3)),
+    );
+    let title = format!("Set up {} {}", safe(&setup.name), safe(&setup.version));
+    let inner = popup(frame, rect, &title);
+    if inner.height == 0 || inner.width == 0 {
+        return;
+    }
+    let width = inner.width as usize;
+    let fit = |text: &str| truncate(&safe(&text.replace(['\n', '\t'], " ")), width);
+    let height = inner.height as usize;
+    let footer = usize::from(height > 2) * 2;
+    let room = height.saturating_sub(footer).max(1);
+    let at = lines
+        .iter()
+        .position(|(i, _)| *i == Some(setup.selected))
+        .unwrap_or(0);
+    let offset = at.saturating_sub(room.saturating_sub(1));
+    let bold = Style::default().add_modifier(Modifier::BOLD);
+    let shown: Vec<Line> = lines
+        .iter()
+        .skip(offset)
+        .take(room)
+        .map(|(i, text)| match i {
+            None => Line::styled(fit(text), bold),
+            Some(i) => {
+                let mut style = Style::default();
+                if !setup.items[*i].on {
+                    style = style.add_modifier(Modifier::DIM);
+                }
+                if *i == setup.selected {
+                    style = style.add_modifier(Modifier::REVERSED);
+                }
+                Line::styled(fit(text), style)
+            }
+        })
+        .collect();
+    let rows = |y: usize, h: usize| Rect {
+        y: inner.y + y as u16,
+        height: h as u16,
+        ..inner
+    };
+    frame.render_widget(Paragraph::new(shown), rows(0, room.min(height)));
+    if footer > 0 {
+        let dim = Style::default().add_modifier(Modifier::DIM);
+        frame.render_widget(
+            Paragraph::new(Line::styled(fit(setup.hint()), dim)),
+            rows(height - 1, 1),
+        );
+    }
+}
+
 #[cfg(test)]
 mod tests {
     use super::*;
`````
Run: `umask 022; cargo build 2>&1 | grep -c '^warning'; cargo test --locked --bin claudio; cargo test --test manager -- --test-threads=1`
Expected: `0`, then `test result: ok. 1094 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`, then `test result: ok. 50 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`

Step 3: commit
`/usr/bin/git add src/tui/plugin_setup.rs src/tui/ui/plugin_setup.rs && /usr/bin/git commit -m "feat(tui): plugin setup screen, saved choices and --plain deferral" -- src/tui/plugin_setup.rs src/tui/ui/plugin_setup.rs src/tui/confirm.rs src/tui/app.rs src/tui/interaction.rs src/tui/ui/mod.rs src/tui/mod.rs src/tui/plugins.rs src/tui/extensions_view.rs src/tui/plain.rs tests/manager.rs`

Interfaces this task provides to later tasks: `tui::plugin_setup::{PluginSetup, SetupItem, choices_for}`, `confirm::Queued`, `Modal::PluginSetup`, `Effect::SavePluginChoices`, `App::{on_manifest_ready, open_plugin_setup, plugin_setup_key, setup_details, plain_try_start, start_plain_now, plugin_enabled}`, `Plugins.{deferred, noticed}`, `App.plain_pending`, the new `extensions_view::TITLE` ("Extensions (Space toggle · Enter plugin setup · r refresh · Esc close)") and the notice `plugin <name> set up (saved)`.

### Task 8: drop the zip bundle ops; t9 applies a manifest on the remote
Model: standard   Repo: /volumes/repos/claudio   Files: src/proto.rs, src/client.rs, src/main.rs, src/remote/diag.rs, tests/remote.rs, src/proxy/plugins.rs, src/paths.rs
Goal: `Msg::Bundle`/`Bundled` and their last users go. `claudio __remote-manifest HOST CWD NAME
SHA256 SECRET` (diag) and t9 drive a seeded manifest on a remote host (AC6). The zip-era doc
comments are fixed.
Patch: .delivery/2026-11-06-plugin-manifest/patches/task-8.patch

Step 1: write the test (t9 is gated by `CLAUDIO_SSH_TEST_HOST`, so it cannot fail here; the
controller runs it for AC6)
`````diff
diff --git a/src/proto.rs b/src/proto.rs
index d8059a2..bf3dba2 100644
--- a/src/proto.rs
+++ b/src/proto.rs
@@ -1297,42 +1276,6 @@ mod tests {
         assert_eq!(back.msg, shell);
     }
 
-    #[test]
-    fn bundle_and_bundled_roundtrip_and_are_unknown_to_older_peers() {
-        #[derive(Debug, PartialEq, Deserialize)]
-        #[serde(tag = "op", rename_all = "snake_case")]
-        enum Old {
-            Ping,
-            #[serde(other)]
-            Unknown,
-        }
-        let bundle = Msg::Bundle {
-            name: "kb".into(),
-            url: "https://e.org/kb.zip".into(),
-            sha256: "a".repeat(64),
-            installed_id: "kb@m".into(),
-        };
-        let reply = Msg::Bundled {
-            name: "kb".into(),
-            path: Some("/c/plugins/kb.zip".into()),
-            installed: false,
-        };
-        for msg in [bundle, reply] {
-            let json = serde_json::to_string(&Envelope::request(4, msg.clone())).unwrap();
-            assert_eq!(serde_json::from_str::<Old>(&json).unwrap(), Old::Unknown);
-            assert_eq!(serde_json::from_str::<Envelope>(&json).unwrap().msg, msg);
-        }
-        let minimal = r#"{"op":"bundled","name":"kb"}"#;
-        assert_eq!(
-            serde_json::from_str::<Msg>(minimal).unwrap(),
-            Msg::Bundled {
-                name: "kb".into(),
-                path: None,
-                installed: false
-            }
-        );
-    }
-
     #[test]
     fn manifest_and_applied_roundtrip_and_are_unknown_to_older_peers() {
         #[derive(Debug, PartialEq, Deserialize)]
diff --git a/tests/remote.rs b/tests/remote.rs
index 1309c3b..7d40a1b 100644
--- a/tests/remote.rs
+++ b/tests/remote.rs
@@ -404,13 +404,39 @@ fn t8_probe_roundtrip() {
     );
 }
 
-// ── t9: plugin bundle on the remote ─────────────────────────────────────────
-
-/// A pre-seeded plugin bundle in the remote daemon's cache answers `Bundle`
-/// with its remote path, and a session spawned with it there has that path
-/// in its argv (when the remote claude knows `--plugin-dir`).
+// ── t9: plugin manifest on the remote ──────────────────────────────────────
+
+/// The manifest t9 seeds: no plugin to install (the host's `~/.claude` is
+/// left alone), one http MCP server needing `T9_TOKEN`, one agent,
+/// instructions and env.
+const T9_MANIFEST: &str = r#"format = 1
+name = "claudio-t9"
+version = "1.0.0"
+instructions = "t9 instructions"
+
+[[mcp]]
+name = "t9-kb"
+type = "http"
+url = "https://t9.invalid/mcp"
+headers = { Authorization = "Bearer ${T9_TOKEN}" }
+secrets = ["T9_TOKEN"]
+
+[[agent]]
+name = "t9-agent"
+description = "t9"
+prompt = "t9 prompt"
+
+[env]
+T9_REGION = "eu"
+"#;
+
+/// A pre-seeded plugin manifest in the remote daemon's cache answers
+/// `Manifest` with the server (placeholders only), the agent, the
+/// instructions and the env, and a session spawned with them there has the
+/// flags in its argv (unless the remote claude lacks them: then `Applied`
+/// says so) and never the secret.
 #[test]
-fn t9_remote_plugin_bundle() {
+fn t9_remote_plugin_manifest() {
     let host = match test_host() {
         Some(h) => h,
         None => {
@@ -418,50 +444,69 @@ fn t9_remote_plugin_bundle() {
             return;
         }
     };
-    // The smallest valid zip: an empty end-of-central-directory record.
-    let zip: Vec<u8> = [b'P', b'K', 5, 6].into_iter().chain([0u8; 18]).collect();
     let sha = {
         use sha2::Digest;
-        format!("{:x}", sha2::Sha256::digest(&zip))
+        format!("{:x}", sha2::Sha256::digest(T9_MANIFEST.as_bytes()))
     };
-    let octal: String = zip.iter().map(|b| format!("\\{b:03o}")).collect();
+    let octal: String = T9_MANIFEST.bytes().map(|b| format!("\\{b:03o}")).collect();
     let seed = format!(
         "d=\"${{XDG_CACHE_HOME:-$HOME/.cache}}/claudio/plugins\" && mkdir -p \"$d\" && chmod 700 \"$d\" \
-         && printf '{octal}' > \"$d/claudio-t9.zip\" && printf '{sha}\\n' > \"$d/claudio-t9.sha256\" \
-         && echo \"$d/claudio-t9.zip\""
+         && printf '{octal}' > \"$d/claudio-t9.toml\" && printf '{sha}\\n' > \"$d/claudio-t9.sha256\" \
+         && echo ok"
     );
-    let (remote_zip, ok) = ssh_run(&host, &seed);
-    assert!(ok, "could not seed the remote cache");
+    let (out, ok) = ssh_run(&host, &seed);
+    assert!(ok && out.contains("ok"), "could not seed the remote cache");
 
     let mut child = Command::new(BINARY)
-        .args(["__remote-bundle", &host, "/tmp", "claudio-t9", &sha])
+        .args([
+            "__remote-manifest",
+            &host,
+            "/tmp",
+            "claudio-t9",
+            &sha,
+            "T9_TOKEN",
+        ])
         .stdout(std::process::Stdio::piped())
         .spawn()
-        .expect("run claudio __remote-bundle");
+        .expect("run claudio __remote-manifest");
     let stdout = child.stdout.take().expect("piped stdout");
     let mut lines = std::io::BufRead::lines(std::io::BufReader::new(stdout));
     let reply: serde_json::Value =
-        serde_json::from_str(&lines.next().expect("a Bundle reply").unwrap()).unwrap();
-    assert_eq!(reply["path"].as_str(), Some(remote_zip.as_str()), "{reply}");
-    assert_eq!(reply["installed"], false);
-    println!("[t9] Bundled {{ path: {remote_zip} }} ✓");
+        serde_json::from_str(&lines.next().expect("an Applied reply").unwrap()).unwrap();
+    let mcp = reply["mcp_config"].as_str().expect("an MCP config");
+    assert!(
+        mcp.contains("${T9_TOKEN}") && !mcp.contains("t9-secret"),
+        "{reply}"
+    );
+    assert!(
+        reply["agents"]
+            .as_str()
+            .is_some_and(|a| a.contains("t9-agent")),
+        "{reply}"
+    );
+    assert_eq!(reply["instructions"], "t9 instructions");
+    assert_eq!(reply["env"], serde_json::json!([["T9_REGION", "eu"]]));
+    println!("[t9] Applied {{ mcp_config, agents, instructions, env }} ✓");
 
     let pid = lines.next().expect("a pid").unwrap();
     let pid = pid.strip_prefix("pid=").expect("pid=<n>").to_owned();
     let (args, _) = ssh_run(&host, &format!("ps -o args= -p {pid}"));
-    let (help, _) = ssh_run(
-        &host,
-        "$HOME/.local/bin/claude --help 2>/dev/null || claude --help",
-    );
-    if help.contains("--plugin-dir") {
+    let dropped: Vec<&str> = reply["dropped"]
+        .as_array()
+        .map(|d| d.iter().filter_map(|f| f.as_str()).collect())
+        .unwrap_or_default();
+    assert!(!args.contains("t9-secret"), "argv: {args}");
+    if dropped.is_empty() {
         assert!(
-            args.contains(&format!("--plugin-dir {remote_zip}")),
+            args.contains("--agents") && args.contains("--mcp-config"),
             "argv: {args}"
         );
-        println!("[t9] the remote session's argv has the bundle ✓");
+        println!("[t9] the remote session's argv has --agents and --mcp-config ✓");
     } else {
-        assert!(!args.contains("--plugin-dir"), "argv: {args}");
-        println!("[t9] the remote claude has no --plugin-dir: left out ✓");
+        for flag in &dropped {
+            assert!(!args.contains(flag), "argv: {args}");
+        }
+        println!("[t9] the remote claude lacks {dropped:?}: left out ✓");
     }
     for _ in lines {}
     assert!(child.wait().expect("wait for claudio").success());
`````
Run: `umask 022; cargo build --features diag && env -u CLAUDIO_SSH_TEST_HOST cargo test --features diag --test remote -- --test-threads=1`
Expected: `test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` (t9 prints `SKIP t9`; `__remote-manifest` does not exist yet, so the gated run would fail with `unknown`/`usage`).

Step 2: implement
`````diff
diff --git a/src/client.rs b/src/client.rs
index acd007a..90e561f 100644
--- a/src/client.rs
+++ b/src/client.rs
@@ -70,8 +70,8 @@ const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
 const SPAWN_TIMEOUT: Duration = Duration::from_secs(60);
 /// `UpdateClaude` runs up to 300 s on the daemon, plus the re-probe.
 const UPDATE_TIMEOUT: Duration = Duration::from_secs(330);
-/// A `Bundle` or `Manifest` download may take the daemon's 120 s, after
-/// waiting for one already running.
+/// A `Manifest` download may take the daemon's 120 s, after waiting for
+/// one already running.
 const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(250);
 
 /// Something the daemon sent without being asked.
@@ -402,7 +402,7 @@ impl Client {
         let timeout = match msg {
             Msg::Spawn(_) | Msg::SpawnShell(_) => SPAWN_TIMEOUT,
             Msg::UpdateClaude { .. } => UPDATE_TIMEOUT,
-            Msg::Bundle { .. } | Msg::Manifest { .. } => DOWNLOAD_TIMEOUT,
+            Msg::Manifest { .. } => DOWNLOAD_TIMEOUT,
             _ => REQUEST_TIMEOUT,
         };
         let queued = self.enqueue(msg);
diff --git a/src/main.rs b/src/main.rs
index fca2197..e3e68a4 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -84,14 +84,15 @@ fn main() -> std::process::ExitCode {
         return std::process::ExitCode::FAILURE;
     }
 
-    // `claudio __remote-bundle HOST CWD NAME SHA256` – ask HOST's daemon for a
-    // pre-seeded plugin bundle, spawn claude with it, print both.
+    // `claudio __remote-manifest HOST CWD NAME SHA256 SECRET` – ask HOST's
+    // daemon to apply a pre-seeded plugin manifest, spawn claude with what it
+    // applied, print both.
     #[cfg(feature = "diag")]
-    if argv.get(1).map(String::as_str) == Some("__remote-bundle") {
-        if let [_, _, host, cwd, name, sha256] = argv.as_slice() {
-            return remote::diag::remote_bundle_cmd(host, cwd, name, sha256);
+    if argv.get(1).map(String::as_str) == Some("__remote-manifest") {
+        if let [_, _, host, cwd, name, sha256, secret] = argv.as_slice() {
+            return remote::diag::remote_manifest_cmd(host, cwd, name, sha256, secret);
         }
-        eprintln!("usage: claudio __remote-bundle <host> <cwd> <name> <sha256>");
+        eprintln!("usage: claudio __remote-manifest <host> <cwd> <name> <sha256> <secret>");
         return std::process::ExitCode::FAILURE;
     }
 
diff --git a/src/paths.rs b/src/paths.rs
index 318d68b..9d9c3b6 100644
--- a/src/paths.rs
+++ b/src/paths.rs
@@ -88,7 +88,7 @@ pub fn plugins_cache() -> PathBuf {
 }
 
 /// `$XDG_CACHE_HOME/claudio`, else `~/.cache/claudio`: downloads this host
-/// keeps (the plugin bundles).
+/// keeps (the plugin manifests).
 pub fn cache_dir() -> PathBuf {
     std::env::var_os("XDG_CACHE_HOME")
         .filter(|v| !v.is_empty())
diff --git a/src/proto.rs b/src/proto.rs
index 003a4ee..bf3dba2 100644
--- a/src/proto.rs
+++ b/src/proto.rs
@@ -207,18 +207,6 @@ pub enum Msg {
         #[serde(default)]
         limit: u32,
     },
-    /// Make plugin `name`'s bundle ready on this host: skipped when the
-    /// Claude Code plugin `installed_id` is installed here, else the cached
-    /// zip whose SHA-256 is `sha256`, downloaded from `url` (https) when
-    /// missing. Answered by `Bundled`, or `Error "bundle <name>: …"`. An
-    /// older daemon answers `Error "unsupported op…"`.
-    Bundle {
-        name: String,
-        url: String,
-        sha256: String,
-        #[serde(default)]
-        installed_id: String,
-    },
     /// Apply proxy plugin `name`'s manifest on this host: the TOML at `url`
     /// (https) whose SHA-256 is `sha256`, cached once downloaded. `secrets`
     /// are the names (never the values) of the plugin's proxy env, which
@@ -300,15 +288,6 @@ pub enum Msg {
         total: u32,
         truncated: bool,
     },
-    /// Reply to `Bundle`: the zip's path on this host (for `--plugin-dir`),
-    /// or `installed` (the plugin is installed in Claude Code; no bundle).
-    Bundled {
-        name: String,
-        #[serde(default)]
-        path: Option<String>,
-        #[serde(default)]
-        installed: bool,
-    },
     /// Reply to `Manifest`.
     Applied(Applied),
     Ok,
diff --git a/src/proxy/plugins.rs b/src/proxy/plugins.rs
index c746da3..9a0042a 100644
--- a/src/proxy/plugins.rs
+++ b/src/proxy/plugins.rs
@@ -1,11 +1,12 @@
 //! Session plugins served by a proxy profile (`GET /v1/claudio/plugins`).
 //!
-//! A plugin is a pinned bundle (a zip claude loads with `--plugin-dir`) and
-//! the env its MCP servers and hooks need. The env holds secrets: the list
-//! is cached per profile in `~/.config/claudio/plugins.json` (mode 0600,
-//! like `config.toml`), so a session can get it when the proxy is
-//! unreachable, and never goes to argv, the journal, `state.json`, logs or
-//! notices. `Debug` prints names and key counts only.
+//! A plugin is a pinned manifest (`crate::plugins::manifest`: what its
+//! sessions get, and the plugins it installs) and the env its MCP servers
+//! need. The env holds secrets: the list is cached per profile in
+//! `~/.config/claudio/plugins.json` (mode 0600, like `config.toml`), so a
+//! session can get it when the proxy is unreachable, and never goes to
+//! argv, the journal, `state.json`, logs or notices. `Debug` prints names
+//! and key counts only.
 //!
 //! Everything a proxy sends is checked by [`admit`] before it is used.
 
@@ -68,8 +69,8 @@ pub struct Plugin {
     pub version: String,
     pub bundle_url: String,
     pub bundle_sha256: String,
-    /// The Claude Code plugin id (`<plugin>@<marketplace>`) that makes the
-    /// bundle redundant when installed; empty when none does.
+    /// Accepted and ignored (the zip bundles' installed-plugin check; a
+    /// manifest names its plugins itself).
     #[serde(default)]
     pub installed_id: String,
     /// Sorted by key.
diff --git a/src/remote/diag.rs b/src/remote/diag.rs
index d1913f3..5a6ff08 100644
--- a/src/remote/diag.rs
+++ b/src/remote/diag.rs
@@ -205,15 +205,23 @@ pub fn remote_session_cmd(host: &str, cwd: &str) -> ExitCode {
     })
 }
 
-/// `claudio __remote-bundle HOST CWD NAME SHA256`
+/// `claudio __remote-manifest HOST CWD NAME SHA256 SECRET`
 ///
-/// Asks `HOST`'s daemon for plugin `NAME`'s bundle with hash `SHA256` (from
-/// a URL nothing listens on, so only a pre-seeded cache answers), prints
-/// the reply as JSON, then spawns claude in `CWD` with `--plugin-dir
-/// <path>`, prints `pid=<pid>`, keeps the session 5 s (for the caller to
-/// look at its argv) and kills it.
-pub fn remote_bundle_cmd(host: &str, cwd: &str, name: &str, sha256: &str) -> ExitCode {
-    use crate::proto::{Msg, SpawnSpec};
+/// Asks `HOST`'s daemon to apply plugin `NAME`'s manifest with hash
+/// `SHA256` (from a URL nothing listens on, so only a pre-seeded cache
+/// answers), with `SECRET` the name of the one secret it is given; prints
+/// the `Applied` reply as JSON, then spawns claude in `CWD` with what it
+/// applied (its flags, its env, the secret set to `t9-secret`, its
+/// instructions), prints `pid=<pid>`, keeps the session 5 s (for the caller
+/// to look at its argv) and kills it.
+pub fn remote_manifest_cmd(
+    host: &str,
+    cwd: &str,
+    name: &str,
+    sha256: &str,
+    secret: &str,
+) -> ExitCode {
+    use crate::proto::{Msg, SessionExtensions, SpawnSpec};
     let rt = match make_runtime() {
         Ok(r) => r,
         Err(code) => return code,
@@ -231,43 +239,48 @@ pub fn remote_bundle_cmd(host: &str, cwd: &str, name: &str, sha256: &str) -> Exi
                 return ExitCode::FAILURE;
             }
         };
-        let bundle = Msg::Bundle {
+        let request = Msg::Manifest {
             name: name.to_owned(),
-            url: "https://127.0.0.1:9/never.zip".to_owned(),
+            url: "https://127.0.0.1:9/never.toml".to_owned(),
             sha256: sha256.to_owned(),
-            installed_id: String::new(),
+            secrets: vec![secret.to_owned()],
+            disabled: Vec::new(),
         };
-        let path = match client.request(bundle).await {
-            Ok(Msg::Bundled {
-                path, installed, ..
-            }) => {
-                let reply = serde_json::json!({ "path": path, "installed": installed });
-                println!("{reply}");
-                path
-            }
+        let applied = match client.request(request).await {
+            Ok(Msg::Applied(applied)) => applied,
             Ok(other) => {
-                eprintln!("unexpected Bundle reply: {other:?}");
+                eprintln!("unexpected Manifest reply: {other:?}");
                 return ExitCode::FAILURE;
             }
             Err(e) => {
-                eprintln!("Bundle request failed: {e}");
+                eprintln!("Manifest request failed: {e}");
                 return ExitCode::FAILURE;
             }
         };
-        let Some(path) = path else {
-            return ExitCode::SUCCESS;
-        };
+        println!("{}", serde_json::to_string(&applied).unwrap_or_default());
+        let mut args = Vec::new();
+        if let Some(agents) = &applied.agents {
+            args.extend(["--agents".to_owned(), agents.clone()]);
+        }
+        if let Some(mcp) = &applied.mcp_config {
+            args.extend(["--mcp-config".to_owned(), mcp.clone()]);
+        }
+        let mut env = applied.env.clone();
+        env.push((secret.to_owned(), "t9-secret".to_owned()));
         let id = uuid::Uuid::new_v4();
         let spec = SpawnSpec {
             id,
             cwd: cwd.to_owned(),
             name: None,
-            args: vec!["--plugin-dir".to_owned(), path],
-            env: vec![],
+            args,
+            env,
             rows: 24,
             cols: 80,
             ephemeral: true,
-            extensions: None,
+            extensions: Some(SessionExtensions {
+                instructions: applied.instructions.clone(),
+                ..Default::default()
+            }),
         };
         match client.request(Msg::Spawn(spec)).await {
             Ok(Msg::Spawned { pid, .. }) => {
`````
Run: `umask 022; cargo build --features diag && env -u CLAUDIO_SSH_TEST_HOST cargo test --features diag --test remote -- --test-threads=1; cargo test --locked --bin claudio`
Expected: `test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`, then `test result: ok. 1093 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` (the bundle round-trip test is gone)

Step 3: commit
`/usr/bin/git commit -m "refactor: drop the zip bundle ops; t9 applies a manifest on the remote" -- src/proto.rs src/client.rs src/main.rs src/remote/diag.rs tests/remote.rs src/proxy/plugins.rs src/paths.rs`

Interfaces this task provides to later tasks: `remote::diag::remote_manifest_cmd(host, cwd, name, sha256, secret)` (diag build).

### Task 9: manager test (AC4)
Model: standard   Repo: /volumes/repos/claudio   Files: tests/manager.rs
Goal: AC4 end to end:
- A fake proxy serves a manifest over loopback http, under D1's knob. The manifest has one
  marketplace, a plugin to install and one present, an http server with a secret, a second
  server, an agent and one the member has, and instructions.
- The setup screen comes first, queued behind the wizard, and nothing is applied before it.
- After `extra` is turned off and Enter pressed, the session's argv has `--agents` and
  `--mcp-config` (placeholders only, no `extra`) and its env has the secret.
- The context file has the instructions, and the fake recorded `marketplace add` and
  `install --json`, never `-y`.
- After `r`, the details list `1 installed, 1 present, 0 installing, 1 skipped` and
  `agent:mine: yours`.
- Turned off, the next session has none of it.
Patch: .delivery/2026-11-06-plugin-manifest/patches/task-9.patch

Step 1: write the test (it exercises Tasks 6–7, so it passes once written; there is no Step 2)
`````diff
diff --git a/tests/manager.rs b/tests/manager.rs
index e9f9350..82f14a6 100644
--- a/tests/manager.rs
+++ b/tests/manager.rs
@@ -2757,3 +2757,321 @@ fn test_extensions_screen_turns_extensions_off_and_saves_it() {
     tui.send_keys(ALT_Q);
     tui.wait_exit(WAIT);
 }
+
+// ── Proxy plugins ────────────────────────────────────────────────────────────
+
+/// The fake plugin and the secret its proxy env carries.
+const PLUGIN: &str = "vocdoni-test";
+const PLUGIN_KEY: &str = "VOCDONI_TEST_TOKEN";
+
+/// The manifest the fake proxy serves: plugins `tool` (to install) and
+/// `have` (installed already), servers `kb` (needs the secret) and
+/// `extra`, agents `reviewer` and `mine` (the member's own), instructions.
+const PLUGIN_MANIFEST: &str = r#"format = 1
+name = "vocdoni-test"
+version = "1.0.0"
+description = "Test setup"
+instructions = "Manifest instructions for the test."
+
+[[marketplace]]
+name = "market"
+source = "github:acme/market"
+
+[[plugin]]
+name = "tool"
+marketplace = "market"
+
+[[plugin]]
+name = "have"
+marketplace = "market"
+
+[[mcp]]
+name = "kb"
+type = "http"
+url = "https://kb.example.org/mcp"
+headers = { Authorization = "Bearer ${VOCDONI_TEST_TOKEN}" }
+secrets = ["VOCDONI_TEST_TOKEN"]
+
+[[mcp]]
+name = "extra"
+type = "sse"
+url = "https://extra.example.org/sse"
+
+[[agent]]
+name = "reviewer"
+description = "Reviews"
+prompt = "Review the change."
+
+[[agent]]
+name = "mine"
+description = "Mine"
+prompt = "Mine."
+"#;
+
+/// A fake claude-proxy on loopback: `GET /v1/claudio/plugins` lists
+/// [`PLUGIN`] with its manifest at `GET /vocdoni-test.toml` (plain http on
+/// loopback, which a debug build takes with `CLAUDIO_TEST_LOOPBACK_HTTP`);
+/// every other path is a 404. Returns its `host:port`.
+fn fake_plugin_proxy() -> String {
+    use sha2::Digest;
+    use std::io::{Read, Write};
+    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind fake proxy");
+    let addr = listener.local_addr().unwrap().to_string();
+    let sha = format!("{:x}", sha2::Sha256::digest(PLUGIN_MANIFEST.as_bytes()));
+    let list = serde_json::json!({"plugins": [{
+        "name": PLUGIN, "version": "1.0.0",
+        "bundle": {"url": format!("http://{addr}/{PLUGIN}.toml"), "sha256": sha},
+        "env": {PLUGIN_KEY: "s3cret"}}]})
+    .to_string();
+    thread::spawn(move || {
+        for stream in listener.incoming() {
+            let Ok(mut stream) = stream else { continue };
+            let mut head = Vec::new();
+            let mut byte = [0u8; 1];
+            while !head.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap_or(0) == 1 {
+                head.push(byte[0]);
+            }
+            let line = String::from_utf8_lossy(&head);
+            let (status, reply) = if line.starts_with("GET /v1/claudio/plugins ") {
+                ("200 OK", list.as_str())
+            } else if line.starts_with(&format!("GET /{PLUGIN}.toml ")) {
+                ("200 OK", PLUGIN_MANIFEST)
+            } else {
+                ("404 Not Found", "{}")
+            };
+            let _ = write!(
+                stream,
+                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n\
+                 Content-Length: {}\r\nConnection: close\r\n\r\n{reply}",
+                reply.len()
+            );
+        }
+    });
+    addr
+}
+
+/// A fake claude whose `--help` lists `--agents` and `--mcp-config`. Its
+/// `plugin …` runs are logged to `<dumps>/plugin.log` and act on
+/// `$CLAUDE_CONFIG_DIR` as Claude Code would (the marketplace is added, the
+/// plugin installed beside `have@market`). Every other launch is recorded
+/// in `<dumps>/<n>/` (`argv`, one argument per line, and `env`) before the
+/// usual banner and `cat`.
+fn write_plugin_claude(h: &ManagerHarness, dumps: &std::path::Path) {
+    use std::os::unix::fs::PermissionsExt;
+    fs::create_dir_all(dumps.join("launches")).unwrap();
+    let dumps = dumps.display();
+    let script = format!(
+        r#"#!/bin/sh
+[ "$1" = "--version" ] && echo 'claude 0.0.0-fake' && exit 0
+if [ "$1" = "--help" ]; then
+  printf '  --agents <json>  Agents\n  --mcp-config <configs...>  MCP\n'
+  exit 0
+fi
+if [ "$1" = "plugin" ]; then
+  echo "$*" >> "{dumps}/plugin.log"
+  p="$CLAUDE_CONFIG_DIR/plugins"
+  mkdir -p "$p"
+  [ "$2" = "marketplace" ] && printf '{{"market":{{}}}}\n' > "$p/known_marketplaces.json"
+  [ "$2" = "install" ] && printf '{{"version":2,"plugins":{{"have@market":[{{"scope":"user"}}],"%s":[{{"scope":"user"}}]}}}}\n' "$3" > "$p/installed_plugins.json"
+  echo '{{"ok":true}}'
+  exit 0
+fi
+d="{dumps}/launches/$(ls "{dumps}/launches" | wc -l | tr -d ' ')"
+mkdir -p "$d"
+printf '%s\n' "$@" > "$d/argv"
+env > "$d/env"
+echo "FAKE_CLAUDE_BANNER pid=$$"
+exec cat
+"#
+    );
+    fs::write(&h.fake_claude, script).unwrap();
+    fs::set_permissions(&h.fake_claude, fs::Permissions::from_mode(0o755)).unwrap();
+}
+
+/// Launch `n`'s recorded argv and env, once both are written.
+fn launch_dump(dumps: &std::path::Path, n: usize) -> (Vec<String>, String) {
+    let dir = dumps.join("launches").join(n.to_string());
+    let deadline = Instant::now() + WAIT;
+    loop {
+        let argv = fs::read_to_string(dir.join("argv"));
+        let env = fs::read_to_string(dir.join("env"));
+        if let (Ok(argv), Ok(env)) = (argv, env) {
+            if !env.is_empty() {
+                return (argv.lines().map(str::to_owned).collect(), env);
+            }
+        }
+        assert!(
+            Instant::now() < deadline,
+            "launch {n} not recorded in {dir:?}"
+        );
+        thread::sleep(Duration::from_millis(100));
+    }
+}
+
+/// The value after `flag` in `argv`.
+fn flag_value<'a>(argv: &'a [String], flag: &str) -> Option<&'a str> {
+    let at = argv.iter().position(|a| a == flag)?;
+    argv.get(at + 1).map(String::as_str)
+}
+
+/// The text of every session context file the daemon wrote.
+fn context_files(h: &ManagerHarness) -> String {
+    fn walk(dir: &std::path::Path, out: &mut String) {
+        for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
+            let path = entry.path();
+            if path.is_dir() {
+                walk(&path, out);
+            } else if path.parent().is_some_and(|p| p.ends_with("context")) {
+                out.push_str(&fs::read_to_string(&path).unwrap_or_default());
+            }
+        }
+    }
+    let mut out = String::new();
+    walk(&h.runtime_dir, &mut out);
+    out
+}
+
+/// A proxy plugin's manifest: the setup screen comes first and nothing is
+/// applied before it closes; after Enter a new session gets the agents and
+/// the MCP servers it kept (placeholders on argv, the secret in the env),
+/// the instructions in its context, and the plugin is installed through
+/// the CLI; the extensions screen shows each item's outcome; turned off,
+/// the next session has none of it.
+#[test]
+fn test_plugin_manifest_setup_then_sessions_get_what_was_kept() {
+    let h = ManagerHarness::new();
+    let dumps = h.root.join("dumps");
+    write_plugin_claude(&h, &dumps);
+    let claude_config = h.root.join("claude-config");
+    fs::create_dir_all(claude_config.join("agents")).unwrap();
+    fs::create_dir_all(claude_config.join("plugins")).unwrap();
+    fs::write(claude_config.join("agents/mine.md"), "my own agent").unwrap();
+    fs::write(
+        claude_config.join("plugins/installed_plugins.json"),
+        r#"{"version":2,"plugins":{"have@market":[{"scope":"user"}]}}"#,
+    )
+    .unwrap();
+    let proxy = fake_plugin_proxy();
+    let mut tui = h.start_tui_with(|cmd| {
+        cmd.env("CLAUDIO_PROXY_URL", format!("tok@{proxy}"));
+        cmd.env("XDG_CACHE_HOME", h.root.join("xdg-cache"));
+        cmd.env("CLAUDE_CONFIG_DIR", &claude_config);
+        cmd.env("CLAUDIO_TEST_LOOPBACK_HTTP", "1");
+    });
+
+    // The wizard is open at start; the setup screen waits its turn.
+    tui.wait_for("New session", Region::Screen, WAIT);
+    tui.send_keys(ESC);
+    tui.wait_for("Set up vocdoni-test 1.0.0", Region::Screen, WAIT);
+    tui.wait_for(
+        "[x] kb (http https://kb.example.org/mcp)",
+        Region::Screen,
+        WAIT,
+    );
+    tui.wait_for("[x] reviewer: Reviews", Region::Screen, WAIT);
+    assert!(
+        !fs::read_to_string(dumps.join("plugin.log")).is_ok_and(|l| !l.is_empty()),
+        "nothing applied before the setup closes"
+    );
+    // Past the arming; `extra` (the fourth item) off, then Enter.
+    thread::sleep(Duration::from_millis(1200));
+    for _ in 0..3 {
+        tui.send_keys(DOWN_ARROW);
+    }
+    tui.send_keys(b" ");
+    tui.wait_for(
+        "[ ] extra (sse https://extra.example.org/sse)",
+        Region::Screen,
+        WAIT,
+    );
+    tui.send_keys(ENTER);
+    tui.wait_for(
+        &format!("plugin {PLUGIN} set up (saved)"),
+        Region::StatusBar,
+        WAIT,
+    );
+    let config = fs::read_to_string(h.config_home.join("claudio/config.toml")).unwrap();
+    assert!(
+        config.contains(&format!(
+            "[plugins.{PLUGIN}]\ndisabled = [\"mcp:extra\"]\nreviewed = \"1.0.0\""
+        )),
+        "{config}"
+    );
+
+    tui.send_keys(ALT_N);
+    wizard_pick_dir(&mut tui, &h.dirs[0]);
+    let (argv, env) = launch_dump(&dumps, 0);
+    let agents = flag_value(&argv, "--agents").expect("--agents");
+    assert!(
+        agents.contains("\"reviewer\"") && !agents.contains("\"mine\""),
+        "{agents}"
+    );
+    let mcp = flag_value(&argv, "--mcp-config").expect("--mcp-config");
+    assert!(mcp.contains("Bearer ${VOCDONI_TEST_TOKEN}"), "{mcp}");
+    assert!(!mcp.contains("extra"), "a server turned off: {mcp}");
+    assert!(
+        !argv.iter().any(|a| a.contains("s3cret")),
+        "no secret in argv"
+    );
+    assert!(env.contains(&format!("{PLUGIN_KEY}=s3cret")), "{env}");
+    let context = context_files(&h);
+    assert!(
+        context.contains("Manifest instructions for the test."),
+        "{context}"
+    );
+
+    // The plugin is installed in the background, never with -y.
+    let deadline = Instant::now() + WAIT;
+    let log = loop {
+        let log = fs::read_to_string(dumps.join("plugin.log")).unwrap_or_default();
+        if log.contains("plugin install tool@market --scope user --json") {
+            break log;
+        }
+        assert!(Instant::now() < deadline, "no install: {log}");
+        thread::sleep(Duration::from_millis(100));
+    };
+    assert!(
+        log.contains("plugin marketplace add acme/market --scope user --json"),
+        "{log}"
+    );
+    assert!(
+        !log.contains("have@market") && !log.contains(" -y"),
+        "{log}"
+    );
+    let installed = claude_config.join("plugins/installed_plugins.json");
+    while !fs::read_to_string(&installed).is_ok_and(|s| s.contains("tool@market")) {
+        assert!(Instant::now() < deadline, "the install never finished");
+        thread::sleep(Duration::from_millis(100));
+    }
+    // The daemon notes the outcome once the CLI exits.
+    thread::sleep(Duration::from_millis(500));
+
+    // The extensions screen: `r` asks again, and each outcome shows.
+    tui.send_keys(ALT_I);
+    tui.wait_for(&format!("[x] {PLUGIN}"), Region::Screen, WAIT);
+    for _ in 0..4 {
+        tui.send_keys(DOWN_ARROW);
+    }
+    tui.wait_for("1 items disabled (Enter to change)", Region::Screen, WAIT);
+    tui.send_keys(b"r");
+    tui.wait_for(
+        "local: ready · 1 installed, 1 present, 0 installing, 1 skipped",
+        Region::Screen,
+        WAIT,
+    );
+    tui.wait_for("agent:mine: yours", Region::Screen, WAIT);
+    // Turned off: the next session has none of it.
+    tui.send_keys(b" ");
+    tui.wait_for(&format!("[ ] {PLUGIN}"), Region::Screen, WAIT);
+    tui.send_keys(ESC);
+    tui.wait_until(|s| !s.contains("Space toggle", Region::Screen), WAIT);
+    tui.send_keys(ALT_N);
+    wizard_pick_dir(&mut tui, &h.dirs[1]);
+    let (argv, env) = launch_dump(&dumps, 1);
+    assert!(
+        !argv.iter().any(|a| a == "--agents" || a == "--mcp-config"),
+        "{argv:?}"
+    );
+    assert!(!env.contains(PLUGIN_KEY), "{env}");
+    tui.quit(WAIT);
+}
`````
Run: `umask 022; cargo test --test manager test_plugin_manifest -- --test-threads=1`
Expected: `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 50 filtered out` (3/3 stable in the scratch; about 5 s).

Step 2: implement
Nothing to implement.
Run: `umask 022; cargo test --test manager -- --test-threads=1`
Expected: `test result: ok. 51 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`

Step 3: commit
`/usr/bin/git commit -m "test(manager): a plugin manifest from setup screen to session argv" -- tests/manager.rs`

Interfaces this task provides to later tasks: none.

### Task 10: docs
Model: fast   Repo: /volumes/repos/claudio   Files: CLAUDE.md, README.md, docs/extensions.md
Goal: every zip-era mention goes. Updated:
- CLAUDE.md:
  - daemon section: `manifest.rs` replaces `bundle.rs`;
  - tui: `plugin_setup.rs` and `plugins.rs`;
  - `proxy/plugins.rs`, with the loopback test knob;
  - the new `plugins/` module;
  - the secrets rule, the no-`-y` rule and the fail-safe paragraph.
- README.md: the plugins paragraph, the extensions screen keys and `[plugins.<name>]` in the
  config example.
- docs/extensions.md: the manifest format, the setup screen, what the daemon does, what a
  session gets, and the details lines.

No env var for users is added, so `cli::HELP_APPENDIX` is unchanged.
Patch: .delivery/2026-11-06-plugin-manifest/patches/task-10.patch

Step 1: write the failing test
No test (docs only).
Run: `true`
Expected: nothing.

Step 2: implement
`````diff
diff --git a/CLAUDE.md b/CLAUDE.md
index 97ead94..5a150d9 100644
--- a/CLAUDE.md
+++ b/CLAUDE.md
@@ -142,15 +142,29 @@ daemons coexist.
   bounded output, sanitized text, no textconv/ext-diff, repo env scrubbed.
 - `update.rs` — `claude update` (or the official installer) on request, in its
   own process group, killed as a group on timeout.
-- `bundle.rs` — `Bundle` (a proxy plugin's zip for `--plugin-dir`): checks
-  the op itself (name `[a-z0-9-]{1,40}`, 64-hex hash, https), skips it when
-  `$CLAUDE_CONFIG_DIR/plugins/installed_plugins.json` has the plugin, else
-  `<cache>/plugins/<name>.zip` whose `.sha256` sidecar matches, else
-  downloads it (30 s connect, 120 s, 64 MiB, https redirects only) to a temp
-  file, verifies, renames into place. 0700 dir, ≤ 32 zips, one download per
-  name at a time, no partial file on failure. Replies from a spawned task.
+- `manifest.rs` — `Manifest` (a proxy plugin's manifest, parsed by
+  `crate::plugins::manifest`): checks the op itself (name `[a-z0-9-]{1,40}`,
+  64-hex hash, https, ≤ 64 secret names by `key_ok`, ≤ 256 disabled item
+  ids), keeps `<cache>/plugins/<name>.toml` + `.sha256` (downloaded once:
+  30 s connect, 120 s, 256 KiB, https redirects only, temp file, verified,
+  renamed; 0700 dir, ≤ 32 manifests, one download per name), and
+  replies `Applied` before any install runs: the `--mcp-config` and
+  `--agents` JSON (placeholders only), the sanitized instructions, the
+  `[env]`, and per id `installed` / `present` / `skipped` (`installing`,
+  `yours`, `missing secret X`, a failure). It reads Claude Code's files
+  under `$CLAUDE_CONFIG_DIR` else `~/.claude` (`.claude.json`,
+  `plugins/known_marketplaces.json`, `plugins/installed_plugins.json`,
+  `agents/`; regular files of this uid, ≤ 16 MiB, never logged) and never
+  writes there itself: one task per manifest hash runs `claude plugin
+  marketplace add … --json` / `claude plugin install … --json` (never `-y`),
+  each holding `Daemon::updating` (shared with `claude update`), stdin null,
+  output capped, markers scrubbed, `GIT_TERMINAL_PROMPT=0`, own process
+  group, 120 s. Outcomes are remembered per (hash, id): installed (daemon's
+  life), `needs manual install` (daemon's life), a failure (10 min).
   `Daemon::claude_flags` (one `claude --help`) says whether claude knows
-  `--plugin-dir`; `command()` drops the pairs when it does not.
+  `--plugin-dir`, `--agents`, `--mcp-config`; `command()` drops such a pair
+  when it does not, and `Applied.dropped` names them. Stale zips of a
+  pre-manifest build are removed at daemon start.
 - `host.rs`, `stats.rs` — host facts for `Welcome`, directory listing, CPU and
   memory samples. `ctl.rs` — `claudio daemon status|stop|restart` (status
   shows the claude path and version the daemon uses).
@@ -225,9 +239,19 @@ is idle (checked every 2 min).
   `Effect::SetExtension` (host; externals) and `Effect::SaveExtension`
   (`mod.rs` writes `[extensions] name = on` with `config::save_extension`,
   a minimal line edit of config.toml, never a whole-file rewrite); a
-  plugin's toggle also updates `App::plugins.config`, and `r` refetches the
-  plugin lists. Drawn by `ui/extensions.rs`. The leader keys `u` and `e`
-  are reserved.
+  plugin's toggle (Space) also updates `App::plugins.config`, Enter on a
+  plugin's row opens its setup screen, and `r` refetches the plugin lists
+  and manifests and asks every host again. Drawn by `ui/extensions.rs`.
+  The leader keys `u` and `e` are reserved.
+- `plugin_setup.rs` — a proxy plugin's setup screen, pure: every item of
+  its manifest with a checkbox (Space, `a`, `n`, Enter saves to
+  `[plugins.<name>]` through `Effect::SavePluginChoices` and
+  `config::save_plugin_choices`, Esc). It opens on first sight (no table
+  yet; Esc then defers for the run: nothing written or sent) and from the
+  extensions screen; it waits its turn in the confirm queue (`Queued`) and
+  swallows every key. `--plain` holds its spawn (`App::plain_pending`)
+  until the profile's new manifests arrived (≤ 20 s) and their screens
+  closed. Drawn by `ui/plugin_setup.rs`.
 - `leader.rs` — Alt+z: the leader popup, action input, list picker,
   documents (a reader view) and paste (attached claude session with
   bracketed paste on only, never Enter). `builtins.rs` — the built-in
@@ -253,11 +277,18 @@ is idle (checked every 2 min).
   (`state.json`).
 - `proxy_state.rs` — per-profile proxy stats and per-session credential cache.
 - `plugins.rs` — proxy plugins, pure: each profile's list (`plugins.json` at
-  start, `Effect::FetchPlugins` at start, every 15 min and on `r`), the
-  registry's `Source::Plugin` rows, each `(host, plugin)` `Bundle` answer,
-  and what a spawn gets (`--plugin-dir` per ready bundle, the plugin env).
-  A spawn whose host has not answered for a bundle is deferred
-  (`Effect::SpawnWithProxy { bundles }`; `tui/mod.rs` waits ≤ 20 s).
+  start, `Effect::FetchPlugins` at start, every 15 min and on `r`), each
+  plugin's manifest (`Effect::FetchManifest`: fetched by the client, 256
+  KiB, hash-checked, memory only), the registry's `Source::Plugin` rows,
+  each `(host, plugin)` `Applied` answer (`ApplyState`; asked again every
+  30 s while it lists `installing`, ≤ 10 min), and what a spawn gets
+  (`compose`: per answered plugin, ≤ 8, `--agents` then `--mcp-config`
+  after every other arg; env layers `[env]`, then the secrets, then the
+  profile env; the instructions in `SessionExtensions.instructions`, cut at
+  8 KiB). A host is asked only once the member saved the plugin's choices.
+  A spawn whose host has not answered is deferred
+  (`Effect::SpawnWithProxy { plugins, pending }`; `tui/mod.rs` waits
+  ≤ 20 s).
 - `notifications.rs` — debounce logic for attention notifications.
 - `selection.rs` — copy-on-select (`[ui] copy_on_select`, default on): the
   pure `Selection` (anchor/head spans, cell/word/line unit, `contains`),
@@ -297,8 +328,20 @@ is idle (checked every 2 min).
 - `login.rs` — the browser sign-in (`/v1/claudio/login/start|poll`, device
   style: the member types the code; `--token` pastes one instead).
 - `plugins.rs` — `GET /v1/claudio/plugins`, `admit` (names, https, hash,
-  env keys a plugin may not set, caps), the `plugins.json` cache (0600, one
-  entry per profile; a 401/403 drops it). `Debug` never shows a value.
+  env keys a plugin may not set (`key_ok`), caps), the `plugins.json` cache
+  (0600, one entry per profile; a 401/403 drops it), `fetch_manifest`.
+  `Debug` never shows a value. `test_http_ok` lets plain http to
+  `127.0.0.1` through in a debug build run with
+  `CLAUDIO_TEST_LOOPBACK_HTTP=1` (the manager tests' fake proxy), never in
+  a release build.
+
+**`plugins/`** — `manifest.rs`, the one parser of a plugin manifest (TOML,
+≤ 256 KiB; marketplaces, plugins, MCP servers with their secret names,
+agents, instructions, env), shared by the client and the daemon: every cap,
+charset and `${NAME}` placeholder rule, item ids (`plugin:<name>@<market>`,
+`mcp:<name>`, `agent:<name>`, `instructions`, `env`), and the renderers
+(`mcp_json`, `agents_json`, checked against 64/32 KiB at parse time).
+Reasons name ids and fields, never values.
 - `api.rs` — HTTP client for `/v1/claudio/config`, `/models`, `/me/stats`,
   `/pool/health` and `/session` (which credential a claude session is on).
 
@@ -345,18 +388,25 @@ is idle (checked every 2 min).
   ambiguous — the VT parser may interpret it as two separate `ESC` sequences.
   Use the crossterm key-event encoding instead.
 - **Secrets never go in argv or state files.** Proxy tokens and plugin env
-  are passed only through `Spawn.env` over the Unix socket. The journal and
-  `state.json` never hold secrets (profile names only); `config.toml` and
+  are passed only through `Spawn.env` over the Unix socket; a `Manifest`
+  request carries secret names, never values. The journal and `state.json`
+  never hold secrets (profile names only); `config.toml` and
   `plugins.json` do, both mode 0600. Logs and notices name keys, never
-  values. `--plugin-dir` is added by claudio's spawn path only, never parsed
-  from the user's args.
+  values. The `--mcp-config` JSON on argv holds `${NAME}` placeholders
+  that claude expands from the env; `--agents`/`--mcp-config` are added by
+  claudio's spawn path only, never parsed from the user's args.
+- **Plugins are installed by the CLI, never with `-y`.** The daemon runs
+  `claude plugin … --json` and never writes `~/.claude` itself; a
+  marketplace-declared command (`shownCommand`) is left for a person.
 - **New request ops must fail safe on old daemons.** An old daemon answers an
   unknown op with `Error "unsupported op…"`; `App::older_daemon_notice` turns
   that into "run `claudio daemon restart`". Never add a field to an existing
   op that an old daemon would silently ignore into wrong behaviour (that is
-  why terminals are `SpawnShell`, not `Spawn{kind}`, and plugin bundles are
-  a `Bundle` op, not a `SpawnSpec` field: an old daemon's error becomes
-  `daemon too old` and the session starts without the bundle).
+  why terminals are `SpawnShell`, not `Spawn{kind}`, and plugin manifests
+  are a `Manifest` op, not a `SpawnSpec` field: an old daemon's error
+  becomes `daemon too old` and the session starts without the plugin;
+  `SessionExtensions.instructions` is `#[serde(default)]`, which an old
+  daemon ignores harmlessly).
 - **A daemon is replaced only by a newer build.** `freshness::compare` orders
   builds by version, then binary mtime; "different" is not enough, or two
   binaries (a release and a dev build) would replace each other's daemon
diff --git a/README.md b/README.md
index f3d4591..cfd5c1c 100644
--- a/README.md
+++ b/README.md
@@ -86,7 +86,7 @@ claudio                                         # new sessions now use the proxy
 
 `claudio proxy login` prints a page and a one-time code, opens the page in a browser when there is a display (`CLAUDIO_NO_BROWSER=1` only prints it), and waits while you sign in and type the code there. The proxy then hands claudio your own token, saved with your e-mail in the profile (`claudio proxy status` shows it). Without a URL it signs in again to the default profile's proxy. `--token`, or a proxy without browser sign-in, asks for a pasted token instead; a token is never an argument.
 
-The proxy can also serve **plugins**: a pinned bundle of skills, agents, hooks and MCP servers, plus the env (API keys) its servers need. Every session claudio opens with that proxy profile (manager, `--plain`, SSH hosts) loads each plugin with `--plugin-dir` and gets its env, unless you installed that plugin in Claude Code yourself. Nothing is installed into `~/.claude`. Each host's daemon downloads a bundle once per version into `~/.cache/claudio/plugins/` (https only, hash-checked, 64 MiB at most). The list and its env are cached in `~/.config/claudio/plugins.json` (mode 0600), so a session still gets them when the proxy is unreachable, and refreshed every 15 minutes. `claudio proxy logout` deletes them. Plugins are listed in the extensions screen (`Alt+i`), where you can turn one off for new sessions.
+The proxy can also serve **plugins**: a pinned manifest (a TOML file, hash-checked) that says what a member needs: marketplaces and the Claude Code plugins to install from them, MCP servers (with the secrets they need, which the proxy serves as env), subagents, instructions for every session and env. The first time claudio sees a manifest it shows every item in a setup screen, all on; you turn off what you don't want and press Enter (the choice is saved under `[plugins.<name>]` in `config.toml`; Esc asks again at the next start). Then each host's daemon installs the plugins in your user scope with `claude plugin install` (never with `-y`: a plugin whose marketplace runs a command is left for you to install by hand), and every session claudio opens with that proxy profile (manager, `--plain`, SSH hosts) gets the servers (`--mcp-config`, secrets only in its env), the agents (`--agents`) and the instructions. What you already have stays yours: an installed plugin, a marketplace, an MCP server or an agent of the same name is left alone. The list and its env are cached in `~/.config/claudio/plugins.json` (mode 0600), so a session still gets them when the proxy is unreachable, and refreshed every 15 minutes. `claudio proxy logout` deletes them. Plugins are listed in the extensions screen (`Alt+i`): `Space` turns one off for new sessions, `Enter` opens its setup screen again, and its details say, per host, what was installed, what was already there, what is still installing and what was skipped and why. Installed plugins stay until you remove them (`claude plugin uninstall <id>`).
 
 - `claudio --no-proxy` or `claudio --proxy <profile>` overrides the default for one run.
 - `CLAUDIO_PROXY_URL=<token>@proxy.example.com claudio` uses a proxy for one run without saving a profile.
@@ -219,7 +219,7 @@ In the pane, each tag is replaced by a one-line Unicode rendering in cyan, such
 
 `Alt+z u` opens the reader view too. You can write your own extensions in any language: a directory in `~/.config/claudio/extensions/` with a manifest and a program that claudio runs and talks to over JSON lines. See [docs/extensions.md](docs/extensions.md) and the example in [`examples/extensions/keys/`](examples/extensions/keys/). Each extension can be turned off under `[extensions]` in `config.toml`.
 
-`Alt+i` (or `Alt+z e`) opens the extensions screen: one row per extension, built-in, external or proxy plugin, with its keys and lenses, and below the list the selected one's description, actions and lenses (for a plugin, its version, profile and state on each host). `Space` or `Enter` turns it on or off; `r` fetches the plugin lists again. The change applies at once (its keys leave the `Alt+z` popup, its lenses stop rendering) and is saved as `name = true|false` under `[extensions]` in `config.toml`, the rest of the file untouched. claude's instructions change for new sessions; `Alt+e` restarts one with them. `Esc` or `Alt+i` closes the screen.
+`Alt+i` (or `Alt+z e`) opens the extensions screen: one row per extension, built-in, external or proxy plugin, with its keys and lenses, and below the list the selected one's description, actions and lenses (for a plugin, its version, profile, setup and state on each host). `Space` turns it on or off, and so does `Enter` except on a plugin's row, where it opens the plugin's setup screen; `r` fetches the plugin lists and manifests again. The change applies at once (its keys leave the `Alt+z` popup, its lenses stop rendering) and is saved as `name = true|false` under `[extensions]` in `config.toml`, the rest of the file untouched. claude's instructions change for new sessions; `Alt+e` restarts one with them. `Esc` or `Alt+i` closes the screen.
 
 ### Keeping claude up to date
 
@@ -303,6 +303,12 @@ diagram = false                 # e.g. no Mermaid instruction or drawing
                                 # (Alt+i turns them on and off and writes this table;
                                 # proxy plugins too, by name)
 
+[plugins.vocdoni-internal]      # a proxy plugin's setup (written by its setup screen)
+disabled = ["mcp:jina-mcp-server"]  # item ids left out: plugin:<name>@<market>, mcp:<name>,
+                                #   agent:<name>, instructions, env
+reviewed = "1.0.0"              # the manifest version last reviewed
+seen = ["plugin:agentic-dev@vocdoni", "mcp:jina-mcp-server"]  # its ids: a newer id is new
+
 [keys]                          # rebind any manager key
 new_session = "alt+n"
 terminal    = "alt+c"
diff --git a/docs/extensions.md b/docs/extensions.md
index 1837d30..e99abeb 100644
--- a/docs/extensions.md
+++ b/docs/extensions.md
@@ -40,8 +40,9 @@ One row per extension: the built-ins, then the external ones that loaded
 the plugins of your proxy profiles (see below).
 Below the list, the selected extension's description, its actions and its
 lenses. `↑`/`↓`, `PgUp`/`PgDn`, `Home`/`End` and the mouse wheel move;
-`Space` or `Enter` turns the selected extension on or off; `r` fetches the
-plugin lists again; `Esc` or `Alt+i` closes the screen.
+`Space` turns the selected extension on or off, and so does `Enter` except
+on a plugin's row, where it opens the plugin's setup screen; `r` fetches the
+plugin lists and manifests again; `Esc` or `Alt+i` closes the screen.
 
 A change applies at once: the extension's keys leave the `Alt+z` popup, its
 tag lenses stop rendering in the pane (at the next draw) and in the reader,
@@ -79,30 +80,96 @@ none); v0.5.0 clients still get them through `Latex`.
 ### Proxy plugins
 
 A claude-proxy profile can serve plugins (`GET /v1/claudio/plugins`): for
-each, a pinned bundle (a zip of skills, agents, hooks and MCP servers, by
-URL and SHA-256) and the env its servers need. claudio fetches the list of
-every profile in use at start, every 15 minutes, after `claudio proxy login`
-and on `r` in the extensions screen, and caches it in
-`~/.config/claudio/plugins.json` (mode 0600: it holds the plugins' secrets).
-
-A plugin's row shows its version and profile; its details say, per
-connected host, `ready`, `downloading`, `installed in Claude Code (bundle
-skipped)`, `download failed: …` or `daemon too old`, and for the list
-`proxy unreachable, cached <age>` when the proxy did not answer. A plugin
-claudio refuses (a bad name, URL or hash, an env key it may not set such as
-`PATH`, `LD_*` or `ANTHROPIC_*`, a name another extension has) is listed as
-`refused: <reason>` and never used.
-
-Each session started with that profile gets `--plugin-dir <zip>` for every
-plugin that is on and the plugin's env (the proxy's own env wins on a shared
-key). The zip is the host's own copy: each daemon downloads it once per
-version into `~/.cache/claudio/plugins/` (https only, 64 MiB, hash-checked)
-and skips it when Claude Code there has the plugin installed. A new session
-waits up to 20 s for a bundle still downloading, then starts without it (a
-notice says so). A claude that does not know `--plugin-dir` starts without
-the bundles. Turning a plugin off is saved as `name = false` under
-`[extensions]` and applies to new sessions; `Alt+e` keeps a session's
-bundles and gives it the current env.
+each, a pinned manifest (a TOML file, by URL and SHA-256) and the env its MCP
+servers need (their secrets). claudio fetches the list of every profile in
+use at start, every 15 minutes, after `claudio proxy login` and on `r` in
+the extensions screen, and caches it in `~/.config/claudio/plugins.json`
+(mode 0600: it holds the secrets). It fetches each manifest itself (https,
+256 KiB at most, hash-checked, kept in memory).
+
+A manifest declares marketplaces and the plugins to install from them, MCP
+servers (http, sse or stdio, with the names of the secrets each needs),
+subagents, instructions for every session's context, and env:
+
+```toml
+format = 1
+name = "vocdoni-internal"
+version = "1.0.0"
+description = "Vocdoni team setup"
+instructions = """Search the vocdoni-kb server before asking a colleague."""
+
+[[marketplace]]
+name = "vocdoni"                       # the name its .claude-plugin/marketplace.json declares
+source = "github:vocdoni/skills"       # github:<owner>/<repo> or an https URL
+
+[[plugin]]
+name = "agentic-dev"
+marketplace = "vocdoni"
+
+[[mcp]]
+name = "vocdoni-kb"
+type = "http"
+url = "https://kb-gw.vocdoni.net/mcp"
+headers = { Authorization = "Bearer ${VOCDONI_KB_TOKEN}" }
+secrets = ["VOCDONI_KB_TOKEN"]         # served by the proxy as env; never on argv
+
+[[agent]]
+name = "reviewer"
+description = "Read-only code reviewer"
+tools = ["Read", "Grep", "Glob"]
+prompt = """You are a code reviewer…"""
+```
+
+The first time claudio sees a manifest (no `[plugins.<name>]` table in
+`config.toml`), its setup screen lists every item by section, all on:
+`↑`/`↓` (or `j`/`k`) move, `Space` toggles, `a` turns all on, `n` all off,
+`Enter` saves the choice and applies it, `Esc` puts it off until the next
+start (nothing is applied meanwhile). `Enter` on the plugin's row of the
+extensions screen opens it again. The choice is saved as
+
+```toml
+[plugins.vocdoni-internal]
+disabled = ["mcp:jina-mcp-server"]
+reviewed = "1.0.0"
+seen = ["plugin:agentic-dev@vocdoni", "mcp:vocdoni-kb", "mcp:jina-mcp-server", "agent:reviewer", "instructions"]
+```
+
+(the table is replaced or appended; the rest of the file stays as it was).
+A newer version's items that are not in `seen` are on, and a notice names
+them.
+
+Each host's daemon applies the chosen items: it adds a marketplace Claude
+Code does not know (`claude plugin marketplace add --scope user`) and
+installs the plugins in your user scope (`claude plugin install --scope
+user`), in the background, one at a time, never with `-y`: a plugin whose
+marketplace declares a command to run is `needs manual install` with the
+command to type. A failed install is tried again after 10 minutes. What you
+already have is left alone: an installed plugin is `present`, an MCP server
+in your `~/.claude.json` or an agent in `~/.claude/agents/` of the same name
+is skipped as `yours`. A server whose secret the proxy does not serve is
+skipped too.
+
+Every session started with that profile then gets, per plugin that is on,
+`--agents` and `--mcp-config` (the server definitions with `${NAME}`
+placeholders; the secrets are only in the session's env), the manifest's
+env and its instructions (through the same context hook as the lenses'). A
+new session waits up to 20 s for a host that has not answered yet, then
+starts without the plugin; installs still under way do not hold it. A
+claude without `--agents` or `--mcp-config` starts without them (the
+details say `claude too old`). At most 8 plugins go on one session.
+
+A plugin's details say, per connected host, `ready · <i> installed, <p>
+present, <s> installing, <k> skipped` with a line per skipped item and its
+reason, `failed: …` or `daemon too old`; the manifest's state when it could
+not be used (`manifest: <reason>`); `setup pending (Enter)` or `<n> items
+disabled (Enter to change)`; and `proxy unreachable, cached <age>` when the
+proxy did not answer. A plugin claudio refuses (a bad name, URL or hash, an
+env key it may not set such as `PATH`, `LD_*` or `ANTHROPIC_*`, a name
+another extension has) is listed as `refused: <reason>` and never used.
+Turning a plugin off is saved as `name = false` under `[extensions]` and
+applies to new sessions; nothing is uninstalled (`claude plugin uninstall
+<id>` does that). `Alt+e` keeps a session's flags and gives it the current
+env and instructions.
 
 Snippets are files in `~/.config/claudio/snippets/` (`*.md` or `*.txt`, the
 file name without its extension is the snippet's name). A paste never presses
`````
Run: `/usr/bin/grep -n -i -e bundle -e '\.zip' -e plugin-dir CLAUDE.md README.md docs/extensions.md; umask 022; cargo test --locked --bin claudio`
Expected: exactly one grep line, ``CLAUDE.md:165:  `--plugin-dir`, `--agents`, `--mcp-config`; `command()` drops such a pair`` (the probe's flag list, still true), then `test result: ok. 1093 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`

Step 3: commit
`/usr/bin/git commit -m "docs: plugin manifests, the setup screen and [plugins.<name>]" -- CLAUDE.md README.md docs/extensions.md`

Interfaces this task provides to later tasks: none.

## Merge and release
<!-- Not copied into briefs; the controller executes this after the gate audit. -->
1. Full suites, on the tip of `feat/plugin-manifest`, each alone on the machine (measured in the scratch on 19647e4):
   - `umask 022; make test-unit` → `test result: ok. 1093 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`, and the dev build has 0 warnings (about 18 s; on one of the base flakes named in the header, rerun once).
   - `umask 022; make test-manager` → `test result: ok. 51 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` (about 170 s).
   - `umask 022; env -u CLAUDIO_SSH_TEST_HOST cargo test --features diag --test remote -- --test-threads=1` → `test result: ok. 9 passed`.
   - `make` → release build OK. `make static` → `statically linked` (not run in the scratch: needs the musl target; CI's release job builds it).
   - `rustfmt --edition 2021 --check` on each file of Global Constraints' clean and new lists (not `main.rs`/`mod.rs`; check those with the `mod x {}` substitution) → nothing.
   - Clippy as in Global Constraints: 74 findings, the same as the base, none on a line this branch adds.
2. AC6 (controller, gated, real claude on the host): `umask 022; CLAUDIO_SSH_TEST_HOST=z6 cargo test --release --features diag --test remote t9 -- --test-threads=1 --nocapture` → `[t9] Applied { mcp_config, agents, instructions, env } ✓` and either `[t9] the remote session's argv has --agents and --mcp-config ✓` or `[t9] the remote claude lacks [...]: left out ✓`. z6 ran a 0.6.1 dev daemon of ed65dec; `freshness::compare` replaces it with this build's (newer mtime) on connect, unless a session there is busy (then run `claudio daemon restart` on z6 first).
3. AC7 (manual, owner or controller) needs the manifest registered (point 6).
   - Setup: the branch's `target/release/claudio` on an isolated config: `XDG_CONFIG_HOME`,
     `XDG_RUNTIME_DIR` and `XDG_CACHE_HOME` under `~/.cache/claudio/ac7/`, `HOME` real (claude
     logged in, `~/.claude` the member's).
   - `claudio proxy login --token` with a member token.
   - The TUI's setup screen lists 13 plugins, 3 servers and 3 agents (plus instructions); Enter.
   - A new session has the `vocdoni-kb` server (`/mcp`), the team skills (`/plugin`, after the
     installs: `Alt+i` says `0 installing`) and the `reviewer` agent (`/agents`).
   - Stop the isolated daemon and remove `~/.cache/claudio/ac7/` afterwards. A failure is a fix
     task.
4. Gate audit of the branch; fixes re-reviewed.
5. Merge: `/usr/bin/git checkout main && /usr/bin/git merge --ff-only feat/plugin-manifest`. `main` is at ed65dec. The branch is linear on top of it: the four spec commits aa58517 … c88c627, then the ten task commits.
6. Release v0.7.0 (a new feature; the convention of `git show --stat dacfede`):
   - Set `version = "0.7.0"` in Cargo.toml and run `cargo update --workspace --offline`, which
     changes only claudio's own entry. `git diff --stat` shows `Cargo.lock | 2 +-` and
     `Cargo.toml | 2 +-`, and `cargo build --locked` succeeds.
   - `/usr/bin/git commit -m "release v0.7.0" -- Cargo.toml Cargo.lock` (no trailer).
   - `/usr/bin/git tag v0.7.0`. The release workflow checks that the tag matches Cargo.toml.
     The old local `v0.7.0` tag was dropped with d143cdb; check with
     `/usr/bin/git tag -l v0.7.0` before tagging.
7. The manifest (a separate owner step, in `vocdoni/skills`):
   - Commit `~/.cache/claudio/vocdoni-internal.toml` as `manifests/vocdoni-internal.toml`
     (it is the fixture of Task 1; `cargo test plugins::manifest` proves it parses) and push.
   - Its URL is pinned to that commit:
     `https://raw.githubusercontent.com/vocdoni/skills/<commit sha>/manifests/vocdoni-internal.toml`.
     Use the full commit sha from `git rev-parse HEAD`, never a branch name.
   - Its hash: `curl -fsSL <that URL> | sha256sum` (it must equal
     `sha256sum manifests/vocdoni-internal.toml`).
   - On the proxy, register plugin `vocdoni-internal` with that URL and sha, `installed_id`
     empty, and its env: `VOCDONI_KB_TOKEN`, `JINA_API_KEY`. Use claude-proxy's plugin admin
     commands (`claude-proxy plugins --help`; the previous release used
     `claude-proxy plugins set …` and `plugins env set …`).
8. **Owner OK required before**, each on its own:
   - pushing the manifest commit to `vocdoni/skills`;
   - registering `vocdoni-internal` (URL, sha, env) on claude.vocdoni.net;
   - `/usr/bin/git push origin main`;
   - `/usr/bin/git push origin v0.7.0` (starts the release workflow: builds and publishes the binaries);
   - announcing the release to members (they run `claudio proxy login --token` or the browser flow, then review the setup screen once).
9. Post-release checks:
   - `gh run list --limit 5` → the `ci` run on `main` (Linux and macOS) and the `release` run
     on `v0.7.0` succeed (`gh run watch <id>`).
   - `gh release view v0.7.0` lists every target of `release.yml`.
   - `claudio upgrade` from v0.6.1 reaches 0.7.0, and `claudio daemon status` shows the new
     daemon.
   - A session on an old (v0.6.1) remote daemon still starts, its plugin row saying
     `daemon too old`.

## Scratch verification record
- Where: `~/.cache/claudio/scratch/plugin-manifest`, a detached worktree of /volumes/repos/claudio at c88c627, one commit per task: 70594b1, c4ea6b4, d9103b8, 8cf5d82, 1a989a3, fea3a8f, d7ab051, 8240c91, 6fb7d73, 19647e4. The patches are `git format-patch -1` of those commits; Steps 1 and 2 of a task together are its patch.
- Split check: each task was rebuilt on a second worktree (`~/.cache/claudio/scratch/pm-replay`) as its Step 1 (test part) and then its Step 2, running the commands above. Every Step 1 failed as quoted (Tasks 8 and 9 pass by design), and every Step 2 tree equals the task's commit.
- Replay: the assembled plan's blocks were applied with `git apply`, in order, on a fresh worktree at c88c627 (`~/.cache/claudio/scratch/pm-final`), and each task committed with its Step 3 command (Task 4's deletion included). Every block applied, each commit's tree equals the scratch commit, no trailer was added, and nothing was left uncommitted. On the replayed tip: `cargo build` 0 warnings, unit 1093 passed, manager 51 passed, remote (no host) 9 passed.
- Unit totals: 1044 (base) → 1063 (Task 1) → 1065 (2) → 1066 (3) → 1073 (4) → 1076 (5) → 1082 (6) → 1094 (7) → 1093 (8) → 1093 (9) → 1093 (10). Manager: 52 (base) → 50 (Tasks 6 and 7) → 51 (Task 9; the new test 4/4 stable alone). Remote (no host): 9 at Tasks 4 and 8. The dev build has 0 warnings from Task 7 on.
- Flakiness: twice a unit run hit the ETXTBSY family the base already shows: a test script exec'd right after it was written while another test thread forks. Once it was `daemon::host::tests::claude_flags_read_long_help`, once `claude_flags_come_from_one_help_run`; the latter, which Task 3 extends, now probes again. Reruns passed. Also seen once at the base: `daemon::host::tests::probe_runs_claude_with_the_given_path` and `daemon::tests::freshness::an_outdated_idle_daemon_is_replaced_and_its_sessions_come_back`.
- Clippy (snap toolchain) at the tip: 74 findings, the base's 74; none on an added line. rustfmt: every file of the clean and new lists checks clean; added lines in the other files are formatted as rustfmt would, except two that sit inside expressions rustfmt would rewrap as a whole because of their base layout (`src/daemon/mod.rs:1047`, the `..Default::default()` of `effective_extensions`; `src/tui/mod.rs:1612`, the `with_plugins` line of the deferred spawn), left as the surrounding code is.
- Not verified here: macOS (CI builds and unit-tests it), `make static`, the release workflow, a real claude, proxy or SSH host (AC6 and AC7 are gated or manual), an old v0.6.1 daemon against this client (covered by the `unsupported op` unit test).
