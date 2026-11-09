//! The reader view (Alt+u): the session's recent answers, laid out by
//! claudio from the transcript as markdown; formulas and diagrams are
//! their source (they render in the web UI only). Its state and keys are
//! here (pure); drawing is in `ui/reader.rs`.
//!
//! - [`doc`] — an answer as markdown blocks (parsed off the UI thread).
//! - [`layout`] — such blocks wrapped to the view's width.
//! - [`ReaderView`] — which answer, and how far down.

pub mod doc;
pub mod layout;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Modifier, Style};
use unicode_width::UnicodeWidthStr;

use claudio_core::proto::SessionId;
use layout::{Line, Seg};

/// How far PageUp/PageDown (and Space) scroll, and the mouse wheel.
const PAGE: u16 = 10;
const WHEEL: u16 = 3;
/// Below this many columns the view shows only [`TOO_NARROW`].
pub const MIN_WIDTH: usize = 20;
const TOO_NARROW: &str = "too narrow";
const LOADING: &str = "Loading…";
const NO_ANSWERS: &str = "No answers yet";

/// One answer, ready to lay out: parsed off the UI thread.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderedAnswer {
    /// The prompt, as the daemon sent it.
    pub prompt: String,
    pub doc: doc::Doc,
}

/// `answers` as the reader shows them: each a markdown document. Run off
/// the UI thread (the text is attacker-influenced).
pub fn render(answers: &[claudio_core::proto::Answer]) -> Vec<RenderedAnswer> {
    answers
        .iter()
        .map(|answer| RenderedAnswer {
            prompt: answer.prompt.clone(),
            doc: doc::parse(&answer.text),
        })
        .collect()
}

/// What the view has to show.
#[derive(Debug, Clone, PartialEq)]
pub enum Content {
    /// The reply, or its rendering, is on its way.
    Loading,
    /// The request failed: why (an old daemon's restart notice, …).
    Failed(String),
    /// The answers, oldest first.
    Ready(Vec<RenderedAnswer>),
}

/// The width the answers were laid out for, and each answer's lines.
type LayoutCache = (usize, HashMap<usize, Rc<Vec<Line>>>);

/// The reader's state: whose answers, which one, and how far down.
pub struct ReaderView {
    pub session: SessionId,
    pub host: String,
    /// The `App::modal_gen` it was opened under: replies for another are
    /// dropped.
    pub gen: u64,
    /// A document's title (an extension's result), shown as the header;
    /// `None` for a session's answers.
    pub title: Option<String>,
    pub content: Content,
    /// The answer shown (an index into `Ready`).
    answer: usize,
    scroll: u16,
    /// The largest useful scroll, known once drawn (see `ui/reader.rs`).
    max_scroll: Cell<u16>,
    /// The answers laid out for one width (a resize starts over).
    layouts: RefCell<LayoutCache>,
}

impl ReaderView {
    /// The view of `session` on `host`, waiting for its answers.
    pub fn new(session: SessionId, host: impl Into<String>, gen: u64) -> ReaderView {
        ReaderView {
            session,
            host: host.into(),
            gen,
            title: None,
            content: Content::Loading,
            answer: 0,
            scroll: 0,
            max_scroll: Cell::new(u16::MAX),
            layouts: RefCell::new((0, HashMap::new())),
        }
    }

    /// A view of one document titled `title` (rendered like an answer,
    /// under `gen`; `session` is the id its rendering is sent with).
    pub fn document(session: SessionId, title: String, gen: u64) -> ReaderView {
        ReaderView {
            title: Some(title),
            ..ReaderView::new(session, "local", gen)
        }
    }

    /// Show `answers` (oldest first), from the top of the newest.
    pub fn set_answers(&mut self, answers: Vec<RenderedAnswer>) {
        self.answer = answers.len().saturating_sub(1);
        self.content = Content::Ready(answers);
        self.layouts.borrow_mut().1.clear();
        self.scroll = 0;
    }

    /// Show why there are no answers.
    pub fn set_failed(&mut self, why: impl Into<String>) {
        self.content = Content::Failed(why.into());
    }

    /// The scroll offset to draw at.
    pub fn scroll(&self) -> u16 {
        self.scroll.min(self.max_scroll.get())
    }

    /// Record the largest useful scroll offset of what is being drawn.
    pub fn set_max_scroll(&self, max: u16) {
        self.max_scroll.set(max);
    }

    /// Handle a key; `true` closes the view. Ctrl and Alt chords are not
    /// its own (a manager key reaches the app before this).
    pub fn on_key(&mut self, key: &KeyEvent) -> bool {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return false;
        }
        let at = self.scroll();
        let answers = match &self.content {
            Content::Ready(answers) => answers.len(),
            _ => 0,
        };
        self.scroll = match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return true,
            KeyCode::Down | KeyCode::Char('j') => at.saturating_add(1),
            KeyCode::Up | KeyCode::Char('k') => at.saturating_sub(1),
            KeyCode::PageDown | KeyCode::Char(' ') => at.saturating_add(PAGE),
            KeyCode::PageUp => at.saturating_sub(PAGE),
            KeyCode::Home | KeyCode::Char('g') => 0,
            KeyCode::End | KeyCode::Char('G') => u16::MAX,
            KeyCode::Left | KeyCode::Char('h') if self.answer > 0 => {
                self.answer -= 1;
                0
            }
            KeyCode::Right | KeyCode::Char('l') if self.answer + 1 < answers => {
                self.answer += 1;
                0
            }
            _ => at,
        };
        false
    }

    /// The mouse wheel: scroll down (`down`) or up.
    pub fn on_wheel(&mut self, down: bool) {
        let at = self.scroll();
        self.scroll = if down {
            at.saturating_add(WHEEL)
        } else {
            at.saturating_sub(WHEEL)
        };
    }

    /// The header row, at most `width` columns: `answer k/N · <prompt>`
    /// (the prompt dimmed, on one line, cut with `…`), or what the body is.
    pub fn header(&self, width: usize) -> Line {
        let mut line = Line::default();
        let push = |line: &mut Line, text: &str, style: Style| {
            line.0.push(Seg {
                text: text.to_owned(),
                style,
            })
        };
        match &self.content {
            _ if self.title.is_some() => {
                let title = self.title.as_deref().unwrap_or_default();
                push(&mut line, &fit(title, width), Style::default());
            }
            Content::Ready(answers) if !answers.is_empty() => {
                let label = format!("answer {}/{}", self.answer + 1, answers.len());
                let prompt = answers[self.answer]
                    .prompt
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                let room = width.saturating_sub(label.width() + 3);
                push(&mut line, &label, Style::default());
                if room > 0 && !prompt.is_empty() {
                    let dim = Style::default().add_modifier(Modifier::DIM);
                    push(&mut line, &format!(" · {}", fit(&prompt, room)), dim);
                }
            }
            _ => push(&mut line, "reader", Style::default()),
        }
        line
    }

    /// The body's lines at `width`: the answer laid out (cached for the
    /// width), or a message.
    pub fn body(&self, width: usize) -> Rc<Vec<Line>> {
        let message = |text: &str| {
            let dim = Style::default().add_modifier(Modifier::DIM);
            Rc::new(vec![Line(vec![Seg {
                text: text.to_owned(),
                style: dim,
            }])])
        };
        if width < MIN_WIDTH {
            return message(TOO_NARROW);
        }
        let answers = match &self.content {
            Content::Loading => return message(LOADING),
            Content::Failed(why) => return message(why),
            Content::Ready(answers) => answers,
        };
        let Some(answer) = answers.get(self.answer) else {
            return message(NO_ANSWERS);
        };
        let mut layouts = self.layouts.borrow_mut();
        if layouts.0 != width {
            *layouts = (width, HashMap::new());
        }
        let laid = layouts
            .1
            .entry(self.answer)
            .or_insert_with(|| Rc::new(layout::layout(&answer.doc, width)));
        Rc::clone(laid)
    }
}

/// `text` cut to `width` columns, with `…` when cut.
fn fit(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_owned();
    }
    let mut out = String::new();
    for c in text.chars() {
        if out.width() + c.to_string().width() + 1 > width {
            break;
        }
        out.push(c);
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn answer(prompt: &str, markdown: &str) -> RenderedAnswer {
        RenderedAnswer {
            prompt: prompt.into(),
            doc: doc::parse(markdown),
        }
    }

    fn texts(lines: &[Line]) -> Vec<String> {
        lines.iter().map(Line::text).collect()
    }

    fn ready(answers: Vec<RenderedAnswer>) -> ReaderView {
        let mut view = ReaderView::new(uuid::Uuid::nil(), "local", 1);
        view.set_answers(answers);
        view
    }

    #[test]
    fn it_opens_on_the_newest_answer_and_moves_between_them() {
        let mut view = ready(vec![answer("first", "one"), answer("second", "two")]);
        assert_eq!(view.header(40).text(), "answer 2/2 · second");
        assert_eq!(texts(&view.body(40)), ["two"]);
        view.set_max_scroll(5);
        view.on_key(&key(KeyCode::Down));
        assert_eq!(view.scroll(), 1);
        // A move to another answer starts at its top.
        view.on_key(&key(KeyCode::Left));
        assert_eq!(view.header(40).text(), "answer 1/2 · first");
        assert_eq!(view.scroll(), 0);
        view.on_key(&key(KeyCode::Char('h')));
        assert_eq!(view.header(40).text(), "answer 1/2 · first", "none before");
        view.on_key(&key(KeyCode::Char('l')));
        view.on_key(&key(KeyCode::Right));
        assert_eq!(view.header(40).text(), "answer 2/2 · second", "none after");
    }

    #[test]
    fn keys_scroll_and_close() {
        let mut view = ready(vec![answer("p", "x")]);
        view.set_max_scroll(30);
        assert_eq!(view.scroll(), 0, "opens at the top");
        view.on_key(&key(KeyCode::PageDown));
        assert_eq!(view.scroll(), 10);
        view.on_key(&key(KeyCode::Char(' ')));
        view.on_key(&key(KeyCode::Char('k')));
        assert_eq!(view.scroll(), 19);
        view.on_key(&key(KeyCode::PageUp));
        assert_eq!(view.scroll(), 9);
        view.on_key(&key(KeyCode::End));
        assert_eq!(view.scroll(), 30);
        view.on_key(&key(KeyCode::Char('g')));
        assert_eq!(view.scroll(), 0);
        view.on_key(&key(KeyCode::Char('G')));
        view.on_key(&key(KeyCode::Home));
        assert_eq!(view.scroll(), 0);
        view.on_wheel(true);
        view.on_wheel(true);
        view.on_wheel(false);
        assert_eq!(view.scroll(), 3);
        // A chord is not the view's.
        let alt_k = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::ALT);
        assert!(!view.on_key(&alt_k));
        assert_eq!(view.scroll(), 3);
        assert!(view.on_key(&key(KeyCode::Char('q'))));
        assert!(view.on_key(&key(KeyCode::Esc)));
    }

    #[test]
    fn the_header_puts_the_prompt_on_one_cut_line() {
        let view = ready(vec![answer("why  is\nthe sky\tblue at all", "x")]);
        assert_eq!(
            view.header(80).text(),
            "answer 1/1 · why is the sky blue at all"
        );
        assert_eq!(view.header(24).text(), "answer 1/1 · why is the…");
        assert_eq!(view.header(12).text(), "answer 1/1");
        let header = view.header(80);
        assert!(header.0[1].style.add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn the_body_says_what_it_is_waiting_for() {
        let mut view = ReaderView::new(uuid::Uuid::nil(), "local", 1);
        assert_eq!(texts(&view.body(40)), [LOADING]);
        let old = "daemon on local is older: run `claudio daemon restart`";
        view.set_failed(old);
        assert_eq!(texts(&view.body(60)), [old]);
        view.set_answers(Vec::new());
        assert_eq!(texts(&view.body(40)), [NO_ANSWERS]);
        assert_eq!(view.header(40).text(), "reader");
        let view = ready(vec![answer("p", "text")]);
        assert_eq!(texts(&view.body(MIN_WIDTH - 1)), [TOO_NARROW]);
    }

    #[test]
    fn a_layout_is_kept_for_its_width_only() {
        let view = ready(vec![answer("p", "a long line of words to wrap")]);
        let first = view.body(40);
        assert!(Rc::ptr_eq(&first, &view.body(40)), "cached");
        let narrow = view.body(20);
        assert_eq!(texts(&narrow), ["a long line of words", "to wrap"]);
        assert!(!Rc::ptr_eq(&first, &view.body(40)), "a resize starts over");
        assert_eq!(view.layouts.borrow().1.len(), 1);
    }

    /// Formulas and diagrams are their source; Tab is not the view's
    /// any more (the formulas list went with the TUI's typesetting).
    #[test]
    fn formulas_and_diagrams_are_shown_as_their_source() {
        let text = "E is <latex>E = mc^2</latex>\n\n```mermaid\nflowchart LR\n  a --> b\n```";
        let mut view = ready(render(&[claudio_core::proto::Answer {
            prompt: "p".into(),
            text: text.into(),
        }]));
        let want = [
            "E is <latex>E = mc^2</latex>",
            "",
            "  flowchart LR",
            "    a --> b",
        ];
        assert_eq!(texts(&view.body(40)), want);
        view.set_max_scroll(5);
        view.on_key(&key(KeyCode::Tab));
        assert_eq!(view.header(40).text(), "answer 1/1 · p");
        assert_eq!(view.scroll(), 0);
        assert_eq!(texts(&view.body(40)), want);
    }
}
