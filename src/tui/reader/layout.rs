//! A [`Doc`] wrapped to the reader view's width: styled lines. Pure; the
//! view caches the result for its width.
//!
//! - Paragraphs wrap at spaces; a word wider than the view is cut.
//! - Code blocks (diagram fences included) are never wrapped (cut at the
//!   edge); tables are aligned columns when they fit, else one
//!   `a │ b │ c` paragraph per row.

use ratatui::style::{Modifier, Style};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::doc::{Block, Doc, Emphasis, Span};

/// The indentation of a code block and a quote's text.
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

/// `doc` as lines at most `width` columns wide (code blocks and over-deep
/// indentation aside, which the drawing cuts).
pub fn layout(doc: &Doc, width: usize) -> Vec<Line> {
    blocks(&doc.blocks, width.max(1), true)
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
    /// A space a line may break at.
    Space,
    /// A forced line break.
    Break,
}

impl Item {
    fn width(&self) -> usize {
        match self {
            Item::Text(t, _) => t.width(),
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

/// A code block's lines, indented, dim and cut at `width`.
fn code(lines: &[String], width: usize, out: &mut Vec<Line>) {
    for line in lines {
        out.push(cut(&format!("{INDENT}{}", safe(line)), dim(), width));
    }
}

/// `blocks` as lines; with `spaced`, a blank line between two blocks.
fn blocks(blocks: &[Block], width: usize, spaced: bool) -> Vec<Line> {
    let mut out = Vec::new();
    for (i, b) in blocks.iter().enumerate() {
        if i > 0 && spaced {
            out.push(Line::default());
        }
        block(b, width, &mut out);
    }
    out
}

fn block(block: &Block, width: usize, out: &mut Vec<Line>) {
    match block {
        Block::Para(spans) => out.extend(para(spans, Style::default(), width)),
        Block::Heading(level, spans) => {
            let style = match level {
                1 => bold().add_modifier(Modifier::UNDERLINED),
                _ => bold(),
            };
            out.extend(para(spans, style, width));
        }
        Block::Code(lines) => code(lines, width, out),
        Block::List(start, items) => list(*start, items, width, out),
        Block::Quote(inner) => {
            let inner = blocks(inner, width.saturating_sub(GUTTER.width()).max(1), true);
            for line in inner {
                let gutter = if line.0.is_empty() {
                    GUTTER.trim_end()
                } else {
                    GUTTER
                };
                out.push(line.after(gutter, dim()));
            }
        }
        Block::Table(rows) => table(rows, width, out),
        Block::Rule => {
            let mut line = Line::default();
            line.push(&"─".repeat(width), dim());
            out.push(line);
        }
    }
}

/// A paragraph of `spans` in `style`, wrapped to `width`.
fn para(spans: &[Span], style: Style, width: usize) -> Vec<Line> {
    wrap(&items(spans, style), width)
        .iter()
        .map(|line| render(line))
        .collect()
}

/// `spans` as items; inline code (a formula's tag among it) dimmed.
fn items(spans: &[Span], style: Style) -> Vec<Item> {
    let mut out = Vec::new();
    for span in spans {
        match span {
            Span::Text(text, emphasis) => words(text, emphasized(style, *emphasis), &mut out),
            Span::Code(code) => words(code, style.patch(dim()), &mut out),
            Span::Break => out.push(Item::Break),
        }
    }
    out
}

/// A list: each item behind its marker (`•`, or its number), its other
/// lines indented as far (a hanging indent); no blank lines.
fn list(start: Option<u64>, items: &[Vec<Block>], width: usize, out: &mut Vec<Line>) {
    let markers: Vec<String> = (0..items.len())
        .map(|k| match start {
            Some(n) => format!("{}.", n.saturating_add(k as u64)),
            None => "•".to_owned(),
        })
        .collect();
    let indent = markers.iter().map(|m| m.width()).max().unwrap_or(1) + 1;
    for (marker, item) in markers.iter().zip(items) {
        let lines = blocks(item, width.saturating_sub(indent).max(1), false);
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

/// A table: aligned columns (the header bold, then a rule) when they fit
/// `width`, else each row one paragraph of its cells.
fn table(rows: &[Vec<Vec<Span>>], width: usize, out: &mut Vec<Line>) {
    let style = |r: usize| if r == 0 { bold() } else { Style::default() };
    let cells: Vec<Vec<Line>> = rows
        .iter()
        .enumerate()
        .map(|(r, row)| {
            row.iter()
                .map(|cell| render(&items(cell, style(r))))
                .collect()
        })
        .collect();
    let columns = cells.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|c| {
            let column = cells.iter().filter_map(|row| row.get(c));
            column.map(Line::width).max().unwrap_or(0)
        })
        .collect();
    let total = widths.iter().sum::<usize>() + CELL_SEPARATOR.width() * columns.saturating_sub(1);
    if total > width {
        for (r, row) in rows.iter().enumerate() {
            let mut line = Vec::new();
            for (c, cell) in row.iter().enumerate() {
                if c > 0 {
                    line.extend([Item::Space, Item::Text("│".to_owned(), dim()), Item::Space]);
                }
                line.extend(items(cell, style(r)));
            }
            out.extend(wrap(&line, width).iter().map(|l| render(l)));
        }
        return;
    }
    for (r, row) in cells.iter().enumerate() {
        let mut line = Line::default();
        for (c, w) in widths.iter().enumerate() {
            if c > 0 {
                line.push(CELL_SEPARATOR, dim());
            }
            let cell = row.get(c);
            let shown = cell.map_or(0, Line::width);
            if let Some(cell) = cell {
                for seg in &cell.0 {
                    line.push(&seg.text, seg.style);
                }
            }
            line.push(&" ".repeat(w - shown), Style::default());
        }
        out.push(trim_end(line));
        if r == 0 && cells.len() > 1 {
            let rule: Vec<String> = widths.iter().map(|w| "─".repeat(*w)).collect();
            let mut line = Line::default();
            line.push(&rule.join("─┼─"), dim());
            out.push(line);
        }
    }
}

/// `items` broken into lines at most `width` wide: at spaces, at forced
/// breaks, and inside a word only when the word alone is wider than the
/// line.
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
            Item::Text(..) => {}
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
                let Item::Text(text, style) = item else {
                    continue;
                };
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
        }
        i = end;
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

/// One wrapped line of items as a line.
fn render(items: &[Item]) -> Line {
    let mut line = Line::default();
    for item in items {
        match item {
            Item::Text(text, style) => line.push(text, *style),
            Item::Space => line.push(" ", Style::default()),
            Item::Break => {}
        }
    }
    trim_end(line)
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
    use crate::tui::reader::doc::parse;

    /// `markdown` laid out at `width`, as text.
    fn shown(markdown: &str, width: usize) -> Vec<String> {
        layout(&parse(markdown), width)
            .iter()
            .map(Line::text)
            .collect()
    }

    #[test]
    fn paragraphs_wrap_at_spaces_and_cut_long_words() {
        assert_eq!(
            shown("the quick brown fox jumps over the dog", 15),
            ["the quick brown", "fox jumps over", "the dog"]
        );
        assert_eq!(shown("abcdefghij xy", 4), ["abcd", "efgh", "ij", "xy"]);
        assert_eq!(shown("one\ntwo  three", 40), ["one two three"]);
        assert_eq!(shown("hard  \nbreak", 40), ["hard", "break"]);
        assert_eq!(shown("a\n\nb", 40), ["a", "", "b"]);
    }

    /// Formulas are their tags as claude wrote them, dimmed like inline
    /// code, in the text flow; one alone is still just a paragraph.
    #[test]
    fn formulas_are_their_source_text() {
        let text = "so <latex>S_{i,f}</latex> and <latex>e^{i\\pi}</latex> end";
        assert_eq!(
            shown(text, 80),
            ["so <latex>S_{i,f}</latex> and <latex>e^{i\\pi}</latex> end"]
        );
        let lines = layout(&parse(text), 80);
        let tag = lines[0].0.iter().find(|s| s.text.contains("S_{i,f}"));
        assert!(tag.is_some_and(|s| s.style == dim()), "{lines:?}");
        let display = "Roots:\n\n<latex>x = \\frac{-b \\pm \\sqrt{b^2-4ac}}{2a}</latex>\n\ndone";
        assert_eq!(
            shown(display, 80),
            [
                "Roots:",
                "",
                "<latex>x = \\frac{-b \\pm \\sqrt{b^2-4ac}}{2a}</latex>",
                "",
                "done"
            ]
        );
        // Wrapped at its spaces like any text.
        assert_eq!(
            shown("<latex>a + b = c</latex>", 12),
            ["<latex>a + b", "= c</latex>"]
        );
    }

    #[test]
    fn lists_quotes_headings_and_rules() {
        let text = "# Title\n\n- one\n- two words here\n  1. sub\n\n> quoted\n>\n> more\n\n---";
        assert_eq!(
            shown(text, 12),
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
        let lines = layout(&parse("# Big\n\n## Small"), 20);
        assert!(lines[0].0[0]
            .style
            .add_modifier
            .contains(Modifier::UNDERLINED));
        assert!(lines[2].0[0].style.add_modifier.contains(Modifier::BOLD));
        assert!(!lines[2].0[0]
            .style
            .add_modifier
            .contains(Modifier::UNDERLINED));
        let ordered = shown("9. a\n10. b", 20);
        assert_eq!(ordered, ["9.  a", "10. b"]);
    }

    #[test]
    fn code_blocks_are_indented_dim_and_cut() {
        let lines = layout(&parse("```\nlet x = 1;\n\tlong line here\n```"), 12);
        let text: Vec<String> = lines.iter().map(Line::text).collect();
        assert_eq!(text, ["  let x = 1;", "      long l"]);
        assert!(lines[0].0[0].style.add_modifier.contains(Modifier::DIM));
    }

    /// A mermaid fence is a code block: its source, as written.
    #[test]
    fn diagrams_are_their_source() {
        let text = "```mermaid\nflowchart LR\n  a --> b\n```";
        assert_eq!(shown(text, 40), ["  flowchart LR", "    a --> b"]);
    }

    #[test]
    fn tables_align_when_they_fit_and_flow_when_not() {
        let text = "| name | value |\n|---|---|\n| one | 1 |\n| frac | <latex>x</latex> |";
        assert_eq!(
            shown(text, 40),
            [
                "name │ value",
                "─────┼─────────────────",
                "one  │ 1",
                "frac │ <latex>x</latex>",
            ]
        );
        assert_eq!(
            shown(text, 10),
            [
                "name │",
                "value",
                "one │ 1",
                "frac │",
                "<latex>x</",
                "latex>"
            ]
        );
    }

    #[test]
    fn wide_clusters_become_one_replacement_character() {
        // A cluster wider than two columns (two leading jamo and a vowel)
        // does not take the line with it.
        let wide = "\u{1100}\u{1100}\u{1161}";
        assert_eq!(wide.graphemes(true).count(), 1);
        assert!(wide.width() > 2);
        let lines = shown(&format!("a {wide} b"), 20);
        assert_eq!(lines, ["a \u{fffd} b"]);
    }
}
