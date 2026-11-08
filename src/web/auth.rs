//! Authentication and request checks for `claudio web`.
//!
//! A secret token (32 random bytes, hex) lives in `<config>/web-token`
//! (0600). The page gets it in the URL fragment and trades it at
//! `POST /api/login` for a cookie naming a server-side session (in memory,
//! 24 h idle expiry, at most 64). [`guard`] checks every request: its
//! `Host` must be one this server answers to (DNS rebinding); anything but
//! `GET`/`HEAD`, and every WebSocket upgrade, must come from an accepted
//! `Origin` (CSRF, cross-site WebSocket hijacking); `/api/*` (but login)
//! and `/ws` need the cookie. Every response, errors included, gets the CSP
//! and the other security headers. Holding the cookie is holding a shell on
//! every connected host.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::paths;

pub(crate) const COOKIE: &str = "claudio_web";
pub(crate) const TOKEN_BYTES: usize = 32;
pub(crate) const SESSION_IDLE: Duration = Duration::from_secs(24 * 60 * 60);
pub(crate) const MAX_SESSIONS: usize = 64;
pub(crate) const MAX_FAILURES: usize = 5;
pub(crate) const FAILURE_WINDOW: Duration = Duration::from_secs(60);
pub(crate) const LOGIN_BODY_MAX: usize = 4096;

/// `<config>/web-token`.
pub(crate) fn token_path() -> PathBuf {
    paths::config_dir().join("web-token")
}

/// `n` random bytes from the kernel, as lowercase hex. Opens
/// `/dev/urandom` each call: for the token at start; a running server uses
/// [`Auth`]'s open file.
pub(crate) fn random_hex(n: usize) -> io::Result<String> {
    read_hex(&mut File::open("/dev/urandom")?, n)
}

/// `n` bytes of `src`, as lowercase hex.
fn read_hex(src: &mut impl Read, n: usize) -> io::Result<String> {
    let mut bytes = vec![0u8; n];
    src.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Whether `s` is a token this server could have written: 64 lowercase hex.
pub(crate) fn token_ok(s: &str) -> bool {
    s.len() == 2 * TOKEN_BYTES && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The saved token, or a new one written 0600 when there is none, it is not
/// a regular private file of ours holding a well-formed token, or `rotate`.
/// Blocking I/O: call before the runtime starts (or in `spawn_blocking`).
pub(crate) fn load_or_create_token(path: &Path, rotate: bool) -> io::Result<String> {
    use std::os::unix::fs::MetadataExt;
    if !rotate {
        if let Ok(meta) = std::fs::symlink_metadata(path) {
            let private = meta.file_type().is_file()
                && meta.mode() & 0o077 == 0
                && meta.uid() == paths::uid();
            if private {
                let text = std::fs::read_to_string(path)?;
                let token = text.trim();
                if token_ok(token) {
                    return Ok(token.to_owned());
                }
            }
        }
    }
    let token = random_hex(TOKEN_BYTES)?;
    paths::write_atomic(path, format!("{token}\n").as_bytes())?;
    Ok(token)
}

/// `a == b`, in time that depends only on the lengths.
pub(crate) fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let diff = a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y));
    std::hint::black_box(diff) == 0
}

/// The `Host` and `Origin` values this server answers to.
#[derive(Debug, Clone)]
pub(crate) struct Allowed {
    /// Every accepted `host[:port]`, lowercase.
    hosts: Vec<String>,
    /// Any `--allow-host` given: cookies then get `Secure`.
    proxied: bool,
}

impl Allowed {
    /// The three loopback forms at `port` (also bare when the port is 80),
    /// and each of `allow_hosts` (lowercase) bare or at `port`.
    pub(crate) fn new(port: u16, allow_hosts: &[String]) -> Allowed {
        let mut hosts = Vec::new();
        for name in ["127.0.0.1", "localhost", "[::1]"] {
            hosts.push(format!("{name}:{port}"));
            if port == 80 {
                hosts.push(name.to_owned());
            }
        }
        for name in allow_hosts {
            let name = name.trim().to_ascii_lowercase();
            hosts.push(format!("{name}:{port}"));
            hosts.push(name);
        }
        Allowed {
            hosts,
            proxied: !allow_hosts.is_empty(),
        }
    }

    pub(crate) fn host_ok(&self, host: &str) -> bool {
        let host = host.trim().to_ascii_lowercase();
        !host.is_empty() && self.hosts.iter().any(|h| *h == host)
    }

    /// `http://` or `https://` and an accepted host, nothing after it.
    /// `null` and every other scheme are refused.
    pub(crate) fn origin_ok(&self, origin: &str) -> bool {
        let origin = origin.trim().to_ascii_lowercase();
        let rest = origin
            .strip_prefix("http://")
            .or_else(|| origin.strip_prefix("https://"));
        match rest {
            Some(host) if !host.contains('/') => self.host_ok(host),
            _ => false,
        }
    }

    pub(crate) fn secure_cookie(&self) -> bool {
        self.proxied
    }
}

/// The CSP: `'self'` only, plus the hash of the one inline script allowed,
/// `index.html`'s import map.
pub(crate) fn csp(importmap_sha256: Option<&str>) -> String {
    let script = match importmap_sha256 {
        Some(hash) => format!("script-src 'self' 'sha256-{hash}'"),
        None => "script-src 'self'".to_owned(),
    };
    format!(
        "default-src 'self'; {script}; style-src 'self' 'unsafe-inline'; \
         img-src 'self' data: blob:; font-src 'self'; connect-src 'self'; \
         object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'"
    )
}

#[derive(Default)]
struct Inner {
    /// Cookie values and their last use.
    sessions: Vec<(String, Instant)>,
    /// When the recent failed logins happened, oldest first.
    failures: VecDeque<Instant>,
}

pub(crate) struct Auth {
    token: String,
    allowed: Allowed,
    csp: HeaderValue,
    inner: Mutex<Inner>,
    /// `/dev/urandom`, opened once at start: a login opens no file on the
    /// executor. Read under its own lock, never held across an `.await`.
    urandom: Mutex<File>,
}

impl Auth {
    /// Fails only when `/dev/urandom` cannot be opened.
    pub(crate) fn new(token: String, allowed: Allowed, csp: String) -> io::Result<Auth> {
        Ok(Auth {
            token,
            allowed,
            csp: HeaderValue::from_str(&csp).expect("an ASCII CSP"),
            inner: Mutex::new(Inner::default()),
            urandom: Mutex::new(File::open("/dev/urandom")?),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `n` random bytes from the open `/dev/urandom`, as lowercase hex.
    fn random_hex(&self, n: usize) -> io::Result<String> {
        let mut urandom = self.urandom.lock().unwrap_or_else(|e| e.into_inner());
        read_hex(&mut *urandom, n)
    }

    #[allow(dead_code)] // for the hub (Task 4)
    pub(crate) fn allowed(&self) -> &Allowed {
        &self.allowed
    }

    /// Check a login: `Ok` for the right token; `429` while
    /// [`MAX_FAILURES`] failures are under [`FAILURE_WINDOW`] old (the
    /// token is not even compared then); else `401`, which counts.
    pub(crate) fn login(&self, token: &str, now: Instant) -> Result<(), StatusCode> {
        let mut inner = self.lock();
        while inner
            .failures
            .front()
            .is_some_and(|t| now.duration_since(*t) >= FAILURE_WINDOW)
        {
            inner.failures.pop_front();
        }
        if inner.failures.len() >= MAX_FAILURES {
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
        if ct_eq(token.as_bytes(), self.token.as_bytes()) {
            Ok(())
        } else {
            inner.failures.push_back(now);
            Err(StatusCode::UNAUTHORIZED)
        }
    }

    /// A new session's cookie value. Idle ones are dropped first; past
    /// [`MAX_SESSIONS`], the least recently used goes.
    pub(crate) fn create_session(&self, now: Instant) -> io::Result<String> {
        let value = self.random_hex(TOKEN_BYTES)?;
        let mut inner = self.lock();
        inner
            .sessions
            .retain(|(_, seen)| now.duration_since(*seen) < SESSION_IDLE);
        if inner.sessions.len() >= MAX_SESSIONS {
            let oldest = inner
                .sessions
                .iter()
                .enumerate()
                .min_by_key(|(_, (_, seen))| *seen)
                .map(|(i, _)| i);
            if let Some(i) = oldest {
                inner.sessions.remove(i);
            }
        }
        inner.sessions.push((value.clone(), now));
        Ok(value)
    }

    /// Whether `value` names a live session (every one is compared in
    /// constant time); refreshes its last use.
    pub(crate) fn session_ok(&self, value: &str, now: Instant) -> bool {
        let mut inner = self.lock();
        inner
            .sessions
            .retain(|(_, seen)| now.duration_since(*seen) < SESSION_IDLE);
        let mut found = None;
        for (i, (v, _)) in inner.sessions.iter().enumerate() {
            if ct_eq(v.as_bytes(), value.as_bytes()) {
                found = Some(i);
            }
        }
        match found {
            Some(i) => {
                inner.sessions[i].1 = now;
                true
            }
            None => false,
        }
    }

    pub(crate) fn end_session(&self, value: &str) {
        self.lock()
            .sessions
            .retain(|(v, _)| !ct_eq(v.as_bytes(), value.as_bytes()));
    }
}

/// Our cookie's value in `headers`, if any.
pub(crate) fn cookie_value(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == COOKIE)
        .map(|(_, value)| value.trim().to_owned())
}

fn set_cookie(value: &str, secure: bool) -> HeaderValue {
    let secure = if secure { "; Secure" } else { "" };
    HeaderValue::from_str(&format!(
        "{COOKIE}={value}; HttpOnly; SameSite=Strict; Path=/{secure}"
    ))
    .expect("an ASCII cookie")
}

fn clear_cookie(secure: bool) -> HeaderValue {
    let secure = if secure { "; Secure" } else { "" };
    HeaderValue::from_str(&format!(
        "{COOKIE}=; Max-Age=0; HttpOnly; SameSite=Strict; Path=/{secure}"
    ))
    .expect("an ASCII cookie")
}

fn is_api(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/")
}

/// The checks, in order: Host (421), Origin for anything that is not a
/// plain GET/HEAD (403), the session cookie for `/ws` and `/api/*` but
/// login (401).
fn check(auth: &Auth, req: &Request) -> Result<(), StatusCode> {
    let headers = req.headers();
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .or_else(|| req.uri().authority().map(|a| a.as_str()))
        .unwrap_or("");
    if !auth.allowed.host_ok(host) {
        return Err(StatusCode::MISDIRECTED_REQUEST);
    }
    let upgrade = headers
        .get(header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
    let plain_read = matches!(*req.method(), Method::GET | Method::HEAD) && !upgrade;
    if !plain_read {
        let origin = headers
            .get(header::ORIGIN)
            .and_then(|o| o.to_str().ok())
            .unwrap_or("");
        if !auth.allowed.origin_ok(origin) {
            return Err(StatusCode::FORBIDDEN);
        }
    }
    let path = req.uri().path();
    if path == "/ws" || (is_api(path) && path != "/api/login") {
        let live = cookie_value(headers).is_some_and(|v| auth.session_ok(&v, Instant::now()));
        if !live {
            return Err(StatusCode::UNAUTHORIZED);
        }
    }
    Ok(())
}

/// Middleware on every route and the asset fallback.
pub(crate) async fn guard(State(auth): State<Arc<Auth>>, req: Request, next: Next) -> Response {
    let path = req.uri().path().to_owned();
    let mut res = match check(&auth, &req) {
        Ok(()) => next.run(req).await,
        Err(status) => status.into_response(),
    };
    let h = res.headers_mut();
    h.insert(header::CONTENT_SECURITY_POLICY, auth.csp.clone());
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    if is_api(&path) || path == "/" || path == "/index.html" {
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    res
}

#[derive(Deserialize)]
struct LoginBody {
    token: String,
}

/// `POST /api/login {"token": …}`: the guard already checked Host and
/// Origin, so only same-origin attempts reach the rate limit.
pub(crate) async fn login(State(auth): State<Arc<Auth>>, req: Request) -> Response {
    let body = match axum::body::to_bytes(req.into_body(), LOGIN_BODY_MAX).await {
        Ok(body) => body,
        Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
    };
    let Ok(LoginBody { token }) = serde_json::from_slice(&body) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if let Err(status) = auth.login(&token, Instant::now()) {
        tracing::info!(status = status.as_u16(), "web: login refused");
        return status.into_response();
    }
    match auth.create_session(Instant::now()) {
        Ok(value) => {
            let mut res = StatusCode::NO_CONTENT.into_response();
            let cookie = set_cookie(&value, auth.allowed.secure_cookie());
            res.headers_mut().insert(header::SET_COOKIE, cookie);
            res
        }
        Err(e) => {
            tracing::error!(error = %e, "web: no random bytes for a session");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `GET /api/me`: 204, the guard having checked the cookie (the page asks
/// at load when its URL carries no token).
pub(crate) async fn me() -> StatusCode {
    StatusCode::NO_CONTENT
}

/// `POST /api/logout`: forget the session (the guard required it).
pub(crate) async fn logout(State(auth): State<Arc<Auth>>, headers: HeaderMap) -> Response {
    if let Some(value) = cookie_value(&headers) {
        auth.end_session(&value);
    }
    let mut res = StatusCode::NO_CONTENT.into_response();
    let cookie = clear_cookie(auth.allowed.secure_cookie());
    res.headers_mut().insert(header::SET_COOKIE, cookie);
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auth() -> Auth {
        Auth::new("ab".repeat(32), Allowed::new(7337, &[]), csp(None)).unwrap()
    }

    #[test]
    fn constant_time_eq_compares_whole_values() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"abcd"));
        assert!(!ct_eq(b"", b"a"));
        assert!(ct_eq(b"", b""));
    }

    #[test]
    fn hosts_and_origins_accept_only_the_listen_forms_and_allowed_names() {
        let a = Allowed::new(7337, &["Claudio.Example".to_owned()]);
        for ok in [
            "127.0.0.1:7337",
            "LOCALHOST:7337",
            "[::1]:7337",
            "claudio.example",
            "claudio.example:7337",
        ] {
            assert!(a.host_ok(ok), "{ok}");
        }
        for bad in [
            "127.0.0.1:7338",
            "evil.example:7337",
            "127.0.0.1",
            "localhost",
            "",
            "claudio.example:80",
        ] {
            assert!(!a.host_ok(bad), "{bad}");
        }
        for ok in [
            "http://127.0.0.1:7337",
            "https://claudio.example",
            "http://localhost:7337",
        ] {
            assert!(a.origin_ok(ok), "{ok}");
        }
        for bad in [
            "null",
            "",
            "127.0.0.1:7337",
            "ftp://127.0.0.1:7337",
            "http://127.0.0.1:7337/x",
            "http://evil.example:7337",
            "http://127.0.0.1:7337.evil.example",
        ] {
            assert!(!a.origin_ok(bad), "{bad}");
        }
        assert!(a.secure_cookie());
        assert!(!Allowed::new(7337, &[]).secure_cookie());
        assert!(Allowed::new(80, &[]).host_ok("127.0.0.1"));
    }

    #[test]
    fn login_locks_after_five_failures_for_a_minute() {
        let a = auth();
        let t0 = Instant::now();
        for _ in 0..MAX_FAILURES {
            assert_eq!(a.login("wrong", t0), Err(StatusCode::UNAUTHORIZED));
        }
        assert_eq!(a.login("wrong", t0), Err(StatusCode::TOO_MANY_REQUESTS));
        assert_eq!(
            a.login(&"ab".repeat(32), t0 + Duration::from_secs(59)),
            Err(StatusCode::TOO_MANY_REQUESTS)
        );
        assert_eq!(
            a.login(&"ab".repeat(32), t0 + Duration::from_secs(61)),
            Ok(())
        );
    }

    #[test]
    fn sessions_expire_when_idle_and_are_capped() {
        let a = auth();
        let t0 = Instant::now();
        let s = a.create_session(t0).unwrap();
        assert!(token_ok(&s));
        let t1 = t0 + SESSION_IDLE - Duration::from_secs(1);
        assert!(a.session_ok(&s, t1), "a use within the idle time keeps it");
        assert!(
            a.session_ok(&s, t1 + SESSION_IDLE - Duration::from_secs(1)),
            "and refreshes it"
        );
        assert!(!a.session_ok(&s, t1 + 2 * SESSION_IDLE), "idle too long");
        let a = auth();
        let oldest = a.create_session(t0).unwrap();
        for i in 1..=MAX_SESSIONS as u64 {
            a.create_session(t0 + Duration::from_secs(i)).unwrap();
        }
        assert!(
            !a.session_ok(&oldest, t0 + Duration::from_secs(100)),
            "the least recently used went"
        );
        a.end_session("nothing");
        assert!(!a.session_ok("", t0 + Duration::from_secs(100)));
    }

    #[test]
    fn cookie_value_finds_ours_among_others() {
        let mut h = HeaderMap::new();
        h.append(
            header::COOKIE,
            HeaderValue::from_static("a=1; claudio_web=abc ; b=2"),
        );
        assert_eq!(cookie_value(&h).as_deref(), Some("abc"));
        let mut h = HeaderMap::new();
        h.append(header::COOKIE, HeaderValue::from_static("claudio_webx=1"));
        assert_eq!(cookie_value(&h), None);
    }

    #[test]
    fn token_file_is_private_reused_and_rotated() {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::paths::socket_test_dir("web-token");
        let path = dir.join("web-token");
        let first = load_or_create_token(&path, false).unwrap();
        assert!(token_ok(&first));
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(load_or_create_token(&path, false).unwrap(), first);
        let rotated = load_or_create_token(&path, true).unwrap();
        assert_ne!(rotated, first);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let replaced = load_or_create_token(&path, false).unwrap();
        assert_ne!(replaced, rotated, "a token others can read is replaced");
        std::fs::write(&path, "short\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(token_ok(&load_or_create_token(&path, false).unwrap()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn csp_allows_only_the_import_map_inline() {
        assert!(csp(None).contains("script-src 'self';"));
        let with = csp(Some("AAAA"));
        assert!(with.contains("script-src 'self' 'sha256-AAAA';"), "{with}");
        let script = with
            .split(';')
            .find(|d| d.trim().starts_with("script-src"))
            .unwrap();
        assert!(!script.contains("unsafe"), "{with}");
        assert!(!with.contains("unsafe-eval"), "{with}");
    }
}
