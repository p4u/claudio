//! Addon services (docs/addons.md, "Daemon ops"; docs/daemon-protocol.md,
//! "Addon services"): the one long-running child each daemon-side addon may
//! ask for. At daemon start and at every reload, the addon's
//! `DaemonOps::service` turns its own `config.toml` sections into an argv;
//! the supervisor runs `Config::claudio` with it in a process group of its
//! own, waits for its `ready` line, restarts it with a backoff whenever it
//! exits unasked, copies its stderr into `<runtime>/<addon>.log` (at most
//! 2 MiB with `.log.1`) and stops it (stdin closed, SIGTERM, SIGKILL) at a
//! reload to nothing and at shutdown. Its stdin is a pipe the daemon holds
//! and never writes (C3): however the daemon dies, the child sees EOF and
//! stops itself. This module names no addon (A3).

use std::collections::HashMap;
use std::io::{self, Read};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::watch;

use super::{Config, Daemon};
use crate::addon::{Addons, ServiceState, TEXT_BYTES};
use crate::claude::SESSION_MARKERS;

/// How the supervisor times a service ([`Config::service_timing`]; tests
/// shorten it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceTiming {
    /// How long a start may take to print its `ready` line.
    pub ready: Duration,
    /// From SIGTERM to SIGKILL when a service is stopped.
    pub stop: Duration,
    /// The first backoff after a failure; it doubles up to `backoff_max`.
    pub backoff: Duration,
    /// The longest backoff.
    pub backoff_max: Duration,
    /// Running this long resets the backoff.
    pub stable: Duration,
}

impl Default for ServiceTiming {
    fn default() -> ServiceTiming {
        ServiceTiming {
            ready: Duration::from_secs(20),
            stop: Duration::from_secs(3),
            backoff: Duration::from_secs(1),
            backoff_max: Duration::from_secs(60),
            stable: Duration::from_secs(5 * 60),
        }
    }
}

/// A log past this size is renamed to `<addon>.log.1` before the next write.
pub(crate) const LOG_MAX: u64 = 1 << 20;
/// The stderr bytes kept in memory for `Failed.tail`.
const TAIL_MAX: usize = 4096;
/// The most read while waiting for the `ready` line.
const READY_MAX: u64 = 4096;
/// The largest `config.toml` the supervisor reads.
const CONFIG_MAX: u64 = 1 << 20;
/// How long an exit report waits for the stderr copier to reach EOF.
const TAIL_WAIT: Duration = Duration::from_millis(500);

/// `(exit code, signal)` once the child has exited and been reaped.
type Exit = Option<(Option<i32>, Option<i32>)>;

/// Every daemon-side addon's service slot, by addon name.
pub(crate) struct Services {
    slots: HashMap<&'static str, Arc<Slot>>,
}

/// One addon's service.
struct Slot {
    addon: &'static str,
    /// Held across a reload or a restart: one at a time.
    turn: tokio::sync::Mutex<()>,
    tickets: Mutex<Tickets>,
    inner: Mutex<Inner>,
}

/// Reloads asked and run: a reload that queued behind a running one is
/// covered by the next one that runs.
#[derive(Default)]
struct Tickets {
    asked: u64,
    done: u64,
    force: bool,
}

struct Inner {
    state: ServiceState,
    /// When a `Failed` service starts again.
    retry_at: Option<Instant>,
    /// What the last start ran (a restart runs it again).
    argv: Option<Vec<String>>,
    child: Option<Child>,
    /// Bumped by every reload that acts and by shutdown: a pending restart
    /// or the exit of a child stopped on purpose is then stale.
    epoch: u64,
    backoff: Duration,
    /// Shutdown: nothing starts again.
    closed: bool,
}

/// A started child: what stops it.
struct Child {
    pid: u32,
    stdin: Option<ChildStdin>,
    exited: watch::Receiver<Exit>,
    tail: Arc<Mutex<Vec<u8>>>,
    copied: watch::Receiver<bool>,
    started: Instant,
}

impl Services {
    /// A slot for each addon with daemon ops; all `Off`.
    pub(crate) fn new(addons: &Addons, timing: ServiceTiming) -> Services {
        let slots = addons
            .iter()
            .filter(|a| a.daemon().is_some())
            .map(|a| {
                let slot = Slot {
                    addon: a.name(),
                    turn: tokio::sync::Mutex::new(()),
                    tickets: Mutex::default(),
                    inner: Mutex::new(Inner {
                        state: ServiceState::Off,
                        retry_at: None,
                        argv: None,
                        child: None,
                        epoch: 0,
                        backoff: timing.backoff,
                        closed: false,
                    }),
                };
                (a.name(), Arc::new(slot))
            })
            .collect();
        Services { slots }
    }

    /// `addon`'s service as of now (`Off` for an addon without a slot).
    pub(crate) fn state(&self, addon: &str) -> ServiceState {
        self.slots
            .get(addon)
            .map_or(ServiceState::Off, |s| s.state())
    }

    /// At daemon start (the socket listens): every addon's service, from
    /// config.toml. Nothing for a daemon playing the previous release, or
    /// without a config file.
    pub(crate) fn start_all(daemon: &Arc<Daemon>) {
        if daemon.config.old_peer || daemon.config.config_file.is_none() {
            return;
        }
        for addon in daemon.services.slots.keys().copied() {
            let daemon = Arc::clone(daemon);
            tokio::spawn(async move {
                daemon.services.reload(&daemon, addon, false).await;
            });
        }
    }

    /// `DaemonCtx::service_reload`: one reload at a time per addon; reloads
    /// queued behind a running one run as one (with `force` if any asked).
    pub(crate) async fn reload(
        &self,
        daemon: &Arc<Daemon>,
        addon: &str,
        force: bool,
    ) -> ServiceState {
        let Some(slot) = self.slots.get(addon) else {
            return ServiceState::Off;
        };
        let ticket = slot.ticket(force);
        let _turn = slot.turn.lock().await;
        if let Some(force) = slot.take(ticket) {
            converge(daemon, slot, force).await;
        }
        slot.state()
    }

    /// At shutdown, after the socket stopped listening: stop every service
    /// at once; nothing starts again.
    pub(crate) async fn stop_all(&self, grace: Duration) {
        let children: Vec<Child> = self
            .slots
            .values()
            .filter_map(|slot| {
                let mut inner = slot.lock();
                inner.closed = true;
                inner.epoch += 1;
                inner.retry_at = None;
                inner.child.take()
            })
            .collect();
        futures::future::join_all(children.into_iter().map(|c| stop(c, grace))).await;
    }

    /// What a daemon killed by a signal leaves: every stdin closed by the
    /// kernel, no signal sent, nothing restarted.
    #[cfg(test)]
    pub(crate) fn abandon(&self) {
        for slot in self.slots.values() {
            let mut inner = slot.lock();
            inner.closed = true;
            if let Some(child) = inner.child.as_mut() {
                child.stdin = None;
            }
        }
    }
}

impl Slot {
    /// The inner state; never held across an await.
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn state(&self) -> ServiceState {
        let inner = self.lock();
        match &inner.state {
            ServiceState::Failed {
                exit, signal, tail, ..
            } => ServiceState::Failed {
                exit: *exit,
                signal: *signal,
                tail: tail.clone(),
                retry_in: inner
                    .retry_at
                    .map(|at| at.saturating_duration_since(Instant::now())),
            },
            state => state.clone(),
        }
    }

    fn ticket(&self, force: bool) -> u64 {
        let mut t = self.tickets.lock().unwrap_or_else(|e| e.into_inner());
        t.asked += 1;
        t.force |= force;
        t.asked
    }

    /// `Some(force)` when `ticket` still needs a run (this run covers every
    /// ticket asked so far); `None` when one that started later has run.
    fn take(&self, ticket: u64) -> Option<bool> {
        let mut t = self.tickets.lock().unwrap_or_else(|e| e.into_inner());
        if t.done >= ticket {
            return None;
        }
        t.done = t.asked;
        Some(std::mem::take(&mut t.force))
    }
}

/// Re-ask the addon and act: nothing when it runs as asked (and not
/// `force`), else stop what runs and start what is asked.
async fn converge(daemon: &Arc<Daemon>, slot: &Arc<Slot>, force: bool) {
    let wanted = wanted(daemon, slot.addon).await;
    let stale = {
        let mut inner = slot.lock();
        if inner.closed {
            return;
        }
        let running = matches!(inner.state, ServiceState::Running { .. });
        let same = matches!(&wanted, Ok(Some(argv)) if inner.argv.as_ref() == Some(argv));
        if running && same && !force {
            return;
        }
        inner.epoch += 1;
        inner.retry_at = None;
        inner.backoff = daemon.config.service_timing.backoff;
        inner.child.take()
    };
    if let Some(child) = stale {
        stop(child, daemon.config.service_timing.stop).await;
    }
    match wanted {
        Ok(Some(argv)) => start(daemon, slot, argv).await,
        Ok(None) => {
            let mut inner = slot.lock();
            inner.state = ServiceState::Off;
            inner.argv = None;
        }
        Err(why) => {
            let mut inner = slot.lock();
            inner.argv = None;
            inner.state = ServiceState::Failed {
                exit: None,
                signal: None,
                tail: crate::addon::line(&why, TEXT_BYTES),
                retry_in: None,
            };
        }
    }
}

/// What `addon` asks for now: its `service()` over its own sections of the
/// config file, read off the runtime.
async fn wanted(daemon: &Daemon, addon: &str) -> Result<Option<Vec<String>>, String> {
    let Some(path) = daemon.config.config_file.clone() else {
        return Ok(None);
    };
    if daemon.config.old_peer {
        return Ok(None);
    }
    let Some(found) = daemon.addons.get(addon) else {
        return Ok(None);
    };
    let Some(ops) = found.daemon() else {
        return Ok(None);
    };
    let table = tokio::task::spawn_blocking(move || read_config(&path))
        .await
        .ok()
        .flatten()
        .ok_or_else(|| "config.toml does not parse".to_owned())?;
    let sections = found.config_sections();
    let section = |name: &str| -> toml::Table {
        match table.get(name) {
            Some(toml::Value::Table(t)) if sections.contains(&name) => t.clone(),
            _ => toml::Table::new(),
        }
    };
    ops.service(&section)
}

/// config.toml as a table: empty when missing, `None` when it cannot be
/// read, is not a regular file (a FIFO would block the open), is over
/// 1 MiB or is not TOML.
pub(super) fn read_config(path: &Path) -> Option<toml::Table> {
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_file() => {}
        Ok(_) => return None,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Some(toml::Table::new()),
        Err(_) => return None,
    }
    let file = std::fs::File::open(path).ok()?;
    let mut text = String::new();
    file.take(CONFIG_MAX + 1).read_to_string(&mut text).ok()?;
    if text.len() as u64 > CONFIG_MAX {
        return None;
    }
    text.parse().ok()
}

/// Start `argv` and wait for its ready line (under the slot's turn).
async fn start(daemon: &Arc<Daemon>, slot: &Arc<Slot>, argv: Vec<String>) {
    let timing = daemon.config.service_timing;
    {
        let mut inner = slot.lock();
        inner.state = ServiceState::Starting;
        inner.retry_at = None;
        inner.argv = Some(argv.clone());
    }
    let log = log_path(&daemon.config, slot.addon);
    let (child, stdout) = match spawn(&daemon.config, &argv, &log) {
        Ok(started) => started,
        Err(e) => {
            let why = format!("cannot run {}: {e}", daemon.config.claudio.display());
            return fail(daemon, slot, None, why);
        }
    };
    let (pid, mut exited) = (child.pid, child.exited.clone());
    let closed = {
        let mut inner = slot.lock();
        if inner.closed {
            Some(child)
        } else {
            inner.child = Some(child);
            None
        }
    };
    if let Some(child) = closed {
        stop(child, timing.stop).await;
        return;
    }
    // The ready line, or the leader's exit, whichever comes first: a process
    // that kept the child's stdout open must not delay `stop`, whose group
    // signal is only sure to reach this group right after the reap.
    let ready = tokio::select! {
        biased;
        ready = wait_ready(stdout, timing.ready) => ready,
        _ = wait_exit(&mut exited) => Err("it exited before it was ready".to_owned()),
    };
    match ready {
        Ok(out) => {
            tokio::spawn(async move {
                let mut out = out;
                let _ = tokio::io::copy(&mut out, &mut tokio::io::sink()).await;
            });
            let mut inner = slot.lock();
            let Some(child) = inner.child.as_ref() else {
                return;
            };
            let watch = (child.exited.clone(), child.started, inner.epoch);
            inner.state = ServiceState::Running {
                pid,
                since: crate::paths::unix_now(),
                argv,
            };
            drop(inner);
            tracing::info!(addon = slot.addon, pid, "service running");
            let (daemon, slot) = (Arc::clone(daemon), Arc::clone(slot));
            tokio::spawn(monitor(daemon, slot, watch.0, watch.1, watch.2));
        }
        Err(why) => {
            let child = slot.lock().child.take();
            let Some(child) = child else {
                return;
            };
            let (tail, mut copied) = (Arc::clone(&child.tail), child.copied.clone());
            let exit = stop(child, timing.stop).await;
            let _ = tokio::time::timeout(TAIL_WAIT, copied.wait_for(|done| *done)).await;
            fail(daemon, slot, exit, tail_or(&tail, why));
        }
    }
}

/// Wait for a running child's exit; one nobody asked for is a failure.
async fn monitor(
    daemon: Arc<Daemon>,
    slot: Arc<Slot>,
    mut exited: watch::Receiver<Exit>,
    started: Instant,
    epoch: u64,
) {
    let exit = wait_exit(&mut exited).await;
    // Under the slot's turn, like every other transition: a reload either
    // ran first (the epoch moved: this exit was its doing) or waits until
    // this failure is recorded, and then starts the service at once.
    let _turn = slot.turn.lock().await;
    let child = {
        let mut inner = slot.lock();
        if inner.epoch != epoch || inner.closed {
            return;
        }
        if started.elapsed() >= daemon.config.service_timing.stable {
            inner.backoff = daemon.config.service_timing.backoff;
        }
        inner.child.take()
    };
    let Some(mut child) = child else {
        return;
    };
    tracing::warn!(addon = slot.addon, pid = child.pid, ?exit, "service exited");
    // Its leftovers in its group, then its last stderr lines.
    signal_group(child.pid, libc::SIGKILL);
    let _ = tokio::time::timeout(TAIL_WAIT, child.copied.wait_for(|done| *done)).await;
    let why = "it exited".to_owned();
    fail(&daemon, &slot, exit, tail_or(&child.tail, why));
}

/// `Failed` with the next start after the backoff (which then doubles),
/// unless the daemon is shutting down.
fn fail(daemon: &Arc<Daemon>, slot: &Arc<Slot>, exit: Exit, tail: String) {
    let timing = daemon.config.service_timing;
    let (code, signal) = exit.unwrap_or((None, None));
    let mut inner = slot.lock();
    if inner.closed {
        return;
    }
    let delay = inner.backoff;
    inner.backoff = (delay * 2).min(timing.backoff_max);
    inner.retry_at = Some(Instant::now() + delay);
    inner.state = ServiceState::Failed {
        exit: code,
        signal,
        tail,
        retry_in: None,
    };
    let epoch = inner.epoch;
    drop(inner);
    let (daemon, slot) = (Arc::clone(daemon), Arc::clone(slot));
    tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        let _turn = slot.turn.lock().await;
        let argv = {
            let inner = slot.lock();
            if inner.epoch != epoch || inner.closed {
                return;
            }
            inner.argv.clone()
        };
        if let Some(argv) = argv {
            start(&daemon, &slot, argv).await;
        }
    });
}

/// `<runtime>/<addon>.log`: beside the daemon's socket.
pub(crate) fn log_path(config: &Config, addon: &str) -> PathBuf {
    let dir = config.socket.parent().unwrap_or(Path::new("."));
    dir.join(format!("{addon}.log"))
}

/// Run `Config::claudio` with `argv` in a process group of its own: the
/// daemon's env without the session markers and with `Config::path`; stdin
/// a pipe held here, stdout for the ready line, stderr to the log.
fn spawn(config: &Config, argv: &[String], log: &Path) -> io::Result<(Child, ChildStdout)> {
    let mut cmd = Command::new(&config.claudio);
    cmd.args(argv)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    for marker in SESSION_MARKERS {
        cmd.env_remove(marker);
    }
    if let Some(path) = &config.path {
        cmd.env("PATH", path);
    }
    let mut child = cmd.spawn()?;
    let pid = child.id().ok_or_else(|| io::Error::other("no pid"))?;
    let (stdin, stdout, stderr) = (child.stdin.take(), child.stdout.take(), child.stderr.take());
    let (Some(stdout), Some(stderr)) = (stdout, stderr) else {
        return Err(io::Error::other("no pipes"));
    };
    let (exit_tx, exited) = watch::channel(None);
    tokio::spawn(async move {
        let exit = match child.wait().await {
            Ok(status) => (status.code(), status.signal()),
            Err(_) => (None, None),
        };
        let _ = exit_tx.send(Some(exit));
    });
    let tail = Arc::new(Mutex::new(Vec::new()));
    let (copied_tx, copied) = watch::channel(false);
    tokio::spawn(copy_stderr(
        stderr,
        log.to_path_buf(),
        Arc::clone(&tail),
        copied_tx,
    ));
    let child = Child {
        pid,
        stdin,
        exited,
        tail,
        copied,
        started: Instant::now(),
    };
    Ok((child, stdout))
}

/// The first stdout line, read within `within` and at most 4 KiB: a line
/// that starts with `ready`, or why not.
async fn wait_ready(
    stdout: ChildStdout,
    within: Duration,
) -> Result<BufReader<ChildStdout>, String> {
    let mut out = BufReader::new(stdout);
    let mut line = Vec::new();
    let mut first = (&mut out).take(READY_MAX);
    match tokio::time::timeout(within, first.read_until(b'\n', &mut line)).await {
        Err(_) => Err(format!("no ready line within {within:?}")),
        Ok(Err(e)) => Err(format!("cannot read its output: {e}")),
        Ok(Ok(0)) => Err("it exited before it was ready".to_owned()),
        Ok(Ok(_)) if line.starts_with(b"ready") && line.ends_with(b"\n") => Ok(out),
        Ok(Ok(_)) => Err("its first line is not `ready`".to_owned()),
    }
}

/// Stop a child: stdin closed, SIGTERM to its group, SIGKILL after
/// `grace`, reaped, and SIGKILL for whatever of its group is left.
async fn stop(mut child: Child, grace: Duration) -> Exit {
    child.stdin = None;
    if child.exited.borrow().is_none() {
        signal_group(child.pid, libc::SIGTERM);
    }
    if tokio::time::timeout(grace, wait_exit(&mut child.exited))
        .await
        .is_err()
    {
        signal_group(child.pid, libc::SIGKILL);
    }
    let exit = wait_exit(&mut child.exited).await;
    // A group id is not reused while a member lives: this reaches only them.
    signal_group(child.pid, libc::SIGKILL);
    exit
}

async fn wait_exit(exited: &mut watch::Receiver<Exit>) -> Exit {
    match exited.wait_for(Option::is_some).await {
        Ok(exit) => *exit,
        Err(_) => None,
    }
}

fn signal_group(pid: u32, signal: i32) {
    if let Ok(pid) = i32::try_from(pid) {
        if pid > 1 {
            // SAFETY: kill(2) with a negative pid signals that process
            // group, which the child leads (`process_group(0)`).
            unsafe { libc::kill(-pid, signal) };
        }
    }
}

/// The stderr tail as text (sanitised, its last 4 KiB), or `why` when the
/// child wrote nothing.
fn tail_or(tail: &Mutex<Vec<u8>>, why: String) -> String {
    let bytes = tail.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let text = crate::text::sanitize(&String::from_utf8_lossy(&bytes));
    let text = text.trim();
    if text.is_empty() {
        return why;
    }
    let mut start = text.len().saturating_sub(TAIL_MAX);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..].to_owned()
}

/// Copy the child's stderr into its log (0600; renamed to `.log.1` before
/// a write would take it past [`LOG_MAX`]) and keep its last 4 KiB.
async fn copy_stderr(
    mut err: ChildStderr,
    path: PathBuf,
    tail: Arc<Mutex<Vec<u8>>>,
    done: watch::Sender<bool>,
) {
    let mut log = LogFile::open(&path).await;
    let mut buf = vec![0u8; 8192];
    loop {
        let n = match err.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        {
            let mut t = tail.lock().unwrap_or_else(|e| e.into_inner());
            t.extend_from_slice(&buf[..n]);
            let excess = t.len().saturating_sub(TAIL_MAX);
            t.drain(..excess);
        }
        if let Some(l) = log.as_mut() {
            l.write(&buf[..n]).await;
        }
    }
    let _ = done.send(true);
}

/// The open log and its size.
struct LogFile {
    path: PathBuf,
    file: tokio::fs::File,
    len: u64,
}

impl LogFile {
    async fn open(path: &Path) -> Option<LogFile> {
        let file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(path)
            .await
            .ok()?;
        let len = file.metadata().await.map_or(0, |m| m.len());
        Some(LogFile {
            path: path.to_path_buf(),
            file,
            len,
        })
    }

    async fn write(&mut self, bytes: &[u8]) {
        if self.len > 0 && self.len + bytes.len() as u64 > LOG_MAX {
            let old = self.path.with_extension("log.1");
            if tokio::fs::rename(&self.path, &old).await.is_ok() {
                if let Some(fresh) = LogFile::open(&self.path).await {
                    *self = fresh;
                }
            } else if self.file.set_len(0).await.is_ok() {
                // No rotation (`.log.1` is in the way): start over rather
                // than grow past the cap.
                self.len = 0;
            }
        }
        if self.file.write_all(bytes).await.is_ok() && self.file.flush().await.is_ok() {
            self.len += bytes.len() as u64;
        }
    }
}
