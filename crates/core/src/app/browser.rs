//! Opening a sign-in page in the member's browser.

/// Whether a window can open here: always on macOS, else with a non-empty
/// `DISPLAY` or `WAYLAND_DISPLAY` (Linux and the BSDs).
pub fn display_present(os: &str, env: impl Fn(&str) -> Option<String>) -> bool {
    let set = |k: &str| env(k).is_some_and(|v| !v.is_empty());
    os == "macos" || set("DISPLAY") || set("WAYLAND_DISPLAY")
}

/// Whether to open the sign-in page in a browser: there is one to show it
/// ([`display_present`]), stdout is a terminal (someone is there to use
/// it), and `CLAUDIO_NO_BROWSER` is unset. The URL is printed either way.
pub fn browser_wanted(os: &str, env: impl Fn(&str) -> Option<String>, tty: bool) -> bool {
    display_present(os, &env) && tty && env("CLAUDIO_NO_BROWSER").is_none()
}

/// Open `url` with `open` (macOS) or `xdg-open`, without waiting for it;
/// whether it could be started. The opener gets a process group of its
/// own: a browser it starts is not the terminal's to stop, so a Ctrl+C in
/// it (`claudio web --foreground`) or its hangup never reaches that browser.
pub fn open_browser(url: &str) -> bool {
    use std::os::unix::process::CommandExt;
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let child = std::process::Command::new(opener)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn();
    match child {
        Ok(mut child) => {
            std::thread::spawn(move || child.wait());
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_browser_opens_only_with_a_display_a_tty_and_no_opt_out() {
        let env = |vars: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                vars.iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        assert!(browser_wanted("linux", env(&[("DISPLAY", ":0")]), true));
        assert!(browser_wanted(
            "linux",
            env(&[("WAYLAND_DISPLAY", "w-0")]),
            true
        ));
        assert!(!browser_wanted("linux", env(&[]), true), "ssh session");
        assert!(!browser_wanted("linux", env(&[("DISPLAY", "")]), true));
        assert!(
            !browser_wanted("linux", env(&[("DISPLAY", ":0")]), false),
            "not a tty"
        );
        assert!(browser_wanted("macos", env(&[]), true));
        let opted_out = env(&[("DISPLAY", ":0"), ("CLAUDIO_NO_BROWSER", "1")]);
        assert!(!browser_wanted("linux", opted_out, true));
    }

    #[test]
    fn a_display_is_a_non_empty_display_variable_or_macos() {
        let env = |vars: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                vars.iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        assert!(display_present("linux", env(&[("DISPLAY", ":0")])));
        assert!(display_present("linux", env(&[("WAYLAND_DISPLAY", "w-0")])));
        assert!(!display_present("linux", env(&[("DISPLAY", "")])));
        assert!(!display_present("linux", env(&[])), "an ssh session");
        assert!(display_present("macos", env(&[])));
        let no_tty_needed = env(&[("DISPLAY", ":0"), ("CLAUDIO_NO_BROWSER", "1")]);
        assert!(
            display_present("linux", no_tty_needed),
            "the opt-out is the caller's"
        );
    }
}
