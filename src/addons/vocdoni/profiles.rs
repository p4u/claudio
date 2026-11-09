//! Where the Vocdoni addon reads its proxy profiles: config.toml's
//! `[proxy]` and `CLAUDIO_PROXY_URL` (the `env` profile, alone when set),
//! or, in tests, one file alone. Reads are blocking: callers run them off
//! the executor, or once at a front-end's start.

use std::collections::BTreeSet;
use std::path::PathBuf;

use super::proxy::profile::{self, Profile};
use super::proxy::resolve::{find_profile, load_proxy_profiles};

/// The profiles as the addon knows them: names only, never a token.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Profiles {
    /// Every profile's name (the env profile alone when it is set).
    pub names: Vec<String>,
    /// The default profile.
    pub default: Option<String>,
    /// The profiles marked `first_party = true`.
    pub first_party: BTreeSet<String>,
    /// Why they could not be read (config.toml unreadable or malformed),
    /// distinct from "no profiles": a launch that needs a profile, the
    /// default's included, fails with it (R20).
    pub error: Option<String>,
}

/// Where the profiles are read.
#[derive(Debug, Clone, Default)]
pub enum ProfileSource {
    /// config.toml and `CLAUDIO_PROXY_URL`.
    #[default]
    Config,
    /// This file's `[proxy]` alone (tests).
    // Built by the tests only (`Runner::with_files`).
    #[cfg_attr(not(test), allow(dead_code))]
    File(PathBuf),
}

impl ProfileSource {
    /// Every profile's name, the default and the first-party ones; on a
    /// read or parse error, none and the error.
    pub fn read(&self) -> Profiles {
        let section = match self {
            // The env profile alone, when it is set (never first-party).
            ProfileSource::Config if profile::from_env().is_some() => {
                let (names, default) = load_proxy_profiles();
                return Profiles {
                    names,
                    default,
                    ..Profiles::default()
                };
            }
            ProfileSource::Config => profile::load(),
            ProfileSource::File(path) => profile::load_from(path),
        };
        let sec = match section {
            Ok(sec) => sec,
            Err(e) => {
                return Profiles {
                    error: Some(unreadable(&e)),
                    ..Profiles::default()
                }
            }
        };
        Profiles {
            names: sec.profiles.keys().cloned().collect(),
            first_party: sec
                .profiles
                .iter()
                .filter(|(_, p)| p.first_party)
                .map(|(n, _)| n.clone())
                .collect(),
            default: sec.default,
            error: None,
        }
    }

    /// Profile `name` (`env`: `CLAUDIO_PROXY_URL`), if it exists; `Err`
    /// when the profiles cannot be read.
    pub fn find(&self, name: &str) -> Result<Option<Profile>, String> {
        let section = match self {
            ProfileSource::Config if name == "env" => return Ok(find_profile(name)),
            ProfileSource::Config => profile::load(),
            ProfileSource::File(path) => profile::load_from(path),
        };
        let mut sec = section.map_err(|e| unreadable(&e))?;
        Ok(sec.profiles.remove(name))
    }
}

/// The launch error of profiles that cannot be read (the error names a
/// location, never the file's text, which holds tokens).
fn unreadable(e: &std::io::Error) -> String {
    format!("cannot read the proxy profiles: {e}")
}

/// Why profile `name` cannot be used.
pub fn missing(name: &str) -> String {
    if name == "env" {
        "proxy profile 'env' requires CLAUDIO_PROXY_URL to be set".to_owned()
    } else {
        format!("proxy profile '{name}' not found (was it deleted?)")
    }
}
