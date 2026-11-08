//! Flowcharts (`graph`/`flowchart`): parsing, a layered layout and drawing.
//!
//! The layout is the classic layered one: cycles are broken by reversing
//! back edges, nodes go to layers by longest path, long edges get dummy
//! nodes, barycenter sweeps order each layer, and an isotonic fit places
//! nodes as close to their neighbours as their widths allow. Edges are
//! orthogonal: down from the source, along a track between the layers,
//! down into the target. Everything is laid out on an abstract (cross,
//! main) grid and mapped to TD, BT, LR or RL only when drawn.

use std::collections::HashMap;

use super::canvas::{self, Canvas, DOWN, LEFT, RIGHT, UP};
use super::text;
use super::{Unsupported, MAX_COLS, MAX_EDGES, MAX_NODES, MAX_ROWS};

#[derive(Clone, Copy, PartialEq, Debug)]
enum Dir {
    Td,
    Bt,
    Lr,
    Rl,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Shape {
    Rect,
    Rounded,
    Decision,
}

struct Node {
    label: Vec<String>,
    shape: Shape,
}

#[derive(Clone)]
struct Edge {
    from: usize,
    to: usize,
    label: Vec<String>,
    dotted: bool,
    head: bool,
}

#[derive(Default)]
struct Graph {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    index: HashMap<String, usize>,
}

/// The drawing of a flowchart whose header was `graph <rest>` (or
/// `flowchart <rest>`), with `lines` its statements.
pub fn render<'a>(
    rest: &'a str,
    lines: impl Iterator<Item = &'a str>,
) -> Result<Vec<String>, Unsupported> {
    let (dir, first) = match rest.split_once(';') {
        Some((dir, first)) => (dir.trim(), first),
        None => (rest.trim(), ""),
    };
    let dir = match dir {
        "" | "TD" | "TB" => Dir::Td,
        "BT" => Dir::Bt,
        "LR" => Dir::Lr,
        "RL" => Dir::Rl,
        other => {
            let other: String = other.chars().take(24).collect();
            return Err(Unsupported::new(format!("unsupported direction: {other}")));
        }
    };
    let mut graph = Graph::default();
    for line in std::iter::once(first).chain(lines) {
        for statement in split_statements(line) {
            graph.statement(statement)?;
        }
    }
    if graph.nodes.is_empty() {
        return Err(Unsupported::new("empty flowchart"));
    }
    Layout::new(&graph, dir).draw(&graph)
}

/// `line` split on the `;` that are not inside quotes, brackets or `|…|`.
fn split_statements(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut quote, mut pipe, mut start) = (0usize, false, false, 0);
    for (i, c) in line.char_indices() {
        match c {
            '"' => quote = !quote,
            '[' | '(' | '{' if !quote => depth += 1,
            ']' | ')' | '}' if !quote => depth = depth.saturating_sub(1),
            '|' if !quote && depth == 0 => pipe = !pipe,
            ';' if !quote && !pipe && depth == 0 => {
                out.push(&line[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&line[start..]);
    out
}

// ── Parsing ──────────────────────────────────────────────────────────────

/// A link between two node groups.
struct Link {
    dotted: bool,
    head: bool,
    both: bool,
    invisible: bool,
    label: Option<String>,
}

/// Words that start a statement that draws nothing.
const IGNORED: [&str; 8] = [
    "subgraph",
    "end",
    "direction",
    "style",
    "classDef",
    "class",
    "click",
    "linkStyle",
];

impl Graph {
    fn statement(&mut self, s: &str) -> Result<(), Unsupported> {
        let s = s.trim();
        let word = s.split(|c: char| c.is_whitespace()).next().unwrap_or("");
        if s.is_empty() || IGNORED.contains(&word) {
            return Ok(());
        }
        let chars: Vec<char> = s.chars().collect();
        let mut p = Parser { s: &chars, i: 0 };
        let mut left = self.group(&mut p)?;
        loop {
            p.skip_ws();
            if p.done() {
                return Ok(());
            }
            let Some(mut link) = p.link() else {
                return Err(Unsupported::new(format!("cannot parse: {}", fit_reason(s))));
            };
            p.skip_ws();
            if p.peek() == Some('|') {
                link.label = Some(p.pipe_label()?);
                p.skip_ws();
            }
            let right = self.group(&mut p)?;
            let label = text::wrap(&text::clean(link.label.as_deref().unwrap_or("")));
            for &a in &left {
                for &b in &right {
                    if link.invisible {
                        continue;
                    }
                    self.edge(a, b, label.clone(), &link)?;
                    if link.both {
                        self.edge(b, a, Vec::new(), &link)?;
                    }
                }
            }
            left = right;
        }
    }

    /// Nodes joined by `&`.
    fn group(&mut self, p: &mut Parser) -> Result<Vec<usize>, Unsupported> {
        let mut ids = Vec::new();
        loop {
            p.skip_ws();
            ids.push(self.node(p)?);
            p.skip_ws();
            if p.peek() != Some('&') {
                return Ok(ids);
            }
            p.i += 1;
        }
    }

    /// A node reference, with or without its shape and text.
    fn node(&mut self, p: &mut Parser) -> Result<usize, Unsupported> {
        let id = p.ident();
        if id.is_empty() {
            return Err(Unsupported::new("expected a node"));
        }
        let shape = p.shape()?;
        p.class_suffix();
        let index = match self.index.get(&id) {
            Some(&i) => i,
            None => {
                if self.nodes.len() == MAX_NODES {
                    return Err(Unsupported::new(format!("more than {MAX_NODES} nodes")));
                }
                self.nodes.push(Node {
                    label: text::wrap(&text::clean(&id)),
                    shape: Shape::Rect,
                });
                self.index.insert(id, self.nodes.len() - 1);
                self.nodes.len() - 1
            }
        };
        if let Some((shape, label)) = shape {
            let node = &mut self.nodes[index];
            node.shape = shape;
            node.label = text::wrap(&text::clean(&label));
            if node.label.is_empty() {
                node.label = vec![String::new()];
            }
        }
        Ok(index)
    }

    fn edge(
        &mut self,
        from: usize,
        to: usize,
        label: Vec<String>,
        link: &Link,
    ) -> Result<(), Unsupported> {
        if self.edges.len() == MAX_EDGES {
            return Err(Unsupported::new(format!("more than {MAX_EDGES} edges")));
        }
        self.edges.push(Edge {
            from,
            to,
            label,
            dotted: link.dotted,
            head: link.head,
        });
        Ok(())
    }
}

/// A statement, shortened for an error message.
fn fit_reason(s: &str) -> String {
    text::fit(&text::clean(s).replace('\n', " "), 40)
}

struct Parser<'a> {
    s: &'a [char],
    i: usize,
}

/// Node shapes: opening and closing delimiters. Longer openings first.
const SHAPES: [(&str, &str, Shape); 12] = [
    ("([", "])", Shape::Rounded),
    ("((", "))", Shape::Rounded),
    ("[[", "]]", Shape::Rect),
    ("[(", ")]", Shape::Rect),
    ("{{", "}}", Shape::Rect),
    ("[/", "/]", Shape::Rect),
    ("[\\", "\\]", Shape::Rect),
    ("[/", "\\]", Shape::Rect),
    ("[", "]", Shape::Rect),
    ("(", ")", Shape::Rounded),
    ("{", "}", Shape::Decision),
    (">", "]", Shape::Rect),
];

impl Parser<'_> {
    fn done(&self) -> bool {
        self.i >= self.s.len()
    }

    fn peek(&self) -> Option<char> {
        self.s.get(self.i).copied()
    }

    fn at(&self, i: usize) -> Option<char> {
        self.s.get(i).copied()
    }

    fn starts_with(&self, i: usize, pat: &str) -> bool {
        for (j, c) in (i..).zip(pat.chars()) {
            if self.at(j) != Some(c) {
                return false;
            }
        }
        true
    }

    fn skip_ws(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.i += 1;
        }
    }

    /// A node id: letters, digits, `_`, and `-` between two of those.
    fn ident(&mut self) -> String {
        let start = self.i;
        while let Some(c) = self.peek() {
            let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
            if word(Some(c)) || (c == '-' && self.i > start && word(self.at(self.i + 1))) {
                self.i += 1;
            } else {
                break;
            }
        }
        self.s[start..self.i].iter().collect()
    }

    /// A shape and its text, if one follows.
    fn shape(&mut self) -> Result<Option<(Shape, String)>, Unsupported> {
        let Some(&(open, _, shape)) = SHAPES.iter().find(|(o, _, _)| self.starts_with(self.i, o))
        else {
            return Ok(None);
        };
        let start = self.i + open.chars().count();
        let closers: Vec<&str> = SHAPES
            .iter()
            .filter(|(o, _, _)| *o == open)
            .map(|(_, c, _)| *c)
            .collect();
        let mut j = start;
        // A quoted text may contain the closing delimiter.
        if self.at(j) == Some('"') {
            j += 1;
            while self.at(j).is_some_and(|c| c != '"') {
                j += 1;
            }
            j += 1;
        }
        while j < self.s.len() {
            if let Some(close) = closers.iter().find(|c| self.starts_with(j, c)) {
                let raw: String = self.s[start..j].iter().collect();
                let raw = raw.trim();
                let label = raw
                    .strip_prefix('"')
                    .and_then(|r| r.strip_suffix('"'))
                    .unwrap_or(raw);
                self.i = j + close.chars().count();
                return Ok(Some((shape, label.to_owned())));
            }
            j += 1;
        }
        Err(Unsupported::new("unclosed node shape"))
    }

    /// Skip a `:::class` suffix.
    fn class_suffix(&mut self) {
        if self.starts_with(self.i, ":::") {
            self.i += 3;
            self.ident();
        }
    }

    /// A link (`-->`, `-.->`, `== t ==>`, …), if one starts here.
    fn link(&mut self) -> Option<Link> {
        let start = self.i;
        let both = self.peek() == Some('<');
        if both {
            self.i += 1;
        }
        let run = self.run();
        if run.chars().count() < 2 {
            self.i = start;
            return None;
        }
        let tip = self.head();
        let mut head = both || tip;
        let mut dotted = run.contains('.');
        let mut label = None;
        if !head && matches!(run.as_str(), "--" | "==" | "-.") && self.peek() == Some(' ') {
            // `-- text -->`: the text runs to the closing link.
            let close = self.closer(&run)?;
            label = Some(
                self.s[self.i..close]
                    .iter()
                    .collect::<String>()
                    .trim()
                    .to_owned(),
            );
            self.i = close;
            let end = self.run();
            dotted |= end.contains('.');
            head = self.head();
        }
        Some(Link {
            dotted,
            head,
            both,
            invisible: run.contains('~'),
            label,
        })
    }

    /// A run of link characters.
    fn run(&mut self) -> String {
        let start = self.i;
        while self
            .peek()
            .is_some_and(|c| matches!(c, '-' | '.' | '=' | '~'))
        {
            self.i += 1;
        }
        self.s[start..self.i].iter().collect()
    }

    /// An arrowhead: `>`, or `o`/`x` before a space, `|` or the end.
    fn head(&mut self) -> bool {
        match self.peek() {
            Some('>') => {
                self.i += 1;
                true
            }
            Some('o' | 'x')
                if self
                    .at(self.i + 1)
                    .is_none_or(|c| c.is_whitespace() || c == '|') =>
            {
                self.i += 1;
                true
            }
            _ => false,
        }
    }

    /// Where the link that closes a `-- text` opening starts.
    fn closer(&self, open: &str) -> Option<usize> {
        let ends: &[&str] = match open {
            "--" => &["-->", "---", "--o ", "--x "],
            "==" => &["==>", "==="],
            _ => &[".->", ".-"],
        };
        (self.i..self.s.len()).find(|&j| ends.iter().any(|e| self.starts_with(j, e)))
    }

    /// `|text|`.
    fn pipe_label(&mut self) -> Result<String, Unsupported> {
        let start = self.i + 1;
        let Some(len) = self.s[start..].iter().position(|&c| c == '|') else {
            return Err(Unsupported::new("unclosed edge label"));
        };
        self.i = start + len + 1;
        let raw: String = self.s[start..start + len].iter().collect();
        let raw = raw.trim();
        Ok(raw
            .strip_prefix('"')
            .and_then(|r| r.strip_suffix('"'))
            .unwrap_or(raw)
            .to_owned())
    }
}

// ── Layout ───────────────────────────────────────────────────────────────

/// A node of the layered graph: a real node or a dummy on a long edge.
#[derive(Clone, Copy, PartialEq)]
enum LNode {
    Real(usize),
    Dummy,
}

/// Segments leaving one port: `(port, leftmost x, rightmost x, segment indices)`.
type Net = ((usize, i64), i64, i64, Vec<usize>);

/// One layer-to-next-layer piece of an edge.
struct Seg {
    from: usize,
    to: usize,
    edge: usize,
    first: bool,
    last: bool,
}

/// Where an edge's label is drawn.
#[derive(Clone, Copy, PartialEq)]
enum Place {
    /// Beside the segment's end, above its target.
    Target(usize),
    /// Beside the segment's start, below its source.
    Source(usize),
}

struct Layout {
    dir: Dir,
    edges: Vec<Edge>,
    /// Kept edges drawn upwards (cycles broken).
    reversed: Vec<bool>,
    lnodes: Vec<LNode>,
    layer_of: Vec<usize>,
    layers: Vec<Vec<usize>>,
    segs: Vec<Seg>,
    /// Per lnode: the slot offset of each segment's port, out (main+) side
    /// and in (main−) side, by segment index.
    out_slot: HashMap<usize, i64>,
    in_slot: HashMap<usize, i64>,
    /// Per lnode: cross size, main size, and the cross extent used for
    /// spacing (labels included).
    cross: Vec<i64>,
    main: Vec<i64>,
    extent: Vec<i64>,
    labels: HashMap<usize, Place>,
    /// Edges listed under the drawing instead (index into `edges`), and
    /// whether only their label is missing from the drawing.
    unplaced: Vec<usize>,
}

impl Layout {
    fn new(graph: &Graph, dir: Dir) -> Layout {
        let n = graph.nodes.len();
        // Self-loops and repeats cannot be drawn: they are listed.
        let mut edges = Vec::new();
        let mut unplaced_edges = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for e in &graph.edges {
            if e.from == e.to || !seen.insert((e.from, e.to)) {
                unplaced_edges.push(e.clone());
            } else {
                edges.push(e.clone());
            }
        }
        let reversed = break_cycles(n, &edges);
        let layer_of_real = assign_layers(n, &edges, &reversed);
        let mut layout = Layout {
            dir,
            edges,
            reversed,
            lnodes: (0..n).map(LNode::Real).collect(),
            layer_of: layer_of_real,
            layers: Vec::new(),
            segs: Vec::new(),
            out_slot: HashMap::new(),
            in_slot: HashMap::new(),
            cross: Vec::new(),
            main: Vec::new(),
            extent: Vec::new(),
            labels: HashMap::new(),
            unplaced: Vec::new(),
        };
        // Listed edges go after the kept ones, so their indices stay apart.
        for e in unplaced_edges {
            layout.unplaced.push(layout.edges.len());
            layout.reversed.push(false);
            layout.edges.push(e);
        }
        layout.split_long_edges();
        layout.order();
        layout.ports(graph);
        layout
    }

    /// The number of edges that are drawn (the rest are listed).
    fn drawn_edges(&self) -> usize {
        self.edges.len() - self.unplaced.len()
    }

    /// Segments for every drawn edge, with dummy nodes on long ones.
    fn split_long_edges(&mut self) {
        for e in 0..self.drawn_edges() {
            let (mut a, b) = if self.reversed[e] {
                (self.edges[e].to, self.edges[e].from)
            } else {
                (self.edges[e].from, self.edges[e].to)
            };
            let (la, lb) = (self.layer_of[a], self.layer_of[b]);
            for layer in la + 1..=lb {
                let next = if layer == lb {
                    b
                } else {
                    self.lnodes.push(LNode::Dummy);
                    self.layer_of.push(layer);
                    self.lnodes.len() - 1
                };
                self.segs.push(Seg {
                    from: a,
                    to: next,
                    edge: e,
                    first: layer == la + 1,
                    last: layer == lb,
                });
                a = next;
            }
        }
        let count = self.layer_of.iter().max().map_or(0, |&l| l + 1);
        self.layers = vec![Vec::new(); count];
        for (v, &l) in self.layer_of.iter().enumerate() {
            self.layers[l].push(v);
        }
    }

    /// Order each layer by barycenter sweeps, keeping the order with the
    /// fewest crossings.
    fn order(&mut self) {
        let mut preds = vec![Vec::new(); self.lnodes.len()];
        let mut succs = vec![Vec::new(); self.lnodes.len()];
        for s in &self.segs {
            preds[s.to].push(s.from);
            succs[s.from].push(s.to);
        }
        let mut best = self.layers.clone();
        let mut best_crossings = self.crossings(&best);
        let mut layers = self.layers.clone();
        for sweep in 0..8 {
            let down = sweep % 2 == 0;
            let range: Vec<usize> = if down {
                (1..layers.len()).collect()
            } else {
                (0..layers.len().saturating_sub(1)).rev().collect()
            };
            for l in range {
                let reference = if down { l - 1 } else { l + 1 };
                let pos = positions(&layers[reference]);
                let neighbours = if down { &preds } else { &succs };
                let mut keyed: Vec<(f64, usize, usize)> = layers[l]
                    .iter()
                    .enumerate()
                    .map(|(i, &v)| {
                        let ps: Vec<f64> = neighbours[v]
                            .iter()
                            .filter_map(|u| pos.get(u).map(|&p| p as f64))
                            .collect();
                        let key = if ps.is_empty() {
                            i as f64
                        } else {
                            ps.iter().sum::<f64>() / ps.len() as f64
                        };
                        (key, i, v)
                    })
                    .collect();
                keyed.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
                layers[l] = keyed.into_iter().map(|(_, _, v)| v).collect();
            }
            let crossings = self.crossings(&layers);
            if crossings < best_crossings {
                best_crossings = crossings;
                best = layers.clone();
            }
        }
        self.layers = best;
    }

    /// Edge crossings between adjacent layers in `layers`.
    fn crossings(&self, layers: &[Vec<usize>]) -> usize {
        let mut pos = vec![0usize; self.lnodes.len()];
        for layer in layers {
            for (i, &v) in layer.iter().enumerate() {
                pos[v] = i;
            }
        }
        let mut by_gap: Vec<Vec<(usize, usize)>> = vec![Vec::new(); layers.len()];
        for s in &self.segs {
            by_gap[self.layer_of[s.from]].push((pos[s.from], pos[s.to]));
        }
        let mut count = 0;
        for pairs in &by_gap {
            for (i, a) in pairs.iter().enumerate() {
                for b in &pairs[i + 1..] {
                    if (a.0 < b.0 && a.1 > b.1) || (a.0 > b.0 && a.1 < b.1) {
                        count += 1;
                    }
                }
            }
        }
        count
    }

    /// Port slots, node sizes and label places.
    fn ports(&mut self, graph: &Graph) {
        let lr = matches!(self.dir, Dir::Lr | Dir::Rl);
        let count = self.lnodes.len();
        // Per lnode and side: the slots in use. Slot 0 is shared by the
        // forward edges; each reversed edge gets its own.
        let mut out_groups: Vec<usize> = vec![0; count];
        let mut in_groups: Vec<usize> = vec![0; count];
        let mut out_shared = vec![false; count];
        let mut in_shared = vec![false; count];
        for s in &self.segs {
            let reversed = self.reversed[s.edge];
            let real_from = matches!(self.lnodes[s.from], LNode::Real(_));
            let real_to = matches!(self.lnodes[s.to], LNode::Real(_));
            if !reversed || !real_from {
                out_shared[s.from] = true;
            }
            if !reversed || !real_to {
                in_shared[s.to] = true;
            }
        }
        for (i, s) in self.segs.iter().enumerate() {
            let reversed = self.reversed[s.edge];
            let real_from = matches!(self.lnodes[s.from], LNode::Real(_));
            let real_to = matches!(self.lnodes[s.to], LNode::Real(_));
            let out = if reversed && real_from {
                let k = out_groups[s.from] + usize::from(out_shared[s.from]);
                out_groups[s.from] += 1;
                slot_offset(k, lr)
            } else {
                0
            };
            let inn = if reversed && real_to {
                let k = in_groups[s.to] + usize::from(in_shared[s.to]);
                in_groups[s.to] += 1;
                slot_offset(k, lr)
            } else {
                0
            };
            self.out_slot.insert(i, out);
            self.in_slot.insert(i, inn);
        }
        // Sizes. Cross is along the layer, main across layers.
        for (v, lnode) in self.lnodes.iter().enumerate() {
            let (cross, main) = match *lnode {
                LNode::Dummy => (1, 0),
                LNode::Real(n) => {
                    let label = &graph.nodes[n].label;
                    let slots = out_groups[v] + usize::from(out_shared[v]);
                    let slots = slots.max(in_groups[v] + usize::from(in_shared[v]));
                    let need = 2 * slot_offset(slots.saturating_sub(1), lr) + 3;
                    let w = (text::widest(label) as i64 + 4).max(5);
                    let h = label.len() as i64 + 2;
                    if lr {
                        (h.max(need), w)
                    } else {
                        (w.max(need), h)
                    }
                }
            };
            self.cross.push(cross);
            self.main.push(main);
        }
        self.extent = self.cross.clone();
        // Labels: beside a port only that edge uses, else listed.
        let mut ins: HashMap<(usize, i64), usize> = HashMap::new();
        let mut outs: HashMap<(usize, i64), usize> = HashMap::new();
        for (i, s) in self.segs.iter().enumerate() {
            *ins.entry((s.to, self.in_slot[&i])).or_default() += 1;
            *outs.entry((s.from, self.out_slot[&i])).or_default() += 1;
        }
        for (i, s) in self.segs.iter().enumerate() {
            let edge = &self.edges[s.edge];
            if edge.label.is_empty() {
                continue;
            }
            let place = if s.last && ins[&(s.to, self.in_slot[&i])] == 1 {
                Place::Target(i)
            } else if s.first && outs[&(s.from, self.out_slot[&i])] == 1 {
                Place::Source(i)
            } else {
                continue;
            };
            if self.labels.contains_key(&s.edge) {
                continue;
            }
            let (node, _) = match place {
                Place::Target(_) => (s.to, ()),
                Place::Source(_) => (s.from, ()),
            };
            let (cross, _) = self.label_size(&edge.label);
            self.extent[node] = self.extent[node].max(cross + 2);
            self.labels.insert(s.edge, place);
        }
        for e in 0..self.drawn_edges() {
            if !self.edges[e].label.is_empty() && !self.labels.contains_key(&e) {
                self.unplaced.push(e);
            }
        }
    }

    /// A label's (cross, main) size beside a line.
    fn label_size(&self, label: &[String]) -> (i64, i64) {
        let w = text::widest(label) as i64;
        let n = label.len() as i64;
        match self.dir {
            Dir::Td | Dir::Bt => (w, n + 1),
            Dir::Lr | Dir::Rl => (2 * n - 1, w + 4),
        }
    }

    /// Cross positions (centers) of every lnode.
    fn place_cross(&self) -> Vec<i64> {
        let mut preds = vec![Vec::new(); self.lnodes.len()];
        let mut succs = vec![Vec::new(); self.lnodes.len()];
        for s in &self.segs {
            preds[s.to].push(s.from);
            succs[s.from].push(s.to);
        }
        let seps: Vec<Vec<f64>> = self
            .layers
            .iter()
            .map(|layer| {
                layer
                    .windows(2)
                    .map(|w| self.sep(w[0], w[1]) as f64)
                    .collect()
            })
            .collect();
        let mut x = vec![0f64; self.lnodes.len()];
        for (layer, seps) in self.layers.iter().zip(&seps) {
            let mut at = 0.0;
            for (i, &v) in layer.iter().enumerate() {
                if i > 0 {
                    at += seps[i - 1];
                }
                x[v] = at;
            }
        }
        for pass in 0..8 {
            let down = pass % 2 == 0;
            let order: Vec<usize> = if down {
                (1..self.layers.len()).collect()
            } else {
                (0..self.layers.len().saturating_sub(1)).rev().collect()
            };
            for l in order {
                let layer = &self.layers[l];
                let neighbours = if down { &preds } else { &succs };
                // Real nodes pull harder than dummies, so boxes line up
                // and long edges bend around them.
                let desired: Vec<f64> = layer
                    .iter()
                    .map(|&v| {
                        let ns = &neighbours[v];
                        let total: f64 = ns.iter().map(|&u| self.weight(u)).sum();
                        if ns.is_empty() {
                            x[v]
                        } else {
                            ns.iter().map(|&u| self.weight(u) * x[u]).sum::<f64>() / total
                        }
                    })
                    .collect();
                let weights: Vec<f64> = layer.iter().map(|&v| self.weight(v)).collect();
                let placed = isotonic(&desired, &weights, &seps[l]);
                for (&v, p) in layer.iter().zip(placed) {
                    x[v] = p;
                }
            }
        }
        // Whole cells, still apart, and nothing left of column 0.
        let mut out = vec![0i64; self.lnodes.len()];
        for (layer, seps) in self.layers.iter().zip(&seps) {
            for (i, &v) in layer.iter().enumerate() {
                let mut p = x[v].round() as i64;
                if i > 0 {
                    p = p.max(out[layer[i - 1]] + seps[i - 1] as i64);
                }
                out[v] = p;
            }
        }
        let min = (0..self.lnodes.len())
            .map(|v| out[v] - self.extent[v] / 2)
            .min()
            .unwrap_or(0);
        out.iter().map(|p| p - min).collect()
    }

    /// How hard `v` pulls in placement: dummies give way to real nodes.
    fn weight(&self, v: usize) -> f64 {
        match self.lnodes[v] {
            LNode::Real(_) => 1.0,
            LNode::Dummy => 0.1,
        }
    }

    /// The least distance between the centers of neighbours `a` and `b`.
    fn sep(&self, a: usize, b: usize) -> i64 {
        let real = |v: usize| matches!(self.lnodes[v], LNode::Real(_));
        let lr = matches!(self.dir, Dir::Lr | Dir::Rl);
        let gap = if real(a) && real(b) && !lr { 3 } else { 2 };
        let hi = self.extent[a] - 1 - self.extent[a] / 2;
        let lo = self.extent[b] / 2;
        hi + gap + lo
    }

    // ── Drawing ──────────────────────────────────────────────────────────

    fn draw(&self, graph: &Graph) -> Result<Vec<String>, Unsupported> {
        let cx = self.place_cross();
        // A reversed edge's slots lie on the side towards its other end at
        // its start, and away from it at its end: then, on the top track,
        // it crosses neither the shared stub nor the shared drop.
        let mut ports: Vec<(i64, i64)> = (self.segs.iter().enumerate())
            .map(|(i, s)| {
                let sign = if cx[s.to] >= cx[s.from] { 1 } else { -1 };
                (sign * self.out_slot[&i], sign * self.in_slot[&i])
            })
            .collect();
        // Unless the other side crosses fewer of the gap's other lines. A
        // reversed edge has the top track: its horizontal crosses the other
        // stubs below its span, and its drop crosses the other horizontals.
        for (i, s) in self.segs.iter().enumerate() {
            if self.out_slot[&i] == 0 && self.in_slot[&i] == 0 {
                continue;
            }
            let g = self.layer_of[s.from];
            let others: Vec<(i64, i64)> = (self.segs.iter().enumerate())
                .filter(|&(j, o)| j != i && self.layer_of[o.from] == g)
                .map(|(j, o)| (cx[o.from] + ports[j].0, cx[o.to] + ports[j].1))
                .collect();
            let crossed = |(a, b): (i64, i64)| {
                let (sx, tx) = (cx[s.from] + a, cx[s.to] + b);
                let (lo, hi) = (sx.min(tx), sx.max(tx));
                let stubs = others.iter().filter(|o| lo <= o.0 && o.0 <= hi).count();
                let drops = (others.iter())
                    .filter(|o| o.0.min(o.1) <= tx && tx <= o.0.max(o.1))
                    .count();
                stubs + drops
            };
            let (a, b) = (self.out_slot[&i], self.in_slot[&i]);
            let best = [ports[i], (a, b), (-a, b), (a, -b), (-a, -b)]
                .into_iter()
                .min_by_key(|&p| crossed(p))
                .expect("candidates");
            ports[i] = best;
        }
        let gaps = self.layers.len().saturating_sub(1);
        // Per gap: the track of each segment, and the band sizes.
        let mut track = vec![0i64; self.segs.len()];
        let mut tracks = vec![1i64; gaps];
        let mut src_band = vec![0i64; gaps];
        let mut dst_band = vec![0i64; gaps];
        for (g, gap_tracks) in tracks.iter_mut().enumerate() {
            // Nets: segments that leave the same port share a track.
            let mut nets: Vec<Net> = Vec::new();
            for (i, s) in self.segs.iter().enumerate() {
                if self.layer_of[s.from] != g {
                    continue;
                }
                let port = (s.from, ports[i].0);
                let sx = cx[s.from] + ports[i].0;
                let tx = cx[s.to] + ports[i].1;
                match nets.iter_mut().find(|n| n.0 == port) {
                    Some(net) => {
                        net.1 = net.1.min(tx);
                        net.2 = net.2.max(tx);
                        net.3.push(i);
                    }
                    None => nets.push((port, sx.min(tx), sx.max(tx), vec![i])),
                }
            }
            // Reversed edges first, so they get the top tracks.
            let reversed = |n: &Net| n.3.iter().any(|&i| self.reversed[self.segs[i].edge]);
            nets.sort_by_key(|n| (!reversed(n), n.1, n.2));
            let mut ends: Vec<i64> = Vec::new();
            for (_, lo, hi, members) in &nets {
                if lo == hi {
                    continue;
                }
                let t = match ends.iter().position(|&end| end + 2 <= *lo) {
                    Some(t) => t,
                    None => {
                        ends.push(i64::MIN);
                        ends.len() - 1
                    }
                };
                ends[t] = *hi;
                for &i in members {
                    track[i] = t as i64;
                }
            }
            *gap_tracks = (ends.len() as i64).max(1);
        }
        for (&e, place) in &self.labels {
            let (_, main) = self.label_size(&self.edges[e].label);
            match *place {
                Place::Target(i) => {
                    let g = self.layer_of[self.segs[i].from];
                    dst_band[g] = dst_band[g].max(main);
                }
                Place::Source(i) => {
                    let g = self.layer_of[self.segs[i].from];
                    src_band[g] = src_band[g].max(main);
                }
            }
        }
        // A `▲` under a box needs a row of line below it before a turn.
        for s in &self.segs {
            if self.reversed[s.edge] && self.edges[s.edge].head && s.first {
                let g = self.layer_of[s.from];
                src_band[g] = src_band[g].max(1);
            }
        }
        // Main positions: layer starts and the rows of each gap.
        let mut layer_start = Vec::with_capacity(self.layers.len());
        let mut layer_size = Vec::with_capacity(self.layers.len());
        let mut m = 0i64;
        for (l, layer) in self.layers.iter().enumerate() {
            layer_start.push(m);
            let size = layer
                .iter()
                .map(|&v| self.main[v])
                .max()
                .unwrap_or(1)
                .max(1);
            layer_size.push(size);
            m += size;
            if l < gaps {
                m += src_band[l] + tracks[l] + dst_band[l] + 1;
            }
        }
        let total_main = m;
        let total_cross = (0..self.lnodes.len())
            .map(|v| cx[v] - self.extent[v] / 2 + self.extent[v])
            .max()
            .unwrap_or(1);
        let listed = self.listing(graph);
        let (w, h) = match self.dir {
            Dir::Td | Dir::Bt => (total_cross, total_main),
            Dir::Lr | Dir::Rl => (total_main, total_cross),
        };
        let listed_rows = if listed.is_empty() {
            0
        } else {
            listed.len() + 1
        };
        if w as usize > MAX_COLS || h as usize + listed_rows > MAX_ROWS {
            return Err(Unsupported::new("diagram too large"));
        }
        let mut pen = Pen {
            dir: self.dir,
            total_main,
            canvas: Canvas::new(w as usize, h as usize),
        };
        // Lines first, so boxes and text are drawn over them.
        for (i, s) in self.segs.iter().enumerate() {
            let g = self.layer_of[s.from];
            let edge = &self.edges[s.edge];
            let sx = cx[s.from] + ports[i].0;
            let tx = cx[s.to] + ports[i].1;
            let start = match self.lnodes[s.from] {
                LNode::Real(_) => layer_start[g] + self.main[s.from],
                LNode::Dummy => layer_start[g],
            };
            let gap = layer_start[g] + layer_size[g];
            let mt = gap + src_band[g] + track[i];
            let arrow = layer_start[g + 1] - 1;
            pen.vline(sx, start, mt, (true, false), edge.dotted);
            if sx != tx {
                pen.hline(mt, sx, tx, edge.dotted);
            }
            pen.vline(tx, mt, arrow, (false, true), edge.dotted);
            if let LNode::Dummy = self.lnodes[s.to] {
                let l = g + 1;
                let end = layer_start[l] + layer_size[l] - 1;
                pen.vline(tx, layer_start[l], end, (true, true), edge.dotted);
            }
        }
        // Boxes.
        for (v, lnode) in self.lnodes.iter().enumerate() {
            let LNode::Real(n) = *lnode else { continue };
            let node = &graph.nodes[n];
            let l = self.layer_of[v];
            let c0 = cx[v] - self.cross[v] / 2;
            let (x, y, w, h) = pen.rect(c0, layer_start[l], self.cross[v], self.main[v]);
            let style = match node.shape {
                Shape::Rect => &canvas::RECT,
                Shape::Rounded => &canvas::ROUNDED,
                Shape::Decision => &canvas::DECISION,
            };
            pen.canvas.frame(x, y, w, h, style);
            let top = y + (h - node.label.len()) / 2;
            for (i, line) in node.label.iter().enumerate() {
                let lw = text::width(line);
                pen.canvas.text(x + (w - lw) / 2, top + i, line);
            }
        }
        // Ports and arrowheads.
        for (i, s) in self.segs.iter().enumerate() {
            let g = self.layer_of[s.from];
            let edge = &self.edges[s.edge];
            let reversed = self.reversed[s.edge];
            let sx = cx[s.from] + ports[i].0;
            let tx = cx[s.to] + ports[i].1;
            if let LNode::Real(_) = self.lnodes[s.from] {
                let border = layer_start[g] + self.main[s.from] - 1;
                if reversed && edge.head {
                    pen.arrow(sx, border + 1, false);
                } else {
                    pen.tee(sx, border, true);
                }
            }
            if let LNode::Real(_) = self.lnodes[s.to] {
                let top = layer_start[g + 1];
                if !reversed && edge.head {
                    pen.arrow(tx, top - 1, true);
                } else if !pen.is_arrow(tx, top - 1) {
                    pen.tee(tx, top, false);
                }
            }
        }
        // Labels, over the lines they sit on.
        for (&e, place) in &self.labels {
            let label = &self.edges[e].label;
            let (_, size) = self.label_size(label);
            let (at, main0) = match *place {
                Place::Target(i) => {
                    let s = &self.segs[i];
                    let arrow = layer_start[self.layer_of[s.to]] - 1;
                    (cx[s.to] + ports[i].1, arrow - size)
                }
                Place::Source(i) => {
                    let s = &self.segs[i];
                    let g = self.layer_of[s.from];
                    let start = match self.lnodes[s.from] {
                        LNode::Real(_) => layer_start[g] + self.main[s.from],
                        LNode::Dummy => layer_start[g],
                    };
                    (cx[s.from] + ports[i].0, start)
                }
            };
            pen.label(label, at, main0, size);
        }
        let mut rows = pen.canvas.rows();
        if !listed.is_empty() {
            rows.push(String::new());
            rows.extend(listed);
        }
        Ok(rows)
    }

    /// The `a ──label──▶ b` lines for the edges not drawn.
    fn listing(&self, graph: &Graph) -> Vec<String> {
        let name = |n: usize| text::fit(&graph.nodes[n].label.join(" "), text::WRAP);
        let mut seen = std::collections::HashSet::new();
        self.unplaced
            .iter()
            .filter(|&&e| seen.insert(e))
            .map(|&e| {
                let edge = &self.edges[e];
                let line = if edge.dotted { "┄┄" } else { "──" };
                let head = if edge.head { "▶" } else { "" };
                let label = edge.label.join(" ");
                let mid = if label.is_empty() {
                    format!("{line}{head}")
                } else {
                    format!("{line}{label}{line}{head}")
                };
                text::fit(
                    &format!("{} {mid} {}", name(edge.from), name(edge.to)),
                    MAX_COLS,
                )
            })
            .collect()
    }
}

/// Which edges to reverse so that the graph has no cycle: the back edges
/// of a depth-first search, run with an explicit stack.
fn break_cycles(n: usize, edges: &[Edge]) -> Vec<bool> {
    let mut out: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, e) in edges.iter().enumerate() {
        out[e.from].push(i);
    }
    // 0: not seen, 1: on the stack, 2: done.
    let mut state = vec![0u8; n];
    let mut reversed = vec![false; edges.len()];
    for root in 0..n {
        if state[root] != 0 {
            continue;
        }
        state[root] = 1;
        let mut stack = vec![(root, 0usize)];
        while let Some(top) = stack.last_mut() {
            let (v, k) = *top;
            if k < out[v].len() {
                top.1 += 1;
                let e = out[v][k];
                let w = edges[e].to;
                match state[w] {
                    0 => {
                        state[w] = 1;
                        stack.push((w, 0));
                    }
                    1 => reversed[e] = true,
                    _ => {}
                }
            } else {
                state[v] = 2;
                stack.pop();
            }
        }
    }
    reversed
}

/// Each node's layer: the longest path to it from a source, with the
/// edges in `reversed` turned around; then a source moves down to just
/// above its nearest successor, so it does not dangle far above it.
fn assign_layers(n: usize, edges: &[Edge], reversed: &[bool]) -> Vec<usize> {
    let ends = |i: usize| {
        let e = &edges[i];
        if reversed[i] {
            (e.to, e.from)
        } else {
            (e.from, e.to)
        }
    };
    let mut indegree = vec![0usize; n];
    let mut out: Vec<Vec<usize>> = vec![Vec::new(); n];
    for i in 0..edges.len() {
        let (a, b) = ends(i);
        indegree[b] += 1;
        out[a].push(b);
    }
    let mut layer = vec![0usize; n];
    let mut ready: std::collections::VecDeque<usize> =
        (0..n).filter(|&v| indegree[v] == 0).collect();
    while let Some(v) = ready.pop_front() {
        for &w in &out[v] {
            layer[w] = layer[w].max(layer[v] + 1);
            indegree[w] -= 1;
            if indegree[w] == 0 {
                ready.push_back(w);
            }
        }
    }
    let sources: Vec<usize> = (0..n)
        .filter(|&v| !out[v].is_empty() && (0..edges.len()).all(|i| ends(i).1 != v))
        .collect();
    for v in sources {
        if let Some(nearest) = out[v].iter().map(|&w| layer[w]).min() {
            layer[v] = nearest - 1;
        }
    }
    layer
}

/// How far the k-th port slot of a side is from its center (the side it
/// lies on is chosen when drawing): two columns apart, or one row.
fn slot_offset(k: usize, lr: bool) -> i64 {
    if lr {
        k as i64
    } else {
        2 * k as i64
    }
}

/// Each lnode's index in `layer`.
fn positions(layer: &[usize]) -> HashMap<usize, usize> {
    layer.iter().enumerate().map(|(i, &v)| (v, i)).collect()
}

/// Positions as close as possible (weighted least squares) to `desired`,
/// in order, with at least `seps[i]` between the i-th and the next. Pool
/// adjacent violators: iterative, linear.
fn isotonic(desired: &[f64], weights: &[f64], seps: &[f64]) -> Vec<f64> {
    // Shift out the separations: then the positions only need to grow.
    let mut offset = vec![0.0; desired.len()];
    for i in 1..desired.len() {
        offset[i] = offset[i - 1] + seps[i - 1];
    }
    // Blocks of (weighted sum, total weight, members).
    let mut blocks: Vec<(f64, f64, usize)> = Vec::new();
    for (i, d) in desired.iter().enumerate() {
        blocks.push((weights[i] * (d - offset[i]), weights[i], 1));
        while blocks.len() > 1 {
            let (s1, c1, n1) = blocks[blocks.len() - 1];
            let (s0, c0, n0) = blocks[blocks.len() - 2];
            if s0 / c0 <= s1 / c1 {
                break;
            }
            blocks.pop();
            *blocks.last_mut().expect("two blocks") = (s0 + s1, c0 + c1, n0 + n1);
        }
    }
    let mut out = Vec::with_capacity(desired.len());
    for (sum, count, n) in blocks {
        for _ in 0..n {
            out.push(sum / count + offset[out.len()]);
        }
    }
    out
}

/// Draws on the canvas in abstract (cross, main) coordinates.
struct Pen {
    dir: Dir,
    total_main: i64,
    canvas: Canvas,
}

impl Pen {
    /// The canvas cell of (cross, main).
    fn at(&self, c: i64, m: i64) -> (usize, usize) {
        let flip = self.total_main - 1 - m;
        let (x, y) = match self.dir {
            Dir::Td => (c, m),
            Dir::Bt => (c, flip),
            Dir::Lr => (m, c),
            Dir::Rl => (flip, c),
        };
        (x.max(0) as usize, y.max(0) as usize)
    }

    /// The canvas rectangle (x, y, w, h) of a cross × main rectangle.
    fn rect(&self, c0: i64, m0: i64, cross: i64, main: i64) -> (usize, usize, usize, usize) {
        let (x0, y0) = self.at(c0, m0);
        let (x1, y1) = self.at(c0 + cross - 1, m0 + main - 1);
        (
            x0.min(x1),
            y0.min(y1),
            x0.abs_diff(x1) + 1,
            y0.abs_diff(y1) + 1,
        )
    }

    /// Canvas links for main+ and main−.
    fn main_links(&self) -> (u8, u8) {
        match self.dir {
            Dir::Td => (DOWN, UP),
            Dir::Bt => (UP, DOWN),
            Dir::Lr => (RIGHT, LEFT),
            Dir::Rl => (LEFT, RIGHT),
        }
    }

    fn cross_links(&self) -> (u8, u8) {
        match self.dir {
            Dir::Td | Dir::Bt => (RIGHT, LEFT),
            Dir::Lr | Dir::Rl => (DOWN, UP),
        }
    }

    fn vline(&mut self, c: i64, m0: i64, m1: i64, open: (bool, bool), dotted: bool) {
        let (plus, minus) = self.main_links();
        for m in m0..=m1 {
            let mut l = 0;
            if m > m0 || open.0 {
                l |= minus;
            }
            if m < m1 || open.1 {
                l |= plus;
            }
            let (x, y) = self.at(c, m);
            self.canvas.link(x, y, l, dotted);
        }
    }

    fn hline(&mut self, m: i64, c0: i64, c1: i64, dotted: bool) {
        let (plus, minus) = self.cross_links();
        let (lo, hi) = (c0.min(c1), c0.max(c1));
        for c in lo..=hi {
            let mut l = 0;
            if c > lo {
                l |= minus;
            }
            if c < hi {
                l |= plus;
            }
            let (x, y) = self.at(c, m);
            self.canvas.link(x, y, l, dotted);
        }
    }

    /// An arrowhead at (c, m) pointing main+ (`forward`) or main−.
    fn arrow(&mut self, c: i64, m: i64, forward: bool) {
        let heads = match self.dir {
            Dir::Td => ('▼', '▲'),
            Dir::Bt => ('▲', '▼'),
            Dir::Lr => ('▶', '◀'),
            Dir::Rl => ('◀', '▶'),
        };
        let (x, y) = self.at(c, m);
        self.canvas
            .put(x, y, if forward { heads.0 } else { heads.1 });
    }

    fn is_arrow(&self, c: i64, m: i64) -> bool {
        let (x, y) = self.at(c, m);
        matches!(self.canvas.get(x, y), Some('▼' | '▲' | '▶' | '◀'))
    }

    /// A box border cell where a line leaves (`out`, main+) or enters.
    fn tee(&mut self, c: i64, m: i64, out: bool) {
        let (x, y) = self.at(c, m);
        let tee = match (self.dir, out) {
            (Dir::Td, true) | (Dir::Bt, false) => '┬',
            (Dir::Td, false) | (Dir::Bt, true) => '┴',
            (Dir::Lr, true) | (Dir::Rl, false) => '├',
            (Dir::Lr, false) | (Dir::Rl, true) => '┤',
        };
        if matches!(self.canvas.get(x, y), Some('─' | '│')) {
            self.canvas.put(x, y, tee);
        }
    }

    /// An edge label centered on the line at cross `c`, filling the main
    /// range from `m0` over `size` cells.
    fn label(&mut self, label: &[String], c: i64, m0: i64, size: i64) {
        let n = label.len() as i64;
        match self.dir {
            Dir::Td | Dir::Bt => {
                let w = text::widest(label) as i64;
                // The last `n` rows of the band; the first stays a line.
                let (x, y, _, _) = self.rect(c - w / 2, m0 + size - n, w, n);
                for (i, line) in label.iter().enumerate() {
                    let lw = text::width(line) as i64;
                    self.canvas.text(x + ((w - lw) / 2) as usize, y + i, line);
                }
            }
            Dir::Lr | Dir::Rl => {
                let w = text::widest(label) as i64;
                // " text " after one cell of line.
                let (x, y, _, _) = self.rect(c - (n - 1) / 2, m0 + 1, n, w + 2);
                for (i, line) in label.iter().enumerate() {
                    let lw = text::width(line) as i64;
                    let pad = ((w - lw) / 2) as usize;
                    let row = format!(
                        "{}{line}{}",
                        " ".repeat(pad + 1),
                        " ".repeat((w - lw) as usize - pad + 1)
                    );
                    self.canvas.text(x, y + i, &row);
                }
            }
        }
    }
}
