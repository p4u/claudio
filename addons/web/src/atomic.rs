//! Lock-free counter updates, written as compare-and-swap loops (portable
//! across toolchains: `fetch_update` is deprecated in newer ones).

use std::sync::atomic::{AtomicUsize, Ordering};

/// `counter -= n`, stopping at zero.
pub(crate) fn saturating_sub(counter: &AtomicUsize, n: usize) {
    let mut seen = counter.load(Ordering::Acquire);
    while let Err(now) = counter.compare_exchange_weak(
        seen,
        seen.saturating_sub(n),
        Ordering::AcqRel,
        Ordering::Acquire,
    ) {
        seen = now;
    }
}

/// `counter += 1` if it stays at most `max`; whether it did.
pub(crate) fn increment_below(counter: &AtomicUsize, max: usize) -> bool {
    let mut seen = counter.load(Ordering::Acquire);
    while seen < max {
        match counter.compare_exchange_weak(seen, seen + 1, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return true,
            Err(now) => seen = now,
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saturating_sub_stops_at_zero() {
        let c = AtomicUsize::new(5);
        saturating_sub(&c, 3);
        assert_eq!(c.load(Ordering::Relaxed), 2);
        saturating_sub(&c, 9);
        assert_eq!(c.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn increment_below_refuses_at_the_max() {
        let c = AtomicUsize::new(0);
        assert!(increment_below(&c, 2));
        assert!(increment_below(&c, 2));
        assert!(!increment_below(&c, 2));
        assert_eq!(c.load(Ordering::Relaxed), 2);
    }
}
