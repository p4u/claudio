//! What an addon shows: segments, panels, checklists and extension rows, as
//! data the front-ends render with generic views (docs/addons.md, "Data,
//! not widgets").

use std::sync::Arc;

use super::{AddonCtx, BoxFuture, SessionSnap};
use crate::proto::SessionId;

/// How a piece of text is coloured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Tone {
    /// Healthy.
    Ok,
    /// Needs attention soon.
    Warn,
    /// Needs attention now.
    Hot,
    /// Secondary.
    Muted,
    /// Highlighted.
    Accent,
}

/// One status-bar segment of a session. Priorities 0-99 go on the TUI's
/// status line 1, 100-255 on line 2 (D9).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Segment {
    /// `[a-z][a-z0-9-]{0,31}`, unique per addon.
    pub id: String,
    /// At most 64 characters.
    pub text: String,
    /// Its colour.
    pub tone: Tone,
    /// Its place: lower first; on a narrow line 2 the highest go first.
    pub priority: u8,
}

/// A panel's key binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PanelKey {
    /// The keymap action name (`proxy_stats`); `[keys] <action>` overrides it.
    pub action: &'static str,
    /// The default key spec (`alt+s`).
    pub default: &'static str,
}

/// A panel an addon offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PanelDecl {
    /// `[a-z][a-z0-9-]{0,31}`, unique per addon.
    pub id: &'static str,
    /// Its title in menus.
    pub title: &'static str,
    /// Keymap action name and default key spec (`proxy_stats`, `alt+s`).
    pub key: Option<PanelKey>,
    /// The windows a request may name, first = default (`["24h", "7d", "30d"]`).
    pub windows: &'static [&'static str],
}

/// A front-end's request for a panel, with its view-local state.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct PanelRequest {
    /// The panel's id.
    pub id: String,
    /// The page shown.
    #[serde(default)]
    pub page: usize,
    /// One of the panel's windows (`None`: the first).
    #[serde(default)]
    pub window: Option<String>,
    /// The session it is about, if any.
    #[serde(default)]
    pub sid: Option<SessionId>,
}

/// A panel: pages of plain-text blocks and actions.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Panel {
    /// Its title.
    pub title: String,
    /// Its pages.
    pub pages: Vec<Page>,
    /// Its actions.
    pub actions: Vec<PanelAction>,
    /// When its data was fetched (Unix seconds).
    pub updated: Option<u64>,
}

/// One page of a panel.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Page {
    /// Its tab title.
    pub title: String,
    /// Its content, top to bottom.
    pub blocks: Vec<Block>,
}

/// A block of a page; every string is plain text.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Block {
    /// A paragraph.
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        tone: Option<Tone>,
    },
    /// Key/value lines.
    KeyValues { items: Vec<(String, String)> },
    /// A table.
    Table {
        header: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    /// A usage gauge.
    Gauge {
        label: String,
        used: f64,
        limit: f64,
        resets: Option<String>,
    },
    /// A bar chart.
    Bars {
        label: String,
        series: Vec<(String, f64)>,
    },
}

/// An action a panel offers.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct PanelAction {
    /// Its id, passed back in `ActionRequest.action`.
    pub id: String,
    /// What the member sees.
    pub label: String,
    /// Its key inside the panel.
    pub key: Option<char>,
    /// A confirm prompt's text, if it needs one.
    pub confirm: Option<String>,
}

/// A front-end's request to run a panel action.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct ActionRequest {
    /// The panel's id.
    pub panel: String,
    /// The action's id.
    pub action: String,
    /// The session it is about, if any.
    #[serde(default)]
    pub sid: Option<SessionId>,
}

/// A list of items the member turns on or off (a plugin manifest's setup).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Checklist {
    /// Its id.
    pub id: String,
    /// Changes when the items change; a save names the revision it saw.
    pub revision: String,
    /// Its title.
    pub title: String,
    /// The text above the items.
    pub intro: String,
    /// At most 256.
    pub items: Vec<ChecklistItem>,
}

/// One item of a checklist.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ChecklistItem {
    /// Its id.
    pub id: String,
    /// What the member sees.
    pub label: String,
    /// One more line.
    pub detail: Option<String>,
    /// A heading printed when it changes (D9).
    pub section: Option<String>,
    /// Whether it is on now.
    pub on: bool,
}

/// What the member did with a checklist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChecklistAnswer {
    /// Saved: the revision seen and the item ids turned off.
    Save { revision: String, off: Vec<String> },
    /// Closed without deciding, for this run.
    Defer,
}

/// A row an addon adds to the extensions screen.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ExtensionRow {
    /// The `[extensions]` key.
    pub name: String,
    /// One line.
    pub description: String,
    /// When `[extensions] <name>` is absent.
    pub default_on: bool,
    /// Why it cannot run, if it cannot.
    pub refused: Option<String>,
    /// The details view's lines.
    pub details: Vec<String>,
    /// The checklist Enter opens (a plugin's setup).
    pub setup: Option<String>,
}

/// What happened to an extension row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowAction {
    /// The core wrote `[extensions] <row>` (D10).
    Toggled(bool),
    /// The member asked to refetch.
    Refresh,
}

/// A notice's level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    /// Informational.
    Info,
    /// A warning.
    Warn,
    /// An error.
    #[allow(dead_code)] // API surface (docs/addons.md); no addon reports one yet.
    Error,
}

/// A line the front-end shows the member.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Notice {
    /// The text.
    pub text: String,
    /// Its level.
    pub level: Level,
}

fn unsupported(what: &str) -> String {
    format!("{what} is not supported by this addon")
}

/// What an addon shows (docs/addons.md, "Data, not widgets").
pub trait Ui: Send + Sync {
    /// No I/O: a snapshot under a std Mutex never held across an await.
    fn segments(&self, session: &SessionSnap, ctx: &dyn AddonCtx) -> Vec<Segment> {
        let _ = (session, ctx);
        Vec::new()
    }
    /// The panels it offers.
    fn panels(&self) -> &'static [PanelDecl] {
        &[]
    }
    /// One panel's content (10 s, then an error page).
    fn panel<'a>(&'a self, req: PanelRequest, ctx: Arc<dyn AddonCtx>) -> BoxFuture<'a, Panel> {
        let _ = ctx;
        Box::pin(async move { super::error_panel(&req.id, &unsupported("this panel")) })
    }
    /// Run a panel action (15 s).
    fn action<'a>(
        &'a self,
        req: ActionRequest,
        ctx: Arc<dyn AddonCtx>,
    ) -> BoxFuture<'a, Result<Notice, String>> {
        let _ = (req, ctx);
        Box::pin(async { Err(unsupported("a panel action")) })
    }
    /// Checklist `id` as it is now. No I/O.
    fn checklist(&self, id: &str, ctx: &dyn AddonCtx) -> Option<Checklist> {
        let _ = (id, ctx);
        None
    }
    /// The member's answer to checklist `id`.
    fn save_checklist<'a>(
        &'a self,
        id: String,
        answer: ChecklistAnswer,
        ctx: Arc<dyn AddonCtx>,
    ) -> BoxFuture<'a, Result<(), String>> {
        let _ = (id, answer, ctx);
        Box::pin(async { Err(unsupported("a checklist")) })
    }
    /// Its rows on the extensions screen. No I/O.
    fn extension_rows(&self, ctx: &dyn AddonCtx) -> Vec<ExtensionRow> {
        let _ = ctx;
        Vec::new()
    }
    /// A row was toggled or refreshed (15 s).
    fn row_action<'a>(
        &'a self,
        row: String,
        action: RowAction,
        ctx: Arc<dyn AddonCtx>,
    ) -> BoxFuture<'a, Result<Option<Notice>, String>> {
        let _ = (row, action, ctx);
        Box::pin(async { Err(unsupported("a row action")) })
    }
}
