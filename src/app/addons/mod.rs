//! The addon runtime: the one implementation of [`AddonCtx`], shared by the
//! console TUI and the web hub (docs/addons.md, "Execution model"). A
//! front-end builds it with [`AddonRuntime::start`] and feeds it what it
//! knows (hosts, sessions, focus, config changes); the runtime delivers the
//! addons' events in order on one task, runs their tasks under bounds, sends
//! their daemon requests and edits their config sections, and tells the
//! front-end what to show through a [`RuntimeRx`].

mod ctx;
mod events;
mod reload;
mod rows;
#[cfg(test)]
pub(crate) mod test_support;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::{JoinHandle, JoinSet};

use crate::addon::{AddonCtx, AddonEvent, Addons, Notice, SessionSnap};
use crate::client::Client;
use crate::proto::SessionId;

use events::Outbox;
pub use events::RuntimeRx;

/// How a front-end runs the runtime.
#[derive(Debug, Clone)]
pub struct RuntimeOptions {
    /// The `config.toml` the addons' sections are read from and edited in.
    pub config_path: PathBuf,
    /// Poll the file every 5 s and re-read it on a change (D15); off while
    /// the front-end watches it itself.
    pub watch_config: bool,
}

/// What the runtime tells its front-end. Notices and offers wait in a
/// queue of 256 (a full one drops its oldest with a warning); `Changed` is
/// one flag per addon, coalesced until the front-end receives it (F13).
#[derive(Debug, Clone, PartialEq)]
pub enum RuntimeOut {
    /// A notice for the member, sanitised and capped.
    Notice { addon: &'static str, notice: Notice },
    /// The addon's segments, rows, options or open panel changed.
    Changed { addon: &'static str },
    /// Show the addon's checklist when idle (D8).
    Offer {
        addon: &'static str,
        checklist: String,
    },
    /// config.toml was re-read (the front-end re-reads its own
    /// tables: `[extensions]`, `[claude]`).
    ConfigChanged,
    /// The addons have started ([`AddonRuntime::started`]): their rows are
    /// what they read at their start, so an `[extensions]` key that matches
    /// no extension now is worth a notice.
    Started,
}

/// The bounds of the runtime (D13); tests shorten them.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    /// Live `ctx.spawn` tasks per addon.
    pub tasks: usize,
    /// One `ctx.spawn` task's run.
    pub task_timeout: Duration,
    /// One `ctx.request`.
    pub request_timeout: Duration,
    /// The `Tick` event's period.
    pub tick: Duration,
    /// The config file's poll period (with `watch_config`).
    pub watch: Duration,
    /// A pause after each config read before its result is applied; zero
    /// but in tests of the reload's ordering.
    pub reload_pause: Duration,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            tasks: 64,
            task_timeout: Duration::from_secs(300),
            request_timeout: Duration::from_secs(170),
            tick: Duration::from_secs(1),
            watch: Duration::from_secs(5),
            reload_pause: Duration::ZERO,
        }
    }
}

/// Events waiting for the event task; a full queue drops the newest with a
/// warning.
const EVENT_QUEUE: usize = 1024;

/// The runtime: a cheap handle. The last one dropped stops it, as
/// [`AddonRuntime::shutdown`] does.
#[derive(Clone)]
pub struct AddonRuntime {
    inner: Arc<Inner>,
}

/// A host as the front-end reported it.
struct Host {
    client: Option<Client>,
    features: Vec<String>,
}

/// What the runtime knows; never locked across an await or an addon call.
#[derive(Default)]
struct State {
    hosts: BTreeMap<String, Host>,
    sessions: Vec<SessionSnap>,
    /// config.toml as last read (or edited by an addon).
    config: toml::Table,
    /// The config re-reads (single flight, coalesced, generation-checked).
    reload: reload::Reload,
    /// The names the front-end's own extensions hold.
    core_extensions: Vec<String>,
}

/// Every task the runtime owns, aborted at shutdown.
#[derive(Default)]
struct Tasks {
    stopped: bool,
    /// Each addon's `ctx.spawn` tasks.
    addons: HashMap<&'static str, JoinSet<()>>,
    /// The event, tick and watch tasks.
    background: Vec<JoinHandle<()>>,
    /// Config re-reads.
    internal: JoinSet<()>,
}

pub(crate) struct Inner {
    addons: Arc<Addons>,
    opts: RuntimeOptions,
    limits: Limits,
    runtime: tokio::runtime::Handle,
    state: Mutex<State>,
    tasks: Mutex<Tasks>,
    outbox: Arc<Outbox>,
    events: mpsc::Sender<AddonEvent>,
    /// Serialises this runtime's config edits (the file's own lock is
    /// process-wide, in `config_edit`).
    edit: tokio::sync::Mutex<()>,
    /// The addons have started ([`AddonRuntime::started`]).
    started: tokio::sync::watch::Sender<bool>,
}

impl AddonRuntime {
    /// Start the runtime for `addons` on the current tokio runtime: the event
    /// task (it reads config.toml, then delivers `Started`, then every event
    /// in order), the 1 s `Tick` and, with `watch_config`, the config poll.
    pub fn start(addons: Arc<Addons>, opts: RuntimeOptions) -> (AddonRuntime, RuntimeRx) {
        AddonRuntime::start_with(addons, opts, Limits::default())
    }

    pub(crate) fn start_with(
        addons: Arc<Addons>,
        opts: RuntimeOptions,
        limits: Limits,
    ) -> (AddonRuntime, RuntimeRx) {
        let (events, rx) = mpsc::channel(EVENT_QUEUE);
        let outbox = Arc::new(Outbox::default());
        let inner = Arc::new(Inner {
            addons,
            opts,
            limits,
            runtime: tokio::runtime::Handle::current(),
            state: Mutex::default(),
            tasks: Mutex::default(),
            outbox: Arc::clone(&outbox),
            events,
            edit: tokio::sync::Mutex::new(()),
            started: tokio::sync::watch::channel(false).0,
        });
        events::start(&inner, rx);
        (AddonRuntime { inner }, RuntimeRx::new(outbox))
    }

    /// Once the addons have started: `Started` delivered and every task an
    /// addon spawned while handling it ended (an addon reads its state off
    /// the executor there; a launch resolves after it). Pending until then,
    /// and forever after a shutdown before it: callers bound the wait.
    pub async fn started(&self) {
        let mut started = self.inner.started.subscribe();
        let _ = started.wait_for(|s| *s).await;
    }

    /// The addons this runtime runs.
    pub fn addons(&self) -> &Arc<Addons> {
        &self.inner.addons
    }

    /// Addon `addon`'s context (D2).
    pub fn ctx(&self, addon: &'static str) -> Arc<dyn AddonCtx> {
        Arc::new(ctx::Ctx::new(&self.inner, addon))
    }

    /// `host`'s daemon connected through `client`: its features come from
    /// the `Welcome`.
    pub fn host_up(&self, host: &str, client: Client) {
        let welcome = client.welcome();
        let features = welcome.features.clone();
        let entry = Host {
            features: features.clone(),
            client: Some(client),
        };
        self.inner.state().hosts.insert(host.to_owned(), entry);
        self.inner.event(AddonEvent::HostUp {
            host: host.to_owned(),
            features,
        });
    }

    /// `host`'s daemon went away.
    pub fn host_down(&self, host: &str) {
        let known = match self.inner.state().hosts.get_mut(host) {
            Some(h) => h.client.take().is_some(),
            None => false,
        };
        if known {
            self.inner.event(AddonEvent::HostDown {
                host: host.to_owned(),
            });
        }
    }

    /// claude was just installed on `host`.
    pub fn claude_installed(&self, host: &str) {
        self.inner.event(AddonEvent::ClaudeInstalled {
            host: host.to_owned(),
        });
    }

    /// The front-end's whole session list (after a (re)connect, or any
    /// change it noticed): `SessionAdded`, `SessionRemoved` and
    /// `SessionChanged` (conversation or focus) for what differs.
    pub fn sessions_reset(&self, sessions: Vec<SessionSnap>) {
        let events = {
            let mut state = self.inner.state();
            let events = diff(&state.sessions, &sessions);
            state.sessions = sessions;
            events
        };
        for event in events {
            self.inner.event(event);
        }
    }

    /// A session appeared.
    pub fn session_added(&self, s: SessionSnap) {
        let mut list = self.inner.state().sessions.clone();
        list.retain(|o| o.sid != s.sid);
        list.push(s);
        self.sessions_reset(list);
    }

    /// A session's snapshot changed: `SessionChanged` when its conversation
    /// or focus did.
    pub fn session_changed(&self, s: SessionSnap) {
        let mut list = self.inner.state().sessions.clone();
        match list.iter_mut().find(|o| o.sid == s.sid) {
            Some(o) => *o = s,
            None => list.push(s),
        }
        self.sessions_reset(list);
    }

    /// A session went away.
    pub fn session_removed(&self, sid: SessionId) {
        let mut list = self.inner.state().sessions.clone();
        list.retain(|o| o.sid != sid);
        self.sessions_reset(list);
    }

    /// The focused sessions (the TUI's active one, the web's attached ones).
    pub fn set_focused(&self, sids: &[SessionId]) {
        let mut list = self.inner.state().sessions.clone();
        for s in &mut list {
            s.focused = sids.contains(&s.sid);
        }
        self.sessions_reset(list);
    }

    /// The names the front-end's own extensions hold (built-in and
    /// external), for `AddonCtx::core_extensions`.
    pub fn set_core_extensions(&self, names: Vec<String>) {
        self.inner.state().core_extensions = names;
    }

    /// Re-read config.toml (off the executor) and send `ConfigChanged`.
    /// One read runs at a time; calls meanwhile coalesce into one more.
    pub fn config_changed(&self) {
        reload::request(&self.inner);
    }

    /// Abort every addon task and the event, tick and watch tasks; the
    /// front-end's [`RuntimeRx`] ends once drained. Idempotent.
    pub fn shutdown(&self) {
        self.inner.stop();
    }
}

impl Inner {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn tasks(&self) -> MutexGuard<'_, Tasks> {
        self.tasks.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Queue `event` for every addon; a full queue drops it with a warning.
    fn event(&self, event: AddonEvent) {
        if let Err(mpsc::error::TrySendError::Full(event)) = self.events.try_send(event) {
            tracing::warn!(?event, "addon event dropped: the queue is full");
        }
    }

    /// A runtime task of its own (a config re-read), aborted at shutdown.
    fn spawn_internal(&self, task: impl std::future::Future<Output = ()> + Send + 'static) {
        let mut tasks = self.tasks();
        if tasks.stopped {
            return;
        }
        while tasks.internal.try_join_next().is_some() {}
        tasks.internal.spawn_on(task, &self.runtime);
    }

    fn stop(&self) {
        let mut tasks = self.tasks();
        tasks.stopped = true;
        for (_, set) in tasks.addons.iter_mut() {
            set.abort_all();
        }
        tasks.internal.abort_all();
        for handle in tasks.background.drain(..) {
            handle.abort();
        }
        drop(tasks);
        self.outbox.close();
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The events that take `old` to `new`.
fn diff(old: &[SessionSnap], new: &[SessionSnap]) -> Vec<AddonEvent> {
    let mut events = Vec::new();
    for s in old {
        if !new.iter().any(|n| n.sid == s.sid) {
            events.push(AddonEvent::SessionRemoved { sid: s.sid });
        }
    }
    for s in new {
        let conversation = s.conversation.clone();
        match old.iter().find(|o| o.sid == s.sid) {
            None => events.push(AddonEvent::SessionAdded {
                sid: s.sid,
                conversation,
            }),
            Some(o) if o.conversation != s.conversation || o.focused != s.focused => {
                events.push(AddonEvent::SessionChanged {
                    sid: s.sid,
                    conversation,
                })
            }
            Some(_) => {}
        }
    }
    events
}
