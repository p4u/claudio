//! Proxy plugins on the client: each profile's list (from the proxy or its
//! cache in `plugins.json`), what each connected host answered for each
//! bundle, and what a spawn gets from them.
//!
//! Pure, like `app.rs`: fetching a list, writing the cache and asking a host
//! for a bundle are [`Effect`]s that `tui/mod.rs` runs. The registry lists
//! the plugins (`Source::Plugin`) so the extensions screen shows and toggles
//! them; turning one off is saved as `[extensions] <name> = false` and
//! applies to new sessions only.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::proto::Msg;
use crate::proxy::plugins::{Plugin, PluginList};

use super::app::{App, Effect, ReplyTo};
use super::extensions::Source;

/// How often every profile's list is fetched again.
pub const REFRESH_SECS: u64 = 15 * 60;

/// How a profile's list was last fetched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListState {
    /// Read from `plugins.json` at start; the first fetch is under way.
    Cached,
    /// From the proxy, this run.
    Fresh,
    /// From the cache; the proxy did not answer (or is too old).
    Unreachable,
}

/// What a fetch of a profile's list gave (`tui/mod.rs` has already saved
/// or dropped the cache entry to match).
#[derive(Debug, Clone, PartialEq)]
pub enum PluginsFetch {
    Listed(PluginList),
    /// 401 or 403: the token is no longer valid; its cached list is gone.
    SignedOut,
    /// Any other failure, or a proxy without plugins (404): the cached list
    /// (if any) stays.
    Unreachable,
}

/// What a host answered for one plugin's bundle.
#[derive(Debug, Clone, PartialEq)]
pub enum BundleState {
    /// Asked; no answer yet.
    Pending,
    /// The zip's path on that host.
    Ready(String),
    /// Claude Code there has the plugin installed: no bundle needed.
    Installed,
    Failed(String),
    /// The daemon does not know `Bundle`.
    TooOld,
}

/// The client's plugin state.
#[derive(Debug, Default)]
pub struct Plugins {
    /// `[extensions]` of config.toml, kept so a plugin that appears later
    /// gets its saved choice (and a toggle updates it).
    pub config: BTreeMap<String, bool>,
    /// Per profile: its list (`None` once signed out) and how it was fetched.
    pub lists: BTreeMap<String, (PluginList, ListState)>,
    /// The hosts whose daemon takes `Bundle` requests now.
    pub hosts: BTreeSet<String>,
    /// Per `(host, plugin)`: the hash asked for and the answer.
    pub bundles: HashMap<(String, String), (String, BundleState)>,
}

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

    /// Every enabled plugin of every profile, in registry order.
    fn all_enabled_plugins(&self) -> Vec<&Plugin> {
        let lists = self.plugins.lists.keys();
        lists
            .flat_map(|profile| self.enabled_plugins(profile))
            .collect()
    }

    /// The env a session on `profile` gets from its enabled plugins.
    pub(super) fn plugin_env(&self, profile: &str) -> Vec<(String, String)> {
        let plugins = self.enabled_plugins(profile);
        plugins
            .into_iter()
            .flat_map(|p| p.env.iter().cloned())
            .collect()
    }

    /// A list arrived (or failed): keep it, list it, and ask the connected
    /// hosts for the bundles it changed.
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
        let hosts: Vec<String> = self.plugins.hosts.iter().cloned().collect();
        for host in hosts {
            self.request_bundles(&host);
        }
        self.redraw = true;
    }

    /// Put every profile's plugins in the registry, with the saved choices.
    pub(super) fn list_plugins(&mut self) {
        let lists: Vec<&PluginList> = self.plugins.lists.values().map(|(l, _)| l).collect();
        self.extensions.set_plugins(&lists, &self.plugins.config);
    }

    /// `host`'s daemon is connected: ask it for every enabled bundle.
    pub fn on_plugin_host(&mut self, host: &str) {
        self.plugins.hosts.insert(host.to_owned());
        // A new connection may be a new daemon: ask again.
        self.plugins.bundles.retain(|(h, _), _| h != host);
        self.request_bundles(host);
    }

    /// `host`'s daemon is gone: its answers no longer hold.
    pub(super) fn plugin_host_gone(&mut self, host: &str) {
        self.plugins.hosts.remove(host);
        self.plugins.bundles.retain(|(h, _), _| h != host);
    }

    /// Ask `host` for each enabled bundle it has not answered for at its
    /// current hash.
    pub(super) fn request_bundles(&mut self, host: &str) {
        let wanted: Vec<Plugin> = self.all_enabled_plugins().into_iter().cloned().collect();
        for p in wanted {
            let key = (host.to_owned(), p.name.clone());
            if self.plugins.bundles.get(&key).is_some_and(|(sha, state)| {
                *sha == p.bundle_sha256 && !matches!(state, BundleState::Failed(_))
            }) {
                continue;
            }
            let sha = p.bundle_sha256.clone();
            self.plugins
                .bundles
                .insert(key, (sha.clone(), BundleState::Pending));
            let to = ReplyTo::Bundle {
                host: host.to_owned(),
                name: p.name.clone(),
                sha256: sha.clone(),
            };
            self.request(host, bundle_msg(&p), to);
        }
    }

    /// `host`'s answer for plugin `name` at hash `sha256`.
    pub(super) fn on_bundled(
        &mut self,
        host: &str,
        name: &str,
        sha256: &str,
        reply: std::io::Result<Msg>,
    ) {
        let key = (host.to_owned(), name.to_owned());
        // An answer for a hash since replaced, or a host since gone.
        if self
            .plugins
            .bundles
            .get(&key)
            .is_none_or(|(sha, _)| sha != sha256)
        {
            return;
        }
        let state = match reply {
            Ok(Msg::Bundled {
                installed: true, ..
            }) => BundleState::Installed,
            Ok(Msg::Bundled {
                path: Some(path), ..
            }) => BundleState::Ready(path),
            Ok(other) => BundleState::Failed(format!("unexpected reply {other:?}")),
            Err(e) if App::older_daemon_notice(host, &e).is_some() => BundleState::TooOld,
            Err(e) => {
                let why = e.to_string();
                let prefix = format!("bundle {name}: ");
                BundleState::Failed(why.strip_prefix(&prefix).unwrap_or(&why).to_owned())
            }
        };
        self.plugins.bundles.insert(key, (sha256.to_owned(), state));
        self.redraw = true;
    }

    /// The bundle state of plugin `name` on `host`, if asked.
    pub fn bundle_state(&self, host: &str, name: &str) -> Option<&BundleState> {
        let key = (host.to_owned(), name.to_owned());
        self.plugins.bundles.get(&key).map(|(_, state)| state)
    }

    /// What the extensions screen says about plugin `name` of `profile`
    /// (beyond its description): one line per host that was asked, and one
    /// for the list.
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
        let mut hosts: Vec<&String> = self.plugins.hosts.iter().collect();
        hosts.sort_by_key(|h| (h.as_str() != "local", h.as_str()));
        for host in hosts {
            let Some(state) = self.bundle_state(host, name) else {
                continue;
            };
            let text = match state {
                BundleState::Pending => "downloading".to_owned(),
                BundleState::Ready(_) => "ready".to_owned(),
                BundleState::Installed => "installed in Claude Code (bundle skipped)".to_owned(),
                BundleState::Failed(why) if why.starts_with("download failed") => why.clone(),
                BundleState::Failed(why) => format!("download failed: {why}"),
                BundleState::TooOld => "daemon too old".to_owned(),
            };
            lines.push(format!("{host}: {text}"));
        }
        if let Some((list, ListState::Unreachable)) = self.plugins.lists.get(profile) {
            let age = super::fmt::fmt_age(self.now.saturating_sub(list.fetched_at));
            lines.push(format!("proxy unreachable, cached {age}"));
        }
        lines
    }

    /// `r` on the extensions screen: fetch every known profile's list again.
    pub(super) fn refresh_plugins(&mut self) {
        self.fetch_plugin_lists();
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

    /// The lists read from `plugins.json` at start, and `[extensions]`.
    pub fn load_plugins(&mut self, cached: Vec<PluginList>, config: BTreeMap<String, bool>) {
        self.plugins.config = config;
        for list in cached {
            let profile = list.profile.clone();
            self.plugins
                .lists
                .insert(profile, (list, ListState::Cached));
        }
        self.list_plugins();
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

/// The `Bundle` request for `p`.
pub fn bundle_msg(p: &Plugin) -> Msg {
    Msg::Bundle {
        name: p.name.clone(),
        url: p.bundle_url.clone(),
        sha256: p.bundle_sha256.clone(),
        installed_id: p.installed_id.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::plugins::sample;
    use crate::tui::test_support::app_with;

    fn list(profile: &str, plugins: Vec<Plugin>) -> PluginList {
        PluginList {
            profile: profile.into(),
            fetched_at: 100,
            plugins,
        }
    }

    /// The `Bundle` requests queued, as (host, name).
    fn bundle_requests(app: &mut App) -> Vec<(String, String)> {
        app.take_effects()
            .into_iter()
            .filter_map(|e| match e {
                Effect::Request {
                    host,
                    msg: Msg::Bundle { name, .. },
                    ..
                } => Some((host, name)),
                _ => None,
            })
            .collect()
    }

    fn pair(host: &str, name: &str) -> (String, String) {
        (host.to_owned(), name.to_owned())
    }

    #[test]
    fn a_list_is_listed_and_its_bundles_asked_of_every_connected_host() {
        let mut app = app_with(&[]);
        app.on_plugin_host("local");
        assert!(bundle_requests(&mut app).is_empty(), "no list yet");
        app.on_plugins(
            "work",
            PluginsFetch::Listed(list("work", vec![sample("kb")])),
        );
        assert_eq!(bundle_requests(&mut app), [pair("local", "kb")]);
        let row = app.extensions.extensions.last().unwrap();
        assert_eq!(
            (row.name.as_str(), row.description.as_str()),
            ("kb", "1.4.0 · work")
        );
        // A second host gets asked on connect; the same list again asks no one.
        app.on_plugin_host("devbox");
        assert_eq!(bundle_requests(&mut app), [pair("devbox", "kb")]);
        app.on_plugins(
            "work",
            PluginsFetch::Listed(list("work", vec![sample("kb")])),
        );
        assert!(bundle_requests(&mut app).is_empty());
        // A new version is asked again.
        let mut v2 = sample("kb");
        v2.bundle_sha256 = "b".repeat(64);
        app.on_plugins("work", PluginsFetch::Listed(list("work", vec![v2])));
        let mut asked = bundle_requests(&mut app);
        asked.sort();
        assert_eq!(asked, [pair("devbox", "kb"), pair("local", "kb")]);
    }

    #[test]
    fn answers_set_each_hosts_state_and_stale_ones_are_dropped() {
        let mut app = app_with(&[]);
        app.on_plugin_host("local");
        app.on_plugins(
            "work",
            PluginsFetch::Listed(list("work", vec![sample("kb")])),
        );
        app.take_effects();
        let sha = "a".repeat(64);
        assert_eq!(app.bundle_state("local", "kb"), Some(&BundleState::Pending));
        let ready = Msg::Bundled {
            name: "kb".into(),
            path: Some("/c/kb.zip".into()),
            installed: false,
        };
        app.on_bundled("local", "kb", &"b".repeat(64), Ok(ready.clone()));
        assert_eq!(
            app.bundle_state("local", "kb"),
            Some(&BundleState::Pending),
            "stale"
        );
        app.on_bundled("local", "kb", &sha, Ok(ready));
        assert_eq!(
            app.bundle_state("local", "kb"),
            Some(&BundleState::Ready("/c/kb.zip".into()))
        );
        let installed = Msg::Bundled {
            name: "kb".into(),
            path: None,
            installed: true,
        };
        app.on_bundled("local", "kb", &sha, Ok(installed));
        assert_eq!(
            app.bundle_state("local", "kb"),
            Some(&BundleState::Installed)
        );
        let old = std::io::Error::other("unsupported op: bundle");
        app.on_bundled("local", "kb", &sha, Err(old));
        assert_eq!(app.bundle_state("local", "kb"), Some(&BundleState::TooOld));
        let failed = std::io::Error::other("bundle kb: hash mismatch");
        app.on_bundled("local", "kb", &sha, Err(failed));
        assert_eq!(
            app.bundle_state("local", "kb"),
            Some(&BundleState::Failed("hash mismatch".into()))
        );
        // A failed download is asked again on the next list arrival.
        app.take_effects();
        app.request_bundles("local");
        assert_eq!(bundle_requests(&mut app), [pair("local", "kb")]);
        assert_eq!(app.bundle_state("local", "kb"), Some(&BundleState::Pending));
        // A host that left forgets its answers.
        app.plugin_host_gone("local");
        assert_eq!(app.bundle_state("local", "kb"), None);
    }

    #[test]
    fn env_comes_from_enabled_admitted_plugins_of_the_profile() {
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
        let pair = |k: &str, v: &str| (k.to_owned(), v.to_owned());
        assert_eq!(
            app.plugin_env("work"),
            [pair("KB_TOKEN", "s3cret"), pair("OTHER_TOKEN", "o")]
        );
        assert!(app.plugin_env("nope").is_empty());
        // Off in the registry: no env.
        app.plugins.config.insert("kb".into(), false);
        app.list_plugins();
        assert_eq!(app.plugin_env("work"), [pair("OTHER_TOKEN", "o")]);
    }

    #[test]
    fn cached_lists_are_listed_at_start_and_every_profile_in_use_is_fetched() {
        let mut app = app_with(&[]);
        app.proxy_default = Some("work".into());
        let choices = BTreeMap::from([("kb".to_owned(), false)]);
        app.load_plugins(
            vec![list("home", vec![sample("kb"), sample("mine")])],
            choices,
        );
        let rows: Vec<(&str, bool)> = app
            .extensions
            .extensions
            .iter()
            .filter(|e| e.is_plugin())
            .map(|e| (e.name.as_str(), e.enabled))
            .collect();
        assert_eq!(rows, [("kb", false), ("mine", true)]);
        // Read from the cache, the first fetch under way: no "unreachable".
        app.now = 1000;
        assert_eq!(app.plugin_details("mine", "home", None).len(), 1);
        app.take_effects();
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
        assert_eq!(app.plugin_env("work").len(), 1);
        app.now = 100 + 180;
        let details = app.plugin_details("kb", "work", None);
        assert_eq!(details.last().unwrap(), "proxy unreachable, cached 3m");
        app.on_plugins("work", PluginsFetch::SignedOut);
        assert!(app.plugin_env("work").is_empty());
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
}
