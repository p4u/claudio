//! The reader view (Alt+u) over the session pane; its state and lines are
//! in `tui/reader/`.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::Clear;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::claude::latex::sanitize;
use crate::tui::app::App;
use crate::tui::latex::text_cells;
use crate::tui::reader::layout::{safe, Line};
use crate::tui::reader::ReaderView;

/// Draw the view over `pane` (the tab and status bars stay): its header on
/// the first row, then the body from where `view` is scrolled. Every line
/// is written cell by cell, sanitized again, and cut at the pane's edge.
pub fn draw_reader(frame: &mut Frame, app: &App, view: &ReaderView, pane: Rect) {
    frame.render_widget(Clear, pane);
    if pane.height == 0 {
        return;
    }
    let width = usize::from(pane.width);
    let buf = frame.buffer_mut();
    let header = view.header(width);
    let bar = Style::default().add_modifier(Modifier::REVERSED);
    for x in pane.left()..pane.right() {
        buf[(x, pane.y)].set_style(bar);
    }
    draw_line(buf, pane, pane.y, &header, bar);
    let session = app.sessions.iter().find(|v| v.id == view.session);
    let lines = view.body(session, width);
    let rows = pane.height - 1;
    let total = u16::try_from(lines.len()).unwrap_or(u16::MAX);
    view.set_max_scroll(total.saturating_sub(rows));
    let shown = lines
        .iter()
        .skip(usize::from(view.scroll()))
        .take(usize::from(rows));
    for (y, line) in (pane.y + 1..).zip(shown) {
        draw_line(buf, pane, y, line, Style::default());
    }
}

/// Write `line` on row `y` of `area`, each piece's style over `base`; a tab is one space.
fn draw_line(buf: &mut Buffer, area: Rect, y: u16, line: &Line, base: Style) {
    let mut cells = Vec::new();
    for seg in &line.0 {
        let text = safe(&sanitize(&seg.text)).replace('\t', " ");
        cells.extend(text_cells(&text, base.patch(seg.style)).unwrap_or_default());
    }
    cells.truncate(usize::from(area.width));
    // A wide character cut in half at the edge goes.
    if cells.last().is_some_and(|c| c.symbol().width() > 1) {
        cells.pop();
    }
    for (x, cell) in (area.x..).zip(cells) {
        buf[(x, y)] = cell;
    }
}

#[cfg(test)]
mod tests {
    use crate::proto::{Answer, Msg};
    use crate::tui::app::{App, AppConfig, Effect, Modal, Mode, ReplyTo};
    use crate::tui::test_support::{self, alt, app_with, info, press};
    use crate::tui::{render_answers, RenderCache};
    use crossterm::event::KeyCode;
    use ratatui::{backend::TestBackend, Terminal};
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;

    /// The screen's rows after drawing `app`, trailing blanks dropped.
    fn screen(app: &App) -> Vec<String> {
        let mut term = Terminal::new(TestBackend::new(app.width, app.height)).unwrap();
        term.draw(|f| super::super::draw(f, app)).unwrap();
        let buf = term.backend().buffer();
        (0..buf.area.height)
            .map(|y| {
                let row: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect();
                row.trim_end().to_owned()
            })
            .collect()
    }

    /// Open the reader on `app`'s first session and answer it with
    /// `answers`, rendered as the worker would (benign formulas only).
    fn open_with(app: &mut App, answers: Vec<Answer>) {
        app.on_terminal(alt('u'));
        let (gen, id) = match app.take_effects().as_slice() {
            [Effect::Request {
                msg: Msg::Answers { id },
                to: ReplyTo::Answers { gen, .. },
                ..
            }] => (*gen, *id),
            other => panic!("{other:?}"),
        };
        let reply = Msg::AnswersReply { id, answers };
        app.on_reply(ReplyTo::Answers { gen, id }, Ok(reply));
        let answers = match app.take_effects().pop() {
            Some(Effect::RenderAnswers { answers, .. }) => answers,
            other => panic!("{other:?}"),
        };
        let running = Arc::new(AtomicUsize::new(0));
        let rendered = render_answers(&answers, &mut RenderCache::default(), &running);
        app.on_answers_rendered(gen, id, rendered);
    }

    fn answer(prompt: &str, text: &str) -> Answer {
        Answer {
            prompt: prompt.into(),
            text: text.into(),
        }
    }

    #[test]
    fn the_reader_covers_the_pane_and_typesets_the_answer() {
        let mut app = app_with(&[info(Some(1), None)]);
        let text = "Energy is <latex>E = mc^2</latex>, and\n\n<latex>\\frac{a}{b}</latex>";
        open_with(&mut app, vec![answer("old", "first"), answer("why?", text)]);
        let rows = screen(&app);
        assert_eq!(rows[0], " 1 ✓ app", "the tab bar stays");
        assert_eq!(rows[1], "answer 2/2 · why?");
        assert_eq!(
            rows[2..7],
            ["Energy is E = mc², and", "", "  a", "  ─", "  b"]
        );
        assert!(rows[28].starts_with(" /srv/app"), "the status bar stays");
        assert!(rows[29].ends_with("Alt+h help"), "{}", rows[29]);
        // ← shows the older answer.
        app.on_terminal(press(KeyCode::Left));
        assert_eq!(screen(&app)[1..3], ["answer 1/2 · old", "first"]);
    }

    #[test]
    fn tab_shows_the_formulas_list_and_how_to_get_formulas() {
        let mut app = app_with(&[info(Some(1), None)]);
        open_with(&mut app, vec![answer("p", "text")]);
        app.on_terminal(press(KeyCode::Tab));
        let text = screen(&app).join("\n");
        assert!(text.contains("formulas"), "{text}");
        assert!(text.contains("No formulas yet."), "{text}");
        assert!(text.contains("claudio daemon restart"), "{text}");
    }

    #[test]
    fn an_old_daemon_gets_the_restart_notice_in_the_view() {
        let mut app = app_with(&[info(Some(1), None)]);
        app.on_terminal(alt('u'));
        assert_eq!(screen(&app)[1..3], ["reader", "Loading…"]);
        let Some(Modal::Reader(view)) = &app.modal else {
            panic!("no reader")
        };
        let (gen, id) = (view.gen, view.session);
        let old = std::io::Error::other("unsupported op: answers");
        app.on_reply(ReplyTo::Answers { gen, id }, Err(old));
        assert_eq!(
            screen(&app)[2],
            "daemon on local is older: run `claudio daemon restart`"
        );
    }

    #[test]
    fn in_plain_mode_the_reader_fills_the_screen() {
        let mut app = App::new(AppConfig {
            mode: Mode::Plain,
            ..test_support::config()
        });
        app.recover(&Default::default(), &[info(Some(1), None)]);
        app.take_effects();
        app.activate(0);
        app.take_effects();
        open_with(&mut app, vec![answer("q", "plain text")]);
        assert_eq!(screen(&app)[..2], ["answer 1/1 · q", "plain text"]);
    }
}
