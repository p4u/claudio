//! A session's daemon events as the hub applies them to its view: pure,
//! no I/O (`hub.rs` sends what changed).

use super::hub::SessionView;
use super::lenses;
use claudio_core::proto::{LensItem, SessionEvent, SessionState};

/// What [`apply_event`] made of one event.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Applied {
    pub changed: bool,
    pub removed: bool,
    pub lens: Option<(u64, Vec<LensItem>)>,
    pub notice: Option<String>,
}

/// Apply one daemon event to a session as `App::on_event` does for the
/// DTO's fields; `lens_host` says whether its daemon has the `lens`
/// feature. Returns whether the DTO changed and the `lens`/`latex` events
/// to send (if any). Pure: no I/O.
pub(crate) fn apply_event(
    view: &mut SessionView,
    event: SessionEvent,
    lens_host: bool,
    now: u64,
) -> Applied {
    let mut out = Applied::default();
    match event {
        // A respawn: same id, new process.
        SessionEvent::Created { info } => {
            let old = std::mem::replace(&mut view.info, info);
            let info = &mut view.info;
            if info.title.is_none() {
                info.title = old.title;
            }
            if info.claude_session_id.is_none() {
                info.claude_session_id = old.claude_session_id;
            }
            if info.branch.is_none() {
                info.branch = old.branch;
            }
            if info.model.is_none() {
                info.model = old.model;
            }
            if info.context_tokens.is_none() {
                info.context_tokens = old.context_tokens;
            }
            info.attention = None;
            view.state_since = now;
            view.lens_seq = 0;
            if let Some(p) = &mut view.prompt {
                p.on_state(view.info.state);
            }
            out.changed = true;
        }
        SessionEvent::Removed => out.removed = true,
        SessionEvent::State { state } => {
            if view.info.state != state {
                view.state_since = now;
            }
            view.info.state = state;
            if !matches!(
                state,
                SessionState::NeedsApproval | SessionState::NeedsInput
            ) {
                view.info.attention = None;
            }
            if let Some(p) = &mut view.prompt {
                p.on_state(state);
            }
            out.changed = true;
        }
        SessionEvent::Attention { attention } => {
            view.info.attention = Some(attention);
            out.changed = true;
        }
        SessionEvent::Exited { .. } => {
            if view.info.state != SessionState::Exited {
                view.state_since = now;
            }
            view.info.state = SessionState::Exited;
            view.info.pid = None;
            view.info.attention = None;
            out.changed = true;
        }
        SessionEvent::ClaudeSession { claude_session_id } => {
            view.info.claude_session_id = Some(claude_session_id);
            out.changed = true;
        }
        SessionEvent::ClaudeSessionCleared => {
            view.info.claude_session_id = None;
            out.changed = true;
        }
        SessionEvent::Title { title } => {
            view.info.title = Some(title);
            out.changed = true;
        }
        SessionEvent::Renamed { name } => {
            view.info.name = name;
            out.changed = true;
        }
        SessionEvent::Notice { text } => out.notice = Some(text),
        // `None` is "no news", never "cleared".
        SessionEvent::Meta {
            branch,
            model,
            context_tokens,
            git,
            output_tokens,
            turns,
        } => {
            let mut changed = false;
            if branch.is_some() {
                changed |= view.info.branch != branch;
                view.info.branch = branch;
            }
            if model.is_some() {
                changed |= view.info.model != model;
                view.info.model = model;
            }
            if context_tokens.is_some() {
                changed |= view.info.context_tokens != context_tokens;
                view.info.context_tokens = context_tokens;
            }
            if git.is_some() {
                changed |= view.git != git;
                view.git = git;
            }
            if output_tokens.is_some() {
                changed |= view.output_tokens != output_tokens;
                view.output_tokens = output_tokens;
            }
            if turns.is_some() {
                changed |= view.turns != turns;
                view.turns = turns;
            }
            out.changed = changed;
        }
        e @ (SessionEvent::Latex { .. } | SessionEvent::Lens { .. }) => {
            out.lens = lenses::relay(view, e, lens_host);
        }
        SessionEvent::Unknown => {}
    }
    out
}

/// The daemon (re)attached the session: its size, and any lens `seq` is
/// taken next (a restarted daemon counts from 1 again).
pub(crate) fn on_attached(view: &mut SessionView, rows: u16, cols: u16) {
    view.size = Some((rows, cols));
    view.lens_seq = 0;
}
