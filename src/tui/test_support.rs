//! Fixtures shared by the TUI's unit tests.

use std::collections::HashMap;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

use super::app::{AddonResult, App, AppConfig, Effect, LaunchCall, Mode};
use super::keymap::Keymap;
use crate::app::state::{ClientState, SavedSession};
use crate::proto::{Msg, SessionInfo};

pub use crate::app::test_support::{info, shell_info};

/// A manager on a 100×30 terminal with home `/home/u`, the default keys, and
/// nothing from the machine it runs on: no proxy profiles, no SSH hosts, no
/// saved state, no local claude.
pub fn config() -> AppConfig {
    let addons = std::sync::Arc::new(crate::addon::Addons::new(crate::addons::all()).unwrap());
    let panels = super::panel_view::panel_bindings(&addons);
    AppConfig {
        mode: Mode::Manager,
        size: (100, 30),
        home: "/home/u".to_owned(),
        keymap: Keymap::build(&Default::default(), &mut Vec::new(), &panels),
        notify: true,
        copy_on_select: true,
        launch_default: Default::default(),
        claude: Default::default(),
        local_claude: None,
        recent_dirs: HashMap::new(),
        claude_skipped: HashMap::new(),
        ssh_hosts: Vec::new(),
        addons,
    }
}

/// The launch choice of proxy profile `profile` (the Vocdoni addon's).
pub fn via(profile: &str) -> Option<crate::addon::LaunchChoice> {
    Some(crate::addon::LaunchChoice {
        addon: "vocdoni".into(),
        option: profile.into(),
        flags: Vec::new(),
    })
}

/// An app made from [`config`].
pub fn app() -> App {
    App::new(config())
}

/// An app that recovered `live` from the local daemon, each one known to
/// state.json as a direct session this manager started (a session only
/// the daemon knows is unknown, R19), with the effects of that drained.
/// Without sessions, the wizard is open.
pub fn app_with(live: &[SessionInfo]) -> App {
    let mut app = app();
    app.recover(&known(live), live);
    app.take_effects();
    app
}

/// state.json knowing each of `live` as a direct session started here.
pub fn known(live: &[SessionInfo]) -> ClientState {
    let sessions = live
        .iter()
        .map(|info| SavedSession {
            id: info.id,
            name: info.name.clone(),
            cwd: info.cwd.clone(),
            host: "local".to_owned(),
            claude_session_id: info.claude_session_id.clone(),
            created_at: info.created_at,
            launch: None,
            proxy: None,
            first_party: false,
            route_unknown: false,
            kind: info.kind,
        })
        .collect();
    ClientState {
        sessions,
        ..ClientState::default()
    }
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
/// `Respawn`'s ack; a chosen option is its choice) and return what that
/// queues (the held-back Attach).
pub fn finish_launches(app: &mut App, effects: &[Effect]) -> Vec<Effect> {
    for call in launches(effects) {
        let done = super::app::Launched {
            id: call.id,
            op: call.op,
            sid: call.sid,
            // A chosen option is the effective choice; a default one direct.
            choice: match &call.select.pick {
                crate::addon::Pick::Option { addon, option } => Some(crate::addon::LaunchChoice {
                    addon: addon.clone(),
                    option: option.clone(),
                    flags: call.select.flags.clone(),
                }),
                _ => None,
            },
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

// ── Addon rows ───────────────────────────────────────────────────────────────

/// Addon `addon`'s rows as the runtime would send them: `names`, each a
/// plugin of profile `work` at 1.4.0 with `details`; `refused` ones say why.
pub fn rows(app: &mut App, addon: &'static str, names: &[(&str, Option<&str>)], details: &[&str]) {
    let rows = names
        .iter()
        .map(|(name, refused)| {
            let row = crate::addon::ExtensionRow {
                name: (*name).to_owned(),
                description: "1.4.0 · work".to_owned(),
                default_on: true,
                refused: refused.map(str::to_owned),
                details: details.iter().map(|d| (*d).to_owned()).collect(),
                setup: Some((*name).to_owned()),
            };
            (addon, row)
        })
        .collect();
    app.on_addon(AddonResult::Rows(rows));
}
