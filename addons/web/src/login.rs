//! `POST /api/login`: a token or a sign-in ticket (local requests, or any
//! without `--remote`) or the password (`--remote`). Every attempt reserves
//! its rate-limit slot before anything is compared, looked up or hashed
//! (`limit.rs`); argon2 runs off the executor under `auth::HASH_PERMITS`
//! slots, a ticket's file checks in `spawn_blocking`. The guard already
//! checked Host and Origin, so only same-origin attempts reach it. Nothing a
//! body carries is ever logged.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::auth::{self, Auth, Caller};
use crate::ticket;
use claudio_core::paths;

/// How long a password login waits for a hashing slot before `503`.
pub(crate) const HASH_WAIT: Duration = Duration::from_secs(2);

/// Exactly one of the three. Unknown fields are ignored, as before.
#[derive(Deserialize)]
struct LoginBody {
    token: Option<String>,
    password: Option<String>,
    ticket: Option<String>,
}

pub(crate) async fn login(State(auth): State<Arc<Auth>>, req: Request) -> Response {
    let caller = auth.caller(auth::peer_of(&req), auth::host_of(&req));
    // A login with a cookie replaces that cookie's session.
    let old = auth::cookie_value(req.headers());
    let body = match axum::body::to_bytes(req.into_body(), auth::LOGIN_BODY_MAX).await {
        Ok(body) => body,
        Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
    };
    let status = match serde_json::from_slice(&body) {
        Ok(LoginBody {
            token: Some(token),
            password: None,
            ticket: None,
        }) => token_login(&auth, &caller, &token, Instant::now()),
        Ok(LoginBody {
            token: None,
            password: Some(pw),
            ticket: None,
        }) => password_login(&auth, &caller, pw, Instant::now()).await,
        Ok(LoginBody {
            token: None,
            password: None,
            ticket: Some(t),
        }) => {
            let (auth, runtime) = (Arc::clone(&auth), paths::runtime_dir());
            tokio::task::spawn_blocking(move || {
                ticket_login(&auth, &caller, &t, Instant::now(), &runtime)
            })
            .await
            .unwrap_or(Err(StatusCode::INTERNAL_SERVER_ERROR))
        }
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
    match auth.create_session(old.as_deref(), paths::unix_now()) {
        Ok((value, change)) => {
            // The cookie goes out once the sessions file has it (≤ 2 s).
            auth.sessions.written(change).await;
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

/// A ticket attempt (docs/web-protocol.md, "Sign-in tickets"): its slot as
/// a token attempt takes it, the token's rule (with `--remote`, local
/// requests only: a refused ticket's file is not touched), then
/// [`ticket::redeem`] under `runtime`. Every refusal keeps its slot.
/// Blocking: the login runs it in `spawn_blocking`.
fn ticket_login(
    auth: &Auth,
    caller: &Caller,
    t: &str,
    now: Instant,
    runtime: &Path,
) -> Result<(), StatusCode> {
    let slot = auth
        .limiter
        .reserve(caller.key, !caller.local, now)
        .map_err(|()| StatusCode::TOO_MANY_REQUESTS)?;
    if !auth.token_allowed(caller) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    match ticket::redeem(runtime, t, SystemTime::now()) {
        Ok(()) => {
            auth.limiter.release(slot);
            Ok(())
        }
        Err(ticket::Refused::Dir(why)) => {
            tracing::error!(error = %why, "web: the sign-in ticket directory failed its check");
            Err(StatusCode::UNAUTHORIZED)
        }
        Err(ticket::Refused::Invalid) => Err(StatusCode::UNAUTHORIZED),
    }
}

/// A password attempt: `400` without `--remote`; its slot (peer and global
/// windows) before any hashing; a hashing slot within [`HASH_WAIT`] or
/// `503` (the attempt is given back: it did not count); argon2 in
/// `spawn_blocking`, the permit held until it is done.
async fn password_login(
    auth: &Auth,
    caller: &Caller,
    pw: String,
    now: Instant,
) -> Result<(), StatusCode> {
    let Some(login) = auth.password.as_ref() else {
        return Err(StatusCode::BAD_REQUEST);
    };
    let slot = auth
        .limiter
        .reserve(caller.key, true, now)
        .map_err(|()| StatusCode::TOO_MANY_REQUESTS)?;
    let permit =
        match tokio::time::timeout(HASH_WAIT, Arc::clone(&login.hashing).acquire_owned()).await {
            Ok(Ok(permit)) => permit,
            _ => {
                auth.limiter.release(slot);
                return Err(StatusCode::SERVICE_UNAVAILABLE);
            }
        };
    let stored = login.stored.clone();
    let ok = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        stored.verify(&pw)
    })
    .await
    .unwrap_or(false);
    if ok {
        auth.limiter.release(slot);
        Ok(())
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{csp, Allowed, HASH_PERMITS};
    use crate::limit::GLOBAL;
    use crate::password;
    use crate::peer::PeerKey;
    use crate::sessions::Sessions;

    const FIXTURE: &str = include_str!("../web/e2e/fixtures/web-password");
    const TEST_PASSWORD: &str = "claudio-e2e-password";

    fn remote() -> Auth {
        let any = std::net::SocketAddr::from(([0, 0, 0, 0], 7337));
        Auth::new(
            "ab".repeat(32),
            Allowed::remote(any, &[]),
            csp(None),
            Sessions::memory(),
        )
        .unwrap()
        .with_password(password::Stored::parse(FIXTURE).unwrap())
    }

    const REMOTE: Caller = Caller {
        local: false,
        key: PeerKey::V4(1),
    };

    fn plain() -> Auth {
        let lo = std::net::SocketAddr::from(([127, 0, 0, 1], 7337));
        Auth::new(
            "ab".repeat(32),
            Allowed::new(lo, &[]),
            csp(None),
            Sessions::memory(),
        )
        .unwrap()
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
        let a = Auth::new(
            "ab".repeat(32),
            Allowed::remote(any, &[]),
            csp(None),
            Sessions::memory(),
        )
        .unwrap();
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

    #[tokio::test]
    async fn a_password_logs_in_and_a_wrong_one_counts() {
        let a = remote();
        let t0 = Instant::now();
        let pw = |s: &str| s.to_owned();
        assert_eq!(
            password_login(&a, &REMOTE, pw(TEST_PASSWORD), t0).await,
            Ok(())
        );
        for _ in 0..5 {
            assert_eq!(
                password_login(&a, &REMOTE, pw("not-the-password"), t0).await,
                Err(StatusCode::UNAUTHORIZED)
            );
        }
        for p in [TEST_PASSWORD, "not-the-password"] {
            assert_eq!(
                password_login(&a, &REMOTE, pw(p), t0).await,
                Err(StatusCode::TOO_MANY_REQUESTS)
            );
        }
    }

    #[tokio::test]
    async fn without_remote_a_password_body_is_400() {
        let a = plain();
        let t0 = Instant::now();
        let r = password_login(&a, &REMOTE, TEST_PASSWORD.to_owned(), t0).await;
        assert_eq!(r, Err(StatusCode::BAD_REQUEST));
        for _ in 0..5 {
            assert_eq!(
                token_login(&a, &LOCAL, "wrong", t0),
                Err(StatusCode::UNAUTHORIZED),
                "nothing was counted"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn no_hashing_slot_answers_503_and_does_not_count() {
        let a = remote();
        let hashing = Arc::clone(&a.password.as_ref().unwrap().hashing);
        let held = hashing
            .acquire_many_owned(HASH_PERMITS as u32)
            .await
            .unwrap();
        let t0 = Instant::now();
        for _ in 0..10 {
            assert_eq!(
                password_login(&a, &REMOTE, "wrong-password".to_owned(), t0).await,
                Err(StatusCode::SERVICE_UNAVAILABLE)
            );
        }
        drop(held);
        for _ in 0..5 {
            assert_eq!(
                password_login(&a, &REMOTE, "wrong-password".to_owned(), t0).await,
                Err(StatusCode::UNAUTHORIZED)
            );
        }
        assert_eq!(
            password_login(&a, &REMOTE, "wrong-password".to_owned(), t0).await,
            Err(StatusCode::TOO_MANY_REQUESTS),
            "the 503s took nothing"
        );
    }

    #[tokio::test]
    async fn the_global_window_stops_passwords_but_not_local_tokens() {
        let a = remote();
        let t0 = Instant::now();
        for i in 0..GLOBAL as u32 {
            a.limiter.reserve(PeerKey::V4(i), true, t0).unwrap();
        }
        let other = Caller {
            local: false,
            key: PeerKey::V4(999),
        };
        // Every hashing slot held: a 429 (not a 503) shows the refusal came
        // before any wait for one, let alone a hash.
        let hashing = Arc::clone(&a.password.as_ref().unwrap().hashing);
        let held = hashing
            .acquire_many_owned(HASH_PERMITS as u32)
            .await
            .unwrap();
        assert_eq!(
            password_login(&a, &other, TEST_PASSWORD.to_owned(), t0).await,
            Err(StatusCode::TOO_MANY_REQUESTS)
        );
        drop(held);
        assert_eq!(token_login(&a, &LOCAL, &"ab".repeat(32), t0), Ok(()));
    }

    /// A login's cookie is in the sessions file (as its hash) before the
    /// reply carries it, and a sign-out is out of it before its reply.
    #[tokio::test]
    async fn a_login_and_a_sign_out_are_written_before_their_replies() {
        use crate::sessions::{self, Mode, Store};
        use sha2::{Digest, Sha256};
        let dir = paths::socket_test_dir("web-login");
        let file = dir.join("web-sessions.json");
        let token = "ab".repeat(32);
        let epoch = sessions::epoch(Mode::Local, &token, b"");
        let lo = std::net::SocketAddr::from(([127, 0, 0, 1], 7337));
        let kept = Sessions::new(Store::default(), epoch, file.clone());
        let a = Arc::new(Auth::new(token.clone(), Allowed::new(lo, &[]), csp(None), kept).unwrap());
        let writer = a.sessions.start_writer().unwrap();
        let body = serde_json::json!({ "token": token }).to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/api/login")
            .body(axum::body::Body::from(body))
            .unwrap();
        let res = login(State(Arc::clone(&a)), req).await;
        assert_eq!(res.status(), StatusCode::NO_CONTENT);
        let set = res.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .to_owned();
        let value = set["claudio_web=".len()..]
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let hash: String = Sha256::digest(value.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains(&hash) && !text.contains(&value), "{text}");
        let mut headers = axum::http::HeaderMap::new();
        let cookie = format!("claudio_web={value}");
        headers.insert(header::COOKIE, cookie.parse().unwrap());
        let res = auth::logout(State(Arc::clone(&a)), headers).await;
        assert_eq!(res.status(), StatusCode::NO_CONTENT);
        assert!(!std::fs::read_to_string(&file).unwrap().contains(&hash));
        writer.abort();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A ticket redeems once from a local caller; a refused one counts like
    /// a wrong token; with `--remote` a non-local caller is refused before
    /// the file is looked at, so the same ticket still redeems locally.
    #[test]
    fn tickets_redeem_once_count_when_refused_and_stay_local() {
        let rt = std::env::temp_dir().join(format!("claudio-login-{}", uuid::Uuid::new_v4()));
        paths::ensure_private_dir(&rt).unwrap();
        let a = plain();
        let t0 = Instant::now();
        let t = ticket::mint(&rt, SystemTime::now()).unwrap();
        assert_eq!(ticket_login(&a, &LOCAL, &t, t0, &rt), Ok(()));
        for _ in 0..5 {
            assert_eq!(
                ticket_login(&a, &LOCAL, &t, t0, &rt),
                Err(StatusCode::UNAUTHORIZED)
            );
        }
        let fresh = ticket::mint(&rt, SystemTime::now()).unwrap();
        assert_eq!(
            ticket_login(&a, &LOCAL, &fresh, t0, &rt),
            Err(StatusCode::TOO_MANY_REQUESTS),
            "the replays counted"
        );
        let file = ticket::tickets_dir(&rt).join(ticket::file_name(&fresh));
        assert!(file.exists(), "a 429 leaves the ticket untouched");
        let r = remote();
        let t60 = t0 + Duration::from_secs(61);
        assert_eq!(
            ticket_login(&r, &REMOTE, &fresh, t60, &rt),
            Err(StatusCode::UNAUTHORIZED)
        );
        assert!(file.exists(), "a non-local refusal does not touch the file");
        assert_eq!(ticket_login(&r, &LOCAL, &fresh, t60, &rt), Ok(()));
        std::fs::remove_dir_all(&rt).unwrap();
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
