//! The daemon test harness, shared by the daemon's tests and the daemon
//! ops tests of addons: a real daemon in a temp dir (`TestDaemon`), fake
//! `claude`s, and a protocol `Client` over the real socket. Every read has a
//! timeout.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tokio::net::UnixStream;
use uuid::Uuid;

pub use super::host::write_executable;
use super::{server, Config, Daemon};
use crate::addon::Addons;
use crate::proto::{self, Envelope, Frame, Hello, Msg, SessionEvent, SessionId, SpawnSpec, PROTO};

/// What the shared fake claude prints first: its session is up.
pub const BANNER: &str = "FAKE-CLAUDE-BANNER";
/// How long any read of the harness waits before it fails the test.
pub const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// Make `dir` (and its parents) and keep it private: 0700 (C4). What a
/// test's fakes write in it may hold env values.
pub fn make_private(dir: &Path) {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .unwrap();
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).unwrap();
}

/// A new private (0700) fixture dir `<temp>/<tag>-<uuid>`.
pub fn private_temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("{tag}-{}", Uuid::new_v4()));
    make_private(&dir);
    dir
}

/// Shell lines for a fake claude that record the named env vars into
/// `out`, never the whole env (C1, E3): a `# env` header line, then
/// `KEY=value` for each of `values` that is set and `KEY=<set>` for each
/// of `present` that is set. A value is recorded only for a key whose
/// value the test sets itself (the daemon's sessions inherit this
/// process's env, a developer's credentials included); any other key a
/// test asserts on goes in `present`. The lines set `umask 077` (every
/// file the fake writes after them is 0600) and write a temp file renamed
/// into place, so a reader never sees half a record.
pub fn record_env_sh(out: &str, values: &[&str], present: &[&str]) -> String {
    let mut sh = String::from("umask 077\n{\necho '# env'\n");
    for (keys, value) in [(values, true), (present, false)] {
        for key in keys {
            assert!(
                super::command::env_remove_ok(&[key.to_string()]).is_ok(),
                "not an env var name: {key}"
            );
            let line = if value {
                format!("printf '%s=%s\\n' {key} \"${key}\"")
            } else {
                format!("echo '{key}=<set>'")
            };
            sh.push_str(&format!("if [ -n \"${{{key}+x}}\" ]; then {line}; fi\n"));
        }
    }
    sh.push_str(&format!(
        "}} > \"{out}.tmp\" && mv \"{out}.tmp\" \"{out}\"\n"
    ));
    sh
}

/// A variant of the fake claude that exits 3 immediately on any real invocation.
/// Used to trigger the commit-before-exit race deterministically.
pub fn fake_claude_exits_immediately() -> &'static Path {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let dir = private_temp_dir("claudio-fake-claude-ei");
        let bin = dir.join("claude");
        let script = "#!/bin/sh\n\
             if [ \"$1\" = \"--version\" ]; then echo '9.9.9 (Claude Code)'; exit 0; fi\n\
             if [ \"$1\" = \"--help\" ]; then exit 0; fi\n\
             exit 3\n";
        crate::daemon::host::write_executable(&bin, script);
        bin
    })
}

/// A variant of the fake claude that exits 1 immediately when given `--resume`
/// (simulating a deleted conversation), and otherwise behaves like the normal
/// fake claude (prints banner + echoes input).
pub fn fake_claude_resume_fails() -> &'static Path {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let dir = private_temp_dir("claudio-fake-claude-rf");
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
        crate::daemon::host::write_executable(&bin, &script);
        bin
    })
}

/// The fake claude, shared by all tests (written once, so no exec races with
/// a file still open for writing). It answers `--version`, saves its
/// `--settings` JSON into its cwd (so tests can read the hook token), prints a
/// banner and echoes input.
pub fn fake_claude() -> &'static Path {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let dir = private_temp_dir("claudio-fake-claude");
        let bin = dir.join("claude");
        let script = format!(
            "#!/bin/sh\n\
             if [ \"$1\" = \"--version\" ]; then echo '9.9.9 (Claude Code)'; exit 0; fi\n\
             if [ \"$1\" = \"--help\" ]; then exit 0; fi\n\
             printf '%s' \"$2\" > settings.json\n\
             echo {BANNER}\n\
             exec cat\n"
        );
        crate::daemon::host::write_executable(&bin, &script);
        bin
    })
}

/// A daemon running in its own temp dir.
pub struct TestDaemon {
    /// Its private temp dir (socket, journal, `work/`), removed on drop.
    pub dir: PathBuf,
    /// The config it was started with.
    pub config: Config,
    /// Its shared state, for tests that drive it directly.
    pub daemon: Arc<Daemon>,
}

impl TestDaemon {
    /// A test config under `dir`, with the shared fake claude.
    pub fn config(dir: &Path) -> Config {
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
            addon_timeout: std::time::Duration::from_secs(150),
            old_peer: false,
        }
    }

    /// A new private temp dir with a `work/` subdir, short enough for a socket path.
    pub fn new_dir() -> PathBuf {
        let dir = crate::paths::socket_test_dir("cl-d");
        make_private(&dir);
        std::fs::create_dir_all(dir.join("work")).unwrap();
        dir
    }

    /// A daemon with the test config, in `dir`.
    pub async fn start_in(dir: PathBuf) -> TestDaemon {
        let config = Self::config(&dir);
        Self::start_with_config(dir, config)
    }

    /// A daemon with `config` and no addons, in `dir`.
    pub fn start_with_config(dir: PathBuf, config: Config) -> TestDaemon {
        Self::start_with_addons(dir, config, Addons::empty())
    }

    /// A daemon with `config` running `addons`' daemon ops, in `dir`.
    pub fn start_with_addons(dir: PathBuf, config: Config, addons: Addons) -> TestDaemon {
        let listening = server::start(config.clone(), Arc::new(addons))
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

    /// A daemon with the test config, in a new temp dir.
    pub async fn start() -> TestDaemon {
        Self::start_in(Self::new_dir()).await
    }

    /// A daemon whose claude is `claude` instead of the shared fake.
    pub async fn start_with_claude(dir: PathBuf, claude: PathBuf) -> TestDaemon {
        let config = Config {
            claude,
            ..Self::config(&dir)
        };
        Self::start_with_config(dir, config)
    }

    /// The sessions' working directory.
    pub fn work(&self) -> PathBuf {
        self.dir.join("work")
    }

    /// A client that has said Hello.
    pub async fn client(&self) -> Client {
        Client::connect(&self.config.socket).await
    }

    /// A spawn of session `id` in `work/`, with one env secret to check redaction against.
    pub fn spec(&self, id: SessionId) -> SpawnSpec {
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
            env_remove: None,
            optional: vec![],
        }
    }

    /// The hook token the fake claude was started with.
    pub async fn token(&self) -> String {
        self.token_other_than("").await
    }

    /// The hook token of a process started after the one with `previous`:
    /// after a respawn, `settings.json` holds the old token until the new
    /// process rewrites it.
    pub async fn token_other_than(&self, previous: &str) -> String {
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

    /// The journal as written on disk.
    pub fn journal(&self) -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(&self.config.journal).unwrap()).unwrap()
    }
}

impl Drop for TestDaemon {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// `f`, or a panic after [`READ_TIMEOUT`].
pub async fn within<T>(f: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(READ_TIMEOUT, f)
        .await
        .expect("timed out")
}

/// A protocol client over a real socket.
pub struct Client {
    /// The connection.
    pub stream: UnixStream,
    /// The last request id sent.
    pub next_req: u64,
}

impl Client {
    /// Connected, before any Hello.
    pub async fn raw(socket: &Path) -> Client {
        Client {
            stream: UnixStream::connect(socket).await.unwrap(),
            next_req: 0,
        }
    }

    /// Connected, and welcomed at this build's protocol version.
    pub async fn connect(socket: &Path) -> Client {
        let mut c = Self::raw(socket).await;
        match c.call(Msg::Hello(hello(PROTO))).await {
            Msg::Welcome(w) => assert_eq!(w.proto, PROTO),
            other => panic!("expected welcome, got {other:?}"),
        }
        c
    }

    /// Write one frame.
    pub async fn send(&mut self, frame: Frame) {
        proto::write_frame(&mut self.stream, &frame).await.unwrap();
    }

    /// Send `msg` as the next request; its id.
    pub async fn request(&mut self, msg: Msg) -> u64 {
        self.next_req += 1;
        self.send(Frame::Control(Envelope::request(self.next_req, msg)))
            .await;
        self.next_req
    }

    /// The next frame (`None` at the end), within the read timeout.
    pub async fn recv(&mut self) -> Option<Frame> {
        within(proto::read_frame(&mut self.stream)).await.unwrap()
    }

    /// Read until `pick` accepts a frame. On a timeout, the panic lists the
    /// last frames that did arrive, so a failure says what was seen instead.
    pub async fn until<T>(&mut self, mut pick: impl FnMut(&Frame) -> Option<T>) -> T {
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
            Err(_) => panic!(
                "timed out; last frames seen:\n{}",
                Vec::from(seen).join("\n")
            ),
        }
    }

    /// Send a request and wait for its reply.
    pub async fn call(&mut self, msg: Msg) -> Msg {
        let req = self.request(msg).await;
        self.until(|f| match f {
            Frame::Control(env) if env.req == Some(req) => Some(env.msg.clone()),
            _ => None,
        })
        .await
    }

    /// Wait for a session event matching `pick`.
    pub async fn event<T>(
        &mut self,
        id: SessionId,
        pick: impl Fn(&SessionEvent) -> Option<T>,
    ) -> T {
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
    pub async fn output_until(&mut self, id: SessionId, needle: &str) -> String {
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
    pub async fn attach(&mut self, id: SessionId, rows: u16, cols: u16) -> String {
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
    pub async fn attach_until(&mut self, id: SessionId, needle: &str) {
        if !self.attach(id, 24, 80).await.contains(needle) {
            self.output_until(id, needle).await;
        }
    }

    /// Spawn `spec`; the pid the daemon reports.
    pub async fn spawn(&mut self, spec: SpawnSpec) -> Option<u32> {
        match self.call(Msg::Spawn(spec)).await {
            Msg::Spawned { pid, .. } => pid,
            other => panic!("spawn failed: {other:?}"),
        }
    }

    /// The daemon's session list.
    pub async fn sessions(&mut self) -> Vec<crate::proto::SessionInfo> {
        match self.call(Msg::ListSessions).await {
            Msg::Sessions { sessions } => sessions,
            other => panic!("expected sessions, got {other:?}"),
        }
    }
}

/// A test client's Hello at protocol `proto`.
pub fn hello(proto: u32) -> Hello {
    Hello {
        claudio_version: "test".into(),
        proto,
        colors: None,
    }
}

/// Send hook `event` as the relay does (`__hook`), authenticated by `token`.
pub async fn send_hook(socket: &Path, token: &str, event: &str, payload: serde_json::Value) {
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
