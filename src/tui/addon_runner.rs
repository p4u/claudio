//! The TUI's edge of the addon runtime (`app::addons`): it is built in
//! `main` and fed from the event loop: host connections and losses, the
//! session list and focus as `App` holds them, claude installs, and what
//! the runtime tells the UI. `App` itself never sees the runtime (A6).

use std::io;
use std::sync::Arc;

use tokio::sync::mpsc;

use super::app::{AddonResult, App, BridgeCall, LaunchCall, Launched, Mode, ReplyTo};
use super::HostEvent;
use crate::addon::{option as clean_option, Addons, Frontend, LaunchOption, Pick, SessionSnap};
use crate::addons::vocdoni::bridge::Bridge;
use crate::app::addons::{AddonRuntime, RuntimeOptions, RuntimeOut, RuntimeRx};
use crate::app::connections::Connections;
use crate::app::launch::{self, LaunchSpec, Step};
use crate::client::Incoming;
use crate::paths;
use crate::proto::Msg;

/// The runtime and what it tells the UI, as the event loop holds them.
pub(super) struct Runner {
    pub(super) runtime: AddonRuntime,
    pub(super) rx: RuntimeRx,
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
    let (runtime, rx) = AddonRuntime::start(addons, opts);
    app.on_addon(AddonResult::Options(options(&runtime)));
    Runner { runtime, rx }
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
/// what `App` shows of the addon and the launch options; offers have no
/// reader yet (P3.11).
pub(super) fn on_out(app: &mut App, runtime: &AddonRuntime, out: RuntimeOut) {
    match out {
        RuntimeOut::Notice { addon, notice } => app.notify(format!("{addon}: {}", notice.text)),
        RuntimeOut::Changed { addon } => {
            app.on_addon(AddonResult::Changed { addon });
            let options = options(runtime);
            if options != app.launch_options {
                app.on_addon(AddonResult::Options(options));
            }
        }
        RuntimeOut::Offer { .. } => {}
    }
}

/// Run a call into the Vocdoni addon's plugin state: the transition is
/// immediate, its I/O runs on the runtime's tasks.
pub(super) fn bridge(call: BridgeCall, runtime: &AddonRuntime) {
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

/// Run an [`Effect::Launch`] on a task: `app::launch::run` with this
/// host's client; the outcome comes back as [`HostEvent::Launched`]. The
/// addons' parts (their env) live only in that task.
pub(super) fn launch(
    call: LaunchCall,
    mode: Mode,
    runtime: &AddonRuntime,
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
    let (rt, tx) = (runtime.clone(), tx.clone());
    tokio::spawn(async move {
        let (choice, notices, result) = match client {
            None => (None, Vec::new(), Err(format!("{host}: not connected"))),
            Some(client) => {
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
                match launch::run(&rt, spec, client).await {
                    Step::Sent {
                        choice,
                        notices,
                        reply,
                    } => (choice, notices, reply.map_err(|e| e.to_string())),
                    // The TUI shows checklists from P3.11 on; until then
                    // such a launch fails (the parked parts are dropped).
                    Step::Review { addon, .. } => {
                        (None, Vec::new(), Err(format!("{addon}: review needed")))
                    }
                    Step::Failed(why) => (None, Vec::new(), Err(why)),
                }
            }
        };
        let done = Launched {
            id,
            op,
            sid,
            choice,
            notices,
            result,
        };
        let _ = tx.send(HostEvent::Launched(done)).await;
    });
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
        launch(call, Mode::Plain, &runtime, &conns, &tx);
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
        launch(call, Mode::Manager, &runtime, &conns, &tx);
        let Some(HostEvent::Launched(done)) = rx.recv().await else {
            panic!("no outcome")
        };
        assert_eq!(done.result.unwrap_err(), "far: not connected");
        runtime.shutdown();
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
