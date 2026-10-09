//! Proxy status cache (S1 split from app.rs). Profile loading and env
//! building live in [`crate::proxy::resolve`], shared with `--plain`.

use std::collections::HashMap;
use std::time::Instant;

use super::stats_view::Window;
use crate::proxy::api::{ModelsResponse, PoolHealthResponse, StatsResponse};

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
