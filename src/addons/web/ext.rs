//! The extensions registry, the external extension host, snippets and the
//! config.toml re-read, for the web. The registry is the TUI's
//! (`app::extensions::Registry`): built-ins, external extensions (their
//! programs run by `ExtensionHost`) and the addons' rows. config.toml is
//! watched by the addon runtime, which says when it was re-read.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use serde::Deserialize;
use serde_json::{json, Value};

use super::events::{extensions_changed, notice_event};
use super::hub::HubHandle;
use super::ws::parse;
use crate::addon::ExtensionRow;
use crate::app::addons::AddonRuntime;
use crate::app::extensions::host::ExtensionHost;
use crate::app::extensions::protocol::SessionContext;
use crate::app::extensions::{core_choices, manifest, ActionResult, Extension, Registry, Source};
use crate::config::Config;
use crate::proto::{LensKind, SessionExtensions, SessionId};

/// The longest action input and render source, in bytes.
const INPUT_MAX: usize = 64 * 1024;
/// The longest snippet, in bytes.
const SNIPPET_MAX: usize = 64 * 1024;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

pub(crate) struct Extensions {
    registry: Arc<Mutex<Registry>>,
    host: Arc<ExtensionHost>,
    config_path: PathBuf,
    /// `[extensions]` as config.toml had it at start, with the toggles
    /// since: the addon rows' on/off.
    choices: Mutex<BTreeMap<String, bool>>,
}

impl Extensions {
    /// Registry::new(core_choices(&config.extensions, &external), external, &mut notices),
    /// with `manifest::load(&manifest::dir(), &mut notices)` in spawn_blocking;
    /// ExtensionHost::new; its notices become `notice` events.
    pub(crate) async fn load(config: &Config, hub: &HubHandle) -> Arc<Extensions> {
        let config_path = crate::paths::config_file();
        let (external, mut notices) = tokio::task::spawn_blocking(move || {
            let mut notices = Vec::new();
            let external = manifest::load(&manifest::dir(), &mut notices);
            (external, notices)
        })
        .await
        .unwrap_or_default();
        let choices = core_choices(&config.extensions, &external);
        let registry = Registry::new(&choices, external, &mut notices);
        for notice in &notices {
            tracing::warn!("web: {}", crate::text::sanitize(notice));
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
            choices: Mutex::new(config.extensions.clone()),
        })
    }

    /// The names the registry's own extensions hold (built-in, external).
    pub(crate) fn own_names(&self) -> Vec<String> {
        let registry = lock(&self.registry);
        let own = registry.extensions.iter().filter(|e| !e.is_addon());
        own.map(|e| e.name.clone()).collect()
    }

    /// The addons' rows now; whether the registry changed.
    pub(crate) fn set_rows(&self, rows: &[(&'static str, ExtensionRow)]) -> bool {
        let choices = lock(&self.choices).clone();
        let mut registry = lock(&self.registry);
        let before = registry.clone();
        registry.set_addon_rows(rows, &choices);
        *registry != before
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

    /// Turn extension `name` on or off: the registry at once, the host for
    /// an external one, `[extensions]` in config.toml (an addon's row: the
    /// runtime writes it, then tells the addon, D10).
    pub(crate) async fn toggle(
        &self,
        addons: &AddonRuntime,
        hub: &HubHandle,
        name: &str,
        on: bool,
    ) -> Result<(), String> {
        let source = {
            let mut registry = lock(&self.registry);
            let ext = registry
                .extensions
                .iter()
                .find(|e| e.name == name)
                .ok_or_else(|| "no such extension".to_owned())?;
            if let Source::Addon {
                refused: Some(why), ..
            } = &ext.source
            {
                return Err(format!("refused: {}", crate::text::sanitize(why)));
            }
            let source = ext.source.clone();
            registry.set_enabled(name, on);
            source
        };
        lock(&self.choices).insert(name.to_owned(), on);
        let saved = match source {
            Source::Addon { addon, .. } => {
                let told = addons.toggle_row(addon, name.to_owned(), on).await;
                told.map(|notice| {
                    if let Some(n) = notice {
                        hub.broadcast(notice_event(&n.text, "info"));
                    }
                })
            }
            other => {
                if let Source::External(_) = other {
                    self.host.set_enabled(name, on).await;
                }
                let (path, owned) = (self.config_path.clone(), name.to_owned());
                let saved = tokio::task::spawn_blocking(move || {
                    crate::config::save_extension(&path, &owned, on)
                })
                .await
                .map_err(|_| "cannot write config.toml".to_owned())?;
                saved.map_err(|e| e.to_string())
            }
        };
        hub.broadcast(extensions_changed());
        saved
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

    /// config.toml was re-read by the addon runtime: `config::load`
    /// (spawn_blocking), `[extensions]` applied to the built-ins and
    /// external extensions and, with `rows` (the addons' rows as the
    /// runtime has them now), to the addons' rows; then `extensions_changed`.
    pub(crate) async fn reread(&self, hub: &HubHandle, rows: &[(&'static str, ExtensionRow)]) {
        let path = self.config_path.clone();
        let Ok(config) = tokio::task::spawn_blocking(move || crate::config::load_from(&path)).await
        else {
            return;
        };
        tracing::debug!("web: config.toml re-read");
        let before = lock(&self.registry).clone();
        // Built-ins and external extensions: on unless listed off.
        let flips: Vec<(String, bool)> = {
            let mut registry = lock(&self.registry);
            let mut flips = Vec::new();
            for ext in registry.extensions.iter_mut().filter(|e| !e.is_addon()) {
                let on = config.extensions.get(&ext.name).copied().unwrap_or(true);
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
        *lock(&self.choices) = config.extensions.clone();
        self.set_rows(rows);
        hub.set_claude_policy(config.claude.clone()).await;
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
    let (source, addon, refused) = match &e.source {
        Source::BuiltIn => ("builtin", None, None),
        Source::External(_) => ("external", None, None),
        Source::Addon { addon, refused, .. } => ("addon", Some(*addon), refused.clone()),
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
    let s = crate::text::sanitize;
    json!({
        "name": s(&e.name),
        "description": s(&e.description),
        "enabled": e.enabled,
        "source": source,
        "addon": addon,
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SnippetArgs {
    text: String,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SnippetDeleteArgs {
    name: String,
}

/// One `snippets.*` op (docs/web-protocol.md §5).
pub(crate) async fn snippets(op: &str, args: Value) -> Result<Value, String> {
    match op {
        "snippets.list" => {
            parse::<NoArgs>(args)?;
            snippets_list().await
        }
        "snippets.save" => {
            let a: SnippetArgs = parse(args)?;
            snippets_save(a.text, a.name).await
        }
        "snippets.delete" => {
            let a: SnippetDeleteArgs = parse(args)?;
            snippets_delete(a.name).await
        }
        _ => Err("unknown op".to_owned()),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoArgs {}

pub(crate) async fn snippets_list() -> Result<Value, String> {
    use crate::app::snippets;
    let list = snippets::load_all()
        .await
        .1
        .map_err(|e| format!("cannot read the snippets: {e}"))?;
    let list: Vec<Value> = list
        .into_iter()
        .map(|s| json!({"name": s.name, "text": s.text}))
        .collect();
    Ok(json!({ "snippets": list }))
}

/// ≤ 64 KiB
pub(crate) async fn snippets_save(text: String, name: Option<String>) -> Result<Value, String> {
    use crate::app::snippets;
    if text.len() > SNIPPET_MAX {
        return Err("bad request: text: at most 64 KiB".to_owned());
    }
    if text.trim().is_empty() {
        return Err("bad request: text: empty".to_owned());
    }
    if name.as_deref().is_some_and(|n| n.trim().is_empty()) {
        return Err("bad request: name: empty".to_owned());
    }
    let saved = match name {
        Some(n) => snippets::save_text_named(n, text).await,
        None => snippets::save_text(text).await,
    };
    let name = saved.map_err(|e| format!("cannot save the snippet: {e}"))?;
    Ok(json!({ "name": name }))
}

/// Delete the snippet `name` (as `snippets.list` returns it).
pub(crate) async fn snippets_delete(name: String) -> Result<Value, String> {
    use crate::app::snippets;
    match snippets::delete_named(name).await {
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
        let search = extension_dto(&registry.extensions[0]);
        let mut keys: Vec<&String> = search.as_object().unwrap().keys().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "actions",
                "addon",
                "description",
                "enabled",
                "lenses",
                "name",
                "refused",
                "source"
            ]
        );
        assert_eq!(search["source"], "builtin");
        assert_eq!(search["addon"], Value::Null);
        assert_eq!(search["lenses"], json!([]));
        assert_eq!(search["actions"][0]["id"], "search");
        let mut keys = Registry::default();
        keys.extensions.push(Extension {
            lenses: vec![crate::app::extensions::Lens {
                name: "keys".into(),
                kind: LensKind::Tag,
                instruction: String::new(),
            }],
            ..registry.extensions[0].clone()
        });
        let keys = extension_dto(&keys.extensions[2]);
        assert_eq!(keys["lenses"][0], json!({"name": "keys", "kind": "tag"}));
    }

    #[test]
    fn action_results_nest() {
        let r = ActionResult::List {
            title: "t".into(),
            items: vec![crate::app::extensions::ListItem {
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
