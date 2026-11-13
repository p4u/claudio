//! Sign-in tickets (docs/web-protocol.md §1, "Sign-in tickets"). The
//! `claudio` that opens a browser on this machine mints a single-use ticket
//! and hands the browser `<base URL>#k=<ticket>` in a 0600 redirect file, so
//! argv holds only `file://<path>`; the server redeems the ticket once. A
//! ticket is an empty 0600 file in `<runtime>/web-tickets/`, named by the
//! SHA-256 of the ticket: the CLI and the service the daemon runs both
//! resolve `paths::runtime_dir()`, as the daemon socket in it relies on, so
//! nothing secret travels between them and a listing shows no usable ticket.
//! Blocking I/O throughout.

use std::fs;
use std::io::{self, IsTerminal, Read};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

use claudio_core::paths;
use sha2::{Digest, Sha256};

use crate::WebArgs;

/// A ticket's life from its mint.
pub(crate) const LIFE: Duration = Duration::from_secs(30);
/// How far ahead of the clock a ticket's mtime may be.
const SKEW: Duration = Duration::from_secs(2);
/// Live tickets at most; a mint removes the oldest beyond.
pub(crate) const MAX_LIVE: usize = 16;
/// A redirect file's life.
pub(crate) const OPEN_LIFE: Duration = Duration::from_secs(60);
/// Redirect files at most.
pub(crate) const MAX_OPEN: usize = 16;
/// Entries a prune looks at: both directories are ours and small.
const SCAN: usize = 1024;
/// A sandboxed browser was given the sign-in link in argv.
pub(crate) const SANDBOXED: &str = "this browser is sandboxed: the sign-in link is visible to \
other users of this machine for up to 30 s";
/// How long the question about the default browser may take.
const ASK_WAIT: Duration = Duration::from_secs(3);

/// `<runtime>/web-tickets`.
pub(crate) fn tickets_dir(runtime: &Path) -> PathBuf {
    runtime.join("web-tickets")
}

/// `<runtime>/web-open`.
pub(crate) fn open_dir(runtime: &Path) -> PathBuf {
    runtime.join("web-open")
}

/// A ticket file's name: the SHA-256 of the ticket, lowercase hex.
pub(crate) fn file_name(ticket: &str) -> String {
    Sha256::digest(ticket.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `runtime` and `dir` in it, private as `ensure_private_dir` makes them.
fn private(runtime: &Path, dir: &Path) -> io::Result<()> {
    paths::ensure_private_dir(runtime)?;
    paths::ensure_private_dir(dir)
}

/// Whether `mtime` lies in the window that ends `life` before `now`
/// and [`SKEW`] after it.
fn fresh(mtime: SystemTime, now: SystemTime, life: Duration) -> bool {
    let from = now.checked_sub(life).unwrap_or(SystemTime::UNIX_EPOCH);
    from <= mtime && mtime <= now + SKEW
}

/// A new ticket. Its directory is checked first, expired files and the
/// oldest beyond `MAX_LIVE - 1` go, then its empty file is created 0600,
/// exclusively and never through a symlink.
pub(crate) fn mint(runtime: &Path, now: SystemTime) -> io::Result<String> {
    let dir = tickets_dir(runtime);
    private(runtime, &dir)?;
    prune(&dir, now, LIFE, MAX_LIVE - 1);
    let ticket = crate::auth::random_hex(32)?;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(dir.join(file_name(&ticket)))?;
    Ok(ticket)
}

/// Why a ticket was refused (each one a `401`).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Refused {
    /// Not 64 lowercase hex, unknown, spent, expired, or not a file of ours.
    Invalid,
    /// The directory failed its check: the path and why.
    Dir(String),
}

/// Steps 3 to 6 of a redeem (docs/web-protocol.md): the form, the
/// directories (on every redeem), the file (regular, ours, `now - 30 s ..=
/// now + 2 s`; one that fails is removed), and its removal, which only one
/// of two racing attempts wins.
pub(crate) fn redeem(runtime: &Path, ticket: &str, now: SystemTime) -> Result<(), Refused> {
    if !crate::auth::token_ok(ticket) {
        return Err(Refused::Invalid);
    }
    let dir = tickets_dir(runtime);
    private(runtime, &dir).map_err(|e| Refused::Dir(format!("{}: {e}", dir.display())))?;
    let path = dir.join(file_name(ticket));
    let meta = fs::symlink_metadata(&path).map_err(|_| Refused::Invalid)?;
    let live = meta.modified().is_ok_and(|t| fresh(t, now, LIFE));
    if !(meta.file_type().is_file() && meta.uid() == paths::uid() && live) {
        let _ = fs::remove_file(&path);
        return Err(Refused::Invalid);
    }
    fs::remove_file(&path).map_err(|_| Refused::Invalid)
}

/// At a server's start: expired tickets and old redirect files go. A
/// directory that fails its check is left alone.
pub(crate) fn sweep(runtime: &Path, now: SystemTime) {
    for (dir, life, keep) in [
        (tickets_dir(runtime), LIFE, MAX_LIVE),
        (open_dir(runtime), OPEN_LIFE, MAX_OPEN),
    ] {
        if dir.exists() && private(runtime, &dir).is_ok() {
            prune(&dir, now, life, keep);
        }
    }
}

/// Remove from `dir` every entry that is not a regular file or is outside
/// its `life`, then the oldest until at most `keep` stay. Best effort: a
/// file that stays is refused at its redeem anyway. A directory entry stays
/// (`remove_file` refuses it).
fn prune(dir: &Path, now: SystemTime, life: Duration, keep: usize) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut kept: Vec<(SystemTime, PathBuf)> = Vec::new();
    for entry in entries.flatten().take(SCAN) {
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        if meta.file_type().is_file() && fresh(mtime, now, life) {
            kept.push((mtime, path));
        } else {
            let _ = fs::remove_file(&path);
        }
    }
    kept.sort();
    let excess = kept.len().saturating_sub(keep);
    for (_, path) in kept.into_iter().take(excess) {
        let _ = fs::remove_file(path);
    }
}

/// The redirect page that sends a browser to `url`, if `url` holds only
/// what a sign-in URL holds (a scheme, an IP literal, a port, `/`, `#k=`
/// and hex): nothing in it then needs escaping in HTML.
pub(crate) fn redirect_html(url: &str) -> Option<String> {
    let safe = !url.is_empty()
        && url
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b":/.[]#=".contains(&b));
    safe.then(|| {
        format!(
            "<!doctype html><meta charset=\"utf-8\"><meta http-equiv=\"refresh\" \
             content=\"0;url={url}\"><title>claudio</title><a href=\"{url}\">Open claudio</a>\n"
        )
    })
}

/// Write `<runtime>/web-open/<32 hex>.html` (0600, atomically) sending the
/// browser to `url`; its path. Old files go first (60 s, 16 at most).
pub(crate) fn write_redirect(runtime: &Path, url: &str, now: SystemTime) -> io::Result<PathBuf> {
    let html = redirect_html(url).ok_or_else(|| io::Error::other("not a sign-in URL"))?;
    let dir = open_dir(runtime);
    private(runtime, &dir)?;
    prune(&dir, now, OPEN_LIFE, MAX_OPEN - 1);
    let path = dir.join(format!("{}.html", crate::auth::random_hex(16)?));
    paths::write_atomic(&path, html.as_bytes())?;
    Ok(path)
}

/// `file://` and `path`, every byte but an unreserved one and `/`
/// percent-encoded.
pub(crate) fn file_url(path: &Path) -> String {
    let mut url = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                url.push(char::from(b))
            }
            _ => url.push_str(&format!("%{b:02X}")),
        }
    }
    url
}

/// The URL this machine's browser reaches `running` at, without a
/// fragment: `http://<listen>/`, or with `--remote` the loopback one
/// (`None` for a specific non-loopback address: no local URL, no ticket).
pub(crate) fn base_url(running: &WebArgs) -> Option<String> {
    match running.remote {
        None => Some(format!("http://{}/", running.listen)),
        Some(addr) => crate::remote::base_url(addr),
    }
}

/// A new ticket's sign-in link: `<base>#k=<ticket>`.
pub(crate) fn link(base: &str) -> io::Result<String> {
    let ticket = mint(&paths::runtime_dir(), SystemTime::now())?;
    Ok(format!("{base}#k={ticket}"))
}

/// The `file://` URL of a new redirect file for `link`.
pub(crate) fn redirect(link: &str) -> io::Result<String> {
    let path = write_redirect(&paths::runtime_dir(), link, SystemTime::now())?;
    Ok(file_url(&path))
}

/// Whether the desktop entry `id` is a snap's or a flatpak's: such a browser
/// cannot read the runtime dir. `data_dirs` are the XDG data directories in
/// their order; the first that has `applications/<id>` is the entry in use,
/// and the snap and flatpak export directories are looked at last, for an
/// env that does not list them. An `id` that is not a plain `*.desktop`
/// name, or is found nowhere, is not sandboxed.
pub(crate) fn entry_sandboxed(
    id: &str,
    data_dirs: &[PathBuf],
    home: &Path,
    exists: impl Fn(&Path) -> bool,
) -> bool {
    let id = id.trim();
    if id.len() > 255 || !id.ends_with(".desktop") || id.contains('/') || id.starts_with('.') {
        return false;
    }
    let boxed = [
        PathBuf::from("/var/lib/snapd/desktop"),
        PathBuf::from("/var/lib/flatpak/exports/share"),
        home.join(".local/share/flatpak/exports/share"),
    ];
    data_dirs
        .iter()
        .chain(&boxed)
        .find(|dir| exists(&dir.join("applications").join(id)))
        .is_some_and(|dir| boxed.contains(dir))
}

/// The XDG data directories, the user's first: `$XDG_DATA_HOME` (else
/// `~/.local/share`), then `$XDG_DATA_DIRS` (else `/usr/local/share` and
/// `/usr/share`); absolute ones only.
fn data_dirs(env: impl Fn(&str) -> Option<String>, home: &Path) -> Vec<PathBuf> {
    let set = |k: &str| env(k).filter(|v| !v.is_empty());
    let user = set("XDG_DATA_HOME").map_or_else(|| home.join(".local/share"), PathBuf::from);
    let system = set("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".to_owned());
    std::iter::once(user)
        .chain(std::env::split_paths(&system))
        .filter(|d| d.is_absolute())
        .collect()
}

/// The desktop entry `xdg-open` opens a local `.html` file with: the one
/// line of `xdg-mime query default text/html`. No shell, stdin null, a 3 s
/// deadline, 512 bytes read; `None` when it cannot be asked or says nothing.
pub(crate) fn html_handler() -> Option<String> {
    let mut cmd = Command::new("xdg-mime");
    cmd.args(["query", "default", "text/html"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for marker in claudio_core::claude::SESSION_MARKERS {
        cmd.env_remove(marker);
    }
    let mut child = cmd.spawn().ok()?;
    let deadline = Instant::now() + ASK_WAIT;
    let done = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    let mut text = String::new();
    child
        .stdout
        .take()?
        .take(512)
        .read_to_string(&mut text)
        .ok()?;
    (done && !text.trim().is_empty()).then_some(text)
}

/// What the default browser is opened on for `link`: a new redirect file's
/// `file://` URL; but when the default browser is a snap or a flatpak (not
/// on macOS: `open` has no such browser), the link itself, said with
/// [`SANDBOXED`]. A default browser that cannot be found out gets the file.
pub(crate) fn tab_target(link: &str) -> io::Result<String> {
    let home = paths::home();
    let boxed = !cfg!(target_os = "macos")
        && html_handler().is_some_and(|id| {
            let dirs = data_dirs(|k| std::env::var(k).ok(), &home);
            entry_sandboxed(&id, &dirs, &home, |p| p.is_file())
        });
    if !boxed {
        return redirect(link);
    }
    eprintln!("claudio web: {SANDBOXED}");
    Ok(link.to_owned())
}

/// `claudio web`'s browser tab (no `--app`): when one is wanted (not
/// `--no-open`, `browser_wanted`), the default browser on a redirect file
/// that signs it in with a ticket; for a `--remote` web with no local URL,
/// the public one. The printed URL lines are unchanged; this only opens.
pub(crate) fn open_tab(running: &WebArgs, no_open: bool) {
    let wanted = claudio_core::app::browser::browser_wanted(
        std::env::consts::OS,
        |k| std::env::var(k).ok(),
        io::stdout().is_terminal(),
    );
    if no_open || !wanted {
        return;
    }
    let target = match base_url(running) {
        Some(base) => match link(&base).and_then(|l| tab_target(&l)) {
            Ok(target) => target,
            Err(e) => {
                eprintln!("claudio web: cannot make a sign-in link for the browser: {e}");
                return;
            }
        },
        None => match running.remote {
            Some(addr) => crate::remote::public_url(addr, &claudio_core::sys::hostname()),
            None => return,
        },
    };
    claudio_core::app::browser::open_browser(&target);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("claudio-ticket-{}", uuid::Uuid::new_v4()));
        paths::ensure_private_dir(&dir).unwrap();
        dir
    }

    fn mode(p: &Path) -> u32 {
        fs::symlink_metadata(p).unwrap().permissions().mode() & 0o777
    }

    fn age(path: &Path, by: Duration, ahead: bool) {
        let t = match ahead {
            true => SystemTime::now() + by,
            false => SystemTime::now() - by,
        };
        fs::File::open(path).unwrap().set_modified(t).unwrap();
    }

    fn files(dir: &Path) -> usize {
        fs::read_dir(dir).map(|d| d.count()).unwrap_or(0)
    }

    #[test]
    fn a_ticket_redeems_once_and_its_files_are_private() {
        let rt = scratch();
        let now = SystemTime::now();
        let t = mint(&rt, now).unwrap();
        assert!(crate::auth::token_ok(&t), "{t}");
        let file = tickets_dir(&rt).join(file_name(&t));
        assert_eq!(fs::metadata(&file).unwrap().len(), 0, "empty");
        assert_eq!(mode(&file), 0o600);
        assert_eq!(mode(&tickets_dir(&rt)), 0o700);
        assert_eq!(redeem(&rt, &t, now), Ok(()));
        assert!(!file.exists(), "spent");
        assert_eq!(redeem(&rt, &t, now), Err(Refused::Invalid), "a replay");
        let unknown = "0123456789abcdef".repeat(4);
        assert_eq!(redeem(&rt, &unknown, now), Err(Refused::Invalid));
        for bad in [
            "",
            "AB".repeat(32).as_str(),
            &t[..63],
            format!("{t}0").as_str(),
        ] {
            assert_eq!(redeem(&rt, bad, now), Err(Refused::Invalid), "{bad:?}");
        }
        fs::remove_dir_all(&rt).unwrap();
    }

    #[test]
    fn an_expired_or_future_ticket_fails_and_its_file_goes() {
        let rt = scratch();
        let old = mint(&rt, SystemTime::now()).unwrap();
        let file = tickets_dir(&rt).join(file_name(&old));
        age(&file, Duration::from_secs(31), false);
        assert_eq!(redeem(&rt, &old, SystemTime::now()), Err(Refused::Invalid));
        assert!(!file.exists(), "an expired file is removed");
        let ahead = mint(&rt, SystemTime::now()).unwrap();
        let file = tickets_dir(&rt).join(file_name(&ahead));
        age(&file, Duration::from_secs(10), true);
        assert_eq!(
            redeem(&rt, &ahead, SystemTime::now()),
            Err(Refused::Invalid)
        );
        assert!(!file.exists(), "a future file is removed");
        let edge = mint(&rt, SystemTime::now()).unwrap();
        age(
            &tickets_dir(&rt).join(file_name(&edge)),
            Duration::from_secs(25),
            false,
        );
        assert_eq!(redeem(&rt, &edge, SystemTime::now()), Ok(()), "25 s old");
        fs::remove_dir_all(&rt).unwrap();
    }

    #[test]
    fn a_ticket_that_is_a_symlink_or_a_directory_fails() {
        let rt = scratch();
        let dir = tickets_dir(&rt);
        paths::ensure_private_dir(&dir).unwrap();
        let target = rt.join("elsewhere");
        fs::write(&target, b"").unwrap();
        let link = "1".repeat(64);
        std::os::unix::fs::symlink(&target, dir.join(file_name(&link))).unwrap();
        assert_eq!(redeem(&rt, &link, SystemTime::now()), Err(Refused::Invalid));
        assert!(target.exists(), "never followed");
        assert!(
            fs::symlink_metadata(dir.join(file_name(&link))).is_err(),
            "the link is removed"
        );
        let sub = "2".repeat(64);
        fs::create_dir(dir.join(file_name(&sub))).unwrap();
        assert_eq!(redeem(&rt, &sub, SystemTime::now()), Err(Refused::Invalid));
        fs::remove_dir_all(&rt).unwrap();
    }

    #[test]
    fn the_ticket_directory_is_checked_and_repaired_at_each_redeem() {
        let rt = scratch();
        let t = mint(&rt, SystemTime::now()).unwrap();
        fs::set_permissions(tickets_dir(&rt), fs::Permissions::from_mode(0o750)).unwrap();
        assert_eq!(redeem(&rt, &t, SystemTime::now()), Ok(()));
        assert_eq!(mode(&tickets_dir(&rt)), 0o700, "repaired");
        let t = mint(&rt, SystemTime::now()).unwrap();
        let real = rt.join("real");
        fs::rename(tickets_dir(&rt), &real).unwrap();
        std::os::unix::fs::symlink(&real, tickets_dir(&rt)).unwrap();
        let refused = redeem(&rt, &t, SystemTime::now());
        assert!(
            matches!(&refused, Err(Refused::Dir(why)) if why.contains("web-tickets")),
            "{refused:?}"
        );
        assert!(real.join(file_name(&t)).exists(), "nothing done through it");
        assert!(mint(&rt, SystemTime::now()).is_err(), "no mint through it");
        fs::remove_dir_all(&rt).unwrap();
    }

    #[test]
    fn at_most_16_tickets_live_and_the_oldest_go_first() {
        let rt = scratch();
        let first = mint(&rt, SystemTime::now()).unwrap();
        age(
            &tickets_dir(&rt).join(file_name(&first)),
            Duration::from_secs(5),
            false,
        );
        let rest: Vec<String> = (0..20)
            .map(|_| mint(&rt, SystemTime::now()).unwrap())
            .collect();
        assert_eq!(files(&tickets_dir(&rt)), MAX_LIVE);
        assert_eq!(
            redeem(&rt, &first, SystemTime::now()),
            Err(Refused::Invalid),
            "the oldest went first"
        );
        let newest = rest.last().unwrap();
        assert_eq!(redeem(&rt, newest, SystemTime::now()), Ok(()));
        fs::remove_dir_all(&rt).unwrap();
    }

    #[test]
    fn a_start_sweeps_expired_tickets_and_old_redirects() {
        let rt = scratch();
        let old = mint(&rt, SystemTime::now()).unwrap();
        let live = mint(&rt, SystemTime::now()).unwrap();
        age(
            &tickets_dir(&rt).join(file_name(&old)),
            Duration::from_secs(40),
            false,
        );
        let page = write_redirect(&rt, "http://127.0.0.1:1/#k=ab", SystemTime::now()).unwrap();
        let stale = write_redirect(&rt, "http://127.0.0.1:1/#k=cd", SystemTime::now()).unwrap();
        age(&stale, Duration::from_secs(61), false);
        sweep(&rt, SystemTime::now());
        assert!(!tickets_dir(&rt).join(file_name(&old)).exists());
        assert!(tickets_dir(&rt).join(file_name(&live)).exists());
        assert!(page.exists() && !stale.exists());
        let empty = scratch();
        sweep(&empty, SystemTime::now());
        assert!(!tickets_dir(&empty).exists(), "a sweep makes nothing");
        for dir in [rt, empty] {
            fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn the_redirect_file_holds_the_link_and_nothing_to_escape() {
        let rt = scratch();
        let url = format!("http://127.0.0.1:7999/#k={}", "a1".repeat(32));
        let path = write_redirect(&rt, &url, SystemTime::now()).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            format!(
                "<!doctype html><meta charset=\"utf-8\"><meta http-equiv=\"refresh\" \
                 content=\"0;url={url}\"><title>claudio</title><a href=\"{url}\">Open claudio</a>\n"
            )
        );
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(&open_dir(&rt)), 0o700);
        let name = path.file_name().unwrap().to_str().unwrap();
        assert!(name.len() == 37 && name.ends_with(".html"), "{name}");
        assert!(redirect_html("https://[::1]:8443/#k=00").is_some());
        for bad in [
            "",
            "http://x/\"><script>",
            "http://h/#k=a b",
            "http://h/?q&r",
            "http://h/'",
        ] {
            assert_eq!(redirect_html(bad), None, "{bad}");
        }
        for _ in 0..20 {
            write_redirect(&rt, &url, SystemTime::now()).unwrap();
        }
        assert_eq!(files(&open_dir(&rt)), MAX_OPEN);
        fs::remove_dir_all(&rt).unwrap();
    }

    #[test]
    fn file_urls_percent_encode_what_is_not_plain() {
        assert_eq!(
            file_url(Path::new("/run/user/1000/claudio/web-open/ab.html")),
            "file:///run/user/1000/claudio/web-open/ab.html"
        );
        assert_eq!(
            file_url(Path::new("/tmp/a b/#?%é")),
            "file:///tmp/a%20b/%23%3F%25%C3%A9"
        );
    }

    #[test]
    fn a_default_browser_from_a_snap_or_a_flatpak_is_sandboxed() {
        let home = Path::new("/home/u");
        let env = |k: &str| match k {
            "XDG_DATA_DIRS" => Some("/usr/share/:relative:/var/lib/snapd/desktop".to_owned()),
            _ => None,
        };
        let dirs = data_dirs(env, home);
        let want = [
            "/home/u/.local/share",
            "/usr/share",
            "/var/lib/snapd/desktop",
        ];
        assert_eq!(dirs, want.map(PathBuf::from));
        let none = data_dirs(|_| None, home);
        assert_eq!(
            none[1..],
            ["/usr/local/share", "/usr/share"].map(PathBuf::from)
        );
        let on_disk = |files: &'static [&'static str]| {
            move |p: &Path| files.iter().any(|f| p == Path::new(f))
        };
        let snap = on_disk(&["/var/lib/snapd/desktop/applications/firefox_firefox.desktop"]);
        assert!(entry_sandboxed(
            "firefox_firefox.desktop\n",
            &dirs,
            home,
            snap
        ));
        assert!(
            entry_sandboxed("firefox_firefox.desktop", &[], home, snap),
            "not in the env"
        );
        let flatpak = on_disk(&[
            "/home/u/.local/share/flatpak/exports/share/applications/org.mozilla.firefox.desktop",
        ]);
        assert!(entry_sandboxed(
            "org.mozilla.firefox.desktop",
            &dirs,
            home,
            flatpak
        ));
        let system = on_disk(&["/var/lib/flatpak/exports/share/applications/c.desktop"]);
        assert!(entry_sandboxed("c.desktop", &dirs, home, system));
        let deb = on_disk(&["/usr/share/applications/google-chrome.desktop"]);
        assert!(!entry_sandboxed("google-chrome.desktop", &dirs, home, deb));
        let both = on_disk(&[
            "/usr/share/applications/com.google.Chrome.desktop",
            "/var/lib/flatpak/exports/share/applications/com.google.Chrome.desktop",
        ]);
        assert!(
            !entry_sandboxed("com.google.Chrome.desktop", &dirs, home, both),
            "the entry in use is the first in the data dirs"
        );
        for odd in ["", "firefox", "../x.desktop", "a/b.desktop", ".desktop"] {
            assert!(!entry_sandboxed(odd, &dirs, home, |_| true), "{odd:?}");
        }
        assert!(!entry_sandboxed("nowhere.desktop", &dirs, home, |_| false));
        assert_eq!(
            SANDBOXED,
            "this browser is sandboxed: the sign-in link is visible to other users of this \
             machine for up to 30 s"
        );
    }

    #[test]
    fn base_urls_carry_no_fragment() {
        let args = |list: &[&str]| {
            crate::parse_args(&list.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap()
        };
        assert_eq!(
            base_url(&args(&["--listen", "127.0.0.1:9000"])).as_deref(),
            Some("http://127.0.0.1:9000/")
        );
        assert_eq!(
            base_url(&args(&["--listen", "[::1]:9000"])).as_deref(),
            Some("http://[::1]:9000/")
        );
        assert_eq!(
            base_url(&args(&["--remote", ":8443"])).as_deref(),
            Some("https://127.0.0.1:8443/")
        );
        assert_eq!(base_url(&args(&["--remote", "192.168.1.2:8443"])), None);
    }
}
