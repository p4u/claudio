//! Choosing and resolving the proxy profile for a new claude session.
//!
//! The profiles' names and lookup, and (until the web launches through the
//! addon) a profile's session env.

use super::api::ConfigResponse;
use super::profile::{self, Profile};

/// Load proxy profiles from config.toml and CLAUDIO_PROXY_URL. Returns
/// `(profile_names, default_name)`. Never panics; returns empty on any error.
pub fn load_proxy_profiles() -> (Vec<String>, Option<String>) {
    // Check env var first — when set it overrides any saved default.
    if let Some((name, _)) = profile::from_env() {
        // Return just the "env" profile, which is always the default.
        return (vec![name.to_owned()], Some(name.to_owned()));
    }
    match profile::load() {
        Ok(sec) => {
            let names: Vec<String> = sec.profiles.keys().cloned().collect();
            (names, sec.default)
        }
        Err(_) => (Vec::new(), None),
    }
}

/// Look up a profile by name. Handles `"env"` (CLAUDIO_PROXY_URL) and named
/// profiles; `None` if it cannot be found.
pub fn find_profile(name: &str) -> Option<Profile> {
    if name == "env" {
        profile::from_env().map(|(_, p)| p)
    } else {
        profile::load()
            .ok()
            .and_then(|sec| sec.profiles.get(name).cloned())
    }
}

/// Resolve a proxy profile by name to `(url, token)`.
pub fn resolve_profile(name: &str) -> Option<(String, String)> {
    find_profile(name).map(|p| (p.url, p.token))
}

/// Build the `SpawnSpec.env` for a proxy profile name.
///
/// Returns `Err(msg)` if a profile name is given but cannot be resolved:
/// - deleted profile
/// - `"env"` profile but `CLAUDIO_PROXY_URL` is unset
///
/// Returns `Ok(vec![])` when `proxy_name` is `None`. `first_party` (the
/// `--first-party` flag) makes the session first-party whatever the profile
/// says.
pub fn proxy_env_for(
    proxy_name: Option<&str>,
    proxy_config: Option<&ConfigResponse>,
    first_party: bool,
) -> Result<Vec<(String, String)>, String> {
    let Some(name) = proxy_name else {
        return Ok(Vec::new());
    };
    match find_profile(name) {
        Some(mut p) => {
            p.first_party |= first_party;
            Ok(super::env::session_env(&p, proxy_config))
        }
        None if name == "env" => {
            Err("proxy profile 'env' requires CLAUDIO_PROXY_URL to be set".to_owned())
        }
        None => Err(format!(
            "proxy profile '{name}' not found (was it deleted?)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_proxy_name_means_empty_env() {
        assert_eq!(proxy_env_for(None, None, false), Ok(Vec::new()));
        assert_eq!(proxy_env_for(None, None, true), Ok(Vec::new()));
    }
}
