//! Facts about the host the daemon runs on, and its filesystem RPCs.
//!
//! [`probe`] gathers the [`HostInfo`] sent in `Welcome` (it runs
//! `claude --version`, so it is computed once and cached by the caller).
//! [`list_dir`] backs the new-session directory picker.

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

/// Gather host facts. Blocking: runs `<claude> --version` (≤ 3 s).
pub fn probe(claude: &Path) -> HostInfo {
    HostInfo {
        claude: claude_info(claude),
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
fn claude_info(claude: &Path) -> Option<ClaudeInfo> {
    let path = which(claude)?;
    let version = run_version(&path)?;
    Some(ClaudeInfo {
        path: path.to_string_lossy().into_owned(),
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
fn run_version(claude: &Path) -> Option<String> {
    let mut child = Command::new(claude)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
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
    let mut out = String::new();
    io::Read::read_to_string(&mut child.stdout.take()?, &mut out).ok()?;
    out.lines()
        .next()
        .map(|l| l.trim().to_owned())
        .filter(|l| !l.is_empty())
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
        let info = probe(Path::new("/nonexistent/claude"));
        assert_eq!(info.os, std::env::consts::OS);
        assert!(!info.hostname.is_empty());
        assert!(info.claude.is_none());
    }
}
