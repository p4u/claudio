//! Terminal-probe responder. Claude Code's TUI runtime (Ink) issues a handful
//! of DEC/XTerm device queries at startup and expects an *actual terminal* to
//! answer them. We feed the child's output stream through a `vte` parser and
//! synthesize the replies a real terminal would send.
//!
//! Why a real VT parser instead of byte-scanning: `vte` implements the full
//! Paul Williams state machine, so it correctly separates private prefixes
//! (`>` for secondary DA / XTVERSION, `?` for DEC private modes), intermediate
//! bytes, and parameters — and never misfires on a query split across read
//! boundaries.
//!
//! Report note: the responses below are deliberately indistinguishable from a
//! common real terminal. That is the point — a server cannot fingerprint
//! "automation" from probe answers without also flagging real terminals.

use vte::{Params, Parser, Perform};

/// Stateful responder. Hold one across the whole session and feed every PTY
/// output byte through `feed`; drain `take_responses` to write back to the PTY.
pub struct ProbeResponder {
    parser: Parser,
    inner: Inner,
}

struct Inner {
    out: Vec<u8>,
    rows: u16,
    cols: u16,
}

impl ProbeResponder {
    pub fn new(rows: u16, cols: u16) -> Self {
        Self {
            parser: Parser::new(),
            inner: Inner { out: Vec::new(), rows, cols },
        }
    }

    /// Feed a chunk of PTY output. Any synthesized replies accumulate; call
    /// `take_responses` to retrieve and clear them.
    pub fn feed(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.parser.advance(&mut self.inner, b);
        }
    }

    /// Take any pending response bytes to write back to the PTY.
    pub fn take_responses(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.inner.out)
    }
}

impl Perform for Inner {
    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        let private = intermediates.first().copied();
        let first = params.iter().next().and_then(|p| p.first().copied()).unwrap_or(0);
        match action {
            'c' => {
                if private == Some(b'>') {
                    // DA2 (secondary device attributes). Mimic an xterm-class
                    // terminal: type 0, firmware 276, ROM cartridge 0.
                    self.out.extend_from_slice(b"\x1b[>0;276;0c");
                } else {
                    // DA1 (primary): "VT100 with Advanced Video Option".
                    self.out.extend_from_slice(b"\x1b[?1;2c");
                }
            }
            'n' => {
                if private.is_none() && first == 6 {
                    // DSR cursor position report.
                    self.out.extend_from_slice(b"\x1b[1;1R");
                } else if private.is_none() && first == 5 {
                    // DSR "terminal OK".
                    self.out.extend_from_slice(b"\x1b[0n");
                }
            }
            'q' => {
                if private == Some(b'>') {
                    // XTVERSION: DCS reply naming the terminal.
                    self.out.extend_from_slice(b"\x1bP>|claude-poc(1.0)\x1b\\");
                }
            }
            't' => {
                if first == 18 {
                    // Report text-area size in characters: ESC[8;rows;cols t
                    let s = format!("\x1b[8;{};{}t", self.rows, self.cols);
                    self.out.extend_from_slice(s.as_bytes());
                }
            }
            _ => {}
        }
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
        assert_eq!(respond(b"\x1b[>c"), b"\x1b[>0;276;0c");
        assert_eq!(respond(b"\x1b[>0c"), b"\x1b[>0;276;0c");
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
    fn xtversion() {
        let r = respond(b"\x1b[>q");
        assert!(r.starts_with(b"\x1bP>|"));
        assert!(r.ends_with(b"\x1b\\"));
    }

    #[test]
    fn winsize() {
        assert_eq!(respond(b"\x1b[18t"), b"\x1b[8;40;120t");
    }

    #[test]
    fn ignores_plain_text_and_set_modes() {
        assert!(respond(b"hello world").is_empty());
        // DECSET alt-screen is a command, not a query — no reply.
        assert!(respond(b"\x1b[?1049h").is_empty());
    }

    #[test]
    fn query_split_across_feeds() {
        // A query arriving in two reads must still be answered exactly once.
        let mut r = ProbeResponder::new(40, 120);
        r.feed(b"\x1b[");
        r.feed(b"6n");
        assert_eq!(r.take_responses(), b"\x1b[1;1R");
    }

    #[test]
    fn multiple_queries_one_chunk() {
        let r = respond(b"x\x1b[cy\x1b[>cz");
        assert_eq!(r, b"\x1b[?1;2c\x1b[>0;276;0c");
    }
}
