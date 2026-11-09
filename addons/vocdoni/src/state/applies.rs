//! What the hosts are asked and answered for each plugin's manifest, and
//! what a spawn gets from them.

use super::{State, Task};
use crate::daemon::{Applied, ApplyRequest};
use crate::plugins::state::{
    reask_after, Apply, ApplyState, ManifestState, PluginPart, INSTALLING,
};
use crate::proxy::plugins::Plugin;
use claudio_core::addon::AddonError;

impl State {
    /// `host`'s daemon is connected: one without `addon:vocdoni` (older
    /// than addons) is never asked, and every plugin is `TooOld` there;
    /// another is asked for every enabled manifest. A new connection may be
    /// a new daemon: its earlier answers are dropped.
    pub fn host_up(&mut self, host: &str, has_addon: bool) -> Vec<Task> {
        if has_addon {
            self.plugins.old_hosts.remove(host);
        } else {
            self.plugins.old_hosts.insert(host.to_owned());
        }
        self.plugins.hosts.insert(host.to_owned());
        self.plugins.applies.retain(|(h, _), _| h != host);
        let mut tasks = Vec::new();
        self.request_manifests(host, None, &mut tasks);
        tasks.push(Task::Changed);
        tasks
    }

    /// `host`'s daemon is gone: its answers no longer hold.
    pub fn host_down(&mut self, host: &str) -> Vec<Task> {
        self.plugins.hosts.remove(host);
        self.plugins.old_hosts.remove(host);
        self.plugins.applies.retain(|(h, _), _| h != host);
        vec![Task::Changed]
    }

    /// Claude was just installed on `host`: it is asked again for every
    /// enabled manifest, answered or not, so the installs that failed
    /// without it run now. An answer stays until the new one comes.
    pub fn claude_installed(&mut self, host: &str) -> Vec<Task> {
        let mut tasks = Vec::new();
        if !self.plugins.hosts.contains(host) {
            return tasks;
        }
        let names: Vec<String> = self.all_enabled().iter().map(|p| p.name.clone()).collect();
        for name in names {
            self.request_manifests(host, Some(&name), &mut tasks);
        }
        tasks
    }

    /// Every connected host, for every enabled manifest it has not answered.
    pub(super) fn request_all(&mut self, tasks: &mut Vec<Task>) {
        let hosts: Vec<String> = self.plugins.hosts.iter().cloned().collect();
        for host in hosts {
            self.request_manifests(&host, None, tasks);
        }
    }

    /// Every connected host for plugin `name`'s manifest again, even where
    /// it answered (its choices changed).
    pub(super) fn request_again(&mut self, name: &str, tasks: &mut Vec<Task>) {
        let hosts: Vec<String> = self.plugins.hosts.iter().cloned().collect();
        for host in hosts {
            self.request_manifests(&host, Some(name), tasks);
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
    fn request_manifests(&mut self, host: &str, again: Option<&str>, tasks: &mut Vec<Task>) {
        let wanted: Vec<Plugin> = self.all_enabled().into_iter().cloned().collect();
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
            self.ask(host, &p, tasks);
        }
    }

    /// Ask `host` to apply `p`. A `Ready` answer at the same hash stays
    /// until the new one comes. A daemon older than addons is not asked:
    /// `TooOld` at once.
    fn ask(&mut self, host: &str, p: &Plugin, tasks: &mut Vec<Task>) {
        let key = (host.to_owned(), p.name.clone());
        let now = self.now;
        if self.plugins.old_hosts.contains(host) {
            let sha = p.bundle_sha256.clone();
            let state = ApplyState::TooOld;
            let apply = Apply {
                sha,
                state,
                asked: now,
            };
            self.plugins.applies.insert(key, apply);
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
        tasks.push(Task::Ask {
            host: host.to_owned(),
            name: p.name.clone(),
            sha256: p.bundle_sha256.clone(),
            body: self.apply_body(p),
        });
    }

    /// The `vocdoni/apply` body for `p`: the names of its proxy env (never
    /// the values) and the ids the member turned off.
    fn apply_body(&self, p: &Plugin) -> ApplyRequest {
        let disabled = self
            .plugins
            .choices
            .get(&p.name)
            .map(|c| c.disabled.clone())
            .unwrap_or_default();
        ApplyRequest {
            name: p.name.clone(),
            url: p.bundle_url.clone(),
            sha256: p.bundle_sha256.clone(),
            secrets: p.env.iter().map(|(k, _)| k.clone()).collect(),
            disabled,
        }
    }

    /// Ask again a host whose answer still lists an install under way
    /// (every `REASK_EVERY` s) or a failed one to retry (once it is due).
    /// The daemon's backoff, up to 5 min, paces a lasting failure.
    pub(super) fn reask_installing(&mut self, tasks: &mut Vec<Task>) {
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
                .all_enabled()
                .into_iter()
                .find(|p| p.name == name)
                .cloned();
            if let Some(p) = plugin.filter(|p| self.requestable(p)) {
                self.ask(&host, &p, tasks);
            }
        }
    }

    /// `host`'s answer for plugin `name`'s manifest at hash `sha256`; one
    /// for a hash since replaced, or a host since gone, is dropped. A
    /// daemon without the op (or the addon) is `TooOld`.
    pub fn on_applied(
        &mut self,
        host: &str,
        name: &str,
        sha256: &str,
        result: Result<Applied, AddonError>,
    ) -> Vec<Task> {
        let key = (host.to_owned(), name.to_owned());
        let Some(apply) = self.plugins.applies.get_mut(&key) else {
            return Vec::new();
        };
        if apply.sha != sha256 {
            return Vec::new();
        }
        apply.state = match result {
            Ok(applied) => ApplyState::Ready(Box::new(applied)),
            Err(AddonError::Missing(_)) => ApplyState::TooOld,
            Err(AddonError::Daemon(why)) if why.starts_with("unsupported op") => ApplyState::TooOld,
            Err(AddonError::Daemon(why)) => {
                let prefix = format!("manifest {name}: ");
                ApplyState::Failed(why.strip_prefix(&prefix).unwrap_or(&why).to_owned())
            }
            Err(e) => ApplyState::Failed(e.to_string()),
        };
        vec![Task::Changed]
    }

    /// The state of plugin `name`'s manifest on `host`, if asked.
    pub fn apply_state(&self, host: &str, name: &str) -> Option<&ApplyState> {
        let key = (host.to_owned(), name.to_owned());
        self.plugins.applies.get(&key).map(|a| &a.state)
    }

    /// What `profile`'s enabled plugins (but `skip`) bring to a spawn on
    /// `host`, in registry order, and the requests of those `host` has not
    /// answered at all (the spawn waits for them). A plugin whose manifest
    /// the client lacks, or whose host failed, brings its secrets only.
    pub fn plugin_parts(
        &self,
        host: &str,
        profile: &str,
        skip: &[String],
    ) -> (Vec<PluginPart>, Vec<(String, ApplyRequest)>) {
        let mut parts = Vec::new();
        let mut pending = Vec::new();
        for p in self.enabled_plugins(profile) {
            if skip.contains(&p.name) {
                continue;
            }
            let applied = match self.apply_state(host, &p.name) {
                Some(ApplyState::Ready(a)) => Some(Applied::clone(a)),
                Some(ApplyState::Pending) | None if self.requestable(p) => {
                    pending.push((p.name.clone(), self.apply_body(p)));
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

    /// The details' lines for every connected host asked for plugin `name`,
    /// `local` first.
    pub(super) fn host_lines(&self, name: &str) -> Vec<String> {
        let mut lines = Vec::new();
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
        lines
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
