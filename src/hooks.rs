//! Completion signalling. We register `SessionStart` and `Stop` hooks via
//! inline `--settings` JSON. Both point at *our own binary* in relay mode
//! (`claude-poc __hook <Event>`), so there is no `/bin/sh`-vs-`cmd.exe`
//! quoting hazard — we control both ends.
//!
//! Two transports carry the payload from the relay subprocess back to the
//! parent:
//!   * `tcp`  — relay connects to a 127.0.0.1:<port> listener (default).
//!   * `file` — relay drops a file into a watched directory (fallback for
//!              sandboxes that block loopback TCP).
//!
//! Report note: every load-bearing piece here — inline `--settings`, the
//! `SessionStart`/`Stop` lifecycle hooks, `command`-type hooks — is a
//! documented, supported Claude Code feature.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crate::cli::HookTransport;

pub const ENV_PORT: &str = "CLAUDE_POC_HOOK_PORT";
pub const ENV_DIR: &str = "CLAUDE_POC_HOOK_DIR";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookEvent {
    SessionStart,
    Stop,
    Unknown,
}

impl HookEvent {
    fn parse(s: &str) -> Self {
        match s.trim() {
            "SessionStart" => HookEvent::SessionStart,
            "Stop" => HookEvent::Stop,
            _ => HookEvent::Unknown,
        }
    }
}

/// A delivered hook: the lifecycle event plus its raw JSON payload string.
pub struct Hook {
    pub event: HookEvent,
    pub payload: String,
}

// ----------------------------------------------------------------------------
// Relay mode (runs as a short-lived subprocess spawned by Claude Code).
// ----------------------------------------------------------------------------

/// Entry point for `claude-poc __hook <Event>`. Reads the JSON payload from
/// stdin and ships it to the parent via whichever transport env var is set.
pub fn run_relay(event: &str) {
    let mut payload = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut payload);

    if let Ok(port) = std::env::var(ENV_PORT) {
        if let Ok(port) = port.parse::<u16>() {
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
            // Atomic publish: a reader only ever sees a complete *.hook file.
            let _ = std::fs::rename(&tmp, &done);
        }
    }
}

// ----------------------------------------------------------------------------
// Listener (runs in the parent; yields hook events as they arrive).
// ----------------------------------------------------------------------------

pub enum Listener {
    Tcp { rx: mpsc::Receiver<Hook>, port: u16 },
    File { dir: PathBuf },
}

impl Listener {
    pub fn start(transport: HookTransport) -> std::io::Result<Self> {
        match transport {
            HookTransport::Tcp => {
                let listener = TcpListener::bind(("127.0.0.1", 0))?;
                let port = listener.local_addr()?.port();
                let (tx, rx) = mpsc::channel();
                thread::spawn(move || {
                    for stream in listener.incoming() {
                        let Ok(mut s) = stream else { continue };
                        let mut buf = Vec::new();
                        if s.read_to_end(&mut buf).is_err() {
                            continue;
                        }
                        if let Some(hook) = parse_framed(&buf) {
                            let _ = tx.send(hook);
                        }
                    }
                });
                Ok(Listener::Tcp { rx, port })
            }
            HookTransport::File => {
                let dir = std::env::temp_dir().join(format!("claude-poc-{}", uuid::Uuid::new_v4()));
                std::fs::create_dir_all(&dir)?;
                Ok(Listener::File { dir })
            }
        }
    }

    /// Environment variables the child `claude` must carry so the relay can
    /// reach us.
    pub fn child_env(&self) -> Vec<(String, String)> {
        match self {
            Listener::Tcp { port, .. } => vec![(ENV_PORT.to_string(), port.to_string())],
            Listener::File { dir } => {
                vec![(ENV_DIR.to_string(), dir.to_string_lossy().into_owned())]
            }
        }
    }

    /// Block up to `timeout` for the next hook event.
    pub fn poll(&self, timeout: Duration) -> Option<Hook> {
        match self {
            Listener::Tcp { rx, .. } => rx.recv_timeout(timeout).ok(),
            Listener::File { dir } => {
                let deadline = std::time::Instant::now() + timeout;
                loop {
                    if let Some(hook) = self.scan_file_dir(dir) {
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

    fn scan_file_dir(&self, dir: &PathBuf) -> Option<Hook> {
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
}

impl Drop for Listener {
    fn drop(&mut self) {
        if let Listener::File { dir } = self {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// "<event>\n<payload...>" → Hook. The payload may itself contain newlines.
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

// ----------------------------------------------------------------------------
// Inline --settings JSON.
// ----------------------------------------------------------------------------

/// One hook entry that invokes our binary in relay mode. The exe path is
/// double-quoted so a path with spaces survives both `/bin/sh -c` and
/// `cmd.exe /c` on Windows.
fn hook_entry(exe: &str, event: &str) -> serde_json::Value {
    serde_json::json!({
        "matcher": "*",
        "hooks": [{ "type": "command", "command": format!("\"{exe}\" __hook {event}") }]
    })
}

/// Build inline `--settings` JSON registering our `SessionStart`/`Stop` hooks
/// on top of the user's existing settings (if any).
///
/// `user_settings` may be inline JSON (`{...}`) or a path to a settings file.
/// The user's settings are preserved; our hook entries are *appended* to the
/// `SessionStart` and `Stop` arrays so the user's own hooks still fire.
pub fn build_settings_merged(exe: &str, user_settings: Option<&str>) -> (String, Vec<String>) {
    let exe = exe.replace('"', "");
    let mut warnings = Vec::new();

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

    // Ensure root.hooks is an object, then append our entries to each event's
    // array (creating the array if absent).
    let obj = root.as_object_mut().unwrap();
    let hooks = obj
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));
    if !hooks.is_object() {
        *hooks = serde_json::json!({});
    }
    let hooks = hooks.as_object_mut().unwrap();
    for event in ["SessionStart", "Stop"] {
        let arr = hooks.entry(event).or_insert_with(|| serde_json::json!([]));
        if !arr.is_array() {
            *arr = serde_json::json!([]);
        }
        arr.as_array_mut().unwrap().push(hook_entry(&exe, event));
    }

    (root.to_string(), warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_json_has_both_events() {
        let (s, warns) = build_settings_merged("/path/to/claude-poc", None);
        assert!(warns.is_empty());
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        let hooks = &v["hooks"];
        assert!(hooks.get("SessionStart").is_some());
        assert!(hooks.get("Stop").is_some());
        let cmd = hooks["Stop"][0]["hooks"][0]["command"].as_str().unwrap();
        assert!(cmd.ends_with("__hook Stop"));
        assert!(cmd.contains("/path/to/claude-poc"));
    }

    #[test]
    fn settings_merge_preserves_user_keys_and_hooks() {
        let user = r#"{"model":"opus","hooks":{"Stop":[{"matcher":"*","hooks":[{"type":"command","command":"echo mine"}]}]}}"#;
        let (s, warns) = build_settings_merged("/p/claude-poc", Some(user));
        assert!(warns.is_empty());
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["model"], "opus");
        // user's Stop hook preserved, ours appended
        let stop = v["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2);
        assert_eq!(stop[0]["hooks"][0]["command"], "echo mine");
        assert!(stop[1]["hooks"][0]["command"].as_str().unwrap().ends_with("__hook Stop"));
    }

    #[test]
    fn settings_merge_invalid_json_warns() {
        let (_s, warns) = build_settings_merged("/p/claude-poc", Some("not json"));
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
}
