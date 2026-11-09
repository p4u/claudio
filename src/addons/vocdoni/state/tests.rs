//! The plugin state machine's tests, ported from the TUI's plugin glue
//! (`tui/plugins_glue.rs`; same names): they assert on the returned
//! [`Task`]s where those asserted on `Effect`s.

use std::collections::{BTreeMap, BTreeSet};

use super::*;
use crate::addon::AddonError;
use crate::addons::vocdoni::daemon::applied::apply_msg;
use crate::addons::vocdoni::daemon::Applied;
use crate::addons::vocdoni::plugins::manifest::Manifest;
use crate::addons::vocdoni::plugins::state::{applied, plugins_part, ApplyState, PluginsFetch};
use crate::addons::vocdoni::proxy::plugins::{sample, Plugin};

fn list(profile: &str, plugins: Vec<Plugin>) -> PluginList {
    PluginList {
        profile: profile.into(),
        fetched_at: 100,
        plugins,
    }
}

/// The manifest plugin `name` serves in these tests: a plugin, a server
/// needing `KB_TOKEN`, instructions and env.
fn manifest_of(name: &str) -> Manifest {
    let src = format!(
        r#"
format = 1
name = "{name}"
version = "1.4.0"
description = "The knowledge base"
instructions = "Search the KB."

[[marketplace]]
name = "m"
source = "github:o/r"

[[plugin]]
name = "p"
marketplace = "m"

[[mcp]]
name = "kb"
type = "http"
url = "https://kb.example.org/mcp"
headers = {{ Authorization = "Bearer ${{KB_TOKEN}}" }}
secrets = ["KB_TOKEN"]

[env]
KB_REGION = "eu"
"#
    );
    Manifest::parse(&src).unwrap()
}

fn sha() -> String {
    "a".repeat(64)
}

/// An apply request: host, name, secrets, disabled.
type Asked = (String, String, Vec<String>, Vec<String>);

/// The `vocdoni/apply` requests among `tasks`.
fn asks(tasks: &[Task]) -> Vec<Asked> {
    tasks
        .iter()
        .filter_map(|t| match t {
            Task::Ask { host, body, .. } => Some((
                host.clone(),
                body.name.clone(),
                body.secrets.clone(),
                body.disabled.clone(),
            )),
            _ => None,
        })
        .collect()
}

/// The hosts asked, as (host, name), sorted.
fn asked(tasks: &[Task]) -> Vec<(String, String)> {
    let mut asked: Vec<(String, String)> =
        asks(tasks).into_iter().map(|(h, n, _, _)| (h, n)).collect();
    asked.sort();
    asked
}

fn pair(host: &str, name: &str) -> (String, String) {
    (host.to_owned(), name.to_owned())
}

fn notices(tasks: &[Task]) -> Vec<String> {
    tasks
        .iter()
        .filter_map(|t| match t {
            Task::Notice(text) => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn listed(s: &mut State, profile: &str, plugins: Vec<Plugin>) -> Vec<Task> {
    s.on_list(profile, PluginsFetch::Listed(list(profile, plugins)))
}

fn ok(a: Applied) -> Result<Applied, AddonError> {
    Ok(a)
}

fn daemon_error(text: &str) -> Result<Applied, AddonError> {
    Err(AddonError::Daemon(text.into()))
}

#[test]
fn a_list_fetches_its_manifests_and_hosts_are_asked_once_they_arrive() {
    let mut s = State::default();
    assert!(asked(&s.host_up("local", true)).is_empty(), "no list yet");
    let tasks = listed(&mut s, "work", vec![sample("kb")]);
    let fetched: Vec<&Task> = tasks
        .iter()
        .filter(|t| matches!(t, Task::FetchManifest { name, .. } if name == "kb"))
        .collect();
    assert_eq!(fetched.len(), 1, "{tasks:?}");
    assert!(asks(&tasks).is_empty(), "nothing asked before the manifest");
    s.plugins.choices.insert(
        "kb".into(),
        PluginChoices {
            disabled: vec!["mcp:kb".into()],
            ..PluginChoices::default()
        },
    );
    let tasks = s.on_manifest("kb", &sha(), Ok(manifest_of("kb")));
    assert_eq!(
        asks(&tasks),
        [(
            "local".to_owned(),
            "kb".to_owned(),
            vec!["KB_TOKEN".to_owned()],
            vec!["mcp:kb".to_owned()]
        )]
    );
    // A second host is asked on connect; the same list again asks no one
    // and fetches nothing.
    assert_eq!(asked(&s.host_up("devbox", true)), [pair("devbox", "kb")]);
    let tasks = listed(&mut s, "work", vec![sample("kb")]);
    assert_eq!(tasks, [Task::Changed]);
    // A new version is fetched; the hosts are asked once it is here.
    let mut v2 = sample("kb");
    v2.bundle_sha256 = "b".repeat(64);
    assert!(asked(&listed(&mut s, "work", vec![v2])).is_empty());
    let tasks = s.on_manifest("kb", &sha(), Ok(manifest_of("kb")));
    assert!(tasks.is_empty(), "stale: the old hash");
    let tasks = s.on_manifest("kb", &"b".repeat(64), Ok(manifest_of("kb")));
    assert_eq!(asked(&tasks), [pair("devbox", "kb"), pair("local", "kb")]);
}

#[test]
fn a_refused_manifest_is_asked_of_no_host_and_shown() {
    let mut s = State::default();
    s.host_up("local", true);
    listed(&mut s, "work", vec![sample("kb")]);
    let tasks = s.on_manifest("kb", &sha(), Err("hash mismatch".into()));
    assert!(asked(&tasks).is_empty());
    let details = s.plugin_details("kb", "work", None);
    assert_eq!(details[1], "manifest: hash mismatch");
}

/// `kb` listed, its manifest here and its choices saved; no host yet.
fn state_with_kb_unasked() -> State {
    let mut s = State::default();
    s.plugins
        .choices
        .insert("kb".into(), PluginChoices::default());
    listed(&mut s, "work", vec![sample("kb")]);
    s.on_manifest("kb", &sha(), Ok(manifest_of("kb")));
    s
}

/// A daemon whose Welcome lacks `addon:vocdoni` (older than addons) is
/// never asked: every plugin is `TooOld` there at once and a spawn does not
/// wait for it. Once a newer daemon connects there, it is asked.
#[test]
fn a_host_without_addon_vocdoni_is_too_old_and_never_asked() {
    let mut s = state_with_kb_unasked();
    assert!(asks(&s.host_up("local", false)).is_empty(), "never asked");
    assert_eq!(s.apply_state("local", "kb"), Some(&ApplyState::TooOld));
    let (parts, pending) = s.plugin_parts("local", "work", &[]);
    assert!(pending.is_empty(), "a spawn does not wait for it");
    assert_eq!(parts[0].applied, None);
    assert_eq!(
        parts[0].secrets,
        [("KB_TOKEN".to_owned(), "s3cret".to_owned())]
    );
    assert_eq!(asked(&s.host_up("local", true)), [pair("local", "kb")]);
}

/// C1: the apply request on the wire names the plugin's secrets, never
/// their values.
#[test]
fn apply_bodies_carry_secret_names_never_values() {
    let mut s = state_with_kb_unasked();
    let bodies: Vec<ApplyRequest> = s
        .host_up("local", true)
        .into_iter()
        .filter_map(|t| match t {
            Task::Ask { body, .. } => Some(body),
            _ => None,
        })
        .collect();
    assert_eq!(bodies.len(), 1);
    let msg = apply_msg(&bodies[0]);
    let frame = serde_json::to_string(&crate::proto::Envelope::request(1, msg)).unwrap();
    assert!(frame.contains(r#""secrets":["KB_TOKEN"]"#), "{frame}");
    assert!(!frame.contains("s3cret"), "a secret value on the wire");
}

/// `local` connected and profile `work` serving `kb`, set up, whose
/// manifest is here and which `local` answered with `answer` (`None`: not
/// yet).
fn state_with_kb(answer: Option<Result<Applied, AddonError>>) -> State {
    let mut s = State::default();
    s.plugins
        .choices
        .insert("kb".into(), PluginChoices::default());
    s.host_up("local", true);
    listed(&mut s, "work", vec![sample("kb")]);
    s.on_manifest("kb", &sha(), Ok(manifest_of("kb")));
    if let Some(answer) = answer {
        s.on_applied("local", "kb", &sha(), answer);
    }
    s
}

#[test]
fn answers_set_each_hosts_state_and_stale_ones_are_dropped() {
    let mut s = state_with_kb(None);
    assert_eq!(s.apply_state("local", "kb"), Some(&ApplyState::Pending));
    s.on_applied("local", "kb", &"b".repeat(64), ok(applied("kb")));
    assert_eq!(s.apply_state("local", "kb"), Some(&ApplyState::Pending));
    s.on_applied("local", "kb", &sha(), ok(applied("kb")));
    assert_eq!(
        s.apply_state("local", "kb"),
        Some(&ApplyState::Ready(Box::new(applied("kb"))))
    );
    s.on_applied(
        "local",
        "kb",
        &sha(),
        daemon_error("unsupported op: manifest"),
    );
    assert_eq!(s.apply_state("local", "kb"), Some(&ApplyState::TooOld));
    let missing = Err(AddonError::Missing("local".into()));
    s.on_applied("local", "kb", &sha(), missing);
    assert_eq!(s.apply_state("local", "kb"), Some(&ApplyState::TooOld));
    s.on_applied(
        "local",
        "kb",
        &sha(),
        daemon_error("manifest kb: format is not 1"),
    );
    assert_eq!(
        s.apply_state("local", "kb"),
        Some(&ApplyState::Failed("format is not 1".into()))
    );
    s.on_applied("local", "kb", &sha(), Err(AddonError::Timeout));
    assert_eq!(
        s.apply_state("local", "kb"),
        Some(&ApplyState::Failed("timed out".into()))
    );
    // A failure is asked again on the next occasion; an answer is not.
    let mut tasks = Vec::new();
    s.request_all(&mut tasks);
    assert_eq!(asked(&tasks), [pair("local", "kb")]);
    s.on_applied("local", "kb", &sha(), ok(applied("kb")));
    let mut tasks = Vec::new();
    s.request_all(&mut tasks);
    assert!(asked(&tasks).is_empty());
    // A host that left forgets its answers.
    s.host_down("local");
    assert_eq!(s.apply_state("local", "kb"), None);
}

#[test]
fn a_host_is_asked_again_while_it_installs_or_until_a_retry_is_due() {
    let answer = |why: &str| {
        let mut a = applied("kb");
        a.skipped = vec![("plugin:p@m".into(), why.into())];
        a
    };
    let installing = answer("installing");
    let mut s = State {
        now: 1000,
        ..State::default()
    };
    s.plugins
        .choices
        .insert("kb".into(), PluginChoices::default());
    s.host_up("local", true);
    listed(&mut s, "work", vec![sample("kb")]);
    s.on_manifest("kb", &sha(), Ok(manifest_of("kb")));
    s.on_applied("local", "kb", &sha(), ok(installing.clone()));
    assert!(asked(&s.tick(1029)).is_empty());
    assert_eq!(asked(&s.tick(1030)), [pair("local", "kb")]);
    // The last answer stays meanwhile.
    assert_eq!(
        s.apply_state("local", "kb"),
        Some(&ApplyState::Ready(Box::new(installing.clone())))
    );
    // As long as it installs.
    s.on_applied("local", "kb", &sha(), ok(installing));
    assert_eq!(asked(&s.tick(1030 + 30 * 60)), [pair("local", "kb")]);

    // A failed install: a second after its retry is due...
    let asked_at = s.now;
    let later = answer("network down (retrying in 120 s)");
    s.on_applied("local", "kb", &sha(), ok(later));
    assert!(asked(&s.tick(asked_at + 120)).is_empty());
    assert_eq!(asked(&s.tick(asked_at + 121)), [pair("local", "kb")]);
    // ...and no sooner than 30 s.
    let asked_at = s.now;
    let soon = answer("network down (retrying in 5 s)");
    s.on_applied("local", "kb", &sha(), ok(soon));
    assert!(asked(&s.tick(asked_at + 29)).is_empty());
    assert_eq!(asked(&s.tick(asked_at + 30)), [pair("local", "kb")]);

    // Installed, or waiting for a person: no more asking.
    for settled in [
        applied("kb"),
        answer("needs manual install: claude plugin install p@m"),
    ] {
        s.on_applied("local", "kb", &sha(), ok(settled));
        let later = s.now + 3600;
        assert!(asked(&s.tick(later)).is_empty());
    }
}

#[test]
fn claude_installed_on_a_host_asks_it_again_for_every_manifest() {
    let mut s = State::default();
    s.plugins
        .choices
        .insert("kb".into(), PluginChoices::default());
    s.host_up("local", true);
    s.host_up("devbox", true);
    listed(&mut s, "work", vec![sample("kb")]);
    s.on_manifest("kb", &sha(), Ok(manifest_of("kb")));
    for host in ["local", "devbox"] {
        s.on_applied(host, "kb", &sha(), ok(applied("kb")));
    }
    assert_eq!(asked(&s.claude_installed("devbox")), [pair("devbox", "kb")]);
    assert_eq!(
        s.apply_state("devbox", "kb"),
        Some(&ApplyState::Ready(Box::new(applied("kb")))),
        "the answer stays until the new one comes"
    );
    assert!(s.claude_installed("far").is_empty(), "not connected");
}

/// A spawn's args, env, instructions, and the manifests it waits for.
type Spawned = (Vec<String>, Vec<(String, String)>, String, Vec<String>);

/// A claude spawn with args `-c` on `local` with `profile`'s share.
fn spawn_with(s: &State, profile: Option<&str>) -> Spawned {
    let (parts, pending) = profile
        .map(|p| s.plugin_parts("local", p, &[]))
        .unwrap_or_default();
    let c = plugins_part(&parts);
    let args = std::iter::once("-c".to_owned()).chain(c.args).collect();
    let waits = pending.into_iter().map(|(name, _)| name).collect();
    (args, c.env, c.instructions, waits)
}

#[test]
fn a_spawn_gets_the_answered_plugins_flags_env_and_instructions() {
    let s = state_with_kb(Some(ok(applied("kb"))));
    let (args, env, instructions, waits) = spawn_with(&s, Some("work"));
    assert_eq!(
        args,
        [
            "-c",
            "--agents",
            r#"{"kb-agent":{}}"#,
            "--mcp-config",
            r#"{"mcpServers":{"kb":{}}}"#
        ]
    );
    let pair = |k: &str, v: &str| (k.to_owned(), v.to_owned());
    assert_eq!(env, [pair("KB_REGION", "kb"), pair("KB_TOKEN", "s3cret")]);
    assert_eq!(instructions, "Use kb.");
    assert!(waits.is_empty());
    // Without a proxy: no plugin.
    let (args, env, instructions, waits) = spawn_with(&s, None);
    assert_eq!(
        (args, env.len(), instructions.as_str(), waits.len()),
        (vec!["-c".to_owned()], 0, "", 0)
    );
}

#[test]
fn a_spawn_waits_for_a_host_that_has_not_answered() {
    let mut s = state_with_kb(None);
    let (args, env, _, waits) = spawn_with(&s, Some("work"));
    assert_eq!(args, ["-c"]);
    assert_eq!(env, [("KB_TOKEN".to_owned(), "s3cret".to_owned())]);
    assert_eq!(waits, ["kb"]);
    let (_, pending) = s.plugin_parts("local", "work", &[]);
    assert_eq!(pending[0].1.disabled, Vec::<String>::new());
    // A host that answers with installs under way is ready.
    let mut installing = applied("kb");
    installing.skipped = vec![("plugin:p@m".into(), "installing".into())];
    s.on_applied("local", "kb", &sha(), ok(installing));
    let (args, _, _, waits) = spawn_with(&s, Some("work"));
    assert_eq!((args.len(), waits.len()), (5, 0));
    // A skipped plugin brings nothing and is not waited for.
    let (parts, pending) = state_with_kb(None).plugin_parts("local", "work", &["kb".into()]);
    assert!(parts.is_empty() && pending.is_empty());
}

#[test]
fn failed_too_old_or_off_plugins_bring_no_flags() {
    let mut s = state_with_kb(None);
    s.on_applied(
        "local",
        "kb",
        &sha(),
        daemon_error("manifest kb: hash mismatch"),
    );
    let (args, env, _, waits) = spawn_with(&s, Some("work"));
    assert_eq!(
        (args.len(), env.len(), waits.len()),
        (1, 1, 0),
        "the secrets still apply"
    );

    let mut s = state_with_kb(None);
    s.on_applied(
        "local",
        "kb",
        &sha(),
        daemon_error("unsupported op: manifest"),
    );
    let (args, _, _, waits) = spawn_with(&s, Some("work"));
    assert_eq!((args.len(), waits.len()), (1, 0));

    let mut s = state_with_kb(Some(ok(applied("kb"))));
    s.set_enabled("kb", false);
    let (args, env, instructions, waits) = spawn_with(&s, Some("work"));
    assert_eq!(
        (args.len(), env.len(), instructions.len(), waits.len()),
        (1, 0, 0, 0)
    );
}

#[test]
fn a_reset_sends_the_current_plugin_env_and_instructions_and_no_args() {
    let s = state_with_kb(Some(ok(applied("kb"))));
    // The composer drops a respawn's args (app::launch); the share's env
    // and instructions are the plugins' now.
    let (parts, pending) = s.plugin_parts("local", "work", &[]);
    let c = plugins_part(&parts);
    assert_eq!(c.env.len(), 2);
    assert_eq!(c.instructions, "Use kb.");
    assert!(pending.is_empty());
}

#[test]
fn details_show_the_manifest_and_each_hosts_outcome() {
    let mut a = applied("kb");
    a.installed = vec!["plugin:p@m".into()];
    a.skipped = vec![
        ("plugin:q@m".into(), "installing".into()),
        ("mcp:kb".into(), "missing secret KB_TOKEN".into()),
    ];
    a.dropped = vec!["--agents".into()];
    let mut s = state_with_kb(Some(ok(a)));
    s.host_up("devbox", true);
    s.on_applied(
        "devbox",
        "kb",
        &sha(),
        daemon_error("unsupported op: manifest"),
    );
    assert_eq!(
        s.plugin_details("kb", "work", None),
        [
            "turning it on or off applies to new sessions",
            "The knowledge base",
            "0 items disabled (Enter to change)",
            "local: ready · 1 installed, 0 present, 1 installing, 1 skipped",
            "  mcp:kb: missing secret KB_TOKEN",
            "local: claude too old: --agents",
            "devbox: daemon too old",
            "installed plugins stay until you remove them: claude plugin uninstall <id>",
        ]
    );
    assert_eq!(
        s.plugin_details("kb", "work", Some("name taken")),
        [
            "turning it on or off applies to new sessions",
            "refused: name taken"
        ]
    );
}

#[test]
fn secrets_come_from_enabled_admitted_plugins_of_the_profile() {
    let mut s = State::default();
    let mut bad = sample("bad");
    bad.env = vec![("LD_PRELOAD".into(), "/x.so".into())];
    let mut other = sample("other");
    other.env = vec![("OTHER_TOKEN".into(), "o".into())];
    listed(&mut s, "work", vec![sample("kb"), bad, other]);
    listed(&mut s, "home", vec![sample("mine")]);
    let secrets = |s: &State, profile: &str| -> Vec<(String, String)> {
        let (parts, _) = s.plugin_parts("local", profile, &[]);
        parts.into_iter().flat_map(|p| p.secrets).collect()
    };
    let pair = |k: &str, v: &str| (k.to_owned(), v.to_owned());
    assert_eq!(
        secrets(&s, "work"),
        [pair("KB_TOKEN", "s3cret"), pair("OTHER_TOKEN", "o")]
    );
    assert!(secrets(&s, "nope").is_empty());
    // Off: no secrets.
    s.set_enabled("kb", false);
    assert_eq!(secrets(&s, "work"), [pair("OTHER_TOKEN", "o")]);
}

#[test]
fn cached_lists_are_listed_at_start_and_every_profile_in_use_is_fetched() {
    let mut s = State::default();
    let extensions = BTreeMap::from([("kb".to_owned(), false)]);
    let cached = vec![list("home", vec![sample("kb"), sample("mine")])];
    let tasks = s.load(
        (vec!["work".into()], Some("work".into())),
        BTreeMap::new(),
        extensions,
        cached,
    );
    let rows: Vec<(&str, bool)> = s
        .rows()
        .into_iter()
        .map(|r| (r.plugin.name.as_str(), r.on))
        .collect();
    assert_eq!(rows, [("kb", false), ("mine", true)]);
    // Both manifests are fetched, on or off, and every profile's list.
    let manifests: Vec<&str> = tasks
        .iter()
        .filter_map(|t| match t {
            Task::FetchManifest { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(manifests, ["kb", "mine"]);
    let fetched: Vec<&str> = tasks
        .iter()
        .filter_map(|t| match t {
            Task::FetchList { profile } => Some(profile.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(fetched, ["home", "work"]);
    // Read from the cache, the first fetch under way: no "unreachable".
    s.now = 1000;
    assert_eq!(
        s.plugin_details("mine", "home", None),
        [
            "turning it on or off applies to new sessions",
            "manifest: fetching"
        ]
    );
}

#[test]
fn signed_out_drops_the_list_and_unreachable_keeps_it() {
    let mut s = State::default();
    listed(&mut s, "work", vec![sample("kb")]);
    s.on_list("work", PluginsFetch::Unreachable);
    assert_eq!(s.plugins.lists["work"].1, ListState::Unreachable);
    assert_eq!(s.plugin_parts("local", "work", &[]).0.len(), 1);
    s.now = 100 + 180;
    let details = s.plugin_details("kb", "work", None);
    assert_eq!(details.last().unwrap(), "proxy unreachable, cached 3m");
    let tasks = s.on_list("work", PluginsFetch::SignedOut);
    assert!(s.plugin_parts("local", "work", &[]).0.is_empty());
    assert!(s.rows().is_empty());
    assert_eq!(
        notices(&tasks),
        ["plugins of work: signed out, run `claudio proxy login`"]
    );
    // Unreachable with no cache: nothing to list.
    s.on_list("home", PluginsFetch::Unreachable);
    assert!(!s.plugins.lists.contains_key("home"));
}

#[test]
fn r_asks_every_host_again_and_fetches_every_manifest() {
    let mut s = state_with_kb(Some(ok(applied("kb"))));
    let tasks = s.refresh();
    let fetches = tasks
        .iter()
        .filter(|t| matches!(t, Task::FetchManifest { .. }))
        .count();
    assert_eq!((asks(&tasks).len(), fetches), (1, 1));
    assert!(tasks.contains(&Task::FetchList {
        profile: "work".into()
    }));
    // The answer stays while it is asked again.
    assert!(matches!(
        s.apply_state("local", "kb"),
        Some(ApplyState::Ready(_))
    ));
}

// ── From the setup screen and the extensions screen ──────────────────────────

/// A first sight is due once the manifest is here, until the choices are
/// saved (written, every host asked) or the setup is put off; a plugin
/// turned off has none.
#[test]
fn first_sight_lists_ready_unsaved_plugins_until_saved_or_deferred() {
    let mut s = State::default();
    s.host_up("local", true);
    listed(
        &mut s,
        "work",
        vec![sample("kb"), sample("two"), sample("off")],
    );
    s.set_enabled("off", false);
    assert!(s.first_sight().is_empty(), "no manifest yet");
    for name in ["kb", "two", "off"] {
        let tasks = s.on_manifest(name, &sha(), Ok(manifest_of(name)));
        assert!(asks(&tasks).is_empty(), "nothing before the setup closes");
    }
    assert_eq!(s.first_sight(), ["kb", "two"]);
    let choices = PluginChoices {
        disabled: vec!["mcp:kb".into()],
        ..PluginChoices::default()
    };
    let tasks = s.save_choices("kb", choices.clone());
    assert!(tasks.contains(&Task::SaveChoices {
        name: "kb".into(),
        choices
    }));
    assert_eq!(
        asks(&tasks),
        [(
            "local".into(),
            "kb".into(),
            vec!["KB_TOKEN".into()],
            vec!["mcp:kb".into()]
        )]
    );
    assert_eq!(
        s.setup_details("kb").as_deref(),
        Some("1 items disabled (Enter to change)")
    );
    s.defer("two");
    assert!(s.first_sight().is_empty());
    assert_eq!(
        s.setup_details("two").as_deref(),
        Some("setup pending (Enter)")
    );
    // Fetched again in this run: still put off, nothing asked.
    s.refresh();
    assert!(asks(&s.on_manifest("two", &sha(), Ok(manifest_of("two")))).is_empty());
    assert!(s.first_sight().is_empty());
}

#[test]
fn new_items_of_a_newer_version_are_named_once_and_on() {
    let mut s = State::default();
    s.host_up("local", true);
    listed(&mut s, "work", vec![sample("team")]);
    let seen = vec!["plugin:p@m".to_owned()];
    s.plugins.choices.insert(
        "team".into(),
        PluginChoices {
            disabled: vec!["mcp:kb".into()],
            reviewed: "0.9".into(),
            seen,
        },
    );
    let tasks = s.on_manifest("team", &sha(), Ok(manifest_of("team")));
    assert!(s.first_sight().is_empty(), "set up before");
    assert_eq!(
        notices(&tasks),
        ["team 1.4.0 adds mcp:kb, instructions, env; Alt+i, Enter to review"]
    );
    assert_eq!(
        asks(&tasks),
        [(
            "local".into(),
            "team".into(),
            vec!["KB_TOKEN".into()],
            vec!["mcp:kb".into()]
        )]
    );
    let tasks = s.on_manifest("team", &sha(), Ok(manifest_of("team")));
    assert!(notices(&tasks).is_empty(), "once per manifest");
}

#[test]
fn a_plugin_turned_off_gets_no_new_items_notice() {
    let mut s = State::default();
    listed(&mut s, "work", vec![sample("team")]);
    s.plugins.choices.insert(
        "team".into(),
        PluginChoices {
            seen: vec!["plugin:p@m".to_owned()],
            ..PluginChoices::default()
        },
    );
    s.set_enabled("team", false);
    let tasks = s.on_manifest("team", &sha(), Ok(manifest_of("team")));
    assert!(notices(&tasks).is_empty(), "off: nothing to review");
    assert!(s.first_sight().is_empty(), "off: no setup");
}

/// A toggle is kept for a list that comes later; turned on, the hosts are
/// asked for it.
#[test]
fn a_plugin_toggle_is_kept_for_later_lists_and_on_asks_the_hosts() {
    let mut s = state_with_kb(Some(ok(applied("kb"))));
    s.set_enabled("kb", false);
    assert!(s.plugin_parts("local", "work", &[]).0.is_empty());
    assert_eq!(s.plugins.config.get("kb"), Some(&false));
    listed(&mut s, "work", vec![sample("kb")]);
    assert!(!s.plugin_enabled("kb"), "kept for the new list");
    s.on_applied("local", "kb", &sha(), daemon_error("down"));
    let tasks = s.set_enabled("kb", true);
    assert_eq!(asked(&tasks), [pair("local", "kb")]);
    // A read of [extensions] does not undo a toggle.
    s.known(BTreeMap::from([("kb".to_owned(), false)]));
    assert!(s.plugin_enabled("kb"));
}

/// A name an extension holds, or one an earlier list has, is refused like
/// its row; the list's own checks refuse too.
#[test]
fn a_taken_name_is_refused_and_never_enabled() {
    let mut s = State {
        taken: BTreeSet::from(["latex".to_owned()]),
        ..State::default()
    };
    let mut evil = sample("evil");
    evil.bundle_url = "http://e.org/x.toml".into();
    listed(&mut s, "home", vec![sample("kb")]);
    listed(&mut s, "work", vec![sample("latex"), sample("kb"), evil]);
    let rows: Vec<(&str, &str, Option<String>)> = s
        .rows()
        .into_iter()
        .map(|r| (r.profile, r.plugin.name.as_str(), r.refused))
        .collect();
    assert_eq!(
        rows,
        [
            ("home", "kb", None),
            ("work", "latex", Some("name taken".into())),
            ("work", "kb", Some("name taken".into())),
            ("work", "evil", Some("bundle URL is not https".into())),
        ]
    );
    assert!(s.enabled_plugins("work").is_empty());
    assert_eq!(s.plugins.lists.len(), 2);
}

#[test]
fn a_profile_put_to_use_is_fetched_once_loaded() {
    let mut s = State::default();
    let used = BTreeSet::from(["lab".to_owned()]);
    assert!(s.use_profiles(used.clone()).is_empty(), "not loaded yet");
    let tasks = s.load((vec![], None), BTreeMap::new(), BTreeMap::new(), Vec::new());
    assert!(tasks.contains(&Task::FetchList {
        profile: "lab".into()
    }));
    let both = BTreeSet::from(["lab".to_owned(), "work".to_owned()]);
    assert_eq!(
        s.use_profiles(both.clone()),
        [Task::FetchList {
            profile: "work".into()
        }]
    );
    assert!(s.use_profiles(both).is_empty(), "once");
}

#[test]
fn every_list_is_fetched_again_every_15_minutes() {
    let mut s = State {
        now: 1000,
        ..State::default()
    };
    s.load(
        (vec!["work".into()], Some("work".into())),
        BTreeMap::new(),
        BTreeMap::new(),
        Vec::new(),
    );
    assert!(s.tick(1000 + REFRESH_SECS - 1).is_empty());
    let fetch = Task::FetchList {
        profile: "work".into(),
    };
    assert_eq!(s.tick(1000 + REFRESH_SECS), [fetch]);
    assert!(s.tick(1000 + REFRESH_SECS + 1).is_empty());
}

#[test]
fn a_reload_asks_again_where_choices_changed() {
    let mut s = state_with_kb(Some(ok(applied("kb"))));
    let same = s.plugins.choices.clone();
    let tasks = s.reload((vec!["work".into()], None), same);
    assert!(asks(&tasks).is_empty());
    let changed = BTreeMap::from([(
        "kb".to_owned(),
        PluginChoices {
            disabled: vec!["mcp:kb".into()],
            ..PluginChoices::default()
        },
    )]);
    let tasks = s.reload((vec!["work".into()], Some("work".into())), changed);
    assert_eq!(asks(&tasks)[0].3, ["mcp:kb"]);
    assert_eq!(s.default.as_deref(), Some("work"));
}

#[test]
fn a_profiles_config_is_kept_five_minutes() {
    let mut s = State {
        now: 1000,
        ..State::default()
    };
    s.on_config("work", None);
    assert!(s.config_cached("work").is_none());
    s.on_config("work", Some(ConfigResponse::default()));
    s.now = 1000 + CONFIG_TTL - 1;
    assert!(s.config_cached("work").is_some());
    s.now = 1000 + CONFIG_TTL;
    assert!(s.config_cached("work").is_none());
}
