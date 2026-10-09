//! Proxy status cache (S1 split from app.rs). Profile loading and env
//! building live in [`crate::proxy::resolve`], shared with `--plain`.

use std::time::Instant;

use crate::proxy::api::{PoolHealthResponse, StatsResponse};

/// Live proxy data shown in the status bar for the active session.
#[derive(Debug, Clone, Default)]
pub struct ProxyStatus {
    pub pool: Option<PoolHealthResponse>,
    pub stats: Option<StatsResponse>,
    /// When the stats were last fetched.
    pub fetched_at: Option<Instant>,
}
