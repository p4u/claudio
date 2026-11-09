//! Saved prompts: the files of `~/.config/claudio/snippets/`.
//!
//! File I/O, so it is called only from the effect runner in `tui/mod.rs`,
//! never from `App`. Names on disk are always made by [`slug`], never taken
//! from claude's text as a path; files are written 0600 in a 0700 dir.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use crate::claude::latex::sanitize;
use crate::paths;

/// The largest snippet file read.
pub const MAX_BYTES: u64 = 64 * 1024;
/// The most snippets listed.
pub const MAX_FILES: usize = 500;
/// The longest name [`slug`] makes.
pub const MAX_SLUG: usize = 40;

/// A saved prompt: its file's stem and its text, both sanitized.
#[derive(Debug, Clone, PartialEq)]
pub struct Snippet {
    pub name: String,
    pub text: String,
}

/// The snippets directory (`$XDG_CONFIG_HOME` honoured).
pub fn dir() -> PathBuf {
    paths::config_dir().join("snippets")
}

/// The snippets in `dir`: its `*.md` and `*.txt` files of at most
/// [`MAX_BYTES`], sorted by name, at most [`MAX_FILES`]. Hidden, larger,
/// unreadable and non-UTF-8 files are skipped; a missing dir has none.
pub fn load(dir: &Path) -> io::Result<Vec<Snippet>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some((stem, ext)) = name.rsplit_once('.') else {
            continue;
        };
        if !stem.is_empty() && !name.starts_with('.') && matches!(ext, "md" | "txt") {
            files.push((stem.to_owned(), path));
        }
    }
    files.sort();
    let mut snippets = Vec::new();
    for (stem, path) in files {
        if snippets.len() == MAX_FILES {
            break;
        }
        if let Some(text) = read(&path) {
            let name: String = sanitize(&stem)
                .chars()
                .map(|c| if c == '\n' || c == '\t' { ' ' } else { c })
                .collect();
            snippets.push(Snippet {
                name,
                text: sanitize(&text),
            });
        }
    }
    Ok(snippets)
}

/// The text of the snippet file at `path`, if it is a regular file of at
/// most [`MAX_BYTES`] of UTF-8 (read without trusting its size).
fn read(path: &Path) -> Option<String> {
    let meta = fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_BYTES {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// Save `text` as a new snippet in `dir` (created 0700) and return its
/// name: [`slug`] of the text, with `-2`, `-3`… when the name is taken by
/// a `.md` or `.txt` file. The file is `<name>.md`, 0600, written whole
/// (temp file + rename).
pub fn save(dir: &Path, text: &str) -> io::Result<String> {
    paths::ensure_private_dir(dir)?;
    let base = slug(text);
    for n in 1..=1000 {
        let name = if n == 1 {
            base.clone()
        } else {
            format!("{base}-{n}")
        };
        let taken = ["md", "txt"]
            .iter()
            .any(|ext| dir.join(format!("{name}.{ext}")).symlink_metadata().is_ok());
        if !taken {
            paths::write_atomic(&dir.join(format!("{name}.md")), text.as_bytes())?;
            return Ok(name);
        }
    }
    Err(io::Error::other(format!("too many snippets named {base}")))
}

/// Save `text` under the name [`slug`] makes of `name` in `dir` (created
/// 0700), replacing the snippet of that name (its `.md`, or its `.txt` when
/// that is the only file). Returns the slug. 0600, written whole.
pub fn save_named(dir: &Path, name: &str, text: &str) -> io::Result<String> {
    paths::ensure_private_dir(dir)?;
    let name = slug(name);
    let md = dir.join(format!("{name}.md"));
    let txt = dir.join(format!("{name}.txt"));
    let target = if md.symlink_metadata().is_err() && txt.symlink_metadata().is_ok() {
        txt
    } else {
        md
    };
    paths::write_atomic(&target, text.as_bytes())?;
    Ok(name)
}

/// Delete the snippet `name`, as [`load`] names it: the file `<name>.md`.
/// `name` is slug characters only (letters, digits, `-`, `_`), so it cannot
/// leave `dir`, reach a hidden file or a non-`.md` one; anything else, or a
/// name with no regular `.md` file, is an `InvalidInput` / `NotFound` error.
pub fn delete(dir: &Path, name: &str) -> io::Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 255 - 3
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !ok {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a snippet name",
        ));
    }
    let path = dir.join(format!("{name}.md"));
    match path.symlink_metadata() {
        Ok(m) if m.is_file() => fs::remove_file(&path),
        _ => Err(io::Error::new(io::ErrorKind::NotFound, "no such snippet")),
    }
}

/// A file name for `text`: its first words, lowercase ASCII letters and
/// digits joined by `-`, at most [`MAX_SLUG`] characters; `snippet` when
/// nothing is left.
pub fn slug(text: &str) -> String {
    let mut out = String::new();
    let words = text
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty());
    for word in words {
        let word = word.to_ascii_lowercase();
        let sep = usize::from(!out.is_empty());
        if out.len() + sep + word.len() > MAX_SLUG {
            if out.is_empty() {
                out = word[..MAX_SLUG].to_owned();
            }
            break;
        }
        if sep == 1 {
            out.push('-');
        }
        out.push_str(&word);
    }
    if out.is_empty() {
        "snippet".into()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn scratch() -> PathBuf {
        std::env::temp_dir().join(format!("claudio-snippets-{}", uuid::Uuid::new_v4()))
    }

    fn snippet(name: &str, text: &str) -> Snippet {
        Snippet {
            name: name.into(),
            text: text.into(),
        }
    }

    #[test]
    fn slugs_are_the_first_words_in_lowercase_ascii() {
        assert_eq!(slug("Review this PR, please!"), "review-this-pr-please");
        assert_eq!(slug("  --fix   the_tests\n now"), "fix-the-tests-now");
        assert_eq!(
            slug("one two three four five six seven eight nine ten"),
            "one-two-three-four-five-six-seven-eight"
        );
        assert_eq!(slug(&"x".repeat(50)), "x".repeat(MAX_SLUG));
        assert_eq!(slug("Résumé ok"), "r-sum-ok");
        assert_eq!(slug("¿?"), "snippet");
        assert_eq!(slug(""), "snippet");
        assert!(slug("../../etc/passwd")
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-'));
    }

    #[test]
    fn load_reads_md_and_txt_sorted_capped_and_sanitized() {
        let dir = scratch();
        assert_eq!(load(&dir).unwrap(), []);
        fs::create_dir_all(dir.join("sub.md")).unwrap();
        fs::write(dir.join("b.md"), "bee\n").unwrap();
        fs::write(dir.join("a.txt"), "ay\u{1b}[2J\tx").unwrap();
        fs::write(dir.join("c.json"), "{}").unwrap();
        fs::write(dir.join(".hidden.md"), "no").unwrap();
        fs::write(dir.join("big.md"), "x".repeat(MAX_BYTES as usize + 1)).unwrap();
        fs::write(dir.join("max.md"), "y".repeat(MAX_BYTES as usize)).unwrap();
        fs::write(dir.join("bad.md"), [0xff, 0xfe]).unwrap();
        let loaded = load(&dir).unwrap();
        let names: Vec<&str> = loaded.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["a", "b", "max"]);
        assert_eq!(loaded[0], snippet("a", "ay[2J\tx"));
        assert_eq!(loaded[1], snippet("b", "bee\n"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_keeps_the_first_500_by_name() {
        let dir = scratch();
        fs::create_dir_all(&dir).unwrap();
        for i in 0..MAX_FILES + 5 {
            fs::write(dir.join(format!("s{i:04}.md")), "t").unwrap();
        }
        let loaded = load(&dir).unwrap();
        assert_eq!(loaded.len(), MAX_FILES);
        assert_eq!(loaded[0].name, "s0000");
        assert_eq!(loaded[MAX_FILES - 1].name, "s0499");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_names_by_slug_avoids_collisions_and_is_private() {
        let dir = scratch().join("snippets");
        assert_eq!(save(&dir, "Fix the tests").unwrap(), "fix-the-tests");
        assert_eq!(save(&dir, "fix the tests!").unwrap(), "fix-the-tests-2");
        fs::write(dir.join("fix-the-tests-3.txt"), "taken").unwrap();
        assert_eq!(save(&dir, "FIX THE TESTS").unwrap(), "fix-the-tests-4");
        assert_eq!(save(&dir, "¿").unwrap(), "snippet");
        let text = "line one\n\tline two\n";
        let name = save(&dir, text).unwrap();
        let path = dir.join(format!("{name}.md"));
        assert_eq!(fs::read_to_string(&path).unwrap(), text);
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(&dir), 0o700);
        // Saved snippets load back under their names.
        let loaded = load(&dir).unwrap();
        assert!(
            loaded.contains(&snippet("line-one-line-two", text)),
            "{loaded:?}"
        );
        assert!(loaded.contains(&snippet("fix-the-tests-2", "fix the tests!")));
        let _ = fs::remove_dir_all(dir.parent().unwrap());
    }

    #[test]
    fn save_named_uses_the_slug_of_the_name_and_replaces() {
        let dir = scratch().join("snippets");
        assert_eq!(
            save_named(&dir, "Release Notes!", "v1").unwrap(),
            "release-notes"
        );
        assert_eq!(
            save_named(&dir, "release notes", "v2\n").unwrap(),
            "release-notes"
        );
        assert_eq!(
            fs::read_to_string(dir.join("release-notes.md")).unwrap(),
            "v2\n"
        );
        assert_eq!(load(&dir).unwrap().len(), 1);
        // a name that is only a .txt file is replaced in place, not doubled
        fs::write(dir.join("old.txt"), "a").unwrap();
        assert_eq!(save_named(&dir, "old", "b").unwrap(), "old");
        assert!(!dir.join("old.md").exists());
        assert_eq!(fs::read_to_string(dir.join("old.txt")).unwrap(), "b");
        // a path-looking name is only ever a slug
        assert_eq!(save_named(&dir, "../x", "c").unwrap(), "x");
        assert!(dir.join("x.md").is_file());
        assert_eq!(save_named(&dir, "", "d").unwrap(), "snippet");
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir.join("x.md")), 0o600);
        assert_eq!(mode(&dir), 0o700);
        let _ = fs::remove_dir_all(dir.parent().unwrap());
    }

    #[test]
    fn delete_removes_only_an_existing_md_snippet() {
        let dir = scratch();
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("keep.md"), "k").unwrap();
        fs::write(dir.join("gone.md"), "g").unwrap();
        fs::write(dir.join("t.txt"), "t").unwrap();
        fs::write(dir.join(".hidden.md"), "h").unwrap();
        fs::write(dir.parent().unwrap().join("outside.md"), "o").unwrap();
        delete(&dir, "gone").unwrap();
        assert!(!dir.join("gone.md").exists());
        let kind = |n: &str| delete(&dir, n).unwrap_err().kind();
        for bad in ["../outside", "a/b", ".hidden", "..", "", "x\0", "k.md"] {
            assert_eq!(kind(bad), io::ErrorKind::InvalidInput, "{bad:?}");
        }
        assert_eq!(kind("unknown"), io::ErrorKind::NotFound);
        assert_eq!(kind("t"), io::ErrorKind::NotFound); // .txt is not deleted
        assert_eq!(kind("sub"), io::ErrorKind::NotFound); // not a file
        assert!(dir.join("keep.md").is_file() && dir.join(".hidden.md").is_file());
        assert!(dir.parent().unwrap().join("outside.md").is_file());
        let _ = fs::remove_file(dir.parent().unwrap().join("outside.md"));
        let _ = fs::remove_dir_all(&dir);
    }
}
