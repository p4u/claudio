//! Manager key bindings.
//!
//! Every key the manager intercepts (instead of forwarding to claude) is
//! defined in [`DEFAULT_BINDINGS`], which is the single source of truth for
//! both dispatch (`lookup`) and the generated help popup.
//!
//! Overrides from `config.toml` `[keys]` are applied at startup by
//! `apply_overrides`, which replaces the action's binding. Unknown action
//! names and unparseable key specs produce a notice and are ignored.
//!
//! Matching is exact: Alt+Shift+← is not Alt+←.

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

/// One entry in the binding table: key + human label + action.
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
    Binding {
        code: KeyCode::Char('q'),
        mods: KeyModifiers::ALT,
        label: "quit UI (sessions keep running)",
        action: Action::Quit,
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
];

/// A runtime binding after config overrides have been applied.
#[derive(Debug, Clone)]
struct RuntimeBinding {
    code: KeyCode,
    mods: KeyModifiers,
    action: Action,
}

/// The effective binding table, populated once at startup.
///
/// We use a `std::sync::OnceLock` so the lookup function needs no state
/// parameter, matching the existing call sites.
static EFFECTIVE: std::sync::OnceLock<Vec<RuntimeBinding>> = std::sync::OnceLock::new();

/// Initialize the effective bindings from defaults + config overrides.
///
/// Must be called once, at startup, before any `lookup` calls. Safe to call
/// multiple times (subsequent calls are no-ops — `OnceLock` semantics).
pub fn init(overrides: &std::collections::BTreeMap<String, String>) -> Vec<String> {
    let mut notices = Vec::new();
    let bindings = EFFECTIVE.get_or_init(|| build_bindings(overrides, &mut notices));
    // If already initialized (test-only), we can't change it; just validate.
    if EFFECTIVE.get().is_some() && notices.is_empty() {
        let _ = bindings;
    }
    notices
}

fn build_bindings(
    overrides: &std::collections::BTreeMap<String, String>,
    notices: &mut Vec<String>,
) -> Vec<RuntimeBinding> {
    // Start from defaults.
    let mut bindings: Vec<RuntimeBinding> = DEFAULT_BINDINGS
        .iter()
        .map(|b| RuntimeBinding { code: b.code, mods: b.mods, action: b.action })
        .collect();

    for (name, spec) in overrides {
        // Find the action by name.
        let action = DEFAULT_BINDINGS.iter().find(|b| b.action.name() == name).map(|b| b.action);
        let Some(action) = action else {
            notices.push(format!("config.toml [keys]: unknown action '{name}'"));
            continue;
        };
        match parse_key_spec(spec) {
            Ok((code, mods)) => {
                // Replace the existing binding for this action.
                if let Some(b) = bindings.iter_mut().find(|b| b.action == action) {
                    b.code = code;
                    b.mods = mods;
                }
            }
            Err(e) => {
                notices.push(format!("config.toml [keys] '{name}': {e}"));
            }
        }
    }
    bindings
}

/// Parse a key spec string like `"alt+right"`, `"alt+g"`, `"ctrl+x"`.
///
/// Supported modifiers: `alt`, `ctrl`, `shift`. Supported keys: letter/digit
/// chars, `left`, `right`, `up`, `down`, `enter`, `esc`, `tab`, `backspace`,
/// `delete`, `home`, `end`, `pageup`, `pagedown`, `f1`–`f12`.
///
/// Multiple modifiers can be combined: `"alt+ctrl+x"`.
pub fn parse_key_spec(spec: &str) -> Result<(KeyCode, KeyModifiers), String> {
    let spec = spec.trim().to_ascii_lowercase();
    let mut parts: Vec<&str> = spec.split('+').collect();
    if parts.is_empty() {
        return Err("empty key spec".into());
    }
    let key_str = parts.pop().unwrap();
    let mut mods = KeyModifiers::NONE;
    for m in &parts {
        match *m {
            "alt" | "meta" => mods |= KeyModifiers::ALT,
            "ctrl" | "control" => mods |= KeyModifiers::CONTROL,
            "shift" => mods |= KeyModifiers::SHIFT,
            other => return Err(format!("unknown modifier '{other}'")),
        }
    }
    let code = match key_str {
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "enter" | "return" => KeyCode::Enter,
        "esc" | "escape" => KeyCode::Esc,
        "tab" => KeyCode::Tab,
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

/// The action bound to `key`, if any. Releases never trigger actions.
///
/// Uses the effective binding table (defaults + config overrides). Falls back
/// to defaults when `init` has not been called.
pub fn lookup(key: &KeyEvent) -> Option<Action> {
    if key.kind == KeyEventKind::Release {
        return None;
    }
    // Use overridden table when available; fall back to defaults.
    if let Some(bindings) = EFFECTIVE.get() {
        return bindings
            .iter()
            .find(|b| b.code == key.code && b.mods == key.modifiers)
            .map(|b| b.action);
    }
    DEFAULT_BINDINGS
        .iter()
        .find(|b| b.code == key.code && b.mods == key.modifiers)
        .map(|b| b.action)
}

/// Human-readable key string for a binding (e.g. `"Alt+←"`, `"Alt+g"`).
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
        KeyCode::Backspace => "Bksp".to_owned(),
        KeyCode::Delete => "Del".to_owned(),
        KeyCode::Home => "Home".to_owned(),
        KeyCode::End => "End".to_owned(),
        KeyCode::PageUp => "PgUp".to_owned(),
        KeyCode::PageDown => "PgDn".to_owned(),
        KeyCode::Char(' ') => "Space".to_owned(),
        KeyCode::Char(c) => c.to_uppercase().collect(),
        KeyCode::F(n) => format!("F{n}"),
        _ => "?".to_owned(),
    };
    parts.push(&key);
    parts.join("+")
}

/// Generate the help lines from the current effective bindings.
/// Each entry is `(key_str, label)`.
pub fn help_entries() -> Vec<(String, &'static str)> {
    // Use effective bindings when available so overrides show up in help.
    if let Some(bindings) = EFFECTIVE.get() {
        bindings
            .iter()
            .filter_map(|rb| {
                let label = DEFAULT_BINDINGS
                    .iter()
                    .find(|b| b.action == rb.action)
                    .map(|b| b.label)?;
                Some((key_str(rb.code, rb.mods), label))
            })
            .collect()
    } else {
        DEFAULT_BINDINGS
            .iter()
            .map(|b| (key_str(b.code, b.mods), b.label))
            .collect()
    }
}

/// Key hints shown in the status bar (compact, rightmost survives longest).
pub const HINTS: &str =
    "Alt+←/→ switch · Alt+n new · Alt+r rename · Alt+x close · Alt+s proxy · Alt+g overview · Alt+h help · Alt+q quit";

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventState;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn default_bindings_match_expected_keys() {
        assert_eq!(lookup(&key(KeyCode::Left, KeyModifiers::ALT)), Some(Action::PrevSession));
        assert_eq!(lookup(&key(KeyCode::Right, KeyModifiers::ALT)), Some(Action::NextSession));
        assert_eq!(lookup(&key(KeyCode::Char('q'), KeyModifiers::ALT)), Some(Action::Quit));
        assert_eq!(lookup(&key(KeyCode::Char('a'), KeyModifiers::ALT)), Some(Action::NextAttention));
        assert_eq!(lookup(&key(KeyCode::Char('g'), KeyModifiers::ALT)), Some(Action::Overview));
        assert_eq!(lookup(&key(KeyCode::Char('h'), KeyModifiers::ALT)), Some(Action::Help));
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
        assert_eq!(parse_key_spec("esc").unwrap(), (KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(parse_key_spec("enter").unwrap(), (KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(parse_key_spec("tab").unwrap(), (KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(parse_key_spec("pageup").unwrap(), (KeyCode::PageUp, KeyModifiers::NONE));
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

    // ── help_entries ──────────────────────────────────────────────────────────

    #[test]
    fn help_entries_covers_all_default_bindings() {
        let entries = help_entries();
        // Every default binding must appear in the help.
        for b in DEFAULT_BINDINGS {
            assert!(
                entries.iter().any(|(_, label)| *label == b.label),
                "missing help entry for '{}'",
                b.label
            );
        }
    }

    #[test]
    fn build_bindings_rejects_unknown_actions_and_bad_specs() {
        let mut notices = Vec::new();
        let mut overrides = std::collections::BTreeMap::new();
        overrides.insert("nonexistent_action".into(), "alt+z".into());
        overrides.insert("overview".into(), "badmod+x".into());
        overrides.insert("help".into(), "alt+z".into()); // valid override
        let bindings = build_bindings(&overrides, &mut notices);
        // Two notices: one for unknown action, one for bad spec.
        assert_eq!(notices.len(), 2, "expected exactly 2 notices, got: {notices:?}");
        // The help binding should have changed to alt+z.
        let help_binding = bindings.iter().find(|b| b.action == Action::Help).unwrap();
        assert_eq!(help_binding.code, KeyCode::Char('z'));
        assert_eq!(help_binding.mods, KeyModifiers::ALT);
    }
}
