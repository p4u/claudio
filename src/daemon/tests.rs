//! End-to-end daemon tests: a real daemon in a temp dir, a fake `claude`
//! (a shell script that prints a banner, then `exec cat`), and real Unix
//! socket clients speaking the wire protocol. Every read has a timeout.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use tokio::net::UnixStream;
use uuid::Uuid;

use super::{server, Config};
use crate::proto::{
    self, Envelope, Frame, Hello, Msg, SessionEvent, SessionId, SessionState, SpawnSpec, PROTO,
};

const BANNER: &str = "FAKE-CLAUDE-BANNER";
const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// A variant of the fake claude that exits 1 immediately when given `--resume`
/// (simulating a deleted conversation), and otherwise behaves like the normal
/// fake claude (prints banner + echoes input).
fn fake_claude_resume_fails() -> &'static Path {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!(
            "claudio-fake-claude-rf-{}",
            Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("claude");
        // Exit 1 immediately if --resume is in the arguments, otherwise behave
        // like the normal fake claude.
        let script = format!(
            "#!/bin/sh\n\
             if [ \"$1\" = \"--version\" ]; then echo '9.9.9 (Claude Code)'; exit 0; fi\n\
             # Check all args for --resume\n\
             for arg in \"$@\"; do\n\
               if [ \"$arg\" = \"--resume\" ]; then exit 1; fi\n\
             done\n\
             printf '%s' \"$2\" > settings.json\n\
             echo {BANNER}\n\
             exec cat\n"
        );
        std::fs::write(&bin, script).unwrap();
        std::fs::set_permissions(&bin, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        bin
    })
}

/// The fake claude, shared by all tests (written once, so no exec races with
/// a file still open for writing). It answers `--version`, saves its
/// `--settings` JSON into its cwd (so tests can read the hook token), prints a
/// banner and echoes input.
fn fake_claude() -> &'static Path {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("claudio-fake-claude-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("claude");
        let script = format!(
            "#!/bin/sh\n\
             if [ \"$1\" = \"--version\" ]; then echo '9.9.9 (Claude Code)'; exit 0; fi\n\
             printf '%s' \"$2\" > settings.json\n\
             echo {BANNER}\n\
             exec cat\n"
        );
        std::fs::write(&bin, script).unwrap();
        std::fs::set_permissions(&bin, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        bin
    })
}

/// A daemon running in its own temp dir.
struct TestDaemon {
    dir: PathBuf,
    config: Config,
}

impl TestDaemon {
    fn config(dir: &Path) -> Config {
        Config {
            socket: dir.join("rt/d.sock"),
            lock: dir.join("rt/d.lock"),
            journal: dir.join("state/journal.json"),
            claude: fake_claude().to_path_buf(),
            claudio: PathBuf::from("/bin/true"),
        }
    }

    fn new_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("claudio-daemon-{}", Uuid::new_v4().simple()));
        std::fs::create_dir_all(dir.join("work")).unwrap();
        dir
    }

    async fn start_in(dir: PathBuf) -> TestDaemon {
        let config = Self::config(&dir);
        let listening = server::start(config.clone())
            .unwrap()
            .expect("lock is free");
        tokio::spawn(listening.serve());
        TestDaemon { dir, config }
    }

    async fn start() -> TestDaemon {
        Self::start_in(Self::new_dir()).await
    }

    fn work(&self) -> PathBuf {
        self.dir.join("work")
    }

    async fn client(&self) -> Client {
        Client::connect(&self.config.socket).await
    }

    fn spec(&self, id: SessionId) -> SpawnSpec {
        SpawnSpec {
            id,
            cwd: self.work().to_string_lossy().into_owned(),
            name: Some("test".into()),
            args: vec![],
            env: vec![("SECRET_ENV".into(), "hunter2".into())],
            rows: 24,
            cols: 80,
        }
    }

    /// The hook token the fake claude was started with.
    async fn token(&self) -> String {
        let path = self.work().join("settings.json");
        let settings: serde_json::Value = within(async {
            loop {
                if let Ok(raw) = std::fs::read(&path) {
                    if let Ok(v) = serde_json::from_slice(&raw) {
                        return v;
                    }
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        let cmd = settings["hooks"]["Stop"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        cmd.rsplit(' ').next().unwrap().to_owned()
    }

    fn journal(&self) -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(&self.config.journal).unwrap()).unwrap()
    }
}

impl Drop for TestDaemon {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn within<T>(f: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(READ_TIMEOUT, f)
        .await
        .expect("timed out")
}

/// A protocol client over a real socket.
struct Client {
    stream: UnixStream,
    next_req: u64,
}

impl Client {
    async fn raw(socket: &Path) -> Client {
        Client {
            stream: UnixStream::connect(socket).await.unwrap(),
            next_req: 0,
        }
    }

    async fn connect(socket: &Path) -> Client {
        let mut c = Self::raw(socket).await;
        match c.call(Msg::Hello(hello(PROTO))).await {
            Msg::Welcome(w) => assert_eq!(w.proto, PROTO),
            other => panic!("expected welcome, got {other:?}"),
        }
        c
    }

    async fn send(&mut self, frame: Frame) {
        proto::write_frame(&mut self.stream, &frame).await.unwrap();
    }

    async fn request(&mut self, msg: Msg) -> u64 {
        self.next_req += 1;
        self.send(Frame::Control(Envelope::request(self.next_req, msg)))
            .await;
        self.next_req
    }

    async fn recv(&mut self) -> Option<Frame> {
        within(proto::read_frame(&mut self.stream)).await.unwrap()
    }

    /// Read until `pick` accepts a frame.
    async fn until<T>(&mut self, mut pick: impl FnMut(&Frame) -> Option<T>) -> T {
        loop {
            let frame = self.recv().await.expect("daemon closed the connection");
            if let Some(v) = pick(&frame) {
                return v;
            }
        }
    }

    /// Send a request and wait for its reply.
    async fn call(&mut self, msg: Msg) -> Msg {
        let req = self.request(msg).await;
        self.until(|f| match f {
            Frame::Control(env) if env.req == Some(req) => Some(env.msg.clone()),
            _ => None,
        })
        .await
    }

    /// Wait for a session event matching `pick`.
    async fn event<T>(&mut self, id: SessionId, pick: impl Fn(&SessionEvent) -> Option<T>) -> T {
        self.until(|f| match f {
            Frame::Control(Envelope {
                req: None,
                msg: Msg::Event { id: eid, event },
            }) if *eid == id => pick(event),
            _ => None,
        })
        .await
    }

    /// Accumulate `D` bytes for `id` until they contain `needle`.
    async fn output_until(&mut self, id: SessionId, needle: &str) -> String {
        let mut seen = Vec::new();
        self.until(|f| {
            if let Frame::Data { session, bytes } = f {
                if *session == id {
                    seen.extend_from_slice(bytes);
                }
            }
            String::from_utf8_lossy(&seen)
                .contains(needle)
                .then(|| String::from_utf8_lossy(&seen).into_owned())
        })
        .await
    }

    /// Attach and return the snapshot (the `D` frame right after `Attached`).
    async fn attach(&mut self, id: SessionId, rows: u16, cols: u16) -> String {
        let req = self.request(Msg::Attach { id, rows, cols }).await;
        let attached = self
            .until(|f| match f {
                Frame::Control(env) if env.req == Some(req) => Some(env.msg.clone()),
                _ => None,
            })
            .await;
        assert_eq!(attached, Msg::Attached { id, rows, cols });
        self.until(|f| match f {
            Frame::Data { session, bytes } if *session == id => {
                Some(String::from_utf8_lossy(bytes).into_owned())
            }
            _ => None,
        })
        .await
    }

    /// Attach, then wait until the screen (snapshot or later output) shows
    /// `needle`.
    async fn attach_until(&mut self, id: SessionId, needle: &str) {
        if !self.attach(id, 24, 80).await.contains(needle) {
            self.output_until(id, needle).await;
        }
    }

    async fn spawn(&mut self, spec: SpawnSpec) -> Option<u32> {
        match self.call(Msg::Spawn(spec)).await {
            Msg::Spawned { pid, .. } => pid,
            other => panic!("spawn failed: {other:?}"),
        }
    }

    async fn sessions(&mut self) -> Vec<crate::proto::SessionInfo> {
        match self.call(Msg::ListSessions).await {
            Msg::Sessions { sessions } => sessions,
            other => panic!("expected sessions, got {other:?}"),
        }
    }
}

fn hello(proto: u32) -> Hello {
    Hello {
        claudio_version: "test".into(),
        proto,
        colors: None,
    }
}

async fn send_hook(socket: &Path, token: &str, event: &str, payload: serde_json::Value) {
    let mut c = Client::raw(socket).await;
    let hook = Msg::Hook {
        token: token.into(),
        event: event.into(),
        payload,
    };
    c.send(Frame::Control(Envelope::event(hook))).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handshake_reports_host_and_rejects_wrong_proto() {
    let d = TestDaemon::start().await;

    let mut c = Client::raw(&d.config.socket).await;
    match c.call(Msg::Hello(hello(PROTO))).await {
        Msg::Welcome(w) => {
            assert_eq!(w.claudio_version, env!("CARGO_PKG_VERSION"));
            let claude = w.host.claude.expect("fake claude found");
            assert_eq!(claude.version, "9.9.9 (Claude Code)");
            assert_eq!(w.host.os, std::env::consts::OS);
        }
        other => panic!("expected welcome, got {other:?}"),
    }

    let mut bad = Client::raw(&d.config.socket).await;
    assert!(matches!(
        bad.call(Msg::Hello(hello(PROTO + 1))).await,
        Msg::Error { .. }
    ));
    assert_eq!(bad.recv().await, None, "connection closed after refusal");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn second_daemon_backs_off() {
    let d = TestDaemon::start().await;
    assert!(server::start(d.config.clone()).unwrap().is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_attach_echo_and_list() {
    let d = TestDaemon::start().await;
    let mut a = d.client().await;
    let mut observer = d.client().await;
    let id = Uuid::new_v4();

    // Created reaches the spawning client before its Spawned reply.
    let req = a.request(Msg::Spawn(d.spec(id))).await;
    let created = a
        .until(|f| match f {
            Frame::Control(env) if env.req == Some(req) => {
                panic!("Spawned before Created: {env:?}")
            }
            Frame::Control(Envelope {
                msg:
                    Msg::Event {
                        event: SessionEvent::Created { info },
                        ..
                    },
                ..
            }) => Some(info.clone()),
            _ => None,
        })
        .await;
    assert_eq!(created.id, id);
    assert_eq!(created.state, SessionState::Starting);
    let pid = a
        .until(|f| match f {
            Frame::Control(Envelope {
                req: Some(r),
                msg: Msg::Spawned { pid, .. },
            }) if *r == req => Some(*pid),
            _ => None,
        })
        .await;
    assert!(pid.is_some());
    assert_eq!(created.pid, pid);
    // ...and every other client.
    observer
        .event(id, |e| {
            matches!(e, SessionEvent::Created { .. }).then_some(())
        })
        .await;

    // Spawn is idempotent for a live id.
    assert_eq!(a.spawn(d.spec(id)).await, pid);

    // Live output, then a later attach sees the banner in its snapshot.
    a.attach_until(id, BANNER).await;
    let snapshot = observer.attach(id, 24, 80).await;
    assert!(snapshot.contains(BANNER), "snapshot: {snapshot:?}");

    // Keyboard input echoes back.
    a.send(Frame::Data {
        session: id,
        bytes: b"hello\r".to_vec(),
    })
    .await;
    a.output_until(id, "hello").await;

    let sessions = a.sessions().await;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, id);
    assert_eq!(sessions[0].pid, pid);
    assert_eq!(sessions[0].name.as_deref(), Some("test"));
    assert_eq!(sessions[0].state, SessionState::Starting);

    // The journal never holds the env.
    let raw = std::fs::read_to_string(&d.config.journal).unwrap();
    assert!(!raw.contains("hunter2") && !raw.contains("SECRET_ENV") && !raw.contains("--settings"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hooks_update_state_and_journal() {
    let d = TestDaemon::start().await;
    let mut c = d.client().await;
    let id = Uuid::new_v4();
    c.spawn(d.spec(id)).await;
    let token = d.token().await;

    // A forged token is ignored: had this Stop applied, Idle would come first.
    send_hook(&d.config.socket, "0000", "Stop", serde_json::json!({})).await;
    send_hook(
        &d.config.socket,
        &token,
        "UserPromptSubmit",
        serde_json::json!({"prompt": "x"}),
    )
    .await;
    let state = c
        .event(id, |e| match e {
            SessionEvent::State { state } => Some(*state),
            _ => None,
        })
        .await;
    assert_eq!(state, SessionState::Working);

    // A new claude conversation id is journaled immediately.
    let payload = serde_json::json!({"session_id": "conv-42", "source": "clear"});
    send_hook(&d.config.socket, &token, "SessionStart", payload).await;
    c.event(id, |e| match e {
        SessionEvent::ClaudeSession { claude_session_id } => {
            Some(assert_eq!(claude_session_id, "conv-42"))
        }
        _ => None,
    })
    .await;
    assert_eq!(d.journal()["sessions"][0]["claude_session_id"], "conv-42");
    assert_eq!(c.sessions().await[0].state, SessionState::NeedsInput);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kill_removes_session_everywhere() {
    let d = TestDaemon::start().await;
    let mut c = d.client().await;
    let id = Uuid::new_v4();
    c.spawn(d.spec(id)).await;
    assert_eq!(d.journal()["sessions"].as_array().unwrap().len(), 1);

    // Removed is broadcast before the Ok reply.
    let req = c.request(Msg::Kill { id }).await;
    c.event(id, |e| matches!(e, SessionEvent::Removed).then_some(()))
        .await;
    let ok = c
        .until(|f| match f {
            Frame::Control(env) if env.req == Some(req) => Some(env.msg.clone()),
            _ => None,
        })
        .await;
    assert_eq!(ok, Msg::Ok);
    assert!(d.journal()["sessions"].as_array().unwrap().is_empty());
    assert!(c.sessions().await.is_empty());
    assert!(matches!(c.call(Msg::Kill { id }).await, Msg::Error { .. }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exit_leaves_a_dormant_session() {
    let d = TestDaemon::start().await;
    let mut c = d.client().await;
    let id = Uuid::new_v4();
    c.spawn(d.spec(id)).await;
    c.attach_until(id, BANNER).await;

    // ^D ends `cat`.
    c.send(Frame::Data {
        session: id,
        bytes: vec![4],
    })
    .await;
    let code = c
        .event(id, |e| match e {
            SessionEvent::Exited { code } => Some(*code),
            _ => None,
        })
        .await;
    assert_eq!(code, Some(0));

    let sessions = c.sessions().await;
    assert_eq!(sessions[0].pid, None);
    assert_eq!(sessions[0].state, SessionState::Exited);
    assert!(matches!(
        c.call(Msg::Attach {
            id,
            rows: 24,
            cols: 80
        })
        .await,
        Msg::Error { .. }
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn journal_reloads_as_dormant() {
    let dir = TestDaemon::new_dir();
    let id = Uuid::new_v4();
    let journal = serde_json::json!({"sessions": [{
        "id": id, "cwd": dir.join("work"), "name": "old", "args": ["--resume", "c1"],
        "claude_session_id": "c1", "created_at": 1700000000u64,
    }]});
    let path = TestDaemon::config(&dir).journal;
    crate::paths::write_atomic(&path, journal.to_string().as_bytes()).unwrap();

    let d = TestDaemon::start_in(dir).await;
    let mut c = d.client().await;
    let sessions = c.sessions().await;
    assert_eq!(sessions.len(), 1);
    let s = &sessions[0];
    assert_eq!((s.id, s.pid, s.state), (id, None, SessionState::Exited));
    assert_eq!(s.claude_session_id.as_deref(), Some("c1"));
    assert_eq!(s.created_at, 1700000000);

    // Re-spawning the dormant entry starts it and keeps its history.
    let pid = c.spawn(d.spec(id)).await;
    assert!(pid.is_some());
    let s = &c.sessions().await[0];
    assert_eq!((s.pid, s.created_at), (pid, 1700000000));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resize_resyncs_other_subscribers() {
    let d = TestDaemon::start().await;
    let mut a = d.client().await;
    let mut b = d.client().await;
    let id = Uuid::new_v4();
    a.spawn(d.spec(id)).await;
    a.attach(id, 24, 80).await;
    b.attach(id, 24, 80).await;

    assert_eq!(
        a.call(Msg::Resize {
            id,
            rows: 30,
            cols: 100
        })
        .await,
        Msg::Ok
    );
    let size = b
        .until(|f| match f {
            Frame::Control(Envelope {
                req: None,
                msg: Msg::Attached { id: i, rows, cols },
            }) if *i == id => Some((*rows, *cols)),
            _ => None,
        })
        .await;
    assert_eq!(size, (30, 100));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_dir_sorts_dirs_first() {
    let d = TestDaemon::start().await;
    let root = d.dir.join("browse");
    std::fs::create_dir_all(root.join("zdir")).unwrap();
    std::fs::create_dir_all(root.join("bdir")).unwrap();
    std::fs::write(root.join("afile"), b"").unwrap();
    let mut c = d.client().await;

    let path = root.to_string_lossy().into_owned();
    match c.call(Msg::ListDir { path: path.clone() }).await {
        Msg::DirEntries {
            path: p, entries, ..
        } => {
            assert_eq!(p, path);
            let got: Vec<(&str, bool)> = entries.iter().map(|e| (e.name.as_str(), e.dir)).collect();
            assert_eq!(got, [("bdir", true), ("zdir", true), ("afile", false)]);
        }
        other => panic!("expected entries, got {other:?}"),
    }
    let missing = Msg::ListDir {
        path: format!("{path}/nope"),
    };
    assert!(matches!(c.call(missing).await, Msg::Error { .. }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bad_requests_get_errors_and_keep_the_connection() {
    let d = TestDaemon::start().await;
    let mut c = d.client().await;

    // An op this daemon doesn't know (e.g. from a newer client).
    let body = br#"J{"req":77,"op":"teleport"}"#;
    let mut frame = (body.len() as u32).to_be_bytes().to_vec();
    frame.extend_from_slice(body);
    tokio::io::AsyncWriteExt::write_all(&mut c.stream, &frame)
        .await
        .unwrap();
    let reply = c
        .until(|f| match f {
            Frame::Control(env) if env.req == Some(77) => Some(env.msg.clone()),
            _ => None,
        })
        .await;
    assert!(matches!(reply, Msg::Error { .. }));

    // A known op the daemon doesn't serve.
    assert!(matches!(c.call(Msg::Pong).await, Msg::Error { .. }));
    // Spawning into a missing directory fails cleanly.
    let mut spec = d.spec(Uuid::new_v4());
    spec.cwd = d.dir.join("missing").to_string_lossy().into_owned();
    assert!(matches!(c.call(Msg::Spawn(spec)).await, Msg::Error { .. }));
    assert_eq!(c.call(Msg::Ping).await, Msg::Pong);
}

/// When a session is spawned with `--resume` and claude exits immediately with
/// a non-zero code before any SessionStart hook fires, the daemon should:
/// 1. Broadcast a `Notice` event (not `Title`) with a descriptive message.
/// 2. Spawn a fresh session (without `--resume`), which becomes live.
/// 3. `ListSessions` shows one live session with a NEW pid.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resume_retry_spawns_fresh_on_quick_exit() {
    // Use a daemon backed by the fake claude that fails on --resume.
    let dir = TestDaemon::new_dir();
    let config = Config {
        socket: dir.join("rt/d.sock"),
        lock: dir.join("rt/d.lock"),
        journal: dir.join("state/journal.json"),
        claude: fake_claude_resume_fails().to_path_buf(),
        claudio: PathBuf::from("/bin/true"),
    };
    // Pre-populate a journal entry with a claude_session_id so the spawn
    // request carries --resume.
    let journal_path = &config.journal;
    std::fs::create_dir_all(journal_path.parent().unwrap()).unwrap();
    let id = Uuid::new_v4();
    let journal_entry = serde_json::json!({
        "sessions": [{
            "id": id,
            "cwd": dir.join("work"),
            "name": "resume-test",
            "args": [],
            "claude_session_id": "old-conv-id",
            "created_at": 1700000000u64
        }]
    });
    crate::paths::write_atomic(journal_path, journal_entry.to_string().as_bytes()).unwrap();

    let listening = server::start(config).unwrap().expect("lock is free");
    tokio::spawn(listening.serve());
    let socket = dir.join("rt/d.sock");
    // Wait for daemon to start.
    within(async {
        loop {
            if tokio::net::UnixStream::connect(&socket).await.is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;

    let mut c = Client::connect(&socket).await;

    // Spawn with --resume (the journal has a claude_session_id).
    let spec = SpawnSpec {
        id,
        cwd: dir.join("work").to_string_lossy().into_owned(),
        name: Some("resume-test".into()),
        args: vec!["--resume".into(), "old-conv-id".into()],
        env: vec![],
        rows: 24,
        cols: 80,
    };
    let initial_pid = c.spawn(spec).await;

    // Wait for the Notice event (resume failed → retry fresh).
    let notice_text = c
        .event(id, |e| match e {
            SessionEvent::Notice { text } => Some(text.clone()),
            _ => None,
        })
        .await;
    assert!(
        notice_text.contains("conversation not found"),
        "expected conversation-not-found notice, got: {notice_text:?}"
    );

    // Wait for the new fresh session's Created event (different pid).
    let new_info = c
        .event(id, |e| match e {
            SessionEvent::Created { info } => Some(info.clone()),
            _ => None,
        })
        .await;
    assert!(
        new_info.pid != initial_pid || new_info.pid.is_none(),
        "fresh spawn should have a different pid; got {:?} vs initial {:?}",
        new_info.pid,
        initial_pid
    );
    assert_eq!(new_info.state, proto::SessionState::Starting);

    // ListSessions must show exactly one live session.
    let sessions = c.sessions().await;
    assert_eq!(sessions.len(), 1, "should have exactly one session");
    // The session must be live (Starting state from the fresh spawn).
    assert!(
        sessions[0].pid.is_some(),
        "fresh session should be live (pid != None)"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
