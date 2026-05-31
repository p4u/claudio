//! Runtime configuration for the `--api` server, loaded from environment
//! variables.
//!
//! Every value has a sensible default so `claudio --api` runs with no
//! configuration at all. Each setting reads a `CLAUDIO_API_*` variable first,
//! then falls back to the original `OPENAI_PROXY_*` name for drop-in
//! compatibility with the standalone proxy.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Semaphore;

/// Immutable server configuration resolved once at startup.
#[derive(Debug, Clone)]
pub struct Config {
    /// Address to bind the HTTP server to.
    pub bind: SocketAddr,
    /// If set, callers must send `Authorization: Bearer <key>` matching this.
    pub api_key: Option<String>,
    /// Path/name of the Claude CLI binary the PTY backend drives.
    pub claude_bin: String,
    /// Model alias used when the request's model is missing or non-Claude.
    pub default_model: String,
    /// Working directory the backend subprocess runs in (kept clean of
    /// CLAUDE.md). Set as the process cwd at startup so every PTY child uses it.
    pub cwd: PathBuf,
    /// Maximum number of concurrent backend turns.
    pub max_concurrency: usize,
    /// Per-request timeout for a backend turn, in seconds.
    pub timeout_secs: u64,
    /// Enable OpenAI tool-calling passthrough (prompt-based). Default on; set
    /// `*_AGENTIC=false` to ignore `tools` (chat-only behavior). Safe to default
    /// on: the proxy never executes tools — the client does.
    pub agentic: bool,
    /// Value for `--setting-sources` (e.g. "project"). Empty string ⇒ flag
    /// omitted (CLI loads its defaults: user+project+local). Defaulting to
    /// "project" sheds user-level plugins/hooks/memory — far less input overhead
    /// per call — while staying OAuth-compatible.
    pub setting_sources: String,
}

impl Config {
    /// Build configuration from the process environment, applying defaults.
    pub fn from_env() -> Self {
        let bind = env_str(&["CLAUDIO_API_BIND", "OPENAI_PROXY_BIND"])
            .unwrap_or_else(|| "127.0.0.1:8080".to_string())
            .parse()
            .expect("CLAUDIO_API_BIND must be a valid socket address");

        let cwd = env_str(&["CLAUDIO_API_CWD", "OPENAI_PROXY_CWD"])
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);

        Config {
            bind,
            api_key: env_str(&["CLAUDIO_API_KEY", "OPENAI_PROXY_API_KEY"]),
            claude_bin: env_str(&["CLAUDIO_CLAUDE_PATH", "OPENAI_PROXY_CLAUDE_BIN"])
                .unwrap_or_else(|| "claude".to_string()),
            default_model: env_str(&["CLAUDIO_API_DEFAULT_MODEL", "OPENAI_PROXY_DEFAULT_MODEL"])
                .unwrap_or_else(|| "sonnet".to_string()),
            cwd,
            max_concurrency: env_parse(&["CLAUDIO_API_MAX_CONCURRENCY", "OPENAI_PROXY_MAX_CONCURRENCY"])
                .unwrap_or(8),
            timeout_secs: env_parse(&["CLAUDIO_API_TIMEOUT_SECS", "OPENAI_PROXY_TIMEOUT_SECS"])
                .unwrap_or(600),
            agentic: env_parse::<bool>(&["CLAUDIO_API_AGENTIC", "OPENAI_PROXY_AGENTIC"])
                .unwrap_or(true),
            // Read raw so an explicit empty value means "omit the flag" (load CLI
            // defaults), distinct from unset (lean "project" default).
            setting_sources: env_raw(&["CLAUDIO_API_SETTING_SOURCES", "OPENAI_PROXY_SETTING_SOURCES"])
                .unwrap_or_else(|| "project".to_string()),
        }
    }
}

/// Shared application state handed to every request handler.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    /// Caps the number of in-flight backend turns.
    pub permits: Arc<Semaphore>,
    /// Pool of persistent claude sessions (the `pty` backend).
    pub pool: Arc<super::pool::SessionPool>,
}

impl AppState {
    pub fn new(config: Config) -> Self {
        let permits = Arc::new(Semaphore::new(config.max_concurrency));
        let pool = Arc::new(super::pool::SessionPool::new(&config.setting_sources));
        AppState {
            config: Arc::new(config),
            permits,
            pool,
        }
    }
}

/// First non-empty value among `keys`, in order.
fn env_str(keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|k| std::env::var(k).ok().filter(|s| !s.is_empty()))
}

/// First *set* value among `keys` (may be empty), in order. Used where an empty
/// string is semantically meaningful (distinct from unset).
fn env_raw(keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| std::env::var(k).ok())
}

fn env_parse<T: std::str::FromStr>(keys: &[&str]) -> Option<T> {
    env_str(keys).and_then(|s| s.parse().ok())
}
