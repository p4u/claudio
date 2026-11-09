//! What the daemon takes from a spawn or respawn request: the caps it
//! checks before anything starts, and the spec a session's process is
//! actually started with (a terminal's, and a claude session's effective
//! extensions).

use std::io;

use super::{command, Daemon};
use crate::proto::{LensKind, LensSpec, RespawnSpec, SessionExtensions, SessionKind, SpawnSpec};

/// Refuse a spawn whose request breaks a cap, before anything is reserved.
pub(super) fn check_spawn(spec: &SpawnSpec) -> io::Result<()> {
    check_extensions(spec.extensions.as_ref())?;
    check_env_remove(spec.env_remove.as_deref())?;
    command::optional_ok(&spec.optional)
        .map_err(|why| io::Error::other(format!("bad optional: {why}")))
}

/// Refuse a respawn whose request breaks a cap, before anything is stopped.
pub(super) fn check_respawn(req: &RespawnSpec) -> io::Result<()> {
    check_extensions(req.extensions.as_ref())?;
    check_env_remove(req.env_remove.as_deref())
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
    /// extensions ([`effective_extensions`]).
    pub(super) fn prepare(&self, spec: &mut SpawnSpec, kind: SessionKind) {
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
