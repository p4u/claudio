//! `claudio web` integration tests: the CLI, authentication and request
//! checks, the browser protocol against a fake claude, spawn secrets.
//! Run single-threaded: `cargo test --test web -- --test-threads=1`.

mod common;

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::os::unix::fs::PermissionsExt;
use std::process::{Child, Command, Stdio};
use std::{fs, thread};

use common::{ManagerHarness, BINARY, DAEMON_WAIT, WAIT};
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::{self, client::IntoClientRequest, Message};

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// The isolated env of a test `claudio web` (and of the daemon it starts).
fn apply_env(h: &ManagerHarness, cmd: &mut Command) {
    for (k, _) in std::env::vars() {
        if k.starts_with("CLAUDIO_") {
            cmd.env_remove(&k);
        }
    }
    cmd.env("XDG_RUNTIME_DIR", &h.runtime_dir)
        .env("XDG_CONFIG_HOME", &h.config_home)
        .env("XDG_CACHE_HOME", h.root.join("xdg-cache"))
        .env("HOME", &h.home)
        .env("CLAUDE_CONFIG_DIR", h.root.join("claude-config"))
        .env("CLAUDIO_CLAUDE_PATH", &h.fake_claude)
        .env("CLAUDIO_NO_UPDATE_CHECK", "1")
        .env("CLAUDIO_NO_BROWSER", "1")
        .env("ANTHROPIC_API_KEY", "test-key-not-real")
        .env("TERM", "xterm-256color")
        .env("SHELL", "/bin/sh")
        .env_remove("COLORTERM")
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        .env_remove("ANTHROPIC_BASE_URL");
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// A running `claudio web` on a free loopback port, in a harness's dirs.
struct Web {
    h: Option<ManagerHarness>,
    child: Child,
    port: u16,
    token: String,
}

impl Web {
    fn start() -> Web {
        Web::start_with(ManagerHarness::new(), &[], |_| {})
    }

    fn start_with(h: ManagerHarness, args: &[&str], setup: impl FnOnce(&mut Command)) -> Web {
        let port = free_port();
        let mut cmd = Command::new(BINARY);
        cmd.arg("web")
            .arg("--listen")
            .arg(format!("127.0.0.1:{port}"))
            .arg("--no-open")
            .args(args);
        apply_env(&h, &mut cmd);
        setup(&mut cmd);
        let log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(h.root.join("web.log"))
            .unwrap();
        cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(log);
        let mut child = cmd.spawn().expect("start claudio web");
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let _ = tx.send(line);
            }
        });
        let url = loop {
            let line = rx
                .recv_timeout(DAEMON_WAIT)
                .expect("claudio web printed no URL");
            if let Some(url) = line.strip_prefix("claudio web: ") {
                break url.to_owned();
            }
        };
        let token = fs::read_to_string(h.config_home.join("claudio/web-token"))
            .unwrap()
            .trim()
            .to_owned();
        assert_eq!(url, format!("http://127.0.0.1:{port}/#t={token}"));
        Web {
            h: Some(h),
            child,
            port,
            token,
        }
    }

    fn h(&self) -> &ManagerHarness {
        self.h.as_ref().unwrap()
    }

    /// Stop the server; the harness (and the daemon) stay.
    fn stop(mut self) -> ManagerHarness {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let h = self.h.take().unwrap();
        daemon_settled(&h);
        h
    }

    fn host(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }

    fn origin(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn get(&self, path: &str, extra: &[(&str, &str)]) -> Reply {
        http(self.port, "GET", path, &self.host(), extra, "")
    }

    fn post(&self, path: &str, origin: Option<&str>, body: &str, extra: &[(&str, &str)]) -> Reply {
        let mut headers: Vec<(&str, &str)> = extra.to_vec();
        if let Some(origin) = origin {
            headers.push(("Origin", origin));
        }
        http(self.port, "POST", path, &self.host(), &headers, body)
    }

    /// Log in with the right token; the cookie's value.
    fn login(&self) -> String {
        let body = json!({"token": self.token}).to_string();
        let reply = self.post("/api/login", Some(&self.origin()), &body, &[]);
        assert_eq!(reply.status, 204, "{reply:?}");
        let cookie = reply.header("set-cookie").expect("a cookie").to_owned();
        for flag in ["HttpOnly", "SameSite=Strict", "Path=/"] {
            assert!(cookie.contains(flag), "{cookie}");
        }
        assert!(!cookie.contains("Secure"), "{cookie}");
        let value = cookie
            .strip_prefix("claudio_web=")
            .and_then(|rest| rest.split(';').next())
            .expect("claudio_web=…")
            .to_owned();
        assert_eq!(value.len(), 64);
        value
    }

    async fn ws_with(&self, cookie: &str, origin: &str) -> Result<Ws, u16> {
        let mut req = format!("ws://127.0.0.1:{}/ws", self.port)
            .into_client_request()
            .unwrap();
        req.headers_mut().insert("Origin", origin.parse().unwrap());
        if !cookie.is_empty() {
            let value = format!("claudio_web={cookie}");
            req.headers_mut().insert("Cookie", value.parse().unwrap());
        }
        match tokio_tungstenite::connect_async(req).await {
            Ok((ws, _)) => Ok(ws),
            Err(tungstenite::Error::Http(resp)) => Err(resp.status().as_u16()),
            Err(e) => panic!("ws: {e}"),
        }
    }
}

impl Drop for Web {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(h) = &self.h {
            daemon_settled(h);
        }
    }
}

/// Once a daemon was started (its log exists), wait until it wrote its PID,
/// so the harness can stop it: a server killed right after starting one
/// must not leave it running.
fn daemon_settled(h: &ManagerHarness) {
    if !h.runtime_dir.join("claudio/daemon.log").exists() {
        return;
    }
    let deadline = std::time::Instant::now() + DAEMON_WAIT;
    while h.daemon_pid().is_none() && std::time::Instant::now() < deadline {
        thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[derive(Debug)]
struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// One raw HTTP/1.1 request, so a test controls `Host` and `Origin`.
fn http(
    port: u16,
    method: &str,
    path: &str,
    host: &str,
    extra: &[(&str, &str)],
    body: &str,
) -> Reply {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(WAIT)).unwrap();
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    if !body.is_empty() {
        req.push_str("Content-Type: application/json\r\n");
    }
    for (k, v) in extra {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(body);
    s.write_all(req.as_bytes()).unwrap();
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let mut lines = head.lines();
    let status = lines
        .next()
        .and_then(|l| l.split(' ').nth(1))
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect();
    Reply {
        status,
        headers,
        body: body.to_owned(),
    }
}

/// One browser tab's WebSocket: requests, events and session output, every
/// frame recorded in `seen`.
struct Tab {
    ws: Ws,
    next: u32,
    pending: VecDeque<Message>,
    hello: Value,
    snapshot: Value,
    seen: Vec<Vec<u8>>,
}

impl Tab {
    async fn open(web: &Web, cookie: &str) -> Tab {
        let ws = web.ws_with(cookie, &web.origin()).await.expect("ws");
        let mut tab = Tab {
            ws,
            next: 0,
            pending: VecDeque::new(),
            hello: Value::Null,
            snapshot: Value::Null,
            seen: Vec::new(),
        };
        tab.hello = tab.text_where(|v| v["t"] == "hello").await;
        tab.snapshot = tab.text_where(|v| v["t"] == "snapshot").await;
        tab
    }

    async fn next_message(&mut self) -> Message {
        let msg = tokio::time::timeout(WAIT, self.ws.next())
            .await
            .expect("no frame in time")
            .expect("socket closed")
            .expect("ws error");
        self.seen.push(msg.clone().into_data().to_vec());
        msg
    }

    /// The first text frame (buffered or new) matching `pick`; the others
    /// stay buffered in order.
    async fn text_where(&mut self, pick: impl Fn(&Value) -> bool) -> Value {
        let found = self
            .pending
            .iter()
            .position(|m| text_json(m).is_some_and(|v| pick(&v)));
        if let Some(i) = found {
            return text_json(&self.pending.remove(i).unwrap()).unwrap();
        }
        // Other frames (host stats every ~2 s) must not keep a wait alive.
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            assert!(
                tokio::time::Instant::now() < deadline,
                "no matching frame in time; buffered: {:?}",
                self.pending
                    .iter()
                    .filter_map(text_json)
                    .collect::<Vec<_>>()
            );
            let msg = self.next_message().await;
            match text_json(&msg) {
                Some(v) if pick(&v) => return v,
                _ => self.pending.push_back(msg),
            }
        }
    }

    async fn req(&mut self, op: &str, args: Value) -> Result<Value, String> {
        self.next += 1;
        let id = self.next;
        let mut frame = if args.is_object() { args } else { json!({}) };
        frame["t"] = "req".into();
        frame["id"] = id.into();
        frame["op"] = op.into();
        self.ws
            .send(Message::text(frame.to_string()))
            .await
            .unwrap();
        let res = self.text_where(|v| v["t"] == "res" && v["id"] == id).await;
        if res["ok"] == true {
            Ok(res["data"].clone())
        } else {
            Err(res["error"].as_str().unwrap_or_default().to_owned())
        }
    }

    async fn event(&mut self, ev: &str, pick: impl Fn(&Value) -> bool) -> Value {
        self.text_where(|v| v["t"] == "ev" && v["ev"] == ev && pick(v))
            .await
    }

    /// Session `sid`'s output (0x01 frames, buffered ones first) until it
    /// contains `needle`.
    async fn output_until(&mut self, sid: &str, needle: &str) -> String {
        let id = uuid_bytes(sid);
        let mut text = String::new();
        let mut keep = VecDeque::new();
        while let Some(m) = self.pending.pop_front() {
            match output_of(&m, &id) {
                Some(b) => text.push_str(&String::from_utf8_lossy(b)),
                None => keep.push_back(m),
            }
        }
        self.pending = keep;
        let deadline = tokio::time::Instant::now() + WAIT;
        while !text.contains(needle) {
            assert!(
                tokio::time::Instant::now() < deadline,
                "{needle:?} not in the output in time: {text:?}"
            );
            let msg = self.next_message().await;
            match output_of(&msg, &id) {
                Some(b) => text.push_str(&String::from_utf8_lossy(b)),
                None => self.pending.push_back(msg),
            }
        }
        text
    }

    async fn input(&mut self, sid: &str, bytes: &[u8]) {
        let mut frame = vec![0x02];
        frame.extend(uuid_bytes(sid));
        frame.extend_from_slice(bytes);
        self.ws.send(Message::binary(frame)).await.unwrap();
    }

    /// The close code the server ends the socket with.
    async fn close_code(&mut self) -> Option<u16> {
        loop {
            match tokio::time::timeout(WAIT, self.ws.next()).await.ok()?? {
                Ok(Message::Close(frame)) => return frame.map(|f| u16::from(f.code)),
                Ok(_) => continue,
                Err(_) => return None,
            }
        }
    }
}

fn text_json(m: &Message) -> Option<Value> {
    match m {
        Message::Text(t) => serde_json::from_str(t.as_str()).ok(),
        _ => None,
    }
}

fn output_of<'a>(m: &'a Message, id: &[u8; 16]) -> Option<&'a [u8]> {
    match m {
        Message::Binary(b) if b.len() >= 17 && b[0] == 0x01 && &b[1..17] == id => Some(&b[17..]),
        _ => None,
    }
}

fn uuid_bytes(sid: &str) -> [u8; 16] {
    *uuid::Uuid::parse_str(sid).unwrap().as_bytes()
}

// ── AC1: loopback only ────────────────────────────────────────────────────────

#[test]
fn web_refuses_a_non_loopback_listen() {
    let h = ManagerHarness::new();
    let mut cmd = Command::new(BINARY);
    cmd.args(["web", "--listen", "0.0.0.0:7337", "--no-open"]);
    apply_env(&h, &mut cmd);
    let out = cmd.output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("ssh -L 7337:127.0.0.1:7337"), "{err}");
    assert!(err.contains("--allow-host"), "{err}");
    assert!(
        !h.config_home.join("claudio/web-token").exists(),
        "no token before the address is accepted"
    );
}

// ── AC2: authentication and request checks ───────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_auth_gates_api_and_ws() {
    let web = Web::start();
    // No cookie.
    assert_eq!(web.get("/api/x", &[]).status, 401);
    assert_eq!(web.get("/api/me", &[]).status, 401);
    assert_eq!(web.ws_with("", &web.origin()).await.err(), Some(401));
    // DNS rebinding: a foreign Host.
    let rebound = http(
        web.port,
        "GET",
        "/",
        &format!("evil.example:{}", web.port),
        &[],
        "",
    );
    assert_eq!(rebound.status, 421);
    // The page needs no cookie and carries the security headers.
    let page = web.get("/", &[]);
    assert_eq!(page.status, 200);
    let csp = page.header("content-security-policy").expect("a CSP");
    for part in [
        "default-src 'self'",
        "script-src 'self'",
        "object-src 'none'",
        "base-uri 'none'",
        "frame-ancestors 'none'",
        "connect-src 'self'",
    ] {
        assert!(csp.contains(part), "{csp}");
    }
    assert!(!csp.contains("unsafe-eval"), "{csp}");
    assert_eq!(page.header("x-content-type-options"), Some("nosniff"));
    assert_eq!(page.header("referrer-policy"), Some("no-referrer"));
    assert_eq!(page.header("cache-control"), Some("no-store"));
    assert!(
        rebound.header("content-security-policy").is_some(),
        "errors carry the headers too"
    );
    // A cross-site or origin-less login is refused before the token is read.
    let body = json!({"token": web.token}).to_string();
    assert_eq!(
        web.post("/api/login", Some("http://evil.example"), &body, &[])
            .status,
        403
    );
    assert_eq!(web.post("/api/login", None, &body, &[]).status, 403);
    // The right token from the page's origin: a cookie (flags checked in login()).
    let cookie = web.login();
    // A WebSocket from another origin is refused even with the cookie.
    assert_eq!(
        web.ws_with(&cookie, "http://evil.example").await.err(),
        Some(403)
    );
    // From the page's origin: hello, then the snapshot.
    let mut tab = Tab::open(&web, &cookie).await;
    assert_eq!(tab.hello["v"], 1);
    assert_eq!(tab.hello["version"], env!("CARGO_PKG_VERSION"));
    assert!(tab.snapshot["sessions"].is_array() && tab.snapshot["hosts"].is_array());
    assert_eq!(
        tab.req("no.such.op", json!({})).await,
        Err("unknown op".to_owned())
    );
    // Authenticated API paths pass the gate.
    let with_cookie = format!("claudio_web={cookie}");
    assert_eq!(web.get("/api/x", &[("Cookie", &with_cookie)]).status, 404);
    assert_eq!(web.get("/api/me", &[("Cookie", &with_cookie)]).status, 204);
    // Logout ends the session.
    let out = web.post(
        "/api/logout",
        Some(&web.origin()),
        "",
        &[("Cookie", &with_cookie)],
    );
    assert_eq!(out.status, 204);
    assert_eq!(web.ws_with(&cookie, &web.origin()).await.err(), Some(401));
}

#[test]
fn web_login_is_rate_limited() {
    let web = Web::start();
    let wrong = json!({"token": "0".repeat(64)}).to_string();
    for i in 0..5 {
        assert_eq!(
            web.post("/api/login", Some(&web.origin()), &wrong, &[])
                .status,
            401,
            "attempt {i}"
        );
    }
    assert_eq!(
        web.post("/api/login", Some(&web.origin()), &wrong, &[])
            .status,
        429
    );
    let right = json!({"token": web.token}).to_string();
    assert_eq!(
        web.post("/api/login", Some(&web.origin()), &right, &[])
            .status,
        429,
        "locked for the rest of the minute, right token or not"
    );
}

#[test]
fn web_token_is_private_reused_and_rotated() {
    let web = Web::start();
    let path = web.h().config_home.join("claudio/web-token");
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let first = web.token.clone();
    let h = web.stop();
    let mut cmd = Command::new(BINARY);
    cmd.args(["web", "--print-url"]);
    apply_env(&h, &mut cmd);
    let out = cmd.output().unwrap();
    assert!(out.status.success());
    let printed = String::from_utf8_lossy(&out.stdout);
    assert!(
        printed.trim().ends_with(&format!("#t={first}")),
        "{printed}"
    );
    let web = Web::start_with(h, &[], |_| {});
    assert_eq!(web.token, first, "reused across restarts");
    let h = web.stop();
    let web = Web::start_with(h, &["--rotate-token"], |_| {});
    assert_ne!(web.token, first);
    assert!(web.token.len() == 64 && web.token.bytes().all(|b| b.is_ascii_hexdigit()));
}

#[test]
fn web_allow_host_accepts_the_proxy_name_and_secures_the_cookie() {
    let web = Web::start_with(
        ManagerHarness::new(),
        &["--allow-host", "Claudio.Example"],
        |_| {},
    );
    assert_eq!(
        http(web.port, "GET", "/", "claudio.example", &[], "").status,
        200
    );
    let with_port = format!("CLAUDIO.EXAMPLE:{}", web.port);
    assert_eq!(http(web.port, "GET", "/", &with_port, &[], "").status, 200);
    assert_eq!(
        http(web.port, "GET", "/", "other.example", &[], "").status,
        421
    );
    let body = json!({"token": web.token}).to_string();
    let reply = http(
        web.port,
        "POST",
        "/api/login",
        "claudio.example",
        &[("Origin", "https://claudio.example")],
        &body,
    );
    assert_eq!(reply.status, 204);
    assert!(reply.header("set-cookie").unwrap().contains("; Secure"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_bad_frames_close_the_socket() {
    let web = Web::start();
    let cookie = web.login();
    let mut tab = Tab::open(&web, &cookie).await;
    tab.ws
        .send(Message::text("x".repeat(128 * 1024 + 1)))
        .await
        .unwrap();
    assert_eq!(tab.close_code().await, Some(1009));
    let mut tab = Tab::open(&web, &cookie).await;
    tab.ws.send(Message::text("not json")).await.unwrap();
    assert_eq!(tab.close_code().await, Some(1003));
    let mut tab = Tab::open(&web, &cookie).await;
    tab.ws.send(Message::binary(vec![0x02; 16])).await.unwrap();
    assert_eq!(tab.close_code().await, Some(1003));
    let mut tab = Tab::open(&web, &cookie).await;
    tab.ws
        .send(Message::binary(vec![0x02; 17 + 64 * 1024 + 1]))
        .await
        .unwrap();
    assert_eq!(tab.close_code().await, Some(1009));
}

// ── The hub: a fake claude, hooks, sessions ──────────────────────────────────

/// A fake claude for the web tests: answers --version/--help, saves its
/// --settings JSON (the hook commands) to `.fake-settings.json` in its cwd,
/// optionally turns bracketed paste on, prints a banner, then logs every
/// input line to `.fake-input.log`, answers `size` with `stty size` and
/// echoes any other line as `line:<text>`.
fn write_web_fake_claude(h: &ManagerHarness, bracketed_paste: bool) {
    let bp = if bracketed_paste {
        r"printf '\033[?2004h'"
    } else {
        ":"
    };
    let script = format!(
        r#"#!/bin/sh
[ "$1" = "--version" ] && echo 'claude 0.0.0-fake' && exit 0
[ "$1" = "--help" ] && exit 0
printf '%s' "$2" > .fake-settings.json
{bp}
echo "FAKE_CLAUDE_BANNER pid=$$"
while IFS= read -r line; do
  printf '%s\n' "$line" >> .fake-input.log
  case "$line" in
    size) stty size ;;
    *) printf 'line:%s\n' "$line" ;;
  esac
done
"#
    );
    fs::write(&h.fake_claude, script).unwrap();
    fs::set_permissions(&h.fake_claude, fs::Permissions::from_mode(0o755)).unwrap();
}

/// Run the hook command for `event` of the session started in `dir` (from
/// the settings its fake claude saved) with `payload` on stdin, as claude
/// does.
fn fire_hook(dir: &std::path::Path, event: &str, payload: Value) {
    let path = dir.join(".fake-settings.json");
    let deadline = std::time::Instant::now() + WAIT;
    let settings: Value = loop {
        if let Some(v) = fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
        {
            break v;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no settings in {dir:?}"
        );
        thread::sleep(std::time::Duration::from_millis(50));
    };
    let command = settings["hooks"][event][0]["hooks"][0]["command"]
        .as_str()
        .expect("a hook command")
        .to_owned();
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(&command)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.to_string().as_bytes())
        .unwrap();
    assert!(child.wait().unwrap().success());
}

/// `dir/.fake-input.log` once it contains `needle`.
fn input_log_until(dir: &std::path::Path, needle: &str) -> String {
    let deadline = std::time::Instant::now() + WAIT;
    loop {
        let log = fs::read_to_string(dir.join(".fake-input.log")).unwrap_or_default();
        if log.contains(needle) {
            return log;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{needle:?} not in {log:?}"
        );
        thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// The name claude gives `dir`'s project directory (the rule of
/// `claude::projects::encode_cwd`, on the canonical path).
fn project_dir_name(dir: &std::path::Path) -> String {
    fs::canonicalize(dir)
        .unwrap()
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect()
}

impl Tab {
    /// Wait until `host` is connected (the snapshot may already say so).
    async fn connected(&mut self, host: &str) {
        let in_snapshot = self.snapshot["hosts"].as_array().is_some_and(|hs| {
            hs.iter()
                .any(|h| h["name"] == host && h["connected"] == true)
        });
        if !in_snapshot {
            let ev = self
                .event("host", |v| {
                    v["host"]["name"] == host && v["host"]["connected"] == true
                })
                .await;
            // Remembered, so a second wait on this tab returns at once.
            match self.snapshot["hosts"].as_array_mut() {
                Some(hosts) => {
                    hosts.retain(|h| h["name"] != host);
                    hosts.push(ev["host"].clone());
                }
                None => self.snapshot["hosts"] = json!([ev["host"]]),
            }
        }
    }

    /// Spawn a direct claude session in `dir` at 24×80 and attach to it; its sid.
    async fn spawn_attached(&mut self, dir: &std::path::Path) -> String {
        self.connected("local").await;
        let reply = self
            .req(
                "session.spawn",
                json!({"host": "local", "cwd": dir, "rows": 24, "cols": 80, "proxy": {"kind": "direct"}}),
            )
            .await
            .expect("spawn");
        let sid = reply["sid"].as_str().unwrap().to_owned();
        self.req(
            "session.attach",
            json!({"sid": sid, "rows": 24, "cols": 80}),
        )
        .await
        .expect("attach");
        let attached = self.event("attached", |v| v["sid"] == sid.as_str()).await;
        assert_eq!(
            (attached["rows"].as_u64(), attached["cols"].as_u64()),
            (Some(24), Some(80))
        );
        self.output_until(&sid, "FAKE_CLAUDE_BANNER").await;
        sid
    }
}

// ── AC4: attach, snapshot, input, resize ─────────────────────────────────────

/// AC4: attach → `attached` + the snapshot frame; input reaches the PTY;
/// a resize reaches it too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_attach_snapshot_input_and_resize() {
    let h = ManagerHarness::new();
    write_web_fake_claude(&h, false);
    let dir = h.dirs[0].clone();
    let web = Web::start_with(h, &[], |_| {});
    let cookie = web.login();
    let mut tab = Tab::open(&web, &cookie).await;
    let sid = tab.spawn_attached(&dir).await;
    tab.input(&sid, b"ping-web\r").await;
    tab.output_until(&sid, "line:ping-web").await;
    tab.req(
        "session.resize",
        json!({"sid": sid, "rows": 30, "cols": 100}),
    )
    .await
    .unwrap();
    tab.input(&sid, b"size\r").await;
    tab.output_until(&sid, "30 100").await;
}

// ── AC7: the approve guard ───────────────────────────────────────────────────

/// AC7: approve only for a permission prompt at the current seq.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_approve_is_guarded_by_state_kind_and_seq() {
    let h = ManagerHarness::new();
    write_web_fake_claude(&h, false);
    let dir = h.dirs[0].clone();
    let web = Web::start_with(h, &[], |_| {});
    let cookie = web.login();
    let mut tab = Tab::open(&web, &cookie).await;
    let sid = tab.spawn_attached(&dir).await;
    fire_hook(
        &dir,
        "PreToolUse",
        json!({"tool_name": "Bash", "tool_input": {"command": "ls -la"}}),
    );
    fire_hook(
        &dir,
        "Notification",
        json!({"notification_type": "permission_prompt", "message": "Claude needs your permission to use Bash"}),
    );
    let ev = tab
        .event("session", |v| {
            v["session"]["id"] == sid.as_str()
                && v["session"]["state"] == "needs_approval"
                && v["session"]["attention"]["kind"] == "permission_prompt"
        })
        .await;
    let a = &ev["session"]["attention"];
    assert_eq!(a["tool"], "Bash");
    assert_eq!(a["detail"], "ls -la");
    assert_eq!(a["message"], "Claude needs your permission to use Bash");
    let seq = a["seq"].as_u64().unwrap();
    let approve = |s: u64| json!({"sid": sid, "decision": "approve", "attention_seq": s});
    assert_eq!(
        tab.req("session.approve", approve(seq + 1)).await,
        Err("stale".to_owned())
    );
    assert_eq!(
        tab.req("session.approve", approve(seq - 1)).await,
        Err("stale".to_owned())
    );
    tab.input(&sid, b"abc").await;
    tab.req("session.approve", approve(seq))
        .await
        .expect("approve");
    input_log_until(&dir, "abc\n"); // the Enter reached the PTY and ended the line
    fire_hook(
        &dir,
        "Notification",
        json!({"notification_type": "elicitation_dialog", "message": "pick one"}),
    );
    let ev = tab
        .event("session", |v| {
            v["session"]["id"] == sid.as_str()
                && v["session"]["attention"]["kind"] == "elicitation_dialog"
        })
        .await;
    let seq = ev["session"]["attention"]["seq"].as_u64().unwrap();
    assert_eq!(
        tab.req("session.approve", approve(seq)).await,
        Err("not a permission prompt".to_owned())
    );
    let unknown =
        json!({"sid": uuid::Uuid::new_v4().to_string(), "decision": "deny", "attention_seq": 1});
    assert_eq!(
        tab.req("session.approve", unknown).await,
        Err("no such session".to_owned())
    );
}

// ── Paste, shared attach, lifecycle, argument checks ─────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_paste_reaches_the_pty_bracketed() {
    let h = ManagerHarness::new();
    write_web_fake_claude(&h, false);
    let dir = h.dirs[0].clone();
    let web = Web::start_with(h, &[], |_| {});
    let cookie = web.login();
    let mut tab = Tab::open(&web, &cookie).await;
    let sid = tab.spawn_attached(&dir).await;
    tab.req(
        "session.paste",
        json!({"sid": sid, "text": "hello\u{1b}[201~ web", "submit": true}),
    )
    .await
    .expect("paste");
    input_log_until(&dir, "\u{1b}[200~hello web\u{1b}[201~");
    // 64 KiB + 1 of text fits a client text frame (128 KiB, Ruling R19):
    // the paste cap answers.
    let big = "x".repeat(64 * 1024 + 1);
    assert_eq!(
        tab.req(
            "session.paste",
            json!({"sid": sid, "text": big, "submit": false})
        )
        .await,
        Err("too large".to_owned())
    );
    // A frame past 128 KiB still closes the socket.
    let huge = json!({"t": "req", "id": 99, "op": "session.paste", "sid": sid,
        "text": "x".repeat(128 * 1024), "submit": false});
    tab.ws.send(Message::text(huge.to_string())).await.unwrap();
    assert_eq!(tab.close_code().await, Some(1009));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_two_tabs_share_one_attach() {
    let h = ManagerHarness::new();
    write_web_fake_claude(&h, false);
    let dir = h.dirs[0].clone();
    let web = Web::start_with(h, &[], |_| {});
    let cookie = web.login();
    let mut a = Tab::open(&web, &cookie).await;
    let sid = a.spawn_attached(&dir).await;
    let mut b = Tab::open(&web, &cookie).await;
    b.req(
        "session.attach",
        json!({"sid": sid, "rows": 40, "cols": 100}),
    )
    .await
    .expect("attach b");
    for tab in [&mut a, &mut b] {
        let ev = tab.event("attached", |v| v["sid"] == sid.as_str()).await;
        assert_eq!(
            (ev["rows"].as_u64(), ev["cols"].as_u64()),
            (Some(24), Some(80)),
            "no resize for the second subscriber"
        );
    }
    b.input(&sid, b"from-b\r").await;
    a.output_until(&sid, "line:from-b").await;
    b.output_until(&sid, "line:from-b").await;
    b.req("session.detach", json!({"sid": sid}))
        .await
        .expect("detach b");
    a.input(&sid, b"still-a\r").await;
    a.output_until(&sid, "line:still-a").await;
    a.input(&sid, b"size\r").await;
    a.output_until(&sid, "24 80").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_session_lifecycle_shell_rename_kill() {
    let h = ManagerHarness::new();
    write_web_fake_claude(&h, false);
    let dir = h.dirs[0].clone();
    let web = Web::start_with(h, &[], |_| {});
    let cookie = web.login();
    let mut tab = Tab::open(&web, &cookie).await;
    tab.connected("local").await;
    let reply = tab
        .req(
            "session.shell",
            json!({"host": "local", "cwd": dir, "rows": 24, "cols": 80}),
        )
        .await
        .expect("shell");
    let sid = reply["sid"].as_str().unwrap().to_owned();
    let ev = tab
        .event("session", |v| v["session"]["id"] == sid.as_str())
        .await;
    assert_eq!(ev["session"]["kind"], "shell");
    assert_eq!(ev["session"]["label"], "term");
    tab.req("session.rename", json!({"sid": sid, "name": "renamed"}))
        .await
        .expect("rename");
    tab.event("session", |v| {
        v["session"]["id"] == sid.as_str()
            && v["session"]["name"] == "renamed"
            && v["session"]["label"] == "renamed"
    })
    .await;
    tab.req("session.rename", json!({"sid": sid, "name": null}))
        .await
        .expect("clear the name");
    tab.event("session", |v| {
        v["session"]["id"] == sid.as_str()
            && v["session"]["name"].is_null()
            && v["session"]["label"] == "term"
    })
    .await;
    tab.req("session.kill", json!({"sid": sid}))
        .await
        .expect("kill");
    tab.event("session_removed", |v| v["sid"] == sid.as_str())
        .await;
    assert_eq!(
        tab.req("session.kill", json!({"sid": sid})).await,
        Err("no such session".to_owned())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_refuses_unknown_ids_and_bad_args() {
    let h = ManagerHarness::new();
    write_web_fake_claude(&h, false);
    let dir = h.dirs[0].clone();
    let web = Web::start_with(h, &[], |_| {});
    let cookie = web.login();
    let mut tab = Tab::open(&web, &cookie).await;
    tab.connected("local").await;
    let random = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        tab.req("session.kill", json!({"sid": random})).await,
        Err("no such session".to_owned())
    );
    let err = tab
        .req(
            "session.attach",
            json!({"sid": random, "rows": 0, "cols": 80}),
        )
        .await
        .unwrap_err();
    assert!(err.starts_with("bad request"), "{err}");
    let err = tab
        .req(
            "session.spawn",
            json!({"host": "local", "cwd": dir, "rows": 24, "cols": 80,
                "proxy": {"kind": "direct"}, "bogus": 1}),
        )
        .await
        .unwrap_err();
    assert!(err.starts_with("bad request"), "{err}");
    assert_eq!(
        tab.req("hosts.connect", json!({"host": "local"})).await,
        Err("local is always connected".to_owned())
    );
    assert_eq!(
        tab.req("hosts.disconnect", json!({"host": "local"})).await,
        Err("cannot disconnect local".to_owned())
    );
    assert!(tab
        .req("hosts.connect", json!({"host": "-oProxyCommand=x"}))
        .await
        .is_err());
    assert_eq!(
        tab.req("fs.list", json!({"host": "nohost", "path": "/"}))
            .await,
        Err("no such host".to_owned())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_opening_prompt_is_pasted_after_bracketed_paste_mode() {
    let h = ManagerHarness::new();
    write_web_fake_claude(&h, true);
    let dir = h.dirs[0].clone();
    let web = Web::start_with(h, &[], |_| {});
    let cookie = web.login();
    let mut tab = Tab::open(&web, &cookie).await;
    tab.connected("local").await;
    let reply = tab
        .req(
            "session.spawn",
            json!({"host": "local", "cwd": dir, "rows": 24, "cols": 80,
                "proxy": {"kind": "direct"}, "prompt": "first task"}),
        )
        .await
        .expect("spawn");
    let sid = reply["sid"].as_str().unwrap().to_owned();
    tab.event("session", |v| {
        v["session"]["id"] == sid.as_str() && v["session"]["prompt_pending"] == true
    })
    .await;
    let log = fs::read_to_string(dir.join(".fake-input.log")).unwrap_or_default();
    assert!(!log.contains("first task"), "pasted too early: {log:?}");
    fire_hook(
        &dir,
        "SessionStart",
        json!({"session_id": "c1", "source": "startup"}),
    );
    input_log_until(&dir, "\u{1b}[200~first task\u{1b}[201~");
    tab.event("session", |v| {
        v["session"]["id"] == sid.as_str() && v["session"]["prompt_pending"] == false
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_tab_limit_is_16() {
    let web = Web::start();
    let cookie = web.login();
    let mut tabs = Vec::new();
    for _ in 0..16 {
        tabs.push(Tab::open(&web, &cookie).await);
    }
    assert_eq!(web.ws_with(&cookie, &web.origin()).await.err(), Some(503));
    let mut first = tabs.remove(0);
    first.ws.close(None).await.unwrap();
    drop(first);
    let deadline = std::time::Instant::now() + WAIT;
    loop {
        match web.ws_with(&cookie, &web.origin()).await {
            Ok(_) => break,
            Err(503) if std::time::Instant::now() < deadline => {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await
            }
            Err(code) => panic!("no room after a close: {code}"),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_git_status_and_worktree_diff() {
    let h = ManagerHarness::new();
    write_web_fake_claude(&h, false);
    let dir = h.dirs[0].clone();
    common::run_git(&dir, &["init", "-q"]);
    fs::write(dir.join("a.txt"), "one\n").unwrap();
    common::run_git(&dir, &["add", "a.txt"]);
    common::run_git(&dir, &["commit", "-q", "-m", "first"]);
    fs::write(dir.join("a.txt"), "one\nchanged\n").unwrap();
    let web = Web::start_with(h, &[], |_| {});
    let cookie = web.login();
    let mut tab = Tab::open(&web, &cookie).await;
    let sid = tab.spawn_attached(&dir).await;
    let status = tab
        .req("git.status", json!({"sid": sid}))
        .await
        .expect("git.status");
    let files = status["files"].as_array().expect("files");
    assert!(
        files
            .iter()
            .any(|f| f["path"] == "a.txt" && f["worktree"] == "M"),
        "{status}"
    );
    let patch = tab
        .req(
            "git.worktree_diff",
            json!({"sid": sid, "path": "a.txt", "staged": false}),
        )
        .await
        .expect("git.worktree_diff");
    assert!(
        patch["patch"]
            .as_str()
            .unwrap_or_default()
            .contains("+changed"),
        "{patch}"
    );
    assert_eq!(
        tab.req(
            "git.worktree_diff",
            json!({"sid": sid, "path": "../x", "staged": false})
        )
        .await,
        Err("no such path".to_owned())
    );
    let log = tab
        .req("git.log", json!({"sid": sid}))
        .await
        .expect("git.log");
    assert!(!log["commits"].as_array().unwrap().is_empty(), "{log}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_reader_and_search_read_a_fixture_transcript() {
    let h = ManagerHarness::new();
    write_web_fake_claude(&h, false);
    let dir = fs::canonicalize(&h.dirs[0]).unwrap();
    let cwd = dir.to_string_lossy().into_owned();
    let project = h
        .root
        .join("claude-config/projects")
        .join(project_dir_name(&dir));
    fs::create_dir_all(&project).unwrap();
    let records = [
        json!({"type": "user", "cwd": cwd, "message": {"role": "user", "content": "first question"}}),
        json!({"type": "assistant", "cwd": cwd, "message": {"role": "assistant",
            "content": [{"type": "text", "text": "The first answer."}]}}),
        json!({"type": "user", "cwd": cwd, "message": {"role": "user", "content": "second question"}}),
        json!({"type": "assistant", "cwd": cwd, "message": {"role": "assistant",
            "content": [{"type": "text", "text": "The second answer has fixtureword."}]}}),
    ];
    let jsonl: String = records.iter().map(|r| format!("{r}\n")).collect();
    fs::write(project.join("conv-1.jsonl"), jsonl).unwrap();
    let web = Web::start_with(h, &[], |_| {});
    let cookie = web.login();
    let mut tab = Tab::open(&web, &cookie).await;
    let sid = tab.spawn_attached(&dir).await;
    fire_hook(
        &dir,
        "SessionStart",
        json!({"session_id": "conv-1", "source": "resume"}),
    );
    tab.event("session", |v| {
        v["session"]["id"] == sid.as_str() && v["session"]["claude_session_id"] == "conv-1"
    })
    .await;
    let answers = tab
        .req("reader.answers", json!({"sid": sid}))
        .await
        .expect("reader.answers");
    let answers = answers["answers"].as_array().expect("answers").clone();
    assert_eq!(answers.len(), 2, "{answers:?}");
    assert_eq!(answers[0]["prompt"], "second question");
    assert_eq!(answers[1]["prompt"], "first question");
    let found = tab
        .req("search", json!({"query": "fixtureword"}))
        .await
        .expect("search");
    assert_eq!(found["failed"], json!([]), "{found}");
    assert!(
        found["hits"]
            .as_array()
            .unwrap()
            .iter()
            .any(|hit| hit["host"] == "local" && hit["conversation"] == "conv-1"),
        "{found}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_hosts_report_daemon_features() {
    let h = ManagerHarness::new();
    write_web_fake_claude(&h, false);
    let web = Web::start_with(h, &[], |_| {});
    let cookie = web.login();
    let mut tab = Tab::open(&web, &cookie).await;
    tab.connected("local").await;
    let hosts = tab.req("hosts.list", json!({})).await.expect("hosts.list");
    let local = hosts["hosts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|h| h["name"] == "local")
        .cloned()
        .expect("local");
    assert_eq!(local["connected"], true);
    let features = local["features"].as_array().unwrap();
    for f in ["attention", "worktree"] {
        assert!(features.iter().any(|x| x == f), "{local}");
    }
    // The Welcome carries claude's raw `--version` line.
    assert_eq!(local["claude"]["version"], "claude 0.0.0-fake");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_order_and_layout_persist_in_web_state() {
    let h = ManagerHarness::new();
    write_web_fake_claude(&h, false);
    let dirs = h.dirs.clone();
    let web = Web::start_with(h, &[], |_| {});
    let cookie = web.login();
    let mut a = Tab::open(&web, &cookie).await;
    let mut b = Tab::open(&web, &cookie).await;
    let sa = a.spawn_attached(&dirs[0]).await;
    let sb = a.spawn_attached(&dirs[1]).await;
    a.req("session.order", json!({"sids": [sb, sa]}))
        .await
        .expect("order");
    let ev = b.event("order", |_| true).await;
    assert_eq!(ev["sids"], json!([sb, sa]));
    let layout = json!({"grid": 2, "panes": [sa, null], "focus": 0});
    a.req("layout.save", json!({"layout": layout}))
        .await
        .expect("layout.save");
    let ev = b.event("layout", |_| true).await;
    assert_eq!(ev["layout"], layout);
    let err = a
        .req(
            "layout.save",
            json!({"layout": {"grid": 3, "panes": [null, null, null], "focus": 0}}),
        )
        .await
        .unwrap_err();
    assert!(err.starts_with("bad request"), "{err}");
    drop((a, b));
    let h = web.stop();
    let web = Web::start_with(h, &[], |_| {});
    let cookie = web.login();
    let tab = Tab::open(&web, &cookie).await;
    assert_eq!(tab.snapshot["layout"], layout);
    assert_eq!(tab.snapshot["order"], json!([sb, sa]));
    let state = web.h().config_home.join("claudio/web-state.json");
    assert_eq!(
        fs::metadata(&state).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(
        !web.h().config_home.join("claudio/state.json").exists(),
        "the web never writes the TUI's state.json"
    );
}

// ── Spawn composition: proxy env, plugins, extensions, snippets ──────────────

const PLUGIN: &str = "vocdoni-test";
const PROXY_TOKEN: &str = "tok-SECRET-proxy-7f3a9c";
const PLUGIN_SECRET: &str = "plugin-SECRET-value-51d2e8";
const MANIFEST: &str = r#"format = 1
name = "vocdoni-test"
version = "1.0.0"
description = "Test setup"
instructions = "Manifest instructions for the test."

[[mcp]]
name = "kb"
type = "http"
url = "https://kb.example.org/mcp"
headers = { Authorization = "Bearer ${VOCDONI_TEST_TOKEN}" }
secrets = ["VOCDONI_TEST_TOKEN"]

[[agent]]
name = "reviewer"
description = "Reviews"
prompt = "Review the change."
"#;

/// A fake claude-proxy on loopback: `/v1/claudio/config`, `/plugins`
/// (PLUGIN, whose env holds PLUGIN_SECRET), the manifest, `/me/stats`
/// (any period) and `/pool/health`; anything else 404. Its `host:port`.
fn fake_proxy() -> String {
    use sha2::Digest;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind fake proxy");
    let addr = listener.local_addr().unwrap().to_string();
    let sha = format!("{:x}", sha2::Sha256::digest(MANIFEST.as_bytes()));
    let plugins = json!({"plugins": [{
        "name": PLUGIN, "version": "1.0.0",
        "bundle": {"url": format!("http://{addr}/{PLUGIN}.toml"), "sha256": sha},
        "env": {"VOCDONI_TEST_TOKEN": PLUGIN_SECRET}}]})
    .to_string();
    let stats = json!({"version": 1, "user_name": "tester", "period": "24h",
        "totals": {"requests": 12, "errors": 1, "input_tokens": 3400, "output_tokens": 5600, "cache_read": 0, "cache_creation": 0},
        "by_model": [{"model": "claude-opus-5-5", "requests": 12, "errors": 1, "input_tokens": 3400, "output_tokens": 5600, "cache_read": 0, "cache_creation": 0}],
        "limit": null})
    .to_string();
    let config = json!({"version": 1, "env": {}}).to_string();
    let pool = json!({"version": 1, "providers": [{"name": "anthropic", "status": "ok"}]}).to_string();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap_or(0) == 1 {
                head.push(byte[0]);
            }
            let line = String::from_utf8_lossy(&head).into_owned();
            let path = line
                .split(' ')
                .nth(1)
                .unwrap_or("")
                .split('?')
                .next()
                .unwrap_or("");
            let (status, reply) = match path {
                "/v1/claudio/config" => ("200 OK", config.as_str()),
                "/v1/claudio/plugins" => ("200 OK", plugins.as_str()),
                "/v1/claudio/me/stats" => ("200 OK", stats.as_str()),
                "/v1/claudio/pool/health" => ("200 OK", pool.as_str()),
                p if p == format!("/{PLUGIN}.toml") => ("200 OK", MANIFEST),
                _ => ("404 Not Found", "{}"),
            };
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                reply.len()
            );
        }
    });
    addr
}

/// A fake claude that dumps its env to `env_file` (never to the terminal),
/// lists `--agents`/`--mcp-config` in `--help`, then prints the banner and
/// echoes lines.
fn write_env_fake_claude(h: &ManagerHarness, env_file: &std::path::Path) {
    let env_path = env_file.display();
    let script = format!(
        r#"#!/bin/sh
[ "$1" = "--version" ] && echo 'claude 0.0.0-fake' && exit 0
if [ "$1" = "--help" ]; then printf '  --agents <json>  Agents\n  --mcp-config <configs...>  MCP\n'; exit 0; fi
env > "{env_path}.tmp" && mv "{env_path}.tmp" "{env_path}"
printf '%s' "$2" > .fake-settings.json
echo "FAKE_CLAUDE_BANNER pid=$$"
while IFS= read -r line; do printf 'line:%s\n' "$line"; done
"#
    );
    fs::write(&h.fake_claude, script).unwrap();
    fs::set_permissions(&h.fake_claude, fs::Permissions::from_mode(0o755)).unwrap();
}

/// Poll `op` until `pick` accepts its data.
async fn until_req(tab: &mut Tab, op: &str, args: Value, pick: impl Fn(&Value) -> bool) -> Value {
    let deadline = std::time::Instant::now() + DAEMON_WAIT;
    loop {
        if let Ok(data) = tab.req(op, args.clone()).await {
            if pick(&data) {
                return data;
            }
        }
        assert!(std::time::Instant::now() < deadline, "{op} never satisfied");
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

/// The env the fake claude of the next launch dumps to `env_file`
/// (removed first by the caller).
async fn launched_env(env_file: &std::path::Path) -> String {
    let deadline = std::time::Instant::now() + WAIT;
    loop {
        if let Ok(env) = fs::read_to_string(env_file) {
            return env;
        }
        assert!(std::time::Instant::now() < deadline, "claude never started");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// `claudio web` with the fake proxy as CLAUDIO_PROXY_URL (profile `env`).
fn web_with_proxy(h: ManagerHarness, proxy: &str) -> Web {
    let url = format!("{PROXY_TOKEN}@{proxy}");
    Web::start_with(h, &[], |cmd| {
        cmd.env("CLAUDIO_PROXY_URL", url);
        cmd.env("CLAUDIO_TEST_LOOPBACK_HTTP", "1");
    })
}

/// AC3: the proxy token and a plugin's secret reach claude's env and never
/// the browser (no WS frame, no HTTP body) nor the server's log.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_spawn_keeps_proxy_and_plugin_secrets_out_of_the_browser() {
    let h = ManagerHarness::new();
    let env_file = h.root.join("claude-env.txt");
    write_env_fake_claude(&h, &env_file);
    let config = h.config_home.join("claudio/config.toml");
    let mut text = fs::read_to_string(&config).unwrap();
    text.push_str(&format!(
        "\n[plugins.{PLUGIN}]\ndisabled = []\nreviewed = \"1.0.0\"\n"
    ));
    fs::write(&config, text).unwrap();
    let proxy = fake_proxy();
    let dir = h.dirs[0].clone();
    let web = Web::start_with(h, &[], |cmd| {
        cmd.env("CLAUDIO_PROXY_URL", format!("{PROXY_TOKEN}@{proxy}"));
        cmd.env("CLAUDIO_TEST_LOOPBACK_HTTP", "1");
    });
    let cookie = web.login();
    let mut tab = Tab::open(&web, &cookie).await;
    assert_eq!(tab.snapshot["profiles"]["default"], "env");
    tab.connected("local").await;
    until_req(&mut tab, "plugins.status", json!({}), |d| {
        d["status"].as_array().is_some_and(|s| {
            s.iter()
                .any(|p| p["host"] == "local" && p["name"] == PLUGIN && p["state"] == "ready")
        })
    })
    .await;
    let reply = tab
        .req(
            "session.spawn",
            json!({"host": "local", "cwd": dir, "rows": 24, "cols": 80, "proxy": {"kind": "default"}}),
        )
        .await
        .expect("spawn");
    let sid = reply["sid"].as_str().unwrap().to_owned();
    let deadline = std::time::Instant::now() + WAIT;
    let env = loop {
        if let Ok(env) = fs::read_to_string(&env_file) {
            break env;
        }
        assert!(std::time::Instant::now() < deadline, "claude never started");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };
    assert!(
        env.contains(&format!("ANTHROPIC_AUTH_TOKEN={PROXY_TOKEN}")),
        "the proxy token reaches claude"
    );
    assert!(
        env.contains(&format!("VOCDONI_TEST_TOKEN={PLUGIN_SECRET}")),
        "the plugin secret reaches claude"
    );

    // Everything a browser can see about this session and its plugins.
    tab.req("session.attach", json!({"sid": sid, "rows": 24, "cols": 80}))
        .await
        .unwrap();
    tab.output_until(&sid, "FAKE_CLAUDE_BANNER").await;
    for (op, args) in [
        ("hosts.list", json!({})),
        ("plugins.list", json!({})),
        ("plugins.items", json!({"name": PLUGIN})),
        ("plugins.status", json!({})),
        ("extensions.list", json!({})),
        ("proxy.stats", json!({"profile": "env", "window": "24h"})),
    ] {
        tab.req(op, args)
            .await
            .unwrap_or_else(|e| panic!("{op}: {e}"));
    }
    let _ = tab.req("proxy.credential", json!({"sid": sid})).await; // no conversation yet: an error
    let second = Tab::open(&web, &cookie).await; // a snapshot with the session in it
    for frame in tab.seen.iter().chain(second.seen.iter()) {
        let text = String::from_utf8_lossy(frame);
        assert!(
            !text.contains(PROXY_TOKEN),
            "the proxy token in a frame: {text}"
        );
        assert!(
            !text.contains(PLUGIN_SECRET),
            "the plugin secret in a frame: {text}"
        );
    }
    for path in ["/", "/index.html"] {
        let body = web.get(path, &[("Accept-Encoding", "identity")]).body;
        assert!(!body.contains(PROXY_TOKEN) && !body.contains(PLUGIN_SECRET));
    }
    let log = fs::read_to_string(web.h().root.join("web.log")).unwrap();
    assert!(
        !log.contains(PROXY_TOKEN) && !log.contains(PLUGIN_SECRET),
        "a secret in the server log"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_respawn_without_a_known_proxy_asks() {
    let h = ManagerHarness::new();
    write_web_fake_claude(&h, false);
    let dir = h.dirs[0].clone();
    let web = Web::start_with(h, &[], |_| {});
    let cookie = web.login();
    let mut tab = Tab::open(&web, &cookie).await;
    let sid = tab.spawn_attached(&dir).await;
    drop(tab);
    let h = web.stop();
    let state = h.config_home.join("claudio/web-state.json");
    fs::remove_file(&state).unwrap();
    assert!(!h.config_home.join("claudio/state.json").exists());
    let web = Web::start_with(h, &[], |_| {});
    let cookie = web.login();
    let mut tab = Tab::open(&web, &cookie).await;
    tab.connected("local").await;
    let ask = json!({"sid": sid, "fresh": false, "rows": 24, "cols": 80});
    let deadline = std::time::Instant::now() + WAIT;
    let err = loop {
        match tab.req("session.respawn", ask.clone()).await {
            Err(e) if e == "no such session" => {
                assert!(std::time::Instant::now() < deadline, "the session never came back");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            other => break other,
        }
    };
    assert_eq!(err, Err("proxy unknown".to_owned()));
    let mut direct = ask.clone();
    direct["proxy"] = json!({"kind": "direct"});
    let reply = tab.req("session.respawn", direct).await.expect("respawn");
    assert_eq!(reply["sid"], sid.as_str());
    tab.event("session", |v| v["session"]["id"] == sid.as_str())
        .await;
    let saved: Value = serde_json::from_slice(&fs::read(&state).unwrap()).unwrap();
    let meta = &saved["sessions"][sid.as_str()];
    assert_eq!(meta["host"], "local", "{saved}");
    assert!(meta["proxy"].is_null(), "{saved}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_plugin_without_choices_is_left_out_until_saved() {
    let h = ManagerHarness::new();
    let env_file = h.root.join("claude-env.txt");
    write_env_fake_claude(&h, &env_file);
    let dirs = h.dirs.clone();
    let proxy = fake_proxy();
    let web = web_with_proxy(h, &proxy);
    let cookie = web.login();
    let mut tab = Tab::open(&web, &cookie).await;
    tab.connected("local").await;
    let list = until_req(&mut tab, "plugins.list", json!({}), |d| {
        d["plugins"][0]["manifest"] == "ready"
    })
    .await;
    assert_eq!(list["plugins"][0]["name"], PLUGIN);
    assert_eq!(list["plugins"][0]["choices_saved"], false);
    let spawn = |dir: &std::path::PathBuf, skip: Option<&str>| {
        let mut args = json!({"host": "local", "cwd": dir, "rows": 24, "cols": 80,
            "proxy": {"kind": "default"}});
        if let Some(skip) = skip {
            args["skip_plugins"] = json!([skip]);
        }
        args
    };
    tab.req("session.spawn", spawn(&dirs[0], None))
        .await
        .expect("spawn");
    let env = launched_env(&env_file).await;
    assert!(env.contains(&format!("ANTHROPIC_AUTH_TOKEN={PROXY_TOKEN}")));
    assert!(!env.contains(PLUGIN_SECRET), "no choices: left out");

    tab.req(
        "plugins.save",
        json!({"name": PLUGIN, "disabled": ["agent:reviewer"]}),
    )
    .await
    .expect("save");
    let config = fs::read_to_string(web.h().config_home.join("claudio/config.toml")).unwrap();
    assert!(config.contains(&format!("[plugins.{PLUGIN}]")), "{config}");
    assert!(
        config.contains("disabled = [\"agent:reviewer\"]"),
        "{config}"
    );
    until_req(&mut tab, "plugins.status", json!({}), |d| {
        d["status"].as_array().is_some_and(|s| {
            s.iter()
                .any(|p| p["host"] == "local" && p["name"] == PLUGIN && p["state"] == "ready")
        })
    })
    .await;
    let items = tab
        .req("plugins.items", json!({"name": PLUGIN}))
        .await
        .unwrap();
    let reviewer = items["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == "agent:reviewer")
        .cloned()
        .unwrap();
    assert_eq!(reviewer["checked"], false);

    fs::remove_file(&env_file).unwrap();
    tab.req("session.spawn", spawn(&dirs[1], None))
        .await
        .expect("spawn with the plugin");
    let env = launched_env(&env_file).await;
    assert!(
        env.contains(&format!("VOCDONI_TEST_TOKEN={PLUGIN_SECRET}")),
        "saved: the plugin's secret is in"
    );

    fs::remove_file(&env_file).unwrap();
    tab.req("session.spawn", spawn(&dirs[0], Some(PLUGIN)))
        .await
        .expect("spawn skipping the plugin");
    let env = launched_env(&env_file).await;
    assert!(env.contains(&format!("ANTHROPIC_AUTH_TOKEN={PROXY_TOKEN}")));
    assert!(!env.contains(PLUGIN_SECRET), "skip_plugins leaves it out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_extension_toggle_edits_config_and_is_reread() {
    let web = Web::start();
    let cookie = web.login();
    let mut tab = Tab::open(&web, &cookie).await;
    let diagram = |list: &Value| {
        list["extensions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == "diagram")
            .cloned()
            .unwrap()
    };
    tab.req("extensions.toggle", json!({"name": "diagram", "on": false}))
        .await
        .expect("toggle");
    let path = web.h().config_home.join("claudio/config.toml");
    let config = fs::read_to_string(&path).unwrap();
    let section = config.split("[extensions]").nth(1).expect("[extensions]");
    assert!(section.contains("diagram = false"), "{config}");
    tab.event("extensions_changed", |_| true).await;
    let list = tab.req("extensions.list", json!({})).await.unwrap();
    assert_eq!(diagram(&list)["enabled"], false);
    assert_eq!(diagram(&list)["source"], "builtin");

    fs::write(&path, config.replace("diagram = false", "diagram = true")).unwrap();
    let edited = std::time::Instant::now();
    tab.event("extensions_changed", |_| true).await;
    assert!(
        edited.elapsed() < std::time::Duration::from_secs(7),
        "re-read within 7 s"
    );
    let list = tab.req("extensions.list", json!({})).await.unwrap();
    assert_eq!(diagram(&list)["enabled"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_snippets_save_and_list() {
    let web = Web::start();
    let cookie = web.login();
    let mut tab = Tab::open(&web, &cookie).await;
    let saved = tab
        .req("snippets.save", json!({"text": "run the tests"}))
        .await
        .expect("save");
    let name = saved["name"].as_str().unwrap().to_owned();
    let list = tab.req("snippets.list", json!({})).await.unwrap();
    assert!(
        list["snippets"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["name"] == name.as_str() && s["text"] == "run the tests"),
        "{list}"
    );
    let dir = web.h().config_home.join("claudio/snippets");
    assert!(dir.join(format!("{name}.md")).is_file());
    assert_eq!(
        tab.req("snippets.save", json!({"text": "x".repeat(64 * 1024 + 1)}))
            .await,
        Err("bad request: text: at most 64 KiB".to_owned())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_external_extension_action_runs_its_program() {
    let h = ManagerHarness::new();
    let dir = h.config_home.join("claudio/extensions/hello");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("extension.toml"),
        r#"name = "hello"
description = "says hi"
command = ["./hello.sh"]
[[action]]
key = "h"
title = "say hi"
id = "greet"
"#,
    )
    .unwrap();
    let program = dir.join("hello.sh");
    fs::write(
        &program,
        "#!/bin/sh\n\
         while IFS= read -r line; do\n\
         id=$(printf '%s' \"$line\" | sed 's/^{\"id\":\\([0-9]*\\).*/\\1/')\n\
         printf '{\"id\":%s,\"result\":{\"kind\":\"notice\",\"text\":\"hi\"}}\\n' \"$id\"\n\
         done\n",
    )
    .unwrap();
    fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
    let web = Web::start_with(h, &[], |_| {});
    let cookie = web.login();
    let mut tab = Tab::open(&web, &cookie).await;
    let list = tab.req("extensions.list", json!({})).await.unwrap();
    let hello = list["extensions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "hello")
        .cloned()
        .expect("hello is listed");
    assert_eq!(hello["source"], "external");
    assert_eq!(hello["actions"][0]["id"], "greet");
    let reply = tab
        .req("extensions.action", json!({"ext": "hello", "action": "greet"}))
        .await
        .expect("action");
    assert_eq!(reply["result"], json!({"kind": "notice", "text": "hi"}));
    assert_eq!(
        tab.req("extensions.action", json!({"ext": "search", "action": "search"}))
            .await,
        Err("not an external extension".to_owned())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_proxy_stats_render_the_fixture() {
    let proxy = fake_proxy();
    let web = web_with_proxy(ManagerHarness::new(), &proxy);
    let cookie = web.login();
    let mut tab = Tab::open(&web, &cookie).await;
    let stats = tab
        .req("proxy.stats", json!({"profile": "env", "window": "7d"}))
        .await
        .expect("stats");
    assert_eq!(stats["user_name"], "tester");
    assert_eq!(stats["window"], "7d");
    assert_eq!(stats["totals"]["output_tokens"], 5600);
    assert_eq!(stats["by_model"][0]["model"], "claude-opus-5-5");
    assert_eq!(stats["pool"][0]["status"], "ok");
    assert_eq!(
        tab.req("proxy.stats", json!({"profile": "env", "window": "1h"}))
            .await,
        Err("bad request: window".to_owned())
    );
}
