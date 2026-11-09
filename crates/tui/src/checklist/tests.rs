//! The checklist view's tests: its keys, arming, Esc by origin, a new
//! revision, and its glue in `App` (the queue, offers, a launch's review,
//! the extensions screen's rows).

use super::*;
use crate::addon_calls::AddonResult;
use crate::confirm::{Choice, ConfirmPrompt};
use crate::test_support::{alt, app_with, info, press};
use claudio_core::addon::ChecklistItem;

fn item(id: &str, on: bool) -> ChecklistItem {
    ChecklistItem {
        id: id.into(),
        label: id.into(),
        detail: None,
        section: Some("Items".into()),
        on,
    }
}

fn checklist(revision: &str, ids: &[&str]) -> Checklist {
    Checklist {
        id: "team".into(),
        revision: revision.into(),
        title: "Set up team 1.0.0".into(),
        intro: String::new(),
        items: ids.iter().map(|id| item(id, true)).collect(),
    }
}

fn k(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn launch_origin() -> Origin {
    Origin::Launch {
        id: 7,
        sid: uuid::Uuid::from_u128(7),
    }
}

/// An app with no modal open.
fn idle_app() -> App {
    let mut app = app_with(&[info(Some(7), None)]);
    app.modal = None;
    app
}

fn open(app: &App) -> &ChecklistView {
    match &app.modal {
        Some(Modal::Checklist(v)) => v,
        other => panic!("no checklist: {}", other.is_some()),
    }
}

fn arm(app: &mut App) {
    for _ in 0..ARM_TICKS {
        app.confirm_tick();
    }
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

fn show(app: &mut App, origin: Origin, c: Checklist) {
    app.on_addon(AddonResult::Checklist {
        addon: "team",
        origin,
        checklist: c,
    });
}

#[test]
fn keys_move_toggle_and_answer() {
    let c = checklist("1", &["a", "b", "c"]);
    let mut v = ChecklistView::new(
        "team",
        c,
        Origin::Row {
            back: Box::default(),
        },
    );
    assert_eq!(v.armed, 0, "opened by the member: keys count at once");
    for code in [KeyCode::Down, KeyCode::Char('j'), KeyCode::Char('j')] {
        assert_eq!(v.on_key(&k(code)), Outcome::Stay);
    }
    assert_eq!(v.selected, 2, "the last item");
    v.on_key(&k(KeyCode::Char('k')));
    v.on_key(&k(KeyCode::Char(' ')));
    assert_eq!(v.off_ids(), ["b"]);
    v.on_key(&k(KeyCode::Char('n')));
    assert_eq!(v.off_ids(), ["a", "b", "c"]);
    v.on_key(&k(KeyCode::Char('a')));
    assert!(v.off_ids().is_empty());
    v.on_key(&k(KeyCode::Up));
    v.on_key(&k(KeyCode::Up));
    assert_eq!(v.selected, 0);
    // A chord is no answer; `s` skips only a launch's review.
    let alt_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT);
    assert_eq!(v.on_key(&alt_enter), Outcome::Stay);
    assert_eq!(v.on_key(&k(KeyCode::Char('s'))), Outcome::Stay);
    assert_eq!(v.on_key(&k(KeyCode::Enter)), Outcome::Save);
    assert_eq!(v.on_key(&k(KeyCode::Esc)), Outcome::Esc);
    assert_eq!(
        v.answer(),
        ChecklistAnswer::Save {
            revision: "1".into(),
            off: Vec::new()
        }
    );
    assert_eq!(
        v.hint(),
        "Space toggle · a all on · n all off · Enter save · Esc cancel"
    );
    // One the member did not open counts keys once armed.
    let mut review = ChecklistView::new("team", checklist("1", &["a"]), launch_origin());
    assert_eq!(review.on_key(&k(KeyCode::Char('s'))), Outcome::Stay);
    review.armed = 0;
    assert_eq!(review.on_key(&k(KeyCode::Char('s'))), Outcome::Skip);
    assert_eq!(
        review.hint(),
        "Space toggle · a all on · n all off · Enter save · s skip · Esc cancel"
    );
    let offer = ChecklistView::new("team", checklist("1", &["a"]), Origin::Offer);
    assert_eq!(offer.armed, ARM_TICKS);
    assert_eq!(
        offer.hint(),
        "Space toggle · a all on · n all off · Enter save · Esc later"
    );
}

/// The revision reopen: a new revision while open keeps the toggles of
/// the ids still there; a new id is as the addon has it.
#[test]
fn a_new_revision_replaces_the_items_keeping_the_toggles() {
    let mut v = ChecklistView::new("team", checklist("1", &["a", "b", "c"]), Origin::Offer);
    v.checklist.items[0].on = false;
    v.selected = 2;
    let mut next = checklist("2", &["a", "d"]);
    next.items[1].on = false;
    v.replace(next);
    assert_eq!(v.checklist.revision, "2");
    assert_eq!(v.off_ids(), ["a", "d"]);
    assert_eq!(v.selected, 1, "kept within the items");
    assert_eq!(v.armed, ARM_TICKS);
    // Through the app: the open view and a queued one of it are refreshed.
    let mut app = idle_app();
    show(&mut app, Origin::Offer, checklist("1", &["a", "b"]));
    arm(&mut app);
    app.on_terminal(press(KeyCode::Char(' ')));
    assert_eq!(app.checklists_shown(), [("team", "team".to_owned())]);
    app.refresh_checklist("team", &checklist("2", &["a", "b", "x"]));
    let v = open(&app);
    assert_eq!(v.checklist.revision, "2");
    assert_eq!(v.off_ids(), ["a"]);
    app.on_terminal(press(KeyCode::Enter));
    let saved = AddonCall::SaveChecklist {
        addon: "team",
        id: "team".into(),
        answer: ChecklistAnswer::Save {
            revision: "2".into(),
            off: vec!["a".into()],
        },
        origin: Origin::Offer,
    };
    assert_eq!(calls(&mut app), [saved]);
}

#[test]
fn an_offer_waits_its_turn_and_esc_puts_it_off() {
    let mut app = idle_app();
    app.queue_confirm(ConfirmPrompt::new(
        "Q",
        "Update?",
        vec![Choice::new('n', "no", None)],
    ));
    app.on_addon(AddonResult::Offer {
        addon: "team",
        id: "team".into(),
    });
    let ask = AddonCall::OpenChecklist {
        addon: "team",
        id: "team".into(),
        origin: Origin::Offer,
    };
    assert_eq!(calls(&mut app), [ask]);
    show(&mut app, Origin::Offer, checklist("1", &["a"]));
    assert!(
        matches!(app.modal, Some(Modal::Confirm(_))),
        "the prompt first"
    );
    // Offered again while queued: still one.
    app.on_addon(AddonResult::Offer {
        addon: "team",
        id: "team".into(),
    });
    assert!(calls(&mut app).is_empty());
    show(&mut app, Origin::Offer, checklist("1", &["a"]));
    assert_eq!(app.checklists_shown().len(), 1);
    arm(&mut app);
    app.on_terminal(press(KeyCode::Char('n')));
    assert_eq!(open(&app).checklist.id, "team");
    // Keys are swallowed while it arms, Alt+n included.
    app.on_terminal(press(KeyCode::Esc));
    app.on_terminal(alt('n'));
    assert!(matches!(app.modal, Some(Modal::Checklist(_))));
    arm(&mut app);
    app.on_terminal(press(KeyCode::Esc));
    assert!(app.modal.is_none());
    let defer = AddonCall::SaveChecklist {
        addon: "team",
        id: "team".into(),
        answer: ChecklistAnswer::Defer,
        origin: Origin::Offer,
    };
    assert_eq!(calls(&mut app), [defer]);
}

#[test]
fn a_launchs_review_saves_skips_or_cancels() {
    for (key, answer) in [
        (KeyCode::Enter, None),
        (KeyCode::Char('s'), Some(ReviewAnswer::Skip)),
    ] {
        let mut app = idle_app();
        show(&mut app, launch_origin(), checklist("1", &["a", "b"]));
        arm(&mut app);
        app.on_terminal(press(KeyCode::Down));
        app.on_terminal(press(KeyCode::Char(' ')));
        app.on_terminal(press(key));
        assert!(app.modal.is_none());
        let answer = answer.unwrap_or(ReviewAnswer::Save(ChecklistAnswer::Save {
            revision: "1".into(),
            off: vec!["b".into()],
        }));
        assert_eq!(
            calls(&mut app),
            [AddonCall::ReviewAnswer { launch: 7, answer }]
        );
        assert_eq!(app.sessions.len(), 1, "the tab stays");
    }
    // Esc cancels the launch and closes the tab it was to fill.
    let mut app = idle_app();
    let sid = app.sessions[0].id;
    let origin = Origin::Launch { id: 7, sid };
    show(&mut app, origin, checklist("1", &["a"]));
    arm(&mut app);
    app.on_terminal(press(KeyCode::Esc));
    let cancel = AddonCall::ReviewAnswer {
        launch: 7,
        answer: ReviewAnswer::Cancel,
    };
    assert_eq!(calls(&mut app), [cancel]);
    assert!(app.index_of(sid).is_none(), "its tab is gone");
    assert_eq!(app.notice.as_ref().unwrap().text, "launch cancelled");
}

/// A launch's review of a checklist queued as an offer takes the view
/// over: one view, answered to the launch.
#[test]
fn a_review_takes_over_the_offer_of_the_same_checklist() {
    let mut app = idle_app();
    app.queue_confirm(ConfirmPrompt::new(
        "Q",
        "?",
        vec![Choice::new('n', "no", None)],
    ));
    show(&mut app, Origin::Offer, checklist("1", &["a"]));
    show(&mut app, launch_origin(), checklist("2", &["a", "b"]));
    assert_eq!(app.checklists_shown().len(), 1);
    arm(&mut app);
    app.on_terminal(press(KeyCode::Char('n')));
    let v = open(&app);
    assert_eq!(
        (v.origin.clone(), v.checklist.revision.as_str()),
        (launch_origin(), "2")
    );
    // Two launches reviewing the same checklist each get their view.
    show(
        &mut app,
        Origin::Launch {
            id: 8,
            sid: uuid::Uuid::from_u128(8),
        },
        checklist("2", &["a"]),
    );
    assert_eq!(app.checklists_shown().len(), 2);
}

#[test]
fn enter_on_a_plugin_row_opens_its_checklist_and_esc_goes_back() {
    let mut app = idle_app();
    crate::test_support::rows(&mut app, "team", &[("team", None)], &[]);
    let row = app
        .extensions
        .extensions
        .iter()
        .position(|e| e.name == "team")
        .unwrap();
    app.on_terminal(alt('i'));
    for _ in 0..row {
        app.on_terminal(press(KeyCode::Down));
    }
    app.take_effects();
    app.on_terminal(press(KeyCode::Enter));
    let call = calls(&mut app);
    let [AddonCall::OpenChecklist { addon, id, origin }] = &call[..] else {
        panic!("{call:?}")
    };
    assert_eq!((*addon, id.as_str()), ("team", "team"));
    let Origin::Row { back } = origin else {
        panic!("{origin:?}")
    };
    assert_eq!(back.selected, row);
    show(&mut app, origin.clone(), checklist("1", &["a"]));
    assert_eq!(open(&app).armed, 0);
    app.on_terminal(press(KeyCode::Char(' ')));
    app.on_terminal(press(KeyCode::Esc));
    assert!(
        matches!(&app.modal, Some(Modal::ExtensionsMenu(v)) if v.selected == row),
        "back on the extensions screen"
    );
    assert!(calls(&mut app).is_empty(), "nothing saved");
    // Enter saves, and goes back too.
    app.on_terminal(press(KeyCode::Enter));
    let call = calls(&mut app);
    let [AddonCall::OpenChecklist { origin, .. }] = &call[..] else {
        panic!("{call:?}")
    };
    show(&mut app, origin.clone(), checklist("1", &["a"]));
    app.on_terminal(press(KeyCode::Enter));
    assert!(matches!(&app.modal, Some(Modal::ExtensionsMenu(_))));
    let call = calls(&mut app);
    assert!(
        matches!(
            &call[..],
            [AddonCall::SaveChecklist {
                answer: ChecklistAnswer::Save { .. },
                ..
            }]
        ),
        "{call:?}"
    );
}
