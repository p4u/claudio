//! The reader view's formulas list (Tab in Alt+u): the active session's
//! recent `<latex>` formulas, oldest first, each rendered in 2D above its
//! source. Its lines are here (pure); the reader draws them.

use super::latex;
use super::sessions::SessionView;

/// What the list shows when the session has no formulas.
pub const EMPTY: [&str; 4] = [
    "No formulas yet.",
    "",
    "claude writes them in <latex> tags when asked for math (on by default;",
    "`latex = false` under [claude] on the session's host, then `claudio daemon restart`).",
];

/// One line of the list.
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

/// The list's lines for session `v`: its formulas oldest first, each its
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
}
