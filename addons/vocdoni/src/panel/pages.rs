//! The proxy panel's five pages as blocks: Overview, Models, Trends, Pool &
//! limits, Sessions. Each is a pure function of the profile's
//! [`ProxyStatus`] and what the panel knows of the sessions.

use super::blocks::{
    cache_hit, credential_line, delta, fmt_age, fmt_count, fmt_delta, fmt_pct, fmt_rfc3339,
    fmt_tokens, key_values, ratio, short_model, table, text, toned,
};
use super::View;
use crate::proxy::api::{LimitInfo, StatsTotals};
use crate::proxy::stats::Window;
use crate::state::stats::ProxyStatus;
use claudio_core::addon::{Block, Tone};

/// The page titles, in order.
pub const TITLES: [&str; 5] = ["Overview", "Models", "Trends", "Pool & limits", "Sessions"];

/// Page `index`'s blocks.
pub fn page(index: usize, v: &View, status: Option<&ProxyStatus>) -> Vec<Block> {
    let mut blocks = Vec::new();
    if let Some(err) = status.and_then(|s| s.error.as_deref()) {
        blocks.push(toned(format!("! {err}"), Tone::Hot));
    }
    let Some(status) = status else {
        blocks.push(waiting(v));
        return blocks;
    };
    match index {
        0 => overview(v, status, &mut blocks),
        1 => models(v, status, &mut blocks),
        2 => trends(status, &mut blocks),
        3 => pool(v, status, &mut blocks),
        _ => sessions(v, &mut blocks),
    }
    blocks
}

fn waiting(v: &View) -> Block {
    let w = v.window.param();
    toned(format!("No {w} data yet: press r to fetch."), Tone::Muted)
}

fn overview(v: &View, status: &ProxyStatus, blocks: &mut Vec<Block>) {
    let stats = status.stats.get(&v.window);
    let user = stats.map_or("?", |s| s.user_name.as_str());
    let mut account = format!("{user} · profile {}", v.profile);
    if v.borrowed {
        account.push_str(" · this session runs direct");
    }
    blocks.push(key_values(vec![
        ("Account", account),
        ("Pool", pool_summary(status)),
    ]));
    let Some(stats) = stats else {
        blocks.push(waiting(v));
        return;
    };
    let t = &stats.totals;
    let mut requests = format!(
        "{} requests in the last {}",
        fmt_count(t.requests),
        v.window.param()
    );
    if t.errors > 0 {
        let share = fmt_pct(ratio(t.errors, t.requests));
        requests.push_str(&format!(" · {} errors ({share})", fmt_count(t.errors)));
    } else if t.requests > 0 {
        requests.push_str(" · no errors");
    }
    let total = t.input_tokens + t.output_tokens + t.cache_read + t.cache_creation;
    let hit = cache_hit(t).map_or_else(
        || "n/a".to_owned(),
        |h| format!("{} of prompt tokens served from cache", fmt_pct(h)),
    );
    blocks.push(key_values(vec![
        ("Requests", requests),
        (
            "Tokens",
            format!(
                "{} total · in {} · out {}",
                fmt_tokens(total),
                fmt_tokens(t.input_tokens),
                fmt_tokens(t.output_tokens)
            ),
        ),
        (
            "Cache",
            format!(
                "{} read · {} written",
                fmt_tokens(t.cache_read),
                fmt_tokens(t.cache_creation)
            ),
        ),
        ("Cache hit", hit),
    ]));
    limit(stats.limit.as_ref(), blocks);
    top_models(stats, blocks);
    this_session(v, blocks);
}

/// `● ok   anthropic ok · bedrock busy`.
fn pool_summary(status: &ProxyStatus) -> String {
    let Some(pool) = &status.pool else {
        return "unknown (health not fetched)".to_owned();
    };
    let detail: Vec<String> = pool
        .providers
        .iter()
        .map(|p| format!("{} {}", p.name, p.status))
        .collect();
    let overall = pool.overall();
    match detail.is_empty() {
        true => format!("● {}", overall.label()),
        false => format!("● {}   {}", overall.label(), detail.join(" · ")),
    }
}

/// The output-token limit as a gauge, or why there is none.
fn limit(lim: Option<&LimitInfo>, blocks: &mut Vec<Block>) {
    let Some(lim) = lim else {
        blocks.push(key_values(vec![("Limit", "none on this token".to_owned())]));
        return;
    };
    let per = fmt_age(lim.window_seconds.max(0) as u64);
    blocks.push(Block::Gauge {
        label: format!("Limit: output tokens per {per}"),
        used: lim.used_output_tokens as f64,
        limit: lim.output_tokens as f64,
        resets: blocked(lim),
    });
}

fn blocked(lim: &LimitInfo) -> Option<String> {
    let until = lim
        .blocked_until
        .as_deref()
        .map_or("?".to_owned(), fmt_rfc3339);
    lim.blocked.then(|| format!("BLOCKED until {until}"))
}

fn top_models(stats: &crate::proxy::api::StatsResponse, blocks: &mut Vec<Block>) {
    if stats.by_model.is_empty() {
        return;
    }
    let total = stats.totals.output_tokens;
    let rows = stats.by_model.iter().take(5).map(|m| {
        vec![
            short_model(&m.model).to_owned(),
            fmt_pct(ratio(m.output_tokens, total)),
            fmt_count(m.requests),
            fmt_tokens(m.output_tokens),
        ]
    });
    blocks.push(text("Top models by output tokens"));
    blocks.push(table(&["Model", "Share", "Req", "Out"], rows.collect()));
}

fn this_session(v: &View, blocks: &mut Vec<Block>) {
    let Some(session) = &v.session else {
        return;
    };
    let mut items = vec![("This session", session.label.clone())];
    if let Some(cred) = session
        .cred
        .as_ref()
        .and_then(|c| credential_line(c, v.now))
    {
        items.push(("Credential", cred));
    }
    blocks.push(key_values(items));
    if v.can_switch {
        blocks.push(toned(
            "press n to move this session to another subscription",
            Tone::Muted,
        ));
    }
}

fn models(v: &View, status: &ProxyStatus, blocks: &mut Vec<Block>) {
    let Some(stats) = status.stats.get(&v.window) else {
        blocks.push(waiting(v));
        return;
    };
    if stats.by_model.is_empty() {
        let w = v.window.param();
        blocks.push(toned(format!("No requests in the last {w}."), Tone::Muted));
        return;
    }
    let total = stats.totals.output_tokens;
    let mut rows: Vec<Vec<String>> = stats
        .by_model
        .iter()
        .map(|m| {
            vec![
                short_model(&m.model).to_owned(),
                fmt_count(m.requests),
                fmt_count(m.errors),
                fmt_tokens(m.input_tokens),
                fmt_tokens(m.output_tokens),
                fmt_tokens(m.cache_read),
                fmt_pct(ratio(m.output_tokens, total)),
            ]
        })
        .collect();
    let t = &stats.totals;
    rows.push(vec![
        "Total".to_owned(),
        fmt_count(t.requests),
        fmt_count(t.errors),
        fmt_tokens(t.input_tokens),
        fmt_tokens(t.output_tokens),
        fmt_tokens(t.cache_read),
        String::new(),
    ]);
    let header = ["Model", "Req", "Err", "In", "Out", "Cache", "Share"];
    blocks.push(table(&header, rows));
    blocks.push(toned(
        "In = input tokens, Cache = cache reads; share is by output tokens.",
        Tone::Muted,
    ));
}

/// One metric of the Trends page.
struct Metric {
    name: &'static str,
    value: fn(&StatsTotals) -> i64,
    /// Up is bad (errors) rather than merely notable (usage).
    up_is_bad: bool,
}

const METRICS: [Metric; 5] = [
    Metric {
        name: "Requests",
        value: |t| t.requests,
        up_is_bad: false,
    },
    Metric {
        name: "Output tokens",
        value: |t| t.output_tokens,
        up_is_bad: false,
    },
    Metric {
        name: "Input tokens",
        value: |t| t.input_tokens,
        up_is_bad: false,
    },
    Metric {
        name: "Cache reads",
        value: |t| t.cache_read,
        up_is_bad: false,
    },
    Metric {
        name: "Errors",
        value: |t| t.errors,
        up_is_bad: true,
    },
];

/// Per-day rates for each window, the last 24 h against the 7-day average.
fn trends(status: &ProxyStatus, blocks: &mut Vec<Block>) {
    if status.cached_windows().is_empty() {
        blocks.push(toned("Fetching 24h, 7d and 30d stats…", Tone::Muted));
        return;
    }
    blocks.push(toned(
        "Daily averages per window; the arrow compares the last 24h with the 7-day average.",
        Tone::Muted,
    ));
    for metric in &METRICS {
        let per_day = |w: Window| -> Option<f64> {
            let s = status.stats.get(&w)?;
            Some((metric.value)(&s.totals) as f64 / w.days())
        };
        let mut label = format!("{} / day", metric.name);
        if let (Some(today), Some(week)) = (per_day(Window::H24), per_day(Window::D7)) {
            if let Some(d) = delta(today, week) {
                let arrow = arrow(d, metric.up_is_bad);
                label.push_str(&format!("   {arrow} {} vs 7d avg", fmt_delta(d)));
            }
        }
        let series = Window::ALL
            .iter()
            .filter_map(|w| Some((w.param().to_owned(), per_day(*w)?.round())))
            .collect();
        blocks.push(Block::Bars { label, series });
    }
}

/// `▲`, `▼` or `=`: the direction of a change; a rise of what is bad
/// (errors) is marked `▲!`.
fn arrow(d: f64, up_is_bad: bool) -> &'static str {
    match (d.abs() < 0.005, d > 0.0) {
        (true, _) => "=",
        (false, true) if up_is_bad => "▲!",
        (false, true) => "▲",
        (false, false) => "▼",
    }
}

fn pool(v: &View, status: &ProxyStatus, blocks: &mut Vec<Block>) {
    blocks.push(text("Providers"));
    match &status.pool {
        Some(pool) if !pool.providers.is_empty() => {
            let rows = pool.providers.iter().map(|p| {
                let desc = provider_desc(&p.status).to_owned();
                vec![p.name.clone(), p.status.clone(), desc]
            });
            blocks.push(table(&["Provider", "Status", ""], rows.collect()));
        }
        Some(_) => blocks.push(toned("no providers reported", Tone::Muted)),
        None => blocks.push(toned("pool health not available", Tone::Muted)),
    }
    blocks.push(text("Your limit"));
    match status.stats.get(&v.window).map(|s| &s.limit) {
        Some(Some(lim)) => {
            limit(Some(lim), blocks);
            if !lim.blocked {
                blocks.push(toned(
                    "The window slides: usage older than the window no longer counts.",
                    Tone::Muted,
                ));
            }
        }
        Some(None) => blocks.push(toned("No output-token limit on this token.", Tone::Muted)),
        None => blocks.push(waiting(v)),
    }
    catalogue(status, blocks);
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

fn catalogue(status: &ProxyStatus, blocks: &mut Vec<Block>) {
    let mut title = "Model catalogue".to_owned();
    let refreshed = status
        .models
        .as_ref()
        .and_then(|m| m.refreshed_at.as_deref());
    if let Some(at) = refreshed {
        title.push_str(&format!(" (refreshed {})", fmt_rfc3339(at)));
    }
    blocks.push(text(title));
    match &status.models {
        Some(models) if !models.data.is_empty() => {
            let rows = models.data.iter().map(|m| {
                let star = if m.recommended_default { "★" } else { "" };
                let ctx = match m.max_input_tokens {
                    n if n > 0 => format!("{} ctx", fmt_tokens(n)),
                    _ => String::new(),
                };
                let id = short_model(&m.id).to_owned();
                vec![
                    star.to_owned(),
                    id,
                    m.family.clone(),
                    m.provider.clone(),
                    ctx,
                ]
            });
            let header = ["", "Model", "Family", "Provider", "Context"];
            blocks.push(table(&header, rows.collect()));
            blocks.push(toned(
                "★ = the proxy's recommended default for that family",
                Tone::Muted,
            ));
        }
        Some(_) => blocks.push(toned(
            "catalogue is empty until the proxy's first /v1/models call",
            Tone::Muted,
        )),
        None => blocks.push(toned("not available on this proxy", Tone::Muted)),
    }
}

fn sessions(v: &View, blocks: &mut Vec<Block>) {
    let using: Vec<&super::Row> = v.sessions.iter().filter(|s| s.on_profile).collect();
    blocks.push(text(format!(
        "{} of {} sessions use proxy profile {}",
        using.len(),
        v.sessions.len(),
        v.profile
    )));
    if using.is_empty() {
        blocks.push(toned(
            "Start one with Alt+n and pick the proxy in the options step.",
            Tone::Muted,
        ));
        return;
    }
    let rows = using.iter().map(|s| {
        let marker = if s.focused { "▶" } else { "" };
        let cred = s.cred.as_ref().and_then(|c| credential_line(c, v.now));
        vec![
            marker.to_owned(),
            s.tab.to_string(),
            s.label.clone(),
            s.host.clone(),
            cred.unwrap_or_else(|| "-".to_owned()),
        ]
    });
    let header = ["", "#", "Name", "Host", "Credential"];
    blocks.push(table(&header, rows.collect()));
    blocks.push(toned(
        "▶ = the focused session; # is its place among the sessions",
        Tone::Muted,
    ));
}
