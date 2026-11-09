//! The Vocdoni addon's runner: it turns the runtime's events into [`State`]
//! transitions and does the [`Task`]s they return with the addon context:
//! the list and manifest fetches, the `vocdoni/apply` asks, the choices'
//! config edits, notices and `changed`. The state's lock is never held
//! across an await or a call into the context (A6).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use super::daemon::{serves_apply, Applied, OP_APPLY};
use super::plugins::choices::{self, PluginChoices};
use super::plugins::manifest::Manifest;
use super::plugins::state::PluginsFetch;
use super::profiles::{ProfileSource, Profiles};
use super::proxy::api::{self, ApiError, ConfigResponse};
use super::proxy::plugins::{self as cache, PluginList};
use super::proxy::profile::Profile;
use super::state::stats::Focused;
use super::state::{State, Task};
use claudio_core::addon::{AddonCtx, AddonError, AddonEvent};

/// Why a launch fails when the profiles were not read yet.
const NOT_READ: &str = "the proxy profiles are not read yet: try again";

/// How long a spawn waits for a profile's proxy config.
const CONFIG_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// The state and the context its tasks run with.
pub struct Runner {
    state: Mutex<State>,
    ctx: OnceLock<Arc<dyn AddonCtx>>,
    /// `plugins.json`: the lists cached per profile.
    cache: PathBuf,
    /// Where the proxy profiles are read.
    source: ProfileSource,
    /// Whether a front-end's runtime runs this addon: set by its first
    /// event or its first launch (a launch may come before the event).
    active: AtomicBool,
}

impl Default for Runner {
    fn default() -> Self {
        Runner::with_cache(claudio_core::paths::plugins_cache())
    }
}

impl Runner {
    /// A runner whose lists are cached in `cache`.
    pub fn with_cache(cache: PathBuf) -> Runner {
        Runner {
            state: Mutex::default(),
            ctx: OnceLock::new(),
            cache,
            source: ProfileSource::Config,
            active: AtomicBool::new(false),
        }
    }

    /// A runner whose profiles are `profiles`' `[proxy]` alone (tests).
    #[cfg(test)]
    pub fn with_files(cache: PathBuf, profiles: PathBuf) -> Runner {
        Runner {
            source: ProfileSource::File(profiles),
            ..Runner::with_cache(cache)
        }
    }

    /// Whether a front-end runs its plugins through this addon.
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::SeqCst)
    }

    /// The profiles as read at the addon's start (or since), never read
    /// here (D3): before that read, none and an error, which fails a launch
    /// that needs one (a launch waits for the start; never direct).
    pub fn profiles(&self) -> Profiles {
        self.state().profiles().unwrap_or_else(|| Profiles {
            error: Some(NOT_READ.to_owned()),
            ..Profiles::default()
        })
    }

    /// Read the profiles now (blocking) and keep them.
    pub fn read_profiles(&self) -> Profiles {
        let read = self.source.read();
        self.state().set_profiles(read.clone());
        read
    }

    /// Profile `name`, read off the executor (`None`: it does not exist,
    /// or the profiles cannot be read).
    pub async fn find_profile(&self, name: &str) -> Option<Profile> {
        self.lookup_profile(name).await.ok().flatten()
    }

    /// Profile `name`, read off the executor: `Err` when the profiles
    /// cannot be read, `Ok(None)` when it does not exist.
    pub async fn lookup_profile(&self, name: &str) -> Result<Option<Profile>, String> {
        let (source, name) = (self.source.clone(), name.to_owned());
        let found = tokio::task::spawn_blocking(move || source.find(&name)).await;
        found.map_err(|e| format!("profile lookup: {e}"))?
    }

    /// The state, locked (never across an await or a context call).
    pub fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A runtime runs this addon (its first event or launch; tests).
    pub fn activate(&self) {
        self.active.store(true, Ordering::SeqCst);
    }

    /// Apply `f` at the current time, then do the tasks it returns.
    pub fn transition(self: &Arc<Self>, f: impl FnOnce(&mut State) -> Vec<Task>) {
        let tasks = {
            let mut state = self.state();
            state.now = claudio_core::paths::unix_now();
            f(&mut state)
        };
        self.run(tasks);
    }

    /// A runtime event: the start loads, hosts and ticks are transitions,
    /// a config change re-reads the profiles and `[plugins]`. Every event
    /// takes the names the core's extensions hold (a plugin of such a name
    /// is refused), and a tick or a session's change the profiles the
    /// sessions use (their lists are fetched).
    pub fn on_event(self: &Arc<Self>, event: &AddonEvent, ctx: &Arc<dyn AddonCtx>) {
        self.activate();
        let _ = self.ctx.set(Arc::clone(ctx));
        self.take_names(ctx.as_ref());
        if matches!(
            event,
            AddonEvent::Tick | AddonEvent::SessionAdded { .. } | AddonEvent::SessionChanged { .. }
        ) {
            let used = used_profiles(ctx.as_ref());
            self.transition(|s| s.use_profiles(used));
        }
        match event {
            AddonEvent::Started => ctx.spawn(Box::pin(Arc::clone(self).load())),
            AddonEvent::ConfigChanged => ctx.spawn(Box::pin(Arc::clone(self).reload())),
            AddonEvent::HostUp { host, features } => {
                self.transition(|s| s.host_up(host, serves_apply(features)))
            }
            AddonEvent::HostDown { host } => self.transition(|s| s.host_down(host)),
            AddonEvent::ClaudeInstalled { host } => self.transition(|s| s.claude_installed(host)),
            AddonEvent::Tick => {
                let now = claudio_core::paths::unix_now();
                let focused = focused(ctx.as_ref());
                let tasks = {
                    let mut state = self.state();
                    let mut tasks = state.tick(now);
                    tasks.extend(state.creds_due(focused));
                    tasks
                };
                self.run(tasks);
            }
            AddonEvent::SessionRemoved { sid } => self.state().forget_session(*sid),
            _ => {}
        }
    }

    /// The names the core's own extensions hold; a change re-reads the rows.
    fn take_names(&self, ctx: &dyn AddonCtx) {
        let names: BTreeSet<String> = ctx.core_extensions().into_iter().collect();
        let moved = {
            let mut state = self.state();
            let moved = state.taken != names;
            state.taken = names;
            moved && state.loaded
        };
        if moved {
            ctx.changed();
        }
    }

    /// The start: the profiles and the cached lists (off the executor),
    /// `[plugins]`, and the `[extensions]` entries of the cached plugins.
    async fn load(self: Arc<Self>) {
        let Some(ctx) = self.ctx.get().cloned() else {
            return;
        };
        let (path, source) = (self.cache.clone(), self.source.clone());
        let read =
            tokio::task::spawn_blocking(move || (source.read(), cache::load_all_from(&path))).await;
        let Ok((profiles, cached)) = read else {
            return;
        };
        let choices = choices::from_table(&ctx.config("plugins"));
        let extensions = extensions_of(&*ctx, &cached);
        self.transition(|s| {
            s.first_party = profiles.first_party;
            s.profile_error = profiles.error;
            s.load(
                (profiles.names, profiles.default),
                choices,
                extensions,
                cached,
            )
        });
    }

    /// config.toml changed: the profiles and `[plugins]` again.
    async fn reload(self: Arc<Self>) {
        let Some(ctx) = self.ctx.get().cloned() else {
            return;
        };
        let source = self.source.clone();
        let Ok(profiles) = tokio::task::spawn_blocking(move || source.read()).await else {
            return;
        };
        let choices = choices::from_table(&ctx.config("plugins"));
        self.transition(|s| {
            s.first_party = profiles.first_party;
            s.profile_error = profiles.error;
            s.reload((profiles.names, profiles.default), choices)
        });
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
                Task::Offer(name) => ctx.offer_checklist(name),
                task => ctx.spawn(Box::pin(Arc::clone(self).task(Arc::clone(ctx), task))),
            }
        }
    }

    /// One task that does I/O, its outcome a transition.
    async fn task(self: Arc<Self>, ctx: Arc<dyn AddonCtx>, task: Task) {
        match task {
            Task::FetchList { profile } => {
                let found = self.find_profile(&profile).await;
                let (fetch, notice) = match found {
                    Some(p) => fetch_list_with(&profile, &p, &self.cache).await,
                    None => (PluginsFetch::Unreachable, None),
                };
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
                let result = match request_body(&body) {
                    Ok(body) => ctx.request(&host, OP_APPLY, body).await.and_then(applied),
                    Err(e) => Err(e),
                };
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
            Task::Offer(name) => ctx.offer_checklist(name),
            Task::FetchCred {
                sid,
                profile,
                conversation,
            } => {
                let result = match self.find_profile(&profile).await {
                    Some(p) => api::fetch_session(&p.url, &p.token, &conversation)
                        .await
                        .map_err(super::proxy::stats::describe),
                    None => Err(super::profiles::missing(&profile)),
                };
                self.transition(|s| s.on_cred(sid, &conversation, result));
            }
        }
    }

    /// The member saved `choices` for plugin `name`: written to
    /// `[plugins.<name>]` first (awaited), then the state's transition
    /// (every host asked again).
    pub async fn save(self: &Arc<Self>, name: &str, choices: PluginChoices) -> Result<(), String> {
        let ctx = self.ctx.get().cloned();
        let ctx = ctx.ok_or_else(|| "the plugins have not started yet".to_owned())?;
        let (own, saved) = (name.to_owned(), choices.clone());
        let edit = Box::new(move |t: &mut toml::Table| choices::set(t, &own, &saved));
        ctx.edit_config("plugins", edit).await?;
        self.transition(|s| {
            let tasks = s.save_choices(name, choices);
            let written = |t: &Task| matches!(t, Task::SaveChoices { .. });
            tasks.into_iter().filter(|t| !written(t)).collect()
        });
        Ok(())
    }

    /// `profile`'s proxy config: cached (5 min), else fetched within 5 s
    /// and kept. `None` when it cannot be had.
    pub async fn config(self: &Arc<Self>, profile: &str) -> Option<ConfigResponse> {
        let now = claudio_core::paths::unix_now();
        {
            let mut state = self.state();
            state.now = now;
            if let Some(config) = state.config_cached(profile) {
                return Some(config.clone());
            }
        }
        let fetched = tokio::time::timeout(CONFIG_WAIT, self.fetch_config(profile)).await;
        let config = fetched.ok().flatten();
        self.transition(|s| {
            s.on_config(profile, config.clone());
            Vec::new()
        });
        config
    }

    /// `profile`'s proxy config, `None` on any error.
    async fn fetch_config(&self, profile: &str) -> Option<ConfigResponse> {
        let p = self.find_profile(profile).await?;
        api::fetch_config(&p.url, &p.token).await.ok().flatten()
    }
}

/// The focused claude sessions this addon routes, with a conversation:
/// the ones whose credential is looked up.
fn focused(ctx: &dyn AddonCtx) -> Vec<Focused> {
    let sessions = ctx.sessions().into_iter();
    let claude =
        sessions.filter(|s| s.focused && s.kind == claudio_core::proto::SessionKind::Claude);
    claude
        .filter_map(|s| {
            let conversation = s.conversation.clone().filter(|c| !c.is_empty())?;
            Some(Focused {
                sid: s.sid,
                profile: super::panel::profile_of(&s)?,
                conversation,
            })
        })
        .collect()
}

/// The profiles the sessions this addon routes use.
fn used_profiles(ctx: &dyn AddonCtx) -> BTreeSet<String> {
    let sessions = ctx.sessions().into_iter();
    sessions
        .filter_map(|s| s.launch)
        .filter(|c| c.addon == super::NAME)
        .map(|c| c.option)
        .collect()
}

/// `body` as a request's JSON: one that does not serialise is a failed
/// answer for the state, never a `null` body the daemon refuses (D5).
pub(super) fn request_body<T: serde::Serialize>(body: &T) -> Result<serde_json::Value, AddonError> {
    serde_json::to_value(body).map_err(|_| AddonError::Daemon("bad request body".to_owned()))
}

/// The `Applied` of a reply body.
pub(super) fn applied(body: serde_json::Value) -> Result<Applied, AddonError> {
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
