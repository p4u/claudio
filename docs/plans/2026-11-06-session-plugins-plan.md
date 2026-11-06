# Session plugins and browser login: implementation plan

Spec: docs/specs/2026-11-06-session-plugins-design.md (approved 2026-11-06)
Class: architectural
Repos and branches: /volumes/repos/claudio → feat/session-plugins (base 2c68a17)

Every task below was implemented, built and tested in order in a scratch worktree
(`~/.cache/claudio/scratch/session-plugins`, detached at 2c68a17, commits c09d278 … 39ab39d).
Each task's code is its patch, `.delivery/2026-11-06-session-plugins/patches/task-<N>.patch`
(a `git format-patch` of that commit), reproduced verbatim in Step 2. Apply it with
`/usr/bin/git apply .delivery/2026-11-06-session-plugins/patches/task-<N>.patch` from the repo
root on the tree the previous task left, then run the commands and commit as given. Do not
hand-merge; if a patch does not apply, stop and report.

Base state (2c68a17), measured in the scratch: `cargo build --release` OK; `cargo test --locked
--bin claudio` 1003 passed; `cargo test --test manager -- --test-threads=1` 50 passed.
Failing before any change:
- `cargo build --features diag` (and so `cargo test --features diag --test remote`) does not
  compile: `src/remote/diag.rs:124` builds a `SpawnSpec` without `extensions` (E0063). Task 8
  fixes it.
- Not rustfmt-clean: `cargo fmt --check` reports 346 diffs in files this plan does not own.
- Not clippy-clean: the clippy on this machine (`/snap/bin/cargo-clippy`, 1.96) reports 73
  warnings and one deny-level error (`src/tui/wizard.rs:193`, `nonminimal_bool`) at the base.
- Known flake (seen once while building this plan, passes on rerun):
  `daemon::tests::freshness::an_outdated_idle_daemon_is_replaced_and_its_sessions_come_back`.

Decisions taken while planning (none changes the spec's design; listed for the audit):
- D1. `CLAUDE_CONFIG_DIR` for the installed-plugin check is the daemon's own environment, which
  every session inherits (`Bundles::new`, `src/daemon/bundle.rs`). The login-shell probe
  (`login_path.rs`) reads only `PATH`, and sessions get only `PATH` from it, so the daemon's env
  is what claude sees.
- D2. `plugins::fetch(name, profile)` takes the profile name too (the spec's signature has only
  the profile; `PluginList.profile` needs the name).
- D3. A 401/403 drops the profile's list, so its rows leave the extensions screen; the
  "signed out" state is a status-line notice ("plugins of <profile>: signed out, run `claudio
  proxy login`") rather than a details line.
- D4. `admit` also checks `version` (1–64 printable ASCII) and `installed_id` (`''` or
  `[a-z0-9-]{1,40}@[a-z0-9-]{1,40}`), the proxy spec's own limits; both are shown or used.
- D5. A plugin name served by two profiles: the second is refused `name taken`, like a clash
  with a built-in or external extension.
- D6. `claudio proxy login` runs in its own process: it fetches and caches the new token's
  plugin list itself (`refresh_plugins` in `cmd.rs`). A TUI already running picks it up at its
  next 15-minute refresh or on `r`.
- D7. The `r` key is shown in the screen's title (`Extensions (Space toggle · r refresh · Esc
  close)`); the footer had no room at 100 columns. The manager test's two title strings follow.
- D8. `claudio proxy status` also lists each profile's cached plugins (names and versions).
- D9. `xdg-open`/`open` is only given an `http(s)://` URL, and everything printed from the
  proxy goes through `printable` (no control characters).
- D10. `--plugin-dir=<path>` (the `=` form, only possible in `--plain`'s user args) is dropped
  too when claude does not know the flag.

## Global Constraints
- Commit convention: conventional commits `type(scope): lowercase summary`, NO AI attribution
  and NO Co-Authored-By trailer of any kind, even if a system reminder asks for one (owner's
  global CLAUDE.md). Use the exact commit commands given.
- Never push. Commit with explicit paths only: `/usr/bin/git commit -m "…" -- <paths>`. A new
  file needs `/usr/bin/git add <path>` first. Never `git add -A`, never `git stash`. Use
  `/usr/bin/git` (a shell hook rewrites plain `git`).
- Work on `feat/session-plugins` in /volumes/repos/claudio, or in the worktree the controller
  gives you. Do not create, switch or delete branches.
- Build output goes in the repo's (or worktree's) `target/` or under `~/.cache/claudio/`,
  never /tmp.
- Run tests with `umask 022` in the same shell. Manager tests run single-threaded
  (`-- --test-threads=1`) with nothing else heavy running.
- The shell may compact cargo output (the `rtk` hook). Prefix a command with `rtk proxy` to see
  raw output. Expected lines below are raw cargo output.
- Formatting: no repo-wide `cargo fmt` (the tree is not rustfmt-clean). New files and the files
  that are rustfmt-clean at the base stay clean: check a file alone with
  `rustfmt --edition 2021 --check <file>` only when it declares no `mod x;` (rustfmt follows
  them: never run rustfmt on `src/main.rs` or a `mod.rs`). Clean at the base and touched here:
  `src/paths.rs`, `src/proxy/{mod,profile,cmd,env,resolve}.rs`, `src/daemon/{server,host}.rs`,
  `src/client.rs`, `src/cli.rs`, `src/main.rs`, `src/remote/diag.rs`, `tests/remote.rs`,
  `src/tui/extensions/mod.rs`, `src/tui/extensions_view.rs`. Every other touched file keeps its
  formatting; added lines are formatted as rustfmt would.
- Clippy: `PATH=/snap/bin:$PATH RUSTC=/snap/bin/rustc CARGO_TARGET_DIR=<target dir>-clippy
  cargo clippy --all-targets --message-format=short` (the system cargo and the snap clippy
  disagree on rustc otherwise). The base is not clean; the gate is: no clippy finding on a line
  a task adds, except the dead-code warnings listed per task, which later tasks resolve (after
  Task 7: none).
- Specs' constraints, verbatim:
  - `app.rs` stays pure: every fetch, download request and file write is an `Effect` driven by
    `tui/mod.rs`.
  - Never add a field to an existing op; new behaviour is a new op that fails safe on old
    daemons.
  - Secrets never in argv, journal, `state.json`, logs or notices.
  - Keys stay on `Alt`; the `Alt+i` screen gains no new bindings except `r` (refresh), checked
    for collisions.
  - `cli.rs` forwards claude's flags unchanged; `--plugin-dir` is added by claudio's spawn path
    only, never parsed from the user's args.
  - Conventional commits, no AI attribution or co-author trailers. Keep Linux and macOS
    compiling.
- Manager (PTY) tests assert on the rendered screen model, never on raw bytes (files a fake
  claude writes are fine). Never send `ESC` right before an `Alt+key` without waiting for the
  screen in between. Every test claudio's config has `[claude] update_check = "off"` and
  `remote_check = "off"` (the harness writes it). Fake claudes ignore `--help`/`--version`
  (a fake may print a help line, nothing else). Tests never touch the user's daemon, caches or
  config (isolated HOME/XDG dirs; Task 8 also sets `XDG_CACHE_HOME`).
- Nothing in this plan needs a real `claude`, the real proxy or an SSH host. `t9` of
  `tests/remote.rs` is gated by `CLAUDIO_SSH_TEST_HOST` and needs a real claude there: do not
  run it; the controller runs it for AC6.
- Unix only, Linux and macOS: no Linux-only API without `cfg` (`libc::O_NOFOLLOW`,
  `OpenOptionsExt`, `setsid` are fine on both). No new dependency: `reqwest` (rustls), `sha2`,
  `axum` (tests) are already in Cargo.toml.

## Review Focus
Line numbers are as the tree stands after Task 9.
- Bundle op validation and file writes: `src/daemon/bundle.rs:93` (`handle`: name
  `[a-z0-9-]{1,40}` is a path component, 64-hex hash, https only; http only under the test-only
  `set_dirs`), `:163` (`ready`: `ensure_private_dir` 0700, sidecar match, 32-zip cap), `:197`
  (`place`: old sidecar removed before the rename so no moment pairs a hash with the wrong
  zip), `:210` (`download`: https-only redirects, 30 s connect / 120 s total, 64 MiB cap on
  `Content-Length` and on bytes read, `O_CREAT|O_EXCL|O_NOFOLLOW` 0600 temp file, SHA-256 before
  rename; the temp file is removed on every error path at `:176`). The per-chunk writes are
  blocking `std::fs` calls inside the async task (small chunks; accepted).
- Single flight: one `tokio::sync::Mutex` per plugin name (`Bundles::lock`); a waiter finds the
  zip cached. Replies go through the client's queue from a spawned task
  (`src/daemon/server.rs:561`), never inline; `BUNDLE_TIMEOUT` 250 s on the client
  (`src/client.rs:75`).
- Installed check: `src/daemon/bundle.rs:144` (1 MiB cap, regular file, `plugins` keys only;
  anything unreadable is "not installed").
- `--plugin-dir` drop: `src/daemon/session.rs:330`/`:347` (pairs and `=` form dropped when
  `Daemon::plugin_dir()` is false; the journal keeps the args); one cached `claude --help`
  probe for both flags (`src/daemon/mod.rs:461`, reset by `refresh_host`).
- Env admission and merge order: `src/proxy/plugins.rs:170` (`key_ok`: shape, denied prefixes
  and keys, case-insensitive), `:198` (`admit`: values ≤ 4 KiB, no C0 but `\n`/`\t`, ≤ 64 keys;
  reasons name keys only), `:115` (`checked`: ≤ 32, duplicates); `src/proxy/env.rs:112`
  (`merge`: plugin first, proxy wins, no key twice; also within plugins, first wins); applied
  on both spawn paths: warm (`src/tui/sessions.rs:479`, `merge_env`) and deferred
  (`src/tui/mod.rs:1505`).
- Secrets on disk and in output: `plugins.json` via `src/proxy/plugins.rs:314` (`write_cache`:
  `ensure_private_dir(config dir)` then `write_atomic` 0600); `Debug` of `Plugin`/`PluginList`
  (`:88`, `:102`), `login::Start`/`Granted` (device code and token redacted); notices and the
  extensions screen name keys and plugins only; `claudio proxy logout` drops the cache entry
  (`src/proxy/cmd.rs:607`); 401/403 drops it in the TUI (`src/tui/mod.rs:1632`).
- Spawn deferral timing: `src/tui/sessions.rs:453` (`plugin_bundles`: ready → `--plugin-dir`,
  installed/failed/too old → none, pending/unknown → wait), `src/tui/mod.rs:1587`
  (`wait_bundles`: every answer, even after the 20 s deadline, still reaches `App` through the
  reply channel; the proxy config is fetched concurrently and the cached one reused).
- Fail-safe on old daemons: an `Error "unsupported op…"` reply to `Bundle` becomes
  `BundleState::TooOld` (`src/tui/plugins.rs:190`), shown as `daemon too old`, never as the
  restart notice; the spawn then goes without the bundle (env still applies).
- Login: `src/proxy/login.rs:121` (`poll`: slow_down doubling, deadline, 404/410 expired, 403
  denied, network errors retried until the deadline), `src/proxy/cmd.rs:241`
  (`browser_login`: nothing from the proxy printed unfiltered, the browser only for http(s)).

## Task order and parallelism
| Task | Depends on | Repo/files | May run in parallel with |
|---|---|---|---|
| 1. plugin list, admission, cache | — | proxy/{plugins (new),env,mod}.rs, paths.rs | — |
| 2. daemon `Bundle` op | 1 | daemon/{bundle (new),mod,server,tests}.rs, proto.rs, client.rs | — |
| 3. daemon `--plugin-dir` probe and drop | 2 | daemon/{host,mod,server,session,tests}.rs | — |
| 4. registry, extensions screen, App plugin state | 3 | tui/{plugins (new),app,interaction,mod,extensions_view}.rs, tui/extensions/mod.rs, tests/manager.rs | — |
| 5. fetch, refresh, Bundle per host | 4 | tui/{mod,plugins}.rs, proxy/plugins.rs | — |
| 6. spawn: args, env, deferral, plain, reset | 5 | proto.rs, proxy/api.rs, tui/{app,mod,plugins,sessions}.rs | — |
| 7. browser login | 6 | proxy/{login (new),cmd,mod,profile,plugins,env}.rs, cli.rs | — |
| 8. manager test (AC3), remote test (AC6) | 7 | tests/{manager,remote}.rs, remote/diag.rs, main.rs | — |
| 9. docs | 8 | README.md, docs/extensions.md, CLAUDE.md | — |

Tasks are one commit each and were built in this order; the patches apply only in this order
(later tasks edit lines earlier ones add), so nothing runs in parallel.

## Tasks

### Task 1: plugin list, admission and private cache
Model: standard   Repo: /volumes/repos/claudio   Files: src/proxy/plugins.rs (new), src/proxy/mod.rs, src/proxy/env.rs, src/paths.rs
Goal: the client-side plugin types, `admit`, the `GET /v1/claudio/plugins` fetch, the 0600 `plugins.json` cache and `env::merge` (AC1: admit, cache, merge).
Patch: .delivery/2026-11-06-session-plugins/patches/task-1.patch

Step 1: write the failing test
The tests are in the patch below: `src/proxy/plugins.rs` `mod tests` (`a_good_plugin_is_admitted`,
`admit_refuses_bad_names_versions_urls_hashes_and_ids`, `admit_refuses_keys_a_plugin_may_not_set` (every
denied prefix and key), `admit_refuses_bad_values_naming_only_the_key`, `a_list_refuses_duplicates_and_more_than_32`,
`parse_reads_the_proxy_reply_with_sorted_env`, `fetch_reads_the_list_and_reports_statuses` (axum fake server:
200, 401, 403, 404), `debug_and_summary_never_show_a_value`, `the_cache_keeps_one_private_entry_per_profile`
(0600 file, 0700 dir, per-profile entries, remove, broken file)) and `src/proxy/env.rs`
`merge_puts_plugins_first_and_lets_the_proxy_win`. Write them first, with `pub mod plugins;` in
`src/proxy/mod.rs` and a `plugins.rs` holding only the test module and the `sample` fixture.
Run: `cargo test --locked --bin claudio proxy::`
Expected: the build fails (`error[E0425]`/`E0433`/`E0422`: `admit`, `Plugin`, `PluginList`, `merge` … not found).

Step 2: implement
Apply the patch (it holds the tests above and the implementation):
`/usr/bin/git apply .delivery/2026-11-06-session-plugins/patches/task-1.patch`

~~~~~~diff
diff --git a/src/paths.rs b/src/paths.rs
index 85e7f4f..318d68b 100644
--- a/src/paths.rs
+++ b/src/paths.rs
@@ -82,6 +82,21 @@ pub fn config_file() -> PathBuf {
     config_dir().join("config.toml")
 }
 
+/// The proxy plugins of each profile, with their env (secrets: 0600).
+pub fn plugins_cache() -> PathBuf {
+    config_dir().join("plugins.json")
+}
+
+/// `$XDG_CACHE_HOME/claudio`, else `~/.cache/claudio`: downloads this host
+/// keeps (the plugin bundles).
+pub fn cache_dir() -> PathBuf {
+    std::env::var_os("XDG_CACHE_HOME")
+        .filter(|v| !v.is_empty())
+        .map(PathBuf::from)
+        .unwrap_or_else(|| home().join(".cache"))
+        .join("claudio")
+}
+
 /// Create `dir` (mode 0700) if needed and verify it is a real directory owned
 /// by us and private — the runtime dir may live in a shared `/tmp`.
 pub fn ensure_private_dir(dir: &Path) -> io::Result<()> {
diff --git a/src/proxy/env.rs b/src/proxy/env.rs
index 5ee9dca..0756097 100644
--- a/src/proxy/env.rs
+++ b/src/proxy/env.rs
@@ -105,6 +105,22 @@ pub fn session_env(profile: &Profile, config: Option<&ConfigResponse>) -> Vec<(S
     env
 }
 
+/// A session's env from the plugins' env (`plugin`, in registry order) and
+/// the proxy's (`proxy`, [`session_env`]): the plugin pairs whose key the
+/// proxy does not set, the first of a key repeated among plugins, then the
+/// proxy's. The proxy wins, and no key appears twice.
+pub fn merge(plugin: &[(String, String)], proxy: Vec<(String, String)>) -> Vec<(String, String)> {
+    let mut out: Vec<(String, String)> = Vec::with_capacity(plugin.len() + proxy.len());
+    for (k, v) in plugin {
+        let taken = proxy.iter().chain(out.iter()).any(|(p, _)| p == k);
+        if !taken {
+            out.push((k.clone(), v.clone()));
+        }
+    }
+    out.extend(proxy);
+    out
+}
+
 /// How to alter the environment a `claude` process inherits.
 ///
 /// Returned by [`env_diff`] so every launcher (the daemon's PTY builder,
@@ -357,6 +373,30 @@ mod tests {
             .contains(&"CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"));
     }
 
+    #[test]
+    fn merge_puts_plugins_first_and_lets_the_proxy_win() {
+        let pair = |k: &str, v: &str| (k.to_owned(), v.to_owned());
+        let plugin = [
+            pair("KB_TOKEN", "kb"),
+            pair("API_TIMEOUT_MS", "1"),
+            pair("KB_TOKEN", "x"),
+        ];
+        let proxy = vec![
+            pair("ANTHROPIC_AUTH_TOKEN", "tok"),
+            pair("API_TIMEOUT_MS", "30000"),
+        ];
+        assert_eq!(
+            merge(&plugin, proxy.clone()),
+            [
+                pair("KB_TOKEN", "kb"),
+                pair("ANTHROPIC_AUTH_TOKEN", "tok"),
+                pair("API_TIMEOUT_MS", "30000"),
+            ]
+        );
+        assert_eq!(merge(&[], proxy.clone()), proxy);
+        assert_eq!(merge(&plugin[..1], Vec::new()), [pair("KB_TOKEN", "kb")]);
+    }
+
     #[test]
     fn neither_mode_sets_the_auto_compact_window() {
         // claude clamps it to the window it believes the model has, so it
diff --git a/src/proxy/mod.rs b/src/proxy/mod.rs
index 995e519..35e6c9b 100644
--- a/src/proxy/mod.rs
+++ b/src/proxy/mod.rs
@@ -6,6 +6,7 @@
 pub mod api;
 pub mod cmd;
 pub mod env;
+pub mod plugins;
 pub mod profile;
 pub mod resolve;
 
diff --git a/src/proxy/plugins.rs b/src/proxy/plugins.rs
new file mode 100644
index 0000000..068b20a
--- /dev/null
+++ b/src/proxy/plugins.rs
@@ -0,0 +1,646 @@
+//! Session plugins served by a proxy profile (`GET /v1/claudio/plugins`).
+//!
+//! A plugin is a pinned bundle (a zip claude loads with `--plugin-dir`) and
+//! the env its MCP servers and hooks need. The env holds secrets: the list
+//! is cached per profile in `~/.config/claudio/plugins.json` (mode 0600,
+//! like `config.toml`), so a session can get it when the proxy is
+//! unreachable, and never goes to argv, the journal, `state.json`, logs or
+//! notices. `Debug` prints names and key counts only.
+//!
+//! Everything a proxy sends is checked by [`admit`] before it is used.
+
+use std::collections::BTreeMap;
+use std::fmt;
+use std::io;
+use std::path::Path;
+
+use serde::{Deserialize, Serialize};
+
+use super::api::{build_client, ApiError};
+use super::profile::Profile;
+use crate::paths;
+
+/// At most this many plugins per list are used.
+pub const MAX_PLUGINS: usize = 32;
+/// At most this many env keys per plugin.
+pub const MAX_ENV_KEYS: usize = 64;
+/// One env value's cap, in bytes.
+pub const MAX_ENV_VALUE: usize = 4096;
+
+/// Env key prefixes a plugin may not set: the proxy's own and Claude Code's
+/// switches, the loaders and the toolchains that read code from the env.
+const DENIED_PREFIXES: &[&str] = &[
+    "ANTHROPIC_",
+    "CLAUDIO_",
+    "CLAUDE_",
+    "NODE_",
+    "LD_",
+    "DYLD_",
+    "GIT_",
+    "SSL_",
+    "XDG_",
+    "SSH_",
+];
+
+/// Env keys a plugin may not set (nor their lowercase forms).
+const DENIED_KEYS: &[&str] = &[
+    "PATH",
+    "HOME",
+    "SHELL",
+    "TERM",
+    "USER",
+    "LOGNAME",
+    "TMPDIR",
+    "BASH_ENV",
+    "ENV",
+    "PYTHONPATH",
+    "PERL5OPT",
+    "HTTP_PROXY",
+    "HTTPS_PROXY",
+    "ALL_PROXY",
+    "NO_PROXY",
+];
+
+/// One plugin of a profile.
+#[derive(Clone, PartialEq, Serialize, Deserialize)]
+pub struct Plugin {
+    pub name: String,
+    pub version: String,
+    pub bundle_url: String,
+    pub bundle_sha256: String,
+    /// The Claude Code plugin id (`<plugin>@<marketplace>`) that makes the
+    /// bundle redundant when installed; empty when none does.
+    #[serde(default)]
+    pub installed_id: String,
+    /// Sorted by key.
+    #[serde(default)]
+    pub env: Vec<(String, String)>,
+}
+
+/// A profile's plugins as fetched at `fetched_at` (Unix seconds).
+#[derive(Clone, PartialEq, Serialize, Deserialize)]
+pub struct PluginList {
+    pub profile: String,
+    pub fetched_at: u64,
+    pub plugins: Vec<Plugin>,
+}
+
+impl fmt::Debug for Plugin {
+    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
+        let keys: Vec<&str> = self.env.iter().map(|(k, _)| k.as_str()).collect();
+        f.debug_struct("Plugin")
+            .field("name", &self.name)
+            .field("version", &self.version)
+            .field("bundle_url", &self.bundle_url)
+            .field("bundle_sha256", &self.bundle_sha256)
+            .field("installed_id", &self.installed_id)
+            .field("env", &keys)
+            .finish()
+    }
+}
+
+impl fmt::Debug for PluginList {
+    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
+        f.debug_struct("PluginList")
+            .field("profile", &self.profile)
+            .field("fetched_at", &self.fetched_at)
+            .field("plugins", &self.plugins)
+            .finish()
+    }
+}
+
+impl PluginList {
+    /// Each plugin with why it is refused (`None`: admitted), in order. Past
+    /// [`MAX_PLUGINS`], and a name given twice, are refused too.
+    pub fn checked(&self) -> Vec<(&Plugin, Option<String>)> {
+        self.plugins
+            .iter()
+            .enumerate()
+            .map(|(i, p)| {
+                let why = if i >= MAX_PLUGINS {
+                    Some(format!("more than {MAX_PLUGINS} plugins"))
+                } else if self.plugins[..i].iter().any(|q| q.name == p.name) {
+                    Some("name given twice".to_owned())
+                } else {
+                    admit(p).err()
+                };
+                (p, why)
+            })
+            .collect()
+    }
+
+    /// The admitted plugins, in order.
+    pub fn admitted(&self) -> Vec<&Plugin> {
+        let checked = self.checked();
+        checked
+            .into_iter()
+            .filter(|(_, why)| why.is_none())
+            .map(|(p, _)| p)
+            .collect()
+    }
+
+    /// What the fetch log says: names and env key counts, never a value.
+    pub fn summary(&self) -> String {
+        let parts: Vec<String> = self
+            .plugins
+            .iter()
+            .map(|p| format!("{} ({} keys)", p.name, p.env.len()))
+            .collect();
+        parts.join(", ")
+    }
+}
+
+/// `[a-z0-9-]{1,40}`: a plugin name, and a path component on every host.
+pub fn name_ok(name: &str) -> bool {
+    (1..=40).contains(&name.len())
+        && name
+            .bytes()
+            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
+}
+
+/// 64 lowercase hex digits.
+pub fn sha256_ok(hash: &str) -> bool {
+    hash.len() == 64
+        && hash
+            .bytes()
+            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
+}
+
+/// `[A-Z_][A-Z0-9_]{0,63}`, and not one a plugin may not set.
+fn key_ok(key: &str) -> Result<(), String> {
+    let mut bytes = key.bytes();
+    let shape = key.len() <= 64
+        && bytes
+            .next()
+            .is_some_and(|b| b.is_ascii_uppercase() || b == b'_')
+        && bytes.all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
+    if !shape {
+        return Err("bad env key".to_owned());
+    }
+    let upper = key.to_ascii_uppercase();
+    if DENIED_PREFIXES.iter().any(|p| upper.starts_with(p)) || DENIED_KEYS.contains(&&*upper) {
+        return Err(format!("env key {key} not allowed"));
+    }
+    Ok(())
+}
+
+/// `''` or `[a-z0-9-]{1,40}@[a-z0-9-]{1,40}`.
+fn installed_id_ok(id: &str) -> bool {
+    id.is_empty()
+        || id
+            .split_once('@')
+            .is_some_and(|(p, m)| name_ok(p) && name_ok(m))
+}
+
+/// Why plugin `p` may not be used: a bad name, version, URL, hash or
+/// installed id, or an env key or value it may not set. The reason names
+/// the key, never a value.
+pub fn admit(p: &Plugin) -> Result<(), String> {
+    if !name_ok(&p.name) {
+        return Err("bad name".to_owned());
+    }
+    let printable = |s: &str| s.bytes().all(|b| (0x20..0x7f).contains(&b));
+    if !(1..=64).contains(&p.version.len()) || !printable(&p.version) {
+        return Err("bad version".to_owned());
+    }
+    if !p.bundle_url.starts_with("https://") || !printable(&p.bundle_url) {
+        return Err("bundle URL is not https".to_owned());
+    }
+    if !sha256_ok(&p.bundle_sha256) {
+        return Err("bad bundle hash".to_owned());
+    }
+    if !installed_id_ok(&p.installed_id) {
+        return Err("bad installed id".to_owned());
+    }
+    if p.env.len() > MAX_ENV_KEYS {
+        return Err(format!("more than {MAX_ENV_KEYS} env keys"));
+    }
+    for (key, value) in &p.env {
+        key_ok(key)?;
+        if value.len() > MAX_ENV_VALUE {
+            return Err(format!("env {key}: value over {MAX_ENV_VALUE} bytes"));
+        }
+        if value.bytes().any(|b| b < 0x20 && b != b'\n' && b != b'\t') {
+            return Err(format!("env {key}: control character in value"));
+        }
+    }
+    Ok(())
+}
+
+// ── Fetch ─────────────────────────────────────────────────────────────────────
+
+/// `GET /v1/claudio/plugins`, as the proxy sends it.
+#[derive(Deserialize)]
+struct Reply {
+    #[serde(default)]
+    plugins: Vec<ReplyPlugin>,
+}
+
+#[derive(Deserialize)]
+struct ReplyPlugin {
+    #[serde(default)]
+    name: String,
+    #[serde(default)]
+    version: String,
+    #[serde(default)]
+    bundle: ReplyBundle,
+    #[serde(default)]
+    installed_id: String,
+    #[serde(default)]
+    env: BTreeMap<String, String>,
+}
+
+#[derive(Default, Deserialize)]
+struct ReplyBundle {
+    #[serde(default)]
+    url: String,
+    #[serde(default)]
+    sha256: String,
+}
+
+/// The list in a `/v1/claudio/plugins` body, for profile `profile` at `now`.
+pub fn parse(body: &[u8], profile: &str, now: u64) -> Result<PluginList, String> {
+    let reply: Reply = serde_json::from_slice(body).map_err(|_| "bad plugins reply".to_owned())?;
+    Ok(list_of(reply, profile, now))
+}
+
+fn list_of(reply: Reply, profile: &str, now: u64) -> PluginList {
+    let plugins = reply
+        .plugins
+        .into_iter()
+        .map(|p| Plugin {
+            name: p.name,
+            version: p.version,
+            bundle_url: p.bundle.url,
+            bundle_sha256: p.bundle.sha256,
+            installed_id: p.installed_id,
+            env: p.env.into_iter().collect(),
+        })
+        .collect();
+    PluginList {
+        profile: profile.to_owned(),
+        fetched_at: now,
+        plugins,
+    }
+}
+
+/// Fetch `profile`'s plugins (`name` is the profile's name). A 401 or 403
+/// is [`ApiError::Http`] with that status (signed out); a 404 too (an
+/// older proxy).
+pub async fn fetch(name: &str, profile: &Profile) -> Result<PluginList, ApiError> {
+    let resp = build_client()?
+        .get(format!("{}/v1/claudio/plugins", profile.url))
+        .bearer_auth(&profile.token)
+        .send()
+        .await?;
+    if !resp.status().is_success() {
+        return Err(ApiError::Http(resp.status()));
+    }
+    let reply: Reply = resp.json().await?;
+    Ok(list_of(reply, name, paths::unix_now()))
+}
+
+// ── Cache ─────────────────────────────────────────────────────────────────────
+
+/// `plugins.json`: one entry per profile name.
+#[derive(Default, Serialize, Deserialize)]
+struct CacheFile {
+    #[serde(default)]
+    profiles: BTreeMap<String, PluginList>,
+}
+
+fn read_cache(path: &Path) -> CacheFile {
+    std::fs::read(path)
+        .ok()
+        .and_then(|raw| serde_json::from_slice(&raw).ok())
+        .unwrap_or_default()
+}
+
+fn write_cache(path: &Path, cache: &CacheFile) -> io::Result<()> {
+    if let Some(dir) = path.parent() {
+        paths::ensure_private_dir(dir)?;
+    }
+    let raw = serde_json::to_vec_pretty(cache).map_err(io::Error::other)?;
+    paths::write_atomic(path, &raw)
+}
+
+/// `profile`'s cached list, if any.
+pub fn load_cache(profile: &str) -> Option<PluginList> {
+    load_cache_from(&paths::plugins_cache(), profile)
+}
+
+/// Every cached list (a missing or unreadable file has none).
+pub fn load_all() -> Vec<PluginList> {
+    load_all_from(&paths::plugins_cache())
+}
+
+/// Store `list` as its profile's entry.
+pub fn save_cache(list: &PluginList) -> io::Result<()> {
+    save_cache_to(&paths::plugins_cache(), list)
+}
+
+/// Drop `profile`'s entry (its secrets go with it).
+pub fn remove_cache(profile: &str) -> io::Result<()> {
+    remove_cache_from(&paths::plugins_cache(), profile)
+}
+
+pub fn load_cache_from(path: &Path, profile: &str) -> Option<PluginList> {
+    read_cache(path).profiles.remove(profile)
+}
+
+pub fn load_all_from(path: &Path) -> Vec<PluginList> {
+    read_cache(path).profiles.into_values().collect()
+}
+
+pub fn save_cache_to(path: &Path, list: &PluginList) -> io::Result<()> {
+    let mut cache = read_cache(path);
+    cache.profiles.insert(list.profile.clone(), list.clone());
+    write_cache(path, &cache)
+}
+
+pub fn remove_cache_from(path: &Path, profile: &str) -> io::Result<()> {
+    let mut cache = read_cache(path);
+    if cache.profiles.remove(profile).is_none() {
+        return Ok(());
+    }
+    write_cache(path, &cache)
+}
+
+/// A good plugin for tests: one env key `KB_TOKEN` = `s3cret`.
+#[cfg(test)]
+pub fn sample(name: &str) -> Plugin {
+    Plugin {
+        name: name.into(),
+        version: "1.4.0".into(),
+        bundle_url: format!("https://example.org/{name}.zip"),
+        bundle_sha256: "a".repeat(64),
+        installed_id: format!("{name}@vocdoni"),
+        env: vec![("KB_TOKEN".into(), "s3cret".into())],
+    }
+}
+
+#[cfg(test)]
+mod tests {
+    use super::*;
+    use std::os::unix::fs::PermissionsExt;
+
+    fn plugin(name: &str) -> Plugin {
+        sample(name)
+    }
+
+    fn refused(p: &Plugin) -> String {
+        admit(p).unwrap_err()
+    }
+
+    #[test]
+    fn a_good_plugin_is_admitted() {
+        assert_eq!(admit(&plugin("vocdoni-internal")), Ok(()));
+        let mut p = plugin("x");
+        p.installed_id = String::new();
+        p.env.push(("MULTI".into(), "a\nb\tc".into()));
+        assert_eq!(admit(&p), Ok(()));
+    }
+
+    #[test]
+    fn admit_refuses_bad_names_versions_urls_hashes_and_ids() {
+        for name in ["", "Up", "a/b", "..", "a_b", &"n".repeat(41)] {
+            let p = Plugin {
+                name: name.into(),
+                ..plugin("x")
+            };
+            assert_eq!(refused(&p), "bad name", "{name:?}");
+        }
+        for version in ["", "1\n2", &"9".repeat(65)] {
+            let p = Plugin {
+                version: version.into(),
+                ..plugin("x")
+            };
+            assert_eq!(refused(&p), "bad version", "{version:?}");
+        }
+        for url in [
+            "http://example.org/x.zip",
+            "file:///x.zip",
+            "https://e.org/\nx",
+        ] {
+            let p = Plugin {
+                bundle_url: url.into(),
+                ..plugin("x")
+            };
+            assert_eq!(refused(&p), "bundle URL is not https", "{url:?}");
+        }
+        for hash in ["A".repeat(64), "a".repeat(63), "g".repeat(64)] {
+            let p = Plugin {
+                bundle_sha256: hash.clone(),
+                ..plugin("x")
+            };
+            assert_eq!(refused(&p), "bad bundle hash", "{hash:?}");
+        }
+        for id in ["x", "x@", "@m", "X@m", "a@b@c"] {
+            let p = Plugin {
+                installed_id: id.into(),
+                ..plugin("x")
+            };
+            assert_eq!(refused(&p), "bad installed id", "{id:?}");
+        }
+    }
+
+    #[test]
+    fn admit_refuses_keys_a_plugin_may_not_set() {
+        let with_key = |key: &str| Plugin {
+            env: vec![(key.into(), "v".into())],
+            ..plugin("x")
+        };
+        for key in ["", "lower", "1ABC", "A-B", &"K".repeat(65)] {
+            assert_eq!(refused(&with_key(key)), "bad env key", "{key:?}");
+        }
+        for key in [
+            "ANTHROPIC_API_KEY",
+            "CLAUDIO_X",
+            "CLAUDE_CONFIG_DIR",
+            "NODE_OPTIONS",
+            "LD_PRELOAD",
+            "DYLD_INSERT_LIBRARIES",
+            "GIT_SSH_COMMAND",
+            "SSL_CERT_FILE",
+            "XDG_CONFIG_HOME",
+            "SSH_AUTH_SOCK",
+            "PATH",
+            "HOME",
+            "SHELL",
+            "TERM",
+            "USER",
+            "LOGNAME",
+            "TMPDIR",
+            "BASH_ENV",
+            "ENV",
+            "PYTHONPATH",
+            "PERL5OPT",
+            "HTTP_PROXY",
+            "HTTPS_PROXY",
+            "ALL_PROXY",
+            "NO_PROXY",
+        ] {
+            assert_eq!(
+                refused(&with_key(key)),
+                format!("env key {key} not allowed"),
+                "{key}"
+            );
+        }
+        // Allowed: close to a denied one but not it.
+        assert_eq!(admit(&with_key("PATHS")), Ok(()));
+        assert_eq!(admit(&with_key("ENVIRONMENT")), Ok(()));
+    }
+
+    #[test]
+    fn admit_refuses_bad_values_naming_only_the_key() {
+        let with_value = |v: String| Plugin {
+            env: vec![("TOKEN".into(), v)],
+            ..plugin("x")
+        };
+        let long = refused(&with_value("v".repeat(MAX_ENV_VALUE + 1)));
+        assert_eq!(long, "env TOKEN: value over 4096 bytes");
+        assert_eq!(admit(&with_value("v".repeat(MAX_ENV_VALUE))), Ok(()));
+        for bad in ["se\0cret", "se\u{1b}[31mcret", "se\rcret"] {
+            let why = refused(&with_value(bad.into()));
+            assert_eq!(why, "env TOKEN: control character in value");
+            assert!(!why.contains("se"), "no value in the reason");
+        }
+        let many = Plugin {
+            env: (0..=MAX_ENV_KEYS)
+                .map(|i| (format!("K{i}"), "v".into()))
+                .collect(),
+            ..plugin("x")
+        };
+        assert_eq!(refused(&many), "more than 64 env keys");
+    }
+
+    #[test]
+    fn a_list_refuses_duplicates_and_more_than_32() {
+        let mut plugins: Vec<Plugin> = (0..MAX_PLUGINS + 1)
+            .map(|i| plugin(&format!("p{i}")))
+            .collect();
+        plugins[1] = plugin("p0");
+        plugins[2].bundle_url = "http://x".into();
+        let list = PluginList {
+            profile: "work".into(),
+            fetched_at: 1,
+            plugins,
+        };
+        let checked = list.checked();
+        assert_eq!(checked[0].1, None);
+        assert_eq!(checked[1].1.as_deref(), Some("name given twice"));
+        assert_eq!(checked[2].1.as_deref(), Some("bundle URL is not https"));
+        assert_eq!(
+            checked[MAX_PLUGINS].1.as_deref(),
+            Some("more than 32 plugins")
+        );
+        assert_eq!(list.admitted().len(), MAX_PLUGINS - 2);
+    }
+
+    #[test]
+    fn parse_reads_the_proxy_reply_with_sorted_env() {
+        let body = br#"{"plugins":[{"name":"vocdoni-internal","version":"1.4.0",
+            "bundle":{"url":"https://e.org/v.zip","sha256":"ab"},
+            "installed_id":"vocdoni-internal@vocdoni",
+            "env":{"ZED":"z","KB_TOKEN":"k"}}]}"#;
+        let list = parse(body, "work", 7).unwrap();
+        assert_eq!(list.profile, "work");
+        assert_eq!(list.fetched_at, 7);
+        let p = &list.plugins[0];
+        assert_eq!(p.bundle_url, "https://e.org/v.zip");
+        assert_eq!(p.bundle_sha256, "ab");
+        assert_eq!(
+            p.env,
+            [
+                ("KB_TOKEN".to_owned(), "k".to_owned()),
+                ("ZED".to_owned(), "z".to_owned())
+            ]
+        );
+        assert!(parse(b"not json", "work", 7).is_err());
+        assert!(parse(b"{}", "work", 7).unwrap().plugins.is_empty());
+    }
+
+    #[tokio::test]
+    async fn fetch_reads_the_list_and_reports_statuses() {
+        use axum::http::StatusCode;
+        use axum::routing::get;
+        async fn serve(router: axum::Router) -> String {
+            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
+            let port = listener.local_addr().unwrap().port();
+            tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
+            format!("http://127.0.0.1:{port}")
+        }
+        let profile = |url: String| Profile {
+            url,
+            token: "tok".into(),
+            first_party: false,
+        };
+        let ok = axum::Router::new().route(
+            "/v1/claudio/plugins",
+            get(|headers: axum::http::HeaderMap| async move {
+                assert_eq!(headers["authorization"], "Bearer tok");
+                axum::Json(
+                    serde_json::json!({"plugins": [{"name": "kb", "version": "1",
+                    "bundle": {"url": "https://e.org/kb.zip", "sha256": "a".repeat(64)},
+                    "env": {"KB_TOKEN": "k"}}]}),
+                )
+            }),
+        );
+        let list = fetch("work", &profile(serve(ok).await)).await.unwrap();
+        assert_eq!(list.profile, "work");
+        assert_eq!(list.admitted().len(), 1);
+        for status in [
+            StatusCode::UNAUTHORIZED,
+            StatusCode::FORBIDDEN,
+            StatusCode::NOT_FOUND,
+        ] {
+            let router = axum::Router::new()
+                .route("/v1/claudio/plugins", get(move || async move { status }));
+            match fetch("work", &profile(serve(router).await)).await {
+                Err(ApiError::Http(s)) => assert_eq!(s.as_u16(), status.as_u16()),
+                other => panic!("{status}: {other:?}"),
+            }
+        }
+    }
+
+    #[test]
+    fn debug_and_summary_never_show_a_value() {
+        let list = PluginList {
+            profile: "work".into(),
+            fetched_at: 1,
+            plugins: vec![plugin("vocdoni-internal")],
+        };
+        let dbg = format!("{list:?}");
+        assert!(dbg.contains("KB_TOKEN") && !dbg.contains("s3cret"), "{dbg}");
+        assert_eq!(list.summary(), "vocdoni-internal (1 keys)");
+    }
+
+    #[test]
+    fn the_cache_keeps_one_private_entry_per_profile() {
+        let dir = std::env::temp_dir().join(format!("claudio-plugins-{}", uuid::Uuid::new_v4()));
+        let path = dir.join("cfg/plugins.json");
+        assert_eq!(load_cache_from(&path, "work"), None);
+        let list = |profile: &str, name: &str| PluginList {
+            profile: profile.into(),
+            fetched_at: 5,
+            plugins: vec![plugin(name)],
+        };
+        save_cache_to(&path, &list("work", "a")).unwrap();
+        save_cache_to(&path, &list("home", "b")).unwrap();
+        save_cache_to(&path, &list("work", "c")).unwrap();
+        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
+        assert_eq!(mode(&path), 0o600, "plugins.json holds secrets");
+        assert_eq!(mode(path.parent().unwrap()), 0o700);
+        assert_eq!(load_cache_from(&path, "work"), Some(list("work", "c")));
+        assert_eq!(load_cache_from(&path, "home"), Some(list("home", "b")));
+        assert_eq!(load_all_from(&path).len(), 2);
+        remove_cache_from(&path, "work").unwrap();
+        assert_eq!(load_cache_from(&path, "work"), None);
+        assert_eq!(load_cache_from(&path, "home"), Some(list("home", "b")));
+        remove_cache_from(&path, "nope").unwrap();
+        // A broken file is no cache.
+        std::fs::write(&path, b"{").unwrap();
+        assert_eq!(load_cache_from(&path, "home"), None);
+        std::fs::remove_dir_all(&dir).unwrap();
+    }
+}
~~~~~~

Run: `rustfmt --edition 2021 --check src/proxy/plugins.rs src/proxy/env.rs src/paths.rs` → no output.
Run: `umask 022; cargo test --locked --bin claudio proxy::plugins`
Expected: `test result: ok. 9 passed; 0 failed`
Run: `umask 022; cargo test --locked --bin claudio`
Expected: `test result: ok. 1013 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`
The dev build has 33 dead-code warnings, all on items this patch adds (`plugins.rs`,
`env::merge`, `paths::{plugins_cache,cache_dir}`); Tasks 2–7 use them. No other clippy finding
on added lines.

Step 3: commit
`/usr/bin/git add src/proxy/plugins.rs`
`/usr/bin/git commit -m "feat(proxy): plugin list, admission and private cache" -- src/proxy/plugins.rs src/proxy/mod.rs src/proxy/env.rs src/paths.rs`

Interfaces this task provides to later tasks:
- `crate::proxy::plugins::{Plugin, PluginList, admit, name_ok, sha256_ok, fetch(name, &Profile) -> Result<PluginList, ApiError>, load_cache, load_all, save_cache, remove_cache, MAX_PLUGINS}`; `PluginList::{checked, admitted, summary}`; `#[cfg(test)] plugins::sample(name)`.
- `crate::proxy::env::merge(plugin: &[(String, String)], proxy: Vec<(String, String)>) -> Vec<(String, String)>`.
- `crate::paths::{plugins_cache(), cache_dir()}`.


### Task 2: the daemon's `Bundle` op
Model: strong   Repo: /volumes/repos/claudio   Files: src/daemon/bundle.rs (new), src/daemon/mod.rs, src/daemon/server.rs, src/daemon/tests.rs, src/proto.rs, src/client.rs
Goal: `Msg::Bundle`/`Msg::Bundled` and the daemon's validated, hash-checked, single-flight bundle cache (AC2).
Patch: .delivery/2026-11-06-session-plugins/patches/task-2.patch

Step 1: write the failing test
The tests are in the patch below: `src/proto.rs` `bundle_and_bundled_roundtrip_and_are_unknown_to_older_peers`,
and in `src/daemon/tests.rs` (section `// ── Bundle ──`): `bundle_refuses_a_bad_name_hash_or_url`,
`bundle_is_skipped_when_claude_code_has_the_plugin_installed`, `a_cached_zip_with_its_sidecar_is_reused`
(a URL nothing listens on), `a_download_is_verified_stored_privately_and_shared` (two clients at once, one
download, 0700 dir, sidecar), `a_wrong_hash_is_an_error_and_leaves_no_file` (and a new version replaces the
old zip in place), `the_cache_keeps_at_most_32_bundles`. Apply the test file alone first:
`/usr/bin/git apply --include='src/daemon/tests.rs' .delivery/2026-11-06-session-plugins/patches/task-2.patch`
Run: `cargo test --locked --bin claudio daemon::tests`
Expected: the build fails with ``error[E0599]: no variant named `Bundle` found for enum `proto::Msg` ``, ``no variant named `Bundled` ``, ``error[E0609]: no field `bundles` on type `std::sync::Arc<Daemon>` `` and ``error[E0433]: … could not find `bundle` in `super` ``. Undo that file
(`/usr/bin/git restore src/daemon/tests.rs`) before Step 2.

Step 2: implement
`/usr/bin/git apply .delivery/2026-11-06-session-plugins/patches/task-2.patch`

~~~~~~diff
diff --git a/src/client.rs b/src/client.rs
index a26db7e..7a3d752 100644
--- a/src/client.rs
+++ b/src/client.rs
@@ -70,6 +70,9 @@ const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
 const SPAWN_TIMEOUT: Duration = Duration::from_secs(60);
 /// `UpdateClaude` runs up to 300 s on the daemon, plus the re-probe.
 const UPDATE_TIMEOUT: Duration = Duration::from_secs(330);
+/// A `Bundle` download may take the daemon's 120 s, after waiting for one
+/// already running.
+const BUNDLE_TIMEOUT: Duration = Duration::from_secs(250);
 
 /// Something the daemon sent without being asked.
 #[derive(Debug, PartialEq)]
@@ -399,6 +402,7 @@ impl Client {
         let timeout = match msg {
             Msg::Spawn(_) | Msg::SpawnShell(_) => SPAWN_TIMEOUT,
             Msg::UpdateClaude { .. } => UPDATE_TIMEOUT,
+            Msg::Bundle { .. } => BUNDLE_TIMEOUT,
             _ => REQUEST_TIMEOUT,
         };
         let queued = self.enqueue(msg);
diff --git a/src/daemon/bundle.rs b/src/daemon/bundle.rs
new file mode 100644
index 0000000..cdcae31
--- /dev/null
+++ b/src/daemon/bundle.rs
@@ -0,0 +1,267 @@
+//! `Bundle`: a proxy plugin's zip on this host, for claude's `--plugin-dir`.
+//!
+//! The zips live in `<cache>/plugins` (0700): `<name>.zip` and its sidecar
+//! `<name>.sha256`, one pair per plugin name, so a journaled
+//! `--plugin-dir <path>` stays valid across versions. claudio never unpacks
+//! a zip; the hash pins the exact file the proxy's admin registered.
+//!
+//! Every request is checked here (the name is a path component). A download
+//! is https only, capped at [`MAX_BYTES`] and [`TOTAL_TIMEOUT`], streamed to
+//! a temp file in the cache dir, verified, then renamed into place; any
+//! failure leaves no partial file. One download per plugin name runs at a
+//! time: a second request waits for it and then finds the zip cached.
+
+use std::collections::HashMap;
+use std::io::Write;
+use std::os::unix::fs::OpenOptionsExt;
+use std::path::{Path, PathBuf};
+use std::sync::{Arc, Mutex};
+use std::time::Duration;
+
+use sha2::{Digest, Sha256};
+
+use crate::paths;
+use crate::proto::Msg;
+use crate::proxy::plugins::{name_ok, sha256_ok};
+
+/// A bundle's size cap.
+pub const MAX_BYTES: u64 = 64 << 20;
+/// At most this many zips are kept.
+pub const MAX_BUNDLES: usize = 32;
+const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
+const TOTAL_TIMEOUT: Duration = Duration::from_secs(120);
+/// `installed_plugins.json` is read up to this size.
+const MAX_INSTALLED: u64 = 1 << 20;
+
+/// Where bundles go and where Claude Code keeps its plugins, and the
+/// per-name download locks.
+pub(crate) struct Bundles {
+    dirs: Mutex<Dirs>,
+    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
+}
+
+#[derive(Clone)]
+struct Dirs {
+    /// `<cache>/plugins`.
+    cache: PathBuf,
+    /// `$CLAUDE_CONFIG_DIR`, else `~/.claude`.
+    claude_config: PathBuf,
+    /// Tests serve bundles over plain http on loopback.
+    allow_http: bool,
+}
+
+impl Bundles {
+    /// The standard places: [`paths::cache_dir`] and the Claude Code config
+    /// dir the sessions see (this daemon's `CLAUDE_CONFIG_DIR`, which every
+    /// session inherits).
+    pub(crate) fn new() -> Bundles {
+        let claude_config = std::env::var_os("CLAUDE_CONFIG_DIR")
+            .filter(|v| !v.is_empty())
+            .map(PathBuf::from)
+            .unwrap_or_else(|| paths::home().join(".claude"));
+        Bundles {
+            dirs: Mutex::new(Dirs {
+                cache: paths::cache_dir().join("plugins"),
+                claude_config,
+                allow_http: false,
+            }),
+            locks: Mutex::new(HashMap::new()),
+        }
+    }
+
+    fn dirs(&self) -> Dirs {
+        self.dirs.lock().unwrap_or_else(|e| e.into_inner()).clone()
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
+    fn lock(&self, name: &str) -> Arc<tokio::sync::Mutex<()>> {
+        let mut locks = self.locks.lock().unwrap_or_else(|e| e.into_inner());
+        Arc::clone(locks.entry(name.to_owned()).or_default())
+    }
+}
+
+/// Answer a `Bundle` request: `Bundled`, or `Error "bundle <name>: …"`.
+pub(crate) async fn handle(
+    bundles: &Bundles,
+    name: String,
+    url: String,
+    sha256: String,
+    installed_id: String,
+) -> Msg {
+    let dirs = bundles.dirs();
+    let scheme_ok = url.starts_with("https://") || (dirs.allow_http && url.starts_with("http://"));
+    if !name_ok(&name) || !sha256_ok(&sha256) || !scheme_ok {
+        return Msg::Error {
+            message: format!("bundle {}: bad request", safe_name(&name)),
+        };
+    }
+    if !installed_id.is_empty() && is_installed(&dirs.claude_config, &installed_id).await {
+        return Msg::Bundled {
+            name,
+            path: None,
+            installed: true,
+        };
+    }
+    // One download per name at a time; whoever waited finds it cached.
+    let lock = bundles.lock(&name);
+    let _held = lock.lock().await;
+    match ready(&dirs, &name, &url, &sha256).await {
+        Ok(path) => Msg::Bundled {
+            name,
+            path: Some(path.to_string_lossy().into_owned()),
+            installed: false,
+        },
+        Err(why) => {
+            tracing::warn!(bundle = %name, error = %why, "bundle not ready");
+            Msg::Error {
+                message: format!("bundle {name}: {why}"),
+            }
+        }
+    }
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
+/// Whether Claude Code has plugin `id` installed: a key of `plugins` in
+/// `<claude_config>/plugins/installed_plugins.json`. A missing, oversized
+/// or unparsable file counts as not installed.
+async fn is_installed(claude_config: &Path, id: &str) -> bool {
+    let file = claude_config.join("plugins/installed_plugins.json");
+    let id = id.to_owned();
+    tokio::task::spawn_blocking(move || {
+        let meta = std::fs::metadata(&file).ok()?;
+        if !meta.is_file() || meta.len() > MAX_INSTALLED {
+            return None;
+        }
+        let raw = std::fs::read(&file).ok()?;
+        let v: serde_json::Value = serde_json::from_slice(&raw).ok()?;
+        Some(v.get("plugins")?.as_object()?.contains_key(&id))
+    })
+    .await
+    .ok()
+    .flatten()
+    .unwrap_or(false)
+}
+
+/// The zip of `name` with hash `sha256`: cached, else downloaded.
+async fn ready(dirs: &Dirs, name: &str, url: &str, sha256: &str) -> Result<PathBuf, String> {
+    let dir = dirs.cache.clone();
+    paths::ensure_private_dir(&dir).map_err(|e| format!("cache dir: {e}"))?;
+    let zip = dir.join(format!("{name}.zip"));
+    let sidecar = dir.join(format!("{name}.sha256"));
+    if zip.is_file() && std::fs::read_to_string(&sidecar).is_ok_and(|s| s.trim() == sha256) {
+        return Ok(zip);
+    }
+    if !zip.exists() && count_zips(&dir) >= MAX_BUNDLES {
+        return Err(format!("cache holds {MAX_BUNDLES} bundles already"));
+    }
+    let tmp = dir.join(format!(".{name}.{}.part", uuid::Uuid::new_v4().simple()));
+    let result = download(url, &tmp, sha256, dirs.allow_http).await;
+    let placed = result.and_then(|()| place(&tmp, &zip, &sidecar, sha256));
+    if placed.is_err() {
+        let _ = std::fs::remove_file(&tmp);
+    }
+    placed.map(|()| zip)
+}
+
+fn count_zips(dir: &Path) -> usize {
+    std::fs::read_dir(dir)
+        .map(|entries| {
+            entries
+                .flatten()
+                .filter(|e| e.file_name().to_string_lossy().ends_with(".zip"))
+                .count()
+        })
+        .unwrap_or(0)
+}
+
+/// Move a verified download into place. The old sidecar goes first, so no
+/// moment pairs a hash with a zip it does not describe; a crash in between
+/// leaves no sidecar, and the next request downloads again.
+fn place(tmp: &Path, zip: &Path, sidecar: &Path, sha256: &str) -> Result<(), String> {
+    match std::fs::remove_file(sidecar) {
+        Ok(()) => {}
+        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
+        Err(e) => return Err(format!("cannot replace the sidecar: {e}")),
+    }
+    std::fs::rename(tmp, zip).map_err(|e| format!("cannot store the bundle: {e}"))?;
+    paths::write_atomic(sidecar, format!("{sha256}\n").as_bytes())
+        .map_err(|e| format!("cannot write the sidecar: {e}"))
+}
+
+/// Stream `url` into a new file `tmp` (0600), checking the size cap and,
+/// at the end, the SHA-256. Redirects are followed to https URLs only.
+async fn download(url: &str, tmp: &Path, sha256: &str, allow_http: bool) -> Result<(), String> {
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
+    if resp.content_length().is_some_and(|n| n > MAX_BYTES) {
+        return Err("bundle over 64 MiB".to_owned());
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
+        if total > MAX_BYTES {
+            return Err("bundle over 64 MiB".to_owned());
+        }
+        hasher.update(&chunk);
+        file.write_all(&chunk)
+            .map_err(|e| format!("cannot write the bundle: {e}"))?;
+    }
+    file.sync_all()
+        .map_err(|e| format!("cannot write the bundle: {e}"))?;
+    let got = format!("{:x}", hasher.finalize());
+    if got != sha256 {
+        return Err("hash mismatch".to_owned());
+    }
+    Ok(())
+}
diff --git a/src/daemon/mod.rs b/src/daemon/mod.rs
index 7a60f3c..aa2d8dd 100644
--- a/src/daemon/mod.rs
+++ b/src/daemon/mod.rs
@@ -11,10 +11,13 @@
 //! - [`update`] — `claude update` (or the installer), on request.
 //! - [`git`] — the read-only git queries behind the TUI's git viewer.
 //! - [`login_path`] — the user's login-shell `PATH`, for claude and sessions.
+//! - [`bundle`] — proxy plugin bundles: the zips sessions load with
+//!   `--plugin-dir`.
 //!
 //! This module holds the shared [`Daemon`] state: the registry of journaled
 //! and live sessions, and the all-clients event broadcast.
 
+mod bundle;
 pub mod ctl;
 mod git;
 mod git_status;
@@ -348,6 +351,8 @@ pub(crate) struct Daemon {
     pub(crate) stats_rx: tokio::sync::watch::Receiver<stats::HostSnapshot>,
     /// `Search`: one at a time, and the transcripts it reads.
     pub(crate) search: server::SearchSlot,
+    /// `Bundle`: where plugin zips go, one download per name at a time.
+    pub(crate) bundles: bundle::Bundles,
 }
 
 /// Journaled sessions, and the live subset with a running process.
@@ -389,6 +394,7 @@ impl Daemon {
             shutdown: tokio::sync::Notify::new(),
             stats_rx,
             search: server::SearchSlot::new(),
+            bundles: bundle::Bundles::new(),
         }
     }
 
diff --git a/src/daemon/server.rs b/src/daemon/server.rs
index fcd59e5..35b7a4a 100644
--- a/src/daemon/server.rs
+++ b/src/daemon/server.rs
@@ -26,7 +26,7 @@ use tokio::sync::broadcast::error::{RecvError, TryRecvError};
 use tokio::sync::{broadcast, mpsc, Semaphore};
 
 use super::session::{ClientId, Cmd};
-use super::{git, host, update, Config, Daemon};
+use super::{bundle, git, host, update, Config, Daemon};
 use crate::claude::projects;
 use crate::claude::search::{self, BadQuery, Budget};
 use crate::paths;
@@ -558,6 +558,23 @@ impl Client {
                 Ok(()) => return,
                 Err(refused) => refused,
             },
+            Msg::Bundle {
+                name,
+                url,
+                sha256,
+                installed_id,
+            } => {
+                // A download (up to minutes): reply from a task, like
+                // `UpdateClaude`.
+                let daemon = Arc::clone(&self.daemon);
+                let queue = self.queue.clone();
+                tokio::spawn(async move {
+                    let msg =
+                        bundle::handle(&daemon.bundles, name, url, sha256, installed_id).await;
+                    let _ = queue.send(Frame::Control(Envelope { req, msg })).await;
+                });
+                return;
+            }
             other => Msg::Error {
                 message: format!("unsupported op: {}", op_name(&other)),
             },
diff --git a/src/daemon/tests.rs b/src/daemon/tests.rs
index 333bfb4..98921a0 100644
--- a/src/daemon/tests.rs
+++ b/src/daemon/tests.rs
@@ -2028,3 +2028,231 @@ async fn a_daemon_start_clears_old_context_files() {
     let _d = TestDaemon::start_in(dir).await;
     assert!(!context.join("old.txt").exists());
 }
+
+// ── Bundle ───────────────────────────────────────────────────────────────────
+
+/// A daemon whose bundles go to `<dir>/cache/plugins` and whose Claude Code
+/// config is `<dir>/claude` (http allowed, for the local test server).
+async fn bundle_daemon() -> (TestDaemon, PathBuf, PathBuf) {
+    let d = TestDaemon::start().await;
+    let cache = d.dir.join("cache/plugins");
+    let claude = d.dir.join("claude");
+    d.daemon.bundles.set_dirs(cache.clone(), claude.clone());
+    (d, cache, claude)
+}
+
+fn bundle(name: &str, url: &str, sha256: &str, installed_id: &str) -> Msg {
+    Msg::Bundle {
+        name: name.into(),
+        url: url.into(),
+        sha256: sha256.into(),
+        installed_id: installed_id.into(),
+    }
+}
+
+fn sha256_hex(bytes: &[u8]) -> String {
+    use sha2::Digest;
+    format!("{:x}", sha2::Sha256::digest(bytes))
+}
+
+/// Serve `body` at `/kb.zip` on loopback, counting the requests.
+async fn serve_zip(body: &'static [u8]) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
+    use std::sync::atomic::{AtomicUsize, Ordering};
+    let hits = Arc::new(AtomicUsize::new(0));
+    let counted = Arc::clone(&hits);
+    let router = axum::Router::new().route(
+        "/kb.zip",
+        axum::routing::get(move || {
+            counted.fetch_add(1, Ordering::SeqCst);
+            async move { body }
+        }),
+    );
+    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
+    let port = listener.local_addr().unwrap().port();
+    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
+    (format!("http://127.0.0.1:{port}/kb.zip"), hits)
+}
+
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
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn bundle_refuses_a_bad_name_hash_or_url() {
+    let d = TestDaemon::start().await;
+    let cache = d.dir.join("cache/plugins");
+    // The real daemon: no http, even on loopback.
+    let mut c = d.client().await;
+    let good = "a".repeat(64);
+    for (msg, shown) in [
+        (bundle("../x", "https://e.org/x.zip", &good, ""), "?"),
+        (bundle("Kb", "https://e.org/x.zip", &good, ""), "?"),
+        (
+            bundle("kb", "https://e.org/x.zip", &"A".repeat(64), ""),
+            "kb",
+        ),
+        (bundle("kb", "https://e.org/x.zip", "abc", ""), "kb"),
+        (bundle("kb", "http://e.org/x.zip", &good, ""), "kb"),
+        (bundle("kb", "file:///etc/passwd", &good, ""), "kb"),
+    ] {
+        let reply = c.call(msg.clone()).await;
+        assert_eq!(
+            reply,
+            Msg::Error {
+                message: format!("bundle {shown}: bad request")
+            },
+            "{msg:?}"
+        );
+    }
+    assert!(!cache.exists(), "nothing written");
+}
+
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn bundle_is_skipped_when_claude_code_has_the_plugin_installed() {
+    let (d, cache, claude) = bundle_daemon().await;
+    std::fs::create_dir_all(claude.join("plugins")).unwrap();
+    std::fs::write(
+        claude.join("plugins/installed_plugins.json"),
+        r#"{"version":2,"plugins":{"kb@vocdoni":[{"scope":"user"}]}}"#,
+    )
+    .unwrap();
+    let mut c = d.client().await;
+    let url = "https://127.0.0.1:9/never.zip";
+    let reply = c
+        .call(bundle("kb", url, &"a".repeat(64), "kb@vocdoni"))
+        .await;
+    assert_eq!(
+        reply,
+        Msg::Bundled {
+            name: "kb".into(),
+            path: None,
+            installed: true
+        }
+    );
+    assert!(!cache.exists(), "no download");
+    // Another id is not installed: on to the cache and the download.
+    let reply = c.call(bundle("kb", url, &"a".repeat(64), "kb@other")).await;
+    assert!(
+        matches!(&reply, Msg::Error { message } if message.starts_with("bundle kb: download failed")),
+        "{reply:?}"
+    );
+    // A broken file counts as not installed.
+    std::fs::write(claude.join("plugins/installed_plugins.json"), "{").unwrap();
+    let reply = c
+        .call(bundle("kb", url, &"a".repeat(64), "kb@vocdoni"))
+        .await;
+    assert!(matches!(reply, Msg::Error { .. }), "{reply:?}");
+}
+
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn a_cached_zip_with_its_sidecar_is_reused() {
+    let (d, cache, _) = bundle_daemon().await;
+    std::fs::create_dir_all(&cache).unwrap();
+    std::fs::write(cache.join("kb.zip"), b"zip").unwrap();
+    let hash = sha256_hex(b"zip");
+    std::fs::write(cache.join("kb.sha256"), format!("{hash}\n")).unwrap();
+    let mut c = d.client().await;
+    // A URL the daemon never reaches.
+    let reply = c
+        .call(bundle("kb", "https://127.0.0.1:9/kb.zip", &hash, ""))
+        .await;
+    let path = cache.join("kb.zip").to_string_lossy().into_owned();
+    assert_eq!(
+        reply,
+        Msg::Bundled {
+            name: "kb".into(),
+            path: Some(path),
+            installed: false
+        }
+    );
+}
+
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn a_download_is_verified_stored_privately_and_shared() {
+    use std::os::unix::fs::PermissionsExt;
+    use std::sync::atomic::Ordering;
+    let (d, cache, _) = bundle_daemon().await;
+    let (url, hits) = serve_zip(b"the bundle").await;
+    let hash = sha256_hex(b"the bundle");
+    let mut a = d.client().await;
+    let mut b = d.client().await;
+    let (ra, rb) = tokio::join!(
+        a.call(bundle("kb", &url, &hash, "")),
+        b.call(bundle("kb", &url, &hash, ""))
+    );
+    let path = cache.join("kb.zip").to_string_lossy().into_owned();
+    for reply in [ra, rb] {
+        assert_eq!(
+            reply,
+            Msg::Bundled {
+                name: "kb".into(),
+                path: Some(path.clone()),
+                installed: false
+            }
+        );
+    }
+    assert_eq!(hits.load(Ordering::SeqCst), 1, "one download for both");
+    assert_eq!(std::fs::read(cache.join("kb.zip")).unwrap(), b"the bundle");
+    assert_eq!(
+        std::fs::read_to_string(cache.join("kb.sha256")).unwrap(),
+        format!("{hash}\n")
+    );
+    assert_eq!(files_in(&cache), ["kb.sha256", "kb.zip"]);
+    let mode = std::fs::metadata(&cache).unwrap().permissions().mode() & 0o777;
+    assert_eq!(mode, 0o700);
+}
+
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn a_wrong_hash_is_an_error_and_leaves_no_file() {
+    let (d, cache, _) = bundle_daemon().await;
+    let (url, _) = serve_zip(b"tampered").await;
+    let mut c = d.client().await;
+    let reply = c.call(bundle("kb", &url, &"b".repeat(64), "")).await;
+    assert_eq!(
+        reply,
+        Msg::Error {
+            message: "bundle kb: hash mismatch".into()
+        }
+    );
+    assert!(files_in(&cache).is_empty(), "{:?}", files_in(&cache));
+
+    // A new version replaces the old zip in place, and its sidecar.
+    std::fs::write(cache.join("kb.zip"), b"old").unwrap();
+    std::fs::write(cache.join("kb.sha256"), sha256_hex(b"old")).unwrap();
+    let reply = c
+        .call(bundle("kb", &url, &sha256_hex(b"tampered"), ""))
+        .await;
+    assert!(
+        matches!(reply, Msg::Bundled { path: Some(_), .. }),
+        "{reply:?}"
+    );
+    assert_eq!(std::fs::read(cache.join("kb.zip")).unwrap(), b"tampered");
+    assert_eq!(files_in(&cache), ["kb.sha256", "kb.zip"]);
+}
+
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn the_cache_keeps_at_most_32_bundles() {
+    let (d, cache, _) = bundle_daemon().await;
+    std::fs::create_dir_all(&cache).unwrap();
+    for i in 0..super::bundle::MAX_BUNDLES {
+        std::fs::write(cache.join(format!("p{i}.zip")), b"z").unwrap();
+    }
+    let (url, hits) = serve_zip(b"x").await;
+    let mut c = d.client().await;
+    let reply = c.call(bundle("kb", &url, &sha256_hex(b"x"), "")).await;
+    assert_eq!(
+        reply,
+        Msg::Error {
+            message: "bundle kb: cache holds 32 bundles already".into()
+        }
+    );
+    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 0);
+}
diff --git a/src/proto.rs b/src/proto.rs
index 701e1f6..c430d07 100644
--- a/src/proto.rs
+++ b/src/proto.rs
@@ -207,6 +207,18 @@ pub enum Msg {
         #[serde(default)]
         limit: u32,
     },
+    /// Make plugin `name`'s bundle ready on this host: skipped when the
+    /// Claude Code plugin `installed_id` is installed here, else the cached
+    /// zip whose SHA-256 is `sha256`, downloaded from `url` (https) when
+    /// missing. Answered by `Bundled`, or `Error "bundle <name>: …"`. An
+    /// older daemon answers `Error "unsupported op…"`.
+    Bundle {
+        name: String,
+        url: String,
+        sha256: String,
+        #[serde(default)]
+        installed_id: String,
+    },
 
     // ── daemon → client replies ──────────────────────────────────────────
     Sessions {
@@ -272,6 +284,15 @@ pub enum Msg {
         total: u32,
         truncated: bool,
     },
+    /// Reply to `Bundle`: the zip's path on this host (for `--plugin-dir`),
+    /// or `installed` (the plugin is installed in Claude Code; no bundle).
+    Bundled {
+        name: String,
+        #[serde(default)]
+        path: Option<String>,
+        #[serde(default)]
+        installed: bool,
+    },
     Ok,
     Error {
         message: String,
@@ -1171,6 +1192,42 @@ mod tests {
         assert_eq!(back.msg, shell);
     }
 
+    #[test]
+    fn bundle_and_bundled_roundtrip_and_are_unknown_to_older_peers() {
+        #[derive(Debug, PartialEq, Deserialize)]
+        #[serde(tag = "op", rename_all = "snake_case")]
+        enum Old {
+            Ping,
+            #[serde(other)]
+            Unknown,
+        }
+        let bundle = Msg::Bundle {
+            name: "kb".into(),
+            url: "https://e.org/kb.zip".into(),
+            sha256: "a".repeat(64),
+            installed_id: "kb@m".into(),
+        };
+        let reply = Msg::Bundled {
+            name: "kb".into(),
+            path: Some("/c/plugins/kb.zip".into()),
+            installed: false,
+        };
+        for msg in [bundle, reply] {
+            let json = serde_json::to_string(&Envelope::request(4, msg.clone())).unwrap();
+            assert_eq!(serde_json::from_str::<Old>(&json).unwrap(), Old::Unknown);
+            assert_eq!(serde_json::from_str::<Envelope>(&json).unwrap().msg, msg);
+        }
+        let minimal = r#"{"op":"bundled","name":"kb"}"#;
+        assert_eq!(
+            serde_json::from_str::<Msg>(minimal).unwrap(),
+            Msg::Bundled {
+                name: "kb".into(),
+                path: None,
+                installed: false
+            }
+        );
+    }
+
     #[test]
     fn respawn_roundtrips_and_keeps_secrets_out_of_debug() {
         let respawn = Msg::Respawn(RespawnSpec {
~~~~~~

Run: `rustfmt --edition 2021 --check src/daemon/bundle.rs src/daemon/server.rs src/client.rs` → no output.
Run: `umask 022; cargo test --locked --bin claudio bundle` → `test result: ok. 4 passed`
Run: `umask 022; cargo test --locked --bin claudio daemon::tests::a_` → `test result: ok. 11 passed`
Run: `umask 022; cargo test --locked --bin claudio` → `test result: ok. 1020 passed; 0 failed`
Dev build: 30 dead-code warnings, all on Task 1's items. No other clippy finding on added lines.

Step 3: commit
`/usr/bin/git add src/daemon/bundle.rs`
`/usr/bin/git commit -m "feat(daemon): bundle op fetches and caches plugin zips" -- src/daemon/bundle.rs src/daemon/mod.rs src/daemon/server.rs src/daemon/tests.rs src/proto.rs src/client.rs`

Interfaces this task provides to later tasks:
- `Msg::Bundle { name, url, sha256, installed_id }` (request) and `Msg::Bundled { name, path: Option<String>, installed: bool }` (reply); an old daemon answers `Error "unsupported op: bundle"`.
- `Daemon.bundles: bundle::Bundles` (`#[cfg(test)] set_dirs(cache, claude_config)`, which also allows http); `bundle::MAX_BUNDLES`.
- `client.request(Msg::Bundle{..})` waits up to 250 s.


### Task 3: `--plugin-dir` probe and drop
Model: standard   Repo: /volumes/repos/claudio   Files: src/daemon/host.rs, src/daemon/mod.rs, src/daemon/server.rs, src/daemon/session.rs, src/daemon/tests.rs
Goal: one cached `claude --help` probe for both optional flags; `command()` drops `--plugin-dir` pairs for a claude without it, so an old claude still starts (spec §5; AC3's last clause at the daemon level).
Patch: .delivery/2026-11-06-session-plugins/patches/task-3.patch

Step 1: write the failing test
The tests are in the patch below: `src/daemon/host.rs` `claude_flags_come_from_one_help_run` (and the existing
long-help test, renamed `claude_flags_read_long_help`, now on `claude_flags`), `src/daemon/session.rs`
`plugin_dirs_are_dropped_for_a_claude_without_the_flag` (pairs, `=` form, a trailing flag) and
`src/daemon/tests.rs` `plugin_dirs_are_dropped_for_an_old_claude_but_journaled`.
Apply the test file alone first:
`/usr/bin/git apply --include='src/daemon/tests.rs' .delivery/2026-11-06-session-plugins/patches/task-3.patch`
Run: `cargo test --locked --bin claudio plugin_dirs`
Expected: the build fails with ``error[E0599]: no method named `plugin_dir` found for struct `std::sync::Arc<Daemon>` ``. Undo it (`/usr/bin/git restore src/daemon/tests.rs`).

Step 2: implement
`/usr/bin/git apply .delivery/2026-11-06-session-plugins/patches/task-3.patch`

~~~~~~diff
diff --git a/src/daemon/host.rs b/src/daemon/host.rs
index 7778396..3164ccb 100644
--- a/src/daemon/host.rs
+++ b/src/daemon/host.rs
@@ -99,10 +99,26 @@ fn run_version(claude: &Path, path: Option<&OsStr>) -> Option<String> {
 /// in it.
 pub const ALLOW_SKIP_PERMISSIONS: &str = "--allow-dangerously-skip-permissions";
 
-/// Whether `<claude> --help` mentions `flag`. Blocking (≤ 3 s); `false` on
-/// failure or timeout, so an old or broken claude never gets the flag.
-pub fn claude_supports(claude: &Path, flag: &str, path: Option<&OsStr>) -> bool {
-    run_capture(claude, "--help", path).is_some_and(|help| help.contains(flag))
+/// Loads a plugin for one session, uninstalled (a directory or a zip).
+pub const PLUGIN_DIR: &str = "--plugin-dir";
+
+/// The optional flags this claude knows, from one `--help` run.
+#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
+pub struct ClaudeFlags {
+    /// [`ALLOW_SKIP_PERMISSIONS`].
+    pub skip_permissions: bool,
+    /// [`PLUGIN_DIR`].
+    pub plugin_dir: bool,
+}
+
+/// [`ClaudeFlags`] of `<claude> --help`. Blocking (≤ 3 s); all `false` on
+/// failure or timeout, so an old or broken claude never gets a flag.
+pub fn claude_flags(claude: &Path, path: Option<&OsStr>) -> ClaudeFlags {
+    let help = run_capture(claude, "--help", path).unwrap_or_default();
+    ClaudeFlags {
+        skip_permissions: help.contains(ALLOW_SKIP_PERMISSIONS),
+        plugin_dir: help.contains(PLUGIN_DIR),
+    }
 }
 
 /// Stdout of `<claude> <arg>` when it exits 0 within [`VERSION_TIMEOUT`].
@@ -229,7 +245,7 @@ mod tests {
 
     #[cfg(unix)]
     #[test]
-    fn claude_supports_reads_help() {
+    fn claude_flags_read_long_help() {
         use std::os::unix::fs::PermissionsExt;
         let dir = std::env::temp_dir().join(format!("claudio-help-{}", uuid::Uuid::new_v4()));
         std::fs::create_dir_all(&dir).unwrap();
@@ -241,13 +257,38 @@ mod tests {
         )
         .unwrap();
         std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
-        assert!(claude_supports(&bin, ALLOW_SKIP_PERMISSIONS, None));
-        assert!(!claude_supports(&bin, "--no-such-flag", None));
-        assert!(!claude_supports(
-            Path::new("/nonexistent/claude"),
-            ALLOW_SKIP_PERMISSIONS,
-            None
-        ));
+        let flags = claude_flags(&bin, None);
+        assert!(flags.skip_permissions);
+        assert!(!flags.plugin_dir);
+        let _ = std::fs::remove_dir_all(&dir);
+    }
+
+    #[cfg(unix)]
+    #[test]
+    fn claude_flags_come_from_one_help_run() {
+        use std::os::unix::fs::PermissionsExt;
+        let dir = std::env::temp_dir().join(format!("claudio-flags-{}", uuid::Uuid::new_v4()));
+        std::fs::create_dir_all(&dir).unwrap();
+        let bin = dir.join("claude");
+        let help = |text: &str| {
+            let script = format!("#!/bin/sh\n[ \"$1\" = --help ] && printf '%s\\n' '{text}'\n");
+            std::fs::write(&bin, script).unwrap();
+            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
+            claude_flags(&bin, None)
+        };
+        let both = help("  --plugin-dir <paths...>  --allow-dangerously-skip-permissions");
+        assert_eq!(
+            both,
+            ClaudeFlags {
+                skip_permissions: true,
+                plugin_dir: true
+            }
+        );
+        let plugins = help("  --plugin-dir <paths...>");
+        assert!(plugins.plugin_dir && !plugins.skip_permissions);
+        assert_eq!(help(""), ClaudeFlags::default());
+        let missing = claude_flags(Path::new("/nonexistent/claude"), None);
+        assert_eq!(missing, ClaudeFlags::default());
         let _ = std::fs::remove_dir_all(&dir);
     }
 
diff --git a/src/daemon/mod.rs b/src/daemon/mod.rs
index aa2d8dd..21c2fff 100644
--- a/src/daemon/mod.rs
+++ b/src/daemon/mod.rs
@@ -341,10 +341,10 @@ pub(crate) struct Daemon {
     host: tokio::sync::Mutex<Option<HostInfo>>,
     /// Held while `claude update` runs: one at a time.
     updating: tokio::sync::Mutex<()>,
-    /// Whether this claude knows `--allow-dangerously-skip-permissions`
-    /// (it runs `claude --help`). Probed on first use, and again after a
-    /// claude update or install, like `host`.
-    skip_permissions_supported: Mutex<Option<bool>>,
+    /// Which optional flags this claude knows (`--allow-dangerously-skip-
+    /// permissions`, `--plugin-dir`; it runs `claude --help`). Probed on
+    /// first use, and again after a claude update or install, like `host`.
+    claude_flags: Mutex<Option<host::ClaudeFlags>>,
     /// Notified when the daemon should shut down cleanly.
     pub(crate) shutdown: tokio::sync::Notify,
     /// Latest host resource snapshot, updated every ~2 s by the stats sampler.
@@ -390,7 +390,7 @@ impl Daemon {
             events,
             host: tokio::sync::Mutex::new(None),
             updating: tokio::sync::Mutex::new(()),
-            skip_permissions_supported: Mutex::new(None),
+            claude_flags: Mutex::new(None),
             shutdown: tokio::sync::Notify::new(),
             stats_rx,
             search: server::SearchSlot::new(),
@@ -419,7 +419,7 @@ impl Daemon {
     /// or old before may know the flag now.
     async fn refresh_host(&self) -> HostInfo {
         *self
-            .skip_permissions_supported
+            .claude_flags
             .lock()
             .unwrap_or_else(|e| e.into_inner()) = None;
         let host = self.probe_host().await;
@@ -447,20 +447,22 @@ impl Daemon {
     /// enabled in the config and supported by this claude. Blocking the first
     /// time (it runs `claude --help`); call it off the async runtime.
     pub(crate) fn skip_permissions(&self) -> bool {
-        if !self.config.skip_permissions {
-            return false;
-        }
+        self.config.skip_permissions && self.claude_flags().skip_permissions
+    }
+
+    /// Whether this claude takes `--plugin-dir`. Blocking the first time,
+    /// like [`Daemon::skip_permissions`].
+    pub(crate) fn plugin_dir(&self) -> bool {
+        self.claude_flags().plugin_dir
+    }
+
+    /// The optional flags this claude knows: probed once (`claude --help`),
+    /// until the next claude update. Blocking the first time.
+    pub(crate) fn claude_flags(&self) -> host::ClaudeFlags {
         // Held while probing, so concurrent spawns wait for one probe.
-        let mut supported = self
-            .skip_permissions_supported
-            .lock()
-            .unwrap_or_else(|e| e.into_inner());
-        *supported.get_or_insert_with(|| {
-            host::claude_supports(
-                &self.config.claude_bin(),
-                host::ALLOW_SKIP_PERMISSIONS,
-                self.config.path.as_deref(),
-            )
+        let mut flags = self.claude_flags.lock().unwrap_or_else(|e| e.into_inner());
+        *flags.get_or_insert_with(|| {
+            host::claude_flags(&self.config.claude_bin(), self.config.path.as_deref())
         })
     }
 
diff --git a/src/daemon/server.rs b/src/daemon/server.rs
index 35b7a4a..406abb5 100644
--- a/src/daemon/server.rs
+++ b/src/daemon/server.rs
@@ -102,7 +102,7 @@ pub fn start(config: Config) -> io::Result<Option<Listening>> {
         warm.host().await;
     });
     let warm = Arc::clone(&daemon);
-    tokio::task::spawn_blocking(move || warm.skip_permissions());
+    tokio::task::spawn_blocking(move || warm.claude_flags());
     Ok(Some(Listening {
         daemon,
         listener,
diff --git a/src/daemon/session.rs b/src/daemon/session.rs
index 5784782..34ef3ce 100644
--- a/src/daemon/session.rs
+++ b/src/daemon/session.rs
@@ -182,6 +182,7 @@ pub fn spawn(
                 &cwd,
                 &token,
                 daemon.skip_permissions(),
+                daemon.plugin_dir(),
                 context.as_deref(),
             );
             let program = PathBuf::from(cmd.get_argv()[0].clone());
@@ -316,12 +317,17 @@ pub(super) fn clear_contexts(socket: &Path) {
 /// The skip-permissions flag is added here, on every launch path (new spawn,
 /// `--resume`, resume-retry), rather than in `spec.args`, so the journal never
 /// stores it. It is left out when the args already choose a permissions flag.
+///
+/// Without `plugin_dir` (this claude does not know `--plugin-dir`) every
+/// `--plugin-dir <path>` pair of the args is dropped, so an old claude still
+/// starts; the journal keeps them for a newer one.
 fn command(
     cfg: &super::Config,
     spec: &SpawnSpec,
     cwd: &Path,
     token: &str,
     skip_permissions: bool,
+    plugin_dir: bool,
     context: Option<&Path>,
 ) -> CommandBuilder {
     let settings = hooks::settings(&cfg.claudio, &cfg.socket, token, context);
@@ -336,7 +342,15 @@ fn command(
     {
         cmd.arg(host::ALLOW_SKIP_PERMISSIONS);
     }
-    for a in &spec.args {
+    let mut args = spec.args.iter();
+    while let Some(a) = args.next() {
+        if !plugin_dir && a == host::PLUGIN_DIR {
+            args.next();
+            continue;
+        }
+        if !plugin_dir && a.starts_with("--plugin-dir=") {
+            continue;
+        }
         cmd.arg(a);
     }
     if let Some(name) = &spec.name {
@@ -1276,7 +1290,7 @@ mod tests {
             ephemeral: false,
             extensions: None,
         };
-        let cmd = command(&cfg, &spec, Path::new("/"), "tok123", false, None);
+        let cmd = command(&cfg, &spec, Path::new("/"), "tok123", false, true, None);
         let argv = argv_of(&cmd);
         assert_eq!(argv[0], "/bin/claude");
         assert_eq!(argv[1], "--settings");
@@ -1289,7 +1303,7 @@ mod tests {
 
         // With skip-permissions on, the flag follows --settings, ahead of
         // the spec args (which are what the journal stores).
-        let cmd = command(&cfg, &spec, Path::new("/"), "tok123", true, None);
+        let cmd = command(&cfg, &spec, Path::new("/"), "tok123", true, true, None);
         assert_eq!(
             &argv_of(&cmd)[3..],
             [ALLOW, "--resume", "abc", "-n", "work"]
@@ -1298,7 +1312,15 @@ mod tests {
 
         // A context file adds its hook to SessionStart: the path, not the text.
         let start = |context: Option<&Path>| {
-            let argv = argv_of(&command(&cfg, &spec, Path::new("/"), "t", false, context));
+            let argv = argv_of(&command(
+                &cfg,
+                &spec,
+                Path::new("/"),
+                "t",
+                false,
+                true,
+                context,
+            ));
             let settings: Value = serde_json::from_str(&argv[2]).unwrap();
             settings["hooks"]["SessionStart"][0]["hooks"].clone()
         };
@@ -1461,8 +1483,54 @@ mod tests {
                 ephemeral: false,
                 extensions: None,
             };
-            let argv = argv_of(&command(&cfg, &spec, Path::new("/"), "t", true, None));
+            let argv = argv_of(&command(&cfg, &spec, Path::new("/"), "t", true, true, None));
             assert_eq!(&argv[3..], [given]);
         }
     }
+
+    #[test]
+    fn plugin_dirs_are_dropped_for_a_claude_without_the_flag() {
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
+        let spec = SpawnSpec {
+            id: uuid::Uuid::nil(),
+            cwd: "/".into(),
+            name: None,
+            args: vec![
+                "--plugin-dir".into(),
+                "/c/kb.zip".into(),
+                "--resume".into(),
+                "abc".into(),
+                "--plugin-dir=/c/x.zip".into(),
+                "--plugin-dir".into(),
+            ],
+            env: vec![],
+            rows: 24,
+            cols: 80,
+            ephemeral: false,
+            extensions: None,
+        };
+        let argv = |plugin_dir| {
+            argv_of(&command(
+                &cfg,
+                &spec,
+                Path::new("/"),
+                "t",
+                false,
+                plugin_dir,
+                None,
+            ))
+        };
+        assert_eq!(&argv(false)[3..], ["--resume", "abc"]);
+        assert_eq!(&argv(true)[3..], &spec.args[..]);
+    }
 }
diff --git a/src/daemon/tests.rs b/src/daemon/tests.rs
index 98921a0..faac254 100644
--- a/src/daemon/tests.rs
+++ b/src/daemon/tests.rs
@@ -2256,3 +2256,23 @@ async fn the_cache_keeps_at_most_32_bundles() {
     );
     assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 0);
 }
+
+/// A claude that does not know `--plugin-dir` (the fake's `--help` is
+/// empty) still starts: the daemon leaves the pairs out of its argv, and
+/// the journal keeps them for a claude that does.
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn plugin_dirs_are_dropped_for_an_old_claude_but_journaled() {
+    let d = TestDaemon::start().await;
+    let mut c = d.client().await;
+    let id = Uuid::new_v4();
+    let mut spec = d.spec(id);
+    spec.args = vec!["--plugin-dir".into(), "/c/plugins/kb.zip".into()];
+    c.spawn(spec).await;
+    c.attach_until(id, BANNER).await;
+    assert!(!d.daemon.plugin_dir());
+    let entry = &d.journal()["sessions"][0];
+    assert_eq!(
+        entry["args"],
+        serde_json::json!(["--plugin-dir", "/c/plugins/kb.zip"])
+    );
+}
~~~~~~

Run: `rustfmt --edition 2021 --check src/daemon/host.rs src/daemon/server.rs` → no output.
Run: `umask 022; cargo test --locked --bin claudio plugin_dirs` → `test result: ok. 2 passed`
Run: `umask 022; cargo test --locked --bin claudio` → `test result: ok. 1023 passed; 0 failed`
`host::claude_supports` is gone (its one caller now uses `claude_flags`). Dev build: 30 dead-code
warnings (Task 1's items).

Step 3: commit
`/usr/bin/git commit -m "feat(daemon): drop --plugin-dir for a claude without it" -- src/daemon/host.rs src/daemon/mod.rs src/daemon/server.rs src/daemon/session.rs src/daemon/tests.rs`

Interfaces this task provides to later tasks:
- `host::{PLUGIN_DIR, ClaudeFlags, claude_flags}`; `Daemon::{claude_flags(), plugin_dir(), skip_permissions()}`.
- `session::command(cfg, spec, cwd, token, skip_permissions, plugin_dir, context)`.


### Task 4: registry rows, extensions screen and the App's plugin state
Model: standard   Repo: /volumes/repos/claudio   Files: src/tui/plugins.rs (new), src/tui/extensions/mod.rs, src/tui/extensions_view.rs, src/tui/app.rs, src/tui/interaction.rs, src/tui/mod.rs, tests/manager.rs
Goal: plugins become `Source::Plugin` registry rows (refused ones listed), the screen shows kind `plugin`, per-host state and the list state, toggles save through `Effect::SaveExtension`, `r` refetches; `App` keeps lists, saved choices and each `(host, plugin)` answer (spec §3, §4 client side).
Patch: .delivery/2026-11-06-session-plugins/patches/task-4.patch

Step 1: write the failing test
The tests are in the patch below: `src/tui/extensions/mod.rs`
`plugins_follow_the_others_and_a_taken_name_is_refused`; `src/tui/plugins.rs`
`a_list_is_listed_and_its_bundles_asked_of_every_connected_host`,
`answers_set_each_hosts_state_and_stale_ones_are_dropped` (ready, installed, `unsupported op` → too old,
failure text without the `bundle <name>: ` prefix, host gone), `env_comes_from_enabled_admitted_plugins_of_the_profile`,
`signed_out_drops_the_list_and_unreachable_keeps_it`; `src/tui/extensions_view.rs`
`plugins_are_listed_with_their_state_on_each_host`, `a_plugin_toggle_is_saved_and_kept_for_later_lists`,
`r_fetches_the_plugin_lists_again`. `tests/manager.rs` only follows the new screen title.
Run: `cargo test --locked --bin claudio tui::`
Expected: the build fails (`set_plugins`, `Source::Plugin`, `on_plugins`, `Effect::FetchPlugins`,
`ReplyTo::Bundle` not found).

Step 2: implement
`/usr/bin/git apply .delivery/2026-11-06-session-plugins/patches/task-4.patch`

~~~~~~diff
diff --git a/src/tui/app.rs b/src/tui/app.rs
index 690a261..f1f6c4c 100644
--- a/src/tui/app.rs
+++ b/src/tui/app.rs
@@ -190,6 +190,9 @@ pub enum Effect {
         id: SessionId,
         answers: Vec<crate::proto::Answer>,
     },
+    /// Fetch `profile`'s plugin list, then save it to (or drop it from)
+    /// `plugins.json`; the outcome goes to [`App::on_plugins`].
+    FetchPlugins { profile: String },
 }
 
 /// What a request's reply is for.
@@ -228,6 +231,12 @@ pub enum ReplyTo {
     Search { gen: u64, host: String },
     /// `LastPrompt` on `host`, for the snippet save started under `gen`.
     LastPrompt { gen: u64, host: String },
+    /// `Bundle` of plugin `name` at hash `sha256` on `host`.
+    Bundle {
+        host: String,
+        name: String,
+        sha256: String,
+    },
 }
 
 impl ReplyTo {
@@ -346,6 +355,8 @@ pub struct App {
     /// The external extensions the extension host disabled after
     /// failures (until claudio restarts), by name.
     pub failed_extensions: Vec<String>,
+    /// The proxy plugins: lists, saved choices, each host's bundles.
+    pub plugins: super::plugins::Plugins,
 }
 
 /// What the app starts from. The event loop gathers it (reading config.toml,
@@ -438,6 +449,7 @@ impl App {
             lens_hosts: HashSet::new(),
             pending_action: None,
             failed_extensions: Vec::new(),
+            plugins: Default::default(),
         }
     }
 
diff --git a/src/tui/extensions/mod.rs b/src/tui/extensions/mod.rs
index 2b97ecb..2b1b571 100644
--- a/src/tui/extensions/mod.rs
+++ b/src/tui/extensions/mod.rs
@@ -17,6 +17,7 @@ pub mod protocol;
 use std::collections::BTreeMap;
 
 use crate::proto::{LensKind, LensSpec, SessionExtensions, MAX_INSTRUCTIONS, MAX_LENSES};
+use crate::proxy::plugins::PluginList;
 
 /// The diagram extension's instruction (spec §7).
 pub const DIAGRAM_CONTEXT: &str = "This terminal draws Mermaid diagrams. When a diagram \
@@ -81,6 +82,12 @@ pub enum Source {
     BuiltIn,
     /// A program the extension host runs.
     External(manifest::Program),
+    /// A proxy plugin of `profile`: a bundle claude loads, and env. No
+    /// lenses or actions. `refused` says why it is not used, if it is not.
+    Plugin {
+        profile: String,
+        refused: Option<String>,
+    },
 }
 
 #[derive(Debug, Clone, PartialEq)]
@@ -97,6 +104,10 @@ impl Extension {
     pub fn is_external(&self) -> bool {
         matches!(self.source, Source::External(_))
     }
+
+    pub fn is_plugin(&self) -> bool {
+        matches!(self.source, Source::Plugin { .. })
+    }
 }
 
 /// The built-in extensions, in registry order, all enabled.
@@ -196,6 +207,32 @@ impl Registry {
         Registry { extensions }
     }
 
+    /// Replace the proxy plugins with those of `lists`, after the built-ins
+    /// and the externals, with `[extensions]` (`enabled`) applied. A plugin
+    /// [`PluginList::checked`] refuses, or whose name another extension
+    /// (or an earlier list) has, is listed as refused and off.
+    pub fn set_plugins(&mut self, lists: &[&PluginList], enabled: &BTreeMap<String, bool>) {
+        self.extensions.retain(|e| !e.is_plugin());
+        for list in lists {
+            for (p, why) in list.checked() {
+                let taken = self.extensions.iter().any(|e| e.name == p.name);
+                let refused = why.or_else(|| taken.then(|| "name taken".to_owned()));
+                let on = refused.is_none() && enabled.get(&p.name).copied().unwrap_or(true);
+                self.extensions.push(Extension {
+                    name: p.name.clone(),
+                    description: format!("{} · {}", p.version, list.profile),
+                    enabled: on,
+                    lenses: Vec::new(),
+                    actions: Vec::new(),
+                    source: Source::Plugin {
+                        profile: list.profile.clone(),
+                        refused,
+                    },
+                });
+            }
+        }
+    }
+
     /// Turn extension `name` on or off; whether there is one.
     pub fn set_enabled(&mut self, name: &str, on: bool) -> bool {
         match self.extensions.iter_mut().find(|e| e.name == name) {
@@ -416,6 +453,53 @@ mod tests {
         assert!(!r.set_enabled("nope", true));
     }
 
+    #[test]
+    fn plugins_follow_the_others_and_a_taken_name_is_refused() {
+        use crate::proxy::plugins::sample;
+        let ext = vec![external("keys", "keys", 'k')];
+        let mut r = Registry::new(&BTreeMap::new(), true, ext, &mut Vec::new());
+        let work = PluginList {
+            profile: "work".into(),
+            fetched_at: 1,
+            plugins: vec![sample("kb"), sample("keys"), sample("latex"), sample("off")],
+        };
+        let mut bad = sample("evil");
+        bad.bundle_url = "http://e.org/x.zip".into();
+        let home = PluginList {
+            profile: "home".into(),
+            fetched_at: 1,
+            plugins: vec![sample("kb"), bad],
+        };
+        let enabled = BTreeMap::from([("off".to_owned(), false)]);
+        r.set_plugins(&[&work, &home], &enabled);
+        let plugins: Vec<(&str, bool, Option<&str>)> = r
+            .extensions
+            .iter()
+            .filter_map(|e| match &e.source {
+                Source::Plugin { refused, .. } => {
+                    Some((e.name.as_str(), e.enabled, refused.as_deref()))
+                }
+                _ => None,
+            })
+            .collect();
+        assert_eq!(
+            plugins,
+            [
+                ("kb", true, None),
+                ("keys", false, Some("name taken")),
+                ("latex", false, Some("name taken")),
+                ("off", false, None),
+                ("kb", false, Some("name taken")),
+                ("evil", false, Some("bundle URL is not https")),
+            ]
+        );
+        assert_eq!(r.extensions[5].description, "1.4.0 · work");
+        // Plugins bring no lenses or actions; a new list replaces the old.
+        assert_eq!(names(&r.session_extensions()), ["latex", "mermaid", "keys"]);
+        r.set_plugins(&[], &enabled);
+        assert_eq!(r.extensions.len(), 5);
+    }
+
     #[test]
     fn the_session_context_stays_within_the_caps() {
         let mut ext: Vec<Extension> = (0..16)
diff --git a/src/tui/extensions_view.rs b/src/tui/extensions_view.rs
index 141def1..614556e 100644
--- a/src/tui/extensions_view.rs
+++ b/src/tui/extensions_view.rs
@@ -1,7 +1,9 @@
 //! The extensions screen (Alt+i, or `e` in the leader popup): one row per
-//! extension (built-ins first, then the external ones that loaded, on or
-//! off) with its kind, keys and lenses; below the list, the selected one's
-//! description, actions and lenses. Space or Enter turns it on or off.
+//! extension (built-ins first, then the external ones that loaded, then the
+//! proxy plugins, on or off) with its kind, keys and lenses; below the list,
+//! the selected one's description, actions and lenses (for a plugin, its
+//! state on each host). Space or Enter turns it on or off; `r` fetches the
+//! plugin lists again.
 //!
 //! Pure, like `app.rs`: a toggle changes the registry at once and returns
 //! [`Effect::SaveExtension`] (the event loop writes `config.toml`) and, for
@@ -11,14 +13,14 @@
 use crossterm::event::{KeyCode, KeyEvent};
 
 use super::app::{App, Effect, Mode};
-use super::extensions::{ActionDecl, Extension};
+use super::extensions::{ActionDecl, Extension, Source};
 use super::interaction::Modal;
 use super::keymap::Action;
 use crate::claude::latex::sanitize;
 use crate::proto::LensKind;
 
 /// The screen's title.
-pub const TITLE: &str = "Extensions (Space toggle · Esc close)";
+pub const TITLE: &str = "Extensions (Space toggle · r refresh · Esc close)";
 /// The screen's last line.
 pub const FOOTER: &str =
     "changes apply now; claude's instructions change for new sessions (Alt+e restarts one)";
@@ -38,6 +40,8 @@ pub struct ExtensionsView {
 enum Outcome {
     Stay,
     Toggle(usize),
+    /// `r`: fetch the plugin lists again.
+    Refresh,
     Close,
 }
 
@@ -50,6 +54,7 @@ impl ExtensionsView {
             KeyCode::Char(' ') | KeyCode::Enter if count > 0 => {
                 return Outcome::Toggle(self.selected.min(last))
             }
+            KeyCode::Char('r') => return Outcome::Refresh,
             KeyCode::Up => self.selected = self.selected.saturating_sub(1),
             KeyCode::Down => self.selected = (self.selected + 1).min(last),
             KeyCode::PageUp => self.selected = self.selected.saturating_sub(PAGE),
@@ -77,7 +82,8 @@ impl ExtensionsView {
 pub struct Row {
     pub name: String,
     pub enabled: bool,
-    pub external: bool,
+    /// `built-in`, `external` or `plugin`.
+    pub kind: &'static str,
     /// Its actions' keys (`Alt+z p/P`), or `—`.
     pub shortcuts: String,
     /// Its lenses (`<latex> · ```mermaid`), or `—`.
@@ -108,14 +114,14 @@ pub fn row_lines(rows: &[Row]) -> Vec<String> {
     rows.iter()
         .map(|r| {
             let mark = if r.enabled { 'x' } else { ' ' };
-            let kind = if r.external { "external" } else { "built-in" };
+            let kind = r.kind;
             let failed = if r.failed {
                 "  (disabled after failures)"
             } else {
                 ""
             };
             let line = format!(
-                "[{mark}] {:name_width$}  {kind}  {:keys_width$}  {}{failed}",
+                "[{mark}] {:name_width$}  {kind:8}  {:keys_width$}  {}{failed}",
                 r.name, r.shortcuts, r.lenses
             );
             line.trim_end().to_owned()
@@ -173,10 +179,15 @@ impl App {
                 } else {
                     lenses.join(" · ")
                 };
+                let kind = match ext.source {
+                    Source::BuiltIn => "built-in",
+                    Source::External(_) => "external",
+                    Source::Plugin { .. } => "plugin",
+                };
                 Row {
                     name: clean(&ext.name),
                     enabled: ext.enabled,
-                    external: ext.is_external(),
+                    kind,
                     shortcuts,
                     lenses,
                     failed: self.failed_extensions.contains(&ext.name),
@@ -213,6 +224,10 @@ impl App {
         if ext.name == "latex" && !self.claude_policy.latex {
             lines.push("also off by [claude] latex = false".to_owned());
         }
+        if let Source::Plugin { profile, refused } = &ext.source {
+            let details = self.plugin_details(&ext.name, profile, refused.as_deref());
+            lines.extend(details.iter().map(|l| clean(l)));
+        }
         lines
     }
 
@@ -226,6 +241,7 @@ impl App {
             Outcome::Stay => {}
             Outcome::Close => self.modal = None,
             Outcome::Toggle(index) => self.toggle_extension(index),
+            Outcome::Refresh => self.refresh_plugins(),
         }
         self.redraw = true;
     }
@@ -238,6 +254,24 @@ impl App {
             return;
         };
         let (name, on, external) = (ext.name.clone(), !ext.enabled, ext.is_external());
+        if let Source::Plugin { refused, .. } = &ext.source {
+            if let Some(why) = refused {
+                self.notify(format!("plugin {} is refused: {why}", clean(&name)));
+                return;
+            }
+            // Kept for a list that comes later; on: the hosts get its bundle.
+            self.plugins.config.insert(name.clone(), on);
+            self.extensions.extensions[index].enabled = on;
+            if on {
+                let hosts: Vec<String> = self.plugins.hosts.iter().cloned().collect();
+                for host in hosts {
+                    self.request_bundles(&host);
+                }
+            }
+            self.emit(Effect::SaveExtension { name, on });
+            self.redraw = true;
+            return;
+        }
         self.extensions.set_enabled(&name, on);
         if external {
             self.emit(Effect::SetExtension {
@@ -253,7 +287,9 @@ impl App {
 #[cfg(test)]
 mod tests {
     use super::*;
-    use crate::tui::extensions::{manifest::Program, Lens, Registry, Source};
+    use crate::proto::Msg;
+    use crate::tui::app::ReplyTo;
+    use crate::tui::extensions::{manifest::Program, Lens, Registry};
     use crate::tui::test_support::{alt, app_with, info, press};
 
     /// An app with one session and an external extension `fake` (a tag
@@ -482,6 +518,119 @@ mod tests {
         assert!(details[0].contains("next"), "on one line: {details:?}");
     }
 
+    /// [`app_with_fake`] with proxy profile `work` serving plugins `kb`
+    /// and `latex` (a taken name), and `local` connected.
+    fn app_with_plugins() -> App {
+        use crate::proxy::plugins::{sample, PluginList};
+        use crate::tui::plugins::PluginsFetch;
+        let mut app = app_with_fake("");
+        app.on_plugin_host("local");
+        let list = PluginList {
+            profile: "work".into(),
+            fetched_at: 1,
+            plugins: vec![sample("kb"), sample("latex")],
+        };
+        app.on_plugins("work", PluginsFetch::Listed(list));
+        app.take_effects();
+        app
+    }
+
+    #[test]
+    fn plugins_are_listed_with_their_state_on_each_host() {
+        let mut app = app_with_plugins();
+        let lines = row_lines(&app.extension_rows());
+        assert_eq!(lines[5], "[x] kb        plugin    —          —");
+        assert_eq!(lines[6], "[ ] latex     plugin    —          —");
+        let details = |app: &App, i| app.extension_details(i);
+        assert_eq!(
+            details(&app, 5),
+            [
+                "1.4.0 · work",
+                "turning it on or off applies to new sessions",
+                "local: downloading",
+            ]
+        );
+        let ready = Msg::Bundled {
+            name: "kb".into(),
+            path: Some("/c/kb.zip".into()),
+            installed: false,
+        };
+        let sha = "a".repeat(64);
+        app.on_reply(
+            ReplyTo::Bundle {
+                host: "local".into(),
+                name: "kb".into(),
+                sha256: sha.clone(),
+            },
+            Ok(ready),
+        );
+        assert_eq!(details(&app, 5)[2], "local: ready");
+        app.on_reply(
+            ReplyTo::Bundle {
+                host: "local".into(),
+                name: "kb".into(),
+                sha256: sha,
+            },
+            Err(std::io::Error::other(
+                "bundle kb: download failed: HTTP 404",
+            )),
+        );
+        assert_eq!(details(&app, 5)[2], "local: download failed: HTTP 404");
+        assert!(
+            app.notice.is_none(),
+            "a bundle error is shown here, not as a notice"
+        );
+        assert_eq!(
+            details(&app, 6),
+            [
+                "1.4.0 · work",
+                "turning it on or off applies to new sessions",
+                "refused: name taken",
+            ]
+        );
+    }
+
+    #[test]
+    fn a_plugin_toggle_is_saved_and_kept_for_later_lists() {
+        let mut app = app_with_plugins();
+        app.on_terminal(alt('i'));
+        for _ in 0..5 {
+            app.on_terminal(press(KeyCode::Down));
+        }
+        app.on_terminal(press(KeyCode::Char(' ')));
+        assert_eq!(toggles(&mut app), [("save", "kb".to_owned(), false)]);
+        assert!(app.plugin_env("work").is_empty());
+        assert_eq!(app.plugins.config.get("kb"), Some(&false));
+        // Back on: saved, and the hosts are asked for its bundle again only
+        // when they have no answer at its hash.
+        app.on_terminal(press(KeyCode::Char(' ')));
+        assert_eq!(toggles(&mut app), [("save", "kb".to_owned(), true)]);
+        // A refused one does not toggle.
+        app.on_terminal(press(KeyCode::Down));
+        app.on_terminal(press(KeyCode::Char(' ')));
+        assert!(app.take_effects().is_empty());
+        let notice = app.notice.as_ref().unwrap().text.clone();
+        assert_eq!(notice, "plugin latex is refused: name taken");
+    }
+
+    #[test]
+    fn r_fetches_the_plugin_lists_again() {
+        let mut app = app_with_plugins();
+        app.proxy_default = Some("home".into());
+        app.on_terminal(alt('i'));
+        app.on_terminal(press(KeyCode::Char('r')));
+        let fetched: Vec<String> = app
+            .take_effects()
+            .into_iter()
+            .filter_map(|e| match e {
+                Effect::FetchPlugins { profile } => Some(profile),
+                _ => None,
+            })
+            .collect();
+        assert_eq!(fetched, ["home", "work"]);
+        assert!(matches!(app.modal, Some(Modal::ExtensionsMenu(_))));
+    }
+
     #[test]
     fn plain_mode_shows_no_search_key() {
         use crate::tui::app::AppConfig;
diff --git a/src/tui/interaction.rs b/src/tui/interaction.rs
index 52124b4..1243a49 100644
--- a/src/tui/interaction.rs
+++ b/src/tui/interaction.rs
@@ -783,6 +783,7 @@ impl App {
                 // Handled in the event loop before on_incoming_from is called.
             }
             Incoming::Disconnected => {
+                self.plugin_host_gone(host);
                 if host == "local" {
                     self.on_disconnected_local();
                 } else {
@@ -1115,6 +1116,9 @@ impl App {
                 }
             }
             (ReplyTo::ClaudeUpdate(host), reply) => self.on_claude_updated(&host, reply),
+            (ReplyTo::Bundle { host, name, sha256 }, reply) => {
+                self.on_bundled(&host, &name, &sha256, reply)
+            }
             // Typing a path that doesn't exist (yet) is not an error.
             (ReplyTo::DirEntries, Err(_)) => {}
             (to, Err(e)) if self.connected => {
diff --git a/src/tui/mod.rs b/src/tui/mod.rs
index f2bdc95..6e35577 100644
--- a/src/tui/mod.rs
+++ b/src/tui/mod.rs
@@ -22,6 +22,7 @@ mod latex;
 mod leader;
 mod notifications;
 mod plain;
+mod plugins;
 mod proxy_state;
 mod reader;
 mod selection;
@@ -555,6 +556,13 @@ enum HostEvent {
         gen: u64,
         result: Result<extensions::ActionResult, String>,
     },
+    /// A profile's plugin list was fetched (and the cache updated), or not.
+    Plugins {
+        profile: String,
+        fetch: plugins::PluginsFetch,
+    },
+    /// A notice from a background task.
+    Notice(String),
 }
 
 // ── Formula rendering ─────────────────────────────────────────────────────────
@@ -1193,6 +1201,8 @@ async fn event_loop(
                     app.on_snippets(gen, &dir, result);
                 }
                 Some(HostEvent::SnippetSaved { result, cut }) => app.on_snippet_saved(result, cut),
+                Some(HostEvent::Plugins { profile, fetch }) => app.on_plugins(&profile, fetch),
+                Some(HostEvent::Notice(text)) => app.notify(text),
                 None => return Ok(Default::default()),
             },
             Some((to, reply)) = reply_rx.recv() => app.on_reply(to, reply),
@@ -1398,6 +1408,13 @@ fn run_effect(
                 app.notify(format!("could not save state: {e}"));
             }
         }
+        Effect::FetchPlugins { profile } => {
+            let tx = ev_tx.clone();
+            tokio::spawn(async move {
+                let fetch = fetch_plugins(&profile, &tx).await;
+                let _ = tx.send(HostEvent::Plugins { profile, fetch }).await;
+            });
+        }
         Effect::FetchProxyConfig { profile_name } => {
             let tx = ev_tx.clone();
             let name = profile_name.clone();
@@ -1518,6 +1535,39 @@ fn run_effect(
     }
 }
 
+/// Fetch `profile`'s plugin list and keep `plugins.json` in step: a list
+/// is saved, a 401/403 drops the profile's entry (its secrets), anything
+/// else leaves it. A failed write is a notice.
+async fn fetch_plugins(profile: &str, tx: &mpsc::Sender<HostEvent>) -> plugins::PluginsFetch {
+    use crate::proxy::api::ApiError;
+    use crate::proxy::plugins as cache;
+    use plugins::PluginsFetch;
+    let Some(p) = crate::proxy::resolve::find_profile(profile) else {
+        return PluginsFetch::Unreachable;
+    };
+    let (fetch, write) = match cache::fetch(profile, &p).await {
+        Ok(list) => {
+            let saved = list.clone();
+            let write = tokio::task::spawn_blocking(move || cache::save_cache(&saved));
+            (PluginsFetch::Listed(list), Some(write))
+        }
+        Err(ApiError::Http(s)) if s.as_u16() == 401 || s.as_u16() == 403 => {
+            let name = profile.to_owned();
+            let write = tokio::task::spawn_blocking(move || cache::remove_cache(&name));
+            (PluginsFetch::SignedOut, Some(write))
+        }
+        Err(_) => (PluginsFetch::Unreachable, None),
+    };
+    if let Some(write) = write {
+        if let Ok(Err(e)) = write.await {
+            let _ = tx
+                .send(HostEvent::Notice(format!("could not save plugins: {e}")))
+                .await;
+        }
+    }
+    fetch
+}
+
 /// Fetch proxy config for a named profile. Returns `None` on any error.
 async fn fetch_proxy_config(profile_name: &str) -> Option<crate::proxy::api::ConfigResponse> {
     let (url, token) = crate::proxy::resolve::resolve_profile(profile_name)?;
diff --git a/src/tui/plugins.rs b/src/tui/plugins.rs
new file mode 100644
index 0000000..fe3775c
--- /dev/null
+++ b/src/tui/plugins.rs
@@ -0,0 +1,470 @@
+//! Proxy plugins on the client: each profile's list (from the proxy or its
+//! cache in `plugins.json`), what each connected host answered for each
+//! bundle, and what a spawn gets from them.
+//!
+//! Pure, like `app.rs`: fetching a list, writing the cache and asking a host
+//! for a bundle are [`Effect`]s that `tui/mod.rs` runs. The registry lists
+//! the plugins (`Source::Plugin`) so the extensions screen shows and toggles
+//! them; turning one off is saved as `[extensions] <name> = false` and
+//! applies to new sessions only.
+
+use std::collections::{BTreeMap, BTreeSet, HashMap};
+
+use crate::proto::Msg;
+use crate::proxy::plugins::{Plugin, PluginList};
+
+use super::app::{App, Effect, ReplyTo};
+use super::extensions::Source;
+
+/// How often every profile's list is fetched again.
+pub const REFRESH_SECS: u64 = 15 * 60;
+
+/// How a profile's list was last fetched.
+#[derive(Debug, Clone, Copy, PartialEq, Eq)]
+pub enum ListState {
+    /// From the proxy, this run.
+    Fresh,
+    /// From the cache; the proxy did not answer (or is too old).
+    Unreachable,
+}
+
+/// What a fetch of a profile's list gave (`tui/mod.rs` has already saved
+/// or dropped the cache entry to match).
+#[derive(Debug, Clone, PartialEq)]
+pub enum PluginsFetch {
+    Listed(PluginList),
+    /// 401 or 403: the token is no longer valid; its cached list is gone.
+    SignedOut,
+    /// Any other failure, or a proxy without plugins (404): the cached list
+    /// (if any) stays.
+    Unreachable,
+}
+
+/// What a host answered for one plugin's bundle.
+#[derive(Debug, Clone, PartialEq)]
+pub enum BundleState {
+    /// Asked; no answer yet.
+    Pending,
+    /// The zip's path on that host.
+    Ready(String),
+    /// Claude Code there has the plugin installed: no bundle needed.
+    Installed,
+    Failed(String),
+    /// The daemon does not know `Bundle`.
+    TooOld,
+}
+
+/// The client's plugin state.
+#[derive(Debug, Default)]
+pub struct Plugins {
+    /// `[extensions]` of config.toml, kept so a plugin that appears later
+    /// gets its saved choice (and a toggle updates it).
+    pub config: BTreeMap<String, bool>,
+    /// Per profile: its list (`None` once signed out) and how it was fetched.
+    pub lists: BTreeMap<String, (PluginList, ListState)>,
+    /// The hosts whose daemon takes `Bundle` requests now.
+    pub hosts: BTreeSet<String>,
+    /// Per `(host, plugin)`: the hash asked for and the answer.
+    pub bundles: HashMap<(String, String), (String, BundleState)>,
+}
+
+impl App {
+    /// The admitted plugin `name` of `profile`'s list, if the registry has
+    /// it (not refused) and it is on.
+    fn enabled_plugin(&self, profile: &str, name: &str) -> Option<&Plugin> {
+        let on = self.extensions.extensions.iter().any(|e| {
+            e.name == name
+                && e.enabled
+                && matches!(&e.source, Source::Plugin { profile: p, refused: None } if p == profile)
+        });
+        let (list, _) = self.plugins.lists.get(profile)?;
+        list.plugins.iter().find(|p| on && p.name == name)
+    }
+
+    /// The enabled plugins of `profile`, in registry order.
+    pub(super) fn enabled_plugins(&self, profile: &str) -> Vec<&Plugin> {
+        self.extensions
+            .extensions
+            .iter()
+            .filter_map(|e| self.enabled_plugin(profile, &e.name))
+            .collect()
+    }
+
+    /// Every enabled plugin of every profile, in registry order.
+    fn all_enabled_plugins(&self) -> Vec<&Plugin> {
+        let lists = self.plugins.lists.keys();
+        lists
+            .flat_map(|profile| self.enabled_plugins(profile))
+            .collect()
+    }
+
+    /// The env a session on `profile` gets from its enabled plugins.
+    pub(super) fn plugin_env(&self, profile: &str) -> Vec<(String, String)> {
+        let plugins = self.enabled_plugins(profile);
+        plugins
+            .into_iter()
+            .flat_map(|p| p.env.iter().cloned())
+            .collect()
+    }
+
+    /// A list arrived (or failed): keep it, list it, and ask the connected
+    /// hosts for the bundles it changed.
+    pub fn on_plugins(&mut self, profile: &str, fetch: PluginsFetch) {
+        match fetch {
+            PluginsFetch::Listed(list) => {
+                self.plugins
+                    .lists
+                    .insert(profile.to_owned(), (list, ListState::Fresh));
+            }
+            PluginsFetch::SignedOut => {
+                if self.plugins.lists.remove(profile).is_some() {
+                    self.notify(format!(
+                        "plugins of {profile}: signed out, run `claudio proxy login`"
+                    ));
+                }
+            }
+            PluginsFetch::Unreachable => {
+                if let Some((_, state)) = self.plugins.lists.get_mut(profile) {
+                    *state = ListState::Unreachable;
+                }
+            }
+        }
+        self.list_plugins();
+        let hosts: Vec<String> = self.plugins.hosts.iter().cloned().collect();
+        for host in hosts {
+            self.request_bundles(&host);
+        }
+        self.redraw = true;
+    }
+
+    /// Put every profile's plugins in the registry, with the saved choices.
+    pub(super) fn list_plugins(&mut self) {
+        let lists: Vec<&PluginList> = self.plugins.lists.values().map(|(l, _)| l).collect();
+        self.extensions.set_plugins(&lists, &self.plugins.config);
+    }
+
+    /// `host`'s daemon is connected: ask it for every enabled bundle.
+    pub fn on_plugin_host(&mut self, host: &str) {
+        self.plugins.hosts.insert(host.to_owned());
+        // A new connection may be a new daemon: ask again.
+        self.plugins.bundles.retain(|(h, _), _| h != host);
+        self.request_bundles(host);
+    }
+
+    /// `host`'s daemon is gone: its answers no longer hold.
+    pub(super) fn plugin_host_gone(&mut self, host: &str) {
+        self.plugins.hosts.remove(host);
+        self.plugins.bundles.retain(|(h, _), _| h != host);
+    }
+
+    /// Ask `host` for each enabled bundle it has not answered for at its
+    /// current hash.
+    pub(super) fn request_bundles(&mut self, host: &str) {
+        let wanted: Vec<Plugin> = self.all_enabled_plugins().into_iter().cloned().collect();
+        for p in wanted {
+            let key = (host.to_owned(), p.name.clone());
+            if self
+                .plugins
+                .bundles
+                .get(&key)
+                .is_some_and(|(sha, _)| *sha == p.bundle_sha256)
+            {
+                continue;
+            }
+            let sha = p.bundle_sha256.clone();
+            self.plugins
+                .bundles
+                .insert(key, (sha.clone(), BundleState::Pending));
+            let to = ReplyTo::Bundle {
+                host: host.to_owned(),
+                name: p.name.clone(),
+                sha256: sha.clone(),
+            };
+            self.request(host, bundle_msg(&p), to);
+        }
+    }
+
+    /// `host`'s answer for plugin `name` at hash `sha256`.
+    pub(super) fn on_bundled(
+        &mut self,
+        host: &str,
+        name: &str,
+        sha256: &str,
+        reply: std::io::Result<Msg>,
+    ) {
+        let key = (host.to_owned(), name.to_owned());
+        // An answer for a hash since replaced, or a host since gone.
+        if self
+            .plugins
+            .bundles
+            .get(&key)
+            .is_none_or(|(sha, _)| sha != sha256)
+        {
+            return;
+        }
+        let state = match reply {
+            Ok(Msg::Bundled {
+                installed: true, ..
+            }) => BundleState::Installed,
+            Ok(Msg::Bundled {
+                path: Some(path), ..
+            }) => BundleState::Ready(path),
+            Ok(other) => BundleState::Failed(format!("unexpected reply {other:?}")),
+            Err(e) if App::older_daemon_notice(host, &e).is_some() => BundleState::TooOld,
+            Err(e) => {
+                let why = e.to_string();
+                let prefix = format!("bundle {name}: ");
+                BundleState::Failed(why.strip_prefix(&prefix).unwrap_or(&why).to_owned())
+            }
+        };
+        self.plugins.bundles.insert(key, (sha256.to_owned(), state));
+        self.redraw = true;
+    }
+
+    /// The bundle state of plugin `name` on `host`, if asked.
+    pub fn bundle_state(&self, host: &str, name: &str) -> Option<&BundleState> {
+        let key = (host.to_owned(), name.to_owned());
+        self.plugins.bundles.get(&key).map(|(_, state)| state)
+    }
+
+    /// What the extensions screen says about plugin `name` of `profile`
+    /// (beyond its description): one line per host that was asked, and one
+    /// for the list.
+    pub(super) fn plugin_details(
+        &self,
+        name: &str,
+        profile: &str,
+        refused: Option<&str>,
+    ) -> Vec<String> {
+        let mut lines = vec!["turning it on or off applies to new sessions".to_owned()];
+        if let Some(why) = refused {
+            lines.push(format!("refused: {why}"));
+            return lines;
+        }
+        let mut hosts: Vec<&String> = self.plugins.hosts.iter().collect();
+        hosts.sort_by_key(|h| (h.as_str() != "local", h.as_str()));
+        for host in hosts {
+            let Some(state) = self.bundle_state(host, name) else {
+                continue;
+            };
+            let text = match state {
+                BundleState::Pending => "downloading".to_owned(),
+                BundleState::Ready(_) => "ready".to_owned(),
+                BundleState::Installed => "installed in Claude Code (bundle skipped)".to_owned(),
+                BundleState::Failed(why) if why.starts_with("download failed") => why.clone(),
+                BundleState::Failed(why) => format!("download failed: {why}"),
+                BundleState::TooOld => "daemon too old".to_owned(),
+            };
+            lines.push(format!("{host}: {text}"));
+        }
+        if let Some((list, ListState::Unreachable)) = self.plugins.lists.get(profile) {
+            let age = super::fmt::fmt_age(self.now.saturating_sub(list.fetched_at));
+            lines.push(format!("proxy unreachable, cached {age}"));
+        }
+        lines
+    }
+
+    /// `r` on the extensions screen: fetch every known profile's list again.
+    pub(super) fn refresh_plugins(&mut self) {
+        let mut profiles: BTreeSet<String> = self.plugins.lists.keys().cloned().collect();
+        profiles.extend(self.plugin_profiles());
+        for profile in profiles {
+            self.emit(Effect::FetchPlugins { profile });
+        }
+        self.notify("refreshing plugins…");
+    }
+
+    /// The profiles sessions use: the one new sessions get, and every open
+    /// session's.
+    pub fn plugin_profiles(&self) -> BTreeSet<String> {
+        let mut profiles: BTreeSet<String> = self
+            .sessions
+            .iter()
+            .filter_map(|v| v.proxy.clone())
+            .collect();
+        if let Some(p) = self.proxy_override.pick(self.proxy_default.as_deref()) {
+            profiles.insert(p.to_owned());
+        }
+        profiles
+    }
+}
+
+/// The `Bundle` request for `p`.
+pub fn bundle_msg(p: &Plugin) -> Msg {
+    Msg::Bundle {
+        name: p.name.clone(),
+        url: p.bundle_url.clone(),
+        sha256: p.bundle_sha256.clone(),
+        installed_id: p.installed_id.clone(),
+    }
+}
+
+#[cfg(test)]
+mod tests {
+    use super::*;
+    use crate::proxy::plugins::sample;
+    use crate::tui::test_support::app_with;
+
+    fn list(profile: &str, plugins: Vec<Plugin>) -> PluginList {
+        PluginList {
+            profile: profile.into(),
+            fetched_at: 100,
+            plugins,
+        }
+    }
+
+    /// The `Bundle` requests queued, as (host, name).
+    fn bundle_requests(app: &mut App) -> Vec<(String, String)> {
+        app.take_effects()
+            .into_iter()
+            .filter_map(|e| match e {
+                Effect::Request {
+                    host,
+                    msg: Msg::Bundle { name, .. },
+                    ..
+                } => Some((host, name)),
+                _ => None,
+            })
+            .collect()
+    }
+
+    fn pair(host: &str, name: &str) -> (String, String) {
+        (host.to_owned(), name.to_owned())
+    }
+
+    #[test]
+    fn a_list_is_listed_and_its_bundles_asked_of_every_connected_host() {
+        let mut app = app_with(&[]);
+        app.on_plugin_host("local");
+        assert!(bundle_requests(&mut app).is_empty(), "no list yet");
+        app.on_plugins(
+            "work",
+            PluginsFetch::Listed(list("work", vec![sample("kb")])),
+        );
+        assert_eq!(bundle_requests(&mut app), [pair("local", "kb")]);
+        let row = app.extensions.extensions.last().unwrap();
+        assert_eq!(
+            (row.name.as_str(), row.description.as_str()),
+            ("kb", "1.4.0 · work")
+        );
+        // A second host gets asked on connect; the same list again asks no one.
+        app.on_plugin_host("devbox");
+        assert_eq!(bundle_requests(&mut app), [pair("devbox", "kb")]);
+        app.on_plugins(
+            "work",
+            PluginsFetch::Listed(list("work", vec![sample("kb")])),
+        );
+        assert!(bundle_requests(&mut app).is_empty());
+        // A new version is asked again.
+        let mut v2 = sample("kb");
+        v2.bundle_sha256 = "b".repeat(64);
+        app.on_plugins("work", PluginsFetch::Listed(list("work", vec![v2])));
+        let mut asked = bundle_requests(&mut app);
+        asked.sort();
+        assert_eq!(asked, [pair("devbox", "kb"), pair("local", "kb")]);
+    }
+
+    #[test]
+    fn answers_set_each_hosts_state_and_stale_ones_are_dropped() {
+        let mut app = app_with(&[]);
+        app.on_plugin_host("local");
+        app.on_plugins(
+            "work",
+            PluginsFetch::Listed(list("work", vec![sample("kb")])),
+        );
+        app.take_effects();
+        let sha = "a".repeat(64);
+        assert_eq!(app.bundle_state("local", "kb"), Some(&BundleState::Pending));
+        let ready = Msg::Bundled {
+            name: "kb".into(),
+            path: Some("/c/kb.zip".into()),
+            installed: false,
+        };
+        app.on_bundled("local", "kb", &"b".repeat(64), Ok(ready.clone()));
+        assert_eq!(
+            app.bundle_state("local", "kb"),
+            Some(&BundleState::Pending),
+            "stale"
+        );
+        app.on_bundled("local", "kb", &sha, Ok(ready));
+        assert_eq!(
+            app.bundle_state("local", "kb"),
+            Some(&BundleState::Ready("/c/kb.zip".into()))
+        );
+        let installed = Msg::Bundled {
+            name: "kb".into(),
+            path: None,
+            installed: true,
+        };
+        app.on_bundled("local", "kb", &sha, Ok(installed));
+        assert_eq!(
+            app.bundle_state("local", "kb"),
+            Some(&BundleState::Installed)
+        );
+        let old = std::io::Error::other("unsupported op: bundle");
+        app.on_bundled("local", "kb", &sha, Err(old));
+        assert_eq!(app.bundle_state("local", "kb"), Some(&BundleState::TooOld));
+        let failed = std::io::Error::other("bundle kb: hash mismatch");
+        app.on_bundled("local", "kb", &sha, Err(failed));
+        assert_eq!(
+            app.bundle_state("local", "kb"),
+            Some(&BundleState::Failed("hash mismatch".into()))
+        );
+        // A host that left forgets its answers.
+        app.plugin_host_gone("local");
+        assert_eq!(app.bundle_state("local", "kb"), None);
+    }
+
+    #[test]
+    fn env_comes_from_enabled_admitted_plugins_of_the_profile() {
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
+        let pair = |k: &str, v: &str| (k.to_owned(), v.to_owned());
+        assert_eq!(
+            app.plugin_env("work"),
+            [pair("KB_TOKEN", "s3cret"), pair("OTHER_TOKEN", "o")]
+        );
+        assert!(app.plugin_env("nope").is_empty());
+        // Off in the registry: no env.
+        app.plugins.config.insert("kb".into(), false);
+        app.list_plugins();
+        assert_eq!(app.plugin_env("work"), [pair("OTHER_TOKEN", "o")]);
+    }
+
+    #[test]
+    fn signed_out_drops_the_list_and_unreachable_keeps_it() {
+        let mut app = app_with(&[]);
+        app.on_plugins(
+            "work",
+            PluginsFetch::Listed(list("work", vec![sample("kb")])),
+        );
+        app.on_plugins("work", PluginsFetch::Unreachable);
+        assert_eq!(app.plugins.lists["work"].1, ListState::Unreachable);
+        assert_eq!(app.plugin_env("work").len(), 1);
+        app.now = 100 + 180;
+        let details = app.plugin_details("kb", "work", None);
+        assert_eq!(details.last().unwrap(), "proxy unreachable, cached 3m");
+        app.on_plugins("work", PluginsFetch::SignedOut);
+        assert!(app.plugin_env("work").is_empty());
+        assert!(!app.extensions.extensions.iter().any(|e| e.name == "kb"));
+        let notice = app.notice.as_ref().unwrap().text.clone();
+        assert_eq!(
+            notice,
+            "plugins of work: signed out, run `claudio proxy login`"
+        );
+        // Unreachable with no cache: nothing to list.
+        app.on_plugins("home", PluginsFetch::Unreachable);
+        assert!(!app.plugins.lists.contains_key("home"));
+    }
+}
diff --git a/tests/manager.rs b/tests/manager.rs
index b7165e7..ad2681b 100644
--- a/tests/manager.rs
+++ b/tests/manager.rs
@@ -2698,7 +2698,11 @@ fn test_extensions_screen_turns_extensions_off_and_saves_it() {
     wizard_pick_dir(&mut tui, &h.dirs[0]);
 
     tui.send_keys(ALT_I);
-    tui.wait_for("Extensions (Space toggle · Esc close)", Region::Screen, WAIT);
+    tui.wait_for(
+        "Extensions (Space toggle · r refresh · Esc close)",
+        Region::Screen,
+        WAIT,
+    );
     let shown = tui.screen_text(Region::Screen);
     for row in [
         "[x] latex     built-in  —              <latex>",
@@ -2743,7 +2747,11 @@ fn test_extensions_screen_turns_extensions_off_and_saves_it() {
 
     // `e` opens the screen too.
     tui.send_keys(b"e");
-    tui.wait_for("Extensions (Space toggle · Esc close)", Region::Screen, WAIT);
+    tui.wait_for(
+        "Extensions (Space toggle · r refresh · Esc close)",
+        Region::Screen,
+        WAIT,
+    );
     tui.send_keys(ALT_I);
     tui.wait_until(|s| !s.contains("Space toggle", Region::Screen), WAIT);
     tui.send_keys(ALT_Q);
~~~~~~

Run: `rustfmt --edition 2021 --check src/tui/plugins.rs src/tui/extensions_view.rs` → no output
(`src/tui/extensions/mod.rs` was formatted with its `mod` lines stubbed; its submodules are clean).
Run: `umask 022; cargo test --locked --bin claudio` → `test result: ok. 1031 passed; 0 failed`
Run: `umask 022; cargo test --test manager test_extensions_screen -- --test-threads=1` → `test result: ok. 1 passed`
Dev build: 9 dead-code warnings (`env::merge`, `PluginList::{admitted,summary}`, `plugins::{parse,load_cache,load_all,…}`, `App::{plugin_env,on_plugin_host}`, `REFRESH_SECS`); Tasks 5–7 use them.

Step 3: commit
`/usr/bin/git add src/tui/plugins.rs`
`/usr/bin/git commit -m "feat(tui): list proxy plugins in the extensions screen" -- src/tui/plugins.rs src/tui/extensions/mod.rs src/tui/extensions_view.rs src/tui/app.rs src/tui/interaction.rs src/tui/mod.rs tests/manager.rs`

Interfaces this task provides to later tasks:
- `extensions::Source::Plugin { profile, refused }`, `Extension::is_plugin`, `Registry::set_plugins(&[&PluginList], &BTreeMap<String, bool>)`.
- `tui::plugins::{Plugins, ListState, PluginsFetch, BundleState, REFRESH_SECS, bundle_msg}`; `App.plugins`; `App::{on_plugins, list_plugins, on_plugin_host, plugin_host_gone, request_bundles, on_bundled, bundle_state, enabled_plugins, plugin_env, plugin_details, refresh_plugins, plugin_profiles}`.
- `Effect::FetchPlugins { profile }` (runner in `tui/mod.rs`: fetch, save or drop the cache, `HostEvent::Plugins`), `HostEvent::Notice(String)`, `ReplyTo::Bundle { host, name, sha256 }`.


### Task 5: fetch at start and every 15 minutes, `Bundle` per connected host
Model: standard   Repo: /volumes/repos/claudio   Files: src/tui/mod.rs, src/tui/plugins.rs, src/proxy/plugins.rs
Goal: `tui/mod.rs` loads the cached lists of the configured profiles at start, fetches the lists of the profiles in use now and every 15 min, and asks each host for the bundles on connect and reconnect; `[extensions]` names of cached plugins no longer give the "no extension named" notice (spec §2, §4).
Patch: .delivery/2026-11-06-session-plugins/patches/task-5.patch

Step 1: write the failing test
The test is in the patch below: `src/tui/plugins.rs`
`cached_lists_are_listed_at_start_and_every_profile_in_use_is_fetched` (cached state shows no
"unreachable" line; `fetch_plugin_lists` asks every listed and in-use profile). `parse` in
`src/proxy/plugins.rs` becomes test-only (`list_of`), with its test adapted.
Run: `cargo test --locked --bin claudio tui::plugins`
Expected: the build fails (`load_plugins`, `fetch_plugin_lists`, `ListState::Cached` not found).

Step 2: implement
`/usr/bin/git apply .delivery/2026-11-06-session-plugins/patches/task-5.patch`

~~~~~~diff
diff --git a/src/proxy/plugins.rs b/src/proxy/plugins.rs
index 068b20a..1beb14d 100644
--- a/src/proxy/plugins.rs
+++ b/src/proxy/plugins.rs
@@ -258,12 +258,7 @@ struct ReplyBundle {
     sha256: String,
 }
 
-/// The list in a `/v1/claudio/plugins` body, for profile `profile` at `now`.
-pub fn parse(body: &[u8], profile: &str, now: u64) -> Result<PluginList, String> {
-    let reply: Reply = serde_json::from_slice(body).map_err(|_| "bad plugins reply".to_owned())?;
-    Ok(list_of(reply, profile, now))
-}
-
+/// The list in a `/v1/claudio/plugins` reply, for profile `profile` at `now`.
 fn list_of(reply: Reply, profile: &str, now: u64) -> PluginList {
     let plugins = reply
         .plugins
@@ -543,7 +538,8 @@ mod tests {
             "bundle":{"url":"https://e.org/v.zip","sha256":"ab"},
             "installed_id":"vocdoni-internal@vocdoni",
             "env":{"ZED":"z","KB_TOKEN":"k"}}]}"#;
-        let list = parse(body, "work", 7).unwrap();
+        let parse = |body: &[u8]| serde_json::from_slice(body).map(|r| list_of(r, "work", 7));
+        let list = parse(body).unwrap();
         assert_eq!(list.profile, "work");
         assert_eq!(list.fetched_at, 7);
         let p = &list.plugins[0];
@@ -556,8 +552,8 @@ mod tests {
                 ("ZED".to_owned(), "z".to_owned())
             ]
         );
-        assert!(parse(b"not json", "work", 7).is_err());
-        assert!(parse(b"{}", "work", 7).unwrap().plugins.is_empty());
+        assert!(parse(b"not json").is_err());
+        assert!(parse(b"{}").unwrap().plugins.is_empty());
     }
 
     #[tokio::test]
diff --git a/src/tui/mod.rs b/src/tui/mod.rs
index 6e35577..6e8e009 100644
--- a/src/tui/mod.rs
+++ b/src/tui/mod.rs
@@ -264,10 +264,27 @@ async fn main(launch: Launch) -> ExitCode {
         }
     };
     let (proxy_profiles, proxy_default) = crate::proxy::resolve::load_proxy_profiles();
+    // The plugin lists cached for the profiles there are; fetched again
+    // once the loop runs.
+    let cached_plugins: Vec<_> = crate::proxy::plugins::load_all()
+        .into_iter()
+        .filter(|l| proxy_profiles.contains(&l.profile))
+        .collect();
     // External extensions are read once, at start.
     let external = extensions::manifest::load(&extensions::manifest::dir(), &mut key_notices);
+    // A plugin's saved choice is no unknown name: the plugins come later.
+    let builtin_choices: std::collections::BTreeMap<String, bool> = config
+        .extensions
+        .iter()
+        .filter(|(name, _)| {
+            !cached_plugins
+                .iter()
+                .any(|l| l.plugins.iter().any(|p| &p.name == *name))
+        })
+        .map(|(name, on)| (name.clone(), *on))
+        .collect();
     let registry = extensions::Registry::new(
-        &config.extensions,
+        &builtin_choices,
         config.claude.latex,
         external,
         &mut key_notices,
@@ -302,7 +319,9 @@ async fn main(launch: Launch) -> ExitCode {
         },
     });
     app.extensions = registry;
+    app.load_plugins(cached_plugins, config.extensions.clone());
     app.set_host_features("local", &local_features);
+    app.on_plugin_host("local");
     let start = match plain {
         Some(start) => Start::Plain(start),
         None => Start::Manager { saved, live },
@@ -1006,6 +1025,9 @@ async fn event_loop(
     let mut events = EventStream::new();
     let mut tick = tokio::time::interval(TICK);
     tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
+    // The plugin lists: now, then every 15 minutes.
+    let mut plugins_tick = tokio::time::interval(Duration::from_secs(plugins::REFRESH_SECS));
+    plugins_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
     // 60-second proxy stats refresh.
     let mut proxy_tick = tokio::time::interval(Duration::from_secs(60));
     proxy_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
@@ -1088,6 +1110,7 @@ async fn event_loop(
                     crate::remote::hosts::touch(&host);
                     app.on_ssh_hosts(crate::remote::hosts::candidates());
                     app.on_host_connected(&host, &home);
+                    app.on_plugin_host(&host);
                     // Recover this host's sessions only.
                     app.recover_host(&host, &sessions);
                     app.subscribe_host_stats(&host);
@@ -1121,6 +1144,7 @@ async fn event_loop(
                     app.set_local_claude(client_claude(&client));
                     app.set_host_features("local", &client.welcome().features);
                     conns.connected("local", client);
+                    app.on_plugin_host("local");
                     app.on_reconnected_local(&sessions, home);
                 }
                 Some(HostEvent::LocalReconnectFailed { generation }) => {
@@ -1215,6 +1239,7 @@ async fn event_loop(
                     app.schedule_proxy_stats(&name, vec![stats_view::Window::H24]);
                 }
             }
+            _ = plugins_tick.tick() => app.fetch_plugin_lists(),
             _ = outdated_tick.tick() => {
                 for (host, conn) in &conns.map {
                     if let Some(client) = conn.client.as_ref().filter(|c| c.daemon_outdated()) {
diff --git a/src/tui/plugins.rs b/src/tui/plugins.rs
index fe3775c..ad0e2df 100644
--- a/src/tui/plugins.rs
+++ b/src/tui/plugins.rs
@@ -22,6 +22,8 @@ pub const REFRESH_SECS: u64 = 15 * 60;
 /// How a profile's list was last fetched.
 #[derive(Debug, Clone, Copy, PartialEq, Eq)]
 pub enum ListState {
+    /// Read from `plugins.json` at start; the first fetch is under way.
+    Cached,
     /// From the proxy, this run.
     Fresh,
     /// From the cache; the proxy did not answer (or is too old).
@@ -266,12 +268,30 @@ impl App {
 
     /// `r` on the extensions screen: fetch every known profile's list again.
     pub(super) fn refresh_plugins(&mut self) {
+        self.fetch_plugin_lists();
+        self.notify("refreshing plugins…");
+    }
+
+    /// Fetch the list of every profile in use or listed (at start, then
+    /// every [`REFRESH_SECS`]).
+    pub fn fetch_plugin_lists(&mut self) {
         let mut profiles: BTreeSet<String> = self.plugins.lists.keys().cloned().collect();
         profiles.extend(self.plugin_profiles());
         for profile in profiles {
             self.emit(Effect::FetchPlugins { profile });
         }
-        self.notify("refreshing plugins…");
+    }
+
+    /// The lists read from `plugins.json` at start, and `[extensions]`.
+    pub fn load_plugins(&mut self, cached: Vec<PluginList>, config: BTreeMap<String, bool>) {
+        self.plugins.config = config;
+        for list in cached {
+            let profile = list.profile.clone();
+            self.plugins
+                .lists
+                .insert(profile, (list, ListState::Cached));
+        }
+        self.list_plugins();
     }
 
     /// The profiles sessions use: the one new sessions get, and every open
@@ -442,6 +462,39 @@ mod tests {
         assert_eq!(app.plugin_env("work"), [pair("OTHER_TOKEN", "o")]);
     }
 
+    #[test]
+    fn cached_lists_are_listed_at_start_and_every_profile_in_use_is_fetched() {
+        let mut app = app_with(&[]);
+        app.proxy_default = Some("work".into());
+        let choices = BTreeMap::from([("kb".to_owned(), false)]);
+        app.load_plugins(
+            vec![list("home", vec![sample("kb"), sample("mine")])],
+            choices,
+        );
+        let rows: Vec<(&str, bool)> = app
+            .extensions
+            .extensions
+            .iter()
+            .filter(|e| e.is_plugin())
+            .map(|e| (e.name.as_str(), e.enabled))
+            .collect();
+        assert_eq!(rows, [("kb", false), ("mine", true)]);
+        // Read from the cache, the first fetch under way: no "unreachable".
+        app.now = 1000;
+        assert_eq!(app.plugin_details("mine", "home", None).len(), 1);
+        app.take_effects();
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
+    }
+
     #[test]
     fn signed_out_drops_the_list_and_unreachable_keeps_it() {
         let mut app = app_with(&[]);
~~~~~~

Run: `rustfmt --edition 2021 --check src/tui/plugins.rs src/proxy/plugins.rs` → no output.
Run: `umask 022; cargo test --locked --bin claudio` → `test result: ok. 1032 passed; 0 failed`
Dev build: 5 dead-code warnings (`env::merge`, `PluginList::{admitted,summary}`, `load_cache`,
`load_cache_from`, `App::plugin_env`); Tasks 6–7 use them.

Step 3: commit
`/usr/bin/git commit -m "feat(tui): fetch plugin lists and ask each host for bundles" -- src/proxy/plugins.rs src/tui/mod.rs src/tui/plugins.rs`

Interfaces this task provides to later tasks:
- `App::{load_plugins(Vec<PluginList>, BTreeMap<String, bool>), fetch_plugin_lists()}`, `ListState::Cached`.
- Start-up order in `tui::main`: `load_plugins` → `set_host_features("local")` → `on_plugin_host("local")`; `on_plugin_host` on `Connected` and `LocalReconnected`; `plugins_tick` (immediate, then every `REFRESH_SECS`).


### Task 6: spawn with bundles and env (manager, `--plain`, reset)
Model: standard   Repo: /volumes/repos/claudio   Files: src/proto.rs, src/proxy/api.rs, src/tui/app.rs, src/tui/sessions.rs, src/tui/mod.rs, src/tui/plugins.rs
Goal: a claude spawn with a proxy profile gets `--plugin-dir <path>` per ready bundle and the plugin env merged under the proxy env; a bundle not answered yet defers the spawn (≤ 20 s, notice); `Alt+e` sends the current env and no args; `--plain` takes the same path (spec §5).
Patch: .delivery/2026-11-06-session-plugins/patches/task-6.patch

Step 1: write the failing test
The tests are in the patch below: `src/proto.rs` `merge_env_keeps_plugin_keys_the_proxy_does_not_set`;
`src/tui/plugins.rs` `a_spawn_gets_the_ready_bundle_and_the_plugin_env`,
`a_spawn_waits_for_a_bundle_not_answered_yet` (deferred with `bundles: [kb]`, notice "fetching plugin kb…"),
`installed_failed_or_off_plugins_bring_no_bundle`, `a_reset_sends_the_current_plugin_env_and_no_args`.
Run: `cargo test --locked --bin claudio tui::plugins`
Expected: the build fails (`merge_env`, `with_plugin_dirs`, field `bundles` of `Effect::SpawnWithProxy` not found).

Step 2: implement
`/usr/bin/git apply .delivery/2026-11-06-session-plugins/patches/task-6.patch`

~~~~~~diff
diff --git a/src/proto.rs b/src/proto.rs
index c430d07..6e03f2a 100644
--- a/src/proto.rs
+++ b/src/proto.rs
@@ -561,6 +561,29 @@ impl Msg {
         }
         self
     }
+
+    /// `self` with the proxy env added after the plugin env it carries
+    /// ([`crate::proxy::env::merge`]: the proxy wins, no key twice).
+    pub fn merge_env(self, proxy: Vec<(String, String)>) -> Msg {
+        let plugin = match &self {
+            Msg::Spawn(spec) => spec.env.clone(),
+            Msg::Respawn(spec) => spec.env.clone(),
+            _ => return self,
+        };
+        self.with_env(crate::proxy::env::merge(&plugin, proxy))
+    }
+
+    /// `self` with `--plugin-dir <path>` added for each of `paths`, if it is
+    /// a `Spawn` (a `Respawn` keeps the journaled args).
+    pub fn with_plugin_dirs(mut self, paths: &[String]) -> Msg {
+        if let Msg::Spawn(spec) = &mut self {
+            for path in paths {
+                spec.args.push("--plugin-dir".to_owned());
+                spec.args.push(path.clone());
+            }
+        }
+        self
+    }
 }
 
 impl std::fmt::Debug for RespawnSpec {
@@ -1228,6 +1251,34 @@ mod tests {
         );
     }
 
+    #[test]
+    fn merge_env_keeps_plugin_keys_the_proxy_does_not_set() {
+        let pair = |k: &str, v: &str| (k.to_owned(), v.to_owned());
+        let spawn = Msg::Spawn(SpawnSpec {
+            id: Uuid::nil(),
+            cwd: "/w".into(),
+            name: None,
+            args: vec!["--resume".into(), "c1".into()],
+            env: vec![pair("KB_TOKEN", "kb"), pair("API_TIMEOUT_MS", "1")],
+            rows: 24,
+            cols: 80,
+            ephemeral: false,
+            extensions: None,
+        });
+        let merged = spawn
+            .merge_env(vec![pair("API_TIMEOUT_MS", "30000")])
+            .with_plugin_dirs(&["/c/kb.zip".to_owned()]);
+        let Msg::Spawn(spec) = merged else {
+            panic!("a spawn")
+        };
+        assert_eq!(
+            spec.env,
+            [pair("KB_TOKEN", "kb"), pair("API_TIMEOUT_MS", "30000")]
+        );
+        assert_eq!(spec.args, ["--resume", "c1", "--plugin-dir", "/c/kb.zip"]);
+        assert_eq!(Msg::Ping.merge_env(vec![pair("A", "b")]), Msg::Ping);
+    }
+
     #[test]
     fn respawn_roundtrips_and_keeps_secrets_out_of_debug() {
         let respawn = Msg::Respawn(RespawnSpec {
diff --git a/src/proxy/api.rs b/src/proxy/api.rs
index cea24b8..1fa6ad1 100644
--- a/src/proxy/api.rs
+++ b/src/proxy/api.rs
@@ -31,7 +31,7 @@ pub enum ApiError {
 // ── Response types ────────────────────────────────────────────────────────────
 
 /// `GET /v1/claudio/config` — recommended env vars for spawning claude.
-#[derive(Debug, Default, Deserialize)]
+#[derive(Debug, Clone, Default, Deserialize)]
 #[serde(default)]
 pub struct ConfigResponse {
     pub version: u32,
diff --git a/src/tui/app.rs b/src/tui/app.rs
index f1f6c4c..256d4d8 100644
--- a/src/tui/app.rs
+++ b/src/tui/app.rs
@@ -131,17 +131,22 @@ pub enum Effect {
         claude_session_id: String,
     },
     /// Spawn (or respawn) a session with proxy config: the effect runner
-    /// fetches the proxy config (with a 5 s timeout), builds the env, then
-    /// sends the request. This avoids silently using fallback model defaults
-    /// when the proxy-config cache is cold.
+    /// fetches the proxy config (with a 5 s timeout) unless it is cached,
+    /// waits up to 20 s for `bundles` (asking `host` for each), builds the
+    /// env, then sends the request. This avoids silently using fallback
+    /// model defaults when the proxy-config cache is cold, and starting a
+    /// session without a plugin whose bundle is a moment away.
     SpawnWithProxy {
         host: String,
-        /// A `Spawn` or `Respawn` whose `env` is empty; the runner fills it.
+        /// A `Spawn` or `Respawn` whose `env` holds the plugin env only;
+        /// the runner adds the proxy env, and the ready bundles' paths.
         msg: Msg,
         proxy_name: String,
         /// `--first-party` for this session (see `SessionView::first_party`).
         first_party: bool,
         to: ReplyTo,
+        /// The plugins whose bundle `host` has not answered for yet.
+        bundles: Vec<PendingBundle>,
     },
     /// Put this text on the outer terminal's clipboard (OSC 52).
     CopyToClipboard(String),
@@ -195,6 +200,15 @@ pub enum Effect {
     FetchPlugins { profile: String },
 }
 
+/// A plugin bundle a deferred spawn waits for: its `Bundle` request, and
+/// how the answer reaches [`App::on_reply`].
+#[derive(Debug, Clone, PartialEq)]
+pub struct PendingBundle {
+    pub name: String,
+    pub msg: Msg,
+    pub to: ReplyTo,
+}
+
 /// What a request's reply is for.
 #[derive(Debug, Clone, PartialEq)]
 pub enum ReplyTo {
diff --git a/src/tui/mod.rs b/src/tui/mod.rs
index 6e8e009..8ef77a6 100644
--- a/src/tui/mod.rs
+++ b/src/tui/mod.rs
@@ -1508,21 +1508,37 @@ fn run_effect(
             proxy_name,
             first_party,
             to,
+            bundles,
         } => {
             // Resolve the client now (synchronous, cheap) so we can move it
             // into the async task. Client is Clone + Send + 'static.
             let client = conns.client(&host).cloned();
+            let cached = app.proxy_config_cached(&proxy_name).cloned();
             let tx = reply_tx.clone();
+            let notices = ev_tx.clone();
             tokio::spawn(async move {
                 // Fetch proxy config with a 5 s timeout so the first spawn
-                // uses the real model/rate-limit overrides, not fallback defaults.
-                let cfg = tokio::time::timeout(
-                    Duration::from_secs(5),
-                    fetch_proxy_config(&proxy_name),
-                )
-                .await
-                .ok()
-                .flatten();
+                // uses the real model/rate-limit overrides, not fallback
+                // defaults; meanwhile, wait for the plugin bundles.
+                let config = async {
+                    match cached {
+                        Some(cfg) => Some(cfg),
+                        None => tokio::time::timeout(
+                            Duration::from_secs(5),
+                            fetch_proxy_config(&proxy_name),
+                        )
+                        .await
+                        .ok()
+                        .flatten(),
+                    }
+                };
+                let waits = wait_bundles(client.as_ref(), bundles, &tx);
+                let (cfg, (dirs, left_out)) = tokio::join!(config, waits);
+                if !left_out.is_empty() {
+                    let names = left_out.join(", ");
+                    let text = format!("plugin {names} left out: not ready on {host}");
+                    let _ = notices.send(HostEvent::Notice(text)).await;
+                }
 
                 // Build env from profile + (possibly fresh) config.
                 let env = crate::proxy::resolve::proxy_env_for(
@@ -1540,7 +1556,8 @@ fn run_effect(
                     Ok(env) => {
                         match client {
                             Some(c) => {
-                                let reply = c.request(msg.with_env(env));
+                                let msg = msg.with_plugin_dirs(&dirs).merge_env(env);
+                                let reply = c.request(msg);
                                 let _ = tx.send((to, reply.await));
                             }
                             None => {
@@ -1560,6 +1577,55 @@ fn run_effect(
     }
 }
 
+/// How long a spawn waits for its plugin bundles.
+const BUNDLE_WAIT: Duration = Duration::from_secs(20);
+
+/// Ask `client` for each of `bundles` and wait up to [`BUNDLE_WAIT`]: the
+/// paths of those that came back ready, and the names of those that did
+/// not (pending, failed, or no client). Every answer, even a late one, also
+/// goes to `App` through `tx`.
+async fn wait_bundles(
+    client: Option<&Client>,
+    bundles: Vec<app::PendingBundle>,
+    tx: &mpsc::UnboundedSender<(ReplyTo, io::Result<Msg>)>,
+) -> (Vec<String>, Vec<String>) {
+    let mut dirs = Vec::new();
+    let mut left_out = Vec::new();
+    let deadline = tokio::time::Instant::now() + BUNDLE_WAIT;
+    let mut waiting = Vec::new();
+    for b in bundles {
+        let Some(client) = client else {
+            left_out.push(b.name);
+            continue;
+        };
+        let reply = client.request(b.msg);
+        let (done, answer) = tokio::sync::oneshot::channel();
+        let tx = tx.clone();
+        tokio::spawn(async move {
+            let reply = reply.await;
+            // `Some(None)`: installed in Claude Code, no bundle needed.
+            let ready = match &reply {
+                Ok(Msg::Bundled {
+                    installed: true, ..
+                }) => Some(None),
+                Ok(Msg::Bundled { path: Some(p), .. }) => Some(Some(p.clone())),
+                _ => None,
+            };
+            let _ = done.send(ready);
+            let _ = tx.send((b.to, reply));
+        });
+        waiting.push((b.name, answer));
+    }
+    for (name, answer) in waiting {
+        match tokio::time::timeout_at(deadline, answer).await {
+            Ok(Ok(Some(Some(path)))) => dirs.push(path),
+            Ok(Ok(Some(None))) => {}
+            _ => left_out.push(name),
+        }
+    }
+    (dirs, left_out)
+}
+
 /// Fetch `profile`'s plugin list and keep `plugins.json` in step: a list
 /// is saved, a 401/403 drops the profile's entry (its secrets), anything
 /// else leaves it. A failed write is a notice.
diff --git a/src/tui/plugins.rs b/src/tui/plugins.rs
index ad0e2df..daac06a 100644
--- a/src/tui/plugins.rs
+++ b/src/tui/plugins.rs
@@ -495,6 +495,146 @@ mod tests {
         assert_eq!(fetched, ["home", "work"]);
     }
 
+    /// An app with `local` connected and profile `work` serving `kb`,
+    /// whose bundle `local` answered with `answer` (`None`: not yet).
+    fn app_with_kb(answer: Option<Msg>) -> App {
+        let mut app = app_with(&[]);
+        app.modal = None;
+        app.on_plugin_host("local");
+        app.on_plugins(
+            "work",
+            PluginsFetch::Listed(list("work", vec![sample("kb")])),
+        );
+        if let Some(msg) = answer {
+            app.on_bundled("local", "kb", &"a".repeat(64), Ok(msg));
+        }
+        app.take_effects();
+        app
+    }
+
+    fn ready() -> Msg {
+        Msg::Bundled {
+            name: "kb".into(),
+            path: Some("/c/plugins/kb.zip".into()),
+            installed: false,
+        }
+    }
+
+    /// Start a claude session in `/w` on `local` with `proxy`; the deferred
+    /// spawn it queues: its args, env and the bundles it waits for.
+    fn spawn_with(
+        app: &mut App,
+        proxy: Option<&str>,
+    ) -> (Vec<String>, Vec<(String, String)>, Vec<String>) {
+        let id = uuid::Uuid::new_v4();
+        let kind = crate::proto::SessionKind::Claude;
+        let proxy = proxy.map(str::to_owned);
+        app.start(
+            id,
+            "local".into(),
+            "/w".into(),
+            kind,
+            vec!["-c".into()],
+            proxy,
+            0,
+        )
+        .unwrap();
+        let effects = app.take_effects();
+        for e in effects {
+            match e {
+                Effect::SpawnWithProxy {
+                    msg: Msg::Spawn(spec),
+                    bundles,
+                    to,
+                    ..
+                } => {
+                    assert_eq!(to, ReplyTo::SpawnedDeferred(id));
+                    let names = bundles.into_iter().map(|b| b.name).collect();
+                    return (spec.args, spec.env, names);
+                }
+                Effect::Request {
+                    msg: Msg::Spawn(spec),
+                    ..
+                } => return (spec.args, spec.env, Vec::new()),
+                _ => {}
+            }
+        }
+        panic!("no spawn")
+    }
+
+    #[test]
+    fn a_spawn_gets_the_ready_bundle_and_the_plugin_env() {
+        let mut app = app_with_kb(Some(ready()));
+        let (args, env, waits) = spawn_with(&mut app, Some("work"));
+        assert_eq!(args, ["-c", "--plugin-dir", "/c/plugins/kb.zip"]);
+        assert_eq!(env, [("KB_TOKEN".to_owned(), "s3cret".to_owned())]);
+        assert!(waits.is_empty());
+        // Without a proxy: no plugin, no env.
+        let (args, env, waits) = spawn_with(&mut app, None);
+        assert_eq!(
+            (args, env.len(), waits.len()),
+            (vec!["-c".to_owned()], 0, 0)
+        );
+    }
+
+    #[test]
+    fn a_spawn_waits_for_a_bundle_not_answered_yet() {
+        let mut app = app_with_kb(None);
+        let (args, env, waits) = spawn_with(&mut app, Some("work"));
+        assert_eq!(args, ["-c"]);
+        assert_eq!(env.len(), 1);
+        assert_eq!(waits, ["kb"]);
+        let notice = app.notice.as_ref().unwrap().text.clone();
+        assert_eq!(notice, "fetching plugin kb…");
+    }
+
+    #[test]
+    fn installed_failed_or_off_plugins_bring_no_bundle() {
+        let installed = Msg::Bundled {
+            name: "kb".into(),
+            path: None,
+            installed: true,
+        };
+        let mut app = app_with_kb(Some(installed));
+        let (args, env, waits) = spawn_with(&mut app, Some("work"));
+        assert_eq!(
+            (args.len(), env.len(), waits.len()),
+            (1, 1, 0),
+            "env still applies"
+        );
+
+        let mut app = app_with_kb(None);
+        let failed = std::io::Error::other("bundle kb: hash mismatch");
+        app.on_bundled("local", "kb", &"a".repeat(64), Err(failed));
+        let (args, _, waits) = spawn_with(&mut app, Some("work"));
+        assert_eq!((args.len(), waits.len()), (1, 0));
+
+        let mut app = app_with_kb(Some(ready()));
+        app.plugins.config.insert("kb".into(), false);
+        app.list_plugins();
+        let (args, env, waits) = spawn_with(&mut app, Some("work"));
+        assert_eq!((args.len(), env.len(), waits.len()), (1, 0, 0));
+    }
+
+    #[test]
+    fn a_reset_sends_the_current_plugin_env_and_no_args() {
+        let mut app = app_with_kb(Some(ready()));
+        spawn_with(&mut app, Some("work"));
+        let id = app.sessions[0].id;
+        app.reset_session(id, false);
+        let respawn = app.take_effects().into_iter().find_map(|e| match e {
+            Effect::SpawnWithProxy {
+                msg: Msg::Respawn(spec),
+                bundles,
+                ..
+            } => Some((spec, bundles)),
+            _ => None,
+        });
+        let (spec, bundles) = respawn.expect("a respawn");
+        assert_eq!(spec.env, [("KB_TOKEN".to_owned(), "s3cret".to_owned())]);
+        assert!(bundles.is_empty());
+    }
+
     #[test]
     fn signed_out_drops_the_list_and_unreachable_keeps_it() {
         let mut app = app_with(&[]);
diff --git a/src/tui/sessions.rs b/src/tui/sessions.rs
index e5fdd9f..e5f0e92 100644
--- a/src/tui/sessions.rs
+++ b/src/tui/sessions.rs
@@ -12,10 +12,11 @@ use crate::proto::{
 };
 use crate::term::screen::Screen;
 
-use super::app::{App, Effect, Mode, ReplyTo, PROJECTS_LIMIT};
+use super::app::{App, Effect, Mode, PendingBundle, ReplyTo, PROJECTS_LIMIT};
 use super::confirm::{Choice, ConfirmAction, ConfirmPrompt};
 use super::interaction::Modal;
 use super::latex::Formula;
+use super::plugins::{bundle_msg, BundleState};
 use super::state::{self, ClientState, KillTombstone, SavedSession};
 use super::wizard::{self, Wizard};
 
@@ -424,30 +425,56 @@ impl App {
             self.request(host, Msg::SpawnShell(shell), ReplyTo::Spawned(id));
             return Ok(());
         }
+        // The profile's plugins: their env now, their bundles' paths when
+        // this host has them (else the spawn waits for them).
+        let (dirs, bundles) = match proxy {
+            Some(profile) => self.plugin_bundles(host, profile),
+            None => (Vec::new(), Vec::new()),
+        };
         let spec = SpawnSpec {
             id,
             cwd,
             name,
             args,
-            env: vec![], // filled by `send_with_proxy`
+            env: proxy.map(|p| self.plugin_env(p)).unwrap_or_default(),
             rows,
             cols,
             // `--plain`'s session is not one for a manager to show.
             ephemeral: self.mode == Mode::Plain,
             extensions: Some(self.extensions.session_extensions()),
         };
-        self.send_with_proxy(
-            host,
-            Msg::Spawn(spec),
-            proxy,
-            first_party,
-            ReplyTo::Spawned(id),
-        )
+        let msg = Msg::Spawn(spec).with_plugin_dirs(&dirs);
+        self.send_with_proxy(host, msg, proxy, first_party, ReplyTo::Spawned(id), bundles)
+    }
+
+    /// The bundles of `profile`'s enabled plugins on `host`: the paths of
+    /// those ready there, and those it has not answered for (installed in
+    /// Claude Code, failed or too old: neither).
+    fn plugin_bundles(&self, host: &str, profile: &str) -> (Vec<String>, Vec<PendingBundle>) {
+        let mut dirs = Vec::new();
+        let mut pending = Vec::new();
+        for p in self.enabled_plugins(profile) {
+            match self.bundle_state(host, &p.name) {
+                Some(BundleState::Ready(path)) => dirs.push(path.clone()),
+                Some(BundleState::Pending) | None => pending.push(PendingBundle {
+                    name: p.name.clone(),
+                    msg: bundle_msg(p),
+                    to: ReplyTo::Bundle {
+                        host: host.to_owned(),
+                        name: p.name.clone(),
+                        sha256: p.bundle_sha256.clone(),
+                    },
+                }),
+                Some(_) => {}
+            }
+        }
+        (dirs, pending)
     }
 
-    /// Send `msg` (a `Spawn` or `Respawn` with an empty env) to `host` with
-    /// the env of `proxy`. A cold proxy-config cache goes through the effect
-    /// runner, which fetches the config first and answers with
+    /// Send `msg` (a `Spawn` or `Respawn` carrying only the plugin env) to
+    /// `host` with the env of `proxy` added. A cold proxy-config cache, or
+    /// `bundles` still to come, go through the effect runner, which fetches
+    /// the config and waits for the bundles first and answers with
     /// [`ReplyTo::deferred`]. Fails when the profile cannot be resolved.
     fn send_with_proxy(
         &mut self,
@@ -456,19 +483,25 @@ impl App {
         proxy: Option<&str>,
         first_party: bool,
         to: ReplyTo,
+        bundles: Vec<PendingBundle>,
     ) -> Result<(), String> {
         match proxy {
-            Some(name) if self.proxy_config_cached(name).is_none() => {
+            Some(name) if self.proxy_config_cached(name).is_none() || !bundles.is_empty() => {
+                if !bundles.is_empty() {
+                    let names: Vec<&str> = bundles.iter().map(|b| b.name.as_str()).collect();
+                    self.notify(format!("fetching plugin {}…", names.join(", ")));
+                }
                 self.emit(Effect::SpawnWithProxy {
                     host: host.to_owned(),
                     msg,
                     proxy_name: name.to_owned(),
                     first_party,
                     to: to.deferred(),
+                    bundles,
                 });
             }
             _ => {
-                let msg = msg.with_env(self.proxy_env_for(proxy, first_party)?);
+                let msg = msg.merge_env(self.proxy_env_for(proxy, first_party)?);
                 self.request(host, msg, to);
             }
         }
@@ -554,7 +587,11 @@ impl App {
         let msg = Msg::Respawn(RespawnSpec {
             id,
             fresh,
-            env: vec![], // filled by `send_with_proxy`
+            // The plugin env now; the journaled args keep their bundles.
+            env: proxy
+                .as_deref()
+                .map(|p| self.plugin_env(p))
+                .unwrap_or_default(),
             rows,
             cols,
             extensions: Some(self.extensions.session_extensions()),
@@ -565,6 +602,7 @@ impl App {
             proxy.as_deref(),
             first_party,
             ReplyTo::Respawned(id),
+            Vec::new(),
         ) {
             Ok(()) => {
                 // Keep the effective choice, so a later restart keeps the env.
~~~~~~

Run: `umask 022; cargo test --locked --bin claudio` → `test result: ok. 1037 passed; 0 failed`
Run: `umask 022; cargo test --test manager -- --test-threads=1` → `test result: ok. 50 passed; 0 failed`
Dev build: 3 dead-code warnings (`PluginList::{admitted,summary}`, `load_cache`, `load_cache_from`); Task 7 uses them.

Step 3: commit
`/usr/bin/git commit -m "feat(tui): spawn with plugin bundles and env" -- src/proto.rs src/proxy/api.rs src/tui/app.rs src/tui/sessions.rs src/tui/mod.rs src/tui/plugins.rs`

Interfaces this task provides to later tasks:
- `Msg::{merge_env(proxy_env), with_plugin_dirs(&[String])}`; `app::PendingBundle { name, msg, to }`; `Effect::SpawnWithProxy { …, bundles: Vec<PendingBundle> }`.
- `tui::mod::{wait_bundles, BUNDLE_WAIT}` (20 s); `ConfigResponse: Clone`.


### Task 7: `claudio proxy login` through the browser
Model: strong   Repo: /volumes/repos/claudio   Files: src/proxy/login.rs (new), src/proxy/cmd.rs, src/proxy/mod.rs, src/proxy/profile.rs, src/proxy/plugins.rs, src/proxy/env.rs, src/cli.rs
Goal: the device-style sign-in (start, code, browser, polling with slow_down and expiry, 404 → paste), `--token`, the URL defaulting to the default profile's, `email` on the profile and in `status`, the plugin list fetched after login, `logout` dropping the cache entry, `CLAUDIO_NO_BROWSER` (spec §1, AC1's login state machine).
Patch: .delivery/2026-11-06-session-plugins/patches/task-7.patch

Step 1: write the failing test
The tests are in the patch below: `src/proxy/login.rs` (axum fake proxy, 5 ms time unit)
`pending_then_approved_gives_the_token`, `slow_down_doubles_the_interval` (5 polls in 45 units),
`expiry_denial_and_unknown_codes_end_the_login` (deadline; 410 and 404 → expired; 403 → denied; 500),
`start_tells_an_old_proxy_and_too_many_pending` (404 → paste; 429), `the_browser_opens_only_with_a_display_a_tty_and_no_opt_out`;
`src/proxy/profile.rs` `email_is_optional_and_round_trips`; `src/proxy/cmd.rs`
`proxy_text_is_printed_without_control_characters` and the two `store_login` tests with the e-mail.
Run: `cargo test --locked --bin claudio proxy::`
Expected: the build fails (`login` module, `Profile.email`, `store_login`'s fifth argument not found).

Step 2: implement
`/usr/bin/git apply .delivery/2026-11-06-session-plugins/patches/task-7.patch`

~~~~~~diff
diff --git a/src/cli.rs b/src/cli.rs
index 8ce336e..f5df913 100644
--- a/src/cli.rs
+++ b/src/cli.rs
@@ -361,6 +361,7 @@ pub const HELP_APPENDIX: &str = "\n\
 \x20  CLAUDIO_COLS=<n> / CLAUDIO_ROWS=<n>  Emulated terminal size (default: 120x40).\n\
 \x20  CLAUDIO_DEBUG=1                   Verbose wrapper diagnostics.\n\
 \x20  CLAUDIO_RAW_LOG=<path>            Dump the raw PTY byte stream to a file.\n\
+\x20  CLAUDIO_NO_BROWSER=1              `claudio proxy login` prints the sign-in URL without opening a browser.\n\
 \n\
 \x20Environment variables — API server (--api):\n\
 \x20  CLAUDIO_API_BIND=<host:port>      Listen address (default: 127.0.0.1:8080).\n\
@@ -394,7 +395,7 @@ pub const HELP_APPENDIX: &str = "\n\
 \x20  CLAUDIO_NO_UPDATE_CHECK=1 or `[update] check = false` is set in config.toml.\n\
 \n\
 \x20Proxy commands (optional claude-proxy integration):\n\
-\x20  claudio proxy login [URL]         Add or update a proxy profile (prompts for token).\n\
+\x20  claudio proxy login [URL]         Sign in through the browser (--token: paste a token instead).\n\
 \x20  claudio proxy status              Show profiles, live stats and pool health.\n\
 \x20  claudio proxy logout [NAME]       Remove a proxy profile.\n\
 \x20  claudio proxy use NAME|none       Set (or clear) the default proxy profile.\n\
diff --git a/src/proxy/cmd.rs b/src/proxy/cmd.rs
index c6c03f0..3ce9394 100644
--- a/src/proxy/cmd.rs
+++ b/src/proxy/cmd.rs
@@ -7,6 +7,8 @@ use std::io::{self, BufRead, Write};
 use std::process::ExitCode;
 
 use super::api;
+use super::login::{self, Granted};
+use super::plugins;
 use super::profile::{self, Profile};
 
 // ── Dispatch ──────────────────────────────────────────────────────────────────
@@ -43,14 +45,20 @@ const PROXY_HELP: &str = "\
 claudio proxy — manage claude-proxy profiles
 
 USAGE
-  claudio proxy login [URL]         Add or update a proxy profile
+  claudio proxy login [URL]         Sign in through the browser (or paste a token)
   claudio proxy status              Show profiles and live stats
-  claudio proxy logout [NAME]       Remove a proxy profile
+  claudio proxy logout [NAME]       Remove a proxy profile and its cached plugins
   claudio proxy use NAME|none       Set the default profile
 
 OPTIONS (login)
-  --name NAME    Profile name (default: first DNS label of the host)
+  URL            The proxy (default: the default profile's)
+  --name NAME    Profile name (default: the default profile's when URL is not
+                 given, else the first DNS label of the host)
+  --token        Paste a token instead of signing in through the browser
   --unverified   Save the profile even if token verification fails
+
+The sign-in page opens in a browser when there is a display; set
+CLAUDIO_NO_BROWSER=1 to only print its URL.
 ";
 
 // ── login ─────────────────────────────────────────────────────────────────────
@@ -60,6 +68,7 @@ fn cmd_login(args: &[String]) -> ExitCode {
     let mut name_override: Option<String> = None;
     let mut url_arg: Option<String> = None;
     let mut unverified = false;
+    let mut paste = false;
     let mut i = 0;
     while i < args.len() {
         match args[i].as_str() {
@@ -70,6 +79,9 @@ fn cmd_login(args: &[String]) -> ExitCode {
             "--unverified" => {
                 unverified = true;
             }
+            "--token" => {
+                paste = true;
+            }
             a if !a.starts_with('-') => {
                 url_arg = Some(a.to_owned());
             }
@@ -81,10 +93,24 @@ fn cmd_login(args: &[String]) -> ExitCode {
         i += 1;
     }
 
-    // Get URL.
-    let url_raw = match url_arg {
-        Some(u) => u,
-        None => {
+    let mut sec = match profile::load() {
+        Ok(s) => s,
+        Err(e) => {
+            eprintln!("claudio proxy login: could not read config.toml: {e}");
+            return ExitCode::FAILURE;
+        }
+    };
+
+    // Get URL: given, else the default profile's (keeping its name), else
+    // asked for.
+    let default = sec
+        .default
+        .as_ref()
+        .and_then(|n| sec.profiles.get(n).map(|p| (p.url.clone(), n.clone())));
+    let (url_raw, default_name) = match (url_arg, default) {
+        (Some(u), _) => (u, None),
+        (None, Some((url, name))) => (url, Some(name)),
+        (None, None) => {
             eprint!("Proxy URL (e.g. https://claude.example.net): ");
             let _ = io::stderr().flush();
             let mut line = String::new();
@@ -92,7 +118,7 @@ fn cmd_login(args: &[String]) -> ExitCode {
                 eprintln!("claudio proxy login: no URL provided");
                 return ExitCode::FAILURE;
             }
-            line.trim().to_owned()
+            (line.trim().to_owned(), None)
         }
     };
 
@@ -113,19 +139,40 @@ fn cmd_login(args: &[String]) -> ExitCode {
         }
     };
 
-    let token = read_token_from_tty_or_stdin();
+    let name = name_override
+        .or(default_name)
+        .unwrap_or_else(|| profile::name_from_url(&url));
+    let rt = tokio::runtime::Builder::new_current_thread()
+        .enable_all()
+        .build()
+        .unwrap();
+
+    // The browser sign-in, unless a token is to be pasted (asked, or the
+    // proxy has none).
+    let (token, email) = if paste {
+        (read_token_from_tty_or_stdin(), None)
+    } else {
+        match rt.block_on(browser_login(&url)) {
+            Ok(Some(granted)) => {
+                let email = Some(granted.email).filter(|e| !e.is_empty());
+                (granted.token, email)
+            }
+            Ok(None) => {
+                eprintln!("This proxy has no browser login; paste a token.");
+                (read_token_from_tty_or_stdin(), None)
+            }
+            Err(e) => {
+                eprintln!("claudio proxy login: {e}");
+                return ExitCode::FAILURE;
+            }
+        }
+    };
     if token.is_empty() {
         eprintln!("claudio proxy login: no token provided");
         return ExitCode::FAILURE;
     }
 
-    let name = name_override.unwrap_or_else(|| profile::name_from_url(&url));
-
     // Validate with the proxy.
-    let rt = tokio::runtime::Builder::new_current_thread()
-        .enable_all()
-        .build()
-        .unwrap();
     let verified = rt.block_on(async {
         match api::check_root(&url, &token).await {
             Ok(true) => {
@@ -163,30 +210,86 @@ fn cmd_login(args: &[String]) -> ExitCode {
     }
 
     // Save.
-    let mut sec = match profile::load() {
-        Ok(s) => s,
-        Err(e) => {
-            eprintln!("claudio proxy login: could not read config.toml: {e}");
-            return ExitCode::FAILURE;
-        }
-    };
     let is_first = sec.profiles.is_empty();
-    store_login(&mut sec, &name, url.clone(), token);
-    if is_first || sec.default.is_none() {
+    store_login(&mut sec, &name, url.clone(), token, email.clone());
+    let as_default = is_first || sec.default.is_none();
+    if as_default {
         sec.default = Some(name.clone());
-        println!("  Set '{name}' as default proxy.");
     }
     if let Err(e) = profile::save(&sec) {
         eprintln!("claudio proxy login: could not save config.toml: {e}");
         return ExitCode::FAILURE;
     }
-    println!("  Saved profile '{name}'.");
+    let saved = if as_default {
+        format!("Profile \"{name}\" saved as default.")
+    } else {
+        format!("Profile \"{name}\" saved.")
+    };
+    match &email {
+        Some(email) => println!("Signed in as {}. {saved}", printable(email)),
+        None => println!("{saved}"),
+    }
+
+    // The new token's plugins, so the next session has them.
+    if let Some(p) = sec.profiles.get(&name) {
+        rt.block_on(refresh_plugins(&name, p));
+    }
     ExitCode::SUCCESS
 }
 
+/// The browser sign-in at `url`: `Ok(None)` when the proxy has none.
+async fn browser_login(url: &str) -> Result<Option<Granted>, login::LoginError> {
+    use std::io::IsTerminal;
+    let Some(start) = login::start(url).await? else {
+        return Ok(None);
+    };
+    let page = printable(&start.verify_url);
+    println!("Open this page, sign in, and type your code:");
+    println!("  {page}");
+    println!("Your code: {}", printable(&start.user_code));
+    let web = page.starts_with("https://") || page.starts_with("http://");
+    let env = |k: &str| std::env::var(k).ok();
+    if web && login::browser_wanted(std::env::consts::OS, env, io::stdout().is_terminal()) {
+        login::open_browser(&page);
+    }
+    println!("Waiting… (Ctrl+C to cancel)");
+    let _ = io::stdout().flush();
+    login::poll(url, &start, std::time::Duration::from_secs(1))
+        .await
+        .map(Some)
+}
+
+/// Fetch profile `name`'s plugins and cache them; say which there are. A
+/// proxy without plugins (404) says nothing.
+async fn refresh_plugins(name: &str, p: &Profile) {
+    match plugins::fetch(name, p).await {
+        Ok(list) => {
+            if let Err(e) = plugins::save_cache(&list) {
+                eprintln!("  notice: could not save the plugins: {e}");
+            } else if !list.plugins.is_empty() {
+                println!("  Plugins: {}", printable(&list.summary()));
+            }
+        }
+        Err(api::ApiError::Http(s)) if s.as_u16() == 404 => {}
+        Err(e) => eprintln!("  notice: could not fetch the plugins: {e}"),
+    }
+}
+
+/// `text` from a proxy, safe to print: no control characters.
+fn printable(text: &str) -> String {
+    text.chars().filter(|c| !c.is_control()).collect()
+}
+
 /// Store a logged-in profile, replacing any profile of that name. A re-login
-/// keeps the profile's `first_party` mode: only the URL and token change.
-fn store_login(sec: &mut profile::ProxySection, name: &str, url: String, token: String) {
+/// keeps the profile's `first_party` mode: only the URL, token and e-mail
+/// change (a pasted token has no e-mail).
+fn store_login(
+    sec: &mut profile::ProxySection,
+    name: &str,
+    url: String,
+    token: String,
+    email: Option<String>,
+) {
     let first_party = sec.profiles.get(name).is_some_and(|p| p.first_party);
     sec.profiles.insert(
         name.to_owned(),
@@ -194,6 +297,7 @@ fn store_login(sec: &mut profile::ProxySection, name: &str, url: String, token:
             url,
             token,
             first_party,
+            email,
         },
     );
 }
@@ -364,6 +468,7 @@ fn cmd_status() -> ExitCode {
             " (env)"
         };
         println!("  {n}{marker}  {}  {}", p.url, p.masked_token());
+        print_plugins(n);
     }
     for (n, p) in &sec.profiles {
         let marker = if default_name.as_deref() == Some(n.as_str()) {
@@ -371,7 +476,10 @@ fn cmd_status() -> ExitCode {
         } else {
             ""
         };
-        println!("  {n}{marker}  {}  {}", p.url, p.masked_token());
+        let email = p.email.as_deref().map(printable).unwrap_or_default();
+        let line = format!("  {n}{marker}  {}  {}  {email}", p.url, p.masked_token());
+        println!("{}", line.trim_end());
+        print_plugins(n);
     }
 
     // Live stats for the default profile.
@@ -392,6 +500,21 @@ fn cmd_status() -> ExitCode {
     ExitCode::SUCCESS
 }
 
+/// Profile `name`'s cached plugins (the admitted ones), if any.
+fn print_plugins(name: &str) {
+    let Some(list) = plugins::load_cache(name) else {
+        return;
+    };
+    let names: Vec<String> = list
+        .admitted()
+        .iter()
+        .map(|p| printable(&format!("{} {}", p.name, p.version)))
+        .collect();
+    if !names.is_empty() {
+        println!("    plugins: {}", names.join(", "));
+    }
+}
+
 async fn print_live_status(p: &Profile) {
     // Stats.
     match api::fetch_stats(&p.url, &p.token, "24h").await {
@@ -480,6 +603,11 @@ fn cmd_logout(args: &[String]) -> ExitCode {
         eprintln!("claudio proxy logout: profile '{name}' not found");
         return ExitCode::FAILURE;
     }
+    // Its plugins' secrets go with it.
+    if let Err(e) = plugins::remove_cache(&name) {
+        eprintln!("claudio proxy logout: could not update plugins.json: {e}");
+        return ExitCode::FAILURE;
+    }
 
     // If we removed the default, clear it (or pick another one).
     if sec.default.as_deref() == Some(&name) {
@@ -545,6 +673,7 @@ mod tests {
                 url: "https://old.example.net".into(),
                 token: "old".into(),
                 first_party: true,
+                email: None,
             },
         );
         store_login(
@@ -552,17 +681,34 @@ mod tests {
             "work",
             "https://new.example.net".into(),
             "new".into(),
+            Some("pau@example.org".into()),
         );
         let p = &sec.profiles["work"];
         assert_eq!(p.url, "https://new.example.net");
         assert_eq!(p.token, "new");
         assert!(p.first_party);
+        assert_eq!(p.email.as_deref(), Some("pau@example.org"));
     }
 
     #[test]
     fn new_profile_is_gateway() {
         let mut sec = profile::ProxySection::default();
-        store_login(&mut sec, "work", "https://a.example.net".into(), "t".into());
+        store_login(
+            &mut sec,
+            "work",
+            "https://a.example.net".into(),
+            "t".into(),
+            None,
+        );
         assert!(!sec.profiles["work"].first_party);
+        assert_eq!(sec.profiles["work"].email, None);
+    }
+
+    #[test]
+    fn proxy_text_is_printed_without_control_characters() {
+        assert_eq!(
+            printable("https://p.example/\u{1b}]52;c;x\u{7}login\n"),
+            "https://p.example/]52;c;xlogin"
+        );
     }
 }
diff --git a/src/proxy/env.rs b/src/proxy/env.rs
index 0756097..336dadd 100644
--- a/src/proxy/env.rs
+++ b/src/proxy/env.rs
@@ -171,6 +171,7 @@ mod tests {
             url: url.into(),
             token: token.into(),
             first_party: false,
+            email: None,
         }
     }
 
diff --git a/src/proxy/login.rs b/src/proxy/login.rs
new file mode 100644
index 0000000..a2665be
--- /dev/null
+++ b/src/proxy/login.rs
@@ -0,0 +1,351 @@
+//! `claudio proxy login`'s browser sign-in: a device-style flow against the
+//! proxy (`/v1/claudio/login/start`, `/v1/claudio/login/poll`).
+//!
+//! The proxy hands out a one-time `user_code` the member types on its
+//! sign-in page (the URL never carries it) and a `device_code` only this
+//! process holds; this side polls until the member approved, then gets the
+//! member's own token. The device code lives only in memory: Ctrl+C
+//! abandons the login.
+
+use std::time::Duration;
+
+use serde::Deserialize;
+
+use super::api::build_client;
+
+/// `POST /v1/claudio/login/start`.
+#[derive(Clone, Deserialize)]
+pub struct Start {
+    pub device_code: String,
+    pub user_code: String,
+    pub verify_url: String,
+    #[serde(default = "default_interval")]
+    pub interval_s: u64,
+    #[serde(default = "default_expiry")]
+    pub expires_in_s: u64,
+}
+
+fn default_interval() -> u64 {
+    3
+}
+
+fn default_expiry() -> u64 {
+    600
+}
+
+impl std::fmt::Debug for Start {
+    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
+        f.debug_struct("Start")
+            .field("device_code", &"[redacted]")
+            .field("user_code", &self.user_code)
+            .field("verify_url", &self.verify_url)
+            .finish()
+    }
+}
+
+/// What an approved login gives: the member's token.
+#[derive(Clone, Deserialize)]
+pub struct Granted {
+    pub token: String,
+    #[serde(default)]
+    pub email: String,
+    #[serde(default)]
+    pub name: String,
+}
+
+impl std::fmt::Debug for Granted {
+    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
+        f.debug_struct("Granted")
+            .field("token", &"[redacted]")
+            .field("email", &self.email)
+            .field("name", &self.name)
+            .finish()
+    }
+}
+
+/// Why a browser login did not give a token.
+#[derive(Debug, PartialEq)]
+pub enum LoginError {
+    /// `login/start` answered 429 `too_many_pending`.
+    TooManyPending,
+    /// The member (or an admin) said no, or the account is disabled.
+    Denied,
+    /// The code expired, or the proxy forgot it (restart, eviction).
+    Expired,
+    /// Any other status.
+    Http(u16),
+    /// The proxy could not be reached.
+    Network(String),
+}
+
+impl std::fmt::Display for LoginError {
+    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
+        match self {
+            LoginError::TooManyPending => write!(
+                f,
+                "too many pending logins from this address, try again in a minute"
+            ),
+            LoginError::Denied => write!(f, "sign-in denied"),
+            LoginError::Expired => write!(f, "login expired, run again"),
+            LoginError::Http(status) => write!(f, "the proxy answered {status}"),
+            LoginError::Network(e) => write!(f, "could not reach the proxy: {e}"),
+        }
+    }
+}
+
+/// Start a login at `base_url`. `Ok(None)`: the proxy has no browser login
+/// (404: an older proxy, or one without sign-in), so a token is pasted.
+pub async fn start(base_url: &str) -> Result<Option<Start>, LoginError> {
+    let client = build_client().map_err(|e| LoginError::Network(e.to_string()))?;
+    let resp = client
+        .post(format!("{base_url}/v1/claudio/login/start"))
+        .send()
+        .await
+        .map_err(|e| LoginError::Network(e.without_url().to_string()))?;
+    match resp.status().as_u16() {
+        200 => resp
+            .json()
+            .await
+            .map(Some)
+            .map_err(|_| LoginError::Http(200)),
+        404 => Ok(None),
+        429 => Err(LoginError::TooManyPending),
+        other => Err(LoginError::Http(other)),
+    }
+}
+
+/// Poll until the login of `start` is decided. `unit` is one second (tests
+/// shrink it): the interval is `interval_s` units, doubled on each
+/// `slow_down`, and the whole wait `expires_in_s` units. A network error is
+/// retried until then.
+pub async fn poll(base_url: &str, start: &Start, unit: Duration) -> Result<Granted, LoginError> {
+    let client = build_client().map_err(|e| LoginError::Network(e.to_string()))?;
+    let deadline = tokio::time::Instant::now() + unit * start.expires_in_s as u32;
+    let mut interval = unit * start.interval_s.max(1) as u32;
+    let body = serde_json::json!({ "device_code": start.device_code });
+    loop {
+        if tokio::time::Instant::now() + interval > deadline {
+            return Err(LoginError::Expired);
+        }
+        tokio::time::sleep(interval).await;
+        let sent = client
+            .post(format!("{base_url}/v1/claudio/login/poll"))
+            .json(&body)
+            .send()
+            .await;
+        let Ok(resp) = sent else {
+            continue;
+        };
+        match resp.status().as_u16() {
+            200 => return resp.json().await.map_err(|_| LoginError::Http(200)),
+            428 => {}
+            429 => interval *= 2,
+            403 => return Err(LoginError::Denied),
+            404 | 410 => return Err(LoginError::Expired),
+            other => return Err(LoginError::Http(other)),
+        }
+    }
+}
+
+/// Whether to open the sign-in page in a browser: there is one to show it
+/// (a display on Linux and the BSDs, always on macOS), stdout is a terminal
+/// (someone is there to use it), and `CLAUDIO_NO_BROWSER` is unset. The URL
+/// is printed either way.
+pub fn browser_wanted(os: &str, env: impl Fn(&str) -> Option<String>, tty: bool) -> bool {
+    let set = |k: &str| env(k).is_some_and(|v| !v.is_empty());
+    let display = os == "macos" || set("DISPLAY") || set("WAYLAND_DISPLAY");
+    display && tty && env("CLAUDIO_NO_BROWSER").is_none()
+}
+
+/// Open `url` with `open` (macOS) or `xdg-open`, without waiting for it;
+/// whether it could be started.
+pub fn open_browser(url: &str) -> bool {
+    let opener = if cfg!(target_os = "macos") {
+        "open"
+    } else {
+        "xdg-open"
+    };
+    let child = std::process::Command::new(opener)
+        .arg(url)
+        .stdin(std::process::Stdio::null())
+        .stdout(std::process::Stdio::null())
+        .stderr(std::process::Stdio::null())
+        .spawn();
+    match child {
+        Ok(mut child) => {
+            std::thread::spawn(move || child.wait());
+            true
+        }
+        Err(_) => false,
+    }
+}
+
+#[cfg(test)]
+mod tests {
+    use super::*;
+    use axum::http::StatusCode;
+    use axum::routing::post;
+    use axum::Json;
+    use std::sync::atomic::{AtomicUsize, Ordering};
+    use std::sync::Arc;
+
+    const UNIT: Duration = Duration::from_millis(5);
+
+    async fn serve(router: axum::Router) -> String {
+        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
+        let port = listener.local_addr().unwrap().port();
+        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
+        format!("http://127.0.0.1:{port}")
+    }
+
+    fn start_reply() -> serde_json::Value {
+        serde_json::json!({"device_code": "dc-secret", "user_code": "K7PQ-3WXM",
+            "verify_url": "https://p.example/claudio/login", "interval_s": 1, "expires_in_s": 45})
+    }
+
+    /// A proxy whose poll answers `script` in turn (then the last one
+    /// forever), counting the polls.
+    async fn proxy(script: Vec<(StatusCode, serde_json::Value)>) -> (String, Arc<AtomicUsize>) {
+        let polls = Arc::new(AtomicUsize::new(0));
+        let counted = Arc::clone(&polls);
+        let script = Arc::new(script);
+        let router = axum::Router::new()
+            .route(
+                "/v1/claudio/login/start",
+                post(|| async { Json(start_reply()) }),
+            )
+            .route(
+                "/v1/claudio/login/poll",
+                post(move |Json(body): Json<serde_json::Value>| {
+                    let n = counted.fetch_add(1, Ordering::SeqCst);
+                    let script = Arc::clone(&script);
+                    async move {
+                        assert_eq!(body["device_code"], "dc-secret");
+                        let (status, reply) = script[n.min(script.len() - 1)].clone();
+                        (status, Json(reply))
+                    }
+                }),
+            );
+        (serve(router).await, polls)
+    }
+
+    fn error(code: &str) -> serde_json::Value {
+        serde_json::json!({ "error": code })
+    }
+
+    async fn run(
+        script: Vec<(StatusCode, serde_json::Value)>,
+    ) -> (Result<Granted, LoginError>, usize) {
+        let (base, polls) = proxy(script).await;
+        let start = start(&base).await.unwrap().expect("browser login");
+        assert_eq!(start.user_code, "K7PQ-3WXM");
+        assert!(!format!("{start:?}").contains("dc-secret"));
+        let result = poll(&base, &start, UNIT).await;
+        (result, polls.load(Ordering::SeqCst))
+    }
+
+    #[tokio::test]
+    async fn pending_then_approved_gives_the_token() {
+        let granted =
+            serde_json::json!({"token": "tok-1", "email": "pau@vocdoni.org", "name": "pau"});
+        let (result, polls) = run(vec![
+            (
+                StatusCode::PRECONDITION_REQUIRED,
+                error("authorization_pending"),
+            ),
+            (
+                StatusCode::PRECONDITION_REQUIRED,
+                error("authorization_pending"),
+            ),
+            (StatusCode::OK, granted),
+        ])
+        .await;
+        let granted = result.unwrap();
+        assert_eq!(
+            (granted.token.as_str(), granted.email.as_str()),
+            ("tok-1", "pau@vocdoni.org")
+        );
+        assert!(!format!("{granted:?}").contains("tok-1"));
+        assert_eq!(polls, 3);
+    }
+
+    #[tokio::test]
+    async fn slow_down_doubles_the_interval() {
+        // 45 units: 1 + 2 + 4 + 8 + 16 = 31 fit, the next 32 does not.
+        let (result, polls) = run(vec![(StatusCode::TOO_MANY_REQUESTS, error("slow_down"))]).await;
+        assert_eq!(result.unwrap_err(), LoginError::Expired);
+        assert_eq!(polls, 5);
+    }
+
+    #[tokio::test]
+    async fn expiry_denial_and_unknown_codes_end_the_login() {
+        let pending = (
+            StatusCode::PRECONDITION_REQUIRED,
+            error("authorization_pending"),
+        );
+        let (result, polls) = run(vec![pending]).await;
+        assert_eq!(result.unwrap_err(), LoginError::Expired, "the deadline");
+        assert!(polls > 5, "{polls}");
+        for (status, code, want) in [
+            (StatusCode::GONE, "expired", LoginError::Expired),
+            (StatusCode::NOT_FOUND, "unknown_code", LoginError::Expired),
+            (StatusCode::FORBIDDEN, "denied", LoginError::Denied),
+            (
+                StatusCode::INTERNAL_SERVER_ERROR,
+                "x",
+                LoginError::Http(500),
+            ),
+        ] {
+            let (result, polls) = run(vec![(status, error(code))]).await;
+            assert_eq!(result.unwrap_err(), want, "{status}");
+            assert_eq!(polls, 1);
+        }
+    }
+
+    #[tokio::test]
+    async fn start_tells_an_old_proxy_and_too_many_pending() {
+        let old = serve(axum::Router::new()).await;
+        assert!(start(&old).await.unwrap().is_none(), "404: paste a token");
+        let busy = axum::Router::new().route(
+            "/v1/claudio/login/start",
+            post(|| async {
+                (
+                    StatusCode::TOO_MANY_REQUESTS,
+                    Json(error("too_many_pending")),
+                )
+            }),
+        );
+        let busy = serve(busy).await;
+        assert_eq!(start(&busy).await.unwrap_err(), LoginError::TooManyPending);
+        assert_eq!(
+            LoginError::TooManyPending.to_string(),
+            "too many pending logins from this address, try again in a minute"
+        );
+    }
+
+    #[test]
+    fn the_browser_opens_only_with_a_display_a_tty_and_no_opt_out() {
+        let env = |vars: &'static [(&'static str, &'static str)]| {
+            move |k: &str| {
+                vars.iter()
+                    .find(|(n, _)| *n == k)
+                    .map(|(_, v)| v.to_string())
+            }
+        };
+        assert!(browser_wanted("linux", env(&[("DISPLAY", ":0")]), true));
+        assert!(browser_wanted(
+            "linux",
+            env(&[("WAYLAND_DISPLAY", "w-0")]),
+            true
+        ));
+        assert!(!browser_wanted("linux", env(&[]), true), "ssh session");
+        assert!(!browser_wanted("linux", env(&[("DISPLAY", "")]), true));
+        assert!(
+            !browser_wanted("linux", env(&[("DISPLAY", ":0")]), false),
+            "not a tty"
+        );
+        assert!(browser_wanted("macos", env(&[]), true));
+        let opted_out = env(&[("DISPLAY", ":0"), ("CLAUDIO_NO_BROWSER", "1")]);
+        assert!(!browser_wanted("linux", opted_out, true));
+    }
+}
diff --git a/src/proxy/mod.rs b/src/proxy/mod.rs
index 35e6c9b..20d76a9 100644
--- a/src/proxy/mod.rs
+++ b/src/proxy/mod.rs
@@ -6,6 +6,7 @@
 pub mod api;
 pub mod cmd;
 pub mod env;
+pub mod login;
 pub mod plugins;
 pub mod profile;
 pub mod resolve;
diff --git a/src/proxy/plugins.rs b/src/proxy/plugins.rs
index 1beb14d..faf1ee9 100644
--- a/src/proxy/plugins.rs
+++ b/src/proxy/plugins.rs
@@ -570,6 +570,7 @@ mod tests {
             url,
             token: "tok".into(),
             first_party: false,
+            email: None,
         };
         let ok = axum::Router::new().route(
             "/v1/claudio/plugins",
diff --git a/src/proxy/profile.rs b/src/proxy/profile.rs
index 90a50ea..f1a6615 100644
--- a/src/proxy/profile.rs
+++ b/src/proxy/profile.rs
@@ -11,6 +11,7 @@
 //! url   = "https://proxy.example.com"
 //! token = "…"
 //! first_party = true  # optional: see `Profile::first_party`
+//! email = "pau@example.org"  # optional: set by a browser login
 //! ```
 //!
 //! The env var `CLAUDIO_PROXY_URL=<token>@<host>` creates an ephemeral profile
@@ -41,6 +42,10 @@ pub struct Profile {
     /// server-side tools through.
     #[serde(default, skip_serializing_if = "is_false")]
     pub first_party: bool,
+    /// The member's e-mail, when the token came from a browser login
+    /// (`claudio proxy login`); shown by `claudio proxy status`.
+    #[serde(default, skip_serializing_if = "Option::is_none")]
+    pub email: Option<String>,
 }
 
 fn is_false(b: &bool) -> bool {
@@ -53,6 +58,7 @@ impl fmt::Debug for Profile {
             .field("url", &self.url)
             .field("token", &"[redacted]")
             .field("first_party", &self.first_party)
+            .field("email", &self.email)
             .finish()
     }
 }
@@ -234,6 +240,7 @@ pub fn from_env() -> Option<(&'static str, Profile)> {
                 url,
                 token,
                 first_party: false,
+                email: None,
             },
         )),
         Err(e) => {
@@ -342,6 +349,7 @@ mod tests {
             url: "https://x.net".into(),
             token: "supersecret".into(),
             first_party: false,
+            email: None,
         };
         let dbg = format!("{p:?}");
         assert!(!dbg.contains("supersecret"));
@@ -365,6 +373,7 @@ mod tests {
                 url: "https://proxy.example.com".into(),
                 token: "topsecret".into(),
                 first_party: false,
+                email: None,
             },
         );
         save_to(&path, &sec).unwrap();
@@ -383,6 +392,32 @@ mod tests {
 
     // ── first_party ───────────────────────────────────────────────────────────
 
+    #[test]
+    fn email_is_optional_and_round_trips() {
+        let dir =
+            std::env::temp_dir().join(format!("claudio-proxy-email-{}", uuid::Uuid::new_v4()));
+        std::fs::create_dir_all(&dir).unwrap();
+        let path = dir.join("config.toml");
+        std::fs::write(
+            &path,
+            "[proxy.profiles.a]\nurl = \"https://a.net\"\ntoken = \"t\"\n",
+        )
+        .unwrap();
+        let mut loaded = load_from(&path).unwrap();
+        assert_eq!(loaded.profiles["a"].email, None);
+        save_to(&path, &loaded).unwrap();
+        assert!(!std::fs::read_to_string(&path).unwrap().contains("email"));
+        loaded.profiles.get_mut("a").unwrap().email = Some("pau@example.org".into());
+        save_to(&path, &loaded).unwrap();
+        let saved = std::fs::read_to_string(&path).unwrap();
+        assert!(saved.contains("email = \"pau@example.org\""), "{saved}");
+        assert_eq!(
+            load_from(&path).unwrap().profiles["a"].email.as_deref(),
+            Some("pau@example.org")
+        );
+        std::fs::remove_dir_all(&dir).unwrap();
+    }
+
     #[test]
     fn first_party_round_trips_and_defaults_to_false() {
         let dir =
@@ -410,6 +445,7 @@ mod tests {
                 url: "https://b.net".into(),
                 token: "t2".into(),
                 first_party: true,
+                email: None,
             },
         );
         save_to(&path, &sec).unwrap();
@@ -429,6 +465,7 @@ mod tests {
             url: "u".into(),
             token: t.into(),
             first_party: false,
+            email: None,
         };
         assert_eq!(p("abcdefghwxyz").masked_token(), "abcd…wxyz");
         assert_eq!(p("ab").masked_token(), "**");
@@ -442,6 +479,7 @@ mod tests {
             url: "u".into(),
             token: "🔑🔑🔑🔑🔑🔑🔑🔑🔑🔑".into(),
             first_party: false,
+            email: None,
         };
         // Should not panic; result length varies by char count.
         let masked = p.masked_token();
@@ -451,6 +489,7 @@ mod tests {
             url: "u".into(),
             token: "🔑🔑".into(),
             first_party: false,
+            email: None,
         };
         assert_eq!(p2.masked_token(), "**");
     }
~~~~~~

Run: `rustfmt --edition 2021 --check src/proxy/login.rs src/proxy/cmd.rs src/proxy/profile.rs src/proxy/plugins.rs src/proxy/env.rs src/cli.rs` → no output.
Run: `umask 022; cargo test --locked --bin claudio proxy::` → `test result: ok. 69 passed`
Run: `umask 022; cargo test --locked --bin claudio` → `test result: ok. 1044 passed; 0 failed`
Dev build: 0 warnings.
Manual smoke (optional, no real proxy): a fake proxy on loopback answering `login/start`, `login/poll`
(428 then 200), `GET /v1/claudio` and `GET /v1/claudio/plugins`; with `XDG_CONFIG_HOME` and `HOME` in a
scratch dir and `CLAUDIO_NO_BROWSER=1`, `claudio proxy login http://127.0.0.1:<port> </dev/null` prints the
page and code, then `Signed in as <email>. Profile "localhost" saved as default.` and
`  Plugins: <name> (<n> keys)`; `config.toml` and `plugins.json` are 0600; `claudio proxy status` shows the
e-mail and `    plugins: <name> <version>`; `claudio proxy logout localhost` leaves `{"profiles": {}}`.

Step 3: commit
`/usr/bin/git add src/proxy/login.rs`
`/usr/bin/git commit -m "feat(proxy): browser sign-in for claudio proxy login" -- src/proxy/login.rs src/proxy/cmd.rs src/proxy/mod.rs src/proxy/profile.rs src/proxy/plugins.rs src/proxy/env.rs src/cli.rs`

Interfaces this task provides to later tasks:
- `proxy::login::{start, poll, Start, Granted, LoginError, browser_wanted, open_browser}`; `Profile.email: Option<String>` (serde default, skipped when `None`).


### Task 8: end-to-end tests (AC3 manager, AC6 remote)
Model: standard   Repo: /volumes/repos/claudio   Files: tests/manager.rs, tests/remote.rs, src/remote/diag.rs, src/main.rs
Goal: AC3 through the real binary (fake claude whose `--help` lists `--plugin-dir`, a fake proxy serving one plugin, a pre-seeded `<XDG_CACHE_HOME>/claudio/plugins/<name>.zip` + `.sha256` with a bundle URL nothing listens on); AC6 as the gated `t9` with a new diag command `__remote-bundle`; `--features diag` compiles again.
Patch: .delivery/2026-11-06-session-plugins/patches/task-8.patch

Step 1: write the failing test
The tests are in the patch below. `tests/manager.rs` (section `// ── Proxy plugins ──`):
`test_proxy_plugin_reaches_new_sessions_until_turned_off` (argv has `--plugin-dir <cached zip>`, env has the key,
no secret in argv; `Alt+i` shows `1.0.0 · env` and `local: ready`; Space → `[ ] vocdoni-test`,
`config.toml` has `[extensions]\nvocdoni-test = false`; the next session has neither) and
`test_proxy_plugin_bundle_is_left_out_for_an_old_claude` (empty `--help`: no `--plugin-dir`, env still set).
They assert on the screen model and on files the fake claude writes, never on raw PTY bytes; they set
`XDG_CACHE_HOME` under the harness root, and wait for `plugins.json` before the wizard.
`tests/remote.rs` `t9_remote_plugin_bundle` (skips without `CLAUDIO_SSH_TEST_HOST`).
With Tasks 1–7 in, these tests pass at once (they exercise behaviour already built); run the
manager ones on the Task 7 tree to see them pass, and `cargo test --features diag --test remote`
there to see the base's diag build failure (``E0063 missing field `extensions` ``).

Step 2: implement
`/usr/bin/git apply .delivery/2026-11-06-session-plugins/patches/task-8.patch`

~~~~~~diff
diff --git a/src/main.rs b/src/main.rs
index c4e3fcb..89eaafa 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -83,6 +83,17 @@ fn main() -> std::process::ExitCode {
         return std::process::ExitCode::FAILURE;
     }
 
+    // `claudio __remote-bundle HOST CWD NAME SHA256` – ask HOST's daemon for a
+    // pre-seeded plugin bundle, spawn claude with it, print both.
+    #[cfg(feature = "diag")]
+    if argv.get(1).map(String::as_str) == Some("__remote-bundle") {
+        if let [_, _, host, cwd, name, sha256] = argv.as_slice() {
+            return remote::diag::remote_bundle_cmd(host, cwd, name, sha256);
+        }
+        eprintln!("usage: claudio __remote-bundle <host> <cwd> <name> <sha256>");
+        return std::process::ExitCode::FAILURE;
+    }
+
     // `claudio __ssh-hosts` – print all known SSH host aliases, one per line.
     #[cfg(feature = "diag")]
     if argv.get(1).map(String::as_str) == Some("__ssh-hosts") {
diff --git a/src/remote/diag.rs b/src/remote/diag.rs
index c3dd9a0..89ca0d6 100644
--- a/src/remote/diag.rs
+++ b/src/remote/diag.rs
@@ -130,6 +130,7 @@ pub fn remote_session_cmd(host: &str, cwd: &str) -> ExitCode {
             rows: 24,
             cols: 80,
             ephemeral: false,
+            extensions: None,
         };
 
         // Spawn the session.
@@ -204,6 +205,82 @@ pub fn remote_session_cmd(host: &str, cwd: &str) -> ExitCode {
     })
 }
 
+/// `claudio __remote-bundle HOST CWD NAME SHA256`
+///
+/// Asks `HOST`'s daemon for plugin `NAME`'s bundle with hash `SHA256` (from
+/// a URL nothing listens on, so only a pre-seeded cache answers), prints
+/// the reply as JSON, then spawns claude in `CWD` with `--plugin-dir
+/// <path>`, prints `pid=<pid>`, keeps the session 5 s (for the caller to
+/// look at its argv) and kills it.
+pub fn remote_bundle_cmd(host: &str, cwd: &str, name: &str, sha256: &str) -> ExitCode {
+    use crate::proto::{Msg, SpawnSpec};
+    let rt = match make_runtime() {
+        Ok(r) => r,
+        Err(code) => return code,
+    };
+    rt.block_on(async move {
+        let client = match crate::client::connect_ssh(host).await {
+            Ok(c) => c,
+            Err(e) => {
+                eprintln!("connect_ssh({host}) failed: {e}");
+                return ExitCode::FAILURE;
+            }
+        };
+        let bundle = Msg::Bundle {
+            name: name.to_owned(),
+            url: "https://127.0.0.1:9/never.zip".to_owned(),
+            sha256: sha256.to_owned(),
+            installed_id: String::new(),
+        };
+        let path = match client.request(bundle).await {
+            Ok(Msg::Bundled {
+                path, installed, ..
+            }) => {
+                let reply = serde_json::json!({ "path": path, "installed": installed });
+                println!("{reply}");
+                path
+            }
+            Ok(other) => {
+                eprintln!("unexpected Bundle reply: {other:?}");
+                return ExitCode::FAILURE;
+            }
+            Err(e) => {
+                eprintln!("Bundle request failed: {e}");
+                return ExitCode::FAILURE;
+            }
+        };
+        let Some(path) = path else {
+            return ExitCode::SUCCESS;
+        };
+        let id = uuid::Uuid::new_v4();
+        let spec = SpawnSpec {
+            id,
+            cwd: cwd.to_owned(),
+            name: None,
+            args: vec!["--plugin-dir".to_owned(), path],
+            env: vec![],
+            rows: 24,
+            cols: 80,
+            ephemeral: true,
+            extensions: None,
+        };
+        match client.request(Msg::Spawn(spec)).await {
+            Ok(Msg::Spawned { pid, .. }) => {
+                println!("pid={}", pid.unwrap_or(0));
+            }
+            other => {
+                eprintln!("Spawn failed: {other:?}");
+                return ExitCode::FAILURE;
+            }
+        }
+        use std::io::Write;
+        let _ = std::io::stdout().flush();
+        tokio::time::sleep(Duration::from_secs(5)).await;
+        let _ = client.request(Msg::Kill { id }).await;
+        ExitCode::SUCCESS
+    })
+}
+
 /// `claudio __ssh-hosts` — print all known SSH host aliases, one per line.
 pub fn ssh_hosts_cmd() -> ExitCode {
     let hosts = super::hosts::candidates();
diff --git a/tests/manager.rs b/tests/manager.rs
index ad2681b..e43958f 100644
--- a/tests/manager.rs
+++ b/tests/manager.rs
@@ -2757,3 +2757,207 @@ fn test_extensions_screen_turns_extensions_off_and_saves_it() {
     tui.send_keys(ALT_Q);
     tui.wait_exit(WAIT);
 }
+
+// ── Proxy plugins ────────────────────────────────────────────────────────────
+
+/// The fake plugin: its name, the bytes of its pre-seeded zip, and its env.
+const PLUGIN: &str = "vocdoni-test";
+const PLUGIN_ZIP: &[u8] = b"PK fake bundle";
+const PLUGIN_KEY: &str = "VOCDONI_TEST_TOKEN";
+
+/// A fake claude-proxy on loopback: `GET /v1/claudio/plugins` lists
+/// [`PLUGIN`] (a bundle URL nothing listens on: the daemon must use its
+/// cache), every other path is a 404. Returns its `host:port`.
+fn fake_plugin_proxy() -> String {
+    use sha2::Digest;
+    use std::io::{Read, Write};
+    let sha = format!("{:x}", sha2::Sha256::digest(PLUGIN_ZIP));
+    let body = serde_json::json!({"plugins": [{
+        "name": PLUGIN, "version": "1.0.0",
+        "bundle": {"url": "https://127.0.0.1:9/never.zip", "sha256": sha},
+        "installed_id": "", "env": {PLUGIN_KEY: "s3cret"}}]})
+    .to_string();
+    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind fake proxy");
+    let addr = listener.local_addr().unwrap().to_string();
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
+                ("200 OK", body.as_str())
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
+/// Pre-seed the daemon's bundle cache under `cache` (`XDG_CACHE_HOME`) with
+/// [`PLUGIN`]'s zip and sidecar; the zip's path.
+fn seed_plugin_cache(cache: &std::path::Path) -> String {
+    use sha2::Digest;
+    let dir = cache.join("claudio/plugins");
+    fs::create_dir_all(&dir).unwrap();
+    let zip = dir.join(format!("{PLUGIN}.zip"));
+    fs::write(&zip, PLUGIN_ZIP).unwrap();
+    let sha = format!("{:x}", sha2::Sha256::digest(PLUGIN_ZIP));
+    fs::write(dir.join(format!("{PLUGIN}.sha256")), format!("{sha}\n")).unwrap();
+    zip.to_string_lossy().into_owned()
+}
+
+/// A fake claude whose `--help` lists `--plugin-dir` when `knows_plugins`,
+/// and which records each launch in `<dumps>/<n>/` (`argv`, one argument
+/// per line, and `env`) before the usual banner and `cat`.
+fn write_plugin_claude(h: &ManagerHarness, dumps: &std::path::Path, knows_plugins: bool) {
+    use std::os::unix::fs::PermissionsExt;
+    fs::create_dir_all(dumps).unwrap();
+    let help = if knows_plugins {
+        "echo '  --plugin-dir <paths...>  Load plugins from directories or zips'"
+    } else {
+        ":"
+    };
+    let dumps = dumps.display();
+    let script = format!(
+        r#"#!/bin/sh
+[ "$1" = "--version" ] && echo 'claude 0.0.0-fake' && exit 0
+if [ "$1" = "--help" ]; then {help}; exit 0; fi
+d="{dumps}/$(ls "{dumps}" | wc -l | tr -d ' ')"
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
+    let dir = dumps.join(n.to_string());
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
+/// Start a TUI with the fake proxy as the `env` profile and the bundle
+/// cache under the harness root; wait until the plugin list is cached.
+fn start_with_plugins(h: &ManagerHarness, proxy: &str) -> TuiProcess {
+    let cache = h.root.join("xdg-cache");
+    let tui = h.start_tui_with(|cmd| {
+        cmd.env("CLAUDIO_PROXY_URL", format!("tok@{proxy}"));
+        cmd.env("XDG_CACHE_HOME", &cache);
+    });
+    let plugins_json = h.config_home.join("claudio/plugins.json");
+    let deadline = Instant::now() + WAIT;
+    while !fs::read_to_string(&plugins_json).is_ok_and(|s| s.contains(PLUGIN)) {
+        assert!(
+            Instant::now() < deadline,
+            "the plugin list was never cached"
+        );
+        thread::sleep(Duration::from_millis(100));
+    }
+    tui
+}
+
+/// A proxy plugin reaches a new session: its bundle as `--plugin-dir <the
+/// daemon's cached zip>` and its env key. Turned off in the extensions
+/// screen (saved in config.toml), the next session has neither.
+#[test]
+fn test_proxy_plugin_reaches_new_sessions_until_turned_off() {
+    let h = ManagerHarness::new();
+    let dumps = h.root.join("launches");
+    write_plugin_claude(&h, &dumps, true);
+    let zip = seed_plugin_cache(&h.root.join("xdg-cache"));
+    let proxy = fake_plugin_proxy();
+    let mut tui = start_with_plugins(&h, &proxy);
+
+    wizard_pick_dir(&mut tui, &h.dirs[0]);
+    let (argv, env) = launch_dump(&dumps, 0);
+    let at = argv.iter().position(|a| a == "--plugin-dir");
+    assert_eq!(
+        at.and_then(|i| argv.get(i + 1)),
+        Some(&zip),
+        "argv: {argv:?}"
+    );
+    assert!(env.contains(&format!("{PLUGIN_KEY}=s3cret")), "{env}");
+    assert!(
+        !argv.iter().any(|a| a.contains("s3cret")),
+        "no secret in argv"
+    );
+
+    // The extensions screen lists it, ready on this host; Space turns it off.
+    tui.send_keys(ALT_I);
+    tui.wait_for(&format!("[x] {PLUGIN}"), Region::Screen, WAIT);
+    for _ in 0..4 {
+        tui.send_keys(DOWN_ARROW);
+    }
+    tui.wait_for("1.0.0 · env", Region::Screen, WAIT);
+    tui.wait_for("local: ready", Region::Screen, WAIT);
+    tui.send_keys(b" ");
+    tui.wait_for(&format!("[ ] {PLUGIN}"), Region::Screen, WAIT);
+    tui.wait_for(
+        &format!("extension {PLUGIN} off (saved)"),
+        Region::StatusBar,
+        WAIT,
+    );
+    let config = fs::read_to_string(h.config_home.join("claudio/config.toml")).unwrap();
+    assert!(
+        config.contains(&format!("[extensions]\n{PLUGIN} = false")),
+        "{config}"
+    );
+    tui.send_keys(ESC);
+    tui.wait_until(|s| !s.contains("Space toggle", Region::Screen), WAIT);
+
+    tui.send_keys(ALT_N);
+    wizard_pick_dir(&mut tui, &h.dirs[1]);
+    let (argv, env) = launch_dump(&dumps, 1);
+    assert!(!argv.iter().any(|a| a == "--plugin-dir"), "{argv:?}");
+    assert!(!env.contains(PLUGIN_KEY), "{env}");
+    tui.quit(WAIT);
+}
+
+/// A claude whose `--help` does not list `--plugin-dir` still starts: the
+/// daemon leaves the bundle out, and the env applies.
+#[test]
+fn test_proxy_plugin_bundle_is_left_out_for_an_old_claude() {
+    let h = ManagerHarness::new();
+    let dumps = h.root.join("launches");
+    write_plugin_claude(&h, &dumps, false);
+    seed_plugin_cache(&h.root.join("xdg-cache"));
+    let proxy = fake_plugin_proxy();
+    let mut tui = start_with_plugins(&h, &proxy);
+    wizard_pick_dir(&mut tui, &h.dirs[0]);
+    let (argv, env) = launch_dump(&dumps, 0);
+    assert!(
+        !argv.iter().any(|a| a.starts_with("--plugin-dir")),
+        "{argv:?}"
+    );
+    assert!(env.contains(&format!("{PLUGIN_KEY}=s3cret")), "{env}");
+    tui.quit(WAIT);
+}
diff --git a/tests/remote.rs b/tests/remote.rs
index 195cc47..1309c3b 100644
--- a/tests/remote.rs
+++ b/tests/remote.rs
@@ -8,7 +8,7 @@
 //! ```
 //!
 //! Tests `t1`, `t7`, and `t8` are pure unit-style tests (no SSH) and always
-//! run.  Tests `t2`–`t6` require a real SSH host.
+//! run.  Tests `t2`–`t6` and `t9` require a real SSH host.
 //!
 //! **Note on `__` diag commands:** `t2`–`t4` use `claudio __bootstrap`,
 //! `__connect-check`, and `__remote-session`.  These may later be gated
@@ -403,3 +403,70 @@ fn t8_probe_roundtrip() {
         &build[60..]
     );
 }
+
+// ── t9: plugin bundle on the remote ─────────────────────────────────────────
+
+/// A pre-seeded plugin bundle in the remote daemon's cache answers `Bundle`
+/// with its remote path, and a session spawned with it there has that path
+/// in its argv (when the remote claude knows `--plugin-dir`).
+#[test]
+fn t9_remote_plugin_bundle() {
+    let host = match test_host() {
+        Some(h) => h,
+        None => {
+            println!("SKIP t9: set CLAUDIO_SSH_TEST_HOST to run SSH tests");
+            return;
+        }
+    };
+    // The smallest valid zip: an empty end-of-central-directory record.
+    let zip: Vec<u8> = [b'P', b'K', 5, 6].into_iter().chain([0u8; 18]).collect();
+    let sha = {
+        use sha2::Digest;
+        format!("{:x}", sha2::Sha256::digest(&zip))
+    };
+    let octal: String = zip.iter().map(|b| format!("\\{b:03o}")).collect();
+    let seed = format!(
+        "d=\"${{XDG_CACHE_HOME:-$HOME/.cache}}/claudio/plugins\" && mkdir -p \"$d\" && chmod 700 \"$d\" \
+         && printf '{octal}' > \"$d/claudio-t9.zip\" && printf '{sha}\\n' > \"$d/claudio-t9.sha256\" \
+         && echo \"$d/claudio-t9.zip\""
+    );
+    let (remote_zip, ok) = ssh_run(&host, &seed);
+    assert!(ok, "could not seed the remote cache");
+
+    let mut child = Command::new(BINARY)
+        .args(["__remote-bundle", &host, "/tmp", "claudio-t9", &sha])
+        .stdout(std::process::Stdio::piped())
+        .spawn()
+        .expect("run claudio __remote-bundle");
+    let stdout = child.stdout.take().expect("piped stdout");
+    let mut lines = std::io::BufRead::lines(std::io::BufReader::new(stdout));
+    let reply: serde_json::Value =
+        serde_json::from_str(&lines.next().expect("a Bundle reply").unwrap()).unwrap();
+    assert_eq!(reply["path"].as_str(), Some(remote_zip.as_str()), "{reply}");
+    assert_eq!(reply["installed"], false);
+    println!("[t9] Bundled {{ path: {remote_zip} }} ✓");
+
+    let pid = lines.next().expect("a pid").unwrap();
+    let pid = pid.strip_prefix("pid=").expect("pid=<n>").to_owned();
+    let (args, _) = ssh_run(&host, &format!("ps -o args= -p {pid}"));
+    let (help, _) = ssh_run(
+        &host,
+        "$HOME/.local/bin/claude --help 2>/dev/null || claude --help",
+    );
+    if help.contains("--plugin-dir") {
+        assert!(
+            args.contains(&format!("--plugin-dir {remote_zip}")),
+            "argv: {args}"
+        );
+        println!("[t9] the remote session's argv has the bundle ✓");
+    } else {
+        assert!(!args.contains("--plugin-dir"), "argv: {args}");
+        println!("[t9] the remote claude has no --plugin-dir: left out ✓");
+    }
+    for _ in lines {}
+    assert!(child.wait().expect("wait for claudio").success());
+    let _ = ssh_run(
+        &host,
+        "rm -f \"${XDG_CACHE_HOME:-$HOME/.cache}/claudio/plugins/claudio-t9.\"*",
+    );
+}
~~~~~~

Run: `rustfmt --edition 2021 --check tests/remote.rs src/remote/diag.rs` → no output.
Run: `umask 022; cargo test --test manager proxy_plugin -- --test-threads=1`
Expected: `test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 50 filtered out` (run it 3 times: it was stable 3/3).
Run: `umask 022; env -u CLAUDIO_SSH_TEST_HOST cargo test --features diag --test remote -- --test-threads=1`
Expected: `test result: ok. 9 passed` (t2–t6 and t9 print SKIP).
Do not run t9 against a host: it starts a real claude there (the controller runs it for AC6).

Step 3: commit
`/usr/bin/git commit -m "test: proxy plugins end to end, and on a remote host" -- tests/manager.rs tests/remote.rs src/remote/diag.rs src/main.rs`

Interfaces this task provides to later tasks: `claudio __remote-bundle HOST CWD NAME SHA256` (diag feature only).


### Task 9: docs
Model: fast   Repo: /volumes/repos/claudio   Files: README.md, docs/extensions.md, CLAUDE.md
Goal: the browser login, plugins, `CLAUDIO_NO_BROWSER` and `r` in the README; a "Proxy plugins" section in docs/extensions.md; CLAUDE.md's module list (`daemon/bundle.rs`, `tui/plugins.rs`, `proxy/{login,plugins}.rs`), the secrets rule (`config.toml` and `plugins.json` hold secrets, 0600; the journal and `state.json` never) and the fail-safe rule (`Bundle` is an op, not a `SpawnSpec` field). `cli::HELP_APPENDIX` already lists `CLAUDIO_NO_BROWSER` (Task 7).
Patch: .delivery/2026-11-06-session-plugins/patches/task-9.patch

Step 1: write the failing test
None (documentation). Check instead that every name the docs give exists:
`grep -n "CLAUDIO_NO_BROWSER" src/cli.rs src/proxy/login.rs` and `grep -n "fn set_plugins\|Source::Plugin" src/tui/extensions/mod.rs` print matches.

Step 2: implement
`/usr/bin/git apply .delivery/2026-11-06-session-plugins/patches/task-9.patch`

~~~~~~diff
diff --git a/CLAUDE.md b/CLAUDE.md
index b830ce9..97ead94 100644
--- a/CLAUDE.md
+++ b/CLAUDE.md
@@ -142,6 +142,15 @@ daemons coexist.
   bounded output, sanitized text, no textconv/ext-diff, repo env scrubbed.
 - `update.rs` — `claude update` (or the official installer) on request, in its
   own process group, killed as a group on timeout.
+- `bundle.rs` — `Bundle` (a proxy plugin's zip for `--plugin-dir`): checks
+  the op itself (name `[a-z0-9-]{1,40}`, 64-hex hash, https), skips it when
+  `$CLAUDE_CONFIG_DIR/plugins/installed_plugins.json` has the plugin, else
+  `<cache>/plugins/<name>.zip` whose `.sha256` sidecar matches, else
+  downloads it (30 s connect, 120 s, 64 MiB, https redirects only) to a temp
+  file, verifies, renames into place. 0700 dir, ≤ 32 zips, one download per
+  name at a time, no partial file on failure. Replies from a spawned task.
+  `Daemon::claude_flags` (one `claude --help`) says whether claude knows
+  `--plugin-dir`; `command()` drops the pairs when it does not.
 - `host.rs`, `stats.rs` — host facts for `Welcome`, directory listing, CPU and
   memory samples. `ctl.rs` — `claudio daemon status|stop|restart` (status
   shows the claude path and version the daemon uses).
@@ -201,7 +210,8 @@ is idle (checked every 2 min).
   `ui/reader.rs`.
 - `formulas.rs` — the lines of the reader's formulas list (Tab).
 - `extensions/` — the extension registry (`mod.rs`: built-ins latex,
-  diagram, search, snippets; `Registry`, `ActionResult`), external
+  diagram, search, snippets; `Registry`, `ActionResult`; the proxy plugins
+  as `Source::Plugin` rows, `Registry::set_plugins`), external
   manifests (`manifest.rs`: `~/.config/claudio/extensions/<name>/`, the
   HTML-tag rule, admission, at most 16), protocol v1 (`protocol.rs`: request
   lines, result caps and sanitizing) and the extension host (`host.rs`: one
@@ -214,8 +224,10 @@ is idle (checked every 2 min).
   details, toggles. A toggle flips the registry at once and emits
   `Effect::SetExtension` (host; externals) and `Effect::SaveExtension`
   (`mod.rs` writes `[extensions] name = on` with `config::save_extension`,
-  a minimal line edit of config.toml, never a whole-file rewrite). Drawn by
-  `ui/extensions.rs`. The leader keys `u` and `e` are reserved.
+  a minimal line edit of config.toml, never a whole-file rewrite); a
+  plugin's toggle also updates `App::plugins.config`, and `r` refetches the
+  plugin lists. Drawn by `ui/extensions.rs`. The leader keys `u` and `e`
+  are reserved.
 - `leader.rs` — Alt+z: the leader popup, action input, list picker,
   documents (a reader view) and paste (attached claude session with
   bracketed paste on only, never Enter). `builtins.rs` — the built-in
@@ -240,6 +252,12 @@ is idle (checked every 2 min).
 - `state.rs` — `ClientState`: persisted session order, names and recent dirs
   (`state.json`).
 - `proxy_state.rs` — per-profile proxy stats and per-session credential cache.
+- `plugins.rs` — proxy plugins, pure: each profile's list (`plugins.json` at
+  start, `Effect::FetchPlugins` at start, every 15 min and on `r`), the
+  registry's `Source::Plugin` rows, each `(host, plugin)` `Bundle` answer,
+  and what a spawn gets (`--plugin-dir` per ready bundle, the plugin env).
+  A spawn whose host has not answered for a bundle is deferred
+  (`Effect::SpawnWithProxy { bundles }`; `tui/mod.rs` waits ≤ 20 s).
 - `notifications.rs` — debounce logic for attention notifications.
 - `selection.rs` — copy-on-select (`[ui] copy_on_select`, default on): the
   pure `Selection` (anchor/head spans, cell/word/line unit, `contains`),
@@ -276,6 +294,11 @@ is idle (checked every 2 min).
   for what a launched claude inherits (session markers scrubbed, an API key
   dropped when a proxy token is set).
 - `cmd.rs` — `claudio proxy login|status|logout|use`.
+- `login.rs` — the browser sign-in (`/v1/claudio/login/start|poll`, device
+  style: the member types the code; `--token` pastes one instead).
+- `plugins.rs` — `GET /v1/claudio/plugins`, `admit` (names, https, hash,
+  env keys a plugin may not set, caps), the `plugins.json` cache (0600, one
+  entry per profile; a 401/403 drops it). `Debug` never shows a value.
 - `api.rs` — HTTP client for `/v1/claudio/config`, `/models`, `/me/stats`,
   `/pool/health` and `/session` (which credential a claude session is on).
 
@@ -321,14 +344,19 @@ is idle (checked every 2 min).
 - **Don't send `ESC` immediately before `Alt+key` in tests.** `ESC ESC q` is
   ambiguous — the VT parser may interpret it as two separate `ESC` sequences.
   Use the crossterm key-event encoding instead.
-- **Secrets never go in argv or state files.** Proxy tokens are passed only
-  through `Spawn.env` over the Unix socket. The journal and `state.json` store
-  only profile names, never tokens.
+- **Secrets never go in argv or state files.** Proxy tokens and plugin env
+  are passed only through `Spawn.env` over the Unix socket. The journal and
+  `state.json` never hold secrets (profile names only); `config.toml` and
+  `plugins.json` do, both mode 0600. Logs and notices name keys, never
+  values. `--plugin-dir` is added by claudio's spawn path only, never parsed
+  from the user's args.
 - **New request ops must fail safe on old daemons.** An old daemon answers an
   unknown op with `Error "unsupported op…"`; `App::older_daemon_notice` turns
   that into "run `claudio daemon restart`". Never add a field to an existing
   op that an old daemon would silently ignore into wrong behaviour (that is
-  why terminals are `SpawnShell`, not `Spawn{kind}`).
+  why terminals are `SpawnShell`, not `Spawn{kind}`, and plugin bundles are
+  a `Bundle` op, not a `SpawnSpec` field: an old daemon's error becomes
+  `daemon too old` and the session starts without the bundle).
 - **A daemon is replaced only by a newer build.** `freshness::compare` orders
   builds by version, then binary mtime; "different" is not enough, or two
   binaries (a release and a dev build) would replace each other's daemon
diff --git a/README.md b/README.md
index f4189c0..f3d4591 100644
--- a/README.md
+++ b/README.md
@@ -80,10 +80,14 @@ curl -fsSL https://github.com/p4u/claudio/releases/latest/download/install.sh |
 [claude-proxy](https://github.com/p4u/claude-proxy) is a multi-subscription credential proxy for Claude Code: sessions authenticate through it instead of a local `claude` login.
 
 ```bash
-claudio proxy login https://proxy.example.com   # prompts for your token; it is never an argument
+claudio proxy login https://proxy.example.com   # sign in through the browser
 claudio                                         # new sessions now use the proxy by default
 ```
 
+`claudio proxy login` prints a page and a one-time code, opens the page in a browser when there is a display (`CLAUDIO_NO_BROWSER=1` only prints it), and waits while you sign in and type the code there. The proxy then hands claudio your own token, saved with your e-mail in the profile (`claudio proxy status` shows it). Without a URL it signs in again to the default profile's proxy. `--token`, or a proxy without browser sign-in, asks for a pasted token instead; a token is never an argument.
+
+The proxy can also serve **plugins**: a pinned bundle of skills, agents, hooks and MCP servers, plus the env (API keys) its servers need. Every session claudio opens with that proxy profile (manager, `--plain`, SSH hosts) loads each plugin with `--plugin-dir` and gets its env, unless you installed that plugin in Claude Code yourself. Nothing is installed into `~/.claude`. Each host's daemon downloads a bundle once per version into `~/.cache/claudio/plugins/` (https only, hash-checked, 64 MiB at most). The list and its env are cached in `~/.config/claudio/plugins.json` (mode 0600), so a session still gets them when the proxy is unreachable, and refreshed every 15 minutes. `claudio proxy logout` deletes them. Plugins are listed in the extensions screen (`Alt+i`), where you can turn one off for new sessions.
+
 - `claudio --no-proxy` or `claudio --proxy <profile>` overrides the default for one run.
 - `CLAUDIO_PROXY_URL=<token>@proxy.example.com claudio` uses a proxy for one run without saving a profile.
 - The status bar shows which upstream credential the session is running on, and `Alt+s` opens the proxy stats.
@@ -215,7 +219,7 @@ In the pane, each tag is replaced by a one-line Unicode rendering in cyan, such
 
 `Alt+z u` opens the reader view too. You can write your own extensions in any language: a directory in `~/.config/claudio/extensions/` with a manifest and a program that claudio runs and talks to over JSON lines. See [docs/extensions.md](docs/extensions.md) and the example in [`examples/extensions/keys/`](examples/extensions/keys/). Each extension can be turned off under `[extensions]` in `config.toml`.
 
-`Alt+i` (or `Alt+z e`) opens the extensions screen: one row per extension, built-in or external, with its keys and lenses, and below the list the selected one's description, actions and lenses. `Space` or `Enter` turns it on or off. The change applies at once (its keys leave the `Alt+z` popup, its lenses stop rendering) and is saved as `name = true|false` under `[extensions]` in `config.toml`, the rest of the file untouched. claude's instructions change for new sessions; `Alt+e` restarts one with them. `Esc` or `Alt+i` closes the screen.
+`Alt+i` (or `Alt+z e`) opens the extensions screen: one row per extension, built-in, external or proxy plugin, with its keys and lenses, and below the list the selected one's description, actions and lenses (for a plugin, its version, profile and state on each host). `Space` or `Enter` turns it on or off; `r` fetches the plugin lists again. The change applies at once (its keys leave the `Alt+z` popup, its lenses stop rendering) and is saved as `name = true|false` under `[extensions]` in `config.toml`, the rest of the file untouched. claude's instructions change for new sessions; `Alt+e` restarts one with them. `Esc` or `Alt+i` closes the screen.
 
 ### Keeping claude up to date
 
@@ -296,7 +300,8 @@ latex = true                    # ask claude to write math in <latex> tags, rend
 
 [extensions]                    # every extension is on unless turned off here
 diagram = false                 # e.g. no Mermaid instruction or drawing
-                                # (Alt+i turns them on and off and writes this table)
+                                # (Alt+i turns them on and off and writes this table;
+                                # proxy plugins too, by name)
 
 [keys]                          # rebind any manager key
 new_session = "alt+n"
@@ -308,7 +313,7 @@ extensions  = "alt+z"           # the extensions' leader key
 extensions_menu = "alt+i"       # the extensions screen
 ```
 
-`CLAUDIO_NO_UPDATE_CHECK=1` disables both the claudio and the claude release checks.
+`CLAUDIO_NO_UPDATE_CHECK=1` disables both the claudio and the claude release checks. `CLAUDIO_NO_BROWSER=1` keeps `claudio proxy login` from opening a browser.
 
 ---
 
diff --git a/docs/extensions.md b/docs/extensions.md
index 24fe30a..1837d30 100644
--- a/docs/extensions.md
+++ b/docs/extensions.md
@@ -27,19 +27,21 @@ screen (also `Alt+i`).
 ### The extensions screen (`Alt+i`)
 
 ```
-[x] latex     built-in  —          <latex>
-[ ] diagram   built-in  —          ```mermaid
-[x] search    built-in  Alt+z s    —
-[x] snippets  built-in  Alt+z p/P  —
-[x] keys      external  Alt+z k    <keys>
+[x] latex             built-in  —          <latex>
+[ ] diagram           built-in  —          ```mermaid
+[x] search            built-in  Alt+z s    —
+[x] snippets          built-in  Alt+z p/P  —
+[x] keys              external  Alt+z k    <keys>
+[x] vocdoni-internal  plugin    —          —
 ```
 
 One row per extension: the built-ins, then the external ones that loaded
-(one that failed to load is not listed; its notice at start said why).
+(one that failed to load is not listed; its notice at start said why), then
+the plugins of your proxy profiles (see below).
 Below the list, the selected extension's description, its actions and its
 lenses. `↑`/`↓`, `PgUp`/`PgDn`, `Home`/`End` and the mouse wheel move;
-`Space` or `Enter` turns the selected extension on or off; `Esc` or `Alt+i`
-closes the screen.
+`Space` or `Enter` turns the selected extension on or off; `r` fetches the
+plugin lists again; `Esc` or `Alt+i` closes the screen.
 
 A change applies at once: the extension's keys leave the `Alt+z` popup, its
 tag lenses stop rendering in the pane (at the next draw) and in the reader,
@@ -74,6 +76,34 @@ host's sessions. On a host whose daemon has `[claude] latex = false`, a new
 client overlays no `<latex>` tags for that host's sessions (the daemon extracts
 none); v0.5.0 clients still get them through `Latex`.
 
+### Proxy plugins
+
+A claude-proxy profile can serve plugins (`GET /v1/claudio/plugins`): for
+each, a pinned bundle (a zip of skills, agents, hooks and MCP servers, by
+URL and SHA-256) and the env its servers need. claudio fetches the list of
+every profile in use at start, every 15 minutes, after `claudio proxy login`
+and on `r` in the extensions screen, and caches it in
+`~/.config/claudio/plugins.json` (mode 0600: it holds the plugins' secrets).
+
+A plugin's row shows its version and profile; its details say, per
+connected host, `ready`, `downloading`, `installed in Claude Code (bundle
+skipped)`, `download failed: …` or `daemon too old`, and for the list
+`proxy unreachable, cached <age>` when the proxy did not answer. A plugin
+claudio refuses (a bad name, URL or hash, an env key it may not set such as
+`PATH`, `LD_*` or `ANTHROPIC_*`, a name another extension has) is listed as
+`refused: <reason>` and never used.
+
+Each session started with that profile gets `--plugin-dir <zip>` for every
+plugin that is on and the plugin's env (the proxy's own env wins on a shared
+key). The zip is the host's own copy: each daemon downloads it once per
+version into `~/.cache/claudio/plugins/` (https only, 64 MiB, hash-checked)
+and skips it when Claude Code there has the plugin installed. A new session
+waits up to 20 s for a bundle still downloading, then starts without it (a
+notice says so). A claude that does not know `--plugin-dir` starts without
+the bundles. Turning a plugin off is saved as `name = false` under
+`[extensions]` and applies to new sessions; `Alt+e` keeps a session's
+bundles and gives it the current env.
+
 Snippets are files in `~/.config/claudio/snippets/` (`*.md` or `*.txt`, the
 file name without its extension is the snippet's name). A paste never presses
 Enter: the text lands in claude's prompt for you to edit or send. It needs
~~~~~~

Run: `umask 022; cargo test --locked --bin claudio` → `test result: ok. 1044 passed; 0 failed` (unchanged).

Step 3: commit
`/usr/bin/git commit -m "docs: proxy plugins and browser login" -- CLAUDE.md README.md docs/extensions.md`

Interfaces this task provides to later tasks: none.


## Merge and release
<!-- Not copied into briefs; the controller executes this after the gate audit. -->
1. Full suites, on the tip of `feat/session-plugins`, each alone on the machine (measured in the scratch on 39ab39d):
   - `umask 022; make test-unit` → `test result: ok. 1044 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`, and the dev build has 0 warnings (about 18 s; on the known base flake, rerun once).
   - `umask 022; make test-manager` → `test result: ok. 52 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` (about 165 s).
   - `umask 022; env -u CLAUDIO_SSH_TEST_HOST cargo test --features diag --test remote -- --test-threads=1` → `test result: ok. 9 passed` (compiles again since Task 8).
   - `make` → release build OK. `make static` → `statically linked` (not run in the scratch: needs the musl target; CI's release job builds it).
   - `rustfmt --edition 2021 --check` on each file of Global Constraints' clean list and on `src/proxy/{plugins,login}.rs`, `src/daemon/bundle.rs`, `src/tui/plugins.rs` → nothing.
   - Clippy as in Global Constraints: no finding on a line this branch adds (the base's 73 warnings and the `wizard.rs:193` error remain).
2. AC6 (controller, gated, real claude on the host): `CLAUDIO_SSH_TEST_HOST=z6 cargo test --features diag --test remote t9 -- --test-threads=1 --nocapture` → `[t9] Bundled { path: … } ✓` and either `the remote session's argv has the bundle ✓` or `the remote claude has no --plugin-dir: left out ✓`.
3. AC5 (manual, owner or controller) needs the proxy: claude-proxy `feat/claudio-plugins` merged and deployed with SSO and `CLAUDE_PROXY_OAUTH_ALLOWED_EMAILS` set, and `vocdoni-internal` registered (`claude-proxy plugins set …`, `plugins env set …`). Then, with the branch's `target/release/claudio` on an isolated config (`XDG_CONFIG_HOME`, `XDG_RUNTIME_DIR`, `XDG_CACHE_HOME` under `~/.cache/claudio/ac5/`; `HOME` real so claude is logged in): `claudio proxy login https://<dev proxy>` through the browser → `Signed in as …`, `Plugins: vocdoni-internal (…)`; the TUI's `Alt+i` shows `vocdoni-internal  plugin` and `local: ready`; a new session lists the bundle's skills (`/skills` or `/plugin` in claude); the same with a session on `z6`. Stop the isolated daemon and remove `~/.cache/claudio/ac5/` afterwards. A failure is a fix task.
4. Gate audit of the branch; fixes re-reviewed.
5. Merge: `/usr/bin/git checkout main && /usr/bin/git merge --ff-only feat/session-plugins` (`main` is dacfede = v0.6.1; the branch is linear on top of it: the four spec commits, then the nine task commits).
6. Release v0.7.0 (new feature; the convention of `git show --stat dacfede`):
   - `version = "0.7.0"` in Cargo.toml, `cargo update --workspace --offline` (only claudio's own entry), `git diff --stat` shows `Cargo.lock | 2 +-` and `Cargo.toml | 2 +-`; `cargo build --locked` succeeds.
   - `/usr/bin/git commit -m "release v0.7.0" -- Cargo.toml Cargo.lock` (no trailer).
   - `/usr/bin/git tag v0.7.0` (the release workflow checks that the tag matches Cargo.toml).
7. **Owner OK required before**, each on its own:
   - deploying claude-proxy `feat/claudio-plugins` (a dependency: AC5 and any real use need it; the claudio release works without it, degrading to today's behaviour);
   - `/usr/bin/git push origin main`;
   - `/usr/bin/git push origin v0.7.0` (starts the release workflow: builds and publishes the binaries);
   - publishing or announcing the release to members (they run `claudio proxy login` once).
8. Post-release checks: `gh run list --limit 5` → the `ci` run on `main` (Linux and macOS) and the `release` run on `v0.7.0` succeed (`gh run watch <id>`); `gh release view v0.7.0` lists every target of `release.yml`; `claudio upgrade` from v0.6.1 reaches 0.7.0 and `claudio daemon status` shows the new daemon; a session on an old (v0.6.1) remote daemon still starts, its plugin row saying `daemon too old`.

## Scratch verification record
- Where: `~/.cache/claudio/scratch/session-plugins`, a detached worktree of /volumes/repos/claudio at 2c68a17; one commit per task (c09d278, 3ec979c, 177b38d, c28d80f, 099250b, e7613d1, c0ed0c3, 63baa3f, 39ab39d). The patches are `git format-patch -1` of those commits.
- Replay: the nine patches were applied with `git apply`, in order, onto a fresh worktree at 2c68a17 (`~/.cache/claudio/scratch/session-plugins-replay`), building and running the unit suite after each: every patch applied cleanly; Step 1's tests-only applies of Tasks 2 and 3 failed to build with exactly the errors quoted there; dev-build warnings 33, 30, 30, 9, 5, 3, 0, 0, 0; unit totals 1013, 1020 (one run hit the base flake `daemon::host::tests::probe_runs_claude_with_the_given_path`: 1019 + 1), 1023, 1031, 1032, 1037, 1044, 1044, 1044. The replayed tree is identical to the scratch tip (`diff -r`, build dirs excluded).
- Unit totals: 1003 (base) → 1013 (Task 1) → 1020 (2) → 1023 (3) → 1031 (4) → 1032 (5) → 1037 (6) → 1044 (7) → 1044 (8) → 1044 (9). Manager: 50 → 50 (Task 6 run) → 52 (Task 8; the two new tests 3/3 stable). Remote (no host): 9 passed.
- Flakiness: one unit run on the Task 5 tree hit the known base flake `daemon::tests::freshness::an_outdated_idle_daemon_is_replaced_and_its_sessions_come_back`; the rerun passed.
- Not verified here: macOS (CI builds and unit-tests it), `make static`, the release workflow, a real claude or proxy (AC5, AC6 are manual or gated), an old v0.6.1 daemon against this client (covered by the `unsupported op` unit test).
