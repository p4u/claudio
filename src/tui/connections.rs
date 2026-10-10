//! Per-host connection state and reconnect backoff.
//!
//! Each host (including "local") has exactly one `HostConn` entry created
//! before the first connection attempt. A generation counter tags readers and
//! results so a stale `Disconnected` from a replaced connection cannot tear
//! down its replacement.

use std::collections::HashMap;
use std::time::Duration;

use crate::client::Client;

/// Initial reconnect delay.
pub const RECONNECT_INIT: Duration = Duration::from_secs(1);
/// Maximum reconnect delay.
pub const RECONNECT_MAX: Duration = Duration::from_secs(30);

/// Connection state for one host.
pub struct HostConn {
    pub client: Option<Client>,
    /// Current reconnect interval (exponential backoff with jitter).
    pub reconnect_delay: Duration,
    /// Generation: incremented each time we start a new connection attempt.
    /// Readers tag their messages with the generation at spawn time; a message
    /// from gen N is dropped if the current gen is > N (stale).
    pub generation: u64,
    /// True when the last failure was a bootstrap failure (remote binary
    /// install), so the next retry must re-run bootstrap.
    pub bootstrap_failed: bool,
}

impl HostConn {
    fn new() -> Self {
        HostConn {
            client: None,
            reconnect_delay: RECONNECT_INIT,
            generation: 0,
            bootstrap_failed: false,
        }
    }
}

/// All-hosts connection map. "local" is always present.
pub struct Connections {
    pub map: HashMap<String, HostConn>,
}

impl Connections {
    /// Start with only the local client connected.
    pub fn with_local(client: Client) -> Self {
        let mut map = HashMap::new();
        map.insert(
            "local".to_owned(),
            HostConn {
                client: Some(client),
                reconnect_delay: RECONNECT_INIT,
                generation: 0,
                bootstrap_failed: false,
            },
        );
        Connections { map }
    }

    /// Ensure a host entry exists (creates it if absent).
    /// Call this before scheduling the first connection attempt to a host,
    /// so that backoff state is tracked even before first success.
    pub fn ensure_host(&mut self, host: &str) {
        self.map
            .entry(host.to_owned())
            .or_insert_with(HostConn::new);
    }

    /// Increment and return the new generation for `host`.
    pub fn next_generation(&mut self, host: &str) -> u64 {
        let conn = self
            .map
            .entry(host.to_owned())
            .or_insert_with(HostConn::new);
        conn.generation += 1;
        conn.generation
    }

    /// Return the current generation for `host` (0 if never tracked).
    pub fn current_generation(&self, host: &str) -> u64 {
        self.map.get(host).map(|c| c.generation).unwrap_or(0)
    }

    pub fn client(&self, host: &str) -> Option<&Client> {
        self.map.get(host).and_then(|c| c.client.as_ref())
    }

    /// Record a successful connection.
    pub fn connected(&mut self, host: &str, client: Client) {
        let conn = self
            .map
            .entry(host.to_owned())
            .or_insert_with(HostConn::new);
        conn.client = Some(client);
        conn.reconnect_delay = RECONNECT_INIT;
        conn.bootstrap_failed = false;
    }

    /// Mark as disconnected (client dropped).
    pub fn disconnect(&mut self, host: &str) {
        if let Some(conn) = self.map.get_mut(host) {
            conn.client = None;
        }
    }

    /// True if the host has an active client.
    pub fn is_connected(&self, host: &str) -> bool {
        self.map.get(host).and_then(|c| c.client.as_ref()).is_some()
    }

    /// The current reconnect delay for `host`.
    fn reconnect_delay(&self, host: &str) -> Duration {
        self.map
            .get(host)
            .map(|c| c.reconnect_delay)
            .unwrap_or(RECONNECT_INIT)
    }

    /// Double the reconnect delay with ±20% jitter, capped at `RECONNECT_MAX`.
    fn bump_delay(&mut self, host: &str) {
        if let Some(conn) = self.map.get_mut(host) {
            let base = (conn.reconnect_delay * 2).min(RECONNECT_MAX);
            // Jitter: use low bits of the current nanosecond timestamp.
            let jitter_factor = jitter_pct();
            let base_ms = base.as_millis() as i64;
            let jitter_ms = base_ms * jitter_factor / 100;
            let total_ms = (base_ms + jitter_ms).max(100) as u64;
            conn.reconnect_delay = Duration::from_millis(total_ms);
        }
    }

    /// Schedule the next attempt to reach `host`: the delay to wait first (the
    /// backoff then grows), and the generation that attempt's events carry.
    pub fn next_attempt(&mut self, host: &str) -> (Duration, u64) {
        let delay = self.reconnect_delay(host);
        self.bump_delay(host);
        (delay, self.next_generation(host))
    }

    /// Reset the reconnect delay to the initial value on successful connect.
    pub fn reset_delay(&mut self, host: &str) {
        if let Some(conn) = self.map.get_mut(host) {
            conn.reconnect_delay = RECONNECT_INIT;
        }
    }

    /// Mark that the last failure was a bootstrap failure.
    pub fn set_bootstrap_failed(&mut self, host: &str, failed: bool) {
        if let Some(conn) = self.map.get_mut(host) {
            conn.bootstrap_failed = failed;
        }
    }

    /// Whether the last failure was a bootstrap failure.
    pub fn bootstrap_failed(&self, host: &str) -> bool {
        self.map
            .get(host)
            .map(|c| c.bootstrap_failed)
            .unwrap_or(false)
    }
}

/// Jitter percent in range −20..+20.
fn jitter_pct() -> i64 {
    let ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    // Map 0..41 range → -20..+20.
    (ns % 41) as i64 - 20
}
