//! The formulas popup (Alt+u): the active session's recent `<latex>`
//! formulas, oldest first, each rendered in 2D above its source. The view's
//! state and keys are here, its lines too (pure); drawing is in
//! `ui/formulas.rs`.

use std::cell::Cell;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::latex;
use super::sessions::SessionView;
use crate::proto::SessionId;

/// How far PageUp/PageDown (and Space) scroll.
const PAGE: u16 = 10;

/// What the popup shows when the session has no formulas.
pub const EMPTY: [&str; 4] = [
    "No formulas yet.",
    "",
    "claude writes them in <latex> tags when asked for math (on by default;",
    "`latex = false` under [claude] on the session's host, then `claudio daemon restart`).",
];

/// The popup's state: whose formulas, and how far down.
pub struct FormulasView {
    pub session: SessionId,
    scroll: u16,
    /// The largest useful scroll, known once drawn (see `ui/formulas.rs`).
    max_scroll: Cell<u16>,
}

/// One line of the popup.
#[derive(Debug, PartialEq)]
pub enum Line {
    /// A row of a formula's rendering.
    Formula(String),
    /// A formula's source, on one line (shown dimmed).
    Source(String),
    /// The text shown when there are no formulas (dimmed).
    Hint(&'static str),
    Blank,
}

impl FormulasView {
    /// The popup for `session`, scrolled to its newest formulas.
    pub fn new(session: SessionId) -> FormulasView {
        FormulasView {
            session,
            scroll: u16::MAX,
            max_scroll: Cell::new(u16::MAX),
        }
    }

    /// The scroll offset to draw at.
    pub fn scroll(&self) -> u16 {
        self.scroll.min(self.max_scroll.get())
    }

    /// Record the largest useful scroll offset of what is being drawn.
    pub fn set_max_scroll(&self, max: u16) {
        self.max_scroll.set(max);
    }

    /// Handle a key; `true` closes the popup. Ctrl and Alt chords are not
    /// its own (a manager key reaches the app before this).
    pub fn on_key(&mut self, key: &KeyEvent) -> bool {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return false;
        }
        let at = self.scroll();
        self.scroll = match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return true,
            KeyCode::Down | KeyCode::Char('j') => at.saturating_add(1),
            KeyCode::Up | KeyCode::Char('k') => at.saturating_sub(1),
            KeyCode::PageDown | KeyCode::Char(' ') => at.saturating_add(PAGE),
            KeyCode::PageUp => at.saturating_sub(PAGE),
            KeyCode::Home | KeyCode::Char('g') => 0,
            KeyCode::End | KeyCode::Char('G') => u16::MAX,
            _ => at,
        };
        false
    }
}

/// The popup's lines for session `v`: its formulas oldest first, each its
/// 2D rendering (else its one-line one, else its source) then its source,
/// with a blank line between them. A formula of a list still being rendered
/// shows its source. With no formulas, how to get them.
pub fn lines(v: &SessionView) -> Vec<Line> {
    if v.latex_srcs.is_empty() {
        return EMPTY
            .iter()
            .map(|t| match *t {
                "" => Line::Blank,
                t => Line::Hint(t),
            })
            .collect();
    }
    let mut lines = Vec::new();
    for src in &v.latex_srcs {
        if !lines.is_empty() {
            lines.push(Line::Blank);
        }
        let source = latex::prepare(src);
        let shown = v.latex.iter().find(|f| f.src == *src);
        match shown.map(|f| (&f.block, &f.inline)) {
            Some((Some(block), _)) => {
                lines.extend(block.rows.iter().cloned().map(Line::Formula));
            }
            Some((None, Some(inline))) => lines.push(Line::Formula(inline.clone())),
            _ => lines.push(Line::Formula(source.clone())),
        }
        lines.push(Line::Source(source));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::SessionKind;
    use crate::tui::latex::{Formula, Rendered};
    use crate::tui::typeset::Laid;
    use crossterm::event::KeyEvent;

    fn view_with(srcs: &[&str], rendered: Vec<Formula>) -> SessionView {
        let mut v = SessionView::new(
            uuid::Uuid::nil(),
            "local",
            "/w",
            SessionKind::Claude,
            (24, 80),
        );
        v.latex_srcs = srcs.iter().map(|s| s.to_string()).collect();
        v.latex = rendered;
        v
    }

    #[test]
    fn each_formula_shows_its_rendering_then_its_source() {
        let block = Laid {
            rows: vec![" a".into(), "───".into(), " b".into()],
            baseline: 1,
        };
        let frac = Formula::new(
            "\\frac{a}{b}",
            Rendered {
                block: Some(block),
                ..Rendered::default()
            },
        );
        let inline = Rendered {
            inline: Some("x²".into()),
            ..Rendered::default()
        };
        let one = Formula::new("x^2", inline);
        let v = view_with(&["\\frac{a}{b}", "x^2", "y\n+ 1"], vec![frac, one]);
        assert_eq!(
            lines(&v),
            [
                Line::Formula(" a".into()),
                Line::Formula("───".into()),
                Line::Formula(" b".into()),
                Line::Source("\\frac{a}{b}".into()),
                Line::Blank,
                Line::Formula("x²".into()),
                Line::Source("x^2".into()),
                Line::Blank,
                // Not rendered yet: its source, on one line.
                Line::Formula("y + 1".into()),
                Line::Source("y + 1".into()),
            ]
        );
    }

    #[test]
    fn no_formulas_says_how_to_get_them() {
        let v = view_with(&[], Vec::new());
        let text: Vec<Line> = lines(&v);
        assert_eq!(text[0], Line::Hint("No formulas yet."));
        assert!(text.contains(&Line::Hint(EMPTY[3])));
    }

    #[test]
    fn keys_scroll_and_close() {
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        let mut view = FormulasView::new(uuid::Uuid::nil());
        view.set_max_scroll(30);
        assert_eq!(view.scroll(), 30, "opens at the newest");
        assert!(!view.on_key(&key(KeyCode::Up)));
        assert_eq!(view.scroll(), 29);
        view.on_key(&key(KeyCode::PageUp));
        assert_eq!(view.scroll(), 19);
        view.on_key(&key(KeyCode::Char('g')));
        assert_eq!(view.scroll(), 0);
        view.on_key(&key(KeyCode::Char('j')));
        view.on_key(&key(KeyCode::Char(' ')));
        assert_eq!(view.scroll(), 11);
        view.on_key(&key(KeyCode::End));
        assert_eq!(view.scroll(), 30);
        // A chord is not the popup's.
        let alt_k = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::ALT);
        assert!(!view.on_key(&alt_k));
        assert_eq!(view.scroll(), 30);
        assert!(view.on_key(&key(KeyCode::Char('q'))));
        assert!(view.on_key(&key(KeyCode::Esc)));
    }
}
