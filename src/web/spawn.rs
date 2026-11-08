//! Starting sessions from the browser: `session.spawn`, `session.shell`,
//! `session.respawn`. Minimal: direct sessions only (Task 5 adds proxy
//! profiles, plugins and extensions). Nothing env-bearing leaves this file
//! except over the daemon socket.

use serde::Deserialize;
use uuid::Uuid;

use super::hub::HubHandle;
use super::state::SessionMeta;
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
    #[allow(dead_code)] // plugins come with Task 5
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

/// Checks shared with Task 5: cwd 1..=4096 bytes; rows/cols 1..=1000; name ≤ 200 chars after `sanitize_label`; resume `[A-Za-z0-9-]{1,64}`; prompt ≤ 64 KiB.
pub(crate) fn check_spawn(req: &SpawnReq) -> Result<(), String> {
    check_cwd(&req.cwd)?;
    check_size(req.rows, req.cols)?;
    check_name(req.name.as_deref())?;
    if let Some(resume) = &req.resume {
        let ok = (1..=64).contains(&resume.len())
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

const NOT_WIRED: &str = "proxy profiles are not wired yet";
const FIRST_PARTY: &str = "first-party needs a proxy profile";

/// The proxy profile `arg` names, given config.toml's and the env's
/// (read in `spawn_blocking`): `None` is direct.
async fn resolve(arg: &ProxyArg) -> Result<Option<String>, String> {
    match arg {
        ProxyArg::Direct => Ok(None),
        ProxyArg::Profile { name } => Ok(Some(name.clone())),
        ProxyArg::Default => {
            let (_, default) =
                tokio::task::spawn_blocking(crate::proxy::resolve::load_proxy_profiles)
                    .await
                    .map_err(|_| "cannot read config.toml".to_owned())?;
            Ok(default)
        }
    }
}

fn reply_id(reply: std::io::Result<Msg>) -> Result<SessionId, String> {
    match reply {
        Ok(Msg::Spawned { id, .. }) => Ok(id),
        Ok(_) => Err("unexpected reply from the daemon".to_owned()),
        Err(e) => Err(super::hub::daemon_error(e)),
    }
}

pub(crate) async fn spawn_claude(hub: &HubHandle, req: SpawnReq) -> Result<SessionId, String> {
    check_spawn(&req)?;
    let (client, _) = hub.host_client(&req.host).await?;
    if resolve(&req.proxy).await?.is_some() {
        return Err(NOT_WIRED.to_owned());
    }
    if req.first_party {
        return Err(FIRST_PARTY.to_owned());
    }
    let id = Uuid::new_v4();
    let args = req
        .resume
        .as_ref()
        .map(|r| vec!["--resume".to_owned(), r.clone()])
        .unwrap_or_default();
    let spec = SpawnSpec {
        id,
        cwd: req.cwd.clone(),
        name: check_name(req.name.as_deref())?,
        args,
        env: vec![],
        rows: req.rows,
        cols: req.cols,
        ephemeral: false,
        extensions: None,
    };
    let sid = reply_id(client.request(Msg::Spawn(spec)).await)?;
    tracing::info!(op = "session.spawn", host = %req.host, %sid, "web op");
    let meta = SessionMeta {
        host: req.host.clone(),
        proxy: None,
        first_party: false,
    };
    let prompt = req
        .prompt
        .filter(|p| !p.trim().is_empty())
        .map(|p| (p, (req.rows, req.cols)));
    hub.spawned(req.host, sid, meta, prompt).await;
    Ok(sid)
}

pub(crate) async fn spawn_shell(
    hub: &HubHandle,
    host: String,
    cwd: String,
    name: Option<String>,
    rows: u16,
    cols: u16,
) -> Result<SessionId, String> {
    check_cwd(&cwd)?;
    check_size(rows, cols)?;
    let name = check_name(name.as_deref())?;
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

pub(crate) async fn respawn(hub: &HubHandle, req: RespawnReq) -> Result<SessionId, String> {
    check_size(req.rows, req.cols)?;
    let session = hub.session(req.sid).await?;
    let first_party = if session.info.kind == SessionKind::Claude {
        let proxy = match &req.proxy {
            Some(arg) => resolve(arg).await?,
            None if session.proxy_known => session.proxy.clone(),
            None => return Err("proxy unknown".to_owned()),
        };
        if proxy.is_some() {
            return Err(NOT_WIRED.to_owned());
        }
        let first_party = if req.proxy.is_some() {
            req.first_party
        } else {
            session.first_party
        };
        if first_party {
            return Err(FIRST_PARTY.to_owned());
        }
        first_party
    } else {
        false
    };
    let spec = RespawnSpec {
        id: req.sid,
        fresh: req.fresh,
        env: vec![],
        rows: req.rows,
        cols: req.cols,
        extensions: None,
    };
    let sid = reply_id(session.client.request(Msg::Respawn(spec)).await)?;
    tracing::info!(op = "session.respawn", host = %session.host, %sid, "web op");
    let meta = SessionMeta {
        host: session.host.clone(),
        proxy: None,
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
