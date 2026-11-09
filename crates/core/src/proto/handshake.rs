//! The handshake: the client's [`Hello`] and the daemon's [`Welcome`], with
//! the host facts it carries. Re-exported from [`crate::proto`].

use serde::{Deserialize, Serialize};

/// The client's first message: its version, protocol and terminal colors.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hello {
    pub claudio_version: String,
    pub proto: u32,
    /// The client's real terminal colors, so the daemon can answer the
    /// child's OSC 10/11 queries truthfully.
    #[serde(default)]
    pub colors: Option<Colors>,
}

/// The daemon's reply to [`Hello`]: its version and protocol, the host's
/// facts, and its binary's build and features (what [`crate::freshness`]
/// compares to decide whether to replace it).
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
/// `last_prompt` — it answers [`Msg::LastPrompt`]; `attention` — it sends
/// [`SessionEvent::Attention`] and fills [`SessionInfo::attention`];
/// `worktree` — it answers [`Msg::GitWorktree`] and [`Msg::GitWorktreeDiff`].
///
/// [`SpawnSpec::extensions`]: super::SpawnSpec::extensions
/// [`SessionEvent::Lens`]: super::SessionEvent::Lens
/// [`SessionEvent::Attention`]: super::SessionEvent::Attention
/// [`SessionInfo::attention`]: super::SessionInfo::attention
/// [`Msg::Search`]: super::Msg::Search
/// [`Msg::LastPrompt`]: super::Msg::LastPrompt
/// [`Msg::GitWorktree`]: super::Msg::GitWorktree
/// [`Msg::GitWorktreeDiff`]: super::Msg::GitWorktreeDiff
pub const DAEMON_FEATURES: &[&str] = &["lens", "search", "last_prompt", "attention", "worktree"];

/// A terminal's foreground and background as RGB.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Colors {
    pub fg: [u8; 3],
    pub bg: [u8; 3],
}

/// The facts a daemon reports about its host in [`Welcome`].
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

/// The claude a daemon would launch: its resolved path and `--version`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClaudeInfo {
    pub path: String,
    pub version: String,
}
