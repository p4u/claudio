//! `claudio proxy login`'s browser sign-in: a device-style flow against the
//! proxy (`/v1/claudio/login/start`, `/v1/claudio/login/poll`).
//!
//! The proxy hands out a one-time `user_code` the member types on its
//! sign-in page (the URL never carries it) and a `device_code` only this
//! process holds; this side polls until the member approved, then gets the
//! member's own token. The device code lives only in memory: Ctrl+C
//! abandons the login.

use std::time::Duration;

use serde::Deserialize;

use super::api::build_client;

/// `POST /v1/claudio/login/start`.
#[derive(Clone, Deserialize)]
pub struct Start {
    pub device_code: String,
    pub user_code: String,
    pub verify_url: String,
    #[serde(default = "default_interval")]
    pub interval_s: u64,
    #[serde(default = "default_expiry")]
    pub expires_in_s: u64,
}

fn default_interval() -> u64 {
    3
}

fn default_expiry() -> u64 {
    600
}

impl std::fmt::Debug for Start {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Start")
            .field("device_code", &"[redacted]")
            .field("user_code", &self.user_code)
            .field("verify_url", &self.verify_url)
            .finish()
    }
}

/// What an approved login gives: the member's token.
#[derive(Clone, Deserialize)]
pub struct Granted {
    pub token: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub name: String,
}

impl std::fmt::Debug for Granted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Granted")
            .field("token", &"[redacted]")
            .field("email", &self.email)
            .field("name", &self.name)
            .finish()
    }
}

/// Why a browser login did not give a token.
#[derive(Debug, PartialEq)]
pub enum LoginError {
    /// `login/start` answered 429 `too_many_pending`.
    TooManyPending,
    /// The member (or an admin) said no, or the account is disabled.
    Denied,
    /// The code expired, or the proxy forgot it (restart, eviction).
    Expired,
    /// Any other status.
    Http(u16),
    /// The proxy could not be reached.
    Network(String),
}

impl std::fmt::Display for LoginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoginError::TooManyPending => write!(
                f,
                "too many pending logins from this address, try again in a minute"
            ),
            LoginError::Denied => write!(f, "sign-in denied"),
            LoginError::Expired => write!(f, "login expired, run again"),
            LoginError::Http(status) => write!(f, "the proxy answered {status}"),
            LoginError::Network(e) => write!(f, "could not reach the proxy: {e}"),
        }
    }
}

/// Start a login at `base_url`. `Ok(None)`: the proxy has no browser login
/// (404 or 401: an older proxy, or one without sign-in), so a token is
/// pasted.
pub async fn start(base_url: &str) -> Result<Option<Start>, LoginError> {
    let client = build_client().map_err(|e| LoginError::Network(e.to_string()))?;
    let resp = client
        .post(format!("{base_url}/v1/claudio/login/start"))
        .send()
        .await
        .map_err(|e| LoginError::Network(e.without_url().to_string()))?;
    match resp.status().as_u16() {
        200 => resp
            .json()
            .await
            .map(Some)
            .map_err(|_| LoginError::Http(200)),
        401 | 404 => Ok(None),
        429 => Err(LoginError::TooManyPending),
        other => Err(LoginError::Http(other)),
    }
}

/// Poll until the login of `start` is decided. `unit` is one second (tests
/// shrink it): the interval is `interval_s` units, doubled on each
/// `slow_down`, and the whole wait `expires_in_s` units. A network error is
/// retried until then.
pub async fn poll(base_url: &str, start: &Start, unit: Duration) -> Result<Granted, LoginError> {
    let client = build_client().map_err(|e| LoginError::Network(e.to_string()))?;
    let deadline = tokio::time::Instant::now() + unit * start.expires_in_s as u32;
    let mut interval = unit * start.interval_s.max(1) as u32;
    let body = serde_json::json!({ "device_code": start.device_code });
    loop {
        if tokio::time::Instant::now() + interval > deadline {
            return Err(LoginError::Expired);
        }
        tokio::time::sleep(interval).await;
        let sent = client
            .post(format!("{base_url}/v1/claudio/login/poll"))
            .json(&body)
            .send()
            .await;
        let Ok(resp) = sent else {
            continue;
        };
        match resp.status().as_u16() {
            200 => return resp.json().await.map_err(|_| LoginError::Http(200)),
            428 => {}
            429 => interval *= 2,
            403 => return Err(LoginError::Denied),
            404 | 410 => return Err(LoginError::Expired),
            500..=599 => {}
            other => return Err(LoginError::Http(other)),
        }
    }
}

/// Whether to open the sign-in page in a browser: there is one to show it
/// (a display on Linux and the BSDs, always on macOS), stdout is a terminal
/// (someone is there to use it), and `CLAUDIO_NO_BROWSER` is unset. The URL
/// is printed either way.
pub fn browser_wanted(os: &str, env: impl Fn(&str) -> Option<String>, tty: bool) -> bool {
    let set = |k: &str| env(k).is_some_and(|v| !v.is_empty());
    let display = os == "macos" || set("DISPLAY") || set("WAYLAND_DISPLAY");
    display && tty && env("CLAUDIO_NO_BROWSER").is_none()
}

/// Open `url` with `open` (macOS) or `xdg-open`, without waiting for it;
/// whether it could be started.
pub fn open_browser(url: &str) -> bool {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let child = std::process::Command::new(opener)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    match child {
        Ok(mut child) => {
            std::thread::spawn(move || child.wait());
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use axum::routing::post;
    use axum::Json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    const UNIT: Duration = Duration::from_millis(5);

    async fn serve(router: axum::Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        format!("http://127.0.0.1:{port}")
    }

    fn start_reply() -> serde_json::Value {
        serde_json::json!({"device_code": "dc-secret", "user_code": "K7PQ-3WXM",
            "verify_url": "https://p.example/claudio/login", "interval_s": 1, "expires_in_s": 45})
    }

    /// A proxy whose poll answers `script` in turn (then the last one
    /// forever), counting the polls.
    async fn proxy(script: Vec<(StatusCode, serde_json::Value)>) -> (String, Arc<AtomicUsize>) {
        let polls = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&polls);
        let script = Arc::new(script);
        let router = axum::Router::new()
            .route(
                "/v1/claudio/login/start",
                post(|| async { Json(start_reply()) }),
            )
            .route(
                "/v1/claudio/login/poll",
                post(move |Json(body): Json<serde_json::Value>| {
                    let n = counted.fetch_add(1, Ordering::SeqCst);
                    let script = Arc::clone(&script);
                    async move {
                        assert_eq!(body["device_code"], "dc-secret");
                        let (status, reply) = script[n.min(script.len() - 1)].clone();
                        (status, Json(reply))
                    }
                }),
            );
        (serve(router).await, polls)
    }

    fn error(code: &str) -> serde_json::Value {
        serde_json::json!({ "error": code })
    }

    async fn run(
        script: Vec<(StatusCode, serde_json::Value)>,
    ) -> (Result<Granted, LoginError>, usize) {
        let (base, polls) = proxy(script).await;
        let start = start(&base).await.unwrap().expect("browser login");
        assert_eq!(start.user_code, "K7PQ-3WXM");
        assert!(!format!("{start:?}").contains("dc-secret"));
        let result = poll(&base, &start, UNIT).await;
        (result, polls.load(Ordering::SeqCst))
    }

    #[tokio::test]
    async fn pending_then_approved_gives_the_token() {
        let granted =
            serde_json::json!({"token": "tok-1", "email": "pau@vocdoni.org", "name": "pau"});
        let (result, polls) = run(vec![
            (
                StatusCode::PRECONDITION_REQUIRED,
                error("authorization_pending"),
            ),
            (
                StatusCode::PRECONDITION_REQUIRED,
                error("authorization_pending"),
            ),
            (StatusCode::INTERNAL_SERVER_ERROR, error("server_error")),
            (StatusCode::OK, granted),
        ])
        .await;
        let granted = result.unwrap();
        assert_eq!(
            (granted.token.as_str(), granted.email.as_str()),
            ("tok-1", "pau@vocdoni.org")
        );
        assert!(!format!("{granted:?}").contains("tok-1"));
        assert_eq!(polls, 4);
    }

    #[tokio::test]
    async fn slow_down_doubles_the_interval() {
        // 45 units: 1 + 2 + 4 + 8 + 16 = 31 fit, the next 32 does not.
        let (result, polls) = run(vec![(StatusCode::TOO_MANY_REQUESTS, error("slow_down"))]).await;
        assert_eq!(result.unwrap_err(), LoginError::Expired);
        assert_eq!(polls, 5);
    }

    #[tokio::test]
    async fn expiry_denial_and_unknown_codes_end_the_login() {
        let pending = (
            StatusCode::PRECONDITION_REQUIRED,
            error("authorization_pending"),
        );
        let (result, polls) = run(vec![pending]).await;
        assert_eq!(result.unwrap_err(), LoginError::Expired, "the deadline");
        assert!(polls > 5, "{polls}");
        for (status, code, want) in [
            (StatusCode::GONE, "expired", LoginError::Expired),
            (StatusCode::NOT_FOUND, "unknown_code", LoginError::Expired),
            (StatusCode::FORBIDDEN, "denied", LoginError::Denied),
            (
                StatusCode::BAD_REQUEST,
                "invalid_request",
                LoginError::Http(400),
            ),
        ] {
            let (result, polls) = run(vec![(status, error(code))]).await;
            assert_eq!(result.unwrap_err(), want, "{status}");
            assert_eq!(polls, 1);
        }
    }

    #[tokio::test]
    async fn start_tells_an_old_proxy_and_too_many_pending() {
        let old = serve(axum::Router::new()).await;
        assert!(start(&old).await.unwrap().is_none(), "404: paste a token");
        // An older proxy without the sign-in's auth exemption asks for a token.
        let gated = axum::Router::new().route(
            "/v1/claudio/login/start",
            post(|| async { (StatusCode::UNAUTHORIZED, Json(error("unauthorized"))) }),
        );
        let gated = serve(gated).await;
        assert!(start(&gated).await.unwrap().is_none(), "401: paste a token");
        let busy = axum::Router::new().route(
            "/v1/claudio/login/start",
            post(|| async {
                (
                    StatusCode::TOO_MANY_REQUESTS,
                    Json(error("too_many_pending")),
                )
            }),
        );
        let busy = serve(busy).await;
        assert_eq!(start(&busy).await.unwrap_err(), LoginError::TooManyPending);
        assert_eq!(
            LoginError::TooManyPending.to_string(),
            "too many pending logins from this address, try again in a minute"
        );
    }

    #[test]
    fn the_browser_opens_only_with_a_display_a_tty_and_no_opt_out() {
        let env = |vars: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                vars.iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        assert!(browser_wanted("linux", env(&[("DISPLAY", ":0")]), true));
        assert!(browser_wanted(
            "linux",
            env(&[("WAYLAND_DISPLAY", "w-0")]),
            true
        ));
        assert!(!browser_wanted("linux", env(&[]), true), "ssh session");
        assert!(!browser_wanted("linux", env(&[("DISPLAY", "")]), true));
        assert!(
            !browser_wanted("linux", env(&[("DISPLAY", ":0")]), false),
            "not a tty"
        );
        assert!(browser_wanted("macos", env(&[]), true));
        let opted_out = env(&[("DISPLAY", ":0"), ("CLAUDIO_NO_BROWSER", "1")]);
        assert!(!browser_wanted("linux", opted_out, true));
    }
}
