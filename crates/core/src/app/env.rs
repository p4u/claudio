//! The custom env of claude sessions (docs/specs/2026-11-15-custom-env-design.md):
//! the global scope in `config.toml [env]`, the per-session scope in
//! `session-env.json` beside it. The one implementation the TUI, the web and
//! the launch composer use (A5).
//!
//! Every name and value passes `claude::env`'s validator when it is written
//! and again when it is read; an entry that fails is skipped with one notice.
//! Values are secrets at rest (both files 0600): nothing here logs or formats
//! one, and errors and notices name keys at most, only well-formed ones (C1).

use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::addon::{line, TEXT_BYTES};
use crate::app::config_edit;
use crate::claude::env::{
    check_name, check_value, name_ok, refused, shown, Refusal, Value, MAX_SCOPE_BYTES, MAX_VARS,
};
use crate::config::SaveError;
use crate::paths;
use crate::proto::SessionId;

/// The `config.toml` table of the global scope.
pub const SECTION: &str = "env";
/// The per-session file, beside `config.toml`.
pub const SESSION_FILE: &str = "session-env.json";
/// The most bytes `session-env.json` may hold.
pub const MAX_SESSION_FILE: usize = 8 * 1024 * 1024;
/// The format of `session-env.json` this build reads and writes.
const VERSION: u32 = 1;
/// How many skipped names a notice lists.
const NAMES_SHOWN: usize = 8;
/// What a front-end shows when `session-env.json` cannot be read.
pub const UNREADABLE: &str = "session-env.json is not readable: fix or delete it";

/// Which env a read or a write is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// Every claude session, on every host: `config.toml [env]`.
    Global,
    /// One claude session, looked up by `sid` alone (the TUI and the web
    /// may name one host differently); `host` as the front-end names it
    /// (`local` or an ssh alias) is recorded for pruning.
    Session { host: String, sid: SessionId },
}

/// A scope as read: its valid entries in name order, how many entries of the
/// file were skipped, and the notices about them (key names only).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Listed {
    pub vars: Vec<(String, Value)>,
    pub skipped: usize,
    pub notices: Vec<String>,
}

/// Why a write did not happen. Its text names a key at most, never a value.
#[derive(Debug)]
pub enum EnvError {
    /// The name or the value is refused; the text says which and why.
    Refused(String),
    /// The scope would hold more than [`MAX_VARS`] variables.
    TooMany,
    /// The scope would hold more than [`MAX_SCOPE_BYTES`].
    TooLarge,
    /// `session-env.json` would pass [`MAX_SESSION_FILE`].
    Full,
    /// `session-env.json` cannot be read; it is never overwritten.
    Unreadable,
    /// The `config.toml` edit failed.
    Save(SaveError),
    /// `session-env.json` could not be locked or written.
    Io(io::Error),
}

impl fmt::Display for EnvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EnvError::Refused(why) => write!(f, "{why}"),
            EnvError::TooMany => write!(f, "too many variables (at most {MAX_VARS})"),
            EnvError::TooLarge => write!(f, "environment too large (at most 128 KiB)"),
            EnvError::Full => write!(f, "session-env.json is full (8 MiB)"),
            EnvError::Unreadable => write!(f, "{UNREADABLE}"),
            EnvError::Save(e) => write!(f, "could not save: {e}"),
            EnvError::Io(e) => write!(f, "could not save: {e}"),
        }
    }
}

/// `session-env.json`. No `Debug`: it holds values.
#[derive(Serialize, Deserialize)]
struct SessionFile {
    version: u32,
    sessions: BTreeMap<SessionId, SessionEntry>,
}

/// One session's entry. A value that is not a string (a hand edit) is kept
/// and skipped on read.
#[derive(Serialize, Deserialize)]
struct SessionEntry {
    host: String,
    env: BTreeMap<String, serde_json::Value>,
}

impl Default for SessionFile {
    fn default() -> SessionFile {
        SessionFile {
            version: VERSION,
            sessions: BTreeMap::new(),
        }
    }
}

/// The custom env's two files: a `config.toml` and `session-env.json` beside
/// it. Every call reads the files again: nothing is cached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Store {
    config: PathBuf,
    sessions: PathBuf,
}

impl Store {
    /// The store of the config file at `config_path` (a runtime's
    /// `RuntimeOptions.config_path`).
    pub fn beside(config_path: &Path) -> Store {
        let dir = config_path.parent().unwrap_or(Path::new("."));
        Store {
            config: config_path.to_path_buf(),
            sessions: dir.join(SESSION_FILE),
        }
    }

    /// The valid entries of `scope` (a session's by its id alone). An
    /// unreadable file reads as empty, with a notice.
    pub fn list(&self, scope: &Scope) -> Listed {
        match scope {
            Scope::Global => self.global(),
            Scope::Session { sid, .. } => self.session(*sid),
        }
    }

    /// Global overlaid with the session's own (it wins), in name order, and
    /// the notices of both reads: what a launch of `sid` gets. The session
    /// is looked up by `sid` alone; `_host` is the caller's name for its
    /// host, not needed for the lookup.
    pub fn effective(&self, _host: &str, sid: SessionId) -> (Vec<(String, Value)>, Vec<String>) {
        let global = self.global();
        let session = self.session(sid);
        let mut vars: BTreeMap<String, Value> = global.vars.into_iter().collect();
        vars.extend(session.vars);
        let mut notices = global.notices;
        notices.extend(session.notices);
        (vars.into_iter().collect(), notices)
    }

    /// Set `name` to `value` in `scope`, replacing whatever the file holds
    /// for it; refused past the caps. A session's entry records `host`.
    pub fn set(&self, scope: &Scope, name: &str, value: &Value) -> Result<(), EnvError> {
        check_name(name).map_err(|e| EnvError::Refused(shown(name, &e)))?;
        check_value(value.as_str()).map_err(|e| EnvError::Refused(shown(name, &e)))?;
        match scope {
            Scope::Global => {
                let mut over = None;
                config_edit::edit_section(&self.config, SECTION, |table| {
                    match fits(&valid_vars(entries_of(table)), name, value) {
                        Ok(()) => {
                            let text = toml::Value::String(value.as_str().to_owned());
                            table.insert(name.to_owned(), text);
                        }
                        Err(e) => over = Some(e),
                    }
                })
                .map_err(EnvError::Save)?;
                over.map_or(Ok(()), Err)
            }
            Scope::Session { host, sid } => self.edit_sessions(|file| {
                let entry = file.sessions.entry(*sid).or_insert_with(|| SessionEntry {
                    host: host.clone(),
                    env: BTreeMap::new(),
                });
                let own = entry.env.iter().map(|(k, v)| (k.clone(), string(v)));
                fits(&valid_vars(own.collect()), name, value)?;
                entry.host = host.clone();
                let text = serde_json::Value::String(value.as_str().to_owned());
                entry.env.insert(name.to_owned(), text);
                Ok(true)
            }),
        }
    }

    /// Remove `name` from `scope` (a name it does not hold is not an error).
    pub fn unset(&self, scope: &Scope, name: &str) -> Result<(), EnvError> {
        if !name_ok(name) {
            return Err(EnvError::Refused(Refusal::BadName.to_string()));
        }
        match scope {
            Scope::Global => config_edit::edit_section(&self.config, SECTION, |table| {
                table.remove(name);
            })
            .map(|_| ())
            .map_err(EnvError::Save),
            Scope::Session { sid, .. } => self.edit_sessions(|file| {
                let Some(entry) = file.sessions.get_mut(sid) else {
                    return Ok(false);
                };
                let removed = entry.env.remove(name).is_some();
                if entry.env.is_empty() {
                    file.sessions.remove(sid);
                }
                Ok(removed)
            }),
        }
    }

    /// Drop session `sid`'s entry (its session was removed), looked up by
    /// `sid` alone (`_host`: the caller's name for its host).
    pub fn forget_session(&self, _host: &str, sid: SessionId) -> Result<(), EnvError> {
        if !self.sessions.exists() {
            return Ok(());
        }
        self.edit_sessions(|file| Ok(file.sessions.remove(&sid).is_some()))
    }

    /// Drop the entries of `host`'s sessions that are not in `live` (its
    /// daemon's list): sessions removed while no front-end ran.
    pub fn prune(&self, host: &str, live: &[SessionId]) -> Result<(), EnvError> {
        if !self.sessions.exists() {
            return Ok(());
        }
        self.edit_sessions(|file| {
            let before = file.sessions.len();
            file.sessions
                .retain(|sid, e| e.host != host || live.contains(sid));
            Ok(file.sessions.len() != before)
        })
    }

    /// `[env]`'s valid entries.
    fn global(&self) -> Listed {
        match read_table(&self.config) {
            Ok(entries) => valid("[env]", entries),
            Err(notice) => Listed {
                notices: vec![notice],
                ..Listed::default()
            },
        }
    }

    /// Session `sid`'s valid entries.
    fn session(&self, sid: SessionId) -> Listed {
        let file = match read_sessions(&self.sessions) {
            Ok(file) => file,
            Err(()) => {
                return Listed {
                    notices: vec![UNREADABLE.to_owned()],
                    ..Listed::default()
                }
            }
        };
        match file.sessions.get(&sid) {
            Some(entry) => {
                let entries = entry.env.iter().map(|(k, v)| (k.clone(), string(v)));
                valid("session env", entries.collect())
            }
            None => Listed::default(),
        }
    }

    /// Read-modify-write `session-env.json` under its sidecar lock: `edit`
    /// says whether it changed anything. An unreadable file fails the write
    /// and stays as it is.
    fn edit_sessions(
        &self,
        edit: impl FnOnce(&mut SessionFile) -> Result<bool, EnvError>,
    ) -> Result<(), EnvError> {
        let _lock =
            paths::FileLock::beside(&self.sessions, paths::LOCK_WAIT).map_err(EnvError::Io)?;
        let mut file = read_sessions(&self.sessions).map_err(|()| EnvError::Unreadable)?;
        if !edit(&mut file)? {
            return Ok(());
        }
        let bytes = serde_json::to_vec(&file).map_err(|e| EnvError::Io(io::Error::other(e)))?;
        if bytes.len() > MAX_SESSION_FILE {
            return Err(EnvError::Full);
        }
        paths::write_atomic(&self.sessions, &bytes).map_err(EnvError::Io)
    }
}

/// A JSON value as an entry's text (`None`: not a string).
fn string(v: &serde_json::Value) -> Option<String> {
    v.as_str().map(str::to_owned)
}

/// `table`'s entries as names and texts (`None`: not a string).
fn entries_of(table: &toml::Table) -> Vec<(String, Option<String>)> {
    table
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().map(str::to_owned)))
        .collect()
}

/// `[env]` of the config file at `path` (a missing file or table: none),
/// or the notice why it cannot be read.
fn read_table(path: &Path) -> Result<Vec<(String, Option<String>)>, String> {
    let text = match std::fs::read(path) {
        Ok(bytes) => {
            String::from_utf8(bytes).map_err(|_| "config.toml is not UTF-8: [env] not applied")?
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err("config.toml is not readable: [env] not applied".to_owned()),
    };
    let table: toml::Table = text
        .parse()
        .map_err(|_| "config.toml is not valid TOML: [env] not applied".to_owned())?;
    match table.get(SECTION) {
        None => Ok(Vec::new()),
        Some(toml::Value::Table(env)) => Ok(entries_of(env)),
        Some(_) => Err("config.toml: [env] is not a table: not applied".to_owned()),
    }
}

/// `session-env.json` at `path` (missing: empty); `Err` when it cannot be
/// read, is over [`MAX_SESSION_FILE`], is not this format or of another
/// version.
fn read_sessions(path: &Path) -> Result<SessionFile, ()> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(SessionFile::default()),
        Err(_) => return Err(()),
    };
    let mut bytes = Vec::new();
    let cap = MAX_SESSION_FILE as u64 + 1;
    file.take(cap).read_to_end(&mut bytes).map_err(|_| ())?;
    if bytes.len() > MAX_SESSION_FILE {
        return Err(());
    }
    let parsed: SessionFile = serde_json::from_slice(&bytes).map_err(|_| ())?;
    if parsed.version != VERSION {
        return Err(());
    }
    Ok(parsed)
}

/// The valid entries of `entries`, in name order, within the caps.
fn valid_vars(entries: Vec<(String, Option<String>)>) -> Vec<(String, Value)> {
    valid("", entries).vars
}

/// `entries` checked in name order: a bad or refused name, a value that is
/// not a string or fails the validator is skipped; past a cap the rest are.
/// One notice, prefixed `label`, says what was skipped (names only when they
/// are well formed).
fn valid(label: &str, mut entries: Vec<(String, Option<String>)>) -> Listed {
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let (mut vars, mut named, mut bad_names, mut over) = (Vec::new(), Vec::new(), 0, 0);
    let mut bytes = 0;
    for (name, text) in entries {
        if !name_ok(&name) {
            bad_names += 1;
            continue;
        }
        let why = match &text {
            _ if refused(&name).is_some() => Some("refused"),
            None => Some("not a string"),
            Some(text) if check_value(text).is_err() => Some("bad value"),
            Some(_) => None,
        };
        if let Some(why) = why {
            named.push(format!("{name} ({why})"));
            continue;
        }
        let text = text.unwrap_or_default();
        let size = name.len() + text.len();
        if over > 0 || vars.len() == MAX_VARS || bytes + size > MAX_SCOPE_BYTES {
            over += 1;
            continue;
        }
        bytes += size;
        vars.push((name, Value::new(text)));
    }
    let skipped = named.len() + bad_names + over;
    let mut parts: Vec<String> = named.iter().take(NAMES_SHOWN).cloned().collect();
    if named.len() > NAMES_SHOWN {
        parts.push(format!("{} more", named.len() - NAMES_SHOWN));
    }
    if bad_names > 0 {
        parts.push(format!("{bad_names} with a bad name"));
    }
    if over > 0 {
        parts.push(format!(
            "{over} past the caps ({MAX_VARS} variables, 128 KiB)"
        ));
    }
    let notices = if skipped == 0 || label.is_empty() {
        Vec::new()
    } else {
        let text = format!("{label}: {skipped} skipped: {}", parts.join(", "));
        vec![line(&text, TEXT_BYTES)]
    };
    Listed {
        vars,
        skipped,
        notices,
    }
}

/// Whether `vars` (without `name`) and `name = value` stay within the caps.
fn fits(vars: &[(String, Value)], name: &str, value: &Value) -> Result<(), EnvError> {
    let others = vars.iter().filter(|(k, _)| k != name);
    let (count, bytes) = others.fold((0, 0), |(n, b), (k, v)| (n + 1, b + k.len() + v.len()));
    if count + 1 > MAX_VARS {
        return Err(EnvError::TooMany);
    }
    if bytes + name.len() + value.len() > MAX_SCOPE_BYTES {
        return Err(EnvError::TooLarge);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
