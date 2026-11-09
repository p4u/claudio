//! Proxy stats data shared by the TUI and the web UI, and their fetch.

/// A stats time window, as accepted by `GET /v1/claudio/me/stats?period=`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Window {
    H24,
    D7,
    D30,
}

impl Window {
    pub const ALL: [Window; 3] = [Window::H24, Window::D7, Window::D30];

    /// The `?period=` query value.
    pub fn param(self) -> &'static str {
        match self {
            Window::H24 => "24h",
            Window::D7 => "7d",
            Window::D30 => "30d",
        }
    }

    /// Length of the window in days, for per-day rates.
    pub fn days(self) -> f64 {
        match self {
            Window::H24 => 1.0,
            Window::D7 => 7.0,
            Window::D30 => 30.0,
        }
    }
}

/// Fetch `profile`'s stats for `windows`, its pool health and (with
/// `models`) its model catalogue. The requests run one after another, well
/// inside the proxy's per-user rate limit (1 req/s, burst 10).
pub async fn fetch(
    profile: &super::profile::Profile,
    windows: &[Window],
    models: bool,
) -> crate::state::stats::ProxyFetch {
    use super::api;
    let (url, token) = (profile.url.as_str(), profile.token.as_str());
    let mut out = crate::state::stats::ProxyFetch::default();
    for w in windows {
        let result = api::fetch_stats(url, token, w.param())
            .await
            .map_err(describe);
        out.stats.push((*w, result));
    }
    out.pool = api::fetch_pool_health(url, token).await.ok();
    if models {
        out.models = api::fetch_models(url, token).await.ok();
    }
    out
}

/// A one-line description of a proxy API error, for the member.
pub fn describe(e: super::api::ApiError) -> String {
    use super::api::ApiError;
    match e {
        ApiError::Http(s) if s.as_u16() == 403 => {
            "stats need a user token (admin and anonymous tokens are refused)".to_owned()
        }
        ApiError::Http(s) if s.as_u16() == 404 => {
            "this proxy has no claudio API (upgrade claude-proxy)".to_owned()
        }
        ApiError::Http(s) if s.as_u16() == 429 => {
            "rate limited by the proxy; wait a few seconds".to_owned()
        }
        other => other.to_string(),
    }
}
