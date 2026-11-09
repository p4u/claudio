//! The TUI's proxy glue: what `App` does with the proxy's answers. The
//! Vocdoni addon takes it over (core-addons plan, P3.5b-P3.12).

use super::app::{App, Effect, Modal};
use super::proxy_state::{ProxyFetch, SessionCred};
use super::sessions::SessionView;
use super::HostEvent;
use crate::addons::vocdoni::proxy::api::{SessionCredential, SwitchError, SwitchOutcome};
use crate::proto::SessionId;

impl App {
    /// A proxy answer from a background fetch: stats, a session's
    /// credential or a subscription switch. Other events are not the proxy's.
    pub(super) fn on_proxy_event(&mut self, ev: HostEvent) {
        match ev {
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
        let profile = v.profile().map(str::to_owned);
        let (Some(profile), Some(csid)) = (profile, v.claude_session_id.clone()) else {
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
}
