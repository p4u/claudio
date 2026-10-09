//! The proxy stats popup (Alt+s).

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::proxy::api::PoolStatus;
use crate::proxy::cmd::fmt_tokens;

use super::super::app::ProxyStatus;
use super::{centered, popup, truncate};

// ── Proxy stats popup ─────────────────────────────────────────────────────────

pub(super) fn draw_proxy_stats(frame: &mut Frame, profile_name: &str, status: Option<&ProxyStatus>) {
    let area = frame.area();
    let rect = centered(
        area,
        70.min(area.width.saturating_sub(4)),
        24.min(area.height.saturating_sub(2)),
    );
    let title = format!("Proxy stats: {} (Esc close)", profile_name);
    let inner = popup(frame, rect, &title);

    let Some(status) = status else {
        frame.render_widget(
            Paragraph::new("Fetching stats…").style(Style::default().add_modifier(Modifier::DIM)),
            inner,
        );
        return;
    };

    let mut lines: Vec<Line> = Vec::new();

    // Pool health.
    if let Some(pool) = &status.pool {
        let overall = pool.overall();
        let color = match overall {
            PoolStatus::Ok => Color::Green,
            PoolStatus::Busy => Color::Yellow,
            PoolStatus::Saturated | PoolStatus::Unavailable => Color::Red,
            PoolStatus::Unknown => Color::DarkGray,
        };
        lines.push(Line::from(vec![
            Span::raw("Pool: "),
            Span::styled(
                overall.label(),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
        ]));
        for prov in &pool.providers {
            let pcolor = match prov.status.as_str() {
                "ok" => Color::Green,
                "busy" => Color::Yellow,
                "saturated" | "unavailable" => Color::Red,
                _ => Color::DarkGray,
            };
            lines.push(Line::from(vec![
                Span::raw(format!("  {:20} ", prov.name)),
                Span::styled(&prov.status, Style::default().fg(pcolor)),
            ]));
        }
        lines.push(Line::raw(""));
    }

    // Stats totals.
    if let Some(stats) = &status.stats {
        let total_tok = stats.totals.input_tokens + stats.totals.output_tokens;
        lines.push(Line::raw(format!(
            "Stats ({}):  {} requests  {}  total tokens",
            stats.period,
            stats.totals.requests,
            fmt_tokens(total_tok),
        )));
        lines.push(Line::raw(format!(
            "  in: {}  out: {}  cache-read: {}  cache-create: {}",
            fmt_tokens(stats.totals.input_tokens),
            fmt_tokens(stats.totals.output_tokens),
            fmt_tokens(stats.totals.cache_read),
            fmt_tokens(stats.totals.cache_creation),
        )));

        // Limit bar.
        if let Some(lim) = &stats.limit {
            let bar_width = (inner.width as usize).saturating_sub(12).min(40);
            let filled = ((lim.used_pct * bar_width as f64) as usize).min(bar_width);
            let bar: String = "█".repeat(filled) + &"░".repeat(bar_width - filled);
            let color = if lim.used_pct >= 0.9 {
                Color::Red
            } else if lim.used_pct >= 0.7 {
                Color::Yellow
            } else {
                Color::Green
            };
            lines.push(Line::raw(""));
            lines.push(Line::raw(format!(
                "Limit: {:.0}% of {}",
                lim.used_pct * 100.0,
                fmt_tokens(lim.output_tokens)
            )));
            lines.push(Line::from(Span::styled(
                format!("[{bar}]"),
                Style::default().fg(color),
            )));
            if lim.blocked {
                lines.push(Line::styled(
                    format!(
                        "BLOCKED until {}",
                        lim.blocked_until.as_deref().unwrap_or("?")
                    ),
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ));
            }
        }

        // By-model table.
        if !stats.by_model.is_empty() {
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                "By model:",
                Style::default().add_modifier(Modifier::BOLD),
            ));
            for m in &stats.by_model {
                let out_tok = fmt_tokens(m.output_tokens);
                let model = truncate(&m.model, 40);
                lines.push(Line::raw(format!(
                    "  {:42} {:>4} req  {:>8} out-tok",
                    model, m.requests, out_tok
                )));
            }
        }
    } else {
        lines.push(Line::styled(
            "Stats unavailable",
            Style::default().fg(Color::DarkGray),
        ));
    }

    frame.render_widget(
        Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: false }),
        inner,
    );
}
