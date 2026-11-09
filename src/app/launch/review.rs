//! A launch parked for a review (docs/addons.md, "Launches", step 3): what
//! is done so far, the addon waiting, and the budget left. The clock is
//! stopped while it waits.

use std::fmt;
use std::time::Duration;

use tokio::time::Instant;

use super::{proceed, request, LaunchSpec, Routings, Step};
use crate::addon::{
    line, ChecklistAnswer, LaunchChoice, LaunchOp, LaunchPart, Routing, TEXT_BYTES,
};
use crate::app::addons::AddonRuntime;
use crate::client::Client;
use crate::proto::SessionId;

/// A launch in progress: stopped for a review when a [`Step::Review`]
/// holds it. It holds the addons' parts, secrets included: keep it in the
/// launch's runner, never in a front-end's state. Its `Debug` names addons
/// only.
pub struct Parked {
    pub(super) spec: LaunchSpec,
    pub(super) choice: Option<LaunchChoice>,
    /// Every Launch addon's routing, in registry order.
    routings: Routings,
    /// The parts so far.
    pub(super) parts: Vec<(&'static str, LaunchPart)>,
    /// The addons still to prepare.
    pub(super) todo: Vec<&'static str>,
    /// Optional addons' failures.
    pub(super) notices: Vec<String>,
    /// Checklists skipped for this launch, per addon.
    deferred: Vec<(&'static str, Vec<String>)>,
    /// The addon whose checklist is open, and its id.
    pub(super) waiting: Option<(&'static str, String)>,
    /// The budget left.
    pub(super) remaining: Duration,
}

/// The member's answer to a parked launch's checklist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewAnswer {
    /// Save it (`Ui::save_checklist`), then prepare the addon again.
    Save(ChecklistAnswer),
    /// Prepare the addon again with the checklist skipped, for this launch
    /// only (never reaches the addon).
    Skip,
    /// Drop the launch (never reaches the addon).
    Cancel,
}

impl Parked {
    pub(super) fn new(
        spec: LaunchSpec,
        choice: Option<LaunchChoice>,
        routings: Routings,
        todo: Vec<&'static str>,
        remaining: Duration,
    ) -> Parked {
        Parked {
            spec,
            choice,
            routings,
            parts: Vec::new(),
            todo,
            notices: Vec::new(),
            deferred: Vec::new(),
            waiting: None,
            remaining,
        }
    }

    /// The budget left.
    pub fn remaining(&self) -> Duration {
        self.remaining
    }

    /// The session being launched.
    pub fn sid(&self) -> SessionId {
        self.spec.sid
    }

    /// Whether this launch never waits for a review (a respawn or a
    /// recovery: a review counts as skipped).
    pub(super) fn never_waits(&self) -> bool {
        matches!(self.spec.op, LaunchOp::Respawn | LaunchOp::Recover)
    }

    /// The checklists `addon` skips for this launch.
    pub(super) fn deferred(&self, addon: &str) -> &[String] {
        self.deferred
            .iter()
            .find(|(a, _)| *a == addon)
            .map_or(&[], |(_, list)| list.as_slice())
    }

    /// Skip `checklist` of `addon` for this launch.
    pub(super) fn defer(&mut self, addon: &'static str, checklist: String) {
        match self.deferred.iter_mut().find(|(a, _)| *a == addon) {
            Some((_, list)) => list.push(checklist),
            None => self.deferred.push((addon, vec![checklist])),
        }
    }

    fn routing(&self, addon: &str) -> Routing {
        self.routings
            .iter()
            .find(|(a, _)| *a == addon)
            .map_or(Routing::Off, |(_, r)| r.clone())
    }

    /// `addon` could not contribute: a Required one fails the launch
    /// (`Err`, the reason), an Optional one becomes a notice.
    pub(super) fn failed(&mut self, addon: &'static str, why: &str) -> Result<(), String> {
        let text = format!("{addon}: {}", line(why, TEXT_BYTES));
        match self.routing(addon) {
            Routing::Required(_) => Err(text),
            _ => {
                self.notices.push(text);
                Ok(())
            }
        }
    }

    /// What `addon` is asked to prepare.
    pub(super) fn request(&self, addon: &str) -> crate::addon::LaunchRequest {
        let mut req = request(&self.spec, self.choice.clone(), self.routing(addon));
        req.deferred = self.deferred(addon).to_vec();
        req
    }

    /// The parts in registry order.
    pub(super) fn sort_parts(&mut self) {
        let order: Vec<&str> = self.routings.iter().map(|(a, _)| *a).collect();
        let rank = |a: &str| order.iter().position(|o| *o == a);
        self.parts.sort_by_key(|(a, _)| rank(a));
    }
}

impl fmt::Debug for Parked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let parts: Vec<&str> = self.parts.iter().map(|(a, _)| *a).collect();
        f.debug_struct("Parked")
            .field("sid", &self.spec.sid)
            .field("op", &self.spec.op)
            .field("parts", &parts)
            .field("todo", &self.todo)
            .field("waiting", &self.waiting)
            .field("remaining", &self.remaining)
            .finish_non_exhaustive()
    }
}

/// Continue a parked launch: Save → `ui.save_checklist`, then that addon's
/// prepare again; Skip → its prepare again with the checklist deferred;
/// Cancel → `Failed("cancelled")`. The budget left runs again from now.
pub async fn resume(
    rt: &AddonRuntime,
    mut parked: Parked,
    answer: ReviewAnswer,
    client: Client,
) -> Step {
    let Some((addon, checklist)) = parked.waiting.take() else {
        return Step::Failed("no review to answer".to_owned());
    };
    match answer {
        ReviewAnswer::Cancel => return Step::Failed("cancelled".to_owned()),
        ReviewAnswer::Skip => parked.defer(addon, checklist),
        ReviewAnswer::Save(answer) => {
            if let Err(why) = save(rt, &mut parked, addon, checklist, answer).await {
                parked.todo.retain(|a| *a != addon);
                if let Err(failed) = parked.failed(addon, &why) {
                    return Step::Failed(failed);
                }
            }
        }
    }
    proceed(rt, parked, client).await
}

/// `addon`'s `save_checklist`, within the budget left.
async fn save(
    rt: &AddonRuntime,
    parked: &mut Parked,
    addon: &'static str,
    checklist: String,
    answer: ChecklistAnswer,
) -> Result<(), String> {
    let Some(entry) = rt.addons().get(addon).cloned() else {
        return Err("not available".to_owned());
    };
    let Some(ui) = entry.ui() else {
        return Err("has no checklist to save".to_owned());
    };
    let deadline = Instant::now() + parked.remaining;
    let saved = ui.save_checklist(checklist, answer, rt.ctx(addon));
    let result = tokio::time::timeout_at(deadline, saved).await;
    parked.remaining = deadline.saturating_duration_since(Instant::now());
    match result {
        Ok(saved) => saved,
        Err(_) => Err("timed out".to_owned()),
    }
}
