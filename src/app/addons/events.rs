//! The runtime's tasks and its queue to the front-end: the event task
//! (addons' `on_event`, in order, never under a runtime lock), the tick, the
//! config poll, and the [`Outbox`] behind [`RuntimeRx`].

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use tokio::sync::{mpsc, Notify};

use super::{ctx::Ctx, Inner, RuntimeOut};
use crate::addon::{AddonCtx, AddonEvent};

/// Notices and offers waiting for the front-end.
const OUT_QUEUE: usize = 256;

/// The runtime's side of [`RuntimeRx`].
#[derive(Default)]
pub(crate) struct Outbox {
    state: Mutex<OutState>,
    notify: Notify,
}

#[derive(Default)]
struct OutState {
    queue: VecDeque<RuntimeOut>,
    /// Addons whose `Changed` is due, in the order they changed.
    changed: Vec<&'static str>,
    closed: bool,
}

impl Outbox {
    fn state(&self) -> MutexGuard<'_, OutState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Queue a notice or an offer; a full queue drops its oldest.
    pub(super) fn push(&self, out: RuntimeOut) {
        let mut state = self.state();
        if state.closed {
            return;
        }
        if state.queue.len() >= OUT_QUEUE {
            if let Some(old) = state.queue.pop_front() {
                tracing::warn!(?old, "addon output dropped: the front-end is behind");
            }
        }
        state.queue.push_back(out);
        drop(state);
        self.notify.notify_one();
    }

    /// `addon` changed: one `Changed` until the front-end takes it.
    pub(super) fn changed(&self, addon: &'static str) {
        let mut state = self.state();
        if state.closed || state.changed.contains(&addon) {
            return;
        }
        state.changed.push(addon);
        drop(state);
        self.notify.notify_one();
    }

    pub(super) fn close(&self) {
        self.state().closed = true;
        self.notify.notify_one();
    }

    fn take(&self) -> Option<Option<RuntimeOut>> {
        let mut state = self.state();
        if let Some(out) = state.queue.pop_front() {
            return Some(Some(out));
        }
        if !state.changed.is_empty() {
            let addon = state.changed.remove(0);
            return Some(Some(RuntimeOut::Changed { addon }));
        }
        state.closed.then_some(None)
    }
}

/// What the runtime tells its front-end, received like an
/// `mpsc::Receiver`: notices and offers in order, then the addons whose
/// data changed (coalesced). `None` once the runtime stopped and all was
/// received.
pub struct RuntimeRx {
    outbox: Arc<Outbox>,
}

impl RuntimeRx {
    pub(super) fn new(outbox: Arc<Outbox>) -> RuntimeRx {
        RuntimeRx { outbox }
    }

    /// The next message, waiting for one.
    pub async fn recv(&mut self) -> Option<RuntimeOut> {
        loop {
            if let Some(out) = self.outbox.take() {
                return out;
            }
            // A notify before this wait left a permit: no wakeup is lost.
            self.outbox.notify.notified().await;
        }
    }
}

/// Start the event task, the tick and (with `watch_config`) the config poll.
pub(super) fn start(inner: &Arc<Inner>, rx: mpsc::Receiver<AddonEvent>) {
    let weak = Arc::downgrade(inner);
    let mut handles = vec![inner.runtime.spawn(deliver_all(weak.clone(), rx))];
    let (events, every) = (inner.events.clone(), inner.limits.tick);
    handles.push(inner.runtime.spawn(async move {
        let mut tick = tokio::time::interval_at(tokio::time::Instant::now() + every, every);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            // A busy event task skips a tick; it is no news.
            let _ = events.try_send(AddonEvent::Tick);
        }
    }));
    if inner.opts.watch_config {
        handles.push(inner.runtime.spawn(watch(weak)));
    }
    inner.tasks().background.extend(handles);
}

/// Read config.toml, deliver `Started`, then every event in order.
async fn deliver_all(inner: Weak<Inner>, mut rx: mpsc::Receiver<AddonEvent>) {
    let Some(path) = inner.upgrade().map(|i| i.opts.config_path.clone()) else {
        return;
    };
    let Some((gen, pause)) = inner
        .upgrade()
        .map(|i| (i.state().reload.begin(), i.limits.reload_pause))
    else {
        return;
    };
    let config = super::reload::load(path, pause).await.unwrap_or_default();
    let ctxs: Vec<(String, Arc<dyn AddonCtx>)> = {
        let Some(strong) = inner.upgrade() else {
            return;
        };
        let mut state = strong.state();
        if state.reload.accept(gen) {
            state.config = config;
        }
        drop(state);
        strong
            .addons
            .iter()
            .map(|a| {
                let ctx: Arc<dyn AddonCtx> = Arc::new(Ctx::new(&strong, a.name()));
                (a.name().to_owned(), ctx)
            })
            .collect()
    };
    let mut next = Some(AddonEvent::Started);
    while let Some(event) = match next.take() {
        Some(event) => Some(event),
        None => rx.recv().await,
    } {
        let Some(strong) = inner.upgrade() else {
            return;
        };
        for (name, ctx) in &ctxs {
            if let Some(addon) = strong.addons.get(name) {
                addon.on_event(&event, ctx);
            }
        }
    }
}

/// Poll config.toml's mtime; a change re-reads it (`config_changed`).
async fn watch(inner: Weak<Inner>) {
    let Some((path, every)) = inner
        .upgrade()
        .map(|i| (i.opts.config_path.clone(), i.limits.watch))
    else {
        return;
    };
    let mtime = |path: PathBuf| async move {
        tokio::fs::metadata(&path)
            .await
            .and_then(|m| m.modified())
            .ok()
    };
    let mut last = mtime(path.clone()).await;
    let mut tick = tokio::time::interval_at(tokio::time::Instant::now() + every, every);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        let now = mtime(path.clone()).await;
        if now == last {
            continue;
        }
        last = now;
        match inner.upgrade() {
            Some(strong) => super::AddonRuntime { inner: strong }.config_changed(),
            None => return,
        }
    }
}
