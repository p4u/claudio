//! Facts about the host the daemon runs on, and its filesystem RPCs.
//!
//! [`probe`] gathers the [`HostInfo`] sent in `Welcome` (it runs
//! `claude --version`, so it is computed once and cached by the caller).
//! [`list_dir`] backs the new-session directory picker.

use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::addon::ClaudeFlags;
use crate::proto::{ClaudeInfo, DirEntry, HostInfo};

/// How long `claude --version` may take before we report claude as missing.
const VERSION_TIMEOUT: Duration = Duration::from_secs(3);

/// Maximum entries returned by [`list_dir`].
const MAX_DIR_ENTRIES: usize = 5000;

/// Maximum entries [`list_dir`] reads before it sorts and cuts.
const MAX_SCAN: usize = 50_000;

/// The current user's home directory.
pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// Expand a leading `~` (alone or as `~/…`) to the home directory.
pub fn expand_tilde(path: &str) -> PathBuf {
    match path.strip_prefix('~') {
        Some("") => home(),
        Some(rest) if rest.starts_with('/') => home().join(rest.trim_start_matches('/')),
        _ => PathBuf::from(path),
    }
}

/// Gather host facts. Blocking: runs `<claude> --version` (≤ 3 s), with
/// `PATH` set to `path` when given (the daemon's login-shell `PATH`).
pub fn probe(claude: &Path, path: Option<&OsStr>) -> HostInfo {
    HostInfo {
        claude: claude_info(claude, path),
        ..basic()
    }
}

/// Host facts that need no subprocess (`claude` reported as missing).
pub fn basic() -> HostInfo {
    HostInfo {
        hostname: crate::sys::hostname(),
        os: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        home: home().to_string_lossy().into_owned(),
        claude: None,
    }
}

/// `Some` when `claude` resolves to a file and `--version` answers in time.
fn claude_info(claude: &Path, path: Option<&OsStr>) -> Option<ClaudeInfo> {
    let bin = which(claude)?;
    let version = run_version(&bin, path)?;
    Some(ClaudeInfo {
        path: bin.to_string_lossy().into_owned(),
        version,
    })
}

/// Resolve a bare command name through `$PATH`; paths are taken as they are.
fn which(cmd: &Path) -> Option<PathBuf> {
    if cmd.components().count() > 1 {
        return cmd.is_file().then(|| cmd.to_path_buf());
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(cmd))
        .find(|p| p.is_file())
}

/// First line of `<claude> --version`, or `None` on failure or timeout.
fn run_version(claude: &Path, path: Option<&OsStr>) -> Option<String> {
    run_capture(claude, "--version", path)?
        .lines()
        .next()
        .map(|l| l.trim().to_owned())
        .filter(|l| !l.is_empty())
}

/// Lets claude offer bypass-permissions mode (Shift+Tab); it does not start
/// in it.
pub const ALLOW_SKIP_PERMISSIONS: &str = "--allow-dangerously-skip-permissions";

/// Loads a plugin for one session, uninstalled (a directory or a zip).
pub const PLUGIN_DIR: &str = "--plugin-dir";

/// Defines subagents for one session (a JSON object).
pub const AGENTS: &str = "--agents";

/// Loads MCP servers for one session (JSON files or strings; variadic).
pub const MCP_CONFIG: &str = "--mcp-config";

/// compat(v0.10): the flags the daemon treats as optional although the
/// spawn's `optional` does not name them, as the previous release did: an
/// older client marks none, and its sessions still start on a claude
/// without them.
pub const LEGACY_OPTIONAL: [&str; 3] = [PLUGIN_DIR, AGENTS, MCP_CONFIG];

/// [`ClaudeFlags`] of `<claude> --help`, or `None` when it did not run
/// (missing, failing or over 3 s), so a failed probe is not remembered.
/// Blocking (≤ 3 s).
pub fn claude_flags(claude: &Path, path: Option<&OsStr>) -> Option<ClaudeFlags> {
    let help = run_capture(claude, "--help", path)?;
    Some(ClaudeFlags::from_help(&help))
}

/// Spawn `cmd`, retrying briefly on `ETXTBSY` ("Text file busy"). Linux
/// refuses to exec a file some process still has open for writing, and a
/// `fork` elsewhere in this process hands its child a copy of that descriptor
/// until the child execs. So a claude that was just written (an update, the
/// installer) can fail to start for a few milliseconds although it is fine;
/// without the retry the probe would report it missing.
fn spawn_when_not_busy(cmd: &mut Command) -> io::Result<std::process::Child> {
    const TRIES: u32 = 20;
    let mut tries = 0;
    loop {
        match cmd.spawn() {
            Err(e) if e.raw_os_error() == Some(libc::ETXTBSY) && tries < TRIES => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(25));
            }
            other => return other,
        }
    }
}

/// Write a fake `claude` script and return once it can be executed. A `fork`
/// on another test thread may hold the write descriptor for a moment, and
/// until its child execs, `exec` of the file fails with `ETXTBSY`; a test that
/// ran the script first would fail at random. One `--version` run (fake
/// claudes answer it) shows the file is free: the descriptor is then gone for
/// good.
#[cfg(any(test, feature = "test-support"))]
pub fn write_executable(path: &Path, script: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, script).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut cmd = Command::new(path);
    cmd.arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    spawn_when_not_busy(&mut cmd).unwrap().wait().unwrap();
}

/// Stdout of `<claude> <arg>` when it exits 0 within [`VERSION_TIMEOUT`].
/// Stdout is drained on a thread so long output can't fill the pipe and stall
/// the child.
fn run_capture(claude: &Path, arg: &str, path: Option<&OsStr>) -> Option<String> {
    let mut cmd = Command::new(claude);
    if let Some(path) = path {
        cmd.env("PATH", path);
    }
    cmd.arg(arg)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = spawn_when_not_busy(&mut cmd).ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        io::Read::read_to_string(&mut stdout, &mut out)
            .ok()
            .map(|_| out)
    });
    let deadline = Instant::now() + VERSION_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    reader.join().ok().flatten()
}

/// Detect the git branch for a directory without running `git`.
///
/// Returns `Some(branch)` when the dir contains `.git/HEAD` with a branch ref,
/// `Some("")` for a detached HEAD or an unreadable HEAD, and `None` when the
/// directory is not a git repo. Worktrees (`.git` is a file) are followed one
/// level by parsing the `gitdir:` pointer.
pub fn detect_git_branch(path: &Path) -> Option<String> {
    let git = path.join(".git");
    let head_path = if git.is_dir() {
        git.join("HEAD")
    } else if git.is_file() {
        // Worktree: .git is a file containing "gitdir: <path>"
        let content = std::fs::read_to_string(&git).ok()?;
        let gitdir = content
            .lines()
            .next()
            .and_then(|l| l.strip_prefix("gitdir:"))?
            .trim();
        Path::new(gitdir).join("HEAD")
    } else {
        return None;
    };
    let head = std::fs::read_to_string(&head_path).ok()?;
    let head = head.trim();
    if let Some(branch) = head.strip_prefix("ref: refs/heads/") {
        Some(branch.to_owned())
    } else {
        // Detached HEAD or unknown format.
        Some(String::new())
    }
}

/// List `path` (with `~` expanded): folders first, then files, each by name
/// case-insensitively, at most [`MAX_DIR_ENTRIES`]. Returns the expanded
/// path, the entries, and whether any were left out.
pub fn list_dir(path: &str) -> io::Result<(String, Vec<DirEntry>, bool)> {
    list_dir_with(path, detect_git_branch)
}

/// [`list_dir`] with `git` as the branch probe (a test counts its calls).
fn list_dir_with(
    path: &str,
    git: impl Fn(&Path) -> Option<String>,
) -> io::Result<(String, Vec<DirEntry>, bool)> {
    let dir = expand_tilde(path);
    let (mut scanned, mut truncated) = scan(&dir)?;
    scanned.sort_by(|a, b| {
        b.dir
            .cmp(&a.dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    truncated |= scanned.len() > MAX_DIR_ENTRIES;
    scanned.truncate(MAX_DIR_ENTRIES);
    // Only the kept entries are enriched, and only folders get a git probe.
    let entries = scanned
        .into_iter()
        .map(|s| DirEntry {
            git: s.dir.then(|| git(&dir.join(&s.name))).flatten(),
            hidden: s.name.starts_with('.'),
            symlink: s.symlink,
            dir: s.dir,
            claude_at: None,
            name: s.name,
        })
        .collect();
    Ok((dir.to_string_lossy().into_owned(), entries, truncated))
}

/// One entry as read: its name and kind, nothing more.
struct Scanned {
    name: String,
    dir: bool,
    symlink: bool,
}

/// Read at most [`MAX_SCAN`] entries of `dir`, the kind from the directory
/// entry itself (no extra syscall on Linux and macOS); a symlink costs one
/// `metadata` call to learn whether it leads to a folder. `true` when the
/// folder held more.
fn scan(dir: &Path) -> io::Result<(Vec<Scanned>, bool)> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)?.filter_map(Result::ok) {
        if out.len() == MAX_SCAN {
            return Ok((out, true));
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let symlink = kind.is_symlink();
        let dir = match symlink {
            true => entry.path().metadata().is_ok_and(|m| m.is_dir()),
            false => kind.is_dir(),
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        out.push(Scanned { name, dir, symlink });
    }
    Ok((out, false))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder under the temp dir.
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("claudio-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

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

    #[test]
    fn tilde_expansion() {
        let h = home();
        assert_eq!(expand_tilde("~"), h);
        assert_eq!(expand_tilde("~/src/x"), h.join("src/x"));
        assert_eq!(expand_tilde("/abs/~"), PathBuf::from("/abs/~"));
        assert_eq!(expand_tilde("~other"), PathBuf::from("~other"));
    }

    #[test]
    fn probe_reports_host_and_missing_claude() {
        let info = probe(Path::new("/nonexistent/claude"), None);
        assert_eq!(info.os, std::env::consts::OS);
        assert!(!info.hostname.is_empty());
        assert!(info.claude.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn claude_flags_read_long_help() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("claudio-help-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("claude");
        // Long help output: more than a pipe buffer, so draining must not stall.
        std::fs::write(
            &bin,
            "#!/bin/sh\nseq 1 20000\necho '  --allow-dangerously-skip-permissions  Enable bypass'\n",
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let flags = claude_flags(&bin, None).unwrap();
        assert!(flags.skip_permissions);
        assert!(!flags.knows(PLUGIN_DIR));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn claude_flags_come_from_one_help_run() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("claudio-flags-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("claude");
        let help = |text: &str| {
            let script = format!("#!/bin/sh\n[ \"$1\" = --help ] && printf '%s\\n' '{text}'\n");
            std::fs::write(&bin, script).unwrap();
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
            claude_flags(&bin, None).unwrap()
        };
        let both = help("  --plugin-dir <paths...>  --allow-dangerously-skip-permissions");
        assert!(both.skip_permissions && both.knows(PLUGIN_DIR));
        assert!(!both.knows(AGENTS) && !both.knows(MCP_CONFIG));
        let plugins = help("  --plugin-dir <paths...>");
        assert!(plugins.knows(PLUGIN_DIR) && !plugins.skip_permissions);
        assert_eq!(help(""), ClaudeFlags::default());
        // Listed as options, not merely named in another one's description.
        let current = help(
            "  --agents <json-or-file>  Agents\n  --mcp-config <configs...>  MCP\n               --bare  Skips --plugin-dir, --strict-mcp-config",
        );
        assert!(current.knows(AGENTS) && current.knows(MCP_CONFIG));
        assert!(!current.knows(PLUGIN_DIR));
        let named = help("  --bare  Skips --mcp-config, --agents\n  --strict-mcp-config  Only");
        assert!(
            !named.knows(AGENTS) && !named.knows(MCP_CONFIG),
            "{named:?}"
        );
        assert_eq!(claude_flags(Path::new("/nonexistent/claude"), None), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn probe_runs_claude_with_the_given_path() {
        let dir = std::env::temp_dir().join(format!("claudio-probe-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("claude");
        // An npm-style claude needs its interpreter from the user's PATH.
        write_executable(&bin, "#!/bin/sh\necho \"$PATH\"\n");
        let info = probe(&bin, Some(OsStr::new("/login/bin:/usr/bin:/bin")));
        let claude = info.claude.unwrap();
        assert_eq!(claude.version, "/login/bin:/usr/bin:/bin");
        assert_eq!(claude.path, bin.to_string_lossy());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
