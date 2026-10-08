//! Where claudio keeps its files.
//!
//! Everything persistent lives under `~/.config/claudio` (or
//! `$XDG_CONFIG_HOME/claudio`), on every host. Sockets go to the per-user
//! runtime dir instead: Unix sockets misbehave on network-mounted homes and
//! should not outlive a reboot.

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::proto::PROTO;

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

pub fn uid() -> u32 {
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

/// `~/.config/claudio` (honours `$XDG_CONFIG_HOME`).
pub fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"))
        .join("claudio")
}

/// `$XDG_RUNTIME_DIR/claudio`, or `/tmp/claudio-<uid>` where there is none
/// (macOS, non-systemd hosts).
pub fn runtime_dir() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir).join("claudio"),
        None => std::env::temp_dir().join(format!("claudio-{}", uid())),
    }
}

/// The daemon's control socket. Versioned, so an upgraded client starts a new
/// daemon beside an old one instead of breaking its live sessions.
pub fn daemon_socket() -> PathBuf {
    runtime_dir().join(format!("daemon-v{PROTO}.sock"))
}

/// Lock file serializing daemon startup.
pub fn daemon_lock() -> PathBuf {
    runtime_dir().join(format!("daemon-v{PROTO}.lock"))
}

/// The daemon's journal of sessions on this host.
pub fn daemon_journal() -> PathBuf {
    config_dir().join("daemon-sessions.json")
}

/// The TUI client's saved state (open sessions, order, names).
pub fn client_state() -> PathBuf {
    config_dir().join("state.json")
}

/// User configuration.
pub fn config_file() -> PathBuf {
    config_dir().join("config.toml")
}

/// Create `dir` (mode 0700) if needed and verify it is a real directory owned
/// by us and private — the runtime dir may live in a shared `/tmp`.
pub fn ensure_private_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let meta = fs::symlink_metadata(dir)?;
    if !meta.file_type().is_dir() {
        return Err(io::Error::other(format!("{} is not a directory", dir.display())));
    }
    if meta.uid() != uid() {
        return Err(io::Error::other(format!("{} is owned by another user", dir.display())));
    }
    if meta.permissions().mode() & 0o077 != 0 {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Atomically replace `path` with `contents` (temp file + fsync + rename, mode
/// 0600), creating the parent directory. Used for every state/journal file.
pub fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(dir)?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("state");
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    f.write_all(contents)?;
    f.sync_all()?;
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        std::env::temp_dir().join(format!("claudio-paths-test-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn socket_name_carries_protocol_version() {
        let s = daemon_socket();
        assert!(s.to_string_lossy().ends_with(&format!("daemon-v{PROTO}.sock")));
    }

    #[test]
    fn private_dir_is_created_0700_and_tightened() {
        let base = scratch();
        let dir = base.join("rt");
        ensure_private_dir(&dir).unwrap();
        assert_eq!(fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        ensure_private_dir(&dir).unwrap();
        assert_eq!(fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn private_dir_rejects_symlink() {
        let base = scratch();
        fs::create_dir_all(base.join("real")).unwrap();
        std::os::unix::fs::symlink(base.join("real"), base.join("link")).unwrap();
        assert!(ensure_private_dir(&base.join("link")).is_err());
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn write_atomic_replaces_contents_privately() {
        let base = scratch();
        let p = base.join("sub/state.json");
        write_atomic(&p, b"one").unwrap();
        write_atomic(&p, b"two").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"two");
        assert_eq!(fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        fs::remove_dir_all(&base).unwrap();
    }
}
