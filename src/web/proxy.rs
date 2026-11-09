//! The proxy ops (`proxy.stats`, `proxy.credential`, `proxy.switch`) and
//! the proxy config cache spawns use. Every DTO is built field by field: no
//! `Profile`, token or proxy URL leaves this file, and an error names the
//! profile, never its URL (a transport error's text carries the URL).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::proxy::api::{
    self, ApiError, ConfigResponse, PoolHealthResponse, SessionCredential, StatsResponse,
    SwitchError, SwitchOutcome,
};
use crate::proxy::profile::Profile;
use crate::tui::stats_view::Window;

/// How long a profile's `/v1/claudio/config` is kept.
const CONFIG_TTL: Duration = Duration::from_secs(300);
/// The longest a config fetch may take.
const CONFIG_TIMEOUT: Duration = Duration::from_secs(5);

/// Each profile's `/v1/claudio/config`, as fetched last (not secret: model
/// names and limits).
pub(crate) struct ProxyCache {
    configs: Mutex<HashMap<String, (ConfigResponse, Instant)>>,
}

impl ProxyCache {
    pub(crate) fn new() -> ProxyCache {
        ProxyCache {
            configs: Mutex::new(HashMap::new()),
        }
    }

    /// `find_profile` in spawn_blocking; `"proxy profile '<name>' not found"`.
    pub(crate) async fn profile(name: &str) -> Result<Profile, String> {
        let owned = name.to_owned();
        tokio::task::spawn_blocking(move || crate::proxy::resolve::find_profile(&owned))
            .await
            .ok()
            .flatten()
            .ok_or_else(|| format!("proxy profile '{}' not found", clean(name)))
    }

    /// `/v1/claudio/config` of `name`, cached 300 s; a failure (5 s timeout) is `None`
    /// (session_env then uses its built-in model defaults, as the TUI).
    pub(crate) async fn config(&self, name: &str, profile: &Profile) -> Option<ConfigResponse> {
        {
            let configs = self.configs.lock().unwrap_or_else(|p| p.into_inner());
            if let Some((cfg, at)) = configs.get(name) {
                if at.elapsed() < CONFIG_TTL {
                    return Some(cfg.clone());
                }
            }
        }
        let fetch = api::fetch_config(&profile.url, &profile.token);
        let cfg = tokio::time::timeout(CONFIG_TIMEOUT, fetch)
            .await
            .ok()?
            .ok()
            .flatten()?;
        let mut configs = self.configs.lock().unwrap_or_else(|p| p.into_inner());
        configs.insert(name.to_owned(), (cfg.clone(), Instant::now()));
        Some(cfg)
    }
}

/// A profile name for a message: sanitized, ≤ 80 characters.
fn clean(name: &str) -> String {
    crate::text::sanitize(name).chars().take(80).collect()
}

/// "24h"|"7d"|"30d" via stats_view::Window::param, else "bad request: window".
pub(crate) fn window_param(window: &str) -> Result<&'static str, String> {
    Window::ALL
        .iter()
        .map(|w| w.param())
        .find(|p| *p == window)
        .ok_or_else(|| "bad request: window".to_owned())
}

/// A proxy API error for the browser: the status, never the URL.
fn api_error(e: ApiError) -> String {
    match e {
        ApiError::Http(s) if s.as_u16() == 401 => {
            "the proxy refused the token: run `claudio proxy login`".to_owned()
        }
        ApiError::Http(s) if s.as_u16() == 403 => {
            "stats need a user token (admin and anonymous tokens are refused)".to_owned()
        }
        ApiError::Http(s) if s.as_u16() == 404 => {
            "this proxy has no claudio API (upgrade claude-proxy)".to_owned()
        }
        ApiError::Http(s) if s.as_u16() == 429 => {
            "rate limited by the proxy; wait a few seconds".to_owned()
        }
        ApiError::Http(s) => format!("proxy returned {}", s.as_u16()),
        ApiError::Request(e) if e.is_timeout() => "the proxy did not answer in time".to_owned(),
        ApiError::Request(_) => "the proxy is unreachable".to_owned(),
    }
}

/// The `proxy.stats` reply (docs/web-protocol.md §5).
pub(crate) fn stats_dto(
    profile: &str,
    window: &str,
    s: &StatsResponse,
    pool: Option<&PoolHealthResponse>,
) -> Value {
    let t = &s.totals;
    let by_model: Vec<Value> = s
        .by_model
        .iter()
        .map(|m| {
            json!({
                "model": crate::text::sanitize(&m.model),
                "requests": m.requests,
                "errors": m.errors,
                "input_tokens": m.input_tokens,
                "output_tokens": m.output_tokens,
                "cache_read": m.cache_read,
                "cache_creation": m.cache_creation,
            })
        })
        .collect();
    let limit = s.limit.as_ref().map(|l| {
        json!({
            "output_tokens": l.output_tokens,
            "window_seconds": l.window_seconds,
            "used_output_tokens": l.used_output_tokens,
            "used_pct": l.used_pct,
            "blocked": l.blocked,
            "blocked_until": l.blocked_until.as_deref().map(crate::text::sanitize),
        })
    });
    let pool = pool.map(|p| {
        p.providers
            .iter()
            .map(|h| {
                json!({
                    "name": crate::text::sanitize(&h.name),
                    "status": crate::text::sanitize(&h.status),
                })
            })
            .collect::<Vec<Value>>()
    });
    json!({
        "profile": profile,
        "window": window,
        "user_name": crate::text::sanitize(&s.user_name),
        "totals": {
            "requests": t.requests,
            "errors": t.errors,
            "input_tokens": t.input_tokens,
            "output_tokens": t.output_tokens,
            "cache_read": t.cache_read,
            "cache_creation": t.cache_creation,
        },
        "by_model": by_model,
        "limit": limit,
        "pool": pool,
    })
}

/// A credential as the browser sees it: names and percentages, no ids.
pub(crate) fn credential_dto(c: &SessionCredential, now: u64) -> Value {
    json!({
        "name": c.name().map(crate::text::sanitize),
        "plan": c.plan().map(crate::text::sanitize),
        "provider": crate::text::sanitize(&c.credential.provider),
        "five_hour_pct": c.five_hour_pct(),
        "seven_day_pct": c.seven_day_pct(),
        "switched_secs_ago": c.recent_switch_age(now),
    })
}

fn switch_dto(o: &SwitchOutcome) -> Value {
    json!({
        "from": o.from.name().map(crate::text::sanitize),
        "to": o.to.as_ref().and_then(|t| t.name()).map(crate::text::sanitize),
        "state": crate::text::sanitize(&o.state),
    })
}

/// fetch_stats + fetch_pool_health (pool None on failure).
pub(crate) async fn stats(profile: &str, window: &str) -> Result<Value, String> {
    let param = window_param(window)?;
    let p = ProxyCache::profile(profile).await?;
    let s = api::fetch_stats(&p.url, &p.token, param)
        .await
        .map_err(api_error)?;
    let pool = api::fetch_pool_health(&p.url, &p.token).await.ok();
    Ok(stats_dto(&clean(profile), param, &s, pool.as_ref()))
}

pub(crate) async fn credential(profile: &str, claude_session_id: &str) -> Result<Value, String> {
    let p = ProxyCache::profile(profile).await?;
    let c = api::fetch_session(&p.url, &p.token, claude_session_id)
        .await
        .map_err(api_error)?;
    let credential = c.map(|c| credential_dto(&c, crate::paths::unix_now()));
    Ok(json!({ "credential": credential }))
}

pub(crate) async fn switch(profile: &str, claude_session_id: &str) -> Result<Value, String> {
    let p = ProxyCache::profile(profile).await?;
    match api::switch_session(&p.url, &p.token, claude_session_id).await {
        Ok(o) => Ok(switch_dto(&o)),
        Err(SwitchError::Api(e)) => Err(api_error(e)),
        Err(SwitchError::NoAlternative(why)) => {
            Err(crate::text::sanitize(&why).chars().take(300).collect())
        }
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::api::{
        CredentialInfo, CredentialUtilization, ProviderHealth, StatsModel, StatsTotals,
    };

    fn keys(v: &Value) -> Vec<String> {
        let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
        k.sort();
        k
    }

    #[test]
    fn windows_are_checked() {
        assert_eq!(window_param("24h"), Ok("24h"));
        assert_eq!(window_param("7d"), Ok("7d"));
        assert_eq!(window_param("30d"), Ok("30d"));
        for bad in ["1h", "", "24H"] {
            assert_eq!(window_param(bad), Err("bad request: window".to_owned()));
        }
    }

    #[test]
    fn stats_dto_has_the_documented_fields() {
        let s = StatsResponse {
            version: 1,
            user_name: "tester".into(),
            period: "7d".into(),
            totals: StatsTotals {
                requests: 12,
                output_tokens: 5600,
                ..Default::default()
            },
            by_model: vec![StatsModel {
                model: "claude-opus-5-5".into(),
                requests: 12,
                ..Default::default()
            }],
            limit: None,
        };
        let pool = PoolHealthResponse {
            version: 1,
            providers: vec![ProviderHealth {
                name: "anthropic".into(),
                status: "ok".into(),
            }],
        };
        let v = stats_dto("env", "7d", &s, Some(&pool));
        assert_eq!(
            keys(&v),
            [
                "by_model",
                "limit",
                "pool",
                "profile",
                "totals",
                "user_name",
                "window"
            ]
        );
        assert_eq!(
            keys(&v["totals"]),
            [
                "cache_creation",
                "cache_read",
                "errors",
                "input_tokens",
                "output_tokens",
                "requests"
            ]
        );
        assert_eq!(v["totals"]["output_tokens"], 5600);
        assert_eq!(v["by_model"][0]["model"], "claude-opus-5-5");
        assert_eq!(keys(&v["by_model"][0]).len(), 7);
        assert!(v["limit"].is_null());
        assert_eq!(v["pool"][0], json!({"name": "anthropic", "status": "ok"}));
        assert!(stats_dto("env", "7d", &s, None)["pool"].is_null());
    }

    #[test]
    fn credential_dto_never_has_a_token() {
        let c = SessionCredential {
            session_id: "conv-1".into(),
            credential: CredentialInfo {
                id: "cred-id-secretish".into(),
                label: "team max".into(),
                provider: "anthropic".into(),
                plan: "max".into(),
            },
            bound_at: None,
            last_seen: None,
            switched_at: None,
            utilization: Some(CredentialUtilization {
                five_hour_pct: Some(12.5),
                seven_day_pct: None,
                captured_at: None,
            }),
        };
        let v = credential_dto(&c, 0);
        assert_eq!(
            keys(&v),
            [
                "five_hour_pct",
                "name",
                "plan",
                "provider",
                "seven_day_pct",
                "switched_secs_ago"
            ]
        );
        assert_eq!(v["name"], "team max");
        assert_eq!(v["five_hour_pct"], 12.5);
        assert!(!v.to_string().contains("cred-id-secretish"));
        assert!(!v.to_string().contains("conv-1"));
    }

    #[test]
    fn api_errors_never_carry_the_url() {
        let e = api_error(ApiError::Http(reqwest::StatusCode::BAD_GATEWAY));
        assert_eq!(e, "proxy returned 502");
    }
}
