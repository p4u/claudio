//! `claudio --plain`: the manager's UI around exactly one bare claude.
//!
//! [`Mode::Plain`] reuses the manager's machinery (daemon session, pane,
//! help / stats / history / reset popups) and drops everything else: no tab
//! bar, status bar or wizard, only the
//! [`PLAIN_ACTIONS`](super::keymap::PLAIN_ACTIONS) keys are claudio's, and
//! state.json is neither read nor written.
//!
//! The session is the UI's: when claude ends, so does the UI (with claude's
//! exit status); when the UI ends, `tui/mod.rs` kills the session, so no
//! trace is left in the daemon.

use std::process::ExitCode;

use crate::proto::{SessionEvent, SessionId, SessionInfo, SessionKind};

use super::app::{App, Mode};

/// What `claudio --plain` starts.
pub struct PlainStart {
    /// The session's id, chosen up front so it can be killed on any exit.
    pub id: SessionId,
    /// Arguments for claude, as given on the command line.
    pub args: Vec<String>,
    /// Where claude runs: the current directory.
    pub cwd: String,
}

/// How a plain UI ends.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Exit {
    /// Claude's exit status (0 when unknown).
    pub code: i32,
    /// A failure to report on stderr once the terminal is restored.
    pub message: Option<String>,
}

impl Exit {
    /// The process exit code: claude's status when it fits in a byte.
    pub fn exit_code(&self) -> ExitCode {
        ExitCode::from(u8::try_from(self.code).unwrap_or(1))
    }
}

impl App {
    /// Start the plain UI's session on the local daemon, with the proxy
    /// profile the startup choice picks (like the wizard's default). The app
    /// is one made in [`Mode::Plain`].
    pub fn start_plain(&mut self, start: PlainStart) {
        debug_assert_eq!(self.mode, Mode::Plain);
        let proxy = self
            .proxy_override
            .pick(self.proxy_default.as_deref())
            .map(str::to_owned);
        let PlainStart { id, args, cwd } = start;
        let started = self.start(id, "local".to_owned(), cwd, SessionKind::Claude, args, proxy, 0);
        if let Err(e) = started {
            self.plain_failed(format!("cannot start claude: {e}"));
        }
    }

    /// The plain UI cannot go on: leave with a failure and say why.
    pub(super) fn plain_failed(&mut self, message: String) {
        self.exit = Exit {
            code: 1,
            message: Some(message),
        };
        self.quit = true;
    }

    /// A daemon event in plain mode. Events of other sessions (the daemon
    /// is shared with the manager) are swallowed; the end of ours ends the UI.
    /// Returns whether the event was consumed.
    pub(super) fn plain_event(&mut self, id: SessionId, event: &SessionEvent) -> bool {
        if self.sessions.first().map(|v| v.id) != Some(id) {
            return true;
        }
        match event {
            SessionEvent::Exited { code } => {
                self.exit.code = code.unwrap_or(0);
                self.quit = true;
                true
            }
            // Closed from outside, or after a clean exit (already handled).
            SessionEvent::Removed => {
                self.quit = true;
                true
            }
            _ => false,
        }
    }

    /// The local daemon came back: re-attach our session, and do not adopt
    /// whatever else it runs.
    pub(super) fn plain_reconnected(&mut self, live: &[SessionInfo]) {
        let Some(id) = self.sessions.first().map(|v| v.id) else {
            return;
        };
        if live.iter().any(|s| s.id == id) {
            self.sessions[0].attached = false;
            self.activate(0);
        } else {
            self.plain_failed("the session daemon restarted and lost the session".to_owned());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::Incoming;
    use crate::proto::{Msg, SessionState};
    use crate::proxy::ProxyChoice;
    use crate::tui::app::{AppConfig, Effect, ReplyTo};
    use crate::tui::interaction::Modal;
    use crate::tui::keymap::Keymap;
    use crate::tui::test_support;
    use crate::tui::ui;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use uuid::Uuid;

    fn plain_app(args: &[&str]) -> (App, SessionId) {
        let mut notices = Vec::new();
        let mut app = App::new(AppConfig {
            mode: Mode::Plain,
            keymap: Keymap::build_plain(&Default::default(), &mut notices),
            notify: false,
            proxy_override: ProxyChoice::Direct,
            ..test_support::config()
        });
        let id = Uuid::new_v4();
        app.start_plain(PlainStart {
            id,
            args: args.iter().map(|a| a.to_string()).collect(),
            cwd: "/work".into(),
        });
        (app, id)
    }

    fn alt(c: char) -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT))
    }

    fn event(app: &mut App, id: SessionId, event: SessionEvent) {
        app.on_incoming_from("local", Incoming::Event { id, event });
    }

    fn screen(app: &App) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(app.width, app.height)).unwrap();
        terminal.draw(|f| ui::draw(f, app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_owned())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }

    #[test]
    fn spawns_one_local_session_in_the_cwd_with_the_given_args() {
        let (mut app, id) = plain_app(&["--resume", "abc"]);
        let effects = app.take_effects();
        let spawn = effects.iter().find_map(|e| match e {
            Effect::Request {
                host,
                msg: Msg::Spawn(spec),
                to,
            } => Some((host, spec, to)),
            _ => None,
        });
        let (host, spec, to) = spawn.expect("a Spawn request");
        assert_eq!(host, "local");
        assert_eq!((spec.id, spec.cwd.as_str()), (id, "/work"));
        assert_eq!(spec.args, ["--resume", "abc"]);
        assert!(spec.env.is_empty(), "no proxy, no env");
        // The pane is the whole screen.
        assert_eq!((spec.rows, spec.cols), (30, 100));
        assert_eq!(*to, ReplyTo::Spawned(id));
        assert_eq!(app.sessions.len(), 1);
        assert!(app.modal.is_none(), "no wizard");
    }

    #[test]
    fn renders_the_pane_full_screen_without_tab_or_status_bar() {
        let (mut app, id) = plain_app(&[]);
        app.take_effects();
        app.sessions[0].mirror.feed(b"FIRST ROW\r\n");
        app.sessions[0].mirror.feed(b"\x1b[30;1HLAST ROW");
        event(&mut app, id, SessionEvent::State { state: SessionState::Idle });
        let rows = screen(&app);
        assert_eq!(rows.len(), 30);
        assert_eq!(rows[0], "FIRST ROW", "the pane starts at the top");
        assert_eq!(rows[29], "LAST ROW", "and runs to the bottom");
        let all = rows.join("\n");
        assert!(!all.contains("Alt+h help"), "no status bar:\n{all}");
        assert!(!all.contains('│'), "no tab separators:\n{all}");
    }

    #[test]
    fn manager_keys_go_to_claude_and_only_the_plain_keys_act() {
        let (mut app, id) = plain_app(&[]);
        app.take_effects();
        for c in ['n', 'x', 'r', 'g', 'c', 'a', 'q'] {
            app.on_terminal(alt(c));
            let effects = app.take_effects();
            let expected = [0x1b, c as u8];
            assert!(
                matches!(&effects[..], [Effect::Input(i, b)] if *i == id && b == &expected),
                "Alt+{c} must reach claude, got {effects:?}"
            );
            assert!(app.modal.is_none() && !app.quit, "Alt+{c}");
        }
        app.on_terminal(alt('h'));
        assert!(matches!(app.modal, Some(Modal::Help)));
        assert!(app.take_effects().is_empty(), "Alt+h is not forwarded");
    }

    #[test]
    fn help_popup_lists_only_the_plain_keys() {
        let (mut app, _) = plain_app(&[]);
        app.take_effects();
        app.on_terminal(alt('h'));
        let text = screen(&app).join("\n");
        for key in ["Alt+h", "Alt+s", "Alt+l", "Alt+e"] {
            assert!(text.contains(key), "{key} missing:\n{text}");
        }
        for gone in ["Alt+n", "Alt+x", "Alt+q", "Alt+g", "Alt+c", "Alt+Shift"] {
            assert!(!text.contains(gone), "{gone} should not be listed:\n{text}");
        }
    }

    #[test]
    fn alt_e_asks_how_to_reset() {
        let (mut app, _) = plain_app(&[]);
        app.take_effects();
        app.on_terminal(alt('e'));
        assert!(matches!(app.modal, Some(Modal::Confirm(_))));
    }

    #[test]
    fn the_end_of_claude_ends_the_ui_with_its_status() {
        let (mut app, id) = plain_app(&[]);
        app.take_effects();
        // Another client's session is none of our business.
        event(&mut app, Uuid::new_v4(), SessionEvent::Exited { code: Some(9) });
        assert!(!app.quit);
        event(&mut app, id, SessionEvent::Exited { code: Some(3) });
        assert!(app.quit);
        assert_eq!(app.exit, Exit { code: 3, message: None });
        // The daemon then closes a cleanly ended session: still quitting.
        event(&mut app, id, SessionEvent::Removed);
        assert_eq!(app.exit.code, 3);
    }

    #[test]
    fn removal_without_a_status_exits_zero() {
        let (mut app, id) = plain_app(&[]);
        app.take_effects();
        event(&mut app, id, SessionEvent::Removed);
        assert!(app.quit);
        assert_eq!(app.exit, Exit::default());
        let (mut app, id) = plain_app(&[]);
        event(&mut app, id, SessionEvent::Exited { code: None });
        assert!(app.quit && app.exit.code == 0);
    }

    #[test]
    fn a_failed_spawn_exits_with_the_reason() {
        let (mut app, id) = plain_app(&[]);
        app.take_effects();
        app.on_reply(
            ReplyTo::Spawned(id),
            Err(std::io::Error::other("working directory /work does not exist")),
        );
        assert!(app.quit);
        assert_eq!(app.exit.code, 1);
        assert!(app.exit.message.as_deref().unwrap().contains("does not exist"));
    }

    #[test]
    fn state_json_is_never_written() {
        let (mut app, id) = plain_app(&[]);
        let mut effects = app.take_effects();
        // Everything that saves in the manager: activation, daemon events,
        // a restart, a rename from elsewhere.
        app.on_incoming_from("local", Incoming::Attached { id, rows: 30, cols: 100 });
        event(&mut app, id, SessionEvent::ClaudeSession { claude_session_id: "c1".into() });
        event(&mut app, id, SessionEvent::Renamed { name: Some("n".into()) });
        app.on_reply(ReplyTo::Respawned(id), Ok(Msg::Pong));
        effects.extend(app.take_effects());
        assert!(
            !effects.iter().any(|e| matches!(e, Effect::Save)),
            "plain mode must not save: {effects:?}"
        );
    }

    #[test]
    fn foreign_sessions_never_appear() {
        let (mut app, _) = plain_app(&[]);
        app.take_effects();
        let other = crate::proto::SessionInfo {
            id: Uuid::new_v4(),
            cwd: "/other".into(),
            name: None,
            state: SessionState::Idle,
            claude_session_id: None,
            title: None,
            pid: Some(1),
            created_at: 1,
            branch: None,
            model: None,
            context_tokens: None,
            kind: SessionKind::Claude,
        };
        event(&mut app, other.id, SessionEvent::Created { info: other.clone() });
        assert_eq!(app.sessions.len(), 1);
        // A reconnect re-attaches ours and leaves the others alone.
        let ours = app.sessions[0].id;
        let mut mine = other.clone();
        mine.id = ours;
        app.on_disconnected_local();
        app.on_reconnected_local(&[other, mine], "/home/u".into());
        assert_eq!(app.sessions.len(), 1);
        assert!(!app.quit);
        let reqs: Vec<_> = app.take_effects();
        assert!(reqs
            .iter()
            .any(|e| matches!(e, Effect::Request { msg: Msg::Attach { id, .. }, .. } if *id == ours)));
    }

    #[test]
    fn a_daemon_that_lost_the_session_ends_the_ui() {
        let (mut app, _) = plain_app(&[]);
        app.take_effects();
        app.on_reconnected_local(&[], "/home/u".into());
        assert!(app.quit);
        assert_eq!(app.exit.code, 1);
    }

    #[test]
    fn exit_status_becomes_the_process_code() {
        let ok = Exit { code: 7, message: None };
        assert_eq!(format!("{:?}", ok.exit_code()), format!("{:?}", ExitCode::from(7)));
        let odd = Exit { code: -1, message: None };
        assert_eq!(format!("{:?}", odd.exit_code()), format!("{:?}", ExitCode::from(1)));
    }
}
