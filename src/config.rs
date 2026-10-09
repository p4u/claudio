//! Global configuration from `~/.config/claudio/config.toml`.
//!
//! Only the sections claudio itself defines are parsed here; the proxy section
//! lives in [`crate::proxy::profile`], which reads the same file. Unknown
//! sections are preserved on write via a `flatten` map.
//!
//! ```toml
//! [ui]
//! notify = true   # desktop notifications for background sessions
//!
//! [keys]
//! overview    = "alt+g"
//! help        = "alt+h"
//! next_session = "alt+right"
//! # …any action from keymap::Action
//! ```

use std::collections::BTreeMap;
use std::io;

use serde::{Deserialize, Serialize};

use crate::paths;

// ── Data model ────────────────────────────────────────────────────────────────

/// The `[ui]` section of `config.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiSection {
    /// Whether to emit desktop notifications (OSC 9 + BEL) when a background
    /// session enters NeedsApproval / NeedsInput / Error.
    #[serde(default = "default_notify")]
    pub notify: bool,
}

fn default_notify() -> bool {
    true
}

impl Default for UiSection {
    fn default() -> Self {
        UiSection { notify: true }
    }
}

/// The `[keys]` section of `config.toml`: action name → key spec string.
///
/// Each entry overrides the default binding for that action. Unknown action
/// names and invalid key specs are ignored with a notice.
pub type KeysSection = BTreeMap<String, String>;

/// The subset of `config.toml` that this module owns. Other sections (e.g.
/// `[proxy]`) are preserved via `extra` on round-trip.
#[derive(Debug, Default, Serialize, Deserialize)]
struct ConfigFile {
    #[serde(default)]
    pub ui: Option<UiSection>,
    #[serde(default)]
    pub keys: Option<KeysSection>,
    /// All other TOML keys — preserved on write.
    #[serde(flatten)]
    extra: toml::Table,
}

/// Loaded claudio configuration.
#[derive(Debug, Clone, Default)]
pub struct Config {
    pub ui: UiSection,
    /// Raw key overrides (action name → key spec). Parsed into bindings by
    /// `keymap::apply_overrides`.
    pub keys: KeysSection,
}

// ── Loading ───────────────────────────────────────────────────────────────────

/// Load the configuration from `config.toml`. A missing file is not an error;
/// an unreadable or invalid file is logged to stderr and defaults are used.
pub fn load() -> Config {
    match try_load() {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("claudio: config.toml: {e} (using defaults)");
            Config::default()
        }
    }
}

fn try_load() -> io::Result<Config> {
    let path = paths::config_file();
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Config::default()),
        Err(e) => return Err(e),
    };
    let file: ConfigFile = toml::from_str(&String::from_utf8_lossy(&bytes))
        .map_err(|e| io::Error::other(format!("parse error: {e}")))?;
    Ok(Config {
        ui: file.ui.unwrap_or_default(),
        keys: file.keys.unwrap_or_default(),
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_gives_defaults() {
        let dir =
            std::env::temp_dir().join(format!("claudio-cfg-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let cfg = load();
        assert!(cfg.ui.notify);
        assert!(cfg.keys.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parses_ui_and_keys_sections() {
        let toml = r#"
[ui]
notify = false

[keys]
overview = "alt+g"
help = "alt+h"

[proxy]
default = "x"
"#;
        let file: ConfigFile = toml::from_str(toml).unwrap();
        let ui = file.ui.unwrap();
        assert!(!ui.notify);
        let keys = file.keys.unwrap();
        assert_eq!(keys.get("overview").map(String::as_str), Some("alt+g"));
        assert_eq!(keys.get("help").map(String::as_str), Some("alt+h"));
        // [proxy] survives in extra.
        assert!(file.extra.contains_key("proxy"));
    }
}
