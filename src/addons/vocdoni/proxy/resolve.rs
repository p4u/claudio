//! The proxy profiles' names and lookup.

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
