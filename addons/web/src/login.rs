//! `POST /api/login`: a token (local requests, or any without `--remote`).
//! Every attempt reserves its rate-limit slot before anything is compared
//! (`limit.rs`); the guard already checked Host and Origin, so only
//! same-origin attempts reach it.

use std::sync::Arc;
use std::time::Instant;

use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::auth::{self, Auth, Caller};

/// Exactly one of the two. Unknown fields are ignored, as before.
#[derive(Deserialize)]
struct LoginBody {
    token: Option<String>,
    password: Option<String>,
}

pub(crate) async fn login(State(auth): State<Arc<Auth>>, req: Request) -> Response {
    let caller = auth.caller(auth::peer_of(&req), auth::host_of(&req));
    let body = match axum::body::to_bytes(req.into_body(), auth::LOGIN_BODY_MAX).await {
        Ok(body) => body,
        Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
    };
    let status = match serde_json::from_slice(&body) {
        Ok(LoginBody {
            token: Some(token),
            password: None,
        }) => token_login(&auth, &caller, &token, Instant::now()),
        // No password login without --remote.
        _ => Err(StatusCode::BAD_REQUEST),
    };
    if let Err(status) = status {
        tracing::info!(
            status = status.as_u16(),
            local = caller.local,
            "web: login refused"
        );
        return status.into_response();
    }
    match auth.create_session(Instant::now()) {
        Ok(value) => {
            let mut res = StatusCode::NO_CONTENT.into_response();
            let cookie = auth::set_cookie(&value, auth.allowed.secure_cookie());
            res.headers_mut().insert(header::SET_COOKIE, cookie);
            res
        }
        Err(e) => {
            tracing::error!(error = %e, "web: no random bytes for a session");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// A token attempt: its slot first (local ones are exempt from the global
/// window), then the rule (with `--remote`, local requests only) and the
/// constant-time compare. A refused token counts like a wrong one.
fn token_login(auth: &Auth, caller: &Caller, token: &str, now: Instant) -> Result<(), StatusCode> {
    let slot = auth
        .limiter
        .reserve(caller.key, !caller.local, now)
        .map_err(|()| StatusCode::TOO_MANY_REQUESTS)?;
    if auth.token_allowed(caller) && auth.token_ok(token) {
        auth.limiter.release(slot);
        Ok(())
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{csp, Allowed};
    use crate::peer::PeerKey;
    use std::time::Duration;

    fn plain() -> Auth {
        let lo = std::net::SocketAddr::from(([127, 0, 0, 1], 7337));
        Auth::new("ab".repeat(32), Allowed::new(lo, &[]), csp(None)).unwrap()
    }

    const LOCAL: Caller = Caller {
        local: true,
        key: PeerKey::Loopback,
    };

    #[test]
    fn token_logins_lock_after_five_failures_for_a_minute() {
        let a = plain();
        let right = "ab".repeat(32);
        let t0 = Instant::now();
        for _ in 0..5 {
            assert_eq!(
                token_login(&a, &LOCAL, "wrong", t0),
                Err(StatusCode::UNAUTHORIZED)
            );
        }
        assert_eq!(
            token_login(&a, &LOCAL, "wrong", t0),
            Err(StatusCode::TOO_MANY_REQUESTS)
        );
        let t59 = t0 + Duration::from_secs(59);
        assert_eq!(
            token_login(&a, &LOCAL, &right, t59),
            Err(StatusCode::TOO_MANY_REQUESTS)
        );
        let t61 = t0 + Duration::from_secs(61);
        assert_eq!(token_login(&a, &LOCAL, &right, t61), Ok(()));
    }

    #[test]
    fn with_remote_a_non_local_token_is_refused_and_counts() {
        let any = std::net::SocketAddr::from(([0, 0, 0, 0], 7337));
        let a = Auth::new("ab".repeat(32), Allowed::remote(any, &[]), csp(None)).unwrap();
        let right = "ab".repeat(32);
        let remote = Caller {
            local: false,
            key: PeerKey::V4(1),
        };
        let t0 = Instant::now();
        for _ in 0..5 {
            assert_eq!(
                token_login(&a, &remote, &right, t0),
                Err(StatusCode::UNAUTHORIZED)
            );
        }
        assert_eq!(
            token_login(&a, &remote, &right, t0),
            Err(StatusCode::TOO_MANY_REQUESTS)
        );
        assert_eq!(token_login(&a, &LOCAL, &right, t0), Ok(()));
    }

    #[test]
    fn successes_do_not_count() {
        let a = plain();
        let t0 = Instant::now();
        for _ in 0..20 {
            assert_eq!(token_login(&a, &LOCAL, &"ab".repeat(32), t0), Ok(()));
        }
    }
}
