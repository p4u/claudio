//! The extension host: runs external extensions' programs and talks to
//! them over protocol v1 (JSON lines on stdin/stdout, [`super::protocol`]).
//!
//! Driven by `tui/mod.rs` (never by `App`): each request is a future the
//! event loop spawns, so a slow or hung program never blocks the UI thread.
//! Per extension:
//! - the program starts on the first request, in the extension's directory,
//!   in its own process group, stdin/stdout piped, stderr discarded, with
//!   the client's environment minus anything secret or claudio's;
//! - at most [`MAX_IN_FLIGHT`] requests are in flight, each with a timeout;
//! - a crash, a protocol error (a line that is not JSON, or longer than
//!   [`protocol::MAX_LINE`]) or a timeout is a failure; the program is
//!   started again on the next request, and [`FAILURES`] failures within
//!   [`FAILURE_WINDOW`] disable the extension for the rest of the run;
//! - a request that cannot be written within its timeout (a program that
//!   stopped reading) is a timeout too, and the program is stopped;
//! - [`ExtensionHost::shutdown`] stops every program (SIGTERM to its group,
//!   SIGKILL after a second), even one a stuck request still holds.
//!
//! The host knows every loaded external extension, on or off: one the user
//! turned off ([`ExtensionHost::set_enabled`]) is refused without starting
//! its program, and turning it off stops a running one the same way.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, Weak};
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot, Mutex, Semaphore};

use super::manifest::Program;
use super::protocol::{self, Rendering, SessionContext};
use super::{ActionResult, Registry, Source};
use crate::proto::LensKind;

/// How long a render may take.
pub const RENDER_TIMEOUT: Duration = Duration::from_secs(1);
/// How long an action may take.
pub const ACTION_TIMEOUT: Duration = Duration::from_secs(15);
/// Requests in flight per extension; more wait for a slot.
pub const MAX_IN_FLIGHT: usize = 16;
/// Failures within [`FAILURE_WINDOW`] that disable an extension.
pub const FAILURES: usize = 3;
pub const FAILURE_WINDOW: Duration = Duration::from_secs(300);
/// How long a failed render is remembered (shown raw, not asked again).
const FAILED_RENDER_TTL: Duration = Duration::from_secs(60);
/// Renderings remembered, by lens and source.
const RENDER_CACHE: usize = 1024;
/// How long a stopped program has to exit before it is killed.
const STOP_GRACE: Duration = Duration::from_secs(1);

/// Why a request got no result.
#[derive(Debug, Clone, PartialEq)]
pub enum Failure {
    /// The extension was disabled after too many failures.
    Disabled,
    /// The user turned the extension off.
    Off,
    /// The program could not be started.
    Start(String),
    /// It exited, or broke the protocol, before answering.
    Crashed,
    Timeout,
    /// It answered with an error.
    Error(String),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Disabled => write!(f, "disabled after {FAILURES} failures"),
            Failure::Off => write!(f, "turned off"),
            Failure::Start(why) => write!(f, "could not start: {why}"),
            Failure::Crashed => write!(f, "stopped before answering"),
            Failure::Timeout => write!(f, "timed out"),
            Failure::Error(why) => write!(f, "failed: {why}"),
        }
    }
}

/// Whether the client's variable `name` stays out of an extension's
/// environment: claude's session markers, anything secret, and claudio's.
fn scrubbed(name: &str) -> bool {
    crate::claude::SESSION_MARKERS.contains(&name)
        || matches!(name, "ANTHROPIC_API_KEY" | "ANTHROPIC_AUTH_TOKEN")
        || name.starts_with("CLAUDIO_")
}

type Pending = Arc<StdMutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

/// A running program.
struct Proc {
    /// Which start this is: a reader that ends only clears its own.
    gen: u64,
    stdin: ChildStdin,
    pid: Option<u32>,
    child: Child,
}

/// One external extension's program and its health.
struct Ext {
    name: String,
    program: Program,
    proc: Mutex<Option<Proc>>,
    /// The running start and its pid, outside `proc`'s lock: what
    /// `shutdown` and `lost` signal when they cannot get that lock in time.
    pid: StdMutex<Option<(u64, u32)>>,
    pending: Pending,
    /// Failure times within the window, and whether it is disabled.
    health: StdMutex<(VecDeque<Instant>, bool)>,
    /// Turned off by the user (not a failure; turned on again at will).
    off: AtomicBool,
    slots: Semaphore,
    next_id: AtomicU64,
    gen: AtomicU64,
    notices: mpsc::UnboundedSender<String>,
}

impl Ext {
    fn disabled(&self) -> bool {
        self.health.lock().unwrap_or_else(|e| e.into_inner()).1
    }

    /// Why a request is refused now: turned off, or disabled.
    fn refused(&self) -> Option<Failure> {
        if self.off.load(Ordering::Acquire) {
            Some(Failure::Off)
        } else if self.disabled() {
            Some(Failure::Disabled)
        } else {
            None
        }
    }

    /// Count a failure; the [`FAILURES`]th within the window disables the
    /// extension, with a notice. What ends because it was turned off is
    /// not one.
    fn fail(&self, why: &str) {
        if self.off.load(Ordering::Acquire) {
            return;
        }
        let mut health = self.health.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        health.0.push_back(now);
        while health
            .0
            .front()
            .is_some_and(|t| now.duration_since(*t) > FAILURE_WINDOW)
        {
            health.0.pop_front();
        }
        if !health.1 && health.0.len() >= FAILURES {
            health.1 = true;
            let _ = self.notices.send(format!(
                "extension {} disabled after {FAILURES} failures ({why})",
                self.name
            ));
        }
    }

    /// Send `method` with `params` and wait at most `timeout` for its result.
    async fn call(
        self: &Arc<Self>,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, Failure> {
        if let Some(refused) = self.refused() {
            return Err(refused);
        }
        let _slot = self.slots.acquire().await.map_err(|_| Failure::Disabled)?;
        // It may have been disabled while this waited for a slot.
        if let Some(refused) = self.refused() {
            return Err(refused);
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = oneshot::channel();
        self.lock_pending().insert(id, tx);
        let mut line = protocol::request(id, method, params);
        line.push('\n');
        let deadline;
        let written = {
            let mut slot = self.proc.lock().await;
            // Or while it waited for the program: do not start it again.
            if let Some(refused) = self.refused() {
                self.lock_pending().remove(&id);
                return Err(refused);
            }
            if slot.is_none() {
                match self.start() {
                    Ok(proc) => *slot = Some(proc),
                    Err(e) => {
                        self.lock_pending().remove(&id);
                        self.fail("could not start");
                        return Err(Failure::Start(e.to_string()));
                    }
                }
            }
            let proc = slot.as_mut().expect("started above");
            let stdin = &mut proc.stdin;
            // A program that stopped reading fills the pipe and the write
            // waits forever, holding the lock: bound it like the reply.
            deadline = tokio::time::Instant::now() + timeout;
            let write = async {
                stdin.write_all(line.as_bytes()).await?;
                stdin.flush().await
            };
            match tokio::time::timeout_at(deadline, write).await {
                Ok(written) => written,
                Err(_) => {
                    if let Some(proc) = slot.take() {
                        self.stop(proc);
                    }
                    self.lock_pending().clear();
                    self.fail("timed out");
                    return Err(Failure::Timeout);
                }
            }
        };
        if written.is_err() {
            // The reader sees the exit and counts it.
            self.lock_pending().remove(&id);
            return Err(Failure::Crashed);
        }
        match tokio::time::timeout_at(deadline, rx).await {
            Ok(Ok(Ok(value))) => Ok(value),
            Ok(Ok(Err(why))) => Err(Failure::Error(why)),
            Ok(Err(_)) => Err(Failure::Crashed),
            Err(_) => {
                self.lock_pending().remove(&id);
                self.fail("timed out");
                Err(Failure::Timeout)
            }
        }
    }

    fn lock_pending(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<u64, oneshot::Sender<Result<Value, String>>>> {
        self.pending.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Start the program, and the task that reads its replies.
    fn start(self: &Arc<Self>) -> std::io::Result<Proc> {
        let mut cmd = Command::new(self.program.program());
        cmd.args(self.program.command.iter().skip(1))
            .current_dir(&self.program.dir)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .process_group(0)
            .kill_on_drop(true);
        for (name, _) in std::env::vars_os() {
            if scrubbed(&name.to_string_lossy()) {
                cmd.env_remove(name);
            }
        }
        cmd.env("CLAUDIO_EXTENSION", &self.name);
        cmd.env("CLAUDIO_EXTENSION_PROTOCOL", protocol::VERSION.to_string());
        let mut child = cmd.spawn()?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            return Err(std::io::Error::other("no pipes"));
        };
        let gen = self.gen.fetch_add(1, Ordering::Relaxed) + 1;
        *self.lock_pid() = child.id().map(|pid| (gen, pid));
        tokio::spawn(read_replies(Arc::downgrade(self), gen, stdout));
        Ok(Proc {
            gen,
            stdin,
            pid: child.id(),
            child,
        })
    }

    fn lock_pid(&self) -> std::sync::MutexGuard<'_, Option<(u64, u32)>> {
        self.pid.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The pid of start `gen` if it is still the running one.
    fn pid_of(&self, gen: u64) -> Option<u32> {
        self.lock_pid()
            .filter(|(g, _)| *g == gen)
            .map(|(_, pid)| pid)
    }

    /// Stop `proc` (see [`stop`]) and forget its pid.
    fn stop(&self, proc: Proc) {
        let mut pid = self.lock_pid();
        if pid.is_some_and(|(g, _)| g == proc.gen) {
            *pid = None;
        }
        drop(pid);
        stop(proc);
    }

    /// Start `gen` ended (`why`): stop it, fail what waits on it, count it.
    async fn lost(&self, gen: u64, why: &str) {
        let mut slot = match tokio::time::timeout(STOP_GRACE, self.proc.lock()).await {
            Ok(slot) => slot,
            Err(_) => {
                // A request holds the lock writing to it: end it by its
                // pid, so that write fails and lets go of the lock.
                signal_group(self.pid_of(gen), libc::SIGTERM);
                self.proc.lock().await
            }
        };
        if slot.as_ref().is_some_and(|p| p.gen == gen) {
            if let Some(proc) = slot.take() {
                self.stop(proc);
            }
            self.lock_pending().clear();
            self.fail(why);
        }
    }

    /// Stop the running program, if any: SIGTERM to its group, SIGKILL
    /// after [`STOP_GRACE`]. Bounded even when a request holds the lock:
    /// the program is then signalled by its pid, without the lock.
    async fn halt(&self) {
        let Ok(mut slot) = tokio::time::timeout(STOP_GRACE, self.proc.lock()).await else {
            let pid = self.lock_pid().take().map(|(_, pid)| pid);
            signal_group(pid, libc::SIGTERM);
            tokio::time::sleep(STOP_GRACE).await;
            signal_group(pid, libc::SIGKILL);
            return;
        };
        let Some(mut proc) = slot.take() else {
            return;
        };
        drop(slot);
        *self.lock_pid() = None;
        // What waits on it ends now (not as a timeout).
        self.lock_pending().clear();
        signal_group(proc.pid, libc::SIGTERM);
        if tokio::time::timeout(STOP_GRACE, proc.child.wait())
            .await
            .is_err()
        {
            signal_group(proc.pid, libc::SIGKILL);
            let _ = proc.child.wait().await;
        }
    }
}

/// Read start `gen`'s replies until it exits or breaks the protocol.
async fn read_replies(ext: Weak<Ext>, gen: u64, stdout: ChildStdout) {
    let mut reader = BufReader::new(stdout);
    let why = loop {
        let mut line = Vec::new();
        let limit = protocol::MAX_LINE as u64 + 1;
        match (&mut reader).take(limit).read_until(b'\n', &mut line).await {
            Ok(0) | Err(_) => break "exited",
            Ok(_) => {}
        }
        if line.pop() != Some(b'\n') {
            break if line.len() >= protocol::MAX_LINE {
                "sent a line over 1 MiB"
            } else {
                "exited"
            };
        }
        let Some(ext) = ext.upgrade() else { return };
        match protocol::reply(&line) {
            Ok(Some((id, result))) => {
                if let Some(tx) = ext.lock_pending().remove(&id) {
                    let _ = tx.send(result);
                }
            }
            Ok(None) => {}
            Err(_) => break "protocol error",
        }
    };
    if let Some(ext) = ext.upgrade() {
        ext.lost(gen, why).await;
    }
}

/// SIGTERM to `proc`'s group, SIGKILL after [`STOP_GRACE`]; reaped in the
/// background.
fn stop(mut proc: Proc) {
    signal_group(proc.pid, libc::SIGTERM);
    tokio::spawn(async move {
        if tokio::time::timeout(STOP_GRACE, proc.child.wait())
            .await
            .is_err()
        {
            signal_group(proc.pid, libc::SIGKILL);
            let _ = proc.child.wait().await;
        }
    });
}

fn signal_group(pid: Option<u32>, signal: i32) {
    if let Some(pid) = pid.filter(|p| *p > 1) {
        // SAFETY: kill(2) with a negative pid signals that process group,
        // which the program leads (it was started with process_group(0)).
        unsafe {
            libc::kill(-(pid as i32), signal);
        }
    }
}

/// Renderings by lens and source: a success is kept, a failure for
/// [`FAILED_RENDER_TTL`].
#[derive(Default)]
struct RenderCache {
    entries: HashMap<(String, String), (Instant, Option<Rendering>)>,
}

/// The external extensions' programs, by extension name.
pub struct ExtensionHost {
    exts: HashMap<String, Arc<Ext>>,
    /// Which extension owns each lens.
    lenses: HashMap<String, String>,
    cache: Arc<StdMutex<RenderCache>>,
}

impl ExtensionHost {
    /// The host of `registry`'s external extensions, on or off (none
    /// started yet). Notices (an extension disabled) go to `notices`.
    pub fn new(registry: &Registry, notices: mpsc::UnboundedSender<String>) -> ExtensionHost {
        let mut exts = HashMap::new();
        let mut lenses = HashMap::new();
        for ext in &registry.extensions {
            let Source::External(program) = &ext.source else {
                continue;
            };
            for lens in &ext.lenses {
                lenses.insert(lens.name.clone(), ext.name.clone());
            }
            let handle = Ext {
                name: ext.name.clone(),
                program: program.clone(),
                proc: Mutex::new(None),
                pid: StdMutex::new(None),
                pending: Arc::default(),
                health: StdMutex::new((VecDeque::new(), false)),
                off: AtomicBool::new(!ext.enabled),
                slots: Semaphore::new(MAX_IN_FLIGHT),
                next_id: AtomicU64::new(0),
                gen: AtomicU64::new(0),
                notices: notices.clone(),
            };
            exts.insert(ext.name.clone(), Arc::new(handle));
        }
        ExtensionHost {
            exts,
            lenses,
            cache: Arc::default(),
        }
    }

    /// Render `source` of lens `lens` with its extension (within
    /// [`RENDER_TIMEOUT`]); `None` leaves it raw (no such lens, a failure,
    /// an error, or a result over the caps). Cached by lens and source.
    pub fn render(
        &self,
        lens: &str,
        kind: LensKind,
        source: &str,
    ) -> impl Future<Output = Option<Rendering>> + Send + 'static {
        let ext = self
            .lenses
            .get(lens)
            .and_then(|e| self.exts.get(e))
            .cloned();
        let cache = Arc::clone(&self.cache);
        let key = (lens.to_owned(), source.to_owned());
        let kind = match kind {
            LensKind::Fence => "fence",
            _ => "tag",
        };
        async move {
            let ext = ext.filter(|e| !e.off.load(Ordering::Acquire))?;
            if let Some((at, cached)) = cache
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .entries
                .get(&key)
            {
                if cached.is_some() || at.elapsed() < FAILED_RENDER_TTL {
                    return cached.clone();
                }
            }
            let params = protocol::render_params(&key.0, kind, &key.1);
            let result = ext.call("render", params, RENDER_TIMEOUT).await;
            // Turned off meanwhile: not a failed rendering to remember.
            if result == Err(Failure::Off) {
                return None;
            }
            let rendering = result.ok().as_ref().and_then(protocol::rendering);
            let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
            if cache.entries.len() >= RENDER_CACHE {
                cache.entries.clear();
            }
            cache
                .entries
                .insert(key, (Instant::now(), rendering.clone()));
            rendering
        }
    }

    /// Run action `action` of extension `ext` (within [`ACTION_TIMEOUT`]):
    /// its result, or the notice that says why there is none.
    pub fn action(
        &self,
        ext: &str,
        action: &str,
        input: Option<String>,
        session: Option<SessionContext>,
    ) -> impl Future<Output = Result<ActionResult, String>> + Send + 'static {
        let handle = self.exts.get(ext).cloned();
        let (name, params) = (
            ext.to_owned(),
            protocol::action_params(action, input.as_deref(), session.as_ref()),
        );
        async move {
            let Some(handle) = handle else {
                return Err(format!("extension {name} is not running"));
            };
            match handle.call("action", params, ACTION_TIMEOUT).await {
                Ok(result) => protocol::action_result(&result),
                Err(Failure::Timeout) => Err(format!("extension {name} timed out")),
                Err(failure) => Err(format!("extension {name}: {failure}")),
            }
        }
    }

    /// Turn extension `name` on or off. Off, its requests are refused
    /// without starting it, and the returned future stops its running
    /// program (see [`ExtensionHost::shutdown`]); on, it starts again on
    /// the next request, unless it was disabled after failures (that
    /// lasts until claudio restarts). The flag changes at once.
    pub fn set_enabled(&self, name: &str, on: bool) -> impl Future<Output = ()> + Send + 'static {
        let ext = self.exts.get(name).cloned();
        if let Some(ext) = &ext {
            ext.off.store(!on, Ordering::Release);
        }
        async move {
            if let Some(ext) = ext.filter(|_| !on) {
                ext.halt().await;
            }
        }
    }

    /// The extensions disabled after failures, by name.
    pub fn failed(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .exts
            .values()
            .filter(|e| e.disabled())
            .map(|e| e.name.clone())
            .collect();
        names.sort();
        names
    }

    /// Stop every program: SIGTERM to its group, SIGKILL after a second.
    /// Bounded even when a request holds a program's lock: the program is
    /// then signalled by its pid, without the lock.
    pub async fn shutdown(&self) {
        futures::future::join_all(self.exts.values().map(|ext| ext.halt())).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::extensions::{Extension, Lens};

    /// A scratch dir with an extension program `prog.sh` (`script`).
    fn program(script: &str) -> (std::path::PathBuf, Program) {
        let dir = std::env::temp_dir().join(format!("claudio-exthost-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("prog.sh");
        std::fs::write(&path, format!("#!/bin/sh\n{script}")).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let program = Program {
            dir: dir.clone(),
            command: vec!["./prog.sh".into()],
        };
        (dir, program)
    }

    /// A host with one extension `fake` (lens `keys`) running `script`.
    fn host(
        script: &str,
    ) -> (
        std::path::PathBuf,
        ExtensionHost,
        mpsc::UnboundedReceiver<String>,
    ) {
        let (dir, program) = program(script);
        let ext = Extension {
            name: "fake".into(),
            description: String::new(),
            enabled: true,
            lenses: vec![Lens {
                name: "keys".into(),
                kind: LensKind::Tag,
                instruction: String::new(),
            }],
            actions: vec![],
            source: Source::External(program),
        };
        let registry = Registry::new(&Default::default(), vec![ext]);
        let (tx, rx) = mpsc::unbounded_channel();
        (dir, ExtensionHost::new(&registry, tx), rx)
    }

    /// A shell loop answering each request line with `reply`, where `$id`
    /// is the request's id (sed-extracted).
    fn answering(reply: &str) -> String {
        format!(
            "while IFS= read -r line; do\n\
             id=$(printf '%s' \"$line\" | sed 's/^{{\"id\":\\([0-9]*\\).*/\\1/')\n\
             {reply}\n\
             done\n"
        )
    }

    #[test]
    fn secrets_markers_and_claudio_variables_are_scrubbed() {
        for name in [
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "CLAUDIO_PROXY_URL",
            "CLAUDIO_CLAUDE_PATH",
            "CLAUDECODE",
            "CLAUDE_CODE_SESSION_ID",
        ] {
            assert!(scrubbed(name), "{name}");
        }
        for name in ["PATH", "HOME", "LANG", "ANTHROPIC_MODEL"] {
            assert!(!scrubbed(name), "{name}");
        }
    }

    #[tokio::test]
    async fn renders_and_actions_go_through_the_program() {
        let script = answering(
            "case \"$line\" in\n\
             *render*) printf '{\"id\":%s,\"result\":{\"inline\":\"[%s]\"}}\\n' \"$id\" \"$CLAUDIO_EXTENSION\" ;;\n\
             *) printf '{\"id\":%s,\"result\":{\"kind\":\"paste\",\"text\":\"%s %s\"}}\\n' \"$id\" \"$(pwd | sed 's|.*/||')\" \"$CLAUDIO_EXTENSION_PROTOCOL\" ;;\n\
             esac",
        );
        let (dir, host, _notices) = host(&script);
        let r = host.render("keys", LensKind::Tag, "Ctrl+C").await;
        assert_eq!(r, Some((Some("[fake]".to_owned()), None)));
        // Not this host's lens.
        assert_eq!(host.render("other", LensKind::Tag, "x").await, None);
        // It runs in the extension's directory, told the protocol version.
        let result = host.action("fake", "stamp", None, None).await;
        let here = dir.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(
            result,
            Ok(ActionResult::Paste {
                text: format!("{here} 1")
            })
        );
        host.shutdown().await;
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_hung_program_times_out_and_three_failures_disable_it() {
        let (dir, host, mut notices) = host("sleep 30\n");
        let started = Instant::now();
        let result = host.action("fake", "stamp", None, None);
        let result = tokio::time::timeout(Duration::from_secs(20), result)
            .await
            .unwrap();
        assert_eq!(result, Err("extension fake timed out".to_owned()));
        assert!(started.elapsed() >= ACTION_TIMEOUT);
        // Renders time out after a second each.
        assert_eq!(host.render("keys", LensKind::Tag, "a").await, None);
        assert_eq!(host.render("keys", LensKind::Tag, "b").await, None);
        assert_eq!(
            notices.try_recv().unwrap(),
            "extension fake disabled after 3 failures (timed out)"
        );
        // Disabled: no more requests, and the failed render is remembered.
        assert_eq!(host.render("keys", LensKind::Tag, "c").await, None);
        let result = host.action("fake", "stamp", None, None).await;
        assert_eq!(
            result,
            Err("extension fake: disabled after 3 failures".to_owned())
        );
        assert_eq!(host.failed(), ["fake"]);
        // Turning it off and on again does not undo that.
        host.set_enabled("fake", false).await;
        host.set_enabled("fake", true).await;
        let result = host.action("fake", "stamp", None, None).await;
        assert_eq!(
            result,
            Err("extension fake: disabled after 3 failures".to_owned())
        );
        host.shutdown().await;
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_program_that_never_reads_stdin_does_not_block_quit() {
        let (dir, host, mut notices) = host("exec sleep 30\n");
        // More than a pipe holds, past the in-flight limit: requests that
        // wait for a slot must see the extension disabled meanwhile.
        let renders: Vec<_> = (0..40)
            .map(|i| {
                let source = format!("{i:04}{}", "x".repeat(2044));
                tokio::spawn(host.render("keys", LensKind::Tag, &source))
            })
            .collect();
        let all = futures::future::join_all(renders);
        let rendered = tokio::time::timeout(Duration::from_secs(10), all)
            .await
            .expect("the renders end");
        assert!(rendered.into_iter().all(|r| r.unwrap().is_none()));
        let quit = tokio::time::timeout(Duration::from_secs(5), host.shutdown()).await;
        assert!(quit.is_ok(), "shutdown is bounded");
        assert_eq!(
            notices.try_recv().unwrap(),
            "extension fake disabled after 3 failures (timed out)"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_request_that_cannot_be_written_times_out_and_stops_the_program() {
        let (dir, host, _notices) = host("exec sleep 30\n");
        // Bigger than any pipe: the write cannot finish.
        let source = "x".repeat(1 << 20);
        let started = Instant::now();
        let render = host.render("keys", LensKind::Tag, &source);
        let rendered = tokio::time::timeout(Duration::from_secs(5), render)
            .await
            .expect("the write is bounded");
        assert_eq!(rendered, None);
        assert!(started.elapsed() >= RENDER_TIMEOUT);
        let ext = &host.exts["fake"];
        assert!(ext.proc.lock().await.is_none(), "the program is stopped");
        assert_eq!(*ext.lock_pid(), None);
        assert_eq!(ext.health.lock().unwrap().0.len(), 1, "one failure");
        host.shutdown().await;
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_crash_or_a_bad_line_restarts_the_program() {
        // Each start answers once, then exits (first) or writes garbage.
        let script = "n=$(cat runs 2>/dev/null || echo 0); echo $((n+1)) > runs\n\
                      IFS= read -r line\n\
                      id=$(printf '%s' \"$line\" | sed 's/^{\"id\":\\([0-9]*\\).*/\\1/')\n\
                      printf '{\"id\":%s,\"result\":{\"kind\":\"notice\",\"text\":\"run %s\"}}\\n' \"$id\" \"$((n+1))\"\n\
                      if [ $n -eq 1 ]; then echo 'not json'; fi\n\
                      sleep 1\n";
        let (dir, host, mut notices) = host(script);
        for run in 1..=3 {
            let result = host.action("fake", "a", None, None).await;
            assert_eq!(
                result,
                Ok(ActionResult::Notice {
                    text: format!("run {run}")
                })
            );
            // Let the program exit (or its bad line arrive).
            tokio::time::sleep(Duration::from_millis(1500)).await;
        }
        assert_eq!(
            notices.try_recv().unwrap(),
            "extension fake disabled after 3 failures (exited)"
        );
        host.shutdown().await;
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_disabled_extension_is_not_started_and_disabling_stops_it() {
        let script = format!(
            "echo started >> starts\nsleep 60 &\necho $! > child\n{}",
            answering(
                "case \"$line\" in\n\
                 *render*) printf '{\"id\":%s,\"result\":{\"inline\":\"R\"}}\\n' \"$id\" ;;\n\
                 *) printf '{\"id\":%s,\"result\":{\"kind\":\"notice\",\"text\":\"up\"}}\\n' \"$id\" ;;\n\
                 esac",
            )
        );
        let (dir, program) = program(&script);
        let ext = Extension {
            name: "fake".into(),
            description: String::new(),
            enabled: true,
            lenses: vec![Lens {
                name: "keys".into(),
                kind: LensKind::Tag,
                instruction: String::new(),
            }],
            actions: vec![],
            source: Source::External(program),
        };
        // Off in config.toml: the host knows it, but starts nothing.
        let off = [("fake".to_owned(), false)].into();
        let registry = Registry::new(&off, vec![ext]);
        let (tx, _notices) = mpsc::unbounded_channel();
        let host = ExtensionHost::new(&registry, tx);
        assert_eq!(host.render("keys", LensKind::Tag, "a").await, None);
        let refused = host.action("fake", "a", None, None).await;
        assert_eq!(refused, Err("extension fake: turned off".to_owned()));
        assert!(!dir.join("starts").exists(), "never started");

        // On: it starts on the next request, and a render refused while it
        // was off is not remembered as a failure.
        host.set_enabled("fake", true).await;
        let up = host.action("fake", "a", None, None).await;
        assert_eq!(up, Ok(ActionResult::Notice { text: "up".into() }));
        let rendered = host.render("keys", LensKind::Tag, "a").await;
        assert_eq!(rendered, Some((Some("R".to_owned()), None)));
        let child: i32 = std::fs::read_to_string(dir.join("child"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();

        // Off again: its process group is stopped, without a failure.
        let stopped = tokio::time::timeout(Duration::from_secs(5), host.set_enabled("fake", false));
        stopped.await.expect("stopping is bounded");
        tokio::time::sleep(Duration::from_millis(200)).await;
        // SAFETY: signal 0 only checks that the process exists.
        assert!(unsafe { libc::kill(child, 0) } != 0, "the group is gone");
        let ext = &host.exts["fake"];
        assert!(ext.proc.lock().await.is_none());
        assert!(
            ext.health.lock().unwrap().0.is_empty(),
            "no failure counted"
        );
        assert!(host.failed().is_empty());
        let refused = host.action("fake", "a", None, None).await;
        assert_eq!(refused, Err("extension fake: turned off".to_owned()));
        let starts = std::fs::read_to_string(dir.join("starts")).unwrap();
        assert_eq!(starts.lines().count(), 1, "not started again");
        host.shutdown().await;
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn shutdown_stops_the_whole_process_group() {
        let script =
            format!("sleep 60 &\necho $! > child\n{}", answering(
            "printf '{\"id\":%s,\"result\":{\"kind\":\"notice\",\"text\":\"up\"}}\\n' \"$id\"",
        ));
        let (dir, host, _notices) = host(&script);
        let up = host.action("fake", "a", None, None).await;
        assert_eq!(up, Ok(ActionResult::Notice { text: "up".into() }));
        let child: i32 = std::fs::read_to_string(dir.join("child"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        host.shutdown().await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        // SAFETY: signal 0 only checks that the process exists.
        let alive = unsafe { libc::kill(child, 0) } == 0;
        assert!(!alive, "the program's own child is gone too");
        std::fs::remove_dir_all(&dir).ok();
    }
}
