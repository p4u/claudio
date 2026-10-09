//! Manager key bindings.
//!
//! Every key the manager intercepts (instead of forwarding to claude) is
//! assigned in [`BINDINGS`], so the table can later be loaded from
//! `config.toml`. Matching is exact: Alt+Shift+← is not Alt+←.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

/// Something the manager does instead of forwarding a key to claude.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    PrevSession,
    NextSession,
    NewSession,
    Rename,
    Close,
    /// Jump to the next session that wants attention (cycles).
    NextAttention,
    /// Leave the UI; sessions keep running in the daemon.
    Quit,
    /// Open the proxy stats popup for the active session.
    ProxyStats,
}

/// The binding table.
pub const BINDINGS: &[(KeyCode, KeyModifiers, Action)] = &[
    (KeyCode::Left, KeyModifiers::ALT, Action::PrevSession),
    (KeyCode::Right, KeyModifiers::ALT, Action::NextSession),
    (KeyCode::Char('n'), KeyModifiers::ALT, Action::NewSession),
    (KeyCode::Char('r'), KeyModifiers::ALT, Action::Rename),
    (KeyCode::Char('x'), KeyModifiers::ALT, Action::Close),
    (KeyCode::Char('a'), KeyModifiers::ALT, Action::NextAttention),
    (KeyCode::Char('q'), KeyModifiers::ALT, Action::Quit),
    // Alt+s: proxy stats. Verified not used by claude (grep returned empty).
    (KeyCode::Char('s'), KeyModifiers::ALT, Action::ProxyStats),
];

/// Key hints shown in the status bar.
pub const HINTS: &str = "Alt+←/→ switch · Alt+n new · Alt+r rename · Alt+x close · Alt+s proxy · Alt+q quit";

/// The action bound to `key`, if any. Releases never trigger actions.
pub fn lookup(key: &KeyEvent) -> Option<Action> {
    if key.kind == KeyEventKind::Release {
        return None;
    }
    BINDINGS
        .iter()
        .find(|(code, mods, _)| *code == key.code && *mods == key.modifiers)
        .map(|&(_, _, action)| action)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventState;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn matches_bound_keys() {
        assert_eq!(lookup(&key(KeyCode::Left, KeyModifiers::ALT)), Some(Action::PrevSession));
        assert_eq!(lookup(&key(KeyCode::Right, KeyModifiers::ALT)), Some(Action::NextSession));
        assert_eq!(lookup(&key(KeyCode::Char('q'), KeyModifiers::ALT)), Some(Action::Quit));
        assert_eq!(lookup(&key(KeyCode::Char('a'), KeyModifiers::ALT)), Some(Action::NextAttention));
    }

    #[test]
    fn modifiers_must_match_exactly() {
        assert_eq!(lookup(&key(KeyCode::Left, KeyModifiers::ALT | KeyModifiers::SHIFT)), None);
        assert_eq!(lookup(&key(KeyCode::Left, KeyModifiers::NONE)), None);
        assert_eq!(lookup(&key(KeyCode::Char('n'), KeyModifiers::CONTROL)), None);
        assert_eq!(lookup(&key(KeyCode::Char('N'), KeyModifiers::ALT | KeyModifiers::SHIFT)), None);
        assert_eq!(lookup(&key(KeyCode::Char('n'), KeyModifiers::ALT | KeyModifiers::CONTROL)), None);
    }

    #[test]
    fn releases_are_ignored() {
        let release = KeyEvent {
            code: KeyCode::Char('q'),
            modifiers: KeyModifiers::ALT,
            kind: KeyEventKind::Release,
            state: KeyEventState::NONE,
        };
        assert_eq!(lookup(&release), None);
    }
}
