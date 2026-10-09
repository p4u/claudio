//! Manager key bindings.
//!
//! Every key the manager intercepts (instead of forwarding to claude) is
//! defined in [`DEFAULT_BINDINGS`], which is the single source of truth for
//! both dispatch (`lookup`) and the generated help popup.
//!
//! Overrides from `config.toml` `[keys]` are applied at startup by
//! [`init`], which replaces the action's binding. Unknown action names,
//! unparseable key specs, and collisions produce a notice and are ignored.
//!
//! The effective table is stored in `App` as a `Keymap` value, so status-bar
//! hints always reflect overrides.

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
    /// Open the overview ("mission control") popup.
    Overview,
    /// Open the help popup.
    Help,
}

impl Action {
    /// The action's canonical name (matches `config.toml` `[keys]` action names).
    pub fn name(self) -> &'static str {
        match self {
            Action::PrevSession => "prev_session",
            Action::NextSession => "next_session",
            Action::NewSession => "new_session",
            Action::Rename => "rename",
            Action::Close => "close",
            Action::NextAttention => "next_attention",
            Action::Quit => "quit",
            Action::ProxyStats => "proxy_stats",
            Action::Overview => "overview",
            Action::Help => "help",
        }
    }
}

/// A single binding in the effective table: key + description + action.
#[derive(Debug, Clone)]
pub struct Binding {
    pub code: KeyCode,
    pub mods: KeyModifiers,
    /// Short description shown in the help popup.
    pub label: &'static str,
    pub action: Action,
}

/// The default binding table. The help popup is generated from this, so it
/// can never drift from what the manager actually does.
pub const DEFAULT_BINDINGS: &[Binding] = &[
    Binding {
        code: KeyCode::Left,
        mods: KeyModifiers::ALT,
        label: "previous session",
        action: Action::PrevSession,
    },
    Binding {
        code: KeyCode::Right,
        mods: KeyModifiers::ALT,
        label: "next session",
        action: Action::NextSession,
    },
    Binding {
        code: KeyCode::Char('n'),
        mods: KeyModifiers::ALT,
        label: "new session (wizard)",
        action: Action::NewSession,
    },
    Binding {
        code: KeyCode::Char('r'),
        mods: KeyModifiers::ALT,
        label: "rename session",
        action: Action::Rename,
    },
    Binding {
        code: KeyCode::Char('x'),
        mods: KeyModifiers::ALT,
        label: "close / kill session",
        action: Action::Close,
    },
    Binding {
        code: KeyCode::Char('a'),
        mods: KeyModifiers::ALT,
        label: "jump to next session needing attention",
        action: Action::NextAttention,
    },
    // Alt+s: proxy stats. Verified not used by claude (bundle grep: empty).
    Binding {
        code: KeyCode::Char('s'),
        mods: KeyModifiers::ALT,
        label: "proxy stats popup",
        action: Action::ProxyStats,
    },
    // Alt+g: "glance" / overview. Verified free: not in claude bundle,
    // not a readline binding, not a Ghostty default.
    Binding {
        code: KeyCode::Char('g'),
        mods: KeyModifiers::ALT,
        label: "overview of all sessions",
        action: Action::Overview,
    },
    // Alt+h: help. Verified free: not in claude bundle, not in Ghostty defaults.
    Binding {
        code: KeyCode::Char('h'),
        mods: KeyModifiers::ALT,
        label: "this help popup",
        action: Action::Help,
    },
    // Quit is last so it's always visible in the status bar even when truncated.
    Binding {
        code: KeyCode::Char('q'),
        mods: KeyModifiers::ALT,
        label: "quit",
        action: Action::Quit,
    },
];

/// The effective keymap: defaults + config overrides.
///
/// Owned by `App`; used for dispatch, help rendering, and status-bar hints.
/// Build with [`Keymap::build`].
#[derive(Debug, Clone)]
pub struct Keymap {
    bindings: Vec<Binding>,
}

impl Default for Keymap {
    fn default() -> Self {
        Keymap::build(&std::collections::BTreeMap::new(), &mut Vec::new())
    }
}

impl Keymap {
    /// Build from defaults + overrides. Collect notices for any problem.
    pub fn build(
        overrides: &std::collections::BTreeMap<String, String>,
        notices: &mut Vec<String>,
    ) -> Keymap {
        // Start from defaults (clone the static slice into an owned Vec).
        let mut bindings: Vec<Binding> = DEFAULT_BINDINGS
            .iter()
            .map(|b| Binding {
                code: b.code,
                mods: b.mods,
                label: b.label,
                action: b.action,
            })
            .collect();

        for (name, spec) in overrides {
            // Find the action by name.
            let action = DEFAULT_BINDINGS
                .iter()
                .find(|b| b.action.name() == name)
                .map(|b| b.action);
            let Some(action) = action else {
                notices.push(format!("config.toml [keys]: unknown action '{name}'"));
                continue;
            };
            let (code, mods) = match parse_key_spec(spec) {
                Ok(km) => km,
                Err(e) => {
                    notices.push(format!("config.toml [keys] '{name}': {e}"));
                    continue;
                }
            };
            // Collision check: reject if another action already claims this key.
            if let Some(existing) = bindings
                .iter()
                .find(|b| b.code == code && b.mods == mods && b.action != action)
            {
                notices.push(format!(
                    "config.toml [keys] '{name}': key {} already bound to '{}', skipping",
                    key_str(code, mods),
                    existing.action.name()
                ));
                continue;
            }
            // Replace the existing binding for this action.
            if let Some(b) = bindings.iter_mut().find(|b| b.action == action) {
                b.code = code;
                b.mods = mods;
            }
        }
        Keymap { bindings }
    }

    /// The action bound to `key`, if any. Releases never trigger actions.
    pub fn lookup(&self, key: &KeyEvent) -> Option<Action> {
        if key.kind == KeyEventKind::Release {
            return None;
        }
        self.bindings
            .iter()
            .find(|b| b.code == key.code && b.mods == key.modifiers)
            .map(|b| b.action)
    }

    /// Generate the help lines from the current effective bindings.
    /// Each entry is `(key_str, label)`.
    pub fn help_entries(&self) -> Vec<(String, &'static str)> {
        self.bindings
            .iter()
            .map(|b| (key_str(b.code, b.mods), b.label))
            .collect()
    }

    /// Generate the status-bar hints line from the current effective bindings.
    ///
    /// Reflects overrides so the hint is always accurate.
    #[allow(dead_code)]
    pub fn hints(&self) -> String {
        let parts: Vec<String> = self
            .bindings
            .iter()
            .map(|b| format!("{} {}", key_str(b.code, b.mods), b.label))
            .collect();
        parts.join(" · ")
    }
}

/// Parse a key spec string like `"alt+right"`, `"alt+g"`, `"ctrl+x"`.
///
/// Supported modifiers: `alt`, `ctrl`, `shift`. Supported keys: letter/digit
/// chars, `left`, `right`, `up`, `down`, `enter`, `esc`, `tab`, `backtab`,
/// `backspace`, `delete`, `home`, `end`, `pageup`, `pagedown`, `f1`–`f12`.
///
/// Multiple modifiers can be combined: `"alt+ctrl+x"`.
///
/// `shift+tab` is canonicalized to `KeyCode::BackTab`.
/// `alt+shift+<letter>` is canonicalized to the uppercase letter.
pub fn parse_key_spec(spec: &str) -> Result<(KeyCode, KeyModifiers), String> {
    let spec_lc = spec.trim().to_ascii_lowercase();
    let mut parts: Vec<&str> = spec_lc.split('+').collect();
    if parts.is_empty() {
        return Err("empty key spec".into());
    }
    let key_str_lc = parts.pop().unwrap();
    let mut mods = KeyModifiers::NONE;
    for m in &parts {
        match *m {
            "alt" | "meta" => mods |= KeyModifiers::ALT,
            "ctrl" | "control" => mods |= KeyModifiers::CONTROL,
            "shift" => mods |= KeyModifiers::SHIFT,
            other => return Err(format!("unknown modifier '{other}'")),
        }
    }
    // Canonicalize shift+tab → BackTab.
    if key_str_lc == "tab" && mods.contains(KeyModifiers::SHIFT) {
        return Ok((KeyCode::BackTab, mods - KeyModifiers::SHIFT));
    }
    let code = match key_str_lc {
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "enter" | "return" => KeyCode::Enter,
        "esc" | "escape" => KeyCode::Esc,
        "tab" => KeyCode::Tab,
        "backtab" => KeyCode::BackTab,
        "backspace" => KeyCode::Backspace,
        "delete" | "del" => KeyCode::Delete,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        "space" => KeyCode::Char(' '),
        "/" => KeyCode::Char('/'),
        "." => KeyCode::Char('.'),
        "," => KeyCode::Char(','),
        ";" => KeyCode::Char(';'),
        "'" => KeyCode::Char('\''),
        s if s.len() == 1 => {
            let c = s.chars().next().unwrap();
            if c.is_ascii_alphanumeric() || c.is_ascii_punctuation() {
                // alt+shift+<letter> → uppercase letter with ALT only.
                if mods.contains(KeyModifiers::SHIFT) && c.is_ascii_lowercase() {
                    let uc = c.to_ascii_uppercase();
                    return Ok((KeyCode::Char(uc), mods - KeyModifiers::SHIFT));
                }
                KeyCode::Char(c)
            } else {
                return Err(format!("unsupported key character '{c}'"));
            }
        }
        s if s.starts_with('f') => {
            let n: u8 = s[1..].parse().map_err(|_| format!("bad F-key '{s}'"))?;
            if n == 0 || n > 12 {
                return Err(format!("F-key number must be 1-12, got {n}"));
            }
            KeyCode::F(n)
        }
        other => return Err(format!("unknown key '{other}'")),
    };
    Ok((code, mods))
}

/// Human-readable key string for a binding (e.g. `"Alt+←"`, `"Alt+G"`).
pub fn key_str(code: KeyCode, mods: KeyModifiers) -> String {
    let mut parts = Vec::new();
    if mods.contains(KeyModifiers::CONTROL) {
        parts.push("Ctrl");
    }
    if mods.contains(KeyModifiers::ALT) {
        parts.push("Alt");
    }
    if mods.contains(KeyModifiers::SHIFT) {
        parts.push("Shift");
    }
    let key = match code {
        KeyCode::Left => "←".to_owned(),
        KeyCode::Right => "→".to_owned(),
        KeyCode::Up => "↑".to_owned(),
        KeyCode::Down => "↓".to_owned(),
        KeyCode::Enter => "Enter".to_owned(),
        KeyCode::Esc => "Esc".to_owned(),
        KeyCode::Tab => "Tab".to_owned(),
        KeyCode::BackTab => "Shift+Tab".to_owned(),
        KeyCode::Backspace => "Bksp".to_owned(),
        KeyCode::Delete => "Del".to_owned(),
        KeyCode::Home => "Home".to_owned(),
        KeyCode::End => "End".to_owned(),
        KeyCode::PageUp => "PgUp".to_owned(),
        KeyCode::PageDown => "PgDn".to_owned(),
        KeyCode::Char(' ') => "Space".to_owned(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::F(n) => format!("F{n}"),
        _ => "?".to_owned(),
    };
    parts.push(&key);
    parts.join("+")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventState;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn default_bindings_match_expected_keys() {
        let km = Keymap::default();
        assert_eq!(
            km.lookup(&key(KeyCode::Left, KeyModifiers::ALT)),
            Some(Action::PrevSession)
        );
        assert_eq!(
            km.lookup(&key(KeyCode::Right, KeyModifiers::ALT)),
            Some(Action::NextSession)
        );
        assert_eq!(
            km.lookup(&key(KeyCode::Char('q'), KeyModifiers::ALT)),
            Some(Action::Quit)
        );
        assert_eq!(
            km.lookup(&key(KeyCode::Char('a'), KeyModifiers::ALT)),
            Some(Action::NextAttention)
        );
        assert_eq!(
            km.lookup(&key(KeyCode::Char('g'), KeyModifiers::ALT)),
            Some(Action::Overview)
        );
        assert_eq!(
            km.lookup(&key(KeyCode::Char('h'), KeyModifiers::ALT)),
            Some(Action::Help)
        );
    }

    #[test]
    fn modifiers_must_match_exactly() {
        let km = Keymap::default();
        assert_eq!(
            km.lookup(&key(KeyCode::Left, KeyModifiers::ALT | KeyModifiers::SHIFT)),
            None
        );
        assert_eq!(km.lookup(&key(KeyCode::Left, KeyModifiers::NONE)), None);
        assert_eq!(
            km.lookup(&key(KeyCode::Char('n'), KeyModifiers::CONTROL)),
            None
        );
        assert_eq!(
            km.lookup(&key(
                KeyCode::Char('n'),
                KeyModifiers::ALT | KeyModifiers::CONTROL
            )),
            None
        );
    }

    #[test]
    fn releases_are_ignored() {
        let km = Keymap::default();
        let release = KeyEvent {
            code: KeyCode::Char('q'),
            modifiers: KeyModifiers::ALT,
            kind: KeyEventKind::Release,
            state: KeyEventState::NONE,
        };
        assert_eq!(km.lookup(&release), None);
    }

    // ── parse_key_spec ────────────────────────────────────────────────────────

    #[test]
    fn parse_alt_letter() {
        let (code, mods) = parse_key_spec("alt+g").unwrap();
        assert_eq!(code, KeyCode::Char('g'));
        assert_eq!(mods, KeyModifiers::ALT);
    }

    #[test]
    fn parse_alt_arrow() {
        let (code, mods) = parse_key_spec("alt+right").unwrap();
        assert_eq!(code, KeyCode::Right);
        assert_eq!(mods, KeyModifiers::ALT);
    }

    #[test]
    fn parse_ctrl_letter() {
        let (code, mods) = parse_key_spec("ctrl+x").unwrap();
        assert_eq!(code, KeyCode::Char('x'));
        assert_eq!(mods, KeyModifiers::CONTROL);
    }

    #[test]
    fn parse_multiple_modifiers() {
        let (code, mods) = parse_key_spec("alt+shift+enter").unwrap();
        assert_eq!(code, KeyCode::Enter);
        assert_eq!(mods, KeyModifiers::ALT | KeyModifiers::SHIFT);
    }

    #[test]
    fn parse_function_keys() {
        let (code, mods) = parse_key_spec("f5").unwrap();
        assert_eq!(code, KeyCode::F(5));
        assert_eq!(mods, KeyModifiers::NONE);
        let (code, mods) = parse_key_spec("alt+f12").unwrap();
        assert_eq!(code, KeyCode::F(12));
        assert_eq!(mods, KeyModifiers::ALT);
    }

    #[test]
    fn parse_special_keys() {
        assert_eq!(
            parse_key_spec("esc").unwrap(),
            (KeyCode::Esc, KeyModifiers::NONE)
        );
        assert_eq!(
            parse_key_spec("enter").unwrap(),
            (KeyCode::Enter, KeyModifiers::NONE)
        );
        assert_eq!(
            parse_key_spec("tab").unwrap(),
            (KeyCode::Tab, KeyModifiers::NONE)
        );
        assert_eq!(
            parse_key_spec("pageup").unwrap(),
            (KeyCode::PageUp, KeyModifiers::NONE)
        );
    }

    #[test]
    fn parse_shift_tab_becomes_backtab() {
        // shift+tab should canonicalize to BackTab (no SHIFT modifier).
        assert_eq!(
            parse_key_spec("shift+tab").unwrap(),
            (KeyCode::BackTab, KeyModifiers::NONE)
        );
        assert_eq!(
            parse_key_spec("alt+shift+tab").unwrap(),
            (KeyCode::BackTab, KeyModifiers::ALT)
        );
    }

    #[test]
    fn parse_alt_shift_letter_becomes_uppercase() {
        // alt+shift+g → Alt+G (uppercase, SHIFT removed)
        assert_eq!(
            parse_key_spec("alt+shift+g").unwrap(),
            (KeyCode::Char('G'), KeyModifiers::ALT)
        );
    }

    #[test]
    fn parse_invalid_key_returns_error() {
        assert!(parse_key_spec("").is_err());
        assert!(parse_key_spec("badmod+x").is_err());
        assert!(parse_key_spec("alt+zzz").is_err());
        assert!(parse_key_spec("alt+f0").is_err());
        assert!(parse_key_spec("alt+f13").is_err());
    }

    #[test]
    fn parse_case_insensitive() {
        let (code, mods) = parse_key_spec("ALT+G").unwrap();
        assert_eq!(code, KeyCode::Char('g'));
        assert_eq!(mods, KeyModifiers::ALT);
    }

    // ── Keymap::build ─────────────────────────────────────────────────────────

    #[test]
    fn collision_detection_emits_notice_and_skips() {
        let mut overrides = std::collections::BTreeMap::new();
        // Try to bind "quit" to alt+n, which is already "new_session".
        overrides.insert("quit".to_owned(), "alt+n".to_owned());
        let mut notices = Vec::new();
        let km = Keymap::build(&overrides, &mut notices);
        // Notice should mention the collision.
        assert!(!notices.is_empty(), "expected a collision notice");
        assert!(
            notices[0].contains("already bound"),
            "notice should mention collision: {:?}",
            notices
        );
        // The override must be rejected: alt+q still quits, alt+n still opens wizard.
        assert_eq!(
            km.lookup(&key(KeyCode::Char('q'), KeyModifiers::ALT)),
            Some(Action::Quit)
        );
        assert_eq!(
            km.lookup(&key(KeyCode::Char('n'), KeyModifiers::ALT)),
            Some(Action::NewSession)
        );
    }

    // ── help_entries ──────────────────────────────────────────────────────────

    #[test]
    fn help_entries_covers_all_default_bindings() {
        let km = Keymap::default();
        let entries = km.help_entries();
        // Every default binding must appear in the help.
        for b in DEFAULT_BINDINGS {
            assert!(
                entries.iter().any(|(_, label)| *label == b.label),
                "missing label: {}",
                b.label
            );
        }
    }

    #[test]
    fn hints_string_not_empty() {
        let km = Keymap::default();
        assert!(!km.hints().is_empty());
    }
}
