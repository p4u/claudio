//! The Linux launcher: `<data>/applications/claudio.desktop` and its icons
//! under `<data>/icons/hicolor` (`<data>`: `$XDG_DATA_HOME`, else
//! `~/.local/share`).

use std::fs;
use std::path::Path;

use super::{app_arg, found, write, Found, Icons};

/// The line that marks a `.desktop` file as claudio's.
const MARKER: &str = "X-Claudio-Launcher=1";
const ENTRY: &str = "applications/claudio.desktop";
const ICONS: [&str; 3] = [
    "icons/hicolor/scalable/apps/claudio.svg",
    "icons/hicolor/192x192/apps/claudio.png",
    "icons/hicolor/512x512/apps/claudio.png",
];
/// The characters the Desktop Entry spec reserves in an `Exec` argument.
const RESERVED: &str = " \t\n\"'\\><~|&;$*?#()`";

/// `s` as a double-quoted `Exec` argument, both layers of escaping: `\`,
/// `"`, `` ` `` and `$` take a backslash, which the value's string escaping
/// then doubles (`\` is `\\\\`, `"` is `\\"`); `%` is `%%`.
fn quoted(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '\\' => out.push_str(r"\\\\"),
            '"' => out.push_str(r#"\\""#),
            '`' => out.push_str(r"\\`"),
            '$' => out.push_str(r"\\$"),
            '%' => out.push_str("%%"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// An `Exec` argument: [`quoted`] when it holds a reserved character, else
/// as it is with `%` as `%%`.
fn arg(s: &str) -> String {
    match s.contains(|c| RESERVED.contains(c)) {
        true => quoted(s),
        false => s.replace('%', "%%"),
    }
}

/// The `.desktop` file running `"<exe>" web --app` (or `--app=<url>`).
fn entry(exe: &str, url: Option<&str>) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=claudio\nExec={} web {}\nIcon=claudio\n\
         Categories=Development;\nTerminal=false\n{MARKER}\n",
        quoted(exe),
        arg(&app_arg(url))
    )
}

/// The icons, then the entry; a `.desktop` that is not claudio's (or a
/// symlink) is left alone.
pub(super) fn install(
    data: &Path,
    exe: &str,
    url: Option<&str>,
    icons: &Icons,
) -> Result<Vec<String>, String> {
    let path = data.join(ENTRY);
    if let Found::Foreign(why) = found(&path, &path, MARKER)? {
        return Err(format!("{why}: not replaced"));
    }
    let mut wrote = Vec::new();
    for (rel, bytes) in ICONS.iter().zip([&icons.svg, &icons.png192, &icons.png512]) {
        wrote.push(write(&data.join(rel), bytes)?);
    }
    wrote.push(write(&path, entry(exe, url).as_bytes())?);
    Ok(wrote)
}

/// claudio's entry and then its icons (regular files only); nothing there
/// is fine; anything else is refused.
pub(super) fn remove(data: &Path) -> Result<Vec<String>, String> {
    let path = data.join(ENTRY);
    match found(&path, &path, MARKER)? {
        Found::Nothing => return Ok(vec![format!("no launcher at {}", path.display())]),
        Found::Foreign(why) => return Err(format!("{why}: not removed")),
        Found::Ours => {}
    }
    let mut removed = Vec::new();
    for p in std::iter::once(path).chain(ICONS.iter().map(|rel| data.join(rel))) {
        if fs::symlink_metadata(&p).is_ok_and(|m| m.file_type().is_file()) {
            fs::remove_file(&p).map_err(|e| format!("cannot remove {}: {e}", p.display()))?;
            removed.push(format!("removed {}", p.display()));
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    fn icons() -> Icons {
        Icons::embedded().unwrap()
    }

    fn scratch() -> PathBuf {
        std::env::temp_dir().join(format!("claudio-launcher-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn exec_quotes_the_exe_with_both_layers_of_escaping() {
        for (exe, exec) in [
            ("/opt/claudio", r#"Exec="/opt/claudio" web --app"#),
            (
                "/opt/my apps/claudio",
                r#"Exec="/opt/my apps/claudio" web --app"#,
            ),
            (
                r#"/opt/a"b/claudio"#,
                r#"Exec="/opt/a\\"b/claudio" web --app"#,
            ),
            ("/opt/a`b/claudio", r#"Exec="/opt/a\\`b/claudio" web --app"#),
            (
                "/opt/$HOME/claudio",
                r#"Exec="/opt/\\$HOME/claudio" web --app"#,
            ),
            (
                r"/opt/a\b/claudio",
                r#"Exec="/opt/a\\\\b/claudio" web --app"#,
            ),
            (
                "/opt/100%/claudio",
                r#"Exec="/opt/100%%/claudio" web --app"#,
            ),
        ] {
            let text = entry(exe, None);
            assert!(text.contains(&format!("\n{exec}\n")), "{exe}: {text}");
        }
        assert_eq!(
            entry("/opt/claudio", Some("https://example.invalid:7337/")),
            "[Desktop Entry]\nType=Application\nName=claudio\n\
             Exec=\"/opt/claudio\" web --app=https://example.invalid:7337/\nIcon=claudio\n\
             Categories=Development;\nTerminal=false\nX-Claudio-Launcher=1\n"
        );
        let odd = entry("/opt/claudio", Some("https://h/a;b%41"));
        assert!(
            odd.contains(r#"Exec="/opt/claudio" web "--app=https://h/a;b%%41""#),
            "{odd}"
        );
        assert_eq!(arg("--app=https://h/%41"), "--app=https://h/%%41");
    }

    #[test]
    fn install_writes_the_entry_and_icons_and_remove_takes_them_away() {
        let data = scratch();
        let icons = icons();
        let wrote = install(&data, "/opt/claudio", None, &icons).unwrap();
        assert_eq!(wrote.len(), 4, "{wrote:?}");
        let path = data.join(ENTRY);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("[Desktop Entry]\n") && text.contains("\nX-Claudio-Launcher=1\n"));
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(&data.join("applications")), 0o700);
        assert_eq!(fs::read(data.join(ICONS[0])).unwrap(), icons.svg);
        assert_eq!(fs::read(data.join(ICONS[1])).unwrap(), icons.png192);
        assert_eq!(fs::read(data.join(ICONS[2])).unwrap(), icons.png512);
        let url = Some("https://z6:7337/");
        install(&data, "/opt/claudio", url, &icons).unwrap();
        assert!(fs::read_to_string(&path)
            .unwrap()
            .contains(" web --app=https://z6:7337/\n"));
        assert_eq!(remove(&data).unwrap().len(), 4);
        assert!(!path.exists() && !data.join(ICONS[2]).exists());
        let again = remove(&data).unwrap();
        assert!(again[0].starts_with("no launcher at "), "{again:?}");
        fs::remove_dir_all(&data).unwrap();
    }

    #[test]
    fn a_foreign_entry_or_a_symlink_is_never_replaced_or_removed() {
        let data = scratch();
        let path = data.join(ENTRY);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "[Desktop Entry]\nName=mine\n").unwrap();
        assert!(remove(&data)
            .unwrap_err()
            .contains("is not claudio's launcher: not removed"));
        let refused = install(&data, "/opt/claudio", None, &icons()).unwrap_err();
        assert!(refused.contains("not replaced"), "{refused}");
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "[Desktop Entry]\nName=mine\n"
        );
        fs::remove_file(&path).unwrap();
        let target = data.join("elsewhere.desktop");
        fs::write(&target, format!("{MARKER}\n")).unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(remove(&data).unwrap_err().contains("is a symlink"));
        assert!(install(&data, "/opt/claudio", None, &icons()).is_err());
        assert!(target.exists() && fs::symlink_metadata(&path).is_ok());
        fs::remove_dir_all(&data).unwrap();
    }
}
