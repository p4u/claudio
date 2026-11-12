//! The `hosts.*` ops (docs/web-protocol.md §5, "Hosts"), the host names the
//! web takes, and `[ssh] forward_agent` as this server last read it (the
//! Host DTO's `forward_agent`).

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use serde::Deserialize;
use serde_json::{json, Value};

use super::hub::{clean, HubHandle};
use super::ws::{parse, HostArgs, NoArgs};
use claudio_core::remote::hosts as ssh_hosts;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ForwardArgs {
    host: String,
    on: bool,
}

/// One `hosts.*` op.
pub(super) async fn dispatch(hub: &HubHandle, op: &str, args: Value) -> Result<Value, String> {
    match op {
        "hosts.list" => {
            parse::<NoArgs>(args)?;
            Ok(json!({ "hosts": hub.hosts().await }))
        }
        "hosts.candidates" => {
            parse::<NoArgs>(args)?;
            let connected: Vec<String> = hub
                .hosts()
                .await
                .into_iter()
                .filter(|h| h["connected"] == true)
                .filter_map(|h| h["name"].as_str().map(str::to_owned))
                .collect();
            let all = tokio::task::spawn_blocking(ssh_hosts::candidates)
                .await
                .unwrap_or_default();
            let hosts: Vec<String> = all.into_iter().filter(|h| !connected.contains(h)).collect();
            Ok(json!({ "hosts": hosts }))
        }
        "hosts.connect" => {
            let a: HostArgs = parse(args)?;
            hub.connect(a.host).await
        }
        "hosts.disconnect" => {
            let a: HostArgs = parse(args)?;
            hub.disconnect(a.host).await.map(|()| json!({}))
        }
        "hosts.forward_agent" => forward_agent(hub, parse(args)?).await,
        _ => Err("unknown op".to_owned()),
    }
}

/// `hosts.forward_agent`: `[ssh] forward_agent` through the core's one
/// writer. When the file changed, a connected host is reconnected (its
/// sessions keep running) and any other known one gets a `host` event, after
/// the DTO's value is read again.
async fn forward_agent(hub: &HubHandle, a: ForwardArgs) -> Result<Value, String> {
    if a.host == "local" {
        return Err("local has no ssh connection".to_owned());
    }
    host_ok(&a.host)?;
    let (host, on) = (a.host.clone(), a.on);
    let saved = tokio::task::spawn_blocking(move || {
        let changed = ssh_hosts::set_forward_agent(&host, on);
        reload_forward(&claudio_core::paths::config_file());
        changed
    })
    .await
    .map_err(|_| "could not save: the save stopped".to_owned())?;
    if saved.map_err(|e| format!("could not save: {e}"))? {
        hub.forward_agent_changed(a.host).await;
    }
    Ok(json!({}))
}

/// A host name the web may connect to: what `remote::validate_host` takes,
/// at most 255 bytes, no whitespace or control characters.
pub(crate) fn host_ok(host: &str) -> Result<(), String> {
    claudio_core::remote::validate_host(host)?;
    if host.len() > 255 || host.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(format!("invalid host {:?}", clean(host, 80)));
    }
    Ok(())
}

/// `[ssh] forward_agent` as last read: at start, after `hosts.forward_agent`
/// and whenever config.toml is re-read.
static FORWARD: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn forward() -> MutexGuard<'static, Vec<String>> {
    FORWARD.lock().unwrap_or_else(|e| e.into_inner())
}

/// Read `[ssh] forward_agent` of the config file at `path` again (blocking:
/// from `spawn_blocking`).
pub(crate) fn reload_forward(path: &Path) {
    let list = ssh_hosts::forward_agent_in(path);
    *forward() = list;
}

/// The Host DTO's `forward_agent`: never for `local`.
pub(crate) fn forwards_agent(host: &str) -> bool {
    host != "local" && forward().iter().any(|h| h == host)
}
