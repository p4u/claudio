//! Proxy plugins for the web: the web's copy of the TUI's plugin state
//! machine (src/tui/plugins.rs, whose `App` methods are re-expressed here
//! in async code). Each profile's list (plugins.json, then the proxy every
//! 15 min), each plugin's manifest (fetched and hash-checked here, kept in
//! memory), and per (host, plugin) what the host answered (`Applied`).
//!
//! A host is asked for a manifest only once the manifest is fetched at the
//! list's hash and its choices are saved (`[plugins.<name>]`). The request
//! carries secret names and disabled ids only; the secrets themselves go
//! into a spawn's env over the daemon socket and nowhere else: the DTOs
//! here name plugins, versions, ids and reasons, never an env value.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::{broadcast, oneshot};

use super::hub::{daemon_error, HubEvent, HubHandle};
use crate::addons::vocdoni::daemon::{self as apply, Applied};
use crate::addons::vocdoni::plugins::choices::{self, choices_for, PluginChoices};
use crate::addons::vocdoni::plugins::manifest::Manifest;
use crate::addons::vocdoni::plugins::state::{
    reask_after, Apply, ApplyState, ListState, ManifestState, PluginPart, REFRESH_SECS,
};
use crate::addons::vocdoni::proxy::api::ApiError;
use crate::addons::vocdoni::proxy::plugins::{self as cache, Plugin, PluginList};
use crate::addons::vocdoni::proxy::resolve::load_proxy_profiles;
use crate::app::extensions::{Registry, Source};
use crate::proto::Msg;

/// How long a spawn waits for a host to answer its plugin manifests.
const MANIFEST_WAIT: Duration = Duration::from_secs(20);
/// How often the answers are checked for a re-ask (`reask_after`).
const REASK_TICK: Duration = Duration::from_secs(5);
/// The most disabled ids a save takes (the daemon's cap).
const MAX_DISABLED: usize = 256;

pub(crate) struct PluginState {
    /// `[extensions]` of config.toml.
    pub enabled: BTreeMap<String, bool>,
    /// `[plugins.<name>]` of config.toml.
    pub choices: BTreeMap<String, PluginChoices>,
    pub lists: BTreeMap<String, (PluginList, ListState)>,
    /// Per plugin: the manifest hash fetched, and the manifest.
    pub manifests: HashMap<String, (String, ManifestState)>,
    /// Per (host, plugin): what that host answered.
    pub applies: HashMap<(String, String), Apply>,
    /// The connected hosts.
    pub hosts: BTreeSet<String>,
}

#[derive(Clone)]
pub(crate) struct Plugins {
    state: Arc<Mutex<PluginState>>,
    hub: HubHandle,
    registry: Arc<Mutex<Registry>>,
    /// The profiles spawns used this run: their lists are fetched too.
    used: Arc<Mutex<BTreeSet<String>>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// `text` sanitized and cut at `max` characters.
fn clean(text: &str, max: usize) -> String {
    crate::text::sanitize(text).chars().take(max).collect()
}

/// A `notice` event (docs/web-protocol.md §6) for every tab.
pub(crate) fn notice_event(text: &str, level: &str) -> Value {
    json!({
        "t": "ev",
        "ev": "notice",
        "text": clean(text, 1000),
        "level": level,
        "host": null,
        "sid": null,
    })
}

/// The `extensions_changed` event.
pub(crate) fn extensions_changed() -> Value {
    json!({"t": "ev", "ev": "extensions_changed"})
}

/// One `plugin_status` entry (the event minus `t`/`ev`): what `host`
/// answered for plugin `name`. Of an `Applied`, only the ids, reasons and
/// dropped flags.
pub(crate) fn status_dto(host: &str, name: &str, apply: &Apply) -> Value {
    let (state, applied, error) = match &apply.state {
        ApplyState::Pending => ("pending", None, None),
        ApplyState::Ready(a) => ("ready", Some(a.as_ref()), None),
        ApplyState::Failed(why) => ("failed", None, Some(clean(why, 300))),
        ApplyState::TooOld => ("too_old", None, None),
    };
    let ids = |list: &[String]| -> Vec<String> { list.iter().map(|i| clean(i, 300)).collect() };
    let skipped: Vec<Value> = applied
        .map(|a| {
            a.skipped
                .iter()
                .map(|(id, reason)| {
                    json!({
                        "id": clean(id, 300),
                        "reason": clean(reason, 300),
                        "retry_secs": apply::retry_secs(reason),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    json!({
        "host": host,
        "name": name,
        "state": state,
        "installed": applied.map(|a| ids(&a.installed)).unwrap_or_default(),
        "present": applied.map(|a| ids(&a.present)).unwrap_or_default(),
        "skipped": skipped,
        "dropped": applied.map(|a| ids(&a.dropped)).unwrap_or_default(),
        "error": error,
    })
}

/// The `plugins.items` reply for manifest `m`: every item, checked unless
/// the saved choices turn it off (all checked without choices).
pub(crate) fn items_dto(m: &Manifest, choices: Option<&PluginChoices>) -> Value {
    let items: Vec<Value> = m
        .items()
        .into_iter()
        .map(|i| {
            let checked = !choices.is_some_and(|c| c.disabled.contains(&i.id));
            json!({
                "id": clean(&i.id, 300),
                "section": i.section.heading(),
                "title": clean(&i.title, 300),
                "checked": checked,
            })
        })
        .collect();
    json!({
        "name": clean(&m.name, 80),
        "version": clean(&m.version, 80),
        "items": items,
    })
}

/// The `plugins.list` reply: every plugin of every list (refused ones too),
/// with its name, version and profile, never its env or bundle URL.
pub(crate) fn list_dto(st: &PluginState, registry: &Registry) -> Value {
    let mut out = Vec::new();
    for (profile, (list, _)) in &st.lists {
        for (p, refused) in list.checked() {
            let enabled = registry.extensions.iter().any(|e| {
                e.name == p.name
                    && e.enabled
                    && matches!(&e.source, Source::Plugin { profile: q, refused: None } if q == profile)
            });
            let (manifest, error, description) = match (&refused, st.manifests.get(&p.name)) {
                (Some(why), _) => ("failed", Some(clean(why, 300)), String::new()),
                (None, Some((sha, ManifestState::Ready(m)))) if *sha == p.bundle_sha256 => {
                    ("ready", None, clean(&m.description, 1000))
                }
                (None, Some((_, ManifestState::Failed(why)))) => {
                    ("failed", Some(clean(why, 300)), String::new())
                }
                (None, _) => ("fetching", None, String::new()),
            };
            out.push(json!({
                "name": clean(&p.name, 80),
                "version": clean(&p.version, 80),
                "profile": clean(profile, 80),
                "enabled": enabled,
                "refused": refused.map(|r| clean(&r, 300)),
                "choices_saved": st.choices.contains_key(&p.name),
                "manifest": manifest,
                "manifest_error": error,
                "description": description,
            }));
        }
    }
    json!({ "plugins": out })
}

/// Whether a host may be asked for `p`'s manifest: fetched, good, at the
/// list's hash, and the member's choices are saved.
fn requestable(st: &PluginState, p: &Plugin) -> bool {
    let fetched = matches!(
        st.manifests.get(&p.name),
        Some((sha, ManifestState::Ready(_))) if *sha == p.bundle_sha256
    );
    fetched && st.choices.contains_key(&p.name)
}

/// The `vocdoni/apply` request for `p`: its secrets' names (never the
/// values) and the ids the member turned off.
fn manifest_msg(st: &PluginState, p: &Plugin) -> Msg {
    let disabled = st
        .choices
        .get(&p.name)
        .map(|c| c.disabled.clone())
        .unwrap_or_default();
    apply::apply_msg(&apply::ApplyRequest {
        name: p.name.clone(),
        url: p.bundle_url.clone(),
        sha256: p.bundle_sha256.clone(),
        secrets: p.env.iter().map(|(k, _)| k.clone()).collect(),
        disabled,
    })
}

/// What a list fetch gave.
enum Fetch {
    Listed(PluginList),
    SignedOut,
    Unreachable,
}

impl Plugins {
    /// Start from plugins.json's lists and config.toml; spawns the list
    /// refresher (now, then every REFRESH_SECS), the manifest fetcher, the
    /// HubEvent listener (HostUp: ask that host for every requestable
    /// enabled manifest; HostDown: forget its applies; ClaudeInstalled: ask
    /// it again for all) and a 5 s re-ask ticker (`reask_after`).
    pub(crate) fn start(
        hub: HubHandle,
        registry: Arc<Mutex<Registry>>,
        cached: Vec<PluginList>,
        enabled: BTreeMap<String, bool>,
        choices: BTreeMap<String, PluginChoices>,
    ) -> Plugins {
        let lists = cached
            .into_iter()
            .map(|l| (l.profile.clone(), (l, ListState::Cached)))
            .collect();
        let state = PluginState {
            enabled,
            choices,
            lists,
            manifests: HashMap::new(),
            applies: HashMap::new(),
            hosts: BTreeSet::new(),
        };
        let plugins = Plugins {
            state: Arc::new(Mutex::new(state)),
            hub: hub.clone(),
            registry,
            used: Arc::default(),
        };
        plugins.list_plugins();
        plugins.fetch_manifests();
        // Subscribed before the hosts are read: no HostUp is missed.
        let events = hub.subscribe();
        tokio::spawn(plugins.clone().listen(events));
        let refresher = plugins.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(REFRESH_SECS));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                refresher.fetch_lists().await;
            }
        });
        let reasker = plugins.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(REASK_TICK);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                reasker.reask_installing();
            }
        });
        plugins
    }

    /// Follow the hub's host events; on a lag (or at start), resync from
    /// the hub's host list.
    async fn listen(self, mut events: broadcast::Receiver<HubEvent>) {
        self.resync().await;
        loop {
            match events.recv().await {
                Ok(HubEvent::HostUp {
                    host,
                    features,
                    has_claude,
                }) => {
                    // An old daemon answers `Manifest` with "unsupported
                    // op" (too_old); a host without claude is asked anyway
                    // (and again once claude is installed there).
                    tracing::debug!(host, ?features, has_claude, "web: plugins: host up");
                    self.host_up(&host);
                }
                Ok(HubEvent::HostDown { host }) => self.host_gone(&host),
                Ok(HubEvent::ClaudeInstalled { host }) => self.reapply_host(&host),
                Err(broadcast::error::RecvError::Lagged(_)) => self.resync().await,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    }

    async fn resync(&self) {
        let connected: BTreeSet<String> = self
            .hub
            .hosts()
            .await
            .into_iter()
            .filter(|h| h["connected"] == true)
            .filter_map(|h| h["name"].as_str().map(str::to_owned))
            .collect();
        let known = lock(&self.state).hosts.clone();
        for host in known.difference(&connected) {
            self.host_gone(host);
        }
        for host in connected.difference(&known) {
            self.host_up(host);
        }
    }

    /// `host`'s daemon is connected: ask it for every enabled manifest (a
    /// new connection may be a new daemon: earlier answers are dropped).
    fn host_up(&self, host: &str) {
        {
            let mut st = lock(&self.state);
            st.hosts.insert(host.to_owned());
            st.applies.retain(|(h, _), _| h != host);
        }
        self.request_manifests(host, None);
    }

    /// `host`'s daemon is gone: its answers no longer hold.
    fn host_gone(&self, host: &str) {
        let mut st = lock(&self.state);
        st.hosts.remove(host);
        st.applies.retain(|(h, _), _| h != host);
    }

    /// Claude was just installed on `host`: ask it again for every enabled
    /// manifest, answered or not.
    fn reapply_host(&self, host: &str) {
        if !lock(&self.state).hosts.contains(host) {
            return;
        }
        for p in self.all_enabled() {
            self.request_manifests(host, Some(&p.name));
        }
    }

    /// The enabled, admitted plugin rows of the registry, in its order:
    /// `(name, profile)`.
    fn enabled_rows(&self) -> Vec<(String, String)> {
        let registry = lock(&self.registry);
        registry
            .extensions
            .iter()
            .filter(|e| e.enabled)
            .filter_map(|e| match &e.source {
                Source::Plugin {
                    profile,
                    refused: None,
                } => Some((e.name.clone(), profile.clone())),
                _ => None,
            })
            .collect()
    }

    /// The enabled plugins of `profile` (every profile with `None`), in
    /// registry order.
    fn enabled_plugins(&self, profile: Option<&str>) -> Vec<Plugin> {
        let rows = self.enabled_rows();
        let st = lock(&self.state);
        rows.into_iter()
            .filter(|(_, p)| profile.is_none_or(|want| want == p))
            .filter_map(|(name, profile)| {
                let (list, _) = st.lists.get(&profile)?;
                list.plugins.iter().find(|p| p.name == name).cloned()
            })
            .collect()
    }

    fn all_enabled(&self) -> Vec<Plugin> {
        self.enabled_plugins(None)
    }

    /// Put every profile's plugins in the registry, with the saved choices;
    /// whether the registry changed.
    fn list_plugins_quiet(&self) -> bool {
        let st = lock(&self.state);
        let lists: Vec<&PluginList> = st.lists.values().map(|(l, _)| l).collect();
        let mut registry = lock(&self.registry);
        let before = registry.clone();
        registry.set_plugins(&lists, &st.enabled);
        *registry != before
    }

    fn list_plugins(&self) {
        if self.list_plugins_quiet() {
            self.hub.broadcast(extensions_changed());
        }
    }

    /// Fetch the manifest of each admitted plugin whose hash is new.
    fn fetch_manifests(&self) {
        let mut wanted = Vec::new();
        {
            let mut st = lock(&self.state);
            let admitted: Vec<Plugin> = st
                .lists
                .values()
                .flat_map(|(l, _)| l.admitted().into_iter().cloned())
                .collect();
            for p in admitted {
                let known = st.manifests.get(&p.name);
                if known.is_some_and(|(sha, _)| *sha == p.bundle_sha256) {
                    continue;
                }
                let entry = (p.bundle_sha256.clone(), ManifestState::Fetching);
                st.manifests.insert(p.name.clone(), entry);
                wanted.push(p);
            }
        }
        for p in wanted {
            let this = self.clone();
            tokio::spawn(async move {
                let fetched = cache::fetch_manifest(&p.bundle_url, &p.bundle_sha256).await;
                let result = fetched.and_then(|text| Manifest::parse_named(&text, &p.name));
                this.on_manifest(&p.name, &p.bundle_sha256, result);
            });
        }
    }

    /// Plugin `name`'s manifest at hash `sha256` was fetched (and parsed), or not.
    fn on_manifest(&self, name: &str, sha256: &str, result: Result<Manifest, String>) {
        {
            let mut st = lock(&self.state);
            match st.manifests.get_mut(name) {
                Some((sha, state)) if sha == sha256 => {
                    *state = match result {
                        Ok(m) => ManifestState::Ready(Box::new(m)),
                        Err(why) => {
                            tracing::warn!(plugin = name, "web: manifest refused: {why}");
                            ManifestState::Failed(why)
                        }
                    };
                }
                // A hash since replaced.
                _ => return,
            }
        }
        self.request_all();
    }

    /// Fetch the list of every profile listed, used, or the default.
    async fn fetch_lists(&self) {
        let (_, default) = tokio::task::spawn_blocking(load_proxy_profiles)
            .await
            .unwrap_or_default();
        let mut profiles: BTreeSet<String> = lock(&self.state).lists.keys().cloned().collect();
        profiles.extend(lock(&self.used).iter().cloned());
        profiles.extend(default);
        for profile in profiles {
            let this = self.clone();
            tokio::spawn(async move { this.fetch_list(&profile).await });
        }
    }

    /// Fetch `profile`'s list and keep plugins.json in step: a list is
    /// saved, a 401/403 drops the profile's entry (its secrets).
    async fn fetch_list(&self, profile: &str) {
        let fetch = match super::proxy::ProxyCache::profile(profile).await {
            Err(_) => Fetch::Unreachable,
            Ok(p) => match cache::fetch(profile, &p).await {
                Ok(list) => {
                    tracing::debug!(profile, "web: plugins: {}", list.summary());
                    let saved = list.clone();
                    if let Ok(Err(e)) =
                        tokio::task::spawn_blocking(move || cache::save_cache(&saved)).await
                    {
                        self.hub.broadcast(notice_event(
                            &format!("could not save plugins: {e}"),
                            "warn",
                        ));
                    }
                    Fetch::Listed(list)
                }
                Err(ApiError::Http(s)) if matches!(s.as_u16(), 401 | 403) => {
                    let name = profile.to_owned();
                    if let Ok(Err(e)) =
                        tokio::task::spawn_blocking(move || cache::remove_cache(&name)).await
                    {
                        self.hub.broadcast(notice_event(
                            &format!("could not save plugins: {e}"),
                            "warn",
                        ));
                    }
                    Fetch::SignedOut
                }
                Err(_) => Fetch::Unreachable,
            },
        };
        self.on_plugins(profile, fetch);
    }

    /// A list arrived (or failed): keep it, list it, fetch the manifests it
    /// changed and ask the connected hosts for them.
    fn on_plugins(&self, profile: &str, fetch: Fetch) {
        {
            let mut st = lock(&self.state);
            match fetch {
                Fetch::Listed(list) => {
                    st.lists
                        .insert(profile.to_owned(), (list, ListState::Fresh));
                }
                Fetch::SignedOut => {
                    st.lists.remove(profile);
                    drop(st);
                    let text =
                        format!("plugins of {profile}: signed out, run `claudio proxy login`");
                    self.hub.broadcast(notice_event(&text, "warn"));
                }
                Fetch::Unreachable => {
                    if let Some((_, state)) = st.lists.get_mut(profile) {
                        *state = ListState::Unreachable;
                    }
                }
            }
        }
        self.list_plugins();
        self.fetch_manifests();
        self.request_all();
    }

    fn hosts(&self) -> Vec<String> {
        lock(&self.state).hosts.iter().cloned().collect()
    }

    fn request_all(&self) {
        for host in self.hosts() {
            self.request_manifests(&host, None);
        }
    }

    /// Ask every connected host for plugin `name`'s manifest again, even
    /// where it answered (its choices changed).
    fn request_again(&self, name: &str) {
        for host in self.hosts() {
            self.request_manifests(&host, Some(name));
        }
    }

    /// Ask `host` for each enabled plugin's manifest it has not answered for
    /// at its current hash (a failure is asked again); `again`: that
    /// plugin's whatever the answer.
    fn request_manifests(&self, host: &str, again: Option<&str>) {
        for p in self.all_enabled() {
            let ask = {
                let st = lock(&self.state);
                if !requestable(&st, &p) {
                    continue;
                }
                let key = (host.to_owned(), p.name.clone());
                let answered = st.applies.get(&key).is_some_and(|a| {
                    a.sha == p.bundle_sha256 && !matches!(a.state, ApplyState::Failed(_))
                });
                !answered || again == Some(p.name.as_str())
            };
            if ask {
                drop(self.ask(host, &p));
            }
        }
    }

    /// Ask again a host whose answer still lists an install under way, or
    /// a failed one to retry once it is due.
    fn reask_installing(&self) {
        let now = crate::paths::unix_now();
        let due: Vec<(String, String)> = {
            let st = lock(&self.state);
            st.applies
                .iter()
                .filter(|(_, a)| {
                    let after = match &a.state {
                        ApplyState::Ready(applied) => reask_after(applied),
                        _ => None,
                    };
                    after.is_some_and(|after| now.saturating_sub(a.asked) >= after)
                })
                .map(|(key, _)| key.clone())
                .collect()
        };
        if due.is_empty() {
            return;
        }
        let enabled = self.all_enabled();
        for (host, name) in due {
            let plugin = enabled.iter().find(|p| p.name == name);
            let ok = plugin.is_some_and(|p| requestable(&lock(&self.state), p));
            if let Some(p) = plugin.filter(|_| ok) {
                drop(self.ask(&host, p));
            }
        }
    }

    /// Send `host` the `Manifest` request for `p` (a `Ready` answer at the
    /// same hash stays until the new one comes); the receiver gets the
    /// `Applied`, if the host answered with one.
    fn ask(&self, host: &str, p: &Plugin) -> oneshot::Receiver<Option<Applied>> {
        let now = crate::paths::unix_now();
        let key = (host.to_owned(), p.name.clone());
        let (msg, changed) = {
            let mut st = lock(&self.state);
            let old = st.applies.remove(&key);
            let apply = match old.clone() {
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
            let changed = old.as_ref().map(|a| &a.state) != Some(&apply.state);
            st.applies.insert(key, apply.clone());
            (manifest_msg(&st, p), changed.then_some(apply))
        };
        if let Some(apply) = changed {
            self.send_status(host, &p.name, &apply);
        }
        let (tx, rx) = oneshot::channel();
        let this = self.clone();
        let (host, name, sha) = (host.to_owned(), p.name.clone(), p.bundle_sha256.clone());
        tokio::spawn(async move {
            let reply = match this.hub.host_client(&host).await {
                Ok((c, _)) if !apply::serves_apply(&c.welcome().features) => {
                    Err("unsupported op: older than addons, not asked".into())
                }
                Ok((client, _)) => client.request(msg).await.map_err(|e| e.to_string()),
                Err(e) => Err(e),
            };
            let applied = reply.clone().ok().and_then(|r| apply::applied_of(r).ok());
            this.on_applied(&host, &name, &sha, reply);
            let _ = tx.send(applied);
        });
        rx
    }

    /// `host`'s answer for plugin `name`'s manifest at hash `sha256`.
    fn on_applied(&self, host: &str, name: &str, sha256: &str, reply: Result<Msg, String>) {
        let key = (host.to_owned(), name.to_owned());
        let changed = {
            let mut st = lock(&self.state);
            // An answer for a hash since replaced, or a host since gone.
            let Some(apply) = st.applies.get_mut(&key) else {
                return;
            };
            if apply.sha != sha256 {
                return;
            }
            let state = match reply {
                Ok(reply) => ApplyState::of_reply(reply),
                Err(e) if e.starts_with("unsupported op") => ApplyState::TooOld,
                Err(e) => {
                    let prefix = format!("manifest {name}: ");
                    ApplyState::Failed(daemon_error(e.strip_prefix(&prefix).unwrap_or(&e)))
                }
            };
            (apply.state != state).then(|| {
                apply.state = state;
                apply.clone()
            })
        };
        if let Some(apply) = changed {
            self.send_status(host, name, &apply);
        }
    }

    fn send_status(&self, host: &str, name: &str, apply: &Apply) {
        let mut v = status_dto(host, name, apply);
        v["t"] = "ev".into();
        v["ev"] = "plugin_status".into();
        self.hub.broadcast(v);
    }

    /// What `profile`'s enabled plugins bring to a spawn on `host`, in
    /// registry order, minus `skip` and minus plugins without saved choices.
    /// Waits ≤ 20 s for this host's pending `Manifest` answers (sending a
    /// `notice` "applying plugin …" when it has to wait).
    pub(crate) async fn parts(
        &self,
        host: &str,
        profile: &str,
        skip: &[String],
    ) -> Vec<PluginPart> {
        self.parts_with(host, profile, skip, true).await
    }

    /// [`Plugins::parts`] without waiting (a respawn, as the TUI's reset).
    pub(crate) async fn parts_now(&self, host: &str, profile: &str) -> Vec<PluginPart> {
        self.parts_with(host, profile, &[], false).await
    }

    async fn parts_with(
        &self,
        host: &str,
        profile: &str,
        skip: &[String],
        wait: bool,
    ) -> Vec<PluginPart> {
        self.use_profile(profile);
        let mut parts = Vec::new();
        let mut pending = Vec::new();
        for p in self.enabled_plugins(Some(profile)) {
            if skip.contains(&p.name) {
                continue;
            }
            let st = lock(&self.state);
            if !st.choices.contains_key(&p.name) {
                continue;
            }
            let key = (host.to_owned(), p.name.clone());
            let applied = match st.applies.get(&key).map(|a| &a.state) {
                Some(ApplyState::Ready(a)) => Some(Applied::clone(a)),
                Some(ApplyState::Pending) | None if requestable(&st, &p) => {
                    pending.push(p.clone());
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
        if !wait || pending.is_empty() {
            return parts;
        }
        let names: Vec<&str> = pending.iter().map(|p| p.name.as_str()).collect();
        let text = format!("applying plugin {}…", names.join(", "));
        self.hub.broadcast(notice_event(&text, "info"));
        let deadline = tokio::time::Instant::now() + MANIFEST_WAIT;
        let asks: Vec<(String, oneshot::Receiver<Option<Applied>>)> = pending
            .iter()
            .map(|p| (p.name.clone(), self.ask(host, p)))
            .collect();
        let mut left_out = Vec::new();
        for (name, answer) in asks {
            match tokio::time::timeout_at(deadline, answer).await {
                Ok(Ok(Some(applied))) => {
                    if let Some(part) = parts.iter_mut().find(|p| p.name == name) {
                        part.applied = Some(applied);
                    }
                }
                _ => left_out.push(name),
            }
        }
        if !left_out.is_empty() {
            let text = format!(
                "plugin {} left out: not ready on {host}",
                left_out.join(", ")
            );
            self.hub.broadcast(notice_event(&text, "warn"));
        }
        parts
    }

    /// A spawn used `profile`: its list is fetched now if it has none.
    fn use_profile(&self, profile: &str) {
        let new = lock(&self.used).insert(profile.to_owned());
        if new && !lock(&self.state).lists.contains_key(profile) {
            let this = self.clone();
            let profile = profile.to_owned();
            tokio::spawn(async move { this.fetch_list(&profile).await });
        }
    }

    pub(crate) fn list(&self) -> Value {
        let st = lock(&self.state);
        let registry = lock(&self.registry);
        list_dto(&st, &registry)
    }

    /// The manifest of plugin `name`, once fetched.
    fn manifest(&self, name: &str) -> Result<Manifest, String> {
        let st = lock(&self.state);
        match st.manifests.get(name) {
            Some((_, ManifestState::Ready(m))) => Ok(Manifest::clone(m)),
            Some((_, ManifestState::Fetching)) => {
                Err("the manifest is still being fetched".to_owned())
            }
            Some((_, ManifestState::Failed(why))) => Err(format!("manifest: {}", clean(why, 300))),
            None => Err("no such plugin".to_owned()),
        }
    }

    pub(crate) fn items(&self, name: &str) -> Result<Value, String> {
        let m = self.manifest(name)?;
        let st = lock(&self.state);
        Ok(items_dto(&m, st.choices.get(name)))
    }

    /// choices_for + the config editor (spawn_blocking), then ask every host again.
    pub(crate) async fn save(&self, name: &str, disabled: Vec<String>) -> Result<(), String> {
        let m = self.manifest(name)?;
        if disabled.len() > MAX_DISABLED {
            return Err(format!("bad request: disabled: at most {MAX_DISABLED} ids"));
        }
        let ids = m.ids();
        if let Some(bad) = disabled.iter().find(|d| !ids.contains(d)) {
            return Err(format!(
                "bad request: disabled: no item {:?}",
                clean(bad, 80)
            ));
        }
        let earlier = lock(&self.state).choices.get(name).cloned();
        let choices = choices_for(&m, &disabled, earlier.as_ref());
        let (owned, saved) = (name.to_owned(), choices.clone());
        tokio::task::spawn_blocking(move || {
            let set = |t: &mut toml::Table| choices::set(t, &owned, &saved);
            crate::app::config_edit::edit_section(&crate::paths::config_file(), "plugins", set)
        })
        .await
        .map_err(|_| "cannot write config.toml".to_owned())?
        .map_err(|e| e.to_string())?;
        tracing::info!(op = "plugins.save", plugin = name, "web op");
        lock(&self.state).choices.insert(name.to_owned(), choices);
        self.request_again(name);
        Ok(())
    }

    pub(crate) fn status(&self) -> Value {
        let st = lock(&self.state);
        let mut keys: Vec<&(String, String)> = st.applies.keys().collect();
        keys.sort();
        let status: Vec<Value> = keys
            .into_iter()
            .map(|k| status_dto(&k.0, &k.1, &st.applies[k]))
            .collect();
        json!({ "status": status })
    }

    /// config re-read; asks again where choices changed.
    pub(crate) fn set_config(
        &self,
        enabled: BTreeMap<String, bool>,
        choices: BTreeMap<String, PluginChoices>,
    ) {
        let changed: Vec<String> = {
            let mut st = lock(&self.state);
            let names: BTreeSet<&String> = st.choices.keys().chain(choices.keys()).collect();
            let changed = names
                .into_iter()
                .filter(|n| st.choices.get(*n) != choices.get(*n))
                .cloned()
                .collect();
            st.enabled = enabled;
            st.choices = choices;
            changed
        };
        self.list_plugins_quiet();
        for name in &changed {
            self.request_again(name);
        }
        self.request_all();
    }

    pub(crate) fn toggle(&self, name: &str, on: bool) {
        lock(&self.state).enabled.insert(name.to_owned(), on);
        if on {
            self.request_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"format = 1
name = "vocdoni-test"
version = "1.0.0"
description = "Test setup"
instructions = "Manifest instructions for the test."

[[mcp]]
name = "kb"
type = "http"
url = "https://kb.example.org/mcp"
headers = { Authorization = "Bearer ${VOCDONI_TEST_TOKEN}" }
secrets = ["VOCDONI_TEST_TOKEN"]

[[agent]]
name = "reviewer"
description = "Reviews"
prompt = "Review the change."
"#;

    #[test]
    fn items_mark_disabled_ids_unchecked() {
        let m = Manifest::parse(MANIFEST).unwrap();
        let choices = PluginChoices {
            disabled: vec!["mcp:kb".into()],
            ..Default::default()
        };
        let v = items_dto(&m, Some(&choices));
        assert_eq!(v["name"], "vocdoni-test");
        assert_eq!(v["version"], "1.0.0");
        let items = v["items"].as_array().unwrap();
        assert!(items.len() >= 3, "{v}");
        for item in items {
            assert_eq!(item["checked"], item["id"] != "mcp:kb", "{item}");
        }
        let kb = items.iter().find(|i| i["id"] == "mcp:kb").unwrap();
        assert_eq!(kb["section"], "MCP servers");
        let v = items_dto(&m, None);
        assert!(v["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["checked"] == true));
    }

    #[test]
    fn status_dto_carries_retry_seconds() {
        let applied = Applied {
            name: "team".into(),
            version: "1.0.0".into(),
            mcp_config: None,
            agents: None,
            instructions: String::new(),
            env: vec![("REGION".into(), "eu-secret-ish".into())],
            installed: vec!["plugin:a@m".into()],
            present: vec![],
            skipped: vec![
                ("plugin:b@m".into(), "down (retrying in 15 s)".into()),
                ("plugin:c@m".into(), "installing".into()),
            ],
            dropped: vec!["--agents".into()],
        };
        let apply = Apply {
            sha: "a".repeat(64),
            state: ApplyState::Ready(Box::new(applied)),
            asked: 0,
        };
        let v = status_dto("local", "team", &apply);
        assert_eq!(v["state"], "ready");
        assert_eq!(v["skipped"][0]["retry_secs"], 15);
        assert!(v["skipped"][1]["retry_secs"].is_null());
        assert_eq!(v["installed"], json!(["plugin:a@m"]));
        assert_eq!(v["dropped"], json!(["--agents"]));
        assert!(v["error"].is_null());
        assert!(!v.to_string().contains("eu-secret-ish"), "no env value");
        let failed = Apply {
            state: ApplyState::Failed("boom".into()),
            ..apply.clone()
        };
        let v = status_dto("local", "team", &failed);
        assert_eq!(
            (v["state"].as_str(), v["error"].as_str()),
            (Some("failed"), Some("boom"))
        );
        let old = Apply {
            state: ApplyState::TooOld,
            ..apply
        };
        assert_eq!(status_dto("h", "team", &old)["state"], "too_old");
    }

    #[test]
    fn list_dto_has_no_env() {
        let mut p = crate::addons::vocdoni::proxy::plugins::sample("vocdoni-test");
        p.env = vec![("VOCDONI_TEST_TOKEN".into(), "plugin-SECRET-value".into())];
        let list = PluginList {
            profile: "env".into(),
            fetched_at: 0,
            plugins: vec![p.clone()],
        };
        let mut registry = Registry::default();
        registry.set_plugins(&[&list], &BTreeMap::new());
        let mut st = PluginState {
            enabled: BTreeMap::new(),
            choices: BTreeMap::new(),
            lists: BTreeMap::from([("env".to_owned(), (list, ListState::Fresh))]),
            manifests: HashMap::new(),
            applies: HashMap::new(),
            hosts: BTreeSet::new(),
        };
        let v = list_dto(&st, &registry);
        let text = v.to_string();
        assert!(!text.contains("plugin-SECRET-value"), "{text}");
        assert!(!text.contains(&p.bundle_url), "{text}");
        let row = &v["plugins"][0];
        assert_eq!(row["name"], "vocdoni-test");
        assert_eq!(row["profile"], "env");
        assert_eq!(row["enabled"], true);
        assert_eq!(row["choices_saved"], false);
        assert_eq!(row["manifest"], "fetching");
        st.choices
            .insert("vocdoni-test".into(), PluginChoices::default());
        let m = Manifest::parse(MANIFEST).unwrap();
        st.manifests.insert(
            "vocdoni-test".into(),
            (p.bundle_sha256.clone(), ManifestState::Ready(Box::new(m))),
        );
        let v = list_dto(&st, &registry);
        assert_eq!(v["plugins"][0]["choices_saved"], true);
        assert_eq!(v["plugins"][0]["manifest"], "ready");
        assert_eq!(v["plugins"][0]["description"], "Test setup");
    }
}
