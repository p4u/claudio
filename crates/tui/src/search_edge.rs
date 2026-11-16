//! The TUI's edge of the multi-host conversation search
//! ([`claudio_core::app::search`]): [`Effect::SearchAll`] asks every
//! connected host whose daemon can search, off the event loop, and what it
//! found comes back as one `HostEvent::SearchAllDone`.
//!
//! [`Effect::SearchAll`]: super::app::Effect::SearchAll

use std::collections::HashMap;

use tokio::sync::mpsc;

use super::builtins::{HostHit, SearchAllDone};
use super::HostEvent;
use claudio_core::app::connections::Connections;
use claudio_core::app::search;

/// Search `query` on every connected host that can, for the search started
/// under `gen`.
pub(super) fn run(gen: u64, query: String, conns: &Connections, ev_tx: &mpsc::Sender<HostEvent>) {
    let mut homes = HashMap::new();
    let mut clients: Vec<_> = conns
        .map
        .iter()
        .filter_map(|(host, conn)| {
            let client = conn.client.as_ref()?;
            search::can_search(client.welcome()).then(|| {
                homes.insert(host.clone(), client.welcome().host.home.clone());
                (host.clone(), client.clone())
            })
        })
        .collect();
    clients.sort_by(|a, b| a.0.cmp(&b.0));
    let searched = clients.len();
    let tx = ev_tx.clone();
    tokio::spawn(async move {
        let found = search::search_clients(clients, query.clone()).await;
        let hits = found
            .hits
            .into_iter()
            .map(|(host, hit)| HostHit {
                home: homes.get(&host).cloned().unwrap_or_default(),
                host,
                hit,
            })
            .collect();
        let done = SearchAllDone {
            gen,
            query,
            searched,
            hits,
            failed: found.failed,
        };
        let _ = tx.send(HostEvent::SearchAllDone(done)).await;
    });
}
