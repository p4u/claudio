//! The login rate limit. An attempt takes its slot **before** any check
//! (a reservation): concurrent attempts cannot all pass a "fewer than five"
//! test that none of them has failed yet. A success gives its slots back;
//! a failure keeps them for [`WINDOW`].

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::peer::PeerKey;

pub(crate) const WINDOW: Duration = Duration::from_secs(60);
/// Attempts per peer per window.
pub(crate) const PER_PEER: usize = 5;
/// Attempts per window over every peer (local token logins are exempt).
pub(crate) const GLOBAL: usize = 30;
/// Peers tracked at once; past it the one idle longest is forgotten.
pub(crate) const MAX_PEERS: usize = 1024;

/// A taken slot; give it to [`Limiter::release`] after a success.
#[derive(Debug)]
pub(crate) struct Reservation {
    key: PeerKey,
    id: u64,
    global: bool,
}

#[derive(Default)]
struct Inner {
    next_id: u64,
    peers: HashMap<PeerKey, VecDeque<(Instant, u64)>>,
    global: VecDeque<(Instant, u64)>,
}

#[derive(Default)]
pub(crate) struct Limiter {
    inner: Mutex<Inner>,
}

fn expire(q: &mut VecDeque<(Instant, u64)>, now: Instant) {
    while q
        .front()
        .is_some_and(|(t, _)| now.saturating_duration_since(*t) >= WINDOW)
    {
        q.pop_front();
    }
}

impl Limiter {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Take a slot for `key` (and a global one when `global`), or `Err(())`
    /// (answer 429) when either window is full. Nothing is taken on `Err`.
    pub(crate) fn reserve(
        &self,
        key: PeerKey,
        global: bool,
        now: Instant,
    ) -> Result<Reservation, ()> {
        let mut inner = self.lock();
        let inner = &mut *inner;
        expire(&mut inner.global, now);
        if global && inner.global.len() >= GLOBAL {
            return Err(());
        }
        if let Some(q) = inner.peers.get_mut(&key) {
            expire(q, now);
            if q.len() >= PER_PEER {
                return Err(());
            }
        } else if inner.peers.len() >= MAX_PEERS {
            inner.peers.retain(|_, q| {
                expire(q, now);
                !q.is_empty()
            });
            if inner.peers.len() >= MAX_PEERS {
                let idle = inner
                    .peers
                    .iter()
                    .min_by_key(|(_, q)| q.back().map(|(t, _)| *t))
                    .map(|(k, _)| *k);
                if let Some(k) = idle {
                    inner.peers.remove(&k);
                }
            }
        }
        inner.next_id += 1;
        let id = inner.next_id;
        inner.peers.entry(key).or_default().push_back((now, id));
        if global {
            inner.global.push_back((now, id));
        }
        Ok(Reservation { key, id, global })
    }

    /// Give back a successful attempt's slots.
    pub(crate) fn release(&self, r: Reservation) {
        let mut inner = self.lock();
        if let Some(q) = inner.peers.get_mut(&r.key) {
            q.retain(|(_, id)| *id != r.id);
            if q.is_empty() {
                inner.peers.remove(&r.key);
            }
        }
        if r.global {
            inner.global.retain(|(_, id)| *id != r.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: PeerKey = PeerKey::V4(0x0a00_0001);
    const B: PeerKey = PeerKey::V4(0x0a00_0002);

    #[test]
    fn a_peer_gets_five_attempts_a_minute_reserved_before_any_check() {
        let l = Limiter::default();
        let t0 = Instant::now();
        for _ in 0..PER_PEER {
            l.reserve(A, true, t0).unwrap();
        }
        assert!(
            l.reserve(A, true, t0).is_err(),
            "the sixth, before any of the five failed"
        );
        assert!(
            l.reserve(B, true, t0).is_ok(),
            "another peer is not affected"
        );
        assert!(l.reserve(A, true, t0 + Duration::from_secs(59)).is_err());
        assert!(l.reserve(A, true, t0 + WINDOW).is_ok(), "the window moved");
    }

    #[test]
    fn a_success_gives_its_slots_back() {
        let l = Limiter::default();
        let t0 = Instant::now();
        for _ in 0..PER_PEER - 1 {
            l.reserve(A, true, t0).unwrap();
        }
        let ok = l.reserve(A, true, t0).unwrap();
        l.release(ok);
        assert!(l.reserve(A, true, t0).is_ok());
        assert!(l.reserve(A, true, t0).is_err());
    }

    #[test]
    fn the_global_window_bounds_a_spread_attack_but_spares_exempt_attempts() {
        let l = Limiter::default();
        let t0 = Instant::now();
        for i in 0..GLOBAL as u32 {
            l.reserve(PeerKey::V4(i), true, t0).unwrap();
        }
        assert!(l.reserve(PeerKey::V4(9999), true, t0).is_err());
        assert!(
            l.reserve(PeerKey::Loopback, false, t0).is_ok(),
            "a local token login"
        );
        assert!(l.reserve(PeerKey::V4(9999), true, t0 + WINDOW).is_ok());
    }

    #[test]
    fn at_most_max_peers_are_tracked() {
        let l = Limiter::default();
        let t0 = Instant::now();
        for i in 0..(MAX_PEERS as u32 + 10) {
            let _ = l.reserve(PeerKey::V4(i), false, t0 + Duration::from_millis(i as u64));
        }
        assert!(l.lock().peers.len() <= MAX_PEERS);
    }

    #[test]
    fn concurrent_attempts_from_one_peer_take_at_most_five_slots() {
        let l = std::sync::Arc::new(Limiter::default());
        let t0 = Instant::now();
        let ok: usize = (0..20)
            .map(|_| {
                let l = std::sync::Arc::clone(&l);
                std::thread::spawn(move || l.reserve(A, true, t0).is_ok())
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|h| usize::from(h.join().unwrap()))
            .sum();
        assert_eq!(ok, PER_PEER);
    }
}
