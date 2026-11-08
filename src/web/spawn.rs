//! Starting sessions from the browser: `session.spawn`, `session.shell`,
//! `session.respawn`, composed as the TUI does (src/tui/sessions.rs
//! `send_spawn`/`send_with_proxy`): the proxy profile's env, the plugins'
//! flags, env and instructions, the extensions' lenses. Nothing
//! env-bearing leaves this file except in `Msg::Spawn`/`Msg::Respawn` over
//! the daemon socket: no DTO, log line, notice or error carries it.

use std::sync::Arc;

use serde::Deserialize;
use uuid::Uuid;

use super::ext::Extensions;
use super::hub::HubHandle;
use super::plugins::{notice_event, Plugins};
use super::proxy::ProxyCache;
use super::state::SessionMeta;
use crate::proto::{Msg, RespawnSpec, SessionId, SessionKind, ShellSpec, SpawnSpec};
use crate::tui::plugins::{compose, with_plugins, PluginPart};

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
    pub proxy: ProxyArg,
    #[serde(default)]
    pub first_party: bool,
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
    let clean = crate::tui::sessions::sanitize_label(name, NAME_MAX + 1);
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

/// What a spawn needs besides its request: the hub, the proxy config
/// cache, the plugins and the extensions.
pub(crate) struct SpawnCtx {
    pub hub: HubHandle,
    pub proxy: ProxyCache,
    pub plugins: Plugins,
    pub ext: Arc<Extensions>,
}

/// default → `default` (the `[proxy] default`, or "env" with CLAUDIO_PROXY_URL); direct → none; profile → its name.
pub(crate) fn pick_profile(arg: &ProxyArg, default: Option<&str>) -> Option<String> {
    match arg {
        ProxyArg::Default => default.map(str::to_owned),
        ProxyArg::Direct => None,
        ProxyArg::Profile { name } => Some(name.clone()),
    }
}

/// The proxy profile `arg` names, with the default read from config.toml
/// and the env (in `spawn_blocking`): `None` is direct.
async fn resolve(arg: &ProxyArg) -> Result<Option<String>, String> {
    let default = match arg {
        ProxyArg::Default => {
            tokio::task::spawn_blocking(crate::proxy::resolve::load_proxy_profiles)
                .await
                .map_err(|_| "cannot read config.toml".to_owned())?
                .1
        }
        _ => None,
    };
    Ok(pick_profile(arg, default.as_deref()))
}

fn reply_id(reply: std::io::Result<Msg>) -> Result<SessionId, String> {
    match reply {
        Ok(Msg::Spawned { id, .. }) => Ok(id),
        Ok(_) => Err("unexpected reply from the daemon".to_owned()),
        Err(e) => Err(super::hub::daemon_error(e)),
    }
}

/// `profile`'s env for a claude session (the proxy URL and token, its
/// config's models), built in `spawn_blocking` (it reads config.toml). It
/// goes into the daemon request only.
async fn proxy_env(
    ctx: &SpawnCtx,
    profile: &str,
    first_party: bool,
) -> Result<Vec<(String, String)>, String> {
    let p = ProxyCache::profile(profile).await?;
    let config = ctx.proxy.config(profile, &p).await;
    drop(p);
    let name = profile.to_owned();
    tokio::task::spawn_blocking(move || {
        crate::proxy::resolve::proxy_env_for(Some(&name), config.as_ref(), first_party)
    })
    .await
    .map_err(|_| "cannot read config.toml".to_owned())?
}

/// `msg` (a `Spawn` or `Respawn` without env) with the plugins' share and
/// `profile`'s env, exactly as the TUI's `send_with_proxy`: compose's
/// notices go to the tabs (they name plugins, never values).
async fn with_proxy(
    ctx: &SpawnCtx,
    msg: Msg,
    profile: Option<&str>,
    first_party: bool,
    parts: Vec<PluginPart>,
) -> Result<Msg, String> {
    let env = match profile {
        Some(name) => proxy_env(ctx, name, first_party).await?,
        None => Vec::new(),
    };
    let composed = compose(&parts);
    for text in &composed.notices {
        ctx.hub.broadcast(notice_event(text, "warn"));
    }
    Ok(with_plugins(msg, composed).merge_env(env))
}

pub(crate) async fn spawn_claude(ctx: &SpawnCtx, req: SpawnReq) -> Result<SessionId, String> {
    check_spawn(&req)?;
    let hub = &ctx.hub;
    let (client, _) = hub.host_client(&req.host).await?;
    let profile = resolve(&req.proxy).await?;
    if req.first_party && profile.is_none() {
        return Err(FIRST_PARTY.to_owned());
    }
    let parts = match &profile {
        Some(p) => ctx.plugins.parts(&req.host, p, &req.skip_plugins).await,
        None => Vec::new(),
    };
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
        env: vec![],
        rows: req.rows,
        cols: req.cols,
        ephemeral: false,
        extensions: Some(ctx.ext.session_extensions()),
    };
    let msg = with_proxy(
        ctx,
        Msg::Spawn(spec),
        profile.as_deref(),
        req.first_party,
        parts,
    )
    .await?;
    let sid = reply_id(client.request(msg).await)?;
    tracing::info!(op = "session.spawn", host = %req.host, %sid, "web op");
    let meta = SessionMeta {
        host: req.host.clone(),
        proxy: profile,
        first_party: req.first_party,
    };
    let prompt = req
        .prompt
        .filter(|p| !p.trim().is_empty())
        .map(|p| (p, (req.rows, req.cols)));
    hub.spawned(req.host, sid, meta, prompt).await;
    Ok(sid)
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
    let meta = SessionMeta {
        host: host.clone(),
        proxy: None,
        first_party: false,
    };
    hub.spawned(host, sid, meta, None).await;
    Ok(sid)
}

pub(crate) async fn respawn(ctx: &SpawnCtx, req: RespawnReq) -> Result<SessionId, String> {
    check_size(req.rows, req.cols)?;
    let hub = &ctx.hub;
    let session = hub.session(req.sid).await?;
    let spec = RespawnSpec {
        id: req.sid,
        fresh: req.fresh,
        env: vec![],
        rows: req.rows,
        cols: req.cols,
        extensions: None,
    };
    let (msg, profile, first_party) = if session.info.kind == SessionKind::Claude {
        let (profile, first_party) = match &req.proxy {
            Some(arg) => {
                let profile = resolve(arg).await?;
                if req.first_party && profile.is_none() {
                    return Err(FIRST_PARTY.to_owned());
                }
                (profile, req.first_party)
            }
            None if session.proxy_known => {
                let profile = session.proxy.clone();
                let first_party = session.first_party && profile.is_some();
                (profile, first_party)
            }
            None => return Err("proxy unknown".to_owned()),
        };
        // The plugins' env and instructions now (the journaled args keep
        // their flags); a respawn waits for no manifest, as the TUI's reset.
        let parts = match &profile {
            Some(p) => ctx.plugins.parts_now(&session.host, p).await,
            None => Vec::new(),
        };
        let spec = RespawnSpec {
            extensions: Some(ctx.ext.session_extensions()),
            ..spec
        };
        let msg = with_proxy(
            ctx,
            Msg::Respawn(spec),
            profile.as_deref(),
            first_party,
            parts,
        )
        .await?;
        (msg, profile, first_party)
    } else {
        (Msg::Respawn(spec), None, false)
    };
    let sid = reply_id(session.client.request(msg).await)?;
    tracing::info!(op = "session.respawn", host = %session.host, %sid, "web op");
    let meta = SessionMeta {
        host: session.host.clone(),
        proxy: profile,
        first_party,
    };
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
    fn spawn_checks_answer_bad_request_with_the_field() {
        assert!(check_spawn(&req()).is_ok());
        let cases: Vec<(Box<dyn Fn(&mut SpawnReq)>, &str)> = vec![
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
    fn pick_profile_follows_the_wizard() {
        let lab = ProxyArg::Profile { name: "lab".into() };
        assert_eq!(
            pick_profile(&ProxyArg::Default, Some("work")),
            Some("work".into())
        );
        assert_eq!(pick_profile(&ProxyArg::Default, None), None);
        assert_eq!(pick_profile(&ProxyArg::Direct, Some("work")), None);
        assert_eq!(pick_profile(&lab, Some("work")), Some("lab".into()));
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
