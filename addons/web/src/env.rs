//! The custom env ops (`env.*`, docs/web-protocol.md §5 "Custom env") on
//! `app::env`, whose files are beside the addon runtime's config.toml (the
//! TUI's on this machine). `env.list` is the one reply that carries env
//! values (§4's exception, an owner decision); errors and the `web op` log
//! name keys only. The hub drops a session's entry when it is removed, and
//! prunes a host's entries when the host lists its sessions.

use serde::Deserialize;
use serde_json::{json, Value as Json};

use super::ws::{parse, NoArgs};
use super::AppState;
use claudio_core::app::addons::AddonRuntime;
use claudio_core::app::env::{Listed, Scope, Store};
use claudio_core::claude::env::{catalogue, refused_names, Value};
use claudio_core::proto::{SessionId, SessionKind};

/// The `scope` arg.
#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum ScopeArg {
    Global,
    Session,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListArgs {
    scope: ScopeArg,
    #[serde(default)]
    sid: Option<SessionId>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetArgs {
    scope: ScopeArg,
    #[serde(default)]
    sid: Option<SessionId>,
    name: String,
    value: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UnsetArgs {
    scope: ScopeArg,
    #[serde(default)]
    sid: Option<SessionId>,
    name: String,
}

/// One `env.*` op.
pub(crate) async fn dispatch(state: &AppState, op: &str, args: Json) -> Result<Json, String> {
    let store = Store::beside(state.addons.config_path());
    match op {
        "env.catalogue" => {
            parse::<NoArgs>(args)?;
            Ok(catalogue_json())
        }
        "env.list" => {
            let a: ListArgs = parse(args)?;
            let scope = scope_of(state, a.scope, a.sid).await?;
            let listed = blocking(move || store.list(&scope)).await?;
            Ok(list_json(listed))
        }
        "env.set" => {
            let a: SetArgs = parse(args)?;
            let scope = scope_of(state, a.scope, a.sid).await?;
            let (kept, name) = (scope.clone(), a.name.clone());
            blocking(move || store.set(&kept, &name, &a.value))
                .await?
                .map_err(|e| e.to_string())?;
            log("env.set", &scope, &a.name);
            Ok(json!({}))
        }
        "env.unset" => {
            let a: UnsetArgs = parse(args)?;
            let scope = scope_of(state, a.scope, a.sid).await?;
            let (kept, name) = (scope.clone(), a.name.clone());
            blocking(move || store.unset(&kept, &name))
                .await?
                .map_err(|e| e.to_string())?;
            log("env.unset", &scope, &a.name);
            Ok(json!({}))
        }
        _ => Err("unknown op".to_owned()),
    }
}

/// `scope` and `sid` as a scope: `sid` only with `session`, naming a claude
/// session the hub knows (its host is the hub's).
async fn scope_of(
    state: &AppState,
    scope: ScopeArg,
    sid: Option<SessionId>,
) -> Result<Scope, String> {
    match (scope, sid) {
        (ScopeArg::Global, None) => Ok(Scope::Global),
        (ScopeArg::Global, Some(_)) => Err("bad request: sid: not with scope global".to_owned()),
        (ScopeArg::Session, None) => {
            Err("bad request: sid: required with scope session".to_owned())
        }
        (ScopeArg::Session, Some(sid)) => {
            let session = state.hub.session(sid).await?;
            if session.info.kind != SessionKind::Claude {
                return Err("not a claude session".to_owned());
            }
            Ok(Scope::Session {
                host: session.host,
                sid,
            })
        }
    }
}

/// `work` off the executor (the files' lock waits up to 5 s).
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| "the env store stopped".to_owned())
}

/// A write, at INFO: the op, the scope, the sid and the name, never the value.
fn log(op: &str, scope: &Scope, name: &str) {
    match scope {
        Scope::Global => tracing::info!(op, scope = "global", name, "web op"),
        Scope::Session { host, sid } => {
            tracing::info!(op, scope = "session", %host, %sid, name, "web op")
        }
    }
}

/// `{"vars":[EnvVar],"refused":[EnvRefused]}`.
fn catalogue_json() -> Json {
    let vars: Vec<Json> = catalogue()
        .iter()
        .map(|v| json!({"name": v.name, "category": v.category, "description": v.description}))
        .collect();
    let refused: Vec<Json> = refused_names()
        .into_iter()
        .map(|(name, reason)| json!({"name": name, "reason": reason}))
        .collect();
    json!({ "vars": vars, "refused": refused })
}

/// `{"vars":[EnvEntry],"skipped": n}`: the values, by owner decision.
fn list_json(listed: Listed) -> Json {
    let vars: Vec<Json> = listed
        .vars
        .into_iter()
        .map(|(name, value)| json!({"name": name, "value": value.into_string()}))
        .collect();
    json!({ "vars": vars, "skipped": listed.skipped })
}

/// Drop session `sid`'s custom env (killed or removed), off the executor; a
/// failure is not shown.
pub(crate) fn forget(runtime: &AddonRuntime, host: &str, sid: SessionId) {
    let (store, host) = (Store::beside(runtime.config_path()), host.to_owned());
    tokio::task::spawn_blocking(move || {
        let _ = store.forget_session(&host, sid);
    });
}

/// Drop the custom env of `host`'s sessions not in `live` (its daemon's
/// list), off the executor; a failure is not shown.
pub(crate) fn prune(runtime: &AddonRuntime, host: &str, live: Vec<SessionId>) {
    let (store, host) = (Store::beside(runtime.config_path()), host.to_owned());
    tokio::task::spawn_blocking(move || {
        let _ = store.prune(&host, &live);
    });
}
