//! The working-tree summary of a session's directory for the status bar:
//! branch, ahead/behind its upstream, and how many paths are staged,
//! modified, untracked or unmerged.
//!
//! One `git status --porcelain=v2 --branch` per refresh, with the same
//! scrubbed environment as the git viewer, `GIT_OPTIONAL_LOCKS=0` so it never
//! writes the index, a short deadline and a capped read. A repo that takes
//! longer than the deadline simply has no status this time.

use std::path::Path;
use std::time::Duration;

use tokio::io::AsyncReadExt;

use crate::proto::GitStatus;

/// How long one status run may take before it is abandoned.
const DEADLINE: Duration = Duration::from_secs(1);
/// At most this much of git's output is read; a repo with more changed paths
/// than fit is reported with the count seen so far.
const OUTPUT_CAP: usize = 256 * 1024;
/// Short commit id for a detached HEAD.
const SHORT_OID: usize = 7;

/// The status of the repository containing `cwd`; `None` when it is not a
/// repository, git is missing, or the run exceeded the deadline.
pub async fn read(cwd: &Path) -> Option<GitStatus> {
    let work = async {
        let mut child = super::git::command(cwd)
            .args([
                "status",
                "--porcelain=v2",
                "--branch",
                "--no-renames",
                "--untracked-files=normal",
            ])
            .spawn()
            .ok()?;
        let mut stdout = child.stdout.take()?;
        let mut out = Vec::new();
        let _ = (&mut stdout)
            .take(OUTPUT_CAP as u64)
            .read_to_end(&mut out)
            .await;
        // Past the cap, git may be blocked on a full pipe: stop waiting for it.
        if out.len() >= OUTPUT_CAP {
            let _ = child.start_kill();
        }
        let status = child.wait().await.ok()?;
        (status.success() || out.len() >= OUTPUT_CAP).then(|| parse(&out))
    };
    tokio::time::timeout(DEADLINE, work).await.ok().flatten()
}

/// Parse porcelain v2 output (newline-terminated records; paths with special
/// characters are quoted on one line, so every record is one line).
pub fn parse(output: &[u8]) -> GitStatus {
    let mut status = GitStatus::default();
    let mut oid = String::new();
    for line in String::from_utf8_lossy(output).lines() {
        if let Some(header) = line.strip_prefix("# ") {
            let (key, value) = header.split_once(' ').unwrap_or((header, ""));
            match key {
                "branch.oid" => oid = value.to_owned(),
                "branch.head" => {
                    if value == "(detached)" {
                        status.detached = true;
                    } else {
                        status.head = value.to_owned();
                    }
                }
                "branch.upstream" => status.upstream = true,
                "branch.ab" => {
                    for part in value.split(' ') {
                        if let Some(n) = part.strip_prefix('+') {
                            status.ahead = n.parse().unwrap_or(0);
                        } else if let Some(n) = part.strip_prefix('-') {
                            status.behind = n.parse().unwrap_or(0);
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        let mut fields = line.splitn(3, ' ');
        let (kind, xy) = (fields.next().unwrap_or(""), fields.next().unwrap_or(""));
        match kind {
            "1" | "2" => {
                let mut xy = xy.chars();
                let (x, y) = (xy.next().unwrap_or('.'), xy.next().unwrap_or('.'));
                if x != '.' {
                    status.staged = status.staged.saturating_add(1);
                }
                if y != '.' {
                    status.unstaged = status.unstaged.saturating_add(1);
                }
            }
            "u" => status.conflicts = status.conflicts.saturating_add(1),
            "?" => status.untracked = status.untracked.saturating_add(1),
            _ => {}
        }
    }
    if status.detached && status.head.is_empty() {
        status.head = oid.chars().take(SHORT_OID).collect();
    }
    status
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    #[test]
    fn branch_with_upstream_ahead_behind_and_dirty_paths() {
        let out = b"# branch.oid 0123456789abcdef\n\
                    # branch.head main\n\
                    # branch.upstream origin/main\n\
                    # branch.ab +2 -1\n\
                    1 M. N... 100644 100644 100644 aaa bbb staged.rs\n\
                    1 .M N... 100644 100644 100644 aaa aaa modified.rs\n\
                    1 MM N... 100644 100644 100644 aaa bbb both.rs\n\
                    1 A. N... 000000 100644 100644 000 ccc \"new file.rs\"\n\
                    2 R. N... 100644 100644 100644 aaa aaa R100 new.rs\told.rs\n\
                    u UU N... 100644 100644 100644 100644 a b c conflict.rs\n\
                    ? untracked.txt\n\
                    ? another.txt\n\
                    ! ignored.o\n";
        let s = parse(out);
        assert_eq!(s.head, "main");
        assert_eq!(s.branch(), Some("main"));
        assert!(!s.detached);
        assert!(s.upstream);
        assert_eq!((s.ahead, s.behind), (2, 1));
        assert_eq!((s.staged, s.unstaged), (4, 2));
        assert_eq!((s.untracked, s.conflicts), (2, 1));
        assert!(s.is_dirty());
    }

    #[test]
    fn branch_without_upstream_has_no_ahead_behind() {
        let s = parse(b"# branch.oid abc\n# branch.head feature\n");
        assert_eq!(s.branch(), Some("feature"));
        assert!(!s.upstream);
        assert_eq!((s.ahead, s.behind), (0, 0));
        assert!(!s.is_dirty());
    }

    #[test]
    fn detached_head_is_the_short_commit_id() {
        let s = parse(b"# branch.oid 9f8e7d6c5b4a3210\n# branch.head (detached)\n? x\n");
        assert!(s.detached);
        assert_eq!(s.head, "9f8e7d6");
        assert_eq!(s.branch(), None);
        assert_eq!(s.untracked, 1);
    }

    #[test]
    fn unborn_branch_and_empty_output() {
        let s = parse(b"# branch.oid (initial)\n# branch.head main\n");
        assert_eq!(s.branch(), Some("main"));
        assert_eq!(parse(b""), GitStatus::default());
        // Garbage never panics.
        let _ = parse(b"1\n2 \nu\n# \n# branch.ab +x -y\n\xff\xfe");
    }

    fn git(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .args(args)
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A repo on `main` tracking a local branch `up`: one commit ahead, one
    /// behind, one modified tracked file and one untracked file.
    pub(in crate::daemon) fn temp_repo() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("claudio-gitstatus-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("a.txt"), "a\n").unwrap();
        git(&dir, &["add", "a.txt"]);
        git(&dir, &["commit", "-q", "-m", "base"]);
        git(&dir, &["branch", "up"]);
        git(&dir, &["branch", "--set-upstream-to=up"]);
        git(&dir, &["checkout", "-q", "up"]);
        std::fs::write(dir.join("b.txt"), "b\n").unwrap();
        git(&dir, &["add", "b.txt"]);
        git(&dir, &["commit", "-q", "-m", "upstream work"]);
        git(&dir, &["checkout", "-q", "main"]);
        std::fs::write(dir.join("c.txt"), "c\n").unwrap();
        git(&dir, &["add", "c.txt"]);
        git(&dir, &["commit", "-q", "-m", "local work"]);
        std::fs::write(dir.join("a.txt"), "changed\n").unwrap();
        std::fs::write(dir.join("untracked.txt"), "u\n").unwrap();
        dir
    }

    #[tokio::test]
    async fn reads_a_real_repository() {
        let dir = temp_repo();
        let s = read(&dir).await.expect("a status");
        assert_eq!(s.branch(), Some("main"));
        assert!(s.upstream);
        assert_eq!((s.ahead, s.behind), (1, 1));
        assert_eq!(
            (s.staged, s.unstaged, s.untracked, s.conflicts),
            (0, 1, 1, 0)
        );
        // Not a repository: nothing.
        let plain = std::env::temp_dir().join(format!("claudio-norepo-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&plain).unwrap();
        assert_eq!(read(&plain).await, None);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&plain);
    }
}
