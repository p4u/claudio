//! The daemon's durable record of the sessions on this host.
//!
//! The journal outlives the daemon: after a crash or reboot every entry comes
//! back *dormant* (no process), ready to be resumed with
//! `claude --resume <claude_session_id>`. It never stores the spawn env (which
//! may carry secrets) nor the daemon's own `--settings` hook injection. Every
//! change is written with [`paths::write_atomic`] (temp + fsync + rename).

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::paths;
use crate::proto::{SessionId, SpawnSpec};

/// One journaled session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: SessionId,
    pub cwd: String,
    #[serde(default)]
    pub name: Option<String>,
    /// The client's extra claude arguments, verbatim.
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub claude_session_id: Option<String>,
    /// Unix seconds.
    pub created_at: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    sessions: Vec<Entry>,
}

/// The journal file and its in-memory copy, in spawn order.
pub struct Journal {
    path: PathBuf,
    sessions: Vec<Entry>,
}

impl Journal {
    /// Load the journal at `path`. A missing file is an empty journal; an
    /// unreadable or corrupt one is moved aside (`<path>.corrupt`) so it is
    /// neither lost nor silently overwritten.
    pub fn load(path: &Path) -> Journal {
        let sessions = match std::fs::read(path) {
            Ok(bytes) => match serde_json::from_slice::<File>(&bytes) {
                Ok(file) => file.sessions,
                Err(e) => {
                    let aside = path.with_extension("json.corrupt");
                    tracing::warn!(path = %path.display(), error = %e, "corrupt journal moved aside");
                    let _ = std::fs::rename(path, aside);
                    Vec::new()
                }
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "could not read journal");
                Vec::new()
            }
        };
        Journal {
            path: path.to_path_buf(),
            sessions,
        }
    }

    /// All entries, in spawn order.
    pub fn entries(&self) -> &[Entry] {
        &self.sessions
    }

    /// Record a spawn and persist. A re-spawn of a known id keeps its
    /// `created_at` and last claude session id; cwd, name and args follow the
    /// new spec.
    pub fn record_spawn(&mut self, spec: &SpawnSpec) -> Entry {
        let entry = match self.sessions.iter_mut().find(|e| e.id == spec.id) {
            Some(e) => {
                e.cwd = spec.cwd.clone();
                e.name = spec.name.clone();
                e.args = spec.args.clone();
                e.clone()
            }
            None => {
                let e = Entry {
                    id: spec.id,
                    cwd: spec.cwd.clone(),
                    name: spec.name.clone(),
                    args: spec.args.clone(),
                    claude_session_id: None,
                    created_at: now(),
                };
                self.sessions.push(e.clone());
                e
            }
        };
        self.save();
        entry
    }

    /// Record a new claude conversation id (`SessionStart`, `/clear`, fork)
    /// and persist. Returns `false` when `id` is not journaled.
    pub fn set_claude_session(&mut self, id: SessionId, claude_session_id: &str) -> bool {
        let Some(e) = self.sessions.iter_mut().find(|e| e.id == id) else {
            return false;
        };
        e.claude_session_id = Some(claude_session_id.to_owned());
        self.save();
        true
    }

    /// Forget a session and persist. Returns `false` when it was unknown.
    pub fn remove(&mut self, id: SessionId) -> bool {
        let before = self.sessions.len();
        self.sessions.retain(|e| e.id != id);
        if self.sessions.len() == before {
            return false;
        }
        self.save();
        true
    }

    /// Persist the journal. A failed write is logged, not propagated: the
    /// in-memory journal stays authoritative and the next change retries, and
    /// a full disk must never take a live session down with it.
    fn save(&self) {
        let file = File {
            sessions: self.sessions.clone(),
        };
        let written = serde_json::to_vec_pretty(&file)
            .map_err(io::Error::other)
            .and_then(|json| paths::write_atomic(&self.path, &json));
        if let Err(e) = written {
            tracing::error!(path = %self.path.display(), error = %e, "could not write journal");
        }
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn spec(id: SessionId) -> SpawnSpec {
        SpawnSpec {
            id,
            cwd: "/w".into(),
            name: Some("n".into()),
            args: vec!["--resume".into(), "abc".into()],
            env: vec![("ANTHROPIC_AUTH_TOKEN".into(), "s3cret".into())],
            rows: 24,
            cols: 80,
        }
    }

    #[test]
    fn roundtrip_keeps_entries_and_never_env() {
        let dir = std::env::temp_dir().join(format!("claudio-journal-{}", Uuid::new_v4()));
        let path = dir.join("j.json");
        let id = Uuid::new_v4();

        let mut j = Journal::load(&path);
        assert!(j.entries().is_empty());
        let first = j.record_spawn(&spec(id));
        assert!(j.set_claude_session(id, "conv-1"));

        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("s3cret"));
        assert!(!raw.contains("ANTHROPIC_AUTH_TOKEN"));

        let mut j = Journal::load(&path);
        let e = j.entries().iter().find(|e| e.id == id).unwrap().clone();
        assert_eq!(e.claude_session_id.as_deref(), Some("conv-1"));
        assert_eq!(e.args, vec!["--resume", "abc"]);

        // A re-spawn keeps created_at and the conversation id.
        let again = j.record_spawn(&spec(id));
        assert_eq!(again.created_at, first.created_at);
        assert_eq!(again.claude_session_id.as_deref(), Some("conv-1"));
        assert_eq!(j.entries().len(), 1);

        assert!(j.remove(id));
        assert!(!j.remove(id));
        assert!(Journal::load(&path).entries().is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn corrupt_journal_is_moved_aside() {
        let dir = std::env::temp_dir().join(format!("claudio-journal-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("j.json");
        std::fs::write(&path, b"{not json").unwrap();
        assert!(Journal::load(&path).entries().is_empty());
        assert!(dir.join("j.json.corrupt").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
