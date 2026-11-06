//! Session plugins served by a proxy profile (`GET /v1/claudio/plugins`).
//!
//! A plugin is a pinned manifest (`crate::plugins::manifest`: what its
//! sessions get, and the plugins it installs) and the env its MCP servers
//! need. The env holds secrets: the list is cached per profile in
//! `~/.config/claudio/plugins.json` (mode 0600, like `config.toml`), so a
//! session can get it when the proxy is unreachable, and never goes to
//! argv, the journal, `state.json`, logs or notices. `Debug` prints names
//! and key counts only.
//!
//! Everything a proxy sends is checked by [`admit`] before it is used.

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::api::{build_client, ApiError};
use super::profile::Profile;
use crate::paths;

/// At most this many plugins per list are used.
pub const MAX_PLUGINS: usize = 32;
/// At most this many env keys per plugin.
pub const MAX_ENV_KEYS: usize = 64;
/// One env value's cap, in bytes.
pub const MAX_ENV_VALUE: usize = 4096;

/// Env key prefixes a plugin may not set: the proxy's own and Claude Code's
/// switches, the loaders and the toolchains that read code from the env.
const DENIED_PREFIXES: &[&str] = &[
    "ANTHROPIC_",
    "CLAUDIO_",
    "CLAUDE_",
    "NODE_",
    "LD_",
    "DYLD_",
    "GIT_",
    "SSL_",
    "XDG_",
    "SSH_",
];

/// Env keys a plugin may not set (nor their lowercase forms).
const DENIED_KEYS: &[&str] = &[
    "PATH",
    "HOME",
    "SHELL",
    "TERM",
    "USER",
    "LOGNAME",
    "TMPDIR",
    "BASH_ENV",
    "ENV",
    "PYTHONPATH",
    "PERL5OPT",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
];

/// One plugin of a profile.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Plugin {
    pub name: String,
    pub version: String,
    pub bundle_url: String,
    pub bundle_sha256: String,
    /// Accepted and ignored (the zip bundles' installed-plugin check; a
    /// manifest names its plugins itself).
    #[serde(default)]
    pub installed_id: String,
    /// Sorted by key.
    #[serde(default)]
    pub env: Vec<(String, String)>,
}

/// A profile's plugins as fetched at `fetched_at` (Unix seconds).
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginList {
    pub profile: String,
    pub fetched_at: u64,
    pub plugins: Vec<Plugin>,
}

impl fmt::Debug for Plugin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let keys: Vec<&str> = self.env.iter().map(|(k, _)| k.as_str()).collect();
        f.debug_struct("Plugin")
            .field("name", &self.name)
            .field("version", &self.version)
            .field("bundle_url", &self.bundle_url)
            .field("bundle_sha256", &self.bundle_sha256)
            .field("installed_id", &self.installed_id)
            .field("env", &keys)
            .finish()
    }
}

impl fmt::Debug for PluginList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PluginList")
            .field("profile", &self.profile)
            .field("fetched_at", &self.fetched_at)
            .field("plugins", &self.plugins)
            .finish()
    }
}

impl PluginList {
    /// Each plugin with why it is refused (`None`: admitted), in order. Past
    /// [`MAX_PLUGINS`], and a name given twice, are refused too.
    pub fn checked(&self) -> Vec<(&Plugin, Option<String>)> {
        self.plugins
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let why = if i >= MAX_PLUGINS {
                    Some(format!("more than {MAX_PLUGINS} plugins"))
                } else if self.plugins[..i].iter().any(|q| q.name == p.name) {
                    Some("name given twice".to_owned())
                } else {
                    admit(p).err()
                };
                (p, why)
            })
            .collect()
    }

    /// The admitted plugins, in order.
    pub fn admitted(&self) -> Vec<&Plugin> {
        let checked = self.checked();
        checked
            .into_iter()
            .filter(|(_, why)| why.is_none())
            .map(|(p, _)| p)
            .collect()
    }

    /// What the fetch log says: names and env key counts, never a value.
    pub fn summary(&self) -> String {
        let parts: Vec<String> = self
            .plugins
            .iter()
            .map(|p| format!("{} ({} keys)", p.name, p.env.len()))
            .collect();
        parts.join(", ")
    }
}

/// `[a-z0-9-]{1,40}`: a plugin name, and a path component on every host.
pub fn name_ok(name: &str) -> bool {
    (1..=40).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// 64 lowercase hex digits.
pub fn sha256_ok(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `[A-Z_][A-Z0-9_]{0,63}`, and not one a plugin may not set.
pub fn key_ok(key: &str) -> Result<(), String> {
    let mut bytes = key.bytes();
    let shape = key.len() <= 64
        && bytes
            .next()
            .is_some_and(|b| b.is_ascii_uppercase() || b == b'_')
        && bytes.all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
    if !shape {
        return Err("bad env key".to_owned());
    }
    let upper = key.to_ascii_uppercase();
    if DENIED_PREFIXES.iter().any(|p| upper.starts_with(p)) || DENIED_KEYS.contains(&&*upper) {
        return Err(format!("env key {key} not allowed"));
    }
    Ok(())
}

/// Whether `url` is a plain-http loopback URL a test lets through:
/// `http://127.0.0.1:<port>/…` in a debug build run with
/// `CLAUDIO_TEST_LOOPBACK_HTTP=1` (the manager tests' fake proxy). Always
/// false in a release build.
pub fn test_http_ok(url: &str) -> bool {
    cfg!(debug_assertions)
        && url.starts_with("http://127.0.0.1:")
        && std::env::var_os("CLAUDIO_TEST_LOOPBACK_HTTP").is_some_and(|v| v == "1")
}

/// `''` or `[a-z0-9-]{1,40}@[a-z0-9-]{1,40}`.
fn installed_id_ok(id: &str) -> bool {
    id.is_empty()
        || id
            .split_once('@')
            .is_some_and(|(p, m)| name_ok(p) && name_ok(m))
}

/// Why plugin `p` may not be used: a bad name, version, URL, hash or
/// installed id, or an env key or value it may not set. The reason names
/// the key, never a value.
pub fn admit(p: &Plugin) -> Result<(), String> {
    if !name_ok(&p.name) {
        return Err("bad name".to_owned());
    }
    let printable = |s: &str| s.bytes().all(|b| (0x20..0x7f).contains(&b));
    if !(1..=64).contains(&p.version.len()) || !printable(&p.version) {
        return Err("bad version".to_owned());
    }
    let https = p.bundle_url.starts_with("https://") || test_http_ok(&p.bundle_url);
    if !https || !printable(&p.bundle_url) {
        return Err("bundle URL is not https".to_owned());
    }
    if !sha256_ok(&p.bundle_sha256) {
        return Err("bad bundle hash".to_owned());
    }
    if !installed_id_ok(&p.installed_id) {
        return Err("bad installed id".to_owned());
    }
    if p.env.len() > MAX_ENV_KEYS {
        return Err(format!("more than {MAX_ENV_KEYS} env keys"));
    }
    for (key, value) in &p.env {
        key_ok(key)?;
        if value.len() > MAX_ENV_VALUE {
            return Err(format!("env {key}: value over {MAX_ENV_VALUE} bytes"));
        }
        if value.bytes().any(|b| b < 0x20 && b != b'\n' && b != b'\t') {
            return Err(format!("env {key}: control character in value"));
        }
    }
    Ok(())
}

// ── Fetch ─────────────────────────────────────────────────────────────────────

/// `GET /v1/claudio/plugins`, as the proxy sends it.
#[derive(Deserialize)]
struct Reply {
    #[serde(default)]
    plugins: Vec<ReplyPlugin>,
}

#[derive(Deserialize)]
struct ReplyPlugin {
    #[serde(default)]
    name: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    bundle: ReplyBundle,
    #[serde(default)]
    installed_id: String,
    #[serde(default)]
    env: BTreeMap<String, String>,
}

#[derive(Default, Deserialize)]
struct ReplyBundle {
    #[serde(default)]
    url: String,
    #[serde(default)]
    sha256: String,
}

/// The list in a `/v1/claudio/plugins` reply, for profile `profile` at `now`.
fn list_of(reply: Reply, profile: &str, now: u64) -> PluginList {
    let plugins = reply
        .plugins
        .into_iter()
        .map(|p| Plugin {
            name: p.name,
            version: p.version,
            bundle_url: p.bundle.url,
            bundle_sha256: p.bundle.sha256,
            installed_id: p.installed_id,
            env: p.env.into_iter().collect(),
        })
        .collect();
    PluginList {
        profile: profile.to_owned(),
        fetched_at: now,
        plugins,
    }
}

/// Fetch `profile`'s plugins (`name` is the profile's name). A 401 or 403
/// is [`ApiError::Http`] with that status (signed out); a 404 too (an
/// older proxy).
pub async fn fetch(name: &str, profile: &Profile) -> Result<PluginList, ApiError> {
    let resp = build_client()?
        .get(format!("{}/v1/claudio/plugins", profile.url))
        .bearer_auth(&profile.token)
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(ApiError::Http(resp.status()));
    }
    let reply: Reply = resp.json().await?;
    Ok(list_of(reply, name, paths::unix_now()))
}

// ── Manifest ──────────────────────────────────────────────────────────────────

/// How long a manifest fetch may take.
const MANIFEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Fetch a plugin's manifest: `url` (https, redirects to https only), at
/// most 256 KiB, whose SHA-256 is `sha256`; its text. Kept in memory only.
/// The reason names no value.
pub async fn fetch_manifest(url: &str, sha256: &str) -> Result<String, String> {
    use crate::plugins::manifest::MAX_BYTES;
    use sha2::{Digest, Sha256};
    let plain_ok = test_http_ok(url);
    if !url.starts_with("https://") && !plain_ok {
        return Err("not https".to_owned());
    }
    let policy = reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() >= 5 {
            attempt.error("too many redirects")
        } else if attempt.url().scheme() == "https" || plain_ok {
            attempt.follow()
        } else {
            attempt.error("redirect to a non-https URL")
        }
    });
    let client = reqwest::Client::builder()
        .connect_timeout(MANIFEST_TIMEOUT)
        .timeout(MANIFEST_TIMEOUT)
        .redirect(policy)
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let mut resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("unreachable: {}", e.without_url()))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status().as_u16()));
    }
    let over = || "over 256 KiB".to_owned();
    if resp.content_length().is_some_and(|n| n > MAX_BYTES as u64) {
        return Err(over());
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("unreachable: {}", e.without_url()))?
    {
        if body.len() + chunk.len() > MAX_BYTES {
            return Err(over());
        }
        body.extend_from_slice(&chunk);
    }
    if format!("{:x}", Sha256::digest(&body)) != sha256 {
        return Err("hash mismatch".to_owned());
    }
    String::from_utf8(body).map_err(|_| "not UTF-8".to_owned())
}

// ── Cache ─────────────────────────────────────────────────────────────────────

/// `plugins.json`: one entry per profile name.
#[derive(Default, Serialize, Deserialize)]
struct CacheFile {
    #[serde(default)]
    profiles: BTreeMap<String, PluginList>,
}

fn read_cache(path: &Path) -> CacheFile {
    std::fs::read(path)
        .ok()
        .and_then(|raw| serde_json::from_slice(&raw).ok())
        .unwrap_or_default()
}

fn write_cache(path: &Path, cache: &CacheFile) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        paths::ensure_private_dir(dir)?;
    }
    let raw = serde_json::to_vec_pretty(cache).map_err(io::Error::other)?;
    paths::write_atomic(path, &raw)
}

/// `profile`'s cached list, if any.
pub fn load_cache(profile: &str) -> Option<PluginList> {
    load_cache_from(&paths::plugins_cache(), profile)
}

/// Every cached list (a missing or unreadable file has none).
pub fn load_all() -> Vec<PluginList> {
    load_all_from(&paths::plugins_cache())
}

/// Store `list` as its profile's entry.
pub fn save_cache(list: &PluginList) -> io::Result<()> {
    save_cache_to(&paths::plugins_cache(), list)
}

/// Drop `profile`'s entry (its secrets go with it).
pub fn remove_cache(profile: &str) -> io::Result<()> {
    remove_cache_from(&paths::plugins_cache(), profile)
}

pub fn load_cache_from(path: &Path, profile: &str) -> Option<PluginList> {
    read_cache(path).profiles.remove(profile)
}

pub fn load_all_from(path: &Path) -> Vec<PluginList> {
    read_cache(path).profiles.into_values().collect()
}

pub fn save_cache_to(path: &Path, list: &PluginList) -> io::Result<()> {
    let mut cache = read_cache(path);
    cache.profiles.insert(list.profile.clone(), list.clone());
    write_cache(path, &cache)
}

pub fn remove_cache_from(path: &Path, profile: &str) -> io::Result<()> {
    let mut cache = read_cache(path);
    if cache.profiles.remove(profile).is_none() {
        return Ok(());
    }
    write_cache(path, &cache)
}

/// A good plugin for tests: one env key `KB_TOKEN` = `s3cret`.
#[cfg(test)]
pub fn sample(name: &str) -> Plugin {
    Plugin {
        name: name.into(),
        version: "1.4.0".into(),
        bundle_url: format!("https://example.org/{name}.zip"),
        bundle_sha256: "a".repeat(64),
        installed_id: format!("{name}@vocdoni"),
        env: vec![("KB_TOKEN".into(), "s3cret".into())],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn plugin(name: &str) -> Plugin {
        sample(name)
    }

    fn refused(p: &Plugin) -> String {
        admit(p).unwrap_err()
    }

    #[test]
    fn a_good_plugin_is_admitted() {
        assert_eq!(admit(&plugin("vocdoni-internal")), Ok(()));
        let mut p = plugin("x");
        p.installed_id = String::new();
        p.env.push(("MULTI".into(), "a\nb\tc".into()));
        assert_eq!(admit(&p), Ok(()));
    }

    #[test]
    fn admit_refuses_bad_names_versions_urls_hashes_and_ids() {
        for name in ["", "Up", "a/b", "..", "a_b", &"n".repeat(41)] {
            let p = Plugin {
                name: name.into(),
                ..plugin("x")
            };
            assert_eq!(refused(&p), "bad name", "{name:?}");
        }
        for version in ["", "1\n2", &"9".repeat(65)] {
            let p = Plugin {
                version: version.into(),
                ..plugin("x")
            };
            assert_eq!(refused(&p), "bad version", "{version:?}");
        }
        for url in [
            "http://example.org/x.zip",
            "file:///x.zip",
            "https://e.org/\nx",
        ] {
            let p = Plugin {
                bundle_url: url.into(),
                ..plugin("x")
            };
            assert_eq!(refused(&p), "bundle URL is not https", "{url:?}");
        }
        for hash in ["A".repeat(64), "a".repeat(63), "g".repeat(64)] {
            let p = Plugin {
                bundle_sha256: hash.clone(),
                ..plugin("x")
            };
            assert_eq!(refused(&p), "bad bundle hash", "{hash:?}");
        }
        for id in ["x", "x@", "@m", "X@m", "a@b@c"] {
            let p = Plugin {
                installed_id: id.into(),
                ..plugin("x")
            };
            assert_eq!(refused(&p), "bad installed id", "{id:?}");
        }
    }

    #[test]
    fn admit_refuses_keys_a_plugin_may_not_set() {
        let with_key = |key: &str| Plugin {
            env: vec![(key.into(), "v".into())],
            ..plugin("x")
        };
        for key in ["", "lower", "1ABC", "A-B", &"K".repeat(65)] {
            assert_eq!(refused(&with_key(key)), "bad env key", "{key:?}");
        }
        for key in [
            "ANTHROPIC_API_KEY",
            "CLAUDIO_X",
            "CLAUDE_CONFIG_DIR",
            "NODE_OPTIONS",
            "LD_PRELOAD",
            "DYLD_INSERT_LIBRARIES",
            "GIT_SSH_COMMAND",
            "SSL_CERT_FILE",
            "XDG_CONFIG_HOME",
            "SSH_AUTH_SOCK",
            "PATH",
            "HOME",
            "SHELL",
            "TERM",
            "USER",
            "LOGNAME",
            "TMPDIR",
            "BASH_ENV",
            "ENV",
            "PYTHONPATH",
            "PERL5OPT",
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "NO_PROXY",
        ] {
            assert_eq!(
                refused(&with_key(key)),
                format!("env key {key} not allowed"),
                "{key}"
            );
        }
        // Allowed: close to a denied one but not it.
        assert_eq!(admit(&with_key("PATHS")), Ok(()));
        assert_eq!(admit(&with_key("ENVIRONMENT")), Ok(()));
    }

    #[test]
    fn admit_refuses_bad_values_naming_only_the_key() {
        let with_value = |v: String| Plugin {
            env: vec![("TOKEN".into(), v)],
            ..plugin("x")
        };
        let long = refused(&with_value("v".repeat(MAX_ENV_VALUE + 1)));
        assert_eq!(long, "env TOKEN: value over 4096 bytes");
        assert_eq!(admit(&with_value("v".repeat(MAX_ENV_VALUE))), Ok(()));
        for bad in ["se\0cret", "se\u{1b}[31mcret", "se\rcret"] {
            let why = refused(&with_value(bad.into()));
            assert_eq!(why, "env TOKEN: control character in value");
            assert!(!why.contains("se"), "no value in the reason");
        }
        let many = Plugin {
            env: (0..=MAX_ENV_KEYS)
                .map(|i| (format!("K{i}"), "v".into()))
                .collect(),
            ..plugin("x")
        };
        assert_eq!(refused(&many), "more than 64 env keys");
    }

    #[test]
    fn a_list_refuses_duplicates_and_more_than_32() {
        let mut plugins: Vec<Plugin> = (0..MAX_PLUGINS + 1)
            .map(|i| plugin(&format!("p{i}")))
            .collect();
        plugins[1] = plugin("p0");
        plugins[2].bundle_url = "http://x".into();
        let list = PluginList {
            profile: "work".into(),
            fetched_at: 1,
            plugins,
        };
        let checked = list.checked();
        assert_eq!(checked[0].1, None);
        assert_eq!(checked[1].1.as_deref(), Some("name given twice"));
        assert_eq!(checked[2].1.as_deref(), Some("bundle URL is not https"));
        assert_eq!(
            checked[MAX_PLUGINS].1.as_deref(),
            Some("more than 32 plugins")
        );
        assert_eq!(list.admitted().len(), MAX_PLUGINS - 2);
    }

    #[test]
    fn parse_reads_the_proxy_reply_with_sorted_env() {
        let body = br#"{"plugins":[{"name":"vocdoni-internal","version":"1.4.0",
            "bundle":{"url":"https://e.org/v.zip","sha256":"ab"},
            "installed_id":"vocdoni-internal@vocdoni",
            "env":{"ZED":"z","KB_TOKEN":"k"}}]}"#;
        let parse = |body: &[u8]| serde_json::from_slice(body).map(|r| list_of(r, "work", 7));
        let list = parse(body).unwrap();
        assert_eq!(list.profile, "work");
        assert_eq!(list.fetched_at, 7);
        let p = &list.plugins[0];
        assert_eq!(p.bundle_url, "https://e.org/v.zip");
        assert_eq!(p.bundle_sha256, "ab");
        assert_eq!(
            p.env,
            [
                ("KB_TOKEN".to_owned(), "k".to_owned()),
                ("ZED".to_owned(), "z".to_owned())
            ]
        );
        assert!(parse(b"not json").is_err());
        assert!(parse(b"{}").unwrap().plugins.is_empty());
    }

    #[tokio::test]
    async fn fetch_reads_the_list_and_reports_statuses() {
        use axum::http::StatusCode;
        use axum::routing::get;
        async fn serve(router: axum::Router) -> String {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
            format!("http://127.0.0.1:{port}")
        }
        let profile = |url: String| Profile {
            url,
            token: "tok".into(),
            first_party: false,
            email: None,
        };
        let ok = axum::Router::new().route(
            "/v1/claudio/plugins",
            get(|headers: axum::http::HeaderMap| async move {
                assert_eq!(headers["authorization"], "Bearer tok");
                axum::Json(
                    serde_json::json!({"plugins": [{"name": "kb", "version": "1",
                    "bundle": {"url": "https://e.org/kb.zip", "sha256": "a".repeat(64)},
                    "env": {"KB_TOKEN": "k"}}]}),
                )
            }),
        );
        let list = fetch("work", &profile(serve(ok).await)).await.unwrap();
        assert_eq!(list.profile, "work");
        assert_eq!(list.admitted().len(), 1);
        for status in [
            StatusCode::UNAUTHORIZED,
            StatusCode::FORBIDDEN,
            StatusCode::NOT_FOUND,
        ] {
            let router = axum::Router::new()
                .route("/v1/claudio/plugins", get(move || async move { status }));
            match fetch("work", &profile(serve(router).await)).await {
                Err(ApiError::Http(s)) => assert_eq!(s.as_u16(), status.as_u16()),
                other => panic!("{status}: {other:?}"),
            }
        }
    }

    #[test]
    fn debug_and_summary_never_show_a_value() {
        let list = PluginList {
            profile: "work".into(),
            fetched_at: 1,
            plugins: vec![plugin("vocdoni-internal")],
        };
        let dbg = format!("{list:?}");
        assert!(dbg.contains("KB_TOKEN") && !dbg.contains("s3cret"), "{dbg}");
        assert_eq!(list.summary(), "vocdoni-internal (1 keys)");
    }

    #[test]
    fn the_cache_keeps_one_private_entry_per_profile() {
        let dir = std::env::temp_dir().join(format!("claudio-plugins-{}", uuid::Uuid::new_v4()));
        let path = dir.join("cfg/plugins.json");
        assert_eq!(load_cache_from(&path, "work"), None);
        let list = |profile: &str, name: &str| PluginList {
            profile: profile.into(),
            fetched_at: 5,
            plugins: vec![plugin(name)],
        };
        save_cache_to(&path, &list("work", "a")).unwrap();
        save_cache_to(&path, &list("home", "b")).unwrap();
        save_cache_to(&path, &list("work", "c")).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600, "plugins.json holds secrets");
        assert_eq!(mode(path.parent().unwrap()), 0o700);
        assert_eq!(load_cache_from(&path, "work"), Some(list("work", "c")));
        assert_eq!(load_cache_from(&path, "home"), Some(list("home", "b")));
        assert_eq!(load_all_from(&path).len(), 2);
        remove_cache_from(&path, "work").unwrap();
        assert_eq!(load_cache_from(&path, "work"), None);
        assert_eq!(load_cache_from(&path, "home"), Some(list("home", "b")));
        remove_cache_from(&path, "nope").unwrap();
        // A broken file is no cache.
        std::fs::write(&path, b"{").unwrap();
        assert_eq!(load_cache_from(&path, "home"), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
