//! The runtime's re-reads of config.toml (A10, C2). One blocking read is in
//! flight at a time; a notification that arrives meanwhile sets a pending
//! flag, and any number of them coalesce into one more read after it. Each
//! read is capped at [`CONFIG_MAX`]. A read's result is applied only when
//! nothing newer was applied since it started (a later read, or an addon's
//! own edit through `AddonCtx::edit_config`), so a slow read never puts an
//! older file back.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::Duration;

use super::Inner;
use crate::addon::AddonEvent;

/// The most of config.toml the runtime reads. The file is the member's own
/// and a few KiB; a bigger one is not read and the last good table stays.
pub(super) const CONFIG_MAX: u64 = 1 << 20;

/// The loader's state, kept in the runtime's `State`.
#[derive(Debug, Default)]
pub(super) struct Reload {
    /// The loader task runs.
    running: bool,
    /// A notification came while it ran: it reads once more.
    pending: bool,
    /// The last generation handed out (to a read as it starts, or an edit).
    gen: u64,
    /// The generation of what the cached table holds.
    applied: u64,
    /// Reads started, for tests.
    #[cfg(test)]
    pub(super) reads: usize,
}

impl Reload {
    /// A read starts: its generation.
    pub(super) fn begin(&mut self) -> u64 {
        self.gen += 1;
        #[cfg(test)]
        {
            self.reads += 1;
        }
        self.gen
    }

    /// Whether the result of the read of generation `gen` is newer than
    /// what is applied; if so it is the applied one from now on.
    pub(super) fn accept(&mut self, gen: u64) -> bool {
        let newer = gen > self.applied;
        if newer {
            self.applied = gen;
        }
        newer
    }

    /// Whether the loader runs (tests).
    #[cfg(test)]
    pub(super) fn is_running(&self) -> bool {
        self.running
    }

    /// An edit was applied to the cached table: it is newer than any read
    /// in flight, whose result is dropped; the loader then reads once more
    /// (another process may have edited another table meanwhile).
    pub(super) fn edited(&mut self) {
        self.gen += 1;
        self.applied = self.gen;
        if self.running {
            self.pending = true;
        }
    }
}

/// Ask for a re-read: start the loader, or leave a pending flag for the
/// one that runs.
pub(super) fn request(inner: &Arc<Inner>) {
    {
        let mut state = inner.state();
        if state.reload.running {
            state.reload.pending = true;
            return;
        }
        state.reload.running = true;
    }
    inner.spawn_internal(run(Arc::downgrade(inner)));
}

/// The loader: read, apply unless something newer was, and read again
/// while notifications came meanwhile. Aborted at shutdown with the
/// runtime's other tasks.
async fn run(weak: Weak<Inner>) {
    loop {
        let Some((path, pause, gen)) = weak.upgrade().map(|i| {
            let gen = i.state().reload.begin();
            (i.opts.config_path.clone(), i.limits.reload_pause, gen)
        }) else {
            return;
        };
        let table = load(path, pause).await;
        let Some(inner) = weak.upgrade() else {
            return;
        };
        let mut state = inner.state();
        let changed = match table {
            Some(table) if state.reload.accept(gen) => {
                state.config = table;
                true
            }
            _ => false,
        };
        let again = std::mem::take(&mut state.reload.pending);
        state.reload.running = again;
        drop(state);
        if changed {
            inner.event(AddonEvent::ConfigChanged);
            inner.outbox.push(super::RuntimeOut::ConfigChanged);
        }
        if !again {
            return;
        }
    }
}

/// config.toml as a table (empty when missing), read off the executor;
/// `None` (with a warning) when it cannot be read, is over
/// [`CONFIG_MAX`] or does not parse. `pause` (tests) holds the result back
/// after the read.
pub(super) async fn load(path: PathBuf, pause: Duration) -> Option<toml::Table> {
    let read = tokio::task::spawn_blocking(move || {
        let text = read_capped(&path);
        if !pause.is_zero() {
            std::thread::sleep(pause);
        }
        text?
            .parse::<toml::Table>()
            .map_err(|e| e.message().to_owned())
    });
    match read.await {
        Ok(Ok(table)) => Some(table),
        Ok(Err(why)) => {
            tracing::warn!(%why, "config.toml not read for the addons");
            None
        }
        Err(_) => None,
    }
}

/// The file's text, at most [`CONFIG_MAX`] bytes ("" when it is missing).
/// Only a regular file is opened (a FIFO would block the read).
fn read_capped(path: &Path) -> Result<String, String> {
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(e) => return Err(e.to_string()),
    };
    if !meta.is_file() {
        return Err("not a regular file".to_owned());
    }
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut text = String::new();
    file.take(CONFIG_MAX + 1)
        .read_to_string(&mut text)
        .map_err(|e| e.to_string())?;
    if text.len() as u64 > CONFIG_MAX {
        return Err(format!("over {} KiB", CONFIG_MAX / 1024));
    }
    Ok(text)
}
