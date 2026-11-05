//! Mermaid diagrams drawn with box-drawing characters, for the reader view's
//! ` ```mermaid ` fence lens.
//!
//! Pure functions only. The input is claude's text, so it is hostile: every
//! size is capped before anything is allocated for it, nothing recurses on
//! the input's depth, and [`render`] still runs only on the render worker
//! behind its guard (pre-check, scratch thread, budget), never on the UI
//! thread. Two kinds are drawn: flowcharts ([`flowchart`]) and sequence
//! diagrams ([`sequence`]); anything else is [`Unsupported`] and the reader
//! keeps the code block.

mod canvas;
mod flowchart;
mod sequence;
mod text;

/// The most bytes a diagram's source may have ([`too_complex`]).
pub const MAX_SOURCE_BYTES: usize = 4096;
/// The most lines a diagram's source may have ([`too_complex`]).
pub const MAX_SOURCE_LINES: usize = 200;
/// The most nodes of a flowchart, or participants of a sequence diagram.
pub const MAX_NODES: usize = 64;
/// The most edges of a flowchart, or messages of a sequence diagram.
pub const MAX_EDGES: usize = 128;
/// The widest drawing, in terminal columns.
pub const MAX_COLS: usize = 200;
/// The tallest drawing, in rows.
pub const MAX_ROWS: usize = 150;

/// Why a diagram is not drawn; the reader shows its code block instead.
#[derive(Debug, Clone, PartialEq)]
pub struct Unsupported {
    pub reason: String,
}

impl Unsupported {
    fn new(reason: impl Into<String>) -> Unsupported {
        Unsupported {
            reason: reason.into(),
        }
    }
}

/// Whether `src` is too big to try: more than [`MAX_SOURCE_BYTES`] bytes or
/// [`MAX_SOURCE_LINES`] lines. Cheap; no parsing.
pub fn too_complex(src: &str) -> bool {
    src.len() > MAX_SOURCE_BYTES || src.lines().nth(MAX_SOURCE_LINES).is_some()
}

/// The rows of the drawing of `src`, a ` ```mermaid ` fence's content
/// (without the fence lines). Every row is at most [`MAX_COLS`] columns,
/// free of control characters and trimmed at the end; there are at most
/// [`MAX_ROWS`] rows. Edges that cannot be drawn are listed under the
/// drawing as `a ──label──▶ b`.
pub fn render(src: &str) -> Result<Vec<String>, Unsupported> {
    if too_complex(src) {
        return Err(Unsupported::new("diagram source too large"));
    }
    let mut lines = statements(src);
    let Some(header) = lines.next() else {
        return Err(Unsupported::new("empty diagram"));
    };
    let (kind, rest) = match header.split_once(char::is_whitespace) {
        Some((kind, rest)) => (kind, rest.trim()),
        None => (header, ""),
    };
    let rows = match kind.trim_end_matches(';') {
        "graph" | "flowchart" => flowchart::render(rest, lines)?,
        "sequenceDiagram" if rest.is_empty() => sequence::render(lines)?,
        other => {
            let other: String = other.chars().take(24).collect();
            return Err(Unsupported::new(format!("unsupported diagram: {other}")));
        }
    };
    debug_assert!(rows.len() <= MAX_ROWS);
    Ok(rows)
}

/// The meaningful lines of `src`, trimmed: no blank lines, no `%%`
/// comments or directives, and no `---` front matter block at the start.
fn statements(src: &str) -> impl Iterator<Item = &str> {
    let mut front_matter = false;
    let mut first = true;
    src.lines().map(str::trim).filter(move |line| {
        if first && *line == "---" {
            front_matter = true;
            first = false;
            return false;
        }
        if front_matter {
            front_matter = *line != "---";
            return false;
        }
        if line.is_empty() || line.starts_with("%%") {
            return false;
        }
        first = false;
        true
    })
}

#[cfg(test)]
mod tests;
