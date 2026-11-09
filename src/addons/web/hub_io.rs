//! The hub's I/O off the actor: connecting a host (and listing its
//! sessions), and the one writer of web-state.json.

use std::path::PathBuf;

use tokio::sync::mpsc;

use super::hub::Done;
use super::state::{self, WebState};
use crate::client::{self, Client};
use crate::freshness::DaemonCheck;
use crate::paths;
use crate::proto::{Msg, SessionInfo};

/// Connect `host` and list its sessions. The error says whether the
/// bootstrap failed (the next attempt runs it again).
pub(super) async fn connect_once(
    host: &str,
    bootstrap: bool,
) -> Result<(Client, Vec<SessionInfo>), (String, bool)> {
    let client = if host == "local" {
        let check = client::ensure_local_daemon()
            .await
            .map_err(|e| (e.to_string(), false))?;
        let client = client::connect(&paths::daemon_socket())
            .await
            .map_err(|e| (e.to_string(), false))?;
        if check == DaemonCheck::Deferred {
            client.mark_daemon_outdated();
        }
        client
    } else {
        if bootstrap {
            crate::remote::bootstrap::ensure_remote(host)
                .await
                .map_err(|e| (format!("bootstrap: {e}"), true))?;
        }
        client::connect_ssh_reporting(host)
            .await
            .map_err(|e| (e.to_string(), false))?
    };
    let sessions = match client.request(Msg::ListSessions).await {
        Ok(Msg::Sessions { sessions }) => sessions,
        Ok(_) => return Err(("unexpected reply to ListSessions".to_owned(), false)),
        Err(e) => return Err((e.to_string(), false)),
    };
    // An old daemon answers an error: no stats, nothing else changes.
    let stats = client.request(Msg::SubscribeHostStats);
    tokio::spawn(async move {
        let _ = stats.await;
    });
    Ok((client, sessions))
}

pub(super) struct WriteJob {
    pub(super) state: WebState,
    pub(super) done: Vec<Done>,
}

/// The one writer of web-state.json: jobs in order, the newest of a burst
/// written once, each job's `done` run after its write.
pub(super) async fn write_state(path: PathBuf, mut jobs: mpsc::UnboundedReceiver<WriteJob>) {
    while let Some(mut job) = jobs.recv().await {
        while let Ok(next) = jobs.try_recv() {
            job.state = next.state;
            job.done.extend(next.done);
        }
        let (p, s) = (path.clone(), job.state);
        let result = match tokio::task::spawn_blocking(move || state::save(&p, &s)).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(format!("cannot save web-state.json: {e}")),
            Err(_) => Err("cannot save web-state.json".to_owned()),
        };
        if let Err(e) = &result {
            tracing::warn!("web: {e}");
        }
        for d in job.done {
            d(result.clone());
        }
    }
}
