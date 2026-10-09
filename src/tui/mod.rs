//! The session manager UI that a bare `claudio` opens.
//!
//! This module is the I/O edge: it owns the terminal and the daemon
//! connections (local + remote) and runs the event loop. State and update
//! logic live in [`app`], rendering in [`ui`].

pub mod app;
mod claude_update;
mod confirm;
mod connections;
mod git_app;
mod git_view;
mod interaction;
mod keymap;
mod notifications;
mod plain;
mod proxy_state;
mod sessions;
mod state;
mod stats_view;
mod ui;
mod wizard;

use std::io::{self, Stdout};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
    EnableFocusChange, EnableMouseCapture, EventStream, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::{cursor, execute};
use futures::{FutureExt, StreamExt};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::mpsc;

use crate::client::{self, Client, Incoming};
use crate::paths;
use crate::proto::{Msg, SessionInfo};
use crate::remote::bootstrap::ensure_remote;

use app::{App, Effect, Mode, ReplyTo};
use connections::Connections;
use plain::PlainStart;
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

/// What the UI is started as.
enum Launch {
    /// The session manager, with a startup proxy override for new sessions.
    Manager(crate::proxy::ProxyChoice),
    /// `claudio --plain`: one bare session; `args` go to claude.
    Plain {
        proxy: crate::proxy::ProxyChoice,
        args: Vec<String>,
    },
}

/// Run the manager until the user quits.
pub fn run() -> ExitCode {
    run_with_proxy(crate::proxy::ProxyChoice::Default)
}

/// Like [`run`] but applies a startup proxy override to every new session.
///
/// - `Direct`  → new sessions never use a proxy (--no-proxy).
/// - `Profile` → new sessions always pre-select that profile (--proxy <name>).
/// - `Default` → wizard pre-selects the config.toml default, if any.
pub fn run_with_proxy(proxy_override: crate::proxy::ProxyChoice) -> ExitCode {
    run_launch(Launch::Manager(proxy_override))
}

/// `claudio --plain`: run claude alone, full screen, with the proxy `proxy`
/// picks, and exit with its status. The manager's help, proxy stats, history
/// and reset keys work; the rest of the keyboard is claude's.
pub fn run_plain(proxy: crate::proxy::ProxyChoice, args: Vec<String>) -> ExitCode {
    run_launch(Launch::Plain { proxy, args })
}

fn run_launch(launch: Launch) -> ExitCode {
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("claudio: cannot start the async runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    let code = rt.block_on(main(launch));
    rt.shutdown_background();
    code
}

async fn main(launch: Launch) -> ExitCode {
    // Load config and build the keymap from defaults + overrides.
    let cfg = crate::config::load();
    let mut key_notices = Vec::new();
    let (proxy_override, plain) = match launch {
        Launch::Manager(proxy) => (proxy, None),
        Launch::Plain { proxy, args } => {
            let cwd = std::env::current_dir()
                .map(|d| d.to_string_lossy().into_owned())
                .unwrap_or_else(|_| ".".to_owned());
            let start = PlainStart {
                id: uuid::Uuid::new_v4(),
                args,
                cwd,
            };
            (proxy, Some(start))
        }
    };
    let km = match plain {
        Some(_) => keymap::Keymap::build_plain(&cfg.keys, &mut key_notices),
        None => keymap::Keymap::build(&cfg.keys, &mut key_notices),
    };
    let plain_id = plain.as_ref().map(|p| p.id);

    // The manager's session list; `--plain` neither reads nor writes it.
    let saved = match plain {
        Some(_) => ClientState::default(),
        None => ClientState::load(&paths::client_state()),
    };
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
    let mut conns = Connections::with_local(local_client.clone());
    // A panic must still reach the cleanup below, so catch it and re-raise.
    let result = std::panic::AssertUnwindSafe(event_loop(
        &mut terminal,
        &mut conns,
        saved,
        local_client,
        live,
        cfg.ui.notify,
        cfg.update.check,
        cfg.claude,
        km,
        key_notices,
        proxy_override,
        plain,
    ))
    .catch_unwind()
    .await;
    restore_terminal();
    // However the UI ended, a plain session dies with it: nothing lingers in
    // the daemon, and a later manager has nothing to recover. (Killing an
    // already closed session is a harmless "no such session".)
    if let Some(id) = plain_id {
        if let Some(client) = conns.client("local") {
            let kill = client.request(Msg::Kill { id });
            let _ = tokio::time::timeout(Duration::from_secs(3), kill).await;
        }
    }
    match result {
        Err(panic) => std::panic::resume_unwind(panic),
        Ok(Ok(exit)) => {
            if let Some(message) = &exit.message {
                eprintln!("claudio: {message}");
            }
            if plain_id.is_some() {
                exit.exit_code()
            } else {
                ExitCode::SUCCESS
            }
        }
        Ok(Err(e)) => {
            eprintln!("claudio: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Start the local daemon if needed, connect and list its sessions.
async fn connect_local() -> io::Result<(Client, Vec<SessionInfo>)> {
    tokio::task::spawn_blocking(client::ensure_daemon)
        .await
        .map_err(io::Error::other)??;
    let client = client::connect(&paths::daemon_socket()).await?;
    list_sessions(&client).await.map(|s| (client, s))
}

/// The `claude --version` its daemon reported at connect, if claude is there.
fn client_claude(client: &Client) -> Option<String> {
    client.welcome().host.claude.as_ref().map(|c| c.version.clone())
}

async fn list_sessions(client: &Client) -> io::Result<Vec<SessionInfo>> {
    match client.request(Msg::ListSessions).await? {
        Msg::Sessions { sessions } => Ok(sessions),
        other => Err(io::Error::other(format!(
            "unexpected reply to ListSessions: {other:?}"
        ))),
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
    execute!(
        out,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste,
        EnableFocusChange
    )?;
    if crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false) {
        execute!(
            out,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
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
    Connected {
        host: String,
        client: Client,
        sessions: Vec<SessionInfo>,
        generation: u64,
    },
    /// A remote host connection attempt failed.
    ConnectFailed {
        host: String,
        error: String,
        generation: u64,
    },
    /// A local reconnect completed.
    LocalReconnected {
        client: Client,
        sessions: Vec<SessionInfo>,
        generation: u64,
    },
    /// A local reconnect failed; retry after delay.
    LocalReconnectFailed { generation: u64 },
    /// Proxy config fetched (or failed).
    ProxyConfig {
        profile_name: String,
        config: Option<crate::proxy::api::ConfigResponse>,
    },
    /// Proxy stats, pool health and model catalogue fetched (or failed).
    ProxyStats {
        profile_name: String,
        fetch: proxy_state::ProxyFetch,
    },
    /// Background upgrade check completed. `Some(tag)` means a newer version is
    /// available; `None` means we are up to date (or the check failed silently).
    UpgradeAvailable(Option<String>),
    /// The newest claude version on its release channel.
    ClaudeLatest(String),
}

// ── Event loop ────────────────────────────────────────────────────────────────

/// Resolves when the terminal's session is hung up or the process is asked to
/// terminate (never, where there are no such signals).
#[cfg(unix)]
async fn shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    let (Ok(mut hup), Ok(mut term)) = (
        signal(SignalKind::hangup()),
        signal(SignalKind::terminate()),
    ) else {
        return std::future::pending().await;
    };
    tokio::select! {
        _ = hup.recv() => {}
        _ = term.recv() => {}
    }
}

#[cfg(not(unix))]
async fn shutdown_signal() {
    std::future::pending::<()>().await
}

#[allow(clippy::too_many_arguments)]
async fn event_loop(
    terminal: &mut Term,
    conns: &mut Connections,
    saved: ClientState,
    local_client: Client,
    live: Vec<SessionInfo>,
    notify_enabled: bool,
    update_check_enabled: bool,
    claude_policy: crate::config::ClaudeSection,
    km: keymap::Keymap,
    key_notices: Vec<String>,
    proxy_override: crate::proxy::ProxyChoice,
    plain: Option<PlainStart>,
) -> io::Result<plain::Exit> {
    let size = terminal.size()?;
    let local_home = local_client.welcome().host.home.clone();
    let mut app = App::new_with_proxy(
        size.width,
        size.height,
        local_home,
        saved.recent_dirs.clone(),
        notify_enabled && plain.is_none(),
        km,
        proxy_override,
    );
    match plain {
        Some(start) => app.start_plain(start),
        None => {
            app.claude_skipped = saved.claude_skipped.clone();
            app.set_local_claude(client_claude(&local_client));
            app.recover(&saved, &live);

            // Subscribe to host stats pushes (CPU/mem sparklines).
            // Old daemons reply Error; we treat that as "unsupported" and the
            // sparklines just stay empty.
            app.effects.push(Effect::Request {
                host: "local".to_owned(),
                msg: Msg::SubscribeHostStats,
                to: ReplyTo::Ack("host_stats"),
            });
        }
    }
    let manager = app.mode == Mode::Manager;

    // Show any config parse notices in the status bar at startup.
    for notice in key_notices {
        app.notify(notice);
    }

    // M6: `conns` has local pre-created (see `main`). The local entry always
    // exists before any connection attempt, so bump_delay/reconnect_delay work
    // even for the very first failure.

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

    // Kick off a non-blocking upgrade check. The result arrives as
    // HostEvent::UpgradeAvailable, which sets app.upgrade_notice.
    if manager {
        let tx = ev_tx.clone();
        tokio::spawn(async move {
            let result = crate::upgrade::check_once(update_check_enabled).await;
            let _ = tx.send(HostEvent::UpgradeAvailable(result)).await;
        });
    }

    // And, at most once a day, compare the local claude with its release
    // channel. The result arrives as HostEvent::ClaudeLatest.
    if manager && claude_policy.update_check != crate::config::UpdatePolicy::Off {
        let tx = ev_tx.clone();
        tokio::spawn(async move {
            if let Some(latest) = crate::claude::update::check_due().await {
                let _ = tx.send(HostEvent::ClaudeLatest(latest)).await;
            }
        });
    }
    app.claude_policy = claude_policy;

    let (reply_tx, mut reply_rx) = mpsc::unbounded_channel::<(ReplyTo, io::Result<Msg>)>();
    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // 60-second proxy stats refresh.
    let mut proxy_tick = tokio::time::interval(Duration::from_secs(60));
    proxy_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_draw = Instant::now() - FRAME;
    // A plain session must not outlive its terminal.
    let mut shutdown = Box::pin(shutdown_signal());

    loop {
        for effect in app.take_effects() {
            run_effect(effect, &mut app, conns, &reply_tx, &ev_tx);
        }
        if app.quit {
            return Ok(std::mem::take(&mut app.exit));
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
                None => return Ok(Default::default()),
            },
            _ = &mut shutdown, if !manager => return Ok(Default::default()),
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
                            Incoming::HostStats { cpu_pct, mem_used, mem_total, .. } => {
                                app.on_host_stats("local".to_owned(), cpu_pct, mem_used, mem_total);
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
                            Incoming::HostStats { cpu_pct, mem_used, mem_total, .. } => {
                                app.on_host_stats(host, cpu_pct, mem_used, mem_total);
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
                    let remote_claude = client_claude(&client);
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
                    app.check_remote_claude(&host, remote_claude.as_deref());
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
                    app.set_local_claude(client_claude(&client));
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
                Some(HostEvent::ProxyStats { profile_name, fetch }) => {
                    app.on_proxy_stats(profile_name, fetch);
                }
                Some(HostEvent::UpgradeAvailable(tag)) => {
                    if let Some(t) = tag {
                        app.upgrade_notice = Some(t);
                        app.redraw = true;
                    }
                }
                Some(HostEvent::ClaudeLatest(latest)) => app.check_local_claude(&latest),
                None => return Ok(Default::default()),
            },
            Some((to, reply)) = reply_rx.recv() => app.on_reply(to, reply),
            _ = tick.tick() => app.on_tick(),
            _ = proxy_tick.tick() => {
                // Refresh proxy stats for the active session's profile (they
                // feed the status bar, which `--plain` does not have).
                let proxy_name = app.active_view().and_then(|v| v.proxy.clone());
                if let Some(name) = proxy_name.filter(|_| manager) {
                    app.schedule_proxy_stats(&name, vec![stats_view::Window::H24]);
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
                let _ = tx
                    .send(HostEvent::ProxyConfig {
                        profile_name: name,
                        config,
                    })
                    .await;
            });
        }
        Effect::FetchProxyStats {
            profile_name,
            windows,
            models,
        } => {
            let tx = ev_tx.clone();
            tokio::spawn(async move {
                let fetch = fetch_proxy_stats(&profile_name, &windows, models).await;
                let _ = tx
                    .send(HostEvent::ProxyStats {
                        profile_name,
                        fetch,
                    })
                    .await;
            });
        }
        Effect::SpawnWithProxy {
            host,
            msg,
            proxy_name,
            to,
        } => {
            // Resolve the client now (synchronous, cheap) so we can move it
            // into the async task. Client is Clone + Send + 'static.
            let client = conns.client(&host).cloned();
            let tx = reply_tx.clone();
            tokio::spawn(async move {
                // Fetch proxy config with a 5 s timeout so the first spawn
                // uses the real model/rate-limit overrides, not fallback defaults.
                let cfg = tokio::time::timeout(
                    Duration::from_secs(5),
                    fetch_proxy_config(&proxy_name),
                )
                .await
                .ok()
                .flatten();

                // Build env from profile + (possibly fresh) config.
                let env = crate::proxy::resolve::proxy_env_for(
                    Some(&proxy_name),
                    cfg.as_ref(),
                );
                match env {
                    Err(e) => {
                        let _ = tx.send((
                            to,
                            Err(io::Error::other(format!("proxy env error: {e}"))),
                        ));
                    }
                    Ok(env) => {
                        match client {
                            Some(c) => {
                                let reply = c.request(msg.with_env(env));
                                let _ = tx.send((to, reply.await));
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
                }
            });
        }
    }
}

/// Fetch proxy config for a named profile. Returns `None` on any error.
async fn fetch_proxy_config(profile_name: &str) -> Option<crate::proxy::api::ConfigResponse> {
    let (url, token) = crate::proxy::resolve::resolve_profile(profile_name)?;
    match crate::proxy::api::fetch_config(&url, &token).await {
        Ok(Some(cfg)) => Some(cfg),
        Ok(None) => None,
        Err(_) => None,
    }
}

/// Fetch proxy stats for `windows`, pool health, and (when `models`) the
/// model catalogue for a named profile. Requests run sequentially to stay
/// well inside the proxy's per-user rate limit (1 req/s, burst 10).
async fn fetch_proxy_stats(
    profile_name: &str,
    windows: &[stats_view::Window],
    models: bool,
) -> proxy_state::ProxyFetch {
    use crate::proxy::api;
    let mut out = proxy_state::ProxyFetch::default();
    let Some((url, token)) = crate::proxy::resolve::resolve_profile(profile_name) else {
        let err = format!("proxy profile '{profile_name}' not found");
        out.stats = windows.iter().map(|w| (*w, Err(err.clone()))).collect();
        return out;
    };
    for w in windows {
        let result = api::fetch_stats(&url, &token, w.param())
            .await
            .map_err(describe_api_error);
        out.stats.push((*w, result));
    }
    out.pool = api::fetch_pool_health(&url, &token).await.ok();
    if models {
        out.models = api::fetch_models(&url, &token).await.ok();
    }
    out
}

/// A one-line, user-facing description of a proxy API error.
fn describe_api_error(e: crate::proxy::api::ApiError) -> String {
    use crate::proxy::api::ApiError;
    match e {
        ApiError::Http(s) if s.as_u16() == 403 => {
            "stats need a user token (admin and anonymous tokens are refused)".to_owned()
        }
        ApiError::Http(s) if s.as_u16() == 404 => {
            "this proxy has no claudio API (upgrade claude-proxy)".to_owned()
        }
        ApiError::Http(s) if s.as_u16() == 429 => {
            "rate limited by the proxy; wait a few seconds".to_owned()
        }
        other => other.to_string(),
    }
}

/// Spawn a task that reads incoming items from `rx` and forwards them,
/// tagged with `host` and `gen`, to `tx`.
fn spawn_reader(
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

/// Spawn a background task that bootstraps + connects to `host`.
fn spawn_connect(host: String, gen: u64, tx: mpsc::Sender<HostEvent>, skip_bootstrap: bool) {
    tokio::spawn(async move {
        do_connect(host, gen, tx, skip_bootstrap).await;
    });
}

/// Spawn a task that sleeps `delay` then connects.
fn spawn_connect_after(
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

async fn do_connect(host: String, gen: u64, tx: mpsc::Sender<HostEvent>, skip_bootstrap: bool) {
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
