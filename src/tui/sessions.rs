//! Session view type and sanitization helpers (S1 split from app.rs).

use crate::proto::{SessionId, SessionState};
use crate::term::screen::Screen;

use super::state::SavedSession;

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

/// The client-side view of one session.
pub struct SessionView {
    pub id: SessionId,
    pub name: Option<String>,
    pub cwd: String,
    pub host: String,
    pub state: SessionState,
    pub title: Option<String>,
    pub claude_session_id: Option<String>,
    pub created_at: u64,
    /// Mirror of the daemon's screen; fed only while attached.
    pub mirror: Screen,
    pub attached: bool,
    /// Proxy profile name (None = no proxy). Only the name, never the token.
    pub proxy: Option<String>,
}

impl SessionView {
    /// The tab label: the user's name, else claude's title, else the cwd's
    /// basename. Sanitized so it is safe to embed in escape sequences.
    pub fn label(&self) -> String {
        let raw = self
            .name
            .clone()
            .or_else(|| self.title.clone())
            .unwrap_or_else(|| self.cwd.rsplit('/').find(|s| !s.is_empty()).unwrap_or("/").to_owned());
        sanitize_label(&raw, 200)
    }

    pub fn saved(&self) -> SavedSession {
        SavedSession {
            id: self.id,
            name: self.name.clone(),
            cwd: self.cwd.clone(),
            host: self.host.clone(),
            claude_session_id: self.claude_session_id.clone(),
            created_at: self.created_at,
            proxy: self.proxy.clone(),
        }
    }
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
