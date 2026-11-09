//! `Manifest`: apply a proxy plugin's manifest on this host.
//!
//! The manifest (`crate::plugins::manifest`) is downloaded once per hash
//! into `<cache>/plugins` (0700) as `<name>.toml` with its sidecar
//! `<name>.sha256`: https only, capped at 256 KiB and [`TOTAL_TIMEOUT`],
//! streamed to a temp file, verified, then renamed into place; one download
//! per plugin name at a time.
//!
//! The reply ([`Applied`]) goes as soon as the manifest is parsed and its
//! static parts are rendered: the MCP servers and agents every session
//! gets, the instructions and the env, and each item's outcome. Plugins
//! that are neither installed nor remembered are `installing`: their
//! installs run after the reply, in one task per manifest hash, each
//! `claude plugin …` subprocess under `Daemon::updating` (the lock `claude
//! update` takes), so `~/.claude` is written by one at a time. The daemon
//! itself never writes there: it reads Claude Code's files (capped, regular
//! files of this uid, never logged) and runs the CLI, never with `-y`.
//!
//! What the installs did is remembered per (manifest hash, id): installed
//! by this daemon (for its life), needing a manual install (for its life:
//! a person has to act), or failed: tried again once its backoff is over
//! ([`RETRY_FIRST`], doubling with each failure in a row up to
//! [`RETRY_MAX`]; a success starts it over). Meanwhile its reason says
//! when (`proto::retrying`).

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

use super::Daemon;
use crate::claude::SESSION_MARKERS;
use crate::paths;
use crate::plugins::manifest::{self, Manifest, Marketplace, PluginRef};
use crate::proto::{retrying, Applied, Msg};
use crate::proxy::plugins::{key_ok, name_ok, sha256_ok, test_http_ok};
use crate::text::sanitize;

/// A manifest's size cap.
const MAX_BYTES: u64 = manifest::MAX_BYTES as u64;
/// At most this many manifests are kept.
pub const MAX_MANIFESTS: usize = 32;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(120);
/// Claude Code's files are read up to this size.
const MAX_CONFIG: u64 = 16 << 20;
/// A request's caps: secret names and disabled ids.
const MAX_SECRETS: usize = 64;
const MAX_DISABLED: usize = 256;
/// A failed install is tried again after this long, the wait doubling with
/// each failure in a row up to [`RETRY_MAX`].
const RETRY_FIRST: Duration = Duration::from_secs(15);
const RETRY_MAX: Duration = Duration::from_secs(300);
/// One `claude plugin …` run's deadline.
const RUN_TIMEOUT: Duration = Duration::from_secs(120);
/// Each output stream of a run is kept up to this size.
const OUTPUT_CAP: usize = 64 << 10;
/// A reason from the CLI is cut to this many characters.
const REASON_CHARS: usize = 200;

/// The reason of a plugin whose install is queued or running.
pub const INSTALLING: &str = "installing";

/// Where manifests go, where Claude Code keeps its files, the per-name
/// download locks, and what the installs did.
pub(crate) struct Manifests {
    dirs: Mutex<Dirs>,
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    memory: Mutex<Memory>,
    /// Added to the clock (tests move time forward).
    skew: Mutex<Duration>,
}

#[derive(Clone)]
struct Dirs {
    /// `<cache>/plugins`.
    cache: PathBuf,
    /// `$CLAUDE_CONFIG_DIR`, else `~/.claude`.
    claude_config: PathBuf,
    /// Tests serve manifests over plain http on loopback.
    allow_http: bool,
}

/// What the installs did, by (manifest hash, key); a key is a plugin id,
/// or `marketplace:<name>` for a marketplace that could not be added.
#[derive(Default)]
struct Memory {
    installed: HashSet<(String, String)>,
    manual: HashMap<(String, String), String>,
    failed: HashMap<(String, String), Failure>,
    /// Plugin installs queued or running.
    busy: HashSet<(String, String)>,
    /// Per manifest hash, the installs left; present while its task runs.
    queues: HashMap<String, VecDeque<Job>>,
}

/// A failed install: why, when, and how many failures in a row.
struct Failure {
    why: String,
    at: Instant,
    count: u32,
}

/// The wait after the `count`-th failure in a row.
fn retry_after(count: u32) -> Duration {
    let doublings = count.saturating_sub(1).min(8);
    (RETRY_FIRST * (1 << doublings)).min(RETRY_MAX)
}

/// `left` in whole seconds, rounded up.
fn ceil_secs(left: Duration) -> u64 {
    left.as_secs() + u64::from(left.subsec_nanos() > 0)
}

impl Memory {
    /// The failure of `key` under `sha` and the time left until it is tried
    /// again, while its backoff is not over.
    fn failure(&self, sha: &str, key: &str, now: Instant) -> Option<(String, Duration)> {
        let f = self.failed.get(&(sha.to_owned(), key.to_owned()))?;
        let left = retry_after(f.count).checked_sub(now.saturating_duration_since(f.at))?;
        (!left.is_zero()).then(|| (f.why.clone(), left))
    }

    /// `key` failed (again) at `now`, for `why`.
    fn fail(&mut self, key: (String, String), why: String, now: Instant) {
        let count = self.failed.get(&key).map_or(0, |f| f.count) + 1;
        self.failed.insert(
            key,
            Failure {
                why,
                at: now,
                count,
            },
        );
    }
}

/// One plugin to install, with its marketplace.
#[derive(Clone)]
struct Job {
    plugin: PluginRef,
    marketplace: Marketplace,
}

/// How one `claude plugin …` run ended.
#[derive(Debug, PartialEq)]
enum Outcome {
    Done,
    /// A marketplace-declared command needs a person.
    Manual,
    Failed(String),
}

impl Manifests {
    /// The standard places: [`paths::cache_dir`] and the Claude Code config
    /// dir the sessions see (this daemon's `CLAUDE_CONFIG_DIR`, which every
    /// session inherits).
    pub(crate) fn new() -> Manifests {
        let claude_config = std::env::var_os("CLAUDE_CONFIG_DIR")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| paths::home().join(".claude"));
        Manifests {
            dirs: Mutex::new(Dirs {
                cache: paths::cache_dir().join("plugins"),
                claude_config,
                allow_http: false,
            }),
            locks: Mutex::new(HashMap::new()),
            memory: Mutex::new(Memory::default()),
            skew: Mutex::new(Duration::ZERO),
        }
    }

    fn dirs(&self) -> Dirs {
        self.dirs.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn memory(&self) -> MutexGuard<'_, Memory> {
        self.memory.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn now(&self) -> Instant {
        Instant::now() + *self.skew.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Use `cache` and `claude_config` instead, and accept http URLs.
    #[cfg(test)]
    pub(crate) fn set_dirs(&self, cache: PathBuf, claude_config: PathBuf) {
        *self.dirs.lock().unwrap_or_else(|e| e.into_inner()) = Dirs {
            cache,
            claude_config,
            allow_http: true,
        };
    }

    /// Whether an install is queued or running.
    #[cfg(test)]
    pub(crate) fn busy(&self) -> bool {
        !self.memory().queues.is_empty()
    }

    /// Forget every failed install, so the next request tries each at once:
    /// claude was just installed on this host. Manual installs stay.
    pub(crate) fn forget_failures(&self) {
        self.memory().failed.clear();
    }

    /// Move this daemon's clock forward by `by`.
    #[cfg(test)]
    pub(crate) fn advance(&self, by: Duration) {
        *self.skew.lock().unwrap_or_else(|e| e.into_inner()) += by;
    }

    fn lock(&self, name: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.locks.lock().unwrap_or_else(|e| e.into_inner());
        Arc::clone(locks.entry(name.to_owned()).or_default())
    }

    /// The reply for `m` (hash `sha`) and the installs it needs, honouring
    /// `disabled` (neither applied nor listed) and the request's `secrets`.
    fn decide(
        &self,
        m: &Manifest,
        sha: &str,
        secrets: &[String],
        disabled: &[String],
        files: &Files,
    ) -> (Applied, Vec<Job>) {
        let on = |id: &str| !disabled.iter().any(|d| d == id);
        let now = self.now();
        let mut mem = self.memory();
        let mut applied = Applied {
            name: m.name.clone(),
            version: m.version.clone(),
            ..Applied::default()
        };
        let mut jobs = Vec::new();
        for p in m.plugins.iter().filter(|p| on(&p.id())) {
            let id = p.id();
            let key = (sha.to_owned(), id.clone());
            if files.installed.contains(&p.spec()) {
                if mem.installed.contains(&key) {
                    applied.installed.push(id);
                } else {
                    applied.present.push(id);
                }
                continue;
            }
            if let Some(why) = mem.manual.get(&key) {
                applied.skipped.push((id, why.clone()));
                continue;
            }
            if let Some((why, left)) = mem.failure(sha, &id, now) {
                applied.skipped.push((id, retrying(&why, ceil_secs(left))));
                continue;
            }
            applied.skipped.push((id, INSTALLING.to_owned()));
            if mem.busy.insert(key) {
                if let Some(marketplace) = m.marketplace(&p.marketplace) {
                    jobs.push(Job {
                        plugin: p.clone(),
                        marketplace: marketplace.clone(),
                    });
                }
            }
        }
        let mut servers = Vec::new();
        for s in m.servers.iter().filter(|s| on(&s.id())) {
            let reason = match &files.servers {
                None => Some("cannot read .claude.json".to_owned()),
                Some(theirs) if theirs.contains(&s.name) => Some("yours".to_owned()),
                Some(_) => s
                    .secrets
                    .iter()
                    .find(|k| !secrets.contains(k))
                    .map(|k| format!("missing secret {k}")),
            };
            match reason {
                Some(why) => applied.skipped.push((s.id(), why)),
                None => servers.push(s),
            }
        }
        applied.mcp_config = manifest::mcp_json(servers);
        let mut agents = Vec::new();
        for a in m.agents.iter().filter(|a| on(&a.id())) {
            if files.agents.contains(&a.name) {
                applied.skipped.push((a.id(), "yours".to_owned()));
            } else {
                agents.push(a);
            }
        }
        applied.agents = manifest::agents_json(agents);
        if on("instructions") {
            applied.instructions = sanitize(&m.instructions).trim().to_owned();
        }
        if on("env") {
            applied.env = m.env.clone();
        }
        (applied, jobs)
    }
}

/// Answer a `Manifest` request: `Applied`, or `Error "manifest <name>: …"`.
/// The installs it needs are queued after the reply is made.
pub(crate) async fn handle(
    daemon: &Arc<Daemon>,
    name: String,
    url: String,
    sha256: String,
    secrets: Vec<String>,
    disabled: Vec<String>,
) -> Msg {
    let manifests = &daemon.manifests;
    let dirs = manifests.dirs();
    let scheme_ok = url.starts_with("https://")
        || (dirs.allow_http && url.starts_with("http://"))
        || test_http_ok(&url);
    let request_ok = name_ok(&name)
        && sha256_ok(&sha256)
        && scheme_ok
        && secrets.len() <= MAX_SECRETS
        && secrets.iter().all(|s| key_ok(s).is_ok())
        && disabled.len() <= MAX_DISABLED
        && disabled.iter().all(|id| manifest::id_ok(id));
    if !request_ok {
        return Msg::Error {
            message: format!("manifest {}: bad request", safe_name(&name)),
        };
    }
    let text = {
        // One download per name at a time; whoever waited finds it cached.
        let lock = manifests.lock(&name);
        let _held = lock.lock().await;
        ready(&dirs, &name, &url, &sha256).await
    };
    let parsed = text.and_then(|text| Manifest::parse_named(&text, &name));
    let m = match parsed {
        Ok(m) => m,
        Err(why) => {
            tracing::warn!(manifest = %name, error = %why, "manifest refused");
            return Msg::Error {
                message: format!("manifest {name}: {why}"),
            };
        }
    };
    let (files, flags) = {
        let config = dirs.claude_config.clone();
        let agents: Vec<String> = m.agents.iter().map(|a| a.name.clone()).collect();
        let d = Arc::clone(daemon);
        let read = tokio::task::spawn_blocking(move || Files::read(&config, &agents));
        let probe = tokio::task::spawn_blocking(move || d.claude_flags());
        let (files, flags) = tokio::join!(read, probe);
        (files.unwrap_or_default(), flags.unwrap_or_default())
    };
    let (mut applied, jobs) = manifests.decide(&m, &sha256, &secrets, &disabled, &files);
    applied.dropped = flags.dropped();
    tracing::info!(
        manifest = %name,
        version = %m.version,
        installed = applied.installed.len(),
        present = applied.present.len(),
        skipped = applied.skipped.len(),
        queued = jobs.len(),
        "manifest applied"
    );
    enqueue(daemon, &sha256, jobs);
    Msg::Applied(applied)
}

/// A name for an error about a refused request: shown only when valid.
fn safe_name(name: &str) -> &str {
    if name_ok(name) {
        name
    } else {
        "?"
    }
}

/// Remove the zips (and their sidecars) a pre-manifest build left in
/// `dir`, the plugin cache. Run once at daemon start.
pub(crate) fn remove_stale_zips(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let file = entry.file_name().to_string_lossy().into_owned();
        let Some(stem) = file.strip_suffix(".zip") else {
            continue;
        };
        let _ = std::fs::remove_file(entry.path());
        if !dir.join(format!("{stem}.toml")).exists() {
            let _ = std::fs::remove_file(dir.join(format!("{stem}.sha256")));
        }
    }
}

// ── Claude Code's files ───────────────────────────────────────────────────────

/// What Claude Code's files say, read once per request.
#[derive(Default)]
struct Files {
    /// The keys of `plugins/installed_plugins.json` (any scope); none when
    /// it is unreadable.
    installed: HashSet<String>,
    /// The `mcpServers` of `.claude.json`; `None` when it is unreadable.
    servers: Option<HashSet<String>>,
    /// The manifest's agents the member has their own `agents/<name>.md` of.
    agents: HashSet<String>,
}

impl Files {
    /// Blocking: reads under `config` (the Claude Code config dir).
    fn read(config: &Path, agents: &[String]) -> Files {
        let installed = installed_plugins(config);
        // No file at all: the member has no servers of their own.
        let servers = match read_json(&config.join(".claude.json")) {
            Ok(Some(v)) => Some(keys(&v, "mcpServers")),
            Ok(None) => Some(HashSet::new()),
            Err(()) => None,
        };
        let agents = agents
            .iter()
            .filter(|a| std::fs::symlink_metadata(config.join(format!("agents/{a}.md"))).is_ok())
            .cloned()
            .collect();
        Files {
            installed,
            servers,
            agents,
        }
    }
}

/// The keys of object `field` of `v`.
fn keys(v: &Value, field: &str) -> HashSet<String> {
    v.get(field)
        .and_then(Value::as_object)
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default()
}

/// The plugins Claude Code lists as installed (any scope): the keys of
/// `plugins` in `plugins/installed_plugins.json` (none when it is
/// unreadable).
fn installed_plugins(config: &Path) -> HashSet<String> {
    read_json(&config.join("plugins/installed_plugins.json"))
        .ok()
        .flatten()
        .map(|v| keys(&v, "plugins"))
        .unwrap_or_default()
}

/// The marketplaces Claude Code knows: the keys of
/// `plugins/known_marketplaces.json` (none when it is unreadable).
fn known_marketplaces(config: &Path) -> HashSet<String> {
    let v = read_json(&config.join("plugins/known_marketplaces.json"));
    v.ok()
        .flatten()
        .and_then(|v| v.as_object().map(|o| o.keys().cloned().collect()))
        .unwrap_or_default()
}

/// The JSON in `path`: `Ok(None)` when there is no such file, `Err` when
/// it is a symlink, not a regular file of this uid, over [`MAX_CONFIG`],
/// unreadable or not JSON. Its content is never logged.
fn read_json(path: &Path) -> Result<Option<Value>, ()> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(()),
    };
    if !meta.file_type().is_file() || meta.uid() != paths::uid() || meta.len() > MAX_CONFIG {
        return Err(());
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| ())?;
    let mut raw = Vec::new();
    file.take(MAX_CONFIG + 1)
        .read_to_end(&mut raw)
        .map_err(|_| ())?;
    if raw.len() as u64 > MAX_CONFIG {
        return Err(());
    }
    serde_json::from_slice(&raw).map(Some).map_err(|_| ())
}

// ── Installs ──────────────────────────────────────────────────────────────────

/// Queue `jobs` for manifest hash `sha`, starting its task unless it runs.
fn enqueue(daemon: &Arc<Daemon>, sha: &str, jobs: Vec<Job>) {
    if jobs.is_empty() {
        return;
    }
    let start = {
        let mut mem = daemon.manifests.memory();
        let start = !mem.queues.contains_key(sha);
        mem.queues.entry(sha.to_owned()).or_default().extend(jobs);
        start
    };
    if start {
        let daemon = Arc::clone(daemon);
        let sha = sha.to_owned();
        tokio::spawn(async move { run_queue(&daemon, &sha).await });
    }
}

/// Run the installs of manifest hash `sha` one by one until none is left.
async fn run_queue(daemon: &Daemon, sha: &str) {
    let manifests = &daemon.manifests;
    loop {
        let job = {
            let mut mem = manifests.memory();
            match mem.queues.get_mut(sha).and_then(VecDeque::pop_front) {
                Some(job) => job,
                None => {
                    mem.queues.remove(sha);
                    return;
                }
            }
        };
        let id = job.plugin.id();
        let outcome = match install(daemon, sha, &job).await {
            // Claude Code said yes but does not list it: a failure, not run
            // again on every re-ask.
            Outcome::Done
                if !installed_plugins(&manifests.dirs().claude_config)
                    .contains(&job.plugin.spec()) =>
            {
                Outcome::Failed("installed but not listed".to_owned())
            }
            other => other,
        };
        tracing::info!(plugin = %id, outcome = ?outcome, "plugin install");
        let now = manifests.now();
        let key = (sha.to_owned(), id);
        let mut mem = manifests.memory();
        mem.busy.remove(&key);
        match outcome {
            Outcome::Done => {
                mem.failed.remove(&key);
                mem.installed.insert(key);
            }
            Outcome::Manual => {
                let why = format!(
                    "needs manual install: claude plugin install {}",
                    job.plugin.spec()
                );
                mem.failed.remove(&key);
                mem.manual.insert(key, why);
            }
            Outcome::Failed(why) => mem.fail(key, why, now),
        }
    }
}

/// Install `job`'s plugin, adding its marketplace first when Claude Code
/// does not know it.
async fn install(daemon: &Daemon, sha: &str, job: &Job) -> Outcome {
    let manifests = &daemon.manifests;
    let config = manifests.dirs().claude_config;
    let market = &job.marketplace.name;
    let market_key = format!("marketplace:{market}");
    let remembered = manifests
        .memory()
        .failure(sha, &market_key, manifests.now());
    if let Some((why, _)) = remembered {
        return Outcome::Failed(why);
    }
    if !known_marketplaces(&config).contains(market) {
        let source = job.marketplace.cli_source();
        let added = run_claude(
            daemon,
            &[
                "plugin",
                "marketplace",
                "add",
                source,
                "--scope",
                "user",
                "--json",
            ],
        )
        .await;
        let known = known_marketplaces(&config).contains(market);
        let failed = match added {
            // Known now, even after a failed add: another manifest's
            // install added it meanwhile.
            _ if known => None,
            Outcome::Done => Some("added under another name".to_owned()),
            Outcome::Manual => Some("needs a person to add it".to_owned()),
            Outcome::Failed(why) => Some(why),
        };
        if let Some(why) = failed {
            let why = format!("marketplace {market}: {why}");
            let key = (sha.to_owned(), market_key);
            let now = manifests.now();
            manifests.memory().fail(key, why.clone(), now);
            return Outcome::Failed(why);
        }
    }
    // Known: a failure to add it is over.
    manifests
        .memory()
        .failed
        .remove(&(sha.to_owned(), market_key));
    let spec = job.plugin.spec();
    run_claude(
        daemon,
        &["plugin", "install", &spec, "--scope", "user", "--json"],
    )
    .await
}

/// Run `claude <args>` while holding `Daemon::updating`: the daemon's
/// claude on its `PATH`, stdin `/dev/null`, the session markers scrubbed,
/// git never prompting, in a process group of its own killed on timeout.
/// Without a claude it fails at once, never holding the lock the install
/// of claude takes.
async fn run_claude(daemon: &Daemon, args: &[&str]) -> Outcome {
    let claude = daemon.config.claude_bin();
    if !claude.is_file() {
        return Outcome::Failed("claude is not installed".to_owned());
    }
    let _held = daemon.updating.lock().await;
    let mut cmd = Command::new(claude);
    cmd.args(args);
    for marker in SESSION_MARKERS {
        cmd.env_remove(marker);
    }
    if let Some(path) = daemon.config.path.as_deref() {
        cmd.env("PATH", path);
    }
    cmd.env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "/usr/bin/true");
    let spawned = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true)
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(e) => return Outcome::Failed(format!("could not start claude: {e}")),
    };
    let (out, err) = (child.stdout.take(), child.stderr.take());
    let finished = tokio::time::timeout(RUN_TIMEOUT, async {
        let (out, err) = tokio::join!(read_capped(out), read_capped(err));
        (child.wait().await, out, err)
    })
    .await;
    match finished {
        Ok((Ok(status), out, err)) => outcome(status.success(), status.code(), &out, &err),
        Ok((Err(e), _, _)) => Outcome::Failed(format!("claude failed: {e}")),
        Err(_) => {
            super::update::kill_group(&mut child).await;
            Outcome::Failed(format!("timed out after {} s", RUN_TIMEOUT.as_secs()))
        }
    }
}

/// Everything `stream` gives, keeping the first [`OUTPUT_CAP`] bytes.
async fn read_capped(stream: Option<impl AsyncRead + Unpin>) -> Vec<u8> {
    let mut kept = Vec::new();
    let Some(mut stream) = stream else {
        return kept;
    };
    let mut buf = [0u8; 4096];
    loop {
        match stream.read(&mut buf).await {
            Ok(0) | Err(_) => return kept,
            Ok(n) => {
                let room = OUTPUT_CAP.saturating_sub(kept.len());
                kept.extend_from_slice(&buf[..n.min(room)]);
            }
        }
    }
}

/// What a `--json` run said: its last stdout line as JSON. A
/// `shownCommand` needs a person; an `error`, a non-zero exit or no JSON
/// is a failure with the CLI's message (else stderr's first line).
fn outcome(success: bool, code: Option<i32>, stdout: &[u8], stderr: &[u8]) -> Outcome {
    let stdout = String::from_utf8_lossy(stdout);
    let json: Option<Value> = stdout
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .and_then(|l| serde_json::from_str(l.trim()).ok());
    let field = |name: &str| {
        let v = json.as_ref()?.get(name)?;
        match v {
            Value::Null => None,
            Value::String(s) => Some(s.clone()),
            other => other
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| Some(other.to_string())),
        }
    };
    if json
        .as_ref()
        .and_then(|v| v.get("shownCommand"))
        .is_some_and(|c| !c.is_null())
    {
        return Outcome::Manual;
    }
    if let Some(error) = field("error") {
        return Outcome::Failed(short(&error));
    }
    if success {
        return Outcome::Done;
    }
    let stderr = String::from_utf8_lossy(stderr);
    let message = field("message")
        .or_else(|| {
            stderr
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| match code {
            Some(code) => format!("exit status {code}"),
            None => "killed".to_owned(),
        });
    Outcome::Failed(short(&message))
}

/// `text` on one line, sanitized, at most [`REASON_CHARS`] characters.
fn short(text: &str) -> String {
    let line = sanitize(text).replace(['\n', '\t', '\r'], " ");
    line.trim().chars().take(REASON_CHARS).collect()
}

// ── Download ──────────────────────────────────────────────────────────────────

/// The text of manifest `name` with hash `sha256`: cached, else downloaded.
async fn ready(dirs: &Dirs, name: &str, url: &str, sha256: &str) -> Result<String, String> {
    let dir = dirs.cache.clone();
    paths::ensure_private_dir(&dir).map_err(|e| format!("cache dir: {e}"))?;
    let file = dir.join(format!("{name}.toml"));
    let sidecar = dir.join(format!("{name}.sha256"));
    let cached =
        file.is_file() && std::fs::read_to_string(&sidecar).is_ok_and(|s| s.trim() == sha256);
    if !cached {
        if !file.exists() && count(&dir, ".toml") >= MAX_MANIFESTS {
            return Err(format!("cache holds {MAX_MANIFESTS} manifests already"));
        }
        let tmp = dir.join(format!(".{name}.{}.part", uuid::Uuid::new_v4().simple()));
        let allow_http = dirs.allow_http || test_http_ok(url);
        let result = download(url, &tmp, sha256, allow_http, MAX_BYTES).await;
        let placed = result.and_then(|()| place(&tmp, &file, &sidecar, sha256));
        if placed.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        placed?;
    }
    let mut raw = Vec::new();
    std::fs::File::open(&file)
        .and_then(|f| f.take(MAX_BYTES + 1).read_to_end(&mut raw))
        .map_err(|e| format!("cannot read the manifest: {e}"))?;
    String::from_utf8(raw).map_err(|_| "not UTF-8".to_owned())
}

fn count(dir: &Path, suffix: &str) -> usize {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().ends_with(suffix))
                .count()
        })
        .unwrap_or(0)
}

/// Move a verified download into place. The old sidecar goes first, so no
/// moment pairs a hash with a file it does not describe; a crash in between
/// leaves no sidecar, and the next request downloads again.
fn place(tmp: &Path, file: &Path, sidecar: &Path, sha256: &str) -> Result<(), String> {
    match std::fs::remove_file(sidecar) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("cannot replace the sidecar: {e}")),
    }
    std::fs::rename(tmp, file).map_err(|e| format!("cannot store the manifest: {e}"))?;
    paths::write_atomic(sidecar, format!("{sha256}\n").as_bytes())
        .map_err(|e| format!("cannot write the sidecar: {e}"))
}

/// Stream `url` into a new file `tmp` (0600), checking the size cap `cap`
/// and, at the end, the SHA-256. Redirects are followed to https URLs only.
async fn download(
    url: &str,
    tmp: &Path,
    sha256: &str,
    allow_http: bool,
    cap: u64,
) -> Result<(), String> {
    let policy = reqwest::redirect::Policy::custom(move |attempt| {
        let https = attempt.url().scheme() == "https";
        if attempt.previous().len() >= 5 {
            attempt.error("too many redirects")
        } else if https || allow_http {
            attempt.follow()
        } else {
            attempt.error("redirect to a non-https URL")
        }
    });
    let client = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(TOTAL_TIMEOUT)
        .redirect(policy)
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let mut resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("download failed: {}", e.without_url()))?;
    if !resp.status().is_success() {
        return Err(format!("download failed: HTTP {}", resp.status().as_u16()));
    }
    let over = || format!("over {} KiB", cap >> 10);
    if resp.content_length().is_some_and(|n| n > cap) {
        return Err(over());
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(tmp)
        .map_err(|e| format!("cannot create a temp file: {e}"))?;
    let mut hasher = Sha256::new();
    let mut total: u64 = 0;
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("download failed: {}", e.without_url()))?
    {
        total += chunk.len() as u64;
        if total > cap {
            return Err(over());
        }
        hasher.update(&chunk);
        file.write_all(&chunk)
            .map_err(|e| format!("cannot write the manifest: {e}"))?;
    }
    file.sync_all()
        .map_err(|e| format!("cannot write the manifest: {e}"))?;
    let got = format!("{:x}", hasher.finalize());
    if got != sha256 {
        return Err("hash mismatch".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_runs_outcome_comes_from_its_last_json_line() {
        let run = |ok, out: &str, err: &str| {
            outcome(ok, Some(i32::from(!ok)), out.as_bytes(), err.as_bytes())
        };
        assert_eq!(run(true, "Installing…\n{\"ok\":true}\n", ""), Outcome::Done);
        assert_eq!(run(true, "", ""), Outcome::Done);
        let shown = r#"{"shownCommand":{"command":"npx x","sha256":"ab"}}"#;
        assert_eq!(run(false, shown, ""), Outcome::Manual);
        assert_eq!(run(true, shown, ""), Outcome::Manual);
        assert_eq!(
            run(false, "{\"error\":\"Plugin not found\"}", "trace"),
            Outcome::Failed("Plugin not found".into())
        );
        assert_eq!(
            run(true, r#"{"error":{"message":"denied","code":3}}"#, ""),
            Outcome::Failed("denied".into())
        );
        assert_eq!(
            run(false, "not json", "\n  fatal: repository not found\nmore"),
            Outcome::Failed("fatal: repository not found".into())
        );
        assert_eq!(run(false, "", ""), Outcome::Failed("exit status 1".into()));
        assert_eq!(
            outcome(false, None, b"", b""),
            Outcome::Failed("killed".into())
        );
        let long = format!("{{\"error\":\"{}\\u001b[31m\"}}", "e".repeat(300));
        let Outcome::Failed(why) = run(false, &long, "") else {
            panic!("a failure")
        };
        assert_eq!(why, "e".repeat(REASON_CHARS));
        let Outcome::Failed(why) = run(false, "", "two\nlines\twith\u{1b}[1m codes") else {
            panic!("a failure")
        };
        assert_eq!(why, "two");
    }

    #[test]
    fn the_backoff_doubles_from_15_s_up_to_300_s() {
        let secs: Vec<u64> = (1..=8).map(|n| retry_after(n).as_secs()).collect();
        assert_eq!(secs, [15, 30, 60, 120, 240, 300, 300, 300]);
        assert_eq!(retry_after(u32::MAX), RETRY_MAX);
        assert_eq!(ceil_secs(Duration::from_millis(14_001)), 15);
        assert_eq!(ceil_secs(Duration::from_secs(1)), 1);
    }

    #[test]
    fn a_failure_waits_out_its_backoff_per_hash() {
        let mut mem = Memory::default();
        let key = |sha: &str| (sha.to_owned(), "plugin:a@m".to_owned());
        let t0 = Instant::now();
        let secs = Duration::from_secs;
        mem.fail(key("h1"), "down".into(), t0);
        assert_eq!(
            mem.failure("h1", "plugin:a@m", t0 + secs(14)),
            Some(("down".to_owned(), secs(1)))
        );
        assert_eq!(mem.failure("h1", "plugin:a@m", t0 + secs(15)), None);
        assert_eq!(mem.failure("h2", "plugin:a@m", t0), None, "another hash");
        // Failing again doubles the wait.
        let t1 = t0 + secs(15);
        mem.fail(key("h1"), "still down".into(), t1);
        assert_eq!(
            mem.failure("h1", "plugin:a@m", t1 + secs(29)),
            Some(("still down".to_owned(), secs(1)))
        );
        assert_eq!(mem.failure("h1", "plugin:a@m", t1 + secs(30)), None);
    }

    #[test]
    fn stale_zips_go_and_a_manifests_sidecar_stays() {
        let dir = std::env::temp_dir().join(format!("claudio-zips-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        for f in [
            "old.zip",
            "old.sha256",
            "kb.zip",
            "kb.toml",
            "kb.sha256",
            "x.toml",
        ] {
            std::fs::write(dir.join(f), "x").unwrap();
        }
        remove_stale_zips(&dir);
        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["kb.sha256", "kb.toml", "x.toml"]);
        remove_stale_zips(&dir.join("missing"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn claude_codes_files_are_read_only_when_regular_ours_and_json() {
        use std::os::unix::fs::symlink;
        let dir = std::env::temp_dir().join(format!("claudio-files-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("plugins")).unwrap();
        std::fs::create_dir_all(dir.join("agents")).unwrap();
        let agents = ["reviewer".to_owned(), "helper".to_owned()];
        // Nothing there: nothing installed, no servers of the member's own.
        let files = Files::read(&dir, &agents);
        assert!(files.installed.is_empty() && files.agents.is_empty());
        assert_eq!(files.servers, Some(HashSet::new()));
        std::fs::write(
            dir.join("plugins/installed_plugins.json"),
            r#"{"version":2,"plugins":{"a@m":[{"scope":"project"}]}}"#,
        )
        .unwrap();
        std::fs::write(dir.join(".claude.json"), r#"{"mcpServers":{"kb":{}}}"#).unwrap();
        std::fs::write(dir.join("agents/reviewer.md"), "mine").unwrap();
        let files = Files::read(&dir, &agents);
        assert_eq!(files.installed, HashSet::from(["a@m".to_owned()]));
        assert_eq!(files.servers, Some(HashSet::from(["kb".to_owned()])));
        assert_eq!(files.agents, HashSet::from(["reviewer".to_owned()]));
        // A symlink, or a file that is not JSON, is unreadable.
        std::fs::remove_file(dir.join(".claude.json")).unwrap();
        std::fs::write(dir.join("elsewhere.json"), r#"{"mcpServers":{}}"#).unwrap();
        symlink(dir.join("elsewhere.json"), dir.join(".claude.json")).unwrap();
        assert_eq!(Files::read(&dir, &agents).servers, None);
        std::fs::write(dir.join("plugins/installed_plugins.json"), "{").unwrap();
        assert!(Files::read(&dir, &agents).installed.is_empty());
        std::fs::write(
            dir.join("plugins/known_marketplaces.json"),
            r#"{"vocdoni":{"source":{"source":"github","repo":"vocdoni/skills"}}}"#,
        )
        .unwrap();
        assert_eq!(
            known_marketplaces(&dir),
            HashSet::from(["vocdoni".to_owned()])
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
