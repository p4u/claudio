//! The git viewer's state machine (Alt+l), a pure sibling of the wizard.
//!
//! Three stacked pages: the commit **log**, one **commit** (header, message,
//! selectable file list) and a **diff** (one file's patch, or the whole
//! commit's). `Esc` or `Backspace` pops a page; `Esc` on the log closes.
//!
//! The view does no I/O. Keys, mouse events and daemon replies go in; a
//! [`GitOutcome`] comes out, and the only thing it can ask for is a request to
//! the session's daemon. Each request carries a sequence number the caller
//! hands back with the reply, so a reply to a request that was superseded or
//! abandoned (the page was popped) is recognized and dropped.
//! `tui/ui/git.rs` renders the state; `tui/git_app.rs` connects it to `App`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEventKind};

use crate::proto::{GitCommitInfo, GitFile, GitLogEntry, Msg};

/// Commits fetched per `GitLog` request.
pub const LOG_PAGE: u32 = 200;
/// Fetch the next page when the selection is this close to the end.
const PREFETCH: usize = 50;
/// Lines a mouse wheel notch moves.
const WHEEL_STEP: usize = 3;
/// Rows around a page body: a header above and a footer below.
pub const CHROME: usize = 2;
/// Message lines shown on the commit page before the rest is cut.
pub const MESSAGE_ROWS_MAX: usize = 8;
/// Fixed rows of the commit page above the message: id, author, date, refs.
const COMMIT_HEAD_ROWS: usize = 4;

/// What the caller must do after feeding the view an input.
#[derive(Debug, PartialEq)]
pub enum GitOutcome {
    None,
    Close,
    /// Send `msg` to the view's host; pass `seq` back with the reply.
    Request {
        seq: u64,
        msg: Msg,
    },
}

/// Something fetched from the daemon.
#[derive(Debug, Clone, PartialEq)]
pub enum Load<T> {
    Loading,
    Ready(T),
    Failed(String),
}

pub struct GitView {
    pub host: String,
    cwd: String,
    /// Whether the log spans all branches (`a`).
    pub all: bool,
    pub log: Log,
    /// The open commit page, if any; its `diff` is the page above it.
    pub commit: Option<CommitPage>,
    inflight: Inflight,
    next_seq: u64,
}

/// The newest outstanding request of each kind.
#[derive(Default)]
struct Inflight {
    /// Sequence number and the `skip` it asked for.
    log: Option<(u64, usize)>,
    commit: Option<u64>,
    diff: Option<u64>,
}

#[derive(Default)]
pub struct Log {
    pub root: String,
    /// The checked-out branch, `None` when detached or unknown.
    pub head: Option<String>,
    /// Everything loaded so far, newest first.
    pub commits: Vec<GitLogEntry>,
    /// Indexes into `commits` that match the filter.
    pub visible: Vec<usize>,
    pub cursor: Cursor,
    pub more: bool,
    pub error: Option<String>,
    pub filter: String,
    /// The filter box has the keyboard.
    pub filtering: bool,
}

pub struct CommitPage {
    pub id: String,
    pub info: Load<GitCommitInfo>,
    pub files: Cursor,
    pub diff: Option<DiffPage>,
}

pub struct DiffPage {
    /// The file shown; `None` for the whole commit.
    pub path: Option<String>,
    pub patch: Load<Patch>,
    scroll: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Patch {
    pub lines: Vec<String>,
    pub truncated: bool,
    /// Indexes of the `diff --git` lines.
    file_starts: Vec<usize>,
}

/// A selection in a list of rows, scrolled to keep it in view.
#[derive(Debug, Default, Clone)]
pub struct Cursor {
    pub selected: usize,
    top: usize,
}

#[derive(Clone, Copy)]
enum Nav {
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
}

impl Nav {
    /// Where the movement leads from `current`, given the last valid index.
    fn target(self, current: usize, last: usize, page: usize) -> usize {
        match self {
            Nav::Up => current.saturating_sub(1),
            Nav::Down => current + 1,
            Nav::PageUp => current.saturating_sub(page),
            Nav::PageDown => current + page,
            Nav::Home => 0,
            Nav::End => last,
        }
        .min(last)
    }
}

impl Cursor {
    /// The first visible row when `rows` fit on screen.
    pub fn top(&self, rows: usize) -> usize {
        let rows = rows.max(1);
        if self.selected < self.top {
            self.selected
        } else if self.selected >= self.top + rows {
            self.selected + 1 - rows
        } else {
            self.top
        }
    }

    fn go(&mut self, nav: Nav, len: usize, rows: usize) {
        if len > 0 {
            self.selected = nav.target(self.selected, len - 1, rows.max(1));
            self.top = self.top(rows);
        }
    }
}

impl Log {
    pub fn selected(&self) -> Option<&GitLogEntry> {
        let index = *self.visible.get(self.cursor.selected)?;
        self.commits.get(index)
    }

    /// Recompute `visible`, keeping `keep` selected when it still matches.
    fn refilter(&mut self, keep: Option<&str>) {
        let filter = self.filter.to_lowercase();
        self.visible = (0..self.commits.len())
            .filter(|&i| matches_filter(&self.commits[i], &filter))
            .collect();
        let at = keep.and_then(|id| self.visible.iter().position(|&i| self.commits[i].id == id));
        self.cursor = Cursor {
            selected: at.unwrap_or(0),
            top: if at.is_some() { self.cursor.top } else { 0 },
        };
    }

    fn selected_id(&self) -> Option<String> {
        self.selected().map(|c| c.id.clone())
    }

    fn edit_filter(&mut self, edit: impl FnOnce(&mut String)) {
        edit(&mut self.filter);
        self.refilter(None);
    }
}

/// Hash, subject or author contains the (lowercased) filter.
fn matches_filter(commit: &GitLogEntry, filter: &str) -> bool {
    filter.is_empty()
        || commit.id.contains(filter)
        || commit.subject.to_lowercase().contains(filter)
        || commit.author.to_lowercase().contains(filter)
}

impl CommitPage {
    fn new(id: String) -> Self {
        CommitPage {
            id,
            info: Load::Loading,
            files: Cursor::default(),
            diff: None,
        }
    }

    fn files(&self) -> &[GitFile] {
        match &self.info {
            Load::Ready(info) => &info.files,
            _ => &[],
        }
    }

    fn selected_file(&self) -> Option<&GitFile> {
        self.files().get(self.files.selected)
    }

    /// First row of the file list: the head, the message and a summary line.
    pub fn files_top(&self) -> usize {
        match &self.info {
            Load::Ready(info) => COMMIT_HEAD_ROWS + 1 + message_rows(info) + 2,
            _ => COMMIT_HEAD_ROWS,
        }
    }

    /// Rows available to the file list on a pane `rows` tall (footer excluded).
    pub fn file_rows(&self, rows: usize) -> usize {
        rows.saturating_sub(self.files_top() + 1).max(1)
    }
}

/// Rows the message takes on the commit page, including a "more" marker.
pub fn message_rows(info: &GitCommitInfo) -> usize {
    let lines = info.message.lines().count();
    if lines > MESSAGE_ROWS_MAX {
        MESSAGE_ROWS_MAX + 1
    } else {
        lines
    }
}

impl DiffPage {
    fn new(path: Option<String>) -> Self {
        DiffPage {
            path,
            patch: Load::Loading,
            scroll: 0,
        }
    }

    /// The first visible line when `rows` fit on screen.
    pub fn scroll(&self, rows: usize) -> usize {
        self.scroll.min(self.max_scroll(rows))
    }

    fn max_scroll(&self, rows: usize) -> usize {
        match &self.patch {
            Load::Ready(p) => p.lines.len().saturating_sub(rows.max(1)),
            _ => 0,
        }
    }

    fn go(&mut self, nav: Nav, rows: usize) {
        let last = self.max_scroll(rows);
        self.scroll = nav.target(self.scroll(rows), last, rows.max(1));
    }

    /// Jump to the next (`forward`) or previous file of the patch.
    fn jump_file(&mut self, forward: bool, rows: usize) {
        let Load::Ready(patch) = &self.patch else {
            return;
        };
        let here = self.scroll(rows);
        let target = if forward {
            patch.file_starts.iter().find(|&&s| s > here)
        } else {
            patch.file_starts.iter().rev().find(|&&s| s < here)
        };
        if let Some(&line) = target {
            self.scroll = line.min(self.max_scroll(rows));
        }
    }
}

impl Patch {
    fn new(patch: &str, truncated: bool) -> Self {
        let lines: Vec<String> = patch.lines().map(str::to_owned).collect();
        let file_starts = (0..lines.len())
            .filter(|&i| lines[i].starts_with("diff --git "))
            .collect();
        Patch {
            lines,
            truncated,
            file_starts,
        }
    }
}

/// The key as a plain character (no Ctrl/Alt), if it is one.
fn plain_char(key: &KeyEvent) -> Option<char> {
    match key.code {
        KeyCode::Char(c) if key.modifiers.difference(KeyModifiers::SHIFT).is_empty() => Some(c),
        _ => None,
    }
}

/// Arrow-style movement keys, shared by every page and the filter box.
fn arrow_nav(code: KeyCode) -> Option<Nav> {
    Some(match code {
        KeyCode::Up => Nav::Up,
        KeyCode::Down => Nav::Down,
        KeyCode::PageUp => Nav::PageUp,
        KeyCode::PageDown => Nav::PageDown,
        KeyCode::Home => Nav::Home,
        KeyCode::End => Nav::End,
        _ => return None,
    })
}

/// Arrow keys plus vi-style `j`/`k`.
fn list_nav(key: &KeyEvent) -> Option<Nav> {
    arrow_nav(key.code).or(match plain_char(key) {
        Some('j') => Some(Nav::Down),
        Some('k') => Some(Nav::Up),
        _ => None,
    })
}

/// Esc or Backspace: go back one level.
fn is_back(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::Esc | KeyCode::Backspace)
}

impl GitView {
    /// A view of the history at `cwd` on `host`, and its first request.
    pub fn open(host: String, cwd: String) -> (GitView, GitOutcome) {
        let mut view = GitView {
            host,
            cwd,
            all: false,
            log: Log::default(),
            commit: None,
            inflight: Inflight::default(),
            next_seq: 0,
        };
        let first = view.request_log(0);
        (view, first)
    }

    /// Whether a log page is being fetched.
    pub fn loading_log(&self) -> bool {
        self.inflight.log.is_some()
    }

    // ── Input ─────────────────────────────────────────────────────────────

    /// A key press, on a pane `rows` tall.
    pub fn on_key(&mut self, key: &KeyEvent, rows: usize) -> GitOutcome {
        match &self.commit {
            None => self.log_key(key, rows),
            Some(commit) if commit.diff.is_some() => self.diff_key(key, rows),
            Some(_) => self.commit_key(key, rows),
        }
    }

    /// Pasted text goes to the filter box when it is open.
    pub fn on_paste(&mut self, text: &str) {
        if self.commit.is_none() && self.log.filtering {
            let line = text.lines().next().unwrap_or("");
            self.log.edit_filter(|f| f.push_str(line));
        }
    }

    /// A mouse event at pane row `row` (0 = the page header).
    pub fn on_mouse(&mut self, kind: MouseEventKind, row: usize, rows: usize) -> GitOutcome {
        match kind {
            MouseEventKind::ScrollUp => self.wheel(Nav::Up, rows),
            MouseEventKind::ScrollDown => self.wheel(Nav::Down, rows),
            MouseEventKind::Down(crossterm::event::MouseButton::Left) => self.click(row, rows),
            _ => GitOutcome::None,
        }
    }

    fn wheel(&mut self, nav: Nav, rows: usize) -> GitOutcome {
        let mut outcome = GitOutcome::None;
        for _ in 0..WHEEL_STEP {
            let next = self.navigate(nav, rows);
            if outcome == GitOutcome::None {
                outcome = next;
            }
        }
        outcome
    }

    fn click(&mut self, row: usize, rows: usize) -> GitOutcome {
        match &mut self.commit {
            None => {
                let body = rows.saturating_sub(CHROME);
                let index = self.log.cursor.top(body) + row.saturating_sub(1);
                if row == 0 || row > body || index >= self.log.visible.len() {
                    return GitOutcome::None;
                }
                self.log.cursor.selected = index;
                self.open_commit()
            }
            Some(commit) if commit.diff.is_none() => {
                let body = commit.file_rows(rows);
                let Some(line) = row.checked_sub(commit.files_top()).filter(|&l| l < body) else {
                    return GitOutcome::None;
                };
                let index = commit.files.top(body) + line;
                if index >= commit.files().len() {
                    return GitOutcome::None;
                }
                commit.files.selected = index;
                self.open_file_diff()
            }
            Some(_) => GitOutcome::None,
        }
    }

    /// Move the selection or scroll of the page on top.
    fn navigate(&mut self, nav: Nav, rows: usize) -> GitOutcome {
        match &mut self.commit {
            None => {
                let log = &mut self.log;
                log.cursor
                    .go(nav, log.visible.len(), rows.saturating_sub(CHROME));
                self.load_more_if_near_end()
            }
            Some(commit) => {
                match &mut commit.diff {
                    Some(diff) => diff.go(nav, rows.saturating_sub(CHROME)),
                    None => {
                        let (len, body) = (commit.files().len(), commit.file_rows(rows));
                        commit.files.go(nav, len, body);
                    }
                }
                GitOutcome::None
            }
        }
    }

    fn log_key(&mut self, key: &KeyEvent, rows: usize) -> GitOutcome {
        if self.log.filtering {
            return self.filter_key(key, rows);
        }
        if let Some(nav) = list_nav(key) {
            return self.navigate(nav, rows);
        }
        match (key.code, plain_char(key)) {
            (KeyCode::Enter, _) => self.open_commit(),
            (KeyCode::Esc, _) if !self.log.filter.is_empty() => {
                self.log.edit_filter(String::clear);
                GitOutcome::None
            }
            (KeyCode::Esc, _) => GitOutcome::Close,
            (_, Some('/')) => {
                self.log.filtering = true;
                GitOutcome::None
            }
            (_, Some('a')) => {
                self.all = !self.all;
                self.reload(true)
            }
            (_, Some('r')) => self.reload(false),
            _ => GitOutcome::None,
        }
    }

    /// Keys while the filter box has the keyboard: text edits it, arrows
    /// still move.
    fn filter_key(&mut self, key: &KeyEvent, rows: usize) -> GitOutcome {
        if let Some(nav) = arrow_nav(key.code) {
            return self.navigate(nav, rows);
        }
        match (key.code, plain_char(key)) {
            (KeyCode::Esc, _) => {
                self.log.edit_filter(String::clear);
                self.log.filtering = false;
            }
            (KeyCode::Enter, _) => self.log.filtering = false,
            (KeyCode::Backspace, _) => self.log.edit_filter(|f| {
                f.pop();
            }),
            (KeyCode::Char('u'), _) if key.modifiers == KeyModifiers::CONTROL => {
                self.log.edit_filter(String::clear)
            }
            (_, Some(c)) => self.log.edit_filter(|f| f.push(c)),
            _ => {}
        }
        GitOutcome::None
    }

    fn commit_key(&mut self, key: &KeyEvent, rows: usize) -> GitOutcome {
        if let Some(nav) = list_nav(key) {
            return self.navigate(nav, rows);
        }
        match (key.code, plain_char(key)) {
            (KeyCode::Enter, _) => self.open_file_diff(),
            (_, Some('d')) => self.open_diff(None),
            _ if is_back(key) => {
                self.commit = None;
                self.inflight.commit = None;
                self.inflight.diff = None;
                GitOutcome::None
            }
            _ => GitOutcome::None,
        }
    }

    fn diff_key(&mut self, key: &KeyEvent, rows: usize) -> GitOutcome {
        let body = rows.saturating_sub(CHROME);
        let Some(diff) = self.commit.as_mut().and_then(|c| c.diff.as_mut()) else {
            return GitOutcome::None;
        };
        let nav = list_nav(key).or(match plain_char(key) {
            Some(' ') => Some(Nav::PageDown),
            Some('g') => Some(Nav::Home),
            Some('G') => Some(Nav::End),
            _ => None,
        });
        match (nav, plain_char(key)) {
            (Some(nav), _) => diff.go(nav, body),
            (None, Some('n')) => diff.jump_file(true, body),
            (None, Some('N')) => diff.jump_file(false, body),
            _ if is_back(key) => {
                if let Some(commit) = &mut self.commit {
                    commit.diff = None;
                }
                self.inflight.diff = None;
            }
            _ => {}
        }
        GitOutcome::None
    }

    // ── Requests ──────────────────────────────────────────────────────────

    fn next_seq(&mut self) -> u64 {
        self.next_seq += 1;
        self.next_seq
    }

    fn request_log(&mut self, skip: usize) -> GitOutcome {
        let seq = self.next_seq();
        self.inflight.log = Some((seq, skip));
        GitOutcome::Request {
            seq,
            msg: Msg::GitLog {
                cwd: self.cwd.clone(),
                all: self.all,
                skip: skip as u32,
                limit: LOG_PAGE,
            },
        }
    }

    /// Fetch the log again from the top. `clear` empties the list meanwhile
    /// (a different branch scope); otherwise it stays until the reply.
    fn reload(&mut self, clear: bool) -> GitOutcome {
        self.log.error = None;
        if clear {
            self.log.commits.clear();
            self.log.refilter(None);
        }
        self.request_log(0)
    }

    fn load_more_if_near_end(&mut self) -> GitOutcome {
        let log = &self.log;
        let near_end = log.cursor.selected + PREFETCH >= log.visible.len();
        if log.more && log.error.is_none() && self.inflight.log.is_none() && near_end {
            self.request_log(log.commits.len())
        } else {
            GitOutcome::None
        }
    }

    fn open_commit(&mut self) -> GitOutcome {
        let Some(id) = self.log.selected_id() else {
            return GitOutcome::None;
        };
        let seq = self.next_seq();
        self.inflight.commit = Some(seq);
        self.inflight.diff = None;
        self.commit = Some(CommitPage::new(id.clone()));
        GitOutcome::Request {
            seq,
            msg: Msg::GitCommit {
                cwd: self.cwd.clone(),
                id,
            },
        }
    }

    fn open_file_diff(&mut self) -> GitOutcome {
        let Some(commit) = &self.commit else {
            return GitOutcome::None;
        };
        let index = commit.files.selected;
        match commit.selected_file() {
            Some(file) => {
                let (path, old_path) = (file.path.clone(), file.old_path.clone());
                self.open_diff(Some((index as u32, path, old_path)))
            }
            None => GitOutcome::None,
        }
    }

    /// Open a diff page: one file (its position in the commit's list, with
    /// its display names) or, with `None`, the whole commit.
    fn open_diff(&mut self, file: Option<(u32, String, Option<String>)>) -> GitOutcome {
        let seq = self.next_seq();
        let Some(commit) = &mut self.commit else {
            return GitOutcome::None;
        };
        if !matches!(commit.info, Load::Ready(_)) {
            return GitOutcome::None;
        }
        // The names are display text and only a fallback for daemons that
        // predate `file`; the position is what identifies the file.
        let (file, path, old_path) = match file {
            Some((index, path, old_path)) => (Some(index), Some(path), old_path),
            None => (None, None, None),
        };
        commit.diff = Some(DiffPage::new(path.clone()));
        self.inflight.diff = Some(seq);
        GitOutcome::Request {
            seq,
            msg: Msg::GitDiff {
                cwd: self.cwd.clone(),
                id: commit.id.clone(),
                file,
                path,
                old_path,
            },
        }
    }

    // ── Replies ───────────────────────────────────────────────────────────

    /// The reply (or error text) to request `seq`. Replies to requests that
    /// were superseded or abandoned are ignored.
    pub fn on_reply(&mut self, seq: u64, reply: Result<Msg, String>) -> GitOutcome {
        if let Some((_, skip)) = self.inflight.log.filter(|&(s, _)| s == seq) {
            self.inflight.log = None;
            return self.log_reply(skip, reply);
        }
        if self.inflight.commit == Some(seq) {
            self.inflight.commit = None;
            if let Some(commit) = &mut self.commit {
                commit.info = match reply {
                    Ok(Msg::GitCommitInfo(info)) => Load::Ready(info),
                    Ok(_) => Load::Failed(UNEXPECTED.into()),
                    Err(e) => Load::Failed(e),
                };
            }
        } else if self.inflight.diff == Some(seq) {
            self.inflight.diff = None;
            let diff = self.commit.as_mut().and_then(|c| c.diff.as_mut());
            if let Some(diff) = diff {
                diff.patch = match reply {
                    Ok(Msg::GitPatch(p)) => Load::Ready(Patch::new(&p.patch, p.truncated)),
                    Ok(_) => Load::Failed(UNEXPECTED.into()),
                    Err(e) => Load::Failed(e),
                };
            }
        }
        GitOutcome::None
    }

    fn log_reply(&mut self, skip: usize, reply: Result<Msg, String>) -> GitOutcome {
        let page = match reply {
            Ok(Msg::GitLogPage(page)) => page,
            Ok(_) => return self.log_failed(UNEXPECTED.into()),
            Err(e) => return self.log_failed(e),
        };
        let log = &mut self.log;
        let keep = log.selected_id();
        // "More" without a single commit would be asked for again, unchanged,
        // for ever.
        let stalled = page.more && page.commits.is_empty();
        if skip == 0 {
            log.commits = page.commits;
        } else if skip == log.commits.len() {
            log.commits.extend(page.commits);
        } else {
            return GitOutcome::None;
        }
        log.root = page.root;
        log.head = page.head;
        log.more = page.more && !stalled;
        log.error = stalled.then(|| NO_PROGRESS.to_owned());
        log.refilter(keep.as_deref());
        // A short first page leaves room to fill, unless a filter hides most.
        if log.filter.is_empty() {
            self.load_more_if_near_end()
        } else {
            GitOutcome::None
        }
    }

    fn log_failed(&mut self, error: String) -> GitOutcome {
        self.log.error = Some(error);
        GitOutcome::None
    }
}

const UNEXPECTED: &str = "unexpected reply from the daemon";
const NO_PROGRESS: &str = "no further commits could be loaded";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{GitLogPage, GitPatch};
    use crossterm::event::{KeyEventKind, KeyEventState};

    const ROWS: usize = 12;

    fn id(n: usize) -> String {
        format!("{n:040x}")
    }

    fn entry(n: usize, subject: &str, author: &str) -> GitLogEntry {
        GitLogEntry {
            id: id(n),
            parents: vec![id(n + 1)],
            author: author.into(),
            time: 1_700_000_000,
            refs: vec![],
            subject: subject.into(),
        }
    }

    fn page(range: std::ops::Range<usize>, more: bool) -> Msg {
        Msg::GitLogPage(GitLogPage {
            root: "/repo".into(),
            head: Some("main".into()),
            commits: range
                .map(|n| entry(n, &format!("commit {n}"), "Ann"))
                .collect(),
            more,
        })
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn ch(c: char) -> KeyEvent {
        key(KeyCode::Char(c))
    }

    fn seq_of(outcome: &GitOutcome) -> u64 {
        match outcome {
            GitOutcome::Request { seq, .. } => *seq,
            other => panic!("expected a request, got {other:?}"),
        }
    }

    fn msg_of(outcome: GitOutcome) -> Msg {
        match outcome {
            GitOutcome::Request { msg, .. } => msg,
            other => panic!("expected a request, got {other:?}"),
        }
    }

    /// A view with `n` commits loaded.
    fn loaded(n: usize, more: bool) -> GitView {
        let (mut view, first) = GitView::open("local".into(), "/repo".into());
        view.on_reply(seq_of(&first), Ok(page(0..n, more)));
        view
    }

    fn info(files: Vec<GitFile>, message: &str) -> Msg {
        Msg::GitCommitInfo(GitCommitInfo {
            id: id(0),
            parents: vec![id(1)],
            author: "Ann".into(),
            email: "a@x".into(),
            time: 1_700_000_000,
            committer_time: 1_700_000_000,
            refs: vec![],
            message: message.into(),
            files,
            first_parent: false,
        })
    }

    fn file(path: &str, old: Option<&str>) -> GitFile {
        GitFile {
            path: path.into(),
            old_path: old.map(Into::into),
            added: Some(2),
            removed: Some(1),
        }
    }

    fn patch(text: &str, truncated: bool) -> Msg {
        Msg::GitPatch(GitPatch {
            id: id(0),
            path: None,
            patch: text.into(),
            truncated,
        })
    }

    /// A view on the commit page of commit 0 with two files.
    fn on_commit_page() -> GitView {
        let mut view = loaded(5, false);
        let open = view.on_key(&key(KeyCode::Enter), ROWS);
        let files = vec![file("a.rs", None), file("new.rs", Some("old.rs"))];
        view.on_reply(seq_of(&open), Ok(info(files, "subject\n\nbody")));
        view
    }

    #[test]
    fn opening_requests_the_first_page() {
        let (view, first) = GitView::open("devbox".into(), "~/repo".into());
        assert_eq!(
            msg_of(first),
            Msg::GitLog {
                cwd: "~/repo".into(),
                all: false,
                skip: 0,
                limit: LOG_PAGE
            }
        );
        assert!(view.loading_log());
        assert_eq!(view.host, "devbox");
    }

    #[test]
    fn keys_move_the_selection_within_bounds() {
        let mut v = loaded(30, false);
        assert!(!v.loading_log());
        assert_eq!(v.log.visible.len(), 30);
        for (k, expect) in [
            (key(KeyCode::Up), 0),
            (ch('j'), 1),
            (key(KeyCode::Down), 2),
            (ch('k'), 1),
            (key(KeyCode::PageDown), 11),
            (key(KeyCode::PageUp), 1),
            (key(KeyCode::End), 29),
            (key(KeyCode::Down), 29),
            (key(KeyCode::Home), 0),
        ] {
            v.on_key(&k, ROWS);
            assert_eq!(v.log.cursor.selected, expect, "{k:?}");
        }
        // Scrolling keeps the selection on screen.
        v.on_key(&key(KeyCode::End), ROWS);
        let top = v.log.cursor.top(ROWS - CHROME);
        assert!((top..top + ROWS - CHROME).contains(&29));
    }

    #[test]
    fn more_commits_load_near_the_end_once() {
        let mut v = loaded(120, true);
        // Far from the end: nothing to fetch.
        assert_eq!(v.on_key(&ch('j'), ROWS), GitOutcome::None);
        // Near the end: fetch the next page, starting after what is loaded.
        let outcome = v.on_key(&key(KeyCode::End), ROWS);
        let seq = seq_of(&outcome);
        assert_eq!(
            msg_of(outcome),
            Msg::GitLog {
                cwd: "/repo".into(),
                all: false,
                skip: 120,
                limit: LOG_PAGE
            }
        );
        // Not again while it is in flight.
        assert_eq!(v.on_key(&ch('k'), ROWS), GitOutcome::None);
        // The reply appends and keeps the selection.
        let before = v.log.selected_id();
        v.on_reply(seq, Ok(page(120..150, false)));
        assert_eq!(v.log.commits.len(), 150);
        assert_eq!(v.log.selected_id().is_some(), before.is_some());
        assert!(!v.log.more);
    }

    #[test]
    fn a_short_first_page_keeps_loading_until_the_screen_is_full() {
        let (mut v, first) = GitView::open("local".into(), "/repo".into());
        let outcome = v.on_reply(seq_of(&first), Ok(page(0..10, true)));
        assert!(matches!(msg_of(outcome), Msg::GitLog { skip: 10, .. }));
    }

    #[test]
    fn filter_matches_hash_subject_and_author() {
        let mut v = loaded(0, false);
        v.log.commits = vec![
            entry(1, "Fix parser", "Ann"),
            entry(2, "Add widget", "Bob"),
            entry(3, "Docs", "Annabel"),
        ];
        v.log.refilter(None);
        v.on_key(&ch('/'), ROWS);
        assert!(v.log.filtering);
        for c in "ANN".chars() {
            v.on_key(&ch(c), ROWS);
        }
        assert_eq!(v.log.visible, [0, 2], "author, case-insensitive");
        // j/k are text while filtering.
        v.on_key(&ch('j'), ROWS);
        assert!(v.log.visible.is_empty());
        v.on_key(&key(KeyCode::Backspace), ROWS);
        v.on_key(&key(KeyCode::Enter), ROWS);
        assert!(!v.log.filtering);
        assert_eq!(v.log.visible, [0, 2], "Enter keeps the filter");
        assert_eq!(v.log.selected().unwrap().subject, "Fix parser");
        // By hash and subject.
        v.log.edit_filter(|f| *f = id(2));
        assert_eq!(v.log.visible, [1]);
        v.log.edit_filter(|f| *f = "widget".into());
        assert_eq!(v.log.visible, [1]);
        // Esc clears the filter first, and closes only when there is none.
        assert_eq!(v.on_key(&key(KeyCode::Esc), ROWS), GitOutcome::None);
        assert_eq!(v.log.visible.len(), 3);
        assert_eq!(v.on_key(&key(KeyCode::Esc), ROWS), GitOutcome::Close);
    }

    #[test]
    fn esc_in_the_filter_box_clears_and_leaves_it() {
        let mut v = loaded(3, false);
        v.on_key(&ch('/'), ROWS);
        v.on_key(&ch('x'), ROWS);
        v.on_key(&key(KeyCode::Esc), ROWS);
        assert!(!v.log.filtering && v.log.filter.is_empty());
        assert_eq!(v.log.visible.len(), 3);
    }

    #[test]
    fn paste_goes_to_the_open_filter_only() {
        let mut v = loaded(3, false);
        v.on_paste("zzz");
        assert!(v.log.filter.is_empty());
        v.on_key(&ch('/'), ROWS);
        v.on_paste("commit 1\nignored");
        assert_eq!(v.log.filter, "commit 1");
        assert_eq!(v.log.visible, [1]);
    }

    #[test]
    fn toggling_all_branches_reloads_from_the_top() {
        let mut v = loaded(5, false);
        let outcome = v.on_key(&ch('a'), ROWS);
        assert!(v.all && v.log.commits.is_empty());
        assert!(matches!(
            msg_of(outcome),
            Msg::GitLog {
                all: true,
                skip: 0,
                ..
            }
        ));
    }

    #[test]
    fn refresh_keeps_the_list_and_the_selected_commit() {
        let mut v = loaded(5, false);
        v.on_key(&ch('j'), ROWS);
        v.on_key(&ch('j'), ROWS);
        let selected = v.log.selected_id();
        let outcome = v.on_key(&ch('r'), ROWS);
        assert_eq!(v.log.commits.len(), 5, "still shown while reloading");
        // The new history has two fresh commits on top.
        let mut fresh = vec![entry(90, "new", "Ann"), entry(91, "newer", "Ann")];
        fresh.extend((0..5).map(|n| entry(n, "old", "Ann")));
        let reply = Msg::GitLogPage(GitLogPage {
            root: "/repo".into(),
            head: None,
            commits: fresh,
            more: false,
        });
        v.on_reply(seq_of(&outcome), Ok(reply));
        assert_eq!(v.log.commits.len(), 7);
        assert_eq!(v.log.selected_id(), selected);
        assert_eq!(v.log.head, None);
    }

    #[test]
    fn log_errors_are_kept_and_not_retried_automatically() {
        let (mut v, first) = GitView::open("local".into(), "/tmp".into());
        v.on_reply(seq_of(&first), Err("not a git repository".into()));
        assert_eq!(v.log.error.as_deref(), Some("not a git repository"));
        assert!(!v.loading_log());
        assert_eq!(v.on_key(&key(KeyCode::Down), ROWS), GitOutcome::None);
        // `r` clears the error and asks again.
        assert!(matches!(
            msg_of(v.on_key(&ch('r'), ROWS)),
            Msg::GitLog { .. }
        ));
        assert!(v.log.error.is_none());
    }

    #[test]
    fn enter_opens_the_selected_commit_and_esc_returns_to_the_same_row() {
        let mut v = loaded(5, false);
        v.on_key(&ch('j'), ROWS);
        v.on_key(&ch('j'), ROWS);
        let open = v.on_key(&key(KeyCode::Enter), ROWS);
        assert_eq!(
            msg_of(open),
            Msg::GitCommit {
                cwd: "/repo".into(),
                id: id(2)
            }
        );
        assert!(matches!(v.commit.as_ref().unwrap().info, Load::Loading));
        // Back out while still loading: nothing is left pending.
        assert_eq!(v.on_key(&key(KeyCode::Esc), ROWS), GitOutcome::None);
        assert!(v.commit.is_none());
        assert_eq!(v.log.cursor.selected, 2);
        assert_eq!(v.on_key(&key(KeyCode::Esc), ROWS), GitOutcome::Close);
    }

    #[test]
    fn the_commit_page_selects_files_and_opens_their_diffs() {
        let mut v = on_commit_page();
        let commit = v.commit.as_ref().unwrap();
        assert!(matches!(commit.info, Load::Ready(_)));
        v.on_key(&ch('j'), ROWS);
        v.on_key(&key(KeyCode::Down), ROWS);
        assert_eq!(v.commit.as_ref().unwrap().files.selected, 1, "clamped");
        let open = v.on_key(&key(KeyCode::Enter), ROWS);
        assert_eq!(
            msg_of(open),
            Msg::GitDiff {
                cwd: "/repo".into(),
                id: id(0),
                file: Some(1),
                path: Some("new.rs".into()),
                old_path: Some("old.rs".into()),
            }
        );
        // Esc/Backspace pop one level at a time; the file selection survives.
        v.on_key(&key(KeyCode::Backspace), ROWS);
        let commit = v.commit.as_ref().unwrap();
        assert!(commit.diff.is_none() && commit.files.selected == 1);
        v.on_key(&key(KeyCode::Esc), ROWS);
        assert!(v.commit.is_none());
    }

    #[test]
    fn d_opens_the_whole_commit_patch() {
        let mut v = on_commit_page();
        let open = v.on_key(&ch('d'), ROWS);
        assert_eq!(
            msg_of(open),
            Msg::GitDiff {
                cwd: "/repo".into(),
                id: id(0),
                file: None,
                path: None,
                old_path: None
            }
        );
    }

    #[test]
    fn files_with_the_same_label_are_told_apart_by_position() {
        let mut v = loaded(1, false);
        let open = v.on_key(&key(KeyCode::Enter), ROWS);
        // Names that sanitize to one label: the position is the identity.
        let twins = vec![file("a b", None), file("a b", None)];
        v.on_reply(seq_of(&open), Ok(info(twins, "subject")));
        let files_of = |outcome| match msg_of(outcome) {
            Msg::GitDiff { file, .. } => file,
            other => panic!("expected a diff request, got {other:?}"),
        };
        assert_eq!(files_of(v.on_key(&key(KeyCode::Enter), ROWS)), Some(0));
        v.on_key(&key(KeyCode::Esc), ROWS);
        v.on_key(&ch('j'), ROWS);
        assert_eq!(files_of(v.on_key(&key(KeyCode::Enter), ROWS)), Some(1));
    }

    #[test]
    fn a_page_without_progress_is_not_asked_for_again() {
        let mut v = loaded(120, true);
        let outcome = v.on_key(&key(KeyCode::End), ROWS);
        assert!(matches!(msg_of(outcome), Msg::GitLog { skip: 120, .. }));
        // The daemon claims more but sent nothing new.
        let seq = v.inflight.log.unwrap().0;
        let stalled = v.on_reply(seq, Ok(page(0..0, true)));
        assert_eq!(stalled, GitOutcome::None);
        assert!(!v.loading_log() && !v.log.more);
        assert_eq!(v.log.error.as_deref(), Some(NO_PROGRESS));
        assert_eq!(v.log.commits.len(), 120, "what was loaded stays");
        assert_eq!(v.on_key(&key(KeyCode::End), ROWS), GitOutcome::None);
        // `r` starts over.
        assert!(matches!(
            msg_of(v.on_key(&ch('r'), ROWS)),
            Msg::GitLog { skip: 0, .. }
        ));
    }

    #[test]
    fn nothing_opens_before_the_commit_has_loaded() {
        let mut v = loaded(3, false);
        v.on_key(&key(KeyCode::Enter), ROWS);
        assert_eq!(v.on_key(&ch('d'), ROWS), GitOutcome::None);
        assert_eq!(v.on_key(&key(KeyCode::Enter), ROWS), GitOutcome::None);
    }

    fn on_diff_page(lines: usize) -> GitView {
        let mut v = on_commit_page();
        let open = v.on_key(&ch('d'), ROWS);
        let mut text = String::new();
        for i in 0..lines {
            if i % 20 == 0 {
                text.push_str(&format!("diff --git a/f{i} b/f{i}\n"));
            } else {
                text.push_str(&format!("+line {i}\n"));
            }
        }
        v.on_reply(seq_of(&open), Ok(patch(&text, true)));
        v
    }

    fn scroll_of(v: &GitView) -> usize {
        v.commit
            .as_ref()
            .unwrap()
            .diff
            .as_ref()
            .unwrap()
            .scroll(ROWS - CHROME)
    }

    #[test]
    fn the_diff_page_scrolls_within_the_patch() {
        let mut v = on_diff_page(100);
        let body = ROWS - CHROME;
        for (k, expect) in [
            (ch('j'), 1),
            (key(KeyCode::Down), 2),
            (ch('k'), 1),
            (ch(' '), 1 + body),
            (key(KeyCode::PageUp), 1),
            (ch('G'), 100 - body),
            (key(KeyCode::Down), 100 - body),
            (ch('g'), 0),
            (key(KeyCode::End), 100 - body),
            (key(KeyCode::Home), 0),
        ] {
            v.on_key(&k, ROWS);
            assert_eq!(scroll_of(&v), expect, "{k:?}");
        }
    }

    #[test]
    fn n_and_shift_n_jump_between_files() {
        let mut v = on_diff_page(100); // files start at lines 0, 20, 40, ...
        v.on_key(&ch('n'), ROWS);
        assert_eq!(scroll_of(&v), 20);
        v.on_key(&ch('n'), ROWS);
        assert_eq!(scroll_of(&v), 40);
        v.on_key(&ch('N'), ROWS);
        assert_eq!(scroll_of(&v), 20);
        v.on_key(&ch('N'), ROWS);
        v.on_key(&ch('N'), ROWS);
        assert_eq!(scroll_of(&v), 0, "stays at the first file");
    }

    #[test]
    fn a_truncated_patch_is_flagged() {
        let v = on_diff_page(10);
        let diff = v.commit.as_ref().unwrap().diff.as_ref().unwrap();
        assert!(matches!(&diff.patch, Load::Ready(p) if p.truncated && p.lines.len() == 10));
    }

    #[test]
    fn replies_to_unknown_or_superseded_requests_are_dropped() {
        let mut v = loaded(5, false);
        // Unknown seq.
        v.on_reply(999, Ok(page(0..1, false)));
        assert_eq!(v.log.commits.len(), 5);
        // Two commit requests: only the newest counts.
        let first = v.on_key(&key(KeyCode::Enter), ROWS);
        v.on_key(&key(KeyCode::Esc), ROWS);
        v.on_key(&ch('j'), ROWS);
        let second = v.on_key(&key(KeyCode::Enter), ROWS);
        v.on_reply(seq_of(&first), Ok(info(vec![], "stale")));
        assert!(matches!(v.commit.as_ref().unwrap().info, Load::Loading));
        v.on_reply(seq_of(&second), Ok(info(vec![], "fresh")));
        assert!(matches!(&v.commit.as_ref().unwrap().info, Load::Ready(i) if i.message == "fresh"));
    }

    #[test]
    fn a_reply_after_the_page_was_popped_is_dropped() {
        let mut v = on_commit_page();
        let open = v.on_key(&ch('d'), ROWS);
        v.on_key(&key(KeyCode::Esc), ROWS); // back to the commit page
        v.on_reply(seq_of(&open), Ok(patch("diff --git a b\n", false)));
        assert!(v.commit.as_ref().unwrap().diff.is_none());
        // Likewise an old log reply after a newer reload.
        let mut v = loaded(5, false);
        let old = v.on_key(&ch('r'), ROWS);
        let new = v.on_key(&ch('r'), ROWS);
        v.on_reply(seq_of(&old), Ok(page(0..1, false)));
        assert_eq!(v.log.commits.len(), 5);
        v.on_reply(seq_of(&new), Ok(page(0..2, false)));
        assert_eq!(v.log.commits.len(), 2);
    }

    #[test]
    fn errors_show_on_the_page_that_asked() {
        let mut v = on_commit_page();
        let open = v.on_key(&ch('d'), ROWS);
        v.on_reply(seq_of(&open), Err("boom".into()));
        let diff = v.commit.as_ref().unwrap().diff.as_ref().unwrap();
        assert_eq!(diff.patch, Load::Failed("boom".into()));
        // A wrong-shaped reply is an error, not a panic.
        let mut v = loaded(2, false);
        let open = v.on_key(&key(KeyCode::Enter), ROWS);
        v.on_reply(seq_of(&open), Ok(Msg::Pong));
        assert!(matches!(v.commit.as_ref().unwrap().info, Load::Failed(_)));
    }

    #[test]
    fn the_wheel_scrolls_and_clicks_open_rows() {
        let mut v = loaded(30, false);
        v.on_mouse(MouseEventKind::ScrollDown, 3, ROWS);
        assert_eq!(v.log.cursor.selected, WHEEL_STEP);
        v.on_mouse(MouseEventKind::ScrollUp, 3, ROWS);
        assert_eq!(v.log.cursor.selected, 0);
        // Row 0 is the header; row 3 is the third commit.
        let click = MouseEventKind::Down(crossterm::event::MouseButton::Left);
        assert_eq!(v.on_mouse(click, 0, ROWS), GitOutcome::None);
        let open = v.on_mouse(click, 3, ROWS);
        assert_eq!(
            msg_of(open),
            Msg::GitCommit {
                cwd: "/repo".into(),
                id: id(2)
            }
        );
    }

    #[test]
    fn clicking_a_file_opens_its_diff() {
        let mut v = on_commit_page();
        let top = v.commit.as_ref().unwrap().files_top();
        let click = MouseEventKind::Down(crossterm::event::MouseButton::Left);
        let open = v.on_mouse(click, top + 1, 30);
        assert!(matches!(msg_of(open), Msg::GitDiff { path: Some(p), .. } if p == "new.rs"));
        // The wheel scrolls the patch.
        let mut v = on_diff_page(100);
        v.on_mouse(MouseEventKind::ScrollDown, 5, ROWS);
        assert_eq!(scroll_of(&v), WHEEL_STEP);
    }
}
