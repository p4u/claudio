//! The session pane: a `term::screen::Screen` drawn into a ratatui buffer.
//! The terminal model is the core's; drawing it is the TUI's (P4 plan D7).

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::{Color, Rgb};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color as RColor, Modifier, Style};

use crate::term::screen::{named_index, Screen};

/// Draw `screen` into a ratatui `Buffer`, clipped to `area`.
///
/// Does not panic on any area or size mismatch — cells outside the
/// intersection of `area` and the screen grid are simply not written.
pub(crate) fn render(screen: &Screen, area: Rect, buf: &mut Buffer) {
    let grid = screen.grid();
    let n_rows = grid.screen_lines();
    let n_cols = grid.columns();
    if n_rows == 0 || n_cols == 0 {
        return;
    }

    for rel_row in 0..area.height {
        if rel_row as usize >= n_rows {
            break;
        }
        let line_idx = Line(rel_row as i32 - grid.display_offset() as i32);

        for rel_col in 0..area.width {
            if rel_col as usize >= n_cols {
                break;
            }
            let cell = &grid[line_idx][Column(rel_col as usize)];

            // Skip wide-char spacer cells; the wide char itself (at the
            // preceding column) already paints the character into the
            // ratatui buffer cell, and the spacer column is left blank so
            // that the buffer width accounting stays correct.
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }

            let bx = area.x + rel_col;
            let by = area.y + rel_row;

            let mut style = Style::default()
                .fg(alac_to_ratatui(cell.fg))
                .bg(alac_to_ratatui(cell.bg));
            if cell.flags.contains(Flags::BOLD) {
                style = style.add_modifier(Modifier::BOLD);
            }
            if cell.flags.contains(Flags::DIM) {
                style = style.add_modifier(Modifier::DIM);
            }
            if cell.flags.contains(Flags::ITALIC) {
                style = style.add_modifier(Modifier::ITALIC);
            }
            if cell.flags.intersects(Flags::ALL_UNDERLINES) {
                style = style.add_modifier(Modifier::UNDERLINED);
            }
            if cell.flags.contains(Flags::INVERSE) {
                style = style.add_modifier(Modifier::REVERSED);
            }
            if cell.flags.contains(Flags::HIDDEN) {
                style = style.add_modifier(Modifier::HIDDEN);
            }
            if cell.flags.contains(Flags::STRIKEOUT) {
                style = style.add_modifier(Modifier::CROSSED_OUT);
            }

            if let Some(bc) = buf.cell_mut((bx, by)) {
                let ch = if cell.c == '\0' { ' ' } else { cell.c };
                bc.set_char(ch);
                bc.set_style(style);
            }
        }
    }
}

/// Map an alacritty color to the closest ratatui color.
///
/// `NamedColor::Foreground`, `Background`, `Cursor`, and the `Dim*`/`Bright*`
/// semantic variants fall back to `Color::Reset` (terminal default), which
/// causes ratatui to use the terminal's own default.
fn alac_to_ratatui(c: Color) -> RColor {
    match c {
        Color::Named(nc) => named_index(nc).map_or(RColor::Reset, RColor::Indexed),
        Color::Indexed(i) => RColor::Indexed(i),
        Color::Spec(Rgb { r, g, b }) => RColor::Rgb(r, g, b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wide characters and emoji occupy two columns; the spacer cell must
    /// render as a blank (not a second copy of the char) and the ratatui
    /// buffer must contain the right characters.
    #[test]
    fn wide_chars_render() {
        // 'Ａ' is a full-width ASCII variant (U+FF21), unicode width = 2.
        // '🦀' (crab emoji) is also width-2.
        let input = b"\x1b[2J\x1b[H\xef\xbc\x81\xf0\x9f\xa6\x80";
        let mut s = Screen::new(4, 20);
        s.feed(input);

        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 4));
        render(&s, Rect::new(0, 0, 20, 4), &mut buf);

        // Column 0: '！' (full-width exclamation, U+FF01)
        // Column 2: '🦀'
        // Columns 1 and 3 are spacer cells — rendered as space (blank).
        let cell0 = buf.cell((0, 0)).unwrap();
        let cell1 = buf.cell((1, 0)).unwrap();
        let cell2 = buf.cell((2, 0)).unwrap();
        let cell3 = buf.cell((3, 0)).unwrap();

        assert_eq!(
            cell0.symbol(),
            "\u{FF01}",
            "expected full-width ！ at col 0"
        );
        assert_eq!(cell1.symbol(), " ", "spacer at col 1 should be blank");
        assert_eq!(cell2.symbol(), "🦀", "expected crab at col 2");
        assert_eq!(cell3.symbol(), " ", "spacer at col 3 should be blank");
    }
}
