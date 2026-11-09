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

use crate::proto::{ClaudeInfo, DirEntry, HostInfo};

/// How long `claude --version` may take before we report claude as missing.
const VERSION_TIMEOUT: Duration = Duration::from_secs(3);

/// Maximum entries returned by [`list_dir`].
const MAX_DIR_ENTRIES: usize = 5000;

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
        hostname: hostname(),
        os: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        home: home().to_string_lossy().into_owned(),
        claude: None,
    }
}

fn hostname() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: the pointer and length describe a valid, writable buffer.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    if rc != 0 {
        return String::new();
    }
    let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..len]).into_owned()
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

/// The optional flags this claude knows, from one `--help` run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClaudeFlags {
    /// [`ALLOW_SKIP_PERMISSIONS`].
    pub skip_permissions: bool,
    /// [`PLUGIN_DIR`].
    pub plugin_dir: bool,
    /// [`AGENTS`].
    pub agents: bool,
    /// [`MCP_CONFIG`].
    pub mcp_config: bool,
}

impl ClaudeFlags {
    /// The flags that take one value and that a session's args may carry
    /// although this claude lacks them, each with whether it knows it.
    pub fn optional(&self) -> [(&'static str, bool); 3] {
        [
            (PLUGIN_DIR, self.plugin_dir),
            (AGENTS, self.agents),
            (MCP_CONFIG, self.mcp_config),
        ]
    }

    /// The plugin flags this claude lacks (`--mcp-config`, `--agents`):
    /// what a plugin's sessions here go without.
    pub fn dropped(&self) -> Vec<String> {
        [(MCP_CONFIG, self.mcp_config), (AGENTS, self.agents)]
            .into_iter()
            .filter(|(_, known)| !known)
            .map(|(flag, _)| flag.to_owned())
            .collect()
    }
}

/// Whether `help` lists `flag` as an option: a line that starts with it
/// (another option's description may name it too).
fn lists(help: &str, flag: &str) -> bool {
    help.lines().any(|line| {
        line.trim_start()
            .strip_prefix(flag)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', '=', ',']))
    })
}

/// [`ClaudeFlags`] of `<claude> --help`, or `None` when it did not run
/// (missing, failing or over 3 s), so a failed probe is not remembered.
/// Blocking (≤ 3 s).
pub fn claude_flags(claude: &Path, path: Option<&OsStr>) -> Option<ClaudeFlags> {
    let help = run_capture(claude, "--help", path)?;
    Some(ClaudeFlags {
        skip_permissions: help.contains(ALLOW_SKIP_PERMISSIONS),
        plugin_dir: lists(&help, PLUGIN_DIR),
        agents: lists(&help, AGENTS),
        mcp_config: lists(&help, MCP_CONFIG),
    })
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
#[cfg(test)]
pub(crate) fn write_executable(path: &Path, script: &str) {
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

/// List `path` (with `~` expanded): directories first, then by name, capped
/// at 5000 entries. Returns the expanded path alongside the entries.
pub fn list_dir(path: &str) -> io::Result<(String, Vec<DirEntry>)> {
    let dir = expand_tilde(path);
    let mut entries: Vec<DirEntry> = std::fs::read_dir(&dir)?
        .filter_map(Result::ok)
        .take(MAX_DIR_ENTRIES)
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let path = e.path();
            let hidden = name.starts_with('.');
            // Detect symlinks without following them (symlink_metadata vs metadata).
            let symlink = e
                .metadata()
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false);
            // Follow symlinks so a link to a directory can be descended into.
            let dir_flag = path.is_dir();
            DirEntry {
                name,
                dir: dir_flag,
                git: detect_git_branch(&path),
                claude_at: None, // enriched below for dirs only
                symlink,
                hidden,
            }
        })
        .collect();
    entries.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.cmp(&b.name)));
    Ok((dir.to_string_lossy().into_owned(), entries))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(!flags.plugin_dir);
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
        assert_eq!(
            both,
            ClaudeFlags {
                skip_permissions: true,
                plugin_dir: true,
                ..ClaudeFlags::default()
            }
        );
        let plugins = help("  --plugin-dir <paths...>");
        assert!(plugins.plugin_dir && !plugins.skip_permissions);
        assert_eq!(help(""), ClaudeFlags::default());
        assert_eq!(help("").dropped(), ["--mcp-config", "--agents"]);
        // Listed as options, not merely named in another one's description.
        let current = help(
            "  --agents <json-or-file>  Agents\n  --mcp-config <configs...>  MCP\n               --bare  Skips --plugin-dir, --strict-mcp-config",
        );
        assert!(current.agents && current.mcp_config && !current.plugin_dir);
        assert!(current.dropped().is_empty());
        let named = help("  --bare  Skips --mcp-config, --agents\n  --strict-mcp-config  Only");
        assert!(!named.agents && !named.mcp_config, "{named:?}");
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
