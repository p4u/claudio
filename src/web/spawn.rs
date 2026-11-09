//! Starting sessions from the browser: `session.spawn`, `session.shell`,
//! `session.respawn`. A claude session goes through the launch composer
//! (`app::launch`), as the TUI's: its select is the v2 `launch` (with
//! `defer`), or the v1 `proxy`/`first_party` (`plugins::select_of`, with
//! `skip_plugins`); the addons add their share there. A v2 spawn whose
//! addon needs a checklist reviewed answers `review` and starts nothing.
//! The env never reaches this file: it lives in the composer until the
//! `Msg::Spawn`/`Msg::Respawn` goes over the daemon socket.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::addons::{deferred_of, select_of as launch_select, DeferArg, LaunchArg};
use super::ext::Extensions;
use super::hub::{HubHandle, SessionRef};
use super::plugins::{deferred, notice_event, select_of};
use super::state::SessionMeta;
use crate::addon::{Frontend, LaunchChoice, LaunchOp, LaunchSelect, LegacyChoice};
use crate::app::addons::AddonRuntime;
use crate::app::launch::{self, LaunchSpec, ReviewAnswer, Step};
use crate::client::Client;
use crate::proto::{Msg, RespawnSpec, SessionId, SessionKind, ShellSpec, SpawnSpec};

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum ProxyArg {
    Default,
    Direct,
    Profile { name: String },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SpawnReq {
    pub host: String,
    pub cwd: String,
    pub rows: u16,
    pub cols: u16,
    #[serde(default)]
    pub proxy: Option<ProxyArg>,
    #[serde(default)]
    pub first_party: bool,
    #[serde(default)]
    pub launch: Option<LaunchArg>,
    #[serde(default)]
    pub defer: Vec<DeferArg>,
    #[serde(default)]
    pub resume: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub skip_plugins: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RespawnReq {
    pub sid: SessionId,
    pub fresh: bool,
    pub rows: u16,
    pub cols: u16,
    #[serde(default)]
    pub proxy: Option<ProxyArg>,
    #[serde(default)]
    pub first_party: bool,
    #[serde(default)]
    pub launch: Option<LaunchArg>,
}

/// The most bytes of an opening prompt.
const PROMPT_MAX: usize = 64 * 1024;
/// The most bytes of a cwd.
const CWD_MAX: usize = 4096;
/// The most characters of a name.
const NAME_MAX: usize = 200;

/// `rows`/`cols` within 1..=1000.
pub(crate) fn check_size(rows: u16, cols: u16) -> Result<(), String> {
    for (field, v) in [("rows", rows), ("cols", cols)] {
        if !(1..=1000).contains(&v) {
            return Err(format!("bad request: {field}: must be 1..=1000"));
        }
    }
    Ok(())
}

pub(crate) fn check_cwd(cwd: &str) -> Result<(), String> {
    if cwd.is_empty() || cwd.len() > CWD_MAX {
        return Err(format!("bad request: cwd: must be 1..={CWD_MAX} bytes"));
    }
    Ok(())
}

/// A session name as the daemon gets it: sanitized, at most 200
/// characters; empty is no name.
pub(crate) fn check_name(name: Option<&str>) -> Result<Option<String>, String> {
    let Some(name) = name else {
        return Ok(None);
    };
    let clean = crate::app::label::sanitize_label(name, NAME_MAX + 1);
    if clean.chars().count() > NAME_MAX {
        return Err(format!("bad request: name: at most {NAME_MAX} characters"));
    }
    Ok(Some(clean).filter(|n| !n.trim().is_empty()))
}

/// Checks shared with Task 5: cwd 1..=4096 bytes; rows/cols 1..=1000; name ≤ 200 chars after `sanitize_label`; resume `[A-Za-z0-9-]{1,64}`, not starting with `-`; prompt ≤ 64 KiB.
pub(crate) fn check_spawn(req: &SpawnReq) -> Result<(), String> {
    check_cwd(&req.cwd)?;
    check_size(req.rows, req.cols)?;
    check_name(req.name.as_deref())?;
    if let Some(resume) = &req.resume {
        let ok = (1..=64).contains(&resume.len())
            && resume
                .bytes()
                .next()
                .is_some_and(|b| b.is_ascii_alphanumeric())
            && resume
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-');
        if !ok {
            return Err("bad request: resume: not a conversation id".to_owned());
        }
    }
    if req.prompt.as_ref().is_some_and(|p| p.len() > PROMPT_MAX) {
        return Err("bad request: prompt: at most 64 KiB".to_owned());
    }
    Ok(())
}

const FIRST_PARTY: &str = "first-party needs a proxy profile";
/// v1 and v2 launch args together.
const BOTH: &str = "bad request: launch: not with proxy, first_party or skip_plugins";
/// Launch flags with no addon's option in effect.
const FLAGS_ALONE: &str = "bad request: launch: flags need an addon's option";

/// What a spawn needs besides its request: the hub, the extensions (the
/// core's lenses) and the addon runtime the launch composer runs on.
pub(crate) struct SpawnCtx {
    pub hub: HubHandle,
    pub ext: Arc<Extensions>,
    pub runtime: AddonRuntime,
}

fn reply_id(reply: std::io::Result<Msg>) -> Result<SessionId, String> {
    match reply {
        Ok(Msg::Spawned { id, .. }) => Ok(id),
        Ok(_) => Err("unexpected reply from the daemon".to_owned()),
        Err(e) => Err(super::hub::daemon_error(e)),
    }
}

/// `select`'s routing, checked: first-party needs an addon's option.
fn check_first_party(
    ctx: &SpawnCtx,
    select: &LaunchSelect,
    first_party: bool,
) -> Result<(), String> {
    let (choice, _) = launch::resolve(&ctx.runtime, select)?;
    match choice {
        None if first_party => Err(FIRST_PARTY.to_owned()),
        _ => Ok(()),
    }
}

/// `base` (a claude `Spawn` or `Respawn` without env or extensions)
/// launched through the launch composer (`app::launch`) as `select` asks,
/// with `deferred` skipped: the core's lenses, the addons' share and
/// `env_remove` are added there, off this function's view; the notices go
/// to the tabs. With `review` (a v2 spawn), a checklist to review stops it
/// (nothing is kept, nothing started); else (v1) it is left out with a
/// notice.
async fn launch(
    ctx: &SpawnCtx,
    (op, select, deferred, review): (LaunchOp, LaunchSelect, Vec<(String, String)>, bool),
    (host, cwd): (&str, &str),
    base: Msg,
    client: Client,
) -> Result<Launched, String> {
    let sid = match &base {
        Msg::Spawn(spec) => spec.id,
        Msg::Respawn(spec) => spec.id,
        _ => return Err("only a claude session is launched".to_owned()),
    };
    let spec = LaunchSpec {
        op,
        sid,
        host: host.to_owned(),
        cwd: cwd.to_owned(),
        select,
        frontend: Frontend::Web,
        base,
        core_lenses: ctx.ext.session_extensions(),
    };
    let mut step = launch::run_deferring(&ctx.runtime, spec, client.clone(), &deferred).await;
    if let (
        true,
        Step::Review {
            addon,
            checklist,
            revision,
            ..
        },
    ) = (review, &step)
    {
        let (addon, checklist, revision) = (*addon, checklist.clone(), revision.clone());
        return Ok(Launched::Review {
            addon,
            checklist,
            revision,
        });
    }
    // Web protocol v1 has no checklists: a plugin still to review is left
    // out of this launch, and said.
    while let Step::Review {
        checklist, parked, ..
    } = step
    {
        let text = format!("plugin {checklist} left out: set it up first");
        ctx.hub.broadcast(notice_event(&text, "warn"));
        step = launch::resume(&ctx.runtime, parked, ReviewAnswer::Skip, client.clone()).await;
    }
    match step {
        Step::Sent {
            choice,
            notices,
            reply,
        } => {
            for text in &notices {
                ctx.hub.broadcast(notice_event(text, "warn"));
            }
            Ok(Launched::Sent(reply_id(reply)?, choice))
        }
        Step::Review { addon, .. } => Err(format!("{addon}: review needed")),
        Step::Failed(why) => Err(why),
    }
}

/// Where a launch got to.
enum Launched {
    /// Started: the session and its launch's choice.
    Sent(SessionId, Option<LaunchChoice>),
    /// Not started: `addon` wants checklist `checklist` (at `revision`)
    /// reviewed first (v2).
    Review {
        addon: &'static str,
        checklist: String,
        revision: String,
    },
}

/// A launch's select, the checklists it skips, and whether it is a v2
/// one (a review is answered, not skipped).
type How = (LaunchSelect, Vec<(String, String)>, bool);

/// How a spawn asks to be launched.
fn how_of(ctx: &SpawnCtx, req: &SpawnReq) -> Result<How, String> {
    let v1 = req.proxy.is_some() || req.first_party || !req.skip_plugins.is_empty();
    let v2 = req.launch.is_some() || !req.defer.is_empty();
    if v1 && v2 {
        return Err(BOTH.to_owned());
    }
    if v1 {
        let proxy = req.proxy.clone().unwrap_or(ProxyArg::Default);
        let select = select_of(&proxy, req.first_party);
        check_first_party(ctx, &select, req.first_party)?;
        return Ok((select, deferred(&req.skip_plugins), false));
    }
    let select = v2_select(ctx, req.launch.as_ref())?;
    let deferred = deferred_of(ctx.runtime.addons(), &req.defer)?;
    Ok((select, deferred, true))
}

/// A v2 `launch` (default: the addons' default) as a select, checked:
/// flags need an addon's option in effect.
fn v2_select(ctx: &SpawnCtx, arg: Option<&LaunchArg>) -> Result<LaunchSelect, String> {
    let select = match arg {
        Some(arg) => launch_select(ctx.runtime.addons(), arg)?,
        None => LaunchSelect::default(),
    };
    if !select.flags.is_empty() {
        let (choice, _) = launch::resolve(&ctx.runtime, &select)?;
        if choice.is_none() {
            return Err(FLAGS_ALONE.to_owned());
        }
    }
    Ok(select)
}

/// The record of a session launched with `choice` (web-state.json keeps
/// its pre-addon fields until P3.19).
fn meta_of(ctx: &SpawnCtx, host: &str, choice: Option<&LaunchChoice>) -> SessionMeta {
    let legacy = launch::legacy(ctx.runtime.addons(), choice);
    SessionMeta {
        host: host.to_owned(),
        proxy: legacy.proxy,
        first_party: legacy.first_party,
    }
}

/// `session.spawn`: `{"sid"}`, or (v2) `{"review"}`.
pub(crate) async fn spawn_claude(ctx: &SpawnCtx, req: SpawnReq) -> Result<Value, String> {
    check_spawn(&req)?;
    let hub = &ctx.hub;
    let (client, _) = hub.host_client(&req.host).await?;
    let (select, deferred, review) = how_of(ctx, &req)?;
    let args = req
        .resume
        .as_ref()
        .map(|r| vec!["--resume".to_owned(), r.clone()])
        .unwrap_or_default();
    let spec = SpawnSpec {
        id: Uuid::new_v4(),
        cwd: req.cwd.clone(),
        name: check_name(req.name.as_deref())?,
        args,
        rows: req.rows,
        cols: req.cols,
        ..Default::default()
    };
    let op = match req.resume {
        Some(_) => LaunchOp::Resume,
        None => LaunchOp::New,
    };
    let how = (op, select, deferred, review);
    let place = (req.host.as_str(), req.cwd.as_str());
    let (sid, choice) = match launch(ctx, how, place, Msg::Spawn(spec), client).await? {
        Launched::Sent(sid, choice) => (sid, choice),
        Launched::Review {
            addon,
            checklist,
            revision,
        } => {
            let review = json!({"addon": addon, "id": checklist, "revision": revision});
            return Ok(json!({ "review": review }));
        }
    };
    tracing::info!(op = "session.spawn", host = %req.host, %sid, "web op");
    let meta = meta_of(ctx, &req.host, choice.as_ref());
    let prompt = req
        .prompt
        .filter(|p| !p.trim().is_empty())
        .map(|p| (p, (req.rows, req.cols)));
    hub.spawned(req.host, sid, meta, prompt).await;
    Ok(json!({ "sid": sid }))
}

pub(crate) async fn spawn_shell(
    ctx: &SpawnCtx,
    host: String,
    cwd: String,
    name: Option<String>,
    rows: u16,
    cols: u16,
) -> Result<SessionId, String> {
    check_cwd(&cwd)?;
    check_size(rows, cols)?;
    let name = check_name(name.as_deref())?;
    let hub = &ctx.hub;
    let (client, _) = hub.host_client(&host).await?;
    let spec = ShellSpec {
        id: Uuid::new_v4(),
        cwd,
        name,
        rows,
        cols,
    };
    let sid = reply_id(client.request(Msg::SpawnShell(spec)).await)?;
    tracing::info!(op = "session.shell", host = %host, %sid, "web op");
    let meta = meta_of(ctx, &host, None);
    hub.spawned(host, sid, meta, None).await;
    Ok(sid)
}

/// How a claude session is launched again: as the request says (v2
/// `launch` or v1 `proxy`), else as it was (its recorded routing; one no
/// addon adopts fails, never direct), else "proxy unknown" (R19).
fn respawn_select(
    ctx: &SpawnCtx,
    req: &RespawnReq,
    session: &SessionRef,
) -> Result<LaunchSelect, String> {
    if req.launch.is_some() && (req.proxy.is_some() || req.first_party) {
        return Err(BOTH.to_owned());
    }
    if let Some(arg) = &req.launch {
        return v2_select(ctx, Some(arg));
    }
    if let Some(arg) = &req.proxy {
        let select = select_of(arg, req.first_party);
        check_first_party(ctx, &select, req.first_party)?;
        return Ok(select);
    }
    if !session.proxy_known {
        return Err("proxy unknown".to_owned());
    }
    let legacy = LegacyChoice {
        proxy: session.proxy.clone(),
        first_party: session.first_party,
    };
    let choice = launch::adopt_legacy(ctx.runtime.addons(), &legacy)?;
    Ok(LaunchSelect::of(choice.as_ref()))
}

pub(crate) async fn respawn(ctx: &SpawnCtx, req: RespawnReq) -> Result<SessionId, String> {
    check_size(req.rows, req.cols)?;
    let hub = &ctx.hub;
    let session = hub.session(req.sid).await?;
    let spec = RespawnSpec {
        id: req.sid,
        fresh: req.fresh,
        rows: req.rows,
        cols: req.cols,
        ..Default::default()
    };
    let (sid, choice) = if session.info.kind == SessionKind::Claude {
        let select = respawn_select(ctx, &req, &session)?;
        let how = (LaunchOp::Respawn, select, Vec::new(), false);
        let place = (session.host.as_str(), session.info.cwd.as_str());
        let client = session.client.clone();
        match launch(ctx, how, place, Msg::Respawn(spec), client).await? {
            Launched::Sent(sid, choice) => (sid, choice),
            Launched::Review { addon, .. } => return Err(format!("{addon}: review needed")),
        }
    } else {
        let msg = Msg::Respawn(spec);
        (reply_id(session.client.request(msg).await)?, None)
    };
    tracing::info!(op = "session.respawn", host = %session.host, %sid, "web op");
    let meta = meta_of(ctx, &session.host, choice.as_ref());
    hub.spawned(session.host, sid, meta, None).await;
    Ok(sid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req() -> SpawnReq {
        serde_json::from_value(serde_json::json!({
            "host": "local", "cwd": "/w", "rows": 24, "cols": 80, "proxy": {"kind": "direct"}
        }))
        .unwrap()
    }

    #[test]
    fn spawn_requests_take_v2_launch_args_and_refuse_unknown_ones_inside() {
        let ok = serde_json::json!({
            "host": "local", "cwd": "/w", "rows": 24, "cols": 80,
            "launch": {"pick": "option", "addon": "vocdoni", "option": "work", "flags": ["first-party"]},
            "defer": [{"addon": "vocdoni", "id": "kb"}]
        });
        let r: SpawnReq = serde_json::from_value(ok).unwrap();
        assert!(r.proxy.is_none() && r.launch.is_some() && r.defer.len() == 1);
        for bad in [
            serde_json::json!({"pick": "default", "profile": "work"}),
            serde_json::json!({"pick": "option", "addon": "vocdoni", "option": "w", "token": "x"}),
        ] {
            let req = serde_json::json!({"host": "local", "cwd": "/w", "rows": 24, "cols": 80, "launch": bad});
            assert!(serde_json::from_value::<SpawnReq>(req).is_err());
        }
        let req = serde_json::json!({"host": "local", "cwd": "/w", "rows": 24, "cols": 80,
            "defer": [{"addon": "vocdoni", "id": "kb", "x": 1}]});
        assert!(serde_json::from_value::<SpawnReq>(req).is_err());
    }

    #[test]
    fn spawn_checks_answer_bad_request_with_the_field() {
        assert!(check_spawn(&req()).is_ok());
        // A mutation of a valid request and the field the check must name.
        type Case = (Box<dyn Fn(&mut SpawnReq)>, &'static str);
        let cases: Vec<Case> = vec![
            (Box::new(|r| r.cwd.clear()), "cwd"),
            (Box::new(|r| r.cwd = "/".repeat(4097)), "cwd"),
            (Box::new(|r| r.rows = 0), "rows"),
            (Box::new(|r| r.cols = 1001), "cols"),
            (Box::new(|r| r.name = Some("n".repeat(201))), "name"),
            (Box::new(|r| r.resume = Some("../x".into())), "resume"),
            (Box::new(|r| r.resume = Some("a".repeat(65))), "resume"),
            (Box::new(|r| r.resume = Some("-x".into())), "resume"),
            (
                Box::new(|r| r.prompt = Some("p".repeat(64 * 1024 + 1))),
                "prompt",
            ),
        ];
        for (change, field) in cases {
            let mut r = req();
            change(&mut r);
            let err = check_spawn(&r).unwrap_err();
            assert!(err.starts_with(&format!("bad request: {field}: ")), "{err}");
        }
        let mut r = req();
        r.name = Some("n".repeat(200));
        r.resume = Some("0b9c-AF12".into());
        r.prompt = Some("p".repeat(64 * 1024));
        assert!(check_spawn(&r).is_ok());
    }

    #[test]
    fn spawn_requests_refuse_unknown_keys() {
        let bad = serde_json::json!({
            "host": "local", "cwd": "/w", "rows": 24, "cols": 80,
            "proxy": {"kind": "direct"}, "bogus": 1
        });
        assert!(serde_json::from_value::<SpawnReq>(bad).is_err());
        let p: ProxyArg =
            serde_json::from_value(serde_json::json!({"kind": "profile", "name": "w"})).unwrap();
        assert!(matches!(p, ProxyArg::Profile { name } if name == "w"));
    }
}
