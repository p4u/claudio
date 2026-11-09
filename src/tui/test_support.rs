//! Fixtures shared by the TUI's unit tests.

use std::collections::HashMap;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

use super::app::{AddonResult, App, AppConfig, BridgeCall, Effect, LaunchCall, Mode};
use super::keymap::Keymap;
use crate::addons::vocdoni::bridge::Bridge;
use crate::addons::vocdoni::plugins::manifest::Manifest;
use crate::addons::vocdoni::plugins::state::PluginsFetch;
use crate::addons::vocdoni::proxy::plugins::{sample, PluginList};
use crate::app::state::ClientState;
use crate::proto::{Msg, SessionInfo};

pub use crate::app::test_support::{info, shell_info};

/// A manager on a 100×30 terminal with home `/home/u`, the default keys, and
/// nothing from the machine it runs on: no proxy profiles, no SSH hosts, no
/// saved state, no local claude.
pub fn config() -> AppConfig {
    AppConfig {
        mode: Mode::Manager,
        size: (100, 30),
        home: "/home/u".to_owned(),
        keymap: Keymap::default(),
        notify: true,
        copy_on_select: true,
        proxy_override: Default::default(),
        first_party: false,
        proxy_profiles: Vec::new(),
        proxy_default: None,
        claude: Default::default(),
        local_claude: None,
        recent_dirs: HashMap::new(),
        claude_skipped: HashMap::new(),
        ssh_hosts: Vec::new(),
        addons: std::sync::Arc::new(crate::addon::Addons::new(crate::addons::all()).unwrap()),
    }
}

/// An app made from [`config`].
pub fn app() -> App {
    App::new(config())
}

/// An app that recovered `live` from the local daemon (and nothing from
/// state.json), with the effects of that drained. Without sessions, the
/// wizard is open.
pub fn app_with(live: &[SessionInfo]) -> App {
    let mut app = app();
    app.recover(&ClientState::default(), live);
    app.take_effects();
    app
}

/// A key press with `mods`, as the terminal reports it.
pub fn key(code: KeyCode, mods: KeyModifiers) -> Event {
    Event::Key(KeyEvent::new(code, mods))
}

/// A key press without modifiers.
pub fn press(code: KeyCode) -> Event {
    key(code, KeyModifiers::NONE)
}

/// Alt+`c`.
pub fn alt(c: char) -> Event {
    key(KeyCode::Char(c), KeyModifiers::ALT)
}

/// The daemon messages `effects` queue, in order: each request's, and each
/// launch's base (`Effect::Launch`: a claude session without a proxy goes
/// through the launch composer, which adds the addons' share).
pub fn messages(effects: &[Effect]) -> Vec<&Msg> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::Request { msg, .. } => Some(msg),
            Effect::Launch(call) => Some(&call.base),
            _ => None,
        })
        .collect()
}

/// The launches `effects` queue, in order.
pub fn launches(effects: &[Effect]) -> Vec<&LaunchCall> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::Launch(call) => Some(call),
            _ => None,
        })
        .collect()
}

/// Answer every launch of `effects` as a daemon would (`Spawned`, or a
/// `Respawn`'s ack) and return what that queues (the held-back Attach).
pub fn finish_launches(app: &mut App, effects: &[Effect]) -> Vec<Effect> {
    for call in launches(effects) {
        let done = super::app::Launched {
            id: call.id,
            op: call.op,
            sid: call.sid,
            choice: None,
            notices: Vec::new(),
            result: Ok(Msg::Spawned {
                id: call.sid,
                pid: None,
            }),
        };
        app.on_launched(done);
    }
    app.take_effects()
}

// ── The Vocdoni addon's plugin state, through its bridge ─────────────────────

/// `app`'s Vocdoni bridge (each test app has its own addon).
pub fn bridge(app: &App) -> Bridge {
    Bridge::of(&app.addons).expect("the vocdoni addon")
}

/// The runtime's `Changed` after a transition: `App` re-reads the state.
pub fn changed(app: &mut App) {
    let addon = crate::addons::vocdoni::NAME;
    app.on_addon(AddonResult::Changed { addon });
}

/// `host` connected, with a daemon that serves the addon.
pub fn plugin_host(app: &mut App, host: &str) {
    bridge(app).state().host_up(host, true);
    changed(app);
}

/// Profile `profile` serves the sample plugins `names` (fetched). The
/// addon knows the names `app`'s own extensions hold, as at start.
pub fn serve(app: &mut App, profile: &str, names: &[&str]) {
    let own = app.extensions.extensions.iter().filter(|e| !e.is_plugin());
    bridge(app).set_taken(own.map(|e| e.name.clone()).collect());
    let list = PluginList {
        profile: profile.into(),
        fetched_at: 1,
        plugins: names.iter().map(|n| sample(n)).collect(),
    };
    bridge(app)
        .state()
        .on_list(profile, PluginsFetch::Listed(list));
    changed(app);
}

/// Plugin `name`'s manifest `m` arrived at hash `aaa…`.
pub fn manifest_arrives(app: &mut App, name: &str, m: Manifest) {
    bridge(app)
        .state()
        .on_manifest(name, &"a".repeat(64), Ok(m));
    changed(app);
}

/// Run the bridge calls among `app`'s effects as `addon_runner::bridge`
/// does (their I/O is dropped: no runtime) and the `Changed` that follows;
/// the other effects.
pub fn run_bridge(app: &mut App) -> Vec<Effect> {
    let b = bridge(app);
    let mut rest = Vec::new();
    for effect in app.take_effects() {
        match effect {
            Effect::Bridge(BridgeCall::SaveChoices { name, choices }) => {
                b.save_choices(&name, choices)
            }
            Effect::Bridge(BridgeCall::Defer(name)) => b.defer(&name),
            Effect::Bridge(BridgeCall::SetEnabled { name, on }) => b.set_enabled(&name, on),
            Effect::Bridge(BridgeCall::Refresh) => b.refresh(),
            other => rest.push(other),
        }
    }
    changed(app);
    rest
}
