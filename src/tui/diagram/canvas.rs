//! A grid of terminal cells that lines, boxes and text are drawn on.
//!
//! Lines are recorded as connections (up, down, left, right) per cell and
//! turned into box-drawing characters at the end, so two lines that meet
//! merge into one junction (`┬`, `┴`, `├`, `┼`…). Text and box borders are
//! written as characters and win over lines.

use unicode_width::UnicodeWidthChar;

pub const UP: u8 = 1;
pub const DOWN: u8 = 2;
pub const LEFT: u8 = 4;
pub const RIGHT: u8 = 8;

/// Marks the second column of a wide character.
const WIDE_TAIL: char = '\0';

pub struct Canvas {
    width: usize,
    height: usize,
    chars: Vec<Option<char>>,
    links: Vec<u8>,
    /// Per cell: some solid line passes (else, with links, only dotted ones).
    solid: Vec<bool>,
}

/// The four corners and the two sides of a box.
pub struct BoxStyle {
    pub top_left: char,
    pub top_right: char,
    pub bottom_left: char,
    pub bottom_right: char,
    pub horizontal: char,
    pub vertical: char,
}

pub const RECT: BoxStyle = BoxStyle {
    top_left: '┌',
    top_right: '┐',
    bottom_left: '└',
    bottom_right: '┘',
    horizontal: '─',
    vertical: '│',
};
pub const ROUNDED: BoxStyle = BoxStyle {
    top_left: '╭',
    top_right: '╮',
    bottom_left: '╰',
    bottom_right: '╯',
    horizontal: '─',
    vertical: '│',
};
/// A decision: slanted corners.
pub const DECISION: BoxStyle = BoxStyle {
    top_left: '╱',
    top_right: '╲',
    bottom_left: '╲',
    bottom_right: '╱',
    horizontal: '─',
    vertical: '│',
};
/// A sequence diagram's note: dashed sides.
pub const NOTE: BoxStyle = BoxStyle {
    top_left: '┌',
    top_right: '┐',
    bottom_left: '└',
    bottom_right: '┘',
    horizontal: '┄',
    vertical: '┆',
};

impl Canvas {
    /// A blank canvas. The caller has checked the size against the caps.
    pub fn new(width: usize, height: usize) -> Canvas {
        let cells = width * height;
        Canvas {
            width,
            height,
            chars: vec![None; cells],
            links: vec![0; cells],
            solid: vec![false; cells],
        }
    }

    fn index(&self, x: usize, y: usize) -> Option<usize> {
        (x < self.width && y < self.height).then(|| y * self.width + x)
    }

    /// Add `links` to cell (x, y); `dotted` lines draw dashed when no solid
    /// line shares the cell.
    pub fn link(&mut self, x: usize, y: usize, links: u8, dotted: bool) {
        if let Some(i) = self.index(x, y) {
            self.links[i] |= links;
            self.solid[i] |= !dotted;
        }
    }

    /// A vertical line from row `y0` to `y1` (inclusive) at column `x`. Its
    /// end cells link only inwards, unless `open` asks them to link onwards
    /// too (`(top, bottom)`).
    pub fn vline(&mut self, x: usize, y0: usize, y1: usize, open: (bool, bool), dotted: bool) {
        let (top, bottom) = (y0.min(y1), y0.max(y1));
        for y in top..=bottom {
            let mut l = 0;
            if y > top || open.0 {
                l |= UP;
            }
            if y < bottom || open.1 {
                l |= DOWN;
            }
            self.link(x, y, l, dotted);
        }
    }

    /// Write `c` at (x, y), over any line.
    pub fn put(&mut self, x: usize, y: usize, c: char) {
        if let Some(i) = self.index(x, y) {
            self.chars[i] = Some(c);
        }
    }

    /// Write `s` from (x, y) rightwards; a wide character takes two cells.
    pub fn text(&mut self, x: usize, y: usize, s: &str) {
        let mut x = x;
        for c in s.chars() {
            let w = c.width().unwrap_or(0);
            if w == 0 {
                continue;
            }
            self.put(x, y, c);
            if w == 2 {
                self.put(x + 1, y, WIDE_TAIL);
            }
            x += w;
        }
    }

    /// A box with its top-left corner at (x, y), `w`×`h` cells, its inside
    /// cleared of lines.
    pub fn frame(&mut self, x: usize, y: usize, w: usize, h: usize, style: &BoxStyle) {
        for dy in 0..h {
            for dx in 0..w {
                let c = match (dx, dy) {
                    (0, 0) => style.top_left,
                    (dx, 0) if dx == w - 1 => style.top_right,
                    (0, dy) if dy == h - 1 => style.bottom_left,
                    (dx, dy) if dx == w - 1 && dy == h - 1 => style.bottom_right,
                    (_, 0) => style.horizontal,
                    (_, dy) if dy == h - 1 => style.horizontal,
                    (0, _) => style.vertical,
                    (dx, _) if dx == w - 1 => style.vertical,
                    _ => ' ',
                };
                self.put(x + dx, y + dy, c);
            }
        }
    }

    /// The character at (x, y), if one was written.
    pub fn get(&self, x: usize, y: usize) -> Option<char> {
        self.index(x, y).and_then(|i| self.chars[i])
    }

    /// The finished rows, trailing spaces trimmed.
    pub fn rows(&self) -> Vec<String> {
        (0..self.height)
            .map(|y| {
                let mut row = String::with_capacity(self.width);
                for x in 0..self.width {
                    let i = y * self.width + x;
                    match self.chars[i] {
                        Some(WIDE_TAIL) => {}
                        Some(c) => row.push(c),
                        None => row.push(glyph(self.links[i], self.solid[i])),
                    }
                }
                row.trim_end().to_owned()
            })
            .collect()
    }
}

/// The box-drawing character for a cell's links.
fn glyph(links: u8, solid: bool) -> char {
    match links {
        0 => ' ',
        l if l & (LEFT | RIGHT) == 0 => {
            if solid {
                '│'
            } else {
                '┆'
            }
        }
        l if l & (UP | DOWN) == 0 => {
            if solid {
                '─'
            } else {
                '┄'
            }
        }
        l if l == DOWN | RIGHT => '┌',
        l if l == DOWN | LEFT => '┐',
        l if l == UP | RIGHT => '└',
        l if l == UP | LEFT => '┘',
        l if l == UP | DOWN | RIGHT => '├',
        l if l == UP | DOWN | LEFT => '┤',
        l if l == DOWN | LEFT | RIGHT => '┬',
        l if l == UP | LEFT | RIGHT => '┴',
        _ => '┼',
    }
}
