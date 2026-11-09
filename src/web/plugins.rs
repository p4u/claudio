//! The web's v1 adapter of the Vocdoni addon (docs/web-protocol.md v1;
//! deleted with the v1 ops in P3.16): the `proxy.*` and `plugins.*` ops,
//! the v1 `proxy`/`first_party`/`skip_plugins` of a spawn as a launch
//! select, and the `plugin_status`, `profiles` and `extensions_changed`
//! events. Everything comes through the addon API: its rows and
//! checklists, and the tables of its v1-only panel (`vocdoni::v1`); the
//! web keeps no plugin state or proxy cache of its own (A5). The DTOs name
//! plugins, versions, ids and reasons, never an env value.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};

use super::ext::Extensions;
use super::hub::HubHandle;
use super::spawn::ProxyArg;
use super::ws::parse;
use super::AppState;
use crate::addon::{Block, Checklist, ChecklistAnswer, LaunchSelect, PanelRequest, Pick};
use crate::addons::vocdoni::daemon as apply;
use crate::addons::vocdoni::launch::FIRST_PARTY_FLAG;
use crate::addons::vocdoni::v1::{PANEL, PLUGINS, SKIPPED, STATUS};
use crate::addons::vocdoni::NAME;
use crate::app::addons::AddonRuntime;
use crate::app::extensions::{Registry, Source};
use crate::proto::SessionId;

/// The most disabled ids a save takes (the daemon's cap).
const MAX_DISABLED: usize = 256;
/// The v1 panel's bound (an addon panel's, D13).
const PANEL_WAIT: Duration = Duration::from_secs(10);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// `text` sanitized and cut at `max` characters.
fn clean(text: &str, max: usize) -> String {
    crate::text::sanitize(text).chars().take(max).collect()
}

/// A `notice` event (docs/web-protocol.md §6) for every tab.
pub(crate) fn notice_event(text: &str, level: &str) -> Value {
    json!({
        "t": "ev",
        "ev": "notice",
        "text": clean(text, 1000),
        "level": level,
        "host": null,
        "sid": null,
    })
}

/// The `extensions_changed` event.
pub(crate) fn extensions_changed() -> Value {
    json!({"t": "ev", "ev": "extensions_changed"})
}

/// The tables of the addon's v1 panel, cleaned (C2): each row a map of
/// its columns.
#[derive(Debug, Default)]
pub(crate) struct Tables {
    plugins: Vec<BTreeMap<&'static str, String>>,
    status: Vec<BTreeMap<&'static str, String>>,
    skipped: Vec<BTreeMap<&'static str, String>>,
}

/// The rows of `block` (a table) keyed by `cols`.
fn keyed(block: Option<&Block>, cols: &[&'static str]) -> Vec<BTreeMap<&'static str, String>> {
    let Some(Block::Table { rows, .. }) = block else {
        return Vec::new();
    };
    rows.iter()
        .map(|r| cols.iter().copied().zip(r.iter().cloned()).collect())
        .collect()
}

/// The addon's v1 panel, within 10 s, through the API.
pub(crate) async fn tables(rt: &AddonRuntime) -> Result<Tables, String> {
    let entry = rt.addons().get(NAME).cloned();
    let entry = entry.ok_or_else(|| "no plugins in this build".to_owned())?;
    let ui = entry
        .ui()
        .ok_or_else(|| "no plugins in this build".to_owned())?;
    let req = PanelRequest {
        id: PANEL.to_owned(),
        page: 0,
        window: None,
        sid: None,
    };
    let panel = tokio::time::timeout(PANEL_WAIT, ui.panel(req, rt.ctx(NAME)))
        .await
        .map_err(|_| "the plugins did not answer".to_owned())?;
    let panel = crate::addon::panel(panel);
    let page = |title: &str| panel.pages.iter().find(|p| p.title == title);
    let (plugins, status) = (page("plugins"), page("status"));
    if plugins.is_none() || status.is_none() {
        return Err("the plugins could not be listed".to_owned());
    }
    fn blocks(p: Option<&crate::addon::Page>, i: usize) -> Option<&Block> {
        p.and_then(|p| p.blocks.get(i))
    }
    Ok(Tables {
        plugins: keyed(blocks(plugins, 0), &PLUGINS),
        status: keyed(blocks(status, 0), &STATUS),
        skipped: keyed(blocks(status, 1), &SKIPPED),
    })
}

/// A column of a row, `""` when absent.
fn col<'a>(row: &'a BTreeMap<&'static str, String>, name: &str) -> &'a str {
    row.get(name).map_or("", String::as_str)
}

/// Words of a space-joined list.
fn words(s: &str) -> Vec<String> {
    s.split(' ')
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

/// `""` as null.
fn or_null(s: &str) -> Value {
    if s.is_empty() {
        Value::Null
    } else {
        Value::String(s.to_owned())
    }
}

/// Every `plugin_status` entry (the event minus `t`/`ev`), by `(host,
/// plugin)`: ids, reasons and dropped flags only.
pub(crate) fn statuses(t: &Tables) -> BTreeMap<(String, String), Value> {
    let mut out = BTreeMap::new();
    for row in &t.status {
        let (host, name) = (col(row, "host"), col(row, "name"));
        let skipped: Vec<Value> = t
            .skipped
            .iter()
            .filter(|k| col(k, "host") == host && col(k, "name") == name)
            .map(|k| {
                let reason = col(k, "reason");
                json!({"id": col(k, "id"), "reason": reason, "retry_secs": apply::retry_secs(reason)})
            })
            .collect();
        let dto = json!({
            "host": host,
            "name": name,
            "state": col(row, "state"),
            "installed": words(col(row, "installed")),
            "present": words(col(row, "present")),
            "skipped": skipped,
            "dropped": words(col(row, "dropped")),
            "error": or_null(col(row, "error")),
        });
        out.insert((host.to_owned(), name.to_owned()), dto);
    }
    out
}

/// The `plugins.items` reply for checklist `c` (plugin `version`).
pub(crate) fn items_dto(c: &Checklist, version: &str) -> Value {
    let items: Vec<Value> = c
        .items
        .iter()
        .map(|i| {
            json!({
                "id": i.id,
                "section": i.section.clone().unwrap_or_default(),
                "title": i.label,
                "checked": i.on,
            })
        })
        .collect();
    json!({"name": c.id, "version": version, "items": items})
}

/// The `plugins.list` reply: every plugin of every list (refused ones too),
/// with its name, version and profile, never its env or bundle URL.
pub(crate) fn list_dto(t: &Tables, registry: &Registry) -> Value {
    let on = |name: &str| {
        registry.extensions.iter().any(|e| {
            e.name == name
                && e.enabled
                && matches!(&e.source, Source::Addon { addon, refused: None, .. } if *addon == NAME)
        })
    };
    let out: Vec<Value> = t
        .plugins
        .iter()
        .map(|p| {
            let name = col(p, "name");
            json!({
                "name": name,
                "version": col(p, "version"),
                "profile": col(p, "profile"),
                "enabled": col(p, "refused").is_empty() && on(name),
                "refused": or_null(col(p, "refused")),
                "choices_saved": col(p, "choices_saved") == "true",
                "manifest": col(p, "manifest"),
                "manifest_error": or_null(col(p, "manifest_error")),
                "description": col(p, "description"),
            })
        })
        .collect();
    json!({ "plugins": out })
}

/// The v1 `proxy` and `first_party` of a spawn or respawn as a launch
/// select: `default` the addons' default, `direct`, or a proxy profile.
pub(crate) fn select_of(arg: &ProxyArg, first_party: bool) -> LaunchSelect {
    let pick = match arg {
        ProxyArg::Default => Pick::Default,
        ProxyArg::Direct => Pick::Direct,
        ProxyArg::Profile { name } => Pick::Option {
            addon: NAME.to_owned(),
            option: name.clone(),
        },
    };
    let flags = first_party.then(|| FIRST_PARTY_FLAG.to_owned());
    LaunchSelect {
        pick,
        flags: flags.into_iter().collect(),
    }
}

/// A spawn's v1 `skip_plugins` as its launch's deferred checklists.
pub(crate) fn deferred(skip: &[String]) -> Vec<(String, String)> {
    skip.iter().map(|n| (NAME.to_owned(), n.clone())).collect()
}

/// The v1 `profiles` (names and default): the Vocdoni launch options.
pub(crate) fn profiles(rt: &AddonRuntime) -> (Vec<String>, Option<String>) {
    let Some(launch) = rt.addons().get(NAME).and_then(|a| a.launch()) else {
        return (Vec::new(), None);
    };
    let options = launch.options(rt.ctx(NAME).as_ref());
    let options: Vec<_> = options.into_iter().map(crate::addon::option).collect();
    let default = options.iter().find(|o| o.default).map(|o| o.id.clone());
    (options.into_iter().map(|o| o.id).collect(), default)
}

/// What the page was last told of the addon's plugins and profiles; on a
/// `Changed` of the addon, what differs is sent.
pub(crate) struct Feed {
    hub: HubHandle,
    ext: Arc<Extensions>,
    runtime: AddonRuntime,
    /// Each `(host, plugin)`'s last `plugin_status`.
    applies: BTreeMap<(String, String), Value>,
    profiles: (Vec<String>, Option<String>),
}

impl Feed {
    /// Starting from `profiles`, as the hub's snapshot has them.
    pub(crate) fn new(
        hub: HubHandle,
        ext: Arc<Extensions>,
        runtime: AddonRuntime,
        profiles: (Vec<String>, Option<String>),
    ) -> Feed {
        Feed {
            hub,
            ext,
            runtime,
            applies: BTreeMap::new(),
            profiles,
        }
    }

    /// An addon's data changed: the registry's addon rows (then
    /// `extensions_changed`), every changed `plugin_status`, the `profiles`.
    pub(crate) async fn changed(&mut self) {
        if self.ext.set_rows(&self.runtime.extension_rows()) {
            self.hub.broadcast(extensions_changed());
        }
        if let Ok(t) = tables(&self.runtime).await {
            let now = statuses(&t);
            for (key, dto) in &now {
                if self.applies.get(key) != Some(dto) {
                    let mut v = dto.clone();
                    v["t"] = "ev".into();
                    v["ev"] = "plugin_status".into();
                    self.hub.broadcast(v);
                }
            }
            self.applies = now;
        }
        let profiles = profiles(&self.runtime);
        if profiles != self.profiles {
            self.profiles = profiles.clone();
            self.hub.set_profiles(profiles.0, profiles.1).await;
        }
    }

    /// config.toml was re-read: the registry follows it.
    pub(crate) async fn config_changed(&self) {
        self.ext.reread(&self.hub).await;
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoArgs {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SidArgs {
    sid: SessionId,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StatsArgs {
    profile: String,
    window: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NameArgs {
    name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PluginSaveArgs {
    name: String,
    disabled: Vec<String>,
}

/// The v1 `proxy.*` and `plugins.*` ops (docs/web-protocol.md §5).
pub(crate) async fn dispatch_v1(state: &AppState, op: &str, args: Value) -> Result<Value, String> {
    let rt = &state.addons;
    match op {
        "proxy.stats" => {
            let a: StatsArgs = parse(args)?;
            super::proxy::stats(&a.profile, &a.window).await
        }
        "proxy.credential" => {
            let a: SidArgs = parse(args)?;
            let (profile, conversation) = session_proxy(&state.hub, a.sid).await?;
            super::proxy::credential(&profile, &conversation).await
        }
        "proxy.switch" => {
            let a: SidArgs = parse(args)?;
            let (profile, conversation) = session_proxy(&state.hub, a.sid).await?;
            super::proxy::switch(&profile, &conversation).await
        }
        "plugins.list" => {
            parse::<NoArgs>(args)?;
            let t = tables(rt).await?;
            let registry = state.ctx.ext.registry();
            let registry = lock(&registry);
            Ok(list_dto(&t, &registry))
        }
        "plugins.items" => {
            let a: NameArgs = parse(args)?;
            let (c, version) = checklist(rt, &a.name).await?;
            Ok(items_dto(&c, &version))
        }
        "plugins.save" => {
            let a: PluginSaveArgs = parse(args)?;
            save(rt, &a.name, a.disabled).await.map(|()| json!({}))
        }
        "plugins.status" => {
            parse::<NoArgs>(args)?;
            let status: Vec<Value> = statuses(&tables(rt).await?).into_values().collect();
            Ok(json!({ "status": status }))
        }
        _ => Err("unknown op".to_owned()),
    }
}

/// Plugin `name`'s checklist (its manifest's items, cleaned) and version,
/// or why there is none yet.
async fn checklist(rt: &AddonRuntime, name: &str) -> Result<(Checklist, String), String> {
    let t = tables(rt).await?;
    let row = t.plugins.iter().find(|p| col(p, "name") == name);
    let row = row.ok_or_else(|| "no such plugin".to_owned())?;
    let entry = rt.addons().get(NAME).cloned();
    let found = entry
        .as_ref()
        .and_then(|e| e.ui())
        .and_then(|ui| ui.checklist(name, rt.ctx(NAME).as_ref()));
    match (found, col(row, "manifest")) {
        (Some(c), _) => Ok((crate::addon::checklist(c), col(row, "version").to_owned())),
        (None, "failed") => Err(format!("manifest: {}", col(row, "manifest_error"))),
        (None, _) => Err("the manifest is still being fetched".to_owned()),
    }
}

/// The proxy profile and conversation of session `sid` (server state), for
/// `proxy.credential` / `proxy.switch`.
async fn session_proxy(hub: &HubHandle, sid: SessionId) -> Result<(String, String), String> {
    let s = hub.session(sid).await?;
    let profile = s
        .proxy
        .ok_or_else(|| "no proxy for this session".to_owned())?;
    let conversation = s
        .info
        .claude_session_id
        .ok_or_else(|| "no conversation yet".to_owned())?;
    Ok((profile, conversation))
}

/// `plugins.save`: the ids checked against the manifest's items, the
/// choices saved by the addon (awaited: `[plugins.<name>]`), every host
/// asked again.
async fn save(rt: &AddonRuntime, name: &str, disabled: Vec<String>) -> Result<(), String> {
    let (c, _) = checklist(rt, name).await?;
    if disabled.len() > MAX_DISABLED {
        return Err(format!("bad request: disabled: at most {MAX_DISABLED} ids"));
    }
    if let Some(bad) = disabled
        .iter()
        .find(|d| !c.items.iter().any(|i| &i.id == *d))
    {
        return Err(format!(
            "bad request: disabled: no item {:?}",
            clean(bad, 80)
        ));
    }
    let entry = rt.addons().get(NAME).cloned();
    let ui = entry.as_ref().and_then(|e| e.ui());
    let ui = ui.ok_or_else(|| "no plugins in this build".to_owned())?;
    let answer = ChecklistAnswer::Save {
        revision: c.revision,
        off: disabled,
    };
    let saving = ui.save_checklist(name.to_owned(), answer, rt.ctx(NAME));
    tokio::time::timeout(Duration::from_secs(15), saving)
        .await
        .map_err(|_| "the save timed out".to_owned())??;
    tracing::info!(op = "plugins.save", plugin = name, "web op");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addon::{ChecklistItem, ExtensionRow, Page, Panel};

    fn table(cols: &[&str], rows: &[&[&str]]) -> Block {
        Block::Table {
            header: cols.iter().map(|c| (*c).to_owned()).collect(),
            rows: rows
                .iter()
                .map(|r| r.iter().map(|c| (*c).to_owned()).collect())
                .collect(),
        }
    }

    fn sample_tables() -> Tables {
        let plugins = table(
            &PLUGINS,
            &[&["kb", "1.4.0", "env", "", "ready", "", "true", "KB"]],
        );
        let status = table(
            &STATUS,
            &[&["local", "kb", "ready", "plugin:a@m", "", "--agents", ""]],
        );
        let skipped = table(
            &SKIPPED,
            &[
                &["local", "kb", "plugin:b@m", "down (retrying in 15 s)"],
                &["local", "kb", "plugin:c@m", "installing"],
            ],
        );
        let panel = Panel {
            title: PANEL.into(),
            pages: vec![
                Page {
                    title: "plugins".into(),
                    blocks: vec![plugins],
                },
                Page {
                    title: "status".into(),
                    blocks: vec![status, skipped],
                },
            ],
            actions: Vec::new(),
            updated: None,
        };
        let page = |i: usize| panel.pages[i].blocks.clone();
        Tables {
            plugins: keyed(page(0).first(), &PLUGINS),
            status: keyed(page(1).first(), &STATUS),
            skipped: keyed(page(1).get(1), &SKIPPED),
        }
    }

    #[test]
    fn statuses_carry_ids_and_retry_seconds() {
        let all = statuses(&sample_tables());
        let v = &all[&("local".to_owned(), "kb".to_owned())];
        assert_eq!(v["state"], "ready");
        assert_eq!(v["skipped"][0]["retry_secs"], 15);
        assert!(v["skipped"][1]["retry_secs"].is_null());
        assert_eq!(v["installed"], json!(["plugin:a@m"]));
        assert_eq!(v["present"], json!([]));
        assert_eq!(v["dropped"], json!(["--agents"]));
        assert!(v["error"].is_null());
    }

    #[test]
    fn list_and_items_rebuild_the_v1_shapes() {
        let mut registry = Registry::default();
        let row = ExtensionRow {
            name: "kb".into(),
            description: "1.4.0 · env".into(),
            default_on: true,
            refused: None,
            details: Vec::new(),
            setup: Some("kb".into()),
        };
        registry.set_addon_rows(&[(NAME, row)], &BTreeMap::new());
        let v = list_dto(&sample_tables(), &registry);
        let row = &v["plugins"][0];
        assert_eq!(row["name"], "kb");
        assert_eq!(row["profile"], "env");
        assert_eq!(row["enabled"], true);
        assert!(row["refused"].is_null());
        assert_eq!(row["choices_saved"], true);
        assert_eq!(row["manifest"], "ready");
        assert!(row["manifest_error"].is_null());
        assert_eq!(row["description"], "KB");
        let c = Checklist {
            id: "kb".into(),
            revision: "a".repeat(64),
            title: "Set up kb 1.4.0".into(),
            intro: String::new(),
            items: vec![ChecklistItem {
                id: "mcp:kb".into(),
                label: "kb".into(),
                detail: None,
                section: Some("MCP servers".into()),
                on: false,
            }],
        };
        let v = items_dto(&c, "1.4.0");
        assert_eq!(
            (v["name"].as_str(), v["version"].as_str()),
            (Some("kb"), Some("1.4.0"))
        );
        assert_eq!(
            v["items"][0],
            json!({"id": "mcp:kb", "section": "MCP servers", "title": "kb", "checked": false})
        );
    }

    #[test]
    fn the_v1_proxy_arg_is_a_launch_select() {
        let work = ProxyArg::Profile {
            name: "work".into(),
        };
        let select = select_of(&work, true);
        assert_eq!(
            select.pick,
            Pick::Option {
                addon: NAME.into(),
                option: "work".into()
            }
        );
        assert_eq!(select.flags, ["first-party"]);
        assert_eq!(
            select_of(&ProxyArg::Default, false),
            LaunchSelect::default()
        );
        assert_eq!(select_of(&ProxyArg::Direct, false), LaunchSelect::of(None));
        assert_eq!(
            deferred(&["kb".to_owned()]),
            [(NAME.to_owned(), "kb".to_owned())]
        );
    }
}
