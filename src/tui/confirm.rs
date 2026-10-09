//! A small yes / no / skip popup, and what its answers do.
//!
//! [`ConfirmPrompt`] is generic: it carries the question, the action `y`
//! performs and, optionally, a "skip this version" to remember in state.json.
//! Prompts raised while another modal is open wait their turn in
//! `App::confirms`, and ignore keys for a moment after appearing so a stray
//! keystroke meant for a session cannot answer one.

use crossterm::event::{KeyCode, KeyEvent};

use super::app::App;
use super::interaction::Modal;

/// Ticks (250 ms each) during which a new prompt ignores the keyboard.
const ARM_TICKS: u8 = 4;

/// What `y` does.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfirmAction {
    /// Run `claude update` (or the installer) on `host`.
    UpdateClaude { host: String, install: bool },
}

/// A host and the version the user may choose to skip for it.
#[derive(Debug, Clone, PartialEq)]
pub struct Skip {
    pub host: String,
    pub version: String,
}

pub struct ConfirmPrompt {
    pub title: String,
    /// The question; may span lines.
    pub text: String,
    /// What `y` is called in the key hint: "update", "install"…
    pub yes_label: &'static str,
    pub yes: ConfirmAction,
    /// When set, `s` remembers it and the prompt offers the key.
    pub skip: Option<Skip>,
    /// Remaining ticks before keys count.
    pub(super) armed: u8,
}

impl ConfirmPrompt {
    pub fn new(
        title: impl Into<String>,
        text: impl Into<String>,
        yes_label: &'static str,
        yes: ConfirmAction,
        skip: Option<Skip>,
    ) -> Self {
        ConfirmPrompt {
            title: title.into(),
            text: text.into(),
            yes_label,
            yes,
            skip,
            armed: ARM_TICKS,
        }
    }
}

enum Answer {
    Yes,
    No,
    Skip,
}

impl App {
    /// Show `prompt` now, or as soon as no other modal is open.
    pub(super) fn queue_confirm(&mut self, prompt: ConfirmPrompt) {
        self.confirms.push_back(prompt);
        self.show_next_confirm();
    }

    fn show_next_confirm(&mut self) {
        if self.modal.is_none() {
            if let Some(prompt) = self.confirms.pop_front() {
                self.modal = Some(Modal::Confirm(prompt));
                self.redraw = true;
            }
        }
    }

    /// Per-tick upkeep: arm the open prompt, open a waiting one.
    pub(super) fn confirm_tick(&mut self) {
        if let Some(Modal::Confirm(prompt)) = &mut self.modal {
            prompt.armed = prompt.armed.saturating_sub(1);
        }
        self.show_next_confirm();
    }

    pub(super) fn confirm_key(&mut self, key: KeyEvent) {
        let Some(Modal::Confirm(prompt)) = &self.modal else {
            return;
        };
        let answer = match key.code {
            _ if prompt.armed > 0 => return,
            KeyCode::Char('y' | 'Y') => Answer::Yes,
            KeyCode::Char('n' | 'N') | KeyCode::Esc => Answer::No,
            KeyCode::Char('s' | 'S') if prompt.skip.is_some() => Answer::Skip,
            _ => return,
        };
        let Some(Modal::Confirm(prompt)) = self.modal.take() else {
            return;
        };
        match (answer, prompt.skip) {
            (Answer::Yes, _) => match prompt.yes {
                ConfirmAction::UpdateClaude { host, install } => {
                    self.start_claude_update(&host, install)
                }
            },
            (Answer::Skip, Some(skip)) => {
                self.claude_skipped.insert(skip.host, skip.version);
                self.save();
            }
            _ => {}
        }
        self.show_next_confirm();
    }
}
