//! The reader view (Alt+u) over the session pane; its state and lines are
//! in `tui/reader/`.

use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::Clear;
use ratatui::Frame;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::claude::latex::{is_unsafe, sanitize};
use crate::tui::reader::layout::{safe, Line};
use crate::tui::reader::ReaderView;

/// Draw the view over `pane` (the tab and status bars stay): its header on
/// the first row, then the body from where `view` is scrolled. Every line
/// is written cell by cell, sanitized again, and cut at the pane's edge.
pub fn draw_reader(frame: &mut Frame, view: &ReaderView, pane: Rect) {
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
    let lines = view.body(width);
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

/// `text` as cells in `style`: one grapheme cluster per cell, a wide one
/// followed by the blank cell it covers (as `Buffer::set_string` lays them
/// out). Clusters of width 0, or holding a character [`sanitize`] removes,
/// are dropped; `None` when one is wider than two columns.
fn text_cells(text: &str, style: Style) -> Option<Vec<Cell>> {
    let mut cells = Vec::with_capacity(text.len());
    for cluster in text.graphemes(true) {
        if cluster
            .chars()
            .any(|c| is_unsafe(c) || c == '\n' || c == '\t')
        {
            continue;
        }
        let mut cell = Cell::default();
        cell.set_symbol(cluster).set_style(style);
        match cluster.width() {
            0 => {}
            1 => cells.push(cell),
            2 => cells.extend([cell, Cell::default()]),
            _ => return None,
        }
    }
    Some(cells)
}

#[cfg(test)]
mod tests {
    use super::text_cells;
    use crate::proto::{Answer, Msg};
    use crate::tui::app::{App, AppConfig, Effect, Modal, Mode, ReplyTo};
    use crate::tui::reader::render;
    use crate::tui::test_support::{self, alt, app_with, info, press};
    use crossterm::event::KeyCode;
    use ratatui::buffer::Cell;
    use ratatui::style::Style;
    use ratatui::{backend::TestBackend, Terminal};

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
    /// `answers`, parsed as the event loop would.
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
        app.on_answers_rendered(gen, id, render(&answers));
    }

    fn answer(prompt: &str, text: &str) -> Answer {
        Answer {
            prompt: prompt.into(),
            text: text.into(),
        }
    }

    #[test]
    fn the_reader_covers_the_pane_and_shows_formulas_as_their_source() {
        let mut app = app_with(&[info(Some(1), None)]);
        let text = "Energy is <latex>E = mc^2</latex>, and\n\n<latex>\\frac{a}{b}</latex>";
        open_with(&mut app, vec![answer("old", "first"), answer("why?", text)]);
        let rows = screen(&app);
        assert_eq!(rows[0], " 1 ✓ app", "the tab bar stays");
        assert_eq!(rows[1], "answer 2/2 · why?");
        assert_eq!(
            rows[2..5],
            [
                "Energy is <latex>E = mc^2</latex>, and",
                "",
                "<latex>\\frac{a}{b}</latex>"
            ]
        );
        assert!(rows[28].starts_with(" /srv/app"), "the status bar stays");
        assert!(rows[29].ends_with("Alt+h help"), "{}", rows[29]);
        // ← shows the older answer.
        app.on_terminal(press(KeyCode::Left));
        assert_eq!(screen(&app)[1..3], ["answer 1/2 · old", "first"]);
    }

    #[test]
    fn unsafe_clusters_never_become_cells() {
        let style = Style::default();
        let cells = text_cells("a\u{202e}b\u{1b}c\u{200b}", style).unwrap();
        let text: String = cells.iter().map(Cell::symbol).collect();
        assert_eq!(text, "abc");
        // A wide cluster takes its cell and the one it covers.
        assert_eq!(text_cells("漢", style).unwrap().len(), 2);
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
