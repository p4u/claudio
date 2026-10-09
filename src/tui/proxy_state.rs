//! Proxy status cache (S1 split from app.rs). Profile loading and env
//! building live in [`crate::proxy::resolve`], shared with `--plain`.

use std::collections::HashMap;
use std::time::Instant;

use super::stats_view::Window;
use crate::proxy::api::{ModelsResponse, PoolHealthResponse, SessionCredential, StatsResponse};

/// Live proxy data per profile, shown in the stats popup (Alt+s).
#[derive(Debug, Clone, Default)]
pub struct ProxyStatus {
    pub pool: Option<PoolHealthResponse>,
    /// Stats per time window; only the windows fetched so far are present.
    pub stats: HashMap<Window, StatsResponse>,
    /// The model catalogue, fetched once per profile.
    pub models: Option<ModelsResponse>,
    /// The last fetch error, cleared by the next successful stats fetch.
    pub error: Option<String>,
    /// When the stats were last fetched.
    pub fetched_at: Option<Instant>,
}

impl ProxyStatus {
    /// Windows that have stats, in canonical order.
    pub fn cached_windows(&self) -> Vec<Window> {
        Window::ALL
            .into_iter()
            .filter(|w| self.stats.contains_key(w))
            .collect()
    }

    /// Merge the result of one fetch.
    pub fn apply(&mut self, fetch: ProxyFetch) {
        for (window, result) in fetch.stats {
            match result {
                Ok(stats) => {
                    self.stats.insert(window, stats);
                    self.error = None;
                }
                Err(e) => self.error = Some(e),
            }
        }
        if fetch.pool.is_some() {
            self.pool = fetch.pool;
        }
        if fetch.models.is_some() {
            self.models = fetch.models;
        }
        self.fetched_at = Some(Instant::now());
    }
}

/// The result of one `Effect::FetchProxyStats`.
#[derive(Debug, Default)]
pub struct ProxyFetch {
    /// One entry per requested window; errors are human-readable.
    pub stats: Vec<(Window, Result<StatsResponse, String>)>,
    pub pool: Option<PoolHealthResponse>,
    pub models: Option<ModelsResponse>,
}

/// How long a credential lookup stays fresh while its session is active.
pub const SESSION_CRED_POLL_SECS: u64 = 30;
/// Minimum gap between lookups triggered by activation or opening Alt+s; the
/// proxy rate-limits to 1 req/s, so tab-flipping must not hammer it.
pub const SESSION_CRED_MIN_GAP_SECS: u64 = 5;

/// The upstream credential claude-proxy last used for one claudio session.
#[derive(Debug, Clone)]
pub struct SessionCred {
    /// The claude session id the lookup was made for; a reply or a cache
    /// entry for another id is stale.
    pub claude_session_id: String,
    /// `None` until the proxy knows the conversation (404) or on an error
    /// before the first success.
    pub cred: Option<SessionCredential>,
    /// Unix seconds when the last lookup was requested.
    pub asked_at: u64,
}
