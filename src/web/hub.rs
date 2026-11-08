//! The hub: one more daemon client per host, exactly like the TUI, shared by
//! every browser tab.
//!
//! One actor task owns all state: the hosts and their connections, the
//! sessions as the daemons report them, the tabs and `web-state.json`.
//! Everything that waits on a daemon runs in a spawned task that reports
//! back with a [`Cmd`]; the actor itself never awaits a daemon request, a
//! tab or a file. Tab request handlers (`ws.rs`) take a [`Client`] clone
//! from [`HubHandle::session`] / [`HubHandle::host_client`] and make their
//! read-only requests themselves.
//!
//! Output fan-out: one daemon attach per session, however many tabs watch
//! it ([`Subs`]); each tab has an unbounded queue capped by queued bytes
//! ([`TabTx`]), so a slow tab is cut (1013) instead of stalling the hub.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Bytes;
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc, oneshot};

use super::dto;
use super::input::{self, Decision};
use super::state::{self, Layout, SessionMeta, WebState};
use crate::claude::update::{decide, needs_update, short, Decision as Policy};
use crate::client::{self, Client, Incoming};
use crate::config::UpdatePolicy;
use crate::freshness::{self, DaemonCheck};
use crate::paths;
use crate::proto::{
    ClaudeInfo, GitStatus, LensItem, Msg, SessionEvent, SessionId, SessionInfo, SessionState,
    Welcome, UPDATE_BUSY,
};
use crate::tui::connections::Connections;

pub(crate) type TabId = u64;
/// The hub's own subscription (an opening prompt), never a real tab.
pub(crate) const PROMPT_TAB: TabId = 0;
pub(crate) const MAX_TABS: usize = 16;
pub(crate) const TAB_QUEUE_MAX: usize = 8 << 20;
pub(crate) const OUTDATED_RETRY: Duration = Duration::from_secs(120);
pub(crate) const PROMPT_DEADLINE: Duration = Duration::from_secs(60);
pub(crate) const PROMPT_QUIET: Duration = Duration::from_millis(500);
pub(crate) const TICK: Duration = Duration::from_millis(100);

/// The fewest milliseconds between two coalesced web-state writes.
const SAVE_EVERY: Duration = Duration::from_millis(500);
/// The size a session with subscribers is re-attached at after a
/// reconnect when no `Attached` told its size yet.
const DEFAULT_SIZE: (u16, u16) = (24, 80);
/// Binary frame kind: server → client PTY output.
pub(crate) const KIND_OUTPUT: u8 = 0x01;
/// The longest error text the browser gets.
const ERROR_MAX: usize = 300;
/// The longest `claude` event tail, in bytes.
const TAIL_MAX: usize = 2048;

/// One frame for a tab's writer.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Out {
    Text(String),
    Binary(Bytes),
    Close(u16, &'static str),
}

impl Out {
    /// The bytes this frame holds in a tab's queue.
    pub(crate) fn size(&self) -> usize {
        match self {
            Out::Text(s) => s.len(),
            Out::Binary(b) => b.len(),
            Out::Close(..) => 0,
        }
    }
}

/// A tab's outgoing queue, bounded by queued bytes.
#[derive(Clone)]
pub(crate) struct TabTx {
    tx: mpsc::UnboundedSender<Out>,
    queued: Arc<AtomicUsize>,
    cut: Arc<AtomicBool>,
}

impl TabTx {
    pub(crate) fn new() -> (TabTx, mpsc::UnboundedReceiver<Out>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let tab = TabTx {
            tx,
            queued: Arc::new(AtomicUsize::new(0)),
            cut: Arc::new(AtomicBool::new(false)),
        };
        (tab, rx)
    }

    /// Queue `out`. Past TAB_QUEUE_MAX queued bytes the tab is cut: one
    /// Close(1013, "too slow") is queued, false is returned, and every later
    /// send is dropped.
    pub(crate) fn send(&self, out: Out) -> bool {
        if self.cut.load(Ordering::Acquire) {
            return false;
        }
        let n = out.size();
        let before = self.queued.fetch_add(n, Ordering::AcqRel);
        if before + n > TAB_QUEUE_MAX {
            self.queued.fetch_sub(n, Ordering::AcqRel);
            if !self.cut.swap(true, Ordering::AcqRel) {
                let _ = self.tx.send(Out::Close(1013, "too slow"));
            }
            return false;
        }
        if self.tx.send(out).is_err() {
            self.queued.fetch_sub(n, Ordering::AcqRel);
            return false;
        }
        true
    }

    /// The writer took `n` bytes off the queue.
    pub(crate) fn sent(&self, n: usize) {
        let _ = self
            .queued
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |q| {
                Some(q.saturating_sub(n))
            });
    }
}

/// Per-session subscribers: a set of tabs (PROMPT_TAB included).
#[derive(Default)]
pub(crate) struct Subs {
    tabs: BTreeSet<TabId>,
    /// The size the first subscriber attached at, until `Attached` says.
    asked: Option<(u16, u16)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttachStep {
    Attach { rows: u16, cols: u16 },
    Reattach { rows: u16, cols: u16 },
}

impl AttachStep {
    fn size(self) -> (u16, u16) {
        match self {
            AttachStep::Attach { rows, cols } | AttachStep::Reattach { rows, cols } => (rows, cols),
        }
    }
}

impl Subs {
    /// First subscriber: attach at its size. Later: re-attach at `current`
    /// (the session's size from the last `Attached`), so the PTY is not
    /// resized and every subscriber gets a fresh snapshot.
    pub(crate) fn subscribe(
        &mut self,
        tab: TabId,
        want: (u16, u16),
        current: Option<(u16, u16)>,
    ) -> AttachStep {
        let first = self.tabs.is_empty();
        self.tabs.insert(tab);
        if first {
            self.asked = Some(want);
            AttachStep::Attach {
                rows: want.0,
                cols: want.1,
            }
        } else {
            let (rows, cols) = current.or(self.asked).unwrap_or(want);
            AttachStep::Reattach { rows, cols }
        }
    }

    /// Whether that was the last subscriber (send `Detach`).
    pub(crate) fn unsubscribe(&mut self, tab: TabId) -> bool {
        let last = self.tabs.remove(&tab) && self.tabs.is_empty();
        if last {
            self.asked = None;
        }
        last
    }

    pub(crate) fn tabs(&self) -> impl Iterator<Item = TabId> + '_ {
        self.tabs.iter().copied()
    }

    fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }
}

/// An opening prompt waiting for the session to take it.
pub(crate) struct PendingPrompt {
    text: String,
    deadline: Instant,
    saw_input: bool,
    bracketed: bool,
    last_output: Option<Instant>,
    /// The last bytes of the output, so a mode switch split across two
    /// chunks is still seen.
    tail: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PromptStep {
    Wait,
    Paste,
    GiveUp,
}

const BRACKETED_ON: &[u8] = b"\x1b[?2004h";
const BRACKETED_OFF: &[u8] = b"\x1b[?2004l";

impl PendingPrompt {
    pub(crate) fn new(text: String, now: Instant) -> PendingPrompt {
        PendingPrompt {
            text,
            deadline: now + PROMPT_DEADLINE,
            saw_input: false,
            bracketed: false,
            last_output: None,
            tail: Vec::new(),
        }
    }

    /// `NeedsInput` sets saw_input.
    pub(crate) fn on_state(&mut self, state: SessionState) {
        if state == SessionState::NeedsInput {
            self.saw_input = true;
        }
    }

    /// Finds `ESC[?2004h` (and `l`) across chunk edges; stamps last_output.
    pub(crate) fn on_output(&mut self, bytes: &[u8], now: Instant) {
        self.last_output = Some(now);
        let mut window = std::mem::take(&mut self.tail);
        window.extend_from_slice(bytes);
        let last = |pat: &[u8]| window.windows(pat.len()).rposition(|w| w == pat);
        match (last(BRACKETED_ON), last(BRACKETED_OFF)) {
            (Some(on), Some(off)) => self.bracketed = on > off,
            (Some(_), None) => self.bracketed = true,
            (None, Some(_)) => self.bracketed = false,
            (None, None) => {}
        }
        let keep = BRACKETED_ON.len() - 1;
        self.tail = window[window.len().saturating_sub(keep)..].to_vec();
    }

    /// Paste when saw_input && bracketed && quiet ≥ 500 ms; GiveUp at the
    /// deadline.
    pub(crate) fn step(&self, now: Instant) -> PromptStep {
        if now >= self.deadline {
            return PromptStep::GiveUp;
        }
        let quiet = self
            .last_output
            .is_none_or(|t| now.saturating_duration_since(t) >= PROMPT_QUIET);
        if self.saw_input && self.bracketed && quiet {
            PromptStep::Paste
        } else {
            PromptStep::Wait
        }
    }
}

/// One host as the hub knows it.
pub(crate) struct HostState {
    pub name: String,
    pub client: Option<Client>,
    pub generation: u64,
    pub connecting: bool,
    pub error: Option<String>,
    pub welcome: Option<Welcome>,
    pub outdated: bool,
    /// `(cpu_pct, mem_used, mem_total)` of the last `HostStats`.
    pub stats: Option<(f32, u64, u64)>,
    pub features: Vec<String>,
    /// The open offer, or the running install/update, as its `claude` event.
    pub claude_offer: Option<Value>,
    /// The claude check ran for this host in this run.
    pub offered: bool,
    /// A failed connection is retried (a remembered host, or one that was
    /// connected); a first `hosts.connect` that fails is dropped instead.
    retry: bool,
}

impl HostState {
    pub(crate) fn new(name: &str) -> HostState {
        HostState {
            name: name.to_owned(),
            client: None,
            generation: 0,
            connecting: false,
            error: None,
            welcome: None,
            outdated: false,
            stats: None,
            features: Vec::new(),
            claude_offer: None,
            offered: false,
            retry: false,
        }
    }

    fn has(&self, feature: &str) -> bool {
        self.features.iter().any(|f| f == feature)
    }
}

/// One session as the hub knows it.
pub(crate) struct SessionView {
    pub host: String,
    /// As the daemon reported it, updated by events.
    pub info: SessionInfo,
    pub git: Option<GitStatus>,
    pub output_tokens: Option<u64>,
    pub turns: Option<u32>,
    pub state_since: u64,
    pub lens_seq: u64,
    pub lens: Vec<LensItem>,
    pub subs: Subs,
    /// From the last Attached.
    pub size: Option<(u16, u16)>,
    pub prompt: Option<PendingPrompt>,
    /// Known proxy/first_party.
    pub meta: Option<(Option<String>, bool)>,
}

impl SessionView {
    pub(crate) fn new(host: &str, info: SessionInfo, now: u64) -> SessionView {
        SessionView {
            host: host.to_owned(),
            info,
            git: None,
            output_tokens: None,
            turns: None,
            state_since: now,
            lens_seq: 0,
            lens: Vec::new(),
            subs: Subs::default(),
            size: None,
            prompt: None,
            meta: None,
        }
    }
}

/// What [`apply_event`] made of one event.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Applied {
    pub changed: bool,
    pub removed: bool,
    pub lens: Option<(u64, Vec<LensItem>)>,
    pub notice: Option<String>,
}

/// Apply one daemon event to a session as `App::on_event` does for the
/// DTO's fields; `lens_host` says whether its daemon has the `lens`
/// feature. Returns whether the DTO changed and the `lens`/`latex` events
/// to send (if any). Pure: no I/O.
pub(crate) fn apply_event(
    view: &mut SessionView,
    event: SessionEvent,
    lens_host: bool,
    now: u64,
) -> Applied {
    let mut out = Applied::default();
    match event {
        // A respawn: same id, new process.
        SessionEvent::Created { info } => {
            let old = std::mem::replace(&mut view.info, info);
            let info = &mut view.info;
            if info.title.is_none() {
                info.title = old.title;
            }
            if info.claude_session_id.is_none() {
                info.claude_session_id = old.claude_session_id;
            }
            if info.branch.is_none() {
                info.branch = old.branch;
            }
            if info.model.is_none() {
                info.model = old.model;
            }
            if info.context_tokens.is_none() {
                info.context_tokens = old.context_tokens;
            }
            info.attention = None;
            view.state_since = now;
            view.lens_seq = 0;
            if let Some(p) = &mut view.prompt {
                p.on_state(view.info.state);
            }
            out.changed = true;
        }
        SessionEvent::Removed => out.removed = true,
        SessionEvent::State { state } => {
            if view.info.state != state {
                view.state_since = now;
            }
            view.info.state = state;
            if !matches!(
                state,
                SessionState::NeedsApproval | SessionState::NeedsInput
            ) {
                view.info.attention = None;
            }
            if let Some(p) = &mut view.prompt {
                p.on_state(state);
            }
            out.changed = true;
        }
        SessionEvent::Attention { attention } => {
            view.info.attention = Some(attention);
            out.changed = true;
        }
        SessionEvent::Exited { .. } => {
            if view.info.state != SessionState::Exited {
                view.state_since = now;
            }
            view.info.state = SessionState::Exited;
            view.info.pid = None;
            view.info.attention = None;
            out.changed = true;
        }
        SessionEvent::ClaudeSession { claude_session_id } => {
            view.info.claude_session_id = Some(claude_session_id);
            out.changed = true;
        }
        SessionEvent::ClaudeSessionCleared => {
            view.info.claude_session_id = None;
            out.changed = true;
        }
        SessionEvent::Title { title } => {
            view.info.title = Some(title);
            out.changed = true;
        }
        SessionEvent::Renamed { name } => {
            view.info.name = name;
            out.changed = true;
        }
        SessionEvent::Notice { text } => out.notice = Some(text),
        // `None` is "no news", never "cleared".
        SessionEvent::Meta {
            branch,
            model,
            context_tokens,
            git,
            output_tokens,
            turns,
        } => {
            let mut changed = false;
            if branch.is_some() {
                changed |= view.info.branch != branch;
                view.info.branch = branch;
            }
            if model.is_some() {
                changed |= view.info.model != model;
                view.info.model = model;
            }
            if context_tokens.is_some() {
                changed |= view.info.context_tokens != context_tokens;
                view.info.context_tokens = context_tokens;
            }
            if git.is_some() {
                changed |= view.git != git;
                view.git = git;
            }
            if output_tokens.is_some() {
                changed |= view.output_tokens != output_tokens;
                view.output_tokens = output_tokens;
            }
            if turns.is_some() {
                changed |= view.turns != turns;
                view.turns = turns;
            }
            out.changed = changed;
        }
        // A daemon without the `lens` feature sends formulas only: they are
        // its `latex` items. A lens daemon's are in its `Lens` events.
        SessionEvent::Latex { seq, formulas } => {
            if !lens_host {
                let items = formulas
                    .into_iter()
                    .map(|text| LensItem {
                        tag: "latex".to_owned(),
                        text,
                    })
                    .collect();
                out.lens = take_lens(view, seq, items);
            }
        }
        SessionEvent::Lens { seq, items } => {
            if lens_host {
                out.lens = take_lens(view, seq, items);
            }
        }
        SessionEvent::Unknown => {}
    }
    out
}

/// Take lens list `seq`, unless a newer one is held (a replay and a
/// broadcast may cross).
fn take_lens(
    view: &mut SessionView,
    seq: u64,
    items: Vec<LensItem>,
) -> Option<(u64, Vec<LensItem>)> {
    if seq <= view.lens_seq {
        return None;
    }
    view.lens_seq = seq;
    view.lens = items.clone();
    Some((seq, items))
}

/// The daemon (re)attached the session: its size, and any lens `seq` is
/// taken next (a restarted daemon counts from 1 again).
pub(crate) fn on_attached(view: &mut SessionView, rows: u16, cols: u16) {
    view.size = Some((rows, cols));
    view.lens_seq = 0;
}

/// What other parts of the server (the plugins) follow.
#[derive(Debug, Clone)]
pub(crate) enum HubEvent {
    HostUp {
        host: String,
        features: Vec<String>,
        has_claude: bool,
    },
    HostDown {
        host: String,
    },
    ClaudeInstalled {
        host: String,
    },
}

pub(crate) struct HubConfig {
    pub config: crate::config::Config,
    /// web-state.json
    pub state_path: PathBuf,
    /// Read at start, never written.
    pub tui_state: crate::tui::state::ClientState,
    pub profiles: (Vec<String>, Option<String>),
    /// web-state.json as read at start (in `spawn_blocking`, by `serve`).
    pub web: WebState,
}

/// A session as a request handler needs it.
pub(crate) struct SessionRef {
    pub host: String,
    pub client: Client,
    pub features: Vec<String>,
    pub info: SessionInfo,
    pub proxy: Option<String>,
    pub first_party: bool,
    pub proxy_known: bool,
}

type Reply<T> = oneshot::Sender<T>;
/// Run after a web-state write, with its outcome.
type Done = Box<dyn FnOnce(Result<(), String>) + Send>;

enum Cmd {
    OpenTab {
        tx: TabTx,
        reply: Reply<Result<TabId, &'static str>>,
    },
    CloseTab {
        tab: TabId,
    },
    Attach {
        tab: TabId,
        sid: SessionId,
        rows: u16,
        cols: u16,
        reply: Reply<Result<(), String>>,
    },
    AttachFailed {
        tab: TabId,
        sid: SessionId,
    },
    Detach {
        tab: TabId,
        sid: SessionId,
        reply: Reply<Result<(), String>>,
    },
    Resize {
        sid: SessionId,
        rows: u16,
        cols: u16,
        reply: Reply<Result<(), String>>,
    },
    Input {
        sid: SessionId,
        bytes: Vec<u8>,
    },
    Approve {
        sid: SessionId,
        decision: Decision,
        seq: u64,
        reply: Reply<Result<(), String>>,
    },
    Paste {
        sid: SessionId,
        text: String,
        submit: bool,
        reply: Reply<Result<(), String>>,
    },
    Session {
        sid: SessionId,
        reply: Reply<Result<SessionRef, String>>,
    },
    HostClient {
        host: String,
        reply: Reply<Result<(Client, Vec<String>), String>>,
    },
    Hosts {
        reply: Reply<Vec<Value>>,
    },
    Connect {
        host: String,
        reply: Reply<Result<Value, String>>,
    },
    Disconnect {
        host: String,
        reply: Reply<Result<(), String>>,
    },
    Spawned {
        host: String,
        sid: SessionId,
        meta: SessionMeta,
        prompt: Option<(String, (u16, u16))>,
        reply: Reply<()>,
    },
    Order {
        tab: TabId,
        sids: Vec<SessionId>,
        reply: Reply<Result<(), String>>,
    },
    Layout {
        tab: TabId,
        layout: Layout,
        reply: Reply<Result<(), String>>,
    },
    Broadcast(Value),
    ClaudeUpdate {
        host: String,
        install: bool,
        reply: Reply<Result<(), String>>,
    },
    ClaudeSkip {
        host: String,
        reply: Reply<()>,
    },
    SetProfiles {
        names: Vec<String>,
        default: Option<String>,
        reply: Reply<()>,
    },
    /// config.toml's `[claude]` was re-read.
    SetClaude {
        claude: crate::config::ClaudeSection,
    },
    Shutdown {
        reply: Reply<()>,
    },
    // ── from the hub's own tasks ─────────────────────────────────────────
    Connected {
        host: String,
        generation: u64,
        client: Client,
        sessions: Vec<SessionInfo>,
    },
    ConnectFailed {
        host: String,
        generation: u64,
        error: String,
        bootstrap: bool,
    },
    Incoming {
        host: String,
        generation: u64,
        inc: Incoming,
    },
    /// A backoff delay ran out.
    Retry {
        host: String,
        generation: u64,
    },
    /// An outdated daemon is idle now: reconnect (which replaces it).
    Reconnect {
        host: String,
        generation: u64,
    },
    ClaudeDone {
        host: String,
        install: bool,
        result: Result<Msg, String>,
    },
}

#[derive(Clone)]
pub(crate) struct HubHandle {
    tx: mpsc::Sender<Cmd>,
    events: broadcast::Sender<HubEvent>,
}

const GONE: &str = "server shutting down";

/// Start the actor; it connects `local` and every remembered remote host.
pub(crate) fn start(cfg: HubConfig) -> HubHandle {
    let (tx, rx) = mpsc::channel(1024);
    let (events, _) = broadcast::channel(64);
    let (writer, jobs) = mpsc::unbounded_channel();
    tokio::spawn(write_state(cfg.state_path.clone(), jobs));
    let web = cfg.web;
    let mut hub = Hub {
        claude: cfg.config.claude.clone(),
        tui_state: cfg.tui_state,
        profiles: cfg.profiles,
        web,
        dirty: false,
        last_save: Instant::now(),
        hosts: BTreeMap::new(),
        conns: Connections {
            map: HashMap::new(),
        },
        sessions: HashMap::new(),
        pending_spawns: HashMap::new(),
        tabs: BTreeMap::new(),
        next_tab: PROMPT_TAB + 1,
        connect_waiters: HashMap::new(),
        tx: tx.clone(),
        events: events.clone(),
        writer,
        local_claude: None,
    };
    // `local`, then the remembered hosts and the TUI's remote ones.
    let mut hosts = vec!["local".to_owned()];
    let remembered = hub.web.hosts.iter().cloned();
    let tui_hosts = hub.tui_state.sessions.iter().map(|s| s.host.clone());
    for host in remembered.chain(tui_hosts) {
        if host_ok(&host).is_ok() && host != "local" && !hosts.contains(&host) {
            hosts.push(host);
        }
    }
    for host in hosts {
        let mut h = HostState::new(&host);
        h.retry = true;
        hub.hosts.insert(host.clone(), h);
        hub.conns.ensure_host(&host);
        let generation = hub.conns.next_generation(&host);
        hub.start_attempt(&host, generation, true);
    }
    tokio::spawn(run(hub, rx));
    HubHandle { tx, events }
}

/// A host name the web may connect to: what `remote::validate_host` takes,
/// at most 255 bytes, no whitespace or control characters.
fn host_ok(host: &str) -> Result<(), String> {
    crate::remote::validate_host(host)?;
    if host.len() > 255 || host.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(format!("invalid host {:?}", clean(host, 80)));
    }
    Ok(())
}

impl HubHandle {
    async fn ask<T>(&self, make: impl FnOnce(Reply<T>) -> Cmd) -> Result<T, String> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(make(reply))
            .await
            .map_err(|_| GONE.to_owned())?;
        rx.await.map_err(|_| GONE.to_owned())
    }

    pub(crate) async fn open_tab(&self, tx: TabTx) -> Result<TabId, &'static str> {
        self.ask(|reply| Cmd::OpenTab { tx, reply })
            .await
            .unwrap_or(Err(GONE))
    }

    pub(crate) fn close_tab(&self, tab: TabId) {
        let tx = self.tx.clone();
        // Never lost, even with the queue full.
        tokio::spawn(async move {
            let _ = tx.send(Cmd::CloseTab { tab }).await;
        });
    }

    pub(crate) async fn attach(
        &self,
        tab: TabId,
        sid: SessionId,
        rows: u16,
        cols: u16,
    ) -> Result<(), String> {
        self.ask(|reply| Cmd::Attach {
            tab,
            sid,
            rows,
            cols,
            reply,
        })
        .await?
    }

    pub(crate) async fn detach(&self, tab: TabId, sid: SessionId) -> Result<(), String> {
        self.ask(|reply| Cmd::Detach { tab, sid, reply }).await?
    }

    pub(crate) async fn resize(&self, sid: SessionId, rows: u16, cols: u16) -> Result<(), String> {
        self.ask(|reply| Cmd::Resize {
            sid,
            rows,
            cols,
            reply,
        })
        .await?
    }

    pub(crate) fn input(&self, sid: SessionId, bytes: Vec<u8>) {
        if self.tx.try_send(Cmd::Input { sid, bytes }).is_err() {
            tracing::warn!(%sid, "web: input dropped: hub congested");
        }
    }

    pub(crate) async fn approve(
        &self,
        sid: SessionId,
        decision: Decision,
        seq: u64,
    ) -> Result<(), String> {
        self.ask(|reply| Cmd::Approve {
            sid,
            decision,
            seq,
            reply,
        })
        .await?
    }

    pub(crate) async fn paste(
        &self,
        sid: SessionId,
        text: String,
        submit: bool,
    ) -> Result<(), String> {
        self.ask(|reply| Cmd::Paste {
            sid,
            text,
            submit,
            reply,
        })
        .await?
    }

    pub(crate) async fn session(&self, sid: SessionId) -> Result<SessionRef, String> {
        self.ask(|reply| Cmd::Session { sid, reply }).await?
    }

    pub(crate) async fn host_client(&self, host: &str) -> Result<(Client, Vec<String>), String> {
        let host = host.to_owned();
        self.ask(|reply| Cmd::HostClient { host, reply }).await?
    }

    pub(crate) async fn hosts(&self) -> Vec<Value> {
        self.ask(|reply| Cmd::Hosts { reply })
            .await
            .unwrap_or_default()
    }

    pub(crate) async fn connect(&self, host: String) -> Result<Value, String> {
        if host == "local" {
            return Err("local is always connected".into());
        }
        host_ok(&host)?;
        self.ask(|reply| Cmd::Connect { host, reply }).await?
    }

    pub(crate) async fn disconnect(&self, host: String) -> Result<(), String> {
        if host == "local" {
            return Err("cannot disconnect local".into());
        }
        self.ask(|reply| Cmd::Disconnect { host, reply }).await?
    }

    /// A spawn this server made: record its proxy (web-state.json) and, with
    /// `prompt`, subscribe PROMPT_TAB at `size` and wait to paste it.
    pub(crate) async fn spawned(
        &self,
        host: String,
        sid: SessionId,
        meta: SessionMeta,
        prompt: Option<(String, (u16, u16))>,
    ) {
        let _ = self
            .ask(|reply| Cmd::Spawned {
                host,
                sid,
                meta,
                prompt,
                reply,
            })
            .await;
    }

    pub(crate) async fn order(&self, tab: TabId, sids: Vec<SessionId>) {
        let _ = self.ask(|reply| Cmd::Order { tab, sids, reply }).await;
    }

    pub(crate) async fn layout(&self, tab: TabId, layout: Layout) -> Result<(), String> {
        self.ask(|reply| Cmd::Layout { tab, layout, reply }).await?
    }

    /// `ev` (a whole event) to every tab.
    pub(crate) fn broadcast(&self, ev: Value) {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let _ = tx.send(Cmd::Broadcast(ev)).await;
        });
    }

    /// Host connections, losses and claude installs, for the plugins.
    pub(crate) fn subscribe(&self) -> broadcast::Receiver<HubEvent> {
        self.events.subscribe()
    }

    pub(crate) async fn claude_update(&self, host: String, install: bool) -> Result<(), String> {
        self.ask(|reply| Cmd::ClaudeUpdate {
            host,
            install,
            reply,
        })
        .await?
    }

    pub(crate) async fn claude_skip(&self, host: String) {
        let _ = self.ask(|reply| Cmd::ClaudeSkip { host, reply }).await;
    }

    /// config.toml's proxy profiles changed: a `profiles` event to every tab.
    pub(crate) async fn set_profiles(&self, names: Vec<String>, default: Option<String>) {
        let _ = self
            .ask(|reply| Cmd::SetProfiles {
                names,
                default,
                reply,
            })
            .await;
    }

    /// config.toml's `[claude]` changed: the install and update policy of
    /// the hosts not offered yet.
    pub(crate) async fn set_claude_policy(&self, claude: crate::config::ClaudeSection) {
        let _ = self.tx.send(Cmd::SetClaude { claude }).await;
    }

    pub(crate) async fn shutdown(&self) {
        let _ = self.ask(|reply| Cmd::Shutdown { reply }).await;
    }
}

/// The actor's state.
struct Hub {
    claude: crate::config::ClaudeSection,
    tui_state: crate::tui::state::ClientState,
    profiles: (Vec<String>, Option<String>),
    web: WebState,
    dirty: bool,
    last_save: Instant,
    hosts: BTreeMap<String, HostState>,
    conns: Connections,
    sessions: HashMap<SessionId, SessionView>,
    /// What `spawned()` said of a session whose `Created` has not come yet.
    pending_spawns: HashMap<SessionId, (SessionMeta, Option<(PendingPrompt, (u16, u16))>)>,
    tabs: BTreeMap<TabId, TabTx>,
    next_tab: TabId,
    connect_waiters: HashMap<String, Vec<Reply<Result<Value, String>>>>,
    tx: mpsc::Sender<Cmd>,
    events: broadcast::Sender<HubEvent>,
    writer: mpsc::UnboundedSender<WriteJob>,
    /// The local claude's `--version` line, from the local Welcome.
    local_claude: Option<String>,
}

async fn run(mut hub: Hub, mut rx: mpsc::Receiver<Cmd>) {
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let start = tokio::time::Instant::now() + OUTDATED_RETRY;
    let mut outdated = tokio::time::interval_at(start, OUTDATED_RETRY);
    loop {
        tokio::select! {
            cmd = rx.recv() => match cmd {
                Some(Cmd::Shutdown { reply }) => {
                    hub.shutdown(reply);
                    break;
                }
                Some(cmd) => hub.handle(cmd),
                None => break,
            },
            _ = tick.tick() => hub.on_tick(),
            _ = outdated.tick() => hub.check_outdated(),
        }
    }
}

/// Unix seconds now.
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// `text` sanitized and cut at `max` characters.
fn clean(text: &str, max: usize) -> String {
    crate::claude::latex::sanitize(text)
        .chars()
        .take(max)
        .collect()
}

/// A daemon error for the browser: sanitized, ≤ 300 characters, and the
/// daemon's `no such session: <id>` as the protocol's `no such session`.
pub(crate) fn daemon_error(e: impl std::fmt::Display) -> String {
    let text = clean(&e.to_string(), ERROR_MAX);
    if text.starts_with("no such session") {
        "no such session".to_owned()
    } else {
        text
    }
}

/// The last `TAIL_MAX` bytes of `text`, sanitized, on a char boundary.
fn tail(text: &str) -> String {
    let text = crate::claude::latex::sanitize(text);
    let mut at = text.len().saturating_sub(TAIL_MAX);
    while !text.is_char_boundary(at) {
        at += 1;
    }
    text[at..].to_owned()
}

fn ev(name: &str, mut fields: Value) -> Value {
    fields["t"] = "ev".into();
    fields["ev"] = name.into();
    fields
}

impl Hub {
    fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::OpenTab { tx, reply } => {
                let _ = reply.send(self.open_tab(tx));
            }
            Cmd::CloseTab { tab } => self.close_tab(tab),
            Cmd::Attach {
                tab,
                sid,
                rows,
                cols,
                reply,
            } => self.attach(tab, sid, (rows, cols), reply),
            Cmd::AttachFailed { tab, sid } => {
                if let Some(view) = self.sessions.get_mut(&sid) {
                    view.subs.unsubscribe(tab);
                }
            }
            Cmd::Detach { tab, sid, reply } => {
                let _ = reply.send(self.detach(tab, sid));
            }
            Cmd::Resize {
                sid,
                rows,
                cols,
                reply,
            } => self.resize(sid, (rows, cols), reply),
            Cmd::Input { sid, bytes } => {
                if let Ok((client, view)) = self.live(sid) {
                    if view.info.state != SessionState::Exited {
                        client.send_input(sid, &bytes);
                    }
                }
            }
            Cmd::Approve {
                sid,
                decision,
                seq,
                reply,
            } => {
                let _ = reply.send(self.approve(sid, decision, seq));
            }
            Cmd::Paste {
                sid,
                text,
                submit,
                reply,
            } => {
                let result = self.live(sid).and_then(|(client, _)| {
                    let bytes = input::paste_bytes(&text, submit).map_err(str::to_owned)?;
                    client.send_input(sid, &bytes);
                    Ok(())
                });
                let _ = reply.send(result);
            }
            Cmd::Session { sid, reply } => {
                let _ = reply.send(self.session_ref(sid));
            }
            Cmd::HostClient { host, reply } => {
                let result = match self.hosts.get(&host) {
                    None => Err("no such host".to_owned()),
                    Some(h) => match &h.client {
                        None => Err("not connected".to_owned()),
                        Some(c) => Ok((c.clone(), h.features.clone())),
                    },
                };
                let _ = reply.send(result);
            }
            Cmd::Hosts { reply } => {
                let _ = reply.send(self.host_list());
            }
            Cmd::Connect { host, reply } => self.connect(host, reply),
            Cmd::Disconnect { host, reply } => self.disconnect(host, reply),
            Cmd::Spawned {
                host,
                sid,
                meta,
                prompt,
                reply,
            } => self.spawned(host, sid, meta, prompt, reply),
            Cmd::Order { tab, sids, reply } => {
                let mut seen = BTreeSet::new();
                let sids: Vec<SessionId> = sids
                    .into_iter()
                    .filter(|s| seen.insert(*s))
                    .take(4096)
                    .collect();
                self.web.order = sids.clone();
                self.to_tabs_but(tab, &ev("order", json!({ "sids": sids })));
                self.save(Some(Box::new(move |r| {
                    let _ = reply.send(r);
                })));
            }
            Cmd::Layout { tab, layout, reply } => {
                if !layout.valid() {
                    let _ = reply.send(Err(
                        "bad request: layout: grid must be 1, 2 or 4, with as many panes and focus below grid"
                            .to_owned(),
                    ));
                    return;
                }
                self.web.layout = layout.clone();
                self.to_tabs_but(tab, &ev("layout", json!({ "layout": layout })));
                self.save(Some(Box::new(move |r| {
                    let _ = reply.send(r);
                })));
            }
            Cmd::Broadcast(v) => self.to_tabs(&v),
            Cmd::ClaudeUpdate {
                host,
                install,
                reply,
            } => {
                let _ = reply.send(self.start_claude(&host, install));
            }
            Cmd::ClaudeSkip { host, reply } => {
                if let Some(h) = self.hosts.get_mut(&host) {
                    let open = h.claude_offer.as_ref().is_some_and(|o| {
                        matches!(o["phase"].as_str(), Some("offer_install" | "offer_update"))
                    });
                    if open {
                        h.claude_offer = None;
                    }
                }
                let _ = reply.send(());
            }
            Cmd::SetProfiles {
                names,
                default,
                reply,
            } => {
                self.profiles = (names, default);
                let v = ev(
                    "profiles",
                    json!({"names": self.profiles.0, "default": self.profiles.1}),
                );
                self.to_tabs(&v);
                let _ = reply.send(());
            }
            Cmd::SetClaude { claude } => self.claude = claude,
            Cmd::Shutdown { reply } => self.shutdown(reply),
            Cmd::Connected {
                host,
                generation,
                client,
                sessions,
            } => {
                if self.current(&host, generation) {
                    self.connected(&host, generation, client, sessions);
                }
            }
            Cmd::ConnectFailed {
                host,
                generation,
                error,
                bootstrap,
            } => {
                if self.current(&host, generation) {
                    self.connect_failed(&host, error, bootstrap);
                }
            }
            Cmd::Incoming {
                host,
                generation,
                inc,
            } => {
                if self.current(&host, generation) {
                    self.incoming(&host, inc);
                }
            }
            Cmd::Retry { host, generation } => {
                let idle = self
                    .hosts
                    .get(&host)
                    .is_some_and(|h| h.client.is_none() && !h.connecting);
                if idle && self.current(&host, generation) {
                    let bootstrap = self.conns.bootstrap_failed(&host);
                    self.start_attempt(&host, generation, bootstrap);
                }
            }
            Cmd::Reconnect { host, generation } => {
                let connected = self.hosts.get(&host).is_some_and(|h| h.client.is_some());
                if connected && self.current(&host, generation) {
                    self.lost(&host);
                    let generation = self.conns.next_generation(&host);
                    self.start_attempt(&host, generation, false);
                }
            }
            Cmd::ClaudeDone {
                host,
                install,
                result,
            } => self.claude_done(&host, install, result),
        }
    }

    /// Whether a message of `generation` from `host`'s connection is current.
    fn current(&self, host: &str, generation: u64) -> bool {
        self.hosts.contains_key(host) && self.conns.current_generation(host) == generation
    }

    // ── tabs ─────────────────────────────────────────────────────────────

    fn open_tab(&mut self, tx: TabTx) -> Result<TabId, &'static str> {
        if self.tabs.len() >= MAX_TABS {
            return Err("too many tabs");
        }
        let tab = self.next_tab;
        self.next_tab += 1;
        // The snapshot and the replay are queued before the tab is
        // registered: no event can come before them.
        tx.send(Out::Text(self.snapshot().to_string()));
        for v in self.replay() {
            tx.send(Out::Text(v.to_string()));
        }
        self.tabs.insert(tab, tx);
        Ok(tab)
    }

    fn close_tab(&mut self, tab: TabId) {
        if tab == PROMPT_TAB || self.tabs.remove(&tab).is_none() {
            return;
        }
        let ids: Vec<SessionId> = self.sessions.keys().copied().collect();
        for sid in ids {
            let last = self
                .sessions
                .get_mut(&sid)
                .is_some_and(|v| v.subs.unsubscribe(tab));
            if last {
                self.send_detach(sid);
            }
        }
    }

    /// Cut tabs whose queue overflowed: forget them.
    fn drop_tabs(&mut self, cut: Vec<TabId>) {
        for tab in cut {
            tracing::info!(tab, "web: tab too slow, closed");
            self.close_tab(tab);
        }
    }

    fn to_tabs(&mut self, v: &Value) {
        let text = v.to_string();
        let cut: Vec<TabId> = self
            .tabs
            .iter()
            .filter(|(_, tx)| !tx.send(Out::Text(text.clone())))
            .map(|(t, _)| *t)
            .collect();
        self.drop_tabs(cut);
    }

    fn to_tabs_but(&mut self, but: TabId, v: &Value) {
        let text = v.to_string();
        let cut: Vec<TabId> = self
            .tabs
            .iter()
            .filter(|(t, _)| **t != but)
            .filter(|(_, tx)| !tx.send(Out::Text(text.clone())))
            .map(|(t, _)| *t)
            .collect();
        self.drop_tabs(cut);
    }

    /// `out` to every tab subscribed to `sid`.
    fn to_subs(&mut self, sid: SessionId, out: Out) {
        let Some(view) = self.sessions.get(&sid) else {
            return;
        };
        let cut: Vec<TabId> = view
            .subs
            .tabs()
            .filter(|t| *t != PROMPT_TAB)
            .filter(|t| self.tabs.get(t).is_some_and(|tx| !tx.send(out.clone())))
            .collect();
        self.drop_tabs(cut);
    }

    fn snapshot(&self) -> Value {
        let sessions: Vec<Value> = self.board().into_iter().map(dto::session).collect();
        json!({
            "t": "snapshot",
            "hosts": self.host_list(),
            "sessions": sessions,
            "profiles": {"names": self.profiles.0, "default": self.profiles.1},
            "layout": self.web.layout,
            "order": self.web.order,
        })
    }

    /// docs/web-protocol.md §3.1, after the snapshot.
    fn replay(&self) -> Vec<Value> {
        let mut out = Vec::new();
        for h in self.ordered_hosts() {
            if let Some(stats) = h.stats.filter(|_| h.client.is_some()) {
                out.push(host_stats(&h.name, stats));
            }
        }
        for view in self.board() {
            if !view.lens.is_empty() {
                out.extend(lens_events(view.info.id, view.lens_seq, &view.lens));
            }
        }
        for h in self.ordered_hosts() {
            if let Some(offer) = &h.claude_offer {
                out.push(offer.clone());
            }
        }
        out
    }

    /// The sessions in board order: `web-state.json`'s order, then the
    /// others oldest first.
    fn board(&self) -> Vec<&SessionView> {
        let rank: HashMap<SessionId, usize> = self
            .web
            .order
            .iter()
            .enumerate()
            .map(|(i, s)| (*s, i))
            .collect();
        let mut views: Vec<&SessionView> = self.sessions.values().collect();
        views.sort_by_key(|v| {
            (
                rank.get(&v.info.id).copied().unwrap_or(usize::MAX),
                v.info.created_at,
                v.info.id,
            )
        });
        views
    }

    fn ordered_hosts(&self) -> impl Iterator<Item = &HostState> {
        let local = self.hosts.get("local");
        local
            .into_iter()
            .chain(self.hosts.values().filter(|h| h.name != "local"))
    }

    fn host_list(&self) -> Vec<Value> {
        self.ordered_hosts().map(dto::host).collect()
    }

    fn send_host(&mut self, host: &str) {
        if let Some(h) = self.hosts.get(host) {
            let v = ev("host", json!({ "host": dto::host(h) }));
            self.to_tabs(&v);
        }
    }

    fn send_session(&mut self, sid: SessionId) {
        if let Some(view) = self.sessions.get(&sid) {
            let v = ev("session", json!({ "session": dto::session(view) }));
            self.to_tabs(&v);
        }
    }

    fn notice(&mut self, text: &str, level: &str, host: Option<&str>, sid: Option<SessionId>) {
        let v = ev(
            "notice",
            json!({"text": clean(text, 1000), "level": level, "host": host, "sid": sid}),
        );
        self.to_tabs(&v);
    }

    // ── sessions ─────────────────────────────────────────────────────────

    /// A session and its connected host's client.
    fn live(&self, sid: SessionId) -> Result<(Client, &SessionView), String> {
        let view = self
            .sessions
            .get(&sid)
            .ok_or_else(|| "no such session".to_owned())?;
        let client = self
            .hosts
            .get(&view.host)
            .and_then(|h| h.client.clone())
            .ok_or_else(|| "not connected".to_owned())?;
        Ok((client, view))
    }

    fn session_ref(&self, sid: SessionId) -> Result<SessionRef, String> {
        let (client, view) = self.live(sid)?;
        let features = self
            .hosts
            .get(&view.host)
            .map(|h| h.features.clone())
            .unwrap_or_default();
        let (proxy, first_party) = view.meta.clone().unwrap_or((None, false));
        Ok(SessionRef {
            host: view.host.clone(),
            client,
            features,
            info: view.info.clone(),
            proxy,
            first_party,
            proxy_known: view.meta.is_some(),
        })
    }

    fn attach(
        &mut self,
        tab: TabId,
        sid: SessionId,
        want: (u16, u16),
        reply: Reply<Result<(), String>>,
    ) {
        let client = match self.live(sid) {
            Ok((client, _)) => client,
            Err(e) => {
                let _ = reply.send(Err(e));
                return;
            }
        };
        if tab != PROMPT_TAB && !self.tabs.contains_key(&tab) {
            let _ = reply.send(Err("tab closed".to_owned()));
            return;
        }
        let Some(view) = self.sessions.get_mut(&sid) else {
            return;
        };
        let (rows, cols) = view.subs.subscribe(tab, want, view.size).size();
        let request = client.request(Msg::Attach {
            id: sid,
            rows,
            cols,
        });
        let tx = self.tx.clone();
        tokio::spawn(async move {
            match request.await {
                Ok(_) => {
                    let _ = reply.send(Ok(()));
                }
                Err(e) => {
                    let _ = reply.send(Err(daemon_error(e)));
                    let _ = tx.send(Cmd::AttachFailed { tab, sid }).await;
                }
            }
        });
    }

    fn detach(&mut self, tab: TabId, sid: SessionId) -> Result<(), String> {
        let view = self
            .sessions
            .get_mut(&sid)
            .ok_or_else(|| "no such session".to_owned())?;
        if view.subs.unsubscribe(tab) {
            self.send_detach(sid);
        }
        Ok(())
    }

    fn send_detach(&self, sid: SessionId) {
        if let Ok((client, _)) = self.live(sid) {
            let request = client.request(Msg::Detach { id: sid });
            tokio::spawn(async move {
                let _ = request.await;
            });
        }
    }

    /// `Resize`, or with subscribers a re-attach at the new size: the daemon
    /// resizes the PTY and every subscribed tab gets `attached` and a fresh
    /// snapshot at that size (a plain `Resize` would resync no tab: the hub
    /// is the daemon's one client for all of them).
    fn resize(
        &mut self,
        sid: SessionId,
        (rows, cols): (u16, u16),
        reply: Reply<Result<(), String>>,
    ) {
        let (client, view) = match self.live(sid) {
            Ok(x) => x,
            Err(e) => {
                let _ = reply.send(Err(e));
                return;
            }
        };
        let msg = if view.subs.is_empty() {
            Msg::Resize {
                id: sid,
                rows,
                cols,
            }
        } else {
            Msg::Attach {
                id: sid,
                rows,
                cols,
            }
        };
        let request = client.request(msg);
        tokio::spawn(async move {
            let _ = reply.send(request.await.map(|_| ()).map_err(daemon_error));
        });
    }

    /// The guard and the key, in one actor step: nothing can change the
    /// session's state or attention between the check and the send.
    fn approve(&mut self, sid: SessionId, decision: Decision, seq: u64) -> Result<(), String> {
        let (client, view) = self.live(sid)?;
        let has = self
            .hosts
            .get(&view.host)
            .is_some_and(|h| h.has("attention"));
        let key = input::approve_key(
            view.info.state,
            view.info.attention.as_ref(),
            has,
            decision,
            seq,
        )
        .map_err(str::to_owned)?;
        client.send_input(sid, key);
        tracing::info!(op = "session.approve", host = %view.host, %sid, "web op");
        Ok(())
    }

    fn spawned(
        &mut self,
        host: String,
        sid: SessionId,
        meta: SessionMeta,
        prompt: Option<(String, (u16, u16))>,
        reply: Reply<()>,
    ) {
        self.web.sessions.insert(sid, meta.clone());
        let prompt = prompt.map(|(text, size)| (PendingPrompt::new(text, Instant::now()), size));
        if self.sessions.get(&sid).is_some_and(|v| v.host == host) {
            self.apply_spawn(sid, meta, prompt);
            self.send_session(sid);
        } else {
            self.pending_spawns.insert(sid, (meta, prompt));
        }
        self.save(Some(Box::new(move |_| {
            let _ = reply.send(());
        })));
    }

    /// Give view `sid` what its spawn said: its proxy and opening prompt.
    fn apply_spawn(
        &mut self,
        sid: SessionId,
        meta: SessionMeta,
        prompt: Option<(PendingPrompt, (u16, u16))>,
    ) {
        let Some(view) = self.sessions.get_mut(&sid) else {
            return;
        };
        view.meta = Some((meta.proxy, meta.first_party));
        if let Some((mut prompt, size)) = prompt {
            prompt.on_state(view.info.state);
            view.prompt = Some(prompt);
            let (rows, cols) = view.subs.subscribe(PROMPT_TAB, size, view.size).size();
            if let Ok((client, _)) = self.live(sid) {
                let request = client.request(Msg::Attach {
                    id: sid,
                    rows,
                    cols,
                });
                tokio::spawn(async move {
                    let _ = request.await;
                });
            }
        }
    }

    fn on_event(&mut self, host: &str, sid: SessionId, event: SessionEvent) {
        let now = now_secs();
        if let SessionEvent::Created { info } = &event {
            if !self.sessions.contains_key(&sid) {
                if info.ephemeral {
                    return;
                }
                self.add_view(host, info.clone(), now);
                self.send_session(sid);
                return;
            }
        }
        let lens_host = self.hosts.get(host).is_some_and(|h| h.has("lens"));
        let Some(view) = self.sessions.get_mut(&sid).filter(|v| v.host == host) else {
            return;
        };
        let created = matches!(event, SessionEvent::Created { .. });
        let applied = apply_event(view, event, lens_host, now);
        if created {
            if let Some((meta, prompt)) = self.pending_spawns.remove(&sid) {
                self.apply_spawn(sid, meta, prompt);
            }
        }
        if applied.removed {
            self.remove_view(sid);
            return;
        }
        if applied.changed {
            self.send_session(sid);
        }
        if let Some((seq, items)) = applied.lens {
            for v in lens_events(sid, seq, &items) {
                self.to_tabs(&v);
            }
        }
        if let Some(text) = applied.notice {
            self.notice(&text, "info", Some(host), Some(sid));
        }
    }

    /// A new view for `info`, with what is known of its proxy and any
    /// pending spawn.
    fn add_view(&mut self, host: &str, info: SessionInfo, now: u64) {
        let sid = info.id;
        let mut view = SessionView::new(host, info, now);
        view.meta = state::known_proxy(&self.web, &self.tui_state, sid);
        self.sessions.insert(sid, view);
        if let Some((meta, prompt)) = self.pending_spawns.remove(&sid) {
            self.apply_spawn(sid, meta, prompt);
        }
    }

    fn remove_view(&mut self, sid: SessionId) {
        if self.sessions.remove(&sid).is_some() {
            if self.web.sessions.remove(&sid).is_some() {
                self.dirty = true;
            }
            self.to_tabs(&ev("session_removed", json!({ "sid": sid })));
        }
    }

    // ── hosts ────────────────────────────────────────────────────────────

    /// Start connecting `host` as attempt `generation`.
    fn start_attempt(&mut self, host: &str, generation: u64, bootstrap: bool) {
        if let Some(h) = self.hosts.get_mut(host) {
            h.connecting = true;
            h.generation = generation;
        }
        self.send_host(host);
        let (host, tx) = (host.to_owned(), self.tx.clone());
        tokio::spawn(async move {
            let cmd = match connect_once(&host, bootstrap).await {
                Ok((client, sessions)) => Cmd::Connected {
                    host,
                    generation,
                    client,
                    sessions,
                },
                Err((error, bootstrap)) => Cmd::ConnectFailed {
                    host,
                    generation,
                    error,
                    bootstrap,
                },
            };
            let _ = tx.send(cmd).await;
        });
    }

    fn connect(&mut self, host: String, reply: Reply<Result<Value, String>>) {
        if let Some(h) = self.hosts.get(&host).filter(|h| h.client.is_some()) {
            let _ = reply.send(Ok(json!({ "host": dto::host(h) })));
            return;
        }
        let h = self
            .hosts
            .entry(host.clone())
            .or_insert_with(|| HostState::new(&host));
        h.error = None;
        let busy = h.connecting;
        self.connect_waiters
            .entry(host.clone())
            .or_default()
            .push(reply);
        if !busy {
            self.conns.ensure_host(&host);
            let generation = self.conns.next_generation(&host);
            self.start_attempt(&host, generation, true);
        }
    }

    fn disconnect(&mut self, host: String, reply: Reply<Result<(), String>>) {
        if self.hosts.remove(&host).is_none() {
            let _ = reply.send(Err("no such host".to_owned()));
            return;
        }
        // Silence its connection and any attempt in flight.
        self.conns.next_generation(&host);
        self.conns.disconnect(&host);
        for waiter in self.connect_waiters.remove(&host).unwrap_or_default() {
            let _ = waiter.send(Err("disconnected".to_owned()));
        }
        let ids: Vec<SessionId> = self
            .sessions
            .values()
            .filter(|v| v.host == host)
            .map(|v| v.info.id)
            .collect();
        for sid in ids {
            self.sessions.remove(&sid);
            self.web.sessions.remove(&sid);
            self.to_tabs(&ev("session_removed", json!({ "sid": sid })));
        }
        self.to_tabs(&ev("host_removed", json!({ "name": host })));
        self.web.hosts.retain(|h| *h != host);
        let _ = self.events.send(HubEvent::HostDown { host });
        self.save(Some(Box::new(move |r| {
            let _ = reply.send(r);
        })));
    }

    fn connected(
        &mut self,
        host: &str,
        generation: u64,
        client: Client,
        sessions: Vec<SessionInfo>,
    ) {
        let now = now_secs();
        let welcome = client.welcome().clone();
        let features = welcome.features.clone();
        let has_claude = welcome.host.claude.is_some();
        if host == "local" {
            self.local_claude = welcome.host.claude.as_ref().map(|c| c.version.clone());
        }
        if let Some(rx) = client.take_incoming() {
            tokio::spawn(forward(host.to_owned(), generation, rx, self.tx.clone()));
        }
        self.conns.connected(host, client.clone());
        let Some(h) = self.hosts.get_mut(host) else {
            return;
        };
        h.outdated = client.daemon_outdated();
        h.client = Some(client);
        h.connecting = false;
        h.error = None;
        h.welcome = Some(welcome);
        h.features = features.clone();
        h.retry = true;
        tracing::info!(host, "web: connected");

        // The daemon's list replaces this host's sessions.
        let listed: HashMap<SessionId, SessionInfo> = sessions
            .into_iter()
            .filter(|s| !s.ephemeral)
            .map(|s| (s.id, s))
            .collect();
        // Sessions that ended while no web was running had no view to remove.
        if self.web.forget_unlisted(host, |sid| listed.contains_key(sid)) {
            self.dirty = true;
        }
        let gone: Vec<SessionId> = self
            .sessions
            .values()
            .filter(|v| v.host == host && !listed.contains_key(&v.info.id))
            .map(|v| v.info.id)
            .collect();
        for sid in gone {
            self.remove_view(sid);
        }
        let mut ids: Vec<SessionId> = Vec::new();
        for (sid, info) in listed {
            ids.push(sid);
            match self.sessions.get_mut(&sid) {
                Some(view) => {
                    if view.info.state != info.state {
                        view.state_since = now;
                    }
                    let old = std::mem::replace(&mut view.info, info);
                    if view.info.title.is_none() {
                        view.info.title = old.title;
                    }
                    // A restarted daemon counts lens lists from 1 again.
                    view.lens_seq = 0;
                }
                None => self.add_view(host, info, now),
            }
        }
        ids.sort();
        for sid in &ids {
            self.send_session(*sid);
        }
        self.send_host(host);
        // Sessions with subscribers are attached again.
        let client = self.hosts.get(host).and_then(|h| h.client.clone());
        if let Some(client) = client {
            for sid in &ids {
                let Some(view) = self.sessions.get(sid).filter(|v| !v.subs.is_empty()) else {
                    continue;
                };
                let (rows, cols) = view.size.or(view.subs.asked).unwrap_or(DEFAULT_SIZE);
                let request = client.request(Msg::Attach {
                    id: *sid,
                    rows,
                    cols,
                });
                tokio::spawn(async move {
                    let _ = request.await;
                });
            }
        }
        let _ = self.events.send(HubEvent::HostUp {
            host: host.to_owned(),
            features,
            has_claude,
        });

        // A remote host is remembered, and moved up in the MRU list.
        let waiters = self.connect_waiters.remove(host).unwrap_or_default();
        let dto = self.hosts.get(host).map(dto::host).unwrap_or(Value::Null);
        if host != "local" {
            let name = host.to_owned();
            tokio::task::spawn_blocking(move || crate::remote::hosts::touch(&name));
            if !self.web.hosts.iter().any(|h| h == host) {
                self.web.hosts.push(host.to_owned());
                self.dirty = true;
            }
        }
        if waiters.is_empty() {
            // Nothing waits; a new host is still saved soon.
        } else {
            self.save(Some(Box::new(move |_| {
                for w in waiters {
                    let _ = w.send(Ok(json!({ "host": dto })));
                }
            })));
        }
        self.check_claude(host);
    }

    fn connect_failed(&mut self, host: &str, error: String, bootstrap: bool) {
        let error = clean(&error, 600);
        tracing::info!(host, "web: connect failed");
        for waiter in self.connect_waiters.remove(host).unwrap_or_default() {
            let _ = waiter.send(Err(error.clone()));
        }
        let Some(h) = self.hosts.get_mut(host) else {
            return;
        };
        h.connecting = false;
        h.error = Some(error);
        if !h.retry {
            // A first `hosts.connect` that failed: not a host of this server.
            self.hosts.remove(host);
            self.conns.next_generation(host);
            self.to_tabs(&ev("host_removed", json!({ "name": host })));
            return;
        }
        self.send_host(host);
        self.schedule_retry(host, bootstrap);
    }

    fn schedule_retry(&mut self, host: &str, bootstrap: bool) {
        self.conns.set_bootstrap_failed(host, bootstrap);
        let (delay, generation) = self.conns.next_attempt(host);
        if let Some(h) = self.hosts.get_mut(host) {
            h.generation = generation;
        }
        let (host, tx) = (host.to_owned(), self.tx.clone());
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            let _ = tx.send(Cmd::Retry { host, generation }).await;
        });
    }

    /// `host`'s connection is gone: its sessions are `unknown` until it is
    /// back.
    fn lost(&mut self, host: &str) {
        self.conns.disconnect(host);
        if let Some(h) = self.hosts.get_mut(host) {
            h.client = None;
            h.connecting = false;
        }
        let now = now_secs();
        let ids: Vec<SessionId> = self
            .sessions
            .values_mut()
            .filter(|v| v.host == host)
            .map(|v| {
                if v.info.state != SessionState::Unknown {
                    v.state_since = now;
                }
                v.info.state = SessionState::Unknown;
                v.info.attention = None;
                v.info.id
            })
            .collect();
        self.send_host(host);
        for sid in ids {
            self.send_session(sid);
        }
        let _ = self.events.send(HubEvent::HostDown {
            host: host.to_owned(),
        });
        tracing::info!(host, "web: connection lost");
    }

    fn incoming(&mut self, host: &str, inc: Incoming) {
        match inc {
            Incoming::Data { id, bytes } => {
                let Some(view) = self.sessions.get_mut(&id).filter(|v| v.host == host) else {
                    return;
                };
                if let Some(p) = &mut view.prompt {
                    p.on_output(&bytes, Instant::now());
                }
                let mut frame = Vec::with_capacity(17 + bytes.len());
                frame.push(KIND_OUTPUT);
                frame.extend_from_slice(id.as_bytes());
                frame.extend_from_slice(&bytes);
                self.to_subs(id, Out::Binary(Bytes::from(frame)));
            }
            Incoming::Attached { id, rows, cols } => {
                let Some(view) = self.sessions.get_mut(&id).filter(|v| v.host == host) else {
                    return;
                };
                on_attached(view, rows, cols);
                let v = ev("attached", json!({"sid": id, "rows": rows, "cols": cols}));
                self.to_subs(id, Out::Text(v.to_string()));
            }
            Incoming::Event { id, event } => self.on_event(host, id, event),
            Incoming::HostStats {
                cpu_pct,
                mem_used,
                mem_total,
                ..
            } => {
                if let Some(h) = self.hosts.get_mut(host) {
                    h.stats = Some((cpu_pct, mem_used, mem_total));
                }
                self.to_tabs(&host_stats(host, (cpu_pct, mem_used, mem_total)));
            }
            Incoming::Disconnected => {
                self.lost(host);
                let bootstrap = self.conns.bootstrap_failed(host);
                self.schedule_retry(host, bootstrap);
            }
        }
    }

    /// Every outdated daemon that is idle now is replaced (by reconnecting:
    /// the connect path's freshness check replaces it).
    fn check_outdated(&mut self) {
        for h in self.hosts.values() {
            let Some(client) = h.client.clone().filter(|_| h.outdated) else {
                continue;
            };
            let (host, generation, tx) = (
                h.name.clone(),
                self.conns.current_generation(&h.name),
                self.tx.clone(),
            );
            tokio::spawn(async move {
                let Ok(Msg::Sessions { sessions }) = client.request(Msg::ListSessions).await else {
                    return;
                };
                if !freshness::busy(&sessions) {
                    let _ = tx.send(Cmd::Reconnect { host, generation }).await;
                }
            });
        }
    }

    // ── claude installs and updates ──────────────────────────────────────

    /// Once per host per run: a missing claude is installed as `[claude]
    /// install` says; a remote one older than ours is updated as
    /// `remote_check` says (unless the TUI's state.json skips that version).
    fn check_claude(&mut self, host: &str) {
        let local = self.local_claude.clone();
        let (install, remote_check) = (self.claude.install, self.claude.remote_check);
        let skipped = self.tui_state.claude_skipped.get(host).cloned();
        let Some(h) = self.hosts.get_mut(host).filter(|h| !h.offered) else {
            return;
        };
        let have = h
            .welcome
            .as_ref()
            .and_then(|w| w.host.claude.as_ref())
            .map(|c| c.version.clone());
        let (policy, want, install) = match &have {
            None => (install, None, true),
            Some(have) => {
                let Some(want) = local.filter(|_| host != "local") else {
                    return;
                };
                if remote_check == UpdatePolicy::Off || !needs_update(Some(have), &want) {
                    return;
                }
                (remote_check, Some(want), false)
            }
        };
        if policy == UpdatePolicy::Off {
            return;
        }
        h.offered = true;
        let skip = want
            .as_deref()
            .is_some_and(|w| skipped.as_deref().is_some_and(|s| short(s) == short(w)));
        match decide(policy, skip) {
            Policy::Nothing => {}
            Policy::Run => {
                let _ = self.start_claude(host, install);
            }
            Policy::Ask => {
                let phase = if install {
                    "offer_install"
                } else {
                    "offer_update"
                };
                let offer = claude_event(host, phase, have.as_deref(), want.as_deref(), None, None);
                h.claude_offer = Some(offer.clone());
                self.to_tabs(&offer);
            }
        }
    }

    fn start_claude(&mut self, host: &str, install: bool) -> Result<(), String> {
        let h = self
            .hosts
            .get_mut(host)
            .ok_or_else(|| "no such host".to_owned())?;
        let client = h.client.clone().ok_or_else(|| "not connected".to_owned())?;
        let running = h
            .claude_offer
            .as_ref()
            .is_some_and(|o| matches!(o["phase"].as_str(), Some("installing" | "updating")));
        if running {
            return Err(UPDATE_BUSY.to_owned());
        }
        let phase = if install { "installing" } else { "updating" };
        let v = claude_event(host, phase, None, None, None, None);
        h.claude_offer = Some(v.clone());
        self.to_tabs(&v);
        tracing::info!(op = "claude.update", host, install, "web op");
        let request = client.request(Msg::UpdateClaude { install });
        let (host, tx) = (host.to_owned(), self.tx.clone());
        tokio::spawn(async move {
            let result = request.await.map_err(|e| e.to_string());
            let _ = tx
                .send(Cmd::ClaudeDone {
                    host,
                    install,
                    result,
                })
                .await;
        });
        Ok(())
    }

    fn claude_done(&mut self, host: &str, install: bool, result: Result<Msg, String>) {
        let Some(h) = self.hosts.get_mut(host) else {
            return;
        };
        h.claude_offer = None;
        let v = match result {
            Ok(Msg::ClaudeUpdated {
                version: Some(version),
                ok: true,
                ..
            }) => {
                if let Some(w) = &mut h.welcome {
                    let path = w
                        .host
                        .claude
                        .as_ref()
                        .map_or_else(|| "claude".to_owned(), |c| c.path.clone());
                    w.host.claude = Some(ClaudeInfo {
                        path,
                        version: version.clone(),
                    });
                }
                if host == "local" {
                    self.local_claude = Some(version.clone());
                }
                if install {
                    let _ = self.events.send(HubEvent::ClaudeInstalled {
                        host: host.to_owned(),
                    });
                }
                let done =
                    claude_event(host, "done", None, None, Some(&clean(&version, 200)), None);
                self.send_host(host);
                done
            }
            Ok(Msg::ClaudeUpdated { tail: t, .. }) => {
                claude_event(host, "failed", None, None, None, Some(&tail(&t)))
            }
            Ok(_) => claude_event(host, "failed", None, None, None, Some("unexpected reply")),
            Err(e) => claude_event(host, "failed", None, None, None, Some(&tail(&e))),
        };
        self.to_tabs(&v);
    }

    // ── web-state.json ───────────────────────────────────────────────────

    /// Hand the state to the writer; `done` runs after the write.
    fn save(&mut self, done: Option<Done>) {
        self.dirty = false;
        self.last_save = Instant::now();
        let job = WriteJob {
            state: self.web.clone(),
            done: done.into_iter().collect(),
        };
        if let Err(mpsc::error::SendError(job)) = self.writer.send(job) {
            for d in job.done {
                d(Err("cannot save web-state.json".to_owned()));
            }
        }
    }

    fn on_tick(&mut self) {
        let now = Instant::now();
        let mut steps = Vec::new();
        for (sid, view) in &self.sessions {
            if let Some(p) = &view.prompt {
                match p.step(now) {
                    PromptStep::Wait => {}
                    step => steps.push((*sid, step)),
                }
            }
        }
        for (sid, step) in steps {
            self.finish_prompt(sid, step);
        }
        if self.dirty && now.duration_since(self.last_save) >= SAVE_EVERY {
            self.save(None);
        }
    }

    /// Paste the opening prompt, or give up on it.
    fn finish_prompt(&mut self, sid: SessionId, step: PromptStep) {
        let Some(prompt) = self.sessions.get_mut(&sid).and_then(|v| v.prompt.take()) else {
            return;
        };
        let host = self
            .sessions
            .get(&sid)
            .map(|v| v.host.clone())
            .unwrap_or_default();
        match step {
            PromptStep::Paste => match (self.live(sid), input::paste_bytes(&prompt.text, true)) {
                (Ok((client, _)), Ok(bytes)) => client.send_input(sid, &bytes),
                _ => self.notice(
                    "opening prompt not sent: the session is not connected",
                    "warn",
                    Some(&host),
                    Some(sid),
                ),
            },
            _ => self.notice(
                "opening prompt not sent: the session did not get ready in 60 s",
                "warn",
                Some(&host),
                Some(sid),
            ),
        }
        let last = self
            .sessions
            .get_mut(&sid)
            .is_some_and(|v| v.subs.unsubscribe(PROMPT_TAB));
        if last {
            self.send_detach(sid);
        }
        self.send_session(sid);
    }

    fn shutdown(&mut self, reply: Reply<()>) {
        for tx in self.tabs.values() {
            tx.send(Out::Close(1001, "server shutting down"));
        }
        self.tabs.clear();
        self.save(Some(Box::new(move |_| {
            let _ = reply.send(());
        })));
    }
}

fn host_stats(host: &str, (cpu, mem_used, mem_total): (f32, u64, u64)) -> Value {
    ev(
        "host_stats",
        json!({"host": host, "cpu": cpu, "mem_used": mem_used, "mem_total": mem_total}),
    )
}

/// The `lens` event of a list and its `latex` twin (same seq).
fn lens_events(sid: SessionId, seq: u64, items: &[LensItem]) -> [Value; 2] {
    let formulas: Vec<&str> = items
        .iter()
        .filter(|i| i.tag == "latex")
        .map(|i| i.text.as_str())
        .collect();
    [
        ev("lens", json!({"sid": sid, "seq": seq, "items": items})),
        ev(
            "latex",
            json!({"sid": sid, "seq": seq, "formulas": formulas}),
        ),
    ]
}

fn claude_event(
    host: &str,
    phase: &str,
    have: Option<&str>,
    want: Option<&str>,
    version: Option<&str>,
    tail: Option<&str>,
) -> Value {
    ev(
        "claude",
        json!({"host": host, "phase": phase, "have": have, "want": want,
            "version": version, "tail": tail}),
    )
}

/// Connect `host` and list its sessions. The error says whether the
/// bootstrap failed (the next attempt runs it again).
async fn connect_once(
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

/// Forward a connection's unsolicited messages to the actor, tagged.
async fn forward(
    host: String,
    generation: u64,
    mut rx: mpsc::Receiver<Incoming>,
    tx: mpsc::Sender<Cmd>,
) {
    while let Some(inc) = rx.recv().await {
        let end = inc == Incoming::Disconnected;
        let cmd = Cmd::Incoming {
            host: host.clone(),
            generation,
            inc,
        };
        if tx.send(cmd).await.is_err() || end {
            break;
        }
    }
}

struct WriteJob {
    state: WebState,
    done: Vec<Done>,
}

/// The one writer of web-state.json: jobs in order, the newest of a burst
/// written once, each job's `done` run after its write.
async fn write_state(path: PathBuf, mut jobs: mpsc::UnboundedReceiver<WriteJob>) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{Attention, SessionKind};
    use uuid::Uuid;

    fn info(state: SessionState) -> SessionInfo {
        SessionInfo {
            id: Uuid::new_v4(),
            cwd: "/home/u/proj".into(),
            name: None,
            state,
            claude_session_id: None,
            title: None,
            pid: Some(42),
            created_at: 1,
            branch: None,
            model: None,
            context_tokens: None,
            kind: SessionKind::Claude,
            ephemeral: false,
            attention: None,
        }
    }

    fn view(state: SessionState) -> SessionView {
        SessionView::new("local", info(state), 1)
    }

    fn attention(seq: u64, kind: &str) -> Attention {
        Attention {
            seq,
            kind: kind.into(),
            message: "m".into(),
            tool: None,
            detail: None,
        }
    }

    fn item(tag: &str, text: &str) -> LensItem {
        LensItem {
            tag: tag.into(),
            text: text.into(),
        }
    }

    #[test]
    fn meta_none_is_no_news() {
        let mut v = view(SessionState::Idle);
        let git = GitStatus {
            head: "main".into(),
            ..GitStatus::default()
        };
        let full = SessionEvent::Meta {
            branch: Some("main".into()),
            model: Some("opus".into()),
            context_tokens: Some(10),
            git: Some(git.clone()),
            output_tokens: Some(5),
            turns: Some(2),
        };
        assert!(apply_event(&mut v, full, true, 2).changed);
        let only_model = SessionEvent::Meta {
            branch: None,
            model: Some("sonnet".into()),
            context_tokens: None,
            git: None,
            output_tokens: None,
            turns: None,
        };
        assert!(apply_event(&mut v, only_model, true, 3).changed);
        assert_eq!(v.info.model.as_deref(), Some("sonnet"));
        assert_eq!(v.info.branch.as_deref(), Some("main"));
        assert_eq!(v.info.context_tokens, Some(10));
        assert_eq!(v.git, Some(git));
        assert_eq!((v.output_tokens, v.turns), (Some(5), Some(2)));
    }

    #[test]
    fn lens_only_from_lens_hosts_latex_only_from_others() {
        let latex = || SessionEvent::Latex {
            seq: 1,
            formulas: vec!["x^2".into()],
        };
        let lens = || SessionEvent::Lens {
            seq: 1,
            items: vec![item("latex", "y"), item("chem", "H2O")],
        };
        let mut v = view(SessionState::Idle);
        assert_eq!(apply_event(&mut v, latex(), true, 1).lens, None);
        let got = apply_event(&mut v, lens(), true, 1).lens;
        assert_eq!(
            got,
            Some((1, vec![item("latex", "y"), item("chem", "H2O")]))
        );
        let mut v = view(SessionState::Idle);
        assert_eq!(apply_event(&mut v, lens(), false, 1).lens, None);
        let got = apply_event(&mut v, latex(), false, 1).lens;
        assert_eq!(got, Some((1, vec![item("latex", "x^2")])));
        assert_eq!(v.lens, vec![item("latex", "x^2")]);
        let events = lens_events(v.info.id, 1, &[item("latex", "a"), item("chem", "b")]);
        assert_eq!(events[0]["ev"], "lens");
        assert_eq!(events[1]["formulas"], json!(["a"]));
        assert_eq!(events[1]["seq"], 1);
    }

    #[test]
    fn lower_seq_lists_are_dropped_until_reattached() {
        let lens = |seq| SessionEvent::Lens {
            seq,
            items: vec![item("latex", &seq.to_string())],
        };
        let mut v = view(SessionState::Idle);
        assert!(apply_event(&mut v, lens(5), true, 1).lens.is_some());
        assert_eq!(apply_event(&mut v, lens(3), true, 1).lens, None);
        assert_eq!(v.lens_seq, 5);
        on_attached(&mut v, 24, 80);
        assert_eq!(v.size, Some((24, 80)));
        assert_eq!(
            apply_event(&mut v, lens(1), true, 1).lens.map(|l| l.0),
            Some(1)
        );
        assert_eq!(v.lens, vec![item("latex", "1")]);
    }

    #[test]
    fn leaving_attention_states_clears_the_attention() {
        let mut v = view(SessionState::Working);
        let state = |state| SessionEvent::State { state };
        apply_event(&mut v, state(SessionState::NeedsApproval), true, 2);
        let a = attention(1, "permission_prompt");
        let attend = SessionEvent::Attention {
            attention: a.clone(),
        };
        assert!(apply_event(&mut v, attend, true, 2).changed);
        apply_event(&mut v, state(SessionState::NeedsInput), true, 3);
        assert_eq!(v.info.attention, Some(a));
        assert_eq!(v.state_since, 3);
        apply_event(&mut v, state(SessionState::Working), true, 4);
        assert_eq!(v.info.attention, None);
        assert_eq!(dto::session(&v)["attention"], Value::Null);
    }

    #[test]
    fn created_and_exited_drop_the_attention() {
        let mut v = view(SessionState::NeedsApproval);
        v.info.attention = Some(attention(3, "permission_prompt"));
        v.lens_seq = 9;
        let mut respawned = info(SessionState::Starting);
        respawned.id = v.info.id;
        v.subs.subscribe(7, (24, 80), None);
        let applied = apply_event(&mut v, SessionEvent::Created { info: respawned }, true, 5);
        assert!(applied.changed && !applied.removed);
        assert_eq!(v.info.attention, None);
        assert_eq!(v.lens_seq, 0);
        assert_eq!(
            v.subs.tabs().collect::<Vec<_>>(),
            vec![7],
            "subscribers kept"
        );
        v.info.state = SessionState::NeedsInput;
        v.info.attention = Some(attention(4, "idle_prompt"));
        apply_event(&mut v, SessionEvent::Exited { code: Some(0) }, true, 6);
        assert_eq!(v.info.state, SessionState::Exited);
        assert_eq!((v.info.pid, v.info.attention.clone()), (None, None));
        assert!(apply_event(&mut v, SessionEvent::Removed, true, 7).removed);
    }

    #[test]
    fn ephemeral_sessions_are_never_listed() {
        let (tx, _rx) = mpsc::channel(8);
        let (events, _) = broadcast::channel(8);
        let (writer, _jobs) = mpsc::unbounded_channel();
        let mut hub = Hub {
            claude: Default::default(),
            tui_state: Default::default(),
            profiles: (Vec::new(), None),
            web: WebState::default(),
            dirty: false,
            last_save: Instant::now(),
            hosts: BTreeMap::new(),
            conns: Connections {
                map: HashMap::new(),
            },
            sessions: HashMap::new(),
            pending_spawns: HashMap::new(),
            tabs: BTreeMap::new(),
            next_tab: 1,
            connect_waiters: HashMap::new(),
            tx,
            events,
            writer,
            local_claude: None,
        };
        hub.hosts.insert("local".into(), HostState::new("local"));
        let mut eph = info(SessionState::Working);
        eph.ephemeral = true;
        let id = eph.id;
        hub.on_event("local", id, SessionEvent::Created { info: eph });
        assert!(hub.sessions.is_empty());
        let (tab_tx, mut rx) = TabTx::new();
        hub.open_tab(tab_tx).unwrap();
        let snapshot: Value = match rx.try_recv().unwrap() {
            Out::Text(t) => serde_json::from_str(&t).unwrap(),
            other => panic!("{other:?}"),
        };
        assert_eq!(snapshot["sessions"], json!([]));
        let normal = info(SessionState::Working);
        let nid = normal.id;
        hub.on_event("local", nid, SessionEvent::Created { info: normal });
        assert_eq!(hub.sessions.len(), 1);
        let ev: Value = match rx.try_recv().unwrap() {
            Out::Text(t) => serde_json::from_str(&t).unwrap(),
            other => panic!("{other:?}"),
        };
        assert_eq!(ev["session"]["id"], json!(nid));
        assert_eq!(ev["session"]["ephemeral"], false);
    }

    #[test]
    fn attach_is_refcounted_across_tabs() {
        let mut subs = Subs::default();
        assert_eq!(
            subs.subscribe(1, (24, 80), None),
            AttachStep::Attach { rows: 24, cols: 80 }
        );
        assert_eq!(
            subs.subscribe(2, (40, 100), Some((24, 80))),
            AttachStep::Reattach { rows: 24, cols: 80 }
        );
        assert!(!subs.unsubscribe(2));
        assert!(subs.unsubscribe(1));
        assert!(!subs.unsubscribe(3));
    }

    #[test]
    fn a_tab_past_8_mib_is_cut() {
        let mib = || Out::Binary(Bytes::from(vec![0u8; 1 << 20]));
        let (tx, mut rx) = TabTx::new();
        for i in 0..8 {
            assert!(tx.send(mib()), "send {i}");
        }
        assert!(!tx.send(mib()), "the 9th MiB cuts the tab");
        assert!(!tx.send(Out::Text("late".into())), "dropped after the cut");
        tx.sent(4 << 20);
        assert!(!tx.send(Out::Text("still cut".into())));
        let mut items = Vec::new();
        while let Ok(o) = rx.try_recv() {
            items.push(o);
        }
        assert_eq!(items.len(), 9);
        assert!(matches!(items.last(), Some(Out::Close(1013, _))));

        // An uncut tab gets room back as its writer drains it.
        let (tx, _rx) = TabTx::new();
        for _ in 0..8 {
            assert!(tx.send(mib()));
        }
        tx.sent(1 << 20);
        assert!(tx.send(mib()));
    }

    #[test]
    fn opening_prompt_waits_for_input_bracketed_paste_and_quiet() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let mut p = PendingPrompt::new("first task".into(), t0);
        p.on_output(b"banner\r\n", ms(10));
        assert_eq!(p.step(ms(600)), PromptStep::Wait, "before needs_input");
        p.on_state(SessionState::NeedsInput);
        assert_eq!(p.step(ms(600)), PromptStep::Wait, "before ESC[?2004h");
        p.on_output(b"xx\x1b[?20", ms(700));
        p.on_output(b"04h", ms(800));
        assert_eq!(p.step(ms(1000)), PromptStep::Wait, "not quiet yet");
        assert_eq!(p.step(ms(1300)), PromptStep::Paste);

        let mut q = PendingPrompt::new("other".into(), t0);
        q.on_state(SessionState::NeedsInput);
        assert_eq!(q.step(ms(59_000)), PromptStep::Wait);
        assert_eq!(q.step(t0 + PROMPT_DEADLINE), PromptStep::GiveUp);
    }
}
