//! `<latex>…</latex>` formulas in claude's answers.
//!
//! The daemon finds them in the transcript's assistant text
//! ([`extract_latex`]), and both the daemon and the TUI pass every formula
//! through [`crate::text::sanitize`]: the text is attacker-influenced (prompt injection
//! through files or tool output) and ends up in the user's terminal.

use crate::text::sanitize;

/// A formula longer than this after [`crate::text::sanitize`] is dropped, not truncated.
pub const MAX_FORMULA_BYTES: usize = 2048;

/// How many of a conversation's most recent formulas are kept.
pub const MAX_FORMULAS: usize = 64;

/// The formulas of one assistant text, in order: what is between `<latex>`
/// and the next `</latex>` (case-sensitive, across lines), trimmed and
/// sanitized. Tags inside fenced code blocks (a line whose trimmed start
/// is ```` ``` ```` or `~~~` opens and closes one) and inline code spans
/// are not formulas. Empty formulas, and ones longer than
/// [`MAX_FORMULA_BYTES`], are dropped.
pub fn extract_latex(text: &str) -> Vec<String> {
    latex_tags(text)
        .into_iter()
        .map(|(_, formula)| formula)
        .collect()
}

/// The formula tags of one assistant text, by [`extract_latex`]'s rules:
/// each tag's byte range in `text` (from its `<latex>` to the end of its
/// `</latex>`) and its formula. Tags that are not formulas (in code, empty,
/// too long) are not listed.
pub fn latex_tags(text: &str) -> Vec<(std::ops::Range<usize>, String)> {
    lens_tags(text, &["latex"])
        .into_iter()
        .map(|(range, _, formula)| (range, formula))
        .collect()
}

/// The tags of any of `tags` in one assistant text, in order, by
/// [`extract_latex`]'s rules for `<latex>`: what is between `<tag>` and the
/// next `</tag>` (case-sensitive, across lines), trimmed and sanitized,
/// outside fenced code blocks and inline code spans; empty texts and ones
/// longer than [`MAX_FORMULA_BYTES`] are dropped. Each tag's byte range in
/// `text` (from its `<tag>` to the end of its `</tag>`), its tag and its
/// text. Where tags start inside one another, the one that opens first wins.
pub fn lens_tags(text: &str, tags: &[&str]) -> Vec<(std::ops::Range<usize>, String, String)> {
    let mut out = Vec::new();
    let pairs: Vec<(&str, String, String)> = tags
        .iter()
        .map(|t| (*t, format!("<{t}>"), format!("</{t}>")))
        .filter(|(_, open, _)| text.contains(open.as_str()))
        .collect();
    if pairs.is_empty() {
        return out;
    }
    // The text with every byte of code blanked out: tags are searched for
    // here, and cut from `text` at the same (ASCII, so char-boundary) offsets.
    let mut visible = text.as_bytes().to_vec();
    let mut in_fence = false;
    let mut start = 0;
    for line in text.split_inclusive('\n') {
        let end = start + line.len();
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            visible[start..end].fill(0);
        } else if in_fence {
            visible[start..end].fill(0);
        } else {
            for (from, to) in code_spans(line.as_bytes()) {
                visible[start + from..start + to].fill(0);
            }
        }
        start = end;
    }
    let mut pairs = pairs;
    let mut at = 0;
    loop {
        let next = pairs
            .iter()
            .enumerate()
            .filter_map(|(i, p)| Some((find(&visible, p.1.as_bytes(), at)?, i)))
            .min_by_key(|(open, _)| *open);
        let Some((open, i)) = next else {
            break;
        };
        let (tag, open_tag, close_tag) = &pairs[i];
        let inner = open + open_tag.len();
        let Some(close) = find(&visible, close_tag.as_bytes(), inner) else {
            // Unclosed: no later tag of this name closes either.
            pairs.remove(i);
            at = inner;
            continue;
        };
        let item = sanitize(&text[inner..close]);
        let item = item.trim();
        let tag = tag.to_string();
        at = close + close_tag.len();
        if !item.is_empty() && item.len() <= MAX_FORMULA_BYTES {
            out.push((open..at, tag, item.to_owned()));
        }
    }
    out
}

/// The inline code spans of one line, as byte ranges including their
/// backticks: a run of backticks up to the next run of the same length. A
/// run with no closing run is plain text.
fn code_spans(line: &[u8]) -> Vec<(usize, usize)> {
    let run_at = |i: usize| line[i..].iter().take_while(|&&b| b == b'`').count();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < line.len() {
        if line[i] != b'`' {
            i += 1;
            continue;
        }
        let run = run_at(i);
        let mut j = i + run;
        let mut closed = None;
        while j < line.len() {
            if line[j] == b'`' {
                let other = run_at(j);
                if other == run {
                    closed = Some(j + other);
                    break;
                }
                j += other;
            } else {
                j += 1;
            }
        }
        match closed {
            Some(end) => {
                spans.push((i, end));
                i = end;
            }
            None => i += run,
        }
    }
    spans
}

fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    haystack
        .get(from..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| from + p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extraction_sanitizes_formulas() {
        assert_eq!(
            extract_latex("<latex>a\u{202e}b\u{200b}c\u{7}</latex>"),
            ["abc"]
        );
    }

    #[test]
    fn formulas_come_in_order_and_trimmed() {
        let text = "Here <latex> E = mc^2 </latex> and <latex>a \\neq 0</latex>.";
        assert_eq!(extract_latex(text), ["E = mc^2", "a \\neq 0"]);
        assert!(extract_latex("no math here").is_empty());
        // Case-sensitive, and a tag needs its closing tag.
        assert!(extract_latex("<LATEX>x</LATEX> <latex>y").is_empty());
    }

    #[test]
    fn a_formula_may_span_lines() {
        let text = "<latex>\nx = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}\n  {2a}\n</latex>";
        assert_eq!(
            extract_latex(text),
            ["x = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}\n  {2a}"]
        );
    }

    #[test]
    fn code_blocks_and_inline_code_are_not_formulas() {
        let text = "```\n<latex>fenced</latex>\n```\n\
                    ~~~ rust\n<latex>tilde</latex>\n~~~\n\
                    Use `<latex>x</latex>` or ``a ` <latex>y</latex>`` to write \
                    <latex>z</latex>; a lone ` stays text: <latex>w</latex>";
        assert_eq!(extract_latex(text), ["z", "w"]);
    }

    #[test]
    fn empty_and_oversized_formulas_are_dropped() {
        let big = "x".repeat(MAX_FORMULA_BYTES + 1);
        let max = "y".repeat(MAX_FORMULA_BYTES);
        let text = format!("<latex>  </latex><latex>{big}</latex><latex>{max}</latex>");
        assert_eq!(extract_latex(&text), [max]);
    }

    #[test]
    fn tags_come_with_their_byte_ranges() {
        let text = "é <latex> x </latex> `<latex>y</latex>` <latex></latex><latex>z</latex>";
        let tags = latex_tags(text);
        assert_eq!(tags.len(), 2);
        assert_eq!(&text[tags[0].0.clone()], "<latex> x </latex>");
        assert_eq!(tags[0].1, "x");
        assert_eq!(&text[tags[1].0.clone()], "<latex>z</latex>");
        assert_eq!(tags[1].1, "z");
    }

    #[test]
    fn lens_tags_find_several_tags_by_the_latex_rules() {
        let text = "Press <keys> Ctrl+C </keys> then <latex>x^2</latex>.\n\
                    ```\n<keys>in a fence</keys>\n```\n\
                    `<keys>inline code</keys>` <KEYS>caps</KEYS> <keys></keys>\n\
                    <keys>across\nlines</keys> <keys>a\u{202e}b</keys> <keys>open";
        let found: Vec<(String, String)> = lens_tags(text, &["latex", "keys"])
            .into_iter()
            .map(|(range, tag, item)| {
                assert!(text[range].starts_with(&format!("<{tag}>")));
                (tag, item)
            })
            .collect();
        let want = [
            ("keys", "Ctrl+C"),
            ("latex", "x^2"),
            ("keys", "across\nlines"),
            ("keys", "ab"),
        ];
        let want: Vec<(String, String)> = want
            .iter()
            .map(|(t, i)| (t.to_string(), i.to_string()))
            .collect();
        assert_eq!(found, want);
        // Only the asked tags; too long is dropped.
        assert_eq!(lens_tags(text, &["latex"]).len(), 1);
        assert!(lens_tags(text, &[]).is_empty());
        let long = format!("<keys>{}</keys><keys>k</keys>", "a".repeat(2049));
        let items: Vec<String> = lens_tags(&long, &["keys"])
            .into_iter()
            .map(|t| t.2)
            .collect();
        assert_eq!(items, ["k"]);
        // An unclosed tag does not hide the others after it.
        for text in ["<latex>open <keys>k</keys>", "<latex>a <keys>k</keys> b"] {
            let items = lens_tags(text, &["latex", "keys"]);
            assert_eq!(items.into_iter().map(|t| t.2).collect::<Vec<_>>(), ["k"]);
        }
        let items = lens_tags("<keys>open <latex>x</latex>", &["latex", "keys"]);
        assert_eq!(items.into_iter().map(|t| t.2).collect::<Vec<_>>(), ["x"]);
    }
}
