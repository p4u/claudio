//! How the proxy panel's numbers read: counts, tokens, shares, changes,
//! ages and dates, and the few rates its pages compute. Plain text only:
//! the front-ends draw the blocks.

use crate::addon::{Block, Tone};
use crate::addons::vocdoni::proxy::api::{SessionCredential, StatsTotals};

pub use crate::text::fmt_tokens;

/// `1234567` → `1,234,567`.
pub fn fmt_count(n: i64) -> String {
    let digits = n.abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 {
        out.insert(0, '-');
    }
    out
}

/// A fraction as a percentage: `0.873` → `87%`, `0.0021` → `0.2%`.
pub fn fmt_pct(frac: f64) -> String {
    let pct = frac * 100.0;
    if pct > 0.0 && pct < 10.0 {
        format!("{pct:.1}%")
    } else {
        format!("{pct:.0}%")
    }
}

/// A signed change: `+26%`, `-8%`, `±0%`.
pub fn fmt_delta(frac: f64) -> String {
    let pct = (frac * 100.0).round() as i64;
    match pct {
        0 => "±0%".to_owned(),
        p if p > 0 => format!("+{p}%"),
        p => format!("{p}%"),
    }
}

/// A compact duration: `42s`, `12m`, `3h`, `5d`.
pub fn fmt_age(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86_399 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86_400),
    }
}

/// `2026-10-09T15:00:00Z` → `2026-10-09 15:00 UTC`; anything else as it is.
pub fn fmt_rfc3339(ts: &str) -> String {
    crate::text::parse_rfc3339(ts)
        .and_then(|secs| i64::try_from(secs).ok())
        .map_or_else(|| ts.to_owned(), fmt_utc)
}

/// `2025-01-31 14:22 UTC` for a Unix timestamp (civil date by hand).
fn fmt_utc(secs: i64) -> String {
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        rest / 3600,
        rest % 3600 / 60
    )
}

/// `claude-opus-5-5[1m]` → `opus-5-5[1m]`.
pub fn short_model(id: &str) -> &str {
    id.strip_prefix("claude-").unwrap_or(id)
}

/// `part / whole`, 0 for nothing.
pub fn ratio(part: i64, whole: i64) -> f64 {
    if whole <= 0 {
        0.0
    } else {
        part as f64 / whole as f64
    }
}

/// The relative change of `now` against `baseline`, if there is one.
pub fn delta(now: f64, baseline: f64) -> Option<f64> {
    (baseline > 0.0).then(|| (now - baseline) / baseline)
}

/// The share of prompt tokens served from the cache.
pub fn cache_hit(t: &StatsTotals) -> Option<f64> {
    let prompt = t.input_tokens + t.cache_read + t.cache_creation;
    (prompt > 0).then(|| t.cache_read as f64 / prompt as f64)
}

/// A usage's tone: healthy below 60 %, warning below 85 %, hot from there.
pub fn tone_of(pct: f64) -> Tone {
    if pct < 60.0 {
        Tone::Ok
    } else if pct < 85.0 {
        Tone::Warn
    } else {
        Tone::Hot
    }
}

/// `work-max (max) · 5h 37% · ⇆ switched 3m ago`: a session's credential
/// in one line; `None` when the proxy named nothing.
pub fn credential_line(cred: &SessionCredential, now: u64) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(name) = cred.name() {
        parts.push(match cred.plan() {
            Some(plan) => format!("{name} ({plan})"),
            None => name.to_owned(),
        });
    }
    if let Some(pct) = cred.five_hour_pct() {
        parts.push(format!("5h {}%", pct.floor() as i64));
    }
    if let Some(age) = cred.recent_switch_age(now) {
        parts.push(format!("⇆ switched {} ago", fmt_age(age)));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// A paragraph.
pub fn text(s: impl Into<String>) -> Block {
    Block::Text {
        text: s.into(),
        tone: None,
    }
}

/// A paragraph in `tone`.
pub fn toned(s: impl Into<String>, tone: Tone) -> Block {
    Block::Text {
        text: s.into(),
        tone: Some(tone),
    }
}

/// Key/value lines.
pub fn key_values(items: Vec<(&str, String)>) -> Block {
    Block::KeyValues {
        items: items.into_iter().map(|(k, v)| (k.to_owned(), v)).collect(),
    }
}

/// A table.
pub fn table(header: &[&str], rows: Vec<Vec<String>>) -> Block {
    Block::Table {
        header: header.iter().map(|h| (*h).to_owned()).collect(),
        rows,
    }
}
