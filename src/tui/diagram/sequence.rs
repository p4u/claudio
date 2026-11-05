//! Sequence diagrams (`sequenceDiagram`): participants side by side,
//! lifelines down, one row band per message, note or block line.

use super::canvas::{self, Canvas};
use super::text;
use super::{Unsupported, MAX_COLS, MAX_EDGES, MAX_NODES, MAX_ROWS};

/// The deepest nesting of `loop`/`alt`/… blocks.
const MAX_DEPTH: usize = 8;

/// Message arrows, longest first so that `-->>` is not read as `-->`.
/// (pattern, dotted, head)
const ARROWS: [(&str, bool, bool); 8] = [
    ("-->>", true, true),
    ("->>", false, true),
    ("--x", true, true),
    ("--)", true, true),
    ("-->", true, false),
    ("-x", false, true),
    ("-)", false, true),
    ("->", false, false),
];

enum Side {
    Left,
    Right,
    Over,
}

enum Item {
    Message {
        from: usize,
        to: usize,
        text: Vec<String>,
        dotted: bool,
        head: bool,
    },
    Note {
        side: Side,
        a: usize,
        b: usize,
        text: Vec<String>,
    },
    /// `loop`, `alt`, `opt`, …: a frame opens.
    Open(String),
    /// `else`, `and`, `option`: a frame's next part.
    Split(String),
    End,
    /// `box [label]`: groups participants; drawn as nothing, and its `end`
    /// closes no frame.
    Box,
}

#[derive(Default)]
struct Diagram {
    ids: Vec<String>,
    labels: Vec<Vec<String>>,
    items: Vec<Item>,
    messages: usize,
    /// `title <text>`, drawn as the first row.
    title: Option<String>,
}

/// The drawing of a sequence diagram with statements `lines`.
pub fn render<'a>(lines: impl Iterator<Item = &'a str>) -> Result<Vec<String>, Unsupported> {
    let mut d = Diagram::default();
    // The open blocks, innermost last: `true` for a frame, `false` for a
    // `box`. At most one per source line.
    let mut open: Vec<bool> = Vec::new();
    let mut depth = 0usize;
    for line in lines {
        let item = d.statement(line)?;
        match &item {
            Some(Item::Open(_)) => {
                depth += 1;
                if depth > MAX_DEPTH {
                    return Err(Unsupported::new("blocks nested too deep"));
                }
                open.push(true);
            }
            Some(Item::Box) => {
                open.push(false);
                continue;
            }
            Some(Item::Split(_)) if open.last() != Some(&true) => {
                return Err(Unsupported::new("else outside a block"));
            }
            Some(Item::End) => {
                let frame = open
                    .pop()
                    .ok_or_else(|| Unsupported::new("end without a block"))?;
                if !frame {
                    continue;
                }
                depth -= 1;
            }
            _ => {}
        }
        d.items.extend(item);
    }
    d.items.extend((0..depth).map(|_| Item::End));
    if d.ids.is_empty() {
        return Err(Unsupported::new("empty sequence diagram"));
    }
    d.draw()
}

impl Diagram {
    /// The participant `id`, added if new.
    fn participant(&mut self, id: &str) -> Result<usize, Unsupported> {
        let id = id.trim();
        if id.is_empty() {
            return Err(Unsupported::new("expected a participant"));
        }
        if let Some(i) = self.ids.iter().position(|p| p == id) {
            return Ok(i);
        }
        if self.ids.len() == MAX_NODES {
            return Err(Unsupported::new(format!(
                "more than {MAX_NODES} participants"
            )));
        }
        self.ids.push(id.to_owned());
        self.labels.push(text::wrap(&text::clean(id)));
        Ok(self.ids.len() - 1)
    }

    fn statement(&mut self, line: &str) -> Result<Option<Item>, Unsupported> {
        let (word, rest) = match line.split_once(char::is_whitespace) {
            Some((word, rest)) => (word, rest.trim()),
            None => (line, ""),
        };
        let label = |s: &str| text::fit(&text::clean(s).replace('\n', " "), 60);
        match word {
            "participant" | "actor" => {
                let (id, alias) = match rest.split_once(" as ") {
                    Some((id, alias)) => (id.trim(), Some(alias.trim())),
                    None => (rest, None),
                };
                let i = self.participant(id)?;
                if let Some(alias) = alias {
                    self.labels[i] = text::wrap(&text::clean(alias));
                }
                Ok(None)
            }
            "autonumber" | "activate" | "deactivate" => Ok(None),
            "loop" | "alt" | "opt" | "par" | "critical" | "break" => {
                Ok(Some(Item::Open(format!("{word}: {}", label(rest)))))
            }
            "rect" => Ok(Some(Item::Open(String::new()))),
            "box" => Ok(Some(Item::Box)),
            "title" => {
                let title = text::clean(rest).replace('\n', " ");
                self.title = (!title.trim().is_empty()).then(|| title.trim().to_owned());
                Ok(None)
            }
            "else" | "and" | "option" => Ok(Some(Item::Split(format!("{word}: {}", label(rest))))),
            "end" if rest.is_empty() => Ok(Some(Item::End)),
            _ if word.eq_ignore_ascii_case("note") => self.note(rest).map(Some),
            _ => self.message(line).map(Some),
        }
    }

    /// `left of a: t`, `right of a: t`, `over a: t`, `over a,b: t`.
    fn note(&mut self, rest: &str) -> Result<Item, Unsupported> {
        let Some((place, body)) = rest.split_once(':') else {
            return Err(Unsupported::new("note without text"));
        };
        let place = place.trim();
        let (side, who) = if let Some(who) = place.strip_prefix("left of ") {
            (Side::Left, who)
        } else if let Some(who) = place.strip_prefix("right of ") {
            (Side::Right, who)
        } else if let Some(who) = place.strip_prefix("over ") {
            (Side::Over, who)
        } else {
            return Err(Unsupported::new("note needs left of, right of or over"));
        };
        let (a, b) = match who.split_once(',') {
            Some((a, b)) if matches!(side, Side::Over) => {
                (self.participant(a)?, self.participant(b)?)
            }
            Some(_) => return Err(Unsupported::new("only a note over may span")),
            None => {
                let a = self.participant(who)?;
                (a, a)
            }
        };
        Ok(Item::Note {
            side,
            a: a.min(b),
            b: a.max(b),
            text: text::wrap(&text::clean(body.trim())),
        })
    }

    /// `a->>b: text` and the other arrows.
    fn message(&mut self, line: &str) -> Result<Item, Unsupported> {
        let (head, body) = line.split_once(':').unwrap_or((line, ""));
        let found = (0..head.len())
            .filter(|&i| head.is_char_boundary(i))
            .find_map(|i| {
                ARROWS
                    .iter()
                    .find(|(p, _, _)| head[i..].starts_with(p))
                    .map(|a| (i, a))
            });
        let Some((at, &(pattern, dotted, arrowhead))) = found else {
            let shown = text::fit(&text::clean(line).replace('\n', " "), 40);
            return Err(Unsupported::new(format!("cannot parse: {shown}")));
        };
        let from = self.participant(&head[..at])?;
        let to = head[at + pattern.len()..]
            .trim()
            .trim_start_matches(['+', '-']);
        let to = self.participant(to)?;
        if self.messages == MAX_EDGES {
            return Err(Unsupported::new(format!("more than {MAX_EDGES} messages")));
        }
        self.messages += 1;
        Ok(Item::Message {
            from,
            to,
            text: text::wrap(&text::clean(body.trim())),
            dotted,
            head: arrowhead,
        })
    }

    fn draw(&self) -> Result<Vec<String>, Unsupported> {
        let n = self.ids.len();
        let widths: Vec<i64> = self
            .labels
            .iter()
            .map(|l| text::widest(l) as i64 + 4)
            .collect();
        let heights: Vec<i64> = self.labels.iter().map(|l| l.len() as i64 + 2).collect();
        // Positions 0 (left edge), 1..=n (lifelines), n + 1 (right edge);
        // constraints (a, b, d): position b is at least d right of a.
        let mut rules: Vec<(usize, usize, i64)> = Vec::new();
        rules.push((0, 1, widths[0] / 2));
        rules.push((n, n + 1, widths[n - 1] - widths[n - 1] / 2));
        for i in 1..n {
            let (wa, wb) = (widths[i - 1], widths[i]);
            rules.push((i, i + 1, wa - wa / 2 + 2 + wb / 2));
        }
        let mut depth = 0usize;
        let mut max_depth = 0usize;
        for item in &self.items {
            match item {
                Item::Message { from, to, text, .. } if from == to => {
                    rules.push((from + 1, from + 2, text::widest(text) as i64 + 6));
                }
                Item::Message { from, to, text, .. } => {
                    let (a, b) = (from.min(to) + 1, from.max(to) + 1);
                    rules.push((a, b, (text::widest(text) as i64 + 4).max(6)));
                }
                Item::Note { side, a, b, text } => {
                    let w = text::widest(text) as i64 + 4;
                    let (a, b) = (a + 1, b + 1);
                    match side {
                        Side::Right => rules.push((a, a + 1, w + 3)),
                        Side::Left => rules.push((a - 1, a, w + 3)),
                        Side::Over if a == b => {
                            rules.push((a - 1, a, w / 2 + 2));
                            rules.push((a, a + 1, w - w / 2 + 2));
                        }
                        Side::Over => {
                            rules.push((a - 1, a, 3));
                            rules.push((a, b, w - 5));
                            rules.push((b, b + 1, 3));
                        }
                    }
                }
                Item::Open(_) => {
                    depth += 1;
                    max_depth = max_depth.max(depth);
                }
                Item::End => depth -= 1,
                Item::Split(_) | Item::Box => {}
            }
        }
        let mut xs = vec![0i64; n + 2];
        for b in 1..n + 2 {
            xs[b] = xs[b - 1] + 1;
            for &(a, rb, d) in &rules {
                if rb == b {
                    xs[b] = xs[b].max(xs[a] + d);
                }
            }
        }
        // Frames take two columns each side per nesting level.
        let margin = 2 * max_depth as i64;
        let xs: Vec<i64> = xs.iter().map(|x| x + margin).collect();
        let width = xs[n + 1] + margin + 1;
        let head = heights.iter().copied().max().unwrap_or(3);
        let mut height = head;
        for item in &self.items {
            height += rows_of(item);
        }
        height += 1;
        let title_rows = i64::from(self.title.is_some());
        if width as usize > MAX_COLS || (height + title_rows) as usize > MAX_ROWS {
            return Err(Unsupported::new("diagram too large"));
        }
        let mut c = Canvas::new(width as usize, height as usize);
        let at = |i: usize| xs[i + 1] as usize;
        for i in 0..n {
            let x = at(i) - (widths[i] / 2) as usize;
            let (w, h) = (widths[i] as usize, heights[i] as usize);
            c.vline(at(i), h, height as usize - 1, (true, false), false);
            c.frame(x, 0, w, h, &canvas::RECT);
            for (k, line) in self.labels[i].iter().enumerate() {
                c.text(x + (w - text::width(line)) / 2, 1 + k, line);
            }
            c.put(at(i), h - 1, '┬');
        }
        let mut y = head as usize;
        let mut open: Vec<(usize, usize)> = Vec::new();
        for item in &self.items {
            match item {
                Item::Message {
                    from,
                    to,
                    text,
                    dotted,
                    head,
                } if from == to => {
                    let x = at(*from);
                    let rows = rows_of(item) as usize;
                    let (h, v) = if *dotted {
                        ('┄', '┆')
                    } else {
                        ('─', '│')
                    };
                    c.text(x, y, &format!("├{h}{h}┐"));
                    for k in 1..rows - 1 {
                        c.put(x + 3, y + k, v);
                    }
                    let back = if *head { '◀' } else { h };
                    c.text(x + 1, y + rows - 1, &format!("{back}{h}┘"));
                    for (k, line) in text.iter().enumerate() {
                        c.text(x + 5, y + k, line);
                    }
                    y += rows;
                }
                Item::Message {
                    from,
                    to,
                    text,
                    dotted,
                    head,
                } => {
                    let (xf, xt) = (at(*from), at(*to));
                    let (a, b) = (xf.min(xt), xf.max(xt));
                    for line in text {
                        let lw = text::width(line);
                        let x = ((a + b) / 2).saturating_sub(lw / 2).max(a + 2);
                        c.text(x, y, line);
                        y += 1;
                    }
                    let line = if *dotted { '┄' } else { '─' };
                    for x in a + 1..b {
                        c.put(x, y, line);
                    }
                    let right = xt > xf;
                    c.put(xf, y, if right { '├' } else { '┤' });
                    if *head {
                        let tip = if right {
                            (xt - 1, '▶')
                        } else {
                            (xt + 1, '◀')
                        };
                        c.put(tip.0, y, tip.1);
                    } else {
                        c.put(xt, y, if right { '┤' } else { '├' });
                    }
                    y += 1;
                }
                Item::Note { side, a, b, text } => {
                    let w = text::widest(text) + 4;
                    let x = match side {
                        Side::Right => at(*a) + 2,
                        Side::Left => at(*a) - 1 - w,
                        Side::Over if a == b => at(*a) - w / 2,
                        Side::Over => at(*a) - 2,
                    };
                    let w = match side {
                        Side::Over if a != b => w.max(at(*b) + 3 - x),
                        _ => w,
                    };
                    let h = text.len() + 2;
                    c.frame(x, y, w, h, &canvas::NOTE);
                    for (k, line) in text.iter().enumerate() {
                        c.text(x + 2, y + 1 + k, line);
                    }
                    y += h;
                }
                Item::Open(label) => {
                    let level = open.len();
                    let (l, r) = frame_columns(level, max_depth, width as usize);
                    self.rule(&mut c, y, l, r, ('┌', '─', '┐'), label, &xs);
                    open.push((level, y));
                    y += 1;
                }
                Item::Split(label) => {
                    let level = open.len() - 1;
                    let (l, r) = frame_columns(level, max_depth, width as usize);
                    self.rule(&mut c, y, l, r, ('├', '┄', '┤'), label, &xs);
                    y += 1;
                }
                Item::End => {
                    let (level, top) = open.pop().expect("blocks are balanced");
                    let (l, r) = frame_columns(level, max_depth, width as usize);
                    self.rule(&mut c, y, l, r, ('└', '─', '┘'), "", &xs);
                    for row in top + 1..y {
                        for x in [l, r] {
                            if c.get(x, row).is_none() {
                                c.put(x, row, '│');
                            }
                        }
                    }
                    y += 1;
                }
                Item::Box => {}
            }
        }
        let mut rows = c.rows();
        if let Some(title) = &self.title {
            // Never wider than the drawing itself.
            let width = rows.iter().map(|r| text::width(r)).max().unwrap_or(0);
            let title = text::fit(title, width);
            let pad = (width - text::width(&title)) / 2;
            rows.insert(0, format!("{}{title}", " ".repeat(pad)));
        }
        Ok(rows)
    }

    /// A frame's top, split or bottom row from column `l` to `r`, with
    /// `label` near its left end; lifelines cross it.
    #[allow(clippy::too_many_arguments)]
    fn rule(
        &self,
        c: &mut Canvas,
        y: usize,
        l: usize,
        r: usize,
        (left, line, right): (char, char, char),
        label: &str,
        xs: &[i64],
    ) {
        c.put(l, y, left);
        for x in l + 1..r {
            c.put(x, y, line);
        }
        c.put(r, y, right);
        for &x in &xs[1..xs.len() - 1] {
            c.put(x as usize, y, '┼');
        }
        if !label.is_empty() {
            let room = r.saturating_sub(l + 4);
            c.text(
                l + 2,
                y,
                &format!(" {} ", text::fit(label, room.saturating_sub(2))),
            );
        }
    }
}

/// The rows `item` takes.
fn rows_of(item: &Item) -> i64 {
    match item {
        Item::Message { from, to, text, .. } if from == to => text.len().max(1) as i64 + 2,
        Item::Message { text, .. } => text.len() as i64 + 1,
        Item::Note { text, .. } => text.len() as i64 + 2,
        Item::Open(_) | Item::Split(_) | Item::End => 1,
        Item::Box => 0,
    }
}

/// The left and right columns of a frame at nesting `level` (0 = outer).
fn frame_columns(level: usize, max_depth: usize, width: usize) -> (usize, usize) {
    let inset = 2 * level;
    debug_assert!(level < max_depth);
    (inset, width - 1 - inset)
}
