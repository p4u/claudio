//! Host connections of the TUI: the local daemon, the background connect
//! and reconnect tasks, and what happens once a host's daemon answers.

use std::io;
use std::time::Duration;

use tokio::sync::mpsc;

use super::app::App;
use super::HostEvent;
use crate::app::connections::Connections;
use crate::client::{self, Client, Incoming};
use crate::freshness::{self, DaemonCheck};
use crate::paths;
use crate::proto::{Msg, SessionInfo};
use crate::remote::bootstrap::ensure_remote;

/// A host's daemon connected (bootstrap + connect): start its reader and
/// recover its sessions. Whether it was taken (a stale attempt is not).
pub(super) fn on_connected(
    app: &mut App,
    conns: &mut Connections,
    ev_tx: &mpsc::Sender<HostEvent>,
    host: &str,
    client: Client,
    sessions: Vec<SessionInfo>,
    generation: u64,
) -> bool {
    // A newer attempt is already in flight.
    if generation < conns.current_generation(host) {
        return false;
    }
    let home = client.welcome().host.home.clone();
    let remote_claude = client_claude(&client);
    let features = client.welcome().features.clone();
    conns.reset_delay(host);
    // Start the reader tagged with the current generation.
    let cur_gen = conns.current_generation(host);
    if let Some(rx) = client.take_incoming() {
        spawn_reader(host.to_owned(), cur_gen, rx, ev_tx.clone());
    }
    conns.connected(host, client);
    // Record this host in the MRU so it appears first next time.
    crate::remote::hosts::touch(host);
    app.on_ssh_hosts(crate::remote::hosts::candidates());
    app.on_host_connected(host, &home);
    app.on_daemon_host(host, &features);
    // Recover this host's sessions only.
    app.recover_host(host, &sessions);
    app.subscribe_host_stats(host);
    app.check_remote_claude(host, remote_claude.as_deref());
    app.redraw = true;
    true
}

/// The local daemon answered again after a lost connection: start its
/// reader and recover its sessions. Whether it was taken (a stale attempt
/// is not).
pub(super) fn on_local_reconnected(
    app: &mut App,
    conns: &mut Connections,
    ev_tx: &mpsc::Sender<HostEvent>,
    client: Client,
    sessions: Vec<SessionInfo>,
    generation: u64,
) -> bool {
    // A newer attempt is already in flight.
    if generation < conns.current_generation("local") {
        return false;
    }
    conns.reset_delay("local");
    let home = client.welcome().host.home.clone();
    let cur_gen = conns.current_generation("local");
    if let Some(rx) = client.take_incoming() {
        spawn_reader("local".to_owned(), cur_gen, rx, ev_tx.clone());
    }
    app.set_local_claude(client_claude(&client));
    let features = client.welcome().features.clone();
    conns.connected("local", client);
    app.on_daemon_host("local", &features);
    app.check_local_install();
    app.on_reconnected_local(&sessions, home);
    true
}

/// Ask every host whose daemon is outdated whether it is idle now, so it
/// can be replaced ([`retry_outdated`]).
pub(super) fn retry_outdated_hosts(conns: &Connections, ev_tx: &mpsc::Sender<HostEvent>) {
    for (host, conn) in &conns.map {
        if let Some(client) = conn.client.as_ref().filter(|c| c.daemon_outdated()) {
            let retry =
                retry_outdated(host.clone(), conn.generation, client.clone(), ev_tx.clone());
            tokio::spawn(retry);
        }
    }
}

/// Start the local daemon if needed (replacing an outdated one), connect and
/// list its sessions.
pub(super) async fn connect_local() -> io::Result<(Client, Vec<SessionInfo>)> {
    let check = client::ensure_local_daemon().await?;
    let client = client::connect(&paths::daemon_socket()).await?;
    if check == DaemonCheck::Deferred {
        client.mark_daemon_outdated();
    }
    list_sessions(&client).await.map(|s| (client, s))
}

/// Once the outdated daemon behind `client` is idle, ask for a reconnect:
/// connecting runs the freshness check again (`connect_local`'s, or the
/// remote bridge's), which replaces it now. A busy one waits for next time.
pub(super) async fn retry_outdated(
    host: String,
    generation: u64,
    client: Client,
    tx: mpsc::Sender<HostEvent>,
) {
    let Ok(Msg::Sessions { sessions }) = client.request(Msg::ListSessions).await else {
        return;
    };
    if !freshness::busy(&sessions) {
        let _ = tx.send(HostEvent::Reconnect { host, generation }).await;
    }
}

/// The `claude --version` its daemon reported at connect, if claude is there.
pub(super) fn client_claude(client: &Client) -> Option<String> {
    client
        .welcome()
        .host
        .claude
        .as_ref()
        .map(|c| c.version.clone())
}

pub(super) async fn list_sessions(client: &Client) -> io::Result<Vec<SessionInfo>> {
    match client.request(Msg::ListSessions).await? {
        Msg::Sessions { sessions } => Ok(sessions),
        other => Err(io::Error::other(format!(
            "unexpected reply to ListSessions: {other:?}"
        ))),
    }
}

/// Spawn a task that reads incoming items from `rx` and forwards them,
/// tagged with `host` and `gen`, to `tx`.
pub(super) fn spawn_reader(
    host: String,
    gen: u64,
    mut rx: mpsc::Receiver<Incoming>,
    tx: mpsc::Sender<HostEvent>,
) {
    tokio::spawn(async move {
        while let Some(item) = rx.recv().await {
            if tx
                .send(HostEvent::Incoming(host.clone(), gen, item))
                .await
                .is_err()
            {
                break;
            }
        }
    });
}

/// Spawn a task that reconnects to the local daemon (starting it if needed)
/// after `delay`; the outcome comes back tagged with `generation`.
pub(super) fn spawn_reconnect_local(delay: Duration, generation: u64, tx: mpsc::Sender<HostEvent>) {
    tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        let event = match connect_local().await {
            Ok((client, sessions)) => HostEvent::LocalReconnected {
                client,
                sessions,
                generation,
            },
            Err(_) => HostEvent::LocalReconnectFailed { generation },
        };
        let _ = tx.send(event).await;
    });
}

/// Spawn a background task that bootstraps + connects to `host`.
pub(super) fn spawn_connect(
    host: String,
    gen: u64,
    tx: mpsc::Sender<HostEvent>,
    skip_bootstrap: bool,
) {
    tokio::spawn(async move {
        do_connect(host, gen, tx, skip_bootstrap).await;
    });
}

/// Spawn a task that sleeps `delay` then connects.
pub(super) fn spawn_connect_after(
    host: String,
    gen: u64,
    delay: Duration,
    tx: mpsc::Sender<HostEvent>,
    skip_bootstrap: bool,
) {
    tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        do_connect(host, gen, tx, skip_bootstrap).await;
    });
}

pub(super) async fn do_connect(
    host: String,
    gen: u64,
    tx: mpsc::Sender<HostEvent>,
    skip_bootstrap: bool,
) {
    // Bootstrap (upload binary if needed). Skip on reconnect or when the last
    // failure was not a bootstrap failure.
    if !skip_bootstrap {
        if let Err(e) = ensure_remote(&host).await {
            let _ = tx
                .send(HostEvent::ConnectFailed {
                    host,
                    error: e,
                    generation: gen,
                })
                .await;
            return;
        }
    }
    // Connect the SSH bridge.
    match client::connect_ssh(&host).await {
        Ok(client) => {
            let sessions = match list_sessions(&client).await {
                Ok(s) => s,
                Err(e) => {
                    let _ = tx
                        .send(HostEvent::ConnectFailed {
                            host,
                            error: e.to_string(),
                            generation: gen,
                        })
                        .await;
                    return;
                }
            };
            let _ = tx
                .send(HostEvent::Connected {
                    host,
                    client,
                    sessions,
                    generation: gen,
                })
                .await;
        }
        Err(e) => {
            let _ = tx
                .send(HostEvent::ConnectFailed {
                    host,
                    error: e.to_string(),
                    generation: gen,
                })
                .await;
        }
    }
}
