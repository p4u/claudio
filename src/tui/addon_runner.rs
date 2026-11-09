//! The TUI's edge of the addon runtime (`app::addons`): it is built in
//! `main` and fed from the event loop: host connections and losses, the
//! session list and focus as `App` holds them, claude installs, and what
//! the runtime tells the UI. It runs `App`'s [`AddonCall`]s, and its
//! launches through the composer, keeping those parked for a review until
//! the member answers. `App` itself never sees the runtime (A6).

use std::collections::HashMap;
use std::io;
use std::ops::Deref;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use tokio::sync::mpsc;

use super::addon_calls::{AddonCall, Origin};
use super::app::{AddonResult, App, BridgeCall, Effect, LaunchCall, Launched, Mode, ReplyTo};
use super::HostEvent;
use crate::addon::{
    checklist as clean_checklist, line, option as clean_option, Addons, Checklist, ChecklistAnswer,
    Frontend, LaunchOp, LaunchOption, Level, Notice, Pick, SessionSnap, TEXT_BYTES,
};
use crate::addons::vocdoni::bridge::Bridge;
use crate::app::addons::{AddonRuntime, RuntimeOptions, RuntimeOut, RuntimeRx};
use crate::app::connections::Connections;
use crate::app::launch::{self, LaunchSpec, Parked, ReviewAnswer, Step};
use crate::client::{Client, Incoming};
use crate::paths;
use crate::proto::{Msg, SessionId};

/// Launches waiting for a review at once; one more fails.
const MAX_PARKED: usize = 8;
/// A checklist's save outside a launch (an addon's action, D13).
const SAVE_TIMEOUT: Duration = Duration::from_secs(15);

/// The runtime and what it tells the UI, as the event loop holds them.
pub(super) struct Runner {
    pub(super) runtime: Edge,
    pub(super) rx: RuntimeRx,
}

/// The runtime, and the launches parked for a review: their parts (the
/// secrets) stay here, never in `App` (C1). Dropped at exit.
#[derive(Clone)]
pub(super) struct Edge {
    rt: AddonRuntime,
    parked: Arc<Mutex<HashMap<u64, Waiting>>>,
}

impl Deref for Edge {
    type Target = AddonRuntime;

    fn deref(&self) -> &AddonRuntime {
        &self.rt
    }
}

impl Edge {
    /// `rt`, no launch parked.
    pub(super) fn new(rt: AddonRuntime) -> Edge {
        Edge {
            rt,
            parked: Arc::default(),
        }
    }

    fn parked(&self) -> MutexGuard<'_, HashMap<u64, Waiting>> {
        self.parked.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Drop the parked launches and stop the runtime.
    pub(super) fn shutdown(&self) {
        self.parked().clear();
        self.rt.shutdown();
    }
}

/// What a launch's outcome reports.
struct Pending {
    id: u64,
    op: LaunchOp,
    sid: SessionId,
    client: Client,
}

/// A launch parked for `addon`'s checklist.
struct Waiting {
    parked: Parked,
    addon: &'static str,
    checklist: String,
    pending: Pending,
}

/// The proxy profiles' names, read now (blocking, at the start): the
/// launches' routing has them from the first launch on.
pub(super) fn read_profiles(addons: &Addons) -> Vec<String> {
    Bridge::of(addons).map_or_else(Vec::new, |b| b.read_profiles())
}

/// The runtime for `addons`, reading the TUI's config.toml. The TUI does
/// not watch the file (D15). The Vocdoni addon runs the TUI's plugins:
/// it learns the names `app`'s own extensions hold first. `app` gets the
/// launch options.
pub(super) fn start(addons: Arc<Addons>, app: &mut App) -> Runner {
    if let Some(bridge) = Bridge::of(&addons) {
        let own = app.extensions.extensions.iter().filter(|e| !e.is_plugin());
        bridge.set_taken(own.map(|e| e.name.clone()).collect());
        bridge.activate();
    }
    let opts = RuntimeOptions {
        config_path: paths::config_file(),
        watch_config: false,
    };
    let (rt, rx) = AddonRuntime::start(addons, opts);
    app.on_addon(AddonResult::Options(options(&rt)));
    Runner {
        runtime: Edge::new(rt),
        rx,
    }
}

/// Every Launch addon's options, cleaned (C2), in registry order.
fn options(runtime: &AddonRuntime) -> Vec<(&'static str, LaunchOption)> {
    let mut all = Vec::new();
    for addon in runtime.addons().iter() {
        let Some(launch) = addon.launch() else {
            continue;
        };
        let ctx = runtime.ctx(addon.name());
        let own = launch.options(ctx.as_ref()).into_iter();
        all.extend(own.map(|o| (addon.name(), clean_option(o))));
    }
    all
}

/// `host`'s daemon answers (through its client in `conns`).
pub(super) fn host_up(runtime: &AddonRuntime, host: &str, conns: &Connections) {
    if let Some(client) = conns.client(host) {
        runtime.host_up(host, client.clone());
    }
}

/// An item from `host`'s daemon for `App`; a lost connection goes to the
/// runtime too.
pub(super) fn on_incoming(app: &mut App, runtime: &AddonRuntime, host: &str, inc: Incoming) {
    if matches!(inc, Incoming::Disconnected) {
        runtime.host_down(host);
    }
    app.on_incoming_from(host, inc);
}

/// A daemon's reply for `App`; a claude install that succeeded goes to the
/// runtime too.
pub(super) fn on_reply(app: &mut App, runtime: &AddonRuntime, to: ReplyTo, reply: io::Result<Msg>) {
    let installed = match (&to, &reply) {
        (
            ReplyTo::ClaudeUpdate(host),
            Ok(Msg::ClaudeUpdated {
                ok: true,
                version: Some(_),
                ..
            }),
        ) => app.claude_installing.contains(host).then(|| host.clone()),
        _ => None,
    };
    app.on_reply(to, reply);
    if let Some(host) = installed {
        runtime.claude_installed(&host);
    }
}

/// The sessions as `App` has them now, its active one focused. The runtime
/// sends events only for what changed; the Vocdoni addon learns the
/// options they (and the run's flags) use.
pub(super) fn sync(runtime: &AddonRuntime, app: &App) {
    if let Some(bridge) = Bridge::of(runtime.addons()) {
        let mut used: Vec<(String, String)> = app
            .sessions
            .iter()
            .filter_map(|v| v.launch.as_ref())
            .map(|c| (c.addon.clone(), c.option.clone()))
            .collect();
        if let Pick::Option { addon, option } = &app.launch_default.pick {
            used.push((addon.clone(), option.clone()));
        }
        bridge.use_launches(&used);
    }
    let active = app.active_view().map(|v| v.id);
    let snaps = app
        .sessions
        .iter()
        .map(|v| SessionSnap {
            sid: v.id,
            host: v.host.clone(),
            kind: v.kind,
            cwd: v.cwd.clone(),
            label: v
                .name
                .clone()
                .or_else(|| v.title.clone())
                .unwrap_or_else(|| v.cwd.clone()),
            conversation: v.claude_session_id.clone(),
            launch: v.launch.clone(),
            focused: Some(v.id) == active,
        })
        .collect();
    runtime.sessions_reset(snaps);
}

/// What the runtime tells the UI: notices are shown, a change re-reads
/// what `App` shows of the addon (its open checklists too) and the launch
/// options; an offer is shown when idle.
pub(super) fn on_out(app: &mut App, runtime: &AddonRuntime, out: RuntimeOut) {
    match out {
        RuntimeOut::Notice { addon, notice } => app.notify(format!("{addon}: {}", notice.text)),
        RuntimeOut::Changed { addon } => {
            app.on_addon(AddonResult::Changed { addon });
            for (shown, id) in app.checklists_shown() {
                if let Some(c) = checklist_of(runtime, shown, &id).filter(|_| shown == addon) {
                    app.refresh_checklist(shown, &c);
                }
            }
            let options = options(runtime);
            if options != app.launch_options {
                app.on_addon(AddonResult::Options(options));
            }
        }
        RuntimeOut::Offer { addon, checklist } => app.on_addon(AddonResult::Offer {
            addon,
            id: checklist,
        }),
    }
}

/// `addon`'s checklist `id` as it is now, cleaned (C2).
fn checklist_of(runtime: &AddonRuntime, addon: &'static str, id: &str) -> Option<Checklist> {
    let entry = runtime.addons().get(addon)?;
    let checklist = entry.ui()?.checklist(id, runtime.ctx(addon).as_ref())?;
    Some(clean_checklist(checklist))
}

/// Run an addon effect: a bridge call, or an [`AddonCall`].
pub(super) fn run(effect: Effect, app: &mut App, edge: &Edge, tx: &mpsc::Sender<HostEvent>) {
    match effect {
        Effect::Bridge(call) => bridge(call, edge),
        Effect::Addon(AddonCall::OpenChecklist { addon, id, origin }) => {
            match checklist_of(edge, addon, &id) {
                Some(checklist) => app.on_addon(AddonResult::Checklist {
                    addon,
                    origin,
                    checklist,
                }),
                None if origin == Origin::Offer => {}
                None => app.notify(format!("{addon}: {}: nothing to set up yet", line(&id, 64))),
            }
        }
        Effect::Addon(AddonCall::SaveChecklist {
            addon,
            id,
            answer,
            origin,
        }) => save_checklist(edge, (addon, id), answer, origin, tx),
        Effect::Addon(AddonCall::ReviewAnswer { launch, answer }) => {
            answer_review(edge, launch, answer, tx)
        }
        _ => {}
    }
}

/// Run a call into the Vocdoni addon's plugin state: the transition is
/// immediate, its I/O runs on the runtime's tasks.
fn bridge(call: BridgeCall, runtime: &AddonRuntime) {
    let Some(bridge) = Bridge::of(runtime.addons()) else {
        return;
    };
    match call {
        BridgeCall::SaveChoices { name, choices } => bridge.save_choices(&name, choices),
        BridgeCall::Defer(name) => bridge.defer(&name),
        BridgeCall::SetEnabled { name, on } => bridge.set_enabled(&name, on),
        BridgeCall::Refresh => bridge.refresh(),
    }
}

/// Save the member's answer to `addon`'s checklist `id` on a task (15 s);
/// a refused save is a notice and opens the checklist again from `origin`.
fn save_checklist(
    edge: &Edge,
    (addon, id): (&'static str, String),
    answer: ChecklistAnswer,
    origin: Origin,
    tx: &mpsc::Sender<HostEvent>,
) {
    let Some(entry) = edge.addons().get(addon).cloned() else {
        return;
    };
    let (rt, tx) = (edge.rt.clone(), tx.clone());
    tokio::spawn(async move {
        let Some(ui) = entry.ui() else {
            return;
        };
        let reopen = matches!(answer, ChecklistAnswer::Save { .. });
        let saving = ui.save_checklist(id.clone(), answer, rt.ctx(addon));
        let why = match tokio::time::timeout(SAVE_TIMEOUT, saving).await {
            Ok(Ok(())) => return,
            Ok(Err(why)) => why,
            Err(_) => "timed out".to_owned(),
        };
        let text = format!(
            "{addon}: {}: not saved: {}",
            line(&id, 64),
            line(&why, TEXT_BYTES)
        );
        let notice = Notice {
            text,
            level: Level::Warn,
        };
        let _ = tx.send(HostEvent::Addon(AddonResult::Notice(notice))).await;
        if let Some(checklist) = checklist_of(&rt, addon, &id).filter(|_| reopen) {
            let shown = AddonResult::Checklist {
                addon,
                origin,
                checklist,
            };
            let _ = tx.send(HostEvent::Addon(shown)).await;
        }
    });
}

/// The member answered launch `id`'s review: the parked launch goes on
/// (or is dropped) on a task. An answer for a launch no longer parked is
/// dropped.
fn answer_review(edge: &Edge, id: u64, answer: ReviewAnswer, tx: &mpsc::Sender<HostEvent>) {
    let Some(waiting) = edge.parked().remove(&id) else {
        return;
    };
    let (edge, tx) = (edge.clone(), tx.clone());
    tokio::spawn(async move {
        let Waiting {
            parked,
            addon,
            checklist,
            pending,
        } = waiting;
        let saved = matches!(answer, ReviewAnswer::Save(_));
        let client = pending.client.clone();
        let step = launch::resume(&edge.rt, parked, answer, client).await;
        let again = matches!(&step, Step::Review { checklist: c, .. } if *c == checklist);
        if saved && again {
            let text = format!("{addon}: {checklist} changed while open: review it again");
            let notice = Notice {
                text,
                level: Level::Warn,
            };
            let _ = tx.send(HostEvent::Addon(AddonResult::Notice(notice))).await;
        }
        settle(&edge, step, pending, &tx).await;
    });
}

/// Run an [`Effect::Launch`] on a task: `app::launch::run` with this
/// host's client; the outcome comes back as [`HostEvent::Launched`], or,
/// for a review, as the checklist to show. The addons' parts (their env)
/// live only in that task and in the parked launches.
pub(super) fn launch(
    call: LaunchCall,
    mode: Mode,
    edge: &Edge,
    conns: &Connections,
    tx: &mpsc::Sender<HostEvent>,
) {
    let LaunchCall {
        id,
        op,
        sid,
        host,
        cwd,
        select,
        mut base,
    } = call;
    let core_lenses = match &mut base {
        Msg::Spawn(spec) => spec.extensions.take(),
        Msg::Respawn(spec) => spec.extensions.take(),
        _ => None,
    };
    let frontend = match mode {
        Mode::Plain => Frontend::Plain,
        Mode::Manager => Frontend::Tui,
    };
    let client = conns.client(&host).cloned();
    let (edge, tx) = (edge.clone(), tx.clone());
    tokio::spawn(async move {
        let Some(client) = client else {
            let failed = Err(format!("{host}: not connected"));
            report(&tx, (id, op, sid), None, Vec::new(), failed).await;
            return;
        };
        let spec = LaunchSpec {
            op,
            sid,
            host,
            cwd,
            select,
            frontend,
            base,
            core_lenses: core_lenses.unwrap_or_default(),
        };
        let step = launch::run(&edge.rt, spec, client.clone()).await;
        let pending = Pending {
            id,
            op,
            sid,
            client,
        };
        settle(&edge, step, pending, &tx).await;
    });
}

/// Where a launch got to: sent or failed is reported; a review parks it
/// and shows the checklist (one the addon cannot show is skipped).
async fn settle(edge: &Edge, mut step: Step, pending: Pending, tx: &mpsc::Sender<HostEvent>) {
    let key = (pending.id, pending.op, pending.sid);
    let (choice, notices, result) = loop {
        match step {
            Step::Sent {
                choice,
                notices,
                reply,
            } => break (choice, notices, reply.map_err(|e| e.to_string())),
            Step::Failed(why) => break (None, Vec::new(), Err(why)),
            Step::Review {
                addon,
                checklist,
                parked,
                ..
            } => {
                let Some(shown) = checklist_of(edge, addon, &checklist) else {
                    let client = pending.client.clone();
                    step = launch::resume(edge, parked, ReviewAnswer::Skip, client).await;
                    continue;
                };
                let waiting = Waiting {
                    parked,
                    addon,
                    checklist,
                    pending,
                };
                if !park(edge, waiting) {
                    let failed = Err("too many launches wait for a review".to_owned());
                    break (None, Vec::new(), failed);
                }
                let origin = Origin::Launch {
                    id: key.0,
                    sid: key.2,
                };
                let open = AddonResult::Checklist {
                    addon,
                    origin,
                    checklist: shown,
                };
                let _ = tx.send(HostEvent::Addon(open)).await;
                return;
            }
        }
    };
    report(tx, key, choice, notices, result).await;
}

/// Keep `waiting` until its answer; `false` when [`MAX_PARKED`] wait.
fn park(edge: &Edge, waiting: Waiting) -> bool {
    let mut parked = edge.parked();
    if parked.len() >= MAX_PARKED {
        return false;
    }
    parked.insert(waiting.pending.id, waiting);
    true
}

/// A launch's outcome, for `App::on_launched`.
async fn report(
    tx: &mpsc::Sender<HostEvent>,
    (id, op, sid): (u64, LaunchOp, SessionId),
    choice: Option<crate::addon::LaunchChoice>,
    notices: Vec<String>,
    result: Result<Msg, String>,
) {
    let done = Launched {
        id,
        op,
        sid,
        choice,
        notices,
        result,
    };
    let _ = tx.send(HostEvent::Launched(done)).await;
}

/// Set `[extensions] name = on` in config.toml off the UI thread; the
/// outcome comes back as a notice.
pub(super) fn save_extension(name: String, on: bool, tx: &mpsc::Sender<HostEvent>) {
    let tx = tx.clone();
    tokio::spawn(async move {
        let state = if on { "on" } else { "off" };
        let path = paths::config_file();
        let own = name.clone();
        let saved =
            tokio::task::spawn_blocking(move || crate::config::save_extension(&path, &own, on))
                .await
                .map_err(|_| "the save stopped".to_owned());
        let text = match saved {
            Ok(Ok(())) => format!("extension {name} {state} (saved)"),
            Ok(Err(e)) => format!("could not save: {e}"),
            Err(e) => format!("could not save: {e}"),
        };
        let _ = tx.send(HostEvent::Notice(text)).await;
    });
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::addon::test_support::TestAddon;
    use crate::addon::Addon;
    use crate::tui::test_support::{app_with, info};

    /// The events `addon` saw, ticks left out, once there are `n`.
    async fn seen(addon: &TestAddon, n: usize) -> Vec<String> {
        let wait = async {
            loop {
                let got: Vec<String> = addon
                    .calls()
                    .into_iter()
                    .filter(|c| c.starts_with("event ") && c != "event Tick")
                    .collect();
                if got.len() >= n {
                    return got;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        };
        tokio::time::timeout(Duration::from_secs(5), wait)
            .await
            .expect("events in time")
    }

    #[tokio::test]
    async fn the_runtime_follows_the_apps_sessions_and_claude_installs() {
        let echo = Arc::new(TestAddon::new("echo"));
        let addons = Arc::new(Addons::new(vec![Arc::clone(&echo) as Arc<dyn Addon>]).unwrap());
        let opts = RuntimeOptions {
            config_path: "/nonexistent/claudio/config.toml".into(),
            watch_config: false,
        };
        let (runtime, _rx) = AddonRuntime::start(addons, opts);
        let mut app = app_with(&[info(Some(7), None)]);
        let sid = app.sessions[0].id;
        sync(&runtime, &app);
        sync(&runtime, &app); // nothing changed: no event
        let got = seen(&echo, 2).await;
        assert_eq!(
            got[1],
            format!("event SessionAdded {{ sid: {sid}, conversation: None }}")
        );
        // A claude install that succeeded reaches the runtime; a failed one not.
        app.claude_installing.insert("local".into());
        let ok = Msg::ClaudeUpdated {
            version: Some("2.0.0".into()),
            ok: true,
            tail: String::new(),
        };
        on_reply(
            &mut app,
            &runtime,
            ReplyTo::ClaudeUpdate("local".into()),
            Ok(ok),
        );
        assert_eq!(
            seen(&echo, 3).await[2],
            "event ClaudeInstalled { host: \"local\" }"
        );
        app.sessions.clear();
        sync(&runtime, &app);
        assert_eq!(
            seen(&echo, 4).await[3],
            format!("event SessionRemoved {{ sid: {sid} }}")
        );
        runtime.shutdown();
    }

    #[tokio::test]
    async fn a_launch_is_composed_sent_and_reported_back() {
        use crate::addon::{LaunchOp, LaunchSelect};
        use crate::app::addons::test_support::{fake_host, next_request, reply};
        use crate::proto::{LensKind, LensSpec, SessionExtensions, SpawnSpec};

        let opts = RuntimeOptions {
            config_path: "/nonexistent/claudio/config.toml".into(),
            watch_config: false,
        };
        let (runtime, _rx) = AddonRuntime::start(Arc::new(Addons::empty()), opts);
        let (seen_tx, mut seen) = mpsc::unbounded_channel();
        let client = fake_host(&[], move |mut s| async move {
            while let Some((req, msg)) = next_request(&mut s).await {
                if let Msg::Spawn(spec) = &msg {
                    let id = spec.id;
                    let _ = seen_tx.send(msg);
                    reply(&mut s, req, Msg::Spawned { id, pid: None }).await;
                }
            }
        })
        .await;
        let conns = Connections::with_local(client);
        let sid = uuid::Uuid::new_v4();
        let core = SessionExtensions {
            lenses: vec![LensSpec {
                name: "latex".into(),
                kind: LensKind::Tag,
                instruction: "write <latex>".into(),
            }],
            ..Default::default()
        };
        let call = LaunchCall {
            id: 3,
            op: LaunchOp::New,
            sid,
            host: "local".into(),
            cwd: "/w".into(),
            select: LaunchSelect::of(None),
            base: Msg::Spawn(SpawnSpec {
                id: sid,
                cwd: "/w".into(),
                extensions: Some(core.clone()),
                ..Default::default()
            }),
        };
        let (tx, mut rx) = mpsc::channel(4);
        launch(call, Mode::Plain, &Edge::new(runtime.clone()), &conns, &tx);
        let Some(HostEvent::Launched(done)) = rx.recv().await else {
            panic!("no outcome")
        };
        assert_eq!((done.id, done.sid, done.op), (3, sid, LaunchOp::New));
        assert!(matches!(done.result, Ok(Msg::Spawned { id, .. }) if id == sid));
        assert!(done.choice.is_none() && done.notices.is_empty());
        // Direct: no env, the markers-only env_remove, the core's lenses.
        let Some(Msg::Spawn(spec)) = seen.recv().await else {
            panic!("no spawn")
        };
        assert!(spec.env.is_empty());
        assert_eq!(spec.env_remove, Some(Vec::new()));
        assert_eq!(spec.extensions, Some(core));

        // Without a connection the launch fails, and nothing is sent.
        let call = LaunchCall {
            host: "far".into(),
            ..launch_call_like(sid)
        };
        launch(
            call,
            Mode::Manager,
            &Edge::new(runtime.clone()),
            &conns,
            &tx,
        );
        let Some(HostEvent::Launched(done)) = rx.recv().await else {
            panic!("no outcome")
        };
        assert_eq!(done.result.unwrap_err(), "far: not connected");
        runtime.shutdown();
    }

    /// A runtime whose one addon routes every launch and needs checklist
    /// `setup` (revision 1) reviewed first; a daemon on `local` that
    /// records the spawns it is sent.
    async fn reviewing() -> (
        Arc<TestAddon>,
        Edge,
        Connections,
        mpsc::UnboundedReceiver<Msg>,
    ) {
        use crate::addon::test_support::Script;
        use crate::addon::{LaunchChoice, LaunchPart, Routing};
        use crate::app::addons::test_support::{fake_host, next_request, reply};
        let checklist = Checklist {
            id: "setup".into(),
            revision: "1".into(),
            title: "Set up".into(),
            intro: String::new(),
            items: Vec::new(),
        };
        let choice = LaunchChoice {
            addon: "route".into(),
            option: "work".into(),
            flags: Vec::new(),
        };
        let part = LaunchPart {
            env: vec![("R".into(), "r".into())],
            ..LaunchPart::default()
        };
        let route = Arc::new(
            TestAddon::new("route")
                .routing(Ok(Routing::Required(choice)))
                .prepare(Script::Review(part))
                .checklist(checklist),
        );
        let addons = Arc::new(Addons::new(vec![route.clone() as Arc<dyn Addon>]).unwrap());
        let opts = RuntimeOptions {
            config_path: "/nonexistent/claudio/config.toml".into(),
            watch_config: false,
        };
        let (runtime, _rx) = AddonRuntime::start(addons, opts);
        let (seen_tx, seen) = mpsc::unbounded_channel();
        let client = fake_host(&[], move |mut s| async move {
            while let Some((req, msg)) = next_request(&mut s).await {
                if let Msg::Spawn(spec) = &msg {
                    let id = spec.id;
                    let _ = seen_tx.send(msg);
                    reply(&mut s, req, Msg::Spawned { id, pid: None }).await;
                }
            }
        })
        .await;
        (
            route,
            Edge::new(runtime),
            Connections::with_local(client),
            seen,
        )
    }

    /// Launch `id`, a new session; its review's checklist comes back.
    async fn parked_launch(
        edge: &Edge,
        conns: &Connections,
        id: u64,
        tx: &mpsc::Sender<HostEvent>,
        rx: &mut mpsc::Receiver<HostEvent>,
    ) -> SessionId {
        let sid = uuid::Uuid::new_v4();
        let call = LaunchCall {
            id,
            sid,
            select: crate::addon::LaunchSelect::default(),
            ..launch_call_like(sid)
        };
        launch(call, Mode::Manager, edge, conns, tx);
        match rx.recv().await {
            Some(HostEvent::Addon(AddonResult::Checklist {
                addon,
                origin,
                checklist,
            })) => {
                assert_eq!((addon, checklist.id.as_str()), ("route", "setup"));
                assert_eq!(origin, Origin::Launch { id, sid });
            }
            other => panic!("no checklist: {:?}", other.is_some()),
        }
        assert_eq!(edge.parked().len(), 1);
        sid
    }

    async fn launched(rx: &mut mpsc::Receiver<HostEvent>) -> Launched {
        match rx.recv().await {
            Some(HostEvent::Launched(done)) => done,
            other => panic!("no outcome: {:?}", other.is_some()),
        }
    }

    /// The 25 s budget stops while the checklist is open: the member takes
    /// longer than that, and the saved answer still launches the session.
    #[tokio::test(start_paused = true)]
    async fn a_review_stops_the_clock_and_enter_saves_then_launches() {
        let (route, edge, conns, mut seen) = reviewing().await;
        let (tx, mut rx) = mpsc::channel(8);
        let sid = parked_launch(&edge, &conns, 1, &tx, &mut rx).await;
        // Longer than the whole budget (the client's own keepalive bounds
        // how long a paused test may sleep).
        tokio::time::sleep(Duration::from_secs(30)).await;
        assert!(seen.try_recv().is_err(), "nothing sent while it waits");
        let save = ReviewAnswer::Save(ChecklistAnswer::Save {
            revision: "1".into(),
            off: Vec::new(),
        });
        answer_review(&edge, 1, save, &tx);
        let done = launched(&mut rx).await;
        assert_eq!((done.id, done.sid), (1, sid));
        assert!(done.result.is_ok(), "{:?}", done.result);
        assert!(route.saved());
        let Some(Msg::Spawn(spec)) = seen.recv().await else {
            panic!("no spawn")
        };
        assert_eq!(spec.env, [("R".to_owned(), "r".to_owned())]);
        assert!(edge.parked().is_empty());
        // A second answer to the same launch is dropped.
        answer_review(&edge, 1, ReviewAnswer::Skip, &tx);
        edge.shutdown();
    }

    #[tokio::test]
    async fn skip_launches_without_saving_and_the_next_launch_asks_again() {
        let (route, edge, conns, mut seen) = reviewing().await;
        let (tx, mut rx) = mpsc::channel(8);
        parked_launch(&edge, &conns, 1, &tx, &mut rx).await;
        answer_review(&edge, 1, ReviewAnswer::Skip, &tx);
        assert!(launched(&mut rx).await.result.is_ok());
        assert!(!route.saved());
        assert!(seen.recv().await.is_some());
        parked_launch(&edge, &conns, 2, &tx, &mut rx).await;
        edge.shutdown();
        assert!(edge.parked().is_empty(), "dropped at exit");
    }

    #[tokio::test]
    async fn esc_cancels_the_launch_and_nothing_is_sent() {
        let (route, edge, conns, mut seen) = reviewing().await;
        let (tx, mut rx) = mpsc::channel(8);
        parked_launch(&edge, &conns, 1, &tx, &mut rx).await;
        answer_review(&edge, 1, ReviewAnswer::Cancel, &tx);
        assert_eq!(launched(&mut rx).await.result.unwrap_err(), "cancelled");
        assert!(!route.saved());
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(seen.try_recv().is_err(), "nothing sent");
        edge.shutdown();
    }

    #[tokio::test]
    async fn at_most_eight_launches_wait_for_a_review() {
        let (_route, edge, conns, _seen) = reviewing().await;
        let (tx, mut rx) = mpsc::channel(32);
        for id in 1..=MAX_PARKED as u64 {
            let sid = uuid::Uuid::new_v4();
            let call = LaunchCall {
                id,
                sid,
                select: crate::addon::LaunchSelect::default(),
                ..launch_call_like(sid)
            };
            launch(call, Mode::Manager, &edge, &conns, &tx);
            assert!(matches!(rx.recv().await, Some(HostEvent::Addon(_))));
        }
        let sid = uuid::Uuid::new_v4();
        let call = LaunchCall {
            id: 99,
            sid,
            select: crate::addon::LaunchSelect::default(),
            ..launch_call_like(sid)
        };
        launch(call, Mode::Manager, &edge, &conns, &tx);
        let done = launched(&mut rx).await;
        assert_eq!(done.id, 99);
        assert_eq!(
            done.result.unwrap_err(),
            "too many launches wait for a review"
        );
        edge.shutdown();
    }

    fn launch_call_like(sid: crate::proto::SessionId) -> LaunchCall {
        LaunchCall {
            id: 4,
            op: crate::addon::LaunchOp::New,
            sid,
            host: "local".into(),
            cwd: "/w".into(),
            select: crate::addon::LaunchSelect::of(None),
            base: Msg::Spawn(crate::proto::SpawnSpec {
                id: sid,
                ..Default::default()
            }),
        }
    }
}
