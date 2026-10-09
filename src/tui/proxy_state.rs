//! Proxy status cache and profile loading (S1 split from app.rs).

use std::time::Instant;

use crate::proxy::api::{ConfigResponse, PoolHealthResponse, StatsResponse};

/// Live proxy data shown in the status bar for the active session.
#[derive(Debug, Clone, Default)]
pub struct ProxyStatus {
    pub pool: Option<PoolHealthResponse>,
    pub stats: Option<StatsResponse>,
    /// When the stats were last fetched.
    pub fetched_at: Option<Instant>,
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
