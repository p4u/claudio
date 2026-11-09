//! Fixtures shared by the TUI's unit tests.

use std::collections::HashMap;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

use super::app::{App, AppConfig, Effect, LaunchCall, Mode};
use super::keymap::Keymap;
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
