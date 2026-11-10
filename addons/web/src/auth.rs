//! Authentication and request checks for `claudio web`.
//!
//! A secret token (32 random bytes, hex) lives in `<config>/web-token`
//! (0600). The page gets it in the URL fragment and trades it at
//! `POST /api/login` for a cookie naming a server-side session
//! (`sessions.rs`: 30 days idle on wall-clock time, at most 64). [`guard`] checks every request: its
//! `Host` must be one this server answers to (DNS rebinding); anything but
//! `GET`/`HEAD`, and every WebSocket upgrade, must come from an accepted
//! `Origin` (CSRF, cross-site WebSocket hijacking); `/api/*` (but login)
//! and `/ws` need the cookie. Every response, errors included, gets the CSP
//! and the other security headers. Holding the cookie is holding a shell on
//! every connected host.

use std::fs::File;
use std::io::{self, Read};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use claudio_core::paths;

use tokio::sync::Semaphore;

use crate::limit::Limiter;
use crate::password;
use crate::peer::{Peer, PeerKey};
use crate::sessions::{self, Sessions};

pub(crate) const COOKIE: &str = "claudio_web";
/// The cookie's life (400 days, the longest browsers keep): closing the
/// browser does not drop it; the server decides when a session ends.
pub(crate) const COOKIE_MAX_AGE: u64 = 34_560_000;
pub(crate) const TOKEN_BYTES: usize = 32;
pub(crate) const LOGIN_BODY_MAX: usize = 4096;
/// Concurrent argon2 verifications (each 19 MiB).
pub(crate) const HASH_PERMITS: usize = 4;

/// `--remote`'s password check: the stored hash and the slots its
/// verifications run under.
pub(crate) struct PasswordLogin {
    pub stored: password::Stored,
    pub hashing: Arc<Semaphore>,
}

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
    /// The loopback forms at the listen port (and the listen address when
    /// it is itself loopback): a loopback peer that used one of these sent
    /// a local request.
    local_hosts: Vec<String>,
    /// `--remote`: any IP literal at this port is accepted too (an address
    /// typed by hand cannot be the target of DNS rebinding).
    ip_port: Option<u16>,
    /// `--remote`: TLS only (`https://` Origins, a `Secure` cookie).
    tls: bool,
    /// Any `--allow-host` given: cookies then get `Secure`.
    proxied: bool,
}

/// `name:port`, and `name` alone when `port` is the scheme's default.
fn at_port(name: &str, port: u16, default: u16) -> Vec<String> {
    let mut v = vec![format!("{name}:{port}")];
    if port == default {
        v.push(name.to_owned());
    }
    v
}

/// An IP as a `Host` names it: IPv6 in brackets.
fn host_ip(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(ip) => ip.to_string(),
        IpAddr::V6(ip) => format!("[{ip}]"),
    }
}

impl Allowed {
    /// The listen address itself, the three loopback forms at its port (also
    /// bare when the port is 80), and each of `allow_hosts` (lowercase) bare
    /// or at the listen port; an `allow_hosts` entry may carry its own
    /// `:port`, and is then accepted at that port only.
    pub(crate) fn new(listen: SocketAddr, allow_hosts: &[String]) -> Allowed {
        let port = listen.port();
        let local_hosts = local_forms(listen, 80);
        let mut hosts = local_hosts.clone();
        hosts.extend(at_port(&host_ip(listen.ip()), port, 80));
        for name in allow_hosts {
            let name = name.trim().to_ascii_lowercase();
            if !name.contains(':') {
                hosts.push(format!("{name}:{port}"));
            }
            hosts.push(name);
        }
        Allowed::sorted(hosts, local_hosts, None, false, !allow_hosts.is_empty())
    }

    /// `--remote`: the loopback forms (local), the listen address when it is
    /// a specific one, any IP literal, and `names` (this machine's host
    /// names and each `--allow-host`, whose port, if any, is the listen
    /// port), all at the listen port (also bare when it is 443).
    pub(crate) fn remote(listen: SocketAddr, names: &[String]) -> Allowed {
        let port = listen.port();
        let local_hosts = local_forms(listen, 443);
        let mut hosts = local_hosts.clone();
        if !listen.ip().is_unspecified() {
            hosts.extend(at_port(&host_ip(listen.ip()), port, 443));
        }
        for name in names {
            let bare = name.split(':').next().unwrap_or_default();
            hosts.extend(at_port(&bare.trim().to_ascii_lowercase(), port, 443));
        }
        Allowed::sorted(hosts, local_hosts, Some(port), true, false)
    }

    fn sorted(
        mut hosts: Vec<String>,
        mut local_hosts: Vec<String>,
        ip_port: Option<u16>,
        tls: bool,
        proxied: bool,
    ) -> Allowed {
        for list in [&mut hosts, &mut local_hosts] {
            list.sort();
            list.dedup();
        }
        Allowed {
            hosts,
            local_hosts,
            ip_port,
            tls,
            proxied,
        }
    }

    /// Whether `host` is one of the loopback forms at the listen port.
    pub(crate) fn is_local_host(&self, host: &str) -> bool {
        self.local_hosts.contains(&host.trim().to_ascii_lowercase())
    }

    pub(crate) fn host_ok(&self, host: &str) -> bool {
        let host = host.trim().to_ascii_lowercase();
        !host.is_empty()
            && (self.hosts.contains(&host) || self.ip_port.is_some_and(|p| ip_literal_at(&host, p)))
    }

    /// `https://` (and, without `--remote`, `http://`) and an accepted host,
    /// nothing after it. `null` and every other scheme are refused.
    pub(crate) fn origin_ok(&self, origin: &str) -> bool {
        let origin = origin.trim().to_ascii_lowercase();
        let rest = match origin.strip_prefix("https://") {
            Some(rest) => Some(rest),
            None if !self.tls => origin.strip_prefix("http://"),
            None => None,
        };
        match rest {
            Some(host) if !host.contains('/') => self.host_ok(host),
            _ => false,
        }
    }

    pub(crate) fn secure_cookie(&self) -> bool {
        self.proxied || self.tls
    }

    pub(crate) fn tls(&self) -> bool {
        self.tls
    }
}

/// The local `Host` forms: `127.0.0.1`, `localhost` and `[::1]` at the
/// listen port (also bare at `default`), and the listen address the same
/// way only when it is itself loopback. A non-loopback address a loopback
/// peer names is not local.
fn local_forms(listen: SocketAddr, default: u16) -> Vec<String> {
    let port = listen.port();
    let mut forms = Vec::new();
    if listen.ip().to_canonical().is_loopback() {
        forms.extend(at_port(&host_ip(listen.ip()), port, default));
    }
    for name in ["127.0.0.1", "localhost", "[::1]"] {
        forms.extend(at_port(name, port, default));
    }
    forms
}

/// `host` (lowercase) is an IPv4 or bracketed IPv6 literal at `port` (bare
/// at 443), neither unspecified nor multicast.
fn ip_literal_at(host: &str, port: u16) -> bool {
    let ip = match host.parse::<SocketAddr>() {
        Ok(sock) if sock.port() == port => sock.ip(),
        Ok(_) => return false,
        Err(_) if port == 443 => {
            let v6 = host.strip_prefix('[').and_then(|h| h.strip_suffix(']'));
            match v6 {
                Some(v6) => match v6.parse::<std::net::Ipv6Addr>() {
                    Ok(ip) => IpAddr::V6(ip),
                    Err(_) => return false,
                },
                None => match host.parse::<std::net::Ipv4Addr>() {
                    Ok(ip) => IpAddr::V4(ip),
                    Err(_) => return false,
                },
            }
        }
        Err(_) => return false,
    };
    !ip.is_unspecified() && !ip.is_multicast()
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

/// A request's sender as the login rules see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Caller {
    pub local: bool,
    pub key: PeerKey,
}

/// The request's `Host` (or its URI authority), `""` when none.
pub(crate) fn host_of(req: &Request) -> &str {
    req.headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .or_else(|| req.uri().authority().map(|a| a.as_str()))
        .unwrap_or("")
}

/// The TCP peer, from the connection (`ConnectInfo`), never a header.
pub(crate) fn peer_of(req: &Request) -> Option<Peer> {
    req.extensions()
        .get::<axum::extract::ConnectInfo<Peer>>()
        .map(|c| c.0)
}

pub(crate) struct Auth {
    token: String,
    pub(crate) allowed: Allowed,
    /// Login attempts, reserved before any check (`limit.rs`).
    pub(crate) limiter: Limiter,
    csp: HeaderValue,
    /// The sessions cookies name (`sessions.rs`).
    pub(crate) sessions: Arc<Sessions>,
    /// `/dev/urandom`, opened once at start: a login opens no file on the
    /// executor. Read under its own lock, never held across an `.await`.
    urandom: Mutex<File>,
    /// [`test_remote_peer`]: a loopback address that counts as remote.
    test_remote_peer: Option<IpAddr>,
    /// `--remote`: password logins (`login.rs`).
    pub(crate) password: Option<PasswordLogin>,
}

/// `CLAUDIO_TEST_REMOTE_PEER=<ip>`, debug builds only: that loopback address
/// then counts as a remote peer, the one way a hermetic suite gets one
/// through the real listener. It can only take the token away from a peer,
/// never give access. A release build has no such function body: the
/// variable is not read, and `make static`'s final check greps for its name.
#[cfg(debug_assertions)]
fn test_remote_peer() -> Option<IpAddr> {
    std::env::var("CLAUDIO_TEST_REMOTE_PEER").ok()?.parse().ok()
}

#[cfg(not(debug_assertions))]
fn test_remote_peer() -> Option<IpAddr> {
    None
}

impl Auth {
    /// Fails only when `/dev/urandom` cannot be opened.
    pub(crate) fn new(
        token: String,
        allowed: Allowed,
        csp: String,
        sessions: Sessions,
    ) -> io::Result<Auth> {
        Ok(Auth {
            token,
            allowed,
            limiter: Limiter::default(),
            csp: HeaderValue::from_str(&csp).expect("an ASCII CSP"),
            sessions: Arc::new(sessions),
            urandom: Mutex::new(File::open("/dev/urandom")?),
            test_remote_peer: test_remote_peer(),
            password: None,
        })
    }

    /// With `--remote`: password logins against `stored`.
    pub(crate) fn with_password(mut self, stored: password::Stored) -> Auth {
        self.password = Some(PasswordLogin {
            stored,
            hashing: Arc::new(Semaphore::new(HASH_PERMITS)),
        });
        self
    }

    /// `n` random bytes from the open `/dev/urandom`, as lowercase hex.
    fn random_hex(&self, n: usize) -> io::Result<String> {
        let mut urandom = self.urandom.lock().unwrap_or_else(|e| e.into_inner());
        read_hex(&mut *urandom, n)
    }

    /// Who sent a request: its key for the limits, and whether it is local
    /// (a loopback peer that used a loopback `Host`). No peer: not local.
    pub(crate) fn caller(&self, peer: Option<Peer>, host: &str) -> Caller {
        let Some(peer) = peer else {
            return Caller {
                local: false,
                key: PeerKey::Unknown,
            };
        };
        let ip = peer.ip();
        let forced = self.test_remote_peer == Some(ip);
        let key = if forced { PeerKey::of(ip) } else { peer.key() };
        Caller {
            local: ip.is_loopback() && !forced && self.allowed.is_local_host(host),
            key,
        }
    }

    /// Whether a token login is open to this caller: always without
    /// `--remote` (every peer is loopback), else to local requests only.
    pub(crate) fn token_allowed(&self, caller: &Caller) -> bool {
        !self.allowed.tls() || caller.local
    }

    /// The login methods `/api/me` lists for this caller.
    pub(crate) fn login_methods(&self, caller: &Caller) -> Vec<&'static str> {
        let mut methods = Vec::new();
        if self.token_allowed(caller) {
            methods.push("token");
        }
        if self.password.is_some() {
            methods.push("password");
        }
        methods
    }

    /// Whether `token` is the token, in constant time.
    pub(crate) fn token_ok(&self, token: &str) -> bool {
        ct_eq(token.as_bytes(), self.token.as_bytes())
    }

    /// A new session's cookie value, and the change its reply waits for
    /// (`Sessions::written`); the session of `old` (the cookie the login
    /// came with) goes in the same step. `now`: Unix seconds.
    pub(crate) fn create_session(&self, old: Option<&str>, now: u64) -> io::Result<(String, u64)> {
        let value = self.random_hex(TOKEN_BYTES)?;
        let hash = sessions::hash_of(&value).ok_or_else(|| io::Error::other("not a token"))?;
        let change = self.sessions.login(hash, old, now);
        Ok((value, change))
    }

    /// Whether `value` names a live session (a use).
    pub(crate) fn session_ok(&self, value: &str, now: u64) -> bool {
        self.sessions.check(value, now)
    }

    /// Sign `value`'s session out; the change its reply waits for.
    pub(crate) fn end_session(&self, value: &str) -> u64 {
        self.sessions.sign_out(value)
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

pub(crate) fn set_cookie(value: &str, secure: bool) -> HeaderValue {
    let secure = if secure { "; Secure" } else { "" };
    HeaderValue::from_str(&format!(
        "{COOKIE}={value}; Max-Age={COOKIE_MAX_AGE}; HttpOnly; SameSite=Strict; Path=/{secure}"
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
    if !auth.allowed.host_ok(host_of(req)) {
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
        let now = paths::unix_now();
        let live = cookie_value(headers).is_some_and(|v| auth.session_ok(&v, now));
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
        Err(StatusCode::UNAUTHORIZED) if path == "/api/me" => {
            let caller = auth.caller(peer_of(&req), host_of(&req));
            let body = serde_json::json!({ "login": auth.login_methods(&caller) });
            (StatusCode::UNAUTHORIZED, axum::Json(body)).into_response()
        }
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

/// `GET /api/me`: 204, the guard having checked the cookie (the page asks
/// at load when its URL carries no token).
pub(crate) async fn me() -> StatusCode {
    StatusCode::NO_CONTENT
}

/// `POST /api/logout`: forget the session (the guard required it); the
/// reply waits until the sessions file no longer has it (≤ 2 s).
pub(crate) async fn logout(State(auth): State<Arc<Auth>>, headers: HeaderMap) -> Response {
    if let Some(value) = cookie_value(&headers) {
        let change = auth.end_session(&value);
        auth.sessions.written(change).await;
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
        let allowed = Allowed::new(lo(7337), &[]);
        Auth::new("ab".repeat(32), allowed, csp(None), Sessions::memory()).unwrap()
    }

    fn lo(port: u16) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], port))
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
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
        let a = Allowed::new(lo(7337), &["Claudio.Example".to_owned()]);
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
        assert!(!Allowed::new(lo(7337), &[]).secure_cookie());
        assert!(Allowed::new(lo(80), &[]).host_ok("127.0.0.1"));
    }

    #[test]
    fn the_listen_address_itself_is_accepted() {
        let a = Allowed::new("127.0.0.2:7337".parse().unwrap(), &[]);
        for ok in [
            "127.0.0.2:7337",
            "127.0.0.1:7337",
            "localhost:7337",
            "[::1]:7337",
        ] {
            assert!(a.host_ok(ok), "{ok}");
        }
        assert!(a.origin_ok("http://127.0.0.2:7337"));
        for bad in [
            "127.0.0.2:7338",
            "127.0.0.2",
            "127.0.0.3:7337",
            "evil.example:7337",
        ] {
            assert!(!a.host_ok(bad), "{bad}");
        }
        assert!(!a.origin_ok("http://127.0.0.3:7337"));
        assert!(!a.secure_cookie());
        let a = Allowed::new("127.1.2.3:80".parse().unwrap(), &[]);
        assert!(a.host_ok("127.1.2.3:80") && a.host_ok("127.1.2.3"));
        let a = Allowed::new("[::1]:9000".parse().unwrap(), &[]);
        assert!(a.host_ok("[::1]:9000") && !a.host_ok("[::1]"));
        let a = Allowed::new("[::1]:80".parse().unwrap(), &[]);
        assert!(a.host_ok("[::1]") && !a.host_ok("::1"));
    }

    #[test]
    fn an_allowed_host_may_name_its_port() {
        let a = Allowed::new(lo(7337), &names(&["Proxy.Example:8443", "bare.example"]));
        for ok in [
            "proxy.example:8443",
            "PROXY.EXAMPLE:8443",
            "bare.example",
            "bare.example:7337",
            "127.0.0.1:7337",
        ] {
            assert!(a.host_ok(ok), "{ok}");
        }
        for bad in [
            "proxy.example",
            "proxy.example:7337",
            "proxy.example:8444",
            "bare.example:8443",
            "evil.example:8443",
        ] {
            assert!(!a.host_ok(bad), "{bad}");
        }
        assert!(a.origin_ok("https://proxy.example:8443"));
        assert!(a.origin_ok("https://bare.example"));
        for bad in [
            "https://proxy.example",
            "https://proxy.example:8443/",
            "https://evil.example:8443",
            "null",
        ] {
            assert!(!a.origin_ok(bad), "{bad}");
        }
        assert!(a.secure_cookie());
    }

    fn peer(s: &str) -> Option<Peer> {
        Some(Peer(s.parse().unwrap()))
    }

    #[test]
    fn local_means_a_loopback_peer_and_a_loopback_host() {
        let a = Auth::new(
            "ab".repeat(32),
            Allowed::new(lo(7337), &names(&["proxy.example"])),
            csp(None),
            Sessions::memory(),
        )
        .unwrap();
        for (p, host) in [
            ("127.0.0.1:5000", "127.0.0.1:7337"),
            ("[::ffff:127.0.0.1]:5000", "localhost:7337"),
            ("[::1]:5000", "[::1]:7337"),
            ("127.0.0.1:5000", "LOCALHOST:7337"),
        ] {
            assert!(a.caller(peer(p), host).local, "{p} {host}");
        }
        for (p, host) in [
            (peer("127.0.0.1:5000"), "proxy.example"),
            (peer("127.0.0.1:5000"), "proxy.example:7337"),
            (peer("127.0.0.1:5000"), "127.0.0.1:7338"),
            (peer("192.168.1.5:5000"), "127.0.0.1:7337"),
            (None, "127.0.0.1:7337"),
        ] {
            assert!(!a.caller(p, host).local, "{p:?} {host}");
        }
        assert_eq!(a.caller(peer("127.0.0.9:1"), "x").key, PeerKey::Loopback);
        assert_eq!(a.caller(None, "127.0.0.1:7337").key, PeerKey::Unknown);
        assert_eq!(
            a.caller(peer("192.168.1.5:1"), "x").key,
            PeerKey::V4(0xc0a8_0105)
        );
        let a = Allowed::new("127.0.0.2:80".parse().unwrap(), &[]);
        assert!(a.is_local_host("127.0.0.2") && a.is_local_host("127.0.0.2:80"));
        assert!(a.is_local_host("localhost") && !a.is_local_host("127.0.0.3:80"));
    }

    fn remote_allowed() -> Allowed {
        Allowed::remote(
            "0.0.0.0:7337".parse().unwrap(),
            &names(&["box", "box.local", "claudio.test:7337"]),
        )
    }

    #[test]
    fn remote_hosts_are_loopback_names_ip_literals_and_this_machines_names() {
        let a = remote_allowed();
        for ok in [
            "127.0.0.1:7337",
            "localhost:7337",
            "[::1]:7337",
            "box:7337",
            "BOX.local:7337",
            "claudio.test:7337",
            "192.168.1.20:7337",
            "[fd00::1]:7337",
        ] {
            assert!(a.host_ok(ok), "{ok}");
        }
        for bad in [
            "0.0.0.0:7337",
            "[::]:7337",
            "224.0.0.1:7337",
            "192.168.1.20:7338",
            "box",
            "evil.example:7337",
            "claudio.test",
            "127.0.0.1",
            "[fd00::1",
        ] {
            assert!(!a.host_ok(bad), "{bad}");
        }
        assert!(a.origin_ok("https://box.local:7337"));
        assert!(a.origin_ok("https://192.168.1.20:7337"));
        for bad in ["http://127.0.0.1:7337", "http://box:7337", "null"] {
            assert!(!a.origin_ok(bad), "{bad}");
        }
        assert!(a.secure_cookie() && a.tls());
        assert!(a.is_local_host("localhost:7337"));
        assert!(!a.is_local_host("box:7337") && !a.is_local_host("192.168.1.20:7337"));
        let a = Allowed::remote("0.0.0.0:443".parse().unwrap(), &names(&["box"]));
        for ok in ["box", "10.0.0.1", "[fd00::1]", "127.0.0.1", "box:443"] {
            assert!(a.host_ok(ok), "{ok}");
        }
        assert!(a.origin_ok("https://box"));
        let a = Allowed::remote("192.168.1.20:7337".parse().unwrap(), &[]);
        assert!(a.host_ok("192.168.1.20:7337"));
    }

    /// R11: only loopback forms are local. A specific non-loopback listen
    /// address is accepted as a `Host`, but a loopback peer naming it did
    /// not send a local request.
    #[test]
    fn a_non_loopback_listen_address_is_never_a_local_host() {
        let listen: SocketAddr = "192.168.1.20:7337".parse().unwrap();
        let a = Allowed::remote(listen, &[]);
        assert!(a.host_ok("192.168.1.20:7337"));
        assert!(!a.is_local_host("192.168.1.20:7337"));
        assert!(a.is_local_host("127.0.0.1:7337") && a.is_local_host("[::1]:7337"));
        let auth = Auth::new("ab".repeat(32), a, csp(None), Sessions::memory()).unwrap();
        let c = auth.caller(peer("127.0.0.1:1"), "192.168.1.20:7337");
        assert!(!c.local && !auth.token_allowed(&c));
        let a = Allowed::new(listen, &[]);
        assert!(a.host_ok("192.168.1.20:7337") && !a.is_local_host("192.168.1.20:7337"));
        let a = Allowed::remote("127.0.0.2:7337".parse().unwrap(), &[]);
        assert!(
            a.is_local_host("127.0.0.2:7337"),
            "a loopback listen address is local"
        );
        let a = Allowed::remote("[::1]:443".parse().unwrap(), &[]);
        assert!(a.is_local_host("[::1]") && a.is_local_host("localhost"));
    }

    #[test]
    fn with_remote_the_token_is_for_local_requests_only() {
        let a = Auth::new(
            "ab".repeat(32),
            remote_allowed(),
            csp(None),
            Sessions::memory(),
        )
        .unwrap();
        for (p, host) in [
            ("127.0.0.1:1", "127.0.0.1:7337"),
            ("[::ffff:127.0.0.1]:1", "localhost:7337"),
        ] {
            let c = a.caller(peer(p), host);
            assert!(c.local && a.token_allowed(&c), "{p} {host}");
            assert_eq!(a.login_methods(&c), ["token"]);
        }
        for (p, host) in [
            (peer("127.0.0.1:1"), "box:7337"),
            (peer("192.168.1.5:1"), "127.0.0.1:7337"),
            (None, "127.0.0.1:7337"),
        ] {
            let c = a.caller(p, host);
            assert!(!c.local && !a.token_allowed(&c), "{p:?} {host}");
            assert!(a.login_methods(&c).is_empty());
        }
        assert_eq!(
            a.caller(peer("[2001:db8::1]:1"), "x").key,
            a.caller(peer("[2001:db8::ffff]:2"), "x").key
        );
        let plain = auth();
        let c = plain.caller(None, "proxy.example");
        assert!(
            plain.token_allowed(&c),
            "without --remote the token is for all"
        );
        assert_eq!(plain.login_methods(&c), ["token"]);
    }

    #[test]
    fn with_a_password_every_caller_may_use_it() {
        const FIXTURE: &str = include_str!("../web/e2e/fixtures/web-password");
        let a = Auth::new(
            "ab".repeat(32),
            remote_allowed(),
            csp(None),
            Sessions::memory(),
        )
        .unwrap()
        .with_password(password::Stored::parse(FIXTURE).unwrap());
        let local = a.caller(peer("127.0.0.1:1"), "127.0.0.1:7337");
        assert_eq!(a.login_methods(&local), ["token", "password"]);
        let remote = a.caller(peer("192.168.1.5:1"), "box:7337");
        assert_eq!(a.login_methods(&remote), ["password"]);
    }

    #[test]
    fn the_test_peer_hook_only_takes_the_token_away() {
        let mut a = Auth::new(
            "ab".repeat(32),
            remote_allowed(),
            csp(None),
            Sessions::memory(),
        )
        .unwrap();
        a.test_remote_peer = Some("127.0.0.2".parse().unwrap());
        let c = a.caller(peer("127.0.0.2:1"), "127.0.0.1:7337");
        assert!(!c.local);
        assert_eq!(c.key, PeerKey::V4(0x7f00_0002));
        assert!(a.caller(peer("127.0.0.1:1"), "127.0.0.1:7337").local);
    }

    #[test]
    fn a_session_is_made_used_replaced_and_ended_on_unix_seconds() {
        let a = auth();
        let t0 = 1_800_000_000;
        let (s, _) = a.create_session(None, t0).unwrap();
        assert!(token_ok(&s));
        assert!(a.session_ok(&s, t0 + sessions::IDLE - 1));
        let (replaced, _) = a.create_session(Some(&s), t0 + 10).unwrap();
        assert!(!a.session_ok(&s, t0 + 11), "the login replaced it");
        assert!(a.session_ok(&replaced, t0 + 11));
        a.end_session(&replaced);
        assert!(!a.session_ok(&replaced, t0 + 12));
        a.end_session("nothing");
    }

    #[test]
    fn the_cookie_outlives_the_browser_and_the_sign_out_cookie_expires_it() {
        let set = set_cookie(&"ab".repeat(32), false);
        assert_eq!(
            set.to_str().unwrap(),
            format!(
                "claudio_web={}; Max-Age=34560000; HttpOnly; SameSite=Strict; Path=/",
                "ab".repeat(32)
            )
        );
        assert!(set_cookie("x", true)
            .to_str()
            .unwrap()
            .ends_with("; Path=/; Secure"));
        let clear = clear_cookie(true);
        assert_eq!(
            clear.to_str().unwrap(),
            "claudio_web=; Max-Age=0; HttpOnly; SameSite=Strict; Path=/; Secure"
        );
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
        let dir = claudio_core::paths::socket_test_dir("web-token");
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
