//! The TUI's proxy glue: what `App` does with the proxy's answers, and how
//! a session with a proxy profile is sent. The Vocdoni addon takes it over
//! (core-addons plan, P3.5b-P3.12).

use std::time::Instant;

use super::app::{App, Effect, Modal, PendingManifest, ReplyTo};
use super::proxy_state::{ProxyFetch, SessionCred};
use super::sessions::SessionView;
use super::HostEvent;
use crate::plugins::state::{compose, with_plugins, PluginPart};
use crate::proto::{Msg, SessionId};
use crate::proxy::api::{ConfigResponse, SessionCredential, SwitchError, SwitchOutcome};

impl App {
    /// A proxy answer from a background fetch: config, stats, a session's
    /// credential or a subscription switch. Other events are not the proxy's.
    pub(super) fn on_proxy_event(&mut self, ev: HostEvent) {
        match ev {
            HostEvent::ProxyConfig {
                profile_name,
                config: Some(cfg),
            } => self.on_proxy_config(profile_name, cfg),
            HostEvent::ProxyStats {
                profile_name,
                fetch,
            } => self.on_proxy_stats(profile_name, fetch),
            HostEvent::SessionCredential {
                session,
                claude_session_id,
                result,
            } => self.on_session_credential(session, &claude_session_id, result),
            HostEvent::SubscriptionSwitch { session, result } => {
                self.on_switch_result(session, result)
            }
            _ => {}
        }
    }

    /// Called when a `FetchProxyConfig` effect completes.
    pub fn on_proxy_config(&mut self, profile_name: String, cfg: ConfigResponse) {
        self.proxy_config
            .insert(profile_name, (cfg, Instant::now()));
        self.redraw = true;
    }

    /// Called when a `FetchProxyStats` effect completes.
    pub fn on_proxy_stats(&mut self, profile_name: String, fetch: ProxyFetch) {
        if let Some(Modal::ProxyStats(view)) = &mut self.modal {
            if view.profile.as_deref() == Some(profile_name.as_str()) {
                view.on_fetched();
            }
        }
        let status = self.proxy_status.entry(profile_name.clone()).or_default();
        status.apply(fetch);
        // A failed catalogue fetch is asked again on the next look.
        if status.models.is_none() {
            self.models_requested.remove(&profile_name);
        }
        self.redraw = true;
    }

    /// Look up the credential of the active session when it uses a proxy and
    /// has a claude session id. A changed claude session id always fetches;
    /// otherwise a lookup newer than `min_age` seconds is reused.
    pub(super) fn refresh_session_cred(&mut self, min_age: u64) {
        let Some(v) = self.active_view() else { return };
        let (Some(profile), Some(csid)) = (v.proxy.clone(), v.claude_session_id.clone()) else {
            return;
        };
        if v.kind != crate::proto::SessionKind::Claude || csid.is_empty() {
            return;
        }
        let id = v.id;
        let now = self.now;
        self.request_models_once(&profile);
        let fresh = self.session_creds.get(&id).is_some_and(|c| {
            c.claude_session_id == csid && now.saturating_sub(c.asked_at) < min_age
        });
        if fresh {
            return;
        }
        let cred = match self.session_creds.remove(&id) {
            Some(old) if old.claude_session_id == csid => old.cred,
            _ => None,
        };
        self.session_creds.insert(
            id,
            SessionCred {
                claude_session_id: csid.clone(),
                cred,
                asked_at: now,
            },
        );
        self.emit(Effect::FetchSessionCredential {
            profile_name: profile,
            session: id,
            claude_session_id: csid,
        });
    }

    /// Called when a `FetchSessionCredential` effect completes. `result` is
    /// `Ok(None)` when the proxy does not know the conversation (yet); an
    /// error (rate limit, network) keeps what is cached.
    pub fn on_session_credential(
        &mut self,
        session: SessionId,
        claude_session_id: &str,
        result: Result<Option<SessionCredential>, String>,
    ) {
        let Some(entry) = self.session_creds.get_mut(&session) else {
            return;
        };
        // A reply for a conversation the session has since left.
        if entry.claude_session_id != claude_session_id {
            return;
        }
        if let Ok(cred) = result {
            entry.cred = cred;
            self.redraw = true;
        }
    }

    /// Fetch a profile's model catalogue (no stats windows) the first time a
    /// session on it is looked at, so the stats popup's Models page has it.
    fn request_models_once(&mut self, profile: &str) {
        let cached = self
            .proxy_status
            .get(profile)
            .is_some_and(|s| s.models.is_some());
        if cached || !self.models_requested.insert(profile.to_owned()) {
            return;
        }
        self.schedule_proxy_stats(profile, Vec::new());
    }

    /// The credential to show for a session: cached, and for its current
    /// claude session id.
    pub fn session_credential(&self, v: &SessionView) -> Option<&SessionCredential> {
        let entry = self.session_creds.get(&v.id)?;
        if v.claude_session_id.as_deref() != Some(entry.claude_session_id.as_str()) {
            return None;
        }
        entry.cred.as_ref()
    }

    /// Proxy config for a profile, if cached and fresh (5 min).
    pub fn proxy_config_cached(&self, name: &str) -> Option<&ConfigResponse> {
        self.proxy_config.get(name).and_then(|(cfg, when)| {
            if when.elapsed().as_secs() < 300 {
                Some(cfg)
            } else {
                None
            }
        })
    }

    /// Schedule a proxy config fetch if the cache is stale.
    pub fn maybe_fetch_proxy_config(&mut self, name: &str) {
        if self.proxy_config_cached(name).is_none() {
            self.emit(Effect::FetchProxyConfig {
                profile_name: name.to_owned(),
            });
        }
    }

    /// Called when a `SwitchSubscription` effect completes: say what the
    /// proxy answered, and look the credential up again so the status bar
    /// follows once the switch happens.
    pub fn on_switch_result(
        &mut self,
        session: SessionId,
        result: Result<SwitchOutcome, SwitchError>,
    ) {
        match result {
            Ok(o) => {
                let from = o.from.name().unwrap_or("current subscription");
                let to =
                    o.to.as_ref()
                        .and_then(|t| t.name())
                        .unwrap_or("next available");
                self.notify(format!(
                    "switch scheduled: {from} → {to} (on the next message)"
                ));
                let active = self.active_view().map(|v| v.id) == Some(session);
                if active {
                    self.refresh_session_cred(0);
                }
            }
            Err(SwitchError::Api(e)) => self.notify(format!("switch failed: {e}")),
            Err(e) => self.notify(e.to_string()),
        }
    }

    /// Build the `SpawnSpec.env` for a proxy profile name. Fails when a
    /// profile is selected but cannot be resolved.
    pub(super) fn proxy_env_for(
        &self,
        proxy_name: Option<&str>,
        first_party: bool,
    ) -> Result<Vec<(String, String)>, String> {
        crate::proxy::resolve::proxy_env_for(
            proxy_name,
            proxy_name.and_then(|n| self.proxy_config_cached(n)),
            first_party,
        )
    }

    /// Send `msg` (a `Spawn` or `Respawn` without the plugins' share or any
    /// env) to `host` with the share of `plugins` ([`compose`]) and the env
    /// of `proxy` added. A cold proxy-config cache, or `pending` manifests
    /// still to come, go through the effect runner, which fetches the
    /// config and waits for them first and answers with
    /// [`ReplyTo::deferred`]. Fails when the profile cannot be resolved.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn send_with_proxy(
        &mut self,
        host: &str,
        msg: Msg,
        proxy: Option<&str>,
        first_party: bool,
        to: ReplyTo,
        plugins: Vec<PluginPart>,
        pending: Vec<PendingManifest>,
    ) -> Result<(), String> {
        match proxy {
            Some(name) if self.proxy_config_cached(name).is_none() || !pending.is_empty() => {
                if !pending.is_empty() {
                    let names: Vec<&str> = pending.iter().map(|b| b.name.as_str()).collect();
                    self.notify(format!("applying plugin {}…", names.join(", ")));
                }
                self.emit(Effect::SpawnWithProxy {
                    host: host.to_owned(),
                    msg,
                    proxy_name: name.to_owned(),
                    first_party,
                    to: to.deferred(),
                    plugins,
                    pending,
                });
            }
            _ => {
                let profile_env = self.proxy_env_for(proxy, first_party)?;
                let composed = compose(&plugins);
                for notice in &composed.notices {
                    self.notify(notice.clone());
                }
                let msg = with_plugins(msg, composed).merge_env(profile_env);
                self.request(host, msg, to);
            }
        }
        Ok(())
    }
}
