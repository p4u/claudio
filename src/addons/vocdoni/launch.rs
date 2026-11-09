//! The Vocdoni addon's share of launches (docs/addons.md, "The Vocdoni
//! addon"): a proxy profile is the route (`--proxy NAME`, `--no-proxy`,
//! the configured default) and `--first-party` its switch. A proxied
//! session gets the profile's env, the removal of the inherited API key,
//! and its plugins' share.
//!
//! A profile that is chosen (or the default) is `Routing::Required`: when
//! it cannot be used the launch fails, it never starts direct. An
//! unreachable proxy config only means the built-in model defaults.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::oneshot;
use tokio::time::Instant;

use super::daemon::{Applied, OP_APPLY};
use super::plugins::state::{layer_env, plugins_part, PluginPart};
use super::profiles::{missing, Profiles};
use super::proxy::env::session_env;
use super::runner::{applied, request_body, Runner};
use super::{Vocdoni, NAME};
use crate::addon::{
    AddonCtx, BoxFuture, FlagArg, FlagDecl, Launch, LaunchArg, LaunchChoice, LaunchError, LaunchOp,
    LaunchOption, LaunchPart, LaunchRequest, LaunchSelect, LegacyChoice, Pick, Routing,
};

/// `--proxy NAME`: this run's sessions use profile NAME.
pub const PROXY: &str = "--proxy";
/// `--no-proxy`: this run's sessions start direct.
pub const NO_PROXY: &str = "--no-proxy";
/// `--first-party`: proxied sessions are first-party.
pub const FIRST_PARTY: &str = "--first-party";
/// The switch as a [`LaunchChoice`] flag.
pub const FIRST_PARTY_FLAG: &str = "first-party";

/// The env a proxied session must not inherit: the proxy's token replaces
/// the API key.
const API_KEY: &str = "ANTHROPIC_API_KEY";
/// Inherited gateway mode would turn a first-party session's flags off.
const GATEWAY: &str = "CLAUDE_CODE_USE_GATEWAY";

/// How long a new session waits for a host to answer its plugins'
/// manifests (and, at a front-end's start, for the plugin lists).
pub const MANIFEST_WAIT: Duration = Duration::from_secs(20);

const FLAGS: &[FlagDecl] = &[
    FlagDecl {
        name: PROXY,
        value: Some("NAME"),
        help: "new sessions use proxy profile NAME",
        label: None,
    },
    FlagDecl {
        name: NO_PROXY,
        value: None,
        help: "new sessions start without a proxy",
        label: None,
    },
    FlagDecl {
        name: FIRST_PARTY,
        value: None,
        help: "proxied sessions are first-party (Claude Code's feature flags stay on)",
        label: Some("First-party session"),
    },
];

impl Launch for Vocdoni {
    fn flags(&self) -> &'static [FlagDecl] {
        FLAGS
    }

    /// The flags parsed; a profile given (`--proxy NAME`) is one in use:
    /// its list is fetched from the start.
    fn select(&self, given: &[FlagArg]) -> Result<LaunchSelect, String> {
        let select = select(given)?;
        if let Pick::Option { option, .. } = &select.pick {
            self.runner.state().used.insert(option.clone());
        }
        Ok(select)
    }

    fn options(&self, _ctx: &dyn AddonCtx) -> Vec<LaunchOption> {
        options(&self.runner.profiles())
    }

    fn resolve(&self, select: &LaunchSelect, _ctx: &dyn AddonCtx) -> Result<Routing, String> {
        resolve(select, || self.runner.profiles())
    }

    fn prepare<'a>(
        &'a self,
        req: LaunchRequest,
        ctx: Arc<dyn AddonCtx>,
    ) -> BoxFuture<'a, Result<LaunchPart, LaunchError>> {
        // Only a live runtime prepares: one that has not delivered its
        // first event yet still loads, and a new session waits for it.
        self.runner.activate();
        Box::pin(prepare(Arc::clone(&self.runner), req, ctx))
    }

    fn adopt_legacy(&self, legacy: &LegacyChoice) -> Option<LaunchChoice> {
        let profile = legacy.proxy.clone()?;
        Some(choice(profile, legacy.first_party))
    }

    fn legacy(&self, choice: &LaunchChoice) -> Option<LegacyChoice> {
        (choice.addon == NAME).then(|| LegacyChoice {
            proxy: Some(choice.option.clone()),
            first_party: first_party(&choice.flags),
        })
    }
}

/// This addon's choice of `profile`.
fn choice(profile: String, first_party: bool) -> LaunchChoice {
    LaunchChoice {
        addon: NAME.to_owned(),
        option: profile,
        flags: first_party
            .then(|| FIRST_PARTY_FLAG.to_owned())
            .into_iter()
            .collect(),
    }
}

/// Whether `flags` hold the first-party switch.
fn first_party(flags: &[String]) -> bool {
    flags.iter().any(|f| f == FIRST_PARTY_FLAG)
}

/// The flags as given (in order; the last route wins).
fn select(given: &[FlagArg]) -> Result<LaunchSelect, String> {
    let mut select = LaunchSelect::default();
    for flag in given {
        match (flag.name.as_str(), &flag.value) {
            (PROXY, Some(name)) => {
                select.pick = Pick::Option {
                    addon: NAME.to_owned(),
                    option: name.clone(),
                }
            }
            (PROXY, None) => return Err("--proxy requires a profile name".to_owned()),
            (NO_PROXY, _) => select.pick = Pick::Direct,
            (FIRST_PARTY, _) if !first_party(&select.flags) => {
                select.flags.push(FIRST_PARTY_FLAG.to_owned())
            }
            (FIRST_PARTY, _) => {}
            (other, _) => return Err(format!("{other}: not a flag of {NAME}")),
        }
    }
    Ok(select)
}

/// One option per profile, the default marked.
fn options(p: &Profiles) -> Vec<LaunchOption> {
    p.names
        .iter()
        .map(|name| LaunchOption {
            id: name.clone(),
            label: name.clone(),
            group: "Proxy".to_owned(),
            default: p.default.as_ref() == Some(name),
            detail: p
                .first_party
                .contains(name)
                .then(|| "first-party".to_owned()),
        })
        .collect()
}

/// The default profile (if any) or the chosen one is required; a chosen
/// one that does not exist, or profiles that cannot be read, fail the
/// launch (R20). Direct, and other addons' options, leave this addon out.
/// The profiles are read only when the pick needs them.
fn resolve(select: &LaunchSelect, profiles: impl FnOnce() -> Profiles) -> Result<Routing, String> {
    let read = || match profiles() {
        Profiles { error: Some(e), .. } => Err(e),
        p => Ok(p),
    };
    let profile = match &select.pick {
        Pick::Default => match read()?.default {
            Some(default) => default,
            None => return Ok(Routing::Off),
        },
        Pick::Option { addon, option } if addon == NAME => {
            if !read()?.names.contains(option) {
                return Err(missing(option));
            }
            option.clone()
        }
        Pick::Option { .. } | Pick::Direct => return Ok(Routing::Off),
    };
    Ok(Routing::Required(choice(
        profile,
        first_party(&select.flags),
    )))
}

/// A proxied session's share: the profile's env (its config's models, or
/// the built-in ones), `env_remove`, and the plugins' share.
async fn prepare(
    runner: Arc<Runner>,
    req: LaunchRequest,
    ctx: Arc<dyn AddonCtx>,
) -> Result<LaunchPart, LaunchError> {
    let Routing::Required(choice) = &req.routing else {
        return Ok(LaunchPart::default());
    };
    let name = choice.option.clone();
    let found = runner.lookup_profile(&name).await;
    let Some(mut profile) = found.map_err(LaunchError::Failed)? else {
        return Err(LaunchError::Failed(missing(&name)));
    };
    profile.first_party |= first_party(&choice.flags);
    let deadline = Instant::now() + MANIFEST_WAIT;
    let reviewed = review(&runner, &req, &name, ctx.as_ref(), deadline).await?;
    let (config, mut share) = tokio::join!(
        runner.config(&name),
        plugin_share(&runner, &req, &name, &ctx, deadline)
    );
    share.notices.splice(0..0, reviewed);
    let env = layer_env(&share.env, session_env(&profile, config.as_ref()));
    let mut env_remove = vec![API_KEY.to_owned()];
    if profile.first_party {
        env_remove.push(GATEWAY.to_owned());
    }
    Ok(LaunchPart {
        env,
        env_remove,
        args: share.args,
        lenses: Vec::new(),
        instructions: share.instructions,
        notices: share.notices,
    })
}

/// A new session's review: `profile`'s enabled plugins without saved
/// choices, but those skipped for this launch. Once the addon has its
/// start and the profile's list, the first whose manifest is here needs
/// its checklist reviewed (D8); one whose manifest failed, or is still not
/// here at `deadline`, is left out with a notice. A respawn, a recovery and
/// a host without the addon review nothing.
async fn review(
    runner: &Arc<Runner>,
    req: &LaunchRequest,
    profile: &str,
    ctx: &dyn AddonCtx,
    deadline: Instant,
) -> Result<Vec<String>, LaunchError> {
    let fresh = matches!(req.op, LaunchOp::New | LaunchOp::Resume);
    if !fresh || !ctx.host_has(&req.host) {
        return Ok(Vec::new());
    }
    runner.transition(|s| s.use_profiles([profile.to_owned()].into()));
    wait_listed(runner, profile, deadline).await;
    loop {
        let open = runner.state().unreviewed(profile, &req.deferred);
        if let Some((checklist, revision)) = open.due.into_iter().next() {
            return Err(LaunchError::NeedsReview {
                checklist,
                revision,
            });
        }
        if open.fetching.is_empty() || Instant::now() >= deadline {
            let failed = open.failed.into_iter().map(|(name, why)| {
                let why: String = crate::text::sanitize(&why).chars().take(300).collect();
                format!("plugin {name} left out: manifest: {why}")
            });
            let late = open
                .fetching
                .into_iter()
                .map(|name| format!("plugin {name} left out: its manifest is not here yet"));
            return Ok(failed.chain(late).collect());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The plugins' part of a launch.
#[derive(Default)]
struct Share {
    args: Vec<LaunchArg>,
    env: Vec<(String, String)>,
    instructions: String,
    notices: Vec<String>,
}

/// What `profile`'s enabled plugins bring to a launch on the request's
/// host. A new session waits (until `deadline`) for the addon's start and
/// for the host's answers it lacks; a respawn or a recovery takes what is
/// answered now. A host whose daemon predates addons gets none of it.
async fn plugin_share(
    runner: &Arc<Runner>,
    req: &LaunchRequest,
    profile: &str,
    ctx: &Arc<dyn AddonCtx>,
    deadline: Instant,
) -> Share {
    let fresh = matches!(req.op, LaunchOp::New | LaunchOp::Resume);
    if fresh {
        wait_loaded(runner, deadline).await;
    }
    let host = req.host.as_str();
    let (mut parts, pending) = saved_parts(runner, host, profile, &req.deferred);
    if !ctx.host_has(host) {
        let notices = (!parts.is_empty())
            .then(|| format!("daemon on {host} is older: plugins after `claudio daemon restart`"))
            .into_iter()
            .collect();
        return Share {
            notices,
            ..Share::default()
        };
    }
    let mut notices = Vec::new();
    if fresh && !pending.is_empty() {
        let (mut ready, left_out) = ask(runner, ctx, host, pending, deadline).await;
        if !left_out.is_empty() {
            let names = left_out.join(", ");
            notices.push(format!("plugin {names} left out: not ready on {host}"));
        }
        fill(&mut parts, &mut ready);
    }
    let composed = plugins_part(&parts);
    notices.extend(composed.notices);
    let args = composed
        .args
        .chunks_exact(2)
        .map(|pair| LaunchArg::Optional {
            flag: pair[0].clone(),
            value: pair[1].clone(),
        })
        .collect();
    Share {
        args,
        env: composed.env,
        instructions: composed.instructions,
        notices,
    }
}

/// `profile`'s enabled plugins on `host` minus `skip`, and the asks they
/// wait for: a plugin without saved choices is left out whole, its
/// secrets too (its review was skipped, or failed). A launch's profile is
/// one in use (its list is fetched).
fn saved_parts(
    runner: &Arc<Runner>,
    host: &str,
    profile: &str,
    skip: &[String],
) -> (Vec<PluginPart>, Vec<(String, super::daemon::ApplyRequest)>) {
    runner.transition(|s| s.use_profiles([profile.to_owned()].into()));
    let state = runner.state();
    let (mut parts, pending) = state.plugin_parts(host, profile, skip);
    parts.retain(|p| state.plugins.choices.contains_key(&p.name));
    (parts, pending)
}

/// The answers that came in, into the parts that lacked one.
fn fill(parts: &mut [PluginPart], ready: &mut HashMap<String, Applied>) {
    for part in parts {
        if part.applied.is_none() {
            part.applied = ready.remove(&part.name);
        }
    }
}

/// Until the addon has read its lists and choices (a launch at a
/// front-end's start), or `deadline`. An inactive addon reads none.
async fn wait_loaded(runner: &Runner, deadline: Instant) {
    while runner.is_active() && !runner.state().loaded && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Until the addon has its start and `profile`'s first list fetch of the
/// run ended, or `deadline`.
async fn wait_listed(runner: &Runner, profile: &str, deadline: Instant) {
    wait_loaded(runner, deadline).await;
    let listed = || runner.state().listed.contains(profile);
    while runner.is_active() && !listed() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Ask `host` for each of `pending` and wait until `deadline`: the answers
/// that came back, and the names of those that did not. Every answer, a
/// late one too, goes to the addon's state.
async fn ask(
    runner: &Arc<Runner>,
    ctx: &Arc<dyn AddonCtx>,
    host: &str,
    pending: Vec<(String, super::daemon::ApplyRequest)>,
    deadline: Instant,
) -> (HashMap<String, Applied>, Vec<String>) {
    let mut waiting = Vec::new();
    for (name, body) in pending {
        let (done, answer) = oneshot::channel();
        let (runner, inner, host) = (Arc::clone(runner), Arc::clone(ctx), host.to_owned());
        let plugin = name.clone();
        ctx.spawn(Box::pin(async move {
            let sha256 = body.sha256.clone();
            let result = match request_body(&body) {
                Ok(body) => inner.request(&host, OP_APPLY, body).await.and_then(applied),
                Err(e) => Err(e),
            };
            let ready = result.clone().ok();
            runner.transition(|s| s.on_applied(&host, &plugin, &sha256, result));
            let _ = done.send(ready);
        }));
        waiting.push((name, answer));
    }
    let mut ready = HashMap::new();
    let mut left_out = Vec::new();
    for (name, answer) in waiting {
        match tokio::time::timeout_at(deadline, answer).await {
            Ok(Ok(Some(a))) => {
                ready.insert(name, a);
            }
            _ => left_out.push(name),
        }
    }
    (ready, left_out)
}

#[cfg(test)]
mod tests;
