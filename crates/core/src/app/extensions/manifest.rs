//! External extensions: `~/.config/claudio/extensions/<name>/extension.toml`.
//!
//! ```toml
//! name = "keys"                      # the directory's name
//! description = "keyboard keys"
//! command = ["python3", "keys.py"]   # argv, no shell
//! [[lens]]
//! tag = "keys"                       # or: fence = "abc"
//! instruction = "Write keyboard keys as <keys>Ctrl+C</keys>."
//! [[action]]
//! key = "k"
//! title = "insert a timestamp"
//! id = "stamp"
//! input = "label"                    # optional
//! ```
//!
//! [`parse`] checks one manifest, [`admit`] checks them against the
//! built-ins and each other, and [`load`] (file I/O: called by `tui/mod.rs`
//! at start, never by `App`) reads the directory. Whatever is skipped gets
//! one notice.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::{builtins, ActionDecl, Extension, Lens, Source, MANAGE_KEY, READER_KEY};
use crate::proto::{lens_name_ok, LensKind, MAX_INSTRUCTION};
use crate::text::sanitize;

/// The most external extensions loaded.
pub const MAX_EXTERNAL: usize = 16;
/// The largest manifest read.
const MAX_MANIFEST: u64 = 64 * 1024;
const MAX_DESCRIPTION: usize = 120;
const MAX_TITLE: usize = 60;
const MAX_PROMPT: usize = 120;

/// HTML element names (the WHATWG list, with obsolete ones claude may still
/// write): a tag lens with one of these names would take what markdown and
/// claude mean as HTML, unless its manifest sets `html = true`.
pub const HTML_ELEMENTS: &[&str] = &[
    "a",
    "abbr",
    "acronym",
    "address",
    "applet",
    "area",
    "article",
    "aside",
    "audio",
    "b",
    "base",
    "basefont",
    "bdi",
    "bdo",
    "big",
    "blink",
    "blockquote",
    "body",
    "br",
    "button",
    "canvas",
    "caption",
    "center",
    "cite",
    "code",
    "col",
    "colgroup",
    "data",
    "datalist",
    "dd",
    "del",
    "details",
    "dfn",
    "dialog",
    "dir",
    "div",
    "dl",
    "dt",
    "em",
    "embed",
    "fieldset",
    "figcaption",
    "figure",
    "font",
    "footer",
    "form",
    "frame",
    "frameset",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "head",
    "header",
    "hgroup",
    "hr",
    "html",
    "i",
    "iframe",
    "img",
    "input",
    "ins",
    "kbd",
    "label",
    "legend",
    "li",
    "link",
    "main",
    "map",
    "mark",
    "marquee",
    "menu",
    "meta",
    "meter",
    "nav",
    "noembed",
    "noframes",
    "noscript",
    "object",
    "ol",
    "optgroup",
    "option",
    "output",
    "p",
    "param",
    "picture",
    "plaintext",
    "pre",
    "progress",
    "q",
    "rb",
    "rp",
    "rt",
    "rtc",
    "ruby",
    "s",
    "samp",
    "script",
    "search",
    "section",
    "select",
    "slot",
    "small",
    "source",
    "span",
    "strike",
    "strong",
    "style",
    "sub",
    "summary",
    "sup",
    "table",
    "tbody",
    "td",
    "template",
    "textarea",
    "tfoot",
    "th",
    "thead",
    "time",
    "title",
    "tr",
    "track",
    "tt",
    "u",
    "ul",
    "var",
    "video",
    "wbr",
    "xmp",
];

/// How an external extension's program is started.
#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    /// The extension's directory: the program's cwd.
    pub dir: PathBuf,
    /// Its argv; a first element with a `/` is relative to `dir`.
    pub command: Vec<String>,
}

impl Program {
    /// The program to execute: the first element resolved against `dir`
    /// when it contains a `/`, else as given (looked up in `PATH`).
    pub fn program(&self) -> PathBuf {
        let first = self.command.first().map(String::as_str).unwrap_or("");
        if first.contains('/') {
            self.dir.join(first)
        } else {
            PathBuf::from(first)
        }
    }
}

#[derive(Deserialize)]
struct File {
    name: String,
    #[serde(default)]
    description: String,
    command: Vec<String>,
    #[serde(default, rename = "lens")]
    lenses: Vec<LensFile>,
    #[serde(default, rename = "action")]
    actions: Vec<ActionFile>,
}

#[derive(Deserialize)]
struct LensFile {
    tag: Option<String>,
    fence: Option<String>,
    #[serde(default)]
    instruction: String,
    #[serde(default)]
    html: bool,
}

#[derive(Deserialize)]
struct ActionFile {
    key: String,
    title: String,
    id: String,
    input: Option<String>,
}

/// `^[a-z][a-z0-9-]{0,31}$`.
fn extension_name_ok(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= 32
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// `^[a-z][a-z0-9_-]{0,31}$`.
fn action_id_ok(id: &str) -> bool {
    let mut chars = id.chars();
    id.len() <= 32
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// A text field: sanitized, on one line, at most `max` chars.
fn line_field(what: &str, text: &str, max: usize) -> Result<String, String> {
    let clean = sanitize(text).replace(['\n', '\t'], " ");
    if clean.chars().count() > max {
        return Err(format!("{what} is longer than {max} characters"));
    }
    Ok(clean)
}

/// The extension in directory `dir` (named `dir_name`) whose manifest is
/// `text`, or why it is not acceptable.
pub fn parse(dir_name: &str, dir: &Path, text: &str) -> Result<Extension, String> {
    let file: File =
        toml::from_str(text).map_err(|e| format!("invalid manifest: {}", e.message()))?;
    if !extension_name_ok(&file.name) {
        return Err(format!("bad name '{}'", file.name));
    }
    if file.name != dir_name {
        return Err(format!("name '{}' is not its directory's", file.name));
    }
    let description = line_field("description", &file.description, MAX_DESCRIPTION)?;
    if file.command.first().is_none_or(|c| c.is_empty()) {
        return Err("command is empty".to_owned());
    }
    let mut lenses = Vec::new();
    for lens in file.lenses {
        let (name, kind) = match (lens.tag, lens.fence) {
            (Some(tag), None) => (tag, LensKind::Tag),
            (None, Some(fence)) => (fence, LensKind::Fence),
            _ => return Err("a lens needs exactly one of tag and fence".to_owned()),
        };
        if !lens_name_ok(&name) {
            return Err(format!("bad lens name '{name}'"));
        }
        if kind == LensKind::Tag && !lens.html && HTML_ELEMENTS.contains(&name.as_str()) {
            return Err(format!("tag {name} is an HTML element; pick another name"));
        }
        if sanitize(&lens.instruction).len() > MAX_INSTRUCTION {
            return Err(format!(
                "lens {name}: instruction over {MAX_INSTRUCTION} bytes"
            ));
        }
        if lenses.iter().any(|l: &Lens| l.name == name) {
            return Err(format!("lens {name} given twice"));
        }
        lenses.push(Lens {
            name,
            kind,
            instruction: lens.instruction,
        });
    }
    let mut actions = Vec::new();
    for action in file.actions {
        let mut chars = action.key.chars();
        let key = match (chars.next(), chars.next()) {
            (Some(k), None) if k.is_ascii_alphanumeric() => k,
            _ => return Err(format!("bad action key '{}'", action.key)),
        };
        if !action_id_ok(&action.id) {
            return Err(format!("bad action id '{}'", action.id));
        }
        let input = match action.input {
            Some(prompt) => Some(line_field("input", &prompt, MAX_PROMPT)?),
            None => None,
        };
        actions.push(ActionDecl {
            key,
            title: line_field("title", &action.title, MAX_TITLE)?,
            id: action.id,
            input,
        });
    }
    Ok(Extension {
        name: file.name,
        description,
        enabled: true,
        lenses,
        actions,
        source: Source::External(Program {
            dir: dir.to_path_buf(),
            command: file.command,
        }),
    })
}

/// `candidates` (parsed, in directory order) that can join the built-ins:
/// at most [`MAX_EXTERNAL`]; an extension whose name, tag or fence is
/// taken is skipped whole, an action whose key is taken alone. Each skip
/// gets a notice.
pub fn admit(candidates: Vec<Extension>, notices: &mut Vec<String>) -> Vec<Extension> {
    let mut taken = builtins();
    let mut keys: Vec<char> = taken
        .iter()
        .flat_map(|e| e.actions.iter().map(|a| a.key))
        .chain([READER_KEY, MANAGE_KEY])
        .collect();
    let builtin_count = taken.len();
    for mut ext in candidates {
        if taken.len() - builtin_count == MAX_EXTERNAL {
            notices.push(format!(
                "extension {}: skipped (at most {MAX_EXTERNAL} extensions)",
                ext.name
            ));
            continue;
        }
        if taken.iter().any(|e| e.name == ext.name) {
            notices.push(format!("extension {}: the name is taken", ext.name));
            continue;
        }
        let clash = ext.lenses.iter().find(|l| {
            taken
                .iter()
                .flat_map(|e| e.lenses.iter())
                .any(|t| t.name == l.name)
        });
        if let Some(lens) = clash {
            notices.push(format!(
                "extension {}: lens {} is taken",
                ext.name, lens.name
            ));
            continue;
        }
        let name = ext.name.clone();
        ext.actions.retain(|a| {
            if keys.contains(&a.key) {
                notices.push(format!("extension {name}: key {} is taken", a.key));
                return false;
            }
            keys.push(a.key);
            true
        });
        taken.push(ext);
    }
    taken.split_off(builtin_count)
}

/// The external extensions under `dir` (`<name>/extension.toml`, by name),
/// admitted; what is skipped gets a notice. A missing `dir` has none.
pub fn load(dir: &Path, notices: &mut Vec<String>) -> Vec<Extension> {
    use std::io::Read;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    let mut candidates = Vec::new();
    for ext_dir in dirs {
        let name = ext_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let manifest = ext_dir.join("extension.toml");
        let mut text = String::new();
        let read = std::fs::File::open(&manifest)
            .and_then(|f| f.take(MAX_MANIFEST + 1).read_to_string(&mut text));
        match read {
            Ok(n) if n as u64 > MAX_MANIFEST => {
                notices.push(format!("extension {name}: manifest over 64 KiB"));
            }
            Ok(_) => match parse(&name, &ext_dir, &text) {
                Ok(ext) => candidates.push(ext),
                Err(why) => notices.push(format!("extension {name}: {why}")),
            },
            Err(e) => notices.push(format!("extension {name}: {e}")),
        }
    }
    admit(candidates, notices)
}

/// `~/.config/claudio/extensions` (honours `$XDG_CONFIG_HOME`).
pub fn dir() -> PathBuf {
    crate::paths::config_dir().join("extensions")
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEYS: &str = r#"
name = "keys"
description = "keyboard keys"
command = ["./keys.sh", "--fast"]
[[lens]]
tag = "keys"
instruction = "Write keys as <keys>Ctrl+C</keys>."
[[lens]]
fence = "abc"
[[action]]
key = "k"
title = "insert a timestamp"
id = "stamp"
input = "label"
"#;

    fn parsed(text: &str) -> Result<Extension, String> {
        parse("keys", Path::new("/ext/keys"), text)
    }

    #[test]
    fn a_manifest_gives_its_lenses_actions_and_program() {
        let ext = parsed(KEYS).unwrap();
        assert_eq!(ext.name, "keys");
        assert_eq!(ext.description, "keyboard keys");
        assert_eq!(ext.lenses.len(), 2);
        assert_eq!(
            (ext.lenses[0].name.as_str(), ext.lenses[0].kind),
            ("keys", LensKind::Tag)
        );
        assert_eq!(
            (ext.lenses[1].name.as_str(), ext.lenses[1].kind),
            ("abc", LensKind::Fence)
        );
        let action = &ext.actions[0];
        assert_eq!((action.key, action.id.as_str()), ('k', "stamp"));
        assert_eq!(action.input.as_deref(), Some("label"));
        let Source::External(program) = &ext.source else {
            panic!("not external")
        };
        assert_eq!(program.program(), Path::new("/ext/keys/./keys.sh"));
        let bare = Program {
            dir: "/ext/keys".into(),
            command: vec!["python3".into()],
        };
        assert_eq!(bare.program(), Path::new("python3"));
    }

    #[test]
    fn invalid_manifests_say_why() {
        let cases = [
            (
                KEYS.replace("name = \"keys\"", "name = \"other\""),
                "not its directory",
            ),
            (
                KEYS.replace("name = \"keys\"", "name = \"Keys\""),
                "bad name",
            ),
            (
                KEYS.replace("[\"./keys.sh\", \"--fast\"]", "[]"),
                "command is empty",
            ),
            (
                KEYS.replace("fence = \"abc\"", "tag = \"x\"\nfence = \"y\""),
                "exactly one",
            ),
            (
                KEYS.replace("tag = \"keys\"", "tag = \"kbd\""),
                "tag kbd is an HTML element; pick another name",
            ),
            (
                KEYS.replace("tag = \"keys\"", "tag = \"Bad tag\""),
                "bad lens name",
            ),
            (
                KEYS.replace("key = \"k\"", "key = \"kk\""),
                "bad action key",
            ),
            (KEYS.replace("key = \"k\"", "key = \"-\""), "bad action key"),
            (
                KEYS.replace("id = \"stamp\"", "id = \"9\""),
                "bad action id",
            ),
            (
                KEYS.replace("insert a timestamp", &"t".repeat(61)),
                "title is longer",
            ),
            (
                KEYS.replace("Write keys", &"w".repeat(2049)),
                "instruction over",
            ),
            ("name = 3".to_owned(), "invalid manifest"),
        ];
        for (text, why) in cases {
            let err = parsed(&text).unwrap_err();
            assert!(err.contains(why), "{err:?} lacks {why:?}");
        }
        // `html = true` takes the HTML name knowingly.
        let html = KEYS.replace("tag = \"keys\"", "tag = \"kbd\"\nhtml = true");
        assert_eq!(parsed(&html).unwrap().lenses[0].name, "kbd");
    }

    fn ext(name: &str, lens: &str, key: char) -> Extension {
        let text = KEYS
            .replace("name = \"keys\"", &format!("name = \"{name}\""))
            .replace("tag = \"keys\"", &format!("tag = \"{lens}\""))
            .replace("fence = \"abc\"", &format!("fence = \"{lens}-f\""))
            .replace("key = \"k\"", &format!("key = \"{key}\""));
        parse(name, Path::new("/ext"), &text).unwrap()
    }

    #[test]
    fn admission_skips_clashes_with_one_notice_each() {
        let mut notices = Vec::new();
        let admitted = admit(
            vec![
                ext("keys", "keys", 'k'),
                ext("search", "x", 'x'),    // a built-in's name
                ext("other", "keys", 'o'),  // a taken tag
                ext("fourth", "four", 's'), // a taken key: the action goes
                ext("fifth", "five", 'u'),  // the reader's key
                ext("sixth", "six", 'e'),   // the extensions screen's key
            ],
            &mut notices,
        );
        let names: Vec<&str> = admitted.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["keys", "fourth", "fifth", "sixth"]);
        assert!(admitted[1..].iter().all(|e| e.actions.is_empty()));
        assert_eq!(
            notices,
            [
                "extension search: the name is taken",
                "extension other: lens keys is taken",
                "extension fourth: key s is taken",
                "extension fifth: key u is taken",
                "extension sixth: key e is taken",
            ]
        );
        let many: Vec<Extension> = (0..18)
            .map(|i| {
                ext(
                    &format!("e{i}"),
                    &format!("t{i}"),
                    char::from(b'a' + i as u8),
                )
            })
            .collect();
        let mut notices = Vec::new();
        assert_eq!(admit(many, &mut notices).len(), MAX_EXTERNAL);
        assert_eq!(
            notices.len(),
            2 + 2,
            "two over the cap, and keys e and p are taken"
        );
    }

    #[test]
    fn load_reads_each_directory_and_reports_bad_ones() {
        let dir = std::env::temp_dir().join(format!("claudio-ext-{}", uuid::Uuid::new_v4()));
        for (name, text) in [("keys", KEYS.to_owned()), ("bad", "nonsense".to_owned())] {
            std::fs::create_dir_all(dir.join(name)).unwrap();
            std::fs::write(dir.join(name).join("extension.toml"), text).unwrap();
        }
        std::fs::create_dir_all(dir.join("big")).unwrap();
        std::fs::write(dir.join("big/extension.toml"), "#".repeat(70_000)).unwrap();
        std::fs::create_dir_all(dir.join("empty")).unwrap();
        let mut notices = Vec::new();
        let loaded = load(&dir, &mut notices);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "keys");
        assert_eq!(notices.len(), 3, "{notices:?}");
        assert!(notices[0].starts_with("extension bad: invalid manifest"));
        assert_eq!(notices[1], "extension big: manifest over 64 KiB");
        assert!(notices[2].starts_with("extension empty: "));
        assert!(load(&dir.join("missing"), &mut notices).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_example_extension_is_valid() {
        let text = include_str!("../../../../../examples/extensions/keys/extension.toml");
        let ext = parse("keys", Path::new("/ext/keys"), text).unwrap();
        assert_eq!(ext.lenses[0].name, "keys");
        assert_eq!(ext.actions[0].key, 'k');
        assert!(admit(vec![ext], &mut Vec::new()).len() == 1);
    }
}
