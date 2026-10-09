//! The two-line bottom bar: session info (line 1) and host CPU/memory (line 2).
//!
//! Every span carries an explicit foreground on the explicit [`BAR_BG`], so the
//! bar reads the same on dark and light terminal themes (reversed video would
//! turn the colors muddy). The colors are fixed 256-color indexes for the same
//! reason: named ANSI colors are remapped by the theme.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::proto::SessionState;

use super::super::app::App;
use super::super::sessions::SessionView;
use super::{abbreviate_home, fmt_age, str_width, truncate};

// ── Palette ───────────────────────────────────────────────────────────────────

const BAR_BG: Color = Color::Indexed(236);
const TEXT: Color = Color::Indexed(252);
const DIM: Color = Color::Indexed(245);
const GREEN: Color = Color::Indexed(114);
const YELLOW: Color = Color::Indexed(221);
const RED: Color = Color::Indexed(203);
const CYAN: Color = Color::Indexed(80);
const BLUE: Color = Color::Indexed(75);
const MAGENTA: Color = Color::Indexed(177);

/// Separator between the segments of line 1.
const SEPARATOR: &str = " · ";

fn style(fg: Color) -> Style {
    Style::default().fg(fg).bg(BAR_BG)
}

fn bold(fg: Color) -> Style {
    style(fg).add_modifier(Modifier::BOLD)
}

fn text(s: impl Into<String>) -> Span<'static> {
    Span::styled(s.into(), style(TEXT))
}

fn dim(s: impl Into<String>) -> Span<'static> {
    Span::styled(s.into(), style(DIM))
}

fn colored(s: impl Into<String>, fg: Color) -> Span<'static> {
    Span::styled(s.into(), style(fg))
}

fn bold_colored(s: impl Into<String>, fg: Color) -> Span<'static> {
    Span::styled(s.into(), bold(fg))
}

/// Color for a utilization percentage: green below 60, yellow below 85, red
/// from there up.
fn level_color(pct: f32) -> Color {
    if pct < 60.0 {
        GREEN
    } else if pct < 85.0 {
        YELLOW
    } else {
        RED
    }
}

/// The color of a session state; the same hues as the tab glyphs.
fn state_color(state: SessionState) -> Color {
    match state {
        SessionState::Working => CYAN,
        SessionState::NeedsInput => YELLOW,
        SessionState::NeedsApproval | SessionState::Error => RED,
        SessionState::Idle => GREEN,
        SessionState::Starting | SessionState::Exited | SessionState::Unknown => DIM,
    }
}

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
    let spans = if !app.connected {
        vec![bold_colored("daemon disconnected, reconnecting…", RED)]
    } else if let Some(notice) = &app.notice {
        vec![bold_colored(notice.text.clone(), YELLOW)]
    } else if let Some(v) = app.active_view() {
        join_segments(session_segments(app, v))
    } else {
        Vec::new()
    };
    let mut line = vec![text(" ")];
    line.extend(spans);
    draw_bar(frame, truncate_spans(line, area.width as usize), area);
}

/// The segments of line 1 for a session, each a run of spans.
fn session_segments(app: &App, v: &SessionView) -> Vec<Vec<Span<'static>>> {
    let cwd = bold_colored(abbreviate_home(&v.cwd, &app.home), TEXT);
    let mut segments = vec![if v.host == "local" {
        vec![cwd]
    } else {
        vec![bold_colored(format!("{}:", v.host), MAGENTA), cwd]
    }];
    if let Some(branch) = v.branch.as_deref().filter(|b| !b.is_empty()) {
        segments.push(vec![colored(format!("⎇ {branch}"), GREEN)]);
    }
    if let Some(model) = &v.model {
        // Strip "claude-" prefix and truncate.
        let short = model.strip_prefix("claude-").unwrap_or(model);
        segments.push(vec![bold_colored(truncate(short, 14), BLUE)]);
    }
    if let Some(ctx) = v.context_tokens {
        segments.push(vec![colored(fmt_tokens_k(ctx), CYAN)]);
    }
    segments.push(vec![colored(state_name(v.state), state_color(v.state))]);
    if v.created_at > 0 && app.now >= v.created_at {
        segments.push(vec![dim(fmt_age(app.now.saturating_sub(v.created_at)))]);
    }
    // The session's proxy choice, not whether stats have been fetched.
    segments.push(vec![match &v.proxy {
        Some(name) => colored(format!("⇄ proxy:{name}"), GREEN),
        None => dim("direct"),
    }]);
    segments
}

/// Flatten segments into spans, with a dim separator between them.
fn join_segments(segments: Vec<Vec<Span<'static>>>) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (i, segment) in segments.into_iter().enumerate() {
        if i > 0 {
            spans.push(dim(SEPARATOR));
        }
        spans.extend(segment);
    }
    spans
}

/// Line 2: machine stats — CPU/mem sparklines left, upgrade + "Alt+h help" right.
pub(super) fn draw_status_machine(frame: &mut Frame, app: &App, area: Rect) {
    let focused_host = app
        .active_view()
        .map(|v| v.host.clone())
        .unwrap_or_else(|| "local".to_owned());

    let mut left = vec![text(" ")];
    if let Some(ring) = app.host_stats.get(&focused_host) {
        let n = ring.len().min(10);
        if let Some(last) = ring.back().filter(|_| n > 0) {
            let cpu: Vec<f32> = ring.iter().rev().take(n).rev().map(|s| s.cpu_pct).collect();
            left.extend(metric_spans("cpu", &cpu, format!("{:.0}%", last.cpu_pct)));

            if last.mem_total > 0 {
                let mem: Vec<f32> = ring
                    .iter()
                    .rev()
                    .take(n)
                    .rev()
                    .map(|s| s.mem_used as f32 / s.mem_total.max(1) as f32 * 100.0)
                    .collect();
                let used_gb = last.mem_used as f64 / 1_073_741_824.0;
                let total_gb = last.mem_total as f64 / 1_073_741_824.0;
                left.push(text("  "));
                left.extend(metric_spans(
                    "mem",
                    &mem,
                    format!("{used_gb:.1}/{total_gb:.0}G"),
                ));
            }
        }
    }

    let spans = layout_line(
        left,
        upgrade_spans(app.upgrade_notice.as_deref()),
        area.width as usize,
    );
    draw_bar(frame, spans, area);
}

/// `cpu ▁▂▃▅ 42%`: a dim bold label, then the sparkline and the current value
/// (bold), both colored by the latest sample.
fn metric_spans(label: &str, history: &[f32], value: String) -> Vec<Span<'static>> {
    let color = level_color(history.last().copied().unwrap_or(0.0));
    vec![
        Span::styled(label.to_owned(), style(DIM).add_modifier(Modifier::BOLD)),
        text(" "),
        colored(sparkline(history, 100.0), color),
        text(" "),
        bold_colored(value, color),
    ]
}

/// The right-hand side of line 2: the upgrade notice (cyan), if any.
fn upgrade_spans(notice: Option<&str>) -> Vec<Span<'static>> {
    notice
        .map(|t| vec![colored(format!("↑ {t} "), CYAN)])
        .unwrap_or_default()
}

/// `left`, padding, then `upgrade` and the `Alt+h help` hint flush right.
///
/// The hint always wins: the upgrade notice is dropped when it doesn't fit
/// beside it, and `left` is cut (span-aware) to whatever room remains.
fn layout_line(
    left: Vec<Span<'static>>,
    upgrade: Vec<Span<'static>>,
    width: usize,
) -> Vec<Span<'static>> {
    let hint = vec![bold_colored("Alt+h", TEXT), dim(" help ")];
    let mut right = [upgrade, hint.clone()].concat();
    if spans_width(&right) >= width {
        right = hint;
    }
    let left = truncate_spans(left, width.saturating_sub(spans_width(&right)));
    let pad = width.saturating_sub(spans_width(&left) + spans_width(&right));
    [left, vec![text(" ".repeat(pad))], right].concat()
}

/// Paint `spans` as one full-width line of the bar.
fn draw_bar(frame: &mut Frame, spans: Vec<Span<'static>>, area: Rect) {
    frame.render_widget(Paragraph::new(Line::from(spans)).style(style(TEXT)), area);
}

// ── Span helpers ──────────────────────────────────────────────────────────────

fn spans_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|s| str_width(&s.content)).sum()
}

/// Cut `spans` to at most `max` columns, keeping each span's style. The span
/// that straddles the limit is shortened with `…`.
fn truncate_spans(spans: Vec<Span<'static>>, max: usize) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut left = max;
    for span in spans {
        let w = str_width(&span.content);
        if w <= left {
            left -= w;
            out.push(span);
            continue;
        }
        if left > 0 {
            out.push(Span::styled(truncate(&span.content, left), span.style));
        }
        break;
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(spans: &[Span<'_>]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn level_color_thresholds() {
        assert_eq!(level_color(0.0), GREEN);
        assert_eq!(level_color(59.9), GREEN);
        assert_eq!(level_color(60.0), YELLOW);
        assert_eq!(level_color(84.9), YELLOW);
        assert_eq!(level_color(85.0), RED);
        assert_eq!(level_color(100.0), RED);
    }

    #[test]
    fn sparkline_scales_and_clamps() {
        assert_eq!(sparkline(&[0.0, 50.0, 100.0, 250.0], 100.0), "▁▄██");
        assert_eq!(sparkline(&[], 100.0), "");
    }

    #[test]
    fn metric_is_colored_by_its_latest_sample() {
        let spans = metric_spans("cpu", &[10.0, 20.0, 90.0], "90%".into());
        assert_eq!(plain(&spans), "cpu ▁▂▇ 90%");
        // Sparkline and value share the latest sample's color; the value is bold.
        assert_eq!(spans[2].style.fg, Some(RED));
        assert_eq!(spans[4].style.fg, Some(RED));
        assert!(spans[4].style.add_modifier.contains(Modifier::BOLD));
        // A busy past doesn't matter once the load drops.
        let spans = metric_spans("cpu", &[99.0, 5.0], "5%".into());
        assert_eq!(spans[2].style.fg, Some(GREEN));
    }

    #[test]
    fn every_span_has_explicit_colors() {
        let mut spans = metric_spans("mem", &[70.0], "7.0/16G".into());
        spans.extend(upgrade_spans(Some("v1.2.3")));
        spans.extend(layout_line(vec![], vec![], 40));
        spans.extend(join_segments(vec![vec![dim("a")], vec![text("b")]]));
        for s in &spans {
            assert!(s.style.fg.is_some(), "no fg on {:?}", s.content);
            assert_eq!(s.style.bg, Some(BAR_BG), "no bg on {:?}", s.content);
        }
    }

    #[test]
    fn segments_are_joined_with_a_dim_separator() {
        let spans = join_segments(vec![vec![text("a")], vec![text("b"), text("c")]]);
        assert_eq!(plain(&spans), "a · bc");
        assert_eq!(spans[1].style.fg, Some(DIM));
    }

    #[test]
    fn truncate_spans_cuts_at_the_boundary_and_keeps_styles() {
        let spans = vec![colored("abc", GREEN), colored("defgh", RED)];
        assert_eq!(plain(&truncate_spans(spans.clone(), 20)), "abcdefgh");
        let cut = truncate_spans(spans.clone(), 6);
        assert_eq!(plain(&cut), "abcde…");
        assert_eq!(cut[1].style.fg, Some(RED));
        assert_eq!(plain(&truncate_spans(spans.clone(), 3)), "abc");
        assert_eq!(plain(&truncate_spans(spans, 0)), "");
    }

    #[test]
    fn layout_fills_the_width_and_keeps_the_hint_flush_right() {
        let left = vec![text(" cpu ▁ 3%")];
        let spans = layout_line(left, upgrade_spans(Some("v9")), 40);
        let line = plain(&spans);
        assert_eq!(str_width(&line), 40);
        assert!(line.starts_with(" cpu ▁ 3%"));
        assert!(line.ends_with("↑ v9 Alt+h help "), "{line:?}");
    }

    #[test]
    fn hint_stays_visible_when_narrow() {
        let left = vec![text(&"x".repeat(100))];
        for width in [60, 30, 17] {
            let line = plain(&layout_line(
                left.clone(),
                upgrade_spans(Some("v1.2.3")),
                width,
            ));
            assert_eq!(str_width(&line), width);
            assert!(line.ends_with("Alt+h help "), "{width}: {line:?}");
        }
        // Too narrow for both: the upgrade notice goes first.
        let line = plain(&layout_line(vec![], upgrade_spans(Some("v1.2.3")), 14));
        assert_eq!(line, "   Alt+h help ");
    }
}
