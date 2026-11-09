//! An exclusive advisory lock beside a file: `flock` on the sidecar
//! `.<name>.lock` in the file's directory. Every claudio process that
//! read-modify-writes `config.toml` holds it for the whole edit, so two
//! processes (a TUI, `claudio web`, `claudio proxy …`) never interleave
//! their edits and lose one (B3). The sidecar is stable: it is never
//! replaced, so every process locks the same inode.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long a writer waits for another process's edit before giving up.
pub const LOCK_WAIT: Duration = Duration::from_secs(5);

/// A held lock; released when dropped (closing the descriptor drops the
/// `flock`).
#[derive(Debug)]
pub struct FileLock {
    _file: File,
}

/// The sidecar lock file of `path`: `<dir>/.<file name>.lock`.
fn sidecar(path: &Path) -> io::Result<PathBuf> {
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name to lock"))?;
    let dir = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    Ok(dir.join(format!(".{}.lock", name.to_string_lossy())))
}

impl FileLock {
    /// Take the exclusive lock beside `path` (its sidecar created 0600 if
    /// missing, never through a symlink; the directory created as
    /// [`super::write_atomic`] would), waiting at most `wait` for another
    /// holder. Each call opens its own descriptor, so two threads of one
    /// process exclude each other too.
    pub fn beside(path: &Path, wait: Duration) -> io::Result<FileLock> {
        let lock = sidecar(path)?;
        if let Some(dir) = lock.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&lock)?;
        let deadline = Instant::now() + wait;
        loop {
            // SAFETY: flock on a descriptor this function owns; no memory
            // is shared with the call.
            let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            if rc == 0 {
                return Ok(FileLock { _file: file });
            }
            let err = io::Error::last_os_error();
            match err.raw_os_error() {
                Some(libc::EINTR) => continue,
                Some(libc::EWOULDBLOCK) => {}
                _ => return Err(err),
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    format!(
                        "{} is being edited by another process",
                        path.file_name().unwrap_or_default().to_string_lossy()
                    ),
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("claudio-lock-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_sidecar_is_private_and_excludes_a_second_holder_until_dropped() {
        let dir = scratch();
        let file = dir.join("config.toml");
        let held = FileLock::beside(&file, LOCK_WAIT).unwrap();
        let lock = dir.join(".config.toml.lock");
        let mode = std::fs::metadata(&lock).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let err = FileLock::beside(&file, Duration::from_millis(50)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::WouldBlock);
        assert_eq!(
            err.to_string(),
            "config.toml is being edited by another process"
        );
        drop(held);
        FileLock::beside(&file, Duration::from_millis(50)).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_symlinked_sidecar_is_refused() {
        let dir = scratch();
        std::os::unix::fs::symlink(dir.join("elsewhere"), dir.join(".config.toml.lock")).unwrap();
        assert!(FileLock::beside(&dir.join("config.toml"), LOCK_WAIT).is_err());
        assert!(!dir.join("elsewhere").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
