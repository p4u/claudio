//! One of claude's answers as the reader view's document: markdown blocks
//! (pulldown-cmark). Width-independent; `layout.rs` wraps it. Formulas and
//! diagrams are shown as their source: they render in the web UI only.
//!
//! Pure, and run off the UI thread (`tui/mod.rs`). The text is
//! attacker-influenced, so:
//! - [`prepare`] sanitizes it again, then replaces each `<latex>` tag (by
//!   `latex_tags`'s rules) with one private-use placeholder, so markdown
//!   leaves the formula alone (no `\\` escapes taken, no emphasis);
//!   sanitizing removed every private-use character, so the text cannot
//!   forge one.
//! - [`parse`] turns placeholders back into their tags, as inline code,
//!   while it reads markdown (iterating chars, never graphemes), and
//!   sanitizes each text run again once its placeholders are gone.
//! - Lists and quotes nest at most [`MAX_NEST`] deep; deeper content is
//!   flattened into the deepest one, so layout recursion is bounded.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

use crate::claude::latex::{latex_tags, MAX_FORMULAS};
use crate::text::{is_combining, sanitize};

/// The first placeholder: tag `i` of an answer is `PLACEHOLDER + i`.
const PLACEHOLDER: u32 = 0xE000;
/// How deep lists and block quotes nest.
pub const MAX_NEST: usize = 8;
/// What a tab is in a code block (elsewhere it is one space).
const TAB: &str = "    ";

/// An answer's text ready for [`parse`]: `<latex>` tags replaced by
/// placeholders, and the tags as written.
#[derive(Debug, Clone, PartialEq)]
struct Prepared {
    text: String,
    tags: Vec<String>,
}

/// An answer as markdown blocks.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Doc {
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    /// A paragraph, or the text of a tight list item.
    Para(Vec<Span>),
    /// A heading of level 1 to 6.
    Heading(u8, Vec<Span>),
    /// A code block's lines, tabs expanded.
    Code(Vec<String>),
    /// A list, numbered from its start or bulleted; each item its blocks.
    List(Option<u64>, Vec<Vec<Block>>),
    Quote(Vec<Block>),
    /// A table's rows, the header first; each row its cells.
    Table(Vec<Vec<Vec<Span>>>),
    Rule,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Span {
    Text(String, Emphasis),
    /// Inline code, or a `<latex>` tag as written.
    Code(String),
    /// A hard line break.
    Break,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Emphasis {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
}

/// `text` (an answer) [`sanitize`]d, with each of its first
/// [`MAX_FORMULAS`] `<latex>` tags replaced by a placeholder, and a
/// combining mark right after a tag dropped (it would join the
/// placeholder). Later tags stay as they are written.
fn prepare(text: &str) -> Prepared {
    let clean = sanitize(text);
    let mut out = String::with_capacity(clean.len());
    let mut tags = Vec::new();
    let mut copied = 0;
    for (range, _) in latex_tags(&clean).into_iter().take(MAX_FORMULAS) {
        out.push_str(&clean[copied..range.start]);
        out.push(placeholder(tags.len()));
        tags.push(clean[range.clone()].to_owned());
        let rest = &clean[range.end..];
        copied = range.end + (rest.len() - rest.trim_start_matches(is_combining).len());
    }
    out.push_str(&clean[copied..]);
    Prepared { text: out, tags }
}

/// The placeholder of tag `i` (`i` < [`MAX_FORMULAS`]).
fn placeholder(i: usize) -> char {
    char::from_u32(PLACEHOLDER + i as u32).unwrap_or(char::REPLACEMENT_CHARACTER)
}

/// The tag a placeholder stands for, if `c` is one of `count`.
fn tag_of(c: char, count: usize) -> Option<usize> {
    let i = (c as u32).checked_sub(PLACEHOLDER)? as usize;
    (i < count).then_some(i)
}

/// `text` (an answer) as a [`Doc`]. pulldown-cmark reads it with tables,
/// strikethrough and task lists only (no math, footnotes or smart
/// punctuation: `$` is text).
pub fn parse(text: &str) -> Doc {
    let prepared = prepare(text);
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut b = Builder::new(&prepared.tags);
    for event in Parser::new_ext(&prepared.text, options) {
        b.event(event);
    }
    b.flush();
    Doc { blocks: b.root }
}

/// A container being read.
enum Frame {
    Quote(Vec<Block>),
    List(Option<u64>, Vec<Vec<Block>>),
    Item(Vec<Block>),
}

/// The state of [`parse`]: the open containers and the inline content
/// being gathered.
struct Builder<'a> {
    /// The `<latex>` tags the placeholders stand for.
    tags: &'a [String],
    root: Vec<Block>,
    frames: Vec<Frame>,
    /// Per open container start: whether it got a frame (`false`: it was
    /// nested too deep, its content goes to the frame around it).
    open: Vec<bool>,
    /// How many open containers got no frame.
    flattened: usize,
    spans: Vec<Span>,
    emphasis: Emphasis,
    /// Nesting counts behind `emphasis`.
    bold: u32,
    italic: u32,
    strike: u32,
    heading: Option<u8>,
    code: Option<String>,
    /// The table being read: its rows, and the row being read.
    table: Option<TableRows>,
}

/// A table being read: the finished rows (cells of spans) and the row in progress.
type TableRows = (Vec<Vec<Vec<Span>>>, Vec<Vec<Span>>);

impl<'a> Builder<'a> {
    fn new(tags: &'a [String]) -> Builder<'a> {
        Builder {
            tags,
            root: Vec::new(),
            frames: Vec::new(),
            open: Vec::new(),
            flattened: 0,
            spans: Vec::new(),
            emphasis: Emphasis::default(),
            bold: 0,
            italic: 0,
            strike: 0,
            heading: None,
            code: None,
            table: None,
        }
    }

    fn event(&mut self, event: Event) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => match &mut self.code {
                Some(code) => code.push_str(&text),
                None => self.text(&text),
            },
            Event::Code(code) => {
                let code = self.code_text(&code, " ");
                self.spans.push(Span::Code(code));
            }
            Event::Html(html) | Event::InlineHtml(html) => self.text(&html),
            Event::InlineMath(math) | Event::DisplayMath(math) => self.text(&math),
            Event::FootnoteReference(name) => self.text(&name),
            Event::SoftBreak => self.text(" "),
            Event::HardBreak => self.spans.push(Span::Break),
            Event::Rule => {
                self.flush();
                self.push(Block::Rule);
            }
            Event::TaskListMarker(done) => self.text(if done { "[x] " } else { "[ ] " }),
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph | Tag::HtmlBlock => self.flush(),
            Tag::Heading { level, .. } => {
                self.flush();
                self.heading = Some(level as u8);
            }
            Tag::CodeBlock(_) => {
                self.flush();
                self.code = Some(String::new());
            }
            Tag::BlockQuote(_) => self.open_frame(Frame::Quote(Vec::new())),
            Tag::List(start) => self.open_frame(Frame::List(start, Vec::new())),
            Tag::Item => self.open_frame(Frame::Item(Vec::new())),
            Tag::Table(_) => {
                self.flush();
                self.table = Some((Vec::new(), Vec::new()));
            }
            Tag::TableHead | Tag::TableRow | Tag::TableCell => self.spans.clear(),
            Tag::Emphasis => self.italic += 1,
            Tag::Strong => self.bold += 1,
            Tag::Strikethrough => self.strike += 1,
            _ => {}
        }
        self.restyle();
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::HtmlBlock => self.flush(),
            TagEnd::Heading(_) => {
                let level = self.heading.take().unwrap_or(1);
                let spans = trimmed(std::mem::take(&mut self.spans));
                if !spans.is_empty() {
                    self.push(Block::Heading(level, spans));
                }
            }
            TagEnd::CodeBlock => {
                let code = self.code.take().unwrap_or_default();
                let code = self.code_text(code.trim_end_matches('\n'), TAB);
                let lines = code.split('\n').map(str::to_owned).collect();
                self.push(Block::Code(lines));
            }
            TagEnd::BlockQuote(_) | TagEnd::List(_) | TagEnd::Item => self.close_frame(),
            TagEnd::TableCell => {
                let cell = trimmed(std::mem::take(&mut self.spans));
                if let Some((_, row)) = &mut self.table {
                    row.push(cell);
                }
            }
            TagEnd::TableHead | TagEnd::TableRow => {
                if let Some((rows, row)) = &mut self.table {
                    rows.push(std::mem::take(row));
                }
            }
            TagEnd::Table => {
                if let Some((rows, _)) = self.table.take() {
                    self.push(Block::Table(rows));
                }
            }
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            _ => {}
        }
        self.restyle();
    }

    fn restyle(&mut self) {
        self.emphasis = Emphasis {
            bold: self.bold > 0,
            italic: self.italic > 0,
            strike: self.strike > 0,
        };
    }

    /// Open a container: a frame of its own, unless lists and quotes are
    /// already [`MAX_NEST`] deep (or an item outside a list frame).
    fn open_frame(&mut self, frame: Frame) {
        self.flush();
        let depth = self
            .frames
            .iter()
            .filter(|f| !matches!(f, Frame::Item(_)))
            .count();
        let fits = match frame {
            Frame::Item(_) => matches!(self.frames.last(), Some(Frame::List(..))),
            _ => depth < MAX_NEST,
        };
        let framed = self.flattened == 0 && fits;
        if framed {
            self.frames.push(frame);
        } else {
            self.flattened += 1;
        }
        self.open.push(framed);
    }

    /// Close the container opened last: its block goes to the one around
    /// it (an item to its list).
    fn close_frame(&mut self) {
        self.flush();
        if !self.open.pop().unwrap_or(false) {
            self.flattened = self.flattened.saturating_sub(1);
            return;
        }
        match self.frames.pop() {
            Some(Frame::Quote(blocks)) => self.push(Block::Quote(blocks)),
            Some(Frame::List(start, items)) => self.push(Block::List(start, items)),
            Some(Frame::Item(blocks)) => match self.frames.last_mut() {
                Some(Frame::List(_, items)) => items.push(blocks),
                _ => self.root.extend(blocks),
            },
            None => {}
        }
    }

    /// Add a block to the innermost container.
    fn push(&mut self, block: Block) {
        match self.frames.last_mut() {
            Some(Frame::Quote(blocks) | Frame::Item(blocks)) => blocks.push(block),
            // Not inside an item: cannot happen with pulldown-cmark.
            Some(Frame::List(_, items)) => items.push(vec![block]),
            None => self.root.push(block),
        }
    }

    /// End the inline content gathered so far as a paragraph, unless a
    /// heading or table cell is gathering it.
    fn flush(&mut self) {
        if self.heading.is_some() || self.table.is_some() {
            return;
        }
        let spans = trimmed(std::mem::take(&mut self.spans));
        if !spans.is_empty() {
            self.push(Block::Para(spans));
        }
    }

    /// Inline text: its placeholders become their tags as inline code,
    /// the runs between them are sanitized (tabs as one space) and joined
    /// to the text before them when they share its emphasis.
    fn text(&mut self, text: &str) {
        let mut run = String::new();
        for c in text.chars() {
            match tag_of(c, self.tags.len()) {
                Some(i) => {
                    self.push_run(&mut run);
                    self.spans.push(Span::Code(self.tags[i].clone()));
                }
                None if c == '\t' => run.push(' '),
                None => run.push(c),
            }
        }
        self.push_run(&mut run);
    }

    fn push_run(&mut self, run: &mut String) {
        let text = sanitize(&std::mem::take(run));
        if text.is_empty() {
            return;
        }
        if let Some(Span::Text(last, emphasis)) = self.spans.last_mut() {
            if *emphasis == self.emphasis {
                last.push_str(&text);
                return;
            }
        }
        self.spans.push(Span::Text(text, self.emphasis));
    }

    /// Code as shown: each placeholder its tag's text again, tabs as `tab`,
    /// sanitized.
    fn code_text(&self, code: &str, tab: &str) -> String {
        let mut out = String::with_capacity(code.len());
        for c in code.chars() {
            match tag_of(c, self.tags.len()) {
                Some(i) => out.push_str(&self.tags[i]),
                None if c == '\t' => out.push_str(tab),
                None => out.push(c),
            }
        }
        sanitize(&out).replace('\t', tab)
    }
}

/// `spans` without the blank text and breaks at either end.
fn trimmed(mut spans: Vec<Span>) -> Vec<Span> {
    let blank = |s: &Span| match s {
        Span::Text(t, _) => t.trim().is_empty(),
        Span::Break => true,
        _ => false,
    };
    while spans.last().is_some_and(blank) {
        spans.pop();
    }
    let lead = spans.iter().take_while(|s| blank(s)).count();
    spans.drain(..lead);
    if let Some(Span::Text(t, _)) = spans.first_mut() {
        *t = t.trim_start().to_owned();
    }
    if let Some(Span::Text(t, _)) = spans.last_mut() {
        *t = t.trim_end().to_owned();
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(text: &str) -> Doc {
        parse(text)
    }

    fn plain(s: &str) -> Span {
        Span::Text(s.into(), Emphasis::default())
    }

    fn code(s: &str) -> Span {
        Span::Code(s.into())
    }

    #[test]
    fn tags_become_placeholders_after_sanitizing() {
        let p = prepare("a <latex>x^2</latex>\u{301}b \u{e000}c `<latex>y</latex>`");
        assert_eq!(p.text, "a \u{e000}b c `<latex>y</latex>`");
        assert_eq!(p.tags, ["<latex>x^2</latex>"]);
        // Past MAX_FORMULAS, tags stay text.
        let many = "<latex>x</latex> ".repeat(MAX_FORMULAS + 2);
        let p = prepare(&many);
        assert_eq!(p.tags.len(), MAX_FORMULAS);
        assert!(p.text.ends_with("<latex>x</latex> <latex>x</latex> "));
    }

    #[test]
    fn markdown_becomes_blocks() {
        let d = doc("# Title\n\nSome *em* and **bold** `code`.\n\n```\nfn\tx\n```\n\n---\n");
        let em = Emphasis {
            italic: true,
            ..Emphasis::default()
        };
        let bold = Emphasis {
            bold: true,
            ..Emphasis::default()
        };
        assert_eq!(
            d.blocks,
            [
                Block::Heading(1, vec![plain("Title")]),
                Block::Para(vec![
                    plain("Some "),
                    Span::Text("em".into(), em),
                    plain(" and "),
                    Span::Text("bold".into(), bold),
                    plain(" "),
                    code("code"),
                    plain("."),
                ]),
                Block::Code(vec!["fn    x".into()]),
                Block::Rule,
            ]
        );
    }

    #[test]
    fn lists_quotes_and_tables() {
        let d = doc("- one\n- two\n  1. deep\n\n> quoted\n\n| a | b |\n|---|---|\n| 1 | <latex>x</latex> |\n");
        assert_eq!(
            d.blocks,
            [
                Block::List(
                    None,
                    vec![
                        vec![Block::Para(vec![plain("one")])],
                        vec![
                            Block::Para(vec![plain("two")]),
                            Block::List(Some(1), vec![vec![Block::Para(vec![plain("deep")])]]),
                        ],
                    ]
                ),
                Block::Quote(vec![Block::Para(vec![plain("quoted")])]),
                Block::Table(vec![
                    vec![vec![plain("a")], vec![plain("b")]],
                    vec![vec![plain("1")], vec![code("<latex>x</latex>")]],
                ]),
            ]
        );
    }

    /// A formula is its tag as claude wrote it, untouched by markdown
    /// (escapes, emphasis), alone or in the text.
    #[test]
    fn formulas_are_their_tags_as_written() {
        let d = doc("Text <latex>a_1 *b* \\\\ c\\{</latex> here.\n\n<latex>b</latex>\n\n- item <latex>d</latex>\n");
        assert_eq!(
            d.blocks,
            [
                Block::Para(vec![
                    plain("Text "),
                    code("<latex>a_1 *b* \\\\ c\\{</latex>"),
                    plain(" here."),
                ]),
                Block::Para(vec![code("<latex>b</latex>")]),
                Block::List(
                    None,
                    vec![vec![Block::Para(vec![
                        plain("item "),
                        code("<latex>d</latex>")
                    ])]]
                ),
            ]
        );
    }

    /// A mermaid fence is a code block like any other.
    #[test]
    fn diagram_fences_are_code() {
        let d = doc("```mermaid extra\nflowchart LR\n\ta --> b\n```\n");
        assert_eq!(
            d.blocks,
            [Block::Code(vec![
                "flowchart LR".into(),
                "    a --> b".into()
            ])]
        );
    }

    #[test]
    fn a_placeholder_in_code_is_its_tag_again() {
        // Markdown takes this backtick run as code, `latex_tags` does not
        // (its code spans end at the line).
        let p = prepare("``a\n<latex>x</latex> b``");
        assert_eq!(p.tags.len(), 1);
        assert_eq!(
            parse("``a\n<latex>x</latex> b``").blocks,
            [Block::Para(vec![code("a <latex>x</latex> b")])]
        );
        // `$` is text: no math extension.
        assert_eq!(doc("$x$").blocks, [Block::Para(vec![plain("$x$")])]);
    }

    #[test]
    fn nesting_is_flattened_past_the_cap() {
        let quotes = format!("{} deep", ">".repeat(MAX_NEST + 5));
        let mut d = &doc(&quotes).blocks;
        let mut depth = 0;
        while let [Block::Quote(inner)] = d.as_slice() {
            depth += 1;
            d = inner;
        }
        assert_eq!(depth, MAX_NEST);
        assert_eq!(*d, [Block::Para(vec![plain("deep")])]);
    }

    /// AC7: hostile markdown is bounded and sanitized.
    #[test]
    fn hostile_markdown_is_bounded_and_sanitized() {
        let started = std::time::Instant::now();
        let deep_list: String = (0..250)
            .map(|i| format!("{}- x\n", "  ".repeat(i)))
            .collect();
        let cases = [
            "x".repeat(64 * 1024),
            ">".repeat(64 * 1024),
            deep_list,
            "*".repeat(32 * 1024) + "x" + &"*".repeat(32 * 1024),
            format!("{}\n{}\n", "|a".repeat(8000) + "|", "|-".repeat(8000) + "|"),
            "<latex>x</latex>".repeat(4000),
            "\u{e000}\u{e001}\u{1b}[2J \u{202e}text\u{9b}".to_owned(),
        ];
        for text in &cases {
            let p = prepare(text);
            assert!(p.tags.len() <= MAX_FORMULAS);
            check(&parse(text).blocks, 0);
        }
        // Debug build, all seven: well under a second each in practice.
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        assert!(
            prepare(&cases[6]).tags.is_empty(),
            "input cannot forge a placeholder"
        );
        assert_eq!(
            parse(&cases[6]).blocks,
            [Block::Para(vec![plain("[2J text")])]
        );
    }

    /// Every string is sanitized (so no placeholder is left: they are
    /// private-use), and lists and quotes are at most MAX_NEST deep.
    fn check(blocks: &[Block], depth: usize) {
        assert!(depth <= MAX_NEST);
        let clean = |t: &str| assert_eq!(*t, sanitize(t));
        let spans = |spans: &[Span]| {
            for s in spans {
                match s {
                    Span::Text(t, _) | Span::Code(t) => clean(t),
                    Span::Break => {}
                }
            }
        };
        for b in blocks {
            match b {
                Block::Para(s) | Block::Heading(_, s) => spans(s),
                Block::Code(lines) => lines.iter().for_each(|l| clean(l)),
                Block::List(_, items) => items.iter().for_each(|i| check(i, depth + 1)),
                Block::Quote(inner) => check(inner, depth + 1),
                Block::Table(rows) => rows.iter().flatten().for_each(|c| spans(c)),
                Block::Rule => {}
            }
        }
    }
}
