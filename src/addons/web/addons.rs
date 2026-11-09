//! The `addons.*` ops (docs/web-protocol.md v2 §5, "Addons") and what the
//! hub shows of the addons in a Session DTO: its launch and its segments.
//! Everything an addon hands the page goes through `crate::addon::clean`
//! here (C2); a session-bound request names only the session, whose
//! launch and conversation the addons read from the server's own state
//! (the runtime's snapshots), never from the page.

use std::collections::HashMap;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};

use super::hub::SessionView;
use super::ws::parse;
use super::AppState;
use crate::addon::{
    self, error_panel, line, ActionRequest, Addons, FlagArg, LaunchChoice, PanelDecl, PanelRequest,
    Segment, SessionSnap, Ui, TEXT_BYTES,
};
use crate::addon::{ChecklistAnswer, LaunchSelect, Pick, RowAction};
use crate::app::addons::AddonRuntime;
use crate::app::extensions::Source;
use crate::proto::SessionId;

/// An addon's panel (D13).
const PANEL_WAIT: Duration = Duration::from_secs(10);
/// An addon's panel action, checklist save or row action (D13).
const ACTION_WAIT: Duration = Duration::from_secs(15);
/// The most item ids a checklist save turns off, and their length.
const OFF_MAX: usize = 256;
const OFF_BYTES: usize = 128;
/// The most checklists a spawn defers, and launch flags it gives.
const DEFER_MAX: usize = 16;
const FLAGS_MAX: usize = 16;

/// What the hub shows of the addons for one session.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct SessionAddons {
    /// Whether its launch is known (web-state.json or the TUI's state.json
    /// know it, and an addon of this build adopts it).
    pub known: bool,
    /// Its launch's choice (`None`: direct, or unknown).
    pub choice: Option<LaunchChoice>,
    /// Its addons' segments, cleaned, by priority.
    pub segments: Vec<(&'static str, Segment)>,
}

/// A session's recorded launch as `(known, choice)`. Unrecorded, or
/// recorded but not one this build can make: unknown (never direct,
/// R19/F4, AC8).
pub(crate) fn launch_of(
    addons: &Addons,
    meta: Option<&crate::app::state::SavedRoute>,
) -> (bool, Option<LaunchChoice>) {
    match meta.map(|route| route.resolve(addons)) {
        Some(Ok(choice)) => (true, choice),
        None | Some(Err(_)) => (false, None),
    }
}

/// The Session DTO's `launch`.
pub(crate) fn launch_dto(a: &SessionAddons) -> Value {
    json!({"known": a.known, "choice": a.choice})
}

/// The Session DTO's `segments`.
pub(crate) fn segments_dto(a: &SessionAddons) -> Value {
    let all: Vec<Value> = a
        .segments
        .iter()
        .map(|(addon, s)| {
            json!({"addon": addon, "id": s.id, "text": s.text, "tone": s.tone,
                "priority": s.priority})
        })
        .collect();
    Value::Array(all)
}

/// Every session's launch and segments again, from `snaps` (the runtime's
/// view of them): the sessions whose fields changed.
pub(crate) fn refresh(
    sessions: &mut HashMap<SessionId, SessionView>,
    snaps: &[SessionSnap],
    rt: &AddonRuntime,
) -> Vec<SessionId> {
    let mut changed = Vec::new();
    for (sid, view) in sessions.iter_mut() {
        let (known, choice) = launch_of(rt.addons(), view.meta.as_ref());
        let segments = snaps
            .iter()
            .find(|s| s.sid == *sid)
            .map(|s| segments_of(rt, s))
            .unwrap_or_default();
        let now = SessionAddons {
            known,
            choice,
            segments,
        };
        if view.addon != now {
            view.addon = now;
            changed.push(*sid);
        }
    }
    changed
}

/// Session `s`'s segments from every addon with a `Ui`, cleaned, by
/// priority.
fn segments_of(rt: &AddonRuntime, s: &SessionSnap) -> Vec<(&'static str, Segment)> {
    let mut all = Vec::new();
    for entry in rt.addons().iter() {
        let Some(ui) = entry.ui() else {
            continue;
        };
        let ctx = rt.ctx(entry.name());
        let own = ui.segments(s, ctx.as_ref()).into_iter();
        all.extend(own.map(|seg| (entry.name(), addon::segment(seg))));
    }
    all.sort_by_key(|(_, s)| s.priority);
    all
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoArgs {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PanelArgs {
    addon: String,
    panel: String,
    #[serde(default)]
    page: usize,
    #[serde(default)]
    window: Option<String>,
    #[serde(default)]
    sid: Option<SessionId>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ActionArgs {
    addon: String,
    panel: String,
    action: String,
    #[serde(default)]
    sid: Option<SessionId>,
}

/// A v2 `launch` (docs/web-protocol.md §4, LaunchSelect).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LaunchArg {
    pick: String,
    #[serde(default)]
    addon: Option<String>,
    #[serde(default)]
    option: Option<String>,
    #[serde(default)]
    flags: Vec<String>,
}

/// One checklist a v2 spawn skips.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeferArg {
    addon: String,
    id: String,
}

/// `launch` as a select: `option` names a registered launch addon and an
/// option (≤ 128 bytes); flags are labelled flags of some addon.
pub(crate) fn select_of(addons: &Addons, a: &LaunchArg) -> Result<LaunchSelect, String> {
    let bad = |why: &str| Err(format!("bad request: launch: {why}"));
    let pick = match (a.pick.as_str(), &a.addon, &a.option) {
        ("default", None, None) => Pick::Default,
        ("direct", None, None) => Pick::Direct,
        ("option", Some(addon), Some(option)) => {
            if addons.get(addon).is_none_or(|e| e.launch().is_none()) {
                return bad("no such addon");
            }
            if option.is_empty() || option.len() > OFF_BYTES {
                return bad("option: 1..=128 bytes");
            }
            let (addon, option) = (addon.clone(), option.clone());
            Pick::Option { addon, option }
        }
        ("default" | "direct" | "option", _, _) => {
            return bad("addon and option go with \"option\" only")
        }
        _ => return bad("pick: default, direct or option"),
    };
    if a.flags.len() > FLAGS_MAX {
        return bad("at most 16 flags");
    }
    let launches = addons.iter().filter_map(|e| e.launch());
    let known: Vec<Value> = launches.flat_map(labelled_flags).collect();
    let named = |f: &String| known.iter().any(|k| k["name"] == f.as_str());
    if let Some(f) = a.flags.iter().find(|f| !named(f)) {
        return bad(&format!("no flag {}", line(f, 64)));
    }
    let flags = a.flags.clone();
    Ok(LaunchSelect { pick, flags })
}

/// A v2 `defer` as the launch's `(addon, checklist)` pairs.
pub(crate) fn deferred_of(
    addons: &Addons,
    defer: &[DeferArg],
) -> Result<Vec<(String, String)>, String> {
    if defer.len() > DEFER_MAX {
        return Err("bad request: defer: at most 16".to_owned());
    }
    defer
        .iter()
        .map(|d| {
            let addon = addon_named(addons, &d.addon)?;
            check_id("defer", &d.id)?;
            Ok((addon.to_owned(), d.id.clone()))
        })
        .collect()
}

/// A checklist save's answer: `{"off": [ids]}` or `"defer"`.
#[derive(Deserialize)]
#[serde(untagged)]
enum AnswerArg {
    Off(OffArg),
    Word(String),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OffArg {
    off: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveArgs {
    addon: String,
    checklist: String,
    revision: String,
    answer: AnswerArg,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RowArgs {
    addon: String,
    name: String,
    action: String,
    #[serde(default)]
    on: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChecklistArgs {
    addon: String,
    checklist: String,
}

/// The registered addon `name` (its static name).
pub(crate) fn addon_named(addons: &Addons, name: &str) -> Result<&'static str, String> {
    match addons.get(name) {
        Some(a) => Ok(a.name()),
        None => Err("bad request: addon: no such addon".to_owned()),
    }
}

/// `value` as an id: `[a-z0-9-]{1,64}`.
pub(crate) fn check_id(field: &str, value: &str) -> Result<(), String> {
    let ok = (1..=64).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if ok {
        Ok(())
    } else {
        Err(format!("bad request: {field}: not an id"))
    }
}

/// Panel `id` of an addon's `Ui`.
fn panel_of<'a>(ui: &'a dyn Ui, id: &str) -> Result<&'a PanelDecl, String> {
    let found = ui.panels().iter().find(|p| p.id == id);
    found.ok_or_else(|| "bad request: panel: no such panel".to_owned())
}

/// One `addons.*` op (docs/web-protocol.md §5, "Addons").
pub(crate) async fn dispatch(state: &AppState, op: &str, args: Value) -> Result<Value, String> {
    let rt = &state.addons;
    match op {
        "addons.list" => {
            parse::<NoArgs>(args)?;
            Ok(list(rt))
        }
        "addons.panel" => panel(state, parse(args)?).await,
        "addons.action" => action(state, parse(args)?).await,
        "addons.checklist" => {
            let a: ChecklistArgs = parse(args)?;
            let addon = addon_named(rt.addons(), &a.addon)?;
            check_id("checklist", &a.checklist)?;
            let entry = rt.addons().get(addon);
            let ctx = rt.ctx(addon);
            let found = entry.and_then(|e| e.ui()?.checklist(&a.checklist, ctx.as_ref()));
            let checklist = found.ok_or_else(|| "no such checklist".to_owned())?;
            to_value(addon::checklist(checklist))
        }
        "addons.rows" => {
            parse::<NoArgs>(args)?;
            Ok(rows(state))
        }
        "addons.checklist.save" => save(rt, parse(args)?).await,
        "addons.row_action" => row_action(state, parse(args)?).await,
        _ => Err("unknown op".to_owned()),
    }
}

fn to_value<T: serde::Serialize>(v: T) -> Result<Value, String> {
    serde_json::to_value(v).map_err(|_| "cannot encode the reply".to_owned())
}

/// `addons.list`: every addon with panels, launch options or labelled
/// launch flags, in registry order.
fn list(rt: &AddonRuntime) -> Value {
    let mut out = Vec::new();
    for entry in rt.addons().iter() {
        let name = entry.name();
        let panels: Vec<Value> = entry
            .ui()
            .map(|ui| ui.panels())
            .unwrap_or_default()
            .iter()
            .map(|p| {
                json!({"id": p.id, "title": line(p.title, TEXT_BYTES), "windows": p.windows,
                    "key": p.key.map(|k| k.default)})
            })
            .collect();
        let (options, flags) = match entry.launch() {
            Some(launch) => {
                let ctx = rt.ctx(name);
                let options: Vec<_> = launch.options(ctx.as_ref());
                let options: Vec<_> = options.into_iter().map(addon::option).collect();
                (options, labelled_flags(launch))
            }
            None => (Vec::new(), Vec::new()),
        };
        if panels.is_empty() && options.is_empty() && flags.is_empty() {
            continue;
        }
        out.push(
            json!({"name": name, "panels": panels, "launch_options": options,
            "launch_flags": flags}),
        );
    }
    json!({ "addons": out })
}

/// A launch addon's labelled switches as `{"name","label"}`: `name` is
/// what its `select` makes of the switch (a `LaunchSelect` flag).
pub(crate) fn labelled_flags(launch: &dyn crate::addon::Launch) -> Vec<Value> {
    launch
        .flags()
        .iter()
        .filter(|d| d.value.is_none())
        .filter_map(|d| {
            let label = d.label?;
            let given = [FlagArg {
                name: d.name.to_owned(),
                value: None,
            }];
            let flag = launch.select(&given).ok()?.flags.into_iter().next()?;
            Some(json!({"name": line(&flag, TEXT_BYTES), "label": line(label, TEXT_BYTES)}))
        })
        .collect()
}

/// `addons.panel` within 10 s: cleaned, or an error page.
async fn panel(state: &AppState, a: PanelArgs) -> Result<Value, String> {
    let rt = &state.addons;
    let addon = addon_named(rt.addons(), &a.addon)?;
    check_id("panel", &a.panel)?;
    let entry = rt.addons().get(addon).cloned();
    let entry = entry.ok_or_else(|| "bad request: addon: no such addon".to_owned())?;
    let ui = entry
        .ui()
        .ok_or_else(|| "bad request: panel: no such panel".to_owned())?;
    let decl = *panel_of(ui, &a.panel)?;
    if let Some(w) = &a.window {
        if !decl.windows.contains(&w.as_str()) {
            return Err("bad request: window: not one of the panel's".to_owned());
        }
    }
    if let Some(sid) = a.sid {
        state.hub.session(sid).await?;
    }
    let req = PanelRequest {
        id: a.panel,
        page: a.page,
        window: a.window,
        sid: a.sid,
    };
    let ctx = rt.ctx(addon);
    let mut job = tokio::spawn(async move {
        let ui = entry.ui()?;
        Some(ui.panel(req, ctx).await)
    });
    let shown = match tokio::time::timeout(PANEL_WAIT, &mut job).await {
        Ok(Ok(Some(p))) => addon::panel(p),
        Ok(_) => error_panel(decl.title, "failed"),
        Err(_) => {
            job.abort();
            error_panel(decl.title, "timed out")
        }
    };
    to_value(shown)
}

/// `addons.action` within 15 s: its notice, cleaned.
async fn action(state: &AppState, a: ActionArgs) -> Result<Value, String> {
    let rt = &state.addons;
    let addon = addon_named(rt.addons(), &a.addon)?;
    check_id("panel", &a.panel)?;
    check_id("action", &a.action)?;
    let entry = rt.addons().get(addon).cloned();
    let entry = entry.ok_or_else(|| "bad request: addon: no such addon".to_owned())?;
    let ui = entry
        .ui()
        .ok_or_else(|| "bad request: panel: no such panel".to_owned())?;
    panel_of(ui, &a.panel)?;
    if let Some(sid) = a.sid {
        state.hub.session(sid).await?;
    }
    let req = ActionRequest {
        panel: a.panel,
        action: a.action.clone(),
        sid: a.sid,
    };
    tracing::info!(op = "addons.action", addon, action = %a.action, sid = ?a.sid, "web op");
    let ctx = rt.ctx(addon);
    let mut job = tokio::spawn(async move {
        let ui = entry.ui().ok_or("no actions")?;
        ui.action(req, ctx).await
    });
    match tokio::time::timeout(ACTION_WAIT, &mut job).await {
        Ok(Ok(Ok(notice))) => to_value(addon::notice(notice)),
        Ok(Ok(Err(why))) => Err(format!("{addon}: {}", line(&why, TEXT_BYTES))),
        Ok(Err(_)) => Err(format!("{addon}: the action failed")),
        Err(_) => {
            job.abort();
            Err(format!("{addon}: the action timed out"))
        }
    }
}

/// `addons.checklist.save` within 15 s: refused as `stale` when the
/// checklist changed since the page read it; the ids it turns off must
/// be its items'.
async fn save(rt: &AddonRuntime, a: SaveArgs) -> Result<Value, String> {
    let addon = addon_named(rt.addons(), &a.addon)?;
    check_id("checklist", &a.checklist)?;
    let entry = rt.addons().get(addon).cloned();
    let entry = entry.ok_or_else(|| "bad request: addon: no such addon".to_owned())?;
    let ui = entry.ui().ok_or_else(|| "no such checklist".to_owned())?;
    let now = ui.checklist(&a.checklist, rt.ctx(addon).as_ref());
    let now = addon::checklist(now.ok_or_else(|| "no such checklist".to_owned())?);
    if now.revision != a.revision {
        return Err("stale".to_owned());
    }
    let answer = match a.answer {
        AnswerArg::Word(w) if w == "defer" => ChecklistAnswer::Defer,
        AnswerArg::Word(_) => {
            return Err(r#"bad request: answer: {"off": [ids]} or "defer""#.to_owned())
        }
        AnswerArg::Off(o) => {
            if o.off.len() > OFF_MAX || o.off.iter().any(|i| i.len() > OFF_BYTES) {
                return Err("bad request: answer: at most 256 ids of 128 bytes".to_owned());
            }
            if let Some(bad) = o
                .off
                .iter()
                .find(|i| !now.items.iter().any(|x| &x.id == *i))
            {
                return Err(format!("bad request: answer: no item {}", line(bad, 64)));
            }
            let revision = a.revision;
            ChecklistAnswer::Save {
                revision,
                off: o.off,
            }
        }
    };
    let saving = ui.save_checklist(a.checklist.clone(), answer, rt.ctx(addon));
    match tokio::time::timeout(ACTION_WAIT, saving).await {
        Ok(Ok(())) => {
            tracing::info!(op = "addons.checklist.save", addon, checklist = %a.checklist, "web op");
            Ok(json!({}))
        }
        Ok(Err(why)) => Err(line(&why, TEXT_BYTES)),
        Err(_) => Err(format!("{addon}: the save timed out")),
    }
}

/// `addons.row_action`: a toggle goes the core's way (`[extensions]`
/// written, the addon told; the registry and `extensions_changed`), a
/// refresh to the addon (15 s).
async fn row_action(state: &AppState, a: RowArgs) -> Result<Value, String> {
    let rt = &state.addons;
    let addon = addon_named(rt.addons(), &a.addon)?;
    check_id("name", &a.name)?;
    let ours = {
        let registry = state.ctx.ext.registry();
        let registry = registry.lock().unwrap_or_else(|p| p.into_inner());
        let of = |e: &&crate::app::extensions::Extension| matches!(&e.source, Source::Addon { addon: x, .. } if *x == addon);
        registry
            .extensions
            .iter()
            .filter(of)
            .any(|e| e.name == a.name)
    };
    if !ours {
        return Err("bad request: name: no such row".to_owned());
    }
    match (a.action.as_str(), a.on) {
        ("toggle", Some(on)) => {
            let hub = &state.hub;
            state.ctx.ext.toggle(rt, hub, &a.name, on).await?;
        }
        ("toggle", None) => return Err("bad request: on: a toggle needs it".to_owned()),
        ("refresh", None) => {
            if let Some(n) = rt.row_action(addon, a.name, RowAction::Refresh).await? {
                let text = n.text;
                state
                    .hub
                    .broadcast(super::events::notice_event(&text, "info"));
            }
        }
        ("refresh", Some(_)) => return Err("bad request: on: a toggle's only".to_owned()),
        _ => return Err("bad request: action: toggle or refresh".to_owned()),
    }
    Ok(json!({}))
}

/// `addons.rows`: the registry's addon rows, as it has them.
fn rows(state: &AppState) -> Value {
    let registry = state.ctx.ext.registry();
    let registry = registry.lock().unwrap_or_else(|p| p.into_inner());
    let rows: Vec<Value> = registry
        .extensions
        .iter()
        .filter_map(|e| match &e.source {
            Source::Addon {
                addon,
                refused,
                details,
                setup,
            } => Some(json!({
                "addon": addon,
                "name": e.name,
                "description": e.description,
                "on": e.enabled,
                "refused": refused,
                "details": details,
                "setup": setup,
            })),
            _ => None,
        })
        .collect();
    json!({ "rows": rows })
}

#[cfg(test)]
mod tests;
