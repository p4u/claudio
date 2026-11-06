//! `Bundle`: a proxy plugin's zip on this host, for claude's `--plugin-dir`.
//!
//! The zips live in `<cache>/plugins` (0700): `<name>.zip` and its sidecar
//! `<name>.sha256`, one pair per plugin name, so a journaled
//! `--plugin-dir <path>` stays valid across versions. claudio never unpacks
//! a zip; the hash pins the exact file the proxy's admin registered.
//!
//! Every request is checked here (the name is a path component). A download
//! is https only, capped at [`MAX_BYTES`] and [`TOTAL_TIMEOUT`], streamed to
//! a temp file in the cache dir, verified, then renamed into place; any
//! failure leaves no partial file. One download per plugin name runs at a
//! time: a second request waits for it and then finds the zip cached.

use std::collections::HashMap;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::paths;
use crate::proto::Msg;
use crate::proxy::plugins::{name_ok, sha256_ok};

/// A bundle's size cap.
pub const MAX_BYTES: u64 = 64 << 20;
/// At most this many zips are kept.
pub const MAX_BUNDLES: usize = 32;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(120);
/// `installed_plugins.json` is read up to this size.
const MAX_INSTALLED: u64 = 1 << 20;

/// Where bundles go and where Claude Code keeps its plugins, and the
/// per-name download locks.
pub(crate) struct Bundles {
    dirs: Mutex<Dirs>,
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

#[derive(Clone)]
struct Dirs {
    /// `<cache>/plugins`.
    cache: PathBuf,
    /// `$CLAUDE_CONFIG_DIR`, else `~/.claude`.
    claude_config: PathBuf,
    /// Tests serve bundles over plain http on loopback.
    allow_http: bool,
}

impl Bundles {
    /// The standard places: [`paths::cache_dir`] and the Claude Code config
    /// dir the sessions see (this daemon's `CLAUDE_CONFIG_DIR`, which every
    /// session inherits).
    pub(crate) fn new() -> Bundles {
        let claude_config = std::env::var_os("CLAUDE_CONFIG_DIR")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| paths::home().join(".claude"));
        Bundles {
            dirs: Mutex::new(Dirs {
                cache: paths::cache_dir().join("plugins"),
                claude_config,
                allow_http: false,
            }),
            locks: Mutex::new(HashMap::new()),
        }
    }

    fn dirs(&self) -> Dirs {
        self.dirs.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Use `cache` and `claude_config` instead, and accept http URLs.
    #[cfg(test)]
    pub(crate) fn set_dirs(&self, cache: PathBuf, claude_config: PathBuf) {
        *self.dirs.lock().unwrap_or_else(|e| e.into_inner()) = Dirs {
            cache,
            claude_config,
            allow_http: true,
        };
    }

    fn lock(&self, name: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.locks.lock().unwrap_or_else(|e| e.into_inner());
        Arc::clone(locks.entry(name.to_owned()).or_default())
    }
}

/// Answer a `Bundle` request: `Bundled`, or `Error "bundle <name>: …"`.
pub(crate) async fn handle(
    bundles: &Bundles,
    name: String,
    url: String,
    sha256: String,
    installed_id: String,
) -> Msg {
    let dirs = bundles.dirs();
    let scheme_ok = url.starts_with("https://") || (dirs.allow_http && url.starts_with("http://"));
    if !name_ok(&name) || !sha256_ok(&sha256) || !scheme_ok {
        return Msg::Error {
            message: format!("bundle {}: bad request", safe_name(&name)),
        };
    }
    if !installed_id.is_empty() && is_installed(&dirs.claude_config, &installed_id).await {
        return Msg::Bundled {
            name,
            path: None,
            installed: true,
        };
    }
    // One download per name at a time; whoever waited finds it cached.
    let lock = bundles.lock(&name);
    let _held = lock.lock().await;
    match ready(&dirs, &name, &url, &sha256).await {
        Ok(path) => Msg::Bundled {
            name,
            path: Some(path.to_string_lossy().into_owned()),
            installed: false,
        },
        Err(why) => {
            tracing::warn!(bundle = %name, error = %why, "bundle not ready");
            Msg::Error {
                message: format!("bundle {name}: {why}"),
            }
        }
    }
}

/// A name for an error about a refused request: shown only when valid.
fn safe_name(name: &str) -> &str {
    if name_ok(name) {
        name
    } else {
        "?"
    }
}

/// Whether Claude Code has plugin `id` installed: a key of `plugins` in
/// `<claude_config>/plugins/installed_plugins.json`. A missing, oversized
/// or unparsable file counts as not installed.
async fn is_installed(claude_config: &Path, id: &str) -> bool {
    let file = claude_config.join("plugins/installed_plugins.json");
    let id = id.to_owned();
    tokio::task::spawn_blocking(move || {
        let meta = std::fs::metadata(&file).ok()?;
        if !meta.is_file() || meta.len() > MAX_INSTALLED {
            return None;
        }
        let raw = std::fs::read(&file).ok()?;
        let v: serde_json::Value = serde_json::from_slice(&raw).ok()?;
        Some(v.get("plugins")?.as_object()?.contains_key(&id))
    })
    .await
    .ok()
    .flatten()
    .unwrap_or(false)
}

/// The zip of `name` with hash `sha256`: cached, else downloaded.
async fn ready(dirs: &Dirs, name: &str, url: &str, sha256: &str) -> Result<PathBuf, String> {
    let dir = dirs.cache.clone();
    paths::ensure_private_dir(&dir).map_err(|e| format!("cache dir: {e}"))?;
    let zip = dir.join(format!("{name}.zip"));
    let sidecar = dir.join(format!("{name}.sha256"));
    if zip.is_file() && std::fs::read_to_string(&sidecar).is_ok_and(|s| s.trim() == sha256) {
        return Ok(zip);
    }
    if !zip.exists() && count_zips(&dir) >= MAX_BUNDLES {
        return Err(format!("cache holds {MAX_BUNDLES} bundles already"));
    }
    let tmp = dir.join(format!(".{name}.{}.part", uuid::Uuid::new_v4().simple()));
    let result = download(url, &tmp, sha256, dirs.allow_http).await;
    let placed = result.and_then(|()| place(&tmp, &zip, &sidecar, sha256));
    if placed.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    placed.map(|()| zip)
}

fn count_zips(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().ends_with(".zip"))
                .count()
        })
        .unwrap_or(0)
}

/// Move a verified download into place. The old sidecar goes first, so no
/// moment pairs a hash with a zip it does not describe; a crash in between
/// leaves no sidecar, and the next request downloads again.
fn place(tmp: &Path, zip: &Path, sidecar: &Path, sha256: &str) -> Result<(), String> {
    match std::fs::remove_file(sidecar) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("cannot replace the sidecar: {e}")),
    }
    std::fs::rename(tmp, zip).map_err(|e| format!("cannot store the bundle: {e}"))?;
    paths::write_atomic(sidecar, format!("{sha256}\n").as_bytes())
        .map_err(|e| format!("cannot write the sidecar: {e}"))
}

/// Stream `url` into a new file `tmp` (0600), checking the size cap and,
/// at the end, the SHA-256. Redirects are followed to https URLs only.
async fn download(url: &str, tmp: &Path, sha256: &str, allow_http: bool) -> Result<(), String> {
    let policy = reqwest::redirect::Policy::custom(move |attempt| {
        let https = attempt.url().scheme() == "https";
        if attempt.previous().len() >= 5 {
            attempt.error("too many redirects")
        } else if https || allow_http {
            attempt.follow()
        } else {
            attempt.error("redirect to a non-https URL")
        }
    });
    let client = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(TOTAL_TIMEOUT)
        .redirect(policy)
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let mut resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("download failed: {}", e.without_url()))?;
    if !resp.status().is_success() {
        return Err(format!("download failed: HTTP {}", resp.status().as_u16()));
    }
    if resp.content_length().is_some_and(|n| n > MAX_BYTES) {
        return Err("bundle over 64 MiB".to_owned());
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(tmp)
        .map_err(|e| format!("cannot create a temp file: {e}"))?;
    let mut hasher = Sha256::new();
    let mut total: u64 = 0;
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("download failed: {}", e.without_url()))?
    {
        total += chunk.len() as u64;
        if total > MAX_BYTES {
            return Err("bundle over 64 MiB".to_owned());
        }
        hasher.update(&chunk);
        file.write_all(&chunk)
            .map_err(|e| format!("cannot write the bundle: {e}"))?;
    }
    file.sync_all()
        .map_err(|e| format!("cannot write the bundle: {e}"))?;
    let got = format!("{:x}", hasher.finalize());
    if got != sha256 {
        return Err("hash mismatch".to_owned());
    }
    Ok(())
}
