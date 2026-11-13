//! `claudio web` integration tests (features: web): the CLI, authentication
//! and request checks, the browser protocol against a fake claude, direct
//! launches. Run single-threaded: `cargo test --test web -- --test-threads=1`.

#[path = "../../../crates/tui/tests/common/mod.rs"]
mod common;
mod web_harness;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::{fs, thread};

use common::{FakeSsh, ManagerHarness, BINARY, DAEMON_WAIT, WAIT};
use futures::SinkExt;
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;
use web_harness::app::*;
use web_harness::service::*;
use web_harness::tls::*;
use web_harness::*;

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
    assert_eq!(tab.hello["v"], 2);
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
fn web_login_bodies_and_me_name_the_methods() {
    let web = Web::start();
    let me = web.get("/api/me", &[]);
    assert_eq!(me.status, 401);
    let body: Value = serde_json::from_str(&me.body).unwrap();
    assert_eq!(body["login"], json!(["token"]));
    let origin = web.origin();
    let post = |body: Value| {
        web.post("/api/login", Some(&origin), &body.to_string(), &[])
            .status
    };
    assert_eq!(
        post(json!({"password": "twelve bytes!"})),
        400,
        "no password without --remote"
    );
    assert_eq!(post(json!({"token": web.token, "password": "x"})), 400);
    assert_eq!(post(json!({})), 400);
    assert_eq!(
        post(json!({"token": web.token, "extra": 1})),
        204,
        "unknown fields ignored"
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

#[test]
fn web_answers_on_its_listen_address_and_a_proxy_port() {
    // macOS routes only 127.0.0.1 to lo0 unless aliased: nothing to test there.
    let port = match std::net::TcpListener::bind("127.0.0.2:0") {
        Ok(l) => l.local_addr().unwrap().port(),
        Err(_) => return,
    };
    let h = ManagerHarness::new();
    let mut cmd = Command::new(BINARY);
    cmd.args(["web", "--foreground"])
        .arg("--listen")
        .arg(format!("127.0.0.2:{port}"))
        .args(["--no-open", "--allow-host", "proxy.example:8443"]);
    apply_env(&h, &mut cmd);
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(h.root.join("web.log"))
        .unwrap();
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(log);
    let mut child = cmd.spawn().expect("start claudio web");
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let url = lines.next();
    let token = fs::read_to_string(h.config_home.join("claudio/web-token"))
        .map(|t| t.trim().to_owned())
        .unwrap_or_default();
    // Dropped (killed) on every path from here.
    let web = Web {
        h: Some(h),
        child,
        port,
        token,
    };
    let url = url.and_then(Result::ok).unwrap_or_default();
    assert_eq!(
        url,
        format!("claudio web: http://127.0.0.2:{port}/#t={}", web.token)
    );
    let get = |host: &str| http_at("127.0.0.2", port, "GET", "/", host, &[], "").status;
    assert_eq!(get(&format!("127.0.0.2:{port}")), 200);
    assert_eq!(get(&format!("127.0.0.1:{port}")), 200);
    assert_eq!(get("proxy.example:8443"), 200);
    assert_eq!(get("proxy.example"), 421);
    assert_eq!(get(&format!("127.0.0.3:{port}")), 421);
    let body = json!({"token": web.token}).to_string();
    let login = |origin: &str| {
        http_at(
            "127.0.0.2",
            port,
            "POST",
            "/api/login",
            "proxy.example:8443",
            &[("Origin", origin)],
            &body,
        )
    };
    assert_eq!(login("https://evil.example:8443").status, 403);
    let reply = login("https://proxy.example:8443");
    assert_eq!(reply.status, 204, "{reply:?}");
    assert!(reply.header("set-cookie").unwrap().contains("; Secure"));
}

// ── --service: what the daemon runs ──────────────────────────────────────────

#[test]
fn web_service_says_ready_never_prints_the_token_and_stops_at_eof() {
    let h = ManagerHarness::new();
    let port = free_port();
    let listen = format!("--listen=127.0.0.1:{port}");
    // It only loads the token: without one it refuses to start, and makes none.
    let mut cmd = Command::new(BINARY);
    cmd.args(["web", "--service", &listen]);
    apply_env(&h, &mut cmd);
    let out = cmd.stdin(Stdio::null()).output().unwrap();
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("no token in ") && err.contains("run claudio web"),
        "{err}"
    );
    assert!(out.stdout.is_empty(), "no ready line");
    let token_file = h.config_home.join("claudio/web-token");
    assert!(!token_file.exists(), "it made no token");
    let mut cmd = Command::new(BINARY);
    cmd.args(["web", "--foreground", "--print-url"]);
    apply_env(&h, &mut cmd);
    assert!(cmd.output().unwrap().status.success(), "the token is made");
    let mut cmd = Command::new(BINARY);
    cmd.args(["web", "--service", &listen]);
    apply_env(&h, &mut cmd);
    let err = fs::File::create(h.root.join("service.err")).unwrap();
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(err);
    let mut child = cmd.spawn().expect("start claudio web --service");
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    out.read_line(&mut line).unwrap();
    assert_eq!(line, "ready\n");
    let token = fs::read_to_string(h.config_home.join("claudio/web-token")).unwrap();
    let token = token.trim();
    let (host, origin) = (
        format!("127.0.0.1:{port}"),
        format!("http://127.0.0.1:{port}"),
    );
    let body = json!({"token": token}).to_string();
    let login = http(
        port,
        "POST",
        "/api/login",
        &host,
        &[("Origin", &origin)],
        &body,
    );
    assert_eq!(login.status, 204, "it serves: {login:?}");
    // The daemon's stop (or death): stdin closes.
    drop(child.stdin.take());
    let deadline = std::time::Instant::now() + WAIT;
    while child.try_wait().unwrap().is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "still running after EOF"
        );
        thread::sleep(std::time::Duration::from_millis(50));
    }
    let mut rest = String::new();
    std::io::Read::read_to_string(&mut out, &mut rest).unwrap();
    assert_eq!(rest, "", "stdout carries the ready line only");
    let log = fs::read_to_string(h.root.join("service.err")).unwrap();
    assert!(!log.contains(token), "the token is never printed: {log}");
    assert!(
        !h.runtime_dir.join("claudio/daemon.log").exists(),
        "it never starts a daemon"
    );
}

#[test]
fn web_service_takes_sigterm_from_the_moment_it_says_ready() {
    use std::os::unix::process::ExitStatusExt;
    let h = ManagerHarness::new();
    let mut cmd = Command::new(BINARY);
    cmd.args(["web", "--foreground", "--print-url"]);
    apply_env(&h, &mut cmd);
    assert!(cmd.output().unwrap().status.success(), "the token is made");
    // A SIGTERM right after `ready` is the graceful stop (exit 0), or with
    // the hang hook ignored until SIGKILL: never the default action.
    for hang in ["0", "1"] {
        let listen = format!("--listen=127.0.0.1:{}", free_port());
        let mut cmd = Command::new(BINARY);
        cmd.args(["web", "--service", &listen]);
        apply_env(&h, &mut cmd);
        cmd.env("CLAUDIO_TEST_SERVICE_HANG", hang);
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped());
        let mut child = cmd.stderr(Stdio::null()).spawn().expect("start it");
        let mut out = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        assert_eq!(line, "ready\n");
        // SAFETY: kill(2) has no preconditions.
        unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
        if hang == "1" {
            thread::sleep(std::time::Duration::from_millis(500));
            assert!(child.try_wait().unwrap().is_none(), "SIGTERM is ignored");
            child.kill().unwrap();
            assert_eq!(child.wait().unwrap().signal(), Some(libc::SIGKILL));
            continue;
        }
        let deadline = std::time::Instant::now() + WAIT;
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            assert!(std::time::Instant::now() < deadline, "still running");
            thread::sleep(std::time::Duration::from_millis(50));
        };
        assert_eq!(status.code(), Some(0), "a graceful stop: {status:?}");
    }
}

// ── The web service: claudio web hands its server to the daemon ─────────────

/// AC3.
#[test]
fn web_hands_its_server_to_the_daemon_and_stop_takes_it_back() {
    let mut svc = ServiceWeb::start(ManagerHarness::new(), &[], |_| {});
    let cookie = svc.login();
    assert_eq!(svc.me(&cookie), 204, "the printed URL logs in");
    let (code, out) = svc.status();
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("web: enabled"), "{out}");
    assert!(
        out.contains(&format!("url: http://127.0.0.1:{}/\n", svc.port)),
        "{out}"
    );
    let first = pid_in(&out).expect("a pid");
    let restart = svc.claudio(&["daemon", "restart"]);
    assert!(restart.status.success(), "{restart:?}");
    assert!(gone_within(first, std::time::Duration::from_secs(5)));
    svc.running_other_than(Some(first), common::DAEMON_WAIT);
    let stop = svc.claudio(&["web", "stop"]);
    assert!(stop.status.success(), "{stop:?}");
    assert!(
        std::net::TcpListener::bind(("127.0.0.1", svc.port)).is_ok(),
        "the port is closed"
    );
    let config = fs::read_to_string(svc.h().config_home.join("claudio/config.toml")).unwrap();
    assert!(config.contains("[web]\nenabled = false"), "{config}");
    let (code, out) = svc.status();
    assert_eq!(code, Some(3), "{out}");
    assert!(
        out.contains("web: disabled") && out.contains("service: off"),
        "{out}"
    );
    svc.finish();
}

/// `stop` and `status` reach a daemon only if one runs: they never start one.
#[test]
fn web_stop_and_status_never_start_a_daemon() {
    let h = ManagerHarness::new();
    let status = claudio(&h, &["web", "status"], |_| {});
    assert_eq!(status.status.code(), Some(3), "{status:?}");
    let out = String::from_utf8_lossy(&status.stdout);
    assert!(
        out.contains("web: disabled") && out.contains("daemon: not running"),
        "{out}"
    );
    let stop = claudio(&h, &["web", "stop"], |_| {});
    assert_eq!(stop.status.code(), Some(0), "{stop:?}");
    let config = fs::read_to_string(h.config_home.join("claudio/config.toml")).unwrap();
    assert!(config.contains("[web]\nenabled = false"), "{config}");
    assert!(
        !h.runtime_dir.join("claudio/daemon.log").exists(),
        "no daemon started"
    );
}

/// `status` names an invalid stored `[web]` and makes up no URL for it.
#[test]
fn web_status_says_why_a_stored_section_is_invalid() {
    let h = ManagerHarness::new();
    let config = h.config_home.join("claudio/config.toml");
    let mut text = fs::read_to_string(&config).unwrap();
    text.push_str("\n[web]\nenabled = true\nlisten = \"0.0.0.0:7337\"\n");
    fs::write(&config, text).unwrap();
    let status = claudio(&h, &["web", "status"], |_| {});
    assert_eq!(status.status.code(), Some(3), "{status:?}");
    let out = String::from_utf8_lossy(&status.stdout);
    let why = "web: [web] listen: refusing to listen on 0.0.0.0:7337";
    assert!(out.contains(why), "{out}");
    assert!(!out.contains("url: "), "no URL for it: {out}");
}

/// AC9.
#[test]
fn web_service_comes_back_after_a_sigkill() {
    let mut svc = ServiceWeb::start(ManagerHarness::new(), &[], |_| {});
    let first = svc.running_other_than(None, common::WAIT);
    // SAFETY: kill(2); `first` is the service this test started.
    unsafe { libc::kill(first as libc::pid_t, libc::SIGKILL) };
    svc.running_other_than(Some(first), std::time::Duration::from_secs(5));
    svc.finish();
}

/// AC2, AC11: a daemon killed by a signal stops nothing; its web service
/// sees stdin's EOF and ends by itself, leaving the port free.
#[test]
fn web_service_ends_when_its_daemon_is_killed() {
    let mut svc = ServiceWeb::start(ManagerHarness::new(), &[], |_| {});
    let pid = svc.running_other_than(None, WAIT);
    // SIGTERM: the daemon has no handler, so it dies at once and stops nothing.
    svc.h().kill_daemon();
    assert!(
        gone_within(pid, std::time::Duration::from_secs(5)),
        "the service outlived its daemon"
    );
    assert!(
        std::net::TcpListener::bind(("127.0.0.1", svc.port)).is_ok(),
        "the port is free"
    );
    svc.finish();
}

/// AC4.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_remote_through_the_daemon_serves_tls_and_prints_the_banner() {
    let h = ManagerHarness::new();
    let port = free_port();
    let remote = format!("127.0.0.1:{port}");
    let args = ["web", "--remote", remote.as_str(), "--no-open"];
    let config = h.config_home.join("claudio/config.toml");
    let before = fs::read_to_string(&config).unwrap();
    let refused = claudio(&h, &args, |_| {});
    assert_eq!(refused.status.code(), Some(1), "{refused:?}");
    assert!(String::from_utf8_lossy(&refused.stderr).contains("--set-password"));
    assert_eq!(
        fs::read_to_string(&config).unwrap(),
        before,
        "config.toml untouched"
    );
    write_test_password(&h);
    let out = claudio(&h, &args, |_| {});
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let mut svc = ServiceWeb::adopt(h, port);
    let token = svc.token();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout, format!("claudio web: https://{remote}/\n"));
    let stderr = String::from_utf8_lossy(&out.stderr);
    let fp = kept_fingerprint(svc.h());
    assert!(
        stderr.contains(&format!("claudio web: TLS certificate SHA-256 {fp}\n")),
        "{stderr}"
    );
    let here = format!("claudio web: on this machine: https://{remote}/#t={token}");
    assert!(stderr.contains(&here), "{stderr}");
    let page = request_text("GET", "/", &remote, &[], "");
    let reply = https_from("127.0.0.1", port, &fp, &page).await.unwrap();
    assert_eq!(reply.status, 200, "TLS through the daemon");
    let (code, status) = svc.status();
    assert_eq!(code, Some(0), "{status}");
    assert!(
        status.contains(&format!("certificate SHA-256 {fp}")),
        "{status}"
    );
    svc.finish();
}

/// AC7.
#[test]
fn web_against_the_previous_release_s_daemon_saves_and_does_not_serve() {
    let h = ManagerHarness::new();
    let port = free_port();
    let listen = format!("127.0.0.1:{port}");
    let args = ["web", "--listen", listen.as_str(), "--no-open"];
    let out = claudio(&h, &args, |cmd| {
        cmd.env("CLAUDIO_TEST_OLD_DAEMON", "1");
    });
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("the daemon has no web service (older, or built without"),
        "{err}"
    );
    // An AddonOp would have been refused as `unsupported op`: none was sent.
    assert!(!err.contains("unsupported op"), "{err}");
    let config = fs::read_to_string(h.config_home.join("claudio/config.toml")).unwrap();
    assert!(config.contains("[web]\nenabled = true"), "{config}");
    assert!(
        std::net::TcpListener::bind(("127.0.0.1", port)).is_ok(),
        "nothing listens"
    );
    let mut svc = ServiceWeb::adopt(h, port);
    let restart = svc.claudio(&["daemon", "restart"]);
    assert!(restart.status.success(), "{restart:?}");
    svc.running_other_than(None, common::DAEMON_WAIT);
    assert_eq!(http(port, "GET", "/", &svc.host(), &[], "").status, 200);
    svc.token();
    svc.finish();
}

/// AC3: a service that ignores SIGTERM (and stdin's EOF) is SIGKILLed
/// after 3 s, and the new daemon still starts.
#[test]
fn web_daemon_restart_kills_a_service_that_ignores_its_stop() {
    let mut svc = ServiceWeb::start(ManagerHarness::new(), &[], |cmd| {
        // The daemon this starts, and so its service, inherit it.
        cmd.env("CLAUDIO_TEST_SERVICE_HANG", "1");
    });
    let first = svc.running_other_than(None, WAIT);
    let started = std::time::Instant::now();
    let restart = svc.claudio(&["daemon", "restart"]);
    assert!(restart.status.success(), "{restart:?}");
    assert!(
        started.elapsed() >= std::time::Duration::from_secs(3),
        "SIGKILL after 3 s"
    );
    assert!(!alive(first));
    svc.running_other_than(Some(first), common::DAEMON_WAIT);
    svc.finish();
}

/// AC13, service mode: sign-ins outlive a daemon restart; a new token
/// (`claudio web --rotate-token`, `--print-url --rotate-token`) or a new
/// password (`--set-password`, typed) restarts the service and signs
/// everyone out at once.
#[test]
fn web_service_sign_ins_outlive_a_daemon_restart_but_not_a_new_secret() {
    let mut svc = ServiceWeb::start(ManagerHarness::new(), &[], |_| {});
    let cookie = svc.login();
    let first = svc.running_other_than(None, WAIT);
    let restart = svc.claudio(&["daemon", "restart"]);
    assert!(restart.status.success(), "{restart:?}");
    let pid = svc.running_other_than(Some(first), common::DAEMON_WAIT);
    assert_eq!(svc.me(&cookie), 204, "kept across the daemon restart");

    let out = svc.claudio(&["web", "--rotate-token", "--no-open"]);
    assert!(out.status.success(), "{out:?}");
    let token = svc.token();
    assert!(String::from_utf8_lossy(&out.stdout).contains(&format!("#t={token}")));
    let pid = svc.running_other_than(Some(pid), WAIT);
    assert_eq!(svc.me(&cookie), 401, "a new token signs everyone out");

    let cookie = svc.login();
    let out = svc.claudio(&["web", "--print-url", "--rotate-token"]);
    assert!(out.status.success(), "{out:?}");
    let token = svc.token();
    assert!(String::from_utf8_lossy(&out.stdout).ends_with(&format!("#t={token}\n")));
    let pid = svc.running_other_than(Some(pid), WAIT);
    assert_eq!(svc.me(&cookie), 401, "--print-url --rotate-token too");

    let cookie = svc.login();
    assert_eq!(set_password_typed(svc.h(), "8-bytes!"), 0);
    svc.running_other_than(Some(pid), WAIT);
    assert_eq!(svc.me(&cookie), 401, "a new password signs everyone out");
    svc.finish();
}

/// A `--rotate-token` whose start is then refused (here `--remote` without
/// a password) still ends the old token: the running service restarts with
/// the new one, and the token file is the one it serves with.
#[test]
fn web_a_rotated_token_holds_when_the_start_is_refused() {
    let mut svc = ServiceWeb::start(ManagerHarness::new(), &[], |_| {});
    let cookie = svc.login();
    let old = svc.token();
    let pid = svc.running_other_than(None, WAIT);
    let remote = format!("127.0.0.1:{}", svc.port);
    let out = svc.claudio(&["web", "--remote", &remote, "--rotate-token", "--no-open"]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("--set-password"));
    assert_ne!(svc.token(), old, "the token was rotated");
    svc.running_other_than(Some(pid), WAIT);
    assert_eq!(svc.me(&cookie), 401, "the old sign-ins are gone");
    let body = json!({ "token": old }).to_string();
    let origin = format!("http://{}", svc.host());
    let reply = http(
        svc.port,
        "POST",
        "/api/login",
        &svc.host(),
        &[("Origin", &origin)],
        &body,
    );
    assert_eq!(reply.status, 401, "the old token is refused");
    let cookie = svc.login();
    assert_eq!(
        svc.me(&cookie),
        204,
        "the token file is the one it serves with"
    );
    svc.finish();
}

// ── Persistent sign-ins ───────────────────────────────────────────────────────

fn cookie_header(cookie: &str) -> String {
    format!("claudio_web={cookie}")
}

/// `/api/me` with `cookie`.
fn me_with(web: &Web, cookie: &str) -> u16 {
    web.get("/api/me", &[("Cookie", &cookie_header(cookie))])
        .status
}

#[test]
fn web_cookie_outlives_the_browser_and_logout_expires_it() {
    let web = Web::start();
    let body = json!({"token": web.token}).to_string();
    let reply = web.post("/api/login", Some(&web.origin()), &body, &[]);
    assert_eq!(reply.status, 204);
    let set = reply.header("set-cookie").unwrap().to_owned();
    let cookie = cookie_of(&set);
    assert_eq!(
        set,
        format!("claudio_web={cookie}; Max-Age=34560000; HttpOnly; SameSite=Strict; Path=/")
    );
    let out = web.post(
        "/api/logout",
        Some(&web.origin()),
        "",
        &[("Cookie", &cookie_header(&cookie))],
    );
    assert_eq!(out.status, 204);
    assert_eq!(
        out.header("set-cookie"),
        Some("claudio_web=; Max-Age=0; HttpOnly; SameSite=Strict; Path=/")
    );
}

#[test]
fn web_sign_ins_and_sign_outs_survive_a_killed_server() {
    let web = Web::start();
    let (kept, ended) = (web.login(), web.login());
    let out = web.post(
        "/api/logout",
        Some(&web.origin()),
        "",
        &[("Cookie", &cookie_header(&ended))],
    );
    assert_eq!(out.status, 204);
    // SIGKILL: only what was written before each reply is in the file.
    let h = web.stop();
    let web = Web::start_with(h, &[], |_| {});
    assert_eq!(me_with(&web, &kept), 204, "kept across the restart");
    assert_eq!(me_with(&web, &ended), 401, "a sign-out stays");
    let h = web.stop();
    let web = Web::start_with(h, &["--rotate-token"], |_| {});
    assert_eq!(me_with(&web, &kept), 401, "a new token signs everyone out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_a_loopback_sign_in_is_refused_by_a_remote_start() {
    let h = ManagerHarness::new();
    // The same password bytes in both starts: only the mode differs.
    write_test_password(&h);
    let web = Web::start_with(h, &[], |_| {});
    let cookie = web.login();
    assert_eq!(me_with(&web, &cookie), 204);
    let h = web.stop();
    let r = RemoteWeb::start(h, &[], |_| {});
    let me = r
        .req("GET", "/api/me", &[("Cookie", &cookie_header(&cookie))], "")
        .await;
    assert_eq!(me.status, 401);
}

/// Whether `tab`'s socket ends (closed, failed or a close frame) before
/// `deadline`; frames before that are skipped.
async fn ends_by(tab: &mut Tab, deadline: tokio::time::Instant) -> bool {
    use futures::StreamExt;
    loop {
        match tokio::time::timeout_at(deadline, tab.ws.next()).await {
            Err(_) => return false,
            Ok(None | Some(Err(_)) | Some(Ok(Message::Close(_)))) => return true,
            Ok(Some(Ok(_))) => continue,
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_a_removed_sign_in_closes_the_tabs_opened_with_it() {
    let web = Web::start();
    let (ended, kept, replaced) = (web.login(), web.login(), web.login());
    let mut first = Tab::open(&web, &ended).await;
    let mut second = Tab::open(&web, &ended).await;
    let mut other = Tab::open(&web, &kept).await;
    let mut old = Tab::open(&web, &replaced).await;
    // A sign-out closes both of its tabs within 2 s.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    let out = web.post(
        "/api/logout",
        Some(&web.origin()),
        "",
        &[("Cookie", &cookie_header(&ended))],
    );
    assert_eq!(out.status, 204);
    assert!(ends_by(&mut first, deadline).await, "first tab still open");
    assert!(
        ends_by(&mut second, deadline).await,
        "second tab still open"
    );
    // A login with a cookie replaces its session: that session's tab closes.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    let body = json!({"token": web.token}).to_string();
    let again = web.post(
        "/api/login",
        Some(&web.origin()),
        &body,
        &[("Cookie", &cookie_header(&replaced))],
    );
    assert_eq!(again.status, 204);
    assert!(
        ends_by(&mut old, deadline).await,
        "a replaced session's tab"
    );
    // Another session's tab stays open and answers.
    assert!(other.req("hosts.list", json!({})).await.is_ok());
    // A closed tab gives its slot back and the hub forgets it: more
    // sign-outs with an open tab than there are tabs leave room for one.
    for _ in 0..17 {
        let cookie = web.login();
        let mut tab = Tab::open(&web, &cookie).await;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        let out = web.post(
            "/api/logout",
            Some(&web.origin()),
            "",
            &[("Cookie", &cookie_header(&cookie))],
        );
        assert_eq!(out.status, 204);
        assert!(ends_by(&mut tab, deadline).await);
    }
    let mut last = Tab::open(&web, &kept).await;
    assert!(last.req("hosts.list", json!({})).await.is_ok());
    assert!(other.req("hosts.list", json!({})).await.is_ok());
}

// ── --set-password (AC3) ──────────────────────────────────────────────────────

#[test]
fn web_set_password_refuses_a_pipe() {
    let h = ManagerHarness::new();
    let mut cmd = Command::new(BINARY);
    cmd.args(["web", "--set-password"]);
    apply_env(&h, &mut cmd);
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // It may exit before reading: a broken pipe here is fine.
    let _ = child
        .stdin
        .take()
        .unwrap()
        .write_all(b"a-long-enough-password\na-long-enough-password\n");
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("terminal"), "{err}");
    assert!(!err.contains("a-long-enough-password"), "never echoed");
    assert!(!h.config_home.join("claudio/web-password").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_set_password_takes_8_bytes_typed_at_a_terminal() {
    let h = ManagerHarness::new();
    assert_eq!(set_password_typed(&h, "8-bytes!"), 0);
    let r = RemoteWeb::start_keeping_password(h, &[], |_| {});
    assert_eq!(r.login(&json!({"password": "8-bytes!"})).await.status, 204);
    assert_eq!(r.login(&json!({"password": "8-bytes?"})).await.status, 401);
}

// ── --remote: TLS (AC2, AC4, AC6) ────────────────────────────────────────────

#[test]
fn web_remote_refuses_to_start_without_a_password() {
    let h = ManagerHarness::new();
    let port = free_port();
    let mut cmd = Command::new(BINARY);
    cmd.args(["web", "--foreground"])
        .arg("--remote")
        .arg(format!("127.0.0.1:{port}"))
        .arg("--no-open");
    apply_env(&h, &mut cmd);
    let out = cmd.stdin(Stdio::null()).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("--set-password"), "{err}");
    assert!(out.stdout.is_empty(), "no URL: nothing served");
    assert!(
        !h.config_home.join("claudio/web-tls").exists(),
        "no certificate made"
    );
    assert!(
        std::net::TcpListener::bind(("127.0.0.1", port)).is_ok(),
        "nothing bound"
    );
}

#[test]
fn web_remote_arguments_are_checked() {
    let h = ManagerHarness::new();
    for (args, needle) in [
        (&["--remote", "--listen", "127.0.0.1:7999"][..], "--listen"),
        (&["--rotate-cert"][..], "needs --remote"),
        (&["--remote", "fe80::1"][..], "brackets"),
        (
            &["--remote", ":7999", "--allow-host", "a.example:8443"][..],
            "one port",
        ),
        (&["--print-cert", "--no-open"][..], "no other argument"),
    ] {
        let mut cmd = Command::new(BINARY);
        cmd.arg("web").args(args);
        apply_env(&h, &mut cmd);
        let out = cmd.stdin(Stdio::null()).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains(needle), "{args:?}: {err}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_remote_serves_tls_with_a_kept_certificate() {
    let r = RemoteWeb::start(ManagerHarness::new(), &[], |_| {});
    let page = r.req("GET", "/", &[], "").await;
    assert_eq!(page.status, 200);
    assert!(page.header("content-security-policy").is_some());
    assert_eq!(page.header("strict-transport-security"), None, "no HSTS");
    let other = request_text("GET", "/", &r.host(), &[], "");
    assert!(
        https_from("127.0.0.1", r.web.port, "00:11", &other)
            .await
            .is_err(),
        "another pin fails"
    );
    let plain = http(r.web.port, "GET", "/", &r.host(), &[], "");
    assert_eq!(plain.status, 0, "plain HTTP gets no HTTP answer: {plain:?}");
    let dir = r.web.h().config_home.join("claudio/web-tls");
    let mode = |p: std::path::PathBuf| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(dir.clone()), 0o700);
    assert_eq!(mode(dir.join("cert.pem")), 0o600);
    assert_eq!(mode(dir.join("key.pem")), 0o600);
    let first = r.fingerprint.clone();
    let h = r.web.stop();
    let mut cmd = Command::new(BINARY);
    cmd.args(["web", "--print-cert"]);
    apply_env(&h, &mut cmd);
    let out = cmd.output().unwrap();
    assert!(out.status.success());
    let printed = String::from_utf8_lossy(&out.stdout);
    assert_eq!(printed, fs::read_to_string(dir.join("cert.pem")).unwrap());
    assert!(!printed.contains("PRIVATE KEY"));
    let r = RemoteWeb::start(h, &[], |_| {});
    assert_eq!(r.fingerprint, first, "kept across restarts");
    let h = r.web.stop();
    let r = RemoteWeb::start(h, &["--rotate-cert"], |_| {});
    assert_ne!(r.fingerprint, first, "replaced by --rotate-cert");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_remote_guard_holds_over_tls() {
    let r = RemoteWeb::start(
        ManagerHarness::new(),
        &["--allow-host", "claudio.test"],
        |_| {},
    );
    let port = r.web.port;
    for (host, status) in [
        (format!("evil.example:{port}"), 421),
        (format!("claudio.test:{port}"), 200),
        (format!("10.1.2.3:{port}"), 200),
        (format!("[fd00::1]:{port}"), 200),
        (format!("0.0.0.0:{port}"), 421),
        (format!("10.1.2.3:{}", port + 1), 421),
    ] {
        let reply = r.req_as("127.0.0.1", &host, "GET", "/", &[], "").await;
        assert_eq!(reply.status, status, "{host}");
    }
    let token = json!({"token": r.web.token});
    let http_origin = format!("http://127.0.0.1:{port}");
    let refused = r
        .req(
            "POST",
            "/api/login",
            &[("Origin", &http_origin)],
            &token.to_string(),
        )
        .await;
    assert_eq!(refused.status, 403, "no http:// Origin with --remote");
    let ok = r.login(&token).await;
    assert_eq!(ok.status, 204, "{ok:?}");
    let set = ok.header("set-cookie").unwrap();
    for flag in ["HttpOnly", "SameSite=Strict", "Path=/", "Secure"] {
        assert!(set.contains(flag), "{set}");
    }
    let cookie = cookie_of(set);
    let hello = wss_hello(port, &r.fingerprint, &cookie, &r.origin()).await;
    assert_eq!(hello.unwrap()["t"], "hello");
    let refused = wss_hello(port, &r.fingerprint, &cookie, &http_origin).await;
    assert_eq!(refused.err(), Some(403));
    // A loopback peer that names this machine is not local: no token.
    let host = format!("claudio.test:{port}");
    let origin = format!("https://claudio.test:{port}");
    let rep = r
        .req_as(
            "127.0.0.1",
            &host,
            "POST",
            "/api/login",
            &[("Origin", &origin)],
            &token.to_string(),
        )
        .await;
    assert_eq!(rep.status, 401);
    let me = r
        .req_as("127.0.0.1", &host, "GET", "/api/me", &[], "")
        .await;
    assert_eq!(me.status, 401);
    let body: Value = serde_json::from_str(&me.body).unwrap();
    assert_eq!(body["login"], json!(["password"]));
}

// ── --remote: logins (AC5) ───────────────────────────────────────────────────

fn login_methods(reply: &Reply) -> Value {
    serde_json::from_str::<Value>(&reply.body).unwrap()["login"].clone()
}

/// A login from 127.0.0.1 that names `host` (and its `https://` Origin).
async fn login_as(r: &RemoteWeb, host: &str, body: Value) -> u16 {
    let origin = format!("https://{host}");
    r.req_as(
        "127.0.0.1",
        host,
        "POST",
        "/api/login",
        &[("Origin", &origin)],
        &body.to_string(),
    )
    .await
    .status
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_remote_login_rules() {
    let r = RemoteWeb::start(
        ManagerHarness::new(),
        &["--allow-host", "claudio.test"],
        |_| {},
    );
    let port = r.web.port;
    // Local: both methods.
    let me = r.req("GET", "/api/me", &[], "").await;
    assert_eq!(
        (me.status, login_methods(&me)),
        (401, json!(["token", "password"]))
    );
    let pw = r.login(&json!({"password": TEST_PASSWORD})).await;
    assert_eq!(pw.status, 204, "{pw:?}");
    assert!(pw.header("set-cookie").unwrap().contains("; Secure"));
    assert_eq!(r.login(&json!({"token": r.web.token})).await.status, 204);
    // Not local (a loopback peer that names this machine): the password only.
    let host = format!("claudio.test:{port}");
    let me = r
        .req_as("127.0.0.1", &host, "GET", "/api/me", &[], "")
        .await;
    assert_eq!(login_methods(&me), json!(["password"]));
    let token = json!({"token": r.web.token});
    assert_eq!(login_as(&r, &host, token).await, 401);
    let right = json!({"password": TEST_PASSWORD});
    assert_eq!(login_as(&r, &host, right).await, 204);
    let wrong = json!({"password": "not-the-password"});
    assert_eq!(login_as(&r, &host, wrong).await, 401);
    let both = json!({"password": TEST_PASSWORD, "token": r.web.token});
    assert_eq!(login_as(&r, &host, both).await, 400);
    let big = json!({"password": "x".repeat(5000)});
    assert_eq!(login_as(&r, &host, big).await, 413);
    // No password reached the log (stderr carries the loopback token URL
    // by design, so the token is not checked here).
    let log = fs::read_to_string(r.web.h().root.join("web.log")).unwrap();
    for secret in [TEST_PASSWORD, "not-the-password"] {
        assert!(!log.contains(secret), "{secret} in the log");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_remote_peer_on_another_address_needs_the_password() {
    // Linux routes all of 127/8 to lo; macOS only 127.0.0.1 unless aliased.
    if std::net::TcpListener::bind("127.0.0.2:0").is_err() {
        return;
    }
    let r = RemoteWeb::start(ManagerHarness::new(), &[], |cmd| {
        cmd.env("CLAUDIO_TEST_REMOTE_PEER", "127.0.0.2");
    });
    let (host, origin) = (r.host(), r.origin());
    let from_2 = |method: &'static str, path: &'static str, body: String| {
        let (r, host, origin) = (&r, host.clone(), origin.clone());
        async move {
            r.req_as(
                "127.0.0.2",
                &host,
                method,
                path,
                &[("Origin", &origin)],
                &body,
            )
            .await
        }
    };
    let me = from_2("GET", "/api/me", String::new()).await;
    assert_eq!(login_methods(&me), json!(["password"]));
    let token = json!({"token": r.web.token}).to_string();
    assert_eq!(from_2("POST", "/api/login", token).await.status, 401);
    let pw = json!({"password": TEST_PASSWORD}).to_string();
    assert_eq!(from_2("POST", "/api/login", pw).await.status, 204);
    assert_eq!(
        r.login(&json!({"token": r.web.token})).await.status,
        204,
        "127.0.0.1 stays local"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_remote_concurrent_wrong_passwords_hash_at_most_five() {
    let r = RemoteWeb::start(ManagerHarness::new(), &[], |_| {});
    let wrong = json!({"password": "wrong-password-xx"});
    let replies = futures::future::join_all((0..20).map(|_| r.login(&wrong))).await;
    let count = |s| replies.iter().filter(|r| r.status == s).count();
    // A 401 is only ever answered after argon2 ran: exactly five hashed.
    assert_eq!((count(401), count(429)), (5, 15));
    let right = json!({"password": TEST_PASSWORD});
    assert_eq!(
        r.login(&right).await.status,
        429,
        "the peer's minute is spent"
    );
}

// ── Frames ────────────────────────────────────────────────────────────────────

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
                "launch": {"pick": "direct"}, "bogus": 1}),
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
                "launch": {"pick": "direct"}, "prompt": "first task"}),
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

/// A web server whose ssh is `fake`'s, logged in, `local` connected.
async fn web_with_fake_ssh(fake: &FakeSsh) -> (Web, Tab) {
    let h = ManagerHarness::new();
    write_web_fake_claude(&h, false);
    let path = fake.path();
    let web = Web::start_with(h, &[], move |cmd| {
        cmd.env("PATH", &path);
    });
    let cookie = web.login();
    let mut tab = Tab::open(&web, &cookie).await;
    tab.connected("local").await;
    (web, tab)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_forward_agent_writes_the_option_and_connects_nothing() {
    let fake = FakeSsh::new();
    let (web, mut tab) = web_with_fake_ssh(&fake).await;
    let set = |host: &str, on: bool| json!({"host": host, "on": on});
    assert_eq!(
        tab.req("hosts.forward_agent", set("local", true)).await,
        Err("local has no ssh connection".to_owned())
    );
    let bad = tab
        .req("hosts.forward_agent", set("-oProxyCommand=x", true))
        .await;
    assert!(bad.unwrap_err().contains("must not start with '-'"));
    let missing = tab
        .req("hosts.forward_agent", json!({"host": "nowhere"}))
        .await;
    assert!(missing.unwrap_err().starts_with("bad request:"));

    // A host this server does not know: written, and nothing connects.
    let reply = tab.req("hosts.forward_agent", set("nowhere", true)).await;
    assert_eq!(reply, Ok(json!({})));
    let config = web.h().config_home.join("claudio/config.toml");
    let text = fs::read_to_string(&config).unwrap();
    assert!(
        text.contains("[ssh]\nforward_agent = [\"nowhere\"]"),
        "{text}"
    );
    assert!(text.starts_with("[claude]\n"), "the rest stays: {text}");
    let hosts = tab.req("hosts.list", json!({})).await.unwrap();
    let names: Vec<&Value> = hosts["hosts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| &h["name"])
        .collect();
    assert_eq!(names, [&json!("local")]);
    assert_eq!(hosts["hosts"][0]["forward_agent"], false, "never for local");
    tab.req("hosts.forward_agent", set("nowhere", false))
        .await
        .unwrap();
    let text = fs::read_to_string(&config).unwrap();
    assert!(text.contains("forward_agent = []"), "{text}");
    assert!(fake.calls().is_empty(), "no ssh ran: {:?}", fake.calls());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_forward_agent_reconnects_a_connected_host_and_keeps_its_sessions() {
    let fake = FakeSsh::new();
    let (web, mut tab) = web_with_fake_ssh(&fake).await;
    let dir = web.h().dirs[0].clone();
    let connect = tokio::time::timeout(
        DAEMON_WAIT,
        tab.req("hosts.connect", json!({"host": "fakehost"})),
    );
    let host = connect
        .await
        .expect("connected in time")
        .expect("hosts.connect");
    assert_eq!(host["host"]["forward_agent"], false);
    let shell = json!({"host": "fakehost", "cwd": dir, "rows": 24, "cols": 80});
    let sid = tab.req("session.shell", shell).await.expect("shell")["sid"]
        .as_str()
        .unwrap()
        .to_owned();
    let before = tab
        .event("session", |v| {
            v["session"]["id"] == sid.as_str() && v["session"]["pid"].is_u64()
        })
        .await;
    let bridges = fake.bridges();
    assert_eq!(bridges.len(), 1, "{bridges:?}");
    assert!(!bridges[0].contains("ForwardAgent"), "{bridges:?}");

    // From here on, only what the toggle causes.
    tab.pending.clear();
    let on = json!({"host": "fakehost", "on": true});
    assert_eq!(
        tab.req("hosts.forward_agent", on.clone()).await,
        Ok(json!({}))
    );
    let fwd = |v: &Value| v["host"]["name"] == "fakehost" && v["host"]["forward_agent"] == true;
    tab.event("host", |v| fwd(v) && v["host"]["connected"] == false)
        .await;
    tab.event("host", |v| fwd(v) && v["host"]["connected"] == true)
        .await;
    let bridges = fake.wait_bridges(2, WAIT);
    assert!(bridges[1].contains("-o ForwardAgent=yes --"), "{bridges:?}");
    // The terminal kept running: back from `unknown` with the same process.
    let state =
        |v: &Value, want: &str| v["session"]["id"] == sid.as_str() && v["session"]["state"] == want;
    tab.event("session", |v| state(v, "unknown")).await;
    let after = tab.event("session", |v| state(v, "idle")).await;
    assert_eq!(after["session"]["pid"], before["session"]["pid"]);
    // Unchanged: no reconnect.
    assert_eq!(tab.req("hosts.forward_agent", on).await, Ok(json!({})));
    let listed = tab.req("hosts.list", json!({})).await.unwrap();
    let fakehost = listed["hosts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|h| h["name"] == "fakehost")
        .cloned();
    assert_eq!(fakehost.unwrap()["forward_agent"], true);
    assert_eq!(fake.bridges().len(), 2);
}

/// What `ssh-add -l` says in terminal `sid`, its output after this run's
/// command line only (`step` makes the line and its marker unique).
async fn ssh_add_in(tab: &mut Tab, sid: &str, step: u32) -> String {
    let line = format!("ssh-add -l; echo step{step}-$((40+2))\r");
    tab.input(sid, line.as_bytes()).await;
    let out = tab.output_until(sid, &format!("step{step}-42")).await;
    let typed = format!("step{step}-$((40+2))");
    out[out.rfind(&typed).unwrap_or(0)..].to_owned()
}

/// A forwarded agent reaches a remote terminal through the bridge's link and
/// the daemon's relay; a terminal started while one was forwarded loses it
/// while no connection forwards one and gets it back on the next that does,
/// without a restart (hermetic: a fake ssh and a fake agent with no keys).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_forwarded_agent_reaches_terminals_through_the_relay() {
    // Never the owner's agent: only whether ssh-add runs at all.
    let mut probe = Command::new("ssh-add");
    probe.arg("-l").env_remove("SSH_AUTH_SOCK");
    if probe.output().is_err() {
        println!("SKIP web_forwarded_agent_reaches_terminals_through_the_relay: no ssh-add");
        return;
    }
    let fake = FakeSsh::with_agent();
    let (web, mut tab) = web_with_fake_ssh(&fake).await;
    let dir = web.h().dirs[0].clone();
    let set = |on: bool| json!({"host": "fakehost", "on": on});
    tab.req("hosts.forward_agent", set(true)).await.unwrap();
    let connect = tab.req("hosts.connect", json!({"host": "fakehost"}));
    let connect = tokio::time::timeout(DAEMON_WAIT, connect).await;
    connect.expect("connected in time").expect("hosts.connect");
    assert!(fake.bridges()[0].contains("ForwardAgent=yes"));
    let shell = json!({"host": "fakehost", "cwd": dir, "rows": 24, "cols": 200});
    let sid = tab.req("session.shell", shell).await.expect("shell")["sid"]
        .as_str()
        .unwrap()
        .to_owned();
    let attach = json!({"sid": sid, "rows": 24, "cols": 200});
    tab.req("session.attach", attach).await.expect("attach");
    let none = "The agent has no identities";
    assert!(ssh_add_in(&mut tab, &sid, 1).await.contains(none));

    // No connection forwards an agent now: the terminal cannot reach one.
    reconnect_with(&mut tab, &fake, &sid, false, 2).await;
    assert!(!ssh_add_in(&mut tab, &sid, 2).await.contains(none));

    // A forwarding connection again: the same terminal reaches the agent.
    reconnect_with(&mut tab, &fake, &sid, true, 3).await;
    assert!(ssh_add_in(&mut tab, &sid, 3).await.contains(none));
}

/// Switch fakehost's forwarding to `on` and wait for its `n`th bridge and for
/// terminal `sid` to be attached again.
async fn reconnect_with(tab: &mut Tab, fake: &FakeSsh, sid: &str, on: bool, n: usize) {
    tab.pending.clear();
    let set = json!({"host": "fakehost", "on": on});
    tab.req("hosts.forward_agent", set).await.unwrap();
    fake.wait_bridges(n, WAIT);
    tab.event("host", |v| {
        v["host"]["name"] == "fakehost" && v["host"]["connected"] == true
    })
    .await;
    tab.event("attached", |v| v["sid"] == sid).await;
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

/// A fake claude that records the env vars these tests assert on to
/// `env_file` (never the whole env, never to the terminal;
/// [`common::record_env_sh`]), lists `--agents`/`--mcp-config` in `--help`,
/// then prints the banner and echoes lines.
fn write_env_fake_claude(h: &ManagerHarness, env_file: &std::path::Path) {
    let record = common::record_env_sh(
        &env_file.display().to_string(),
        &["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY"],
        &["CLAUDECODE"],
    );
    let script = format!(
        r#"#!/bin/sh
[ "$1" = "--version" ] && echo 'claude 0.0.0-fake' && exit 0
if [ "$1" = "--help" ]; then printf '  --agents <json>  Agents\n  --mcp-config <configs...>  MCP\n'; exit 0; fi
{record}printf '%s' "$2" > .fake-settings.json
echo "FAKE_CLAUDE_BANNER pid=$$"
while IFS= read -r line; do printf 'line:%s\n' "$line"; done
"#
    );
    fs::write(&h.fake_claude, script).unwrap();
    fs::set_permissions(&h.fake_claude, fs::Permissions::from_mode(0o755)).unwrap();
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
                assert!(
                    std::time::Instant::now() < deadline,
                    "the session never came back"
                );
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            other => break other,
        }
    };
    assert_eq!(err, Err("launch unknown".to_owned()));
    let mut direct = ask.clone();
    direct["launch"] = json!({"pick": "direct"});
    let reply = tab.req("session.respawn", direct).await.expect("respawn");
    assert_eq!(reply["sid"], sid.as_str());
    tab.event("session", |v| v["session"]["id"] == sid.as_str())
        .await;
    let saved: Value = serde_json::from_slice(&fs::read(&state).unwrap()).unwrap();
    let meta = &saved["sessions"][sid.as_str()];
    assert_eq!(meta["host"], "local", "{saved}");
    assert!(meta["proxy"].is_null(), "{saved}");
}

/// Wait for the env-dumping fake claude's `env_file`.
async fn read_env_file(env_file: &std::path::Path) -> String {
    let deadline = std::time::Instant::now() + WAIT;
    loop {
        if let Ok(env) = fs::read_to_string(env_file) {
            return env;
        }
        assert!(std::time::Instant::now() < deadline, "claude never started");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// A direct session goes through the launch composer (env_remove: the
/// session markers only): spawned or respawned, claude keeps the inherited
/// API key and never sees a parent claude's markers.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_direct_launches_keep_the_inherited_api_key() {
    let h = ManagerHarness::new();
    let env_file = h.root.join("claude-env.txt");
    write_env_fake_claude(&h, &env_file);
    let dir = h.dirs[0].clone();
    let web = Web::start_with(h, &[], |cmd| {
        cmd.env("CLAUDECODE", "1");
    });
    let cookie = web.login();
    let mut tab = Tab::open(&web, &cookie).await;
    tab.connected("local").await;
    let reply = tab
        .req(
            "session.spawn",
            json!({"host": "local", "cwd": dir, "rows": 24, "cols": 80, "launch": {"pick": "direct"}}),
        )
        .await
        .expect("spawn");
    let sid = reply["sid"].as_str().unwrap().to_owned();
    let env = read_env_file(&env_file).await;
    assert!(env.contains("ANTHROPIC_API_KEY=test-key-not-real"), "{env}");
    assert!(!env.contains("CLAUDECODE="), "a parent claude's marker");
    fs::remove_file(&env_file).unwrap();
    let ask =
        json!({"sid": sid, "fresh": true, "rows": 24, "cols": 80, "launch": {"pick": "direct"}});
    let reply = tab.req("session.respawn", ask).await.expect("respawn");
    assert_eq!(reply["sid"], sid.as_str());
    let env = read_env_file(&env_file).await;
    assert!(env.contains("ANTHROPIC_API_KEY=test-key-not-real"), "{env}");
    assert!(!env.contains("CLAUDECODE="), "a parent claude's marker");
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
    // P3.18: the diagram row is the web addon's.
    assert_eq!(diagram(&list)["source"], "addon");
    assert_eq!(diagram(&list)["addon"], "web");

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
async fn web_snippets_save_named_list_delete() {
    let web = Web::start();
    let cookie = web.login();
    let mut tab = Tab::open(&web, &cookie).await;
    let dir = web.h().config_home.join("claudio/snippets");
    let saved = tab
        .req(
            "snippets.save",
            json!({"text": "v1", "name": "Release Notes"}),
        )
        .await
        .expect("save named");
    assert_eq!(saved["name"], "release-notes");
    tab.req(
        "snippets.save",
        json!({"text": "v2", "name": "release notes"}),
    )
    .await
    .expect("replace");
    let list = tab.req("snippets.list", json!({})).await.unwrap();
    let mine: Vec<&Value> = list["snippets"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["name"] == "release-notes")
        .collect();
    assert_eq!(mine.len(), 1, "{list}");
    assert_eq!(mine[0]["text"], "v2");
    assert_eq!(
        fs::read_to_string(dir.join("release-notes.md")).unwrap(),
        "v2"
    );
    for bad in ["../x", "a/b", ".hidden", "nope", ""] {
        let r = tab.req("snippets.delete", json!({ "name": bad })).await;
        assert!(
            r.as_ref()
                .is_err_and(|e| e.starts_with("bad request: name:")),
            "{bad}: {r:?}"
        );
    }
    assert_eq!(
        tab.req("snippets.save", json!({"text": "x", "name": " "}))
            .await,
        Err("bad request: name: empty".to_owned())
    );
    tab.req("snippets.delete", json!({"name": "release-notes"}))
        .await
        .expect("delete");
    assert!(!dir.join("release-notes.md").exists());
    let list = tab.req("snippets.list", json!({})).await.unwrap();
    assert!(list["snippets"].as_array().unwrap().is_empty(), "{list}");
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
        .req(
            "extensions.action",
            json!({"ext": "hello", "action": "greet"}),
        )
        .await
        .expect("action");
    assert_eq!(reply["result"], json!({"kind": "notice", "text": "hi"}));
    assert_eq!(
        tab.req(
            "extensions.action",
            json!({"ext": "search", "action": "search"})
        )
        .await,
        Err("not an external extension".to_owned())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hello_is_v2() {
    let web = Web::start();
    let cookie = web.login();
    let tab = Tab::open(&web, &cookie).await;
    assert_eq!(tab.hello["t"], "hello");
    assert_eq!(tab.hello["v"], 2);
    assert_eq!(tab.hello["features"], json!([]));
}

// ── Sign-in tickets (docs/specs/2026-11-13-desktop-design.md) ────────────────

/// AC4: a ticket signs in once; a body with a ticket and anything else is
/// a bad one.
#[test]
fn web_a_ticket_signs_in_once() {
    let web = Web::start();
    let ticket = mint_ticket(web.h());
    let post = |body: Value| web.post("/api/login", Some(&web.origin()), &body.to_string(), &[]);
    assert_eq!(
        post(json!({"ticket": ticket, "token": web.token})).status,
        400
    );
    assert_eq!(post(json!({"ticket": ticket, "password": "x"})).status, 400);
    assert!(
        ticket_file(web.h(), &ticket).exists(),
        "a 400 spends nothing"
    );
    let first = post(json!({ "ticket": ticket }));
    assert_eq!(first.status, 204, "{first:?}");
    let cookie = cookie_of(first.header("set-cookie").expect("a cookie"));
    assert_eq!(me_with(&web, &cookie), 204);
    assert!(!ticket_file(web.h(), &ticket).exists(), "spent");
    assert_eq!(post(json!({ "ticket": ticket })).status, 401, "a replay");
}

/// AC4: with `--remote`, a ticket from a peer that is not local is refused
/// and counts, and its file stays: the same ticket then signs in locally.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_remote_tickets_are_for_local_requests_only() {
    // Linux routes all of 127/8 to lo; macOS only 127.0.0.1 unless aliased.
    if std::net::TcpListener::bind("127.0.0.2:0").is_err() {
        return;
    }
    let r = RemoteWeb::start(ManagerHarness::new(), &[], |cmd| {
        cmd.env("CLAUDIO_TEST_REMOTE_PEER", "127.0.0.2");
    });
    let ticket = mint_ticket(r.web.h());
    let body = json!({ "ticket": ticket }).to_string();
    let (host, origin) = (r.host(), r.origin());
    let headers = [("Origin", origin.as_str())];
    let from_2 = || r.req_as("127.0.0.2", &host, "POST", "/api/login", &headers, &body);
    for i in 0..5 {
        assert_eq!(from_2().await.status, 401, "attempt {i}");
    }
    assert_eq!(from_2().await.status, 429, "the refusals counted");
    assert!(ticket_file(r.web.h(), &ticket).exists(), "the file stays");
    let local = r.login(&json!({ "ticket": ticket })).await;
    assert_eq!(local.status, 204, "{local:?}");
    assert!(local.header("set-cookie").unwrap().contains("; Secure"));
}

/// AC6: `claudio web` opens the browser on a `file://` redirect file that
/// carries a ticket; neither argv nor the file holds the token.
#[test]
fn web_opens_the_browser_on_a_redirect_file_never_the_token() {
    let mut svc = ServiceWeb::start(ManagerHarness::new(), &[], |_| {});
    let token = svc.token();
    let fake = FakeBin::new(&svc.h().root);
    let mut cmd = portable_pty::CommandBuilder::new(BINARY);
    cmd.arg("web");
    svc.h().apply(&mut cmd);
    desktop_env_pty(&mut cmd, &fake);
    let mut tty = common::TuiProcess::spawn(cmd);
    assert_eq!(tty.wait_exit_code(DAEMON_WAIT), 0);
    let opener = match cfg!(target_os = "macos") {
        true => "open",
        false => "xdg-open",
    };
    let calls = fake.wait_calls(opener, 1, WAIT);
    assert_eq!(calls.len(), 1, "{calls:?}");
    let [url] = calls[0].as_slice() else {
        panic!("one argument: {calls:?}");
    };
    assert!(!url.contains('#') && !url.contains(&token), "{url}");
    let html = redirect_of(svc.h(), url);
    assert!(
        html.contains(&format!("url=http://127.0.0.1:{}/#k=", svc.port)),
        "{html}"
    );
    assert!(!html.contains("#t=") && !html.contains(&token), "{html}");
    let ticket = ticket_in(&html);
    assert_eq!(svc.ticket_login(&ticket), 204, "the ticket signs in");
    svc.finish();
}

// ── The app window (docs/specs/2026-11-13-desktop-design.md) ─────────────────

/// `--class=claudio` where the app argv has it.
fn class() -> Vec<String> {
    match cfg!(target_os = "linux") {
        true => vec!["--class=claudio".to_owned()],
        false => Vec::new(),
    }
}

/// AC5: `claudio web --app`, in a terminal and without one, starts the
/// browser once each with `--app=file://<redirect file>`, the file holding
/// a ticket; without a terminal neither stdout nor stderr holds the token
/// or the ticket.
#[test]
fn web_app_opens_a_signed_in_window_and_prints_no_secret_without_a_terminal() {
    let mut svc = ServiceWeb::start(ManagerHarness::new(), &[], |_| {});
    let token = svc.token();
    let fake = FakeBin::new(&svc.h().root);
    let mut cmd = portable_pty::CommandBuilder::new(BINARY);
    cmd.args(["web", "--app"]);
    svc.h().apply(&mut cmd);
    desktop_env_pty(&mut cmd, &fake);
    let mut tty = common::TuiProcess::spawn(cmd);
    assert_eq!(tty.wait_exit_code(DAEMON_WAIT), 0);
    fake.wait_calls("fake-browser", 1, WAIT);
    let out = claudio(svc.h(), &["web", "--app"], |cmd| desktop_env(cmd, &fake));
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let (stdout, stderr) = (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    );
    assert_eq!(
        stdout,
        format!("claudio web: http://127.0.0.1:{}/\n", svc.port)
    );
    assert!(
        !stdout.contains(&token) && !stderr.contains(&token),
        "{stderr}"
    );
    let calls = fake.wait_calls("fake-browser", 2, WAIT);
    assert_eq!(calls.len(), 2, "once per run: {calls:?}");
    for (i, call) in calls.iter().enumerate() {
        assert_eq!(call[1..], class(), "{call:?}");
        let url = call[0].strip_prefix("--app=").expect("--app=<url>");
        assert!(!url.contains('#') && !url.contains(&token), "{url}");
        let html = redirect_of(svc.h(), url);
        assert!(
            html.contains(&format!("url=http://127.0.0.1:{}/#k=", svc.port)),
            "{html}"
        );
        let ticket = ticket_in(&html);
        assert!(!call.iter().any(|a| a.contains(&ticket)), "{call:?}");
        if i == 1 {
            assert!(!stdout.contains(&ticket) && !stderr.contains(&ticket));
        }
        assert_eq!(svc.ticket_login(&ticket), 204, "its ticket signs in");
    }
    assert!(fake.calls("xdg-open").is_empty() && fake.calls("open").is_empty());
    svc.finish();
}

/// AC5: without a terminal, an `--app` whose `[web]` cannot start (here
/// `--remote` without a password) is said on the desktop (a fake
/// `notify-send`, `osascript` on macOS) and in `<runtime>/web-app.log`.
#[test]
fn web_app_failures_reach_the_desktop_without_a_terminal() {
    let h = ManagerHarness::new();
    let fake = FakeBin::new(&h.root);
    let remote = format!("127.0.0.1:{}", free_port());
    let out = claudio(&h, &["web", "--app", "--remote", &remote], |cmd| {
        desktop_env(cmd, &fake)
    });
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(stderr.contains("--set-password"), "{stderr}");
    let notifier = match cfg!(target_os = "macos") {
        true => "osascript",
        false => "notify-send",
    };
    let calls = fake.wait_calls(notifier, 1, WAIT);
    let message = calls[0].last().expect("the message").clone();
    assert!(
        message.starts_with("claudio web: ") && message.contains("--set-password"),
        "{message}"
    );
    assert!(stderr.contains(&message), "{stderr}");
    if notifier == "notify-send" {
        assert_eq!(calls[0][..3], ["-a", "claudio", "claudio"]);
    }
    let log = fs::read_to_string(runtime(&h).join("web-app.log")).unwrap();
    assert!(log.contains(&message), "{log}");
    assert!(fake.calls("fake-browser").is_empty(), "no window");
}

/// AC11: `claudio web --foreground --app` opens the window once the server
/// listens, on a ticket of its own; without a terminal its stdout line has
/// no token.
#[test]
fn web_foreground_app_opens_the_window_once_it_listens() {
    let h = ManagerHarness::new();
    let fake = FakeBin::new(&h.root);
    let port = free_port();
    let mut cmd = Command::new(BINARY);
    cmd.args(["web", "--foreground", "--app", "--listen"])
        .arg(format!("127.0.0.1:{port}"));
    apply_env(&h, &mut cmd);
    desktop_env(&mut cmd, &fake);
    let log = fs::File::create(h.root.join("web.log")).unwrap();
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(log);
    let mut child = cmd.spawn().expect("start claudio web");
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    // Dropped (killed) on every path from here.
    let web = Web {
        h: Some(h),
        child,
        port,
        token: String::new(),
    };
    let line = lines.next().and_then(Result::ok).unwrap_or_default();
    assert_eq!(line, format!("claudio web: http://127.0.0.1:{port}/"));
    let calls = fake.wait_calls("fake-browser", 1, DAEMON_WAIT);
    assert_eq!(web.get("/", &[]).status, 200, "it listens");
    assert_eq!(calls[0][1..], class());
    let html = redirect_of(web.h(), calls[0][0].strip_prefix("--app=").unwrap());
    let body = json!({ "ticket": ticket_in(&html) }).to_string();
    let reply = web.post("/api/login", Some(&web.origin()), &body, &[]);
    assert_eq!(reply.status, 204, "{reply:?}");
    let token = fs::read_to_string(web.h().config_home.join("claudio/web-token")).unwrap();
    let stderr = fs::read_to_string(web.h().root.join("web.log")).unwrap();
    assert!(!stderr.contains(token.trim()), "{stderr}");
}

/// AC11: `CLAUDIO_NO_BROWSER=1 claudio web --app` starts nothing and mints
/// nothing.
#[test]
fn web_app_with_claudio_no_browser_opens_and_mints_nothing() {
    let svc = ServiceWeb::start(ManagerHarness::new(), &[], |_| {});
    let fake = FakeBin::new(&svc.h().root);
    let out = claudio(svc.h(), &["web", "--app"], |cmd| {
        desktop_env(cmd, &fake);
        cmd.env("CLAUDIO_NO_BROWSER", "1");
    });
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    thread::sleep(std::time::Duration::from_millis(300));
    assert!(fake.calls("fake-browser").is_empty());
    let tickets = fs::read_dir(runtime(svc.h()).join("web-tickets"));
    assert_eq!(tickets.map(|d| d.count()).unwrap_or(0), 0, "nothing minted");
    svc.finish();
}

/// AC14: `claudio web --app=<url>` starts the browser once on exactly that
/// URL and nothing else: no daemon, no token, `config.toml` unchanged. A
/// URL with credentials exits 2 with the message.
#[test]
fn web_app_url_opens_only_the_window() {
    let h = ManagerHarness::new();
    let fake = FakeBin::new(&h.root);
    let config = h.config_home.join("claudio/config.toml");
    let before = fs::read_to_string(&config).unwrap();
    let url = "https://example.invalid:7337/";
    let arg = format!("--app={url}");
    let out = claudio(&h, &["web", &arg], |cmd| desktop_env(cmd, &fake));
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let calls = fake.wait_calls("fake-browser", 1, WAIT);
    let want: Vec<String> = std::iter::once(arg.clone()).chain(class()).collect();
    assert_eq!(calls, [want]);
    let sockets = fs::read_dir(runtime(&h))
        .map(|d| {
            d.flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "sock"))
                .count()
        })
        .unwrap_or(0);
    assert_eq!(sockets, 0, "no daemon");
    assert_eq!(
        fs::read_to_string(&config).unwrap(),
        before,
        "config.toml unchanged"
    );
    assert!(
        !h.config_home.join("claudio/web-token").exists(),
        "no token"
    );
    let bad = claudio(&h, &["web", "--app=https://u:p@example.invalid/"], |cmd| {
        desktop_env(cmd, &fake)
    });
    assert_eq!(bad.status.code(), Some(2), "{bad:?}");
    assert_eq!(
        String::from_utf8_lossy(&bad.stderr),
        "claudio web --app: expected an http(s) URL without credentials or fragment\n"
    );
    assert_eq!(fake.calls("fake-browser").len(), 1);
}

/// Compatibility: a service that started before this binary's file changed
/// (as when a busy daemon kept it) is restarted from the binary on disk
/// before an `--app` window opens on a ticket; a current one is kept.
#[test]
fn web_app_restarts_a_service_older_than_its_binary() {
    let h = ManagerHarness::new();
    let bin = h.root.join("bin/claudio");
    fs::create_dir_all(bin.parent().unwrap()).unwrap();
    fs::copy(BINARY, &bin).unwrap();
    let fake = FakeBin::new(&h.root);
    let port = free_port();
    let listen = format!("127.0.0.1:{port}");
    let mut svc = ServiceWeb::adopt(h, port);
    let out = claudio_at(
        &bin,
        svc.h(),
        &["web", "--listen", &listen, "--no-open"],
        |_| {},
    );
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let first = svc.running_other_than(None, WAIT);
    let app = |svc: &ServiceWeb| {
        claudio_at(&bin, svc.h(), &["web", "--app"], |cmd| {
            desktop_env(cmd, &fake)
        })
    };
    let out = app(&svc);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    fake.wait_calls("fake-browser", 1, WAIT);
    assert_eq!(
        svc.running_other_than(None, WAIT),
        first,
        "a current service is kept"
    );
    // The same build (the daemon stays), its file newer than the service.
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(120);
    fs::File::open(&bin).unwrap().set_modified(later).unwrap();
    let out = app(&svc);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let calls = fake.wait_calls("fake-browser", 2, WAIT);
    svc.running_other_than(Some(first), WAIT);
    let html = redirect_of(svc.h(), calls[1][0].strip_prefix("--app=").unwrap());
    assert_eq!(
        svc.ticket_login(&ticket_in(&html)),
        204,
        "the restarted service redeems it"
    );
    svc.token();
    svc.finish();
}

/// AC7, AC14: `--install-launcher --app=<url>` writes a launcher running
/// `web --app=<url>` under the test `HOME` (`XDG_DATA_HOME` unset);
/// `--remove-launcher` takes it away, and again finds nothing to remove.
#[test]
fn web_install_launcher_writes_under_home_and_remove_takes_it_away() {
    let h = ManagerHarness::new();
    let no_data_home = |cmd: &mut Command| {
        cmd.env_remove("XDG_DATA_HOME");
    };
    let url = "https://example.invalid:7337/";
    let arg = format!("--app={url}");
    let out = claudio(&h, &["web", "--install-launcher", &arg], no_data_home);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let exes = [
        BINARY.to_owned(),
        fs::canonicalize(BINARY).unwrap().display().to_string(),
    ];
    let (launcher, line) = match cfg!(target_os = "macos") {
        true => (
            h.home.join("Applications/claudio.app"),
            exes.map(|e| format!("exec '{e}' web '{arg}'")),
        ),
        false => (
            h.home.join(".local/share/applications/claudio.desktop"),
            exes.map(|e| format!("Exec=\"{e}\" web {arg}")),
        ),
    };
    let text = match cfg!(target_os = "macos") {
        true => fs::read_to_string(launcher.join("Contents/MacOS/claudio")).unwrap(),
        false => fs::read_to_string(&launcher).unwrap(),
    };
    assert!(text.lines().any(|l| line.contains(&l.to_owned())), "{text}");
    let printed = String::from_utf8_lossy(&out.stdout);
    assert!(
        printed
            .lines()
            .all(|l| l.starts_with("claudio web: wrote ")),
        "{printed}"
    );
    let out = claudio(&h, &["web", "--remove-launcher"], no_data_home);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(fs::symlink_metadata(&launcher).is_err(), "removed");
    let again = claudio(&h, &["web", "--remove-launcher"], no_data_home);
    assert_eq!(again.status.code(), Some(0), "{again:?}");
    assert!(String::from_utf8_lossy(&again.stdout).contains("no launcher at "));
}
