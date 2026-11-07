//! End-to-end daemon tests: a real daemon in a temp dir, a fake `claude`
//! (a shell script that prints a banner, then `exec cat`), and real Unix
//! socket clients speaking the wire protocol. Every read has a timeout.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tokio::net::UnixStream;
use uuid::Uuid;

use super::{server, Config, Daemon};
use crate::proto::{
    self, Envelope, Frame, Hello, Msg, SessionEvent, SessionId, SessionState, SpawnSpec, PROTO,
};

const BANNER: &str = "FAKE-CLAUDE-BANNER";
const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// A variant of the fake claude that exits 3 immediately on any real invocation.
/// Used to trigger the commit-before-exit race deterministically.
fn fake_claude_exits_immediately() -> &'static Path {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("claudio-fake-claude-ei-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("claude");
        let script = "#!/bin/sh\n\
             if [ \"$1\" = \"--version\" ]; then echo '9.9.9 (Claude Code)'; exit 0; fi\n\
             if [ \"$1\" = \"--help\" ]; then exit 0; fi\n\
             exit 3\n";
        std::fs::write(&bin, script).unwrap();
        std::fs::set_permissions(&bin, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        bin
    })
}

/// A variant of the fake claude that exits 1 immediately when given `--resume`
/// (simulating a deleted conversation), and otherwise behaves like the normal
/// fake claude (prints banner + echoes input).
fn fake_claude_resume_fails() -> &'static Path {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("claudio-fake-claude-rf-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("claude");
        // Exit 1 immediately if --resume is in the arguments, otherwise behave
        // like the normal fake claude.
        let script = format!(
            "#!/bin/sh\n\
             if [ \"$1\" = \"--version\" ]; then echo '9.9.9 (Claude Code)'; exit 0; fi\n\
             if [ \"$1\" = \"--help\" ]; then exit 0; fi\n\
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
             if [ \"$1\" = \"--help\" ]; then exit 0; fi\n\
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
    daemon: Arc<Daemon>,
}

impl TestDaemon {
    fn config(dir: &Path) -> Config {
        Config {
            socket: dir.join("rt/d.sock"),
            lock: dir.join("rt/d.lock"),
            journal: dir.join("state/journal.json"),
            claude: fake_claude().to_path_buf(),
            claudio: PathBuf::from("/bin/true"),
            skip_permissions: true,
            latex: false,
            build: None,
            path: None,
        }
    }

    fn new_dir() -> PathBuf {
        let dir = crate::paths::socket_test_dir("cl-d");
        std::fs::create_dir_all(dir.join("work")).unwrap();
        dir
    }

    async fn start_in(dir: PathBuf) -> TestDaemon {
        let config = Self::config(&dir);
        Self::start_with_config(dir, config)
    }

    fn start_with_config(dir: PathBuf, config: Config) -> TestDaemon {
        let listening = server::start(config.clone())
            .unwrap()
            .expect("lock is free");
        let daemon = listening.daemon();
        tokio::spawn(listening.serve());
        TestDaemon {
            dir,
            config,
            daemon,
        }
    }

    async fn start() -> TestDaemon {
        Self::start_in(Self::new_dir()).await
    }

    /// A daemon whose claude is `claude` instead of the shared fake.
    async fn start_with_claude(dir: PathBuf, claude: PathBuf) -> TestDaemon {
        let config = Config {
            claude,
            ..Self::config(&dir)
        };
        Self::start_with_config(dir, config)
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
            ephemeral: false,
            extensions: None,
        }
    }

    /// The hook token the fake claude was started with.
    async fn token(&self) -> String {
        self.token_other_than("").await
    }

    /// The hook token of a process started after the one with `previous`:
    /// after a respawn, `settings.json` holds the old token until the new
    /// process rewrites it.
    async fn token_other_than(&self, previous: &str) -> String {
        let path = self.work().join("settings.json");
        within(async {
            loop {
                let token = std::fs::read(&path)
                    .ok()
                    .and_then(|raw| serde_json::from_slice::<serde_json::Value>(&raw).ok())
                    .and_then(|settings| {
                        let cmd = settings["hooks"]["Stop"][0]["hooks"][0]["command"].as_str()?;
                        Some(cmd.rsplit(' ').next()?.to_owned())
                    });
                match token {
                    Some(token) if token != previous => return token,
                    _ => tokio::time::sleep(Duration::from_millis(20)).await,
                }
            }
        })
        .await
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

    /// Read until `pick` accepts a frame. On a timeout, the panic lists the
    /// last frames that did arrive, so a failure says what was seen instead.
    async fn until<T>(&mut self, mut pick: impl FnMut(&Frame) -> Option<T>) -> T {
        let mut seen: std::collections::VecDeque<String> = Default::default();
        let wait = async {
            loop {
                let frame = proto::read_frame(&mut self.stream)
                    .await
                    .unwrap()
                    .expect("daemon closed the connection");
                if let Some(v) = pick(&frame) {
                    return v;
                }
                if seen.len() == 12 {
                    seen.pop_front();
                }
                seen.push_back(match &frame {
                    Frame::Control(env) => format!("{:?}", env.msg).chars().take(160).collect(),
                    Frame::Data { bytes, .. } => format!("{} data bytes", bytes.len()),
                });
            }
        };
        match tokio::time::timeout(READ_TIMEOUT, wait).await {
            Ok(v) => v,
            Err(_) => panic!("timed out; last frames seen:\n{}", Vec::from(seen).join("\n")),
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
    // Stay connected until the daemon hangs up, as the real relay does.
    let _ = tokio::time::timeout(READ_TIMEOUT, proto::read_frame(&mut c.stream)).await;
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
async fn an_ephemeral_session_is_listed_as_such_and_never_journaled() {
    let d = TestDaemon::start().await;
    let mut c = d.client().await;
    let (kept, plain) = (Uuid::new_v4(), Uuid::new_v4());
    c.spawn(d.spec(kept)).await;
    let ephemeral = SpawnSpec {
        ephemeral: true,
        ..d.spec(plain)
    };
    assert!(c.spawn(ephemeral).await.is_some());

    let sessions = c.sessions().await;
    let flag = |id| sessions.iter().find(|s| s.id == id).map(|s| s.ephemeral);
    assert_eq!((flag(kept), flag(plain)), (Some(false), Some(true)));
    let journaled: Vec<_> = d.journal()["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(journaled, [kept.to_string()]);
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

/// `Answers` is served off the client loop: no conversation yet (or no
/// transcript) is an empty list, an unknown session an error.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn answers_are_empty_without_a_transcript_and_refused_for_unknown_sessions() {
    let d = TestDaemon::start().await;
    let mut c = d.client().await;
    let id = Uuid::new_v4();
    c.spawn(d.spec(id)).await;
    let empty = Msg::AnswersReply {
        id,
        answers: Vec::new(),
    };
    assert_eq!(c.call(Msg::Answers { id }).await, empty);
    // A conversation whose transcript does not exist (the test's work dir
    // has none) has no answers either.
    let payload = serde_json::json!({
        "session_id": "0a1b2c3d-0000-4000-8000-000000000000",
        "source": "startup",
    });
    send_hook(&d.config.socket, &d.token().await, "SessionStart", payload).await;
    c.event(id, |e| {
        matches!(e, SessionEvent::ClaudeSession { .. }).then_some(())
    })
    .await;
    assert_eq!(c.call(Msg::Answers { id }).await, empty);
    let unknown = Uuid::new_v4();
    match c.call(Msg::Answers { id: unknown }).await {
        Msg::Error { message } => assert!(message.contains("no such session"), "{message}"),
        other => panic!("expected an error, got {other:?}"),
    }
}

/// `LastPrompt` is served like `Answers`: no conversation (or no
/// transcript) has no prompt, an unknown session is an error.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn last_prompt_is_none_without_a_transcript_and_refused_for_unknown_sessions() {
    let d = TestDaemon::start().await;
    let mut c = d.client().await;
    let id = Uuid::new_v4();
    c.spawn(d.spec(id)).await;
    let none = Msg::LastPromptReply {
        id,
        prompt: None,
        cut: false,
    };
    assert_eq!(c.call(Msg::LastPrompt { id }).await, none);
    let payload = serde_json::json!({
        "session_id": "0a1b2c3d-0000-4000-8000-000000000001",
        "source": "startup",
    });
    send_hook(&d.config.socket, &d.token().await, "SessionStart", payload).await;
    c.event(id, |e| {
        matches!(e, SessionEvent::ClaudeSession { .. }).then_some(())
    })
    .await;
    assert_eq!(c.call(Msg::LastPrompt { id }).await, none);
    let unknown = Uuid::new_v4();
    match c.call(Msg::LastPrompt { id: unknown }).await {
        Msg::Error { message } => assert!(message.contains("no such session"), "{message}"),
        other => panic!("expected an error, got {other:?}"),
    }
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
async fn clean_exit_closes_the_session() {
    let d = TestDaemon::start().await;
    let mut c = d.client().await;
    let id = Uuid::new_v4();
    c.spawn(d.spec(id)).await;
    c.attach_until(id, BANNER).await;

    // ^D ends `cat` with status 0, like Ctrl+D twice or `/exit` in claude.
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
    c.event(id, |e| matches!(e, SessionEvent::Removed).then_some(()))
        .await;

    assert!(c.sessions().await.is_empty());
    assert!(d.journal()["sessions"].as_array().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crash_leaves_a_dormant_session() {
    let d = TestDaemon::start().await;
    let mut c = d.client().await;
    let id = Uuid::new_v4();
    c.spawn(d.spec(id)).await;
    c.attach_until(id, BANNER).await;

    // ^C kills `cat` with SIGINT.
    c.send(Frame::Data {
        session: id,
        bytes: vec![3],
    })
    .await;
    let code = c
        .event(id, |e| match e {
            SessionEvent::Exited { code } => Some(*code),
            _ => None,
        })
        .await;
    assert_ne!(code, Some(0));

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

/// `Search` runs off the client loop over the daemon's projects root, one
/// at a time; a bad query and a second search are refused at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn search_finds_conversations_one_search_at_a_time() {
    let d = TestDaemon::start().await;
    let project = d.dir.join("projects/-w");
    std::fs::create_dir_all(&project).unwrap();
    let record = serde_json::json!({"type": "user", "cwd": "/w",
        "message": {"role": "user", "content": "find the Needle"}});
    std::fs::write(project.join("c1.jsonl"), format!("{record}\n")).unwrap();
    d.daemon.search.set_root(d.dir.join("projects"));
    let mut c = d.client().await;
    let ask = |query: &str| Msg::Search {
        query: query.into(),
        limit: 0,
    };
    match c.call(ask("needle")).await {
        Msg::SearchReply {
            query,
            hits,
            scanned,
            total,
            truncated,
        } => {
            assert_eq!(
                (query.as_str(), scanned, total, truncated),
                ("needle", 1, 1, false)
            );
            assert_eq!(hits.len(), 1);
            assert_eq!(
                (hits[0].conversation.as_str(), hits[0].cwd.as_str()),
                ("c1", "/w")
            );
            assert_eq!(hits[0].snippet, "find the Needle");
        }
        other => panic!("expected a reply, got {other:?}"),
    }
    match c.call(ask(" x ")).await {
        Msg::Error { message } => assert_eq!(message, "bad query: too short"),
        other => panic!("expected an error, got {other:?}"),
    }
    let running = d.daemon.search.hold();
    match c.call(ask("needle")).await {
        Msg::Error { message } => assert_eq!(message, "search busy"),
        other => panic!("expected busy, got {other:?}"),
    }
    drop(running);
    assert!(matches!(
        c.call(ask("needle")).await,
        Msg::SearchReply { .. }
    ));
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
        skip_permissions: true,
        latex: false,
        build: None,
        path: None,
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

    // Use TWO clients: `observer` subscribes first and receives every broadcast
    // independently; `spawner` sends the Spawn request and waits for Spawned.
    // This avoids a race where the actor broadcasts `Notice` (and the retry
    // `Created`) concurrently with the daemon sending the `Spawned` reply, so
    // `spawner.spawn`'s internal `until` loop might consume the Notice frame
    // before the test gets to read it.
    let mut observer = Client::connect(&socket).await;
    let mut spawner = Client::connect(&socket).await;

    let spec = SpawnSpec {
        id,
        cwd: dir.join("work").to_string_lossy().into_owned(),
        name: Some("resume-test".into()),
        args: vec!["--resume".into(), "old-conv-id".into()],
        env: vec![],
        rows: 24,
        cols: 80,
        ephemeral: false,
        extensions: None,
    };

    // spawner handles the Spawn/Spawned handshake; observer watches events.
    let initial_pid = spawner.spawn(spec).await;

    // Wait for the Notice event (resume failed → retry fresh).
    let notice_text = observer
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
    // Skip the initial Created (if observer catches it before Spawned arrives
    // on the spawner) by waiting for any Created that appears after the Notice.
    let new_info = observer
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

    // ListSessions must show exactly one live session (the fresh one).
    let sessions = spawner.sessions().await;
    assert_eq!(sessions.len(), 1, "should have exactly one session");
    assert!(
        sessions[0].pid.is_some(),
        "fresh session should be live (pid != None)"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A child that crashes immediately (exit 3, no --resume) must leave the session
/// dormant (pid=None) and must not strand a stale live handle for a dead pid.
///
/// This is a direct regression test for the commit-before-exit race: the fake
/// claude exits before `Daemon::spawn` Phase 3 has a chance to commit the
/// handle to `registry.live`. The §Race fix (committed oneshot signal) makes
/// the actor wait for the commit before running on_exit.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn immediate_exit_leaves_session_dormant() {
    let dir = TestDaemon::new_dir();
    let config = Config {
        socket: dir.join("rt/d.sock"),
        lock: dir.join("rt/d.lock"),
        journal: dir.join("state/journal.json"),
        claude: fake_claude_exits_immediately().to_path_buf(),
        claudio: PathBuf::from("/bin/true"),
        skip_permissions: true,
        latex: false,
        build: None,
        path: None,
    };

    let listening = server::start(config).unwrap().expect("lock is free");
    tokio::spawn(listening.serve());
    let socket = dir.join("rt/d.sock");
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
    let id = Uuid::new_v4();
    let spec = SpawnSpec {
        id,
        cwd: dir.join("work").to_string_lossy().into_owned(),
        name: Some("immediate-exit".into()),
        args: vec![],
        env: vec![],
        rows: 24,
        cols: 80,
        ephemeral: false,
        extensions: None,
    };
    c.spawn(spec).await;

    // Session must become dormant (pid == None) — not stranded as "live" with
    // a dead pid. We poll `sessions()` rather than waiting for the `Exited`
    // event, because the event may have been consumed by `c.spawn`'s internal
    // frame-draining loop (it arrives between `Created` and `Spawned`).
    within(async {
        loop {
            let sessions = c.sessions().await;
            let s = sessions
                .iter()
                .find(|s| s.id == id)
                .expect("session in list");
            if s.pid.is_none() {
                return; // dormant — correct
            }
            // Still reports a live pid — wait and retry.
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;

    let _ = std::fs::remove_dir_all(&dir);
}

/// Kill a session while its spawn is still in-flight (i.e. before the daemon
/// has committed the actor to the live registry). The kill must succeed (Ok)
/// rather than returning "no such session", and the session must not appear
/// live afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kill_in_flight_session_succeeds() {
    let d = TestDaemon::start().await;
    let mut c = d.client().await;
    let id = Uuid::new_v4();

    // Start the spawn and immediately issue a Kill — both are sent over c.
    // We accept that the spawn may or may not have committed by the time Kill
    // arrives; what matters is that Kill returns Ok in both cases.
    c.request(Msg::Spawn(d.spec(id))).await;
    let kill_result = c.call(Msg::Kill { id }).await;

    // Kill must succeed (Ok), not "no such session", regardless of timing.
    assert_eq!(
        kill_result,
        Msg::Ok,
        "Kill of in-flight (or just-committed) session must return Ok; got {kill_result:?}"
    );

    // After all events settle, the session must not appear live in ListSessions.
    // We poll until settled or timeout.
    within(async {
        loop {
            let sessions = c.sessions().await;
            let found = sessions.iter().find(|s| s.id == id);
            match found {
                // Session is gone — success.
                None => return,
                // Session is dormant/exited — acceptable (kill happened, actor finished).
                Some(s) if s.pid.is_none() => return,
                // Still live — wait a bit and retry.
                Some(_) => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
    })
    .await;
}

/// A fake claude that reports the version in its `version` file and, on
/// `update`, either bumps it to 2.0.0 or (with a `fail` file) exits 1.
fn fake_claude_updatable(dir: &Path) -> PathBuf {
    let bin = dir.join("claude");
    std::fs::write(dir.join("version"), "1.0.0 (Claude Code)\n").unwrap();
    let script = "#!/bin/sh\n\
         D=$(dirname \"$0\")\n\
         if [ \"$1\" = \"--version\" ]; then cat \"$D/version\"; exit 0; fi\n\
         if [ \"$1\" = \"update\" ]; then\n\
           echo \"Checking for updates\"\n\
           echo \"permission note\" >&2\n\
           if [ -e \"$D/fail\" ]; then exit 1; fi\n\
           echo \"2.0.0 (Claude Code)\" > \"$D/version\"\n\
           echo \"Updated to 2.0.0\"; exit 0\n\
         fi\n\
         exit 3\n";
    std::fs::write(&bin, script).unwrap();
    std::fs::set_permissions(&bin, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    bin
}

async fn welcome_claude_version(d: &TestDaemon) -> Option<String> {
    let mut c = Client::raw(&d.config.socket).await;
    match c.call(Msg::Hello(hello(PROTO))).await {
        Msg::Welcome(w) => w.host.claude.map(|c| c.version),
        other => panic!("expected welcome, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_claude_runs_claude_update_and_refreshes_the_host() {
    let dir = TestDaemon::new_dir();
    let claude = fake_claude_updatable(&dir);
    let d = TestDaemon::start_with_claude(dir.clone(), claude).await;
    assert_eq!(
        welcome_claude_version(&d).await.as_deref(),
        Some("1.0.0 (Claude Code)")
    );

    let mut c = d.client().await;
    match c.call(Msg::UpdateClaude { install: false }).await {
        Msg::ClaudeUpdated { version, ok, tail } => {
            assert!(ok, "update failed: {tail}");
            assert_eq!(version.as_deref(), Some("2.0.0 (Claude Code)"));
            assert!(tail.contains("Checking for updates"), "stdout in tail: {tail:?}");
            assert!(tail.contains("permission note"), "stderr in tail: {tail:?}");
            assert!(tail.ends_with("Updated to 2.0.0"), "tail is trimmed: {tail:?}");
        }
        other => panic!("expected ClaudeUpdated, got {other:?}"),
    }
    // The next Welcome reports the refreshed version, and the connection that
    // asked kept working meanwhile.
    assert_eq!(
        welcome_claude_version(&d).await.as_deref(),
        Some("2.0.0 (Claude Code)")
    );
    assert_eq!(c.call(Msg::Ping).await, Msg::Pong);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_update_reports_ok_false_beside_other_requests() {
    let dir = TestDaemon::new_dir();
    let claude = fake_claude_updatable(&dir);
    std::fs::write(dir.join("fail"), "").unwrap();
    let d = TestDaemon::start_with_claude(dir.clone(), claude).await;

    let mut c = d.client().await;
    // The reply comes from a task: a Ping sent right behind it is answered
    // whichever finishes first, and both arrive.
    let update = c.request(Msg::UpdateClaude { install: false }).await;
    let ping = c.request(Msg::Ping).await;
    let (mut updated, mut ponged) = (None, false);
    c.until(|f| {
        if let Frame::Control(env) = f {
            if env.req == Some(update) {
                updated = Some(env.msg.clone());
            } else if env.req == Some(ping) {
                ponged = env.msg == Msg::Pong;
            }
        }
        (updated.is_some() && ponged).then_some(())
    })
    .await;
    match updated.unwrap() {
        Msg::ClaudeUpdated { version, ok, tail } => {
            assert!(!ok);
            assert_eq!(version.as_deref(), Some("1.0.0 (Claude Code)"));
            assert!(tail.contains("permission note"));
        }
        other => panic!("expected ClaudeUpdated, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_with_a_missing_claude_reports_failure() {
    let dir = TestDaemon::new_dir();
    let d = TestDaemon::start_with_claude(dir.clone(), dir.join("nope/claude")).await;
    let mut c = d.client().await;
    match c.call(Msg::UpdateClaude { install: false }).await {
        Msg::ClaudeUpdated { version, ok, tail } => {
            assert!(!ok && version.is_none(), "{tail}");
        }
        other => panic!("expected ClaudeUpdated, got {other:?}"),
    }
}

// ── Terminal tabs (SessionKind::Shell) ───────────────────────────────────────

impl TestDaemon {
    /// `SpawnShell` for `id` in the work dir. The shell is pinned to `/bin/sh`
    /// so the tests do not depend on the user's login shell and rc files.
    fn shell_spec(&self, id: SessionId) -> proto::ShellSpec {
        std::env::set_var("SHELL", "/bin/sh");
        proto::ShellSpec {
            id,
            cwd: self.work().to_string_lossy().into_owned(),
            name: None,
            rows: 24,
            cols: 80,
        }
    }
}

impl Client {
    async fn spawn_shell(&mut self, spec: proto::ShellSpec) -> Option<u32> {
        match self.call(Msg::SpawnShell(spec)).await {
            Msg::Spawned { pid, .. } => pid,
            other => panic!("spawn_shell failed: {other:?}"),
        }
    }

    async fn type_line(&mut self, id: SessionId, line: &str) {
        self.send(Frame::Data {
            session: id,
            bytes: format!("{line}\r").into_bytes(),
        })
        .await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_spawn_runs_a_shell_and_echoes() {
    let d = TestDaemon::start().await;
    let mut c = d.client().await;
    let id = Uuid::new_v4();

    let req = c.request(Msg::SpawnShell(d.shell_spec(id))).await;
    let created = c
        .until(|f| match f {
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
    assert_eq!(created.kind, proto::SessionKind::Shell);
    assert_eq!(created.state, SessionState::Idle, "shells never wait on hooks");
    c.until(|f| matches!(f, Frame::Control(env) if env.req == Some(req)).then_some(()))
        .await;

    c.attach(id, 24, 80).await;
    c.type_line(id, "echo hi-$((20 + 22))").await;
    c.output_until(id, "hi-42").await;

    // Not claude: the fake would have written its --settings here.
    assert!(!d.work().join("settings.json").exists());
    let journal = d.journal();
    let entry = &journal["sessions"][0];
    assert_eq!(entry["kind"], "shell");
    assert_eq!(entry["args"], serde_json::json!([]));
    assert_eq!(c.sessions().await[0].kind, proto::SessionKind::Shell);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_closes_on_any_exit_status() {
    let d = TestDaemon::start().await;
    let mut c = d.client().await;
    let id = Uuid::new_v4();
    c.spawn_shell(d.shell_spec(id)).await;
    c.attach(id, 24, 80).await;

    // A claude session would stay dormant after a status-1 exit.
    c.type_line(id, "exit 1").await;
    let code = c
        .event(id, |e| match e {
            SessionEvent::Exited { code } => Some(*code),
            _ => None,
        })
        .await;
    assert_eq!(code, Some(1));
    c.event(id, |e| matches!(e, SessionEvent::Removed).then_some(()))
        .await;
    assert!(c.sessions().await.is_empty());
    assert!(d.journal()["sessions"].as_array().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_for_a_journaled_shell_starts_a_shell() {
    // An old client (or a recovering one) sends a plain `Spawn`, with claude
    // args and a proxy env, for a terminal. The journal says it is a shell.
    let dir = TestDaemon::new_dir();
    let id = Uuid::new_v4();
    let journal = serde_json::json!({"sessions": [{
        "id": id, "cwd": dir.join("work"), "kind": "shell", "created_at": 1700000000u64,
    }]});
    let path = TestDaemon::config(&dir).journal;
    crate::paths::write_atomic(&path, journal.to_string().as_bytes()).unwrap();

    let d = TestDaemon::start_in(dir).await;
    let mut c = d.client().await;
    let dormant = &c.sessions().await[0];
    assert_eq!(dormant.kind, proto::SessionKind::Shell);
    assert_eq!(dormant.pid, None);

    let mut spec = d.spec(id);
    spec.args = vec!["--resume".into(), "c1".into()];
    std::env::set_var("SHELL", "/bin/sh");
    assert!(c.spawn(spec).await.is_some());
    c.attach(id, 24, 80).await;
    c.type_line(id, "echo [${SECRET_ENV}]").await;
    let seen = c.output_until(id, "[]").await;
    assert!(!seen.contains("hunter2"), "a shell gets no proxy env");
    assert!(!d.work().join("settings.json").exists(), "claude was not run");

    let s = &c.sessions().await[0];
    assert_eq!((s.kind, s.created_at), (proto::SessionKind::Shell, 1700000000));
}
mod freshness;
mod git;
mod login_path;

// ── Respawn (reset a session in place) ───────────────────────────────────────

/// A fake claude that appends its arguments, one per line and closed by a
/// `---` line, to `argv.log` in its cwd, then behaves like the normal fake.
fn fake_claude_logging_argv() -> &'static Path {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("claudio-fake-claude-av-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("claude");
        let script = format!(
            "#!/bin/sh\n\
             if [ \"$1\" = \"--version\" ]; then echo '9.9.9 (Claude Code)'; exit 0; fi\n\
             if [ \"$1\" = \"--help\" ]; then exit 0; fi\n\
             printf '%s' \"$2\" > settings.json\n\
             {{ for a in \"$@\"; do printf '%s\\n' \"$a\"; done; echo ---; }} >> argv.log\n\
             echo {BANNER}\n\
             exec cat\n"
        );
        std::fs::write(&bin, script).unwrap();
        std::fs::set_permissions(&bin, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        bin
    })
}

impl TestDaemon {
    async fn start_logging_argv() -> TestDaemon {
        Self::start_with_claude(Self::new_dir(), fake_claude_logging_argv().to_path_buf()).await
    }

    /// The arguments of every claude run so far, without the daemon's
    /// `--settings <json>` prefix.
    fn runs(&self) -> Vec<Vec<String>> {
        let log = std::fs::read_to_string(self.work().join("argv.log")).unwrap_or_default();
        log.split("---\n")
            .filter(|run| !run.is_empty())
            .map(|run| run.lines().skip(2).map(str::to_owned).collect())
            .collect()
    }

    /// Wait until the fake claude has been started `n` times.
    async fn runs_reach(&self, n: usize) {
        within(async {
            while self.runs().len() < n {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
    }
}

impl Client {
    /// Like `call`, also returning the session events seen until the reply.
    async fn call_seen(&mut self, msg: Msg) -> (Msg, Vec<SessionEvent>) {
        let mut seen = Vec::new();
        let req = self.request(msg).await;
        let reply = self
            .until(|f| match f {
                Frame::Control(env) if env.req == Some(req) => Some(env.msg.clone()),
                Frame::Control(Envelope {
                    req: None,
                    msg: Msg::Event { event, .. },
                }) => {
                    seen.push(event.clone());
                    None
                }
                _ => None,
            })
            .await;
        (reply, seen)
    }

    async fn respawn(&mut self, id: SessionId, fresh: bool) -> (Option<u32>, Vec<SessionEvent>) {
        let spec = proto::RespawnSpec {
            id,
            fresh,
            env: vec![("SECRET_ENV".into(), "hunter2".into())],
            rows: 24,
            cols: 80,
            extensions: None,
        };
        match self.call_seen(Msg::Respawn(spec)).await {
            (Msg::Spawned { id: spawned, pid }, seen) => {
                assert_eq!(spawned, id);
                (pid, seen)
            }
            (other, _) => panic!("respawn failed: {other:?}"),
        }
    }
}

/// The process is gone (or was never ours).
fn is_dead(pid: u32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid as i32, 0) != 0 }
}

/// Wait for `pid` to be gone. A SIGKILLed child lingers as a zombie until
/// its waiter thread reaps it, a moment after the daemon reports it stopped.
async fn wait_dead(pid: u32) {
    within(async {
        while !is_dead(pid) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
}

fn assert_restarted_in_place(seen: &[SessionEvent]) {
    assert!(
        seen.iter().any(|e| matches!(e, SessionEvent::Created { .. })),
        "a respawn is announced like a spawn: {seen:?}"
    );
    assert!(
        !seen
            .iter()
            .any(|e| matches!(e, SessionEvent::Removed | SessionEvent::Exited { .. })),
        "the tab must not close: {seen:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn respawn_resumes_the_journaled_conversation() {
    let d = TestDaemon::start_logging_argv().await;
    let mut c = d.client().await;
    let id = Uuid::new_v4();
    let mut spec = d.spec(id);
    spec.args = vec!["--model".into(), "m".into(), "--resume".into(), "stale".into()];
    let old_pid = c.spawn(spec).await.unwrap();
    c.attach_until(id, BANNER).await;
    let payload = serde_json::json!({"session_id": "conv-1", "source": "startup"});
    send_hook(&d.config.socket, &d.token().await, "SessionStart", payload).await;
    c.event(id, |e| matches!(e, SessionEvent::ClaudeSession { .. }).then_some(()))
        .await;
    let created_at = d.journal()["sessions"][0]["created_at"].clone();

    let (pid, seen) = c.respawn(id, false).await;
    assert_restarted_in_place(&seen);
    assert_ne!(pid, Some(old_pid));
    wait_dead(old_pid).await;

    d.runs_reach(2).await;
    let runs = d.runs();
    assert_eq!(runs[0], ["--model", "m", "--resume", "stale", "-n", "test"]);
    assert_eq!(runs[1], ["--model", "m", "--resume", "conv-1", "-n", "test"]);

    // Same id, same journal entry (with the conversation), now live again.
    let journal = d.journal();
    assert_eq!(journal["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(journal["sessions"][0]["claude_session_id"], "conv-1");
    assert_eq!(journal["sessions"][0]["created_at"], created_at);
    let (listed, seen) = c.call_seen(Msg::ListSessions).await;
    let Msg::Sessions { sessions } = listed else {
        panic!("expected sessions, got {listed:?}");
    };
    assert_eq!((sessions.len(), sessions[0].id, sessions[0].pid), (1, id, pid));
    assert!(!seen.iter().any(|e| matches!(e, SessionEvent::Removed)));

    // The tab works again after a re-attach.
    c.attach_until(id, BANNER).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fresh_respawn_starts_a_new_conversation() {
    let d = TestDaemon::start_logging_argv().await;
    let mut c = d.client().await;
    let id = Uuid::new_v4();
    let mut spec = d.spec(id);
    spec.args = vec!["--model".into(), "m".into()];
    c.spawn(spec).await;
    c.attach_until(id, BANNER).await;
    let first = d.token().await;

    // No conversation recorded yet (no SessionStart): nothing to resume.
    let (_, seen) = c.respawn(id, false).await;
    assert_restarted_in_place(&seen);
    c.attach_until(id, BANNER).await;

    let payload = serde_json::json!({"session_id": "conv-1", "source": "startup"});
    let token = d.token_other_than(&first).await;
    send_hook(&d.config.socket, &token, "SessionStart", payload).await;
    c.event(id, |e| matches!(e, SessionEvent::ClaudeSession { .. }).then_some(()))
        .await;

    let (_, seen) = c.respawn(id, true).await;
    assert_restarted_in_place(&seen);
    d.runs_reach(3).await;
    let runs = d.runs();
    assert_eq!(runs[1], ["--model", "m", "-n", "test"]);
    assert_eq!(runs[2], ["--model", "m", "-n", "test"], "fresh omits --resume");
    assert_eq!(d.journal()["sessions"].as_array().unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn respawn_revives_a_dormant_session_and_rejects_unknown_ones() {
    let d = TestDaemon::start_logging_argv().await;
    let mut c = d.client().await;
    let unknown = c
        .call(Msg::Respawn(proto::RespawnSpec {
            id: Uuid::new_v4(),
            fresh: false,
            env: vec![],
            rows: 24,
            cols: 80,
            extensions: None,
        }))
        .await;
    assert!(matches!(unknown, Msg::Error { message } if message.contains("no such session")));

    // A crash leaves the session dormant; a respawn brings it back.
    let id = Uuid::new_v4();
    c.spawn(d.spec(id)).await;
    c.attach_until(id, BANNER).await;
    c.send(Frame::Data {
        session: id,
        bytes: vec![3],
    })
    .await;
    c.event(id, |e| matches!(e, SessionEvent::Exited { .. }).then_some(()))
        .await;
    assert_eq!(c.sessions().await[0].pid, None);
    let (pid, _) = c.respawn(id, false).await;
    assert!(pid.is_some());
    assert_eq!(c.sessions().await[0].pid, pid);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn respawn_restarts_a_shell() {
    let d = TestDaemon::start().await;
    let mut c = d.client().await;
    let id = Uuid::new_v4();
    let old_pid = c.spawn_shell(d.shell_spec(id)).await.unwrap();
    c.attach(id, 24, 80).await;
    c.type_line(id, "FLAG=set; echo flag-[$FLAG]").await;
    c.output_until(id, "flag-[set]").await;

    let (pid, seen) = c.respawn(id, true).await;
    assert_restarted_in_place(&seen);
    assert_ne!(pid, Some(old_pid));
    wait_dead(old_pid).await;

    // A new login shell: the variable is gone, and it is still a terminal.
    c.attach(id, 24, 80).await;
    c.type_line(id, "echo flag-[$FLAG]").await;
    c.output_until(id, "flag-[]").await;
    assert!(!d.work().join("settings.json").exists(), "claude was not run");
    let journal = d.journal();
    assert_eq!(journal["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(journal["sessions"][0]["kind"], "shell");
}

// ── Respawn races ────────────────────────────────────────────────────────────

/// A fake claude that ignores SIGHUP, so stopping it takes the actor's whole
/// kill grace (3 s): a wide, deterministic window in the middle of a respawn.
fn fake_claude_slow_to_stop() -> &'static Path {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("claudio-fake-claude-ss-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("claude");
        let script = format!(
            "#!/bin/sh\n\
             if [ \"$1\" = \"--version\" ]; then echo '9.9.9 (Claude Code)'; exit 0; fi\n\
             if [ \"$1\" = \"--help\" ]; then exit 0; fi\n\
             trap '' HUP\n\
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

fn respawn_spec(id: SessionId, fresh: bool) -> proto::RespawnSpec {
    proto::RespawnSpec {
        id,
        fresh,
        env: vec![],
        rows: 24,
        cols: 80,
        extensions: None,
    }
}

impl Client {
    /// Wait until `id` no longer runs `old_pid`: a respawn has taken it over
    /// and is stopping the old process (or, where it died at once, already
    /// started the new one).
    async fn until_stopping(&mut self, id: SessionId, old_pid: u32) {
        within(async {
            while self
                .sessions()
                .await
                .iter()
                .any(|s| s.id == id && s.pid == Some(old_pid))
            {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
    }

    /// The reply to request `req`.
    async fn reply(&mut self, req: u64) -> Msg {
        self.until(|f| match f {
            Frame::Control(env) if env.req == Some(req) => Some(env.msg.clone()),
            _ => None,
        })
        .await
    }
}

/// A Kill from another client while a respawn is stopping the old process
/// wins: it is acknowledged, the respawn gives up, and the session never
/// comes back. A second respawn meanwhile is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_kill_during_a_respawn_is_not_undone() {
    let d = TestDaemon::start_with_claude(
        TestDaemon::new_dir(),
        fake_claude_slow_to_stop().to_path_buf(),
    )
    .await;
    let mut a = d.client().await;
    let mut b = d.client().await;
    let id = Uuid::new_v4();
    let old_pid = a.spawn(d.spec(id)).await.unwrap();
    a.attach_until(id, BANNER).await;

    let respawn = a.request(Msg::Respawn(respawn_spec(id, false))).await;
    b.until_stopping(id, old_pid).await;

    let second = b.call(Msg::Respawn(respawn_spec(id, true))).await;
    assert!(
        matches!(&second, Msg::Error { message } if message.contains("in progress")),
        "{second:?}"
    );
    assert_eq!(b.call(Msg::Kill { id }).await, Msg::Ok);

    let reply = a.reply(respawn).await;
    assert!(matches!(reply, Msg::Error { .. }), "{reply:?}");
    assert!(b.sessions().await.is_empty(), "the killed session came back");
    assert!(d.journal()["sessions"].as_array().unwrap().is_empty());
    wait_dead(old_pid).await;
    assert!(d.daemon.registry().in_flight.is_empty());
}

/// The old process's clean-exit close, landing while a respawn has taken the
/// id over (or after it committed), must leave the new process alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_late_clean_exit_close_spares_the_respawned_session() {
    let d = TestDaemon::start_with_claude(
        TestDaemon::new_dir(),
        fake_claude_slow_to_stop().to_path_buf(),
    )
    .await;
    let mut a = d.client().await;
    let mut b = d.client().await;
    let id = Uuid::new_v4();
    let old_pid = a.spawn(d.spec(id)).await.unwrap();

    let respawn = a.request(Msg::Respawn(respawn_spec(id, false))).await;
    b.until_stopping(id, old_pid).await;
    // What the old actor does after a clean exit, at the worst moment.
    d.daemon.close_exited(id);
    let pid = match a.reply(respawn).await {
        Msg::Spawned { pid, .. } => pid,
        other => panic!("respawn failed: {other:?}"),
    };
    // ...or once the new process is committed.
    d.daemon.close_exited(id);

    let sessions = b.sessions().await;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].pid, pid);
    assert_eq!(d.journal()["sessions"].as_array().unwrap().len(), 1);
}

/// A clean exit racing a respawn from another client, end to end: whichever
/// wins, the outcome is consistent. A respawn that answers `Spawned` keeps
/// its session; otherwise the session is closed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_clean_exit_racing_a_respawn_stays_consistent() {
    let d = TestDaemon::start().await;
    let mut a = d.client().await;
    let mut b = d.client().await;
    for _ in 0..5 {
        let id = Uuid::new_v4();
        a.spawn(d.spec(id)).await;
        a.attach_until(id, BANNER).await;
        a.send(Frame::Data {
            session: id,
            bytes: vec![4],
        })
        .await;
        let reply = b.call(Msg::Respawn(respawn_spec(id, false))).await;
        // Let a close, if one is coming, land.
        tokio::time::sleep(Duration::from_millis(300)).await;
        let listed = b.sessions().await.into_iter().find(|s| s.id == id);
        match reply {
            Msg::Spawned { pid, .. } => {
                let s = listed.expect("a respawned session was closed");
                assert_eq!(s.pid, pid);
                b.call(Msg::Kill { id }).await;
            }
            other => assert!(listed.is_none(), "{other:?} but still listed"),
        }
    }
}

/// Every client attached to a session keeps receiving it after another
/// client resets it: it gets `Attached` and the new snapshot unasked.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_respawn_keeps_other_clients_attached() {
    let d = TestDaemon::start().await;
    let mut a = d.client().await;
    let mut b = d.client().await;
    let id = Uuid::new_v4();
    a.spawn(d.spec(id)).await;
    a.attach_until(id, BANNER).await;
    b.attach_until(id, BANNER).await;

    a.respawn(id, false).await;
    // `Attached` (unrequested) comes before any of the new process's output.
    b.until(|f| match f {
        Frame::Control(Envelope {
            req: None,
            msg: Msg::Attached { id: i, .. },
        }) if *i == id => Some(()),
        _ => None,
    })
    .await;
    b.output_until(id, BANNER).await;
    b.type_line(id, "after-reset").await;
    b.output_until(id, "after-reset").await;
}

/// A fresh reset drops the old conversation everywhere: the journal, the
/// listing, and clients (told by `ClaudeSessionCleared`). So when the new
/// process dies before its first `SessionStart`, bringing it back does not
/// resume the conversation the user discarded.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fresh_reset_forgets_the_old_conversation() {
    let d = TestDaemon::start_logging_argv().await;
    let mut c = d.client().await;
    let id = Uuid::new_v4();
    c.spawn(d.spec(id)).await;
    c.attach_until(id, BANNER).await;
    let payload = serde_json::json!({"session_id": "conv-1", "source": "startup"});
    send_hook(&d.config.socket, &d.token().await, "SessionStart", payload).await;
    c.event(id, |e| matches!(e, SessionEvent::ClaudeSession { .. }).then_some(()))
        .await;

    let (_, seen) = c.respawn(id, true).await;
    assert!(seen.contains(&SessionEvent::ClaudeSessionCleared), "{seen:?}");
    let journal = d.journal();
    assert_eq!(journal["sessions"][0]["claude_session_id"], serde_json::Value::Null);

    // The new claude crashes before SessionStart.
    c.attach_until(id, BANNER).await;
    c.send(Frame::Data {
        session: id,
        bytes: vec![3],
    })
    .await;
    c.event(id, |e| matches!(e, SessionEvent::Exited { .. }).then_some(()))
        .await;
    let s = &c.sessions().await[0];
    assert_eq!((s.pid, s.claude_session_id.as_deref()), (None, None));

    c.respawn(id, false).await;
    d.runs_reach(3).await;
    let runs = d.runs();
    assert!(
        !runs[2].iter().any(|a| a == "--resume"),
        "resumed the discarded conversation: {:?}",
        runs[2]
    );
}

/// The `--allow-dangerously-skip-permissions` check is redone after a claude
/// update, and a claude missing at startup is not stuck without it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skip_permissions_support_is_rechecked_after_an_update() {
    let dir = TestDaemon::new_dir();
    let claude = dir.join("bin/claude");
    let d = TestDaemon::start_with_claude(dir.clone(), claude.clone()).await;
    let check = |daemon: &Arc<Daemon>| {
        let daemon = Arc::clone(daemon);
        tokio::task::spawn_blocking(move || daemon.skip_permissions())
    };
    assert!(!check(&d.daemon).await.unwrap(), "no claude yet");

    std::fs::create_dir_all(claude.parent().unwrap()).unwrap();
    let script = "#!/bin/sh\n\
         if [ \"$1\" = \"--help\" ]; then echo '  --allow-dangerously-skip-permissions'; fi\n\
         exit 0\n";
    std::fs::write(&claude, script).unwrap();
    std::fs::set_permissions(&claude, std::os::unix::fs::PermissionsExt::from_mode(0o755))
        .unwrap();
    assert!(
        check(&d.daemon).await.unwrap(),
        "a probe that did not run is not remembered"
    );
    // A probe that ran is cached until a refresh.
    std::fs::write(&claude, "#!/bin/sh\nexit 0\n").unwrap();
    assert!(check(&d.daemon).await.unwrap(), "cached until a refresh");
    d.daemon.refresh_host().await;
    assert!(!check(&d.daemon).await.unwrap());
}

/// A session started in a repository announces its git status (branch,
/// ahead/behind its upstream, dirty paths) in `Meta` without any hook: a
/// terminal tab gets it too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn meta_carries_the_git_status_of_the_session_directory() {
    let d = TestDaemon::start().await;
    let repo = super::git_status::tests::temp_repo();
    let mut c = d.client().await;
    let id = Uuid::new_v4();
    let spec = SpawnSpec {
        cwd: repo.to_string_lossy().into_owned(),
        ..d.spec(id)
    };
    c.spawn(spec).await;
    let (branch, git) = c
        .event(id, |e| match e {
            SessionEvent::Meta {
                branch,
                git: Some(git),
                ..
            } => Some((branch.clone(), git.clone())),
            _ => None,
        })
        .await;
    assert_eq!(branch.as_deref(), Some("main"));
    assert_eq!(git.branch(), Some("main"));
    assert!(git.upstream);
    assert_eq!((git.ahead, git.behind), (1, 1));
    assert_eq!((git.staged, git.unstaged), (0, 1));
    // The repo's own untracked file, plus the fake claude's `settings.json`
    // when it has been written by the time git looked.
    assert!((1..=2).contains(&git.untracked), "{}", git.untracked);
    assert!(git.is_dirty());
    let _ = std::fs::remove_dir_all(&repo);
}

// ── Extensions: the context file and the effective value ─────────────────────

fn lens(name: &str, instruction: &str) -> proto::LensSpec {
    proto::LensSpec {
        name: name.into(),
        kind: proto::LensKind::Tag,
        instruction: instruction.into(),
    }
}

fn extensions(lenses: &[(&str, &str)]) -> proto::SessionExtensions {
    proto::SessionExtensions {
        lenses: lenses.iter().map(|(n, i)| lens(n, i)).collect(),
        ..Default::default()
    }
}

/// The lens names a journal entry holds (`None`: no `extensions` field).
fn journaled_lenses(entry: &serde_json::Value) -> Option<Vec<String>> {
    let lenses = entry.get("extensions")?["lenses"].as_array()?.clone();
    Some(lenses.iter().map(|l| l["name"].as_str().unwrap().to_owned()).collect())
}

#[test]
fn effective_extensions_follow_request_then_journal_then_legacy() {
    use super::effective_extensions;
    let names = |e: proto::SessionExtensions| -> Vec<String> {
        e.lenses.into_iter().map(|l| l.name).collect()
    };
    let asked = extensions(&[("keys", "k")]);
    let journaled = extensions(&[("abc", "a")]);
    // The request wins; without one the journal's; without both the legacy.
    let e = effective_extensions(Some(asked.clone()), Some(journaled.clone()), true);
    assert_eq!(names(e), ["keys"]);
    let e = effective_extensions(None, Some(journaled), true);
    assert_eq!(names(e), ["abc"]);
    let legacy = effective_extensions(None, None, true);
    assert_eq!(legacy.lenses.len(), 1);
    assert_eq!(legacy.lenses[0].name, "latex");
    assert_eq!(legacy.lenses[0].kind, proto::LensKind::Tag);
    assert_eq!(legacy.lenses[0].instruction, crate::claude::hooks::LATEX_CONTEXT);
    // A host with `[claude] latex = false` drops the latex lens, asked or not.
    assert!(effective_extensions(None, None, false).lenses.is_empty());
    let both = extensions(&[("latex", "math"), ("keys", "k")]);
    assert_eq!(names(effective_extensions(Some(both), None, false)), ["keys"]);
    // A lens of a kind this daemon does not know is dropped.
    let mut odd = extensions(&[("keys", "k"), ("new", "n")]);
    odd.lenses[1].kind = proto::LensKind::Unknown;
    assert_eq!(names(effective_extensions(Some(odd), None, true)), ["keys"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_spawn_writes_a_private_context_file_and_a_kill_removes_it() {
    use std::os::unix::fs::PermissionsExt;
    let d = TestDaemon::start().await;
    let mut c = d.client().await;
    let id = Uuid::new_v4();
    let mut spec = d.spec(id);
    // This host has `[claude] latex = false`: the latex lens goes.
    spec.extensions = Some(extensions(&[
        ("latex", "MATH-INSTRUCTION"),
        ("keys", "KEYS-INSTRUCTION"),
        ("abc", ""),
    ]));
    c.spawn(spec).await;
    d.token().await;

    let path = d.dir.join("rt/context").join(format!("{id}.txt"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "KEYS-INSTRUCTION");
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&path), 0o600);
    assert_eq!(mode(path.parent().unwrap()), 0o700);

    // The hook has the path; the settings never the text.
    let settings = std::fs::read_to_string(d.work().join("settings.json")).unwrap();
    assert!(!settings.contains("INSTRUCTION"), "{settings}");
    let settings: serde_json::Value = serde_json::from_str(&settings).unwrap();
    let hooks = settings["hooks"]["SessionStart"][0]["hooks"].as_array().unwrap();
    let expected = format!("'/bin/true' __context file '{}'", path.display());
    assert_eq!(hooks[1]["command"], expected.as_str());

    // The journal keeps the effective value.
    let journal = d.journal();
    assert_eq!(journaled_lenses(&journal["sessions"][0]).unwrap(), ["keys", "abc"]);

    assert_eq!(c.call(Msg::Kill { id }).await, Msg::Ok);
    assert!(!path.exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_instructions_means_no_context_file_and_no_hook() {
    let d = TestDaemon::start().await;
    let mut c = d.client().await;
    let id = Uuid::new_v4();
    // An old client: no extensions, and this host has latex off.
    c.spawn(d.spec(id)).await;
    d.token().await;
    let settings = std::fs::read_to_string(d.work().join("settings.json")).unwrap();
    let settings: serde_json::Value = serde_json::from_str(&settings).unwrap();
    assert_eq!(settings["hooks"]["SessionStart"][0]["hooks"].as_array().unwrap().len(), 1);
    assert!(!d.dir.join("rt/context").join(format!("{id}.txt")).exists());
    assert_eq!(journaled_lenses(&d.journal()["sessions"][0]).unwrap(), Vec::<String>::new());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_respawn_with_extensions_replaces_the_journaled_ones() {
    let d = TestDaemon::start_logging_argv().await;
    let mut c = d.client().await;
    let id = Uuid::new_v4();
    let mut spec = d.spec(id);
    spec.extensions = Some(extensions(&[("keys", "OLD")]));
    c.spawn(spec).await;
    d.runs_reach(1).await;
    let path = d.dir.join("rt/context").join(format!("{id}.txt"));

    // No extensions in the respawn (an old client): the journal's stay.
    c.respawn(id, false).await;
    d.runs_reach(2).await;
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "OLD");

    let mut respawn = respawn_spec(id, false);
    respawn.extensions = Some(extensions(&[("abc", "NEW")]));
    assert!(matches!(c.call(Msg::Respawn(respawn)).await, Msg::Spawned { .. }));
    d.runs_reach(3).await;
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "NEW");
    assert_eq!(journaled_lenses(&d.journal()["sessions"][0]).unwrap(), ["abc"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn extensions_over_the_caps_are_refused() {
    let d = TestDaemon::start().await;
    let mut c = d.client().await;
    let id = Uuid::new_v4();
    let mut spec = d.spec(id);
    spec.extensions = Some(extensions(&[("keys", "a"), ("keys", "b")]));
    let reply = c.call(Msg::Spawn(spec)).await;
    assert!(
        matches!(&reply, Msg::Error { message } if message.contains("bad extensions")),
        "{reply:?}"
    );
    assert!(c.sessions().await.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_daemon_start_clears_old_context_files() {
    let dir = TestDaemon::new_dir();
    let context = dir.join("rt/context");
    std::fs::create_dir_all(&context).unwrap();
    std::fs::write(context.join("old.txt"), "stale").unwrap();
    let _d = TestDaemon::start_in(dir).await;
    assert!(!context.join("old.txt").exists());
}

// ── Manifest ─────────────────────────────────────────────────────────────────

/// The manifest the `Manifest` tests apply: one marketplace with plugins
/// `a` and `b`, servers `kb` (secret `KB_TOKEN`), `mine` and `jina`
/// (secret `JINA_KEY`), agents `reviewer` and `helper`, instructions, env.
const TEAM: &str = r#"
format = 1
name = "team"
version = "1.0.0"
instructions = "Team rules.\u202e"

[[marketplace]]
name = "market"
source = "github:acme/market"

[[plugin]]
name = "a"
marketplace = "market"

[[plugin]]
name = "b"
marketplace = "market"

[[mcp]]
name = "kb"
type = "http"
url = "https://kb.example.org/mcp"
headers = { Authorization = "Bearer ${KB_TOKEN}" }
secrets = ["KB_TOKEN"]

[[mcp]]
name = "mine"
type = "sse"
url = "https://mine.example.org/sse"

[[mcp]]
name = "jina"
type = "http"
url = "https://jina.example.org"
headers = { Authorization = "Bearer ${JINA_KEY}" }
secrets = ["JINA_KEY"]

[[agent]]
name = "reviewer"
description = "Reviews"
prompt = "Review."

[[agent]]
name = "helper"
description = "Helps"
prompt = "Help."

[env]
REGION = "eu"
"#;

/// The fake `claude` of the `Manifest` tests, shared (written once). Its
/// control dir is the first entry of its `PATH` (the test daemon's
/// `Config::path`), and Claude Code's config dir is `<ctl>/../claude`.
/// Every `plugin …` run appends its argv to `<ctl>/log`. `marketplace
/// add` registers the name in `<ctl>/market-name` (default `market`), or
/// fails with `<ctl>/market-fails`; with `<ctl>/market-raced` it registers
/// the name and then fails, as when another run added it meanwhile.
/// `install <spec>` waits while `<ctl>/gate-<spec>` exists, then fails
/// with `<ctl>/fail-<spec>`, needs a person with `<ctl>/manual-<spec>`,
/// succeeds without listing it with `<ctl>/silent-<spec>`, or adds the
/// spec to `installed_plugins.json`. `--help` prints `<ctl>/help`.
fn fake_plugin_claude() -> &'static Path {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("claudio-fake-plugins-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("claude");
        let script = r#"#!/bin/sh
ctl="${PATH%%:*}"
cfg="$ctl/../claude"
case "$1" in
--version) echo '9.9.9 (Claude Code)'; exit 0;;
--help) [ -f "$ctl/help" ] && cat "$ctl/help"; exit 0;;
update) echo updated; exit 0;;
plugin) ;;
*) exit 2;;
esac
echo "$*" >> "$ctl/log"
mkdir -p "$cfg/plugins"
if [ "$2" = marketplace ]; then
  if [ -f "$ctl/market-fails" ]; then echo '{"error":"repository not found"}'; exit 1; fi
  name=$(cat "$ctl/market-name" 2>/dev/null || echo market)
  printf '{"%s":{"source":{"source":"github","repo":"%s"}}}\n' "$name" "$4" \
    > "$cfg/plugins/known_marketplaces.json"
  if [ -f "$ctl/market-raced" ]; then echo '{"error":"already exists"}'; exit 1; fi
  echo '{"ok":true}'
  exit 0
fi
spec="$3"
while [ -e "$ctl/gate-$spec" ]; do sleep 0.05; done
if [ -f "$ctl/fail-$spec" ]; then echo 'Installing'; echo '{"error":"network down"}'; exit 1; fi
if [ -f "$ctl/manual-$spec" ]; then echo '{"shownCommand":{"command":"npx x","sha256":"ab"}}'; exit 1; fi
if [ -f "$ctl/silent-$spec" ]; then echo '{"ok":true}'; exit 0; fi
echo "$spec" >> "$ctl/installed"
{
  printf '{"version":2,"plugins":{'
  sep=
  while read -r k; do printf '%s"%s":[{"scope":"user"}]' "$sep" "$k"; sep=,; done < "$ctl/installed"
  printf '}}\n'
} > "$cfg/plugins/installed_plugins.json"
echo "done $spec" >> "$ctl/log"
echo '{"ok":true}'
"#;
        std::fs::write(&bin, script).unwrap();
        std::fs::set_permissions(&bin, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        bin
    })
}

/// A `Manifest` test's daemon: the fake plugin claude, manifests cached in
/// `<dir>/cache/plugins` (TEAM there already), Claude Code's config in
/// `<dir>/claude`, and the fake's control dir `<dir>/ctl`.
struct PluginHost {
    d: TestDaemon,
    cache: PathBuf,
    claude: PathBuf,
    ctl: PathBuf,
}

impl PluginHost {
    async fn start() -> PluginHost {
        let dir = TestDaemon::new_dir();
        let (cache, claude, ctl) = (
            dir.join("cache/plugins"),
            dir.join("claude"),
            dir.join("ctl"),
        );
        for d in [&cache, &claude, &ctl] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(cache.join("team.toml"), TEAM).unwrap();
        std::fs::write(
            cache.join("team.sha256"),
            format!("{}\n", sha256_hex(TEAM.as_bytes())),
        )
        .unwrap();
        let config = Config {
            claude: fake_plugin_claude().to_path_buf(),
            path: Some(format!("{}:/usr/bin:/bin", ctl.display()).into()),
            ..TestDaemon::config(&dir)
        };
        let d = TestDaemon::start_with_config(dir, config);
        d.daemon.manifests.set_dirs(cache.clone(), claude.clone());
        PluginHost {
            d,
            cache,
            claude,
            ctl,
        }
    }

    /// Create (or with `on` false, remove) control file `name`.
    fn set(&self, name: &str, on: bool) {
        let path = self.ctl.join(name);
        if on {
            std::fs::write(path, "").unwrap();
        } else {
            let _ = std::fs::remove_file(path);
        }
    }

    /// The fake's `plugin …` runs so far.
    fn log(&self) -> Vec<String> {
        std::fs::read_to_string(self.ctl.join("log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// Wait until the log has a line equal to `line`.
    async fn logged(&self, line: &str) {
        within(async {
            while !self.log().iter().any(|l| l == line) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
    }

    /// Wait until no install of manifest TEAM is queued or running.
    async fn idle(&self) {
        within(async {
            while self.d.daemon.manifests.busy() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
    }
}

/// A `Manifest` request for TEAM (cached; its URL is never reached).
fn team(secrets: &[&str], disabled: &[&str]) -> Msg {
    Msg::Manifest {
        name: "team".into(),
        url: "https://127.0.0.1:9/never.toml".into(),
        sha256: sha256_hex(TEAM.as_bytes()),
        secrets: secrets.iter().map(|s| s.to_string()).collect(),
        disabled: disabled.iter().map(|s| s.to_string()).collect(),
    }
}

fn applied(reply: Msg) -> proto::Applied {
    match reply {
        Msg::Applied(a) => a,
        other => panic!("expected Applied, got {other:?}"),
    }
}

fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

/// `item` is `id` skipped for `why`, tried again in `secs` s (or one less:
/// the clock moved on since the failure).
fn assert_retrying(item: &(String, String), id: &str, why: &str, secs: u64) {
    let due = (secs.saturating_sub(1)..=secs).any(|s| item.1 == proto::retrying(why, s));
    assert!(item.0 == id && due, "{item:?}: want {id}, {why}, {secs} s");
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(bytes))
}

/// The reply comes before any install runs: plugins are `installing`,
/// every static part is there (servers with their secrets, agents, the
/// instructions sanitized, the env), and no `-y` reaches the CLI. The
/// marketplace is added once for both plugins; later replies say
/// `installed`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manifest_replies_first_then_installs_each_plugin_once() {
    let h = PluginHost::start().await;
    h.set("gate-a@market", true);
    let mut c = h.d.client().await;
    let a = applied(c.call(team(&["KB_TOKEN", "JINA_KEY"], &[])).await);
    assert_eq!((a.name.as_str(), a.version.as_str()), ("team", "1.0.0"));
    assert_eq!(
        a.skipped,
        pairs(&[
            ("plugin:a@market", "installing"),
            ("plugin:b@market", "installing")
        ])
    );
    assert!(a.installed.is_empty() && a.present.is_empty());
    let mcp: serde_json::Value = serde_json::from_str(a.mcp_config.as_deref().unwrap()).unwrap();
    let servers: Vec<&String> = mcp["mcpServers"].as_object().unwrap().keys().collect();
    assert_eq!(servers, ["jina", "kb", "mine"]);
    assert_eq!(
        mcp["mcpServers"]["kb"]["headers"]["Authorization"],
        "Bearer ${KB_TOKEN}"
    );
    let agents: serde_json::Value = serde_json::from_str(a.agents.as_deref().unwrap()).unwrap();
    assert_eq!(agents["reviewer"]["prompt"], "Review.");
    assert_eq!(a.instructions, "Team rules.");
    assert_eq!(a.env, pairs(&[("REGION", "eu")]));
    // The fake's `--help` lists neither flag.
    assert_eq!(a.dropped, ["--mcp-config", "--agents"]);

    // Asked again while `a` is blocked: nothing is queued twice.
    h.logged("plugin install a@market --scope user --json")
        .await;
    let again = applied(c.call(team(&["KB_TOKEN", "JINA_KEY"], &[])).await);
    assert_eq!(again.skipped, a.skipped);
    h.set("gate-a@market", false);
    h.idle().await;
    assert_eq!(
        h.log(),
        [
            "plugin marketplace add acme/market --scope user --json",
            "plugin install a@market --scope user --json",
            "done a@market",
            "plugin install b@market --scope user --json",
            "done b@market",
        ]
    );
    assert!(!h
        .log()
        .iter()
        .any(|l| l.contains("-y") || l.contains("--yes") || l.contains("--accept-command")));
    let done = applied(c.call(team(&["KB_TOKEN", "JINA_KEY"], &[])).await);
    assert_eq!(done.installed, ["plugin:a@market", "plugin:b@market"]);
    assert!(done.skipped.is_empty() && done.present.is_empty());
    assert_eq!(h.log().len(), 5, "nothing ran again");
    assert!(h.cache.join("team.toml").is_file());
}

/// What is already there is left alone: an installed plugin (any scope) is
/// `present`, a server in `.claude.json` and an agent in `agents/` are the
/// member's (`yours`), a server whose secret the request lacks is skipped,
/// and a disabled id is neither applied nor listed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manifest_leaves_the_members_own_alone_and_honours_disabled() {
    let h = PluginHost::start().await;
    std::fs::create_dir_all(h.claude.join("plugins")).unwrap();
    std::fs::create_dir_all(h.claude.join("agents")).unwrap();
    std::fs::write(
        h.claude.join("plugins/installed_plugins.json"),
        r#"{"version":2,"plugins":{"a@market":[{"scope":"project"}]}}"#,
    )
    .unwrap();
    std::fs::write(
        h.claude.join(".claude.json"),
        r#"{"mcpServers":{"mine":{}}}"#,
    )
    .unwrap();
    std::fs::write(h.claude.join("agents/helper.md"), "my helper").unwrap();
    let mut c = h.d.client().await;
    let a = applied(
        c.call(team(
            &["KB_TOKEN"],
            &["plugin:b@market", "env", "instructions"],
        ))
        .await,
    );
    assert_eq!(a.present, ["plugin:a@market"]);
    assert_eq!(
        a.skipped,
        pairs(&[
            ("mcp:mine", "yours"),
            ("mcp:jina", "missing secret JINA_KEY"),
            ("agent:helper", "yours"),
        ])
    );
    let mcp: serde_json::Value = serde_json::from_str(a.mcp_config.as_deref().unwrap()).unwrap();
    let servers: Vec<&String> = mcp["mcpServers"].as_object().unwrap().keys().collect();
    assert_eq!(servers, ["kb"]);
    let agents: serde_json::Value = serde_json::from_str(a.agents.as_deref().unwrap()).unwrap();
    assert!(agents.get("helper").is_none() && agents.get("reviewer").is_some());
    assert!(a.instructions.is_empty() && a.env.is_empty());
    h.idle().await;
    assert!(h.log().is_empty(), "nothing to install: {:?}", h.log());

    // Every server and agent disabled: no JSON at all.
    let all = [
        "plugin:b@market",
        "mcp:kb",
        "mcp:mine",
        "mcp:jina",
        "agent:reviewer",
        "agent:helper",
    ];
    let a = applied(c.call(team(&["KB_TOKEN"], &all)).await);
    assert_eq!((a.mcp_config, a.agents), (None, None));
    assert!(a.skipped.is_empty(), "{:?}", a.skipped);

    // An unreadable `.claude.json` skips every server, with the reason.
    std::fs::write(h.claude.join(".claude.json"), "{").unwrap();
    let a = applied(
        c.call(team(&["KB_TOKEN", "JINA_KEY"], &["plugin:b@market"]))
            .await,
    );
    assert_eq!(a.mcp_config, None);
    let unreadable: Vec<&(String, String)> = a
        .skipped
        .iter()
        .filter(|(_, why)| why == "cannot read .claude.json")
        .collect();
    assert_eq!(unreadable.len(), 3, "{:?}", a.skipped);
}

/// A failed install is skipped with the CLI's reason and the seconds to
/// its retry: 15 s after the first failure, doubling with each one in a
/// row, and 15 s again after a success. One that needs a person stays
/// `needs manual install` for the daemon's life.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_installs_back_off_and_manual_ones_never_retry() {
    let h = PluginHost::start().await;
    h.set("fail-a@market", true);
    h.set("manual-b@market", true);
    let mut c = h.d.client().await;
    c.call(team(&[], &[])).await;
    h.idle().await;
    let manual = "needs manual install: claude plugin install b@market";
    let a = applied(c.call(team(&[], &[])).await);
    assert_retrying(&a.skipped[0], "plugin:a@market", "network down", 15);
    assert_eq!(a.skipped[1], pairs(&[("plugin:b@market", manual)])[0]);
    let installs = |h: &PluginHost, spec: &str| {
        let line = format!("plugin install {spec} --scope user --json");
        h.log().iter().filter(|l| **l == line).count()
    };
    assert_eq!(installs(&h, "a@market"), 1);

    // Within the backoff nothing runs again; after it, one more run, and
    // the next wait is twice as long.
    h.d.daemon.manifests.advance(Duration::from_secs(14));
    let a = applied(c.call(team(&[], &[])).await);
    assert_retrying(&a.skipped[0], "plugin:a@market", "network down", 1);
    h.idle().await;
    assert_eq!(installs(&h, "a@market"), 1);
    h.d.daemon.manifests.advance(Duration::from_secs(1));
    let a = applied(c.call(team(&[], &[])).await);
    assert_eq!(a.skipped[0].1, "installing");
    h.idle().await;
    assert_eq!(installs(&h, "a@market"), 2);
    let a = applied(c.call(team(&[], &[])).await);
    assert_retrying(&a.skipped[0], "plugin:a@market", "network down", 30);

    // Then it works; the manual one still waits for a person.
    h.set("fail-a@market", false);
    h.d.daemon.manifests.advance(Duration::from_secs(30));
    c.call(team(&[], &[])).await;
    h.idle().await;
    let a = applied(c.call(team(&[], &[])).await);
    assert_eq!(a.installed, ["plugin:a@market"]);
    assert_eq!(a.skipped[0], pairs(&[("plugin:b@market", manual)])[0]);
    assert_eq!(installs(&h, "b@market"), 1);

    // Uninstalled and failing again: the backoff starts over at 15 s.
    std::fs::remove_file(h.ctl.join("installed")).unwrap();
    std::fs::write(
        h.claude.join("plugins/installed_plugins.json"),
        r#"{"version":2,"plugins":{}}"#,
    )
    .unwrap();
    h.set("fail-a@market", true);
    let a = applied(c.call(team(&[], &[])).await);
    assert_eq!(a.skipped[0].1, "installing");
    h.idle().await;
    let a = applied(c.call(team(&[], &[])).await);
    assert_retrying(&a.skipped[0], "plugin:a@market", "network down", 15);
    assert_eq!(installs(&h, "a@market"), 4);
}

/// A marketplace Claude Code files under another name than the manifest's
/// skips its plugins with that reason (and is not added again at once).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_marketplace_added_under_another_name_skips_its_plugins() {
    let h = PluginHost::start().await;
    std::fs::write(h.ctl.join("market-name"), "acme").unwrap();
    let mut c = h.d.client().await;
    c.call(team(&[], &[])).await;
    h.idle().await;
    let a = applied(c.call(team(&[], &[])).await);
    let why = "marketplace market: added under another name";
    assert_retrying(&a.skipped[0], "plugin:a@market", why, 15);
    assert_retrying(&a.skipped[1], "plugin:b@market", why, 15);
    assert_eq!(
        h.log(),
        ["plugin marketplace add acme/market --scope user --json"]
    );
}

/// An install that exits 0 but leaves the plugin out of
/// `installed_plugins.json` is remembered as a failure for its backoff,
/// not run again on every re-ask.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_install_claude_does_not_list_is_remembered_as_failed() {
    let h = PluginHost::start().await;
    h.set("silent-a@market", true);
    let mut c = h.d.client().await;
    c.call(team(&[], &[])).await;
    h.idle().await;
    let a = applied(c.call(team(&[], &[])).await);
    assert_eq!(a.installed, ["plugin:b@market"]);
    let why = "installed but not listed";
    assert_retrying(&a.skipped[0], "plugin:a@market", why, 15);

    // Within the backoff: not run again.
    h.d.daemon.manifests.advance(Duration::from_secs(10));
    let a = applied(c.call(team(&[], &[])).await);
    assert_retrying(&a.skipped[0], "plugin:a@market", why, 5);
    h.idle().await;
    let installs = |h: &PluginHost| {
        h.log()
            .iter()
            .filter(|l| l.as_str() == "plugin install a@market --scope user --json")
            .count()
    };
    assert_eq!(installs(&h), 1);

    // After it: tried again.
    h.d.daemon.manifests.advance(Duration::from_secs(5));
    let a = applied(c.call(team(&[], &[])).await);
    assert_eq!(a.skipped[0].1, "installing");
    h.idle().await;
    assert_eq!(installs(&h), 2);
}

/// A marketplace add that fails while the name is known by then (another
/// manifest's install added it meanwhile) counts as added: its plugins
/// install, and no failure is remembered.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_marketplace_add_of_a_name_known_by_then_counts_as_added() {
    let h = PluginHost::start().await;
    h.set("market-raced", true);
    let mut c = h.d.client().await;
    c.call(team(&[], &[])).await;
    h.idle().await;
    let a = applied(c.call(team(&[], &[])).await);
    assert_eq!(a.installed, ["plugin:a@market", "plugin:b@market"]);
    assert!(
        !a.skipped.iter().any(|(id, _)| id.starts_with("plugin:")),
        "{:?}",
        a.skipped
    );
    assert_eq!(
        h.log(),
        [
            "plugin marketplace add acme/market --scope user --json",
            "plugin install a@market --scope user --json",
            "done a@market",
            "plugin install b@market --scope user --json",
            "done b@market",
        ]
    );
}

/// Installs share `claude update`'s lock, one subprocess at a time: an
/// update during an install is refused with a message naming both, and a
/// waiter that queued during one install gets the lock before the next.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn installs_and_claude_update_take_turns() {
    let h = PluginHost::start().await;
    h.set("gate-a@market", true);
    let mut c = h.d.client().await;
    c.call(team(&[], &[])).await;
    h.logged("plugin install a@market --scope user --json")
        .await;
    assert_eq!(
        c.call(Msg::UpdateClaude { install: false }).await,
        Msg::Error {
            message: "a claude update or plugin install is already running on this host".into()
        }
    );
    // Queue for the lock while `a` installs; `b` must wait for us.
    let daemon = Arc::clone(&h.d.daemon);
    let (got, held) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel::<()>();
    let waiter = tokio::spawn(async move {
        let _lock = daemon.updating.lock().await;
        let _ = got.send(());
        let _ = released.await;
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    h.set("gate-a@market", false);
    within(held).await.unwrap();
    h.logged("done a@market").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !h.log().iter().any(|l| l.contains("install b@market")),
        "{:?}",
        h.log()
    );
    release.send(()).unwrap();
    waiter.await.unwrap();
    h.idle().await;
    // Between installs (here: after them) an update runs.
    assert!(matches!(
        c.call(Msg::UpdateClaude { install: false }).await,
        Msg::ClaudeUpdated { ok: true, .. }
    ));
}

/// The fake's `--help` lists the flags: nothing is dropped.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn applied_names_the_flags_this_claude_lacks() {
    let h = PluginHost::start().await;
    std::fs::write(
        h.ctl.join("help"),
        "  --agents <json>  Agents\n  --mcp-config <configs...>  MCP\n",
    )
    .unwrap();
    let mut c = h.d.client().await;
    let a = applied(
        c.call(team(&[], &["plugin:a@market", "plugin:b@market"]))
            .await,
    );
    assert!(a.dropped.is_empty(), "{:?}", a.dropped);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manifest_refuses_a_bad_request() {
    let d = TestDaemon::start().await;
    let cache = d.dir.join("cache/plugins");
    // The real daemon: no http, even on loopback.
    let mut c = d.client().await;
    let good = "a".repeat(64);
    let req = |name: &str, url: &str, sha: &str, secrets: Vec<String>, disabled: Vec<String>| {
        Msg::Manifest {
            name: name.into(),
            url: url.into(),
            sha256: sha.into(),
            secrets,
            disabled,
        }
    };
    let url = "https://e.org/x.toml";
    let many_secrets: Vec<String> = (0..65).map(|i| format!("K{i}")).collect();
    let many_ids: Vec<String> = (0..257).map(|i| format!("mcp:s{i}")).collect();
    for (msg, shown) in [
        (req("../x", url, &good, vec![], vec![]), "?"),
        (req("kb", url, "abc", vec![], vec![]), "kb"),
        (
            req("kb", "http://e.org/x.toml", &good, vec![], vec![]),
            "kb",
        ),
        (req("kb", "file:///etc/passwd", &good, vec![], vec![]), "kb"),
        (req("kb", url, &good, vec!["PATH".into()], vec![]), "kb"),
        (req("kb", url, &good, vec!["bad key".into()], vec![]), "kb"),
        (req("kb", url, &good, many_secrets, vec![]), "kb"),
        (req("kb", url, &good, vec![], vec!["skill:x".into()]), "kb"),
        (req("kb", url, &good, vec![], many_ids), "kb"),
    ] {
        let reply = c.call(msg.clone()).await;
        assert_eq!(
            reply,
            Msg::Error {
                message: format!("manifest {shown}: bad request")
            },
            "{msg:?}"
        );
    }
    assert!(!cache.exists(), "nothing written");
}

/// Serve `body` at `/team.toml` on loopback, counting the requests.
async fn serve(body: &'static [u8]) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let hits = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&hits);
    let router = axum::Router::new().route(
        "/team.toml",
        axum::routing::get(move || {
            counted.fetch_add(1, Ordering::SeqCst);
            async move { body }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (format!("http://127.0.0.1:{port}/team.toml"), hits)
}

fn files_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn download(url: &str, sha256: &str) -> Msg {
    Msg::Manifest {
        name: "team".into(),
        url: url.into(),
        sha256: sha256.into(),
        secrets: vec![],
        disabled: vec!["plugin:a@market".into(), "plugin:b@market".into()],
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_download_is_verified_stored_privately_and_shared() {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::Ordering;
    let h = PluginHost::start().await;
    std::fs::remove_dir_all(&h.cache).unwrap();
    let (url, hits) = serve(TEAM.as_bytes()).await;
    let hash = sha256_hex(TEAM.as_bytes());
    let mut a = h.d.client().await;
    let mut b = h.d.client().await;
    let (ra, rb) = tokio::join!(a.call(download(&url, &hash)), b.call(download(&url, &hash)));
    assert_eq!(applied(ra), applied(rb));
    assert_eq!(hits.load(Ordering::SeqCst), 1, "one download for both");
    assert_eq!(
        std::fs::read_to_string(h.cache.join("team.toml")).unwrap(),
        TEAM
    );
    assert_eq!(
        std::fs::read_to_string(h.cache.join("team.sha256")).unwrap(),
        format!("{hash}\n")
    );
    assert_eq!(files_in(&h.cache), ["team.sha256", "team.toml"]);
    let mode = std::fs::metadata(&h.cache).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_wrong_hash_a_bad_manifest_or_an_oversized_one_is_an_error() {
    static BIG: OnceLock<Vec<u8>> = OnceLock::new();
    let h = PluginHost::start().await;
    std::fs::remove_dir_all(&h.cache).unwrap();
    let (url, _) = serve(b"tampered").await;
    let mut c = h.d.client().await;
    assert_eq!(
        c.call(download(&url, &"b".repeat(64))).await,
        Msg::Error {
            message: "manifest team: hash mismatch".into()
        }
    );
    assert!(files_in(&h.cache).is_empty(), "{:?}", files_in(&h.cache));
    // Downloaded and verified, but not a manifest: refused with the reason.
    assert_eq!(
        c.call(download(&url, &sha256_hex(b"tampered"))).await,
        Msg::Error {
            message: "manifest team: not valid TOML, or a value of the wrong type (line 1)".into()
        }
    );
    let other = TEAM.replacen("name = \"team\"", "name = \"other\"", 1);
    let other: &'static [u8] = Box::leak(other.into_bytes().into_boxed_slice());
    let (url, _) = serve(other).await;
    assert_eq!(
        c.call(download(&url, &sha256_hex(other))).await,
        Msg::Error {
            message: "manifest team: the manifest is other's".into()
        }
    );
    let big = BIG.get_or_init(|| vec![b'#'; (256 << 10) + 1]);
    let (url, _) = serve(big).await;
    assert_eq!(
        c.call(download(&url, &sha256_hex(big))).await,
        Msg::Error {
            message: "manifest team: over 256 KiB".into()
        }
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_cache_keeps_at_most_32_manifests() {
    let h = PluginHost::start().await;
    std::fs::remove_dir_all(&h.cache).unwrap();
    std::fs::create_dir_all(&h.cache).unwrap();
    for i in 0..super::manifest::MAX_MANIFESTS {
        std::fs::write(h.cache.join(format!("p{i}.toml")), b"z").unwrap();
    }
    let (url, hits) = serve(TEAM.as_bytes()).await;
    let mut c = h.d.client().await;
    let reply = c.call(download(&url, &sha256_hex(TEAM.as_bytes()))).await;
    assert_eq!(
        reply,
        Msg::Error {
            message: "manifest team: cache holds 32 manifests already".into()
        }
    );
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 0);
}

/// A claude that does not know `--plugin-dir` (the fake's `--help` is
/// empty) still starts: the daemon leaves the pairs out of its argv, and
/// the journal keeps them for a claude that does.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plugin_dirs_are_dropped_for_an_old_claude_but_journaled() {
    let d = TestDaemon::start().await;
    let mut c = d.client().await;
    let id = Uuid::new_v4();
    let mut spec = d.spec(id);
    spec.args = vec!["--plugin-dir".into(), "/c/plugins/kb.zip".into()];
    c.spawn(spec).await;
    c.attach_until(id, BANNER).await;
    assert!(!d.daemon.claude_flags().plugin_dir);
    let entry = &d.journal()["sessions"][0];
    assert_eq!(
        entry["args"],
        serde_json::json!(["--plugin-dir", "/c/plugins/kb.zip"])
    );
}
