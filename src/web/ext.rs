//! The extensions registry, the external extension host, snippets and the
//! config.toml re-read, for the web. The registry is the TUI's
//! (`tui::extensions::Registry`): built-ins, external extensions (their
//! programs run by `ExtensionHost`) and the proxy plugins' rows.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use super::hub::HubHandle;
use super::plugins::{extensions_changed, notice_event, Plugins};
use crate::config::Config;
use crate::proto::{LensKind, SessionExtensions, SessionId};
use crate::tui::extensions::host::ExtensionHost;
use crate::tui::extensions::protocol::SessionContext;
use crate::tui::extensions::{manifest, ActionResult, Extension, Registry, Source};

/// How often config.toml's mtime is checked.
const CONFIG_POLL: Duration = Duration::from_secs(5);
/// The longest action input and render source, in bytes.
const INPUT_MAX: usize = 64 * 1024;
/// The longest snippet, in bytes.
const SNIPPET_MAX: usize = 64 * 1024;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// What config.toml's file looks like: its mtime and size.
type Stamp = Option<(SystemTime, u64)>;

fn stamp(path: &std::path::Path) -> Stamp {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

pub(crate) struct Extensions {
    registry: Arc<Mutex<Registry>>,
    host: Arc<ExtensionHost>,
    config_path: PathBuf,
    last: Mutex<Stamp>,
    /// The proxy profiles as last sent to the hub.
    profiles: Mutex<(Vec<String>, Option<String>)>,
    /// Held by a toggle (registry, then config.toml) and by a re-read, so a
    /// re-read never sees the registry ahead of the file.
    edit: tokio::sync::Mutex<()>,
}

impl Extensions {
    /// Registry::new(&config.extensions, config.claude.latex, external, &mut notices),
    /// with `manifest::load(&manifest::dir(), &mut notices)` in spawn_blocking;
    /// ExtensionHost::new; its notices become `notice` events.
    pub(crate) async fn load(config: &Config, hub: &HubHandle) -> Arc<Extensions> {
        let config_path = crate::paths::config_file();
        let path = config_path.clone();
        let (external, mut notices, last, profiles) = tokio::task::spawn_blocking(move || {
            let mut notices = Vec::new();
            let external = manifest::load(&manifest::dir(), &mut notices);
            let profiles = crate::proxy::resolve::load_proxy_profiles();
            (external, notices, stamp(&path), profiles)
        })
        .await
        .unwrap_or_default();
        let registry = Registry::new(
            &config.extensions,
            config.claude.latex,
            external,
            &mut notices,
        );
        for notice in &notices {
            tracing::warn!("web: {}", crate::claude::latex::sanitize(notice));
        }
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let host = ExtensionHost::new(&registry, tx);
        let notify = hub.clone();
        tokio::spawn(async move {
            while let Some(text) = rx.recv().await {
                notify.broadcast(notice_event(&text, "warn"));
            }
        });
        Arc::new(Extensions {
            registry: Arc::new(Mutex::new(registry)),
            host: Arc::new(host),
            config_path,
            last: Mutex::new(last),
            profiles: Mutex::new(profiles),
            edit: tokio::sync::Mutex::new(()),
        })
    }

    pub(crate) fn registry(&self) -> Arc<Mutex<Registry>> {
        Arc::clone(&self.registry)
    }

    pub(crate) fn session_extensions(&self) -> SessionExtensions {
        lock(&self.registry).session_extensions()
    }

    /// Extension DTOs, registry order.
    pub(crate) fn list(&self) -> Value {
        let registry = lock(&self.registry);
        let list: Vec<Value> = registry.extensions.iter().map(extension_dto).collect();
        json!({ "extensions": list })
    }

    pub(crate) async fn toggle(
        &self,
        plugins: &Plugins,
        hub: &HubHandle,
        name: &str,
        on: bool,
    ) -> Result<(), String> {
        let _edit = self.edit.lock().await;
        let source = {
            let mut registry = lock(&self.registry);
            let ext = registry
                .extensions
                .iter()
                .find(|e| e.name == name)
                .ok_or_else(|| "no such extension".to_owned())?;
            if let Source::Plugin {
                refused: Some(why), ..
            } = &ext.source
            {
                return Err(format!("refused: {}", crate::claude::latex::sanitize(why)));
            }
            let source = ext.source.clone();
            registry.set_enabled(name, on);
            source
        };
        match source {
            Source::External(_) => self.host.set_enabled(name, on).await,
            Source::Plugin { .. } => plugins.toggle(name, on),
            Source::BuiltIn => {}
        }
        let (path, owned) = (self.config_path.clone(), name.to_owned());
        let saved =
            tokio::task::spawn_blocking(move || crate::config::save_extension(&path, &owned, on))
                .await
                .map_err(|_| "cannot write config.toml".to_owned())?;
        hub.broadcast(extensions_changed());
        saved.map_err(|e| e.to_string())
    }

    pub(crate) async fn action(
        &self,
        hub: &HubHandle,
        ext: &str,
        action: &str,
        input: Option<String>,
        sid: Option<SessionId>,
    ) -> Result<Value, String> {
        {
            let registry = lock(&self.registry);
            let e = registry
                .extensions
                .iter()
                .find(|e| e.name == ext)
                .ok_or_else(|| "no such extension".to_owned())?;
            if !e.is_external() {
                return Err("not an external extension".to_owned());
            }
            if !e.enabled {
                return Err(format!("extension {ext} is turned off"));
            }
            if !e.actions.iter().any(|a| a.id == action) {
                return Err("no such action".to_owned());
            }
        }
        if input.as_ref().is_some_and(|i| i.len() > INPUT_MAX) {
            return Err("bad request: input: at most 64 KiB".to_owned());
        }
        let (host, session) = match sid {
            Some(sid) => {
                let s = hub.session(sid).await?;
                let ctx = SessionContext {
                    host: s.host.clone(),
                    cwd: s.info.cwd.clone(),
                    name: s.info.name.clone(),
                    conversation: s.info.claude_session_id.clone(),
                };
                (Some(s.host), Some(ctx))
            }
            None => (None, None),
        };
        tracing::info!(op = "extensions.action", ext, action, host = ?host, sid = ?sid, "web op");
        let result = self.host.action(ext, action, input, session).await?;
        Ok(json!({ "result": action_dto(&result) }))
    }

    pub(crate) async fn render(
        &self,
        lens: &str,
        kind: &str,
        source: &str,
    ) -> Result<Value, String> {
        let kind = match kind {
            "tag" => LensKind::Tag,
            "fence" => LensKind::Fence,
            _ => return Err("bad request: kind: must be \"tag\" or \"fence\"".to_owned()),
        };
        if source.len() > INPUT_MAX {
            return Err("bad request: source: at most 64 KiB".to_owned());
        }
        let render = self.host.render(lens, kind, source);
        let (inline, block) = match render.await {
            Some((inline, block)) => (inline, block.map(|laid| laid.rows)),
            None => (None, None),
        };
        Ok(json!({ "inline": inline, "block": block }))
    }

    /// Every 5 s: config.toml's mtime (spawn_blocking); on a change, `config::load`
    /// (spawn_blocking) and apply [extensions], [plugins.*], [claude] and the proxy
    /// profiles; then `extensions_changed` (and `profiles` if they changed).
    pub(crate) fn watch_config(self: &Arc<Self>, plugins: Plugins, hub: HubHandle) {
        let this = Arc::clone(self);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(CONFIG_POLL);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            tick.tick().await;
            loop {
                tick.tick().await;
                this.reread(&plugins, &hub).await;
            }
        });
    }

    /// Re-read config.toml if it changed since the last look.
    async fn reread(&self, plugins: &Plugins, hub: &HubHandle) {
        let _edit = self.edit.lock().await;
        let path = self.config_path.clone();
        let now = tokio::task::spawn_blocking(move || stamp(&path))
            .await
            .unwrap_or(None);
        {
            let mut last = lock(&self.last);
            if *last == now {
                return;
            }
            *last = now;
        }
        let path = self.config_path.clone();
        let Ok((config, profiles)) = tokio::task::spawn_blocking(move || {
            (
                crate::config::load_from(&path),
                crate::proxy::resolve::load_proxy_profiles(),
            )
        })
        .await
        else {
            return;
        };
        tracing::debug!("web: config.toml re-read");
        let before = lock(&self.registry).clone();
        // Built-ins and external extensions: on unless listed off.
        let flips: Vec<(String, bool)> = {
            let mut registry = lock(&self.registry);
            let mut flips = Vec::new();
            for ext in registry.extensions.iter_mut().filter(|e| !e.is_plugin()) {
                let mut on = config.extensions.get(&ext.name).copied().unwrap_or(true);
                if ext.name == "latex" && !config.claude.latex {
                    on = false;
                }
                if ext.enabled != on {
                    ext.enabled = on;
                    if ext.is_external() {
                        flips.push((ext.name.clone(), on));
                    }
                }
            }
            flips
        };
        for (name, on) in flips {
            self.host.set_enabled(&name, on).await;
        }
        plugins.set_config(config.extensions.clone(), config.plugins.clone());
        hub.set_claude_policy(config.claude.clone()).await;
        let changed = {
            let mut last = lock(&self.profiles);
            let changed = *last != profiles;
            *last = profiles.clone();
            changed
        };
        if changed {
            hub.set_profiles(profiles.0, profiles.1).await;
        }
        if *lock(&self.registry) != before {
            hub.broadcast(extensions_changed());
        }
    }

    /// ExtensionHost::shutdown
    pub(crate) async fn shutdown(&self) {
        self.host.shutdown().await;
    }
}

/// An Extension DTO (docs/web-protocol.md §4).
fn extension_dto(e: &Extension) -> Value {
    let (source, profile, refused) = match &e.source {
        Source::BuiltIn => ("builtin", None, None),
        Source::External(_) => ("external", None, None),
        Source::Plugin { profile, refused } => ("plugin", Some(profile.clone()), refused.clone()),
    };
    let lenses: Vec<Value> = e
        .lenses
        .iter()
        .map(|l| {
            let kind = match l.kind {
                LensKind::Fence => "fence",
                _ => "tag",
            };
            json!({"name": l.name, "kind": kind})
        })
        .collect();
    let actions: Vec<Value> = e
        .actions
        .iter()
        .map(|a| json!({"id": a.id, "title": a.title, "input": a.input}))
        .collect();
    let s = crate::claude::latex::sanitize;
    json!({
        "name": s(&e.name),
        "description": s(&e.description),
        "enabled": e.enabled,
        "source": source,
        "profile": profile.map(|p| s(&p)),
        "refused": refused.map(|r| s(&r)),
        "lenses": lenses,
        "actions": actions,
    })
}

/// An ActionResult DTO (docs/web-protocol.md §4); the host already capped
/// and sanitized it.
fn action_dto(r: &ActionResult) -> Value {
    match r {
        ActionResult::Document { title, markdown } => {
            json!({"kind": "document", "title": title, "markdown": markdown})
        }
        ActionResult::Notice { text } => json!({"kind": "notice", "text": text}),
        ActionResult::Paste { text } => json!({"kind": "paste", "text": text}),
        ActionResult::List { title, items } => {
            let items: Vec<Value> = items
                .iter()
                .map(|i| json!({"label": i.label, "detail": i.detail, "result": action_dto(&i.result)}))
                .collect();
            json!({"kind": "list", "title": title, "items": items})
        }
        ActionResult::Resume {
            host,
            cwd,
            conversation,
        } => json!({"kind": "resume", "host": host, "cwd": cwd, "conversation": conversation}),
    }
}

pub(crate) async fn snippets_list() -> Result<Value, String> {
    use crate::tui::snippets;
    let list = tokio::task::spawn_blocking(|| snippets::load(&snippets::dir()))
        .await
        .map_err(|_| "cannot read the snippets".to_owned())?
        .map_err(|e| format!("cannot read the snippets: {e}"))?;
    let list: Vec<Value> = list
        .into_iter()
        .map(|s| json!({"name": s.name, "text": s.text}))
        .collect();
    Ok(json!({ "snippets": list }))
}

/// ≤ 64 KiB
pub(crate) async fn snippets_save(text: String, name: Option<String>) -> Result<Value, String> {
    use crate::tui::snippets;
    if text.len() > SNIPPET_MAX {
        return Err("bad request: text: at most 64 KiB".to_owned());
    }
    if text.trim().is_empty() {
        return Err("bad request: text: empty".to_owned());
    }
    if name.as_deref().is_some_and(|n| n.trim().is_empty()) {
        return Err("bad request: name: empty".to_owned());
    }
    let name = tokio::task::spawn_blocking(move || match name {
        Some(n) => snippets::save_named(&snippets::dir(), &n, &text),
        None => snippets::save(&snippets::dir(), &text),
    })
    .await
    .map_err(|_| "cannot save the snippet".to_owned())?
    .map_err(|e| format!("cannot save the snippet: {e}"))?;
    Ok(json!({ "name": name }))
}

/// Delete the snippet `name` (as `snippets.list` returns it).
pub(crate) async fn snippets_delete(name: String) -> Result<Value, String> {
    use crate::tui::snippets;
    let r = tokio::task::spawn_blocking(move || snippets::delete(&snippets::dir(), &name))
        .await
        .map_err(|_| "cannot delete the snippet".to_owned())?;
    match r {
        Ok(()) => Ok(json!({})),
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::InvalidInput | std::io::ErrorKind::NotFound
            ) =>
        {
            Err(format!("bad request: name: {e}"))
        }
        Err(e) => Err(format!("cannot delete the snippet: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_dtos_have_the_documented_fields() {
        let registry = Registry::default();
        let latex = extension_dto(&registry.extensions[0]);
        let mut keys: Vec<&String> = latex.as_object().unwrap().keys().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "actions",
                "description",
                "enabled",
                "lenses",
                "name",
                "profile",
                "refused",
                "source"
            ]
        );
        assert_eq!(latex["source"], "builtin");
        assert_eq!(latex["lenses"][0], json!({"name": "latex", "kind": "tag"}));
        let search = extension_dto(&registry.extensions[2]);
        assert_eq!(search["actions"][0]["id"], "search");
    }

    #[test]
    fn action_results_nest() {
        let r = ActionResult::List {
            title: "t".into(),
            items: vec![crate::tui::extensions::ListItem {
                label: "a".into(),
                detail: None,
                result: ActionResult::Notice { text: "hi".into() },
            }],
        };
        let v = action_dto(&r);
        assert_eq!(v["kind"], "list");
        assert_eq!(
            v["items"][0]["result"],
            json!({"kind": "notice", "text": "hi"})
        );
    }
}
