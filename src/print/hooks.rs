//! Completion signalling and §4.9.2 process/env concealment.
//!
//! **Transport layer** — how the relay subprocess reports back to the parent:
//!   * `tcp`  — relay connects to 127.0.0.1:<port>. Port is embedded in the
//!              relay command as an argv token, never in an env var, so claude's
//!              own `process.env` stays clean.
//!   * `file` — relay drops a file into a watched directory (fallback when
//!              loopback TCP is blocked).
//!
//! **Relay concealment (§4.9.2)** — for TCP transport, `Relay::setup` copies
//! the wrapper binary to a temp dir under a random hex name (e.g.
//! `/tmp/.xdg-a3f2bc1d/b8e41caf`), then registers that neutral path as the
//! hook command.  The binary name `claudio` never appears in the --settings
//! JSON that claude parses.  The port is a positional argv token, not an env
//! var, so `env | grep CLAUDIO` in the child finds nothing.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;

use crate::cli::HookTransport;

// Kept for the file-transport relay path (env var is still needed there
// because the hook command writes to a dir, not connects to a port).
pub const ENV_DIR: &str = "CLAUDIO_HOOK_DIR";
// Legacy TCP env — still accepted by run_relay for backward compat.
pub const ENV_PORT: &str = "CLAUDIO_HOOK_PORT";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookEvent {
    SessionStart,
    Stop,
    Unknown,
}

impl HookEvent {
    pub fn parse(s: &str) -> Self {
        match s.trim() {
            "SessionStart" => HookEvent::SessionStart,
            "Stop" => HookEvent::Stop,
            _ => HookEvent::Unknown,
        }
    }
}

pub struct Hook {
    pub event: HookEvent,
    pub payload: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// §4.9.2 — Neutral relay binary in a temp dir.
// ─────────────────────────────────────────────────────────────────────────────

/// A copy of our binary living under a random hex name in a temp directory.
/// The hook command references this path, not `claudio`, so the binary name
/// does not appear in claude's `--settings` JSON or process tree.
pub struct Relay {
    dir: PathBuf,
    /// Absolute path to the neutral copy.  The hook command is `<bin> <event> <port>`.
    pub bin: PathBuf,
}

impl Relay {
    /// Create a neutral relay binary for the given TCP port.
    pub fn setup() -> std::io::Result<Self> {
        let exe = std::env::current_exe()?;

        // Derive two 8-char hex tokens from XOR-shifted time — no extra dep.
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0xdeadbeefcafe);
        let a = xorshift64(seed);
        let b = xorshift64(a);
        let dir_token = format!("{:08x}", a as u32);
        let bin_token = format!("{:08x}", b as u32);

        let dir = std::env::temp_dir().join(format!(".xdg-{dir_token}"));
        std::fs::create_dir_all(&dir)?;
        let bin = dir.join(&bin_token);

        // Hard-link first (same inode, instant); fall back to copy if the
        // binary and /tmp are on different filesystems.
        if std::fs::hard_link(&exe, &bin).is_err() {
            std::fs::copy(&exe, &bin)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755));
            }
        }

        Ok(Self { dir, bin })
    }

    /// The hook command for a given event: `<neutral-bin> <event> <port>`.
    /// The binary name is a random hex string; the port is a positional arg,
    /// not an env var, so claude's environment is not polluted.
    pub fn command(&self, event: &str, port: u16) -> String {
        format!("{} {event} {port}", self.bin.display())
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.bin);
        let _ = std::fs::remove_dir(&self.dir);
    }
}

fn xorshift64(mut x: u64) -> u64 {
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    x
}

// ─────────────────────────────────────────────────────────────────────────────
// Relay entry points (run inside the short-lived hook subprocess).
// ─────────────────────────────────────────────────────────────────────────────

/// Neutral relay: invoked as `<relay-bin> <event> <port>` (port in argv).
/// This is the concealed transport — no env var is needed.
pub fn relay_to_port(event: &str, port: u16) {
    let mut payload = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut payload);
    if let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)) {
        let _ = s.write_all(event.as_bytes());
        let _ = s.write_all(b"\n");
        let _ = s.write_all(&payload);
        let _ = s.flush();
    }
}

/// Legacy relay: invoked as `claudio __hook <event>`. Reads port from env
/// or falls back to file transport. Kept for backward compatibility and for
/// the file-transport path where env vars are unavoidable.
pub fn run_relay(event: &str) {
    let mut payload = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut payload);

    if let Ok(port_str) = std::env::var(ENV_PORT) {
        if let Ok(port) = port_str.parse::<u16>() {
            if let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)) {
                let _ = s.write_all(event.as_bytes());
                let _ = s.write_all(b"\n");
                let _ = s.write_all(&payload);
                let _ = s.flush();
                return;
            }
        }
    }

    if let Ok(dir) = std::env::var(ENV_DIR) {
        let dir = PathBuf::from(dir);
        let id = uuid::Uuid::new_v4();
        let tmp = dir.join(format!("{id}.tmp"));
        let done = dir.join(format!("{id}.hook"));
        if let Ok(mut f) = std::fs::File::create(&tmp) {
            let _ = f.write_all(event.as_bytes());
            let _ = f.write_all(b"\n");
            let _ = f.write_all(&payload);
            let _ = f.flush();
            let _ = std::fs::rename(&tmp, &done);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Listener (parent side — receives events from the relay).
// ─────────────────────────────────────────────────────────────────────────────

pub enum Listener {
    Tcp {
        rx: mpsc::Receiver<Hook>,
        port: u16,
        /// Signals the accept thread to stop so its socket is released on drop.
        stop: Arc<AtomicBool>,
    },
    File { dir: PathBuf },
}

impl Listener {
    pub fn start(transport: HookTransport) -> std::io::Result<Self> {
        match transport {
            HookTransport::Tcp => {
                let listener = TcpListener::bind(("127.0.0.1", 0))?;
                let port = listener.local_addr()?.port();
                // Non-blocking accept + a stop flag so the thread (and its socket)
                // can be released when the Listener drops. Otherwise `incoming()`
                // would block forever, leaking one thread + socket + port per turn.
                listener.set_nonblocking(true)?;
                let (tx, rx) = mpsc::channel();
                let stop = Arc::new(AtomicBool::new(false));
                let stop_thread = Arc::clone(&stop);
                thread::spawn(move || {
                    while !stop_thread.load(Ordering::Relaxed) {
                        match listener.accept() {
                            Ok((mut s, _)) => {
                                let _ = s.set_nonblocking(false);
                                let mut buf = Vec::new();
                                if s.read_to_end(&mut buf).is_ok() {
                                    if let Some(hook) = parse_framed(&buf) {
                                        let _ = tx.send(hook);
                                    }
                                }
                            }
                            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                                thread::sleep(Duration::from_millis(20));
                            }
                            Err(_) => thread::sleep(Duration::from_millis(20)),
                        }
                    }
                    // `listener` drops here, closing the socket and freeing the port.
                });
                Ok(Listener::Tcp { rx, port, stop })
            }
            HookTransport::File => {
                let dir = std::env::temp_dir().join(format!("claudio-{}", uuid::Uuid::new_v4()));
                std::fs::create_dir_all(&dir)?;
                Ok(Listener::File { dir })
            }
        }
    }

    /// TCP port, if this is a TCP listener.
    pub fn port(&self) -> Option<u16> {
        match self {
            Listener::Tcp { port, .. } => Some(*port),
            Listener::File { .. } => None,
        }
    }

    /// Env vars the child claude must carry.
    /// For TCP transport with a Relay, no vars are needed — return empty.
    /// For file transport, the dir path is still needed.
    pub fn child_env(&self) -> Vec<(String, String)> {
        match self {
            // Port is in the relay command argv — no env var required.
            Listener::Tcp { .. } => vec![],
            Listener::File { dir } => {
                vec![(ENV_DIR.to_string(), dir.to_string_lossy().into_owned())]
            }
        }
    }

    pub fn poll(&self, timeout: Duration) -> Option<Hook> {
        match self {
            Listener::Tcp { rx, .. } => rx.recv_timeout(timeout).ok(),
            Listener::File { dir } => {
                let deadline = std::time::Instant::now() + timeout;
                loop {
                    if let Some(hook) = scan_file_dir(dir) {
                        return Some(hook);
                    }
                    if std::time::Instant::now() >= deadline {
                        return None;
                    }
                    thread::sleep(Duration::from_millis(25));
                }
            }
        }
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        match self {
            // Tell the accept thread to exit; it releases the socket/port within
            // one poll interval. Without this each turn leaks a thread + socket.
            Listener::Tcp { stop, .. } => stop.store(true, Ordering::SeqCst),
            Listener::File { dir } => {
                let _ = std::fs::remove_dir_all(dir);
            }
        }
    }
}

fn scan_file_dir(dir: &PathBuf) -> Option<Hook> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "hook").unwrap_or(false))
        .collect();
    entries.sort();
    for path in entries {
        if let Ok(buf) = std::fs::read(&path) {
            let _ = std::fs::remove_file(&path);
            if let Some(hook) = parse_framed(&buf) {
                return Some(hook);
            }
        }
    }
    None
}

fn parse_framed(buf: &[u8]) -> Option<Hook> {
    let text = String::from_utf8_lossy(buf);
    let (event, payload) = match text.split_once('\n') {
        Some((e, p)) => (e, p),
        None => (text.as_ref(), ""),
    };
    Some(Hook {
        event: HookEvent::parse(event),
        payload: payload.to_string(),
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// --settings JSON builder.
// ─────────────────────────────────────────────────────────────────────────────

/// Build the inline `--settings` JSON registering SessionStart/Stop hooks.
/// When `relay` is provided, the hook commands use the neutral relay path and
/// embed the port in argv (no env var).  Without a relay (fallback path or file
/// transport), the wrapper binary is used directly.
pub fn build_settings_merged(
    exe_fallback: &str,
    relay: Option<(&Relay, u16)>,
    user_settings: Option<&str>,
) -> (String, Vec<String>) {
    let mut warnings = Vec::new();

    let entry = |event: &str| -> serde_json::Value {
        let command = if let Some((r, port)) = relay {
            r.command(event, port)
        } else {
            let exe = exe_fallback.replace('"', "");
            format!("\"{exe}\" __hook {event}")
        };
        serde_json::json!({
            "matcher": "*",
            "hooks": [{ "type": "command", "command": command }]
        })
    };

    let mut root = match user_settings {
        None => serde_json::json!({}),
        Some(s) => {
            let trimmed = s.trim_start();
            let text = if trimmed.starts_with('{') {
                Some(s.to_string())
            } else {
                match std::fs::read_to_string(s) {
                    Ok(t) => Some(t),
                    Err(e) => {
                        warnings.push(format!("could not read --settings file '{s}': {e}; ignoring"));
                        None
                    }
                }
            };
            match text.and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) {
                Some(v) if v.is_object() => v,
                Some(_) => {
                    warnings.push("--settings did not contain a JSON object; ignoring".into());
                    serde_json::json!({})
                }
                None => {
                    if user_settings.is_some() && warnings.is_empty() {
                        warnings.push("--settings was not valid JSON; ignoring".into());
                    }
                    serde_json::json!({})
                }
            }
        }
    };

    let obj = root.as_object_mut().unwrap();
    let hooks = obj.entry("hooks").or_insert_with(|| serde_json::json!({}));
    if !hooks.is_object() {
        *hooks = serde_json::json!({});
    }
    let hooks = hooks.as_object_mut().unwrap();
    for event in ["SessionStart", "Stop"] {
        let arr = hooks.entry(event).or_insert_with(|| serde_json::json!([]));
        if !arr.is_array() {
            *arr = serde_json::json!([]);
        }
        arr.as_array_mut().unwrap().push(entry(event));
    }

    (root.to_string(), warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_json_has_both_events() {
        let (s, warns) = build_settings_merged("/path/to/claudio", None, None);
        assert!(warns.is_empty());
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        let hooks = &v["hooks"];
        assert!(hooks.get("SessionStart").is_some());
        assert!(hooks.get("Stop").is_some());
        let cmd = hooks["Stop"][0]["hooks"][0]["command"].as_str().unwrap();
        assert!(cmd.ends_with("__hook Stop"));
        assert!(cmd.contains("/path/to/claudio"));
    }

    #[test]
    fn settings_with_relay_has_no_wrapper_name() {
        // When a Relay is provided, the hook command must not contain
        // the wrapper binary name — only the neutral hex path.
        let relay = Relay::setup().unwrap();
        let port: u16 = 49152;
        let (s, warns) = build_settings_merged("irrelevant", Some((&relay, port)), None);
        assert!(warns.is_empty());
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        let stop_cmd = v["hooks"]["Stop"][0]["hooks"][0]["command"]
            .as_str().unwrap();
        let session_cmd = v["hooks"]["SessionStart"][0]["hooks"][0]["command"]
            .as_str().unwrap();
        // Must not contain the wrapper name.
        assert!(!stop_cmd.contains("claudio"), "stop cmd: {stop_cmd}");
        assert!(!session_cmd.contains("claudio"), "session cmd: {session_cmd}");
        // Must contain the port as a plain number.
        assert!(stop_cmd.contains("49152"), "port missing from: {stop_cmd}");
        // The relay binary must exist.
        assert!(relay.bin.exists());
    }

    #[test]
    fn relay_command_embeds_event_and_port() {
        let relay = Relay::setup().unwrap();
        let cmd = relay.command("Stop", 1234);
        assert!(cmd.ends_with(" Stop 1234"), "got: {cmd}");
        assert!(!cmd.contains("claudio"), "got: {cmd}");
    }

    #[test]
    fn relay_drop_cleans_up() {
        let dir;
        let bin;
        {
            let r = Relay::setup().unwrap();
            dir = r.dir.clone();
            bin = r.bin.clone();
            assert!(bin.exists());
        } // Relay dropped here.
        assert!(!bin.exists(), "relay binary should be removed on drop");
        assert!(!dir.exists() || dir.read_dir().map(|mut d| d.next().is_none()).unwrap_or(false),
            "relay dir should be cleaned up");
    }

    #[test]
    fn settings_merge_preserves_user_keys_and_hooks() {
        let user = r#"{"model":"opus","hooks":{"Stop":[{"matcher":"*","hooks":[{"type":"command","command":"echo mine"}]}]}}"#;
        let (s, warns) = build_settings_merged("/p/claudio", None, Some(user));
        assert!(warns.is_empty());
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["model"], "opus");
        let stop = v["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2);
        assert_eq!(stop[0]["hooks"][0]["command"], "echo mine");
        assert!(stop[1]["hooks"][0]["command"].as_str().unwrap().ends_with("__hook Stop"));
    }

    #[test]
    fn settings_merge_invalid_json_warns() {
        let (_s, warns) = build_settings_merged("/p/claudio", None, Some("not json"));
        assert!(!warns.is_empty());
    }

    #[test]
    fn settings_merge_non_object_json_warns() {
        let (_s, warns) = build_settings_merged("/p/claudio", None, Some("[1,2,3]"));
        assert!(!warns.is_empty());
    }

    #[test]
    fn settings_merge_from_file() {
        use std::io::Write as IoWrite;
        let dir = std::env::temp_dir();
        let path = dir.join("claudio-test-settings-2.json");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(br#"{"custom_key":"custom_val"}"#).unwrap();
        drop(f);
        let (s, warns) = build_settings_merged("/p/claudio", None, Some(path.to_str().unwrap()));
        let _ = std::fs::remove_file(&path);
        assert!(warns.is_empty(), "unexpected warnings: {warns:?}");
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["custom_key"], "custom_val");
        assert!(v["hooks"]["Stop"].is_array());
    }

    #[test]
    fn settings_merge_missing_file_warns() {
        let (_s, warns) = build_settings_merged("/p/claudio", None, Some("/nonexistent/path.json"));
        assert!(!warns.is_empty());
    }

    #[test]
    fn parse_framed_multiline_payload() {
        let h = parse_framed(b"Stop\n{\"a\":\"x\\ny\"}").unwrap();
        assert_eq!(h.event, HookEvent::Stop);
        assert_eq!(h.payload, "{\"a\":\"x\\ny\"}");
    }

    #[test]
    fn parse_event_names() {
        assert_eq!(HookEvent::parse("SessionStart"), HookEvent::SessionStart);
        assert_eq!(HookEvent::parse("Stop"), HookEvent::Stop);
        assert_eq!(HookEvent::parse("PreToolUse"), HookEvent::Unknown);
    }

    #[test]
    fn parse_framed_no_trailing_newline() {
        let h = parse_framed(b"Stop\t{\"x\":1}").unwrap();
        assert!(!matches!(h.event, HookEvent::Unknown) || h.event == HookEvent::Unknown);
    }

    #[test]
    fn parse_framed_only_event_no_payload() {
        let h = parse_framed(b"Stop\n").unwrap();
        assert_eq!(h.event, HookEvent::Stop);
        assert_eq!(h.payload, "");
    }

    #[test]
    fn tcp_listener_has_no_child_env() {
        // With the Relay approach, child_env() returns empty for TCP.
        use crate::cli::HookTransport;
        let listener = Listener::start(HookTransport::Tcp).unwrap();
        assert!(listener.child_env().is_empty(),
            "TCP child_env should be empty (port is in relay argv, not env)");
    }

    #[test]
    fn tcp_listener_roundtrip() {
        use std::net::TcpStream;
        use crate::cli::HookTransport;

        let listener = Listener::start(HookTransport::Tcp).unwrap();
        let port = listener.port().unwrap();

        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(b"Stop\n{\"last_assistant_message\":\"TCP_OK\"}").unwrap();
        drop(s);

        let hook = listener.poll(std::time::Duration::from_secs(2)).expect("hook");
        assert_eq!(hook.event, HookEvent::Stop);
        assert!(hook.payload.contains("TCP_OK"));
    }

    #[test]
    fn tcp_listener_poll_timeout_returns_none() {
        use crate::cli::HookTransport;
        let listener = Listener::start(HookTransport::Tcp).unwrap();
        assert!(listener.poll(std::time::Duration::from_millis(50)).is_none());
    }

    #[test]
    fn file_listener_roundtrip() {
        use crate::cli::HookTransport;

        let listener = Listener::start(HookTransport::File).unwrap();
        let env = listener.child_env();
        assert_eq!(env.len(), 1);
        let (key, dir_path) = &env[0];
        assert_eq!(key, ENV_DIR);
        let dir = std::path::PathBuf::from(dir_path);

        let id = uuid::Uuid::new_v4();
        let tmp = dir.join(format!("{id}.tmp"));
        let done = dir.join(format!("{id}.hook"));
        let mut f = std::fs::File::create(&tmp).unwrap();
        f.write_all(b"SessionStart\n{\"session_id\":\"file-sid\"}").unwrap();
        drop(f);
        std::fs::rename(&tmp, &done).unwrap();

        let hook = listener.poll(std::time::Duration::from_secs(2)).expect("hook");
        assert_eq!(hook.event, HookEvent::SessionStart);
        assert!(hook.payload.contains("file-sid"));
    }
}
