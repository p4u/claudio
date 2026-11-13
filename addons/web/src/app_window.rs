//! `claudio web --app`: the manager in a chromeless window of a
//! Chromium-family browser (docs/web.md, "App window and launcher"). Bare
//! `--app` opens this machine's web service, signed in with a ticket through
//! a redirect file (`ticket.rs`); `--app=<url>` opens a web on another
//! machine and starts nothing else. Both need a display but no terminal: a
//! desktop launcher runs them, so a failure also goes to `web-app.log` and a
//! desktop notification. The browser is spawned without a shell; its argv
//! holds `--app=` and the redirect file's `file://` URL, or the URL the user
//! gave, never the token, and the ticket only for a sandboxed browser.

use std::fs;
use std::io::{self, IsTerminal, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use claudio_core::app::browser;
use claudio_core::claude::SESSION_MARKERS;
use claudio_core::paths;

use crate::remote::Remote;
use crate::{ticket, WebArgs};

/// What `--app` opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AppTarget {
    /// `--app`: this machine's web service, signed in with a ticket.
    Local,
    /// `--app=<url>`: an existing web elsewhere ([`url_ok`]).
    Url(String),
}

/// `--app=<url>` refused (exit 2).
pub(crate) const URL_BAD: &str =
    "claudio web --app: expected an http(s) URL without credentials or fragment";
/// No Chromium-family browser: the default one was opened.
pub(crate) const FALLBACK: &str = "no Chromium-family browser found: opened the default browser \
(an app window needs Chrome, Chromium, Brave or Edge)";

/// The Chromium-family names looked for on Linux, in this order.
const LINUX_NAMES: [&str; 7] = [
    "google-chrome",
    "google-chrome-stable",
    "chromium",
    "chromium-browser",
    "brave-browser",
    "microsoft-edge",
    "microsoft-edge-stable",
];
/// Where a name is looked for after `$PATH` (then the user's flatpak exports).
const LINUX_DIRS: [&str; 4] = [
    "/usr/bin",
    "/usr/local/bin",
    "/snap/bin",
    "/var/lib/flatpak/exports/bin",
];
/// The macOS bundles looked for, in this order.
const MAC_BUNDLES: [&str; 4] = [
    "Google Chrome",
    "Chromium",
    "Brave Browser",
    "Microsoft Edge",
];
/// `web-app.log` starts over past this size.
const LOG_MAX: u64 = 64 * 1024;
/// One logged line at most, in bytes.
const LOG_LINE: usize = 4096;
/// A desktop notification's deadline.
const NOTIFY_WAIT: Duration = Duration::from_secs(3);

/// `--app`'s value: none is the local service; a URL must pass [`url_ok`].
pub(crate) fn target(value: Option<String>) -> Result<AppTarget, String> {
    match value {
        None => Ok(AppTarget::Local),
        Some(url) if url_ok(&url) => Ok(AppTarget::Url(url)),
        Some(_) => Err(URL_BAD.to_owned()),
    }
}

/// `--app=<url>`: `http` or `https`, a host, an optional port and path. No
/// userinfo, no query and no fragment, so no password, token (`#t=`) or
/// ticket (`#k=`) can reach argv this way.
pub(crate) fn url_ok(url: &str) -> bool {
    let rest = ["http://", "https://"].iter().find_map(|scheme| {
        url.get(..scheme.len())
            .filter(|head| head.eq_ignore_ascii_case(scheme))
            .map(|_| &url[scheme.len()..])
    });
    let Some(rest) = rest else {
        return false;
    };
    let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    authority_ok(authority) && path.bytes().all(path_byte)
}

/// `host[:port]`: a name of `[A-Za-z0-9.-]` not starting with `-` or `.`
/// (an IPv4 address is one), or `[<IPv6>]`; a port of 1 to 65535.
fn authority_ok(authority: &str) -> bool {
    let (host_ok, port) = match authority.strip_prefix('[') {
        Some(v6) => match v6.split_once(']') {
            Some((ip, tail)) => (ip.parse::<std::net::Ipv6Addr>().is_ok(), tail),
            None => return false,
        },
        None => {
            let at = authority.find(':').unwrap_or(authority.len());
            let (host, tail) = authority.split_at(at);
            let name = !host.is_empty()
                && !host.starts_with(['-', '.'])
                && host
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
            (name, tail)
        }
    };
    let port_ok = match port.strip_prefix(':') {
        None => port.is_empty(),
        Some(p) => {
            !p.is_empty()
                && p.bytes().all(|b| b.is_ascii_digit())
                && p.parse::<u16>().is_ok_and(|n| n > 0)
        }
    };
    host_ok && port_ok
}

/// A byte RFC 3986 allows in a path: unreserved, `%`, sub-delims, `:`, `@`
/// and `/`.
fn path_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"-._~%!$&'()*+,;=:@/".contains(&b)
}

/// What the browser search looks at (pure: [`find_browser`]).
pub(crate) struct Search {
    pub os: &'static str,
    /// `[web] browser`, else `$CLAUDIO_BROWSER`: a path or a name.
    pub chosen: Option<String>,
    /// Where a name is looked for: `$PATH`, then the fixed directories.
    pub dirs: Vec<PathBuf>,
    /// Where the macOS bundles are looked for.
    pub bundles: Vec<PathBuf>,
}

impl Search {
    /// This process's search: `configured` (`[web] browser`), the env and
    /// `home`. No login shell is run.
    pub(crate) fn here(
        os: &'static str,
        configured: Option<String>,
        env: impl Fn(&str) -> Option<String>,
        home: &Path,
    ) -> Search {
        let mut dirs: Vec<PathBuf> = env("PATH")
            .map(|p| {
                std::env::split_paths(&p)
                    .filter(|d| d.is_absolute())
                    .collect()
            })
            .unwrap_or_default();
        dirs.extend(LINUX_DIRS.map(PathBuf::from));
        dirs.push(home.join(".local/share/flatpak/exports/bin"));
        let set = |v: Option<String>| v.filter(|v| !v.is_empty());
        Search {
            os,
            chosen: set(configured).or_else(|| set(env("CLAUDIO_BROWSER"))),
            dirs,
            bundles: vec![PathBuf::from("/Applications"), home.join("Applications")],
        }
    }
}

/// The browser an app window opens in. A chosen one always wins (a name
/// found in `dirs`, else as given, for the spawn to report); else on Linux
/// the first of [`LINUX_NAMES`] in `dirs`, on macOS the executable of the
/// first of [`MAC_BUNDLES`] in `bundles`. `exists` is the only look at the
/// disk.
pub(crate) fn find_browser(s: &Search, exists: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    let lookup = |name: &str| s.dirs.iter().map(|d| d.join(name)).find(|p| exists(p));
    if let Some(chosen) = &s.chosen {
        return Some(match chosen.contains('/') {
            true => PathBuf::from(chosen),
            false => lookup(chosen).unwrap_or_else(|| PathBuf::from(chosen)),
        });
    }
    match s.os {
        "macos" => MAC_BUNDLES
            .iter()
            .flat_map(|b| {
                s.bundles
                    .iter()
                    .map(move |root| root.join(format!("{b}.app/Contents/MacOS/{b}")))
            })
            .find(|p| exists(p)),
        _ => LINUX_NAMES.iter().find_map(|name| lookup(name)),
    }
}

/// The names a default browser's desktop entry `id` (`xdg-mime query default
/// text/html`) maps to when it is a Chromium-family one; none for any other.
pub(crate) fn default_names(id: &str) -> &'static [&'static str] {
    const FAMILY: [(&str, &[&str]); 4] = [
        ("google-chrome", &["google-chrome", "google-chrome-stable"]),
        ("chromium", &["chromium", "chromium-browser"]),
        ("brave-browser", &["brave-browser"]),
        (
            "microsoft-edge",
            &["microsoft-edge", "microsoft-edge-stable"],
        ),
    ];
    let id = id.trim();
    FAMILY
        .iter()
        .find(|(prefix, _)| id.starts_with(prefix) && id.ends_with(".desktop"))
        .map(|(_, names)| *names)
        .unwrap_or(&[])
}

/// [`find_browser`], but on Linux with no chosen browser the default one
/// first when [`default_names`] maps it and it is in `dirs`: the window gets
/// the profile the user signs in with. `default_id` is asked only then.
pub(crate) fn find_preferred(
    s: &Search,
    default_id: impl FnOnce() -> Option<String>,
    exists: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    if s.chosen.is_none() && s.os == "linux" {
        let id = default_id().unwrap_or_default();
        let hit = default_names(&id)
            .iter()
            .find_map(|name| s.dirs.iter().map(|d| d.join(name)).find(|p| exists(p)));
        if hit.is_some() {
            return hit;
        }
    }
    find_browser(s, exists)
}

/// Whether the browser found at `path`, resolving to `resolved`, is a snap
/// or a flatpak, which cannot read the runtime dir: under `/snap`, the snap
/// launcher (`/snap/bin/*` link to `/usr/bin/snap`), or in a flatpak
/// `exports/bin`.
pub(crate) fn sandboxed(path: &Path, resolved: &Path) -> bool {
    let boxed =
        |p: &Path| p.starts_with("/snap") || p.to_string_lossy().contains("/flatpak/exports/bin/");
    boxed(path) || boxed(resolved) || resolved.file_name().is_some_and(|n| n == "snap")
}

/// An app window's argv on `target`: `--app=<target>`, and `--class=claudio`
/// on Linux.
pub(crate) fn app_argv(os: &str, target: &str) -> Vec<String> {
    let mut argv = vec![format!("--app={target}")];
    if os == "linux" {
        argv.push("--class=claudio".to_owned());
    }
    argv
}

/// `[web] browser` in config.toml (written by hand, never by claudio).
fn configured() -> Option<String> {
    let text = fs::read_to_string(paths::config_file()).ok()?;
    let table: toml::Table = text.parse().ok()?;
    table
        .get("web")?
        .get("browser")?
        .as_str()
        .map(str::to_owned)
}

/// Start `browser` with `argv`, detached: no shell, null stdio, a process
/// group of its own (a Ctrl+C in this terminal never reaches it), reaped
/// but never waited for.
fn spawn(browser: &Path, argv: &[String]) -> Result<(), String> {
    let mut cmd = Command::new(browser);
    cmd.args(argv)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0);
    for marker in SESSION_MARKERS {
        cmd.env_remove(marker);
    }
    match cmd.spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || child.wait());
            Ok(())
        }
        Err(e) => Err(format!("cannot start {}: {e}", browser.display())),
    }
}

/// An app window on `url`. With `sign_in`, `url` is the local web's base and
/// the window gets a new ticket in a redirect file (the link itself for a
/// sandboxed browser). Nothing with `CLAUDIO_NO_BROWSER`; an error without
/// a display; the default browser when no Chromium-family one is found
/// (`ticket::tab_target` then says what it can read).
fn window(url: &str, sign_in: bool) -> Result<(), String> {
    let env = |k: &str| std::env::var(k).ok();
    let os = std::env::consts::OS;
    if env("CLAUDIO_NO_BROWSER").is_some() {
        return Ok(());
    }
    if !browser::display_present(os, env) {
        return Err(format!(
            "claudio web --app: no display here; open {url} in a browser on this machine"
        ));
    }
    let search = Search::here(os, configured(), env, &paths::home());
    let found = find_preferred(&search, ticket::html_handler, |p| p.is_file());
    let boxed = found
        .as_deref()
        .is_some_and(|b| sandboxed(b, &fs::canonicalize(b).unwrap_or_else(|_| b.to_owned())));
    let target = match sign_in {
        false => url.to_owned(),
        true => {
            let link =
                ticket::link(url).map_err(|e| format!("cannot make a sign-in ticket: {e}"))?;
            let page = match (found.is_some(), boxed) {
                (true, true) => {
                    eprintln!("claudio web: {}", ticket::SANDBOXED);
                    Ok(link)
                }
                (true, false) => ticket::redirect(&link),
                (false, _) => ticket::tab_target(&link),
            };
            page.map_err(|e| format!("cannot write the sign-in page: {e}"))?
        }
    };
    match found {
        Some(b) => spawn(&b, &app_argv(os, &target)),
        None if browser::open_browser(&target) => {
            eprintln!("claudio web: {FALLBACK}");
            Ok(())
        }
        None => Err("no Chromium-family browser found, and no default browser starts".to_owned()),
    }
}

/// `claudio web --app=<url>`: the window and nothing else: no daemon, no
/// token, no ticket, no `[web]`.
pub(crate) fn open_url(url: &str) -> ExitCode {
    match window(url, false) {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            let line = line(&why);
            eprintln!("{line}");
            tell_desktop(&line);
            ExitCode::FAILURE
        }
    }
}

/// After a start printed its lines (`running`: what serves; `asked`: this
/// command line): `--app`'s window, signed in (with no local URL, a
/// specific non-loopback `--remote` address, the public one and no
/// ticket); else `claudio web`'s tab (`ticket::open_tab`).
pub(crate) fn open(running: &WebArgs, asked: &WebArgs) -> Result<(), String> {
    if asked.app.is_none() {
        ticket::open_tab(running, asked.no_open);
        return Ok(());
    }
    match (ticket::base_url(running), running.remote) {
        (Some(base), _) => window(&base, true),
        (None, Some(addr)) => window(
            &crate::remote::public_url(addr, &claudio_core::sys::hostname()),
            false,
        ),
        (None, None) => Ok(()),
    }
}

/// Whether [`open`] would open a browser on a ticket, for the check of an
/// older service (`cli.rs`).
pub(crate) fn opens(asked: &WebArgs) -> bool {
    let env = |k: &str| std::env::var(k).ok();
    match asked.app {
        Some(_) => env("CLAUDIO_NO_BROWSER").is_none(),
        None => {
            let tty = io::stdout().is_terminal();
            !asked.no_open && browser::browser_wanted(std::env::consts::OS, env, tty)
        }
    }
}

/// A start's lines (`crate::announce`); but with `--app` and stdout not a
/// terminal (a desktop launcher), none that holds the token: the base URL
/// alone on stdout, and the `--remote` banner without its token link.
pub(crate) fn announce(running: &WebArgs, asked: &WebArgs, url: &str, remote: Option<&Remote>) {
    if asked.app.is_none() || io::stdout().is_terminal() {
        return crate::announce(url, remote);
    }
    let line = match remote {
        Some(r) => {
            token_free(&r.banner.stderr).for_each(|l| eprintln!("{l}"));
            r.banner.stdout.clone()
        }
        None => format!("claudio web: http://{}/", running.listen),
    };
    let mut out = io::stdout();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// `lines` but those with a token link (`#t=`).
fn token_free(lines: &[String]) -> impl Iterator<Item = &String> {
    lines.iter().filter(|l| !l.contains("#t="))
}

/// A failure's line: `why` when it is one already (`claudio web --app: …`),
/// else `claudio web: <why>`.
pub(crate) fn line(why: &str) -> String {
    match why.starts_with("claudio web") {
        true => why.to_owned(),
        false => format!("claudio web: {why}"),
    }
}

/// An `--app` failure's `line`, already on stderr, also where a desktop
/// user sees it: appended to `<runtime>/web-app.log` and, when stderr is
/// not a terminal, a desktop notification. It holds no token or ticket.
pub(crate) fn tell_desktop(line: &str) {
    append_log(&paths::runtime_dir(), line);
    if !io::stderr().is_terminal() {
        notify(std::env::consts::OS, line);
    }
}

/// Append `line` (one line, at most 4 KiB) to `<runtime>/web-app.log`,
/// 0600, never through a symlink; a log that would pass 64 KiB starts over.
/// Best effort: the line is on stderr already.
fn append_log(runtime: &Path, line: &str) {
    if paths::ensure_private_dir(runtime).is_err() {
        return;
    }
    let mut end = line.len().min(LOG_LINE);
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    let entry = format!("{} {}\n", paths::unix_now(), line[..end].replace('\n', " "));
    let path = runtime.join("web-app.log");
    let full = fs::symlink_metadata(&path).is_ok_and(|m| m.len() + entry.len() as u64 > LOG_MAX);
    if full {
        let _ = paths::write_atomic(&path, entry.as_bytes());
        return;
    }
    let file = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path);
    if let Ok(mut f) = file {
        let _ = f.write_all(entry.as_bytes());
    }
}

/// The notifier and its argv: `message` is one argument, never through a
/// shell (on macOS the item of an `on run argv` script).
fn notifier(os: &str, message: &str) -> (&'static str, Vec<String>) {
    let words = |w: &[&str]| w.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    match os {
        "macos" => (
            "osascript",
            words(&[
                "-e",
                "on run argv",
                "-e",
                "display notification (item 1 of argv) with title \"claudio\"",
                "-e",
                "end run",
                message,
            ]),
        ),
        _ => ("notify-send", words(&["-a", "claudio", "claudio", message])),
    }
}

/// `message` as a desktop notification: stdin null, output dropped, killed
/// after 3 s; nothing when the notifier is not on `PATH`.
fn notify(os: &str, message: &str) {
    let (program, argv) = notifier(os, message);
    let mut cmd = Command::new(program);
    cmd.args(&argv)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for marker in SESSION_MARKERS {
        cmd.env_remove(marker);
    }
    let Ok(mut child) = cmd.spawn() else {
        return;
    };
    let deadline = Instant::now() + NOTIFY_WAIT;
    while Instant::now() < deadline {
        if !matches!(child.try_wait(), Ok(None)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn search(os: &'static str, chosen: Option<&str>, dirs: &[&str]) -> Search {
        Search {
            os,
            chosen: chosen.map(str::to_owned),
            dirs: dirs.iter().map(PathBuf::from).collect(),
            bundles: vec!["/Applications".into(), "/home/u/Applications".into()],
        }
    }

    fn on_disk(files: &'static [&'static str]) -> impl Fn(&Path) -> bool {
        move |p: &Path| files.iter().any(|f| p == Path::new(f))
    }

    #[test]
    fn the_chosen_browser_wins_and_config_beats_the_env() {
        let env = |k: &str| match k {
            "CLAUDIO_BROWSER" => Some("/opt/env-browser".to_owned()),
            "PATH" => Some("/fake/bin:relative:/other".to_owned()),
            _ => None,
        };
        let home = Path::new("/home/u");
        let s = Search::here("linux", Some("/opt/configured".into()), env, home);
        assert_eq!(s.chosen.as_deref(), Some("/opt/configured"));
        let s = Search::here("linux", None, env, home);
        assert_eq!(s.chosen.as_deref(), Some("/opt/env-browser"));
        assert_eq!(
            Search::here("linux", Some(String::new()), env, home)
                .chosen
                .as_deref(),
            Some("/opt/env-browser"),
            "an empty [web] browser is none"
        );
        let dirs: Vec<&str> = s.dirs.iter().map(|d| d.to_str().unwrap()).collect();
        assert_eq!(
            dirs,
            [
                "/fake/bin",
                "/other",
                "/usr/bin",
                "/usr/local/bin",
                "/snap/bin",
                "/var/lib/flatpak/exports/bin",
                "/home/u/.local/share/flatpak/exports/bin"
            ]
        );
        assert_eq!(
            find_browser(&s, |_| false),
            Some(PathBuf::from("/opt/env-browser"))
        );
        let named = search("linux", Some("my-chrome"), &["/a", "/b"]);
        let found = find_browser(&named, on_disk(&["/b/my-chrome", "/b/google-chrome"]));
        assert_eq!(found, Some(PathBuf::from("/b/my-chrome")));
        assert_eq!(
            find_browser(&named, |_| false),
            Some(PathBuf::from("my-chrome")),
            "not found: as given, for the spawn to report"
        );
    }

    #[test]
    fn linux_looks_for_the_names_in_order_over_the_dirs() {
        let s = search("linux", None, &["/a", "/b"]);
        let found = find_browser(&s, on_disk(&["/a/chromium", "/b/google-chrome"]));
        assert_eq!(
            found,
            Some(PathBuf::from("/b/google-chrome")),
            "names first"
        );
        let found = find_browser(&s, on_disk(&["/b/brave-browser", "/a/brave-browser"]));
        assert_eq!(found, Some(PathBuf::from("/a/brave-browser")), "then dirs");
        let edge = find_browser(&s, on_disk(&["/b/microsoft-edge-stable"]));
        assert_eq!(edge, Some(PathBuf::from("/b/microsoft-edge-stable")));
        assert_eq!(find_browser(&s, |_| false), None);
        let empty = search("linux", None, &[]);
        assert_eq!(find_browser(&empty, |_| true), None, "no search roots");
        assert!(FALLBACK.starts_with("no Chromium-family browser found: opened the default"));
        assert_eq!(
            FALLBACK,
            "no Chromium-family browser found: opened the default browser (an app window \
             needs Chrome, Chromium, Brave or Edge)"
        );
    }

    #[test]
    fn macos_looks_for_the_bundles_in_order() {
        let s = search("macos", None, &["/usr/bin"]);
        let found = find_browser(
            &s,
            on_disk(&[
                "/home/u/Applications/Chromium.app/Contents/MacOS/Chromium",
                "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
                "/usr/bin/google-chrome",
            ]),
        );
        assert_eq!(
            found,
            Some(PathBuf::from(
                "/home/u/Applications/Chromium.app/Contents/MacOS/Chromium"
            ))
        );
        const CHROME: &str = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
        let found = find_browser(
            &s,
            on_disk(&[
                CHROME,
                "/home/u/Applications/Chromium.app/Contents/MacOS/Chromium",
            ]),
        );
        assert_eq!(found, Some(PathBuf::from(CHROME)));
        assert_eq!(find_browser(&s, on_disk(&["/usr/bin/google-chrome"])), None);
    }

    #[test]
    fn a_chromium_family_default_browser_is_looked_for_first() {
        assert_eq!(default_names("brave-browser.desktop\n"), ["brave-browser"]);
        let chrome = ["google-chrome", "google-chrome-stable"];
        assert_eq!(default_names("google-chrome.desktop"), chrome);
        assert_eq!(default_names("google-chrome-beta.desktop"), chrome);
        let chromium = ["chromium", "chromium-browser"];
        assert_eq!(default_names("chromium-browser.desktop"), chromium);
        assert_eq!(default_names("chromium_chromium.desktop"), chromium);
        let edge = ["microsoft-edge", "microsoft-edge-stable"];
        assert_eq!(default_names("microsoft-edge.desktop"), edge);
        for other in [
            "firefox.desktop",
            "firefox_firefox.desktop",
            "brave-browser",
            "",
        ] {
            assert!(default_names(other).is_empty(), "{other}");
        }
        let disk = on_disk(&["/a/google-chrome", "/b/brave-browser"]);
        let s = search("linux", None, &["/a", "/b"]);
        let id = |v: &str| {
            let v = v.to_owned();
            move || Some(v)
        };
        let fixed = Some(PathBuf::from("/a/google-chrome"));
        assert_eq!(
            find_preferred(&s, id("brave-browser.desktop\n"), &disk),
            Some(PathBuf::from("/b/brave-browser")),
            "the default first"
        );
        assert_eq!(find_preferred(&s, id("firefox.desktop"), &disk), fixed);
        assert_eq!(
            find_preferred(&s, id("microsoft-edge.desktop"), &disk),
            fixed
        );
        assert_eq!(find_preferred(&s, || None, &disk), fixed, "no answer");
        let chosen = search("linux", Some("/opt/mine"), &["/a", "/b"]);
        let not_asked = || -> Option<String> { panic!("asked") };
        assert_eq!(
            find_preferred(&chosen, not_asked, &disk),
            Some(PathBuf::from("/opt/mine"))
        );
        let mac = search("macos", None, &["/a", "/b"]);
        assert_eq!(find_preferred(&mac, not_asked, &disk), None, "macOS");
    }

    #[test]
    fn the_app_argv_holds_the_file_url_and_never_a_secret() {
        let token = "ab".repeat(32);
        let file = ticket::file_url(Path::new("/run/user/1000/claudio/web-open/0f.html"));
        assert_eq!(
            app_argv("linux", &file),
            [
                "--app=file:///run/user/1000/claudio/web-open/0f.html",
                "--class=claudio"
            ]
        );
        assert_eq!(
            app_argv("macos", &file),
            ["--app=file:///run/user/1000/claudio/web-open/0f.html"]
        );
        for arg in app_argv("linux", &file) {
            assert!(!arg.contains("#t=") && !arg.contains("#k=") && !arg.contains(&token));
        }
        let link = format!("http://127.0.0.1:7337/#k={token}");
        assert_eq!(
            app_argv("linux", &link)[0],
            format!("--app={link}"),
            "the sandboxed fallback: the one argv with #k="
        );
    }

    #[test]
    fn snaps_and_flatpaks_are_sandboxed() {
        let yes = |p: &str, r: &str| sandboxed(Path::new(p), Path::new(r));
        assert!(yes("/snap/bin/chromium", "/usr/bin/snap"));
        assert!(yes(
            "/usr/bin/chromium",
            "/snap/chromium/3000/usr/lib/chromium"
        ));
        assert!(yes(
            "/var/lib/flatpak/exports/bin/com.google.Chrome",
            "/var/lib/flatpak/app/com.google.Chrome/current/active/export/bin/com.google.Chrome"
        ));
        assert!(yes(
            "/home/u/.local/share/flatpak/exports/bin/org.chromium.Chromium",
            "/home/u/.local/share/flatpak/exports/bin/org.chromium.Chromium"
        ));
        assert!(!yes(
            "/usr/bin/google-chrome",
            "/opt/google/chrome/google-chrome"
        ));
        assert!(!yes("/usr/bin/snapper", "/usr/bin/snapper"));
    }

    #[test]
    fn app_urls_are_http_with_a_host_and_no_secret() {
        for ok in [
            "https://z6:7337/",
            "http://127.0.0.1:7337",
            "https://[::1]:8443/a/b",
            "HTTPS://Z6.example.org/claudio/",
            "https://example.invalid:7337/",
            "http://h/a;b=c/%41",
        ] {
            assert!(url_ok(ok), "{ok}");
            assert_eq!(
                target(Some(ok.to_owned())),
                Ok(AppTarget::Url(ok.to_owned()))
            );
        }
        for bad in [
            "",
            "https://",
            "https:///x",
            "https://user:pass@z6:7337/",
            "https://user@z6/",
            "https://z6:7337/#t=ab",
            "https://z6:7337/#k=ab",
            "https://z6/#x",
            "https://z6#x",
            "https://z6/?q=1",
            "ftp://z6/",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "z6:7337",
            "https://z6:0/",
            "https://z6:65536/",
            "https://z6:/",
            "https://-z6/",
            "https://[::1/",
            "https://[z6]/",
            "https://z6/a b",
            "https://z6/\"",
            "https://é.example/",
        ] {
            assert!(!url_ok(bad), "{bad}");
            assert_eq!(
                target(Some(bad.to_owned())),
                Err(URL_BAD.to_owned()),
                "{bad}"
            );
        }
        assert_eq!(target(None), Ok(AppTarget::Local));
        assert_eq!(
            URL_BAD,
            "claudio web --app: expected an http(s) URL without credentials or fragment"
        );
    }

    #[test]
    fn the_notifier_takes_the_message_as_one_argument() {
        let msg = "claudio web: it failed; $(rm -rf ~) \"quoted\"";
        assert_eq!(
            notifier("linux", msg),
            (
                "notify-send",
                vec![
                    "-a".into(),
                    "claudio".into(),
                    "claudio".into(),
                    msg.to_owned()
                ]
            )
        );
        let (program, argv) = notifier("macos", msg);
        assert_eq!(program, "osascript");
        assert_eq!(argv.last().map(String::as_str), Some(msg));
        assert_eq!(argv[1], "on run argv");
        assert!(argv[3].contains("(item 1 of argv)"));
    }

    #[test]
    fn failures_are_lines_and_the_log_is_capped() {
        assert_eq!(line("cannot start x"), "claudio web: cannot start x");
        assert_eq!(line(URL_BAD), URL_BAD);
        let rt = std::env::temp_dir().join(format!("claudio-appwin-{}", uuid::Uuid::new_v4()));
        append_log(&rt, "claudio web: first\nsecond");
        let log = rt.join("web-app.log");
        let text = fs::read_to_string(&log).unwrap();
        assert!(text.ends_with(" claudio web: first second\n"), "{text}");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&log).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let long = "é".repeat(3000);
        for _ in 0..40 {
            append_log(&rt, &long);
        }
        let size = fs::metadata(&log).unwrap().len();
        assert!(size <= LOG_MAX, "{size}");
        assert!(fs::read_to_string(&log)
            .unwrap()
            .lines()
            .all(|l| l.len() < LOG_LINE + 32));
        fs::remove_dir_all(&rt).unwrap();
    }

    #[test]
    fn the_remote_banner_loses_its_token_line_only() {
        let lines = [
            "claudio web: TLS certificate SHA-256 AB".to_owned(),
            "claudio web: on this machine: https://127.0.0.1:7337/#t=abc".to_owned(),
            "claudio web: other machines log in with the password".to_owned(),
        ];
        let kept: Vec<&String> = token_free(&lines).collect();
        assert_eq!(kept, [&lines[0], &lines[2]]);
    }
}
