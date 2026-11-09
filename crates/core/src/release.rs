//! Release facts shared by `upgrade`, `remote::bootstrap`, `freshness` and
//! `claude::update`: the releases repository, asset naming, version comparison
//! and the HTTP client that talks to GitHub.

use std::time::Duration;

/// The public GitHub repo where release assets are published.
///
/// Shared by `upgrade`, `remote::bootstrap` and `claude::update`.
pub const RELEASES_REPO: &str = "p4u/claudio";

// ── Version comparison ────────────────────────────────────────────────────────

/// A parsed semantic version (major.minor.patch).  Anything that doesn't parse
/// cleanly is considered equal to 0.0.0 so it never triggers an upgrade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SemVer {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl SemVer {
    /// Parse a string like "1.2.3" or "v1.2.3".
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches('v');
        let mut parts = s.splitn(3, '.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        // Allow trailing pre-release suffixes on patch (e.g. "3-alpha").
        let patch_str = parts.next().unwrap_or("0");
        let patch: u32 = patch_str
            .split(|c: char| !c.is_ascii_digit())
            .next()
            .and_then(|p| p.parse().ok())
            .unwrap_or(0);
        Some(SemVer {
            major,
            minor,
            patch,
        })
    }

    pub fn zero() -> Self {
        SemVer {
            major: 0,
            minor: 0,
            patch: 0,
        }
    }
}

/// An HTTP client that identifies itself as claudio.
pub fn http_client(timeout: Duration) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(timeout)
        .user_agent(format!("claudio/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("cannot build HTTP client: {e}"))
}

/// Map probe OS name to release asset OS name.
///
/// The probe reports `"macos"` but release assets are named `claudio-darwin-*`.
pub fn release_os(probe_os: &str) -> Result<&str, String> {
    match probe_os {
        "linux" => Ok("linux"),
        "macos" => Ok("darwin"),
        other => Err(format!(
            "unsupported remote OS {other:?} — install claudio manually on the remote host"
        )),
    }
}

/// Map probe arch name to release asset arch name.
pub fn release_arch(probe_arch: &str) -> Result<&str, String> {
    match probe_arch {
        "x86_64" | "aarch64" => Ok(probe_arch),
        other => Err(format!(
            "unsupported remote arch {other:?} — install claudio manually on the remote host"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Version comparison ────────────────────────────────────────────────────

    #[test]
    fn semver_parse() {
        assert_eq!(
            SemVer::parse("1.2.3"),
            Some(SemVer {
                major: 1,
                minor: 2,
                patch: 3
            })
        );
        assert_eq!(
            SemVer::parse("v0.2.0"),
            Some(SemVer {
                major: 0,
                minor: 2,
                patch: 0
            })
        );
        assert_eq!(SemVer::parse("bad"), None);
        // Trailing pre-release suffix on patch.
        assert_eq!(
            SemVer::parse("1.2.3-alpha"),
            Some(SemVer {
                major: 1,
                minor: 2,
                patch: 3
            })
        );
    }

    #[test]
    fn semver_comparison() {
        let v020 = SemVer::parse("0.2.0").unwrap();
        let v030 = SemVer::parse("0.3.0").unwrap();
        let v100 = SemVer::parse("1.0.0").unwrap();
        assert!(v030 > v020);
        assert!(v100 > v030);
        assert_eq!(v020, v020);
    }

    #[test]
    fn release_os_maps_correctly() {
        assert_eq!(release_os("linux").unwrap(), "linux");
        assert_eq!(release_os("macos").unwrap(), "darwin");
        assert!(release_os("windows").is_err());
    }

    #[test]
    fn release_arch_maps_correctly() {
        assert_eq!(release_arch("x86_64").unwrap(), "x86_64");
        assert_eq!(release_arch("aarch64").unwrap(), "aarch64");
        assert!(release_arch("mips").is_err());
    }
}
