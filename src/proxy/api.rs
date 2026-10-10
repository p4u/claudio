//! HTTP client for the `GET /v1/claudio/*` proxy endpoints.
//!
//! A short 5-second timeout is used on every request. All response structs
//! are lenient (`#[serde(default)]`) so an older proxy that omits new fields
//! still parses without error.
//!
//! The client is async (reqwest) and intended to be called from tokio tasks,
//! never blocking the UI event loop.

use std::time::Duration;

use reqwest::Client;
use serde::Deserialize;
use thiserror::Error;

const TIMEOUT: Duration = Duration::from_secs(5);

// ── Error type ────────────────────────────────────────────────────────────────

/// Errors returned by the proxy API client.
#[derive(Debug, Error)]
pub enum ApiError {
    /// Network or transport failure.
    #[error("request failed: {0}")]
    Request(#[from] reqwest::Error),
    /// The proxy returned an unexpected HTTP status.
    #[error("proxy returned {0}")]
    Http(reqwest::StatusCode),
}

// ── Response types ────────────────────────────────────────────────────────────

/// `GET /v1/claudio/config` — recommended env vars for spawning claude.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ConfigResponse {
    pub version: u32,
    pub env: std::collections::HashMap<String, String>,
    pub models_refreshed_at: Option<String>,
}

/// `GET /v1/claudio/me/stats?period=24h`
#[derive(Debug, Default, Deserialize, Clone)]
#[serde(default)]
pub struct StatsResponse {
    pub version: u32,
    pub user_name: String,
    pub period: String,
    pub totals: StatsTotals,
    pub by_model: Vec<StatsModel>,
    pub limit: Option<LimitInfo>,
}

#[derive(Debug, Default, Deserialize, Clone)]
#[serde(default)]
pub struct StatsTotals {
    pub requests: i64,
    pub errors: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read: i64,
    pub cache_creation: i64,
}

#[derive(Debug, Default, Deserialize, Clone)]
#[serde(default)]
pub struct StatsModel {
    pub model: String,
    pub requests: i64,
    pub errors: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read: i64,
    pub cache_creation: i64,
}

#[derive(Debug, Default, Deserialize, Clone)]
#[serde(default)]
pub struct LimitInfo {
    pub output_tokens: i64,
    pub window_seconds: i64,
    pub used_output_tokens: i64,
    pub used_pct: f64,
    pub blocked: bool,
    pub blocked_until: Option<String>,
}

/// `GET /v1/claudio/models` — the proxy's augmented model catalogue.
#[derive(Debug, Default, Deserialize, Clone)]
#[serde(default)]
pub struct ModelsResponse {
    pub version: u32,
    pub data: Vec<ModelEntry>,
    pub refreshed_at: Option<String>,
}

#[derive(Debug, Default, Deserialize, Clone)]
#[serde(default)]
pub struct ModelEntry {
    pub id: String,
    pub display_name: String,
    pub provider: String,
    /// `fable`, `opus`, `sonnet`, `haiku` or empty when unrecognised.
    pub family: String,
    /// 0 when the proxy does not know the context size.
    pub max_input_tokens: i64,
    /// The proxy's recommended default for this family.
    pub recommended_default: bool,
}

/// `GET /v1/claudio/pool/health`
#[derive(Debug, Default, Deserialize, Clone)]
#[serde(default)]
pub struct PoolHealthResponse {
    pub version: u32,
    pub providers: Vec<ProviderHealth>,
}

#[derive(Debug, Default, Deserialize, Clone)]
#[serde(default)]
pub struct ProviderHealth {
    pub name: String,
    pub status: String,
}

/// `GET /v1/claudio/session?id=<claude_session_id>` — the upstream credential
/// the proxy last used for one conversation.
#[derive(Debug, Default, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct SessionCredential {
    pub session_id: String,
    pub credential: CredentialInfo,
    pub bound_at: Option<String>,
    pub last_seen: Option<String>,
    /// When the proxy last moved the conversation to another credential.
    pub switched_at: Option<String>,
    pub utilization: Option<CredentialUtilization>,
}

/// How long after a switch the UI keeps saying so.
pub const SWITCH_NOTICE_SECS: u64 = 600;

impl SessionCredential {
    /// The credential's name for display: its label, else its id; `None`
    /// when the proxy sent neither.
    pub fn name(&self) -> Option<&str> {
        let c = &self.credential;
        [c.label.as_str(), c.id.as_str()]
            .into_iter()
            .find(|s| !s.is_empty())
    }

    /// The plan (`max`, `pro`…), if known.
    pub fn plan(&self) -> Option<&str> {
        Some(self.credential.plan.as_str()).filter(|p| !p.is_empty())
    }

    /// The 5-hour window utilization in percent, if reported.
    pub fn five_hour_pct(&self) -> Option<f64> {
        self.utilization.as_ref()?.five_hour_pct
    }

    /// The 7-day window utilization in percent, if reported.
    pub fn seven_day_pct(&self) -> Option<f64> {
        self.utilization.as_ref()?.seven_day_pct
    }

    /// Seconds since the proxy moved the conversation to this credential,
    /// while that is recent enough (see [`SWITCH_NOTICE_SECS`]) to mention.
    pub fn recent_switch_age(&self, now: u64) -> Option<u64> {
        let at = parse_rfc3339(self.switched_at.as_deref()?)?;
        Some(now.saturating_sub(at)).filter(|age| *age < SWITCH_NOTICE_SECS)
    }
}

#[derive(Debug, Default, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct CredentialInfo {
    pub id: String,
    pub label: String,
    pub provider: String,
    pub plan: String,
}

/// How much of the credential's rate-limit windows is used (0..=100).
#[derive(Debug, Default, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct CredentialUtilization {
    pub five_hour_pct: Option<f64>,
    pub seven_day_pct: Option<f64>,
    pub captured_at: Option<String>,
}

/// Parse an RFC 3339 timestamp (`2026-10-09T10:07:02Z`, with optional
/// fractional seconds and `±HH:MM` offset) to unix seconds.
pub fn parse_rfc3339(ts: &str) -> Option<u64> {
    let (date, rest) = ts.split_once(['T', 't', ' '])?;
    let mut d = date.split('-');
    let year: i64 = d.next()?.parse().ok()?;
    let month: i64 = d.next()?.parse().ok()?;
    let day: i64 = d.next()?.parse().ok()?;
    if d.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let (clock, offset) = match rest.find(['Z', 'z', '+', '-']) {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "Z"),
    };
    let mut c = clock.split(':');
    let hour: i64 = c.next()?.parse().ok()?;
    let min: i64 = c.next()?.parse().ok()?;
    let sec: i64 = c.next().unwrap_or("0").split('.').next()?.parse().ok()?;
    if hour > 23 || min > 59 || sec > 60 {
        return None;
    }
    let offset_secs: i64 = if offset.eq_ignore_ascii_case("z") {
        0
    } else {
        let sign = if offset.starts_with('-') { -1 } else { 1 };
        let (oh, om) = offset[1..].split_once(':')?;
        sign * (oh.parse::<i64>().ok()? * 3600 + om.parse::<i64>().ok()? * 60)
    };
    // Days since 1970-01-01 (Howard Hinnant's days-from-civil).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + hour * 3600 + min * 60 + sec - offset_secs;
    u64::try_from(secs).ok()
}

/// Overall pool status derived from individual providers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PoolStatus {
    Ok,
    Busy,
    Saturated,
    Unavailable,
    /// Could not reach the proxy.
    Unknown,
}

impl PoolStatus {
    pub fn label(&self) -> &'static str {
        match self {
            PoolStatus::Ok => "ok",
            PoolStatus::Busy => "busy",
            PoolStatus::Saturated => "saturated",
            PoolStatus::Unavailable => "unavailable",
            PoolStatus::Unknown => "?",
        }
    }
}

impl PoolHealthResponse {
    /// Aggregate all providers into one coarse status.
    pub fn overall(&self) -> PoolStatus {
        if self.providers.is_empty() {
            return PoolStatus::Unknown;
        }
        let statuses: Vec<&str> = self.providers.iter().map(|p| p.status.as_str()).collect();
        if statuses.iter().all(|s| *s == "saturated") {
            PoolStatus::Saturated
        } else if statuses.iter().any(|s| *s == "saturated" || *s == "busy") {
            PoolStatus::Busy
        } else if statuses.iter().all(|s| *s == "unavailable") {
            PoolStatus::Unavailable
        } else {
            PoolStatus::Ok
        }
    }
}

// ── HTTP client ───────────────────────────────────────────────────────────────

/// Build a reqwest Client with rustls-tls and our short timeout.
pub fn build_client() -> Result<Client, ApiError> {
    Ok(Client::builder().timeout(TIMEOUT).build()?)
}

/// `GET /v1/claudio` — check that the proxy speaks the Claudio API.
/// Returns `Ok(false)` when the endpoint returns 404 (older proxy without
/// Claudio API support). Returns `Err` on network/auth errors.
pub async fn check_root(base_url: &str, token: &str) -> Result<bool, ApiError> {
    let client = build_client()?;
    let resp = client
        .get(format!("{base_url}/v1/claudio"))
        .bearer_auth(token)
        .send()
        .await?;
    if resp.status().as_u16() == 404 {
        return Ok(false);
    }
    if !resp.status().is_success() {
        return Err(ApiError::Http(resp.status()));
    }
    Ok(true)
}

/// `GET /v1/models` — verify that a token is accepted (fallback when
/// `/v1/claudio` is not available). Requires a 2xx response; 404 is treated
/// as an auth failure (the endpoint is present but the token is rejected).
pub async fn check_models_fallback(base_url: &str, token: &str) -> Result<(), ApiError> {
    let client = build_client()?;
    let resp = client
        .get(format!("{base_url}/v1/models"))
        .bearer_auth(token)
        .send()
        .await?;
    if resp.status().is_success() {
        return Ok(());
    }
    Err(ApiError::Http(resp.status()))
}

/// `GET /v1/claudio/config` — fetch recommended env vars.
/// Returns `None` when the endpoint is not available (404), allowing
/// the caller to fall back to built-in defaults.
pub async fn fetch_config(base_url: &str, token: &str) -> Result<Option<ConfigResponse>, ApiError> {
    let client = build_client()?;
    let resp = client
        .get(format!("{base_url}/v1/claudio/config"))
        .bearer_auth(token)
        .send()
        .await?;
    if resp.status().as_u16() == 404 {
        return Ok(None);
    }
    if !resp.status().is_success() {
        return Err(ApiError::Http(resp.status()));
    }
    let cfg: ConfigResponse = resp.json().await?;
    Ok(Some(cfg))
}

/// `GET /v1/claudio/me/stats?period=<period>` — per-user statistics.
pub async fn fetch_stats(
    base_url: &str,
    token: &str,
    period: &str,
) -> Result<StatsResponse, ApiError> {
    let client = build_client()?;
    let resp = client
        .get(format!("{base_url}/v1/claudio/me/stats"))
        .query(&[("period", period)])
        .bearer_auth(token)
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(ApiError::Http(resp.status()));
    }
    Ok(resp.json().await?)
}

/// `GET /v1/claudio/pool/health` — pool availability.
pub async fn fetch_pool_health(
    base_url: &str,
    token: &str,
) -> Result<PoolHealthResponse, ApiError> {
    let client = build_client()?;
    let resp = client
        .get(format!("{base_url}/v1/claudio/pool/health"))
        .bearer_auth(token)
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(ApiError::Http(resp.status()));
    }
    Ok(resp.json().await?)
}

/// `GET /v1/claudio/models` — the augmented model catalogue.
pub async fn fetch_models(base_url: &str, token: &str) -> Result<ModelsResponse, ApiError> {
    let client = build_client()?;
    let resp = client
        .get(format!("{base_url}/v1/claudio/models"))
        .bearer_auth(token)
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(ApiError::Http(resp.status()));
    }
    Ok(resp.json().await?)
}

/// `GET /v1/claudio/session?id=<claude_session_id>` — the credential the proxy
/// last used for a conversation. `Ok(None)` for 404: unknown yet, another
/// user's session, a deleted credential, or a proxy without the endpoint.
pub async fn fetch_session(
    base_url: &str,
    token: &str,
    claude_session_id: &str,
) -> Result<Option<SessionCredential>, ApiError> {
    let client = build_client()?;
    let resp = client
        .get(format!("{base_url}/v1/claudio/session"))
        .query(&[("id", claude_session_id)])
        .bearer_auth(token)
        .send()
        .await?;
    if resp.status().as_u16() == 404 {
        return Ok(None);
    }
    if !resp.status().is_success() {
        return Err(ApiError::Http(resp.status()));
    }
    Ok(Some(resp.json().await?))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::get;
    use axum::{Json, Router};
    use std::net::SocketAddr;
    use tokio::net::TcpListener;

    /// Spin up a local axum server serving the given router. Returns the base URL.
    async fn serve(router: Router) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        format!("http://127.0.0.1:{}", addr.port())
    }

    #[tokio::test]
    async fn fetch_config_200() {
        let router = Router::new().route(
            "/v1/claudio/config",
            get(|| async {
                Json(serde_json::json!({
                    "version": 1,
                    "env": {
                        "ANTHROPIC_DEFAULT_SONNET_MODEL": "claude-sonnet-5-5[1m]",
                        "ANTHROPIC_DEFAULT_HAIKU_MODEL":  "claude-haiku-5-5[1m]"
                    }
                }))
            }),
        );
        let base = serve(router).await;
        let cfg = fetch_config(&base, "tok").await.unwrap().unwrap();
        assert_eq!(
            cfg.env
                .get("ANTHROPIC_DEFAULT_SONNET_MODEL")
                .map(String::as_str),
            Some("claude-sonnet-5-5[1m]")
        );
    }

    #[tokio::test]
    async fn fetch_config_404_returns_none() {
        // No route registered → axum returns 404.
        let router = Router::new();
        let base = serve(router).await;
        let result = fetch_config(&base, "tok").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn fetch_config_timeout() {
        // Bind a port, then close the listener — any connection attempt will
        // get ECONNREFUSED immediately (faster than a real timeout test).
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let base = format!("http://127.0.0.1:{}", addr.port());
        let result = fetch_config(&base, "tok").await;
        assert!(result.is_err(), "should fail on connection refused");
    }

    #[tokio::test]
    async fn pool_health_overall_status() {
        let ok = PoolHealthResponse {
            version: 1,
            providers: vec![
                ProviderHealth {
                    name: "anthropic".into(),
                    status: "ok".into(),
                },
                ProviderHealth {
                    name: "google".into(),
                    status: "ok".into(),
                },
            ],
        };
        assert_eq!(ok.overall(), PoolStatus::Ok);

        let busy = PoolHealthResponse {
            version: 1,
            providers: vec![
                ProviderHealth {
                    name: "anthropic".into(),
                    status: "busy".into(),
                },
                ProviderHealth {
                    name: "google".into(),
                    status: "ok".into(),
                },
            ],
        };
        assert_eq!(busy.overall(), PoolStatus::Busy);

        let saturated = PoolHealthResponse {
            version: 1,
            providers: vec![ProviderHealth {
                name: "anthropic".into(),
                status: "saturated".into(),
            }],
        };
        assert_eq!(saturated.overall(), PoolStatus::Saturated);
    }

    /// check_models_fallback must NOT accept 404 as success.
    #[tokio::test]
    async fn check_models_fallback_404_is_failure() {
        // No routes → 404 for /v1/models.
        let router = Router::new();
        let base = serve(router).await;
        let result = check_models_fallback(&base, "tok").await;
        assert!(
            result.is_err(),
            "404 from /v1/models must not count as auth success"
        );
    }

    #[tokio::test]
    async fn fetch_models_200() {
        let router = Router::new().route(
            "/v1/claudio/models",
            get(|| async {
                Json(serde_json::json!({
                    "version": 1,
                    "refreshed_at": "2026-10-09T10:00:00Z",
                    "data": [
                        {"id": "claude-opus-5-5[1m]", "display_name": "Claude Opus 5.5",
                         "provider": "anthropic", "family": "opus",
                         "max_input_tokens": 1000000, "recommended_default": true},
                        // An older proxy may omit optional fields entirely.
                        {"id": "claude-glm-5", "provider": "glm", "recommended_default": false}
                    ]
                }))
            }),
        );
        let base = serve(router).await;
        let models = fetch_models(&base, "tok").await.unwrap();
        assert_eq!(models.refreshed_at.as_deref(), Some("2026-10-09T10:00:00Z"));
        assert_eq!(models.data.len(), 2);
        assert_eq!(models.data[0].family, "opus");
        assert_eq!(models.data[0].max_input_tokens, 1_000_000);
        assert!(models.data[0].recommended_default);
        assert_eq!(models.data[1].family, "");
        assert_eq!(models.data[1].max_input_tokens, 0);
    }

    #[tokio::test]
    async fn fetch_models_403_is_error() {
        let router = Router::new().route(
            "/v1/claudio/models",
            get(|| async { (axum::http::StatusCode::FORBIDDEN, "nope") }),
        );
        let base = serve(router).await;
        assert!(matches!(
            fetch_models(&base, "tok").await,
            Err(ApiError::Http(s)) if s.as_u16() == 403
        ));
    }

    /// An old proxy that omits `limit`, `errors` and `by_model` still parses.
    #[test]
    fn stats_parses_with_missing_fields() {
        let s: StatsResponse = serde_json::from_str(
            r#"{"version":1,"user_name":"pau","period":"24h",
                "totals":{"requests":12,"input_tokens":100,"output_tokens":50}}"#,
        )
        .unwrap();
        assert_eq!(s.totals.requests, 12);
        assert_eq!(s.totals.errors, 0);
        assert_eq!(s.totals.cache_read, 0);
        assert!(s.by_model.is_empty());
        assert!(s.limit.is_none());
    }

    #[tokio::test]
    async fn check_models_fallback_200_is_success() {
        let router = Router::new().route(
            "/v1/models",
            get(|| async { Json(serde_json::json!({"models": []})) }),
        );
        let base = serve(router).await;
        assert!(check_models_fallback(&base, "tok").await.is_ok());
    }

    #[tokio::test]
    async fn fetch_session_200_full() {
        use axum::extract::Query;
        use axum::http::HeaderMap;
        let router = Router::new().route(
            "/v1/claudio/session",
            get(
                |Query(q): Query<std::collections::HashMap<String, String>>, h: HeaderMap| async move {
                    // The id and the bearer token must reach the proxy.
                    assert_eq!(q.get("id").map(String::as_str), Some("0a1b"));
                    assert_eq!(h["authorization"], "Bearer tok");
                    Json(serde_json::json!({
                        "session_id": "0a1b",
                        "credential": {"id": "cred_ab12", "label": "work-max",
                                       "provider": "anthropic", "plan": "max"},
                        "bound_at": "2026-10-09T10:00:00Z",
                        "last_seen": "2026-10-09T10:12:31Z",
                        "switched_at": "2026-10-09T10:07:02Z",
                        "utilization": {"five_hour_pct": 37.5, "seven_day_pct": 12,
                                        "captured_at": "2026-10-09T10:11:00Z"}
                    }))
                },
            ),
        );
        let base = serve(router).await;
        let s = fetch_session(&base, "tok", "0a1b").await.unwrap().unwrap();
        assert_eq!(s.credential.label, "work-max");
        assert_eq!(s.credential.plan, "max");
        assert_eq!(s.switched_at.as_deref(), Some("2026-10-09T10:07:02Z"));
        let u = s.utilization.unwrap();
        assert_eq!(u.five_hour_pct, Some(37.5));
        assert_eq!(u.seven_day_pct, Some(12.0));
    }

    #[tokio::test]
    async fn fetch_session_200_minimal() {
        let router = Router::new().route(
            "/v1/claudio/session",
            get(|| async {
                Json(serde_json::json!({
                    "session_id": "0a1b",
                    "credential": {"id": "cred_1", "label": "personal"}
                }))
            }),
        );
        let base = serve(router).await;
        let s = fetch_session(&base, "tok", "0a1b").await.unwrap().unwrap();
        assert_eq!(s.credential.label, "personal");
        assert_eq!(s.credential.plan, "");
        assert!(s.switched_at.is_none());
        assert!(s.utilization.is_none());
    }

    #[tokio::test]
    async fn fetch_session_404_is_none() {
        let base = serve(Router::new()).await;
        assert_eq!(fetch_session(&base, "tok", "0a1b").await.unwrap(), None);
    }

    #[tokio::test]
    async fn fetch_session_429_and_401_are_errors() {
        for code in [429u16, 401] {
            let router = Router::new().route(
                "/v1/claudio/session",
                get(move || async move {
                    (axum::http::StatusCode::from_u16(code).unwrap(), "no")
                }),
            );
            let base = serve(router).await;
            assert!(matches!(
                fetch_session(&base, "tok", "x").await,
                Err(ApiError::Http(s)) if s.as_u16() == code
            ));
        }
    }

    #[test]
    fn rfc3339_parses_to_unix_seconds() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("2026-10-09T10:07:02Z"), Some(1_791_540_422));
        assert_eq!(parse_rfc3339("2026-10-09T10:07:02.123456Z"), Some(1_791_540_422));
        // +02:00 is two hours ahead of UTC.
        assert_eq!(parse_rfc3339("2026-10-09T12:07:02+02:00"), Some(1_791_540_422));
        assert_eq!(parse_rfc3339("2026-10-09T05:37:02-04:30"), Some(1_791_540_422));
        assert_eq!(parse_rfc3339("2024-02-29T00:00:00Z"), Some(1_709_164_800));
        assert_eq!(parse_rfc3339("soon"), None);
        assert_eq!(parse_rfc3339("2026-13-09T10:07:02Z"), None);
    }
}
