//! Fakes for what `claudio web` starts on the desktop: a browser, the
//! openers (`xdg-open`, `open`) and the notifiers (`notify-send`,
//! `osascript`). Each records its argv; none opens anything. Put
//! [`FakeBin::dir`] alone on `PATH`, so no real one is reached, and the
//! browser in `CLAUDIO_BROWSER`.

use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use std::{fs, thread};

use sha2::{Digest, Sha256};

use crate::common::ManagerHarness;

/// The fake programs, all in one directory.
pub struct FakeBin {
    dir: PathBuf,
}

/// The programs a `FakeBin` holds.
const NAMES: [&str; 5] = [
    "fake-browser",
    "xdg-open",
    "open",
    "notify-send",
    "osascript",
];

impl FakeBin {
    /// The fakes under `root/fakebin`. Each invocation of `<name>` appends
    /// one line to `<name>.log`: its arguments, each followed by `\x1f`
    /// (shell builtins only: the `PATH` reaches nothing else).
    pub fn new(root: &Path) -> FakeBin {
        let dir = root.join("fakebin");
        fs::create_dir_all(&dir).unwrap();
        for name in NAMES {
            let log = dir.join(format!("{name}.log"));
            let script = format!(
                "#!/bin/sh\nprintf '%s\\037' \"$@\" >> '{}'\nprintf '\\n' >> '{}'\n",
                log.display(),
                log.display()
            );
            let path = dir.join(name);
            fs::write(&path, script).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        FakeBin { dir }
    }

    /// The directory, to be the whole `PATH`.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The fake browser, for `CLAUDIO_BROWSER`.
    pub fn browser(&self) -> PathBuf {
        self.dir.join("fake-browser")
    }

    /// Every recorded invocation of `name`, its arguments in order.
    pub fn calls(&self, name: &str) -> Vec<Vec<String>> {
        let text = fs::read_to_string(self.dir.join(format!("{name}.log"))).unwrap_or_default();
        text.lines()
            .map(|l| {
                l.split('\x1f')
                    .filter(|a| !a.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .collect()
    }

    /// Wait until `name` was run `n` times (it is started detached).
    pub fn wait_calls(&self, name: &str, n: usize, within: Duration) -> Vec<Vec<String>> {
        let deadline = Instant::now() + within;
        loop {
            let calls = self.calls(name);
            if calls.len() >= n {
                return calls;
            }
            assert!(
                Instant::now() < deadline,
                "{name} ran {} times",
                calls.len()
            );
            thread::sleep(Duration::from_millis(50));
        }
    }
}

/// The env of a `claudio web` on a desktop: a display, the fakes alone on
/// `PATH`, the fake browser chosen, `CLAUDIO_NO_BROWSER` unset.
pub fn desktop_env(cmd: &mut std::process::Command, fake: &FakeBin) {
    cmd.env_remove("CLAUDIO_NO_BROWSER")
        .env_remove("WAYLAND_DISPLAY")
        .env("DISPLAY", ":99")
        .env("PATH", fake.dir())
        .env("CLAUDIO_BROWSER", fake.browser());
}

/// [`desktop_env`] for a command run in a PTY (stdout a terminal).
pub fn desktop_env_pty(cmd: &mut portable_pty::CommandBuilder, fake: &FakeBin) {
    cmd.env_remove("CLAUDIO_NO_BROWSER");
    cmd.env_remove("WAYLAND_DISPLAY");
    cmd.env("DISPLAY", ":99");
    cmd.env("PATH", fake.dir());
    cmd.env("CLAUDIO_BROWSER", fake.browser());
}

/// `h`'s `<runtime>/claudio`.
pub fn runtime(h: &ManagerHarness) -> PathBuf {
    h.runtime_dir.join("claudio")
}

/// Where the file of `ticket` lives.
pub fn ticket_file(h: &ManagerHarness, ticket: &str) -> PathBuf {
    let name: String = Sha256::digest(ticket.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    runtime(h).join("web-tickets").join(name)
}

/// A sign-in ticket minted as `claudio` mints one: 64 lowercase hex, its
/// empty 0600 file named by its SHA-256 in `<runtime>/web-tickets` (0700).
pub fn mint_ticket(h: &ManagerHarness) -> String {
    let ticket = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let file = ticket_file(h, &ticket);
    let dir = file.parent().unwrap();
    for d in [runtime(h).as_path(), dir] {
        fs::create_dir_all(d).unwrap();
        fs::set_permissions(d, fs::Permissions::from_mode(0o700)).unwrap();
    }
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&file)
        .unwrap();
    ticket
}

/// The ticket in a redirect file's text (`…#k=<64 hex>"…`).
pub fn ticket_in(html: &str) -> String {
    let at = html.find("#k=").expect("a #k= link") + 3;
    html[at..at + 64].to_owned()
}

/// The redirect file a `file://` URL names (the harness paths need no
/// percent-decoding), checked to be in `h`'s `<runtime>/claudio/web-open`.
pub fn redirect_of(h: &ManagerHarness, url: &str) -> String {
    let path = url.strip_prefix("file://").expect("a file:// URL");
    assert!(
        Path::new(path).starts_with(runtime(h).join("web-open")),
        "{path}"
    );
    let mode = fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "{path}");
    fs::read_to_string(path).unwrap()
}
