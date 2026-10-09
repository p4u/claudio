//! A small multiple-choice popup, and what its answers do.
//!
//! [`ConfirmPrompt`] is generic: it carries the question and the keys that
//! answer it, each with the [`ConfirmAction`] it runs (or none, to just close).
//! `Esc` always closes. Prompts raised while another modal is open wait their
//! turn in `App::confirms`, and ignore keys for a moment after appearing so a
//! stray keystroke meant for a session cannot answer one.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::proto::SessionId;

use super::app::App;
use super::interaction::Modal;

/// Ticks (250 ms each) during which a new prompt ignores the keyboard.
const ARM_TICKS: u8 = 4;

/// What a key does.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfirmAction {
    /// Run `claude update` (or the installer) on `host`.
    UpdateClaude { host: String, install: bool },
    /// Remember not to ask about this version again.
    SkipClaude(Skip),
    /// Restart session `id` in its tab; `fresh` starts a new conversation.
    Reset { id: SessionId, fresh: bool },
    /// Kill session `id` and close its tab.
    Kill(SessionId),
}

/// A host and the version the user may choose to skip for it.
#[derive(Debug, Clone, PartialEq)]
pub struct Skip {
    pub host: String,
    pub version: String,
}

/// One key of a prompt.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    pub key: KeyCode,
    /// What the key is called in the hint: "update", "skip this version"…
    pub label: &'static str,
    /// `None` just closes the prompt.
    pub action: Option<ConfirmAction>,
}

impl Choice {
    /// A letter key (lowercase) that runs `action`, or just closes.
    pub fn new(key: char, label: &'static str, action: Option<ConfirmAction>) -> Self {
        Choice {
            key: KeyCode::Char(key),
            label,
            action,
        }
    }

    /// `Esc`, listed in the hint under `label`. `Esc` closes a prompt whether
    /// or not it is listed.
    pub fn esc(label: &'static str) -> Self {
        Choice {
            key: KeyCode::Esc,
            label,
            action: None,
        }
    }

    /// The hint for this key: `[y] update` or `Esc cancel`.
    pub fn hint(&self) -> String {
        match self.key {
            KeyCode::Char(c) => format!("[{c}] {}", self.label),
            _ => format!("Esc {}", self.label),
        }
    }
}

pub struct ConfirmPrompt {
    pub title: String,
    /// The question; may span lines.
    pub text: String,
    pub choices: Vec<Choice>,
    /// Remaining ticks before keys count.
    pub(super) armed: u8,
}

impl ConfirmPrompt {
    pub fn new(title: impl Into<String>, text: impl Into<String>, choices: Vec<Choice>) -> Self {
        ConfirmPrompt {
            title: title.into(),
            text: text.into(),
            choices,
            armed: ARM_TICKS,
        }
    }

    /// Take keys at once: for a prompt the user just asked for.
    pub fn ready(mut self) -> Self {
        self.armed = 0;
        self
    }

    /// The choice `code` selects. Letters match in either case.
    fn choice_for(&self, code: KeyCode) -> Option<&Choice> {
        let code = match code {
            KeyCode::Char(c) => KeyCode::Char(c.to_ascii_lowercase()),
            other => other,
        };
        self.choices.iter().find(|c| c.key == code)
    }
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
        // Ctrl+Y or Alt+N are not answers; Shift only makes a capital.
        if prompt.armed > 0 || !key.modifiers.difference(KeyModifiers::SHIFT).is_empty() {
            return;
        }
        let action = match prompt.choice_for(key.code) {
            Some(choice) => choice.action.clone(),
            None if key.code == KeyCode::Esc => None,
            None => return,
        };
        self.modal = None;
        match action {
            Some(ConfirmAction::UpdateClaude { host, install }) => {
                self.start_claude_update(&host, install)
            }
            Some(ConfirmAction::SkipClaude(skip)) => {
                self.claude_skipped.insert(skip.host, skip.version);
                self.save();
            }
            Some(ConfirmAction::Reset { id, fresh }) => self.reset_session(id, fresh),
            Some(ConfirmAction::Kill(id)) => self.kill_session(id),
            None => {}
        }
        self.show_next_confirm();
    }
}
