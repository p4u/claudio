//! The read-only git ops (docs/web-protocol.md §5, "Git"): a session's
//! working tree, log, commits and diffs, asked of its host's daemon.

use serde::Deserialize;
use serde_json::Value;

use super::hub::{self, daemon_error};
use super::ws::{check_path, parse, to_json, SidArgs, UNEXPECTED};
use crate::proto::{Msg, SessionId};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorktreeDiffArgs {
    sid: SessionId,
    path: String,
    #[serde(default)]
    staged: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LogArgs {
    sid: SessionId,
    #[serde(default)]
    all: bool,
    #[serde(default)]
    skip: u32,
    #[serde(default)]
    limit: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CommitArgs {
    sid: SessionId,
    oid: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DiffArgs {
    sid: SessionId,
    oid: String,
    #[serde(default)]
    file: Option<u32>,
}

/// One `git.*` op.
pub(crate) async fn dispatch(hub: &hub::HubHandle, op: &str, args: Value) -> Result<Value, String> {
    match op {
        "git.status" => {
            let a: SidArgs = parse(args)?;
            let s = worktree_session(hub, a.sid).await?;
            match s
                .client
                .request(Msg::GitWorktree { cwd: s.info.cwd })
                .await
                .map_err(daemon_error)?
            {
                Msg::GitWorktreeStatus(status) => to_json(status),
                _ => Err(UNEXPECTED.to_owned()),
            }
        }
        "git.worktree_diff" => {
            let a: WorktreeDiffArgs = parse(args)?;
            check_path("path", &a.path)?;
            let s = worktree_session(hub, a.sid).await?;
            let msg = Msg::GitWorktreeDiff {
                cwd: s.info.cwd,
                path: a.path,
                staged: a.staged,
            };
            match s.client.request(msg).await.map_err(daemon_error)? {
                Msg::GitPatch(patch) => to_json(patch),
                _ => Err(UNEXPECTED.to_owned()),
            }
        }
        "git.log" => {
            let a: LogArgs = parse(args)?;
            let limit = a.limit.unwrap_or(100);
            if !(1..=crate::proto::GIT_LOG_MAX).contains(&limit) {
                return Err("bad request: limit: must be 1..=500".to_owned());
            }
            let s = hub.session(a.sid).await?;
            let msg = Msg::GitLog {
                cwd: s.info.cwd,
                all: a.all,
                skip: a.skip,
                limit,
            };
            match s.client.request(msg).await.map_err(daemon_error)? {
                Msg::GitLogPage(page) => to_json(page),
                _ => Err(UNEXPECTED.to_owned()),
            }
        }
        "git.commit" => {
            let a: CommitArgs = parse(args)?;
            check_oid(&a.oid)?;
            let s = hub.session(a.sid).await?;
            let msg = Msg::GitCommit {
                cwd: s.info.cwd,
                id: a.oid,
            };
            match s.client.request(msg).await.map_err(daemon_error)? {
                Msg::GitCommitInfo(info) => to_json(info),
                _ => Err(UNEXPECTED.to_owned()),
            }
        }
        "git.diff" => {
            let a: DiffArgs = parse(args)?;
            check_oid(&a.oid)?;
            let s = hub.session(a.sid).await?;
            let msg = Msg::GitDiff {
                cwd: s.info.cwd,
                id: a.oid,
                file: a.file,
                path: None,
                old_path: None,
            };
            match s.client.request(msg).await.map_err(daemon_error)? {
                Msg::GitPatch(patch) => to_json(patch),
                _ => Err(UNEXPECTED.to_owned()),
            }
        }
        _ => Err("unknown op".to_owned()),
    }
}

/// A full object id: 40 or 64 hex digits.
fn check_oid(oid: &str) -> Result<(), String> {
    if matches!(oid.len(), 40 | 64) && oid.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err("bad request: oid: not a full object id".to_owned())
    }
}

/// The session, if its daemon has the working-tree ops.
async fn worktree_session(hub: &hub::HubHandle, sid: SessionId) -> Result<hub::SessionRef, String> {
    let s = hub.session(sid).await?;
    if !s.features.iter().any(|f| f == "worktree") {
        return Err("unsupported by this host's daemon".to_owned());
    }
    Ok(s)
}
