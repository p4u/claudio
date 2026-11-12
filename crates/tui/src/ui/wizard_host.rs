//! The wizard's first screen: the filter, LOCAL (explore, recent dirs) and
//! REMOTE (ssh hosts, a typed one), with the ssh agent forwarding key's hint
//! and the marker of a host in `[ssh] forward_agent`.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{dir_row, draw_input, draw_list, popup, truncate};
use crate::app::App;
use crate::keymap::Action;
use crate::wizard::{HostSection, HostStep, Wizard};

/// After a remote host whose bridge asks for agent forwarding.
const AGENT_MARK: &str = " [agent]";

pub(super) fn draw_host_step(frame: &mut Frame, rect: Rect, w: &Wizard, hs: &HostStep, app: &App) {
    let inner = popup(
        frame,
        rect,
        "New session (↑/↓ move · Tab switch section · Enter pick · Esc cancel)",
    );
    if let Some(host) = &hs.connecting {
        frame.render_widget(
            Paragraph::new(format!("Connecting to {host}…"))
                .style(Style::default().fg(Color::Cyan)),
            inner,
        );
        return;
    }
    let height = inner.height as usize;
    // Layout: filter + hint (2) + LOCAL label (1) + local items + REMOTE label (1) + remote items.
    let local_count = hs.local_len().min((height.saturating_sub(4)) / 2);
    let [filter_area, rest] =
        Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).areas(inner);
    draw_input(frame, filter_area, "filter> ", &hs.input);
    draw_agent_hint(frame, filter_area, app);
    let sections = Layout::vertical([
        Constraint::Length(1),                  // LOCAL header
        Constraint::Length(local_count as u16), // local items
        Constraint::Length(1),                  // REMOTE header
        Constraint::Min(0),                     // remote items
    ])
    .split(rest);
    let local_focused = hs.focus == HostSection::Local;
    let remote_focused = hs.focus == HostSection::Remote;
    frame.render_widget(
        Paragraph::new("── LOCAL ──────────────────────────────────────────────────")
            .style(header(local_focused)),
        sections[0],
    );
    draw_local(frame, sections[1], w, hs, local_focused, app.now);
    frame.render_widget(
        Paragraph::new("── REMOTE ─────────────────────────────────────────────────")
            .style(header(remote_focused)),
        sections[2],
    );
    // REMOTE items (plus optional synthetic "connect to <query>" row).
    let mark = |h: &str| match app.forward_agent.iter().any(|f| f == h) {
        true => AGENT_MARK,
        false => "",
    };
    let mut remote_rows: Vec<String> = hs
        .items
        .iter()
        .map(|h| format!("  {h}{}", mark(h)))
        .collect();
    if hs.connect_raw {
        let typed = hs.input.trim();
        remote_rows.push(format!("  connect to {typed}{}", mark(typed)));
    }
    draw_list(
        frame,
        sections[3],
        &remote_rows,
        remote_focused.then_some(hs.selected),
    );
}

/// A section header: bright while it has the focus.
fn header(focused: bool) -> Style {
    match focused {
        true => Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
        false => Style::default().add_modifier(Modifier::DIM),
    }
}

/// LOCAL items: "Explore local dirs…" + recent dirs, badged like the
/// directory list.
fn draw_local(frame: &mut Frame, area: Rect, w: &Wizard, hs: &HostStep, focused: bool, now: u64) {
    let visible = area.height as usize;
    let selected = focused.then_some(hs.local_selected);
    let width = area.width as usize;
    let explore = truncate("  Explore local dirs…", width);
    let explore = if selected == Some(0) {
        Line::styled(explore, Style::default().add_modifier(Modifier::REVERSED))
    } else {
        Line::raw(explore)
    };
    let offset = selected.map_or(0, |i| i.saturating_sub(visible.saturating_sub(1)));
    let lines: Vec<Line> = std::iter::once(explore)
        .chain(hs.local_items.iter().enumerate().map(|(i, dir)| {
            let mut row = dir_row(
                w,
                dir,
                selected == Some(i + 1),
                width.saturating_sub(2),
                now,
            );
            row.spans.insert(0, Span::raw("  "));
            row
        }))
        .skip(offset)
        .take(visible)
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

/// The filter's second row: what the forwarding key does to the highlighted
/// remote host.
fn draw_agent_hint(frame: &mut Frame, area: Rect, app: &App) {
    if area.height < 2 {
        return;
    }
    let key = app.keymap.key_for(Action::ForwardAgent).unwrap_or_default();
    let hint = match app.wizard_remote_host() {
        Some(host) if app.forward_agent.contains(&host) => {
            format!("{key} ssh agent for {host}: forward on connect")
        }
        Some(host) => format!("{key} ssh agent for {host}: off (your ssh config decides)"),
        None => format!("{key} forward your ssh agent (pick a remote host)"),
    };
    let row = Rect {
        y: area.y + 1,
        height: 1,
        ..area
    };
    let text = truncate(&hint, area.width as usize);
    let style = Style::default().add_modifier(Modifier::DIM);
    frame.render_widget(Paragraph::new(text).style(style), row);
}
