//! Drawing the extensions screen (`tui/extensions_view.rs` has its state
//! and keys): the rows, the selected extension's details below them, and
//! the footer. Every line is made safe and cut to the width: names,
//! descriptions and titles may come from manifests.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{centered, popup};
use crate::app::App;
use crate::extensions_view::{row_lines, ExtensionsView, FOOTER, TITLE};
use crate::fmt::truncate;
use crate::reader::layout::safe;

/// The extensions screen over the whole terminal.
pub fn draw_extensions(frame: &mut Frame, app: &App, view: &ExtensionsView) {
    let area = frame.area();
    // Sized to the content: rows, a blank line, the tallest details, a
    // blank line, the footer, and the borders; capped to the pane.
    let rows_n = app.extension_rows().len();
    let tallest = (0..rows_n)
        .map(|i| app.extension_details(i).len())
        .max()
        .unwrap_or(0);
    let wanted = (rows_n + tallest + 5).min(u16::MAX as usize) as u16;
    let rect = centered(
        area,
        100.min(area.width.saturating_sub(2)),
        wanted.min(area.height.saturating_sub(4).max(3)),
    );
    let inner = popup(frame, rect, TITLE);
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    let width = inner.width as usize;
    let fit = |text: &str| truncate(&safe(text), width);
    let dim = Style::default().add_modifier(Modifier::DIM);
    let rows = app.extension_rows();
    let texts = row_lines(&rows);
    let selected = view.selected.min(texts.len().saturating_sub(1));
    let details = app.extension_details(selected);

    // The footer takes the last line, the details (and a blank line before
    // them) what the list leaves; the list keeps at least one row.
    let height = inner.height as usize;
    let footer = usize::from(height > 1);
    let list = texts
        .len()
        .min(height.saturating_sub(footer + details.len() + 1))
        .max(1)
        .min(height);
    let offset = selected.saturating_sub(list.saturating_sub(1));
    let lines: Vec<Line> = texts
        .iter()
        .zip(&rows)
        .enumerate()
        .skip(offset)
        .take(list)
        .map(|(i, (text, row))| {
            let mut style = Style::default();
            if !row.enabled {
                style = style.add_modifier(Modifier::DIM);
            }
            if i == selected {
                style = style.add_modifier(Modifier::REVERSED);
            }
            Line::styled(fit(text), style)
        })
        .collect();
    let at = |y: usize, h: usize| Rect {
        y: inner.y + y as u16,
        height: h as u16,
        ..inner
    };
    frame.render_widget(Paragraph::new(lines), at(0, list));

    let room = height.saturating_sub(list + footer + 1);
    if room > 0 {
        let lines: Vec<Line> = details
            .iter()
            .enumerate()
            .map(|(i, text)| {
                let line = fit(&format!("  {text}"));
                if i == 0 {
                    Line::raw(line)
                } else {
                    Line::styled(line, dim)
                }
            })
            .take(room)
            .collect();
        frame.render_widget(Paragraph::new(lines), at(list + 1, room));
    }
    if footer == 1 {
        frame.render_widget(
            Paragraph::new(Line::styled(fit(FOOTER), dim)),
            at(height - 1, 1),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interaction::Modal;
    use crate::test_support::{alt, app_with, info};
    use ratatui::{backend::TestBackend, Terminal};

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

    #[test]
    fn the_screen_shows_rows_details_and_the_footer() {
        let mut app = app_with(&[info(Some(1), None)]);
        // The web addon's rows follow the built-ins.
        crate::test_support::rows(&mut app, "web", &[("diagram", None)], &[]);
        app.activate(0);
        app.on_terminal(alt('i'));
        assert!(matches!(app.modal, Some(Modal::ExtensionsMenu(_))));
        for _ in 0..2 {
            app.on_terminal(crate::test_support::press(crossterm::event::KeyCode::Down));
        }
        let shown = screen(&app);
        for expected in [
            TITLE,
            "[x] search    built-in  Alt+z s    —",
            "[x] snippets  built-in  Alt+z p/P  —",
            "[x] diagram   web       —          —",
            "1.4.0 · work",
            FOOTER,
        ] {
            assert!(shown.contains(expected), "{expected} missing:\n{shown}");
        }
    }

    #[test]
    fn a_tiny_terminal_still_draws() {
        let mut app = app_with(&[info(Some(1), None)]);
        app.activate(0);
        app.on_terminal(crossterm::event::Event::Resize(20, 6));
        app.on_terminal(alt('i'));
        let _ = screen(&app);
        app.on_terminal(crossterm::event::Event::Resize(3, 3));
        let _ = screen(&app);
    }
}
