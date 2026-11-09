//! `TestAddon`: a configurable fake addon for unit tests of the core's addon
//! plumbing (registry, runtime, launch composer, daemon ops). It records
//! every call.

use std::collections::BTreeMap;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;

use super::*;

/// What `prepare` answers.
#[derive(Debug, Clone)]
pub(crate) enum Script {
    /// This part.
    Ok(LaunchPart),
    /// `LaunchError::Failed(why)`.
    Fail(String),
    /// The inner answer after a delay.
    Delay(Duration, Box<Script>),
    /// `NeedsReview` for the checklist until it is saved (or deferred), then this part.
    Review(LaunchPart),
}

/// The fake addon. Build with `TestAddon::new(name)` and the builder methods.
pub(crate) struct TestAddon {
    name: &'static str,
    sections: &'static [&'static str],
    flags: &'static [FlagDecl],
    panels: &'static [PanelDecl],
    words: BTreeMap<String, u8>,
    relays: BTreeMap<String, u8>,
    help: &'static str,
    has_launch: bool,
    has_ui: bool,
    has_daemon: bool,
    routing: Mutex<Result<Routing, String>>,
    prepare: Mutex<Script>,
    segments: Mutex<Vec<Segment>>,
    checklist: Mutex<Option<Checklist>>,
    rows: Mutex<Vec<ExtensionRow>>,
    saved: AtomicBool,
    installs: AtomicUsize,
    calls: Mutex<Vec<String>>,
}

impl TestAddon {
    /// An addon with no capabilities.
    pub fn new(name: &'static str) -> TestAddon {
        TestAddon {
            name,
            sections: &[],
            flags: &[],
            panels: &[],
            words: BTreeMap::new(),
            relays: BTreeMap::new(),
            help: "",
            has_launch: false,
            has_ui: false,
            has_daemon: false,
            routing: Mutex::new(Ok(Routing::Optional)),
            prepare: Mutex::new(Script::Ok(LaunchPart::default())),
            segments: Mutex::new(Vec::new()),
            checklist: Mutex::new(None),
            rows: Mutex::new(Vec::new()),
            saved: AtomicBool::new(false),
            installs: AtomicUsize::new(0),
            calls: Mutex::new(Vec::new()),
        }
    }

    /// Claims these config sections.
    pub fn sections(mut self, sections: &'static [&'static str]) -> Self {
        self.sections = sections;
        self
    }

    /// Has `Launch` with these flags.
    pub fn launch_flags(mut self, flags: &'static [FlagDecl]) -> Self {
        self.flags = flags;
        self.has_launch = true;
        self
    }

    /// Has `Launch`; `resolve` answers `routing`.
    pub fn routing(mut self, routing: Result<Routing, String>) -> Self {
        self.routing = Mutex::new(routing);
        self.has_launch = true;
        self
    }

    /// Has `Launch`; `prepare` follows `script`.
    pub fn prepare(mut self, script: Script) -> Self {
        self.prepare = Mutex::new(script);
        self.has_launch = true;
        self
    }

    /// Has `Ui` with these panels.
    pub fn panels(mut self, panels: &'static [PanelDecl]) -> Self {
        self.panels = panels;
        self.has_ui = true;
        self
    }

    /// Has `Ui` with these segments for every session.
    pub fn segments(mut self, segments: Vec<Segment>) -> Self {
        self.segments = Mutex::new(segments);
        self.has_ui = true;
        self
    }

    /// Has `Ui` with this checklist.
    pub fn checklist(mut self, checklist: Checklist) -> Self {
        self.checklist = Mutex::new(Some(checklist));
        self.has_ui = true;
        self
    }

    /// The checklist from now on (a new revision: the items changed).
    pub fn set_checklist(&self, checklist: Checklist) {
        *lock(&self.checklist) = Some(checklist);
    }

    /// Has `Ui` with these extension rows.
    pub fn rows(mut self, rows: Vec<ExtensionRow>) -> Self {
        self.rows = Mutex::new(rows);
        self.has_ui = true;
        self
    }

    /// Has `DaemonOps`: `echo`, `sleep` (`{"secs": n}`), `big` (600 KiB), `fail`.
    pub fn with_daemon(mut self) -> Self {
        self.has_daemon = true;
        self
    }

    /// `claudio <word>` exits with `code`.
    pub fn command(mut self, word: &str, code: u8) -> Self {
        self.words.insert(word.into(), code);
        self
    }

    /// `claudio <word> …` is a relay that exits with `code`.
    pub fn relay(mut self, word: &str, code: u8) -> Self {
        self.relays.insert(word.into(), code);
        self
    }

    /// Its `--help` appendix (gives it `Commands`).
    pub fn help_text(mut self, help: &'static str) -> Self {
        self.help = help;
        self
    }

    /// Change what `prepare` answers from now on.
    pub fn set_prepare(&self, script: Script) {
        *lock(&self.prepare) = script;
    }

    /// Change what `resolve` answers from now on.
    pub fn set_routing(&self, routing: Result<Routing, String>) {
        *lock(&self.routing) = routing;
    }

    /// Every call so far, in order (`"prepare New"`, `"daemon echo"`, …).
    pub fn calls(&self) -> Vec<String> {
        lock(&self.calls).clone()
    }

    /// How many times `on_claude_installed` ran.
    pub fn installs(&self) -> usize {
        self.installs.load(Ordering::SeqCst)
    }

    /// Whether the checklist was saved at its current revision.
    pub fn saved(&self) -> bool {
        self.saved.load(Ordering::SeqCst)
    }

    fn record(&self, call: String) {
        lock(&self.calls).push(call);
    }

    fn has_commands(&self) -> bool {
        !self.words.is_empty() || !self.relays.is_empty() || !self.help.is_empty()
    }

    fn review_id(&self) -> (String, String) {
        lock(&self.checklist)
            .as_ref()
            .map_or(("setup".into(), "1".into()), |c| {
                (c.id.clone(), c.revision.clone())
            })
    }

    async fn run_script(
        &self,
        script: Script,
        deferred: &[String],
    ) -> Result<LaunchPart, LaunchError> {
        let mut script = script;
        loop {
            match script {
                Script::Ok(part) => return Ok(part),
                Script::Fail(why) => return Err(LaunchError::Failed(why)),
                Script::Delay(wait, inner) => {
                    tokio::time::sleep(wait).await;
                    script = *inner;
                }
                Script::Review(part) => {
                    let (checklist, revision) = self.review_id();
                    if self.saved() || deferred.contains(&checklist) {
                        return Ok(part);
                    }
                    return Err(LaunchError::NeedsReview {
                        checklist,
                        revision,
                    });
                }
            }
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Addon for TestAddon {
    fn name(&self) -> &'static str {
        self.name
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn config_sections(&self) -> &'static [&'static str] {
        self.sections
    }
    fn commands(&self) -> Option<&dyn Commands> {
        self.has_commands().then_some(self as &dyn Commands)
    }
    fn launch(&self) -> Option<&dyn Launch> {
        self.has_launch.then_some(self as &dyn Launch)
    }
    fn ui(&self) -> Option<&dyn Ui> {
        self.has_ui.then_some(self as &dyn Ui)
    }
    fn daemon(&self) -> Option<&dyn DaemonOps> {
        self.has_daemon.then_some(self as &dyn DaemonOps)
    }
    fn on_event(&self, event: &AddonEvent, ctx: &Arc<dyn AddonCtx>) {
        let _ = ctx;
        self.record(format!("event {event:?}"));
    }
}

impl Commands for TestAddon {
    fn is_relay(&self, argv: &[String]) -> bool {
        self.record("is_relay".into());
        argv.get(1).is_some_and(|w| self.relays.contains_key(w))
    }
    fn dispatch(&self, argv: &[String], addons: &Arc<Addons>) -> Option<ExitCode> {
        let _ = addons;
        let word = argv.get(1).map_or("", String::as_str);
        self.record(format!("dispatch {word}"));
        let code = self.relays.get(word).or_else(|| self.words.get(word))?;
        Some(ExitCode::from(*code))
    }
    fn help(&self) -> &'static str {
        self.help
    }
}

impl Launch for TestAddon {
    fn flags(&self) -> &'static [FlagDecl] {
        self.flags
    }
    /// A flag with a value picks the option it names (an empty one is a
    /// usage error), a `--no-…` switch picks direct, any other switch is
    /// one of the select's flags.
    fn select(&self, given: &[FlagArg]) -> Result<LaunchSelect, String> {
        self.record(format!("select {}", given.len()));
        let mut select = LaunchSelect::default();
        for flag in given {
            match &flag.value {
                Some(v) if v.is_empty() => return Err(format!("{} needs a value", flag.name)),
                Some(v) => {
                    select.pick = Pick::Option {
                        addon: self.name.to_owned(),
                        option: v.clone(),
                    }
                }
                None if flag.name.starts_with("--no-") => select.pick = Pick::Direct,
                None => select.flags.push(flag.name.clone()),
            }
        }
        Ok(select)
    }
    fn resolve(&self, select: &LaunchSelect, ctx: &dyn AddonCtx) -> Result<Routing, String> {
        let _ = ctx;
        self.record(format!("resolve {:?}", select.pick));
        lock(&self.routing).clone()
    }
    fn prepare<'a>(
        &'a self,
        req: LaunchRequest,
        ctx: Arc<dyn AddonCtx>,
    ) -> BoxFuture<'a, Result<LaunchPart, LaunchError>> {
        let _ = ctx;
        self.record(format!("prepare {:?}", req.op));
        let script = lock(&self.prepare).clone();
        Box::pin(async move { self.run_script(script, &req.deferred).await })
    }
}

impl Ui for TestAddon {
    fn segments(&self, session: &SessionSnap, ctx: &dyn AddonCtx) -> Vec<Segment> {
        let _ = (session, ctx);
        self.record("segments".into());
        lock(&self.segments).clone()
    }
    fn panels(&self) -> &'static [PanelDecl] {
        self.panels
    }
    fn panel<'a>(&'a self, req: PanelRequest, ctx: Arc<dyn AddonCtx>) -> BoxFuture<'a, Panel> {
        let _ = ctx;
        self.record(format!("panel {}", req.id));
        Box::pin(async move {
            Panel {
                title: req.id.clone(),
                pages: vec![Page {
                    title: format!("page {}", req.page),
                    blocks: vec![Block::Text {
                        text: format!("window {}", req.window.as_deref().unwrap_or("-")),
                        tone: None,
                    }],
                }],
                actions: vec![PanelAction {
                    id: "go".into(),
                    label: "Go".into(),
                    key: Some('g'),
                    confirm: None,
                }],
                updated: None,
            }
        })
    }
    fn action<'a>(
        &'a self,
        req: ActionRequest,
        ctx: Arc<dyn AddonCtx>,
    ) -> BoxFuture<'a, Result<Notice, String>> {
        let _ = ctx;
        self.record(format!("action {}", req.action));
        Box::pin(async move {
            Ok(Notice {
                text: format!("ran {}", req.action),
                level: Level::Info,
            })
        })
    }
    fn checklist(&self, id: &str, ctx: &dyn AddonCtx) -> Option<Checklist> {
        let _ = ctx;
        self.record(format!("checklist {id}"));
        lock(&self.checklist).clone().filter(|c| c.id == id)
    }
    fn save_checklist<'a>(
        &'a self,
        id: String,
        answer: ChecklistAnswer,
        ctx: Arc<dyn AddonCtx>,
    ) -> BoxFuture<'a, Result<(), String>> {
        let _ = ctx;
        self.record(format!("save_checklist {id}"));
        Box::pin(async move {
            match answer {
                ChecklistAnswer::Defer => Ok(()),
                ChecklistAnswer::Save { revision, .. } => {
                    if self.review_id() != (id, revision) {
                        return Err("stale revision".into());
                    }
                    self.saved.store(true, Ordering::SeqCst);
                    Ok(())
                }
            }
        })
    }
    fn extension_rows(&self, ctx: &dyn AddonCtx) -> Vec<ExtensionRow> {
        let _ = ctx;
        self.record("rows".into());
        lock(&self.rows).clone()
    }
    fn row_action<'a>(
        &'a self,
        row: String,
        action: RowAction,
        ctx: Arc<dyn AddonCtx>,
    ) -> BoxFuture<'a, Result<Option<Notice>, String>> {
        let _ = ctx;
        self.record(format!("row_action {row} {action:?}"));
        Box::pin(async { Ok(None) })
    }
}

impl DaemonOps for TestAddon {
    fn handle<'a>(
        &'a self,
        op: &'a str,
        body: Value,
        ctx: Arc<dyn DaemonCtx>,
    ) -> BoxFuture<'a, Result<Value, String>> {
        let _ = ctx;
        self.record(format!("daemon {op}"));
        Box::pin(async move {
            match op {
                "echo" => Ok(body),
                "sleep" => {
                    let secs = body.get("secs").and_then(Value::as_f64).unwrap_or(0.0);
                    tokio::time::sleep(Duration::from_secs_f64(secs.max(0.0))).await;
                    Ok(serde_json::json!({ "slept": secs }))
                }
                "big" => Ok(Value::String("x".repeat(600 * 1024))),
                "fail" => Err("echo failed".into()),
                other => Err(format!("unknown op {other}")),
            }
        })
    }
    fn on_claude_installed(&self, ctx: &Arc<dyn DaemonCtx>) {
        let _ = ctx;
        self.record("installed".into());
        self.installs.fetch_add(1, Ordering::SeqCst);
    }
}
