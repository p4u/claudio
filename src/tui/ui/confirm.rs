//! The yes / no / skip popup.

use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use super::{centered, popup, str_width};
use crate::tui::confirm::{Choice, ConfirmPrompt};

/// Widest the popup gets, border included.
const MAX_WIDTH: u16 = 76;

pub fn draw_confirm(frame: &mut Frame, prompt: &ConfirmPrompt) {
    let area = frame.area();
    let width = MAX_WIDTH.min(area.width.saturating_sub(2));
    let inner_width = usize::from(width.saturating_sub(2)).max(1);
    let text_rows: usize = prompt
        .text
        .lines()
        .map(|line| str_width(line).div_ceil(inner_width).max(1))
        .sum();
    // Borders, the text, a blank line and the key hint.
    let height = (text_rows + 4) as u16;
    let inner = popup(frame, centered(area, width, height), &prompt.title);

    let keys = prompt
        .choices
        .iter()
        .map(Choice::hint)
        .collect::<Vec<_>>()
        .join(" · ");
    let mut lines: Vec<Line> = prompt.text.lines().map(Line::from).collect();
    lines.push(Line::default());
    lines.push(Line::styled(
        keys,
        Style::default().add_modifier(Modifier::BOLD),
    ));
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::confirm::{ConfirmAction, Skip};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn render(prompt: &ConfirmPrompt, width: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, 14)).unwrap();
        terminal.draw(|f| draw_confirm(f, prompt)).unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn prompt(skip: bool) -> ConfirmPrompt {
        let mut choices = vec![
            Choice::new(
                'y',
                "update",
                Some(ConfirmAction::UpdateClaude {
                    host: "devbox".into(),
                    install: false,
                }),
            ),
            Choice::new('n', "not now", None),
        ];
        if skip {
            choices.push(Choice::new(
                's',
                "skip this version",
                Some(ConfirmAction::SkipClaude(Skip {
                    host: "devbox".into(),
                    version: "2.1.296".into(),
                })),
            ));
        }
        ConfirmPrompt::new(
            "Claude is out of date",
            "claude on devbox is 2.1.280, older than the 2.1.296 on this machine.\nRun `claude update` there?",
            choices,
        )
    }

    #[test]
    fn shows_both_versions_and_the_keys() {
        let screen = render(&prompt(true), 100);
        assert!(screen.contains("Claude is out of date"));
        assert!(screen.contains("2.1.280") && screen.contains("2.1.296"));
        assert!(screen.contains("[y] update · [n] not now · [s] skip this version"));
        assert!(!render(&prompt(false), 100).contains("[s]"));
    }

    #[test]
    fn long_text_wraps_inside_a_narrow_terminal() {
        let screen = render(&prompt(true), 40);
        assert!(screen.contains("[y] update"), "keys stay visible:\n{screen}");
    }
}
