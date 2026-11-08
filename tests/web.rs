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
        .send(Message::text("x".repeat(64 * 1024 + 1)))
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
    // 64 KiB + 1 of text cannot fit a client text frame (64 KiB): the
    // socket's frame limit refuses it before the paste cap
    // (`input::paste_has_a_size_cap` covers "too large").
    let big = json!({"t": "req", "id": 99, "op": "session.paste", "sid": sid,
        "text": "x".repeat(64 * 1024 + 1), "submit": false});
    tab.ws.send(Message::text(big.to_string())).await.unwrap();
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
