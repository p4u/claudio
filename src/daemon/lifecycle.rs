//! A session's lifecycle after its first spawn: the in-place respawn, the
//! fresh retry after a quick-exit failure, and closing a session whose
//! process exited for good.

use std::io;
use std::sync::Arc;

use super::admit::SessionEnv;
use super::journal::Entry;
use super::session::{self, Cmd};
use super::{in_progress, Daemon};
use crate::proto::{RespawnSpec, SessionEvent, SessionId, SessionState, SpawnSpec};

impl Daemon {
    /// Retry spawning a session without `--resume` after a quick-exit failure.
    ///
    /// Called by the session actor (§2.3). Looks up the journal entry, strips
    /// `--resume`/`--session-id` args, and re-spawns with a fresh conversation
    /// and the env the failed spawn had (kept in memory, never journaled; a
    /// daemon restarted meanwhile has none, and the retry goes without).
    pub(crate) async fn retry_spawn_fresh(self: &Arc<Self>, id: SessionId) {
        // Build a stripped spec from the journal entry.
        let spec_opt = {
            let reg = self.registry();
            reg.journal.entries().iter().find(|e| e.id == id).map(|e| {
                let kept = reg.session_env.get(&id).cloned().unwrap_or_else(|| {
                    tracing::warn!(%id, "retry_spawn_fresh: no spawn env kept; retrying without");
                    SessionEnv::default()
                });
                let spec = crate::proto::SpawnSpec {
                    id,
                    cwd: e.cwd.clone(),
                    name: e.name.clone(),
                    args: without_conversation(&e.args),
                    env: kept.env,
                    rows: 24,
                    cols: 80,
                    ephemeral: e.ephemeral,
                    extensions: e.extensions.clone(),
                    env_remove: kept.env_remove,
                    optional: e.optional.clone(),
                };
                (spec, e.kind)
            })
        };
        let Some((spec, kind)) = spec_opt else {
            tracing::warn!(%id, "retry_spawn_fresh: no journal entry found");
            self.forget_live(id, ""); // ensure it's dormant
            self.broadcast(
                id,
                crate::proto::SessionEvent::State {
                    state: crate::proto::SessionState::Exited,
                },
            );
            self.broadcast(id, crate::proto::SessionEvent::Exited { code: None });
            return;
        };
        tracing::info!(%id, "retrying spawn without --resume");
        match self.spawn(spec, kind).await {
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(%id, error = %e, "retry spawn failed; session stays dormant");
                self.forget_live(id, "");
                self.broadcast(
                    id,
                    crate::proto::SessionEvent::State {
                        state: crate::proto::SessionState::Exited,
                    },
                );
                self.broadcast(id, crate::proto::SessionEvent::Exited { code: Some(1) });
            }
        }
    }

    /// Restart a journaled session in place: stop its process (if any), then
    /// spawn the same id again with its journaled cwd, name and args, resuming
    /// the conversation unless `fresh` (or none was recorded yet). The journal
    /// entry stays, and the old actor leaves silently (it is no longer in
    /// `live`, and `Stop` skips its exit handling), so clients see `Created`
    /// but never `Removed`. Clients attached to the old process are attached
    /// to the new one, each getting a fresh `Attached` + snapshot.
    ///
    /// The id is reserved in `in_flight` from before the old process is
    /// stopped until the new one is committed, like a spawn's: a second
    /// Respawn (or a Spawn) is refused meanwhile, and a Kill is deferred to
    /// `to_kill` and wins, so a killed session is never brought back.
    pub(super) async fn respawn(self: &Arc<Self>, req: RespawnSpec) -> io::Result<Option<u32>> {
        self.check_respawn(&req)?;
        let id = req.id;
        let (entry, old) = {
            let mut reg = self.registry();
            if reg.in_flight.contains(&id) {
                return Err(in_progress(id));
            }
            let entry = reg.journal.entries().iter().find(|e| e.id == id).cloned();
            let entry = entry.ok_or_else(|| io::Error::other(format!("no such session: {id}")))?;
            reg.in_flight.insert(id);
            (entry, reg.live.remove(&id))
        };
        let subscribers = match old {
            Some(old) => session::stop(&old).await,
            None => Vec::new(),
        };
        // A Kill that came while the old process was stopping has been
        // acknowledged: finish the removal instead of starting a new one.
        let killed = {
            let mut reg = self.registry();
            let killed = reg.to_kill.remove(&id);
            if killed {
                reg.in_flight.remove(&id);
            }
            killed
        };
        if killed {
            tracing::info!(%id, "respawn abandoned: the session was killed meanwhile");
            let _ = self.kill(id).await;
            return Err(io::Error::other(format!("session {id} was removed")));
        }
        if req.fresh {
            self.forget_conversation(id);
        }
        let kind = entry.kind;
        let spawned = self.spawn_reserved(respawn_spec(entry, req), kind).await;
        match (&spawned, self.session(id)) {
            (Ok(_), Some(tx)) => {
                for (client, queue) in subscribers {
                    // A zero size keeps the new process's size.
                    let attach = Cmd::Attach {
                        client,
                        queue,
                        req: None,
                        rows: 0,
                        cols: 0,
                    };
                    let _ = tx.send(attach).await;
                }
            }
            // Spawned, then killed by a Kill that came meanwhile.
            (Ok(_), None) => {}
            (Err(_), _) => {
                // The old process is gone: the tab is dormant until the next try.
                self.broadcast(
                    id,
                    SessionEvent::State {
                        state: SessionState::Exited,
                    },
                );
                self.broadcast(id, SessionEvent::Exited { code: None });
            }
        }
        spawned
    }

    /// Forget session `id`'s claude conversation (a fresh reset), durably,
    /// and tell clients to drop theirs too. Otherwise a new process that dies
    /// before its first `SessionStart` would come back, from the journal or a
    /// client's saved state, resuming the conversation the user discarded.
    fn forget_conversation(&self, id: SessionId) {
        let snap = {
            let mut reg = self.registry();
            if !reg.journal.clear_claude_session(id) {
                return;
            }
            reg.journal.snapshot()
        };
        if let Err(e) = self.write_journal(&snap) {
            tracing::error!(%id, error = %e, "could not write journal after a fresh reset");
        }
        self.broadcast(id, SessionEvent::ClaudeSessionCleared);
    }

    /// Close a session whose process exited for good (a clean exit): remove
    /// its journal entry and announce `Removed`, like a Kill. Unless the id
    /// has been taken over since the exit (a respawn or spawn has reserved or
    /// committed it): then the new process owns it and nothing is closed.
    /// The check and the removal are one step under the registry lock.
    pub(crate) fn close_exited(&self, id: SessionId) {
        let ((entry, env), snap) = {
            let mut reg = self.registry();
            if reg.live.contains_key(&id) || reg.in_flight.contains(&id) {
                tracing::info!(%id, "exited session was replaced; not closing it");
                return;
            }
            let Some(entry) = reg.journal.entries().iter().find(|e| e.id == id).cloned() else {
                return; // already killed
            };
            reg.journal.remove_in_memory(id);
            let env = reg.session_env.remove(&id);
            ((entry, env), reg.journal.snapshot())
        };
        if let Err(e) = self.write_journal(&snap) {
            // Not removed on disk: keep it, dormant, rather than lie.
            tracing::warn!(%id, error = %e, "could not close exited session");
            let mut reg = self.registry();
            reg.journal.reinsert(entry);
            if let Some(env) = env {
                reg.session_env.insert(id, env);
            }
            return;
        }
        session::remove_context(&self.config.socket, id);
        tracing::info!(%id, "session closed after its process exited");
        self.broadcast(id, SessionEvent::Removed);
    }
}

/// What a respawn starts: the journaled cwd, name, args and optional flags
/// (resuming the journaled conversation unless `fresh`), with the request's
/// env, size and extensions (else the journaled extensions).
fn respawn_spec(entry: Entry, req: RespawnSpec) -> SpawnSpec {
    let mut args = without_conversation(&entry.args);
    if let Some(conversation) = entry.claude_session_id.filter(|_| !req.fresh) {
        args.extend(["--resume".to_owned(), conversation]);
    }
    SpawnSpec {
        id: req.id,
        cwd: entry.cwd,
        name: entry.name,
        args,
        env: req.env,
        rows: req.rows,
        cols: req.cols,
        ephemeral: entry.ephemeral,
        extensions: req.extensions.or(entry.extensions),
        env_remove: req.env_remove,
        optional: entry.optional,
    }
}

/// `args` without the flags that pick a conversation (`--resume`,
/// `--continue`, `--session-id`), so a respawn can choose its own.
fn without_conversation(args: &[String]) -> Vec<String> {
    let mut kept = Vec::with_capacity(args.len());
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--resume" | "-r" | "--session-id" => {
                args.next();
            }
            "--continue" | "-c" => {}
            a if a.starts_with("--resume=") || a.starts_with("--session-id=") => {}
            _ => kept.push(arg.clone()),
        }
    }
    kept
}
