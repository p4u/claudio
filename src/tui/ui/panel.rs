//! Drawing any addon panel (`tui/panel_view.rs` has its state and keys):
//! the header (its pages, the current one marked, and its window), the
//! page's blocks, and the footer (keys, actions, age, scroll). Pure: a
//! function of the view and the clock. Every string came through the
//! core's `addon::clean`; it is made safe again and cut to the width.

use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{centered, popup};
use crate::addon::{Block, Tone};
use crate::tui::fmt::{fmt_age, fmt_tokens, str_width, truncate};
use crate::tui::panel_view::PanelView;
use crate::tui::reader::layout::safe;

const POPUP_W: u16 = 86;
const POPUP_H: u16 = 28;
/// Cells of a gauge's and a bar's bar.
const BAR_W: usize = 20;
/// The widest key column of key/value lines.
const KEY_W: usize = 16;

fn dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

fn bold() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

/// A tone's style.
fn toned(tone: Option<Tone>) -> Style {
    match tone {
        None => Style::default(),
        Some(Tone::Ok) => Style::default().fg(Color::Green),
        Some(Tone::Warn) => Style::default().fg(Color::Yellow),
        Some(Tone::Hot) => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        Some(Tone::Muted) => dim(),
        Some(Tone::Accent) => Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    }
}

/// The panel over the terminal; `now` (Unix seconds) ages its numbers.
pub(super) fn draw_panel(frame: &mut Frame, view: &PanelView, now: u64) {
    let area = frame.area();
    let rect = centered(
        area,
        POPUP_W.min(area.width.saturating_sub(2)),
        POPUP_H.min(area.height.saturating_sub(2)),
    );
    let title = view
        .panel
        .as_ref()
        .map_or(view.decl.title, |p| p.title.as_str());
    let inner = popup(frame, rect, &safe(title));
    if inner.height < 3 || inner.width == 0 {
        return;
    }
    let [header, rule, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(inner);
    let width = inner.width as usize;
    frame.render_widget(Paragraph::new(header_line(view, width)), header);
    let rule_line = Line::styled("─".repeat(width), dim());
    frame.render_widget(Paragraph::new(rule_line), rule);
    let lines = body_lines(view, width);
    let max = lines.len().saturating_sub(usize::from(body.height));
    let max = max.min(usize::from(u16::MAX)) as u16;
    // Only here is the page's length known: the view scrolls within it.
    view.set_max_scroll(max);
    let scroll = view.scroll();
    frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), body);
    let foot = footer_line(view, width, (scroll, max), now);
    frame.render_widget(Paragraph::new(foot), footer);
}

/// ` 1 Overview │ 2 Models │ …        window: [24h] 7d 30d`.
fn header_line(view: &PanelView, width: usize) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let titles: Vec<String> = match &view.panel {
        Some(p) => p.pages.iter().map(|page| safe(&page.title)).collect(),
        None => vec![String::new()],
    };
    for (i, title) in titles.iter().enumerate() {
        let text = format!(" {} {} ", i + 1, title);
        let style = match i == view.req.page {
            true => Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
            false => Style::default().fg(Color::Cyan),
        };
        spans.push(Span::styled(text, style));
        spans.push(Span::styled("│", dim()));
    }
    let mut right = Vec::new();
    if view.decl.windows.len() > 1 {
        right.push(Span::styled("window: ", dim()));
        for w in view.decl.windows {
            match Some(*w) == view.req.window.as_deref() {
                true => right.push(Span::styled(format!("[{w}]"), bold().fg(Color::Cyan))),
                false => right.push(Span::styled(format!(" {w} "), dim())),
            }
        }
    }
    let left_w: usize = spans.iter().map(Span::width).sum();
    let right_w: usize = right.iter().map(Span::width).sum();
    if left_w + right_w < width {
        spans.push(Span::raw(" ".repeat(width - left_w - right_w)));
        spans.extend(right);
    }
    Line::from(spans)
}

/// The keys, the panel's actions, and how old its numbers are.
fn footer_line(
    view: &PanelView,
    width: usize,
    (scroll, max): (u16, u16),
    now: u64,
) -> Line<'static> {
    let mut keys = " ←→ page · w window · ↑↓ scroll · r refresh".to_owned();
    for a in view.panel.iter().flat_map(|p| p.actions.iter()) {
        if let Some(k) = a.key {
            keys.push_str(&format!(" · {k} {}", safe(&a.label)));
        }
    }
    keys.push_str(" · q close");
    let (right, style) = match (view.loading, view.panel.as_ref().and_then(|p| p.updated)) {
        (true, _) => ("⟳ fetching…".to_owned(), Style::default().fg(Color::Yellow)),
        (false, Some(at)) => (
            format!("updated {} ago", fmt_age(now.saturating_sub(at))),
            dim(),
        ),
        (false, None) => (String::new(), dim()),
    };
    let more = match (scroll > 0, scroll < max) {
        (true, true) => "↕ ",
        (true, false) => "↑ ",
        (false, true) => "↓ ",
        (false, false) => "",
    };
    let right = format!("{more}{right} ");
    let keys = truncate(&keys, width.saturating_sub(str_width(&right)));
    let pad = width.saturating_sub(str_width(&keys) + str_width(&right));
    Line::from(vec![
        Span::styled(keys, dim()),
        Span::raw(" ".repeat(pad)),
        Span::styled(right, style),
    ])
}

/// The current page's blocks as lines, a blank line between blocks.
pub fn body_lines(view: &PanelView, width: usize) -> Vec<Line<'static>> {
    let Some(page) = view.panel.as_ref().and_then(|p| p.pages.get(view.req.page)) else {
        let wait = if view.loading {
            " Fetching…"
        } else {
            " Nothing to show."
        };
        return vec![Line::styled(wait.to_owned(), dim())];
    };
    let mut lines = Vec::new();
    for (i, block) in page.blocks.iter().enumerate() {
        if i > 0 {
            lines.push(Line::raw(""));
        }
        block_lines(block, width, &mut lines);
    }
    lines
}

/// One block's lines, each within `width` columns.
fn block_lines(block: &Block, width: usize, lines: &mut Vec<Line<'static>>) {
    let fit = |s: &str, w: usize| truncate(&safe(s), w);
    match block {
        Block::Text { text, tone } => {
            for row in wrap(&safe(text), width.saturating_sub(1)) {
                lines.push(Line::styled(format!(" {row}"), toned(*tone)));
            }
        }
        Block::KeyValues { items } => {
            let key_w = items
                .iter()
                .map(|(k, _)| str_width(k))
                .max()
                .unwrap_or(0)
                .min(KEY_W);
            for (k, v) in items {
                let room = width.saturating_sub(key_w + 3);
                lines.push(Line::from(vec![
                    Span::styled(format!(" {:<key_w$}  ", fit(k, key_w)), dim()),
                    Span::raw(fit(v, room)),
                ]));
            }
        }
        Block::Table { header, rows } => table_lines(header, rows, width, lines),
        Block::Gauge {
            label,
            used,
            limit,
            resets,
        } => {
            let frac = if *limit > 0.0 { used / limit } else { 0.0 };
            let color = match frac {
                f if f >= 0.9 => Color::Red,
                f if f >= 0.7 => Color::Yellow,
                _ => Color::Green,
            };
            lines.push(Line::styled(
                format!(" {}", fit(label, width.saturating_sub(1))),
                bold(),
            ));
            let mut spans = vec![
                Span::raw(" ["),
                Span::styled(bar(frac, BAR_W), Style::default().fg(color)),
                Span::raw("] "),
                Span::styled(format!("{}/{}", number(*used), number(*limit)), bold()),
            ];
            if let Some(r) = resets {
                spans.push(Span::styled(format!("  {}", safe(r)), dim()));
            }
            lines.push(Line::from(spans));
        }
        Block::Bars { label, series } => {
            lines.push(Line::styled(
                format!(" {}", fit(label, width.saturating_sub(1))),
                bold(),
            ));
            let max = series.iter().map(|(_, v)| *v).fold(0.0_f64, f64::max);
            let key_w = series
                .iter()
                .map(|(k, _)| str_width(k))
                .max()
                .unwrap_or(0)
                .min(KEY_W);
            for (k, v) in series {
                let frac = if max > 0.0 { v / max } else { 0.0 };
                lines.push(Line::from(vec![
                    Span::styled(format!("   {:<key_w$} ", fit(k, key_w)), dim()),
                    Span::styled(bar(frac, BAR_W), Style::default().fg(Color::Cyan)),
                    Span::styled(format!(" {:>8}", number(*v)), bold()),
                ]));
            }
        }
    }
}

/// A table with its columns sized to their content; the last columns are
/// cut when it is wider than `width`.
fn table_lines(
    header: &[String],
    rows: &[Vec<String>],
    width: usize,
    lines: &mut Vec<Line<'static>>,
) {
    let cols = header
        .len()
        .max(rows.iter().map(Vec::len).max().unwrap_or(0));
    let mut widths = vec![0usize; cols];
    for row in std::iter::once(header).chain(rows.iter().map(Vec::as_slice)) {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(str_width(&safe(cell)));
        }
    }
    let render = |row: &[String]| -> String {
        let cells: Vec<String> = (0..cols)
            .map(|i| {
                let cell = row.get(i).map(|c| safe(c)).unwrap_or_default();
                let w = widths[i];
                format!("{:<w$}", truncate(&cell, w))
            })
            .collect();
        truncate(&format!(" {}", cells.join("  ").trim_end()), width)
    };
    if header.iter().any(|h| !h.is_empty()) {
        lines.push(Line::styled(
            render(header),
            bold().add_modifier(Modifier::UNDERLINED),
        ));
    }
    for row in rows {
        lines.push(Line::raw(render(row)));
    }
}

/// `s` broken into lines of at most `width` columns, at spaces when it can.
fn wrap(s: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let mut out = Vec::new();
    let mut line = String::new();
    for word in s.split(' ') {
        let sep = usize::from(!line.is_empty());
        if str_width(&line) + sep + str_width(word) <= width {
            if sep == 1 {
                line.push(' ');
            }
            line.push_str(word);
            continue;
        }
        if !line.is_empty() {
            out.push(std::mem::take(&mut line));
        }
        line = truncate(word, width);
    }
    out.push(line);
    out
}

/// A number as a gauge or bar shows it: compact past a thousand.
fn number(v: f64) -> String {
    if v.abs() >= 1000.0 {
        fmt_tokens(v.round() as i64)
    } else if v.fract() == 0.0 {
        format!("{v:.0}")
    } else {
        format!("{v:.2}")
    }
}

/// A bar of `width` cells filled to `frac` (0..=1), in eighths.
fn bar(frac: f64, width: usize) -> String {
    const PARTIAL: [char; 8] = ['░', '▏', '▎', '▍', '▌', '▋', '▊', '▉'];
    let frac = frac.clamp(0.0, 1.0);
    let eighths = (frac * width as f64 * 8.0).round() as usize;
    let full = eighths / 8;
    let mut out = "█".repeat(full.min(width));
    if full < width {
        out.push(PARTIAL[eighths % 8]);
        out.push_str(&"░".repeat(width - full - 1));
    }
    out
}

#[cfg(test)]
mod tests;
