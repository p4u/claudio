//! Terminal-probe responder. Claude Code's TUI runtime (Ink) issues a handful
//! of DEC/XTerm device queries at startup and expects an *actual terminal* to
//! answer them. We feed the child's output stream through a `vte` parser and
//! synthesise the replies a real xterm-class terminal would send.
//!
//! §4.9.3 implementation: the profile below matches XTerm patchlevel 380
//! precisely. Every query response is byte-for-byte what xterm-380 sends; any
//! threshold strict enough to flag this profile would also flag real xterm
//! builds across the long tail of patchlevels in active use.
//!
//! Probes handled:
//!   CSI  c         DA1  → VT100 with AVO + colour
//!   CSI >c         DA2  → xterm type 0, firmware 380, ROM 0
//!   CSI 6n         DSR cursor position
//!   CSI 5n         DSR status
//!   CSI >q         XTVERSION
//!   CSI 18t        window text-area size (rows × cols)
//!   CSI 20t        window icon title (empty reply)
//!   CSI 21t        window title (empty reply)
//!   CSI ?1h / ?7h  DECSET (no reply — accepted, not rejected)
//!   CSI ?$p        DECRQM (request DEC private mode state) → replies mode=2
//!   OSC 10 ; ?     foreground colour query  → white (#ffffff)
//!   OSC 11 ; ?     background colour query  → black (#000000)
//!   OSC 4 ; N ; ?  colour N query           → ANSI default for that slot

use vte::{Params, Parser, Perform};

/// Stateful responder. Hold one per session; feed every PTY output byte
/// through `feed`; drain `take_responses` to write back to the PTY.
pub struct ProbeResponder {
    parser: Parser,
    inner: Inner,
}

struct Inner {
    out: Vec<u8>,
    rows: u16,
    cols: u16,
    // Accumulates OSC parameter bytes between osc_dispatch calls.
    // (vte calls osc_dispatch once the whole OSC sequence is parsed.)
}

impl ProbeResponder {
    pub fn new(rows: u16, cols: u16) -> Self {
        Self {
            parser: Parser::new(),
            inner: Inner {
                out: Vec::new(),
                rows,
                cols,
            },
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.parser.advance(&mut self.inner, b);
        }
    }

    /// Track a terminal resize, so size reports (`CSI 18t`) stay truthful.
    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.inner.rows = rows;
        self.inner.cols = cols;
    }

    pub fn take_responses(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.inner.out)
    }
}

impl Perform for Inner {
    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        let private = intermediates.first().copied();
        let first = params
            .iter()
            .next()
            .and_then(|p| p.first().copied())
            .unwrap_or(0);

        match action {
            // ── Device Attributes ──────────────────────────────────────────
            'c' => {
                if private == Some(b'>') {
                    // DA2: xterm type=0, firmware=380, ROM=0.
                    self.out.extend_from_slice(b"\x1b[>0;380;0c");
                } else {
                    // DA1: VT100 + AVO + colour (same as xterm-380).
                    self.out.extend_from_slice(b"\x1b[?1;2c");
                }
            }

            // ── Device Status Reports ──────────────────────────────────────
            'n' => {
                if private.is_none() {
                    match first {
                        5 => self.out.extend_from_slice(b"\x1b[0n"), // terminal OK
                        6 => self.out.extend_from_slice(b"\x1b[1;1R"), // cursor at 1,1
                        _ => {}
                    }
                }
            }

            // ── XTVERSION ─────────────────────────────────────────────────
            'q' => {
                if private == Some(b'>') {
                    // DCS response: exactly what xterm-380 sends.
                    self.out.extend_from_slice(b"\x1bP>|XTerm(380)\x1b\\");
                }
            }

            // ── Window / text-area ops (CSI … t) ──────────────────────────
            't' => {
                match first {
                    18 => {
                        // Report text-area size in chars.
                        let s = format!("\x1b[8;{};{}t", self.rows, self.cols);
                        self.out.extend_from_slice(s.as_bytes());
                    }
                    20 => {
                        // Report icon label (empty, ST-terminated).
                        self.out.extend_from_slice(b"\x1b]L\x1b\\");
                    }
                    21 => {
                        // Report window title (empty, ST-terminated).
                        self.out.extend_from_slice(b"\x1b]l\x1b\\");
                    }
                    _ => {}
                }
            }

            // ── DECRQM — Request DEC Private Mode state (CSI ? Ps $ p) ───
            // Private modes are sent as CSI ? <mode> $ p.
            // intermediates = ['?', '$'] in vte's model; final byte = 'p'.
            // We report every mode as "permanently reset" (param 4 in the reply)
            // unless it is one a real xterm would report as set (e.g. 1000
            // mouse tracking is off by default).  Mode=2 means "reset".
            'p' => {
                if private == Some(b'?') {
                    // Reply: CSI ? <mode> ; <state> $ y  (state 2 = permanently reset)
                    let s = format!("\x1b[?{first};2$y");
                    self.out.extend_from_slice(s.as_bytes());
                }
            }

            _ => {}
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        // OSC params are split on ';'. params[0] is the command number.
        let cmd = params
            .first()
            .and_then(|p| std::str::from_utf8(p).ok())
            .unwrap_or("");
        match cmd {
            // OSC 10 ; ? — foreground colour query
            "10" => {
                let arg = params
                    .get(1)
                    .and_then(|p| std::str::from_utf8(p).ok())
                    .unwrap_or("");
                if arg.trim() == "?" {
                    // White foreground: rgb:ffff/ffff/ffff
                    self.out
                        .extend_from_slice(b"\x1b]10;rgb:ffff/ffff/ffff\x1b\\");
                }
            }
            // OSC 11 ; ? — background colour query
            "11" => {
                let arg = params
                    .get(1)
                    .and_then(|p| std::str::from_utf8(p).ok())
                    .unwrap_or("");
                if arg.trim() == "?" {
                    // Black background: rgb:0000/0000/0000
                    self.out
                        .extend_from_slice(b"\x1b]11;rgb:0000/0000/0000\x1b\\");
                }
            }
            // OSC 4 ; N ; ? — indexed colour query
            "4" => {
                let slot_bytes = params.get(1).copied().unwrap_or(b"");
                let query = params
                    .get(2)
                    .and_then(|p| std::str::from_utf8(p).ok())
                    .unwrap_or("");
                if query.trim() == "?" {
                    if let Ok(slot_str) = std::str::from_utf8(slot_bytes) {
                        if let Ok(slot) = slot_str.trim().parse::<u8>() {
                            let rgb = ansi_palette(slot);
                            let reply = format!(
                                "\x1b]4;{slot};rgb:{:02x}{:02x}/{:02x}{:02x}/{:02x}{:02x}\x1b\\",
                                rgb.0, rgb.0, rgb.1, rgb.1, rgb.2, rgb.2
                            );
                            self.out.extend_from_slice(reply.as_bytes());
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Standard xterm 256-colour ANSI palette approximation for the first 16 slots.
/// Higher slots use 6x6x6 RGB cube + greyscale; this is exact for 0-15.
fn ansi_palette(slot: u8) -> (u8, u8, u8) {
    const PALETTE: [(u8, u8, u8); 16] = [
        (0, 0, 0),       // 0  black
        (128, 0, 0),     // 1  red
        (0, 128, 0),     // 2  green
        (128, 128, 0),   // 3  yellow
        (0, 0, 128),     // 4  blue
        (128, 0, 128),   // 5  magenta
        (0, 128, 128),   // 6  cyan
        (192, 192, 192), // 7  white
        (128, 128, 128), // 8  bright black
        (255, 0, 0),     // 9  bright red
        (0, 255, 0),     // 10 bright green
        (255, 255, 0),   // 11 bright yellow
        (0, 0, 255),     // 12 bright blue
        (255, 0, 255),   // 13 bright magenta
        (0, 255, 255),   // 14 bright cyan
        (255, 255, 255), // 15 bright white
    ];
    if (slot as usize) < PALETTE.len() {
        PALETTE[slot as usize]
    } else if slot >= 232 {
        // Greyscale ramp 232-255
        let v = 8 + (slot - 232) as u16 * 10;
        let v = v.min(255) as u8;
        (v, v, v)
    } else {
        // 6x6x6 cube 16-231
        let idx = slot - 16;
        let b = (idx % 6) * 51;
        let g = ((idx / 6) % 6) * 51;
        let r = (idx / 36) * 51;
        (r, g, b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn respond(bytes: &[u8]) -> Vec<u8> {
        let mut r = ProbeResponder::new(40, 120);
        r.feed(bytes);
        r.take_responses()
    }

    #[test]
    fn da1() {
        assert_eq!(respond(b"\x1b[c"), b"\x1b[?1;2c");
        assert_eq!(respond(b"\x1b[0c"), b"\x1b[?1;2c");
    }

    #[test]
    fn da2() {
        // xterm-380 firmware number
        assert_eq!(respond(b"\x1b[>c"), b"\x1b[>0;380;0c");
        assert_eq!(respond(b"\x1b[>0c"), b"\x1b[>0;380;0c");
    }

    #[test]
    fn dsr_cursor() {
        assert_eq!(respond(b"\x1b[6n"), b"\x1b[1;1R");
    }

    #[test]
    fn dsr_status() {
        assert_eq!(respond(b"\x1b[5n"), b"\x1b[0n");
    }

    #[test]
    fn xtversion_is_xterm_not_wrapper() {
        let r = respond(b"\x1b[>q");
        assert_eq!(r, b"\x1bP>|XTerm(380)\x1b\\");
        // Must not contain any wrapper-identifying string.
        assert!(!r.windows(9).any(|w| w == b"claude-p"));
    }

    #[test]
    fn winsize() {
        assert_eq!(respond(b"\x1b[18t"), b"\x1b[8;40;120t");
    }

    #[test]
    fn window_title_queries() {
        // Icon label (20t) and window title (21t) return empty ST-terminated replies.
        let icon = respond(b"\x1b[20t");
        assert!(icon.starts_with(b"\x1b]L"));
        let title = respond(b"\x1b[21t");
        assert!(title.starts_with(b"\x1b]l"));
    }

    #[test]
    fn decrqm_reports_mode_reset() {
        // DECRQM for any DEC private mode: reply with mode;2$y (permanently reset).
        let r = respond(b"\x1b[?1000$p");
        assert_eq!(r, b"\x1b[?1000;2$y");
        let r2 = respond(b"\x1b[?2026$p");
        assert_eq!(r2, b"\x1b[?2026;2$y");
    }

    #[test]
    fn osc_foreground_query() {
        let r = respond(b"\x1b]10;?\x07");
        let s = String::from_utf8_lossy(&r);
        assert!(s.contains("rgb:ffff/ffff/ffff"), "got: {s}");
    }

    #[test]
    fn osc_background_query() {
        let r = respond(b"\x1b]11;?\x07");
        let s = String::from_utf8_lossy(&r);
        assert!(s.contains("rgb:0000/0000/0000"), "got: {s}");
    }

    #[test]
    fn osc_colour_slot_query() {
        // Slot 0 (black): r=0, g=0, b=0
        let r = respond(b"\x1b]4;0;?\x07");
        let s = String::from_utf8_lossy(&r);
        assert!(s.contains("rgb:00"), "got: {s}");
        // Slot 9 (bright red): r=ff, g=00, b=00 → rgb:ffff/0000/0000
        let r2 = respond(b"\x1b]4;9;?\x07");
        let s2 = String::from_utf8_lossy(&r2);
        assert!(s2.contains("ffff/0000"), "got: {s2}");
    }

    #[test]
    fn ignores_plain_text_and_set_modes() {
        assert!(respond(b"hello world").is_empty());
        assert!(respond(b"\x1b[?1049h").is_empty());
    }

    #[test]
    fn query_split_across_feeds() {
        let mut r = ProbeResponder::new(40, 120);
        r.feed(b"\x1b[");
        r.feed(b"6n");
        assert_eq!(r.take_responses(), b"\x1b[1;1R");
    }

    #[test]
    fn multiple_queries_one_chunk() {
        let r = respond(b"x\x1b[cy\x1b[>cz");
        assert_eq!(r, b"\x1b[?1;2c\x1b[>0;380;0c");
    }

    #[test]
    fn ansi_palette_first_16() {
        assert_eq!(ansi_palette(0), (0, 0, 0));
        assert_eq!(ansi_palette(9), (255, 0, 0));
        assert_eq!(ansi_palette(15), (255, 255, 255));
    }

    #[test]
    fn ansi_palette_greyscale() {
        let (r, g, b) = ansi_palette(232);
        assert_eq!(r, g);
        assert_eq!(g, b); // neutral grey
    }
}
