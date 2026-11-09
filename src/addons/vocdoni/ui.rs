//! What the Vocdoni addon shows (docs/addons.md, "Data, not widgets"): a
//! plugin's setup as a checklist, one item per manifest item. Its id is
//! the plugin's name and its revision the manifest's hash, so a save of
//! items since replaced is refused as stale.

use std::sync::Arc;

use super::plugins::choices::choices_for;
use super::plugins::manifest::Manifest;
use super::plugins::state::ManifestState;
use super::runner::Runner;
use super::Vocdoni;
use crate::addon::{AddonCtx, BoxFuture, Checklist, ChecklistAnswer, ChecklistItem, Ui};

impl Ui for Vocdoni {
    fn checklist(&self, id: &str, _ctx: &dyn AddonCtx) -> Option<Checklist> {
        checklist(&self.runner, id)
    }

    fn save_checklist<'a>(
        &'a self,
        id: String,
        answer: ChecklistAnswer,
        ctx: Arc<dyn AddonCtx>,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(save(Arc::clone(&self.runner), id, answer, ctx))
    }
}

/// Plugin `id`'s setup, once its manifest is here: every item on but those
/// its saved choices turn off (all on at first sight).
pub(super) fn checklist(runner: &Runner, id: &str) -> Option<Checklist> {
    let state = runner.state();
    let (sha, m) = match state.plugins.manifests.get(id) {
        Some((sha, ManifestState::Ready(m))) => (sha.clone(), m),
        _ => return None,
    };
    let disabled = state
        .plugins
        .choices
        .get(id)
        .map(|c| c.disabled.as_slice())
        .unwrap_or_default();
    Some(of_manifest(m, sha, disabled))
}

/// `m`'s checklist at hash `sha`, with `disabled` off.
fn of_manifest(m: &Manifest, sha: String, disabled: &[String]) -> Checklist {
    let items = m
        .items()
        .into_iter()
        .map(|i| ChecklistItem {
            on: !disabled.contains(&i.id),
            id: i.id,
            label: i.title,
            detail: None,
            section: Some(i.section.heading().to_owned()),
        })
        .collect();
    Checklist {
        id: m.name.clone(),
        revision: sha,
        title: format!("Set up {} {}", m.name, m.version),
        intro: m.description.trim().to_owned(),
        items,
    }
}

/// The member's answer to plugin `id`'s setup: saved choices go to
/// `[plugins.<id>]` and every host is asked again; a revision that is not
/// the manifest's now is refused (`stale`). Deferred: nothing is asked for
/// it in this run until it is saved.
async fn save(
    runner: Arc<Runner>,
    id: String,
    answer: ChecklistAnswer,
    ctx: Arc<dyn AddonCtx>,
) -> Result<(), String> {
    let (revision, off) = match answer {
        ChecklistAnswer::Defer => {
            runner.transition(|s| {
                s.defer(&id);
                Vec::new()
            });
            return Ok(());
        }
        ChecklistAnswer::Save { revision, off } => (revision, off),
    };
    let choices = {
        let state = runner.state();
        let m = match state.plugins.manifests.get(&id) {
            Some((sha, ManifestState::Ready(m))) if *sha == revision => m,
            Some(_) => return Err("stale".to_owned()),
            None => return Err("no such plugin".to_owned()),
        };
        choices_for(m, &off, state.plugins.choices.get(&id))
    };
    runner.save(&id, choices).await?;
    ctx.notify(format!("plugin {id} set up (saved)"));
    Ok(())
}

#[cfg(test)]
mod tests;
