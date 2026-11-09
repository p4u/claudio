//! What the Vocdoni addon shows (docs/addons.md, "Data, not widgets"): a
//! proxied session's segments (its profile, credential and rate-limit
//! windows), the proxy panel and its switch action (`panel/`), and a
//! plugin's setup as a checklist, one item per manifest item. A
//! checklist's id is the plugin's name and its revision the manifest's
//! hash, so a save of items since replaced is refused as stale.

use std::sync::Arc;

use super::panel::{self, profile_of, tone_of};
use super::plugins::choices::choices_for;
use super::plugins::manifest::Manifest;
use super::plugins::state::ManifestState;
use super::runner::Runner;
use super::state::State;
use super::Vocdoni;
use crate::addon::{
    ActionRequest, AddonCtx, BoxFuture, Checklist, ChecklistAnswer, ChecklistItem, ExtensionRow,
    Notice, Panel, PanelDecl, PanelRequest, RowAction, Segment, SessionSnap, Tone, Ui,
};

/// The most characters of a credential's name a segment shows.
const NAME_CHARS: usize = 24;

impl Ui for Vocdoni {
    fn segments(&self, session: &SessionSnap, _ctx: &dyn AddonCtx) -> Vec<Segment> {
        segments(&self.runner.state(), session)
    }

    fn panels(&self) -> &'static [PanelDecl] {
        &[panel::DECL]
    }

    fn panel<'a>(&'a self, req: PanelRequest, ctx: Arc<dyn AddonCtx>) -> BoxFuture<'a, Panel> {
        Box::pin(panel::panel(Arc::clone(&self.runner), req, ctx))
    }

    fn action<'a>(
        &'a self,
        req: ActionRequest,
        ctx: Arc<dyn AddonCtx>,
    ) -> BoxFuture<'a, Result<Notice, String>> {
        Box::pin(panel::action(Arc::clone(&self.runner), req, ctx))
    }

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

    fn extension_rows(&self, _ctx: &dyn AddonCtx) -> Vec<ExtensionRow> {
        super::rows::rows(&self.runner.state())
    }

    fn row_action<'a>(
        &'a self,
        row: String,
        action: RowAction,
        _ctx: Arc<dyn AddonCtx>,
    ) -> BoxFuture<'a, Result<Option<Notice>, String>> {
        Box::pin(super::rows::action(Arc::clone(&self.runner), row, action))
    }
}

fn segment(id: &str, text: String, tone: Tone, priority: u8) -> Segment {
    Segment {
        id: id.to_owned(),
        text,
        tone,
        priority,
    }
}

/// A proxied session's segments: `⇄ proxy:<profile>` (10), and once the
/// proxy named its credential for this conversation `work-max (max)`
/// (20), `⇆ switched 3m ago` (21, for ten minutes), `5h 37%` (110) and
/// `7d 12%` (150). A direct session gets none.
pub(super) fn segments(state: &State, s: &SessionSnap) -> Vec<Segment> {
    let Some(profile) = profile_of(s) else {
        return Vec::new();
    };
    let mut out = vec![segment(
        "proxy",
        format!("⇄ proxy:{profile}"),
        Tone::Accent,
        10,
    )];
    let Some(cred) = state.cred(s.sid, s.conversation.as_deref()) else {
        return out;
    };
    if let Some(name) = cred.name() {
        let mut text: String = name.chars().take(NAME_CHARS).collect();
        if name.chars().count() > NAME_CHARS {
            text.pop();
            text.push('…');
        }
        if let Some(plan) = cred.plan() {
            text.push_str(&format!(" ({plan})"));
        }
        out.push(segment("credential", text, Tone::Accent, 20));
    }
    if let Some(age) = cred.recent_switch_age(state.now) {
        let text = format!("⇆ switched {} ago", panel::fmt_age(age));
        out.push(segment("switch", text, Tone::Warn, 21));
    }
    if let Some(pct) = cred.five_hour_pct() {
        let text = format!("5h {}%", pct.floor() as i64);
        out.push(segment("five-hour", text, tone_of(pct), 110));
    }
    if let Some(pct) = cred.seven_day_pct() {
        let text = format!("7d {}%", pct.floor() as i64);
        out.push(segment("seven-day", text, tone_of(pct), 150));
    }
    out
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
