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
    /// The session's most recent answers, for the reader view. Answered by
    /// `AnswersReply` (empty when the session has no conversation yet). An
    /// older daemon answers `Error "unsupported op…"`.
    Answers {
        id: SessionId,
    },
    /// The newest prompt of the session's conversation, answered or not,
    /// for saving as a snippet. Answered by `LastPromptReply`; an unknown
    /// session by `Error "no such session: …"`. An older daemon answers
    /// `Error "unsupported op…"`.
    LastPrompt {
        id: SessionId,
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
    /// A patch: one file of a commit, or the whole commit when neither `file`
    /// nor `path` is set. `file` is a position in the `files` of that commit's
    /// `GitCommitInfo`; the daemon resolves it to the exact name, which the
    /// display text in `GitFile` cannot carry. `path` and `old_path` (the
    /// pre-rename name) select by plain text and apply only without `file`.
    /// Answered by `GitPatch`.
    GitDiff {
        cwd: String,
        id: String,
        #[serde(default)]
        file: Option<u32>,
        #[serde(default)]
        path: Option<String>,
        #[serde(default)]
        old_path: Option<String>,
    },
    /// Search this host's claude conversations (prompts and answers) for
    /// every word of `query`; at most `limit` hits (0: the default, see
    /// [`crate::claude::search`]). Answered by `SearchReply`, or `Error
    /// "search busy"` while another search runs on this daemon, or `Error
    /// "bad query: …"`. An older daemon answers `Error "unsupported op…"`.
    Search {
        query: String,
        #[serde(default)]
        limit: u32,
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
    /// Reply to `Answers`: oldest first, built by [`answers_reply`].
    AnswersReply {
        id: SessionId,
        answers: Vec<Answer>,
    },
    /// Reply to `LastPrompt`: sanitized, at most 64 KiB (`cut` when it was
    /// longer); `None` when the conversation has no prompt yet.
    LastPromptReply {
        id: SessionId,
        prompt: Option<String>,
        #[serde(default)]
        cut: bool,
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
    /// Reply to `Search`, built by [`search_reply`]: one hit per
    /// conversation, newest first. `scanned` of `total` transcripts were
    /// read to their end; `truncated` when the search stopped early.
    SearchReply {
        query: String,
        hits: Vec<SearchHit>,
        scanned: u32,
        total: u32,
        truncated: bool,
    },
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
    /// SHA-256 of the binary the daemon was started from (what `__probe`
    /// reports as `build`). Old daemons omit it: they predate the freshness
    /// check, so they are outdated by definition ([`crate::freshness`]).
    #[serde(default)]
    pub build: Option<String>,
    /// That binary's modification time (Unix seconds), which orders two
    /// builds of one version.
    #[serde(default)]
    pub build_time: Option<u64>,
    /// What this daemon can do beyond the base protocol ([`DAEMON_FEATURES`]).
    /// Old daemons omit it: they have none of them.
    #[serde(default)]
    pub features: Vec<String>,
}

/// The features a daemon of this build reports in [`Welcome::features`]:
/// `lens` — it takes [`SpawnSpec::extensions`] and sends
/// [`SessionEvent::Lens`]; `search` — it answers [`Msg::Search`];
/// `last_prompt` — it answers [`Msg::LastPrompt`].
pub const DAEMON_FEATURES: &[&str] = &["lens", "search", "last_prompt"];

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
    /// Not the manager's: the session lives only as long as the client that
    /// started it (`claudio --plain`). The daemon does not journal it, and
    /// reports it with [`SessionInfo::ephemeral`] so a manager leaves it out.
    /// An old daemon ignores the field and treats it as any other session.
    #[serde(default)]
    pub ephemeral: bool,
    /// The session's extension lenses (the client's registry): what claude
    /// is told through the context hook and which tags the daemon extracts.
    /// `None` (an old client) keeps the journaled value. Ignored by an old
    /// daemon, which goes by its own `[claude] latex`.
    #[serde(default)]
    pub extensions: Option<SessionExtensions>,
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
    /// Replaces the journaled extensions when `Some` (see
    /// [`SpawnSpec::extensions`]).
    #[serde(default)]
    pub extensions: Option<SessionExtensions>,
}

/// The lenses of a claude session, in the client's registry order. The
/// daemon checks the caps ([`SessionExtensions::validate`]), writes the
/// instructions to the session's context file and extracts the tag lenses'
/// tags from the transcript.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionExtensions {
    #[serde(default)]
    pub lenses: Vec<LensSpec>,
}

/// One lens: its name (the tag, or the fence's info word), its kind, and
/// the instruction claude gets for it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LensSpec {
    pub name: String,
    pub kind: LensKind,
    #[serde(default)]
    pub instruction: String,
}

/// A tag lens (`<name>…</name>` in claude's text) or a fence lens (a fenced
/// code block whose info string starts with the name).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LensKind {
    Tag,
    Fence,
    /// A kind from a newer client: the daemon drops the lens.
    #[serde(other)]
    Unknown,
}

/// At most this many lenses per session.
pub const MAX_LENSES: usize = 16;
/// One instruction's cap, in bytes after `sanitize`.
pub const MAX_INSTRUCTION: usize = 2048;
/// All instructions together, in bytes after `sanitize`.
pub const MAX_INSTRUCTIONS: usize = 8192;

impl SessionExtensions {
    /// Why this value breaks the caps: more than [`MAX_LENSES`] lenses, a
    /// name that is not `^[a-z][a-z0-9-]{0,23}$` or is repeated, or
    /// instructions over [`MAX_INSTRUCTION`] / [`MAX_INSTRUCTIONS`].
    pub fn validate(&self) -> Result<(), String> {
        if self.lenses.len() > MAX_LENSES {
            return Err(format!("more than {MAX_LENSES} lenses"));
        }
        let mut total = 0;
        for (i, lens) in self.lenses.iter().enumerate() {
            if !lens_name_ok(&lens.name) {
                return Err(format!("bad lens name {:?}", lens.name));
            }
            if self.lenses[..i].iter().any(|l| l.name == lens.name) {
                return Err(format!("lens {} given twice", lens.name));
            }
            let len = crate::claude::latex::sanitize(&lens.instruction).len();
            if len > MAX_INSTRUCTION {
                return Err(format!("lens {} instruction over {MAX_INSTRUCTION} bytes", lens.name));
            }
            total += len;
        }
        if total > MAX_INSTRUCTIONS {
            return Err(format!("instructions over {MAX_INSTRUCTIONS} bytes"));
        }
        Ok(())
    }

    /// The names of the tag lenses: the tags the daemon extracts.
    pub fn tags(&self) -> Vec<String> {
        self.lenses
            .iter()
            .filter(|l| l.kind == LensKind::Tag)
            .map(|l| l.name.clone())
            .collect()
    }

    /// What claude is told: the non-empty instructions, sanitized, joined
    /// by a blank line.
    pub fn context(&self) -> String {
        let parts: Vec<String> = self
            .lenses
            .iter()
            .map(|l| crate::claude::latex::sanitize(&l.instruction).trim().to_owned())
            .filter(|i| !i.is_empty())
            .collect();
        parts.join("\n\n")
    }

    /// The lens names, for `Debug` and logs (never the instructions).
    pub fn names(&self) -> Vec<&str> {
        self.lenses.iter().map(|l| l.name.as_str()).collect()
    }
}

/// `^[a-z][a-z0-9-]{0,23}$`.
pub fn lens_name_ok(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= 24
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
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
            .field("extensions", &self.extensions.as_ref().map(|e| e.names()))
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
            .field("ephemeral", &self.ephemeral)
            .field("extensions", &self.extensions.as_ref().map(|e| e.names()))
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
    /// Started with [`SpawnSpec::ephemeral`]: not a tab for the manager.
    #[serde(default)]
    pub ephemeral: bool,
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
    /// The state's human name: "needs approval", "idle"…
    pub fn name(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Working => "working",
            Self::NeedsApproval => "needs approval",
            Self::NeedsInput => "needs input",
            Self::Idle => "idle",
            Self::Error => "error",
            Self::Exited => "exited",
            Self::Unknown => "unknown",
        }
    }

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
    /// The session no longer has a claude conversation to resume (it was
    /// restarted with a new one): forget any saved id. Old clients map this
    /// to `Unknown`.
    ClaudeSessionCleared,
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
    ///
    /// Every field is optional and `None` means "no news", never "cleared":
    /// a client keeps what it had. `git`, `output_tokens` and `turns` came
    /// later; an old daemon never sends them and an old client ignores them.
    Meta {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        branch: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context_tokens: Option<u64>,
        /// The working tree's state against its upstream.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        git: Option<GitStatus>,
        /// Output tokens the whole conversation has produced so far.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_tokens: Option<u64>,
        /// Prompts the user has sent in the conversation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turns: Option<u32>,
    },
    /// The conversation's recent `<latex>` formulas, oldest first: always
    /// the whole list, which replaces the client's. `seq` grows with every
    /// change for the session's whole life (a new conversation starts with
    /// an empty list under the next `seq`), so a client keeps the newer of
    /// an attach replay and a broadcast that cross. Old clients map this to
    /// `Unknown`.
    Latex {
        seq: u64,
        formulas: Vec<String>,
    },
    /// The conversation's recent tag-lens items (every tag lens of the
    /// session, `latex` included), oldest first, by `Latex`'s rules: the
    /// whole list, `seq` growing for the session's whole life. Sent beside
    /// `Latex` by a daemon with the `lens` feature; at most
    /// [`LENS_EVENT_MAX`] bytes encoded ([`lens_event`]). Old clients map
    /// this to `Unknown`.
    Lens {
        seq: u64,
        items: Vec<LensItem>,
    },
    /// Catch-all for event kinds this client doesn't recognise yet.
    /// Keeps older clients alive when the daemon sends a newer event kind.
    #[serde(other)]
    Unknown,
}

/// One tag-lens item: the tag (the lens name) and its text, sanitized.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LensItem {
    pub tag: String,
    pub text: String,
}

/// The most a `Lens` event takes encoded.
pub const LENS_EVENT_MAX: usize = 256 * 1024;

/// The `Lens` event for `items`, its oldest items dropped until it encodes
/// to at most [`LENS_EVENT_MAX`] bytes.
pub fn lens_event(seq: u64, items: Vec<LensItem>) -> SessionEvent {
    let mut event = SessionEvent::Lens { seq, items };
    while serde_json::to_vec(&event).map_or(0, |b| b.len()) > LENS_EVENT_MAX {
        let SessionEvent::Lens { items, .. } = &mut event else {
            break;
        };
        if items.is_empty() {
            break;
        }
        items.remove(0);
    }
    event
}

/// A working tree's state, from `git status --porcelain=v2 --branch`.
/// Counts are capped by the daemon; a zero everywhere is a clean tree.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitStatus {
    /// The branch name, or the abbreviated commit id when `detached`.
    #[serde(default)]
    pub head: String,
    #[serde(default)]
    pub detached: bool,
    /// Whether the branch tracks an upstream (then `ahead`/`behind` mean
    /// something; without one both are zero).
    #[serde(default)]
    pub upstream: bool,
    #[serde(default)]
    pub ahead: u32,
    #[serde(default)]
    pub behind: u32,
    /// Paths with changes in the index.
    #[serde(default)]
    pub staged: u32,
    /// Tracked paths with changes in the working tree.
    #[serde(default)]
    pub unstaged: u32,
    #[serde(default)]
    pub untracked: u32,
    /// Unmerged paths.
    #[serde(default)]
    pub conflicts: u32,
}

impl GitStatus {
    /// Whether anything is staged, modified, untracked or unmerged.
    pub fn is_dirty(&self) -> bool {
        self.staged + self.unstaged + self.untracked + self.conflicts > 0
    }

    /// The branch name when HEAD is on one.
    pub fn branch(&self) -> Option<&str> {
        (!self.detached && !self.head.is_empty()).then_some(self.head.as_str())
    }
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

/// The largest encoded `AnswersReply` the daemon sends: half of
/// [`MAX_FRAME`], so the reply and its envelope always fit a frame.
pub const ANSWERS_REPLY_MAX: usize = 512 * 1024;

/// One of claude's answers, for the reader view: the user's prompt and the
/// text claude answered it with (markdown, `<latex>` tags included). Both
/// are capped and sanitized by the daemon.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    pub prompt: String,
    pub text: String,
}

/// The `AnswersReply` for session `id`: `answers` (oldest first) without as
/// many of the oldest as it takes for its JSON to fit
/// [`ANSWERS_REPLY_MAX`]. JSON escapes every `\` and `"` of a formula, so
/// the cap is on the encoded size, not on the text's.
pub fn answers_reply(id: SessionId, answers: Vec<Answer>) -> Msg {
    let mut msg = Msg::AnswersReply { id, answers };
    while serde_json::to_vec(&msg).map_or(false, |json| json.len() > ANSWERS_REPLY_MAX) {
        match &mut msg {
            Msg::AnswersReply { answers, .. } if !answers.is_empty() => {
                answers.remove(0);
            }
            _ => break,
        }
    }
    msg
}

/// The largest encoded `SearchReply` the daemon sends (as for answers).
pub const SEARCH_REPLY_MAX: usize = 512 * 1024;

/// One conversation that matched a `Search`. Every text is sanitized by
/// the daemon; `title` is its first prompt and `snippet` the newest match,
/// each on one line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    /// The claude session id (the transcript's name), for `--resume`.
    pub conversation: String,
    pub cwd: String,
    /// Unix seconds of the transcript's last modification.
    pub modified: u64,
    pub title: String,
    pub snippet: String,
    /// How many messages matched.
    pub matches: u32,
}

/// The `SearchReply` for `query`, without as many of the last hits as it
/// takes for its JSON to fit [`SEARCH_REPLY_MAX`] (then `truncated`).
pub fn search_reply(
    query: String,
    hits: Vec<SearchHit>,
    scanned: u32,
    total: u32,
    truncated: bool,
) -> Msg {
    let mut msg = Msg::SearchReply {
        query,
        hits,
        scanned,
        total,
        truncated,
    };
    while serde_json::to_vec(&msg).map_or(false, |json| json.len() > SEARCH_REPLY_MAX) {
        match &mut msg {
            Msg::SearchReply {
                hits, truncated, ..
            } if !hits.is_empty() => {
                hits.pop();
                *truncated = true;
            }
            _ => break,
        }
    }
    msg
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
    fn welcome_from_an_old_daemon_has_no_build() {
        let old = r#"{"claudio_version":"0.2.0","proto":1,
            "host":{"hostname":"h","os":"linux","arch":"x86_64","home":"/h"}}"#;
        let w: Welcome = serde_json::from_str(old).unwrap();
        assert_eq!((w.build, w.build_time), (None, None));
        // And an old client still reads a new Welcome (unknown fields ignored).
        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct OldWelcome {
            claudio_version: String,
            proto: u32,
        }
        let new = Welcome {
            build: Some("ab".into()),
            build_time: Some(7),
            ..serde_json::from_str(old).unwrap()
        };
        let json = serde_json::to_string(&new).unwrap();
        assert!(serde_json::from_str::<OldWelcome>(&json).is_ok());
        assert_eq!(serde_json::from_str::<Welcome>(&json).unwrap(), new);
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
            extensions: None,
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
                extensions: None,
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
    fn ephemeral_is_absent_from_older_peers() {
        let id = "00000000-0000-0000-0000-000000000000";
        let spec = format!(r#"{{"id":"{id}","cwd":"/w","rows":24,"cols":80}}"#);
        assert!(!serde_json::from_str::<SpawnSpec>(&spec).unwrap().ephemeral);
        let info = format!(r#"{{"id":"{id}","cwd":"/w","state":"idle","created_at":1}}"#);
        assert!(!serde_json::from_str::<SessionInfo>(&info).unwrap().ephemeral);
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
            ephemeral: false,
            extensions: None,
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
    fn claude_session_cleared_is_unknown_to_older_clients() {
        #[derive(Debug, PartialEq, Deserialize)]
        #[serde(tag = "kind", rename_all = "snake_case")]
        enum Older {
            Removed,
            #[serde(other)]
            Unknown,
        }
        let json = serde_json::to_string(&SessionEvent::ClaudeSessionCleared).unwrap();
        assert_eq!(json, r#"{"kind":"claude_session_cleared"}"#);
        assert_eq!(serde_json::from_str::<Older>(&json).unwrap(), Older::Unknown);
        let back: SessionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, SessionEvent::ClaudeSessionCleared);
    }

    #[test]
    fn answers_round_trip_and_are_unknown_to_older_daemons() {
        #[derive(Debug, PartialEq, Deserialize)]
        #[serde(tag = "op", rename_all = "snake_case")]
        enum V04 {
            Ping,
            #[serde(other)]
            Unknown,
        }
        let ask = Msg::Answers { id: Uuid::nil() };
        let json = serde_json::to_string(&Envelope::request(5, ask.clone())).unwrap();
        assert!(json.contains(r#""op":"answers""#), "{json}");
        assert_eq!(serde_json::from_str::<V04>(&json).unwrap(), V04::Unknown);
        assert_eq!(serde_json::from_str::<Envelope>(&json).unwrap().msg, ask);
        let reply = Msg::AnswersReply {
            id: Uuid::nil(),
            answers: vec![Answer {
                prompt: "why?".into(),
                text: "Because <latex>x^2</latex>.".into(),
            }],
        };
        let frame = Frame::Control(Envelope::request(5, reply.clone()));
        assert_eq!(roundtrip(frame.clone()), frame);
        let json = serde_json::to_string(&reply).unwrap();
        assert!(json.contains(r#""op":"answers_reply""#), "{json}");
        assert_eq!(serde_json::from_str::<V04>(&json).unwrap(), V04::Unknown);
    }

    #[test]
    fn last_prompt_round_trips_and_is_unknown_to_older_daemons() {
        #[derive(Debug, PartialEq, Deserialize)]
        #[serde(tag = "op", rename_all = "snake_case")]
        enum V05 {
            Ping,
            #[serde(other)]
            Unknown,
        }
        let ask = Msg::LastPrompt { id: Uuid::nil() };
        let json = serde_json::to_string(&Envelope::request(4, ask.clone())).unwrap();
        assert!(json.contains(r#""op":"last_prompt""#), "{json}");
        assert_eq!(serde_json::from_str::<V05>(&json).unwrap(), V05::Unknown);
        assert_eq!(serde_json::from_str::<Envelope>(&json).unwrap().msg, ask);
        // The largest reply (64 KiB of quotes, escaped) fits a frame.
        let reply = Msg::LastPromptReply {
            id: Uuid::nil(),
            prompt: Some("\"".repeat(64 * 1024)),
            cut: true,
        };
        let frame = Frame::Control(Envelope::request(4, reply.clone()));
        assert!(frame.encode().len() - 4 <= MAX_FRAME);
        assert_eq!(roundtrip(frame.clone()), frame);
        let old: Msg = serde_json::from_str(&format!(
            r#"{{"op":"last_prompt_reply","id":"{}","prompt":null}}"#,
            Uuid::nil()
        ))
        .unwrap();
        assert_eq!(
            old,
            Msg::LastPromptReply {
                id: Uuid::nil(),
                prompt: None,
                cut: false
            }
        );
    }


    #[test]
    fn an_answers_reply_always_fits_a_frame() {
        // Ten answers of 64 KiB made only of backslashes: twice that once
        // encoded, 1.25 MiB in all.
        let answers: Vec<Answer> = (0..10)
            .map(|i| Answer {
                prompt: format!("p{i}"),
                text: "\\\\".repeat(32 * 1024),
            })
            .collect();
        let msg = answers_reply(Uuid::nil(), answers.clone());
        let json = serde_json::to_vec(&msg).unwrap();
        assert!(json.len() <= ANSWERS_REPLY_MAX, "{}", json.len());
        let frame = Frame::Control(Envelope::request(u64::MAX, msg.clone())).encode();
        assert!(frame.len() - 4 <= MAX_FRAME);
        let Msg::AnswersReply { answers: kept, .. } = msg else {
            panic!("{msg:?}")
        };
        // The oldest went; the newest stay, in order.
        assert_eq!(kept.len(), 3);
        assert_eq!(kept[..], answers[7..]);
        // A reply that fits is left alone.
        let small = answers_reply(Uuid::nil(), answers[..2].to_vec());
        assert_eq!(
            small,
            Msg::AnswersReply {
                id: Uuid::nil(),
                answers: answers[..2].to_vec(),
            }
        );
    }

    #[test]
    fn search_round_trips_and_is_unknown_to_older_daemons() {
        #[derive(Debug, PartialEq, Deserialize)]
        #[serde(tag = "op", rename_all = "snake_case")]
        enum V05 {
            Ping,
            #[serde(other)]
            Unknown,
        }
        let ask: Msg = serde_json::from_str(r#"{"op":"search","query":"toml"}"#).unwrap();
        assert_eq!(
            ask,
            Msg::Search {
                query: "toml".into(),
                limit: 0
            }
        );
        let json = serde_json::to_string(&Envelope::request(3, ask.clone())).unwrap();
        assert_eq!(serde_json::from_str::<V05>(&json).unwrap(), V05::Unknown);
        let reply = search_reply(
            "toml".into(),
            vec![SearchHit {
                conversation: "c1".into(),
                cwd: "/p".into(),
                modified: 7,
                title: "t".into(),
                snippet: "s".into(),
                matches: 2,
            }],
            1,
            3,
            false,
        );
        let frame = Frame::Control(Envelope::request(3, reply.clone()));
        assert_eq!(roundtrip(frame.clone()), frame);
        let json = serde_json::to_string(&reply).unwrap();
        assert!(json.contains(r#""op":"search_reply""#), "{json}");
        assert_eq!(serde_json::from_str::<V05>(&json).unwrap(), V05::Unknown);
    }

    #[test]
    fn a_search_reply_always_fits_a_frame() {
        // 100 hits with 8 KiB of quotes each: 1.6 MiB once encoded.
        let hits: Vec<SearchHit> = (0..100)
            .map(|i| SearchHit {
                conversation: format!("c{i}"),
                cwd: "\"".repeat(4096),
                modified: i,
                title: "\"".repeat(4096),
                snippet: String::new(),
                matches: 1,
            })
            .collect();
        let msg = search_reply("q".into(), hits.clone(), 100, 100, false);
        assert!(serde_json::to_vec(&msg).unwrap().len() <= SEARCH_REPLY_MAX);
        let Msg::SearchReply {
            hits: kept,
            truncated,
            ..
        } = msg
        else {
            panic!("{msg:?}")
        };
        // The newest stay, in order; the reply says it is cut.
        assert!(truncated);
        assert_eq!(kept[..], hits[..kept.len()]);
        assert_eq!(kept.len(), 31);
    }

    #[test]
    fn latex_round_trips_and_is_unknown_to_older_clients() {
        let event = SessionEvent::Latex {
            seq: 7,
            formulas: vec!["E = mc^2".into(), "a \\neq 0".into()],
        };
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(
            json,
            r#"{"kind":"latex","seq":7,"formulas":["E = mc^2","a \\neq 0"]}"#
        );
        assert_eq!(serde_json::from_str::<SessionEvent>(&json).unwrap(), event);
        #[derive(Debug, PartialEq, Deserialize)]
        #[serde(tag = "kind", rename_all = "snake_case")]
        enum Older {
            Removed,
            #[serde(other)]
            Unknown,
        }
        assert_eq!(serde_json::from_str::<Older>(&json).unwrap(), Older::Unknown);
    }

    #[test]
    fn meta_git_and_conversation_fields_are_optional_both_ways() {
        // An old daemon's Meta: only the three original fields.
        let old = r#"{"kind":"meta","branch":"main","model":"m","context_tokens":5}"#;
        let event: SessionEvent = serde_json::from_str(old).unwrap();
        assert_eq!(
            event,
            SessionEvent::Meta {
                branch: Some("main".into()),
                model: Some("m".into()),
                context_tokens: Some(5),
                git: None,
                output_tokens: None,
                turns: None,
            }
        );
        // A new daemon's Meta read by an old client: unknown fields ignored.
        #[derive(Debug, PartialEq, Deserialize)]
        #[serde(tag = "kind", rename_all = "snake_case")]
        enum Older {
            Meta {
                branch: Option<String>,
            },
            #[serde(other)]
            Unknown,
        }
        let new = SessionEvent::Meta {
            branch: Some("main".into()),
            model: None,
            context_tokens: None,
            git: Some(GitStatus {
                head: "main".into(),
                upstream: true,
                ahead: 2,
                behind: 1,
                unstaged: 3,
                ..GitStatus::default()
            }),
            output_tokens: Some(48_000),
            turns: Some(14),
        };
        let json = serde_json::to_string(&new).unwrap();
        assert!(!json.contains("model"), "absent fields are not sent: {json}");
        assert_eq!(
            serde_json::from_str::<Older>(&json).unwrap(),
            Older::Meta {
                branch: Some("main".into())
            }
        );
        assert_eq!(serde_json::from_str::<SessionEvent>(&json).unwrap(), new);
        // A GitStatus from a future daemon with fields we lack still parses.
        let git: GitStatus = serde_json::from_str(r#"{"head":"x","stashes":2}"#).unwrap();
        assert_eq!(git.branch(), Some("x"));
        assert!(!git.is_dirty());
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

    fn lens(name: &str, instruction: &str) -> LensSpec {
        LensSpec {
            name: name.into(),
            kind: LensKind::Tag,
            instruction: instruction.into(),
        }
    }

    #[test]
    fn extensions_are_optional_on_the_wire_and_unknown_kinds_survive() {
        // An old client's Spawn and Respawn: no extensions.
        let spawn = r#"{"id":"00000000-0000-0000-0000-000000000000","cwd":"/","rows":1,"cols":1}"#;
        assert_eq!(serde_json::from_str::<SpawnSpec>(spawn).unwrap().extensions, None);
        let respawn = r#"{"id":"00000000-0000-0000-0000-000000000000","rows":1,"cols":1}"#;
        assert_eq!(serde_json::from_str::<RespawnSpec>(respawn).unwrap().extensions, None);
        // A newer client's lens kind still parses (the daemon drops it).
        let ext = r#"{"lenses":[{"name":"x","kind":"widget","instruction":"i"},{"name":"keys","kind":"tag"}]}"#;
        let ext: SessionExtensions = serde_json::from_str(ext).unwrap();
        assert_eq!(ext.lenses[0].kind, LensKind::Unknown);
        assert_eq!(ext.lenses[1].kind, LensKind::Tag);
        assert_eq!(ext.lenses[1].instruction, "");
        // An old daemon's Welcome: no features.
        let welcome = serde_json::json!({
            "claudio_version": "0.5.0", "proto": 1,
            "host": {"hostname": "h", "os": "linux", "arch": "x86_64", "home": "/h"}
        });
        let welcome: Welcome = serde_json::from_value(welcome).unwrap();
        assert!(welcome.features.is_empty());
        assert_eq!(DAEMON_FEATURES, ["lens", "search", "last_prompt"]);
    }

    #[test]
    fn extension_caps() {
        let ok = |lenses: Vec<LensSpec>| SessionExtensions { lenses }.validate();
        assert_eq!(ok(vec![lens("latex", "x"), lens("a-1", "")]), Ok(()));
        assert!(ok((0..17).map(|i| lens(&format!("l{i}"), "")).collect()).is_err());
        assert!(ok(vec![lens("keys", ""), lens("keys", "")]).is_err());
        for bad in ["", "Keys", "1x", "a_b", "a b", &"a".repeat(25)] {
            assert!(ok(vec![lens(bad, "")]).is_err(), "{bad:?}");
        }
        assert_eq!(ok(vec![lens(&"a".repeat(24), "")]), Ok(()));
        // 2048 bytes after sanitize: controls do not count.
        let at_cap = format!("{}\u{7}", "i".repeat(2048));
        assert_eq!(ok(vec![lens("a", &at_cap)]), Ok(()));
        assert!(ok(vec![lens("a", &"i".repeat(2049))]).is_err());
        // 8192 in total.
        let full: Vec<LensSpec> = (0..4).map(|i| lens(&format!("l{i}"), &"i".repeat(2048))).collect();
        assert_eq!(ok(full.clone()), Ok(()));
        let mut over = full;
        over.push(lens("one", "i"));
        assert!(ok(over).is_err());
    }

    #[test]
    fn extension_context_and_debug_never_show_instructions() {
        let ext = SessionExtensions {
            lenses: vec![lens("latex", " Math.\u{1b} "), lens("abc", ""), lens("keys", "Keys.")],
        };
        assert_eq!(ext.context(), "Math.\n\nKeys.");
        let spec = SpawnSpec {
            id: Uuid::nil(),
            cwd: "/".into(),
            name: None,
            args: vec![],
            env: vec![],
            rows: 1,
            cols: 1,
            ephemeral: false,
            extensions: Some(ext.clone()),
        };
        let shown = format!("{spec:?}");
        assert!(shown.contains("\"latex\"") && !shown.contains("Math"), "{shown}");
        let respawn = RespawnSpec {
            id: Uuid::nil(),
            fresh: false,
            env: vec![],
            rows: 1,
            cols: 1,
            extensions: Some(ext),
        };
        assert!(!format!("{respawn:?}").contains("Keys."));
    }

    #[test]
    fn lens_events_stay_under_their_cap_dropping_the_oldest_items() {
        let item = |i: usize| LensItem {
            tag: "keys".into(),
            text: format!("{i}{}", "\\".repeat(2040)),
        };
        // 64 items of 2 KiB backslashes encode to about 260 KiB.
        let items: Vec<LensItem> = (0..64).map(item).collect();
        let event = lens_event(7, items);
        let encoded = serde_json::to_vec(&event).unwrap().len();
        assert!(encoded <= LENS_EVENT_MAX, "{encoded}");
        let SessionEvent::Lens { seq, items } = event else {
            panic!("not a lens event")
        };
        assert_eq!(seq, 7);
        assert!(items.len() < 64);
        assert!(items.last().unwrap().text.starts_with("63"));
        // A small list is untouched; an old client reads the event as Unknown.
        let small = lens_event(1, vec![item(0)]);
        let json = serde_json::to_string(&small).unwrap();
        assert!(json.contains("\"lens\""), "{json}");
        #[derive(serde::Deserialize)]
        #[serde(tag = "kind", rename_all = "snake_case")]
        enum OldEvent {
            Latex {},
            #[serde(other)]
            Unknown,
        }
        assert!(matches!(serde_json::from_str::<OldEvent>(&json), Ok(OldEvent::Unknown)));
    }
}
