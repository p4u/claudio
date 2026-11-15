//! The custom env menu: keys, effects and drawing, without I/O.

use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::{backend::TestBackend, Terminal};

use super::*;
use crate::app::Effect;
use crate::interaction::Modal;
use crate::test_support::{alt, app_with, info, key, press, shell_info};
use claudio_core::app::env::Listed;
use claudio_core::proto::{SessionEvent, SessionInfo};

fn screen(app: &App) -> String {
    let mut term = Terminal::new(TestBackend::new(app.width, app.height)).unwrap();
    term.draw(|f| crate::ui::draw(f, app)).unwrap();
    let buf = term.backend().buffer();
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
                + "\n"
        })
        .collect()
}

fn view(app: &App) -> &EnvView {
    match &app.modal {
        Some(Modal::Environment(view)) => view,
        _ => panic!("the env menu is not open"),
    }
}

fn typed(app: &mut App, text: &str) {
    for c in text.chars() {
        app.on_terminal(press(KeyCode::Char(c)));
    }
}

fn env_calls(effects: Vec<Effect>) -> Vec<EnvCall> {
    effects
        .into_iter()
        .filter_map(|e| match e {
            Effect::Env(call) => Some(call),
            _ => None,
        })
        .collect()
}

fn listed(vars: &[(&str, &str)]) -> Listed {
    Listed {
        vars: vars
            .iter()
            .map(|(k, v)| (k.to_string(), Value::new(*v)))
            .collect(),
        ..Listed::default()
    }
}

/// The menu open on `live[0]`, its first read answered with `global` and
/// `own`.
fn opened(live: &[SessionInfo], global: &[(&str, &str)], own: &[(&str, &str)]) -> App {
    let mut app = app_with(live);
    app.on_terminal(alt(','));
    let gen = view(&app).gen;
    app.take_effects();
    let session = view(&app).session.is_ok().then(|| listed(own));
    app.on_env(EnvDone {
        gen,
        global: listed(global),
        session,
        failed: None,
    });
    app
}

#[test]
fn alt_comma_opens_the_menu_on_the_global_tab_and_reads_both_scopes() {
    let live = [info(Some(1), None)];
    let mut app = app_with(&live);
    app.on_terminal(alt(','));
    let calls = env_calls(app.take_effects());
    let want = Scope::Session {
        host: "local".into(),
        sid: live[0].id,
    };
    assert_eq!(
        calls,
        [EnvCall::Read {
            gen: view(&app).gen,
            session: Some(want)
        }]
    );
    assert_eq!(view(&app).tab, Tab::Global);
    assert!(screen(&app).contains("reading…"));
}

#[test]
fn rows_show_escaped_with_the_description_and_the_footer() {
    let live = [info(Some(1), None)];
    let global = [("ANTHROPIC_DEFAULT_OPUS_MODEL", "opus-x"), ("H", "a\nb\\c")];
    let app = opened(&live, &global, &[]);
    let text = screen(&app);
    assert!(
        text.contains("ANTHROPIC_DEFAULT_OPUS_MODEL = opus-x"),
        "{text}"
    );
    assert!(text.contains("H = a\\nb\\\\c"), "{text}");
    assert!(
        text.contains("Model ID that the opus alias resolves to"),
        "{text}"
    );
    assert!(
        text.contains("applies on next start or restart (Alt+e)"),
        "{text}"
    );
}

#[test]
fn adding_a_var_decodes_the_escapes_and_writes_the_open_tab() {
    let live = [info(Some(1), None)];
    let mut app = opened(&live, &[], &[]);
    app.on_terminal(press(KeyCode::Tab));
    assert_eq!(view(&app).tab, Tab::Session);
    app.on_terminal(press(KeyCode::Char('a')));
    typed(&mut app, "ANTHROPIC_DEF");
    assert!(screen(&app).contains("ANTHROPIC_DEFAULT_OPUS_MODEL"));
    typed(&mut app, "AULT_OPUS_MODEL");
    app.on_terminal(press(KeyCode::Enter));
    assert!(screen(&app).contains("\\n newline, \\t tab, \\\\ backslash"));
    typed(&mut app, "C:\\\\new\\tx");
    app.on_terminal(press(KeyCode::Enter));
    let calls = env_calls(app.take_effects());
    let session = Scope::Session {
        host: "local".into(),
        sid: live[0].id,
    };
    assert_eq!(
        calls,
        [EnvCall::Write {
            gen: view(&app).gen,
            scope: session.clone(),
            name: "ANTHROPIC_DEFAULT_OPUS_MODEL".into(),
            value: Some(Value::new("C:\\new\tx")),
            session: Some(session),
        }]
    );
    assert_eq!(view(&app).step, Step::List);
}

#[test]
fn a_bad_escape_or_a_refused_name_writes_nothing_and_says_why() {
    let live = [info(Some(1), None)];
    let mut app = opened(&live, &[], &[]);
    app.on_terminal(press(KeyCode::Char('a')));
    typed(&mut app, "PATH");
    app.on_terminal(press(KeyCode::Enter));
    assert!(matches!(view(&app).step, Step::Name { .. }));
    assert!(screen(&app).contains("PATH: refused: "));
    app.on_terminal(key(KeyCode::Char('u'), KeyModifiers::CONTROL));
    typed(&mut app, "WIN_DIR");
    app.on_terminal(press(KeyCode::Enter));
    typed(&mut app, "C:\\Users");
    app.on_terminal(press(KeyCode::Enter));
    assert!(screen(&app).contains("WIN_DIR: unknown escape \\U: write \\\\ for a backslash"));
    assert!(env_calls(app.take_effects()).is_empty());
    app.on_terminal(press(KeyCode::Esc));
    assert_eq!(view(&app).step, Step::List);
}

#[test]
fn enter_edits_and_d_deletes_after_a_confirm() {
    let live = [info(Some(1), None)];
    let mut app = opened(&live, &[("A", "1"), ("B", "x\ny")], &[]);
    app.on_terminal(press(KeyCode::Down));
    app.on_terminal(press(KeyCode::Enter));
    match &view(&app).step {
        Step::Value { name, input } => {
            assert_eq!((name.as_str(), input.text.as_str()), ("B", "x\\ny"))
        }
        other => panic!("{other:?}"),
    }
    app.on_terminal(press(KeyCode::Esc));
    app.on_terminal(press(KeyCode::Char('d')));
    assert!(screen(&app).contains("delete B? (y/n)"));
    app.on_terminal(press(KeyCode::Char('y')));
    let calls = env_calls(app.take_effects());
    assert!(matches!(
        calls.as_slice(),
        [EnvCall::Write { scope: Scope::Global, name, value: None, .. }] if name == "B"
    ));
}

#[test]
fn a_short_popup_keeps_the_value_hint_off_its_border() {
    let live = [info(Some(1), None)];
    let mut app = opened(&live, &[], &[]);
    app.on_terminal(press(KeyCode::Char('a')));
    typed(&mut app, "MY_VAR");
    app.on_terminal(press(KeyCode::Enter));
    assert!(screen(&app).contains(ESCAPES));
    for height in [3, 8, 9, 10] {
        app.on_terminal(crossterm::event::Event::Resize(60, height));
        let text = screen(&app);
        assert_eq!(text.contains(ESCAPES), height > 10, "{height}:\n{text}");
    }
}

#[test]
fn a_terminal_tab_has_no_session_tab() {
    let live = [shell_info(Some(1))];
    let mut app = opened(&live, &[], &[]);
    assert!(screen(&app).contains("Session: — (a terminal tab runs no claude)"));
    app.on_terminal(press(KeyCode::Tab));
    assert_eq!(view(&app).tab, Tab::Global);
    assert!(screen(&app).contains("no session tab: a terminal tab runs no claude"));
}

#[test]
fn a_stale_read_is_dropped_and_a_failed_write_is_a_notice() {
    let live = [info(Some(1), None)];
    let mut app = opened(&live, &[("A", "1")], &[]);
    let gen = view(&app).gen;
    app.on_env(EnvDone {
        gen: gen + 1,
        global: listed(&[("STALE", "x")]),
        session: None,
        failed: None,
    });
    assert_eq!(view(&app).rows().len(), 1);
    app.on_env(EnvDone {
        gen,
        global: listed(&[("A", "1")]),
        session: Some(listed(&[])),
        failed: Some("A: too many variables (at most 128)".into()),
    });
    assert_eq!(
        app.notice.as_ref().map(|n| n.text.as_str()),
        Some("A: too many variables (at most 128)")
    );
}

#[test]
fn a_closed_tab_and_a_removed_session_forget_their_env_and_a_recovery_prunes() {
    let live = [info(Some(1), None), shell_info(Some(2))];
    let mut app = app_with(&live);
    app.kill_session(live[0].id);
    let calls = env_calls(app.take_effects());
    assert_eq!(
        calls,
        [EnvCall::Forget {
            host: "local".into(),
            sid: live[0].id
        }]
    );
    // A terminal has no custom env.
    app.on_incoming_from(
        "local",
        claudio_core::client::Incoming::Event {
            id: live[1].id,
            event: SessionEvent::Removed,
        },
    );
    assert!(env_calls(app.take_effects()).is_empty());
    let other = info(Some(3), None);
    app.recover_host("local", std::slice::from_ref(&other));
    let calls = env_calls(app.take_effects());
    assert!(calls.contains(&EnvCall::Prune {
        host: "local".into(),
        live: vec![other.id]
    }));
}

#[test]
fn no_value_reaches_debug() {
    let live = [info(Some(1), None)];
    let mut app = opened(&live, &[("TOKEN", "sk-SECRET-1")], &[]);
    app.on_terminal(press(KeyCode::Enter));
    typed(&mut app, "-MORE-SECRET");
    let shown = format!("{:?}", view(&app));
    assert!(!shown.contains("SECRET"), "{shown}");
    app.on_terminal(press(KeyCode::Enter));
    let shown = format!("{:?}", app.take_effects());
    assert!(!shown.contains("SECRET"), "{shown}");
}

#[test]
fn a_pasted_value_keeps_its_line_breaks_whatever_the_terminal_sends() {
    for pasted in [
        "X-One: 1\rX-Two: 2",
        "X-One: 1\nX-Two: 2",
        "X-One: 1\r\nX-Two: 2",
    ] {
        let live = [info(Some(1), None)];
        let mut app = opened(&live, &[], &[]);
        app.on_terminal(press(KeyCode::Char('a')));
        typed(&mut app, "H");
        app.on_terminal(press(KeyCode::Enter));
        app.on_terminal(crossterm::event::Event::Paste(pasted.into()));
        match &view(&app).step {
            Step::Value { input, .. } => {
                assert_eq!(input.text, "X-One: 1\\nX-Two: 2", "{pasted:?}")
            }
            other => panic!("{other:?}"),
        }
    }
}
