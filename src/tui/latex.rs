//! `<latex>` formulas: rendering them as Unicode text, and laying them over
//! the session pane.
//!
//! Pure functions only. The render itself ([`render`]) parses and lays out
//! attacker-influenced text with a parser that is slow and deep on hostile
//! input, so it never runs on the UI thread: `tui/mod.rs` calls it from its
//! `latex-render` worker, on a scratch thread with a time budget, after
//! [`too_complex`] has turned away what could overflow that thread's stack.
//!
//! - [`Formula`] — one formula's source, its renderings and matching keys.
//! - [`render`] — the 2D renderings (`typeset.rs`) and the one-line form.
//! - [`too_complex`] — the pre-check that runs before any parsing.
//! - [`overlay`] — the pane's `<latex>` tags replaced by one-line renderings.

use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::typeset::{self, Environment, Laid};
use crate::claude::latex::{is_unsafe, sanitize, MAX_FORMULA_BYTES};
use crate::term::screen::Screen;

/// Deepest `{`…`}` nesting a formula may have to be rendered.
const MAX_DEPTH: usize = 32;
/// Most recursion-prone tokens a formula may have to be rendered: every
/// `\command` (`\left`, `\begin` included), every `/`, and every `(` right
/// after a bareword (`sqrt(`, `frac(`). The quadratic formula has about 12.
const MAX_TOKENS: usize = 256;
/// The largest 2D rendering kept.
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
    /// The display-style rendering: the reader's display formulas and its
    /// formulas list.
    pub block: Option<Laid>,
    /// The text-style rendering: the reader's formulas inside a line.
    pub text: Option<Laid>,
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
    pub fn new(src: &str, rendered: Rendered) -> Formula {
        let unescaped = md_unescape(src);
        Formula {
            src: src.to_owned(),
            inline: rendered.inline,
            block: rendered.block,
            text: rendered.text,
            key_strict: normalize(&unescaped),
            key_loose: normalize_loose(&unescaped),
            key_html: normalize_loose(&without_html(&unescaped)),
        }
    }

    /// `src` shown as it is: not rendered (too complex, too slow, or the
    /// renderer failed).
    pub fn raw(src: &str) -> Formula {
        Formula::new(src, Rendered::default())
    }
}

/// What [`render`] makes of a formula; `None` for what does not fit or
/// comes out empty.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Rendered {
    /// One line, at most [`INLINE_MAX_COLS`] wide.
    pub inline: Option<String>,
    /// Display style, at most [`BLOCK_MAX_COLS`] × [`BLOCK_MAX_ROWS`].
    pub block: Option<Laid>,
    /// Text style, within the same limits.
    pub text: Option<Laid>,
}

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

/// Render a [`prepare`]d formula with the typesetting engine: in display
/// style, in text style, and on one line, which is the text style when that
/// is one row and the v0.4.0 walk of the parse tree otherwise. A formula
/// made of an `aligned`-like environment is rendered row by row; one with
/// an environment the parser does not know, or past [`too_complex`] once
/// `\sqrt[n]` is rewritten, is not rendered. Everything is [`sanitize`]d.
///
/// May be slow, deep or panic on hostile input: call it only through the
/// guard in `tui/mod.rs`, after [`too_complex`].
pub fn render(src: &str) -> Rendered {
    let parts = match typeset::environment(src) {
        Environment::None => vec![src.to_owned()],
        Environment::Rows(rows) => rows,
        Environment::Unknown => return Rendered::default(),
    };
    let (mut blocks, mut texts, mut lines) = (Vec::new(), Vec::new(), Vec::new());
    for part in parts {
        let part = typeset::rewrite_roots(&part);
        if too_complex(&part) {
            return Rendered::default();
        }
        let tree = rust_latex_parser::parse_equation(&part);
        let text = typeset::typeset(&tree, false);
        lines.push(match &text {
            Some(one) if one.rows.len() == 1 => one.rows[0].clone(),
            _ => typeset::walk::one_line(&tree),
        });
        blocks.push(typeset::typeset(&tree, true));
        texts.push(text);
    }
    let line = sanitize(&lines.join("; ")).trim().to_owned();
    let inline = (!line.is_empty() && line.width() <= INLINE_MAX_COLS).then_some(line);
    Rendered {
        inline,
        block: capped(typeset::stacked(blocks)),
        text: capped(typeset::stacked(texts)),
    }
}

/// `laid`, [`sanitize`]d, if it fits [`BLOCK_MAX_COLS`] × [`BLOCK_MAX_ROWS`].
fn capped(laid: Option<Laid>) -> Option<Laid> {
    let laid = laid?;
    if laid.width() > BLOCK_MAX_COLS || laid.rows.len() > BLOCK_MAX_ROWS {
        return None;
    }
    Some(Laid {
        rows: laid.rows.iter().map(|r| sanitize(r)).collect(),
        baseline: laid.baseline,
    })
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

// ── Pane overlay ──────────────────────────────────────────────────────────────

/// The colour of a formula rendered in the pane.
const ACCENT: Color = Color::Cyan;
/// The most screen rows one tag may span: claude wraps long lines itself,
/// indenting the rows that continue one.
const MAX_SPAN_ROWS: u16 = 8;
const OPEN_TAG: &str = "<latex>";
const CLOSE_TAG: &str = "</latex>";

/// Replace, in the pane `buf` has just been drawn with from `screen` at
/// `area`, each complete `<latex>…</latex>` whose text matches one of
/// `formulas` by that formula's one-line rendering, in [`ACCENT`]. A tag may
/// continue on the next rows (up to [`MAX_SPAN_ROWS`]) when it is the only
/// one on its first row. Rows are rebuilt from their own cells, so no row
/// moves; a row (or tag) whose result would not fit stays as it was. The
/// selection, copy and the cursor still go by `screen`.
pub fn overlay(buf: &mut Buffer, area: Rect, screen: &Screen, formulas: &[Formula]) {
    let rows = area.height.min(screen.size().0);
    let mut row = 0;
    while row < rows {
        row = if shows_open_tag(buf, area, row) {
            overlay_row(buf, area, screen, formulas, row, rows)
        } else {
            row + 1
        };
    }
}

/// Whether row `row` of the drawn pane shows `<latex>`: a look at its
/// cells, so a row without one costs nothing more.
fn shows_open_tag(buf: &Buffer, area: Rect, row: u16) -> bool {
    let tag = OPEN_TAG.as_bytes();
    let mut matched = 0;
    for x in area.left()..area.right() {
        let Some(cell) = buf.cell((x, area.y + row)) else {
            return false;
        };
        matched = match cell.symbol().as_bytes() {
            [b] if *b == tag[matched] => matched + 1,
            b"<" => 1,
            _ => 0,
        };
        if matched == tag.len() {
            return true;
        }
    }
    false
}

/// Lay the formulas over the tags that start on `row`; returns the next row
/// to look at (past a tag that continues on later rows).
fn overlay_row(
    buf: &mut Buffer,
    area: Rect,
    screen: &Screen,
    formulas: &[Formula],
    row: u16,
    rows: u16,
) -> u16 {
    let line = Row::read(screen, row, area.width);
    let tags = line.tags();
    match tags.as_slice() {
        [Tag { open, close: None }] => {
            overlay_span(buf, area, screen, formulas, (row, rows), &line, *open)
        }
        _ => {
            rebuild_row(buf, area, row, &line, &tags, formulas);
            row + 1
        }
    }
}

/// Rebuild `row` with each closed tag that matches a formula replaced by
/// its rendering, unless the row would no longer fit. An unmatched or
/// unclosed tag stays as it is.
fn rebuild_row(
    buf: &mut Buffer,
    area: Rect,
    row: u16,
    line: &Row,
    tags: &[Tag],
    formulas: &[Formula],
) {
    let original = cells_of(buf, area, row);
    let mut out = Vec::with_capacity(original.len());
    let mut copied = 0;
    for tag in tags {
        let Some(close) = tag.close else { continue };
        let (start, end) = (line.col(tag.open), line.col(close + CLOSE_TAG.len()));
        let text = line.text(tag.open + OPEN_TAG.len(), close);
        let Some(cells) = matching(formulas, &text)
            .and_then(|inline| text_cells(inline, accent_on(&original[start])))
        else {
            continue;
        };
        out.extend_from_slice(&original[copied..start]);
        out.extend(cells);
        copied = end;
    }
    if copied == 0 {
        return;
    }
    out.extend_from_slice(&original[copied..content_end(&original).max(copied)]);
    if out.len() <= original.len() {
        write_row(buf, area, row, out, &original);
    }
}

/// A tag opened on `row` (its only one) that closes on one of the next
/// [`MAX_SPAN_ROWS`] rows; its text is its rows' text after their
/// indentation, joined by spaces. When it matches, the first row ends with
/// the rendering, then the text after the tag if it fits there (else that
/// text stays on the last row, after its indentation), and the tag's other
/// rows are blanked. Returns the next row to look at.
fn overlay_span(
    buf: &mut Buffer,
    area: Rect,
    screen: &Screen,
    formulas: &[Formula],
    (row, rows): (u16, u16),
    first: &Row,
    open: usize,
) -> u16 {
    let mut text = first.text(open + OPEN_TAG.len(), first.glyphs.len());
    let mut closing = None;
    for last in (row + 1..rows).take(MAX_SPAN_ROWS as usize) {
        let line = Row::read(screen, last, area.width);
        // Another tag in the way: this one is not laid over.
        if line.find(OPEN_TAG, 0).is_some() {
            break;
        }
        let indent = line.indent();
        let close = line.find(CLOSE_TAG, indent);
        text.push(' ');
        text.push_str(&line.text(indent, close.unwrap_or(line.glyphs.len())));
        if let Some(close) = close {
            closing = Some((last, line, indent, close));
            break;
        }
    }
    let Some((last, line, indent, close)) = closing else {
        return row + 1;
    };
    let first_cells = cells_of(buf, area, row);
    let start = first.col(open);
    let Some(rendered) = matching(formulas, &text)
        .and_then(|inline| text_cells(inline, accent_on(&first_cells[start])))
    else {
        return last + 1;
    };
    let width = first_cells.len();
    let mut head = first_cells[..start].to_vec();
    head.extend(rendered);
    if head.len() > width {
        return last + 1;
    }
    let last_cells = cells_of(buf, area, last);
    let suffix_start = line.col(close + CLOSE_TAG.len());
    let suffix = &last_cells[suffix_start..content_end(&last_cells).max(suffix_start)];
    let suffix_fits = head.len() + suffix.len() <= width;
    if suffix_fits {
        head.extend_from_slice(suffix);
    }
    write_row(buf, area, row, head, &first_cells);
    for blank in row + 1..last {
        let cells = cells_of(buf, area, blank);
        write_row(buf, area, blank, Vec::new(), &cells);
    }
    let mut tail = Vec::new();
    if !suffix_fits {
        tail.extend_from_slice(&last_cells[..line.col(indent)]);
        tail.extend_from_slice(suffix);
    }
    write_row(buf, area, last, tail, &last_cells);
    last + 1
}

/// The one-line rendering of the newest formula whose key matches a tag's
/// on-screen `text`: strictly, then without emphasis characters, then also
/// without HTML-looking runs (markdown ate them from the screen). `None`
/// when no formula matches, or the matching one has no rendering.
fn matching<'a>(formulas: &'a [Formula], text: &str) -> Option<&'a str> {
    let (strict, loose) = (normalize(text), normalize_loose(text));
    let newest = || formulas.iter().rev();
    newest()
        .find(|f| f.key_strict == strict)
        .or_else(|| newest().find(|f| f.key_loose == loose))
        .or_else(|| newest().find(|f| f.key_html == loose))?
        .inline
        .as_deref()
}

/// The accent colour on the background the tag had.
fn accent_on(cell: &Cell) -> Style {
    Style::default().fg(ACCENT).bg(cell.bg)
}

/// `text` as pane cells in `style`: one grapheme cluster per cell, a wide
/// one followed by the blank cell it covers (as `Buffer::set_string` lays
/// them out). Clusters of width 0, or holding a character [`sanitize`]
/// removes, are dropped; `None` when one is wider than two columns.
pub(super) fn text_cells(text: &str, style: Style) -> Option<Vec<Cell>> {
    let mut cells = Vec::with_capacity(text.len());
    for cluster in text.graphemes(true) {
        if cluster
            .chars()
            .any(|c| is_unsafe(c) || c == '\n' || c == '\t')
        {
            continue;
        }
        let mut cell = Cell::default();
        cell.set_symbol(cluster).set_style(style);
        match cluster.width() {
            0 => {}
            1 => cells.push(cell),
            2 => cells.extend([cell, Cell::default()]),
            _ => return None,
        }
    }
    Some(cells)
}

/// The cells of pane row `row`.
fn cells_of(buf: &Buffer, area: Rect, row: u16) -> Vec<Cell> {
    (area.left()..area.right())
        .map(|x| buf[(x, area.y + row)].clone())
        .collect()
}

/// Where a row's content ends: past its last non-blank cell (and the cell a
/// wide one covers).
fn content_end(cells: &[Cell]) -> usize {
    cells
        .iter()
        .rposition(|c| c.symbol() != " ")
        .map_or(0, |i| {
            (i + cells[i].symbol().width().max(1)).min(cells.len())
        })
}

/// Put `cells` on pane row `row`, padded with blanks styled like the row's
/// last cell (of `original`).
fn write_row(buf: &mut Buffer, area: Rect, row: u16, cells: Vec<Cell>, original: &[Cell]) {
    let mut blank = original.last().cloned().unwrap_or_default();
    blank.set_symbol(" ");
    let padded = cells.into_iter().chain(std::iter::repeat(blank));
    for (x, cell) in (area.left()..area.right()).zip(padded) {
        buf[(x, area.y + row)] = cell;
    }
}

/// A `<latex>` on a row, by glyph index: its `<`, and the `<` of its
/// `</latex>` when it closes on the same row.
struct Tag {
    open: usize,
    close: Option<usize>,
}

/// The characters of a screen row: each once (a wide one at its first
/// column), with the column it starts at.
struct Row {
    glyphs: Vec<(usize, char)>,
    width: usize,
}

impl Row {
    fn read(screen: &Screen, row: u16, width: u16) -> Row {
        let mut chars = screen.row_chars(row);
        chars.truncate(width as usize);
        let mut glyphs = Vec::with_capacity(chars.len());
        let mut col = 0;
        while col < chars.len() {
            let c = chars[col];
            glyphs.push((col, c));
            // `row_chars` repeats a wide character in its second column.
            col += c.width().unwrap_or(1).max(1);
        }
        Row {
            glyphs,
            width: chars.len(),
        }
    }

    /// Every `<latex>` that starts on the row, in order; only the last one
    /// can be unclosed.
    fn tags(&self) -> Vec<Tag> {
        let mut tags = Vec::new();
        let mut at = 0;
        while let Some(open) = self.find(OPEN_TAG, at) {
            let close = self.find(CLOSE_TAG, open + OPEN_TAG.len());
            tags.push(Tag { open, close });
            match close {
                Some(close) => at = close + CLOSE_TAG.len(),
                None => break,
            }
        }
        tags
    }

    /// The glyph index of the next `tag` (ASCII) at or after glyph `from`.
    fn find(&self, tag: &str, from: usize) -> Option<usize> {
        let n = tag.len();
        let last = self.glyphs.len().checked_sub(n)?;
        (from..=last).find(|&i| self.glyphs[i..i + n].iter().map(|g| g.1).eq(tag.chars()))
    }

    /// The text of glyphs `from..to`, trailing blanks dropped.
    fn text(&self, from: usize, to: usize) -> String {
        let text: String = self.glyphs[from..to].iter().map(|g| g.1).collect();
        text.trim_end().to_owned()
    }

    /// The column glyph `i` starts at (the row's width past its end).
    fn col(&self, i: usize) -> usize {
        self.glyphs.get(i).map_or(self.width, |g| g.0)
    }

    /// The index of the first non-blank glyph.
    fn indent(&self) -> usize {
        self.glyphs
            .iter()
            .position(|g| !g.1.is_whitespace())
            .unwrap_or(self.glyphs.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one-line rendering of `src`.
    fn inline(src: &str) -> String {
        render(&prepare(src))
            .inline
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

    /// The rows of a rendering.
    fn rows(laid: Option<Laid>) -> Vec<String> {
        laid.expect("rendered").rows
    }

    #[test]
    fn the_renderings_are_the_engines() {
        let r = render(&prepare("\\frac{a}{b}"));
        assert_eq!(rows(r.block), ["a", "─", "b"]);
        assert_eq!(rows(r.text), ["a/b"]);
        assert_eq!(r.inline.as_deref(), Some("a/b"));
        let r = render(&prepare("E = mc^2"));
        assert_eq!(rows(r.block), ["E = mc²"]);
        // Taller than one row in text style: the one-line walk.
        let r = render(&prepare("e^{i\\pi}"));
        assert_eq!(rows(r.text), [" iπ", "e"]);
        assert_eq!(r.inline.as_deref(), Some("e^(iπ)"));
        assert_eq!(
            render(&prepare("\\sqrt[3]{x}")).inline.as_deref(),
            Some("³√x")
        );
    }

    #[test]
    fn aligned_rows_are_rendered_one_by_one() {
        let r = render(&prepare(
            "\\begin{aligned} a &= b \\\\ c &= d \\end{aligned}",
        ));
        assert_eq!(rows(r.block), ["a = b", "c = d"]);
        assert_eq!(r.inline.as_deref(), Some("a = b; c = d"));
        // An environment the parser does not know is not rendered at all.
        assert_eq!(
            render(&prepare("\\begin{tabular}{c} a \\end{tabular}")),
            Rendered::default()
        );
        // A rewritten root counts against the pre-check too: its index
        // gains a brace.
        let deep = format!("\\sqrt[{}x{}]{{y}}", "{".repeat(32), "}".repeat(32));
        assert!(!too_complex(&deep));
        assert_eq!(render(&deep), Rendered::default());
    }

    #[test]
    fn renderings_are_sanitized() {
        // A bidi override and a zero-width space never reach a cell.
        let src = prepare("a\u{202e}b\u{200b} + c");
        assert_eq!(src, "ab + c");
        let r = render("\\text{a\u{202e}b}");
        assert_eq!(r.inline.as_deref(), Some("ab"));
        assert_eq!(rows(r.block), ["ab"]);
        assert_eq!(rows(r.text), ["ab"]);
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

    // ── overlay ───────────────────────────────────────────────────────────────

    /// A 60×6 screen showing `text`.
    fn screen(text: &str) -> Screen {
        let mut screen = Screen::new(6, 60);
        screen.feed(text.replace('\n', "\r\n").as_bytes());
        screen
    }

    /// The pane `screen` draws, with `formulas` laid over it.
    fn drawn(screen: &Screen, formulas: &[Formula]) -> Buffer {
        let (rows, cols) = screen.size();
        let area = Rect::new(0, 0, cols, rows);
        let mut buf = Buffer::empty(area);
        screen.render(area, &mut buf);
        overlay(&mut buf, area, screen, formulas);
        buf
    }

    /// The rows of `buf` as text: each cell's symbol once (a wide one's
    /// covered cell skipped), trailing blanks dropped.
    fn rows_of(buf: &Buffer) -> Vec<String> {
        let area = buf.area;
        (0..area.height)
            .map(|y| {
                let mut line = String::new();
                let mut covered = 0;
                for x in 0..area.width {
                    if covered > 0 {
                        covered -= 1;
                        continue;
                    }
                    let symbol = buf[(x, y)].symbol();
                    line.push_str(symbol);
                    covered = symbol.width().saturating_sub(1);
                }
                line.trim_end().to_owned()
            })
            .collect()
    }

    fn shown(screen: &Screen, formulas: &[Formula]) -> Vec<String> {
        rows_of(&drawn(screen, formulas))
    }

    /// A formula `src` whose one-line rendering is `inline`.
    fn formula(src: &str, inline: &str) -> Formula {
        let rendered = Rendered {
            inline: Some(inline.to_owned()),
            ..Rendered::default()
        };
        Formula::new(src, rendered)
    }

    #[test]
    fn a_tag_on_one_row_is_replaced_in_the_accent_colour() {
        let s = screen("before\nso <latex>E = mc^2</latex>, done\nafter");
        let buf = drawn(&s, &[formula("E = mc^2", "E = mc²")]);
        assert_eq!(rows_of(&buf)[..3], ["before", "so E = mc², done", "after"]);
        assert_eq!(buf[(3, 1)].fg, ACCENT);
        assert_ne!(buf[(10, 1)].fg, ACCENT, "the rest keeps its colour");
        // Without formulas nothing changes.
        assert_eq!(shown(&s, &[])[1], "so <latex>E = mc^2</latex>, done");
    }

    #[test]
    fn two_tags_on_a_row_are_both_replaced() {
        let s = screen("<latex>\\alpha</latex> and <latex>\\beta</latex>!");
        let formulas = [formula("\\alpha", "α"), formula("\\beta", "β")];
        assert_eq!(shown(&s, &formulas)[0], "α and β!");
    }

    #[test]
    fn tags_match_after_markdown_ate_escapes_emphasis_or_html() {
        let cases = [
            // `\{` and `\,` lost their backslash on screen.
            (
                "\\{a_i\\} \\cup \\{b_i\\}",
                "{a_i} \\cup {b_i}",
                "{aᵢ} ∪ {bᵢ}",
            ),
            ("x^2\\,dx", "x^2,dx", "x² dx"),
            // `_i + b_` was taken for emphasis.
            ("a_i + b_i", "ai + bi", "aᵢ + bᵢ"),
            // `<y>` was taken for an HTML tag.
            ("x <y> z", "x  z", "x y z"),
        ];
        for (src, on_screen, inline) in cases {
            let s = screen(&format!("<latex>{on_screen}</latex>"));
            assert_eq!(shown(&s, &[formula(src, inline)])[0], inline, "{src}");
        }
    }

    #[test]
    fn the_newest_matching_formula_wins() {
        let s = screen("<latex>x</latex>");
        let formulas = [formula("x", "old"), formula("x", "new")];
        assert_eq!(shown(&s, &formulas)[0], "new");
    }

    #[test]
    fn a_tag_continued_on_an_indented_row_is_joined() {
        let s =
            screen("root: <latex>x = \\frac{-b \\pm\n  \\sqrt{b^2 - 4ac}}{2a}</latex> here\nnext");
        let f = formula(
            "x = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}{2a}",
            "x = (-b ± √(b² - 4ac))/(2a)",
        );
        assert_eq!(
            shown(&s, &[f])[..3],
            ["root: x = (-b ± √(b² - 4ac))/(2a) here", "", "next"]
        );
    }

    #[test]
    fn a_suffix_that_no_longer_fits_stays_on_the_last_row() {
        let s = screen(&format!("{}<latex>a\n  b</latex> tail", "p".repeat(50)));
        let f = formula("a b", "abcdefgh");
        let want = [format!("{}abcdefgh", "p".repeat(50)), "   tail".to_owned()];
        assert_eq!(shown(&s, &[f])[..2], want);
    }

    #[test]
    fn tags_that_cannot_be_replaced_stay_raw() {
        let raw = |text: &str, formulas: &[Formula]| {
            let s = screen(text);
            assert_eq!(shown(&s, formulas), shown(&s, &[]), "{text}");
        };
        let ab = [formula("a b", "AB")];
        raw("<latex>z</latex>", &ab); // no formula matches
        raw("<latex>a b", &ab); // never closed
        raw("<latex>a b</latex>", &[Formula::raw("a b")]); // no rendering
        let wide = [formula("a b", &"w".repeat(30))];
        raw(&format!("{}<latex>a b</latex>", "p".repeat(40)), &wide); // too wide
                                                                      // A tag continued on the next row after another tag on its first row
                                                                      // stays raw, while that other tag is still replaced.
        raw("<latex>q</latex> <latex>a\n  b</latex>", &ab);
        let s = screen("<latex>q</latex> <latex>a\n  b</latex>");
        assert_eq!(
            shown(&s, &[formula("q", "Q")])[..2],
            ["Q <latex>a", "  b</latex>"]
        );
        // A tag that runs past MAX_SPAN_ROWS rows.
        let long = format!(
            "<latex>a{}\n  b</latex>",
            "\n  x".repeat(MAX_SPAN_ROWS as usize)
        );
        let mut s = Screen::new(14, 60);
        s.feed(long.replace('\n', "\r\n").as_bytes());
        assert_eq!(shown(&s, &ab), shown(&s, &[]));
    }

    #[test]
    fn wide_characters_after_a_tag_keep_their_two_cells() {
        let s = screen("<latex>a</latex>漢字x");
        let buf = drawn(&s, &[formula("a", "α")]);
        assert_eq!(rows_of(&buf)[0], "α漢字x");
        assert_eq!(buf[(1, 0)].symbol(), "漢");
        assert_eq!(buf[(3, 0)].symbol(), "字");
        assert_eq!(buf[(5, 0)].symbol(), "x");
    }

    #[test]
    fn unsafe_clusters_never_become_cells() {
        let style = Style::default();
        let cells = text_cells("a\u{202e}b\u{1b}c\u{200b}", style).unwrap();
        let text: String = cells.iter().map(Cell::symbol).collect();
        assert_eq!(text, "abc");
        // A wide cluster takes its cell and the one it covers.
        assert_eq!(text_cells("漢", style).unwrap().len(), 2);
    }
}
