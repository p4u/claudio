//! The spawn specs: how the daemon starts and restarts a session, and the
//! session's extensions (lenses and instructions) with their caps.
//! Re-exported from [`crate::proto`].

use serde::{Deserialize, Serialize};

use super::SessionId;

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

/// The lenses of a claude session, in the client's registry order, and
/// the proxy plugins' instructions. The daemon checks the caps
/// ([`SessionExtensions::validate`]), writes the instructions to the
/// session's context file and extracts the tag lenses' tags from the
/// transcript.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionExtensions {
    #[serde(default)]
    pub lenses: Vec<LensSpec>,
    /// The enabled proxy plugins' instructions, joined by a blank line:
    /// at most [`MAX_PLUGIN_INSTRUCTIONS`] bytes after `sanitize`, counted
    /// apart from the lenses. An old daemon ignores it.
    #[serde(default)]
    pub instructions: String,
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
/// The plugins' instructions, in bytes after `sanitize`. With the lenses'
/// and the blank lines `context()` puts between the parts, they always fit
/// the context hook's 16 KiB read.
pub const MAX_PLUGIN_INSTRUCTIONS: usize = 8192 - 2 * (MAX_LENSES + 1);

impl SessionExtensions {
    /// Why this value breaks the caps: more than [`MAX_LENSES`] lenses, a
    /// name that is not `^[a-z][a-z0-9-]{0,23}$` or is repeated, or
    /// instructions over [`MAX_INSTRUCTION`] / [`MAX_INSTRUCTIONS`], or
    /// plugin instructions over [`MAX_PLUGIN_INSTRUCTIONS`].
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
            let len = crate::text::sanitize(&lens.instruction).len();
            if len > MAX_INSTRUCTION {
                return Err(format!(
                    "lens {} instruction over {MAX_INSTRUCTION} bytes",
                    lens.name
                ));
            }
            total += len;
        }
        if total > MAX_INSTRUCTIONS {
            return Err(format!("instructions over {MAX_INSTRUCTIONS} bytes"));
        }
        if crate::text::sanitize(&self.instructions).len() > MAX_PLUGIN_INSTRUCTIONS {
            return Err(format!(
                "plugin instructions over {MAX_PLUGIN_INSTRUCTIONS} bytes"
            ));
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
    /// by a blank line; the plugins' after the lenses'.
    pub fn context(&self) -> String {
        let parts: Vec<String> = self
            .lenses
            .iter()
            .map(|l| l.instruction.as_str())
            .chain([self.instructions.as_str()])
            .map(|i| crate::text::sanitize(i).trim().to_owned())
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
