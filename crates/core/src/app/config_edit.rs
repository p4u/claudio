//! The one writer of `config.toml`'s tables (B3): a minimal line edit of one
//! top-level table, serialised within the process and across processes
//! ([`lock`]), atomic, and checked by parsing before anything is written.
//! The core's `[extensions]` toggles and the addons' own sections
//! (`AddonCtx::edit_config`) go through it.

use std::io;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use toml::{Table, Value};

use crate::config::SaveError;
use crate::paths;

/// Held for every read-modify-write of `config.toml` in this process (F5).
static EDIT: Mutex<()> = Mutex::new(());

/// The config edit lock of the file at `path`, held across a whole
/// read-modify-write: this process's mutex, then the cross-process
/// [`paths::FileLock`] on the sidecar `.<name>.lock` beside `path` (a TUI,
/// `claudio web` and `claudio proxy …` edit the same file). Released on
/// drop. A poisoned mutex guards no data and is taken anyway.
pub(crate) fn lock(path: &Path) -> Result<EditLock, SaveError> {
    let mutex = EDIT.lock().unwrap_or_else(|e| e.into_inner());
    let file = paths::FileLock::beside(path, paths::LOCK_WAIT)?;
    Ok(EditLock {
        _file: file,
        _mutex: mutex,
    })
}

/// A held config edit lock (see [`lock`]); the file lock is released
/// before the mutex.
pub(crate) struct EditLock {
    _file: paths::FileLock,
    _mutex: MutexGuard<'static, ()>,
}

/// Apply `edit` to top-level table `section` of the config file at `path`
/// (through a symlink, to its target), minimally: a changed scalar or
/// flat-array key of `[section]` → one `key = value` line replaced, added
/// or removed (indent and trailing comment kept); a changed sub-table
/// `[section.name]` (of such keys) → its header and key lines replaced or
/// the table appended; a removed one → its lines removed. Anything else
/// (deeper tables, inline tables, dotted keys, floats, dates) is refused
/// with [`SaveError::UnsupportedSection`]. Written atomically keeping the
/// file's mode (0600 when created); nothing is written unless the result
/// parses and differs from the input exactly in `section`. Returns whether
/// the file changed.
pub fn edit_section(
    path: &Path,
    section: &str,
    edit: impl FnOnce(&mut Table),
) -> Result<bool, SaveError> {
    let _edit = lock(path)?;
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let (text, mode) = read(&path)?;
    let before: Table = text.parse().map_err(|_| SaveError::InvalidToml)?;
    let unsupported = || SaveError::UnsupportedSection(section.to_owned());
    let old = match before.get(section) {
        None => Table::new(),
        Some(Value::Table(t)) => t.clone(),
        Some(_) => return Err(unsupported()),
    };
    let mut new = old.clone();
    edit(&mut new);
    if new == old {
        return Ok(false);
    }
    let edited = edit_text(&text, section, &old, &new).ok_or_else(unsupported)?;
    if !only_section_differs(&before, &edited, section, &new) {
        return Err(unsupported());
    }
    write(&path, &edited, mode)?;
    Ok(true)
}

/// The file's text and mode (`None`: it does not exist yet).
fn read(path: &Path) -> Result<(String, Option<u32>), SaveError> {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::read(path) {
        Ok(bytes) => {
            let text = String::from_utf8(bytes).map_err(|_| SaveError::InvalidToml)?;
            let mode = std::fs::metadata(path)?.permissions().mode() & 0o7777;
            Ok((text, Some(mode)))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok((String::new(), None)),
        Err(e) => Err(e.into()),
    }
}

/// Atomically replace the file, keeping `mode` (a new file stays 0600).
fn write(path: &Path, text: &str, mode: Option<u32>) -> Result<(), SaveError> {
    use std::os::unix::fs::PermissionsExt;
    paths::write_atomic(path, text.as_bytes())?;
    if let Some(mode) = mode.filter(|m| *m != 0o600) {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}

/// Whether `edited` parses, has `section` as `new` (an absent table counts
/// as an empty one) and every other top-level key as `before` has it.
fn only_section_differs(before: &Table, edited: &str, section: &str, new: &Table) -> bool {
    let Ok(mut after) = edited.parse::<Table>() else {
        return false;
    };
    let got = match after.remove(section) {
        None => Table::new(),
        Some(Value::Table(t)) => t,
        Some(_) => return false,
    };
    let mut before = before.clone();
    before.remove(section);
    got == *new && after == before
}

/// `text` with table `section` changed from `old` (what `text` parses to)
/// to `new`, one key at a time; `None` when a change is not one this editor
/// makes, or the text does not hold a key where the edit expects it.
pub fn edit_text(text: &str, section: &str, old: &Table, new: &Table) -> Option<String> {
    let mut out = text.to_owned();
    let keys = old
        .keys()
        .chain(new.keys().filter(|k| !old.contains_key(*k)));
    for key in keys {
        out = match (old.get(key), new.get(key)) {
            (a, b) if a == b => continue,
            (Some(Value::Table(o)), Some(Value::Table(t))) => {
                edit_table_keys(&out, section, key, o, t)
                    .or_else(|| set_table(&out, section, key, Some(t)))?
            }
            (Some(Value::Table(_)), None) => set_table(&out, section, key, None)?,
            (None, Some(Value::Table(t))) => set_table(&out, section, key, Some(t))?,
            (Some(Value::Table(_)), Some(_)) | (Some(_), Some(Value::Table(_))) => return None,
            (_, Some(value)) => set_key(&out, section, key, Some(&line_value(value)?))?,
            (_, None) => set_key(&out, section, key, None)?,
        };
    }
    Some(out)
}

/// A value this editor writes on one line: a string, an integer, a boolean,
/// or a flat array of those.
fn line_value(value: &Value) -> Option<String> {
    let flat = |v: &Value| matches!(v, Value::String(_) | Value::Integer(_) | Value::Boolean(_));
    match value {
        v if flat(v) => Some(v.to_string()),
        Value::Array(items) if items.iter().all(flat) => Some(value.to_string()),
        _ => None,
    }
}

/// `key` as a TOML key: bare when it can be, else a quoted string.
fn render_key(key: &str) -> String {
    let bare = !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if bare {
        key.to_owned()
    } else {
        Value::String(key.to_owned()).to_string()
    }
}

/// The file's line ending: `\r\n` when it has one, else `\n`.
fn eol_of(text: &str) -> &'static str {
    if text.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

/// One line of the file as the scanner sees it.
struct Line<'a> {
    content: &'a str,
    ending: &'a str,
    /// Inside a multi-line string that an earlier line opened.
    continued: bool,
    /// The table this line belongs to (`None` before any header).
    table: Option<String>,
    /// Whether this line is a table header.
    is_header: bool,
}

/// The lines of `text` with the table each belongs to.
fn scan(text: &str) -> Vec<Line<'_>> {
    let mut table: Option<String> = None;
    let mut multiline: Option<&'static str> = None;
    let mut out = Vec::new();
    for line in text.split_inclusive('\n') {
        let (content, ending) = split_eol(line);
        let continued = multiline.is_some();
        track_multiline(content, &mut multiline);
        let header = if continued { None } else { header(content) };
        let is_header = header.is_some();
        if is_header {
            table = header;
        }
        out.push(Line {
            content,
            ending,
            continued,
            table: table.clone(),
            is_header,
        });
    }
    out
}

/// Whether `line` is a key line (not blank, not a comment, not a header).
fn is_key_line(line: &Line) -> bool {
    let t = line.content.trim_start();
    !line.is_header && !line.continued && !t.is_empty() && !t.starts_with('#')
}

/// `text` with `[section] key` set to `value` (rendered), or removed.
fn set_key(text: &str, section: &str, key: &str, value: Option<&str>) -> Option<String> {
    let eol = eol_of(text);
    let lines = scan(text);
    let mut out: Vec<String> = lines
        .iter()
        .map(|l| format!("{}{}", l.content, l.ending))
        .collect();
    let ours = |l: &Line| l.table.as_deref() == Some(section);
    // The header or the last key line of the table (a value's continuation
    // lines count as its own).
    let mut last: Option<usize> = None;
    for (i, line) in lines.iter().enumerate() {
        if !ours(line) || !(line.is_header || line.continued || is_key_line(line)) {
            continue;
        }
        if line.is_header && last.is_some() {
            return None; // the table twice: not valid TOML anyway
        }
        last = Some(i);
        if !is_key_line(line) || line_key(line.content, key).is_none() {
            continue;
        }
        let multiline_value = lines.get(i + 1).is_some_and(|next| next.continued);
        if multiline_value {
            return None;
        }
        match value {
            Some(value) => {
                out[i] = format!(
                    "{}{}",
                    replace_value(line.content, key, value)?,
                    line.ending
                )
            }
            None => out[i] = String::new(),
        }
        return Some(out.concat());
    }
    let value = value?; // removing a key this table does not hold
    let entry = format!("{} = {value}{eol}", render_key(key));
    match last {
        Some(i) => {
            if !out[i].ends_with('\n') {
                out[i].push_str(eol);
            }
            out.insert(i + 1, entry);
            Some(out.concat())
        }
        None => Some(append(text, &format!("[{section}]{eol}{entry}"))),
    }
}

/// `text` with the keys of sub-table `[section.name]` changed from `old`
/// to `new` one line at a time, as [`set_key`] does for a table: only a
/// changed key's line is replaced (its indent and trailing comment kept),
/// a removed key's line removed and a new key added after the last one;
/// the header's comment and the comments between keys stay. `None` when
/// the table has no header line of its own or either side holds a value
/// this editor does not write on one line (a deeper table).
fn edit_table_keys(
    text: &str,
    section: &str,
    name: &str,
    old: &Table,
    new: &Table,
) -> Option<String> {
    let wanted = format!("{section}.{}", render_key(name));
    let has_header = scan(text)
        .iter()
        .any(|l| l.is_header && l.table.as_deref() == Some(wanted.as_str()));
    let flat = |t: &Table| t.values().all(|v| line_value(v).is_some());
    if !(has_header && flat(old) && flat(new)) {
        return None;
    }
    edit_text(text, &wanted, old, new)
}

/// `text` with sub-table `[section.name]` set to `table`, or removed.
fn set_table(text: &str, section: &str, name: &str, table: Option<&Table>) -> Option<String> {
    let eol = eol_of(text);
    let wanted = format!("{section}.{}", render_key(name));
    let rendered = match table {
        Some(t) => {
            let mut s = format!("[{wanted}]{eol}");
            for (k, v) in t {
                s.push_str(&format!("{} = {}{eol}", render_key(k), line_value(v)?));
            }
            Some(s)
        }
        None => None,
    };
    let lines = scan(text);
    let start = lines
        .iter()
        .position(|l| l.is_header && l.table.as_deref() == Some(wanted.as_str()));
    let Some(start) = start else {
        return rendered.map(|r| append(text, &r)); // removing a table it lacks: None
    };
    // Through the line after its last key (blank lines and comments after
    // that stay).
    let mut end = start + 1;
    for (i, line) in lines.iter().enumerate().skip(start + 1) {
        if line.is_header {
            break;
        }
        if line.continued || is_key_line(line) {
            end = i + 1;
        }
    }
    let joined = |range: &[Line]| -> String {
        range
            .iter()
            .map(|l| format!("{}{}", l.content, l.ending))
            .collect()
    };
    let mut out = joined(&lines[..start]);
    if let Some(r) = rendered {
        out.push_str(&r);
    }
    out.push_str(&joined(&lines[end..]));
    Some(out)
}

/// `text` with `block` appended after one blank line.
fn append(text: &str, block: &str) -> String {
    let eol = eol_of(text);
    let mut out = text.to_owned();
    if !out.is_empty() {
        if !out.ends_with('\n') {
            out.push_str(eol);
        }
        out.push_str(eol);
    }
    out.push_str(block);
    out
}

/// `line` as its content and its line ending (`\r\n`, `\n` or none).
fn split_eol(line: &str) -> (&str, &str) {
    if let Some(content) = line.strip_suffix("\r\n") {
        (content, "\r\n")
    } else if let Some(content) = line.strip_suffix('\n') {
        (content, "\n")
    } else {
        (line, "")
    }
}

/// The table a header line opens (`[ name ]` → `name`, unquoted), `[[`
/// for an array of tables, or `None` when `content` is not a header.
fn header(content: &str) -> Option<String> {
    let t = content.trim_start();
    if t.starts_with("[[") {
        return Some("[[".to_owned());
    }
    let inner = t.strip_prefix('[')?;
    let end = inner.find(']')?;
    let rest = inner[end + 1..].trim_start();
    if !(rest.is_empty() || rest.starts_with('#')) {
        return None;
    }
    let name = inner[..end].trim();
    let unquoted = ['"', '\'']
        .iter()
        .find_map(|q| name.strip_prefix(*q).and_then(|n| n.strip_suffix(*q)))
        .unwrap_or(name);
    Some(unquoted.to_owned())
}

/// The key of `content` as written, when it is `key` (bare, `"key"` or
/// `'key'`) followed by `=`.
fn line_key<'a>(content: &'a str, key: &str) -> Option<&'a str> {
    let t = content.trim_start();
    let forms = [render_key(key), format!("\"{key}\""), format!("'{key}'")];
    forms.iter().find_map(|k| {
        let rest = t.strip_prefix(k.as_str())?;
        rest.trim_start().starts_with('=').then(|| &t[..k.len()])
    })
}

/// `content` with key `key`'s value set to `value` (already rendered),
/// keeping its indentation, the key as written, the spacing and a trailing
/// comment; `None` when the line is not `key`'s or its value does not end
/// on this line.
fn replace_value(content: &str, key: &str, value: &str) -> Option<String> {
    let t = content.trim_start();
    let indent = &content[..content.len() - t.len()];
    let written = line_key(content, key)?;
    let rest = &t[written.len()..];
    let eq = rest.find('=')?;
    let after = &rest[eq + 1..];
    let old = after.trim_start();
    let spaces = &after[..after.len() - old.len()];
    let tail = &old[value_end(old)?..];
    Some(format!(
        "{indent}{written}{}{spaces}{value}{tail}",
        &rest[..=eq]
    ))
}

/// Where the value at the start of `text` ends: the shortest prefix that
/// parses as one TOML value and is followed by nothing, spaces or a
/// comment. `None` when no prefix does (a value continued on later lines).
fn value_end(text: &str) -> Option<usize> {
    let ends = text
        .char_indices()
        .filter(|(_, c)| c.is_whitespace() || *c == '#')
        .map(|(i, _)| i)
        .chain([text.len()]);
    for end in ends.filter(|e| *e > 0) {
        let rest = text[end..].trim_start();
        if !(rest.is_empty() || rest.starts_with('#')) {
            continue;
        }
        if format!("v = {}", &text[..end]).parse::<Table>().is_ok() {
            return Some(end);
        }
    }
    None
}

/// Toggle the multi-line string `content` opens or closes (`"""`, `'''`).
fn track_multiline(content: &str, open: &mut Option<&'static str>) {
    match *open {
        Some(delim) => {
            if content.matches(delim).count() % 2 == 1 {
                *open = None;
            }
        }
        None => {
            // Only a value can open a multi-line string; a comment or a
            // key never does.
            if content.trim_start().starts_with('#') {
                return;
            }
            let Some(eq) = content.find('=') else { return };
            let value = content[eq + 1..].trim_start();
            for delim in ["\"\"\"", "'''"] {
                if value.starts_with(delim) && value.matches(delim).count() % 2 == 1 {
                    *open = Some(delim);
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use super::*;

    /// A scratch dir with `config.toml` holding `text` (if any).
    fn scratch(text: Option<&str>) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("claudio-cfg-edit-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        if let Some(text) = text {
            std::fs::write(&path, text).unwrap();
        }
        (dir, path)
    }

    fn strings(list: &[&str]) -> Value {
        Value::Array(list.iter().map(|s| Value::String(s.to_string())).collect())
    }

    /// A plugin's choices as `[plugins.<name>]` holds them.
    fn choices(disabled: &[&str], reviewed: &str, seen: &[&str]) -> Table {
        let mut t = Table::new();
        t.insert("disabled".into(), strings(disabled));
        t.insert("reviewed".into(), Value::String(reviewed.into()));
        t.insert("seen".into(), strings(seen));
        t
    }

    /// The env var that turns [`edit_section_child`] on: the config path.
    const CHILD: &str = "CLAUDIO_TEST_CONFIG_EDIT_CHILD";

    /// Add `[extensions] <prefix>0..<prefix>39 = true`, one edit each.
    fn add_keys(path: &Path, prefix: &str) {
        for i in 0..40 {
            edit_section(path, "extensions", |t| {
                t.insert(format!("{prefix}{i}"), Value::Boolean(true));
            })
            .unwrap();
        }
    }

    /// The second process of
    /// `two_processes_editing_distinct_keys_both_survive`; a no-op unless
    /// that test runs this binary with [`CHILD`] set.
    #[test]
    fn edit_section_child() {
        if let Some(path) = std::env::var_os(CHILD) {
            add_keys(Path::new(&path), "child");
        }
    }

    /// Two processes (this test and a second run of this test binary)
    /// edit distinct keys of one config at once: the cross-process lock
    /// serialises the read-modify-writes, so no edit is lost (B3, R15).
    #[test]
    fn two_processes_editing_distinct_keys_both_survive() {
        let (dir, path) = scratch(Some("# mine\n[extensions]\n"));
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "app::config_edit::tests::edit_section_child",
                "--test-threads=1",
                "--quiet",
            ])
            .env(CHILD, &path)
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        add_keys(&path, "parent");
        let mut child = child;
        assert!(child.wait().unwrap().success(), "the child's edits failed");
        let text = std::fs::read_to_string(&path).unwrap();
        let table: Table = text.parse().unwrap();
        let ext = table["extensions"].as_table().unwrap();
        for prefix in ["parent", "child"] {
            for i in 0..40 {
                let key = format!("{prefix}{i}");
                assert_eq!(ext.get(&key), Some(&Value::Boolean(true)), "{key} lost");
            }
        }
        assert!(text.starts_with("# mine\n"));
        assert!(dir.join(".config.toml.lock").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn edit_section_sets_keys_keeping_comments_and_the_rest_byte_identical() {
        let text = "# top\n\
                    [extensions]\n  \
                    diagram   =  true   # keep me\n\
                    # snippets are handy\n\
                    snippets = true\n\
                    \n\
                    [proxy.profiles.work]\n\
                    token = \"s3cr3t\"   # never touch\n";
        let (dir, path) = scratch(Some(text));
        let changed = edit_section(&path, "extensions", |t| {
            t.insert("diagram".into(), Value::Boolean(false));
            t.insert("search".into(), Value::Boolean(true));
        })
        .unwrap();
        assert!(changed);
        let expected = text
            .replace("=  true   # keep me", "=  false   # keep me")
            .replace("snippets = true\n", "snippets = true\nsearch = true\n");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), expected);
        // The same edit again changes nothing and writes nothing.
        let unchanged = edit_section(&path, "extensions", |t| {
            t.insert("search".into(), Value::Boolean(true));
        })
        .unwrap();
        assert!(!unchanged);
        // A key goes, its line with it.
        edit_section(&path, "extensions", |t| {
            t.remove("snippets");
        })
        .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("snippets = true") && text.contains("# snippets are handy"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn edit_section_writes_strings_arrays_and_numbers_and_keeps_crlf() {
        let text = "[ui]\r\nnotify = true\r\n[team]\r\nname = \"a b\" # who\r\n";
        let (dir, path) = scratch(Some(text));
        edit_section(&path, "team", |t| {
            t.insert("name".into(), Value::String("c \"d\"".into()));
            t.insert("ids".into(), strings(&["x", "y"]));
            t.insert("limit".into(), Value::Integer(3));
        })
        .unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            out,
            "[ui]\r\nnotify = true\r\n[team]\r\nname = 'c \"d\"' # who\r\n\
             ids = [\"x\", \"y\"]\r\nlimit = 3\r\n"
        );
        let parsed: Table = out.parse().unwrap();
        assert_eq!(parsed["team"]["name"].as_str(), Some("c \"d\""));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The plugin-choices fixtures: a sub-table is replaced in place (the
    /// comments after its last key stay), appended when missing, removed.
    #[test]
    fn edit_section_replaces_appends_and_removes_sub_tables() {
        let text = "[plugins.team]\ndisabled = []\nreviewed = \"0.9\"\nseen = []\n\
                    # after the table\n\n[ui]\nnotify = false\n";
        let (dir, path) = scratch(Some(text));
        let c = choices(&["mcp:jina"], "1.0.0", &["mcp:jina", "plugin:a@m"]);
        edit_section(&path, "plugins", |t| {
            t.insert("team".into(), Value::Table(c.clone()));
        })
        .unwrap();
        let table = "[plugins.team]\ndisabled = [\"mcp:jina\"]\nreviewed = \"1.0.0\"\n\
                     seen = [\"mcp:jina\", \"plugin:a@m\"]\n";
        let expected = format!("{table}# after the table\n\n[ui]\nnotify = false\n");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), expected);
        // Another plugin's table is appended.
        edit_section(&path, "plugins", |t| {
            t.insert("other".into(), Value::Table(choices(&[], "1", &[])));
        })
        .unwrap();
        let other = "\n[plugins.other]\ndisabled = []\nreviewed = \"1\"\nseen = []\n";
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            format!("{expected}{other}")
        );
        // And removed again.
        edit_section(&path, "plugins", |t| {
            t.remove("other");
        })
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            format!("{expected}\n")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A changed key inside an existing sub-table changes that key's line
    /// only: the header's comment, the comments between keys and the
    /// trailing comments stay (R17, B3).
    #[test]
    fn edit_section_keeps_comments_inside_a_sub_table() {
        let text = "# top\n\
                    [plugins.team] # the team's plugins\n\
                    # why these are off\n\
                    disabled = [\"mcp:a\"] # reviewed in June\n\
                    \x20 # between keys\n\
                    reviewed = \"0.9\"   # last review\n\
                    seen = []\n\
                    # after the table\n\
                    \n\
                    [ui]\n\
                    notify = false\n";
        let (dir, path) = scratch(Some(text));
        let edit = |f: fn(&mut Table)| {
            edit_section(&path, "plugins", |t| {
                if let Some(Value::Table(team)) = t.get_mut("team") {
                    f(team);
                }
            })
            .unwrap();
            std::fs::read_to_string(&path).unwrap()
        };
        let got = edit(|team| {
            team.insert("reviewed".into(), Value::String("1.0.0".into()));
            team.insert("seen".into(), strings(&["mcp:a"]));
        });
        let changed = text
            .replace("reviewed = \"0.9\"   #", "reviewed = \"1.0.0\"   #")
            .replace("seen = []", "seen = [\"mcp:a\"]");
        assert_eq!(got, changed);
        // A new key goes after the last one; a removed key's line goes and
        // the comment above it stays.
        let got = edit(|team| {
            team.remove("disabled");
            team.insert("extra".into(), Value::Boolean(true));
        });
        let expected = changed
            .replace("disabled = [\"mcp:a\"] # reviewed in June\n", "")
            .replace("seen = [\"mcp:a\"]\n", "seen = [\"mcp:a\"]\nextra = true\n");
        assert_eq!(got, expected);
        assert!(got.contains("[plugins.team] # the team's plugins\n# why these are off\n"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn edit_section_creates_a_missing_file_private_and_keeps_a_files_mode() {
        let (dir, path) = scratch(None);
        edit_section(&path, "extensions", |t| {
            t.insert("search".into(), Value::Boolean(true));
        })
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[extensions]\nsearch = true\n"
        );
        let mode = || std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(), 0o600);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        edit_section(&path, "extensions", |t| {
            t.insert("search".into(), Value::Boolean(false));
        })
        .unwrap();
        assert_eq!(mode(), 0o640);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn edit_section_writes_through_a_symlink_to_its_target() {
        let (dir, target) = scratch(Some("[ui]\nnotify = true\n"));
        let link = dir.join("link.toml");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        edit_section(&link, "extensions", |t| {
            t.insert("diagram".into(), Value::Boolean(false));
        })
        .unwrap();
        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "[ui]\nnotify = true\n\n[extensions]\ndiagram = false\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn edit_section_refuses_nested_tables() {
        let text = "[plugins.team]\nreviewed = \"1\"\n";
        let (dir, path) = scratch(Some(text));
        let refused = |edit: Box<dyn FnOnce(&mut Table)>| {
            let err = edit_section(&path, "plugins", edit).unwrap_err();
            assert_eq!(
                err.to_string(),
                "config.toml layout not understood; edit [plugins] by hand"
            );
            assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        };
        // A table in a sub-table, a float, a date-like value in an array
        // of tables: none is a line this editor writes.
        refused(Box::new(|t| {
            let mut deep = Table::new();
            deep.insert("x".into(), Value::Table(Table::new()));
            t.insert("team".into(), Value::Table(deep));
        }));
        refused(Box::new(|t| {
            t.insert("ratio".into(), Value::Float(0.5));
        }));
        refused(Box::new(|t| {
            t.insert(
                "list".into(),
                Value::Array(vec![Value::Table(Table::new())]),
            );
        }));
        // A key turned into a table.
        refused(Box::new(|t| {
            t.insert("team".into(), Value::Boolean(true));
        }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn edit_section_refuses_layouts_it_cannot_edit_and_invalid_toml() {
        // An inline table, a dotted key and a multi-line value: the edit
        // would not land where the value is, so nothing is written.
        for text in [
            "team = { reviewed = \"1\" }\n",
            "team.reviewed = \"1\"\n",
            "[team]\nnote = \"\"\"\nline\n\"\"\"\n",
        ] {
            let (dir, path) = scratch(Some(text));
            let err = edit_section(&path, "team", |t| {
                t.insert("reviewed".into(), Value::String("2".into()));
                t.insert("note".into(), Value::String("x".into()));
            })
            .unwrap_err();
            assert!(matches!(err, SaveError::UnsupportedSection(_)), "{text}");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
            let _ = std::fs::remove_dir_all(&dir);
        }
        let bad = "[proxy\ntoken = \"s3cr3t\"\n";
        let (dir, path) = scratch(Some(bad));
        let err = edit_section(&path, "team", |_| {}).unwrap_err();
        assert_eq!(err.to_string(), "config.toml is not valid TOML");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), bad);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A header the line scanner cannot read (a quoted name with a `]`)
    /// makes the table seem to go on; the key would land in the next table,
    /// so the parse check refuses the edit and nothing is written.
    #[test]
    fn edit_section_writes_nothing_when_another_section_would_change() {
        let text = "[team]\na = 1\n[\"x]y\"]\nk = 1\n";
        let (dir, path) = scratch(Some(text));
        let err = edit_section(&path, "team", |t| {
            t.insert("b".into(), Value::Integer(2));
        })
        .unwrap_err();
        assert!(matches!(err, SaveError::UnsupportedSection(_)));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn edits_are_serialised_by_one_lock() {
        let (dir, path) = scratch(Some(""));
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    edit_section(&path, "team", |t| {
                        t.insert(format!("k{i}"), Value::Integer(i));
                    })
                    .unwrap()
                })
            })
            .collect();
        for t in threads {
            assert!(t.join().unwrap());
        }
        let parsed: Table = std::fs::read_to_string(&path).unwrap().parse().unwrap();
        assert_eq!(parsed["team"].as_table().unwrap().len(), 8, "no edit lost");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
