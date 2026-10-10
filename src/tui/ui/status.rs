//! The two-line bottom bar: session info (line 1) and gauges (line 2).
//!
//! Line 1 says where the session is and what it is doing: host and directory,
//! model, state, uptime, the proxy and its credential. Line 2 is the
//! instrument panel: host CPU and memory, the context gauge, the git working
//! tree, the credential's rate-limit windows and the conversation totals. Its
//! segments have priorities; a narrow bar drops whole segments, least
//! important first, and never cuts one mid-text.
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

use crate::proto::{GitStatus, SessionKind, SessionState};
use crate::proxy::api::{ModelsResponse, SessionCredential};

use super::super::app::{App, HostStatsSample};
use super::super::fmt::{
    abbreviate_home, fmt_age, fmt_tokens, fmt_window, short_model, spans_width, str_width,
    truncate,
};
use super::super::sessions::SessionView;

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
/// Separator between the segments of line 2.
const BAR_SEPARATOR: &str = " │ ";

/// The context window assumed when neither the proxy's catalogue nor the
/// model id says otherwise.
const DEFAULT_CONTEXT_WINDOW: u64 = 200_000;
/// Cells of the context gauge.
const CONTEXT_GAUGE: usize = 10;
/// Cells of the rate-limit gauge.
const USAGE_GAUGE: usize = 5;
/// Most samples a sparkline shows.
const SPARKLINE_SAMPLES: usize = 10;

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

/// A segment's name: dim and bold, like `cpu` or `ctx`.
fn label(s: &str) -> Span<'static> {
    Span::styled(s.to_owned(), style(DIM).add_modifier(Modifier::BOLD))
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

// ── Line 1: the session ───────────────────────────────────────────────────────

/// Line 1: session info — host:cwd, model, state, uptime, proxy and credential.
pub(super) fn draw_status_session(frame: &mut Frame, app: &App, area: Rect) {
    let spans = if !app.connected {
        vec![bold_colored("daemon disconnected, reconnecting…", RED)]
    } else if let Some(notice) = &app.notice {
        vec![bold_colored(notice.text.clone(), YELLOW)]
    } else if let Some(v) = app.active_view().filter(|v| v.kind == SessionKind::Shell) {
        join_segments(terminal_segments(app, v))
    } else if let Some(v) = app.active_view() {
        join_segments(session_segments(app, v))
    } else {
        Vec::new()
    };
    let mut line = vec![text(" ")];
    line.extend(spans);
    draw_bar(frame, truncate_spans(line, area.width as usize), area);
}

/// The segments of line 1 for a session, each a run of spans. The branch,
/// context and rate-limit figures live on line 2.
fn session_segments(app: &App, v: &SessionView) -> Vec<Vec<Span<'static>>> {
    let cwd = bold_colored(abbreviate_home(&v.cwd, &app.home), TEXT);
    let mut segments = vec![if v.host == "local" {
        vec![cwd]
    } else {
        vec![bold_colored(format!("{}:", v.host), MAGENTA), cwd]
    }];
    if let Some(model) = &v.model {
        segments.push(vec![bold_colored(truncate(short_model(model), 14), BLUE)]);
    }
    segments.push(vec![colored(v.state.name(), state_color(v.state))]);
    if v.created_at > 0 && app.now >= v.created_at {
        segments.push(vec![dim(fmt_age(app.now.saturating_sub(v.created_at)))]);
    }
    // The session's proxy choice, not whether stats have been fetched.
    segments.push(vec![match &v.proxy {
        Some(name) => colored(format!("⇄ proxy:{name}"), GREEN),
        None => dim("direct"),
    }]);
    if let Some(cred) = app.session_credential(v) {
        segments.extend(credential_segments(cred, app.now));
    }
    segments
}

/// The credential the proxy reports for the session: `work-max (max)` and a
/// short-lived switch notice. Empty when the proxy sent nothing to name.
pub(super) fn credential_segments(
    cred: &SessionCredential,
    now: u64,
) -> Vec<Vec<Span<'static>>> {
    let mut segments = Vec::new();
    if let Some(name) = cred.name() {
        let mut spans = vec![bold_colored(truncate(name, 24), TEXT)];
        if let Some(plan) = cred.plan() {
            spans.push(dim(format!(" ({plan})")));
        }
        segments.push(spans);
    }
    if let Some(age) = cred.recent_switch_age(now) {
        segments.push(vec![Span::styled(
            format!("⇆ switched {} ago", fmt_age(age)),
            style(YELLOW).add_modifier(Modifier::DIM),
        )]);
    }
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

/// Line 1 for a terminal tab: `$ terminal · host:cwd · uptime`. No model,
/// context or proxy; the cwd is where it was launched, not tracked.
fn terminal_segments(app: &App, v: &SessionView) -> Vec<Vec<Span<'static>>> {
    let mut segments = vec![
        vec![bold_colored("$ terminal", GREEN)],
        vec![
            bold_colored(format!("{}:", v.host), MAGENTA),
            bold_colored(abbreviate_home(&v.cwd, &app.home), TEXT),
        ],
    ];
    if v.created_at > 0 && app.now >= v.created_at {
        segments.push(vec![dim(fmt_age(app.now - v.created_at))]);
    }
    segments
}

// ── Line 2: the instrument panel ──────────────────────────────────────────────

/// A run of spans and how long it survives a narrow bar: priority 0 is kept
/// the longest, the highest number goes first.
struct Segment {
    priority: u8,
    spans: Vec<Span<'static>>,
}

/// Drop order of the line-2 segments (higher goes first).
const PRIO_CPU: u8 = 0;
const PRIO_CONTEXT: u8 = 0;
const PRIO_GIT: u8 = 1;
const PRIO_MEM: u8 = 2;
const PRIO_FIVE_HOUR: u8 = 3;
const PRIO_CONVERSATION: u8 = 4;
const PRIO_SEVEN_DAY: u8 = 5;

/// Line 2: host CPU/mem, context gauge, git, rate limits and conversation
/// totals left; upgrade notice and "Alt+h help" right.
pub(super) fn draw_status_machine(frame: &mut Frame, app: &App, area: Rect) {
    let spans = layout_segments(
        machine_segments(app),
        upgrade_spans(app.upgrade_notice.as_deref()),
        area.width as usize,
    );
    draw_bar(frame, spans, area);
}

/// Every line-2 segment for the active session, in display order. A terminal
/// tab has no conversation: it shows the host and the working tree only.
fn machine_segments(app: &App) -> Vec<Segment> {
    let view = app.active_view();
    let host = view.map_or("local", |v| v.host.as_str());
    let mut segments = host_segments(app.host_stats.get(host).map(|r| r.iter()));
    let Some(v) = view else {
        return segments;
    };
    let claude = v.kind == SessionKind::Claude;
    if claude {
        segments.extend(context_segment(
            v.context_tokens,
            v.model.as_deref(),
            app.session_models(v),
        ));
    }
    segments.extend(git_segment(v.git.as_ref(), v.branch.as_deref()));
    if claude {
        let cred = app.session_credential(v);
        segments.extend(cred.and_then(five_hour_segment));
        segments.extend(conversation_segment(v.turns, v.output_tokens));
        segments.extend(cred.and_then(seven_day_segment));
    }
    segments
}

/// `cpu ▁▂▃▅ 42%` and `mem ▅▅ 26.3/46G` from the host's sample ring.
fn host_segments<'a>(
    ring: Option<impl DoubleEndedIterator<Item = &'a HostStatsSample>>,
) -> Vec<Segment> {
    let Some(ring) = ring else {
        return Vec::new();
    };
    let recent: Vec<&HostStatsSample> = ring.rev().take(SPARKLINE_SAMPLES).collect();
    let Some(last) = recent.first() else {
        return Vec::new();
    };
    let cpu: Vec<f32> = recent.iter().rev().map(|s| s.cpu_pct).collect();
    let mut segments = vec![Segment {
        priority: PRIO_CPU,
        spans: metric_spans("cpu", &cpu, format!("{:.0}%", last.cpu_pct)),
    }];
    if last.mem_total > 0 {
        let mem: Vec<f32> = recent
            .iter()
            .rev()
            .map(|s| s.mem_used as f32 / s.mem_total.max(1) as f32 * 100.0)
            .collect();
        let used_gb = last.mem_used as f64 / 1_073_741_824.0;
        let total_gb = last.mem_total as f64 / 1_073_741_824.0;
        segments.push(Segment {
            priority: PRIO_MEM,
            spans: metric_spans("mem", &mem, format!("{used_gb:.1}/{total_gb:.0}G")),
        });
    }
    segments
}

/// `cpu ▁▂▃▅ 42%`: a label, then the sparkline and the current value (bold),
/// both colored by the latest sample.
fn metric_spans(name: &str, history: &[f32], value: String) -> Vec<Span<'static>> {
    let color = level_color(history.last().copied().unwrap_or(0.0));
    vec![
        label(name),
        text(" "),
        colored(sparkline(history, 100.0), color),
        text(" "),
        bold_colored(value, color),
    ]
}

/// `ctx ██████▎░░░ 62% 620k/1M`: the last turn's context against the model's
/// window, colored by how full it is. Nothing until a turn has happened.
fn context_segment(
    used: Option<u64>,
    model: Option<&str>,
    models: Option<&ModelsResponse>,
) -> Option<Segment> {
    let used = used?;
    let window = context_window(model.unwrap_or_default(), models);
    let pct = used as f32 / window.max(1) as f32 * 100.0;
    let color = level_color(pct);
    let mut spans = vec![label("ctx"), text(" ")];
    spans.extend(gauge_spans(pct / 100.0, CONTEXT_GAUGE, color));
    spans.push(text(" "));
    spans.push(bold_colored(format!("{}%", pct.floor() as i64), color));
    spans.push(dim(format!(
        " {}/{}",
        fmt_tokens(used.min(i64::MAX as u64) as i64),
        fmt_window(window)
    )));
    Some(Segment {
        priority: PRIO_CONTEXT,
        spans,
    })
}

/// A model's context window: the proxy's catalogue when it knows the model,
/// else 1M for a `[1m]`/`1m` model id, else [`DEFAULT_CONTEXT_WINDOW`].
pub fn context_window(model: &str, models: Option<&ModelsResponse>) -> u64 {
    let known = models.and_then(|m| {
        m.data
            .iter()
            .filter(|e| e.max_input_tokens > 0 && !e.id.is_empty())
            .find(|e| e.id == model || model.starts_with(&e.id))
    });
    if let Some(entry) = known {
        return entry.max_input_tokens as u64;
    }
    let one_million = model
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|part| part.eq_ignore_ascii_case("1m"));
    if one_million {
        1_000_000
    } else {
        DEFAULT_CONTEXT_WINDOW
    }
}

/// `⎇ main ↑2 ↓1 ●1 ✚3 …2`: the branch, how it stands against its upstream,
/// and the staged, modified, untracked and unmerged path counts. A clean
/// tree in sync with its upstream gets a `✓`. With only a branch name (an
/// old daemon), just that.
fn git_segment(git: Option<&GitStatus>, branch: Option<&str>) -> Option<Segment> {
    let mut spans = vec![colored("⎇ ", GREEN)];
    let Some(git) = git else {
        let branch = branch.filter(|b| !b.is_empty())?;
        spans.push(bold_colored(truncate(branch, 24), GREEN));
        return Some(Segment {
            priority: PRIO_GIT,
            spans,
        });
    };
    match git.branch() {
        Some(name) => spans.push(bold_colored(truncate(name, 24), GREEN)),
        None if git.head.is_empty() => spans.push(dim("detached")),
        None => spans.push(dim(format!("@{}", git.head))),
    }
    if git.upstream {
        if git.ahead > 0 {
            spans.push(colored(format!(" ↑{}", git.ahead), CYAN));
        }
        if git.behind > 0 {
            spans.push(colored(format!(" ↓{}", git.behind), MAGENTA));
        }
    }
    if git.staged > 0 {
        spans.push(colored(format!(" ●{}", git.staged), BLUE));
    }
    if git.unstaged > 0 {
        spans.push(colored(format!(" ✚{}", git.unstaged), YELLOW));
    }
    if git.untracked > 0 {
        spans.push(dim(format!(" …{}", git.untracked)));
    }
    if git.conflicts > 0 {
        spans.push(bold_colored(format!(" ✖{}", git.conflicts), RED));
    }
    if !git.is_dirty() && git.upstream && git.ahead == 0 && git.behind == 0 {
        spans.push(colored(" ✓", GREEN));
    }
    Some(Segment {
        priority: PRIO_GIT,
        spans,
    })
}

/// The credential's 5-hour rate-limit window: `5h ██▍░░ 37%`.
fn five_hour_segment(cred: &SessionCredential) -> Option<Segment> {
    let pct = cred.five_hour_pct()? as f32;
    let color = level_color(pct);
    let mut spans = vec![label("5h"), text(" ")];
    spans.extend(gauge_spans(pct / 100.0, USAGE_GAUGE, color));
    spans.push(text(" "));
    spans.push(bold_colored(format!("{}%", pct.floor() as i64), color));
    Some(Segment {
        priority: PRIO_FIVE_HOUR,
        spans,
    })
}

/// The credential's 7-day rate-limit window: `7d 12%`.
fn seven_day_segment(cred: &SessionCredential) -> Option<Segment> {
    let pct = cred.seven_day_pct()? as f32;
    Some(Segment {
        priority: PRIO_SEVEN_DAY,
        spans: vec![
            label("7d"),
            text(" "),
            bold_colored(format!("{}%", pct.floor() as i64), level_color(pct)),
        ],
    })
}

/// `14 turns · 48k out`: the conversation so far.
fn conversation_segment(turns: Option<u32>, output_tokens: Option<u64>) -> Option<Segment> {
    let mut spans = Vec::new();
    if let Some(n) = turns {
        spans.push(bold_colored(n.to_string(), TEXT));
        spans.push(dim(if n == 1 { " turn" } else { " turns" }));
    }
    if let Some(n) = output_tokens {
        if !spans.is_empty() {
            spans.push(dim(SEPARATOR));
        }
        spans.push(bold_colored(fmt_tokens(n.min(i64::MAX as u64) as i64), TEXT));
        spans.push(dim(" out"));
    }
    (!spans.is_empty()).then_some(Segment {
        priority: PRIO_CONVERSATION,
        spans,
    })
}

/// A `width`-cell bar filled to `frac` (0..=1) in `color`, the rest dim. The
/// last filled cell is a partial block, so the gauge moves in eighths.
fn gauge_spans(frac: f32, width: usize, color: Color) -> Vec<Span<'static>> {
    const EIGHTHS: [char; 8] = ['█', '▏', '▎', '▍', '▌', '▋', '▊', '▉'];
    let cells = frac.clamp(0.0, 1.0) * width as f32;
    let full = cells.floor() as usize;
    let eighths = ((cells - full as f32) * 8.0).round() as usize;
    let mut filled = "█".repeat(full);
    let mut used = full;
    if eighths > 0 && used < width {
        filled.push(EIGHTHS[eighths % 8]);
        used += 1;
    }
    vec![colored(filled, color), dim("░".repeat(width - used))]
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

/// The right-hand side of line 2: the upgrade notice (cyan), if any.
fn upgrade_spans(notice: Option<&str>) -> Vec<Span<'static>> {
    notice
        .map(|t| vec![colored(format!("↑ {t} "), CYAN)])
        .unwrap_or_default()
}

/// `segments` joined by dim `│`, then `upgrade` and the `Alt+h help` hint
/// flush right. Segments that don't fit are dropped whole, lowest priority
/// first (the later one among equals); the last one left is cut.
fn layout_segments(
    mut segments: Vec<Segment>,
    upgrade: Vec<Span<'static>>,
    width: usize,
) -> Vec<Span<'static>> {
    let room = width.saturating_sub(spans_width(&right_side(upgrade.clone(), width)));
    loop {
        let left = join_bar(&segments);
        if spans_width(&left) <= room || segments.len() <= 1 {
            return layout_line(left, upgrade, width);
        }
        let victim = segments
            .iter()
            .enumerate()
            .max_by_key(|(_, s)| s.priority)
            .map(|(i, _)| i)
            .expect("at least two segments");
        segments.remove(victim);
    }
}

/// A leading space, then the segments with a dim `│` between them.
fn join_bar(segments: &[Segment]) -> Vec<Span<'static>> {
    let mut spans = vec![text(" ")];
    for (i, segment) in segments.iter().enumerate() {
        if i > 0 {
            spans.push(dim(BAR_SEPARATOR));
        }
        spans.extend(segment.spans.iter().cloned());
    }
    spans
}

/// `upgrade` and the hint, or the hint alone when both don't fit in `width`.
fn right_side(upgrade: Vec<Span<'static>>, width: usize) -> Vec<Span<'static>> {
    let hint = vec![bold_colored("Alt+h", TEXT), dim(" help ")];
    let both = [upgrade, hint.clone()].concat();
    if spans_width(&both) >= width {
        hint
    } else {
        both
    }
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
    let right = right_side(upgrade, width);
    let left = truncate_spans(left, width.saturating_sub(spans_width(&right)));
    let pad = width.saturating_sub(spans_width(&left) + spans_width(&right));
    [left, vec![text(" ".repeat(pad))], right].concat()
}

/// Paint `spans` as one full-width line of the bar.
fn draw_bar(frame: &mut Frame, spans: Vec<Span<'static>>, area: Rect) {
    frame.render_widget(Paragraph::new(Line::from(spans)).style(style(TEXT)), area);
}

// ── Span helpers ──────────────────────────────────────────────────────────────

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::api::{CredentialInfo, CredentialUtilization, ModelEntry};
    use crate::tui::proxy_state::{ProxyStatus, SessionCred};

    fn plain(spans: &[Span<'_>]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn segment_text(seg: &Segment) -> String {
        plain(&seg.spans)
    }

    fn find<'a>(spans: &'a [Span<'a>], content: &str) -> &'a Span<'a> {
        spans
            .iter()
            .find(|s| s.content == content)
            .unwrap_or_else(|| panic!("no span {content:?} in {:?}", plain(spans)))
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

    fn cred(five_hour: Option<f64>, switched_at: Option<&str>) -> SessionCredential {
        SessionCredential {
            credential: CredentialInfo {
                label: "work-max".into(),
                plan: "max".into(),
                ..Default::default()
            },
            switched_at: switched_at.map(str::to_owned),
            utilization: five_hour.map(|p| CredentialUtilization {
                five_hour_pct: Some(p),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    // ── line 1 ───────────────────────────────────────────────────────────────

    #[test]
    fn credential_shows_label_and_plan_but_leaves_the_windows_to_line_two() {
        let segs = credential_segments(&cred(Some(37.5), None), 0);
        let spans = join_segments(segs);
        assert_eq!(plain(&spans), "work-max (max)");
        assert_eq!(find(&spans, " (max)").style.fg, Some(DIM));
    }

    #[test]
    fn credential_without_utilization_or_plan_is_just_the_label() {
        let mut c = cred(None, None);
        c.credential.plan.clear();
        assert_eq!(plain(&join_segments(credential_segments(&c, 0))), "work-max");
        // The id stands in for a missing label; nothing to name shows nothing.
        c.credential.label.clear();
        c.credential.id = "cred_ab12".into();
        assert_eq!(plain(&join_segments(credential_segments(&c, 0))), "cred_ab12");
        c.credential.id.clear();
        assert!(credential_segments(&c, 0).is_empty());
    }

    #[test]
    fn switch_notice_lasts_ten_minutes() {
        let at = "2026-10-09T10:07:02Z";
        let t0 = crate::proxy::api::parse_rfc3339(at).unwrap();
        let line = |now| plain(&join_segments(credential_segments(&cred(None, Some(at)), now)));
        assert_eq!(line(t0 + 180), "work-max (max) · ⇆ switched 3m ago");
        assert_eq!(line(t0 + 599), "work-max (max) · ⇆ switched 9m ago");
        assert_eq!(line(t0 + 600), "work-max (max)");
        let spans = join_segments(credential_segments(&cred(None, Some(at)), t0 + 5));
        let sw = spans.iter().find(|s| s.content.contains("switched")).unwrap();
        assert_eq!(sw.style.fg, Some(YELLOW));
        assert!(sw.style.add_modifier.contains(Modifier::DIM));
    }

    /// An app with one active claude session on a proxy, in a repo, with a
    /// conversation under way and host stats.
    fn demo_app() -> App {
        let mut app = App::new(crate::tui::app::AppConfig {
            size: (120, 30),
            ..crate::tui::test_support::config()
        });
        app.now = 1_000;
        app.sessions.push(SessionView {
            name: Some("s".into()),
            state: SessionState::Idle,
            claude_session_id: Some("c1".into()),
            attached: true,
            proxy: Some("work".into()),
            branch: Some("main".into()),
            model: Some("claude-opus-5-5[1m]".into()),
            context_tokens: Some(620_000),
            git: Some(GitStatus {
                head: "main".into(),
                upstream: true,
                ahead: 2,
                behind: 1,
                unstaged: 3,
                ..GitStatus::default()
            }),
            output_tokens: Some(48_200),
            turns: Some(14),
            ..SessionView::new(uuid::Uuid::new_v4(), "local", "/srv", SessionKind::Claude, (10, 40))
        });
        app.active = Some(0);
        for (cpu, mem) in [(10.0, 20u64), (30.0, 25), (23.0, 26)] {
            app.on_host_stats("local".into(), cpu, mem << 30, 46 << 30);
        }
        app.session_creds.insert(
            app.sessions[0].id,
            SessionCred {
                claude_session_id: "c1".into(),
                cred: Some(SessionCredential {
                    utilization: Some(CredentialUtilization {
                        five_hour_pct: Some(37.0),
                        seven_day_pct: Some(12.0),
                        ..Default::default()
                    }),
                    ..cred(None, None)
                }),
                asked_at: 0,
            },
        );
        app
    }

    #[test]
    fn status_line_appends_the_credential_after_the_proxy_badge() {
        let mut app = demo_app();
        app.session_creds.clear();
        let line = |app: &App| plain(&join_segments(session_segments(app, &app.sessions[0])));
        assert_eq!(line(&app), "/srv · opus-5-5[1m] · idle · ⇄ proxy:work");
        // Unknown yet: nothing extra.
        app.session_creds.insert(
            app.sessions[0].id,
            SessionCred {
                claude_session_id: "c1".into(),
                cred: None,
                asked_at: 0,
            },
        );
        assert!(line(&app).ends_with("⇄ proxy:work"));
        app.session_creds.get_mut(&app.sessions[0].id).unwrap().cred =
            Some(cred(Some(37.5), None));
        assert!(
            line(&app).ends_with("⇄ proxy:work · work-max (max)"),
            "{}",
            line(&app)
        );
        // A cached credential of a previous conversation is not shown.
        app.sessions[0].claude_session_id = Some("c2".into());
        assert!(line(&app).ends_with("⇄ proxy:work"));
    }

    // ── gauges ───────────────────────────────────────────────────────────────

    #[test]
    fn gauge_fills_in_eighths_and_keeps_its_width() {
        for (frac, want) in [
            (0.0, "░░░░░░░░░░"),
            (0.62, "██████▎░░░"),
            (0.65, "██████▌░░░"),
            (0.5, "█████░░░░░"),
            (0.99, "█████████▉"),
            (1.0, "██████████"),
            (7.0, "██████████"),
            (-1.0, "░░░░░░░░░░"),
        ] {
            let spans = gauge_spans(frac, 10, RED);
            assert_eq!(plain(&spans), want, "{frac}");
            assert_eq!(spans_width(&spans), 10, "{frac}");
            assert_eq!(spans[0].style.fg, Some(RED));
            assert_eq!(spans[1].style.fg, Some(DIM));
        }
        assert_eq!(plain(&gauge_spans(0.37, 5, GREEN)), "█▉░░░");
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
    fn host_segments_show_the_last_ten_samples_oldest_first() {
        let samples: Vec<HostStatsSample> = (0..14)
            .map(|i| HostStatsSample {
                cpu_pct: i as f32 * 7.0,
                mem_used: 8 << 30,
                mem_total: 16 << 30,
            })
            .collect();
        let segs = host_segments(Some(samples.iter()));
        assert_eq!(segs.len(), 2);
        assert_eq!(segment_text(&segs[0]), "cpu ▂▃▃▄▄▅▅▆▆▇ 91%");
        assert_eq!(segment_text(&segs[1]), "mem ▄▄▄▄▄▄▄▄▄▄ 8.0/16G");
        assert_eq!((segs[0].priority, segs[1].priority), (PRIO_CPU, PRIO_MEM));
        // No total: no memory segment. No samples: nothing.
        let one = [HostStatsSample {
            cpu_pct: 3.0,
            mem_used: 0,
            mem_total: 0,
        }];
        assert_eq!(host_segments(Some(one.iter())).len(), 1);
        assert!(host_segments(Some([].iter())).is_empty());
        assert!(host_segments(None::<std::slice::Iter<HostStatsSample>>).is_empty());
    }

    // ── context ──────────────────────────────────────────────────────────────

    fn catalogue(entries: &[(&str, i64)]) -> ModelsResponse {
        ModelsResponse {
            data: entries
                .iter()
                .map(|(id, max)| ModelEntry {
                    id: (*id).to_owned(),
                    max_input_tokens: *max,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn context_window_prefers_the_catalogue_then_the_model_id_then_200k() {
        let models = catalogue(&[
            ("claude-opus-5-5", 500_000),
            ("claude-sonnet-4-5", 0),
            ("", 7),
        ]);
        assert_eq!(context_window("claude-opus-5-5", Some(&models)), 500_000);
        // A dated id matches its catalogue family.
        assert_eq!(
            context_window("claude-opus-5-5-20260101", Some(&models)),
            500_000
        );
        // Unknown size in the catalogue: fall through to the id.
        assert_eq!(context_window("claude-sonnet-4-5", Some(&models)), 200_000);
        assert_eq!(context_window("claude-sonnet-4-5[1m]", Some(&models)), 1_000_000);
        assert_eq!(context_window("claude-opus-5-5[1m]", None), 1_000_000);
        assert_eq!(context_window("claude-sonnet-4-5-1m", None), 1_000_000);
        assert_eq!(context_window("CLAUDE-X-1M", None), 1_000_000);
        // `1m` has to be a whole token: a date is not a window.
        assert_eq!(context_window("claude-opus-4-1-20250805", None), 200_000);
        assert_eq!(context_window("claude-haiku-4-5", None), 200_000);
        assert_eq!(context_window("", None), 200_000);
    }

    #[test]
    fn context_segment_is_a_colored_gauge_with_bold_percent() {
        let seg = context_segment(Some(620_000), Some("claude-opus-5-5[1m]"), None).unwrap();
        assert_eq!(segment_text(&seg), "ctx ██████▎░░░ 62% 620k/1M");
        assert_eq!(seg.priority, PRIO_CONTEXT);
        let pct = find(&seg.spans, "62%");
        assert_eq!(pct.style.fg, Some(YELLOW));
        assert!(pct.style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(seg.spans[2].style.fg, Some(YELLOW), "gauge shares the color");
        assert_eq!(find(&seg.spans, " 620k/1M").style.fg, Some(DIM));

        let seg = context_segment(Some(40_000), Some("claude-haiku-4-5"), None).unwrap();
        assert_eq!(segment_text(&seg), "ctx ██░░░░░░░░ 20% 40k/200k");
        assert_eq!(find(&seg.spans, "20%").style.fg, Some(GREEN));

        let seg = context_segment(Some(190_000), None, None).unwrap();
        assert_eq!(segment_text(&seg), "ctx █████████▌ 95% 190k/200k");
        assert_eq!(find(&seg.spans, "95%").style.fg, Some(RED));

        // Over the window (a different model than the catalogue thinks): capped.
        let seg = context_segment(Some(300_000), Some("x"), None).unwrap();
        assert_eq!(segment_text(&seg), "ctx ██████████ 150% 300k/200k");
        assert!(context_segment(None, Some("x"), None).is_none());
    }

    // ── git ──────────────────────────────────────────────────────────────────

    fn git(ahead: u32, behind: u32) -> GitStatus {
        GitStatus {
            head: "main".into(),
            upstream: true,
            ahead,
            behind,
            ..GitStatus::default()
        }
    }

    #[test]
    fn git_segment_shows_branch_upstream_delta_and_dirty_counts() {
        let status = GitStatus {
            staged: 1,
            unstaged: 3,
            untracked: 2,
            conflicts: 1,
            ..git(2, 1)
        };
        let seg = git_segment(Some(&status), None).unwrap();
        assert_eq!(segment_text(&seg), "⎇ main ↑2 ↓1 ●1 ✚3 …2 ✖1");
        assert_eq!(seg.priority, PRIO_GIT);
        assert_eq!(find(&seg.spans, "main").style.fg, Some(GREEN));
        assert_eq!(find(&seg.spans, " ↑2").style.fg, Some(CYAN));
        assert_eq!(find(&seg.spans, " ↓1").style.fg, Some(MAGENTA));
        assert_eq!(find(&seg.spans, " ●1").style.fg, Some(BLUE));
        assert_eq!(find(&seg.spans, " ✚3").style.fg, Some(YELLOW));
        assert_eq!(find(&seg.spans, " …2").style.fg, Some(DIM));
        assert_eq!(find(&seg.spans, " ✖1").style.fg, Some(RED));
    }

    #[test]
    fn git_segment_variants() {
        // Clean and in sync with the upstream: a check mark.
        assert_eq!(segment_text(&git_segment(Some(&git(0, 0)), None).unwrap()), "⎇ main ✓");
        // Clean, ahead only.
        assert_eq!(segment_text(&git_segment(Some(&git(3, 0)), None).unwrap()), "⎇ main ↑3");
        // No upstream: no delta, no check mark.
        let local = GitStatus {
            upstream: false,
            ahead: 5,
            ..git(0, 0)
        };
        assert_eq!(segment_text(&git_segment(Some(&local), None).unwrap()), "⎇ main");
        // Detached: the short commit id, dim.
        let detached = GitStatus {
            head: "9f8e7d6".into(),
            detached: true,
            untracked: 1,
            ..GitStatus::default()
        };
        let seg = git_segment(Some(&detached), None).unwrap();
        assert_eq!(segment_text(&seg), "⎇ @9f8e7d6 …1");
        assert_eq!(find(&seg.spans, "@9f8e7d6").style.fg, Some(DIM));
        // Only a branch name (old daemon): just that. Nothing: nothing.
        assert_eq!(segment_text(&git_segment(None, Some("dev")).unwrap()), "⎇ dev");
        assert!(git_segment(None, Some("")).is_none());
        assert!(git_segment(None, None).is_none());
        // A long branch name is cut.
        let long = GitStatus {
            head: "feature/very-long-branch-name-here".into(),
            ..GitStatus::default()
        };
        assert_eq!(
            segment_text(&git_segment(Some(&long), None).unwrap()),
            "⎇ feature/very-long-branc…"
        );
    }

    // ── proxy and conversation ───────────────────────────────────────────────

    #[test]
    fn usage_segments_gauge_the_five_hour_window_and_name_the_seven_day_one() {
        let c = SessionCredential {
            utilization: Some(CredentialUtilization {
                five_hour_pct: Some(37.9),
                seven_day_pct: Some(91.0),
                ..Default::default()
            }),
            ..cred(None, None)
        };
        let five = five_hour_segment(&c).unwrap();
        let seven = seven_day_segment(&c).unwrap();
        assert_eq!(segment_text(&five), "5h █▉░░░ 37%");
        assert_eq!(segment_text(&seven), "7d 91%");
        assert_eq!((five.priority, seven.priority), (PRIO_FIVE_HOUR, PRIO_SEVEN_DAY));
        assert_eq!(find(&five.spans, "37%").style.fg, Some(GREEN));
        assert_eq!(find(&seven.spans, "91%").style.fg, Some(RED));
        for (value, color) in [(72.0, YELLOW), (91.0, RED)] {
            let c = cred(Some(value), None);
            assert!(seven_day_segment(&c).is_none(), "no 7d figure");
            let five = five_hour_segment(&c).unwrap();
            let pct = five.spans.iter().find(|s| s.content.ends_with('%')).unwrap();
            assert_eq!(pct.style.fg, Some(color), "{value}");
        }
        assert!(five_hour_segment(&cred(None, None)).is_none());
    }

    #[test]
    fn conversation_segment_counts_turns_and_output() {
        let seg = conversation_segment(Some(14), Some(48_200)).unwrap();
        assert_eq!(segment_text(&seg), "14 turns · 48k out");
        assert_eq!(seg.priority, PRIO_CONVERSATION);
        assert!(find(&seg.spans, "14").style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(
            segment_text(&conversation_segment(Some(1), None).unwrap()),
            "1 turn"
        );
        assert_eq!(
            segment_text(&conversation_segment(None, Some(1_250_000)).unwrap()),
            "1.2M out"
        );
        assert!(conversation_segment(None, None).is_none());
    }

    // ── the whole line ───────────────────────────────────────────────────────

    fn render(app: &App, width: usize) -> String {
        plain(&layout_segments(
            machine_segments(app),
            upgrade_spans(app.upgrade_notice.as_deref()),
            width,
        ))
    }

    #[test]
    fn claude_tab_shows_every_segment_when_wide() {
        let app = demo_app();
        let line = render(&app, 200);
        assert_eq!(str_width(&line), 200);
        assert!(
            line.starts_with(
                " cpu ▁▃▂ 23% │ mem ▄▄▄ 26.0/46G │ ctx ██████▎░░░ 62% 620k/1M │ ⎇ main ↑2 ↓1 ✚3 │ 5h █▉░░░ 37% │ 14 turns · 48k out │ 7d 12%"
            ),
            "{line:?}"
        );
        assert!(line.ends_with("Alt+h help "), "{line:?}");
        // The catalogue's window wins over the model id.
        let mut app = app;
        app.proxy_status.insert(
            "work".into(),
            ProxyStatus {
                models: Some(catalogue(&[("claude-opus-5-5", 800_000)])),
                ..Default::default()
            },
        );
        assert!(render(&app, 200).contains("77% 620k/800k"), "{}", render(&app, 200));
    }

    #[test]
    fn narrow_bars_drop_whole_segments_least_important_first() {
        let app = demo_app();
        let at = |w| render(&app, w);
        // 160: everything fits.
        let line = at(160);
        assert_eq!(str_width(&line), 160);
        assert!(line.contains("7d 12%") && line.contains("14 turns"), "{line:?}");
        // 120: the 7-day window and the conversation totals go; 5h stays.
        let line = at(120);
        assert_eq!(str_width(&line), 120);
        assert!(line.contains("5h") && line.contains("⎇ main"), "{line:?}");
        assert!(!line.contains("7d") && !line.contains("turns"), "{line:?}");
        // 100: the conversation totals and the 5-hour window go; git and mem stay.
        let line = at(100);
        assert_eq!(str_width(&line), 100);
        assert!(line.contains("⎇ main") && line.contains("mem"), "{line:?}");
        assert!(!line.contains("5h") && !line.contains("turns"), "{line:?}");
        // 80: mem goes before git; ctx and cpu stay.
        let line = at(80);
        assert_eq!(str_width(&line), 80);
        assert!(line.contains("cpu") && line.contains("ctx") && line.contains("⎇"), "{line:?}");
        assert!(!line.contains("mem"), "{line:?}");
        // 60: cpu and ctx only.
        let line = at(60);
        assert_eq!(str_width(&line), 60);
        assert!(line.contains("cpu") && line.contains("ctx"), "{line:?}");
        assert!(!line.contains("⎇"), "{line:?}");
        assert!(line.ends_with("Alt+h help "), "{line:?}");
        // Narrower than one segment: the last one is cut, the hint stays.
        let line = at(24);
        assert_eq!(str_width(&line), 24);
        assert!(line.ends_with("Alt+h help "), "{line:?}");
        assert!(!line.contains("│"), "{line:?}");
    }

    #[test]
    fn terminal_tab_shows_host_and_git_only() {
        let mut app = demo_app();
        app.sessions[0].kind = SessionKind::Shell;
        let line = render(&app, 200);
        assert!(line.contains("cpu") && line.contains("mem") && line.contains("⎇ main ↑2 ↓1 ✚3"));
        for claude_only in ["ctx", "5h", "7d", "turns", "out"] {
            assert!(!line.contains(claude_only), "{claude_only} on a terminal: {line:?}");
        }
        // A claude tab without a transcript or credential yet: host and git.
        let mut app = demo_app();
        let v = &mut app.sessions[0];
        (v.context_tokens, v.output_tokens, v.turns) = (None, None, None);
        app.session_creds.clear();
        let line = render(&app, 200);
        assert!(line.contains("⎇ main") && !line.contains("ctx") && !line.contains("5h"));
        // No session at all: just the host.
        app.sessions.clear();
        app.active = None;
        let line = render(&app, 80);
        assert!(line.starts_with(" cpu") && line.contains("mem"), "{line:?}");
        assert!(!line.contains("⎇") && !line.contains("ctx"), "{line:?}");
    }

    #[test]
    fn every_span_has_explicit_colors() {
        let app = demo_app();
        let mut spans = layout_segments(machine_segments(&app), upgrade_spans(Some("v1.2.3")), 200);
        spans.extend(layout_line(vec![], vec![], 40));
        spans.extend(join_segments(session_segments(&app, &app.sessions[0])));
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
        let bar = join_bar(&[
            Segment {
                priority: 0,
                spans: vec![text("a")],
            },
            Segment {
                priority: 0,
                spans: vec![text("b")],
            },
        ]);
        assert_eq!(plain(&bar), " a │ b");
        assert_eq!(bar[2].style.fg, Some(DIM));
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

    /// Prints the bar at the widths of the README paragraph; run with
    /// `--nocapture` to see it.
    #[test]
    fn print_renderings() {
        let mut app = demo_app();
        for (cpu, mem) in [(18.0, 26u64), (41.0, 27), (35.0, 27), (22.0, 26), (27.0, 26), (19.0, 26), (23.0, 26)] {
            app.on_host_stats("local".into(), cpu, mem << 30, 46 << 30);
        }
        let shell = {
            let mut shell = demo_app();
            shell.host_stats = app.host_stats.clone();
            shell.sessions[0].kind = SessionKind::Shell;
            shell
        };
        for width in [200, 120, 80] {
            println!("claude {width:>3}: {}", render(&app, width));
        }
        for width in [200, 120, 80] {
            println!("term   {width:>3}: {}", render(&shell, width));
        }
    }
}
