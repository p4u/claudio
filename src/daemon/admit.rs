//! What the daemon takes from a spawn or respawn request: the caps it
//! checks before anything starts, the spec a session's process is actually
//! started with (a terminal's, and a claude session's effective
//! extensions), and the spawn env it keeps in memory for a retry.

use std::io;

use super::journal::Entry;
use super::{command, Daemon, Registry};
use crate::proto::{LensKind, LensSpec, RespawnSpec, SessionExtensions, SessionKind, SpawnSpec};

/// A session's last spawn env and `env_remove`, kept in memory only (never
/// journaled: the env may carry secrets) so the fresh retry after a failed
/// `--resume` starts with what the spawn it replaces had. `Debug` prints
/// the env's keys only (C1).
#[derive(Clone, Default)]
pub(super) struct SessionEnv {
    pub(super) env: Vec<(String, String)>,
    pub(super) env_remove: Option<Vec<String>>,
}

impl std::fmt::Debug for SessionEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let keys: Vec<&str> = self.env.iter().map(|(k, _)| k.as_str()).collect();
        f.debug_struct("SessionEnv")
            .field("env", &keys)
            .field("env_remove", &self.env_remove)
            .finish()
    }
}

impl Registry {
    /// Journal a committed spawn (in memory) and keep its env for a retry.
    pub(super) fn commit_entry(&mut self, spec: &SpawnSpec, kind: SessionKind) -> Entry {
        let kept = SessionEnv {
            env: spec.env.clone(),
            env_remove: spec.env_remove.clone(),
        };
        self.session_env.insert(spec.id, kept);
        self.journal.upsert_entry(spec, kind)
    }
}

impl Daemon {
    /// Refuse a spawn whose request breaks a cap, before anything is
    /// reserved. An old peer knows neither `env_remove` nor `optional`.
    pub(super) fn check_spawn(&self, spec: &SpawnSpec) -> io::Result<()> {
        check_extensions(spec.extensions.as_ref())?;
        if self.config.old_peer {
            return Ok(());
        }
        check_env_remove(spec.env_remove.as_deref())?;
        command::optional_ok(&spec.optional)
            .map_err(|why| io::Error::other(format!("bad optional: {why}")))
    }

    /// Refuse a respawn whose request breaks a cap, before anything is
    /// stopped.
    pub(super) fn check_respawn(&self, req: &RespawnSpec) -> io::Result<()> {
        check_extensions(req.extensions.as_ref())?;
        if self.config.old_peer {
            return Ok(());
        }
        check_env_remove(req.env_remove.as_deref())
    }
}

/// `CLAUDIO_TEST_OLD_DAEMON=1` in a debug build: the daemon plays the
/// previous release ([`super::Config::old_peer`]). A release build never
/// does.
pub(super) fn old_peer(value: Option<&std::ffi::OsStr>) -> bool {
    cfg!(debug_assertions) && value.is_some_and(|v| v == "1")
}

fn check_env_remove(list: Option<&[String]>) -> io::Result<()> {
    match list.map(command::env_remove_ok) {
        Some(Err(why)) => Err(io::Error::other(format!("bad env_remove: {why}"))),
        _ => Ok(()),
    }
}

fn check_extensions(ext: Option<&SessionExtensions>) -> io::Result<()> {
    match ext.map(SessionExtensions::validate) {
        Some(Err(why)) => Err(io::Error::other(format!("bad extensions: {why}"))),
        _ => Ok(()),
    }
}

impl Daemon {
    /// The spec as the process starts with it: a terminal never carries
    /// claude args, env or extensions; a claude session gets its effective
    /// extensions ([`effective_extensions`]). An old peer drops what the
    /// previous release did not know.
    pub(super) fn prepare(&self, spec: &mut SpawnSpec, kind: SessionKind) {
        if self.config.old_peer {
            spec.env_remove = None;
            spec.optional.clear();
        }
        if kind == SessionKind::Shell {
            spec.args.clear();
            spec.env.clear();
            spec.extensions = None;
            return;
        }
        let journaled = {
            let reg = self.registry();
            let entry = reg.journal.entries().iter().find(|e| e.id == spec.id);
            entry.and_then(|e| e.extensions.clone())
        };
        let requested = spec.extensions.take();
        spec.extensions = Some(effective_extensions(
            requested,
            journaled,
            self.config.latex,
        ));
    }
}

/// The extensions a claude launch runs with: the request's when it has
/// them, else the journaled ones, else the legacy value (the latex lens
/// when this host's `[claude] latex` is on). Lenses of a kind this daemon
/// does not know are dropped, and `[claude] latex = false` drops the latex
/// lens whatever the client asked: a host that turned math off stays off.
pub(super) fn effective_extensions(
    requested: Option<SessionExtensions>,
    journaled: Option<SessionExtensions>,
    latex: bool,
) -> SessionExtensions {
    let mut ext = requested
        .or(journaled)
        // compat(v0.10): a client of the releases before addons sends no
        // extensions; it gets the latex lens it always had. New clients
        // ask for every lens (the web addon's latex), so a core-only build
        // asks for none.
        .unwrap_or_else(|| SessionExtensions {
            lenses: vec![LensSpec {
                name: "latex".to_owned(),
                kind: LensKind::Tag,
                instruction: crate::claude::hooks::LATEX_CONTEXT.to_owned(),
            }],
            ..Default::default()
        });
    ext.lenses
        .retain(|l| l.kind != LensKind::Unknown && (latex || l.name != "latex"));
    ext
}
