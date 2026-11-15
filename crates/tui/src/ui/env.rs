//! Drawing the custom env menu (`tui/env_view.rs` has its state and keys):
//! the tabs, the rows (`NAME = value`, escaped) or the step's input with
//! the catalogue's matches, the selected name's description, the last
//! refusal and the footer. Every line is made safe and cut to the width.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{centered, draw_input, popup};
use crate::app::App;
use crate::env_view::{EnvView, Step, Tab, ESCAPES};
use crate::fmt::truncate;
use crate::reader::layout::safe;
use claudio_core::claude::env::{escape, known};

/// The menu's title.
pub const TITLE: &str = "Environment (Tab scope · a add · Enter edit · d delete · Esc close)";

/// The menu over the whole terminal.
pub fn draw_env(frame: &mut Frame, app: &App, view: &EnvView) {
    let area = frame.area();
    let rect = centered(
        area,
        100.min(area.width.saturating_sub(2)),
        area.height.saturating_sub(4).max(3),
    );
    let inner = popup(frame, rect, TITLE);
    if inner.height < 3 || inner.width == 0 {
        return;
    }
    let width = inner.width as usize;
    let fit = |text: &str| truncate(&safe(text), width);
    let dim = Style::default().add_modifier(Modifier::DIM);
    let at = |y: usize, h: usize| Rect {
        y: inner.y + y as u16,
        height: h as u16,
        ..inner
    };
    let height = inner.height as usize;
    frame.render_widget(Paragraph::new(tabs(view)), at(0, 1));
    // The footer and the error take the last lines, the description the one
    // above them; the step's body what is left.
    let footer = app.env_footer();
    frame.render_widget(
        Paragraph::new(Line::styled(fit(&footer), dim)),
        at(height - 1, 1),
    );
    let mut bottom = height - 1;
    if let Some(error) = &view.error {
        bottom -= 1;
        let style = Style::default().add_modifier(Modifier::BOLD);
        frame.render_widget(
            Paragraph::new(Line::styled(fit(error), style)),
            at(bottom, 1),
        );
    }
    let described = match &view.step {
        Step::List => view.selected_name().map(str::to_owned),
        Step::Name { input, pick } => EnvView::matches(&input.text)
            .get(pick.unwrap_or(0))
            .map(|v| v.name.to_owned()),
        Step::Value { name, .. } | Step::Delete(name) => Some(name.clone()),
    };
    if let Some(var) = described.as_deref().and_then(known) {
        bottom = bottom.saturating_sub(1);
        let text = format!("{} · {}: {}", var.name, var.category, var.description);
        frame.render_widget(Paragraph::new(Line::styled(fit(&text), dim)), at(bottom, 1));
    }
    let body = at(2, bottom.saturating_sub(3));
    match &view.step {
        Step::List => draw_rows(frame, view, body),
        Step::Name { input, pick } => {
            draw_input(frame, at(2, 1), "name: ", &input.text);
            draw_matches(frame, &input.text, *pick, at(3, bottom.saturating_sub(4)));
        }
        Step::Value { name, input } => {
            draw_input(frame, at(2, 1), &format!("{name} = "), &input.text);
            let hint = Line::styled(fit(&format!("  {ESCAPES}")), dim);
            frame.render_widget(Paragraph::new(hint), at(3, 1));
        }
        Step::Delete(name) => {
            let ask = Line::raw(fit(&format!("delete {name}? (y/n)")));
            frame.render_widget(Paragraph::new(ask), at(2, 1));
        }
    }
}

/// `Global` and `Session: <label>` (or why there is none), the open one
/// reversed.
fn tabs(view: &EnvView) -> Line<'static> {
    let on = Style::default().add_modifier(Modifier::REVERSED);
    let off = Style::default();
    let dim = Style::default().add_modifier(Modifier::DIM);
    let (global, session) = match view.tab {
        Tab::Global => (on, off),
        Tab::Session => (off, on),
    };
    let label = match &view.session {
        Ok((_, label)) => Span::styled(format!(" Session: {} ", safe(label)), session),
        Err(why) => Span::styled(format!(" Session: — ({why}) "), dim),
    };
    Line::from(vec![
        Span::styled(" Global ", global),
        Span::raw("  "),
        label,
    ])
}

/// The tab's rows, `NAME = value` (escaped), the selected one reversed.
fn draw_rows(frame: &mut Frame, view: &EnvView, area: Rect) {
    let width = area.width as usize;
    let dim = Style::default().add_modifier(Modifier::DIM);
    let loaded = match view.tab {
        Tab::Global => view.global.is_some(),
        Tab::Session => view.own.is_some(),
    };
    let rows = view.rows();
    let lines: Vec<Line> = if !loaded {
        vec![Line::styled("reading…", dim)]
    } else if rows.is_empty() {
        vec![Line::styled("none yet: a adds one", dim)]
    } else {
        let visible = (area.height as usize).max(1);
        let offset = view.selected.saturating_sub(visible - 1);
        rows.iter()
            .enumerate()
            .skip(offset)
            .take(visible)
            .map(|(i, (name, value))| {
                let text = truncate(
                    &safe(&format!("{name} = {}", escape(value.as_str()))),
                    width,
                );
                let style = if i == view.selected {
                    Style::default().add_modifier(Modifier::REVERSED)
                } else {
                    Style::default()
                };
                Line::styled(text, style)
            })
            .collect()
    };
    frame.render_widget(Paragraph::new(lines), area);
}

/// The catalogue's matches of `typed`, `pick` highlighted and scrolled into
/// view, each with its category.
fn draw_matches(frame: &mut Frame, typed: &str, pick: Option<usize>, area: Rect) {
    let visible = area.height as usize;
    if visible == 0 {
        return;
    }
    let width = area.width as usize;
    let dim = Style::default().add_modifier(Modifier::DIM);
    let matches = EnvView::matches(typed);
    let offset = pick.map_or(0, |p| p.saturating_sub(visible - 1));
    let lines: Vec<Line> = matches
        .iter()
        .enumerate()
        .skip(offset)
        .take(visible)
        .map(|(i, var)| {
            let style = if Some(i) == pick {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            let name = truncate(&format!("  {}", var.name), width);
            Line::from(vec![
                Span::styled(name, style),
                Span::styled(format!("  {}", var.category), dim),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}
