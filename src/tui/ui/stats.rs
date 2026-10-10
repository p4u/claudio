//! The proxy stats popup (Alt+s): rendering of the pages owned by
//! `stats_view.rs`. Everything here is a pure function of [`App`] and the
//! [`StatsView`]; the only non-determinism is the "updated N s ago" age.
//!
//! Colours use plain ANSI foregrounds (readable on dark and light themes) and
//! `BOLD` for the key numbers; bars are hand-built spans so they stay crisp
//! at any width.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::proxy::api::{PoolStatus, SessionCredential, StatsModel, StatsTotals};

use super::super::app::{App, ProxyStatus, SessionView};
use super::super::fmt::{
    fmt_age, fmt_count, fmt_delta, fmt_pct, fmt_rfc3339, fmt_tokens, short_model, str_width,
    truncate,
};
use super::super::stats_view::{Page, StatsView, Window};
use super::{centered, popup};

const POPUP_W: u16 = 86;
const POPUP_H: u16 = 28;
const BAR_W: usize = 20;

// ── Entry points ──────────────────────────────────────────────────────────────

pub(super) fn draw_proxy_stats(frame: &mut Frame, app: &App, view: &StatsView) {
    let rect = popup_rect(frame.area());
    let title = match &view.profile {
        Some(p) => format!("Proxy stats: {p}"),
        None => "Proxy stats".to_owned(),
    };
    let inner = popup(frame, rect, &title);
    let [header, rule, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(inner);

    frame.render_widget(
        Paragraph::new(header_line(view, inner.width as usize)),
        header,
    );
    frame.render_widget(
        Paragraph::new(Line::styled("─".repeat(inner.width as usize), dim())),
        rule,
    );
    let lines = body_lines(app, view, inner.width as usize);
    let max = lines
        .len()
        .saturating_sub(usize::from(body.height))
        .min(usize::from(u16::MAX)) as u16;
    // Only here is the page's length known: the view scrolls within it.
    view.set_max_scroll(max);
    let scroll = view.scroll();
    frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), body);
    frame.render_widget(
        Paragraph::new(footer_line(app, view, inner.width as usize, scroll, max)),
        footer,
    );
}

fn popup_rect(area: Rect) -> Rect {
    centered(
        area,
        POPUP_W.min(area.width.saturating_sub(2)),
        POPUP_H.min(area.height.saturating_sub(2)),
    )
}

// ── Chrome ────────────────────────────────────────────────────────────────────

/// `│ 1 Overview │ 2 Models │ …                 window: [24h] 7d 30d`
fn header_line(view: &StatsView, width: usize) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for page in Page::ALL {
        let text = format!(" {} {} ", page.index(), page.title());
        if page == view.page {
            spans.push(Span::styled(
                text,
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(text, Style::default().fg(Color::Cyan)));
        }
        spans.push(Span::styled("│", dim()));
    }
    let mut right: Vec<Span<'static>> = vec![Span::styled("window: ", dim())];
    for w in Window::ALL {
        if w == view.window {
            right.push(Span::styled(
                format!("[{}]", w.param()),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            right.push(Span::styled(format!(" {} ", w.param()), dim()));
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

fn footer_line(app: &App, view: &StatsView, width: usize, scroll: u16, max: u16) -> Line<'static> {
    let keys = if view.can_switch {
        " ←→ page · 1-5 jump · w window · ↑↓ scroll · r refresh · n next sub · q close"
    } else {
        " ←→ page · 1-5 jump · w window · ↑↓ scroll · r refresh · q close"
    };
    let status = view
        .profile
        .as_deref()
        .and_then(|p| app.proxy_status.get(p));
    let (right, style) = if view.loading {
        ("⟳ fetching…".to_owned(), Style::default().fg(Color::Yellow))
    } else if let Some(at) = status.and_then(|s| s.fetched_at) {
        (
            format!("updated {} ago", fmt_age(at.elapsed().as_secs())),
            dim(),
        )
    } else {
        (String::new(), dim())
    };
    let more = match (scroll > 0, scroll < max) {
        (true, true) => "↕ ",
        (true, false) => "↑ ",
        (false, true) => "↓ ",
        (false, false) => "",
    };
    let right = format!("{more}{right} ");
    let pad = width.saturating_sub(str_width(keys) + str_width(&right));
    Line::from(vec![
        Span::styled(keys.to_owned(), dim()),
        Span::raw(" ".repeat(pad)),
        Span::styled(right, style),
    ])
}

// ── Pages ─────────────────────────────────────────────────────────────────────

fn body_lines(app: &App, view: &StatsView, width: usize) -> Vec<Line<'static>> {
    let Some(profile) = view.profile.as_deref() else {
        return no_proxy_lines();
    };
    let status = app.proxy_status.get(profile);
    let mut lines = Vec::new();
    if let Some(err) = status.and_then(|s| s.error.as_deref()) {
        lines.push(Line::from(vec![
            Span::styled(
                " ! ".to_owned(),
                fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::styled(err.to_owned(), fg(Color::Red)),
        ]));
        lines.push(Line::raw(""));
    }
    let Some(status) = status else {
        lines.push(waiting_line(view));
        return lines;
    };
    match view.page {
        Page::Overview => overview_lines(app, view, profile, status, &mut lines),
        Page::Models => models_lines(view, status, width, &mut lines),
        Page::Trends => trends_lines(status, &mut lines),
        Page::Pool => pool_lines(view, status, &mut lines),
        Page::Sessions => sessions_lines(app, profile, &mut lines),
    }
    lines
}

fn no_proxy_lines() -> Vec<Line<'static>> {
    vec![
        Line::raw(""),
        Line::from(Span::styled(
            " No proxy configured.".to_owned(),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
        Line::raw(" This popup shows your claude-proxy account: requests, tokens, limits,"),
        Line::raw(" pool health and the model catalogue. To connect one, run:"),
        Line::raw(""),
        Line::from(Span::styled(
            "   claudio proxy login https://proxy.example.com".to_owned(),
            fg(Color::Cyan),
        )),
        Line::raw(""),
        Line::raw(" (the token is read from the prompt or CLAUDIO_PROXY_URL=<token>@host),"),
        Line::raw(" then start a new session with Alt+n and pick the proxy profile."),
    ]
}

fn waiting_line(view: &StatsView) -> Line<'static> {
    if view.loading {
        Line::styled(" Fetching stats…".to_owned(), dim())
    } else {
        Line::styled(" No data yet — press r to fetch.".to_owned(), dim())
    }
}

// ── Overview ──────────────────────────────────────────────────────────────────

fn overview_lines(
    app: &App,
    view: &StatsView,
    profile: &str,
    status: &ProxyStatus,
    lines: &mut Vec<Line<'static>>,
) {
    let stats = status.stats.get(&view.window);

    let mut account = vec![Span::styled(
        stats
            .map(|s| s.user_name.clone())
            .unwrap_or_else(|| "?".to_owned()),
        bold(),
    )];
    account.push(Span::styled(format!(" · profile {profile}"), dim()));
    if view.borrowed_profile {
        account.push(Span::styled(
            " · this session runs direct".to_owned(),
            fg(Color::Yellow),
        ));
    }
    lines.push(kv("Account", account));

    lines.push(kv("Pool", pool_summary(status)));
    lines.push(Line::raw(""));

    let Some(stats) = stats else {
        lines.push(waiting_line(view));
        return;
    };
    let t = &stats.totals;

    let mut req = vec![Span::styled(fmt_count(t.requests), bold())];
    req.push(Span::styled(
        format!(" requests in the last {}", view.window.param()),
        dim(),
    ));
    if t.errors > 0 {
        req.push(Span::styled(
            format!(
                "  ·  {} errors ({})",
                fmt_count(t.errors),
                fmt_pct(ratio(t.errors, t.requests))
            ),
            fg(Color::Red),
        ));
    } else if t.requests > 0 {
        req.push(Span::styled("  ·  no errors".to_owned(), fg(Color::Green)));
    }
    lines.push(kv("Requests", req));

    let total = t.input_tokens + t.output_tokens + t.cache_read + t.cache_creation;
    lines.push(kv(
        "Tokens",
        vec![
            Span::styled(fmt_tokens(total), bold()),
            Span::styled(" total".to_owned(), dim()),
            Span::styled("  ·  in ".to_owned(), dim()),
            Span::raw(fmt_tokens(t.input_tokens)),
            Span::styled("  ·  out ".to_owned(), dim()),
            Span::styled(fmt_tokens(t.output_tokens), fg(Color::Cyan)),
        ],
    ));
    lines.push(kv(
        "Cache",
        vec![
            Span::raw(fmt_tokens(t.cache_read)),
            Span::styled(" read  ·  ".to_owned(), dim()),
            Span::raw(fmt_tokens(t.cache_creation)),
            Span::styled(" written".to_owned(), dim()),
        ],
    ));
    lines.push(Line::raw(""));

    match cache_hit(t) {
        Some(hit) => lines.push(kv(
            "Cache hit",
            gauge(
                hit,
                Color::Green,
                "of prompt tokens served from cache".to_owned(),
            ),
        )),
        None => lines.push(kv("Cache hit", vec![Span::styled("n/a".to_owned(), dim())])),
    }
    match &stats.limit {
        Some(lim) => {
            let color = limit_color(lim.used_pct);
            let text = format!(
                "{} of {} output tokens per {}",
                fmt_tokens(lim.used_output_tokens),
                fmt_tokens(lim.output_tokens),
                fmt_age(lim.window_seconds.max(0) as u64)
            );
            lines.push(kv("Limit", gauge(lim.used_pct, color, text)));
            if lim.blocked {
                lines.push(kv("", blocked_spans(lim.blocked_until.as_deref())));
            }
        }
        None => lines.push(kv(
            "Limit",
            vec![Span::styled("none on this token".to_owned(), dim())],
        )),
    }
    lines.push(Line::raw(""));

    if !stats.by_model.is_empty() {
        lines.push(Line::styled(
            " Top models by output tokens".to_owned(),
            bold(),
        ));
        let top_out = stats
            .by_model
            .iter()
            .map(|m| m.output_tokens)
            .max()
            .unwrap_or(0);
        for m in stats.by_model.iter().take(5) {
            let share = ratio(m.output_tokens, t.output_tokens);
            let fam = family(&m.model);
            lines.push(Line::from(vec![
                Span::styled(
                    format!("   {:<22} ", truncate(short_model(&m.model), 22)),
                    fg(family_color(fam)),
                ),
                Span::styled(
                    bar(ratio(m.output_tokens, top_out), BAR_W),
                    fg(family_color(fam)),
                ),
                Span::styled(format!(" {:>4}", fmt_pct(share)), bold()),
                Span::styled(
                    format!(
                        "  {:>6} req  {:>6} out",
                        fmt_count(m.requests),
                        fmt_tokens(m.output_tokens)
                    ),
                    dim(),
                ),
            ]));
        }
        lines.push(Line::raw(""));
    }

    if let Some(v) = app
        .active_view()
        .filter(|v| v.proxy.as_deref() == Some(profile))
    {
        lines.push(kv("This session", session_summary(v, app.now)));
        if let Some(cred) = app.session_credential(v) {
            lines.push(kv("Credential", credential_spans(cred, app.now)));
        }
        if view.can_switch {
            lines.push(kv(
                "",
                vec![Span::styled(
                    "press n to move this session to another subscription".to_owned(),
                    dim(),
                )],
            ));
        }
    }
}

fn pool_summary(status: &ProxyStatus) -> Vec<Span<'static>> {
    let Some(pool) = &status.pool else {
        return vec![Span::styled(
            "unknown (health not fetched)".to_owned(),
            dim(),
        )];
    };
    let overall = pool.overall();
    let mut spans = vec![
        Span::styled("● ".to_owned(), fg(pool_color(&overall))),
        Span::styled(
            overall.label().to_owned(),
            fg(pool_color(&overall)).add_modifier(Modifier::BOLD),
        ),
    ];
    let detail: Vec<String> = pool
        .providers
        .iter()
        .map(|p| format!("{} {}", p.name, p.status))
        .collect();
    if !detail.is_empty() {
        spans.push(Span::styled(format!("   {}", detail.join(" · ")), dim()));
    }
    spans
}

fn session_summary(v: &SessionView, now: u64) -> Vec<Span<'static>> {
    let mut spans = vec![Span::styled(v.label(), bold())];
    if let Some(m) = &v.model {
        spans.push(Span::styled(" · ".to_owned(), dim()));
        spans.push(Span::styled(
            short_model(m).to_owned(),
            fg(family_color(family(m))),
        ));
    }
    if let Some(ctx) = v.context_tokens {
        spans.push(Span::styled(" · ctx ".to_owned(), dim()));
        spans.push(Span::raw(fmt_tokens(ctx as i64)));
    }
    spans.push(Span::styled(format!(" · {}", v.state.name()), dim()));
    if v.created_at > 0 && now >= v.created_at {
        spans.push(Span::styled(
            format!(" · up {}", fmt_age(now - v.created_at)),
            dim(),
        ));
    }
    spans
}

/// `work-max (max) · 5h 37% · ⇆ switched 3m ago`: the upstream credential the
/// proxy reports for a session.
fn credential_spans(cred: &SessionCredential, now: u64) -> Vec<Span<'static>> {
    fn sep(spans: &mut Vec<Span<'static>>) {
        if !spans.is_empty() {
            spans.push(Span::styled(" · ".to_owned(), dim()));
        }
    }
    let mut spans = Vec::new();
    if let Some(name) = cred.name() {
        spans.push(Span::styled(truncate(name, 28), bold()));
        if let Some(plan) = cred.plan() {
            spans.push(Span::styled(format!(" ({plan})"), dim()));
        }
    }
    if let Some(pct) = cred.five_hour_pct() {
        sep(&mut spans);
        spans.push(Span::styled("5h ".to_owned(), dim()));
        spans.push(Span::styled(
            format!("{}%", pct.floor() as i64),
            fg(limit_color(pct / 100.0)).add_modifier(Modifier::BOLD),
        ));
    }
    if let Some(age) = cred.recent_switch_age(now) {
        sep(&mut spans);
        spans.push(Span::styled(
            format!("⇆ switched {} ago", fmt_age(age)),
            fg(Color::Yellow),
        ));
    }
    spans
}

// ── Models ────────────────────────────────────────────────────────────────────

fn models_lines(
    view: &StatsView,
    status: &ProxyStatus,
    width: usize,
    lines: &mut Vec<Line<'static>>,
) {
    let Some(stats) = status.stats.get(&view.window) else {
        lines.push(waiting_line(view));
        return;
    };
    if stats.by_model.is_empty() {
        lines.push(Line::styled(
            format!(" No requests in the last {}.", view.window.param()),
            dim(),
        ));
        return;
    }
    let name_w = width.saturating_sub(56).clamp(12, 30);
    lines.push(Line::styled(
        format!(
            " {:<name_w$} {:>6} {:>4} {:>7} {:>7} {:>7}  Share of output",
            "Model", "Req", "Err", "In", "Out", "Cache"
        ),
        bold().add_modifier(Modifier::UNDERLINED),
    ));
    let total_out = stats.totals.output_tokens;
    for m in &stats.by_model {
        lines.push(model_row(m, name_w, total_out));
    }
    lines.push(Line::raw(""));
    let t = &stats.totals;
    lines.push(Line::styled(
        format!(
            " {:<name_w$} {:>6} {:>4} {:>7} {:>7} {:>7}",
            "Total",
            fmt_count(t.requests),
            fmt_count(t.errors),
            fmt_tokens(t.input_tokens),
            fmt_tokens(t.output_tokens),
            fmt_tokens(t.cache_read),
        ),
        bold(),
    ));
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        " In = input tokens, Cache = cache reads; share is by output tokens.".to_owned(),
        dim(),
    ));
}

fn model_row(m: &StatsModel, name_w: usize, total_out: i64) -> Line<'static> {
    let fam = family(&m.model);
    let share = ratio(m.output_tokens, total_out);
    let err_style = if m.errors > 0 { fg(Color::Red) } else { dim() };
    Line::from(vec![
        Span::styled(
            format!(" {:<name_w$}", truncate(short_model(&m.model), name_w)),
            fg(family_color(fam)),
        ),
        Span::raw(format!(" {:>6}", fmt_count(m.requests))),
        Span::styled(format!(" {:>4}", fmt_count(m.errors)), err_style),
        Span::raw(format!(" {:>7}", fmt_tokens(m.input_tokens))),
        Span::styled(format!(" {:>7}", fmt_tokens(m.output_tokens)), bold()),
        Span::raw(format!(" {:>7}  ", fmt_tokens(m.cache_read))),
        Span::styled(bar(share, 10), fg(family_color(fam))),
        Span::styled(format!(" {:>4}", fmt_pct(share)), bold()),
    ])
}

// ── Trends ────────────────────────────────────────────────────────────────────

/// Per-day rates for each window side by side, with the last 24 h compared
/// against the 7-day daily average.
fn trends_lines(status: &ProxyStatus, lines: &mut Vec<Line<'static>>) {
    struct Metric {
        name: &'static str,
        value: fn(&StatsTotals) -> i64,
        fmt: fn(i64) -> String,
        /// Up is bad (errors) rather than merely notable (usage).
        up_is_bad: bool,
    }
    const METRICS: [Metric; 5] = [
        Metric {
            name: "Requests",
            value: |t| t.requests,
            fmt: fmt_count,
            up_is_bad: false,
        },
        Metric {
            name: "Output tokens",
            value: |t| t.output_tokens,
            fmt: fmt_tokens,
            up_is_bad: false,
        },
        Metric {
            name: "Input tokens",
            value: |t| t.input_tokens,
            fmt: fmt_tokens,
            up_is_bad: false,
        },
        Metric {
            name: "Cache reads",
            value: |t| t.cache_read,
            fmt: fmt_tokens,
            up_is_bad: false,
        },
        Metric {
            name: "Errors",
            value: |t| t.errors,
            fmt: fmt_count,
            up_is_bad: true,
        },
    ];

    let have: Vec<Window> = status.cached_windows();
    if have.is_empty() {
        lines.push(Line::styled(
            " Fetching 24h, 7d and 30d stats…".to_owned(),
            dim(),
        ));
        return;
    }
    lines.push(Line::styled(
        " Daily averages per window; the arrow compares the last 24h with the 7-day average."
            .to_owned(),
        dim(),
    ));
    lines.push(Line::raw(""));
    for metric in &METRICS {
        let per_day = |w: Window| -> Option<f64> {
            status
                .stats
                .get(&w)
                .map(|s| (metric.value)(&s.totals) as f64 / w.days())
        };
        let rates: Vec<(Window, Option<f64>)> =
            Window::ALL.iter().map(|w| (*w, per_day(*w))).collect();
        let max = rates.iter().filter_map(|(_, r)| *r).fold(0.0_f64, f64::max);

        let mut head = vec![Span::styled(format!(" {} / day", metric.name), bold())];
        if let (Some(today), Some(week)) = (per_day(Window::H24), per_day(Window::D7)) {
            if let Some(d) = delta(today, week) {
                let (arrow, color) = trend_glyph(d, metric.up_is_bad);
                head.push(Span::styled(
                    format!("   {arrow} {} vs 7d avg", fmt_delta(d)),
                    fg(color),
                ));
            }
        }
        lines.push(Line::from(head));
        for (w, rate) in rates {
            let label = Span::styled(format!("   {:<4} ", w.param()), dim());
            match rate {
                Some(r) => lines.push(Line::from(vec![
                    label,
                    Span::styled(
                        bar(if max > 0.0 { r / max } else { 0.0 }, BAR_W + 6),
                        fg(Color::Cyan),
                    ),
                    Span::styled(format!(" {:>8}", (metric.fmt)(r.round() as i64)), bold()),
                ])),
                None => lines.push(Line::from(vec![label, Span::styled("…".to_owned(), dim())])),
            }
        }
        lines.push(Line::raw(""));
    }
}

fn trend_glyph(d: f64, up_is_bad: bool) -> (&'static str, Color) {
    if d.abs() < 0.005 {
        ("=", Color::DarkGray)
    } else if d > 0.0 {
        ("▲", if up_is_bad { Color::Red } else { Color::Yellow })
    } else {
        ("▼", if up_is_bad { Color::Green } else { Color::Cyan })
    }
}

// ── Pool & limits ─────────────────────────────────────────────────────────────

fn pool_lines(view: &StatsView, status: &ProxyStatus, lines: &mut Vec<Line<'static>>) {
    lines.push(Line::styled(" Providers".to_owned(), bold()));
    match &status.pool {
        Some(pool) if !pool.providers.is_empty() => {
            for p in &pool.providers {
                let color = provider_color(&p.status);
                lines.push(Line::from(vec![
                    Span::styled("   ● ".to_owned(), fg(color)),
                    Span::raw(format!("{:<14}", truncate(&p.name, 14))),
                    Span::styled(
                        format!("{:<13}", p.status),
                        fg(color).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(provider_desc(&p.status).to_owned(), dim()),
                ]));
            }
        }
        Some(_) => lines.push(Line::styled("   no providers reported".to_owned(), dim())),
        None => lines.push(Line::styled(
            "   pool health not available".to_owned(),
            dim(),
        )),
    }
    lines.push(Line::raw(""));

    lines.push(Line::styled(" Your limit".to_owned(), bold()));
    match status.stats.get(&view.window).map(|s| &s.limit) {
        Some(Some(lim)) => {
            lines.push(Line::from(vec![
                Span::raw("   ".to_owned()),
                Span::styled(fmt_tokens(lim.output_tokens), bold()),
                Span::styled(
                    format!(
                        " output tokens per {} · used ",
                        fmt_age(lim.window_seconds.max(0) as u64)
                    ),
                    dim(),
                ),
                Span::styled(fmt_tokens(lim.used_output_tokens), bold()),
                Span::styled(format!(" ({})", fmt_pct(lim.used_pct)), dim()),
            ]));
            let color = limit_color(lim.used_pct);
            lines.push(Line::from(vec![
                Span::raw("   ".to_owned()),
                Span::styled(bar(lim.used_pct, 2 * BAR_W), fg(color)),
                Span::styled(
                    format!(" {}", fmt_pct(lim.used_pct)),
                    fg(color).add_modifier(Modifier::BOLD),
                ),
            ]));
            if lim.blocked {
                let mut spans = vec![Span::raw("   ".to_owned())];
                spans.extend(blocked_spans(lim.blocked_until.as_deref()));
                lines.push(Line::from(spans));
            } else {
                lines.push(Line::styled(
                    "   The window slides: usage older than the window no longer counts."
                        .to_owned(),
                    dim(),
                ));
            }
        }
        Some(None) => lines.push(Line::styled(
            "   No output-token limit on this token.".to_owned(),
            dim(),
        )),
        None => lines.push(waiting_line(view)),
    }
    lines.push(Line::raw(""));

    let mut title = " Model catalogue".to_owned();
    if let Some(at) = status
        .models
        .as_ref()
        .and_then(|m| m.refreshed_at.as_deref())
    {
        title.push_str(&format!(" (refreshed {})", fmt_rfc3339(at)));
    }
    lines.push(Line::styled(title, bold()));
    match &status.models {
        Some(models) if !models.data.is_empty() => {
            for m in &models.data {
                let fam = family(&m.id);
                let star = if m.recommended_default { "★" } else { " " };
                let ctx = if m.max_input_tokens > 0 {
                    format!("{} ctx", fmt_tokens(m.max_input_tokens))
                } else {
                    String::new()
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("   {star} "), fg(Color::Yellow)),
                    Span::styled(
                        format!("{:<28}", truncate(short_model(&m.id), 28)),
                        fg(family_color(fam)),
                    ),
                    Span::styled(format!("{:<8}", m.family), dim()),
                    Span::raw(format!("{:<12}", truncate(&m.provider, 12))),
                    Span::styled(ctx, dim()),
                ]));
            }
            lines.push(Line::styled(
                "   ★ = the proxy's recommended default for that family".to_owned(),
                dim(),
            ));
        }
        Some(_) => lines.push(Line::styled(
            "   catalogue is empty until the proxy's first /v1/models call".to_owned(),
            dim(),
        )),
        None => lines.push(Line::styled(
            "   not available on this proxy".to_owned(),
            dim(),
        )),
    }
}

fn provider_desc(status: &str) -> &'static str {
    match status {
        "ok" => "credentials available",
        "busy" => "half or more of the credentials are saturated",
        "saturated" => "every credential is saturated",
        "unavailable" => "no active credential",
        _ => "",
    }
}

// ── Sessions ──────────────────────────────────────────────────────────────────

fn sessions_lines(app: &App, profile: &str, lines: &mut Vec<Line<'static>>) {
    let using: Vec<(usize, &SessionView)> = app
        .sessions
        .iter()
        .enumerate()
        .filter(|(_, v)| v.proxy.as_deref() == Some(profile))
        .collect();
    lines.push(Line::from(vec![
        Span::styled(format!(" {}", using.len()), bold()),
        Span::styled(
            format!(
                " of {} sessions use proxy profile {profile}",
                app.sessions.len()
            ),
            dim(),
        ),
    ]));
    lines.push(Line::raw(""));
    if using.is_empty() {
        lines.push(Line::styled(
            " Start one with Alt+n and pick the proxy in the options step.".to_owned(),
            dim(),
        ));
        return;
    }
    lines.push(Line::styled(
        format!(
            "   {:>2}  {:<16} {:<9} {:<16} {:>6}  {:<10} {:>4}",
            "#", "Name", "Host", "Model", "Ctx", "State", "Up"
        ),
        bold().add_modifier(Modifier::UNDERLINED),
    ));
    for &(i, v) in &using {
        let active = app.active == Some(i);
        let model = v.model.as_deref().map(short_model).unwrap_or("-");
        let ctx = v
            .context_tokens
            .map(|c| fmt_tokens(c as i64))
            .unwrap_or_else(|| "-".to_owned());
        let up = if v.created_at > 0 && app.now >= v.created_at {
            fmt_age(app.now - v.created_at)
        } else {
            "-".to_owned()
        };
        let marker = if active { " ▶" } else { "  " };
        lines.push(Line::from(vec![
            Span::styled(marker.to_owned(), fg(Color::Cyan)),
            Span::styled(
                format!(" {:>2}  ", i + 1),
                if active { bold() } else { dim() },
            ),
            Span::styled(
                format!("{:<16} ", truncate(&v.label(), 16)),
                if active { bold() } else { Style::default() },
            ),
            Span::raw(format!("{:<9} ", truncate(&v.host, 9))),
            Span::styled(
                format!("{:<16} ", truncate(model, 16)),
                fg(family_color(family(model))),
            ),
            Span::raw(format!("{ctx:>6}  ")),
            Span::raw(format!("{:<10} ", v.state.name())),
            Span::styled(format!("{up:>4}"), dim()),
        ]));
    }
    lines.push(Line::raw(""));
    if let Some(cred) = using
        .iter()
        .find(|(i, _)| app.active == Some(*i))
        .and_then(|(_, v)| app.session_credential(v))
    {
        lines.push(kv("▶ credential", credential_spans(cred, app.now)));
    }
    lines.push(Line::styled(
        "   ▶ = the active session; # is the tab number".to_owned(),
        dim(),
    ));
}

// ── Shared pieces ─────────────────────────────────────────────────────────────

/// ` Label      value…` with a dim, fixed-width label.
fn kv(label: &str, value: Vec<Span<'static>>) -> Line<'static> {
    let mut spans = vec![Span::styled(format!(" {label:<13}"), dim())];
    spans.extend(value);
    Line::from(spans)
}

/// A coloured bar, a bold percentage and a dim caption.
fn gauge(frac: f64, color: Color, caption: String) -> Vec<Span<'static>> {
    vec![
        Span::styled(bar(frac, BAR_W), fg(color)),
        Span::styled(
            format!(" {:>4}", fmt_pct(frac)),
            fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("  {caption}"), dim()),
    ]
}

fn blocked_spans(until: Option<&str>) -> Vec<Span<'static>> {
    vec![Span::styled(
        format!(
            "BLOCKED until {}",
            until.map(fmt_rfc3339).unwrap_or_else(|| "?".to_owned())
        ),
        fg(Color::Red).add_modifier(Modifier::BOLD),
    )]
}

/// A horizontal bar of `width` cells filled to `frac` (0..=1), using eighth
/// blocks for the partial cell so small values are still visible.
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

/// Relative change of `now` against `baseline`; `None` when there is no
/// baseline to compare with.
fn delta(now: f64, baseline: f64) -> Option<f64> {
    if baseline <= 0.0 {
        None
    } else {
        Some((now - baseline) / baseline)
    }
}

/// Share of prompt tokens that were served from the cache.
fn cache_hit(t: &StatsTotals) -> Option<f64> {
    let prompt = t.input_tokens + t.cache_read + t.cache_creation;
    (prompt > 0).then(|| t.cache_read as f64 / prompt as f64)
}

fn ratio(part: i64, whole: i64) -> f64 {
    if whole <= 0 {
        0.0
    } else {
        part as f64 / whole as f64
    }
}

fn limit_color(used: f64) -> Color {
    if used >= 0.9 {
        Color::Red
    } else if used >= 0.7 {
        Color::Yellow
    } else {
        Color::Green
    }
}

fn pool_color(s: &PoolStatus) -> Color {
    match s {
        PoolStatus::Ok => Color::Green,
        PoolStatus::Busy => Color::Yellow,
        PoolStatus::Saturated | PoolStatus::Unavailable => Color::Red,
        PoolStatus::Unknown => Color::DarkGray,
    }
}

fn provider_color(status: &str) -> Color {
    match status {
        "ok" => Color::Green,
        "busy" => Color::Yellow,
        "saturated" | "unavailable" => Color::Red,
        _ => Color::DarkGray,
    }
}

/// The model family, for colouring. Mirrors the proxy's `modelFamily`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Fable,
    Opus,
    Sonnet,
    Haiku,
    Other,
}

fn family(model: &str) -> Family {
    let m = model.to_ascii_lowercase();
    if m.contains("fable") || m.contains("mythos") {
        Family::Fable
    } else if m.contains("opus") {
        Family::Opus
    } else if m.contains("sonnet") {
        Family::Sonnet
    } else if m.contains("haiku") {
        Family::Haiku
    } else {
        Family::Other
    }
}

fn family_color(f: Family) -> Color {
    match f {
        Family::Fable => Color::Magenta,
        Family::Opus => Color::Blue,
        Family::Sonnet => Color::Cyan,
        Family::Haiku => Color::Green,
        Family::Other => Color::Yellow,
    }
}

fn fg(c: Color) -> Style {
    Style::default().fg(c)
}

fn dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

fn bold() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::SessionState;
    use crate::proxy::api::{
        LimitInfo, ModelEntry, ModelsResponse, PoolHealthResponse, ProviderHealth, StatsResponse,
    };
    use crate::tui::app::Modal;
    use crate::tui::proxy_state::ProxyFetch;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use uuid::Uuid;

    fn model(name: &str, req: i64, err: i64, inp: i64, out: i64, cr: i64) -> StatsModel {
        StatsModel {
            model: name.into(),
            requests: req,
            errors: err,
            input_tokens: inp,
            output_tokens: out,
            cache_read: cr,
            cache_creation: cr / 20,
        }
    }

    fn stats(scale: i64) -> StatsResponse {
        let models = vec![
            model(
                "claude-opus-5-5[1m]",
                812 * scale,
                scale,
                8_100_000 * scale,
                301_000 * scale,
                60_200_000 * scale,
            ),
            model(
                "claude-sonnet-5-5",
                300 * scale,
                0,
                2_400_000 * scale,
                120_000 * scale,
                31_000_000 * scale,
            ),
            model(
                "claude-haiku-5-5",
                122 * scale,
                2 * scale,
                400_000 * scale,
                35_000 * scale,
                7_500_000 * scale,
            ),
        ];
        let mut totals = StatsTotals::default();
        for m in &models {
            totals.requests += m.requests;
            totals.errors += m.errors;
            totals.input_tokens += m.input_tokens;
            totals.output_tokens += m.output_tokens;
            totals.cache_read += m.cache_read;
            totals.cache_creation += m.cache_creation;
        }
        StatsResponse {
            version: 1,
            user_name: "pau".into(),
            period: "24h".into(),
            totals,
            by_model: models,
            limit: Some(LimitInfo {
                output_tokens: 5_000_000,
                window_seconds: 18_000,
                used_output_tokens: 900_000,
                used_pct: 0.18,
                blocked: false,
                blocked_until: None,
            }),
        }
    }

    fn fetch_all() -> ProxyFetch {
        ProxyFetch {
            stats: vec![
                (Window::H24, Ok(stats(1))),
                (Window::D7, Ok(stats(5))),
                (Window::D30, Ok(stats(18))),
            ],
            pool: Some(PoolHealthResponse {
                version: 1,
                providers: vec![
                    ProviderHealth {
                        name: "anthropic".into(),
                        status: "ok".into(),
                    },
                    ProviderHealth {
                        name: "google".into(),
                        status: "busy".into(),
                    },
                    ProviderHealth {
                        name: "glm".into(),
                        status: "unavailable".into(),
                    },
                ],
            }),
            models: Some(ModelsResponse {
                version: 1,
                refreshed_at: Some("2026-10-09T10:00:00Z".into()),
                data: vec![
                    ModelEntry {
                        id: "claude-opus-5-5[1m]".into(),
                        display_name: "Claude Opus 5.5".into(),
                        provider: "anthropic".into(),
                        family: "opus".into(),
                        max_input_tokens: 1_000_000,
                        recommended_default: true,
                    },
                    ModelEntry {
                        id: "claude-sonnet-5-5".into(),
                        display_name: "Claude Sonnet 5.5".into(),
                        provider: "anthropic".into(),
                        family: "sonnet".into(),
                        max_input_tokens: 200_000,
                        recommended_default: false,
                    },
                ],
            }),
        }
    }

    fn session(name: &str, proxy: Option<&str>, model: Option<&str>) -> SessionView {
        let kind = crate::proto::SessionKind::Claude;
        SessionView {
            name: Some(name.into()),
            state: SessionState::Working,
            created_at: 1,
            attached: true,
            proxy: proxy.map(str::to_owned),
            model: model.map(str::to_owned),
            context_tokens: Some(123_000),
            ..SessionView::new(Uuid::new_v4(), "local", "/home/u/proj", kind, (28, 100))
        }
    }

    /// A manager on a 90×30 terminal.
    fn stats_app() -> App {
        App::new(crate::tui::app::AppConfig {
            size: (90, 30),
            ..crate::tui::test_support::config()
        })
    }

    /// An app with three sessions (two on the proxy) and all stats loaded.
    fn app_with_stats() -> App {
        let mut app = stats_app();
        app.now = 7_201;
        app.proxy_profiles = vec!["myproxy".into()];
        app.proxy_default = Some("myproxy".into());
        app.sessions.push(session(
            "claudio",
            Some("myproxy"),
            Some("claude-opus-5-5[1m]"),
        ));
        app.sessions
            .push(session("direct", None, Some("claude-sonnet-5-5")));
        app.sessions.push(session("docs", Some("myproxy"), None));
        app.active = Some(0);
        app.open_proxy_stats();
        app.take_effects();
        app.on_proxy_stats("myproxy".into(), fetch_all());
        app
    }

    #[test]
    fn bars_deltas_and_families() {
        assert_eq!(bar(0.0, 4), "░░░░");
        assert_eq!(bar(1.0, 4), "████");
        assert_eq!(bar(0.5, 4), "██░░");
        assert_eq!(bar(0.125, 2), "▎░");
        assert_eq!(bar(2.0, 3), "███", "clamped");
        assert_eq!(bar(0.3, 0), "");

        assert_eq!(delta(126.0, 100.0), Some(0.26));
        assert_eq!(delta(5.0, 0.0), None);

        assert_eq!(family("claude-opus-5-5[1m]"), Family::Opus);
        assert_eq!(family("claude-mythos-1"), Family::Fable);
        assert_eq!(family("claude-glm-5"), Family::Other);
    }

    fn render(app: &App) -> String {
        let mut term = Terminal::new(TestBackend::new(app.width, app.height)).unwrap();
        term.draw(|f| super::super::draw(f, app)).unwrap();
        let buf = term.backend().buffer();
        let mut out = String::new();
        for y in 0..buf.area.height {
            let row: String = (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect();
            out.push_str(row.trim_end());
            out.push('\n');
        }
        out
    }

    /// Type `c` into the popup.
    fn goto(app: &mut App, c: char) {
        app.on_terminal(crate::tui::test_support::press(crossterm::event::KeyCode::Char(c)));
    }

    #[test]
    fn open_fetches_and_result_clears_loading() {
        let mut app = stats_app();
        app.sessions.push(session("s", Some("myproxy"), None));
        app.active = Some(0);
        app.open_proxy_stats();
        let effects = app.take_effects();
        assert!(matches!(
            &effects[..],
            [crate::tui::app::Effect::FetchProxyStats { profile_name, windows, models: true }]
                if profile_name == "myproxy" && windows == &[Window::H24]
        ));
        let Some(Modal::ProxyStats(v)) = &app.modal else {
            panic!("popup not open")
        };
        assert!(v.loading);
        app.on_proxy_stats("myproxy".into(), fetch_all());
        let Some(Modal::ProxyStats(v)) = &app.modal else {
            panic!("popup not open")
        };
        assert!(!v.loading);
        assert!(app.proxy_status["myproxy"].models.is_some());
        // The catalogue is cached: the next fetch skips it.
        goto(&mut app, 'r');
        assert!(matches!(
            &app.take_effects()[..],
            [crate::tui::app::Effect::FetchProxyStats { models: false, .. }]
        ));
    }

    #[test]
    fn direct_session_borrows_default_profile() {
        let mut app = app_with_stats();
        app.modal = None;
        app.active = Some(1);
        app.open_proxy_stats();
        app.take_effects();
        let Some(Modal::ProxyStats(v)) = &app.modal else {
            panic!("popup not open")
        };
        assert_eq!(v.profile.as_deref(), Some("myproxy"));
        assert!(v.borrowed_profile);
        assert!(render(&app).contains("this session runs direct"));
    }

    #[test]
    fn no_proxy_shows_login_hint() {
        let mut app = stats_app();
        app.sessions.push(session("s", None, None));
        app.active = Some(0);
        app.open_proxy_stats();
        assert!(!app
            .take_effects()
            .iter()
            .any(|e| matches!(e, crate::tui::app::Effect::FetchProxyStats { .. })));
        let text = render(&app);
        assert!(text.contains("No proxy configured"));
        assert!(text.contains("claudio proxy login"));
    }

    #[test]
    fn fetch_error_is_shown_with_stale_data() {
        let mut app = app_with_stats();
        app.on_proxy_stats(
            "myproxy".into(),
            ProxyFetch {
                stats: vec![(
                    Window::H24,
                    Err("rate limited by the proxy; wait a few seconds".into()),
                )],
                ..Default::default()
            },
        );
        let text = render(&app);
        assert!(text.contains("! rate limited"));
        assert!(text.contains("1,234 requests"), "stale stats stay visible");
    }

    #[test]
    fn scroll_is_clamped_to_page_content() {
        let scroll = |app: &App| match &app.modal {
            Some(Modal::ProxyStats(v)) => v.scroll(),
            _ => panic!("popup not open"),
        };
        let mut app = app_with_stats();
        goto(&mut app, '3'); // Trends: 5 metrics × 5 lines, taller than the body.
        app.take_effects();
        render(&app);
        goto(&mut app, 'G');
        let bottom = scroll(&app);
        assert!(bottom > 0);
        goto(&mut app, 'j');
        assert_eq!(scroll(&app), bottom, "the end of the page");
        assert!(render(&app).contains("↑ "), "the footer says there is more above");
        goto(&mut app, 'k');
        assert_eq!(scroll(&app), bottom - 1);
        goto(&mut app, '1'); // Overview fits: scroll resets.
        render(&app);
        assert_eq!(scroll(&app), 0);
        goto(&mut app, 'G');
        assert_eq!(scroll(&app), 0);
    }

    /// Renders every page; prints the snapshots with `--nocapture`.
    #[test]
    fn pages_render() {
        let mut app = app_with_stats();
        let expectations: [(char, &[&str]); 5] = [
            (
                '1',
                &[
                    "pau",
                    "1,234 requests",
                    "Cache hit",
                    "Limit",
                    "opus-5-5[1m]",
                    "This session",
                ],
            ),
            ('2', &["Model", "opus-5-5[1m]", "812", "Total", "1,234"]),
            ('3', &["Requests / day", "24h", "7d", "30d", "vs 7d avg"]),
            (
                '4',
                &[
                    "anthropic",
                    "busy",
                    "5.0M",
                    "per 5h",
                    "Model catalogue",
                    "★",
                ],
            ),
            ('5', &["2 of 3 sessions", "claudio", "docs", "▶  1"]),
        ];
        for (key, expect) in expectations {
            goto(&mut app, key);
            app.take_effects();
            let text = render(&app);
            println!("{text}");
            for e in expect {
                assert!(text.contains(e), "page {key} should contain {e:?}:\n{text}");
            }
        }
        // The Sessions page lists only sessions on this profile (the direct
        // session is the only one on sonnet).
        assert!(!render(&app).contains("sonnet-5-5"));
    }

    #[test]
    fn credential_shows_on_overview_and_sessions_pages() {
        use crate::proxy::api::{CredentialInfo, CredentialUtilization};
        let mut app = app_with_stats();
        let id = app.sessions[0].id;
        app.sessions[0].claude_session_id = Some("c1".into());
        app.session_creds.insert(
            id,
            crate::tui::proxy_state::SessionCred {
                claude_session_id: "c1".into(),
                cred: Some(SessionCredential {
                    credential: CredentialInfo {
                        label: "work-max".into(),
                        plan: "max".into(),
                        ..Default::default()
                    },
                    utilization: Some(CredentialUtilization {
                        five_hour_pct: Some(37.5),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                asked_at: 0,
            },
        );
        for key in ['1', '5'] {
            goto(&mut app, key);
            app.take_effects();
            let text = render(&app);
            assert!(
                text.contains("work-max (max) · 5h 37%"),
                "page {key}:\n{text}"
            );
        }
        // Nothing known: no credential row.
        app.session_creds.get_mut(&id).unwrap().cred = None;
        assert!(!render(&app).contains("work-max"));
    }

    #[test]
    fn popup_survives_tiny_terminals() {
        let mut app = app_with_stats();
        app.width = 20;
        app.height = 6;
        for key in ['1', '2', '3', '4', '5'] {
            goto(&mut app, key);
            render(&app);
        }
        app.width = 4;
        app.height = 3;
        render(&app);
    }
}
