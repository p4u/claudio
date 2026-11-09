//! One view for every addon panel (docs/addons.md, "Data, not widgets"):
//! the panel's pages of blocks, its window and its actions. Keys: Esc/q
//! close, Left/Right/Tab/h/l (or 1-9) change the page, `w` the window, `r`
//! asks again, j/k/PgUp/PgDn/space/g/G scroll, and an action's key asks
//! its confirm prompt first (when it has one), then runs it.
//!
//! Pure, like `confirm.rs`: every page, window or refresh asks the addon
//! for the panel again ([`AddonCall::Panel`], with a new generation: an
//! older answer is dropped); drawn by `ui/panel.rs`.

use std::cell::Cell;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use claudio_core::addon::{
    ActionRequest, Addons, Notice, Panel, PanelAction, PanelDecl, PanelRequest,
};

use super::addon_calls::AddonCall;
use super::app::{App, Effect};
use super::confirm::{Choice, ConfirmAction, ConfirmPrompt};
use super::interaction::Modal;
use super::keymap::PanelBinding;

/// The addons' panels that have a key, in registry order:
/// [`Action::Panel`](super::keymap::Action::Panel)`(i)` opens the `i`th.
pub fn keyed_panels(addons: &Addons) -> Vec<(&'static str, PanelDecl)> {
    let panels = addons.panels().into_iter();
    panels.filter(|(_, p)| p.key.is_some()).collect()
}

/// The keymap's view of [`keyed_panels`].
pub fn panel_bindings(addons: &Addons) -> Vec<PanelBinding> {
    keyed_panels(addons)
        .into_iter()
        .filter_map(|(_, p)| {
            let key = p.key?;
            Some(PanelBinding {
                action: key.action,
                default: key.default,
                title: p.title,
            })
        })
        .collect()
}

/// An addon's panel on screen.
#[derive(Debug, Clone, PartialEq)]
pub struct PanelView {
    pub addon: &'static str,
    pub decl: PanelDecl,
    /// What the view asks for: its page, window and session.
    pub req: PanelRequest,
    /// The last answer (cleaned), if any.
    pub panel: Option<Panel>,
    scroll: u16,
    /// The largest useful scroll offset, as the renderer last measured it.
    max_scroll: Cell<u16>,
    /// An answer is awaited.
    pub loading: bool,
    /// The generation of the request awaited: older answers are dropped.
    pub gen: u64,
}

/// What a key did.
#[derive(Debug, PartialEq)]
pub enum PanelOutcome {
    Nothing,
    Close,
    /// Ask the addon again for `req`.
    Request,
    /// Run this action (after its confirm prompt, if any).
    Action(PanelAction),
}

impl PanelView {
    /// `decl` of `addon` about session `sid` (if any), asked under `gen`.
    pub fn new(
        addon: &'static str,
        decl: PanelDecl,
        sid: Option<claudio_core::proto::SessionId>,
        gen: u64,
    ) -> PanelView {
        PanelView {
            addon,
            decl,
            req: PanelRequest {
                id: decl.id.to_owned(),
                page: 0,
                window: decl.windows.first().map(|w| (*w).to_owned()),
                sid,
            },
            panel: None,
            scroll: 0,
            max_scroll: Cell::new(u16::MAX),
            loading: true,
            gen,
        }
    }

    /// The scroll offset to draw the page at.
    pub fn scroll(&self) -> u16 {
        self.scroll.min(self.max_scroll.get())
    }

    /// Record the largest useful scroll offset of the page being drawn.
    pub fn set_max_scroll(&self, max: u16) {
        self.max_scroll.set(max);
    }

    /// The pages the panel has (one until it answers).
    pub fn pages(&self) -> usize {
        self.panel.as_ref().map_or(1, |p| p.pages.len().max(1))
    }

    /// The addon's answer to generation `gen`; `false` when it is stale.
    pub fn answered(&mut self, gen: u64, panel: Panel) -> bool {
        if gen != self.gen {
            return false;
        }
        self.panel = Some(panel);
        self.loading = false;
        true
    }

    /// Handle a key.
    pub fn on_key(&mut self, key: &KeyEvent) -> PanelOutcome {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return PanelOutcome::Nothing;
        }
        let pages = self.pages();
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => PanelOutcome::Close,
            KeyCode::Right | KeyCode::Tab | KeyCode::Char('l') => {
                self.goto((self.req.page + 1) % pages)
            }
            KeyCode::Left | KeyCode::BackTab | KeyCode::Char('h') => {
                self.goto((self.req.page + pages - 1) % pages)
            }
            KeyCode::Char(c @ '1'..='9') if (c as usize - '1' as usize) < pages => {
                self.goto(c as usize - '1' as usize)
            }
            KeyCode::Char('w') if self.decl.windows.len() > 1 => {
                let windows = self.decl.windows;
                let at = windows
                    .iter()
                    .position(|w| Some(*w) == self.req.window.as_deref());
                let next = windows[at.map_or(0, |i| (i + 1) % windows.len())];
                self.req.window = Some(next.to_owned());
                self.scroll = 0;
                PanelOutcome::Request
            }
            KeyCode::Char('r') => PanelOutcome::Request,
            KeyCode::Down | KeyCode::Char('j') => self.scroll_to(self.scroll().saturating_add(1)),
            KeyCode::Up | KeyCode::Char('k') => self.scroll_to(self.scroll().saturating_sub(1)),
            KeyCode::PageDown | KeyCode::Char(' ') => {
                self.scroll_to(self.scroll().saturating_add(10))
            }
            KeyCode::PageUp => self.scroll_to(self.scroll().saturating_sub(10)),
            KeyCode::Home | KeyCode::Char('g') => self.scroll_to(0),
            KeyCode::End | KeyCode::Char('G') => self.scroll_to(u16::MAX),
            KeyCode::Char(c) => self.action_for(c),
            _ => PanelOutcome::Nothing,
        }
    }

    fn goto(&mut self, page: usize) -> PanelOutcome {
        if page != self.req.page {
            self.req.page = page;
            self.scroll = 0;
        }
        PanelOutcome::Request
    }

    fn scroll_to(&mut self, to: u16) -> PanelOutcome {
        self.scroll = to;
        PanelOutcome::Nothing
    }

    fn action_for(&self, c: char) -> PanelOutcome {
        let actions = self.panel.as_ref().map(|p| p.actions.as_slice());
        match actions
            .unwrap_or_default()
            .iter()
            .find(|a| a.key == Some(c))
        {
            Some(a) => PanelOutcome::Action(a.clone()),
            None => PanelOutcome::Nothing,
        }
    }
}

impl App {
    /// Open keyed panel `index` about the active session.
    pub(super) fn open_panel(&mut self, index: u8) {
        let Some((addon, decl)) = keyed_panels(&self.addons).get(usize::from(index)).copied()
        else {
            return;
        };
        self.modal_gen += 1;
        let sid = self.active_view().map(|v| v.id);
        let view = PanelView::new(addon, decl, sid, self.modal_gen);
        self.request_panel(&view);
        self.modal = Some(Modal::Panel(Box::new(view)));
        self.redraw = true;
    }

    fn request_panel(&mut self, view: &PanelView) {
        let (gen, addon, req) = (view.gen, view.addon, view.req.clone());
        self.emit(Effect::Addon(AddonCall::Panel { gen, addon, req }));
    }

    /// Ask the open panel's addon again, under a new generation.
    fn refresh_panel(&mut self) {
        self.modal_gen += 1;
        let gen = self.modal_gen;
        let Some(Modal::Panel(view)) = &mut self.modal else {
            return;
        };
        view.gen = gen;
        view.loading = true;
        let view = view.clone();
        self.request_panel(&view);
    }

    /// A key while a panel is open.
    pub(super) fn panel_key(&mut self, key: &KeyEvent) {
        let Some(Modal::Panel(view)) = &mut self.modal else {
            return;
        };
        let outcome = view.on_key(key);
        let (addon, req) = (view.addon, view.req.clone());
        self.redraw = true;
        match outcome {
            PanelOutcome::Nothing => {}
            PanelOutcome::Close => self.modal = None,
            PanelOutcome::Request => self.refresh_panel(),
            PanelOutcome::Action(action) => {
                let run = ActionRequest {
                    panel: req.id,
                    action: action.id.clone(),
                    sid: req.sid,
                };
                let Some(question) = action.confirm else {
                    self.emit(Effect::Addon(AddonCall::Action { addon, req: run }));
                    return;
                };
                // The panel gives way to the prompt; its key opens it again.
                self.modal = None;
                let choices = vec![
                    Choice::new(
                        'y',
                        "yes",
                        Some(ConfirmAction::AddonAction { addon, req: run }),
                    ),
                    Choice::new('n', "cancel", None),
                ];
                let prompt = ConfirmPrompt::new(action.label, question, choices);
                self.queue_confirm(prompt.ready());
            }
        }
    }

    /// The addon answered panel request `gen`.
    pub(super) fn on_panel(&mut self, gen: u64, panel: Panel) {
        if let Some(Modal::Panel(view)) = &mut self.modal {
            if view.answered(gen, panel) {
                self.redraw = true;
            }
        }
    }

    /// A panel action ended: its notice, and the open panel asked again.
    pub(super) fn on_action_done(&mut self, result: Result<Notice, String>) {
        match result {
            Ok(notice) => self.notify(notice.text),
            Err(why) => self.notify(why),
        }
        self.refresh_panel();
    }
}

#[cfg(test)]
mod tests;
