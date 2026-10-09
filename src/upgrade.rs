//! Upgrade-check and self-upgrade for claudio.
//!
//! # Upgrade check
//! [`check_once`] checks `https://api.github.com/repos/p4u/claudio-releases/releases/latest`
//! at most once per 24 hours (cached in `~/.config/claudio/update.json`).
//! The check is skipped when `[update] check = false` in `config.toml` or
//! `CLAUDIO_NO_UPDATE_CHECK=1` is set.
//!
//! # CLI commands
//! - `claudio upgrade`        — download and install the latest release if newer.
//! - `claudio upgrade --check` — report whether a newer version is available; don't install.

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::paths;
use crate::remote::bootstrap::RELEASES_REPO;

// ── Version comparison ────────────────────────────────────────────────────────

/// A parsed semantic version (major.minor.patch).  Anything that doesn't parse
/// cleanly is considered equal to 0.0.0 so it never triggers an upgrade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SemVer {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl SemVer {
    /// Parse a string like "1.2.3" or "v1.2.3".
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches('v');
        let mut parts = s.splitn(3, '.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        // Allow trailing pre-release suffixes on patch (e.g. "3-alpha").
        let patch_str = parts.next().unwrap_or("0");
        let patch: u32 = patch_str
            .split(|c: char| !c.is_ascii_digit())
            .next()
            .and_then(|p| p.parse().ok())
            .unwrap_or(0);
        Some(SemVer { major, minor, patch })
    }

    fn zero() -> Self {
        SemVer { major: 0, minor: 0, patch: 0 }
    }
}

// ── Cache ─────────────────────────────────────────────────────────────────────

/// `~/.config/claudio/update.json`
fn cache_path() -> PathBuf {
    paths::config_dir().join("update.json")
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct UpdateCache {
    /// Unix timestamp of the last successful check.
    last_check: u64,
    /// Latest tag name returned by the API (e.g. "v0.3.0").
    latest_tag: String,
}

impl UpdateCache {
    fn load() -> Self {
        let path = cache_path();
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(_) => return Self::default(),
        };
        serde_json::from_slice(&bytes).unwrap_or_default()
    }

    fn save(&self) {
        let path = cache_path();
        if let Ok(json) = serde_json::to_vec_pretty(self) {
            let _ = paths::write_atomic(&path, &json);
        }
    }

    fn is_fresh(&self) -> bool {
        let now = paths::unix_now();
        now.saturating_sub(self.last_check) < 86_400 // 24 hours
    }
}

// ── GitHub API ────────────────────────────────────────────────────────────────

const CHECK_TIMEOUT: Duration = Duration::from_secs(5);

/// Fetch the latest release tag from the GitHub API.
///
/// Returns the tag name (e.g. `"v0.3.0"`) or an error string.
pub async fn latest_tag() -> Result<String, String> {
    latest_tag_from(&api_url()).await
}

fn api_url() -> String {
    format!(
        "https://api.github.com/repos/{}/releases/latest",
        RELEASES_REPO
    )
}

/// Injectable for tests.
pub async fn latest_tag_from(url: &str) -> Result<String, String> {
    #[derive(Deserialize)]
    struct Release {
        tag_name: String,
    }

    let client = reqwest::Client::builder()
        .timeout(CHECK_TIMEOUT)
        .user_agent(format!("claudio/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("cannot build HTTP client: {e}"))?;

    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("HTTP request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("GitHub API returned {}", resp.status()));
    }

    let release: Release = resp
        .json()
        .await
        .map_err(|e| format!("cannot parse GitHub response: {e}"))?;

    Ok(release.tag_name)
}

// ── Check once (with cache) ───────────────────────────────────────────────────

/// Returns `Some(tag)` if a newer release is available, `None` otherwise.
///
/// Respects the 24-hour cache and the opt-out env var / config flag.
pub async fn check_once(update_check_enabled: bool) -> Option<String> {
    if !update_check_enabled {
        return None;
    }
    if std::env::var_os("CLAUDIO_NO_UPDATE_CHECK").is_some() {
        return None;
    }

    let mut cache = UpdateCache::load();

    let tag = if cache.is_fresh() && !cache.latest_tag.is_empty() {
        cache.latest_tag.clone()
    } else {
        match latest_tag().await {
            Ok(t) => {
                cache.latest_tag = t.clone();
                cache.last_check = paths::unix_now();
                cache.save();
                t
            }
            Err(_) => return None, // silent: don't bother users if the network is down
        }
    };

    let current = SemVer::parse(env!("CARGO_PKG_VERSION")).unwrap_or_else(SemVer::zero);
    let latest = SemVer::parse(&tag).unwrap_or_else(SemVer::zero);
    if latest > current {
        Some(tag)
    } else {
        None
    }
}

// ── Asset naming (shared with bootstrap) ─────────────────────────────────────

/// Returns the platform asset name for the current binary, e.g.
/// `"claudio-linux-x86_64"`.
pub fn current_asset_name() -> Result<String, String> {
    use crate::remote::bootstrap::{release_arch, release_os};
    let os = std::env::consts::OS; // "linux" | "macos" | ...
    let probe_os = match os {
        "linux" => "linux",
        "macos" => "macos",
        other => {
            return Err(format!(
                "unsupported OS {other:?} — install claudio manually"
            ))
        }
    };
    let probe_arch = std::env::consts::ARCH; // "x86_64" | "aarch64" | ...
    Ok(format!(
        "claudio-{}-{}",
        release_os(probe_os)?,
        release_arch(probe_arch)?
    ))
}

// ── Download helpers ──────────────────────────────────────────────────────────

const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);

/// Download `url`, returning the raw bytes.
async fn download_bytes(url: &str) -> Result<Vec<u8>, String> {
    let client = reqwest::Client::builder()
        .timeout(DOWNLOAD_TIMEOUT)
        .user_agent(format!("claudio/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("cannot build HTTP client: {e}"))?;

    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("download failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("server returned {}", resp.status()));
    }

    resp.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| format!("read failed: {e}"))
}

/// Verify SHA-256 of `data` against the first hex word in `checksum_line`.
fn verify_sha256(data: &[u8], checksum_line: &str) -> Result<(), String> {
    use sha2::Digest;
    let expected = checksum_line
        .split_ascii_whitespace()
        .next()
        .ok_or("empty checksum file")?;
    let mut hasher = sha2::Sha256::new();
    hasher.update(data);
    let actual = format!("{:x}", hasher.finalize());
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "checksum mismatch (expected {expected}, got {actual})"
        ))
    }
}

// ── `claudio upgrade` ─────────────────────────────────────────────────────────

/// Download and install the latest release if it is newer than the running
/// binary.  Returns an exit code.
pub fn upgrade_cmd(check_only: bool) -> std::process::ExitCode {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("claudio upgrade: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    rt.block_on(run_upgrade(check_only, &api_url()))
}

/// Injectable base-URL variant used by tests.
pub async fn run_upgrade(
    check_only: bool,
    api_base: &str,
) -> std::process::ExitCode {
    // 1. Fetch latest tag.
    let tag = match latest_tag_from(api_base).await {
        Ok(t) => t,
        Err(e) => {
            eprintln!("claudio upgrade: cannot reach the releases repo: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };

    let current = SemVer::parse(env!("CARGO_PKG_VERSION")).unwrap_or_else(SemVer::zero);
    let latest = SemVer::parse(&tag).unwrap_or_else(SemVer::zero);

    if latest <= current {
        println!(
            "claudio is up to date (v{})",
            env!("CARGO_PKG_VERSION")
        );
        return std::process::ExitCode::SUCCESS;
    }

    println!("New version available: {tag} (current: v{})", env!("CARGO_PKG_VERSION"));

    if check_only {
        println!("Run `claudio upgrade` to install.");
        return std::process::ExitCode::SUCCESS;
    }

    // 2. Determine asset for this platform.
    let asset_name = match current_asset_name() {
        Ok(n) => n,
        Err(e) => {
            eprintln!("claudio upgrade: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };

    // Build the download base from the api_base (strip /releases/latest → versioned).
    // For tests the api_base is a mock server URL; we derive the asset URL from it.
    let download_base = derive_download_base(api_base, &tag);

    // 3. Download binary + checksum.
    let asset_url = format!("{download_base}/{asset_name}");
    let hash_url = format!("{download_base}/{asset_name}.sha256");

    println!("Downloading {asset_name}…");
    let binary_bytes = match download_bytes(&asset_url).await {
        Ok(b) => b,
        Err(e) => {
            eprintln!("claudio upgrade: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };

    let hash_bytes = match download_bytes(&hash_url).await {
        Ok(b) => b,
        Err(e) => {
            eprintln!("claudio upgrade: cannot download checksum: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };

    let checksum_str = String::from_utf8_lossy(&hash_bytes);
    if let Err(e) = verify_sha256(&binary_bytes, checksum_str.trim()) {
        eprintln!("claudio upgrade: {e}");
        return std::process::ExitCode::FAILURE;
    }
    println!("Checksum verified.");

    // 4. Replace current binary atomically.
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("claudio upgrade: cannot find own binary: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };

    if let Err(e) = install_binary(&exe, &binary_bytes) {
        eprintln!("claudio upgrade: {e}");
        return std::process::ExitCode::FAILURE;
    }

    println!("Installed {tag} → {}", exe.display());

    // 5. Restart daemon so sessions continue on the new binary.
    let daemon_running = std::process::Command::new(&exe)
        .args(["daemon", "status"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if daemon_running {
        println!("Restarting daemon…");
        let _ = std::process::Command::new(&exe)
            .args(["daemon", "restart"])
            .status();
    }

    std::process::ExitCode::SUCCESS
}

/// Derive a download URL base from the API URL and tag.
///
/// - Real GitHub API:  `https://api.github.com/repos/p4u/claudio-releases/releases/latest`
///   → `https://github.com/p4u/claudio-releases/releases/download/<tag>`
/// - Mock server (tests):  `http://127.0.0.1:PORT/releases/latest`
///   → `http://127.0.0.1:PORT/releases/download/<tag>`
fn derive_download_base(api_base: &str, tag: &str) -> String {
    if api_base.contains("api.github.com/repos/") {
        // Real GitHub: translate API URL to download URL.
        // api.github.com/repos/p4u/claudio-releases → github.com/p4u/claudio-releases
        let repo_path = api_base
            .trim_start_matches("https://api.github.com/repos/")
            .split("/releases")
            .next()
            .unwrap_or(RELEASES_REPO);
        format!("https://github.com/{repo_path}/releases/download/{tag}")
    } else {
        // Test/mock server: the api_base ends with /releases/latest.
        // Replace that suffix with /releases/download/<tag>.
        if let Some(prefix) = api_base.strip_suffix("/releases/latest") {
            format!("{prefix}/releases/download/{tag}")
        } else if let Some(prefix) = api_base.strip_suffix("/latest") {
            format!("{prefix}/download/{tag}")
        } else {
            format!("{api_base}/download/{tag}")
        }
    }
}

/// Write `binary_bytes` to a temp file next to `exe_path`, then atomically
/// rename it over `exe_path`.  Sets mode 0755.
fn install_binary(exe_path: &std::path::Path, binary_bytes: &[u8]) -> io::Result<()> {
    let dir = exe_path.parent().ok_or_else(|| {
        io::Error::other("cannot determine install directory from current_exe()")
    })?;

    // Check we can write to the install directory.
    let meta = std::fs::metadata(dir)?;
    if meta.permissions().readonly() {
        return Err(io::Error::other(format!(
            "install directory {} is read-only; try `make install` or the install script with sudo",
            dir.display()
        )));
    }

    // Write to a unique temp file.
    let tmp = dir.join(format!(".claudio-upgrade-{}.tmp", uuid::Uuid::new_v4().simple()));
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o755)
            .open(&tmp)?;
        f.write_all(binary_bytes)?;
        f.sync_all()?;
    }
    // Atomic rename.
    if let Err(e) = std::fs::rename(&tmp, exe_path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Version comparison ────────────────────────────────────────────────────

    #[test]
    fn semver_parse() {
        assert_eq!(
            SemVer::parse("1.2.3"),
            Some(SemVer { major: 1, minor: 2, patch: 3 })
        );
        assert_eq!(
            SemVer::parse("v0.2.0"),
            Some(SemVer { major: 0, minor: 2, patch: 0 })
        );
        assert_eq!(SemVer::parse("bad"), None);
        // Trailing pre-release suffix on patch.
        assert_eq!(
            SemVer::parse("1.2.3-alpha"),
            Some(SemVer { major: 1, minor: 2, patch: 3 })
        );
    }

    #[test]
    fn semver_comparison() {
        let v020 = SemVer::parse("0.2.0").unwrap();
        let v030 = SemVer::parse("0.3.0").unwrap();
        let v100 = SemVer::parse("1.0.0").unwrap();
        assert!(v030 > v020);
        assert!(v100 > v030);
        assert_eq!(v020, v020);
    }

    // ── Cache TTL ─────────────────────────────────────────────────────────────

    #[test]
    fn cache_fresh_within_24h() {
        let now = paths::unix_now();
        let cache = UpdateCache {
            last_check: now - 3600, // 1 hour ago
            latest_tag: "v0.3.0".into(),
        };
        assert!(cache.is_fresh());
    }

    #[test]
    fn cache_stale_after_24h() {
        let now = paths::unix_now();
        let cache = UpdateCache {
            last_check: now - 90_000, // 25 hours ago
            latest_tag: "v0.3.0".into(),
        };
        assert!(!cache.is_fresh());
    }

    // ── Asset naming ─────────────────────────────────────────────────────────

    #[test]
    fn current_asset_name_parses() {
        // We can't know the exact name in CI, but it must not error on the
        // current platform (linux or macos, x86_64 or aarch64).
        let name = current_asset_name();
        assert!(
            name.is_ok(),
            "current_asset_name failed: {:?}",
            name.err()
        );
        let name = name.unwrap();
        assert!(name.starts_with("claudio-"), "unexpected name: {name}");
    }

    // ── derive_download_base ──────────────────────────────────────────────────

    #[test]
    fn derive_download_base_github() {
        let api = "https://api.github.com/repos/p4u/claudio-releases/releases/latest";
        let base = derive_download_base(api, "v0.3.0");
        assert_eq!(
            base,
            "https://github.com/p4u/claudio-releases/releases/download/v0.3.0"
        );
    }

    #[test]
    fn derive_download_base_mock() {
        let api = "http://127.0.0.1:12345/releases/latest";
        let base = derive_download_base(api, "v0.3.0");
        assert_eq!(
            base,
            "http://127.0.0.1:12345/releases/download/v0.3.0"
        );
    }

    // ── install_binary (replace-by-rename) ───────────────────────────────────

    #[test]
    fn install_binary_replaces_file() {
        // Create a fake "exe" in a temp dir.
        let tmp = std::env::temp_dir()
            .join(format!("claudio-upgrade-test-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&tmp).unwrap();
        let exe = tmp.join("claudio-fake");
        std::fs::write(&exe, b"old content").unwrap();

        install_binary(&exe, b"new content").unwrap();

        let content = std::fs::read(&exe).unwrap();
        assert_eq!(&content, b"new content");

        // Verify mode 0755.
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&exe).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755, "unexpected mode {:o}", mode & 0o777);

        std::fs::remove_dir_all(&tmp).unwrap();
    }

    // ── Mock-server upgrade flow ──────────────────────────────────────────────
    //
    // Starts a local axum server that serves:
    //   GET /releases/latest          → JSON with tag_name
    //   GET /releases/download/TAG/ASSET   → fake binary bytes
    //   GET /releases/download/TAG/ASSET.sha256 → sha256 of fake binary

    #[tokio::test]
    async fn upgrade_flow_mock_server() {
        use axum::routing::get;
        use axum::Router;
        use sha2::Digest;

        // Fake binary content.
        let fake_binary = b"fake claudio binary v0.3.0";
        let mut hasher = sha2::Sha256::new();
        hasher.update(fake_binary);
        let hash_hex = format!("{:x}", hasher.finalize());
        let fake_hash_file = format!("{hash_hex}  claudio-linux-x86_64\n");

        let fake_binary_b = fake_binary.to_vec();
        let fake_hash_b = fake_hash_file.clone();

        // Bind tokio listener directly (avoids the blocking-socket restriction).
        let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = tcp.local_addr().unwrap();

        // Build axum router.
        let tag_route = get(|| async {
            r#"{"tag_name":"v0.3.0","name":"v0.3.0"}"#
        });
        let bin_route = get(move || {
            let b = fake_binary_b.clone();
            async move { b }
        });
        let hash_route = get(move || {
            let h = fake_hash_b.clone();
            async move { h }
        });

        let app = Router::new()
            .route("/releases/latest", tag_route)
            .route("/releases/download/v0.3.0/claudio-linux-x86_64", bin_route)
            .route(
                "/releases/download/v0.3.0/claudio-linux-x86_64.sha256",
                hash_route,
            );

        tokio::spawn(async move {
            axum::serve(tcp, app).await.unwrap();
        });

        // Prepare a temp "exe" to be replaced.
        let tmp = std::env::temp_dir()
            .join(format!("claudio-mock-upgrade-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&tmp).unwrap();
        let fake_exe = tmp.join("claudio-linux-x86_64");
        std::fs::write(&fake_exe, b"old").unwrap();

        // Patch std::env::current_exe to point at our fake_exe.
        // We can't easily override current_exe(), so we test install_binary directly
        // and just verify run_upgrade makes the right HTTP calls via latest_tag_from.

        let api_url = format!("http://127.0.0.1:{}/releases/latest", addr.port());
        let tag = latest_tag_from(&api_url).await.unwrap();
        assert_eq!(tag, "v0.3.0");

        // Derive download base.
        let base = derive_download_base(&api_url, &tag);
        let asset = "claudio-linux-x86_64";
        let bin_url = format!("{base}/{asset}");
        let hash_url_s = format!("{base}/{asset}.sha256");

        let bin = download_bytes(&bin_url).await.unwrap();
        let hash = download_bytes(&hash_url_s).await.unwrap();
        let hash_str = String::from_utf8_lossy(&hash);

        verify_sha256(&bin, hash_str.trim()).unwrap();
        install_binary(&fake_exe, &bin).unwrap();

        let content = std::fs::read(&fake_exe).unwrap();
        assert_eq!(content, fake_binary);

        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
