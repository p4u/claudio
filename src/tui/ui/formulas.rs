//! The formulas popup (Alt+u); its state and lines are in `tui/formulas.rs`.

use ratatui::style::{Modifier, Style};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use super::{centered, popup};
use crate::claude::latex::sanitize;
use crate::tui::app::App;
use crate::tui::formulas::{self, FormulasView, Line};
use crate::tui::latex::text_cells;

/// The popup's widest: it is also at most 90% of the screen's height.
const MAX_WIDTH: u16 = 100;

/// Draw the popup over the whole screen, scrolled where `view` says. Every
/// line is written cell by cell like the pane's formulas, and cut at the
/// popup's edge.
pub fn draw_formulas(frame: &mut Frame, app: &App, view: &FormulasView) {
    let area = frame.area();
    let width = MAX_WIDTH.min(area.width.saturating_sub(2));
    let height = (u32::from(area.height) * 9 / 10) as u16;
    let inner = popup(frame, centered(area, width, height), "formulas");
    let lines = app
        .sessions
        .iter()
        .find(|v| v.id == view.session)
        .map(formulas::lines)
        .unwrap_or_default();
    let total = u16::try_from(lines.len()).unwrap_or(u16::MAX);
    view.set_max_scroll(total.saturating_sub(inner.height));
    let plain = Style::default();
    let dim = plain.add_modifier(Modifier::DIM);
    let shown = lines
        .iter()
        .skip(usize::from(view.scroll()))
        .take(usize::from(inner.height));
    let buf = frame.buffer_mut();
    for (y, line) in (inner.y..).zip(shown) {
        let (text, style) = match line {
            Line::Formula(text) => (text.as_str(), plain),
            Line::Source(text) => (text.as_str(), dim),
            Line::Hint(text) => (*text, dim),
            Line::Blank => continue,
        };
        let Some(mut cells) = text_cells(&sanitize(text), style) else {
            continue;
        };
        cells.truncate(usize::from(inner.width));
        // A wide character cut in half at the edge goes.
        if cells.last().is_some_and(|c| c.symbol().width() > 1) {
            cells.pop();
        }
        for (x, cell) in (inner.x..).zip(cells) {
            buf[(x, y)] = cell;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::tui::app::{App, Modal};
    use crate::tui::latex::Formula;
    use crate::tui::test_support::{alt, app_with, info};
    use ratatui::{backend::TestBackend, Terminal};

    /// The screen's rows after drawing `app`.
    fn screen(app: &App) -> Vec<String> {
        let mut term = Terminal::new(TestBackend::new(app.width, app.height)).unwrap();
        term.draw(|f| super::super::draw(f, app)).unwrap();
        let buf = term.backend().buffer();
        (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
            .collect()
    }

    #[test]
    fn the_popup_shows_each_formula_over_its_source_newest_in_view() {
        let mut app = app_with(&[info(Some(1), None)]);
        let v = &mut app.sessions[0];
        v.latex_srcs = (0..20).map(|i| format!("x_{i}")).collect();
        v.latex = vec![Formula::new(
            "x_19",
            (Some("x₁₉".into()), Some(vec!["x₁₉".into()])),
        )];
        app.on_terminal(alt('u'));
        assert!(matches!(app.modal, Some(Modal::Formulas(_))));
        let text = screen(&app).join("\n");
        assert!(text.contains(" formulas "), "{text}");
        // Scrolled to the bottom: the newest is shown, rendered, the oldest not.
        assert!(text.contains("x₁₉") && text.contains("x_19"), "{text}");
        assert!(!text.contains("x_0 "), "{text}");
    }

    #[test]
    fn the_empty_popup_says_how_to_get_formulas() {
        let mut app = app_with(&[info(Some(1), None)]);
        app.on_terminal(alt('u'));
        let text = screen(&app).join("\n");
        assert!(text.contains("No formulas yet."), "{text}");
        assert!(text.contains("claudio daemon restart"), "{text}");
    }
}
