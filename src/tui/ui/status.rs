//! The two-line bottom bar: session info (line 1) and host CPU/memory (line 2).

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::proto::SessionState;

use super::super::app::App;
use super::{abbreviate_home, fmt_age, str_width, truncate};

// ── Status bar ────────────────────────────────────────────────────────────────

/// Human name of a session state.
pub fn state_name(state: SessionState) -> &'static str {
    match state {
        SessionState::Starting => "starting",
        SessionState::Working => "working",
        SessionState::NeedsApproval => "needs approval",
        SessionState::NeedsInput => "needs input",
        SessionState::Idle => "idle",
        SessionState::Error => "error",
        SessionState::Exited => "exited",
        SessionState::Unknown => "unknown",
    }
}

/// Line 1: session info — host:cwd, branch, model, context tokens, state, proxy.
pub(super) fn draw_status_session(frame: &mut Frame, app: &App, area: Rect) {
    let (left, left_style) = if !app.connected {
        (
            "daemon disconnected, reconnecting…".to_owned(),
            Style::default().fg(Color::Red),
        )
    } else if let Some(notice) = &app.notice {
        (notice.text.clone(), Style::default().fg(Color::Yellow))
    } else if let Some(v) = app.active_view() {
        let cwd = abbreviate_home(&v.cwd, &app.home);
        let location = if v.host == "local" {
            cwd
        } else {
            format!("{}:{}", v.host, cwd)
        };
        let mut parts = vec![location];
        if let Some(branch) = &v.branch {
            if !branch.is_empty() {
                parts.push(format!("⎇ {branch}"));
            }
        }
        if let Some(model) = &v.model {
            // Strip "claude-" prefix and truncate.
            let short = model.strip_prefix("claude-").unwrap_or(model);
            parts.push(truncate(short, 14));
        }
        if let Some(ctx) = v.context_tokens {
            parts.push(fmt_tokens_k(ctx));
        }
        parts.push(state_name(v.state).to_owned());
        // Uptime.
        if v.created_at > 0 && app.now >= v.created_at {
            parts.push(fmt_age(app.now.saturating_sub(v.created_at)));
        }
        // The session's proxy choice, not whether stats have been fetched.
        match &v.proxy {
            Some(name) => parts.push(format!("proxy:{name}")),
            None => parts.push("direct".to_owned()),
        }
        (parts.join(" · "), Style::default())
    } else {
        (String::new(), Style::default())
    };
    let width = area.width as usize;
    let left = truncate(&format!(" {left}"), width);
    let spans = vec![Span::styled(left, left_style)];
    let bar =
        Paragraph::new(Line::from(spans)).style(Style::default().add_modifier(Modifier::REVERSED));
    frame.render_widget(bar, area);
}

/// Line 2: machine stats — CPU/mem sparklines left, upgrade + "Alt+h help" right.
pub(super) fn draw_status_machine(frame: &mut Frame, app: &App, area: Rect) {
    let width = area.width as usize;
    let focused_host = app
        .active_view()
        .map(|v| v.host.clone())
        .unwrap_or_else(|| "local".to_owned());

    // Build left side: CPU and memory sparklines.
    let mut left = String::new();
    if let Some(ring) = app.host_stats.get(&focused_host) {
        let n = ring.len().min(10);
        if n > 0 {
            let cpu_vals: Vec<f32> = ring.iter().rev().take(n).rev().map(|s| s.cpu_pct).collect();
            let cpu_bar = sparkline(&cpu_vals, 100.0);
            let cpu_pct = ring.back().map(|s| s.cpu_pct).unwrap_or(0.0);
            left.push_str(&format!(" cpu {cpu_bar} {cpu_pct:.0}%"));

            if let Some(last) = ring.back() {
                if last.mem_total > 0 {
                    let mem_vals: Vec<f32> = ring
                        .iter()
                        .rev()
                        .take(n)
                        .rev()
                        .map(|s| s.mem_used as f32 / s.mem_total.max(1) as f32 * 100.0)
                        .collect();
                    let mem_bar = sparkline(&mem_vals, 100.0);
                    let used_gb = last.mem_used as f64 / 1_073_741_824.0;
                    let total_gb = last.mem_total as f64 / 1_073_741_824.0;
                    left.push_str(&format!("  mem {mem_bar} {used_gb:.1}/{total_gb:.0}G"));
                }
            }
        }
    }

    // Build right side: upgrade notice + "Alt+h help".
    let upgrade = app
        .upgrade_notice
        .as_deref()
        .map(|t| format!("↑ {t} "))
        .unwrap_or_default();
    let hint = "Alt+h help ";

    let left_w = str_width(&left);
    let right_w = str_width(&upgrade) + str_width(hint);
    let pad = width.saturating_sub(left_w + right_w);

    let mut spans = Vec::new();
    if !left.is_empty() {
        spans.push(Span::raw(left));
    }
    spans.push(Span::raw(" ".repeat(pad)));
    if !upgrade.is_empty() {
        spans.push(Span::styled(
            upgrade,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::DIM),
        ));
    }
    spans.push(Span::styled(
        hint,
        Style::default().add_modifier(Modifier::DIM),
    ));

    let bar =
        Paragraph::new(Line::from(spans)).style(Style::default().add_modifier(Modifier::REVERSED));
    frame.render_widget(bar, area);
}

/// Render a sparkline from `vals` (each 0..=`max`).
fn sparkline(vals: &[f32], max: f32) -> String {
    const BARS: &[char] = &['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    vals.iter()
        .map(|&v| {
            let idx = ((v / max.max(1.0)) * 7.0).clamp(0.0, 7.0) as usize;
            BARS[idx]
        })
        .collect()
}

/// Format context tokens: `143k`, `1.2M`, or raw for small values.
fn fmt_tokens_k(n: u64) -> String {
    if n >= 1_000_000 {
        format!("ctx {:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1000 {
        format!("ctx {}k", n / 1000)
    } else {
        format!("ctx {n}")
    }
}
