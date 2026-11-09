//! The session manager UI that a bare `claudio` opens.
//!
//! This module is the I/O edge: it owns the terminal and the daemon
//! connections (local + remote) and runs the event loop. State and update
//! logic live in [`app`], rendering in [`ui`].

pub mod app;
mod builtins;
mod claude_update;
mod confirm;
mod extensions_view;
mod fmt;
mod git_app;
mod git_view;
mod hosts;
use hosts::{
    client_claude, connect_local, on_connected, on_local_reconnected, retry_outdated_hosts,
    spawn_connect, spawn_connect_after, spawn_reader, spawn_reconnect_local,
};
mod interaction;
mod keymap;
mod leader;
mod notifications;
mod plain;
pub(crate) mod plugin_setup;
mod plugins_glue;
mod proxy_glue;
mod proxy_state;
mod reader;
mod selection;
pub(crate) mod sessions;
pub(crate) mod stats_view;

/// `recent` without the local directories that no longer exist, so the
/// wizard never offers a deleted directory (the check is I/O, so it happens
/// here rather than in the pure wizard).
fn existing_local_dirs(
    mut recent: std::collections::HashMap<String, Vec<String>>,
) -> std::collections::HashMap<String, Vec<String>> {
    if let Some(dirs) = recent.get_mut("local") {
        dirs.retain(|d| std::path::Path::new(d).is_dir());
    }
    recent
}

#[cfg(test)]
mod test_support;
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

use crate::client::{Client, Incoming};
use crate::paths;
use crate::proto::{Msg, SessionInfo};

use crate::app::connections::Connections;
use crate::app::extensions;
use crate::app::snippets;
use crate::app::state::ClientState;
use crate::plugins::state as plugin_state;
use app::{App, AppConfig, Effect, Mode, ReplyTo};
use plain::PlainStart;

/// Animation / clock tick.
const TICK: Duration = Duration::from_millis(250);
/// Minimum time between redraws (~60 fps), coalescing output bursts.
const FRAME: Duration = Duration::from_millis(16);
/// How often a daemon left outdated (its sessions were busy) is retried.
const OUTDATED_RETRY: Duration = Duration::from_secs(120);

/// Whether keyboard enhancement flags were pushed (and must be popped).
static KEYBOARD_ENHANCED: AtomicBool = AtomicBool::new(false);

type Term = Terminal<CrosstermBackend<Stdout>>;

/// Emit an OSC 9 desktop notification + single BEL to the outer terminal for a
/// session label. Written directly to stdout, between ratatui frames.
///
/// The label comes from claude (its title) and is sanitized first, so it
/// cannot smuggle escape sequences into the outer terminal.
fn emit_notification(label: &str) {
    use std::io::Write;
    let safe = crate::app::label::sanitize_label(label, 200);
    let msg = format!("\x1b]9;claudio: {safe} needs you\x07");
    let _ = std::io::stdout().write_all(msg.as_bytes());
    let _ = std::io::stdout().flush();
}

/// Hand `text` to the outer terminal's clipboard with OSC 52, written to
/// stdout between ratatui frames like [`emit_notification`]. The terminal
/// must allow it (tmux: `set -g set-clipboard on`).
fn copy_to_clipboard(text: &str) {
    use std::io::Write;
    let mut out = std::io::stdout();
    let _ = out.write_all(selection::osc52(text).as_bytes());
    let _ = out.flush();
}

/// What the UI is started as.
enum Entry {
    /// The session manager, with a startup proxy override for new sessions.
    Manager {
        proxy: crate::proxy::ProxyChoice,
        first_party: bool,
    },
    /// `claudio --plain`: one bare session; `args` go to claude.
    Plain {
        proxy: crate::proxy::ProxyChoice,
        first_party: bool,
        args: Vec<String>,
    },
}

/// What the app does first, once made.
enum Start {
    /// Recover the manager's tabs from state.json and the local daemon.
    Manager {
        saved: ClientState,
        live: Vec<SessionInfo>,
    },
    /// Start the one `--plain` session.
    Plain(PlainStart),
}

/// Run the manager until the user quits.
pub fn run() -> ExitCode {
    run_with_proxy(crate::proxy::ProxyChoice::Default, false)
}

/// Like [`run`] but applies a startup proxy override to every new session.
///
/// - `Direct`  → new sessions never use a proxy (--no-proxy).
/// - `Profile` → new sessions always pre-select that profile (--proxy <name>).
/// - `Default` → wizard pre-selects the config.toml default, if any.
///
/// `first_party` (--first-party) makes every session of this run first-party.
pub fn run_with_proxy(proxy_override: crate::proxy::ProxyChoice, first_party: bool) -> ExitCode {
    run_launch(Entry::Manager {
        proxy: proxy_override,
        first_party,
    })
}

/// `claudio --plain`: run claude alone, full screen, with the proxy `proxy`
/// picks, and exit with its status. The manager's help, proxy stats, history
/// and reset keys work; the rest of the keyboard is claude's.
pub fn run_plain(
    proxy: crate::proxy::ProxyChoice,
    first_party: bool,
    args: Vec<String>,
) -> ExitCode {
    run_launch(Entry::Plain {
        proxy,
        first_party,
        args,
    })
}

fn run_launch(entry: Entry) -> ExitCode {
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
    let code = rt.block_on(main(entry));
    rt.shutdown_background();
    code
}

async fn main(entry: Entry) -> ExitCode {
    // Load config and build the keymap from defaults + overrides.
    let config = crate::config::load();
    let mut key_notices = Vec::new();
    let (proxy_override, first_party, plain) = match entry {
        Entry::Manager { proxy, first_party } => (proxy, first_party, None),
        Entry::Plain {
            proxy,
            first_party,
            args,
        } => {
            let cwd = std::env::current_dir()
                .map(|d| d.to_string_lossy().into_owned())
                .unwrap_or_else(|_| ".".to_owned());
            let start = PlainStart {
                id: uuid::Uuid::new_v4(),
                args,
                cwd,
            };
            (proxy, first_party, Some(start))
        }
    };
    let keymap = match plain {
        Some(_) => keymap::Keymap::build_plain(&config.keys, &mut key_notices),
        None => keymap::Keymap::build(&config.keys, &mut key_notices),
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
    let (mut terminal, size) = match enter_terminal() {
        Ok(t) => t,
        Err(e) => {
            restore_terminal();
            eprintln!("claudio: cannot set up the terminal: {e}");
            return ExitCode::FAILURE;
        }
    };
    let (proxy_profiles, proxy_default) = crate::proxy::resolve::load_proxy_profiles();
    // The plugin lists cached for the profiles there are; fetched again
    // once the loop runs.
    let cached_plugins: Vec<_> = crate::proxy::plugins::load_all()
        .into_iter()
        .filter(|l| proxy_profiles.contains(&l.profile))
        .collect();
    let registry = extension_registry(&config, &cached_plugins, &mut key_notices);
    let mut app = App::new(AppConfig {
        mode: if plain.is_some() {
            Mode::Plain
        } else {
            Mode::Manager
        },
        size,
        home: local_client.welcome().host.home.clone(),
        keymap,
        notify: config.ui.notify && plain.is_none(),
        copy_on_select: config.ui.copy_on_select,
        proxy_override,
        first_party,
        proxy_profiles,
        proxy_default,
        claude: config.claude,
        local_claude: client_claude(&local_client),
        recent_dirs: existing_local_dirs(saved.recent_dirs.clone()),
        claude_skipped: saved.claude_skipped.clone(),
        // `--plain` has no wizard to offer them in.
        ssh_hosts: match plain {
            Some(_) => Vec::new(),
            None => crate::remote::hosts::candidates(),
        },
    });
    app.extensions = registry;
    app.load_plugins(
        cached_plugins,
        config.extensions.clone(),
        config.plugins.clone(),
    );
    app.on_plugin_host("local");
    app.check_local_install();
    let start = match plain {
        Some(start) => Start::Plain(start),
        None => Start::Manager { saved, live },
    };
    let mut conns = Connections::with_local(local_client.clone());
    // A panic must still reach the cleanup below, so catch it and re-raise.
    let result = std::panic::AssertUnwindSafe(event_loop(
        &mut terminal,
        &mut conns,
        app,
        start,
        local_client,
        config.update.check,
        key_notices,
    ))
    .catch_unwind()
    .await;
    restore_terminal();
    finish(result, plain_id, &conns).await
}

/// After the UI: kill a plain session (it dies with the UI: nothing lingers
/// in the daemon, and a later manager has nothing to recover), re-raise a
/// panic, report why the UI ended, and give `--plain` claude's status.
async fn finish(
    result: std::thread::Result<io::Result<plain::Exit>>,
    plain_id: Option<crate::proto::SessionId>,
    conns: &Connections,
) -> ExitCode {
    // (Killing an already closed session is a harmless "no such session".)
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

/// The extension registry this run starts with: the built-ins and the
/// external extensions (read once, now) with config.toml's choices; a
/// cached plugin's choice is left for the plugins. A lens the caps leave
/// out is a notice.
fn extension_registry(
    config: &crate::config::Config,
    cached_plugins: &[crate::proxy::plugins::PluginList],
    key_notices: &mut Vec<String>,
) -> extensions::Registry {
    // External extensions are read once, at start.
    let external = extensions::manifest::load(&extensions::manifest::dir(), key_notices);
    // A plugin's saved choice is no unknown name: the plugins come later. A
    // built-in's or external's choice stays, whatever a plugin is named.
    let builtins = extensions::Registry::default();
    let known = |name: &str| {
        builtins.extensions.iter().any(|e| e.name == name)
            || external.iter().any(|e| e.name == name)
    };
    let builtin_choices: std::collections::BTreeMap<String, bool> = config
        .extensions
        .iter()
        .filter(|(name, _)| {
            known(name)
                || !cached_plugins
                    .iter()
                    .any(|l| l.plugins.iter().any(|p| &p.name == *name))
        })
        .map(|(name, on)| (name.clone(), *on))
        .collect();
    let registry =
        extensions::Registry::new(&builtin_choices, config.claude.latex, external, key_notices);
    let dropped = registry.dropped_lenses();
    if !dropped.is_empty() {
        key_notices.push(format!(
            "extensions: lenses {} left out of the session context (caps)",
            dropped.join(", ")
        ));
    }
    registry
}

// ── Terminal setup ────────────────────────────────────────────────────────────

/// Set the terminal up for the UI; returns it with its size `(width, height)`.
fn enter_terminal() -> io::Result<(Term, (u16, u16))> {
    std::panic::set_hook(panic_hook(restore_terminal, std::panic::take_hook()));
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
    let terminal = Terminal::new(CrosstermBackend::new(out))?;
    let size = terminal.size()?;
    Ok((terminal, (size.width, size.height)))
}

type PanicHook = Box<dyn Fn(&std::panic::PanicHookInfo<'_>) + Sync + Send + 'static>;

/// The UI's panic hook: put the terminal back (`restore`), then report the
/// panic as `default` would.
fn panic_hook(restore: fn(), default: PanicHook) -> PanicHook {
    Box::new(move |info| {
        restore();
        default(info);
    })
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
    /// Drop `host`'s connection and connect again: its outdated daemon has
    /// become idle (see [`retry_outdated`]).
    Reconnect { host: String, generation: u64 },
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
    /// The credential claude-proxy reports for a session (or why not).
    SessionCredential {
        session: crate::proto::SessionId,
        claude_session_id: String,
        result: Result<Option<crate::proxy::api::SessionCredential>, String>,
    },
    /// The proxy answered a subscription switch request.
    SubscriptionSwitch {
        session: crate::proto::SessionId,
        result: Result<crate::proxy::api::SwitchOutcome, crate::proxy::api::SwitchError>,
    },
    /// Background upgrade check completed. `Some(tag)` means a newer version is
    /// available; `None` means we are up to date (or the check failed silently).
    UpgradeAvailable(Option<String>),
    /// The newest claude version on its release channel.
    ClaudeLatest(String),
    /// The answers of a reader view were parsed (or why not).
    AnswersRendered {
        gen: u64,
        id: crate::proto::SessionId,
        answers: Result<Vec<reader::RenderedAnswer>, String>,
    },
    /// A notice from the extension host (an extension was disabled).
    ExtensionNotice(String),
    /// The snippets (name, text) read for the action started under `gen`,
    /// from `dir`.
    SnippetsLoaded {
        gen: u64,
        dir: String,
        result: Result<Vec<(String, String)>, String>,
    },
    /// A snippet was saved (its name), or why not; `cut` as asked.
    SnippetSaved {
        result: Result<String, String>,
        cut: bool,
    },
    /// An external action started under `gen` ended.
    ActionDone {
        gen: u64,
        result: Result<extensions::ActionResult, String>,
    },
    /// A profile's plugin list was fetched (and the cache updated), or not.
    Plugins {
        profile: String,
        fetch: plugin_state::PluginsFetch,
    },
    /// Plugin `name`'s manifest at hash `sha256` was fetched and parsed, or
    /// why not.
    Manifest {
        name: String,
        sha256: String,
        result: Result<crate::plugins::manifest::Manifest, String>,
    },
    /// A notice from a background task.
    Notice(String),
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

async fn event_loop(
    terminal: &mut Term,
    conns: &mut Connections,
    mut app: App,
    start: Start,
    local_client: Client,
    update_check_enabled: bool,
    key_notices: Vec<String>,
) -> io::Result<plain::Exit> {
    let remote_hosts = match start {
        Start::Plain(start) => {
            app.start_plain(start);
            Vec::new()
        }
        Start::Manager { saved, live } => {
            app.start_manager(&saved, &live);
            // Every remote host a saved tab lives on.
            let hosts: std::collections::BTreeSet<String> = saved
                .sessions
                .into_iter()
                .map(|s| s.host)
                .filter(|h| h != "local")
                .collect();
            hosts.into_iter().collect()
        }
    };
    let manager = app.mode == Mode::Manager;

    // Show any config parse notices in the status bar at startup.
    for notice in key_notices {
        app.notify(notice);
    }

    // Merge all incoming streams into one tagged channel.
    let (ev_tx, mut ev_rx) = mpsc::channel::<HostEvent>(1024);

    // Start the local incoming reader, tagged with the current local generation.
    if let Some(rx) = local_client.take_incoming() {
        let gen = conns.current_generation("local");
        spawn_reader("local".to_owned(), gen, rx, ev_tx.clone());
    }
    // `conns` owns the connection from here: dropping it there closes it.
    drop(local_client);

    // Start background connections to the saved tabs' remote hosts.
    for host in remote_hosts {
        // The entry must exist before the first attempt to track backoff.
        conns.ensure_host(&host);
        let gen = conns.next_generation(&host);
        spawn_connect(host, gen, ev_tx.clone(), false);
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
    if manager && app.claude_policy.update_check != crate::config::UpdatePolicy::Off {
        let tx = ev_tx.clone();
        tokio::spawn(async move {
            if let Some(latest) = crate::claude::update::check_due().await {
                let _ = tx.send(HostEvent::ClaudeLatest(latest)).await;
            }
        });
    }

    // External extensions' programs: started on first use, stopped on quit.
    let (notice_tx, mut notice_rx) = mpsc::unbounded_channel();
    let ext_host = extensions::host::ExtensionHost::new(&app.extensions, notice_tx);
    let notices = ev_tx.clone();
    tokio::spawn(async move {
        while let Some(text) = notice_rx.recv().await {
            if notices
                .send(HostEvent::ExtensionNotice(text))
                .await
                .is_err()
            {
                break;
            }
        }
    });

    let (reply_tx, mut reply_rx) = mpsc::unbounded_channel::<(ReplyTo, io::Result<Msg>)>();
    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // The plugin lists: now, then every 15 minutes.
    let mut plugins_tick = tokio::time::interval(Duration::from_secs(plugin_state::REFRESH_SECS));
    plugins_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // 60-second proxy stats refresh.
    let mut proxy_tick = tokio::time::interval(Duration::from_secs(60));
    proxy_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut outdated_tick =
        tokio::time::interval_at(tokio::time::Instant::now() + OUTDATED_RETRY, OUTDATED_RETRY);
    outdated_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_draw = Instant::now() - FRAME;
    // A plain session must not outlive its terminal.
    let mut shutdown = Box::pin(shutdown_signal());

    loop {
        for effect in app.take_effects() {
            run_effect(effect, &mut app, conns, &reply_tx, &ev_tx, &ext_host);
        }
        if app.quit {
            ext_host.shutdown().await;
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
                    // Drop stale events from replaced connections.
                    if gen < conns.current_generation(&host) {
                        continue;
                    }
                    match inc {
                        // Reconnect with backoff.
                        Incoming::Disconnected => {
                            conns.disconnect(&host);
                            app.on_incoming_from(&host, Incoming::Disconnected);
                            let (delay, gen) = conns.next_attempt(&host);
                            if host == "local" {
                                spawn_reconnect_local(delay, gen, ev_tx.clone());
                            } else {
                                spawn_connect_after(host, gen, delay, ev_tx.clone(), true);
                            }
                        }
                        Incoming::HostStats { cpu_pct, mem_used, mem_total, .. } => {
                            app.on_host_stats(host, cpu_pct, mem_used, mem_total);
                        }
                        inc => app.on_incoming_from(&host, inc),
                    }
                }
                Some(HostEvent::Connected { host, client, sessions, generation }) => {
                    on_connected(&mut app, conns, &ev_tx, host, client, sessions, generation);
                }
                Some(HostEvent::ConnectFailed { host, error, generation }) => {
                    // A newer attempt is already in flight.
                    if generation < conns.current_generation(&host) {
                        continue;
                    }
                    app.on_host_error(&host, &error);
                    conns.set_bootstrap_failed(&host, error.contains("bootstrap") || error.contains("install"));
                    // Retry with backoff, bootstrapping again only if that
                    // is what failed.
                    let (delay, gen) = conns.next_attempt(&host);
                    let skip_bootstrap = !conns.bootstrap_failed(&host);
                    spawn_connect_after(host, gen, delay, ev_tx.clone(), skip_bootstrap);
                }
                Some(HostEvent::LocalReconnected { client, sessions, generation }) => {
                    on_local_reconnected(&mut app, conns, &ev_tx, client, sessions, generation);
                }
                Some(HostEvent::LocalReconnectFailed { generation }) => {
                    if generation < conns.current_generation("local") {
                        continue;
                    }
                    let (delay, gen) = conns.next_attempt("local");
                    spawn_reconnect_local(delay, gen, ev_tx.clone());
                }
                Some(HostEvent::Reconnect { host, generation }) => {
                    if generation < conns.current_generation(&host) || !conns.is_connected(&host) {
                        continue;
                    }
                    // Dropping the client closes the connection; the newer
                    // generation silences its reader.
                    conns.disconnect(&host);
                    app.on_incoming_from(&host, Incoming::Disconnected);
                    let gen = conns.next_generation(&host);
                    if host == "local" {
                        spawn_reconnect_local(Duration::ZERO, gen, ev_tx.clone());
                    } else {
                        spawn_connect(host, gen, ev_tx.clone(), true);
                    }
                }
                Some(
                    ev @ (HostEvent::ProxyConfig { .. }
                    | HostEvent::ProxyStats { .. }
                    | HostEvent::SessionCredential { .. }
                    | HostEvent::SubscriptionSwitch { .. }),
                ) => app.on_proxy_event(ev),
                Some(HostEvent::UpgradeAvailable(tag)) => {
                    if let Some(t) = tag {
                        app.upgrade_notice = Some(t);
                        app.redraw = true;
                    }
                }
                Some(HostEvent::ClaudeLatest(latest)) => app.check_local_claude(&latest),
                Some(HostEvent::AnswersRendered { gen, id, answers }) => match answers {
                    Ok(answers) => app.on_answers_rendered(gen, id, answers),
                    Err(why) => app.on_answers_failed(gen, id, &why),
                },
                Some(HostEvent::ExtensionNotice(text)) => {
                    app.notify(text);
                    // The one notice the host sends: an extension disabled.
                    app.failed_extensions = ext_host.failed();
                }
                Some(HostEvent::ActionDone { gen, result }) => app.on_action_result(gen, result),
                Some(HostEvent::SnippetsLoaded { gen, dir, result }) => {
                    app.on_snippets(gen, &dir, result);
                }
                Some(HostEvent::SnippetSaved { result, cut }) => app.on_snippet_saved(result, cut),
                Some(HostEvent::Plugins { profile, fetch }) => app.on_plugins(&profile, fetch),
                Some(HostEvent::Manifest { name, sha256, result }) => {
                    app.on_manifest(&name, &sha256, result);
                }
                Some(HostEvent::Notice(text)) => app.notify(text),
                None => return Ok(Default::default()),
            },
            Some((to, reply)) = reply_rx.recv() => app.on_reply(to, reply),
            _ = tick.tick() => app.on_tick(),
            _ = proxy_tick.tick() => {
                // Refresh proxy stats for the active session's profile (they
                // feed the status bar, which `--plain` does not have).
                let proxy_name = app.active_view().and_then(|v| v.proxy.clone());
                if let Some(name) = proxy_name.filter(|_| manager) {
                    app.schedule_proxy_stats(&name, vec![crate::proxy::stats::Window::H24]);
                }
            }
            _ = plugins_tick.tick() => app.fetch_plugin_lists(),
            _ = outdated_tick.tick() => retry_outdated_hosts(conns, &ev_tx),
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
    ext_host: &extensions::host::ExtensionHost,
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
            // The wizard picked a host that is already connected: reuse the
            // connection, and let the wizard advance as if it had just connected.
            if conns.is_connected(&host) {
                if let Some(client) = conns.client(&host) {
                    let home = client.welcome().host.home.clone();
                    app.on_host_connected(&host, &home);
                    // Don't re-spawn a reader: the existing one is still running.
                }
                return;
            }
            // The entry must exist before the first attempt to track backoff.
            conns.ensure_host(&host);
            let gen = conns.next_generation(&host);
            spawn_connect(host, gen, ev_tx.clone(), false);
        }
        Effect::LoadSshHosts => app.on_ssh_hosts(crate::remote::hosts::candidates()),
        Effect::CopyToClipboard(text) => copy_to_clipboard(&text),
        Effect::LoadSnippets { gen } => {
            let tx = ev_tx.clone();
            tokio::spawn(async move {
                let (dir, read) = snippets::load_all().await;
                let shown = dir.display().to_string();
                let result = match read {
                    Ok(list) => Ok(list.into_iter().map(|s| (s.name, s.text)).collect()),
                    Err(e) => Err(e.to_string()),
                };
                let event = HostEvent::SnippetsLoaded {
                    gen,
                    dir: shown,
                    result,
                };
                let _ = tx.send(event).await;
            });
        }
        Effect::SaveSnippet { text, cut } => {
            let tx = ev_tx.clone();
            tokio::spawn(async move {
                let result = snippets::save_text(text).await.map_err(|e| e.to_string());
                let _ = tx.send(HostEvent::SnippetSaved { result, cut }).await;
            });
        }
        Effect::RunAction {
            gen,
            extension,
            action,
            input,
            session,
        } => {
            let done = ext_host.action(&extension, &action, input, session);
            let tx = ev_tx.clone();
            tokio::spawn(async move {
                let result = done.await;
                let _ = tx.send(HostEvent::ActionDone { gen, result }).await;
            });
        }
        Effect::SetExtension { name, on } => {
            // Stopping a program waits up to its grace: never here.
            tokio::spawn(ext_host.set_enabled(&name, on));
        }
        Effect::SavePluginChoices { name, choices } => {
            match crate::config::save_plugin_choices(&paths::config_file(), &name, &choices) {
                Ok(()) => app.notify(format!("plugin {name} set up (saved)")),
                Err(e) => app.notify(format!("could not save: {e}")),
            }
        }
        Effect::SaveExtension { name, on } => {
            let state = if on { "on" } else { "off" };
            match crate::config::save_extension(&paths::config_file(), &name, on) {
                Ok(()) => app.notify(format!("extension {name} {state} (saved)")),
                Err(e) => app.notify(format!("could not save: {e}")),
            }
        }
        Effect::RenderAnswers { gen, id, answers } => {
            // Markdown from the transcript is parsed off the UI thread.
            let tx = ev_tx.clone();
            tokio::spawn(async move {
                let parsed = tokio::task::spawn_blocking(move || reader::render(&answers)).await;
                let answers = parsed.map_err(|e| format!("could not lay the answers out: {e}"));
                let _ = tx
                    .send(HostEvent::AnswersRendered { gen, id, answers })
                    .await;
            });
        }
        Effect::Save => {
            if let Err(e) = app.to_state().save(&paths::client_state()) {
                app.notify(format!("could not save state: {e}"));
            }
        }
        Effect::FetchPlugins { profile } => {
            let tx = ev_tx.clone();
            tokio::spawn(async move {
                let fetch = fetch_plugins(&profile, &tx).await;
                let _ = tx.send(HostEvent::Plugins { profile, fetch }).await;
            });
        }
        Effect::FetchManifest { name, url, sha256 } => {
            let tx = ev_tx.clone();
            tokio::spawn(async move {
                let fetched = crate::proxy::plugins::fetch_manifest(&url, &sha256).await;
                let result = fetched
                    .and_then(|text| crate::plugins::manifest::Manifest::parse_named(&text, &name));
                let _ = tx
                    .send(HostEvent::Manifest {
                        name,
                        sha256,
                        result,
                    })
                    .await;
            });
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
        Effect::FetchSessionCredential {
            profile_name,
            session,
            claude_session_id,
        } => {
            let tx = ev_tx.clone();
            tokio::spawn(async move {
                let result = fetch_session_credential(&profile_name, &claude_session_id).await;
                let _ = tx
                    .send(HostEvent::SessionCredential {
                        session,
                        claude_session_id,
                        result,
                    })
                    .await;
            });
        }
        Effect::SwitchSubscription {
            profile_name,
            session,
            claude_session_id,
        } => {
            let tx = ev_tx.clone();
            tokio::spawn(async move {
                let result = switch_subscription(&profile_name, &claude_session_id).await;
                let _ = tx
                    .send(HostEvent::SubscriptionSwitch { session, result })
                    .await;
            });
        }
        Effect::SpawnWithProxy {
            host,
            msg,
            proxy_name,
            first_party,
            to,
            plugins: mut parts,
            pending,
        } => {
            // Resolve the client now (synchronous, cheap) so we can move it
            // into the async task. Client is Clone + Send + 'static.
            let client = conns.client(&host).cloned();
            let cached = app.proxy_config_cached(&proxy_name).cloned();
            let tx = reply_tx.clone();
            let notices = ev_tx.clone();
            tokio::spawn(async move {
                // Fetch proxy config with a 5 s timeout so the first spawn
                // uses the real model/rate-limit overrides, not fallback
                // defaults; meanwhile, wait for the plugin manifests.
                let config = async {
                    match cached {
                        Some(cfg) => Some(cfg),
                        None => tokio::time::timeout(
                            Duration::from_secs(5),
                            fetch_proxy_config(&proxy_name),
                        )
                        .await
                        .ok()
                        .flatten(),
                    }
                };
                let waits = wait_manifests(client.as_ref(), pending, &tx);
                let (cfg, (mut ready, left_out)) = tokio::join!(config, waits);
                if !left_out.is_empty() {
                    let names = left_out.join(", ");
                    let text = format!("plugin {names} left out: not ready on {host}");
                    let _ = notices.send(HostEvent::Notice(text)).await;
                }
                for part in &mut parts {
                    if part.applied.is_none() {
                        part.applied = ready.remove(&part.name);
                    }
                }
                let composed = plugin_state::compose(&parts);
                for text in composed.notices.clone() {
                    let _ = notices.send(HostEvent::Notice(text)).await;
                }

                // Build env from profile + (possibly fresh) config.
                let env = crate::proxy::resolve::proxy_env_for(
                    Some(&proxy_name),
                    cfg.as_ref(),
                    first_party,
                );
                match env {
                    Err(e) => {
                        let _ =
                            tx.send((to, Err(io::Error::other(format!("proxy env error: {e}")))));
                    }
                    Ok(env) => match client {
                        Some(c) => {
                            let msg = plugin_state::with_plugins(msg, composed).merge_env(env);
                            let reply = c.request(msg);
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
                    },
                }
            });
        }
    }
}

/// How long a spawn waits for a host to answer its plugin manifests.
pub(super) const MANIFEST_WAIT: Duration = Duration::from_secs(20);

/// Ask `client` for each of `pending` and wait up to [`MANIFEST_WAIT`]: the
/// answers that came back (`Applied`, by plugin name), and the names of
/// those that did not (late, failed, or no client). Every answer, even a
/// late one, also goes to `App` through `tx`.
async fn wait_manifests(
    client: Option<&Client>,
    pending: Vec<app::PendingManifest>,
    tx: &mpsc::UnboundedSender<(ReplyTo, io::Result<Msg>)>,
) -> (
    std::collections::HashMap<String, crate::proto::Applied>,
    Vec<String>,
) {
    let mut ready = std::collections::HashMap::new();
    let mut left_out = Vec::new();
    let deadline = tokio::time::Instant::now() + MANIFEST_WAIT;
    let mut waiting = Vec::new();
    for m in pending {
        let Some(client) = client else {
            left_out.push(m.name);
            continue;
        };
        let reply = client.request(m.msg);
        let (done, answer) = tokio::sync::oneshot::channel();
        let tx = tx.clone();
        tokio::spawn(async move {
            let reply = reply.await;
            let applied = match &reply {
                Ok(Msg::Applied(a)) => Some(a.clone()),
                _ => None,
            };
            let _ = done.send(applied);
            let _ = tx.send((m.to, reply));
        });
        waiting.push((m.name, answer));
    }
    for (name, answer) in waiting {
        match tokio::time::timeout_at(deadline, answer).await {
            Ok(Ok(Some(applied))) => {
                ready.insert(name, applied);
            }
            _ => left_out.push(name),
        }
    }
    (ready, left_out)
}

/// Fetch `profile`'s plugin list and keep `plugins.json` in step: a list
/// is saved, a 401/403 drops the profile's entry (its secrets), anything
/// else leaves it. A failed write is a notice.
async fn fetch_plugins(profile: &str, tx: &mpsc::Sender<HostEvent>) -> plugin_state::PluginsFetch {
    use crate::proxy::api::ApiError;
    use crate::proxy::plugins as cache;
    use plugin_state::PluginsFetch;
    let Some(p) = crate::proxy::resolve::find_profile(profile) else {
        return PluginsFetch::Unreachable;
    };
    let (fetch, write) = match cache::fetch(profile, &p).await {
        Ok(list) => {
            let saved = list.clone();
            let write = tokio::task::spawn_blocking(move || cache::save_cache(&saved));
            (PluginsFetch::Listed(list), Some(write))
        }
        Err(ApiError::Http(s)) if s.as_u16() == 401 || s.as_u16() == 403 => {
            let name = profile.to_owned();
            let write = tokio::task::spawn_blocking(move || cache::remove_cache(&name));
            (PluginsFetch::SignedOut, Some(write))
        }
        Err(_) => (PluginsFetch::Unreachable, None),
    };
    if let Some(write) = write {
        if let Ok(Err(e)) = write.await {
            let _ = tx
                .send(HostEvent::Notice(format!("could not save plugins: {e}")))
                .await;
        }
    }
    fetch
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
    windows: &[crate::proxy::stats::Window],
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

/// Ask the proxy which credential a conversation uses. `Ok(None)` is "not
/// known (yet)": no credential to show, not an error.
async fn fetch_session_credential(
    profile_name: &str,
    claude_session_id: &str,
) -> Result<Option<crate::proxy::api::SessionCredential>, String> {
    let (url, token) = crate::proxy::resolve::resolve_profile(profile_name)
        .ok_or_else(|| format!("proxy profile '{profile_name}' not found"))?;
    crate::proxy::api::fetch_session(&url, &token, claude_session_id)
        .await
        .map_err(describe_api_error)
}

/// Ask the proxy to move a conversation to its next subscription.
async fn switch_subscription(
    profile_name: &str,
    claude_session_id: &str,
) -> Result<crate::proxy::api::SwitchOutcome, crate::proxy::api::SwitchError> {
    use crate::proxy::api::{ApiError, SwitchError};
    let Some((url, token)) = crate::proxy::resolve::resolve_profile(profile_name) else {
        return Err(SwitchError::Api(ApiError::Http(
            reqwest::StatusCode::UNAUTHORIZED,
        )));
    };
    crate::proxy::api::switch_session(&url, &token, claude_session_id).await
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
