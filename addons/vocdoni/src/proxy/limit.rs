//! The Vocdoni addon's `DaemonOps::on_usage_limit` (docs/addons.md, "The Vocdoni addon"):
//! a proxy session whose turn ended on a usage limit is moved to the proxy's next
//! subscription. The token comes from the session's spawn env and goes only to the
//! session's own proxy URL; it is never logged and never in an outcome (C1).

use std::sync::Arc;
use std::time::Duration;

use claudio_core::addon::{DaemonCtx, LimitOutcome, UsageLimit};

use super::api::{self, SwitchError};

/// The most one switch may take, the capability probe after a 404 included.
const SWITCH_TIMEOUT: Duration = Duration::from_secs(15);

/// Unless `[proxy] auto_switch` is set to anything but `true` on this host: `POST /v1/claudio/session/switch`
/// for the conversation with the session's own `ANTHROPIC_BASE_URL` and
/// `ANTHROPIC_AUTH_TOKEN`. A session without both is not a proxy session: `NotMine`.
pub async fn on_usage_limit(hit: UsageLimit<'_>, ctx: Arc<dyn DaemonCtx>) -> LimitOutcome {
    let proxy = ctx.config("proxy");
    // Absent is on; any present value but `true` is off, so a typo never leaves it on.
    if proxy
        .get("auto_switch")
        .is_some_and(|v| v.as_bool() != Some(true))
    {
        return LimitOutcome::NotMine;
    }
    let var = |name| (hit.env)(name).filter(|v| !v.is_empty());
    let (Some(url), Some(token)) = (var("ANTHROPIC_BASE_URL"), var("ANTHROPIC_AUTH_TOKEN")) else {
        return LimitOutcome::NotMine;
    };
    let switch = api::switch_session(&url, &token, hit.conversation);
    match tokio::time::timeout(SWITCH_TIMEOUT, switch).await {
        Ok(Ok(o)) => LimitOutcome::Switched {
            from: o.from.name().unwrap_or("current subscription").to_owned(),
            to: o
                .to
                .as_ref()
                .and_then(|t| t.name())
                .unwrap_or("next available")
                .to_owned(),
        },
        Ok(Err(SwitchError::UnknownSession)) => {
            LimitOutcome::Failed("the proxy does not know this session".to_owned())
        }
        Ok(Err(e)) => LimitOutcome::Failed(e.to_string()),
        Err(_) => LimitOutcome::Failed("the proxy did not answer within 15 s".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::Router;
    use claudio_core::addon::{BoxFuture, ClaudeFlags, ServiceState};
    use serde_json::json;

    use super::*;
    use crate::proxy::api::tests::{serve, switch_router, switch_router_with};

    /// A daemon context whose `config.toml` is this table.
    struct Ctx(toml::Table);

    impl DaemonCtx for Ctx {
        fn claude_bin(&self) -> PathBuf {
            PathBuf::from("claude")
        }
        fn path(&self) -> Option<OsString> {
            None
        }
        fn claude_flags(&self) -> BoxFuture<'static, ClaudeFlags> {
            Box::pin(async { ClaudeFlags::default() })
        }
        fn cache_dir(&self) -> PathBuf {
            std::env::temp_dir()
        }
        fn updating(&self) -> Arc<tokio::sync::Mutex<()>> {
            Arc::default()
        }
        fn service_reload(&self, _force: bool) -> BoxFuture<'static, ServiceState> {
            Box::pin(async { ServiceState::Off })
        }
        fn service_state(&self) -> ServiceState {
            ServiceState::Off
        }
        fn config(&self, section: &str) -> toml::Table {
            match self.0.get(section) {
                Some(toml::Value::Table(t)) => t.clone(),
                _ => toml::Table::new(),
            }
        }
    }

    /// `router`, counting the requests it gets.
    fn counted(router: Router) -> (Router, Arc<AtomicUsize>) {
        let hits = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&hits);
        let count = move |req: axum::extract::Request, next: axum::middleware::Next| {
            seen.fetch_add(1, Ordering::SeqCst);
            next.run(req)
        };
        (router.layer(axum::middleware::from_fn(count)), hits)
    }

    /// `on_usage_limit` for conversation `0a1b` of a session with `env`, on a host
    /// whose `config.toml` is `config`.
    async fn ask(env: &[(&str, &str)], config: &str) -> LimitOutcome {
        let env: Vec<(String, String)> = env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let lookup = move |name: &str| env.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());
        let hit = UsageLimit {
            conversation: "0a1b",
            message: "You've hit your 5-hour limit · resets 4pm (UTC)",
            env: &lookup,
        };
        on_usage_limit(hit, Arc::new(Ctx(config.parse().unwrap()))).await
    }

    fn proxied(base: &str) -> [(&str, &str); 2] {
        [
            ("ANTHROPIC_BASE_URL", base),
            ("ANTHROPIC_AUTH_TOKEN", "tok"),
        ]
    }

    /// AC3: 202 is `Switched` with the proxy's names; the request carries the
    /// session's conversation and token (`switch_router` checks both).
    #[tokio::test]
    async fn accepted_is_switched_with_the_proxys_names() {
        let body = json!({"session_id": "0a1b", "state": "pending",
            "from": {"id": "c1", "label": "work-max"}, "to": {"id": "c2", "label": "personal"}});
        let base = serve(switch_router(202, body)).await;
        let want = LimitOutcome::Switched {
            from: "work-max".into(),
            to: "personal".into(),
        };
        assert_eq!(ask(&proxied(&base), "").await, want);
        // No target chosen yet, and no names at all.
        let body = json!({"session_id": "0a1b", "from": {}, "to": null, "state": "pending"});
        let base = serve(switch_router(202, body)).await;
        let want = LimitOutcome::Switched {
            from: "current subscription".into(),
            to: "next available".into(),
        };
        assert_eq!(ask(&proxied(&base), "").await, want);
    }

    /// AC3: 409 fails with the proxy's own reason; 404 says the proxy does not
    /// know the session.
    #[tokio::test]
    async fn refused_fails_with_the_proxys_reason() {
        let body = json!({"error": {"type": "no_alternative",
            "message": "claudio: no other eligible credential can serve this session"}});
        let base = serve(switch_router(409, body)).await;
        let want = "no other eligible credential can serve this session";
        assert_eq!(
            ask(&proxied(&base), "").await,
            LimitOutcome::Failed(want.into())
        );
        let unknown = json!({"error": {"type": "not_found"}});
        let base = serve(switch_router_with(404, unknown, &["session_switch"])).await;
        let want = "the proxy does not know this session";
        assert_eq!(
            ask(&proxied(&base), "").await,
            LimitOutcome::Failed(want.into())
        );
    }

    /// AC3: `[proxy] auto_switch = false` is `NotMine` and sends nothing.
    #[tokio::test]
    async fn switched_off_is_not_mine_and_sends_nothing() {
        let body = json!({"session_id": "0a1b", "from": {"id": "c1"}, "state": "pending"});
        let (router, hits) = counted(switch_router(202, body));
        let base = serve(router).await;
        // Off is any present value but `true`: a typo must not leave it on.
        for off in ["false", "\"false\"", "0"] {
            let off = format!("[proxy]\nauto_switch = {off}\n");
            assert_eq!(ask(&proxied(&base), &off).await, LimitOutcome::NotMine);
        }
        assert_eq!(hits.load(Ordering::SeqCst), 0);
        let on = "[proxy]\nauto_switch = true\n";
        assert!(matches!(
            ask(&proxied(&base), on).await,
            LimitOutcome::Switched { .. }
        ));
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    /// AC3: a session without the proxy's base URL or token is `NotMine` and
    /// sends nothing.
    #[tokio::test]
    async fn no_proxy_env_is_not_mine_and_sends_nothing() {
        let body = json!({"session_id": "0a1b", "from": {"id": "c1"}, "state": "pending"});
        let (router, hits) = counted(switch_router(202, body));
        let base = serve(router).await;
        for env in [
            vec![("ANTHROPIC_AUTH_TOKEN", "tok")],
            vec![("ANTHROPIC_BASE_URL", base.as_str())],
            vec![
                ("ANTHROPIC_BASE_URL", base.as_str()),
                ("ANTHROPIC_AUTH_TOKEN", ""),
            ],
            vec![],
        ] {
            assert_eq!(ask(&env, "").await, LimitOutcome::NotMine, "{env:?}");
        }
        assert_eq!(hits.load(Ordering::SeqCst), 0);
    }
}
