//! The built-in extensions' actions: search (`s`: find a conversation on
//! the active session's host and resume it; the wizard's "Search
//! conversations" row searches every connected host) and snippets (`p`: paste a
//! saved prompt, `P`: save the last one). Pure, like `app.rs`: the daemon
//! searches and reads the last prompt (`Msg::Search`, `Msg::LastPrompt`),
//! and the snippet files are read and written by the event loop
//! ([`Effect::LoadSnippets`], [`Effect::SaveSnippet`]).

use std::io;

use super::app::{App, Effect, ReplyTo};
use super::fmt::{abbreviate_home, fmt_age};
use super::interaction::Modal;
use super::leader::Picker;
use claudio_core::app::extensions::{ActionResult, ListItem};
use claudio_core::proto::{Msg, SearchHit, SessionKind};

/// The most hits a search asks for.
const SEARCH_LIMIT: u32 = 50;

/// Why the wizard's search found no host to ask.
const NO_SEARCH_HOST: &str = "no connected host can search (run `claudio daemon restart` there)";

/// What an [`Effect::SearchAll`] found (`search_edge::run`).
#[derive(Debug)]
pub struct SearchAllDone {
    pub gen: u64,
    pub query: String,
    /// How many hosts were asked (0: none could search).
    pub searched: usize,
    /// Newest first.
    pub hits: Vec<HostHit>,
    /// `(host, error)`.
    pub failed: Vec<(String, String)>,
}

/// One hit with its host and that host's home (for `~/`).
#[derive(Debug)]
pub struct HostHit {
    pub host: String,
    pub home: String,
    pub hit: SearchHit,
}

impl App {
    /// A built-in extension's action (`input`: what its input line got).
    pub(super) fn builtin_action(&mut self, ext: &str, action: &str, input: Option<String>) {
        match (ext, action) {
            ("search", "search") => self.start_search(input.unwrap_or_default()),
            ("snippets", "insert") => self.load_snippets(),
            ("snippets", "save") => self.save_last_prompt(),
            _ => self.notify(format!("{ext}: {action} is not available")),
        }
    }

    /// Search the conversations on the active session's host (local when
    /// there is none) for `query`.
    fn start_search(&mut self, query: String) {
        if query.trim().is_empty() {
            return self.notify("search needs a query");
        }
        let host = self.active_host();
        self.modal_gen += 1;
        let gen = self.modal_gen;
        self.pending_action = Some(gen);
        self.notify("searching…");
        let msg = Msg::Search {
            query,
            limit: SEARCH_LIMIT,
        };
        self.request(
            &host,
            msg,
            ReplyTo::Search {
                gen,
                host: host.clone(),
            },
        );
    }

    /// `host`'s answer to the search started under `gen`: a list of the
    /// conversations (Enter resumes one), or a notice.
    pub(super) fn on_search_reply(&mut self, gen: u64, host: &str, reply: io::Result<Msg>) {
        if self.pending_action != Some(gen) {
            return;
        }
        self.pending_action = None;
        match reply {
            Ok(Msg::SearchReply { query, hits, .. }) if hits.is_empty() => {
                let _ = query;
                self.notify("no matches")
            }
            Ok(Msg::SearchReply { query, hits, .. }) => {
                let title = format!("{} conversations · \"{query}\"", hits.len());
                let home = if host == "local" {
                    self.home.as_str()
                } else {
                    ""
                };
                let items = hits
                    .into_iter()
                    .map(|hit| self.search_item(host, home, hit, false))
                    .collect();
                if self.modal.is_none() {
                    self.modal = Some(Modal::Picker(Box::new(Picker::new(title, items))));
                } else {
                    self.notify("close the popup and run it again")
                }
            }
            Ok(_) => self.notify("unexpected reply from the daemon"),
            Err(e) if e.to_string().contains("search busy") => {
                self.notify("search busy, try again")
            }
            Err(e) => {
                let notice = App::older_daemon_notice(host, &e);
                self.notify(notice.unwrap_or_else(|| format!("search failed: {e}")));
            }
        }
        self.redraw = true;
    }

    /// One hit as a list line: `<when>  [<host>  ]<~/cwd>  <title>` (`home`
    /// is `host`'s, empty when unknown), the snippet as its detail, resuming
    /// it as its result.
    fn search_item(&self, host: &str, home: &str, hit: SearchHit, with_host: bool) -> ListItem {
        let age = fmt_age(self.now.saturating_sub(hit.modified));
        let cwd = abbreviate_home(&hit.cwd, home);
        let host_col = if with_host {
            format!("{host}  ")
        } else {
            String::new()
        };
        ListItem {
            label: format!("{age}  {host_col}{cwd}  {}", hit.title),
            detail: Some(hit.snippet),
            result: ActionResult::Resume {
                host: host.to_owned(),
                cwd: hit.cwd,
                conversation: hit.conversation,
            },
        }
    }

    /// The wizard's "Search conversations" row: close the wizard and search
    /// every connected host ([`Effect::SearchAll`], one at a time); the hits
    /// come back to [`App::on_search_all`].
    pub(super) fn search_all(&mut self, query: String) {
        self.modal = None;
        if self.searching.is_some() {
            return self.notify("a search is already running");
        }
        self.modal_gen += 1;
        let gen = self.modal_gen;
        (self.pending_action, self.searching) = (Some(gen), Some(gen));
        self.notify("searching…");
        self.emit(Effect::SearchAll { gen, query });
    }

    /// What the search started under `gen` found: its hits in the search
    /// list (Enter resumes one), the hosts that failed in one notice.
    pub(crate) fn on_search_all(&mut self, done: SearchAllDone) {
        if self.searching == Some(done.gen) {
            self.searching = None;
        }
        if self.pending_action != Some(done.gen) {
            return;
        }
        self.pending_action = None;
        self.redraw = true;
        if done.searched == 0 {
            return self.notify(NO_SEARCH_HOST);
        }
        let failed = (!done.failed.is_empty()).then(|| {
            let hosts: Vec<String> = done
                .failed
                .iter()
                .map(|(h, e)| format!("{h} ({e})"))
                .collect();
            format!("search failed on {}", hosts.join(", "))
        });
        if done.hits.is_empty() {
            return self.notify(failed.unwrap_or_else(|| "no matches".to_owned()));
        }
        if self.modal.is_some() {
            return self.notify("close the popup and run it again");
        }
        let title = format!("{} conversations · \"{}\"", done.hits.len(), done.query);
        let items = done
            .hits
            .into_iter()
            .map(|h| self.search_item(&h.host, &h.home, h.hit, true))
            .collect();
        self.modal = Some(Modal::Picker(Box::new(Picker::new(title, items))));
        if let Some(failed) = failed {
            self.notify(failed);
        }
    }

    /// Show `conversation`: switch to the tab on `host` that has it, else
    /// start one there resuming it, in `cwd`, routed as the wizard would
    /// preselect.
    pub(super) fn resume_conversation(&mut self, host: String, cwd: String, conversation: String) {
        if cwd.is_empty() {
            return self.notify("that conversation has no working directory to resume in");
        }
        let open = self.sessions.iter().position(|v| {
            v.host == host && v.claude_session_id.as_deref() == Some(conversation.as_str())
        });
        match open {
            Some(i) => self.activate(i),
            None => {
                let select = self.preselected();
                self.spawn(host, cwd, Some(conversation), select);
            }
        }
    }

    /// Alt+z p: read the snippets (off the UI thread), then list them.
    fn load_snippets(&mut self) {
        if self.active_view().is_none() {
            return self.notify("no active session");
        }
        self.modal_gen += 1;
        let gen = self.modal_gen;
        self.pending_action = Some(gen);
        self.emit(Effect::LoadSnippets { gen });
    }

    /// The snippets in `dir` (name, text), read for the action started
    /// under `gen`: a list whose Enter pastes one, or a notice.
    pub fn on_snippets(
        &mut self,
        gen: u64,
        dir: &str,
        result: Result<Vec<(String, String)>, String>,
    ) {
        if self.pending_action != Some(gen) {
            return;
        }
        self.pending_action = None;
        match result {
            Err(e) => self.notify(format!("could not read snippets: {e}")),
            Ok(snippets) if snippets.is_empty() => self.notify(format!("no snippets in {dir}")),
            Ok(snippets) => {
                let items = snippets
                    .into_iter()
                    .map(|(name, text)| ListItem {
                        label: name,
                        detail: text
                            .lines()
                            .map(str::trim)
                            .find(|l| !l.is_empty())
                            .map(str::to_owned),
                        result: ActionResult::Paste { text },
                    })
                    .collect();
                if self.modal.is_none() {
                    let picker = Picker::new("Snippets".to_owned(), items);
                    self.modal = Some(Modal::Picker(Box::new(picker)));
                } else {
                    self.notify("close the popup and run it again")
                }
            }
        }
        self.redraw = true;
    }

    /// Alt+z P: ask the active session's daemon for its last prompt.
    fn save_last_prompt(&mut self) {
        let Some(v) = self.active_view() else {
            return self.notify("no active session");
        };
        if v.kind != SessionKind::Claude {
            return self.notify("no prompt to save");
        }
        let (id, host) = (v.id, v.host.clone());
        self.modal_gen += 1;
        let gen = self.modal_gen;
        self.pending_action = Some(gen);
        let to = ReplyTo::LastPrompt {
            gen,
            host: host.clone(),
        };
        self.request(&host, Msg::LastPrompt { id }, to);
    }

    /// `host`'s answer to the last-prompt request started under `gen`:
    /// saved as a snippet by the event loop, or a notice.
    pub(super) fn on_last_prompt_reply(&mut self, gen: u64, host: &str, reply: io::Result<Msg>) {
        if self.pending_action != Some(gen) {
            return;
        }
        self.pending_action = None;
        match reply {
            Ok(Msg::LastPromptReply {
                prompt: Some(text),
                cut,
                ..
            }) => self.emit(Effect::SaveSnippet { text, cut }),
            Ok(Msg::LastPromptReply { prompt: None, .. }) => self.notify("no prompt to save"),
            Ok(_) => self.notify("unexpected reply from the daemon"),
            Err(e) => {
                let notice = App::older_daemon_notice(host, &e);
                self.notify(
                    notice.unwrap_or_else(|| format!("could not read the last prompt: {e}")),
                );
            }
        }
        self.redraw = true;
    }

    /// The snippet was saved as `name` (or why not); `cut`: the prompt was
    /// cut at 64 KiB.
    pub fn on_snippet_saved(&mut self, result: Result<String, String>, cut: bool) {
        match result {
            Ok(name) if cut => self.notify(format!("saved snippet {name} (cut at 64 KiB)")),
            Ok(name) => self.notify(format!("saved snippet {name}")),
            Err(e) => self.notify(format!("could not save the snippet: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::LaunchCall;
    use crate::test_support::{alt, app_with, info, press};
    use crossterm::event::KeyCode;

    fn hit(conversation: &str, cwd: &str) -> SearchHit {
        SearchHit {
            conversation: conversation.into(),
            cwd: cwd.into(),
            modified: 0,
            title: "Fix the parser".into(),
            snippet: "…the needle…".into(),
            matches: 2,
        }
    }

    fn requests(app: &mut App) -> Vec<(Msg, ReplyTo)> {
        app.take_effects()
            .into_iter()
            .filter_map(|e| match e {
                Effect::Request { msg, to, .. } => Some((msg, to)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_search_lists_its_hits_and_enter_resumes_or_switches() {
        let mut app = app_with(&[info(Some(1), Some("open-conv"))]);
        app.activate(0);
        app.take_effects();
        app.builtin_action("search", "search", Some("needle".into()));
        let sent = requests(&mut app);
        let (Msg::Search { query, limit }, ReplyTo::Search { gen, host }) = sent[0].clone() else {
            panic!("no search: {sent:?}")
        };
        assert_eq!(
            (query.as_str(), limit, host.as_str()),
            ("needle", 50, "local")
        );
        let reply = Msg::SearchReply {
            query: "needle".into(),
            hits: vec![hit("new-conv", "/home/u/p"), hit("open-conv", "/srv/app")],
            scanned: 2,
            total: 2,
            truncated: false,
        };
        app.on_reply(
            ReplyTo::Search {
                gen,
                host: host.clone(),
            },
            Ok(reply),
        );
        let Some(Modal::Picker(picker)) = &app.modal else {
            panic!("no list")
        };
        assert_eq!(picker.title, "2 conversations · \"needle\"");
        assert!(
            picker.items[0].label.ends_with("~/p  Fix the parser"),
            "{}",
            picker.items[0].label
        );
        // The open conversation: its tab.
        let open = picker.items[1].result.clone();
        app.modal = None;
        let tabs = app.sessions.len();
        app.apply_result(open);
        assert_eq!(app.sessions.len(), tabs);
        // Another: a new tab resuming it, in its directory.
        app.apply_result(ActionResult::Resume {
            host: "local".into(),
            cwd: "/home/u/p".into(),
            conversation: "new-conv".into(),
        });
        assert_eq!(app.sessions.len(), tabs + 1);
        // A direct session goes through the launch composer.
        let spawned = app.take_effects().into_iter().find_map(|e| match e {
            Effect::Launch(LaunchCall {
                base: Msg::Spawn(spec),
                ..
            }) => Some(spec),
            _ => None,
        });
        let spec = spawned.expect("a spawn");
        assert_eq!(spec.cwd, "/home/u/p");
        assert_eq!(spec.args, ["--resume", "new-conv"]);
    }

    /// Open the wizard (Alt+n) unless it is, type `text` on its first
    /// screen and press Enter: the `SearchAll` that emits.
    fn wizard_search(app: &mut App, text: &str) -> (u64, String) {
        if app.modal.is_none() {
            app.on_terminal(alt('n'));
        }
        for c in text.chars() {
            app.on_terminal(press(KeyCode::Char(c)));
        }
        app.on_terminal(press(KeyCode::Enter));
        app.take_effects()
            .into_iter()
            .find_map(|e| match e {
                Effect::SearchAll { gen, query } => Some((gen, query)),
                _ => None,
            })
            .expect("a SearchAll")
    }

    fn host_hit(host: &str, home: &str, conversation: &str, cwd: &str, modified: u64) -> HostHit {
        let hit = SearchHit {
            modified,
            ..hit(conversation, cwd)
        };
        let (host, home) = (host.to_owned(), home.to_owned());
        HostHit { host, home, hit }
    }

    #[test]
    fn the_wizard_search_row_lists_every_hosts_hits_and_resumes() {
        let mut app = app_with(&[info(Some(1), Some("open-conv"))]);
        app.activate(0);
        app.on_terminal(alt('n'));
        assert!(matches!(app.modal, Some(Modal::Wizard(_))));
        let (gen, query) = wizard_search(&mut app, "zyzzyva");
        assert_eq!(query, "zyzzyva");
        assert!(app.modal.is_none(), "Enter closes the wizard");
        let done = |gen, hits, failed| SearchAllDone {
            gen,
            query: query.clone(),
            searched: 3,
            hits,
            failed,
        };
        // A reply for an older search is dropped.
        app.on_search_all(done(
            gen - 1,
            vec![host_hit("local", "/home/u", "x", "/x", 1)],
            vec![],
        ));
        assert!(app.modal.is_none());
        let hits = vec![
            host_hit("local", "/home/u", "open-conv", "/home/u/p", 200),
            host_hit("z6", "/srv", "far-conv", "/srv/app", 100),
        ];
        let failed = vec![("box".to_owned(), "timed out".to_owned())];
        app.on_search_all(done(gen, hits, failed));
        let Some(Modal::Picker(picker)) = &app.modal else {
            panic!("no list")
        };
        assert_eq!(picker.title, "2 conversations · \"zyzzyva\"");
        assert!(
            picker.items[0]
                .label
                .ends_with("local  ~/p  Fix the parser"),
            "{}",
            picker.items[0].label
        );
        assert!(
            picker.items[1].label.ends_with("z6  ~/app  Fix the parser"),
            "{}",
            picker.items[1].label
        );
        assert_eq!(
            app.notice.as_ref().unwrap().text,
            "search failed on box (timed out)"
        );
        // A hit whose conversation is open in a tab: that tab, no new one.
        let open = picker.items[0].result.clone();
        app.modal = None;
        let tabs = app.sessions.len();
        app.apply_result(open);
        assert_eq!(app.sessions.len(), tabs);
        assert_eq!(app.active, Some(0));
    }

    #[test]
    fn a_wizard_search_says_why_it_found_nothing_and_runs_one_at_a_time() {
        let mut app = app_with(&[]);
        let (gen, query) = wizard_search(&mut app, "zyzzyva");
        // One at a time: a second search waits for the first.
        app.on_terminal(alt('n'));
        for c in "other".chars() {
            app.on_terminal(press(KeyCode::Char(c)));
        }
        app.on_terminal(press(KeyCode::Enter));
        assert!(!app
            .take_effects()
            .iter()
            .any(|e| matches!(e, Effect::SearchAll { .. })));
        assert_eq!(
            app.notice.as_ref().unwrap().text,
            "a search is already running"
        );
        let none = SearchAllDone {
            gen,
            query,
            searched: 0,
            hits: vec![],
            failed: vec![],
        };
        app.on_search_all(none);
        assert_eq!(
            app.notice.as_ref().unwrap().text,
            "no connected host can search (run `claudio daemon restart` there)"
        );
        let (gen, query) = wizard_search(&mut app, "zyzzyva");
        let failed = vec![("z6".to_owned(), "connection closed".to_owned())];
        let (searched, hits) = (2, vec![]);
        app.on_search_all(SearchAllDone {
            gen,
            query,
            searched,
            hits,
            failed,
        });
        assert_eq!(
            app.notice.as_ref().unwrap().text,
            "search failed on z6 (connection closed)"
        );
        let (gen, query) = wizard_search(&mut app, "zyzzyva");
        let (searched, hits, failed) = (1, vec![], vec![]);
        app.on_search_all(SearchAllDone {
            gen,
            query,
            searched,
            hits,
            failed,
        });
        assert_eq!(app.notice.as_ref().unwrap().text, "no matches");
        assert!(app.modal.is_none());
    }

    #[test]
    fn a_hit_without_a_directory_is_not_resumed() {
        let mut app = app_with(&[info(Some(1), None)]);
        let tabs = app.sessions.len();
        app.take_effects();
        app.apply_result(ActionResult::Resume {
            host: "local".into(),
            cwd: String::new(),
            conversation: "c".into(),
        });
        assert_eq!(app.sessions.len(), tabs);
        assert_eq!(
            app.notice.as_ref().map(|n| n.text.as_str()),
            Some("that conversation has no working directory to resume in")
        );
        assert!(!app.take_effects().iter().any(|e| matches!(
            e,
            Effect::Request {
                msg: Msg::Spawn(_),
                ..
            }
        )));
    }

    #[test]
    fn search_notices_say_what_went_wrong() {
        let mut app = app_with(&[info(Some(1), None)]);
        let ask = |app: &mut App, reply: io::Result<Msg>| {
            app.builtin_action("search", "search", Some("x".into()));
            let (_, to) = requests(app).remove(0);
            app.on_reply(to, reply);
            app.notice
                .as_ref()
                .map(|n| n.text.clone())
                .unwrap_or_default()
        };
        let none = Msg::SearchReply {
            query: "x".into(),
            hits: vec![],
            scanned: 0,
            total: 0,
            truncated: false,
        };
        assert_eq!(ask(&mut app, Ok(none)), "no matches");
        let busy = io::Error::other("search busy");
        assert_eq!(ask(&mut app, Err(busy)), "search busy, try again");
        let old = io::Error::other("unsupported op: search");
        assert_eq!(
            ask(&mut app, Err(old)),
            "daemon on local is older: run `claudio daemon restart`"
        );
        // A list that arrives with a popup open says so instead.
        let hits = Msg::SearchReply {
            query: "x".into(),
            hits: vec![hit("c", "/p")],
            scanned: 1,
            total: 1,
            truncated: false,
        };
        app.modal = Some(Modal::Picker(Box::new(Picker::new("open".into(), vec![]))));
        assert_eq!(ask(&mut app, Ok(hits)), "close the popup and run it again");
        let Some(Modal::Picker(open)) = &app.modal else {
            panic!("the open popup is gone")
        };
        assert_eq!(open.title, "open");
        // An empty query asks nothing.
        app.modal = None;
        app.builtin_action("search", "search", Some("  ".into()));
        assert!(requests(&mut app).is_empty());
        assert_eq!(app.notice.as_ref().unwrap().text, "search needs a query");
    }

    #[test]
    fn snippets_list_and_paste_and_the_last_prompt_is_saved() {
        let mut app = app_with(&[info(Some(1), None)]);
        app.activate(0);
        app.take_effects();
        app.builtin_action("snippets", "insert", None);
        let gen = app
            .take_effects()
            .into_iter()
            .find_map(|e| match e {
                Effect::LoadSnippets { gen } => Some(gen),
                _ => None,
            })
            .expect("a load");
        let snippets = vec![(
            "review".to_owned(),
            "\n  Review this.\nThen more.".to_owned(),
        )];
        app.on_snippets(gen, "/cfg/snippets", Ok(snippets));
        let Some(Modal::Picker(picker)) = &app.modal else {
            panic!("no list")
        };
        assert_eq!(picker.items[0].detail.as_deref(), Some("Review this."));
        app.modal = None;
        app.builtin_action("snippets", "insert", None);
        let gen = app.modal_gen;
        app.on_snippets(gen, "/cfg/snippets", Ok(vec![]));
        assert_eq!(
            app.notice.as_ref().unwrap().text,
            "no snippets in /cfg/snippets"
        );
        // A list that arrives with a popup open says so instead.
        app.builtin_action("snippets", "insert", None);
        let gen = app.modal_gen;
        app.modal = Some(Modal::Picker(Box::new(Picker::new("open".into(), vec![]))));
        let late = vec![("review".to_owned(), "Review this.".to_owned())];
        app.on_snippets(gen, "/cfg/snippets", Ok(late));
        assert_eq!(
            app.notice.as_ref().unwrap().text,
            "close the popup and run it again"
        );
        app.modal = None;

        app.builtin_action("snippets", "save", None);
        let (msg, to) = requests(&mut app).remove(0);
        assert!(matches!(msg, Msg::LastPrompt { .. }));
        let id = app.sessions[0].id;
        let reply = Msg::LastPromptReply {
            id,
            prompt: Some("Fix it".into()),
            cut: true,
        };
        app.on_reply(to, Ok(reply));
        let saves: Vec<(String, bool)> = app
            .take_effects()
            .into_iter()
            .filter_map(|e| match e {
                Effect::SaveSnippet { text, cut } => Some((text, cut)),
                _ => None,
            })
            .collect();
        assert_eq!(saves, [("Fix it".to_owned(), true)]);
        app.on_snippet_saved(Ok("fix-it".into()), true);
        assert_eq!(
            app.notice.as_ref().unwrap().text,
            "saved snippet fix-it (cut at 64 KiB)"
        );
        app.builtin_action("snippets", "save", None);
        let (_, to) = requests(&mut app).remove(0);
        app.on_reply(
            to,
            Ok(Msg::LastPromptReply {
                id,
                prompt: None,
                cut: false,
            }),
        );
        assert_eq!(app.notice.as_ref().unwrap().text, "no prompt to save");
    }
}
