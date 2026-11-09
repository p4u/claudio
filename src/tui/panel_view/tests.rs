//! The panel view's tests: its keys (the stats popup's, generic), stale
//! answers, and its glue in `App` (Alt+s opens the Vocdoni panel about
//! the active session; an action asks first).

use super::*;
use crate::addon::{Block, Level, Page};
use crate::tui::confirm::Choice;
use crate::tui::test_support::{alt, app_with, info, press};

const DECL: PanelDecl = PanelDecl {
    id: "proxy",
    title: "Proxy stats",
    key: None,
    windows: &["24h", "7d", "30d"],
};

fn panel(pages: usize, actions: Vec<PanelAction>) -> Panel {
    Panel {
        title: "Proxy stats: work".into(),
        pages: (0..pages)
            .map(|i| Page {
                title: format!("P{i}"),
                blocks: vec![Block::Text {
                    text: "x".into(),
                    tone: None,
                }],
            })
            .collect(),
        actions,
        updated: Some(1),
    }
}

fn switch() -> PanelAction {
    PanelAction {
        id: "switch".into(),
        label: "move to another subscription".into(),
        key: Some('n'),
        confirm: Some("Move this conversation to another subscription?".into()),
    }
}

fn k(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
fn keys_page_window_scroll_and_close() {
    let mut v = PanelView::new("vocdoni", DECL, None, 1);
    assert_eq!(v.req.window.as_deref(), Some("24h"));
    assert!(v.loading);
    assert!(
        !v.answered(0, panel(5, Vec::new())),
        "a stale answer is dropped"
    );
    assert!(v.answered(1, panel(5, Vec::new())));
    assert!(!v.loading);
    // Pages: next, previous (wrapping), a digit; each asks again.
    assert_eq!(v.on_key(&k(KeyCode::Right)), PanelOutcome::Request);
    assert_eq!(v.req.page, 1);
    v.on_key(&k(KeyCode::Char('h')));
    v.on_key(&k(KeyCode::BackTab));
    assert_eq!(v.req.page, 4, "wraps");
    v.on_key(&k(KeyCode::Char('3')));
    assert_eq!(v.req.page, 2);
    assert_eq!(v.on_key(&k(KeyCode::Char('9'))), PanelOutcome::Nothing);
    // Windows cycle; `r` asks again.
    assert_eq!(v.on_key(&k(KeyCode::Char('w'))), PanelOutcome::Request);
    assert_eq!(v.req.window.as_deref(), Some("7d"));
    v.on_key(&k(KeyCode::Char('w')));
    v.on_key(&k(KeyCode::Char('w')));
    assert_eq!(v.req.window.as_deref(), Some("24h"));
    assert_eq!(v.on_key(&k(KeyCode::Char('r'))), PanelOutcome::Request);
    // Scrolling is clamped by what the renderer measured.
    v.set_max_scroll(3);
    for _ in 0..5 {
        v.on_key(&k(KeyCode::Char('j')));
    }
    assert_eq!(v.scroll(), 3);
    v.on_key(&k(KeyCode::Char('k')));
    assert_eq!(v.scroll(), 2);
    v.on_key(&k(KeyCode::Char('G')));
    assert_eq!(v.scroll(), 3);
    v.on_key(&k(KeyCode::Char('g')));
    assert_eq!(v.scroll(), 0);
    // Chords are not the panel's; Esc and q close.
    let alt_q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::ALT);
    assert_eq!(v.on_key(&alt_q), PanelOutcome::Nothing);
    assert_eq!(v.on_key(&k(KeyCode::Char('q'))), PanelOutcome::Close);
    assert_eq!(v.on_key(&k(KeyCode::Esc)), PanelOutcome::Close);
    // An action's key, only when the panel offers it.
    assert_eq!(v.on_key(&k(KeyCode::Char('n'))), PanelOutcome::Nothing);
    v.answered(1, panel(5, vec![switch()]));
    assert_eq!(
        v.on_key(&k(KeyCode::Char('n'))),
        PanelOutcome::Action(switch())
    );
}

fn calls(app: &mut App) -> Vec<AddonCall> {
    let effects = app.take_effects();
    effects
        .into_iter()
        .filter_map(|e| match e {
            Effect::Addon(call) => Some(call),
            _ => None,
        })
        .collect()
}

#[test]
fn alt_s_opens_the_vocdoni_panel_about_the_active_session() {
    let mut app = app_with(&[info(Some(7), None)]);
    let sid = app.sessions[0].id;
    app.on_terminal(alt('s'));
    let call = calls(&mut app);
    let [AddonCall::Panel { gen, addon, req }] = &call[..] else {
        panic!("{call:?}")
    };
    assert_eq!(
        (*addon, req.id.as_str(), req.sid),
        ("vocdoni", "proxy", Some(sid))
    );
    let gen = *gen;
    assert!(matches!(&app.modal, Some(Modal::Panel(v)) if v.loading));
    app.on_panel(gen, panel(5, Vec::new()));
    assert!(matches!(&app.modal, Some(Modal::Panel(v)) if !v.loading));
    // A page change asks again under a new generation; the old answer is dropped.
    app.on_terminal(press(KeyCode::Char('2')));
    let call = calls(&mut app);
    let [AddonCall::Panel {
        gen: newer, req, ..
    }] = &call[..]
    else {
        panic!("{call:?}")
    };
    assert!(*newer > gen && req.page == 1);
    app.on_panel(gen, panel(2, Vec::new()));
    assert!(matches!(&app.modal, Some(Modal::Panel(v)) if v.loading));
    app.on_panel(*newer, panel(2, Vec::new()));
    let pages = match &app.modal {
        Some(Modal::Panel(v)) if !v.loading => v.pages(),
        _ => 0,
    };
    assert_eq!(pages, 2);
    // Its key closes it again.
    app.on_terminal(alt('s'));
    assert!(app.modal.is_none());
}

#[test]
fn an_action_asks_its_confirm_then_runs_and_its_notice_shows() {
    let mut app = app_with(&[info(Some(7), None)]);
    let sid = app.sessions[0].id;
    app.on_terminal(alt('s'));
    let Some(Modal::Panel(v)) = &app.modal else {
        panic!("no panel")
    };
    let gen = v.gen;
    app.on_panel(gen, panel(1, vec![switch()]));
    calls(&mut app);
    app.on_terminal(press(KeyCode::Char('n')));
    match &app.modal {
        Some(Modal::Confirm(p)) => {
            assert_eq!(p.text, "Move this conversation to another subscription?");
            let hints: Vec<_> = p.choices.iter().map(Choice::hint).collect();
            assert_eq!(hints, ["[y] yes", "[n] cancel"]);
        }
        _ => panic!("expected the confirm prompt"),
    }
    // Cancelled: nothing runs.
    app.on_terminal(press(KeyCode::Char('n')));
    assert!(app.modal.is_none() && calls(&mut app).is_empty());
    // Confirmed: the action runs about the panel's session.
    app.on_terminal(alt('s'));
    let gen = match &app.modal {
        Some(Modal::Panel(v)) => v.gen,
        _ => panic!("no panel"),
    };
    app.on_panel(gen, panel(1, vec![switch()]));
    calls(&mut app);
    app.on_terminal(press(KeyCode::Char('n')));
    app.on_terminal(press(KeyCode::Char('y')));
    let run = ActionRequest {
        panel: "proxy".into(),
        action: "switch".into(),
        sid: Some(sid),
    };
    let addon = "vocdoni";
    assert_eq!(calls(&mut app), [AddonCall::Action { addon, req: run }]);
    let notice = Notice {
        text: "switch scheduled: a → b (on the next message)".into(),
        level: Level::Info,
    };
    app.on_action_done(Ok(notice));
    assert_eq!(
        app.notice.as_ref().unwrap().text,
        "switch scheduled: a → b (on the next message)"
    );
    app.on_action_done(Err("vocdoni: switch failed: 429".into()));
    assert_eq!(
        app.notice.as_ref().unwrap().text,
        "vocdoni: switch failed: 429"
    );
}
