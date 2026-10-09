//! Session view type, sanitization helpers, and session lifecycle methods.
//!
//! `SessionView` and `sanitize_label` are the shared types.
//! The `impl App` block at the bottom adds session lifecycle methods to `App`;
//! it lives here (a sibling of `app.rs`) to keep each file focused. It can
//! access `App::effects` because that field is `pub(super)`.

use uuid::Uuid;

use crate::proto::{Msg, SessionId, SessionInfo, SessionState, SpawnSpec};
use crate::term::screen::Screen;

use super::state::{self, ClientState, SavedSession};

// ── SessionView ───────────────────────────────────────────────────────────────

/// Strip C0 controls (0x00–0x1F), DEL (0x7F), and C1 controls (0x80–0x9F)
/// from `s`, and cap the result at `max_chars` characters. Used for any
/// text that leaves the process boundary (OSC notifications, tab labels
/// written to the outer terminal).
pub fn sanitize_label(s: &str, max_chars: usize) -> String {
    s.chars()
        .filter(|&c| {
            let n = c as u32;
            // Keep printable ASCII and non-C1 Unicode.
            !(n < 0x20 || n == 0x7f || (0x80..=0x9f).contains(&n))
        })
        .take(max_chars)
        .collect()
}

/// The client-side view of one session.
pub struct SessionView {
    pub id: SessionId,
    pub name: Option<String>,
    pub cwd: String,
    pub host: String,
    pub state: SessionState,
    pub title: Option<String>,
    pub claude_session_id: Option<String>,
    pub created_at: u64,
    /// Mirror of the daemon's screen; fed only while attached.
    pub mirror: Screen,
    pub attached: bool,
    /// Proxy profile name (None = no proxy). Only the name, never the token.
    pub proxy: Option<String>,
    /// Git branch of the session's cwd, if any.
    pub branch: Option<String>,
    /// Short human-readable model name from the last assistant turn.
    pub model: Option<String>,
    /// Total input+cache tokens of the last assistant turn.
    pub context_tokens: Option<u64>,
}

impl SessionView {
    /// The tab label: the user's name, else a meaningful claude title (not
    /// "Claude Code"), else the cwd's basename.
    /// Sanitized so it is safe to embed in escape sequences.
    pub fn label(&self) -> String {
        let raw = self
            .name
            .clone()
            .or_else(|| {
                self.title.as_ref().and_then(|t| {
                    if t == "Claude Code" || t.is_empty() {
                        None
                    } else {
                        Some(t.clone())
                    }
                })
            })
            .unwrap_or_else(|| {
                self.cwd
                    .rsplit('/')
                    .find(|s| !s.is_empty())
                    .unwrap_or("/")
                    .to_owned()
            });
        sanitize_label(&raw, 200)
    }

    pub fn saved(&self) -> SavedSession {
        SavedSession {
            id: self.id,
            name: self.name.clone(),
            cwd: self.cwd.clone(),
            host: self.host.clone(),
            claude_session_id: self.claude_session_id.clone(),
            created_at: self.created_at,
            proxy: self.proxy.clone(),
        }
    }
}

// ── Session lifecycle (impl App) ──────────────────────────────────────────────
//
// These methods are spread into sessions.rs to keep app.rs focused on the
// App struct, constructors, and proxy/persistence logic.

use super::app::{App, Effect, ReplyTo, PROJECTS_LIMIT};
use super::interaction::Modal;
use super::wizard::{self, Wizard};

impl App {
    // ── Accessors ─────────────────────────────────────────────────────────────

    pub(super) fn index_of(&self, id: SessionId) -> Option<usize> {
        self.sessions.iter().position(|v| v.id == id)
    }

    /// The host of the active session (or "local" when none is active).
    pub(super) fn active_host(&self) -> String {
        self.active_view()
            .map(|v| v.host.clone())
            .unwrap_or_else(|| "local".to_owned())
    }

    /// The host the wizard is currently targeting (or "local").
    pub(super) fn wizard_host(&self) -> String {
        match &self.modal {
            Some(Modal::Wizard(w)) => w.host.clone(),
            _ => "local".to_owned(),
        }
    }

    /// The current wizard's generation (0 if no wizard is open).
    pub(super) fn wizard_generation(&self) -> u64 {
        match &self.modal {
            Some(Modal::Wizard(w)) => w.generation,
            _ => 0,
        }
    }

    // ── Effects helpers ───────────────────────────────────────────────────────

    /// Enqueue a Save effect (deduped: only one at a time at the tail).
    pub(super) fn save(&mut self) {
        if !matches!(self.effects.last(), Some(Effect::Save)) {
            self.effects.push(Effect::Save);
        }
    }

    /// Send a request to `host`'s daemon.
    pub(super) fn request(&mut self, host: &str, msg: Msg, to: ReplyTo) {
        self.effects.push(Effect::Request {
            host: host.to_owned(),
            msg,
            to,
        });
    }

    /// Send a request to the local daemon.
    pub(super) fn request_local(&mut self, msg: Msg, to: ReplyTo) {
        self.request("local", msg, to);
    }

    // ── Activate / remove ─────────────────────────────────────────────────────

    /// Make session `i` the active one: detach the old, attach the new.
    pub(super) fn activate(&mut self, i: usize) {
        if i >= self.sessions.len() {
            return;
        }
        if self.active == Some(i) && self.sessions[i].attached {
            return;
        }
        if let Some(old) = self.active.filter(|&old| old != i) {
            if let Some(v) = self.sessions.get_mut(old).filter(|v| v.attached) {
                v.attached = false;
                let id = v.id;
                let host = v.host.clone();
                self.request(&host, Msg::Detach { id }, ReplyTo::Ack("detach"));
            }
        }
        self.active = Some(i);
        let (rows, cols) = self.pane_size();
        let view = &mut self.sessions[i];
        view.attached = true;
        // Blank until the daemon's `Attached` + snapshot arrive.
        view.mirror = Screen::new(rows, cols);
        let id = view.id;
        let host = view.host.clone();
        self.request(
            &host,
            Msg::Attach { id, rows, cols },
            ReplyTo::Ack("attach"),
        );
        self.redraw = true;
        self.save();
    }

    /// Forget session `i` and activate a neighbour.
    pub(super) fn remove(&mut self, i: usize) {
        if i >= self.sessions.len() {
            return;
        }
        self.sessions.remove(i);
        match self.active {
            Some(a) if a == i => {
                self.active = None;
                if !self.sessions.is_empty() {
                    self.activate(i.min(self.sessions.len() - 1));
                }
            }
            Some(a) if a > i => self.active = Some(a - 1),
            _ => {}
        }
        if self.sessions.is_empty() && self.modal.is_none() {
            self.open_wizard();
        }
        self.redraw = true;
        self.save();
    }

    // ── Spawn / wizard ────────────────────────────────────────────────────────

    /// Spawn claude in `cwd` on `host` (optionally resuming) and switch to it.
    ///
    /// When a proxy is selected and its config is cached, env is built
    /// immediately. When the cache is cold (first spawn), the effect runner
    /// fetches the config (5 s timeout) before building env so the proxy's
    /// model/rate-limit overrides are applied even on the first session.
    pub(super) fn spawn(
        &mut self,
        host: String,
        cwd: String,
        resume: Option<String>,
        proxy: Option<String>,
    ) {
        let (rows, cols) = self.pane_size();
        let id = Uuid::new_v4();
        let args = resume
            .map(|r| vec!["--resume".to_owned(), r])
            .unwrap_or_default();

        // When a proxy is selected and the config cache is cold, defer env
        // construction to the effect runner so it can await the fetch.
        let cache_cold = proxy.as_deref().is_some_and(|n| self.proxy_config_cached(n).is_none());
        if cache_cold {
            let proxy_name = proxy.clone().unwrap();
            let spec = SpawnSpec {
                id,
                cwd: cwd.clone(),
                name: None,
                args,
                env: vec![], // runner fills this in
                rows,
                cols,
            };
            self.effects.push(Effect::SpawnWithProxy {
                host: host.clone(),
                spec,
                proxy_name,
                to: ReplyTo::SpawnedDeferred(id),
            });
        } else {
            // Cache is warm (or no proxy) — build env synchronously.
            let env = match self.proxy_env_for(proxy.as_deref()) {
                Ok(e) => e,
                Err(e) => {
                    self.notify(format!("cannot spawn: {e}"));
                    return;
                }
            };
            let spec = SpawnSpec {
                id,
                cwd: cwd.clone(),
                name: None,
                args,
                env,
                rows,
                cols,
            };
            self.request(&host, Msg::Spawn(spec), ReplyTo::Spawned(id));
        }
        self.sessions.push(SessionView {
            id,
            name: None,
            cwd: cwd.clone(),
            host: host.clone(),
            state: SessionState::Starting,
            title: None,
            claude_session_id: None,
            created_at: self.now,
            mirror: Screen::new(rows, cols),
            attached: false,
            proxy: proxy.clone(),
            branch: None,
            model: None,
            context_tokens: None,
        });
        state::push_recent(&mut self.recent_dirs, &host, &cwd);
        self.activate(self.sessions.len() - 1);
    }

    pub(super) fn open_wizard(&mut self) {
        let active_cwd = self.active_view().map(|v| v.cwd.clone());
        let active_proxy = self.active_view().and_then(|v| v.proxy.clone());
        let active_host = self.active_host();

        // Seeds come from the active host's recent dirs. If the user later
        // switches to a remote host, on_host_connected rebuilds the seeds.
        let host_recent = state::recent_for_host(&self.recent_dirs, &active_host).to_vec();

        // Only include the active session's cwd if it is on the active host.
        let active_cwd_for_host = active_cwd.as_deref().filter(|_| {
            self.active_view()
                .map(|v| v.host == active_host)
                .unwrap_or(false)
        });
        let seeds = wizard::assemble(active_cwd_for_host, &host_recent, &self.projects);
        let host_candidates = crate::remote::hosts::candidates();
        // Determine effective proxy default for the wizard:
        // active session's proxy > startup override > config default.
        let proxy_default = active_proxy.as_deref().or_else(|| {
            match &self.proxy_override {
                crate::tui::app::ProxyChoice::Direct => None,
                crate::tui::app::ProxyChoice::Profile(p) => Some(p.as_str()),
                crate::tui::app::ProxyChoice::Default => self.proxy_default.as_deref(),
            }
        });
        self.modal = Some(Modal::Wizard(Wizard::new(
            seeds,
            self.home.clone(),
            &active_host,
            &host_candidates,
            &self.proxy_profiles.clone(),
            proxy_default,
        )));
        self.request_local(
            Msg::RecentProjects {
                limit: PROJECTS_LIMIT,
            },
            ReplyTo::Projects,
        );
        self.redraw = true;
    }

    // ── Recovery ─────────────────────────────────────────────────────────────

    /// Full recovery: rebuild the entire session list from `saved` and the
    /// daemon's `live` sessions (initial local startup).
    ///
    /// Only call once at startup. For reconnects, use [`recover_host`].
    pub fn recover(&mut self, saved: &ClientState, live: &[SessionInfo]) {
        // Merge in tombstones from saved state.
        self.killed = saved.killed.clone();
        self.recover_host_inner("local", saved, live, true);
    }

    /// Host-scoped recovery (M1 fix): reconcile only the given host's
    /// sessions against the current live list from that host's daemon.
    ///
    /// - Does NOT touch sessions belonging to other hosts.
    /// - Assigns unknown live sessions the correct originating host (not "local").
    /// - Preserves offline hosts' tabs, names and proxy associations.
    /// - Re-sends Kill for any tombstone that is still in the live list.
    pub fn recover_host(&mut self, host: &str, live: &[SessionInfo]) {
        // Build a synthetic ClientState from current in-memory sessions for this host.
        let saved_for_host = ClientState {
            sessions: self
                .sessions
                .iter()
                .filter(|v| v.host == host)
                .map(SessionView::saved)
                .collect(),
            active: self.active_view().filter(|v| v.host == host).map(|v| v.id),
            recent_dirs: self.recent_dirs.clone(),
            killed: self.killed.clone(),
        };
        // Remove existing sessions for this host (they'll be re-added after merge).
        let active_id = self.active_view().map(|v| v.id);
        self.sessions.retain(|v| v.host != host);
        // Adjust active index (may now point into a shrunk vec).
        self.active = active_id.and_then(|id| self.sessions.iter().position(|v| v.id == id));

        self.recover_host_inner(host, &saved_for_host, live, false);
    }

    /// Internal: merge `saved` + `live` for `host` and add the resulting
    /// sessions to `self.sessions`. When `is_full` is true (initial startup),
    /// sessions.clear() is called first and active/wizard logic runs.
    fn recover_host_inner(
        &mut self,
        host: &str,
        saved: &ClientState,
        live: &[SessionInfo],
        is_full: bool,
    ) {
        let (rows, cols) = self.pane_size();
        if is_full {
            self.connected = true;
            self.sessions.clear();
            self.active = None;
        }

        let merged = state::merge_for_host(host, saved, live);

        // Re-send Kill for tombstoned sessions that are still live.
        for live_info in live {
            if self
                .killed
                .iter()
                .any(|t| t.id == live_info.id && t.host == host)
            {
                let id = live_info.id;
                self.effects.push(Effect::Request {
                    host: host.to_owned(),
                    msg: Msg::Kill { id },
                    to: ReplyTo::Kill(id),
                });
            }
        }

        for r in merged {
            if let Some(ref args) = r.respawn {
                let proxy_name = r.saved.proxy.clone();
                let cache_cold = proxy_name
                    .as_deref()
                    .is_some_and(|n| self.proxy_config_cached(n).is_none());
                if cache_cold {
                    // Proxy config not cached — defer env construction.
                    let spec = SpawnSpec {
                        id: r.saved.id,
                        cwd: r.saved.cwd.clone(),
                        name: r.saved.name.clone(),
                        args: args.clone(),
                        env: vec![], // runner fills in
                        rows,
                        cols,
                    };
                    self.effects.push(Effect::SpawnWithProxy {
                        host: r.saved.host.clone(),
                        spec,
                        proxy_name: proxy_name.unwrap(),
                        to: ReplyTo::SpawnedDeferred(r.saved.id),
                    });
                } else {
                    // Cache is warm (or no proxy) — build env now.
                    match self.proxy_env_for(proxy_name.as_deref()) {
                        Ok(env) => {
                            let spec = SpawnSpec {
                                id: r.saved.id,
                                cwd: r.saved.cwd.clone(),
                                name: r.saved.name.clone(),
                                args: args.clone(),
                                env,
                                rows,
                                cols,
                            };
                            let h = r.saved.host.clone();
                            self.effects.push(Effect::Request {
                                host: h,
                                msg: Msg::Spawn(spec),
                                to: ReplyTo::Spawned(r.saved.id),
                            });
                        }
                        Err(e) => {
                            self.notify(format!("cannot respawn '{}': {e}", r.saved.cwd));
                        }
                    }
                }
            }
            self.sessions.push(SessionView {
                id: r.saved.id,
                name: r.saved.name,
                cwd: r.saved.cwd,
                host: r.saved.host,
                state: r.state,
                title: r.title,
                claude_session_id: r.saved.claude_session_id,
                created_at: r.saved.created_at,
                mirror: Screen::new(rows, cols),
                attached: false,
                proxy: r.saved.proxy,
                branch: None,
                model: None,
                context_tokens: None,
            });
        }

        if is_full {
            let restore = saved.active.and_then(|id| self.index_of(id));
            match restore.or((!self.sessions.is_empty()).then_some(0)) {
                Some(i) => self.activate(i),
                None if self.modal.is_none() => self.open_wizard(),
                None => {}
            }
        } else {
            // After a per-host recovery, activate the first session for this
            // host if nothing is currently active.
            if self.active.is_none() && !self.sessions.is_empty() {
                self.activate(0);
            }
        }
        self.save();
    }

    /// Re-run recovery after the local daemon reconnects.
    pub fn on_reconnected_local(&mut self, live: &[SessionInfo], home: String) {
        self.home = home;
        self.notice = None;
        self.connected = true;
        self.recover_host("local", live);
    }

    /// The local daemon connection dropped (M1 fix: only affects local sessions).
    pub fn on_disconnected_local(&mut self) {
        self.connected = false;
        for v in &mut self.sessions {
            if v.host == "local" {
                v.attached = false;
            }
        }
        self.redraw = true;
    }

    /// A remote daemon connection dropped (only affects that host's sessions).
    pub fn on_disconnected_remote(&mut self, host: &str) {
        for v in &mut self.sessions {
            if v.host == host {
                v.state = SessionState::Unknown;
                v.attached = false;
            }
        }
        self.redraw = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_c0_controls() {
        assert_eq!(sanitize_label("hello\x07world", 200), "helloworld");
        // ESC (0x1B) is stripped; the remaining "[31m" chars are printable ASCII.
        assert_eq!(sanitize_label("a\x1b[31mb", 200), "a[31mb");
    }

    #[test]
    fn sanitize_strips_del() {
        assert_eq!(sanitize_label("a\x7fb", 200), "ab");
    }

    #[test]
    fn sanitize_strips_c1_controls() {
        // C1 control: 0x9B (CSI in Latin-1).
        let s = "\u{009B}test";
        assert_eq!(sanitize_label(s, 200), "test");
    }

    #[test]
    fn sanitize_caps_length() {
        let long: String = "a".repeat(300);
        assert_eq!(sanitize_label(&long, 200).len(), 200);
    }

    #[test]
    fn sanitize_keeps_normal_unicode() {
        assert_eq!(sanitize_label("héllo wörld", 200), "héllo wörld");
    }

    #[test]
    fn sanitize_malicious_label() {
        // Injection attempt: ESC (stripped), ] (printable), BEL (stripped), [31m (printable).
        // ESC is the dangerous byte; stripping it neutralises the OSC sequence.
        let evil = "\x1b]0;injected\x07\x1b[31m";
        assert_eq!(sanitize_label(evil, 200), "]0;injected[31m");
    }
}
