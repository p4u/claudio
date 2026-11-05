//! Global configuration from `~/.config/claudio/config.toml`.
//!
//! Only the sections claudio itself defines are parsed here; the proxy section
//! lives in [`crate::proxy::profile`], which reads the same file. Unknown
//! sections are preserved on write via a `flatten` map.
//!
//! ```toml
//! [ui]
//! notify = true          # desktop notifications for background sessions
//! copy_on_select = true  # drag in a pane to select and copy (OSC 52)
//!
//! [claude]
//! allow_skip_permissions = true   # pass --allow-dangerously-skip-permissions
//! update_check = "ask"            # local claude vs its release channel
//! remote_check = "ask"            # remote claude vs the local version
//! latex = true                    # ask claude to wrap math in <latex> tags
//!                                 # (read at daemon start: `claudio daemon restart`)
//!
//! [extensions]
//! diagram = false                 # any extension, built-in or external, by name
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
    /// Copy on select: a left-button drag in a session pane selects text
    /// (double-click a word, triple-click a row) and copies it to the
    /// clipboard through OSC 52. Only while the session has not turned on
    /// mouse reporting; those clicks go to the session as before.
    #[serde(default = "default_true")]
    pub copy_on_select: bool,
}

fn default_notify() -> bool {
    true
}

impl Default for UiSection {
    fn default() -> Self {
        UiSection {
            notify: true,
            copy_on_select: true,
        }
    }
}

/// The `[update]` section of `config.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateSection {
    /// Whether to check for newer releases in the background (at most once per
    /// 24 h).  Set to `false` or export `CLAUDIO_NO_UPDATE_CHECK=1` to opt out.
    #[serde(default = "default_update_check")]
    pub check: bool,
}

fn default_update_check() -> bool {
    true
}

impl Default for UpdateSection {
    fn default() -> Self {
        UpdateSection { check: true }
    }
}

/// What to do when a claude install is out of date.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdatePolicy {
    /// Ask before updating.
    #[default]
    Ask,
    /// Update without asking (a notice says so).
    Auto,
    /// Never check.
    Off,
}

/// The `[claude]` section of `config.toml`: how claudio launches and maintains
/// the claude CLI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeSection {
    /// Start claude with `--allow-dangerously-skip-permissions`, so bypass
    /// mode is selectable with Shift+Tab (it does not start in it).
    #[serde(default = "default_true")]
    pub allow_skip_permissions: bool,
    /// Compare the local claude with its release channel (once a day).
    #[serde(default)]
    pub update_check: UpdatePolicy,
    /// Compare a remote host's claude with the local version on connect.
    #[serde(default)]
    pub remote_check: UpdatePolicy,
    /// Tell every claude session this host's daemon starts to wrap math in
    /// `<latex>…</latex>`, which the manager renders. On unless turned off.
    /// Read when the daemon starts, from the config of the host that runs
    /// the session.
    #[serde(default = "default_true")]
    pub latex: bool,
}

fn default_true() -> bool {
    true
}

impl Default for ClaudeSection {
    fn default() -> Self {
        ClaudeSection {
            allow_skip_permissions: true,
            update_check: UpdatePolicy::Ask,
            remote_check: UpdatePolicy::Ask,
            latex: true,
        }
    }
}

/// The `[keys]` section of `config.toml`: action name → key spec string.
///
/// Each entry overrides the default binding for that action. Unknown action
/// names and invalid key specs are ignored with a notice.
pub type KeysSection = BTreeMap<String, String>;

/// The `[extensions]` section of `config.toml`: extension name → on/off
/// (every extension is on unless listed as `false`).
pub type ExtensionsSection = BTreeMap<String, bool>;

/// The subset of `config.toml` that this module owns. Other sections (e.g.
/// `[proxy]`) are preserved via `extra` on round-trip.
#[derive(Debug, Default, Serialize, Deserialize)]
struct ConfigFile {
    #[serde(default)]
    pub ui: Option<UiSection>,
    #[serde(default)]
    pub keys: Option<KeysSection>,
    #[serde(default)]
    pub update: Option<UpdateSection>,
    #[serde(default)]
    pub claude: Option<ClaudeSection>,
    #[serde(default)]
    pub extensions: Option<ExtensionsSection>,
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
    pub update: UpdateSection,
    pub claude: ClaudeSection,
    pub extensions: ExtensionsSection,
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
        let loc = e
            .to_string()
            .lines()
            .next()
            .unwrap_or("TOML parse error")
            .to_owned();
        io::Error::other(format!("{loc} (content redacted)"))
    })?;
    Ok(Config {
        ui: file.ui.unwrap_or_default(),
        keys: file.keys.unwrap_or_default(),
        update: file.update.unwrap_or_default(),
        claude: file.claude.unwrap_or_default(),
        extensions: file.extensions.unwrap_or_default(),
    })
}

// ── Editing ───────────────────────────────────────────────────────────────────

/// Why [`save_extension`] wrote nothing.
#[derive(Debug)]
pub enum SaveError {
    /// The file does not parse as TOML, before or after the edit.
    InvalidToml,
    /// The name is not an extension's (`^[a-z][a-z0-9-]{0,31}$`).
    BadName,
    Io(io::Error),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::InvalidToml => write!(f, "config.toml is not valid TOML"),
            SaveError::BadName => write!(f, "not an extension name"),
            SaveError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl From<io::Error> for SaveError {
    fn from(e: io::Error) -> Self {
        SaveError::Io(e)
    }
}

/// `^[a-z][a-z0-9-]{0,31}$`: what an extension's name looks like, so it
/// needs no quoting as a TOML key.
fn extension_name_ok(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= 32
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// `line` as its content and its line ending (`\r\n`, `\n` or none).
fn split_eol(line: &str) -> (&str, &str) {
    if let Some(content) = line.strip_suffix("\r\n") {
        (content, "\r\n")
    } else if let Some(content) = line.strip_suffix('\n') {
        (content, "\n")
    } else {
        (line, "")
    }
}

/// The table a header line opens (`[ name ]` → `name`, unquoted), `[[`
/// for an array of tables, or `None` when `content` is not a header.
fn header(content: &str) -> Option<String> {
    let t = content.trim_start();
    if t.starts_with("[[") {
        return Some("[[".to_owned());
    }
    let inner = t.strip_prefix('[')?;
    let end = inner.find(']')?;
    let rest = inner[end + 1..].trim_start();
    if !(rest.is_empty() || rest.starts_with('#')) {
        return None;
    }
    let name = inner[..end].trim();
    let unquoted = ['"', '\'']
        .iter()
        .find_map(|q| name.strip_prefix(*q).and_then(|n| n.strip_suffix(*q)))
        .unwrap_or(name);
    Some(unquoted.to_owned())
}

/// `content` with key `name`'s value set to `on`, keeping its
/// indentation, the key as written and a trailing comment; `None` when
/// the line is not `name`'s.
fn replace_key(content: &str, name: &str, on: bool) -> Option<String> {
    let t = content.trim_start();
    let indent = &content[..content.len() - t.len()];
    let forms = [name.to_owned(), format!("\"{name}\""), format!("'{name}'")];
    let key = forms.iter().find(|k| {
        t.strip_prefix(k.as_str())
            .is_some_and(|rest| rest.trim_start().starts_with('='))
    })?;
    let rest = &t[key.len()..];
    let eq = rest.find('=')?;
    let after = &rest[eq + 1..];
    let value = after.trim_start();
    let spaces = &after[..after.len() - value.len()];
    let end = value
        .find(|c: char| c.is_whitespace() || c == '#')
        .unwrap_or(value.len());
    let tail = &value[end..];
    Some(format!("{indent}{key}{}{spaces}{on}{tail}", &rest[..=eq]))
}

/// Toggle the multi-line string `content` opens or closes (`"""`, `'''`).
fn track_multiline(content: &str, open: &mut Option<&'static str>) {
    match *open {
        Some(delim) => {
            if content.matches(delim).count() % 2 == 1 {
                *open = None;
            }
        }
        None => {
            for delim in ["\"\"\"", "'''"] {
                if content.matches(delim).count() % 2 == 1 {
                    *open = Some(delim);
                    break;
                }
            }
        }
    }
}

/// `text` (a `config.toml`) with `[extensions] name = on`, edited
/// minimally: an existing `name = …` line of the `[extensions]` table is
/// replaced (its indentation and trailing comment kept), else the key goes
/// at the end of that table, else the table is appended. Every other byte
/// stays as it was; the line ending follows the file's (`\r\n` or `\n`).
/// `name` must match `^[a-z][a-z0-9-]{0,31}$` (no quoting needed).
pub fn set_extension(text: &str, name: &str, on: bool) -> String {
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<String> = text.split_inclusive('\n').map(str::to_owned).collect();
    let mut table: Option<String> = None;
    let mut multiline: Option<&'static str> = None;
    // The header or the last key line of `[extensions]`.
    let mut last: Option<usize> = None;
    for i in 0..lines.len() {
        let line = lines[i].clone();
        let (content, ending) = split_eol(&line);
        let continued = multiline.is_some();
        track_multiline(content, &mut multiline);
        let ours = table.as_deref() == Some("extensions");
        if continued {
            if ours {
                last = Some(i);
            }
            continue;
        }
        if let Some(name) = header(content) {
            if name == "extensions" && last.is_none() {
                last = Some(i);
            }
            table = Some(name);
            continue;
        }
        if !ours {
            continue;
        }
        let t = content.trim_start();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if let Some(new) = replace_key(content, name, on) {
            lines[i] = format!("{new}{ending}");
            return lines.concat();
        }
        last = Some(i);
    }
    let entry = format!("{name} = {on}{eol}");
    match last {
        Some(i) => {
            if !lines[i].ends_with('\n') {
                lines[i].push_str(eol);
            }
            lines.insert(i + 1, entry);
            lines.concat()
        }
        None => {
            let mut out = text.to_owned();
            if !out.is_empty() {
                if !out.ends_with('\n') {
                    out.push_str(eol);
                }
                out.push_str(eol);
            }
            out.push_str(&format!("[extensions]{eol}{entry}"));
            out
        }
    }
}

/// Set `[extensions] name = on` in the config file at `path` (through a
/// symlink, to its target) with [`set_extension`]; written atomically,
/// keeping the file's mode (0600 when it is created). Nothing is written
/// when the file does not parse as TOML, before or after the edit.
pub fn save_extension(path: &Path, name: &str, on: bool) -> Result<(), SaveError> {
    use std::os::unix::fs::PermissionsExt;
    if !extension_name_ok(name) {
        return Err(SaveError::BadName);
    }
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let (text, mode) = match std::fs::read(&path) {
        Ok(bytes) => {
            let text = String::from_utf8(bytes).map_err(|_| SaveError::InvalidToml)?;
            let mode = std::fs::metadata(&path)?.permissions().mode() & 0o7777;
            (text, Some(mode))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => (String::new(), None),
        Err(e) => return Err(e.into()),
    };
    if toml::from_str::<toml::Table>(&text).is_err() {
        return Err(SaveError::InvalidToml);
    }
    let edited = set_extension(&text, name, on);
    let table: toml::Table = toml::from_str(&edited).map_err(|_| SaveError::InvalidToml)?;
    let set = table
        .get("extensions")
        .and_then(|e| e.get(name))
        .and_then(toml::Value::as_bool);
    if set != Some(on) {
        return Err(SaveError::InvalidToml);
    }
    paths::write_atomic(&path, edited.as_bytes())?;
    if let Some(mode) = mode.filter(|m| *m != 0o600) {
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Uses load_from with an explicit temp dir to avoid mutating XDG_CONFIG_HOME.
    #[test]
    fn missing_file_gives_defaults() {
        let dir = std::env::temp_dir().join(format!("claudio-cfg-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml"); // does not exist
        let cfg = load_from(&path);
        assert!(cfg.ui.notify);
        assert!(cfg.ui.copy_on_select);
        assert!(cfg.keys.is_empty());
        assert!(cfg.claude.allow_skip_permissions);
        assert_eq!(cfg.claude.update_check, UpdatePolicy::Ask);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parses_claude_section() {
        let dir = std::env::temp_dir().join(format!("claudio-cfg-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            "[claude]\nallow_skip_permissions = false\nupdate_check = \"auto\"\nremote_check = \"off\"\n",
        )
        .unwrap();
        let cfg = load_from(&path);
        assert!(!cfg.claude.allow_skip_permissions);
        assert_eq!(cfg.claude.update_check, UpdatePolicy::Auto);
        assert_eq!(cfg.claude.remote_check, UpdatePolicy::Off);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn latex_is_on_unless_turned_off() {
        let file: ConfigFile = toml::from_str("[claude]\nupdate_check = \"off\"\n").unwrap();
        assert!(file.claude.unwrap().latex);
        assert!(ClaudeSection::default().latex);
        let file: ConfigFile = toml::from_str("[claude]\nlatex = false\n").unwrap();
        let claude = file.claude.unwrap();
        assert!(!claude.latex);
        // The other keys keep their defaults.
        assert!(claude.allow_skip_permissions);
        assert_eq!(claude.update_check, UpdatePolicy::Ask);
    }

    #[test]
    fn parses_ui_and_keys_sections() {
        let toml = r#"
[ui]
notify = false
copy_on_select = false

[keys]
overview = "alt+g"
help = "alt+h"

[proxy]
default = "x"
"#;
        let file: ConfigFile = toml::from_str(toml).unwrap();
        let ui = file.ui.unwrap();
        assert!(!ui.notify);
        assert!(!ui.copy_on_select);
        let keys = file.keys.unwrap();
        assert_eq!(keys.get("overview").map(String::as_str), Some("alt+g"));
        assert_eq!(keys.get("help").map(String::as_str), Some("alt+h"));
        // [proxy] survives in extra.
        assert!(file.extra.contains_key("proxy"));
    }

    /// TOML errors must not embed source content (which may contain
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
        assert!(
            !err.to_string().contains("MYVERYSECRETTOKEN_UNQUOTED"),
            "error leaked secret: {}",
            err
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn set_extension_replaces_the_line_keeping_indentation_and_comment() {
        let text = "# mine\n[extensions]\n  diagram   =  true   # keep me\nsearch = true\n";
        assert_eq!(
            set_extension(text, "diagram", false),
            "# mine\n[extensions]\n  diagram   =  false   # keep me\nsearch = true\n"
        );
        // The quoted-key form too, keeping its spelling.
        let text = "[extensions]\n\"diagram\" = false\n'search'=false#x\n";
        assert_eq!(
            set_extension(text, "diagram", true),
            "[extensions]\n\"diagram\" = true\n'search'=false#x\n"
        );
        assert_eq!(
            set_extension(text, "search", true),
            "[extensions]\n\"diagram\" = false\n'search'=true#x\n"
        );
    }

    #[test]
    fn set_extension_inserts_at_the_end_of_an_existing_table() {
        let text = "[extensions]\ndiagram = false\n\n# keys\n[keys]\nhelp = \"alt+h\"\n";
        assert_eq!(
            set_extension(text, "search", false),
            "[extensions]\ndiagram = false\nsearch = false\n\n# keys\n[keys]\nhelp = \"alt+h\"\n"
        );
        // An empty table, last in the file without a final newline.
        assert_eq!(
            set_extension("[ui]\nnotify = true\n[ extensions ]", "latex", true),
            "[ui]\nnotify = true\n[ extensions ]\nlatex = true\n"
        );
        // A subtable is another table.
        let text = "[extensions.x]\ndiagram = true\n";
        assert_eq!(
            set_extension(text, "diagram", false),
            "[extensions.x]\ndiagram = true\n\n[extensions]\ndiagram = false\n"
        );
    }

    #[test]
    fn set_extension_appends_the_table_when_missing() {
        assert_eq!(
            set_extension("", "diagram", false),
            "[extensions]\ndiagram = false\n"
        );
        assert_eq!(
            set_extension("[ui]\nnotify = true\n", "diagram", false),
            "[ui]\nnotify = true\n\n[extensions]\ndiagram = false\n"
        );
        // No trailing newline: one is added first.
        assert_eq!(
            set_extension("[ui]\nnotify = true", "diagram", false),
            "[ui]\nnotify = true\n\n[extensions]\ndiagram = false\n"
        );
    }

    #[test]
    fn set_extension_keeps_crlf_line_endings() {
        let text = "[extensions]\r\ndiagram = true # c\r\n[keys]\r\n";
        assert_eq!(
            set_extension(text, "diagram", false),
            "[extensions]\r\ndiagram = false # c\r\n[keys]\r\n"
        );
        assert_eq!(
            set_extension(text, "search", false),
            "[extensions]\r\ndiagram = true # c\r\nsearch = false\r\n[keys]\r\n"
        );
        assert_eq!(
            set_extension("[ui]\r\nnotify = true\r\n", "x", true),
            "[ui]\r\nnotify = true\r\n\r\n[extensions]\r\nx = true\r\n"
        );
    }

    #[test]
    fn set_extension_leaves_everything_else_byte_identical() {
        let text = "# top comment\n\
                    diagram = true   # a top-level key, not ours\n\
                    \n\
                    [proxy]\n\
                    default = \"work\"  # the default profile\n\
                    [proxy.profiles.work]\n\
                    url = \"https://proxy.example\"\n\
                    token = \"s3cr3t-T0KEN\"   # never touch\n\
                    \n\
                    [ui]\n\
                    extensions = true\n\
                    \n\
                    [extensions]\n\
                    # snippets are handy\n\
                    snippets = true\n\
                    \n\
                    [keys]\n\
                    help = \"alt+h\"\n";
        let out = set_extension(text, "diagram", false);
        let expected = text.replace("snippets = true\n", "snippets = true\ndiagram = false\n");
        assert_eq!(out, expected);
        let out = set_extension(text, "snippets", false);
        assert_eq!(out, text.replace("snippets = true\n", "snippets = false\n"));
        // The other sections are untouched, byte for byte.
        let proxy =
            |t: &str| t[t.find("[proxy]").unwrap()..t.find("[extensions]").unwrap()].to_owned();
        assert_eq!(proxy(&out), proxy(text));
        let table: toml::Table = out.parse().unwrap();
        assert_eq!(table["ui"]["extensions"].as_bool(), Some(true));
        assert_eq!(table["diagram"].as_bool(), Some(true));
    }

    fn scratch_config(text: Option<&str>) -> (std::path::PathBuf, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("claudio-cfg-ext-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        if let Some(text) = text {
            std::fs::write(&path, text).unwrap();
        }
        (dir, path)
    }

    #[test]
    fn save_extension_writes_keeping_the_mode() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, path) = scratch_config(Some("# mine\n[claude]\nlatex = true\n"));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        save_extension(&path, "diagram", false).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            text,
            "# mine\n[claude]\nlatex = true\n\n[extensions]\ndiagram = false\n"
        );
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640);
        assert_eq!(load_from(&path).extensions.get("diagram"), Some(&false));
        // A missing file is created, private.
        std::fs::remove_file(&path).unwrap();
        save_extension(&path, "search", true).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[extensions]\nsearch = true\n"
        );
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_extension_refuses_invalid_toml_and_writes_nothing() {
        let bad = "[proxy\ntoken = \"s3cr3t\"\n";
        let (dir, path) = scratch_config(Some(bad));
        let err = save_extension(&path, "diagram", false).unwrap_err();
        assert_eq!(err.to_string(), "config.toml is not valid TOML");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), bad);
        // An edit that would not parse (a dotted `extensions` key already
        // defines the table) is refused too.
        let dotted = "extensions.diagram = true\n";
        std::fs::write(&path, dotted).unwrap();
        let err = save_extension(&path, "search", false).unwrap_err();
        assert_eq!(err.to_string(), "config.toml is not valid TOML");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), dotted);
        // A name that is not an extension's is refused.
        assert!(save_extension(&path, "bad name", true).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn extensions_section_turns_extensions_on_and_off() {
        let dir = std::env::temp_dir().join(format!("claudio-cfg-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "[extensions]\ndiagram = false\nkeys = true\n").unwrap();
        let cfg = load_from(&path);
        assert_eq!(cfg.extensions.get("diagram"), Some(&false));
        assert_eq!(cfg.extensions.get("keys"), Some(&true));
        assert!(load_from(&dir.join("missing.toml")).extensions.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
