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
        Self {
            req: Some(req),
            msg,
        }
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
    /// Start the user's login shell in a new PTY (a terminal tab). Idempotent
    /// by `spec.id`, answered by `Spawned`. An older daemon answers `Error
    /// "unsupported op"`, so a terminal can never be mistaken for claude.
    SpawnShell(ShellSpec),
    /// Restart a journaled session in place (same id, tab and journal entry):
    /// kill its process if it has one, then start it again, resuming its
    /// conversation unless `fresh`. Answered by `Spawned`; clients see
    /// `Created`, never `Removed`. An older daemon answers `Error
    /// "unsupported op…"`.
    Respawn(RespawnSpec),
    /// Subscribe to a session's output (and resize it to the client's pane).
    /// Answered by `Attached`, immediately followed by `D` frames carrying the
    /// screen snapshot.
    Attach {
        id: SessionId,
        rows: u16,
        cols: u16,
    },
    Detach {
        id: SessionId,
    },
    Resize {
        id: SessionId,
        rows: u16,
        cols: u16,
    },
    Kill {
        id: SessionId,
    },
    /// Update the human-readable name for a session. `None` clears the name.
    /// The daemon persists the rename in its journal so it survives restarts.
    Rename {
        id: SessionId,
        name: Option<String>,
    },
    ListDir {
        path: String,
    },
    ListClaudeSessions {
        cwd: String,
    },
    /// Directories with claude history on this host, newest first (seeds the
    /// new-session directory picker).
    RecentProjects {
        limit: u32,
    },
    Ping,
    /// Request a clean shutdown. Served only to our own uid; the daemon kills
    /// all live sessions and exits. Sessions remain journaled as dormant.
    Shutdown,

    // ── hook relay (`claudio __hook`) → daemon ───────────────────────────
    /// A Claude Code hook fired. `token` identifies the session generation it
    /// belongs to; `payload` is the hook's stdin JSON, verbatim.
    Hook {
        token: String,
        event: String,
        payload: serde_json::Value,
    },

    // ── host stats subscription (opt-in, additive) ───────────────────────
    /// Client opts in to receiving periodic `HostStats` pushes from the daemon.
    /// An old daemon that does not know this op answers `Error`; the client
    /// treats that as "unsupported" and hides the sparklines.
    SubscribeHostStats,

    // ── claude maintenance (opt-in, additive) ────────────────────────────
    /// Run `claude update` on the daemon's host, or the official installer
    /// when `install` (claude is missing). Answered by `ClaudeUpdated` once it
    /// finishes (minutes, at most); running sessions are left alone. An old
    /// daemon answers `Error "unsupported op…"`.
    UpdateClaude {
        #[serde(default)]
        install: bool,
    },
    // ── git viewer (read-only; see `daemon::git`) ────────────────────────
    /// One page of `git log` for `cwd`, newest first. `limit` is capped at
    /// [`GIT_LOG_MAX`]. Answered by `GitLogPage`.
    GitLog {
        cwd: String,
        #[serde(default)]
        all: bool,
        #[serde(default)]
        skip: u32,
        limit: u32,
    },
    /// One commit's metadata and file list. `id` must be a full object id.
    /// Answered by `GitCommitInfo`.
    GitCommit {
        cwd: String,
        id: String,
    },
    /// A patch: one file of a commit, or the whole commit when `path` is
    /// `None`. `old_path` is the pre-rename name. Answered by `GitPatch`.
    GitDiff {
        cwd: String,
        id: String,
        #[serde(default)]
        path: Option<String>,
        #[serde(default)]
        old_path: Option<String>,
    },

    // ── daemon → client replies ──────────────────────────────────────────
    Sessions {
        sessions: Vec<SessionInfo>,
    },
    Spawned {
        id: SessionId,
        pid: Option<u32>,
    },
    /// The subscriber must reset its mirror to `rows`×`cols` and apply the
    /// snapshot that follows. Also re-sent unprompted when the session is
    /// resized by another client or the subscriber fell behind.
    Attached {
        id: SessionId,
        rows: u16,
        cols: u16,
    },
    DirEntries {
        path: String,
        entries: Vec<DirEntry>,
        /// Set when the entry list was capped to fit within the frame size limit.
        #[serde(default)]
        truncated: bool,
    },
    ClaudeSessions {
        cwd: String,
        sessions: Vec<ClaudeSession>,
    },
    Projects {
        dirs: Vec<ProjectDir>,
    },
    /// The outcome of `UpdateClaude`. `version` is the host's re-probed
    /// `claude --version` (`None` when claude is still missing); `tail` is the
    /// last few KiB of the command's output.
    ClaudeUpdated {
        version: Option<String>,
        ok: bool,
        tail: String,
    },
    GitLogPage(GitLogPage),
    GitCommitInfo(GitCommitInfo),
    GitPatch(GitPatch),
    Ok,
    Error {
        message: String,
    },
    Pong,

    // ── daemon → client pushed events ────────────────────────────────────
    Event {
        id: SessionId,
        event: SessionEvent,
    },
    /// Pushed every ~2 s to subscribed clients. Carries the host's current
    /// CPU and memory utilisation so the TUI can render sparklines.
    HostStats {
        cpu_pct: f32,
        mem_used: u64,
        mem_total: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        load1: Option<f32>,
    },

    /// Catch-all for ops this client does not recognise yet.
    /// Keeps older clients alive when a newer daemon sends a new op.
    #[serde(other)]
    Unknown,
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

/// How to restart a session. Everything else (cwd, name, claude arguments,
/// kind) comes from the daemon's journal. `env` may carry secrets and is
/// redacted from `Debug`.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RespawnSpec {
    pub id: SessionId,
    /// Start a new conversation instead of resuming the journaled one.
    #[serde(default)]
    pub fresh: bool,
    #[serde(default)]
    pub env: Vec<(String, String)>,
    pub rows: u16,
    pub cols: u16,
}

/// How to start a terminal tab: the login shell, in `cwd`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShellSpec {
    pub id: SessionId,
    pub cwd: String,
    #[serde(default)]
    pub name: Option<String>,
    pub rows: u16,
    pub cols: u16,
}

/// What a session runs. The daemon journal's value is authoritative.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionKind {
    #[default]
    Claude,
    /// A plain terminal: the user's login shell.
    Shell,
}

impl Msg {
    /// `self` with the session env set, if it is a `Spawn` or `Respawn`; any
    /// other message is returned unchanged.
    pub fn with_env(mut self, env: Vec<(String, String)>) -> Msg {
        match &mut self {
            Msg::Spawn(spec) => spec.env = env,
            Msg::Respawn(spec) => spec.env = env,
            _ => {}
        }
        self
    }
}

impl std::fmt::Debug for RespawnSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let env_keys: Vec<&str> = self.env.iter().map(|(k, _)| k.as_str()).collect();
        f.debug_struct("RespawnSpec")
            .field("id", &self.id)
            .field("fresh", &self.fresh)
            .field("env", &env_keys)
            .field("rows", &self.rows)
            .field("cols", &self.cols)
            .finish()
    }
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
    /// Git branch of the session's cwd (None when not a git repo or unknown).
    /// Added in a later protocol version; old daemons omit this field.
    #[serde(default)]
    pub branch: Option<String>,
    /// Short human-readable model name (e.g. `"opus-4.5"`).
    /// Added in a later protocol version; old daemons omit this field.
    #[serde(default)]
    pub model: Option<String>,
    /// Total input+cache tokens of the last assistant turn.
    /// Added in a later protocol version; old daemons omit this field.
    #[serde(default)]
    pub context_tokens: Option<u64>,
    /// Absent from old daemons, which only run claude.
    #[serde(default)]
    pub kind: SessionKind,
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
    Created {
        info: SessionInfo,
    },
    /// A session was killed and forgotten.
    Removed,
    State {
        state: SessionState,
    },
    ClaudeSession {
        claude_session_id: String,
    },
    Title {
        title: String,
    },
    Exited {
        code: Option<i32>,
    },
    /// The session's human-readable name was updated (or cleared).
    Renamed {
        name: Option<String>,
    },
    /// A transient user-visible notice (e.g. resume-fallback).
    Notice {
        text: String,
    },
    /// Session metadata updated: git branch, active model, context token count.
    /// Broadcast on `SessionStart`, `Stop`, `UserPromptSubmit` hooks and when
    /// the claude session id changes. Old clients map this to `Unknown`.
    Meta {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        branch: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context_tokens: Option<u64>,
    },
    /// Catch-all for event kinds this client doesn't recognise yet.
    /// Keeps older clients alive when the daemon sends a newer event kind.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DirEntry {
    pub name: String,
    pub dir: bool,
    /// The git branch name when the directory is a git repo, `Some("")` for a
    /// detached/unknown HEAD, `None` when not a git repo. Filled by the daemon;
    /// older daemons leave this as `None`.
    #[serde(default)]
    pub git: Option<String>,
    /// Unix seconds of the last claude session activity in this directory.
    /// `None` when no claude session is known. Filled by the daemon.
    #[serde(default)]
    pub claude_at: Option<u64>,
    /// Whether the directory entry is a symlink.
    #[serde(default)]
    pub symlink: bool,
    /// Whether the entry name starts with `.`.
    #[serde(default)]
    pub hidden: bool,
}

impl DirEntry {
    /// Construct a minimal entry with no enrichment. Used in tests and by
    /// callers that do their own field assignment.
    #[allow(dead_code)]
    pub fn simple(name: impl Into<String>, dir: bool) -> Self {
        DirEntry {
            name: name.into(),
            dir,
            git: None,
            claude_at: None,
            symlink: false,
            hidden: false,
        }
    }
}

/// A directory with claude history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectDir {
    pub path: String,
    /// Unix seconds of the newest transcript there.
    pub modified: u64,
    /// Git branch name, `Some("")` for detached HEAD, `None` for non-git.
    #[serde(default)]
    pub git: Option<String>,
    /// Whether the path is a symlink.
    #[serde(default)]
    pub symlink: bool,
    /// Whether the path's basename starts with `.`.
    #[serde(default)]
    pub hidden: bool,
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

/// The most commits one `GitLog` request may ask for.
pub const GIT_LOG_MAX: u32 = 500;

/// Reply to `GitLog`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitLogPage {
    /// The repository's top-level directory.
    pub root: String,
    /// The checked-out branch; `None` for a detached HEAD.
    #[serde(default)]
    pub head: Option<String>,
    pub commits: Vec<GitLogEntry>,
    /// Whether commits exist beyond this page.
    #[serde(default)]
    pub more: bool,
}

/// One log row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitLogEntry {
    /// Full object id.
    pub id: String,
    pub parents: Vec<String>,
    pub author: String,
    /// Author time, Unix seconds.
    pub time: i64,
    /// Decorations as `--decorate=full` prints them, one per entry:
    /// `HEAD -> refs/heads/main`, `refs/remotes/origin/main`, `tag: refs/tags/v1`.
    pub refs: Vec<String>,
    pub subject: String,
}

/// Reply to `GitCommit`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitCommitInfo {
    pub id: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    pub time: i64,
    pub committer_time: i64,
    pub refs: Vec<String>,
    /// The full message, subject first.
    pub message: String,
    pub files: Vec<GitFile>,
    /// Set for merges: `files` and patches are against the first parent only.
    #[serde(default)]
    pub first_parent: bool,
}

/// A file changed by a commit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitFile {
    pub path: String,
    /// The previous name, for a rename.
    #[serde(default)]
    pub old_path: Option<String>,
    /// Lines added; `None` for a binary file.
    #[serde(default)]
    pub added: Option<u32>,
    /// Lines removed; `None` for a binary file.
    #[serde(default)]
    pub removed: Option<u32>,
}

/// Reply to `GitDiff`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitPatch {
    pub id: String,
    pub path: Option<String>,
    pub patch: String,
    /// Set when the patch was cut to fit the size limits.
    #[serde(default)]
    pub truncated: bool,
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
                Ok(Frame::Data {
                    session,
                    bytes: bytes.to_vec(),
                })
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
        return Err(invalid(format!(
            "frame of {} bytes exceeds the cap",
            bytes.len() - 4
        )));
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
    fn session_info_without_kind_is_claude() {
        let old = r#"{"id":"00000000-0000-0000-0000-000000000000","cwd":"/w",
            "state":"idle","created_at":1}"#;
        let info: SessionInfo = serde_json::from_str(old).unwrap();
        assert_eq!(info.kind, SessionKind::Claude);
        let json = serde_json::to_value(SessionKind::Shell).unwrap();
        assert_eq!(json, "shell");
    }

    #[test]
    fn spawn_shell_is_unknown_to_a_daemon_or_client_that_lacks_it() {
        // A frozen copy of the pre-terminal enum: the `#[serde(other)]`
        // fallback is all that keeps the connection alive.
        #[derive(Debug, PartialEq, Deserialize)]
        #[serde(tag = "op", rename_all = "snake_case")]
        enum V02 {
            Ping,
            #[serde(other)]
            Unknown,
        }
        let shell = Msg::SpawnShell(ShellSpec {
            id: Uuid::nil(),
            cwd: "/w".into(),
            name: None,
            rows: 24,
            cols: 80,
        });
        let json = serde_json::to_string(&Envelope::request(3, shell.clone())).unwrap();
        assert!(json.contains(r#""op":"spawn_shell""#), "{json}");
        assert_eq!(serde_json::from_str::<V02>(&json).unwrap(), V02::Unknown);
        let back: Envelope = serde_json::from_str(&json).unwrap();
        assert_eq!(back.msg, shell);
    }

    #[test]
    fn respawn_roundtrips_and_keeps_secrets_out_of_debug() {
        let respawn = Msg::Respawn(RespawnSpec {
            id: Uuid::nil(),
            fresh: true,
            env: vec![("ANTHROPIC_AUTH_TOKEN".into(), "sekrit".into())],
            rows: 24,
            cols: 80,
        });
        let json = serde_json::to_string(&Envelope::request(4, respawn.clone())).unwrap();
        assert!(json.contains(r#""op":"respawn""#) && json.contains(r#""fresh":true"#));
        let back: Envelope = serde_json::from_str(&json).unwrap();
        assert_eq!(back.msg, respawn);
        let debug = format!("{respawn:?}");
        assert!(debug.contains("ANTHROPIC_AUTH_TOKEN") && !debug.contains("sekrit"));
        assert_eq!(
            respawn.with_env(vec![]),
            Msg::Respawn(RespawnSpec {
                id: Uuid::nil(),
                fresh: true,
                env: vec![],
                rows: 24,
                cols: 80,
            })
        );
    }

    #[test]
    fn control_roundtrip() {
        let f = Frame::Control(Envelope::request(
            7,
            Msg::Attach {
                id: Uuid::new_v4(),
                rows: 40,
                cols: 120,
            },
        ));
        assert_eq!(roundtrip(f.clone()), f);
    }

    #[test]
    fn data_roundtrip() {
        let f = Frame::Data {
            session: Uuid::new_v4(),
            bytes: b"\x1b[?2026h hi".to_vec(),
        };
        assert_eq!(roundtrip(f.clone()), f);
    }

    #[test]
    fn json_shape_is_flat_and_tagged() {
        let env = Envelope::request(
            1,
            Msg::ListDir {
                path: "/srv".into(),
            },
        );
        let v: serde_json::Value = serde_json::to_value(&env).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"req": 1, "op": "list_dir", "path": "/srv"})
        );
        let ev = Envelope::event(Msg::Event {
            id: Uuid::nil(),
            event: SessionEvent::State {
                state: SessionState::NeedsApproval,
            },
        });
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(
            v["event"],
            serde_json::json!({"kind": "state", "state": "needs_approval"})
        );
        assert!(v.get("req").is_none());
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let json = br#"J{"op":"kill","id":"00000000-0000-0000-0000-000000000000","future":true}"#;
        let f = Frame::decode(json).unwrap();
        assert_eq!(
            f,
            Frame::Control(Envelope::event(Msg::Kill { id: Uuid::nil() }))
        );
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
        let f2 = Frame::Data {
            session: Uuid::new_v4(),
            bytes: vec![0u8; 50_000],
        };
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
        a.write_all(&((MAX_FRAME as u32) + 1).to_be_bytes())
            .await
            .unwrap();
        assert!(read_frame(&mut b).await.is_err());
    }

    #[test]
    fn session_event_notice_roundtrips() {
        let event = SessionEvent::Notice {
            text: "resume fallback".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let back: SessionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, event);
    }

    #[test]
    fn git_requests_default_optional_fields_and_replies_have_distinct_ops() {
        let json = br#"J{"req":3,"op":"git_log","cwd":"/r","limit":5}"#;
        let Frame::Control(env) = Frame::decode(json).unwrap() else {
            panic!("control frame")
        };
        assert_eq!(
            env.msg,
            Msg::GitLog {
                cwd: "/r".into(),
                all: false,
                skip: 0,
                limit: 5
            }
        );
        let reply = Msg::GitLogPage(GitLogPage {
            root: "/r".into(),
            head: None,
            commits: vec![],
            more: false,
        });
        let v = serde_json::to_value(Envelope::request(3, reply.clone())).unwrap();
        assert_eq!(v["op"], "git_log_page");
        let frame = Frame::Control(Envelope::request(3, reply));
        assert_eq!(roundtrip(frame.clone()), frame);
        // A daemon that predates the ops decodes them as `Unknown`.
        let future = br#"J{"req":4,"op":"git_commit_future","x":1}"#;
        let Frame::Control(env) = Frame::decode(future).unwrap() else {
            panic!("control frame")
        };
        assert_eq!(env.msg, Msg::Unknown);
    }

    #[test]
    fn session_event_unknown_kind_deserialises_to_unknown() {
        let json = r#"{"kind":"future_event","data":42}"#;
        let event: SessionEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event, SessionEvent::Unknown);
    }

    #[test]
    fn update_claude_wire_format() {
        let req: Msg = serde_json::from_str(r#"{"op":"update_claude"}"#).unwrap();
        assert_eq!(req, Msg::UpdateClaude { install: false });
        let req = Msg::UpdateClaude { install: true };
        assert_eq!(
            serde_json::to_string(&req).unwrap(),
            r#"{"op":"update_claude","install":true}"#
        );
        let reply = Msg::ClaudeUpdated {
            version: Some("2.1.296 (Claude Code)".into()),
            ok: true,
            tail: "done".into(),
        };
        let back: Msg = serde_json::from_str(&serde_json::to_string(&reply).unwrap()).unwrap();
        assert_eq!(back, reply);
    }
}
