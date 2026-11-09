//! Drawing the leader popup, an action's input line and the list picker
//! (`tui/leader.rs` has their state and keys).

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{centered, draw_input, popup};
use crate::app::App;
use crate::fmt::{str_width, truncate};
use crate::leader::{Input, Picker};
use crate::reader::layout::safe;

/// The leader popup: `key  title  (extension)` per action.
pub fn draw_leader(frame: &mut Frame, app: &App) {
    let entries = app.leader_entries();
    let area = frame.area();
    let height = (entries.len() as u16 + 2).min(area.height);
    let rect = centered(area, 56.min(area.width.saturating_sub(2)), height);
    let inner = popup(frame, rect, "Extensions (a key runs · Esc closes)");
    let dim = Style::default().add_modifier(Modifier::DIM);
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let lines: Vec<Line> = entries
        .iter()
        .map(|e| {
            let mut spans = vec![
                Span::styled(format!(" {}  ", e.key), bold),
                Span::raw(safe(&e.title)),
            ];
            if let Some(ext) = &e.extension {
                spans.push(Span::styled(format!("  ({ext})"), dim));
            }
            Line::from(spans)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

/// An action's line of input.
pub fn draw_action_input(frame: &mut Frame, input: &Input) {
    let area = frame.area();
    let rect = centered(area, 64.min(area.width.saturating_sub(4)), 3);
    let title = format!("{} (Enter run · Esc cancel)", safe(&input.prompt));
    let inner = popup(frame, rect, &title);
    draw_input(frame, inner, "", &input.text);
}

/// The list picker: the filter line, then `label  detail` rows (the detail
/// dimmed and cut to the width), the selection reversed.
pub fn draw_picker(frame: &mut Frame, picker: &Picker) {
    let area = frame.area();
    let rect = centered(
        area,
        100.min(area.width.saturating_sub(2)),
        area.height.saturating_sub(4).max(3),
    );
    let title = format!("{} (Enter · Esc · type to filter)", safe(&picker.title));
    let inner = popup(frame, rect, &title);
    if inner.height == 0 {
        return;
    }
    let width = inner.width as usize;
    let dim = Style::default().add_modifier(Modifier::DIM);
    let shown = picker.shown();
    let count = format!("  {} of {}", shown.len(), picker.items.len());
    let mut lines = vec![Line::from(vec![
        Span::styled("/ ", dim),
        Span::raw(safe(&picker.filter)),
        Span::styled(count, dim),
    ])];
    let visible = (inner.height as usize).saturating_sub(1);
    let offset = picker.selected.saturating_sub(visible.saturating_sub(1));
    for (row, &i) in shown.iter().enumerate().skip(offset).take(visible) {
        let item = &picker.items[i];
        let label = truncate(&safe(&item.label), width);
        let room = width.saturating_sub(str_width(&label) + 2);
        let detail = item
            .detail
            .as_deref()
            .filter(|_| room > 0)
            .map(|d| truncate(&safe(d), room))
            .unwrap_or_default();
        let mut line = Line::from(vec![
            Span::raw(label),
            Span::styled(format!("  {detail}"), dim),
        ]);
        if row == picker.selected {
            line = line.patch_style(Style::default().add_modifier(Modifier::REVERSED));
        }
        lines.push(line);
    }
    frame.render_widget(Paragraph::new(lines), inner);
}
