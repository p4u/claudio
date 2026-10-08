//! The session manager UI that a bare `claudio` opens.
//!
//! This module is the I/O edge: it owns the terminal and the daemon
//! connection and runs the event loop. State and update logic live in
//! [`app`], rendering in [`ui`].

mod app;
mod keymap;
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

use app::{App, Effect, ReplyTo};
use state::ClientState;

/// Animation / clock tick.
const TICK: Duration = Duration::from_millis(250);
/// Minimum time between redraws (~60 fps), coalescing output bursts.
const FRAME: Duration = Duration::from_millis(16);
/// Delay between reconnection attempts.
const RECONNECT: Duration = Duration::from_secs(2);

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
    // Don't wait for a reconnect attempt that may be blocked in autostart.
    rt.shutdown_background();
    code
}

async fn main() -> ExitCode {
    let saved = ClientState::load(&paths::client_state());
    let (client, live) = match connect_and_list().await {
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
    let result = event_loop(&mut terminal, saved, client, live).await;
    restore_terminal();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("claudio: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Start the daemon if needed, connect and list its sessions.
async fn connect_and_list() -> io::Result<(Client, Vec<SessionInfo>)> {
    tokio::task::spawn_blocking(client::ensure_daemon).await.map_err(io::Error::other)??;
    let client = client::connect(&paths::daemon_socket()).await?;
    match client.request(Msg::ListSessions).await? {
        Msg::Sessions { sessions } => Ok((client, sessions)),
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

// ── Event loop ────────────────────────────────────────────────────────────────

/// The live connection, if any.
struct Conn {
    client: Option<Client>,
    incoming: Option<mpsc::Receiver<Incoming>>,
}

async fn event_loop(
    terminal: &mut Term,
    saved: ClientState,
    client: Client,
    live: Vec<SessionInfo>,
) -> io::Result<()> {
    let size = terminal.size()?;
    let mut app = App::new(size.width, size.height, client.welcome().host.home.clone(), saved.recent_dirs.clone());
    app.recover(&saved, &live);
    let mut conn = Conn { incoming: client.take_incoming(), client: Some(client) };

    let (reply_tx, mut reply_rx) = mpsc::unbounded_channel::<(ReplyTo, io::Result<Msg>)>();
    let (conn_tx, mut conn_rx) = mpsc::unbounded_channel::<(Client, Vec<SessionInfo>)>();
    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_draw = Instant::now() - FRAME;

    loop {
        for effect in app.take_effects() {
            run_effect(effect, &mut app, &conn, &reply_tx);
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
            inc = recv_incoming(&mut conn.incoming) => match inc {
                Some(Incoming::Disconnected) | None => {
                    if conn.client.take().is_some() {
                        conn.incoming = None;
                        app.on_disconnected();
                        spawn_reconnect(conn_tx.clone());
                    }
                }
                Some(inc) => app.on_incoming(inc),
            },
            Some((to, reply)) = reply_rx.recv() => app.on_reply(to, reply),
            Some((client, live)) = conn_rx.recv() => {
                conn.incoming = client.take_incoming();
                app.on_reconnected(&live, client.welcome().host.home.clone());
                conn.client = Some(client);
            }
            _ = tick.tick() => app.on_tick(),
            _ = tokio::time::sleep(wait), if app.redraw => {}
        }
    }
}

async fn recv_incoming(rx: &mut Option<mpsc::Receiver<Incoming>>) -> Option<Incoming> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

fn run_effect(
    effect: Effect,
    app: &mut App,
    conn: &Conn,
    reply_tx: &mpsc::UnboundedSender<(ReplyTo, io::Result<Msg>)>,
) {
    match effect {
        Effect::Request(msg, to) => {
            let tx = reply_tx.clone();
            match &conn.client {
                Some(client) => {
                    let reply = client.request(msg);
                    tokio::spawn(async move {
                        let _ = tx.send((to, reply.await));
                    });
                }
                None => {
                    let _ = tx.send((to, Err(io::Error::new(io::ErrorKind::NotConnected, "not connected"))));
                }
            }
        }
        Effect::Input(id, bytes) => {
            if let Some(client) = &conn.client {
                client.send_input(id, &bytes);
            }
        }
        Effect::Save => {
            if let Err(e) = app.to_state().save(&paths::client_state()) {
                app.notify(format!("could not save state: {e}"));
            }
        }
    }
}

/// Retry connecting every [`RECONNECT`] until it works, then hand the new
/// connection to the event loop.
fn spawn_reconnect(tx: mpsc::UnboundedSender<(Client, Vec<SessionInfo>)>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(RECONNECT).await;
            if let Ok(conn) = connect_and_list().await {
                let _ = tx.send(conn);
                return;
            }
        }
    });
}
