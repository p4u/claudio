//! The web's v1 adapter of the Vocdoni addon (docs/web-protocol.md v1;
//! deleted with the v1 ops in P3.16): the `proxy.*` and `plugins.*` ops,
//! the v1 `proxy`/`first_party`/`skip_plugins` of a spawn as a launch
//! select, and the `plugin_status`, `profiles` and `extensions_changed`
//! events. Everything is read from the addon's state through its bridge
//! (`addons::vocdoni::bridge`): the web keeps no plugin state or proxy
//! cache of its own (A5). The DTOs name plugins, versions, ids and
//! reasons, never an env value.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use serde::Deserialize;
use serde_json::{json, Value};

use super::hub::HubHandle;
use super::spawn::ProxyArg;
use super::ws::parse;
use super::AppState;
use crate::addon::{LaunchSelect, Pick};
use crate::addons::vocdoni::bridge::Bridge;
use crate::addons::vocdoni::daemon as apply;
use crate::addons::vocdoni::launch::FIRST_PARTY_FLAG;
use crate::addons::vocdoni::plugins::choices::{choices_for, PluginChoices};
use crate::addons::vocdoni::plugins::manifest::Manifest;
use crate::addons::vocdoni::plugins::state::{Apply, ApplyState, ManifestState};
use crate::addons::vocdoni::proxy::plugins::PluginList;
use crate::addons::vocdoni::NAME;
use crate::app::addons::AddonRuntime;
use crate::app::extensions::{Registry, Source};
use crate::proto::SessionId;

/// The most disabled ids a save takes (the daemon's cap).
const MAX_DISABLED: usize = 256;

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

/// One `plugin_status` entry (the event minus `t`/`ev`): what `host`
/// answered for plugin `name`. Of an `Applied`, only the ids, reasons and
/// dropped flags.
pub(crate) fn status_dto(host: &str, name: &str, apply: &Apply) -> Value {
    let (state, applied, error) = match &apply.state {
        ApplyState::Pending => ("pending", None, None),
        ApplyState::Ready(a) => ("ready", Some(a.as_ref()), None),
        ApplyState::Failed(why) => ("failed", None, Some(clean(why, 300))),
        ApplyState::TooOld => ("too_old", None, None),
    };
    let ids = |list: &[String]| -> Vec<String> { list.iter().map(|i| clean(i, 300)).collect() };
    let skipped: Vec<Value> = applied
        .map(|a| {
            a.skipped
                .iter()
                .map(|(id, reason)| {
                    json!({
                        "id": clean(id, 300),
                        "reason": clean(reason, 300),
                        "retry_secs": apply::retry_secs(reason),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    json!({
        "host": host,
        "name": name,
        "state": state,
        "installed": applied.map(|a| ids(&a.installed)).unwrap_or_default(),
        "present": applied.map(|a| ids(&a.present)).unwrap_or_default(),
        "skipped": skipped,
        "dropped": applied.map(|a| ids(&a.dropped)).unwrap_or_default(),
        "error": error,
    })
}

/// The `plugins.items` reply for manifest `m`: every item, checked unless
/// the saved choices turn it off (all checked without choices).
pub(crate) fn items_dto(m: &Manifest, choices: Option<&PluginChoices>) -> Value {
    let items: Vec<Value> = m
        .items()
        .into_iter()
        .map(|i| {
            let checked = !choices.is_some_and(|c| c.disabled.contains(&i.id));
            json!({
                "id": clean(&i.id, 300),
                "section": i.section.heading(),
                "title": clean(&i.title, 300),
                "checked": checked,
            })
        })
        .collect();
    json!({
        "name": clean(&m.name, 80),
        "version": clean(&m.version, 80),
        "items": items,
    })
}

/// The `plugins.list` reply: every plugin of every list (refused ones too),
/// with its name, version and profile, never its env or bundle URL.
pub(crate) fn list_dto(bridge: &Bridge, registry: &Registry) -> Value {
    let mut out = Vec::new();
    for list in bridge.lists() {
        let profile = &list.profile;
        for (p, refused) in list.checked() {
            let enabled = registry.extensions.iter().any(|e| {
                e.name == p.name
                    && e.enabled
                    && matches!(&e.source, Source::Plugin { profile: q, refused: None } if q == profile)
            });
            let (manifest, error, description) = match (&refused, bridge.manifest_at(&p.name)) {
                (Some(why), _) => ("failed", Some(clean(why, 300)), String::new()),
                (None, Some((sha, ManifestState::Ready(m)))) if sha == p.bundle_sha256 => {
                    ("ready", None, clean(&m.description, 1000))
                }
                (None, Some((_, ManifestState::Failed(why)))) => {
                    ("failed", Some(clean(&why, 300)), String::new())
                }
                (None, _) => ("fetching", None, String::new()),
            };
            out.push(json!({
                "name": clean(&p.name, 80),
                "version": clean(&p.version, 80),
                "profile": clean(profile, 80),
                "enabled": enabled,
                "refused": refused.map(|r| clean(&r, 300)),
                "choices_saved": bridge.choices(&p.name).is_some(),
                "manifest": manifest,
                "manifest_error": error,
                "description": description,
            }));
        }
    }
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
    registry: Arc<Mutex<Registry>>,
    runtime: AddonRuntime,
    /// Each `(host, plugin)`'s last `plugin_status`.
    applies: BTreeMap<(String, String), Value>,
    profiles: (Vec<String>, Option<String>),
}

impl Feed {
    /// Starting from `profiles`, as the hub's snapshot has them.
    pub(crate) fn new(
        hub: HubHandle,
        registry: Arc<Mutex<Registry>>,
        runtime: AddonRuntime,
        profiles: (Vec<String>, Option<String>),
    ) -> Feed {
        Feed {
            hub,
            registry,
            runtime,
            applies: BTreeMap::new(),
            profiles,
        }
    }

    /// The addon's state changed: the registry's plugin rows (then
    /// `extensions_changed`), every changed `plugin_status`, the `profiles`.
    pub(crate) async fn changed(&mut self) {
        let Some(bridge) = Bridge::of(self.runtime.addons()) else {
            return;
        };
        let lists = bridge.lists();
        let lists: Vec<&PluginList> = lists.iter().collect();
        let rows = {
            let mut registry = lock(&self.registry);
            let before = registry.clone();
            registry.set_plugins(&lists, &bridge.enabled());
            *registry != before
        };
        if rows {
            self.hub.broadcast(extensions_changed());
        }
        let now: BTreeMap<(String, String), Value> = bridge
            .apply_states()
            .into_iter()
            .map(|(host, name, a)| {
                let dto = status_dto(&host, &name, &a);
                ((host, name), dto)
            })
            .collect();
        for (key, dto) in &now {
            if self.applies.get(key) != Some(dto) {
                let mut v = dto.clone();
                v["t"] = "ev".into();
                v["ev"] = "plugin_status".into();
                self.hub.broadcast(v);
            }
        }
        self.applies = now;
        let profiles = profiles(&self.runtime);
        if profiles != self.profiles {
            self.profiles = profiles.clone();
            self.hub.set_profiles(profiles.0, profiles.1).await;
        }
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
    let bridge =
        || Bridge::of(state.addons.addons()).ok_or_else(|| "no plugins in this build".to_owned());
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
            let registry = state.ctx.ext.registry();
            let registry = lock(&registry);
            Ok(list_dto(&bridge()?, &registry))
        }
        "plugins.items" => {
            let a: NameArgs = parse(args)?;
            let (m, choices) = bridge()?.items(&a.name)?;
            Ok(items_dto(&m, choices.as_ref()))
        }
        "plugins.save" => {
            let a: PluginSaveArgs = parse(args)?;
            save(&bridge()?, &a.name, a.disabled)
                .await
                .map(|()| json!({}))
        }
        "plugins.status" => {
            parse::<NoArgs>(args)?;
            let states = bridge()?.apply_states();
            let status: Vec<Value> = states.iter().map(|(h, n, a)| status_dto(h, n, a)).collect();
            Ok(json!({ "status": status }))
        }
        _ => Err("unknown op".to_owned()),
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

/// `plugins.save`: the ids checked against the manifest, the choices
/// written to `[plugins.<name>]` by the addon (awaited), every host asked
/// again.
async fn save(bridge: &Bridge, name: &str, disabled: Vec<String>) -> Result<(), String> {
    let (m, earlier) = bridge.items(name)?;
    if disabled.len() > MAX_DISABLED {
        return Err(format!("bad request: disabled: at most {MAX_DISABLED} ids"));
    }
    let ids = m.ids();
    if let Some(bad) = disabled.iter().find(|d| !ids.contains(d)) {
        return Err(format!(
            "bad request: disabled: no item {:?}",
            clean(bad, 80)
        ));
    }
    let choices = choices_for(&m, &disabled, earlier.as_ref());
    bridge.save(name, choices).await?;
    tracing::info!(op = "plugins.save", plugin = name, "web op");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::vocdoni::daemon::Applied;
    use crate::addons::vocdoni::plugins::state::PluginsFetch;

    const MANIFEST: &str = r#"format = 1
name = "vocdoni-test"
version = "1.0.0"
description = "Test setup"
instructions = "Manifest instructions for the test."

[[mcp]]
name = "kb"
type = "http"
url = "https://kb.example.org/mcp"
headers = { Authorization = "Bearer ${VOCDONI_TEST_TOKEN}" }
secrets = ["VOCDONI_TEST_TOKEN"]

[[agent]]
name = "reviewer"
description = "Reviews"
prompt = "Review the change."
"#;

    #[test]
    fn items_mark_disabled_ids_unchecked() {
        let m = Manifest::parse(MANIFEST).unwrap();
        let choices = PluginChoices {
            disabled: vec!["mcp:kb".into()],
            ..Default::default()
        };
        let v = items_dto(&m, Some(&choices));
        assert_eq!(v["name"], "vocdoni-test");
        assert_eq!(v["version"], "1.0.0");
        let items = v["items"].as_array().unwrap();
        assert!(items.len() >= 3, "{v}");
        for item in items {
            assert_eq!(item["checked"], item["id"] != "mcp:kb", "{item}");
        }
        let kb = items.iter().find(|i| i["id"] == "mcp:kb").unwrap();
        assert_eq!(kb["section"], "MCP servers");
        let v = items_dto(&m, None);
        assert!(v["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["checked"] == true));
    }

    #[test]
    fn status_dto_carries_retry_seconds() {
        let applied = Applied {
            name: "team".into(),
            version: "1.0.0".into(),
            mcp_config: None,
            agents: None,
            instructions: String::new(),
            env: vec![("REGION".into(), "eu-secret-ish".into())],
            installed: vec!["plugin:a@m".into()],
            present: vec![],
            skipped: vec![
                ("plugin:b@m".into(), "down (retrying in 15 s)".into()),
                ("plugin:c@m".into(), "installing".into()),
            ],
            dropped: vec!["--agents".into()],
        };
        let apply = Apply {
            sha: "a".repeat(64),
            state: ApplyState::Ready(Box::new(applied)),
            asked: 0,
        };
        let v = status_dto("local", "team", &apply);
        assert_eq!(v["state"], "ready");
        assert_eq!(v["skipped"][0]["retry_secs"], 15);
        assert!(v["skipped"][1]["retry_secs"].is_null());
        assert_eq!(v["installed"], json!(["plugin:a@m"]));
        assert_eq!(v["dropped"], json!(["--agents"]));
        assert!(v["error"].is_null());
        assert!(!v.to_string().contains("eu-secret-ish"), "no env value");
        let failed = Apply {
            state: ApplyState::Failed("boom".into()),
            ..apply.clone()
        };
        let v = status_dto("local", "team", &failed);
        assert_eq!(
            (v["state"].as_str(), v["error"].as_str()),
            (Some("failed"), Some("boom"))
        );
        let old = Apply {
            state: ApplyState::TooOld,
            ..apply
        };
        assert_eq!(status_dto("h", "team", &old)["state"], "too_old");
    }

    #[test]
    fn list_dto_has_no_env() {
        let mut p = crate::addons::vocdoni::proxy::plugins::sample("vocdoni-test");
        p.env = vec![("VOCDONI_TEST_TOKEN".into(), "plugin-SECRET-value".into())];
        let list = PluginList {
            profile: "env".into(),
            fetched_at: 0,
            plugins: vec![p.clone()],
        };
        let addons = crate::addon::Addons::new(crate::addons::all()).unwrap();
        let bridge = Bridge::of(&addons).unwrap();
        bridge
            .state()
            .on_list("env", PluginsFetch::Listed(list.clone()));
        let mut registry = Registry::default();
        registry.set_plugins(&[&list], &BTreeMap::new());
        let v = list_dto(&bridge, &registry);
        let text = v.to_string();
        assert!(!text.contains("plugin-SECRET-value"), "{text}");
        assert!(!text.contains(&p.bundle_url), "{text}");
        let row = &v["plugins"][0];
        assert_eq!(row["name"], "vocdoni-test");
        assert_eq!(row["profile"], "env");
        assert_eq!(row["enabled"], true);
        assert_eq!(row["choices_saved"], false);
        assert_eq!(row["manifest"], "fetching");
        let m = Manifest::parse(MANIFEST).unwrap();
        let mut state = bridge.state();
        state
            .plugins
            .choices
            .insert("vocdoni-test".into(), PluginChoices::default());
        state.on_manifest("vocdoni-test", &p.bundle_sha256, Ok(m));
        drop(state);
        let v = list_dto(&bridge, &registry);
        assert_eq!(v["plugins"][0]["choices_saved"], true);
        assert_eq!(v["plugins"][0]["manifest"], "ready");
        assert_eq!(v["plugins"][0]["description"], "Test setup");
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
