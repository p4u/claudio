//! The session manager UI that a bare `claudio` opens.
//!
//! This module is the I/O edge: it owns the terminal and the daemon
//! connections (local + remote) and runs the event loop. State and update
//! logic live in [`app`], rendering in [`ui`].

pub mod app;
mod builtins;
mod claude_update;
mod confirm;
mod connections;
mod diagram;
mod extensions;
mod extensions_view;
mod fmt;
mod formulas;
mod git_app;
mod git_view;
mod interaction;
mod keymap;
mod latex;
mod leader;
mod notifications;
mod plain;
mod plugins;
mod proxy_state;
mod reader;
mod selection;
mod sessions;
mod snippets;
mod state;
mod stats_view;
mod typeset;

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

use std::cell::Cell;
use std::collections::HashMap;
use std::io::{self, Stdout};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
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
use crate::freshness::{self, DaemonCheck};
use crate::paths;
use crate::proto::{Msg, SessionInfo};
use crate::remote::bootstrap::ensure_remote;

use app::{App, AppConfig, Effect, Mode, ReplyTo};
use connections::Connections;
use plain::PlainStart;
use state::ClientState;

/// Animation / clock tick.
const TICK: Duration = Duration::from_millis(250);
/// Minimum time between redraws (~60 fps), coalescing output bursts.
const FRAME: Duration = Duration::from_millis(16);
/// How often a daemon left outdated (its sessions were busy) is retried.
const OUTDATED_RETRY: Duration = Duration::from_secs(120);

/// Whether keyboard enhancement flags were pushed (and must be popped).
static KEYBOARD_ENHANCED: AtomicBool = AtomicBool::new(false);

/// How long the render worker waits for one formula before it gives up on
/// it (it is shown raw).
const RENDER_BUDGET: Duration = Duration::from_millis(150);
/// How many renders that ran over budget may still be running before new
/// formulas are left raw (and tried again on a later list).
const MAX_ABANDONED: usize = 2;
/// How many rendered formulas the worker remembers, by source.
const CACHE_SIZE: usize = 1024;
/// The stack of a render scratch thread: deep enough for anything that
/// passes `latex::too_complex`, in a debug or a release build.
const SCRATCH_STACK: usize = 32 << 20;
/// The thread names of the render worker and of its scratch threads.
const RENDER_THREAD: &str = "latex-render";
const SCRATCH_THREAD: &str = "latex-scratch";

thread_local! {
    /// Set on a render scratch thread. A panic there is the renderer's,
    /// caught and shown as a raw formula: the UI's panic hook must not
    /// touch the terminal for it (see [`panic_hook`]).
    static QUIET_PANIC: Cell<bool> = const { Cell::new(false) };
}

type Term = Terminal<CrosstermBackend<Stdout>>;

/// Emit an OSC 9 desktop notification + single BEL to the outer terminal for a
/// session label. Written directly to stdout, between ratatui frames.
///
/// The label comes from claude (its title) and is sanitized first, so it
/// cannot smuggle escape sequences into the outer terminal.
fn emit_notification(label: &str) {
    use std::io::Write;
    let safe = sessions::sanitize_label(label, 200);
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
enum Launch {
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
    run_launch(Launch::Manager {
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
    run_launch(Launch::Plain {
        proxy,
        first_party,
        args,
    })
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
    let config = crate::config::load();
    let mut key_notices = Vec::new();
    let (proxy_override, first_party, plain) = match launch {
        Launch::Manager { proxy, first_party } => (proxy, first_party, None),
        Launch::Plain {
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
    // External extensions are read once, at start.
    let external = extensions::manifest::load(&extensions::manifest::dir(), &mut key_notices);
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
    let registry = extensions::Registry::new(
        &builtin_choices,
        config.claude.latex,
        external,
        &mut key_notices,
    );
    let dropped = registry.dropped_lenses();
    if !dropped.is_empty() {
        key_notices.push(format!(
            "extensions: lenses {} left out of the session context (caps)",
            dropped.join(", ")
        ));
    }
    let local_features = local_client.welcome().features.clone();
    let mut app = App::new(AppConfig {
        mode: if plain.is_some() { Mode::Plain } else { Mode::Manager },
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
    app.load_plugins(cached_plugins, config.extensions.clone());
    app.set_host_features("local", &local_features);
    app.on_plugin_host("local");
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

/// Start the local daemon if needed (replacing an outdated one), connect and
/// list its sessions.
async fn connect_local() -> io::Result<(Client, Vec<SessionInfo>)> {
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
async fn retry_outdated(
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
/// panic as `default` would. A panic on a render scratch thread does
/// neither: it is caught there, and the UI goes on.
fn panic_hook(restore: fn(), default: PanicHook) -> PanicHook {
    Box::new(move |info| {
        if QUIET_PANIC.with(Cell::get) {
            return;
        }
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
    /// The render worker finished a session's formula list.
    LatexRendered {
        host: String,
        id: crate::proto::SessionId,
        seq: u64,
        formulas: Vec<latex::Formula>,
    },
    /// The render worker finished the answers of a reader view; the
    /// external lenses' items and blocks in them are still to render.
    AnswersRendered {
        gen: u64,
        id: crate::proto::SessionId,
        answers: Vec<reader::RenderedAnswer>,
        external: Vec<reader::External>,
    },
    /// The extension host rendered a session's external lens items (list
    /// `seq`), in order; `None` stays raw.
    ExternalRendered {
        host: String,
        id: crate::proto::SessionId,
        seq: u64,
        items: Vec<(crate::proto::LensItem, Option<extensions::protocol::Rendering>)>,
    },
    /// The extension host rendered the external items of reader view `gen`.
    ReaderExternalRendered {
        gen: u64,
        id: crate::proto::SessionId,
        rendered: Vec<(reader::External, Option<extensions::protocol::Rendering>)>,
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
        fetch: plugins::PluginsFetch,
    },
    /// A notice from a background task.
    Notice(String),
}

// ── Formula rendering ─────────────────────────────────────────────────────────

/// One [`Effect::RenderLatex`]: session `id`'s formulas as of `seq`.
struct LatexJob {
    host: String,
    id: crate::proto::SessionId,
    seq: u64,
    srcs: Vec<String>,
}

/// One [`Effect::RenderAnswers`]: the answers of the reader view `gen`
/// shows for session `id`.
struct AnswersJob {
    gen: u64,
    id: crate::proto::SessionId,
    answers: Vec<crate::proto::Answer>,
    /// The enabled tag and fence lenses (the registry's, when sent), and
    /// which of them are external.
    tags: Vec<String>,
    fences: Vec<String>,
    external: Vec<String>,
}

/// Work for the `latex-render` thread, one job at a time.
enum RenderJob {
    Latex(LatexJob),
    Answers(AnswersJob),
}

/// What the `latex-render` thread made of a job.
enum Rendered {
    Latex(LatexJob, Vec<latex::Formula>),
    Answers(AnswersJob, Vec<reader::RenderedAnswer>, Vec<reader::External>),
}

/// Start the `latex-render` thread: it renders each job (its formulas from
/// its cache when it can) and hands the result to `done`. It ends when the
/// returned sender is dropped. Rendering never runs on the UI thread: one
/// hostile formula can take a minute to parse, and markdown is parsed here
/// too.
fn spawn_latex_worker(
    done: impl Fn(Rendered) + Send + 'static,
) -> io::Result<std::sync::mpsc::Sender<RenderJob>> {
    let (jobs, queue) = std::sync::mpsc::channel::<RenderJob>();
    std::thread::Builder::new()
        .name(RENDER_THREAD.to_owned())
        .spawn(move || {
            let mut cache = RenderCache::default();
            let running = Arc::new(AtomicUsize::new(0));
            for job in queue {
                match job {
                    RenderJob::Latex(job) => {
                        let formulas = job
                            .srcs
                            .iter()
                            .map(|src| cache.formula(src, &running))
                            .collect();
                        done(Rendered::Latex(job, formulas));
                    }
                    RenderJob::Answers(job) => {
                        let lenses = (&job.tags[..], &job.fences[..], &job.external[..]);
                        let (answers, work) =
                            render_lens_answers(&job.answers, lenses, &mut cache, &running);
                        done(Rendered::Answers(job, answers, work));
                    }
                }
            }
        })?;
    Ok(jobs)
}

/// `answers` as the reader shows them with only the latex lens (see
/// [`render_lens_answers`]).
#[cfg(test)]
fn render_answers(
    answers: &[crate::proto::Answer],
    cache: &mut RenderCache,
    running: &Arc<AtomicUsize>,
) -> Vec<reader::RenderedAnswer> {
    let tags = ["latex".to_owned()];
    render_lens_answers(answers, (&tags, &[], &[]), cache, running).0
}

/// `answers` as the reader shows them: each a markdown document whose
/// items of tag lenses `lenses.0` (the first `MAX_FORMULAS`) and blocks of
/// fence lenses `lenses.1` are rendered through the guard and its cache,
/// like the pane's. The external lenses (`lenses.2`) are left raw here and
/// listed: the extension host renders them.
fn render_lens_answers(
    answers: &[crate::proto::Answer],
    lenses: (&[String], &[String], &[String]),
    cache: &mut RenderCache,
    running: &Arc<AtomicUsize>,
) -> (Vec<reader::RenderedAnswer>, Vec<reader::External>) {
    use crate::proto::LensKind;
    let tags: Vec<&str> = lenses.0.iter().map(String::as_str).collect();
    let fences: Vec<&str> = lenses.1.iter().map(String::as_str).collect();
    let external = |lens: &str| lenses.2.iter().any(|e| e == lens);
    let mut work = Vec::new();
    let rendered = answers
        .iter()
        .enumerate()
        .map(|(a, answer)| {
            let prepared = reader::doc::prepare_lenses(&answer.text, &tags, &fences);
            let mut formulas = Vec::with_capacity(prepared.tags.len());
            for (i, (tag, lens)) in prepared.tags.iter().zip(&prepared.lenses).enumerate() {
                if external(lens) {
                    work.push(reader::External {
                        answer: a,
                        slot: reader::Slot::Formula(i),
                        lens: lens.clone(),
                        kind: LensKind::Tag,
                        source: tag.src.clone(),
                    });
                    formulas.push(latex::Formula::raw(&tag.src));
                } else {
                    formulas.push(cache.render(lens, &tag.src, running));
                }
            }
            let mut doc = reader::doc::parse(&prepared);
            let mut n = 0;
            doc.each_lens(&mut |lens, src, rows| {
                if external(lens) {
                    work.push(reader::External {
                        answer: a,
                        slot: reader::Slot::Fence(n),
                        lens: lens.to_owned(),
                        kind: LensKind::Fence,
                        source: src,
                    });
                } else {
                    *rows = cache.render(lens, &src, running).block.map(|b| b.rows);
                }
                n += 1;
            });
            reader::RenderedAnswer {
                prompt: answer.prompt.clone(),
                doc,
                formulas,
            }
        })
        .collect();
    (rendered, work)
}

/// Renderings by lens and source, the [`CACHE_SIZE`] most recently used:
/// each list resends every item, and a source is rendered once.
#[derive(Default)]
struct RenderCache {
    entries: HashMap<(String, String), (u64, latex::Formula)>,
    /// Bumped on every lookup; an entry keeps the value of its last use.
    clock: u64,
}

impl RenderCache {
    /// A `<latex>` formula's rendering.
    fn formula(&mut self, src: &str, running: &Arc<AtomicUsize>) -> latex::Formula {
        self.render("latex", src, running)
    }

    fn render(&mut self, lens: &str, src: &str, running: &Arc<AtomicUsize>) -> latex::Formula {
        self.clock += 1;
        let key = (lens.to_owned(), src.to_owned());
        if let Some((used, formula)) = self.entries.get_mut(&key) {
            *used = self.clock;
            return formula.clone();
        }
        let (formula, keep) = render_lens(lens, src, running);
        if keep {
            if self.entries.len() >= CACHE_SIZE {
                let oldest = self
                    .entries
                    .iter()
                    .min_by_key(|(_, (used, _))| *used)
                    .map(|(key, _)| key.clone());
                if let Some(oldest) = oldest {
                    self.entries.remove(&oldest);
                }
            }
            self.entries.insert(key, (self.clock, formula.clone()));
        }
        formula
    }
}

/// Render `src` of lens `lens` with its built-in renderer: a tag lens's
/// item, or a fence lens's block (its rows as `block`). A lens without one
/// (an external lens's) stays raw. The flag says whether it may be cached.
fn render_lens(lens: &str, src: &str, running: &Arc<AtomicUsize>) -> (latex::Formula, bool) {
    match lens {
        "latex" => render_formula(src, running),
        "mermaid" => render_diagram(src, running),
        _ => (latex::Formula::raw(src), true),
    }
}

/// Draw one Mermaid diagram within [`RENDER_BUDGET`], as `render_formula`
/// renders a formula: what the pre-check turns away, is unsupported, times
/// out or panics stays its code block (raw); its rows are the block.
fn render_diagram(src: &str, running: &Arc<AtomicUsize>) -> (latex::Formula, bool) {
    if diagram::too_complex(src) {
        return (latex::Formula::raw(src), true);
    }
    let owned = src.to_owned();
    match guarded(move || diagram::render(&owned), running) {
        Guarded::Done(Ok(rows)) => {
            let block = Some(typeset::Laid { rows, baseline: 0 });
            let rendered = latex::Rendered {
                inline: None,
                block,
                text: None,
            };
            (latex::Formula::new(src, rendered), true)
        }
        Guarded::Done(Err(_)) | Guarded::Failed => (latex::Formula::raw(src), true),
        Guarded::Busy => (latex::Formula::raw(src), false),
    }
}

/// Render one formula within [`RENDER_BUDGET`]: what the pre-check turns
/// away, or what times out or panics, is shown raw. The flag says whether
/// the result may be cached: not when too many renders were still running
/// to start this one (a later list tries it again).
fn render_formula(src: &str, running: &Arc<AtomicUsize>) -> (latex::Formula, bool) {
    let clean = latex::prepare(src);
    if latex::too_complex(&clean) {
        return (latex::Formula::raw(src), true);
    }
    match guarded(move || latex::render(&clean), running) {
        Guarded::Done(rendered) => (latex::Formula::new(src, rendered), true),
        Guarded::Failed => (latex::Formula::raw(src), true),
        Guarded::Busy => (latex::Formula::raw(src), false),
    }
}

/// How a [`guarded`] call ended.
enum Guarded<T> {
    Done(T),
    /// It panicked or ran over budget.
    Failed,
    /// It was not started: [`MAX_ABANDONED`] renders still run (or no
    /// thread could be made).
    Busy,
}

/// Run `f` on a fresh scratch thread with a [`SCRATCH_STACK`] stack and a
/// panic in it caught quietly ([`QUIET_PANIC`]), waiting at most
/// [`RENDER_BUDGET`]. A thread still running then is abandoned and ends on
/// its own. `running` counts the scratch threads alive; when this returns,
/// those are all abandoned ones.
fn guarded<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
    running: &Arc<AtomicUsize>,
) -> Guarded<T> {
    if running.load(Ordering::Acquire) >= MAX_ABANDONED {
        return Guarded::Busy;
    }
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let alive = Arc::clone(running);
    alive.fetch_add(1, Ordering::AcqRel);
    let spawned = std::thread::Builder::new()
        .name(SCRATCH_THREAD.to_owned())
        .stack_size(SCRATCH_STACK)
        .spawn(move || {
            QUIET_PANIC.with(|quiet| quiet.set(true));
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
            // Counted out before the result goes, so a caller that has it
            // never counts this thread as running.
            alive.fetch_sub(1, Ordering::AcqRel);
            let _ = tx.send(result);
        });
    if spawned.is_err() {
        running.fetch_sub(1, Ordering::AcqRel);
        return Guarded::Busy;
    }
    match rx.recv_timeout(RENDER_BUDGET) {
        Ok(Ok(value)) => Guarded::Done(value),
        Ok(Err(_)) | Err(_) => Guarded::Failed,
    }
}

/// Render each of `requests` (lens, kind, source) with the extension host,
/// concurrently; the renderings in order (`None`: stays raw).
fn render_all(
    host: &extensions::host::ExtensionHost,
    requests: Vec<(String, crate::proto::LensKind, String)>,
) -> impl std::future::Future<Output = Vec<Option<extensions::protocol::Rendering>>> + Send + 'static
{
    let futures: Vec<_> = requests
        .iter()
        .map(|(lens, kind, source)| host.render(lens, *kind, source))
        .collect();
    async move {
        let mut set = tokio::task::JoinSet::new();
        let count = futures.len();
        for (i, f) in futures.into_iter().enumerate() {
            set.spawn(async move { (i, f.await) });
        }
        let mut out = vec![None; count];
        while let Some(Ok((i, rendering))) = set.join_next().await {
            out[i] = rendering;
        }
        out
    }
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

    // Formulas and answers are rendered on their own thread; the results
    // come back here.
    let rendered = ev_tx.clone();
    let latex_jobs = spawn_latex_worker(move |done| {
        let event = match done {
            Rendered::Latex(job, formulas) => HostEvent::LatexRendered {
                host: job.host,
                id: job.id,
                seq: job.seq,
                formulas,
            },
            Rendered::Answers(job, answers, external) => HostEvent::AnswersRendered {
                gen: job.gen,
                id: job.id,
                answers,
                external,
            },
        };
        let _ = rendered.blocking_send(event);
    })
    .ok();

    // External extensions' programs: started on first use, stopped on quit.
    let (notice_tx, mut notice_rx) = mpsc::unbounded_channel();
    let ext_host = extensions::host::ExtensionHost::new(&app.extensions, notice_tx);
    let notices = ev_tx.clone();
    tokio::spawn(async move {
        while let Some(text) = notice_rx.recv().await {
            if notices.send(HostEvent::ExtensionNotice(text)).await.is_err() {
                break;
            }
        }
    });

    let (reply_tx, mut reply_rx) = mpsc::unbounded_channel::<(ReplyTo, io::Result<Msg>)>();
    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // The plugin lists: now, then every 15 minutes.
    let mut plugins_tick = tokio::time::interval(Duration::from_secs(plugins::REFRESH_SECS));
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
            let workers = (latex_jobs.as_ref(), &ext_host);
            run_effect(effect, &mut app, conns, &reply_tx, &ev_tx, workers);
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
                    // A newer attempt is already in flight.
                    if generation < conns.current_generation(&host) {
                        continue;
                    }
                    let home = client.welcome().host.home.clone();
                    let remote_claude = client_claude(&client);
                    app.set_host_features(&host, &client.welcome().features);
                    conns.reset_delay(&host);
                    // Start the reader tagged with the current generation.
                    let cur_gen = conns.current_generation(&host);
                    if let Some(rx) = client.take_incoming() {
                        spawn_reader(host.clone(), cur_gen, rx, ev_tx.clone());
                    }
                    conns.connected(&host, client);
                    // Record this host in the MRU so it appears first next time.
                    crate::remote::hosts::touch(&host);
                    app.on_ssh_hosts(crate::remote::hosts::candidates());
                    app.on_host_connected(&host, &home);
                    app.on_plugin_host(&host);
                    // Recover this host's sessions only.
                    app.recover_host(&host, &sessions);
                    app.subscribe_host_stats(&host);
                    app.check_remote_claude(&host, remote_claude.as_deref());
                    app.redraw = true;
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
                    // A newer attempt is already in flight.
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
                    app.set_host_features("local", &client.welcome().features);
                    conns.connected("local", client);
                    app.on_plugin_host("local");
                    app.on_reconnected_local(&sessions, home);
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
                Some(HostEvent::ProxyConfig { profile_name, config }) => {
                    if let Some(cfg) = config {
                        app.on_proxy_config(profile_name, cfg);
                    }
                }
                Some(HostEvent::ProxyStats { profile_name, fetch }) => {
                    app.on_proxy_stats(profile_name, fetch);
                }
                Some(HostEvent::SessionCredential { session, claude_session_id, result }) => {
                    app.on_session_credential(session, &claude_session_id, result);
                }
                Some(HostEvent::SubscriptionSwitch { session, result }) => {
                    app.on_switch_result(session, result);
                }
                Some(HostEvent::UpgradeAvailable(tag)) => {
                    if let Some(t) = tag {
                        app.upgrade_notice = Some(t);
                        app.redraw = true;
                    }
                }
                Some(HostEvent::ClaudeLatest(latest)) => app.check_local_claude(&latest),
                Some(HostEvent::LatexRendered { host, id, seq, formulas }) => {
                    app.on_latex_rendered(&host, id, seq, formulas);
                }
                Some(HostEvent::AnswersRendered { gen, id, answers, external }) => {
                    app.on_answers_rendered(gen, id, answers);
                    if !external.is_empty() {
                        let requests = external
                            .iter()
                            .map(|w| (w.lens.clone(), w.kind, w.source.clone()))
                            .collect();
                        let all = render_all(&ext_host, requests);
                        let tx = ev_tx.clone();
                        tokio::spawn(async move {
                            let rendered = external.into_iter().zip(all.await).collect();
                            let event = HostEvent::ReaderExternalRendered { gen, id, rendered };
                            let _ = tx.send(event).await;
                        });
                    }
                }
                Some(HostEvent::ExternalRendered { host, id, seq, items }) => {
                    app.on_external_rendered(&host, id, seq, items);
                }
                Some(HostEvent::ReaderExternalRendered { gen, id, rendered }) => {
                    app.on_reader_external(gen, id, rendered);
                }
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
                    app.schedule_proxy_stats(&name, vec![stats_view::Window::H24]);
                }
            }
            _ = plugins_tick.tick() => app.fetch_plugin_lists(),
            _ = outdated_tick.tick() => {
                for (host, conn) in &conns.map {
                    if let Some(client) = conn.client.as_ref().filter(|c| c.daemon_outdated()) {
                        let retry = retry_outdated(
                            host.clone(),
                            conn.generation,
                            client.clone(),
                            ev_tx.clone(),
                        );
                        tokio::spawn(retry);
                    }
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
    (latex_jobs, ext_host): (
        Option<&std::sync::mpsc::Sender<RenderJob>>,
        &extensions::host::ExtensionHost,
    ),
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
        Effect::RenderLatex { host, id, seq, srcs } => {
            // Without a worker (it could not start) formulas stay raw.
            if let Some(jobs) = latex_jobs {
                let _ = jobs.send(RenderJob::Latex(LatexJob { host, id, seq, srcs }));
            }
        }
        Effect::RenderExternal {
            host,
            id,
            seq,
            items,
        } => {
            let requests = items
                .iter()
                .map(|i| (i.tag.clone(), crate::proto::LensKind::Tag, i.text.clone()))
                .collect();
            let all = render_all(ext_host, requests);
            let tx = ev_tx.clone();
            tokio::spawn(async move {
                let items = items.into_iter().zip(all.await).collect();
                let _ = tx
                    .send(HostEvent::ExternalRendered {
                        host,
                        id,
                        seq,
                        items,
                    })
                    .await;
            });
        }
        Effect::LoadSnippets { gen } => {
            let tx = ev_tx.clone();
            tokio::spawn(async move {
                let dir = snippets::dir();
                let shown = dir.display().to_string();
                let read = tokio::task::spawn_blocking(move || snippets::load(&dir)).await;
                let result = match read {
                    Ok(Ok(list)) => Ok(list.into_iter().map(|s| (s.name, s.text)).collect()),
                    Ok(Err(e)) => Err(e.to_string()),
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
                let save = move || snippets::save(&snippets::dir(), &text);
                let result = match tokio::task::spawn_blocking(save).await {
                    Ok(Ok(name)) => Ok(name),
                    Ok(Err(e)) => Err(e.to_string()),
                    Err(e) => Err(e.to_string()),
                };
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
        Effect::SaveExtension { name, on } => {
            let state = if on { "on" } else { "off" };
            match crate::config::save_extension(&paths::config_file(), &name, on) {
                Ok(()) => app.notify(format!("extension {name} {state} (saved)")),
                Err(e) => app.notify(format!("could not save: {e}")),
            }
        }
        Effect::RenderAnswers { gen, id, answers } => {
            let names = |list: Vec<&str>| list.iter().map(|t| t.to_string()).collect::<Vec<_>>();
            let tags = names(app.extensions.tag_lenses());
            let fences = names(app.extensions.fence_lenses());
            let external = tags
                .iter()
                .chain(&fences)
                .filter(|l| app.extensions.external_lens(l).is_some())
                .cloned()
                .collect();
            let job = RenderJob::Answers(AnswersJob {
                gen,
                id,
                answers,
                tags,
                fences,
                external,
            });
            if latex_jobs.is_none_or(|jobs| jobs.send(job).is_err()) {
                app.on_answers_failed(gen, id, "the render thread is not running");
            }
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
                    .send(HostEvent::SubscriptionSwitch {
                        session,
                        result,
                    })
                    .await;
            });
        }
        Effect::SpawnWithProxy {
            host,
            msg,
            proxy_name,
            first_party,
            to,
            bundles,
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
                // defaults; meanwhile, wait for the plugin bundles.
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
                let waits = wait_bundles(client.as_ref(), bundles, &tx);
                let (cfg, (dirs, left_out)) = tokio::join!(config, waits);
                if !left_out.is_empty() {
                    let names = left_out.join(", ");
                    let text = format!("plugin {names} left out: not ready on {host}");
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
                        let _ = tx.send((
                            to,
                            Err(io::Error::other(format!("proxy env error: {e}"))),
                        ));
                    }
                    Ok(env) => {
                        match client {
                            Some(c) => {
                                let msg = msg.with_plugin_dirs(&dirs).merge_env(env);
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
                        }
                    }
                }
            });
        }
    }
}

/// How long a spawn waits for its plugin bundles.
const BUNDLE_WAIT: Duration = Duration::from_secs(20);

/// Ask `client` for each of `bundles` and wait up to [`BUNDLE_WAIT`]: the
/// paths of those that came back ready, and the names of those that did
/// not (pending, failed, or no client). Every answer, even a late one, also
/// goes to `App` through `tx`.
async fn wait_bundles(
    client: Option<&Client>,
    bundles: Vec<app::PendingBundle>,
    tx: &mpsc::UnboundedSender<(ReplyTo, io::Result<Msg>)>,
) -> (Vec<String>, Vec<String>) {
    let mut dirs = Vec::new();
    let mut left_out = Vec::new();
    let deadline = tokio::time::Instant::now() + BUNDLE_WAIT;
    let mut waiting = Vec::new();
    for b in bundles {
        let Some(client) = client else {
            left_out.push(b.name);
            continue;
        };
        let reply = client.request(b.msg);
        let (done, answer) = tokio::sync::oneshot::channel();
        let tx = tx.clone();
        tokio::spawn(async move {
            let reply = reply.await;
            // `Some(None)`: installed in Claude Code, no bundle needed.
            let ready = match &reply {
                Ok(Msg::Bundled {
                    installed: true, ..
                }) => Some(None),
                Ok(Msg::Bundled { path: Some(p), .. }) => Some(Some(p.clone())),
                _ => None,
            };
            let _ = done.send(ready);
            let _ = tx.send((b.to, reply));
        });
        waiting.push((b.name, answer));
    }
    for (name, answer) in waiting {
        match tokio::time::timeout_at(deadline, answer).await {
            Ok(Ok(Some(Some(path)))) => dirs.push(path),
            Ok(Ok(Some(None))) => {}
            _ => left_out.push(name),
        }
    }
    (dirs, left_out)
}

/// Fetch `profile`'s plugin list and keep `plugins.json` in step: a list
/// is saved, a 401/403 drops the profile's entry (its secrets), anything
/// else leaves it. A failed write is a notice.
async fn fetch_plugins(profile: &str, tx: &mpsc::Sender<HostEvent>) -> plugins::PluginsFetch {
    use crate::proxy::api::ApiError;
    use crate::proxy::plugins as cache;
    use plugins::PluginsFetch;
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

/// Spawn a task that reconnects to the local daemon (starting it if needed)
/// after `delay`; the outcome comes back tagged with `generation`.
fn spawn_reconnect_local(delay: Duration, generation: u64, tx: mpsc::Sender<HostEvent>) {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A panic in a render guard leaves the terminal alone: the UI's panic
    /// hook neither restores it nor prints. One on a thread without the
    /// guard's flag still does.
    #[test]
    fn a_render_panic_does_not_restore_the_terminal() {
        static RESTORES: AtomicUsize = AtomicUsize::new(0);
        // Other tests panic too: count only the threads this one names.
        fn restore() {
            if std::thread::current().name() == Some(SCRATCH_THREAD) {
                RESTORES.fetch_add(1, Ordering::SeqCst);
            }
        }
        let previous: Arc<dyn Fn(&std::panic::PanicHookInfo<'_>) + Sync + Send> =
            std::panic::take_hook().into();
        let report = Arc::clone(&previous);
        std::panic::set_hook(panic_hook(restore, Box::new(move |info| report(info))));
        let running = Arc::new(AtomicUsize::new(0));
        let caught = guarded(|| -> u8 { panic!("a renderer bug") }, &running);
        let loud = std::thread::Builder::new()
            .name(SCRATCH_THREAD.to_owned())
            .spawn(|| panic!("not a render"))
            .unwrap()
            .join();
        std::panic::set_hook(Box::new(move |info| previous(info)));
        assert!(matches!(caught, Guarded::Failed));
        assert!(loud.is_err());
        assert_eq!(RESTORES.load(Ordering::SeqCst), 1, "only the unguarded one");
        assert_eq!(running.load(Ordering::SeqCst), 0);
    }

    /// Hostile formulas go through the worker's own entry point: each
    /// comes back raw or rendered and sanitized, nothing panics or aborts the
    /// test binary, and none holds the worker longer than twice the budget.
    #[test]
    fn hostile_formulas_are_raw_or_sanitized_and_bounded_in_time() {
        let corpus = [
            "\\frac{a}{b".to_owned(),
            "}}}{{{x".to_owned(),
            "{".repeat(5000) + "x" + &"}".repeat(5000),
            "\\frac ".repeat(340),
            "\\hat ".repeat(400),
            "\\not".repeat(512),
            "x/".repeat(1000),
            // Braceless script chains: no `{`, so not counted by the pre-check.
            "x^".repeat(1000),
            "x_".repeat(1000),
            "sqrt(".repeat(400),
            "x\u{1b}[2J\u{1b}]52;c;AAAA\u{7}y\u{9b}31m".to_owned(),
            "a\u{202e}b\u{2066}c\u{200b}d\u{feff}e".to_owned(),
            "\\binom{}{}".to_owned(),
            "x".repeat(10 * 1024),
            // Past the pre-check, but seconds of parsing: it times out.
            "x/".repeat(255),
        ];
        let running = Arc::new(AtomicUsize::new(0));
        for src in &corpus {
            let started = Instant::now();
            let (formula, _) = render_formula(src, &running);
            let took = started.elapsed();
            let head: String = src.chars().take(16).collect();
            assert!(took < 2 * RENDER_BUDGET, "{took:?} for {head:?}…");
            let laid = formula.block.iter().chain(&formula.text);
            for text in formula.inline.iter().chain(laid.flat_map(|l| &l.rows)) {
                assert_eq!(*text, crate::claude::latex::sanitize(text), "{head:?}");
                assert!(!text.contains(['\u{1b}', '\u{9b}', '\u{202e}', '\u{200b}']));
            }
        }
    }

    /// A render over budget is abandoned. While MAX_ABANDONED of them still
    /// run, nothing new starts (raw, and not cached); once they end,
    /// rendering resumes. Sleeps stand in for slow parses, so the test
    /// burns no CPU.
    #[test]
    fn slow_renders_are_abandoned_up_to_a_cap() {
        let running = Arc::new(AtomicUsize::new(0));
        for _ in 0..MAX_ABANDONED {
            let started = Instant::now();
            let nap = || std::thread::sleep(4 * RENDER_BUDGET);
            assert!(matches!(guarded(nap, &running), Guarded::Failed));
            assert!(started.elapsed() < 2 * RENDER_BUDGET);
        }
        assert_eq!(running.load(Ordering::SeqCst), MAX_ABANDONED);
        let started = Instant::now();
        let (formula, cache) = render_formula("E = mc^2", &running);
        assert!(started.elapsed() < RENDER_BUDGET, "not even started");
        assert_eq!((formula.inline, cache), (None, false), "busy: raw, not cached");
        let deadline = Instant::now() + Duration::from_secs(10);
        while running.load(Ordering::SeqCst) > 0 {
            assert!(Instant::now() < deadline, "the abandoned renders never ended");
            std::thread::sleep(Duration::from_millis(20));
        }
        let (formula, cache) = render_formula("E = mc^2", &running);
        assert_eq!((formula.inline.as_deref(), cache), (Some("E = mc²"), true));
    }

    #[test]
    fn the_worker_renders_each_job_and_passes_its_seq_on() {
        let (tx, rx) = std::sync::mpsc::channel();
        let jobs = spawn_latex_worker(move |done| {
            if let Rendered::Latex(job, formulas) = done {
                let _ = tx.send((job.seq, formulas));
            }
        })
        .unwrap();
        let id = uuid::Uuid::new_v4();
        let job = |seq, srcs: Vec<String>| {
            RenderJob::Latex(LatexJob {
                host: "local".into(),
                id,
                seq,
                srcs,
            })
        };
        let deep = "\\frac ".repeat(300);
        jobs.send(job(1, vec!["E = mc^2".into()])).unwrap();
        jobs.send(job(2, vec!["E = mc^2".into(), deep.clone()])).unwrap();
        let wait = Duration::from_secs(10);
        let (seq, first) = rx.recv_timeout(wait).unwrap();
        assert_eq!(seq, 1);
        assert_eq!(first[0].inline.as_deref(), Some("E = mc²"));
        let (seq, second) = rx.recv_timeout(wait).unwrap();
        assert_eq!(seq, 2);
        assert_eq!(second[0], first[0]);
        assert_eq!(second[1], latex::Formula::raw(&deep), "too complex: raw");
    }

    /// AC7: answers go through the same worker and guard. Hostile ones
    /// (thousands of tags, a 32-deep nested script, the hostile formula
    /// corpus, a 64 KiB line, private-use input) come back bounded and
    /// sanitized, and lay out with no inline box taller than three rows.
    #[test]
    fn hostile_answers_are_rendered_bounded_and_sanitized() {
        use crate::proto::Answer;
        use reader::layout::layout;
        let (tx, rx) = std::sync::mpsc::channel();
        let jobs = spawn_latex_worker(move |done| {
            if let Rendered::Answers(job, answers, _) = done {
                let _ = tx.send((job.gen, answers));
            }
        })
        .unwrap();
        let nested = format!("x{}{}", "_{a".repeat(32), "}".repeat(32));
        let corpus = [
            "\\frac ".repeat(340),
            "x/".repeat(255),
            "x\u{1b}[2J\u{1b}]52;c;AAAA\u{7}y".to_owned(),
            "a\u{202e}b\u{200b}c".to_owned(),
        ];
        let texts = [
            "<latex>x</latex> ".repeat(4000),
            format!("inline <latex>{nested}</latex> end\n\n<latex>{nested}</latex>"),
            corpus
                .iter()
                .map(|f| format!("<latex>{f}</latex>\n\n"))
                .collect(),
            "w".repeat(64 * 1024),
            "\u{e000}forged\u{e001} <latex>\u{e002}y</latex>".to_owned(),
        ];
        let answers: Vec<Answer> = texts
            .iter()
            .map(|text| Answer {
                prompt: "p\u{1b}[2J".into(),
                text: text.clone(),
            })
            .collect();
        let started = Instant::now();
        let job = AnswersJob {
            gen: 7,
            id: uuid::Uuid::new_v4(),
            answers,
            tags: vec!["latex".into()],
            fences: vec![],
            external: vec![],
        };
        jobs.send(RenderJob::Answers(job)).unwrap();
        let (gen, rendered) = rx.recv_timeout(Duration::from_secs(30)).unwrap();
        assert_eq!(gen, 7);
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(rendered.len(), texts.len());
        assert_eq!(
            rendered[0].formulas.len(),
            crate::claude::latex::MAX_FORMULAS
        );
        for answer in &rendered {
            for f in &answer.formulas {
                let laid = f.block.iter().chain(&f.text);
                for text in f.inline.iter().chain(laid.flat_map(|l| &l.rows)) {
                    assert_eq!(*text, crate::claude::latex::sanitize(text));
                }
            }
            for line in layout(&answer.doc, &answer.formulas, 80) {
                let text = line.text();
                assert_eq!(text, crate::claude::latex::sanitize(&text));
                assert!(!text.contains(['\u{e000}', '\u{e001}', '\u{e002}']));
            }
        }
        // The nested script is taller than three rows: inline, it is its
        // one-line form, so its paragraph is one line.
        let nested_doc = &rendered[1];
        assert!(nested_doc.formulas[0].text.as_ref().unwrap().rows.len() > 3);
        let lines = layout(&nested_doc.doc, &nested_doc.formulas, 200);
        assert!(
            lines[0].text().starts_with("inline x"),
            "{:?}",
            lines[0].text()
        );
        assert!(
            lines[1].text().is_empty(),
            "one line, then the display formula"
        );
    }

    #[test]
    fn the_cache_forgets_the_least_recently_used() {
        let running = Arc::new(AtomicUsize::new(0));
        let mut cache = RenderCache::default();
        cache.formula("0", &running);
        for i in 1..CACHE_SIZE {
            cache.formula(&i.to_string(), &running);
        }
        cache.formula("0", &running); // used again: "1" is now the oldest
        cache.formula("new", &running);
        assert_eq!(cache.entries.len(), CACHE_SIZE);
        let has = |src: &str| cache.entries.contains_key(&("latex".to_owned(), src.to_owned()));
        assert!(has("0") && has("new"));
        assert!(!has("1"));
    }

    #[test]
    fn answers_render_every_tag_lens_and_fence_lens_by_its_renderer() {
        use crate::proto::Answer;
        let (tx, rx) = std::sync::mpsc::channel();
        let jobs = spawn_latex_worker(move |done| {
            if let Rendered::Answers(_, answers, external) = done {
                let _ = tx.send((answers, external));
            }
        })
        .unwrap();
        let text = "Press <keys>Ctrl+C</keys> for <latex>x^2</latex>.\n\n\
                    ```mermaid\nflowchart LR\n  a --> b\n```\n\n```rust\nfn main() {}\n```\n";
        let job = AnswersJob {
            gen: 1,
            id: uuid::Uuid::new_v4(),
            answers: vec![Answer {
                prompt: "p".into(),
                text: text.into(),
            }],
            tags: vec!["latex".into(), "keys".into()],
            fences: vec!["mermaid".into()],
            external: vec!["keys".into()],
        };
        jobs.send(RenderJob::Answers(job)).unwrap();
        let (answers, external) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let answer = &answers[0];
        // The external lens's item stays raw, listed for the host; latex is
        // rendered.
        assert_eq!(answer.formulas[0], latex::Formula::raw("Ctrl+C"));
        let want = reader::External {
            answer: 0,
            slot: reader::Slot::Formula(0),
            lens: "keys".into(),
            kind: crate::proto::LensKind::Tag,
            source: "Ctrl+C".into(),
        };
        assert_eq!(external, [want]);
        assert_eq!(answer.formulas[1].inline.as_deref(), Some("x²"));
        // The fence lens is a lens block; another fence stays code.
        let lens: Vec<&reader::doc::Block> = answer
            .doc
            .blocks
            .iter()
            .filter(|b| matches!(b, reader::doc::Block::Lens { .. }))
            .collect();
        assert_eq!(lens.len(), 1);
        assert!(answer
            .doc
            .blocks
            .iter()
            .any(|b| matches!(b, reader::doc::Block::Code(lines) if lines == &["fn main() {}"])));
    }

    #[test]
    fn the_cache_keeps_lenses_apart() {
        let running = Arc::new(AtomicUsize::new(0));
        let mut cache = RenderCache::default();
        let latex = cache.render("latex", "x^2", &running);
        let keys = cache.render("keys", "x^2", &running);
        assert_eq!(latex.inline.as_deref(), Some("x²"));
        assert_eq!(keys, latex::Formula::raw("x^2"));
        assert_eq!(cache.entries.len(), 2);
    }

    /// AC8: a Mermaid fence is drawn on the worker through the guard; an
    /// unsupported or too large one stays its code block.
    #[test]
    fn mermaid_fences_are_drawn_through_the_guard() {
        let running = Arc::new(AtomicUsize::new(0));
        let drawn = render_lens("mermaid", "flowchart LR\n  alpha --> beta", &running);
        let rows = drawn.0.block.expect("drawn").rows;
        assert!(rows.iter().any(|r| r.contains("alpha")), "{rows:?}");
        assert!(rows.iter().any(|r| r.contains("beta")), "{rows:?}");
        for src in ["classDiagram\n  A <|-- B".to_owned(), "flowchart LR\n".repeat(300)] {
            let (raw, keep) = render_lens("mermaid", &src, &running);
            assert_eq!(raw, latex::Formula::raw(&src));
            assert!(keep);
        }
    }
}
