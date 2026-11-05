//! Labels: cleaning, measuring and wrapping.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Labels wrap at this many columns.
pub const WRAP: usize = 24;
/// A label has at most this many lines; the last one ends in `…` if cut.
pub const MAX_LINES: usize = 3;

/// `raw` made safe to draw: [`sanitize`](crate::claude::latex::sanitize)d,
/// `<br>` and `\n` as line breaks, tabs as spaces, and characters without a
/// width of 1 or 2 (combining marks, zero-width) dropped, so that a
/// character always takes the columns it is counted for.
pub fn clean(raw: &str) -> String {
    let raw = raw
        .replace("<br/>", "\n")
        .replace("<br />", "\n")
        .replace("<br>", "\n");
    crate::claude::latex::sanitize(&raw)
        .chars()
        .map(|c| if c == '\t' { ' ' } else { c })
        .filter(|&c| c == '\n' || matches!(c.width(), Some(1 | 2)))
        .collect()
}

/// Display columns of `s` (already [`clean`]).
pub fn width(s: &str) -> usize {
    s.width()
}

/// `label` ([`clean`]ed) in lines of at most [`WRAP`] columns, at most
/// [`MAX_LINES`] of them. Words longer than a line are split.
pub fn wrap(label: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for paragraph in label.split('\n') {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            for piece in split_word(word) {
                let sep = usize::from(!line.is_empty());
                if !line.is_empty() && width(&line) + sep + width(&piece) > WRAP {
                    lines.push(std::mem::take(&mut line));
                }
                if !line.is_empty() {
                    line.push(' ');
                }
                line.push_str(&piece);
            }
            if lines.len() > MAX_LINES {
                break;
            }
        }
        if !line.is_empty() {
            lines.push(line);
        }
        if lines.len() > MAX_LINES {
            break;
        }
    }
    if lines.len() > MAX_LINES {
        lines.truncate(MAX_LINES);
        let last = &mut lines[MAX_LINES - 1];
        *last = fit(&format!("{last} …"), WRAP);
    }
    lines
}

/// `word` in pieces of at most [`WRAP`] columns.
fn split_word(word: &str) -> Vec<String> {
    let mut pieces = vec![String::new()];
    for c in word.chars() {
        let piece = pieces.last_mut().expect("never empty");
        if width(piece) + c.width().unwrap_or(0) > WRAP {
            pieces.push(String::new());
        }
        pieces.last_mut().expect("never empty").push(c);
    }
    pieces
}

/// `s` cut to at most `max` columns, ending in `…` when cut.
pub fn fit(s: &str, max: usize) -> String {
    if width(s) <= max {
        return s.to_owned();
    }
    let mut out = String::new();
    for c in s.chars() {
        if width(&out) + c.width().unwrap_or(0) + 1 > max {
            break;
        }
        out.push(c);
    }
    let mut out = out.trim_end().to_owned();
    out.push('…');
    out
}

/// The widest line of `lines`.
pub fn widest(lines: &[String]) -> usize {
    lines.iter().map(|l| width(l)).max().unwrap_or(0)
}
