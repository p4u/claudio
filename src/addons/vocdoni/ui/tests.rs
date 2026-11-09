//! The Vocdoni checklists' tests: a plugin's setup as a checklist, saved
//! and deferred through the runner, and offered once at first sight.

use std::sync::Arc;

use super::*;
use crate::addon::{AddonEvent, Ui};
use crate::addons::vocdoni::plugins::choices::{self, PluginChoices};
use crate::addons::vocdoni::plugins::state::PluginsFetch;
use crate::addons::vocdoni::proxy::plugins::{sample, PluginList};
use crate::addons::vocdoni::state::Task;
use crate::addons::vocdoni::test_support::{lock, TestCtx, TEAM};
use crate::addons::vocdoni::Vocdoni;

fn sha() -> String {
    "a".repeat(64)
}

/// The addon with an active runner whose context is `ctx` (`local`
/// connected), `kb` listed for `work` and its manifest here.
fn vocdoni_with_kb(ctx: &Arc<TestCtx>) -> (Vocdoni, Arc<Runner>) {
    let runner = Arc::new(Runner::with_cache("/nonexistent/plugins.json".into()));
    runner.activate();
    let dyn_ctx: Arc<dyn AddonCtx> = ctx.clone();
    let up = AddonEvent::HostUp {
        host: "local".into(),
        features: vec!["addon:vocdoni".into()],
    };
    runner.on_event(&up, &dyn_ctx);
    let list = PluginList {
        profile: "work".into(),
        fetched_at: 1,
        plugins: vec![sample("kb")],
    };
    let manifest = Manifest::parse(&TEAM.replace("\"team\"", "\"kb\"")).unwrap();
    {
        let mut state = runner.state();
        state.on_list("work", PluginsFetch::Listed(list));
        state.on_manifest("kb", &sha(), Ok(manifest));
    }
    (Vocdoni::with_runner(Arc::clone(&runner)), runner)
}

#[test]
fn checklist_lists_manifest_items_with_saved_choices() {
    let ctx = Arc::new(TestCtx::default());
    let (v, runner) = vocdoni_with_kb(&ctx);
    let c = v.checklist("kb", ctx.as_ref()).expect("its checklist");
    assert_eq!(c.id, "kb");
    assert_eq!(c.revision, sha());
    assert_eq!(c.title, "Set up kb 1.0.0");
    let items: Vec<(&str, &str, Option<&str>, bool)> = c
        .items
        .iter()
        .map(|i| (i.id.as_str(), i.label.as_str(), i.section.as_deref(), i.on))
        .collect();
    assert_eq!(
        items[0],
        ("plugin:a@market", "a@market", Some("Plugins"), true)
    );
    assert!(items.contains(&(
        "mcp:kb",
        "kb (http https://kb.example.org/mcp)",
        Some("MCP servers"),
        true
    )));
    assert!(items.contains(&("agent:reviewer", "reviewer: Reviews", Some("Agents"), true)));
    assert!(c.items.iter().all(|i| i.on), "first sight: all on");
    // Saved choices turn their ids off.
    runner.state().plugins.choices.insert(
        "kb".into(),
        PluginChoices {
            disabled: vec!["mcp:kb".into()],
            ..PluginChoices::default()
        },
    );
    let c = v.checklist("kb", ctx.as_ref()).unwrap();
    let off: Vec<&str> = c
        .items
        .iter()
        .filter(|i| !i.on)
        .map(|i| i.id.as_str())
        .collect();
    assert_eq!(off, ["mcp:kb"]);
    // No manifest (or still fetching): no checklist.
    assert!(v.checklist("other", ctx.as_ref()).is_none());
    runner.state().plugins.manifests.get_mut("kb").unwrap().1 = ManifestState::Fetching;
    assert!(v.checklist("kb", ctx.as_ref()).is_none());
}

#[tokio::test]
async fn a_stale_revision_is_refused() {
    let ctx = Arc::new(TestCtx::default());
    let (v, runner) = vocdoni_with_kb(&ctx);
    let dyn_ctx: Arc<dyn AddonCtx> = ctx.clone();
    let stale = ChecklistAnswer::Save {
        revision: "b".repeat(64),
        off: Vec::new(),
    };
    let out = v.save_checklist("kb".into(), stale, dyn_ctx.clone()).await;
    assert_eq!(out, Err("stale".to_owned()));
    assert!(runner.state().plugins.choices.is_empty(), "nothing saved");
    assert!(ctx.config("plugins").is_empty());
    // The current revision saves: to `[plugins.kb]`, then said.
    let save = ChecklistAnswer::Save {
        revision: sha(),
        off: vec!["mcp:kb".into()],
    };
    v.save_checklist("kb".into(), save, dyn_ctx.clone())
        .await
        .unwrap();
    let saved = choices::from_table(&ctx.config("plugins"));
    assert_eq!(saved["kb"].disabled, ["mcp:kb"]);
    assert_eq!(saved["kb"].reviewed, "1.0.0");
    assert_eq!(runner.state().plugins.choices["kb"], saved["kb"]);
    assert_eq!(*lock(&ctx.notices), ["plugin kb set up (saved)"]);
    // Deferred: kept for this run, nothing written.
    let (v, runner) = vocdoni_with_kb(&Arc::new(TestCtx::default()));
    v.save_checklist("kb".into(), ChecklistAnswer::Defer, dyn_ctx)
        .await
        .unwrap();
    assert!(runner.state().plugins.deferred.contains("kb"));
    assert!(runner.state().plugins.choices.is_empty());
}

#[test]
fn first_sight_offers_once() {
    let ctx = Arc::new(TestCtx::default());
    let (_, runner) = vocdoni_with_kb(&ctx);
    // `vocdoni_with_kb`'s manifest arrived through the state directly:
    // the task it returned is the offer.
    let manifest = Manifest::parse(&TEAM.replace("\"team\"", "\"kb\"")).unwrap();
    let mut state = runner.state();
    assert!(state.offered.contains("kb"));
    // Fetched again (another hash): no second offer in this run.
    let other = "c".repeat(64);
    state.plugins.manifests.insert(
        "kb".into(),
        (
            other.clone(),
            crate::addons::vocdoni::plugins::state::ManifestState::Fetching,
        ),
    );
    let tasks = state.on_manifest("kb", &other, Ok(manifest.clone()));
    assert!(
        !tasks.iter().any(|t| matches!(t, Task::Offer(_))),
        "{tasks:?}"
    );
    // A fresh state offers on the manifest's arrival, once.
    let mut fresh = crate::addons::vocdoni::state::State::default();
    let list = PluginList {
        profile: "work".into(),
        fetched_at: 1,
        plugins: vec![sample("kb")],
    };
    fresh.on_list("work", PluginsFetch::Listed(list));
    let tasks = fresh.on_manifest("kb", &sha(), Ok(manifest));
    let offers: Vec<&Task> = tasks
        .iter()
        .filter(|t| matches!(t, Task::Offer(_)))
        .collect();
    assert_eq!(offers, [&Task::Offer("kb".into())]);
    // Saved or turned off: nothing to offer.
    let mut saved = crate::addons::vocdoni::state::State::default();
    saved
        .plugins
        .choices
        .insert("kb".into(), PluginChoices::default());
    let list = PluginList {
        profile: "work".into(),
        fetched_at: 1,
        plugins: vec![sample("kb")],
    };
    saved.on_list("work", PluginsFetch::Listed(list));
    let m = Manifest::parse(&TEAM.replace("\"team\"", "\"kb\"")).unwrap();
    let tasks = saved.on_manifest("kb", &sha(), Ok(m));
    assert!(!tasks.iter().any(|t| matches!(t, Task::Offer(_))));
}
