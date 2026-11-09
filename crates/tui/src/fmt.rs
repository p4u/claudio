//! Text formatting shared by the views and the renderer: display widths,
//! truncation, and how counts, tokens, ages, dates and model names read.

use ratatui::text::Span;

/// Token counts as `claudio proxy status` prints them: `1.2M`, `340k`, `42`.
pub use claudio_core::text::fmt_tokens;

// ── Widths ────────────────────────────────────────────────────────────────────

/// Display width of `s` in terminal columns.
pub fn str_width(s: &str) -> usize {
    Span::raw(s).width()
}

/// Display width of a run of spans.
pub fn spans_width(spans: &[Span]) -> usize {
    spans.iter().map(Span::width).sum()
}

fn char_width(c: char) -> usize {
    let mut buf = [0u8; 4];
    str_width(c.encode_utf8(&mut buf))
}

/// Cut `s` to at most `max` columns, ending in `…` when shortened.
pub fn truncate(s: &str, max: usize) -> String {
    if str_width(s) <= max {
        return s.to_owned();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let w = char_width(c);
        if used + w > max - 1 {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('…');
    out
}

/// The last `max` columns of `s`.
pub fn tail(s: &str, max: usize) -> String {
    let mut used = 0;
    let mut chars: Vec<char> = Vec::new();
    for c in s.chars().rev() {
        used += char_width(c);
        if used > max {
            break;
        }
        chars.push(c);
    }
    chars.into_iter().rev().collect()
}

// ── Paths and names ───────────────────────────────────────────────────────────

/// Replace a leading `home` with `~`.
pub fn abbreviate_home(path: &str, home: &str) -> String {
    if home.is_empty() || home == "/" {
        return path.to_owned();
    }
    match path.strip_prefix(home) {
        Some("") => "~".to_owned(),
        Some(rest) if rest.starts_with('/') => format!("~{rest}"),
        _ => path.to_owned(),
    }
}

/// `claude-opus-5-5[1m]` → `opus-5-5[1m]`.
pub fn short_model(id: &str) -> &str {
    id.strip_prefix("claude-").unwrap_or(id)
}

// ── Numbers ───────────────────────────────────────────────────────────────────

/// A context window: `1M`, `200k`, `32k`. Round figures stay round.
pub fn fmt_window(n: u64) -> String {
    if n >= 1_000_000 && n.is_multiple_of(1_000_000) {
        format!("{}M", n / 1_000_000)
    } else {
        fmt_tokens(n.min(i64::MAX as u64) as i64)
    }
}

// ── Time ──────────────────────────────────────────────────────────────────────

/// A compact duration: `42s`, `12m`, `3h`, `5d`.
pub fn fmt_age(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86_399 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86_400),
    }
}

/// `2025-01-31 14:22 UTC` for a Unix timestamp.
///
/// The civil-date arithmetic is done by hand on purpose: it is a dozen lines,
/// and claudio has no date crate to pull in for it.
pub fn fmt_utc(secs: i64) -> String {
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_marks_cut_text() {
        assert_eq!(truncate("claudio", 10), "claudio");
        assert_eq!(truncate("claudio", 7), "claudio");
        assert_eq!(truncate("claudio", 5), "clau…");
        assert_eq!(truncate("claudio", 1), "…");
        assert_eq!(truncate("claudio", 0), "");
        // Wide characters count two columns.
        assert_eq!(truncate("日本語です", 5), "日本…");
        assert_eq!(str_width(&truncate("日本語です", 4)), 3);
        assert_eq!(tail("日本語です", 4), "です");
    }

    #[test]
    fn home_is_abbreviated_only_on_a_path_boundary() {
        assert_eq!(abbreviate_home("/home/u/repos", "/home/u"), "~/repos");
        assert_eq!(abbreviate_home("/home/u", "/home/u"), "~");
        assert_eq!(abbreviate_home("/home/user2", "/home/u"), "/home/user2");
    }

    #[test]
    fn ages_are_compact() {
        assert_eq!(fmt_age(5), "5s");
        assert_eq!(fmt_age(300), "5m");
        assert_eq!(fmt_age(7200), "2h");
        assert_eq!(fmt_age(3 * 86_400), "3d");
    }

    #[test]
    fn utc_dates_format() {
        assert_eq!(fmt_utc(0), "1970-01-01 00:00 UTC");
        assert_eq!(fmt_utc(1_700_000_000), "2023-11-14 22:13 UTC");
        assert_eq!(fmt_utc(951_782_400), "2000-02-29 00:00 UTC", "leap day");
        assert_eq!(fmt_utc(-86_400), "1969-12-31 00:00 UTC");
    }

    #[test]
    fn numbers() {
        assert_eq!(fmt_window(1_000_000), "1M");
        assert_eq!(fmt_window(200_000), "200k");
        assert_eq!(fmt_window(1_500_000), "1.5M");
        assert_eq!(fmt_window(812), "812");
    }

    #[test]
    fn names() {
        assert_eq!(short_model("claude-opus-5-5[1m]"), "opus-5-5[1m]");
        assert_eq!(short_model("gpt-x"), "gpt-x");
    }
}
