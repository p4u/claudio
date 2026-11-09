//! The Vocdoni addon's client state (A5, A6): the profiles, the proxy
//! config cache, and the one plugin state machine (each profile's list,
//! each plugin's manifest, what each host answered, the member's choices).
//! Pure: every transition returns the [`Task`]s the runner must do (fetch,
//! ask a host, save, tell the front-ends), and touches no file, socket or
//! clock. Ported from the TUI's plugin glue (`tui/plugins_glue.rs`), whose
//! `self.emit`/`self.request` calls are the tasks here.
//!
//! [`lists`] holds the lists, manifests and choices; [`applies`] what the
//! hosts are asked and answered, and a spawn's share.

mod applies;
mod lists;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::addons::vocdoni::daemon::ApplyRequest;
use crate::addons::vocdoni::plugins::choices::PluginChoices;
use crate::addons::vocdoni::plugins::state::{ListState, ManifestState, Plugins, REFRESH_SECS};
use crate::addons::vocdoni::profiles::Profiles;
use crate::addons::vocdoni::proxy::api::ConfigResponse;
use crate::addons::vocdoni::proxy::plugins::PluginList;

/// How long a profile's proxy config is kept (seconds).
pub const CONFIG_TTL: u64 = 5 * 60;

/// What the runner must do after a transition.
#[derive(Debug, Clone, PartialEq)]
pub enum Task {
    /// Fetch `profile`'s plugin list (and keep `plugins.json` in step):
    /// the outcome goes to [`State::on_list`].
    FetchList { profile: String },
    /// Fetch plugin `name`'s manifest at hash `sha256` and parse it: the
    /// outcome goes to [`State::on_manifest`].
    FetchManifest {
        name: String,
        url: String,
        sha256: String,
    },
    /// Ask `host` to apply plugin `name`'s manifest at `sha256`: the
    /// answer goes to [`State::on_applied`].
    Ask {
        host: String,
        name: String,
        sha256: String,
        body: ApplyRequest,
    },
    /// Write `[plugins.<name>]` to config.toml.
    SaveChoices {
        name: String,
        choices: PluginChoices,
    },
    /// A notice for the member.
    Notice(String),
    /// What the front-ends show changed.
    Changed,
}

/// The addon's client state.
#[derive(Default)]
pub struct State {
    /// The proxy profiles (the env profile alone when it is set).
    pub profiles: Vec<String>,
    /// The default profile.
    pub default: Option<String>,
    /// The profiles marked `first_party = true`.
    pub first_party: BTreeSet<String>,
    /// Whether the profiles were read (a launch reads them once if not).
    pub profiles_read: bool,
    /// Per profile: its proxy config and when it was fetched (Unix seconds).
    pub config: HashMap<String, (ConfigResponse, u64)>,
    /// Lists, manifests, host answers, choices, `[extensions]`.
    pub plugins: Plugins,
    /// Extension names the front-end's own extensions hold: a plugin of
    /// that name is refused ("name taken"), as its row is.
    pub taken: BTreeSet<String>,
    /// The profiles sessions use: their lists are fetched too.
    pub used: BTreeSet<String>,
    /// Whether [`State::load`] ran: nothing is fetched before.
    pub loaded: bool,
    /// When every list was last fetched.
    pub refreshed: u64,
    /// Unix seconds, as the runner last set it.
    pub now: u64,
}

impl State {
    /// The start: the profiles, `[plugins]`, the `[extensions]` entries of
    /// the cached lists' plugins, and the lists cached in `plugins.json`.
    /// Their manifests are fetched, and every profile's list.
    pub fn load(
        &mut self,
        profiles: (Vec<String>, Option<String>),
        choices: BTreeMap<String, PluginChoices>,
        extensions: BTreeMap<String, bool>,
        cached: Vec<PluginList>,
    ) -> Vec<Task> {
        (self.profiles, self.default) = profiles;
        self.profiles_read = true;
        self.plugins.choices = choices;
        self.known(extensions);
        for list in cached {
            let profile = list.profile.clone();
            self.plugins
                .lists
                .insert(profile, (list, ListState::Cached));
        }
        self.loaded = true;
        self.refreshed = self.now;
        let mut tasks = Vec::new();
        self.fetch_manifests(false, &mut tasks);
        tasks.extend(self.fetch_lists());
        tasks.push(Task::Changed);
        tasks
    }

    /// config.toml changed: the profiles and `[plugins]` again. A plugin
    /// whose choices changed is asked of every host again. (`[extensions]`
    /// stays: its entries change through [`State::set_enabled`].)
    pub fn reload(
        &mut self,
        profiles: (Vec<String>, Option<String>),
        choices: BTreeMap<String, PluginChoices>,
    ) -> Vec<Task> {
        (self.profiles, self.default) = profiles;
        self.profiles_read = true;
        let names: BTreeSet<String> = self
            .plugins
            .choices
            .keys()
            .chain(choices.keys())
            .cloned()
            .collect();
        let changed: Vec<String> = names
            .into_iter()
            .filter(|n| self.plugins.choices.get(n) != choices.get(n))
            .collect();
        self.plugins.choices = choices;
        let mut tasks = Vec::new();
        for name in &changed {
            self.request_again(name, &mut tasks);
        }
        self.request_all(&mut tasks);
        if !changed.is_empty() {
            tasks.push(Task::Changed);
        }
        tasks
    }

    /// The profiles as read, if they were.
    pub fn profiles(&self) -> Option<Profiles> {
        self.profiles_read.then(|| Profiles {
            names: self.profiles.clone(),
            default: self.default.clone(),
            first_party: self.first_party.clone(),
        })
    }

    /// The profiles were read: names, default and first-party ones.
    pub fn set_profiles(&mut self, p: Profiles) {
        (self.profiles, self.default) = (p.names, p.default);
        self.first_party = p.first_party;
        self.profiles_read = true;
    }

    /// `[extensions]` entries for plugin names it has none for yet (a
    /// toggle since read wins).
    pub fn known(&mut self, extensions: BTreeMap<String, bool>) {
        for (name, on) in extensions {
            self.plugins.config.entry(name).or_insert(on);
        }
    }

    /// The profiles sessions use now: a new one's list is fetched.
    pub fn use_profiles(&mut self, profiles: BTreeSet<String>) -> Vec<Task> {
        let mut tasks = Vec::new();
        for profile in profiles {
            if !self.used.insert(profile.clone()) {
                continue;
            }
            if self.loaded && !self.plugins.lists.contains_key(&profile) {
                tasks.push(Task::FetchList { profile });
            }
        }
        tasks
    }

    /// The second's work at `now`: the hosts due to be asked again, and
    /// every list once [`REFRESH_SECS`] have passed.
    pub fn tick(&mut self, now: u64) -> Vec<Task> {
        self.now = now;
        let mut tasks = Vec::new();
        self.reask_installing(&mut tasks);
        if self.loaded && now.saturating_sub(self.refreshed) >= REFRESH_SECS {
            self.refreshed = now;
            tasks.extend(self.fetch_lists());
        }
        tasks
    }

    /// The list of every profile listed, used or the default.
    pub fn fetch_lists(&self) -> Vec<Task> {
        let mut profiles: BTreeSet<String> = self.plugins.lists.keys().cloned().collect();
        profiles.extend(self.used.iter().cloned());
        profiles.extend(self.default.iter().cloned());
        profiles
            .into_iter()
            .map(|profile| Task::FetchList { profile })
            .collect()
    }

    /// `profile`'s proxy config was fetched (`None`: it was not).
    pub fn on_config(&mut self, profile: &str, config: Option<ConfigResponse>) {
        if let Some(config) = config {
            self.config.insert(profile.to_owned(), (config, self.now));
        }
    }

    /// `profile`'s proxy config, if fetched in the last [`CONFIG_TTL`].
    pub fn config_cached(&self, profile: &str) -> Option<&ConfigResponse> {
        let (config, at) = self.config.get(profile)?;
        (self.now.saturating_sub(*at) < CONFIG_TTL).then_some(config)
    }

    /// The setup line of plugin `name`'s details.
    pub fn setup_details(&self, name: &str) -> Option<String> {
        let ids = match self.plugins.manifests.get(name) {
            Some((_, ManifestState::Ready(m))) => m.ids(),
            _ => return None,
        };
        match self.plugins.choices.get(name) {
            None => Some("setup pending (Enter)".to_owned()),
            Some(c) => {
                let off = c.disabled.iter().filter(|d| ids.contains(d)).count();
                Some(format!("{off} items disabled (Enter to change)"))
            }
        }
    }

    /// What the extensions screen says about plugin `name` of `profile`
    /// (beyond its version): its manifest, one line per host that was
    /// asked, and the list's state.
    pub fn plugin_details(&self, name: &str, profile: &str, refused: Option<&str>) -> Vec<String> {
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
        lines.extend(self.host_lines(name));
        if manifest.is_some_and(|m| !m.plugins.is_empty()) {
            lines.push(
                "installed plugins stay until you remove them: claude plugin uninstall <id>"
                    .to_owned(),
            );
        }
        if let Some((list, ListState::Unreachable)) = self.plugins.lists.get(profile) {
            let age = fmt_age(self.now.saturating_sub(list.fetched_at));
            lines.push(format!("proxy unreachable, cached {age}"));
        }
        lines
    }
}

/// `42s`, `3m`, `5h`, `2d`.
fn fmt_age(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86_399 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86_400),
    }
}
