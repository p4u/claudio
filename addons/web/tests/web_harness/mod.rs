//! The `claudio web` test client the web suites share: a test server
//! (`Web`), plain HTTP (`http`, `Reply`), a browser tab on the WebSocket
//! (`Tab`) and the fake claude they drive. The isolated dirs are the PTY
//! harness's (`common::ManagerHarness`).

#![allow(dead_code)]

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::os::unix::fs::PermissionsExt;
use std::process::{Child, Command, Stdio};
use std::{fs, thread};

use crate::common::{ManagerHarness, BINARY, DAEMON_WAIT, WAIT};
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::{self, client::IntoClientRequest, Message};

pub type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// The isolated env of a test `claudio web` (and of the daemon it starts).
pub fn apply_env(h: &ManagerHarness, cmd: &mut Command) {
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

pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// A running `claudio web` on a free loopback port, in a harness's dirs.
pub struct Web {
    pub h: Option<ManagerHarness>,
    pub child: Child,
    pub port: u16,
    pub token: String,
}

impl Web {
    pub fn start() -> Web {
        Web::start_with(ManagerHarness::new(), &[], |_| {})
    }

    pub fn start_with(h: ManagerHarness, args: &[&str], setup: impl FnOnce(&mut Command)) -> Web {
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

    pub fn h(&self) -> &ManagerHarness {
        self.h.as_ref().unwrap()
    }

    /// Stop the server; the harness (and the daemon) stay.
    pub fn stop(mut self) -> ManagerHarness {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let h = self.h.take().unwrap();
        daemon_settled(&h);
        h
    }

    pub fn host(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }

    pub fn origin(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    pub fn get(&self, path: &str, extra: &[(&str, &str)]) -> Reply {
        http(self.port, "GET", path, &self.host(), extra, "")
    }

    pub fn post(
        &self,
        path: &str,
        origin: Option<&str>,
        body: &str,
        extra: &[(&str, &str)],
    ) -> Reply {
        let mut headers: Vec<(&str, &str)> = extra.to_vec();
        if let Some(origin) = origin {
            headers.push(("Origin", origin));
        }
        http(self.port, "POST", path, &self.host(), &headers, body)
    }

    /// Log in with the right token; the cookie's value.
    pub fn login(&self) -> String {
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

    pub async fn ws_with(&self, cookie: &str, origin: &str) -> Result<Ws, u16> {
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
pub fn daemon_settled(h: &ManagerHarness) {
    if !h.runtime_dir.join("claudio/daemon.log").exists() {
        return;
    }
    let deadline = std::time::Instant::now() + DAEMON_WAIT;
    while h.daemon_pid().is_none() && std::time::Instant::now() < deadline {
        thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Reply {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// One raw HTTP/1.1 request, so a test controls `Host` and `Origin`.
pub fn http(
    port: u16,
    method: &str,
    path: &str,
    host: &str,
    extra: &[(&str, &str)],
    body: &str,
) -> Reply {
    http_at("127.0.0.1", port, method, path, host, extra, body)
}

/// [`http`] to a server listening on `ip`.
pub fn http_at(
    ip: &str,
    port: u16,
    method: &str,
    path: &str,
    host: &str,
    extra: &[(&str, &str)],
    body: &str,
) -> Reply {
    let mut s = TcpStream::connect((ip, port)).unwrap();
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
pub struct Tab {
    pub ws: Ws,
    pub next: u32,
    pub pending: VecDeque<Message>,
    pub hello: Value,
    pub snapshot: Value,
    pub seen: Vec<Vec<u8>>,
}

impl Tab {
    pub async fn open(web: &Web, cookie: &str) -> Tab {
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

    pub async fn next_message(&mut self) -> Message {
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
    pub async fn text_where(&mut self, pick: impl Fn(&Value) -> bool) -> Value {
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

    pub async fn req(&mut self, op: &str, args: Value) -> Result<Value, String> {
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

    pub async fn event(&mut self, ev: &str, pick: impl Fn(&Value) -> bool) -> Value {
        self.text_where(|v| v["t"] == "ev" && v["ev"] == ev && pick(v))
            .await
    }

    /// Session `sid`'s output (0x01 frames, buffered ones first) until it
    /// contains `needle`.
    pub async fn output_until(&mut self, sid: &str, needle: &str) -> String {
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

    pub async fn input(&mut self, sid: &str, bytes: &[u8]) {
        let mut frame = vec![0x02];
        frame.extend(uuid_bytes(sid));
        frame.extend_from_slice(bytes);
        self.ws.send(Message::binary(frame)).await.unwrap();
    }

    /// The close code the server ends the socket with.
    pub async fn close_code(&mut self) -> Option<u16> {
        loop {
            match tokio::time::timeout(WAIT, self.ws.next()).await.ok()?? {
                Ok(Message::Close(frame)) => return frame.map(|f| u16::from(f.code)),
                Ok(_) => continue,
                Err(_) => return None,
            }
        }
    }
}

pub fn text_json(m: &Message) -> Option<Value> {
    match m {
        Message::Text(t) => serde_json::from_str(t.as_str()).ok(),
        _ => None,
    }
}

pub fn output_of<'a>(m: &'a Message, id: &[u8; 16]) -> Option<&'a [u8]> {
    match m {
        Message::Binary(b) if b.len() >= 17 && b[0] == 0x01 && &b[1..17] == id => Some(&b[17..]),
        _ => None,
    }
}

pub fn uuid_bytes(sid: &str) -> [u8; 16] {
    *uuid::Uuid::parse_str(sid).unwrap().as_bytes()
}

// ── The hub: a fake claude, hooks, sessions ──────────────────────────────────

/// A fake claude for the web tests: answers --version/--help, saves its
/// --settings JSON (the hook commands) to `.fake-settings.json` in its cwd,
/// optionally turns bracketed paste on, prints a banner, then logs every
/// input line to `.fake-input.log`, answers `size` with `stty size` and
/// echoes any other line as `line:<text>`.
pub fn write_web_fake_claude(h: &ManagerHarness, bracketed_paste: bool) {
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

impl Tab {
    /// Wait until `host` is connected (the snapshot may already say so).
    pub async fn connected(&mut self, host: &str) {
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
    pub async fn spawn_attached(&mut self, dir: &std::path::Path) -> String {
        self.connected("local").await;
        let reply = self
            .req(
                "session.spawn",
                json!({"host": "local", "cwd": dir, "rows": 24, "cols": 80, "launch": {"pick": "direct"}}),
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
