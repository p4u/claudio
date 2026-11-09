//! Opening a sign-in page in the member's browser.

/// Whether to open the sign-in page in a browser: there is one to show it
/// (a display on Linux and the BSDs, always on macOS), stdout is a terminal
/// (someone is there to use it), and `CLAUDIO_NO_BROWSER` is unset. The URL
/// is printed either way.
pub fn browser_wanted(os: &str, env: impl Fn(&str) -> Option<String>, tty: bool) -> bool {
    let set = |k: &str| env(k).is_some_and(|v| !v.is_empty());
    let display = os == "macos" || set("DISPLAY") || set("WAYLAND_DISPLAY");
    display && tty && env("CLAUDIO_NO_BROWSER").is_none()
}

/// Open `url` with `open` (macOS) or `xdg-open`, without waiting for it;
/// whether it could be started.
pub fn open_browser(url: &str) -> bool {
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
}
