//! `claudio web` handed to the harness daemon (the web service): run the
//! CLI, read `claudio web status`, and end every test with no process of
//! its own left and no token in a log (AC5, AC11).

use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};
use std::{fs, thread};

use super::{apply_env, http};
use crate::common::{ManagerHarness, BINARY};

/// `claudio <args>` in `h`'s isolated env, run to its end: at most 60 s,
/// so a command that keeps running fails the test instead of hanging it.
pub fn claudio(h: &ManagerHarness, args: &[&str], setup: impl FnOnce(&mut Command)) -> Output {
    claudio_at(Path::new(BINARY), h, args, setup)
}

/// [`claudio`] with the binary at `bin` (a copy of it, say).
pub fn claudio_at(
    bin: &Path,
    h: &ManagerHarness,
    args: &[&str],
    setup: impl FnOnce(&mut Command),
) -> Output {
    let mut cmd = Command::new(bin);
    cmd.args(args);
    apply_env(h, &mut cmd);
    setup(&mut cmd);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("run claudio");
    let deadline = Instant::now() + Duration::from_secs(60);
    while child.try_wait().expect("wait for claudio").is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("claudio {args:?} did not exit within 60 s");
        }
        thread::sleep(Duration::from_millis(50));
    }
    child.wait_with_output().expect("claudio's output")
}

/// Whether process `pid` exists (a zombie counts).
pub fn alive(pid: u32) -> bool {
    // SAFETY: kill(2) with signal 0 only checks that the process exists.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

/// Wait up to `within` for `pid` to be gone; whether it is.
pub fn gone_within(pid: u32, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    while alive(pid) {
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(50));
    }
    true
}

/// The pid in `claudio web status`'s `service: running (pid N, …)` line.
pub fn pid_in(status: &str) -> Option<u32> {
    let rest = status.split("service: running (pid ").nth(1)?;
    rest.split(',').next()?.parse().ok()
}

/// A web the harness daemon serves.
pub struct ServiceWeb {
    pub h: Option<ManagerHarness>,
    pub port: u16,
    /// Every token it served with (none may reach a log).
    pub tokens: Vec<String>,
    /// Every service pid seen (all must be gone at the end).
    pub pids: Vec<u32>,
}

impl ServiceWeb {
    /// `claudio web --listen 127.0.0.1:<port> --no-open` plus `args`: exits
    /// 0 and prints the token URL.
    pub fn start(h: ManagerHarness, args: &[&str], setup: impl FnOnce(&mut Command)) -> ServiceWeb {
        let port = super::free_port();
        let listen = format!("127.0.0.1:{port}");
        let mut all = vec!["web", "--listen", &listen, "--no-open"];
        all.extend_from_slice(args);
        let out = claudio(&h, &all, setup);
        let mut web = ServiceWeb {
            h: Some(h),
            port,
            tokens: Vec::new(),
            pids: Vec::new(),
        };
        assert_eq!(out.status.code(), Some(0), "{out:?}");
        let token = web.token();
        let printed = String::from_utf8_lossy(&out.stdout);
        assert_eq!(
            printed,
            format!("claudio web: http://{listen}/#t={token}\n")
        );
        web
    }

    /// A web some other command handed over (`port` is its listener).
    pub fn adopt(h: ManagerHarness, port: u16) -> ServiceWeb {
        ServiceWeb {
            h: Some(h),
            port,
            tokens: Vec::new(),
            pids: Vec::new(),
        }
    }

    pub fn h(&self) -> &ManagerHarness {
        self.h.as_ref().unwrap()
    }

    /// The token file now (remembered for the log check).
    pub fn token(&mut self) -> String {
        let path = self.h().config_home.join("claudio/web-token");
        let token = fs::read_to_string(path).unwrap().trim().to_owned();
        if !self.tokens.contains(&token) {
            self.tokens.push(token.clone());
        }
        token
    }

    /// `claudio <args>` in this harness.
    pub fn claudio(&self, args: &[&str]) -> Output {
        claudio(self.h(), args, |_| {})
    }

    /// `claudio web status`: its exit code and stdout; a running pid is
    /// remembered.
    pub fn status(&mut self) -> (Option<i32>, String) {
        let out = self.claudio(&["web", "status"]);
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        if let Some(pid) = pid_in(&text).filter(|p| !self.pids.contains(p)) {
            self.pids.push(pid);
        }
        (out.status.code(), text)
    }

    /// Wait until `status` says running with a pid other than `not`.
    pub fn running_other_than(&mut self, not: Option<u32>, within: Duration) -> u32 {
        let deadline = Instant::now() + within;
        loop {
            let (code, text) = self.status();
            match pid_in(&text) {
                Some(pid) if code == Some(0) && Some(pid) != not => return pid,
                _ => assert!(Instant::now() < deadline, "not running: {text}"),
            }
            thread::sleep(Duration::from_millis(100));
        }
    }

    /// A token login on the loopback listener: the cookie.
    pub fn login(&mut self) -> String {
        let token = self.token();
        let (host, origin) = (self.host(), format!("http://{}", self.host()));
        let body = serde_json::json!({ "token": token }).to_string();
        let reply = http(
            self.port,
            "POST",
            "/api/login",
            &host,
            &[("Origin", &origin)],
            &body,
        );
        assert_eq!(reply.status, 204, "{reply:?}");
        let set = reply.header("set-cookie").expect("a cookie").to_owned();
        super::tls::cookie_of(&set)
    }

    /// A sign-in with `ticket` on the loopback listener: its status.
    pub fn ticket_login(&self, ticket: &str) -> u16 {
        let (host, origin) = (self.host(), format!("http://{}", self.host()));
        let body = serde_json::json!({ "ticket": ticket }).to_string();
        let headers = [("Origin", origin.as_str())];
        http(self.port, "POST", "/api/login", &host, &headers, &body).status
    }

    /// `/api/me` with `cookie`.
    pub fn me(&self, cookie: &str) -> u16 {
        let with = format!("claudio_web={cookie}");
        http(
            self.port,
            "GET",
            "/api/me",
            &self.host(),
            &[("Cookie", &with)],
            "",
        )
        .status
    }

    pub fn host(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }

    /// The end of a test (AC11, AC5): `claudio web stop`, the daemon
    /// killed, every service pid gone within 5 s, and no token in
    /// `web.log` or `daemon.log`. Then the dirs go.
    pub fn finish(mut self) {
        let out = self.claudio(&["web", "stop"]);
        assert!(out.status.success(), "{out:?}");
        let h = self.h.take().unwrap();
        h.kill_daemon();
        for pid in &self.pids {
            assert!(
                gone_within(*pid, Duration::from_secs(5)),
                "service {pid} left"
            );
        }
        for log in ["web.log", "daemon.log"] {
            let text = fs::read_to_string(h.runtime_dir.join("claudio").join(log));
            let text = text.unwrap_or_default();
            // A service ran: there is a log to check, not a missing file.
            assert!(self.pids.is_empty() || !text.is_empty(), "no {log}");
            for token in &self.tokens {
                assert!(!text.contains(token.as_str()), "the token is in {log}");
            }
        }
    }
}

impl Drop for ServiceWeb {
    /// A failed test still leaves nothing running before its dirs go.
    fn drop(&mut self) {
        let Some(h) = self.h.take() else {
            return;
        };
        let _ = claudio(&h, &["web", "stop"], |_| {});
        h.kill_daemon();
        for pid in &self.pids {
            gone_within(*pid, Duration::from_secs(5));
        }
    }
}
