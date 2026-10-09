//! The new-session wizard: pick a host (step 0), then a directory (step 1),
//! then optionally a claude conversation to resume (step 2).
//!
//! The wizard is a pure state machine. Key handling returns an [`Outcome`]
//! telling the app what to do next (connect to a host, ask the daemon for
//! directory or session data, or spawn a new session). Replies are fed back
//! through the `set_*` and `on_host_connected` methods.
//!
//! Each wizard instance carries a `generation` counter so that async replies
//! from a cancelled wizard are discarded.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::collections::HashMap;

use crate::proto::{ClaudeSession, DirEntry};

use super::fmt::{abbreviate_home, fmt_age};

/// Enrichment metadata for a candidate directory.
///
/// Carried in `Wizard::meta` keyed by absolute path. Populated from
/// `ListDir` replies and `RecentProjects` replies.
#[derive(Debug, Clone, Default)]
pub struct DirMeta {
    /// Git branch name (`None` = not a git repo, `Some("")` = detached HEAD).
    pub git: Option<String>,
    /// Unix seconds of the last claude session in this directory.
    pub claude_at: Option<u64>,
    /// Whether the path is a symlink.
    pub symlink: bool,
    /// Whether the path's basename starts with `.`.
    pub hidden: bool,
    /// Whether this directory is in the host's `recent_dirs` list.
    pub recently_used: bool,
}

/// What the app should do after the wizard handled an input.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    None,
    Cancel,
    /// Connect to this host before proceeding to the directory step.
    ConnectHost(String),
    /// Request `ListDir` of this absolute path (path completion).
    ListDir(String),
    /// A directory was chosen: request `ListClaudeSessions` for it.
    ChooseDir(String),
    /// Start claude in `cwd`, resuming `resume` if set, with optional proxy profile.
    Spawn {
        cwd: String,
        resume: Option<String>,
        proxy: Option<String>,
    },
}

// ── Step 0: host / first screen ───────────────────────────────────────────────

/// Which section of the first screen has keyboard focus.
#[derive(Debug, Clone, PartialEq)]
pub enum HostSection {
    Local,
    Remote,
}

/// State for the wizard's first screen.
///
/// Two scrollable sections:
/// - **LOCAL** (top): "Explore local dirs…" + recent local dirs not already open
/// - **REMOTE** (bottom): SSH host candidates
///
/// Tab switches focus between sections. ↑/↓ cross section edges. Typing
/// filters both sections at once. Backspace edits the filter; Esc cancels.
#[derive(Debug, Clone)]
pub struct HostStep {
    /// Shared filter string (applied to both sections).
    pub input: String,
    /// Which section has keyboard focus.
    pub focus: HostSection,
    /// Selected row in the LOCAL section (0 = "Explore local dirs…").
    pub local_selected: usize,
    /// Filtered local dirs (does not include the "Explore" pseudo-entry).
    pub local_items: Vec<String>,
    /// All recent local dirs (unfiltered).
    local_dirs: Vec<String>,
    /// Selected row in the REMOTE section.
    pub selected: usize,
    /// Filtered candidate list of SSH hosts.
    pub items: Vec<String>,
    /// `Some(host)` while bootstrap + connect_ssh is running.
    pub connecting: Option<String>,
    /// The full unfiltered SSH host list.
    candidates: Vec<String>,
    /// `true` when the query looks like a bare hostname / `user@host` but matches no
    /// known candidate.  A synthetic "connect to <query>" row is appended to the
    /// REMOTE section and auto-selected so Enter immediately initiates the connection.
    pub connect_raw: bool,
}

impl HostStep {
    /// Build a host step without local dirs.
    ///
    /// - `active_host`: pre-selects the active session's host.
    /// - `extra`: SSH host candidates (from `hosts::candidates()`).
    ///
    /// Use [`with_local_dirs`] to also populate the LOCAL section.
    #[allow(dead_code)]
    pub fn new(active_host: &str, extra: &[String]) -> HostStep {
        HostStep::with_local_dirs(active_host, extra, &[])
    }

    /// Like `new` but also populates the LOCAL section with recent dirs.
    ///
    /// Non-existing local dirs are silently dropped so stale test artifacts
    /// don't pollute the list (see `claude::projects::recent_project_dirs`).
    pub fn with_local_dirs(
        active_host: &str,
        extra: &[String],
        local_dirs: &[String],
    ) -> HostStep {
        let candidates: Vec<String> = extra
            .iter()
            .filter(|h| h.as_str() != "local")
            .cloned()
            .collect();
        let remote_selected = if active_host != "local" {
            candidates
                .iter()
                .position(|h| h == active_host)
                .unwrap_or(0)
        } else {
            0
        };
        let focus = if active_host != "local" && !candidates.is_empty() {
            HostSection::Remote
        } else {
            HostSection::Local
        };
        let items = candidates.clone();
        // Drop local dirs that no longer exist on the filesystem so stale
        // test temp dirs (e.g. /tmp/cl-e2e-*) don't appear in the list.
        let local_dirs_existing: Vec<String> = local_dirs
            .iter()
            .filter(|d| std::path::Path::new(d.as_str()).exists())
            .cloned()
            .collect();
        let local_items = local_dirs_existing.clone();
        HostStep {
            input: String::new(),
            focus,
            local_selected: 0,
            local_items,
            local_dirs: local_dirs_existing,
            selected: remote_selected,
            items,
            connecting: None,
            candidates,
            connect_raw: false,
        }
    }

    /// Total rows in the LOCAL section (including the "Explore" pseudo-entry).
    pub fn local_len(&self) -> usize {
        1 + self.local_items.len()
    }

    /// Total rows in the REMOTE section.
    ///
    /// Includes the synthetic "connect to <query>" row when `connect_raw` is set.
    pub fn remote_len(&self) -> usize {
        self.items.len() + if self.connect_raw { 1 } else { 0 }
    }

    /// Handle a key press on the host step.
    pub fn on_key(&mut self, key: &KeyEvent) -> Outcome {
        if key.kind == KeyEventKind::Release {
            return Outcome::None;
        }
        if self.connecting.is_some() {
            // Waiting for connection: only allow backing out.
            if key.code == KeyCode::Esc {
                self.connecting = None;
            }
            return Outcome::None;
        }
        let plain = key.modifiers.difference(KeyModifiers::SHIFT).is_empty();
        match key.code {
            KeyCode::Esc => Outcome::Cancel,
            KeyCode::Tab => {
                // Switch focus between LOCAL and REMOTE.
                self.focus = match self.focus {
                    HostSection::Local => HostSection::Remote,
                    HostSection::Remote => HostSection::Local,
                };
                Outcome::None
            }
            KeyCode::Up => {
                match self.focus {
                    HostSection::Local => {
                        if self.local_selected > 0 {
                            self.local_selected -= 1;
                        }
                    }
                    HostSection::Remote => {
                        if self.selected > 0 {
                            self.selected -= 1;
                        } else if !self.local_items.is_empty() || true {
                            // Cross edge: go to bottom of LOCAL section.
                            self.focus = HostSection::Local;
                            self.local_selected = self.local_len().saturating_sub(1);
                        }
                    }
                }
                Outcome::None
            }
            KeyCode::Down => {
                match self.focus {
                    HostSection::Local => {
                        let max = self.local_len().saturating_sub(1);
                        if self.local_selected < max {
                            self.local_selected += 1;
                        } else if !self.items.is_empty() {
                            // Cross edge: go to top of REMOTE section.
                            self.focus = HostSection::Remote;
                            self.selected = 0;
                        }
                    }
                    HostSection::Remote => {
                        let max = self.remote_len().saturating_sub(1);
                        if self.selected < max {
                            self.selected += 1;
                        }
                    }
                }
                Outcome::None
            }
            KeyCode::Enter => match self.focus {
                HostSection::Local => {
                    if self.local_selected == 0 {
                        // "Explore local dirs…" → directory step
                        Outcome::ConnectHost("local".to_owned())
                    } else {
                        // A specific recent dir → go directly to that dir
                        match self.local_items.get(self.local_selected - 1) {
                            Some(dir) => Outcome::ChooseDir(dir.clone()),
                            None => Outcome::ConnectHost("local".to_owned()),
                        }
                    }
                }
                HostSection::Remote => {
                    match self.items.get(self.selected) {
                        Some(h) => {
                            self.connecting = Some(h.clone());
                            Outcome::ConnectHost(h.clone())
                        }
                        // connect_raw row or any non-empty query with no list match
                        // → treat the raw input as the hostname.
                        None if !self.input.trim().is_empty() => {
                            let h = self.input.trim().to_owned();
                            self.connecting = Some(h.clone());
                            Outcome::ConnectHost(h)
                        }
                        None => Outcome::None,
                    }
                }
            },
            KeyCode::Backspace => {
                self.input.pop();
                self.refilter();
                Outcome::None
            }
            KeyCode::Char('u') if key.modifiers == KeyModifiers::CONTROL => {
                self.input.clear();
                self.refilter();
                Outcome::None
            }
            KeyCode::Char(c) if plain => {
                self.input.push(c);
                self.refilter();
                Outcome::None
            }
            _ => Outcome::None,
        }
    }

    /// Paste handler for the host step: set input to the first pasted line.
    pub fn on_paste(&mut self, text: &str) {
        let first = text.lines().next().unwrap_or("").trim();
        if !first.is_empty() {
            self.input = first.to_owned();
            self.refilter();
        }
    }

    fn refilter(&mut self) {
        let input = self.input.trim().to_lowercase();
        if input.is_empty() {
            // Empty query → restore unfiltered lists and default selection.
            self.items = self.candidates.clone();
            self.local_items = self.local_dirs.clone();
            self.focus = HostSection::Local;
            self.local_selected = 0;
            self.selected = 0;
            self.connect_raw = false;
            return;
        }

        // ── Filter remote hosts (scored, best first). ──────────────────────────
        let mut scored_remote: Vec<(i64, String)> = self
            .candidates
            .iter()
            .filter_map(|h| Some((fuzzy_score(&input, h)?, h.clone())))
            .collect();
        scored_remote.sort_by(|a, b| b.0.cmp(&a.0));
        self.items = scored_remote.iter().map(|(_, h)| h.clone()).collect();

        // ── Filter local dirs (scored, best first). ────────────────────────────
        let mut scored_local: Vec<(i64, String)> = self
            .local_dirs
            .iter()
            .filter_map(|d| Some((fuzzy_score(&input, d)?, d.clone())))
            .collect();
        scored_local.sort_by(|a, b| b.0.cmp(&a.0));
        self.local_items = scored_local.iter().map(|(_, d)| d.clone()).collect();

        // ── Score the "Explore local dirs…" pseudo-entry. ─────────────────────
        let explore_score: Option<i64> = fuzzy_score(&input, "explore local dirs");

        // ── Decide whether to offer a "connect to <query>" synthetic row. ─────
        // Only when nothing else matches and the query looks like a hostname.
        let has_any_match =
            !scored_remote.is_empty() || !scored_local.is_empty() || explore_score.is_some();
        self.connect_raw = !has_any_match && looks_like_host(self.input.trim());

        // ── Auto-select the best-scoring visible row. ──────────────────────────
        // Remote beats local on ties (typing a host name is more specific than
        // a dir fragment, and we must not stay pinned on "Explore local dirs…").
        let best_remote = scored_remote.first().map(|(s, _)| *s);
        let best_local_dir = scored_local.first().map(|(s, _)| *s);

        if let Some(rs) = best_remote {
            let ls = best_local_dir.unwrap_or(i64::MIN);
            let es = explore_score.unwrap_or(i64::MIN);
            if rs >= ls && rs >= es {
                self.focus = HostSection::Remote;
                self.selected = 0;
                // Clamp local too.
                self.local_selected =
                    self.local_selected.min(self.local_len().saturating_sub(1));
                return;
            }
        }
        if let Some(ls) = best_local_dir {
            let es = explore_score.unwrap_or(i64::MIN);
            if ls > es {
                // A specific recent local dir beats "Explore local dirs…".
                self.focus = HostSection::Local;
                self.local_selected = 1; // index 1 = first local_items entry
                self.selected = self.selected.min(self.remote_len().saturating_sub(1));
                return;
            }
        }
        if explore_score.is_some() {
            // "Explore local dirs…" matched the query.
            self.focus = HostSection::Local;
            self.local_selected = 0;
            self.selected = self.selected.min(self.remote_len().saturating_sub(1));
            return;
        }
        // Nothing matched. If connect_raw, point Remote at the synthetic row.
        if self.connect_raw {
            self.focus = HostSection::Remote;
            self.selected = 0; // remote_len() == 1 (only the connect_raw row)
        }
        // Clamp both indices within the new bounds.
        let remote_cap = self.remote_len().saturating_sub(1);
        self.selected = self.selected.min(remote_cap);
        let local_cap = self.local_len().saturating_sub(1);
        self.local_selected = self.local_selected.min(local_cap);
    }
}

/// Returns `true` if `s` looks like a hostname or `user@host` (no spaces,
/// at least 2 chars, does not start with `/` or `~`).
///
/// Used by [`HostStep::refilter`] to decide whether to show a "connect to
/// <query>" synthetic row in the REMOTE section.
pub fn looks_like_host(s: &str) -> bool {
    let s = s.trim();
    s.len() >= 2 && !s.contains(' ') && !s.starts_with('/') && !s.starts_with('~')
}

/// Step 2: the resume picker.
#[derive(Debug, Clone)]
pub struct ResumeStep {
    pub cwd: String,
    /// All sessions, newest first.
    pub sessions: Vec<ClaudeSession>,
    pub selected: usize,
    /// Live filter string (fuzzy match against session age/path).
    pub filter: String,
}

impl ResumeStep {
    /// Sessions matching the current filter (all when filter is empty).
    pub fn filtered_sessions(&self) -> Vec<&ClaudeSession> {
        if self.filter.is_empty() {
            return self.sessions.iter().collect();
        }
        let f = self.filter.to_lowercase();
        self.sessions
            .iter()
            .filter(|s| {
                s.id.to_lowercase().contains(&f)
                    || resume_label(s, 0).to_lowercase().contains(&f)
            })
            .collect()
    }

    /// Count of items shown (including the "New session" entry).
    pub fn filtered_count(&self) -> usize {
        self.filtered_sessions().len()
    }

    /// The `i`-th filtered session (0-based, not counting the "New" entry).
    pub fn filtered_session(&self, i: usize) -> Option<&ClaudeSession> {
        self.filtered_sessions().into_iter().nth(i)
    }
}

/// The wizard's state (three steps: host → directory → resume).
#[derive(Debug, Clone)]
pub struct Wizard {
    /// Step 0: host selection. `None` once a host is confirmed and we have
    /// moved to the directory step.
    pub host_step: Option<HostStep>,
    /// The chosen host (always set; `"local"` by default).
    pub host: String,
    /// The directory input line.
    pub input: String,
    /// Filtered candidates (absolute paths), best first.
    pub items: Vec<String>,
    pub selected: usize,
    /// `Some` once a directory was chosen and its sessions are listed.
    pub resume: Option<ResumeStep>,
    /// A chosen directory whose `ListClaudeSessions` reply is pending.
    pub pending: Option<String>,
    /// Seed candidates: active cwd, recent dirs, claude projects.
    seeds: Vec<String>,
    /// Subdirectories of `listed` (absolute paths).
    completions: Vec<String>,
    /// The directory last requested through `ListDir`.
    listed: Option<String>,
    /// The daemon host's home directory, for `~`.
    home: String,
    /// Available proxy profile names (`"none"` prepended at index 0).
    pub proxy_options: Vec<String>,
    /// Currently selected index in `proxy_options` (0 = none).
    pub proxy_selected: usize,
    /// Unique per wizard. Carried in reply tags so stale replies from a cancelled wizard are
    /// discarded.
    pub generation: u64,
    /// Saved first-screen state so Backspace in the directory step can
    /// return to the previous screen.
    pub saved_host_step: Option<HostStep>,
    /// Per-path enrichment metadata (git branch, claude_at, symlink, hidden,
    /// recently_used). Populated lazily from `ListDir` and `RecentProjects`
    /// replies.
    pub meta: HashMap<String, DirMeta>,
    /// Whether dot-directories are listed in the directory step. Off by
    /// default; toggled with the wizard-scoped `ToggleHidden` key and kept for
    /// the lifetime of the wizard. A query that starts with `.` reveals them
    /// regardless (see [`Wizard::refilter`]).
    pub show_hidden: bool,
}

/// Seed candidates in priority order, deduplicated (trailing slashes are
/// ignored when comparing).
pub fn assemble(active_cwd: Option<&str>, recent: &[String], projects: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let all = active_cwd
        .into_iter()
        .chain(recent.iter().map(String::as_str))
        .chain(projects.iter().map(String::as_str));
    for dir in all {
        let dir = trim_slash(dir);
        if !dir.is_empty() && !out.iter().any(|d| d == dir) {
            out.push(dir.to_owned());
        }
    }
    out
}

/// Case-insensitive fuzzy subsequence score: `None` when `query` is not a
/// subsequence of `candidate`, higher is better otherwise. Consecutive
/// matches, matches at word boundaries and an exact match score extra;
/// skipped characters cost a little.
pub fn fuzzy_score(query: &str, candidate: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }
    let query: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    let cand: Vec<char> = candidate.chars().flat_map(char::to_lowercase).collect();
    let mut score = 0i64;
    let mut qi = 0;
    let mut last: Option<usize> = None;
    for (ci, &c) in cand.iter().enumerate() {
        if qi == query.len() {
            break;
        }
        if c != query[qi] {
            continue;
        }
        score += 1;
        match last {
            Some(l) if l + 1 == ci => score += 5,
            Some(l) => score -= (ci - l - 1).min(10) as i64,
            None => {}
        }
        if ci == 0 || matches!(cand[ci - 1], '/' | '-' | '_' | '.' | ' ') {
            score += 8;
        }
        last = Some(ci);
        qi += 1;
    }
    if qi < query.len() {
        return None;
    }
    if query == cand {
        score += 1000;
    }
    Some(score)
}

/// Gives each wizard a unique generation.
static WIZARD_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl Wizard {
    /// A wizard over `seeds` (see [`assemble`]) with a host-selection step.
    ///
    /// - `active_host` pre-selects the active session's host in step 0.
    /// - `host_candidates` is `hosts::candidates()` (MRU + ssh config).
    /// - `home` expands `~` on the chosen host.
    /// - `proxy_profiles` is the list of saved proxy profile names; pass `&[]`
    ///   when none are configured (the proxy toggle is hidden).
    /// - `proxy_default` is the name of the profile that should be pre-selected.
    pub fn new(
        seeds: Vec<String>,
        home: String,
        active_host: &str,
        host_candidates: &[String],
        proxy_profiles: &[String],
        proxy_default: Option<&str>,
    ) -> Wizard {
        Self::new_with_recent(seeds, &[], home, active_host, host_candidates, proxy_profiles, proxy_default)
    }

    /// Like [`Wizard::new`] but also accepts the `recent` slice so seeds that
    /// came from `recent_dirs` are marked `recently_used` in `meta`.
    pub fn new_with_recent(
        seeds: Vec<String>,
        recent: &[String],
        home: String,
        active_host: &str,
        host_candidates: &[String],
        proxy_profiles: &[String],
        proxy_default: Option<&str>,
    ) -> Wizard {
        // Show recent local dirs in the LOCAL section so the user can jump
        // directly to a recent dir without going through the directory step.
        let host_step = HostStep::with_local_dirs(active_host, host_candidates, recent);
        let host = active_host.to_owned();
        // Build proxy options: ["none", "profile1", "profile2", …]
        let mut proxy_options = vec!["none".to_owned()];
        proxy_options.extend_from_slice(proxy_profiles);
        let proxy_selected = proxy_default
            .and_then(|d| proxy_options.iter().position(|o| o == d))
            .unwrap_or(0);
        let generation = WIZARD_GEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut meta: HashMap<String, DirMeta> = HashMap::new();
        for r in recent {
            let r = trim_slash(r);
            if !r.is_empty() {
                meta.entry(r.to_owned()).or_default().recently_used = true;
            }
        }
        let mut w = Wizard {
            host_step: Some(host_step),
            host,
            input: String::new(),
            items: Vec::new(),
            selected: 0,
            resume: None,
            pending: None,
            seeds,
            completions: Vec::new(),
            listed: None,
            home,
            proxy_options,
            proxy_selected,
            generation,
            meta,
            saved_host_step: None,
            show_hidden: false,
        };
        w.refilter();
        w
    }

    /// A fresher list of SSH host candidates. It replaces the first screen's
    /// only while nothing was typed there, so rows never move under the user.
    pub fn set_ssh_hosts(&mut self, hosts: &[String]) {
        let Some(step) = self.host_step.as_mut() else {
            return;
        };
        if step.input.is_empty() && step.connecting.is_none() {
            let local_dirs = std::mem::take(&mut step.local_dirs);
            *step = HostStep::with_local_dirs(&self.host, hosts, &local_dirs);
        }
    }

    /// The currently selected proxy profile name, or `None` when "none".
    pub fn selected_proxy(&self) -> Option<&str> {
        self.proxy_options.get(self.proxy_selected).and_then(|s| {
            if s == "none" {
                None
            } else {
                Some(s.as_str())
            }
        })
    }

    /// The host was chosen and the connection is ready. Advance to step 1
    /// (directory) and update `home` and seeds for the target host.
    ///
    /// `new_seeds` should already be assembled from the host's recent dirs and
    /// active cwd (if on that host). The caller supplies this so the wizard
    /// state machine stays pure.
    pub fn on_host_connected(&mut self, host: &str, home: &str, new_seeds: Vec<String>) {
        self.on_host_connected_with_recent(host, home, new_seeds, &[]);
    }

    /// Like [`on_host_connected`] but also marks `recent` seeds as
    /// `recently_used` in `meta`.
    pub fn on_host_connected_with_recent(
        &mut self,
        host: &str,
        home: &str,
        new_seeds: Vec<String>,
        recent: &[String],
    ) {
        // Save first screen state so Backspace can return to it.
        self.saved_host_step = self.host_step.take();
        self.host = host.to_owned();
        self.home = home.to_owned();
        self.seeds = new_seeds;
        self.completions.clear();
        self.listed = None;
        self.input.clear();
        self.selected = 0;
        // Reset meta for the new host but preserve recently_used marks.
        self.meta.clear();
        for r in recent {
            let r = trim_slash(r);
            if !r.is_empty() {
                self.meta.entry(r.to_owned()).or_default().recently_used = true;
            }
        }
        self.refilter();
    }

    /// Add late-arriving seeds (e.g. `RecentProjects` reply), keeping order.
    /// Only applies when the wizard is on `host` and has advanced to the
    /// directory step. Stale replies for a different host are silently ignored.
    pub fn add_seeds_for_host(&mut self, host: &str, more: &[String]) {
        if self.host != host || self.host_step.is_some() {
            return;
        }
        self.seeds = assemble(None, &self.seeds, more);
        self.refilter();
    }

    /// A `ListDir` reply. Ignored unless it answers the latest request.
    pub fn set_dir_entries(&mut self, path: &str, entries: &[DirEntry]) {
        if self.listed.as_deref() != Some(path) {
            return;
        }
        self.completions = entries
            .iter()
            .filter(|e| e.dir)
            .map(|e| join(path, &e.name))
            .collect();
        // Populate enrichment metadata from the daemon reply.
        for e in entries.iter().filter(|e| e.dir) {
            let full = join(path, &e.name);
            let m = self.meta.entry(full).or_default();
            if e.git.is_some() {
                m.git = e.git.clone();
            }
            if e.claude_at.is_some() {
                m.claude_at = e.claude_at;
            }
            m.symlink = e.symlink;
            m.hidden = e.hidden;
        }
        self.refilter();
    }

    /// Populate enrichment metadata from a `RecentProjects` reply.
    ///
    /// Call this after `add_seeds_for_host` so the meta is available for the
    /// frecency scorer and badge renderer.
    pub fn add_project_meta(&mut self, projects: &[crate::proto::ProjectDir]) {
        for p in projects {
            let path = trim_slash(&p.path);
            if path.is_empty() {
                continue;
            }
            let m = self.meta.entry(path.to_owned()).or_default();
            if p.git.is_some() {
                m.git = p.git.clone();
            }
            m.symlink = p.symlink;
            m.hidden = p.hidden;
        }
    }

    /// A `ListClaudeSessions` reply for `cwd`. With no sessions there is
    /// nothing to pick, so the outcome is to spawn a fresh one.
    ///
    /// Returns `Outcome::None` if the reply is for a different cwd (stale).
    pub fn set_claude_sessions(&mut self, cwd: &str, mut sessions: Vec<ClaudeSession>) -> Outcome {
        if self.pending.as_deref() != Some(cwd) {
            return Outcome::None;
        }
        self.pending = None;
        if sessions.is_empty() {
            return Outcome::Spawn {
                cwd: cwd.to_owned(),
                resume: None,
                proxy: self.selected_proxy().map(str::to_owned),
            };
        }
        sessions.sort_by(|a, b| b.modified.cmp(&a.modified));
        self.resume = Some(ResumeStep {
            cwd: cwd.to_owned(),
            sessions,
            selected: 0,
            filter: String::new(),
        });
        Outcome::None
    }

    /// A `ListClaudeSessions` request failed.
    ///
    /// Clears `pending` so the user can retry or choose a different directory.
    /// Does NOT spawn fresh — the caller must show an error notice.
    pub fn set_claude_sessions_error(&mut self, cwd: &str) {
        if self.pending.as_deref() == Some(cwd) {
            self.pending = None;
        }
    }

    /// Show or hide dot-directories in the directory step. A no-op on the
    /// other steps. The setting survives going back to the host step.
    pub fn toggle_hidden(&mut self) -> Outcome {
        if self.host_step.is_some() || self.resume.is_some() || self.pending.is_some() {
            return Outcome::None;
        }
        self.show_hidden = !self.show_hidden;
        self.refilter()
    }

    /// Whether hidden entries are currently listed: the toggle is on, or the
    /// query (or the path segment being completed) starts with `.`.
    pub fn hidden_visible(&self) -> bool {
        self.show_hidden || self.query_base().starts_with('.')
    }

    /// The part of the input being completed: the last path segment for a
    /// path query, otherwise the whole (trimmed) input.
    fn query_base(&self) -> &str {
        let input = self.input.trim();
        if self.browsing() {
            input.rfind('/').map_or("", |i| &input[i + 1..])
        } else {
            input
        }
    }

    /// Whether the directory step is in browse mode: the input is a path
    /// (starts with `/` or `~`), so the list is the listing of a directory
    /// rather than a fuzzy search over the seeds.
    pub fn browsing(&self) -> bool {
        let input = self.input.trim();
        input.starts_with('/') || input.starts_with('~')
    }

    /// Split a path query into the directory being listed and the fragment
    /// typed after its last `/` (empty for `~/`, `/srv/`, or a bare `~`).
    fn path_parts(&self) -> (String, String) {
        let input = self.input.trim();
        match input.rfind('/') {
            Some(i) => (self.expand(&input[..=i]), input[i + 1..].to_owned()),
            // A bare `~` or `~user`: list home itself.
            None => (self.expand(input), String::new()),
        }
    }

    /// Whether the "start here" pseudo-row (row 0 in browse mode) is selected.
    pub fn here_selected(&self) -> bool {
        self.browsing() && self.selected == 0
    }

    /// The directory the "start here" row picks (`None` outside browse mode):
    /// the one being listed. When a fragment matches no child (or the listing
    /// has not arrived yet) it is the typed path itself, so Enter still takes it.
    pub fn here_dir(&self) -> Option<String> {
        if !self.browsing() {
            return None;
        }
        let (dir, fragment) = self.path_parts();
        if fragment.is_empty() || !self.items.is_empty() {
            Some(dir)
        } else {
            Some(self.expand(self.input.trim()))
        }
    }

    /// The highlighted candidate; `None` on the "start here" row.
    fn selected_item(&self) -> Option<&String> {
        if self.browsing() {
            self.items.get(self.selected.checked_sub(1)?)
        } else {
            self.items.get(self.selected)
        }
    }

    /// The input line that browses `dir`: `~`-abbreviated, ending in `/`.
    fn dir_input(&self, dir: &str) -> String {
        let shown = abbreviate_home(dir, &self.home);
        if shown.ends_with('/') {
            shown
        } else {
            format!("{shown}/")
        }
    }

    /// Browse the highlighted candidate.
    fn descend(&mut self) -> Outcome {
        match self.selected_item() {
            Some(item) => {
                self.input = self.dir_input(item);
                self.refilter()
            }
            None => Outcome::None,
        }
    }

    /// Browse the parent of the listed directory (stops at `/`).
    fn go_up(&mut self) -> Outcome {
        let (dir, _) = self.path_parts();
        let parent = match dir.rfind('/') {
            Some(0) | None => "/",
            Some(i) => &dir[..i],
        };
        self.input = self.dir_input(parent);
        self.refilter()
    }

    /// Open the directory step in browse mode at the host's home (`~/`).
    /// Call right after [`Wizard::on_host_connected`]; returns the `ListDir`
    /// for the home directory.
    pub fn browse_home(&mut self) -> Outcome {
        self.input = "~/".to_owned();
        self.refilter()
    }

    /// Handle a key press.
    pub fn on_key(&mut self, key: &KeyEvent) -> Outcome {
        if key.kind == KeyEventKind::Release {
            return Outcome::None;
        }
        // Step 0: host selection.
        if let Some(step) = &mut self.host_step {
            return step.on_key(key);
        }
        // Step 2: resume picker.
        if let Some(step) = &mut self.resume {
            let plain = key.modifiers.difference(KeyModifiers::SHIFT).is_empty();
            return match key.code {
                KeyCode::Esc => {
                    if step.filter.is_empty() {
                        self.resume = None;
                    } else {
                        step.filter.clear();
                        step.selected = 0;
                    }
                    Outcome::None
                }
                KeyCode::Backspace => {
                    step.filter.pop();
                    step.selected = 0;
                    Outcome::None
                }
                KeyCode::Char('u') if key.modifiers == KeyModifiers::CONTROL => {
                    step.filter.clear();
                    step.selected = 0;
                    Outcome::None
                }
                KeyCode::Up => {
                    step.selected = step.selected.saturating_sub(1);
                    Outcome::None
                }
                KeyCode::Down => {
                    let max = step.filtered_count();
                    step.selected = (step.selected + 1).min(max);
                    Outcome::None
                }
                KeyCode::Left if self.proxy_options.len() > 1 => {
                    if self.proxy_selected > 0 {
                        self.proxy_selected -= 1;
                    } else {
                        self.proxy_selected = self.proxy_options.len().saturating_sub(1);
                    }
                    Outcome::None
                }
                KeyCode::Right if self.proxy_options.len() > 1 => {
                    self.proxy_selected =
                        (self.proxy_selected + 1) % self.proxy_options.len().max(1);
                    Outcome::None
                }
                KeyCode::Enter => {
                    let cwd = step.cwd.clone();
                    // selected=0 means "New session", 1+ is offset into filtered list
                    let resume = step
                        .selected
                        .checked_sub(1)
                        .and_then(|i| step.filtered_session(i))
                        .map(|s| s.id.clone());
                    Outcome::Spawn {
                        cwd,
                        resume,
                        proxy: self.selected_proxy().map(str::to_owned),
                    }
                }
                KeyCode::Char(c) if plain => {
                    step.filter.push(c);
                    step.selected = 0;
                    Outcome::None
                }
                _ => Outcome::None,
            };
        }
        if self.pending.is_some() {
            // Waiting for the session list: only allow backing out.
            if key.code == KeyCode::Esc {
                self.pending = None;
            }
            return Outcome::None;
        }
        let plain = key.modifiers.difference(KeyModifiers::SHIFT).is_empty();
        match key.code {
            KeyCode::Esc => Outcome::Cancel,
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                Outcome::None
            }
            KeyCode::Down => {
                // Browse mode has the "start here" row above the items.
                let last = if self.browsing() {
                    self.items.len()
                } else {
                    self.items.len().saturating_sub(1)
                };
                self.selected = (self.selected + 1).min(last);
                Outcome::None
            }
            // The proxy toggle is also available in the directory step:
            // when the input box is empty, Left/Right cycle the proxy profile.
            KeyCode::Left if self.proxy_options.len() > 1 && self.input.is_empty() => {
                if self.proxy_selected > 0 {
                    self.proxy_selected -= 1;
                } else {
                    self.proxy_selected = self.proxy_options.len().saturating_sub(1);
                }
                Outcome::None
            }
            KeyCode::Right if self.proxy_options.len() > 1 && self.input.is_empty() => {
                self.proxy_selected = (self.proxy_selected + 1) % self.proxy_options.len().max(1);
                Outcome::None
            }
            // Browse mode: → opens the highlighted directory, ← goes up. The
            // proxy arms above win only while the input is empty, which never
            // holds in browse mode.
            KeyCode::Right if self.browsing() => self.descend(),
            KeyCode::Left if self.browsing() => self.go_up(),
            KeyCode::Tab => self.descend(),
            KeyCode::Enter => {
                let dir = match (self.here_dir(), self.selected_item()) {
                    (Some(here), _) if self.here_selected() => here,
                    (_, Some(item)) => item.clone(),
                    _ if !self.input.trim().is_empty() => self.expand(self.input.trim()),
                    _ => return Outcome::None,
                };
                self.pending = Some(dir.clone());
                Outcome::ChooseDir(dir)
            }
            KeyCode::Backspace => {
                let input = self.input.trim();
                if matches!(input, "" | "~/" | "/") {
                    // Backspace on an empty or root input: return to the first
                    // screen (item E: step history / back navigation).
                    if let Some(hs) = self.saved_host_step.take() {
                        self.host_step = Some(hs);
                        self.input.clear();
                        self.items.clear();
                        self.selected = 0;
                        self.completions.clear();
                        self.listed = None;
                        return Outcome::None;
                    }
                }
                if input.len() > 1 && input.ends_with('/') {
                    // Right after a `/`: go up one level (drop the last component).
                    let kept = input.trim_end_matches('/').rfind('/').map_or(0, |i| i + 1);
                    self.input.truncate(kept);
                } else {
                    self.input.pop();
                }
                self.refilter()
            }
            KeyCode::Char('u') if key.modifiers == KeyModifiers::CONTROL => {
                self.input.clear();
                self.refilter()
            }
            KeyCode::Char(c) if plain => {
                self.input.push(c);
                self.refilter()
            }
            _ => Outcome::None,
        }
    }

    /// Insert pasted text into the active step's input.
    ///
    /// In the host step, paste sets the host filter; otherwise it goes into
    /// the directory input.
    pub fn on_paste(&mut self, text: &str) -> Outcome {
        // Step 0: paste into host filter.
        if let Some(step) = &mut self.host_step {
            step.on_paste(text);
            return Outcome::None;
        }
        if self.resume.is_some() || self.pending.is_some() {
            return Outcome::None;
        }
        let line = text.lines().next().unwrap_or("");
        if line.starts_with('/') || line.starts_with('~') {
            // A pasted path replaces the input instead of extending `~/`.
            self.input.clear();
        }
        self.input.push_str(line);
        self.refilter()
    }

    /// Display form of a candidate (`~`-abbreviated).
    pub fn display(&self, path: &str) -> String {
        abbreviate_home(path, &self.home)
    }

    /// Recompute `items` from the input. Returns `ListDir` when the input is
    /// a path whose parent directory hasn't been listed yet.
    fn refilter(&mut self) -> Outcome {
        let mut outcome = Outcome::None;
        let mut items: Vec<String> = Vec::new();
        let input = self.input.trim().to_owned();
        let reveal_hidden = self.hidden_visible();

        let browsing = self.browsing();
        if browsing {
            let (parent, base) = self.path_parts();
            let base = base.to_lowercase();
            if self.listed.as_deref() != Some(parent.as_str()) {
                self.listed = Some(parent.clone());
                self.completions.clear();
                outcome = Outcome::ListDir(parent);
            }
            let mut matching: Vec<&String> = self
                .completions
                .iter()
                .filter(|p| {
                    let name = p.rsplit('/').next().unwrap_or("").to_lowercase();
                    name.starts_with(&base) && (reveal_hidden || !name.starts_with('.'))
                })
                .collect();
            // Exact basename first, then alphabetical.
            matching.sort_by_key(|p| {
                let name = p.rsplit('/').next().unwrap_or("").to_lowercase();
                (name != base, name)
            });
            items.extend(matching.into_iter().cloned());
        }

        // Seeds only feed the fuzzy search; a browsed path lists just its directory.
        let mut scored: Vec<(i64, &String)> = self
            .seeds
            .iter()
            .filter(|_| !browsing)
            .filter_map(|s| {
                if !reveal_hidden && is_hidden_path(s) {
                    return None;
                }
                let display = abbreviate_home(s, &self.home);
                let fuzzy = fuzzy_score(&input, s).max(fuzzy_score(&input, &display))?;
                // Layer frecency bonus on top of fuzzy score.  When the query
                // is empty every fuzzy score is 0, so frecency dominates and
                // dirs sort as: claude-active > recently-used > git-repo >
                // other > hidden.  When typing, fuzzy dominates and frecency
                // is a tie-breaker.
                let frecency = self.meta.get(s.as_str()).map_or(0i64, |m| {
                    let mut bonus = 0i64;
                    if m.hidden { bonus -= 100_000; }
                    if m.claude_at.is_some() { bonus += 200; }
                    if m.recently_used { bonus += 100; }
                    if m.git.is_some() { bonus += 50; }
                    bonus
                });
                Some((fuzzy + frecency, s))
            })
            .collect();
        // Stable: equal scores keep seed (recency) order.
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        for (_, s) in scored {
            if !items.contains(s) {
                items.push(s.clone());
            }
        }

        self.items = items;
        if browsing {
            // Row 0 is "start here"; with a fragment typed, the best match
            // (first child) is preselected instead.
            let has_fragment = !self.query_base().is_empty();
            self.selected = usize::from(has_fragment && !self.items.is_empty());
        } else {
            self.selected = self.selected.min(self.items.len().saturating_sub(1));
        }
        outcome
    }

    /// Expand `~` and drop a trailing slash.
    fn expand(&self, path: &str) -> String {
        let full = match path.strip_prefix('~') {
            Some(rest) if rest.is_empty() || rest.starts_with('/') => {
                format!("{}{rest}", self.home)
            }
            _ => path.to_owned(),
        };
        trim_slash(&full).to_owned()
    }
}

/// One resume-picker row for `s`: `{title or last prompt or id} · {age} · {n} msgs`.
pub fn resume_label(s: &ClaudeSession, now: u64) -> String {
    let what = s
        .title
        .as_deref()
        .or(s.last_prompt.as_deref())
        .unwrap_or(&s.id);
    let what = what.lines().next().unwrap_or("");
    format!(
        "{what}  ·  {} ago  ·  {} msgs",
        fmt_age(now.saturating_sub(s.modified)),
        s.messages
    )
}

/// Whether the final component of `path` starts with `.`.
fn is_hidden_path(path: &str) -> bool {
    trim_slash(path)
        .rsplit('/')
        .next()
        .is_some_and(|n| n.starts_with('.'))
}

fn trim_slash(path: &str) -> &str {
    match path.trim_end_matches('/') {
        "" if path.starts_with('/') => "/",
        trimmed => trimmed,
    }
}

fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn type_str(w: &mut Wizard, s: &str) -> Vec<Outcome> {
        s.chars()
            .map(|c| w.on_key(&press(KeyCode::Char(c))))
            .collect()
    }

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// Create a wizard that has already completed the host step (starts on
    /// the directory step). Convenience wrapper for tests that don't care
    /// about the host step.
    fn wizard_local(seeds: Vec<String>, home: &str) -> Wizard {
        let seeds_clone = seeds.clone();
        let mut w = Wizard::new(seeds, home.into(), "local", &[], &[], None);
        // Advance past the host step by picking "local".
        w.on_host_connected("local", home, seeds_clone);
        w
    }

    #[test]
    fn fuzzy_requires_subsequence() {
        assert!(fuzzy_score("clo", "claudio").is_some());
        assert!(fuzzy_score("xyz", "claudio").is_none());
        assert!(fuzzy_score("oic", "claudio").is_none());
        assert_eq!(fuzzy_score("", "anything"), Some(0));
    }

    #[test]
    fn fuzzy_prefers_contiguous_boundary_and_exact() {
        let contiguous = fuzzy_score("clau", "/r/claudio").unwrap();
        let scattered = fuzzy_score("clau", "/r/cxlxaxu").unwrap();
        assert!(contiguous > scattered);
        let boundary = fuzzy_score("api", "/srv/api").unwrap();
        let inside = fuzzy_score("api", "/srv/rapid").unwrap();
        assert!(boundary > inside);
        assert!(fuzzy_score("/srv", "/srv").unwrap() > fuzzy_score("/srv", "/srv2").unwrap());
        assert!(fuzzy_score("CLAU", "claudio").is_some(), "case-insensitive");
    }

    #[test]
    fn assemble_orders_and_dedupes() {
        let seeds = assemble(
            Some("/w/a/"),
            &strings(&["/w/b", "/w/a", "/w/c"]),
            &strings(&["/w/c", "/w/d", "/w/b/"]),
        );
        assert_eq!(seeds, strings(&["/w/a", "/w/b", "/w/c", "/w/d"]));
        assert_eq!(assemble(None, &[], &strings(&["/"])), strings(&["/"]));
    }

    #[test]
    fn typing_filters_and_enter_chooses_highlighted() {
        let mut w = wizard_local(
            strings(&["/home/u/repos/claudio", "/srv/app", "/home/u/docs"]),
            "/home/u",
        );
        assert_eq!(w.items.len(), 3);
        type_str(&mut w, "app");
        assert_eq!(w.items, strings(&["/srv/app"]));
        assert_eq!(
            w.on_key(&press(KeyCode::Enter)),
            Outcome::ChooseDir("/srv/app".into())
        );
        assert_eq!(w.pending.as_deref(), Some("/srv/app"));
    }

    #[test]
    fn enter_with_no_match_takes_the_literal_input() {
        let mut w = wizard_local(vec![], "/home/u");
        type_str(&mut w, "zzz");
        assert!(w.items.is_empty());
        assert_eq!(
            w.on_key(&press(KeyCode::Enter)),
            Outcome::ChooseDir("zzz".into())
        );
    }

    #[test]
    fn path_input_requests_listdir_and_offers_matching_subdirs() {
        let mut w = wizard_local(strings(&["/srv/app"]), "/home/u");
        let outcomes = type_str(&mut w, "~/re");
        assert_eq!(outcomes[0], Outcome::ListDir("/home/u".into()));
        assert!(
            outcomes[1..].iter().all(|o| *o == Outcome::None),
            "listed once per parent"
        );
        w.set_dir_entries(
            "/home/u",
            &[
                DirEntry::simple("repos", true),
                DirEntry::simple("readme.md", false),
                DirEntry::simple("Desktop", true),
                DirEntry::simple("reports", true),
            ],
        );
        assert_eq!(w.items[..2], strings(&["/home/u/reports", "/home/u/repos"]));
        // The best match (first child) is preselected; Down moves to `repos`.
        assert_eq!(w.selected, 1);
        w.on_key(&press(KeyCode::Down));
        // Tab opens the highlighted entry and lists it.
        assert_eq!(
            w.on_key(&press(KeyCode::Tab)),
            Outcome::ListDir("/home/u/repos".into())
        );
        assert_eq!(w.input, "~/repos/");
    }

    #[test]
    fn stale_dir_entries_are_ignored() {
        let mut w = wizard_local(vec![], "/h");
        type_str(&mut w, "/a/");
        w.set_dir_entries("/b", &[DirEntry::simple("x", true)]);
        assert!(w.items.is_empty());
    }

    #[test]
    fn resume_step_lists_newest_first_and_picks() {
        let mut w = wizard_local(strings(&["/w"]), "/h");
        w.on_key(&press(KeyCode::Enter));
        let s = |id: &str, modified| ClaudeSession {
            id: id.into(),
            title: None,
            last_prompt: None,
            modified,
            messages: 3,
        };
        assert_eq!(
            w.set_claude_sessions("/other", vec![s("x", 1)]),
            Outcome::None,
            "stale reply"
        );
        assert_eq!(
            w.set_claude_sessions("/w", vec![s("old", 1), s("new", 9)]),
            Outcome::None
        );
        let step = w.resume.as_ref().unwrap();
        assert_eq!(step.sessions[0].id, "new");
        assert_eq!(
            w.on_key(&press(KeyCode::Enter)),
            Outcome::Spawn {
                cwd: "/w".into(),
                resume: None,
                proxy: None
            },
            "first row is `+ New session`"
        );
        w.on_key(&press(KeyCode::Down));
        w.on_key(&press(KeyCode::Down));
        assert_eq!(
            w.on_key(&press(KeyCode::Enter)),
            Outcome::Spawn {
                cwd: "/w".into(),
                resume: Some("old".into()),
                proxy: None
            }
        );
        // Esc goes back to the directory step.
        w.on_key(&press(KeyCode::Esc));
        assert!(w.resume.is_none());
        assert_eq!(w.on_key(&press(KeyCode::Esc)), Outcome::Cancel);
    }

    #[test]
    fn no_sessions_spawns_directly() {
        let mut w = wizard_local(strings(&["/w"]), "/h");
        w.on_key(&press(KeyCode::Enter));
        assert_eq!(
            w.set_claude_sessions("/w", vec![]),
            Outcome::Spawn {
                cwd: "/w".into(),
                resume: None,
                proxy: None
            }
        );
    }

    #[test]
    fn resume_label_prefers_title_then_prompt_then_id() {
        let mut s = ClaudeSession {
            id: "abc".into(),
            title: None,
            last_prompt: Some("fix it\nplease".into()),
            modified: 100,
            messages: 7,
        };
        assert_eq!(resume_label(&s, 400), "fix it  ·  5m ago  ·  7 msgs");
        s.title = Some("Refactor".into());
        assert!(resume_label(&s, 400).starts_with("Refactor  ·"));
        s.title = None;
        s.last_prompt = None;
        assert!(resume_label(&s, 400).starts_with("abc  ·"));
    }

    #[test]
    fn set_claude_sessions_error_clears_pending_without_spawning() {
        let mut w = wizard_local(strings(&["/w"]), "/h");
        w.on_key(&press(KeyCode::Enter));
        // Simulate a pending request.
        assert_eq!(w.pending.as_deref(), Some("/w"));
        // An error should clear pending but not spawn.
        w.set_claude_sessions_error("/w");
        assert!(w.pending.is_none());
        assert!(w.resume.is_none());
    }

    #[test]
    fn set_claude_sessions_error_ignores_stale_cwd() {
        let mut w = wizard_local(strings(&["/w"]), "/h");
        w.on_key(&press(KeyCode::Enter));
        // Error for a different cwd is ignored.
        w.set_claude_sessions_error("/other");
        assert_eq!(w.pending.as_deref(), Some("/w"), "pending should remain");
    }

    #[test]
    fn proxy_toggle_available_in_directory_step() {
        let mut w = Wizard::new(
            strings(&["/w"]),
            "/h".into(),
            "local",
            &[],
            &["myproxy".to_owned()],
            None,
        );
        w.on_host_connected("local", "/h", strings(&["/w"]));
        // proxy_options = ["none", "myproxy"], selected = 0 initially
        assert_eq!(w.proxy_selected, 0);
        // Left/Right should cycle the proxy when input is empty.
        w.on_key(&KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(w.proxy_selected, 1);
        w.on_key(&KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(w.proxy_selected, 0);
    }

    #[test]
    fn host_paste_sets_host_input() {
        let mut w = Wizard::new(
            vec![],
            "/h".into(),
            "local",
            &["server.example.com".to_owned()],
            &[],
            None,
        );
        // Host step is active; paste should go to it.
        assert!(w.host_step.is_some());
        w.on_paste("server.example.com");
        let hs = w.host_step.as_ref().unwrap();
        assert_eq!(hs.input, "server.example.com");
    }

    #[test]
    fn host_filtering_uses_fuzzy_scorer() {
        // Fuzzy: "srv" matches "my-server" (subsequence).
        let mut w = Wizard::new(
            vec![],
            "/h".into(),
            "local",
            &["my-server".to_owned(), "production".to_owned()],
            &[],
            None,
        );
        let hs = w.host_step.as_mut().unwrap();
        // Type a fuzzy query that matches "my-server" but not "production".
        hs.input = "msr".to_owned();
        hs.refilter();
        // "my-server" should appear (m-s-r is a subsequence of my-server).
        // Note: "local" is always first in candidates.
        assert!(
            hs.items.iter().any(|h| h == "my-server"),
            "expected my-server in items: {:?}",
            hs.items
        );
    }

    #[test]
    fn each_wizard_gets_unique_generation() {
        let w1 = Wizard::new(vec![], "/h".into(), "local", &[], &[], None);
        let w2 = Wizard::new(vec![], "/h".into(), "local", &[], &[], None);
        assert_ne!(w1.generation, w2.generation);
    }

    #[test]
    fn frecency_hidden_sorts_last() {
        let seeds = strings(&["/visible", "/hidden-dir"]);
        let mut w = wizard_local(seeds, "/h");
        // Mark /hidden-dir as hidden in meta.
        w.meta.entry("/hidden-dir".to_owned()).or_default().hidden = true;
        // Trigger refilter with empty input so frecency dominates.
        w.input.clear();
        w.refilter();
        // /visible should appear before /hidden-dir.
        let pos_visible = w.items.iter().position(|p| p == "/visible");
        let pos_hidden = w.items.iter().position(|p| p == "/hidden-dir");
        assert!(
            pos_visible < pos_hidden,
            "visible should sort before hidden: {:?}",
            w.items
        );
    }

    #[test]
    fn frecency_claude_active_sorts_first() {
        let seeds = strings(&["/regular", "/claude-active", "/git-only"]);
        let mut w = wizard_local(seeds, "/h");
        // /claude-active has a recent claude session.
        w.meta.entry("/claude-active".to_owned()).or_default().claude_at = Some(1_000_000);
        // /git-only has a git repo.
        w.meta.entry("/git-only".to_owned()).or_default().git = Some("main".to_owned());
        w.input.clear();
        w.refilter();
        // Order should be: claude-active, git-only, regular.
        let pos_claude = w.items.iter().position(|p| p == "/claude-active").unwrap();
        let pos_git = w.items.iter().position(|p| p == "/git-only").unwrap();
        let pos_regular = w.items.iter().position(|p| p == "/regular").unwrap();
        assert!(
            pos_claude < pos_git,
            "claude-active should sort before git-only: {:?}",
            w.items
        );
        assert!(
            pos_git < pos_regular,
            "git-only should sort before regular: {:?}",
            w.items
        );
    }

    #[test]
    fn recently_used_badge_set_from_new_with_recent() {
        let seeds = strings(&["/a", "/b", "/c"]);
        let recent = strings(&["/b"]);
        let w = Wizard::new_with_recent(
            seeds, &recent, "/h".into(), "local", &[], &[], None,
        );
        assert!(
            w.meta.get("/b").map_or(false, |m| m.recently_used),
            "/b should be marked recently_used"
        );
        assert!(
            !w.meta.get("/a").map_or(false, |m| m.recently_used),
            "/a should not be recently_used"
        );
    }

    // ── HostStep selection rules (Bug 1) ──────────────────────────────────────

    fn make_host_step(hosts: &[&str], local_dirs: &[&str]) -> HostStep {
        let hosts: Vec<String> = hosts.iter().map(|s| s.to_string()).collect();
        let dirs: Vec<String> = local_dirs.iter().map(|s| s.to_string()).collect();
        HostStep::with_local_dirs("local", &hosts, &dirs)
    }

    /// An empty query always restores the default: Local focus on "Explore".
    #[test]
    fn host_step_empty_query_selects_explore() {
        let mut hs = make_host_step(&["devbox", "prod"], &[]);
        // Type something then clear.
        hs.input = "devbox".to_owned();
        hs.refilter();
        hs.input.clear();
        hs.refilter();
        assert_eq!(hs.focus, HostSection::Local);
        assert_eq!(hs.local_selected, 0, "should be on Explore");
        assert!(!hs.connect_raw);
    }

    /// Typing a known host switches focus to Remote and selects that host.
    #[test]
    fn host_step_typing_known_host_switches_to_remote() {
        let mut hs = make_host_step(&["devbox", "prod"], &[]);
        hs.input = "devbox".to_owned();
        hs.refilter();
        assert_eq!(hs.focus, HostSection::Remote, "should switch to Remote");
        assert_eq!(hs.selected, 0, "top of REMOTE section");
        assert!(hs.items.iter().any(|h| h == "devbox"), "devbox should be visible");
        assert!(!hs.connect_raw);
    }

    /// Typing a host-like token that matches no candidate shows connect_raw.
    #[test]
    fn host_step_unknown_host_shows_connect_raw() {
        let mut hs = make_host_step(&["prod"], &[]);
        hs.input = "newhost".to_owned();
        hs.refilter();
        assert!(hs.connect_raw, "connect_raw should be set for unknown host-like query");
        assert_eq!(hs.focus, HostSection::Remote);
        assert_eq!(hs.remote_len(), 1, "only the connect_raw row");
        // Enter should use the raw input.
        let out = hs.on_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(out, Outcome::ConnectHost("newhost".to_owned()));
    }

    /// "Explore local dirs…" must NOT stay selected when query doesn't match it.
    #[test]
    fn host_step_explore_not_selected_for_host_query() {
        let mut hs = make_host_step(&["devbox"], &[]);
        hs.input = "devbox".to_owned();
        hs.refilter();
        // Must not be on Local / Explore.
        assert!(
            hs.focus != HostSection::Local || hs.local_selected != 0,
            "Explore must not be selected when query is 'devbox'"
        );
    }

    /// "Explore" IS selected when query matches its text.
    #[test]
    fn host_step_explore_selected_when_query_matches_its_text() {
        let mut hs = make_host_step(&["devbox"], &[]);
        hs.input = "explore".to_owned();
        hs.refilter();
        assert_eq!(hs.focus, HostSection::Local);
        assert_eq!(hs.local_selected, 0, "Explore should be selected");
    }

    /// When a local dir scores best, focus switches to Local dir (not Explore).
    #[test]
    fn host_step_local_dir_scores_best_selects_it() {
        // No remote hosts, one local dir that exists and matches the query.
        // /tmp always exists on Unix; query "tmp" fuzzy-matches it and doesn't
        // look like a host (it's short and matches the dir), so focus goes Local.
        let mut hs = make_host_step(&[], &["/tmp"]);
        // Ensure the dir is in local_dirs (it exists, so it wasn't filtered).
        assert!(
            hs.local_items.contains(&"/tmp".to_owned()),
            "/tmp should survive the existence filter"
        );
        hs.input = "tmp".to_owned();
        hs.refilter();
        // /tmp matches the query; there are no remote candidates.
        // "tmp" does NOT match "explore local dirs", so the local dir row wins.
        assert_eq!(hs.focus, HostSection::Local);
        // local_selected=1 means the first local_items entry.
        assert_eq!(hs.local_selected, 1);
        assert!(!hs.connect_raw);
    }

    /// looks_like_host rejects paths and whitespace.
    #[test]
    fn looks_like_host_basic() {
        assert!(super::looks_like_host("devbox"));
        assert!(super::looks_like_host("user@host"));
        assert!(super::looks_like_host("my-server"));
        assert!(!super::looks_like_host("/tmp"), "paths are not hosts");
        assert!(!super::looks_like_host("~/foo"), "~ paths are not hosts");
        assert!(!super::looks_like_host("a b"), "spaces disqualify");
        assert!(!super::looks_like_host("x"), "single char too short");
        assert!(!super::looks_like_host(""), "empty is not a host");
    }

    /// HostStep::with_local_dirs silently drops non-existing paths.
    #[test]
    fn host_step_drops_nonexistent_local_dirs() {
        // /tmp always exists; /nonexistent_claudio_test_dir should not.
        let hs = make_host_step(
            &[],
            &["/tmp", "/nonexistent_claudio_test_dir_xyzzy"],
        );
        assert!(
            hs.local_items.iter().any(|d| d == "/tmp"),
            "/tmp should be kept"
        );
        assert!(
            !hs.local_items
                .iter()
                .any(|d| d == "/nonexistent_claudio_test_dir_xyzzy"),
            "non-existing dir should be dropped"
        );
    }

    // ── Hidden directories ────────────────────────────────────────────────────

    fn names(w: &Wizard) -> Vec<&str> {
        w.items
            .iter()
            .map(|p| p.rsplit('/').next().unwrap_or(""))
            .collect()
    }

    fn listed_home_wizard() -> Wizard {
        let mut w = wizard_local(vec![], "/home/u");
        type_str(&mut w, "~/");
        w.set_dir_entries(
            "/home/u",
            &[
                DirEntry::simple(".config", true),
                DirEntry::simple(".secret", true),
                DirEntry::simple("repos", true),
            ],
        );
        w
    }

    #[test]
    fn hidden_dirs_are_filtered_by_default() {
        let w = listed_home_wizard();
        assert!(!w.show_hidden);
        assert_eq!(names(&w), vec!["repos"]);

        // Seeds with a hidden final component are hidden too.
        let w = wizard_local(strings(&["/w/app", "/w/.cache", "/home/u/.dotproj"]), "/home/u");
        assert_eq!(w.items, strings(&["/w/app"]));
    }

    #[test]
    fn toggle_shows_and_hides_hidden_dirs() {
        let mut w = listed_home_wizard();
        assert_eq!(w.toggle_hidden(), Outcome::None, "same dir: no new ListDir");
        assert!(w.show_hidden);
        assert_eq!(names(&w), vec![".config", ".secret", "repos"]);
        w.toggle_hidden();
        assert_eq!(names(&w), vec!["repos"]);

        // Seeds appear after the toggle as well.
        let mut w = wizard_local(strings(&["/w/app", "/w/.cache"]), "/h");
        w.toggle_hidden();
        assert_eq!(w.items, strings(&["/w/app", "/w/.cache"]));
    }

    #[test]
    fn toggle_is_remembered_while_typing() {
        let mut w = listed_home_wizard();
        w.toggle_hidden();
        type_str(&mut w, "r");
        assert!(w.show_hidden);
        w.on_key(&press(KeyCode::Backspace));
        assert_eq!(names(&w), vec![".config", ".secret", "repos"]);
    }

    #[test]
    fn dot_query_reveals_hidden_dirs_automatically() {
        let mut w = listed_home_wizard();
        assert!(!w.hidden_visible());
        type_str(&mut w, ".con");
        assert!(w.hidden_visible(), "auto-revealed");
        assert!(!w.show_hidden, "the toggle itself is untouched");
        assert_eq!(names(&w), vec![".config"]);
        // Backspacing out of the dot segment hides them again.
        for _ in 0..4 {
            w.on_key(&press(KeyCode::Backspace));
        }
        assert_eq!(names(&w), vec!["repos"]);

        // A bare (non-path) query starting with `.` reveals hidden seeds.
        let mut w = wizard_local(strings(&["/w/app", "/w/.cache"]), "/h");
        type_str(&mut w, ".ca");
        assert_eq!(w.items, strings(&["/w/.cache"]));
    }

    #[test]
    fn toggle_is_a_noop_outside_the_directory_step() {
        let mut w = Wizard::new(strings(&["/w"]), "/h".into(), "local", &[], &[], None);
        assert!(w.host_step.is_some());
        w.toggle_hidden();
        assert!(!w.show_hidden, "host step ignores the toggle");
    }

    // ── Directory browser ─────────────────────────────────────────────────────

    /// A wizard past the host step, browsing `/home/u` (`~/`) with `entries`
    /// listed. Returns the `ListDir` outcome that opening the browser produced.
    fn browser(entries: &[&str]) -> (Wizard, Outcome) {
        let mut w = Wizard::new(strings(&["/srv/app"]), "/home/u".into(), "local", &[], &[], None);
        w.on_host_connected("local", "/home/u", strings(&["/srv/app"]));
        let opened = w.browse_home();
        let entries: Vec<DirEntry> = entries.iter().map(|n| DirEntry::simple(*n, true)).collect();
        w.set_dir_entries("/home/u", &entries);
        (w, opened)
    }

    #[test]
    fn explore_opens_the_browser_at_home() {
        let (w, opened) = browser(&["repos", "docs"]);
        assert_eq!(opened, Outcome::ListDir("/home/u".into()));
        assert_eq!(w.input, "~/");
        assert!(w.browsing());
        assert_eq!(w.items, strings(&["/home/u/docs", "/home/u/repos"]));
        assert!(w.here_selected(), "`start here` is preselected on a trailing slash");
        assert_eq!(w.here_dir().as_deref(), Some("/home/u"));
        assert_eq!(w.display("/home/u"), "~");
    }

    #[test]
    fn browser_uses_the_remote_hosts_home() {
        let mut w = Wizard::new(vec![], "/home/u".into(), "local", &["box".into()], &[], None);
        w.on_host_connected("box", "/home/remote", vec![]);
        assert_eq!(w.browse_home(), Outcome::ListDir("/home/remote".into()));
        w.set_dir_entries("/home/remote", &[DirEntry::simple("src", true)]);
        assert_eq!(
            w.on_key(&press(KeyCode::Enter)),
            Outcome::ChooseDir("/home/remote".into())
        );
    }

    #[test]
    fn enter_on_start_here_picks_the_listed_dir() {
        let (mut w, _) = browser(&["repos"]);
        assert_eq!(
            w.on_key(&press(KeyCode::Enter)),
            Outcome::ChooseDir("/home/u".into())
        );
        assert_eq!(w.pending.as_deref(), Some("/home/u"));
    }

    #[test]
    fn enter_on_a_child_picks_the_child() {
        let (mut w, _) = browser(&["docs", "repos"]);
        w.on_key(&press(KeyCode::Down));
        assert!(!w.here_selected());
        assert_eq!(
            w.on_key(&press(KeyCode::Enter)),
            Outcome::ChooseDir("/home/u/docs".into())
        );
    }

    #[test]
    fn a_fragment_preselects_the_best_child_and_start_here_is_the_parent() {
        let (mut w, _) = browser(&["docs", "repos", "reports"]);
        type_str(&mut w, "rep");
        assert_eq!(w.items, strings(&["/home/u/reports", "/home/u/repos"]));
        assert_eq!(w.selected, 1, "first match");
        // Up reaches `start here`, which is the listed dir, not the fragment.
        w.on_key(&press(KeyCode::Up));
        assert!(w.here_selected());
        assert_eq!(w.here_dir().as_deref(), Some("/home/u"));
        assert_eq!(
            w.on_key(&press(KeyCode::Enter)),
            Outcome::ChooseDir("/home/u".into())
        );
    }

    #[test]
    fn unmatched_fragment_falls_back_to_the_typed_path() {
        let (mut w, _) = browser(&["docs"]);
        type_str(&mut w, "new");
        assert!(w.items.is_empty());
        assert_eq!(w.here_dir().as_deref(), Some("/home/u/new"));
        assert_eq!(
            w.on_key(&press(KeyCode::Enter)),
            Outcome::ChooseDir("/home/u/new".into())
        );
    }

    #[test]
    fn right_and_tab_descend_into_the_child() {
        let (mut w, _) = browser(&["docs", "repos"]);
        w.on_key(&press(KeyCode::Down));
        w.on_key(&press(KeyCode::Down));
        assert_eq!(
            w.on_key(&press(KeyCode::Right)),
            Outcome::ListDir("/home/u/repos".into())
        );
        assert_eq!(w.input, "~/repos/");
        assert!(w.here_selected(), "`start here` is selected after descending");
        w.set_dir_entries("/home/u/repos", &[DirEntry::simple("claudio", true)]);
        w.on_key(&press(KeyCode::Down));
        assert_eq!(
            w.on_key(&press(KeyCode::Tab)),
            Outcome::ListDir("/home/u/repos/claudio".into())
        );
        assert_eq!(w.input, "~/repos/claudio/");
        assert_eq!(w.here_dir().as_deref(), Some("/home/u/repos/claudio"));
        // → on `start here` has nothing to open.
        assert_eq!(w.on_key(&press(KeyCode::Right)), Outcome::None);
        assert_eq!(w.input, "~/repos/claudio/");
    }

    #[test]
    fn left_goes_up_and_stops_at_root() {
        let (mut w, _) = browser(&["repos"]);
        // Above home we leave the `~` form.
        assert_eq!(
            w.on_key(&press(KeyCode::Left)),
            Outcome::ListDir("/home".into())
        );
        assert_eq!(w.input, "/home/");
        assert_eq!(w.on_key(&press(KeyCode::Left)), Outcome::ListDir("/".into()));
        assert_eq!(w.input, "/");
        assert_eq!(w.on_key(&press(KeyCode::Left)), Outcome::None, "stays at /");
        assert_eq!(w.input, "/");
        assert_eq!(w.here_dir().as_deref(), Some("/"));
    }

    #[test]
    fn left_from_a_subdir_returns_to_the_parent_with_tilde() {
        let (mut w, _) = browser(&["repos"]);
        type_str(&mut w, "repos/");
        assert_eq!(w.input, "~/repos/");
        assert_eq!(w.on_key(&press(KeyCode::Left)), Outcome::ListDir("/home/u".into()));
        assert_eq!(w.input, "~/");
    }

    #[test]
    fn backspace_after_a_slash_goes_up_one_level() {
        let (mut w, _) = browser(&["repos"]);
        type_str(&mut w, "repos/x/");
        assert_eq!(w.input, "~/repos/x/");
        w.on_key(&press(KeyCode::Backspace));
        assert_eq!(w.input, "~/repos/");
        w.on_key(&press(KeyCode::Backspace));
        assert_eq!(w.input, "~/");
        assert!(w.host_step.is_none(), "`~/` is still the directory step");
    }

    #[test]
    fn backspace_on_a_fragment_removes_one_character() {
        let (mut w, _) = browser(&["repos"]);
        type_str(&mut w, "rep");
        w.on_key(&press(KeyCode::Backspace));
        assert_eq!(w.input, "~/re");
    }

    #[test]
    fn backspace_on_bare_home_returns_to_the_start_screen() {
        let (mut w, _) = browser(&["repos"]);
        assert_eq!(w.on_key(&press(KeyCode::Backspace)), Outcome::None);
        assert!(w.host_step.is_some(), "back on the first screen");
        assert!(w.input.is_empty());
    }

    #[test]
    fn arrows_cycle_the_proxy_only_without_input() {
        let mut w = Wizard::new(vec![], "/h".into(), "local", &[], &["p".to_owned()], None);
        w.on_host_connected("local", "/h", vec![]);
        // Empty input: proxy cycling.
        w.on_key(&press(KeyCode::Right));
        assert_eq!(w.proxy_selected, 1);
        // Browse mode always has input: arrows browse instead.
        w.browse_home();
        w.on_key(&press(KeyCode::Right));
        w.on_key(&press(KeyCode::Left));
        assert_eq!(w.proxy_selected, 1, "unchanged while browsing");
    }

    #[test]
    fn non_path_input_is_still_a_fuzzy_seed_search() {
        let mut w = wizard_local(strings(&["/srv/app", "/home/u/docs"]), "/home/u");
        assert!(!w.browsing());
        assert!(w.here_dir().is_none());
        assert_eq!(w.items.len(), 2);
        type_str(&mut w, "app");
        assert_eq!(w.items, strings(&["/srv/app"]));
        assert_eq!(w.selected, 0, "no `start here` row");
        assert_eq!(w.on_key(&press(KeyCode::Enter)), Outcome::ChooseDir("/srv/app".into()));
    }

    #[test]
    fn seeds_do_not_leak_into_a_browsed_directory() {
        let mut w = wizard_local(strings(&["/home/u/repos/claudio"]), "/home/u");
        type_str(&mut w, "~/");
        w.set_dir_entries("/home/u", &[DirEntry::simple("docs", true)]);
        assert_eq!(w.items, strings(&["/home/u/docs"]));
    }

    #[test]
    fn hidden_toggle_and_dot_fragment_work_in_the_browser() {
        let (mut w, _) = browser(&[".config", "repos"]);
        assert_eq!(names(&w), vec!["repos"]);
        w.toggle_hidden();
        assert_eq!(names(&w), vec![".config", "repos"]);
        w.toggle_hidden();
        type_str(&mut w, ".c");
        assert_eq!(names(&w), vec![".config"]);
        assert_eq!(w.selected, 1);
    }

    #[test]
    fn pasting_a_path_replaces_the_browse_input() {
        let (mut w, _) = browser(&[]);
        assert_eq!(w.on_paste("/srv/app/"), Outcome::ListDir("/srv/app".into()));
        assert_eq!(w.input, "/srv/app/");
        // A non-path paste extends it.
        w.on_paste("x");
        assert_eq!(w.input, "/srv/app/x");
    }
}
