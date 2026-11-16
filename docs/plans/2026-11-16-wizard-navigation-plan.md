# New-session wizard: folder navigation and finding a conversation: implementation plan

Spec: docs/specs/2026-11-16-wizard-navigation-design.md
Class: architectural
Repos and branches: /home/p4u/claudio → feat/wizard-nav (base 115925b)

Implementers work in the main checkout `/home/p4u/claudio` on `feat/wizard-nav`, one task at a
time, in the order below. Every code block was applied, in this order, in a scratch worktree of
main 3bf7d67 and run there (the gates under "Merge and release" too); each "Expected" line is that run's real output (cargo's own lines;
the RTK hook may summarize them, `rtk proxy <cmd>` shows them raw). A diff applies with
`git apply` from the repo root (save it to a file under `~/.cache/`, never /tmp); the diff of a
task already contains the tests of its Step 1, so Step 1 is "add these tests", Step 2 "apply the
rest of the diff". The red step was observed where its Expected line quotes output (Task 6);
where it says "the build fails" or "all 7 tests fail" it was not run on its own (the tests name
items the task adds, or testids the old page lacks).

Where the spec is loose or wrong against the code, this plan decides (the controller may rule
otherwise before the task that depends on it):
- **A8 and `proto.rs`.** `proto.rs` is at its baseline (878). `dir_entries_reply` goes in a new
  `proto/listing.rs` with `DirEntry` (moved out of `proto.rs`, re-exported), so `proto::DirEntry`
  and `proto::dir_entries_reply` keep their paths and `proto.rs` shrinks (spec "Daemon" 5 says
  "next to `search_reply`").
- **A8 and `wizard.rs`.** ACCEPTANCE A8: "a commit that only moves files may rename entries (same
  counts) and nothing else". Task 3 renames the four function entries and leaves `file
  crates/tui/src/wizard.rs 1138` as it is, so the moved-out lines are the room Tasks 4-5 use.
  `HostStep::on_key` (110) and `HostStep::refilter` (85) stay at baseline: Task 5 adds the
  search row by taking `Enter` and the auto-selection out of them into new functions.
- **`run_effect` (123, no room).** Task 5 shortens its `Effect::Request` "not connected" arm by
  four lines and adds one arm calling `search_edge::run` (spec: "run by a function outside
  `run_effect`").
- **`dir_entries_reply` measures each entry** (`serde_json` size of the entry plus a comma)
  instead of re-encoding the whole reply after every drop (`search_reply`'s loop would encode a
  1 MiB reply thousands of times on a folder of long names).
- **Listing errors in the TUI** need the requested path: `ReplyTo::DirEntries` becomes
  `ReplyTo::DirEntries { path }` (a TUI enum, not the wire), so an error for a path the wizard no
  longer lists is dropped like a stale listing. The error shows with the home abbreviated
  (`cannot list ~/afile: Not a directory (os error 20)`).
- **The core search helper's errors** are the daemon's text unchanged; the web keeps passing
  each failure through its `daemon_error` (as `search_host` did), the TUI shows them in one
  notice.

## Global Constraints

- Commit convention: conventional commits `type(scope): lowercase summary`; no AI attribution,
  no Co-Authored-By or similar trailers; one logical change per commit (ACCEPTANCE F2). Each
  task gives its commit messages; use them as written.
- Never push; commit with explicit paths only (`git commit -m … -- <paths>`, new files
  `git add <path>` first); never git add -A; never git stash; never switch branches.
- Build output never in /tmp: cargo's default `target/` (and `make check`'s `target/check`) are
  in the repo. Run tests with `TMPDIR=$HOME/.cache/cl-tmp` (create it); a TMPDIR whose path
  holds "claudio" fails two existing claudio-api tests (`print::hooks::tests::relay_command_*`).
- Leave files you did not create alone: never stage, revert or format someone else's change
  (`cargo fmt` only after checking `git status`).
- ACCEPTANCE.md applies; each task names its rules. A2/A3: claudio-core names no addon, the web
  uses the core's public API only (the new `app::search` joins the services the web uses, see
  Task 8's CLAUDE.md line). A5: the multi-host search (fan-out, deadline, retry, merge) lives
  once, in `crates/core/src/app/search.rs`, used by the web server and the TUI; each front-end
  keeps only its own navigation state. A6: `app.rs` and every `impl App` stay pure; the search
  runs in an effect. A7: the console TUI stays plain (no new renderer). A8 size budgets, below.
  C2: inputs bounded (`MAX_SCAN`, the measured reply, `PATH_MAX` on the web as today, the
  2-character query floor). F1: docs in the same branch (Task 8).
- A8 (`scripts/check_rules.py`, `scripts/size-baseline.txt`): these are at their baseline and
  must not grow: `crates/core/src/daemon/server.rs` (900), `crates/core/src/proto.rs` (878),
  `crates/tui/src/interaction.rs::on_reply` (111), `crates/tui/src/lib.rs::run_effect` (123),
  `HostStep::on_key` (110) and `HostStep::refilter` (85), `Wizard::on_key` (155) and
  `Wizard::refilter` (85). `crates/tui/src/lib.rs` has 3 lines of room (814), `event_loop` 10
  (229). New files stay under 800 production lines, functions under 80. Run
  `python3 scripts/check_rules.py` before every commit; expected `rules: 0 problem(s)`.
- No wire or protocol change: only the existing ops (`ListDir`/`fs.list`,
  `RecentProjects`/`fs.projects`, `Search`/`search`, `ListClaudeSessions`/`claude.sessions`,
  the spawn). The `DirEntries` reply keeps its shape; `docs/web-protocol.md` §search behaviour
  is unchanged.
- Tests: the PTY harness asserts on the screen model only; never ESC right before an Alt+key;
  manager suites single-threaded (`-- --test-threads=1`); fake claudes ignore `--help` and
  `--version`; test configs have `update_check`/`remote_check`/`install = "off"` (the harness
  writes them).
- Web: DESIGN.md tokens only (`styles/tokens.css` variables, never a literal colour, radius or
  font); no inline scripts or handlers (the import map stays the only inline script); pure
  modules under `app/` (not `app/ui/`) import no bare specifiers; stable testids per the spec:
  kept `wizard-dir-input`, `wizard-browse`; removed `wizard-dirs`, `wizard-dir`,
  `wizard-browser`; new `wizard-dir-list`, `wizard-dir-row` (`data-path`), `wizard-dir-use`,
  `wizard-dir-error`, `wizard-find`, `wizard-find-row` (`data-conversation`); zero CSP
  violations in the e2e (`window.__claudioCsp`).

## Review Focus

- `crates/core/src/daemon/host.rs` `list_dir_with`/`scan`: read at most `MAX_SCAN`, kind from
  `DirEntry::file_type` (one `metadata` for a symlink only), sort folders first then
  case-insensitive then bytes, cut at 5000, and only then the git probe, on kept folders only.
- `crates/core/src/proto/listing.rs` `dir_entries_reply`: the measured cut keeps the frame
  under `MAX_FRAME` with escapes; it drops from the end (files first); `truncated` is kept from
  the scan.
- `crates/core/src/app/search.rs`: the 10 s deadline per host counts the web's wait for its
  slot; exactly one retry, 500 ms after `search busy`; merge newest first; every failed host
  named; no host means an empty `Found`.
- The web combobox (`addons/web/web/app/ui/dirfield.js`, `app/dirnav.js`): Enter/Esc/Tab/→/
  Backspace exactly as spec "Keys"; Enter and Esc never reach the wizard's Enter-starts handler
  or the Dialog while the list is open; the "use" row never names the parent; "(not found)"
  only with a whole listing (arrived, not cut, < 5000 entries).
- `~` resolution before `claude.sessions` and `session.spawn` (`resolveTilde`); a `~` path
  cannot start until the host's home is known.
- Stale replies: the web listing's sequence guard (`ui/dirfield.js`), the find field's
  (`ui/wizard-find.js`), the TUI's `SearchAllDone` `gen` guard and `ReplyTo::DirEntries { path }`.
- The TUI search row: order (hosts, search, connect to), default (search row when nothing
  else matches; "connect to" never the default while it shows), Enter emits `SearchAll` and
  closes the wizard.
- A8: the counts above after every task.

## Task order and parallelism

| Task | Depends on | Repo/files | May run in parallel with |
|---|---|---|---|
| 1 daemon listing | none | crates/core/src/daemon/{host,server}.rs, crates/core/src/proto.rs, crates/core/src/proto/listing.rs | none (one checkout) |
| 2 core search helper, web uses it | none | crates/core/src/app/{mod,search}.rs, addons/web/src/ws.rs | none |
| 3 TUI: move the host step (move only) | none | crates/tui/src/{wizard,wizard_host,lib}.rs, scripts/size-baseline.txt | none |
| 4 TUI: listing errors and the cut line | 1, 3 | crates/tui/src/{wizard,interaction,app}.rs, crates/tui/src/ui/mod.rs, crates/tui/tests/manager.rs | none |
| 5 TUI: "Search conversations" row | 2, 3, 4 | crates/tui/src/{wizard_host,wizard,interaction,app,builtins,lib,search_edge}.rs, crates/tui/src/ui/wizard_host.rs, crates/tui/tests/manager.rs | none |
| 6 web: pure navigator and hits modules | none | addons/web/web/app/{dirnav,hits}.js, addons/web/web/tests/{dirnav,hits}.test.mjs | none |
| 7 web: combobox, find field, mock, e2e | 1, 6 | addons/web/web/app/ui/{wizard,dirfield,wizard-find,search}.js, addons/web/web/app/wizard.js, addons/web/web/tests/wizard.test.mjs, addons/web/web/styles/components.css, addons/web/web/dev/mock-server.mjs, addons/web/web/e2e/tests/wizard-nav.spec.mjs | none |
| 8 docs | 1-7 | docs/manager-design.md, docs/web.md, README.md, docs/extensions.md, docs/web-protocol.md, CLAUDE.md | none |

Every task runs in the one main checkout, so nothing runs in parallel.

## Tasks

### Task 1: daemon: sort before the cut, enrich only what is kept, measure the reply
Model: sonnet   Repo: /home/p4u/claudio   Files: crates/core/src/daemon/host.rs, crates/core/src/daemon/server.rs, crates/core/src/proto.rs, crates/core/src/proto/listing.rs (new)
Goal: spec "Daemon: `host::list_dir`" 1-5 and AC1. Rules: A8 (`server.rs` and `proto.rs` shrink), C2 (`MAX_SCAN`, a reply that always fits a frame).

Step 1: write the failing tests

In `crates/core/src/daemon/host.rs`'s `mod tests`, before `fn tilde_expansion`:

```rust
    /// A fresh folder under the temp dir.
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("claudio-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
```

```rust
    #[test]
    fn folders_come_first_before_the_cut_and_only_they_are_probed() {
        let dir = scratch("listing");
        for i in 0..6000 {
            std::fs::write(dir.join(format!("file{i:04}")), b"").unwrap();
        }
        for name in [
            "delta", "Alpha", "charlie", "Bravo", "echo", "Foxtrot", "golf", "Hotel", "india",
            "Juliet",
        ] {
            std::fs::create_dir(dir.join(name)).unwrap();
        }
        let probes = std::cell::Cell::new(0);
        let (_, entries, truncated) = list_dir_with(dir.to_str().unwrap(), |_| {
            probes.set(probes.get() + 1);
            None
        })
        .unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        let first: Vec<&str> = entries[..10].iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            first,
            [
                "Alpha", "Bravo", "charlie", "delta", "echo", "Foxtrot", "golf", "Hotel", "india",
                "Juliet"
            ]
        );
        assert!(entries[..10].iter().all(|e| e.dir));
        assert!(entries[10..].iter().all(|e| !e.dir && e.git.is_none()));
        assert_eq!(entries.len(), MAX_DIR_ENTRIES);
        assert!(truncated);
        assert_eq!(
            probes.get(),
            10,
            "the git probe runs on the kept folders only"
        );
    }
```

```rust
    #[test]
    fn names_sort_case_insensitively_and_flags_are_kept() {
        let dir = scratch("listing-case");
        std::fs::create_dir(dir.join("b")).unwrap();
        std::fs::create_dir(dir.join("A")).unwrap();
        std::fs::create_dir(dir.join(".cfg")).unwrap();
        std::fs::write(dir.join("Z.txt"), b"").unwrap();
        std::fs::write(dir.join("a.txt"), b"").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.join("b"), dir.join("link")).unwrap();
        let (_, entries, truncated) = list_dir(dir.to_str().unwrap()).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        let got: Vec<(&str, bool)> = entries.iter().map(|e| (e.name.as_str(), e.dir)).collect();
        #[cfg(unix)]
        assert_eq!(
            got,
            [
                (".cfg", true),
                ("A", true),
                ("b", true),
                ("link", true),
                ("a.txt", false),
                ("Z.txt", false)
            ]
        );
        assert!(!truncated);
        assert!(entries
            .iter()
            .find(|e| e.name == ".cfg")
            .is_some_and(|e| e.hidden));
        #[cfg(unix)]
        assert!(entries
            .iter()
            .find(|e| e.name == "link")
            .is_some_and(|e| e.symlink));
    }
```

The tests of `proto/listing.rs` are in its full text in Step 2 (`a_reply_of_long_names_is_cut_to_fit_a_frame`: 5000 folders with 200-byte names; `escapes_count_and_files_go_first`; `a_small_listing_is_kept_whole`).

Run: `TMPDIR=$HOME/.cache/cl-tmp cargo test -p claudio-core --lib -- daemon::host`
Expected: the build fails: the tests call `list_dir_with`, which does not exist yet.

Step 2: implement

New file `crates/core/src/proto/listing.rs` (`DirEntry` moves here from `proto.rs` unchanged
except its docs: `claude_at` is always `None` today):

```rust
//! The wizard's directory listing (`ListDir` → `DirEntries`): one entry, and
//! the reply cut to fit a frame. Re-exported from [`crate::proto`].

use serde::{Deserialize, Serialize};

use super::{Msg, MAX_FRAME};

/// Room left in a frame for the envelope around a `DirEntries` reply.
const ENVELOPE_SLACK: usize = 256;

/// One entry of a [`Msg::DirEntries`] listing (the wizard's directory
/// browser), with the daemon's git enrichment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DirEntry {
    pub name: String,
    pub dir: bool,
    /// The git branch name when the directory is a git repo, `Some("")` for a
    /// detached/unknown HEAD, `None` when not a git repo. Filled for folders
    /// only; older daemons leave this as `None`.
    #[serde(default)]
    pub git: Option<String>,
    /// Always `None` today: no daemon fills it. Kept so the reply's shape
    /// stays the same for older clients.
    #[serde(default)]
    pub claude_at: Option<u64>,
    /// Whether the directory entry is a symlink.
    #[serde(default)]
    pub symlink: bool,
    /// Whether the entry name starts with `.`.
    #[serde(default)]
    pub hidden: bool,
}

impl DirEntry {
    /// Construct a minimal entry with no enrichment. Used in tests and by
    /// callers that do their own field assignment.
    pub fn simple(name: impl Into<String>, dir: bool) -> Self {
        DirEntry {
            name: name.into(),
            dir,
            git: None,
            claude_at: None,
            symlink: false,
            hidden: false,
        }
    }
}

/// The `DirEntries` reply for `path`, without as many of the last entries
/// (files first: `entries` is sorted folders first) as it takes for its JSON
/// to fit [`MAX_FRAME`] less the envelope's room (then `truncated`). Each
/// entry is measured as it encodes, escapes included.
pub fn dir_entries_reply(path: String, mut entries: Vec<DirEntry>, truncated: bool) -> Msg {
    let budget = MAX_FRAME - ENVELOPE_SLACK;
    let mut used = json_len(&Msg::DirEntries {
        path: path.clone(),
        entries: Vec::new(),
        truncated: true,
    });
    // One comma per entry after the first; counting one for each is safe.
    let kept = entries
        .iter()
        .take_while(|e| {
            used += json_len(e) + 1;
            used <= budget
        })
        .count();
    let cut = kept < entries.len();
    entries.truncate(kept);
    Msg::DirEntries {
        path,
        entries,
        truncated: truncated || cut,
    }
}

fn json_len<T: Serialize>(value: &T) -> usize {
    serde_json::to_vec(value).map_or(usize::MAX / 2, |json| json.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{Envelope, Frame};

    fn frame_len(msg: Msg) -> usize {
        Frame::Control(Envelope::request(u64::MAX, msg))
            .encode()
            .len()
            - 4
    }

    #[test]
    fn a_reply_of_long_names_is_cut_to_fit_a_frame() {
        let entries: Vec<DirEntry> = (0..5000)
            .map(|i| DirEntry::simple(format!("{i:05}{}", "n".repeat(195)), true))
            .collect();
        let msg = dir_entries_reply("/srv/big".into(), entries, false);
        let Msg::DirEntries {
            entries, truncated, ..
        } = &msg
        else {
            panic!("expected DirEntries");
        };
        assert!(*truncated);
        assert!(
            entries.len() > 3000 && entries.len() < 5000,
            "{}",
            entries.len()
        );
        assert_eq!(entries[0].name.len(), 200);
        assert!(frame_len(msg) <= MAX_FRAME);
    }

    #[test]
    fn escapes_count_and_files_go_first() {
        // A control character encodes as six bytes (`\u0001`): 40 of them
        // per name make a reply the old `name + 24` estimate passed.
        let mut entries: Vec<DirEntry> = (0..20)
            .map(|i| DirEntry::simple(format!("dir{i}"), true))
            .collect();
        entries.extend(
            (0..5000).map(|i| DirEntry::simple(format!("{i}{}", "\u{1}".repeat(40)), false)),
        );
        let msg = dir_entries_reply("/x".into(), entries, false);
        let Msg::DirEntries {
            entries, truncated, ..
        } = &msg
        else {
            panic!("expected DirEntries");
        };
        assert!(*truncated);
        assert!(entries[..20].iter().all(|e| e.dir), "every folder is kept");
        assert!(frame_len(msg) <= MAX_FRAME);
    }

    #[test]
    fn a_small_listing_is_kept_whole() {
        let entries = vec![DirEntry::simple("a", true), DirEntry::simple("b", false)];
        let msg = dir_entries_reply("/x".into(), entries.clone(), false);
        assert_eq!(
            msg,
            Msg::DirEntries {
                path: "/x".into(),
                entries,
                truncated: false
            }
        );
        let Msg::DirEntries { truncated, .. } = dir_entries_reply("/x".into(), vec![], true) else {
            panic!("expected DirEntries");
        };
        assert!(truncated, "the daemon's own cut is kept");
    }
}
```

`crates/core/src/proto.rs` (drop `DirEntry`, add the module):

```diff
diff --git a/crates/core/src/proto.rs b/crates/core/src/proto.rs
index 890fdec..6b471d5 100644
--- a/crates/core/src/proto.rs
+++ b/crates/core/src/proto.rs
@@ -21,11 +21,13 @@ use uuid::Uuid;
 
 mod git;
 mod handshake;
+mod listing;
 mod spawn;
 
 pub use crate::version::PROTO;
 pub use git::*;
 pub use handshake::*;
+pub use listing::*;
 pub use spawn::*;
 
 /// Hard cap on a single frame (tag + payload).
@@ -652,44 +654,6 @@ impl GitStatus {
     }
 }
 
-/// One entry of a [`Msg::DirEntries`] listing (the wizard's directory
-/// browser), with the daemon's git and claude-activity enrichment.
-#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
-pub struct DirEntry {
-    pub name: String,
-    pub dir: bool,
-    /// The git branch name when the directory is a git repo, `Some("")` for a
-    /// detached/unknown HEAD, `None` when not a git repo. Filled by the daemon;
-    /// older daemons leave this as `None`.
-    #[serde(default)]
-    pub git: Option<String>,
-    /// Unix seconds of the last claude session activity in this directory.
-    /// `None` when no claude session is known. Filled by the daemon.
-    #[serde(default)]
-    pub claude_at: Option<u64>,
-    /// Whether the directory entry is a symlink.
-    #[serde(default)]
-    pub symlink: bool,
-    /// Whether the entry name starts with `.`.
-    #[serde(default)]
-    pub hidden: bool,
-}
-
-impl DirEntry {
-    /// Construct a minimal entry with no enrichment. Used in tests and by
-    /// callers that do their own field assignment.
-    pub fn simple(name: impl Into<String>, dir: bool) -> Self {
-        DirEntry {
-            name: name.into(),
-            dir,
-            git: None,
-            claude_at: None,
-            symlink: false,
-            hidden: false,
-        }
-    }
-}
-
 /// A directory with claude history.
 #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
 pub struct ProjectDir {
```

`crates/core/src/daemon/host.rs`:

```diff
diff --git a/crates/core/src/daemon/host.rs b/crates/core/src/daemon/host.rs
index c1020b4..163d8dc 100644
--- a/crates/core/src/daemon/host.rs
+++ b/crates/core/src/daemon/host.rs
@@ -19,6 +19,9 @@ const VERSION_TIMEOUT: Duration = Duration::from_secs(3);
 /// Maximum entries returned by [`list_dir`].
 const MAX_DIR_ENTRIES: usize = 5000;
 
+/// Maximum entries [`list_dir`] reads before it sorts and cuts.
+const MAX_SCAN: usize = 50_000;
+
 /// The current user's home directory.
 pub fn home() -> PathBuf {
     std::env::var_os("HOME")
@@ -218,42 +221,160 @@ pub fn detect_git_branch(path: &Path) -> Option<String> {
     }
 }
 
-/// List `path` (with `~` expanded): directories first, then by name, capped
-/// at 5000 entries. Returns the expanded path alongside the entries.
-pub fn list_dir(path: &str) -> io::Result<(String, Vec<DirEntry>)> {
+/// List `path` (with `~` expanded): folders first, then files, each by name
+/// case-insensitively, at most [`MAX_DIR_ENTRIES`]. Returns the expanded
+/// path, the entries, and whether any were left out.
+pub fn list_dir(path: &str) -> io::Result<(String, Vec<DirEntry>, bool)> {
+    list_dir_with(path, detect_git_branch)
+}
+
+/// [`list_dir`] with `git` as the branch probe (a test counts its calls).
+fn list_dir_with(
+    path: &str,
+    git: impl Fn(&Path) -> Option<String>,
+) -> io::Result<(String, Vec<DirEntry>, bool)> {
     let dir = expand_tilde(path);
-    let mut entries: Vec<DirEntry> = std::fs::read_dir(&dir)?
-        .filter_map(Result::ok)
-        .take(MAX_DIR_ENTRIES)
-        .map(|e| {
-            let name = e.file_name().to_string_lossy().into_owned();
-            let path = e.path();
-            let hidden = name.starts_with('.');
-            // Detect symlinks without following them (symlink_metadata vs metadata).
-            let symlink = e
-                .metadata()
-                .map(|m| m.file_type().is_symlink())
-                .unwrap_or(false);
-            // Follow symlinks so a link to a directory can be descended into.
-            let dir_flag = path.is_dir();
-            DirEntry {
-                name,
-                dir: dir_flag,
-                git: detect_git_branch(&path),
-                claude_at: None, // enriched below for dirs only
-                symlink,
-                hidden,
-            }
+    let (mut scanned, mut truncated) = scan(&dir)?;
+    scanned.sort_by(|a, b| {
+        b.dir
+            .cmp(&a.dir)
+            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
+            .then_with(|| a.name.cmp(&b.name))
+    });
+    truncated |= scanned.len() > MAX_DIR_ENTRIES;
+    scanned.truncate(MAX_DIR_ENTRIES);
+    // Only the kept entries are enriched, and only folders get a git probe.
+    let entries = scanned
+        .into_iter()
+        .map(|s| DirEntry {
+            git: s.dir.then(|| git(&dir.join(&s.name))).flatten(),
+            hidden: s.name.starts_with('.'),
+            symlink: s.symlink,
+            dir: s.dir,
+            claude_at: None,
+            name: s.name,
         })
         .collect();
-    entries.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.cmp(&b.name)));
-    Ok((dir.to_string_lossy().into_owned(), entries))
+    Ok((dir.to_string_lossy().into_owned(), entries, truncated))
+}
+
+/// One entry as read: its name and kind, nothing more.
+struct Scanned {
+    name: String,
+    dir: bool,
+    symlink: bool,
+}
+
+/// Read at most [`MAX_SCAN`] entries of `dir`, the kind from the directory
+/// entry itself (no extra syscall on Linux and macOS); a symlink costs one
+/// `metadata` call to learn whether it leads to a folder. `true` when the
+/// folder held more.
+fn scan(dir: &Path) -> io::Result<(Vec<Scanned>, bool)> {
+    let mut out = Vec::new();
+    for entry in std::fs::read_dir(dir)?.filter_map(Result::ok) {
+        if out.len() == MAX_SCAN {
+            return Ok((out, true));
+        }
+        let Ok(kind) = entry.file_type() else {
+            continue;
+        };
+        let symlink = kind.is_symlink();
+        let dir = match symlink {
+            true => entry.path().metadata().is_ok_and(|m| m.is_dir()),
+            false => kind.is_dir(),
+        };
+        let name = entry.file_name().to_string_lossy().into_owned();
+        out.push(Scanned { name, dir, symlink });
+    }
+    Ok((out, false))
 }
 
 #[cfg(test)]
 mod tests {
     use super::*;
 
+    /// A fresh folder under the temp dir.
+    fn scratch(tag: &str) -> PathBuf {
+        let dir = std::env::temp_dir().join(format!("claudio-{tag}-{}", uuid::Uuid::new_v4()));
+        std::fs::create_dir_all(&dir).unwrap();
+        dir
+    }
+
+    #[test]
+    fn folders_come_first_before_the_cut_and_only_they_are_probed() {
+        let dir = scratch("listing");
+        for i in 0..6000 {
+            std::fs::write(dir.join(format!("file{i:04}")), b"").unwrap();
+        }
+        for name in [
+            "delta", "Alpha", "charlie", "Bravo", "echo", "Foxtrot", "golf", "Hotel", "india",
+            "Juliet",
+        ] {
+            std::fs::create_dir(dir.join(name)).unwrap();
+        }
+        let probes = std::cell::Cell::new(0);
+        let (_, entries, truncated) = list_dir_with(dir.to_str().unwrap(), |_| {
+            probes.set(probes.get() + 1);
+            None
+        })
+        .unwrap();
+        std::fs::remove_dir_all(&dir).unwrap();
+        let first: Vec<&str> = entries[..10].iter().map(|e| e.name.as_str()).collect();
+        assert_eq!(
+            first,
+            [
+                "Alpha", "Bravo", "charlie", "delta", "echo", "Foxtrot", "golf", "Hotel", "india",
+                "Juliet"
+            ]
+        );
+        assert!(entries[..10].iter().all(|e| e.dir));
+        assert!(entries[10..].iter().all(|e| !e.dir && e.git.is_none()));
+        assert_eq!(entries.len(), MAX_DIR_ENTRIES);
+        assert!(truncated);
+        assert_eq!(
+            probes.get(),
+            10,
+            "the git probe runs on the kept folders only"
+        );
+    }
+
+    #[test]
+    fn names_sort_case_insensitively_and_flags_are_kept() {
+        let dir = scratch("listing-case");
+        std::fs::create_dir(dir.join("b")).unwrap();
+        std::fs::create_dir(dir.join("A")).unwrap();
+        std::fs::create_dir(dir.join(".cfg")).unwrap();
+        std::fs::write(dir.join("Z.txt"), b"").unwrap();
+        std::fs::write(dir.join("a.txt"), b"").unwrap();
+        #[cfg(unix)]
+        std::os::unix::fs::symlink(dir.join("b"), dir.join("link")).unwrap();
+        let (_, entries, truncated) = list_dir(dir.to_str().unwrap()).unwrap();
+        std::fs::remove_dir_all(&dir).unwrap();
+        let got: Vec<(&str, bool)> = entries.iter().map(|e| (e.name.as_str(), e.dir)).collect();
+        #[cfg(unix)]
+        assert_eq!(
+            got,
+            [
+                (".cfg", true),
+                ("A", true),
+                ("b", true),
+                ("link", true),
+                ("a.txt", false),
+                ("Z.txt", false)
+            ]
+        );
+        assert!(!truncated);
+        assert!(entries
+            .iter()
+            .find(|e| e.name == ".cfg")
+            .is_some_and(|e| e.hidden));
+        #[cfg(unix)]
+        assert!(entries
+            .iter()
+            .find(|e| e.name == "link")
+            .is_some_and(|e| e.symlink));
+    }
+
     #[test]
     fn tilde_expansion() {
         let h = home();
```

`crates/core/src/daemon/server.rs` (the byte-budget cut and `dir_entry_encoded_size` go):

```diff
diff --git a/crates/core/src/daemon/server.rs b/crates/core/src/daemon/server.rs
index 940bfae..4fa8d6e 100644
--- a/crates/core/src/daemon/server.rs
+++ b/crates/core/src/daemon/server.rs
@@ -32,8 +32,7 @@ use crate::claude::projects;
 use crate::claude::search::{self, BadQuery, Budget};
 use crate::paths;
 use crate::proto::{
-    self, Envelope, Frame, Msg, ProjectDir, SessionId, SessionKind, SpawnSpec, Welcome, MAX_FRAME,
-    PROTO,
+    self, Envelope, Frame, Msg, ProjectDir, SessionId, SessionKind, SpawnSpec, Welcome, PROTO,
 };
 
 /// Deadline for a connection's first frame.
@@ -709,40 +708,13 @@ fn not_running(id: SessionId) -> String {
     format!("session {id} is not running")
 }
 
-/// Approximate encoded size of a `DirEntry` (name + bool + JSON overhead).
-fn dir_entry_encoded_size(name: &str) -> usize {
-    // JSON: {"name":"<name>","dir":false} ≈ 20 + name.len() bytes.
-    name.len() + 24
-}
-
 async fn list_dir(path: String) -> Msg {
     let listed = tokio::task::spawn_blocking(move || {
         host::list_dir(&path).map_err(|e| format!("cannot list {path}: {e}"))
     })
     .await;
     match listed {
-        Ok(Ok((path, entries))) => {
-            // Cap entries so the reply fits within MAX_FRAME.
-            // Reserve ~256 bytes for the JSON envelope.
-            let budget = MAX_FRAME.saturating_sub(256);
-            let mut used = 0usize;
-            let mut truncated = false;
-            let mut capped: Vec<_> = Vec::new();
-            for entry in entries {
-                let sz = dir_entry_encoded_size(&entry.name);
-                if used + sz > budget {
-                    truncated = true;
-                    break;
-                }
-                used += sz;
-                capped.push(entry);
-            }
-            Msg::DirEntries {
-                path,
-                entries: capped,
-                truncated,
-            }
-        }
+        Ok(Ok((path, entries, truncated))) => proto::dir_entries_reply(path, entries, truncated),
         Ok(Err(message)) => Msg::Error { message },
         Err(e) => Msg::Error {
             message: format!("list_dir failed: {e}"),
```

Run: `cargo fmt && TMPDIR=$HOME/.cache/cl-tmp cargo test -p claudio-core --lib -- proto::listing daemon::host daemon::tests::list_dir`
Expected:
```
test daemon::host::tests::folders_come_first_before_the_cut_and_only_they_are_probed ... ok
test daemon::host::tests::names_sort_case_insensitively_and_flags_are_kept ... ok
test proto::listing::tests::a_reply_of_long_names_is_cut_to_fit_a_frame ... ok
test proto::listing::tests::escapes_count_and_files_go_first ... ok
test proto::listing::tests::a_small_listing_is_kept_whole ... ok
test daemon::tests::list_dir_sorts_dirs_first ... ok
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 608 filtered out
```
Run: `python3 scripts/check_rules.py`
Expected: `rules: 0 problem(s); …` (`server.rs` 872, `proto.rs` 842).

Step 3: commit
`git add crates/core/src/proto/listing.rs && git commit -m "fix(daemon): list folders first and cut a listing by its encoded size" -- crates/core/src/daemon/host.rs crates/core/src/daemon/server.rs crates/core/src/proto.rs crates/core/src/proto/listing.rs`

Interfaces this task provides to later tasks:
- `claudio_core::daemon::host::list_dir(path: &str) -> io::Result<(String, Vec<DirEntry>, bool)>` (daemon-internal).
- `claudio_core::proto::dir_entries_reply(path: String, entries: Vec<DirEntry>, truncated: bool) -> Msg`.
- `proto::DirEntry` unchanged in shape and path. A listing is folders first, case-insensitive; `truncated` is set when the folder held more than 5000 entries (or 50,000 were scanned) or the reply was cut.

### Task 2: core: the multi-host conversation search, and the web server uses it
Model: sonnet   Repo: /home/p4u/claudio   Files: crates/core/src/app/search.rs (new), crates/core/src/app/mod.rs, addons/web/src/ws.rs
Goal: spec "The multi-host search lives once, in claudio-core" and AC11's core part. Rules: A5 (one implementation), A2/A3 (the web calls `claudio_core::app::search`, the core names no addon), behaviour of the web's `search` op unchanged (docs/web-protocol.md §search).

Step 1: write the failing tests

The test module of the new `crates/core/src/app/search.rs` (its full text is in Step 2):

```rust
    #[tokio::test(start_paused = true)]
    async fn a_busy_host_is_asked_once_more_after_500_ms() {
        let calls = AtomicU32::new(0);
        let start = tokio::time::Instant::now();
        let got = ask_host(|| {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            async move {
                match n {
                    0 => Err(io::Error::other("search busy")),
                    _ => reply(vec![hit("c1", 5)]),
                }
            }
        })
        .await;
        assert_eq!(got, Ok((vec![hit("c1", 5)], false)));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(start.elapsed(), BUSY_RETRY);
    }
```

```rust
    #[tokio::test(start_paused = true)]
    async fn still_busy_after_the_retry_is_a_failure() {
        let calls = AtomicU32::new(0);
        let got = ask_host(|| {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Err(io::Error::other("search busy")) }
        })
        .await;
        assert_eq!(got, Err("search busy".to_owned()));
        assert_eq!(calls.load(Ordering::SeqCst), 2, "one retry only");
    }
```

```rust
    #[tokio::test(start_paused = true)]
    async fn hosts_merge_newest_first_and_failures_are_named() {
        let hosts = vec!["local".into(), "down".into(), "slow".into(), "z6".into()];
        let found = search_all(hosts, |host| async move {
            match host.as_str() {
                "local" => Ok((vec![hit("old", 10), hit("new", 30)], false)),
                "z6" => Ok((vec![hit("mid", 20)], true)),
                "down" => Err("connection closed".to_owned()),
                _ => std::future::pending().await,
            }
        })
        .await;
        let order: Vec<(&str, &str)> = found
            .hits
            .iter()
            .map(|(h, x)| (h.as_str(), x.conversation.as_str()))
            .collect();
        assert_eq!(order, [("local", "new"), ("z6", "mid"), ("local", "old")]);
        assert_eq!(
            found.failed,
            [
                ("down".to_owned(), "connection closed".to_owned()),
                ("slow".to_owned(), "timed out".to_owned())
            ]
        );
        assert!(found.truncated);
    }
```

```rust
    #[tokio::test]
    async fn no_host_finds_nothing() {
        let found = search_clients(Vec::new(), "q".into()).await;
        assert_eq!(found, Found::default());
    }
```

Run: `TMPDIR=$HOME/.cache/cl-tmp cargo test -p claudio-core --lib app::search`
Expected (with only the test module and `pub mod search;` written): the build fails: `ask_host`, `search_all`, `search_clients` do not exist yet.

Step 2: implement

New file `crates/core/src/app/search.rs`:

```rust
//! The conversation search over every connected host, for both front-ends
//! (A5): the web's `search` op and the TUI wizard's "Search conversations"
//! row. One `Search` per host, each within [`DEADLINE`]; a host that answers
//! `search busy` (its daemon runs one search at a time) is asked once more
//! after [`BUSY_RETRY`]. The hits come back merged newest first with their
//! host, and every host that failed is named with its error.

use std::future::Future;
use std::io;
use std::time::Duration;

use crate::client::Client;
use crate::proto::{Msg, SearchHit, Welcome};

/// A host's deadline, counting whatever its search waits for first.
pub const DEADLINE: Duration = Duration::from_secs(10);
/// The pause before asking a busy host again.
pub const BUSY_RETRY: Duration = Duration::from_millis(500);

/// What a search over several hosts found.
#[derive(Debug, Default, PartialEq)]
pub struct Found {
    /// `(host, hit)`, newest first.
    pub hits: Vec<(String, SearchHit)>,
    /// `(host, error)` for every host that did not answer.
    pub failed: Vec<(String, String)>,
    /// Whether a host left hits out (its reply was cut).
    pub truncated: bool,
}

/// One host's hits and whether its reply was cut.
pub type HostHits = Result<(Vec<SearchHit>, bool), String>;

/// Whether the daemon behind `welcome` answers `Search`.
pub fn can_search(welcome: &Welcome) -> bool {
    welcome.features.iter().any(|f| f == "search")
}

/// Ask one host: `ask` sends the `Search`; after `search busy` it is sent
/// once more, [`BUSY_RETRY`] later.
pub async fn ask_host<F, Fut>(ask: F) -> HostHits
where
    F: Fn() -> Fut,
    Fut: Future<Output = io::Result<Msg>>,
{
    let mut reply = ask().await;
    if matches!(&reply, Err(e) if e.to_string() == "search busy") {
        tokio::time::sleep(BUSY_RETRY).await;
        reply = ask().await;
    }
    match reply.map_err(|e| e.to_string())? {
        Msg::SearchReply {
            hits, truncated, ..
        } => Ok((hits, truncated)),
        _ => Err("unexpected reply from the daemon".to_owned()),
    }
}

/// Search every host of `hosts` at once with `one`, each within
/// [`DEADLINE`], and merge what they found.
pub async fn search_all<F, Fut>(hosts: Vec<String>, one: F) -> Found
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = HostHits>,
{
    let asks = hosts.into_iter().map(|host| {
        let ask = one(host.clone());
        async move {
            let res = tokio::time::timeout(DEADLINE, ask)
                .await
                .unwrap_or_else(|_| Err("timed out".to_owned()));
            (host, res)
        }
    });
    let mut found = Found::default();
    for (host, res) in futures::future::join_all(asks).await {
        match res {
            Ok((hits, cut)) => {
                found.truncated |= cut;
                found
                    .hits
                    .extend(hits.into_iter().map(|h| (host.clone(), h)));
            }
            Err(error) => found.failed.push((host, error)),
        }
    }
    found.hits.sort_by_key(|h| std::cmp::Reverse(h.1.modified));
    found
}

/// Search the conversations of `clients` (`(host, client)`, each one
/// [`can_search`] accepts) for `query`, every hit a host has (`limit: 0`).
pub async fn search_clients(clients: Vec<(String, Client)>, query: String) -> Found {
    let hosts = clients.iter().map(|(h, _)| h.clone()).collect();
    search_all(hosts, |host| {
        let client = clients
            .iter()
            .find(|(h, _)| *h == host)
            .map(|(_, c)| c.clone());
        let query = query.clone();
        async move {
            let client = client.ok_or_else(|| "not connected".to_owned())?;
            ask_host(|| {
                client.request(Msg::Search {
                    query: query.clone(),
                    limit: 0,
                })
            })
            .await
        }
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn hit(conversation: &str, modified: u64) -> SearchHit {
        SearchHit {
            conversation: conversation.into(),
            cwd: "/w".into(),
            modified,
            title: "t".into(),
            snippet: "s".into(),
            matches: 1,
        }
    }

    fn reply(hits: Vec<SearchHit>) -> io::Result<Msg> {
        Ok(Msg::SearchReply {
            query: "q".into(),
            hits,
            scanned: 1,
            total: 1,
            truncated: false,
        })
    }

    #[tokio::test(start_paused = true)]
    async fn a_busy_host_is_asked_once_more_after_500_ms() {
        let calls = AtomicU32::new(0);
        let start = tokio::time::Instant::now();
        let got = ask_host(|| {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            async move {
                match n {
                    0 => Err(io::Error::other("search busy")),
                    _ => reply(vec![hit("c1", 5)]),
                }
            }
        })
        .await;
        assert_eq!(got, Ok((vec![hit("c1", 5)], false)));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(start.elapsed(), BUSY_RETRY);
    }

    #[tokio::test(start_paused = true)]
    async fn still_busy_after_the_retry_is_a_failure() {
        let calls = AtomicU32::new(0);
        let got = ask_host(|| {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Err(io::Error::other("search busy")) }
        })
        .await;
        assert_eq!(got, Err("search busy".to_owned()));
        assert_eq!(calls.load(Ordering::SeqCst), 2, "one retry only");
    }

    #[tokio::test(start_paused = true)]
    async fn hosts_merge_newest_first_and_failures_are_named() {
        let hosts = vec!["local".into(), "down".into(), "slow".into(), "z6".into()];
        let found = search_all(hosts, |host| async move {
            match host.as_str() {
                "local" => Ok((vec![hit("old", 10), hit("new", 30)], false)),
                "z6" => Ok((vec![hit("mid", 20)], true)),
                "down" => Err("connection closed".to_owned()),
                _ => std::future::pending().await,
            }
        })
        .await;
        let order: Vec<(&str, &str)> = found
            .hits
            .iter()
            .map(|(h, x)| (h.as_str(), x.conversation.as_str()))
            .collect();
        assert_eq!(order, [("local", "new"), ("z6", "mid"), ("local", "old")]);
        assert_eq!(
            found.failed,
            [
                ("down".to_owned(), "connection closed".to_owned()),
                ("slow".to_owned(), "timed out".to_owned())
            ]
        );
        assert!(found.truncated);
    }

    #[tokio::test]
    async fn no_host_finds_nothing() {
        let found = search_clients(Vec::new(), "q".into()).await;
        assert_eq!(found, Found::default());
    }
}
```

`crates/core/src/app/mod.rs`:

```diff
diff --git a/crates/core/src/app/mod.rs b/crates/core/src/app/mod.rs
index 1e6e80b..9a0402a 100644
--- a/crates/core/src/app/mod.rs
+++ b/crates/core/src/app/mod.rs
@@ -11,6 +11,7 @@ pub mod env;
 pub mod extensions;
 pub mod label;
 pub mod launch;
+pub mod search;
 pub mod snippets;
 pub mod state;
```

`addons/web/src/ws.rs` (the per-host queue stays here; the deadline, retry and merge come from the core):

```diff
diff --git a/addons/web/src/ws.rs b/addons/web/src/ws.rs
index bed5b96..0bb30a1 100644
--- a/addons/web/src/ws.rs
+++ b/addons/web/src/ws.rs
@@ -24,8 +24,9 @@ use super::spawn::{self, check_size, RespawnReq, SpawnReq};
 use super::state::Layout;
 use super::AppState;
 use super::{addons, auth, ext, git_ops, hosts, sessions};
+use claudio_core::app::search;
 use claudio_core::paths;
-use claudio_core::proto::{Msg, SearchHit, SessionId};
+use claudio_core::proto::{Msg, SessionId};
 
 /// The largest client text frame: a 64 KiB text and its JSON envelope.
 pub(crate) const MAX_TEXT: usize = 128 * 1024;
@@ -42,8 +43,6 @@ const CLOSE_BAD_FRAME: u16 = 1003;
 
 /// Requests one tab may have in flight.
 const IN_FLIGHT: usize = 64;
-/// A search's deadline, counting the wait for the host's slot.
-const SEARCH_DEADLINE: Duration = Duration::from_secs(10);
 
 /// One of the [`MAX_TABS`] tab slots, held from the upgrade to the end of
 /// the tab.
@@ -643,8 +642,9 @@ pub(crate) async fn dispatch(
 }
 
 /// `search` (docs/web-protocol.md §5): every connected host with the
-/// `search` feature, one request per host at a time, 10 s counting the
-/// wait, one more try 500 ms after `search busy`; hits newest first.
+/// `search` feature, through the core's search (`claudio_core::app::search`:
+/// 10 s a host counting the wait for its slot, one more try 500 ms after
+/// `search busy`, hits newest first).
 async fn search(state: &AppState, query: String) -> Value {
     let hosts: Vec<String> = state
         .hub
@@ -659,29 +659,14 @@ async fn search(state: &AppState, query: String) -> Value {
         })
         .filter_map(|h| h["name"].as_str().map(str::to_owned))
         .collect();
-    let asks = hosts.into_iter().map(|host| {
-        let (state, query) = (state.clone(), query.clone());
-        async move {
-            let res = tokio::time::timeout(SEARCH_DEADLINE, search_host(&state, &host, query))
-                .await
-                .unwrap_or_else(|_| Err("timed out".to_owned()));
-            (host, res)
-        }
-    });
-    let mut hits: Vec<(String, SearchHit)> = Vec::new();
-    let mut failed = Vec::new();
-    let mut truncated = false;
-    for (host, res) in futures::future::join_all(asks).await {
-        match res {
-            Ok((found, cut)) => {
-                truncated |= cut;
-                hits.extend(found.into_iter().map(|h| (host.clone(), h)));
-            }
-            Err(error) => failed.push(json!({"host": host, "error": error})),
-        }
-    }
-    hits.sort_by_key(|h| std::cmp::Reverse(h.1.modified));
-    let hits: Vec<Value> = hits
+    let found = search::search_all(hosts, |host| search_host(state, host, query.clone())).await;
+    let failed: Vec<Value> = found
+        .failed
+        .into_iter()
+        .map(|(host, error)| json!({"host": host, "error": daemon_error(error)}))
+        .collect();
+    let hits: Vec<Value> = found
+        .hits
         .into_iter()
         .map(|(host, h)| {
             json!({
@@ -695,19 +680,16 @@ async fn search(state: &AppState, query: String) -> Value {
             })
         })
         .collect();
-    json!({"hits": hits, "failed": failed, "truncated": truncated})
+    json!({"hits": hits, "failed": failed, "truncated": found.truncated})
 }
 
-async fn search_host(
-    state: &AppState,
-    host: &str,
-    query: String,
-) -> Result<(Vec<SearchHit>, bool), String> {
+/// One host's search, one at a time per host (the daemon runs one).
+async fn search_host(state: &AppState, host: String, query: String) -> search::HostHits {
     let slot = {
         let mut slots = state.search_slots.lock().unwrap_or_else(|p| p.into_inner());
         Arc::clone(
             slots
-                .entry(host.to_owned())
+                .entry(host.clone())
                 .or_insert_with(|| Arc::new(Semaphore::new(1))),
         )
     };
@@ -715,24 +697,14 @@ async fn search_host(
         .acquire_owned()
         .await
         .map_err(|_| "closed".to_owned())?;
-    let (client, _) = state.hub.host_client(host).await?;
+    let (client, _) = state.hub.host_client(&host).await?;
     let ask = || {
         client.request(Msg::Search {
             query: query.clone(),
             limit: 0,
         })
     };
-    let mut reply = ask().await;
-    if matches!(&reply, Err(e) if e.to_string() == "search busy") {
-        tokio::time::sleep(Duration::from_millis(500)).await;
-        reply = ask().await;
-    }
-    match reply.map_err(daemon_error)? {
-        Msg::SearchReply {
-            hits, truncated, ..
-        } => Ok((hits, truncated)),
-        _ => Err(UNEXPECTED.to_owned()),
-    }
+    search::ask_host(ask).await
 }
 
 #[cfg(test)]
```

Run: `cargo fmt && TMPDIR=$HOME/.cache/cl-tmp cargo test -p claudio-core --lib app::search`
Expected:
```
test app::search::tests::a_busy_host_is_asked_once_more_after_500_ms ... ok
test app::search::tests::still_busy_after_the_retry_is_a_failure ... ok
test app::search::tests::hosts_merge_newest_first_and_failures_are_named ... ok
test app::search::tests::no_host_finds_nothing ... ok
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 615 filtered out
```
Run: `TMPDIR=$HOME/.cache/cl-tmp cargo test --test web web_reader_and_search -- --test-threads=1`
Expected: `test web_reader_and_search_read_a_fixture_transcript ... ok` and `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 65 filtered out`

Step 3: commit
`git add crates/core/src/app/search.rs && git commit -m "refactor(core): one multi-host conversation search for both front-ends" -- crates/core/src/app/search.rs crates/core/src/app/mod.rs addons/web/src/ws.rs`

Interfaces this task provides to later tasks:
- `claudio_core::app::search::{DEADLINE, BUSY_RETRY, Found, HostHits, can_search, ask_host, search_all, search_clients}`.
- `pub async fn search_clients(clients: Vec<(String, Client)>, query: String) -> Found` — `Found { hits: Vec<(String, SearchHit)> /* newest first */, failed: Vec<(String, String)>, truncated: bool }`.
- `pub fn can_search(welcome: &Welcome) -> bool`.

### Task 3: TUI: move the wizard's host step to its own module (move only)
Model: haiku   Repo: /home/p4u/claudio   Files: crates/tui/src/wizard.rs, crates/tui/src/wizard_host.rs (new), crates/tui/src/lib.rs, scripts/size-baseline.txt
Goal: spec "Where the code lives (A8)": make room in `wizard.rs` before Tasks 4-5 add to it. Nothing changes behaviour. Rule: A8 ("a commit that only moves files may rename entries (same counts) and nothing else": the `file crates/tui/src/wizard.rs 1138` line stays as it is).

Step 1: write the failing test

None: a pure move. The existing wizard tests (they cover the host step) are the check.
Run: `TMPDIR=$HOME/.cache/cl-tmp cargo test -p claudio-tui --lib`
Expected (before): `test result: ok. 406 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`

Step 2: implement

Save this script as `~/.cache/claudio/move_host_step.py` and run it from the repo root with
`python3 -I ~/.cache/claudio/move_host_step.py && cargo fmt`. It moves `HostSection`,
`HostStep` (with its `impl`) and `looks_like_host` byte for byte, re-exports
`HostSection`/`HostStep` from `wizard.rs` (so `wizard::HostStep` keeps working for `app.rs` and
`ui/wizard_host.rs`), makes the three items `wizard.rs` and its tests reach into `pub(crate)`,
and renames the baseline entries:

```python
# Task 3: move the wizard's host step (HostSection, HostStep, looks_like_host)
# from crates/tui/src/wizard.rs to crates/tui/src/wizard_host.rs, unchanged.
# Run from the repo root, then `cargo fmt`.
p = "crates/tui/src/wizard.rs"
s = open(p).read()
start = s.index("// ── Step 0: host / first screen")
end = s.index("/// Step 2: the resume picker.")
moved = s[start:end]
s = s[:start] + s[end:]
s = s.replace(
    "use super::fmt::{abbreviate_home, fmt_age};\nuse super::wizard_route::Routes;",
    "use super::fmt::{abbreviate_home, fmt_age};\n"
    "pub use super::wizard_host::{HostSection, HostStep};\n"
    "use super::wizard_route::Routes;",
)
s = s.replace(
    "mod tests {\n    use super::*;\n    use claudio_core::addon::Pick;",
    "mod tests {\n    use super::*;\n    use crate::wizard_host::looks_like_host;\n"
    "    use claudio_core::addon::Pick;",
)
s = s.replace("super::looks_like_host(", "looks_like_host(")
open(p, "w").write(s)

head = (
    "//! The wizard's first screen (step 0): LOCAL (explore, recent dirs) and\n"
    "//! REMOTE (ssh hosts, a typed one), filtered by what is typed. Drawn by\n"
    "//! `ui/wizard_host.rs`; the wizard itself is `wizard.rs`.\n\n"
    "use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};\n\n"
    "use super::wizard::{fuzzy_score, Outcome};\n\n"
)
moved = moved.split("\n", 2)[2]  # drop the "Step 0" banner and its blank line
for old, new in [
    ("    local_dirs: Vec<String>,", "    pub(crate) local_dirs: Vec<String>,"),
    ("    candidates: Vec<String>,", "    pub(crate) candidates: Vec<String>,"),
    ("    fn refilter(&mut self) {", "    pub(crate) fn refilter(&mut self) {"),
]:
    assert moved.count(old) == 1, old
    moved = moved.replace(old, new)
open("crates/tui/src/wizard_host.rs", "w").write(head + moved.rstrip() + "\n")

p = "crates/tui/src/lib.rs"
s = open(p).read()
s = s.replace("mod wizard;\nmod wizard_route;", "mod wizard;\nmod wizard_host;\nmod wizard_route;")
open(p, "w").write(s)

# A8: a commit that only moves may rename baseline entries (same counts), nothing else.
p = "scripts/size-baseline.txt"
s = open(p).read()
old = (
    "fn crates/tui/src/wizard.rs::on_key 110\n"
    "fn crates/tui/src/wizard.rs::on_key#2 155\n"
    "fn crates/tui/src/wizard.rs::refilter 85\n"
    "fn crates/tui/src/wizard.rs::refilter#2 85\n"
)
new = (
    "fn crates/tui/src/wizard.rs::on_key 155\n"
    "fn crates/tui/src/wizard.rs::refilter 85\n"
    "fn crates/tui/src/wizard_host.rs::on_key 110\n"
    "fn crates/tui/src/wizard_host.rs::refilter 85\n"
)
assert old in s
open(p, "w").write(s.replace(old, new))
```

The result, `crates/tui/src/wizard_host.rs`, starts:

```rust
//! The wizard's first screen (step 0): LOCAL (explore, recent dirs) and
//! REMOTE (ssh hosts, a typed one), filtered by what is typed. Drawn by
//! `ui/wizard_host.rs`; the wizard itself is `wizard.rs`.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::wizard::{fuzzy_score, Outcome};

/// Which section of the first screen has keyboard focus.
#[derive(Debug, Clone, PartialEq)]
pub enum HostSection {
```

`scripts/size-baseline.txt` after it:

```diff
diff --git a/scripts/size-baseline.txt b/scripts/size-baseline.txt
index c0ae77a..68a1768 100644
--- a/scripts/size-baseline.txt
+++ b/scripts/size-baseline.txt
@@ -45,10 +45,10 @@ fn crates/tui/src/lib.rs::main 91
 fn crates/tui/src/lib.rs::run_effect 123
 fn crates/tui/src/ui/extensions.rs::draw_extensions 85
 fn crates/tui/src/ui/mod.rs::draw_wizard 188
-fn crates/tui/src/wizard.rs::on_key 110
-fn crates/tui/src/wizard.rs::on_key#2 155
+fn crates/tui/src/wizard.rs::on_key 155
 fn crates/tui/src/wizard.rs::refilter 85
-fn crates/tui/src/wizard.rs::refilter#2 85
+fn crates/tui/src/wizard_host.rs::on_key 110
+fn crates/tui/src/wizard_host.rs::refilter 85
 unwrap addons/api/src/api/pool.rs 9
 unwrap addons/api/src/print/hooks.rs 3
 unwrap addons/vocdoni/src/cmd.rs 2
```

Run: `TMPDIR=$HOME/.cache/cl-tmp cargo test -p claudio-tui --lib && python3 scripts/check_rules.py`
Expected: `test result: ok. 406 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` and `rules: 0 problem(s); …`
Check: after `git add crates/tui/src/wizard_host.rs`, `git diff HEAD --numstat` shows `10 319 crates/tui/src/wizard.rs`, `316 0 crates/tui/src/wizard_host.rs`, `1 0 crates/tui/src/lib.rs`, `3 3 scripts/size-baseline.txt`.

Step 3: commit
`git add crates/tui/src/wizard_host.rs && git commit -m "refactor(tui): move the wizard's host step to wizard_host.rs" -- crates/tui/src/wizard.rs crates/tui/src/wizard_host.rs crates/tui/src/lib.rs scripts/size-baseline.txt`

Interfaces this task provides to later tasks: `crate::wizard_host::{HostStep, HostSection, looks_like_host}` (also `crate::wizard::{HostStep, HostSection}`); `HostStep::{local_dirs, candidates, refilter}` are `pub(crate)`.

### Task 4: TUI: the directory step shows listing errors and a cut list
Model: sonnet   Repo: /home/p4u/claudio   Files: crates/tui/src/wizard.rs, crates/tui/src/interaction.rs, crates/tui/src/app.rs, crates/tui/src/ui/mod.rs, crates/tui/tests/manager.rs
Goal: spec "TUI" bullets 1-2; AC2, AC8 and AC9 (TUI parts), AC7's TUI test kept (`hidden_toggle_and_dot_fragment_work_in_the_browser`). Rules: A6 (`App` pure; the wizard state machine pure), A8 (`on_reply` shrinks to 105; `Wizard::on_key`/`refilter` untouched), C2 (the error line is sanitized, one line).

Step 1: write the failing tests

At the end of `crates/tui/src/wizard.rs`'s `mod tests`:

```rust
    #[test]
    fn a_listing_error_shows_and_the_next_listing_clears_it() {
        let (mut w, _) = browser(&["repos"]);
        assert_eq!(type_str(&mut w, "x/")[1], Outcome::ListDir("/home/u/x".into()));
        let error = "cannot list /home/u/x: Permission denied (os error 13)\nmore";
        w.set_dir_error("/home/u/other", error);
        assert_eq!(w.dir_status(), None, "an error for another path is stale");
        w.set_dir_error("/home/u/x", error);
        assert_eq!(
            w.dir_status(),
            Some("cannot list ~/x: Permission denied (os error 13)")
        );
        assert!(w.items.is_empty());
        w.set_dir_entries("/home/u/x", &[DirEntry::simple("y", true)], false);
        assert_eq!(w.dir_status(), None);
        assert_eq!(names(&w), vec!["y"]);
    }
```

```rust
    #[test]
    fn a_cut_listing_says_so_and_enter_takes_a_typed_unlisted_name() {
        let (mut w, _) = browser(&[]);
        w.set_dir_entries("/home/u", &[DirEntry::simple("alpha", true)], true);
        assert_eq!(w.dir_status(), Some(CUT_NOTE));
        assert_eq!(CUT_NOTE, "list cut: type the folder's full name");
        type_str(&mut w, "zeta");
        assert!(w.items.is_empty());
        assert_eq!(
            w.on_key(&press(KeyCode::Enter)),
            Outcome::ChooseDir("/home/u/zeta".into())
        );
    }
```

```rust
    #[test]
    fn an_unsorted_listing_shows_folders_only_sorted_case_insensitively() {
        let (mut w, _) = browser(&[]);
        let entries = [
            DirEntry::simple("b.txt", false),
            DirEntry::simple("zed", true),
            DirEntry::simple("A.md", false),
            DirEntry::simple("Alpha", true),
            DirEntry::simple("beta", true),
        ];
        w.set_dir_entries("/home/u", &entries, false);
        assert_eq!(names(&w), vec!["Alpha", "beta", "zed"]);
    }
```

In `crates/tui/tests/manager.rs`, before `// ── daemon CLI`:

```rust
/// A path under a regular file cannot be listed: the directory step says
/// why instead of showing an empty list (a mode-000 folder would not do: it
/// is listable when the suite runs as root).
#[test]
fn test_wizard_shows_a_listing_error() {
    let harness = ManagerHarness::new();
    fs::write(harness.home.join("afile"), b"").expect("write a file under HOME");
    let mut tui = harness.start_tui();
    tui.send_keys(ENTER);
    tui.wait_for("start here: ~", Region::Screen, WAIT);
    tui.send_keys(b"afile/x");
    tui.wait_for("cannot list ~/afile: Not a directory", Region::Screen, WAIT);
    tui.quit(WAIT);
}
```

Run: `TMPDIR=$HOME/.cache/cl-tmp cargo test -p claudio-tui --lib wizard`
Expected: the build fails: `set_dir_error`, `dir_status`, `CUT_NOTE` and the third argument of `set_dir_entries` do not exist yet.

Step 2: implement

`crates/tui/src/wizard.rs` (every existing test call of `set_dir_entries` gains `, false`):

```diff
diff --git a/crates/tui/src/wizard.rs b/crates/tui/src/wizard.rs
index dc15c7e..5cf12d4 100644
--- a/crates/tui/src/wizard.rs
+++ b/crates/tui/src/wizard.rs
@@ -130,8 +130,16 @@ pub struct Wizard {
     /// the lifetime of the wizard. A query that starts with `.` reveals them
     /// regardless (see [`Wizard::refilter`]).
     pub show_hidden: bool,
+    /// Why the listed directory could not be listed (one line, sanitized);
+    /// cleared by the next listing.
+    dir_error: Option<String>,
+    /// Whether the daemon cut the listing (a folder may be missing from it).
+    dir_cut: bool,
 }
 
+/// The status line of a cut listing.
+pub const CUT_NOTE: &str = "list cut: type the folder's full name";
+
 /// Seed candidates in priority order, deduplicated (trailing slashes are
 /// ignored when comparing).
 pub fn assemble(active_cwd: Option<&str>, recent: &[String], projects: &[String]) -> Vec<String> {
@@ -228,6 +236,8 @@ impl Wizard {
             meta,
             saved_host_step: None,
             show_hidden: false,
+            dir_error: None,
+            dir_cut: false,
         };
         w.refilter();
         w
@@ -284,6 +294,7 @@ impl Wizard {
         self.seeds = new_seeds;
         self.completions.clear();
         self.listed = None;
+        (self.dir_error, self.dir_cut) = (None, false);
         self.input.clear();
         self.selected = 0;
         // Reset meta for the new host but preserve recently_used marks.
@@ -308,11 +319,13 @@ impl Wizard {
         self.refilter();
     }
 
-    /// A `ListDir` reply. Ignored unless it answers the latest request.
-    pub fn set_dir_entries(&mut self, path: &str, entries: &[DirEntry]) {
+    /// A `ListDir` reply (`truncated`: the daemon cut it). Ignored unless it
+    /// answers the latest request.
+    pub fn set_dir_entries(&mut self, path: &str, entries: &[DirEntry], truncated: bool) {
         if self.listed.as_deref() != Some(path) {
             return;
         }
+        (self.dir_error, self.dir_cut) = (None, truncated);
         self.completions = entries
             .iter()
             .filter(|e| e.dir)
@@ -334,6 +347,35 @@ impl Wizard {
         self.refilter();
     }
 
+    /// A `ListDir` of `path` failed with `error` (the daemon's "cannot list
+    /// <path>: <why>"): the list empties and the step says why. Ignored
+    /// unless it answers the latest request.
+    pub fn set_dir_error(&mut self, path: &str, error: &str) {
+        if self.listed.as_deref() != Some(path) {
+            return;
+        }
+        let line = claudio_core::text::sanitize(error.lines().next().unwrap_or(""));
+        let shown = match line.strip_prefix(&format!("cannot list {path}")) {
+            Some(why) => format!("cannot list {}{why}", self.display(path)),
+            None => line,
+        };
+        (self.dir_error, self.dir_cut) = (Some(shown), false);
+        self.completions.clear();
+        self.refilter();
+    }
+
+    /// The directory step's status line while browsing: the listing's error,
+    /// or that the daemon cut it.
+    pub fn dir_status(&self) -> Option<&str> {
+        if !self.browsing() || self.host_step.is_some() {
+            return None;
+        }
+        match &self.dir_error {
+            Some(error) => Some(error),
+            None => self.dir_cut.then_some(CUT_NOTE),
+        }
+    }
+
     /// Populate enrichment metadata from a `RecentProjects` reply.
     ///
     /// Call this after `add_seeds_for_host` so the meta is available for the
@@ -956,6 +998,7 @@ mod tests {
                 DirEntry::simple("Desktop", true),
                 DirEntry::simple("reports", true),
             ],
+            false,
         );
         assert_eq!(w.items[..2], strings(&["/home/u/reports", "/home/u/repos"]));
         // The best match (first child) is preselected; Down moves to `repos`.
@@ -973,7 +1016,7 @@ mod tests {
     fn stale_dir_entries_are_ignored() {
         let mut w = wizard_local(vec![], "/h");
         type_str(&mut w, "/a/");
-        w.set_dir_entries("/b", &[DirEntry::simple("x", true)]);
+        w.set_dir_entries("/b", &[DirEntry::simple("x", true)], false);
         assert!(w.items.is_empty());
     }
 
@@ -1356,6 +1399,7 @@ mod tests {
                 DirEntry::simple(".secret", true),
                 DirEntry::simple("repos", true),
             ],
+            false,
         );
         w
     }
@@ -1443,7 +1487,7 @@ mod tests {
         w.on_host_connected("local", "/home/u", strings(&["/srv/app"]));
         let opened = w.browse_home();
         let entries: Vec<DirEntry> = entries.iter().map(|n| DirEntry::simple(*n, true)).collect();
-        w.set_dir_entries("/home/u", &entries);
+        w.set_dir_entries("/home/u", &entries, false);
         (w, opened)
     }
 
@@ -1474,7 +1518,7 @@ mod tests {
         );
         w.on_host_connected("box", "/home/remote", vec![]);
         assert_eq!(w.browse_home(), Outcome::ListDir("/home/remote".into()));
-        w.set_dir_entries("/home/remote", &[DirEntry::simple("src", true)]);
+        w.set_dir_entries("/home/remote", &[DirEntry::simple("src", true)], false);
         assert_eq!(
             w.on_key(&press(KeyCode::Enter)),
             Outcome::ChooseDir("/home/remote".into())
@@ -1544,7 +1588,7 @@ mod tests {
             w.here_selected(),
             "`start here` is selected after descending"
         );
-        w.set_dir_entries("/home/u/repos", &[DirEntry::simple("claudio", true)]);
+        w.set_dir_entries("/home/u/repos", &[DirEntry::simple("claudio", true)], false);
         w.on_key(&press(KeyCode::Down));
         assert_eq!(
             w.on_key(&press(KeyCode::Tab)),
@@ -1656,7 +1700,7 @@ mod tests {
     fn seeds_do_not_leak_into_a_browsed_directory() {
         let mut w = wizard_local(strings(&["/home/u/repos/claudio"]), "/home/u");
         type_str(&mut w, "~/");
-        w.set_dir_entries("/home/u", &[DirEntry::simple("docs", true)]);
+        w.set_dir_entries("/home/u", &[DirEntry::simple("docs", true)], false);
         assert_eq!(w.items, strings(&["/home/u/docs"]));
     }
 
@@ -1681,4 +1725,50 @@ mod tests {
         w.on_paste("x");
         assert_eq!(w.input, "/srv/app/x");
     }
+
+    #[test]
+    fn a_listing_error_shows_and_the_next_listing_clears_it() {
+        let (mut w, _) = browser(&["repos"]);
+        assert_eq!(type_str(&mut w, "x/")[1], Outcome::ListDir("/home/u/x".into()));
+        let error = "cannot list /home/u/x: Permission denied (os error 13)\nmore";
+        w.set_dir_error("/home/u/other", error);
+        assert_eq!(w.dir_status(), None, "an error for another path is stale");
+        w.set_dir_error("/home/u/x", error);
+        assert_eq!(
+            w.dir_status(),
+            Some("cannot list ~/x: Permission denied (os error 13)")
+        );
+        assert!(w.items.is_empty());
+        w.set_dir_entries("/home/u/x", &[DirEntry::simple("y", true)], false);
+        assert_eq!(w.dir_status(), None);
+        assert_eq!(names(&w), vec!["y"]);
+    }
+
+    #[test]
+    fn a_cut_listing_says_so_and_enter_takes_a_typed_unlisted_name() {
+        let (mut w, _) = browser(&[]);
+        w.set_dir_entries("/home/u", &[DirEntry::simple("alpha", true)], true);
+        assert_eq!(w.dir_status(), Some(CUT_NOTE));
+        assert_eq!(CUT_NOTE, "list cut: type the folder's full name");
+        type_str(&mut w, "zeta");
+        assert!(w.items.is_empty());
+        assert_eq!(
+            w.on_key(&press(KeyCode::Enter)),
+            Outcome::ChooseDir("/home/u/zeta".into())
+        );
+    }
+
+    #[test]
+    fn an_unsorted_listing_shows_folders_only_sorted_case_insensitively() {
+        let (mut w, _) = browser(&[]);
+        let entries = [
+            DirEntry::simple("b.txt", false),
+            DirEntry::simple("zed", true),
+            DirEntry::simple("A.md", false),
+            DirEntry::simple("Alpha", true),
+            DirEntry::simple("beta", true),
+        ];
+        w.set_dir_entries("/home/u", &entries, false);
+        assert_eq!(names(&w), vec!["Alpha", "beta", "zed"]);
+    }
 }
```

`crates/tui/src/app.rs`:

```diff
diff --git a/crates/tui/src/app.rs b/crates/tui/src/app.rs
index 3f81044..bd84d23 100644
--- a/crates/tui/src/app.rs
+++ b/crates/tui/src/app.rs
@@ -192,9 +192,11 @@ pub enum ReplyTo {
     Respawned(SessionId),
     /// Kill acknowledged; the `SessionId` lets us clear the tombstone.
     Kill(SessionId),
-    /// A directory listing for the wizard (which ignores one for a path it
-    /// no longer shows).
-    DirEntries,
+    /// A directory listing of `path` for the wizard (which ignores one for a
+    /// path it no longer shows).
+    DirEntries {
+        path: String,
+    },
     /// The claude sessions in `cwd` on `host`, for the wizard of generation
     /// `gen`; a reply for a cancelled or replaced wizard is dropped.
     ClaudeSessions {
```

`crates/tui/src/interaction.rs` (`on_reply`'s two `DirEntries` arms become one call):

```diff
diff --git a/crates/tui/src/interaction.rs b/crates/tui/src/interaction.rs
index 9878f23..14c762f 100644
--- a/crates/tui/src/interaction.rs
+++ b/crates/tui/src/interaction.rs
@@ -607,7 +607,8 @@ impl App {
             }
             Outcome::ConnectHost(host) => self.emit(Effect::Connect(host)),
             Outcome::ListDir(path) => {
-                self.request(&wizard_host, Msg::ListDir { path }, ReplyTo::DirEntries)
+                let to = ReplyTo::DirEntries { path: path.clone() };
+                self.request(&wizard_host, Msg::ListDir { path }, to)
             }
             Outcome::ChooseDir(cwd) => {
                 // A directory picked from the first screen's LOCAL section:
@@ -687,6 +688,23 @@ impl App {
             .collect()
     }
 
+    /// The wizard's `ListDir` of `path` answered: its folders, or why it
+    /// could not be listed (shown in the directory step).
+    fn on_dir_entries(&mut self, path: &str, reply: io::Result<Msg>) {
+        let Some(w) = self.wizard_mut() else {
+            return;
+        };
+        match reply {
+            Ok(Msg::DirEntries {
+                path,
+                entries,
+                truncated,
+            }) => w.set_dir_entries(&path, &entries, truncated),
+            Ok(_) => {}
+            Err(e) => w.set_dir_error(path, &e.to_string()),
+        }
+    }
+
     pub fn on_host_error(&mut self, host: &str, error: &str) {
         if let Some(Modal::Wizard(w)) = &mut self.modal {
             if let Some(hs) = &mut w.host_step {
@@ -865,11 +883,7 @@ impl App {
                     }
                 }
             }
-            (ReplyTo::DirEntries, Ok(Msg::DirEntries { path, entries, .. })) => {
-                if let Some(w) = self.wizard_mut() {
-                    w.set_dir_entries(&path, &entries);
-                }
-            }
+            (ReplyTo::DirEntries { path }, reply) => self.on_dir_entries(&path, reply),
             (ReplyTo::ClaudeSessions { host, cwd, gen }, reply) => {
                 if gen != self.wizard_generation() {
                     return;
@@ -932,8 +946,6 @@ impl App {
                 }
             }
             (ReplyTo::ClaudeUpdate(host), reply) => self.on_claude_updated(&host, reply),
-            // Typing a path that doesn't exist (yet) is not an error.
-            (ReplyTo::DirEntries, Err(_)) => {}
             (to, Err(e)) if self.connected => {
                 let what = match to {
                     ReplyTo::Ack(what) => what,
```

`crates/tui/src/ui/mod.rs` (the second head row shows the status instead of the hints while there is one):

```diff
diff --git a/crates/tui/src/ui/mod.rs b/crates/tui/src/ui/mod.rs
index 2272aa3..93edfb7 100644
--- a/crates/tui/src/ui/mod.rs
+++ b/crates/tui/src/ui/mod.rs
@@ -492,9 +492,15 @@ fn draw_wizard(frame: &mut Frame, w: &Wizard, app: &App) {
         } else {
             ""
         };
+        let (line, style) = match w.dir_status() {
+            Some(status) => (status.to_owned(), Style::default().fg(Color::Yellow)),
+            None => (
+                format!("{hint}{} hidden: {state}", toggle_key(app)),
+                Style::default().add_modifier(Modifier::DIM),
+            ),
+        };
         frame.render_widget(
-            Paragraph::new(format!("{hint}{} hidden: {state}", toggle_key(app)))
-                .style(Style::default().add_modifier(Modifier::DIM)),
+            Paragraph::new(line).style(style),
             Rect {
                 y: head.y + 1,
                 height: 1,
```

Run: `cargo fmt && TMPDIR=$HOME/.cache/cl-tmp cargo test -p claudio-tui --lib`
Expected: `test result: ok. 409 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`
Run: `TMPDIR=$HOME/.cache/cl-tmp cargo test --test manager test_wizard_ -- --test-threads=1`
Expected: `test test_wizard_shows_a_listing_error ... ok` and `test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 43 filtered out`
Run: `python3 scripts/check_rules.py`
Expected: `rules: 0 problem(s); …` (`wizard.rs` 870 of 1138, `on_reply` 105, `draw_wizard` 100 of 188).

Step 3: commit
`git commit -m "feat(tui): show why a folder cannot be listed and when a listing was cut" -- crates/tui/src/wizard.rs crates/tui/src/interaction.rs crates/tui/src/app.rs crates/tui/src/ui/mod.rs crates/tui/tests/manager.rs`

Interfaces this task provides to later tasks: `Wizard::set_dir_entries(&mut self, path: &str, entries: &[DirEntry], truncated: bool)`, `Wizard::set_dir_error(&mut self, path: &str, error: &str)`, `Wizard::dir_status(&self) -> Option<&str>`, `wizard::CUT_NOTE`, `ReplyTo::DirEntries { path: String }`.

### Task 5: TUI: the "Search conversations" row, searched on every connected host
Model: opus   Repo: /home/p4u/claudio   Files: crates/tui/src/wizard_host.rs, crates/tui/src/wizard.rs, crates/tui/src/ui/wizard_host.rs, crates/tui/src/app.rs, crates/tui/src/interaction.rs, crates/tui/src/builtins.rs, crates/tui/src/search_edge.rs (new), crates/tui/src/lib.rs, crates/tui/tests/manager.rs, addons/vocdoni/tests/manager.rs
Goal: spec "TUI" "Finding a conversation" (owner decision F15: the existing Picker) and AC3, AC10 (TUI part). Rules: A5 (the fan-out is `claudio_core::app::search::search_clients`, nothing re-implemented here), A6 (`App` emits `Effect::SearchAll` and takes `SearchAllDone`; the I/O is `search_edge::run`), A7 (the existing Picker, no new renderer), A8 (`HostStep::on_key` 110 → 81 by moving Enter to `HostStep::enter`; `HostStep::refilter` stays 85; `run_effect` 123 → 119; `event_loop` 220 of 229; `lib.rs` 812 of 814).

Two existing tests change with the behaviour, as the spec wants ("connect to" is never the default while the search row shows): `wizard::tests::host_step_unknown_host_shows_connect_raw` and `app::tests::alt_f_switches_forwarding_for_the_highlighted_remote_host` press ↓ past the search row, and so do the manager tests `test_wizard_alt_f_forwards_the_agent_and_reconnects_the_host` and `addons/vocdoni/tests/manager.rs` `test_a_recovered_remote_proxy_session_keeps_its_proxy` (it typed a host and pressed Enter).

Step 1: write the failing tests

In `crates/tui/src/wizard.rs`'s `mod tests`, replace `host_step_unknown_host_shows_connect_raw` with these three:

```rust
    /// Typing a host-like token that matches no candidate shows connect_raw.
    #[test]
    fn host_step_unknown_host_shows_connect_raw() {
        let mut hs = make_host_step(&["prod"], &[]);
        hs.input = "newhost".to_owned();
        hs.refilter();
        assert!(
            hs.connect_raw,
            "connect_raw should be set for unknown host-like query"
        );
        assert_eq!(hs.focus, HostSection::Remote);
        assert_eq!(hs.remote_len(), 2, "the search row, then connect_raw");
        // ↓ past the search row (the default), then Enter uses the raw input.
        hs.on_key(&KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        let out = hs.on_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(out, Outcome::ConnectHost("newhost".to_owned()));
    }
```

```rust
    #[test]
    fn host_step_search_row_is_the_default_when_nothing_matches() {
        let mut hs = make_host_step(&["prod"], &["/home/u/repos/claudio"]);
        for c in "zyzzyva".chars() {
            hs.on_key(&KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        assert!(hs.search && hs.connect_raw);
        assert_eq!(hs.focus, HostSection::Remote);
        assert!(hs.search_selected(), "connect to is never the default");
        let out = hs.on_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(out, Outcome::Search("zyzzyva".to_owned()));
        assert_eq!(hs.connecting, None);
    }
```

```rust
    #[test]
    fn host_step_search_row_follows_the_hosts_and_needs_two_characters() {
        let mut hs = make_host_step(&["prod", "nas"], &[]);
        hs.on_key(&KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
        assert!(!hs.search, "one character searches nothing");
        hs.on_key(&KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
        assert_eq!(hs.items, ["prod"]);
        assert!(hs.search && !hs.connect_raw);
        // The matching host stays the default; the search row is below it.
        assert_eq!((hs.focus.clone(), hs.selected), (HostSection::Remote, 0));
        hs.on_key(&KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert!(hs.search_selected());
        let out = hs.on_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(out, Outcome::Search("pr".to_owned()));
    }
```

In `crates/tui/src/builtins.rs`'s `mod tests` (imports: `use crate::test_support::{alt, app_with, info, press};` and `use crossterm::event::KeyCode;`), before `a_hit_without_a_directory_is_not_resumed`:

```rust
    /// Open the wizard (Alt+n) unless it is, type `text` on its first
    /// screen and press Enter: the `SearchAll` that emits.
    fn wizard_search(app: &mut App, text: &str) -> (u64, String) {
        if app.modal.is_none() {
            app.on_terminal(alt('n'));
        }
        for c in text.chars() {
            app.on_terminal(press(KeyCode::Char(c)));
        }
        app.on_terminal(press(KeyCode::Enter));
        app.take_effects()
            .into_iter()
            .find_map(|e| match e {
                Effect::SearchAll { gen, query } => Some((gen, query)),
                _ => None,
            })
            .expect("a SearchAll")
    }
```

```rust
    fn host_hit(host: &str, home: &str, conversation: &str, cwd: &str, modified: u64) -> HostHit {
        let hit = SearchHit {
            modified,
            ..hit(conversation, cwd)
        };
        let (host, home) = (host.to_owned(), home.to_owned());
        HostHit { host, home, hit }
    }
```

```rust
    #[test]
    fn the_wizard_search_row_lists_every_hosts_hits_and_resumes() {
        let mut app = app_with(&[info(Some(1), Some("open-conv"))]);
        app.activate(0);
        app.on_terminal(alt('n'));
        assert!(matches!(app.modal, Some(Modal::Wizard(_))));
        let (gen, query) = wizard_search(&mut app, "zyzzyva");
        assert_eq!(query, "zyzzyva");
        assert!(app.modal.is_none(), "Enter closes the wizard");
        let done = |gen, hits, failed| SearchAllDone {
            gen,
            query: query.clone(),
            searched: 3,
            hits,
            failed,
        };
        // A reply for an older search is dropped.
        app.on_search_all(done(
            gen - 1,
            vec![host_hit("local", "/home/u", "x", "/x", 1)],
            vec![],
        ));
        assert!(app.modal.is_none());
        let hits = vec![
            host_hit("local", "/home/u", "open-conv", "/home/u/p", 200),
            host_hit("z6", "/srv", "far-conv", "/srv/app", 100),
        ];
        let failed = vec![("box".to_owned(), "timed out".to_owned())];
        app.on_search_all(done(gen, hits, failed));
        let Some(Modal::Picker(picker)) = &app.modal else {
            panic!("no list")
        };
        assert_eq!(picker.title, "2 conversations · \"zyzzyva\"");
        assert!(
            picker.items[0]
                .label
                .ends_with("local  ~/p  Fix the parser"),
            "{}",
            picker.items[0].label
        );
        assert!(
            picker.items[1].label.ends_with("z6  ~/app  Fix the parser"),
            "{}",
            picker.items[1].label
        );
        assert_eq!(
            app.notice.as_ref().unwrap().text,
            "search failed on box (timed out)"
        );
        // A hit whose conversation is open in a tab: that tab, no new one.
        let open = picker.items[0].result.clone();
        app.modal = None;
        let tabs = app.sessions.len();
        app.apply_result(open);
        assert_eq!(app.sessions.len(), tabs);
        assert_eq!(app.active, Some(0));
    }
```

```rust
    #[test]
    fn a_wizard_search_says_why_it_found_nothing_and_runs_one_at_a_time() {
        let mut app = app_with(&[]);
        let (gen, query) = wizard_search(&mut app, "zyzzyva");
        // One at a time: a second search waits for the first.
        app.on_terminal(alt('n'));
        for c in "other".chars() {
            app.on_terminal(press(KeyCode::Char(c)));
        }
        app.on_terminal(press(KeyCode::Enter));
        assert!(!app
            .take_effects()
            .iter()
            .any(|e| matches!(e, Effect::SearchAll { .. })));
        assert_eq!(
            app.notice.as_ref().unwrap().text,
            "a search is already running"
        );
        let none = SearchAllDone {
            gen,
            query,
            searched: 0,
            hits: vec![],
            failed: vec![],
        };
        app.on_search_all(none);
        assert_eq!(
            app.notice.as_ref().unwrap().text,
            "no connected host can search (run `claudio daemon restart` there)"
        );
        let (gen, query) = wizard_search(&mut app, "zyzzyva");
        let failed = vec![("z6".to_owned(), "connection closed".to_owned())];
        let (searched, hits) = (2, vec![]);
        app.on_search_all(SearchAllDone {
            gen,
            query,
            searched,
            hits,
            failed,
        });
        assert_eq!(
            app.notice.as_ref().unwrap().text,
            "search failed on z6 (connection closed)"
        );
        let (gen, query) = wizard_search(&mut app, "zyzzyva");
        let (searched, hits, failed) = (1, vec![], vec![]);
        app.on_search_all(SearchAllDone {
            gen,
            query,
            searched,
            hits,
            failed,
        });
        assert_eq!(app.notice.as_ref().unwrap().text, "no matches");
        assert!(app.modal.is_none());
    }
```

`crates/tui/src/app.rs`, in `alt_f_switches_forwarding_for_the_highlighted_remote_host`:

```diff
diff --git a/crates/tui/src/app.rs b/crates/tui/src/app.rs
index bd84d23..bf472ae 100644
--- a/crates/tui/src/app.rs
+++ b/crates/tui/src/app.rs
@@ -138,6 +138,9 @@ pub enum Effect {
         id: SessionId,
         answers: Vec<claudio_core::proto::Answer>,
     },
+    /// Search the conversations on every connected host that can
+    /// (`search_edge::run`); the hits go to [`App::on_search_all`] with `gen`.
+    SearchAll { gen: u64, query: String },
     /// A call into an addon (`addon_runner::run`).
     Addon(AddonCall),
     /// A read or a write of the custom env, off the UI thread
@@ -333,6 +336,8 @@ pub struct App {
     pub extension_config: std::collections::BTreeMap<String, bool>,
     /// The `modal_gen` of the external action whose result is awaited.
     pub(super) pending_action: Option<u64>,
+    /// The `gen` of the [`Effect::SearchAll`] under way: one runs at a time.
+    pub(super) searching: Option<u64>,
     /// The external extensions the extension host disabled after
     /// failures (until claudio restarts), by name.
     pub failed_extensions: Vec<String>,
@@ -424,6 +429,7 @@ impl App {
                 Vec::new(),
             ),
             pending_action: None,
+            searching: None,
             failed_extensions: Vec::new(),
             extension_config: Default::default(),
             addons: cfg.addons,
@@ -464,6 +470,7 @@ impl App {
         }
         match hs.items.get(hs.selected) {
             Some(host) => Some(host.clone()),
+            None if hs.search_selected() => None,
             None => hs.connect_raw.then(|| hs.input.trim().to_owned()),
         }
     }
@@ -868,6 +875,9 @@ mod tests {
         for c in "me@10.0.0.9".chars() {
             app.on_terminal(press(KeyCode::Char(c)));
         }
+        // The search row comes first and names no host; ↓ is "connect to".
+        assert_eq!(app.wizard_remote_host(), None);
+        app.on_terminal(press(KeyCode::Down));
         assert_eq!(app.wizard_remote_host().as_deref(), Some("me@10.0.0.9"));
         app.on_terminal(alt('f'));
         let effects = app.take_effects();
```

(that diff also holds Step 2's `app.rs` changes: `Effect::SearchAll`, `App::searching`, `wizard_remote_host`).

`crates/tui/tests/manager.rs` (the new test, its transcript helper, and the ↓ in the agent test):

```diff
diff --git a/crates/tui/tests/manager.rs b/crates/tui/tests/manager.rs
index a5c3baa..ffedd13 100644
--- a/crates/tui/tests/manager.rs
+++ b/crates/tui/tests/manager.rs
@@ -507,9 +507,11 @@ fn test_wizard_alt_f_forwards_the_agent_and_reconnects_the_host() {
     let path = fake.path();
     let mut tui = harness.start_tui_with(|cmd| cmd.env("PATH", &path));
 
-    // A typed host ("connect to"), then a session in one of its dirs.
+    // A typed host ("connect to", below the search row), then a session in
+    // one of its dirs.
     tui.send_paste("fakehost");
     tui.wait_for("connect to fakehost", Region::Screen, WAIT);
+    tui.send_keys(DOWN_ARROW);
     tui.wait_for("Alt+f ssh agent for fakehost: off", Region::Screen, WAIT);
     tui.send_keys(ENTER);
     tui.wait_for("New session on fakehost", Region::Screen, DAEMON_WAIT);
@@ -1838,6 +1840,11 @@ fn test_plain_rejects_print_mode() {
 /// Write one claude transcript for `dir` in `home`, filed the way claude
 /// files it (under the resolved path).
 fn write_transcript(home: &std::path::Path, dir: &std::path::Path) {
+    write_transcript_saying(home, dir, "PROBE-PROMPT hello");
+}
+
+/// [`write_transcript`] whose one prompt is `prompt`.
+fn write_transcript_saying(home: &std::path::Path, dir: &std::path::Path, prompt: &str) {
     let real = fs::canonicalize(dir).unwrap();
     let encoded: String = real
         .to_string_lossy()
@@ -1848,7 +1855,7 @@ fn write_transcript(home: &std::path::Path, dir: &std::path::Path) {
     fs::create_dir_all(&pdir).unwrap();
     let rec = serde_json::json!({
         "type": "user", "cwd": real.to_string_lossy(),
-        "message": {"role": "user", "content": "PROBE-PROMPT hello"},
+        "message": {"role": "user", "content": prompt},
     });
     fs::write(
         pdir.join("11111111-2222-3333-4444-555555555555.jsonl"),
@@ -1876,6 +1883,28 @@ fn test_wizard_start_here_offers_resume() {
     tui.wait_for("resume?", Region::Screen, WAIT);
 }
 
+/// The first screen's "Search conversations" row finds a conversation on
+/// the connected hosts, lists it with its host, and Enter resumes it in its
+/// folder. (The harness has no CLAUDE_CONFIG_DIR: transcripts are under
+/// `$HOME/.claude/projects`.)
+#[test]
+fn test_wizard_search_row_finds_and_resumes_a_conversation() {
+    let h = ManagerHarness::new();
+    write_transcript_saying(&h.home, &h.dirs[0], "explain the zyzzyva please");
+    let mut tui = h.start_tui();
+    tui.wait_for("Explore local dirs", Region::Screen, WAIT);
+    tui.send_keys(b"zyzzyva");
+    tui.wait_for("Search conversations for “zyzzyva”", Region::Screen, WAIT);
+    tui.send_keys(ENTER);
+    tui.wait_for("1 conversations", Region::Screen, WAIT);
+    tui.wait_for("local  ", Region::Screen, WAIT);
+    tui.wait_for("explain the zyzzyva please", Region::Screen, WAIT);
+    tui.send_keys(ENTER);
+    // (The fake prints its argv on one long, wrapped line.)
+    tui.wait_for("--resume 11111111-2222", Region::Pane, WAIT);
+    tui.quit(WAIT);
+}
+
 /// The same when the directory is picked from the first screen's recent list.
 #[test]
 fn test_wizard_recent_dir_offers_resume() {
```

`addons/vocdoni/tests/manager.rs`:

```diff
diff --git a/addons/vocdoni/tests/manager.rs b/addons/vocdoni/tests/manager.rs
index fdb5cf8..4a52921 100644
--- a/addons/vocdoni/tests/manager.rs
+++ b/addons/vocdoni/tests/manager.rs
@@ -968,6 +968,7 @@ fn test_a_recovered_remote_proxy_session_keeps_its_proxy() {
     let mut tui = harness.start_tui_with(|cmd| cmd.env("PATH", &path));
     tui.send_paste("fakehost");
     tui.wait_for("connect to fakehost", Region::Screen, WAIT);
+    tui.send_keys(DOWN_ARROW); // past the "Search conversations" row
     tui.send_keys(ENTER);
     tui.wait_for("New session on fakehost", Region::Screen, DAEMON_WAIT);
     tui.send_paste(harness.dirs[0].to_str().unwrap());
```

Run: `TMPDIR=$HOME/.cache/cl-tmp cargo test -p claudio-tui --lib`
Expected: the build fails: `Outcome::Search`, `HostStep::search`/`search_selected`, `Effect::SearchAll`, `SearchAllDone`, `HostHit` and `on_search_all` do not exist yet.

Step 2: implement

`crates/tui/src/wizard_host.rs` (the `search` flag and row, `search_selected`, Enter moved to `enter()` with the search arm, the default row):

```diff
diff --git a/crates/tui/src/wizard_host.rs b/crates/tui/src/wizard_host.rs
index 4aeec36..f61e805 100644
--- a/crates/tui/src/wizard_host.rs
+++ b/crates/tui/src/wizard_host.rs
@@ -41,6 +41,9 @@ pub struct HostStep {
     pub connecting: Option<String>,
     /// The full unfiltered SSH host list.
     pub(crate) candidates: Vec<String>,
+    /// Whether the "Search conversations for “<query>”" row shows (after the
+    /// hosts, before "connect to"): the query has at least 2 characters.
+    pub search: bool,
     /// `true` when the query looks like a bare hostname / `user@host` but matches no
     /// known candidate.  A synthetic "connect to <query>" row is appended to the
     /// REMOTE section and auto-selected so Enter immediately initiates the connection.
@@ -81,6 +84,7 @@ impl HostStep {
             items,
             connecting: None,
             candidates,
+            search: false,
             connect_raw: false,
         }
     }
@@ -92,9 +96,15 @@ impl HostStep {
 
     /// Total rows in the REMOTE section.
     ///
-    /// Includes the synthetic "connect to <query>" row when `connect_raw` is set.
+    /// Includes the synthetic "Search conversations" and "connect to <query>"
+    /// rows when they show.
     pub fn remote_len(&self) -> usize {
-        self.items.len() + if self.connect_raw { 1 } else { 0 }
+        self.items.len() + usize::from(self.search) + usize::from(self.connect_raw)
+    }
+
+    /// Whether the "Search conversations" row is highlighted.
+    pub fn search_selected(&self) -> bool {
+        self.search && self.focus == HostSection::Remote && self.selected == self.items.len()
     }
 
     /// Handle a key press on the host step.
@@ -145,7 +155,7 @@ impl HostStep {
                         let max = self.local_len().saturating_sub(1);
                         if self.local_selected < max {
                             self.local_selected += 1;
-                        } else if !self.items.is_empty() {
+                        } else if self.remote_len() > 0 {
                             // Cross edge: go to top of REMOTE section.
                             self.focus = HostSection::Remote;
                             self.selected = 0;
@@ -160,36 +170,7 @@ impl HostStep {
                 }
                 Outcome::None
             }
-            KeyCode::Enter => match self.focus {
-                HostSection::Local => {
-                    if self.local_selected == 0 {
-                        // "Explore local dirs…" → directory step
-                        Outcome::ConnectHost("local".to_owned())
-                    } else {
-                        // A specific recent dir → go directly to that dir
-                        match self.local_items.get(self.local_selected - 1) {
-                            Some(dir) => Outcome::ChooseDir(dir.clone()),
-                            None => Outcome::ConnectHost("local".to_owned()),
-                        }
-                    }
-                }
-                HostSection::Remote => {
-                    match self.items.get(self.selected) {
-                        Some(h) => {
-                            self.connecting = Some(h.clone());
-                            Outcome::ConnectHost(h.clone())
-                        }
-                        // connect_raw row or any non-empty query with no list match
-                        // → treat the raw input as the hostname.
-                        None if !self.input.trim().is_empty() => {
-                            let h = self.input.trim().to_owned();
-                            self.connecting = Some(h.clone());
-                            Outcome::ConnectHost(h)
-                        }
-                        None => Outcome::None,
-                    }
-                }
-            },
+            KeyCode::Enter => self.enter(),
             KeyCode::Backspace => {
                 self.input.pop();
                 self.refilter();
@@ -209,6 +190,43 @@ impl HostStep {
         }
     }
 
+    /// Enter: the highlighted row's action.
+    fn enter(&mut self) -> Outcome {
+        match self.focus {
+            HostSection::Local => {
+                if self.local_selected == 0 {
+                    // "Explore local dirs…" → directory step
+                    Outcome::ConnectHost("local".to_owned())
+                } else {
+                    // A specific recent dir → go directly to that dir
+                    match self.local_items.get(self.local_selected - 1) {
+                        Some(dir) => Outcome::ChooseDir(dir.clone()),
+                        None => Outcome::ConnectHost("local".to_owned()),
+                    }
+                }
+            }
+            HostSection::Remote if self.search_selected() => {
+                Outcome::Search(self.input.trim().to_owned())
+            }
+            HostSection::Remote => {
+                match self.items.get(self.selected) {
+                    Some(h) => {
+                        self.connecting = Some(h.clone());
+                        Outcome::ConnectHost(h.clone())
+                    }
+                    // connect_raw row or any non-empty query with no list match
+                    // → treat the raw input as the hostname.
+                    None if !self.input.trim().is_empty() => {
+                        let h = self.input.trim().to_owned();
+                        self.connecting = Some(h.clone());
+                        Outcome::ConnectHost(h)
+                    }
+                    None => Outcome::None,
+                }
+            }
+        }
+    }
+
     /// Paste handler for the host step: set input to the first pasted line.
     pub fn on_paste(&mut self, text: &str) {
         let first = text.lines().next().unwrap_or("").trim();
@@ -227,7 +245,7 @@ impl HostStep {
             self.focus = HostSection::Local;
             self.local_selected = 0;
             self.selected = 0;
-            self.connect_raw = false;
+            (self.search, self.connect_raw) = (false, false);
             return;
         }
 
@@ -252,8 +270,8 @@ impl HostStep {
         // ── Score the "Explore local dirs…" pseudo-entry. ─────────────────────
         let explore_score: Option<i64> = fuzzy_score(&input, "explore local dirs");
 
-        // ── Decide whether to offer a "connect to <query>" synthetic row. ─────
-        // Only when nothing else matches and the query looks like a hostname.
+        // ── Synthetic rows: search (2+ chars); "connect to" if nothing matches. ─
+        self.search = input.chars().count() >= 2;
         let has_any_match =
             !scored_remote.is_empty() || !scored_local.is_empty() || explore_score.is_some();
         self.connect_raw = !has_any_match && looks_like_host(self.input.trim());
@@ -292,10 +310,10 @@ impl HostStep {
             self.selected = self.selected.min(self.remote_len().saturating_sub(1));
             return;
         }
-        // Nothing matched. If connect_raw, point Remote at the synthetic row.
-        if self.connect_raw {
+        // Nothing matched: the first synthetic row (search, else connect_raw).
+        if self.search || self.connect_raw {
             self.focus = HostSection::Remote;
-            self.selected = 0; // remote_len() == 1 (only the connect_raw row)
+            self.selected = 0; // no host rows: the first synthetic one
         }
         // Clamp both indices within the new bounds.
         let remote_cap = self.remote_len().saturating_sub(1);
```

`crates/tui/src/wizard.rs` (`Outcome::Search`; the rest of the diff is Step 1's tests):

```diff
diff --git a/crates/tui/src/wizard.rs b/crates/tui/src/wizard.rs
index 5cf12d4..0b026b8 100644
--- a/crates/tui/src/wizard.rs
+++ b/crates/tui/src/wizard.rs
@@ -46,6 +46,9 @@ pub enum Outcome {
     ListDir(String),
     /// A directory was chosen: request `ListClaudeSessions` for it.
     ChooseDir(String),
+    /// Search every connected host's conversations for this text (the first
+    /// screen's "Search conversations" row).
+    Search(String),
     /// Start claude in `cwd`, resuming `resume` if set, routed as `select`.
     Spawn {
         cwd: String,
@@ -1291,12 +1294,43 @@ mod tests {
             "connect_raw should be set for unknown host-like query"
         );
         assert_eq!(hs.focus, HostSection::Remote);
-        assert_eq!(hs.remote_len(), 1, "only the connect_raw row");
-        // Enter should use the raw input.
+        assert_eq!(hs.remote_len(), 2, "the search row, then connect_raw");
+        // ↓ past the search row (the default), then Enter uses the raw input.
+        hs.on_key(&KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
         let out = hs.on_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
         assert_eq!(out, Outcome::ConnectHost("newhost".to_owned()));
     }
 
+    #[test]
+    fn host_step_search_row_is_the_default_when_nothing_matches() {
+        let mut hs = make_host_step(&["prod"], &["/home/u/repos/claudio"]);
+        for c in "zyzzyva".chars() {
+            hs.on_key(&KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
+        }
+        assert!(hs.search && hs.connect_raw);
+        assert_eq!(hs.focus, HostSection::Remote);
+        assert!(hs.search_selected(), "connect to is never the default");
+        let out = hs.on_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
+        assert_eq!(out, Outcome::Search("zyzzyva".to_owned()));
+        assert_eq!(hs.connecting, None);
+    }
+
+    #[test]
+    fn host_step_search_row_follows_the_hosts_and_needs_two_characters() {
+        let mut hs = make_host_step(&["prod", "nas"], &[]);
+        hs.on_key(&KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
+        assert!(!hs.search, "one character searches nothing");
+        hs.on_key(&KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
+        assert_eq!(hs.items, ["prod"]);
+        assert!(hs.search && !hs.connect_raw);
+        // The matching host stays the default; the search row is below it.
+        assert_eq!((hs.focus.clone(), hs.selected), (HostSection::Remote, 0));
+        hs.on_key(&KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
+        assert!(hs.search_selected());
+        let out = hs.on_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
+        assert_eq!(out, Outcome::Search("pr".to_owned()));
+    }
+
     /// "Explore local dirs…" must NOT stay selected when query doesn't match it.
     #[test]
     fn host_step_explore_not_selected_for_host_query() {
@@ -1729,7 +1763,10 @@ mod tests {
     #[test]
     fn a_listing_error_shows_and_the_next_listing_clears_it() {
         let (mut w, _) = browser(&["repos"]);
-        assert_eq!(type_str(&mut w, "x/")[1], Outcome::ListDir("/home/u/x".into()));
+        assert_eq!(
+            type_str(&mut w, "x/")[1],
+            Outcome::ListDir("/home/u/x".into())
+        );
         let error = "cannot list /home/u/x: Permission denied (os error 13)\nmore";
         w.set_dir_error("/home/u/other", error);
         assert_eq!(w.dir_status(), None, "an error for another path is stale");
```

`crates/tui/src/ui/wizard_host.rs`:

```diff
diff --git a/crates/tui/src/ui/wizard_host.rs b/crates/tui/src/ui/wizard_host.rs
index 060109a..f5e4a48 100644
--- a/crates/tui/src/ui/wizard_host.rs
+++ b/crates/tui/src/ui/wizard_host.rs
@@ -67,6 +67,10 @@ pub(super) fn draw_host_step(frame: &mut Frame, rect: Rect, w: &Wizard, hs: &Hos
         .iter()
         .map(|h| format!("  {h}{}", mark(h)))
         .collect();
+    if hs.search {
+        let typed = hs.input.trim();
+        remote_rows.push(format!("  Search conversations for “{typed}”"));
+    }
     if hs.connect_raw {
         let typed = hs.input.trim();
         remote_rows.push(format!("  connect to {typed}{}", mark(typed)));
```

`crates/tui/src/interaction.rs`:

```diff
diff --git a/crates/tui/src/interaction.rs b/crates/tui/src/interaction.rs
index 14c762f..6e629fd 100644
--- a/crates/tui/src/interaction.rs
+++ b/crates/tui/src/interaction.rs
@@ -629,6 +629,7 @@ impl App {
                 };
                 self.request(&wizard_host, Msg::ListClaudeSessions { cwd }, to)
             }
+            Outcome::Search(query) => self.search_all(query),
             Outcome::Spawn {
                 cwd,
                 resume,
```

`crates/tui/src/builtins.rs` (`SearchAllDone`, `HostHit`, `search_all`, `on_search_all`, `search_item` with the host column; the rest is Step 1's tests):

```diff
diff --git a/crates/tui/src/builtins.rs b/crates/tui/src/builtins.rs
index a14ce70..1c90419 100644
--- a/crates/tui/src/builtins.rs
+++ b/crates/tui/src/builtins.rs
@@ -1,5 +1,6 @@
 //! The built-in extensions' actions: search (`s`: find a conversation on
-//! the active session's host and resume it) and snippets (`p`: paste a
+//! the active session's host and resume it; the wizard's "Search
+//! conversations" row searches every connected host) and snippets (`p`: paste a
 //! saved prompt, `P`: save the last one). Pure, like `app.rs`: the daemon
 //! searches and reads the last prompt (`Msg::Search`, `Msg::LastPrompt`),
 //! and the snippet files are read and written by the event loop
@@ -17,6 +18,30 @@ use claudio_core::proto::{Msg, SearchHit, SessionKind};
 /// The most hits a search asks for.
 const SEARCH_LIMIT: u32 = 50;
 
+/// Why the wizard's search found no host to ask.
+const NO_SEARCH_HOST: &str = "no connected host can search (run `claudio daemon restart` there)";
+
+/// What an [`Effect::SearchAll`] found (`search_edge::run`).
+#[derive(Debug)]
+pub struct SearchAllDone {
+    pub gen: u64,
+    pub query: String,
+    /// How many hosts were asked (0: none could search).
+    pub searched: usize,
+    /// Newest first.
+    pub hits: Vec<HostHit>,
+    /// `(host, error)`.
+    pub failed: Vec<(String, String)>,
+}
+
+/// One hit with its host and that host's home (for `~/`).
+#[derive(Debug)]
+pub struct HostHit {
+    pub host: String,
+    pub home: String,
+    pub hit: SearchHit,
+}
+
 impl App {
     /// A built-in extension's action (`input`: what its input line got).
     pub(super) fn builtin_action(&mut self, ext: &str, action: &str, input: Option<String>) {
@@ -67,9 +92,14 @@ impl App {
             }
             Ok(Msg::SearchReply { query, hits, .. }) => {
                 let title = format!("{} conversations · \"{query}\"", hits.len());
+                let home = if host == "local" {
+                    self.home.as_str()
+                } else {
+                    ""
+                };
                 let items = hits
                     .into_iter()
-                    .map(|hit| self.search_item(host, hit))
+                    .map(|hit| self.search_item(host, home, hit, false))
                     .collect();
                 if self.modal.is_none() {
                     self.modal = Some(Modal::Picker(Box::new(Picker::new(title, items))));
@@ -89,18 +119,19 @@ impl App {
         self.redraw = true;
     }
 
-    /// One hit as a list line: `<when>  <~/cwd>  <title>`, the snippet as
-    /// its detail, resuming it as its result.
-    fn search_item(&self, host: &str, hit: SearchHit) -> ListItem {
-        let home = if host == "local" {
-            self.home.as_str()
-        } else {
-            ""
-        };
+    /// One hit as a list line: `<when>  [<host>  ]<~/cwd>  <title>` (`home`
+    /// is `host`'s, empty when unknown), the snippet as its detail, resuming
+    /// it as its result.
+    fn search_item(&self, host: &str, home: &str, hit: SearchHit, with_host: bool) -> ListItem {
         let age = fmt_age(self.now.saturating_sub(hit.modified));
         let cwd = abbreviate_home(&hit.cwd, home);
+        let host_col = if with_host {
+            format!("{host}  ")
+        } else {
+            String::new()
+        };
         ListItem {
-            label: format!("{age}  {cwd}  {}", hit.title),
+            label: format!("{age}  {host_col}{cwd}  {}", hit.title),
             detail: Some(hit.snippet),
             result: ActionResult::Resume {
                 host: host.to_owned(),
@@ -110,6 +141,61 @@ impl App {
         }
     }
 
+    /// The wizard's "Search conversations" row: close the wizard and search
+    /// every connected host ([`Effect::SearchAll`], one at a time); the hits
+    /// come back to [`App::on_search_all`].
+    pub(super) fn search_all(&mut self, query: String) {
+        self.modal = None;
+        if self.searching.is_some() {
+            return self.notify("a search is already running");
+        }
+        self.modal_gen += 1;
+        let gen = self.modal_gen;
+        (self.pending_action, self.searching) = (Some(gen), Some(gen));
+        self.notify("searching…");
+        self.emit(Effect::SearchAll { gen, query });
+    }
+
+    /// What the search started under `gen` found: its hits in the search
+    /// list (Enter resumes one), the hosts that failed in one notice.
+    pub(crate) fn on_search_all(&mut self, done: SearchAllDone) {
+        if self.searching == Some(done.gen) {
+            self.searching = None;
+        }
+        if self.pending_action != Some(done.gen) {
+            return;
+        }
+        self.pending_action = None;
+        self.redraw = true;
+        if done.searched == 0 {
+            return self.notify(NO_SEARCH_HOST);
+        }
+        let failed = (!done.failed.is_empty()).then(|| {
+            let hosts: Vec<String> = done
+                .failed
+                .iter()
+                .map(|(h, e)| format!("{h} ({e})"))
+                .collect();
+            format!("search failed on {}", hosts.join(", "))
+        });
+        if done.hits.is_empty() {
+            return self.notify(failed.unwrap_or_else(|| "no matches".to_owned()));
+        }
+        if self.modal.is_some() {
+            return self.notify("close the popup and run it again");
+        }
+        let title = format!("{} conversations · \"{}\"", done.hits.len(), done.query);
+        let items = done
+            .hits
+            .into_iter()
+            .map(|h| self.search_item(&h.host, &h.home, h.hit, true))
+            .collect();
+        self.modal = Some(Modal::Picker(Box::new(Picker::new(title, items))));
+        if let Some(failed) = failed {
+            self.notify(failed);
+        }
+    }
+
     /// Show `conversation`: switch to the tab on `host` that has it, else
     /// start one there resuming it, in `cwd`, routed as the wizard would
     /// preselect.
@@ -238,7 +324,8 @@ impl App {
 mod tests {
     use super::*;
     use crate::app::LaunchCall;
-    use crate::test_support::{app_with, info};
+    use crate::test_support::{alt, app_with, info, press};
+    use crossterm::event::KeyCode;
 
     fn hit(conversation: &str, cwd: &str) -> SearchHit {
         SearchHit {
@@ -324,6 +411,149 @@ mod tests {
         assert_eq!(spec.args, ["--resume", "new-conv"]);
     }
 
+    /// Open the wizard (Alt+n) unless it is, type `text` on its first
+    /// screen and press Enter: the `SearchAll` that emits.
+    fn wizard_search(app: &mut App, text: &str) -> (u64, String) {
+        if app.modal.is_none() {
+            app.on_terminal(alt('n'));
+        }
+        for c in text.chars() {
+            app.on_terminal(press(KeyCode::Char(c)));
+        }
+        app.on_terminal(press(KeyCode::Enter));
+        app.take_effects()
+            .into_iter()
+            .find_map(|e| match e {
+                Effect::SearchAll { gen, query } => Some((gen, query)),
+                _ => None,
+            })
+            .expect("a SearchAll")
+    }
+
+    fn host_hit(host: &str, home: &str, conversation: &str, cwd: &str, modified: u64) -> HostHit {
+        let hit = SearchHit {
+            modified,
+            ..hit(conversation, cwd)
+        };
+        let (host, home) = (host.to_owned(), home.to_owned());
+        HostHit { host, home, hit }
+    }
+
+    #[test]
+    fn the_wizard_search_row_lists_every_hosts_hits_and_resumes() {
+        let mut app = app_with(&[info(Some(1), Some("open-conv"))]);
+        app.activate(0);
+        app.on_terminal(alt('n'));
+        assert!(matches!(app.modal, Some(Modal::Wizard(_))));
+        let (gen, query) = wizard_search(&mut app, "zyzzyva");
+        assert_eq!(query, "zyzzyva");
+        assert!(app.modal.is_none(), "Enter closes the wizard");
+        let done = |gen, hits, failed| SearchAllDone {
+            gen,
+            query: query.clone(),
+            searched: 3,
+            hits,
+            failed,
+        };
+        // A reply for an older search is dropped.
+        app.on_search_all(done(
+            gen - 1,
+            vec![host_hit("local", "/home/u", "x", "/x", 1)],
+            vec![],
+        ));
+        assert!(app.modal.is_none());
+        let hits = vec![
+            host_hit("local", "/home/u", "open-conv", "/home/u/p", 200),
+            host_hit("z6", "/srv", "far-conv", "/srv/app", 100),
+        ];
+        let failed = vec![("box".to_owned(), "timed out".to_owned())];
+        app.on_search_all(done(gen, hits, failed));
+        let Some(Modal::Picker(picker)) = &app.modal else {
+            panic!("no list")
+        };
+        assert_eq!(picker.title, "2 conversations · \"zyzzyva\"");
+        assert!(
+            picker.items[0]
+                .label
+                .ends_with("local  ~/p  Fix the parser"),
+            "{}",
+            picker.items[0].label
+        );
+        assert!(
+            picker.items[1].label.ends_with("z6  ~/app  Fix the parser"),
+            "{}",
+            picker.items[1].label
+        );
+        assert_eq!(
+            app.notice.as_ref().unwrap().text,
+            "search failed on box (timed out)"
+        );
+        // A hit whose conversation is open in a tab: that tab, no new one.
+        let open = picker.items[0].result.clone();
+        app.modal = None;
+        let tabs = app.sessions.len();
+        app.apply_result(open);
+        assert_eq!(app.sessions.len(), tabs);
+        assert_eq!(app.active, Some(0));
+    }
+
+    #[test]
+    fn a_wizard_search_says_why_it_found_nothing_and_runs_one_at_a_time() {
+        let mut app = app_with(&[]);
+        let (gen, query) = wizard_search(&mut app, "zyzzyva");
+        // One at a time: a second search waits for the first.
+        app.on_terminal(alt('n'));
+        for c in "other".chars() {
+            app.on_terminal(press(KeyCode::Char(c)));
+        }
+        app.on_terminal(press(KeyCode::Enter));
+        assert!(!app
+            .take_effects()
+            .iter()
+            .any(|e| matches!(e, Effect::SearchAll { .. })));
+        assert_eq!(
+            app.notice.as_ref().unwrap().text,
+            "a search is already running"
+        );
+        let none = SearchAllDone {
+            gen,
+            query,
+            searched: 0,
+            hits: vec![],
+            failed: vec![],
+        };
+        app.on_search_all(none);
+        assert_eq!(
+            app.notice.as_ref().unwrap().text,
+            "no connected host can search (run `claudio daemon restart` there)"
+        );
+        let (gen, query) = wizard_search(&mut app, "zyzzyva");
+        let failed = vec![("z6".to_owned(), "connection closed".to_owned())];
+        let (searched, hits) = (2, vec![]);
+        app.on_search_all(SearchAllDone {
+            gen,
+            query,
+            searched,
+            hits,
+            failed,
+        });
+        assert_eq!(
+            app.notice.as_ref().unwrap().text,
+            "search failed on z6 (connection closed)"
+        );
+        let (gen, query) = wizard_search(&mut app, "zyzzyva");
+        let (searched, hits, failed) = (1, vec![], vec![]);
+        app.on_search_all(SearchAllDone {
+            gen,
+            query,
+            searched,
+            hits,
+            failed,
+        });
+        assert_eq!(app.notice.as_ref().unwrap().text, "no matches");
+        assert!(app.modal.is_none());
+    }
+
     #[test]
     fn a_hit_without_a_directory_is_not_resumed() {
         let mut app = app_with(&[info(Some(1), None)]);
```

New file `crates/tui/src/search_edge.rs`:

```rust
//! The TUI's edge of the multi-host conversation search
//! ([`claudio_core::app::search`]): [`Effect::SearchAll`] asks every
//! connected host whose daemon can search, off the event loop, and what it
//! found comes back as one `HostEvent::SearchAllDone`.
//!
//! [`Effect::SearchAll`]: super::app::Effect::SearchAll

use std::collections::HashMap;

use tokio::sync::mpsc;

use super::builtins::{HostHit, SearchAllDone};
use super::HostEvent;
use claudio_core::app::connections::Connections;
use claudio_core::app::search;

/// Search `query` on every connected host that can, for the search started
/// under `gen`.
pub(super) fn run(gen: u64, query: String, conns: &Connections, ev_tx: &mpsc::Sender<HostEvent>) {
    let mut homes = HashMap::new();
    let mut clients: Vec<_> = conns
        .map
        .iter()
        .filter_map(|(host, conn)| {
            let client = conn.client.as_ref()?;
            search::can_search(client.welcome()).then(|| {
                homes.insert(host.clone(), client.welcome().host.home.clone());
                (host.clone(), client.clone())
            })
        })
        .collect();
    clients.sort_by(|a, b| a.0.cmp(&b.0));
    let searched = clients.len();
    let tx = ev_tx.clone();
    tokio::spawn(async move {
        let found = search::search_clients(clients, query.clone()).await;
        let hits = found
            .hits
            .into_iter()
            .map(|(host, hit)| HostHit {
                home: homes.get(&host).cloned().unwrap_or_default(),
                host,
                hit,
            })
            .collect();
        let done = SearchAllDone {
            gen,
            query,
            searched,
            hits,
            failed: found.failed,
        };
        let _ = tx.send(HostEvent::SearchAllDone(done)).await;
    });
}
```

`crates/tui/src/lib.rs` (the module, `HostEvent::SearchAllDone`, its arm in `event_loop`, the `SearchAll` arm in `run_effect`, and the "not connected" arm shortened by four lines for A8):

```diff
diff --git a/crates/tui/src/lib.rs b/crates/tui/src/lib.rs
index 1057988..c4d9857 100644
--- a/crates/tui/src/lib.rs
+++ b/crates/tui/src/lib.rs
@@ -31,6 +31,7 @@ mod pane;
 mod panel_view;
 pub mod plain;
 mod reader;
+mod search_edge;
 mod selection;
 pub(crate) mod sessions;
 
@@ -440,6 +441,8 @@ enum HostEvent {
     Addon(app::AddonResult),
     /// What a read or a write of the custom env found (`env_edge`).
     Env(env_view::EnvDone),
+    /// What an `Effect::SearchAll` found (`search_edge`).
+    SearchAllDone(builtins::SearchAllDone),
 }
 
 // ── Event loop ────────────────────────────────────────────────────────────────
@@ -675,6 +678,7 @@ async fn event_loop(
                 Some(HostEvent::Launched(done)) => app.on_launched(done),
                 Some(HostEvent::Addon(result)) => app.on_addon(result),
                 Some(HostEvent::Env(done)) => app.on_env(done),
+                Some(HostEvent::SearchAllDone(done)) => app.on_search_all(done),
                 None => return Ok(Default::default()),
             },
             Some((to, reply)) = reply_rx.recv() => addon_runner::on_reply(&mut app, &runtime, to, reply),
@@ -706,13 +710,8 @@ fn run_effect(
                     });
                 }
                 None => {
-                    let _ = tx.send((
-                        to,
-                        Err(io::Error::new(
-                            io::ErrorKind::NotConnected,
-                            format!("{host}: not connected"),
-                        )),
-                    ));
+                    let error = format!("{host}: not connected");
+                    let _ = tx.send((to, Err(io::Error::new(io::ErrorKind::NotConnected, error))));
                 }
             }
         }
@@ -807,5 +806,6 @@ fn run_effect(
             }
         }
         Effect::Launch(call) => addon_runner::launch(call, app.mode, runtime, conns, ev_tx),
+        Effect::SearchAll { gen, query } => search_edge::run(gen, query, conns, ev_tx),
     }
 }
```

Run: `cargo fmt && TMPDIR=$HOME/.cache/cl-tmp cargo test -p claudio-tui --lib`
Expected: `test result: ok. 413 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`
Run: `TMPDIR=$HOME/.cache/cl-tmp cargo test --test manager test_wizard_ -- --test-threads=1`
Expected: `test test_wizard_search_row_finds_and_resumes_a_conversation ... ok`, `test test_wizard_alt_f_forwards_the_agent_and_reconnects_the_host ... ok`, `test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 43 filtered out`
Run: `TMPDIR=$HOME/.cache/cl-tmp cargo test --test manager -- --test-threads=1` and `TMPDIR=$HOME/.cache/cl-tmp cargo test --no-default-features --test manager -- --test-threads=1`
Expected (each): `test result: ok. 50 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`
Run: `TMPDIR=$HOME/.cache/cl-tmp cargo test --test vocdoni_manager -- --test-threads=1`
Expected: `test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`
Run: `python3 scripts/check_rules.py`
Expected: `rules: 0 problem(s); …`

Step 3: commit
`git add crates/tui/src/search_edge.rs && git commit -m "feat(tui): search every connected host's conversations from the wizard" -- crates/tui/src/wizard_host.rs crates/tui/src/wizard.rs crates/tui/src/ui/wizard_host.rs crates/tui/src/app.rs crates/tui/src/interaction.rs crates/tui/src/builtins.rs crates/tui/src/search_edge.rs crates/tui/src/lib.rs crates/tui/tests/manager.rs addons/vocdoni/tests/manager.rs`

Interfaces this task provides to later tasks: `wizard::Outcome::Search(String)`, `HostStep::{search, search_selected()}`, `Effect::SearchAll { gen, query }`, `builtins::{SearchAllDone, HostHit}`, `App::{search_all, on_search_all}`. Task 8 documents the row.

### Task 6: web page: the folder navigator's and the hits' pure modules
Model: sonnet   Repo: /home/p4u/claudio   Files: addons/web/web/app/dirnav.js (new), addons/web/web/app/hits.js (new), addons/web/web/tests/dirnav.test.mjs (new), addons/web/web/tests/hits.test.mjs (new)
Goal: AC4, and the node parts of AC7, AC8, AC9, AC10: the parent/fragment split, filtering and ordering, the highlight and "use" rules (never the parent; "(not found)" only with a whole listing), `~` resolution, Backspace's edit, the listing's sequence guard; the hits' order, pick action and 2-character floor shared with `ui/search.js`. Rules: CLAUDE.md "pure browser modules import nothing through bare specifiers" (these import nothing), C2 (the page asks only with 2+ characters).

Step 1: write the failing tests

New file `addons/web/web/tests/dirnav.test.mjs`:

```js
import test from 'node:test';
import assert from 'node:assert/strict';
import {
  MAX_LISTED, isPath, splitPath, useTarget, resolveTilde, abbreviateHome, startFolder, folderRows, useRow,
  firstHighlight, complete, backspaceUp, recentRows, sequencer,
} from '../app/dirnav.js';

const dir = (name) => ({ name, dir: true, hidden: name.startsWith('.') });
const file = (name) => ({ name, dir: false, hidden: false });

test('a typed path splits into the listed parent and the fragment', () => {
  assert.deepEqual(splitPath('~/src/cla'), { parent: '~/src/', fragment: 'cla' });
  assert.deepEqual(splitPath('~/src/'), { parent: '~/src/', fragment: '' });
  assert.deepEqual(splitPath('~'), { parent: '~/', fragment: '' });
  assert.deepEqual(splitPath('/'), { parent: '/', fragment: '' });
  assert.deepEqual(splitPath('/srv'), { parent: '/', fragment: 'srv' });
  assert.ok(isPath('~/x') && isPath('/x') && isPath(' /x'));
  assert.ok(!isPath('claudio') && !isPath(''));
});

test('the use row names the typed folder, never its parent', () => {
  assert.equal(useTarget('~/src/'), '~/src');
  assert.equal(useTarget('~/src/cla'), '~/src/cla');
  assert.equal(useTarget('/'), '/');
  assert.equal(useTarget('~/'), '~');
  const listing = { parent: '~/src/', entries: [dir('claudio'), file('notes')], truncated: false, error: null };
  assert.deepEqual(useRow('~/src/claudio', listing), { target: '~/src/claudio', notFound: false });
  assert.deepEqual(useRow('~/src/cla', listing), { target: '~/src/cla', notFound: true });
  assert.deepEqual(useRow('~/src/notes', listing), { target: '~/src/notes', notFound: true }, 'a file is not a folder');
  assert.equal(useRow('~/src/', listing).notFound, false, 'an empty fragment is the listed folder');
});

test('not found needs a whole listing of that parent', () => {
  const whole = { parent: '~/', entries: [dir('a')], truncated: false, error: null };
  assert.equal(useRow('~/missing', whole).notFound, true);
  assert.equal(useRow('~/missing', null).notFound, false, 'not arrived yet');
  assert.equal(useRow('~/missing', { ...whole, entries: null }).notFound, false, 'still reading');
  assert.equal(useRow('~/missing', { ...whole, parent: '~/x/' }).notFound, false, 'another parent');
  assert.equal(useRow('~/missing', { ...whole, truncated: true }).notFound, false, 'cut by the daemon');
  assert.equal(useRow('~/missing', { ...whole, error: 'cannot list' }).notFound, false);
  const full = Array.from({ length: MAX_LISTED }, (_, i) => dir(`d${i}`));
  assert.equal(useRow('~/missing', { ...whole, entries: full }).notFound, false, 'an old daemon cut at 5000 silently');
});

test('folder rows: folders only, prefix-filtered and sorted case-insensitively', () => {
  // An old daemon's listing: directory order, files mixed in.
  const entries = [file('b.txt'), dir('zed'), file('A.md'), dir('Alpha'), dir('beta'), dir('.cfg'), dir('alps')];
  assert.deepEqual(folderRows(entries, ''), ['Alpha', 'alps', 'beta', 'zed']);
  assert.deepEqual(folderRows(entries, 'AL'), ['Alpha', 'alps']);
  assert.deepEqual(folderRows(entries, 'q'), []);
});

test('hidden folders: off by default, on with the toggle or a dot fragment', () => {
  const entries = [dir('.cfg'), dir('src'), dir('.cache')];
  assert.deepEqual(folderRows(entries, ''), ['src']);
  assert.deepEqual(folderRows(entries, '', true), ['.cache', '.cfg', 'src']);
  assert.deepEqual(folderRows(entries, '.'), ['.cache', '.cfg']);
  assert.deepEqual(folderRows(entries, '.cf'), ['.cfg']);
});

test('the highlight: use with no fragment or no match, else the first folder', () => {
  assert.equal(firstHighlight('', ['a']), 0);
  assert.equal(firstHighlight('a', ['a', 'ab']), 1);
  assert.equal(firstHighlight('q', []), 0);
});

test('completing a folder and going up with backspace', () => {
  assert.equal(complete('~/src/', 'claudio'), '~/src/claudio/');
  assert.equal(backspaceUp('~/a/b/', '/home/u'), '~/a/');
  assert.equal(backspaceUp('~/a/', '/home/u'), '~/');
  assert.equal(backspaceUp('~/', '/home/u'), '/home/');
  assert.equal(backspaceUp('~/', null), null, 'home unknown: an ordinary backspace');
  assert.equal(backspaceUp('/srv/app/', null), '/srv/');
  assert.equal(backspaceUp('/srv/', null), '/');
  assert.equal(backspaceUp('/', null), '/');
  assert.equal(backspaceUp('~/a/b', '/home/u'), null, 'no trailing slash: one character');
});

test('~ is resolved with the host home before anything is sent', () => {
  assert.equal(resolveTilde('~', '/home/u'), '/home/u');
  assert.equal(resolveTilde('~/src', '/home/u/'), '/home/u/src');
  assert.equal(resolveTilde('~/src', '/'), '/src');
  assert.equal(resolveTilde('~/src', null), null);
  assert.equal(resolveTilde('/srv', null), '/srv');
  assert.equal(resolveTilde('~other/x', '/home/u'), '~other/x');
  assert.deepEqual(startFolder('~/src/', '/home/u'), { cwd: '/home/u/src' });
  assert.deepEqual(startFolder('/srv/app', null), { cwd: '/srv/app' });
  assert.deepEqual(startFolder('', null), { why: null });
  assert.deepEqual(startFolder('claudio', '/home/u'), { why: 'Type a path (it starts with / or ~)' });
  assert.deepEqual(startFolder('~/x', null), { why: 'Reading the home folder…' });
  assert.deepEqual(startFolder('~/x', null, 'timed out'), { why: 'cannot read the home folder: timed out' });
  assert.ok(startFolder('~other', '/home/u').why.includes('~user'));
  assert.equal(abbreviateHome('/home/u/src', '/home/u'), '~/src');
  assert.equal(abbreviateHome('/home/uu', '/home/u'), '/home/uu');
  assert.equal(abbreviateHome('/srv', null), '/srv');
});

test('recent folders filter by substring, case-insensitively', () => {
  const dirs = [{ path: '/home/dev/Claudio' }, { path: '/home/dev/api' }, { path: '/srv/infra' }];
  assert.equal(recentRows(dirs, '').length, 3);
  assert.deepEqual(recentRows(dirs, 'claud').map((d) => d.path), ['/home/dev/Claudio']);
  assert.deepEqual(recentRows(dirs, 'DEV').map((d) => d.path), ['/home/dev/Claudio', '/home/dev/api']);
  assert.deepEqual(recentRows(dirs, 'dvapi'), [], 'substring, not fuzzy');
});

test('a listing reply for an earlier request is ignored', () => {
  const seq = sequencer();
  const first = seq.next();
  const second = seq.next();
  assert.equal(seq.fresh(first), false);
  assert.equal(seq.fresh(second), true);
  seq.drop(); // the host changed
  assert.equal(seq.fresh(second), false);
});
```

New file `addons/web/web/tests/hits.test.mjs`:

```js
import test from 'node:test';
import assert from 'node:assert/strict';
import { MIN_QUERY, searchable, searchResult, liveSessionFor, pickHit, moveSel } from '../app/hits.js';

test('a query needs two characters', () => {
  assert.equal(MIN_QUERY, 2);
  assert.equal(searchable(' a '), false);
  assert.equal(searchable('ab'), true);
  assert.equal(searchable('é'), false);
});

test('hits are shown newest first with the failures and the cut', () => {
  const r = searchResult({ hits: [{ conversation: 'a', modified: 1 }, { conversation: 'b', modified: 3 }, { conversation: 'c', modified: 2 }],
    failed: [{ host: 'z6', error: 'timed out' }], truncated: true });
  assert.deepEqual(r.hits.map((h) => h.conversation), ['b', 'c', 'a']);
  assert.deepEqual(r.failed, [{ host: 'z6', error: 'timed out' }]);
  assert.equal(r.truncated, true);
  assert.deepEqual(searchResult(undefined), { hits: [], failed: [], truncated: false });
});

test('picking a hit focuses its live session, else resumes it on its host', () => {
  const sessions = {
    s1: { id: 's1', host: 'local', claude_session_id: 'conv', dormant: false, state: 'idle' },
    s2: { id: 's2', host: 'z6', claude_session_id: 'conv2', dormant: true, state: 'idle' },
    s3: { id: 's3', host: 'z6', claude_session_id: 'conv3', dormant: false, state: 'exited' },
  };
  assert.deepEqual(pickHit(sessions, { host: 'local', conversation: 'conv', cwd: '/a' }), { focus: 's1' });
  assert.equal(liveSessionFor(sessions, { host: 'z6', conversation: 'conv' }), null, 'another host');
  for (const conversation of ['conv2', 'conv3']) {
    assert.deepEqual(pickHit(sessions, { host: 'z6', conversation, cwd: '/srv' }), { resume: { host: 'z6', cwd: '/srv', conversation } });
  }
});

test('the highlight wraps over the hits', () => {
  assert.equal(moveSel(0, 1, 3), 1);
  assert.equal(moveSel(2, 1, 3), 0);
  assert.equal(moveSel(0, -1, 3), 2);
  assert.equal(moveSel(0, 1, 0), 0);
});
```

Run: `node --test addons/web/web/tests/dirnav.test.mjs addons/web/web/tests/hits.test.mjs`
Expected: `Error [ERR_MODULE_NOT_FOUND]: Cannot find module '…/addons/web/web/app/dirnav.js' imported from …/tests/dirnav.test.mjs` (and the same for `hits.js`)

Step 2: implement

New file `addons/web/web/app/dirnav.js`:

```js
// The new-session wizard's folder field, pure part: a typed path split into the folder it lists
// and the fragment being completed, the rows shown under it, and the edits its keys make.
// Imports nothing (node --test runs it without a browser).

// The daemon's cap on one listing: a listing this long may be cut even when it does not say so
// (an older daemon cut at 5000 without setting `truncated`).
export const MAX_LISTED = 5000;

// Whether the text browses folders: a path, starting with `/` or `~`.
export const isPath = (text) => /^[/~]/.test(String(text ?? '').trim());

// `{ parent, fragment }`: `parent` is the text up to and including its last `/` (the folder that
// is listed, as typed), `fragment` what follows it. A bare `~` lists home.
export function splitPath(text) {
  const t = String(text ?? '').trim();
  if (t === '~') return { parent: '~/', fragment: '' };
  const i = t.lastIndexOf('/');
  return { parent: t.slice(0, i + 1), fragment: t.slice(i + 1) };
}

// The folder the text denotes: the text itself without a trailing `/` (`~/src/` is `~/src`,
// `/` stays `/`). Never the parent in place of the typed text.
export function useTarget(text) {
  const t = String(text ?? '').trim();
  const trimmed = t.replace(/\/+$/, '');
  return trimmed || (t.startsWith('/') ? '/' : t);
}

// `~` and `~/…` made absolute with `home` (the host's, from `fs.list {path: "~"}`); null while the
// home is unknown. `~user` is left as it is (the daemon does not expand it either).
export function resolveTilde(text, home) {
  const t = String(text ?? '').trim();
  if (t !== '~' && !t.startsWith('~/')) return t;
  if (!home) return null;
  return home.replace(/\/+$/, '') + t.slice(1) || '/';
}

// `path` with `home` shown as `~` (for a hit's folder).
export function abbreviateHome(path, home) {
  const h = String(home ?? '').replace(/\/+$/, '');
  if (!h || !path) return path;
  if (path === h) return '~';
  return path.startsWith(`${h}/`) ? `~${path.slice(h.length)}` : path;
}

// What the field's text starts in: `{ cwd }` (absolute, `~` resolved) or `{ why }` (shown under
// the field; null for an empty field). `homeError`: why the home could not be read.
export function startFolder(text, home, homeError = null) {
  const t = String(text ?? '').trim();
  if (!t) return { why: null };
  if (!isPath(t)) return { why: 'Type a path (it starts with / or ~)' };
  if (t.startsWith('~') && t !== '~' && !t.startsWith('~/')) return { why: '~user is not expanded: type the full path' };
  const cwd = resolveTilde(useTarget(t), home);
  if (cwd !== null) return { cwd };
  return { why: homeError ? `cannot read the home folder: ${homeError}` : 'Reading the home folder…' };
}

const byName = (a, b) => {
  const x = a.toLowerCase();
  const y = b.toLowerCase();
  if (x !== y) return x < y ? -1 : 1;
  return a < b ? -1 : a > b ? 1 : 0;
};

// The folder rows under a typed path: the listing's folders whose name starts with the fragment
// (case-insensitively), sorted case-insensitively (an older daemon sends files and folders in
// directory order). Hidden ones only with `showHidden` or a fragment starting with `.`.
export function folderRows(entries, fragment, showHidden = false) {
  const f = String(fragment ?? '').toLowerCase();
  const hidden = showHidden || f.startsWith('.');
  return (entries ?? [])
    .filter((e) => e.dir && e.name.toLowerCase().startsWith(f) && (hidden || !e.name.startsWith('.')))
    .map((e) => e.name)
    .sort(byName);
}

// The "▸ use <folder>" row: the folder the text denotes, and whether it is known not to exist.
// "(not found)" needs a whole listing of its parent: the reply for that parent arrived, without an
// error, not cut, with fewer than MAX_LISTED entries, and no folder of that name.
export function useRow(text, listing) {
  const target = useTarget(text);
  const { parent, fragment } = splitPath(text);
  const whole = !!listing && listing.parent === parent && !listing.error && listing.entries !== null
    && !listing.truncated && listing.entries.length < MAX_LISTED;
  const notFound = !!fragment && whole && !listing.entries.some((e) => e.dir && e.name === fragment);
  return { target, notFound };
}

// The highlighted row in [use, ...folders]: the "use" row (0) with an empty fragment or no match,
// else the first matching folder (1), as the TUI does.
export const firstHighlight = (fragment, rows) => (fragment && rows.length ? 1 : 0);

// The text after completing folder `name` of `parent` (Tab, →, Enter or a click on its row).
export const complete = (parent, name) => `${parent}${name}/`;

// Backspace with the caret at the end of a text ending in `/`: the text without its last segment
// (`~/a/b/` → `~/a/`); `~/` becomes the home's parent, absolute (null while the home is unknown);
// `/` stays. null when the text does not end in `/` (an ordinary Backspace).
export function backspaceUp(text, home) {
  if (text === '/') return '/';
  if (!text.endsWith('/')) return null;
  if (text === '~/') {
    if (!home) return null;
    const h = home.replace(/\/+$/, '');
    return h ? h.slice(0, h.lastIndexOf('/') + 1) : '/';
  }
  const t = text.slice(0, -1);
  return t.slice(0, t.lastIndexOf('/') + 1);
}

// The recent folders (`fs.projects`) a non-path text shows: those whose path contains it,
// case-insensitively; all of them for an empty text.
export function recentRows(dirs, text) {
  const q = String(text ?? '').trim().toLowerCase();
  return (dirs ?? []).filter((d) => !q || d.path.toLowerCase().includes(q));
}

// Request numbering for listings: `next()` numbers a request, `fresh(n)` says whether the reply
// to request `n` is still wanted (it answers the latest one), `drop()` makes every one stale
// (the host changed).
export function sequencer() {
  let last = 0;
  return { next: () => ++last, fresh: (n) => n === last, drop: () => { last += 1; } };
}
```

New file `addons/web/web/app/hits.js`:

```js
// Conversation search hits, pure part, shared by the search dialog (ui/search.js) and the
// wizard's "Find a conversation…" field (ui/wizard-find.js). Imports nothing.

// The shortest query the daemon takes (claude/search.rs refuses shorter ones as "bad query").
export const MIN_QUERY = 2;

// Whether `text` is worth asking every host about.
export const searchable = (text) => [...String(text ?? '').trim()].length >= MIN_QUERY;

// A `search` reply as shown: hits newest first (the server merges them so; kept stable), the
// hosts that failed, and whether a host cut its list.
export function searchResult(r) {
  const hits = [...(r?.hits ?? [])].sort((a, b) => (b.modified ?? 0) - (a.modified ?? 0));
  return { hits, failed: r?.failed ?? [], truncated: !!r?.truncated };
}

// The live session already on a hit's conversation, or null: same host and conversation, not
// dormant, not exited.
export function liveSessionFor(sessions, { host, conversation }) {
  return Object.values(sessions ?? {}).find((s) => s.host === host && s.claude_session_id === conversation
    && !s.dormant && s.state !== 'exited') ?? null;
}

// What picking a hit does: `{ focus: sid }` (its live session) or `{ resume: {host, cwd,
// conversation} }` (start a claude session there resuming it).
export function pickHit(sessions, hit) {
  const live = liveSessionFor(sessions, hit);
  if (live) return { focus: live.id };
  return { resume: { host: hit.host, cwd: hit.cwd, conversation: hit.conversation } };
}

// The highlight after moving `delta` rows over `n` hits (wrapping; 0 with none).
export const moveSel = (sel, delta, n) => (n ? (((sel + delta) % n) + n) % n : 0);
```

Run: `node --test addons/web/web/tests/dirnav.test.mjs addons/web/web/tests/hits.test.mjs`
Expected: `# tests 14`, `# pass 14`, `# fail 0`

Step 3: commit
`git add addons/web/web/app/dirnav.js addons/web/web/app/hits.js addons/web/web/tests/dirnav.test.mjs addons/web/web/tests/hits.test.mjs && git commit -m "feat(web): pure rules for the wizard's folder navigator and conversation hits" -- addons/web/web/app/dirnav.js addons/web/web/app/hits.js addons/web/web/tests/dirnav.test.mjs addons/web/web/tests/hits.test.mjs`

Interfaces this task provides to later tasks: `dirnav.js`: `MAX_LISTED, isPath, splitPath, useTarget, resolveTilde, abbreviateHome, startFolder, folderRows, useRow, firstHighlight, complete, backspaceUp, recentRows, sequencer`; `hits.js`: `MIN_QUERY, searchable, searchResult, liveSessionFor, pickHit, moveSel`.

### Task 7: web page: the folder combobox, "Find a conversation…", the mock, the e2e
Model: opus   Repo: /home/p4u/claudio   Files: addons/web/web/app/ui/dirfield.js (new), addons/web/web/app/ui/wizard-find.js (new), addons/web/web/app/ui/wizard.js, addons/web/web/app/ui/search.js, addons/web/web/app/wizard.js, addons/web/web/tests/wizard.test.mjs, addons/web/web/styles/components.css, addons/web/web/dev/mock-server.mjs, addons/web/web/e2e/tests/wizard-nav.spec.mjs (new)
Goal: spec "Web" (the combobox, keys F1/F6, focus, mouse, `~`, hidden, error/cut line, "Browse folders…", "Finding a conversation", testids) and AC5, AC7, AC11 (web parts). Rules: DESIGN.md (tokens only; the rows reuse the search list's look), CLAUDE.md "Web" (no inline script or handler: Preact's `onX` props are listeners, not inline attributes; zero CSP violations, asserted after every test), testids (kept `wizard-dir-input`, `wizard-browse`; removed `wizard-dirs`, `wizard-dir`, `wizard-browser`; new `wizard-dir-list`, `wizard-dir-row`, `wizard-dir-use`, `wizard-dir-error`, `wizard-find`, `wizard-find-row`), no protocol change (`fs.list`, `fs.projects`, `search`, `claude.sessions`, `session.spawn`).

Decisions the spec leaves open, taken here:
- The page's requests wait for the connection (`conn.value === 'open'`): a wizard opened before the
  socket is up would otherwise get "disconnected" for the home and never resolve `~` (seen in the e2e).
- "Moves focus to Start" happens once Start can start (a `~` folder waits for the home): an effect
  focuses it when `ready` turns true after a pick.
- With a recent-folder row highlighted, Enter or a click picks it (as "use"), Tab or → completes it
  into the field with a `/` (browsing into it).
- The find hits abbreviate `~/` for the wizard's current host (the only home the page knows).
- The search dialog (`/`) also waits for 2 characters now (`searchable`), so it no longer asks every
  daemon a query they all refuse.

Step 1: write the failing test

New file `addons/web/web/e2e/tests/wizard-nav.spec.mjs`:

```js
// The wizard's folder combobox (typed navigation from ~, Tab/Backspace/Enter/Esc, hidden
// folders, errors, "Browse folders…") and its "Find a conversation…" field.
import fs from 'node:fs';
import path from 'node:path';
import { test, expect } from '@playwright/test';
import { startServer } from '../fixtures/server.mjs';
import { writeTranscript, FIXTURE_WORD } from '../fixtures/transcript.mjs';

let srv;
let home;
test.beforeAll(async () => {
  srv = await startServer();
  home = fs.realpathSync(path.join(srv.root, 'home'));
  for (const d of ['alpha', 'Beta', 'gamma/inner', '.cfg']) fs.mkdirSync(path.join(home, d), { recursive: true });
  fs.writeFileSync(path.join(home, 'afile'), '');
});
test.afterAll(async () => { await srv?.stop(); });
test.afterEach(async ({ page }) => {
  expect(await page.evaluate(() => window.__claudioCsp ?? [])).toEqual([]);
  await srv.reset();
});

async function openWizard(page) {
  await page.goto(srv.url);
  await expect(page.getByTestId('rail')).toBeVisible();
  await page.keyboard.press('n');
  await expect(page.getByTestId('wizard')).toBeVisible();
  await page.getByTestId('wizard-launch').selectOption('direct');
  const input = page.getByTestId('wizard-dir-input');
  await input.focus();
  return input;
}

test('typing ~/ lists the home folders; typing filters, Tab enters, Backspace goes up', async ({ page }) => {
  const input = await openWizard(page);
  const rows = page.getByTestId('wizard-dir-row');
  await input.pressSequentially('~/');
  await expect(rows).toHaveText(['alpha/', 'Beta/', 'gamma/']);
  await expect(page.getByTestId('wizard-dir-use')).toHaveText('▸ use ~');
  await expect(page.getByTestId('wizard-dir-use')).toHaveAttribute('aria-selected', 'true');
  await input.press('g');
  await expect(rows).toHaveText(['gamma/']);
  await expect(rows.first()).toHaveAttribute('aria-selected', 'true');
  await input.press('Tab');
  await expect(input).toHaveValue('~/gamma/');
  await expect(input).toBeFocused();
  await expect(rows).toHaveText(['inner/']);
  await expect(rows.first()).toHaveAttribute('data-path', '~/gamma/inner');
  await input.press('Backspace');
  await expect(input).toHaveValue('~/');
  await expect(rows).toHaveText(['alpha/', 'Beta/', 'gamma/']);
  // Esc closes the list and stops there; a second Esc closes the dialog.
  await input.press('Escape');
  await expect(page.getByTestId('wizard-dir-list')).toHaveCount(0);
  await expect(page.getByTestId('wizard')).toBeVisible();
  await input.press('Escape');
  await expect(page.getByTestId('wizard')).toHaveCount(0);
});

test('hidden folders show with the toggle or a dot fragment', async ({ page }) => {
  const input = await openWizard(page);
  const rows = page.getByTestId('wizard-dir-row');
  await input.pressSequentially('~/');
  await expect(rows).toHaveText(['alpha/', 'Beta/', 'gamma/']);
  await expect(page.locator('[data-testid="wizard-dir-row"][data-path="~/.cfg"]')).toHaveCount(0);
  const hidden = page.getByTestId('wizard').getByLabel('hidden', { exact: true });
  await hidden.check();
  await expect(rows).toHaveText(['.cfg/', 'alpha/', 'Beta/', 'gamma/']);
  await hidden.uncheck();
  await expect(rows).toHaveText(['alpha/', 'Beta/', 'gamma/']);
  await input.press('.');
  await expect(rows).toHaveText(['.cfg/']);
});

test('Enter on use picks the folder and focuses Start; Enter again starts there', async ({ page }) => {
  const input = await openWizard(page);
  await input.pressSequentially('~/alpha');
  await expect(page.getByTestId('wizard-dir-row')).toHaveText(['alpha/']);
  await input.press('Enter'); // a folder row completes, starts nothing
  await expect(input).toHaveValue('~/alpha/');
  await expect(page.getByTestId('wizard-dir-use')).toHaveAttribute('aria-selected', 'true');
  await input.press('Enter'); // "use": the folder, the list closes, Start has the focus
  await expect(input).toHaveValue('~/alpha');
  await expect(page.getByTestId('wizard-dir-list')).toHaveCount(0);
  await expect(page.getByTestId('wizard-start')).toBeFocused();
  await expect(page.getByTestId('session-card')).toHaveCount(0);
  await page.keyboard.press('Enter');
  const card = page.getByTestId('session-card');
  await expect(card).toHaveCount(1);
  await card.hover();
  await expect(page.getByTestId('session-info')).toContainText(path.join(home, 'alpha'));
});

test('a missing folder never starts a session in ~', async ({ page }) => {
  const input = await openWizard(page);
  await input.pressSequentially('~/missing');
  await input.press('Enter'); // before the listing: "use ~/missing", nothing starts
  await expect(input).toHaveValue('~/missing');
  await expect(page.getByTestId('wizard-start')).toBeFocused();
  // After the listing: the row says so and Enter does nothing.
  await input.press('End');
  await input.press('Backspace');
  await input.press('g');
  await expect(page.getByTestId('wizard-dir-use')).toHaveText('▸ use ~/missing (not found)');
  await input.press('Enter');
  await expect(page.getByTestId('wizard-dir-list')).toBeVisible();
  await expect(page.getByTestId('wizard')).toBeVisible();
  await expect(page.getByTestId('session-card')).toHaveCount(0);
});

test('Browse folders opens the list at ~/, and an unreadable folder shows its error', async ({ page }) => {
  const input = await openWizard(page);
  await page.getByTestId('wizard-browse').click();
  await expect(input).toHaveValue('~/');
  await expect(input).toBeFocused();
  await expect(page.getByTestId('wizard-dir-row')).toHaveText(['alpha/', 'Beta/', 'gamma/']);
  await input.pressSequentially('afile/x');
  await expect(page.getByTestId('wizard-dir-error')).toContainText('cannot list ~/afile/');
  await expect(page.getByTestId('wizard-dir-error')).toContainText('Not a directory');
  await expect(page.getByTestId('wizard-dir-row')).toHaveCount(0);
});

test('Find a conversation fills the wizard and starts it with resume', async ({ page }) => {
  writeTranscript(srv.claudeConfig, srv.dirs.b, 'conv-find');
  const spawns = [];
  page.on('websocket', (ws) => ws.on('framesent', (f) => {
    try {
      const m = JSON.parse(f.payload);
      if (m.op === 'session.spawn') spawns.push(m);
    } catch { /* a binary frame */ }
  }));
  await openWizard(page);
  const find = page.getByTestId('wizard-find');
  await find.fill(FIXTURE_WORD);
  const rows = page.getByTestId('wizard-find-row');
  await expect(rows).toHaveCount(1);
  await expect(rows.first()).toHaveAttribute('data-conversation', 'conv-find');
  await expect(rows.first()).toContainText(`local · ${srv.dirs.b}`);
  await find.press('Enter'); // picks the highlighted hit, starts nothing
  await expect(page.getByTestId('wizard-dir-input')).toHaveValue(srv.dirs.b);
  await expect(page.getByTestId('wizard-start')).toBeFocused();
  await expect(page.getByTestId('wizard')).toBeVisible();
  expect(spawns).toEqual([]);
  await page.keyboard.press('Enter');
  await expect(page.getByTestId('session-card')).toHaveCount(1);
  await expect.poll(() => spawns.length).toBe(1);
  expect(spawns[0].resume).toBe('conv-find');
  expect(spawns[0].cwd).toBe(srv.dirs.b);
});

test('a hit whose conversation is live focuses that session and closes the wizard', async ({ page }) => {
  writeTranscript(srv.claudeConfig, srv.dirs.a, 'conv-live');
  const sid = await srv.spawn(srv.dirs.a);
  await srv.fireHook(srv.dirs.a, 'SessionStart', { session_id: 'conv-live', source: 'startup' });
  await openWizard(page);
  await page.getByTestId('wizard-find').fill(FIXTURE_WORD);
  await page.locator('[data-testid="wizard-find-row"][data-conversation="conv-live"]').click();
  await expect(page.getByTestId('wizard')).toHaveCount(0);
  await expect(page.locator(`[data-testid="session-card"][data-sid="${sid}"]`)).toHaveAttribute('aria-current', 'true');
  await expect(page.locator('[data-testid="pane"][data-focus="true"]').getByTestId('terminal')).toContainText('FAKE_CLAUDE_BANNER');
});
```

Run: `TMPDIR=$HOME/.cache/cl-tmp cargo build --locked && cd addons/web/web/e2e && npx playwright test wizard-nav`
Expected: all 7 tests fail (the old page has no `wizard-dir-row`, `wizard-dir-use` or `wizard-find`).

Step 2: implement

New file `addons/web/web/app/ui/dirfield.js`:

```js
// The wizard's folder field: a combobox that navigates the host's folders as you type (a path
// starting with / or ~ lists its parent's subfolders), or offers the recent folders otherwise.
// The pure rules are in ../dirnav.js; this is the state, the keys and the drawing.
import { html } from 'htm/preact';
import { useEffect, useMemo, useRef, useState } from 'preact/hooks';
import { reqQuiet } from '../store.js';
import { isPath, splitPath, folderRows, useRow, firstHighlight, complete, backspaceUp, recentRows, sequencer } from '../dirnav.js';
import { ago } from './glyph.js';

const LIST_DEBOUNCE = 120;
const CUT = "list cut: type the folder's full name";

// `w.cwd` is the field's text; `home` the host's home (null until known) for `~`; `online` whether
// the connection is up (requests wait for it). `browse` is bumped by "Browse folders…" (it sets
// the text to `~/` and opens the list). `onPicked` moves the focus on to Start.
export function DirField({ w, set, home, online, browse, onPicked }) {
  const [dirs, setDirs] = useState(null);
  const [listing, setListing] = useState(null); // { parent, entries | null, truncated, error }
  const [open, setOpen] = useState(false);
  const [sel, setSel] = useState(0);
  const [showHidden, setShowHidden] = useState(false);
  const seq = useRef(sequencer());
  const input = useRef(null);
  const list = useRef(null);
  const text = w.cwd;
  const path = isPath(text);
  const { parent, fragment } = splitPath(text);

  useEffect(() => {
    let dead = false;
    setDirs(null);
    seq.current.drop();
    setListing(null);
    setOpen(false);
    if (!online) return undefined;
    reqQuiet('fs.projects', { host: w.host, limit: 50 }).then((r) => { if (!dead) setDirs(r.dirs ?? []); }).catch(() => { if (!dead) setDirs([]); });
    return () => { dead = true; };
  }, [w.host, online]);

  // "Browse folders…": the list at ~/.
  useEffect(() => {
    if (!browse) return;
    set({ cwd: '~/', resume: null });
    setOpen(true);
    input.current?.focus();
  }, [browse]);

  // The parent's listing, asked when the parent changes (debounced); only the latest reply counts.
  const want = open && online && path && parent ? parent : null;
  useEffect(() => {
    if (!want || listing?.parent === want) return undefined;
    const t = setTimeout(() => {
      const mine = seq.current.next();
      setListing({ parent: want, entries: null, truncated: false, error: null });
      reqQuiet('fs.list', { host: w.host, path: want })
        .then((r) => { if (seq.current.fresh(mine)) setListing({ parent: want, entries: r.entries ?? [], truncated: !!r.truncated, error: null }); })
        .catch((e) => { if (seq.current.fresh(mine)) setListing({ parent: want, entries: [], truncated: false, error: e.message }); });
    }, LIST_DEBOUNCE);
    return () => clearTimeout(t);
  }, [want, w.host]);

  const listed = listing?.parent === parent ? listing : null;
  const folders = useMemo(() => (path && listed ? folderRows(listed.entries, fragment, showHidden) : []), [path, listed, fragment, showHidden]);
  const recents = useMemo(() => (path ? [] : recentRows(dirs ?? [], text)), [path, dirs, text]);
  const use = path ? useRow(text, listed) : null;
  // Rows: path mode [use, ...folders], else the recent folders.
  const count = path ? 1 + folders.length : recents.length;
  useEffect(() => { setSel(path ? firstHighlight(fragment, folders) : 0); }, [text, listed, showHidden]);
  useEffect(() => { list.current?.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: 'nearest' }); }, [sel, open]);

  const edit = (cwd) => { set({ cwd, resume: null }); setOpen(true); };
  const pick = (cwd) => { set({ cwd, resume: null }); setOpen(false); onPicked(); };
  // The highlighted row's folder to complete into the field (null on "use").
  const folderAt = (i) => (path ? (i >= 1 ? complete(parent, folders[i - 1]) : null) : recents[i] ? `${recents[i].path.replace(/\/+$/, '')}/` : null);
  const choose = (i) => {
    if (path && i === 0) { if (!use.notFound) pick(use.target); return; }
    if (path) { edit(folderAt(i)); return; }
    if (recents[i]) pick(recents[i].path);
  };
  const atEnd = (e) => e.target.selectionStart === e.target.value.length && e.target.selectionEnd === e.target.value.length;

  const key = (e) => {
    if (!open) {
      if (e.key === 'ArrowDown') { e.preventDefault(); setOpen(true); }
      return; // Enter starts the session (the wizard's handler); Esc closes the dialog.
    }
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      setSel((s) => Math.max(0, Math.min(count - 1, s + (e.key === 'ArrowDown' ? 1 : -1))));
    } else if ((e.key === 'Tab' && !e.shiftKey) || (e.key === 'ArrowRight' && atEnd(e))) {
      const next = folderAt(sel);
      if (next) { e.preventDefault(); edit(next); }
    } else if (e.key === 'Backspace' && atEnd(e)) {
      const up = backspaceUp(text, home);
      if (up !== null) { e.preventDefault(); if (up !== text) edit(up); }
    } else if (e.key === 'Enter') {
      e.preventDefault();
      e.stopPropagation();
      if (count) choose(sel);
    } else if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      setOpen(false);
    }
  };

  const note = listed?.error ? html`<p class="wz-note wz-warn" data-testid="wizard-dir-error" role="alert">${listed.error}</p>`
    : listed?.truncated ? html`<p class="wz-note" data-testid="wizard-dir-error">${CUT}</p>` : null;
  return html`<div class="wz-combo">
    <input id="wz-dir" ref=${input} class="input" data-testid="wizard-dir-input" data-autofocus value=${text} placeholder="~/ or /path/to/project" autocomplete="off" spellcheck=${false}
      role="combobox" aria-expanded=${open} aria-controls="wz-dir-list" aria-autocomplete="list"
      onInput=${(e) => edit(e.target.value)} onKeyDown=${key} onBlur=${() => setOpen(false)} />
    ${open ? html`<ul id="wz-dir-list" class="wz-dir-list" role="listbox" data-testid="wizard-dir-list" ref=${list} onMouseDown=${(e) => e.preventDefault()}>
      ${path ? html`<li role="option" class="wz-dir-row wz-dir-use" data-testid="wizard-dir-use" aria-selected=${sel === 0} aria-disabled=${use.notFound}
          onMouseMove=${() => sel !== 0 && setSel(0)} onClick=${() => choose(0)}>
          <span class="wz-dir-path">▸ use ${use.target}${use.notFound ? ' (not found)' : ''}</span>
        </li>` : null}
      ${path && listed?.entries === null ? html`<li class="wz-note">Reading…</li>` : null}
      ${path ? folders.map((name, k) => html`<li key=${name} role="option" class="wz-dir-row" data-testid="wizard-dir-row" data-path=${`${parent}${name}`} aria-selected=${sel === k + 1}
          onMouseMove=${() => sel !== k + 1 && setSel(k + 1)} onClick=${() => choose(k + 1)}>
          <span class="wz-dir-path">${name}/</span>
        </li>`) : null}
      ${!path && dirs === null ? html`<li class="wz-note">Looking for recent projects…</li>` : null}
      ${!path ? recents.map((d, k) => html`<li key=${d.path} role="option" class="wz-dir-row" data-testid="wizard-dir-row" data-path=${d.path} aria-selected=${sel === k}
          onMouseMove=${() => sel !== k && setSel(k)} onClick=${() => choose(k)}>
          <span class="wz-dir-path">${d.path}</span>
          ${d.git ? html`<span class="wz-dir-branch">⎇ ${d.git}</span>` : null}
          ${d.modified ? html`<span class="wz-dir-age">${ago(d.modified)}</span>` : null}
        </li>`) : null}
    </ul>` : null}
    ${open && path ? html`<div class="wz-dir-tools" onMouseDown=${(e) => e.preventDefault()}>
      ${note ?? html`<span></span>`}
      <label class="wz-check"><input type="checkbox" tabindex="-1" checked=${showHidden} onChange=${(e) => setShowHidden(e.target.checked)} /> hidden</label>
    </div>` : null}
  </div>`;
}
```

New file `addons/web/web/app/ui/wizard-find.js`:

```js
// The wizard's "Find a conversation…" field: the `search` op over every connected host, its hits
// under the field; picking one fills the wizard to resume it (or focuses its live session).
import { html } from 'htm/preact';
import { useEffect, useRef, useState } from 'preact/hooks';
import { reqQuiet } from '../store.js';
import { searchable, searchResult, moveSel } from '../hits.js';
import { abbreviateHome } from '../dirnav.js';
import { ago } from './glyph.js';

// `onPick(hit)`: a hit was chosen. `homes`: known host homes, for `~/`.
export function FindField({ onPick, homes }) {
  const [q, setQ] = useState('');
  const [res, setRes] = useState(null);
  const [busy, setBusy] = useState(false);
  const [sel, setSel] = useState(0);
  const seq = useRef(0);

  function run(query) {
    const mine = ++seq.current;
    if (!searchable(query)) { setRes(null); setBusy(false); return; }
    setBusy(true);
    reqQuiet('search', { query: query.trim() })
      .then((r) => { if (mine === seq.current) { setRes(searchResult(r)); setSel(0); } })
      .catch((e) => { if (mine === seq.current) setRes({ ...searchResult(null), error: e.message }); })
      .finally(() => { if (mine === seq.current) setBusy(false); });
  }
  useEffect(() => { const t = setTimeout(() => run(q), 280); return () => clearTimeout(t); }, [q]);

  const hits = res?.hits ?? [];
  const choose = (h) => { if (!h) return; seq.current += 1; setQ(''); setRes(null); onPick(h); };
  const key = (e) => {
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') { e.preventDefault(); setSel(moveSel(sel, e.key === 'ArrowDown' ? 1 : -1, hits.length)); }
    else if (e.key === 'Enter') { e.preventDefault(); e.stopPropagation(); choose(hits[sel]); }
    else if (e.key === 'Escape' && q) { e.preventDefault(); e.stopPropagation(); seq.current += 1; setQ(''); setRes(null); }
  };
  const state = busy ? 'Searching…' : res?.error ? res.error : res && !hits.length ? 'No conversation matches.' : null;
  return html`<div class="wz-field">
    <label for="wz-find">Find a conversation</label>
    <input id="wz-find" class="input" data-testid="wizard-find" value=${q} placeholder="Find a conversation…" autocomplete="off" spellcheck=${false}
      onInput=${(e) => setQ(e.target.value)} onKeyDown=${key} />
    ${state ? html`<p class=${`wz-note${res?.error ? ' wz-warn' : ''}`} aria-live="polite">${state}</p>` : null}
    ${hits.length ? html`<ul class="search-hits wz-find-hits" role="listbox">
      ${hits.map((h, i) => html`<li key=${`${h.host}:${h.conversation}`} class="search-hit" role="option" data-testid="wizard-find-row" data-conversation=${h.conversation} aria-selected=${i === sel}
        onMouseMove=${() => sel !== i && setSel(i)} onClick=${() => choose(h)}>
        <div class="hit-top"><b class="hit-title">${h.title || '(untitled)'}</b></div>
        <div class="hit-foot"><code>${h.host} · ${abbreviateHome(h.cwd, homes[h.host])} · ${ago(h.modified)}</code></div>
        <div class="hit-snippet">${h.snippet}</div>
      </li>`)}
    </ul>` : null}
    ${(res?.failed ?? []).map((f) => html`<p class="wz-note wz-warn" key=${f.host}>${f.host}: ${f.error}</p>`)}
  </div>`;
}
```

`addons/web/web/app/ui/wizard.js` (the old `Folders` panel goes; the home, the resolved folder, the find field):

```diff
diff --git a/addons/web/web/app/ui/wizard.js b/addons/web/web/app/ui/wizard.js
index aae6c90..dcb7cda 100644
--- a/addons/web/web/app/ui/wizard.js
+++ b/addons/web/web/app/ui/wizard.js
@@ -1,12 +1,14 @@
-// The new-session wizard: one dialog, top to bottom (host, kind, folder, conversation, launch
-// options, name, prompt). A launch an addon wants reviewed first shows that checklist next.
+// The new-session wizard: one dialog, top to bottom (find a conversation, host, kind, folder,
+// conversation, launch options, name, prompt). A launch an addon wants reviewed first shows that checklist next.
 import { html } from 'htm/preact';
 import { signal } from '@preact/signals';
-import { useEffect, useMemo, useRef, useState } from 'preact/hooks';
-import { model, focused, reqQuiet } from '../store.js';
+import { useEffect, useRef, useState } from 'preact/hooks';
+import { model, focused, reqQuiet, conn } from '../store.js';
 import { overlay, defineAction } from '../palette.js';
 import { termSize } from '../term.js';
-import { initialWizard, spawnRequest, filterDirs } from '../wizard.js';
+import { initialWizard, spawnRequest } from '../wizard.js';
+import { startFolder } from '../dirnav.js';
+import { pickHit } from '../hits.js';
 import { optionsByGroup, groupChoices, groupValue, launchFlags, flagsAllowed, reviewLoop } from '../launch.js';
 import { Dialog } from './modal.js';
 import { toast } from './toasts.js';
@@ -15,6 +17,8 @@ import { openSession } from './session-actions.js';
 import { addonDecls, loadAddonDecls } from './panel.js';
 import { openChecklist, checklist as checklistShown } from './checklist.js';
 import { AGENT_HELP } from './hosts.js';
+import { DirField } from './dirfield.js';
+import { FindField } from './wizard-find.js';
 
 export const wizardArgs = signal({});
 
@@ -25,8 +29,6 @@ export function openWizard(opts = {}) {
 }
 
 const abs = (p) => typeof p === 'string' && p.startsWith('/');
-const dirname = (p) => { const t = p.replace(/\/+$/, ''); const i = t.lastIndexOf('/'); return i <= 0 ? '/' : t.slice(0, i); };
-const join = (a, b) => `${a.replace(/\/+$/, '')}/${b}`;
 
 function useDebounced(value, ms) {
   const [v, setV] = useState(value);
@@ -88,66 +90,21 @@ function HostField({ w, set }) {
   </div>`;
 }
 
-function Folders({ w, set }) {
-  const [dirs, setDirs] = useState(null);
-  const [browse, setBrowse] = useState(null); // { path, entries, truncated, error }
-  const [showHidden, setShowHidden] = useState(false);
-  useEffect(() => {
-    let dead = false;
-    setDirs(null);
-    reqQuiet('fs.projects', { host: w.host, limit: 50 }).then((r) => { if (!dead) setDirs(r.dirs ?? []); }).catch(() => { if (!dead) setDirs([]); });
-    return () => { dead = true; };
-  }, [w.host]);
-  const list = useMemo(() => filterDirs(dirs ?? [], w.cwd).slice(0, 8), [dirs, w.cwd]);
-  async function open(path) {
-    setBrowse({ path, entries: null, error: null });
-    try {
-      const r = await reqQuiet('fs.list', { host: w.host, path });
-      setBrowse({ path: r.path ?? path, entries: r.entries ?? [], truncated: !!r.truncated, error: null });
-    } catch (e) {
-      setBrowse({ path, entries: [], error: e.message });
-    }
-  }
-  const home = dirs?.[0] ? dirname(dirs[0].path) : '/';
+// The folder field, "Browse folders…" and why the folder cannot start yet. `folder`:
+// startFolder()'s answer for the field.
+function Folders({ w, set, home, online, folder, onPicked }) {
+  const [browse, setBrowse] = useState(0);
   return html`<div class="wz-field">
     <label for="wz-dir">Folder</label>
-    <input id="wz-dir" class="input" data-testid="wizard-dir-input" data-autofocus value=${w.cwd} placeholder="/path/to/project" autocomplete="off" spellcheck=${false}
-      onInput=${(e) => set({ cwd: e.target.value, resume: null })} />
-    ${w.cwd && !abs(w.cwd) ? html`<p class="wz-note wz-warn">Type an absolute path (it starts with /).</p>` : null}
-    <ul class="wz-dirs" data-testid="wizard-dirs">
-      ${dirs === null ? html`<li class="wz-note">Looking for recent projects…</li>` : null}
-      ${list.map((d) => html`<li key=${d.path}><button type="button" class="wz-dir" data-testid="wizard-dir" data-path=${d.path} aria-pressed=${d.path === w.cwd} onClick=${() => set({ cwd: d.path, resume: null })}>
-        <span class="wz-dir-path">${d.typed ? `Use ${d.path}` : d.path}</span>
-        ${d.git ? html`<span class="wz-dir-branch">⎇ ${d.git}</span>` : null}
-        ${d.modified ? html`<span class="wz-dir-age">${ago(d.modified)}</span>` : null}
-      </button></li>`)}
-    </ul>
-    <button type="button" class="wz-link" data-testid="wizard-browse" aria-expanded=${browse !== null} onClick=${() => (browse ? setBrowse(null) : open(abs(w.cwd) ? w.cwd : home))}>
-      ${browse ? 'Hide folder browser' : 'Browse folders…'}</button>
-    ${browse ? html`<div class="wz-browser" data-testid="wizard-browser">
-      <div class="wz-browser-head">
-        <button type="button" class="btn btn-sm" aria-label="Parent folder" disabled=${browse.path === '/'} onClick=${() => open(dirname(browse.path))}>..</button>
-        <code class="wz-browser-path" title=${browse.path}>${browse.path}</code>
-        <button type="button" class="btn btn-sm" onClick=${() => set({ cwd: browse.path, resume: null })}>Use this folder</button>
-        <label class="wz-check"><input type="checkbox" checked=${showHidden} onChange=${(e) => setShowHidden(e.target.checked)} /> hidden</label>
-      </div>
-      <ul class="wz-entries">
-        ${browse.entries === null ? html`<li class="wz-note">Reading…</li>` : null}
-        ${browse.error ? html`<li class="wz-note wz-warn">${browse.error}</li>` : null}
-        ${(browse.entries ?? []).filter((e) => e.dir && (showHidden || !e.hidden)).map((e) => html`<li key=${e.name}>
-          <button type="button" class="wz-entry" onClick=${() => { set({ cwd: join(browse.path, e.name), resume: null }); open(join(browse.path, e.name)); }}>
-            <span>▸ ${e.name}${e.symlink ? ' →' : ''}</span>
-            ${e.git ? html`<span class="wz-dir-branch">⎇ ${e.git}</span>` : null}
-            ${e.claude_at ? html`<span class="wz-dir-age">claude ${ago(e.claude_at)}</span>` : null}
-          </button></li>`)}
-        ${browse.truncated ? html`<li class="wz-note">The list is cut. Type the path to go further.</li>` : null}
-      </ul>
-    </div>` : null}
+    <${DirField} w=${w} set=${set} home=${home.path} online=${online} browse=${browse} onPicked=${onPicked} />
+    ${folder.why ? html`<p class="wz-note wz-warn">${folder.why}</p>` : null}
+    <button type="button" class="wz-link" data-testid="wizard-browse" onClick=${() => setBrowse((n) => n + 1)}>Browse folders…</button>
   </div>`;
 }
 
-function ResumeField({ w, set }) {
-  const cwd = useDebounced(w.cwd, 300);
+// `cwd`: the folder, absolute (`~` resolved), or '' while there is none.
+function ResumeField({ w, set, cwd: folder }) {
+  const cwd = useDebounced(folder, 300);
   const [list, setList] = useState(null);
   useEffect(() => {
     if (!abs(cwd)) { setList(null); return; }
@@ -211,7 +168,36 @@ export function Wizard() {
   const set = (patch) => setW((cur) => ({ ...cur, ...patch }));
   useEffect(() => { loadAddonDecls(); }, []);
   const shell = w.kind === 'shell';
-  const ready = abs(w.cwd) && !busy && !w.other;
+  // The host's home, for `~` in the folder (the page never sends `~` as a cwd); asked once the
+  // connection is up (a wizard can open before it is).
+  const online = conn.value === 'open';
+  const [home, setHome] = useState({ host: null, path: null, error: null });
+  useEffect(() => {
+    let dead = false;
+    setHome({ host: w.host, path: null, error: null });
+    if (!online) return undefined;
+    reqQuiet('fs.list', { host: w.host, path: '~' })
+      .then((r) => { if (!dead) setHome({ host: w.host, path: r.path ?? null, error: null }); })
+      .catch((e) => { if (!dead) setHome({ host: w.host, path: null, error: e.message }); });
+    return () => { dead = true; };
+  }, [w.host, online]);
+  const folder = startFolder(w.cwd, home.path, home.error);
+  const ready = abs(folder.cwd) && !busy && !w.other;
+  // Focus Start once it can start (it may wait for the home, for a `~` folder).
+  const startBtn = useRef(null);
+  const [toStartAsked, setToStart] = useState(false);
+  useEffect(() => {
+    if (toStartAsked && ready) { startBtn.current?.focus(); setToStart(false); }
+  }, [toStartAsked, ready]);
+  const toStart = () => setToStart(true);
+  // A hit of "Find a conversation…": its live session, or this wizard filled in to resume it.
+  const found = (hit) => {
+    const to = pickHit(model.value.sessions, hit);
+    if (to.focus) { overlay.value = null; openSession(to.focus); return; }
+    const { host, cwd, conversation } = to.resume;
+    set({ host, hosts: [...new Set([...w.hosts, host])], cwd, resume: conversation, kind: 'claude', other: false });
+    toStart();
+  };
 
   async function start() {
     if (!ready || busyRef.current) return;
@@ -219,9 +205,9 @@ export function Wizard() {
     setBusy(true);
     try {
       const size = termSize(focused.value) ?? { rows: 24, cols: 80 };
-      const starting = w.name.trim() || w.cwd.replace(/\/+$/, '').split('/').pop() || 'the session';
+      const starting = w.name.trim() || folder.cwd.replace(/\/+$/, '').split('/').pop() || 'the session';
       const spawn = (defer) => {
-        const { op, args: a } = spawnRequest(w, size, defer);
+        const { op, args: a } = spawnRequest({ ...w, cwd: folder.cwd }, size, defer);
         return reqQuiet(op, a);
       };
       const r = await reviewLoop(spawn, (rv) => openChecklist({ addon: rv.addon, id: rv.id, review: true, starting }));
@@ -248,6 +234,7 @@ export function Wizard() {
 
   return html`<${Dialog} title=${shell ? 'New terminal' : 'New session'} testid="wizard" wide onClose=${() => { overlay.value = null; }}>
     <div class="wizard" onKeyDown=${key}>
+      <${FindField} onPick=${found} homes=${home.path ? { [home.host]: home.path } : {}} />
       <${HostField} w=${w} set=${set} />
       <div class="wz-field">
         <label id="wz-kind-l">What to start</label>
@@ -256,8 +243,8 @@ export function Wizard() {
           <button type="button" class="seg-btn" role="radio" aria-checked=${shell} data-testid="wizard-kind-shell" onClick=${() => set({ kind: 'shell', resume: null })}>Terminal</button>
         </div>
       </div>
-      <${Folders} w=${w} set=${set} />
-      ${shell ? null : html`<${ResumeField} w=${w} set=${set} />
+      <${Folders} w=${w} set=${set} home=${home} online=${online} folder=${folder} onPicked=${toStart} />
+      ${shell ? null : html`<${ResumeField} w=${w} set=${set} cwd=${folder.cwd ?? ''} />
         <${LaunchFields} w=${w} set=${set} />`}
       <div class="wz-field">
         <label for="wz-name">Name <span class="muted">(optional)</span></label>
@@ -270,7 +257,7 @@ export function Wizard() {
     </div>
     <div class="dialog-actions">
       <button type="button" class="btn" onClick=${() => { overlay.value = null; }}>Cancel</button>
-      <button type="button" class="btn btn-primary" data-testid="wizard-start" disabled=${!ready} onClick=${start}>${busy ? 'Starting…' : shell ? 'Open terminal' : 'Start session'}</button>
+      <button type="button" class="btn btn-primary" data-testid="wizard-start" ref=${startBtn} disabled=${!ready} onClick=${start}>${busy ? 'Starting…' : shell ? 'Open terminal' : 'Start session'}</button>
     </div>
   <//>`;
 }
```

`addons/web/web/app/ui/search.js` (the shared hits module):

```diff
diff --git a/addons/web/web/app/ui/search.js b/addons/web/web/app/ui/search.js
index 13f4655..7b87a2e 100644
--- a/addons/web/web/app/ui/search.js
+++ b/addons/web/web/app/ui/search.js
@@ -8,16 +8,17 @@ import { toast } from './toasts.js';
 import { ago } from './glyph.js';
 import { openSession } from './session-actions.js';
 import { openWizard } from './wizard.js';
+import { searchable, searchResult, pickHit, moveSel } from '../hits.js';
 
 export function openSearch() {
   overlay.value = 'search';
 }
 
 // A live session on that conversation gets focus; otherwise the wizard opens filled in to resume it.
-export function openConversation({ host, cwd, conversation }) {
-  const live = Object.values(model.value.sessions).find((s) => s.host === host && s.claude_session_id === conversation && !s.dormant && s.state !== 'exited');
-  if (live) { overlay.value = null; openSession(live.id); return; }
-  openWizard({ host, cwd, resume: conversation, kind: 'claude' });
+export function openConversation(hit) {
+  const to = pickHit(model.value.sessions, hit);
+  if (to.focus) { overlay.value = null; openSession(to.focus); return; }
+  openWizard({ host: to.resume.host, cwd: to.resume.cwd, resume: to.resume.conversation, kind: 'claude' });
 }
 
 export function Search() {
@@ -32,11 +33,11 @@ export function Search() {
   function run(query) {
     const mine = ++seq.current;
     const text = query.trim();
-    if (!text) { setRes(null); setBusy(false); return; }
+    if (!searchable(text)) { setRes(null); setBusy(false); return; }
     setBusy(true);
     reqQuiet('search', { query: text })
-      .then((r) => { if (mine === seq.current) { setRes({ hits: r.hits ?? [], failed: r.failed ?? [], truncated: !!r.truncated }); setSel(0); } })
-      .catch((e) => { if (mine === seq.current) { setRes({ hits: [], failed: [], truncated: false, error: e.message }); } })
+      .then((r) => { if (mine === seq.current) { setRes(searchResult(r)); setSel(0); } })
+      .catch((e) => { if (mine === seq.current) { setRes({ ...searchResult(null), error: e.message }); } })
       .finally(() => { if (mine === seq.current) setBusy(false); });
   }
   useEffect(() => { const t = setTimeout(() => run(q), 280); return () => clearTimeout(t); }, [q]);
@@ -45,8 +46,8 @@ export function Search() {
   const hits = res?.hits ?? [];
   const choose = (h) => { if (h) openConversation(h); };
   const key = (e) => {
-    if (e.key === 'ArrowDown') { e.preventDefault(); setSel((sel + 1) % Math.max(hits.length, 1)); }
-    else if (e.key === 'ArrowUp') { e.preventDefault(); setSel((sel - 1 + hits.length) % Math.max(hits.length, 1)); }
+    if (e.key === 'ArrowDown') { e.preventDefault(); setSel(moveSel(sel, 1, hits.length)); }
+    else if (e.key === 'ArrowUp') { e.preventDefault(); setSel(moveSel(sel, -1, hits.length)); }
     else if (e.key === 'Enter') {
       e.preventDefault();
       if (hits.length && res && q.trim()) choose(hits[sel]);
@@ -57,7 +58,7 @@ export function Search() {
     <input ref=${input} class="input search-input" data-testid="search-input" data-autofocus value=${q} placeholder="Search every host's past conversations" autocomplete="off" spellcheck=${false}
       onInput=${(e) => setQ(e.target.value)} onKeyDown=${key} />
     <div class="search-state" aria-live="polite">
-      ${busy ? 'Searching…' : res?.error ? html`<span class="wz-warn">${res.error}</span>` : res && !hits.length && q.trim() ? 'No conversation matches.' : !q.trim() ? 'Type a word or a phrase. Enter opens the highlighted result.' : ''}
+      ${busy ? 'Searching…' : res?.error ? html`<span class="wz-warn">${res.error}</span>` : res && !hits.length && q.trim() ? 'No conversation matches.' : !q.trim() ? 'Type a word or a phrase (2 characters at least). Enter opens the highlighted result.' : ''}
     </div>
     <ul class="search-hits" ref=${list} role="listbox">
       ${hits.map((h, i) => html`<li key=${`${h.host}:${h.conversation}`} class="search-hit" data-testid="search-hit" role="option" aria-selected=${i === sel}
```

`addons/web/web/app/wizard.js` and its test (`filterDirs` goes: `recentRows` replaces it):

```diff
diff --git a/addons/web/web/app/wizard.js b/addons/web/web/app/wizard.js
index b69ed01..1a00d84 100644
--- a/addons/web/web/app/wizard.js
+++ b/addons/web/web/app/wizard.js
@@ -1,5 +1,5 @@
-// New-session wizard, pure part. Imports only the fuzzy matcher and the launch options.
-import { filter } from './fuzzy.js';
+// New-session wizard, pure part. Imports only the launch options (the folder field's rules are
+// in dirnav.js).
 import { flagsAllowed, selectFrom } from './launch.js';
 
 // `decls`: the addons' declarations (addons.list), for the launch options and flags.
@@ -23,12 +23,3 @@ export function spawnRequest(state, size, defer = []) {
   if (state.prompt && state.prompt.trim() !== '') args.prompt = state.prompt;
   return { op: 'session.spawn', args };
 }
-
-// Fuzzy over the directories' paths; a typed absolute path that is not listed comes first.
-export function filterDirs(dirs, typed) {
-  const q = String(typed ?? '').trim();
-  if (!q) return dirs;
-  const hits = filter(dirs, q, (d) => d.path);
-  if (q.startsWith('/') && !dirs.some((d) => d.path === q)) return [{ path: q, typed: true }, ...hits];
-  return hits;
-}
```

```diff
diff --git a/addons/web/web/tests/wizard.test.mjs b/addons/web/web/tests/wizard.test.mjs
index 269aeb6..7e13aa6 100644
--- a/addons/web/web/tests/wizard.test.mjs
+++ b/addons/web/web/tests/wizard.test.mjs
@@ -1,6 +1,6 @@
 import test from 'node:test';
 import assert from 'node:assert/strict';
-import { initialWizard, spawnRequest, filterDirs } from '../app/wizard.js';
+import { initialWizard, spawnRequest } from '../app/wizard.js';
 
 const decls = [{ name: 'vocdoni', panels: [], launch_flags: [{ name: 'first-party', label: 'First-party session' }],
   launch_options: [{ id: 'work', label: 'work', group: 'Proxy', default: true, detail: null }, { id: 'personal', label: 'personal', group: 'Proxy', default: false, detail: null }] }];
@@ -31,14 +31,3 @@ test('a terminal spawns a shell without launch, resume or prompt', () => {
   assert.equal(r.op, 'session.shell');
   assert.deepEqual(r.args, { host: 'local', cwd: '/tmp/x', rows: 24, cols: 80, name: 'sh' });
 });
-
-test('typed paths filter the recent dirs fuzzily', () => {
-  const dirs = [{ path: '/home/dev/claudio' }, { path: '/home/dev/api' }, { path: '/srv/infra' }];
-  assert.deepEqual(filterDirs(dirs, '').map((d) => d.path), dirs.map((d) => d.path));
-  assert.deepEqual(filterDirs(dirs, 'dvapi').map((d) => d.path), ['/home/dev/api']);
-  const typed = filterDirs(dirs, '/opt/new');
-  assert.equal(typed[0].path, '/opt/new');
-  assert.equal(typed[0].typed, true);
-  assert.equal(filterDirs(dirs, '/srv/infra')[0].path, '/srv/infra');
-  assert.equal(filterDirs(dirs, '/srv/infra').length, 1);
-});
```

`addons/web/web/styles/components.css` (the browser panel's rules go; tokens only):

```diff
diff --git a/addons/web/web/styles/components.css b/addons/web/web/styles/components.css
index dd907a9..cdf5938 100644
--- a/addons/web/web/styles/components.css
+++ b/addons/web/web/styles/components.css
@@ -303,24 +303,25 @@ input[type="radio"] { accent-color: var(--accent); }
 .wz-note { color: var(--text-muted); font-size: var(--fs-sm); margin: 2px 0; line-height: 1.5; }
 .wz-warn { color: var(--state-waiting); }
 .wz-error { margin: 0; padding: 6px 10px; background: var(--bg); border: var(--hair) solid var(--state-error); border-radius: var(--radius); color: var(--state-error); font-size: var(--fs-sm); white-space: pre-wrap; }
-.wz-dirs, .wz-entries, .wz-resume, .snippet-list, .search-hits { list-style: none; margin: 0; padding: 0; }
-.wz-dirs { border: var(--hair) solid var(--border); border-radius: var(--radius); background: var(--bg); }
-.wz-dirs > li + li { border-top: var(--hair) solid var(--border); }
-.wz-dir, .wz-entry {
-  display: flex; align-items: baseline; gap: var(--space-3); width: 100%; text-align: left; min-width: 0;
-  background: none; border: 0; border-left: var(--hair) solid transparent; padding: 5px 10px;
-  font-family: var(--font-mono); color: var(--text);
+.wz-dir-list, .wz-resume, .snippet-list, .search-hits { list-style: none; margin: 0; padding: 0; }
+/* the folder combobox: the field, its listbox, and one line of tools (error or cut, hidden) */
+.wz-combo { display: flex; flex-direction: column; gap: 4px; min-width: 0; }
+.wz-dir-list { max-height: 220px; overflow-y: auto; border: var(--hair) solid var(--border); border-radius: var(--radius); background: var(--bg); }
+.wz-dir-list > li + li { border-top: var(--hair) solid var(--border); }
+.wz-dir-row {
+  display: flex; align-items: baseline; gap: var(--space-3); min-width: 0; padding: 5px 10px; cursor: pointer;
+  border-left: var(--hair) solid transparent; font-family: var(--font-mono); color: var(--text);
 }
-.wz-dir:hover, .wz-entry:hover { background: var(--hover); color: var(--text-strong); }
-.wz-dir[aria-pressed="true"] { background: var(--surface); border-left-color: var(--accent); color: var(--text-strong); }
+.wz-dir-row:hover { background: var(--hover); color: var(--text-strong); }
+.wz-dir-row[aria-selected="true"] { background: var(--surface); border-left-color: var(--accent); color: var(--text-strong); }
+.wz-dir-use { color: var(--text-2); }
+.wz-dir-use[aria-disabled="true"] { color: var(--text-muted); cursor: default; }
 .wz-dir-path { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
 .wz-dir-branch, .wz-dir-age { color: var(--text-muted); font-size: var(--fs-xs); white-space: nowrap; }
+.wz-dir-tools { display: flex; align-items: center; justify-content: space-between; gap: var(--space-3); min-height: 22px; }
+.wz-find-hits { max-height: 200px; border: var(--hair) solid var(--border); border-radius: var(--radius); }
 .wz-link { align-self: flex-start; background: none; border: 0; padding: 0; color: var(--text-2); font-size: var(--fs-sm); text-decoration: none; }
 .wz-link:hover { color: var(--text-strong); }
-.wz-browser { border: var(--hair) solid var(--border); border-radius: var(--radius); background: var(--bg); }
-.wz-browser-head { display: flex; align-items: center; gap: var(--space-2); padding: 6px; border-bottom: var(--hair) solid var(--border); font-family: var(--font-mono); }
-.wz-browser-path { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
-.wz-entries { max-height: 180px; overflow-y: auto; padding: 4px 0; }
 .wz-check { display: flex; align-items: center; gap: var(--space-2); font-size: var(--fs-sm); color: var(--text); }
 .wz-resume { max-height: 170px; overflow-y: auto; border: var(--hair) solid var(--border); border-radius: var(--radius); background: var(--bg); }
 .wz-resume > li + li { border-top: var(--hair) solid var(--border); }
```

`addons/web/web/dev/mock-server.mjs` (`fs.list` answers per path from a small tree, `~` expanded):

```diff
diff --git a/addons/web/web/dev/mock-server.mjs b/addons/web/web/dev/mock-server.mjs
index 06ddf04..f4185d6 100644
--- a/addons/web/web/dev/mock-server.mjs
+++ b/addons/web/web/dev/mock-server.mjs
@@ -245,6 +245,18 @@ function toTerminals(sid, text) {
 
 const BANNER = (s) => `\x1b[2J\x1b[H\x1b[1;36mclaudio mock\x1b[0m  ${s.label} @ ${s.host}\r\n\x1b[2mThis is a fixture terminal: what you type is echoed back.\x1b[0m\r\n\r\n$ `;
 
+// The folders fs.list answers with (a name ending in `/` is a folder); `~` is MOCK_HOME.
+const MOCK_HOME = '/home/dev';
+const MOCK_TREE = {
+  '/': ['home/', 'srv/', 'etc/'],
+  '/home': ['dev/'],
+  '/home/dev': ['api/', 'blog/', 'claudio/', 'docs/', 'Notebooks/', '.config/', '.cache/', 'notes.txt'],
+  '/home/dev/claudio': ['addons/', 'crates/', 'docs/', '.git/', 'Cargo.toml', 'README.md'],
+  '/home/dev/claudio/crates': ['core/', 'tui/'],
+  '/home/dev/api': ['src/', 'package.json'],
+  '/srv': ['infra/'],
+};
+
 // ---- ops ----
 class OpError extends Error {}
 const bad = (m) => new OpError(`bad request: ${m}`);
@@ -531,12 +543,12 @@ const OPS = {
   'fs.list': (a) => {
     host(a);
     const p = need(a, 'path');
-    return { path: p, truncated: false, entries: [
-      { name: 'src', dir: true, git: null, claude_at: null, symlink: false, hidden: false },
-      { name: 'web', dir: true, git: null, claude_at: now() - 86400, symlink: false, hidden: false },
-      { name: '.git', dir: true, git: null, claude_at: null, symlink: false, hidden: true },
-      { name: 'Cargo.toml', dir: false, git: null, claude_at: null, symlink: false, hidden: false },
-    ] };
+    const abs = p === '~' || p.startsWith('~/') ? MOCK_HOME + p.slice(1) : p;
+    const path = abs.replace(/\/+$/, '') || '/';
+    const names = MOCK_TREE[path];
+    if (!names) throw new OpError(`cannot list ${p}: No such file or directory (os error 2)`);
+    const entry = (n) => ({ name: n.replace(/\/$/, ''), dir: n.endsWith('/'), git: n === 'claudio/' ? 'main' : null, claude_at: null, symlink: false, hidden: n.startsWith('.') });
+    return { path, truncated: false, entries: names.map(entry) };
   },
   'fs.projects': (a) => {
     host(a);
```

Run: `node --test addons/web/web/tests/*.test.mjs`
Expected: `# pass 173`, `# fail 0`
Run: `TMPDIR=$HOME/.cache/cl-tmp cargo build --locked && cd addons/web/web/e2e && npx playwright test wizard-nav` (the debug binary embeds the page: build first)
Expected: `7 passed`
Run: `TMPDIR=$HOME/.cache/cl-tmp make test-web`
Expected: `73 passed`
Check: `grep -rn "wizard-dirs\|\"wizard-dir\"\|wizard-browser\|filterDirs" addons/web/web/app addons/web/web/tests addons/web/web/e2e/tests` prints nothing.

Step 3: commit
`git add addons/web/web/app/ui/dirfield.js addons/web/web/app/ui/wizard-find.js addons/web/web/e2e/tests/wizard-nav.spec.mjs && git commit -m "feat(web): navigate folders as you type and find a conversation in the wizard" -- addons/web/web/app/ui/dirfield.js addons/web/web/app/ui/wizard-find.js addons/web/web/app/ui/wizard.js addons/web/web/app/ui/search.js addons/web/web/app/wizard.js addons/web/web/tests/wizard.test.mjs addons/web/web/styles/components.css addons/web/web/dev/mock-server.mjs addons/web/web/e2e/tests/wizard-nav.spec.mjs`

Interfaces this task provides to later tasks: the testids above; `DirField`, `FindField`. Task 8 documents them.

### Task 8: docs
Model: haiku   Repo: /home/p4u/claudio   Files: docs/web-protocol.md, docs/manager-design.md, docs/web.md, README.md, docs/extensions.md, CLAUDE.md
Goal: spec "Constraints / Docs" and ACCEPTANCE F1. `docs/web-protocol.md` gets its own commit (CLAUDE.md: the protocol doc changes in its own commit; this is a clarification, the shape is unchanged). `proto.rs`'s `DirEntry::claude_at` doc was done in Task 1 (`proto/listing.rs`).

Step 1: write the failing test

None (docs). Check before: `grep -n "claude_at" docs/web-protocol.md` shows only the DirEntry shape line.

Step 2: implement

```diff
diff --git a/docs/web-protocol.md b/docs/web-protocol.md
index aec8dea..c613c99 100644
--- a/docs/web-protocol.md
+++ b/docs/web-protocol.md
@@ -263,7 +263,9 @@ browsers.
 
 ### Others
 
-- DirEntry `{"name","dir","git": string|null,"claude_at": n|null,"symlink","hidden"}`;
+- DirEntry `{"name","dir","git": string|null,"claude_at": n|null,"symlink","hidden"}`
+  (`claude_at` is always null today; `git` is filled for folders only; a listing is folders
+  first, by name case-insensitively, and `truncated` says entries were left out);
   ProjectDir `{"path","modified","git","symlink","hidden"}`; ClaudeSession
   `{"id","title","last_prompt","modified","messages"}`; Answer `{"prompt","text"}`;
   GitLogPage, GitCommitInfo, GitPatch, GitWorktreeStatus: exactly the `proto.rs` structs.
```

```diff
diff --git a/docs/manager-design.md b/docs/manager-design.md
index 5e802ad..67b98ce 100644
--- a/docs/manager-design.md
+++ b/docs/manager-design.md
@@ -595,12 +595,27 @@ step.
    - The default is the current session's host.
    - claudio needs only the alias. The system `ssh` resolves User, Port,
      ProxyJump and keys from the user's config.
+   - Two typed characters or more add a row "Search conversations for
+     “<text>”" after the host rows and before "connect to <text>"; it is the
+     default when no host or folder row matches ("connect to" never is while
+     it shows). Enter closes the wizard and searches every connected host whose
+     daemon has the `search` feature (`claudio_core::app::search`, the web's
+     too: 10 s a host, one retry 500 ms after `search busy`, newest first).
+     The hits open in the search list (`<age>  <host>  <~/cwd>  <title>`);
+     Enter focuses the tab that has the conversation, or starts one resuming
+     it in its folder. Hosts that failed are named in one notice.
 2. **Directory:** a fuzzy list seeded from:
    - the current dir
    - recent dirs
    - dirs that have claude history (the `cwd` fields in `~/.claude/projects`)
 
-   You can also type a path with completion through `ListDir`.
+   A typed path (`~/…` or `/…`) browses instead: the list is the parent's
+   subfolders (`ListDir`), → or Tab enters one, ← goes up, "start here" picks
+   the listed folder, Alt+. shows hidden ones. A folder that cannot be listed
+   says why ("cannot list ~/x: Permission denied"). The daemon lists folders
+   first, sorted case-insensitively, before it cuts at 5000 entries or at the
+   reply's frame budget; a cut list says "list cut: type the folder's full
+   name", and a typed folder that is not listed is still taken.
    **[later]** zoxide seeding.
 3. **Resume?** If the dir has claude sessions, the list shows `New session`
    plus each existing session with title, last prompt, age and message count.
```

```diff
diff --git a/docs/web.md b/docs/web.md
index ce6bce7..8ba3155 100644
--- a/docs/web.md
+++ b/docs/web.md
@@ -104,6 +104,21 @@ without one it listens on `0.0.0.0:7337`, IPv4 only. It is the one listener: `--
 (with `--remote`) makes a new certificate. `--set-password` and `--print-cert` take no other
 argument and exit.
 
+### The new-session wizard
+
+`n` (or `Alt+n`) opens it. "Find a conversation…" at the top searches every connected host
+(two characters at least); Enter or a click on a hit fills in its host and folder to resume it,
+or focuses its session when one already has it. The folder field takes a recent folder (typing
+filters them) or a path: `~/` or `/` lists that folder's subfolders as you type, filtered by
+what follows the last `/`. ↓/↑ move, Tab (or → at the end) enters the highlighted folder,
+Backspace after a `/` goes up a level, Enter on a folder enters it and Enter on "▸ use <folder>"
+picks the folder you typed and moves to Start (Enter again starts). Esc closes the list, a
+second Esc the wizard. "Browse folders…" opens the list at `~/`. Hidden folders show with the
+"hidden" box or a name starting with `.`. A folder that cannot be listed says why; a cut list
+(more than 5000 entries) says "list cut: type the folder's full name", and a folder that is not
+listed can still be typed. `~` is resolved with the host's home (read with `fs.list`) before
+anything starts: the page never sends `~` as a folder.
+
 ### Custom env vars
 
 `Alt+,` (or Environment in the palette and in Settings) opens the env vars of the claude
@@ -477,7 +492,8 @@ Rust test in `addons/web/tests/web.rs`; rows without an automated test say how t
 | SSH agent forwarding | `Alt+f` in the wizard's Where step | host panel "SSH agent" row; "Forward my SSH agent" in Add host and the wizard's "Another host…" | `hosts.forward_agent`, Host `forward_agent` | `the host panel switches ssh agent forwarding for a remote host`; `the wizard sends the agent choice before it connects a typed host`; `web_forward_agent_reconnects_a_connected_host_and_keeps_its_sessions`; `web_forwarded_agent_reaches_terminals_through_the_relay` |
 | Terminal panes (1, 2, 4) | `Alt+←/→` | pane grid buttons, `1`-`4`, drag a card | `session.attach`, `session.detach`, `session.resize`, `layout.save` | `a pane shows the terminal and typed keys echo`; `web_attach_snapshot_input_and_resize`; `web_two_tabs_share_one_attach` |
 | Terminal tabs | `Alt+c` | `t`, "Terminal beside" in a pane | `session.shell` | `a terminal tab runs echo ok`; `web_session_lifecycle_shell_rename_kill` |
-| New-session wizard: dirs | `Alt+n` | `n`: folder field, recent folders, browser | `fs.list`, `fs.projects`, `session.spawn` | `the wizard starts a session in a chosen dir` |
+| New-session wizard: dirs | `Alt+n` (typed `~/`, `/` browse) | `n`: the folder combobox (recent folders, typed `~/`, `/` navigation), "Browse folders…" | `fs.list`, `fs.projects`, `session.spawn` | `the wizard starts a session in a chosen dir`; `typing ~/ lists the home folders; typing filters, Tab enters, Backspace goes up`; `Enter on use picks the folder and focuses Start; Enter again starts there` |
+| Wizard: find a conversation | `Alt+n`: "Search conversations for …" row | "Find a conversation…" | `search`, `session.spawn` (`resume`) | `Find a conversation fills the wizard and starts it with resume`; `a hit whose conversation is live focuses that session and closes the wizard` |
 | Wizard: resume | `Alt+n` resume list | "Conversation" field | `claude.sessions`, `session.spawn` (`resume`) | `web_reader_and_search_read_a_fixture_transcript`; `search finds a fixture conversation` |
 | Wizard: launch options (proxy profile) | wizard proxy choice | one select per launch option group ("Proxy") | `addons.list`, `session.spawn` (`launch`) | `web_spawn_keeps_proxy_and_plugin_secrets_out_of_the_browser`; `the wizard starts a session in a chosen dir` |
 | Wizard: launch flags (first-party) | wizard checkbox | one checkbox per labelled flag ("First-party session") | `session.spawn` (`launch.flags`) | `web_addons_ops_refuse_unknown_addons_and_bad_args` (the rule); the UI box: manual check |
```

```diff
diff --git a/README.md b/README.md
index b9d5b4e..2f1b7d4 100644
--- a/README.md
+++ b/README.md
@@ -49,7 +49,7 @@ It prints `claudio web: http://127.0.0.1:7337/#t=<token>` and exits: the daemon
 
 ![The new-session wizard](docs/screenshots/web-wizard.png)
 
-*New session (`n` or `Alt+n`): pick a host and a folder, then start a new conversation or resume one from that folder. A terminal tab starts from the same dialog.*
+*New session (`n` or `Alt+n`): pick a host and a folder, then start a new conversation or resume one from that folder. Typing `~/` or `/` in the folder field walks the host's folders (Tab enters one, Backspace goes up), and "Find a conversation…" searches every connected host when you do not know the folder. A terminal tab starts from the same dialog.*
 
 Also: one, two or four panes, terminal tabs, search across every conversation (`/`), a command palette (`k`), desktop notifications, and the addons' panels (proxy stats, `Alt+s`).
```

````diff
diff --git a/docs/extensions.md b/docs/extensions.md
index ae4a2fb..69af22b 100644
--- a/docs/extensions.md
+++ b/docs/extensions.md
@@ -17,7 +17,7 @@ renders). You can add your own as a small program in any language.
 |---|---|---|---|
 | `latex` (web addon) | tag `<latex>` | — | Math typeset with KaTeX in the web UI's reader. |
 | `diagram` (web addon) | fence ```` ```mermaid ```` | — | Mermaid diagrams drawn in the web UI's reader. |
-| `search` | — | `Alt+z s` | Search the claude conversations on the active session's host; Enter resumes one. |
+| `search` | — | `Alt+z s` | Search the claude conversations on the active session's host; Enter resumes one. The wizard (`Alt+n`, the web's `n`) also searches every connected host: its "Search conversations" row, the web's "Find a conversation…". |
 | `snippets` | — | `Alt+z p`, `Alt+z P` | Paste a saved prompt; save the last prompt as one. |
 
 `Alt+z` opens the leader popup: one line per action. Press the action's key to
````

```diff
diff --git a/CLAUDE.md b/CLAUDE.md
index c20eb55..a695396 100644
--- a/CLAUDE.md
+++ b/CLAUDE.md
@@ -183,7 +183,7 @@ declares. The services addons use today: `paths` (all); `term::{probe, strip_esc
 (api); `proto`, `text`, `client`, `remote::bootstrap`, `term::secret` and
 `app::{addons, launch, browser}` (web and vocdoni); `claude::SESSION_MARKERS` and
 `app::config_edit` (vocdoni, web; and `daemon::test_support` in the web's tests); `config`, `freshness`, `claude::{hooks, update}`,
-`remote::{hosts, validate_host}`, `sys::hostname`, `app::label`, `app::env` and `claude::env` (web). The web addon (the former in-tree web UI, a second front-end) also uses
+`remote::{hosts, validate_host}`, `sys::hostname`, `app::label`, `app::env`, `app::search` and `claude::env` (web). The web addon (the former in-tree web UI, a second front-end) also uses
 `app::{state, connections, extensions, snippets, env}`: the layer both front-ends share (A5).
 
 ### Session manager (bare `claudio`)
@@ -242,7 +242,8 @@ flowchart LR
   `launch/` (the launch composer), `config_edit.rs` (every `config.toml` writer: a minimal,
   atomic section edit under one process-wide lock), `state.rs` (`state.json`),
   `connections.rs`, `extensions/` (the registry, external manifests in
-  `~/.config/claudio/extensions/`, the extension host), `snippets.rs`, `env.rs` (the custom env:
+  `~/.config/claudio/extensions/`, the extension host), `snippets.rs`, `search.rs` (the
+  conversation search over every connected host, both front-ends'), `env.rs` (the custom env:
   `config.toml [env]` and `session-env.json`, both front-ends' and the composer's).
 - **Launch composer** (`app/launch/`): the one path of a claude session's `Spawn`/`Respawn`,
   from the TUI, `--plain` and the web. Resolve (every `Launch` addon's `Routing`; a `Required`
```

Run: `git diff --stat`
Expected: `6 files changed, 41 insertions(+), 7 deletions(-)`

Step 3: commit
`git commit -m "docs(web-protocol): claude_at is always null, listings are folders first" -- docs/web-protocol.md`
`git commit -m "docs: the wizard's folder navigation and conversation search" -- docs/manager-design.md docs/web.md README.md docs/extensions.md CLAUDE.md`

Interfaces this task provides to later tasks: none.


## Merge and release

1. Full suites on `feat/wizard-nav` (all with `TMPDIR=$HOME/.cache/cl-tmp`):
   - `make check` → ends `rules: 0 problem(s); …` and the unit tests green (the workspace, then each crate alone).
   - `make matrix` → every cell (core only, each addon alone, all) builds and passes clippy.
   - `cargo test --test manager -- --test-threads=1` → `test result: ok. 50 passed`; `cargo test --no-default-features --test manager -- --test-threads=1` → `test result: ok. 50 passed`.
   - `cargo test --test web --test web_manager -- --test-threads=1` → `test result: ok. 66 passed` (web) and `test result: ok. 4 passed` (web_manager); `cargo test --test vocdoni_web -- --test-threads=1` → `test result: ok. 14 passed`; `cargo test --test vocdoni_manager -- --test-threads=1` → `test result: ok. 19 passed`.
   - `node --test addons/web/web/tests/*.test.mjs` → `# pass 173`, `# fail 0`.
   - `make test-web` → `73 passed`.
   Verified in the scratch worktree with all eight tasks applied: every gate above passed
   (`make check` ended `rules: 0 problem(s)` and exit 0; `make matrix` exit 0; `make test-web` was
   run as its three steps, the node tests, `cargo build --locked`, `npx playwright test` with
   `CLAUDIO_BIN` pointing at that build and the main checkout's `node_modules`: `73 passed`). Not
   run: the `CLAUDIO_E2E=1` and `CLAUDIO_PROXY_TEST=1` suites (real claude, real proxy; untouched
   code paths), `tests/remote.rs` against a real ssh host, and CI on macOS (the controller's
   post-release check covers it). `dev/mock-server.mjs`'s `fs.list` was checked over its
   WebSocket (`~`, `~/claudio/`, an absolute path, a missing one).
2. Merge order: one repo. The controller merges `feat/wizard-nav` into `main` with `git merge --no-ff feat/wizard-nav` (message `merge: wizard folder navigation and conversation search`).
3. Release v0.9.11 (v0.10.0 is reserved): bump `version` in the root `Cargo.toml` to `0.9.11`, `cargo update -w`, commit `chore(release): v0.9.11` (`-- Cargo.toml Cargo.lock`), annotated tag `git tag -a v0.9.11 -m "v0.9.11"`; release notes through `gh` per the repo's release procedure (the tag triggers the release workflow).
4. **Owner OK required before:** none beyond the standing approval: the owner pre-approved merging and releasing claudio. The controller (not an implementer) pushes `main` and the tag, over HTTPS with gh's credentials.
5. Post-release checks: both CI runs (the `main` push and the `v0.9.11` tag) green on Linux and macOS; the release has its assets (the static binaries per OS/arch the workflow builds); `claudio --version` of a downloaded asset prints 0.9.11.

