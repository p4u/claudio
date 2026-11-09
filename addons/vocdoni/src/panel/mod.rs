//! The proxy panel (the stats popup of the releases before addons): the
//! profile's account, models, trends, pool and limits, and sessions, as
//! pages of blocks, and its action: moving a conversation to another
//! subscription. The profile is the session's (`req.sid`), else the
//! default, else the first; the conversation an action moves is the
//! addon's own snapshot of the session, never the request's.

mod blocks;
mod pages;
#[cfg(test)]
mod tests;

use std::sync::Arc;

pub use blocks::{fmt_age, tone_of};

use super::proxy::api::{self, SessionCredential, SwitchError};
use super::proxy::stats::Window;
use super::runner::Runner;
use super::NAME;
use claudio_core::addon::{
    ActionRequest, AddonCtx, Level, Notice, Page, Panel, PanelAction, PanelDecl, PanelKey,
    PanelRequest, SessionSnap, Tone,
};

/// The panel this addon offers.
pub const DECL: PanelDecl = PanelDecl {
    id: "proxy",
    title: "Proxy stats",
    key: Some(PanelKey {
        action: "proxy_stats",
        default: "alt+s",
    }),
    windows: &["24h", "7d", "30d"],
};

/// The switch action's id.
pub const SWITCH: &str = "switch";
/// Stats younger than this are shown as they are; older ones are fetched
/// again for the page asked.
const STALE_SECS: u64 = 10;

/// What the pages are built from.
pub struct View {
    pub profile: String,
    pub window: Window,
    /// The session asked about runs direct: its profile is borrowed.
    pub borrowed: bool,
    /// The session asked about is on this profile with a conversation.
    pub can_switch: bool,
    pub now: u64,
    /// The session asked about, when it is on this profile.
    pub session: Option<Row>,
    /// Every claude session the front-end knows.
    pub sessions: Vec<Row>,
}

/// A session as the panel shows it.
pub struct Row {
    /// Its place among the sessions, from 1.
    pub tab: usize,
    pub label: String,
    pub host: String,
    pub focused: bool,
    /// Whether it uses the panel's profile.
    pub on_profile: bool,
    pub cred: Option<SessionCredential>,
}

/// The profile session `s` was launched with, if this addon routes it.
pub fn profile_of(s: &SessionSnap) -> Option<String> {
    let choice = s.launch.as_ref().filter(|c| c.addon == NAME)?;
    Some(choice.option.clone())
}

/// The panel for `req`: its profile's numbers fetched as the page needs
/// them, then every page built.
pub async fn panel(runner: Arc<Runner>, req: PanelRequest, ctx: Arc<dyn AddonCtx>) -> Panel {
    let sessions = ctx.sessions();
    let asked = req
        .sid
        .and_then(|sid| sessions.iter().find(|s| s.sid == sid));
    let own = asked.and_then(profile_of);
    let profiles = profiles(&runner).await;
    let fallback = profiles.default.or_else(|| profiles.names.first().cloned());
    let Some(profile) = own.clone().or(fallback) else {
        return no_proxy();
    };
    let window = Window::ALL
        .into_iter()
        .find(|w| Some(w.param()) == req.window.as_deref())
        .unwrap_or(Window::H24);
    fetch(&runner, &profile, req.page, window).await;
    let borrowed = asked.is_some() && own.is_none();
    let view = view(&runner, &sessions, asked, (profile, window, borrowed));
    build(&runner, view)
}

/// The profiles, read off the executor the first time.
async fn profiles(runner: &Arc<Runner>) -> crate::profiles::Profiles {
    if let Some(p) = runner.state().profiles() {
        return p;
    }
    let r = Arc::clone(runner);
    tokio::task::spawn_blocking(move || r.read_profiles())
        .await
        .unwrap_or_default()
}

/// Fetch what page `page` needs of `profile` at `window` that is missing
/// or older than [`STALE_SECS`]; the catalogue once.
async fn fetch(runner: &Arc<Runner>, profile: &str, page: usize, window: Window) {
    let wanted = match page {
        2 => Window::ALL.to_vec(),
        _ => vec![window],
    };
    let (windows, models) = {
        let state = runner.state();
        let now = claudio_core::paths::unix_now();
        let status = state.stats.get(profile);
        let fresh = status
            .and_then(|s| s.fetched_at)
            .is_some_and(|at| now.saturating_sub(at) < STALE_SECS);
        let have = status.map(|s| s.cached_windows()).unwrap_or_default();
        let windows: Vec<Window> = match fresh {
            true => wanted.into_iter().filter(|w| !have.contains(w)).collect(),
            false => wanted,
        };
        let models = status.is_none_or(|s| s.models.is_none());
        (windows, models)
    };
    if windows.is_empty() && !models {
        return;
    }
    let fetched = match runner.find_profile(profile).await {
        Some(p) => super::proxy::stats::fetch(&p, &windows, models).await,
        None => {
            let why = super::profiles::missing(profile);
            let stats = windows.iter().map(|w| (*w, Err(why.clone()))).collect();
            super::state::stats::ProxyFetch {
                stats,
                ..Default::default()
            }
        }
    };
    runner.transition(|s| {
        s.on_stats(profile, fetched);
        Vec::new()
    });
}

/// The view of `profile` for the session `asked` about, if any.
fn view(
    runner: &Runner,
    sessions: &[SessionSnap],
    asked: Option<&SessionSnap>,
    (profile, window, borrowed): (String, Window, bool),
) -> View {
    let state = runner.state();
    let now = claudio_core::paths::unix_now();
    let row = |(i, s): (usize, &SessionSnap)| Row {
        tab: i + 1,
        label: s.label.clone(),
        host: s.host.clone(),
        focused: s.focused,
        on_profile: profile_of(s).as_deref() == Some(profile.as_str()),
        cred: state.cred(s.sid, s.conversation.as_deref()).cloned(),
    };
    let claude = sessions
        .iter()
        .enumerate()
        .filter(|(_, s)| s.kind == claudio_core::proto::SessionKind::Claude);
    let rows: Vec<Row> = claude.map(row).collect();
    let session = asked.and_then(|a| {
        let i = sessions.iter().position(|s| s.sid == a.sid)?;
        Some(row((i, a))).filter(|r| r.on_profile)
    });
    let can_switch = session.is_some() && asked.is_some_and(|a| a.conversation.is_some());
    View {
        profile,
        window,
        borrowed,
        can_switch,
        now,
        session,
        sessions: rows,
    }
}

/// Every page of `view`, its action, and when its numbers were fetched.
fn build(runner: &Runner, view: View) -> Panel {
    let state = runner.state();
    let status = state.stats.get(&view.profile);
    let pages = pages::TITLES
        .iter()
        .enumerate()
        .map(|(i, title)| Page {
            title: (*title).to_owned(),
            blocks: pages::page(i, &view, status),
        })
        .collect();
    let actions = view
        .can_switch
        .then(|| PanelAction {
            id: SWITCH.to_owned(),
            label: "move to another subscription".to_owned(),
            key: Some('n'),
            confirm: Some("Move this conversation to another subscription?".to_owned()),
        })
        .into_iter()
        .collect();
    Panel {
        title: format!("Proxy stats: {}", view.profile),
        pages,
        actions,
        updated: status.and_then(|s| s.fetched_at),
    }
}

/// Without any profile: how to connect one.
fn no_proxy() -> Panel {
    let blocks = vec![
        blocks::toned("No proxy configured.", Tone::Accent),
        blocks::text(
            "This panel shows your claude-proxy account: requests, tokens, limits, \
             pool health and the model catalogue. To connect one, run:",
        ),
        blocks::toned(
            "claudio proxy login https://proxy.example.com",
            Tone::Accent,
        ),
        blocks::text(
            "(the token is read from the prompt or CLAUDIO_PROXY_URL=<token>@host), \
             then start a new session with Alt+n and pick the proxy profile.",
        ),
    ];
    Panel {
        title: DECL.title.to_owned(),
        pages: vec![Page {
            title: pages::TITLES[0].to_owned(),
            blocks,
        }],
        actions: Vec::new(),
        updated: None,
    }
}

/// Move session `req.sid`'s conversation to another subscription: its
/// profile and conversation are the addon's snapshot of the session.
pub async fn action(
    runner: Arc<Runner>,
    req: ActionRequest,
    ctx: Arc<dyn AddonCtx>,
) -> Result<Notice, String> {
    if req.panel != DECL.id || req.action != SWITCH {
        return Err("no such action".to_owned());
    }
    let sid = req.sid.ok_or("no session to move")?;
    let sessions = ctx.sessions();
    let s = sessions
        .iter()
        .find(|s| s.sid == sid)
        .ok_or("no such session")?;
    let profile = profile_of(s).ok_or("this session runs direct")?;
    let conversation = s.conversation.clone().filter(|c| !c.is_empty());
    let conversation = conversation.ok_or("this session has no conversation yet")?;
    let p = runner
        .find_profile(&profile)
        .await
        .ok_or_else(|| super::profiles::missing(&profile))?;
    let outcome = match api::switch_session(&p.url, &p.token, &conversation).await {
        Ok(o) => o,
        Err(SwitchError::Api(e)) => return Err(format!("switch failed: {e}")),
        Err(e) => return Err(e.to_string()),
    };
    runner.transition(|s| {
        s.cred_stale(sid);
        Vec::new()
    });
    let from = outcome.from.name().unwrap_or("current subscription");
    let to = outcome.to.as_ref().and_then(|t| t.name());
    let to = to.unwrap_or("next available");
    Ok(Notice {
        text: format!("switch scheduled: {from} → {to} (on the next message)"),
        level: Level::Info,
    })
}
