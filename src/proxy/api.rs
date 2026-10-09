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
pub async fn fetch_pool_health(base_url: &str, token: &str) -> Result<PoolHealthResponse, ApiError> {
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
        assert_eq!(cfg.env.get("ANTHROPIC_DEFAULT_SONNET_MODEL").map(String::as_str), Some("claude-sonnet-5-5[1m]"));
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
                ProviderHealth { name: "anthropic".into(), status: "ok".into() },
                ProviderHealth { name: "google".into(), status: "ok".into() },
            ],
        };
        assert_eq!(ok.overall(), PoolStatus::Ok);

        let busy = PoolHealthResponse {
            version: 1,
            providers: vec![
                ProviderHealth { name: "anthropic".into(), status: "busy".into() },
                ProviderHealth { name: "google".into(), status: "ok".into() },
            ],
        };
        assert_eq!(busy.overall(), PoolStatus::Busy);

        let saturated = PoolHealthResponse {
            version: 1,
            providers: vec![ProviderHealth { name: "anthropic".into(), status: "saturated".into() }],
        };
        assert_eq!(saturated.overall(), PoolStatus::Saturated);
    }

    /// Astra #19: check_models_fallback must NOT accept 404 as success.
    #[tokio::test]
    async fn check_models_fallback_404_is_failure() {
        // No routes → 404 for /v1/models.
        let router = Router::new();
        let base = serve(router).await;
        let result = check_models_fallback(&base, "tok").await;
        assert!(result.is_err(), "404 from /v1/models must not count as auth success");
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
}
