//! The web's edge of the addon runtime (`app::addons`): the hub's host
//! connections, losses and claude installs (its `HubEvent`s), its sessions
//! with the attached ones focused, and what the runtime tells the page.

use serde_json::json;
use tokio::sync::broadcast;

use std::sync::Arc;

use super::events::{ev, extensions_changed};
use super::ext::Extensions;
use super::hub::{HubEvent, HubHandle, SessionView};
use crate::addon::{Addons, SessionSnap};
use crate::app::addons::{AddonRuntime, RuntimeOut, RuntimeRx};

/// Follow the hub's host events into `runtime` (from a resync of the hosts
/// connected already), send the runtime's notices to every tab, an addon's
/// change through `feed` (its extension rows), and a re-read config.toml
/// to the extensions.
pub(crate) fn follow(hub: HubHandle, runtime: AddonRuntime, mut rx: RuntimeRx, mut feed: Feed) {
    // Subscribed before the hosts are read: no HostUp is missed.
    let events = hub.subscribe();
    tokio::spawn(follow_hosts(hub.clone(), runtime, events));
    tokio::spawn(async move {
        feed.changed().await;
        while let Some(out) = rx.recv().await {
            match out {
                RuntimeOut::Notice { notice, .. } => {
                    let fields = json!({"text": notice.text, "level": notice.level,
                        "host": null, "sid": null});
                    hub.broadcast(ev("notice", fields));
                }
                RuntimeOut::Changed { addon } => {
                    hub.broadcast(ev("addons.changed", json!({ "addon": addon })));
                    feed.changed().await;
                }
                RuntimeOut::ConfigChanged => feed.config_changed().await,
                RuntimeOut::Offer { .. } => {}
            }
        }
    });
}

/// What an addon's change and a re-read config.toml mean for the
/// extension registry.
pub(crate) struct Feed {
    hub: HubHandle,
    ext: Arc<Extensions>,
    runtime: AddonRuntime,
}

impl Feed {
    pub(crate) fn new(hub: HubHandle, ext: Arc<Extensions>, runtime: AddonRuntime) -> Feed {
        Feed { hub, ext, runtime }
    }

    /// An addon's data changed: the registry's addon rows, then
    /// `extensions_changed` when they differ.
    async fn changed(&mut self) {
        if self.ext.set_rows(&self.runtime.extension_rows()) {
            self.hub.broadcast(extensions_changed());
        }
    }

    /// config.toml was re-read: the registry follows it, the addons' rows
    /// with it.
    async fn config_changed(&self) {
        let rows = self.runtime.extension_rows();
        self.ext.reread(&self.hub, &rows).await;
    }
}

async fn follow_hosts(
    hub: HubHandle,
    runtime: AddonRuntime,
    mut events: broadcast::Receiver<HubEvent>,
) {
    resync(&hub, &runtime).await;
    loop {
        match events.recv().await {
            Ok(HubEvent::HostUp { host }) => host_up(&hub, &runtime, &host).await,
            Ok(HubEvent::HostDown { host }) => runtime.host_down(&host),
            Ok(HubEvent::ClaudeInstalled { host }) => runtime.claude_installed(&host),
            Err(broadcast::error::RecvError::Lagged(_)) => resync(&hub, &runtime).await,
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

/// Every host the hub knows: up when connected, down when not.
async fn resync(hub: &HubHandle, runtime: &AddonRuntime) {
    for h in hub.hosts().await {
        let Some(name) = h["name"].as_str() else {
            continue;
        };
        if h["connected"] == true {
            host_up(hub, runtime, name).await;
        } else {
            runtime.host_down(name);
        }
    }
}

async fn host_up(hub: &HubHandle, runtime: &AddonRuntime, host: &str) {
    if let Ok((client, _)) = hub.host_client(host).await {
        runtime.host_up(host, client);
    }
}

/// The hub's sessions as addons see them, `focused` where a tab attached,
/// each with its recorded launch (the server's state, never the page's).
pub(crate) fn snaps<'a>(
    addons: &Addons,
    views: impl Iterator<Item = (&'a SessionView, bool)>,
) -> Vec<SessionSnap> {
    views
        .map(|(v, focused)| SessionSnap {
            sid: v.info.id,
            host: v.host.clone(),
            kind: v.info.kind,
            cwd: v.info.cwd.clone(),
            label: v
                .info
                .name
                .clone()
                .or_else(|| v.info.title.clone())
                .unwrap_or_else(|| v.info.cwd.clone()),
            conversation: v.info.claude_session_id.clone(),
            launch: super::addons::launch_of(addons, v.meta.as_ref()).1,
            focused,
        })
        .collect()
}
