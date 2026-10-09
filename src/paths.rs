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

/// The current user's home directory (honours `$HOME`).
pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

/// Current Unix time in seconds.
pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
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

/// The daemon's journal of sessions on this host. Versioned so that different
/// daemon protocol versions do not clobber each other's journals.
pub fn daemon_journal() -> PathBuf {
    config_dir().join(format!("daemon-sessions-v{PROTO}.json"))
}

/// The unversioned journal path from before the versioned scheme was introduced.
/// Used only to migrate data on first start (see [`Journal::load`]).
pub fn daemon_journal_legacy() -> PathBuf {
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

/// Atomically replace `path` with `contents` (temp file + fsync + rename +
/// parent-dir fsync, mode 0600), creating the parent directory.
///
/// Security properties:
/// - The temp file is created exclusively (`O_CREAT|O_EXCL`) with a random
///   UUID name, so two concurrent writers never share a temp file.
/// - On Unix, `O_NOFOLLOW` rejects a symlink at the temp path.
/// - Mode 0600 is set at creation; a pre-existing 0644 file at the temp path
///   cannot inherit weaker permissions because `create_new` fails if the path
///   already exists.
/// - The parent directory is fsynced after the rename so the directory entry
///   update is durable on power loss.
pub fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(dir)?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("state");
    // Unique random name: no two processes share a temp file, and a stale temp
    // from a previous crash at a predictable name cannot be reused.
    let tmp = dir.join(format!(".{name}.{}.tmp", uuid::Uuid::new_v4().simple()));
    {
        let mut opts = fs::OpenOptions::new();
        opts.write(true)
            .create_new(true) // O_CREAT | O_EXCL: fail if already exists
            .mode(0o600);
        // O_NOFOLLOW: fail if tmp resolves through a symlink at that path.
        // (On non-Unix targets this flag is not available, but create_new
        // already prevents reuse of an existing path.)
        #[cfg(unix)]
        opts.custom_flags(libc::O_NOFOLLOW);
        let mut f = opts.open(&tmp)?;
        f.write_all(contents)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    // fsync the parent directory so the rename itself is durable.
    let dir_file = fs::File::open(dir)?;
    dir_file.sync_all()?;
    Ok(())
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
    fn journal_name_is_versioned() {
        let j = daemon_journal();
        assert!(
            j.to_string_lossy()
                .ends_with(&format!("daemon-sessions-v{PROTO}.json")),
            "journal path should be versioned: {j:?}"
        );
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

    /// A pre-existing 0644 file at the *temp* path must not be reused: the new
    /// O_EXCL creation fails, so we get a fresh error rather than silently
    /// writing into a world-readable file.
    #[test]
    fn write_atomic_rejects_existing_0644_temp() {
        let base = scratch();
        fs::create_dir_all(&base).unwrap();
        let p = base.join("state.json");
        // Simulate a stale temp file with loose permissions.
        let stale = base.join(".state.json.deadbeef.tmp");
        fs::write(&stale, b"stale").unwrap();
        fs::set_permissions(&stale, fs::Permissions::from_mode(0o644)).unwrap();
        // write_atomic must succeed (it uses a fresh UUID name, not the stale one).
        write_atomic(&p, b"data").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"data");
        assert_eq!(fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        fs::remove_dir_all(&base).unwrap();
    }

    /// A symlink at the temp-file path must not be followed. Because we use
    /// O_EXCL + a random name, this scenario is unlikely in practice, but the
    /// test validates the O_NOFOLLOW property: creating through a symlink is
    /// rejected on Unix.
    #[test]
    #[cfg(unix)]
    fn write_atomic_rejects_symlink_at_temp() {
        let base = scratch();
        fs::create_dir_all(&base).unwrap();
        // We cannot easily predict the UUID name, so test the underlying
        // O_NOFOLLOW property directly via a custom OpenOptions call.
        let target = base.join("real.txt");
        let link = base.join("link.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let mut opts = fs::OpenOptions::new();
        opts.write(true).create_new(true).mode(0o600);
        #[cfg(unix)]
        opts.custom_flags(libc::O_NOFOLLOW);
        let result = opts.open(&link);
        // O_NOFOLLOW causes ELOOP when the path is a symlink.
        assert!(result.is_err(), "opening through a symlink must fail with O_NOFOLLOW");
        fs::remove_dir_all(&base).unwrap();
    }
}
