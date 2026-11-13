//! `claudio web --install-launcher` and `--remove-launcher` (docs/web.md,
//! "App window and launcher"): a desktop menu entry (Linux, `linux.rs`) or
//! `~/Applications/claudio.app` (macOS, `macos.rs`) that runs `claudio web
//! --app`, or `--app=<url>`. One launcher at a time: installing again
//! replaces claudio's own. Files go through `paths::write_atomic` (0600,
//! missing directories 0700), under the user's home or `XDG_DATA_HOME`; a
//! symlink at a launcher's path is never followed, and a launcher claudio
//! did not write is neither replaced nor removed.

mod linux;
mod macos;

use std::fs;
use std::io::{self, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use claudio_core::paths;

/// `--install-launcher` or `--remove-launcher`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LauncherOp {
    Install,
    Remove,
}

/// The icons a launcher installs: the page's own, from the binary.
struct Icons {
    svg: Vec<u8>,
    png192: Vec<u8>,
    png512: Vec<u8>,
}

impl Icons {
    fn embedded() -> Result<Icons, String> {
        let get =
            |p: &str| crate::assets::embedded(p).ok_or_else(|| format!("this build has no {p}"));
        Ok(Icons {
            svg: get("/icons/claudio.svg")?,
            png192: get("/icons/claudio-192.png")?,
            png512: get("/icons/claudio-512.png")?,
        })
    }
}

/// `claudio web --install-launcher [--app=<url>]` or `--remove-launcher`;
/// each line it wrote or removed is printed.
pub(crate) fn run(op: LauncherOp, url: Option<&str>) -> ExitCode {
    let home = paths::home();
    let mac = cfg!(target_os = "macos");
    let done = match op {
        LauncherOp::Install => install(mac, &home, url),
        LauncherOp::Remove if mac => macos::remove(&home),
        LauncherOp::Remove => linux::remove(&data_home(&home)),
    };
    match done {
        Ok(lines) => {
            lines.iter().for_each(|l| println!("claudio web: {l}"));
            ExitCode::SUCCESS
        }
        Err(why) => {
            eprintln!("claudio web: {why}");
            ExitCode::FAILURE
        }
    }
}

fn install(mac: bool, home: &Path, url: Option<&str>) -> Result<Vec<String>, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot find this claudio: {e}"))?;
    let exe = exe_ok(&exe)?;
    let icons = Icons::embedded()?;
    match mac {
        true => macos::install(home, exe, url, &icons.png512),
        false => linux::install(&data_home(home), exe, url, &icons),
    }
}

/// `$XDG_DATA_HOME` when set and absolute, else `~/.local/share`.
fn data_home(home: &Path) -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".local/share"))
}

/// The exe a launcher runs, as `current_exe` gave it (not canonicalized):
/// absolute, UTF-8, no control character, not a replaced binary (`
/// (deleted)`), and there. Its text.
fn exe_ok(exe: &Path) -> Result<&str, String> {
    let Some(text) = exe.to_str() else {
        return Err(format!(
            "{}: not UTF-8, which a launcher cannot name",
            exe.display()
        ));
    };
    if text.chars().any(char::is_control) {
        return Err(format!("{text:?}: a path with a control character"));
    }
    if !exe.is_absolute() {
        return Err(format!("{text}: not an absolute path"));
    }
    if text.ends_with(" (deleted)") {
        return Err(format!("{text}: this binary was replaced; run the new one"));
    }
    if !exe.is_file() {
        return Err(format!("{text}: no such file"));
    }
    Ok(text)
}

/// The argument after `web`: `--app`, or `--app=<url>`.
fn app_arg(url: Option<&str>) -> String {
    match url {
        Some(url) => format!("--app={url}"),
        None => "--app".to_owned(),
    }
}

/// What is at a launcher's path.
enum Found {
    Nothing,
    /// claudio's: its `file` has the marker line.
    Ours,
    /// Not to be touched, and why.
    Foreign(String),
}

/// What is at `path`, read from `file` (the path itself, or a bundle's
/// `Info.plist`): a symlink is foreign, never followed.
fn found(path: &Path, file: &Path, marker: &str) -> Result<Found, String> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Found::Nothing),
        Err(e) => return Err(format!("{}: {e}", path.display())),
        Ok(m) if m.file_type().is_symlink() => {
            return Ok(Found::Foreign(format!("{} is a symlink", path.display())))
        }
        Ok(_) => {}
    }
    match read_small(file) {
        Some(text) if text.lines().any(|l| l.trim() == marker) => Ok(Found::Ours),
        _ => Ok(Found::Foreign(format!(
            "{} is not claudio's launcher",
            path.display()
        ))),
    }
}

/// `path`'s text, never through a symlink, at most 64 KiB.
fn read_small(path: &Path) -> Option<String> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .ok()?;
    let mut text = String::new();
    file.take(64 * 1024).read_to_string(&mut text).ok()?;
    Some(text)
}

/// `bytes` to `path` (`write_atomic`: 0600, missing directories 0700); the
/// line that says so.
fn write(path: &Path, bytes: &[u8]) -> Result<String, String> {
    paths::write_atomic(path, bytes)
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(format!("wrote {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_exe_must_be_absolute_present_and_plain() {
        let exe = std::env::current_exe().unwrap();
        assert_eq!(exe_ok(&exe), Ok(exe.to_str().unwrap()));
        for bad in [
            "relative/claudio",
            "/no/such/claudio",
            "/usr/bin/claudio (deleted)",
            "/opt/clau\ndio",
            "/opt/clau\tdio",
        ] {
            assert!(exe_ok(Path::new(bad)).is_err(), "{bad:?}");
        }
        use std::os::unix::ffi::OsStrExt;
        let latin1 = Path::new(std::ffi::OsStr::from_bytes(b"/opt/\xe9/claudio"));
        assert!(exe_ok(latin1).unwrap_err().contains("not UTF-8"));
        assert_eq!(app_arg(None), "--app");
        assert_eq!(app_arg(Some("https://z6:7337/")), "--app=https://z6:7337/");
    }
}
