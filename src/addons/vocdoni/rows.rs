//! The Vocdoni addon's extension rows (docs/addons.md, "Extension rows"):
//! one per plugin of every profile's list, refused when its list's checks
//! refuse it or its name is taken (by a core extension or an earlier row).
//! Its description is its version and profile; its details its manifest,
//! setup and each host's answer; Enter opens its setup (the checklist of
//! the same name). A toggle turns it on or off for new sessions; a refresh
//! fetches every list and manifest again and asks every host again.

use std::sync::Arc;

use super::runner::Runner;
use super::state::State;
use crate::addon::{ExtensionRow, Notice, RowAction};

/// Every plugin of every list, as rows.
pub(super) fn rows(state: &State) -> Vec<ExtensionRow> {
    state
        .rows()
        .into_iter()
        .map(|r| ExtensionRow {
            name: r.plugin.name.clone(),
            description: format!("{} · {}", r.plugin.version, r.profile),
            default_on: true,
            details: state.plugin_details(&r.plugin.name, r.profile, r.refused.as_deref()),
            refused: r.refused,
            setup: Some(r.plugin.name.clone()),
        })
        .collect()
}

/// What happened to row `row`: the core wrote `[extensions] <row>` (the
/// plugin on or off for new sessions; on, the hosts are asked for it), or
/// the member asked for a refresh.
pub(super) async fn action(
    runner: Arc<Runner>,
    row: String,
    action: RowAction,
) -> Result<Option<Notice>, String> {
    match action {
        RowAction::Toggled(on) => runner.transition(|s| s.set_enabled(&row, on)),
        RowAction::Refresh => runner.transition(|s| s.refresh()),
    }
    Ok(None)
}

#[cfg(test)]
mod tests;
