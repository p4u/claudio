//! The new-session wizard: pick a directory, then optionally a claude
//! conversation to resume.
//!
//! The wizard is a pure state machine. Key handling returns an [`Outcome`]
//! telling the app what to ask the daemon (`ListDir`, `ListClaudeSessions`)
//! or what to spawn; replies are fed back through the `set_*` methods.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::proto::{ClaudeSession, DirEntry};

use super::ui::{abbreviate_home, fmt_age};

/// What the app should do after the wizard handled an input.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    None,
    Cancel,
    /// Request `ListDir` of this absolute path (path completion).
    ListDir(String),
    /// A directory was chosen: request `ListClaudeSessions` for it.
    ChooseDir(String),
    /// Start claude in `cwd`, resuming `resume` if set.
    Spawn { cwd: String, resume: Option<String> },
}

/// Step 2: the resume picker.
#[derive(Debug, Clone)]
pub struct ResumeStep {
    pub cwd: String,
    /// Newest first. The list shown is `+ New session` followed by these.
    pub sessions: Vec<ClaudeSession>,
    pub selected: usize,
}

/// The wizard's state.
#[derive(Debug, Clone)]
pub struct Wizard {
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

impl Wizard {
    /// A wizard over `seeds` (see [`assemble`]); `home` expands `~`.
    pub fn new(seeds: Vec<String>, home: String) -> Wizard {
        let mut w = Wizard {
            input: String::new(),
            items: Vec::new(),
            selected: 0,
            resume: None,
            pending: None,
            seeds,
            completions: Vec::new(),
            listed: None,
            home,
        };
        w.refilter();
        w
    }

    /// Add late-arriving seeds (the `RecentProjects` reply), keeping order.
    pub fn add_seeds(&mut self, more: &[String]) {
        self.seeds = assemble(None, &self.seeds, more);
        self.refilter();
    }

    /// A `ListDir` reply. Ignored unless it answers the latest request.
    pub fn set_dir_entries(&mut self, path: &str, entries: &[DirEntry]) {
        if self.listed.as_deref() != Some(path) {
            return;
        }
        self.completions = entries.iter().filter(|e| e.dir).map(|e| join(path, &e.name)).collect();
        self.refilter();
    }

    /// A `ListClaudeSessions` reply for `cwd`. With no sessions there is
    /// nothing to pick, so the outcome is to spawn a fresh one.
    pub fn set_claude_sessions(&mut self, cwd: &str, mut sessions: Vec<ClaudeSession>) -> Outcome {
        if self.pending.as_deref() != Some(cwd) {
            return Outcome::None;
        }
        self.pending = None;
        if sessions.is_empty() {
            return Outcome::Spawn { cwd: cwd.to_owned(), resume: None };
        }
        sessions.sort_by(|a, b| b.modified.cmp(&a.modified));
        self.resume = Some(ResumeStep { cwd: cwd.to_owned(), sessions, selected: 0 });
        Outcome::None
    }

    /// Handle a key press.
    pub fn on_key(&mut self, key: &KeyEvent) -> Outcome {
        if key.kind == KeyEventKind::Release {
            return Outcome::None;
        }
        if let Some(step) = &mut self.resume {
            return match key.code {
                KeyCode::Esc => {
                    self.resume = None;
                    Outcome::None
                }
                KeyCode::Up => {
                    step.selected = step.selected.saturating_sub(1);
                    Outcome::None
                }
                KeyCode::Down => {
                    step.selected = (step.selected + 1).min(step.sessions.len());
                    Outcome::None
                }
                KeyCode::Enter => Outcome::Spawn {
                    cwd: step.cwd.clone(),
                    resume: step.selected.checked_sub(1).and_then(|i| step.sessions.get(i)).map(|s| s.id.clone()),
                },
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
                self.selected = (self.selected + 1).min(self.items.len().saturating_sub(1));
                Outcome::None
            }
            KeyCode::Tab => match self.items.get(self.selected) {
                Some(item) => {
                    self.input = abbreviate_home(item, &self.home);
                    self.refilter()
                }
                None => Outcome::None,
            },
            KeyCode::Enter => {
                let dir = match self.items.get(self.selected) {
                    Some(item) => item.clone(),
                    None if !self.input.trim().is_empty() => self.expand(self.input.trim()),
                    None => return Outcome::None,
                };
                self.pending = Some(dir.clone());
                Outcome::ChooseDir(dir)
            }
            KeyCode::Backspace => {
                self.input.pop();
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

    /// Insert pasted text into the input line (first line only).
    pub fn on_paste(&mut self, text: &str) -> Outcome {
        if self.resume.is_some() || self.pending.is_some() {
            return Outcome::None;
        }
        self.input.push_str(text.lines().next().unwrap_or(""));
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

        if input.starts_with('/') || input.starts_with('~') {
            let (parent, base) = match input.rfind('/') {
                Some(i) => (self.expand(&input[..=i]), input[i + 1..].to_lowercase()),
                // A bare `~` or `~user`: list home itself.
                None => (self.expand(&input), String::new()),
            };
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
                    name.starts_with(&base) && (base.starts_with('.') || !name.starts_with('.'))
                })
                .collect();
            // Exact basename first, then alphabetical.
            matching.sort_by_key(|p| {
                let name = p.rsplit('/').next().unwrap_or("").to_lowercase();
                (name != base, name)
            });
            items.extend(matching.into_iter().cloned());
        }

        let mut scored: Vec<(i64, &String)> = self
            .seeds
            .iter()
            .filter_map(|s| {
                let display = abbreviate_home(s, &self.home);
                let score = fuzzy_score(&input, s).max(fuzzy_score(&input, &display))?;
                Some((score, s))
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
        self.selected = self.selected.min(self.items.len().saturating_sub(1));
        outcome
    }

    /// Expand `~` and drop a trailing slash.
    fn expand(&self, path: &str) -> String {
        let full = match path.strip_prefix('~') {
            Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("{}{rest}", self.home),
            _ => path.to_owned(),
        };
        trim_slash(&full).to_owned()
    }
}

/// One resume-picker row for `s`: `{title or last prompt or id} · {age} · {n} msgs`.
pub fn resume_label(s: &ClaudeSession, now: u64) -> String {
    let what = s.title.as_deref().or(s.last_prompt.as_deref()).unwrap_or(&s.id);
    let what = what.lines().next().unwrap_or("");
    format!("{what}  ·  {} ago  ·  {} msgs", fmt_age(now.saturating_sub(s.modified)), s.messages)
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
        s.chars().map(|c| w.on_key(&press(KeyCode::Char(c)))).collect()
    }

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
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
        let mut w = Wizard::new(strings(&["/home/u/repos/claudio", "/srv/app", "/home/u/docs"]), "/home/u".into());
        assert_eq!(w.items.len(), 3);
        type_str(&mut w, "app");
        assert_eq!(w.items, strings(&["/srv/app"]));
        assert_eq!(w.on_key(&press(KeyCode::Enter)), Outcome::ChooseDir("/srv/app".into()));
        assert_eq!(w.pending.as_deref(), Some("/srv/app"));
    }

    #[test]
    fn enter_with_no_match_takes_the_literal_input() {
        let mut w = Wizard::new(vec![], "/home/u".into());
        type_str(&mut w, "zzz");
        assert!(w.items.is_empty());
        assert_eq!(w.on_key(&press(KeyCode::Enter)), Outcome::ChooseDir("zzz".into()));
    }

    #[test]
    fn path_input_requests_listdir_and_offers_matching_subdirs() {
        let mut w = Wizard::new(strings(&["/srv/app"]), "/home/u".into());
        let outcomes = type_str(&mut w, "~/re");
        assert_eq!(outcomes[0], Outcome::ListDir("/home/u".into()));
        assert!(outcomes[1..].iter().all(|o| *o == Outcome::None), "listed once per parent");
        w.set_dir_entries(
            "/home/u",
            &[
                DirEntry { name: "repos".into(), dir: true },
                DirEntry { name: "readme.md".into(), dir: false },
                DirEntry { name: "Desktop".into(), dir: true },
                DirEntry { name: "reports".into(), dir: true },
            ],
        );
        assert_eq!(w.items[..2], strings(&["/home/u/reports", "/home/u/repos"]));
        // Tab completes to the highlighted entry; its exact match ranks first.
        w.on_key(&press(KeyCode::Down));
        w.on_key(&press(KeyCode::Tab));
        assert_eq!(w.input, "~/repos");
        assert_eq!(w.items[0], "/home/u/repos");
        // Descending lists the next directory.
        assert_eq!(w.on_key(&press(KeyCode::Char('/'))), Outcome::ListDir("/home/u/repos".into()));
    }

    #[test]
    fn stale_dir_entries_are_ignored() {
        let mut w = Wizard::new(vec![], "/h".into());
        type_str(&mut w, "/a/");
        w.set_dir_entries("/b", &[DirEntry { name: "x".into(), dir: true }]);
        assert!(w.items.is_empty());
    }

    #[test]
    fn resume_step_lists_newest_first_and_picks() {
        let mut w = Wizard::new(strings(&["/w"]), "/h".into());
        w.on_key(&press(KeyCode::Enter));
        let s = |id: &str, modified| ClaudeSession {
            id: id.into(),
            title: None,
            last_prompt: None,
            modified,
            messages: 3,
        };
        assert_eq!(w.set_claude_sessions("/other", vec![s("x", 1)]), Outcome::None, "stale reply");
        assert_eq!(w.set_claude_sessions("/w", vec![s("old", 1), s("new", 9)]), Outcome::None);
        let step = w.resume.as_ref().unwrap();
        assert_eq!(step.sessions[0].id, "new");
        assert_eq!(
            w.on_key(&press(KeyCode::Enter)),
            Outcome::Spawn { cwd: "/w".into(), resume: None },
            "first row is `+ New session`"
        );
        w.on_key(&press(KeyCode::Down));
        w.on_key(&press(KeyCode::Down));
        assert_eq!(
            w.on_key(&press(KeyCode::Enter)),
            Outcome::Spawn { cwd: "/w".into(), resume: Some("old".into()) }
        );
        // Esc goes back to the directory step.
        w.on_key(&press(KeyCode::Esc));
        assert!(w.resume.is_none());
        assert_eq!(w.on_key(&press(KeyCode::Esc)), Outcome::Cancel);
    }

    #[test]
    fn no_sessions_spawns_directly() {
        let mut w = Wizard::new(strings(&["/w"]), "/h".into());
        w.on_key(&press(KeyCode::Enter));
        assert_eq!(w.set_claude_sessions("/w", vec![]), Outcome::Spawn { cwd: "/w".into(), resume: None });
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
}
