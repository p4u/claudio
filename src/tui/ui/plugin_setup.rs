//! Drawing the plugin setup screen (`tui/plugin_setup.rs` has its state
//! and keys): the manifest's items by section, each with its checkbox, and
//! the key hint. Every line is made safe and cut to the width: titles come
//! from the manifest.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{centered, popup};
use crate::tui::fmt::truncate;
use crate::tui::plugin_setup::PluginSetup;
use crate::tui::reader::layout::safe;

/// The lines of `setup`'s list: a heading before each section, then one
/// line per item, with the index of the item each line shows.
pub fn setup_lines(setup: &PluginSetup) -> Vec<(Option<usize>, String)> {
    let mut lines = Vec::new();
    let mut section = None;
    for (i, item) in setup.items.iter().enumerate() {
        if section != Some(item.section) {
            section = Some(item.section);
            lines.push((None, item.section.heading().to_owned()));
        }
        let mark = if item.on { 'x' } else { ' ' };
        lines.push((Some(i), format!("  [{mark}] {}", item.title)));
    }
    if lines.is_empty() {
        lines.push((None, "(nothing to apply)".to_owned()));
    }
    lines
}

/// The setup screen over the whole terminal.
pub fn draw_plugin_setup(frame: &mut Frame, setup: &PluginSetup) {
    let area = frame.area();
    let lines = setup_lines(setup);
    // The list, a blank line, the hint, and the borders; capped.
    let wanted = (lines.len() + 4).min(u16::MAX as usize) as u16;
    let rect = centered(
        area,
        100.min(area.width.saturating_sub(2)),
        wanted.min(area.height.saturating_sub(2).max(3)),
    );
    let title = format!("Set up {} {}", safe(&setup.name), safe(&setup.version));
    let inner = popup(frame, rect, &title);
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
        .position(|(i, _)| *i == Some(setup.selected))
        .unwrap_or(0);
    let offset = at.saturating_sub(room.saturating_sub(1));
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let shown: Vec<Line> = lines
        .iter()
        .skip(offset)
        .take(room)
        .map(|(i, text)| match i {
            None => Line::styled(fit(text), bold),
            Some(i) => {
                let mut style = Style::default();
                if !setup.items[*i].on {
                    style = style.add_modifier(Modifier::DIM);
                }
                if *i == setup.selected {
                    style = style.add_modifier(Modifier::REVERSED);
                }
                Line::styled(fit(text), style)
            }
        })
        .collect();
    let rows = |y: usize, h: usize| Rect {
        y: inner.y + y as u16,
        height: h as u16,
        ..inner
    };
    frame.render_widget(Paragraph::new(shown), rows(0, room.min(height)));
    if footer > 0 {
        let dim = Style::default().add_modifier(Modifier::DIM);
        frame.render_widget(
            Paragraph::new(Line::styled(fit(setup.hint()), dim)),
            rows(height - 1, 1),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::vocdoni::plugins::manifest::Manifest;
    use ratatui::{backend::TestBackend, Terminal};

    fn setup() -> PluginSetup {
        let src = r#"
format = 1
name = "team"
version = "1.0.0"
instructions = "Rules."
[[marketplace]]
name = "m"
source = "github:o/r"
[[plugin]]
name = "a"
marketplace = "m"
[[mcp]]
name = "kb"
type = "sse"
url = "https://kb.example.org"
"#;
        let m = Manifest::parse(src).unwrap();
        PluginSetup::new(&m, &["mcp:kb".to_owned()], true)
    }

    #[test]
    fn items_are_listed_by_section_with_their_boxes() {
        let lines: Vec<String> = setup_lines(&setup()).into_iter().map(|(_, l)| l).collect();
        assert_eq!(
            lines,
            [
                "Plugins",
                "  [x] a@m",
                "MCP servers",
                "  [ ] kb (sse https://kb.example.org)",
                "Instructions",
                "  [x] instructions",
            ]
        );
    }

    #[test]
    fn the_screen_draws_its_title_items_and_hint() {
        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal.draw(|f| draw_plugin_setup(f, &setup())).unwrap();
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
