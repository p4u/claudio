//! Terminal input encoding.
//!
//! Converts crossterm events into the raw byte sequences a child PTY expects,
//! honouring the child's current `Modes`. Encoding follows the xterm legacy
//! (VT100/ECMA-48) scheme; Kitty Keyboard Protocol is not emitted — the child
//! has not negotiated it through the `Screen`.

use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use super::screen::{Modes, MouseMode};

// ── Key encoding ──────────────────────────────────────────────────────────────

/// Encode a crossterm `KeyEvent` into the bytes expected by the child PTY.
///
/// Key-release events (`KeyEventKind::Release`) are ignored and return an
/// empty `Vec`: the child only ever sees presses and repeats.
///
/// Application-keypad mode (`modes.app_keypad`) is not encoded here because
/// crossterm does not report numpad events distinctly from regular number
/// keys; the flag is exposed in `Modes` for completeness.
pub fn encode_key(ev: &KeyEvent, modes: &Modes) -> Vec<u8> {
    // Releases carry no meaning in VT semantics.
    if ev.kind == KeyEventKind::Release {
        return Vec::new();
    }

    let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
    let alt = ev.modifiers.contains(KeyModifiers::ALT);
    let shift = ev.modifiers.contains(KeyModifiers::SHIFT);

    // xterm modifier parameter: 1 + shift(1) + alt(2) + ctrl(4).
    let modp: u8 = 1 + u8::from(shift) + 2 * u8::from(alt) + 4 * u8::from(ctrl);
    let has_mod = modp > 1;

    let mut out: Vec<u8> = Vec::new();

    match ev.code {
        // ── Printable characters ──────────────────────────────────────────
        KeyCode::Char(mut c) => {
            if ctrl {
                // Ctrl+letter → 0x01–0x1a; Ctrl+@/Space → NUL;
                // Ctrl+[/\/]/^/_ → ESC/FS/GS/RS/US.
                let ctrl_byte = if c.is_ascii_alphabetic() {
                    (c.to_ascii_lowercase() as u8) - b'a' + 1
                } else {
                    match c {
                        ' ' | '@' => 0x00,
                        '[' => 0x1b,
                        '\\' => 0x1c,
                        ']' => 0x1d,
                        '^' => 0x1e,
                        '_' => 0x1f,
                        // Non-ASCII characters have no defined Ctrl+<ch>
                        // encoding; emitting `c as u8` would truncate the
                        // codepoint into arbitrary bytes. Ignore silently.
                        _ => {
                            if !c.is_ascii() {
                                return out;
                            }
                            c as u8
                        }
                    }
                };
                if alt {
                    out.push(0x1b);
                }
                out.push(ctrl_byte);
                return out;
            }
            if alt {
                out.push(0x1b);
            }
            // Shift is already reflected in the character crossterm gives us,
            // but capitalise bare ASCII letters just in case.
            if shift && c.is_ascii_lowercase() {
                c = c.to_ascii_uppercase();
            }
            let mut buf = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        }

        // ── Special keys ──────────────────────────────────────────────────
        KeyCode::Enter => {
            if alt {
                out.push(0x1b);
            }
            out.push(b'\r');
        }
        KeyCode::Backspace => {
            if alt {
                out.push(0x1b);
            }
            out.push(0x7f);
        }
        KeyCode::Tab => {
            if alt {
                out.push(0x1b);
            }
            out.push(b'\t');
        }
        KeyCode::BackTab => {
            // Shift+Tab is always \x1b[Z regardless of DECCKM.
            out.extend_from_slice(b"\x1b[Z");
        }
        KeyCode::Esc => {
            out.push(0x1b);
        }

        // ── Arrow keys (DECCKM-aware) ─────────────────────────────────────
        //
        // With modifiers, xterm uses CSI 1 ; {modp} A/B/C/D regardless of
        // DECCKM. Without modifiers, DECCKM selects SS3 (O) vs CSI ([) prefix.
        KeyCode::Up => arrow_key(&mut out, b'A', modp, has_mod, modes.app_cursor),
        KeyCode::Down => arrow_key(&mut out, b'B', modp, has_mod, modes.app_cursor),
        KeyCode::Right => arrow_key(&mut out, b'C', modp, has_mod, modes.app_cursor),
        KeyCode::Left => arrow_key(&mut out, b'D', modp, has_mod, modes.app_cursor),

        // ── Home / End ────────────────────────────────────────────────────
        KeyCode::Home => {
            if has_mod {
                push_csi_mod(&mut out, "1", modp, b'H');
            } else {
                out.extend_from_slice(b"\x1b[H");
            }
        }
        KeyCode::End => {
            if has_mod {
                push_csi_mod(&mut out, "1", modp, b'F');
            } else {
                out.extend_from_slice(b"\x1b[F");
            }
        }

        // ── Page Up / Page Down / Insert / Delete ─────────────────────────
        KeyCode::PageUp => tilde_key(&mut out, 5, modp, has_mod),
        KeyCode::PageDown => tilde_key(&mut out, 6, modp, has_mod),
        KeyCode::Insert => tilde_key(&mut out, 2, modp, has_mod),
        KeyCode::Delete => tilde_key(&mut out, 3, modp, has_mod),

        // ── Function keys F1–F12 ──────────────────────────────────────────
        KeyCode::F(n) => encode_fkey(&mut out, n, modp, has_mod),

        _ => {} // Unhandled codes produce no output.
    }

    out
}

// ── Paste encoding ────────────────────────────────────────────────────────────

/// Encode a paste string for delivery to the child PTY.
///
/// If `modes.bracketed_paste` is set the text is wrapped in the bracketed-paste
/// delimiters `\x1b[200~` … `\x1b[201~`; otherwise the bytes are sent verbatim.
pub fn encode_paste(text: &str, modes: &Modes) -> Vec<u8> {
    if modes.bracketed_paste {
        let mut out = Vec::with_capacity(text.len() + 12);
        out.extend_from_slice(b"\x1b[200~");
        out.extend_from_slice(text.as_bytes());
        out.extend_from_slice(b"\x1b[201~");
        out
    } else {
        text.as_bytes().to_vec()
    }
}

// ── Mouse encoding ────────────────────────────────────────────────────────────

/// Encode a mouse event for the child PTY.
///
/// Returns `None` when:
/// - Mouse reporting is off (`modes.mouse == Off`).
/// - The event kind is not reportable at the current `MouseMode` level.
/// - The event position is outside the pane rectangle `(origin, size)`.
///
/// `origin` is the top-left corner of the pane in outer-terminal coordinates.
/// `size` is `(cols, rows)`. Coordinates are translated to pane-relative
/// 1-based values before encoding.
pub fn encode_mouse(
    ev: &MouseEvent,
    origin: (u16, u16),
    size: (u16, u16),
    modes: &Modes,
) -> Option<Vec<u8>> {
    if modes.mouse == MouseMode::Off {
        return None;
    }

    // Bounds check: is the event inside the pane?
    let (ox, oy) = origin;
    let (sw, sh) = size;
    let col = ev.column.checked_sub(ox)?;
    let row = ev.row.checked_sub(oy)?;
    if col >= sw || row >= sh {
        return None;
    }

    let (btn_code, is_release) = classify_mouse(ev, modes)?;
    let mods_bits = (u8::from(ev.modifiers.contains(KeyModifiers::SHIFT)) * 4)
        | (u8::from(ev.modifiers.contains(KeyModifiers::ALT)) * 8)
        | (u8::from(ev.modifiers.contains(KeyModifiers::CONTROL)) * 16);

    let cb = btn_code | mods_bits;
    // Protocol columns/rows are 1-based.
    let cx = col + 1;
    let cy = row + 1;

    if modes.mouse_sgr {
        // SGR 1006: \x1b[<{cb};{cx};{cy}M (press/drag) or m (release).
        let final_char = if is_release { b'm' } else { b'M' };
        let mut out = format!("\x1b[<{cb};{cx};{cy}").into_bytes();
        out.push(final_char);
        Some(out)
    } else {
        // X10/normal encoding: \x1b[M{cb+32}{cx+32}{cy+32}
        // Values are clamped so cx/cy ≤ 223 (0xFF - 0x20 = 223) to avoid
        // emitting bytes that look like UTF-8 lead bytes.
        let cx_b = (cx.min(223) as u8).wrapping_add(32);
        let cy_b = (cy.min(223) as u8).wrapping_add(32);
        let cb_b = cb.wrapping_add(32);
        Some(vec![0x1b, b'[', b'M', cb_b, cx_b, cy_b])
    }
}

/// The xterm button code and release flag for a mouse event, or `None` when
/// the child's mouse mode does not report this kind of event. Callers have
/// already checked that reporting is on, so clicks and the wheel always pass.
fn classify_mouse(ev: &MouseEvent, modes: &Modes) -> Option<(u8, bool)> {
    match ev.kind {
        MouseEventKind::Down(btn) => Some((mouse_button_code(btn), false)),
        // Legacy X10 encoding cannot say which button was released.
        MouseEventKind::Up(btn) => Some((
            if modes.mouse_sgr {
                mouse_button_code(btn)
            } else {
                3
            },
            true,
        )),
        // Motion with a button held: button code | 32.
        MouseEventKind::Drag(btn) if modes.mouse != MouseMode::Click => {
            Some((mouse_button_code(btn) | 0x20, false))
        }
        // Motion with no button: code 35.
        MouseEventKind::Moved if modes.mouse == MouseMode::Motion => Some((35, false)),
        // Wheel: up = button 4 (64), down = button 5 (65), left/right = 66/67.
        MouseEventKind::ScrollUp => Some((64, false)),
        MouseEventKind::ScrollDown => Some((65, false)),
        MouseEventKind::ScrollLeft => Some((66, false)),
        MouseEventKind::ScrollRight => Some((67, false)),
        _ => None,
    }
}

fn mouse_button_code(btn: MouseButton) -> u8 {
    match btn {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    }
}

// ── Focus encoding ────────────────────────────────────────────────────────────

/// Encode a focus-in or focus-out event.
///
/// Returns `None` when `modes.focus_reporting` is `false`.
pub fn encode_focus(gained: bool, modes: &Modes) -> Option<Vec<u8>> {
    if !modes.focus_reporting {
        return None;
    }
    if gained {
        Some(b"\x1b[I".to_vec())
    } else {
        Some(b"\x1b[O".to_vec())
    }
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Arrow key encoding: `\x1bO{letter}` (DECCKM, no mod), `\x1b[{letter}` (no
/// DECCKM, no mod), or `\x1b[1;{modp}{letter}` (any modifier).
fn arrow_key(out: &mut Vec<u8>, letter: u8, modp: u8, has_mod: bool, app_cursor: bool) {
    if has_mod {
        // Modifiers override DECCKM.
        out.extend_from_slice(b"\x1b[1;");
        out.extend_from_slice(modp.to_string().as_bytes());
        out.push(letter);
    } else if app_cursor {
        out.extend_from_slice(b"\x1bO");
        out.push(letter);
    } else {
        out.extend_from_slice(b"\x1b[");
        out.push(letter);
    }
}

/// Tilde-form keys (Insert=2, Delete=3, PageUp=5, PageDown=6).
fn tilde_key(out: &mut Vec<u8>, num: u8, modp: u8, has_mod: bool) {
    if has_mod {
        out.extend_from_slice(format!("\x1b[{num};{modp}~").as_bytes());
    } else {
        out.extend_from_slice(format!("\x1b[{num}~").as_bytes());
    }
}

/// Build `\x1b[1;{modp}{final}` CSI modifier sequence.
fn push_csi_mod(out: &mut Vec<u8>, n: &str, modp: u8, final_byte: u8) {
    out.extend_from_slice(format!("\x1b[{n};{modp}").as_bytes());
    out.push(final_byte);
}

/// Function key encoding F1–F12, with optional xterm modifier parameters.
fn encode_fkey(out: &mut Vec<u8>, n: u8, modp: u8, has_mod: bool) {
    match n {
        1..=4 => {
            // F1–F4 use SS3: \x1bOP … \x1bOS (or CSI 1;{modp} P…S with mods).
            let letter = b'P' + (n - 1); // P, Q, R, S
            if has_mod {
                push_csi_mod(out, "1", modp, letter);
            } else {
                out.extend_from_slice(b"\x1bO");
                out.push(letter);
            }
        }
        5..=12 => {
            let num: u8 = match n {
                5 => 15,
                6 => 17,
                7 => 18,
                8 => 19,
                9 => 20,
                10 => 21,
                11 => 23,
                12 => 24,
                _ => return,
            };
            if has_mod {
                out.extend_from_slice(format!("\x1b[{num};{modp}~").as_bytes());
            } else {
                out.extend_from_slice(format!("\x1b[{num}~").as_bytes());
            }
        }
        _ => {}
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEventState, MouseEventKind};

    fn press(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: mods,
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        }
    }

    fn modes_default() -> Modes {
        Modes::default()
    }

    fn modes_with(f: impl Fn(&mut Modes)) -> Modes {
        let mut m = Modes::default();
        f(&mut m);
        m
    }

    fn k(code: KeyCode, mods: KeyModifiers) -> Vec<u8> {
        encode_key(&press(code, mods), &modes_default())
    }

    const NONE: KeyModifiers = KeyModifiers::NONE;
    const CTRL: KeyModifiers = KeyModifiers::CONTROL;
    const ALT: KeyModifiers = KeyModifiers::ALT;
    const SHIFT: KeyModifiers = KeyModifiers::SHIFT;

    // ── Release events are ignored ────────────────────────────────────────────

    #[test]
    fn release_is_empty() {
        let ev = KeyEvent {
            code: KeyCode::Char('a'),
            modifiers: NONE,
            kind: KeyEventKind::Release,
            state: KeyEventState::empty(),
        };
        assert_eq!(encode_key(&ev, &modes_default()), b"");
    }

    // ── Arrow keys without DECCKM ─────────────────────────────────────────────

    #[test]
    fn arrows_normal() {
        assert_eq!(k(KeyCode::Up, NONE), b"\x1b[A");
        assert_eq!(k(KeyCode::Down, NONE), b"\x1b[B");
        assert_eq!(k(KeyCode::Right, NONE), b"\x1b[C");
        assert_eq!(k(KeyCode::Left, NONE), b"\x1b[D");
    }

    // ── Arrow keys with DECCKM ────────────────────────────────────────────────

    #[test]
    fn arrows_decckm() {
        let modes = modes_with(|m| m.app_cursor = true);
        let enc = |c| encode_key(&press(c, NONE), &modes);
        assert_eq!(enc(KeyCode::Up), b"\x1bOA");
        assert_eq!(enc(KeyCode::Down), b"\x1bOB");
        assert_eq!(enc(KeyCode::Right), b"\x1bOC");
        assert_eq!(enc(KeyCode::Left), b"\x1bOD");
    }

    // ── Arrows with modifiers (override DECCKM) ───────────────────────────────

    #[test]
    fn arrows_shift() {
        assert_eq!(k(KeyCode::Up, SHIFT), b"\x1b[1;2A");
        assert_eq!(k(KeyCode::Down, SHIFT), b"\x1b[1;2B");
    }

    #[test]
    fn arrows_alt() {
        assert_eq!(k(KeyCode::Right, ALT), b"\x1b[1;3C");
    }

    #[test]
    fn arrows_ctrl() {
        assert_eq!(k(KeyCode::Left, CTRL), b"\x1b[1;5D");
    }

    #[test]
    fn arrows_ctrl_shift() {
        assert_eq!(k(KeyCode::Up, CTRL | SHIFT), b"\x1b[1;6A");
    }

    #[test]
    fn arrows_ctrl_alt() {
        assert_eq!(k(KeyCode::Down, CTRL | ALT), b"\x1b[1;7B");
    }

    #[test]
    fn arrows_decckm_with_modifier_overrides() {
        let modes = modes_with(|m| m.app_cursor = true);
        // Modifier takes priority; SS3 form is NOT used.
        assert_eq!(encode_key(&press(KeyCode::Up, SHIFT), &modes), b"\x1b[1;2A");
    }

    // ── Ctrl+letter ───────────────────────────────────────────────────────────

    #[test]
    fn ctrl_letters() {
        assert_eq!(k(KeyCode::Char('a'), CTRL), vec![1]);
        assert_eq!(k(KeyCode::Char('c'), CTRL), vec![3]);
        assert_eq!(k(KeyCode::Char('z'), CTRL), vec![26]);
        // Uppercase letters also work (crossterm may pass them uppercased).
        assert_eq!(k(KeyCode::Char('A'), CTRL), vec![1]);
    }

    #[test]
    fn ctrl_special_chars() {
        assert_eq!(k(KeyCode::Char(' '), CTRL), vec![0]);
        assert_eq!(k(KeyCode::Char('@'), CTRL), vec![0]);
        assert_eq!(k(KeyCode::Char('['), CTRL), vec![0x1b]);
        assert_eq!(k(KeyCode::Char('\\'), CTRL), vec![0x1c]);
        assert_eq!(k(KeyCode::Char(']'), CTRL), vec![0x1d]);
        assert_eq!(k(KeyCode::Char('^'), CTRL), vec![0x1e]);
        assert_eq!(k(KeyCode::Char('_'), CTRL), vec![0x1f]);
    }

    // ── Alt prefix ────────────────────────────────────────────────────────────

    #[test]
    fn alt_char() {
        assert_eq!(k(KeyCode::Char('x'), ALT), b"\x1bx");
        assert_eq!(k(KeyCode::Enter, ALT), b"\x1b\r");
        assert_eq!(k(KeyCode::Backspace, ALT), b"\x1b\x7f");
    }

    #[test]
    fn alt_ctrl() {
        // Ctrl+Alt+c → ESC + 0x03
        assert_eq!(k(KeyCode::Char('c'), CTRL | ALT), vec![0x1b, 3]);
    }

    // ── Special keys ──────────────────────────────────────────────────────────

    #[test]
    fn special_keys() {
        assert_eq!(k(KeyCode::Enter, NONE), b"\r");
        assert_eq!(k(KeyCode::Backspace, NONE), b"\x7f");
        assert_eq!(k(KeyCode::Tab, NONE), b"\t");
        assert_eq!(k(KeyCode::BackTab, NONE), b"\x1b[Z");
        assert_eq!(k(KeyCode::Esc, NONE), b"\x1b");
    }

    #[test]
    fn editing_keys() {
        assert_eq!(k(KeyCode::Home, NONE), b"\x1b[H");
        assert_eq!(k(KeyCode::End, NONE), b"\x1b[F");
        assert_eq!(k(KeyCode::Insert, NONE), b"\x1b[2~");
        assert_eq!(k(KeyCode::Delete, NONE), b"\x1b[3~");
        assert_eq!(k(KeyCode::PageUp, NONE), b"\x1b[5~");
        assert_eq!(k(KeyCode::PageDown, NONE), b"\x1b[6~");
    }

    #[test]
    fn editing_keys_with_modifier() {
        assert_eq!(k(KeyCode::Home, SHIFT), b"\x1b[1;2H");
        assert_eq!(k(KeyCode::End, CTRL), b"\x1b[1;5F");
        assert_eq!(k(KeyCode::Delete, ALT), b"\x1b[3;3~");
        assert_eq!(k(KeyCode::PageUp, SHIFT), b"\x1b[5;2~");
    }

    // ── Function keys ─────────────────────────────────────────────────────────

    #[test]
    fn fkeys() {
        assert_eq!(k(KeyCode::F(1), NONE), b"\x1bOP");
        assert_eq!(k(KeyCode::F(2), NONE), b"\x1bOQ");
        assert_eq!(k(KeyCode::F(3), NONE), b"\x1bOR");
        assert_eq!(k(KeyCode::F(4), NONE), b"\x1bOS");
        assert_eq!(k(KeyCode::F(5), NONE), b"\x1b[15~");
        assert_eq!(k(KeyCode::F(6), NONE), b"\x1b[17~");
        assert_eq!(k(KeyCode::F(7), NONE), b"\x1b[18~");
        assert_eq!(k(KeyCode::F(8), NONE), b"\x1b[19~");
        assert_eq!(k(KeyCode::F(9), NONE), b"\x1b[20~");
        assert_eq!(k(KeyCode::F(10), NONE), b"\x1b[21~");
        assert_eq!(k(KeyCode::F(11), NONE), b"\x1b[23~");
        assert_eq!(k(KeyCode::F(12), NONE), b"\x1b[24~");
    }

    #[test]
    fn fkeys_with_modifier() {
        assert_eq!(k(KeyCode::F(1), SHIFT), b"\x1b[1;2P");
        assert_eq!(k(KeyCode::F(4), CTRL), b"\x1b[1;5S");
        assert_eq!(k(KeyCode::F(5), SHIFT), b"\x1b[15;2~");
        assert_eq!(k(KeyCode::F(12), ALT), b"\x1b[24;3~");
    }

    // ── Paste bracketing ──────────────────────────────────────────────────────

    #[test]
    fn paste_no_bracket() {
        let m = modes_default();
        assert_eq!(encode_paste("hello", &m), b"hello");
        assert_eq!(encode_paste("a\nb", &m), b"a\nb");
    }

    #[test]
    fn paste_bracketed() {
        let m = modes_with(|m| m.bracketed_paste = true);
        assert_eq!(encode_paste("hello", &m), b"\x1b[200~hello\x1b[201~");
    }

    // ── Mouse ─────────────────────────────────────────────────────────────────

    fn mouse_ev(kind: MouseEventKind, col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    /// Pane: origin (0,0), size (80,24).
    const ORIGIN: (u16, u16) = (0, 0);
    const SIZE: (u16, u16) = (80, 24);

    #[test]
    fn mouse_off_returns_none() {
        let m = modes_default(); // mouse = Off
        let ev = mouse_ev(MouseEventKind::Down(MouseButton::Left), 5, 5);
        assert!(encode_mouse(&ev, ORIGIN, SIZE, &m).is_none());
    }

    #[test]
    fn mouse_outside_pane_returns_none() {
        let m = modes_with(|m| m.mouse = MouseMode::Click);
        let ev = mouse_ev(MouseEventKind::Down(MouseButton::Left), 90, 5); // col 90 > size 80
        assert!(encode_mouse(&ev, ORIGIN, SIZE, &m).is_none());
    }

    #[test]
    fn mouse_click_x10_left_press() {
        let m = modes_with(|m| m.mouse = MouseMode::Click);
        let ev = mouse_ev(MouseEventKind::Down(MouseButton::Left), 4, 2);
        // Expected: \x1b[M + (0+32) + (4+1+32) + (2+1+32) = \x1b[M + 32 + 37 + 35
        let out = encode_mouse(&ev, ORIGIN, SIZE, &m).unwrap();
        assert_eq!(out, vec![0x1b, b'[', b'M', 32, 37, 35]);
    }

    #[test]
    fn mouse_click_x10_left_release() {
        let m = modes_with(|m| m.mouse = MouseMode::Click);
        let ev = mouse_ev(MouseEventKind::Up(MouseButton::Left), 4, 2);
        // Release in X10 = button 3.
        let out = encode_mouse(&ev, ORIGIN, SIZE, &m).unwrap();
        assert_eq!(out, vec![0x1b, b'[', b'M', 3 + 32, 37, 35]);
    }

    #[test]
    fn mouse_sgr_left_press() {
        let m = modes_with(|m| {
            m.mouse = MouseMode::Click;
            m.mouse_sgr = true;
        });
        let ev = mouse_ev(MouseEventKind::Down(MouseButton::Left), 10, 5);
        let out = encode_mouse(&ev, ORIGIN, SIZE, &m).unwrap();
        assert_eq!(out, b"\x1b[<0;11;6M");
    }

    #[test]
    fn mouse_sgr_left_release() {
        let m = modes_with(|m| {
            m.mouse = MouseMode::Click;
            m.mouse_sgr = true;
        });
        let ev = mouse_ev(MouseEventKind::Up(MouseButton::Left), 10, 5);
        let out = encode_mouse(&ev, ORIGIN, SIZE, &m).unwrap();
        assert_eq!(out, b"\x1b[<0;11;6m");
    }

    #[test]
    fn mouse_drag_not_reported_in_click_mode() {
        let m = modes_with(|m| m.mouse = MouseMode::Click);
        let ev = mouse_ev(MouseEventKind::Drag(MouseButton::Left), 5, 5);
        assert!(encode_mouse(&ev, ORIGIN, SIZE, &m).is_none());
    }

    #[test]
    fn mouse_drag_reported_in_drag_mode() {
        let m = modes_with(|m| {
            m.mouse = MouseMode::Drag;
            m.mouse_sgr = true;
        });
        let ev = mouse_ev(MouseEventKind::Drag(MouseButton::Left), 5, 5);
        // button 0 | 0x20 = 32
        let out = encode_mouse(&ev, ORIGIN, SIZE, &m).unwrap();
        assert_eq!(out, b"\x1b[<32;6;6M");
    }

    #[test]
    fn mouse_motion_only_in_motion_mode() {
        let m_drag = modes_with(|m| m.mouse = MouseMode::Drag);
        let m_motion = modes_with(|m| m.mouse = MouseMode::Motion);
        let ev = mouse_ev(MouseEventKind::Moved, 5, 5);
        assert!(encode_mouse(&ev, ORIGIN, SIZE, &m_drag).is_none());
        assert!(encode_mouse(&ev, ORIGIN, SIZE, &m_motion).is_some());
    }

    #[test]
    fn mouse_scroll_in_click_mode() {
        let m = modes_with(|m| {
            m.mouse = MouseMode::Click;
            m.mouse_sgr = true;
        });
        // xterm: wheel up = button 4 (64), wheel down = button 5 (65).
        let up = encode_mouse(&mouse_ev(MouseEventKind::ScrollUp, 5, 5), ORIGIN, SIZE, &m);
        assert_eq!(up.unwrap(), b"\x1b[<64;6;6M");
        let down = encode_mouse(
            &mouse_ev(MouseEventKind::ScrollDown, 5, 5),
            ORIGIN,
            SIZE,
            &m,
        );
        assert_eq!(down.unwrap(), b"\x1b[<65;6;6M");
    }

    #[test]
    fn mouse_coordinate_translation() {
        // Pane starts at origin (10, 5); event at (12, 7) → pane-relative (2,2) → 1-based (3,3).
        let m = modes_with(|m| {
            m.mouse = MouseMode::Click;
            m.mouse_sgr = true;
        });
        let ev = mouse_ev(MouseEventKind::Down(MouseButton::Left), 12, 7);
        let out = encode_mouse(&ev, (10, 5), (80, 24), &m).unwrap();
        assert_eq!(out, b"\x1b[<0;3;3M");
    }

    #[test]
    fn mouse_x10_coord_clamp() {
        // Coordinate at 224 exceeds the 223 max for X10 encoding.
        let m = modes_with(|m| m.mouse = MouseMode::Motion);
        let ev = mouse_ev(MouseEventKind::Moved, 250, 5);
        let out = encode_mouse(&ev, ORIGIN, (300, 100), &m).unwrap();
        // cx = min(251, 223) + 32 = 223 + 32 = 255 = 0xFF
        assert_eq!(out[4], 0xFF);
    }

    // ── Focus ─────────────────────────────────────────────────────────────────

    #[test]
    fn focus_off() {
        let m = modes_default();
        assert!(encode_focus(true, &m).is_none());
        assert!(encode_focus(false, &m).is_none());
    }

    #[test]
    fn focus_on() {
        let m = modes_with(|m| m.focus_reporting = true);
        assert_eq!(encode_focus(true, &m).unwrap(), b"\x1b[I");
        assert_eq!(encode_focus(false, &m).unwrap(), b"\x1b[O");
    }

    // ── Ctrl+non-ASCII must produce no bytes ───────────────────────────────

    /// Ctrl+non-ASCII (e.g. Ctrl+é) must produce an empty byte sequence, not
    /// truncated garbage from `c as u8` on a multi-byte codepoint.
    #[test]
    fn ctrl_non_ascii_ignored() {
        let m = modes_default();
        // Ctrl+é (U+00E9) — non-ASCII, has no defined Ctrl encoding.
        let out = encode_key(&press(KeyCode::Char('é'), KeyModifiers::CONTROL), &m);
        assert!(
            out.is_empty(),
            "Ctrl+non-ASCII must produce empty output, got: {out:?}"
        );

        // Ctrl+€ (U+20AC) — another non-ASCII character.
        let out2 = encode_key(&press(KeyCode::Char('€'), KeyModifiers::CONTROL), &m);
        assert!(
            out2.is_empty(),
            "Ctrl+non-ASCII must produce empty output, got: {out2:?}"
        );
    }

    /// Ctrl+ASCII characters must still work correctly.
    #[test]
    fn ctrl_ascii_still_works() {
        let m = modes_default();
        // Ctrl+C = 0x03
        let out = encode_key(&press(KeyCode::Char('c'), KeyModifiers::CONTROL), &m);
        assert_eq!(out, vec![0x03], "Ctrl+c must produce 0x03");
        // Ctrl+A = 0x01
        let out2 = encode_key(&press(KeyCode::Char('a'), KeyModifiers::CONTROL), &m);
        assert_eq!(out2, vec![0x01], "Ctrl+a must produce 0x01");
    }
}
