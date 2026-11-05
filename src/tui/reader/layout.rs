//! A [`Doc`] wrapped to the reader view's width: styled lines, formulas in
//! TeX's line boxes. Pure; the view caches the result for its width.
//!
//! - Inline formulas are unbreakable boxes in the text flow, at most three
//!   rows (one above and one below the baseline); a line is as tall as its
//!   tallest box, baselines aligned. A taller formula, or one wider than
//!   the view, is its one-line form; one that was not rendered is its
//!   source, dimmed.
//! - A display formula is its own block, two columns in; wider than the
//!   view, it is its one-line form, wrapped.
//! - Code blocks are never wrapped (cut at the edge); tables are aligned
//!   columns when they fit, else one `a │ b │ c` paragraph per row.

use ratatui::style::{Modifier, Style};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::doc::{Block, Doc, Emphasis, Span};
use crate::tui::latex::{prepare, Formula};
use crate::tui::typeset::Laid;

/// How many rows an inline formula may have above and below its baseline.
const INLINE_REACH: usize = 1;
/// The indentation of a display formula, a code block and a quote's text.
const INDENT: &str = "  ";
const GUTTER: &str = "│ ";
const CELL_SEPARATOR: &str = " │ ";

/// One drawn line: styled pieces of text, left to right.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Line(pub Vec<Seg>);

#[derive(Debug, Clone, PartialEq)]
pub struct Seg {
    pub text: String,
    pub style: Style,
}

impl Line {
    /// The line's text, styles dropped.
    #[cfg(test)]
    pub fn text(&self) -> String {
        self.0.iter().map(|s| s.text.as_str()).collect()
    }

    fn width(&self) -> usize {
        self.0.iter().map(|s| s.text.width()).sum()
    }

    /// Append `text` in `style`, joined to the last piece when it has the
    /// same style.
    fn push(&mut self, text: &str, style: Style) {
        if text.is_empty() {
            return;
        }
        match self.0.last_mut() {
            Some(last) if last.style == style => last.text.push_str(text),
            _ => self.0.push(Seg {
                text: text.to_owned(),
                style,
            }),
        }
    }

    /// This line after `prefix` in `style`.
    fn after(self, prefix: &str, style: Style) -> Line {
        let mut line = Line::default();
        line.push(prefix, style);
        for seg in self.0 {
            line.push(&seg.text, seg.style);
        }
        line
    }
}

/// `doc`, whose formulas are `formulas`, as lines at most `width` columns
/// wide (code blocks and over-deep indentation aside, which the drawing
/// cuts).
pub fn layout(doc: &Doc, formulas: &[Formula], width: usize) -> Vec<Line> {
    Ctx { formulas }.blocks(&doc.blocks, width.max(1), true)
}

fn dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

fn bold() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

/// `style` with `emphasis` added.
fn emphasized(style: Style, emphasis: Emphasis) -> Style {
    let mut style = style;
    for (on, modifier) in [
        (emphasis.bold, Modifier::BOLD),
        (emphasis.italic, Modifier::ITALIC),
        (emphasis.strike, Modifier::CROSSED_OUT),
    ] {
        if on {
            style = style.add_modifier(modifier);
        }
    }
    style
}

/// `s` with every grapheme wider than two columns replaced by `�`, so no
/// line is lost to one (the cells take at most two columns each).
pub fn safe(s: &str) -> String {
    s.graphemes(true)
        .map(|g| if g.width() > 2 { "\u{fffd}" } else { g })
        .collect()
}

/// A piece of a paragraph, for wrapping.
#[derive(Debug, Clone)]
enum Item {
    /// Text without spaces.
    Text(String, Style),
    /// A formula box.
    Box(Laid),
    /// A space a line may break at.
    Space,
    /// A forced line break.
    Break,
}

impl Item {
    fn width(&self) -> usize {
        match self {
            Item::Text(t, _) => t.width(),
            Item::Box(laid) => laid.width(),
            Item::Space => 1,
            Item::Break => 0,
        }
    }
}

/// Text as items: words, and one space for each run of whitespace.
fn words(text: &str, style: Style, out: &mut Vec<Item>) {
    let mut word = String::new();
    for c in text.chars() {
        if c.is_whitespace() {
            if !word.is_empty() {
                out.push(Item::Text(safe(&std::mem::take(&mut word)), style));
            }
            if !matches!(out.last(), Some(Item::Space) | None) {
                out.push(Item::Space);
            }
        } else {
            word.push(c);
        }
    }
    if !word.is_empty() {
        out.push(Item::Text(safe(&word), style));
    }
}

/// Whether `laid` may sit in a line of text: three rows at most, one above
/// and one below its baseline.
fn fits_inline(laid: &Laid) -> bool {
    laid.baseline <= INLINE_REACH && laid.rows.len() <= laid.baseline + 1 + INLINE_REACH
}

struct Ctx<'a> {
    formulas: &'a [Formula],
}

impl Ctx<'_> {
    /// `blocks` as lines; with `spaced`, a blank line between two blocks.
    fn blocks(&self, blocks: &[Block], width: usize, spaced: bool) -> Vec<Line> {
        let mut out = Vec::new();
        for (i, block) in blocks.iter().enumerate() {
            if i > 0 && spaced {
                out.push(Line::default());
            }
            self.block(block, width, &mut out);
        }
        out
    }

    fn block(&self, block: &Block, width: usize, out: &mut Vec<Line>) {
        match block {
            Block::Para(spans) => out.extend(self.para(spans, Style::default(), width)),
            Block::Heading(level, spans) => {
                let style = match level {
                    1 => bold().add_modifier(Modifier::UNDERLINED),
                    _ => bold(),
                };
                out.extend(self.para(spans, style, width));
            }
            Block::Code(lines) => {
                for line in lines {
                    out.push(cut(&format!("{INDENT}{}", safe(line)), dim(), width));
                }
            }
            Block::List(start, items) => self.list(*start, items, width, out),
            Block::Quote(inner) => {
                let inner = self.blocks(inner, width.saturating_sub(GUTTER.width()).max(1), true);
                for line in inner {
                    let gutter = if line.0.is_empty() {
                        GUTTER.trim_end()
                    } else {
                        GUTTER
                    };
                    out.push(line.after(gutter, dim()));
                }
            }
            Block::Table(rows) => self.table(rows, width, out),
            Block::Rule => {
                let mut line = Line::default();
                line.push(&"─".repeat(width), dim());
                out.push(line);
            }
            Block::Display(i) => self.display(*i, width, out),
        }
    }

    /// A paragraph of `spans` in `style`, wrapped to `width`.
    fn para(&self, spans: &[Span], style: Style, width: usize) -> Vec<Line> {
        let items = self.items(spans, style, width, false);
        wrap(&items, width)
            .iter()
            .flat_map(|line| render(line).0)
            .collect()
    }

    /// `spans` as items. A table cell's display formula (`cells`) is its
    /// display box.
    fn items(&self, spans: &[Span], style: Style, width: usize, cells: bool) -> Vec<Item> {
        let mut out = Vec::new();
        for span in spans {
            match span {
                Span::Text(text, emphasis) => words(text, emphasized(style, *emphasis), &mut out),
                Span::Code(code) => words(code, style.patch(dim()), &mut out),
                Span::Formula(i) => self.inline(*i, style, width, &mut out),
                Span::Display(i) => match self.formulas.get(*i).and_then(|f| f.block.as_ref()) {
                    Some(block) if cells => out.push(Item::Box(block.clone())),
                    _ => self.inline(*i, style, width, &mut out),
                },
                Span::Break => out.push(Item::Break),
            }
        }
        out
    }

    /// Formula `i` in a line: its text-style box when that fits a line and
    /// the width, else its one-line form, else its source dimmed.
    fn inline(&self, i: usize, style: Style, width: usize, out: &mut Vec<Item>) {
        let Some(f) = self.formulas.get(i) else {
            return;
        };
        match (&f.text, &f.inline) {
            (Some(text), _) if fits_inline(text) && text.width() <= width => {
                out.push(Item::Box(text.clone()))
            }
            (_, Some(line)) => words(line, style, out),
            _ => words(&prepare(&f.src), style.patch(dim()), out),
        }
    }

    /// Display formula `i`: its display box, two columns in, when it fits;
    /// else its one-line form wrapped, else its source dimmed.
    fn display(&self, i: usize, width: usize, out: &mut Vec<Line>) {
        let Some(f) = self.formulas.get(i) else {
            return;
        };
        let inner = width.saturating_sub(INDENT.width()).max(1);
        let lines = match (&f.block, &f.inline) {
            (Some(block), _) if block.width() <= inner => render(&[Item::Box(block.clone())]).0,
            (_, Some(line)) => self.text_lines(line, Style::default(), inner),
            _ => self.text_lines(&prepare(&f.src), dim(), inner),
        };
        out.extend(lines.into_iter().map(|l| l.after(INDENT, Style::default())));
    }

    /// Plain text wrapped to `width`.
    fn text_lines(&self, text: &str, style: Style, width: usize) -> Vec<Line> {
        let mut items = Vec::new();
        words(text, style, &mut items);
        wrap(&items, width)
            .iter()
            .flat_map(|line| render(line).0)
            .collect()
    }

    /// A list: each item behind its marker (`•`, or its number), its other
    /// lines indented as far (a hanging indent); no blank lines.
    fn list(&self, start: Option<u64>, items: &[Vec<Block>], width: usize, out: &mut Vec<Line>) {
        let markers: Vec<String> = (0..items.len())
            .map(|k| match start {
                Some(n) => format!("{}.", n.saturating_add(k as u64)),
                None => "•".to_owned(),
            })
            .collect();
        let indent = markers.iter().map(|m| m.width()).max().unwrap_or(1) + 1;
        for (marker, item) in markers.iter().zip(items) {
            let lines = self.blocks(item, width.saturating_sub(indent).max(1), false);
            let first = format!("{marker:<indent$}");
            if lines.is_empty() {
                out.push(Line::default().after(first.trim_end(), Style::default()));
            }
            for (j, line) in lines.into_iter().enumerate() {
                let prefix = if j == 0 {
                    first.clone()
                } else {
                    " ".repeat(indent)
                };
                out.push(line.after(&prefix, Style::default()));
            }
        }
    }

    /// A table: aligned columns (the header bold, then a rule) when they
    /// fit `width`, else each row one paragraph of its cells.
    fn table(&self, rows: &[Vec<Vec<Span>>], width: usize, out: &mut Vec<Line>) {
        let style = |r: usize| if r == 0 { bold() } else { Style::default() };
        let cells: Vec<Vec<(Vec<Line>, usize)>> = rows
            .iter()
            .enumerate()
            .map(|(r, row)| {
                row.iter()
                    .map(|cell| render(&self.items(cell, style(r), usize::MAX, true)))
                    .collect()
            })
            .collect();
        let columns = cells.iter().map(Vec::len).max().unwrap_or(0);
        let widths: Vec<usize> = (0..columns)
            .map(|c| {
                let column = cells.iter().filter_map(|row| row.get(c));
                column
                    .flat_map(|(lines, _)| lines.iter().map(Line::width))
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        let total =
            widths.iter().sum::<usize>() + CELL_SEPARATOR.width() * columns.saturating_sub(1);
        if total > width {
            for (r, row) in rows.iter().enumerate() {
                let mut items = Vec::new();
                for (c, cell) in row.iter().enumerate() {
                    if c > 0 {
                        items.extend([Item::Space, Item::Text("│".to_owned(), dim()), Item::Space]);
                    }
                    items.extend(self.items(cell, style(r), width, false));
                }
                out.extend(wrap(&items, width).iter().flat_map(|line| render(line).0));
            }
            return;
        }
        for (r, row) in cells.iter().enumerate() {
            let above = row.iter().map(|(_, base)| *base).max().unwrap_or(0);
            let below = row
                .iter()
                .map(|(lines, base)| lines.len().saturating_sub(base + 1))
                .max()
                .unwrap_or(0);
            for k in 0..above + 1 + below {
                let mut line = Line::default();
                for (c, w) in widths.iter().enumerate() {
                    if c > 0 {
                        line.push(CELL_SEPARATOR, dim());
                    }
                    let cell = row.get(c).and_then(|(lines, base)| {
                        k.checked_sub(above - base).and_then(|i| lines.get(i))
                    });
                    let shown = cell.map_or(0, Line::width);
                    if let Some(cell) = cell {
                        for seg in &cell.0 {
                            line.push(&seg.text, seg.style);
                        }
                    }
                    line.push(&" ".repeat(w - shown), Style::default());
                }
                out.push(trim_end(line));
            }
            if r == 0 && cells.len() > 1 {
                let rule: Vec<String> = widths.iter().map(|w| "─".repeat(*w)).collect();
                let mut line = Line::default();
                line.push(&rule.join("─┼─"), dim());
                out.push(line);
            }
        }
    }
}

/// `items` broken into lines at most `width` wide: at spaces, at forced
/// breaks, and inside a word only when the word alone is wider than the
/// line (formula boxes are never cut).
fn wrap(items: &[Item], width: usize) -> Vec<Vec<Item>> {
    let mut lines = Vec::new();
    let mut line: Vec<Item> = Vec::new();
    let mut used = 0;
    let mut space = false;
    let mut i = 0;
    while i < items.len() {
        match &items[i] {
            Item::Space => {
                space = used > 0;
                i += 1;
                continue;
            }
            Item::Break => {
                lines.push(std::mem::take(&mut line));
                (used, space) = (0, false);
                i += 1;
                continue;
            }
            _ => {}
        }
        let end = items[i..]
            .iter()
            .position(|it| matches!(it, Item::Space | Item::Break))
            .map_or(items.len(), |p| i + p);
        let word = &items[i..end];
        let word_width: usize = word.iter().map(Item::width).sum();
        let gap = usize::from(space);
        if used > 0 && used + gap + word_width > width {
            lines.push(std::mem::take(&mut line));
            used = 0;
        } else if space {
            line.push(Item::Space);
            used += 1;
        }
        space = false;
        if word_width <= width - used {
            line.extend(word.iter().cloned());
            used += word_width;
        } else {
            // A word wider than a line: cut between its graphemes.
            for item in word {
                match item {
                    Item::Text(text, style) => {
                        let mut piece = String::new();
                        for g in text.graphemes(true) {
                            if used + g.width() > width && used > 0 {
                                if !piece.is_empty() {
                                    line.push(Item::Text(std::mem::take(&mut piece), *style));
                                }
                                lines.push(std::mem::take(&mut line));
                                used = 0;
                            }
                            piece.push_str(g);
                            used += g.width();
                        }
                        if !piece.is_empty() {
                            line.push(Item::Text(piece, *style));
                        }
                    }
                    other => {
                        if used > 0 && used + other.width() > width {
                            lines.push(std::mem::take(&mut line));
                            used = 0;
                        }
                        used += other.width();
                        line.push(other.clone());
                    }
                }
            }
        }
        i = end;
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

/// One wrapped line of items as rows, and which row is the baseline: as
/// tall as its tallest box, text on the baseline.
fn render(items: &[Item]) -> (Vec<Line>, usize) {
    let above = items
        .iter()
        .map(|it| match it {
            Item::Box(laid) => laid.baseline,
            _ => 0,
        })
        .max()
        .unwrap_or(0);
    let below = items
        .iter()
        .map(|it| match it {
            Item::Box(laid) => laid.rows.len() - 1 - laid.baseline,
            _ => 0,
        })
        .max()
        .unwrap_or(0);
    let rows = (0..above + 1 + below)
        .map(|row| {
            let mut line = Line::default();
            for item in items {
                let width = item.width();
                match item {
                    Item::Text(text, style) if row == above => line.push(text, *style),
                    Item::Box(laid) => {
                        let text = (row + laid.baseline)
                            .checked_sub(above)
                            .and_then(|i| laid.rows.get(i))
                            .map_or(String::new(), |r| safe(r));
                        let pad = width.saturating_sub(text.width());
                        line.push(&text, Style::default());
                        line.push(&" ".repeat(pad), Style::default());
                    }
                    _ => line.push(&" ".repeat(width), Style::default()),
                }
            }
            trim_end(line)
        })
        .collect();
    (rows, above)
}

/// `line` without trailing blanks.
fn trim_end(mut line: Line) -> Line {
    while let Some(last) = line.0.last_mut() {
        let kept = last.text.trim_end().len();
        if kept > 0 {
            last.text.truncate(kept);
            break;
        }
        line.0.pop();
    }
    line
}

/// `text` in `style`, cut to `width` columns.
fn cut(text: &str, style: Style, width: usize) -> Line {
    let mut kept = String::new();
    let mut used = 0;
    for g in text.graphemes(true) {
        if used + g.width() > width {
            break;
        }
        used += g.width();
        kept.push_str(g);
    }
    let mut line = Line::default();
    line.push(kept.trim_end(), style);
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::latex::Rendered;
    use crate::tui::reader::doc::{parse, prepare as prepare_doc};

    fn laid(rows: &[&str], baseline: usize) -> Laid {
        Laid {
            rows: rows.iter().map(|r| r.to_string()).collect(),
            baseline,
        }
    }

    /// A formula with these renderings.
    fn formula(
        src: &str,
        inline: Option<&str>,
        text: Option<Laid>,
        block: Option<Laid>,
    ) -> Formula {
        Formula::new(
            src,
            Rendered {
                inline: inline.map(str::to_owned),
                block,
                text,
            },
        )
    }

    /// `markdown` laid out at `width`, with `formulas` for its tags, as text.
    fn shown(markdown: &str, formulas: &[Formula], width: usize) -> Vec<String> {
        let doc = parse(&prepare_doc(markdown));
        layout(&doc, formulas, width)
            .iter()
            .map(Line::text)
            .collect()
    }

    #[test]
    fn paragraphs_wrap_at_spaces_and_cut_long_words() {
        assert_eq!(
            shown("the quick brown fox jumps over the dog", &[], 15),
            ["the quick brown", "fox jumps over", "the dog"]
        );
        assert_eq!(shown("abcdefghij xy", &[], 4), ["abcd", "efgh", "ij", "xy"]);
        assert_eq!(shown("one\ntwo  three", &[], 40), ["one two three"]);
        assert_eq!(shown("hard  \nbreak", &[], 40), ["hard", "break"]);
        assert_eq!(shown("a\n\nb", &[], 40), ["a", "", "b"]);
    }

    #[test]
    fn inline_formulas_are_line_boxes_on_the_baseline() {
        let s = formula("S_{i,f}", None, Some(laid(&["S", " i,f"], 0)), None);
        let e = formula("e^{i\\pi}", None, Some(laid(&[" iπ", "e"], 1)), None);
        let text = "so <latex>S_{i,f}</latex> and <latex>e^{i\\pi}</latex> end";
        assert_eq!(
            shown(text, &[s.clone(), e], 40),
            ["             iπ", "so S    and e   end", "    i,f"]
        );
        // A line without boxes is one row; the next line is its own box.
        assert_eq!(
            shown("aaaa bbbb <latex>S_{i,f}</latex>", &[s], 10),
            ["aaaa bbbb", "S", " i,f"]
        );
    }

    #[test]
    fn tall_or_wide_inline_formulas_fall_back_to_one_line() {
        // 33 rows: a 32-deep nested script.
        let rows: Vec<String> = (0..33).map(|i| format!("{}x", " ".repeat(i))).collect();
        let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
        let deep = formula("deep", Some("x_(…)"), Some(laid(&rows, 0)), None);
        let lines = shown("a <latex>deep</latex> b", &[deep], 40);
        assert_eq!(lines, ["a x_(…) b"]);
        // Wider than the view.
        let wide = formula("w", Some("w w w"), Some(laid(&["wwwwwwwwww"], 0)), None);
        assert_eq!(shown("<latex>w</latex>x", &[wide], 8), ["w w wx"]);
        // Not rendered at all: the source.
        assert_eq!(
            shown("a <latex>\\oops</latex>", &[Formula::raw("\\oops")], 40),
            ["a \\oops"]
        );
    }

    #[test]
    fn display_formulas_are_indented_blocks_with_a_fallback() {
        let quad = formula(
            "q",
            Some("x = (-b ± √(b² - 4ac))/(2a)"),
            None,
            Some(laid(
                &[
                    "    -b ± √(b² - 4ac)",
                    "x = ────────────────",
                    "           2a",
                ],
                1,
            )),
        );
        let text = "Roots:\n\n<latex>q</latex>\n\ndone";
        assert_eq!(
            shown(text, &[quad.clone()], 40),
            [
                "Roots:",
                "",
                "      -b ± √(b² - 4ac)",
                "  x = ────────────────",
                "             2a",
                "",
                "done"
            ]
        );
        // Too wide for the view: the one-line form, wrapped, indented.
        assert_eq!(
            shown("<latex>q</latex>", &[quad], 18),
            ["  x = (-b ± √(b² -", "  4ac))/(2a)"]
        );
    }

    #[test]
    fn lists_quotes_headings_and_rules() {
        let text = "# Title\n\n- one\n- two words here\n  1. sub\n\n> quoted\n>\n> more\n\n---";
        assert_eq!(
            shown(text, &[], 12),
            [
                "Title",
                "",
                "• one",
                "• two words",
                "  here",
                "  1. sub",
                "",
                "│ quoted",
                "│",
                "│ more",
                "",
                "────────────",
            ]
        );
        let doc = parse(&prepare_doc("# Big\n\n## Small"));
        let lines = layout(&doc, &[], 20);
        assert!(lines[0].0[0]
            .style
            .add_modifier
            .contains(Modifier::UNDERLINED));
        assert!(lines[2].0[0].style.add_modifier.contains(Modifier::BOLD));
        assert!(!lines[2].0[0]
            .style
            .add_modifier
            .contains(Modifier::UNDERLINED));
        let ordered = shown("9. a\n10. b", &[], 20);
        assert_eq!(ordered, ["9.  a", "10. b"]);
    }

    #[test]
    fn code_blocks_are_indented_dim_and_cut() {
        let doc = parse(&prepare_doc("```\nlet x = 1;\n\tlong line here\n```"));
        let lines = layout(&doc, &[], 12);
        let text: Vec<String> = lines.iter().map(Line::text).collect();
        assert_eq!(text, ["  let x = 1;", "      long l"]);
        assert!(lines[0].0[0].style.add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn tables_align_when_they_fit_and_flow_when_not() {
        let e = formula(
            "x",
            Some("x"),
            Some(laid(&["x"], 0)),
            Some(laid(&["a", "─", "b"], 1)),
        );
        let text = "| name | value |\n|---|---|\n| one | 1 |\n| frac | <latex>x</latex> |";
        assert_eq!(
            shown(text, &[e.clone()], 40),
            [
                "name │ value",
                "─────┼──────",
                "one  │ 1",
                "     │ a",
                "frac │ ─",
                "     │ b",
            ]
        );
        assert_eq!(
            shown(text, &[e], 10),
            ["name │", "value", "one │ 1", "frac │ x"]
        );
    }

    #[test]
    fn wide_clusters_become_one_replacement_character() {
        // A cluster wider than two columns (two leading jamo and a vowel)
        // does not take the line with it.
        let wide = "\u{1100}\u{1100}\u{1161}";
        assert_eq!(wide.graphemes(true).count(), 1);
        assert!(wide.width() > 2);
        let lines = shown(&format!("a {wide} b"), &[], 20);
        assert_eq!(lines, ["a \u{fffd} b"]);
    }
}
