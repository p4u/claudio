//! `<latex>` formulas: rendering them as Unicode text.
//!
//! Pure functions only. The render itself ([`render`]) parses and lays out
//! attacker-influenced text with a parser that is slow and deep on hostile
//! input, so it never runs on the UI thread: `tui/mod.rs` calls it from its
//! `latex-render` worker, on a scratch thread with a time budget, after
//! [`too_complex`] has turned away what could overflow that thread's stack.
//!
//! - [`Formula`] — one formula's source, its renderings and matching keys.
//! - [`render`] — the 2D block (term-maths) and the one-line form.
//! - [`too_complex`] — the pre-check that runs before any parsing.

use rust_latex_parser::{AccentKind, EqNode, MatrixKind};
use unicode_width::UnicodeWidthStr;

use crate::claude::latex::{sanitize, MAX_FORMULA_BYTES};

/// Deepest `{`…`}` nesting a formula may have to be rendered.
const MAX_DEPTH: usize = 32;
/// Most recursion-prone tokens a formula may have to be rendered: every
/// `\command` (`\left`, `\begin` included), every `/`, and every `(` right
/// after a bareword (`sqrt(`, `frac(`). The quadratic formula has about 12.
const MAX_TOKENS: usize = 256;
/// The largest 2D rendering shown in the formulas popup.
const BLOCK_MAX_COLS: usize = 200;
const BLOCK_MAX_ROWS: usize = 40;
/// The widest one-line rendering put in the pane.
const INLINE_MAX_COLS: usize = 512;

/// One `<latex>` formula as the client shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Formula {
    /// The LaTeX, as the daemon sent it.
    pub src: String,
    /// The one-line rendering that replaces the tag in the pane; `None`
    /// leaves the tag raw.
    pub inline: Option<String>,
    /// The 2D rendering shown in the formulas popup, one string per row.
    pub block: Option<Vec<String>>,
    /// The matching keys of the on-screen tag text (see `overlay`):
    /// whitespace dropped, after undoing markdown's backslash escapes...
    pub key_strict: String,
    /// ...and without the characters markdown takes as emphasis...
    pub key_loose: String,
    /// ...and without what markdown may take for an HTML tag (`<b>`).
    pub key_html: String,
}

impl Formula {
    /// `src` with its renderings (see [`render`]); the keys come from `src`.
    pub fn new(src: &str, (inline, block): Rendered) -> Formula {
        let unescaped = md_unescape(src);
        Formula {
            src: src.to_owned(),
            inline,
            block,
            key_strict: normalize(&unescaped),
            key_loose: normalize_loose(&unescaped),
            key_html: normalize_loose(&without_html(&unescaped)),
        }
    }

    /// `src` shown as it is: not rendered (too complex, too slow, or the
    /// renderer failed).
    pub fn raw(src: &str) -> Formula {
        Formula::new(src, (None, None))
    }
}

/// What [`render`] makes of a formula: its one-line and 2D renderings.
pub type Rendered = (Option<String>, Option<Vec<String>>);

/// `src` as the renderer gets it: [`sanitize`]d, on one line.
pub fn prepare(src: &str) -> String {
    sanitize(src).replace(['\n', '\t'], " ")
}

/// Whether `src` must not be parsed at all: longer than a formula may be,
/// nested deeper than [`MAX_DEPTH`] braces, or with more than [`MAX_TOKENS`]
/// tokens that make the parser recurse (it recurses per argument, not per
/// brace: `\frac \frac \frac …` nests without any `{`).
pub fn too_complex(src: &str) -> bool {
    if src.len() > MAX_FORMULA_BYTES {
        return true;
    }
    let (mut depth, mut deepest, mut tokens) = (0usize, 0usize, 0usize);
    let mut after_word = false;
    let mut chars = src.chars().peekable();
    while let Some(c) = chars.next() {
        let mut word = false;
        match c {
            '{' => {
                depth += 1;
                deepest = deepest.max(depth);
            }
            '}' => depth = depth.saturating_sub(1),
            '\\' => {
                let mut letters = 0;
                while chars.next_if(char::is_ascii_alphabetic).is_some() {
                    letters += 1;
                }
                if letters > 0 {
                    tokens += 1;
                }
            }
            '/' => tokens += 1,
            '(' if after_word => tokens += 1,
            c if c.is_ascii_alphabetic() => word = true,
            _ => {}
        }
        after_word = word;
    }
    deepest > MAX_DEPTH || tokens > MAX_TOKENS
}

/// Render a [`prepare`]d formula: the 2D block term-maths lays out (at most
/// [`BLOCK_MAX_COLS`] × [`BLOCK_MAX_ROWS`]), and the one-line form, which is
/// that block when it is one row and otherwise our own one-line walk of the
/// parse tree (at most [`INLINE_MAX_COLS`] wide). Either is `None` when it
/// does not fit or comes out empty. Both are [`sanitize`]d.
///
/// May be slow, deep or panic on hostile input: call it only through the
/// guard in `tui/mod.rs`, after [`too_complex`].
pub fn render(src: &str) -> Rendered {
    let tree = rust_latex_parser::parse_equation(src);
    let laid_out = term_maths::layout::layout(&tree);
    let rows: Vec<String> = laid_out
        .cells()
        .iter()
        .map(|row| sanitize(&row.concat()).trim_end().to_owned())
        .collect();
    let line = match rows.as_slice() {
        [one] => one.trim().to_owned(),
        _ => sanitize(&one_line(&tree)).trim().to_owned(),
    };
    let inline = (!line.is_empty() && line.width() <= INLINE_MAX_COLS).then_some(line);
    let fits = laid_out.width() <= BLOCK_MAX_COLS && rows.len() <= BLOCK_MAX_ROWS;
    let block = (fits && rows.iter().any(|r| !r.trim().is_empty())).then_some(rows);
    (inline, block)
}

// ── One-line walk ─────────────────────────────────────────────────────────────

/// One line of text for `node`, modelled on term-maths' layout: fractions
/// become `a/b`, scripts Unicode super- and subscripts (else `^(…)` and
/// `_(…)`), roots `√x`, matrices `[a b; c d]`, cases `{ 1 if x > 0; 0
/// otherwise }`. Symbols are already Unicode in the parse tree.
fn one_line(node: &EqNode) -> String {
    match node {
        EqNode::Text(s) => s.clone(),
        EqNode::Space(points) => {
            if *points < 2.0 {
                String::new()
            } else {
                " ".to_owned()
            }
        }
        EqNode::Seq(children) => seq(children),
        EqNode::Frac(num, den) => {
            let (num, den) = (one_line(num), one_line(den));
            let num = if top_level(&num, |c| c.is_whitespace() || c == '/') {
                format!("({num})")
            } else {
                num
            };
            format!("{num}/{}", operand(den))
        }
        EqNode::Sup(base, sup) => format!("{}{}", operand(one_line(base)), superscript(sup)),
        EqNode::Sub(base, sub) => format!("{}{}", operand(one_line(base)), subscript(sub)),
        EqNode::SupSub(base, sup, sub) => format!(
            "{}{}{}",
            operand(one_line(base)),
            subscript(sub),
            superscript(sup)
        ),
        EqNode::Sqrt(body) => format!("√{}", operand(one_line(body))),
        EqNode::BigOp {
            symbol,
            lower,
            upper,
        } => {
            let lower = lower.as_deref().map(subscript).unwrap_or_default();
            let upper = upper.as_deref().map(superscript).unwrap_or_default();
            format!("{symbol}{lower}{upper}")
        }
        EqNode::Accent(body, kind) => accent(&one_line(body), *kind),
        EqNode::Limit { name, lower } => {
            let lower = lower.as_deref().map(subscript).unwrap_or_default();
            format!("{name}{lower}")
        }
        EqNode::TextBlock(s) => s.clone(),
        EqNode::MathFont { kind, content } => {
            term_maths::mathfont::map_str(kind, &one_line(content))
        }
        EqNode::Delimited {
            left,
            right,
            content,
        } => format!("{left}{}{right}", one_line(content).trim()),
        EqNode::Matrix { kind, rows } => {
            let rows: Vec<String> = rows
                .iter()
                .map(|row| {
                    let cells: Vec<String> =
                        row.iter().map(|c| one_line(c).trim().to_owned()).collect();
                    cells.join(" ")
                })
                .collect();
            let (open, close) = match kind {
                MatrixKind::VBar => ("|", "|"),
                MatrixKind::DoubleVBar => ("‖", "‖"),
                _ => ("[", "]"),
            };
            format!("{open}{}{close}", rows.join("; "))
        }
        EqNode::Cases { rows } => {
            let rows: Vec<String> = rows
                .iter()
                .map(|(value, condition)| {
                    let value = one_line(value).trim().to_owned();
                    match condition.as_ref().map(|c| one_line(c).trim().to_owned()) {
                        None => value,
                        Some(c) if starts_with_word(&c) => format!("{value} {c}"),
                        Some(c) => format!("{value} if {c}"),
                    }
                })
                .collect();
            format!("{{ {} }}", rows.join("; "))
        }
        EqNode::Binom(top, bottom) => {
            format!(
                "({} choose {})",
                one_line(top).trim(),
                one_line(bottom).trim()
            )
        }
        EqNode::Brace {
            content,
            label,
            over,
        } => {
            let label = match (label.as_deref(), over) {
                (Some(l), true) => superscript(l),
                (Some(l), false) => subscript(l),
                (None, _) => String::new(),
            };
            format!("{}{label}", one_line(content))
        }
        EqNode::StackRel {
            base,
            annotation,
            over,
        } => {
            let annotation = if *over {
                superscript(annotation)
            } else {
                subscript(annotation)
            };
            format!("{}{annotation}", one_line(base))
        }
    }
}

/// A sequence: its parts side by side, every run of spaces one space, none
/// at the ends.
fn seq(children: &[EqNode]) -> String {
    let mut out = String::new();
    let mut space = false;
    let mut parts = Vec::new();
    flatten(children, &mut parts);
    for part in parts {
        let spacing = match part {
            EqNode::Space(_) => true,
            EqNode::Text(s) => s.chars().all(char::is_whitespace),
            _ => false,
        };
        if spacing {
            space = true;
            continue;
        }
        if space && !out.is_empty() {
            out.push(' ');
        }
        space = false;
        out.push_str(&one_line(part));
    }
    out
}

fn flatten<'a>(children: &'a [EqNode], out: &mut Vec<&'a EqNode>) {
    for child in children {
        match child {
            EqNode::Seq(inner) => flatten(inner, out),
            other => out.push(other),
        }
    }
}

/// `s` as an operand: as it is when it reads as one (see [`is_atom`]),
/// else in parentheses.
fn operand(s: String) -> String {
    if is_atom(&s) {
        s
    } else {
        format!("({s})")
    }
}

/// One operand without parentheses: a number, one symbol (with scripts or a
/// root sign), or something already in brackets.
fn is_atom(s: &str) -> bool {
    let s = s.strip_prefix('√').unwrap_or(s);
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return true;
    };
    s.chars().all(|c| c.is_ascii_digit() || c == '.')
        || chars.all(is_script_glyph)
        || (matches!(first, '(' | '[' | '{' | '|' | '⟨')
            && !top_level(&s[first.len_utf8()..], |_| false))
}

/// Whether `c` is one of the super- or subscript characters scripts become.
fn is_script_glyph(c: char) -> bool {
    "⁰¹²³⁴⁵⁶⁷⁸⁹⁺⁻⁼⁽⁾ᵃᵇᶜᵈᵉᶠᵍʰⁱʲᵏˡᵐⁿᵒᵖʳˢᵗᵘᵛʷˣʸᶻ₀₁₂₃₄₅₆₇₈₉₊₋₌₍₎ₐₑₕᵢⱼₖₗₘₙₒₚᵣₛₜᵤᵥₓ'′″".contains(c)
}

/// Whether `s` has, outside any brackets, a character `pick` accepts. A
/// bracket that closes the outermost one before the end also counts (so
/// `(a)(b)` is not one bracketed operand).
fn top_level(s: &str, pick: impl Fn(char) -> bool) -> bool {
    let mut depth = 0usize;
    let last = s.chars().count().saturating_sub(1);
    for (i, c) in s.chars().enumerate() {
        match c {
            '(' | '[' | '{' | '⟨' => depth += 1,
            ')' | ']' | '}' | '⟩' => {
                if depth == 0 && i < last {
                    return true;
                }
                depth = depth.saturating_sub(1);
            }
            c if depth == 0 && pick(c) => return true,
            _ => {}
        }
    }
    false
}

/// Whether `s` starts with a word of two or more letters (`otherwise`,
/// `if`, `for`): a cases condition that needs no `if`.
fn starts_with_word(s: &str) -> bool {
    s.chars().take_while(|c| c.is_alphabetic()).count() >= 2
}

fn superscript(node: &EqNode) -> String {
    script(node, superscript_char, '^')
}

fn subscript(node: &EqNode) -> String {
    script(node, subscript_char, '_')
}

/// A script in Unicode script characters when every character has one,
/// else `^(…)` / `_(…)`. Spaces are dropped: `i = 1` is `ᵢ₌₁`.
fn script(node: &EqNode, map: fn(char) -> Option<char>, mark: char) -> String {
    let text: String = one_line(node)
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    match text.chars().map(map).collect::<Option<String>>() {
        Some(mapped) if !mapped.is_empty() => mapped,
        _ => format!("{mark}({text})"),
    }
}

fn superscript_char(c: char) -> Option<char> {
    let i = "0123456789+-=()abcdefghijklmnoprstuvwxyz".find(c)?;
    "⁰¹²³⁴⁵⁶⁷⁸⁹⁺⁻⁼⁽⁾ᵃᵇᶜᵈᵉᶠᵍʰⁱʲᵏˡᵐⁿᵒᵖʳˢᵗᵘᵛʷˣʸᶻ".chars().nth(i)
}

fn subscript_char(c: char) -> Option<char> {
    let i = "0123456789+-=()aehijklmnoprstuvx".find(c)?;
    "₀₁₂₃₄₅₆₇₈₉₊₋₌₍₎ₐₑₕᵢⱼₖₗₘₙₒₚᵣₛₜᵤᵥₓ".chars().nth(i)
}

/// `body` with an accent: a combining mark over a one-character body, else
/// `hat(…)` and the like.
fn accent(body: &str, kind: AccentKind) -> String {
    let (mark, name) = match kind {
        AccentKind::Hat => ('\u{302}', "hat"),
        AccentKind::Bar => ('\u{305}', "bar"),
        AccentKind::Dot => ('\u{307}', "dot"),
        AccentKind::DoubleDot => ('\u{308}', "ddot"),
        AccentKind::Tilde => ('\u{303}', "tilde"),
        AccentKind::Vec => ('\u{20d7}', "vec"),
    };
    let body = body.trim();
    if body.chars().count() == 1 {
        format!("{body}{mark}")
    } else {
        format!("{name}({body})")
    }
}

// ── Matching keys ─────────────────────────────────────────────────────────────

/// `s` without the backslash of each CommonMark escape (`\{`, `\\`, `\,`),
/// as Claude Code's markdown shows them.
fn md_unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some(next) if next.is_ascii_punctuation() => out.push(next),
            Some(next) => {
                out.push(c);
                out.push(next);
            }
            None => out.push(c),
        }
    }
    out
}

/// `s` without whitespace.
fn normalize(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

/// [`normalize`], also without the characters markdown may take as emphasis
/// or code (`*`, `_`, `~`, `` ` ``).
fn normalize_loose(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace() && !matches!(c, '*' | '_' | '~' | '`'))
        .collect()
}

/// `s` without the runs markdown may take for an HTML tag: `<` (or `</`),
/// a letter, then anything but `<` up to the next `>`.
fn without_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(open) = rest.find('<') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let name = after.strip_prefix('/').unwrap_or(after);
        let tag_end = name
            .starts_with(|c: char| c.is_ascii_alphabetic())
            .then(|| after.find(['<', '>']))
            .flatten()
            .filter(|&end| after.as_bytes()[end] == b'>');
        match tag_end {
            Some(end) => rest = &after[end + 1..],
            None => {
                out.push('<');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one-line rendering of `src`.
    fn inline(src: &str) -> String {
        render(&prepare(src))
            .0
            .unwrap_or_else(|| panic!("no inline for {src}"))
    }

    #[test]
    fn formulas_render_to_one_line() {
        let table = [
            ("E = mc^2", "E = mc²"),
            (
                "x = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}{2a}",
                "x = (-b ± √(b² - 4ac))/(2a)",
            ),
            (
                "\\sum_{i=1}^{n} i = \\frac{n(n+1)}{2}",
                "∑ᵢ₌₁ⁿ i = n(n + 1)/2",
            ),
            ("\\int_0^1 x^2\\,dx", "∫₀¹ x² dx"),
            ("a \\neq 0", "a ≠ 0"),
            (
                "\\begin{pmatrix} a & b \\\\ c & d \\end{pmatrix}",
                "[a b; c d]",
            ),
            (
                "f(x) = \\begin{cases} 1 & x > 0 \\\\ 0 & \\text{otherwise} \\end{cases}",
                "f(x) = { 1 if x > 0; 0 otherwise }",
            ),
            (
                "\\begin{vmatrix} a & b \\\\ c & d \\end{vmatrix}",
                "|a b; c d|",
            ),
            ("\\mathbb{R}^n", "ℝⁿ"),
            ("\\frac{1}{\\sqrt{x}}", "1/√x"),
            ("\\binom{n}{k}", "(n choose k)"),
            ("e^{i\\pi} + 1 = 0", "e^(iπ) + 1 = 0"),
            ("\\hat{x} + \\vec{v}", "x\u{302} + v\u{20d7}"),
        ];
        for (src, want) in table {
            assert_eq!(inline(src), want, "{src}");
        }
    }

    #[test]
    fn the_block_is_term_maths_rendering() {
        let (_, block) = render(&prepare("\\frac{a}{b}"));
        assert_eq!(block.unwrap(), [" a", "───", " b"]);
        let (inline, block) = render(&prepare("E = mc^2"));
        assert_eq!(block.unwrap(), ["E = mc²"]);
        assert_eq!(
            inline.as_deref(),
            Some("E = mc²"),
            "one row is its own inline"
        );
    }

    #[test]
    fn renderings_are_sanitized() {
        // A bidi override and a zero-width space never reach a cell.
        let src = prepare("a\u{202e}b\u{200b} + c");
        assert_eq!(src, "ab + c");
        let (inline, block) = render("\\text{a\u{202e}b}");
        assert_eq!(inline.as_deref(), Some("ab"));
        assert_eq!(block.unwrap(), ["ab"]);
        assert_eq!(prepare("a\nb\tc"), "a b c");
    }

    #[test]
    fn the_pre_check_turns_away_deep_and_long_formulas() {
        assert!(!too_complex("x = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}{2a}"));
        assert!(!too_complex(&"{".repeat(MAX_DEPTH)));
        assert!(too_complex(&"{".repeat(MAX_DEPTH + 1)));
        // Recursion without braces: commands, slashes, bareword calls.
        assert!(!too_complex(&"\\frac ".repeat(MAX_TOKENS)));
        assert!(too_complex(&"\\frac ".repeat(MAX_TOKENS + 1)));
        assert!(too_complex(&"\\not".repeat(MAX_TOKENS + 1)));
        assert!(too_complex(&"x/".repeat(MAX_TOKENS + 1)));
        assert!(too_complex(&"sqrt(".repeat(MAX_TOKENS + 1)));
        assert!(
            !too_complex(&"(x)".repeat(MAX_TOKENS + 1)),
            "plain parentheses"
        );
        assert!(too_complex(&"x".repeat(MAX_FORMULA_BYTES + 1)));
    }

    #[test]
    fn keys_undo_what_markdown_does_on_screen() {
        let f = Formula::raw("\\{a_i\\} \\cup \\{b_i\\} \\\\ x^2\\,dx");
        // Escapes lose their backslash, whitespace goes.
        assert_eq!(f.key_strict, "{a_i}\\cup{b_i}\\x^2,dx");
        // Emphasis characters go too.
        assert_eq!(f.key_loose, "{ai}\\cup{bi}\\x^2,dx");
        let f = Formula::raw("x <b> y </b> < z");
        assert_eq!(f.key_html, "xy<z");
        assert_eq!(f.key_loose, "x<b>y</b><z");
    }
}
