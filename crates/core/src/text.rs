//! Small text helpers shared by every module: terminal-safe sanitizing,
//! RFC 3339 parsing and token-count formatting.

/// `s` without what could reach the terminal as anything but text: controls
/// (`Cc`, except `\n` and `\t`), format characters (`Cf`: bidi controls,
/// zero-width characters, BOM, soft hyphen), private-use code points and
/// noncharacters, and a combining mark with no base character before it.
///
/// Table-free: `Cf` is the list of its ranges, and only noncharacters stand
/// for unassigned code points.
pub fn sanitize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    // Whether the last kept character can carry a combining mark.
    let mut base = false;
    for c in s.chars() {
        if c == '\n' || c == '\t' {
            out.push(c);
            base = false;
        } else if is_unsafe(c) {
            continue;
        } else if is_combining(c) {
            if base {
                out.push(c);
            }
        } else {
            base = !c.is_whitespace();
            out.push(c);
        }
    }
    out
}

/// Whether [`sanitize`] removes `c` wherever it is: a control other than
/// `\n` and `\t`, a format character, a private-use code point or a
/// noncharacter. (A combining mark goes only when it has no base.)
pub fn is_unsafe(c: char) -> bool {
    (c.is_control() && c != '\n' && c != '\t') || is_format(c) || is_private_or_unassigned(c)
}

/// General category `Cf`.
fn is_format(c: char) -> bool {
    matches!(
        c as u32,
        0xAD | 0x600..=0x605
            | 0x61C
            | 0x6DD
            | 0x70F
            | 0x180E
            | 0x200B..=0x200F
            | 0x202A..=0x202E
            | 0x2060..=0x206F
            | 0xFEFF
            | 0xFFF9..=0xFFFB
            | 0x1D173..=0x1D17A
            | 0xE0001
            | 0xE0020..=0xE007F
    )
}

/// Private use (`Co`) and the noncharacters among the unassigned (`Cn`).
fn is_private_or_unassigned(c: char) -> bool {
    let n = c as u32;
    matches!(n, 0xE000..=0xF8FF | 0xF0000..=0x10FFFF | 0xFDD0..=0xFDEF) || n & 0xFFFE == 0xFFFE
}

/// The combining-mark (`Mn`) blocks a formula may carry: diacritics, their
/// extensions and supplements, the marks for symbols, half marks and
/// variation selectors.
pub fn is_combining(c: char) -> bool {
    matches!(
        c as u32,
        0x300..=0x36F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE00..=0xFE0F
            | 0xFE20..=0xFE2F
    )
}

/// Parse an RFC 3339 timestamp (`2026-10-09T10:07:02Z`, with optional
/// fractional seconds and `±HH:MM` offset) to unix seconds.
pub fn parse_rfc3339(ts: &str) -> Option<u64> {
    let (date, rest) = ts.split_once(['T', 't', ' '])?;
    let mut d = date.split('-');
    let year: i64 = d.next()?.parse().ok()?;
    let month: i64 = d.next()?.parse().ok()?;
    let day: i64 = d.next()?.parse().ok()?;
    if d.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let (clock, offset) = match rest.find(['Z', 'z', '+', '-']) {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "Z"),
    };
    let mut c = clock.split(':');
    let hour: i64 = c.next()?.parse().ok()?;
    let min: i64 = c.next()?.parse().ok()?;
    let sec: i64 = c.next().unwrap_or("0").split('.').next()?.parse().ok()?;
    if hour > 23 || min > 59 || sec > 60 {
        return None;
    }
    let offset_secs: i64 = if offset.eq_ignore_ascii_case("z") {
        0
    } else {
        let sign = if offset.starts_with('-') { -1 } else { 1 };
        let (oh, om) = offset[1..].split_once(':')?;
        sign * (oh.parse::<i64>().ok()? * 3600 + om.parse::<i64>().ok()? * 60)
    };
    // Days since 1970-01-01 (Howard Hinnant's days-from-civil).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + hour * 3600 + min * 60 + sec - offset_secs;
    u64::try_from(secs).ok()
}

/// Format a token count for display (e.g. `1.2M`, `340k`, `42`).
pub fn fmt_tokens(n: i64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.0}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_drops_controls_format_and_private_use() {
        // A C0 escape, a C1 control, a bidi override, a zero-width space, a
        // BOM, a soft hyphen and a private-use character.
        let dirty = "a\u{1b}[31mb\u{9b}c\u{202e}d\u{200b}e\u{feff}f\u{ad}g\u{e000}h";
        assert_eq!(sanitize(dirty), "a[31mbcdefgh");
        // Newlines and tabs stay; other controls (CR) go.
        assert_eq!(sanitize("x\r\n\ty"), "x\n\ty");
    }

    #[test]
    fn sanitize_keeps_combining_marks_only_after_a_base() {
        assert_eq!(sanitize("x\u{338}"), "x\u{338}");
        assert_eq!(
            sanitize("x\u{302}\u{303}"),
            "x\u{302}\u{303}",
            "stacked marks stay"
        );
        assert_eq!(sanitize("\u{338}x"), "x", "a leading mark goes");
        assert_eq!(
            sanitize("a \u{338}b"),
            "a b",
            "after a space it has no base"
        );
        assert_eq!(sanitize("a\n\u{338}b"), "a\nb");
    }

    #[test]
    fn rfc3339_parses_to_unix_seconds() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("2026-10-09T10:07:02Z"), Some(1_791_540_422));
        assert_eq!(
            parse_rfc3339("2026-10-09T10:07:02.123456Z"),
            Some(1_791_540_422)
        );
        // +02:00 is two hours ahead of UTC.
        assert_eq!(
            parse_rfc3339("2026-10-09T12:07:02+02:00"),
            Some(1_791_540_422)
        );
        assert_eq!(
            parse_rfc3339("2026-10-09T05:37:02-04:30"),
            Some(1_791_540_422)
        );
        assert_eq!(parse_rfc3339("2024-02-29T00:00:00Z"), Some(1_709_164_800));
        assert_eq!(parse_rfc3339("soon"), None);
        assert_eq!(parse_rfc3339("2026-13-09T10:07:02Z"), None);
    }
}
