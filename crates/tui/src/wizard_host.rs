//! The wizard's first screen (step 0): LOCAL (explore, recent dirs) and
//! REMOTE (ssh hosts, a typed one), filtered by what is typed. Drawn by
//! `ui/wizard_host.rs`; the wizard itself is `wizard.rs`.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::wizard::{fuzzy_score, Outcome};

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
    pub(crate) local_dirs: Vec<String>,
    /// Selected row in the REMOTE section.
    pub selected: usize,
    /// Filtered candidate list of SSH hosts.
    pub items: Vec<String>,
    /// `Some(host)` while bootstrap + connect_ssh is running.
    pub connecting: Option<String>,
    /// The full unfiltered SSH host list.
    pub(crate) candidates: Vec<String>,
    /// Whether the "Search conversations for “<query>”" row shows (after the
    /// hosts, before "connect to"): the query has at least 2 characters.
    pub search: bool,
    /// `true` when the query looks like a bare hostname / `user@host` but matches no
    /// known candidate.  A synthetic "connect to <query>" row is appended to the
    /// REMOTE section and auto-selected so Enter immediately initiates the connection.
    pub connect_raw: bool,
}

impl HostStep {
    /// Build a host step: `active_host` is pre-selected, `extra` are the SSH
    /// host candidates (from `hosts::candidates()`) and `local_dirs` the
    /// LOCAL section's recent dirs.
    pub fn with_local_dirs(active_host: &str, extra: &[String], local_dirs: &[String]) -> HostStep {
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
        HostStep {
            input: String::new(),
            focus,
            local_selected: 0,
            local_items: local_dirs.to_vec(),
            local_dirs: local_dirs.to_vec(),
            selected: remote_selected,
            items,
            connecting: None,
            candidates,
            search: false,
            connect_raw: false,
        }
    }

    /// Total rows in the LOCAL section (including the "Explore" pseudo-entry).
    pub fn local_len(&self) -> usize {
        1 + self.local_items.len()
    }

    /// Total rows in the REMOTE section.
    ///
    /// Includes the synthetic "Search conversations" and "connect to <query>"
    /// rows when they show.
    pub fn remote_len(&self) -> usize {
        self.items.len() + usize::from(self.search) + usize::from(self.connect_raw)
    }

    /// Whether the "Search conversations" row is highlighted.
    pub fn search_selected(&self) -> bool {
        self.search && self.focus == HostSection::Remote && self.selected == self.items.len()
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
                        } else {
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
                        } else if self.remote_len() > 0 {
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
            KeyCode::Enter => self.enter(),
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

    /// Enter: the highlighted row's action.
    fn enter(&mut self) -> Outcome {
        match self.focus {
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
            HostSection::Remote if self.search_selected() => {
                Outcome::Search(self.input.trim().to_owned())
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

    pub(crate) fn refilter(&mut self) {
        let input = self.input.trim().to_lowercase();
        if input.is_empty() {
            // Empty query → restore unfiltered lists and default selection.
            self.items = self.candidates.clone();
            self.local_items = self.local_dirs.clone();
            self.focus = HostSection::Local;
            self.local_selected = 0;
            self.selected = 0;
            (self.search, self.connect_raw) = (false, false);
            return;
        }

        // ── Filter remote hosts (scored, best first). ──────────────────────────
        let mut scored_remote: Vec<(i64, String)> = self
            .candidates
            .iter()
            .filter_map(|h| Some((fuzzy_score(&input, h)?, h.clone())))
            .collect();
        scored_remote.sort_by_key(|s| std::cmp::Reverse(s.0));
        self.items = scored_remote.iter().map(|(_, h)| h.clone()).collect();

        // ── Filter local dirs (scored, best first). ────────────────────────────
        let mut scored_local: Vec<(i64, String)> = self
            .local_dirs
            .iter()
            .filter_map(|d| Some((fuzzy_score(&input, d)?, d.clone())))
            .collect();
        scored_local.sort_by_key(|s| std::cmp::Reverse(s.0));
        self.local_items = scored_local.iter().map(|(_, d)| d.clone()).collect();

        // ── Score the "Explore local dirs…" pseudo-entry. ─────────────────────
        let explore_score: Option<i64> = fuzzy_score(&input, "explore local dirs");

        // ── Synthetic rows: search (2+ chars); "connect to" if nothing matches. ─
        self.search = input.chars().count() >= 2;
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
                self.local_selected = self.local_selected.min(self.local_len().saturating_sub(1));
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
        // Nothing matched: the first synthetic row (search, else connect_raw).
        if self.search || self.connect_raw {
            self.focus = HostSection::Remote;
            self.selected = 0; // no host rows: the first synthetic one
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
