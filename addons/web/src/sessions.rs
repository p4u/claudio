//! Sign-ins (docs/web.md, "Cookie"; docs/web-protocol.md §1): the
//! server-side sessions a `claudio_web` cookie names, kept across restarts
//! in `<config>/web-sessions.json` (0600).
//!
//! [`Store`] is pure: each entry is the SHA-256 of a cookie value and its
//! last use in wall-clock Unix seconds; a session unused for 30 days is
//! gone, and past 64 the least recently used goes. `now` is a parameter
//! (A6). The file format, [`epoch`], [`load`] and the writer task of
//! [`Sessions`] are the I/O edge. The file holds hashes only: a copy of it
//! signs no one in.

use std::io::{self, Read};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::watch;

use claudio_core::paths;

use crate::auth::{ct_eq, token_ok};

/// A session unused this long (seconds) is gone.
pub(crate) const IDLE: u64 = 30 * 24 * 60 * 60;
/// Sessions kept at most; past it the least recently used goes.
pub(crate) const MAX: usize = 64;
/// The file's `seen` of a session is rewritten once memory's is this much
/// newer (seconds): uses do not each write the disk.
pub(crate) const WRITE_EVERY: u64 = 60 * 60;
/// How long a login or a sign-out waits for its write, and the last write
/// at stop.
pub(crate) const WRITE_WAIT: Duration = Duration::from_secs(2);
/// An open tab marks its session used this often ([`OpenTab`]).
pub(crate) const MARK_EVERY: Duration = Duration::from_secs(60 * 60);
/// A failed write is tried again after this, doubling up to [`RETRY_MAX`].
const RETRY_FIRST: Duration = Duration::from_secs(60);
const RETRY_MAX: Duration = Duration::from_secs(60 * 60);
/// The largest sessions file read.
const FILE_MAX: u64 = 64 * 1024;
const VERSION: u64 = 1;

/// The SHA-256 of a cookie value.
pub(crate) type Hash = [u8; 32];

/// `<config>/web-sessions.json`.
pub(crate) fn path() -> PathBuf {
    paths::config_dir().join("web-sessions.json")
}

/// The SHA-256 of `value` when it is one this server could have made (64
/// lowercase hex); anything else is refused without hashing.
pub(crate) fn hash_of(value: &str) -> Option<Hash> {
    token_ok(value).then(|| Sha256::digest(value.as_bytes()).into())
}

/// Which listener the server runs: the epoch's `mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Loopback (`--listen`).
    Local,
    /// `--remote`.
    Remote,
}

/// What the sessions file is bound to: the lowercase hex SHA-256 of
/// `"claudio-web-sessions-v1" ‖ 0 ‖ mode ‖ 0 ‖ token ‖ 0 ‖ password`, where
/// `password` is the bytes of `web-password` as read at this start (empty
/// when it is missing or not acceptable). Computed once per start: a new
/// token, password or mode signs every browser out at the next start.
pub(crate) fn epoch(mode: Mode, token: &str, password: &[u8]) -> String {
    let mode = match mode {
        Mode::Local => "local",
        Mode::Remote => "remote",
    };
    let mut h = Sha256::new();
    h.update(b"claudio-web-sessions-v1");
    for part in [mode.as_bytes(), token.as_bytes(), password] {
        h.update([0u8]);
        h.update(part);
    }
    hex(&h.finalize())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    hash: Hash,
    /// Last use, Unix seconds.
    seen: u64,
    /// `seen` as the file holds it (0: not written yet).
    written: u64,
    /// When a use last asked for a write of this entry (0: never): while
    /// the file cannot be written, uses do not each ask again.
    asked: u64,
}

/// The sessions, pure: every method takes `now`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Store {
    entries: Vec<Entry>,
}

impl Store {
    /// A `seen` later than `now` pulled back to `now`, idle entries
    /// dropped, and of more than [`MAX`] the most recently seen kept.
    fn tidy(&mut self, now: u64) {
        for e in &mut self.entries {
            e.seen = e.seen.min(now);
        }
        self.entries.retain(|e| now - e.seen < IDLE);
        if self.entries.len() > MAX {
            self.entries.sort_by_key(|e| std::cmp::Reverse(e.seen));
            self.entries.truncate(MAX);
        }
    }

    /// The entry `hash` names, every hash compared in constant time.
    fn find(&self, hash: &Hash) -> Option<usize> {
        let mut found = None;
        for (i, e) in self.entries.iter().enumerate() {
            if ct_eq(&e.hash, hash) {
                found = Some(i);
            }
        }
        found
    }

    /// A login: `new`'s session in, and `old`'s (the cookie the login came
    /// with) out in the same step; past [`MAX`] the least recently seen goes.
    pub(crate) fn login(&mut self, new: Hash, old: Option<&str>, now: u64) {
        if let Some(old) = old {
            self.remove(old);
        }
        self.tidy(now);
        if self.entries.len() >= MAX {
            let oldest = (0..self.entries.len()).min_by_key(|&i| self.entries[i].seen);
            if let Some(i) = oldest {
                self.entries.remove(i);
            }
        }
        self.entries.push(Entry {
            hash: new,
            seen: now,
            written: 0,
            asked: 0,
        });
    }

    /// A use of `hash`'s session if it is live (its `seen` becomes `now`):
    /// `Some(true)` when the file's `seen` is then over [`WRITE_EVERY`]
    /// older than memory's.
    pub(crate) fn use_hash(&mut self, hash: &Hash, now: u64) -> Option<bool> {
        self.tidy(now);
        let i = self.find(hash)?;
        let e = &mut self.entries[i];
        e.seen = now;
        let due = now - e.written.max(e.asked).min(now) > WRITE_EVERY;
        if due {
            e.asked = now;
        }
        Some(due)
    }

    /// A sign-out: `value`'s session out.
    pub(crate) fn remove(&mut self, value: &str) {
        if let Some(i) = hash_of(value).and_then(|h| self.find(&h)) {
            self.entries.remove(i);
        }
    }

    /// The file's bytes for `epoch`.
    fn to_file(&self, epoch: &str) -> Vec<u8> {
        let sessions = self
            .entries
            .iter()
            .map(|e| FileEntry {
                hash: hex(&e.hash),
                seen: e.seen,
            })
            .collect();
        let file = FileV1 {
            version: VERSION,
            epoch: epoch.to_owned(),
            sessions,
        };
        serde_json::to_vec(&file).unwrap_or_default()
    }

    /// `snapshot` was written: what the file now holds of each session.
    fn written(&mut self, snapshot: &Store) {
        for e in &mut self.entries {
            if let Some(w) = snapshot.entries.iter().find(|w| w.hash == e.hash) {
                e.written = w.seen;
            }
        }
    }
}

/// `web-sessions.json`, version 1.
#[derive(Serialize, Deserialize)]
struct FileV1 {
    version: u64,
    epoch: String,
    sessions: Vec<FileEntry>,
}

#[derive(Serialize, Deserialize)]
struct FileEntry {
    hash: String,
    seen: u64,
}

/// The store the file at `path` holds for `epoch`, tidied at `now`, and why
/// it was ignored if it was (one line for stderr). A missing file and one
/// of another epoch give an empty store and no reason. Blocking I/O.
pub(crate) fn load(path: &Path, epoch: &str, now: u64) -> (Store, Option<String>) {
    match read(path, epoch) {
        Ok(Some(mut store)) => {
            store.tidy(now);
            (store, None)
        }
        Ok(None) => (Store::default(), None),
        Err(why) => (Store::default(), Some(why)),
    }
}

/// `Ok(None)`: missing, or bound to another epoch. Opened without
/// following a symlink and checked on the open file (as
/// `claude/hooks.rs`'s `read_context`).
fn read(path: &Path, epoch: &str) -> Result<Option<Store>, String> {
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) if e.raw_os_error() == Some(libc::ELOOP) => return Err("a symlink".to_owned()),
        Err(e) => return Err(e.to_string()),
    };
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if !meta.file_type().is_file() || meta.uid() != paths::uid() || meta.mode() & 0o077 != 0 {
        return Err("not a private (0600) regular file of yours".to_owned());
    }
    let mut bytes = Vec::new();
    file.take(FILE_MAX + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > FILE_MAX {
        return Err("over 64 KiB".to_owned());
    }
    let file: FileV1 =
        serde_json::from_slice(&bytes).map_err(|_| "not a sessions file".to_owned())?;
    if file.version != VERSION {
        return Err(format!("version {} is not {VERSION}", file.version));
    }
    if file.epoch != epoch {
        return Ok(None);
    }
    let mut store = Store::default();
    for s in file.sessions {
        let hash = parse_hash(&s.hash).ok_or("a malformed hash")?;
        if store.find(&hash).is_some() {
            return Err("a hash twice".to_owned());
        }
        store.entries.push(Entry {
            hash,
            seen: s.seen,
            written: s.seen,
            asked: 0,
        });
    }
    Ok(Some(store))
}

/// 64 lowercase hex as 32 bytes.
fn parse_hash(s: &str) -> Option<Hash> {
    if !token_ok(s) {
        return None;
    }
    let mut out = [0u8; 32];
    for (byte, pair) in out.iter_mut().zip(s.as_bytes().chunks(2)) {
        *byte = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(out)
}

/// The store the server uses, its epoch and file, and the writer that
/// keeps the file current: one task that always writes the newest store
/// (a `watch`, so nothing queues: A10).
pub(crate) struct Sessions {
    store: Mutex<Store>,
    /// Fixed at start: a write never reads the token or the password.
    epoch: String,
    /// `None`: in memory only.
    path: Option<PathBuf>,
    /// Bumped by every change the file must get; the writer writes the newest.
    wanted: watch::Sender<u64>,
    /// The last change the writer has tried to write (or failed to).
    tried: watch::Sender<u64>,
}

impl Sessions {
    /// `store` (from [`load`]), kept in `path` for `epoch`.
    pub(crate) fn new(store: Store, epoch: String, path: PathBuf) -> Sessions {
        Sessions {
            store: Mutex::new(store),
            epoch,
            path: Some(path),
            wanted: watch::Sender::new(0),
            tried: watch::Sender::new(0),
        }
    }

    /// An empty store, never written (unit tests).
    #[cfg(test)]
    pub(crate) fn memory() -> Sessions {
        Sessions {
            store: Mutex::default(),
            epoch: String::new(),
            path: None,
            wanted: watch::Sender::new(0),
            tried: watch::Sender::new(0),
        }
    }

    /// Never held across an await.
    fn lock(&self) -> MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Ask the writer for the newest store; the change's number.
    fn change(&self) -> u64 {
        let mut gen = 0;
        self.wanted.send_modify(|w| {
            *w += 1;
            gen = *w;
        });
        gen
    }

    /// [`Store::login`]; the change a reply waits for ([`Sessions::written`]).
    pub(crate) fn login(&self, new: Hash, old: Option<&str>, now: u64) -> u64 {
        self.lock().login(new, old, now);
        self.change()
    }

    /// Whether `value` names a live session (a use); asks for a write when
    /// the file's copy is over an hour old.
    pub(crate) fn check(&self, value: &str, now: u64) -> bool {
        let Some(hash) = hash_of(value) else {
            return false;
        };
        self.mark(&hash, now)
    }

    /// A use of `hash`'s session; `false` when it is gone (nothing happens).
    pub(crate) fn mark(&self, hash: &Hash, now: u64) -> bool {
        let used = self.lock().use_hash(hash, now);
        if used == Some(true) {
            self.change();
        }
        used.is_some()
    }

    /// [`Store::remove`]; the change a reply waits for.
    pub(crate) fn sign_out(&self, value: &str) -> u64 {
        self.lock().remove(value);
        self.change()
    }

    /// Until the writer has tried change `gen` (written or failed), at most
    /// [`WRITE_WAIT`]; at once in memory.
    pub(crate) async fn written(&self, gen: u64) {
        if self.path.is_none() {
            return;
        }
        let mut tried = self.tried.subscribe();
        let _ = tokio::time::timeout(WRITE_WAIT, tried.wait_for(|t| *t >= gen)).await;
    }

    /// The last write, in `serve`'s stop sequence (≤ [`WRITE_WAIT`]).
    pub(crate) async fn flush(&self) {
        let gen = self.change();
        self.written(gen).await;
    }

    /// The writer task (none in memory); aborted at stop. Changes from now
    /// on are written.
    pub(crate) fn start_writer(self: &Arc<Self>) -> Option<tokio::task::JoinHandle<()>> {
        let path = self.path.clone()?;
        let wanted = self.wanted.subscribe();
        Some(tokio::spawn(writer(Arc::clone(self), path, wanted)))
    }
}

/// Write the newest store at each change; after a failure, again at the
/// next change or after the retry delay (60 s, doubling to 1 h:
/// `write_atomic` leaves a temp file behind when it fails).
async fn writer(sessions: Arc<Sessions>, path: PathBuf, mut wanted: watch::Receiver<u64>) {
    let mut retry: Option<Duration> = None;
    loop {
        match retry {
            None => {
                if wanted.changed().await.is_err() {
                    return;
                }
            }
            Some(delay) => {
                let _ = tokio::time::timeout(delay, wanted.changed()).await;
            }
        }
        let gen = *wanted.borrow_and_update();
        let snapshot = sessions.lock().clone();
        let bytes = snapshot.to_file(&sessions.epoch);
        let p = path.clone();
        let wrote = tokio::task::spawn_blocking(move || paths::write_atomic(&p, &bytes))
            .await
            .unwrap_or_else(|e| Err(io::Error::other(e)));
        match wrote {
            Ok(()) => {
                sessions.lock().written(&snapshot);
                retry = None;
            }
            Err(e) => {
                tracing::warn!(file = %path.display(), error = %e, "web: cannot write the sessions file");
                retry = Some(next_retry(retry));
            }
        }
        sessions.tried.send_modify(|t| *t = (*t).max(gen));
    }
}

/// While a tab's WebSocket is open its session counts as used: the page
/// talks over the socket only after it loads. Marks it every
/// [`MARK_EVERY`]; stops when the session is gone or the tab drops this.
pub(crate) struct OpenTab(tokio::task::JoinHandle<()>);

impl OpenTab {
    /// Mark `hash`'s session used every [`MARK_EVERY`], at `clock()` (Unix
    /// seconds).
    pub(crate) fn start(sessions: Arc<Sessions>, hash: Hash, clock: fn() -> u64) -> OpenTab {
        OpenTab(tokio::spawn(async move {
            let first = tokio::time::Instant::now() + MARK_EVERY;
            let mut every = tokio::time::interval_at(first, MARK_EVERY);
            loop {
                every.tick().await;
                if !sessions.mark(&hash, clock()) {
                    return;
                }
            }
        }))
    }
}

impl Drop for OpenTab {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// The next retry delay after a failed write.
fn next_retry(retry: Option<Duration>) -> Duration {
    retry.map_or(RETRY_FIRST, |r| (r * 2).min(RETRY_MAX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    const DAY: u64 = 24 * 60 * 60;
    const T0: u64 = 1_800_000_000;

    /// A cookie value: 64 lowercase hex from `n`.
    fn value(n: u64) -> String {
        format!("{n:064x}")
    }

    fn login(s: &mut Store, n: u64, now: u64) -> String {
        let v = value(n);
        s.login(hash_of(&v).unwrap(), None, now);
        v
    }

    /// Whether `v` names a live session at `now` (a use).
    fn used(s: &mut Store, v: &str, now: u64) -> bool {
        hash_of(v).is_some_and(|h| s.use_hash(&h, now).is_some())
    }

    #[test]
    fn only_64_lowercase_hex_is_hashed() {
        assert!(hash_of(&value(1)).is_some());
        for bad in ["", "ab", &"A".repeat(64), &"g".repeat(64), &"a".repeat(65)] {
            assert_eq!(hash_of(bad), None, "{bad}");
        }
        let mut s = Store::default();
        let v = login(&mut s, 0xabc, T0);
        assert!(!used(&mut s, &v.to_uppercase(), T0));
        assert!(used(&mut s, &v, T0));
    }

    #[test]
    fn a_session_lives_30_days_from_its_last_use_on_wall_clock_time() {
        let mut s = Store::default();
        let v = login(&mut s, 1, T0);
        assert!(used(&mut s, &v, T0 + IDLE - 1), "live at 30 days - 1 s");
        assert!(
            used(&mut s, &v, T0 + IDLE - 1 + IDLE - 1),
            "a use refreshes it"
        );
        let mut s = Store::default();
        let v = login(&mut s, 1, T0);
        assert!(!used(&mut s, &v, T0 + IDLE), "gone at 30 days");
        assert_eq!(IDLE, 30 * DAY);
    }

    #[test]
    fn a_seen_later_than_now_counts_as_now() {
        let mut s = Store::default();
        // The clock ran 10 days ahead at the login, then was corrected.
        let v = login(&mut s, 1, T0 + 10 * DAY);
        assert!(used(&mut s, &v, T0), "pulled back to now");
        assert!(!used(&mut s, &v, T0 + IDLE), "and idle from there");
    }

    #[test]
    fn a_65th_login_drops_the_least_recently_seen() {
        let mut s = Store::default();
        let first = login(&mut s, 0, T0);
        let second = login(&mut s, 1, T0 + 1);
        assert!(used(&mut s, &first, T0 + 2), "first used last");
        for n in 2..MAX as u64 {
            login(&mut s, n, T0 + 2 + n);
        }
        assert_eq!(s.entries.len(), MAX);
        login(&mut s, 999, T0 + 100);
        assert_eq!(s.entries.len(), MAX);
        assert!(
            !used(&mut s, &second, T0 + 101),
            "the least recently seen went"
        );
        assert!(used(&mut s, &first, T0 + 101));
    }

    #[test]
    fn a_login_with_a_live_cookie_replaces_its_session() {
        let mut s = Store::default();
        let old = login(&mut s, 1, T0);
        let new = value(2);
        s.login(hash_of(&new).unwrap(), Some(&old), T0 + 1);
        assert_eq!(s.entries.len(), 1);
        assert!(!used(&mut s, &old, T0 + 2));
        assert!(used(&mut s, &new, T0 + 2));
    }

    #[test]
    fn a_sign_out_ends_only_its_session() {
        let mut s = Store::default();
        let (a, b) = (login(&mut s, 1, T0), login(&mut s, 2, T0));
        s.remove(&a);
        s.remove("not a cookie");
        assert!(!used(&mut s, &a, T0) && used(&mut s, &b, T0));
    }
    // ── The file, the epoch, the writer ────────────────────────────────────

    /// A scratch dir, removed on drop.
    struct Dir(PathBuf);

    impl Dir {
        fn new() -> Dir {
            let d = paths::socket_test_dir("web-sessions");
            std::fs::create_dir_all(&d).unwrap();
            Dir(d)
        }
        fn file(&self) -> PathBuf {
            self.0.join("web-sessions.json")
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn token() -> String {
        "ab".repeat(32)
    }

    fn ep() -> String {
        epoch(Mode::Local, &token(), b"")
    }

    #[test]
    fn a_store_round_trips_through_a_private_file_of_hashes() {
        let dir = Dir::new();
        let mut s = Store::default();
        let (a, b) = (login(&mut s, 0xabc, T0), login(&mut s, 0xdef, T0 + 5));
        paths::write_atomic(&dir.file(), &s.to_file(&ep())).unwrap();
        let text = std::fs::read_to_string(dir.file()).unwrap();
        assert!(!text.contains(&a) && !text.contains(&b), "no cookie value");
        assert!(!text.contains(&token()), "no token");
        assert!(text.contains(&hex(&hash_of(&a).unwrap())));
        let mode = std::fs::metadata(dir.file()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let (mut back, why) = load(&dir.file(), &ep(), T0 + 10);
        assert_eq!(why, None);
        assert_eq!(back.entries.len(), 2);
        assert!(used(&mut back, &a, T0 + 10) && used(&mut back, &b, T0 + 10));
    }

    #[test]
    fn the_epoch_binds_the_mode_the_token_and_the_password() {
        let base = epoch(Mode::Local, &token(), b"pw");
        assert!(base.len() == 64 && token_ok(&base), "{base}");
        let others = [
            epoch(Mode::Local, &"cd".repeat(32), b"pw"),
            epoch(Mode::Local, &token(), b"pw2"),
            epoch(Mode::Remote, &token(), b"pw"),
        ];
        let dir = Dir::new();
        let mut s = Store::default();
        login(&mut s, 1, T0);
        paths::write_atomic(&dir.file(), &s.to_file(&base)).unwrap();
        for other in others {
            assert_ne!(other, base);
            assert_eq!(load(&dir.file(), &other, T0), (Store::default(), None));
        }
        assert_eq!(load(&dir.file(), &base, T0).0.entries.len(), 1);
        // The separators keep the parts apart.
        assert_ne!(
            epoch(Mode::Local, &token(), b""),
            epoch(Mode::Local, &token(), b"\0")
        );
    }

    #[test]
    fn a_missing_file_is_an_empty_store_and_a_bad_one_says_why() {
        let dir = Dir::new();
        assert_eq!(load(&dir.file(), &ep(), T0), (Store::default(), None));
        let h = hex(&hash_of(&value(1)).unwrap());
        let with = |sessions: &str| {
            format!(
                r#"{{"version":1,"epoch":"{}","sessions":[{sessions}]}}"#,
                ep()
            )
        };
        let entry = |hash: &str| format!(r#"{{"hash":"{hash}","seen":{T0}}}"#);
        let cases = [
            (b"{not json".to_vec(), "not a sessions file"),
            (
                format!(r#"{{"version":2,"epoch":"{}","sessions":[]}}"#, ep()).into_bytes(),
                "version 2 is not 1",
            ),
            (with(&entry("xyz")).into_bytes(), "a malformed hash"),
            (
                with(&entry(&h.to_uppercase())).into_bytes(),
                "a malformed hash",
            ),
            (
                with(&format!("{},{}", entry(&h), entry(&h))).into_bytes(),
                "a hash twice",
            ),
            (vec![b' '; 64 * 1024 + 1], "over 64 KiB"),
        ];
        for (bytes, why) in cases {
            paths::write_atomic(&dir.file(), &bytes).unwrap();
            let (store, got) = load(&dir.file(), &ep(), T0);
            assert_eq!((store, got.as_deref()), (Store::default(), Some(why)));
        }
        paths::write_atomic(&dir.file(), with(&entry(&h)).as_bytes()).unwrap();
        assert_eq!(load(&dir.file(), &ep(), T0).0.entries.len(), 1);
        let link = dir.0.join("link.json");
        std::os::unix::fs::symlink(dir.file(), &link).unwrap();
        assert_eq!(load(&link, &ep(), T0).1.as_deref(), Some("a symlink"));
        let open = std::fs::Permissions::from_mode(0o644);
        std::fs::set_permissions(dir.file(), open).unwrap();
        let why = load(&dir.file(), &ep(), T0).1;
        assert_eq!(
            why.as_deref(),
            Some("not a private (0600) regular file of yours")
        );
    }

    #[test]
    fn a_file_of_70_keeps_the_64_most_recently_seen() {
        let dir = Dir::new();
        let mut s = Store::default();
        let values: Vec<String> = (0..70).map(|n| login(&mut s, n, T0 + n)).collect();
        // Past the cap of a running server: written as a file of 70.
        assert_eq!(s.entries.len(), MAX);
        s.entries.extend((0..6).map(|n| Entry {
            hash: hash_of(&values[n]).unwrap(),
            seen: T0 + n as u64,
            written: 0,
            asked: 0,
        }));
        paths::write_atomic(&dir.file(), &s.to_file(&ep())).unwrap();
        let (mut back, _) = load(&dir.file(), &ep(), T0 + 100);
        assert_eq!(back.entries.len(), MAX);
        assert!(!used(&mut back, &values[5], T0 + 100));
        assert!(used(&mut back, &values[6], T0 + 100));
    }

    #[test]
    fn steady_use_is_due_for_the_file_once_an_hour() {
        let mut s = Store::default();
        let v = login(&mut s, 1, T0);
        let hash = hash_of(&v).unwrap();
        // The login's write.
        let snapshot = s.clone();
        s.written(&snapshot);
        let mut due = Vec::new();
        for minute in 1..=180 {
            let now = T0 + 60 * minute;
            if s.use_hash(&hash, now) == Some(true) {
                due.push(now - T0);
                let snapshot = s.clone();
                s.written(&snapshot);
            }
        }
        assert_eq!(due, [3660, 7320]);
    }

    #[test]
    fn a_use_asks_for_the_file_once_an_hour_while_nothing_is_written() {
        let mut s = Store::default();
        let v = login(&mut s, 1, T0);
        let hash = hash_of(&v).unwrap();
        // The login's write failed: nothing calls `written`.
        let due: Vec<u64> = (1..=120)
            .filter(|m| s.use_hash(&hash, T0 + 60 * m) == Some(true))
            .collect();
        assert_eq!(due, [1, 62]);
    }

    #[tokio::test]
    async fn a_login_is_written_before_it_returns_and_with_the_start_s_epoch() {
        let dir = Dir::new();
        let s = Arc::new(Sessions::new(Store::default(), ep(), dir.file()));
        let writer = s.start_writer().unwrap();
        let v = value(1);
        let change = s.login(hash_of(&v).unwrap(), None, T0);
        // The token file changes on disk (a rotation); the epoch does not.
        std::fs::write(dir.0.join("web-token"), "cd".repeat(32)).unwrap();
        s.written(change).await;
        let text = std::fs::read_to_string(dir.file()).unwrap();
        assert!(text.contains(&hex(&hash_of(&v).unwrap())));
        assert!(text.contains(&ep()), "{text}");
        let change = s.sign_out(&v);
        s.written(change).await;
        let text = std::fs::read_to_string(dir.file()).unwrap();
        assert!(!text.contains(&hex(&hash_of(&v).unwrap())));
        writer.abort();
    }

    /// The marker's clock in the test below (its only user).
    static CLOCK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    fn clock() -> u64 {
        CLOCK.load(std::sync::atomic::Ordering::SeqCst)
    }

    #[tokio::test(start_paused = true)]
    async fn an_open_tab_keeps_its_session_alive() {
        use std::sync::atomic::Ordering::SeqCst;
        CLOCK.store(T0, SeqCst);
        let s = Arc::new(Sessions::memory());
        let (open, idle) = (value(1), value(2));
        s.login(hash_of(&open).unwrap(), None, T0);
        s.login(hash_of(&idle).unwrap(), None, T0);
        let tab = OpenTab::start(Arc::clone(&s), hash_of(&open).unwrap(), clock);
        for _ in 0..31 * 24 {
            CLOCK.fetch_add(60 * 60, SeqCst);
            tokio::time::advance(MARK_EVERY).await;
            tokio::task::yield_now().await;
        }
        assert_eq!(clock(), T0 + 31 * DAY);
        assert!(s.check(&open, clock()), "an open tab's session is live");
        assert!(!s.check(&idle, clock()), "the other idled out");
        drop(tab);
    }

    #[tokio::test]
    async fn a_failed_write_keeps_the_session_and_is_tried_again() {
        if paths::uid() == 0 {
            return; // root writes into a 0500 directory
        }
        let dir = Dir::new();
        let locked = dir.0.join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();
        let file = locked.join("web-sessions.json");
        let s = Arc::new(Sessions::new(Store::default(), ep(), file.clone()));
        let writer = s.start_writer().unwrap();
        let first = value(1);
        let change = s.login(hash_of(&first).unwrap(), None, T0);
        s.written(change).await;
        assert!(!file.exists());
        assert!(s.check(&first, T0 + 1), "kept in memory");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
        let second = value(2);
        let change = s.login(hash_of(&second).unwrap(), None, T0 + 2);
        s.written(change).await;
        let text = std::fs::read_to_string(&file).unwrap();
        for v in [&first, &second] {
            assert!(text.contains(&hex(&hash_of(v).unwrap())), "{text}");
        }
        writer.abort();
        let minutes = |m: u64| Duration::from_secs(60 * m);
        assert_eq!(next_retry(None), minutes(1));
        assert_eq!(next_retry(Some(minutes(1))), minutes(2));
        assert_eq!(next_retry(Some(minutes(40))), minutes(60));
        assert_eq!(next_retry(Some(minutes(60))), minutes(60));
    }
}
