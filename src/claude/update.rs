//! Keeping claude up to date: version comparison, the release-channel lookup
//! and the policy that turns "out of date" into prompt / update / nothing.
//!
//! Everything here is pure except [`check_due`], the once-a-day lookup of the
//! channel's latest version. The update itself runs in the daemon
//! (`daemon/update.rs`); the prompt lives in the TUI.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::config::UpdatePolicy;
use crate::paths;
use crate::upgrade::{http_client, SemVer};

/// Where claude publishes its release channels; `<base>/<channel>` answers
/// with a bare version such as `2.1.296`.
const RELEASES_URL: &str = "https://downloads.claude.ai/claude-code-releases";

const FETCH_TIMEOUT: Duration = Duration::from_secs(5);

/// Seconds between two lookups of the release channel.
const CHECK_INTERVAL: u64 = 86_400;

// ── Versions ──────────────────────────────────────────────────────────────────

/// Whether `have` is a strictly older version than `want`. Accepts claude's
/// `2.1.280 (Claude Code)` as well as a bare `2.1.296`; anything that does
/// not parse is never "older", so odd output never triggers an update.
pub fn is_older(have: &str, want: &str) -> bool {
    match (SemVer::parse(have), SemVer::parse(want)) {
        (Some(have), Some(want)) => have < want,
        _ => false,
    }
}

/// `2.1.280` from `2.1.280 (Claude Code)`.
pub fn short(version: &str) -> &str {
    version.split_whitespace().next().unwrap_or(version)
}

/// Whether a host whose claude reports `have` (`None`: not installed) needs
/// to move to `want`.
pub fn needs_update(have: Option<&str>, want: &str) -> bool {
    have.is_none_or(|have| is_older(have, want))
}

// ── Policy ────────────────────────────────────────────────────────────────────

/// What to do about an out-of-date claude.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Nothing,
    /// Ask the user first.
    Ask,
    /// Update right away (and say so).
    Run,
}

/// Apply `policy`. `skipped` is the user's "skip this version" for this host.
/// Installing claude where it is missing always needs a `y`, even under
/// `auto`.
pub fn decide(policy: UpdatePolicy, install: bool, skipped: bool) -> Decision {
    match policy {
        UpdatePolicy::Off => Decision::Nothing,
        UpdatePolicy::Auto if !install => Decision::Run,
        UpdatePolicy::Auto | UpdatePolicy::Ask if skipped => Decision::Nothing,
        UpdatePolicy::Auto | UpdatePolicy::Ask => Decision::Ask,
    }
}

// ── Release channel ───────────────────────────────────────────────────────────

/// claude's own `autoUpdatesChannel` from a `settings.json` body: `stable`,
/// else `latest`.
pub fn channel_from_settings(json: &str) -> &'static str {
    let channel = serde_json::from_str::<serde_json::Value>(json)
        .ok()
        .and_then(|v| v.get("autoUpdatesChannel")?.as_str().map(str::to_owned));
    match channel.as_deref() {
        Some("stable") => "stable",
        _ => "latest",
    }
}

fn channel() -> &'static str {
    std::fs::read_to_string(super::projects::config_dir().join("settings.json"))
        .map_or("latest", |json| channel_from_settings(&json))
}

/// The newest version on `channel`, from `<base>/<channel>`.
pub async fn fetch_latest(base: &str, channel: &str) -> Result<String, String> {
    let resp = http_client(FETCH_TIMEOUT)?
        .get(format!("{base}/{channel}"))
        .send()
        .await
        .map_err(|e| format!("HTTP request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("release server returned {}", resp.status()));
    }
    let body = resp.text().await.map_err(|e| format!("read failed: {e}"))?;
    let version = body.trim();
    SemVer::parse(version).ok_or_else(|| format!("not a version: {version:?}"))?;
    Ok(version.to_owned())
}

// ── Once-a-day check ──────────────────────────────────────────────────────────

/// `~/.config/claudio/claude-update.json`: when the channel was last asked.
/// (Separate from `update.json`, which is claudio's own release check.)
fn cache_path() -> PathBuf {
    paths::config_dir().join("claude-update.json")
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Cache {
    last_check: u64,
    channel: String,
}

impl Cache {
    fn load() -> Cache {
        std::fs::read(cache_path())
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    fn save(&self) {
        if let Ok(json) = serde_json::to_vec_pretty(self) {
            let _ = paths::write_atomic(&cache_path(), &json);
        }
    }

    fn is_fresh(&self, channel: &str, now: u64) -> bool {
        self.channel == channel && now.saturating_sub(self.last_check) < CHECK_INTERVAL
    }
}

/// The latest version on claude's release channel, at most once a day (and
/// never with `CLAUDIO_NO_UPDATE_CHECK=1`). `None` when a check is not due or
/// the network is unavailable (silently: a failed lookup is retried at the
/// next start).
pub async fn check_due() -> Option<String> {
    if std::env::var_os("CLAUDIO_NO_UPDATE_CHECK").is_some() {
        return None;
    }
    let channel = channel();
    if Cache::load().is_fresh(channel, paths::unix_now()) {
        return None;
    }
    let latest = fetch_latest(RELEASES_URL, channel).await.ok()?;
    Cache {
        last_check: paths::unix_now(),
        channel: channel.to_owned(),
    }
    .save();
    Some(latest)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions_numerically() {
        assert!(is_older("2.1.280 (Claude Code)", "2.1.296"));
        assert!(is_older("2.1.9", "2.1.10"), "numeric, not lexicographic");
        assert!(is_older("1.99.99", "2.0.0"));
        assert!(!is_older("2.1.296", "2.1.296 (Claude Code)"), "equal");
        assert!(!is_older("2.2.0", "2.1.296"));
        assert!(is_older("2.1", "2.1.1"), "a missing patch is 0");
    }

    #[test]
    fn odd_versions_never_look_older() {
        assert!(!is_older("", "2.1.296"));
        assert!(!is_older("garbage", "2.1.296"));
        assert!(!is_older("2.1.280", "garbage"));
        assert!(!is_older("2.1.280", ""));
        assert!(is_older("2.1.280-beta.1", "2.1.296"), "suffix ignored");
        assert!(is_older("v2.1.280", "2.1.296"));
    }

    #[test]
    fn short_drops_the_product_name() {
        assert_eq!(short("2.1.280 (Claude Code)"), "2.1.280");
        assert_eq!(short("2.1.280"), "2.1.280");
    }

    #[test]
    fn missing_claude_always_needs_an_install() {
        assert!(needs_update(None, "2.1.296"));
        assert!(needs_update(Some("2.1.280 (Claude Code)"), "2.1.296"));
        assert!(!needs_update(Some("2.1.296 (Claude Code)"), "2.1.296"));
        assert!(!needs_update(Some("3.0.0"), "2.1.296"), "newer is fine");
    }

    #[test]
    fn policy_decides_prompt_update_or_nothing() {
        use Decision as D;
        use UpdatePolicy as P;
        assert_eq!(decide(P::Ask, false, false), D::Ask);
        assert_eq!(decide(P::Ask, false, true), D::Nothing, "skipped version");
        assert_eq!(decide(P::Auto, false, false), D::Run);
        assert_eq!(decide(P::Auto, false, true), D::Run, "auto ignores skips");
        assert_eq!(decide(P::Off, false, false), D::Nothing);
        assert_eq!(decide(P::Off, true, false), D::Nothing);
        // Installing needs consent even under auto.
        assert_eq!(decide(P::Auto, true, false), D::Ask);
        assert_eq!(decide(P::Auto, true, true), D::Nothing);
        assert_eq!(decide(P::Ask, true, false), D::Ask);
    }

    #[test]
    fn channel_comes_from_claudes_settings() {
        assert_eq!(
            channel_from_settings(r#"{"autoUpdatesChannel":"stable"}"#),
            "stable"
        );
        assert_eq!(
            channel_from_settings(r#"{"autoUpdatesChannel":"latest"}"#),
            "latest"
        );
        assert_eq!(
            channel_from_settings(r#"{"autoUpdatesChannel":"../x"}"#),
            "latest"
        );
        assert_eq!(channel_from_settings(r#"{"model":"opus"}"#), "latest");
        assert_eq!(channel_from_settings("not json"), "latest");
        assert_eq!(channel_from_settings(""), "latest");
    }

    #[test]
    fn cache_is_fresh_for_a_day_per_channel() {
        let cache = Cache {
            last_check: 1_000,
            channel: "latest".into(),
        };
        assert!(cache.is_fresh("latest", 1_000 + CHECK_INTERVAL - 1));
        assert!(!cache.is_fresh("latest", 1_000 + CHECK_INTERVAL));
        assert!(!cache.is_fresh("stable", 1_001), "another channel");
        assert!(!Cache::default().is_fresh("latest", 5), "never checked");
    }

    #[tokio::test]
    async fn fetches_the_channel_from_a_mock_server() {
        use axum::routing::get;
        use axum::Router;

        let app = Router::new()
            .route("/latest", get(|| async { "2.1.296\n" }))
            .route("/stable", get(|| async { "<html>oops</html>" }));
        let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", tcp.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(tcp, app).await.unwrap() });

        assert_eq!(fetch_latest(&base, "latest").await.unwrap(), "2.1.296");
        assert!(
            fetch_latest(&base, "stable").await.is_err(),
            "not a version"
        );
        assert!(fetch_latest(&base, "nope").await.is_err(), "404");
    }
}
