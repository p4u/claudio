//! Fixtures for the Vocdoni addon's tests: the manifest the daemon tests
//! apply (`TEAM`), the fake plugin `claude` and installer, a daemon host
//! for them (`PluginHost`), and a loopback manifest server (`serve`).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use serde_json::Value;

use super::daemon::applied::apply_msg;
use super::daemon::{retrying, Applied, ApplyRequest, Manifests, VocdoniDaemon};
use super::plugins::state::applied as applied_for;
use super::Vocdoni;
use crate::addon::{
    Addon, AddonCtx, AddonError, Addons, BoxFuture, ConfigEdit, HostSnap, SessionSnap,
};
use crate::daemon::test_support::{private_temp_dir, within, TestDaemon};
use crate::daemon::Config;
use crate::proto::Msg;

/// The manifest the `Manifest` tests apply: one marketplace with plugins
/// `a` and `b`, servers `kb` (secret `KB_TOKEN`), `mine` and `jina`
/// (secret `JINA_KEY`), agents `reviewer` and `helper`, instructions, env.
pub(crate) const TEAM: &str = r#"
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
pub(crate) fn fake_plugin_claude() -> &'static Path {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let dir = private_temp_dir("claudio-fake-plugins");
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
        crate::daemon::test_support::write_executable(&bin, script);
        bin
    })
}

/// The name of a claude that is installed only during the test (never a
/// real one: no such file in any directory claude is looked up in).
pub(crate) const LATE_CLAUDE: &str = "claude-late";

/// A directory for a `PATH` with claude's official installer faked, shared
/// (written once): `sh`, `bash`, `cat`, `cp`, `mkdir` and `sleep` link to
/// the system's, and `curl` logs its arguments to `<ctl>/log` (`<ctl>`:
/// the `PATH`'s first entry) and prints a script that copies the fake
/// plugin claude to `<ctl>/claude-late`. No real `curl` is on such a
/// `PATH`, so the real installer can never run.
pub(crate) fn fake_installer_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = private_temp_dir("claudio-fake-installer");
        for tool in ["sh", "bash", "cat", "cp", "mkdir", "sleep"] {
            let real = ["/usr/bin", "/bin"]
                .iter()
                .map(|d| Path::new(d).join(tool))
                .find(|p| p.is_file())
                .unwrap_or_else(|| panic!("no {tool} in /usr/bin or /bin"));
            std::os::unix::fs::symlink(real, dir.join(tool)).unwrap();
        }
        let script = format!(
            "#!/bin/sh\n\
             ctl=\"${{PATH%%:*}}\"\n\
             echo \"curl $*\" >> \"$ctl/log\"\n\
             echo \"cp '{}' '$ctl/{LATE_CLAUDE}'\"\n\
             echo 'echo claude installed'\n",
            fake_plugin_claude().display()
        );
        std::fs::write(dir.join("curl"), script).unwrap();
        std::fs::set_permissions(
            dir.join("curl"),
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
        dir
    })
}

/// A `Manifest` test's daemon: the fake plugin claude, manifests cached in
/// `<dir>/cache/plugins` (TEAM there already), Claude Code's config in
/// `<dir>/claude`, and the fake's control dir `<dir>/ctl`.
pub(crate) struct PluginHost {
    pub(crate) d: TestDaemon,
    pub(crate) cache: PathBuf,
    pub(crate) claude: PathBuf,
    pub(crate) ctl: PathBuf,
    /// A daemon-side addon beside the plugins: counts `on_claude_installed`.
    pub(crate) echo: Arc<crate::addon::test_support::TestAddon>,
    /// The Vocdoni addon's daemon state: what the installs did.
    pub(crate) manifests: Arc<Manifests>,
}

impl PluginHost {
    pub(crate) async fn start() -> PluginHost {
        Self::start_with(|ctl| {
            let path = format!("{}:/usr/bin:/bin", ctl.display());
            (fake_plugin_claude().to_path_buf(), path)
        })
        .await
    }

    /// With the claude and `PATH` that `setup` gives for the control dir.
    pub(crate) async fn start_with(setup: impl FnOnce(&Path) -> (PathBuf, String)) -> PluginHost {
        let dir = TestDaemon::new_dir();
        let (cache, claude, ctl) = (
            dir.join("cache/vocdoni"),
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
        let (bin, path) = setup(&ctl);
        let config = Config {
            claude: bin,
            path: Some(path.into()),
            ..TestDaemon::config(&dir)
        };
        let echo = Arc::new(crate::addon::test_support::TestAddon::new("echo").with_daemon());
        let ops = VocdoniDaemon::with_dirs(cache.clone(), claude.clone());
        let manifests = Arc::clone(ops.manifests());
        let vocdoni: Arc<dyn Addon> = Arc::new(Vocdoni::with_daemon(ops));
        let addons = Addons::new(vec![vocdoni, Arc::clone(&echo) as Arc<dyn Addon>]);
        let d = TestDaemon::start_with_addons(dir, config, addons.unwrap());
        PluginHost {
            d,
            cache,
            claude,
            ctl,
            echo,
            manifests,
        }
    }

    /// Create (or with `on` false, remove) control file `name`.
    pub(crate) fn set(&self, name: &str, on: bool) {
        let path = self.ctl.join(name);
        if on {
            std::fs::write(path, "").unwrap();
        } else {
            let _ = std::fs::remove_file(path);
        }
    }

    /// The fake's `plugin …` runs so far.
    pub(crate) fn log(&self) -> Vec<String> {
        std::fs::read_to_string(self.ctl.join("log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// Wait until the log has a line equal to `line`.
    pub(crate) async fn logged(&self, line: &str) {
        within(async {
            while !self.log().iter().any(|l| l == line) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
    }

    /// Wait until no install of manifest TEAM is queued or running.
    pub(crate) async fn idle(&self) {
        within(async {
            while self.manifests.busy() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
    }
}

/// A `vocdoni/apply` request for TEAM (cached; its URL is never reached).
pub(crate) fn team(secrets: &[&str], disabled: &[&str]) -> Msg {
    apply_msg(&ApplyRequest {
        name: "team".into(),
        url: "https://127.0.0.1:9/never.toml".into(),
        sha256: sha256_hex(TEAM.as_bytes()),
        secrets: secrets.iter().map(|s| s.to_string()).collect(),
        disabled: disabled.iter().map(|s| s.to_string()).collect(),
    })
}

/// The `Applied` of a reply to an apply request.
pub(crate) fn applied(reply: Msg) -> Applied {
    match reply {
        Msg::AddonReply { body } => serde_json::from_value(body).expect("an Applied body"),
        other => panic!("expected an AddonReply, got {other:?}"),
    }
}

pub(crate) fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

/// `item` is `id` skipped for `why`, tried again in `secs` s (or one less:
/// the clock moved on since the failure).
pub(crate) fn assert_retrying(item: &(String, String), id: &str, why: &str, secs: u64) {
    let due = (secs.saturating_sub(1)..=secs).any(|s| item.1 == retrying(why, s));
    assert!(item.0 == id && due, "{item:?}: want {id}, {why}, {secs} s");
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(bytes))
}

/// Serve `body` at `/team.toml` on loopback, counting the requests.
pub(crate) async fn serve(body: &'static [u8]) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
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

pub(crate) fn files_in(dir: &Path) -> Vec<String> {
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

pub(crate) fn download(url: &str, sha256: &str) -> Msg {
    apply_msg(&ApplyRequest {
        name: "team".into(),
        url: url.into(),
        sha256: sha256.into(),
        secrets: vec![],
        disabled: vec!["plugin:a@market".into(), "plugin:b@market".into()],
    })
}

/// A context that records what the runner asks of it: requests answer
/// with `applied(<name>)`, config edits apply to `config`.
#[derive(Default)]
pub(crate) struct TestCtx {
    /// Whether `host_has` says no (a daemon older than addons).
    pub(crate) old: AtomicBool,
    pub(crate) requests: Mutex<Vec<(String, String, Value)>>,
    pub(crate) config: Mutex<toml::Table>,
    pub(crate) notices: Mutex<Vec<String>>,
    pub(crate) changed: AtomicUsize,
    pub(crate) offers: Mutex<Vec<String>>,
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl AddonCtx for TestCtx {
    fn addon(&self) -> &'static str {
        "vocdoni"
    }
    fn spawn(&self, task: BoxFuture<'static, ()>) {
        tokio::spawn(task);
    }
    fn http(&self) -> reqwest::Client {
        reqwest::Client::new()
    }
    fn request(
        &self,
        host: &str,
        op: &str,
        body: Value,
    ) -> BoxFuture<'static, Result<Value, AddonError>> {
        let name = body["name"].as_str().unwrap_or("").to_owned();
        lock(&self.requests).push((host.to_owned(), op.to_owned(), body));
        Box::pin(async move { Ok(serde_json::to_value(applied_for(&name)).unwrap()) })
    }
    fn host_has(&self, _host: &str) -> bool {
        !self.old.load(Ordering::SeqCst)
    }
    fn sessions(&self) -> Vec<SessionSnap> {
        Vec::new()
    }
    fn hosts(&self) -> Vec<HostSnap> {
        Vec::new()
    }
    fn config(&self, section: &str) -> toml::Table {
        match lock(&self.config).get(section) {
            Some(toml::Value::Table(t)) => t.clone(),
            _ => toml::Table::new(),
        }
    }
    fn edit_config(
        &self,
        section: &str,
        edit: ConfigEdit,
    ) -> BoxFuture<'static, Result<(), String>> {
        let mut config = lock(&self.config);
        let entry = config
            .entry(section.to_owned())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        if let toml::Value::Table(t) = entry {
            edit(t);
        }
        Box::pin(async { Ok(()) })
    }
    fn extension_on(&self, _name: &str) -> Option<bool> {
        None
    }
    fn notify(&self, text: String) {
        lock(&self.notices).push(text);
    }
    fn changed(&self) {
        self.changed.fetch_add(1, Ordering::SeqCst);
    }
    fn offer_checklist(&self, id: String) {
        lock(&self.offers).push(id);
    }
}
