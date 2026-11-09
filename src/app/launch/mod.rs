//! The launch composer (docs/addons.md, "Launches"): resolve → prepare →
//! review → compose, the one path a claude session's `Spawn` or `Respawn`
//! takes from the TUI, `--plain` and the web. A terminal launch skips it.
//!
//! The addons' parts hold the session's secrets (their env). They stay in
//! this module's futures and in [`Parked`] until the composed message goes
//! over the daemon socket: never into a front-end's state, a log, a
//! notice or `Debug` (C1).

mod compose;
mod review;
#[cfg(test)]
mod tests;

use std::io;
use std::time::Duration;

use futures::future::join_all;
use tokio::time::Instant;

use crate::addon::{
    line, Addons, FlagArg, Frontend, LaunchChoice, LaunchError, LaunchOp, LaunchPart,
    LaunchRequest, LaunchSelect, LegacyChoice, Pick, Routing, TEXT_BYTES,
};
use crate::app::addons::AddonRuntime;
use crate::client::Client;
use crate::proto::{Msg, SessionExtensions, SessionId};

pub use compose::compose;
pub use review::{resume, Parked, ReviewAnswer};

/// The prepare budget of one launch (5 s proxy config + 20 s manifests,
/// before addons). The clock stops while a review is open.
pub const BUDGET: Duration = Duration::from_secs(25);

/// How long a launch waits for the addons' start before it resolves
/// ([`AddonRuntime::started`]); past it, it resolves on what they have.
pub const START_WAIT: Duration = Duration::from_secs(10);

/// The most bytes of an addon name quoted from a member's choice.
const NAME_BYTES: usize = 64;

/// What a front-end asks for. `base` is a `Spawn` or `Respawn` with no
/// env, env_remove, optional or extensions; `core_lenses` the core
/// registry's lenses (external extensions, and until P3.18 the built-ins).
pub struct LaunchSpec {
    /// New, resume, respawn or recover.
    pub op: LaunchOp,
    /// The session's id.
    pub sid: SessionId,
    /// The host it runs on.
    pub host: String,
    /// Its working directory.
    pub cwd: String,
    /// What the member asked for.
    pub select: LaunchSelect,
    /// The front-end launching it.
    pub frontend: Frontend,
    /// The message to compose into.
    pub base: Msg,
    /// The core's share of the session's extensions.
    pub core_lenses: SessionExtensions,
}

/// Every Launch addon's routing, in registry order.
pub type Routings = Vec<(&'static str, Routing)>;

/// Where a launch got to.
// large_enum_variant: a step is moved a few times per launch; boxing the
// parked launch would only add an allocation.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum Step {
    /// Sent; `reply` is the daemon's answer.
    Sent {
        /// The launch's effective choice (`None`: direct), to persist.
        choice: Option<LaunchChoice>,
        /// For the member: the addons' notices and what was left out.
        notices: Vec<String>,
        reply: io::Result<Msg>,
    },
    /// Stopped until the member answers `addon`'s checklist
    /// ([`resume`]); the budget's clock is stopped meanwhile.
    Review {
        addon: &'static str,
        checklist: String,
        revision: String,
        parked: Parked,
    },
    /// Not sent, and why.
    Failed(String),
}

/// Leading launch flags of any addon in `args` (in any order) → the
/// select, and the arguments after them. Each addon's `select` parses its
/// own; two addons picking different routes is an error.
pub fn split_flags<'a>(
    addons: &Addons,
    args: &'a [String],
) -> Result<(LaunchSelect, &'a [String]), String> {
    let decls = addons.launch_flags();
    let mut given: Vec<(&'static str, FlagArg)> = Vec::new();
    let mut rest = args;
    while let Some((first, tail)) = rest.split_first() {
        let Some((addon, decl)) = decls.iter().find(|(_, d)| d.name == first.as_str()) else {
            break;
        };
        rest = tail;
        let value = match decl.value {
            None => None,
            Some(placeholder) => {
                let Some((value, tail)) = rest.split_first() else {
                    return Err(format!("{} requires {placeholder}", decl.name));
                };
                rest = tail;
                Some(value.clone())
            }
        };
        let name = decl.name.to_owned();
        given.push((addon, FlagArg { name, value }));
    }
    let mut select = LaunchSelect::default();
    for addon in addons.iter() {
        let mine: Vec<FlagArg> = given
            .iter()
            .filter(|(a, _)| *a == addon.name())
            .map(|(_, f)| f.clone())
            .collect();
        let Some(launch) = addon.launch().filter(|_| !mine.is_empty()) else {
            continue;
        };
        let theirs = launch.select(&mine)?;
        if theirs.pick != Pick::Default {
            if select.pick != Pick::Default && select.pick != theirs.pick {
                return Err("launch flags pick two different routes".to_owned());
            }
            select.pick = theirs.pick;
        }
        select.flags.extend(theirs.flags);
    }
    Ok((select, rest))
}

/// A session's pre-addon routing (state.json's `proxy`, `first_party`) as
/// a launch choice: `Ok(None)` for a direct one; `Err` when no addon of
/// this build adopts it, and then its launches fail, never direct (F4).
pub fn adopt_legacy(
    addons: &Addons,
    legacy: &LegacyChoice,
) -> Result<Option<LaunchChoice>, String> {
    let Some(proxy) = &legacy.proxy else {
        return Ok(None);
    };
    let adopted = addons
        .iter()
        .find_map(|a| a.launch().and_then(|l| l.adopt_legacy(legacy)));
    match adopted {
        Some(choice) => Ok(Some(choice)),
        None => Err(format!(
            "saved proxy {}: no addon of this build routes it",
            line(proxy, NAME_BYTES)
        )),
    }
}

/// The pre-addon fields still written for `choice` (B3: a rolled-back
/// build keeps the session's routing); empty for a direct one.
pub fn legacy(addons: &Addons, choice: Option<&LaunchChoice>) -> LegacyChoice {
    choice
        .and_then(|c| addons.get(&c.addon)?.launch()?.legacy(c))
        .unwrap_or_default()
}

/// Until the addons have started, at most [`START_WAIT`]: `resolve` reads
/// their state without I/O, and a launch at a front-end's start (a
/// `--plain` session, the web's first spawn) must not resolve before it.
pub async fn addons_started(rt: &AddonRuntime) {
    let _ = tokio::time::timeout(START_WAIT, rt.started()).await;
}

/// Every Launch addon's routing, in registry order, and the launch's
/// effective choice (the Required addon's). Errors: a `Pick::Option` of no
/// registered Launch addon, or one that addon did not take; an addon's
/// `Err`; two `Required`. None of them ever becomes a direct launch.
pub fn resolve(
    rt: &AddonRuntime,
    select: &LaunchSelect,
) -> Result<(Option<LaunchChoice>, Routings), String> {
    let addons = rt.addons();
    if let Pick::Option { addon, .. } = &select.pick {
        if addons.get(addon).is_none_or(|a| a.launch().is_none()) {
            let addon = line(addon, NAME_BYTES);
            return Err(format!(
                "launch option of `{addon}` is not available in this build"
            ));
        }
    }
    let mut choice: Option<LaunchChoice> = None;
    let mut routings = Vec::new();
    for addon in addons.iter() {
        let Some(launch) = addon.launch() else {
            continue;
        };
        let name = addon.name();
        let ctx = rt.ctx(name);
        let routing = launch
            .resolve(select, ctx.as_ref())
            .map_err(|e| format!("{name}: {}", line(&e, TEXT_BYTES)))?;
        if let Routing::Required(c) = &routing {
            if c.addon != name {
                return Err(format!("{name}: routed a launch as another addon"));
            }
            if let Some(first) = &choice {
                return Err(format!(
                    "two addons route this launch: {}, {name}",
                    first.addon
                ));
            }
            choice = Some(c.clone());
        }
        routings.push((name, routing));
    }
    if let Pick::Option { addon, option } = &select.pick {
        if choice.as_ref().map(|c| c.addon.as_str()) != Some(addon.as_str()) {
            let option = line(option, NAME_BYTES);
            return Err(format!("{addon}: launch option {option} was not taken"));
        }
    }
    Ok((choice, routings))
}

/// Resolve, prepare every non-Off addon concurrently within [`BUDGET`],
/// compose, check the frame and send it through `client`.
pub async fn run(rt: &AddonRuntime, spec: LaunchSpec, client: Client) -> Step {
    run_within(rt, spec, client, BUDGET).await
}

/// [`run`] with checklists skipped for this launch from the start:
/// `(addon, checklist)` pairs (the web's v1 `skip_plugins`); a pair of an
/// addon this build does not register is ignored.
pub async fn run_deferring(
    rt: &AddonRuntime,
    spec: LaunchSpec,
    client: Client,
    deferred: &[(String, String)],
) -> Step {
    start(rt, spec, client, BUDGET, deferred).await
}

/// [`run`] with another budget (tests).
pub(crate) async fn run_within(
    rt: &AddonRuntime,
    spec: LaunchSpec,
    client: Client,
    budget: Duration,
) -> Step {
    start(rt, spec, client, budget, &[]).await
}

/// Resolve, then prepare from `deferred` on.
async fn start(
    rt: &AddonRuntime,
    spec: LaunchSpec,
    client: Client,
    budget: Duration,
    deferred: &[(String, String)],
) -> Step {
    if !matches!(spec.base, Msg::Spawn(_) | Msg::Respawn(_)) {
        return Step::Failed("only a claude spawn or respawn is launched".to_owned());
    }
    addons_started(rt).await;
    let (choice, routings) = match resolve(rt, &spec.select) {
        Ok(resolved) => resolved,
        Err(e) => return Step::Failed(e),
    };
    let todo = routings
        .iter()
        .filter(|(_, r)| *r != Routing::Off)
        .map(|(a, _)| *a)
        .collect();
    let mut launch = Parked::new(spec, choice, routings, todo, budget);
    for (addon, checklist) in deferred {
        if let Some(addon) = rt.addons().get(addon) {
            launch.defer(addon.name(), checklist.clone());
        }
    }
    proceed(rt, launch, client).await
}

/// Prepare `launch`'s remaining addons until every one has its part (then
/// compose and send), one needs a review (park), or a Required one fails.
pub(super) async fn proceed(rt: &AddonRuntime, mut launch: Parked, client: Client) -> Step {
    loop {
        let deadline = Instant::now() + launch.remaining;
        let todo = std::mem::take(&mut launch.todo);
        let results = prepare_all(rt, &launch, &todo, deadline).await;
        launch.remaining = deadline.saturating_duration_since(Instant::now());
        let mut review = None;
        for (addon, result) in results {
            match result {
                Ok(part) => launch.parts.push((addon, part)),
                Err(LaunchError::Failed(why)) => {
                    if let Err(failed) = launch.failed(addon, &why) {
                        return Step::Failed(failed);
                    }
                }
                Err(LaunchError::NeedsReview {
                    checklist,
                    revision,
                }) => {
                    if launch.deferred(addon).contains(&checklist) {
                        let why = "asked for a review it was told to skip";
                        if let Err(failed) = launch.failed(addon, why) {
                            return Step::Failed(failed);
                        }
                    } else if launch.never_waits() {
                        launch.defer(addon, checklist);
                        launch.todo.push(addon);
                    } else {
                        launch.todo.push(addon);
                        review.get_or_insert((addon, checklist, revision));
                    }
                }
            }
        }
        if let Some((addon, checklist, revision)) = review {
            launch.waiting = Some((addon, checklist.clone()));
            return Step::Review {
                addon,
                checklist: line(&checklist, NAME_BYTES),
                revision: line(&revision, NAME_BYTES),
                parked: launch,
            };
        }
        if launch.todo.is_empty() {
            return send(launch, client).await;
        }
    }
}

/// Each of `todo`'s `prepare`, concurrently, until `deadline` (a late one
/// has timed out); in `todo`'s order.
async fn prepare_all(
    rt: &AddonRuntime,
    launch: &Parked,
    todo: &[&'static str],
    deadline: Instant,
) -> Vec<(&'static str, Result<LaunchPart, LaunchError>)> {
    let addons: Vec<_> = todo
        .iter()
        .filter_map(|name| rt.addons().get(name).map(|a| (*name, a.clone())))
        .collect();
    let calls = addons.iter().map(|(name, addon)| async move {
        let Some(launch_cap) = addon.launch() else {
            return (*name, Err(LaunchError::Failed("no launch".to_owned())));
        };
        let req = launch.request(name);
        let prepared = launch_cap.prepare(req, rt.ctx(name));
        match tokio::time::timeout_at(deadline, prepared).await {
            Ok(result) => (*name, result),
            Err(_) => (*name, Err(LaunchError::Failed("timed out".to_owned()))),
        }
    });
    join_all(calls).await
}

/// Compose `launch`'s parts and send the message.
async fn send(mut launch: Parked, client: Client) -> Step {
    launch.sort_parts();
    let Parked {
        spec,
        choice,
        parts,
        mut notices,
        ..
    } = launch;
    let (msg, more) = match compose(spec.base, spec.core_lenses, parts) {
        Ok(composed) => composed,
        Err(e) => return Step::Failed(e),
    };
    notices.extend(more);
    let reply = client.request(msg).await;
    Step::Sent {
        choice,
        notices,
        reply,
    }
}

/// The request `addon` prepares for.
fn request(spec: &LaunchSpec, choice: Option<LaunchChoice>, routing: Routing) -> LaunchRequest {
    LaunchRequest {
        op: spec.op,
        sid: spec.sid,
        host: spec.host.clone(),
        cwd: spec.cwd.clone(),
        choice,
        routing,
        frontend: spec.frontend,
        deferred: Vec::new(),
    }
}
