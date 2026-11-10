//! Sign-ins (docs/web.md, "Cookie"; docs/web-protocol.md §1): the
//! server-side sessions a `claudio_web` cookie names.
//!
//! [`Store`] is pure: each entry is the SHA-256 of a cookie value and its
//! last use in wall-clock Unix seconds; a session unused for 30 days is
//! gone, and past 64 the least recently used goes. `now` is a parameter
//! (A6). [`Sessions`] holds the store the server uses.

use std::sync::{Mutex, MutexGuard};

use sha2::{Digest, Sha256};

use crate::auth::{ct_eq, token_ok};

/// A session unused this long (seconds) is gone.
pub(crate) const IDLE: u64 = 30 * 24 * 60 * 60;
/// Sessions kept at most; past it the least recently used goes.
pub(crate) const MAX: usize = 64;

/// The SHA-256 of a cookie value.
pub(crate) type Hash = [u8; 32];

/// The SHA-256 of `value` when it is one this server could have made (64
/// lowercase hex); anything else is refused without hashing.
pub(crate) fn hash_of(value: &str) -> Option<Hash> {
    token_ok(value).then(|| Sha256::digest(value.as_bytes()).into())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    hash: Hash,
    /// Last use, Unix seconds.
    seen: u64,
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
        });
    }

    /// Whether `value` names a live session; a use: its `seen` becomes `now`.
    pub(crate) fn use_value(&mut self, value: &str, now: u64) -> bool {
        let Some(hash) = hash_of(value) else {
            return false;
        };
        self.tidy(now);
        match self.find(&hash) {
            Some(i) => {
                self.entries[i].seen = now;
                true
            }
            None => false,
        }
    }

    /// A sign-out: `value`'s session out.
    pub(crate) fn remove(&mut self, value: &str) {
        if let Some(i) = hash_of(value).and_then(|h| self.find(&h)) {
            self.entries.remove(i);
        }
    }
}

/// The store the server uses, shared by every request.
#[derive(Default)]
pub(crate) struct Sessions {
    store: Mutex<Store>,
}

impl Sessions {
    /// An empty store.
    pub(crate) fn memory() -> Sessions {
        Sessions::default()
    }

    /// Never held across an await.
    fn lock(&self) -> MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// [`Store::login`].
    pub(crate) fn login(&self, new: Hash, old: Option<&str>, now: u64) {
        self.lock().login(new, old, now);
    }

    /// [`Store::use_value`].
    pub(crate) fn check(&self, value: &str, now: u64) -> bool {
        self.lock().use_value(value, now)
    }

    /// [`Store::remove`].
    pub(crate) fn sign_out(&self, value: &str) {
        self.lock().remove(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        s.use_value(v, now)
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
}
