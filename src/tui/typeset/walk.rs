//! The one-line walk of v0.4.0, moved here unchanged: one line of text
//! for a parse tree, for formulas whose text-style layout is taller than one
//! row (the pane shows one line per tag).

use rust_latex_parser::{AccentKind, EqNode, MatrixKind};

use super::{flatten, is_atom, mathfont, subscript_char, superscript_char, top_level};

/// One line of text for `node`, modelled on term-maths' layout: fractions
/// become `a/b`, scripts Unicode super- and subscripts (else `^(…)` and
/// `_(…)`), roots `√x`, matrices `[a b; c d]`, cases `{ 1 if x > 0; 0
/// otherwise }`. Symbols are already Unicode in the parse tree.
pub fn one_line(node: &EqNode) -> String {
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
        EqNode::MathFont { kind, content } => one_line(content)
            .chars()
            .map(|c| mathfont::map_char(*kind, c))
            .collect(),
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

/// `s` as an operand: as it is when it reads as one (see [`is_atom`]),
/// else in parentheses.
fn operand(s: String) -> String {
    if is_atom(&s) {
        s
    } else {
        format!("({s})")
    }
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
