//! The plugin rows: the TUI's plugin-row tests (`tui::extensions_view`'s
//! `plugins_are_listed_with_their_state_on_each_host`,
//! `a_plugin_toggle_is_saved_and_kept_for_later_lists` and
//! `r_fetches_the_plugin_lists_again`), as the addon's rows.

use std::sync::Arc;

use super::*;
use crate::daemon::Applied;
use crate::plugins::choices::PluginChoices;
use crate::plugins::manifest::Manifest;
use crate::plugins::state::PluginsFetch;
use crate::proxy::plugins::{sample, PluginList};
use crate::state::Task;
use crate::test_support::TestCtx;
use crate::Vocdoni;
use claudio_core::addon::{AddonCtx, AddonError, AddonEvent, Ui};

/// Profile `work` serves `kb` and `latex`; `local` is connected; the core
/// holds `latex`.
fn served() -> State {
    let mut s = State {
        taken: ["latex".to_owned()].into(),
        ..State::default()
    };
    s.host_up("local", true);
    let list = PluginList {
        profile: "work".into(),
        fetched_at: 1,
        plugins: vec![sample("kb"), sample("latex"), sample("kb")],
    };
    s.on_list("work", PluginsFetch::Listed(list));
    s
}

#[test]
fn rows_carry_the_version_profile_and_each_hosts_state() {
    let mut s = served();
    let rows = rows(&s);
    let names: Vec<(&str, Option<&str>)> = rows
        .iter()
        .map(|r| (r.name.as_str(), r.refused.as_deref()))
        .collect();
    assert_eq!(
        names,
        [
            ("kb", None),
            ("latex", Some("name taken")),
            ("kb", Some("name given twice"))
        ]
    );
    assert_eq!(rows[0].description, "1.4.0 · work");
    assert_eq!(rows[0].setup.as_deref(), Some("kb"));
    assert!(rows[0].default_on);
    assert_eq!(
        rows[0].details,
        [
            "turning it on or off applies to new sessions",
            "manifest: fetching"
        ]
    );
    assert_eq!(
        rows[1].details,
        [
            "turning it on or off applies to new sessions",
            "refused: name taken"
        ]
    );
    let src = "format = 1\nname = \"kb\"\nversion = \"1.4.0\"\ndescription = \"KB\"";
    let sha = "a".repeat(64);
    s.plugins
        .choices
        .insert("kb".into(), PluginChoices::default());
    s.on_manifest("kb", &sha, Ok(Manifest::parse(src).unwrap()));
    assert_eq!(
        rows_of(&s)[1..],
        [
            "KB",
            "0 items disabled (Enter to change)",
            "local: applying"
        ]
    );
    let ready = Applied {
        name: "kb".into(),
        version: "1.4.0".into(),
        ..Default::default()
    };
    s.on_applied("local", "kb", &sha, Ok(ready));
    assert_eq!(
        rows_of(&s)[3],
        "local: ready · 0 installed, 0 present, 0 installing, 0 skipped"
    );
    let failed = AddonError::Daemon("manifest kb: download failed: HTTP 404".into());
    s.on_applied("local", "kb", &sha, Err(failed));
    assert_eq!(rows_of(&s)[3], "local: failed: download failed: HTTP 404");
}

/// The first row's details.
fn rows_of(s: &State) -> Vec<String> {
    rows(s).remove(0).details
}

/// The Vocdoni addon over `state`, started on `ctx`.
fn addon_with(state: State, ctx: &Arc<dyn AddonCtx>) -> Vocdoni {
    let runner = Arc::new(Runner::with_files(
        "/nonexistent/plugins.json".into(),
        "/nonexistent/config.toml".into(),
    ));
    *runner.state() = state;
    let v = Vocdoni::with_runner(runner);
    let up = AddonEvent::HostUp {
        host: "local".into(),
        features: vec!["addon:vocdoni".into()],
    };
    claudio_core::addon::Addon::on_event(&v, &up, ctx);
    v
}

#[tokio::test]
async fn a_toggle_turns_the_plugin_off_for_new_sessions_and_keeps_it_for_later_lists() {
    let test = Arc::new(TestCtx::default());
    *crate::test_support::lock(&test.taken) = vec!["latex".into()];
    let ctx: Arc<dyn AddonCtx> = test.clone();
    let mut state = served();
    state.loaded = true;
    let v = addon_with(state, &ctx);
    let parts = |v: &Vocdoni| v.runner.state().plugin_parts("local", "work", &[]).0.len();
    assert_eq!(parts(&v), 1);
    let off = v.row_action("kb".into(), RowAction::Toggled(false), Arc::clone(&ctx));
    assert_eq!(off.await, Ok(None));
    assert_eq!(parts(&v), 0);
    assert_eq!(v.runner.state().plugins.config.get("kb"), Some(&false));
    // Kept for a list that comes later.
    let list = PluginList {
        profile: "work".into(),
        fetched_at: 2,
        plugins: vec![sample("kb"), sample("latex")],
    };
    v.runner.state().on_list("work", PluginsFetch::Listed(list));
    assert_eq!(parts(&v), 0);
    let on = v.row_action("kb".into(), RowAction::Toggled(true), Arc::clone(&ctx));
    assert_eq!(on.await, Ok(None));
    assert_eq!(parts(&v), 1);
    // The core's names, from the context.
    let rows = v.extension_rows(ctx.as_ref());
    assert_eq!(rows[1].refused.as_deref(), Some("name taken"));
}

#[test]
fn a_refresh_fetches_every_list_in_use_again() {
    let mut s = served();
    s.default = Some("home".into());
    let fetched: Vec<String> = s
        .refresh()
        .into_iter()
        .filter_map(|t| match t {
            Task::FetchList { profile } => Some(profile),
            _ => None,
        })
        .collect();
    assert_eq!(fetched, ["home", "work"]);
}
