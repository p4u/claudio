//! The session manager UI that a bare `claudio` opens.
//!
//! This module is the I/O edge: it owns the terminal and the daemon
//! connections (local + remote) and runs the event loop. State and update
//! logic live in [`app`], rendering in [`ui`].

mod app;
mod keymap;
mod state;
mod ui;
mod wizard;

use std::collections::HashMap;
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
use state::ClientState;

/// Animation / clock tick.
const TICK: Duration = Duration::from_millis(250);
/// Minimum time between redraws (~60 fps), coalescing output bursts.
const FRAME: Duration = Duration::from_millis(16);
/// Initial reconnect delay.
const RECONNECT_INIT: Duration = Duration::from_secs(1);
/// Maximum reconnect delay.
const RECONNECT_MAX: Duration = Duration::from_secs(30);

/// Whether keyboard enhancement flags were pushed (and must be popped).
static KEYBOARD_ENHANCED: AtomicBool = AtomicBool::new(false);

type Term = Terminal<CrosstermBackend<Stdout>>;

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
    let result = event_loop(&mut terminal, saved, local_client, live).await;
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

// ── Multi-host connection management ─────────────────────────────────────────

/// Connection state for one host.
struct HostConn {
    client: Option<Client>,
    /// Current reconnect interval (doubles on each failure, up to the max).
    reconnect_delay: Duration,
}

impl HostConn {
    fn connected(client: Client) -> Self {
        HostConn { client: Some(client), reconnect_delay: RECONNECT_INIT }
    }
}

/// All-hosts connection map. "local" is always present.
struct Connections {
    map: HashMap<String, HostConn>,
}

impl Connections {
    fn with_local(client: Client) -> Self {
        let mut map = HashMap::new();
        map.insert("local".to_owned(), HostConn::connected(client));
        Connections { map }
    }

    fn client(&self, host: &str) -> Option<&Client> {
        self.map.get(host).and_then(|c| c.client.as_ref())
    }

    fn add(&mut self, host: String, client: Client) {
        self.map.insert(host, HostConn::connected(client));
    }

    fn disconnect(&mut self, host: &str) {
        if let Some(conn) = self.map.get_mut(host) {
            conn.client = None;
            conn.reconnect_delay = RECONNECT_INIT;
        }
    }

    fn reconnect_delay(&self, host: &str) -> Duration {
        self.map.get(host).map(|c| c.reconnect_delay).unwrap_or(RECONNECT_INIT)
    }

    fn bump_delay(&mut self, host: &str) {
        if let Some(conn) = self.map.get_mut(host) {
            conn.reconnect_delay = (conn.reconnect_delay * 2).min(RECONNECT_MAX);
        }
    }

    fn reset_delay(&mut self, host: &str) {
        if let Some(conn) = self.map.get_mut(host) {
            conn.reconnect_delay = RECONNECT_INIT;
        }
    }
}

// ── Events ────────────────────────────────────────────────────────────────────

/// A tagged incoming item from any host's daemon.
enum HostEvent {
    /// Terminal output or event from a host's daemon.
    Incoming(String, Incoming),
    /// A host connection was established (via bootstrap + connect_ssh).
    Connected { host: String, client: Client, sessions: Vec<SessionInfo> },
    /// A remote host connection attempt failed.
    ConnectFailed { host: String, error: String },
    /// A local reconnect completed.
    LocalReconnected { client: Client, sessions: Vec<SessionInfo> },
}

// ── Event loop ────────────────────────────────────────────────────────────────

async fn event_loop(
    terminal: &mut Term,
    saved: ClientState,
    local_client: Client,
    live: Vec<SessionInfo>,
) -> io::Result<()> {
    let size = terminal.size()?;
    let local_home = local_client.welcome().host.home.clone();
    let mut app = App::new(size.width, size.height, local_home, saved.recent_dirs.clone());
    app.recover(&saved, &live);

    let mut conns = Connections::with_local(local_client.clone());

    // Merge all incoming streams into one tagged channel.
    let (ev_tx, mut ev_rx) = mpsc::channel::<HostEvent>(1024);

    // Start the local incoming reader.
    if let Some(rx) = local_client.take_incoming() {
        spawn_reader("local".to_owned(), rx, ev_tx.clone());
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
            spawn_connect(host, ev_tx.clone(), false);
        }
    }

    let (reply_tx, mut reply_rx) = mpsc::unbounded_channel::<(ReplyTo, io::Result<Msg>)>();
    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
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
        }

        tokio::select! {
            ev = events.next() => match ev {
                Some(Ok(ev)) => app.on_terminal(ev),
                Some(Err(e)) => return Err(e),
                None => return Ok(()),
            },
            ev = ev_rx.recv() => match ev {
                Some(HostEvent::Incoming(host, inc)) => {
                    if host == "local" {
                        match inc {
                            Incoming::Disconnected => {
                                conns.disconnect("local");
                                app.on_disconnected();
                                let tx = ev_tx.clone();
                                let delay = conns.reconnect_delay("local");
                                conns.bump_delay("local");
                                tokio::spawn(async move {
                                    tokio::time::sleep(delay).await;
                                    match connect_local().await {
                                        Ok((client, sessions)) => {
                                            let _ = tx.send(HostEvent::LocalReconnected { client, sessions }).await;
                                        }
                                        Err(_) => {}
                                    }
                                });
                            }
                            inc => app.on_incoming(inc),
                        }
                    } else {
                        match inc {
                            Incoming::Disconnected => {
                                conns.disconnect(&host);
                                // Mark remote sessions as reconnecting.
                                for v in &mut app.sessions {
                                    if v.host == host {
                                        v.state = crate::proto::SessionState::Unknown;
                                        v.attached = false;
                                    }
                                }
                                app.redraw = true;
                                // Reconnect with backoff.
                                let delay = conns.reconnect_delay(&host);
                                conns.bump_delay(&host);
                                spawn_connect_after(host, delay, ev_tx.clone(), true);
                            }
                            inc => app.on_incoming(inc),
                        }
                    }
                }
                Some(HostEvent::Connected { host, client, sessions }) => {
                    let home = client.welcome().host.home.clone();
                    conns.reset_delay(&host);
                    // Start the reader.
                    if let Some(rx) = client.take_incoming() {
                        spawn_reader(host.clone(), rx, ev_tx.clone());
                    }
                    conns.add(host.clone(), client);
                    // Record this host in the MRU so it appears first next time.
                    crate::remote::hosts::touch(&host);
                    app.on_host_connected(&host, &home);
                    // Recover remote sessions.
                    let saved_for_host = ClientState {
                        sessions: saved.sessions.iter().filter(|s| s.host == host).cloned().collect(),
                        ..Default::default()
                    };
                    app.recover(&saved_for_host, &sessions);
                    app.redraw = true;
                }
                Some(HostEvent::ConnectFailed { host, error }) => {
                    app.on_host_error(&host, &error);
                    // Retry with backoff.
                    let delay = conns.reconnect_delay(&host);
                    conns.bump_delay(&host);
                    spawn_connect_after(host, delay, ev_tx.clone(), true);
                }
                Some(HostEvent::LocalReconnected { client, sessions }) => {
                    conns.reset_delay("local");
                    let home = client.welcome().host.home.clone();
                    if let Some(rx) = client.take_incoming() {
                        spawn_reader("local".to_owned(), rx, ev_tx.clone());
                    }
                    conns.add("local".to_owned(), client);
                    app.on_reconnected(&sessions, home);
                }
                None => return Ok(()),
            },
            Some((to, reply)) = reply_rx.recv() => app.on_reply(to, reply),
            _ = tick.tick() => app.on_tick(),
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
            spawn_connect(host, ev_tx.clone(), false);
        }
        Effect::Save => {
            if let Err(e) = app.to_state().save(&paths::client_state()) {
                app.notify(format!("could not save state: {e}"));
            }
        }
    }
}

/// Spawn a task that reads incoming items from `rx` and forwards them,
/// tagged with `host`, to `tx`.
fn spawn_reader(host: String, mut rx: mpsc::Receiver<Incoming>, tx: mpsc::Sender<HostEvent>) {
    tokio::spawn(async move {
        while let Some(item) = rx.recv().await {
            if tx.send(HostEvent::Incoming(host.clone(), item)).await.is_err() {
                break;
            }
        }
    });
}

/// Spawn a background task that bootstraps + connects to `host`.
fn spawn_connect(host: String, tx: mpsc::Sender<HostEvent>, is_reconnect: bool) {
    tokio::spawn(async move {
        do_connect(host, tx, is_reconnect).await;
    });
}

/// Spawn a task that sleeps `delay` then connects.
fn spawn_connect_after(host: String, delay: Duration, tx: mpsc::Sender<HostEvent>, is_reconnect: bool) {
    tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        do_connect(host, tx, is_reconnect).await;
    });
}

async fn do_connect(host: String, tx: mpsc::Sender<HostEvent>, is_reconnect: bool) {
    // Bootstrap (upload binary if needed).
    if !is_reconnect {
        if let Err(e) = ensure_remote(&host).await {
            let _ = tx.send(HostEvent::ConnectFailed { host, error: e }).await;
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
                        .send(HostEvent::ConnectFailed { host, error: e.to_string() })
                        .await;
                    return;
                }
            };
            let _ = tx.send(HostEvent::Connected { host, client, sessions }).await;
        }
        Err(e) => {
            let _ = tx
                .send(HostEvent::ConnectFailed { host, error: e.to_string() })
                .await;
        }
    }
}
