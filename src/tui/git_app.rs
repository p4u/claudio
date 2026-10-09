//! The history viewer's glue to [`App`]: opening it on the active session,
//! turning its outcomes into daemon requests, and routing replies back.
//!
//! The state machine is in `git_view`; like the wizard, it never sees the
//! `App`. A request leaves tagged with the view's generation and its own
//! sequence number ([`ReplyTo::Git`]); a reply for a view that was closed or
//! reopened in the meantime finds the generation changed and is dropped.

use std::io;

use crate::proto::Msg;

use super::app::{App, ReplyTo};
use super::git_view::{GitOutcome, GitView};
use super::interaction::Modal;

impl App {
    /// Alt+l: open the history of the active session's directory.
    pub(super) fn open_git_log(&mut self) {
        let Some(view) = self.active_view() else {
            self.notify("no session to show the git history of");
            return;
        };
        let (view, first) = GitView::open(view.host.clone(), view.cwd.clone());
        self.modal = Some(Modal::Git(Box::new(view)));
        self.git_outcome(first);
    }

    /// Carry out what the open view asked for.
    pub(super) fn git_outcome(&mut self, outcome: GitOutcome) {
        match outcome {
            GitOutcome::None => {}
            GitOutcome::Close => self.modal = None,
            GitOutcome::Request { seq, msg } => {
                if let Some(Modal::Git(view)) = &self.modal {
                    let (host, gen) = (view.host.clone(), view.gen);
                    self.request(&host, msg, ReplyTo::Git { gen, seq });
                }
            }
        }
        self.redraw = true;
    }

    /// The reply (or failure) to a request made by the view of generation `gen`.
    pub(super) fn on_git_reply(&mut self, gen: u64, seq: u64, reply: io::Result<Msg>) {
        let Some(Modal::Git(view)) = &mut self.modal else {
            return;
        };
        if view.gen != gen {
            return;
        }
        if let Some(notice) = reply
            .as_ref()
            .err()
            .and_then(|e| App::older_daemon_notice(&view.host, e))
        {
            self.modal = None;
            self.notify(notice);
            return;
        }
        let reply = reply.map_err(|e| e.to_string());
        let outcome = view.on_reply(seq, reply);
        self.git_outcome(outcome);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::Incoming;
    use crate::proto::{GitLogEntry, GitLogPage, SessionInfo, SessionState};
    use crate::tui::app::Effect;
    use crate::tui::state::ClientState;
    use crossterm::event::{
        Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use uuid::Uuid;

    fn session(cwd: &str) -> SessionInfo {
        SessionInfo {
            id: Uuid::new_v4(),
            cwd: cwd.into(),
            name: None,
            state: SessionState::Idle,
            claude_session_id: None,
            title: None,
            pid: Some(1),
            created_at: 1,
            branch: None,
            model: None,
            context_tokens: None,
            kind: crate::proto::SessionKind::Claude,
        }
    }

    fn app(sessions: &[SessionInfo]) -> App {
        let mut app = App::new(100, 30, "/home/u".into(), vec![]);
        app.proxy_default = None;
        app.proxy_profiles = Vec::new();
        app.recover(&ClientState::default(), sessions);
        app.modal = None;
        app.take_effects();
        app
    }

    fn alt(c: char) -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT))
    }

    fn plain(c: char) -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
    }

    fn id(n: usize) -> String {
        format!("{n:040x}")
    }

    fn page(n: usize) -> Msg {
        let commits = (0..n)
            .map(|i| GitLogEntry {
                id: id(i),
                parents: vec![],
                author: "Ann".into(),
                time: 1,
                refs: vec![],
                subject: format!("commit {i}"),
            })
            .collect();
        Msg::GitLogPage(GitLogPage {
            root: "/srv/app".into(),
            head: Some("main".into()),
            commits,
            more: false,
        })
    }

    /// The `GitLog` request Alt+l queued, and the tag its reply must carry.
    fn first_request(app: &mut App) -> (String, Msg, ReplyTo) {
        let effects = app.take_effects();
        match effects
            .into_iter()
            .find(|e| matches!(e, Effect::Request { .. }))
        {
            Some(Effect::Request { host, msg, to }) => (host, msg, to),
            _ => panic!("no request queued"),
        }
    }

    fn git_view(app: &App) -> &GitView {
        match &app.modal {
            Some(Modal::Git(view)) => view,
            _ => panic!("the git view is not open"),
        }
    }

    #[test]
    fn alt_l_opens_the_history_of_the_active_session() {
        let mut app = app(&[session("/srv/app")]);
        app.on_terminal(alt('l'));
        let (host, msg, to) = first_request(&mut app);
        assert_eq!(host, "local");
        assert!(matches!(&msg, Msg::GitLog { cwd, all: false, skip: 0, .. } if cwd == "/srv/app"));
        let gen = git_view(&app).gen;
        assert!(matches!(to, ReplyTo::Git { gen: g, .. } if g == gen));
    }

    #[test]
    fn alt_l_without_a_session_says_so() {
        let mut app = app(&[]);
        app.on_terminal(alt('l'));
        assert!(app.modal.is_none());
        assert!(app.notice.as_ref().unwrap().text.contains("no session"));
    }

    #[test]
    fn replies_fill_the_view_and_stale_ones_are_dropped() {
        let mut app = app(&[session("/srv/app")]);
        app.on_terminal(alt('l'));
        let (_, _, to) = first_request(&mut app);
        let ReplyTo::Git { gen, seq } = to else {
            panic!()
        };
        // A reply for another generation does nothing.
        app.on_reply(ReplyTo::Git { gen: gen + 1, seq }, Ok(page(3)));
        assert!(git_view(&app).log.commits.is_empty());
        app.on_reply(ReplyTo::Git { gen, seq }, Ok(page(3)));
        assert_eq!(git_view(&app).log.commits.len(), 3);
        // After the view is closed, a late reply is ignored (and harmless).
        app.on_terminal(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
        assert!(app.modal.is_none());
        app.on_reply(ReplyTo::Git { gen, seq }, Ok(page(3)));
        assert!(app.modal.is_none());
        // Reopening makes a new generation: the old tag no longer matches.
        app.on_terminal(alt('l'));
        app.on_reply(ReplyTo::Git { gen, seq }, Ok(page(3)));
        assert!(git_view(&app).log.commits.is_empty());
    }

    #[test]
    fn keys_in_the_view_make_requests_for_the_sessions_host() {
        let mut app = app(&[session("/srv/app")]);
        app.on_terminal(alt('l'));
        let (_, _, to) = first_request(&mut app);
        let ReplyTo::Git { gen, seq } = to else {
            panic!()
        };
        app.on_reply(ReplyTo::Git { gen, seq }, Ok(page(3)));
        app.on_terminal(plain('j'));
        app.on_terminal(Event::Key(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE,
        )));
        let (host, msg, to) = first_request(&mut app);
        assert_eq!(host, "local");
        assert_eq!(
            msg,
            Msg::GitCommit {
                cwd: "/srv/app".into(),
                id: id(1)
            }
        );
        assert!(matches!(to, ReplyTo::Git { gen: g, .. } if g == gen));
        // Nothing reached the session's terminal.
        assert!(app
            .take_effects()
            .iter()
            .all(|e| !matches!(e, Effect::Input(..))));
    }

    #[test]
    fn an_older_daemon_closes_the_view_with_a_notice() {
        let mut app = app(&[session("/srv/app")]);
        app.on_terminal(alt('l'));
        let (_, _, to) = first_request(&mut app);
        app.on_reply(to, Err(io::Error::other("unsupported op: unknown")));
        assert!(app.modal.is_none());
        let text = &app.notice.as_ref().unwrap().text;
        assert_eq!(
            text,
            "daemon on local is older: run `claudio daemon restart`"
        );
    }

    #[test]
    fn git_errors_stay_inside_the_view() {
        let mut app = app(&[session("/srv/app")]);
        app.on_terminal(alt('l'));
        let (_, _, to) = first_request(&mut app);
        app.on_reply(to, Err(io::Error::other("not a git repository")));
        assert_eq!(
            git_view(&app).log.error.as_deref(),
            Some("not a git repository")
        );
    }

    #[test]
    fn manager_actions_close_the_view_and_run() {
        let live = [session("/a"), session("/b")];
        let mut app = app(&live);
        app.on_terminal(alt('l'));
        app.take_effects();
        // Alt+→ switches tabs and closes the view.
        app.on_terminal(Event::Key(KeyEvent::new(KeyCode::Right, KeyModifiers::ALT)));
        assert!(app.modal.is_none());
        assert_eq!(app.active, Some(1));
        // Another action (overview) replaces the view.
        app.on_terminal(alt('l'));
        app.on_terminal(alt('g'));
        assert!(matches!(app.modal, Some(Modal::Overview { .. })));
    }

    #[test]
    fn the_key_that_opened_a_modal_closes_it() {
        let mut app = app(&[session("/a")]);
        app.on_terminal(alt('l'));
        app.on_terminal(alt('l'));
        assert!(app.modal.is_none());
        app.on_terminal(alt('h'));
        assert!(matches!(app.modal, Some(Modal::Help)));
        app.on_terminal(alt('h'));
        assert!(app.modal.is_none());
    }

    #[test]
    fn quit_still_works_with_the_view_open() {
        let mut app = app(&[session("/a")]);
        app.on_terminal(alt('l'));
        app.on_terminal(alt('q'));
        assert!(app.quit);
    }

    #[test]
    fn text_input_modals_keep_swallowing_manager_keys() {
        let mut app = app(&[session("/a")]);
        app.on_terminal(alt('r'));
        app.on_terminal(alt('l'));
        assert!(matches!(app.modal, Some(Modal::Rename { .. })));
        app.on_terminal(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
        app.on_terminal(alt('x'));
        app.on_terminal(alt('g'));
        assert!(matches!(app.modal, Some(Modal::Confirm(_))));
    }

    #[test]
    fn the_mouse_goes_to_the_view_not_the_session() {
        let mut app = app(&[session("/a")]);
        app.on_terminal(alt('l'));
        let (_, _, to) = first_request(&mut app);
        let ReplyTo::Git { gen, seq } = to else {
            panic!()
        };
        app.on_reply(ReplyTo::Git { gen, seq }, Ok(page(30)));
        let mouse = |kind, row| {
            Event::Mouse(MouseEvent {
                kind,
                column: 5,
                row,
                modifiers: KeyModifiers::NONE,
            })
        };
        app.on_terminal(mouse(MouseEventKind::ScrollDown, 5));
        assert_eq!(git_view(&app).log.cursor.selected, 3);
        // A click on the third commit row (screen row 3 = pane row 2) opens it.
        app.on_terminal(mouse(MouseEventKind::Down(MouseButton::Left), 3));
        let (_, msg, _) = first_request(&mut app);
        assert!(matches!(msg, Msg::GitCommit { id: i, .. } if i == id(1)));
        assert!(app
            .take_effects()
            .iter()
            .all(|e| !matches!(e, Effect::Input(..))));
    }

    #[test]
    fn disconnects_do_not_panic_the_open_view() {
        let mut app = app(&[session("/a")]);
        app.on_terminal(alt('l'));
        app.on_incoming_from("local", Incoming::Disconnected);
        assert!(app.modal.is_some());
    }
}
