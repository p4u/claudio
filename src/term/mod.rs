//! Terminal plumbing shared by every PTY host (print mode, the daemon, the TUI).

pub mod probe;

#[cfg_attr(not(test), allow(dead_code))]
pub mod keys;
#[cfg_attr(not(test), allow(dead_code))]
pub mod screen;

/// Strip CSI / OSC / DCS escape sequences so plain-text matching is robust
/// against cursor-positioning escapes that pad words.
pub fn strip_escapes(bytes: &[u8]) -> String {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != 0x1b {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        if i + 1 >= bytes.len() {
            break;
        }
        match bytes[i + 1] {
            b'[' => {
                i += 2;
                while i < bytes.len() && (0x30..=0x3f).contains(&bytes[i]) {
                    i += 1;
                }
                while i < bytes.len() && (0x20..=0x2f).contains(&bytes[i]) {
                    i += 1;
                }
                if i < bytes.len() {
                    i += 1;
                }
            }
            b']' => {
                i += 2;
                while i < bytes.len() {
                    if bytes[i] == 0x07 {
                        i += 1;
                        break;
                    }
                    if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'\\' {
                        i += 2;
                        break;
                    }
                    i += 1;
                }
            }
            b'P' | b'X' | b'^' | b'_' => {
                i += 2;
                while i < bytes.len() {
                    if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'\\' {
                        i += 2;
                        break;
                    }
                    i += 1;
                }
            }
            _ => {
                i += 2;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_csi_bold_and_cursor_move() {
        assert_eq!(strip_escapes(b"\x1b[1mhello\x1b[0m"), "hello");
        assert_eq!(strip_escapes(b"do\x1b[1Cyou\x1b[1Ctrust"), "doyoutrust");
    }

    #[test]
    fn strip_osc_sequence() {
        // OSC 0 (set window title): ESC ] 0 ; title BEL
        let input = b"before\x1b]0;My Terminal\x07after";
        assert_eq!(strip_escapes(input), "beforeafter");
    }

    #[test]
    fn strip_osc_st_terminated() {
        // OSC terminated by ST (ESC \) instead of BEL
        let input = b"x\x1b]2;title\x1b\\y";
        assert_eq!(strip_escapes(input), "xy");
    }

    #[test]
    fn strip_dcs_sequence() {
        // DCS (ESC P ... ESC \)
        let input = b"a\x1bP>|xterm\x1b\\b";
        assert_eq!(strip_escapes(input), "ab");
    }

    #[test]
    fn strip_leaves_plain_text() {
        assert_eq!(strip_escapes(b"hello world"), "hello world");
    }

    #[test]
    fn strip_incomplete_escape_at_end() {
        // Incomplete escape at end of buffer should not panic.
        assert_eq!(strip_escapes(b"text\x1b"), "text");
    }

    #[test]
    fn strip_two_letter_escape() {
        // ESC M (reverse index) — 2-byte sequence, no bracket
        let input = b"a\x1bMb";
        assert_eq!(strip_escapes(input), "ab");
    }
}
