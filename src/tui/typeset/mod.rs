//! Typesetting formulas as rows of Unicode text: a compact 2D layout of the
//! `rust-latex-parser` tree, after TeX's display and text styles.
//!
//! Pure functions only, and as deep as the tree: the parser behind it is
//! slow and deep on hostile input, so this runs where the parse does, on
//! the render guard's scratch thread (`tui/mod.rs`), after the pre-check.
//!
//! - [`typeset`] — a parsed formula as rows, in display or text style.
//! - [`rewrite_roots`] — `\sqrt[n]{x}` as the parser can read it.
//! - [`Laid`] — the result: rows and the row of the baseline.
//!
//! The rules (spec "Typeset reader view", §3 and Appendix A):
//! - Display style only at the top of a display formula. Inside a script,
//!   a limit, a fraction or a root everything is text style, and scripts
//!   and limits drop whitespace.
//! - In text style fractions are flat (`a/b`) and big operators take their
//!   limits as scripts.
//! - A script whose characters all have Unicode script forms is those
//!   glyphs, on the base's row; otherwise it is a row of its own, raised or
//!   lowered next to the base.
//! - A run of spaces is one space (two for `\quad`), never at a box's ends.

mod mathfont;

use rust_latex_parser::{AccentKind, EqNode, MatrixKind};
use unicode_width::UnicodeWidthStr;

/// The widest and tallest any part of a layout may grow: past these the
/// formula is not typeset at all (it can only grow from there).
const MAX_COLS: usize = 512;
const MAX_ROWS: usize = 40;

/// Big operators whose limits go above and below in display style; the
/// others (integrals) keep them as scripts, as TeX does.
const LIMITS_ABOVE: [&str; 6] = ["∑", "∏", "∐", "⋃", "⋂", "⨁"];

/// Binary operators and relations the parser leaves without spacing; they
/// get one space each side like the ones it spaces.
const SPACED: [&str; 12] = ["·", "∉", "∋", "∪", "∩", "∧", "∨", "∘", "⊕", "⊗", "∖", "⊥"];

/// A typeset formula: its rows (no trailing blanks, no blank row at either
/// end, no blank column on the left) and which of them is the baseline,
/// where the surrounding text sits.
#[derive(Debug, Clone, PartialEq)]
pub struct Laid {
    pub rows: Vec<String>,
    pub baseline: usize,
}

impl Laid {
    /// The width of the widest row, in columns.
    pub fn width(&self) -> usize {
        self.rows.iter().map(|r| r.width()).max().unwrap_or(0)
    }
}

/// Lay `tree` out in display style (`display`) or text style. `None` when
/// a part grows past [`MAX_COLS`] × [`MAX_ROWS`] or nothing visible is
/// left.
pub fn typeset(tree: &EqNode, display: bool) -> Option<Laid> {
    let style = if display { Style::Display } else { Style::Text };
    trimmed(lay(tree, style)?)
}

/// `src` with each `\sqrt[<index>]` written as `{}^{<index>}\sqrt`: the
/// parser has no root index, so it becomes a raised script in front of the
/// root (`³√x`). An index with no closing `]` is left alone.
pub fn rewrite_roots(src: &str) -> String {
    const ROOT: &str = "\\sqrt";
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    while let Some(at) = rest.find(ROOT) {
        let after = &rest[at + ROOT.len()..];
        let open = after.len() - after.trim_start().len();
        let index = after[open..]
            .strip_prefix('[')
            .and_then(|tail| bracket_end(tail).map(|end| &tail[..end]));
        out.push_str(&rest[..at]);
        match index {
            Some(index) => {
                out.push_str("{}^{");
                out.push_str(index);
                out.push('}');
                out.push_str(ROOT);
                rest = &after[open + 1 + index.len() + 1..];
                // `\sqrt[n]x`: the root's name must not run into its argument.
                if rest.starts_with(|c: char| c.is_ascii_alphabetic()) {
                    out.push(' ');
                }
            }
            None => {
                out.push_str(ROOT);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Where the `]` that closes a bracket ends `s` (the text after `[`): the
/// first one outside braces.
fn bracket_end(s: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            ']' if depth == 0 => return Some(i),
            _ => {}
        }
    }
    None
}

// ── Boxes ─────────────────────────────────────────────────────────────────────

/// TeX's styles, plus the script style: text style without whitespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Style {
    Display,
    Text,
    Script,
}

impl Style {
    /// The style inside a fraction (either part) or a root.
    fn inner(self) -> Style {
        match self {
            Style::Script => Style::Script,
            _ => Style::Text,
        }
    }
}

/// A box of text: `rows`, each exactly `width` columns, with the baseline
/// at row `baseline`.
#[derive(Debug, Clone, PartialEq)]
struct Block {
    rows: Vec<String>,
    width: usize,
    baseline: usize,
}

impl Block {
    /// One row of text.
    fn text(s: &str) -> Block {
        Block {
            rows: vec![s.to_owned()],
            width: s.width(),
            baseline: 0,
        }
    }

    /// Nothing: one row, no columns (neutral in [`hcat`]).
    fn empty() -> Block {
        Block::text("")
    }

    fn height(&self) -> usize {
        self.rows.len()
    }

    /// The text of a one-row block.
    fn line(&self) -> Option<&str> {
        match self.rows.as_slice() {
            [one] => Some(one),
            _ => None,
        }
    }

    /// Every character mapped through `f`, rows padded again.
    fn map(self, f: impl Fn(char) -> char) -> Block {
        let rows: Vec<String> = self
            .rows
            .iter()
            .map(|r| r.chars().map(&f).collect())
            .collect();
        let width = rows.iter().map(|r| r.width()).max().unwrap_or(0);
        let rows = rows.into_iter().map(|r| padded(r, width)).collect();
        Block {
            rows,
            width,
            baseline: self.baseline,
        }
    }
}

/// `None` when a box of `width` × `height` is past the limits.
fn fits(width: usize, height: usize) -> Option<()> {
    (width <= MAX_COLS && height <= MAX_ROWS).then_some(())
}

/// `s` padded with spaces to `width` columns.
fn padded(mut s: String, width: usize) -> String {
    let w = s.width();
    s.extend(std::iter::repeat_n(' ', width.saturating_sub(w)));
    s
}

/// `parts` side by side, their baselines on one row.
fn hcat(parts: &[Block]) -> Option<Block> {
    let above = parts.iter().map(|b| b.baseline).max().unwrap_or(0);
    let below = parts
        .iter()
        .map(|b| b.height() - 1 - b.baseline)
        .max()
        .unwrap_or(0);
    let height = above + below + 1;
    let width = parts.iter().map(|b| b.width).sum();
    fits(width, height)?;
    let mut rows = vec![String::new(); height];
    for b in parts {
        let top = above - b.baseline;
        for (r, row) in rows.iter_mut().enumerate() {
            match r.checked_sub(top).and_then(|i| b.rows.get(i)) {
                Some(text) => row.push_str(text),
                None => row.extend(std::iter::repeat_n(' ', b.width)),
            }
        }
    }
    Some(Block {
        rows,
        width,
        baseline: above,
    })
}

/// `parts` one above the other, each centred (an odd slack leaves the extra
/// column on the right); the baseline is that of `parts[base]`.
fn stack(parts: &[Block], base: usize) -> Option<Block> {
    let width = parts.iter().map(|b| b.width).max().unwrap_or(0);
    let height = parts.iter().map(Block::height).sum();
    fits(width, height)?;
    let mut rows = Vec::with_capacity(height);
    let mut baseline = 0;
    for (i, b) in parts.iter().enumerate() {
        if i == base {
            baseline = rows.len() + b.baseline;
        }
        let left = (width - b.width) / 2;
        for row in &b.rows {
            rows.push(padded(" ".repeat(left) + row, width));
        }
    }
    Some(Block {
        rows,
        width,
        baseline,
    })
}

/// `rows` one above the other, left-aligned, with the baseline at the
/// middle row (the upper one of two): matrices, cases, aligned rows.
fn column(rows: &[Block]) -> Option<Block> {
    let width = rows.iter().map(|b| b.width).max().unwrap_or(0);
    let height: usize = rows.iter().map(Block::height).sum();
    fits(width, height)?;
    let rows: Vec<String> = rows
        .iter()
        .flat_map(|b| b.rows.iter().map(|r| padded(r.clone(), width)))
        .collect();
    Some(Block {
        baseline: (rows.len().max(1) - 1) / 2,
        rows,
        width,
    })
}

/// `b` in parentheses one row tall, on its baseline.
fn paren(b: Block) -> Option<Block> {
    hcat(&[Block::text("("), b, Block::text(")")])
}

/// The [`Laid`] of `b`: rows without trailing blanks, without blank rows at
/// either end and without the blank columns all rows start with.
fn trimmed(b: Block) -> Option<Laid> {
    let rows: Vec<&str> = b.rows.iter().map(|r| r.trim_end()).collect();
    let first = rows.iter().position(|r| !r.is_empty())?;
    let last = rows.iter().rposition(|r| !r.is_empty())?;
    let indent = rows[first..=last]
        .iter()
        .filter(|r| !r.is_empty())
        .map(|r| r.len() - r.trim_start_matches(' ').len())
        .min()
        .unwrap_or(0);
    Some(Laid {
        rows: rows[first..=last]
            .iter()
            .map(|r| r.get(indent..).unwrap_or("").to_owned())
            .collect(),
        baseline: b.baseline.clamp(first, last) - first,
    })
}

// ── Layout ────────────────────────────────────────────────────────────────────

/// The box of `node` in `style`.
fn lay(node: &EqNode, style: Style) -> Option<Block> {
    match node {
        EqNode::Text(s) => Some(text(s, style)),
        EqNode::Space(_) => Some(Block::empty()),
        EqNode::Seq(children) => seq(children, style),
        EqNode::Sup(base, sup) => scripts(base, Some(sup), None, style),
        EqNode::Sub(base, sub) => scripts(base, None, Some(sub), style),
        EqNode::SupSub(base, sup, sub) => scripts(base, Some(sup), Some(sub), style),
        EqNode::Frac(num, den) => frac(num, den, style),
        EqNode::Sqrt(body) => {
            let inner = lay(body, style.inner())?;
            let inner = if atomic(&inner) { inner } else { paren(inner)? };
            hcat(&[Block::text("√"), inner])
        }
        EqNode::BigOp {
            symbol,
            lower,
            upper,
        } => {
            let lower = limit_box(lower.as_deref())?;
            let upper = limit_box(upper.as_deref())?;
            let sym = Block::text(symbol);
            if style == Style::Display && LIMITS_ABOVE.contains(&symbol.as_str()) {
                over_under(sym, upper, lower)
            } else {
                attach(sym, upper, lower)
            }
        }
        EqNode::Limit { name, lower } => {
            let lower = limit_box(lower.as_deref())?;
            let name = Block::text(name);
            if style == Style::Display && name.rows[0].starts_with("lim") {
                over_under(name, None, lower)
            } else {
                attach(name, None, lower)
            }
        }
        EqNode::Accent(body, kind) => accent(lay(body, style)?, *kind),
        EqNode::TextBlock(s) => Some(Block::text(s)),
        EqNode::MathFont { kind, content } => {
            Some(lay(content, style)?.map(|c| mathfont::map_char(*kind, c)))
        }
        EqNode::Delimited {
            left,
            right,
            content,
        } => delimited(left, right, lay(content, style)?),
        EqNode::Matrix { kind, rows } => matrix(*kind, rows, style),
        EqNode::Cases { rows } => cases(rows, style),
        EqNode::Binom(top, bottom) => binom(top, bottom, style),
        EqNode::Brace {
            content,
            label,
            over,
        } => annotate(content, label.as_deref(), *over, style),
        EqNode::StackRel {
            base,
            annotation,
            over,
        } => annotate(base, Some(annotation), *over, style),
    }
}

/// A text node: as it is, without whitespace in script style. Whitespace
/// alone is nothing (runs of it are [`seq`]'s).
fn text(s: &str, style: Style) -> Block {
    if s.trim().is_empty() {
        Block::empty()
    } else if style == Style::Script {
        Block::text(&s.chars().filter(|c| !c.is_whitespace()).collect::<String>())
    } else {
        Block::text(s)
    }
}

/// A sequence: its parts side by side. Outside script style a run of
/// spacing is one space (two for `\quad` and wider, none for a lone thin
/// space), never at either end; an operator name is followed by a space
/// unless a bracket follows it; and the operators of [`SPACED`] get one
/// space each side.
fn seq(children: &[EqNode], style: Style) -> Option<Block> {
    let mut nodes = Vec::new();
    flatten(children, &mut nodes);
    let mut parts: Vec<Block> = Vec::new();
    let mut gap = Gap::default();
    let mut after_name = false;
    for node in nodes {
        if let Some(points) = spacing(node) {
            gap.add(points);
            continue;
        }
        let spaced = matches!(node, EqNode::Text(s) if SPACED.contains(&s.as_str()));
        if spaced {
            gap.add(None);
        }
        if style != Style::Script && !parts.is_empty() {
            let mut spaces = gap.columns();
            if after_name && spaces == 0 && !opens_bracket(node) {
                spaces = 1;
            }
            if spaces > 0 {
                parts.push(Block::text(&" ".repeat(spaces)));
            }
        }
        gap = Gap::default();
        if spaced {
            gap.add(None);
        }
        parts.push(lay(node, style)?);
        after_name = is_name(node);
    }
    hcat(&parts)
}

/// A run of spacing nodes between two parts of a sequence.
#[derive(Default)]
struct Gap {
    /// A source space, or a space of 2 points or more, was seen...
    one: bool,
    /// ...or one of 12 points or more (`\quad`).
    two: bool,
}

impl Gap {
    /// Add a source space (`None`) or a `Space(points)`.
    fn add(&mut self, points: Option<f32>) {
        match points {
            None => self.one = true,
            Some(p) if p >= 12.0 => self.two = true,
            Some(p) if p >= 2.0 => self.one = true,
            Some(_) => {}
        }
    }

    fn columns(&self) -> usize {
        if self.two {
            2
        } else {
            usize::from(self.one)
        }
    }
}

/// `Some` for a spacing node: `None` inside for a source space, the points
/// of a `Space`.
fn spacing(node: &EqNode) -> Option<Option<f32>> {
    match node {
        EqNode::Space(points) => Some(Some(*points)),
        EqNode::Text(s) if s.chars().all(char::is_whitespace) => Some(None),
        _ => None,
    }
}

fn flatten<'a>(children: &'a [EqNode], out: &mut Vec<&'a EqNode>) {
    for child in children {
        match child {
            EqNode::Seq(inner) => flatten(inner, out),
            other => out.push(other),
        }
    }
}

/// Whether `node` is an operator name (`\sin`, `\log`), with or without
/// scripts.
fn is_name(node: &EqNode) -> bool {
    match node {
        EqNode::Limit { .. } => true,
        EqNode::Sup(base, _) | EqNode::Sub(base, _) | EqNode::SupSub(base, _, _) => {
            matches!(**base, EqNode::Limit { .. })
        }
        _ => false,
    }
}

/// Whether `node` starts with an opening bracket (`\sin(x)` keeps no space).
fn opens_bracket(node: &EqNode) -> bool {
    let first = match node {
        EqNode::Text(s) => s.chars().next(),
        EqNode::Delimited { left, .. } => left.chars().next(),
        _ => None,
    };
    matches!(first, Some('(' | '[' | '{'))
}

/// The box of a script or limit: script style; empty is none.
fn limit_box(node: Option<&EqNode>) -> Option<Option<Block>> {
    match node {
        None => Some(None),
        Some(node) => {
            let b = lay(node, Style::Script)?;
            Some((b.width > 0).then_some(b))
        }
    }
}

/// A base with its scripts.
fn scripts(
    base: &EqNode,
    sup: Option<&EqNode>,
    sub: Option<&EqNode>,
    style: Style,
) -> Option<Block> {
    let base = lay(base, style)?;
    attach(base, limit_box(sup)?, limit_box(sub)?)
}

/// `base` with a superscript and a subscript (either may be missing). When
/// every character of both has a script glyph, the glyphs follow the base
/// (subscript first, `zᵢ²`): on its row, or on its top and bottom rows when
/// it is taller. Otherwise both are rows of their own: the superscript
/// right above the base's top, the subscript right below its bottom,
/// left-aligned with each other.
fn attach(base: Block, sup: Option<Block>, sub: Option<Block>) -> Option<Block> {
    let sup_glyphs = sup.as_ref().map(|b| glyphs(b, superscript_char));
    let sub_glyphs = sub.as_ref().map(|b| glyphs(b, subscript_char));
    if sup_glyphs.as_ref().is_none_or(Option::is_some)
        && sub_glyphs.as_ref().is_none_or(Option::is_some)
    {
        let (sup, sub) = (sup_glyphs.flatten(), sub_glyphs.flatten());
        let (sup, sub) = (sup.unwrap_or_default(), sub.unwrap_or_default());
        if base.height() == 1 {
            let line = format!("{}{sub}{sup}", base.rows[0]);
            return Some(Block::text(&line));
        }
        let mut rows = base.rows;
        let last = rows.len() - 1;
        rows[0].push_str(&sup);
        rows[last].push_str(&sub);
        let width = rows.iter().map(|r| r.width()).max().unwrap_or(0);
        fits(width, rows.len())?;
        let rows = rows.into_iter().map(|r| padded(r, width)).collect();
        return Some(Block {
            rows,
            width,
            baseline: base.baseline,
        });
    }
    let above = sup.as_ref().map_or(0, Block::height);
    let below = sub.as_ref().map_or(0, Block::height);
    let script_width = sup.iter().chain(&sub).map(|b| b.width).max().unwrap_or(0);
    let width = base.width + script_width;
    let height = above + base.height() + below;
    fits(width, height)?;
    let blank = " ".repeat(base.width);
    let mut rows = Vec::with_capacity(height);
    for row in sup.iter().flat_map(|b| &b.rows) {
        rows.push(padded(blank.clone() + row, width));
    }
    for row in &base.rows {
        rows.push(padded(row.clone(), width));
    }
    for row in sub.iter().flat_map(|b| &b.rows) {
        rows.push(padded(blank.clone() + row, width));
    }
    Some(Block {
        rows,
        width,
        baseline: above + base.baseline,
    })
}

/// `b` in script glyphs (`map`), when it is one row and every character
/// has one.
fn glyphs(b: &Block, map: fn(char) -> Option<char>) -> Option<String> {
    let line = b.line()?;
    (!line.is_empty())
        .then(|| line.chars().map(map).collect())
        .flatten()
}

/// `base` with limits centred above and below it.
fn over_under(base: Block, upper: Option<Block>, lower: Option<Block>) -> Option<Block> {
    let index = usize::from(upper.is_some());
    let parts: Vec<Block> = upper.into_iter().chain([base]).chain(lower).collect();
    stack(&parts, index)
}

/// A fraction: in display style the numerator over a bar over the
/// denominator (both in text style), the bar as wide as the wider; else
/// flat, `num/den`, with parentheses where the one-line form puts them.
fn frac(num: &EqNode, den: &EqNode, style: Style) -> Option<Block> {
    let (num, den) = (lay(num, style.inner())?, lay(den, style.inner())?);
    if style == Style::Display {
        let bar = Block::text(&"─".repeat(num.width.max(den.width).max(1)));
        return stack(&[num, bar, den], 1);
    }
    let num = match num.line() {
        Some(line) if !top_level(line, |c| c.is_whitespace() || c == '/') => num,
        _ => paren(num)?,
    };
    let den = if atomic(&den) { den } else { paren(den)? };
    hcat(&[num, Block::text("/"), den])
}

/// Whether `b` reads as one operand: one row that [`is_atom`].
fn atomic(b: &Block) -> bool {
    b.line().is_some_and(is_atom)
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

pub(super) fn superscript_char(c: char) -> Option<char> {
    let i = "0123456789+-=()abcdefghijklmnoprstuvwxyz".find(c)?;
    "⁰¹²³⁴⁵⁶⁷⁸⁹⁺⁻⁼⁽⁾ᵃᵇᶜᵈᵉᶠᵍʰⁱʲᵏˡᵐⁿᵒᵖʳˢᵗᵘᵛʷˣʸᶻ".chars().nth(i)
}

pub(super) fn subscript_char(c: char) -> Option<char> {
    let i = "0123456789+-=()aehijklmnoprstuvx".find(c)?;
    "₀₁₂₃₄₅₆₇₈₉₊₋₌₍₎ₐₑₕᵢⱼₖₗₘₙₒₚᵣₛₜᵤᵥₓ".chars().nth(i)
}

/// `body` with an accent: a combining mark over a one-character body, else
/// `hat(…)` and the like.
fn accent(body: Block, kind: AccentKind) -> Option<Block> {
    let (mark, name) = match kind {
        AccentKind::Hat => ('\u{302}', "hat"),
        AccentKind::Bar => ('\u{305}', "bar"),
        AccentKind::Dot => ('\u{307}', "dot"),
        AccentKind::DoubleDot => ('\u{308}', "ddot"),
        AccentKind::Tilde => ('\u{303}', "tilde"),
        AccentKind::Vec => ('\u{20d7}', "vec"),
    };
    match body.line().map(str::trim) {
        Some(one) if one.chars().count() == 1 => Some(Block::text(&format!("{one}{mark}"))),
        _ => hcat(&[Block::text(&format!("{name}(")), body, Block::text(")")]),
    }
}

/// `\left…\right`: the delimiters as they are around one row, else drawn
/// as tall as `inner` (which keeps its baseline).
fn delimited(left: &str, right: &str, inner: Block) -> Option<Block> {
    if inner.height() == 1 {
        return hcat(&[Block::text(left), inner, Block::text(right)]);
    }
    let (l, r) = (
        fence(left, inner.height(), inner.baseline),
        fence(right, inner.height(), inner.baseline),
    );
    hcat(&[l, inner, r])
}

/// Delimiter `d` drawn `n` rows tall (n > 1) with its baseline at
/// `baseline`: brackets from their pieces, bars repeated, anything else
/// once on the middle row (the upper one of two).
fn fence(d: &str, n: usize, baseline: usize) -> Block {
    let (last, middle) = (n - 1, (n - 1) / 2);
    let rows = (0..n)
        .map(|r| {
            let pick = |top: &str, ext: &str, bottom: &str| match r {
                0 => top.to_owned(),
                r if r == last => bottom.to_owned(),
                _ => ext.to_owned(),
            };
            match d {
                "(" => pick("⎛", "⎜", "⎝"),
                ")" => pick("⎞", "⎟", "⎠"),
                "[" => pick("⎡", "⎢", "⎣"),
                "]" => pick("⎤", "⎥", "⎦"),
                "{" if n == 2 => pick("⎰", "", "⎱"),
                "}" if n == 2 => pick("⎱", "", "⎰"),
                "{" if r == middle && r > 0 => "⎨".to_owned(),
                "}" if r == middle && r > 0 => "⎬".to_owned(),
                "{" => pick("⎧", "⎪", "⎩"),
                "}" => pick("⎫", "⎪", "⎭"),
                "|" => "│".to_owned(),
                "⌊" | "⌋" if r < last => "│".to_owned(),
                "⌈" | "⌉" if r > 0 => "│".to_owned(),
                "‖" | "⌊" | "⌋" | "⌈" | "⌉" => d.to_owned(),
                _ if r == middle => d.to_owned(),
                _ => " ".repeat(d.width()),
            }
        })
        .collect();
    Block {
        rows,
        width: d.width(),
        baseline,
    }
}

/// A matrix: its cells in columns two spaces apart, each centred, between
/// the kind's brackets (plain ones around a one-row matrix).
fn matrix(kind: MatrixKind, rows: &[Vec<EqNode>], style: Style) -> Option<Block> {
    let grid: Vec<Vec<Block>> = rows
        .iter()
        .map(|row| row.iter().map(|cell| lay(cell, style)).collect())
        .collect::<Option<_>>()?;
    let grid: Vec<Vec<Block>> = grid
        .into_iter()
        .filter(|row| row.iter().any(|b| b.width > 0))
        .collect();
    let columns = grid.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|c| {
            grid.iter()
                .filter_map(|row| row.get(c))
                .map(|b| b.width)
                .max()
                .unwrap_or(0)
        })
        .collect();
    let lines = grid
        .iter()
        .map(|row| {
            let mut parts = Vec::new();
            for (c, cell) in row.iter().enumerate() {
                if c > 0 {
                    parts.push(Block::text("  "));
                }
                let slack = widths[c] - cell.width;
                parts.push(Block::text(&" ".repeat(slack / 2)));
                parts.push(cell.clone());
                parts.push(Block::text(&" ".repeat(slack - slack / 2)));
            }
            hcat(&parts)
        })
        .collect::<Option<Vec<_>>>()?;
    let body = column(&lines)?;
    let (left, right) = match kind {
        MatrixKind::Plain => ("", ""),
        MatrixKind::Paren => ("(", ")"),
        MatrixKind::Bracket => ("[", "]"),
        MatrixKind::VBar => ("|", "|"),
        MatrixKind::DoubleVBar => ("‖", "‖"),
        MatrixKind::Brace => ("{", "}"),
    };
    around(left, right, body)
}

/// `body` between `left` and `right`: plain around one row, else drawn as
/// tall as it.
fn around(left: &str, right: &str, body: Block) -> Option<Block> {
    if body.height() == 1 {
        return hcat(&[Block::text(left), body, Block::text(right)]);
    }
    let fence_of = |d: &str| {
        if d.is_empty() {
            Block {
                rows: vec![String::new(); body.height()],
                width: 0,
                baseline: body.baseline,
            }
        } else {
            fence(d, body.height(), body.baseline)
        }
    };
    hcat(&[fence_of(left), body.clone(), fence_of(right)])
}

/// `cases`: values in one column, conditions two spaces after the widest,
/// behind a brace (no `if`).
fn cases(rows: &[(EqNode, Option<EqNode>)], style: Style) -> Option<Block> {
    let laid: Vec<(Block, Option<Block>)> = rows
        .iter()
        .map(|(value, condition)| {
            let condition = match condition {
                Some(c) => Some(lay(c, style)?),
                None => None,
            };
            Some((lay(value, style)?, condition))
        })
        .collect::<Option<_>>()?;
    let width = laid.iter().map(|(v, _)| v.width).max().unwrap_or(0);
    let lines = laid
        .into_iter()
        .map(|(value, condition)| match condition {
            Some(c) => {
                let gap = Block::text(&" ".repeat(width - value.width + 2));
                hcat(&[value, gap, c])
            }
            None => Some(value),
        })
        .collect::<Option<Vec<_>>>()?;
    around("{", "", column(&lines)?)
}

/// `\binom`: `(n choose k)` on one line, or stacked in parentheses in
/// display style.
fn binom(top: &EqNode, bottom: &EqNode, style: Style) -> Option<Block> {
    let (top, bottom) = (lay(top, style.inner())?, lay(bottom, style.inner())?);
    if style == Style::Display {
        let pair = stack(&[top, bottom], 0)?;
        let pair = column(std::slice::from_ref(&pair))?;
        return around("(", ")", pair);
    }
    hcat(&[
        Block::text("("),
        top,
        Block::text(" choose "),
        bottom,
        Block::text(")"),
    ])
}

/// `\overbrace`, `\underbrace`, `\overset`, `\underset`: the annotation in
/// script style, centred above (`over`) or below the content.
fn annotate(content: &EqNode, label: Option<&EqNode>, over: bool, style: Style) -> Option<Block> {
    let content = lay(content, style)?;
    match limit_box(label)? {
        None => Some(content),
        Some(label) if over => stack(&[label, content], 1),
        Some(label) => stack(&[content, label], 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rows of `src` typeset in display style (`display`) or text style.
    fn rows(src: &str, display: bool) -> Vec<String> {
        let tree = rust_latex_parser::parse_equation(&rewrite_roots(src));
        typeset(&tree, display)
            .unwrap_or_else(|| panic!("nothing for {src}"))
            .rows
    }

    fn display(src: &str) -> Vec<String> {
        rows(src, true)
    }

    fn text(src: &str) -> Vec<String> {
        rows(src, false)
    }

    /// AC4: the golden rows of the spec (Appendix A, F3).
    #[test]
    fn display_formulas_match_the_golden_rows() {
        let golden: [(&str, &[&str]); 10] = [
            (
                "x = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}{2a}",
                &[
                    "    -b ± √(b² - 4ac)",
                    "x = ────────────────",
                    "           2a",
                ],
            ),
            (
                "p(x) = \\frac{1}{\\sqrt{2\\pi\\sigma^2}} e^{-\\frac{(x-\\mu)^2}{2\\sigma^2}}",
                &[
                    "          1     -(x-μ)²/(2σ²)",
                    "p(x) = ─────── e",
                    "       √(2πσ²)",
                ],
            ),
            (
                "S_{i,f} = \\sum_{s : P_s \\notin X} s_{P_s,i,f} + G_f(i)",
                &[
                    "S    =   ∑    s       + G (i)",
                    " i,f   s:Pₛ∉X  Pₛ,i,f    f",
                ],
            ),
            (
                "\\sum_{k=1}^{n} k = \\frac{n(n+1)}{2}",
                // The spec's middle row, " ∑ k = ────────", is one column
                // short of its other two (ruling requested in the plan).
                &[" n      n(n + 1)", " ∑  k = ────────", "k=1        2"],
            ),
            (
                "\\begin{pmatrix} a & b \\\\ c & d \\end{pmatrix}",
                &["⎛a  b⎞", "⎝c  d⎠"],
            ),
            (
                "\\begin{vmatrix} a & b \\\\ c & d \\end{vmatrix}",
                &["│a  b│", "│c  d│"],
            ),
            (
                "f(x) = \\begin{cases} 1 & x > 0 \\\\ 0 & \\text{otherwise} \\end{cases}",
                &["f(x) = ⎰1  x > 0", "       ⎱0  otherwise"],
            ),
            (
                "x^{a^b} + y_{i_j} + z_{i}^{2}",
                &[" aᵇ", "x   + y   + zᵢ²", "       iⱼ"],
            ),
            (
                "\\int_0^\\infty e^{-x^2} \\, dx = \\frac{\\sqrt{\\pi}}{2}",
                // The spec's first and last rows, " ∞  -x²      √π" and
                // " 0           2", are one column off its middle row
                // (ruling requested in the plan).
                &[" ∞ -x²      √π", "∫ e    dx = ──", " 0          2"],
            ),
            (
                "\\lim_{x \\to 0} \\frac{\\sin x}{x} = 1",
                &["    sin x", "lim ───── = 1", "x→0   x"],
            ),
        ];
        for (src, want) in golden {
            assert_eq!(display(src), want, "{src}");
        }
        assert_eq!(display("\\sin x"), ["sin x"]);
        assert_eq!(display("\\sin(x) + \\log_2 n"), ["sin(x) + log₂ n"]);
    }

    /// AC6: the condensed targets.
    #[test]
    fn the_layout_is_condensed() {
        let quadratic = display("x = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}{2a}");
        assert_eq!(quadratic.len(), 3);
        let gaussian =
            display("p(x) = \\frac{1}{\\sqrt{2\\pi\\sigma^2}} e^{-\\frac{(x-\\mu)^2}{2\\sigma^2}}");
        assert!(gaussian.len() <= 4, "{gaussian:?}");
        let s = display("S_{i,f} = \\sum_{s : P_s \\notin X} s_{P_s,i,f} + G_f(i)");
        let width = s.iter().map(|r| r.width()).max().unwrap();
        assert!(s.len() <= 3 && width <= 30, "{s:?}");
        for laid in [&quadratic, &gaussian, &s] {
            assert!(
                laid.iter().all(|r| !r.trim().is_empty()),
                "a blank row: {laid:?}"
            );
            assert!(
                laid.iter().any(|r| !r.starts_with(' ')),
                "a blank first column"
            );
            assert!(laid.iter().all(|r| !r.ends_with(' ')), "trailing blanks");
        }
    }

    #[test]
    fn text_style_is_flat_with_limits_as_scripts() {
        assert_eq!(text("\\frac{a+b}{c}"), ["(a + b)/c"]);
        assert_eq!(
            text("\\sum_{i=1}^{n} i = \\frac{n(n+1)}{2}"),
            ["∑ᵢ₌₁ⁿ i = n(n + 1)/2"]
        );
        assert_eq!(text("\\int_0^1 x^2\\,dx"), ["∫₀¹ x² dx"]);
        assert_eq!(text("\\lim_{n \\to \\infty} a_n"), ["lim    aₙ", "   n→∞"]);
        // Inside a display formula, a fraction's parts are text style.
        assert_eq!(display("\\frac{\\frac{a}{b}}{c}"), ["a/b", "───", " c"]);
    }

    #[test]
    fn whitespace_runs_are_one_space_and_quads_two() {
        assert_eq!(display("a \\quad b \\qquad c \\! d \\; e"), ["a  b  c d e"]);
        assert_eq!(display("a\\!b\\,c"), ["ab c"]);
        assert_eq!(display("  x  =  y  "), ["x = y"]);
        // Operators the parser leaves bare are spaced like the others.
        assert_eq!(display("a \\cdot b \\cup c"), ["a · b ∪ c"]);
        // Not in scripts.
        assert_eq!(display("x_{a \\cdot b}"), ["x", " a·b"]);
    }

    #[test]
    fn operator_names_are_spaced_from_their_argument() {
        assert_eq!(display("\\sin\\theta + \\cos(x)"), ["sin θ + cos(x)"]);
        assert_eq!(display("\\sin^2 x"), ["sin² x"]);
        assert_eq!(
            display("\\operatorname{rank}(A) \\leq \\min(m, n)"),
            ["rank(A) ≤ min(m, n)"]
        );
    }

    #[test]
    fn centring_leaves_the_odd_column_on_the_right() {
        assert_eq!(display("\\frac{ab}{c}"), ["ab", "──", "c"]);
        assert_eq!(display("\\frac{abcd}{c}"), ["abcd", "────", " c"]);
        assert_eq!(display("\\frac{a}{b}"), ["a", "─", "b"]);
    }

    #[test]
    fn roots_take_their_index_as_a_raised_script() {
        assert_eq!(rewrite_roots("\\sqrt[3]{x}"), "{}^{3}\\sqrt{x}");
        assert_eq!(
            rewrite_roots("\\sqrt [n]x + \\sqrt{y}"),
            "{}^{n}\\sqrt x + \\sqrt{y}"
        );
        assert_eq!(rewrite_roots("\\sqrt[{a]}]{x}"), "{}^{{a]}}\\sqrt{x}");
        assert_eq!(rewrite_roots("\\sqrt[3"), "\\sqrt[3");
        assert_eq!(display("\\sqrt[3]{x}"), ["³√x"]);
        assert_eq!(display("\\sqrt[3]{x+1}"), ["³√(x + 1)"]);
        assert_eq!(display("\\sqrt[\\alpha]{x}"), ["α", " √x"]);
        assert_eq!(display("\\sqrt{x}"), ["√x"]);
    }

    #[test]
    fn delimiters_stretch_to_their_content() {
        assert_eq!(display("\\left( x \\right)"), ["(x)"]);
        assert_eq!(
            display("\\left( \\frac{a}{b} \\right)^2"),
            ["⎛a⎞²", "⎜─⎟", "⎝b⎠"]
        );
        assert_eq!(
            display("\\left\\{ \\frac{a}{b} \\right."),
            ["⎧a", "⎨─", "⎩b"]
        );
        assert_eq!(
            display("\\left| \\frac{a}{b} \\right|"),
            ["│a│", "│─│", "│b│"]
        );
        assert_eq!(
            display("\\left[ \\frac{a}{b} \\right]"),
            ["⎡a⎤", "⎢─⎥", "⎣b⎦"]
        );
    }

    #[test]
    fn accents_binomials_and_annotations() {
        assert_eq!(display("\\hat{x} + \\vec{AB}"), ["x\u{302} + vec(AB)"]);
        assert_eq!(text("\\binom{n}{k}"), ["(n choose k)"]);
        assert_eq!(display("\\binom{n}{k}"), ["⎛n⎞", "⎝k⎠"]);
        assert_eq!(display("\\overbrace{a+b}^{n}"), ["  n", "a + b"]);
        assert_eq!(display("\\underbrace{x+y}_{2}"), ["x + y", "  2"]);
        assert_eq!(display("\\overset{def}{=}"), ["def", " ="]);
    }

    #[test]
    fn matrices_and_cases_of_other_shapes() {
        assert_eq!(display("\\begin{bmatrix} 1 & 2 \\end{bmatrix}"), ["[1  2]"]);
        assert_eq!(
            display("\\begin{bmatrix} 1 & 20 \\\\ 300 & 4 \\\\ 5 & 6 \\end{bmatrix}"),
            ["⎡ 1   20⎤", "⎢300  4 ⎥", "⎣ 5   6 ⎦"]
        );
        assert_eq!(
            display("\\begin{cases} a \\\\ b \\\\ c \\end{cases}"),
            ["⎧a", "⎨b", "⎩c"]
        );
        assert_eq!(
            display("x = \\begin{Vmatrix} a \\\\ b \\end{Vmatrix}"),
            ["x = ‖a‖", "    ‖b‖"]
        );
    }

    #[test]
    fn math_fonts_map_letters() {
        assert_eq!(display("\\mathbb{R}^n"), ["ℝⁿ"]);
        assert_eq!(display("\\mathbf{v} + \\mathcal{L}"), ["𝐯 + ℒ"]);
    }

    #[test]
    fn layouts_past_the_limits_are_none() {
        let wide = "x + ".repeat(MAX_COLS / 4 + 1);
        let tree = rust_latex_parser::parse_equation(&wide);
        assert!(typeset(&tree, true).is_none());
        let tall = format!(
            "\\begin{{pmatrix}} {} \\end{{pmatrix}}",
            "a \\\\ ".repeat(MAX_ROWS + 1)
        );
        let tree = rust_latex_parser::parse_equation(&tall);
        assert!(typeset(&tree, true).is_none());
        // Nothing visible is nothing.
        let tree = rust_latex_parser::parse_equation("\\, \\quad");
        assert!(typeset(&tree, true).is_none());
    }
}
