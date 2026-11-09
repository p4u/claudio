//! The TUI's view of the addon's plugin state, for the old setup screen
//! and extension rows (temporary: core-addons plan P3.8-P3.13, the one
//! core-to-addon import; checklists (P3.11) and extension rows (P3.13)
//! replace it). Queries read the state; calls are transitions whose tasks
//! the runner does.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use super::plugins::choices::PluginChoices;
use super::plugins::manifest::Manifest;
use super::plugins::state::{Apply, ManifestState};
use super::proxy::plugins::PluginList;
use super::runner::Runner;
use super::{Vocdoni, NAME};
use crate::addon::Addons;

/// The addon whose checklists the extensions screen's plugin rows open.
pub const ADDON: &str = NAME;

/// A handle on the registered Vocdoni addon's runner.
#[derive(Clone)]
pub struct Bridge(pub(super) Arc<Runner>);

impl Bridge {
    /// The registered Vocdoni addon's, if this build registers it.
    pub fn of(addons: &Addons) -> Option<Bridge> {
        let vocdoni = addons.get(NAME)?.as_any().downcast_ref::<Vocdoni>()?;
        Some(Bridge(Arc::clone(&vocdoni.runner)))
    }

    /// The front-end runs its plugins through the addon from now on: the
    /// runtime's events drive it (until then they are ignored).
    pub fn activate(&self) {
        self.0.activate();
    }

    /// The lists, for the registry's plugin rows.
    pub fn lists(&self) -> Vec<PluginList> {
        self.0.state().lists()
    }

    /// `[extensions]` as the addon keeps it (toggles included), for the
    /// registry's plugin rows.
    pub fn enabled(&self) -> BTreeMap<String, bool> {
        self.0.state().plugins.config.clone()
    }

    /// Plugin `name`'s manifest, as fetched.
    pub fn manifest(&self, name: &str) -> Option<ManifestState> {
        self.manifest_at(name).map(|(_, m)| m)
    }

    /// Plugin `name`'s manifest, as fetched, and the hash it was fetched at.
    pub fn manifest_at(&self, name: &str) -> Option<(String, ManifestState)> {
        self.0.state().plugins.manifests.get(name).cloned()
    }

    /// Plugin `name`'s manifest, once fetched, and its saved choices.
    pub fn items(&self, name: &str) -> Result<(Manifest, Option<PluginChoices>), String> {
        let state = self.0.state();
        let m = match state.plugins.manifests.get(name) {
            Some((_, ManifestState::Ready(m))) => Manifest::clone(m),
            Some((_, ManifestState::Fetching)) => {
                return Err("the manifest is still being fetched".to_owned())
            }
            Some((_, ManifestState::Failed(why))) => {
                let why: String = crate::text::sanitize(why).chars().take(300).collect();
                return Err(format!("manifest: {why}"));
            }
            None => return Err("no such plugin".to_owned()),
        };
        Ok((m, state.plugins.choices.get(name).cloned()))
    }

    /// What each connected host answered for each plugin, by
    /// `(host, plugin)` in order.
    pub fn apply_states(&self) -> Vec<(String, String, Apply)> {
        let state = self.0.state();
        let mut all: Vec<(String, String, Apply)> = state
            .plugins
            .applies
            .iter()
            .map(|((host, name), a)| (host.clone(), name.clone(), a.clone()))
            .collect();
        all.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
        all
    }

    /// Plugin `name`'s saved choices.
    pub fn choices(&self, name: &str) -> Option<PluginChoices> {
        self.0.state().plugins.choices.get(name).cloned()
    }

    /// The extensions screen's lines for plugin `name` of `profile`, at
    /// `now`.
    pub fn plugin_details(
        &self,
        name: &str,
        profile: &str,
        refused: Option<&str>,
        now: u64,
    ) -> Vec<String> {
        let mut state = self.0.state();
        state.now = now;
        state.plugin_details(name, profile, refused)
    }

    /// The plugins whose first-sight setup is due.
    pub fn first_sight(&self) -> Vec<String> {
        self.0.state().first_sight()
    }

    /// The member saved `choices` for plugin `name`.
    pub fn save_choices(&self, name: &str, choices: PluginChoices) {
        self.0.transition(|s| s.save_choices(name, choices));
    }

    /// The member saved `choices` for plugin `name`: written to
    /// `[plugins.<name>]` (awaited: `Err` says why not), then every host is
    /// asked again.
    pub async fn save(&self, name: &str, choices: PluginChoices) -> Result<(), String> {
        self.0.save(name, choices).await
    }

    /// The member put plugin `name`'s first-sight setup off for this run.
    pub fn defer(&self, name: &str) {
        self.0.transition(|s| {
            s.defer(name);
            Vec::new()
        });
    }

    /// Plugin `name` turned on or off.
    pub fn set_enabled(&self, name: &str, on: bool) {
        self.0.transition(|s| s.set_enabled(name, on));
    }

    /// `r` on the extensions screen.
    pub fn refresh(&self) {
        self.0.transition(|s| s.refresh());
    }

    /// The names the front-end's own extensions hold.
    pub fn set_taken(&self, names: BTreeSet<String>) {
        self.0.state().taken = names;
    }

    /// The launch options sessions use now, as `(addon, option)`: this
    /// addon's are its profiles, whose lists are fetched.
    pub fn use_launches(&self, used: &[(String, String)]) {
        let profiles = used
            .iter()
            .filter(|(addon, _)| addon == NAME)
            .map(|(_, profile)| profile.clone())
            .collect();
        self.0.transition(|s| s.use_profiles(profiles));
    }

    /// Read the proxy profiles now (blocking: at a front-end's start), so
    /// launches route by them from the first one on; their names.
    pub fn read_profiles(&self) -> Vec<String> {
        self.0.read_profiles().names
    }

    /// The state itself (tests set it up).
    #[cfg(test)]
    pub fn state(&self) -> std::sync::MutexGuard<'_, super::state::State> {
        self.0.state()
    }
}
