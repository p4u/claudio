//! The TUI's edge of the addon runtime (`app::addons`): it is built in
//! `main` and fed from the event loop: host connections and losses, the
//! session list and focus as `App` holds them, claude installs, and what
//! the runtime tells the UI. `App` itself never sees the runtime (A6).

use std::io;
use std::sync::Arc;

use tokio::sync::mpsc;

use super::app::{App, ReplyTo};
use super::HostEvent;
use crate::addon::{Addons, SessionSnap};
use crate::app::addons::{AddonRuntime, RuntimeOptions, RuntimeOut, RuntimeRx};
use crate::app::connections::Connections;
use crate::client::Incoming;
use crate::paths;
use crate::proto::Msg;

/// The runtime and what it tells the UI, as the event loop holds them.
pub(super) struct Runner {
    pub(super) runtime: AddonRuntime,
    pub(super) rx: RuntimeRx,
}

/// The runtime for `addons`, reading the TUI's config.toml. The TUI does
/// not watch the file (D15).
pub(super) fn start(addons: Arc<Addons>) -> Runner {
    let opts = RuntimeOptions {
        config_path: paths::config_file(),
        watch_config: false,
    };
    let (runtime, rx) = AddonRuntime::start(addons, opts);
    Runner { runtime, rx }
}

/// `host`'s daemon answers (through its client in `conns`).
pub(super) fn host_up(runtime: &AddonRuntime, host: &str, conns: &Connections) {
    if let Some(client) = conns.client(host) {
        runtime.host_up(host, client.clone());
    }
}

/// An item from `host`'s daemon for `App`; a lost connection goes to the
/// runtime too.
pub(super) fn on_incoming(app: &mut App, runtime: &AddonRuntime, host: &str, inc: Incoming) {
    if matches!(inc, Incoming::Disconnected) {
        runtime.host_down(host);
    }
    app.on_incoming_from(host, inc);
}

/// A daemon's reply for `App`; a claude install that succeeded goes to the
/// runtime too.
pub(super) fn on_reply(app: &mut App, runtime: &AddonRuntime, to: ReplyTo, reply: io::Result<Msg>) {
    let installed = match (&to, &reply) {
        (
            ReplyTo::ClaudeUpdate(host),
            Ok(Msg::ClaudeUpdated {
                ok: true,
                version: Some(_),
                ..
            }),
        ) => app.claude_installing.contains(host).then(|| host.clone()),
        _ => None,
    };
    app.on_reply(to, reply);
    if let Some(host) = installed {
        runtime.claude_installed(&host);
    }
}

/// The sessions as `App` has them now, its active one focused. The runtime
/// sends events only for what changed.
pub(super) fn sync(runtime: &AddonRuntime, app: &App) {
    let active = app.active_view().map(|v| v.id);
    let snaps = app
        .sessions
        .iter()
        .map(|v| SessionSnap {
            sid: v.id,
            host: v.host.clone(),
            kind: v.kind,
            cwd: v.cwd.clone(),
            label: v
                .name
                .clone()
                .or_else(|| v.title.clone())
                .unwrap_or_else(|| v.cwd.clone()),
            conversation: v.claude_session_id.clone(),
            launch: None,
            focused: Some(v.id) == active,
        })
        .collect();
    runtime.sessions_reset(snaps);
}

/// What the runtime tells the UI: notices are shown; the rest has no
/// reader in the TUI yet.
pub(super) fn on_out(app: &mut App, out: RuntimeOut) {
    if let RuntimeOut::Notice { addon, notice } = out {
        app.notify(format!("{addon}: {}", notice.text));
    }
}

/// Set `[extensions] name = on` in config.toml off the UI thread; the
/// outcome comes back as a notice.
pub(super) fn save_extension(name: String, on: bool, tx: &mpsc::Sender<HostEvent>) {
    let tx = tx.clone();
    tokio::spawn(async move {
        let state = if on { "on" } else { "off" };
        let path = paths::config_file();
        let own = name.clone();
        let saved =
            tokio::task::spawn_blocking(move || crate::config::save_extension(&path, &own, on))
                .await
                .map_err(|_| "the save stopped".to_owned());
        let text = match saved {
            Ok(Ok(())) => format!("extension {name} {state} (saved)"),
            Ok(Err(e)) => format!("could not save: {e}"),
            Err(e) => format!("could not save: {e}"),
        };
        let _ = tx.send(HostEvent::Notice(text)).await;
    });
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::addon::test_support::TestAddon;
    use crate::addon::Addon;
    use crate::tui::test_support::{app_with, info};

    /// The events `addon` saw, ticks left out, once there are `n`.
    async fn seen(addon: &TestAddon, n: usize) -> Vec<String> {
        let wait = async {
            loop {
                let got: Vec<String> = addon
                    .calls()
                    .into_iter()
                    .filter(|c| c.starts_with("event ") && c != "event Tick")
                    .collect();
                if got.len() >= n {
                    return got;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        };
        tokio::time::timeout(Duration::from_secs(5), wait)
            .await
            .expect("events in time")
    }

    #[tokio::test]
    async fn the_runtime_follows_the_apps_sessions_and_claude_installs() {
        let echo = Arc::new(TestAddon::new("echo"));
        let addons = Arc::new(Addons::new(vec![Arc::clone(&echo) as Arc<dyn Addon>]).unwrap());
        let opts = RuntimeOptions {
            config_path: "/nonexistent/claudio/config.toml".into(),
            watch_config: false,
        };
        let (runtime, _rx) = AddonRuntime::start(addons, opts);
        let mut app = app_with(&[info(Some(7), None)]);
        let sid = app.sessions[0].id;
        sync(&runtime, &app);
        sync(&runtime, &app); // nothing changed: no event
        let got = seen(&echo, 2).await;
        assert_eq!(
            got[1],
            format!("event SessionAdded {{ sid: {sid}, conversation: None }}")
        );
        // A claude install that succeeded reaches the runtime; a failed one not.
        app.claude_installing.insert("local".into());
        let ok = Msg::ClaudeUpdated {
            version: Some("2.0.0".into()),
            ok: true,
            tail: String::new(),
        };
        on_reply(
            &mut app,
            &runtime,
            ReplyTo::ClaudeUpdate("local".into()),
            Ok(ok),
        );
        assert_eq!(
            seen(&echo, 3).await[2],
            "event ClaudeInstalled { host: \"local\" }"
        );
        app.sessions.clear();
        sync(&runtime, &app);
        assert_eq!(
            seen(&echo, 4).await[3],
            format!("event SessionRemoved {{ sid: {sid} }}")
        );
        runtime.shutdown();
    }
}
