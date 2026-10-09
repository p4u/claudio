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
use std::path::Path;

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

/// Load the configuration from the default `config.toml`. A missing file is
/// not an error; an unreadable or invalid file is logged to stderr and
/// defaults are used. Parse errors do NOT include source excerpts (which may
/// contain secrets from the `[proxy]` section).
pub fn load() -> Config {
    match try_load_from(&paths::config_file()) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("claudio: config.toml: {e} (using defaults)");
            Config::default()
        }
    }
}

/// Load configuration from an explicit path (used in tests to avoid mutating
/// the global `XDG_CONFIG_HOME`).
#[allow(dead_code)]
pub fn load_from(path: &Path) -> Config {
    match try_load_from(path) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("claudio: config.toml: {e} (using defaults)");
            Config::default()
        }
    }
}

fn try_load_from(path: &Path) -> io::Result<Config> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Config::default()),
        Err(e) => return Err(e),
    };
    let file: ConfigFile = toml::from_str(&String::from_utf8_lossy(&bytes)).map_err(|e| {
        // Keep only the first line of the TOML error (location), never the source
        // excerpt which could contain a token from the [proxy] section.
        let loc = e.to_string().lines().next().unwrap_or("TOML parse error").to_owned();
        io::Error::other(format!("{loc} (content redacted)"))
    })?;
    Ok(Config {
        ui: file.ui.unwrap_or_default(),
        keys: file.keys.unwrap_or_default(),
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Uses load_from with an explicit temp dir to avoid mutating XDG_CONFIG_HOME.
    #[test]
    fn missing_file_gives_defaults() {
        let dir =
            std::env::temp_dir().join(format!("claudio-cfg-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml"); // does not exist
        let cfg = load_from(&path);
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

    /// Astra #8: TOML errors must not embed source content (which may contain
    /// a token from the [proxy] section).
    #[test]
    fn toml_error_does_not_include_source_content() {
        let dir =
            std::env::temp_dir().join(format!("claudio-cfg-toml-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");

        // Malformed TOML that would expose "MYVERYSECRETTOKEN" in a naive error.
        let bad = "[proxy.profiles.x]\ntoken = MYVERYSECRETTOKEN_UNQUOTED\n";
        std::fs::write(&path, bad).unwrap();

        let cfg = load_from(&path); // falls back to defaults, logs to stderr
        // The config returned is defaults (no panic).
        assert!(cfg.ui.notify);
        // We can't easily capture stderr here, but we verify the
        // try_load_from error message via direct call.
        let err = try_load_from(&path).unwrap_err();
        assert!(!err.to_string().contains("MYVERYSECRETTOKEN_UNQUOTED"),
            "error leaked secret: {}", err);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
