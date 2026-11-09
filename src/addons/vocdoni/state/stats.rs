//! The proxy's numbers for the front-ends: each profile's stats, pool
//! health and model catalogue (the proxy panel), and the upstream
//! credential each focused proxied session last used (its segments).
//! Pure, like the rest of [`State`]: fetches are [`Task`]s or the panel's
//! own awaits, their answers transitions.

use std::collections::HashMap;

use super::{State, Task};
use crate::addons::vocdoni::proxy::api::{
    ModelsResponse, PoolHealthResponse, SessionCredential, StatsResponse,
};
use crate::addons::vocdoni::proxy::stats::Window;
use crate::proto::SessionId;

/// How long a focused session's credential lookup stays fresh.
pub const CRED_POLL_SECS: u64 = 30;
/// The least gap between two lookups of one session (the proxy allows one
/// request a second): a new conversation waits for it too.
pub const CRED_MIN_GAP_SECS: u64 = 5;

/// One profile's proxy data, as last fetched.
#[derive(Debug, Clone, Default)]
pub struct ProxyStatus {
    pub pool: Option<PoolHealthResponse>,
    /// Stats per window; only the windows fetched so far.
    pub stats: HashMap<Window, StatsResponse>,
    /// The model catalogue, fetched once per profile.
    pub models: Option<ModelsResponse>,
    /// The last fetch error, cleared by the next successful stats fetch.
    pub error: Option<String>,
    /// When the stats were last fetched (Unix seconds).
    pub fetched_at: Option<u64>,
}

impl ProxyStatus {
    /// The windows that have stats, in canonical order.
    pub fn cached_windows(&self) -> Vec<Window> {
        Window::ALL
            .into_iter()
            .filter(|w| self.stats.contains_key(w))
            .collect()
    }

    /// Merge one fetch's result, made at `now`.
    pub fn apply(&mut self, fetch: ProxyFetch, now: u64) {
        for (window, result) in fetch.stats {
            match result {
                Ok(stats) => {
                    self.stats.insert(window, stats);
                    self.error = None;
                }
                Err(e) => self.error = Some(e),
            }
        }
        if fetch.pool.is_some() {
            self.pool = fetch.pool;
        }
        if fetch.models.is_some() {
            self.models = fetch.models;
        }
        self.fetched_at = Some(now);
    }
}

/// What one stats fetch got.
#[derive(Debug, Default)]
pub struct ProxyFetch {
    /// One entry per window asked; errors are one line for the member.
    pub stats: Vec<(Window, Result<StatsResponse, String>)>,
    pub pool: Option<PoolHealthResponse>,
    pub models: Option<ModelsResponse>,
}

/// The upstream credential the proxy last named for one session.
#[derive(Debug, Clone)]
pub struct SessionCred {
    /// The conversation the lookup was for: an answer or an entry for
    /// another one is stale.
    pub conversation: String,
    /// `None` until the proxy knows the conversation (404), or after an
    /// error before the first answer.
    pub cred: Option<SessionCredential>,
    /// When the lookup was last asked (Unix seconds).
    pub asked_at: u64,
}

/// A focused session on a profile, with its conversation.
pub struct Focused {
    pub sid: SessionId,
    pub profile: String,
    pub conversation: String,
}

impl State {
    /// The second's credential lookups: each focused proxied session's,
    /// once [`CRED_POLL_SECS`] passed, or [`CRED_MIN_GAP_SECS`] after its
    /// conversation changed. Unfocused sessions keep what they have.
    pub fn creds_due(&mut self, focused: Vec<Focused>) -> Vec<Task> {
        let mut tasks = Vec::new();
        for f in focused {
            let gap = match self.creds.get(&f.sid) {
                Some(c) if c.conversation == f.conversation => CRED_POLL_SECS,
                Some(_) => CRED_MIN_GAP_SECS,
                None => 0,
            };
            let asked = self.creds.get(&f.sid).map_or(0, |c| c.asked_at);
            if asked != 0 && self.now.saturating_sub(asked) < gap {
                continue;
            }
            let kept = match self.creds.remove(&f.sid) {
                Some(old) if old.conversation == f.conversation => old.cred,
                _ => None,
            };
            let entry = SessionCred {
                conversation: f.conversation.clone(),
                cred: kept,
                asked_at: self.now,
            };
            self.creds.insert(f.sid, entry);
            tasks.push(Task::FetchCred {
                sid: f.sid,
                profile: f.profile,
                conversation: f.conversation,
            });
        }
        tasks
    }

    /// The proxy answered session `sid`'s lookup for `conversation`
    /// (`Ok(None)`: it does not know it yet); an error keeps what is
    /// cached, as does an answer for a conversation since left.
    pub fn on_cred(
        &mut self,
        sid: SessionId,
        conversation: &str,
        result: Result<Option<SessionCredential>, String>,
    ) -> Vec<Task> {
        let Some(entry) = self.creds.get_mut(&sid) else {
            return Vec::new();
        };
        match result {
            Ok(cred) if entry.conversation == conversation && entry.cred != cred => {
                entry.cred = cred;
                vec![Task::Changed]
            }
            _ => Vec::new(),
        }
    }

    /// Session `sid`'s credential, for its conversation now.
    pub fn cred(&self, sid: SessionId, conversation: Option<&str>) -> Option<&SessionCredential> {
        let entry = self.creds.get(&sid)?;
        if conversation != Some(entry.conversation.as_str()) {
            return None;
        }
        entry.cred.as_ref()
    }

    /// Ask session `sid`'s credential again on the next second (after a
    /// switch).
    pub fn cred_stale(&mut self, sid: SessionId) {
        if let Some(entry) = self.creds.get_mut(&sid) {
            entry.asked_at = 1;
        }
    }

    /// Session `sid` went away.
    pub fn forget_session(&mut self, sid: SessionId) {
        self.creds.remove(&sid);
    }

    /// `profile`'s stats were fetched.
    pub fn on_stats(&mut self, profile: &str, fetch: ProxyFetch) {
        let now = self.now;
        let status = self.stats.entry(profile.to_owned()).or_default();
        status.apply(fetch, now);
    }
}

#[cfg(test)]
mod tests;
