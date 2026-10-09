//! Proxy status cache and profile loading (S1 split from app.rs).

use std::collections::HashMap;
use std::time::Instant;

use super::stats_view::Window;
use crate::proxy::api::{ConfigResponse, ModelsResponse, PoolHealthResponse, StatsResponse};

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

/// Load proxy profiles from config.toml and CLAUDIO_PROXY_URL. Returns
/// `(profile_names, default_name)`. Never panics; returns empty on any error.
pub fn load_proxy_profiles() -> (Vec<String>, Option<String>) {
    // Check env var first — when set it overrides any saved default.
    if let Some((name, _)) = crate::proxy::profile::from_env() {
        // Return just the "env" profile, which is always the default.
        return (vec![name.to_owned()], Some(name.to_owned()));
    }
    match crate::proxy::profile::load() {
        Ok(sec) => {
            let names: Vec<String> = sec.profiles.keys().cloned().collect();
            (names, sec.default)
        }
        Err(_) => (Vec::new(), None),
    }
}

/// Resolve a proxy profile by name to `(url, token)`.
///
/// Handles `"env"` (CLAUDIO_PROXY_URL) and named profiles.
/// Returns `None` if the profile cannot be found.
pub fn resolve_profile(name: &str) -> Option<(String, String)> {
    if name == "env" {
        crate::proxy::profile::from_env().map(|(_, p)| (p.url, p.token))
    } else {
        crate::proxy::profile::load().ok().and_then(|sec| {
            sec.profiles
                .get(name)
                .map(|p| (p.url.clone(), p.token.clone()))
        })
    }
}

/// Build the `SpawnSpec.env` for a proxy profile name.
///
/// Returns `Err(msg)` if a profile name is given but cannot be resolved:
/// - deleted profile
/// - `"env"` profile but `CLAUDIO_PROXY_URL` is unset
///
/// Returns `Ok(vec![])` when `proxy_name` is `None`.
pub fn proxy_env_for(
    proxy_name: Option<&str>,
    proxy_config: Option<&ConfigResponse>,
) -> Result<Vec<(String, String)>, String> {
    let name = match proxy_name {
        Some(n) => n,
        None => return Ok(Vec::new()),
    };
    let profile = if name == "env" {
        crate::proxy::profile::from_env().map(|(_, p)| p)
    } else {
        crate::proxy::profile::load()
            .ok()
            .and_then(|sec| sec.profiles.get(name).cloned())
    };
    match profile {
        Some(p) => Ok(crate::proxy::env::session_env(&p, proxy_config)),
        None => {
            if name == "env" {
                Err("proxy profile 'env' requires CLAUDIO_PROXY_URL to be set".to_owned())
            } else {
                Err(format!(
                    "proxy profile '{name}' not found (was it deleted?)"
                ))
            }
        }
    }
}
