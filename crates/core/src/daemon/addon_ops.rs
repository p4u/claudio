//! Addon ops in the daemon (docs/daemon-protocol.md, "Addon ops"): the
//! `DaemonCtx` an addon's op gets, and the runner of `Msg::AddonOp`, which
//! the client loop starts through `reply_later`, so it never blocks the loop.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::Semaphore;

use super::{service, Daemon};
use crate::addon::{Addons, BoxFuture, ClaudeFlags, DaemonCtx, ServiceState};
use crate::proto::{self, Msg};

/// How long one addon op may run (`Config::addon_timeout`).
pub(crate) const TIMEOUT: Duration = Duration::from_secs(150);

/// Ops of one addon in flight at once; the next is refused as busy.
const IN_FLIGHT: usize = 8;

/// One semaphore of [`IN_FLIGHT`] permits per daemon-side addon.
pub(crate) fn slots(addons: &Addons) -> HashMap<&'static str, Arc<Semaphore>> {
    addons
        .iter()
        .filter(|a| a.daemon().is_some())
        .map(|a| (a.name(), Arc::new(Semaphore::new(IN_FLIGHT))))
        .collect()
}

/// `Welcome.features`: this build's, then `addon:<name>` per daemon-side
/// addon (none for a daemon playing the previous release).
pub(crate) fn features(daemon: &Daemon) -> Vec<String> {
    let own = proto::DAEMON_FEATURES.iter().map(|f| f.to_string());
    let addons = match daemon.config.old_peer {
        true => Vec::new(),
        false => daemon.addons.daemon_features(),
    };
    own.chain(addons).collect()
}

/// The `DaemonCtx` of addon `addon` in this daemon.
pub(crate) struct Ctx {
    daemon: Arc<Daemon>,
    addon: &'static str,
    /// The addon's declared sections of `config.toml`, read when the context was made.
    config: toml::Table,
}

/// Addon `addon`'s context, with its declared `config.toml` sections read now, off the
/// runtime, under the service supervisor's 1 MiB cap.
pub(crate) async fn ctx(daemon: &Arc<Daemon>, addon: &'static str) -> Arc<dyn DaemonCtx> {
    let sections = daemon
        .addons
        .get(addon)
        .map_or(&[][..], |a| a.config_sections());
    let path = daemon.config.config_file.clone();
    let read = tokio::task::spawn_blocking(move || path.and_then(|p| service::read_config(&p)));
    let mut config = read.await.ok().flatten().unwrap_or_default();
    config.retain(|name, value| sections.contains(&name) && value.is_table());
    Arc::new(Ctx {
        daemon: Arc::clone(daemon),
        addon,
        config,
    })
}

impl DaemonCtx for Ctx {
    fn claude_bin(&self) -> PathBuf {
        self.daemon.config.claude_bin()
    }

    fn path(&self) -> Option<OsString> {
        self.daemon.config.path.clone()
    }

    fn claude_flags(&self) -> BoxFuture<'static, ClaudeFlags> {
        let daemon = Arc::clone(&self.daemon);
        Box::pin(async move {
            // The first call runs `claude --help`: off the runtime.
            tokio::task::spawn_blocking(move || daemon.claude_flags())
                .await
                .unwrap_or_default()
        })
    }

    fn cache_dir(&self) -> PathBuf {
        crate::paths::cache_dir().join(self.addon)
    }

    fn updating(&self) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(&self.daemon.updating)
    }

    fn service_reload(&self, force: bool) -> BoxFuture<'static, ServiceState> {
        let (daemon, addon) = (Arc::clone(&self.daemon), self.addon);
        Box::pin(async move { daemon.services.reload(&daemon, addon, force).await })
    }

    fn service_state(&self) -> ServiceState {
        self.daemon.services.state(self.addon)
    }

    fn config(&self, section: &str) -> toml::Table {
        match self.config.get(section) {
            Some(toml::Value::Table(t)) => t.clone(),
            _ => toml::Table::new(),
        }
    }
}

/// Run one AddonOp: body cap, find the addon's DaemonOps, take a slot (none
/// free → Error "addon <name> busy"), `handle` under the timeout, reply cap.
/// Logs `addon`, `op` and the body's encoded length, never the body (F23).
pub(crate) async fn run(daemon: Arc<Daemon>, addon: String, op: String, body: Value) -> Msg {
    if daemon.config.old_peer {
        // The previous release decodes the op as unknown.
        return Msg::Error {
            message: "unsupported op: addon_op".to_owned(),
        };
    }
    let bytes = proto::addon_body_len(&body);
    tracing::info!(%addon, %op, bytes, "addon op");
    match handle(daemon, &addon, &op, body).await {
        Ok(body) => Msg::AddonReply { body },
        Err(message) => {
            tracing::warn!(%addon, %op, error = %message, "addon op refused or failed");
            Msg::Error { message }
        }
    }
}

async fn handle(daemon: Arc<Daemon>, addon: &str, op: &str, body: Value) -> Result<Value, String> {
    proto::addon_body_ok(&body)?;
    let no_such = || format!("no such addon {addon}");
    let found = daemon.addons.get(addon).ok_or_else(no_such)?;
    let ops = found.daemon().ok_or_else(no_such)?;
    let slot = daemon.addon_slots.get(addon).ok_or_else(no_such)?;
    let _permit = Arc::clone(slot)
        .try_acquire_owned()
        .map_err(|_| format!("addon {addon} busy"))?;
    let ctx = ctx(&daemon, found.name()).await;
    let reply = tokio::time::timeout(daemon.config.addon_timeout, ops.handle(op, body, ctx))
        .await
        .map_err(|_| format!("addon {addon} timed out"))??;
    proto::addon_body_ok(&reply)?;
    Ok(reply)
}

/// Every daemon-side addon's `on_claude_installed`: claude was just
/// installed on this host.
pub(crate) async fn claude_installed(daemon: &Arc<Daemon>) {
    for addon in daemon.addons.iter() {
        if let Some(ops) = addon.daemon() {
            ops.on_claude_installed(&ctx(daemon, addon.name()).await);
        }
    }
}
