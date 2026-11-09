//! Text that leaves the process as a label (a tab title, an OSC
//! notification, a session name).

/// Strip C0 controls (0x00–0x1F), DEL (0x7F), and C1 controls (0x80–0x9F)
/// from `s`, and cap the result at `max_chars` characters. Used for any
/// text that leaves the process boundary (OSC notifications, tab labels
/// written to the outer terminal).
pub fn sanitize_label(s: &str, max_chars: usize) -> String {
    s.chars()
        .filter(|&c| {
            let n = c as u32;
            // Keep printable ASCII and non-C1 Unicode.
            !(n < 0x20 || n == 0x7f || (0x80..=0x9f).contains(&n))
        })
        .take(max_chars)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_c0_controls() {
        assert_eq!(sanitize_label("hello\x07world", 200), "helloworld");
        // ESC (0x1B) is stripped; the remaining "[31m" chars are printable ASCII.
        assert_eq!(sanitize_label("a\x1b[31mb", 200), "a[31mb");
    }

    #[test]
    fn sanitize_strips_del() {
        assert_eq!(sanitize_label("a\x7fb", 200), "ab");
    }

    #[test]
    fn sanitize_strips_c1_controls() {
        // C1 control: 0x9B (CSI in Latin-1).
        let s = "\u{009B}test";
        assert_eq!(sanitize_label(s, 200), "test");
    }

    #[test]
    fn sanitize_caps_length() {
        let long: String = "a".repeat(300);
        assert_eq!(sanitize_label(&long, 200).len(), 200);
    }

    #[test]
    fn sanitize_keeps_normal_unicode() {
        assert_eq!(sanitize_label("héllo wörld", 200), "héllo wörld");
    }

    #[test]
    fn sanitize_malicious_label() {
        // Injection attempt: ESC (stripped), ] (printable), BEL (stripped), [31m (printable).
        // ESC is the dangerous byte; stripping it neutralises the OSC sequence.
        let evil = "\x1b]0;injected\x07\x1b[31m";
        assert_eq!(sanitize_label(evil, 200), "]0;injected[31m");
    }
}
