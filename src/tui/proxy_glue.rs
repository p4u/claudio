//! The TUI's proxy glue: what `App` does with the proxy's answers. The
//! Vocdoni addon takes it over (core-addons plan, P3.5b-P3.12).

use super::app::App;
use super::HostEvent;

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
}
