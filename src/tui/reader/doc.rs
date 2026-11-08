//! One of claude's answers as the reader view's document: markdown blocks
//! (pulldown-cmark) whose `<latex>` formulas are indices into the answer's
//! formula list. Width-independent; `layout.rs` wraps it.
//!
//! Pure, and run on the render worker (`tui/mod.rs`), never on the UI
//! thread. The text is attacker-influenced, so:
//! - [`prepare`] sanitizes it again, then replaces each formula tag (by
//!   `extract_latex`'s rules) with one private-use placeholder; sanitizing
//!   removed every private-use character, so the text cannot forge one.
//! - [`parse`] turns placeholders back into formulas while it reads
//!   markdown (iterating chars, never graphemes), and sanitizes each text
//!   run again once its placeholders are gone. In code, a placeholder is
//!   its tag's text again.
//! - Lists and quotes nest at most [`MAX_NEST`] deep; deeper content is
//!   flattened into the deepest one, so layout recursion is bounded.

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

use crate::claude::latex::{is_combining, lens_tags, sanitize, MAX_FORMULAS};

/// The first placeholder: formula `i` of an answer is `PLACEHOLDER + i`.
const PLACEHOLDER: u32 = 0xE000;
/// How deep lists and block quotes nest.
pub const MAX_NEST: usize = 8;
/// What a tab is in a code block (elsewhere it is one space).
const TAB: &str = "    ";

/// An answer's text ready for [`parse`]: formula tags replaced by
/// placeholders, and the tags.
#[derive(Debug, Clone, PartialEq)]
pub struct Prepared {
    pub text: String,
    pub tags: Vec<FormulaTag>,
    /// The tag lens of each of `tags` (`latex`, …).
    pub lenses: Vec<String>,
    /// The fence lenses: a fenced code block whose info string starts with
    /// one becomes a [`Block::Lens`].
    pub fences: Vec<String>,
}

/// One formula tag of an answer.
#[derive(Debug, Clone, PartialEq)]
pub struct FormulaTag {
    /// The formula, as `extract_latex` gives it (the renderer's input).
    pub src: String,
    /// The whole tag as written, shown where markdown made it code.
    pub raw: String,
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
    /// A display formula: a paragraph or tight item that is only a formula.
    Display(usize),
    /// A fenced code block of fence lens `lens`: its lines (as `Code`'s),
    /// and its rendering's rows once the render worker (or the extension
    /// host) made one.
    Lens {
        lens: String,
        lines: Vec<String>,
        rows: Option<Vec<String>>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Span {
    Text(String, Emphasis),
    /// Inline code.
    Code(String),
    /// A formula in the text: an index into the answer's formulas.
    Formula(usize),
    /// A table cell that is only a formula, typeset in display style.
    Display(usize),
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
/// [`MAX_FORMULAS`] formula tags replaced by a placeholder, and a combining
/// mark right after a tag dropped (it would join the placeholder). Later
/// tags stay as they are written.
#[cfg(test)]
pub fn prepare(text: &str) -> Prepared {
    prepare_lenses(text, &["latex"], &[])
}

/// [`prepare`] for the tags of tag lenses `tags` (the first
/// [`MAX_FORMULAS`] of them all), with fence lenses `fences` for [`parse`].
pub fn prepare_lenses(text: &str, tags: &[&str], fences: &[&str]) -> Prepared {
    let clean = sanitize(text);
    let mut out = String::with_capacity(clean.len());
    let mut found = Vec::new();
    let mut lenses = Vec::new();
    let mut copied = 0;
    for (range, lens, src) in lens_tags(&clean, tags).into_iter().take(MAX_FORMULAS) {
        out.push_str(&clean[copied..range.start]);
        out.push(placeholder(found.len()));
        found.push(FormulaTag {
            src,
            raw: clean[range.clone()].to_owned(),
        });
        lenses.push(lens);
        let rest = &clean[range.end..];
        copied = range.end + (rest.len() - rest.trim_start_matches(is_combining).len());
    }
    out.push_str(&clean[copied..]);
    Prepared {
        text: out,
        tags: found,
        lenses,
        fences: fences.iter().map(|f| f.to_string()).collect(),
    }
}

/// The placeholder of formula `i` (`i` < [`MAX_FORMULAS`]).
fn placeholder(i: usize) -> char {
    char::from_u32(PLACEHOLDER + i as u32).unwrap_or(char::REPLACEMENT_CHARACTER)
}

/// The formula a placeholder stands for, if `c` is one of `count`.
fn formula_of(c: char, count: usize) -> Option<usize> {
    let i = (c as u32).checked_sub(PLACEHOLDER)? as usize;
    (i < count).then_some(i)
}

/// The markdown of `prepared` as a [`Doc`]. pulldown-cmark reads it with
/// tables, strikethrough and task lists only (no math, footnotes or smart
/// punctuation: `$` is text). A formula is display when its placeholder is
/// all a paragraph, a tight list item or a table cell holds.
pub fn parse(prepared: &Prepared) -> Doc {
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut b = Builder::new(&prepared.tags, &prepared.fences);
    for event in Parser::new_ext(&prepared.text, options) {
        b.event(event);
    }
    b.flush();
    Doc { blocks: b.root }
}

impl Doc {
    /// Call `f` with each fence-lens block's lens, source (its lines) and
    /// rows, in document order, inside lists and quotes too.
    pub fn each_lens(&mut self, f: &mut impl FnMut(&str, String, &mut Option<Vec<String>>)) {
        fn walk(blocks: &mut [Block], f: &mut impl FnMut(&str, String, &mut Option<Vec<String>>)) {
            for block in blocks {
                match block {
                    Block::Lens { lens, lines, rows } => f(lens, lines.join("\n"), rows),
                    Block::List(_, items) => {
                        for item in items {
                            walk(item, f);
                        }
                    }
                    Block::Quote(inner) => walk(inner, f),
                    _ => {}
                }
            }
        }
        walk(&mut self.blocks, f);
    }
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
    tags: &'a [FormulaTag],
    fences: &'a [String],
    /// The fence lens of the code block being read, if it is one's.
    fence: Option<String>,
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
    fn new(tags: &'a [FormulaTag], fences: &'a [String]) -> Builder<'a> {
        Builder {
            tags,
            fences,
            fence: None,
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
            Tag::CodeBlock(kind) => {
                self.flush();
                self.code = Some(String::new());
                self.fence = match kind {
                    CodeBlockKind::Fenced(info) => info
                        .split_whitespace()
                        .next()
                        .filter(|word| self.fences.iter().any(|f| f == word))
                        .map(str::to_owned),
                    CodeBlockKind::Indented => None,
                };
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
                match self.fence.take() {
                    Some(lens) => self.push(Block::Lens {
                        lens,
                        lines,
                        rows: None,
                    }),
                    None => self.push(Block::Code(lines)),
                }
            }
            TagEnd::BlockQuote(_) | TagEnd::List(_) | TagEnd::Item => self.close_frame(),
            TagEnd::TableCell => {
                let mut cell = trimmed(std::mem::take(&mut self.spans));
                if let [Span::Formula(i)] = cell.as_slice() {
                    cell = vec![Span::Display(*i)];
                }
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

    /// End the inline content gathered so far as a paragraph (a display
    /// formula when it is only one), unless a heading or table cell is
    /// gathering it.
    fn flush(&mut self) {
        if self.heading.is_some() || self.table.is_some() {
            return;
        }
        let spans = trimmed(std::mem::take(&mut self.spans));
        match spans.as_slice() {
            [] => {}
            [Span::Formula(i)] => self.push(Block::Display(*i)),
            _ => self.push(Block::Para(spans)),
        }
    }

    /// Inline text: its placeholders become formulas, the runs between
    /// them are sanitized (tabs as one space) and joined to the text before
    /// them when they share its emphasis.
    fn text(&mut self, text: &str) {
        let mut run = String::new();
        for c in text.chars() {
            match formula_of(c, self.tags.len()) {
                Some(i) => {
                    self.push_run(&mut run);
                    self.spans.push(Span::Formula(i));
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
            match formula_of(c, self.tags.len()) {
                Some(i) => out.push_str(&self.tags[i].raw),
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
        parse(&prepare(text))
    }

    fn plain(s: &str) -> Span {
        Span::Text(s.into(), Emphasis::default())
    }

    #[test]
    fn tags_become_placeholders_after_sanitizing() {
        let p = prepare("a <latex>x^2</latex>\u{301}b \u{e000}c `<latex>y</latex>`");
        assert_eq!(p.text, "a \u{e000}b c `<latex>y</latex>`");
        assert_eq!(
            p.tags,
            [FormulaTag {
                src: "x^2".into(),
                raw: "<latex>x^2</latex>".into(),
            }]
        );
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
                    Span::Code("code".into()),
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
                    vec![vec![plain("1")], vec![Span::Display(0)]],
                ]),
            ]
        );
    }

    #[test]
    fn a_formula_alone_is_display() {
        let d = doc("Text <latex>a</latex> here.\n\n<latex>b</latex>\n\n- <latex>c</latex>\n- item <latex>d</latex>\n\n> <latex>e</latex>\n");
        assert_eq!(
            d.blocks,
            [
                Block::Para(vec![plain("Text "), Span::Formula(0), plain(" here.")]),
                Block::Display(1),
                Block::List(
                    None,
                    vec![
                        vec![Block::Display(2)],
                        vec![Block::Para(vec![plain("item "), Span::Formula(3)])],
                    ]
                ),
                Block::Quote(vec![Block::Display(4)]),
            ]
        );
    }

    #[test]
    fn a_placeholder_in_code_is_its_tag_again() {
        // Markdown takes this backtick run as code, `extract_latex` does
        // not (its code spans end at the line).
        let p = prepare("``a\n<latex>x</latex> b``");
        assert_eq!(p.tags.len(), 1);
        assert_eq!(
            parse(&p).blocks,
            [Block::Para(vec![Span::Code("a <latex>x</latex> b".into())])]
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
            let d = parse(&p);
            check(&d.blocks, p.tags.len(), 0);
        }
        // Debug build, all seven: well under a second each in practice.
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        let p = prepare(&cases[6]);
        assert!(p.tags.is_empty(), "input cannot forge a placeholder");
        assert_eq!(parse(&p).blocks, [Block::Para(vec![plain("[2J text")])]);
    }

    /// Every string is sanitized, every formula index valid, and lists and
    /// quotes at most MAX_NEST deep.
    fn check(blocks: &[Block], formulas: usize, depth: usize) {
        assert!(depth <= MAX_NEST);
        let spans = |spans: &[Span]| {
            for s in spans {
                match s {
                    Span::Text(t, _) | Span::Code(t) => assert_eq!(*t, sanitize(t)),
                    Span::Formula(i) | Span::Display(i) => assert!(*i < formulas),
                    Span::Break => {}
                }
            }
        };
        for b in blocks {
            match b {
                Block::Para(s) | Block::Heading(_, s) => spans(s),
                Block::Code(lines) | Block::Lens { lines, .. } => {
                    lines.iter().for_each(|l| assert_eq!(*l, sanitize(l)))
                }
                Block::List(_, items) => items.iter().for_each(|i| check(i, formulas, depth + 1)),
                Block::Quote(inner) => check(inner, formulas, depth + 1),
                Block::Table(rows) => rows.iter().flatten().for_each(|c| spans(c)),
                Block::Rule => {}
                Block::Display(i) => assert!(*i < formulas),
            }
        }
    }

    #[test]
    fn every_tag_lens_gets_placeholders_and_fence_lenses_become_lens_blocks() {
        let text = "Press <keys>Ctrl+C</keys> for <latex>x</latex> <kbd>k</kbd>.\n\n\
                    ```mermaid extra\nflowchart LR\n\ta --> b\n```\n\n\
                    - ```mermaid\n  graph TD\n  ```\n\n```text\nplain\n```\n";
        let p = prepare_lenses(text, &["latex", "keys"], &["mermaid"]);
        assert_eq!(p.lenses, ["keys", "latex"]);
        assert_eq!(p.tags[0].src, "Ctrl+C");
        assert_eq!(p.tags[0].raw, "<keys>Ctrl+C</keys>");
        let mut d = parse(&p);
        assert_eq!(
            d.blocks[0],
            Block::Para(vec![
                plain("Press "),
                Span::Formula(0),
                plain(" for "),
                Span::Formula(1),
                plain(" <kbd>k</kbd>."),
            ])
        );
        assert_eq!(
            d.blocks[1],
            Block::Lens {
                lens: "mermaid".into(),
                lines: vec!["flowchart LR".into(), "    a --> b".into()],
                rows: None,
            }
        );
        assert_eq!(d.blocks[3], Block::Code(vec!["plain".into()]));
        // Every lens block, nested ones too, in order.
        let mut seen = Vec::new();
        d.each_lens(&mut |lens, src, rows| {
            seen.push((lens.to_owned(), src));
            *rows = Some(vec!["drawn".into()]);
        });
        assert_eq!(
            seen,
            [
                ("mermaid".to_owned(), "flowchart LR\n    a --> b".to_owned()),
                ("mermaid".to_owned(), "graph TD".to_owned()),
            ]
        );
        assert!(matches!(&d.blocks[1], Block::Lens { rows: Some(r), .. } if r == &["drawn"]));
        // Without fence lenses, fences are code.
        let d = parse(&prepare_lenses(text, &["latex"], &[]));
        assert!(matches!(&d.blocks[1], Block::Code(_)));
    }
}
