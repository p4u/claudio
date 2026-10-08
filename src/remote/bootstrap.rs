//! Remote host bootstrap: probe, upload and verify the claudio binary.
//!
//! [`ensure_remote`] runs `~/.local/bin/claudio __probe` on the remote host
//! and returns a [`RemoteInfo`] on success. If the binary is missing or
//! out-of-date for the same os/arch, it uploads the current executable. If
//! the os/arch differs it downloads the matching release asset from GitHub
//! and uploads that.
//!
//! **No remote dotfiles are edited.** The binary is always installed at
//! `~/.local/bin/claudio` by absolute path, and the remote daemon is invoked
//! there too.

use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use super::probe::{sha256_file, Probe};

/// GitHub release base URL.
const RELEASE_BASE: &str = "https://github.com/p4u/claudio/releases/latest/download";

/// How long a single ssh command may run before we give up.
const SSH_TIMEOUT: Duration = Duration::from_secs(60);

/// Information returned after a successful bootstrap.
#[derive(Debug, Clone)]
pub struct RemoteInfo {
    /// `true` when the remote binary was already up to date.
    pub was_current: bool,
    /// The remote probe (version, proto, os, arch) after any install.
    pub probe: Probe,
}

/// Ensure `~/.local/bin/claudio` is installed on `host` and is up to date.
///
/// Runs `__probe`, uploads if needed, and returns [`RemoteInfo`]. Errors are
/// returned as a human-readable string for display in the TUI.
pub async fn ensure_remote(host: &str) -> Result<RemoteInfo, String> {
    let local = Probe::current().map_err(|e| format!("cannot read local binary: {e}"))?;

    // Step 1: probe the remote.
    let remote_probe = probe_remote(host).await;

    match remote_probe {
        Some(ref r) if r.is_up_to_date(&local) => {
            return Ok(RemoteInfo { was_current: true, probe: r.clone() });
        }
        _ => {}
    }

    // Step 2: upload.
    let target_os = remote_probe.as_ref().map_or_else(|| local.os.clone(), |r| r.os.clone());
    let target_arch = remote_probe.as_ref().map_or_else(|| local.arch.clone(), |r| r.arch.clone());

    let same_platform = remote_probe.as_ref().map_or(true, |r| r.same_platform(&local));

    if same_platform {
        upload_self(host).await?;
    } else {
        upload_release(host, &target_os, &target_arch).await?;
    }

    // Step 3: re-probe to confirm.
    let new_probe = probe_remote(host)
        .await
        .ok_or_else(|| format!("claudio was uploaded to {host} but __probe failed afterwards"))?;

    Ok(RemoteInfo { was_current: false, probe: new_probe })
}

/// Run `~/.local/bin/claudio __probe` on the remote and parse the result.
/// Returns `None` when the binary is missing or the output is unparseable.
async fn probe_remote(host: &str) -> Option<Probe> {
    let output = ssh_command(host)
        .arg("$HOME/.local/bin/claudio __probe 2>/dev/null")
        .output()
        .await
        .ok()?;
    if !output.status.success() && output.stdout.is_empty() {
        return None;
    }
    let stdout = std::str::from_utf8(&output.stdout).ok()?;
    stdout.lines().find_map(Probe::parse)
}

/// Upload this binary to `host` via ssh, verify the hash remotely and install.
async fn upload_self(host: &str) -> Result<(), String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("cannot find own binary: {e}"))?;
    let expected_hash = sha256_file(&exe)
        .map_err(|e| format!("cannot hash own binary: {e}"))?;
    upload_bytes(host, &exe, &expected_hash).await
}

/// Download the release asset for `(os, arch)`, verify its hash locally, then
/// upload it to the remote host.
async fn upload_release(host: &str, os: &str, arch: &str) -> Result<(), String> {
    let asset_name = format!("claudio-{os}-{arch}");
    let asset_url = format!("{RELEASE_BASE}/{asset_name}");
    let hash_url = format!("{RELEASE_BASE}/{asset_name}.sha256");

    // Download asset and its sha256 sidecar locally with curl.
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
        return Err(format!("could not download {asset_name} from GitHub: {e}. \
            Check that the release has this asset, or install claudio manually on {host}."));
    }

    // Parse the expected hash from the sidecar file.
    let hash_content = tokio::fs::read_to_string(&hash_path)
        .await
        .map_err(|e| format!("cannot read hash file: {e}"))?;
    let expected_hash = hash_content
        .split_ascii_whitespace()
        .next()
        .map(str::to_owned)
        .ok_or_else(|| "empty sha256 sidecar file".to_owned())?;

    // Verify locally before upload.
    let actual_hash = sha256_file(&asset_path)
        .map_err(|e| format!("cannot hash downloaded asset: {e}"))?;
    if actual_hash != expected_hash {
        let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
        return Err(format!(
            "downloaded {asset_name} hash mismatch (expected {expected_hash}, got {actual_hash})"
        ));
    }

    let upload_result = upload_bytes(host, &asset_path, &expected_hash).await;
    let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
    upload_result
}

/// Stream `local_path` into `~/.local/bin/.claudio.<random>.tmp` on `host`,
/// verify the remote SHA-256, chmod, and atomically rename to
/// `~/.local/bin/claudio`.
async fn upload_bytes(
    host: &str,
    local_path: &std::path::Path,
    expected_hash: &str,
) -> Result<(), String> {
    let rand_suffix: String = (0..8).map(|_| format!("{:02x}", rand_byte())).collect();
    let tmp_name = format!(".claudio.{rand_suffix}.tmp");
    let tmp_remote = format!("$HOME/.local/bin/{tmp_name}");
    let dest_remote = "$HOME/.local/bin/claudio";

    // Read the binary into memory (typically a few MB).
    let bytes = tokio::fs::read(local_path)
        .await
        .map_err(|e| format!("cannot read {}: {e}", local_path.display()))?;

    // Upload: pipe into ssh's stdin, which writes to a temp file.
    let upload_cmd = format!(
        "mkdir -p $HOME/.local/bin && cat > {tmp_remote}"
    );
    let status = tokio::time::timeout(SSH_TIMEOUT, async move {
        let mut child = ssh_command(host)
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
        // Try to clean up the temp file.
        let _ = ssh_run(host, &format!("rm -f {tmp_remote}")).await;
        return Err(format!("upload command failed on {host} (exit {})", status.code().unwrap_or(-1)));
    }

    // Verify remote hash.
    let verify_cmd = format!(
        "sha256sum {tmp_remote} 2>/dev/null || shasum -a 256 {tmp_remote} 2>/dev/null"
    );
    let out = ssh_run(host, &verify_cmd)
        .await
        .map_err(|e| {
            let _ = std::process::Command::new("ssh")
                .args(["-o", "BatchMode=yes", host, &format!("rm -f {tmp_remote}")])
                .output();
            format!("remote hash check failed: {e}")
        })?;
    let remote_hash = out.split_ascii_whitespace().next().unwrap_or("").to_owned();
    if remote_hash != expected_hash {
        let _ = ssh_run(host, &format!("rm -f {tmp_remote}")).await;
        return Err(format!(
            "remote hash mismatch after upload (expected {expected_hash}, got {remote_hash})"
        ));
    }

    // chmod and atomic rename.
    let install_cmd = format!("chmod 755 {tmp_remote} && mv -f {tmp_remote} {dest_remote}");
    let install_out = ssh_run(host, &install_cmd).await;
    if let Err(e) = install_out {
        let _ = ssh_run(host, &format!("rm -f {tmp_remote}")).await;
        return Err(format!("install failed on {host}: {e}"));
    }

    Ok(())
}

/// Build an `ssh -o BatchMode=yes HOST` command ready for a single argument
/// (the remote shell command).
fn ssh_command(host: &str) -> Command {
    let mut cmd = Command::new("ssh");
    cmd.args(["-o", "BatchMode=yes", host]);
    cmd
}

/// Run a shell command on `host` and return its trimmed stdout.
async fn ssh_run(host: &str, cmd: &str) -> Result<String, String> {
    let output = tokio::time::timeout(SSH_TIMEOUT, async {
        ssh_command(host)
            .arg(cmd)
            .output()
            .await
    })
    .await
    .map_err(|_| format!("ssh command timed out on {host}"))?
    .map_err(|e| format!("ssh command failed: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("command failed (exit {}): {}", output.status.code().unwrap_or(-1), stderr.trim()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Download a URL with `curl` to a local file.
async fn download_asset(url: &str, dest: &std::path::Path) -> Result<(), String> {
    let status = Command::new("curl")
        .args(["-fsSL", "--output", &dest.to_string_lossy(), url])
        .status()
        .await
        .map_err(|e| format!("curl failed: {e}"))?;
    if !status.success() {
        return Err(format!("curl exited with {}", status.code().unwrap_or(-1)));
    }
    Ok(())
}

/// A single random byte (simple entropy for temp file names).
fn rand_byte() -> u8 {
    // Use the process id + a counter as a basic entropy source.
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let v = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let pid = std::process::id() as u64;
    ((pid ^ v ^ (pid << 17) ^ (v << 3)).wrapping_mul(0x9e37_79b9_7f4a_7c15)) as u8
}
