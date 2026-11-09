//! What `App` asks of the addons ([`AddonCall`], run by `addon_runner`)
//! and what comes back ([`AddonResult`]). `App` never sees the runtime
//! (A6): it emits calls as effects and takes results as events.

use crate::addon::{
    ActionRequest, Checklist, ChecklistAnswer, LaunchOption, Notice, Panel, PanelRequest,
};
use crate::addons::vocdoni::bridge::Bridge;
use crate::addons::vocdoni::proxy::plugins::PluginList;
use crate::app::launch::ReviewAnswer;
use crate::proto::SessionId;

use super::app::App;
use super::extensions_view::ExtensionsView;

/// Where a checklist view came from: what Esc does, and where its answer
/// goes.
#[derive(Debug, Clone, PartialEq)]
pub enum Origin {
    /// Launch `id` of session `sid` waits for it: Enter saves and the
    /// launch goes on, `s` skips it for this launch, Esc cancels the launch.
    Launch { id: u64, sid: SessionId },
    /// The addon offered it (first sight): Esc puts it off for this run.
    Offer,
    /// Enter on an extensions row: Esc goes back to that screen.
    Row { back: Box<ExtensionsView> },
}

/// A call into an addon for the event loop's runner (`addon_runner::run`).
#[derive(Debug, Clone, PartialEq)]
pub enum AddonCall {
    /// Show `addon`'s checklist `id` (an [`AddonResult::Checklist`] back,
    /// or a notice when it has none).
    OpenChecklist {
        addon: &'static str,
        id: String,
        origin: Origin,
    },
    /// The member's answer to a checklist outside a launch; a refused save
    /// opens it again (from `origin`) with a notice.
    SaveChecklist {
        addon: &'static str,
        id: String,
        answer: ChecklistAnswer,
        origin: Origin,
    },
    /// The member's answer to the review launch `launch` waits for.
    ReviewAnswer { launch: u64, answer: ReviewAnswer },
    /// `addon`'s panel as `req` asks (10 s; an [`AddonResult::Panel`] with
    /// `gen` back).
    Panel {
        gen: u64,
        addon: &'static str,
        req: PanelRequest,
    },
    /// Run one of `addon`'s panel actions (15 s; an
    /// [`AddonResult::ActionDone`] back).
    Action {
        addon: &'static str,
        req: ActionRequest,
    },
}

/// What the addon runtime and the runner tell `App` (`addon_runner`).
#[derive(Debug, Clone, PartialEq)]
pub enum AddonResult {
    /// The addon's data changed: re-read what is shown of it.
    Changed { addon: &'static str },
    /// Every Launch addon's options, cleaned, in registry order (F11).
    Options(Vec<(&'static str, LaunchOption)>),
    /// Show `addon`'s checklist, cleaned (C2), from `origin`.
    Checklist {
        addon: &'static str,
        origin: Origin,
        checklist: Checklist,
    },
    /// `addon` offers its checklist `id` (D8): shown when idle.
    Offer { addon: &'static str, id: String },
    /// A notice, cleaned.
    Notice(Notice),
    /// `addon`'s panel for request `gen`, cleaned (an error page when it
    /// failed or timed out).
    Panel {
        gen: u64,
        addon: &'static str,
        panel: Panel,
    },
    /// A panel action of `addon` ended: its notice (cleaned) or why not.
    ActionDone {
        addon: &'static str,
        result: Result<Notice, String>,
    },
}

impl App {
    /// The Vocdoni addon's plugin state, when this build has the addon
    /// (the extension rows', until they are the addon's rows).
    pub(super) fn bridge(&self) -> Option<Bridge> {
        Bridge::of(&self.addons)
    }

    /// What the addon runtime says.
    pub fn on_addon(&mut self, result: AddonResult) {
        match result {
            AddonResult::Changed { .. } => self.on_plugins_changed(),
            AddonResult::Options(options) => self.on_launch_options(options),
            AddonResult::Checklist {
                addon,
                origin,
                checklist,
            } => self.on_checklist(addon, origin, checklist),
            AddonResult::Offer { addon, id } => self.on_offer(addon, id),
            AddonResult::Notice(notice) => self.notify(notice.text),
            AddonResult::Panel { gen, panel, .. } => self.on_panel(gen, panel),
            AddonResult::ActionDone { result, .. } => self.on_action_done(result),
        }
    }

    /// The plugin state changed: the registry's plugin rows.
    fn on_plugins_changed(&mut self) {
        let Some(bridge) = self.bridge() else {
            return;
        };
        let lists = bridge.lists();
        let lists: Vec<&PluginList> = lists.iter().collect();
        self.extensions.set_plugins(&lists, &bridge.enabled());
        self.redraw = true;
    }
}
