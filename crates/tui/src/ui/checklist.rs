//! Drawing an addon's checklist (`tui/checklist.rs` has its state and
//! keys): the title, the intro, a heading where the section changes, one
//! `[x]`/`[ ]` line per item with its detail dim below, and the key hint.
//! Every line is made safe and cut to the width.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{centered, popup};
use crate::checklist::ChecklistView;
use crate::fmt::truncate;
use crate::reader::layout::safe;

/// How a line of the list is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The intro, dim.
    Intro,
    /// A section heading, bold.
    Heading,
    /// Item `i`'s checkbox line.
    Item(usize),
    /// Item `i`'s detail, dim.
    Detail(usize),
}

/// The lines of `view`'s list.
pub fn checklist_lines(view: &ChecklistView) -> Vec<(Kind, String)> {
    let c = &view.checklist;
    let mut lines = Vec::new();
    if !c.intro.is_empty() {
        lines.push((Kind::Intro, c.intro.clone()));
    }
    let mut section: Option<&str> = None;
    for (i, item) in c.items.iter().enumerate() {
        if let Some(heading) = item.section.as_deref().filter(|h| section != Some(*h)) {
            section = Some(heading);
            lines.push((Kind::Heading, heading.to_owned()));
        }
        let mark = if item.on { 'x' } else { ' ' };
        lines.push((Kind::Item(i), format!("  [{mark}] {}", item.label)));
        if let Some(detail) = &item.detail {
            lines.push((Kind::Detail(i), format!("      {detail}")));
        }
    }
    if c.items.is_empty() {
        lines.push((Kind::Heading, "(nothing to choose)".to_owned()));
    }
    lines
}

/// The checklist over the whole terminal.
pub fn draw_checklist(frame: &mut Frame, view: &ChecklistView) {
    let area = frame.area();
    let lines = checklist_lines(view);
    // The list, a blank line, the hint, and the borders; capped.
    let wanted = (lines.len() + 4).min(u16::MAX as usize) as u16;
    let rect = centered(
        area,
        100.min(area.width.saturating_sub(2)),
        wanted.min(area.height.saturating_sub(2).max(3)),
    );
    let inner = popup(frame, rect, &safe(&view.checklist.title));
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    let width = inner.width as usize;
    let fit = |text: &str| truncate(&safe(&text.replace(['\n', '\t'], " ")), width);
    let height = inner.height as usize;
    let footer = usize::from(height > 2) * 2;
    let room = height.saturating_sub(footer).max(1);
    let at = lines
        .iter()
        .position(|(k, _)| *k == Kind::Item(view.selected))
        .unwrap_or(0);
    let offset = at.saturating_sub(room.saturating_sub(1));
    let items = &view.checklist.items;
    let dim = Style::default().add_modifier(Modifier::DIM);
    let shown: Vec<Line> = lines
        .iter()
        .skip(offset)
        .take(room)
        .map(|(kind, text)| {
            let style = match kind {
                Kind::Intro | Kind::Detail(_) => dim,
                Kind::Heading => Style::default().add_modifier(Modifier::BOLD),
                Kind::Item(i) => {
                    let mut style = Style::default();
                    if !items[*i].on {
                        style = style.add_modifier(Modifier::DIM);
                    }
                    if *i == view.selected {
                        style = style.add_modifier(Modifier::REVERSED);
                    }
                    style
                }
            };
            Line::styled(fit(text), style)
        })
        .collect();
    let rows = |y: usize, h: usize| Rect {
        y: inner.y + y as u16,
        height: h as u16,
        ..inner
    };
    frame.render_widget(Paragraph::new(shown), rows(0, room.min(height)));
    if footer > 0 {
        frame.render_widget(
            Paragraph::new(Line::styled(fit(view.hint()), dim)),
            rows(height - 1, 1),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addon_calls::Origin;
    use claudio_core::addon::{Checklist, ChecklistItem};
    use ratatui::{backend::TestBackend, Terminal};

    fn item(id: &str, label: &str, section: &str, on: bool) -> ChecklistItem {
        ChecklistItem {
            id: id.into(),
            label: label.into(),
            detail: None,
            section: Some(section.into()),
            on,
        }
    }

    fn view() -> ChecklistView {
        let mut kb = item(
            "mcp:kb",
            "kb (sse https://kb.example.org)",
            "MCP servers",
            false,
        );
        kb.detail = Some("needs KB_TOKEN".into());
        let checklist = Checklist {
            id: "team".into(),
            revision: "1".into(),
            title: "Set up team 1.0.0".into(),
            intro: "The team's tools.".into(),
            items: vec![
                item("plugin:a@m", "a@m", "Plugins", true),
                kb,
                item("instructions", "instructions", "Instructions", true),
            ],
        };
        ChecklistView::new("team", checklist, Origin::Offer)
    }

    #[test]
    fn items_are_listed_by_section_with_their_boxes() {
        let lines: Vec<String> = checklist_lines(&view())
            .into_iter()
            .map(|(_, l)| l)
            .collect();
        assert_eq!(
            lines,
            [
                "The team's tools.",
                "Plugins",
                "  [x] a@m",
                "MCP servers",
                "  [ ] kb (sse https://kb.example.org)",
                "      needs KB_TOKEN",
                "Instructions",
                "  [x] instructions",
            ]
        );
    }

    #[test]
    fn the_view_draws_its_title_items_and_hint() {
        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal.draw(|f| draw_checklist(f, &view())).unwrap();
        let buf = terminal.backend().buffer().clone();
        let text: String = (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    + "\n"
            })
            .collect();
        assert!(text.contains("Set up team 1.0.0"), "{text}");
        assert!(
            text.contains("[ ] kb (sse https://kb.example.org)"),
            "{text}"
        );
        assert!(text.contains("Enter save · Esc later"), "{text}");
    }
}
