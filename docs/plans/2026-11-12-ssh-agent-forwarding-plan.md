# SSH agent forwarding per remote host: implementation plan

Spec: docs/specs/2026-11-12-ssh-agent-forwarding-design.md (audited; `.delivery/2026-11-12-ssh-agent-forwarding/audit-spec.md` F1-F10 folded in)
Class: architectural
Repos and branches: /home/p4u/claudio → feat/agent-forwarding (base 88f344d)

Every patch in this plan comes from a scratch worktree of 88f344d and was replayed there task by
task: each task's patches apply with `git apply` on top of the previous task's, every step 1
fails as written under "Expected", every step 2 passes. The final tree passed the `make check`
and `make matrix` sequences, the `manager`, `vocdoni_manager`, `web`, `web_manager`,
`vocdoni_web` and `remote` suites and the whole Playwright e2e. "Decisions and verification"
lists what was not run.

A second delivery, feat/limit-auto-switch, runs in parallel from the same base. This plan's
edits to the files both branches touch are small and placed away from that spec's edits.
"Merge and release" step 3 lists the hunks that may conflict.

## Decisions and verification

The spec leaves these to the plan, or does not say. The gate audit checks the code against them.

1. **TUI key: `Alt+f`**, action `forward_agent` (`[keys] forward_agent` rebinds it), scope
   `Wizard` like `Alt+.`.
   - It is free among the manager's bindings (`DEFAULT_BINDINGS`: `Alt+←/→ n r x a g h c l e . u
     z i q`, the addons' panel keys such as `Alt+s`, the fixed `Alt+Shift+<digit>`).
   - Wizard-scoped, it is looked up only while the wizard is open, so Claude Code's own `Alt+f`
     (forward a word) still reaches claude. A plain letter cannot be used: typing in the Where
     step is the filter. "f" for "forward".
   - The step's hint line (the filter's second row, empty until now) shows it:
     `Alt+f ssh agent for <host>: forward on connect | off (your ssh config decides)`. A marker
     ` [agent]` follows each listed host that has the option.
2. **TUI flags travel with the candidates.**
   - `Effect::LoadSshHosts` → `hosts::ssh_hosts` reads `remote::hosts::candidates()` and
     `forward_agent_hosts()` and calls `App::on_ssh_hosts(hosts, forward_agent)`.
   - `App.forward_agent` holds the list and `ui/wizard_host.rs` draws from it. `wizard.rs` (at
     its A8 baseline) is not touched.
   - `Alt+f` → `App::toggle_forward_agent` (pure): it flips the marker at once, sets
     `forward_pending` (one toggle at a time), and emits `Effect::ForwardAgent { host, on }`.
   - `hosts::ssh_hosts` writes in `spawn_blocking`. If the host was connected at toggle time
     and the file changed, it sends `HostEvent::Reconnect { host, generation }`; a failed write
     is a `HostEvent::Notice`. Then `HostEvent::ForwardSaved(read)` re-sends the hosts and
     flags to `App::on_forward_saved`. `LoadSshHosts` stays synchronous, as today.
3. **TUI reconnect** is the existing `HostEvent::Reconnect` path: same stale guard,
   `conns.disconnect`, then `spawn_connect(…, skip_bootstrap = true)`. Its handling moves out
   of `event_loop` into `hosts::reconnect`, unchanged, to pay for the new event (A8:
   `lib.rs` 814/814, `event_loop` 229/229).
4. **Web module `addons/web/src/hosts.rs`** holds every `hosts.*` op. `ws.rs::dispatch`
   delegates with one `op if op.starts_with("hosts.")` arm, as it already does for `addons.*`;
   `dispatch` shrinks 234 → 208 (A8). `host_ok` moves there from `hub.rs` to pay for the new
   hub code (`hub.rs` 2061 → 2064 of its 2065).
5. **Web reconnect.**
   - `HubHandle::forward_agent_changed(host)` sends `Cmd::Reconnect { host, generation: None }`.
     `Cmd::Reconnect.generation` becomes `Option<u64>`; `check_outdated` sends `Some`.
   - `Hub::reconnect` drops and re-establishes a connected host through `lost()` +
     `start_attempt()`, as the outdated-daemon path does. Any other host gets its `host` event
     (only for `None`).
   - The op does this only when the file changed (nothing else to report). A host the hub does
     not know gets no event. A host mid-connect is only written (spec §4).
6. **Host DTO `forward_agent`** comes from a server-side copy of `[ssh] forward_agent`
   (`hosts::FORWARD`), read off the executor at start (`build_state`), after each
   `hosts.forward_agent`, and at every config.toml re-read (`ext.rs` `reread`). The hub actor
   never reads a file. An edit made elsewhere (the TUI) shows at the page's next `host` event
   or reload.
7. **Relay module `crates/core/src/daemon/agent.rs`.**
   - `AgentRelay` is a new daemon `Config` field, `agent`. Its `fallback` is the daemon's
     inherited `SSH_AUTH_SOCK`, read in `Config::from_env`. Its listening path is an
     `Arc<OnceLock>` that every clone of the config shares.
   - The paths derive from the daemon socket: `paths::agent_relay_beside(socket)` is
     `<runtime>/agent-v{PROTO}.sock`, and `paths::agent_links_beside(socket)` is
     `<runtime>/agents` (the bridge uses `paths::agent_links()`). In production that is the
     spec's `paths::agent_relay_socket()`. A test daemon's relay stays in its temp dir, and
     `TestDaemon::config` has no fallback, so no test reaches the owner's agent.
   - The relay starts in `serve_until_shutdown` before `serve()` and stops when `serve`
     returns. Its accept task is aborted; it never waits on `Daemon::shutdown`, which is a
     `notify_one`.
8. **Spawn env.**
   - `AgentRelay::apply` runs before `apply_env`. So an `env_remove` naming `SSH_AUTH_SOCK`
     takes it away again, and a value in the spawn's env wins (spec §3). `apply_env` itself is
     unchanged.
   - Terminals: `shell_command` takes the daemon `Config`. In `session.rs` one call changes and
     one line goes.
9. **AC5 "spawned with no link".** After F1 the rule is "`agents/` exists". The test checks:
   - no `agents/`: the daemon's own value (`unset` or `other`; the value is never recorded);
   - `agents/` with a live link: the relay, for a claude session and a terminal;
   - an `env_remove` of `SSH_AUTH_SOCK`: none;
   - an explicit value in the env: it wins.
10. **A hermetic fake remote, `FakeSsh`** (`crates/tui/tests/common/mod.rs`, shared by the web
    and TUI suites).
    - A fake `ssh` on `PATH` logs its argv, then runs the remote command here with its own
      `HOME`, config and `TMPDIR`.
    - It sets no `XDG_RUNTIME_DIR`, so the bridge starts the remote daemon with setsid, never
      `systemd-run`.
    - The owner's `SSH_AUTH_SOCK` never reaches it. Like sshd, it forwards an agent only when
      given `-o ForwardAgent=yes`: a per-connection path to a fake agent, gone when the
      connection ends (`with_agent`).
    - AC6 and AC8 use it. So does a hermetic end-to-end test of Goals 1-2,
      `web_forwarded_agent_reaches_terminals_through_the_relay`: a terminal reaches the agent,
      loses it while no connection forwards one, and gets it back on the next forwarding
      connection without a restart.
11. **AC9 is `t10`** in `tests/remote.rs`, through a new diag word,
    `claudio __agent-check HOST [ID]`.
    - ssh forwards the agent it authenticates with (`IdentityAgent` changes both). A separate
      throwaway agent would need the connection to authenticate with it, which means editing
      the host's `authorized_keys` or adding a key to the owner's agent.
    - So `t10` uses the agent the controller's ssh already uses, read-only. It compares
      `SHA256:` fingerprints only, never comments, and skips when that agent holds no key.
    - Its second run attaches to the same terminal over a new bridge ("still works after the
      bridge reconnects").
12. **Errors.**
    - `SaveError::BadHost(String)` (config.rs) for a host `validate_host` refuses.
    - The web answers `could not save: <why>`; the TUI gives the notice
      `ssh agent forwarding for <host> not saved: <why>`.
    - The bridge says nothing when `SSH_AUTH_SOCK` is unset or empty. A set but unusable one is
      one stderr line: `claudio --slave: ssh agent not linked: <why>`.
13. **Page.**
    - One help text constant (`AGENT_HELP`), shown under the panel row and under a checked
      box.
    - A new testid `host-agent` (the row's text) joins the spec's `host-agent-btn`,
      `host-add-agent` and `wizard-host-agent`.
    - No CSS: the existing `kv`, `wz-check`, `wz-note` and `btn btn-sm` classes do it.
14. **`connect_ssh_with`** reads `config.toml` synchronously for the flag. It is one small
    local file, like the `hosts.json` reads around the connect path. `client.rs` stays at
    856 of its 857.
15. **A8 room** (scripts/check_rules.py):
    - every baseline item touched shrinks or stays the same;
    - `crates/core/src/daemon/mod.rs` goes 780 → 786. With feat/limit-auto-switch's +3 that is
      789 of the 800 budget (Merge step 3).

**Verified in the scratch worktree** (`~/.cache/claudio/scratch/plan-agent`, target
`~/.cache/claudio/scratch/target-plan-agent`, rustc 1.99.1, Linux):

- Per task, step 1 failed and step 2 passed, with the exact commands and lines under each
  task, on the replayed history (Task 6 with Playwright).
- On the final tree:
  - `cargo fmt --all --check`;
  - `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`;
  - `scripts/check_rules.py` (0 problems);
  - `cargo build --locked --no-default-features`;
  - workspace and per-crate lib tests (core 559, tui 393, web 119, api 106, vocdoni 184);
  - the five matrix cells (clippy, build, bins tests) and the core-only `manager` suite (43);
  - the suites: `manager` 43, `vocdoni_manager` 16, `web` 55, `web_manager` 4,
    `vocdoni_web` 13, `remote` 10 (t2-t6, t9 and t10 skip without a host);
  - the node tests (152) and the whole Playwright e2e (50, with `agent.spec.mjs`).
- The AC6, AC8 and hermetic web tests were each run three times or more, with no failure and
  no leftover process.

**Not run:** `t10` against a real host (none was reachable from the scratch machine; the
controller runs it, Merge step 1), macOS (CI), and `make static`.

## Global Constraints

- Commits: conventional commits `type(scope): lowercase summary`; no AI attribution, no
  Co-Authored-By or similar trailers; commit with explicit paths only (`git add <new files>`
  first, then `git commit -m … -- <paths>`); never `git add -A`; never push.
- Work in `/home/p4u/claudio` on `feat/agent-forwarding`. Build output goes under the repo's
  `target/` (or `~/.cache/claudio/`), never `/tmp`. Subagents never dispatch subagents.
- **Applying a patch.** Every `diff` block is exact.
  - Save it verbatim, without the fence lines, to `~/.cache/claudio/plan-patches/<file name
    given>`.
  - From the repo root, run `git apply --check <file>`, then `git apply <file>`. Never copy a
    patch by hand.
  - If `--check` fails, stop and report: the tree is not what the plan expects.
  - `git status --short` must then list exactly the step's files (new files untracked).
- **`make check` passes at every code commit.** `scripts/check_rules.py` must print
  `rules: 0 problem(s)`. The A8 numbers each task must end with are given in its step 3.
- Integration suites run single-threaded:
  `cargo test --locked --test <suite> -- --test-threads=1`.
- The shell's `rtk` hook may summarise `cargo`, `git` and `grep` output. Prefix a command with
  `rtk proxy` when a step needs its exact lines.
- **Hermetic (E3).**
  - Run claudio only through the suites' isolated env (`XDG_*`, `HOME`,
    `CLAUDIO_CLAUDE_PATH`). Never run `claudio`, `claudio web` or `claudio daemon …` against
    the owner's config, daemon or web service.
  - Never read, probe or change the real `~/.ssh` or the real `SSH_AUTH_SOCK`. `FakeSsh`
    scrubs it, daemon test configs carry no fallback agent, and the one `ssh-add` probe runs
    with `SSH_AUTH_SOCK` removed.
  - The only exception is Task 8's `t10`: opt-in (`CLAUDIO_SSH_TEST_HOST`), run by the
    controller only.
  - Tests must pass on Linux and macOS: no `/proc`, no `setsid` command, socket paths short.
- **ACCEPTANCE.md throughout.**
  - **A2/A3:** the core names no addon; the web and the TUI write `[ssh]` only through
    `remote::hosts::set_forward_agent`; `"ssh"` joins `CORE_SECTIONS`.
  - **A6:** `app.rs` stays pure; the TUI's I/O is in `lib.rs`/`hosts.rs`.
  - **A8:** baseline items may shrink, never grow; this plan's numbers are in each task.
  - **A10:** the relay is bounded (32 connections, 1 s connects, at most 64 links tried); the
    probe is bounded (2 s, the answer's 5-byte head only).
  - **B2:** Task 1 is the first commit.
  - **C1:** no key material, agent traffic or key comments in logs or test output; the socket
    path is not a secret.
  - **D2:** no `unwrap()` outside tests.
  - **E2:** no test weakened; the changed existing tests only gain the new `Config` field, the
    new `on_ssh_hosts` argument or the new `shell_command` argument.
- **Unchanged behaviour.**
  - A host without the option gets exactly today's ssh argv (AC1); bootstrap never forwards.
  - Local sessions keep their env: no bridge runs locally, so no `agents/` directory exists
    there.
  - The web protocol stays version 2.
- **Page.** Pure browser modules import nothing through bare specifiers. New UI follows
  DESIGN.md and uses only existing classes and tokens. The new `data-testid`s are
  `host-agent`, `host-agent-btn`, `host-add-agent` and `wizard-host-agent`.

## Review Focus

Line numbers are in the final tree (after Task 8).

- **Relay accept, peer check, mode** (crates/core/src/daemon/agent.rs):
  - `start` (:80) binds through `server::bind` (crates/core/src/daemon/server.rs:116, 0600 at
    :123) and refuses a fallback equal to its own path (:86, no self-loop);
  - `serve` (:93) caps at `MAX_RELAYS` (:25, `try_acquire_owned` :106; over the cap the
    stream is dropped);
  - `own_peer` (:122) checks the uid on every accept;
  - `relay` copies bytes only (:143, `copy_bidirectional`).
- **Link pruning**:
  - `connect_agent` (:150) removes a link only on refused, gone or forbidden (`dead` :172;
    removal :159), never the fallback;
  - `newest_first` (:183) is bounded, symlinks only, ties by name.
- **Spawn-env rule**:
  - `AgentRelay::apply` (:56): it sets the env only while the relay listens and `agents/` is a
    directory (`symlink_metadata`);
  - called before `apply_env` in `command()` (crates/core/src/daemon/command.rs:71) and
    `shell_command()` (:191), so `env_remove` and the spec's env still win.
- **Bridge probe and link** (crates/core/src/remote/agent.rs):
  - `register` (:51) prints a failure on stderr only;
  - `probe` (:69) wants an absolute path to a socket (:74) and has a 2 s budget (:84);
  - `answer_ok` (:95) parses only 5 bytes, with length bounds;
  - `link` (:110) makes `agents/` private (:111), then symlinks a temporary name and renames
    it into place;
  - `Link::drop` (:40) unlinks;
  - the bridge (crates/core/src/remote/bridge.rs:73-84) holds the `Link` until it returns and
    writes nothing to stdout.
- **Config writer and ssh argv**:
  - `set_forward_agent_in` (crates/core/src/remote/hosts.rs:113) validates the host (:114)
    and goes only through `config_edit::edit_section` (:115);
  - `forward_agent_in` (:85) drops invalid entries on read (:96);
  - `ssh_cmd` (crates/core/src/remote/mod.rs:45) puts `-o ForwardAgent=yes` (:61) before
    `--` (:63);
  - `connect_ssh_with` reads the flag (crates/core/src/client.rs:227).
- **Web op** (addons/web/src/hosts.rs):
  - `forward_agent` (:61) refuses `local` (:63), runs `host_ok` (:65), writes off the
    executor (:67), and reconnects only on a change (:75);
  - the cached list `FORWARD` (:92) and `forwards_agent` (:106, never `local`);
  - `Hub::reconnect` (addons/web/src/hub.rs:1044): with a `generation`, only the current
    connection is dropped (:1046), so no stale reconnect.
- **TUI toggle**:
  - `App::toggle_forward_agent` (crates/tui/src/app.rs:468) runs one write at a time
    (`forward_pending`);
  - `hosts::ssh_hosts` (crates/tui/src/hosts.rs:91) captures the generation at toggle time
    (:106) and does its I/O in `spawn_blocking` (:110, :132).

## Task order and parallelism

| Task | Depends on | Repo/files | May run in parallel with |
|---|---|---|---|
| 1 Web protocol doc (B2) | none | docs/web-protocol.md | none: first commit |
| 2 `[ssh] forward_agent` and `ssh_cmd` | 1 | crates/core/src/{remote/mod.rs, remote/hosts.rs, remote/bootstrap.rs, client.rs, addon/registry.rs, config.rs} | none |
| 3 The bridge links its agent | 2 | crates/core/src/{paths.rs, remote/agent.rs (new), remote/mod.rs, remote/bridge.rs} | none |
| 4 The daemon's relay and the spawn env | 3 | crates/core/src/daemon/{agent.rs (new), mod.rs, server.rs, command.rs, session.rs, test_support.rs, tests.rs, tests/agent.rs (new)} | none |
| 5 Web server, fake remote harness | 2, 3, 4 | addons/web/src/{hosts.rs (new), dto.rs, ext.rs, hub.rs, lib.rs, ws.rs}, addons/web/tests/web.rs, crates/tui/tests/common/mod.rs | none |
| 6 Page | 5 | addons/web/web/app/ui/{hosts.js, wizard.js}, addons/web/web/e2e/tests/agent.spec.mjs (new) | 7 |
| 7 TUI | 2, 5 (`FakeSsh`) | crates/tui/src/{keymap.rs, interaction.rs, app.rs, hosts.rs, lib.rs, ui/mod.rs, ui/wizard_host.rs (new)}, crates/tui/tests/{manager.rs, common/mod.rs} | 6 |
| 8 AC9 on a real host; docs | 1-7 | crates/core/src/remote/diag.rs, tests/remote.rs, docs/web.md, docs/manager-design.md, CLAUDE.md, README.md | none |

Tasks 6 and 7 touch disjoint files, so their patches apply in either order. Everything else is
sequential: the patches were generated on one linear history.

## Tasks

### Task 1: The web protocol doc (B2, first commit)
Model: haiku   Repo: /home/p4u/claudio   Files: `docs/web-protocol.md`
Goal: AC10's first bullet. The Host DTO gets `forward_agent`, and the op `hosts.forward_agent`
is documented, before any code (B2). Both are additive: the protocol stays version 2.

Step 1: apply `t1.diff` (no test: a doc change).
````diff
diff --git a/docs/web-protocol.md b/docs/web-protocol.md
index d82c913..d44b4a1 100644
--- a/docs/web-protocol.md
+++ b/docs/web-protocol.md
@@ -202,6 +202,7 @@ only for the approval kinds and the two kinds above. Show `message` first.
 | `claude` | `{"path": string, "version": string}` \| null | null: claude is not installed there |
 | `features` | [string] | the daemon's features: `lens`, `search`, `last_prompt`, `attention`, `worktree`, and `addon:<name>` for each addon it runs ops for |
 | `version` | string \| null | the daemon's claudio version |
+| `forward_agent` | bool | the host is in `config.toml`'s `[ssh] forward_agent`: its bridge ssh asks for agent forwarding (`-o ForwardAgent=yes`). Whether a forwarded agent is live there is not known to the server. Always `false` for `"local"` |
 
 ### Layout **(decided here)**
 
@@ -274,6 +275,7 @@ known host answers `"no such host"`; a known but disconnected one `"not connecte
 | `hosts.candidates` | — | `{"hosts":[string]}` | MRU then `~/.ssh/config` aliases, without connected ones |
 | `hosts.connect` | `host` | `{"host":Host}` once connected | ssh `BatchMode=yes`; failure: the last 3 sanitized ssh stderr lines (or the bootstrap error). `"local"` answers `"local is always connected"` |
 | `hosts.disconnect` | `host` | `{}` | drops the host: `session_removed` for its sessions, then `host_removed`. `"local"` answers `"cannot disconnect local"` |
+| `hosts.forward_agent` | `host`, `on` (bool) | `{}` | adds `host` to `[ssh] forward_agent` (`on`) or removes it. When that changed the file: a connected host is reconnected (its sessions keep running; the UIs re-attach as after a dropped connection), any other known host gets a `host` event; a host mid-connect keeps its attempt, and the next one uses the new value. Any host string `hosts.connect` takes, known or not. `"local"` answers `"local has no ssh connection"`; a config the edit cannot make answers `"could not save: <why>"` |
 
 ### Sessions
 
````
Run: `git diff --stat`
Expected: `docs/web-protocol.md | 2 ++`

Step 2: commit
`git commit -m "docs(web-protocol): forward_agent on host and the hosts.forward_agent op" -- docs/web-protocol.md`

Interfaces this task provides to later tasks: the contract Task 5 implements. Host DTO
`forward_agent: bool` (never true for `local`). `hosts.forward_agent {host, on}` → `{}`, with
the error texts `"local has no ssh connection"` and `"could not save: <why>"`.

### Task 2: `[ssh] forward_agent` and `ssh_cmd`'s forwarding choice
Model: opus   Repo: /home/p4u/claudio   Files: `crates/core/src/remote/mod.rs`,
`crates/core/src/remote/hosts.rs`, `crates/core/src/remote/bootstrap.rs`,
`crates/core/src/client.rs`, `crates/core/src/addon/registry.rs`, `crates/core/src/config.rs`
Goal: Goal 1 and spec §1 (AC1, AC2).
- `ssh_cmd(host, forward_agent)`: with `true`, `-o ForwardAgent=yes` goes before `--`; with
  `false`, the argv is byte for byte today's.
- Bootstrap passes `false`. `connect_ssh_with` passes `remote::hosts::forward_agent(host)`, so
  the TUI and the web connect through it unchanged.
- `forward_agent_in` (reads; invalid entries and non-arrays read as empty) and
  `set_forward_agent_in` (the one writer, through `config_edit::edit_section`). Thin wrappers
  on `config.toml`: `forward_agent`, `forward_agent_hosts`, `set_forward_agent`.
- `"ssh"` joins `CORE_SECTIONS`. `SaveError::BadHost`.

Step 1: write the failing tests: `t2-step1.diff` (AC1 in `remote/mod.rs`, AC2 in `remote/hosts.rs`).
````diff
diff --git a/crates/core/src/remote/hosts.rs b/crates/core/src/remote/hosts.rs
index 67a7b08..24ab1fc 100644
--- a/crates/core/src/remote/hosts.rs
+++ b/crates/core/src/remote/hosts.rs
@@ -385,6 +385,100 @@ mod tests {
         assert!(start.elapsed().as_millis() < 10, "glob_match took too long");
     }
 
+    /// A config file in a new private temp dir, holding `text`.
+    fn config_with(text: &str) -> PathBuf {
+        let path = scratch().join("config.toml");
+        fs::write(&path, text).unwrap();
+        path
+    }
+
+    #[test]
+    fn forward_agent_is_added_removed_and_deduplicated_in_order() {
+        let text =
+            "# mine\n[claude]\ninstall = \"off\"\n\n[ssh]\nforward_agent = [\"a\", \"b\", \"a\"]\n";
+        let path = config_with(text);
+        assert_eq!(forward_agent_in(&path), ["a", "b", "a"]);
+        // On: the first copy stays, a later one goes.
+        assert!(set_forward_agent_in(&path, "a", true).unwrap());
+        assert_eq!(forward_agent_in(&path), ["a", "b"]);
+        assert!(
+            !set_forward_agent_in(&path, "a", true).unwrap(),
+            "already on"
+        );
+        assert!(set_forward_agent_in(&path, "me@10.0.0.2", true).unwrap());
+        assert_eq!(forward_agent_in(&path), ["a", "b", "me@10.0.0.2"]);
+        // Off: every copy goes; the others keep their order.
+        assert!(set_forward_agent_in(&path, "a", false).unwrap());
+        assert_eq!(forward_agent_in(&path), ["b", "me@10.0.0.2"]);
+        assert!(
+            !set_forward_agent_in(&path, "a", false).unwrap(),
+            "already off"
+        );
+        let after = fs::read_to_string(&path).unwrap();
+        assert!(
+            after.starts_with("# mine\n[claude]\ninstall = \"off\"\n"),
+            "{after}"
+        );
+        let table: toml::Table = after.parse().unwrap();
+        assert_eq!(table.len(), 2, "{after}");
+        fs::remove_dir_all(path.parent().unwrap()).unwrap();
+    }
+
+    #[test]
+    fn forward_agent_section_is_made_when_missing() {
+        let path = config_with("[ui]\nnotify = true\n");
+        assert!(forward_agent_in(&path).is_empty());
+        assert!(set_forward_agent_in(&path, "devbox", true).unwrap());
+        let after = fs::read_to_string(&path).unwrap();
+        assert!(after.starts_with("[ui]\nnotify = true\n"), "{after}");
+        assert!(
+            after.contains("[ssh]\nforward_agent = [\"devbox\"]"),
+            "{after}"
+        );
+        assert_eq!(forward_agent_in(&path), ["devbox"]);
+        // No file at all reads as empty, and is created on a write.
+        let missing = path.with_file_name("none.toml");
+        assert!(forward_agent_in(&missing).is_empty());
+        assert!(set_forward_agent_in(&missing, "devbox", true).unwrap());
+        assert_eq!(forward_agent_in(&missing), ["devbox"]);
+        fs::remove_dir_all(path.parent().unwrap()).unwrap();
+    }
+
+    #[test]
+    fn forward_agent_refuses_and_ignores_bad_hosts() {
+        let path = config_with("[ssh]\nforward_agent = [\"-oProxyCommand=x\", \"\", \"ok\", 3]\n");
+        assert_eq!(forward_agent_in(&path), ["ok"], "bad entries are ignored");
+        for bad in ["-oProxyCommand=x", ""] {
+            let err = set_forward_agent_in(&path, bad, true).unwrap_err();
+            assert!(matches!(err, SaveError::BadHost(_)), "{bad}: {err}");
+        }
+        assert!(crate::addon::CORE_SECTIONS.contains(&SSH_SECTION));
+        fs::remove_dir_all(path.parent().unwrap()).unwrap();
+    }
+
+    #[test]
+    fn forward_agent_that_is_not_an_array_reads_as_empty() {
+        let path = config_with("[claude]\ninstall = \"off\"\n[ssh]\nforward_agent = \"devbox\"\n");
+        assert!(forward_agent_in(&path).is_empty());
+        let config = crate::config::load_from(&path);
+        assert_eq!(
+            config.claude.install,
+            crate::config::UpdatePolicy::Off,
+            "the rest still loads"
+        );
+        assert!(set_forward_agent_in(&path, "devbox", true).unwrap());
+        assert_eq!(forward_agent_in(&path), ["devbox"]);
+        let path2 = config_with("ssh = 1\n");
+        assert!(forward_agent_in(&path2).is_empty());
+        assert!(matches!(
+            set_forward_agent_in(&path2, "devbox", true),
+            Err(SaveError::UnsupportedSection(_))
+        ));
+        for p in [&path, &path2] {
+            fs::remove_dir_all(p.parent().unwrap()).unwrap();
+        }
+    }
+
     #[test]
     fn glob_match_multiple_stars() {
         assert!(glob_match("a*b*c", "aXbYc"));
diff --git a/crates/core/src/remote/mod.rs b/crates/core/src/remote/mod.rs
index d7534d0..d05da1d 100644
--- a/crates/core/src/remote/mod.rs
+++ b/crates/core/src/remote/mod.rs
@@ -87,9 +87,50 @@ mod tests {
         assert!(validate_host("192.168.1.1").is_ok());
     }
 
+    fn args(cmd: &Command) -> Vec<String> {
+        cmd.as_std()
+            .get_args()
+            .map(|a| a.to_string_lossy().into_owned())
+            .collect()
+    }
+
+    #[test]
+    fn ssh_cmd_without_forwarding_is_unchanged() {
+        let want = [
+            "-T",
+            "-o",
+            "BatchMode=yes",
+            "-o",
+            "ConnectTimeout=15",
+            "-o",
+            "ServerAliveInterval=15",
+            "-o",
+            "ServerAliveCountMax=3",
+            "-o",
+            "StrictHostKeyChecking=accept-new",
+            "--",
+            "devbox",
+        ];
+        assert_eq!(args(&ssh_cmd("devbox", false)), want);
+    }
+
+    #[test]
+    fn ssh_cmd_forwards_the_agent_before_the_host() {
+        let args = args(&ssh_cmd("me@10.0.0.2", true));
+        let opt = args
+            .iter()
+            .position(|a| a == "ForwardAgent=yes")
+            .expect("ForwardAgent option");
+        assert_eq!(args[opt - 1], "-o");
+        let dd = args.iter().position(|a| a == "--").unwrap();
+        assert!(opt < dd);
+        assert_eq!(args.last().unwrap(), "me@10.0.0.2");
+        assert_eq!(args.len(), dd + 2, "the host is the only operand");
+    }
+
     #[test]
     fn ssh_cmd_accepts_new_host_keys_before_the_host() {
-        let cmd = ssh_cmd("devbox");
+        let cmd = ssh_cmd("devbox", false);
         let args: Vec<String> = cmd
             .as_std()
             .get_args()
````
Run: `cargo test --locked -p claudio-core --lib remote::`
Expected: it does not compile:
``error[E0425]: cannot find function `forward_agent_in` in this scope``,
``error[E0425]: cannot find function `set_forward_agent_in` in this scope``,
`error[E0061]: this function takes 1 argument but 2 arguments were supplied` (`ssh_cmd`).

Step 2: implement: `t2-step2.diff`.
````diff
diff --git a/crates/core/src/addon/registry.rs b/crates/core/src/addon/registry.rs
index 7133038..4a4f569 100644
--- a/crates/core/src/addon/registry.rs
+++ b/crates/core/src/addon/registry.rs
@@ -7,7 +7,7 @@ use std::sync::Arc;
 use super::{feature, name_ok, Addon, FlagDecl, PanelDecl};
 
 /// The `config.toml` tables the core owns; no addon may claim them.
-pub const CORE_SECTIONS: &[&str] = &["ui", "keys", "update", "claude", "extensions"];
+pub const CORE_SECTIONS: &[&str] = &["ui", "keys", "update", "claude", "extensions", "ssh"];
 
 /// The core's command words; never an addon's name.
 pub const RESERVED_WORDS: &[&str] = &[
diff --git a/crates/core/src/client.rs b/crates/core/src/client.rs
index 94281a0..bd37f9d 100644
--- a/crates/core/src/client.rs
+++ b/crates/core/src/client.rs
@@ -224,7 +224,7 @@ fn ssh_tail_line(line: &str) -> Option<String> {
 async fn connect_ssh_with(host: &str, tail: Option<Arc<Mutex<SshTail>>>) -> io::Result<Client> {
     crate::remote::validate_host(host).map_err(io::Error::other)?;
 
-    let mut child = crate::remote::ssh_cmd(host)
+    let mut child = crate::remote::ssh_cmd(host, crate::remote::hosts::forward_agent(host))
         .arg(r#""$HOME"/.local/bin/claudio --slave"#)
         .stdin(Stdio::piped())
         .stdout(Stdio::piped())
diff --git a/crates/core/src/config.rs b/crates/core/src/config.rs
index efc5699..fba6ed3 100644
--- a/crates/core/src/config.rs
+++ b/crates/core/src/config.rs
@@ -257,6 +257,8 @@ pub enum SaveError {
     UnsupportedSection(String),
     /// The name is not an extension's (`^[a-z][a-z0-9-]{0,31}$`).
     BadName,
+    /// Not a host `remote::validate_host` takes (why).
+    BadHost(String),
     Io(io::Error),
 }
 
@@ -273,6 +275,7 @@ impl std::fmt::Display for SaveError {
                 "config.toml layout not understood; edit [{section}] by hand"
             ),
             SaveError::BadName => write!(f, "not an extension name"),
+            SaveError::BadHost(why) => write!(f, "{why}"),
             SaveError::Io(e) => write!(f, "{e}"),
         }
     }
diff --git a/crates/core/src/remote/bootstrap.rs b/crates/core/src/remote/bootstrap.rs
index e5bf405..63fa186 100644
--- a/crates/core/src/remote/bootstrap.rs
+++ b/crates/core/src/remote/bootstrap.rs
@@ -164,7 +164,7 @@ async fn probe_and_platform(host: &str) -> Result<(Option<Probe>, String, String
     let cmd = format!(
         r#"{home_quoted}/.local/bin/claudio __probe 2>/dev/null; echo '---UNAME---'; uname -sm"#
     );
-    let output = tokio::time::timeout(SSH_TIMEOUT, ssh_cmd(host).arg(&cmd).output())
+    let output = tokio::time::timeout(SSH_TIMEOUT, ssh_cmd(host, false).arg(&cmd).output())
         .await
         .map_err(|_| format!("ssh probe/uname timed out on {host}"))?
         .map_err(|e| format!("ssh failed: {e}"))?;
@@ -237,7 +237,7 @@ fn parse_probe(
 /// Returns `None` when the binary is missing or the output is unparseable.
 async fn probe_remote(host: &str) -> Option<Probe> {
     let output = tokio::time::timeout(SSH_TIMEOUT, async {
-        ssh_cmd(host)
+        ssh_cmd(host, false)
             .arg(r#""$HOME"/.local/bin/claudio __probe 2>/dev/null"#)
             .output()
             .await
@@ -380,7 +380,7 @@ async fn upload_bytes(
     // Upload: pipe into ssh's stdin, which writes to the temp file.
     let upload_cmd = format!("cat > {tmp_quoted}");
     let status = tokio::time::timeout(SSH_TIMEOUT, async {
-        let mut child = ssh_cmd(host)
+        let mut child = ssh_cmd(host, false)
             .arg(&upload_cmd)
             .stdin(Stdio::piped())
             .stdout(Stdio::null())
@@ -465,10 +465,12 @@ fi"#
 
 /// Run a shell command on `host` and return its trimmed stdout.
 async fn ssh_run(host: &str, cmd: &str) -> Result<String, String> {
-    let output = tokio::time::timeout(SSH_TIMEOUT, async { ssh_cmd(host).arg(cmd).output().await })
-        .await
-        .map_err(|_| format!("ssh command timed out on {host}"))?
-        .map_err(|e| format!("ssh command failed: {e}"))?;
+    let output = tokio::time::timeout(SSH_TIMEOUT, async {
+        ssh_cmd(host, false).arg(cmd).output().await
+    })
+    .await
+    .map_err(|_| format!("ssh command timed out on {host}"))?
+    .map_err(|e| format!("ssh command failed: {e}"))?;
 
     if !output.status.success() {
         let stderr = String::from_utf8_lossy(&output.stderr);
diff --git a/crates/core/src/remote/hosts.rs b/crates/core/src/remote/hosts.rs
index 24ab1fc..175d4bd 100644
--- a/crates/core/src/remote/hosts.rs
+++ b/crates/core/src/remote/hosts.rs
@@ -7,6 +7,10 @@
 //!
 //! [`touch`] updates the MRU list in `~/.config/claudio/hosts.json` whenever
 //! a remote host is successfully connected.
+//!
+//! [`forward_agent`] and [`set_forward_agent`] read and write `config.toml`'s
+//! core section `[ssh] forward_agent`: the hosts whose bridge ssh asks for
+//! agent forwarding.
 
 use std::collections::HashSet;
 use std::fs;
@@ -14,6 +18,7 @@ use std::path::{Path, PathBuf};
 
 use serde::{Deserialize, Serialize};
 
+use crate::config::SaveError;
 use crate::paths;
 
 /// The hosts MRU file.
@@ -56,6 +61,73 @@ pub fn touch(host: &str) {
     let _ = save_mru(&mru);
 }
 
+// ── [ssh] forward_agent ───────────────────────────────────────────────────────
+
+/// The core section of per-host ssh options, and its forwarding key.
+const SSH_SECTION: &str = "ssh";
+const FORWARD_AGENT: &str = "forward_agent";
+
+/// Whether `host`, exactly as connected, is in `config.toml`'s
+/// `[ssh] forward_agent`, read now.
+pub fn forward_agent(host: &str) -> bool {
+    forward_agent_hosts().iter().any(|h| h == host)
+}
+
+/// `config.toml`'s `[ssh] forward_agent`, read now ([`forward_agent_in`]).
+pub fn forward_agent_hosts() -> Vec<String> {
+    forward_agent_in(&paths::config_file())
+}
+
+/// `[ssh] forward_agent` of the config file at `path`: its entries that
+/// [`super::validate_host`] takes, in order. A missing or unreadable file,
+/// section or key, or a value that is not an array, reads as empty; the rest
+/// of the file is not looked at.
+pub fn forward_agent_in(path: &Path) -> Vec<String> {
+    let table = fs::read_to_string(path)
+        .ok()
+        .and_then(|text| text.parse::<toml::Table>().ok());
+    let items = table
+        .as_ref()
+        .and_then(|t| t.get(SSH_SECTION)?.get(FORWARD_AGENT)?.as_array());
+    items
+        .into_iter()
+        .flatten()
+        .filter_map(toml::Value::as_str)
+        .filter(|h| super::validate_host(h).is_ok())
+        .map(str::to_owned)
+        .collect()
+}
+
+/// [`set_forward_agent_in`] on `config.toml`: the one writer of
+/// `[ssh] forward_agent`; front-ends call it.
+pub fn set_forward_agent(host: &str, on: bool) -> Result<bool, SaveError> {
+    set_forward_agent_in(&paths::config_file(), host, on)
+}
+
+/// Put `host` in (`on`) or take it out of `[ssh] forward_agent` of the
+/// config file at `path` with [`crate::app::config_edit::edit_section`]
+/// (atomic, minimal, serialised). On: the first copy stays where it is, a
+/// later one goes, and a new host goes at the end. Off: every copy goes.
+/// Other entries keep their order; a value that is not an array is
+/// replaced. Whether the file changed.
+pub fn set_forward_agent_in(path: &Path, host: &str, on: bool) -> Result<bool, SaveError> {
+    super::validate_host(host).map_err(SaveError::BadHost)?;
+    crate::app::config_edit::edit_section(path, SSH_SECTION, |table| {
+        let mut items = match table.remove(FORWARD_AGENT) {
+            Some(toml::Value::Array(items)) => items,
+            _ => Vec::new(),
+        };
+        let is_host = |v: &toml::Value| v.as_str() == Some(host);
+        let had = items.iter().any(is_host);
+        let mut first = true;
+        items.retain(|v| !is_host(v) || (on && std::mem::replace(&mut first, false)));
+        if on && !had {
+            items.push(toml::Value::String(host.to_owned()));
+        }
+        table.insert(FORWARD_AGENT.to_owned(), toml::Value::Array(items));
+    })
+}
+
 // ── SSH config parsing ────────────────────────────────────────────────────────
 
 /// Recursion depth limit for `Include` directives.
diff --git a/crates/core/src/remote/mod.rs b/crates/core/src/remote/mod.rs
index d05da1d..422b38c 100644
--- a/crates/core/src/remote/mod.rs
+++ b/crates/core/src/remote/mod.rs
@@ -32,13 +32,15 @@ pub fn validate_host(host: &str) -> Result<(), String> {
 /// Build the standard `ssh` invocation shared by bootstrap and the client
 /// bridge: `-T -o BatchMode=yes -o ConnectTimeout=15
 /// -o ServerAliveInterval=15 -o ServerAliveCountMax=3
-/// -o StrictHostKeyChecking=accept-new -- HOST`.
+/// -o StrictHostKeyChecking=accept-new [-o ForwardAgent=yes] -- HOST`.
 ///
 /// `accept-new` trusts a host's key on first contact (BatchMode cannot ask)
-/// and still refuses a changed key.
+/// and still refuses a changed key. `forward_agent` asks for agent
+/// forwarding (the bridge of a host in `[ssh] forward_agent`); without it
+/// nothing is said about forwarding, so `~/.ssh/config` decides.
 ///
 /// Callers append the remote command as one more `.arg(…)` call.
-pub fn ssh_cmd(host: &str) -> Command {
+pub fn ssh_cmd(host: &str, forward_agent: bool) -> Command {
     let mut cmd = Command::new("ssh");
     cmd.args([
         "-T",
@@ -52,9 +54,11 @@ pub fn ssh_cmd(host: &str) -> Command {
         "ServerAliveCountMax=3",
         "-o",
         "StrictHostKeyChecking=accept-new",
-        "--",
-        host,
     ]);
+    if forward_agent {
+        cmd.args(["-o", "ForwardAgent=yes"]);
+    }
+    cmd.args(["--", host]);
     cmd
 }
 
````
Run: `cargo test --locked -p claudio-core --lib remote::`
Expected: `test result: ok. 41 passed; 0 failed` (among them `ssh_cmd_without_forwarding_is_unchanged`,
`ssh_cmd_forwards_the_agent_before_the_host`, `forward_agent_is_added_removed_and_deduplicated_in_order`,
`forward_agent_section_is_made_when_missing`, `forward_agent_refuses_and_ignores_bad_hosts`,
`forward_agent_that_is_not_an_array_reads_as_empty`).

Step 3: `make check` (A8: `client.rs` stays 856/857 and `bootstrap.rs::upload_bytes` 108/108;
`bootstrap.rs` 501 → 503, under 800). Then commit:
`git commit -m "feat(remote): per-host [ssh] forward_agent and ssh_cmd's forwarding choice" -- crates/core/src/remote/mod.rs crates/core/src/remote/hosts.rs crates/core/src/remote/bootstrap.rs crates/core/src/client.rs crates/core/src/addon/registry.rs crates/core/src/config.rs`

Interfaces this task provides to later tasks:
- `remote::ssh_cmd(host: &str, forward_agent: bool) -> tokio::process::Command`;
- `remote::hosts::{forward_agent(&str) -> bool, forward_agent_hosts() -> Vec<String>,
  forward_agent_in(&Path) -> Vec<String>, set_forward_agent(&str, bool) -> Result<bool,
  SaveError>, set_forward_agent_in(&Path, &str, bool) -> Result<bool, SaveError>}`;
- `config::SaveError::BadHost(String)`.

### Task 3: The bridge links its forwarded agent
Model: opus   Repo: /home/p4u/claudio   Files: `crates/core/src/paths.rs`,
`crates/core/src/remote/agent.rs` (new), `crates/core/src/remote/mod.rs`, `crates/core/src/remote/bridge.rs`
Goal: spec §2 (AC3).
- Before relaying, `claudio --slave` asks its `SSH_AUTH_SOCK` for identities. This runs
  concurrently with starting the daemon (`tokio::join!`), within 2 s, reading only the
  answer's length and type.
- An agent that answers is linked as `<runtime>/agents/<pid>`: `agents/` made private, a
  temporary name renamed into place. The link goes when the bridge returns.
- Every failure is one stderr line; stdout stays clean.
- `paths::{agent_relay_beside, agent_links_beside, agent_links}` give both sides the same
  paths.

Step 1: write the failing tests: `t3-step1.diff`. `remote/agent.rs` holds only its module doc
and tests, against fake agent sockets under `paths::socket_test_dir`. It is declared in
`remote/mod.rs`. A `paths` test is added.
````diff
diff --git a/crates/core/src/paths.rs b/crates/core/src/paths.rs
index a5675d8..c6ca73e 100644
--- a/crates/core/src/paths.rs
+++ b/crates/core/src/paths.rs
@@ -188,6 +188,16 @@ mod tests {
         std::env::temp_dir().join(format!("claudio-paths-test-{}", uuid::Uuid::new_v4()))
     }
 
+    #[test]
+    fn agent_paths_sit_beside_the_daemon_socket() {
+        let dir = Path::new("/run/user/1000/claudio");
+        let sock = dir.join(format!("daemon-v{PROTO}.sock"));
+        let relay = agent_relay_beside(&sock);
+        assert_eq!(relay, dir.join(format!("agent-v{PROTO}.sock")));
+        assert_eq!(agent_links_beside(&sock), dir.join("agents"));
+        assert_eq!(agent_links(), runtime_dir().join("agents"));
+    }
+
     #[test]
     fn socket_name_carries_protocol_version() {
         let s = daemon_socket();
diff --git a/crates/core/src/remote/agent.rs b/crates/core/src/remote/agent.rs
new file mode 100644
index 0000000..71e163d
--- /dev/null
+++ b/crates/core/src/remote/agent.rs
@@ -0,0 +1,139 @@
+//! `claudio --slave`'s side of SSH agent forwarding: when the connection's
+//! `SSH_AUTH_SOCK` answers an identities request, the bridge links it as
+//! `<runtime>/agents/<pid>` for the daemon's relay (docs/manager-design.md,
+//! "Agent forwarding"), and removes the link when it exits. A failure is
+//! one stderr line and never fails the bridge: stdout is the protocol
+//! channel and stays clean.
+
+#[cfg(test)]
+mod tests {
+    use super::*;
+    use tokio::net::UnixListener;
+
+    /// What a fake agent does with a request.
+    #[derive(Clone, Copy)]
+    enum Agent {
+        /// Answers with these bytes.
+        Says(&'static [u8]),
+        /// Closes the connection.
+        Hangs,
+        /// Reads, then never answers.
+        Silent,
+    }
+
+    /// A fake agent at `<dir>/agent.sock`, serving every connection.
+    fn fake_agent(dir: &Path, agent: Agent) -> PathBuf {
+        let path = dir.join("agent.sock");
+        let listener = UnixListener::bind(&path).unwrap();
+        tokio::spawn(async move {
+            while let Ok((mut s, _)) = listener.accept().await {
+                tokio::spawn(async move {
+                    let mut req = [0u8; 5];
+                    if s.read_exact(&mut req).await.is_err() {
+                        return;
+                    }
+                    match agent {
+                        Agent::Says(bytes) => {
+                            let _ = s.write_all(bytes).await;
+                        }
+                        Agent::Hangs => {}
+                        Agent::Silent => tokio::time::sleep(Duration::from_secs(30)).await,
+                    }
+                });
+            }
+        });
+        path
+    }
+
+    fn dir() -> PathBuf {
+        let dir = crate::paths::socket_test_dir("cl-ag");
+        crate::paths::ensure_private_dir(&dir).unwrap();
+        dir
+    }
+
+    const SHORT: Duration = Duration::from_millis(300);
+
+    #[tokio::test]
+    async fn an_answering_agent_is_linked_until_the_link_drops() {
+        let dir = dir();
+        let sock = fake_agent(&dir, Agent::Says(&[0, 0, 0, 5, 12, 0, 0, 0, 0]));
+        let links = dir.join("agents");
+        let link = register(Some(sock.clone().into()), &links, 4242, SHORT)
+            .await
+            .expect("linked");
+        assert_eq!(link.path(), links.join("4242"));
+        assert_eq!(std::fs::read_link(link.path()).unwrap(), sock);
+        use std::os::unix::fs::PermissionsExt;
+        let mode = std::fs::metadata(&links).unwrap().permissions().mode();
+        assert_eq!(mode & 0o777, 0o700);
+        drop(link);
+        assert!(std::fs::symlink_metadata(links.join("4242")).is_err());
+        std::fs::remove_dir_all(&dir).unwrap();
+    }
+
+    #[tokio::test]
+    async fn a_reused_pid_replaces_the_stale_link() {
+        let dir = dir();
+        let sock = fake_agent(&dir, Agent::Says(&[0, 0, 0, 5, 12, 0, 0, 0, 0]));
+        let links = dir.join("agents");
+        crate::paths::ensure_private_dir(&links).unwrap();
+        std::os::unix::fs::symlink("/nonexistent/agent", links.join("7")).unwrap();
+        let link = register(Some(sock.clone().into()), &links, 7, SHORT).await;
+        assert_eq!(std::fs::read_link(links.join("7")).unwrap(), sock);
+        assert!(!links.join(".7.tmp").exists());
+        drop(link);
+        std::fs::remove_dir_all(&dir).unwrap();
+    }
+
+    #[tokio::test]
+    async fn agents_that_do_not_answer_12_are_not_linked() {
+        let cases: [(&str, Agent); 4] = [
+            ("eof", Agent::Hangs),
+            ("wrong type", Agent::Says(&[0, 0, 0, 1, 5])),
+            ("too long", Agent::Says(&[0, 4, 0, 1, 12])),
+            ("silent", Agent::Silent),
+        ];
+        for (what, agent) in cases {
+            let dir = dir();
+            let sock = fake_agent(&dir, agent);
+            let links = dir.join("agents");
+            let start = std::time::Instant::now();
+            let link = register(Some(sock.into()), &links, 1, SHORT).await;
+            assert!(link.is_none(), "{what}");
+            assert!(
+                start.elapsed() < SHORT * 3,
+                "{what}: bounded by the timeout"
+            );
+            assert!(!links.join("1").exists(), "{what}");
+            std::fs::remove_dir_all(&dir).unwrap();
+        }
+    }
+
+    #[tokio::test]
+    async fn no_usable_sock_links_nothing() {
+        let dir = dir();
+        let links = dir.join("agents");
+        let file = dir.join("not-a-socket");
+        std::fs::write(&file, b"").unwrap();
+        for sock in [
+            None,
+            Some(OsString::new()),
+            Some("relative/agent".into()),
+            Some(file.into()),
+            Some(dir.join("missing").into()),
+        ] {
+            assert!(register(sock, &links, 1, SHORT).await.is_none());
+        }
+        assert!(!links.exists(), "agents/ is made only for a working agent");
+        std::fs::remove_dir_all(&dir).unwrap();
+    }
+
+    #[test]
+    fn the_answer_head_is_checked_by_length_and_type() {
+        assert!(answer_ok([0, 0, 0, 5, 12]).is_ok());
+        assert!(answer_ok([0, 4, 0, 0, 12]).is_ok(), "256 KiB is the limit");
+        assert!(answer_ok([0, 4, 0, 1, 12]).is_err());
+        assert!(answer_ok([0, 0, 0, 0, 12]).is_err());
+        assert!(answer_ok([0, 0, 0, 1, 5]).is_err());
+    }
+}
diff --git a/crates/core/src/remote/mod.rs b/crates/core/src/remote/mod.rs
index 422b38c..f1b1f11 100644
--- a/crates/core/src/remote/mod.rs
+++ b/crates/core/src/remote/mod.rs
@@ -2,11 +2,13 @@
 //!
 //! - [`probe`] — `claudio __probe`: prints a JSON fingerprint of this binary.
 //! - [`bridge`] — `claudio --slave`: the remote-side stdio↔socket bridge.
+//! - [`agent`] — the bridge's forwarded SSH agent, linked for the daemon's relay.
 //! - [`hosts`] — SSH host alias discovery and MRU persistence.
 //! - [`bootstrap`] — install or update the claudio binary on a remote host.
 //! - [`diag`] — diagnostic / test-harness subcommands (`__bootstrap`, etc.),
 //!   enabled only with `--features diag`.
 
+pub mod agent;
 pub mod bootstrap;
 pub mod bridge;
 #[cfg(feature = "diag")]
````
Run: `cargo test --locked -p claudio-core --lib -- remote::agent agent_paths`
Expected: it does not compile:
``error[E0425]: cannot find function `register` in this scope``,
``error[E0425]: cannot find function `agent_relay_beside` in this scope``.

Step 2: implement: `t3-step2.diff`.
````diff
diff --git a/crates/core/src/paths.rs b/crates/core/src/paths.rs
index c6ca73e..211fc62 100644
--- a/crates/core/src/paths.rs
+++ b/crates/core/src/paths.rs
@@ -60,6 +60,24 @@ pub fn daemon_socket() -> PathBuf {
     runtime_dir().join(format!("daemon-v{PROTO}.sock"))
 }
 
+/// The SSH agent relay's socket beside the daemon socket `daemon_socket`:
+/// `agent-v{PROTO}.sock`, versioned like it. The daemon derives it from its
+/// own socket, so a test daemon's relay stays in its temp dir.
+pub fn agent_relay_beside(daemon_socket: &Path) -> PathBuf {
+    daemon_socket.with_file_name(format!("agent-v{PROTO}.sock"))
+}
+
+/// Where the bridges beside the daemon socket `daemon_socket` link their
+/// forwarded agents for the relay: `agents/`.
+pub fn agent_links_beside(daemon_socket: &Path) -> PathBuf {
+    daemon_socket.with_file_name("agents")
+}
+
+/// This host's [`agent_links_beside`] the daemon socket: `<runtime>/agents`.
+pub fn agent_links() -> PathBuf {
+    agent_links_beside(&daemon_socket())
+}
+
 /// Lock file serializing daemon startup.
 pub fn daemon_lock() -> PathBuf {
     runtime_dir().join(format!("daemon-v{PROTO}.lock"))
diff --git a/crates/core/src/remote/agent.rs b/crates/core/src/remote/agent.rs
index 71e163d..8a2c780 100644
--- a/crates/core/src/remote/agent.rs
+++ b/crates/core/src/remote/agent.rs
@@ -5,6 +5,121 @@
 //! one stderr line and never fails the bridge: stdout is the protocol
 //! channel and stays clean.
 
+use std::ffi::OsString;
+use std::io;
+use std::path::{Path, PathBuf};
+use std::time::Duration;
+
+use tokio::io::{AsyncReadExt, AsyncWriteExt};
+use tokio::net::UnixStream;
+
+use crate::paths;
+
+/// How long the identities request may take, connect included.
+pub const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
+/// The longest identities answer taken; only its length and type are read.
+const MAX_ANSWER: u32 = 256 * 1024;
+/// `SSH_AGENTC_REQUEST_IDENTITIES` (11), framed: length 1, then the type.
+const REQUEST_IDENTITIES: [u8; 5] = [0, 0, 0, 1, 11];
+/// `SSH_AGENT_IDENTITIES_ANSWER`.
+const IDENTITIES_ANSWER: u8 = 12;
+
+/// This bridge's link under `agents/`; removed (best effort) when dropped,
+/// on every return path of the bridge.
+#[derive(Debug)]
+pub struct Link(PathBuf);
+
+impl Link {
+    /// Where the link is.
+    pub fn path(&self) -> &Path {
+        &self.0
+    }
+}
+
+impl Drop for Link {
+    fn drop(&mut self) {
+        let _ = std::fs::remove_file(&self.0);
+    }
+}
+
+/// Link `sock` (the bridge's `SSH_AUTH_SOCK`) as `links/<pid>` when it is
+/// an absolute path to a Unix socket that answers an identities request
+/// within `timeout`. `None` when it is unset; otherwise any failure is one
+/// line on stderr and `None`. ssh asks for forwarding even when its own
+/// side has no agent, and sshd then makes a socket that answers nothing:
+/// the probe keeps that one out.
+pub async fn register(
+    sock: Option<OsString>,
+    links: &Path,
+    pid: u32,
+    timeout: Duration,
+) -> Option<Link> {
+    let sock = PathBuf::from(sock.filter(|s| !s.is_empty())?);
+    let linked = match probe(&sock, timeout).await {
+        Ok(()) => link(&sock, links, pid),
+        Err(e) => Err(e),
+    };
+    linked
+        .inspect_err(|e| eprintln!("claudio --slave: ssh agent not linked: {e}"))
+        .ok()
+}
+
+/// Ask the agent at `sock` for its identities; `Ok` when it answers with
+/// an identities answer within `timeout`.
+async fn probe(sock: &Path, timeout: Duration) -> io::Result<()> {
+    use std::os::unix::fs::FileTypeExt;
+    if !sock.is_absolute() {
+        return Err(io::Error::other("SSH_AUTH_SOCK is not an absolute path"));
+    }
+    if !std::fs::metadata(sock)?.file_type().is_socket() {
+        return Err(io::Error::other("SSH_AUTH_SOCK is not a socket"));
+    }
+    let ask = async {
+        let mut agent = UnixStream::connect(sock).await?;
+        agent.write_all(&REQUEST_IDENTITIES).await?;
+        let mut head = [0u8; 5];
+        agent.read_exact(&mut head).await?;
+        answer_ok(head).map_err(io::Error::other)
+    };
+    match tokio::time::timeout(timeout, ask).await {
+        Ok(answer) => answer,
+        Err(_) => Err(io::Error::new(
+            io::ErrorKind::TimedOut,
+            "the agent did not answer",
+        )),
+    }
+}
+
+/// Whether `head`, an answer's 4-byte length and its type byte, starts an
+/// identities answer of at most [`MAX_ANSWER`] bytes.
+fn answer_ok(head: [u8; 5]) -> Result<(), String> {
+    let [a, b, c, d, kind] = head;
+    let len = u32::from_be_bytes([a, b, c, d]);
+    if len == 0 || len > MAX_ANSWER {
+        return Err(format!("answer length {len} out of range"));
+    }
+    if kind != IDENTITIES_ANSWER {
+        return Err(format!("answer type {kind}, not an identities answer"));
+    }
+    Ok(())
+}
+
+/// `links/<pid>` → `sock`: a symlink made under a temporary name, then
+/// renamed into place, so a stale link of a reused pid is replaced
+/// atomically. `links` is made private (0700) first.
+fn link(sock: &Path, links: &Path, pid: u32) -> io::Result<Link> {
+    paths::ensure_private_dir(links)?;
+    let tmp = links.join(format!(".{pid}.tmp"));
+    let _ = std::fs::remove_file(&tmp);
+    std::os::unix::fs::symlink(sock, &tmp)?;
+    let path = links.join(pid.to_string());
+    if let Err(e) = std::fs::rename(&tmp, &path) {
+        let _ = std::fs::remove_file(&tmp);
+        return Err(e);
+    }
+    Ok(Link(path))
+}
+
 #[cfg(test)]
 mod tests {
     use super::*;
diff --git a/crates/core/src/remote/bridge.rs b/crates/core/src/remote/bridge.rs
index d4a1e3e..2756c17 100644
--- a/crates/core/src/remote/bridge.rs
+++ b/crates/core/src/remote/bridge.rs
@@ -67,10 +67,22 @@ async fn async_run() -> io::Result<()> {
     paths::ensure_private_dir(&dir)?;
 
     // Ensure the remote daemon is running, and runs this binary (the one the
-    // bootstrap installed) or a newer one.
-    let socket = paths::daemon_socket();
-    ensure_daemon_detached(&socket).await?;
-    ensure_current(&socket).await;
+    // bootstrap installed) or a newer one. Meanwhile, link this connection's
+    // forwarded agent for the daemon's relay; the link goes when we return.
+    let (socket, links) = (paths::daemon_socket(), paths::agent_links());
+    let link = super::agent::register(
+        std::env::var_os("SSH_AUTH_SOCK"),
+        &links,
+        std::process::id(),
+        super::agent::PROBE_TIMEOUT,
+    );
+    let daemon = async {
+        ensure_daemon_detached(&socket).await?;
+        ensure_current(&socket).await;
+        io::Result::Ok(())
+    };
+    let (_link, daemon) = tokio::join!(link, daemon);
+    daemon?;
 
     // Connect to the daemon socket.
     let daemon = UnixStream::connect(&socket).await.map_err(|e| {
````
Run: `cargo test --locked -p claudio-core --lib -- remote::agent agent_paths`
Expected: `test result: ok. 6 passed; 0 failed` (`an_answering_agent_is_linked_until_the_link_drops`,
`a_reused_pid_replaces_the_stale_link`, `agents_that_do_not_answer_12_are_not_linked` (EOF, wrong
type, too long, silent past the timeout), `no_usable_sock_links_nothing`,
`the_answer_head_is_checked_by_length_and_type`, `agent_paths_sit_beside_the_daemon_socket`).
The failure tests print `claudio --slave: ssh agent not linked: …` on stderr; that is expected.

Step 3: `make check` (`bridge.rs` 337 → 349; `remote/agent.rs` 122 production lines). Commit:
`git add crates/core/src/remote/agent.rs`
`git commit -m "feat(remote): the bridge links its forwarded ssh agent for the daemon" -- crates/core/src/paths.rs crates/core/src/remote/agent.rs crates/core/src/remote/mod.rs crates/core/src/remote/bridge.rs`

Interfaces this task provides to later tasks:
- `paths::agent_relay_beside(&Path) -> PathBuf`, `paths::agent_links_beside(&Path) -> PathBuf`,
  `paths::agent_links() -> PathBuf`;
- `remote::agent::{register(Option<OsString>, &Path, u32, Duration) -> Option<Link>,
  PROBE_TIMEOUT, Link}`.

### Task 4: The daemon's agent relay and the sessions' env
Model: opus   Repo: /home/p4u/claudio   Files: `crates/core/src/daemon/agent.rs` (new),
`crates/core/src/daemon/mod.rs`, `crates/core/src/daemon/server.rs`, `crates/core/src/daemon/command.rs`,
`crates/core/src/daemon/session.rs`, `crates/core/src/daemon/test_support.rs`,
`crates/core/src/daemon/tests.rs`, `crates/core/src/daemon/tests/agent.rs` (new)
Goal: spec §3 (AC4, AC5).
- Every daemon listens on `agent-v{PROTO}.sock` beside its socket: bound like it (stale file
  removed, 0600, `server::bind` made `pub(super)`), peer uid checked, at most 32 connections.
- A connection goes to the newest live link, then to the daemon's inherited agent. Dead links
  are pruned.
- Claude sessions and terminals get `SSH_AUTH_SOCK=<relay>` once `agents/` exists.
- `Config.agent: AgentRelay` goes right after `socket` in the struct, in `from_env` and in
  every test literal (Merge step 3).

Step 1: write the failing tests: `t4-step1.diff`.
- AC4 and AC5 go in `daemon/tests/agent.rs`, against fake agents in the test daemon's dir.
- `command.rs`'s tests get a `daemon_config` helper and the new `Config` field.
````diff
diff --git a/crates/core/src/daemon/command.rs b/crates/core/src/daemon/command.rs
index b8b3c30..b71977b 100644
--- a/crates/core/src/daemon/command.rs
+++ b/crates/core/src/daemon/command.rs
@@ -215,6 +215,14 @@ mod tests {
         assert_eq!(clean_title("◑ "), "");
     }
 
+    /// A daemon config whose login `PATH` is `path`, its relay not listening.
+    fn daemon_config(path: Option<&OsStr>) -> super::super::Config {
+        super::super::Config {
+            path: path.map(OsStr::to_os_string),
+            ..super::super::test_support::TestDaemon::config(Path::new("/run"))
+        }
+    }
+
     /// Flags as a claude whose `--help` lists those marked true.
     fn knowing(flags: &[(&str, bool)]) -> ClaudeFlags {
         let help: Vec<&str> = flags
@@ -257,7 +265,11 @@ mod tests {
 
     #[test]
     fn shell_command_is_a_plain_login_shell() {
-        let cmd = shell_command(Path::new("/bin/zsh"), Path::new("/tmp"), None);
+        let cmd = shell_command(
+            Path::new("/bin/zsh"),
+            Path::new("/tmp"),
+            &daemon_config(None),
+        );
         let argv: Vec<_> = cmd
             .get_argv()
             .iter()
@@ -299,7 +311,11 @@ mod tests {
         apply_env(&mut cmd, None, &[], None);
         assert_eq!(cmd.get_env("PATH"), Some(OsStr::new("/usr/bin")));
         // Terminals get it too.
-        let cmd = shell_command(Path::new("/bin/sh"), Path::new("/tmp"), Some(login));
+        let cmd = shell_command(
+            Path::new("/bin/sh"),
+            Path::new("/tmp"),
+            &daemon_config(Some(login)),
+        );
         assert_eq!(cmd.get_env("PATH"), Some(login));
     }
 
@@ -315,6 +331,7 @@ mod tests {
     fn command_line_shape() {
         let cfg = super::super::Config {
             socket: "/run/d.sock".into(),
+            agent: Default::default(),
             lock: "/run/d.lock".into(),
             journal: "/j.json".into(),
             claude: "/bin/claude".into(),
@@ -397,6 +414,7 @@ mod tests {
     fn skip_permissions_not_duplicated() {
         let cfg = super::super::Config {
             socket: "/run/d.sock".into(),
+            agent: Default::default(),
             lock: "/run/d.lock".into(),
             journal: "/j.json".into(),
             claude: "/bin/claude".into(),
@@ -442,6 +460,7 @@ mod tests {
     fn plugin_dirs_are_dropped_for_a_claude_without_the_flag() {
         let cfg = super::super::Config {
             socket: "/run/d.sock".into(),
+            agent: Default::default(),
             lock: "/run/d.lock".into(),
             journal: "/j.json".into(),
             claude: "/bin/claude".into(),
@@ -499,6 +518,7 @@ mod tests {
     fn plugin_flags_keep_their_order_and_are_dropped_when_unknown() {
         let cfg = super::super::Config {
             socket: "/run/d.sock".into(),
+            agent: Default::default(),
             lock: "/run/d.lock".into(),
             journal: "/j.json".into(),
             claude: "/bin/claude".into(),
@@ -571,6 +591,7 @@ mod tests {
     fn optional_flags_are_dropped_only_when_unknown() {
         let cfg = super::super::Config {
             socket: "/run/d.sock".into(),
+            agent: Default::default(),
             lock: "/run/d.lock".into(),
             journal: "/j.json".into(),
             claude: "/bin/claude".into(),
@@ -761,7 +782,7 @@ mod tests {
 
     #[test]
     fn a_shell_loses_only_the_markers() {
-        let cmd = shell_command(Path::new("/bin/sh"), Path::new("/"), None);
+        let cmd = shell_command(Path::new("/bin/sh"), Path::new("/"), &daemon_config(None));
         // Every inherited variable but the markers is still there.
         for (key, _) in std::env::vars_os() {
             let key = key.to_string_lossy();
diff --git a/crates/core/src/daemon/mod.rs b/crates/core/src/daemon/mod.rs
index 704b5a4..3234030 100644
--- a/crates/core/src/daemon/mod.rs
+++ b/crates/core/src/daemon/mod.rs
@@ -861,6 +861,7 @@ mod claude_path_tests {
     fn config(claude: PathBuf, path: Option<OsString>) -> Config {
         Config {
             socket: "/run/d.sock".into(),
+            agent: Default::default(),
             lock: "/run/d.lock".into(),
             journal: "/j.json".into(),
             claude,
diff --git a/crates/core/src/daemon/test_support.rs b/crates/core/src/daemon/test_support.rs
index 37a51db..525d8c9 100644
--- a/crates/core/src/daemon/test_support.rs
+++ b/crates/core/src/daemon/test_support.rs
@@ -150,6 +150,7 @@ impl TestDaemon {
     pub fn config(dir: &Path) -> Config {
         Config {
             socket: dir.join("rt/d.sock"),
+            agent: Default::default(),
             lock: dir.join("rt/d.lock"),
             journal: dir.join("state/journal.json"),
             claude: fake_claude().to_path_buf(),
diff --git a/crates/core/src/daemon/tests.rs b/crates/core/src/daemon/tests.rs
index 525429f..d49995f 100644
--- a/crates/core/src/daemon/tests.rs
+++ b/crates/core/src/daemon/tests.rs
@@ -560,6 +560,7 @@ async fn resume_retry_spawns_fresh_on_quick_exit() {
     let dir = TestDaemon::new_dir();
     let config = Config {
         socket: dir.join("rt/d.sock"),
+        agent: Default::default(),
         lock: dir.join("rt/d.lock"),
         journal: dir.join("state/journal.json"),
         claude: fake_claude_resume_fails().to_path_buf(),
@@ -684,6 +685,7 @@ async fn immediate_exit_leaves_session_dormant() {
     let dir = TestDaemon::new_dir();
     let config = Config {
         socket: dir.join("rt/d.sock"),
+        agent: Default::default(),
         lock: dir.join("rt/d.lock"),
         journal: dir.join("state/journal.json"),
         claude: fake_claude_exits_immediately().to_path_buf(),
@@ -1044,6 +1046,7 @@ async fn spawn_for_a_journaled_shell_starts_a_shell() {
     );
 }
 mod addon_ops;
+mod agent;
 mod freshness;
 mod git;
 mod login_path;
diff --git a/crates/core/src/daemon/tests/agent.rs b/crates/core/src/daemon/tests/agent.rs
new file mode 100644
index 0000000..11c95f3
--- /dev/null
+++ b/crates/core/src/daemon/tests/agent.rs
@@ -0,0 +1,213 @@
+//! Daemon tests of the SSH agent relay (docs/manager-design.md, "Agent
+//! forwarding"): which linked agent a connection reaches, pruning, the
+//! cap, the socket's mode, and the env sessions and terminals get. The
+//! agents are fake sockets in the daemon's temp dir; the test daemon has
+//! no inherited agent, so the owner's `SSH_AUTH_SOCK` is never used.
+
+use std::os::unix::fs::{symlink, PermissionsExt};
+
+use tokio::io::{AsyncReadExt, AsyncWriteExt};
+use tokio::net::{UnixListener, UnixStream};
+
+use super::*;
+use crate::paths;
+
+/// A fake agent at `path`: it greets every connection with `tag`, then
+/// echoes. Dropping it closes its listener (the socket file stays, so a
+/// connect is refused).
+struct FakeAgent(tokio::task::JoinHandle<()>);
+
+impl FakeAgent {
+    fn start(path: &Path, tag: &'static [u8]) -> FakeAgent {
+        let listener = UnixListener::bind(path).unwrap();
+        FakeAgent(tokio::spawn(async move {
+            while let Ok((mut s, _)) = listener.accept().await {
+                tokio::spawn(async move {
+                    let (mut rd, mut wr) = s.split();
+                    if wr.write_all(tag).await.is_ok() {
+                        let _ = tokio::io::copy(&mut rd, &mut wr).await;
+                    }
+                });
+            }
+        }))
+    }
+}
+
+impl Drop for FakeAgent {
+    fn drop(&mut self) {
+        self.0.abort();
+    }
+}
+
+/// The relay's socket, once the daemon listens on it, and its `agents/`.
+async fn relay_of(d: &TestDaemon) -> (PathBuf, PathBuf) {
+    let relay = paths::agent_relay_beside(&d.config.socket);
+    within(async {
+        while !relay.exists() {
+            tokio::time::sleep(Duration::from_millis(20)).await;
+        }
+    })
+    .await;
+    (relay, paths::agent_links_beside(&d.config.socket))
+}
+
+/// What a new relay connection is greeted with: the agent's tag, or ""
+/// when the relay closes it.
+async fn greeting(relay: &Path) -> String {
+    let mut s = UnixStream::connect(relay).await.unwrap();
+    let mut buf = [0u8; 1];
+    let n = within(s.read(&mut buf)).await.unwrap_or(0);
+    String::from_utf8_lossy(&buf[..n]).into_owned()
+}
+
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn relay_reaches_the_newest_live_agent_and_prunes_dead_links() {
+    let d = TestDaemon::start().await;
+    let (relay, links) = relay_of(&d).await;
+    let mode = std::fs::metadata(&relay).unwrap().permissions().mode();
+    assert_eq!(mode & 0o777, 0o600);
+    paths::ensure_private_dir(&links).unwrap();
+    let a = FakeAgent::start(&d.dir.join("a.sock"), b"A");
+    symlink(d.dir.join("a.sock"), links.join("100")).unwrap();
+    let b = FakeAgent::start(&d.dir.join("b.sock"), b"B");
+    symlink(d.dir.join("b.sock"), links.join("200")).unwrap();
+
+    assert_eq!(greeting(&relay).await, "B", "the newer link wins");
+    drop(b);
+    assert_eq!(greeting(&relay).await, "A");
+    assert!(
+        std::fs::symlink_metadata(links.join("200")).is_err(),
+        "B's link is gone"
+    );
+    drop(a);
+    assert_eq!(greeting(&relay).await, "", "no live agent: closed");
+    assert!(std::fs::symlink_metadata(links.join("100")).is_err());
+}
+
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn relay_takes_32_connections_and_closes_the_33rd() {
+    let d = TestDaemon::start().await;
+    let (relay, links) = relay_of(&d).await;
+    paths::ensure_private_dir(&links).unwrap();
+    let _c = FakeAgent::start(&d.dir.join("c.sock"), b"C");
+    symlink(d.dir.join("c.sock"), links.join("1")).unwrap();
+    let mut held = Vec::new();
+    for _ in 0..32 {
+        let mut s = UnixStream::connect(&relay).await.unwrap();
+        let mut tag = [0u8; 1];
+        within(s.read_exact(&mut tag)).await.unwrap();
+        assert_eq!(&tag, b"C");
+        held.push(s);
+    }
+    assert_eq!(greeting(&relay).await, "", "the 33rd is closed");
+    // The held ones still relay both ways.
+    held[0].write_all(b"x").await.unwrap();
+    let mut echo = [0u8; 1];
+    within(held[0].read_exact(&mut echo)).await.unwrap();
+    assert_eq!(&echo, b"x");
+    // One closed frees its slot.
+    held.pop();
+    within(async {
+        while greeting(&relay).await != "C" {
+            tokio::time::sleep(Duration::from_millis(50)).await;
+        }
+    })
+    .await;
+}
+
+/// A fake claude that writes to `agent.txt` in its cwd which agent it got:
+/// `relay` (its `SSH_AUTH_SOCK` is the spec's `EXPECT_AGENT`), `explicit`,
+/// `unset` or `other` (never the value).
+fn fake_claude_agent() -> &'static Path {
+    static BIN: OnceLock<PathBuf> = OnceLock::new();
+    BIN.get_or_init(|| {
+        let dir = private_temp_dir("claudio-fake-claude-agent");
+        let bin = dir.join("claude");
+        let script = format!(
+            "#!/bin/sh\n\
+             if [ \"$1\" = \"--version\" ]; then echo '9.9.9 (Claude Code)'; exit 0; fi\n\
+             if [ \"$1\" = \"--help\" ]; then exit 0; fi\n\
+             case \"$SSH_AUTH_SOCK\" in\n\
+               \"$EXPECT_AGENT\") seen=relay ;;\n\
+               /explicit/agent) seen=explicit ;;\n\
+               '') seen=unset ;;\n\
+               *) seen=other ;;\n\
+             esac\n\
+             echo \"$seen\" > agent.tmp && mv agent.tmp agent.txt\n\
+             echo {BANNER}\n\
+             exec cat\n"
+        );
+        crate::daemon::host::write_executable(&bin, &script);
+        bin
+    })
+}
+
+/// Spawn a claude session with `edit` applied to its spec; what agent it got.
+async fn claude_agent(
+    d: &TestDaemon,
+    c: &mut Client,
+    relay: &Path,
+    edit: impl FnOnce(&mut SpawnSpec),
+) -> String {
+    let seen = d.work().join("agent.txt");
+    let _ = std::fs::remove_file(&seen);
+    let mut spec = d.spec(Uuid::new_v4());
+    spec.env
+        .push(("EXPECT_AGENT".into(), relay.to_string_lossy().into_owned()));
+    edit(&mut spec);
+    c.spawn(spec).await;
+    within(async {
+        loop {
+            match std::fs::read_to_string(&seen) {
+                Ok(text) if text.ends_with('\n') => return text.trim().to_owned(),
+                _ => tokio::time::sleep(Duration::from_millis(20)).await,
+            }
+        }
+    })
+    .await
+}
+
+/// Whether a new terminal's `SSH_AUTH_SOCK` is the relay.
+async fn shell_has_relay(d: &TestDaemon, c: &mut Client, relay: &Path) -> bool {
+    let id = Uuid::new_v4();
+    c.spawn_shell(d.shell_spec(id)).await;
+    c.attach(id, 24, 80).await;
+    // The answer is computed, so the echoed command line never matches it.
+    let test = format!(
+        "[ \"$SSH_AUTH_SOCK\" = '{}' ] && echo relay-$((40+2)) || echo other-$((40+2))",
+        relay.display()
+    );
+    c.type_line(id, &test).await;
+    c.output_until(id, "-42").await.contains("relay-42")
+}
+
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn sessions_get_the_relay_once_a_bridge_linked_an_agent() {
+    let dir = TestDaemon::new_dir();
+    let d = TestDaemon::start_with_claude(dir, fake_claude_agent().to_path_buf()).await;
+    let (relay, links) = relay_of(&d).await;
+    let mut c = d.client().await;
+    // No bridge ever linked an agent here: the daemon's own value.
+    let own = match std::env::var_os("SSH_AUTH_SOCK") {
+        Some(v) if !v.is_empty() => "other",
+        _ => "unset",
+    };
+    assert_eq!(claude_agent(&d, &mut c, &relay, |_| {}).await, own);
+    assert!(!shell_has_relay(&d, &mut c, &relay).await);
+
+    // A bridge linked a live agent: claude sessions and terminals get the relay.
+    paths::ensure_private_dir(&links).unwrap();
+    let _a = FakeAgent::start(&d.dir.join("a.sock"), b"A");
+    symlink(d.dir.join("a.sock"), links.join("100")).unwrap();
+    assert_eq!(claude_agent(&d, &mut c, &relay, |_| {}).await, "relay");
+    assert!(shell_has_relay(&d, &mut c, &relay).await);
+
+    // A spawn that scrubs the agent gets none; an explicit value wins.
+    let scrub = |s: &mut SpawnSpec| s.env_remove = Some(vec!["SSH_AUTH_SOCK".into()]);
+    assert_eq!(claude_agent(&d, &mut c, &relay, scrub).await, "unset");
+    let explicit = |s: &mut SpawnSpec| {
+        s.env
+            .push(("SSH_AUTH_SOCK".into(), "/explicit/agent".into()))
+    };
+    assert_eq!(claude_agent(&d, &mut c, &relay, explicit).await, "explicit");
+}
````
Run: `cargo test --locked -p claudio-core --lib -- daemon::tests::agent daemon::command daemon::agent`
Expected: it does not compile:
``error[E0560]: struct `daemon::Config` has no field named `agent` `` and `error[E0308]: mismatched types`
(`shell_command` still takes a `PATH`).

Step 2: implement: `t4-step2.diff`.
````diff
diff --git a/crates/core/src/daemon/agent.rs b/crates/core/src/daemon/agent.rs
new file mode 100644
index 0000000..0b8dcd0
--- /dev/null
+++ b/crates/core/src/daemon/agent.rs
@@ -0,0 +1,241 @@
+//! The daemon's SSH agent relay. It listens on `agent-v{PROTO}.sock` beside
+//! the daemon socket (0600, peer uid checked like the daemon socket), and
+//! sessions get that path as `SSH_AUTH_SOCK` once a bridge has linked an
+//! agent here (`agents/` exists). Each connection goes to the newest live
+//! link a bridge made (`remote::agent`), else to the daemon's own inherited
+//! agent, so a session outlives the connection that forwarded its agent
+//! (docs/manager-design.md, "Agent forwarding"). The traffic is copied,
+//! never parsed, logged or stored.
+
+use std::ffi::OsString;
+use std::io;
+use std::path::{Path, PathBuf};
+use std::sync::{Arc, OnceLock};
+use std::time::{Duration, SystemTime};
+
+use portable_pty::CommandBuilder;
+use tokio::net::{UnixListener, UnixStream};
+use tokio::sync::Semaphore;
+use tokio::task::{JoinHandle, JoinSet};
+
+use super::Daemon;
+use crate::paths;
+
+/// Relayed connections at once; one more is closed when accepted.
+const MAX_RELAYS: usize = 32;
+/// How long one agent may take to accept a connection.
+const CONNECT_TIMEOUT: Duration = Duration::from_secs(1);
+/// The most `agents/` entries looked at, and links tried, per connection.
+const MAX_ENTRIES: usize = 1024;
+const MAX_LINKS: usize = 64;
+
+/// The relay as the daemon's [`super::Config`] holds it: the daemon's own
+/// inherited `SSH_AUTH_SOCK` (tried after the links, never pruned) and,
+/// once the relay listens, its path. Clones share the listening state.
+#[derive(Debug, Clone, Default)]
+pub struct AgentRelay {
+    /// `SSH_AUTH_SOCK` as the daemon inherited it, when absolute.
+    pub fallback: Option<PathBuf>,
+    listening: Arc<OnceLock<PathBuf>>,
+}
+
+impl AgentRelay {
+    /// The relay of a daemon that inherited `sock` as its `SSH_AUTH_SOCK`.
+    pub fn inherited(sock: Option<OsString>) -> AgentRelay {
+        AgentRelay {
+            fallback: sock.map(PathBuf::from).filter(|p| p.is_absolute()),
+            listening: Arc::default(),
+        }
+    }
+
+    /// Point a session at the relay (`SSH_AUTH_SOCK`) when it listens and a
+    /// bridge has linked an agent here at least once: `agents/` exists, so
+    /// a host no bridge reaches (the local one) keeps its own agent. Runs
+    /// before the spawn's env is applied: an `env_remove` naming
+    /// `SSH_AUTH_SOCK` takes it away again, and a value in the env wins.
+    pub(super) fn apply(&self, cmd: &mut CommandBuilder) {
+        let Some(relay) = self.listening.get() else {
+            return;
+        };
+        let links = paths::agent_links_beside(relay);
+        if std::fs::symlink_metadata(links).is_ok_and(|m| m.is_dir()) {
+            cmd.env("SSH_AUTH_SOCK", relay);
+        }
+    }
+}
+
+/// The relay's accept loop; dropping it stops listening and every relayed
+/// connection.
+pub(super) struct Serving(JoinHandle<()>);
+
+impl Drop for Serving {
+    fn drop(&mut self) {
+        self.0.abort();
+    }
+}
+
+/// Bind the relay beside the daemon socket and serve it until the returned
+/// [`Serving`] is dropped. A bind failure is logged once and leaves the
+/// relay off (and sessions' env as it was); the daemon still serves.
+pub(super) fn start(daemon: &Arc<Daemon>) -> Option<Serving> {
+    let config = &daemon.config;
+    let path = paths::agent_relay_beside(&config.socket);
+    let listener = super::server::bind(&path)
+        .inspect_err(|e| tracing::warn!(error = %e, "agent relay off: cannot bind"))
+        .ok()?;
+    let fallback = config.agent.fallback.clone().filter(|f| *f != path);
+    let links = paths::agent_links_beside(&path);
+    let _ = config.agent.listening.set(path);
+    Some(Serving(tokio::spawn(serve(listener, links, fallback))))
+}
+
+/// Accept forever: our own uid only, at most [`MAX_RELAYS`] at once.
+async fn serve(listener: UnixListener, links: PathBuf, fallback: Option<PathBuf>) {
+    let slots = Arc::new(Semaphore::new(MAX_RELAYS));
+    let mut relays = JoinSet::new();
+    loop {
+        tokio::select! {
+            accepted = listener.accept() => {
+                let Ok((client, _)) = accepted else {
+                    tokio::time::sleep(Duration::from_millis(100)).await;
+                    continue;
+                };
+                if !own_peer(&client) {
+                    continue;
+                }
+                let Ok(slot) = Arc::clone(&slots).try_acquire_owned() else {
+                    tracing::debug!("agent relay: {MAX_RELAYS} connections already; closed one");
+                    continue;
+                };
+                let (links, fallback) = (links.clone(), fallback.clone());
+                relays.spawn(async move {
+                    relay(client, &links, fallback.as_deref()).await;
+                    drop(slot);
+                });
+            }
+            Some(_) = relays.join_next(), if !relays.is_empty() => {}
+        }
+    }
+}
+
+/// Whether the peer runs as our uid (the daemon socket's guard).
+fn own_peer(client: &UnixStream) -> bool {
+    match client.peer_cred() {
+        Ok(cred) if cred.uid() == paths::uid() => true,
+        Ok(cred) => {
+            tracing::warn!(uid = cred.uid(), "agent relay: refused another user");
+            false
+        }
+        Err(e) => {
+            tracing::warn!(error = %e, "agent relay: no peer credentials");
+            false
+        }
+    }
+}
+
+/// Copy bytes between `client` and the first live agent until either side
+/// closes; no live agent closes `client`.
+async fn relay(mut client: UnixStream, links: &Path, fallback: Option<&Path>) {
+    let Some(mut agent) = connect_agent(links, fallback).await else {
+        tracing::debug!("agent relay: no live agent");
+        return;
+    };
+    let _ = tokio::io::copy_bidirectional(&mut client, &mut agent).await;
+}
+
+/// The links newest first, then `fallback`: the first that accepts within
+/// [`CONNECT_TIMEOUT`]. A link whose target refuses, is gone or is not ours
+/// to use is removed (the link is ours even when the target is not); a
+/// timeout or another error keeps it. `fallback` is never removed.
+async fn connect_agent(links: &Path, fallback: Option<&Path>) -> Option<UnixStream> {
+    let dir = links.to_owned();
+    let ordered = tokio::task::spawn_blocking(move || newest_first(&dir))
+        .await
+        .unwrap_or_default();
+    for link in ordered {
+        match tokio::time::timeout(CONNECT_TIMEOUT, UnixStream::connect(&link)).await {
+            Ok(Ok(agent)) => return Some(agent),
+            Ok(Err(e)) if dead(&e) => {
+                let _ = tokio::fs::remove_file(&link).await;
+            }
+            _ => {}
+        }
+    }
+    let connect = UnixStream::connect(fallback?);
+    tokio::time::timeout(CONNECT_TIMEOUT, connect)
+        .await
+        .ok()?
+        .ok()
+}
+
+/// A link whose target will not come back: refused, gone, or forbidden.
+fn dead(e: &io::Error) -> bool {
+    matches!(
+        e.kind(),
+        io::ErrorKind::ConnectionRefused
+            | io::ErrorKind::NotFound
+            | io::ErrorKind::PermissionDenied
+    )
+}
+
+/// The symlinks in `dir`, newest `lstat` mtime first, ties by file name
+/// descending; at most [`MAX_LINKS`] of the first [`MAX_ENTRIES`] entries.
+fn newest_first(dir: &Path) -> Vec<PathBuf> {
+    let Ok(entries) = std::fs::read_dir(dir) else {
+        return Vec::new();
+    };
+    let mut links: Vec<(SystemTime, OsString, PathBuf)> = entries
+        .take(MAX_ENTRIES)
+        .filter_map(Result::ok)
+        .filter_map(|e| {
+            let meta = e.metadata().ok().filter(|m| m.file_type().is_symlink())?;
+            Some((meta.modified().ok()?, e.file_name(), e.path()))
+        })
+        .collect();
+    links.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
+    links
+        .into_iter()
+        .take(MAX_LINKS)
+        .map(|(_, _, p)| p)
+        .collect()
+}
+
+#[cfg(test)]
+mod tests {
+    use super::*;
+
+    #[test]
+    fn links_are_newest_first_ties_by_name_and_only_symlinks() {
+        let dir = crate::paths::socket_test_dir("cl-ar");
+        crate::paths::ensure_private_dir(&dir).unwrap();
+        for name in ["100", "300", "200"] {
+            std::os::unix::fs::symlink("/nowhere", dir.join(name)).unwrap();
+        }
+        std::fs::write(dir.join("999"), b"not a link").unwrap();
+        let names: Vec<String> = newest_first(&dir)
+            .iter()
+            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
+            .collect();
+        // Made within one tick or not, "300" was made after "100": newest first
+        // or, on a tie, the larger name first, never the plain file.
+        assert_eq!(names.len(), 3, "{names:?}");
+        assert!(!names.contains(&"999".to_owned()));
+        let pos = |n: &str| names.iter().position(|x| x == n).unwrap();
+        assert!(pos("300") < pos("100"), "{names:?}");
+        std::fs::remove_dir_all(&dir).unwrap();
+    }
+
+    #[test]
+    fn a_relay_that_does_not_listen_changes_no_env() {
+        let mut cmd = CommandBuilder::new("claude");
+        cmd.env_remove("SSH_AUTH_SOCK");
+        AgentRelay::inherited(Some("relative".into())).apply(&mut cmd);
+        assert_eq!(cmd.get_env("SSH_AUTH_SOCK"), None);
+        assert_eq!(
+            AgentRelay::inherited(Some("relative".into())).fallback,
+            None
+        );
+        let abs = AgentRelay::inherited(Some("/tmp/a.sock".into()));
+        assert_eq!(abs.fallback.as_deref(), Some(Path::new("/tmp/a.sock")));
+    }
+}
diff --git a/crates/core/src/daemon/command.rs b/crates/core/src/daemon/command.rs
index b71977b..2d0622c 100644
--- a/crates/core/src/daemon/command.rs
+++ b/crates/core/src/daemon/command.rs
@@ -68,6 +68,7 @@ pub(super) fn command(
         cmd.arg(name);
     }
     cmd.cwd(cwd);
+    cfg.agent.apply(&mut cmd);
     let env_remove = spec.env_remove.as_deref();
     apply_env(&mut cmd, cfg.path.as_deref(), &spec.env, env_remove);
     cmd
@@ -181,12 +182,14 @@ pub(super) fn optional_ok(list: &[String]) -> Result<(), String> {
 
 /// `<shell> -l` in `cwd`: a plain terminal. No hooks, no proxy env, and
 /// only the claude session markers are scrubbed, so a `claude` typed into it
-/// is a top-level session, not a child of whatever started the daemon.
-pub(super) fn shell_command(shell: &Path, cwd: &Path, path: Option<&OsStr>) -> CommandBuilder {
+/// is a top-level session, not a child of whatever started the daemon. It
+/// gets the login `PATH` and the agent relay as a claude session does.
+pub(super) fn shell_command(shell: &Path, cwd: &Path, cfg: &super::Config) -> CommandBuilder {
     let mut cmd = CommandBuilder::new(shell);
     cmd.arg("-l");
     cmd.cwd(cwd);
-    apply_env(&mut cmd, path, &[], Some(&[]));
+    cfg.agent.apply(&mut cmd);
+    apply_env(&mut cmd, cfg.path.as_deref(), &[], Some(&[]));
     cmd
 }
 
diff --git a/crates/core/src/daemon/mod.rs b/crates/core/src/daemon/mod.rs
index 3234030..9405ff7 100644
--- a/crates/core/src/daemon/mod.rs
+++ b/crates/core/src/daemon/mod.rs
@@ -20,6 +20,7 @@
 
 mod addon_ops;
 mod admit;
+mod agent;
 mod command;
 pub mod ctl;
 mod git;
@@ -72,6 +73,8 @@ const STATUS_TIMEOUT: Duration = Duration::from_secs(2);
 pub struct Config {
     /// The control socket (also handed to hooks).
     pub socket: PathBuf,
+    /// The SSH agent relay beside it ([`agent`]): its inherited fallback.
+    pub agent: agent::AgentRelay,
     /// The startup lock file; held for the daemon's lifetime.
     pub lock: PathBuf,
     /// The session journal.
@@ -121,6 +124,7 @@ impl Config {
         let claude = crate::config::load().claude;
         Ok(Config {
             socket: paths::daemon_socket(),
+            agent: agent::AgentRelay::inherited(std::env::var_os("SSH_AUTH_SOCK")),
             lock: paths::daemon_lock(),
             journal: paths::daemon_journal(),
             claude: claude_setting(std::env::var_os("CLAUDIO_CLAUDE_PATH")),
@@ -240,7 +244,9 @@ const SHUTDOWN_GRACE: Duration = Duration::from_secs(1);
 async fn serve_until_shutdown(listening: server::Listening) -> std::fs::File {
     let daemon = listening.daemon();
     service::Services::start_all(&daemon);
+    let relay = agent::start(&daemon);
     let lock = listening.serve().await;
+    drop(relay);
     let grace = daemon.config.service_timing.stop;
     daemon.services.stop_all(grace).await;
     lock
diff --git a/crates/core/src/daemon/server.rs b/crates/core/src/daemon/server.rs
index 5df0725..940bfae 100644
--- a/crates/core/src/daemon/server.rs
+++ b/crates/core/src/daemon/server.rs
@@ -112,8 +112,8 @@ pub fn start(config: Config, addons: Arc<crate::addon::Addons>) -> io::Result<Op
 }
 
 /// Bind `path`, replacing a stale socket left by a dead daemon (we hold the
-/// lock, so no live daemon owns it).
-fn bind(path: &Path) -> io::Result<UnixListener> {
+/// lock, so no live daemon owns it). The agent relay binds the same way.
+pub(super) fn bind(path: &Path) -> io::Result<UnixListener> {
     match std::fs::remove_file(path) {
         Ok(()) => {}
         Err(e) if e.kind() == io::ErrorKind::NotFound => {}
diff --git a/crates/core/src/daemon/session.rs b/crates/core/src/daemon/session.rs
index c4dd043..e734d35 100644
--- a/crates/core/src/daemon/session.rs
+++ b/crates/core/src/daemon/session.rs
@@ -172,7 +172,6 @@ pub fn spawn(
         )));
     }
     let token = hooks::new_token();
-    let path = daemon.config.path.as_deref();
     let (cmd, program) = match kind {
         SessionKind::Claude => {
             let context = write_context(&daemon.config.socket, spec);
@@ -190,7 +189,7 @@ pub fn spawn(
         }
         SessionKind::Shell => {
             let shell = super::login_path::login_shell();
-            (shell_command(&shell, &cwd, path), shell)
+            (shell_command(&shell, &cwd, &daemon.config), shell)
         }
     };
     let (rows, cols) = size_or_default(spec.rows, spec.cols);
````
Run: `cargo test --locked -p claudio-core --lib -- daemon::tests::agent daemon::command daemon::agent`
Expected: `test result: ok. 20 passed; 0 failed`, among them:
- `relay_reaches_the_newest_live_agent_and_prunes_dead_links` (B, then A after B's listener
  closes and its link is gone, then closed; 0600);
- `relay_takes_32_connections_and_closes_the_33rd`;
- `sessions_get_the_relay_once_a_bridge_linked_an_agent`;
- `links_are_newest_first_ties_by_name_and_only_symlinks`.

Then the whole crate: `cargo test --locked -p claudio-core --lib` → `test result: ok. 559 passed`.

Step 3: `make check`. A8 numbers:
- `daemon/mod.rs` 780 → 786 (budget 800);
- `session.rs` 1036 → 1035 and `session.rs::spawn` 105 → 104 (baseline items, shrunk);
- `server.rs` 900 → 900;
- `daemon/agent.rs` 202 production lines.

Then commit:
`git add crates/core/src/daemon/agent.rs crates/core/src/daemon/tests/agent.rs`
`git commit -m "feat(daemon): relay sessions' ssh agent to the newest live forwarded one" -- crates/core/src/daemon/agent.rs crates/core/src/daemon/tests/agent.rs crates/core/src/daemon/mod.rs crates/core/src/daemon/server.rs crates/core/src/daemon/command.rs crates/core/src/daemon/session.rs crates/core/src/daemon/test_support.rs crates/core/src/daemon/tests.rs`

Interfaces this task provides to later tasks: `daemon::Config.agent` (`AgentRelay`; tests use
`Default::default()`, and `TestDaemon::config` sets it). The relay at
`paths::agent_relay_beside(&config.socket)`. Sessions' `SSH_AUTH_SOCK` rule.

### Task 5: The web server's `hosts.forward_agent`, the Host DTO, and a fake remote for the suites
Model: opus   Repo: /home/p4u/claudio   Files: `addons/web/src/hosts.rs` (new),
`addons/web/src/dto.rs`, `addons/web/src/ext.rs`, `addons/web/src/hub.rs`, `addons/web/src/lib.rs`,
`addons/web/src/ws.rs`, `addons/web/tests/web.rs`, `crates/tui/tests/common/mod.rs`
Goal: spec §4 Web, server side (AC6), and Goals 1-2 end to end, hermetically.
- `hosts.forward_agent` writes through `set_forward_agent`. When the file changed, a connected
  host is reconnected through the hub's `Cmd::Reconnect` path (decision 5) and any other known
  host gets a `host` event. `local` is refused.
- `hosts.list` and every `host` event carry `forward_agent` (decision 6).
- `FakeSsh` (decision 10) is the shared fake remote. Task 7 and the hermetic test here use it.

Step 1: write the failing tests: `t5-step1.diff`.
- `FakeSsh` and its fake agent, in the shared harness.
- In `web.rs`: `web_forward_agent_writes_the_option_and_connects_nothing`,
  `web_forward_agent_reconnects_a_connected_host_and_keeps_its_sessions` (AC6) and
  `web_forwarded_agent_reaches_terminals_through_the_relay`.
````diff
diff --git a/addons/web/tests/web.rs b/addons/web/tests/web.rs
index 505d244..8379ba9 100644
--- a/addons/web/tests/web.rs
+++ b/addons/web/tests/web.rs
@@ -11,7 +11,7 @@ use std::os::unix::fs::PermissionsExt;
 use std::process::{Command, Stdio};
 use std::{fs, thread};
 
-use common::{ManagerHarness, BINARY, WAIT};
+use common::{FakeSsh, ManagerHarness, BINARY, DAEMON_WAIT, WAIT};
 use futures::SinkExt;
 use serde_json::{json, Value};
 use tokio_tungstenite::tungstenite::Message;
@@ -1685,6 +1685,191 @@ async fn web_hosts_report_daemon_features() {
     assert_eq!(local["claude"]["version"], "claude 0.0.0-fake");
 }
 
+/// A web server whose ssh is `fake`'s, logged in, `local` connected.
+async fn web_with_fake_ssh(fake: &FakeSsh) -> (Web, Tab) {
+    let h = ManagerHarness::new();
+    write_web_fake_claude(&h, false);
+    let path = fake.path();
+    let web = Web::start_with(h, &[], move |cmd| {
+        cmd.env("PATH", &path);
+    });
+    let cookie = web.login();
+    let mut tab = Tab::open(&web, &cookie).await;
+    tab.connected("local").await;
+    (web, tab)
+}
+
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn web_forward_agent_writes_the_option_and_connects_nothing() {
+    let fake = FakeSsh::new();
+    let (web, mut tab) = web_with_fake_ssh(&fake).await;
+    let set = |host: &str, on: bool| json!({"host": host, "on": on});
+    assert_eq!(
+        tab.req("hosts.forward_agent", set("local", true)).await,
+        Err("local has no ssh connection".to_owned())
+    );
+    let bad = tab
+        .req("hosts.forward_agent", set("-oProxyCommand=x", true))
+        .await;
+    assert!(bad.unwrap_err().contains("must not start with '-'"));
+    let missing = tab
+        .req("hosts.forward_agent", json!({"host": "nowhere"}))
+        .await;
+    assert!(missing.unwrap_err().starts_with("bad request:"));
+
+    // A host this server does not know: written, and nothing connects.
+    let reply = tab.req("hosts.forward_agent", set("nowhere", true)).await;
+    assert_eq!(reply, Ok(json!({})));
+    let config = web.h().config_home.join("claudio/config.toml");
+    let text = fs::read_to_string(&config).unwrap();
+    assert!(
+        text.contains("[ssh]\nforward_agent = [\"nowhere\"]"),
+        "{text}"
+    );
+    assert!(text.starts_with("[claude]\n"), "the rest stays: {text}");
+    let hosts = tab.req("hosts.list", json!({})).await.unwrap();
+    let names: Vec<&Value> = hosts["hosts"]
+        .as_array()
+        .unwrap()
+        .iter()
+        .map(|h| &h["name"])
+        .collect();
+    assert_eq!(names, [&json!("local")]);
+    assert_eq!(hosts["hosts"][0]["forward_agent"], false, "never for local");
+    tab.req("hosts.forward_agent", set("nowhere", false))
+        .await
+        .unwrap();
+    let text = fs::read_to_string(&config).unwrap();
+    assert!(text.contains("forward_agent = []"), "{text}");
+    assert!(fake.calls().is_empty(), "no ssh ran: {:?}", fake.calls());
+}
+
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn web_forward_agent_reconnects_a_connected_host_and_keeps_its_sessions() {
+    let fake = FakeSsh::new();
+    let (web, mut tab) = web_with_fake_ssh(&fake).await;
+    let dir = web.h().dirs[0].clone();
+    let connect = tokio::time::timeout(
+        DAEMON_WAIT,
+        tab.req("hosts.connect", json!({"host": "fakehost"})),
+    );
+    let host = connect
+        .await
+        .expect("connected in time")
+        .expect("hosts.connect");
+    assert_eq!(host["host"]["forward_agent"], false);
+    let shell = json!({"host": "fakehost", "cwd": dir, "rows": 24, "cols": 80});
+    let sid = tab.req("session.shell", shell).await.expect("shell")["sid"]
+        .as_str()
+        .unwrap()
+        .to_owned();
+    let before = tab
+        .event("session", |v| {
+            v["session"]["id"] == sid.as_str() && v["session"]["pid"].is_u64()
+        })
+        .await;
+    let bridges = fake.bridges();
+    assert_eq!(bridges.len(), 1, "{bridges:?}");
+    assert!(!bridges[0].contains("ForwardAgent"), "{bridges:?}");
+
+    // From here on, only what the toggle causes.
+    tab.pending.clear();
+    let on = json!({"host": "fakehost", "on": true});
+    assert_eq!(
+        tab.req("hosts.forward_agent", on.clone()).await,
+        Ok(json!({}))
+    );
+    let fwd = |v: &Value| v["host"]["name"] == "fakehost" && v["host"]["forward_agent"] == true;
+    tab.event("host", |v| fwd(v) && v["host"]["connected"] == false)
+        .await;
+    tab.event("host", |v| fwd(v) && v["host"]["connected"] == true)
+        .await;
+    let bridges = fake.wait_bridges(2, WAIT);
+    assert!(bridges[1].contains("-o ForwardAgent=yes --"), "{bridges:?}");
+    // The terminal kept running: back from `unknown` with the same process.
+    let state =
+        |v: &Value, want: &str| v["session"]["id"] == sid.as_str() && v["session"]["state"] == want;
+    tab.event("session", |v| state(v, "unknown")).await;
+    let after = tab.event("session", |v| state(v, "idle")).await;
+    assert_eq!(after["session"]["pid"], before["session"]["pid"]);
+    // Unchanged: no reconnect.
+    assert_eq!(tab.req("hosts.forward_agent", on).await, Ok(json!({})));
+    let listed = tab.req("hosts.list", json!({})).await.unwrap();
+    let fakehost = listed["hosts"]
+        .as_array()
+        .unwrap()
+        .iter()
+        .find(|h| h["name"] == "fakehost")
+        .cloned();
+    assert_eq!(fakehost.unwrap()["forward_agent"], true);
+    assert_eq!(fake.bridges().len(), 2);
+}
+
+/// What `ssh-add -l` says in terminal `sid`, its output after this run's
+/// command line only (`step` makes the line and its marker unique).
+async fn ssh_add_in(tab: &mut Tab, sid: &str, step: u32) -> String {
+    let line = format!("ssh-add -l; echo step{step}-$((40+2))\r");
+    tab.input(sid, line.as_bytes()).await;
+    let out = tab.output_until(sid, &format!("step{step}-42")).await;
+    let typed = format!("step{step}-$((40+2))");
+    out[out.rfind(&typed).unwrap_or(0)..].to_owned()
+}
+
+/// A forwarded agent reaches a remote terminal through the bridge's link and
+/// the daemon's relay; a terminal started while one was forwarded loses it
+/// while no connection forwards one and gets it back on the next that does,
+/// without a restart (hermetic: a fake ssh and a fake agent with no keys).
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn web_forwarded_agent_reaches_terminals_through_the_relay() {
+    // Never the owner's agent: only whether ssh-add runs at all.
+    let mut probe = Command::new("ssh-add");
+    probe.arg("-l").env_remove("SSH_AUTH_SOCK");
+    if probe.output().is_err() {
+        println!("SKIP web_forwarded_agent_reaches_terminals_through_the_relay: no ssh-add");
+        return;
+    }
+    let fake = FakeSsh::with_agent();
+    let (web, mut tab) = web_with_fake_ssh(&fake).await;
+    let dir = web.h().dirs[0].clone();
+    let set = |on: bool| json!({"host": "fakehost", "on": on});
+    tab.req("hosts.forward_agent", set(true)).await.unwrap();
+    let connect = tab.req("hosts.connect", json!({"host": "fakehost"}));
+    let connect = tokio::time::timeout(DAEMON_WAIT, connect).await;
+    connect.expect("connected in time").expect("hosts.connect");
+    assert!(fake.bridges()[0].contains("ForwardAgent=yes"));
+    let shell = json!({"host": "fakehost", "cwd": dir, "rows": 24, "cols": 200});
+    let sid = tab.req("session.shell", shell).await.expect("shell")["sid"]
+        .as_str()
+        .unwrap()
+        .to_owned();
+    let attach = json!({"sid": sid, "rows": 24, "cols": 200});
+    tab.req("session.attach", attach).await.expect("attach");
+    let none = "The agent has no identities";
+    assert!(ssh_add_in(&mut tab, &sid, 1).await.contains(none));
+
+    // No connection forwards an agent now: the terminal cannot reach one.
+    reconnect_with(&mut tab, &fake, &sid, false, 2).await;
+    assert!(!ssh_add_in(&mut tab, &sid, 2).await.contains(none));
+
+    // A forwarding connection again: the same terminal reaches the agent.
+    reconnect_with(&mut tab, &fake, &sid, true, 3).await;
+    assert!(ssh_add_in(&mut tab, &sid, 3).await.contains(none));
+}
+
+/// Switch fakehost's forwarding to `on` and wait for its `n`th bridge and for
+/// terminal `sid` to be attached again.
+async fn reconnect_with(tab: &mut Tab, fake: &FakeSsh, sid: &str, on: bool, n: usize) {
+    tab.pending.clear();
+    let set = json!({"host": "fakehost", "on": on});
+    tab.req("hosts.forward_agent", set).await.unwrap();
+    fake.wait_bridges(n, WAIT);
+    tab.event("host", |v| {
+        v["host"]["name"] == "fakehost" && v["host"]["connected"] == true
+    })
+    .await;
+    tab.event("attached", |v| v["sid"] == sid).await;
+}
+
 #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
 async fn web_order_and_layout_persist_in_web_state() {
     let h = ManagerHarness::new();
diff --git a/crates/tui/tests/common/mod.rs b/crates/tui/tests/common/mod.rs
index d7ee68c..8d1defd 100644
--- a/crates/tui/tests/common/mod.rs
+++ b/crates/tui/tests/common/mod.rs
@@ -985,6 +985,177 @@ impl Drop for ManagerHarness {
     }
 }
 
+// ── FakeSsh ───────────────────────────────────────────────────────────────────
+
+/// A fake `ssh` for hermetic remote-host tests (put [`FakeSsh::path`] on the
+/// client's `PATH`). It logs its argv, then runs the remote command with
+/// `sh -c` on this machine as a "remote" of its own: its own `HOME` (whose
+/// `~/.local/bin/claudio` execs this build, so bootstrap uploads nothing),
+/// config (the test config) and runtime dir (`TMPDIR`; without
+/// `XDG_RUNTIME_DIR` the bridge starts the remote daemon with setsid, never
+/// `systemd-run`). The owner's `SSH_AUTH_SOCK` never reaches it: like ssh,
+/// it forwards an agent only when given `-o ForwardAgent=yes`, and then only
+/// the fake one of [`FakeSsh::with_agent`]. Every host name reaches the same
+/// remote. Dropping it stops the remote daemon and removes its dirs.
+pub struct FakeSsh {
+    pub root: PathBuf,
+    bin: PathBuf,
+    log: PathBuf,
+}
+
+impl FakeSsh {
+    /// A fake ssh that forwards no agent.
+    pub fn new() -> FakeSsh {
+        FakeSsh::make(false)
+    }
+
+    /// A fake ssh that forwards a fake agent (`<root>/agent.sock`, which
+    /// holds no key) when asked to.
+    pub fn with_agent() -> FakeSsh {
+        FakeSsh::make(true)
+    }
+
+    fn make(agent: bool) -> FakeSsh {
+        let root = short_tmp_root();
+        private_dir(&root);
+        let agent = match agent {
+            true => {
+                let sock = root.join("agent.sock");
+                fake_ssh_agent(&sock);
+                sock.display().to_string()
+            }
+            false => String::new(),
+        };
+        let (bin, home, cfg, tmp) = (
+            root.join("bin"),
+            root.join("home"),
+            root.join("cfg"),
+            root.join("t"),
+        );
+        for d in [&bin, &home.join(".local/bin"), &tmp] {
+            fs::create_dir_all(d).expect("create fake remote dir");
+        }
+        write_test_config(&cfg);
+        let claudio = home.join(".local/bin/claudio");
+        fs::write(&claudio, format!("#!/bin/sh\nexec '{BINARY}' \"$@\"\n")).expect("write claudio");
+        let log = root.join("ssh.log");
+        let ssh = format!(
+            r#"#!/bin/sh
+printf '%s\n' "$*" >> '{log}'
+agent=
+while [ $# -gt 0 ]; do
+  case "$1" in
+    --) shift; break ;;
+    -o) [ "$2" = ForwardAgent=yes ] && [ -n '{agent}' ] && agent='{root}/fwd-'$$; shift 2 ;;
+    *) shift ;;
+  esac
+done
+shift
+# Like sshd: this connection's own agent path, gone when the connection ends.
+[ -n "$agent" ] && ln -s '{agent}' "$agent"
+env -u SSH_AUTH_SOCK -u XDG_RUNTIME_DIR ${{agent:+SSH_AUTH_SOCK=$agent}} HOME='{home}' XDG_CONFIG_HOME='{cfg}' TMPDIR='{tmp}' /bin/sh -c "$*"
+status=$?
+[ -n "$agent" ] && rm -f "$agent"
+exit $status
+"#,
+            log = log.display(),
+            root = root.display(),
+            home = home.display(),
+            cfg = cfg.display(),
+            tmp = tmp.display(),
+        );
+        fs::write(bin.join("ssh"), ssh).expect("write fake ssh");
+        for f in [bin.join("ssh"), claudio] {
+            fs::set_permissions(&f, fs::Permissions::from_mode(0o755)).expect("chmod");
+        }
+        FakeSsh { root, bin, log }
+    }
+
+    /// `PATH` with the fake ssh first.
+    pub fn path(&self) -> String {
+        let rest = std::env::var("PATH").unwrap_or_default();
+        format!("{}:{rest}", self.bin.display())
+    }
+
+    /// The logged command lines (ssh's argv) that ran a bridge, oldest first.
+    pub fn bridges(&self) -> Vec<String> {
+        fs::read_to_string(&self.log)
+            .unwrap_or_default()
+            .lines()
+            .filter(|l| l.ends_with("--slave"))
+            .map(str::to_owned)
+            .collect()
+    }
+
+    /// Every logged ssh command line, oldest first.
+    pub fn calls(&self) -> Vec<String> {
+        let log = fs::read_to_string(&self.log).unwrap_or_default();
+        log.lines().map(str::to_owned).collect()
+    }
+
+    /// Wait until `n` bridges ran; their command lines.
+    pub fn wait_bridges(&self, n: usize, timeout: Duration) -> Vec<String> {
+        let deadline = Instant::now() + timeout;
+        loop {
+            let bridges = self.bridges();
+            if bridges.len() >= n {
+                return bridges;
+            }
+            assert!(
+                Instant::now() < deadline,
+                "{n} bridges never ran: {bridges:?}"
+            );
+            thread::sleep(Duration::from_millis(50));
+        }
+    }
+
+    /// The remote daemon's lock file (it holds its pid).
+    fn lock(&self) -> PathBuf {
+        // SAFETY: getuid has no preconditions.
+        let uid = unsafe { libc::getuid() };
+        self.root.join(format!("t/claudio-{uid}/daemon-v1.lock"))
+    }
+}
+
+impl Drop for FakeSsh {
+    fn drop(&mut self) {
+        let pid = fs::read_to_string(self.lock()).ok();
+        if let Some(pid) = pid.and_then(|s| s.trim().parse::<libc::pid_t>().ok()) {
+            // SAFETY: kill(2) has no preconditions.
+            unsafe { libc::kill(pid, libc::SIGTERM) };
+        }
+        let _ = fs::remove_dir_all(&self.root);
+    }
+}
+
+/// A fake ssh agent at `path`, for the rest of the test run: it answers an
+/// identities request with an empty list and anything else with a failure.
+fn fake_ssh_agent(path: &Path) {
+    let listener = std::os::unix::net::UnixListener::bind(path).expect("bind fake agent");
+    thread::spawn(move || {
+        for stream in listener.incoming() {
+            let Ok(mut s) = stream else { break };
+            thread::spawn(move || {
+                let mut len = [0u8; 4];
+                while s.read_exact(&mut len).is_ok() {
+                    let mut body = vec![0u8; (u32::from_be_bytes(len) as usize).min(1 << 16)];
+                    if s.read_exact(&mut body).is_err() {
+                        break;
+                    }
+                    // SSH_AGENT_IDENTITIES_ANSWER (no keys), else SSH_AGENT_FAILURE.
+                    let answer: &[u8] = match body.first() {
+                        Some(11) => &[0, 0, 0, 5, 12, 0, 0, 0, 0],
+                        _ => &[0, 0, 0, 1, 5],
+                    };
+                    if s.write_all(answer).is_err() {
+                        break;
+                    }
+                }
+            });
+        }
+    });
+}
+
 /// One `J` frame: `[u32 BE length][b'J'][JSON]`, the length counting the tag.
 fn write_control(s: &mut impl Write, msg: &serde_json::Value) {
     let body = serde_json::to_vec(msg).expect("encode");
````
Run: `cargo test --locked --test web web_forward -- --test-threads=1`
Expected: `test result: FAILED. 0 passed; 3 failed`, with `left: Err("unknown op")` (and
``called `Result::unwrap()` on an `Err` value: "unknown op"``).

Step 2: implement: `t5-step2.diff`.
````diff
diff --git a/addons/web/src/dto.rs b/addons/web/src/dto.rs
index 1f60bc2..3b8b904 100644
--- a/addons/web/src/dto.rs
+++ b/addons/web/src/dto.rs
@@ -50,6 +50,7 @@ pub(crate) const HOST_FIELDS: &[&str] = &[
     "claude",
     "features",
     "version",
+    "forward_agent",
 ];
 
 fn kind_name(kind: SessionKind) -> &'static str {
@@ -159,6 +160,7 @@ pub(crate) fn host(state: &HostState) -> Value {
         "claude": claude,
         "features": state.features,
         "version": welcome.map(|w| w.claudio_version.as_str()),
+        "forward_agent": super::hosts::forwards_agent(&state.name),
     })
 }
 
diff --git a/addons/web/src/ext.rs b/addons/web/src/ext.rs
index e2ef0ef..499bba5 100644
--- a/addons/web/src/ext.rs
+++ b/addons/web/src/ext.rs
@@ -252,9 +252,11 @@ impl Extensions {
     pub(crate) async fn reread(&self, hub: &HubHandle, rows: &[(&'static str, ExtensionRow)]) {
         let toggles = self.toggles.load(Ordering::Acquire);
         let path = self.config_path.clone();
-        let Ok(config) =
-            tokio::task::spawn_blocking(move || claudio_core::config::load_from(&path)).await
-        else {
+        let read = move || {
+            super::hosts::reload_forward(&path);
+            claudio_core::config::load_from(&path)
+        };
+        let Ok(config) = tokio::task::spawn_blocking(read).await else {
             return;
         };
         tracing::debug!("web: config.toml re-read");
diff --git a/addons/web/src/hosts.rs b/addons/web/src/hosts.rs
new file mode 100644
index 0000000..f83fc92
--- /dev/null
+++ b/addons/web/src/hosts.rs
@@ -0,0 +1,108 @@
+//! The `hosts.*` ops (docs/web-protocol.md §5, "Hosts"), the host names the
+//! web takes, and `[ssh] forward_agent` as this server last read it (the
+//! Host DTO's `forward_agent`).
+
+use std::path::Path;
+use std::sync::{Mutex, MutexGuard};
+
+use serde::Deserialize;
+use serde_json::{json, Value};
+
+use super::hub::{clean, HubHandle};
+use super::ws::{parse, HostArgs, NoArgs};
+use claudio_core::remote::hosts as ssh_hosts;
+
+#[derive(Deserialize)]
+#[serde(deny_unknown_fields)]
+struct ForwardArgs {
+    host: String,
+    on: bool,
+}
+
+/// One `hosts.*` op.
+pub(super) async fn dispatch(hub: &HubHandle, op: &str, args: Value) -> Result<Value, String> {
+    match op {
+        "hosts.list" => {
+            parse::<NoArgs>(args)?;
+            Ok(json!({ "hosts": hub.hosts().await }))
+        }
+        "hosts.candidates" => {
+            parse::<NoArgs>(args)?;
+            let connected: Vec<String> = hub
+                .hosts()
+                .await
+                .into_iter()
+                .filter(|h| h["connected"] == true)
+                .filter_map(|h| h["name"].as_str().map(str::to_owned))
+                .collect();
+            let all = tokio::task::spawn_blocking(ssh_hosts::candidates)
+                .await
+                .unwrap_or_default();
+            let hosts: Vec<String> = all.into_iter().filter(|h| !connected.contains(h)).collect();
+            Ok(json!({ "hosts": hosts }))
+        }
+        "hosts.connect" => {
+            let a: HostArgs = parse(args)?;
+            hub.connect(a.host).await
+        }
+        "hosts.disconnect" => {
+            let a: HostArgs = parse(args)?;
+            hub.disconnect(a.host).await.map(|()| json!({}))
+        }
+        "hosts.forward_agent" => forward_agent(hub, parse(args)?).await,
+        _ => Err("unknown op".to_owned()),
+    }
+}
+
+/// `hosts.forward_agent`: `[ssh] forward_agent` through the core's one
+/// writer. When the file changed, a connected host is reconnected (its
+/// sessions keep running) and any other known one gets a `host` event, after
+/// the DTO's value is read again.
+async fn forward_agent(hub: &HubHandle, a: ForwardArgs) -> Result<Value, String> {
+    if a.host == "local" {
+        return Err("local has no ssh connection".to_owned());
+    }
+    host_ok(&a.host)?;
+    let (host, on) = (a.host.clone(), a.on);
+    let saved = tokio::task::spawn_blocking(move || {
+        let changed = ssh_hosts::set_forward_agent(&host, on);
+        reload_forward(&claudio_core::paths::config_file());
+        changed
+    })
+    .await
+    .map_err(|_| "could not save: the save stopped".to_owned())?;
+    if saved.map_err(|e| format!("could not save: {e}"))? {
+        hub.forward_agent_changed(a.host).await;
+    }
+    Ok(json!({}))
+}
+
+/// A host name the web may connect to: what `remote::validate_host` takes,
+/// at most 255 bytes, no whitespace or control characters.
+pub(crate) fn host_ok(host: &str) -> Result<(), String> {
+    claudio_core::remote::validate_host(host)?;
+    if host.len() > 255 || host.chars().any(|c| c.is_whitespace() || c.is_control()) {
+        return Err(format!("invalid host {:?}", clean(host, 80)));
+    }
+    Ok(())
+}
+
+/// `[ssh] forward_agent` as last read: at start, after `hosts.forward_agent`
+/// and whenever config.toml is re-read.
+static FORWARD: Mutex<Vec<String>> = Mutex::new(Vec::new());
+
+fn forward() -> MutexGuard<'static, Vec<String>> {
+    FORWARD.lock().unwrap_or_else(|e| e.into_inner())
+}
+
+/// Read `[ssh] forward_agent` of the config file at `path` again (blocking:
+/// from `spawn_blocking`).
+pub(crate) fn reload_forward(path: &Path) {
+    let list = ssh_hosts::forward_agent_in(path);
+    *forward() = list;
+}
+
+/// The Host DTO's `forward_agent`: never for `local`.
+pub(crate) fn forwards_agent(host: &str) -> bool {
+    host != "local" && forward().iter().any(|h| h == host)
+}
diff --git a/addons/web/src/hub.rs b/addons/web/src/hub.rs
index 7396cca..a2e4cbd 100644
--- a/addons/web/src/hub.rs
+++ b/addons/web/src/hub.rs
@@ -509,10 +509,10 @@ enum Cmd {
         host: String,
         generation: u64,
     },
-    /// An outdated daemon is idle now: reconnect (which replaces it).
+    /// Reconnect a connected host: outdated and idle now, or `hosts.forward_agent` (`None`).
     Reconnect {
         host: String,
-        generation: u64,
+        generation: Option<u64>,
     },
     ClaudeDone {
         host: String,
@@ -562,7 +562,7 @@ pub(crate) fn start(cfg: HubConfig) -> HubHandle {
     let remembered = hub.web.hosts.iter().cloned();
     let tui_hosts = hub.tui_state.sessions.iter().map(|s| s.host.clone());
     for host in remembered.chain(tui_hosts) {
-        if host_ok(&host).is_ok() && host != "local" && !hosts.contains(&host) {
+        if super::hosts::host_ok(&host).is_ok() && host != "local" && !hosts.contains(&host) {
             hosts.push(host);
         }
     }
@@ -578,16 +578,6 @@ pub(crate) fn start(cfg: HubConfig) -> HubHandle {
     HubHandle { tx, events }
 }
 
-/// A host name the web may connect to: what `remote::validate_host` takes,
-/// at most 255 bytes, no whitespace or control characters.
-fn host_ok(host: &str) -> Result<(), String> {
-    claudio_core::remote::validate_host(host)?;
-    if host.len() > 255 || host.chars().any(|c| c.is_whitespace() || c.is_control()) {
-        return Err(format!("invalid host {:?}", clean(host, 80)));
-    }
-    Ok(())
-}
-
 impl HubHandle {
     async fn ask<T>(&self, make: impl FnOnce(Reply<T>) -> Cmd) -> Result<T, String> {
         let (reply, rx) = oneshot::channel();
@@ -698,10 +688,16 @@ impl HubHandle {
         if host == "local" {
             return Err("local is always connected".into());
         }
-        host_ok(&host)?;
+        super::hosts::host_ok(&host)?;
         self.ask(|reply| Cmd::Connect { host, reply }).await?
     }
 
+    /// `hosts.forward_agent` changed `host`'s option: reconnect it if connected.
+    pub(crate) async fn forward_agent_changed(&self, host: String) {
+        let generation = None;
+        let _ = self.tx.send(Cmd::Reconnect { host, generation }).await;
+    }
+
     pub(crate) async fn disconnect(&self, host: String) -> Result<(), String> {
         if host == "local" {
             return Err("cannot disconnect local".into());
@@ -837,7 +833,7 @@ fn now_secs() -> u64 {
 }
 
 /// `text` sanitized and cut at `max` characters.
-fn clean(text: &str, max: usize) -> String {
+pub(crate) fn clean(text: &str, max: usize) -> String {
     claudio_core::text::sanitize(text)
         .chars()
         .take(max)
@@ -1033,14 +1029,7 @@ impl Hub {
                     self.start_attempt(&host, generation, bootstrap);
                 }
             }
-            Cmd::Reconnect { host, generation } => {
-                let connected = self.hosts.get(&host).is_some_and(|h| h.client.is_some());
-                if connected && self.current(&host, generation) {
-                    self.lost(&host);
-                    let generation = self.conns.next_generation(&host);
-                    self.start_attempt(&host, generation, false);
-                }
-            }
+            Cmd::Reconnect { host, generation } => self.reconnect(&host, generation),
             Cmd::ClaudeDone {
                 host,
                 install,
@@ -1049,6 +1038,20 @@ impl Hub {
         }
     }
 
+    /// Drop a connected `host`'s connection and connect again (its sessions
+    /// stay, `unknown` until it is back); a `generation` must still be its
+    /// connection's. Without one, a host left alone gets its `host` event.
+    fn reconnect(&mut self, host: &str, generation: Option<u64>) {
+        let connected = self.hosts.get(host).is_some_and(|h| h.client.is_some());
+        if connected && generation.is_none_or(|g| self.current(host, g)) {
+            self.lost(host);
+            let generation = self.conns.next_generation(host);
+            self.start_attempt(host, generation, false);
+        } else if generation.is_none() {
+            self.send_host(host);
+        }
+    }
+
     /// Whether a message of `generation` from `host`'s connection is current.
     fn current(&self, host: &str, generation: u64) -> bool {
         self.hosts.contains_key(host) && self.conns.current_generation(host) == generation
@@ -1810,7 +1813,7 @@ impl Hub {
             };
             let (host, generation, tx) = (
                 h.name.clone(),
-                self.conns.current_generation(&h.name),
+                Some(self.conns.current_generation(&h.name)),
                 self.tx.clone(),
             );
             tokio::spawn(async move {
diff --git a/addons/web/src/lib.rs b/addons/web/src/lib.rs
index 3b5ab16..a816ec8 100644
--- a/addons/web/src/lib.rs
+++ b/addons/web/src/lib.rs
@@ -13,6 +13,7 @@ mod dto;
 mod events;
 mod ext;
 mod git_ops;
+mod hosts;
 mod hub;
 mod hub_io;
 mod input;
@@ -603,14 +604,17 @@ async fn build_state(
     let state_path = state::path();
     let hub_cfg = {
         let (state_path, runtime) = (state_path.clone(), runtime.clone());
-        tokio::task::spawn_blocking(move || hub::HubConfig {
-            config: claudio_core::config::load(),
-            tui_state: claudio_core::app::state::ClientState::load(
-                &claudio_core::paths::client_state(),
-            ),
-            web: state::load(&state_path),
-            state_path,
-            runtime,
+        tokio::task::spawn_blocking(move || {
+            hosts::reload_forward(&claudio_core::paths::config_file());
+            hub::HubConfig {
+                config: claudio_core::config::load(),
+                tui_state: claudio_core::app::state::ClientState::load(
+                    &claudio_core::paths::client_state(),
+                ),
+                web: state::load(&state_path),
+                state_path,
+                runtime,
+            }
         })
         .await
         .map_err(|e| format!("cannot read the configuration: {e}"))?
diff --git a/addons/web/src/ws.rs b/addons/web/src/ws.rs
index 5afee35..b263664 100644
--- a/addons/web/src/ws.rs
+++ b/addons/web/src/ws.rs
@@ -23,7 +23,7 @@ use super::input::Decision;
 use super::spawn::{self, check_size, RespawnReq, SpawnReq};
 use super::state::Layout;
 use super::AppState;
-use super::{addons, auth, ext, git_ops, sessions};
+use super::{addons, auth, ext, git_ops, hosts, sessions};
 use claudio_core::paths;
 use claudio_core::proto::{Msg, SearchHit, SessionId};
 
@@ -277,12 +277,12 @@ pub(super) fn parse<T: DeserializeOwned>(args: Value) -> Result<T, String> {
 
 #[derive(Deserialize)]
 #[serde(deny_unknown_fields)]
-struct NoArgs {}
+pub(super) struct NoArgs {}
 
 #[derive(Deserialize)]
 #[serde(deny_unknown_fields)]
-struct HostArgs {
-    host: String,
+pub(super) struct HostArgs {
+    pub(super) host: String,
 }
 
 #[derive(Deserialize)]
@@ -439,33 +439,7 @@ pub(crate) async fn dispatch(
 ) -> Result<Value, String> {
     let hub = &state.hub;
     match op {
-        "hosts.list" => {
-            parse::<NoArgs>(args)?;
-            Ok(json!({ "hosts": hub.hosts().await }))
-        }
-        "hosts.candidates" => {
-            parse::<NoArgs>(args)?;
-            let connected: Vec<String> = hub
-                .hosts()
-                .await
-                .into_iter()
-                .filter(|h| h["connected"] == true)
-                .filter_map(|h| h["name"].as_str().map(str::to_owned))
-                .collect();
-            let all = tokio::task::spawn_blocking(claudio_core::remote::hosts::candidates)
-                .await
-                .unwrap_or_default();
-            let hosts: Vec<String> = all.into_iter().filter(|h| !connected.contains(h)).collect();
-            Ok(json!({ "hosts": hosts }))
-        }
-        "hosts.connect" => {
-            let a: HostArgs = parse(args)?;
-            hub.connect(a.host).await
-        }
-        "hosts.disconnect" => {
-            let a: HostArgs = parse(args)?;
-            hub.disconnect(a.host).await.map(|()| json!({}))
-        }
+        op if op.starts_with("hosts.") => hosts::dispatch(hub, op, args).await,
         "session.attach" => {
             let a: SizeArgs = parse(args)?;
             check_size(a.rows, a.cols)?;
````
Run: `cargo test --locked --test web web_forward -- --test-threads=1`, then
`cargo test --locked -p claudio-web --lib`
Expected: `test result: ok. 3 passed; 0 failed` (about 5 s), then `test result: ok. 119 passed`.
No `claudio --daemon` of the fake remote is left running: `FakeSsh` stops it on drop.
`pgrep -f 'claudio --daemon'` shows only daemons that were there before.

Step 3: `make check`, then the web suites:
`cargo test --locked --test web --test web_manager --test vocdoni_web -- --test-threads=1` →
`web` 55, `web_manager` 4, `vocdoni_web` 13, all passing. A8 numbers:
- `hub.rs` 2061 → 2064 (baseline 2065);
- `hub.rs::handle` 181 → 174;
- `ws.rs::dispatch` 234 → 208;
- `ws.rs::tab` 102 (untouched);
- `addons/web/src/hosts.rs` 109.

Then commit:
`git add addons/web/src/hosts.rs`
`git commit -m "feat(web): hosts.forward_agent, the host dto's forward_agent, and a fake ssh for the suites" -- addons/web/src/hosts.rs addons/web/src/dto.rs addons/web/src/ext.rs addons/web/src/hub.rs addons/web/src/lib.rs addons/web/src/ws.rs addons/web/tests/web.rs crates/tui/tests/common/mod.rs`

Interfaces this task provides to later tasks:
- `hosts.forward_agent` and Host `forward_agent` for the page;
- `common::FakeSsh { new(), with_agent(), path(), bridges(), calls(), wait_bridges(n, timeout), root }`.

### Task 6: The page: the host panel row, the add form and the wizard's box
Model: sonnet   Repo: /home/p4u/claudio   Files: `addons/web/web/app/ui/hosts.js`,
`addons/web/web/app/ui/wizard.js`, `addons/web/web/e2e/tests/agent.spec.mjs` (new)
Goal: spec §4 Web, page side (AC7).
- A remote host's panel has a row, "SSH agent": "forward on connect" or "not forwarded (your
  ssh config decides)", with a toggle button (`host-agent-btn`).
- The add form (`host-add-agent`) and the wizard's "Another host…" (`wizard-host-agent`) get a
  "Forward my SSH agent" box, initially clear. `hosts.forward_agent {host, on: <checked>}` is
  sent before `hosts.connect`, checked or not.
- The help text says what forwarding allows.

Step 1: write the failing e2e: `t6-step1.diff`.
````diff
diff --git a/addons/web/web/e2e/tests/agent.spec.mjs b/addons/web/web/e2e/tests/agent.spec.mjs
new file mode 100644
index 0000000..f86502a
--- /dev/null
+++ b/addons/web/web/e2e/tests/agent.spec.mjs
@@ -0,0 +1,55 @@
+import { test, expect } from '@playwright/test';
+import fs from 'node:fs';
+import { startServer } from '../fixtures/server.mjs';
+
+// A remembered remote host (offline: the ssh to it just fails) has a host panel.
+let srv;
+test.beforeAll(async () => { srv = await startServer({ hosts: ['z6'] }); });
+test.afterAll(async () => { await srv?.stop(); });
+
+test('the host panel switches ssh agent forwarding for a remote host', async ({ page }) => {
+  await page.goto(srv.url);
+  await page.getByTestId('hosts-btn').click();
+  await page.getByTestId('hosts-menu').locator('[data-testid="host-chip"][data-host="z6"]').click();
+  await expect(page.getByTestId('host-panel-facts')).toHaveAttribute('data-host', 'z6');
+  const row = page.getByTestId('host-agent');
+  const btn = page.getByTestId('host-agent-btn');
+  await expect(row).toHaveText('not forwarded (your ssh config decides)');
+  await btn.click();
+  await expect(row).toHaveText('forward on connect');
+  expect(fs.readFileSync(srv.configToml, 'utf8')).toContain('forward_agent = ["z6"]');
+  await btn.click();
+  await expect(row).toHaveText('not forwarded (your ssh config decides)');
+  expect(fs.readFileSync(srv.configToml, 'utf8')).toContain('forward_agent = []');
+  // local has no ssh connection: no row.
+  await page.getByRole('tab', { name: 'local' }).click();
+  await expect(page.getByTestId('host-panel-facts')).toHaveAttribute('data-host', 'local');
+  await expect(page.getByTestId('host-agent')).toHaveCount(0);
+  expect(await page.evaluate(() => window.__claudioCsp ?? [])).toEqual([]);
+});
+
+test('the wizard sends the agent choice before it connects a typed host', async ({ page }) => {
+  const sent = [];
+  page.on('websocket', (ws) => ws.on('framesent', (f) => {
+    if (typeof f.payload !== 'string') return;
+    try { sent.push(JSON.parse(f.payload)); } catch { /* not JSON */ }
+  }));
+  await page.goto(srv.url);
+  await expect(page.getByTestId('rail')).toBeVisible();
+  await page.keyboard.press('Alt+n');
+  await expect(page.getByTestId('wizard')).toBeVisible();
+  await page.getByTestId('wizard-host').selectOption('other:');
+  await page.getByTestId('wizard-host-input').fill('nonexistent.invalid');
+  const agent = page.getByTestId('wizard-host-agent');
+  await expect(agent).not.toBeChecked();
+  await agent.check();
+  await page.getByTestId('wizard-host-connect').click();
+  await expect(page.getByTestId('wizard-host-error')).toContainText(/resolve|ssh failed/i, { timeout: 60_000 });
+  const ops = sent.filter((m) => m.t === 'req').map((m) => m.op);
+  const fwd = ops.indexOf('hosts.forward_agent');
+  expect(fwd).toBeGreaterThanOrEqual(0);
+  expect(fwd).toBeLessThan(ops.indexOf('hosts.connect'));
+  expect(sent.find((m) => m.op === 'hosts.forward_agent')).toMatchObject({ host: 'nonexistent.invalid', on: true });
+  expect(fs.readFileSync(srv.configToml, 'utf8')).toContain('"nonexistent.invalid"');
+  expect(await page.evaluate(() => window.__claudioCsp ?? [])).toEqual([]);
+});
````
Run: `cargo build --locked && cd addons/web/web/e2e && PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 npm ci --no-audit --no-fund && npx playwright test tests/agent.spec.mjs`
Expected: `1) tests/agent.spec.mjs:10:1 › the host panel switches ssh agent forwarding for a remote host`,
with `Error: expect(locator).toHaveText(expected) failed` and `getByTestId('host-agent')`:
`element(s) not found`.

Step 2: implement: `t6-step2.diff`.
````diff
diff --git a/addons/web/web/app/ui/hosts.js b/addons/web/web/app/ui/hosts.js
index fb2bf1b..7799cf3 100644
--- a/addons/web/web/app/ui/hosts.js
+++ b/addons/web/web/app/ui/hosts.js
@@ -37,6 +37,20 @@ function Fact({ k, v, mono }) {
   return html`<div class="kv"><dt>${k}</dt><dd class=${mono ? 'mono' : ''}>${v}</dd></div>`;
 }
 
+/** What forwarding allows, wherever it can be switched on. */
+export const AGENT_HELP = 'With forwarding on, every session on this host, and anything claude runs in one, can sign with your SSH keys while a connection that forwards your agent is up. Prefer ssh-add -c (confirm each use) or a dedicated key.';
+
+// `[ssh] forward_agent` for a remote host. The server cannot see whether a forwarded agent is
+// live there, so the row says what the next connection asks for, never "forwarded".
+function AgentRow({ h }) {
+  const on = !!h.forward_agent;
+  return html`<div class="kv"><dt>SSH agent</dt><dd>
+    <span data-testid="host-agent">${on ? 'forward on connect' : 'not forwarded (your ssh config decides)'}</span>${' '}
+    <button type="button" class="btn btn-sm" data-testid="host-agent-btn" onClick=${() => req('hosts.forward_agent', { host: h.name, on: !on }).catch(() => {})}>${on ? 'Stop forwarding' : 'Forward'}</button>
+    <p class="wz-note">${AGENT_HELP}</p>
+  </dd></div>`;
+}
+
 function HostFacts({ h }) {
   const claude = model.value.claude[h.name];
   const working = claude && (claude.phase === 'installing' || claude.phase === 'updating');
@@ -58,6 +72,7 @@ function HostFacts({ h }) {
       ${offered ? html`<${Fact} k="Update" v=${`claude ${offered.have ?? '?'} → ${offered.want ?? '?'} is available`} />` : null}
       <${Fact} k="Daemon" v=${h.version ? `claudio ${h.version}${h.outdated ? ' (older than this server; it is replaced once idle)' : ''}` : null} />
       <${Fact} k="Sessions" v=${sessions} />
+      ${remote ? html`<${AgentRow} h=${h} />` : null}
       <div class="kv"><dt>Features</dt><dd>${(h.features ?? []).length ? (h.features ?? []).map((f) => html`<span class="chip" key=${f}>${f}</span>`) : html`<span class="muted">none</span>`}</dd></div>
     </dl>
     ${h.error ? html`<pre class="host-error" data-testid="host-error">${h.error}</pre>` : null}
@@ -75,12 +90,15 @@ function AddHost() {
   const [cands, setCands] = useState([]);
   const [busy, setBusy] = useState(false);
   const [err, setErr] = useState(null);
+  const [agent, setAgent] = useState(false);
   useEffect(() => { reqQuiet('hosts.candidates').then((r) => setCands(r.hosts ?? [])).catch(() => {}); }, []);
   async function connect(name) {
     const host = name.trim();
     if (!host) return;
     setBusy(true); setErr(null);
     try {
+      // Always sent, so a host added again with the box clear loses an old entry.
+      await reqQuiet('hosts.forward_agent', { host, on: agent });
       await reqQuiet('hosts.connect', { host });
       toast({ text: `Connected to ${host}.`, level: 'info' });
       setAlias('');
@@ -98,6 +116,8 @@ function AddHost() {
       <datalist id="host-candidates">${cands.map((c) => html`<option value=${c}></option>`)}</datalist>
       <button type="button" class="btn btn-primary" data-testid="host-add-btn" disabled=${busy || !alias.trim()} onClick=${() => connect(alias)}>${busy ? 'Connecting…' : 'Connect'}</button>
     </div>
+    <label class="wz-check"><input type="checkbox" data-testid="host-add-agent" checked=${agent} onChange=${(e) => setAgent(e.target.checked)} /> Forward my SSH agent</label>
+    ${agent ? html`<p class="wz-note">${AGENT_HELP}</p>` : null}
     ${cands.length ? html`<div class="host-cands">${cands.slice(0, 12).map((c) => html`<button type="button" class="chip chip-btn" key=${c} data-testid="host-candidate" onClick=${() => setAlias(c)}>${c}</button>`)}</div>` : null}
     ${err ? html`<pre class="host-error" data-testid="host-connect-error" role="alert">${err}</pre>` : null}
   </section>`;
diff --git a/addons/web/web/app/ui/wizard.js b/addons/web/web/app/ui/wizard.js
index 3848776..aae6c90 100644
--- a/addons/web/web/app/ui/wizard.js
+++ b/addons/web/web/app/ui/wizard.js
@@ -14,6 +14,7 @@ import { ago } from './glyph.js';
 import { openSession } from './session-actions.js';
 import { addonDecls, loadAddonDecls } from './panel.js';
 import { openChecklist, checklist as checklistShown } from './checklist.js';
+import { AGENT_HELP } from './hosts.js';
 
 export const wizardArgs = signal({});
 
@@ -44,9 +45,12 @@ function HostField({ w, set }) {
   const other = !!w.other;
   const setOther = (v) => set({ other: v });
   const [typed, setTyped] = useState('');
-  async function connectTo(name) {
+  const [agent, setAgent] = useState(false);
+  // `forward` (a typed host only): sent before the connect, whatever the box says.
+  async function connectTo(name, forward = null) {
     setBusy(name);
     try {
+      if (forward !== null) await reqQuiet('hosts.forward_agent', { host: name, on: forward });
       await reqQuiet('hosts.connect', { host: name });
       set({ host: name, hosts: [...new Set([...w.hosts, name])], resume: null, other: false });
       setTyped('');
@@ -63,7 +67,7 @@ function HostField({ w, set }) {
     await connectTo(v.slice(4));
   }
   const typedHost = typed.trim();
-  const connectTyped = () => { if (typedHost && busy === null) { setErr(null); connectTo(typedHost); } };
+  const connectTyped = () => { if (typedHost && busy === null) { setErr(null); connectTo(typedHost, agent); } };
   return html`<div class="wz-field">
     <label for="wz-host">Host</label>
     <select id="wz-host" class="select" data-testid="wizard-host" value=${other ? 'other:' : w.host} disabled=${busy !== null} onChange=${(e) => pick(e.target.value)}>
@@ -76,7 +80,9 @@ function HostField({ w, set }) {
       <input class="input" data-testid="wizard-host-input" data-autofocus placeholder="alias or user@host" autocomplete="off" spellcheck=${false} value=${typed} disabled=${busy !== null}
         onInput=${(e) => setTyped(e.target.value)} onKeyDown=${(e) => { if (e.key === 'Enter') { e.preventDefault(); e.stopPropagation(); connectTyped(); } }} />
       <button type="button" class="btn btn-primary" data-testid="wizard-host-connect" disabled=${busy !== null || !typedHost} onClick=${connectTyped}>Connect</button>
-    </div>` : null}
+    </div>
+    <label class="wz-check"><input type="checkbox" data-testid="wizard-host-agent" checked=${agent} disabled=${busy !== null} onChange=${(e) => setAgent(e.target.checked)} /> Forward my SSH agent</label>
+    ${agent ? html`<p class="wz-note">${AGENT_HELP}</p>` : null}` : null}
     ${busy ? html`<p class="wz-note">Connecting to ${busy}… (first time it uploads claudio)</p>` : null}
     ${err ? html`<pre class="wz-error" role="alert" data-testid="wizard-host-error">${err}</pre>` : null}
   </div>`;
````
Run: `cargo build --locked && cd addons/web/web/e2e && npx playwright test tests/agent.spec.mjs`
(the page is embedded at build time: rebuild first)
Expected: `2 passed`. Then `make test-web` passes: the node tests (152) and the whole e2e
(50, zero CSP violations).

Step 3: commit:
`git add addons/web/web/e2e/tests/agent.spec.mjs`
`git commit -m "feat(web): switch ssh agent forwarding from the host panel and the wizard" -- addons/web/web/app/ui/hosts.js addons/web/web/app/ui/wizard.js addons/web/web/e2e/tests/agent.spec.mjs`

Interfaces this task provides to later tasks: `AGENT_HELP`, exported by `app/ui/hosts.js`.

### Task 7: The TUI: `Alt+f` in the wizard's Where step
Model: sonnet   Repo: /home/p4u/claudio   Files: `crates/tui/src/keymap.rs`,
`crates/tui/src/interaction.rs`, `crates/tui/src/app.rs`, `crates/tui/src/hosts.rs`, `crates/tui/src/lib.rs`,
`crates/tui/src/ui/mod.rs`, `crates/tui/src/ui/wizard_host.rs` (new), `crates/tui/tests/manager.rs`,
`crates/tui/tests/common/mod.rs`
Goal: spec §4 TUI (AC8), as decisions 1-3 say.
- `Alt+f` switches the option for the highlighted remote row or the "connect to <query>" row.
  The marker and the hint follow it.
- A connected host is reconnected. Its session keeps running.
- Room for A8:
  - the Where step's drawing moves to `ui/wizard_host.rs` (unchanged, but split into ≤ 80-line
    functions, with the hint and the marker added);
  - `HostEvent::Reconnect`'s handling moves to `hosts::reconnect`;
  - `modal_key`'s wizard branch becomes a `match` of the same length.

Step 1: write the failing tests: `t7-step1.diff`.
- `app.rs`: `alt_f_switches_forwarding_for_the_highlighted_remote_host`, plus the existing
  test's new `on_ssh_hosts` argument.
- `keymap.rs`: `forward_agent_is_wizard_scoped_on_alt_f`.
- `manager.rs`: `test_wizard_alt_f_forwards_the_agent_and_reconnects_the_host`, on `FakeSsh`.
- `common`: `ALT_F`.
````diff
diff --git a/crates/tui/src/app.rs b/crates/tui/src/app.rs
index 5b6b31a..e68116e 100644
--- a/crates/tui/src/app.rs
+++ b/crates/tui/src/app.rs
@@ -749,13 +749,74 @@ mod tests {
         };
         assert_eq!(hosts(&app), ["devbox"]);
         // The fresh list replaces the untouched first screen...
-        app.on_ssh_hosts(vec!["devbox".into(), "nas".into()]);
+        app.on_ssh_hosts(vec!["devbox".into(), "nas".into()], vec![]);
         assert_eq!(hosts(&app), ["devbox", "nas"]);
         // ...but not one the user is typing in.
         app.on_terminal(press(KeyCode::Char('n')));
-        app.on_ssh_hosts(vec!["other".into()]);
+        app.on_ssh_hosts(vec!["other".into()], vec!["other".into()]);
         assert_eq!(hosts(&app), ["nas"]);
         assert_eq!(app.ssh_hosts, ["other"], "the next wizard has it");
+        assert_eq!(app.forward_agent, ["other"]);
+    }
+
+    #[test]
+    fn alt_f_switches_forwarding_for_the_highlighted_remote_host() {
+        let mut app = App::new(AppConfig {
+            ssh_hosts: vec!["devbox".into(), "nas".into()],
+            ..test_support::config()
+        });
+        app.recover(&ClientState::default(), &[]);
+        app.take_effects();
+        // The LOCAL section has focus: nothing to switch.
+        app.on_terminal(alt('f'));
+        assert!(app.take_effects().is_empty());
+        for c in "nas".chars() {
+            app.on_terminal(press(KeyCode::Char(c)));
+        }
+        assert_eq!(app.wizard_remote_host().as_deref(), Some("nas"));
+        app.on_terminal(alt('f'));
+        let effects = app.take_effects();
+        assert!(
+            matches!(&effects[..], [Effect::ForwardAgent { host, on: true }] if host == "nas"),
+            "{effects:?}"
+        );
+        assert_eq!(app.forward_agent, ["nas"], "the marker flips at once");
+        // One at a time: a second press waits for the first write.
+        app.on_terminal(alt('f'));
+        assert!(app.take_effects().is_empty());
+        app.on_forward_saved(Some((
+            vec!["devbox".into(), "nas".into()],
+            vec!["nas".into()],
+        )));
+        assert!(!app.forward_pending);
+        app.on_terminal(alt('f'));
+        let effects = app.take_effects();
+        assert!(matches!(
+            &effects[..],
+            [Effect::ForwardAgent { on: false, .. }]
+        ));
+        assert!(app.forward_agent.is_empty());
+        // The typed "connect to <query>" row is switched too.
+        app.on_forward_saved(None);
+        assert!(app.forward_agent.is_empty(), "nothing read: kept");
+        for _ in 0..3 {
+            app.on_terminal(press(KeyCode::Backspace));
+        }
+        for c in "me@10.0.0.9".chars() {
+            app.on_terminal(press(KeyCode::Char(c)));
+        }
+        assert_eq!(app.wizard_remote_host().as_deref(), Some("me@10.0.0.9"));
+        app.on_terminal(alt('f'));
+        let effects = app.take_effects();
+        assert!(
+            matches!(&effects[..], [Effect::ForwardAgent { host, on: true }] if host == "me@10.0.0.9"),
+            "{effects:?}"
+        );
+        // The typed text is the filter: plain `f` still types.
+        assert_eq!(
+            app.wizard_mut().unwrap().host_step.as_ref().unwrap().input,
+            "me@10.0.0.9"
+        );
     }
 
     #[test]
diff --git a/crates/tui/src/keymap.rs b/crates/tui/src/keymap.rs
index 25a3c2f..19ca155 100644
--- a/crates/tui/src/keymap.rs
+++ b/crates/tui/src/keymap.rs
@@ -1006,6 +1006,15 @@ mod tests {
         }
     }
 
+    #[test]
+    fn forward_agent_is_wizard_scoped_on_alt_f() {
+        let km = Keymap::default();
+        let alt_f = key(KeyCode::Char('f'), KeyModifiers::ALT);
+        assert_eq!(km.lookup_wizard(&alt_f), Some(Action::ForwardAgent));
+        assert_eq!(km.lookup(&alt_f), None, "claude still gets Alt+f");
+        assert_eq!(km.key_for(Action::ForwardAgent).as_deref(), Some("Alt+f"));
+    }
+
     #[test]
     fn toggle_hidden_is_wizard_scoped() {
         let km = Keymap::default();
diff --git a/crates/tui/tests/common/mod.rs b/crates/tui/tests/common/mod.rs
index 8d1defd..d991323 100644
--- a/crates/tui/tests/common/mod.rs
+++ b/crates/tui/tests/common/mod.rs
@@ -63,6 +63,7 @@ pub const ALT_H: &[u8] = b"\x1bh";
 pub const ALT_C: &[u8] = b"\x1bc";
 pub const ALT_L: &[u8] = b"\x1bl";
 pub const ALT_E: &[u8] = b"\x1be";
+pub const ALT_F: &[u8] = b"\x1bf";
 pub const ALT_U: &[u8] = b"\x1bu";
 pub const ENTER: &[u8] = b"\r";
 pub const CTRL_U: &[u8] = b"\x15";
diff --git a/crates/tui/tests/manager.rs b/crates/tui/tests/manager.rs
index 9f7a413..52cb432 100644
--- a/crates/tui/tests/manager.rs
+++ b/crates/tui/tests/manager.rs
@@ -27,10 +27,10 @@ use std::{fs, thread};
 
 use common::{
     current_nonce, encode_cwd, extract_nonce, run_git, scopeguard, wizard_pick_dir,
-    ClaudeProjectGuard, ManagerHarness, Region, TuiProcess, ALT_C, ALT_E, ALT_G, ALT_H, ALT_L,
-    ALT_LEFT, ALT_N, ALT_Q, ALT_R, ALT_RIGHT, ALT_SHIFT_1, ALT_SHIFT_2, ALT_U, ALT_X, ALT_Z,
-    BINARY, CTRL_U, DAEMON_WAIT, DOWN_ARROW, ENTER, ESC, LEFT_ARROW, RECONNECT_WAIT, RIGHT_ARROW,
-    UP_ARROW, WAIT,
+    ClaudeProjectGuard, FakeSsh, ManagerHarness, Region, TuiProcess, ALT_C, ALT_E, ALT_F, ALT_G,
+    ALT_H, ALT_L, ALT_LEFT, ALT_N, ALT_Q, ALT_R, ALT_RIGHT, ALT_SHIFT_1, ALT_SHIFT_2, ALT_U, ALT_X,
+    ALT_Z, BINARY, CTRL_U, DAEMON_WAIT, DOWN_ARROW, ENTER, ESC, LEFT_ARROW, RECONNECT_WAIT,
+    RIGHT_ARROW, UP_ARROW, WAIT,
 };
 use portable_pty::CommandBuilder;
 
@@ -497,6 +497,68 @@ fn test_wizard_hidden_dirs_toggle() {
     tui.quit(WAIT);
 }
 
+/// The Where step's Alt+f switches `[ssh] forward_agent` for the highlighted
+/// remote host (its marker, its hint, `config.toml`), and the connected host
+/// is reconnected with `-o ForwardAgent=yes`, its session kept running.
+#[test]
+fn test_wizard_alt_f_forwards_the_agent_and_reconnects_the_host() {
+    let fake = FakeSsh::new();
+    let harness = ManagerHarness::new();
+    let path = fake.path();
+    let mut tui = harness.start_tui_with(|cmd| cmd.env("PATH", &path));
+
+    // A typed host ("connect to"), then a session in one of its dirs.
+    tui.send_paste("fakehost");
+    tui.wait_for("connect to fakehost", Region::Screen, WAIT);
+    tui.wait_for("Alt+f ssh agent for fakehost: off", Region::Screen, WAIT);
+    tui.send_keys(ENTER);
+    tui.wait_for("New session on fakehost", Region::Screen, DAEMON_WAIT);
+    tui.send_paste(harness.dirs[0].to_str().unwrap());
+    thread::sleep(Duration::from_millis(50));
+    tui.send_keys(ENTER);
+    tui.wait_for("FAKE_CLAUDE_BANNER", Region::Pane, DAEMON_WAIT);
+    let nonce = current_nonce(&tui);
+    let bridges = fake.bridges();
+    assert_eq!(bridges.len(), 1, "{bridges:?}");
+    assert!(!bridges[0].contains("ForwardAgent"), "{bridges:?}");
+
+    // The host is a candidate now; Alt+f on its highlighted row.
+    tui.send_keys(ALT_N);
+    tui.wait_for("── REMOTE", Region::Screen, WAIT);
+    tui.send_keys(b"fakeh");
+    tui.wait_for("Alt+f ssh agent for fakehost: off", Region::Screen, WAIT);
+    tui.send_keys(ALT_F);
+    tui.wait_for("fakehost [agent]", Region::Screen, WAIT);
+    tui.wait_for(
+        "Alt+f ssh agent for fakehost: forward on connect",
+        Region::Screen,
+        WAIT,
+    );
+    let config = harness.config_home.join("claudio/config.toml");
+    let deadline = Instant::now() + WAIT;
+    while !fs::read_to_string(&config)
+        .unwrap_or_default()
+        .contains("[ssh]\nforward_agent = [\"fakehost\"]")
+    {
+        assert!(
+            Instant::now() < deadline,
+            "[ssh] forward_agent never written"
+        );
+        thread::sleep(Duration::from_millis(50));
+    }
+
+    // Reconnected with forwarding; the same session answers through it.
+    let bridges = fake.wait_bridges(2, DAEMON_WAIT);
+    assert!(bridges[1].contains("-o ForwardAgent=yes --"), "{bridges:?}");
+    tui.send_keys(ESC);
+    tui.wait_until(|s| !s.contains("── REMOTE", Region::Screen), WAIT);
+    tui.wait_until(|s| !s.contains("⇄", Region::TabBar), DAEMON_WAIT);
+    tui.send_keys(b"after-reconnect\r");
+    tui.wait_for("after-reconnect", Region::Pane, WAIT);
+    assert_eq!(current_nonce(&tui), nonce, "not respawned");
+    tui.quit(WAIT);
+}
+
 // ── Wizard: directory browser ─────────────────────────────────────────────────
 
 /// "Explore local dirs…" opens a browser at $HOME: → descends into a child,
````
Run: `cargo test --locked -p claudio-tui --lib -- alt_f forward_agent the_wizard_offers`
Expected: it does not compile:
`error[E0061]: this method takes 1 argument but 2 arguments were supplied` (`on_ssh_hosts`),
``error[E0609]: no field `forward_agent` on type `app::App` ``,
``error[E0599]: no method named `wizard_remote_host` found for struct `app::App` ``.

Step 2: implement: `t7-step2.diff`.
````diff
diff --git a/crates/tui/src/app.rs b/crates/tui/src/app.rs
index e68116e..0982c9f 100644
--- a/crates/tui/src/app.rs
+++ b/crates/tui/src/app.rs
@@ -97,8 +97,12 @@ pub enum Effect {
     Connect(String),
     /// Write state.json.
     Save,
-    /// Read the SSH host candidates again; they go to [`App::on_ssh_hosts`].
+    /// Read the SSH host candidates and `[ssh] forward_agent` again; they go
+    /// to [`App::on_ssh_hosts`].
     LoadSshHosts,
+    /// Put `host` in `[ssh] forward_agent` (`on`) or take it out, off the
+    /// event loop; then [`App::on_forward_saved`].
+    ForwardAgent { host: String, on: bool },
     /// Launch a claude session through the launch composer
     /// ([`claudio_core::app::launch`]); the outcome goes to [`App::on_launched`].
     Launch(LaunchCall),
@@ -300,6 +304,10 @@ pub struct App {
     /// The SSH hosts the wizard offers: recently used ones, then
     /// `~/.ssh/config` aliases.
     pub(super) ssh_hosts: Vec<String>,
+    /// `[ssh] forward_agent`, read with them: the wizard's markers.
+    pub(super) forward_agent: Vec<String>,
+    /// An [`Effect::ForwardAgent`] runs; the next waits for it.
+    pub(super) forward_pending: bool,
     /// The generation of the open wizard or history view, raised each time
     /// one opens. Their requests carry it, so a reply meant for one that
     /// was closed (or replaced) since is recognised and dropped.
@@ -399,6 +407,8 @@ impl App {
             claude_installing: HashSet::new(),
             confirms: VecDeque::new(),
             ssh_hosts: cfg.ssh_hosts,
+            forward_agent: Vec::new(),
+            forward_pending: false,
             modal_gen: 0,
             copy_on_select: cfg.copy_on_select,
             selection: None,
@@ -415,13 +425,60 @@ impl App {
         }
     }
 
-    /// The SSH host candidates were (re)read: offer them from now on, and in
-    /// the wizard's first screen if it is open and untouched.
-    pub fn on_ssh_hosts(&mut self, hosts: Vec<String>) {
+    /// The SSH host candidates and `[ssh] forward_agent` were (re)read: offer
+    /// them from now on, and in the wizard's first screen if it is open and
+    /// untouched.
+    pub fn on_ssh_hosts(&mut self, hosts: Vec<String>, forward_agent: Vec<String>) {
         if let Some(w) = self.wizard_mut() {
             w.set_ssh_hosts(&hosts);
         }
         self.ssh_hosts = hosts;
+        self.forward_agent = forward_agent;
+        self.redraw = true;
+    }
+
+    /// An [`Effect::ForwardAgent`] ended: the hosts and flags read after it
+    /// (`None`: they could not be read; the next wizard reads them again).
+    pub fn on_forward_saved(&mut self, read: Option<(Vec<String>, Vec<String>)>) {
+        self.forward_pending = false;
+        if let Some((hosts, forward_agent)) = read {
+            self.on_ssh_hosts(hosts, forward_agent);
+        }
+    }
+
+    /// The remote host the wizard's first screen has highlighted: a
+    /// candidate row, or the "connect to <query>" row.
+    pub fn wizard_remote_host(&self) -> Option<String> {
+        let Some(Modal::Wizard(w)) = &self.modal else {
+            return None;
+        };
+        let hs = w.host_step.as_ref().filter(|hs| hs.connecting.is_none())?;
+        if hs.focus != super::wizard::HostSection::Remote {
+            return None;
+        }
+        match hs.items.get(hs.selected) {
+            Some(host) => Some(host.clone()),
+            None => hs.connect_raw.then(|| hs.input.trim().to_owned()),
+        }
+    }
+
+    /// The wizard's forwarding key: switch `[ssh] forward_agent` for the
+    /// highlighted remote host. The marker flips now; the write runs off the
+    /// event loop, one at a time, and its end sends the flags again.
+    pub(super) fn toggle_forward_agent(&mut self) {
+        if self.forward_pending {
+            return;
+        }
+        let Some(host) = self.wizard_remote_host() else {
+            return;
+        };
+        let on = !self.forward_agent.contains(&host);
+        self.forward_agent.retain(|h| *h != host);
+        if on {
+            self.forward_agent.push(host.clone());
+        }
+        self.forward_pending = true;
+        self.emit(Effect::ForwardAgent { host, on });
         self.redraw = true;
     }
 
diff --git a/crates/tui/src/hosts.rs b/crates/tui/src/hosts.rs
index 6659691..b6df986 100644
--- a/crates/tui/src/hosts.rs
+++ b/crates/tui/src/hosts.rs
@@ -6,7 +6,7 @@ use std::time::Duration;
 
 use tokio::sync::mpsc;
 
-use super::app::App;
+use super::app::{App, Effect};
 use super::HostEvent;
 use claudio_core::app::connections::Connections;
 use claudio_core::client::{self, Client, Incoming};
@@ -14,6 +14,7 @@ use claudio_core::freshness::{self, DaemonCheck};
 use claudio_core::paths;
 use claudio_core::proto::{Msg, SessionInfo};
 use claudio_core::remote::bootstrap::ensure_remote;
+use claudio_core::remote::hosts as remote_hosts;
 
 /// A host's daemon connected (bootstrap + connect): start its reader and
 /// recover its sessions. Whether it was taken (a stale attempt is not).
@@ -41,7 +42,10 @@ pub(super) fn on_connected(
     conns.connected(host, client);
     // Record this host in the MRU so it appears first next time.
     claudio_core::remote::hosts::touch(host);
-    app.on_ssh_hosts(claudio_core::remote::hosts::candidates());
+    app.on_ssh_hosts(
+        remote_hosts::candidates(),
+        remote_hosts::forward_agent_hosts(),
+    );
     app.on_host_connected(host, &home);
     // Recover this host's sessions only.
     app.recover_host(host, &sessions);
@@ -79,6 +83,81 @@ pub(super) fn on_local_reconnected(
     true
 }
 
+/// [`Effect::LoadSshHosts`]: read the wizard's SSH hosts and `[ssh]
+/// forward_agent` now. [`Effect::ForwardAgent`]: write the option off the
+/// event loop, then read both again ([`HostEvent::ForwardSaved`]). A host
+/// that was connected and whose option changed is reconnected
+/// ([`HostEvent::Reconnect`]); a failed write is a notice.
+pub(super) fn ssh_hosts(
+    effect: Effect,
+    app: &mut App,
+    conns: &Connections,
+    ev_tx: &mpsc::Sender<HostEvent>,
+) {
+    let Effect::ForwardAgent { host, on } = effect else {
+        app.on_ssh_hosts(
+            remote_hosts::candidates(),
+            remote_hosts::forward_agent_hosts(),
+        );
+        return;
+    };
+    let generation = conns
+        .is_connected(&host)
+        .then(|| conns.current_generation(&host));
+    let tx = ev_tx.clone();
+    tokio::spawn(async move {
+        let write = move || (remote_hosts::set_forward_agent(&host, on), host);
+        match tokio::task::spawn_blocking(write).await {
+            Ok((Ok(true), host)) => {
+                if let Some(generation) = generation {
+                    let _ = tx.send(HostEvent::Reconnect { host, generation }).await;
+                }
+            }
+            Ok((Ok(false), _)) => {}
+            Ok((Err(e), host)) => {
+                let text = format!("ssh agent forwarding for {host} not saved: {e}");
+                let _ = tx.send(HostEvent::Notice(text)).await;
+            }
+            Err(_) => {
+                let text = "ssh agent forwarding not saved: the save stopped".to_owned();
+                let _ = tx.send(HostEvent::Notice(text)).await;
+            }
+        }
+        let read = || {
+            (
+                remote_hosts::candidates(),
+                remote_hosts::forward_agent_hosts(),
+            )
+        };
+        let read = tokio::task::spawn_blocking(read).await.ok();
+        let _ = tx.send(HostEvent::ForwardSaved(read)).await;
+    });
+}
+
+/// [`HostEvent::Reconnect`]: drop `host`'s connection and connect again, its
+/// sessions kept, unless a newer attempt runs or it is not connected.
+pub(super) fn reconnect(
+    app: &mut App,
+    conns: &mut Connections,
+    runtime: &super::addon_runner::Edge,
+    ev_tx: &mpsc::Sender<HostEvent>,
+    (host, generation): (String, u64),
+) {
+    if generation < conns.current_generation(&host) || !conns.is_connected(&host) {
+        return;
+    }
+    // Dropping the client closes the connection; the newer generation
+    // silences its reader.
+    conns.disconnect(&host);
+    super::addon_runner::on_incoming(app, runtime, &host, Incoming::Disconnected);
+    let gen = conns.next_generation(&host);
+    if host == "local" {
+        spawn_reconnect_local(Duration::ZERO, gen, ev_tx.clone());
+    } else {
+        spawn_connect(host, gen, ev_tx.clone(), true);
+    }
+}
+
 /// Ask every host whose daemon is outdated whether it is idle now, so it
 /// can be replaced ([`retry_outdated`]).
 pub(super) fn retry_outdated_hosts(conns: &Connections, ev_tx: &mpsc::Sender<HostEvent>) {
diff --git a/crates/tui/src/interaction.rs b/crates/tui/src/interaction.rs
index d850c7a..4f38e41 100644
--- a/crates/tui/src/interaction.rs
+++ b/crates/tui/src/interaction.rs
@@ -303,10 +303,10 @@ impl App {
         let Some(modal) = &mut self.modal else { return };
         match modal {
             Modal::Wizard(w) => {
-                let outcome = if self.keymap.lookup_wizard(&key) == Some(Action::ToggleHidden) {
-                    w.toggle_hidden()
-                } else {
-                    w.on_key(&key)
+                let outcome = match self.keymap.lookup_wizard(&key) {
+                    Some(Action::ToggleHidden) => w.toggle_hidden(),
+                    Some(Action::ForwardAgent) => return self.toggle_forward_agent(),
+                    _ => w.on_key(&key),
                 };
                 self.wizard_outcome(outcome);
             }
diff --git a/crates/tui/src/keymap.rs b/crates/tui/src/keymap.rs
index 19ca155..c10535a 100644
--- a/crates/tui/src/keymap.rs
+++ b/crates/tui/src/keymap.rs
@@ -48,6 +48,9 @@ pub enum Action {
     ExtensionsMenu,
     /// Show/hide dot-directories in the new-session wizard (wizard scope).
     ToggleHidden,
+    /// Switch ssh agent forwarding for the wizard's highlighted remote host
+    /// (wizard scope; `[ssh] forward_agent`).
+    ForwardAgent,
     /// Jump to tab `n` (`Alt+Shift+1`…`9` → 1…9, `Alt+Shift+0` → 10).
     /// Fixed: not in [`DEFAULT_BINDINGS`], so `[keys]` can't override it.
     GotoSession(u8),
@@ -74,6 +77,7 @@ impl Action {
             Action::Extensions => "extensions",
             Action::ExtensionsMenu => "extensions_menu",
             Action::ToggleHidden => "toggle_hidden",
+            Action::ForwardAgent => "forward_agent",
             Action::GotoSession(_) => "goto_session",
         }
     }
@@ -199,6 +203,16 @@ pub const DEFAULT_BINDINGS: &[Binding] = &[
         action: Action::ToggleHidden,
         scope: Scope::Wizard,
     },
+    // Alt+f: ssh agent forwarding for the wizard's highlighted remote host.
+    // Wizard scoped like Alt+.: typing there is the filter, and claude
+    // (whose readline-style Alt+f moves a word) never sees it.
+    Binding {
+        code: KeyCode::Char('f'),
+        mods: KeyModifiers::ALT,
+        label: "wizard: forward ssh agent to the host",
+        action: Action::ForwardAgent,
+        scope: Scope::Wizard,
+    },
     // Alt+u: the reader view. Verified free in Claude Code 2.1.312 (no
     // alt+u/meta+u in its bundle); readline's Alt+u (upcase-word) is
     // shadowed in terminal tabs, like Alt+l and Alt+c.
diff --git a/crates/tui/src/lib.rs b/crates/tui/src/lib.rs
index b20f135..7b45e84 100644
--- a/crates/tui/src/lib.rs
+++ b/crates/tui/src/lib.rs
@@ -18,7 +18,7 @@ mod git_view;
 mod hosts;
 use hosts::{
     client_claude, connect_local, on_connected, on_local_reconnected, retry_outdated_hosts,
-    spawn_connect, spawn_connect_after, spawn_reader, spawn_reconnect_local,
+    spawn_connect, spawn_connect_after, spawn_reader, spawn_reconnect_local, ssh_hosts,
 };
 mod interaction;
 mod keymap;
@@ -394,8 +394,11 @@ enum HostEvent {
     /// A local reconnect failed; retry after delay.
     LocalReconnectFailed { generation: u64 },
     /// Drop `host`'s connection and connect again: its outdated daemon has
-    /// become idle (see [`retry_outdated`]).
+    /// become idle ([`retry_outdated`]), or its `[ssh] forward_agent` changed.
     Reconnect { host: String, generation: u64 },
+    /// An [`Effect::ForwardAgent`] ended: the SSH hosts and `[ssh]
+    /// forward_agent` read after it (`None`: they could not be read).
+    ForwardSaved(Option<(Vec<String>, Vec<String>)>),
     /// Background upgrade check completed. `Some(tag)` means a newer version is
     /// available; `None` means we are up to date (or the check failed silently).
     UpgradeAvailable(Option<String>),
@@ -644,20 +647,9 @@ async fn event_loop(
                     spawn_reconnect_local(delay, gen, ev_tx.clone());
                 }
                 Some(HostEvent::Reconnect { host, generation }) => {
-                    if generation < conns.current_generation(&host) || !conns.is_connected(&host) {
-                        continue;
-                    }
-                    // Dropping the client closes the connection; the newer
-                    // generation silences its reader.
-                    conns.disconnect(&host);
-                    addon_runner::on_incoming(&mut app, &runtime, &host, Incoming::Disconnected);
-                    let gen = conns.next_generation(&host);
-                    if host == "local" {
-                        spawn_reconnect_local(Duration::ZERO, gen, ev_tx.clone());
-                    } else {
-                        spawn_connect(host, gen, ev_tx.clone(), true);
-                    }
+                    hosts::reconnect(&mut app, conns, &runtime, &ev_tx, (host, generation));
                 }
+                Some(HostEvent::ForwardSaved(read)) => app.on_forward_saved(read),
                 Some(HostEvent::UpgradeAvailable(tag)) => app.on_upgrade_available(tag),
                 Some(HostEvent::ClaudeLatest(latest)) => app.check_local_claude(&latest),
                 Some(HostEvent::AnswersRendered { gen, id, answers }) => match answers {
@@ -746,7 +738,7 @@ fn run_effect(
             let gen = conns.next_generation(&host);
             spawn_connect(host, gen, ev_tx.clone(), false);
         }
-        Effect::LoadSshHosts => app.on_ssh_hosts(claudio_core::remote::hosts::candidates()),
+        e @ (Effect::LoadSshHosts | Effect::ForwardAgent { .. }) => ssh_hosts(e, app, conns, ev_tx),
         Effect::CopyToClipboard(text) => copy_to_clipboard(&text),
         Effect::LoadSnippets { gen } => {
             let tx = ev_tx.clone();
diff --git a/crates/tui/src/ui/mod.rs b/crates/tui/src/ui/mod.rs
index 86343be..e43e693 100644
--- a/crates/tui/src/ui/mod.rs
+++ b/crates/tui/src/ui/mod.rs
@@ -19,6 +19,7 @@ mod leader;
 mod panel;
 mod reader;
 mod status;
+mod wizard_host;
 
 use confirm::draw_confirm;
 use git::draw_git;
@@ -90,13 +91,7 @@ fn draw_plain(frame: &mut Frame, app: &App) {
 fn draw_modal(frame: &mut Frame, app: &App, pane: Rect) {
     match &app.modal {
         Some(Modal::Rename { input, .. }) => draw_rename(frame, input),
-        Some(Modal::Wizard(w)) => {
-            let toggle_key = app
-                .keymap
-                .key_for(super::keymap::Action::ToggleHidden)
-                .unwrap_or_default();
-            draw_wizard(frame, w, app.now, &toggle_key)
-        }
+        Some(Modal::Wizard(w)) => draw_wizard(frame, w, app),
         Some(Modal::Panel(view)) => panel::draw_panel(frame, view, app.now),
         Some(Modal::Overview { selected, filter }) => draw_overview(frame, app, *selected, filter),
         Some(Modal::Help) => draw_help(frame, app),
@@ -413,7 +408,8 @@ fn draw_list(frame: &mut Frame, area: Rect, rows: &[String], selected: Option<us
     frame.render_widget(Paragraph::new(lines), area);
 }
 
-fn draw_wizard(frame: &mut Frame, w: &Wizard, now: u64, toggle_key: &str) {
+fn draw_wizard(frame: &mut Frame, w: &Wizard, app: &App) {
+    let now = app.now;
     let area = frame.area();
     let rect = centered(
         area,
@@ -423,102 +419,7 @@ fn draw_wizard(frame: &mut Frame, w: &Wizard, now: u64, toggle_key: &str) {
 
     // Step 0: first screen (LOCAL / REMOTE sections).
     if let Some(hs) = &w.host_step {
-        use super::wizard::HostSection;
-        let inner = popup(
-            frame,
-            rect,
-            "New session (↑/↓ move · Tab switch section · Enter pick · Esc cancel)",
-        );
-        if let Some(host) = &hs.connecting {
-            frame.render_widget(
-                Paragraph::new(format!("Connecting to {host}…"))
-                    .style(Style::default().fg(Color::Cyan)),
-                inner,
-            );
-            return;
-        }
-        let height = inner.height as usize;
-        // Layout: filter (2) + LOCAL label (1) + local items + REMOTE label (1) + remote items.
-        let local_count = hs.local_len().min((height.saturating_sub(4)) / 2);
-        let [filter_area, rest] =
-            ratatui::layout::Layout::vertical([Constraint::Length(2), Constraint::Min(0)])
-                .areas(inner);
-        draw_input(frame, filter_area, "filter> ", &hs.input);
-        // Render LOCAL section header.
-        let sections = ratatui::layout::Layout::vertical([
-            Constraint::Length(1),                  // LOCAL header
-            Constraint::Length(local_count as u16), // local items
-            Constraint::Length(1),                  // REMOTE header
-            Constraint::Min(0),                     // remote items
-        ])
-        .split(rest);
-        let local_focused = hs.focus == HostSection::Local;
-        let remote_focused = hs.focus == HostSection::Remote;
-        let local_hdr_style = if local_focused {
-            Style::default()
-                .fg(Color::Cyan)
-                .add_modifier(Modifier::BOLD)
-        } else {
-            Style::default().add_modifier(Modifier::DIM)
-        };
-        let remote_hdr_style = if remote_focused {
-            Style::default()
-                .fg(Color::Cyan)
-                .add_modifier(Modifier::BOLD)
-        } else {
-            Style::default().add_modifier(Modifier::DIM)
-        };
-        frame.render_widget(
-            Paragraph::new("── LOCAL ──────────────────────────────────────────────────")
-                .style(local_hdr_style),
-            sections[0],
-        );
-        // LOCAL items: "Explore local dirs…" + recent dirs, badged like the
-        // directory list.
-        let local_area = sections[1];
-        let visible = local_area.height as usize;
-        let selected = local_focused.then_some(hs.local_selected);
-        let width = local_area.width as usize;
-        let explore = truncate("  Explore local dirs…", width);
-        let explore = if selected == Some(0) {
-            Line::styled(explore, Style::default().add_modifier(Modifier::REVERSED))
-        } else {
-            Line::raw(explore)
-        };
-        let offset = selected.map_or(0, |i| i.saturating_sub(visible.saturating_sub(1)));
-        let lines: Vec<Line> = std::iter::once(explore)
-            .chain(hs.local_items.iter().enumerate().map(|(i, dir)| {
-                let mut row = dir_row(
-                    w,
-                    dir,
-                    selected == Some(i + 1),
-                    width.saturating_sub(2),
-                    now,
-                );
-                row.spans.insert(0, Span::raw("  "));
-                row
-            }))
-            .skip(offset)
-            .take(visible)
-            .collect();
-        frame.render_widget(Paragraph::new(lines), local_area);
-        frame.render_widget(
-            Paragraph::new("── REMOTE ─────────────────────────────────────────────────")
-                .style(remote_hdr_style),
-            sections[2],
-        );
-        // REMOTE items (plus optional synthetic "connect to <query>" row).
-        let mut remote_rows: Vec<String> = hs.items.iter().map(|h| format!("  {h}")).collect();
-        if hs.connect_raw {
-            remote_rows.push(format!("  connect to {}", hs.input.trim()));
-        }
-        draw_list(
-            frame,
-            sections[3],
-            &remote_rows,
-            remote_focused.then_some(hs.selected),
-        );
-        return;
+        return wizard_host::draw_host_step(frame, rect, w, hs, app);
     }
 
     // Step 2: resume picker.
@@ -590,7 +491,7 @@ fn draw_wizard(frame: &mut Frame, w: &Wizard, now: u64, toggle_key: &str) {
             ""
         };
         frame.render_widget(
-            Paragraph::new(format!("{hint}{toggle_key} hidden: {state}"))
+            Paragraph::new(format!("{hint}{} hidden: {state}", toggle_key(app)))
                 .style(Style::default().add_modifier(Modifier::DIM)),
             Rect {
                 y: head.y + 1,
@@ -602,6 +503,12 @@ fn draw_wizard(frame: &mut Frame, w: &Wizard, now: u64, toggle_key: &str) {
     draw_dir_list(frame, list, w, now);
 }
 
+/// The key that shows or hides the wizard's hidden dirs.
+fn toggle_key(app: &App) -> String {
+    let action = super::keymap::Action::ToggleHidden;
+    app.keymap.key_for(action).unwrap_or_default()
+}
+
 /// Render the directory candidate list with right-aligned metadata badges.
 ///
 /// Badges (right-aligned, only shown when metadata is available):
diff --git a/crates/tui/src/ui/wizard_host.rs b/crates/tui/src/ui/wizard_host.rs
new file mode 100644
index 0000000..060109a
--- /dev/null
+++ b/crates/tui/src/ui/wizard_host.rs
@@ -0,0 +1,145 @@
+//! The wizard's first screen: the filter, LOCAL (explore, recent dirs) and
+//! REMOTE (ssh hosts, a typed one), with the ssh agent forwarding key's hint
+//! and the marker of a host in `[ssh] forward_agent`.
+
+use ratatui::layout::{Constraint, Layout, Rect};
+use ratatui::style::{Color, Modifier, Style};
+use ratatui::text::{Line, Span};
+use ratatui::widgets::Paragraph;
+use ratatui::Frame;
+
+use super::{dir_row, draw_input, draw_list, popup, truncate};
+use crate::app::App;
+use crate::keymap::Action;
+use crate::wizard::{HostSection, HostStep, Wizard};
+
+/// After a remote host whose bridge asks for agent forwarding.
+const AGENT_MARK: &str = " [agent]";
+
+pub(super) fn draw_host_step(frame: &mut Frame, rect: Rect, w: &Wizard, hs: &HostStep, app: &App) {
+    let inner = popup(
+        frame,
+        rect,
+        "New session (↑/↓ move · Tab switch section · Enter pick · Esc cancel)",
+    );
+    if let Some(host) = &hs.connecting {
+        frame.render_widget(
+            Paragraph::new(format!("Connecting to {host}…"))
+                .style(Style::default().fg(Color::Cyan)),
+            inner,
+        );
+        return;
+    }
+    let height = inner.height as usize;
+    // Layout: filter + hint (2) + LOCAL label (1) + local items + REMOTE label (1) + remote items.
+    let local_count = hs.local_len().min((height.saturating_sub(4)) / 2);
+    let [filter_area, rest] =
+        Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).areas(inner);
+    draw_input(frame, filter_area, "filter> ", &hs.input);
+    draw_agent_hint(frame, filter_area, app);
+    let sections = Layout::vertical([
+        Constraint::Length(1),                  // LOCAL header
+        Constraint::Length(local_count as u16), // local items
+        Constraint::Length(1),                  // REMOTE header
+        Constraint::Min(0),                     // remote items
+    ])
+    .split(rest);
+    let local_focused = hs.focus == HostSection::Local;
+    let remote_focused = hs.focus == HostSection::Remote;
+    frame.render_widget(
+        Paragraph::new("── LOCAL ──────────────────────────────────────────────────")
+            .style(header(local_focused)),
+        sections[0],
+    );
+    draw_local(frame, sections[1], w, hs, local_focused, app.now);
+    frame.render_widget(
+        Paragraph::new("── REMOTE ─────────────────────────────────────────────────")
+            .style(header(remote_focused)),
+        sections[2],
+    );
+    // REMOTE items (plus optional synthetic "connect to <query>" row).
+    let mark = |h: &str| match app.forward_agent.iter().any(|f| f == h) {
+        true => AGENT_MARK,
+        false => "",
+    };
+    let mut remote_rows: Vec<String> = hs
+        .items
+        .iter()
+        .map(|h| format!("  {h}{}", mark(h)))
+        .collect();
+    if hs.connect_raw {
+        let typed = hs.input.trim();
+        remote_rows.push(format!("  connect to {typed}{}", mark(typed)));
+    }
+    draw_list(
+        frame,
+        sections[3],
+        &remote_rows,
+        remote_focused.then_some(hs.selected),
+    );
+}
+
+/// A section header: bright while it has the focus.
+fn header(focused: bool) -> Style {
+    match focused {
+        true => Style::default()
+            .fg(Color::Cyan)
+            .add_modifier(Modifier::BOLD),
+        false => Style::default().add_modifier(Modifier::DIM),
+    }
+}
+
+/// LOCAL items: "Explore local dirs…" + recent dirs, badged like the
+/// directory list.
+fn draw_local(frame: &mut Frame, area: Rect, w: &Wizard, hs: &HostStep, focused: bool, now: u64) {
+    let visible = area.height as usize;
+    let selected = focused.then_some(hs.local_selected);
+    let width = area.width as usize;
+    let explore = truncate("  Explore local dirs…", width);
+    let explore = if selected == Some(0) {
+        Line::styled(explore, Style::default().add_modifier(Modifier::REVERSED))
+    } else {
+        Line::raw(explore)
+    };
+    let offset = selected.map_or(0, |i| i.saturating_sub(visible.saturating_sub(1)));
+    let lines: Vec<Line> = std::iter::once(explore)
+        .chain(hs.local_items.iter().enumerate().map(|(i, dir)| {
+            let mut row = dir_row(
+                w,
+                dir,
+                selected == Some(i + 1),
+                width.saturating_sub(2),
+                now,
+            );
+            row.spans.insert(0, Span::raw("  "));
+            row
+        }))
+        .skip(offset)
+        .take(visible)
+        .collect();
+    frame.render_widget(Paragraph::new(lines), area);
+}
+
+/// The filter's second row: what the forwarding key does to the highlighted
+/// remote host.
+fn draw_agent_hint(frame: &mut Frame, area: Rect, app: &App) {
+    if area.height < 2 {
+        return;
+    }
+    let key = app.keymap.key_for(Action::ForwardAgent).unwrap_or_default();
+    let hint = match app.wizard_remote_host() {
+        Some(host) if app.forward_agent.contains(&host) => {
+            format!("{key} ssh agent for {host}: forward on connect")
+        }
+        Some(host) => format!("{key} ssh agent for {host}: off (your ssh config decides)"),
+        None => format!("{key} forward your ssh agent (pick a remote host)"),
+    };
+    let row = Rect {
+        y: area.y + 1,
+        height: 1,
+        ..area
+    };
+    let text = truncate(&hint, area.width as usize);
+    let style = Style::default().add_modifier(Modifier::DIM);
+    frame.render_widget(Paragraph::new(text).style(style), row);
+}
````
Run: `cargo test --locked -p claudio-tui --lib -- alt_f forward_agent the_wizard_offers`, then
`cargo test --locked --test manager test_wizard_alt_f -- --test-threads=1`, then the same test
on the core alone:
`cargo test --locked --no-default-features --test manager test_wizard_alt_f -- --test-threads=1`
Expected: `test result: ok. 3 passed`, then `test result: ok. 1 passed` (about 4 s) twice.

Step 3: `make check`, `cargo test --locked -p claudio-tui --lib` (393), and
`cargo test --locked --test manager --test vocdoni_manager -- --test-threads=1` (43 and 16).
A8 numbers:
- `lib.rs` 814 → 806, `lib.rs::event_loop` 229 → 218, `lib.rs::run_effect` 123 → 123;
- `interaction.rs` 1024 → 1024, `interaction.rs::modal_key` 113 → 113;
- `ui/mod.rs` 866 → 773, `ui/mod.rs::draw_wizard` 188 → 94;
- `wizard.rs` untouched;
- `ui/wizard_host.rs` 146, its longest function 62.

Then commit:
`git add crates/tui/src/ui/wizard_host.rs`
`git commit -m "feat(tui): alt+f in the wizard's where step switches ssh agent forwarding" -- crates/tui/src/keymap.rs crates/tui/src/interaction.rs crates/tui/src/app.rs crates/tui/src/hosts.rs crates/tui/src/lib.rs crates/tui/src/ui/mod.rs crates/tui/src/ui/wizard_host.rs crates/tui/tests/manager.rs crates/tui/tests/common/mod.rs`

Interfaces this task provides to later tasks:
- `Action::ForwardAgent` (`[keys] forward_agent`, default `alt+f`, wizard scope);
- `Effect::ForwardAgent { host, on }`, `HostEvent::ForwardSaved`;
- `App::{on_ssh_hosts(hosts, forward_agent), on_forward_saved, wizard_remote_host}`.

### Task 8: AC9 on a real host, and the docs
Model: sonnet   Repo: /home/p4u/claudio   Files: `crates/core/src/remote/diag.rs`, `tests/remote.rs`,
`docs/web.md`, `docs/manager-design.md`, `CLAUDE.md`, `README.md`
Goal: AC9 (decision 11) and AC10.
- `claudio __agent-check HOST [ID]` (diag builds only): in a new terminal on HOST (printed as
  `session=<id>` and left running), or in session ID (killed afterwards), it runs `ssh-add -l`.
  It prints the `SHA256:` fingerprints only.
- `t10` checks that a terminal lists the local agent's fingerprints, and that the same
  terminal still does over a second bridge.
- Docs:
  - docs/web.md "SSH hosts": the option, the trust warning, where the agent comes from (the
    web service), newest wins, ControlMaster, old daemons; plus a TUI parity row;
  - docs/manager-design.md §2.4.1;
  - CLAUDE.md's Remote bullet;
  - README: one sentence and the `[ssh]` table in the config example.

Step 1: write the gated test: `t8-step1.diff`.
````diff
diff --git a/tests/remote.rs b/tests/remote.rs
index 14985fe..6f74dfa 100644
--- a/tests/remote.rs
+++ b/tests/remote.rs
@@ -8,7 +8,8 @@
 //! ```
 //!
 //! Tests `t1`, `t7`, and `t8` are pure unit-style tests (no SSH) and always
-//! run.  Tests `t2`–`t6` and `t9` require a real SSH host.
+//! run.  Tests `t2`–`t6`, `t9` and `t10` require a real SSH host (`t10`
+//! also a local ssh agent holding a key: it checks agent forwarding).
 //!
 //! **Note on `__` diag commands:** `t2`–`t4` use `claudio __bootstrap`,
 //! `__connect-check`, and `__remote-session`.  These may later be gated
@@ -515,3 +516,74 @@ fn t9_remote_plugin_manifest() {
         "rm -f \"${XDG_CACHE_HOME:-$HOME/.cache}/claudio/plugins/claudio-t9.\"*",
     );
 }
+
+// ── t10: ssh agent forwarding ────────────────────────────────────────────────
+
+/// The `SHA256:` fingerprints in `text` (`ssh-add -l`'s, or `__agent-check`'s).
+fn fingerprints(text: &str) -> std::collections::BTreeSet<String> {
+    text.split_whitespace()
+        .filter_map(|w| w.strip_prefix("fingerprint=").or(Some(w)))
+        .filter(|w| w.starts_with("SHA256:"))
+        .map(str::to_owned)
+        .collect()
+}
+
+/// `claudio __agent-check HOST [ID]` with `cfg` as `XDG_CONFIG_HOME`.
+fn agent_check(cfg: &std::path::Path, host: &str, id: Option<&str>) -> String {
+    let out = Command::new(BINARY)
+        .args(["__agent-check", host])
+        .args(id)
+        .env("XDG_CONFIG_HOME", cfg)
+        .output()
+        .unwrap_or_else(|e| panic!("failed to run claudio: {e}"));
+    let stderr = String::from_utf8_lossy(&out.stderr);
+    assert!(out.status.success(), "__agent-check failed: {stderr}");
+    String::from_utf8_lossy(&out.stdout).into_owned()
+}
+
+/// With `[ssh] forward_agent` naming the host, a terminal there lists the
+/// keys of the agent this test runs with (the same fingerprints as
+/// `ssh-add -l` here), and the same terminal still does after a second
+/// bridge replaced the first. Only fingerprints are compared or printed.
+#[test]
+fn t10_agent_forwarding_reaches_sessions_across_bridges() {
+    let host = match test_host() {
+        Some(h) => h,
+        None => {
+            println!("SKIP t10: set CLAUDIO_SSH_TEST_HOST to run SSH tests");
+            return;
+        }
+    };
+    let local = Command::new("ssh-add").arg("-l").output();
+    let local = fingerprints(
+        &local
+            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
+            .unwrap_or_default(),
+    );
+    if local.is_empty() {
+        println!("SKIP t10: the local ssh agent holds no key (ssh-add one first)");
+        return;
+    }
+    let (_, stderr, ok) = run_claudio(&["__bootstrap", &host]);
+    assert!(ok, "__bootstrap failed: {stderr}");
+    let cfg = env::temp_dir().join(format!("cl-agent-{}", std::process::id()));
+    std::fs::create_dir_all(cfg.join("claudio")).unwrap();
+    let toml = format!("[ssh]\nforward_agent = [{:?}]\n", host);
+    std::fs::write(cfg.join("claudio/config.toml"), toml).unwrap();
+
+    let first = agent_check(&cfg, &host, None);
+    let id = first
+        .lines()
+        .find_map(|l| l.strip_prefix("session="))
+        .expect("a session id")
+        .to_owned();
+    let second = agent_check(&cfg, &host, Some(&id));
+    let _ = std::fs::remove_dir_all(&cfg);
+    println!("[t10] local agent: {} key(s)", local.len());
+    assert_eq!(fingerprints(&first), local, "the first bridge's terminal");
+    assert_eq!(
+        fingerprints(&second),
+        local,
+        "the same terminal after a second bridge"
+    );
+}
````
Run: `cargo test --locked --features diag,vocdoni --test remote -- --test-threads=1 --nocapture`
Expected: `test result: ok. 10 passed`, with `SKIP t10: set CLAUDIO_SSH_TEST_HOST to run SSH tests`.
Without a host, `t10` cannot fail here. With one it fails until step 2: `__agent-check` is not
a claudio word yet.

Step 2: implement the diag word and the docs: `t8-step2.diff`.
````diff
diff --git a/CLAUDE.md b/CLAUDE.md
index 59a4b08..4ccc1b9 100644
--- a/CLAUDE.md
+++ b/CLAUDE.md
@@ -231,7 +231,10 @@ flowchart LR
   in one round trip; a failed probe reports ssh's stderr, and `ssh_cmd` uses
   `StrictHostKeyChecking=accept-new`) and uploads the binary if missing or stale (SHA-256);
   cross-platform it uploads the release asset. `bridge.rs` is `--slave`, the stdio↔socket bridge,
-  starting or replacing the remote daemon.
+  starting or replacing the remote daemon. Agent forwarding (`[ssh] forward_agent` per host,
+  `ssh_cmd`'s `-o ForwardAgent=yes`): `agent.rs` links the bridge's live `SSH_AUTH_SOCK` under
+  `<runtime>/agents/`, and the daemon's relay (`daemon/agent.rs`, `agent-v{PROTO}.sock`) is every
+  session's `SSH_AUTH_SOCK` once one exists (docs/manager-design.md §2.4.1).
 - **The shared layer** (`crates/core/src/app/`, no ratatui, crossterm or axum; the web imports
   nothing from claudio-tui): `addons/` (`AddonRuntime`, the one `AddonCtx` implementation),
   `launch/` (the launch composer), `config_edit.rs` (every `config.toml` writer: a minimal,
diff --git a/README.md b/README.md
index a6ac6ed..5a263a7 100644
--- a/README.md
+++ b/README.md
@@ -120,7 +120,7 @@ You need a Claude Code account. If `claude` is missing, claudio installs it with
 3. Start a new conversation or resume one from that folder.
 4. Close the browser tab or quit with `Alt+q`. The sessions keep running; open either UI again and they are where you left them.
 
-**SSH hosts.** Hosts come from `~/.ssh/config`, the ones you used before, or a name or `user@host` you type (Alt+n, the web's hosts panel). Key-based `ssh <host>` must work without a password. A host's key is recorded at its first connection and a changed key is refused (`StrictHostKeyChecking=accept-new` on ssh's command line, whatever your ssh config sets). claudio copies itself to `~/.local/bin/claudio` on the host, keeps it in sync by checksum, and starts a daemon there (under `systemd-run --user` when it can). No remote dotfiles are touched. The daemon finds claude through your login shell's `PATH`; `CLAUDIO_CLAUDE_PATH` pins one, and `claudio daemon status` shows which one a host uses.
+**SSH hosts.** Hosts come from `~/.ssh/config`, the ones you used before, or a name or `user@host` you type (Alt+n, the web's hosts panel). Key-based `ssh <host>` must work without a password. A host's key is recorded at its first connection and a changed key is refused (`StrictHostKeyChecking=accept-new` on ssh's command line, whatever your ssh config sets). claudio copies itself to `~/.local/bin/claudio` on the host, keeps it in sync by checksum, and starts a daemon there (under `systemd-run --user` when it can). No remote dotfiles are touched. The daemon finds claude through your login shell's `PATH`; `CLAUDIO_CLAUDE_PATH` pins one, and `claudio daemon status` shows which one a host uses. To use your SSH keys from sessions on a host (`git push`, `ssh` onward), turn on agent forwarding for it: `Alt+f` on the host in Alt+n, or the host panel in the web ([docs/web.md](docs/web.md#ssh-hosts) says what that lets the host do).
 
 **Keeping claude current.** Once a day claudio compares `claude --version` with the latest release and asks before running `claude update`; an SSH host with an older claude than yours gets the same offer. Running sessions keep their binary. claudio starts claude with `--allow-dangerously-skip-permissions` when claude supports it, so bypass mode is one `Shift+Tab` away; claude never starts in it (`allow_skip_permissions = false` turns this off).
 
@@ -203,6 +203,9 @@ update_check = "ask"            # this machine's claude vs its release channel:
 remote_check = "ask"            # SSH hosts' claude vs this machine's: "ask" | "auto" | "off"
 install = "auto"                # install claude where a host has none: "auto" | "ask" | "off"
 
+[ssh]
+forward_agent = ["devbox"]      # hosts whose connection forwards your ssh agent (Alt+f in Alt+n)
+
 [extensions]                    # every extension is on unless turned off here
 diagram = false                 # e.g. no Mermaid instruction or drawing (Alt+i writes this table)
 
diff --git a/crates/core/src/remote/diag.rs b/crates/core/src/remote/diag.rs
index 3104c1b..ad10b69 100644
--- a/crates/core/src/remote/diag.rs
+++ b/crates/core/src/remote/diag.rs
@@ -12,7 +12,8 @@ use std::process::ExitCode;
 use std::time::Duration;
 
 /// The core's diag words (`__bootstrap`, `__connect-check`,
-/// `__remote-session`, `__ssh-hosts`); `None` for any other word.
+/// `__remote-session`, `__ssh-hosts`, `__agent-check`); `None` for any
+/// other word.
 pub fn dispatch(argv: &[String]) -> Option<ExitCode> {
     let arg = |i: usize| argv.get(i).map(String::as_str);
     let code = match (arg(1)?, arg(2), arg(3)) {
@@ -20,6 +21,8 @@ pub fn dispatch(argv: &[String]) -> Option<ExitCode> {
         ("__connect-check", Some(host), _) => connect_check_cmd(host),
         ("__remote-session", Some(host), Some(cwd)) => remote_session_cmd(host, cwd),
         ("__ssh-hosts", _, _) => ssh_hosts_cmd(),
+        ("__agent-check", Some(host), id) => agent_check_cmd(host, id),
+        ("__agent-check", ..) => usage("__agent-check <host> [<session id>]"),
         ("__bootstrap", ..) => usage("__bootstrap <host>"),
         ("__connect-check", ..) => usage("__connect-check <host>"),
         ("__remote-session", ..) => usage("__remote-session <host> <cwd>"),
@@ -232,3 +235,118 @@ fn ssh_hosts_cmd() -> ExitCode {
     }
     ExitCode::SUCCESS
 }
+
+/// `claudio __agent-check HOST [ID]`
+///
+/// In a terminal on `HOST` (a new one, printed as `session=<id>` and left
+/// running, or session `ID`, killed afterwards), runs `ssh-add -l` and
+/// prints the fingerprints it lists, `fingerprint=SHA256:…`, never a key's
+/// comment. For `tests/remote.rs`' agent forwarding test.
+fn agent_check_cmd(host: &str, id: Option<&str>) -> ExitCode {
+    let given = match id.map(uuid::Uuid::parse_str) {
+        None => None,
+        Some(Ok(id)) => Some(id),
+        Some(Err(_)) => return usage("__agent-check <host> [<session id>]"),
+    };
+    let rt = match make_runtime() {
+        Ok(r) => r,
+        Err(code) => return code,
+    };
+    let host = host.to_owned();
+    rt.block_on(async move {
+        let client = match crate::client::connect_ssh(&host).await {
+            Ok(c) => c,
+            Err(e) => {
+                eprintln!("connect_ssh({host}) failed: {e}");
+                return ExitCode::FAILURE;
+            }
+        };
+        let id = match given {
+            Some(id) => id,
+            None => match spawn_shell(&client).await {
+                Ok(id) => id,
+                Err(e) => {
+                    eprintln!("SpawnShell failed: {e}");
+                    return ExitCode::FAILURE;
+                }
+            },
+        };
+        let output = ssh_add_in(&client, id).await;
+        if given.is_some() {
+            let _ = client.request(crate::proto::Msg::Kill { id }).await;
+        }
+        let Some(output) = output else {
+            eprintln!("no answer from the terminal");
+            return ExitCode::FAILURE;
+        };
+        for word in output.split_whitespace() {
+            if word.starts_with("SHA256:") {
+                println!("fingerprint={word}");
+            }
+        }
+        ExitCode::SUCCESS
+    })
+}
+
+/// A new terminal in `/tmp`, announced as `session=<id>`.
+async fn spawn_shell(client: &crate::client::Client) -> Result<uuid::Uuid, String> {
+    let id = uuid::Uuid::new_v4();
+    let spec = crate::proto::ShellSpec {
+        id,
+        cwd: "/tmp".to_owned(),
+        name: None,
+        rows: 24,
+        cols: 160,
+    };
+    match client.request(crate::proto::Msg::SpawnShell(spec)).await {
+        Ok(crate::proto::Msg::Spawned { .. }) => {
+            println!("session={id}");
+            Ok(id)
+        }
+        Ok(other) => Err(format!("unexpected reply {other:?}")),
+        Err(e) => Err(e.to_string()),
+    }
+}
+
+/// Attach to terminal `id`, let its snapshot pass, run `ssh-add -l` there
+/// and collect what it prints until a marker computed by the shell (so the
+/// echoed command line never matches it); `None` after 15 s.
+async fn ssh_add_in(client: &crate::client::Client, id: uuid::Uuid) -> Option<String> {
+    use crate::client::Incoming;
+    let mut rx = client.take_incoming()?;
+    let attach = crate::proto::Msg::Attach {
+        id,
+        rows: 24,
+        cols: 160,
+    };
+    client.request(attach).await.ok()?;
+    let settle = tokio::time::sleep(Duration::from_millis(500));
+    tokio::pin!(settle);
+    loop {
+        tokio::select! {
+            _ = &mut settle => break,
+            item = rx.recv() => item?,
+        };
+    }
+    let n = std::process::id() % 1000 + 1000;
+    let line = format!("ssh-add -l; echo agent-check-$(({n}+1))\r");
+    let marker = format!("agent-check-{}", n + 1);
+    client.send_input(id, line.as_bytes());
+    let mut text = String::new();
+    let collect = async {
+        while !text.contains(&marker) {
+            match rx.recv().await? {
+                Incoming::Data { id: from, bytes } if from == id => {
+                    text.push_str(&String::from_utf8_lossy(&bytes));
+                }
+                Incoming::Disconnected => return None,
+                _ => {}
+            }
+        }
+        Some(())
+    };
+    tokio::time::timeout(Duration::from_secs(15), collect)
+        .await
+        .ok()??;
+    Some(text)
+}
diff --git a/docs/manager-design.md b/docs/manager-design.md
index 702f4f7..e2219fb 100644
--- a/docs/manager-design.md
+++ b/docs/manager-design.md
@@ -282,6 +282,32 @@ Recovery runs on start or reconnect:
 
 With the proxy enabled, the remote needs **no claude login**.
 
+### 2.4.1 Agent forwarding
+
+A host listed in `config.toml`'s core section `[ssh] forward_agent` (host strings exactly as
+connected) gets `-o ForwardAgent=yes` on the ssh that carries its bridge (`remote::ssh_cmd`; never
+on bootstrap's). Without it claudio says nothing about forwarding and `~/.ssh/config` decides.
+`remote::hosts::set_forward_agent` is the one writer; the TUI (`Alt+f` in the wizard's Where
+step) and the web (`hosts.forward_agent`) call it, and switching a connected host reconnects it.
+
+sshd's forwarded socket lives as long as one connection, while the daemon and its sessions
+outlive every connection. So:
+
+- **The bridge** (`remote/agent.rs`) asks its `SSH_AUTH_SOCK` for identities (2 s, the answer's
+  type only). One that answers is linked as `<runtime>/agents/<pid>` (made private, renamed into
+  place) and unlinked when the bridge exits. A failure is one stderr line.
+- **The daemon's relay** (`daemon/agent.rs`) listens on `<runtime>/agent-v{PROTO}.sock` (0600,
+  peer uid checked, at most 32 connections). Each connection goes to the newest live link
+  (`lstat` mtime, then name), then to the daemon's own inherited `SSH_AUTH_SOCK`. A link whose
+  target refuses, is gone or is forbidden is removed. Bytes are copied, never read.
+- **Sessions** (claude and terminals) get `SSH_AUTH_SOCK=<relay>` once `agents/` exists: once a
+  bridge linked a working agent there. A host no bridge reaches (the local one) keeps its own
+  agent. A spawn's `env_remove` of `SSH_AUTH_SOCK` drops it, and a value in its env wins.
+
+A session keeps reaching an agent through any later forwarding connection, whichever started
+it; the newest one wins when several forward different agents. A busy daemon older than this
+feature ignores the links until it is replaced (`claudio daemon restart` on that host).
+
 ### 2.5 claude-proxy integration
 
 #### Configuration
diff --git a/docs/web.md b/docs/web.md
index a2c2340..33f73f9 100644
--- a/docs/web.md
+++ b/docs/web.md
@@ -80,7 +80,8 @@ allow_hosts = ["claudio.example.org"]
   running service at once: every browser signs in again.
 - The server runs with the daemon's environment, not the terminal's that ran `claudio web`: its
   SSH hosts use the daemon's `SSH_AUTH_SOCK` (stale or absent when the daemon was started over ssh
-  or by `systemd-run`), and `CLAUDIO_*` set in the terminal do not reach it. Run
+  or by `systemd-run`), so its connections usually forward no agent (below), and `CLAUDIO_*` set
+  in the terminal do not reach it. Run
   `claudio daemon restart` from a shell with the environment you want, or use `--foreground`.
 - A daemon older than this claudio (or a build without the web addon) has no web service:
   `claudio web` says so, keeps `[web]` saved, and exits 1. `claudio daemon restart` when its
@@ -109,6 +110,31 @@ is still accepted at its first connection, and with `no`, a host whose key chang
 A host that cannot be reached shows ssh's own error (`ssh failed (exit N): …`, such as "Could
 not resolve hostname").
 
+**SSH agent forwarding.** A remote host's panel has an "SSH agent" row with a button, and the add
+form and Alt+n's "Another host…" a "Forward my SSH agent" box (sent before the connect, checked
+or not). They set the host in `config.toml`'s `[ssh] forward_agent` (the TUI: `Alt+f` on the
+host in Alt+n's Where step). claudio then connects that host with `-o ForwardAgent=yes`; without
+it, claudio passes nothing and your ssh config decides. Switching a connected host reconnects it;
+its sessions keep running. On the host, every claude session and terminal reaches the agent of
+the newest connection that forwards one, for as long as one does, whichever connection started
+the session, and a reconnect restores it without a restart. Sessions started before the first
+forwarding connection to that host need a restart. Things to know:
+
+- **Trust.** While a forwarding connection is up, every session on that host, anything claude
+  runs in one, and root there can sign with your keys. That is the feature; prefer `ssh-add -c`
+  (confirm each use) or a dedicated key.
+- **Where the agent comes from.** Forwarding needs an agent where the ssh runs. The web service
+  runs with the daemon's environment, which often has no `SSH_AUTH_SOCK`, so its connections
+  register nothing: forwarding reaches the host while a client whose own environment has a live
+  agent is connected, normally the desktop TUI or a `--foreground` web started from a desktop
+  terminal. The panel says "forward on connect", never "forwarded": the server cannot see the
+  remote side.
+- **Several clients.** When two machines forward different agents, sessions use the newest.
+- **ControlMaster.** A multiplexed connection forwards only if its master was opened with
+  forwarding (OpenSSH decides by the master's `ForwardAgent`).
+- **Old daemons.** A daemon older than this claudio, left running because it was busy, ignores
+  forwarded agents until it is replaced (`claudio daemon restart` on that host).
+
 ### From another machine
 
 Without `--remote` the server never listens on a non-loopback address. Three ways to reach it:
@@ -320,6 +346,7 @@ Rust test in `addons/web/tests/web.rs`; rows without an automated test say how t
 | Capability | TUI key | Web action | WS op(s) | Test |
 |---|---|---|---|---|
 | Sessions on many hosts | wizard host list | the top bar's Hosts menu (one row per host), host panel, Add host… | `hosts.list`, `hosts.candidates`, `hosts.connect`, `hosts.disconnect` | `web_hosts_report_daemon_features`; ssh hosts: manual check on a real host |
+| SSH agent forwarding | `Alt+f` in the wizard's Where step | host panel "SSH agent" row; "Forward my SSH agent" in Add host and the wizard's "Another host…" | `hosts.forward_agent`, Host `forward_agent` | `the host panel switches ssh agent forwarding for a remote host`; `the wizard sends the agent choice before it connects a typed host`; `web_forward_agent_reconnects_a_connected_host_and_keeps_its_sessions`; `web_forwarded_agent_reaches_terminals_through_the_relay` |
 | Terminal panes (1, 2, 4) | `Alt+←/→` | pane grid buttons, `1`-`4`, drag a card | `session.attach`, `session.detach`, `session.resize`, `layout.save` | `a pane shows the terminal and typed keys echo`; `web_attach_snapshot_input_and_resize`; `web_two_tabs_share_one_attach` |
 | Terminal tabs | `Alt+c` | `t`, "Terminal beside" in a pane | `session.shell` | `a terminal tab runs echo ok`; `web_session_lifecycle_shell_rename_kill` |
 | New-session wizard: dirs | `Alt+n` | `n`: folder field, recent folders, browser | `fs.list`, `fs.projects`, `session.spawn` | `the wizard starts a session in a chosen dir` |
````
Run:
- `cargo test --locked --features diag,vocdoni --test remote -- --test-threads=1`;
- `cargo build --locked --features diag,vocdoni && target/debug/claudio __agent-check`.

Expected: `test result: ok. 10 passed`; then `usage: claudio __agent-check <host> [<session id>]`
and exit status 1. Docs: read the four changed sections against spec §4 Web, the Failure modes
and security section, and Compatibility ("ControlMaster", "New bridge, old remote daemon").

Step 3: `make check`, then two commits:
`git commit -m "test(remote): agent forwarding on a real host through __agent-check" -- crates/core/src/remote/diag.rs tests/remote.rs`
`git commit -m "docs: ssh agent forwarding per host" -- docs/web.md docs/manager-design.md CLAUDE.md README.md`

Interfaces this task provides to later tasks: none (last task).

## Merge and release

1. **Full suites** on `feat/agent-forwarding` (the controller):
   - `make check` → `rules: 0 problem(s)`, every step green;
   - `make matrix` → all five cells and the core-only `manager` suite (43) pass;
   - `cargo test --locked --test manager --test vocdoni_manager --test web --test web_manager --test vocdoni_web -- --test-threads=1`
     → `manager` 43, `vocdoni_manager` 16, `web` 55, `web_manager` 4, `vocdoni_web` 13;
   - `make test-web` → node 152, Playwright 50;
   - `cargo test --features diag,vocdoni --test remote -- --test-threads=1` → 10 passed.
     **AC9:** if an SSH test host is reachable, run it again with
     `CLAUDIO_SSH_TEST_HOST=<host> … -- --test-threads=1 --nocapture` from a shell whose ssh
     agent holds a key. `t10` must pass (`[t10] local agent: N key(s)`). If no host is
     available, say so in the gate report and the release notes: AC9 then rests on the
     hermetic `web_forwarded_agent_reaches_terminals_through_the_relay`.
2. **Fable gate audit** of `main...feat/agent-forwarding`, against the spec, this plan's
   decisions and the Review Focus. Fixes are re-reviewed, then step 1 is re-run.
3. **Merge**: `git switch main && git merge --no-ff feat/agent-forwarding`. If
   feat/limit-auto-switch merged first, its plan
   (docs/plans/2026-11-12-limit-auto-switch-plan.md, "Shared files with
   feat/agent-forwarding") puts its edits apart from this branch's. What remains to watch:
   - `crates/core/src/daemon/mod.rs`. This branch adds:
     - `mod agent;` in the module list;
     - the `Config` field `agent` right after `socket`, and its `from_env` line right after
       `socket:`;
     - `let relay = agent::start(&daemon);` / `drop(relay);` around `serve()` in
       `serve_until_shutdown`;
     - one line in the test literal.

     That branch adds `limit_attempt_timeout` after `addon_timeout`, in the struct, in
     `from_env` and in the literals. Both merge cleanly into the same literals. The production
     part becomes 780 + 6 + 3 = 789 lines, under the 800 budget. Confirm with
     `scripts/check_rules.py`. If it is over, move `claude_setting`/`resolve_claude_path` (pure
     helpers) to `daemon/host.rs` in a `refactor(daemon):` commit, never by raising the
     budget.
   - Every full daemon `Config { … }` literal needs both fields (the five in
     `crates/core/src/daemon/command.rs`, `TestDaemon::config`, `crates/core/src/daemon/tests.rs`,
     `daemon/mod.rs`'s tests). A literal that one branch added and the other never saw fails to
     compile. The side merging second adds the missing field (`agent: Default::default(),` or
     `limit_attempt_timeout: …`).
   - `crates/core/src/daemon/session.rs`: this branch removes
     `let path = daemon.config.path.as_deref();` from `spawn()` and changes the
     `shell_command(&shell, &cwd, &daemon.config)` call. That branch adds actor methods. After
     the merge, `session.rs` must stay at or under 1036 and `spawn` at or under 105 (A8).
   - `crates/core/src/daemon/tests.rs`: `mod agent;` in the module list; keep any module that
     branch adds beside it.
   - Docs merge apart:
     - `docs/manager-design.md`: this branch adds §2.4.1 before §2.5; that branch adds a
       subsection at the end of §2.2;
     - `CLAUDE.md`: this branch edits the "Remote" bullet; that branch the "vocdoni" bullet.
   - `Cargo.toml` / `Cargo.lock`: the version (step 4).

   After any conflict, re-run step 1's `make check` and the five suites on `main` before
   tagging.
4. **Release**:
   - Bump `version` in the root `Cargo.toml` to the next patch after the one there at merge
     time: `0.9.3`, or `0.9.4` if feat/limit-auto-switch took `0.9.3`. `v0.10.0` is reserved
     for removing the `compat(v0.10)` paths.
   - `cargo update -w`.
   - `git commit -m "chore(release): vX.Y.Z" -- Cargo.toml Cargo.lock`.
   - `git tag -a vX.Y.Z -m "vX.Y.Z"`.
   - Push over HTTPS with the gh credential helper (the shell's ssh agent is often stale here):
     `git -c credential.helper= -c 'credential.helper=!gh auth git-credential' push https://github.com/p4u/claudio.git main vX.Y.Z`.
   - The tag starts `.github/workflows/release.yml`. Once it has published the release, set
     hand-written notes in the v0.9.x style:
     `gh release edit vX.Y.Z --repo p4u/claudio --notes-file <notes>`. The notes say:
     - what forwarding does and how to turn it on (`Alt+f` in Alt+n; the host panel);
     - the trust warning (`ssh-add -c`);
     - that the web service usually has no agent of its own;
     - that a busy old daemon needs `claudio daemon restart` on that host;
     - whether AC9 ran on a real host.
5. **Owner OK:** none needed. The owner said to merge, tag and publish without asking once the
   suites and the gate audit pass (2026-11-10, standing). Report what was released afterwards.
6. **Post-release checks**:
   - `gh release view vX.Y.Z --repo p4u/claudio` lists the four platform binaries;
   - `git ls-remote https://github.com/p4u/claudio.git refs/tags/vX.Y.Z` shows the tag;
   - CI on `main` is green (Linux and macOS).
