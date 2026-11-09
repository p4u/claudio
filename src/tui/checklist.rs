//! One view for every addon checklist (docs/addons.md, "Launches", step
//! 3): a plugin's setup at a launch's review, at first sight (the addon's
//! offer) and from the extensions screen. Each item has a checkbox; the
//! member chooses what is on.
//!
//! Keys: Up/Down (or j/k) move, Space toggles, `a` turns every item on,
//! `n` every item off, Enter saves. Esc cancels the launch at a review,
//! puts an offer off for this run, and goes back to the extensions screen
//! from there; `s` skips a review for this launch. The view swallows every
//! key and waits its turn in the confirm queue like a prompt; one the
//! member did not open ignores keys for a moment after appearing, so a
//! keystroke meant for a session cannot answer it. A new revision of the
//! same checklist while it is open or queued replaces its items, keeping
//! the toggles of the ids still there.
//!
//! Pure, like `confirm.rs`; drawn by `ui/checklist.rs`. Every string in it
//! went through the core's `addon::clean` in the runner.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::addon::{Checklist, ChecklistAnswer};
use crate::app::extensions::Source;
use crate::app::launch::ReviewAnswer;

use super::addon_calls::{AddonCall, Origin};
use super::app::{App, Effect, Mode};
use super::confirm::Queued;
use super::extensions_view::ExtensionsView;
use super::interaction::Modal;

/// Ticks (250 ms each) during which a view the member did not open
/// ignores keys.
pub const ARM_TICKS: u8 = 4;

/// An addon's checklist on screen.
#[derive(Debug, Clone, PartialEq)]
pub struct ChecklistView {
    pub addon: &'static str,
    /// The items, their `on` as the member set them.
    pub checklist: Checklist,
    pub selected: usize,
    /// Remaining ticks before keys count.
    pub(super) armed: u8,
    pub origin: Origin,
}

/// What a key did.
#[derive(Debug, PartialEq)]
enum Outcome {
    Stay,
    Save,
    /// `s` at a launch's review.
    Skip,
    Esc,
}

impl ChecklistView {
    /// `checklist` of `addon`, opened from `origin`.
    pub fn new(addon: &'static str, checklist: Checklist, origin: Origin) -> ChecklistView {
        let armed = match origin {
            Origin::Row { .. } => 0,
            _ => ARM_TICKS,
        };
        ChecklistView {
            addon,
            checklist,
            selected: 0,
            armed,
            origin,
        }
    }

    /// Show `next` (a new revision) instead, keeping the toggle of every
    /// id still there; a new id is as `next` has it.
    pub fn replace(&mut self, mut next: Checklist) {
        let off = self.off_ids();
        let known: Vec<String> = self.checklist.items.iter().map(|i| i.id.clone()).collect();
        for item in &mut next.items {
            if known.contains(&item.id) {
                item.on = !off.contains(&item.id);
            }
        }
        self.selected = self.selected.min(next.items.len().saturating_sub(1));
        self.checklist = next;
    }

    /// The ids turned off.
    pub fn off_ids(&self) -> Vec<String> {
        let items = self.checklist.items.iter();
        items.filter(|i| !i.on).map(|i| i.id.clone()).collect()
    }

    /// Enter's answer: the revision shown and what is off.
    pub fn answer(&self) -> ChecklistAnswer {
        ChecklistAnswer::Save {
            revision: self.checklist.revision.clone(),
            off: self.off_ids(),
        }
    }

    fn on_key(&mut self, key: &KeyEvent) -> Outcome {
        // Ctrl or Alt chords are not answers; Shift only makes a capital.
        if self.armed > 0 || !key.modifiers.difference(KeyModifiers::SHIFT).is_empty() {
            return Outcome::Stay;
        }
        let items = &mut self.checklist.items;
        let last = items.len().saturating_sub(1);
        match key.code {
            KeyCode::Enter => return Outcome::Save,
            KeyCode::Esc => return Outcome::Esc,
            KeyCode::Char('s') if matches!(self.origin, Origin::Launch { .. }) => {
                return Outcome::Skip
            }
            KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.selected = (self.selected + 1).min(last),
            KeyCode::Char(' ') => {
                if let Some(item) = items.get_mut(self.selected) {
                    item.on = !item.on;
                }
            }
            KeyCode::Char('a') => items.iter_mut().for_each(|i| i.on = true),
            KeyCode::Char('n') => items.iter_mut().for_each(|i| i.on = false),
            _ => {}
        }
        Outcome::Stay
    }

    /// The key hint, by origin.
    pub fn hint(&self) -> &'static str {
        match self.origin {
            Origin::Launch { .. } => {
                "Space toggle · a all on · n all off · Enter save · s skip · Esc cancel"
            }
            Origin::Offer => "Space toggle · a all on · n all off · Enter save · Esc later",
            Origin::Row { .. } => "Space toggle · a all on · n all off · Enter save · Esc cancel",
        }
    }

    fn is(&self, addon: &str, id: &str) -> bool {
        self.addon == addon && self.checklist.id == id
    }
}

impl App {
    /// Every checklist view open or queued.
    fn checklist_views(&mut self) -> Vec<&mut ChecklistView> {
        let open = match &mut self.modal {
            Some(Modal::Checklist(v)) => Some(&mut **v),
            _ => None,
        };
        let queued = self.confirms.iter_mut().filter_map(|q| match q {
            Queued::Checklist(v) => Some(&mut **v),
            _ => None,
        });
        open.into_iter().chain(queued).collect()
    }

    /// The `(addon, id)` of every checklist open or queued: the runner
    /// reads them again when their addon changes.
    pub(super) fn checklists_shown(&mut self) -> Vec<(&'static str, String)> {
        let views = self.checklist_views();
        views
            .into_iter()
            .map(|v| (v.addon, v.checklist.id.clone()))
            .collect()
    }

    /// The addon's checklist as it is now: every view of it open or queued
    /// shows the new revision.
    pub(super) fn refresh_checklist(&mut self, addon: &str, checklist: &Checklist) {
        for view in self.checklist_views() {
            if view.is(addon, &checklist.id) && view.checklist.revision != checklist.revision {
                view.replace(checklist.clone());
            }
        }
        self.redraw = true;
    }

    /// Show `checklist` from `origin`: over the extensions screen at once
    /// for a row, else queued like a prompt. An offer already shown is
    /// dropped; a launch's review of one shown as an offer takes it over.
    pub(super) fn on_checklist(
        &mut self,
        addon: &'static str,
        origin: Origin,
        checklist: Checklist,
    ) {
        let id = checklist.id.clone();
        let mut views = self.checklist_views();
        let taken = match (&origin, views.iter_mut().find(|v| v.is(addon, &id))) {
            (Origin::Offer, Some(_)) => true,
            (Origin::Launch { .. }, Some(v)) if v.origin == Origin::Offer => {
                v.origin = origin.clone();
                v.replace(checklist.clone());
                true
            }
            _ => false,
        };
        if taken {
            self.redraw = true;
            return;
        }
        let view = Box::new(ChecklistView::new(addon, checklist, origin));
        if matches!(view.origin, Origin::Row { .. }) {
            self.modal = Some(Modal::Checklist(view));
        } else {
            self.confirms.push_back(Queued::Checklist(view));
            self.show_next_confirm();
        }
        self.redraw = true;
    }

    /// `addon` offers checklist `id`: asked for unless shown already.
    pub(super) fn on_offer(&mut self, addon: &'static str, id: String) {
        if self
            .checklists_shown()
            .iter()
            .any(|(a, i)| *a == addon && *i == id)
        {
            return;
        }
        let origin = Origin::Offer;
        self.emit(Effect::Addon(AddonCall::OpenChecklist {
            addon,
            id,
            origin,
        }));
    }

    /// Enter on the extensions screen's plugin row `index`: its setup
    /// (the addon's checklist), over (and back to) the extensions screen.
    pub(super) fn open_plugin_setup(&mut self, index: usize, back: ExtensionsView) {
        let Some(ext) = self.extensions.extensions.get(index) else {
            return;
        };
        let name = ext.name.clone();
        if let Source::Plugin {
            refused: Some(why), ..
        } = &ext.source
        {
            self.notify(format!("plugin {name} is refused: {why}"));
            return;
        }
        let call = AddonCall::OpenChecklist {
            addon: crate::addons::vocdoni::bridge::ADDON,
            id: name,
            origin: Origin::Row {
                back: Box::new(back),
            },
        };
        self.emit(Effect::Addon(call));
    }

    /// A key on the open checklist.
    pub(super) fn checklist_key(&mut self, key: &KeyEvent) {
        let Some(Modal::Checklist(view)) = &mut self.modal else {
            return;
        };
        let outcome = view.on_key(key);
        self.redraw = true;
        if outcome == Outcome::Stay {
            return;
        }
        let Some(Modal::Checklist(view)) = self.modal.take() else {
            return;
        };
        let answer = view.answer();
        let ChecklistView {
            addon,
            checklist,
            origin,
            ..
        } = *view;
        let review = |launch, answer| Effect::Addon(AddonCall::ReviewAnswer { launch, answer });
        match (outcome, origin) {
            (Outcome::Save, Origin::Launch { id, .. }) => {
                self.emit(review(id, ReviewAnswer::Save(answer)))
            }
            (Outcome::Skip, Origin::Launch { id, .. }) => self.emit(review(id, ReviewAnswer::Skip)),
            (_, Origin::Launch { id, sid }) => self.cancel_launch(id, sid),
            (outcome, origin) => {
                let answer = match outcome {
                    Outcome::Save => answer,
                    _ => ChecklistAnswer::Defer,
                };
                if let Origin::Row { back } = &origin {
                    self.modal = Some(Modal::ExtensionsMenu(*back.clone()));
                }
                if outcome == Outcome::Save || origin == Origin::Offer {
                    let id = checklist.id;
                    let call = AddonCall::SaveChecklist {
                        addon,
                        id,
                        answer,
                        origin,
                    };
                    self.emit(Effect::Addon(call));
                }
            }
        }
        self.show_next_confirm();
    }

    /// Esc at launch `id`'s review: the launch is dropped, and so is the
    /// manager's tab it was to fill (`--plain` ends with the launch).
    fn cancel_launch(&mut self, id: u64, sid: crate::proto::SessionId) {
        let answer = ReviewAnswer::Cancel;
        self.emit(Effect::Addon(AddonCall::ReviewAnswer {
            launch: id,
            answer,
        }));
        if self.mode == Mode::Manager {
            if let Some(i) = self.index_of(sid) {
                self.remove(i);
            }
            self.notify("launch cancelled");
        }
    }
}

#[cfg(test)]
mod tests;
