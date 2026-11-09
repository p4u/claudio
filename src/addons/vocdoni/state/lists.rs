//! The lists, the manifests and the member's choices: which plugins are on,
//! what arrived, and what the member saved or put off.

use super::{State, Task};
use crate::addons::vocdoni::plugins::choices::PluginChoices;
use crate::addons::vocdoni::plugins::state::{ListState, ManifestState, PluginsFetch};
use crate::addons::vocdoni::proxy::plugins::Plugin;

/// A launch's plugins without saved choices ([`State::unreviewed`]).
#[derive(Debug, Default, PartialEq)]
pub struct Unreviewed {
    /// Their manifest is here: `(name, hash)`, to review.
    pub due: Vec<(String, String)>,
    /// Their manifest is still being fetched.
    pub fetching: Vec<String>,
    /// Their manifest failed: `(name, why)`.
    pub failed: Vec<(String, String)>,
}

/// One plugin of a list as the extensions screen lists it.
pub struct Row<'a> {
    pub profile: &'a str,
    pub plugin: &'a Plugin,
    /// Why it is refused (its list's checks, or a name already taken).
    pub refused: Option<String>,
    /// Admitted and not turned off.
    pub on: bool,
}

impl State {
    /// Every plugin of every list, in profile order then list order, as the
    /// registry's plugin rows: refused by its list's checks or when its
    /// name is taken (by an extension or an earlier row), on unless refused
    /// or turned off in `[extensions]`.
    pub fn rows(&self) -> Vec<Row<'_>> {
        let mut rows: Vec<Row<'_>> = Vec::new();
        for (list, _) in self.plugins.lists.values() {
            for (plugin, why) in list.checked() {
                let taken = self.taken.contains(&plugin.name)
                    || rows.iter().any(|r| r.plugin.name == plugin.name);
                let refused = why.or_else(|| taken.then(|| "name taken".to_owned()));
                let off = self.plugins.config.get(&plugin.name) == Some(&false);
                rows.push(Row {
                    profile: &list.profile,
                    plugin,
                    on: refused.is_none() && !off,
                    refused,
                });
            }
        }
        rows
    }

    /// The enabled plugins of `profile`, in registry order.
    pub fn enabled_plugins(&self, profile: &str) -> Vec<&Plugin> {
        self.rows()
            .into_iter()
            .filter(|r| r.on && r.profile == profile)
            .map(|r| r.plugin)
            .collect()
    }

    /// Every enabled plugin of every profile, in registry order.
    pub(super) fn all_enabled(&self) -> Vec<&Plugin> {
        let rows = self.rows();
        rows.into_iter()
            .filter(|r| r.on)
            .map(|r| r.plugin)
            .collect()
    }

    /// Whether plugin `name` (of any profile) is on.
    pub fn plugin_enabled(&self, name: &str) -> bool {
        self.all_enabled().iter().any(|p| p.name == name)
    }

    /// Every admitted plugin of every list (on or off).
    fn admitted(&self) -> Vec<Plugin> {
        let lists = self.plugins.lists.values();
        lists
            .flat_map(|(list, _)| list.admitted().into_iter().cloned())
            .collect()
    }

    /// A list arrived (or failed): keep it, fetch the manifests it changed
    /// and ask the connected hosts for them.
    pub fn on_list(&mut self, profile: &str, fetch: PluginsFetch) -> Vec<Task> {
        let mut tasks = Vec::new();
        self.listed.insert(profile.to_owned());
        match fetch {
            PluginsFetch::Listed(list) => {
                self.plugins
                    .lists
                    .insert(profile.to_owned(), (list, ListState::Fresh));
            }
            PluginsFetch::SignedOut => {
                self.plugins.lists.remove(profile);
                tasks.push(Task::Notice(format!(
                    "plugins of {profile}: signed out, run `claudio proxy login`"
                )));
            }
            PluginsFetch::Unreachable => {
                if let Some((_, state)) = self.plugins.lists.get_mut(profile) {
                    *state = ListState::Unreachable;
                }
            }
        }
        self.fetch_manifests(false, &mut tasks);
        self.request_all(&mut tasks);
        tasks.push(Task::Changed);
        tasks
    }

    /// Fetch the manifest of each admitted plugin whose hash is new (every
    /// one when `all`).
    pub(super) fn fetch_manifests(&mut self, all: bool, tasks: &mut Vec<Task>) {
        for p in self.admitted() {
            let known = self.plugins.manifests.get(&p.name);
            if !all && known.is_some_and(|(sha, _)| *sha == p.bundle_sha256) {
                continue;
            }
            let entry = (p.bundle_sha256.clone(), ManifestState::Fetching);
            self.plugins.manifests.insert(p.name.clone(), entry);
            tasks.push(Task::FetchManifest {
                name: p.name,
                url: p.bundle_url,
                sha256: p.bundle_sha256,
            });
        }
    }

    /// Plugin `name`'s manifest at hash `sha256` was fetched (and parsed),
    /// or not. A hash since replaced is dropped.
    pub fn on_manifest(
        &mut self,
        name: &str,
        sha256: &str,
        result: Result<crate::addons::vocdoni::plugins::manifest::Manifest, String>,
    ) -> Vec<Task> {
        match self.plugins.manifests.get_mut(name) {
            Some((sha, state)) if sha == sha256 => {
                *state = match result {
                    Ok(m) => ManifestState::Ready(Box::new(m)),
                    Err(why) => ManifestState::Failed(why),
                };
            }
            _ => return Vec::new(),
        }
        let mut tasks = Vec::new();
        self.on_manifest_ready(name, sha256, &mut tasks);
        self.request_all(&mut tasks);
        self.offer_due(&mut tasks);
        tasks.push(Task::Changed);
        tasks
    }

    /// Offer each first-sight setup now due, once per run.
    fn offer_due(&mut self, tasks: &mut Vec<Task>) {
        for name in self.first_sight() {
            if self.offered.insert(name.clone()) {
                tasks.push(Task::Offer(name));
            }
        }
    }

    /// What a launch with `profile` must review first: its enabled plugins
    /// without saved choices, but `skip`, by their manifest's state.
    pub fn unreviewed(&self, profile: &str, skip: &[String]) -> Unreviewed {
        let mut out = Unreviewed::default();
        for p in self.enabled_plugins(profile) {
            let name = p.name.clone();
            if skip.contains(&name) || self.plugins.choices.contains_key(&name) {
                continue;
            }
            match self.plugins.manifests.get(&name) {
                Some((sha, ManifestState::Ready(_))) => out.due.push((name, sha.clone())),
                Some((_, ManifestState::Failed(why))) => out.failed.push((name, why.clone())),
                Some((_, ManifestState::Fetching)) | None => out.fetching.push(name),
            }
        }
        out
    }

    /// Plugin `name`'s manifest is here: with choices saved and the plugin
    /// on, the ids not seen before are named once per hash. (Without
    /// choices it is in [`State::first_sight`].)
    fn on_manifest_ready(&mut self, name: &str, sha256: &str, tasks: &mut Vec<Task>) {
        let Some((_, ManifestState::Ready(m))) = self.plugins.manifests.get(name) else {
            return;
        };
        let Some(choices) = self.plugins.choices.get(name) else {
            return;
        };
        if !self.plugin_enabled(name) {
            return;
        }
        let new: Vec<String> = m
            .ids()
            .into_iter()
            .filter(|id| !choices.seen.contains(id))
            .collect();
        let text = format!(
            "{name} {} adds {}; Alt+i, Enter to review",
            m.version,
            new.join(", ")
        );
        let key = (name.to_owned(), sha256.to_owned());
        if !new.is_empty() && self.plugins.noticed.insert(key) {
            tasks.push(Task::Notice(text));
        }
    }

    /// The plugins whose setup is due: on, their manifest here, no choices
    /// saved, not put off in this run.
    pub fn first_sight(&self) -> Vec<String> {
        self.all_enabled()
            .into_iter()
            .map(|p| p.name.clone())
            .filter(|name| {
                matches!(
                    self.plugins.manifests.get(name),
                    Some((_, ManifestState::Ready(_)))
                ) && !self.plugins.choices.contains_key(name)
                    && !self.plugins.deferred.contains(name)
            })
            .collect()
    }

    /// The member saved `choices` for plugin `name`: written, and every
    /// host asked again.
    pub fn save_choices(&mut self, name: &str, choices: PluginChoices) -> Vec<Task> {
        self.plugins.deferred.remove(name);
        self.plugins
            .choices
            .insert(name.to_owned(), choices.clone());
        let mut tasks = vec![Task::SaveChoices {
            name: name.to_owned(),
            choices,
        }];
        self.request_again(name, &mut tasks);
        tasks.push(Task::Changed);
        tasks
    }

    /// The member put plugin `name`'s setup off for this run: nothing is
    /// asked for it until it is saved.
    pub fn defer(&mut self, name: &str) {
        self.plugins.deferred.insert(name.to_owned());
    }

    /// Plugin `name` turned on or off (`[extensions]`), for new sessions;
    /// on, the hosts are asked for it.
    pub fn set_enabled(&mut self, name: &str, on: bool) -> Vec<Task> {
        self.plugins.config.insert(name.to_owned(), on);
        let mut tasks = Vec::new();
        if on {
            let sha = self.plugins.manifests.get(name).map(|(s, _)| s.clone());
            if let Some(sha) = sha {
                self.on_manifest_ready(name, &sha, &mut tasks);
            }
            self.request_all(&mut tasks);
            self.offer_due(&mut tasks);
        }
        tasks.push(Task::Changed);
        tasks
    }

    /// `r`: every list and every manifest fetched again, and every host
    /// asked again for every enabled manifest.
    pub fn refresh(&mut self) -> Vec<Task> {
        let mut tasks = self.fetch_lists();
        let names: Vec<String> = self.all_enabled().iter().map(|p| p.name.clone()).collect();
        for name in names {
            self.request_again(&name, &mut tasks);
        }
        self.fetch_manifests(true, &mut tasks);
        tasks.push(Task::Changed);
        tasks
    }
}
