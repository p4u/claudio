//! The macOS launcher: `~/Applications/claudio.app`, a bundle whose
//! executable is a shell script running `claudio web --app` (or
//! `--app=<url>`). It only launches: the window is the browser's, under its
//! Dock icon.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use super::{app_arg, found, write, Found};

/// The `Info.plist` line that marks a bundle as claudio's.
const MARKER: &str = "<key>ClaudioLauncher</key>";
const BUNDLE: &str = "Applications/claudio.app";

/// The bundle's `Info.plist`.
fn info_plist() -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n<dict>\n\
         \t<key>CFBundleName</key>\n\t<string>claudio</string>\n\
         \t<key>CFBundleIdentifier</key>\n\t<string>io.vocdoni.claudio</string>\n\
         \t<key>CFBundleExecutable</key>\n\t<string>claudio</string>\n\
         \t<key>CFBundleIconFile</key>\n\t<string>claudio</string>\n\
         \t<key>CFBundlePackageType</key>\n\t<string>APPL</string>\n\
         \t{MARKER}\n\t<true/>\n\
         </dict>\n</plist>\n"
    )
}

/// `s` in single quotes for `sh`, each `'` written `'\''`.
fn sh_quoted(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The bundle's executable: `exec '<exe>' web --app` (or `'--app=<url>'`).
fn script(exe: &str, url: Option<&str>) -> String {
    let app = match url {
        None => app_arg(None),
        Some(_) => sh_quoted(&app_arg(url)),
    };
    format!("#!/bin/sh\nexec {} web {app}\n", sh_quoted(exe))
}

/// An `.icns` of one `ic09` entry (a 512 px PNG): `icns`, the file's
/// length, `ic09`, the entry's length (with its 8-byte header), the PNG;
/// lengths 32-bit big-endian.
fn icns(png: &[u8]) -> Vec<u8> {
    let len = |n: usize| u32::try_from(n).unwrap_or(u32::MAX).to_be_bytes();
    let mut out = Vec::with_capacity(png.len() + 16);
    out.extend_from_slice(b"icns");
    out.extend_from_slice(&len(png.len() + 16));
    out.extend_from_slice(b"ic09");
    out.extend_from_slice(&len(png.len() + 8));
    out.extend_from_slice(png);
    out
}

/// The bundle under `home`: claudio's own is replaced whole; any other
/// (or a symlink) is left alone. The script is made 0755.
pub(super) fn install(
    home: &Path,
    exe: &str,
    url: Option<&str>,
    png512: &[u8],
) -> Result<Vec<String>, String> {
    let bundle = home.join(BUNDLE);
    let plist = bundle.join("Contents/Info.plist");
    match found(&bundle, &plist, MARKER)? {
        Found::Foreign(why) => return Err(format!("{why}: not replaced")),
        Found::Ours => fs::remove_dir_all(&bundle)
            .map_err(|e| format!("cannot replace {}: {e}", bundle.display()))?,
        Found::Nothing => {}
    }
    let run = bundle.join("Contents/MacOS/claudio");
    let wrote = vec![
        write(&plist, info_plist().as_bytes())?,
        write(&run, script(exe, url).as_bytes())?,
        write(
            &bundle.join("Contents/Resources/claudio.icns"),
            &icns(png512),
        )?,
    ];
    fs::set_permissions(&run, fs::Permissions::from_mode(0o755))
        .map_err(|e| format!("cannot make {} executable: {e}", run.display()))?;
    Ok(wrote)
}

/// claudio's bundle, whole; nothing there is fine; anything else is
/// refused.
pub(super) fn remove(home: &Path) -> Result<Vec<String>, String> {
    let bundle = home.join(BUNDLE);
    match found(&bundle, &bundle.join("Contents/Info.plist"), MARKER)? {
        Found::Nothing => Ok(vec![format!("no launcher at {}", bundle.display())]),
        Found::Foreign(why) => Err(format!("{why}: not removed")),
        Found::Ours => {
            fs::remove_dir_all(&bundle)
                .map_err(|e| format!("cannot remove {}: {e}", bundle.display()))?;
            Ok(vec![format!("removed {}", bundle.display())])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn png() -> Vec<u8> {
        crate::assets::embedded("/icons/claudio-512.png").unwrap()
    }

    fn scratch() -> PathBuf {
        std::env::temp_dir().join(format!("claudio-launcher-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn the_script_quotes_the_exe_for_sh() {
        assert_eq!(
            script("/opt/claudio", None),
            "#!/bin/sh\nexec '/opt/claudio' web --app\n"
        );
        assert_eq!(
            script("/Users/o'brien/my apps/claudio", None),
            "#!/bin/sh\nexec '/Users/o'\\''brien/my apps/claudio' web --app\n"
        );
        assert_eq!(
            script("/opt/claudio", Some("https://z6:7337/")),
            "#!/bin/sh\nexec '/opt/claudio' web '--app=https://z6:7337/'\n"
        );
        assert_eq!(
            script("/opt/$x`y`\"z", None),
            "#!/bin/sh\nexec '/opt/$x`y`\"z' web --app\n"
        );
    }

    #[test]
    fn install_writes_the_bundle_and_remove_takes_it_away() {
        let home = scratch();
        let png = png();
        assert_eq!(install(&home, "/opt/claudio", None, &png).unwrap().len(), 3);
        let bundle = home.join(BUNDLE);
        let plist = fs::read_to_string(bundle.join("Contents/Info.plist")).unwrap();
        for (key, value) in [
            ("CFBundleName", "<string>claudio</string>"),
            ("CFBundleIdentifier", "<string>io.vocdoni.claudio</string>"),
            ("CFBundleExecutable", "<string>claudio</string>"),
            ("CFBundleIconFile", "<string>claudio</string>"),
            ("CFBundlePackageType", "<string>APPL</string>"),
            ("ClaudioLauncher", "<true/>"),
        ] {
            let pair = format!("\t<key>{key}</key>\n\t{value}\n");
            assert!(plist.contains(&pair), "{key}: {plist}");
        }
        let run = bundle.join("Contents/MacOS/claudio");
        assert_eq!(
            fs::metadata(&run).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert_eq!(
            fs::read_to_string(&run).unwrap(),
            script("/opt/claudio", None)
        );
        let icns = fs::read(bundle.join("Contents/Resources/claudio.icns")).unwrap();
        assert_eq!(&icns[..4], b"icns");
        assert_eq!(
            u32::from_be_bytes(icns[4..8].try_into().unwrap()) as usize,
            icns.len()
        );
        assert_eq!(&icns[8..12], b"ic09");
        assert_eq!(
            u32::from_be_bytes(icns[12..16].try_into().unwrap()) as usize,
            png.len() + 8
        );
        assert_eq!(&icns[16..], png.as_slice(), "one entry: the 512 px PNG");
        install(&home, "/opt/claudio", Some("https://z6:7337/"), &png).unwrap();
        assert!(fs::read_to_string(&run)
            .unwrap()
            .ends_with("web '--app=https://z6:7337/'\n"));
        assert_eq!(
            remove(&home).unwrap(),
            [format!("removed {}", bundle.display())]
        );
        assert!(!bundle.exists());
        assert!(remove(&home).unwrap()[0].starts_with("no launcher at "));
        fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn a_foreign_bundle_or_a_symlink_is_never_replaced_or_removed() {
        let home = scratch();
        let bundle = home.join(BUNDLE);
        fs::create_dir_all(bundle.join("Contents")).unwrap();
        fs::write(bundle.join("Contents/Info.plist"), "<plist></plist>\n").unwrap();
        assert!(remove(&home)
            .unwrap_err()
            .contains("is not claudio's launcher: not removed"));
        assert!(install(&home, "/opt/claudio", None, &png())
            .unwrap_err()
            .contains("not replaced"));
        assert!(bundle.join("Contents/Info.plist").exists());
        fs::remove_dir_all(&bundle).unwrap();
        let real = home.join("real.app");
        install(&home, "/opt/claudio", None, &png()).unwrap();
        fs::rename(&bundle, &real).unwrap();
        std::os::unix::fs::symlink(&real, &bundle).unwrap();
        assert!(remove(&home).unwrap_err().contains("is a symlink"));
        assert!(install(&home, "/opt/claudio", None, &png()).is_err());
        assert!(real.join("Contents/Info.plist").exists(), "never followed");
        fs::remove_dir_all(&home).unwrap();
    }
}
