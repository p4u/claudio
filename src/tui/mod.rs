//! The session manager UI that a bare `claudio` opens.
//!
//! This module is the I/O edge: it owns the terminal and the daemon
//! connections (local + remote) and runs the event loop. State and update
//! logic live in [`app`], rendering in [`ui`].

mod app;
mod connections;
mod interaction;
mod keymap;
mod notifications;
mod proxy_state;
mod sessions;
mod state;
mod ui;
mod wizard;

use std::io::{self, Stdout};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste, EnableFocusChange,
    EnableMouseCapture, EventStream, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{cursor, execute};
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::mpsc;

use crate::client::{self, Client, Incoming};
use crate::paths;
use crate::proto::{Msg, SessionInfo};
use crate::remote::bootstrap::ensure_remote;

use app::{App, Effect, ReplyTo};
use connections::Connections;
use state::ClientState;

/// Animation / clock tick.
const TICK: Duration = Duration::from_millis(250);
/// Minimum time between redraws (~60 fps), coalescing output bursts.
const FRAME: Duration = Duration::from_millis(16);

/// Whether keyboard enhancement flags were pushed (and must be popped).
static KEYBOARD_ENHANCED: AtomicBool = AtomicBool::new(false);

type Term = Terminal<CrosstermBackend<Stdout>>;

/// Emit an OSC 9 desktop notification + single BEL to the outer terminal for a
/// session label. Written directly to stdout, between ratatui frames.
///
/// M3 fix: label is sanitized before embedding; only one BEL.
fn emit_notification(label: &str) {
    use std::io::Write;
    // sanitize_label strips C0/C1/DEL and caps at 200 chars.
    let safe = sessions::sanitize_label(label, 200);
    let msg = format!("\x1b]9;claudio: {safe} needs you\x07");
    let _ = std::io::stdout().write_all(msg.as_bytes());
    let _ = std::io::stdout().flush();
}

/// Run the manager until the user quits.
pub fn run() -> ExitCode {
    let rt = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("claudio: cannot start the async runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    let code = rt.block_on(main());
    rt.shutdown_background();
    code
}

async fn main() -> ExitCode {
    // Load config and initialize the keymap (must happen before the TUI starts).
    let cfg = crate::config::load();
    let key_notices = keymap::init(&cfg.keys);

    let saved = ClientState::load(&paths::client_state());
    let (local_client, live) = match connect_local().await {
        Ok(conn) => conn,
        Err(e) => {
            eprintln!("claudio: cannot reach the session daemon: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut terminal = match enter_terminal() {
        Ok(t) => t,
        Err(e) => {
            restore_terminal();
            eprintln!("claudio: cannot set up the terminal: {e}");
            return ExitCode::FAILURE;
        }
    };
    let result = event_loop(&mut terminal, saved, local_client, live, cfg.ui.notify, key_notices).await;
    restore_terminal();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("claudio: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Start the local daemon if needed, connect and list its sessions.
async fn connect_local() -> io::Result<(Client, Vec<SessionInfo>)> {
    tokio::task::spawn_blocking(client::ensure_daemon).await.map_err(io::Error::other)??;
    let client = client::connect(&paths::daemon_socket()).await?;
    list_sessions(&client).await.map(|s| (client, s))
}

async fn list_sessions(client: &Client) -> io::Result<Vec<SessionInfo>> {
    match client.request(Msg::ListSessions).await? {
        Msg::Sessions { sessions } => Ok(sessions),
        other => Err(io::Error::other(format!("unexpected reply to ListSessions: {other:?}"))),
    }
}

// ── Terminal setup ────────────────────────────────────────────────────────────

fn enter_terminal() -> io::Result<Term> {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        default_hook(info);
    }));
    enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen, EnableMouseCapture, EnableBracketedPaste, EnableFocusChange)?;
    if crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false) {
        execute!(out, PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES))?;
        KEYBOARD_ENHANCED.store(true, Ordering::SeqCst);
    }
    Terminal::new(CrosstermBackend::new(out))
}

/// Undo everything `enter_terminal` did. Safe to call more than once and
/// from the panic hook.
fn restore_terminal() {
    let mut out = io::stdout();
    if KEYBOARD_ENHANCED.swap(false, Ordering::SeqCst) {
        let _ = execute!(out, PopKeyboardEnhancementFlags);
    }
    let _ = execute!(
        out,
        DisableFocusChange,
        DisableBracketedPaste,
        DisableMouseCapture,
        LeaveAlternateScreen,
        cursor::Show
    );
    let _ = disable_raw_mode();
}

// ── Events ────────────────────────────────────────────────────────────────────

/// A tagged incoming item from any host's daemon.
enum HostEvent {
    /// Terminal output or event from a host's daemon, tagged with `(host, generation)`.
    /// The generation is compared against the current generation in Connections;
    /// stale events (gen < current) are dropped.
    Incoming(String, u64, Incoming),
    /// A host connection was established (via bootstrap + connect_ssh).
    Connected { host: String, client: Client, sessions: Vec<SessionInfo>, generation: u64 },
    /// A remote host connection attempt failed.
    ConnectFailed { host: String, error: String, generation: u64 },
    /// A local reconnect completed.
    LocalReconnected { client: Client, sessions: Vec<SessionInfo>, generation: u64 },
    /// A local reconnect failed; retry after delay.
    LocalReconnectFailed { generation: u64 },
    /// Proxy config fetched (or failed).
    ProxyConfig {
        profile_name: String,
        config: Option<crate::proxy::api::ConfigResponse>,
    },
    /// Proxy stats + pool health fetched (or failed).
    ProxyStats {
        profile_name: String,
        stats: Option<crate::proxy::api::StatsResponse>,
        pool: Option<crate::proxy::api::PoolHealthResponse>,
    },
}

// ── Event loop ────────────────────────────────────────────────────────────────

async fn event_loop(
    terminal: &mut Term,
    saved: ClientState,
    local_client: Client,
    live: Vec<SessionInfo>,
    notify_enabled: bool,
    key_notices: Vec<String>,
) -> io::Result<()> {
    let size = terminal.size()?;
    let local_home = local_client.welcome().host.home.clone();
    let mut app = App::new_with_config(size.width, size.height, local_home, saved.recent_dirs.clone(), notify_enabled);
    app.recover(&saved, &live);

    // Show any config parse notices in the status bar at startup.
    for notice in key_notices {
        app.notify(notice);
    }

    // M6: Create Connections with local pre-created. The local entry always
    // exists before any connection attempt, so bump_delay/reconnect_delay work
    // even for the very first failure.
    let mut conns = Connections::with_local(local_client.clone());

    // Merge all incoming streams into one tagged channel.
    let (ev_tx, mut ev_rx) = mpsc::channel::<HostEvent>(1024);

    // Start the local incoming reader, tagged with the current local generation.
    if let Some(rx) = local_client.take_incoming() {
        let gen = conns.current_generation("local");
        spawn_reader("local".to_owned(), gen, rx, ev_tx.clone());
    }

    // Start background connections to every remote host that appears in saved.
    {
        let remote_hosts: Vec<String> = saved
            .sessions
            .iter()
            .filter(|s| s.host != "local")
            .map(|s| s.host.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        for host in remote_hosts {
            // M6: ensure the entry exists before the first attempt.
            conns.ensure_host(&host);
            let gen = conns.next_generation(&host);
            spawn_connect(host, gen, ev_tx.clone(), false);
        }
    }

    let (reply_tx, mut reply_rx) = mpsc::unbounded_channel::<(ReplyTo, io::Result<Msg>)>();
    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // 60-second proxy stats refresh.
    let mut proxy_tick = tokio::time::interval(Duration::from_secs(60));
    proxy_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_draw = Instant::now() - FRAME;

    loop {
        for effect in app.take_effects() {
            run_effect(effect, &mut app, &mut conns, &reply_tx, &ev_tx);
        }
        if app.quit {
            return Ok(());
        }
        let wait = FRAME.saturating_sub(last_draw.elapsed());
        if app.redraw && wait.is_zero() {
            terminal.draw(|f| ui::draw(f, &app))?;
            app.redraw = false;
            last_draw = Instant::now();
            // Check for and emit attention notifications after drawing so
            // the OSC escape doesn't land inside a ratatui buffer update.
            app.check_notifications();
            for label in app.pending_notifs.drain(..) {
                emit_notification(&label);
            }
        }

        tokio::select! {
            ev = events.next() => match ev {
                Some(Ok(ev)) => app.on_terminal(ev),
                Some(Err(e)) => return Err(e),
                None => return Ok(()),
            },
            ev = ev_rx.recv() => match ev {
                Some(HostEvent::Incoming(host, gen, inc)) => {
                    // M6: drop stale events from replaced connections.
                    if gen < conns.current_generation(&host) {
                        continue;
                    }
                    if host == "local" {
                        match inc {
                            Incoming::Disconnected => {
                                conns.disconnect("local");
                                app.on_disconnected_local();
                                let delay = conns.reconnect_delay("local");
                                conns.bump_delay("local");
                                // M6: get generation for this reconnect attempt.
                                let new_gen = conns.next_generation("local");
                                let tx = ev_tx.clone();
                                tokio::spawn(async move {
                                    tokio::time::sleep(delay).await;
                                    match connect_local().await {
                                        Ok((client, sessions)) => {
                                            let _ = tx.send(HostEvent::LocalReconnected { client, sessions, generation: new_gen }).await;
                                        }
                                        Err(_) => {
                                            // M6: local failures retry (was silently dropped before).
                                            let _ = tx.send(HostEvent::LocalReconnectFailed { generation: new_gen }).await;
                                        }
                                    }
                                });
                            }
                            inc => app.on_incoming_from("local", inc),
                        }
                    } else {
                        match inc {
                            Incoming::Disconnected => {
                                conns.disconnect(&host);
                                app.on_disconnected_remote(&host);
                                // Reconnect with backoff.
                                let delay = conns.reconnect_delay(&host);
                                conns.bump_delay(&host);
                                // M6: bump generation so stale LocalReconnected events are dropped.
                                let new_gen = conns.next_generation(&host);
                                spawn_connect_after(host, new_gen, delay, ev_tx.clone(), true);
                            }
                            inc => app.on_incoming_from(&host, inc),
                        }
                    }
                }
                Some(HostEvent::Connected { host, client, sessions, generation }) => {
                    // M6: discard if a newer generation is already in flight.
                    if generation < conns.current_generation(&host) {
                        continue;
                    }
                    let home = client.welcome().host.home.clone();
                    conns.reset_delay(&host);
                    // Start the reader tagged with the current generation.
                    let cur_gen = conns.current_generation(&host);
                    if let Some(rx) = client.take_incoming() {
                        spawn_reader(host.clone(), cur_gen, rx, ev_tx.clone());
                    }
                    conns.connected(&host, client);
                    // Record this host in the MRU so it appears first next time.
                    crate::remote::hosts::touch(&host);
                    app.on_host_connected(&host, &home);
                    // Recover remote sessions for this host only (M1 fix).
                    app.recover_host(&host, &sessions);
                    app.redraw = true;
                }
                Some(HostEvent::ConnectFailed { host, error, generation }) => {
                    // M6: discard if a newer generation is already in flight.
                    if generation < conns.current_generation(&host) {
                        continue;
                    }
                    app.on_host_error(&host, &error);
                    conns.set_bootstrap_failed(&host, error.contains("bootstrap") || error.contains("install"));
                    // Retry with backoff.
                    let delay = conns.reconnect_delay(&host);
                    conns.bump_delay(&host);
                    let new_gen = conns.next_generation(&host);
                    // On retry: skip bootstrap only if the last attempt did NOT fail in bootstrap.
                    let skip_bootstrap = !conns.bootstrap_failed(&host);
                    spawn_connect_after(host, new_gen, delay, ev_tx.clone(), skip_bootstrap);
                }
                Some(HostEvent::LocalReconnected { client, sessions, generation }) => {
                    // M6: discard stale reconnections (a newer attempt already succeeded).
                    if generation < conns.current_generation("local") {
                        continue;
                    }
                    conns.reset_delay("local");
                    let home = client.welcome().host.home.clone();
                    let cur_gen = conns.current_generation("local");
                    if let Some(rx) = client.take_incoming() {
                        spawn_reader("local".to_owned(), cur_gen, rx, ev_tx.clone());
                    }
                    conns.connected("local", client);
                    app.on_reconnected_local(&sessions, home);
                }
                Some(HostEvent::LocalReconnectFailed { generation }) => {
                    // M6: local failure retries same as remote (was silently discarded before).
                    if generation < conns.current_generation("local") {
                        continue;
                    }
                    let delay = conns.reconnect_delay("local");
                    conns.bump_delay("local");
                    let new_gen = conns.next_generation("local");
                    let tx = ev_tx.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(delay).await;
                        match connect_local().await {
                            Ok((client, sessions)) => {
                                let _ = tx.send(HostEvent::LocalReconnected { client, sessions, generation: new_gen }).await;
                            }
                            Err(_) => {
                                let _ = tx.send(HostEvent::LocalReconnectFailed { generation: new_gen }).await;
                            }
                        }
                    });
                }
                Some(HostEvent::ProxyConfig { profile_name, config }) => {
                    if let Some(cfg) = config {
                        app.on_proxy_config(profile_name, cfg);
                    }
                }
                Some(HostEvent::ProxyStats { profile_name, stats, pool }) => {
                    app.on_proxy_stats(profile_name, stats, pool);
                }
                None => return Ok(()),
            },
            Some((to, reply)) = reply_rx.recv() => app.on_reply(to, reply),
            _ = tick.tick() => app.on_tick(),
            _ = proxy_tick.tick() => {
                // Refresh proxy stats for the active session's profile.
                let proxy_name = app.active_view().and_then(|v| v.proxy.clone());
                if let Some(name) = proxy_name {
                    app.schedule_proxy_stats(&name);
                }
            }
            _ = tokio::time::sleep(wait), if app.redraw => {}
        }
    }
}

fn run_effect(
    effect: Effect,
    app: &mut App,
    conns: &mut Connections,
    reply_tx: &mpsc::UnboundedSender<(ReplyTo, io::Result<Msg>)>,
    ev_tx: &mpsc::Sender<HostEvent>,
) {
    match effect {
        Effect::Request { host, msg, to } => {
            let tx = reply_tx.clone();
            match conns.client(&host) {
                Some(client) => {
                    let reply = client.request(msg);
                    tokio::spawn(async move {
                        let _ = tx.send((to, reply.await));
                    });
                }
                None => {
                    let _ = tx.send((
                        to,
                        Err(io::Error::new(
                            io::ErrorKind::NotConnected,
                            format!("{host}: not connected"),
                        )),
                    ));
                }
            }
        }
        Effect::Input(id, bytes) => {
            // Route to the session's host (look up from app.sessions).
            let host = app
                .sessions
                .iter()
                .find(|v| v.id == id)
                .map(|v| v.host.clone())
                .unwrap_or_else(|| "local".to_owned());
            if let Some(client) = conns.client(&host) {
                client.send_input(id, &bytes);
            }
        }
        Effect::Connect(host) => {
            // M6: Wizard Connect on already-connected host reuses the connection.
            if conns.is_connected(&host) {
                // Deliver a synthetic Connected event so the wizard advances.
                if let Some(client) = conns.client(&host) {
                    let home = client.welcome().host.home.clone();
                    app.on_host_connected(&host, &home);
                    // Don't re-spawn a reader: the existing one is still running.
                }
                return;
            }
            // M6: ensure entry exists before first attempt.
            conns.ensure_host(&host);
            let gen = conns.next_generation(&host);
            spawn_connect(host, gen, ev_tx.clone(), false);
        }
        Effect::Save => {
            if let Err(e) = app.to_state().save(&paths::client_state()) {
                app.notify(format!("could not save state: {e}"));
            }
        }
        Effect::FetchProxyConfig { profile_name } => {
            let tx = ev_tx.clone();
            let name = profile_name.clone();
            tokio::spawn(async move {
                let config = fetch_proxy_config(&name).await;
                let _ = tx.send(HostEvent::ProxyConfig { profile_name: name, config }).await;
            });
        }
        Effect::FetchProxyStats { profile_name } => {
            let tx = ev_tx.clone();
            let name = profile_name.clone();
            tokio::spawn(async move {
                let (stats, pool) = fetch_proxy_stats(&name).await;
                let _ = tx.send(HostEvent::ProxyStats { profile_name: name, stats, pool }).await;
            });
        }
    }
}

/// Fetch proxy config for a named profile. Returns `None` on any error.
async fn fetch_proxy_config(profile_name: &str) -> Option<crate::proxy::api::ConfigResponse> {
    let (url, token) = proxy_state::resolve_profile(profile_name)?;
    match crate::proxy::api::fetch_config(&url, &token).await {
        Ok(Some(cfg)) => Some(cfg),
        Ok(None) => None,
        Err(_) => None,
    }
}

/// Fetch proxy stats and pool health for a named profile.
async fn fetch_proxy_stats(
    profile_name: &str,
) -> (Option<crate::proxy::api::StatsResponse>, Option<crate::proxy::api::PoolHealthResponse>) {
    let Some((url, token)) = proxy_state::resolve_profile(profile_name) else {
        return (None, None);
    };
    let stats = crate::proxy::api::fetch_stats(&url, &token, "24h").await.ok();
    let pool = crate::proxy::api::fetch_pool_health(&url, &token).await.ok();
    (stats, pool)
}

/// Spawn a task that reads incoming items from `rx` and forwards them,
/// tagged with `host` and `gen`, to `tx`.
fn spawn_reader(host: String, gen: u64, mut rx: mpsc::Receiver<Incoming>, tx: mpsc::Sender<HostEvent>) {
    tokio::spawn(async move {
        while let Some(item) = rx.recv().await {
            if tx.send(HostEvent::Incoming(host.clone(), gen, item)).await.is_err() {
                break;
            }
        }
    });
}

/// Spawn a background task that bootstraps + connects to `host`.
fn spawn_connect(host: String, gen: u64, tx: mpsc::Sender<HostEvent>, skip_bootstrap: bool) {
    tokio::spawn(async move {
        do_connect(host, gen, tx, skip_bootstrap).await;
    });
}

/// Spawn a task that sleeps `delay` then connects.
fn spawn_connect_after(host: String, gen: u64, delay: Duration, tx: mpsc::Sender<HostEvent>, skip_bootstrap: bool) {
    tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        do_connect(host, gen, tx, skip_bootstrap).await;
    });
}

async fn do_connect(host: String, gen: u64, tx: mpsc::Sender<HostEvent>, skip_bootstrap: bool) {
    // Bootstrap (upload binary if needed). Skip on reconnect or when the last
    // failure was not a bootstrap failure.
    if !skip_bootstrap {
        if let Err(e) = ensure_remote(&host).await {
            let _ = tx.send(HostEvent::ConnectFailed { host, error: e, generation: gen }).await;
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
                        .send(HostEvent::ConnectFailed { host, error: e.to_string(), generation: gen })
                        .await;
                    return;
                }
            };
            let _ = tx.send(HostEvent::Connected { host, client, sessions, generation: gen }).await;
        }
        Err(e) => {
            let _ = tx
                .send(HostEvent::ConnectFailed { host, error: e.to_string(), generation: gen })
                .await;
        }
    }
}
