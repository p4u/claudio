//! Wire protocol between the claudio TUI client and a per-host daemon.
//!
//! A frame is `[u32 BE length][u8 tag][payload]`, where `length` counts the tag
//! and the payload:
//!
//! - tag `J` — a JSON control message ([`Envelope`]).
//! - tag `D` — terminal bytes for one session: a 16-byte session id, then raw
//!   bytes. Client→daemon it is keyboard/mouse input; daemon→client it is PTY
//!   output. The first `D` frame after `Attached` is the screen snapshot.
//!
//! JSON keeps the control plane debuggable (`socat`) and tolerant of version
//! skew — unknown fields are ignored — which matters because a running daemon
//! may be older than the client talking to it. Only terminal I/O, the hot path,
//! is binary.

use std::io;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use uuid::Uuid;

/// Protocol version. Bump on incompatible changes; it is part of the daemon
/// socket name, so daemons of different versions coexist instead of colliding.
pub const PROTO: u32 = 1;

/// Hard cap on a single frame (tag + payload).
pub const MAX_FRAME: usize = 1 << 20;

const TAG_JSON: u8 = b'J';
const TAG_DATA: u8 = b'D';

/// A manager session id (stable across claude restarts and `/clear`).
pub type SessionId = Uuid;

/// One protocol frame.
#[derive(Debug, Clone, PartialEq)]
pub enum Frame {
    Control(Envelope),
    Data { session: SessionId, bytes: Vec<u8> },
}

/// A control message plus an optional request id. Requests carry `req`; the
/// reply echoes it. Pushed events carry none.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub req: Option<u64>,
    #[serde(flatten)]
    pub msg: Msg,
}

impl Envelope {
    pub fn request(req: u64, msg: Msg) -> Self {
        Self { req: Some(req), msg }
    }

    pub fn event(msg: Msg) -> Self {
        Self { req: None, msg }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Msg {
    // ── handshake ────────────────────────────────────────────────────────
    /// First message from a client.
    Hello(Hello),
    /// The daemon's answer to `Hello`.
    Welcome(Welcome),

    // ── client → daemon requests ─────────────────────────────────────────
    ListSessions,
    /// Start claude in a new PTY. Idempotent by `spec.id`: re-sending a spawn
    /// for a live session just answers `Spawned` again.
    Spawn(SpawnSpec),
    /// Subscribe to a session's output (and resize it to the client's pane).
    /// Answered by `Attached`, immediately followed by `D` frames carrying the
    /// screen snapshot.
    Attach { id: SessionId, rows: u16, cols: u16 },
    Detach { id: SessionId },
    Resize { id: SessionId, rows: u16, cols: u16 },
    Kill { id: SessionId },
    ListDir { path: String },
    ListClaudeSessions { cwd: String },
    /// Directories with claude history on this host, newest first (seeds the
    /// new-session directory picker).
    RecentProjects { limit: u32 },
    Ping,

    // ── hook relay (`claudio __hook`) → daemon ───────────────────────────
    /// A Claude Code hook fired. `token` identifies the session generation it
    /// belongs to; `payload` is the hook's stdin JSON, verbatim.
    Hook { token: String, event: String, payload: serde_json::Value },

    // ── daemon → client replies ──────────────────────────────────────────
    Sessions { sessions: Vec<SessionInfo> },
    Spawned { id: SessionId, pid: Option<u32> },
    /// The subscriber must reset its mirror to `rows`×`cols` and apply the
    /// snapshot that follows. Also re-sent unprompted when the session is
    /// resized by another client or the subscriber fell behind.
    Attached { id: SessionId, rows: u16, cols: u16 },
    DirEntries { path: String, entries: Vec<DirEntry> },
    ClaudeSessions { cwd: String, sessions: Vec<ClaudeSession> },
    Projects { dirs: Vec<ProjectDir> },
    Ok,
    Error { message: String },
    Pong,

    // ── daemon → client pushed events ────────────────────────────────────
    Event { id: SessionId, event: SessionEvent },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hello {
    pub claudio_version: String,
    pub proto: u32,
    /// The client's real terminal colors, so the daemon can answer the
    /// child's OSC 10/11 queries truthfully.
    #[serde(default)]
    pub colors: Option<Colors>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Welcome {
    pub claudio_version: String,
    pub proto: u32,
    pub host: HostInfo,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Colors {
    pub fg: [u8; 3],
    pub bg: [u8; 3],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostInfo {
    pub hostname: String,
    pub os: String,
    pub arch: String,
    pub home: String,
    /// `None` when `claude` is not installed on this host.
    #[serde(default)]
    pub claude: Option<ClaudeInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClaudeInfo {
    pub path: String,
    pub version: String,
}

/// How to start a session. `args` are extra claude arguments (e.g.
/// `["--resume", "<uuid>"]`); the daemon adds the claude binary and its own
/// hook settings. `env` may carry secrets and is redacted from `Debug`.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct SpawnSpec {
    pub id: SessionId,
    pub cwd: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Vec<(String, String)>,
    pub rows: u16,
    pub cols: u16,
}

impl std::fmt::Debug for SpawnSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let env_keys: Vec<&str> = self.env.iter().map(|(k, _)| k.as_str()).collect();
        f.debug_struct("SpawnSpec")
            .field("id", &self.id)
            .field("cwd", &self.cwd)
            .field("name", &self.name)
            .field("args", &self.args)
            .field("env", &env_keys)
            .field("rows", &self.rows)
            .field("cols", &self.cols)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: SessionId,
    pub cwd: String,
    #[serde(default)]
    pub name: Option<String>,
    pub state: SessionState,
    /// The current claude conversation id (changes on `/clear` and forks).
    #[serde(default)]
    pub claude_session_id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub pid: Option<u32>,
    /// Unix seconds.
    pub created_at: u64,
}

/// What a session is doing, derived from Claude Code hooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// Spawned; no hook has fired yet.
    Starting,
    Working,
    /// A permission prompt or elicitation dialog is waiting for the user.
    NeedsApproval,
    /// Claude is waiting for the user's next message.
    NeedsInput,
    /// The turn finished.
    Idle,
    /// The turn failed (`StopFailure`).
    Error,
    Exited,
    /// State is not known (e.g. reattached before any hook fired).
    Unknown,
}

impl SessionState {
    /// Whether the user should be pulled to this session.
    pub fn wants_attention(self) -> bool {
        matches!(self, Self::NeedsApproval | Self::NeedsInput | Self::Error)
    }
}

/// Session changes, broadcast to every connected client (attached or not), so
/// tab bars stay current.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionEvent {
    /// A session was spawned (possibly by another client).
    Created { info: SessionInfo },
    /// A session was killed and forgotten.
    Removed,
    State { state: SessionState },
    ClaudeSession { claude_session_id: String },
    Title { title: String },
    Exited { code: Option<i32> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DirEntry {
    pub name: String,
    pub dir: bool,
}

/// A directory with claude history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectDir {
    pub path: String,
    /// Unix seconds of the newest transcript there.
    pub modified: u64,
}

/// A resumable claude conversation found under `~/.claude/projects`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClaudeSession {
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub last_prompt: Option<String>,
    /// Unix seconds of the transcript's last modification.
    pub modified: u64,
    pub messages: u32,
}

impl Frame {
    /// Serialize into `[len][tag][payload]`.
    pub fn encode(&self) -> Vec<u8> {
        let mut body = Vec::new();
        match self {
            Frame::Control(env) => {
                body.push(TAG_JSON);
                // Serializing our own types cannot fail.
                serde_json::to_writer(&mut body, env).expect("serialize envelope");
            }
            Frame::Data { session, bytes } => {
                body.reserve(1 + 16 + bytes.len());
                body.push(TAG_DATA);
                body.extend_from_slice(session.as_bytes());
                body.extend_from_slice(bytes);
            }
        }
        let mut out = Vec::with_capacity(4 + body.len());
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(&body);
        out
    }

    /// Parse a frame body (tag + payload, without the length prefix).
    pub fn decode(body: &[u8]) -> io::Result<Frame> {
        match body.split_first() {
            Some((&TAG_JSON, json)) => serde_json::from_slice(json)
                .map(Frame::Control)
                .map_err(|e| invalid(format!("bad control frame: {e}"))),
            Some((&TAG_DATA, rest)) if rest.len() >= 16 => {
                let (id, bytes) = rest.split_at(16);
                let session = Uuid::from_slice(id).expect("16-byte slice");
                Ok(Frame::Data { session, bytes: bytes.to_vec() })
            }
            Some((tag, _)) => Err(invalid(format!("unknown or short frame (tag {tag:#04x})"))),
            None => Err(invalid("empty frame".into())),
        }
    }
}

/// Read one frame. `Ok(None)` means the peer closed the stream cleanly.
pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> io::Result<Option<Frame>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_be_bytes(len) as usize;
    if len == 0 || len > MAX_FRAME {
        return Err(invalid(format!("frame length {len} out of range")));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).await?;
    Frame::decode(&body).map(Some)
}

/// Write one frame and flush.
pub async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, frame: &Frame) -> io::Result<()> {
    let bytes = frame.encode();
    if bytes.len() - 4 > MAX_FRAME {
        return Err(invalid(format!("frame of {} bytes exceeds the cap", bytes.len() - 4)));
    }
    w.write_all(&bytes).await?;
    w.flush().await
}

fn invalid(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(frame: Frame) -> Frame {
        let bytes = frame.encode();
        let len = u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize;
        assert_eq!(len, bytes.len() - 4);
        Frame::decode(&bytes[4..]).unwrap()
    }

    #[test]
    fn control_roundtrip() {
        let f = Frame::Control(Envelope::request(
            7,
            Msg::Attach { id: Uuid::new_v4(), rows: 40, cols: 120 },
        ));
        assert_eq!(roundtrip(f.clone()), f);
    }

    #[test]
    fn data_roundtrip() {
        let f = Frame::Data { session: Uuid::new_v4(), bytes: b"\x1b[?2026h hi".to_vec() };
        assert_eq!(roundtrip(f.clone()), f);
    }

    #[test]
    fn json_shape_is_flat_and_tagged() {
        let env = Envelope::request(1, Msg::ListDir { path: "/srv".into() });
        let v: serde_json::Value = serde_json::to_value(&env).unwrap();
        assert_eq!(v, serde_json::json!({"req": 1, "op": "list_dir", "path": "/srv"}));
        let ev = Envelope::event(Msg::Event {
            id: Uuid::nil(),
            event: SessionEvent::State { state: SessionState::NeedsApproval },
        });
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["event"], serde_json::json!({"kind": "state", "state": "needs_approval"}));
        assert!(v.get("req").is_none());
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let json = br#"J{"op":"kill","id":"00000000-0000-0000-0000-000000000000","future":true}"#;
        let f = Frame::decode(json).unwrap();
        assert_eq!(f, Frame::Control(Envelope::event(Msg::Kill { id: Uuid::nil() })));
    }

    #[test]
    fn spawn_debug_redacts_env_values() {
        let spec = SpawnSpec {
            id: Uuid::nil(),
            cwd: "/w".into(),
            name: None,
            args: vec![],
            env: vec![("ANTHROPIC_AUTH_TOKEN".into(), "s3cret".into())],
            rows: 24,
            cols: 80,
        };
        let dbg = format!("{spec:?}");
        assert!(dbg.contains("ANTHROPIC_AUTH_TOKEN"));
        assert!(!dbg.contains("s3cret"));
    }

    #[test]
    fn rejects_garbage() {
        assert!(Frame::decode(b"").is_err());
        assert!(Frame::decode(b"X123").is_err());
        assert!(Frame::decode(b"Dshort").is_err());
        assert!(Frame::decode(b"J{not json").is_err());
    }

    #[tokio::test]
    async fn async_stream_roundtrip_and_eof() {
        let (mut a, mut b) = tokio::io::duplex(64 * 1024);
        let f1 = Frame::Control(Envelope::request(1, Msg::Ping));
        let f2 = Frame::Data { session: Uuid::new_v4(), bytes: vec![0u8; 50_000] };
        write_frame(&mut a, &f1).await.unwrap();
        write_frame(&mut a, &f2).await.unwrap();
        drop(a);
        assert_eq!(read_frame(&mut b).await.unwrap(), Some(f1));
        assert_eq!(read_frame(&mut b).await.unwrap(), Some(f2));
        assert_eq!(read_frame(&mut b).await.unwrap(), None);
    }

    #[tokio::test]
    async fn rejects_oversized_length() {
        let (mut a, mut b) = tokio::io::duplex(64);
        a.write_all(&((MAX_FRAME as u32) + 1).to_be_bytes()).await.unwrap();
        assert!(read_frame(&mut b).await.is_err());
    }
}
