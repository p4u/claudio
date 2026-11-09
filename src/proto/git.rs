//! The git viewer's replies (`GitLog`, `GitCommit`, `GitDiff`,
//! `GitWorktree`). Re-exported from [`crate::proto`].

use serde::{Deserialize, Serialize};

/// The most commits one `GitLog` request may ask for.
pub const GIT_LOG_MAX: u32 = 500;

/// Reply to `GitLog`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitLogPage {
    /// The repository's top-level directory.
    pub root: String,
    /// The checked-out branch; `None` for a detached HEAD.
    #[serde(default)]
    pub head: Option<String>,
    pub commits: Vec<GitLogEntry>,
    /// Whether commits exist beyond this page.
    #[serde(default)]
    pub more: bool,
}

/// One log row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitLogEntry {
    /// Full object id.
    pub id: String,
    pub parents: Vec<String>,
    pub author: String,
    /// Author time, Unix seconds.
    pub time: i64,
    /// Decorations as `--decorate=full` prints them, one per entry:
    /// `HEAD -> refs/heads/main`, `refs/remotes/origin/main`, `tag: refs/tags/v1`.
    pub refs: Vec<String>,
    pub subject: String,
}

/// Reply to `GitCommit`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitCommitInfo {
    pub id: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    pub time: i64,
    pub committer_time: i64,
    pub refs: Vec<String>,
    /// The full message, subject first.
    pub message: String,
    pub files: Vec<GitFile>,
    /// Set for merges: `files` and patches are against the first parent only.
    #[serde(default)]
    pub first_parent: bool,
}

/// A file changed by a commit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitFile {
    pub path: String,
    /// The previous name, for a rename.
    #[serde(default)]
    pub old_path: Option<String>,
    /// Lines added; `None` for a binary file.
    #[serde(default)]
    pub added: Option<u32>,
    /// Lines removed; `None` for a binary file.
    #[serde(default)]
    pub removed: Option<u32>,
}

/// Reply to `GitDiff`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitPatch {
    pub id: String,
    pub path: Option<String>,
    pub patch: String,
    /// Set when the patch was cut to fit the size limits.
    #[serde(default)]
    pub truncated: bool,
}

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
