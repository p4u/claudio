//! The Vocdoni addon's runner: it turns the runtime's events into [`State`]
//! transitions and does the [`Task`]s they return with the addon context:
//! the list and manifest fetches, the `vocdoni/apply` asks, the choices'
//! config edits, notices and `changed`. The state's lock is never held
//! across an await or a call into the context (A6).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use super::daemon::{serves_apply, Applied, OP_APPLY};
use super::plugins::choices;
use super::plugins::manifest::Manifest;
use super::plugins::state::PluginsFetch;
use super::proxy::api::{self, ApiError, ConfigResponse};
use super::proxy::plugins::{self as cache, PluginList};
use super::proxy::profile::Profile;
use super::proxy::resolve::{find_profile, load_proxy_profiles};
use super::state::{State, Task};
use crate::addon::{AddonCtx, AddonError, AddonEvent};

/// How long a spawn waits for a profile's proxy config.
const CONFIG_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// The state and the context its tasks run with.
pub struct Runner {
    state: Mutex<State>,
    ctx: OnceLock<Arc<dyn AddonCtx>>,
    /// `plugins.json`: the lists cached per profile.
    cache: PathBuf,
    /// Whether a front-end runs its plugins through this addon (core-addons
    /// plan P3.8-P3.10: the web keeps its own until P3.10); until then
    /// every event is ignored.
    active: AtomicBool,
}

impl Default for Runner {
    fn default() -> Self {
        Runner::with_cache(crate::paths::plugins_cache())
    }
}

impl Runner {
    /// A runner whose lists are cached in `cache`.
    pub fn with_cache(cache: PathBuf) -> Runner {
        Runner {
            state: Mutex::default(),
            ctx: OnceLock::new(),
            cache,
            active: AtomicBool::new(false),
        }
    }

    /// The state, locked (never across an await or a context call).
    pub fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// From now on the runtime's events drive the state.
    pub fn activate(&self) {
        self.active.store(true, Ordering::SeqCst);
    }

    /// Apply `f` at the current time, then do the tasks it returns.
    pub fn transition(self: &Arc<Self>, f: impl FnOnce(&mut State) -> Vec<Task>) {
        let tasks = {
            let mut state = self.state();
            state.now = crate::paths::unix_now();
            f(&mut state)
        };
        self.run(tasks);
    }

    /// A runtime event: the start loads, hosts and ticks are transitions,
    /// a config change re-reads the profiles and `[plugins]`.
    pub fn on_event(self: &Arc<Self>, event: &AddonEvent, ctx: &Arc<dyn AddonCtx>) {
        if !self.active.load(Ordering::SeqCst) {
            return;
        }
        let _ = self.ctx.set(Arc::clone(ctx));
        match event {
            AddonEvent::Started => ctx.spawn(Box::pin(Arc::clone(self).load())),
            AddonEvent::ConfigChanged => ctx.spawn(Box::pin(Arc::clone(self).reload())),
            AddonEvent::HostUp { host, features } => {
                self.transition(|s| s.host_up(host, serves_apply(features)))
            }
            AddonEvent::HostDown { host } => self.transition(|s| s.host_down(host)),
            AddonEvent::ClaudeInstalled { host } => self.transition(|s| s.claude_installed(host)),
            AddonEvent::Tick => {
                let now = crate::paths::unix_now();
                let tasks = self.state().tick(now);
                self.run(tasks);
            }
            _ => {}
        }
    }

    /// The start: the profiles and the cached lists (off the executor),
    /// `[plugins]`, and the `[extensions]` entries of the cached plugins.
    async fn load(self: Arc<Self>) {
        let Some(ctx) = self.ctx.get().cloned() else {
            return;
        };
        let path = self.cache.clone();
        let read = tokio::task::spawn_blocking(move || {
            (load_proxy_profiles(), cache::load_all_from(&path))
        })
        .await;
        let Ok((profiles, cached)) = read else {
            return;
        };
        let choices = choices::from_table(&ctx.config("plugins"));
        let extensions = extensions_of(&*ctx, &cached);
        self.transition(|s| s.load(profiles, choices, extensions, cached));
    }

    /// config.toml changed: the profiles and `[plugins]` again.
    async fn reload(self: Arc<Self>) {
        let Some(ctx) = self.ctx.get().cloned() else {
            return;
        };
        let Ok(profiles) = tokio::task::spawn_blocking(load_proxy_profiles).await else {
            return;
        };
        let choices = choices::from_table(&ctx.config("plugins"));
        self.transition(|s| s.reload(profiles, choices));
    }

    /// Do `tasks` (dropped while no front-end has started the addon).
    pub fn run(self: &Arc<Self>, tasks: Vec<Task>) {
        let Some(ctx) = self.ctx.get() else {
            return;
        };
        for task in tasks {
            match task {
                Task::Notice(text) => ctx.notify(text),
                Task::Changed => ctx.changed(),
                task => ctx.spawn(Box::pin(Arc::clone(self).task(Arc::clone(ctx), task))),
            }
        }
    }

    /// One task that does I/O, its outcome a transition.
    async fn task(self: Arc<Self>, ctx: Arc<dyn AddonCtx>, task: Task) {
        match task {
            Task::FetchList { profile } => {
                let (fetch, notice) = fetch_list(&profile, &self.cache).await;
                if let Some(text) = notice {
                    ctx.notify(text);
                }
                let extensions = match &fetch {
                    PluginsFetch::Listed(list) => extensions_of(&*ctx, std::slice::from_ref(list)),
                    _ => BTreeMap::new(),
                };
                self.transition(|s| {
                    s.known(extensions);
                    s.on_list(&profile, fetch)
                });
            }
            Task::FetchManifest { name, url, sha256 } => {
                let fetched = cache::fetch_manifest(&url, &sha256).await;
                let result = fetched.and_then(|text| Manifest::parse_named(&text, &name));
                self.transition(|s| s.on_manifest(&name, &sha256, result));
            }
            Task::Ask {
                host,
                name,
                sha256,
                body,
            } => {
                let body = serde_json::to_value(&body).unwrap_or_default();
                let result = ctx.request(&host, OP_APPLY, body).await.and_then(applied);
                self.transition(|s| s.on_applied(&host, &name, &sha256, result));
            }
            Task::SaveChoices { name, choices } => {
                let own = name.clone();
                let edit = Box::new(move |t: &mut toml::Table| choices::set(t, &own, &choices));
                match ctx.edit_config("plugins", edit).await {
                    Ok(()) => ctx.notify(format!("plugin {name} set up (saved)")),
                    Err(e) => ctx.notify(format!("could not save: {e}")),
                }
            }
            Task::Notice(text) => ctx.notify(text),
            Task::Changed => ctx.changed(),
        }
    }

    /// `profile`'s proxy config: cached (5 min), else fetched within 5 s
    /// and kept. `None` when it cannot be had.
    pub async fn config(self: &Arc<Self>, profile: &str) -> Option<ConfigResponse> {
        let now = crate::paths::unix_now();
        {
            let mut state = self.state();
            state.now = now;
            if let Some(config) = state.config_cached(profile) {
                return Some(config.clone());
            }
        }
        let fetched = tokio::time::timeout(CONFIG_WAIT, fetch_config(profile)).await;
        let config = fetched.ok().flatten();
        self.transition(|s| {
            s.on_config(profile, config.clone());
            Vec::new()
        });
        config
    }
}

/// The `Applied` of a reply body.
fn applied(body: serde_json::Value) -> Result<Applied, AddonError> {
    serde_json::from_value(body).map_err(|_| AddonError::Daemon("unexpected reply".to_owned()))
}

/// `[extensions]` as config.toml has it for the plugins of `lists`.
fn extensions_of(ctx: &dyn AddonCtx, lists: &[PluginList]) -> BTreeMap<String, bool> {
    lists
        .iter()
        .flat_map(|l| l.plugins.iter())
        .filter_map(|p| Some((p.name.clone(), ctx.extension_on(&p.name)?)))
        .collect()
}

/// `profile`'s proxy config, `None` on any error.
async fn fetch_config(profile: &str) -> Option<ConfigResponse> {
    let name = profile.to_owned();
    let found = tokio::task::spawn_blocking(move || find_profile(&name)).await;
    let p = found.ok().flatten()?;
    api::fetch_config(&p.url, &p.token).await.ok().flatten()
}

/// Fetch `profile`'s list and keep `cache` (plugins.json) in step, and a
/// notice when the cache could not be written.
async fn fetch_list(profile: &str, cache: &Path) -> (PluginsFetch, Option<String>) {
    let name = profile.to_owned();
    let found = tokio::task::spawn_blocking(move || find_profile(&name)).await;
    match found.ok().flatten() {
        Some(p) => fetch_list_with(profile, &p, cache).await,
        None => (PluginsFetch::Unreachable, None),
    }
}

/// Fetch `profile`'s list from `p`: a list is saved in `cache`, a 401/403
/// drops the profile's entry (its secrets), anything else leaves it.
async fn fetch_list_with(
    profile: &str,
    p: &Profile,
    cache: &Path,
) -> (PluginsFetch, Option<String>) {
    let path = cache.to_path_buf();
    let (fetch, write) = match cache::fetch(profile, p).await {
        Ok(list) => {
            let saved = list.clone();
            let write = tokio::task::spawn_blocking(move || cache::save_cache_to(&path, &saved));
            (PluginsFetch::Listed(list), Some(write))
        }
        Err(ApiError::Http(s)) if matches!(s.as_u16(), 401 | 403) => {
            let name = profile.to_owned();
            let write = tokio::task::spawn_blocking(move || cache::remove_cache_from(&path, &name));
            (PluginsFetch::SignedOut, Some(write))
        }
        Err(_) => (PluginsFetch::Unreachable, None),
    };
    let notice = match write {
        Some(write) => match write.await {
            Ok(Err(e)) => Some(format!("could not save plugins: {e}")),
            _ => None,
        },
        None => None,
    };
    (fetch, notice)
}

#[cfg(test)]
mod tests;
