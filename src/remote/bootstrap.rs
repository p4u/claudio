//! Remote host bootstrap: probe, upload and verify the claudio binary.
//!
//! [`ensure_remote`] runs `~/.local/bin/claudio __probe` on the remote host
//! and returns a [`RemoteInfo`] on success. If the binary is missing or
//! out-of-date it uploads the correct binary.
//!
//! Cross-platform install: the remote OS/arch is discovered independently
//! with `uname -sm` *in the same ssh round trip* as the probe; never assume
//! local == remote. Downloaded release assets are cached under
//! `~/.cache/claudio/<version>/` keyed by version so freshness can be
//! checked without a re-download.
//!
//! **No remote dotfiles are edited.** The binary is always installed at
//! `~/.local/bin/claudio` by absolute path.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use super::probe::{sha256_file, Probe};
use super::{shell_quote, ssh_cmd, validate_host};

/// GitHub release base URL.
const RELEASE_BASE: &str = "https://github.com/p4u/claudio/releases/latest/download";

/// How long a single ssh command may run before we give up.
const SSH_TIMEOUT: Duration = Duration::from_secs(60);

/// How long a curl download may run.
const CURL_TIMEOUT: Duration = Duration::from_secs(120);

/// Information returned after a successful bootstrap.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct RemoteInfo {
    /// `true` when the remote binary was already up to date.
    pub was_current: bool,
    /// The remote probe (version, proto, os, arch) after any install.
    pub probe: Probe,
}

/// Ensure `~/.local/bin/claudio` is installed on `host` and is up to date.
///
/// Runs `__probe` (and `uname -sm` when no binary exists), uploads if needed,
/// and returns [`RemoteInfo`]. Errors are returned as a human-readable string
/// for display in the TUI.
pub async fn ensure_remote(host: &str) -> Result<RemoteInfo, String> {
    validate_host(host).map_err(|e| e)?;

    let local = Probe::current().map_err(|e| format!("cannot read local binary: {e}"))?;

    // Step 1: probe the remote *and* discover its OS/arch in one round trip.
    // We run both commands joined with `; echo ---` so a missing binary
    // doesn't prevent uname from running.
    let (remote_probe, remote_os, remote_arch) = probe_and_platform(host).await?;

    // Step 2: check freshness.
    if let Some(ref r) = remote_probe {
        if is_up_to_date(r, &local, &remote_os, &remote_arch).await {
            return Ok(RemoteInfo { was_current: true, probe: r.clone() });
        }
    }

    // Step 3: upload the correct binary.
    let same_platform = local.os == remote_os && local.arch == remote_arch;
    if same_platform {
        upload_self(host, &remote_os, &remote_arch).await?;
    } else {
        upload_release(host, &remote_os, &remote_arch).await?;
    }

    // Step 4: re-probe to confirm.
    let new_probe = probe_remote(host)
        .await
        .ok_or_else(|| format!("claudio was uploaded to {host} but __probe failed afterwards"))?;

    Ok(RemoteInfo { was_current: false, probe: new_probe })
}

/// Map probe OS name to release asset OS name.
///
/// The probe reports `"macos"` but release assets are named `claudio-darwin-*`.
pub fn release_os(probe_os: &str) -> Result<&str, String> {
    match probe_os {
        "linux" => Ok("linux"),
        "macos" => Ok("darwin"),
        other => Err(format!(
            "unsupported remote OS {other:?} — install claudio manually on the remote host"
        )),
    }
}

/// Map probe arch name to release asset arch name.
pub fn release_arch(probe_arch: &str) -> Result<&str, String> {
    match probe_arch {
        "x86_64" | "aarch64" => Ok(probe_arch),
        other => Err(format!(
            "unsupported remote arch {other:?} — install claudio manually on the remote host"
        )),
    }
}

/// Build the release asset name, e.g. `claudio-linux-x86_64`.
pub fn release_asset_name(probe_os: &str, probe_arch: &str) -> Result<String, String> {
    Ok(format!("claudio-{}-{}", release_os(probe_os)?, release_arch(probe_arch)?))
}

/// Local cache dir for downloaded release assets.
fn asset_cache_dir(version: &str) -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    home.join(".cache/claudio").join(version)
}

/// Cached path for a release asset.
fn cached_asset_path(version: &str, asset_name: &str) -> PathBuf {
    asset_cache_dir(version).join(asset_name)
}

/// Check whether the remote binary is current.
///
/// - Same-platform: compare `remote.build` vs sha256 of the local binary.
/// - Cross-platform: compare `remote.build` vs sha256 of the cached release
///   asset (so a correct cross-platform install is not re-uploaded every time).
async fn is_up_to_date(
    remote: &Probe,
    local: &Probe,
    remote_os: &str,
    remote_arch: &str,
) -> bool {
    if remote.version != local.version || remote.proto != local.proto {
        return false;
    }
    let same_platform = remote_os == local.os && remote_arch == local.arch;
    if same_platform {
        remote.build == local.build
    } else {
        // Cross-platform: check against the cached release asset.
        let asset_name = match release_asset_name(remote_os, remote_arch) {
            Ok(n) => n,
            Err(_) => return false,
        };
        let cache = cached_asset_path(&local.version, &asset_name);
        if let Ok(hash) = sha256_file(&cache) {
            remote.build == hash
        } else {
            false // Not cached → assume stale.
        }
    }
}

/// Run `__probe` on the remote and discover its OS/arch with `uname -sm`
/// in a single ssh round trip.
///
/// Returns `(Option<Probe>, os_str, arch_str)`.
async fn probe_and_platform(host: &str) -> Result<(Option<Probe>, String, String), String> {
    // Run probe and uname together. If the binary doesn't exist `__probe`
    // exits non-zero, but uname still runs.
    let home_quoted = r#""$HOME""#;
    let cmd = format!(
        r#"{home_quoted}/.local/bin/claudio __probe 2>/dev/null; echo '---UNAME---'; uname -sm"#
    );
    let output = tokio::time::timeout(SSH_TIMEOUT, ssh_cmd(host).arg(&cmd).output())
        .await
        .map_err(|_| format!("ssh probe/uname timed out on {host}"))?
        .map_err(|e| format!("ssh failed: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let (probe_part, uname_part) =
        stdout.split_once("---UNAME---").unwrap_or(("", stdout.as_ref()));

    // Parse uname output: e.g. "Linux x86_64" or "Darwin arm64"
    let uname_line = uname_part.trim();
    let mut uparts = uname_line.split_ascii_whitespace();
    let uname_os = uparts.next().unwrap_or("");
    let uname_arch = uparts.next().unwrap_or("");

    let remote_os = match uname_os {
        "Linux" => "linux".to_owned(),
        "Darwin" => "macos".to_owned(),
        other => return Err(format!("unknown remote OS from uname: {other:?}")),
    };
    let remote_arch = match uname_arch {
        "x86_64" => "x86_64".to_owned(),
        "aarch64" | "arm64" => "aarch64".to_owned(),
        other => return Err(format!("unknown remote arch from uname: {other:?}")),
    };

    let probe = probe_part.lines().find_map(Probe::parse);
    Ok((probe, remote_os, remote_arch))
}

/// Run `~/.local/bin/claudio __probe` on the remote and parse the result.
/// Returns `None` when the binary is missing or the output is unparseable.
async fn probe_remote(host: &str) -> Option<Probe> {
    let output = tokio::time::timeout(SSH_TIMEOUT, async {
        ssh_cmd(host).arg(r#""$HOME"/.local/bin/claudio __probe 2>/dev/null"#).output().await
    })
    .await
    .ok()?
    .ok()?;
    let stdout = std::str::from_utf8(&output.stdout).ok()?;
    stdout.lines().find_map(Probe::parse)
}

/// Upload this binary to `host` via ssh, verify the hash remotely and install.
async fn upload_self(host: &str, remote_os: &str, remote_arch: &str) -> Result<(), String> {
    let _ = (remote_os, remote_arch); // confirmed same-platform by caller
    let exe = std::env::current_exe().map_err(|e| format!("cannot find own binary: {e}"))?;
    let expected_hash =
        sha256_file(&exe).map_err(|e| format!("cannot hash own binary: {e}"))?;
    upload_bytes(host, &exe, &expected_hash).await
}

/// Download the release asset for `(os, arch)`, verify its hash locally,
/// cache it, then upload it to the remote host.
async fn upload_release(host: &str, remote_os: &str, remote_arch: &str) -> Result<(), String> {
    let asset_name = release_asset_name(remote_os, remote_arch)?;
    let version = env!("CARGO_PKG_VERSION");

    let cache_path = cached_asset_path(version, &asset_name);

    // Use cache if available and hash-verified.
    let expected_hash = if cache_path.exists() {
        sha256_file(&cache_path).map_err(|e| format!("cannot hash cached asset: {e}"))?
    } else {
        // Download to cache.
        let asset_url = format!("{RELEASE_BASE}/{asset_name}");
        let hash_url = format!("{RELEASE_BASE}/{asset_name}.sha256");

        let tmp_dir = std::env::temp_dir()
            .join(format!("claudio-bootstrap-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&tmp_dir)
            .await
            .map_err(|e| format!("cannot create temp dir: {e}"))?;
        let asset_path = tmp_dir.join(&asset_name);
        let hash_path = tmp_dir.join(format!("{asset_name}.sha256"));

        let dl_result: Result<(), String> = async {
            download_asset(&asset_url, &asset_path).await?;
            download_asset(&hash_url, &hash_path).await
        }
        .await;

        if let Err(e) = dl_result {
            let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
            return Err(format!(
                "could not download {asset_name} from GitHub: {e}. \
                 Check that the release has this asset, or install claudio manually on {host}."
            ));
        }

        // Parse the expected hash from the sidecar file.
        let hash_content = tokio::fs::read_to_string(&hash_path)
            .await
            .map_err(|e| format!("cannot read hash file: {e}"))?;
        let expected = hash_content
            .split_ascii_whitespace()
            .next()
            .map(str::to_owned)
            .ok_or_else(|| "empty sha256 sidecar file".to_owned())?;

        // Verify locally before upload.
        let actual_hash =
            sha256_file(&asset_path).map_err(|e| format!("cannot hash downloaded asset: {e}"))?;
        if actual_hash != expected {
            let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
            return Err(format!(
                "downloaded {asset_name} hash mismatch (expected {expected}, got {actual_hash})"
            ));
        }

        // Save to cache.
        if let Ok(()) = tokio::fs::create_dir_all(cache_path.parent().unwrap_or(Path::new("."))).await {
            let _ = tokio::fs::copy(&asset_path, &cache_path).await;
        }

        let upload_result = upload_bytes(host, &asset_path, &expected).await;
        let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
        return upload_result;
    };

    upload_bytes(host, &cache_path, &expected_hash).await
}

/// Stream `local_path` into a remote temp file (via `mktemp`), verify the
/// remote SHA-256, take a remote install lock, chmod, and atomically rename
/// to `~/.local/bin/claudio`.
async fn upload_bytes(
    host: &str,
    local_path: &std::path::Path,
    expected_hash: &str,
) -> Result<(), String> {
    // Ensure the destination directory exists and get a unique temp path.
    let mkdir_and_mktemp = r#"mkdir -p "$HOME/.local/bin" && mktemp "$HOME/.local/bin/.claudio.XXXXXX""#;
    let tmp_remote = ssh_run(host, mkdir_and_mktemp)
        .await
        .map_err(|e| format!("remote mktemp failed: {e}"))?;
    let tmp_remote = tmp_remote.trim().to_owned();
    if tmp_remote.is_empty() {
        return Err(format!("mktemp returned empty output on {host}"));
    }

    // Read the binary into memory (typically a few MB).
    let bytes = tokio::fs::read(local_path)
        .await
        .map_err(|e| format!("cannot read {}: {e}", local_path.display()))?;

    let tmp_quoted = shell_quote(&tmp_remote);

    // Upload: pipe into ssh's stdin, which writes to the temp file.
    let upload_cmd = format!("cat > {tmp_quoted}");
    let status = tokio::time::timeout(SSH_TIMEOUT, async {
        let mut child = ssh_cmd(host)
            .arg(&upload_cmd)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("ssh spawn failed: {e}"))?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(&bytes).await.map_err(|e| format!("ssh stdin write: {e}"))?;
            drop(stdin);
        }
        child.wait().await.map_err(|e| format!("ssh wait: {e}"))
    })
    .await
    .map_err(|_| format!("upload to {host} timed out"))??;

    if !status.success() {
        let _ = ssh_run(host, &format!("rm -f {tmp_quoted} 2>/dev/null; true")).await;
        return Err(format!(
            "upload command failed on {host} (exit {})",
            status.code().unwrap_or(-1)
        ));
    }

    // Verify remote hash.
    // tmp_quoted is single-quoted (absolute path from mktemp), safe to use directly.
    let verify_cmd = format!(
        "_tmp={tmp_quoted}; sha256sum \"$_tmp\" 2>/dev/null || shasum -a 256 \"$_tmp\" 2>/dev/null"
    );
    let hash_out = ssh_run(host, &verify_cmd).await.map_err(|e| {
        let _ = ssh_run_sync(host, &format!("rm -f {tmp_quoted}"));
        format!("remote hash check failed: {e}")
    })?;
    let remote_hash = hash_out.split_ascii_whitespace().next().unwrap_or("").to_owned();
    if remote_hash != expected_hash {
        let _ = ssh_run(host, &format!("rm -f {tmp_quoted} 2>/dev/null; true")).await;
        return Err(format!(
            "remote hash mismatch after upload (expected {expected_hash}, got {remote_hash})"
        ));
    }

    // Serialize install with a remote flock (or mkdir fallback).
    // chmod and atomic rename under the lock.
    // NOTE: $HOME-relative paths use double-quote expansion, not shell_quote,
    // because shell_quote wraps in single quotes which suppress $HOME expansion.
    // The tmp path (from mktemp) is single-quoted since it is absolute.
    let install_cmd = format!(
        r#"_tmp={tmp_quoted}
_dest="$HOME/.local/bin/claudio"
_lock="$HOME/.local/bin/.claudio.install.lock"
if command -v flock >/dev/null 2>&1; then
  flock "$_lock" sh -c "chmod 755 '$_tmp' && mv -f '$_tmp' '$_dest'"
else
  _lk="$HOME/.local/bin/.claudio.install.lock.d"
  for _i in 1 2 3 4 5; do mkdir "$_lk" 2>/dev/null && break; sleep 1; done
  chmod 755 "$_tmp" && mv -f "$_tmp" "$_dest"
  rmdir "$_lk" 2>/dev/null; true
fi"#
    );
    ssh_run(host, &install_cmd).await.map_err(|e| {
        let _ = ssh_run_sync(host, &format!("rm -f {tmp_quoted}"));
        format!("install failed on {host}: {e}")
    })?;

    Ok(())
}

/// Run a shell command on `host` and return its trimmed stdout.
async fn ssh_run(host: &str, cmd: &str) -> Result<String, String> {
    let output = tokio::time::timeout(SSH_TIMEOUT, async {
        ssh_cmd(host).arg(cmd).output().await
    })
    .await
    .map_err(|_| format!("ssh command timed out on {host}"))?
    .map_err(|e| format!("ssh command failed: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "command failed (exit {}): {}",
            output.status.code().unwrap_or(-1),
            stderr.trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Synchronous ssh run used only for async-context cleanup (e.g. removing a
/// temp file after a failed hash check). Must not be called from within a
/// tokio worker thread — only from `spawn_blocking` contexts or after the
/// async runtime has exited. In practice we call it only on error paths that
/// don't need the result.
fn ssh_run_sync(host: &str, cmd: &str) {
    let _ = std::process::Command::new("ssh")
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10", "--", host, cmd])
        .output();
}

/// Download a URL with `curl` to a local file.
async fn download_asset(url: &str, dest: &std::path::Path) -> Result<(), String> {
    let dest_str = dest.to_string_lossy().to_string();
    let status = tokio::time::timeout(CURL_TIMEOUT, async {
        Command::new("curl")
            .args(["-fsSL", "--output", &dest_str, url])
            .status()
            .await
    })
    .await
    .map_err(|_| format!("curl download timed out for {url}"))?
    .map_err(|e| format!("curl failed: {e}"))?;
    if !status.success() {
        return Err(format!("curl exited with {}", status.code().unwrap_or(-1)));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_os_maps_correctly() {
        assert_eq!(release_os("linux").unwrap(), "linux");
        assert_eq!(release_os("macos").unwrap(), "darwin");
        assert!(release_os("windows").is_err());
    }

    #[test]
    fn release_arch_maps_correctly() {
        assert_eq!(release_arch("x86_64").unwrap(), "x86_64");
        assert_eq!(release_arch("aarch64").unwrap(), "aarch64");
        assert!(release_arch("mips").is_err());
    }

    #[test]
    fn release_asset_name_macos() {
        assert_eq!(release_asset_name("macos", "aarch64").unwrap(), "claudio-darwin-aarch64");
        assert_eq!(release_asset_name("linux", "x86_64").unwrap(), "claudio-linux-x86_64");
    }

    #[test]
    fn shell_quote_in_remote_cmd() {
        // Simulate building a remote command for a home dir with spaces.
        let home = "/home/my user";
        let bin = format!("{home}/.local/bin/claudio");
        let cmd = format!("cat > {}", shell_quote(&bin));
        assert_eq!(cmd, "cat > '/home/my user/.local/bin/claudio'");

        // Verify a path with single quote in it.
        let tricky = "/home/o'brien/.local/bin/claudio";
        let quoted = shell_quote(tricky);
        assert_eq!(quoted, r"'/home/o'\''brien/.local/bin/claudio'");
    }

    #[test]
    fn validate_host_rejects_injection() {
        assert!(validate_host("-oProxyCommand=evil").is_err());
        assert!(validate_host("").is_err());
    }

    #[tokio::test]
    async fn is_up_to_date_same_platform_version_mismatch() {
        let local = Probe {
            version: "1.0.0".into(),
            proto: 1,
            os: "linux".into(),
            arch: "x86_64".into(),
            build: "aabbcc".into(),
        };
        let remote_old = Probe { version: "0.9.0".into(), ..local.clone() };
        assert!(!is_up_to_date(&remote_old, &local, "linux", "x86_64").await);
    }

    #[tokio::test]
    async fn is_up_to_date_same_platform_hash_match() {
        let local = Probe {
            version: "1.0.0".into(),
            proto: 1,
            os: "linux".into(),
            arch: "x86_64".into(),
            build: "aabbcc".into(),
        };
        let remote = local.clone();
        assert!(is_up_to_date(&remote, &local, "linux", "x86_64").await);
    }

    #[tokio::test]
    async fn is_up_to_date_cross_platform_no_cache_is_stale() {
        let local = Probe {
            version: "1.0.0".into(),
            proto: 1,
            os: "linux".into(),
            arch: "x86_64".into(),
            build: "locallinuxhash".into(),
        };
        let remote = Probe { os: "macos".into(), arch: "aarch64".into(), build: "macoshash".into(), ..local.clone() };
        // No cache → stale
        assert!(!is_up_to_date(&remote, &local, "macos", "aarch64").await);
    }
}
