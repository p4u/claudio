//! The TUI's glue for proxy plugins: `impl App` turns profile lists, manifests
//! and host answers into state changes and [`Effect`]s. The pure state it
//! works on is [`crate::addons::vocdoni::plugins::state`].

use std::collections::{BTreeMap, BTreeSet};

use crate::addons::vocdoni::daemon::{apply_msg, serves_apply, Applied, ApplyRequest};
use crate::addons::vocdoni::plugins::choices::PluginChoices;
use crate::addons::vocdoni::plugins::manifest::Manifest;
use crate::addons::vocdoni::proxy::plugins::{Plugin, PluginList};
use crate::app::extensions::Source;
use crate::proto::Msg;

use super::app::{App, Effect, PendingManifest, ReplyTo};
use crate::addons::vocdoni::plugins::state::{
    reask_after, Apply, ApplyState, ListState, ManifestState, PluginPart, PluginsFetch, INSTALLING,
};

impl App {
    /// The admitted plugin `name` of `profile`'s list, if the registry has
    /// it (not refused) and it is on.
    fn enabled_plugin(&self, profile: &str, name: &str) -> Option<&Plugin> {
        let on = self.extensions.extensions.iter().any(|e| {
            e.name == name
                && e.enabled
                && matches!(&e.source, Source::Plugin { profile: p, refused: None } if p == profile)
        });
        let (list, _) = self.plugins.lists.get(profile)?;
        list.plugins.iter().find(|p| on && p.name == name)
    }

    /// The enabled plugins of `profile`, in registry order.
    pub(super) fn enabled_plugins(&self, profile: &str) -> Vec<&Plugin> {
        self.extensions
            .extensions
            .iter()
            .filter_map(|e| self.enabled_plugin(profile, &e.name))
            .collect()
    }

    /// Whether plugin `name` (of any profile) is on.
    pub(super) fn plugin_enabled(&self, name: &str) -> bool {
        self.all_enabled_plugins().iter().any(|p| p.name == name)
    }

    /// Every enabled plugin of every profile, in registry order.
    fn all_enabled_plugins(&self) -> Vec<&Plugin> {
        let lists = self.plugins.lists.keys();
        lists
            .flat_map(|profile| self.enabled_plugins(profile))
            .collect()
    }

    /// Every admitted plugin of every list (on or off).
    fn admitted_plugins(&self) -> Vec<Plugin> {
        let lists = self.plugins.lists.values();
        lists
            .flat_map(|(list, _)| list.admitted().into_iter().cloned())
            .collect()
    }

    /// A list arrived (or failed): keep it, list it, fetch the manifests it
    /// changed and ask the connected hosts for them.
    pub fn on_plugins(&mut self, profile: &str, fetch: PluginsFetch) {
        match fetch {
            PluginsFetch::Listed(list) => {
                self.plugins
                    .lists
                    .insert(profile.to_owned(), (list, ListState::Fresh));
            }
            PluginsFetch::SignedOut => {
                self.plugins.lists.remove(profile);
                self.notify(format!(
                    "plugins of {profile}: signed out, run `claudio proxy login`"
                ));
            }
            PluginsFetch::Unreachable => {
                if let Some((_, state)) = self.plugins.lists.get_mut(profile) {
                    *state = ListState::Unreachable;
                }
            }
        }
        self.list_plugins();
        self.fetch_manifests(false);
        self.request_all();
        self.redraw = true;
    }

    /// Put every profile's plugins in the registry, with the saved choices.
    pub(super) fn list_plugins(&mut self) {
        let lists: Vec<&PluginList> = self.plugins.lists.values().map(|(l, _)| l).collect();
        self.extensions.set_plugins(&lists, &self.plugins.config);
    }

    /// Fetch the manifest of each admitted plugin whose hash is new (every
    /// one when `all`).
    fn fetch_manifests(&mut self, all: bool) {
        for p in self.admitted_plugins() {
            let known = self.plugins.manifests.get(&p.name);
            if !all && known.is_some_and(|(sha, _)| *sha == p.bundle_sha256) {
                continue;
            }
            let entry = (p.bundle_sha256.clone(), ManifestState::Fetching);
            self.plugins.manifests.insert(p.name.clone(), entry);
            self.emit(Effect::FetchManifest {
                name: p.name,
                url: p.bundle_url,
                sha256: p.bundle_sha256,
            });
        }
    }

    /// Plugin `name`'s manifest at hash `sha256` was fetched (and parsed),
    /// or not.
    pub fn on_manifest(&mut self, name: &str, sha256: &str, result: Result<Manifest, String>) {
        match self.plugins.manifests.get_mut(name) {
            Some((sha, state)) if sha == sha256 => {
                *state = match result {
                    Ok(m) => ManifestState::Ready(Box::new(m)),
                    Err(why) => ManifestState::Failed(why),
                };
            }
            // A hash since replaced.
            _ => return,
        }
        self.on_manifest_ready(name, sha256);
        self.request_all();
        self.redraw = true;
    }

    /// `host`'s daemon is connected, with `features`: one without
    /// `addon:vocdoni` (older than addons) is never asked, and every plugin
    /// is `TooOld` there; another is asked for every enabled manifest.
    pub(super) fn on_daemon_host(&mut self, host: &str, features: &[String]) {
        if serves_apply(features) {
            self.plugins.old_hosts.remove(host);
        } else {
            self.plugins.old_hosts.insert(host.to_owned());
        }
        self.on_plugin_host(host);
    }

    /// `host`'s daemon is connected: ask it for every enabled manifest.
    pub fn on_plugin_host(&mut self, host: &str) {
        self.plugins.hosts.insert(host.to_owned());
        // A new connection may be a new daemon: ask again.
        self.plugins.applies.retain(|(h, _), _| h != host);
        self.request_manifests(host, None);
    }

    /// `host`'s daemon is gone: its answers no longer hold.
    pub(super) fn plugin_host_gone(&mut self, host: &str) {
        self.plugins.hosts.remove(host);
        self.plugins.old_hosts.remove(host);
        self.plugins.applies.retain(|(h, _), _| h != host);
    }

    /// [`App::request_manifests`] on every connected host.
    pub(super) fn request_all(&mut self) {
        let hosts: Vec<String> = self.plugins.hosts.iter().cloned().collect();
        for host in hosts {
            self.request_manifests(&host, None);
        }
    }

    /// Ask every connected host for plugin `name`'s manifest again, even
    /// where it answered (its choices changed).
    pub(super) fn request_again(&mut self, name: &str) {
        let hosts: Vec<String> = self.plugins.hosts.iter().cloned().collect();
        for host in hosts {
            self.request_manifests(&host, Some(name));
        }
    }

    /// Whether a host may be asked for `p`'s manifest now: the client has
    /// it, good, at the list's hash, and the member's choices are saved
    /// (nothing is sent before the first-sight setup closes with Enter).
    fn requestable(&self, p: &Plugin) -> bool {
        let fetched = matches!(
            self.plugins.manifests.get(&p.name),
            Some((sha, ManifestState::Ready(_))) if *sha == p.bundle_sha256
        );
        fetched && self.plugins.choices.contains_key(&p.name)
    }

    /// Ask `host` for each enabled plugin's manifest it has not answered for
    /// at its current hash (a failure is asked again); `again`: that
    /// plugin's whatever the answer.
    pub(super) fn request_manifests(&mut self, host: &str, again: Option<&str>) {
        let wanted: Vec<Plugin> = self.all_enabled_plugins().into_iter().cloned().collect();
        for p in wanted {
            if !self.requestable(&p) {
                continue;
            }
            let key = (host.to_owned(), p.name.clone());
            let answered = self.plugins.applies.get(&key).is_some_and(|a| {
                a.sha == p.bundle_sha256 && !matches!(a.state, ApplyState::Failed(_))
            });
            if answered && again != Some(p.name.as_str()) {
                continue;
            }
            self.ask(host, &p);
        }
    }

    /// Send `host` the `vocdoni/apply` request for `p`. A `Ready` answer at
    /// the same hash stays until the new one comes. A daemon older than
    /// addons is not asked: `TooOld` at once.
    fn ask(&mut self, host: &str, p: &Plugin) {
        let key = (host.to_owned(), p.name.clone());
        let now = self.now;
        if self.plugins.old_hosts.contains(host) {
            let sha = p.bundle_sha256.clone();
            let state = ApplyState::TooOld;
            self.plugins.applies.insert(
                key,
                Apply {
                    sha,
                    state,
                    asked: now,
                },
            );
            return;
        }
        let apply = match self.plugins.applies.remove(&key) {
            Some(a) if a.sha == p.bundle_sha256 && matches!(a.state, ApplyState::Ready(_)) => {
                Apply { asked: now, ..a }
            }
            Some(a) if a.sha == p.bundle_sha256 => Apply {
                state: ApplyState::Pending,
                asked: now,
                ..a
            },
            _ => Apply {
                sha: p.bundle_sha256.clone(),
                state: ApplyState::Pending,
                asked: now,
            },
        };
        self.plugins.applies.insert(key, apply);
        let to = ReplyTo::Manifest {
            host: host.to_owned(),
            name: p.name.clone(),
            sha256: p.bundle_sha256.clone(),
        };
        let msg = self.manifest_msg(p);
        self.request(host, msg, to);
    }

    /// The `vocdoni/apply` request for `p`: the names of its proxy env
    /// (never the values) and the ids the member turned off.
    pub(super) fn manifest_msg(&self, p: &Plugin) -> Msg {
        let disabled = self
            .plugins
            .choices
            .get(&p.name)
            .map(|c| c.disabled.clone())
            .unwrap_or_default();
        apply_msg(&ApplyRequest {
            name: p.name.clone(),
            url: p.bundle_url.clone(),
            sha256: p.bundle_sha256.clone(),
            secrets: p.env.iter().map(|(k, _)| k.clone()).collect(),
            disabled,
        })
    }

    /// Ask again a host whose answer still lists an install under way
    /// (every [`REASK_EVERY`] s) or a failed one to retry (once it is due).
    /// The daemon's backoff, up to 5 min, paces a lasting failure.
    pub(super) fn reask_installing(&mut self) {
        let now = self.now;
        let due: Vec<(String, String)> = self
            .plugins
            .applies
            .iter()
            .filter(|(_, a)| {
                let after = match &a.state {
                    ApplyState::Ready(applied) => reask_after(applied),
                    _ => None,
                };
                after.is_some_and(|after| now.saturating_sub(a.asked) >= after)
            })
            .map(|(key, _)| key.clone())
            .collect();
        for (host, name) in due {
            let plugin = self
                .all_enabled_plugins()
                .into_iter()
                .find(|p| p.name == name)
                .cloned();
            if let Some(p) = plugin.filter(|p| self.requestable(p)) {
                self.ask(&host, &p);
            }
        }
    }

    /// Claude was just installed on `host`: ask it again for every enabled
    /// manifest, answered or not, so the installs that failed without it
    /// run now. An answer stays until the new one comes.
    pub(super) fn reapply_host(&mut self, host: &str) {
        if !self.plugins.hosts.contains(host) {
            return;
        }
        let names: Vec<String> = self
            .all_enabled_plugins()
            .iter()
            .map(|p| p.name.clone())
            .collect();
        for name in names {
            self.request_manifests(host, Some(&name));
        }
    }

    /// `host`'s answer for plugin `name`'s manifest at hash `sha256`.
    pub(super) fn on_applied(
        &mut self,
        host: &str,
        name: &str,
        sha256: &str,
        reply: std::io::Result<Msg>,
    ) {
        let key = (host.to_owned(), name.to_owned());
        // An answer for a hash since replaced, or a host since gone.
        let Some(apply) = self.plugins.applies.get_mut(&key) else {
            return;
        };
        if apply.sha != sha256 {
            return;
        }
        apply.state = match reply {
            Ok(reply) => ApplyState::of_reply(reply),
            Err(e) if App::older_daemon_notice(host, &e).is_some() => ApplyState::TooOld,
            Err(e) => {
                let why = e.to_string();
                let prefix = format!("manifest {name}: ");
                ApplyState::Failed(why.strip_prefix(&prefix).unwrap_or(&why).to_owned())
            }
        };
        self.redraw = true;
    }

    /// The state of plugin `name`'s manifest on `host`, if asked.
    pub fn apply_state(&self, host: &str, name: &str) -> Option<&ApplyState> {
        let key = (host.to_owned(), name.to_owned());
        self.plugins.applies.get(&key).map(|a| &a.state)
    }

    /// What `profile`'s enabled plugins bring to a spawn on `host`, in
    /// registry order, and the `Manifest` requests of those `host` has not
    /// answered at all (the spawn waits for them). A plugin whose manifest
    /// the client lacks, or whose host failed, brings its secrets only.
    pub(super) fn plugin_parts(
        &self,
        host: &str,
        profile: &str,
    ) -> (Vec<PluginPart>, Vec<PendingManifest>) {
        let mut parts = Vec::new();
        let mut pending = Vec::new();
        for p in self.enabled_plugins(profile) {
            let applied = match self.apply_state(host, &p.name) {
                Some(ApplyState::Ready(a)) => Some(Applied::clone(a)),
                Some(ApplyState::Pending) | None if self.requestable(p) => {
                    pending.push(PendingManifest {
                        name: p.name.clone(),
                        msg: self.manifest_msg(p),
                        to: ReplyTo::Manifest {
                            host: host.to_owned(),
                            name: p.name.clone(),
                            sha256: p.bundle_sha256.clone(),
                        },
                    });
                    None
                }
                _ => None,
            };
            parts.push(PluginPart {
                name: p.name.clone(),
                applied,
                secrets: p.env.clone(),
            });
        }
        (parts, pending)
    }

    /// What the extensions screen says about plugin `name` of `profile`
    /// (beyond its version): its manifest, one line per host that was
    /// asked, and the list's state.
    pub(super) fn plugin_details(
        &self,
        name: &str,
        profile: &str,
        refused: Option<&str>,
    ) -> Vec<String> {
        let mut lines = vec!["turning it on or off applies to new sessions".to_owned()];
        if let Some(why) = refused {
            lines.push(format!("refused: {why}"));
            return lines;
        }
        let manifest = match self.plugins.manifests.get(name) {
            Some((_, ManifestState::Ready(m))) => Some(m),
            Some((_, ManifestState::Fetching)) => {
                lines.push("manifest: fetching".to_owned());
                None
            }
            Some((_, ManifestState::Failed(why))) => {
                lines.push(format!("manifest: {why}"));
                None
            }
            None => None,
        };
        if let Some(m) = manifest.filter(|m| !m.description.trim().is_empty()) {
            lines.push(m.description.clone());
        }
        lines.extend(self.setup_details(name));
        let mut hosts: Vec<&String> = self.plugins.hosts.iter().collect();
        hosts.sort_by_key(|h| (h.as_str() != "local", h.as_str()));
        for host in hosts {
            match self.apply_state(host, name) {
                None => {}
                Some(ApplyState::Pending) => lines.push(format!("{host}: applying")),
                Some(ApplyState::Ready(a)) => lines.extend(ready_lines(host, a)),
                Some(ApplyState::Failed(why)) => lines.push(format!("{host}: failed: {why}")),
                Some(ApplyState::TooOld) => lines.push(format!("{host}: daemon too old")),
            }
        }
        if manifest.is_some_and(|m| !m.plugins.is_empty()) {
            lines.push(
                "installed plugins stay until you remove them: claude plugin uninstall <id>"
                    .to_owned(),
            );
        }
        if let Some((list, ListState::Unreachable)) = self.plugins.lists.get(profile) {
            let age = super::fmt::fmt_age(self.now.saturating_sub(list.fetched_at));
            lines.push(format!("proxy unreachable, cached {age}"));
        }
        lines
    }

    /// `r` on the extensions screen: ask every host again for every
    /// enabled manifest, and fetch every list and manifest again.
    pub(super) fn refresh_plugins(&mut self) {
        self.fetch_plugin_lists();
        let names: Vec<String> = self
            .all_enabled_plugins()
            .iter()
            .map(|p| p.name.clone())
            .collect();
        for name in names {
            self.request_again(&name);
        }
        self.fetch_manifests(true);
        self.notify("refreshing plugins…");
    }

    /// Fetch the list of every profile in use or listed (at start, then
    /// every [`REFRESH_SECS`]).
    pub fn fetch_plugin_lists(&mut self) {
        let mut profiles: BTreeSet<String> = self.plugins.lists.keys().cloned().collect();
        profiles.extend(self.plugin_profiles());
        for profile in profiles {
            self.emit(Effect::FetchPlugins { profile });
        }
    }

    /// The lists read from `plugins.json` at start, `[extensions]` and
    /// `[plugins]`; their manifests are fetched.
    pub fn load_plugins(
        &mut self,
        cached: Vec<PluginList>,
        config: BTreeMap<String, bool>,
        choices: BTreeMap<String, PluginChoices>,
    ) {
        self.plugins.config = config;
        self.plugins.choices = choices;
        for list in cached {
            let profile = list.profile.clone();
            self.plugins
                .lists
                .insert(profile, (list, ListState::Cached));
        }
        self.list_plugins();
        self.fetch_manifests(false);
    }

    /// The profiles sessions use: the one new sessions get, and every open
    /// session's.
    pub fn plugin_profiles(&self) -> BTreeSet<String> {
        let mut profiles: BTreeSet<String> = self
            .sessions
            .iter()
            .filter_map(|v| v.proxy.clone())
            .collect();
        if let Some(p) = self.proxy_override.pick(self.proxy_default.as_deref()) {
            profiles.insert(p.to_owned());
        }
        profiles
    }
}

/// The extensions screen's lines for `host`'s answer `a`: the counts, one
/// line per skipped id that is not installing, and the flags its claude
/// lacks.
fn ready_lines(host: &str, a: &Applied) -> Vec<String> {
    let installing = a
        .skipped
        .iter()
        .filter(|(_, why)| why == INSTALLING)
        .count();
    let skipped = a.skipped.len() - installing;
    let mut lines = vec![format!(
        "{host}: ready · {} installed, {} present, {installing} installing, {skipped} skipped",
        a.installed.len(),
        a.present.len()
    )];
    for (id, why) in a.skipped.iter().filter(|(_, why)| why != INSTALLING) {
        lines.push(format!("  {id}: {why}"));
    }
    if !a.dropped.is_empty() {
        lines.push(format!("{host}: claude too old: {}", a.dropped.join(", ")));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::vocdoni::daemon::{applied_reply, apply_request_of};
    use crate::addons::vocdoni::plugins::state::{applied, compose, with_plugins};
    use crate::addons::vocdoni::proxy::plugins::sample;
    use crate::tui::app::LaunchCall;
    use crate::tui::test_support::app_with;

    fn list(profile: &str, plugins: Vec<Plugin>) -> PluginList {
        PluginList {
            profile: profile.into(),
            fetched_at: 100,
            plugins,
        }
    }

    /// The manifest plugin `name` serves in these tests: a plugin, a
    /// server needing `KB_TOKEN`, instructions and env.
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

    /// A `Manifest` request: host, name, secrets, disabled.
    type Asked = (String, String, Vec<String>, Vec<String>);

    /// The `vocdoni/apply` requests queued.
    fn manifest_requests(app: &mut App) -> Vec<Asked> {
        app.take_effects()
            .into_iter()
            .filter_map(|e| match e {
                Effect::Request { host, msg, .. } => {
                    apply_request_of(&msg).map(|r| (host, r.name, r.secrets, r.disabled))
                }
                _ => None,
            })
            .collect()
    }

    /// The hosts asked, as (host, name).
    fn asked(app: &mut App) -> Vec<(String, String)> {
        let mut asked: Vec<(String, String)> = manifest_requests(app)
            .into_iter()
            .map(|(h, n, _, _)| (h, n))
            .collect();
        asked.sort();
        asked
    }

    fn pair(host: &str, name: &str) -> (String, String) {
        (host.to_owned(), name.to_owned())
    }

    #[test]
    fn a_list_fetches_its_manifests_and_hosts_are_asked_once_they_arrive() {
        let mut app = app_with(&[]);
        app.on_plugin_host("local");
        assert!(asked(&mut app).is_empty(), "no list yet");
        app.on_plugins(
            "work",
            PluginsFetch::Listed(list("work", vec![sample("kb")])),
        );
        let effects = app.take_effects();
        let fetched: Vec<&Effect> = effects
            .iter()
            .filter(|e| matches!(e, Effect::FetchManifest { name, .. } if name == "kb"))
            .collect();
        assert_eq!(fetched.len(), 1, "{effects:?}");
        assert!(
            !effects.iter().any(|e| matches!(e, Effect::Request { .. })),
            "nothing asked before the manifest is here"
        );
        app.plugins.choices.insert(
            "kb".into(),
            PluginChoices {
                disabled: vec!["mcp:kb".into()],
                ..PluginChoices::default()
            },
        );
        app.on_manifest("kb", &sha(), Ok(manifest_of("kb")));
        assert_eq!(
            manifest_requests(&mut app),
            [(
                "local".to_owned(),
                "kb".to_owned(),
                vec!["KB_TOKEN".to_owned()],
                vec!["mcp:kb".to_owned()]
            )]
        );
        // A second host is asked on connect; the same list again asks no
        // one and fetches nothing.
        app.on_plugin_host("devbox");
        assert_eq!(asked(&mut app), [pair("devbox", "kb")]);
        app.on_plugins(
            "work",
            PluginsFetch::Listed(list("work", vec![sample("kb")])),
        );
        assert!(app.take_effects().is_empty());
        // A new version is fetched; the hosts are asked once it is here.
        let mut v2 = sample("kb");
        v2.bundle_sha256 = "b".repeat(64);
        app.on_plugins("work", PluginsFetch::Listed(list("work", vec![v2])));
        assert!(asked(&mut app).is_empty());
        app.on_manifest("kb", &sha(), Ok(manifest_of("kb")));
        assert!(asked(&mut app).is_empty(), "stale: the old hash");
        app.on_manifest("kb", &"b".repeat(64), Ok(manifest_of("kb")));
        assert_eq!(asked(&mut app), [pair("devbox", "kb"), pair("local", "kb")]);
    }

    #[test]
    fn a_refused_manifest_is_asked_of_no_host_and_shown() {
        let mut app = app_with(&[]);
        app.on_plugin_host("local");
        app.on_plugins(
            "work",
            PluginsFetch::Listed(list("work", vec![sample("kb")])),
        );
        app.take_effects();
        app.on_manifest("kb", &sha(), Err("hash mismatch".into()));
        assert!(asked(&mut app).is_empty());
        let details = app.plugin_details("kb", "work", None);
        assert_eq!(details[1], "manifest: hash mismatch");
    }

    /// `kb` listed, its manifest here and its choices saved; no host yet.
    fn app_with_kb_unasked() -> App {
        let mut app = app_with(&[]);
        app.modal = None;
        app.plugins
            .choices
            .insert("kb".into(), PluginChoices::default());
        app.on_plugins(
            "work",
            PluginsFetch::Listed(list("work", vec![sample("kb")])),
        );
        app.on_manifest("kb", &sha(), Ok(manifest_of("kb")));
        app.take_effects();
        app
    }

    /// A daemon whose Welcome lacks `addon:vocdoni` (older than addons) is
    /// never asked: every plugin is `TooOld` there at once and a spawn does
    /// not wait for it. Once a newer daemon connects there, it is asked.
    #[test]
    fn a_host_without_addon_vocdoni_is_too_old_and_never_asked() {
        let mut app = app_with_kb_unasked();
        app.on_daemon_host("local", &["lens".to_owned()]);
        assert!(manifest_requests(&mut app).is_empty(), "never asked");
        assert_eq!(app.apply_state("local", "kb"), Some(&ApplyState::TooOld));
        let (parts, pending) = app.plugin_parts("local", "work");
        assert!(pending.is_empty(), "a spawn does not wait for it");
        assert_eq!(parts[0].applied, None);
        assert_eq!(
            parts[0].secrets,
            [("KB_TOKEN".to_owned(), "s3cret".to_owned())]
        );
        let features = ["lens".to_owned(), "addon:vocdoni".to_owned()];
        app.on_daemon_host("local", &features);
        assert_eq!(asked(&mut app), [pair("local", "kb")]);
    }

    /// C1: the apply request on the wire names the plugin's secrets, never
    /// their values.
    #[test]
    fn apply_bodies_carry_secret_names_never_values() {
        let mut app = app_with_kb_unasked();
        app.on_daemon_host("local", &["addon:vocdoni".to_owned()]);
        let msgs: Vec<Msg> = app
            .take_effects()
            .into_iter()
            .filter_map(|e| match e {
                Effect::Request { msg, .. } => Some(msg),
                _ => None,
            })
            .collect();
        assert_eq!(msgs.len(), 1);
        let frame = serde_json::to_string(&crate::proto::Envelope::request(1, msgs[0].clone()));
        let frame = frame.unwrap();
        assert!(frame.contains(r#""secrets":["KB_TOKEN"]"#), "{frame}");
        assert!(!frame.contains("s3cret"), "a secret value on the wire");
    }

    /// An app with `local` connected and profile `work` serving `kb`, set
    /// up, whose manifest is here and which `local` answered with `answer`
    /// (`None`: not yet).
    fn app_with_kb(answer: Option<Msg>) -> App {
        let mut app = app_with(&[]);
        app.modal = None;
        app.plugins
            .choices
            .insert("kb".into(), PluginChoices::default());
        app.on_plugin_host("local");
        app.on_plugins(
            "work",
            PluginsFetch::Listed(list("work", vec![sample("kb")])),
        );
        app.on_manifest("kb", &sha(), Ok(manifest_of("kb")));
        if let Some(msg) = answer {
            app.on_applied("local", "kb", &sha(), Ok(msg));
        }
        app.take_effects();
        app
    }

    #[test]
    fn answers_set_each_hosts_state_and_stale_ones_are_dropped() {
        let mut app = app_with_kb(None);
        assert_eq!(app.apply_state("local", "kb"), Some(&ApplyState::Pending));
        let ready = applied_reply(applied("kb"));
        app.on_applied("local", "kb", &"b".repeat(64), Ok(ready.clone()));
        assert_eq!(app.apply_state("local", "kb"), Some(&ApplyState::Pending));
        app.on_applied("local", "kb", &sha(), Ok(ready));
        assert_eq!(
            app.apply_state("local", "kb"),
            Some(&ApplyState::Ready(Box::new(applied("kb"))))
        );
        let old = std::io::Error::other("unsupported op: manifest");
        app.on_applied("local", "kb", &sha(), Err(old));
        assert_eq!(app.apply_state("local", "kb"), Some(&ApplyState::TooOld));
        let failed = std::io::Error::other("manifest kb: format is not 1");
        app.on_applied("local", "kb", &sha(), Err(failed));
        assert_eq!(
            app.apply_state("local", "kb"),
            Some(&ApplyState::Failed("format is not 1".into()))
        );
        // A failure is asked again on the next occasion; an answer is not.
        app.request_all();
        assert_eq!(asked(&mut app), [pair("local", "kb")]);
        app.on_applied("local", "kb", &sha(), Ok(applied_reply(applied("kb"))));
        app.request_all();
        assert!(asked(&mut app).is_empty());
        // A host that left forgets its answers.
        app.plugin_host_gone("local");
        assert_eq!(app.apply_state("local", "kb"), None);
    }

    #[test]
    fn a_host_is_asked_again_while_it_installs_or_until_a_retry_is_due() {
        let answer = |why: &str| {
            let mut a = applied("kb");
            a.skipped = vec![("plugin:p@m".into(), why.into())];
            a
        };
        let installing = answer("installing");
        let mut app = app_with(&[]);
        app.now = 1000;
        app.plugins
            .choices
            .insert("kb".into(), PluginChoices::default());
        app.on_plugin_host("local");
        app.on_plugins(
            "work",
            PluginsFetch::Listed(list("work", vec![sample("kb")])),
        );
        app.on_manifest("kb", &sha(), Ok(manifest_of("kb")));
        app.on_applied("local", "kb", &sha(), Ok(applied_reply(installing.clone())));
        app.take_effects();
        app.now = 1029;
        app.reask_installing();
        assert!(asked(&mut app).is_empty());
        app.now = 1030;
        app.reask_installing();
        assert_eq!(asked(&mut app), [pair("local", "kb")]);
        // The last answer stays meanwhile.
        assert_eq!(
            app.apply_state("local", "kb"),
            Some(&ApplyState::Ready(Box::new(installing.clone())))
        );
        // As long as it installs.
        app.on_applied("local", "kb", &sha(), Ok(applied_reply(installing)));
        app.now = 1030 + 30 * 60;
        app.reask_installing();
        assert_eq!(asked(&mut app), [pair("local", "kb")]);

        // A failed install: a second after its retry is due...
        let asked_at = app.now;
        let later = answer("network down (retrying in 120 s)");
        app.on_applied("local", "kb", &sha(), Ok(applied_reply(later)));
        app.now = asked_at + 120;
        app.reask_installing();
        assert!(asked(&mut app).is_empty());
        app.now = asked_at + 121;
        app.reask_installing();
        assert_eq!(asked(&mut app), [pair("local", "kb")]);
        // ...and no sooner than 30 s.
        let asked_at = app.now;
        let soon = answer("network down (retrying in 5 s)");
        app.on_applied("local", "kb", &sha(), Ok(applied_reply(soon)));
        app.now = asked_at + 29;
        app.reask_installing();
        assert!(asked(&mut app).is_empty());
        app.now = asked_at + 30;
        app.reask_installing();
        assert_eq!(asked(&mut app), [pair("local", "kb")]);

        // Installed, or waiting for a person: no more asking.
        for settled in [
            applied("kb"),
            answer("needs manual install: claude plugin install p@m"),
        ] {
            app.on_applied("local", "kb", &sha(), Ok(applied_reply(settled)));
            app.now += 3600;
            app.reask_installing();
            assert!(asked(&mut app).is_empty());
        }
    }

    #[test]
    fn claude_installed_on_a_host_asks_it_again_for_every_manifest() {
        let mut app = app_with(&[]);
        app.plugins
            .choices
            .insert("kb".into(), PluginChoices::default());
        app.on_plugin_host("local");
        app.on_plugin_host("devbox");
        app.on_plugins(
            "work",
            PluginsFetch::Listed(list("work", vec![sample("kb")])),
        );
        app.on_manifest("kb", &sha(), Ok(manifest_of("kb")));
        for host in ["local", "devbox"] {
            app.on_applied(host, "kb", &sha(), Ok(applied_reply(applied("kb"))));
        }
        app.take_effects();
        let reply = |app: &mut App, install: bool, ok: bool| {
            app.start_claude_update("devbox", install);
            let msg = Msg::ClaudeUpdated {
                version: ok.then(|| "2.1.296 (Claude Code)".to_owned()),
                ok,
                tail: "claude installer needs curl".into(),
            };
            app.on_reply(ReplyTo::ClaudeUpdate("devbox".into()), Ok(msg));
        };

        reply(&mut app, true, false);
        assert!(asked(&mut app).is_empty(), "the install failed");
        reply(&mut app, true, true);
        assert_eq!(asked(&mut app), [pair("devbox", "kb")]);
        assert_eq!(
            app.apply_state("devbox", "kb"),
            Some(&ApplyState::Ready(Box::new(applied("kb")))),
            "the answer stays until the new one comes"
        );
        reply(&mut app, false, true);
        assert!(asked(&mut app).is_empty(), "an update is not an install");
    }

    /// A spawn's args, env, instructions, and the manifests it waits for.
    type Spawned = (Vec<String>, Vec<(String, String)>, String, Vec<String>);

    /// Start a claude session in `/w` on `local` with `proxy`.
    fn spawn_with(app: &mut App, proxy: Option<&str>) -> Spawned {
        let id = uuid::Uuid::new_v4();
        let kind = crate::proto::SessionKind::Claude;
        let proxy = proxy.map(str::to_owned);
        app.start(
            id,
            "local".into(),
            "/w".into(),
            kind,
            vec!["-c".into()],
            proxy,
            0,
        )
        .unwrap();
        let effects = app.take_effects();
        for e in effects {
            match e {
                Effect::SpawnWithProxy {
                    msg,
                    plugins,
                    pending,
                    to,
                    ..
                } => {
                    assert_eq!(to, ReplyTo::SpawnedDeferred(id));
                    let names = pending.into_iter().map(|b| b.name).collect();
                    let Msg::Spawn(spec) = with_plugins(msg, compose(&plugins)) else {
                        panic!("a spawn")
                    };
                    let ext = spec.extensions.unwrap_or_default();
                    return (spec.args, spec.env, ext.instructions, names);
                }
                // A direct session goes through the launch composer.
                Effect::Launch(LaunchCall {
                    base: Msg::Spawn(spec),
                    ..
                }) => {
                    let ext = spec.extensions.unwrap_or_default();
                    return (spec.args, spec.env, ext.instructions, Vec::new());
                }
                _ => {}
            }
        }
        panic!("no spawn")
    }

    #[test]
    fn a_spawn_gets_the_answered_plugins_flags_env_and_instructions() {
        let mut app = app_with_kb(Some(applied_reply(applied("kb"))));
        let (args, env, instructions, waits) = spawn_with(&mut app, Some("work"));
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
        assert_eq!(
            env[..2],
            [pair("KB_REGION", "kb"), pair("KB_TOKEN", "s3cret")]
        );
        assert_eq!(instructions, "Use kb.");
        assert!(waits.is_empty());
        // Without a proxy: no plugin.
        let (args, env, instructions, waits) = spawn_with(&mut app, None);
        assert_eq!(
            (args, env.len(), instructions.as_str(), waits.len()),
            (vec!["-c".to_owned()], 0, "", 0)
        );
    }

    #[test]
    fn a_spawn_waits_for_a_host_that_has_not_answered() {
        let mut app = app_with_kb(None);
        let (args, env, _, waits) = spawn_with(&mut app, Some("work"));
        assert_eq!(args, ["-c"]);
        assert_eq!(env, [("KB_TOKEN".to_owned(), "s3cret".to_owned())]);
        assert_eq!(waits, ["kb"]);
        let notice = app.notice.as_ref().unwrap().text.clone();
        assert_eq!(notice, "applying plugin kb…");
        // A host that answers with installs under way is ready.
        let mut installing = applied("kb");
        installing.skipped = vec![("plugin:p@m".into(), "installing".into())];
        app.on_applied("local", "kb", &sha(), Ok(applied_reply(installing)));
        let (args, _, _, waits) = spawn_with(&mut app, Some("work"));
        assert_eq!((args.len(), waits.len()), (5, 0));
    }

    #[test]
    fn failed_too_old_or_off_plugins_bring_no_flags() {
        let mut app = app_with_kb(None);
        let failed = std::io::Error::other("manifest kb: hash mismatch");
        app.on_applied("local", "kb", &sha(), Err(failed));
        let (args, env, _, waits) = spawn_with(&mut app, Some("work"));
        assert_eq!(
            (args.len(), env.len(), waits.len()),
            (1, 1, 0),
            "the secrets still apply"
        );

        let mut app = app_with_kb(None);
        let old = std::io::Error::other("unsupported op: manifest");
        app.on_applied("local", "kb", &sha(), Err(old));
        let (args, _, _, waits) = spawn_with(&mut app, Some("work"));
        assert_eq!((args.len(), waits.len()), (1, 0));

        let mut app = app_with_kb(Some(applied_reply(applied("kb"))));
        app.plugins.config.insert("kb".into(), false);
        app.list_plugins();
        let (args, env, instructions, waits) = spawn_with(&mut app, Some("work"));
        assert_eq!(
            (args.len(), env.len(), instructions.len(), waits.len()),
            (1, 0, 0, 0)
        );
    }

    #[test]
    fn a_reset_sends_the_current_plugin_env_and_instructions_and_no_args() {
        let mut app = app_with_kb(Some(applied_reply(applied("kb"))));
        spawn_with(&mut app, Some("work"));
        let id = app.sessions[0].id;
        app.reset_session(id, false);
        let respawn = app.take_effects().into_iter().find_map(|e| match e {
            Effect::SpawnWithProxy {
                msg,
                plugins,
                pending,
                ..
            } => Some((with_plugins(msg, compose(&plugins)), pending)),
            _ => None,
        });
        let (msg, pending) = respawn.expect("a respawn");
        let Msg::Respawn(spec) = msg else {
            panic!("a respawn")
        };
        assert_eq!(spec.env.len(), 2);
        assert_eq!(spec.extensions.unwrap().instructions, "Use kb.");
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
        let mut app = app_with_kb(Some(applied_reply(a)));
        app.on_plugin_host("devbox");
        let old = std::io::Error::other("unsupported op: manifest");
        app.on_applied("devbox", "kb", &sha(), Err(old));
        assert_eq!(
            app.plugin_details("kb", "work", None),
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
    }

    #[test]
    fn secrets_come_from_enabled_admitted_plugins_of_the_profile() {
        let mut app = app_with(&[]);
        let mut bad = sample("bad");
        bad.env = vec![("LD_PRELOAD".into(), "/x.so".into())];
        let mut other = sample("other");
        other.env = vec![("OTHER_TOKEN".into(), "o".into())];
        app.on_plugins(
            "work",
            PluginsFetch::Listed(list("work", vec![sample("kb"), bad, other])),
        );
        app.on_plugins(
            "home",
            PluginsFetch::Listed(list("home", vec![sample("mine")])),
        );
        let secrets = |app: &App, profile: &str| -> Vec<(String, String)> {
            let (parts, _) = app.plugin_parts("local", profile);
            parts.into_iter().flat_map(|p| p.secrets).collect()
        };
        let pair = |k: &str, v: &str| (k.to_owned(), v.to_owned());
        assert_eq!(
            secrets(&app, "work"),
            [pair("KB_TOKEN", "s3cret"), pair("OTHER_TOKEN", "o")]
        );
        assert!(secrets(&app, "nope").is_empty());
        // Off in the registry: no secrets.
        app.plugins.config.insert("kb".into(), false);
        app.list_plugins();
        assert_eq!(secrets(&app, "work"), [pair("OTHER_TOKEN", "o")]);
    }

    #[test]
    fn cached_lists_are_listed_at_start_and_every_profile_in_use_is_fetched() {
        let mut app = app_with(&[]);
        app.proxy_default = Some("work".into());
        let choices = BTreeMap::from([("kb".to_owned(), false)]);
        app.load_plugins(
            vec![list("home", vec![sample("kb"), sample("mine")])],
            choices,
            BTreeMap::new(),
        );
        let rows: Vec<(&str, bool)> = app
            .extensions
            .extensions
            .iter()
            .filter(|e| e.is_plugin())
            .map(|e| (e.name.as_str(), e.enabled))
            .collect();
        assert_eq!(rows, [("kb", false), ("mine", true)]);
        // Both manifests are fetched, on or off.
        let manifests: Vec<String> = app
            .take_effects()
            .into_iter()
            .filter_map(|e| match e {
                Effect::FetchManifest { name, .. } => Some(name),
                _ => None,
            })
            .collect();
        assert_eq!(manifests, ["kb", "mine"]);
        // Read from the cache, the first fetch under way: no "unreachable".
        app.now = 1000;
        assert_eq!(
            app.plugin_details("mine", "home", None),
            [
                "turning it on or off applies to new sessions",
                "manifest: fetching"
            ]
        );
        app.fetch_plugin_lists();
        let fetched: Vec<String> = app
            .take_effects()
            .into_iter()
            .filter_map(|e| match e {
                Effect::FetchPlugins { profile } => Some(profile),
                _ => None,
            })
            .collect();
        assert_eq!(fetched, ["home", "work"]);
    }

    #[test]
    fn signed_out_drops_the_list_and_unreachable_keeps_it() {
        let mut app = app_with(&[]);
        app.on_plugins(
            "work",
            PluginsFetch::Listed(list("work", vec![sample("kb")])),
        );
        app.on_plugins("work", PluginsFetch::Unreachable);
        assert_eq!(app.plugins.lists["work"].1, ListState::Unreachable);
        assert_eq!(app.plugin_parts("local", "work").0.len(), 1);
        app.now = 100 + 180;
        let details = app.plugin_details("kb", "work", None);
        assert_eq!(details.last().unwrap(), "proxy unreachable, cached 3m");
        app.on_plugins("work", PluginsFetch::SignedOut);
        assert!(app.plugin_parts("local", "work").0.is_empty());
        assert!(!app.extensions.extensions.iter().any(|e| e.name == "kb"));
        let notice = app.notice.as_ref().unwrap().text.clone();
        assert_eq!(
            notice,
            "plugins of work: signed out, run `claudio proxy login`"
        );
        // Unreachable with no cache: nothing to list.
        app.on_plugins("home", PluginsFetch::Unreachable);
        assert!(!app.plugins.lists.contains_key("home"));
    }

    #[test]
    fn r_asks_every_host_again_and_fetches_every_manifest() {
        let mut app = app_with_kb(Some(applied_reply(applied("kb"))));
        app.refresh_plugins();
        let effects = app.take_effects();
        let again = effects
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    Effect::Request { msg, .. } if apply_request_of(msg).is_some()
                )
            })
            .count();
        let fetches = effects
            .iter()
            .filter(|e| matches!(e, Effect::FetchManifest { .. }))
            .count();
        assert_eq!((again, fetches), (1, 1));
        // The answer stays while it is asked again.
        assert!(matches!(
            app.apply_state("local", "kb"),
            Some(ApplyState::Ready(_))
        ));
    }
}
