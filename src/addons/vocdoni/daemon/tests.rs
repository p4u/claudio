//! The daemon side of the Vocdoni addon against a real daemon: manifests
//! applied, plugins installed with `claude plugin …`, downloads verified
//! and cached.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use super::{apply_msg, ApplyRequest, VocdoniDaemon};
use crate::addon::{Addon, Addons};
use crate::addons::vocdoni::test_support::*;
use crate::addons::vocdoni::Vocdoni;
use crate::daemon::test_support::{within, TestDaemon};
use crate::daemon::Config;
use crate::proto::Msg;

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
    h.manifests.advance(Duration::from_secs(14));
    let a = applied(c.call(team(&[], &[])).await);
    assert_retrying(&a.skipped[0], "plugin:a@market", "network down", 1);
    h.idle().await;
    assert_eq!(installs(&h, "a@market"), 1);
    h.manifests.advance(Duration::from_secs(1));
    let a = applied(c.call(team(&[], &[])).await);
    assert_eq!(a.skipped[0].1, "installing");
    h.idle().await;
    assert_eq!(installs(&h, "a@market"), 2);
    let a = applied(c.call(team(&[], &[])).await);
    assert_retrying(&a.skipped[0], "plugin:a@market", "network down", 30);

    // Then it works; the manual one still waits for a person.
    h.set("fail-a@market", false);
    h.manifests.advance(Duration::from_secs(30));
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
    h.manifests.advance(Duration::from_secs(10));
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
    h.manifests.advance(Duration::from_secs(5));
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
/// Claude installed on the host: the failures are forgotten, so the next
/// request queues those installs at once; a manual one still waits.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forgotten_failures_install_on_the_next_request() {
    let h = PluginHost::start().await;
    h.set("fail-a@market", true);
    h.set("manual-b@market", true);
    let mut c = h.d.client().await;
    c.call(team(&[], &[])).await;
    h.idle().await;
    let a = applied(c.call(team(&[], &[])).await);
    assert_retrying(&a.skipped[0], "plugin:a@market", "network down", 15);
    h.set("fail-a@market", false);
    h.manifests.forget_failures();
    let a = applied(c.call(team(&[], &[])).await);
    let manual = "needs manual install: claude plugin install b@market";
    assert_eq!(
        &a.skipped[..2],
        &pairs(&[
            ("plugin:a@market", "installing"),
            ("plugin:b@market", manual)
        ])[..]
    );
    h.idle().await;
    let a = applied(c.call(team(&[], &[])).await);
    assert_eq!(a.installed, ["plugin:a@market"]);
}
/// A claude installed after the plugins' first request: until then each
/// plugin install fails at once (`claude is not installed`, never holding
/// the lock the installer takes). The installer (a fake `curl`: no real
/// one is on this `PATH`) puts claude in place, the failures are
/// forgotten, and the next request installs the plugins.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn installing_claude_forgets_the_plugin_failures() {
    let tools = fake_installer_dir();
    let h = PluginHost::start_with(|ctl| {
        let path = format!("{}:{}", ctl.display(), tools.display());
        (PathBuf::from(LATE_CLAUDE), path)
    })
    .await;
    let mut c = h.d.client().await;
    c.call(team(&[], &[])).await;
    h.idle().await;
    let a = applied(c.call(team(&[], &[])).await);
    let why = "marketplace market: claude is not installed";
    assert_retrying(&a.skipped[0], "plugin:a@market", why, 15);
    assert_retrying(&a.skipped[1], "plugin:b@market", why, 15);
    assert!(h.log().is_empty(), "{:?}", h.log());
    assert_eq!(h.echo.installs(), 0);

    match c.call(Msg::UpdateClaude { install: true }).await {
        Msg::ClaudeUpdated { version, ok, tail } => {
            assert!(ok, "{tail}");
            assert_eq!(version.as_deref(), Some("9.9.9 (Claude Code)"));
            assert_eq!(tail, "claude installed");
        }
        other => panic!("expected ClaudeUpdated, got {other:?}"),
    }
    assert_eq!(h.log(), ["curl -fsSL https://claude.ai/install.sh"]);
    // Every daemon-side addon hears it (claude_install_calls_on_claude_installed).
    assert_eq!(h.echo.installs(), 1);
    let a = applied(c.call(team(&[], &[])).await);
    assert_eq!(a.skipped[0].1, "installing");
    h.idle().await;
    let a = applied(c.call(team(&[], &[])).await);
    assert_eq!(a.installed, ["plugin:a@market", "plugin:b@market"]);
}
/// The installer does not run without `curl` on the daemon's `PATH`, nor
/// for a claude that `CLAUDIO_CLAUDE_PATH` pins: the reply says why.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn install_says_why_it_cannot_run() {
    let dir = TestDaemon::new_dir();
    let empty = dir.join("bin");
    std::fs::create_dir_all(&empty).unwrap();
    let config = Config {
        claude: PathBuf::from(LATE_CLAUDE),
        path: Some(empty.into_os_string()),
        ..TestDaemon::config(&dir)
    };
    let d = TestDaemon::start_with_config(dir, config);
    let mut c = d.client().await;
    assert_eq!(
        c.call(Msg::UpdateClaude { install: true }).await,
        Msg::ClaudeUpdated {
            version: None,
            ok: false,
            tail: "claude installer needs curl".into()
        }
    );

    let pinned = TestDaemon::start_in(TestDaemon::new_dir()).await;
    let mut c = pinned.client().await;
    match c.call(Msg::UpdateClaude { install: true }).await {
        Msg::ClaudeUpdated {
            ok: false, tail, ..
        } => {
            assert!(tail.starts_with("CLAUDIO_CLAUDE_PATH is /"), "{tail}");
        }
        other => panic!("expected ClaudeUpdated, got {other:?}"),
    }
}
/// A claude that is there but does not answer `--version` (as `Welcome`
/// sees on a slow box) is never installed over: no `curl` runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn install_never_runs_over_a_present_claude() {
    let tools = fake_installer_dir();
    let h = PluginHost::start_with(|ctl| {
        let present = ctl.parent().unwrap().join("present");
        std::fs::create_dir_all(&present).unwrap();
        let bin = present.join(LATE_CLAUDE);
        std::fs::write(&bin, "#!/bin/sh\nexit 1\n").unwrap();
        let exec = std::os::unix::fs::PermissionsExt::from_mode(0o755);
        std::fs::set_permissions(&bin, exec).unwrap();
        let (c, t, p) = (ctl.display(), tools.display(), present.display());
        let path = format!("{c}:{t}:{p}");
        (PathBuf::from(LATE_CLAUDE), path)
    })
    .await;
    let mut c = h.d.client().await;
    match c.call(Msg::UpdateClaude { install: true }).await {
        Msg::ClaudeUpdated {
            ok: false, tail, ..
        } => assert!(tail.starts_with("claude is at "), "{tail}"),
        other => panic!("expected ClaudeUpdated, got {other:?}"),
    }
    assert!(h.log().is_empty(), "{:?}", h.log());
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
    let dir = TestDaemon::new_dir();
    let cache = dir.join("cache/vocdoni");
    // As a real daemon: no http, even on loopback.
    let ops = VocdoniDaemon::https_only(cache.clone(), dir.join("claude"));
    let vocdoni: Arc<dyn Addon> = Arc::new(Vocdoni::with_daemon(ops));
    let config = TestDaemon::config(&dir);
    let d = TestDaemon::start_with_addons(dir, config, Addons::new(vec![vocdoni]).unwrap());
    let mut c = d.client().await;
    let good = "a".repeat(64);
    let req = |name: &str, url: &str, sha: &str, secrets: Vec<String>, disabled: Vec<String>| {
        apply_msg(&ApplyRequest {
            name: name.into(),
            url: url.into(),
            sha256: sha.into(),
            secrets,
            disabled,
        })
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
    for i in 0..super::MAX_MANIFESTS {
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

/// compat(v0.10): an old client's `manifest` op (the raw JSON of the
/// releases before addons) is refused as unsupported, which that client
/// reports as "daemon too old" and starts its session without plugins
/// (AC8, old client → new daemon).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_manifest_op_from_an_old_client_is_refused_as_unsupported() {
    use tokio::io::AsyncWriteExt;
    let h = PluginHost::start().await;
    let mut c = h.d.client().await;
    let json = format!(
        r#"{{"req":77,"op":"manifest","name":"team","url":"https://127.0.0.1:9/never.toml","sha256":"{}","secrets":[],"disabled":[]}}"#,
        sha256_hex(TEAM.as_bytes())
    );
    let mut frame = ((json.len() + 1) as u32).to_be_bytes().to_vec();
    frame.push(b'J');
    frame.extend_from_slice(json.as_bytes());
    c.stream.write_all(&frame).await.unwrap();
    let reply = c
        .until(|f| match f {
            crate::proto::Frame::Control(env) if env.req == Some(77) => Some(env.msg.clone()),
            _ => None,
        })
        .await;
    match reply {
        Msg::Error { message } => assert!(message.starts_with("unsupported op"), "{message}"),
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert!(h.log().is_empty(), "nothing was installed");
}

/// A daemon with the Vocdoni addon lists `addon:vocdoni` in its features.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn welcome_lists_addon_vocdoni() {
    let h = PluginHost::start().await;
    let mut c = crate::daemon::test_support::Client::raw(&h.d.config.socket).await;
    let hello = crate::daemon::test_support::hello(crate::proto::PROTO);
    let Msg::Welcome(welcome) = c.call(Msg::Hello(hello)).await else {
        panic!("no welcome");
    };
    assert!(
        welcome.features.iter().any(|f| f == "addon:vocdoni"),
        "{:?}",
        welcome.features
    );
    assert!(super::serves_apply(&welcome.features));
}
