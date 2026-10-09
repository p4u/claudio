//! Proxy profiles: URL + token pairs stored in `~/.config/claudio/config.toml`.
//!
//! **Tokens are never in argv, never in logs, never in state.json.**
//! state.json stores only the profile *name*. `Debug` impls redact the token.
//!
//! Config file format:
//! ```toml
//! [proxy]
//! default = "vocdoni"
//! [proxy.profiles.vocdoni]
//! url   = "https://claude.vocdoni.net"
//! token = "…"
//! ```
//!
//! The env var `CLAUDIO_PROXY_URL=<token>@<host>` creates an ephemeral profile
//! named `"env"` and makes it the default for this run.

use std::collections::BTreeMap;
use std::fmt;
use std::io;

use serde::{Deserialize, Serialize};

use crate::paths;

// ── Data model ────────────────────────────────────────────────────────────────

/// A single proxy profile: a base URL and a bearer token.
#[derive(Clone, Serialize, Deserialize)]
pub struct Profile {
    /// `https://claude.example.net` (or `http://localhost:PORT` for local dev).
    pub url: String,
    /// Bearer token, never logged.
    pub token: String,
}

impl fmt::Debug for Profile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Profile")
            .field("url", &self.url)
            .field("token", &"[redacted]")
            .finish()
    }
}

impl Profile {
    /// Masked token for display: first 4 + last 4 chars, or all `*` when short.
    pub fn masked_token(&self) -> String {
        let t = &self.token;
        if t.len() > 8 {
            format!("{}…{}", &t[..4], &t[t.len() - 4..])
        } else {
            "*".repeat(t.len())
        }
    }
}

/// The `[proxy]` section of `config.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProxySection {
    /// The default profile name used by new sessions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// Named profiles.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub profiles: BTreeMap<String, Profile>,
}

/// The whole `config.toml` file (other sections are preserved via `extra`).
#[derive(Debug, Default, Serialize, Deserialize)]
struct ConfigFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    proxy: Option<ProxySection>,
    /// All other TOML keys — preserved on write so we don't clobber them.
    #[serde(flatten)]
    extra: toml::Table,
}

// ── Loading and saving ────────────────────────────────────────────────────────

/// Load the `[proxy]` section from `config.toml`. Missing file → empty section.
pub fn load() -> io::Result<ProxySection> {
    let path = paths::config_file();
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(ProxySection::default()),
        Err(e) => return Err(e),
    };
    let cfg: ConfigFile = toml::from_str(&String::from_utf8_lossy(&bytes))
        .map_err(|e| io::Error::other(format!("config.toml parse error: {e}")))?;
    Ok(cfg.proxy.unwrap_or_default())
}

/// Save an updated `[proxy]` section back to `config.toml`, preserving any
/// other keys that may already be present.
pub fn save(proxy: &ProxySection) -> io::Result<()> {
    let path = paths::config_file();
    // Load the current file (or start with an empty one) so we keep other sections.
    let bytes = std::fs::read(&path).unwrap_or_default();
    let mut cfg: ConfigFile = if bytes.is_empty() {
        ConfigFile::default()
    } else {
        toml::from_str(&String::from_utf8_lossy(&bytes))
            .map_err(|e| io::Error::other(format!("config.toml parse error: {e}")))?
    };
    cfg.proxy = Some(proxy.clone());
    let out = toml::to_string_pretty(&cfg)
        .map_err(|e| io::Error::other(format!("config.toml serialize error: {e}")))?;
    paths::write_atomic(&path, out.as_bytes())
}

// ── URL / env-var parsing ─────────────────────────────────────────────────────

/// Parse a `CLAUDIO_PROXY_URL` value into `(url, token)`.
///
/// Format: `<token>@<host>`, where `<host>` may be:
/// - `claude.example.net`              → `https://claude.example.net`
/// - `https://claude.example.net`      → verbatim
/// - `http://127.0.0.1:PORT`           → allowed (localhost only)
/// - `127.0.0.1:PORT`                  → `http://127.0.0.1:PORT` (localhost)
/// - `localhost:PORT`                  → `http://localhost:PORT`
///
/// Returns `Err` when:
/// - No `@` separator (missing token)
/// - The scheme is `http` but the host is not `127.0.0.1` / `localhost`
pub fn parse_proxy_url(raw: &str) -> Result<(String, String), String> {
    // Split on the LAST `@` so tokens themselves may contain `@`.
    let at = raw.rfind('@').ok_or_else(|| {
        "CLAUDIO_PROXY_URL must be <token>@<host> (missing token or '@' separator)".to_owned()
    })?;
    let token = raw[..at].trim().to_owned();
    if token.is_empty() {
        return Err("CLAUDIO_PROXY_URL: token before '@' is empty".to_owned());
    }
    let host_part = raw[at + 1..].trim().to_owned();
    if host_part.is_empty() {
        return Err("CLAUDIO_PROXY_URL: host after '@' is empty".to_owned());
    }

    let url = normalize_host(&host_part)?;
    Ok((url, token))
}

/// Turn a raw host string into a canonical `https://…` or `http://…` URL.
fn normalize_host(host: &str) -> Result<String, String> {
    // Already has a scheme.
    if let Some(rest) = host.strip_prefix("https://") {
        return Ok(format!("https://{}", rest.trim_end_matches('/')));
    }
    if let Some(rest) = host.strip_prefix("http://") {
        let trimmed = rest.trim_end_matches('/');
        let bare_host = trimmed.split(':').next().unwrap_or(trimmed);
        if !is_localhost(bare_host) {
            return Err(format!(
                "http:// is only allowed for 127.0.0.1/localhost, got '{bare_host}'"
            ));
        }
        return Ok(format!("http://{trimmed}"));
    }

    // No scheme: detect bare localhost patterns.
    let bare_host = host.split(':').next().unwrap_or(host);
    if is_localhost(bare_host) {
        Ok(format!("http://{}", host.trim_end_matches('/')))
    } else {
        Ok(format!("https://{}", host.trim_end_matches('/')))
    }
}

fn is_localhost(host: &str) -> bool {
    host == "127.0.0.1" || host == "localhost"
}

/// Return `Some(("env", Profile))` when `CLAUDIO_PROXY_URL` is set and valid.
/// Prints a warning to stderr and returns `None` on parse error.
pub fn from_env() -> Option<(&'static str, Profile)> {
    let raw = std::env::var("CLAUDIO_PROXY_URL").ok()?;
    match parse_proxy_url(&raw) {
        Ok((url, token)) => Some(("env", Profile { url, token })),
        Err(e) => {
            eprintln!("claudio: CLAUDIO_PROXY_URL: {e}");
            None
        }
    }
}

/// Normalize a raw host/URL string to a canonical URL (no token).
/// Same logic as the URL part of [`parse_proxy_url`] but without the token.
pub fn normalize_url(host: &str) -> Result<String, String> {
    normalize_host(host)
}

/// Derive a default profile name from a URL (the first DNS label).
///
/// `https://claude.vocdoni.net` → `"vocdoni"`
/// `http://127.0.0.1:8080`      → `"localhost"`
pub fn name_from_url(url: &str) -> String {
    let host = url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or(url)
        .split(':')
        .next()
        .unwrap_or(url);
    if is_localhost(host) {
        return "localhost".to_owned();
    }
    host.splitn(2, '.').next().unwrap_or(host).to_owned()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    // ── parse_proxy_url ───────────────────────────────────────────────────────

    #[test]
    fn parse_full_https() {
        let (url, tok) = parse_proxy_url("mytoken@https://claude.vocdoni.net").unwrap();
        assert_eq!(url, "https://claude.vocdoni.net");
        assert_eq!(tok, "mytoken");
    }

    #[test]
    fn parse_bare_host_defaults_https() {
        let (url, tok) = parse_proxy_url("tok@claude.example.net").unwrap();
        assert_eq!(url, "https://claude.example.net");
        assert_eq!(tok, "tok");
    }

    #[test]
    fn parse_localhost_port_defaults_http() {
        let (url, tok) = parse_proxy_url("tok@127.0.0.1:8080").unwrap();
        assert_eq!(url, "http://127.0.0.1:8080");
        assert_eq!(tok, "tok");
    }

    #[test]
    fn parse_localhost_name() {
        let (url, tok) = parse_proxy_url("tok@localhost:9000").unwrap();
        assert_eq!(url, "http://localhost:9000");
        assert_eq!(tok, "tok");
    }

    #[test]
    fn parse_http_non_localhost_rejected() {
        assert!(parse_proxy_url("tok@http://claude.example.net").is_err());
    }

    #[test]
    fn parse_missing_token_rejected() {
        assert!(parse_proxy_url("@claude.example.net").is_err());
        assert!(parse_proxy_url("claude.example.net").is_err());
    }

    #[test]
    fn parse_token_with_at_sign() {
        // Token itself contains '@'; we split on the last '@'.
        let (url, tok) = parse_proxy_url("user@example.com@claude.vocdoni.net").unwrap();
        assert_eq!(url, "https://claude.vocdoni.net");
        assert_eq!(tok, "user@example.com");
    }

    // ── name_from_url ─────────────────────────────────────────────────────────

    #[test]
    fn name_from_url_extracts_first_label() {
        assert_eq!(name_from_url("https://claude.vocdoni.net"), "claude");
        assert_eq!(name_from_url("https://proxy.example.com"), "proxy");
        assert_eq!(name_from_url("http://127.0.0.1:8080"), "localhost");
    }

    // ── Debug redaction ───────────────────────────────────────────────────────

    #[test]
    fn debug_redacts_token() {
        let p = Profile { url: "https://x.net".into(), token: "supersecret".into() };
        let dbg = format!("{p:?}");
        assert!(!dbg.contains("supersecret"));
        assert!(dbg.contains("[redacted]"));
    }

    // ── TOML round-trip and file mode 0600 ───────────────────────────────────

    #[test]
    fn round_trip_and_file_mode() {
        let dir = std::env::temp_dir()
            .join(format!("claudio-proxy-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        // Override XDG_CONFIG_HOME so paths::config_file() points into our temp dir.
        std::env::set_var("XDG_CONFIG_HOME", &dir);

        let mut sec = ProxySection::default();
        sec.default = Some("vocdoni".into());
        sec.profiles.insert(
            "vocdoni".into(),
            Profile { url: "https://claude.vocdoni.net".into(), token: "topsecret".into() },
        );
        save(&sec).unwrap();

        let path = dir.join("claudio/config.toml");
        let mode = std::fs::metadata(&path).unwrap().mode() & 0o777;
        assert_eq!(mode, 0o600, "config.toml must be mode 0600");

        let loaded = load().unwrap();
        assert_eq!(loaded.default.as_deref(), Some("vocdoni"));
        let p = loaded.profiles.get("vocdoni").unwrap();
        assert_eq!(p.url, "https://claude.vocdoni.net");
        assert_eq!(p.token, "topsecret");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    // ── masked_token ──────────────────────────────────────────────────────────

    #[test]
    fn masked_token_long_and_short() {
        let p = |t: &str| Profile { url: "u".into(), token: t.into() };
        assert_eq!(p("abcdefghwxyz").masked_token(), "abcd…wxyz");
        assert_eq!(p("ab").masked_token(), "**");
    }
}
