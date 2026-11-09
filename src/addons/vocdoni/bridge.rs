//! The TUI's view of the addon's plugin state, for the old setup screen
//! and extension rows (temporary: core-addons plan P3.8-P3.13, the one
//! core-to-addon import; checklists (P3.11) and extension rows (P3.13)
//! replace it). Queries read the state; calls are transitions whose tasks
//! the runner does.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use super::plugins::choices::PluginChoices;
use super::plugins::state::ManifestState;
use super::proxy::plugins::PluginList;
use super::runner::Runner;
use super::{Vocdoni, NAME};
use crate::addon::Addons;

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
        let state = self.0.state();
        state.plugins.manifests.get(name).map(|(_, m)| m.clone())
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
